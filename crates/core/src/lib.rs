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

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
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
    pending_compat_input: Vec<u8>,
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
        let size = TermSize::new(cols.max(1), rows.max(1));
        let pty_writes = Arc::new(Mutex::new(Vec::new()));
        let event_sink = TerminalEventSink {
            pty_writes: Arc::clone(&pty_writes),
        };
        let mut term = Term::new(Config::default(), &size, event_sink);
        term.reset_damage();

        Self {
            term,
            processor: Processor::new(),
            damage: Damage::Full,
            pty_writes,
            pending_compat_input: Vec::new(),
        }
    }

    /// Resize the visible grid.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let size = TermSize::new(cols.max(1), rows.max(1));
        self.term.resize(size);
        self.damage = Damage::Full;
        self.term.reset_damage();
    }

    /// Return the current visible grid size as `(cols, rows)`.
    pub fn size(&self) -> (usize, usize) {
        let grid = self.term.grid();
        (grid.columns(), grid.screen_lines())
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

        let mut input = core::mem::take(&mut self.pending_compat_input);
        input.extend_from_slice(bytes);

        let pending_start = self.process_compat_input(&input);
        if pending_start < input.len() {
            self.pending_compat_input
                .extend_from_slice(&input[pending_start..]);
        }

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

    /// Return a render-safe copy of the visible grid.
    pub fn snapshot(&self) -> GridSnapshot {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let mut cells = vec![Cell::default(); cols * rows];

        for indexed in grid.display_iter() {
            let line = indexed.point.line.0;
            if line < 0 {
                continue;
            }

            let x = indexed.point.column.0;
            let y = line as usize;
            if x >= cols || y >= rows {
                continue;
            }

            cells[y * cols + x] = Cell::from_alacritty(&indexed.cell);
        }

        let cursor_point = grid.cursor.point;
        let cursor_x = cursor_point.column.0;
        let cursor_y = usize::try_from(cursor_point.line.0).unwrap_or(rows);
        let cursor_visible =
            self.term.mode().contains(TermMode::SHOW_CURSOR) && cursor_x < cols && cursor_y < rows;

        GridSnapshot {
            cols,
            rows,
            cells,
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
        }
    }
}

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

const ESC: u8 = 0x1b;
const CSI: u8 = 0x9b;
const MAX_COMPAT_CSI_LEN: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompatAction {
    SetAltScreen,
    ResetAltScreen,
    SaveCursor,
    RestoreCursor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

    let mode = params.strip_prefix(b"?").and_then(parse_private_mode)?;
    match (mode, final_byte) {
        (1047, b'h') => Some(CompatAction::SetAltScreen),
        (1047, b'l') => Some(CompatAction::ResetAltScreen),
        (1048, b'h') => Some(CompatAction::SaveCursor),
        (1048, b'l') => Some(CompatAction::RestoreCursor),
        _ => None,
    }
}

fn parse_private_mode(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }

    core::str::from_utf8(bytes).ok()?.parse().ok()
}

#[derive(Clone)]
struct TerminalEventSink {
    pty_writes: Arc<Mutex<Vec<String>>>,
}

impl EventListener for TerminalEventSink {
    fn send_event(&self, event: Event) {
        let Event::PtyWrite(text) = event else {
            return;
        };

        if let Ok(mut pty_writes) = self.pty_writes.lock() {
            pty_writes.push(text);
        }
    }
}

/// Render-safe visible terminal grid snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Cursor,
}

impl GridSnapshot {
    pub fn cell(&self, x: usize, y: usize) -> &Cell {
        &self.cells[y * self.cols + x]
    }

    pub fn lines(&self) -> impl Iterator<Item = &[Cell]> {
        self.cells.chunks(self.cols)
    }
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
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            flags: CellFlags::default(),
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
    use super::{Color, Damage, Terminal};

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
                        .any(|line| line.line == 0 && line.left <= 0 && line.right >= 5)
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

    fn line_text(grid: &super::GridSnapshot, row: usize) -> String {
        (0..grid.cols)
            .map(|column| grid.cell(column, row).ch)
            .collect()
    }
}
