use crate::animation::{
    LOOP_DURATION_SECONDS, clamp01, ease_in_cubic, ease_in_out_cubic, ease_out_cubic, lerp,
    segment_t, smoothstep,
};
use crate::canvas::{Canvas, PaletteIndex};
use crate::raster::{Point, hash_unit, stable_hash};

use super::asset::{AssetFrame, CapeLayerId, KfaAsset};

const REFERENCE_WIDTH: f32 = 160.0;
const REFERENCE_HEIGHT: f32 = 90.0;
const BAYER_4X4: [[f32; 4]; 4] = [
    [0.0, 0.5, 0.125, 0.625],
    [0.75, 0.25, 0.875, 0.375],
    [0.1875, 0.6875, 0.0625, 0.5625],
    [0.9375, 0.4375, 0.8125, 0.3125],
];

pub fn render(canvas: &mut Canvas, asset: &KfaAsset, buffers: &mut CapeBuffers, t: f32) {
    canvas.clear(PaletteIndex::Background);
    if canvas.is_empty() {
        return;
    }

    let stage = Stage::for_canvas(canvas);
    if stage.scale <= 0.0 {
        return;
    }

    let seconds = t * LOOP_DURATION_SECONDS;
    let state = TimelineState::at(seconds);

    draw_far_background(canvas, stage, state);
    draw_mid_background(canvas, stage, state);
    draw_cape_layer(canvas, stage, asset, buffers, state, CapeLayerId::CapeFar);
    draw_cape_layer(canvas, stage, asset, buffers, state, CapeLayerId::RibbonFar);
    draw_cape_layer(canvas, stage, asset, buffers, state, CapeLayerId::CapeMain);
    draw_cape_layer(canvas, stage, asset, buffers, state, CapeLayerId::CapeLower);

    if let Some(frame) = asset.frame(state.pose_name) {
        draw_asset_frame(
            canvas,
            stage,
            frame,
            asset.width(),
            asset.height(),
            state.camera,
            1.0,
            1.0,
        );
    }

    draw_cape_layer(canvas, stage, asset, buffers, state, CapeLayerId::CapeNear);
    draw_cape_layer(
        canvas,
        stage,
        asset,
        buffers,
        state,
        CapeLayerId::RibbonNear,
    );

    if state.slash > 0.0 {
        draw_slash(canvas, stage, state);
    }

    if let Some(logo) = asset.frame("logo") {
        if state.logo_reveal > 0.0 {
            draw_logo(canvas, stage, logo, asset.width(), state);
        }
        if state.dissolve > 0.0 {
            draw_logo_particles(canvas, stage, logo, asset.width(), state);
        }
    }

    draw_foreground(canvas, stage, state);
}

#[derive(Debug)]
pub struct CapeBuffers {
    reference_points: Vec<Point>,
    screen_points: Vec<Point>,
}

