use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use knightty_core::{
    Damage, GridSnapshot, Hyperlink, ImageId, ImagePlacementId, MouseButton as TerminalMouseButton,
    MouseEventKind, MouseProtocol, SelectionMode, SelectionPoint, Terminal, TerminalMouseEvent,
};
use knightty_proto::iterm2::parse_iterm2_inline_image;
use knightty_proto::kitty::{
    GraphicsEscapeRouter, GraphicsStreamItem, KittyAction, KittyCommand, KittyContinuation,
    KittyErrorCode, KittyImageKey, KittyPlacementKey, KittyProtocolError, KittyResponseContext,
    MAX_KITTY_CHUNK_BYTES, ParsedKittyCommand, encode_response, parse_kitty_command,
    response_context,
};
use knightty_render::{CellMetrics, ImePreedit, InlineImageData, SearchMatch};
use thiserror::Error;
use url::Url;
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Cursor, CursorIcon, Window};

use crate::config::AppConfig;
use crate::graphics_diagnostics;
use crate::inline_image::{
    DecodedImage, ImageLimits, InlineImageError, PtyStreamItem, decode_png, decode_png_payload,
    pending_inline_image_is_oversized, placement_for_image, placement_for_kitty, scan_pty_stream,
};

pub trait ClipboardPort {
    fn get_text(&mut self) -> Result<String, String>;
    fn set_text(&mut self, text: String) -> Result<(), String>;
}

pub trait PtyWritePort {
    fn write_pty(&mut self, bytes: &[u8]);
}

pub trait UrlOpenPort {
    fn open_url(&mut self, url: String);
}

pub trait CursorPort {
    fn set_cursor_icon(&mut self, icon: CursorIcon);
}

pub trait InputPorts: ClipboardPort + PtyWritePort + UrlOpenPort + CursorPort {}

impl<T> InputPorts for T where T: ClipboardPort + PtyWritePort + UrlOpenPort + CursorPort {}

#[derive(Default)]
pub struct RuntimeInputPorts {
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    window: Option<Arc<Window>>,
}

impl RuntimeInputPorts {
    pub fn set_writer(&mut self, writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>) {
        self.writer = writer;
    }

    pub fn set_window(&mut self, window: Option<Arc<Window>>) {
        self.window = window;
    }
}

impl ClipboardPort for RuntimeInputPorts {
    fn get_text(&mut self) -> Result<String, String> {
        Clipboard::new()
            .and_then(|mut clipboard| clipboard.get_text())
            .map_err(|error| error.to_string())
    }

    fn set_text(&mut self, text: String) -> Result<(), String> {
        Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text))
            .map_err(|error| error.to_string())
    }
}

impl PtyWritePort for RuntimeInputPorts {
    fn write_pty(&mut self, bytes: &[u8]) {
        let Some(writer) = &self.writer else {
            return;
        };

        if let Ok(mut writer) = writer.lock() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }
}

impl UrlOpenPort for RuntimeInputPorts {
    fn open_url(&mut self, url: String) {
        let _ = thread::spawn(move || {
            if let Err(error) = open::that(&url) {
                eprintln!("knightty hyperlink: failed to open `{url}`: {error}");
            }
        });
    }
}

