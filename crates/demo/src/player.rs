use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

use crate::animation::{Animation, AssetError, frame_index_for_elapsed, skipped_frames};
use crate::canvas::{Canvas, CanvasError, DEFAULT_PALETTE, PaletteIndex};
use crate::cli::DemoConfig;
use crate::encoder::FrameEncoder;
use crate::metrics::Metrics;
use crate::terminal::{self, TerminalGuard};

const MIN_COLS: u16 = 60;
const MIN_ROWS: u16 = 24;
const RESIZE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const PAUSED_SLEEP: Duration = Duration::from_millis(20);

pub fn run(config: DemoConfig) -> Result<Option<String>, DemoError> {
    if !terminal::stdout_is_tty() {
        return Err(DemoError::StdoutNotTty);
    }

    let mut guard = TerminalGuard::enter()?;
    let mut animation = Animation::new()?;
    let start = Instant::now();
    let mut stdout = io::stdout();
    let encoder = FrameEncoder::new(DEFAULT_PALETTE);
    let (cols, rows) = terminal::terminal_size()?;
    let mut screen = Screen::new(cols, rows)?;
    let mut output = Vec::with_capacity(screen.estimated_buffer_capacity());
    let mut metrics = Metrics::new(config.fps);
    let frame_duration = frame_duration(config.fps);
    let mut next_frame_index = 0_u64;
    let mut total_paused = Duration::ZERO;
    let mut pause_started = None;
    let mut frozen_elapsed = Duration::ZERO;
    let mut last_resize_poll = Instant::now();
    let mut needs_redraw = true;

    loop {
        let now = Instant::now();
        let event_result = process_events(&mut screen)?;
        if event_result.exit {
            break;
        }
        needs_redraw |= event_result.resized;

        if event_result.toggle_pause {
            if let Some(paused_at) = pause_started.take() {
                total_paused =
                    total_paused.saturating_add(now.saturating_duration_since(paused_at));
                if let Some(frame_duration) = frame_duration {
                    next_frame_index = frame_index_for_elapsed(
                        active_elapsed(start, total_paused, now).as_secs_f64(),
                        frame_duration.as_secs_f64(),
                    );
                }
                needs_redraw = true;
            } else {
                frozen_elapsed = active_elapsed(start, total_paused, now);
                pause_started = Some(now);
            }
        }

        if last_resize_poll.elapsed() >= RESIZE_POLL_INTERVAL {
            last_resize_poll = Instant::now();
            if let Ok((cols, rows)) = terminal::terminal_size() {
                needs_redraw |= screen.resize(cols, rows)?;
            }
        }

        let elapsed = if pause_started.is_some() {
            frozen_elapsed
        } else {
            active_elapsed(start, total_paused, Instant::now())
        };

        if let Some(duration) = config.duration
            && elapsed >= duration
        {
            break;
        }

        if pause_started.is_some() && !needs_redraw {
            thread::sleep(PAUSED_SLEEP);
            continue;
        }

        if let Some(frame_duration) = frame_duration {
            let target_frame =
                frame_index_for_elapsed(elapsed.as_secs_f64(), frame_duration.as_secs_f64());
            let dropped = skipped_frames(target_frame, next_frame_index);
            metrics.add_dropped_frames(dropped);
            if target_frame > next_frame_index {
                next_frame_index = target_frame;
            }
        }

        let frame_start = Instant::now();
        metrics.record_frame_start(frame_start);
        let encode_time = render_once(
            &mut animation,
            &mut screen,
            &encoder,
            elapsed,
            &mut output,
            &mut stdout,
        )?;
        metrics.record_frame(output.len(), encode_time);
        needs_redraw = false;

        if pause_started.is_some() {
            thread::sleep(PAUSED_SLEEP);
            continue;
        }

        if let Some(frame_duration) = frame_duration {
            next_frame_index = next_frame_index.saturating_add(1);
            let deadline_elapsed = frame_duration.mul_f64(next_frame_index as f64);
            let deadline = start + total_paused + deadline_elapsed;
            sleep_until(deadline);
        } else {
            thread::yield_now();
        }
    }

    let elapsed = pause_started
        .map(|_| frozen_elapsed)
        .unwrap_or_else(|| active_elapsed(start, total_paused, Instant::now()));
    guard.restore();

    if config.show_stats {
        Ok(Some(metrics.report(elapsed)))
    } else {
        Ok(None)
    }
}

