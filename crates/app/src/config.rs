use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use knightty_render::{CellMetrics, RendererConfig};
use serde::Deserialize;
use thiserror::Error;

const DEFAULT_INITIAL_COLS: usize = 80;
const DEFAULT_INITIAL_ROWS: usize = 24;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub render: RenderConfig,
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

    fn validate(&self, source: Option<&Path>) -> Result<(), ConfigError> {
        validate_positive_float("font.size", self.font.size, source)?;
        validate_positive_float("font.line_height", self.font.line_height, source)?;
        validate_positive_usize("window.initial_cols", self.window.initial_cols, source)?;
        validate_positive_usize("window.initial_rows", self.window.initial_rows, source)?;
        if let Some(value) = &self.render.wgpu_backend {
            if parse_wgpu_backend_name(value).is_none() {
                return Err(ConfigError::InvalidValue {
                    path: source.map(Path::to_path_buf),
                    field: "render.wgpu_backend",
                    message: format!("expected one of auto, vulkan, dx12, or gl, got `{value}`"),
                });
            }
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
    if let Some(value) = value {
        if !value.is_finite() || value <= 0.0 {
            return Err(ConfigError::InvalidValue {
                path: source.map(Path::to_path_buf),
                field,
                message: format!("expected a positive number, got `{value}`"),
            });
        }
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

        let error = load_app_config_from_paths(&[path.clone()]).expect_err("invalid JSON errors");

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