impl CursorPort for RuntimeInputPorts {
    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if let Some(window) = &self.window {
            window.set_cursor(Cursor::Icon(icon));
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct KittyTransmitMetadata {
    display: bool,
    image_id: Option<KittyImageKey>,
    placement_id: Option<u32>,
    columns: Option<u16>,
    rows: Option<u16>,
    cursor_movement: bool,
}

impl KittyTransmitMetadata {
    fn from_command(command: &KittyCommand<'_>, display: bool) -> Self {
        Self {
            display,
            image_id: command.image_id,
            placement_id: command.placement_id,
            columns: command.columns,
            rows: command.rows,
            cursor_movement: command.cursor_movement,
        }
    }
}

#[derive(Debug)]
struct PartialKittyUpload {
    metadata: KittyTransmitMetadata,
    response_context: KittyResponseContext,
    encoded: Vec<u8>,
}

pub struct InputRouter<P> {
    config: AppConfig,
    cell_metrics: CellMetrics,
    terminal: Terminal,
    graphics_escape_router: GraphicsEscapeRouter,
    pending_pty_bytes: Vec<u8>,
    inline_images: BTreeMap<ImageId, DecodedImage>,
    inline_image_bytes: usize,
    kitty_image_ids: BTreeMap<KittyImageKey, ImageId>,
    kitty_placement_ids: BTreeMap<KittyPlacementKey, ImagePlacementId>,
    partial_kitty_upload: Option<PartialKittyUpload>,
    next_image_id: u64,
    next_placement_id: u64,
    ports: P,
    modifiers: ModifiersState,
    cursor_cell: Option<(usize, usize)>,
    pressed_mouse_button: Option<TerminalMouseButton>,
    selection_drag_active: bool,
    selection_drag_anchor: Option<SelectionPoint>,
    selection_drag_mode: Option<SelectionMode>,
    selection_drag_moved: bool,
    hovered_hyperlink: Option<HoveredHyperlink>,
    pending_hyperlink_click: Option<PendingHyperlinkClick>,
    hover_visual_dirty: bool,
    search_visual_dirty: bool,
    ime_visual_dirty: bool,
    ime_preedit: Option<ImePreedit>,
    search: SearchState,
    config_reload_requested: bool,
    current_cursor_icon: CursorIcon,
    click_tracker: ClickTracker,
}

impl<P: InputPorts> InputRouter<P> {
    pub fn new(config: AppConfig, ports: P) -> Self {
        let initial_cols = config.initial_cols();
        let initial_rows = config.initial_rows();
        let scrollback_lines = config.scrollback_lines();
        let cell_metrics = config.renderer_config().cell_metrics;
        let max_encoded_bytes = config.graphics.max_encoded_bytes;
        Self {
            config,
            cell_metrics,
            terminal: Terminal::with_scrollback(initial_cols, initial_rows, scrollback_lines),
            graphics_escape_router: GraphicsEscapeRouter::new(max_encoded_bytes),
            pending_pty_bytes: Vec::new(),
            inline_images: BTreeMap::new(),
            inline_image_bytes: 0,
            kitty_image_ids: BTreeMap::new(),
            kitty_placement_ids: BTreeMap::new(),
            partial_kitty_upload: None,
            next_image_id: 1,
            next_placement_id: 1,
            ports,
            modifiers: ModifiersState::empty(),
            cursor_cell: None,
            pressed_mouse_button: None,
            selection_drag_active: false,
            selection_drag_anchor: None,
            selection_drag_mode: None,
            selection_drag_moved: false,
            hovered_hyperlink: None,
            pending_hyperlink_click: None,
            hover_visual_dirty: false,
            search_visual_dirty: false,
            ime_visual_dirty: false,
            ime_preedit: None,
            search: SearchState::default(),
            config_reload_requested: false,
            current_cursor_icon: CursorIcon::Default,
            click_tracker: ClickTracker::default(),
        }
    }

    #[cfg(test)]
    pub fn ports(&self) -> &P {
        &self.ports
    }

    pub fn ports_mut(&mut self) -> &mut P {
        &mut self.ports
    }

    pub fn size(&self) -> (usize, usize) {
        self.terminal.size()
    }

    pub fn update_config(&mut self, config: AppConfig) {
        self.partial_kitty_upload = None;
        self.cell_metrics = config.renderer_config().cell_metrics;
        self.graphics_escape_router
            .set_max_payload_bytes(config.graphics.max_encoded_bytes);
        if !config.graphics.enabled {
            self.terminal.clear_image_placements();
            self.inline_images.clear();
            self.inline_image_bytes = 0;
            self.kitty_image_ids.clear();
            self.kitty_placement_ids.clear();
        }
        self.config = config;
    }

    pub fn take_config_reload_request(&mut self) -> bool {
        let requested = self.config_reload_requested;
        self.config_reload_requested = false;
        requested
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.terminal.resize(cols, rows);
        self.update_cursor_icon();
    }

    pub fn snapshot(&self) -> GridSnapshot {
        self.terminal.snapshot()
    }

    pub fn inline_images_for_snapshot(&self, snapshot: &GridSnapshot) -> Vec<InlineImageData> {
        let mut seen = BTreeSet::new();
        snapshot
            .image_placements
            .iter()
            .filter_map(|placement| {
                if !seen.insert(placement.image_id) {
                    return None;
                }
                let image = self.inline_images.get(&placement.image_id)?;
                Some(InlineImageData {
                    id: placement.image_id,
                    width: image.width,
                    height: image.height,
                    rgba: Arc::clone(&image.rgba),
                })
            })
            .collect()
    }

    pub fn search_query_for_render(&self) -> Option<String> {
        self.search.active.then(|| self.search.query.clone())
    }

    pub fn ime_preedit_for_render(&self) -> Option<ImePreedit> {
        if self.search.active {
            None
        } else {
            self.ime_preedit.clone()
        }
    }

    pub fn search_matches_for_snapshot(
        &self,
        snapshot: &GridSnapshot,
    ) -> (Vec<SearchMatch>, Option<SearchMatch>) {
        if !self.search.active || self.search.query.is_empty() {
            return (Vec::new(), None);
        }

        let matches = find_search_matches(snapshot, &self.search.query);
        let current_match = if matches.is_empty() {
            None
        } else {
            matches
                .get(self.search.current_index % matches.len())
                .cloned()
        };
        (matches, current_match)
    }

    pub fn take_render_damage(&mut self) -> Damage {
        let damage = if self.hover_visual_dirty || self.search_visual_dirty || self.ime_visual_dirty
        {
            let _ = self.terminal.take_damage();
            Damage::Full
        } else {
            self.terminal.take_damage()
        };
        self.hover_visual_dirty = false;
        self.search_visual_dirty = false;
        self.ime_visual_dirty = false;
        damage
    }

    pub fn feed_pty_bytes(&mut self, bytes: &[u8]) -> Option<String> {
        let stream_items = self.graphics_escape_router.feed(bytes);
        let mut window_title = None;
        for item in stream_items {
            match item {
                GraphicsStreamItem::Vte(bytes) => {
                    self.feed_vte_bytes(&bytes, &mut window_title);
                }
                GraphicsStreamItem::Kitty(command) => {
                    graphics_diagnostics::record_completed_kitty_command(command.len());
                    self.handle_kitty_command(&command);
                }
                GraphicsStreamItem::OversizedKitty { control_data } => {
                    if self.config.graphics.enabled {
                        let context = self.partial_kitty_upload.take().map_or_else(
                            || response_context(&control_data),
                            |upload| upload.response_context,
                        );
                        self.write_kitty_response(
                            context,
                            Err(KittyProtocolError::new(
                                KittyErrorCode::TooBig,
                                "graphics payload exceeds configured limit",
                            )),
                        );
                    }
                }
            }
        }

        self.evict_unreferenced_inline_images();
        self.refresh_hover_hyperlink();
        window_title
    }

    fn feed_vte_bytes(&mut self, bytes: &[u8], window_title: &mut Option<String>) {
        let mut input = core::mem::take(&mut self.pending_pty_bytes);
        input.extend_from_slice(bytes);
        let scan = scan_pty_stream(&input);

        for item in scan.items {
            match item {
                PtyStreamItem::Text(text) => {
                    self.feed_terminal_bytes(text, window_title);
                }
                PtyStreamItem::InlineImage(sequence) => self.handle_inline_image(sequence),
                PtyStreamItem::InvalidInlineImage => {
                    eprintln!("knightty inline image: ignored non-UTF-8 OSC 1337 metadata");
                }
            }
        }

        if scan.consumed < input.len() {
            self.pending_pty_bytes
                .extend_from_slice(&input[scan.consumed..]);
            let limits = ImageLimits::from(&self.config.graphics);
            if pending_inline_image_is_oversized(&self.pending_pty_bytes, limits) {
                eprintln!("knightty inline image: discarded unterminated oversized payload");
                self.pending_pty_bytes.clear();
            }
        }
    }

    fn feed_terminal_bytes(&mut self, bytes: &[u8], window_title: &mut Option<String>) {
        let reset_images = bytes.windows(2).any(|window| window == b"\x1bc");
        self.terminal.feed(bytes);
        if reset_images {
            self.partial_kitty_upload = None;
            self.kitty_image_ids.clear();
            self.kitty_placement_ids.clear();
        }
        for response in self.terminal.take_pty_writes() {
            self.write_pty(response.as_bytes());
        }
        if let Some(title) = self.terminal.take_window_title_changed() {
            *window_title = Some(title);
        }
    }

    fn handle_inline_image(&mut self, sequence: &str) {
        if !self.config.graphics.enabled {
            return;
        }
        self.evict_unreferenced_inline_images();

        let command = format!("File={sequence}");
        let parsed = match parse_iterm2_inline_image(&command) {
            Ok(parsed) => parsed,
            Err(error) => {
                eprintln!("knightty inline image: ignored invalid metadata ({error:?})");
                return;
            }
        };
        let limits = ImageLimits::from(&self.config.graphics);
        if self.inline_images.len() >= limits.max_images {
            eprintln!("knightty inline image: image count limit reached");
            return;
        }
        let image = match decode_png(&parsed, limits) {
            Ok(image) => image,
            Err(error) => {
                eprintln!("knightty inline image: ignored image ({error})");
                return;
            }
        };
        let Some(total_bytes) = self.inline_image_bytes.checked_add(image.rgba.len()) else {
            eprintln!("knightty inline image: GPU byte accounting overflow");
            return;
        };
        if total_bytes > limits.max_gpu_bytes {
            eprintln!("knightty inline image: GPU byte limit reached");
            return;
        }

        let id = self.allocate_image_id();
        let placement_id = self.allocate_placement_id();
        let (terminal_columns, _) = self.terminal.size();
        let placement = match placement_for_image(
            id,
            placement_id,
            &image,
            &parsed,
            self.terminal.image_anchor(),
            terminal_columns,
            self.cell_metrics,
        ) {
            Ok(placement) => placement,
            Err(error) => {
                eprintln!("knightty inline image: ignored placement ({error})");
                return;
            }
        };

        self.inline_image_bytes = total_bytes;
        self.inline_images.insert(id, image);
        self.terminal.add_image_placement(placement);
        self.terminal.advance_after_image_rows(placement.rows);
    }

    fn handle_kitty_command(&mut self, input: &[u8]) {
        if !self.config.graphics.enabled {
            return;
        }

        let context = response_context(input);
        let parsed = match parse_kitty_command(input) {
            Ok(parsed) => parsed,
            Err(error) => {
                let context = self
                    .partial_kitty_upload
                    .take()
                    .map_or(context, |upload| upload.response_context);
                self.write_kitty_response(context, Err(error.protocol_error()));
                return;
            }
        };

        let command = match parsed {
            ParsedKittyCommand::Command(command) => command,
            ParsedKittyCommand::Continuation(continuation) => {
                self.handle_kitty_continuation(continuation);
                return;
            }
        };

        // The protocol permits only one multipart transfer at a time. Any new
        // complete graphics command explicitly recovers from an unfinished one.
        self.partial_kitty_upload = None;

        let result = match command.action {
            KittyAction::TransmitAndDisplay => self.handle_kitty_transmit(&command, true, context),
            KittyAction::Transmit => self.handle_kitty_transmit(&command, false, context),
            KittyAction::Place => Some(self.handle_kitty_place(&command)),
            KittyAction::Delete => {
                self.handle_kitty_delete(&command);
                return;
            }
        };
        if let Some(result) = result {
            self.write_kitty_response(context, result);
        }
    }

    fn handle_kitty_transmit(
        &mut self,
        command: &KittyCommand<'_>,
        display: bool,
        response_context: KittyResponseContext,
    ) -> Option<Result<(), KittyProtocolError>> {
        let metadata = KittyTransmitMetadata::from_command(command, display);
        if command.more_chunks {
            let result = self.begin_kitty_upload(metadata, response_context, command.payload);
            return result.err().map(Err);
        }
        Some(self.commit_kitty_transmit(metadata, command.payload))
    }

    fn begin_kitty_upload(
        &mut self,
        metadata: KittyTransmitMetadata,
        response_context: KittyResponseContext,
        payload: &[u8],
    ) -> Result<(), KittyProtocolError> {
        validate_kitty_chunk(payload, true)?;
        if payload.len() > self.config.graphics.max_encoded_bytes {
            return Err(KittyProtocolError::new(
                KittyErrorCode::TooBig,
                "graphics payload exceeds configured limit",
            ));
        }
        self.partial_kitty_upload = Some(PartialKittyUpload {
            metadata,
            response_context,
            encoded: payload.to_vec(),
        });
        Ok(())
    }

    fn handle_kitty_continuation(&mut self, continuation: KittyContinuation<'_>) {
        let Some(mut upload) = self.partial_kitty_upload.take() else {
            let context = KittyResponseContext {
                quiet: continuation.quiet.unwrap_or_default(),
                ..KittyResponseContext::default()
            };
            self.write_kitty_response(
                context,
                Err(KittyProtocolError::new(
                    KittyErrorCode::Invalid,
                    "no multipart upload is in progress",
                )),
            );
            return;
        };

        if let Some(quiet) = continuation.quiet {
            upload.response_context.quiet = quiet;
        }
        let context = upload.response_context;
        if let Err(error) = self.append_kitty_chunk(
            &mut upload.encoded,
            continuation.payload,
            continuation.more_chunks,
        ) {
            self.write_kitty_response(context, Err(error));
            return;
        }
        if continuation.more_chunks {
            self.partial_kitty_upload = Some(upload);
            return;
        }

        let result = self.commit_kitty_transmit(upload.metadata, &upload.encoded);
        self.write_kitty_response(context, result);
    }

    fn append_kitty_chunk(
        &self,
        encoded: &mut Vec<u8>,
        payload: &[u8],
        more_chunks: bool,
    ) -> Result<(), KittyProtocolError> {
        validate_kitty_chunk(payload, more_chunks)?;
        let total = encoded.len().checked_add(payload.len()).ok_or_else(|| {
            KittyProtocolError::new(KittyErrorCode::TooBig, "graphics payload size overflow")
        })?;
        if total > self.config.graphics.max_encoded_bytes {
            return Err(KittyProtocolError::new(
                KittyErrorCode::TooBig,
                "graphics payload exceeds configured limit",
            ));
        }
        encoded.extend_from_slice(payload);
        Ok(())
    }

    fn commit_kitty_transmit(
        &mut self,
        metadata: KittyTransmitMetadata,
        payload: &[u8],
    ) -> Result<(), KittyProtocolError> {
        self.evict_unreferenced_inline_images();
        let limits = ImageLimits::from(&self.config.graphics);
        let image = decode_png_payload(payload, limits).map_err(kitty_image_error)?;
        graphics_diagnostics::record_decode_success(payload.len(), image.width, image.height);
        let existing_id = metadata
            .image_id
            .and_then(|key| self.kitty_image_ids.get(&key).copied());
        let should_store = metadata.image_id.is_some() || metadata.display;
        if !should_store {
            return Ok(());
        }

        let previous_bytes = existing_id
            .and_then(|id| self.inline_images.get(&id))
            .map_or(0, |image| image.rgba.len());
        let prospective_count = self
            .inline_images
            .len()
            .saturating_sub(usize::from(existing_id.is_some()))
            .saturating_add(1);
        if prospective_count > limits.max_images {
            return Err(KittyProtocolError::new(
                KittyErrorCode::NoSpace,
                "image count limit reached",
            ));
        }
        let prospective_bytes = self
            .inline_image_bytes
            .saturating_sub(previous_bytes)
            .checked_add(image.rgba.len())
            .ok_or_else(|| {
                KittyProtocolError::new(KittyErrorCode::NoSpace, "image byte quota overflow")
            })?;
        if prospective_bytes > limits.max_gpu_bytes {
            return Err(KittyProtocolError::new(
                KittyErrorCode::NoSpace,
                "image byte quota reached",
            ));
        }

        let internal_id = self.allocate_image_id();
        let placement = if metadata.display {
            let placement_id = self.allocate_placement_id();
            Some(
                placement_for_kitty(
                    internal_id,
                    placement_id,
                    &image,
                    metadata.columns,
                    metadata.rows,
                    self.terminal.image_anchor(),
                    self.cell_metrics,
                )
                .map_err(kitty_image_error)?,
            )
        } else {
            None
        };

        if let Some(existing_id) = existing_id {
            self.terminal.remove_image_placements(existing_id);
            self.inline_images.remove(&existing_id);
        }
        if let Some(key) = metadata.image_id {
            self.kitty_placement_ids
                .retain(|placement, _| placement.client_image_id != key.client_id);
            self.kitty_image_ids.insert(key, internal_id);
        }
        self.inline_images.insert(internal_id, image);
        self.inline_image_bytes = prospective_bytes;

        if let Some(placement) = placement {
            self.terminal.upsert_image_placement(placement);
            graphics_diagnostics::record_placement_created();
            if let (Some(image_key), Some(client_placement_id)) =
                (metadata.image_id, metadata.placement_id)
            {
                self.kitty_placement_ids.insert(
                    KittyPlacementKey {
                        client_image_id: image_key.client_id,
                        placement_id: client_placement_id,
                    },
                    placement.placement_id,
                );
            }
            if metadata.cursor_movement {
                self.terminal
                    .move_after_kitty_image(placement.columns, placement.rows);
            }
        }
        Ok(())
    }

    fn handle_kitty_place(&mut self, command: &KittyCommand<'_>) -> Result<(), KittyProtocolError> {
        let image_key = command.image_id.expect("parser requires an image id");
        let internal_id = self
            .kitty_image_ids
            .get(&image_key)
            .copied()
            .ok_or_else(kitty_image_not_found)?;
        let image = self
            .inline_images
            .get(&internal_id)
            .cloned()
            .ok_or_else(kitty_image_not_found)?;
        let placement_key = command.placement_id.map(|placement_id| KittyPlacementKey {
            client_image_id: image_key.client_id,
            placement_id,
        });
        let placement_id = placement_key
            .and_then(|key| self.kitty_placement_ids.get(&key).copied())
            .unwrap_or_else(|| self.allocate_placement_id());
        let placement = placement_for_kitty(
            internal_id,
            placement_id,
            &image,
            command.columns,
            command.rows,
            self.terminal.image_anchor(),
            self.cell_metrics,
        )
        .map_err(kitty_image_error)?;

        self.terminal.upsert_image_placement(placement);
        graphics_diagnostics::record_placement_created();
        if let Some(placement_key) = placement_key {
            self.kitty_placement_ids.insert(placement_key, placement_id);
        }
        if command.cursor_movement {
            self.terminal
                .move_after_kitty_image(placement.columns, placement.rows);
        }
        Ok(())
    }

    fn handle_kitty_delete(&mut self, command: &KittyCommand<'_>) {
        let image_key = command.image_id.expect("parser requires an image id");
        let Some(internal_id) = self.kitty_image_ids.get(&image_key).copied() else {
            return;
        };

        if let Some(placement_id) = command.placement_id {
            let key = KittyPlacementKey {
                client_image_id: image_key.client_id,
                placement_id,
            };
            if let Some(internal_placement_id) = self.kitty_placement_ids.remove(&key) {
                self.terminal.remove_image_placement(internal_placement_id);
            }
        } else {
            self.terminal.remove_image_placements(internal_id);
            self.kitty_placement_ids
                .retain(|placement, _| placement.client_image_id != image_key.client_id);
        }
    }

    fn write_kitty_response(
        &mut self,
        context: KittyResponseContext,
        result: Result<(), KittyProtocolError>,
    ) {
        if let Some(response) = encode_response(context, result) {
            self.write_pty(&response);
        }
    }

    fn allocate_image_id(&mut self) -> ImageId {
        let id = ImageId::new(self.next_image_id);
        self.next_image_id = self.next_image_id.saturating_add(1).max(1);
        id
    }

    fn allocate_placement_id(&mut self) -> ImagePlacementId {
        let id = ImagePlacementId::new(self.next_placement_id);
        self.next_placement_id = self.next_placement_id.saturating_add(1).max(1);
        id
    }

    fn evict_unreferenced_inline_images(&mut self) {
        let mut live = self.terminal.image_placement_ids().collect::<BTreeSet<_>>();
        live.extend(self.kitty_image_ids.values().copied());
        self.inline_images.retain(|id, _| live.contains(id));
        let live_placements = self.terminal.placement_ids().collect::<BTreeSet<_>>();
        self.kitty_placement_ids
            .retain(|_, placement_id| live_placements.contains(placement_id));
        self.inline_image_bytes = self
            .inline_images
            .values()
            .map(|image| image.rgba.len())
            .sum();
    }

    pub fn set_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }

    pub fn handle_text_input(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.search.active {
            self.search.query.push_str(text);
            self.search.current_index = 0;
            self.search_visual_dirty = true;
            return;
        }

        self.write_user_input_to_pty(text.as_bytes());
    }

    pub fn handle_ime_preedit(&mut self, text: String, cursor_range: Option<Range<usize>>) {
        let next = if text.is_empty() {
            None
        } else {
            Some(ImePreedit { text, cursor_range })
        };
        if self.ime_preedit != next {
            self.ime_preedit = next;
            self.ime_visual_dirty = true;
        }
    }

    pub fn handle_ime_commit(&mut self, text: &str) {
        self.clear_ime_preedit();
        self.handle_text_input(text);
    }

    pub fn clear_ime_preedit(&mut self) {
        if self.ime_preedit.take().is_some() {
            self.ime_visual_dirty = true;
        }
    }

    pub fn handle_key_pressed(&mut self, key: &Key, text: Option<&str>) {
        if is_config_reload_shortcut(key, self.modifiers) {
            self.config_reload_requested = true;
            return;
        }
        if is_find_shortcut(key, self.modifiers) {
            self.clear_ime_preedit();
            self.search.active = true;
            self.search.current_index = 0;
            self.search_visual_dirty = true;
            return;
        }
        if matches!(key, Key::Named(NamedKey::Escape)) {
            self.clear_ime_preedit();
        }
        if self.search.active && self.handle_search_key(key) {
            return;
        }
        if let Some(action) = scroll_shortcut_for_key(key, self.modifiers) {
            self.handle_scroll_shortcut(action);
            return;
        }
        if is_copy_shortcut(key, self.modifiers) {
            self.copy_selection_to_clipboard();
            return;
        }
        if is_paste_shortcut(key, self.modifiers) {
            self.paste_from_clipboard();
            return;
        }
        if let Some(text) = keyboard_text_input(key, text, self.modifiers) {
            self.handle_text_input(text);
            return;
        }
        if let Some(bytes) = input_bytes(key, text, self.modifiers) {
            self.write_user_input_to_pty(&bytes);
        }
    }

    fn handle_search_key(&mut self, key: &Key) -> bool {
        match key {
            Key::Named(NamedKey::Escape) => {
                self.search = SearchState::default();
                self.search_visual_dirty = true;
                true
            }
            Key::Named(NamedKey::Enter) => {
                if self.modifiers.shift_key() {
                    self.search.current_index = self.search.current_index.saturating_sub(1);
                } else {
                    self.search.current_index = self.search.current_index.saturating_add(1);
                }
                self.search_visual_dirty = true;
                true
            }
            Key::Named(NamedKey::Backspace) => {
                self.search.query.pop();
                self.search.current_index = 0;
                self.search_visual_dirty = true;
                true
            }
            _ => false,
        }
    }

    pub fn handle_cursor_moved(&mut self, cell: Option<(usize, usize)>) {
        self.cursor_cell = cell;
        if self.selection_drag_active {
            self.clear_hover_hyperlink();
            if let Some(point) = self.cursor_selection_point() {
                if self.selection_drag_anchor != Some(point) {
                    self.selection_drag_moved = true;
                }
                self.terminal.update_selection(point);
            }
            return;
        }

        let current_hyperlink = self.cursor_hyperlink();
        if let Some(pending) = &self.pending_hyperlink_click
            && current_hyperlink.as_ref() != Some(&pending.hovered)
        {
            self.pending_hyperlink_click = None;
        }
        self.set_hovered_hyperlink(current_hyperlink);

        if self.pending_hyperlink_click.is_some() {
            return;
        }

        let (kind, button) = if self.pressed_mouse_button.is_some() {
            (MouseEventKind::Drag, self.pressed_mouse_button)
        } else {
            (MouseEventKind::Move, None)
        };
        self.write_mouse_event(kind, button);
    }

    pub fn handle_cursor_left(&mut self) {
        self.cursor_cell = None;
        self.pending_hyperlink_click = None;
        self.clear_hover_hyperlink();
    }

    pub fn handle_mouse_input(&mut self, state: ElementState, button: WinitMouseButton) {
        let terminal_button = terminal_mouse_button(button);
        match state {
            ElementState::Pressed => {
                self.pending_hyperlink_click = None;
                if terminal_button == Some(TerminalMouseButton::Left) {
                    let hovered = self.cursor_hyperlink();
                    if route_hyperlink_click(HyperlinkClickInput {
                        left_button: true,
                        ctrl_pressed: self.modifiers.control_key(),
                        open_enabled: self.config.hyperlink_open_enabled(),
                        has_hyperlink: hovered.is_some(),
                        selection_drag_active: self.selection_drag_active,
                        selection_active: self.terminal.has_selection(),
                        mouse_reporting_enabled: self.terminal.mouse_modes().protocol
                            != MouseProtocol::Off,
                    }) != HyperlinkClickRouting::ExistingMouseRouting
                    {
                        self.pressed_mouse_button = None;
                        self.selection_drag_active = false;
                        self.selection_drag_anchor = None;
                        self.selection_drag_mode = None;
                        self.selection_drag_moved = false;
                        self.pending_hyperlink_click =
                            hovered.map(|hovered| PendingHyperlinkClick { hovered });
                        return;
                    }
                    self.clear_selection_if_left_click_outside_selection();
                }

                self.pressed_mouse_button = terminal_button;
                if terminal_button == Some(TerminalMouseButton::Left)
                    && route_mouse_drag(
                        self.terminal.mouse_modes().protocol != MouseProtocol::Off,
                        self.modifiers.shift_key(),
                    ) == MouseDragRouting::Selection
                {
                    let Some(point) = self.cursor_selection_point() else {
                        self.pressed_mouse_button = None;
                        return;
                    };
                    let mode = self.click_tracker.record_click(point, Instant::now());
                    self.selection_drag_active = true;
                    self.selection_drag_anchor = Some(point);
                    self.selection_drag_mode = Some(mode);
                    self.selection_drag_moved = false;
                    self.terminal.begin_selection(point, mode);
                    self.update_cursor_icon();
                    return;
                }

                self.selection_drag_active = false;
                self.write_mouse_event(MouseEventKind::Press, terminal_button);
            }
            ElementState::Released => {
                if terminal_button == Some(TerminalMouseButton::Left)
                    && let Some(pending) = self.pending_hyperlink_click.take()
                {
                    self.pressed_mouse_button = None;
                    let current = self.cursor_hyperlink();
                    if should_open_pending_hyperlink(
                        &pending,
                        current.as_ref(),
                        self.selection_drag_active,
                        self.terminal.has_selection(),
                    ) {
                        self.handle_hyperlink_open(&pending.hovered.hyperlink);
                    }
                    return;
                }

                if self.selection_drag_active && terminal_button == Some(TerminalMouseButton::Left)
                {
                    let release_point = self.cursor_selection_point();
                    if let Some(point) = release_point {
                        if self.selection_drag_anchor != Some(point) {
                            self.selection_drag_moved = true;
                        }
                        self.terminal.update_selection(point);
                    }
                    let clear_simple_click = should_clear_simple_click_selection(
                        self.selection_drag_anchor,
                        release_point.or(self.selection_drag_anchor),
                        self.selection_drag_mode.unwrap_or(SelectionMode::Simple),
                        self.selection_drag_moved,
                    );
                    self.terminal.end_selection();
                    if clear_simple_click {
                        self.terminal.clear_selection();
                    }
                    self.selection_drag_active = false;
                    self.selection_drag_anchor = None;
                    self.selection_drag_mode = None;
                    self.selection_drag_moved = false;
                    self.pressed_mouse_button = None;
                    self.refresh_hover_hyperlink();
                    return;
                }

                self.write_mouse_event(
                    MouseEventKind::Release,
                    terminal_button.or(self.pressed_mouse_button),
                );
                if terminal_button == self.pressed_mouse_button || terminal_button.is_none() {
                    self.pressed_mouse_button = None;
                }
            }
        }
    }

    pub fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        if self.selection_drag_active {
            return;
        }

        match route_mouse_wheel(
            self.terminal.alternate_screen_enabled(),
            self.terminal.mouse_modes().protocol != MouseProtocol::Off,
            delta,
            self.config.scroll_multiplier(),
        ) {
            WheelRouting::PtyMouse(button) => {
                self.write_mouse_event(MouseEventKind::Wheel, Some(button));
            }
            WheelRouting::Scrollback { direction, lines } => {
                let damage = match direction {
                    ScrollDirection::Up => self.terminal.scroll_up_lines(lines),
                    ScrollDirection::Down => self.terminal.scroll_down_lines(lines),
                };
                if !matches!(damage, Damage::None) {
                    self.refresh_hover_hyperlink();
                }
            }
            WheelRouting::Noop => {}
        }
    }