impl CapeBuffers {
    pub fn for_asset(asset: &KfaAsset) -> Self {
        let capacity = asset.max_cape_vertices().max(12);
        Self {
            reference_points: Vec::with_capacity(capacity),
            screen_points: Vec::with_capacity(capacity),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimelineState {
    #[cfg_attr(not(test), allow(dead_code))]
    pub scene: Scene,
    pub pose_name: &'static str,
    pub camera: Camera,
    pub cape_motion: CapeMotionState,
    pub logo_reveal: f32,
    pub dissolve: f32,
    pub particle_count: usize,
    pub slash: f32,
}

impl TimelineState {
    pub fn at(seconds: f32) -> Self {
        let scene = Scene::at(seconds);
        let slash = segment_t(seconds, 3.72, 3.86);
        let dissolve = segment_t(seconds, 6.20, 8.00);
        let logo_reveal = if seconds < 4.70 {
            0.0
        } else if seconds < 6.20 {
            smoothstep(segment_t(seconds, 4.70, 5.22))
        } else {
            1.0 - smoothstep(dissolve)
        };

        Self {
            scene,
            pose_name: pose_name(seconds),
            camera: Camera::at(seconds),
            cape_motion: CapeMotionState { seconds },
            logo_reveal,
            dissolve,
            particle_count: if seconds >= 6.20 {
                (128.0 * smoothstep(dissolve)) as usize
            } else if seconds >= 3.72 {
                (48.0 * smoothstep(segment_t(seconds, 3.72, 4.70))) as usize
            } else {
                0
            },
            slash,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CapeMotionState {
    seconds: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scene {
    DistantMoon,
    CloseUp,
    HandOnHilt,
    Anticipation,
    Slash,
    FollowThrough,
    Knightty,
    Dissolve,
}

impl Scene {
    fn at(seconds: f32) -> Self {
        match seconds {
            value if value < 1.15 => Self::DistantMoon,
            value if value < 2.45 => Self::CloseUp,
            value if value < 3.30 => Self::HandOnHilt,
            value if value < 3.72 => Self::Anticipation,
            value if value < 3.86 => Self::Slash,
            value if value < 4.70 => Self::FollowThrough,
            value if value < 6.20 => Self::Knightty,
            _ => Self::Dissolve,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    position: Point,
    zoom: f32,
    shake: Point,
}

impl Camera {
    fn at(seconds: f32) -> Self {
        let base = match Scene::at(seconds) {
            Scene::DistantMoon => {
                let p = ease_out_cubic(segment_t(seconds, 0.0, 1.15));
                Self::new(
                    lerp(80.0, 75.0, p),
                    lerp(45.0, 43.5, p),
                    lerp(0.94, 1.08, p),
                )
            }
            Scene::CloseUp => {
                let p = ease_in_out_cubic(segment_t(seconds, 1.15, 2.45));
                Self::new(lerp(58.0, 61.0, p), lerp(43.0, 44.0, p), 1.62)
            }
            Scene::HandOnHilt => {
                let p = smoothstep(segment_t(seconds, 2.45, 3.30));
                Self::new(lerp(61.0, 65.0, p), 45.0, lerp(1.55, 1.42, p))
            }
            Scene::Anticipation => Self::new(66.0, 45.0, 1.38),
            Scene::Slash => Self::new(72.0, 45.0, 1.24),
            Scene::FollowThrough => {
                let p = smoothstep(segment_t(seconds, 3.86, 4.70));
                Self::new(lerp(75.0, 80.0, p), 45.0, lerp(1.18, 1.0, p))
            }
            Scene::Knightty => Self::new(80.0, 45.0, 1.0),
            Scene::Dissolve => {
                let p = smoothstep(segment_t(seconds, 6.20, 8.00));
                Self::new(lerp(80.0, 79.0, p), lerp(45.0, 44.0, p), lerp(1.0, 0.96, p))
            }
        };

        if matches!(Scene::at(seconds), Scene::Slash) {
            let pulse = 1.0 - (segment_t(seconds, 3.72, 3.86) - 0.5).abs() * 2.0;
            let shake = Point::new(
                (seconds * 91.0).sin() * 2.4 * pulse,
                (seconds * 73.0).cos() * 1.6 * pulse,
            );
            Self { shake, ..base }
        } else {
            base
        }
    }

    const fn new(x: f32, y: f32, zoom: f32) -> Self {
        Self {
            position: Point::new(x, y),
            zoom,
            shake: Point::new(0.0, 0.0),
        }
    }
}

#[derive(Clone, Copy)]
struct Stage {
    scale: f32,
    origin_x: f32,
    origin_y: f32,
}

impl Stage {
    fn for_canvas(canvas: &Canvas) -> Self {
        let width = canvas.width() as f32;
        let height = canvas.height() as f32;
        let scale = (width / REFERENCE_WIDTH).min(height / REFERENCE_HEIGHT);
        let scaled_width = REFERENCE_WIDTH * scale;
        let scaled_height = REFERENCE_HEIGHT * scale;
        Self {
            scale,
            origin_x: (width - scaled_width) * 0.5,
            origin_y: (height - scaled_height) * 0.5,
        }
    }

    fn point(self, x: f32, y: f32, camera: Camera, parallax: f32) -> Point {
        let camera_x = (camera.position.x - REFERENCE_WIDTH * 0.5) * parallax;
        let camera_y = (camera.position.y - REFERENCE_HEIGHT * 0.5) * parallax;
        let x = (x - camera_x - REFERENCE_WIDTH * 0.5) * camera.zoom
            + REFERENCE_WIDTH * 0.5
            + camera.shake.x;
        let y = (y - camera_y - REFERENCE_HEIGHT * 0.5) * camera.zoom
            + REFERENCE_HEIGHT * 0.5
            + camera.shake.y;
        Point::new(
            self.origin_x + x * self.scale,
            self.origin_y + y * self.scale,
        )
    }

    fn scalar(self, value: f32, camera: Camera) -> f32 {
        (value * self.scale * camera.zoom).max(1.0)
    }
}

fn pose_name(seconds: f32) -> &'static str {
    match Scene::at(seconds) {
        Scene::DistantMoon => held(seconds, 0.0, &["idle_a", "idle_a", "idle_b"]),
        Scene::CloseUp => held(seconds, 1.15, &["idle_b", "enter_b", "enter_c"]),
        Scene::HandOnHilt => held(seconds, 2.45, &["reach", "draw_a", "draw_b"]),
        Scene::Anticipation => "anticipation",
        Scene::Slash => "slash_smear",
        Scene::FollowThrough => held(seconds, 3.86, &["follow_a", "follow_a", "follow_b"]),
        Scene::Knightty | Scene::Dissolve => "follow_b",
    }
}

fn held(seconds: f32, start: f32, frames: &'static [&'static str]) -> &'static str {
    let index = ((seconds - start).max(0.0) * 13.0).floor() as usize;
    frames[index.min(frames.len().saturating_sub(1))]
}

fn draw_cape_layer(
    canvas: &mut Canvas,
    stage: Stage,
    asset: &KfaAsset,
    buffers: &mut CapeBuffers,
    state: TimelineState,
    layer_id: CapeLayerId,
) {
    let Some(layer_index) = asset.cape_layer_index(layer_id) else {
        return;
    };
    let Some(layer) = asset.cape_layers().get(layer_index).copied() else {
        return;
    };
    let Some(palette) = layer.color().to_palette() else {
        return;
    };
    if !sample_cape_layer_reference(asset, buffers, state.cape_motion, layer_id, layer_index) {
        return;
    }

    buffers.screen_points.clear();
    for point in &buffers.reference_points {
        buffers
            .screen_points
            .push(stage.point(point.x, point.y, state.camera, 1.0));
    }
    canvas.fill_polygon(&buffers.screen_points, palette);
}

fn sample_cape_layer_reference(
    asset: &KfaAsset,
    buffers: &mut CapeBuffers,
    motion: CapeMotionState,
    layer_id: CapeLayerId,
    layer_index: usize,
) -> bool {
    let Some(first_pose) = asset.cape_pose(CAPE_KEYS[0].name) else {
        return false;
    };
    let Some(vertices) = first_pose.layer_vertices(layer_index) else {
        return false;
    };
    let vertex_count = vertices.len();
    if vertex_count < 3 {
        return false;
    }

    buffers.reference_points.clear();
    for vertex_index in 0..vertex_count {
        let weight = vertex_weight(vertex_index, vertex_count);
        let delay = lerp(0.08, layer_tip_delay(layer_id), weight);
        let local_seconds = (motion.seconds - delay).max(0.0);
        let key = cape_key_pair(local_seconds);
        let Some(from_pose) = asset.cape_pose(key.from) else {
            return false;
        };
        let Some(to_pose) = asset.cape_pose(key.to) else {
            return false;
        };
        let Some(from_vertices) = from_pose.layer_vertices(layer_index) else {
            return false;
        };
        let Some(to_vertices) = to_pose.layer_vertices(layer_index) else {
            return false;
        };
        let Some(a) = from_vertices.get(vertex_index).copied() else {
            return false;
        };
        let Some(b) = to_vertices.get(vertex_index).copied() else {
            return false;
        };
        let eased = if key.allow_overshoot {
            overshoot(key.progress, 0.36)
        } else {
            smoothstep(key.progress)
        };
        let mut point = Point::new(lerp(a.x, b.x, eased), lerp(a.y, b.y, eased));
        apply_secondary_cape_motion(&mut point, motion.seconds, layer_id, weight);
        buffers.reference_points.push(point);
    }
    true
}

fn vertex_weight(index: usize, count: usize) -> f32 {
    if count <= 1 {
        return 0.0;
    }
    let raw = index as f32 / (count - 1) as f32;
    raw.powf(0.72).clamp(0.0, 1.0)
}

fn layer_tip_delay(layer_id: CapeLayerId) -> f32 {
    match layer_id {
        CapeLayerId::CapeMain => 0.28,
        CapeLayerId::CapeNear => 0.34,
        CapeLayerId::CapeLower => 0.36,
        CapeLayerId::CapeFar => 0.42,
        CapeLayerId::RibbonFar | CapeLayerId::RibbonNear => 0.52,
    }
}

fn apply_secondary_cape_motion(
    point: &mut Point,
    seconds: f32,
    layer_id: CapeLayerId,
    weight: f32,
) {
    let settle = damped_settle(segment_t(seconds, 4.15, 5.55), 2.6, 3.8);
    let rebound = smoothstep(segment_t(seconds, 4.30, 4.85))
        * (1.0 - smoothstep(segment_t(seconds, 5.20, 5.90)));
    let (sx, sy) = match layer_id {
        CapeLayerId::CapeFar => (-4.5, 3.0),
        CapeLayerId::CapeMain => (-3.2, 2.0),
        CapeLayerId::CapeNear => (4.0, -2.5),
        CapeLayerId::CapeLower => (-2.4, 4.2),
        CapeLayerId::RibbonFar => (-7.0, 4.8),
        CapeLayerId::RibbonNear => (7.5, -3.5),
    };
    point.x += (settle * sx + rebound * sx * 0.25) * weight;
    point.y += (settle * sy + rebound * sy * 0.20) * weight;
}

fn overshoot(t: f32, amount: f32) -> f32 {
    let t = clamp01(t) - 1.0;
    let c1 = amount.max(0.0);
    let c3 = c1 + 1.0;
    1.0 + c3 * t * t * t + c1 * t * t
}

fn damped_settle(t: f32, frequency: f32, decay: f32) -> f32 {
    let t = clamp01(t);
    (t * frequency * std::f32::consts::TAU).sin() * (-decay * t).exp()
}

#[derive(Clone, Copy)]
struct CapeKey {
    seconds: f32,
    name: &'static str,
}

#[derive(Clone, Copy)]
struct CapeKeyPair {
    from: &'static str,
    to: &'static str,
    progress: f32,
    allow_overshoot: bool,
}

const CAPE_KEYS: &[CapeKey] = &[
    CapeKey {
        seconds: 0.00,
        name: "cape_idle_a",
    },
    CapeKey {
        seconds: 0.80,
        name: "cape_idle_b",
    },
    CapeKey {
        seconds: 2.45,
        name: "cape_pull_back",
    },
    CapeKey {
        seconds: 3.30,
        name: "cape_anticipation",
    },
    CapeKey {
        seconds: 3.72,
        name: "cape_slash_hold",
    },
    CapeKey {
        seconds: 3.90,
        name: "cape_whip_forward",
    },
    CapeKey {
        seconds: 4.30,
        name: "cape_overshoot",
    },
    CapeKey {
        seconds: 4.70,
        name: "cape_rebound",
    },
    CapeKey {
        seconds: 5.30,
        name: "cape_settle_a",
    },
    CapeKey {
        seconds: 7.55,
        name: "cape_settle_b",
    },
    CapeKey {
        seconds: 8.00,
        name: "cape_idle_a",
    },
];

fn cape_key_pair(seconds: f32) -> CapeKeyPair {
    let seconds = seconds.clamp(0.0, LOOP_DURATION_SECONDS);
    for window in CAPE_KEYS.windows(2) {
        let from = window[0];
        let to = window[1];
        if seconds <= to.seconds {
            let progress = segment_t(seconds, from.seconds, to.seconds);
            return CapeKeyPair {
                from: from.name,
                to: to.name,
                progress,
                allow_overshoot: matches!(
                    (from.name, to.name),
                    ("cape_slash_hold", "cape_whip_forward")
                        | ("cape_whip_forward", "cape_overshoot")
                ),
            };
        }
    }
    CapeKeyPair {
        from: "cape_idle_a",
        to: "cape_idle_a",
        progress: 1.0,
        allow_overshoot: false,
    }
}

fn draw_far_background(canvas: &mut Canvas, stage: Stage, state: TimelineState) {
    let camera = state.camera;
    for index in 0..52_u32 {
        let x = 3.0 + hash_unit(index.wrapping_mul(17)) * 154.0;
        let y = 3.0 + hash_unit(index.wrapping_mul(31)) * 39.0;
        let threshold = 0.60 + 0.18 * (state.dissolve * std::f32::consts::TAU).sin().abs();
        if hash_unit(index ^ 0x51) < threshold {
            let p = stage.point(x, y, camera, 0.10);
            canvas.set(
                p.x.round() as i32,
                p.y.round() as i32,
                PaletteIndex::MidTone,
            );
        }
    }

    draw_dithered_ref_circle(
        canvas,
        stage,
        Point::new(124.0, 21.0),
        12.0,
        0.92,
        PaletteIndex::Foreground,
        camera,
        0.10,
        0x41,
    );
    draw_dithered_ref_circle(
        canvas,
        stage,
        Point::new(128.0, 18.0),
        5.6,
        1.0,
        PaletteIndex::Background,
        camera,
        0.10,
        0x92,
    );

    for cloud in 0..6_u32 {
        let drift = state.dissolve * 3.0 + cloud as f32 * 4.7;
        let x = 8.0 + cloud as f32 * 29.0 + drift.sin() * 2.0;
        let y = 11.0 + hash_unit(cloud + 7) * 23.0;
        draw_dithered_ref_ellipse(canvas, stage, x, y, 18.0, 4.0, 0.35, camera, 0.10, cloud);
    }
}

fn draw_mid_background(canvas: &mut Canvas, stage: Stage, state: TimelineState) {
    let camera = state.camera;
    draw_ref_polygon(
        canvas,
        stage,
        &[
            Point::new(95.0, 66.0),
            Point::new(105.0, 45.0),
            Point::new(111.0, 66.0),
        ],
        PaletteIndex::MidTone,
        camera,
        0.35,
    );
    draw_ref_polygon(
        canvas,
        stage,
        &[
            Point::new(112.0, 69.0),
            Point::new(125.0, 52.0),
            Point::new(133.0, 69.0),
        ],
        PaletteIndex::MidTone,
        camera,
        0.35,
    );

    for band in 0..4 {
        let y = 58.0 + band as f32 * 4.2;
        let phase = state.dissolve * 2.0 + band as f32 * 1.7;
        for index in 0..26 {
            let x = index as f32 * 7.0 - 8.0 + phase.sin() * 3.0;
            if (index + band) % 3 == 0 {
                continue;
            }
            let p = stage.point(x, y + (index as f32 * 0.63).sin(), camera, 0.35);
            canvas.fill_rect(
                p.x.round() as i32,
                p.y.round() as i32,
                (stage.scale * 2.0).ceil() as i32,
                stage.scale.ceil() as i32,
                PaletteIndex::MidTone,
            );
        }
    }
}

fn draw_foreground(canvas: &mut Canvas, stage: Stage, state: TimelineState) {
    let camera = state.camera;
    let ground = [
        Point::new(-8.0, 78.0),
        Point::new(18.0, 72.0),
        Point::new(52.0, 76.0),
        Point::new(84.0, 73.0),
        Point::new(168.0, 78.0),
        Point::new(168.0, 94.0),
        Point::new(-8.0, 94.0),
    ];
    draw_ref_polygon(
        canvas,
        stage,
        &ground,
        PaletteIndex::Background,
        camera,
        1.30,
    );

    for blade in 0..32_u32 {
        let x = hash_unit(blade.wrapping_mul(19)) * 170.0 - 5.0;
        let base_y = 76.0 + hash_unit(blade.wrapping_mul(23)) * 10.0;
        let height = 3.0 + hash_unit(blade.wrapping_mul(29)) * 7.0;
        let bend = (state.dissolve * 6.0 + blade as f32).sin() * 1.7;
        draw_ref_thick_line(
            canvas,
            stage,
            Point::new(x, base_y),
            Point::new(x + bend, base_y - height),
            0.7,
            PaletteIndex::MidTone,
            camera,
            1.30,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_asset_frame(
    canvas: &mut Canvas,
    stage: Stage,
    frame: &AssetFrame,
    width: u16,
    height: u16,
    camera: Camera,
    parallax: f32,
    opacity: f32,
) {
    let width_i32 = i32::from(width);
    let height_i32 = i32::from(height);
    let brush = stage.scalar(0.82, camera).ceil() as i32;
    for y in 0..height_i32 {
        for x in 0..width_i32 {
            let color = frame.pixel_at(width, x, y);
            let Some(palette) = color.to_palette() else {
                continue;
            };
            if opacity < 1.0 && hash_unit(stable_hash((x as u32) << 16 ^ y as u32)) > opacity {
                continue;
            }
            let p = stage.point(x as f32, y as f32, camera, parallax);
            canvas.fill_rect(
                p.x.round() as i32,
                p.y.round() as i32,
                brush,
                brush,
                palette,
            );
        }
    }
}

fn draw_logo(
    canvas: &mut Canvas,
    stage: Stage,
    logo: &AssetFrame,
    width: u16,
    state: TimelineState,
) {
    let camera = Camera::new(80.0, 45.0, 1.0);
    let diag_min = 34.0 - 60.0 * 0.55;
    let diag_max = 132.0 - 29.0 * 0.55;
    let threshold = lerp(diag_min - 8.0, diag_max + 8.0, state.logo_reveal);
    for y in 0..90_i32 {
        for x in 0..160_i32 {
            let color = logo.pixel_at(width, x, y);
            let Some(palette) = color.to_palette() else {
                continue;
            };
            if x as f32 - y as f32 * 0.55 > threshold {
                continue;
            }
            let p = stage.point(x as f32, y as f32, camera, 1.0);
            canvas.fill_rect(
                p.x.round() as i32,
                p.y.round() as i32,
                stage.scale.ceil() as i32,
                stage.scale.ceil() as i32,
                palette,
            );
        }
    }
}

fn draw_logo_particles(
    canvas: &mut Canvas,
    stage: Stage,
    logo: &AssetFrame,
    width: u16,
    state: TimelineState,
) {
    let camera = Camera::new(80.0, 45.0, 1.0);
    let p = smoothstep(state.dissolve);
    let moon = Point::new(124.0, 21.0);
    let mut drawn = 0_usize;

    for y in 0..90_i32 {
        for x in 0..160_i32 {
            let color = logo.pixel_at(width, x, y);
            let Some(palette) = color.to_palette() else {
                continue;
            };
            let seed = stable_hash((x as u32) << 17 ^ (y as u32) << 3 ^ 0x53a9);
            let delay = hash_unit(seed ^ 0x11) * 0.32;
            let local = clamp01((p - delay) / (1.0 - delay).max(0.01));
            if local <= 0.0 {
                continue;
            }

            let burst = Point::new(
                x as f32 + (hash_unit(seed ^ 0xa51) - 0.5) * 42.0,
                y as f32 + (hash_unit(seed ^ 0x5ea) - 0.5) * 28.0,
            );
            let curve = (hash_unit(seed ^ 0x771) - 0.5) * 18.0 * (1.0 - local);
            let travel = if local < 0.42 {
                let q = smoothstep(local / 0.42);
                Point::new(lerp(x as f32, burst.x, q), lerp(y as f32, burst.y, q))
            } else {
                let q = ease_in_cubic((local - 0.42) / 0.58);
                Point::new(
                    lerp(burst.x, moon.x, q) + curve,
                    lerp(burst.y, moon.y, q) - curve * 0.35,
                )
            };
            let size = if seed.is_multiple_of(9) { 2.0 } else { 1.0 };
            let point = stage.point(travel.x, travel.y, camera, 1.0);
            canvas.fill_rect(
                point.x.round() as i32,
                point.y.round() as i32,
                (stage.scale * size).ceil().max(1.0) as i32,
                (stage.scale * size).ceil().max(1.0) as i32,
                palette,
            );
            drawn += 1;
            if drawn > state.particle_count.max(24) {
                return;
            }
        }
    }
}

fn draw_slash(canvas: &mut Canvas, stage: Stage, state: TimelineState) {
    let p = smoothstep(state.slash);
    let start = Point::new(26.0, 69.0);
    let end = Point::new(142.0, 19.0);
    for trail in 0..3 {
        let lag = trail as f32 * 0.15;
        let head = clamp01(p - lag);
        if head <= 0.0 {
            continue;
        }
        let tail = clamp01(head - 0.38);
        let color = if trail == 0 {
            PaletteIndex::Accent
        } else {
            PaletteIndex::Foreground
        };
        let thickness = 3.2 - trail as f32 * 0.85;
        draw_ref_thick_line(
            canvas,
            stage,
            Point::new(lerp(start.x, end.x, tail), lerp(start.y, end.y, tail)),
            Point::new(lerp(start.x, end.x, head), lerp(start.y, end.y, head)),
            thickness,
            color,
            state.camera,
            1.0,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_dithered_ref_circle(
    canvas: &mut Canvas,
    stage: Stage,
    center: Point,
    radius: f32,
    density: f32,
    color: PaletteIndex,
    camera: Camera,
    parallax: f32,
    seed: u32,
) {
    let min_x = (center.x - radius).floor() as i32;
    let max_x = (center.x + radius).ceil() as i32;
    let min_y = (center.y - radius).floor() as i32;
    let max_y = (center.y + radius).ceil() as i32;
    let radius_sq = radius * radius;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 - center.x;
            let dy = y as f32 - center.y;
            if dx * dx + dy * dy > radius_sq {
                continue;
            }
            if !dither_accept(x, y, density, seed) {
                continue;
            }
            let p = stage.point(x as f32, y as f32, camera, parallax);
            canvas.fill_rect(
                p.x.round() as i32,
                p.y.round() as i32,
                stage.scale.ceil() as i32,
                stage.scale.ceil() as i32,
                color,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_dithered_ref_ellipse(
    canvas: &mut Canvas,
    stage: Stage,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    density: f32,
    camera: Camera,
    parallax: f32,
    seed: u32,
) {
    for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
        for x in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
            let dx = (x as f32 - cx) / rx;
            let dy = (y as f32 - cy) / ry;
            if dx * dx + dy * dy > 1.0 || !dither_accept(x, y, density, seed) {
                continue;
            }
            let p = stage.point(x as f32, y as f32, camera, parallax);
            canvas.set(
                p.x.round() as i32,
                p.y.round() as i32,
                PaletteIndex::MidTone,
            );
        }
    }
}

fn dither_accept(x: i32, y: i32, density: f32, seed: u32) -> bool {
    let x = (x + (seed as i32 & 3)).rem_euclid(4) as usize;
    let y = (y + ((seed >> 2) as i32 & 3)).rem_euclid(4) as usize;
    BAYER_4X4[y][x] <= clamp01(density)
}

fn draw_ref_polygon(
    canvas: &mut Canvas,
    stage: Stage,
    points: &[Point],
    color: PaletteIndex,
    camera: Camera,
    parallax: f32,
) {
    let transformed: Vec<_> = points
        .iter()
        .map(|point| stage.point(point.x, point.y, camera, parallax))
        .collect();
    canvas.fill_polygon(&transformed, color);
}

#[allow(clippy::too_many_arguments)]
fn draw_ref_thick_line(
    canvas: &mut Canvas,
    stage: Stage,
    start: Point,
    end: Point,
    thickness: f32,
    color: PaletteIndex,
    camera: Camera,
    parallax: f32,
) {
    canvas.draw_thick_line(
        stage.point(start.x, start.y, camera, parallax),
        stage.point(end.x, end.y, camera, parallax),
        stage.scalar(thickness, camera),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::{CapeBuffers, Scene, TimelineState, render, sample_cape_layer_reference};
    use crate::animation::asset::{CapeLayerId, KfaAsset};
    use crate::canvas::{Canvas, PaletteIndex};

    #[test]
    fn representative_timeline_states_are_structurally_distinct() {
        let asset =
            KfaAsset::from_bytes(include_bytes!("../../assets/generated/animation.kfa")).unwrap();
        let mut buffers = CapeBuffers::for_asset(&asset);
        let times = [
            0.00, 0.80, 1.50, 2.80, 3.50, 3.78, 4.20, 5.30, 6.80, 7.70, 7.99,
        ];
        let mut hashes = Vec::new();
        for seconds in times {
            let t = seconds / 8.0;
            let state = TimelineState::at(seconds);
            let mut canvas = Canvas::new(140, 70, PaletteIndex::Background).unwrap();
            render(&mut canvas, &asset, &mut buffers, t);
            let bbox = bounding_box(&canvas).expect("representative frames should not be empty");
            assert!(bbox.2 > bbox.0);
            assert!(canvas.count(PaletteIndex::Foreground) > 20);
            hashes.push(canvas.content_hash());
            match state.scene {
                Scene::Slash => assert!(canvas.count(PaletteIndex::Accent) > 20),
                Scene::Knightty | Scene::Dissolve => assert!(state.logo_reveal > 0.0),
                _ => {}
            }
        }
        hashes.dedup();
        assert!(hashes.len() >= 8);
    }

    #[test]
    fn loop_boundary_keeps_camera_and_content_near_start_state() {
        let asset =
            KfaAsset::from_bytes(include_bytes!("../../assets/generated/animation.kfa")).unwrap();
        let mut buffers = CapeBuffers::for_asset(&asset);
        let mut start = Canvas::new(120, 60, PaletteIndex::Background).unwrap();
        let mut end = Canvas::new(120, 60, PaletteIndex::Background).unwrap();
        render(&mut start, &asset, &mut buffers, 0.0);
        render(&mut end, &asset, &mut buffers, 7.99 / 8.0);
        assert!(bounding_box(&start).unwrap().2 > 80);
        assert!(bounding_box(&end).unwrap().2 > 80);
        let start_camera = TimelineState::at(0.0).camera;
        let end_camera = TimelineState::at(7.99).camera;
        assert!((start_camera.position.x - end_camera.position.x).abs() < 4.0);
        assert!((start_camera.zoom - end_camera.zoom).abs() < 0.08);
    }

    #[test]
    fn pose_assets_have_readable_structure() {
        let asset =
            KfaAsset::from_bytes(include_bytes!("../../assets/generated/animation.kfa")).unwrap();
        let anticipation = asset.frame("anticipation").unwrap();
        let follow = asset.frame("follow_a").unwrap();
        let smear = asset.frame("slash_smear").unwrap();
        let idle = asset.frame("idle_a").unwrap();

        for frame in asset.frames() {
            let bbox =
                asset_bbox(frame).unwrap_or_else(|| panic!("{} should not be empty", frame.name()));
            assert!(bbox.2 < asset.width() as usize);
            assert!(bbox.3 < asset.height() as usize);
            assert!(
                frame
                    .pixels()
                    .contains(&crate::animation::asset::AssetColor::Foreground)
            );
        }

        assert_ne!(idle.pixels(), anticipation.pixels());
        assert_ne!(anticipation.pixels(), follow.pixels());
        assert!(bbox_width(smear) > bbox_width(idle));
        assert_ne!(center_x(anticipation), center_x(follow));
        assert!(downscaled_non_background(idle, asset.width(), asset.height()) > 40);
    }

    #[test]
    fn cape_assets_have_fixed_topology_and_readable_layers() {
        let asset =
            KfaAsset::from_bytes(include_bytes!("../../assets/generated/animation.kfa")).unwrap();
        assert_eq!(asset.cape_layers().len(), CapeLayerId::ALL.len());
        for pose in asset.cape_poses() {
            for (index, layer) in asset.cape_layers().iter().enumerate() {
                let vertices = pose.layer_vertices(index).unwrap();
                assert_eq!(vertices.len(), layer.vertex_count() as usize);
                let bbox =
                    point_bbox(vertices).unwrap_or_else(|| panic!("{}", layer.id().as_str()));
                assert!(bbox.2 - bbox.0 > 6.0, "{}", layer.id().as_str());
                assert!(bbox.3 - bbox.1 > 3.0, "{}", layer.id().as_str());
            }
        }
    }

    #[test]
    fn cape_tips_continue_after_body_follow_through() {
        let asset =
            KfaAsset::from_bytes(include_bytes!("../../assets/generated/animation.kfa")).unwrap();
        let mut buffers = CapeBuffers::for_asset(&asset);
        let main_index = asset.cape_layer_index(CapeLayerId::CapeMain).unwrap();
        let ribbon_index = asset.cape_layer_index(CapeLayerId::RibbonNear).unwrap();

        let early_main = sample_bbox(
            &asset,
            &mut buffers,
            4.20,
            CapeLayerId::CapeMain,
            main_index,
        );
        let early_ribbon = sample_bbox(
            &asset,
            &mut buffers,
            4.20,
            CapeLayerId::RibbonNear,
            ribbon_index,
        );
        let late_ribbon = sample_bbox(
            &asset,
            &mut buffers,
            4.80,
            CapeLayerId::RibbonNear,
            ribbon_index,
        );
        assert!(early_main.2 - early_main.0 > 38.0);
        assert!(
            late_ribbon.3 - early_ribbon.3 > 4.0,
            "early_ribbon={early_ribbon:?} late_ribbon={late_ribbon:?}"
        );
    }

    #[test]
    fn tiny_canvas_does_not_panic() {
        let asset =
            KfaAsset::from_bytes(include_bytes!("../../assets/generated/animation.kfa")).unwrap();
        let mut buffers = CapeBuffers::for_asset(&asset);
        let mut canvas = Canvas::new(4, 3, PaletteIndex::Background).unwrap();
        render(&mut canvas, &asset, &mut buffers, 0.45);
        assert_eq!(canvas.width(), 4);
        assert_eq!(canvas.height(), 3);
    }

    fn sample_bbox(
        asset: &KfaAsset,
        buffers: &mut CapeBuffers,
        seconds: f32,
        layer: CapeLayerId,
        layer_index: usize,
    ) -> (f32, f32, f32, f32) {
        let state = TimelineState::at(seconds);
        assert!(sample_cape_layer_reference(
            asset,
            buffers,
            state.cape_motion,
            layer,
            layer_index
        ));
        point_bbox(&buffers.reference_points).unwrap()
    }

    fn bounding_box(canvas: &Canvas) -> Option<(usize, usize, usize, usize)> {
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0;
        let mut max_y = 0;
        for y in 0..canvas.height() {
            for x in 0..canvas.width() {
                if canvas.pixel_at(x, y) == PaletteIndex::Background {
                    continue;
                }
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
        (min_x != usize::MAX).then_some((min_x, min_y, max_x, max_y))
    }

    fn asset_bbox(
        frame: &crate::animation::asset::AssetFrame,
    ) -> Option<(usize, usize, usize, usize)> {
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0;
        let mut max_y = 0;
        for y in 0..90_usize {
            for x in 0..160_usize {
                if frame.pixels()[y * 160 + x] == crate::animation::asset::AssetColor::Transparent {
                    continue;
                }
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
        (min_x != usize::MAX).then_some((min_x, min_y, max_x, max_y))
    }

    fn point_bbox(points: &[crate::raster::Point]) -> Option<(f32, f32, f32, f32)> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for point in points {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
        min_x.is_finite().then_some((min_x, min_y, max_x, max_y))
    }

    fn bbox_width(frame: &crate::animation::asset::AssetFrame) -> usize {
        let bbox = asset_bbox(frame).unwrap();
        bbox.2 - bbox.0 + 1
    }

    fn center_x(frame: &crate::animation::asset::AssetFrame) -> usize {
        let bbox = asset_bbox(frame).unwrap();
        (bbox.0 + bbox.2) / 2
    }

    fn downscaled_non_background(
        frame: &crate::animation::asset::AssetFrame,
        width: u16,
        height: u16,
    ) -> usize {
        let width = usize::from(width);
        let height = usize::from(height);
        let mut count = 0;
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                let mut found = false;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let index = (y + dy).min(height - 1) * width + (x + dx).min(width - 1);
                        found |= frame.pixels()[index]
                            != crate::animation::asset::AssetColor::Transparent;
                    }
                }
                count += usize::from(found);
            }
        }
        count
    }
}
