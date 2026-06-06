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
        }
    }

    /// Resize the visible grid.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let size = TermSize::new(cols.max(1), rows.max(1));
        self.term.resize(size);
        self.damage = Damage::Full;
        self.term.reset_damage();
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

        self.processor.advance(&mut self.term, bytes);
        let damage = self.collect_damage();
        self.damage = merge_damage(core::mem::replace(&mut self.damage, Damage::None), damage);
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
}
