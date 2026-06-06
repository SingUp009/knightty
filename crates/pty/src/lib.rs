use std::io::{Read, Write};
#[cfg(windows)]
use std::path::Path;

use portable_pty::{Child, CommandBuilder, MasterPty, native_pty_system};
use thiserror::Error;

/// Running PTY session with one child process.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer_taken: bool,
}

impl PtySession {
    pub fn spawn_default_shell(size: PtySize) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size.into())
            .map_err(|error| PtyError::SpawnFailed(error.to_string()))?;

        let command = default_shell_command();
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| PtyError::SpawnFailed(error.to_string()))?;

        drop(pair.slave);

        Ok(Self {
            master: pair.master,
            child,
            writer_taken: false,
        })
    }

    pub fn take_reader(&mut self) -> Result<Box<dyn Read + Send>, PtyError> {
        self.master
            .try_clone_reader()
            .map_err(|error| PtyError::Io(error.to_string()))
    }

    pub fn take_writer(&mut self) -> Result<Box<dyn Write + Send>, PtyError> {
        if self.writer_taken {
            return Err(PtyError::WriterAlreadyTaken);
        }

        let writer = self
            .master
            .take_writer()
            .map_err(|error| PtyError::Io(error.to_string()))?;
        self.writer_taken = true;
        Ok(writer)
    }

    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        self.master
            .resize(size.into())
            .map_err(|error| PtyError::ResizeFailed(error.to_string()))
    }

    pub fn child_id(&self) -> Option<u32> {
        self.child.process_id()
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// PTY character and pixel size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl From<PtySize> for portable_pty::PtySize {
    fn from(size: PtySize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }
    }
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("failed to spawn PTY child: {0}")]
    SpawnFailed(String),
    #[error("PTY IO failed: {0}")]
    Io(String),
    #[error("failed to resize PTY: {0}")]
    ResizeFailed(String),
    #[error("PTY writer has already been taken")]
    WriterAlreadyTaken,
}

fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
}

fn default_shell_command() -> CommandBuilder {
    let shell = default_shell();
    command_for_shell(shell)
}

fn command_for_shell(shell: String) -> CommandBuilder {
    let mut command = CommandBuilder::new(&shell);
    add_default_shell_args(&shell, &mut command);
    command
}

fn add_default_shell_args(shell: &str, command: &mut CommandBuilder) {
    #[cfg(windows)]
    {
        if is_cmd_shell(shell) {
            command.args(["/Q", "/D", "/K", "cls"]);
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (shell, command);
    }
}

#[cfg(windows)]
fn is_cmd_shell(shell: &str) -> bool {
    Path::new(shell)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .is_some_and(|file_name| {
            file_name.eq_ignore_ascii_case("cmd.exe") || file_name.eq_ignore_ascii_case("cmd")
        })
}

#[cfg(all(test, windows))]
mod tests {
    use super::command_for_shell;

    #[test]
    fn cmd_shell_command_uses_quiet_startup_args() {
        let command = command_for_shell(r"C:\Windows\System32\cmd.exe".to_owned());
        let argv = command.get_argv();

        assert_eq!(argv.len(), 5);
        assert_eq!(argv[1], "/Q");
        assert_eq!(argv[2], "/D");
        assert_eq!(argv[3], "/K");
        assert_eq!(argv[4], "cls");
    }

    #[test]
    fn non_cmd_windows_shell_gets_no_cmd_specific_args() {
        let command = command_for_shell("pwsh.exe".to_owned());
        let argv = command.get_argv();

        assert_eq!(argv.len(), 1);
    }
}