    pub fn handle_focus_event(&mut self, focused: bool) {
        if !focused {
            self.clear_ime_preedit();
        }
        let Some(bytes) = self.terminal.encode_focus_event(focused) else {
            return;
        };
        self.write_pty(&bytes);
    }

    pub fn refresh_hover_hyperlink(&mut self) {
        self.set_hovered_hyperlink(self.cursor_hyperlink());
    }

    pub fn hovered_hyperlink_id_for_snapshot(&self, snapshot: &GridSnapshot) -> Option<usize> {
        if !self.config.hyperlink_underline_on_hover() || self.terminal.has_selection() {
            return None;
        }

        let hovered = self.hovered_hyperlink.as_ref()?;
        if hovered.cell.0 >= snapshot.cols || hovered.cell.1 >= snapshot.rows {
            return None;
        }

        let hyperlink_id = snapshot.cell(hovered.cell.0, hovered.cell.1).hyperlink?;
        if snapshot.hyperlinks.get(hyperlink_id) == Some(&hovered.hyperlink) {
            Some(hyperlink_id)
        } else {
            None
        }
    }

    fn paste_from_clipboard(&mut self) {
        let text = match self.ports.get_text() {
            Ok(text) => text,
            Err(error) => {
                eprintln!("knightty clipboard: paste failed: {error}");
                return;
            }
        };
        let bytes = self.terminal.paste_bytes(text.as_bytes());
        self.write_user_input_to_pty(&bytes);
    }

    fn copy_selection_to_clipboard(&mut self) {
        let Some(text) = self.terminal.selected_text() else {
            return;
        };

        if let Err(error) = self.ports.set_text(text) {
            eprintln!("knightty clipboard: copy failed: {error}");
        }
    }

    fn write_user_input_to_pty(&mut self, bytes: &[u8]) {
        if self.terminal.has_selection() {
            self.terminal.clear_selection();
            self.update_cursor_icon();
        }

        self.scroll_terminal_to_bottom();
        self.write_pty(bytes);
    }

    fn scroll_terminal_to_bottom(&mut self) {
        if !matches!(self.terminal.scroll_to_bottom(), Damage::None) {
            self.refresh_hover_hyperlink();
        }
    }

    fn clear_selection_if_left_click_outside_selection(&mut self) {
        if !self.terminal.has_selection() {
            return;
        }

        let outside_selection = self
            .cursor_cell
            .is_none_or(|(col, row)| !self.terminal.selection_contains_visible_cell(col, row));
        if outside_selection {
            self.terminal.clear_selection();
            self.update_cursor_icon();
        }
    }

