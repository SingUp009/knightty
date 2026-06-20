//! GUI-independent terminal state.
//!
//! ```
//! use knightty_core::Terminal;
//!
//! let mut term = Terminal::new(80, 24);
//! term.feed(b"hello");
//!
//! let grid = term.snapshot();
//! assert_eq!(grid.cell(0, 0).ch, 'h');
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags as AlacrittyFlags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AlacrittyColor, NamedColor, Processor, Rgb as AlacrittyRgb,
};

/// GUI-independent terminal state wrapper.
pub struct Terminal {
    term: Term<TerminalEventSink>,
    processor: Processor,
    damage: Damage,
    pty_writes: Arc<Mutex<Vec<String>>>,
    title_events: Arc<Mutex<Vec<Option<String>>>>,
    window_title: String,
    pending_window_title: Option<String>,
    pending_compat_input: Vec<u8>,
    mouse_state: MouseModeState,
    selection: Option<SelectionState>,
    image_placements: Vec<ImagePlacement>,
}

/// Enabled xterm mouse reporting protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseProtocol {
    Off,
    X10,
    Normal,
    ButtonMotion,
    AnyMotion,
}

/// Enabled xterm mouse coordinate encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEncoding {
    Default,
    Sgr,
}

/// Mouse and focus reporting state needed by GUI frontends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseModes {
    pub protocol: MouseProtocol,
    pub encoding: MouseEncoding,
    pub focus_events: bool,
}

impl Default for MouseModes {
    fn default() -> Self {
        Self {
            protocol: MouseProtocol::Off,
            encoding: MouseEncoding::Default,
            focus_events: false,
        }
    }
}

/// Mouse button reported to a terminal application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    Other(u8),
}

/// Mouse event kind reported to a terminal application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    Move,
    Drag,
    Wheel,
}

/// Mouse event in terminal cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalMouseEvent {
    pub kind: MouseEventKind,
    pub button: Option<MouseButton>,
    pub col: usize,
    pub row: usize,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Selection endpoint in terminal logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPoint {
    pub col: usize,
    pub row: isize,
}

/// Selection expansion mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    Simple,
    Word,
    Line,
}

/// Current terminal text selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionState {
    pub anchor: SelectionPoint,
    pub focus: SelectionPoint,
    pub mode: SelectionMode,
    pub active: bool,
}

/// Visible selected range in viewport cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRect {
    pub col: usize,
    pub row: usize,
    pub width: usize,
    pub height: usize,
}

/// Stable identifier for an image resource owned outside the terminal core.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImageId(u64);

impl ImageId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// Stable identifier for one logical placement of an image resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImagePlacementId(u64);

impl ImagePlacementId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// Terminal-grid position. Rows may be negative while an image is in scrollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GridPoint {
    pub col: usize,
    pub row: isize,
}

/// Logical cell placement for one externally-owned image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImagePlacement {
    pub placement_id: ImagePlacementId,
    pub image_id: ImageId,
    pub anchor: GridPoint,
    pub columns: u16,
    pub rows: u16,
    pub source_width: u32,
    pub source_height: u32,
}

