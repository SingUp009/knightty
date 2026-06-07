use std::env;
use std::io::{Read, Write};

use portable_pty::{Child, CommandBuilder, MasterPty, native_pty_system};
use thiserror::Error;

/// Shell command used to start a PTY session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// Running PTY session with one child process.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer_taken: bool,
}

impl PtySession {
    pub fn spawn_default_shell(size: PtySize) -> Result<Self, PtyError> {
        Self::spawn_with_command(size, default_shell_command())
    }

    pub fn spawn_shell(size: PtySize, shell: Option<&ShellCommand>) -> Result<Self, PtyError> {
        let command = shell
            .map(command_for_configured_shell)
            .unwrap_or_else(default_shell_command);
        Self::spawn_with_command(size, command)
    }

    fn spawn_with_command(size: PtySize, command: CommandBuilder) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size.into())
            .map_err(|error| PtyError::SpawnFailed(error.to_string()))?;

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
    command_for_shell_with_identity(shell, &PromptIdentity::from_env())
}

fn command_for_shell_with_identity(shell: String, identity: &PromptIdentity) -> CommandBuilder {
    let shell_kind = shell_kind(&shell);
    let mut command = CommandBuilder::new(&shell);
    add_default_shell_args(shell_kind, &mut command);
    configure_prompt_header(shell_kind, &[], identity, &mut command);
    command
}

fn command_for_configured_shell(shell: &ShellCommand) -> CommandBuilder {
    command_for_configured_shell_with_identity(shell, &PromptIdentity::from_env())
}

fn command_for_configured_shell_with_identity(
    shell: &ShellCommand,
    identity: &PromptIdentity,
) -> CommandBuilder {
    let shell_kind = shell_kind(&shell.program);
    let mut command = CommandBuilder::new(&shell.program);
    command.args(&shell.args);
    configure_prompt_header(shell_kind, &shell.args, identity, &mut command);
    command
}