fn render_once(
    animation: &mut Animation,
    screen: &mut Screen,
    encoder: &FrameEncoder,
    elapsed: Duration,
    output: &mut Vec<u8>,
    stdout: &mut io::Stdout,
) -> Result<Duration, DemoError> {
    if screen.too_small() {
        terminal::encode_small_terminal_message(screen.cols, screen.rows, output);
        stdout.write_all(output)?;
        stdout.flush()?;
        return Ok(Duration::ZERO);
    }

    output.clear();
    terminal::push_cursor_home(output);
    animation.render_frame(&mut screen.canvas, elapsed.as_secs_f32());
    let encode_start = Instant::now();
    encoder.encode(&screen.canvas, output);
    let encode_time = encode_start.elapsed();
    stdout.write_all(output)?;
    stdout.flush()?;
    Ok(encode_time)
}

fn process_events(screen: &mut Screen) -> Result<EventResult, DemoError> {
    let mut result = EventResult::default();
    while event::poll(Duration::ZERO)? {
        match event::read()? {
            Event::Key(key) if is_exit_key(key) => result.exit = true,
            Event::Key(key) if key.code == KeyCode::Char(' ') => result.toggle_pause = true,
            Event::Resize(cols, rows) => {
                result.resized |= screen.resize(cols, rows)?;
            }
            _ => {}
        }
    }
    Ok(result)
}

fn is_exit_key(key: KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q')
    ) || (matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && key.modifiers.contains(KeyModifiers::CONTROL))
}

fn frame_duration(fps: u32) -> Option<Duration> {
    (fps > 0).then(|| Duration::from_secs_f64(1.0 / fps as f64))
}

fn active_elapsed(start: Instant, total_paused: Duration, now: Instant) -> Duration {
    now.saturating_duration_since(start)
        .saturating_sub(total_paused)
}

fn sleep_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(now);
        if remaining > Duration::from_millis(2) {
            thread::sleep(remaining - Duration::from_millis(1));
        } else {
            thread::yield_now();
        }
    }
}

#[derive(Default)]
struct EventResult {
    exit: bool,
    toggle_pause: bool,
    resized: bool,
}

struct Screen {
    cols: u16,
    rows: u16,
    canvas: Canvas,
}

impl Screen {
    fn new(cols: u16, rows: u16) -> Result<Self, DemoError> {
        let (width, height) = logical_canvas_size(cols, rows);
        Ok(Self {
            cols,
            rows,
            canvas: Canvas::new(width, height, PaletteIndex::Background)?,
        })
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<bool, DemoError> {
        if self.cols == cols && self.rows == rows {
            return Ok(false);
        }
        self.cols = cols;
        self.rows = rows;
        let (width, height) = logical_canvas_size(cols, rows);
        self.canvas
            .resize(width, height, PaletteIndex::Background)?;
        Ok(true)
    }

    fn too_small(&self) -> bool {
        self.cols < MIN_COLS || self.rows < MIN_ROWS
    }

    fn estimated_buffer_capacity(&self) -> usize {
        usize::from(self.cols)
            .saturating_mul(usize::from(self.rows))
            .saturating_mul(32)
            .max(4096)
    }
}

fn logical_canvas_size(cols: u16, rows: u16) -> (usize, usize) {
    (
        usize::from(cols).saturating_sub(1),
        usize::from(rows).saturating_mul(2),
    )
}

#[derive(Debug, Error)]
pub enum DemoError {
    #[error("stdout is not a TTY; run this demo from an interactive terminal")]
    StdoutNotTty,
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("canvas allocation failed: {0}")]
    Canvas(#[from] CanvasError),
    #[error("animation asset failed: {0}")]
    Asset(#[from] AssetError),
}
