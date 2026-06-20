#![cfg(unix)]

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use knightty_pty::{PtySession, PtySize, ShellCommand};

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

#[test]
fn unix_pty_preserves_a_minimal_kitty_apc_byte_for_byte() {
    let apc = format!("\x1b_Ga=T,f=100,t=d,i=4242,p=7,q=0,m=0,c=1,r=1,C=1;{PNG}\x1b\\");
    let expected = format!("text-before:{apc}:text-after");
    let payload_split = PNG.len() / 2;
    let (payload_start, payload_end) = PNG.split_at(payload_split);
    let shell_script = format!(
        "printf 'text-before:'; \
         printf '\\033_'; \
         printf 'Ga=T,f=100,t=d,i=4242,p=7,q=0,m=0,c=1,r=1,C=1;'; \
         printf '{payload_start}'; \
         printf '{payload_end}'; \
         printf '\\033'; \
         printf '\\\\'; \
         printf ':text-after'"
    );
    let command = ShellCommand {
        program: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), shell_script],
    };
    let mut session = PtySession::spawn_shell(PtySize::default(), Some(&command))
        .expect("spawn shell in Unix PTY");
    let mut reader = session.take_reader().expect("take Unix PTY reader");
    let (sender, receiver) = mpsc::channel();

    let reader_thread = thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(read) => {
                    if sender.send(buffer[..read].to_vec()).is_err() {
                        return Ok(());
                    }
                }
                Err(error) if is_pty_eof(&error) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(timeout) {
            Ok(chunk) => output.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(session);
                panic!("Unix PTY APC probe timed out after 5 seconds: {output:?}");
            }
        }
    }

    drop(session);
    let read_result = reader_thread
        .join()
        .expect("Unix PTY reader thread panicked");
    read_result.expect("read Unix PTY output through child EOF");

    assert_eq!(output, expected.as_bytes());

    let apc_start = output
        .windows(3)
        .position(|window| window == b"\x1b_G")
        .expect("Kitty APC introducer reached the parent");
    let apc_end = output[apc_start..]
        .windows(2)
        .position(|window| window == b"\x1b\\")
        .map(|offset| apc_start + offset + 2)
        .expect("Kitty APC terminator reached the parent");
    assert_eq!(&output[apc_start..apc_end], apc.as_bytes());
}

fn is_pty_eof(error: &std::io::Error) -> bool {
    // Linux PTY masters report EIO after the slave closes instead of returning
    // a zero-length read. At that point all preceding output has been drained.
    error.raw_os_error() == Some(5)
}
