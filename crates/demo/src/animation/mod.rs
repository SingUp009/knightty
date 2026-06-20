mod asset;
pub mod knight;

use crate::canvas::Canvas;

use asset::{KfaAsset, load_demo_asset};

pub const LOOP_DURATION_SECONDS: f32 = 8.0;

#[derive(Debug)]
pub struct Animation {
    asset: KfaAsset,
    cape_buffers: knight::CapeBuffers,
}

impl Animation {
    pub fn new() -> Result<Self, asset::AssetError> {
        let asset = load_demo_asset()?;
        let cape_buffers = knight::CapeBuffers::for_asset(&asset);
        Ok(Self {
            asset,
            cape_buffers,
        })
    }

    pub fn render_frame(&mut self, canvas: &mut Canvas, elapsed_seconds: f32) {
        let t = elapsed_seconds.rem_euclid(LOOP_DURATION_SECONDS) / LOOP_DURATION_SECONDS;
        knight::render(canvas, &self.asset, &mut self.cape_buffers, t);
    }
}

pub fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smoothstep(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * (3.0 - 2.0 * t)
}

pub fn ease_in_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * t
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = 1.0 - clamp01(t);
    1.0 - t * t * t
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let value = -2.0 * t + 2.0;
        1.0 - value * value * value / 2.0
    }
}

pub fn segment_t(t: f32, start: f32, end: f32) -> f32 {
    if end <= start {
        return 1.0;
    }
    clamp01((t - start) / (end - start))
}

pub fn frame_index_for_elapsed(elapsed_seconds: f64, frame_duration_seconds: f64) -> u64 {
    if frame_duration_seconds <= 0.0 || !frame_duration_seconds.is_finite() {
        return 0;
    }
    (elapsed_seconds / frame_duration_seconds).floor().max(0.0) as u64
}

pub fn skipped_frames(target_frame_index: u64, next_frame_index: u64) -> u64 {
    target_frame_index.saturating_sub(next_frame_index)
}

pub use asset::AssetError;

#[cfg(test)]
mod tests {
    use super::{
        clamp01, ease_in_cubic, ease_in_out_cubic, ease_out_cubic, frame_index_for_elapsed, lerp,
        segment_t, skipped_frames, smoothstep,
    };

    #[test]
    fn clamp01_limits_values_to_unit_interval() {
        assert_eq!(clamp01(-0.25), 0.0);
        assert_eq!(clamp01(0.5), 0.5);
        assert_eq!(clamp01(2.0), 1.0);
    }

    #[test]
    fn segment_t_normalizes_scene_local_time() {
        assert_eq!(segment_t(0.2, 0.2, 0.4), 0.0);
        assert!((segment_t(0.3, 0.2, 0.4) - 0.5).abs() < f32::EPSILON);
        assert_eq!(segment_t(0.5, 0.2, 0.4), 1.0);
    }

    #[test]
    fn easing_functions_keep_start_and_end_points() {
        for easing in [smoothstep, ease_in_cubic, ease_out_cubic, ease_in_out_cubic] {
            assert_eq!(easing(0.0), 0.0);
            assert_eq!(easing(1.0), 1.0);
        }
        assert_eq!(lerp(2.0, 6.0, 0.25), 3.0);
    }

    #[test]
    fn frame_index_uses_elapsed_time_without_accumulating_sleep_drift() {
        assert_eq!(frame_index_for_elapsed(1.0, 1.0 / 60.0), 60);
        assert_eq!(frame_index_for_elapsed(0.5, 1.0 / 30.0), 15);
    }

    #[test]
    fn skipped_frames_counts_late_frames_without_underflow() {
        assert_eq!(skipped_frames(10, 6), 4);
        assert_eq!(skipped_frames(6, 10), 0);
    }
}