impl Terminal {
    /// Create a terminal with the provided visible grid size.
    ///
    /// ```
    /// use knightty_core::Terminal;
    ///
    /// let term = Terminal::new(80, 24);
    /// assert_eq!(term.snapshot().cols, 80);
    /// assert_eq!(term.snapshot().rows, 24);
    /// ```
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_scrollback(cols, rows, DEFAULT_SCROLLBACK_LINES)
    }

    /// Create a terminal with an explicit primary-screen scrollback limit.
    pub fn with_scrollback(cols: usize, rows: usize, scrollback_lines: usize) -> Self {
        let size = TermSize::new(cols.max(1), rows.max(1));
        let pty_writes = Arc::new(Mutex::new(Vec::new()));
        let title_events = Arc::new(Mutex::new(Vec::new()));
        let event_sink = TerminalEventSink {
            pty_writes: Arc::clone(&pty_writes),
            title_events: Arc::clone(&title_events),
        };
        let config = Config {
            scrolling_history: scrollback_lines,
            ..Config::default()
        };
        let mut term = Term::new(config, &size, event_sink);
        term.reset_damage();

        Self {
            term,
            processor: Processor::new(),
            damage: Damage::Full,
            pty_writes,
            title_events,
            window_title: String::new(),
            pending_window_title: None,
            pending_compat_input: Vec::new(),
            mouse_state: MouseModeState::default(),
            selection: None,
            image_placements: Vec::new(),
        }
    }

    /// Resize the visible grid.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.clear_selection();
        let size = TermSize::new(cols.max(1), rows.max(1));
        self.term.resize(size);
        self.damage = Damage::Full;
        self.term.reset_damage();
    }

    /// Return the current cursor position for a new image anchor.
    pub fn image_anchor(&self) -> GridPoint {
        let cursor = self.term.grid().cursor.point;
        GridPoint {
            col: cursor.column.0,
            row: cursor.line.0 as isize,
        }
    }

    /// Add a logical image placement without taking ownership of image pixels.
    pub fn add_image_placement(&mut self, placement: ImagePlacement) {
        self.image_placements.push(placement);
        self.mark_full_damage();
    }

    /// Add a placement or atomically replace the placement with the same id.
    pub fn upsert_image_placement(&mut self, placement: ImagePlacement) {
        if let Some(existing) = self
            .image_placements
            .iter_mut()
            .find(|existing| existing.placement_id == placement.placement_id)
        {
            *existing = placement;
        } else {
            self.image_placements.push(placement);
        }
        self.mark_full_damage();
    }

    /// Remove one logical image placement.
    pub fn remove_image_placement(&mut self, placement_id: ImagePlacementId) {
        let previous_len = self.image_placements.len();
        self.image_placements
            .retain(|placement| placement.placement_id != placement_id);
        if self.image_placements.len() != previous_len {
            self.mark_full_damage();
        }
    }

    /// Remove all placements that reference one image resource.
    pub fn remove_image_placements(&mut self, image_id: ImageId) {
        let previous_len = self.image_placements.len();
        self.image_placements
            .retain(|placement| placement.image_id != image_id);
        if self.image_placements.len() != previous_len {
            self.mark_full_damage();
        }
    }

    /// Remove every image placement.
    pub fn clear_image_placements(&mut self) {
        if !self.image_placements.is_empty() {
            self.image_placements.clear();
            self.mark_full_damage();
        }
    }

    /// Iterate over all image resources still referenced by terminal history.
    pub fn image_placement_ids(&self) -> impl Iterator<Item = ImageId> + '_ {
        self.image_placements
            .iter()
            .map(|placement| placement.image_id)
    }

    /// Iterate over stable ids for logical placements still in terminal history.
    pub fn placement_ids(&self) -> impl Iterator<Item = ImagePlacementId> + '_ {
        self.image_placements
            .iter()
            .map(|placement| placement.placement_id)
    }

    /// Advance the terminal cursor after displaying a block image.
    pub fn advance_after_image_rows(&mut self, rows: u16) {
        if rows == 0 {
            return;
        }

        let previous_history = self.term.grid().history_size();
        for _ in 0..rows {
            self.processor.advance(&mut self.term, b"\r\n");
        }
        self.apply_output_scroll(previous_history);
        let damage = self.collect_damage();
        self.damage = merge_damage(
            core::mem::replace(&mut self.damage, Damage::None),
            merge_damage(damage, Damage::Full),
        );
    }

    /// Move the cursor to the cell immediately following a Kitty image rectangle.
    pub fn move_after_kitty_image(&mut self, columns: u16, rows: u16) {
        if columns == 0 || rows == 0 {
            return;
        }

        let terminal_columns = self.term.grid().columns().max(1);
        let cursor_column = self.term.grid().cursor.point.column.0;
        let horizontal = cursor_column.saturating_add(usize::from(columns));
        let wrapped = usize::from(horizontal >= terminal_columns);
        let target_column = if wrapped == 0 { horizontal } else { 0 };
        let down = usize::from(rows.saturating_sub(1)).saturating_add(wrapped);

        let mut sequence = Vec::with_capacity(32);
        if down != 0 {
            sequence.extend_from_slice(format!("\x1b[{down}B").as_bytes());
        }
        sequence.extend_from_slice(format!("\x1b[{}G", target_column + 1).as_bytes());

        let previous_history = self.term.grid().history_size();
        self.processor.advance(&mut self.term, &sequence);
        self.apply_output_scroll(previous_history);
        let damage = self.collect_damage();
        self.damage = merge_damage(
            core::mem::replace(&mut self.damage, Damage::None),
            merge_damage(damage, Damage::Full),
        );
    }

    /// Return the number of lines currently stored in the active screen scrollback.
    pub fn scrollback_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Return the current viewport offset into scrollback.
    pub fn scroll_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scroll the viewport up by a number of lines.
    pub fn scroll_up_lines(&mut self, lines: usize) -> Damage {
        let lines = lines.min(self.scrollback_len().saturating_sub(self.scroll_offset()));
        if lines == 0 {
            return Damage::None;
        }

        self.scroll_display(Scroll::Delta(usize_to_i32(lines)))
    }

    /// Scroll the viewport down by a number of lines.
    pub fn scroll_down_lines(&mut self, lines: usize) -> Damage {
        let lines = lines.min(self.scroll_offset());
        if lines == 0 {
            return Damage::None;
        }

        self.scroll_display(Scroll::Delta(-usize_to_i32(lines)))
    }

    /// Scroll to the oldest available scrollback line.
    pub fn scroll_to_top(&mut self) -> Damage {
        self.scroll_display(Scroll::Top)
    }

    /// Scroll to the live bottom of the terminal.
    pub fn scroll_to_bottom(&mut self) -> Damage {
        self.scroll_display(Scroll::Bottom)
    }

    /// Return whether the viewport is currently looking at scrollback.
    pub fn is_scrolled_back(&self) -> bool {
        self.scroll_offset() > 0
    }

    /// Return the current visible grid size as `(cols, rows)`.
    pub fn size(&self) -> (usize, usize) {
        let grid = self.term.grid();
        (grid.columns(), grid.screen_lines())
    }

    /// Return the current terminal window title.
    pub fn window_title(&self) -> &str {
        &self.window_title
    }

    /// Return and clear the latest terminal window title update.
    pub fn take_window_title_changed(&mut self) -> Option<String> {
        self.pending_window_title.take()
    }

    /// Feed PTY bytes into the VT state machine.
    ///
    /// ```
    /// use knightty_core::{Color, Terminal};
    ///
    /// let mut term = Terminal::new(80, 24);
    /// term.feed(b"\x1b[38;2;255;0;0mX");
    ///
    /// assert_eq!(term.snapshot().cell(0, 0).fg, Color::Rgb(255, 0, 0));
    /// ```
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        self.clear_selection();
        self.scroll_to_bottom();
        let previous_history = self.term.grid().history_size();

        let mut input = core::mem::take(&mut self.pending_compat_input);
        input.extend_from_slice(bytes);
        let reset_images = input.windows(2).any(|window| window == b"\x1bc");

        let pending_start = self.process_compat_input(&input);
        if pending_start < input.len() {
            self.pending_compat_input
                .extend_from_slice(&input[pending_start..]);
        }
        if reset_images {
            self.clear_image_placements();
        }
        self.apply_title_events();
        self.apply_output_scroll(previous_history);

        let damage = self.collect_damage();
        self.damage = merge_damage(core::mem::replace(&mut self.damage, Damage::None), damage);
    }

    /// Return whether bracketed paste mode is currently enabled.
    ///
    /// ```
    /// use knightty_core::Terminal;
    ///
    /// let mut term = Terminal::new(80, 24);
    /// assert!(!term.bracketed_paste_enabled());
    ///
    /// term.feed(b"\x1b[?2004h");
    /// assert!(term.bracketed_paste_enabled());
    /// ```
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Return whether the alternate screen is currently active.
    pub fn alternate_screen_enabled(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Return the current mouse and focus reporting modes.
    pub fn mouse_modes(&self) -> MouseModes {
        self.mouse_state.modes()
    }

    /// Return whether focus in/out reporting is enabled.
    pub fn focus_events_enabled(&self) -> bool {
        self.mouse_state.focus_events
    }

    /// Encode a terminal mouse event for writing to the PTY.
    pub fn encode_mouse_event(&self, event: TerminalMouseEvent) -> Option<Vec<u8>> {
        let modes = self.mouse_modes();
        if !event_is_reported(modes.protocol, event) {
            return None;
        }

        match modes.encoding {
            MouseEncoding::Sgr => encode_sgr_mouse_event(event),
            MouseEncoding::Default => encode_default_mouse_event(event),
        }
    }

    /// Encode a focus in/out event for writing to the PTY.
    pub fn encode_focus_event(&self, focused: bool) -> Option<Vec<u8>> {
        if !self.focus_events_enabled() {
            return None;
        }

        Some(if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        })
    }

    /// Wrap pasted bytes when bracketed paste mode is enabled.
    ///
    /// ```
    /// use knightty_core::Terminal;
    ///
    /// let mut term = Terminal::new(80, 24);
    /// assert_eq!(term.paste_bytes(b"hello"), b"hello");
    ///
    /// term.feed(b"\x1b[?2004h");
    /// assert_eq!(
    ///     term.paste_bytes(b"hello"),
    ///     b"\x1b[200~hello\x1b[201~",
    /// );
    /// ```
    pub fn paste_bytes(&self, bytes: &[u8]) -> Vec<u8> {
        if !self.bracketed_paste_enabled() {
            return bytes.to_vec();
        }

        let mut wrapped = Vec::with_capacity(
            bytes.len() + BRACKETED_PASTE_START.len() + BRACKETED_PASTE_END.len(),
        );
        wrapped.extend_from_slice(BRACKETED_PASTE_START);
        wrapped.extend_from_slice(bytes);
        wrapped.extend_from_slice(BRACKETED_PASTE_END);
        wrapped
    }

    /// Convert a visible viewport cell to a logical selection point.
    pub fn selection_point_for_visible_cell(
        &self,
        col: usize,
        row: usize,
    ) -> Option<SelectionPoint> {
        let grid = self.term.grid();
        if col >= grid.columns() || row >= grid.screen_lines() {
            return None;
        }

        let point = SelectionPoint {
            col,
            row: usize_to_isize(row).saturating_sub(usize_to_isize(grid.display_offset())),
        };
        self.clamp_selection_point(point)
    }

    /// Clear the current text selection.
    pub fn clear_selection(&mut self) {
        if self.selection.take().is_some() {
            self.mark_full_damage();
        }
    }

    /// Begin a text selection at a logical point.
    pub fn begin_selection(&mut self, point: SelectionPoint, mode: SelectionMode) {
        let Some(point) = self.clamp_selection_point(point) else {
            self.clear_selection();
            return;
        };

        self.selection = Some(SelectionState {
            anchor: point,
            focus: point,
            mode,
            active: true,
        });
        self.mark_full_damage();
    }

    /// Update the current text selection focus point.
    pub fn update_selection(&mut self, point: SelectionPoint) {
        let Some(point) = self.clamp_selection_point(point) else {
            return;
        };
        let Some(selection) = &mut self.selection else {
            return;
        };

        selection.focus = point;
        self.mark_full_damage();
    }

    /// Mark the current text selection as no longer actively dragged.
    pub fn end_selection(&mut self) {
        let Some(selection) = &mut self.selection else {
            return;
        };

        if selection.active {
            selection.active = false;
            self.mark_full_damage();
        }
    }

    /// Return whether a text selection currently exists.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// Return selected terminal text.
    pub fn selected_text(&self) -> Option<String> {
        let range = self.effective_selection_range()?;
        let mut lines = Vec::new();

        for row in range.start.row..=range.end.row {
            let start_col = if row == range.start.row {
                range.start.col
            } else {
                0
            };
            let end_col = if row == range.end.row {
                range.end.col
            } else {
                self.term.grid().columns().saturating_sub(1)
            };
            lines.push(self.line_to_selected_string(row, start_col, end_col));
        }

        let text = lines.join("\n");
        if text.is_empty() { None } else { Some(text) }
    }

    /// Return selected ranges visible in the current viewport.
    pub fn selection_rects(&self) -> Vec<SelectionRect> {
        let Some(range) = self.effective_selection_range() else {
            return Vec::new();
        };
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let display_offset = usize_to_isize(grid.display_offset());
        let mut rects = Vec::new();

        for logical_row in range.start.row..=range.end.row {
            let visible_row = logical_row.saturating_add(display_offset);
            if visible_row < 0 {
                continue;
            }
            let visible_row = visible_row as usize;
            if visible_row >= rows {
                continue;
            }

            let start_col = if logical_row == range.start.row {
                range.start.col
            } else {
                0
            };
            let end_col = if logical_row == range.end.row {
                range.end.col
            } else {
                cols.saturating_sub(1)
            };
            if cols == 0 || start_col >= cols {
                continue;
            }
            let end_col = end_col.min(cols.saturating_sub(1));
            if end_col < start_col {
                continue;
            }

            rects.push(SelectionRect {
                col: start_col,
                row: visible_row,
                width: end_col - start_col + 1,
                height: 1,
            });
        }

        rects
    }

    /// Return whether a visible viewport cell is inside the current selection.
    pub fn selection_contains_visible_cell(&self, col: usize, row: usize) -> bool {
        let Some(point) = self.selection_point_for_visible_cell(col, row) else {
            return false;
        };
        self.selection_contains_point(point)
    }

    /// Return the hyperlink metadata at a visible viewport cell.
    pub fn hyperlink_at_cell(&self, col: usize, row: usize) -> Option<Hyperlink> {
        self.snapshot().hyperlink_at_cell(col, row).cloned()
    }

    /// Return a render-safe copy of the visible grid.
    pub fn snapshot(&self) -> GridSnapshot {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let display_offset = grid.display_offset();
        let mut cells = vec![Cell::default(); cols * rows];
        let mut hyperlinks = Vec::new();
        let mut hyperlink_indexes = HashMap::<Hyperlink, usize>::new();
        let image_placements = self
            .image_placements
            .iter()
            .filter_map(|placement| {
                let visible_row = placement.anchor.row + display_offset as isize;
                let bottom = visible_row + placement.rows as isize;
                if bottom <= 0 || visible_row >= rows as isize {
                    return None;
                }

                Some(ImagePlacement {
                    anchor: GridPoint {
                        col: placement.anchor.col,
                        row: visible_row,
                    },
                    ..*placement
                })
            })
            .collect();

        for indexed in grid.display_iter() {
            let viewport_line = indexed.point.line.0 + display_offset as i32;
            if viewport_line < 0 {
                continue;
            }

            let x = indexed.point.column.0;
            let y = viewport_line as usize;
            if x >= cols || y >= rows {
                continue;
            }

            let mut cell = Cell::from_alacritty(indexed.cell);
            if let Some(hyperlink) = indexed.cell.hyperlink().and_then(hyperlink_from_alacritty) {
                let hyperlink_index =
                    if let Some(index) = hyperlink_indexes.get(&hyperlink).copied() {
                        index
                    } else {
                        let index = hyperlinks.len();
                        hyperlinks.push(hyperlink.clone());
                        hyperlink_indexes.insert(hyperlink, index);
                        index
                    };
                cell.hyperlink = Some(hyperlink_index);
            }
            cells[y * cols + x] = cell;
        }

        let cursor_point = grid.cursor.point;
        let cursor_x = cursor_point.column.0;
        let cursor_y = usize::try_from(cursor_point.line.0).unwrap_or(rows);
        let cursor_visible = !self.is_scrolled_back()
            && self.term.mode().contains(TermMode::SHOW_CURSOR)
            && cursor_x < cols
            && cursor_y < rows;

        GridSnapshot {
            cols,
            rows,
            cells,
            hyperlinks,
            selection_rects: self.selection_rects(),
            image_placements,
            cursor: Cursor {
                x: cursor_x,
                y: cursor_y,
                visible: cursor_visible,
            },
        }
    }

    /// Return and clear accumulated damage.
    pub fn take_damage(&mut self) -> Damage {
        let damage = core::mem::replace(&mut self.damage, Damage::None);
        self.term.reset_damage();
        damage
    }

    /// Return and clear terminal-generated replies that must be written back to the PTY.
    pub fn take_pty_writes(&mut self) -> Vec<String> {
        let Ok(mut pty_writes) = self.pty_writes.lock() else {
            return Vec::new();
        };

        core::mem::take(&mut *pty_writes)
    }

    fn collect_damage(&mut self) -> Damage {
        let damage = match self.term.damage() {
            TermDamage::Full => Damage::Full,
            TermDamage::Partial(lines) => {
                let lines = lines
                    .map(|line| LineDamage {
                        line: line.line,
                        left: line.left,
                        right: line.right,
                    })
                    .collect::<Vec<_>>();

                if lines.is_empty() {
                    Damage::None
                } else {
                    Damage::Lines(lines)
                }
            }
        };
        self.term.reset_damage();
        damage
    }

    fn apply_output_scroll(&mut self, previous_history: usize) {
        let current_history = self.term.grid().history_size();
        let added_history = current_history.saturating_sub(previous_history);
        if added_history != 0 {
            let shift = isize::try_from(added_history).unwrap_or(isize::MAX);
            for placement in &mut self.image_placements {
                placement.anchor.row = placement.anchor.row.saturating_sub(shift);
            }
            self.mark_full_damage();
        }

        let topmost_row = -(current_history as isize);
        self.image_placements
            .retain(|placement| placement.anchor.row + placement.rows as isize > topmost_row);
    }

    fn scroll_display(&mut self, scroll: Scroll) -> Damage {
        let previous_offset = self.scroll_offset();
        self.term.scroll_display(scroll);
        if self.scroll_offset() == previous_offset {
            Damage::None
        } else {
            self.damage = merge_damage(
                core::mem::replace(&mut self.damage, Damage::None),
                Damage::Full,
            );
            Damage::Full
        }
    }

    fn mark_full_damage(&mut self) {
        self.damage = merge_damage(
            core::mem::replace(&mut self.damage, Damage::None),
            Damage::Full,
        );
    }

    fn clamp_selection_point(&self, point: SelectionPoint) -> Option<SelectionPoint> {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        if cols == 0 || rows == 0 {
            return None;
        }

        let min_row = -usize_to_isize(grid.history_size());
        let max_row = usize_to_isize(rows).saturating_sub(1);
        Some(SelectionPoint {
            col: point.col.min(cols.saturating_sub(1)),
            row: point.row.clamp(min_row, max_row),
        })
    }

    fn effective_selection_range(&self) -> Option<SelectionRange> {
        let selection = self.selection?;
        match selection.mode {
            SelectionMode::Simple => {
                let anchor = self.clamp_selection_point(selection.anchor)?;
                let focus = self.clamp_selection_point(selection.focus)?;
                if anchor == focus {
                    return None;
                }
                Some(SelectionRange::normalized(anchor, focus))
            }
            SelectionMode::Word => {
                let anchor = self.word_range_at(selection.anchor)?;
                let focus = self.word_range_at(selection.focus)?;
                if selection_point_le(anchor.start, focus.start) {
                    Some(SelectionRange::normalized(anchor.start, focus.end))
                } else {
                    Some(SelectionRange::normalized(focus.start, anchor.end))
                }
            }
            SelectionMode::Line => {
                let anchor = self.clamp_selection_point(selection.anchor)?;
                let focus = self.clamp_selection_point(selection.focus)?;
                let start_row = anchor.row.min(focus.row);
                let end_row = anchor.row.max(focus.row);
                Some(SelectionRange {
                    start: SelectionPoint {
                        col: 0,
                        row: start_row,
                    },
                    end: SelectionPoint {
                        col: self.term.grid().columns().saturating_sub(1),
                        row: end_row,
                    },
                })
            }
        }
    }

    fn selection_contains_point(&self, point: SelectionPoint) -> bool {
        let Some(range) = self.effective_selection_range() else {
            return false;
        };
        let Some(point) = self.clamp_selection_point(point) else {
            return false;
        };

        selection_point_le(range.start, point) && selection_point_le(point, range.end)
    }

    fn word_range_at(&self, point: SelectionPoint) -> Option<SelectionRange> {
        let point = self.clamp_selection_point(point)?;
        let mut start_col = point.col;
        let mut end_col = point.col;

        if !is_word_constituent(self.cell_char(point.row, point.col)?) {
            return None;
        }

        while start_col > 0
            && self
                .cell_char(point.row, start_col - 1)
                .is_some_and(is_word_constituent)
        {
            start_col -= 1;
        }

        let last_col = self.term.grid().columns().saturating_sub(1);
        while end_col < last_col
            && self
                .cell_char(point.row, end_col + 1)
                .is_some_and(is_word_constituent)
        {
            end_col += 1;
        }

        Some(SelectionRange {
            start: SelectionPoint {
                col: start_col,
                row: point.row,
            },
            end: SelectionPoint {
                col: end_col,
                row: point.row,
            },
        })
    }

    fn cell_char(&self, row: isize, col: usize) -> Option<char> {
        let grid = self.term.grid();
        if col >= grid.columns() {
            return None;
        }

        Some(grid[line_for_row(row)?][Column(col)].c)
    }

    fn line_to_selected_string(&self, row: isize, start_col: usize, end_col: usize) -> String {
        let grid = self.term.grid();
        let cols = grid.columns();
        let Some(line) = line_for_row(row) else {
            return String::new();
        };
        if cols == 0 || start_col >= cols {
            return String::new();
        }

        let mut start_col = start_col;
        let end_col = end_col.min(cols.saturating_sub(1));
        if end_col < start_col {
            return String::new();
        }

        if start_col > 0
            && grid[line][Column(start_col)]
                .flags
                .intersects(WIDE_SPACER_FLAGS)
        {
            start_col -= 1;
        }

        let mut text = String::new();
        for col in start_col..=end_col {
            let cell = &grid[line][Column(col)];
            if cell.flags.intersects(WIDE_SPACER_FLAGS) {
                continue;
            }

            text.push(cell.c);
            for ch in cell.zerowidth().into_iter().flatten() {
                text.push(*ch);
            }
        }

        text.trim_end().to_owned()
    }

    fn process_compat_input(&mut self, input: &[u8]) -> usize {
        let mut chunk_start = 0;
        let mut cursor = 0;

        while cursor < input.len() {
            match scan_csi(input, cursor) {
                CsiScan::NotCsi => cursor += 1,
                CsiScan::Incomplete => break,
                CsiScan::Complete { end, action } => {
                    if let Some(action) = action {
                        self.advance_processor(&input[chunk_start..cursor]);
                        self.apply_compat_action(action);
                        cursor = end + 1;
                        chunk_start = cursor;
                    } else {
                        cursor = end + 1;
                    }
                }
            }
        }

        self.advance_processor(&input[chunk_start..cursor]);
        cursor
    }

    fn advance_processor(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.processor.advance(&mut self.term, bytes);
        }
    }

    fn apply_title_events(&mut self) {
        let title_event = {
            let Ok(mut title_events) = self.title_events.lock() else {
                return;
            };
            title_events.drain(..).next_back()
        };

        let Some(title) = title_event else {
            return;
        };

        let title = sanitize_window_title(title.as_deref().unwrap_or_default());
        self.window_title.clone_from(&title);
        self.pending_window_title = Some(title);
    }

    fn apply_compat_action(&mut self, action: CompatAction) {
        match action {
            CompatAction::SetAltScreen => {
                if !self.term.mode().contains(TermMode::ALT_SCREEN) {
                    self.term.swap_alt();
                }
            }
            CompatAction::ResetAltScreen => {
                if self.term.mode().contains(TermMode::ALT_SCREEN) {
                    self.term.swap_alt();
                }
            }
            CompatAction::SaveCursor => self.processor.advance(&mut self.term, b"\x1b7"),
            CompatAction::RestoreCursor => self.processor.advance(&mut self.term, b"\x1b8"),
            CompatAction::PrivateModes { modes, enabled } => {
                for mode in modes {
                    self.apply_private_mode(mode, enabled);
                }
            }
        }
    }

    fn apply_private_mode(&mut self, mode: CompatPrivateMode, enabled: bool) {
        match mode {
            CompatPrivateMode::X10Mouse => self.mouse_state.x10 = enabled,
            CompatPrivateMode::NormalMouse => self.mouse_state.normal = enabled,
            CompatPrivateMode::ButtonMotionMouse => self.mouse_state.button_motion = enabled,
            CompatPrivateMode::AnyMotionMouse => self.mouse_state.any_motion = enabled,
            CompatPrivateMode::FocusEvents => self.mouse_state.focus_events = enabled,
            CompatPrivateMode::SgrMouse => {
                self.mouse_state.encoding = if enabled {
                    MouseEncoding::Sgr
                } else {
                    MouseEncoding::Default
                };
            }
            CompatPrivateMode::UnsupportedMouseEncoding => {}
        }
    }
}

