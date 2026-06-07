use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

mod config;

use arboard::Clipboard;
use config::{AppConfig, load_app_config, parse_wgpu_backend_name};
use knightty_core::{
    Damage, GridSnapshot, Hyperlink, MouseButton as TerminalMouseButton, MouseEventKind,
    MouseProtocol, SelectionMode, SelectionPoint, Terminal, TerminalMouseEvent,
};
use knightty_pty::{PtySession, PtySize};
use knightty_render::{
    FontFamilyInfo, RenderError, RenderOptions, Renderer, RendererConfig, available_font_families,
};
use thiserror::Error;
use url::Url;
use wgpu::{Backends, Instance, InstanceDescriptor};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Cursor, CursorIcon, Window, WindowId};

fn main() -> Result<(), AppError> {
    match startup_action_from_args(std::env::args())? {
        StartupAction::RunApp => run_app(),
        StartupAction::ListFonts => {
            print_font_list(&available_font_families());
            Ok(())
        }
    }
}

fn run_app() -> Result<(), AppError> {
    let config = load_app_config()?;
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    event_loop.run_app(&mut Application::new(proxy, config))?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupAction {
    RunApp,
    ListFonts,
}

fn startup_action_from_args(
    args: impl IntoIterator<Item = String>,
) -> Result<StartupAction, AppError> {
    let mut args = args.into_iter();
    let _program = args.next();
    let Some(action) = args.next() else {
        return Ok(StartupAction::RunApp);
    };

    if action == "+list-fonts" && args.next().is_none() {
        Ok(StartupAction::ListFonts)
    } else if action.starts_with('+') {
        Err(AppError::UnknownAction(action))
    } else {
        Err(AppError::UnexpectedArgument(action))
    }
}

fn print_font_list(fonts: &[FontFamilyInfo]) {
    print!("{}", format_font_list(fonts));
}

fn format_font_list(fonts: &[FontFamilyInfo]) -> String {
    let mut output = String::new();
    for font in fonts {
        output.push_str(&font.name);
        output.push('\n');
    }
    output
}

#[derive(Debug)]
enum UserEvent {
    PtyBytes(Vec<u8>),
    PtyExited,
}

struct Application {
    config: AppConfig,
    renderer_config: RendererConfig,
    proxy: EventLoopProxy<UserEvent>,
    window_state: Option<WindowState>,
    terminal: Terminal,
    pty: Option<PtySession>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    modifiers: ModifiersState,
    last_cursor_position: Option<PhysicalPosition<f64>>,
    pressed_mouse_button: Option<TerminalMouseButton>,
    selection_drag_active: bool,
    selection_drag_anchor: Option<SelectionPoint>,
    selection_drag_mode: Option<SelectionMode>,
    selection_drag_moved: bool,
    hovered_hyperlink: Option<HoveredHyperlink>,
    pending_hyperlink_click: Option<PendingHyperlinkClick>,
    hover_visual_dirty: bool,
    current_cursor_icon: CursorIcon,
    click_tracker: ClickTracker,
}

impl Application {
    fn new(proxy: EventLoopProxy<UserEvent>, config: AppConfig) -> Self {
        let renderer_config = config.renderer_config();
        let initial_cols = config.initial_cols();
        let initial_rows = config.initial_rows();
        let scrollback_lines = config.scrollback_lines();
        Self {
            config,
            renderer_config,
            proxy,
            window_state: None,
            terminal: Terminal::with_scrollback(initial_cols, initial_rows, scrollback_lines),
            pty: None,
            writer: None,
            modifiers: ModifiersState::empty(),
            last_cursor_position: None,
            pressed_mouse_button: None,
            selection_drag_active: false,
            selection_drag_anchor: None,
            selection_drag_mode: None,
            selection_drag_moved: false,
            hovered_hyperlink: None,
            pending_hyperlink_click: None,
            hover_visual_dirty: false,
            current_cursor_icon: CursorIcon::Default,
            click_tracker: ClickTracker::default(),
        }
    }

    fn start_pty(&mut self, pixel_width: u32, pixel_height: u32) -> Result<(), AppError> {
        if self.pty.is_some() {
            return Ok(());
        }

        let (cols, rows) = self.terminal.size();
        let size = pty_size_for_terminal(cols, rows, pixel_width, pixel_height);
        let shell = self.config.shell_command();
        let mut pty = PtySession::spawn_shell(size, shell.as_ref())?;
        let reader = pty.take_reader()?;
        let writer = Arc::new(Mutex::new(pty.take_writer()?));
        spawn_pty_reader(reader, self.proxy.clone());

        self.writer = Some(writer);
        self.pty = Some(pty);
        Ok(())
    }

    fn request_redraw(&self) {
        if let Some(state) = &self.window_state {
            state.window.request_redraw();
        }
    }

    fn write_pty(&mut self, bytes: &[u8]) {
        let Some(writer) = &self.writer else {
            return;
        };

        if let Ok(mut writer) = writer.lock() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    fn scroll_terminal_to_bottom(&mut self) {
        if !matches!(self.terminal.scroll_to_bottom(), Damage::None) {
            self.refresh_hover_hyperlink();
            self.request_redraw();
        }
    }

    fn resize_terminal(&mut self, width: u32, height: u32) {
        let metrics = self
            .window_state
            .as_ref()
            .map(|state| state.renderer.cell_metrics())
            .unwrap_or(self.renderer_config.cell_metrics);
        let cols = metrics.cols_for_width(width);
        let rows = metrics.rows_for_height(height);
        self.terminal.resize(cols, rows);

        if let Some(pty) = &mut self.pty {
            let _ = pty.resize(pty_size_for_terminal(cols, rows, width, height));
        }
    }

    fn paste_from_clipboard(&mut self) {
        let text = match Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
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

        if let Err(error) = Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
            eprintln!("knightty clipboard: copy failed: {error}");
        }
    }

    fn write_user_input_to_pty(&mut self, bytes: &[u8]) {
        if self.terminal.has_selection() {
            self.terminal.clear_selection();
            self.update_cursor_icon();
            self.request_redraw();
        }

        self.scroll_terminal_to_bottom();
        self.write_pty(bytes);
    }

    fn clear_selection_if_left_click_outside_selection(&mut self) {
        if !self.terminal.has_selection() {
            return;
        }

        let outside_selection = self
            .cursor_cell_position()
            .is_none_or(|(col, row)| !self.terminal.selection_contains_visible_cell(col, row));
        if outside_selection {
            self.terminal.clear_selection();
            self.update_cursor_icon();
            self.request_redraw();
        }
    }

    fn cursor_hyperlink(&self) -> Option<HoveredHyperlink> {
        let cell = self.cursor_cell_position()?;
        let hyperlink = self.terminal.hyperlink_at_cell(cell.0, cell.1)?;
        Some(HoveredHyperlink { cell, hyperlink })
    }

    fn refresh_hover_hyperlink(&mut self) {
        self.set_hovered_hyperlink(self.cursor_hyperlink());
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
            self.request_redraw();
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
        if let Some(state) = &self.window_state {
            state.window.set_cursor(Cursor::Icon(icon));
        }
    }

    fn hovered_hyperlink_id_for_snapshot(&self, snapshot: &GridSnapshot) -> Option<usize> {
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

    fn handle_hyperlink_open(&self, hyperlink: &Hyperlink) {
        match allowed_hyperlink_url(&hyperlink.uri, self.config.hyperlink_allowed_schemes()) {
            Ok(url) => open_url_in_background(url),
            Err(error) => {
                eprintln!("knightty hyperlink: rejected `{}`: {error}", hyperlink.uri);
            }
        }
    }

    fn cursor_cell_position(&self) -> Option<(usize, usize)> {
        let position = self.last_cursor_position?;
        let state = self.window_state.as_ref()?;
        let metrics = state.renderer.cell_metrics();
        let (cols, rows) = self.terminal.size();
        cell_position_for_pixel(CellHitTest {
            pixel_x: position.x,
            pixel_y: position.y,
            origin_x: 0.0,
            origin_y: 0.0,
            cell_width: metrics.width,
            cell_height: metrics.height,
            cols,
            rows,
        })
    }

    fn cursor_selection_point(&self) -> Option<SelectionPoint> {
        let (col, row) = self.cursor_cell_position()?;
        self.terminal.selection_point_for_visible_cell(col, row)
    }

    fn terminal_mouse_event(
        &self,
        kind: MouseEventKind,
        button: Option<TerminalMouseButton>,
    ) -> Option<TerminalMouseEvent> {
        let (col, row) = self.cursor_cell_position()?;
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

    fn handle_mouse_input(&mut self, state: ElementState, button: WinitMouseButton) {
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
                    self.request_redraw();
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
                    self.request_redraw();
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

    fn handle_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.last_cursor_position = Some(position);
        if self.selection_drag_active {
            self.clear_hover_hyperlink();
            if let Some(point) = self.cursor_selection_point() {
                if self.selection_drag_anchor != Some(point) {
                    self.selection_drag_moved = true;
                }
                self.terminal.update_selection(point);
                self.request_redraw();
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

    fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
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
                    self.request_redraw();
                }
            }
            WheelRouting::Noop => {}
        }
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
            self.request_redraw();
        }
    }

    fn handle_focus_event(&mut self, focused: bool) {
        let Some(bytes) = self.terminal.encode_focus_event(focused) else {
            return;
        };
        self.write_pty(&bytes);
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window_state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("knightty")
                        .with_inner_size(LogicalSize::new(
                            (self.config.initial_cols() as u32
                                * self.renderer_config.cell_metrics.width)
                                as f64,
                            (self.config.initial_rows() as u32
                                * self.renderer_config.cell_metrics.height)
                                as f64,
                        )),
                )
                .expect("create window"),
        );
        let size = window.inner_size();
        let mut instance_descriptor = InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        ));
        instance_descriptor.backends = match selected_wgpu_backends(self.config.wgpu_backend()) {
            Ok(backends) => backends,
            Err(error) => {
                eprintln!("{error}");
                event_loop.exit();
                return;
            }
        };
        let instance = Instance::new(instance_descriptor);
        let surface = instance
            .create_surface(window.clone())
            .expect("create render surface");
        let renderer = match pollster::block_on(Renderer::with_config(
            instance,
            surface,
            size.width,
            size.height,
            self.renderer_config.clone(),
        )) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("knightty renderer: initialization failed: {error}");
                if matches!(error, RenderError::NoAdapter) {
                    eprintln!(
                        "knightty renderer: no primary backend adapter was found; install a Vulkan/DX12-capable driver or run with KNIGHTTY_WGPU_BACKEND=gl for the explicit software/fallback path"
                    );
                }
                event_loop.exit();
                return;
            }
        };

        self.resize_terminal(size.width, size.height);
        self.start_pty(size.width, size.height)
            .expect("spawn default shell");
        self.window_state = Some(WindowState {
            window: window.clone(),
            renderer,
        });
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytes(bytes) => {
                self.terminal.feed(&bytes);
                for response in self.terminal.take_pty_writes() {
                    self.write_pty(response.as_bytes());
                }
                self.refresh_hover_hyperlink();
                let window_title = self.terminal.take_window_title_changed();
                if let Some(state) = &self.window_state {
                    if let Some(title) = window_title {
                        state.window.set_title(&format_window_title(&title));
                    }
                    state.window.request_redraw();
                }
            }
            UserEvent::PtyExited => {
                if let Some(state) = &self.window_state {
                    state.window.request_redraw();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let Some(action) =
                        scroll_shortcut_for_key(&event.logical_key, self.modifiers)
                    {
                        self.handle_scroll_shortcut(action);
                        return;
                    }
                    if is_copy_shortcut(&event.logical_key, self.modifiers) {
                        self.copy_selection_to_clipboard();
                        return;
                    }
                    if is_paste_shortcut(&event.logical_key, self.modifiers) {
                        self.paste_from_clipboard();
                        return;
                    }
                    if let Some(bytes) =
                        input_bytes(&event.logical_key, event.text.as_deref(), self.modifiers)
                    {
                        self.write_user_input_to_pty(&bytes);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position);
            }
            WindowEvent::CursorLeft { .. } => {
                self.last_cursor_position = None;
                self.pending_hyperlink_click = None;
                self.clear_hover_hyperlink();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.handle_mouse_wheel(delta);
            }
            WindowEvent::Focused(focused) => {
                self.handle_focus_event(focused);
            }
            WindowEvent::Resized(size) => {
                self.resize_terminal(size.width, size.height);
                self.refresh_hover_hyperlink();
                if let Some(state) = &mut self.window_state {
                    state.renderer.resize(size.width, size.height);
                    state.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let snapshot = self.terminal.snapshot();
                let hovered_hyperlink_id = self.hovered_hyperlink_id_for_snapshot(&snapshot);
                let hover_visual_dirty = self.hover_visual_dirty;
                let damage = if hover_visual_dirty {
                    let _ = self.terminal.take_damage();
                    Damage::Full
                } else {
                    self.terminal.take_damage()
                };
                if matches!(damage, Damage::None) {
                    return;
                }
                self.hover_visual_dirty = false;

                if let Some(state) = &mut self.window_state {
                    match state.renderer.render(
                        &snapshot,
                        &damage,
                        RenderOptions {
                            hovered_hyperlink_id,
                        },
                    ) {
                        Ok(()) => {}
                        Err(RenderError::SurfaceLost) => {
                            if state
                                .renderer
                                .recreate_surface(state.window.clone())
                                .is_ok()
                            {
                                state.window.request_redraw();
                            } else {
                                event_loop.exit();
                            }
                        }
                        Err(RenderError::OutOfMemory) => event_loop.exit(),
                        Err(error) => {
                            eprintln!("knightty renderer: render failed: {error}");
                            state.window.request_redraw();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

struct WindowState {
    window: Arc<Window>,
    renderer: Renderer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HoveredHyperlink {
    cell: (usize, usize),
    hyperlink: Hyperlink,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingHyperlinkClick {
    hovered: HoveredHyperlink,
}

#[derive(Clone, Copy, Debug)]
struct HyperlinkClickInput {
    left_button: bool,
    ctrl_pressed: bool,
    open_enabled: bool,
    has_hyperlink: bool,
    selection_drag_active: bool,
    selection_active: bool,
    mouse_reporting_enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HyperlinkClickRouting {
    PendingOpen { mouse_reporting_overridden: bool },
    ExistingMouseRouting,
}

fn route_hyperlink_click(input: HyperlinkClickInput) -> HyperlinkClickRouting {
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

fn should_open_pending_hyperlink(
    pending: &PendingHyperlinkClick,
    current: Option<&HoveredHyperlink>,
    selection_drag_active: bool,
    selection_active: bool,
) -> bool {
    !selection_drag_active && !selection_active && current == Some(&pending.hovered)
}

fn should_clear_simple_click_selection(
    anchor: Option<SelectionPoint>,
    focus: Option<SelectionPoint>,
    mode: SelectionMode,
    moved: bool,
) -> bool {
    matches!(mode, SelectionMode::Simple) && !moved && anchor.is_some() && anchor == focus
}

fn hyperlink_cursor_icon(has_hover: bool, selection_active: bool) -> CursorIcon {
    if has_hover && !selection_active {
        CursorIcon::Pointer
    } else {
        CursorIcon::Default
    }
}

fn allowed_hyperlink_url(
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

fn open_url_in_background(url: String) {
    let _ = thread::spawn(move || {
        if let Err(error) = open::that(&url) {
            eprintln!("knightty hyperlink: failed to open `{url}`: {error}");
        }
    });
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
enum HyperlinkUrlError {
    #[error("no URL schemes are allowed")]
    NoAllowedSchemes,
    #[error("invalid URL")]
    InvalidUrl,
    #[error("scheme `{0}` is not allowed")]
    DisallowedScheme(String),
}

fn input_bytes(key: &Key, text: Option<&str>, modifiers: ModifiersState) -> Option<Vec<u8>> {
    if modifiers.control_key() {
        match key {
            Key::Character(value) if value.eq_ignore_ascii_case("c") => {
                return Some(b"\x03".to_vec());
            }
            _ => {}
        }
    }

    match key {
        Key::Named(NamedKey::Enter) => Some(b"\r".to_vec()),
        Key::Named(NamedKey::Backspace) => Some(b"\x7f".to_vec()),
        Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        _ => text
            .filter(|value| !value.is_empty() && !modifiers.control_key() && !modifiers.super_key())
            .map(|value| value.as_bytes().to_vec()),
    }
}

fn is_paste_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
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

fn is_copy_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
    modifiers.control_key()
        && modifiers.shift_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("c"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseDragRouting {
    Selection,
    Pty,
}

fn route_mouse_drag(mouse_reporting_enabled: bool, shift_pressed: bool) -> MouseDragRouting {
    if shift_pressed || !mouse_reporting_enabled {
        MouseDragRouting::Selection
    } else {
        MouseDragRouting::Pty
    }
}

const MULTI_CLICK_MAX_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
struct ClickTracker {
    last_point: Option<SelectionPoint>,
    last_at: Option<Instant>,
    count: u8,
}

impl ClickTracker {
    fn record_click(&mut self, point: SelectionPoint, now: Instant) -> SelectionMode {
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
enum ScrollShortcut {
    PageUp,
    PageDown,
    Top,
    Bottom,
}

fn scroll_shortcut_for_key(key: &Key, modifiers: ModifiersState) -> Option<ScrollShortcut> {
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

#[derive(Clone, Copy, Debug)]
struct CellHitTest {
    pixel_x: f64,
    pixel_y: f64,
    origin_x: f64,
    origin_y: f64,
    cell_width: u32,
    cell_height: u32,
    cols: usize,
    rows: usize,
}

fn cell_position_for_pixel(hit: CellHitTest) -> Option<(usize, usize)> {
    if hit.cell_width == 0
        || hit.cell_height == 0
        || hit.pixel_x < hit.origin_x
        || hit.pixel_y < hit.origin_y
    {
        return None;
    }

    let col = ((hit.pixel_x - hit.origin_x) / f64::from(hit.cell_width)).floor();
    let row = ((hit.pixel_y - hit.origin_y) / f64::from(hit.cell_height)).floor();
    if col < 0.0 || row < 0.0 {
        return None;
    }

    let col = col as usize;
    let row = row as usize;
    if col >= hit.cols || row >= hit.rows {
        return None;
    }

    Some((col, row))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WheelRouting {
    PtyMouse(TerminalMouseButton),
    Scrollback {
        direction: ScrollDirection,
        lines: usize,
    },
    Noop,
}

fn route_mouse_wheel(
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

fn terminal_mouse_button(button: WinitMouseButton) -> Option<TerminalMouseButton> {
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

fn spawn_pty_reader(mut reader: Box<dyn Read + Send>, proxy: EventLoopProxy<UserEvent>) {
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    let reader_proxy = proxy.clone();

    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if sender.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    eprintln!("knightty pty: reader exited: {error}");
                    break;
                }
            }
        }
        let _ = reader_proxy.send_event(UserEvent::PtyExited);
    });

    thread::spawn(move || {
        let mut pending = Vec::new();
        while let Ok(chunk) = receiver.recv() {
            pending.extend_from_slice(&chunk);

            while let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(4)) {
                pending.extend_from_slice(&chunk);
            }

            let bytes = core::mem::take(&mut pending);
            if proxy.send_event(UserEvent::PtyBytes(bytes)).is_err() {
                break;
            }
        }
    });
}

fn pty_size_for_terminal(cols: usize, rows: usize, pixel_width: u32, pixel_height: u32) -> PtySize {
    PtySize {
        rows: clamp_pty_cells(rows),
        cols: clamp_pty_cells(cols),
        pixel_width: clamp_pty_pixels(pixel_width),
        pixel_height: clamp_pty_pixels(pixel_height),
    }
}

fn clamp_pty_cells(value: usize) -> u16 {
    value.clamp(1, u16::MAX as usize) as u16
}

fn clamp_pty_pixels(value: u32) -> u16 {
    value.min(u16::MAX as u32) as u16
}

fn format_window_title(title: &str) -> String {
    if title.is_empty() {
        "knightty".to_owned()
    } else {
        format!("{title} - knightty")
    }
}

fn selected_wgpu_backends(config_value: Option<&str>) -> Result<Backends, AppError> {
    let value = std::env::var("KNIGHTTY_WGPU_BACKEND")
        .ok()
        .or_else(|| config_value.map(str::to_owned))
        .unwrap_or_else(|| "auto".to_owned());
    let Some(value) = parse_wgpu_backend_name(&value) else {
        return Err(AppError::InvalidWgpuBackend(value));
    };

    match value {
        "auto" => Ok(Backends::PRIMARY),
        "vulkan" => Ok(Backends::VULKAN),
        "dx12" => Ok(Backends::DX12),
        "gl" => Ok(Backends::GL),
        _ => unreachable!("config parser only accepts supported backend names"),
    }
}

#[derive(Debug, Error)]
enum AppError {
    #[error("event loop failed: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
    #[error("PTY failed: {0}")]
    Pty(#[from] knightty_pty::PtyError),
    #[error("config failed: {0}")]
    Config(#[from] config::ConfigError),
    #[error("invalid wgpu backend `{0}`; expected auto, vulkan, dx12, or gl")]
    InvalidWgpuBackend(String),
    #[error("unknown action `{0}`; expected +list-fonts")]
    UnknownAction(String),
    #[error("unexpected argument `{0}`; run without arguments or use +list-fonts")]
    UnexpectedArgument(String),
}

#[cfg(test)]
mod tests {
    use super::{
        CellHitTest, ClickTracker, HoveredHyperlink, HyperlinkClickInput, HyperlinkClickRouting,
        HyperlinkUrlError, MouseDragRouting, PendingHyperlinkClick, ScrollDirection,
        ScrollShortcut, WheelRouting, allowed_hyperlink_url, cell_position_for_pixel,
        format_font_list, format_window_title, hyperlink_cursor_icon, is_copy_shortcut,
        is_paste_shortcut, pty_size_for_terminal, route_hyperlink_click, route_mouse_drag,
        route_mouse_wheel, scroll_shortcut_for_key, should_clear_simple_click_selection,
        should_open_pending_hyperlink,
    };
    use std::time::{Duration, Instant};

    use knightty_core::{
        Hyperlink, MouseButton as TerminalMouseButton, SelectionMode, SelectionPoint,
    };
    use knightty_render::FontFamilyInfo;
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    #[test]
    fn startup_action_defaults_to_running_app() {
        assert_eq!(
            super::startup_action_from_args(["knightty"].map(str::to_owned)).unwrap(),
            super::StartupAction::RunApp
        );
    }

    #[test]
    fn startup_action_accepts_list_fonts() {
        assert_eq!(
            super::startup_action_from_args(["knightty", "+list-fonts"].map(str::to_owned))
                .unwrap(),
            super::StartupAction::ListFonts
        );
    }

    #[test]
    fn startup_action_rejects_unknown_plus_action() {
        assert!(matches!(
            super::startup_action_from_args(["knightty", "+missing"].map(str::to_owned)),
            Err(super::AppError::UnknownAction(action)) if action == "+missing"
        ));
    }

    #[test]
    fn font_list_output_uses_plain_family_names() {
        let output = format_font_list(&[
            FontFamilyInfo {
                name: "CaskaydiaCove Nerd Font".to_owned(),
                monospaced: true,
            },
            FontFamilyInfo {
                name: "Inter".to_owned(),
                monospaced: false,
            },
        ]);

        assert_eq!(output, "CaskaydiaCove Nerd Font\nInter\n");
    }

    #[test]
    fn pty_size_uses_live_terminal_grid_and_window_pixels() {
        let size = pty_size_for_terminal(132, 43, 1920, 1080);

        assert_eq!(size.cols, 132);
        assert_eq!(size.rows, 43);
        assert_eq!(size.pixel_width, 1920);
        assert_eq!(size.pixel_height, 1080);
    }

    #[test]
    fn pty_size_clamps_zero_cells_and_large_pixels() {
        let size = pty_size_for_terminal(0, usize::MAX, u32::MAX, 70_000);

        assert_eq!(size.cols, 1);
        assert_eq!(size.rows, u16::MAX);
        assert_eq!(size.pixel_width, u16::MAX);
        assert_eq!(size.pixel_height, u16::MAX);
    }

    #[test]
    fn cell_position_maps_pixels_to_zero_based_cells() {
        assert_eq!(
            cell_position_for_pixel(hit(18.0, 36.0, 0.0, 0.0)),
            Some((2, 2))
        );
    }

    #[test]
    fn cell_position_accounts_for_origin_padding() {
        assert_eq!(
            cell_position_for_pixel(hit(23.0, 46.0, 5.0, 10.0)),
            Some((2, 2))
        );
    }

    #[test]
    fn cell_position_rejects_out_of_range_coordinates() {
        assert_eq!(cell_position_for_pixel(hit(720.0, 36.0, 0.0, 0.0)), None);
        assert_eq!(cell_position_for_pixel(hit(18.0, 432.0, 0.0, 0.0)), None);
        assert_eq!(cell_position_for_pixel(hit(4.0, 36.0, 5.0, 0.0)), None);
    }

    #[test]
    fn ctrl_shift_v_is_paste_shortcut() {
        assert!(is_paste_shortcut(
            &Key::Character("v".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ));
        assert!(is_paste_shortcut(
            &Key::Character("V".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ));
        assert!(!is_paste_shortcut(
            &Key::Character("v".into()),
            ModifiersState::CONTROL
        ));
    }

    #[test]
    fn shift_insert_is_paste_shortcut() {
        assert!(is_paste_shortcut(
            &Key::Named(NamedKey::Insert),
            ModifiersState::SHIFT
        ));
        assert!(!is_paste_shortcut(
            &Key::Named(NamedKey::Insert),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ));
    }

    #[test]
    fn ctrl_shift_c_is_copy_shortcut() {
        assert!(is_copy_shortcut(
            &Key::Character("c".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ));
        assert!(is_copy_shortcut(
            &Key::Character("C".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ));
    }

    #[test]
    fn ctrl_c_is_not_copy_shortcut() {
        assert!(!is_copy_shortcut(
            &Key::Character("c".into()),
            ModifiersState::CONTROL
        ));
    }

    #[test]
    fn mouse_reporting_off_routes_drag_to_selection() {
        assert_eq!(route_mouse_drag(false, false), MouseDragRouting::Selection);
    }

    #[test]
    fn mouse_reporting_on_without_shift_routes_drag_to_pty() {
        assert_eq!(route_mouse_drag(true, false), MouseDragRouting::Pty);
    }

    #[test]
    fn mouse_reporting_on_with_shift_routes_drag_to_selection() {
        assert_eq!(route_mouse_drag(true, true), MouseDragRouting::Selection);
    }

    #[test]
    fn allowed_hyperlink_url_accepts_https_and_http() {
        let allowed = schemes(["https", "http"]);

        assert_eq!(
            allowed_hyperlink_url("https://example.com/path", &allowed),
            Ok("https://example.com/path".to_owned())
        );
        assert_eq!(
            allowed_hyperlink_url("http://example.com/path", &allowed),
            Ok("http://example.com/path".to_owned())
        );
    }

    #[test]
    fn allowed_hyperlink_url_rejects_disallowed_and_invalid_urls() {
        let allowed = schemes(["https", "http"]);

        assert_eq!(
            allowed_hyperlink_url("file:///C:/Windows/win.ini", &allowed),
            Err(HyperlinkUrlError::DisallowedScheme("file".to_owned()))
        );
        assert_eq!(
            allowed_hyperlink_url("javascript:alert(1)", &allowed),
            Err(HyperlinkUrlError::DisallowedScheme("javascript".to_owned()))
        );
        assert_eq!(
            allowed_hyperlink_url("not a url", &allowed),
            Err(HyperlinkUrlError::InvalidUrl)
        );
    }

    #[test]
    fn allowed_hyperlink_url_rejects_empty_allowlist() {
        assert_eq!(
            allowed_hyperlink_url("https://example.com", &[]),
            Err(HyperlinkUrlError::NoAllowedSchemes)
        );
    }

    #[test]
    fn ctrl_click_on_hyperlink_routes_to_pending_open() {
        assert_eq!(
            route_hyperlink_click(hyperlink_click_input(true, true, true, false, false, false)),
            HyperlinkClickRouting::PendingOpen {
                mouse_reporting_overridden: false
            }
        );
    }

    #[test]
    fn selection_suppresses_hyperlink_press_even_with_mouse_reporting() {
        assert_eq!(
            route_hyperlink_click(hyperlink_click_input(true, true, true, false, true, true)),
            HyperlinkClickRouting::ExistingMouseRouting
        );
    }

    #[test]
    fn click_without_ctrl_does_not_open_hyperlink() {
        assert_eq!(
            route_hyperlink_click(hyperlink_click_input(
                true, false, true, false, false, false
            )),
            HyperlinkClickRouting::ExistingMouseRouting
        );
    }

    #[test]
    fn drag_or_selection_active_prevents_hyperlink_open() {
        assert_eq!(
            route_hyperlink_click(hyperlink_click_input(true, true, true, true, false, false)),
            HyperlinkClickRouting::ExistingMouseRouting
        );
        assert_eq!(
            route_hyperlink_click(hyperlink_click_input(true, true, true, false, true, false)),
            HyperlinkClickRouting::ExistingMouseRouting
        );
    }

    #[test]
    fn mouse_reporting_does_not_prevent_ctrl_click_hyperlink_open() {
        assert_eq!(
            route_hyperlink_click(hyperlink_click_input(true, true, true, false, false, true)),
            HyperlinkClickRouting::PendingOpen {
                mouse_reporting_overridden: true
            }
        );
    }

    #[test]
    fn pending_hyperlink_opens_only_when_released_on_same_link_without_selection() {
        let current_hover = hovered("https://example.com", (1, 0));
        let pending = PendingHyperlinkClick {
            hovered: current_hover.clone(),
        };
        let other_cell = hovered("https://example.com", (2, 0));

        assert!(should_open_pending_hyperlink(
            &pending,
            Some(&current_hover),
            false,
            false
        ));
        assert!(!should_open_pending_hyperlink(
            &pending,
            Some(&other_cell),
            false,
            false
        ));
        assert!(!should_open_pending_hyperlink(
            &pending,
            Some(&current_hover),
            true,
            false
        ));
        assert!(!should_open_pending_hyperlink(
            &pending,
            Some(&current_hover),
            false,
            true
        ));
    }

    #[test]
    fn selection_suppresses_pending_hyperlink_release() {
        let current_hover = hovered("https://example.com", (1, 0));
        let pending = PendingHyperlinkClick {
            hovered: current_hover.clone(),
        };

        assert!(!should_open_pending_hyperlink(
            &pending,
            Some(&current_hover),
            false,
            true
        ));
    }

    #[test]
    fn simple_click_without_drag_clears_selection_after_release() {
        let point = SelectionPoint { col: 1, row: 0 };

        assert!(should_clear_simple_click_selection(
            Some(point),
            Some(point),
            SelectionMode::Simple,
            false
        ));
    }

    #[test]
    fn drag_or_expanded_selection_is_not_cleared_after_release() {
        let anchor = SelectionPoint { col: 1, row: 0 };
        let focus = SelectionPoint { col: 3, row: 0 };

        assert!(!should_clear_simple_click_selection(
            Some(anchor),
            Some(focus),
            SelectionMode::Simple,
            true
        ));
        assert!(!should_clear_simple_click_selection(
            Some(anchor),
            Some(anchor),
            SelectionMode::Word,
            false
        ));
        assert!(!should_clear_simple_click_selection(
            Some(anchor),
            Some(anchor),
            SelectionMode::Line,
            false
        ));
    }

    #[test]
    fn hyperlink_hover_updates_cursor_state() {
        assert_eq!(
            hyperlink_cursor_icon(true, false),
            winit::window::CursorIcon::Pointer
        );
        assert_eq!(
            hyperlink_cursor_icon(false, false),
            winit::window::CursorIcon::Default
        );
        assert_eq!(
            hyperlink_cursor_icon(true, true),
            winit::window::CursorIcon::Default
        );
    }

    #[test]
    fn click_tracker_detects_double_and_triple_clicks() {
        let mut tracker = ClickTracker::default();
        let point = SelectionPoint { col: 2, row: 1 };
        let now = Instant::now();

        assert_eq!(tracker.record_click(point, now), SelectionMode::Simple);
        assert_eq!(
            tracker.record_click(point, now + Duration::from_millis(100)),
            SelectionMode::Word
        );
        assert_eq!(
            tracker.record_click(point, now + Duration::from_millis(200)),
            SelectionMode::Line
        );
    }

    #[test]
    fn click_tracker_resets_after_timeout_or_cell_change() {
        let mut tracker = ClickTracker::default();
        let point = SelectionPoint { col: 2, row: 1 };
        let other = SelectionPoint { col: 3, row: 1 };
        let now = Instant::now();

        assert_eq!(tracker.record_click(point, now), SelectionMode::Simple);
        assert_eq!(
            tracker.record_click(point, now + Duration::from_millis(600)),
            SelectionMode::Simple
        );
        assert_eq!(
            tracker.record_click(other, now + Duration::from_millis(700)),
            SelectionMode::Simple
        );
    }

    #[test]
    fn primary_wheel_without_mouse_reporting_routes_to_scrollback() {
        assert_eq!(
            route_mouse_wheel(false, false, MouseScrollDelta::LineDelta(0.0, 2.2), 3),
            WheelRouting::Scrollback {
                direction: ScrollDirection::Up,
                lines: 9
            }
        );
        assert_eq!(
            route_mouse_wheel(
                false,
                false,
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -12.0)),
                3
            ),
            WheelRouting::Scrollback {
                direction: ScrollDirection::Down,
                lines: 3
            }
        );
    }

    #[test]
    fn mouse_reporting_routes_wheel_to_pty_mouse_event() {
        assert_eq!(
            route_mouse_wheel(true, true, MouseScrollDelta::LineDelta(0.0, 1.0), 3),
            WheelRouting::PtyMouse(TerminalMouseButton::WheelUp)
        );
        assert_eq!(
            route_mouse_wheel(false, true, MouseScrollDelta::LineDelta(0.0, -1.0), 3),
            WheelRouting::PtyMouse(TerminalMouseButton::WheelDown)
        );
    }

    #[test]
    fn alternate_wheel_without_mouse_reporting_is_noop() {
        assert_eq!(
            route_mouse_wheel(true, false, MouseScrollDelta::LineDelta(0.0, 1.0), 3),
            WheelRouting::Noop
        );
    }

    #[test]
    fn ctrl_shift_page_keys_are_scroll_shortcuts() {
        let modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;

        assert_eq!(
            scroll_shortcut_for_key(&Key::Named(NamedKey::PageUp), modifiers),
            Some(ScrollShortcut::PageUp)
        );
        assert_eq!(
            scroll_shortcut_for_key(&Key::Named(NamedKey::PageDown), modifiers),
            Some(ScrollShortcut::PageDown)
        );
        assert_eq!(
            scroll_shortcut_for_key(&Key::Named(NamedKey::Home), modifiers),
            Some(ScrollShortcut::Top)
        );
        assert_eq!(
            scroll_shortcut_for_key(&Key::Named(NamedKey::End), modifiers),
            Some(ScrollShortcut::Bottom)
        );
    }

    #[test]
    fn scroll_shortcuts_require_ctrl_shift_without_alt_or_super() {
        assert_eq!(
            scroll_shortcut_for_key(&Key::Named(NamedKey::PageUp), ModifiersState::SHIFT),
            None
        );
        assert_eq!(
            scroll_shortcut_for_key(
                &Key::Named(NamedKey::PageUp),
                ModifiersState::CONTROL | ModifiersState::SHIFT | ModifiersState::ALT
            ),
            None
        );
    }

    #[test]
    fn empty_terminal_title_formats_as_default_window_title() {
        assert_eq!(format_window_title(""), "knightty");
    }

    #[test]
    fn non_empty_terminal_title_formats_with_app_suffix() {
        assert_eq!(format_window_title("build logs"), "build logs - knightty");
    }

    #[test]
    fn sanitized_terminal_title_is_formatted_without_extra_changes() {
        let mut terminal = knightty_core::Terminal::new(10, 1);
        terminal.feed(b"\x1b]0;clean\x01 title\x07");
        let title = terminal.take_window_title_changed().unwrap();

        assert_eq!(format_window_title(&title), "clean title - knightty");
    }

    fn schemes<const N: usize>(schemes: [&str; N]) -> Vec<String> {
        schemes.iter().map(|scheme| (*scheme).to_owned()).collect()
    }

    fn hyperlink_click_input(
        ctrl_pressed: bool,
        open_enabled: bool,
        has_hyperlink: bool,
        selection_drag_active: bool,
        selection_active: bool,
        mouse_reporting_enabled: bool,
    ) -> HyperlinkClickInput {
        HyperlinkClickInput {
            left_button: true,
            ctrl_pressed,
            open_enabled,
            has_hyperlink,
            selection_drag_active,
            selection_active,
            mouse_reporting_enabled,
        }
    }

    fn hovered(uri: &str, cell: (usize, usize)) -> HoveredHyperlink {
        HoveredHyperlink {
            cell,
            hyperlink: Hyperlink {
                id: None,
                uri: uri.to_owned(),
            },
        }
    }

    fn hit(pixel_x: f64, pixel_y: f64, origin_x: f64, origin_y: f64) -> CellHitTest {
        CellHitTest {
            pixel_x,
            pixel_y,
            origin_x,
            origin_y,
            cell_width: 9,
            cell_height: 18,
            cols: 80,
            rows: 24,
        }
    }
}
