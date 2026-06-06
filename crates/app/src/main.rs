use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod config;

use config::{AppConfig, load_app_config, parse_wgpu_backend_name};
use knightty_core::{Damage, Terminal};
use knightty_pty::{PtySession, PtySize};
use knightty_render::{RenderError, Renderer, RendererConfig};
use thiserror::Error;
use wgpu::{Backends, Instance, InstanceDescriptor};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

fn main() -> Result<(), AppError> {
    let config = load_app_config()?;
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    event_loop.run_app(&mut Application::new(proxy, config))?;
    Ok(())
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
}

impl Application {
    fn new(proxy: EventLoopProxy<UserEvent>, config: AppConfig) -> Self {
        let renderer_config = config.renderer_config();
        let initial_cols = config.initial_cols();
        let initial_rows = config.initial_rows();
        Self {
            config,
            renderer_config,
            proxy,
            window_state: None,
            terminal: Terminal::new(initial_cols, initial_rows),
            pty: None,
            writer: None,
            modifiers: ModifiersState::empty(),
        }
    }

    fn start_pty(&mut self) -> Result<(), AppError> {
        if self.pty.is_some() {
            return Ok(());
        }

        let metrics = self
            .window_state
            .as_ref()
            .map(|state| state.renderer.cell_metrics())
            .unwrap_or(self.renderer_config.cell_metrics);
        let initial_cols = self.config.initial_cols();
        let initial_rows = self.config.initial_rows();
        let size = PtySize {
            rows: initial_rows as u16,
            cols: initial_cols as u16,
            pixel_width: (initial_cols as u32 * metrics.width).min(u16::MAX as u32) as u16,
            pixel_height: (initial_rows as u32 * metrics.height).min(u16::MAX as u32) as u16,
        };
        let mut pty = PtySession::spawn_default_shell(size)?;
        let reader = pty.take_reader()?;
        let writer = Arc::new(Mutex::new(pty.take_writer()?));
        spawn_pty_reader(reader, self.proxy.clone());

        self.writer = Some(writer);
        self.pty = Some(pty);
        Ok(())
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
            let _ = pty.resize(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: width.min(u16::MAX as u32) as u16,
                pixel_height: height.min(u16::MAX as u32) as u16,
            });
        }
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
        self.start_pty().expect("spawn default shell");
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
                if let Some(state) = &self.window_state {
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
                    if let Some(bytes) =
                        input_bytes(&event.logical_key, event.text.as_deref(), self.modifiers)
                    {
                        self.write_pty(&bytes);
                    }
                }
            }
            WindowEvent::Resized(size) => {
                self.resize_terminal(size.width, size.height);
                if let Some(state) = &mut self.window_state {
                    state.renderer.resize(size.width, size.height);
                    state.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.window_state {
                    let snapshot = self.terminal.snapshot();
                    let damage = self.terminal.take_damage();
                    if matches!(damage, Damage::None) {
                        return;
                    }
                    match state.renderer.render(&snapshot, &damage) {
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
}