const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const WIDE_SPACER_FLAGS: AlacrittyFlags =
    AlacrittyFlags::WIDE_CHAR_SPACER.union(AlacrittyFlags::LEADING_WIDE_CHAR_SPACER);

const ESC: u8 = 0x1b;
const CSI: u8 = 0x9b;
const MAX_COMPAT_CSI_LEN: usize = 64;
const MAX_WINDOW_TITLE_CHARS: usize = 1024;
const MAX_HYPERLINK_URI_BYTES: usize = 2048;
const MAX_HYPERLINK_ID_BYTES: usize = 1024;

fn sanitize_window_title(title: &str) -> String {
    title
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_WINDOW_TITLE_CHARS)
        .collect()
}

fn hyperlink_from_alacritty(
    hyperlink: alacritty_terminal::term::cell::Hyperlink,
) -> Option<Hyperlink> {
    let uri = sanitize_hyperlink_text(hyperlink.uri(), MAX_HYPERLINK_URI_BYTES);
    if uri.is_empty() {
        return None;
    }

    Some(Hyperlink {
        id: sanitize_hyperlink_id(hyperlink.id()),
        uri,
    })
}

fn sanitize_hyperlink_id(id: &str) -> Option<String> {
    if is_alacritty_generated_hyperlink_id(id) {
        return None;
    }

    let id = sanitize_hyperlink_text(id, MAX_HYPERLINK_ID_BYTES);
    if id.is_empty() { None } else { Some(id) }
}

fn sanitize_hyperlink_text(value: &str, max_bytes: usize) -> String {
    let mut sanitized = String::new();
    for ch in value.chars().filter(|ch| !ch.is_control()) {
        if sanitized.len().saturating_add(ch.len_utf8()) > max_bytes {
            break;
        }
        sanitized.push(ch);
    }
    sanitized
}

