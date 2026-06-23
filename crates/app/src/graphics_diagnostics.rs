use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub const ENV_VAR: &str = "KNIGHTTY_GRAPHICS_DIAGNOSTICS";

const HEX_EDGE_BYTES: usize = 16;
const KITTY_START: &[u8] = b"\x1b_G";
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

static ENABLED: OnceLock<bool> = OnceLock::new();
static RAW_PROBE: OnceLock<Mutex<RawApcProbe>> = OnceLock::new();
static PTY_BYTES: AtomicU64 = AtomicU64::new(0);
static KITTY_STARTS: AtomicU64 = AtomicU64::new(0);
static STRING_TERMINATORS: AtomicU64 = AtomicU64::new(0);
static COMPLETED_COMMANDS: AtomicU64 = AtomicU64::new(0);
static DECODE_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static PLACEMENTS_CREATED: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var(ENV_VAR).is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    })
}

/// Record bytes at the earliest application-owned boundary: immediately after
/// the PTY reader returns and before coalescing or graphics escape routing.
pub fn record_pty_read(bytes: &[u8]) {
    if !enabled() {
        return;
    }

    let (new_starts, new_terminators) = RAW_PROBE
        .get_or_init(|| Mutex::new(RawApcProbe::default()))
        .lock()
        .expect("graphics diagnostics probe lock")
        .feed(bytes);
    let total_bytes =
        PTY_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed) + bytes.len() as u64;
    let total_starts = KITTY_STARTS.fetch_add(new_starts, Ordering::Relaxed) + new_starts;
    let total_terminators =
        STRING_TERMINATORS.fetch_add(new_terminators, Ordering::Relaxed) + new_terminators;
    let (head, tail) = hex_edges(bytes);

    eprintln!(
        "knightty graphics diag: pty-read bytes={} total_bytes={} head={} tail={} esc_G=+{}({}) esc_st=+{}({})",
        bytes.len(),
        total_bytes,
        head,
        tail,
        new_starts,
        total_starts,
        new_terminators,
        total_terminators,
    );
}

pub fn record_completed_kitty_command(command_bytes: usize) {
    if !enabled() {
        return;
    }
    let total = COMPLETED_COMMANDS.fetch_add(1, Ordering::Relaxed) + 1;
    eprintln!(
        "knightty graphics diag: kitty-command complete=1 total={} command_bytes={}",
        total, command_bytes
    );
}

pub fn record_decode_success(encoded_bytes: usize, width: u32, height: u32) {
    if !enabled() {
        return;
    }
    let total = DECODE_SUCCESSES.fetch_add(1, Ordering::Relaxed) + 1;
    eprintln!(
        "knightty graphics diag: image-decode success=1 total={} encoded_bytes={} dimensions={}x{}",
        total, encoded_bytes, width, height
    );
}

pub fn record_placement_created() {
    if !enabled() {
        return;
    }
    let total = PLACEMENTS_CREATED.fetch_add(1, Ordering::Relaxed) + 1;
    eprintln!(
        "knightty graphics diag: kitty-placement created=1 total={}",
        total
    );
}

#[derive(Default)]
struct RawApcProbe {
    tail: Vec<u8>,
}

impl RawApcProbe {
    fn feed(&mut self, bytes: &[u8]) -> (u64, u64) {
        let old_len = self.tail.len();
        let mut combined = core::mem::take(&mut self.tail);
        combined.extend_from_slice(bytes);

        let starts = count_new_matches(&combined, old_len, KITTY_START);
        let terminators = count_new_matches(&combined, old_len, STRING_TERMINATOR);
        let keep_from = combined.len().saturating_sub(KITTY_START.len() - 1);
        self.tail.extend_from_slice(&combined[keep_from..]);
        (starts, terminators)
    }
}

fn count_new_matches(bytes: &[u8], old_len: usize, needle: &[u8]) -> u64 {
    bytes
        .windows(needle.len())
        .enumerate()
        .filter(|(start, window)| start + needle.len() > old_len && *window == needle)
        .count() as u64
}

fn hex_edges(bytes: &[u8]) -> (String, String) {
    let head_len = bytes.len().min(HEX_EDGE_BYTES);
    let head = hex(&bytes[..head_len]);
    let tail = if bytes.len() > HEX_EDGE_BYTES {
        hex(&bytes[bytes.len().saturating_sub(HEX_EDGE_BYTES)..])
    } else {
        "-".to_owned()
    };
    (head, tail)
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{RawApcProbe, hex_edges};

    #[test]
    fn probe_counts_markers_across_read_boundaries_once() {
        let mut probe = RawApcProbe::default();

        assert_eq!(probe.feed(b"text\x1b_"), (0, 0));
        assert_eq!(probe.feed(b"Gq=0;payload\x1b"), (1, 0));
        assert_eq!(probe.feed(b"\\tail"), (0, 1));
        assert_eq!(probe.feed(b"\x1b_Gx\x1b\\"), (1, 1));
    }

    #[test]
    fn hex_preview_is_bounded_to_short_edges() {
        let bytes = (0_u8..64).collect::<Vec<_>>();
        let (head, tail) = hex_edges(&bytes);

        assert_eq!(head.split_whitespace().count(), 16);
        assert_eq!(tail.split_whitespace().count(), 16);
        assert!(head.starts_with("00 01 02"));
        assert!(tail.ends_with("3d 3e 3f"));
    }
}
