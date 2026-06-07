use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use knightty_pty::ShellCommand;
use knightty_render::{CellMetrics, RendererConfig};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

const DEFAULT_INITIAL_COLS: usize = 80;
const DEFAULT_INITIAL_ROWS: usize = 24;
const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
const DEFAULT_SCROLL_MULTIPLIER: usize = 3;
const MAX_SCROLLBACK_LINES: usize = 100_000;
const MAX_SCROLL_MULTIPLIER: usize = 100;
const DEFAULT_HYPERLINK_ALLOWED_SCHEMES: &[&str] = &["https", "http"];

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub render: RenderConfig,
    pub terminal: TerminalConfig,
    pub shell: ShellConfig,
    pub hyperlink: HyperlinkConfig,
}

impl AppConfig {
    pub fn initial_cols(&self) -> usize {
        self.window.initial_cols.unwrap_or(DEFAULT_INITIAL_COLS)
    }

    pub fn initial_rows(&self) -> usize {
        self.window.initial_rows.unwrap_or(DEFAULT_INITIAL_ROWS)
    }

    pub fn wgpu_backend(&self) -> Option<&str> {
        self.render.wgpu_backend.as_deref()
    }

    pub fn scrollback_lines(&self) -> usize {
        self.terminal
            .scrollback_lines
            .unwrap_or(DEFAULT_SCROLLBACK_LINES)
    }

    pub fn scroll_multiplier(&self) -> usize {
        self.terminal
            .scroll_multiplier
            .unwrap_or(DEFAULT_SCROLL_MULTIPLIER)
    }

    pub fn renderer_config(&self) -> RendererConfig {
        let default_metrics = CellMetrics::default();
        let font_size = self.font.size.unwrap_or(default_metrics.font_size);
        let line_height = self
            .font
            .line_height
            .unwrap_or_else(|| font_size * default_metrics.line_height / default_metrics.font_size);

        RendererConfig {
            cell_metrics: CellMetrics::from_font_size(font_size, line_height),
            font_family: self.font.family.clone(),
        }
    }

    pub fn shell_command(&self) -> Option<ShellCommand> {
        self.shell.program.as_ref().map(|program| ShellCommand {
            program: program.clone(),
            args: self.shell.args.clone(),
        })
    }

    pub fn hyperlink_open_on_ctrl_click(&self) -> bool {
        self.hyperlink.open_on_ctrl_click
    }

    pub fn hyperlink_allowed_schemes(&self) -> &[String] {
        &self.hyperlink.allowed_schemes
    }

    pub fn hyperlink_underline_on_hover(&self) -> bool {
        self.hyperlink.underline_on_hover
    }

    pub fn hyperlink_open_enabled(&self) -> bool {
        self.hyperlink_open_on_ctrl_click() && !self.hyperlink.allowed_schemes.is_empty()
    }