fn add_default_shell_args(shell_kind: ShellKind, command: &mut CommandBuilder) {
    #[cfg(windows)]
    {
        if shell_kind == ShellKind::Cmd {
            command.args(["/Q", "/D", "/K", "cls"]);
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (shell_kind, command);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellKind {
    Cmd,
    PowerShell,
    BashLike,
    Zsh,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PromptIdentity {
    username: String,
    computer_name: String,
}

impl PromptIdentity {
    fn from_env() -> Self {
        Self {
            username: first_env_value(["USERNAME", "USER", "LOGNAME"])
                .unwrap_or_else(|| "user".to_owned()),
            computer_name: first_env_value(["COMPUTERNAME", "HOSTNAME"])
                .unwrap_or_else(|| "computer".to_owned()),
        }
    }
}

fn first_env_value<const N: usize>(keys: [&str; N]) -> Option<String> {
    keys.into_iter()
        .find_map(|key| env::var(key).ok().filter(|value| !value.trim().is_empty()))
}

fn shell_kind(shell: &str) -> ShellKind {
    match shell_file_name(shell).to_ascii_lowercase().as_str() {
        "cmd" | "cmd.exe" => ShellKind::Cmd,
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe" => ShellKind::PowerShell,
        "bash" | "bash.exe" | "sh" | "sh.exe" | "dash" | "dash.exe" | "ksh" | "ksh.exe"
        | "mksh" | "mksh.exe" => ShellKind::BashLike,
        "zsh" | "zsh.exe" => ShellKind::Zsh,
        _ => ShellKind::Other,
    }
}

fn shell_file_name(shell: &str) -> &str {
    shell.rsplit(['/', '\\']).next().unwrap_or(shell)
}

fn configure_prompt_header(
    shell_kind: ShellKind,
    args: &[String],
    identity: &PromptIdentity,
    command: &mut CommandBuilder,
) {
    match shell_kind {
        ShellKind::Cmd => {
            command.env("PROMPT", cmd_prompt(identity));
        }
        ShellKind::PowerShell => {
            if !powershell_args_execute_command(args) {
                add_powershell_prompt_args(command, args, identity);
            }
        }
        ShellKind::BashLike => {
            command.env("PS1", posix_prompt(identity));
        }
        ShellKind::Zsh => {
            command.env("PROMPT", zsh_prompt(identity));
        }
        ShellKind::Other => {}
    }
}

fn cmd_prompt(identity: &PromptIdentity) -> String {
    format!(
        "[{}@{} $P]$$$S",
        cmd_prompt_literal(&identity.username),
        cmd_prompt_literal(&identity.computer_name)
    )
}

fn cmd_prompt_literal(value: &str) -> String {
    value.replace('$', "$$")
}

fn add_powershell_prompt_args(
    command: &mut CommandBuilder,
    existing_args: &[String],
    identity: &PromptIdentity,
) {
    if !powershell_arg_present(existing_args, "nologo") {
        command.arg("-NoLogo");
    }
    if !powershell_arg_present(existing_args, "noexit") {
        command.arg("-NoExit");
    }
    command.arg("-Command");
    command.arg(powershell_prompt_script(identity));
}

fn powershell_prompt_script(identity: &PromptIdentity) -> String {
    format!(
        "function global:prompt {{ $path = (Get-Location).Path; if ($path -eq $HOME) {{ $dir = '~' }} else {{ $trimmed = $path.TrimEnd([char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)); $dir = [System.IO.Path]::GetFileName($trimmed); if ([string]::IsNullOrEmpty($dir)) {{ $dir = $path }} }}; '[' + {} + '@' + {} + ' ' + $dir + ']$ ' }}",
        powershell_single_quoted(&identity.username),
        powershell_single_quoted(&identity.computer_name)
    )
}

fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn powershell_args_execute_command(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            normalized_powershell_arg(arg).as_deref(),
            Some("command" | "c" | "file" | "f" | "encodedcommand" | "e")
        )
    })
}

fn powershell_arg_present(args: &[String], expected: &str) -> bool {
    args.iter()
        .any(|arg| normalized_powershell_arg(arg).as_deref() == Some(expected))
}

fn normalized_powershell_arg(arg: &str) -> Option<String> {
    let trimmed = arg.trim_start_matches(['-', '/']);
    (trimmed.len() != arg.len()).then(|| trimmed.to_ascii_lowercase())
}

fn posix_prompt(identity: &PromptIdentity) -> String {
    format!(
        "[{}@{} $(if [ \"$PWD\" = \"$HOME\" ]; then printf '~'; else dir=${{PWD##*/}}; if [ -n \"$dir\" ]; then printf '%s' \"$dir\"; else printf '%s' \"$PWD\"; fi; fi)]$ ",
        posix_prompt_literal(&identity.username),
        posix_prompt_literal(&identity.computer_name)
    )
}

fn posix_prompt_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

fn zsh_prompt(identity: &PromptIdentity) -> String {
    format!(
        "[{}@{} %1~]$ ",
        zsh_prompt_literal(&identity.username),
        zsh_prompt_literal(&identity.computer_name)
    )
}

fn zsh_prompt_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('`', "\\`")
        .replace('%', "%%")
}

#[cfg(test)]
mod tests {
    use super::{
        PromptIdentity, ShellCommand, cmd_prompt, command_for_configured_shell_with_identity,
        command_for_shell_with_identity, posix_prompt, powershell_prompt_script, shell_kind,
        zsh_prompt,
    };

    fn test_identity() -> PromptIdentity {
        PromptIdentity {
            username: "alice".to_owned(),
            computer_name: "workstation".to_owned(),
        }
    }

    #[cfg(windows)]
    #[test]
    fn cmd_shell_command_uses_quiet_startup_args_and_prompt_header() {
        let identity = test_identity();
        let command =
            command_for_shell_with_identity(r"C:\Windows\System32\cmd.exe".to_owned(), &identity);
        let argv = command.get_argv();

        assert_eq!(argv.len(), 5);
        assert_eq!(argv[1], "/Q");
        assert_eq!(argv[2], "/D");
        assert_eq!(argv[3], "/K");
        assert_eq!(argv[4], "cls");
        assert_eq!(
            command.get_env("PROMPT").and_then(|value| value.to_str()),
            Some("[alice@workstation $P]$$$S")
        );
    }

    #[cfg(windows)]
    #[test]
    fn powershell_shell_command_installs_prompt_function() {
        let identity = test_identity();
        let command = command_for_shell_with_identity("pwsh.exe".to_owned(), &identity);
        let argv = command.get_argv();

        assert_eq!(argv.len(), 5);
        assert_eq!(argv[1], "-NoLogo");
        assert_eq!(argv[2], "-NoExit");
        assert_eq!(argv[3], "-Command");
        let script = powershell_prompt_script(&identity);
        assert_eq!(argv[4].to_str(), Some(script.as_str()));
    }