fn is_alacritty_generated_hyperlink_id(id: &str) -> bool {
    let Some(prefix) = id.strip_suffix("_alacritty") else {
        return false;
    };
    !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectionRange {
    start: SelectionPoint,
    end: SelectionPoint,
}

impl SelectionRange {
    fn normalized(a: SelectionPoint, b: SelectionPoint) -> Self {
        if selection_point_le(a, b) {
            Self { start: a, end: b }
        } else {
            Self { start: b, end: a }
        }
    }
}

fn selection_point_le(a: SelectionPoint, b: SelectionPoint) -> bool {
    a.row < b.row || (a.row == b.row && a.col <= b.col)
}

fn line_for_row(row: isize) -> Option<Line> {
    let row = i32::try_from(row).ok()?;
    Some(Line(row))
}

fn usize_to_isize(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

fn is_word_constituent(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | ':' | '-')
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CompatAction {
    SetAltScreen,
    ResetAltScreen,
    SaveCursor,
    RestoreCursor,
    PrivateModes {
        modes: Vec<CompatPrivateMode>,
        enabled: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompatPrivateMode {
    X10Mouse,
    NormalMouse,
    ButtonMotionMouse,
    AnyMotionMouse,
    FocusEvents,
    SgrMouse,
    UnsupportedMouseEncoding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CsiScan {
    NotCsi,
    Incomplete,
    Complete {
        end: usize,
        action: Option<CompatAction>,
    },
}

fn scan_csi(input: &[u8], start: usize) -> CsiScan {
    let Some(&first) = input.get(start) else {
        return CsiScan::NotCsi;
    };

    let body_start = match first {
        ESC => match input.get(start + 1) {
            Some(b'[') => start + 2,
            Some(_) => return CsiScan::NotCsi,
            None => return CsiScan::Incomplete,
        },
        CSI => start + 1,
        _ => return CsiScan::NotCsi,
    };

    for index in body_start..input.len() {
        let byte = input[index];
        if (0x40..=0x7e).contains(&byte) {
            return CsiScan::Complete {
                end: index,
                action: compat_action(&input[body_start..index], byte),
            };
        }
    }

    if input.len().saturating_sub(start) > MAX_COMPAT_CSI_LEN {
        CsiScan::NotCsi
    } else {
        CsiScan::Incomplete
    }
}

fn compat_action(params: &[u8], final_byte: u8) -> Option<CompatAction> {
    if final_byte != b'h' && final_byte != b'l' {
        return None;
    }

    let modes = params.strip_prefix(b"?").and_then(parse_private_modes)?;
    if modes.len() == 1 {
        match (modes[0], final_byte) {
            (1047, b'h') => return Some(CompatAction::SetAltScreen),
            (1047, b'l') => return Some(CompatAction::ResetAltScreen),
            (1048, b'h') => return Some(CompatAction::SaveCursor),
            (1048, b'l') => return Some(CompatAction::RestoreCursor),
            _ => {}
        }
    }

    let private_modes = modes
        .iter()
        .copied()
        .map(compat_private_mode)
        .collect::<Option<Vec<_>>>()?;
    if private_modes.is_empty() {
        return None;
    }

    Some(CompatAction::PrivateModes {
        modes: private_modes,
        enabled: final_byte == b'h',
    })
}

fn parse_private_mode(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }

    core::str::from_utf8(bytes).ok()?.parse().ok()
}

fn usize_to_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn parse_private_modes(bytes: &[u8]) -> Option<Vec<u16>> {
    let modes = bytes
        .split(|byte| *byte == b';')
        .map(parse_private_mode)
        .collect::<Option<Vec<_>>>()?;
    if modes.is_empty() { None } else { Some(modes) }
}

fn compat_private_mode(mode: u16) -> Option<CompatPrivateMode> {
    match mode {
        9 => Some(CompatPrivateMode::X10Mouse),
        1000 => Some(CompatPrivateMode::NormalMouse),
        1002 => Some(CompatPrivateMode::ButtonMotionMouse),
        1003 => Some(CompatPrivateMode::AnyMotionMouse),
        1004 => Some(CompatPrivateMode::FocusEvents),
        1006 => Some(CompatPrivateMode::SgrMouse),
        1005 | 1015 | 1016 => Some(CompatPrivateMode::UnsupportedMouseEncoding),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MouseModeState {
    x10: bool,
    normal: bool,
    button_motion: bool,
    any_motion: bool,
    encoding: MouseEncoding,
    focus_events: bool,
}

impl Default for MouseModeState {
    fn default() -> Self {
        Self {
            x10: false,
            normal: false,
            button_motion: false,
            any_motion: false,
            encoding: MouseEncoding::Default,
            focus_events: false,
        }
    }
}

impl MouseModeState {
    fn modes(self) -> MouseModes {
        MouseModes {
            protocol: self.protocol(),
            encoding: self.encoding,
            focus_events: self.focus_events,
        }
    }

    fn protocol(self) -> MouseProtocol {
        if self.any_motion {
            MouseProtocol::AnyMotion
        } else if self.button_motion {
            MouseProtocol::ButtonMotion
        } else if self.normal {
            MouseProtocol::Normal
        } else if self.x10 {
            MouseProtocol::X10
        } else {
            MouseProtocol::Off
        }
    }
}

fn event_is_reported(protocol: MouseProtocol, event: TerminalMouseEvent) -> bool {
    match protocol {
        MouseProtocol::Off => false,
        MouseProtocol::X10 => matches!(event.kind, MouseEventKind::Press | MouseEventKind::Wheel),
        MouseProtocol::Normal => {
            matches!(
                event.kind,
                MouseEventKind::Press | MouseEventKind::Release | MouseEventKind::Wheel
            )
        }
        MouseProtocol::ButtonMotion => {
            matches!(
                event.kind,
                MouseEventKind::Press
                    | MouseEventKind::Release
                    | MouseEventKind::Drag
                    | MouseEventKind::Wheel
            )
        }
        MouseProtocol::AnyMotion => true,
    }
}

fn encode_sgr_mouse_event(event: TerminalMouseEvent) -> Option<Vec<u8>> {
    let button_code = mouse_button_code(event)?;
    let final_byte = if matches!(event.kind, MouseEventKind::Release) {
        b'm'
    } else {
        b'M'
    };
    let col = event.col.checked_add(1)?;
    let row = event.row.checked_add(1)?;
    Some(
        format!(
            "\x1b[<{};{};{}{}",
            button_code, col, row, final_byte as char
        )
        .into_bytes(),
    )
}

fn encode_default_mouse_event(event: TerminalMouseEvent) -> Option<Vec<u8>> {
    let button_code = mouse_button_code(event)?;
    let col = event.col.checked_add(1)?;
    let row = event.row.checked_add(1)?;
    // TODO(phase4): legacy default encoding cannot represent large coordinates safely.
    if button_code > 223 || col > 223 || row > 223 {
        return None;
    }

    Some(vec![
        ESC,
        b'[',
        b'M',
        u8::try_from(button_code + 32).ok()?,
        u8::try_from(col + 32).ok()?,
        u8::try_from(row + 32).ok()?,
    ])
}

fn mouse_button_code(event: TerminalMouseEvent) -> Option<usize> {
    let mut code = match event.kind {
        MouseEventKind::Release => button_base_code(event.button).unwrap_or(3),
        MouseEventKind::Move => 35,
        MouseEventKind::Drag => button_base_code(event.button)? + 32,
        MouseEventKind::Wheel => wheel_button_code(event.button)?,
        MouseEventKind::Press => button_base_code(event.button)?,
    };

    if event.shift {
        code += 4;
    }
    if event.alt {
        code += 8;
    }
    if event.ctrl {
        code += 16;
    }

    Some(code)
}

fn button_base_code(button: Option<MouseButton>) -> Option<usize> {
    match button? {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Other(code) => Some(code as usize),
        MouseButton::WheelUp | MouseButton::WheelDown => None,
    }
}

fn wheel_button_code(button: Option<MouseButton>) -> Option<usize> {
    match button? {
        MouseButton::WheelUp => Some(64),
        MouseButton::WheelDown => Some(65),
        _ => None,
    }
}

#[derive(Clone)]
struct TerminalEventSink {
    pty_writes: Arc<Mutex<Vec<String>>>,
    title_events: Arc<Mutex<Vec<Option<String>>>>,
}

impl EventListener for TerminalEventSink {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                if let Ok(mut pty_writes) = self.pty_writes.lock() {
                    pty_writes.push(text);
                }
            }
            Event::Title(title) => {
                if let Ok(mut title_events) = self.title_events.lock() {
                    title_events.push(Some(title));
                }
            }
            Event::ResetTitle => {
                if let Ok(mut title_events) = self.title_events.lock() {
                    title_events.push(None);
                }
            }
            _ => {}
        }
    }
}

/// Render-safe visible terminal grid snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub hyperlinks: Vec<Hyperlink>,
    pub selection_rects: Vec<SelectionRect>,
    pub image_placements: Vec<ImagePlacement>,
    pub cursor: Cursor,
}

impl GridSnapshot {
    pub fn cell(&self, x: usize, y: usize) -> &Cell {
        &self.cells[y * self.cols + x]
    }

    pub fn hyperlink_at_cell(&self, x: usize, y: usize) -> Option<&Hyperlink> {
        if x >= self.cols || y >= self.rows {
            return None;
        }

        let hyperlink_index = self.cell(x, y).hyperlink?;
        self.hyperlinks.get(hyperlink_index)
    }

    pub fn lines(&self) -> impl Iterator<Item = &[Cell]> {
        self.cells.chunks(self.cols)
    }
}

/// Hyperlink metadata associated with terminal cells.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Hyperlink {
    pub id: Option<String>,
    pub uri: String,
}

/// Visible cursor state in grid coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
    pub visible: bool,
}

/// Render-safe terminal cell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
    pub hyperlink: Option<usize>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            flags: CellFlags::default(),
            hyperlink: None,
        }
    }
}

impl Cell {
    fn from_alacritty(cell: &alacritty_terminal::term::cell::Cell) -> Self {
        Self {
            ch: cell.c,
            fg: Color::from_alacritty(cell.fg, true),
            bg: Color::from_alacritty(cell.bg, false),
            flags: CellFlags {
                bold: cell.flags.contains(AlacrittyFlags::BOLD),
                italic: cell.flags.contains(AlacrittyFlags::ITALIC),
                underline: cell.flags.intersects(AlacrittyFlags::ALL_UNDERLINES),
                inverse: cell.flags.contains(AlacrittyFlags::INVERSE),
                wide: cell.flags.contains(AlacrittyFlags::WIDE_CHAR),
                wide_spacer: cell.flags.intersects(
                    AlacrittyFlags::WIDE_CHAR_SPACER | AlacrittyFlags::LEADING_WIDE_CHAR_SPACER,
                ),
            },
            hyperlink: None,
        }
    }
}

/// Basic cell style flags.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub wide: bool,
    pub wide_spacer: bool,
}

/// Render-safe terminal color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    DefaultFg,
    DefaultBg,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    fn from_alacritty(color: AlacrittyColor, foreground: bool) -> Self {
        match color {
            AlacrittyColor::Spec(AlacrittyRgb { r, g, b }) => Self::Rgb(r, g, b),
            AlacrittyColor::Indexed(index) => Self::Indexed(index),
            AlacrittyColor::Named(NamedColor::Foreground) if foreground => Self::DefaultFg,
            AlacrittyColor::Named(NamedColor::Background) if !foreground => Self::DefaultBg,
            AlacrittyColor::Named(named) => match named as usize {
                0..=255 => Self::Indexed(named as u8),
                _ if foreground => Self::DefaultFg,
                _ => Self::DefaultBg,
            },
        }
    }
}

/// Coarse render damage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Damage {
    None,
    Full,
    Lines(Vec<LineDamage>),
    Rects(Vec<Rect>),
}

/// Damaged range on one visible line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineDamage {
    pub line: usize,
    pub left: usize,
    pub right: usize,
}

/// Rectangular damaged region.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

