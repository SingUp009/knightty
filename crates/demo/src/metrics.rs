use std::collections::VecDeque;
use std::time::{Duration, Instant};

const SAMPLE_LIMIT: usize = 4096;

#[derive(Debug)]
pub struct Metrics {
    target_fps: u32,
    rendered_frames: u64,
    dropped_frames: u64,
    bytes_written: u64,
    encode_times: SampleWindow,
    frame_times: SampleWindow,
    last_frame_start: Option<Instant>,
}

impl Metrics {
    pub fn new(target_fps: u32) -> Self {
        Self {
            target_fps,
            rendered_frames: 0,
            dropped_frames: 0,
            bytes_written: 0,
            encode_times: SampleWindow::new(SAMPLE_LIMIT),
            frame_times: SampleWindow::new(SAMPLE_LIMIT),
            last_frame_start: None,
        }
    }

    pub fn record_frame_start(&mut self, now: Instant) {
        if let Some(last) = self.last_frame_start.replace(now) {
            self.frame_times.push(now.saturating_duration_since(last));
        }
    }

    pub fn record_frame(&mut self, bytes: usize, encode_time: Duration) {
        self.rendered_frames += 1;
        self.bytes_written = self.bytes_written.saturating_add(bytes as u64);
        self.encode_times.push(encode_time);
    }

    pub fn add_dropped_frames(&mut self, dropped: u64) {
        self.dropped_frames = self.dropped_frames.saturating_add(dropped);
    }

    pub fn report(&self, elapsed: Duration) -> String {
        let bytes_per_frame = if self.rendered_frames == 0 {
            0.0
        } else {
            self.bytes_written as f64 / self.rendered_frames as f64
        };
        let average_encode = self.encode_times.average();
        let encode_p50 = self.encode_times.percentile(50.0);
        let frame_p50 = self.frame_times.percentile(50.0);
        let frame_p95 = self.frame_times.percentile(95.0);
        let frame_p99 = self.frame_times.percentile(99.0);

        format!(
            "Knightty Demo Results\n\
             ---------------------\n\
             Duration:            {:>10.3} s\n\
             Target FPS:          {:>10}\n\
             Rendered frames:     {:>10}\n\
             Dropped frames:      {:>10}\n\
             Bytes written:       {:>10}\n\
             Average bytes/frame: {:>10}\n\
             Average encode time: {:>10}\n\
             Encode time p50:     {:>10}\n\
             Frame time p50:      {:>10}\n\
             Frame time p95:      {:>10}\n\
             Frame time p99:      {:>10}\n",
            elapsed.as_secs_f64(),
            format_target_fps(self.target_fps),
            self.rendered_frames,
            self.dropped_frames,
            format_bytes(self.bytes_written as f64),
            format_bytes(bytes_per_frame),
            format_duration(average_encode),
            format_duration(encode_p50),
            format_duration(frame_p50),
            format_duration(frame_p95),
            format_duration(frame_p99)
        )
    }
}

#[derive(Debug)]
struct SampleWindow {
    limit: usize,
    samples: VecDeque<Duration>,
}

impl SampleWindow {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            samples: VecDeque::with_capacity(limit),
        }
    }

    fn push(&mut self, sample: Duration) {
        if self.samples.len() == self.limit {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    fn average(&self) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let total = self.samples.iter().fold(Duration::ZERO, |total, sample| {
            total.saturating_add(*sample)
        });
        Some(total.div_f64(self.samples.len() as f64))
    }

    fn percentile(&self, percentile: f64) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let mut samples: Vec<_> = self.samples.iter().copied().collect();
        samples.sort_unstable();
        let rank = ((percentile / 100.0) * (samples.len().saturating_sub(1)) as f64).round();
        samples.get(rank as usize).copied()
    }
}

fn format_target_fps(target_fps: u32) -> String {
    if target_fps == 0 {
        "uncapped".to_owned()
    } else {
        target_fps.to_string()
    }
}

fn format_bytes(bytes: f64) -> String {
    if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MiB", bytes / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 {
        format!("{:.1} KiB", bytes / 1024.0)
    } else {
        format!("{bytes:.0} B")
    }
}

fn format_duration(duration: Option<Duration>) -> String {
    let Some(duration) = duration else {
        return "n/a".to_owned();
    };
    let micros = duration.as_secs_f64() * 1_000_000.0;
    if micros >= 1000.0 {
        format!("{:.2} ms", micros / 1000.0)
    } else {
        format!("{micros:.1} us")
    }
}
