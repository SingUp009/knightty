use std::fs;
use std::io::Read;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use app::config::{AppConfig, load_app_config, parse_wgpu_backend_name};
use app::config_spec::default_config_toml;
use app::graphics_diagnostics;
use app::input::{InputRouter, RuntimeInputPorts};
use knightty_core::{Damage, GridSnapshot};
use knightty_pty::{PtySession, PtySize};
use knightty_render::{
    CellMetrics, FontFamilyInfo, RenderError, RenderOptions, Renderer, RendererConfig,
    WindowBackdrop, available_font_families,
};
use thiserror::Error;
use wgpu::{Backends, Instance, InstanceDescriptor};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
#[cfg(target_os = "windows")]
use winit::platform::windows::{BackdropType, WindowExtWindows};
use winit::window::{Window, WindowId};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const CONFIG_RELOAD_INTERVAL: Duration = Duration::from_secs(1);

fn main() -> Result<(), AppError> {
    match startup_action_from_args(std::env::args())? {
        StartupAction::RunApp => run_app(),
        StartupAction::ListFonts => {
            print_font_list(&available_font_families());
            Ok(())
        }
        StartupAction::PrintDefaultConfig => {
            print!("{}", default_config_toml());
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
    PrintDefaultConfig,
}

fn startup_action_from_args(
    args: impl IntoIterator<Item = String>,
) -> Result<StartupAction, AppError> {
    let mut args = args.into_iter();
    let _program = args.next();
    let Some(action) = args.next() else {
        return Ok(StartupAction::RunApp);
    };

    let extra_arg = args.next();
    if action == "+list-fonts" && extra_arg.is_none() {
        Ok(StartupAction::ListFonts)
    } else if action == "+print-default-config" && extra_arg.is_none() {
        Ok(StartupAction::PrintDefaultConfig)
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
    input: InputRouter<RuntimeInputPorts>,
    pty: Option<PtySession>,
    focused: bool,
    cursor_blink_visible: bool,
    cursor_blink_dirty: bool,
    surface_resize_dirty: bool,
    pending_terminal_resize: Option<PhysicalSize<u32>>,
    ime_cursor_area_active: bool,
    last_cursor_blink: Instant,
    last_config_check: Instant,
    config_modified: Option<SystemTime>,
}

impl Application {
    fn new(proxy: EventLoopProxy<UserEvent>, config: AppConfig) -> Self {
        let renderer_config = config.renderer_config();
        let input = InputRouter::new(config.clone(), RuntimeInputPorts::default());
        let config_modified = config.source_path.as_deref().and_then(config_modified_time);
        Self {
            config,
            renderer_config,
            proxy,
            window_state: None,
            input,
            pty: None,
            focused: true,
            cursor_blink_visible: true,
            cursor_blink_dirty: false,
            surface_resize_dirty: false,
            pending_terminal_resize: None,
            ime_cursor_area_active: false,
            last_cursor_blink: Instant::now(),
            last_config_check: Instant::now(),
            config_modified,
        }
    }

    fn start_pty(&mut self, pixel_width: u32, pixel_height: u32) -> Result<(), AppError> {
        if self.pty.is_some() {
            return Ok(());
        }

        let (cols, rows) = self.input.size();
        let size = pty_size_for_terminal(cols, rows, pixel_width, pixel_height);
        let shell = self.config.shell_command();
        let mut pty = PtySession::spawn_shell(size, shell.as_ref())?;
        let reader = pty.take_reader()?;
        let writer = Arc::new(Mutex::new(pty.take_writer()?));
        spawn_pty_reader(reader, self.proxy.clone());

        self.input.ports_mut().set_writer(Some(writer));
        self.pty = Some(pty);
        Ok(())
    }

    fn request_redraw(&self) {
        if let Some(state) = &self.window_state {
            state.window.request_redraw();
        }
    }

    fn cursor_blink_enabled(&self) -> bool {
        self.renderer_config.appearance.cursor.blink
    }

    fn reset_cursor_blink(&mut self) {
        if !self.cursor_blink_enabled() {
            return;
        }
        self.cursor_blink_visible = true;
        self.cursor_blink_dirty = true;
        self.last_cursor_blink = Instant::now();
    }

    fn reload_config_if_changed(&mut self) {
        if self.last_config_check.elapsed() < CONFIG_RELOAD_INTERVAL {
            return;
        }
        self.last_config_check = Instant::now();

        let Some(path) = self.config.source_path.clone() else {
            return;
        };
        let Some(modified) = config_modified_time(&path) else {
            return;
        };
        if self.config_modified.is_some_and(|last| modified <= last) {
            return;
        }

        match load_app_config() {
            Ok(config) => self.apply_reloaded_config(config, modified),
            Err(error) => eprintln!("knightty config: reload failed: {error}"),
        }
    }

    fn reload_config_now(&mut self) {
        match load_app_config() {
            Ok(config) => {
                let modified = config
                    .source_path
                    .as_deref()
                    .and_then(config_modified_time)
                    .unwrap_or_else(SystemTime::now);
                self.apply_reloaded_config(config, modified);
            }
            Err(error) => eprintln!("knightty config: reload failed: {error}"),
        }
    }

    fn apply_reloaded_config(&mut self, config: AppConfig, modified: SystemTime) {
        let renderer_config = config.renderer_config();
        self.config = config.clone();
        self.renderer_config = renderer_config.clone();
        self.config_modified = Some(modified);
        self.input.update_config(config);

        let window = self.window_state.as_ref().map(|state| state.window.clone());
        if let Some(state) = &mut self.window_state {
            state.renderer.update_config(renderer_config);
        }
        if let Some(window) = window {
            apply_platform_appearance(&window, self.renderer_config.appearance.window);
            let size = window.inner_size();
            self.resize_terminal(size.width, size.height);
            self.update_ime_cursor_area_from_current_snapshot();
            window.request_redraw();
        }
    }

    fn queue_terminal_resize(&mut self, size: PhysicalSize<u32>) {
        self.pending_terminal_resize = Some(size);
    }

    fn apply_pending_terminal_resize(&mut self) -> bool {
        let Some(size) = self.pending_terminal_resize.take() else {
            return false;
        };
        self.resize_terminal(size.width, size.height)
    }

    fn resize_terminal(&mut self, width: u32, height: u32) -> bool {
        let metrics = self
            .window_state
            .as_ref()
            .map(|state| state.renderer.cell_metrics())
            .unwrap_or(self.renderer_config.cell_metrics);
        let window = self.renderer_config.appearance.window;
        let usable_width = window.usable_width(width);
        let usable_height = window.usable_height(height);
        let cols = metrics.cols_for_width(usable_width);
        let rows = metrics.rows_for_height(usable_height);
        if self.input.size() == (cols, rows) {
            return false;
        }

        self.input.resize(cols, rows);

        if let Some(pty) = &mut self.pty {
            let _ = pty.resize(pty_size_for_terminal(
                cols,
                rows,
                usable_width,
                usable_height,
            ));
        }
        true
    }

    fn cursor_cell_for_position(&self, position: PhysicalPosition<f64>) -> Option<(usize, usize)> {
        let state = self.window_state.as_ref()?;
        let metrics = state.renderer.cell_metrics();
        let (cols, rows) = self.input.size();
        cell_position_for_pixel(CellHitTest {
            pixel_x: position.x,
            pixel_y: position.y,
            origin_x: f64::from(self.renderer_config.appearance.window.padding_x),
            origin_y: f64::from(self.renderer_config.appearance.window.padding_y),
            cell_width: metrics.width,
            cell_height: metrics.height,
            cols,
            rows,
        })
    }

    fn update_ime_cursor_area_from_current_snapshot(&self) {
        if !self.ime_cursor_area_active {
            return;
        }
        let snapshot = self.input.snapshot();
        self.update_ime_cursor_area(&snapshot);
    }

    fn update_ime_cursor_area(&self, snapshot: &GridSnapshot) {
        if !self.ime_cursor_area_active {
            return;
        }
        let Some(state) = self.window_state.as_ref() else {
            return;
        };
        let metrics = state.renderer.cell_metrics();
        let window = self.renderer_config.appearance.window;
        let Some(area) =
            ime_cursor_area_for_snapshot(snapshot, metrics, window.padding_x, window.padding_y)
        else {
            return;
        };
        state.window.set_ime_cursor_area(
            PhysicalPosition::new(area.x, area.y),
            PhysicalSize::new(area.width, area.height),
        );
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window_state.is_some() {
            return;
        }

        let window_appearance = self.renderer_config.appearance.window;
        let initial_width = self.config.initial_cols() as u32
            * self.renderer_config.cell_metrics.width
            + window_appearance.padding_x.saturating_mul(2);
        let initial_height = self.config.initial_rows() as u32
            * self.renderer_config.cell_metrics.height
            + window_appearance.padding_y.saturating_mul(2);
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("knightty")
                        .with_inner_size(LogicalSize::new(
                            initial_width as f64,
                            initial_height as f64,
                        ))
                        .with_transparent(window_appearance.opacity < 1.0),
                )
                .expect("create window"),
        );
        let size = window.inner_size();
        window.set_ime_allowed(true);
        apply_platform_appearance(&window, window_appearance);
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
        self.input.ports_mut().set_window(Some(window.clone()));
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytes(bytes) => {
                self.reset_cursor_blink();
                let window_title = self.input.feed_pty_bytes(&bytes);
                self.update_ime_cursor_area_from_current_snapshot();
                if let Some(state) = &self.window_state {
                    if let Some(title) = window_title {
                        state.window.set_title(&format_window_title(&title));
                    }
                    state.window.request_redraw();
                }
            }
            UserEvent::PtyExited => {
                self.input.clear_ime_preedit();
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
                self.input.set_modifiers(modifiers.state());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.reset_cursor_blink();
                    self.input
                        .handle_key_pressed(&event.logical_key, event.text.as_deref());
                    if self.input.take_config_reload_request() {
                        self.reload_config_now();
                    }
                    self.update_ime_cursor_area_from_current_snapshot();
                    self.request_redraw();
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.reset_cursor_blink();
                self.input.handle_ime_commit(&text);
                self.update_ime_cursor_area_from_current_snapshot();
                self.request_redraw();
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor_range)) => {
                let cursor_range = cursor_range.map(|(start, end)| start..end);
                self.input.handle_ime_preedit(text, cursor_range);
                self.update_ime_cursor_area_from_current_snapshot();
                self.request_redraw();
            }
            WindowEvent::Ime(Ime::Enabled) => {
                self.ime_cursor_area_active = true;
                self.update_ime_cursor_area_from_current_snapshot();
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.ime_cursor_area_active = false;
                self.input.clear_ime_preedit();
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let cell = self.cursor_cell_for_position(position);
                self.input.handle_cursor_moved(cell);
                self.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.input.handle_cursor_left();
                self.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.input.handle_mouse_input(state, button);
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.handle_mouse_wheel(delta);
                self.update_ime_cursor_area_from_current_snapshot();
                self.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.input.handle_focus_event(focused);
                self.update_ime_cursor_area_from_current_snapshot();
                self.request_redraw();
            }
            WindowEvent::Resized(size) => {
                self.queue_terminal_resize(size);
                let renderer_resized = if let Some(state) = &mut self.window_state {
                    state.renderer.resize(size.width, size.height)
                } else {
                    false
                };
                if renderer_resized {
                    self.surface_resize_dirty = true;
                }
                if let Some(state) = &mut self.window_state {
                    state.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.update_ime_cursor_area_from_current_snapshot();
            }
            WindowEvent::RedrawRequested => {
                let terminal_resized = self.apply_pending_terminal_resize();
                if terminal_resized {
                    self.input.refresh_hover_hyperlink();
                }
                let snapshot = self.input.snapshot();
                let inline_images = self.input.inline_images_for_snapshot(&snapshot);
                self.update_ime_cursor_area(&snapshot);
                let hovered_hyperlink_id = self.input.hovered_hyperlink_id_for_snapshot(&snapshot);
                let search_query = self.input.search_query_for_render();
                let ime_preedit = self.input.ime_preedit_for_render();
                let (search_matches, current_search_match) =
                    self.input.search_matches_for_snapshot(&snapshot);
                let blink_dirty = self.cursor_blink_dirty;
                let resize_dirty = self.surface_resize_dirty || terminal_resized;
                let mut damage = self.input.take_render_damage();
                if matches!(damage, Damage::None) && !blink_dirty && !resize_dirty {
                    return;
                }
                if matches!(damage, Damage::None) {
                    damage = Damage::Full;
                }

                if let Some(state) = &mut self.window_state {
                    match state.renderer.render(
                        &snapshot,
                        &damage,
                        RenderOptions {
                            hovered_hyperlink_id,
                            search_matches,
                            current_search_match,
                            search_query,
                            cursor_blink_visible: self.cursor_blink_visible,
                            focused: self.focused,
                            ime_preedit,
                            inline_images,
                        },
                    ) {
                        Ok(()) => {
                            self.cursor_blink_dirty = false;
                            self.surface_resize_dirty = false;
                        }
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.reload_config_if_changed();

        if !self.cursor_blink_enabled() || self.last_cursor_blink.elapsed() < CURSOR_BLINK_INTERVAL
        {
            return;
        }

        self.cursor_blink_visible = !self.cursor_blink_visible;
        self.cursor_blink_dirty = true;
        self.last_cursor_blink = Instant::now();
        self.request_redraw();
    }
}

fn config_modified_time(path: &std::path::Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

struct WindowState {
    window: Arc<Window>,
    renderer: Renderer,
}

trait PlatformAppearance {
    fn set_window_opacity(&self, opacity: f32);
    fn set_blur(&self, enabled: bool, radius: u32) -> Result<(), AppearanceError>;
    fn set_backdrop(&self, backdrop: WindowBackdrop) -> Result<(), AppearanceError>;
}

struct WinitPlatformAppearance<'a> {
    window: &'a Window,
}

impl PlatformAppearance for WinitPlatformAppearance<'_> {
    fn set_window_opacity(&self, opacity: f32) {
        self.window.set_transparent(opacity < 1.0);
    }

    fn set_blur(&self, enabled: bool, _radius: u32) -> Result<(), AppearanceError> {
        self.window.set_blur(enabled);
        Ok(())
    }

    fn set_backdrop(&self, backdrop: WindowBackdrop) -> Result<(), AppearanceError> {
        set_platform_backdrop(self.window, backdrop)
    }
}

#[cfg(target_os = "windows")]
fn set_platform_backdrop(window: &Window, backdrop: WindowBackdrop) -> Result<(), AppearanceError> {
    let backdrop_type = match backdrop {
        WindowBackdrop::None => BackdropType::None,
        WindowBackdrop::Acrylic => BackdropType::TransientWindow,
        WindowBackdrop::Mica => BackdropType::MainWindow,
        WindowBackdrop::Tabbed => BackdropType::TabbedWindow,
    };
    window.set_system_backdrop(backdrop_type);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_platform_backdrop(
    _window: &Window,
    backdrop: WindowBackdrop,
) -> Result<(), AppearanceError> {
    match backdrop {
        WindowBackdrop::None => Ok(()),
        _ => Err(AppearanceError::UnsupportedBackdrop(backdrop)),
    }
}

fn apply_platform_appearance(window: &Window, appearance: knightty_render::WindowAppearance) {
    let platform = WinitPlatformAppearance { window };
    platform.set_window_opacity(appearance.opacity);

    let blur_enabled = appearance.blur && appearance.opacity < 1.0;
    if let Err(error) = platform.set_blur(blur_enabled, appearance.blur_radius) {
        eprintln!("knightty appearance: warning: {error}");
    }
    if let Err(error) = platform.set_backdrop(appearance.backdrop) {
        eprintln!("knightty appearance: warning: {error}");
    }
}

#[derive(Debug, Error)]
enum AppearanceError {
    #[error(
        "window backdrop `{0}` is not supported by the current platform backend; continuing without it"
    )]
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    UnsupportedBackdrop(WindowBackdrop),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImeCursorArea {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
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

fn ime_cursor_area_for_snapshot(
    snapshot: &GridSnapshot,
    metrics: CellMetrics,
    padding_x: u32,
    padding_y: u32,
) -> Option<ImeCursorArea> {
    if !snapshot.cursor.visible {
        return None;
    }

    Some(ImeCursorArea {
        x: padding_x.saturating_add((snapshot.cursor.x as u32).saturating_mul(metrics.width)),
        y: padding_y.saturating_add((snapshot.cursor.y as u32).saturating_mul(metrics.height)),
        width: metrics.width,
        height: metrics.height,
    })
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
                    graphics_diagnostics::record_pty_read(&buffer[..read]);
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
    Config(#[from] app::config::ConfigError),
    #[error("invalid wgpu backend `{0}`; expected auto, vulkan, dx12, or gl")]
    InvalidWgpuBackend(String),
    #[error("unknown action `{0}`; expected +list-fonts or +print-default-config")]
    UnknownAction(String),
    #[error(
        "unexpected argument `{0}`; run without arguments or use +list-fonts or +print-default-config"
    )]
    UnexpectedArgument(String),
}

#[cfg(test)]
mod tests {
    use super::{
        CellHitTest, ImeCursorArea, cell_position_for_pixel, format_font_list, format_window_title,
        ime_cursor_area_for_snapshot, pty_size_for_terminal,
    };
    use app::input::{
        ClickTracker, HoveredHyperlink, HyperlinkClickInput, HyperlinkClickRouting,
        HyperlinkUrlError, MouseDragRouting, PendingHyperlinkClick, ScrollDirection,
        ScrollShortcut, WheelRouting, allowed_hyperlink_url, hyperlink_cursor_icon,
        is_copy_shortcut, is_paste_shortcut, route_hyperlink_click, route_mouse_drag,
        route_mouse_wheel, scroll_shortcut_for_key, should_clear_simple_click_selection,
        should_open_pending_hyperlink,
    };
    use std::time::{Duration, Instant};

    use knightty_core::{
        Hyperlink, MouseButton as TerminalMouseButton, SelectionMode, SelectionPoint, Terminal,
    };
    use knightty_render::{CellMetrics, FontFamilyInfo};
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
    fn startup_action_accepts_print_default_config() {
        assert_eq!(
            super::startup_action_from_args(
                ["knightty", "+print-default-config"].map(str::to_owned)
            )
            .unwrap(),
            super::StartupAction::PrintDefaultConfig
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
    fn ime_cursor_area_uses_visible_cursor_metrics_and_padding() {
        let mut terminal = Terminal::new(10, 5);
        terminal.feed(b"\x1b[3;4H");
        let metrics = CellMetrics {
            width: 11,
            height: 23,
            font_size: 16.0,
            line_height: 23.0,
        };

        assert_eq!(
            ime_cursor_area_for_snapshot(&terminal.snapshot(), metrics, 5, 7),
            Some(ImeCursorArea {
                x: 38,
                y: 53,
                width: 11,
                height: 23,
            })
        );
    }

    #[test]
    fn ime_cursor_area_is_none_when_cursor_is_hidden() {
        let mut terminal = Terminal::new(10, 5);
        terminal.feed(b"\x1b[?25l");

        assert_eq!(
            ime_cursor_area_for_snapshot(&terminal.snapshot(), CellMetrics::default(), 0, 0),
            None
        );
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
