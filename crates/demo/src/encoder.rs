use std::fmt::Write as _;

use crate::canvas::{Canvas, Palette, PaletteIndex, Rgb};

const UPPER_HALF_BLOCK: &str = "▀";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellEncoding {
    HalfBlock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateMode {
    Full,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameEncoder {
    palette: Palette,
    encoding: CellEncoding,
    update_mode: UpdateMode,
}

impl FrameEncoder {
    pub const fn new(palette: Palette) -> Self {
        Self {
            palette,
            encoding: CellEncoding::HalfBlock,
            update_mode: UpdateMode::Full,
        }
    }

    pub fn encode(&self, canvas: &Canvas, output: &mut Vec<u8>) {
        match (self.encoding, self.update_mode) {
            (CellEncoding::HalfBlock, UpdateMode::Full) => {
                self.encode_half_block_full(canvas, output)
            }
        }
    }

    fn encode_half_block_full(&self, canvas: &Canvas, output: &mut Vec<u8>) {
        let terminal_rows = canvas.height().div_ceil(2);
        let mut current_fg: Option<PaletteIndex> = None;
        let mut current_bg: Option<PaletteIndex> = None;

        for row in 0..terminal_rows {
            if row > 0 {
                write!(ByteSink(output), "\x1b[{};1H", row + 1)
                    .expect("writing to Vec cannot fail");
            }

            for x in 0..canvas.width() {
                let top = canvas.pixel_at(x, row * 2);
                let bottom = if row * 2 + 1 < canvas.height() {
                    canvas.pixel_at(x, row * 2 + 1)
                } else {
                    PaletteIndex::Background
                };

                if top == bottom {
                    self.set_bg_if_needed(top, &mut current_bg, output);
                    output.push(b' ');
                } else {
                    self.set_fg_bg_if_needed(top, bottom, &mut current_fg, &mut current_bg, output);
                    output.extend_from_slice(UPPER_HALF_BLOCK.as_bytes());
                }
            }
        }
    }

    fn set_bg_if_needed(
        &self,
        bg: PaletteIndex,
        current_bg: &mut Option<PaletteIndex>,
        output: &mut Vec<u8>,
    ) {
        if *current_bg == Some(bg) {
            return;
        }
        push_bg_sgr(self.palette.color(bg), output);
        *current_bg = Some(bg);
    }

    fn set_fg_bg_if_needed(
        &self,
        fg: PaletteIndex,
        bg: PaletteIndex,
        current_fg: &mut Option<PaletteIndex>,
        current_bg: &mut Option<PaletteIndex>,
        output: &mut Vec<u8>,
    ) {
        if *current_fg == Some(fg) && *current_bg == Some(bg) {
            return;
        }
        push_fg_bg_sgr(self.palette.color(fg), self.palette.color(bg), output);
        *current_fg = Some(fg);
        *current_bg = Some(bg);
    }
}

fn push_bg_sgr(bg: Rgb, output: &mut Vec<u8>) {
    write!(ByteSink(output), "\x1b[48;2;{};{};{}m", bg.r, bg.g, bg.b)
        .expect("writing to Vec cannot fail");
}

fn push_fg_bg_sgr(fg: Rgb, bg: Rgb, output: &mut Vec<u8>) {
    write!(
        ByteSink(output),
        "\x1b[38;2;{};{};{};48;2;{};{};{}m",
        fg.r,
        fg.g,
        fg.b,
        bg.r,
        bg.g,
        bg.b
    )
    .expect("writing to Vec cannot fail");
}

struct ByteSink<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for ByteSink<'_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.0.extend_from_slice(value.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameEncoder, UPPER_HALF_BLOCK};
    use crate::canvas::{Canvas, DEFAULT_PALETTE, PaletteIndex};

    #[test]
    fn encoder_outputs_space_when_top_and_bottom_have_the_same_color() {
        let canvas = Canvas::new(1, 2, PaletteIndex::Foreground).unwrap();
        let mut output = Vec::new();
        FrameEncoder::new(DEFAULT_PALETTE).encode(&canvas, &mut output);
        let text = String::from_utf8(output).unwrap();
        assert!(text.ends_with(' '));
        assert!(!text.contains(UPPER_HALF_BLOCK));
    }

    #[test]
    fn encoder_outputs_half_block_when_top_and_bottom_differ() {
        let mut canvas = Canvas::new(1, 2, PaletteIndex::Background).unwrap();
        canvas.set(0, 0, PaletteIndex::Foreground);
        let mut output = Vec::new();
        FrameEncoder::new(DEFAULT_PALETTE).encode(&canvas, &mut output);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains(UPPER_HALF_BLOCK));
    }

    #[test]
    fn encoder_treats_odd_height_missing_bottom_pixel_as_background() {
        let mut canvas = Canvas::new(1, 1, PaletteIndex::Background).unwrap();
        canvas.set(0, 0, PaletteIndex::Accent);
        let mut output = Vec::new();
        FrameEncoder::new(DEFAULT_PALETTE).encode(&canvas, &mut output);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains(UPPER_HALF_BLOCK));
    }

    #[test]
    fn encoder_accepts_empty_canvas() {
        let canvas = Canvas::new(0, 0, PaletteIndex::Background).unwrap();
        let mut output = Vec::new();
        FrameEncoder::new(DEFAULT_PALETTE).encode(&canvas, &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn encoder_reuses_sgr_for_same_color_run() {
        let mut canvas = Canvas::new(4, 2, PaletteIndex::Background).unwrap();
        for x in 0..4 {
            canvas.set(x, 0, PaletteIndex::Foreground);
        }
        let mut output = Vec::new();
        FrameEncoder::new(DEFAULT_PALETTE).encode(&canvas, &mut output);
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.matches("\x1b[38;2").count(), 1);
        assert_eq!(text.matches(UPPER_HALF_BLOCK).count(), 4);
    }

    #[test]
    fn encoder_emits_utf8_half_block() {
        let mut canvas = Canvas::new(1, 2, PaletteIndex::Background).unwrap();
        canvas.set(0, 0, PaletteIndex::Foreground);
        let mut output = Vec::new();
        FrameEncoder::new(DEFAULT_PALETTE).encode(&canvas, &mut output);
        assert!(
            output
                .windows(UPPER_HALF_BLOCK.len())
                .any(|window| window == UPPER_HALF_BLOCK.as_bytes())
        );
    }
}