    fn cursor_hyperlink(&self) -> Option<HoveredHyperlink> {
        let cell = self.cursor_cell?;
        let hyperlink = self.terminal.hyperlink_at_cell(cell.0, cell.1)?;
        Some(HoveredHyperlink { cell, hyperlink })
    }

    fn clear_hover_hyperlink(&mut self) {
        self.set_hovered_hyperlink(None);
    }

    fn set_hovered_hyperlink(&mut self, next: Option<HoveredHyperlink>) {
        if self.hovered_hyperlink == next {
            self.update_cursor_icon();
            return;
        }

        self.hovered_hyperlink = next;
        if self.config.hyperlink_underline_on_hover() {
            self.hover_visual_dirty = true;
        }
        self.update_cursor_icon();
    }

    fn update_cursor_icon(&mut self) {
        let icon = hyperlink_cursor_icon(
            self.hovered_hyperlink.is_some(),
            self.terminal.has_selection(),
        );
        self.set_cursor_icon(icon);
    }

    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if self.current_cursor_icon == icon {
            return;
        }
        self.current_cursor_icon = icon;
        self.ports.set_cursor_icon(icon);
    }

    fn handle_hyperlink_open(&mut self, hyperlink: &Hyperlink) {
        match allowed_hyperlink_url(&hyperlink.uri, self.config.hyperlink_allowed_schemes()) {
            Ok(url) => self.ports.open_url(url),
            Err(error) => {
                eprintln!("knightty hyperlink: rejected `{}`: {error}", hyperlink.uri);
            }
        }
    }

    fn cursor_selection_point(&self) -> Option<SelectionPoint> {
        let (col, row) = self.cursor_cell?;
        self.terminal.selection_point_for_visible_cell(col, row)
    }

    fn terminal_mouse_event(
        &self,
        kind: MouseEventKind,
        button: Option<TerminalMouseButton>,
    ) -> Option<TerminalMouseEvent> {
        let (col, row) = self.cursor_cell?;
        Some(TerminalMouseEvent {
            kind,
            button,
            col,
            row,
            shift: self.modifiers.shift_key(),
            alt: self.modifiers.alt_key(),
            ctrl: self.modifiers.control_key(),
        })
    }

    fn write_mouse_event(&mut self, kind: MouseEventKind, button: Option<TerminalMouseButton>) {
        let Some(event) = self.terminal_mouse_event(kind, button) else {
            return;
        };
        let Some(bytes) = self.terminal.encode_mouse_event(event) else {
            return;
        };
        self.write_pty(&bytes);
    }

    fn write_pty(&mut self, bytes: &[u8]) {
        self.ports.write_pty(bytes);
    }

    fn handle_scroll_shortcut(&mut self, action: ScrollShortcut) {
        let rows = self.terminal.size().1;
        let damage = match action {
            ScrollShortcut::PageUp => self.terminal.scroll_up_lines(rows),
            ScrollShortcut::PageDown => self.terminal.scroll_down_lines(rows),
            ScrollShortcut::Top => self.terminal.scroll_to_top(),
            ScrollShortcut::Bottom => self.terminal.scroll_to_bottom(),
        };
        if !matches!(damage, Damage::None) {
            self.refresh_hover_hyperlink();
        }
    }
}

fn kitty_image_not_found() -> KittyProtocolError {
    KittyProtocolError::new(KittyErrorCode::NoEntry, "image id was not found")
}

fn validate_kitty_chunk(payload: &[u8], more_chunks: bool) -> Result<(), KittyProtocolError> {
    if payload.len() > MAX_KITTY_CHUNK_BYTES {
        return Err(KittyProtocolError::new(
            KittyErrorCode::TooBig,
            "multipart chunk exceeds 4096 bytes",
        ));
    }
    if more_chunks && !payload.len().is_multiple_of(4) {
        return Err(KittyProtocolError::new(
            KittyErrorCode::Invalid,
            "non-final multipart chunk size must be a multiple of four",
        ));
    }
    Ok(())
}

fn kitty_image_error(error: InlineImageError) -> KittyProtocolError {
    match error {
        InlineImageError::InvalidBase64
        | InlineImageError::InvalidPng
        | InlineImageError::ZeroDimension => {
            KittyProtocolError::new(KittyErrorCode::BadPng, "invalid PNG image data")
        }
        InlineImageError::EncodedTooLarge
        | InlineImageError::CompressedTooLarge
        | InlineImageError::DimensionLimit
        | InlineImageError::PixelLimit
        | InlineImageError::DecodedTooLarge
        | InlineImageError::SizeOverflow => {
            KittyProtocolError::new(KittyErrorCode::TooBig, "image exceeds configured limits")
        }
        InlineImageError::ZeroCellSize => {
            KittyProtocolError::new(KittyErrorCode::Invalid, "terminal cell size is zero")
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoveredHyperlink {
    pub cell: (usize, usize),
    pub hyperlink: Hyperlink,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingHyperlinkClick {
    pub hovered: HoveredHyperlink,
}

#[derive(Clone, Copy, Debug)]
pub struct HyperlinkClickInput {
    pub left_button: bool,
    pub ctrl_pressed: bool,
    pub open_enabled: bool,
    pub has_hyperlink: bool,
    pub selection_drag_active: bool,
    pub selection_active: bool,
    pub mouse_reporting_enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HyperlinkClickRouting {
    PendingOpen { mouse_reporting_overridden: bool },
    ExistingMouseRouting,
}

pub fn route_hyperlink_click(input: HyperlinkClickInput) -> HyperlinkClickRouting {
    let HyperlinkClickInput {
        left_button,
        ctrl_pressed,
        open_enabled,
        has_hyperlink,
        selection_drag_active,
        selection_active,
        mouse_reporting_enabled,
    } = input;

    if left_button
        && ctrl_pressed
        && open_enabled
        && has_hyperlink
        && !selection_drag_active
        && !selection_active
    {
        HyperlinkClickRouting::PendingOpen {
            mouse_reporting_overridden: mouse_reporting_enabled,
        }
    } else {
        HyperlinkClickRouting::ExistingMouseRouting
    }
}

pub fn should_open_pending_hyperlink(
    pending: &PendingHyperlinkClick,
    current: Option<&HoveredHyperlink>,
    selection_drag_active: bool,
    selection_active: bool,
) -> bool {
    !selection_drag_active && !selection_active && current == Some(&pending.hovered)
}

pub fn should_clear_simple_click_selection(
    anchor: Option<SelectionPoint>,
    focus: Option<SelectionPoint>,
    mode: SelectionMode,
    moved: bool,
) -> bool {
    matches!(mode, SelectionMode::Simple) && !moved && anchor.is_some() && anchor == focus
}

pub fn hyperlink_cursor_icon(has_hover: bool, selection_active: bool) -> CursorIcon {
    if has_hover && !selection_active {
        CursorIcon::Pointer
    } else {
        CursorIcon::Default
    }
}

pub fn allowed_hyperlink_url(
    uri: &str,
    allowed_schemes: &[String],
) -> Result<String, HyperlinkUrlError> {
    if allowed_schemes.is_empty() {
        return Err(HyperlinkUrlError::NoAllowedSchemes);
    }

    let parsed = Url::parse(uri).map_err(|_| HyperlinkUrlError::InvalidUrl)?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if allowed_schemes.iter().any(|allowed| allowed == &scheme) {
        Ok(parsed.to_string())
    } else {
        Err(HyperlinkUrlError::DisallowedScheme(scheme))
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum HyperlinkUrlError {
    #[error("no URL schemes are allowed")]
    NoAllowedSchemes,
    #[error("invalid URL")]
    InvalidUrl,
    #[error("scheme `{0}` is not allowed")]
    DisallowedScheme(String),
}

fn keyboard_text_input<'a>(
    key: &Key,
    text: Option<&'a str>,
    modifiers: ModifiersState,
) -> Option<&'a str> {
    if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() {
        return None;
    }

    match key {
        Key::Character(_) => text.filter(|value| !value.is_empty()),
        Key::Named(NamedKey::Space) => text.filter(|value| !value.is_empty()).or(Some(" ")),
        _ => None,
    }
}

fn input_bytes(key: &Key, text: Option<&str>, modifiers: ModifiersState) -> Option<Vec<u8>> {
    if modifiers.control_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
        && let Some(bytes) = control_character_bytes(key)
    {
        return Some(bytes);
    }

    if modifiers.alt_key()
        && !modifiers.control_key()
        && !modifiers.super_key()
        && let Some(text) = keyboard_alt_text_input(key, text)
    {
        let mut bytes = Vec::with_capacity(1 + text.len());
        bytes.push(0x1b);
        bytes.extend_from_slice(text.as_bytes());
        return Some(bytes);
    }

    match key {
        Key::Named(NamedKey::Enter) => Some(b"\r".to_vec()),
        Key::Named(NamedKey::Backspace) => Some(b"\x7f".to_vec()),
        Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
        Key::Named(NamedKey::Escape) => Some(b"\x1b".to_vec()),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        Key::Named(NamedKey::Home) => Some(b"\x1b[H".to_vec()),
        Key::Named(NamedKey::End) => Some(b"\x1b[F".to_vec()),
        Key::Named(NamedKey::PageUp) => Some(b"\x1b[5~".to_vec()),
        Key::Named(NamedKey::PageDown) => Some(b"\x1b[6~".to_vec()),
        Key::Named(NamedKey::Insert) => Some(b"\x1b[2~".to_vec()),
        Key::Named(NamedKey::Delete) => Some(b"\x1b[3~".to_vec()),
        Key::Named(NamedKey::F1) => Some(b"\x1bOP".to_vec()),
        Key::Named(NamedKey::F2) => Some(b"\x1bOQ".to_vec()),
        Key::Named(NamedKey::F3) => Some(b"\x1bOR".to_vec()),
        Key::Named(NamedKey::F4) => Some(b"\x1bOS".to_vec()),
        Key::Named(NamedKey::F5) => Some(b"\x1b[15~".to_vec()),
        Key::Named(NamedKey::F6) => Some(b"\x1b[17~".to_vec()),
        Key::Named(NamedKey::F7) => Some(b"\x1b[18~".to_vec()),
        Key::Named(NamedKey::F8) => Some(b"\x1b[19~".to_vec()),
        Key::Named(NamedKey::F9) => Some(b"\x1b[20~".to_vec()),
        Key::Named(NamedKey::F10) => Some(b"\x1b[21~".to_vec()),
        Key::Named(NamedKey::F11) => Some(b"\x1b[23~".to_vec()),
        Key::Named(NamedKey::F12) => Some(b"\x1b[24~".to_vec()),
        _ => None,
    }
}

fn keyboard_alt_text_input<'a>(key: &Key, text: Option<&'a str>) -> Option<&'a str> {
    match key {
        Key::Character(_) => text.filter(|value| !value.is_empty()),
        Key::Named(NamedKey::Space) => text.filter(|value| !value.is_empty()).or(Some(" ")),
        _ => None,
    }
}

fn control_character_bytes(key: &Key) -> Option<Vec<u8>> {
    if matches!(key, Key::Named(NamedKey::Space)) {
        return Some(vec![0x00]);
    }

    let Key::Character(value) = key else {
        return None;
    };
    let mut chars = value.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    match ch {
        'a'..='z' => Some(vec![ch as u8 - b'a' + 1]),
        'A'..='Z' => Some(vec![ch as u8 - b'A' + 1]),
        '@' | ' ' => Some(vec![0x00]),
        '[' => Some(vec![0x1b]),
        '\\' => Some(vec![0x1c]),
        ']' => Some(vec![0x1d]),
        '^' => Some(vec![0x1e]),
        '_' => Some(vec![0x1f]),
        _ => None,
    }
}

#[derive(Clone, Debug, Default)]
struct SearchState {
    active: bool,
    query: String,
    current_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SearchCell {
    col: usize,
    ch: char,
    width: usize,
}

fn find_search_matches(snapshot: &GridSnapshot, query: &str) -> Vec<SearchMatch> {
    let needle = query
        .chars()
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if needle.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (row_index, row) in snapshot.lines().enumerate() {
        let haystack = row
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.flags.wide_spacer)
            .map(|(col, cell)| SearchCell {
                col,
                ch: cell.ch.to_ascii_lowercase(),
                width: if cell.flags.wide { 2 } else { 1 },
            })
            .collect::<Vec<_>>();
        if haystack.len() < needle.len() {
            continue;
        }

        for start in 0..=haystack.len() - needle.len() {
            let is_match = needle
                .iter()
                .enumerate()
                .all(|(offset, ch)| haystack[start + offset].ch == *ch);
            if !is_match {
                continue;
            }

            let end = haystack[start + needle.len() - 1];
            matches.push(SearchMatch {
                row: row_index,
                start_col: haystack[start].col,
                end_col: end.col + end.width,
            });
        }
    }

    matches
}

pub fn is_find_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
    modifiers.control_key()
        && modifiers.shift_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("f"))
}

pub fn is_config_reload_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
    modifiers.control_key()
        && modifiers.shift_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
        && matches!(key, Key::Character(value) if value.as_str() == ",")
}

