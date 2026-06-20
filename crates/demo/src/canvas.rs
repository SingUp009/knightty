use thiserror::Error;

use crate::raster::Point;

const MAX_LOGICAL_PIXELS: usize = 8_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum PaletteIndex {
    Background = 0,
    Foreground = 1,
    Accent = 2,
    MidTone = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    pub accent: Rgb,
    pub mid_tone: Rgb,
}

impl Palette {
    pub const fn color(self, index: PaletteIndex) -> Rgb {
        match index {
            PaletteIndex::Background => self.background,
            PaletteIndex::Foreground => self.foreground,
            PaletteIndex::Accent => self.accent,
            PaletteIndex::MidTone => self.mid_tone,
        }
    }
}

pub const DEFAULT_PALETTE: Palette = Palette {
    background: Rgb::new(0x11, 0x11, 0x1b),
    foreground: Rgb::new(0xcd, 0xd6, 0xf4),
    accent: Rgb::new(0xb4, 0xbe, 0xfe),
    mid_tone: Rgb::new(0x6c, 0x70, 0x86),
};

#[derive(Clone, Debug)]
pub struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<PaletteIndex>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, fill: PaletteIndex) -> Result<Self, CanvasError> {
        let len = checked_len(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![fill; len],
        })
    }

    pub fn resize(
        &mut self,
        width: usize,
        height: usize,
        fill: PaletteIndex,
    ) -> Result<(), CanvasError> {
        let len = checked_len(width, height)?;
        self.width = width;
        self.height = height;
        self.pixels.resize(len, fill);
        self.clear(fill);
        Ok(())
    }

    pub fn clear(&mut self, fill: PaletteIndex) {
        self.pixels.fill(fill);
    }

    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn set(&mut self, x: i32, y: i32, value: PaletteIndex) {
        let Some(index) = self.offset(x, y) else {
            return;
        };
        self.pixels[index] = value;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self, x: i32, y: i32) -> Option<PaletteIndex> {
        self.offset(x, y).map(|index| self.pixels[index])
    }

    pub fn pixel_at(&self, x: usize, y: usize) -> PaletteIndex {
        if x >= self.width || y >= self.height {
            return PaletteIndex::Background;
        }
        self.pixels[y * self.width + x]
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: i32, height: i32, value: PaletteIndex) {
        if width <= 0 || height <= 0 || self.is_empty() {
            return;
        }
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = x.saturating_add(width).clamp(0, self.width as i32) as usize;
        let y1 = y.saturating_add(height).clamp(0, self.height as i32) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for row in y0..y1 {
            let start = row * self.width + x0;
            let end = row * self.width + x1;
            self.pixels[start..end].fill(value);
        }
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, value: PaletteIndex) {
        if radius < 0 || self.is_empty() {
            return;
        }
        let radius_sq = radius.saturating_mul(radius);
        for y in cy.saturating_sub(radius)..=cy.saturating_add(radius) {
            for x in cx.saturating_sub(radius)..=cx.saturating_add(radius) {
                let dx = x.saturating_sub(cx);
                let dy = y.saturating_sub(cy);
                if dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)) <= radius_sq {
                    self.set(x, y, value);
                }
            }
        }
    }

    pub fn draw_line(&mut self, start: Point, end: Point, value: PaletteIndex) {
        let mut x0 = start.x.round() as i32;
        let mut y0 = start.y.round() as i32;
        let x1 = end.x.round() as i32;
        let y1 = end.y.round() as i32;

        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;

        loop {
            self.set(x0, y0, value);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = error.saturating_mul(2);
            if e2 >= dy {
                error += dy;
                x0 += sx;
            }
            if e2 <= dx {
                error += dx;
                y0 += sy;
            }
        }
    }

    pub fn draw_thick_line(
        &mut self,
        start: Point,
        end: Point,
        thickness: f32,
        value: PaletteIndex,
    ) {
        if thickness <= 1.0 {
            self.draw_line(start, end, value);
            return;
        }

        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length = (dx * dx + dy * dy).sqrt();
        if length <= f32::EPSILON {
            self.fill_circle(
                start.x.round() as i32,
                start.y.round() as i32,
                (thickness * 0.5).ceil() as i32,
                value,
            );
            return;
        }

        let nx = -dy / length * thickness * 0.5;
        let ny = dx / length * thickness * 0.5;
        let points = [
            Point::new(start.x + nx, start.y + ny),
            Point::new(end.x + nx, end.y + ny),
            Point::new(end.x - nx, end.y - ny),
            Point::new(start.x - nx, start.y - ny),
        ];
        self.fill_polygon(&points, value);
        let radius = (thickness * 0.5).ceil() as i32;
        self.fill_circle(
            start.x.round() as i32,
            start.y.round() as i32,
            radius,
            value,
        );
        self.fill_circle(end.x.round() as i32, end.y.round() as i32, radius, value);
    }

    pub fn fill_polygon(&mut self, points: &[Point], value: PaletteIndex) {
        if points.len() < 3
            || self.is_empty()
            || points
                .iter()
                .any(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            return;
        }

        let min_y = points
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i32;
        let max_y = points
            .iter()
            .map(|point| point.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(self.height.saturating_sub(1) as f32) as i32;

        if min_y > max_y {
            return;
        }

        let mut intersections = Vec::with_capacity(points.len());
        for y in min_y..=max_y {
            let scan_y = y as f32 + 0.5;
            intersections.clear();
            for index in 0..points.len() {
                let a = points[index];
                let b = points[(index + 1) % points.len()];
                let low_y = a.y.min(b.y);
                let high_y = a.y.max(b.y);
                if scan_y < low_y || scan_y >= high_y || (a.y - b.y).abs() <= f32::EPSILON {
                    continue;
                }
                let t = (scan_y - a.y) / (b.y - a.y);
                intersections.push(a.x + t * (b.x - a.x));
            }
            intersections.sort_by(f32::total_cmp);
            for pair in intersections.chunks_exact(2) {
                let x0 = pair[0].ceil().max(0.0) as i32;
                let x1 = pair[1].floor().min(self.width.saturating_sub(1) as f32) as i32;
                for x in x0..=x1 {
                    self.set(x, y, value);
                }
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn count(&self, value: PaletteIndex) -> usize {
        self.pixels.iter().filter(|pixel| **pixel == value).count()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn content_hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for pixel in &self.pixels {
            hash ^= *pixel as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    fn offset(&self, x: i32, y: i32) -> Option<usize> {
        let x = usize::try_from(x).ok()?;
        let y = usize::try_from(y).ok()?;
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(y * self.width + x)
    }
}

fn checked_len(width: usize, height: usize) -> Result<usize, CanvasError> {
    let Some(len) = width.checked_mul(height) else {
        return Err(CanvasError::TooLarge { width, height });
    };
    if len > MAX_LOGICAL_PIXELS {
        return Err(CanvasError::TooLarge { width, height });
    }
    Ok(len)
}

#[derive(Debug, Error)]
pub enum CanvasError {
    #[error("terminal canvas is too large: {width} x {height} logical pixels")]
    TooLarge { width: usize, height: usize },
}

#[cfg(test)]
mod tests {
    use super::{Canvas, PaletteIndex};
    use crate::raster::Point;

    #[test]
    fn canvas_set_and_get_work_for_valid_coordinates() {
        let mut canvas = Canvas::new(4, 3, PaletteIndex::Background).unwrap();
        canvas.set(2, 1, PaletteIndex::Accent);
        assert_eq!(canvas.get(2, 1), Some(PaletteIndex::Accent));
    }

    #[test]
    fn canvas_set_outside_bounds_does_not_panic_or_modify_canvas() {
        let mut canvas = Canvas::new(2, 2, PaletteIndex::Background).unwrap();
        canvas.set(-1, 0, PaletteIndex::Accent);
        canvas.set(2, 0, PaletteIndex::Accent);
        assert_eq!(canvas.count(PaletteIndex::Accent), 0);
    }

    #[test]
    fn canvas_clear_sets_every_pixel_to_the_requested_color() {
        let mut canvas = Canvas::new(3, 2, PaletteIndex::Foreground).unwrap();
        canvas.clear(PaletteIndex::Accent);
        assert_eq!(canvas.count(PaletteIndex::Accent), 6);
    }

    #[test]
    fn circle_fills_center_but_not_far_outside_point() {
        let mut canvas = Canvas::new(11, 11, PaletteIndex::Background).unwrap();
        canvas.fill_circle(5, 5, 3, PaletteIndex::Foreground);
        assert_eq!(canvas.get(5, 5), Some(PaletteIndex::Foreground));
        assert_eq!(canvas.get(0, 0), Some(PaletteIndex::Background));
    }

    #[test]
    fn line_draws_both_endpoints() {
        let mut canvas = Canvas::new(8, 8, PaletteIndex::Background).unwrap();
        canvas.draw_line(
            Point::new(1.0, 1.0),
            Point::new(6.0, 4.0),
            PaletteIndex::Foreground,
        );
        assert_eq!(canvas.get(1, 1), Some(PaletteIndex::Foreground));
        assert_eq!(canvas.get(6, 4), Some(PaletteIndex::Foreground));
    }

    #[test]
    fn polygon_fills_representative_inside_point() {
        let mut canvas = Canvas::new(10, 10, PaletteIndex::Background).unwrap();
        canvas.fill_polygon(
            &[
                Point::new(2.0, 2.0),
                Point::new(7.0, 2.0),
                Point::new(5.0, 7.0),
            ],
            PaletteIndex::Accent,
        );
        assert_eq!(canvas.get(5, 4), Some(PaletteIndex::Accent));
    }
}