    fn validate(&self, source: Option<&Path>) -> Result<(), ConfigError> {
        validate_positive_float("font.size", self.font.size, source)?;
        validate_positive_float("font.line_height", self.font.line_height, source)?;
        validate_positive_usize("window.initial_cols", self.window.initial_cols, source)?;
        validate_positive_usize("window.initial_rows", self.window.initial_rows, source)?;
        validate_non_empty_string("shell.program", self.shell.program.as_deref(), source)?;
        validate_usize_range(
            "terminal.scrollback_lines",
            self.terminal.scrollback_lines,
            0,
            MAX_SCROLLBACK_LINES,
            source,
        )?;
        validate_usize_range(
            "terminal.scroll_multiplier",
            self.terminal.scroll_multiplier,
            1,
            MAX_SCROLL_MULTIPLIER,
            source,
        )?;
        if let Some(value) = &self.render.wgpu_backend
            && parse_wgpu_backend_name(value).is_none()
        {
            return Err(ConfigError::InvalidValue {
                path: source.map(Path::to_path_buf),
                field: "render.wgpu_backend",
                message: format!("expected one of auto, vulkan, dx12, or gl, got `{value}`"),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FontConfig {
    pub family: Option<String>,
    pub size: Option<f32>,
    pub line_height: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowConfig {
    pub initial_cols: Option<usize>,
    pub initial_rows: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct RenderConfig {
    pub wgpu_backend: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    pub scrollback_lines: Option<usize>,
    pub scroll_multiplier: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ShellConfig {
    pub program: Option<String>,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct HyperlinkConfig {
    pub open_on_ctrl_click: bool,
    #[serde(
        default = "default_hyperlink_allowed_schemes",
        deserialize_with = "deserialize_lowercase_schemes"
    )]
    pub allowed_schemes: Vec<String>,
    pub underline_on_hover: bool,
}

impl Default for HyperlinkConfig {
    fn default() -> Self {
        Self {
            open_on_ctrl_click: true,
            allowed_schemes: default_hyperlink_allowed_schemes(),
            underline_on_hover: true,
        }
    }
}

fn default_hyperlink_allowed_schemes() -> Vec<String> {
    DEFAULT_HYPERLINK_ALLOWED_SCHEMES
        .iter()
        .map(|scheme| (*scheme).to_owned())
        .collect()
}

fn deserialize_lowercase_schemes<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer).map(|schemes| {
        schemes
            .into_iter()
            .map(|scheme| scheme.to_ascii_lowercase())
            .collect()
    })
}

pub fn load_app_config() -> Result<AppConfig, ConfigError> {
    let paths = config_paths_from_env(|key| env::var_os(key));
    load_app_config_from_paths(&paths)
}

fn load_app_config_from_paths(paths: &[PathBuf]) -> Result<AppConfig, ConfigError> {
    for path in paths {
        match fs::read_to_string(path) {
            Ok(contents) => {
                let config = serde_json::from_str::<AppConfig>(&contents).map_err(|source| {
                    ConfigError::Parse {
                        path: path.clone(),
                        source,
                    }
                })?;
                config.validate(Some(path))?;
                return Ok(config);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.clone(),
                    source,
                });
            }
        }
    }

    let config = AppConfig::default();
    config.validate(None)?;
    Ok(config)
}

fn config_paths_from_env(mut get_env: impl FnMut(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = get_env("KNIGHTTY_CONFIG").filter(|value| !value.is_empty()) {
        paths.push(PathBuf::from(path));
    }

    if let Some(path) = user_config_path_from_env(get_env) {
        paths.push(path);
    }

    paths
}

fn user_config_path_from_env(mut get_env: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        get_env("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("knightty").join("config.json"))
    }

    #[cfg(not(windows))]
    {
        if let Some(config_home) = get_env("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
            return Some(
                PathBuf::from(config_home)
                    .join("knightty")
                    .join("config.json"),
            );
        }

        get_env("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("knightty").join("config.json"))
    }
}

pub fn parse_wgpu_backend_name(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Some("auto"),
        "vulkan" => Some("vulkan"),
        "dx12" => Some("dx12"),
        "gl" => Some("gl"),
        _ => None,
    }
}

fn validate_positive_float(
    field: &'static str,
    value: Option<f32>,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if let Some(value) = value
        && (!value.is_finite() || value <= 0.0)
    {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: format!("expected a positive number, got `{value}`"),
        });
    }
    Ok(())
}

fn validate_positive_usize(
    field: &'static str,
    value: Option<usize>,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if matches!(value, Some(0)) {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(())
}

fn validate_non_empty_string(
    field: &'static str,
    value: Option<&str>,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if matches!(value, Some(value) if value.is_empty()) {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: "expected a non-empty string".to_owned(),
        });
    }
    Ok(())
}

fn validate_usize_range(
    field: &'static str,
    value: Option<usize>,
    min: usize,
    max: usize,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if let Some(value) = value
        && !(min..=max).contains(&value)
    {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: format!("expected an integer from {min} to {max}, got `{value}`"),
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config `{}`: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to parse config `{}`: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid config{} field `{field}`: {message}", path.as_ref().map(|path| format!(" `{}`", path.display())).unwrap_or_default())]
    InvalidValue {
        path: Option<PathBuf>,
        field: &'static str,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, ConfigError, config_paths_from_env, load_app_config_from_paths,
        user_config_path_from_env,
    };
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn missing_config_uses_defaults() {
        let config = load_app_config_from_paths(&[temp_path("missing").join("config.json")])
            .expect("missing config should fall back to defaults");

        assert_eq!(config.initial_cols(), 80);
        assert_eq!(config.initial_rows(), 24);
        assert_eq!(config.renderer_config().cell_metrics.font_size, 16.0);
        assert_eq!(config.renderer_config().font_family, None);
        assert_eq!(config.scrollback_lines(), 10_000);
        assert_eq!(config.scroll_multiplier(), 3);
        assert!(config.hyperlink_open_on_ctrl_click());
        assert_eq!(
            config.hyperlink_allowed_schemes(),
            &["https".to_owned(), "http".to_owned()]
        );
        assert!(config.hyperlink_underline_on_hover());
        assert!(config.hyperlink_open_enabled());
    }

    #[test]
    fn env_config_path_is_first_candidate() {
        let paths = config_paths_from_env(env_map([
            ("KNIGHTTY_CONFIG", "C:\\custom\\knightty.json"),
            ("APPDATA", "C:\\Users\\me\\AppData\\Roaming"),
        ]));

        assert_eq!(
            paths.first(),
            Some(&PathBuf::from("C:\\custom\\knightty.json"))
        );
    }

    #[test]
    fn explicit_path_is_loaded_before_user_config_path() {
        let explicit_dir = temp_path("explicit");
        let user_dir = temp_path("user");
        fs::create_dir_all(&explicit_dir).expect("create explicit dir");
        fs::create_dir_all(&user_dir).expect("create user dir");

        let explicit_path = explicit_dir.join("config.json");
        let user_path = user_dir.join("config.json");
        fs::write(&explicit_path, r#"{"window":{"initial_cols":120}}"#)
            .expect("write explicit config");
        fs::write(&user_path, r#"{"window":{"initial_cols":90}}"#).expect("write user config");

        let config = load_app_config_from_paths(&[explicit_path, user_path]).expect("load config");

        assert_eq!(config.initial_cols(), 120);
    }

    #[test]
    fn invalid_json_reports_parse_error() {
        let dir = temp_path("invalid-json");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("config.json");
        fs::write(&path, "{not json").expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid JSON errors");

        assert!(matches!(error, ConfigError::Parse { path: error_path, .. } if error_path == path));
    }

    #[test]
    fn font_settings_build_renderer_config() {
        let config: AppConfig = serde_json::from_str(
            r#"{"font":{"family":"CaskaydiaCove Nerd Font","size":18,"line_height":22}}"#,
        )
        .expect("parse config");

        let renderer_config = config.renderer_config();

        assert_eq!(
            renderer_config.font_family,
            Some("CaskaydiaCove Nerd Font".to_owned())
        );
        assert_eq!(renderer_config.cell_metrics.font_size, 18.0);
        assert_eq!(renderer_config.cell_metrics.line_height, 22.0);
        assert_eq!(renderer_config.cell_metrics.height, 22);
    }

    #[test]
    fn terminal_settings_are_loaded() {
        let config: AppConfig =
            serde_json::from_str(r#"{"terminal":{"scrollback_lines":2000,"scroll_multiplier":5}}"#)
                .expect("parse config");

        assert_eq!(config.scrollback_lines(), 2000);
        assert_eq!(config.scroll_multiplier(), 5);
    }

    #[test]
    fn hyperlink_allowed_schemes_are_lowercase_normalized() {
        let config: AppConfig =
            serde_json::from_str(r#"{"hyperlink":{"allowed_schemes":["HTTPS","Http"]}}"#)
                .expect("parse config");

        assert_eq!(
            config.hyperlink_allowed_schemes(),
            &["https".to_owned(), "http".to_owned()]
        );
    }

    #[test]
    fn empty_hyperlink_allowed_schemes_disable_open() {
        let config: AppConfig =
            serde_json::from_str(r#"{"hyperlink":{"allowed_schemes":[]}}"#).expect("parse config");

        assert!(config.hyperlink_open_on_ctrl_click());
        assert!(config.hyperlink_allowed_schemes().is_empty());
        assert!(!config.hyperlink_open_enabled());
    }

    #[test]
    fn shell_settings_build_shell_command() {
        let config: AppConfig =
            serde_json::from_str(r#"{"shell":{"program":"pwsh","args":["-NoLogo"]}}"#)
                .expect("parse config");

        let shell = config.shell_command().expect("shell should be configured");

        assert_eq!(shell.program, "pwsh");
        assert_eq!(shell.args, vec!["-NoLogo"]);
    }

    #[test]
    fn missing_shell_settings_use_default_shell() {
        let config = AppConfig::default();

        assert_eq!(config.shell_command(), None);
    }

    #[test]
    fn terminal_scrollback_lines_are_range_validated() {
        let dir = temp_path("invalid-scrollback-lines");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("config.json");
        fs::write(&path, r#"{"terminal":{"scrollback_lines":100001}}"#)
            .expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid scrollback_lines errors");

        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                field: "terminal.scrollback_lines",
                ..
            }
        ));
    }

    #[test]
    fn terminal_scroll_multiplier_is_range_validated() {
        let dir = temp_path("invalid-scroll-multiplier");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("config.json");
        fs::write(&path, r#"{"terminal":{"scroll_multiplier":0}}"#).expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid scroll_multiplier errors");

        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                field: "terminal.scroll_multiplier",
                ..
            }
        ));
    }

    #[test]
    fn empty_shell_program_is_invalid() {
        let dir = temp_path("invalid-shell-program");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("config.json");
        fs::write(&path, r#"{"shell":{"program":""}}"#).expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid shell program errors");

        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                field: "shell.program",
                ..
            }
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_user_config_path_uses_appdata() {
        let path = user_config_path_from_env(env_map([("APPDATA", "C:\\Users\\me\\Roaming")]))
            .expect("appdata path");

        assert_eq!(
            path,
            PathBuf::from("C:\\Users\\me\\Roaming")
                .join("knightty")
                .join("config.json")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_user_config_path_prefers_xdg_config_home() {
        let path = user_config_path_from_env(env_map([
            ("XDG_CONFIG_HOME", "/tmp/config"),
            ("HOME", "/home/me"),
        ]))
        .expect("xdg path");

        assert_eq!(path, PathBuf::from("/tmp/config/knightty/config.json"));
    }

    fn env_map<const N: usize>(
        values: [(&'static str, &'static str); N],
    ) -> impl FnMut(&str) -> Option<OsString> {
        let values = values
            .into_iter()
            .map(|(key, value)| (key, OsString::from(value)))
            .collect::<HashMap<_, _>>();

        move |key| values.get(key).cloned()
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "knightty-config-test-{}-{}-{name}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
