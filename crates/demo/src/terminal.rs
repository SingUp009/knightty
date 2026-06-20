use std::io::{self, IsTerminal, Write};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};

pub const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h";
pub const EXIT_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049l";
pub const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
pub const SHOW_CURSOR: &[u8] = b"\x1b[?25h";
pub const CLEAR_SCREEN: &[u8] = b"\x1b[2J";
pub const CURSOR_HOME: &[u8] = b"\x1b[H";
pub const DISABLE_AUTOWRAP: &[u8] = b"\x1b[?7l";
pub const ENABLE_AUTOWRAP: &[u8] = b"\x1b[?7h";
pub const RESET_ATTRIBUTES: &[u8] = b"\x1b[0m";

#[derive(Debug)]
pub struct TerminalGuard {
    restored: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut guard = Self { restored: false };
        let setup_result = (|| {
            let mut stdout = io::stdout();
            stdout.write_all(ENTER_ALTERNATE_SCREEN)?;
            stdout.write_all(HIDE_CURSOR)?;
            stdout.write_all(DISABLE_AUTOWRAP)?;
            stdout.write_all(CLEAR_SCREEN)?;
            stdout.write_all(CURSOR_HOME)?;
            stdout.flush()
        })();
        if let Err(error) = setup_result {
            guard.restore();
            return Err(error);
        }
        Ok(guard)
    }

    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = stdout.write_all(RESET_ATTRIBUTES);
        let _ = stdout.write_all(ENABLE_AUTOWRAP);
        let _ = stdout.write_all(SHOW_CURSOR);
        let _ = stdout.write_all(EXIT_ALTERNATE_SCREEN);
        let _ = stdout.flush();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub fn stdout_is_tty() -> bool {
    io::stdout().is_terminal()
}

pub fn terminal_size() -> io::Result<(u16, u16)> {
    size()
}

pub fn push_cursor_home(output: &mut Vec<u8>) {
    output.extend_from_slice(CURSOR_HOME);
}

pub fn push_clear(output: &mut Vec<u8>) {
    output.extend_from_slice(CLEAR_SCREEN);
    output.extend_from_slice(CURSOR_HOME);
}

pub fn encode_small_terminal_message(cols: u16, rows: u16, output: &mut Vec<u8>) {
    use std::fmt::Write as _;

    output.clear();
    push_clear(output);

    let lines = [
        "Knightty Demo",
        "",
        "Terminal is too small.",
        "Resize to at least 60 x 24.",
        "",
        "Press q or Esc to exit.",
    ];
    let cols = usize::from(cols);
    let rows = usize::from(rows);
    let start_row = rows.saturating_sub(lines.len()) / 2 + 1;

    for (index, line) in lines.iter().enumerate() {
        if start_row + index > rows || cols == 0 {
            break;
        }
        let visible: String = line.chars().take(cols).collect();
        let col = cols.saturating_sub(visible.chars().count()) / 2 + 1;
        write!(
            ByteSink(output),
            "\x1b[{};{}H{}",
            start_row + index,
            col,
            visible
        )
        .expect("writing to Vec cannot fail");
    }
}

struct ByteSink<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for ByteSink<'_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.0.extend_from_slice(value.as_bytes());
        Ok(())
    }
}