    #[cfg(windows)]
    #[test]
    fn configured_powershell_preserves_options_before_prompt_function() {
        let identity = test_identity();
        let command = command_for_configured_shell_with_identity(
            &ShellCommand {
                program: "pwsh".to_owned(),
                args: vec!["-NoLogo".to_owned(), "-NoProfile".to_owned()],
            },
            &identity,
        );
        let argv = command.get_argv();

        assert_eq!(argv.len(), 6);
        assert_eq!(argv[0], "pwsh");
        assert_eq!(argv[1], "-NoLogo");
        assert_eq!(argv[2], "-NoProfile");
        assert_eq!(argv[3], "-NoExit");
        assert_eq!(argv[4], "-Command");
        let script = powershell_prompt_script(&identity);
        assert_eq!(argv[5].to_str(), Some(script.as_str()));
    }

    #[cfg(windows)]
    #[test]
    fn configured_powershell_command_is_left_intact() {
        let identity = test_identity();
        let command = command_for_configured_shell_with_identity(
            &ShellCommand {
                program: "pwsh".to_owned(),
                args: vec!["-Command".to_owned(), "Write-Host hi".to_owned()],
            },
            &identity,
        );
        let argv = command.get_argv();

        assert_eq!(argv.len(), 3);
        assert_eq!(argv[0], "pwsh");
        assert_eq!(argv[1], "-Command");
        assert_eq!(argv[2], "Write-Host hi");
    }

    #[test]
    fn configured_unknown_shell_uses_program_and_args_exactly() {
        let identity = test_identity();
        let command = command_for_configured_shell_with_identity(
            &ShellCommand {
                program: "custom-shell".to_owned(),
                args: vec!["--login".to_owned()],
            },
            &identity,
        );
        let argv = command.get_argv();

        assert_eq!(argv.len(), 2);
        assert_eq!(argv[0], "custom-shell");
        assert_eq!(argv[1], "--login");
    }

    #[test]
    fn bash_like_shell_uses_home_aware_directory_name_prompt_header() {
        let identity = test_identity();
        let command = command_for_shell_with_identity("/bin/bash".to_owned(), &identity);

        assert_eq!(
            command.get_env("PS1").and_then(|value| value.to_str()),
            Some(
                r#"[alice@workstation $(if [ "$PWD" = "$HOME" ]; then printf '~'; else dir=${PWD##*/}; if [ -n "$dir" ]; then printf '%s' "$dir"; else printf '%s' "$PWD"; fi; fi)]$ "#
            )
        );
    }

    #[test]
    fn zsh_shell_uses_home_aware_directory_name_prompt_header() {
        let identity = test_identity();
        let command = command_for_shell_with_identity("/bin/zsh".to_owned(), &identity);

        assert_eq!(
            command.get_env("PROMPT").and_then(|value| value.to_str()),
            Some("[alice@workstation %1~]$ ")
        );
    }

    #[test]
    fn prompt_literals_escape_shell_prompt_markers() {
        let identity = PromptIdentity {
            username: "a$li%ce".to_owned(),
            computer_name: "work$station%".to_owned(),
        };

        assert_eq!(cmd_prompt(&identity), "[a$$li%ce@work$$station% $P]$$$S");
        assert_eq!(
            posix_prompt(&identity),
            r#"[a\$li%ce@work\$station% $(if [ "$PWD" = "$HOME" ]; then printf '~'; else dir=${PWD##*/}; if [ -n "$dir" ]; then printf '%s' "$dir"; else printf '%s' "$PWD"; fi; fi)]$ "#
        );
        assert_eq!(
            zsh_prompt(&identity),
            r#"[a\$li%%ce@work\$station%% %1~]$ "#
        );
    }

    #[test]
    fn shell_kind_uses_file_name_from_windows_or_unix_paths() {
        assert_eq!(
            shell_kind(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            super::ShellKind::PowerShell
        );
        assert_eq!(shell_kind("/usr/bin/bash"), super::ShellKind::BashLike);
    }
}