pub fn is_paste_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
    let plain_terminal_modifier =
        !modifiers.alt_key() && !modifiers.super_key() && modifiers.shift_key();
    let ctrl_shift_v = plain_terminal_modifier
        && modifiers.control_key()
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("v"));
    let shift_insert = plain_terminal_modifier
        && !modifiers.control_key()
        && matches!(key, Key::Named(NamedKey::Insert));

    ctrl_shift_v || shift_insert
}

pub fn is_copy_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
    modifiers.control_key()
        && modifiers.shift_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("c"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseDragRouting {
    Selection,
    Pty,
}

pub fn route_mouse_drag(mouse_reporting_enabled: bool, shift_pressed: bool) -> MouseDragRouting {
    if shift_pressed || !mouse_reporting_enabled {
        MouseDragRouting::Selection
    } else {
        MouseDragRouting::Pty
    }
}

const MULTI_CLICK_MAX_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub struct ClickTracker {
    last_point: Option<SelectionPoint>,
    last_at: Option<Instant>,
    count: u8,
}

impl ClickTracker {
    pub fn record_click(&mut self, point: SelectionPoint, now: Instant) -> SelectionMode {
        let within_multi_click = self.last_point == Some(point)
            && self
                .last_at
                .and_then(|last| now.checked_duration_since(last))
                .is_some_and(|elapsed| elapsed <= MULTI_CLICK_MAX_DELAY);

        self.count = if within_multi_click && self.count < 3 {
            self.count + 1
        } else {
            1
        };
        self.last_point = Some(point);
        self.last_at = Some(now);

        match self.count {
            2 => SelectionMode::Word,
            3 => SelectionMode::Line,
            _ => SelectionMode::Simple,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollShortcut {
    PageUp,
    PageDown,
    Top,
    Bottom,
}

pub fn scroll_shortcut_for_key(key: &Key, modifiers: ModifiersState) -> Option<ScrollShortcut> {
    if !modifiers.control_key()
        || !modifiers.shift_key()
        || modifiers.alt_key()
        || modifiers.super_key()
    {
        return None;
    }

    match key {
        Key::Named(NamedKey::PageUp) => Some(ScrollShortcut::PageUp),
        Key::Named(NamedKey::PageDown) => Some(ScrollShortcut::PageDown),
        Key::Named(NamedKey::Home) => Some(ScrollShortcut::Top),
        Key::Named(NamedKey::End) => Some(ScrollShortcut::Bottom),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WheelRouting {
    PtyMouse(TerminalMouseButton),
    Scrollback {
        direction: ScrollDirection,
        lines: usize,
    },
    Noop,
}

pub fn route_mouse_wheel(
    alternate_screen_enabled: bool,
    mouse_reporting_enabled: bool,
    delta: MouseScrollDelta,
    scroll_multiplier: usize,
) -> WheelRouting {
    let Some(direction) = wheel_direction(delta) else {
        return WheelRouting::Noop;
    };

    if mouse_reporting_enabled {
        return WheelRouting::PtyMouse(direction.wheel_button());
    }

    if alternate_screen_enabled {
        return WheelRouting::Noop;
    }

    WheelRouting::Scrollback {
        direction,
        lines: wheel_scroll_lines(delta, scroll_multiplier),
    }
}

pub fn terminal_mouse_button(button: WinitMouseButton) -> Option<TerminalMouseButton> {
    match button {
        WinitMouseButton::Left => Some(TerminalMouseButton::Left),
        WinitMouseButton::Middle => Some(TerminalMouseButton::Middle),
        WinitMouseButton::Right => Some(TerminalMouseButton::Right),
        WinitMouseButton::Back => Some(TerminalMouseButton::Other(3)),
        WinitMouseButton::Forward => Some(TerminalMouseButton::Other(4)),
        WinitMouseButton::Other(code) => u8::try_from(code).ok().map(TerminalMouseButton::Other),
    }
}

fn wheel_direction(delta: MouseScrollDelta) -> Option<ScrollDirection> {
    let y = match delta {
        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
        MouseScrollDelta::PixelDelta(position) => position.y,
    };

    if y > 0.0 {
        Some(ScrollDirection::Up)
    } else if y < 0.0 {
        Some(ScrollDirection::Down)
    } else {
        None
    }
}

impl ScrollDirection {
    fn wheel_button(self) -> TerminalMouseButton {
        match self {
            Self::Up => TerminalMouseButton::WheelUp,
            Self::Down => TerminalMouseButton::WheelDown,
        }
    }
}

fn wheel_scroll_lines(delta: MouseScrollDelta, scroll_multiplier: usize) -> usize {
    let delta_lines = match delta {
        MouseScrollDelta::LineDelta(_, y) => y.abs().ceil().max(1.0) as usize,
        MouseScrollDelta::PixelDelta(_) => 1,
    };
    delta_lines.saturating_mul(scroll_multiplier.max(1))
}

#[cfg(test)]
mod harness_tests {
    use super::{
        ClipboardPort, CursorPort, InputRouter, KittyErrorCode, KittyImageKey, PtyWritePort,
        UrlOpenPort, allowed_hyperlink_url,
    };
    use crate::config::AppConfig;
    use knightty_render::{ImePreedit, SearchMatch};
    use std::ops::Range;
    use winit::event::{ElementState, MouseButton};
    use winit::keyboard::{Key, ModifiersState, NamedKey};
    use winit::window::CursorIcon;

    struct FakePorts {
        clipboard: String,
        pty_writes: Vec<Vec<u8>>,
        opened_urls: Vec<String>,
        cursor_icon: CursorIcon,
    }

    impl Default for FakePorts {
        fn default() -> Self {
            Self {
                clipboard: String::new(),
                pty_writes: Vec::new(),
                opened_urls: Vec::new(),
                cursor_icon: CursorIcon::Default,
            }
        }
    }

    impl ClipboardPort for FakePorts {
        fn get_text(&mut self) -> Result<String, String> {
            Ok(self.clipboard.clone())
        }

        fn set_text(&mut self, text: String) -> Result<(), String> {
            self.clipboard = text;
            Ok(())
        }
    }

    impl PtyWritePort for FakePorts {
        fn write_pty(&mut self, bytes: &[u8]) {
            self.pty_writes.push(bytes.to_vec());
        }
    }

    impl UrlOpenPort for FakePorts {
        fn open_url(&mut self, url: String) {
            self.opened_urls.push(url);
        }
    }

    impl CursorPort for FakePorts {
        fn set_cursor_icon(&mut self, icon: CursorIcon) {
            self.cursor_icon = icon;
        }
    }

    struct AppHarness {
        router: InputRouter<FakePorts>,
    }

    impl AppHarness {
        fn new() -> Self {
            let mut config = AppConfig::default();
            config.window.initial_cols = Some(40);
            config.window.initial_rows = Some(5);
            Self {
                router: InputRouter::new(config, FakePorts::default()),
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            self.router.feed_pty_bytes(bytes);
        }

        fn feed_text(&mut self, text: &str) {
            self.feed(text.as_bytes());
        }

        fn feed_link(&mut self, uri: &str, text: &str) {
            let sequence = format!("\x1b]8;;{uri}\x07{text}\x1b]8;;\x07");
            self.feed(sequence.as_bytes());
        }

        fn move_to(&mut self, col: usize, row: usize) {
            self.router.handle_cursor_moved(Some((col, row)));
        }

        fn set_ctrl(&mut self, enabled: bool) {
            let modifiers = if enabled {
                ModifiersState::CONTROL
            } else {
                ModifiersState::empty()
            };
            self.router.set_modifiers(modifiers);
        }

        fn left_press(&mut self) {
            self.router
                .handle_mouse_input(ElementState::Pressed, MouseButton::Left);
        }

        fn left_release(&mut self) {
            self.router
                .handle_mouse_input(ElementState::Released, MouseButton::Left);
        }

        fn left_drag(&mut self, start: (usize, usize), end: (usize, usize)) {
            self.move_to(start.0, start.1);
            self.left_press();
            self.move_to(end.0, end.1);
            self.left_release();
        }

        fn ctrl_click(&mut self, col: usize, row: usize) {
            self.set_ctrl(true);
            self.move_to(col, row);
            self.left_press();
            self.left_release();
            self.set_ctrl(false);
        }

        fn send_text_input(&mut self, text: &str) {
            self.router
                .handle_key_pressed(&Key::Character(text.into()), Some(text));
        }

        fn send_key(&mut self, key: Key, text: Option<&str>) {
            self.router.handle_key_pressed(&key, text);
        }

        fn set_modifiers(&mut self, modifiers: ModifiersState) {
            self.router.set_modifiers(modifiers);
        }

        fn start_search(&mut self) {
            self.router
                .set_modifiers(ModifiersState::CONTROL | ModifiersState::SHIFT);
            self.router
                .handle_key_pressed(&Key::Character("f".into()), None);
            self.router.set_modifiers(ModifiersState::empty());
        }

        fn ime_commit(&mut self, text: &str) {
            self.router.handle_ime_commit(text);
        }

        fn ime_preedit(&mut self, text: &str, cursor_range: Option<Range<usize>>) {
            self.router
                .handle_ime_preedit(text.to_owned(), cursor_range);
        }

        fn rendered_preedit(&self) -> Option<ImePreedit> {
            self.router.ime_preedit_for_render()
        }

        fn focus(&mut self, focused: bool) {
            self.router.handle_focus_event(focused);
        }

        fn copy_shortcut(&mut self) {
            self.router
                .set_modifiers(ModifiersState::CONTROL | ModifiersState::SHIFT);
            self.router
                .handle_key_pressed(&Key::Character("c".into()), None);
            self.router.set_modifiers(ModifiersState::empty());
        }

        fn paste_shortcut(&mut self) {
            self.router
                .set_modifiers(ModifiersState::CONTROL | ModifiersState::SHIFT);
            self.router
                .handle_key_pressed(&Key::Character("v".into()), None);
            self.router.set_modifiers(ModifiersState::empty());
        }

        fn shift_insert_paste(&mut self) {
            self.router.set_modifiers(ModifiersState::SHIFT);
            self.router
                .handle_key_pressed(&Key::Named(NamedKey::Insert), None);
            self.router.set_modifiers(ModifiersState::empty());
        }

        fn clipboard(&self) -> &str {
            &self.router.ports().clipboard
        }

        fn set_clipboard(&mut self, text: &str) {
            self.router.ports_mut().clipboard = text.to_owned();
        }

        fn opened_urls(&self) -> &[String] {
            &self.router.ports().opened_urls
        }

        fn pty_writes(&self) -> &[Vec<u8>] {
            &self.router.ports().pty_writes
        }

        fn clear_pty_writes(&mut self) {
            self.router.ports_mut().pty_writes.clear();
        }

        fn cursor_icon(&self) -> CursorIcon {
            self.router.ports().cursor_icon
        }

        fn has_selection(&self) -> bool {
            self.router.terminal.has_selection()
        }

        fn selected_text(&self) -> Option<String> {
            self.router.terminal.selected_text()
        }
    }

    const TRANSPARENT_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    fn kitty_sequence(control: &str, payload: Option<&str>) -> Vec<u8> {
        let payload = payload.map_or_else(String::new, |payload| format!(";{payload}"));
        format!("\x1b_G{control}{payload}\x1b\\").into_bytes()
    }

    fn kitty_continuation(more_chunks: bool, quiet: Option<u8>, payload: &str) -> Vec<u8> {
        let more_chunks = u8::from(more_chunks);
        let quiet = quiet.map_or_else(String::new, |quiet| format!(",q={quiet}"));
        kitty_sequence(&format!("m={more_chunks}{quiet}"), Some(payload))
    }

    #[test]
    fn inline_png_is_decoded_placed_and_kept_out_of_terminal_text() {
        let mut app = AppHarness::new();
        let sequence = format!("A\x1b]1337;File=inline=1:{TRANSPARENT_PNG}\x07B");

        app.feed(sequence.as_bytes());

        let snapshot = app.router.snapshot();
        assert_eq!(snapshot.cell(0, 0).ch, 'A');
        assert_eq!(snapshot.cell(0, 1).ch, 'B');
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_eq!(snapshot.image_placements[0].anchor.col, 1);
        assert_eq!(app.router.inline_images_for_snapshot(&snapshot).len(), 1);
    }

    #[test]
    fn disabled_graphics_swallow_image_payload_without_decoding_or_cursor_advance() {
        let mut config = AppConfig::default();
        config.graphics.enabled = false;
        let mut router = InputRouter::new(config, FakePorts::default());
        let sequence = format!("A\x1b]1337;File=inline=1:{TRANSPARENT_PNG}\x07B");

        router.feed_pty_bytes(sequence.as_bytes());

        let snapshot = router.snapshot();
        assert_eq!(snapshot.cell(0, 0).ch, 'A');
        assert_eq!(snapshot.cell(1, 0).ch, 'B');
        assert!(snapshot.image_placements.is_empty());
        assert!(router.inline_images_for_snapshot(&snapshot).is_empty());
    }

    #[test]
    fn inline_image_split_across_feeds_waits_for_st_terminator() {
        let mut app = AppHarness::new();
        let first = format!("\x1b]1337;File=inline=1:{TRANSPARENT_PNG}");

        app.feed(first.as_bytes());
        assert!(app.router.snapshot().image_placements.is_empty());

        app.feed(b"\x1b\\");

        assert_eq!(app.router.snapshot().image_placements.len(), 1);
    }

    #[test]
    fn reset_between_images_releases_the_previous_store_entry_immediately() {
        let mut config = AppConfig::default();
        config.graphics.max_images = 1;
        let mut router = InputRouter::new(config, FakePorts::default());
        let sequence = format!(
            "\x1b]1337;File=inline=1:{TRANSPARENT_PNG}\x07\x1bc\
             \x1b]1337;File=inline=1:{TRANSPARENT_PNG}\x07"
        );

        router.feed_pty_bytes(sequence.as_bytes());

        let snapshot = router.snapshot();
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_eq!(router.inline_images_for_snapshot(&snapshot).len(), 1);
    }

    #[test]
    fn kitty_transmit_and_display_decodes_png_without_exposing_apc_as_text() {
        let mut app = AppHarness::new();
        app.feed(b"A");
        app.feed(&kitty_sequence(
            "a=T,f=100,t=d,i=42,p=7,c=2,r=1,C=1",
            Some(TRANSPARENT_PNG),
        ));
        app.feed(b"B");

        let snapshot = app.router.snapshot();
        assert_eq!(snapshot.cell(0, 0).ch, 'A');
        assert_eq!(snapshot.cell(1, 0).ch, 'B');
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_eq!(snapshot.image_placements[0].anchor.col, 1);
        assert_eq!(snapshot.image_placements[0].columns, 2);
        assert_eq!(app.pty_writes(), &[b"\x1b_Gi=42,p=7;OK\x1b\\".to_vec()]);
    }

    #[test]
    fn kitty_q0_m0_known_1x1_png_smoke_creates_a_placement() {
        let mut app = AppHarness::new();

        app.feed(&kitty_sequence(
            "a=T,f=100,t=d,i=4242,p=7,q=0,m=0,c=1,r=1,C=1",
            Some(TRANSPARENT_PNG),
        ));

        let snapshot = app.router.snapshot();
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_eq!(snapshot.image_placements[0].columns, 1);
        assert_eq!(snapshot.image_placements[0].rows, 1);
        assert_eq!(app.router.inline_images_for_snapshot(&snapshot).len(), 1);
        assert_eq!(app.pty_writes(), &[b"\x1b_Gi=4242,p=7;OK\x1b\\".to_vec()]);
    }

    #[test]
    fn kitty_transfer_only_image_can_be_placed_later() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence("a=t,f=100,i=42", Some(TRANSPARENT_PNG)));
        assert!(app.router.snapshot().image_placements.is_empty());

        app.feed(&kitty_sequence("a=p,i=42,p=7,c=2,r=1,C=1", None));

        let snapshot = app.router.snapshot();
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_eq!(app.router.inline_images_for_snapshot(&snapshot).len(), 1);
        assert_eq!(
            app.pty_writes(),
            &[
                b"\x1b_Gi=42;OK\x1b\\".to_vec(),
                b"\x1b_Gi=42,p=7;OK\x1b\\".to_vec(),
            ]
        );
    }

    #[test]
    fn kitty_multipart_waits_for_final_chunk_and_then_displays_atomically() {
        let mut app = AppHarness::new();
        let (first, final_chunk) = TRANSPARENT_PNG.split_at(4);

        app.feed(&kitty_sequence(
            "a=T,f=100,t=d,i=42,p=7,c=2,r=1,C=1,m=1",
            Some(first),
        ));

        assert!(app.router.snapshot().image_placements.is_empty());
        assert!(app.router.inline_images.is_empty());
        assert!(app.pty_writes().is_empty());

        app.feed(&kitty_continuation(false, None, final_chunk));

        let snapshot = app.router.snapshot();
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_eq!(snapshot.image_placements[0].columns, 2);
        assert_eq!(app.router.inline_images_for_snapshot(&snapshot).len(), 1);
        assert_eq!(app.pty_writes(), &[b"\x1b_Gi=42,p=7;OK\x1b\\".to_vec()]);
    }

    #[test]
    fn kitty_multipart_accepts_many_small_chunks_and_empty_intermediate_and_final_chunks() {
        let mut app = AppHarness::new();
        let chunks = TRANSPARENT_PNG.as_bytes().chunks(4).collect::<Vec<_>>();
        let first = std::str::from_utf8(chunks[0]).unwrap();
        app.feed(&kitty_sequence("a=T,f=100,t=d,i=42,C=1,m=1", Some(first)));
        app.feed(&kitty_continuation(true, None, ""));
        for chunk in &chunks[1..] {
            app.feed(&kitty_continuation(
                true,
                None,
                std::str::from_utf8(chunk).unwrap(),
            ));
        }

        assert!(app.router.snapshot().image_placements.is_empty());
        assert!(app.pty_writes().is_empty());

        app.feed(&kitty_continuation(false, None, ""));

        assert_eq!(app.router.snapshot().image_placements.len(), 1);
        assert_eq!(app.pty_writes(), &[b"\x1b_Gi=42;OK\x1b\\".to_vec()]);
    }

    #[test]
    fn kitty_multipart_continuation_quiet_value_overrides_initial_response_policy() {
        let mut app = AppHarness::new();
        let (first, final_chunk) = TRANSPARENT_PNG.split_at(4);
        app.feed(&kitty_sequence("a=t,f=100,t=d,i=42,q=0,m=1", Some(first)));

        app.feed(&kitty_continuation(false, Some(1), final_chunk));

        assert!(app.pty_writes().is_empty());
        assert!(
            app.router
                .kitty_image_ids
                .contains_key(&KittyImageKey { client_id: 42 })
        );
    }

    #[test]
    fn kitty_orphan_continuation_is_discarded_without_creating_image_state() {
        let mut app = AppHarness::new();

        app.feed(&kitty_continuation(false, None, "AAAA"));

        assert!(app.router.partial_kitty_upload.is_none());
        assert!(app.router.inline_images.is_empty());
        assert!(app.router.kitty_image_ids.is_empty());
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn kitty_multipart_chunk_and_total_encoded_limits_are_enforced() {
        assert!(super::validate_kitty_chunk(&vec![b'A'; 4096], true).is_ok());
        assert_eq!(
            super::validate_kitty_chunk(&vec![b'A'; 4097], false)
                .unwrap_err()
                .code,
            KittyErrorCode::TooBig
        );
        assert_eq!(
            super::validate_kitty_chunk(b"AAA", true).unwrap_err().code,
            KittyErrorCode::Invalid
        );
        assert!(super::validate_kitty_chunk(b"AAA", false).is_ok());

        let mut config = AppConfig::default();
        config.graphics.max_encoded_bytes = 8;
        let mut router = InputRouter::new(config, FakePorts::default());
        let mut exact_limit = b"AAAA".to_vec();
        assert!(
            router
                .append_kitty_chunk(&mut exact_limit, b"BBBB", false)
                .is_ok()
        );
        assert_eq!(exact_limit.len(), 8);
        router.feed_pty_bytes(&kitty_sequence(
            "a=t,f=100,t=d,i=42,m=1",
            Some(&TRANSPARENT_PNG[..8]),
        ));
        router.feed_pty_bytes(&kitty_continuation(false, None, &TRANSPARENT_PNG[8..12]));

        assert!(router.partial_kitty_upload.is_none());
        assert_eq!(
            router.ports().pty_writes,
            vec![b"\x1b_Gi=42;E2BIG:graphics payload exceeds configured limit\x1b\\".to_vec()]
        );
    }

    #[test]
    fn kitty_invalid_base64_in_final_chunk_aborts_upload() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence("a=t,f=100,t=d,i=42,m=1", Some("AAAA")));

        app.feed(&kitty_continuation(false, None, "!"));

        assert!(app.router.partial_kitty_upload.is_none());
        assert!(app.router.kitty_image_ids.is_empty());
        assert_eq!(
            app.pty_writes(),
            &[b"\x1b_Gi=42;EBADPNG:invalid PNG image data\x1b\\".to_vec()]
        );
    }

    #[test]
    fn kitty_new_multipart_upload_with_same_id_aborts_the_previous_partial() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence("a=t,f=100,t=d,i=42,q=1,m=1", Some("AAAA")));
        let (first, final_chunk) = TRANSPARENT_PNG.split_at(4);

        app.feed(&kitty_sequence("a=t,f=100,t=d,i=42,q=1,m=1", Some(first)));
        app.feed(&kitty_continuation(false, None, final_chunk));

        assert!(app.router.partial_kitty_upload.is_none());
        assert!(
            app.router
                .kitty_image_ids
                .contains_key(&KittyImageKey { client_id: 42 })
        );
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn kitty_failed_multipart_retransmit_keeps_previous_image_and_placement() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,p=7,C=1,q=1",
            Some(TRANSPARENT_PNG),
        ));
        let previous = app.router.snapshot().image_placements[0];

        app.feed(&kitty_sequence(
            "a=T,f=100,t=d,i=42,p=8,C=1,m=1",
            Some("bm90"),
        ));
        app.feed(&kitty_continuation(false, None, "IGEgcG5n"));

        assert_eq!(app.router.snapshot().image_placements, vec![previous]);
        assert_eq!(
            app.pty_writes(),
            &[b"\x1b_Gi=42,p=8;EBADPNG:invalid PNG image data\x1b\\".to_vec()]
        );
    }

    #[test]
    fn kitty_successful_multipart_retransmit_replaces_all_previous_placements() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,p=7,C=1,q=1",
            Some(TRANSPARENT_PNG),
        ));
        app.feed(&kitty_sequence("a=p,i=42,p=8,C=1,q=1", None));
        let previous_id = app.router.snapshot().image_placements[0].image_id;
        let (first, final_chunk) = TRANSPARENT_PNG.split_at(4);

        app.feed(&kitty_sequence(
            "a=T,f=100,t=d,i=42,p=9,C=1,q=1,m=1",
            Some(first),
        ));
        app.feed(&kitty_continuation(false, None, final_chunk));

        let snapshot = app.router.snapshot();
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_ne!(snapshot.image_placements[0].image_id, previous_id);
    }

    #[test]
    fn kitty_delete_new_command_reset_and_config_update_abort_partial_uploads() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence("a=t,f=100,t=d,i=42,q=1,m=1", Some("AAAA")));
        app.feed(&kitty_sequence("a=d,d=i,i=42", None));
        assert!(app.router.partial_kitty_upload.is_none());

        app.feed(&kitty_sequence("a=t,f=100,t=d,i=42,q=1,m=1", Some("AAAA")));
        app.feed(&kitty_sequence(
            "a=t,f=100,t=d,i=43,q=1",
            Some(TRANSPARENT_PNG),
        ));
        assert!(app.router.partial_kitty_upload.is_none());
        assert!(
            app.router
                .kitty_image_ids
                .contains_key(&KittyImageKey { client_id: 43 })
        );

        app.feed(&kitty_sequence("a=t,f=100,t=d,i=44,q=1,m=1", Some("AAAA")));
        app.feed(b"\x1bc");
        assert!(app.router.partial_kitty_upload.is_none());

        app.feed(&kitty_sequence("a=t,f=100,t=d,i=45,q=1,m=1", Some("AAAA")));
        app.router.update_config(AppConfig::default());
        assert!(app.router.partial_kitty_upload.is_none());
    }

    #[test]
    fn kitty_named_placement_is_replaced_while_anonymous_puts_accumulate() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence("a=t,f=100,i=42,q=1", Some(TRANSPARENT_PNG)));
        app.feed(&kitty_sequence("a=p,i=42,p=7,C=1,q=1", None));
        app.feed(b"\x1b[2;3H");
        app.feed(&kitty_sequence("a=p,i=42,p=7,C=1,q=1", None));

        let named = app.router.snapshot();
        assert_eq!(named.image_placements.len(), 1);
        assert_eq!(named.image_placements[0].anchor.col, 2);
        assert_eq!(named.image_placements[0].anchor.row, 1);

        app.feed(&kitty_sequence("a=p,i=42,C=1,q=1", None));
        app.feed(&kitty_sequence("a=p,i=42,p=0,C=1,q=1", None));
        assert_eq!(app.router.snapshot().image_placements.len(), 3);
    }

    #[test]
    fn kitty_retransmit_replaces_image_and_all_existing_placements() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,p=7,C=1,q=1",
            Some(TRANSPARENT_PNG),
        ));
        app.feed(&kitty_sequence("a=p,i=42,p=8,C=1,q=1", None));
        let previous_id = app.router.snapshot().image_placements[0].image_id;
        assert_eq!(app.router.snapshot().image_placements.len(), 2);

        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,p=9,C=1,q=1",
            Some(TRANSPARENT_PNG),
        ));

        let snapshot = app.router.snapshot();
        assert_eq!(snapshot.image_placements.len(), 1);
        assert_ne!(snapshot.image_placements[0].image_id, previous_id);
    }

    #[test]
    fn kitty_missing_image_and_quiet_modes_control_responses() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence("a=p,i=99,q=1", None));
        assert_eq!(
            app.pty_writes(),
            &[b"\x1b_Gi=99;ENOENT:image id was not found\x1b\\".to_vec()]
        );

        app.clear_pty_writes();
        app.feed(&kitty_sequence("a=p,i=99,q=2", None));
        assert!(app.pty_writes().is_empty());

        app.feed(&kitty_sequence("a=t,f=100,i=42,q=1", Some(TRANSPARENT_PNG)));
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn kitty_failed_retransmit_keeps_previous_image_and_placement() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,p=7,C=1,q=1",
            Some(TRANSPARENT_PNG),
        ));
        let previous = app.router.snapshot().image_placements[0];

        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,p=8,C=1",
            Some("bm90IGEgcG5n"),
        ));

        assert_eq!(app.router.snapshot().image_placements, vec![previous]);
        assert_eq!(
            app.pty_writes(),
            &[b"\x1b_Gi=42,p=8;EBADPNG:invalid PNG image data\x1b\\".to_vec()]
        );
    }

    #[test]
    fn kitty_image_count_limit_rejects_new_id_but_allows_same_id_replacement() {
        let mut config = AppConfig::default();
        config.graphics.max_images = 1;
        let mut router = InputRouter::new(config, FakePorts::default());
        router.feed_pty_bytes(&kitty_sequence("a=t,f=100,i=1,q=1", Some(TRANSPARENT_PNG)));
        router.feed_pty_bytes(&kitty_sequence("a=t,f=100,i=1,q=1", Some(TRANSPARENT_PNG)));
        router.feed_pty_bytes(&kitty_sequence("a=t,f=100,i=2", Some(TRANSPARENT_PNG)));

        assert_eq!(
            router.ports().pty_writes,
            vec![b"\x1b_Gi=2;ENOSPC:image count limit reached\x1b\\".to_vec()]
        );
    }

    #[test]
    fn oversized_kitty_payload_returns_e2big_and_following_text_recovers() {
        let mut config = AppConfig::default();
        config.graphics.max_encoded_bytes = 4;
        let mut router = InputRouter::new(config, FakePorts::default());
        let mut input = kitty_sequence("a=T,f=100,i=42", Some(TRANSPARENT_PNG));
        input.extend_from_slice(b"ok");

        router.feed_pty_bytes(&input);

        let snapshot = router.snapshot();
        assert_eq!(snapshot.cell(0, 0).ch, 'o');
        assert_eq!(snapshot.cell(1, 0).ch, 'k');
        assert_eq!(
            router.ports().pty_writes,
            vec![b"\x1b_Gi=42;E2BIG:graphics payload exceeds configured limit\x1b\\".to_vec()]
        );
    }

    #[test]
    fn kitty_soft_delete_preserves_image_for_later_placement() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,p=7,C=1,q=1",
            Some(TRANSPARENT_PNG),
        ));
        app.feed(&kitty_sequence("a=p,i=42,p=8,C=1,q=1", None));
        app.feed(&kitty_sequence("a=d,d=i,i=42,p=7", None));
        assert_eq!(app.router.snapshot().image_placements.len(), 1);

        app.feed(&kitty_sequence("a=d,d=i,i=42", None));
        assert!(app.router.snapshot().image_placements.is_empty());

        app.feed(&kitty_sequence("a=p,i=42,p=8,C=1,q=1", None));

        assert_eq!(app.router.snapshot().image_placements.len(), 1);
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn terminal_reset_releases_named_kitty_images() {
        let mut config = AppConfig::default();
        config.graphics.max_images = 1;
        let mut router = InputRouter::new(config, FakePorts::default());
        router.feed_pty_bytes(&kitty_sequence("a=t,f=100,i=1,q=1", Some(TRANSPARENT_PNG)));

        router.feed_pty_bytes(b"\x1bc");
        router.feed_pty_bytes(&kitty_sequence("a=t,f=100,i=2,q=1", Some(TRANSPARENT_PNG)));

        assert!(router.ports().pty_writes.is_empty());
    }

    #[test]
    fn kitty_cursor_movement_uses_placement_rectangle_and_c_one_disables_it() {
        let mut app = AppHarness::new();
        app.feed(&kitty_sequence(
            "a=T,f=100,i=42,c=3,r=2,q=1",
            Some(TRANSPARENT_PNG),
        ));
        assert_eq!(
            (
                app.router.snapshot().cursor.x,
                app.router.snapshot().cursor.y
            ),
            (3, 1)
        );

        app.feed(&kitty_sequence("a=p,i=42,c=4,r=3,C=1,q=1", None));
        assert_eq!(
            (
                app.router.snapshot().cursor.x,
                app.router.snapshot().cursor.y
            ),
            (3, 1)
        );
    }

    #[test]
    fn disabled_graphics_swallow_kitty_payload_without_response() {
        let mut config = AppConfig::default();
        config.graphics.enabled = false;
        let mut router = InputRouter::new(config, FakePorts::default());
        let sequence = kitty_sequence("a=T,f=100,i=42,c=2,r=1", Some(TRANSPARENT_PNG));

        router.feed_pty_bytes(&sequence);

        assert!(router.snapshot().image_placements.is_empty());
        assert!(router.ports().pty_writes.is_empty());
    }

    #[test]
    fn hovering_osc8_link_sets_pointer_cursor() {
        let mut app = AppHarness::new();
        app.feed_link("https://example.com", "link");

        app.move_to(0, 0);

        assert_eq!(app.cursor_icon(), CursorIcon::Pointer);
    }

    #[test]
    fn hovering_non_link_text_sets_default_cursor() {
        let mut app = AppHarness::new();
        app.feed_text("plain");

        app.move_to(0, 0);

        assert_eq!(app.cursor_icon(), CursorIcon::Default);
    }

    #[test]
    fn ctrl_click_press_and_release_on_same_link_opens_url() {
        let mut app = AppHarness::new();
        app.feed_link("https://example.com", "link");

        app.ctrl_click(1, 0);

        assert_eq!(app.opened_urls(), &["https://example.com/".to_owned()]);
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn ctrl_click_press_on_one_link_and_release_on_another_does_not_open() {
        let mut app = AppHarness::new();
        app.feed_link("https://one.example", "one");
        app.feed_text(" ");
        app.feed_link("https://two.example", "two");

        app.set_ctrl(true);
        app.move_to(0, 0);
        app.left_press();
        app.move_to(4, 0);
        app.left_release();
        app.set_ctrl(false);

        assert!(app.opened_urls().is_empty());
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn javascript_links_are_not_opened() {
        let mut app = AppHarness::new();
        app.feed_link("javascript:alert(1)", "link");

        app.ctrl_click(0, 0);

        assert!(app.opened_urls().is_empty());
    }

    #[test]
    fn invalid_urls_are_not_opened() {
        let mut app = AppHarness::new();
        app.feed_link("not a url", "link");

        app.ctrl_click(0, 0);

        assert!(app.opened_urls().is_empty());
    }

    #[test]
    fn disallowed_urls_consumed_by_hyperlink_routing_are_not_written_to_pty() {
        let mut app = AppHarness::new();
        app.feed(b"\x1b[?1000;1006h");
        app.feed_link("javascript:alert(1)", "link");
        app.clear_pty_writes();

        app.ctrl_click(0, 0);

        assert!(app.opened_urls().is_empty());
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn left_drag_creates_selection() {
        let mut app = AppHarness::new();
        app.feed_text("hello");

        app.left_drag((0, 0), (4, 0));

        assert!(app.has_selection());
        assert_eq!(app.selected_text(), Some("hello".to_owned()));
    }

    #[test]
    fn dragging_over_hyperlink_selects_text_instead_of_opening_url() {
        let mut app = AppHarness::new();
        app.feed_link("https://example.com", "hello");

        app.left_drag((0, 0), (4, 0));

        assert_eq!(app.selected_text(), Some("hello".to_owned()));
        assert!(app.opened_urls().is_empty());
    }

    #[test]
    fn existing_selection_suppresses_hyperlink_opening() {
        let mut app = AppHarness::new();
        app.feed_text("xx ");
        app.feed_link("https://example.com", "link");
        app.left_drag((0, 0), (1, 0));

        app.ctrl_click(3, 0);

        assert!(app.opened_urls().is_empty());
    }

    #[test]
    fn normal_keyboard_input_clears_selection() {
        let mut app = AppHarness::new();
        app.feed_text("hello");
        app.left_drag((0, 0), (4, 0));

        app.send_text_input("x");

        assert!(!app.has_selection());
        assert_eq!(app.pty_writes(), &[b"x".to_vec()]);
    }

    #[test]
    fn printable_ascii_text_is_written_to_pty() {
        let mut app = AppHarness::new();

        app.send_text_input("abc");

        assert_eq!(app.pty_writes(), &[b"abc".to_vec()]);
    }

    #[test]
    fn named_space_key_writes_space_without_text_payload() {
        let mut app = AppHarness::new();

        app.send_key(Key::Named(NamedKey::Space), None);

        assert_eq!(app.pty_writes(), &[b" ".to_vec()]);
    }

    #[test]
    fn search_mode_collects_query_without_writing_to_pty() {
        let mut app = AppHarness::new();
        app.feed_text("alpha beta Alpha");

        app.start_search();
        app.send_text_input("alpha");

        let snapshot = app.router.snapshot();
        let (matches, current_match) = app.router.search_matches_for_snapshot(&snapshot);
        assert_eq!(
            app.router.search_query_for_render(),
            Some("alpha".to_owned())
        );
        assert!(app.pty_writes().is_empty());
        assert_eq!(
            matches,
            vec![
                SearchMatch {
                    row: 0,
                    start_col: 0,
                    end_col: 5,
                },
                SearchMatch {
                    row: 0,
                    start_col: 11,
                    end_col: 16,
                },
            ]
        );
        assert_eq!(current_match, matches.first().cloned());
    }

    #[test]
    fn search_enter_advances_current_match_and_escape_clears_search() {
        let mut app = AppHarness::new();
        app.feed_text("one two one");

        app.start_search();
        app.send_text_input("one");
        app.send_key(Key::Named(NamedKey::Enter), None);

        let snapshot = app.router.snapshot();
        let (matches, current_match) = app.router.search_matches_for_snapshot(&snapshot);
        assert_eq!(current_match, matches.get(1).cloned());

        app.send_key(Key::Named(NamedKey::Escape), None);

        let (matches, current_match) = app.router.search_matches_for_snapshot(&snapshot);
        assert!(matches.is_empty());
        assert_eq!(current_match, None);
        assert_eq!(app.router.search_query_for_render(), None);
    }

    #[test]
    fn ctrl_shift_comma_requests_config_reload_without_writing_to_pty() {
        let mut app = AppHarness::new();
        app.router
            .set_modifiers(ModifiersState::CONTROL | ModifiersState::SHIFT);

        app.send_key(Key::Character(",".into()), Some(","));

        assert!(app.router.take_config_reload_request());
        assert!(!app.router.take_config_reload_request());
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn ime_committed_text_uses_text_input_path_and_clears_selection() {
        let mut app = AppHarness::new();
        app.feed_text("hello");
        app.left_drag((0, 0), (4, 0));

        app.ime_commit("日本語");

        assert!(!app.has_selection());
        assert_eq!(app.pty_writes(), &["日本語".as_bytes().to_vec()]);
    }

    #[test]
    fn ime_preedit_is_render_state_only() {
        let mut app = AppHarness::new();

        app.ime_preedit("nihon", Some(1..3));

        assert!(app.pty_writes().is_empty());
        assert_eq!(
            app.rendered_preedit(),
            Some(ImePreedit {
                text: "nihon".to_owned(),
                cursor_range: Some(1..3),
            })
        );
    }

    #[test]
    fn ime_commit_writes_once_and_clears_preedit() {
        let mut app = AppHarness::new();
        app.ime_preedit("にほん", Some(0.."にほん".len()));

        app.ime_commit("日本");

        assert_eq!(app.pty_writes(), &["日本".as_bytes().to_vec()]);
        assert_eq!(app.rendered_preedit(), None);
    }

    #[test]
    fn empty_ime_preedit_clears_state() {
        let mut app = AppHarness::new();
        app.ime_preedit("abc", Some(0..1));

        app.ime_preedit("", None);

        assert_eq!(app.rendered_preedit(), None);
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn focus_lost_clears_ime_preedit() {
        let mut app = AppHarness::new();
        app.ime_preedit("abc", Some(0..1));

        app.focus(false);

        assert_eq!(app.rendered_preedit(), None);
    }

    #[test]
    fn escape_clears_ime_preedit_and_keeps_terminal_escape_routing() {
        let mut app = AppHarness::new();
        app.ime_preedit("abc", Some(0..1));

        app.send_key(Key::Named(NamedKey::Escape), None);

        assert_eq!(app.rendered_preedit(), None);
        assert_eq!(app.pty_writes(), &[b"\x1b".to_vec()]);
    }

    #[test]
    fn named_keyboard_keys_write_terminal_sequences() {
        let cases: &[(NamedKey, &[u8])] = &[
            (NamedKey::Enter, b"\r"),
            (NamedKey::Backspace, b"\x7f"),
            (NamedKey::Tab, b"\t"),
            (NamedKey::Escape, b"\x1b"),
            (NamedKey::ArrowUp, b"\x1b[A"),
            (NamedKey::ArrowDown, b"\x1b[B"),
            (NamedKey::ArrowRight, b"\x1b[C"),
            (NamedKey::ArrowLeft, b"\x1b[D"),
            (NamedKey::Home, b"\x1b[H"),
            (NamedKey::End, b"\x1b[F"),
            (NamedKey::PageUp, b"\x1b[5~"),
            (NamedKey::PageDown, b"\x1b[6~"),
            (NamedKey::Insert, b"\x1b[2~"),
            (NamedKey::Delete, b"\x1b[3~"),
            (NamedKey::F1, b"\x1bOP"),
            (NamedKey::F2, b"\x1bOQ"),
            (NamedKey::F3, b"\x1bOR"),
            (NamedKey::F4, b"\x1bOS"),
            (NamedKey::F5, b"\x1b[15~"),
            (NamedKey::F6, b"\x1b[17~"),
            (NamedKey::F7, b"\x1b[18~"),
            (NamedKey::F8, b"\x1b[19~"),
            (NamedKey::F9, b"\x1b[20~"),
            (NamedKey::F10, b"\x1b[21~"),
            (NamedKey::F11, b"\x1b[23~"),
            (NamedKey::F12, b"\x1b[24~"),
        ];

        for &(key, expected) in cases {
            let mut app = AppHarness::new();

            app.send_key(Key::Named(key), None);

            assert_eq!(app.pty_writes(), &[expected.to_vec()], "{key:?}");
        }
    }

    #[test]
    fn plain_ctrl_c_sends_interrupt_and_is_not_copy() {
        let mut app = AppHarness::new();
        app.feed_text("hello");
        app.left_drag((0, 0), (4, 0));

        app.set_modifiers(ModifiersState::CONTROL);
        app.send_key(Key::Character("c".into()), Some("c"));
        app.set_modifiers(ModifiersState::empty());

        assert_eq!(app.clipboard(), "");
        assert_eq!(app.pty_writes(), &[b"\x03".to_vec()]);
    }

    #[test]
    fn representative_ctrl_character_mappings_are_written_to_pty() {
        let cases: &[(&str, &[u8])] = &[
            ("a", b"\x01"),
            ("z", b"\x1a"),
            ("[", b"\x1b"),
            ("\\", b"\x1c"),
            ("]", b"\x1d"),
            ("^", b"\x1e"),
            ("_", b"\x1f"),
            (" ", b"\x00"),
        ];

        for &(value, expected) in cases {
            let mut app = AppHarness::new();

            app.set_modifiers(ModifiersState::CONTROL);
            app.send_key(Key::Character(value.into()), Some(value));
            app.set_modifiers(ModifiersState::empty());

            assert_eq!(app.pty_writes(), &[expected.to_vec()], "Ctrl+{value}");
        }
    }

    #[test]
    fn named_ctrl_space_writes_nul_without_text_payload() {
        let mut app = AppHarness::new();

        app.set_modifiers(ModifiersState::CONTROL);
        app.send_key(Key::Named(NamedKey::Space), None);
        app.set_modifiers(ModifiersState::empty());

        assert_eq!(app.pty_writes(), &[b"\x00".to_vec()]);
    }

    #[test]
    fn alt_printable_text_is_escape_prefixed() {
        let mut app = AppHarness::new();

        app.set_modifiers(ModifiersState::ALT);
        app.send_key(Key::Character("x".into()), Some("x"));
        app.set_modifiers(ModifiersState::empty());

        assert_eq!(app.pty_writes(), &[b"\x1bx".to_vec()]);
    }

    #[test]
    fn alt_named_space_is_escape_prefixed_without_text_payload() {
        let mut app = AppHarness::new();

        app.set_modifiers(ModifiersState::ALT);
        app.send_key(Key::Named(NamedKey::Space), None);
        app.set_modifiers(ModifiersState::empty());

        assert_eq!(app.pty_writes(), &[b"\x1b ".to_vec()]);
    }

    #[test]
    fn ctrl_shift_page_shortcuts_do_not_write_to_pty_or_clear_selection() {
        let mut app = AppHarness::new();
        app.feed_text("hello");
        app.left_drag((0, 0), (4, 0));

        app.set_modifiers(ModifiersState::CONTROL | ModifiersState::SHIFT);
        app.send_key(Key::Named(NamedKey::PageUp), None);
        app.send_key(Key::Named(NamedKey::PageDown), None);
        app.set_modifiers(ModifiersState::empty());

        assert!(app.has_selection());
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn paste_clears_selection() {
        let mut app = AppHarness::new();
        app.feed_text("hello");
        app.left_drag((0, 0), (4, 0));
        app.set_clipboard("pasted");

        app.paste_shortcut();

        assert!(!app.has_selection());
        assert_eq!(app.pty_writes(), &[b"pasted".to_vec()]);
    }

    #[test]
    fn ctrl_shift_c_copies_without_clearing_selection() {
        let mut app = AppHarness::new();
        app.feed_text("hello");
        app.left_drag((0, 0), (4, 0));

        app.copy_shortcut();

        assert_eq!(app.clipboard(), "hello");
        assert!(app.has_selection());
        assert!(app.pty_writes().is_empty());
    }

    #[test]
    fn left_click_outside_selection_clears_selection() {
        let mut app = AppHarness::new();
        app.feed_text("hello world");
        app.left_drag((0, 0), (4, 0));

        app.move_to(7, 0);
        app.left_press();
        app.left_release();

        assert!(!app.has_selection());
    }

    #[test]
    fn ctrl_shift_c_copies_selected_text_to_fake_clipboard() {
        let mut app = AppHarness::new();
        app.feed_text("copy me");
        app.left_drag((0, 0), (3, 0));

        app.copy_shortcut();

        assert_eq!(app.clipboard(), "copy");
    }

    #[test]
    fn ctrl_shift_v_writes_fake_clipboard_text_to_fake_pty() {
        let mut app = AppHarness::new();
        app.set_clipboard("paste me");

        app.paste_shortcut();

        assert_eq!(app.pty_writes(), &[b"paste me".to_vec()]);
    }

    #[test]
    fn shift_insert_writes_fake_clipboard_text_to_fake_pty() {
        let mut app = AppHarness::new();
        app.set_clipboard("insert paste");

        app.shift_insert_paste();

        assert_eq!(app.pty_writes(), &[b"insert paste".to_vec()]);
    }

    #[test]
    fn multiline_paste_preserves_current_newline_policy() {
        let mut app = AppHarness::new();
        app.set_clipboard("one\ntwo");

        app.paste_shortcut();

        assert_eq!(app.pty_writes(), &[b"one\ntwo".to_vec()]);
    }

    #[test]
    fn japanese_utf8_paste_is_preserved() {
        let mut app = AppHarness::new();
        app.set_clipboard("こんにちは");

        app.paste_shortcut();

        assert_eq!(app.pty_writes(), &["こんにちは".as_bytes().to_vec()]);
    }

    #[test]
    fn pasting_url_writes_to_pty_and_does_not_open_url() {
        let mut app = AppHarness::new();
        app.set_clipboard("https://example.com");

        app.paste_shortcut();

        assert_eq!(app.pty_writes(), &[b"https://example.com".to_vec()]);
        assert!(app.opened_urls().is_empty());
    }

    #[test]
    fn bracketed_paste_policy_is_preserved_for_multiline_text() {
        let mut app = AppHarness::new();
        app.feed(b"\x1b[?2004h");
        app.set_clipboard("one\ntwo");

        app.paste_shortcut();

        assert_eq!(app.pty_writes(), &[b"\x1b[200~one\ntwo\x1b[201~".to_vec()]);
    }

    #[test]
    fn allowed_url_validation_remains_scheme_based() {
        let allowed = ["https".to_owned(), "http".to_owned()];

        assert_eq!(
            allowed_hyperlink_url("https://example.com", &allowed),
            Ok("https://example.com/".to_owned())
        );
    }
}