fn merge_damage(current: Damage, next: Damage) -> Damage {
    match (current, next) {
        (Damage::Full, _) | (_, Damage::Full) => Damage::Full,
        (Damage::None, damage) | (damage, Damage::None) => damage,
        (Damage::Lines(mut current), Damage::Lines(next)) => {
            for line in next {
                if let Some(existing) = current.iter_mut().find(|item| item.line == line.line) {
                    existing.left = existing.left.min(line.left);
                    existing.right = existing.right.max(line.right);
                } else {
                    current.push(line);
                }
            }

            Damage::Lines(current)
        }
        (Damage::Rects(mut current), Damage::Rects(next)) => {
            current.extend(next);
            Damage::Rects(current)
        }
        (Damage::Lines(lines), Damage::Rects(rects))
        | (Damage::Rects(rects), Damage::Lines(lines)) => {
            let mut merged_rects = rects;
            merged_rects.extend(lines.into_iter().map(|line| Rect {
                x: line.left,
                y: line.line,
                width: line.right.saturating_sub(line.left) + 1,
                height: 1,
            }));
            Damage::Rects(merged_rects)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Color, Damage, GridPoint, ImageId, ImagePlacement, ImagePlacementId,
        MAX_HYPERLINK_ID_BYTES, MAX_HYPERLINK_URI_BYTES, MAX_WINDOW_TITLE_CHARS, MouseButton,
        MouseEncoding, MouseEventKind, MouseModes, MouseProtocol, SelectionMode, SelectionPoint,
        SelectionRect, Terminal, TerminalMouseEvent,
    };

    #[test]
    fn plain_ascii_text_is_written_to_grid() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"hello");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'h');
        assert_eq!(grid.cell(1, 0).ch, 'e');
        assert_eq!(grid.cell(2, 0).ch, 'l');
        assert_eq!(grid.cell(3, 0).ch, 'l');
        assert_eq!(grid.cell(4, 0).ch, 'o');
    }

    #[test]
    fn newline_moves_cursor_to_next_row() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"hello\nworld");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'h');
        assert_eq!(grid.cell(5, 1).ch, 'w');
    }

    #[test]
    fn carriage_return_moves_cursor_to_column_zero() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"hello\rY");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'Y');
        assert_eq!(grid.cell(1, 0).ch, 'e');
    }

    #[test]
    fn csi_clear_screen_erases_visible_grid() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"hello\x1b[2J");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, ' ');
        assert_eq!(grid.cell(1, 0).ch, ' ');
        assert_eq!(grid.cell(2, 0).ch, ' ');
    }

    #[test]
    fn csi_truecolor_fg_sets_cell_color() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b[38;2;255;0;0mX");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'X');
        assert_eq!(grid.cell(0, 0).fg, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn csi_truecolor_bg_sets_cell_color() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b[48;2;0;128;255mX");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'X');
        assert_eq!(grid.cell(0, 0).bg, Color::Rgb(0, 128, 255));
    }

    #[test]
    fn csi_truecolor_fg_and_bg_are_preserved_together() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b[38;2;10;20;30;48;2;40;50;60mX");

        let cell = term.snapshot().cell(0, 0).clone();
        assert_eq!(cell.fg, Color::Rgb(10, 20, 30));
        assert_eq!(cell.bg, Color::Rgb(40, 50, 60));
    }

    #[test]
    fn csi_sequence_split_across_two_feeds_still_parses() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b[38;2;255;");
        term.feed(b"0;0mX");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'X');
        assert_eq!(grid.cell(0, 0).fg, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn utf8_split_across_two_feeds_still_decodes() {
        let mut term = Terminal::new(80, 24);
        let bytes = "é".as_bytes();

        term.feed(&bytes[..1]);
        term.feed(&bytes[1..]);

        assert_eq!(term.snapshot().cell(0, 0).ch, 'é');
    }

    #[test]
    fn osc_0_bel_sets_window_title() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b]0;hello\x07");

        assert_eq!(term.window_title(), "hello");
        assert_eq!(term.take_window_title_changed(), Some("hello".to_owned()));
    }

    #[test]
    fn osc_2_bel_sets_window_title() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b]2;hello\x07");

        assert_eq!(term.window_title(), "hello");
        assert_eq!(term.take_window_title_changed(), Some("hello".to_owned()));
    }

    #[test]
    fn osc_0_st_sets_window_title() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b]0;hello\x1b\\");

        assert_eq!(term.window_title(), "hello");
        assert_eq!(term.take_window_title_changed(), Some("hello".to_owned()));
    }

    #[test]
    fn osc_title_split_across_feeds_still_updates_title() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b]0;hel");
        term.feed(b"lo\x07");

        assert_eq!(term.window_title(), "hello");
        assert_eq!(term.take_window_title_changed(), Some("hello".to_owned()));
    }

    #[test]
    fn unsupported_osc_does_not_change_window_title() {
        let mut term = Terminal::new(80, 24);
        term.feed(b"\x1b]0;hello\x07");
        assert_eq!(term.take_window_title_changed(), Some("hello".to_owned()));

        term.feed(b"\x1b]1;icon\x07");

        assert_eq!(term.window_title(), "hello");
        assert_eq!(term.take_window_title_changed(), None);
    }

    #[test]
    fn empty_osc_title_is_allowed() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b]0;\x07");

        assert_eq!(term.window_title(), "");
        assert_eq!(term.take_window_title_changed(), Some(String::new()));
    }

    #[test]
    fn osc_title_update_preserves_grid_contents() {
        let mut term = Terminal::new(80, 24);
        term.feed(b"hi");

        term.feed(b"\x1b]0;hello\x07");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'h');
        assert_eq!(grid.cell(1, 0).ch, 'i');
        assert_eq!(term.window_title(), "hello");
    }

    #[test]
    fn osc_title_control_chars_are_removed() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b]0;clean\x01\t title\x07");

        assert_eq!(term.window_title(), "clean title");
        assert_eq!(
            term.take_window_title_changed(),
            Some("clean title".to_owned())
        );
    }

    #[test]
    fn oversized_osc_title_is_safely_limited() {
        let mut term = Terminal::new(80, 24);
        let mut bytes = b"\x1b]0;".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', MAX_WINDOW_TITLE_CHARS * 2));
        bytes.push(b'\x07');

        term.feed(&bytes);

        assert!(term.window_title().chars().count() <= MAX_WINDOW_TITLE_CHARS);
        assert_eq!(term.snapshot().cell(0, 0).ch, ' ');
    }

    #[test]
    fn osc8_bel_attaches_hyperlink_to_visible_cells() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]8;id=foo;https://example.com\x07hi");

        let grid = term.snapshot();
        assert_eq!(line_text(&grid, 0), "hi        ");
        assert_eq!(grid.hyperlinks.len(), 1);
        assert_eq!(
            grid.hyperlink_at_cell(0, 0),
            Some(&super::Hyperlink {
                id: Some("foo".to_owned()),
                uri: "https://example.com".to_owned(),
            })
        );
        assert_eq!(grid.cell(0, 0).hyperlink, grid.cell(1, 0).hyperlink);
    }

    #[test]
    fn osc8_st_attaches_hyperlink_without_explicit_id() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]8;;https://example.com\x1b\\X");

        let hyperlink = term.hyperlink_at_cell(0, 0).unwrap();
        assert_eq!(hyperlink.id, None);
        assert_eq!(hyperlink.uri, "https://example.com");
    }

    #[test]
    fn osc8_close_removes_hyperlink_from_following_cells() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]8;;https://example.com\x07A\x1b]8;;\x07B");

        let grid = term.snapshot();
        assert!(grid.hyperlink_at_cell(0, 0).is_some());
        assert_eq!(grid.hyperlink_at_cell(1, 0), None);
    }

    #[test]
    fn osc8_split_across_feeds_still_sets_hyperlink() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]8;id=foo;https://exa");
        term.feed(b"mple.com\x07X");

        let hyperlink = term.hyperlink_at_cell(0, 0).unwrap();
        assert_eq!(hyperlink.id, Some("foo".to_owned()));
        assert_eq!(hyperlink.uri, "https://example.com");
    }

    #[test]
    fn osc8_st_split_across_feeds_still_sets_hyperlink() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]8;;https://example.com\x1b");
        term.feed(b"\\X");

        assert_eq!(
            term.hyperlink_at_cell(0, 0).map(|hyperlink| hyperlink.uri),
            Some("https://example.com".to_owned())
        );
    }

    #[test]
    fn osc8_unsupported_params_preserve_uri() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]8;foo=bar;https://example.com\x07X");

        let hyperlink = term.hyperlink_at_cell(0, 0).unwrap();
        assert_eq!(hyperlink.id, None);
        assert_eq!(hyperlink.uri, "https://example.com");
    }

    #[test]
    fn osc8_empty_uri_closes_current_hyperlink() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]8;;https://example.com\x07A\x1b]8;;\x1b\\B");

        assert!(term.hyperlink_at_cell(0, 0).is_some());
        assert_eq!(term.hyperlink_at_cell(1, 0), None);
    }

    #[test]
    fn osc8_sequence_itself_is_not_rendered_to_grid() {
        let mut term = Terminal::new(5, 1);

        term.feed(b"\x1b]8;;https://example.com\x07X\x1b]8;;\x07");

        assert_eq!(line_text(&term.snapshot(), 0), "X    ");
    }

    #[test]
    fn osc8_hyperlink_metadata_survives_primary_scrollback() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        term.feed(b"\x1b]8;id=hist;https://example.com\x07A\x1b]8;;\x07\r\n");
        term.feed(b"B\r\nC\r\n");

        term.scroll_to_top();
        let grid = term.snapshot();
        let (column, row) = find_cell(&grid, 'A').expect("history cell should be visible");

        assert_eq!(
            grid.hyperlink_at_cell(column, row),
            Some(&super::Hyperlink {
                id: Some("hist".to_owned()),
                uri: "https://example.com".to_owned(),
            })
        );
        assert_eq!(
            term.hyperlink_at_cell(column, row)
                .map(|hyperlink| hyperlink.uri),
            Some("https://example.com".to_owned())
        );
    }

    #[test]
    fn alternate_screen_hyperlinks_do_not_enter_primary_scrollback() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        term.feed(b"A\r\nB\r\n");
        term.feed(b"\x1b[?1049h");
        term.feed(b"\x1b]8;;https://example.com\x07X\x1b]8;;\x07\r\nY\r\nZ\r\n");
        term.feed(b"\x1b[?1049l");

        term.scroll_to_top();
        let grid = term.snapshot();

        assert!(!snapshot_text(&grid).contains('X'));
        assert!(grid.hyperlinks.is_empty());
    }

    #[test]
    fn selected_text_does_not_include_hyperlink_uri() {
        let mut term = Terminal::new(10, 1);
        term.feed(b"\x1b]8;;https://example.com\x07hello\x1b]8;;\x07");

        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(4, 0));

        assert_eq!(term.selected_text(), Some("hello".to_owned()));
    }

    #[test]
    fn scrollback_hyperlink_selection_copies_display_text_only() {
        let mut term = Terminal::with_scrollback(8, 2, 10);
        term.feed(b"\x1b]8;id=hist;https://example.com\x07LINK\x1b]8;;\x07\r\n");
        term.feed(b"NEXT\r\nTAIL\r\n");
        term.scroll_to_top();
        let grid = term.snapshot();
        let (column, row) = find_cell(&grid, 'L').expect("history link should be visible");

        assert_eq!(
            grid.hyperlink_at_cell(column, row),
            Some(&super::Hyperlink {
                id: Some("hist".to_owned()),
                uri: "https://example.com".to_owned(),
            })
        );

        let start = term.selection_point_for_visible_cell(column, row).unwrap();
        let end = term
            .selection_point_for_visible_cell(column + 3, row)
            .unwrap();
        term.begin_selection(start, SelectionMode::Simple);
        term.update_selection(end);

        assert_eq!(term.selected_text(), Some("LINK".to_owned()));
    }

    #[test]
    fn has_selection_tracks_selection_lifecycle() {
        let mut term = Terminal::new(10, 1);
        term.feed(b"hello");

        assert!(!term.has_selection());
        term.begin_selection(point(0, 0), SelectionMode::Simple);
        assert!(term.has_selection());
        term.clear_selection();
        assert!(!term.has_selection());
    }

    #[test]
    fn osc_title_and_osc8_hyperlink_metadata_coexist() {
        let mut term = Terminal::new(10, 1);

        term.feed(b"\x1b]0;title test\x07");
        term.feed(b"\x1b]8;id=abc;https://example.com\x07hello\x1b]8;;\x07");

        assert_eq!(term.window_title(), "title test");
        assert_eq!(
            term.take_window_title_changed(),
            Some("title test".to_owned())
        );
        assert_eq!(
            term.hyperlink_at_cell(0, 0),
            Some(super::Hyperlink {
                id: Some("abc".to_owned()),
                uri: "https://example.com".to_owned(),
            })
        );
    }

    #[test]
    fn hyperlink_metadata_is_sanitized_and_limited() {
        let hyperlink =
            super::hyperlink_from_alacritty(alacritty_terminal::term::cell::Hyperlink::new(
                Some(format!("id\x01{}", "a".repeat(MAX_HYPERLINK_ID_BYTES * 2))),
                format!(
                    "https://example.com/\x02{}",
                    "b".repeat(MAX_HYPERLINK_URI_BYTES * 2)
                ),
            ))
            .expect("long hyperlink should be truncated rather than dropped");

        let id = hyperlink.id.expect("non-empty id should remain present");
        assert!(id.len() <= MAX_HYPERLINK_ID_BYTES);
        assert!(hyperlink.uri.len() <= MAX_HYPERLINK_URI_BYTES);
        assert!(!id.chars().any(char::is_control));
        assert!(!hyperlink.uri.chars().any(char::is_control));
    }

    #[test]
    fn osc8_invalid_utf8_does_not_panic_or_render_escape_bytes() {
        let mut term = Terminal::new(5, 1);

        term.feed(b"\x1b]8;;https://exa\xffmple.com\x07X");

        let grid = term.snapshot();
        assert_eq!(line_text(&grid, 0), "X    ");
        assert_eq!(grid.hyperlink_at_cell(0, 0), None);
    }

    #[test]
    fn hyperlink_at_cell_returns_none_for_unlinked_or_out_of_range_cells() {
        let mut term = Terminal::new(5, 1);
        term.feed(b"\x1b]8;;https://example.com\x07A\x1b]8;;\x07B");
        let grid = term.snapshot();

        assert!(grid.hyperlink_at_cell(0, 0).is_some());
        assert_eq!(grid.hyperlink_at_cell(1, 0), None);
        assert_eq!(grid.hyperlink_at_cell(5, 0), None);
        assert_eq!(term.hyperlink_at_cell(5, 0), None);
    }

    #[test]
    fn take_window_title_changed_is_one_shot() {
        let mut term = Terminal::new(80, 24);
        term.feed(b"\x1b]2;hello\x07");

        assert_eq!(term.take_window_title_changed(), Some("hello".to_owned()));
        assert_eq!(term.take_window_title_changed(), None);
    }

    #[test]
    fn resize_changes_visible_grid_size() {
        let mut term = Terminal::new(80, 24);

        term.resize(100, 30);

        let grid = term.snapshot();
        assert_eq!(grid.cols, 100);
        assert_eq!(grid.rows, 30);
        assert_eq!(grid.cells.len(), 3000);
        assert_eq!(grid.cursor.x, 0);
        assert_eq!(grid.cursor.y, 0);
        assert!(grid.cursor.visible);
    }

    #[test]
    fn feed_records_line_damage_until_taken() {
        let mut term = Terminal::new(80, 24);

        assert_eq!(term.take_damage(), Damage::Full);
        assert_eq!(term.take_damage(), Damage::None);
        term.feed(b"hello");
        match term.take_damage() {
            Damage::Lines(lines) => {
                assert!(
                    lines
                        .iter()
                        .any(|line| line.line == 0 && line.left == 0 && line.right >= 5)
                );
            }
            damage => panic!("expected line damage, got {damage:?}"),
        }
        assert_eq!(term.take_damage(), Damage::None);
    }

    #[test]
    fn device_status_report_writes_cursor_position_reply_to_pty() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b[6n");

        assert_eq!(term.take_pty_writes(), vec!["\x1b[1;1R".to_owned()]);
        assert_eq!(term.take_pty_writes(), Vec::<String>::new());
    }

    #[test]
    fn sgr_background_and_style_flags_are_exposed_in_snapshot() {
        let mut term = Terminal::new(80, 24);

        term.feed(b"\x1b[1;3;4;7;48;2;1;2;3mX");

        let cell = term.snapshot().cell(0, 0).clone();
        assert_eq!(cell.ch, 'X');
        assert_eq!(cell.bg, Color::Rgb(1, 2, 3));
        assert!(cell.flags.bold);
        assert!(cell.flags.italic);
        assert!(cell.flags.underline);
        assert!(cell.flags.inverse);
    }

    #[test]
    fn wide_character_marks_leading_cell_and_spacer() {
        let mut term = Terminal::new(80, 24);

        term.feed("界".as_bytes());

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, '界');
        assert!(grid.cell(0, 0).flags.wide);
        assert!(grid.cell(1, 0).flags.wide_spacer);
    }

    #[test]
    fn dec_private_mode_can_hide_cursor_in_snapshot() {
        let mut term = Terminal::new(80, 24);

        assert!(term.snapshot().cursor.visible);
        term.feed(b"\x1b[?25l");

        assert!(!term.snapshot().cursor.visible);
    }

    #[test]
    fn dec_1049_alt_screen_preserves_and_restores_primary_screen() {
        let mut term = Terminal::new(10, 3);

        term.feed(b"primary");
        term.feed(b"\x1b[?1049h");
        term.feed(b"\x1b[H");
        term.feed(b"alt");

        assert_eq!(term.snapshot().cell(0, 0).ch, 'a');

        term.feed(b"\x1b[?1049l");
        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'p');
        assert_eq!(grid.cell(1, 0).ch, 'r');
        assert_eq!(grid.cell(2, 0).ch, 'i');
    }

    #[test]
    fn dec_1047_alt_screen_preserves_and_restores_primary_screen() {
        let mut term = Terminal::new(10, 3);

        term.feed(b"primary");
        term.feed(b"\x1b[?1047h");
        term.feed(b"\x1b[H");
        term.feed(b"alt");

        assert_eq!(term.snapshot().cell(0, 0).ch, 'a');

        term.feed(b"\x1b[?1047l");
        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'p');
        assert_eq!(grid.cell(1, 0).ch, 'r');
        assert_eq!(grid.cell(2, 0).ch, 'i');
    }

    #[test]
    fn dec_1048_saves_and_restores_cursor_position() {
        let mut term = Terminal::new(10, 5);

        term.feed(b"\x1b[2;3H");
        term.feed(b"\x1b[?1048h");
        term.feed(b"\x1b[4;5H");
        assert_eq!(term.snapshot().cursor.x, 4);
        assert_eq!(term.snapshot().cursor.y, 3);

        term.feed(b"\x1b[?1048l");
        let cursor = term.snapshot().cursor;
        assert_eq!(cursor.x, 2);
        assert_eq!(cursor.y, 1);
    }

    #[test]
    fn dec_1048_split_across_feeds_still_restores_cursor_position() {
        let mut term = Terminal::new(10, 5);

        term.feed(b"\x1b[2;3H");
        term.feed(b"\x1b[?1048h");
        term.feed(b"\x1b[4;5H");
        term.feed(b"\x1b[?104");
        term.feed(b"8l");

        let cursor = term.snapshot().cursor;
        assert_eq!(cursor.x, 2);
        assert_eq!(cursor.y, 1);
    }

    #[test]
    fn decstbm_scrolls_only_inside_scroll_region() {
        let mut term = Terminal::new(5, 4);

        term.feed(b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC\x1b[4;1HD");
        term.feed(b"\x1b[2;3r\x1b[3;1H\n");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'A');
        assert_eq!(grid.cell(0, 1).ch, 'C');
        assert_eq!(grid.cell(0, 2).ch, ' ');
        assert_eq!(grid.cell(0, 3).ch, 'D');
    }

    #[test]
    fn origin_mode_addresses_cursor_relative_to_scroll_region() {
        let mut term = Terminal::new(5, 5);

        term.feed(b"\x1b[2;4r\x1b[?6h\x1b[1;1HX");
        assert_eq!(term.snapshot().cell(0, 1).ch, 'X');

        term.feed(b"\x1b[?6l\x1b[1;1HY");
        assert_eq!(term.snapshot().cell(0, 0).ch, 'Y');
    }

    #[test]
    fn delete_line_is_limited_to_scroll_region() {
        let mut term = Terminal::new(5, 5);

        term.feed(b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC\x1b[4;1HD\x1b[5;1HE");
        term.feed(b"\x1b[2;4r\x1b[2;1H\x1b[1M");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'A');
        assert_eq!(grid.cell(0, 1).ch, 'C');
        assert_eq!(grid.cell(0, 2).ch, 'D');
        assert_eq!(grid.cell(0, 3).ch, ' ');
        assert_eq!(grid.cell(0, 4).ch, 'E');
    }

    #[test]
    fn insert_line_is_limited_to_scroll_region() {
        let mut term = Terminal::new(5, 5);

        term.feed(b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC\x1b[4;1HD\x1b[5;1HE");
        term.feed(b"\x1b[2;4r\x1b[3;1H\x1b[1L");

        let grid = term.snapshot();
        assert_eq!(grid.cell(0, 0).ch, 'A');
        assert_eq!(grid.cell(0, 1).ch, 'B');
        assert_eq!(grid.cell(0, 2).ch, ' ');
        assert_eq!(grid.cell(0, 3).ch, 'C');
        assert_eq!(grid.cell(0, 4).ch, 'E');
    }

    #[test]
    fn insert_and_delete_character_affect_only_current_line() {
        let mut term = Terminal::new(5, 2);

        term.feed(b"abcde\x1b[2;1Hvwxyz\x1b[1;3H\x1b[2@");
        let grid = term.snapshot();
        assert_eq!(line_text(&grid, 0), "ab  c");
        assert_eq!(line_text(&grid, 1), "vwxyz");

        term.feed(b"\x1b[2;2H\x1b[2P");
        let grid = term.snapshot();
        assert_eq!(line_text(&grid, 1), "vyz  ");
    }

    #[test]
    fn insert_character_preserves_wide_cell_pairing() {
        let mut term = Terminal::new(6, 1);

        term.feed("A界BC".as_bytes());
        term.feed(b"\x1b[1;2H\x1b[1@");

        let grid = term.snapshot();
        let wide_column = (0..grid.cols)
            .find(|column| grid.cell(*column, 0).flags.wide)
            .expect("wide cell should remain visible");
        assert!(grid.cell(wide_column + 1, 0).flags.wide_spacer);
    }

    #[test]
    fn bracketed_paste_wraps_payload_only_when_mode_is_enabled() {
        let mut term = Terminal::new(10, 3);

        assert!(!term.bracketed_paste_enabled());
        assert_eq!(term.paste_bytes(b"hello"), b"hello");

        term.feed(b"\x1b[?2004h");
        assert!(term.bracketed_paste_enabled());
        assert_eq!(term.paste_bytes(b"hello"), b"\x1b[200~hello\x1b[201~");

        term.feed(b"\x1b[?2004l");
        assert!(!term.bracketed_paste_enabled());
        assert_eq!(term.paste_bytes(b"hello"), b"hello");
    }

    #[test]
    fn sgr_mouse_mode_can_be_enabled_and_disabled() {
        let mut term = Terminal::new(10, 3);

        assert_eq!(term.mouse_modes().encoding, MouseEncoding::Default);
        term.feed(b"\x1b[?1006h");
        assert_eq!(term.mouse_modes().encoding, MouseEncoding::Sgr);

        term.feed(b"\x1b[?1006l");
        assert_eq!(term.mouse_modes().encoding, MouseEncoding::Default);
    }

    #[test]
    fn normal_mouse_reporting_can_be_enabled() {
        let mut term = Terminal::new(10, 3);

        term.feed(b"\x1b[?1000h");

        assert_eq!(
            term.mouse_modes(),
            MouseModes {
                protocol: MouseProtocol::Normal,
                encoding: MouseEncoding::Default,
                focus_events: false,
            }
        );
    }

    #[test]
    fn button_motion_mouse_reporting_has_priority_over_normal() {
        let mut term = Terminal::new(10, 3);

        term.feed(b"\x1b[?1000h");
        term.feed(b"\x1b[?1002h");

        assert_eq!(term.mouse_modes().protocol, MouseProtocol::ButtonMotion);

        term.feed(b"\x1b[?1002l");
        assert_eq!(term.mouse_modes().protocol, MouseProtocol::Normal);
    }

    #[test]
    fn any_motion_mouse_reporting_has_priority_over_button_motion() {
        let mut term = Terminal::new(10, 3);

        term.feed(b"\x1b[?1000;1002h");
        term.feed(b"\x1b[?1003h");

        assert_eq!(term.mouse_modes().protocol, MouseProtocol::AnyMotion);

        term.feed(b"\x1b[?1003l");
        assert_eq!(term.mouse_modes().protocol, MouseProtocol::ButtonMotion);
    }

    #[test]
    fn focus_event_mode_can_be_enabled_and_disabled() {
        let mut term = Terminal::new(10, 3);

        assert!(!term.focus_events_enabled());
        term.feed(b"\x1b[?1004h");
        assert!(term.focus_events_enabled());

        term.feed(b"\x1b[?1004l");
        assert!(!term.focus_events_enabled());
    }

    #[test]
    fn sgr_left_press_uses_one_based_coordinates() {
        let mut term = Terminal::new(10, 3);
        term.feed(b"\x1b[?1000;1006h");

        assert_eq!(
            term.encode_mouse_event(mouse_event(MouseEventKind::Press, Some(MouseButton::Left))),
            Some(b"\x1b[<0;3;2M".to_vec())
        );
    }

    #[test]
    fn sgr_left_release_uses_lowercase_final_byte() {
        let mut term = Terminal::new(10, 3);
        term.feed(b"\x1b[?1000;1006h");

        assert_eq!(
            term.encode_mouse_event(mouse_event(
                MouseEventKind::Release,
                Some(MouseButton::Left)
            )),
            Some(b"\x1b[<0;3;2m".to_vec())
        );
    }

    #[test]
    fn sgr_wheel_up_and_down_use_wheel_button_codes() {
        let mut term = Terminal::new(10, 3);
        term.feed(b"\x1b[?1000;1006h");

        assert_eq!(
            term.encode_mouse_event(mouse_event(
                MouseEventKind::Wheel,
                Some(MouseButton::WheelUp)
            )),
            Some(b"\x1b[<64;3;2M".to_vec())
        );
        assert_eq!(
            term.encode_mouse_event(mouse_event(
                MouseEventKind::Wheel,
                Some(MouseButton::WheelDown)
            )),
            Some(b"\x1b[<65;3;2M".to_vec())
        );
    }

    #[test]
    fn sgr_modifiers_are_added_to_button_code() {
        let mut term = Terminal::new(10, 3);
        term.feed(b"\x1b[?1000;1006h");

        assert_eq!(
            term.encode_mouse_event(TerminalMouseEvent {
                shift: true,
                alt: true,
                ctrl: true,
                ..mouse_event(MouseEventKind::Press, Some(MouseButton::Left))
            }),
            Some(b"\x1b[<28;3;2M".to_vec())
        );
    }

    #[test]
    fn focus_events_encode_only_when_enabled() {
        let mut term = Terminal::new(10, 3);

        assert_eq!(term.encode_focus_event(true), None);
        assert_eq!(term.encode_focus_event(false), None);

        term.feed(b"\x1b[?1004h");
        assert_eq!(term.encode_focus_event(true), Some(b"\x1b[I".to_vec()));
        assert_eq!(term.encode_focus_event(false), Some(b"\x1b[O".to_vec()));
    }

    #[test]
    fn mouse_event_encoding_returns_none_when_reporting_is_off() {
        let term = Terminal::new(10, 3);

        assert_eq!(
            term.encode_mouse_event(mouse_event(MouseEventKind::Press, Some(MouseButton::Left))),
            None
        );
    }

    #[test]
    fn motion_reporting_respects_enabled_protocol() {
        let mut term = Terminal::new(10, 3);
        term.feed(b"\x1b[?1000;1006h");
        assert_eq!(
            term.encode_mouse_event(mouse_event(MouseEventKind::Move, None)),
            None
        );

        term.feed(b"\x1b[?1003h");
        assert_eq!(
            term.encode_mouse_event(mouse_event(MouseEventKind::Move, None)),
            Some(b"\x1b[<35;3;2M".to_vec())
        );
    }

    #[test]
    fn resize_clamps_cursor_to_visible_grid_and_reports_full_damage() {
        let mut term = Terminal::new(5, 5);

        term.take_damage();
        term.feed(b"\x1b[5;5H");
        term.take_damage();
        term.resize(2, 2);

        let grid = term.snapshot();
        assert!(grid.cursor.x < grid.cols);
        assert!(grid.cursor.y < grid.rows);
        assert_eq!(term.take_damage(), Damage::Full);
    }

    #[test]
    fn zero_scrollback_limit_disables_history() {
        let mut term = Terminal::with_scrollback(5, 2, 0);

        feed_lines(&mut term, &["A", "B", "C", "D"]);

        assert_eq!(term.scrollback_len(), 0);
        assert_eq!(term.scroll_offset(), 0);
    }

    #[test]
    fn output_beyond_visible_height_increases_scrollback_len() {
        let mut term = Terminal::with_scrollback(5, 2, 10);

        feed_lines(&mut term, &["A", "B", "C", "D"]);

        assert!(term.scrollback_len() > 0);
    }

    #[test]
    fn scrollback_limit_discards_oldest_lines() {
        let mut term = Terminal::with_scrollback(5, 2, 2);

        feed_lines(&mut term, &["A", "B", "C", "D", "E"]);

        assert_eq!(term.scrollback_len(), 2);
        term.scroll_to_top();
        let grid = term.snapshot();
        assert_ne!(line_text(&grid, 0).trim_end(), "A");
        assert_ne!(line_text(&grid, 0).trim_end(), "B");
    }

    #[test]
    fn scroll_api_clamps_offsets_and_reports_damage() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        feed_lines(&mut term, &["A", "B", "C", "D"]);

        assert_eq!(term.scroll_offset(), 0);
        assert_eq!(term.scroll_up_lines(1), Damage::Full);
        assert_eq!(term.scroll_offset(), 1);
        assert_eq!(term.scroll_down_lines(1), Damage::Full);
        assert_eq!(term.scroll_offset(), 0);

        assert_eq!(term.scroll_to_top(), Damage::Full);
        assert_eq!(term.scroll_offset(), term.scrollback_len());
        assert_eq!(term.scroll_up_lines(usize::MAX), Damage::None);
        assert_eq!(term.scroll_to_bottom(), Damage::Full);
        assert_eq!(term.scroll_offset(), 0);
    }

    #[test]
    fn scrolled_snapshot_contains_history_and_hides_cursor() {
        let mut term = Terminal::with_scrollback(5, 3, 10);
        feed_lines(&mut term, &["A", "B", "C", "D"]);

        term.scroll_up_lines(2);
        let grid = term.snapshot();
        let text = snapshot_text(&grid);

        assert!(text.contains('A') || text.contains('B'));
        assert!(!grid.cursor.visible);
    }

    #[test]
    fn feed_returns_scrolled_view_to_bottom() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        feed_lines(&mut term, &["A", "B", "C", "D"]);
        term.scroll_to_top();

        term.feed(b"Z");

        assert_eq!(term.scroll_offset(), 0);
        assert!(term.snapshot().cursor.visible);
    }

    #[test]
    fn alternate_screen_output_does_not_enter_primary_scrollback() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        feed_lines(&mut term, &["A", "B", "C"]);
        let primary_history_len = term.scrollback_len();

        term.feed(b"\x1b[?1049h");
        feed_lines(&mut term, &["X", "Y", "Z"]);
        assert_eq!(term.scrollback_len(), 0);

        term.feed(b"\x1b[?1049l");
        assert_eq!(term.scrollback_len(), primary_history_len);
        term.scroll_to_top();
        assert!(snapshot_text(&term.snapshot()).contains('A'));
    }

    #[test]
    fn resize_clamps_scroll_offset_to_available_history() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        feed_lines(&mut term, &["A", "B", "C", "D", "E"]);
        term.scroll_to_top();

        term.resize(5, 5);

        assert!(term.scroll_offset() <= term.scrollback_len());
    }

    #[test]
    fn scrolled_snapshot_preserves_wide_cell_pairing() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        term.feed("界\r\nA\r\nB".as_bytes());

        term.scroll_to_top();
        let grid = term.snapshot();
        let wide_column = (0..grid.rows)
            .flat_map(|row| (0..grid.cols).map(move |column| (column, row)))
            .find(|(column, row)| grid.cell(*column, *row).flags.wide)
            .expect("wide cell should be present in scrolled snapshot");

        assert!(
            grid.cell(wide_column.0 + 1, wide_column.1)
                .flags
                .wide_spacer
        );
    }

    #[test]
    fn single_line_selection_returns_selected_text() {
        let mut term = Terminal::new(20, 2);
        term.feed(b"hello world");

        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(4, 0));

        assert_eq!(term.selected_text(), Some("hello".to_owned()));
    }

    #[test]
    fn multi_line_selection_joins_lines_with_lf() {
        let mut term = Terminal::new(20, 3);
        term.feed(b"hello world\r\nabc def");

        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(2, 1));

        assert_eq!(term.selected_text(), Some("hello world\nabc".to_owned()));
    }

    #[test]
    fn reversed_selection_returns_same_text() {
        let mut term = Terminal::new(20, 3);
        term.feed(b"hello world\r\nabc def");

        term.begin_selection(point(2, 1), SelectionMode::Simple);
        term.update_selection(point(0, 0));

        assert_eq!(term.selected_text(), Some("hello world\nabc".to_owned()));
    }

    #[test]
    fn reversed_selection_normalizes_same_line_and_multi_line_ranges() {
        let mut same_line = Terminal::new(20, 2);
        same_line.feed(b"hello world");
        same_line.begin_selection(point(4, 0), SelectionMode::Simple);
        same_line.update_selection(point(1, 0));

        assert_eq!(same_line.selected_text(), Some("ello".to_owned()));
        assert_eq!(
            same_line.selection_rects(),
            vec![SelectionRect {
                col: 1,
                row: 0,
                width: 4,
                height: 1,
            }]
        );

        let mut multi_line = Terminal::new(20, 3);
        multi_line.feed(b"hello world\r\nabc def");
        multi_line.begin_selection(point(2, 1), SelectionMode::Simple);
        multi_line.update_selection(point(0, 0));

        assert_eq!(
            multi_line.selected_text(),
            Some("hello world\nabc".to_owned())
        );
        assert_eq!(
            multi_line.selection_rects(),
            vec![
                SelectionRect {
                    col: 0,
                    row: 0,
                    width: 20,
                    height: 1,
                },
                SelectionRect {
                    col: 0,
                    row: 1,
                    width: 3,
                    height: 1,
                },
            ]
        );
    }

    #[test]
    fn selection_contains_visible_cell_uses_normalized_range() {
        let mut term = Terminal::new(20, 3);
        term.feed(b"hello world\r\nabc def");
        term.begin_selection(point(4, 0), SelectionMode::Simple);
        term.update_selection(point(1, 0));

        assert!(!term.selection_contains_visible_cell(0, 0));
        assert!(term.selection_contains_visible_cell(1, 0));
        assert!(term.selection_contains_visible_cell(4, 0));
        assert!(!term.selection_contains_visible_cell(5, 0));

        term.clear_selection();
        term.begin_selection(point(2, 1), SelectionMode::Simple);
        term.update_selection(point(0, 0));

        assert!(term.selection_contains_visible_cell(19, 0));
        assert!(term.selection_contains_visible_cell(2, 1));
        assert!(!term.selection_contains_visible_cell(3, 1));
    }

    #[test]
    fn selection_skips_wide_spacer_cells() {
        let mut term = Terminal::new(10, 1);
        term.feed("A界B".as_bytes());

        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(3, 0));

        assert_eq!(term.selected_text(), Some("A界B".to_owned()));
    }

    #[test]
    fn scrollback_selection_extracts_visible_history_text() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        feed_lines(&mut term, &["A", "B", "C", "D"]);
        term.scroll_to_top();
        let grid = term.snapshot();
        let expected = (0..grid.rows)
            .map(|row| line_text(&grid, row).trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n");

        let start = term.selection_point_for_visible_cell(0, 0).unwrap();
        let end = term.selection_point_for_visible_cell(4, 1).unwrap();
        term.begin_selection(start, SelectionMode::Simple);
        term.update_selection(end);

        assert_eq!(term.selected_text(), Some(expected));
    }

    #[test]
    fn scrolled_selection_rects_only_include_visible_rows() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        feed_lines(&mut term, &["A", "B", "C", "D"]);
        term.scroll_to_top();

        let start = term.selection_point_for_visible_cell(1, 0).unwrap();
        let end = term.selection_point_for_visible_cell(3, 1).unwrap();
        term.begin_selection(start, SelectionMode::Simple);
        term.update_selection(end);

        assert_eq!(
            term.selection_rects(),
            vec![
                SelectionRect {
                    col: 1,
                    row: 0,
                    width: 4,
                    height: 1,
                },
                SelectionRect {
                    col: 0,
                    row: 1,
                    width: 4,
                    height: 1,
                },
            ]
        );
    }

    #[test]
    fn clear_selection_removes_text_and_rects() {
        let mut term = Terminal::new(10, 1);
        term.feed(b"hello");
        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(4, 0));

        term.clear_selection();

        assert_eq!(term.selected_text(), None);
        assert!(term.selection_rects().is_empty());
    }

    #[test]
    fn feed_clears_existing_selection() {
        let mut term = Terminal::new(10, 1);
        term.feed(b"hello");
        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(4, 0));

        term.feed(b"!");

        assert_eq!(term.selected_text(), None);
        assert!(term.snapshot().selection_rects.is_empty());
    }

    #[test]
    fn resize_clears_existing_selection() {
        let mut term = Terminal::new(10, 1);
        term.feed(b"hello");
        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(4, 0));

        term.resize(20, 2);

        assert_eq!(term.selected_text(), None);
        assert!(term.selection_rects().is_empty());
    }

    #[test]
    fn word_selection_expands_ascii_word_constituents() {
        let mut term = Terminal::new(20, 1);
        term.feed(b"foo/bar baz");

        term.begin_selection(point(2, 0), SelectionMode::Word);

        assert_eq!(term.selected_text(), Some("foo/bar".to_owned()));
    }

    #[test]
    fn line_selection_selects_visible_line_without_trailing_spaces() {
        let mut term = Terminal::new(20, 2);
        term.feed(b"hello   \r\nnext");

        term.begin_selection(point(3, 0), SelectionMode::Line);

        assert_eq!(term.selected_text(), Some("hello".to_owned()));
    }

    #[test]
    fn alternate_screen_selection_does_not_read_primary_scrollback() {
        let mut term = Terminal::with_scrollback(5, 2, 10);
        feed_lines(&mut term, &["A", "B", "C"]);
        term.feed(b"\x1b[?1049h");
        term.feed(b"\x1b[Halt");

        term.begin_selection(point(0, 0), SelectionMode::Simple);
        term.update_selection(point(2, 0));

        assert_eq!(term.selected_text(), Some("alt".to_owned()));
    }

    #[test]
    fn non_full_scroll_region_without_top_edge_does_not_add_scrollback() {
        let mut term = Terminal::with_scrollback(5, 4, 10);

        term.feed(b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC\x1b[4;1HD");
        // TODO(phase4-b): define exact scrollback behavior for all non-full scroll regions.
        term.feed(b"\x1b[2;3r\x1b[3;1H\n");

        assert_eq!(term.scrollback_len(), 0);
    }

    #[test]
    fn image_placement_uses_current_cursor_as_anchor() {
        let mut term = Terminal::new(10, 4);
        term.feed(b"\x1b[2;3H");
        let anchor = term.image_anchor();
        let placement = image_placement(anchor, 2);

        term.add_image_placement(placement);

        assert_eq!(term.snapshot().image_placements, vec![placement]);
        assert_eq!(term.take_damage(), Damage::Full);
    }

    #[test]
    fn image_placement_id_upserts_and_removes_one_logical_placement() {
        let mut term = Terminal::new(10, 4);
        let original = image_placement(GridPoint { col: 0, row: 0 }, 2);
        term.add_image_placement(original);
        let replacement = ImagePlacement {
            anchor: GridPoint { col: 4, row: 1 },
            ..original
        };

        term.upsert_image_placement(replacement);

        assert_eq!(term.snapshot().image_placements, vec![replacement]);
        term.remove_image_placement(replacement.placement_id);
        assert!(term.snapshot().image_placements.is_empty());
    }

    #[test]
    fn removing_image_placements_preserves_other_image_resources() {
        let mut term = Terminal::new(10, 4);
        let first = image_placement(GridPoint { col: 0, row: 0 }, 1);
        let second = ImagePlacement {
            placement_id: ImagePlacementId::new(2),
            image_id: ImageId::new(2),
            anchor: GridPoint { col: 1, row: 0 },
            ..first
        };
        term.add_image_placement(first);
        term.add_image_placement(second);

        term.remove_image_placements(first.image_id);

        assert_eq!(term.snapshot().image_placements, vec![second]);
    }

    #[test]
    fn advancing_after_image_moves_cursor_by_image_rows() {
        let mut term = Terminal::new(10, 6);
        term.feed(b"\x1b[2;3H");

        term.advance_after_image_rows(3);

        let cursor = term.snapshot().cursor;
        assert_eq!((cursor.x, cursor.y), (0, 4));
    }

    #[test]
    fn kitty_image_cursor_moves_after_rectangle_without_writing_cells() {
        let mut term = Terminal::new(10, 6);
        term.feed(b"\x1b[2;3H");

        term.move_after_kitty_image(3, 2);

        let snapshot = term.snapshot();
        assert_eq!((snapshot.cursor.x, snapshot.cursor.y), (5, 2));
        assert!(snapshot.cells.iter().all(|cell| cell.ch == ' '));
    }

    #[test]
    fn kitty_image_cursor_wraps_to_column_zero_at_right_edge() {
        let mut term = Terminal::new(10, 6);
        term.feed(b"\x1b[1;9H");

        term.move_after_kitty_image(3, 2);

        let cursor = term.snapshot().cursor;
        assert_eq!((cursor.x, cursor.y), (0, 2));
    }

    #[test]
    fn image_near_bottom_scrolls_with_generated_rows() {
        let mut term = Terminal::with_scrollback(10, 4, 10);
        term.feed(b"\x1b[4;1H");
        let placement = image_placement(term.image_anchor(), 2);
        term.add_image_placement(placement);

        term.advance_after_image_rows(2);

        let visible = term.snapshot().image_placements;
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].anchor.row, 1);
    }

    #[test]
    fn user_scrollback_reveals_image_that_left_current_viewport() {
        let mut term = Terminal::with_scrollback(10, 3, 10);
        term.add_image_placement(image_placement(term.image_anchor(), 1));
        term.feed(b"\r\none\r\ntwo\r\nthree\r\nfour");
        assert!(term.snapshot().image_placements.is_empty());

        term.scroll_to_top();

        assert_eq!(term.snapshot().image_placements.len(), 1);
    }

    #[test]
    fn resize_keeps_image_cell_dimensions() {
        let mut term = Terminal::new(10, 4);
        let placement = image_placement(term.image_anchor(), 3);
        term.add_image_placement(placement);

        term.resize(20, 8);

        let visible = term.snapshot().image_placements;
        assert_eq!(visible[0].columns, placement.columns);
        assert_eq!(visible[0].rows, placement.rows);
    }

    #[test]
    fn terminal_reset_clears_image_placements() {
        let mut term = Terminal::new(10, 4);
        term.add_image_placement(image_placement(term.image_anchor(), 2));

        term.feed(b"\x1bc");

        assert!(term.snapshot().image_placements.is_empty());
    }

    fn line_text(grid: &super::GridSnapshot, row: usize) -> String {
        (0..grid.cols)
            .map(|column| grid.cell(column, row).ch)
            .collect()
    }

    fn snapshot_text(grid: &super::GridSnapshot) -> String {
        (0..grid.rows)
            .map(|row| line_text(grid, row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn find_cell(grid: &super::GridSnapshot, ch: char) -> Option<(usize, usize)> {
        (0..grid.rows)
            .flat_map(|row| (0..grid.cols).map(move |column| (column, row)))
            .find(|(column, row)| grid.cell(*column, *row).ch == ch)
    }

    fn feed_lines(term: &mut Terminal, lines: &[&str]) {
        for line in lines {
            term.feed(line.as_bytes());
            term.feed(b"\r\n");
        }
    }

    fn point(col: usize, row: isize) -> SelectionPoint {
        SelectionPoint { col, row }
    }

    fn image_placement(anchor: GridPoint, rows: u16) -> ImagePlacement {
        ImagePlacement {
            placement_id: ImagePlacementId::new(1),
            image_id: ImageId::new(1),
            anchor,
            columns: 2,
            rows,
            source_width: 2,
            source_height: u32::from(rows),
        }
    }

    fn mouse_event(kind: MouseEventKind, button: Option<MouseButton>) -> TerminalMouseEvent {
        TerminalMouseEvent {
            kind,
            button,
            col: 2,
            row: 1,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }
}
