#![cfg(windows)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use knightty_pty::{PtySession, PtySize, ShellCommand};

const METHODS: [&str; 3] = ["ConsoleWrite", "ConsoleOutWrite", "StandardOutput"];

#[test]
#[ignore = "manual Windows ConPTY transport diagnostic"]
fn compare_powershell_apc_write_methods_at_raw_pty_reader() {
    let mut preserved_results = Vec::new();

    for method in METHODS {
        let output = capture_method(method);
        let kitty_starts = count(&output, b"\x1b_G");
        let string_terminators = count(&output, b"\x1b\\");
        let (head, tail) = hex_edges(&output);
        let preserved = kitty_starts == 1 && string_terminators >= 1;
        let end_seen = contains(&output, format!("END:{method}").as_bytes());
        preserved_results.push(preserved);

        println!(
            "method={method} bytes={} esc_G={} esc_st={} preserved={} end_seen={} head={} tail={}",
            output.len(),
            kitty_starts,
            string_terminators,
            preserved,
            end_seen,
            head,
            tail
        );
        assert!(
            contains(&output, format!("BEGIN:{method}").as_bytes()),
            "PowerShell probe did not start for {method}"
        );
    }

    assert!(
        preserved_results.windows(2).all(|pair| pair[0] == pair[1]),
        "PowerShell output APIs produced different APC preservation results: {preserved_results:?}"
    );
}

fn capture_method(method: &str) -> Vec<u8> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/dev/probe-kitty-apc.ps1")
        .canonicalize()
        .expect("canonical probe script path");
    let command = ShellCommand {
        program: "powershell.exe".to_owned(),
        args: vec![
            "-NoLogo".to_owned(),
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-ExecutionPolicy".to_owned(),
            "Bypass".to_owned(),
            "-File".to_owned(),
            script.display().to_string(),
            "-Method".to_owned(),
            method.to_owned(),
        ],
    };
    let mut session = PtySession::spawn_shell(
        PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        },
        Some(&command),
    )
    .expect("spawn PowerShell in ConPTY");
    let mut reader = session.take_reader().expect("take ConPTY reader");
    let mut writer = session.take_writer().expect("take ConPTY writer");
    writer
        .write_all(b"\x1b[1;1R")
        .expect("answer ConPTY cursor position query");
    writer.flush().expect("flush cursor position response");
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let marker = format!("END:{method}");
    let mut output = Vec::new();
    while std::time::Instant::now() < deadline {
        let timeout = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(chunk) = receiver.recv_timeout(timeout) else {
            return output;
        };
        output.extend_from_slice(&chunk);
        if output
            .windows(marker.len())
            .any(|window| window == marker.as_bytes())
        {
            return output;
        }
    }
    panic!("PowerShell ConPTY probe did not emit its end marker")
}

fn count(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

fn hex_edges(bytes: &[u8]) -> (String, String) {
    const EDGE: usize = 16;
    let head = hex(&bytes[..bytes.len().min(EDGE)]);
    let tail = if bytes.len() > EDGE {
        hex(&bytes[bytes.len() - EDGE..])
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
