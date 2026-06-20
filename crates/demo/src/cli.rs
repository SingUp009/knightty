use std::time::Duration;

use thiserror::Error;

pub const DEFAULT_FPS: u32 = 60;
pub const MAX_FPS: u32 = 240;
pub const MAX_RUN_SECONDS: f64 = 24.0 * 60.0 * 60.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemoConfig {
    pub fps: u32,
    pub duration: Option<Duration>,
    pub show_stats: bool,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            fps: DEFAULT_FPS,
            duration: None,
            show_stats: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StartupAction {
    Run(DemoConfig),
    Help,
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<StartupAction, CliError> {
    let mut args = args.into_iter();
    let _program = args.next();
    let mut config = DemoConfig::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(StartupAction::Help),
            "--no-stats" => config.show_stats = false,
            "--fps" => {
                let value = args
                    .next()
                    .ok_or(CliError::MissingValue { option: "--fps" })?;
                config.fps = parse_fps(&value)?;
            }
            "--duration" => {
                let value = args.next().ok_or(CliError::MissingValue {
                    option: "--duration",
                })?;
                config.duration = Some(parse_duration(&value)?);
            }
            _ if arg.starts_with("--fps=") => {
                config.fps = parse_fps(&arg["--fps=".len()..])?;
            }
            _ if arg.starts_with("--duration=") => {
                config.duration = Some(parse_duration(&arg["--duration=".len()..])?);
            }
            _ if arg.starts_with('-') => return Err(CliError::UnknownOption(arg)),
            _ => return Err(CliError::UnexpectedArgument(arg)),
        }
    }

    Ok(StartupAction::Run(config))
}

fn parse_fps(value: &str) -> Result<u32, CliError> {
    let fps: i64 = value
        .parse()
        .map_err(|_| CliError::InvalidFps(value.to_owned()))?;
    if fps < 0 {
        return Err(CliError::InvalidFps(value.to_owned()));
    }
    let fps = fps as u32;
    if fps > MAX_FPS {
        return Err(CliError::FpsTooHigh {
            value: fps,
            max: MAX_FPS,
        });
    }
    Ok(fps)
}

fn parse_duration(value: &str) -> Result<Duration, CliError> {
    let seconds: f64 = value
        .parse()
        .map_err(|_| CliError::InvalidDuration(value.to_owned()))?;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > MAX_RUN_SECONDS {
        return Err(CliError::InvalidDuration(value.to_owned()));
    }
    Ok(Duration::from_secs_f64(seconds))
}

pub fn usage() -> &'static str {
    "Usage: knightty-demo [--fps <number>] [--duration <seconds>] [--no-stats]\n\
\n\
Options:\n\
  --fps <number>        Target FPS. Use 0 for uncapped. Default: 60. Max: 240.\n\
  --duration <seconds> Stop after the given number of seconds.\n\
  --no-stats           Do not print the performance report after exit.\n\
  -h, --help           Show this help text.\n\
\n\
Controls:\n\
  q or Escape          Exit the demo.\n\
  Space                Pause or resume.\n"
}

#[derive(Debug, Error, PartialEq)]
pub enum CliError {
    #[error("missing value for {option}")]
    MissingValue { option: &'static str },
    #[error("invalid --fps value `{0}`; expected an integer from 0 to {MAX_FPS}")]
    InvalidFps(String),
    #[error("invalid --fps value `{value}`; maximum supported value is {max}")]
    FpsTooHigh { value: u32, max: u32 },
    #[error("invalid --duration value `{0}`; expected seconds in the range 0 < seconds <= 86400")]
    InvalidDuration(String),
    #[error("unknown option `{0}`")]
    UnknownOption(String),
    #[error("unexpected argument `{0}`")]
    UnexpectedArgument(String),
}

#[cfg(test)]
mod tests {
    use super::{DemoConfig, StartupAction, parse_args};

    #[test]
    fn parse_args_uses_60_fps_by_default() {
        assert_eq!(
            parse_args(["knightty-demo"].map(str::to_owned)).unwrap(),
            StartupAction::Run(DemoConfig::default())
        );
    }

    #[test]
    fn parse_args_accepts_uncapped_fps() {
        let StartupAction::Run(config) =
            parse_args(["knightty-demo", "--fps", "0"].map(str::to_owned)).unwrap()
        else {
            panic!("expected run config");
        };
        assert_eq!(config.fps, 0);
    }

    #[test]
    fn parse_args_rejects_negative_fps() {
        assert!(parse_args(["knightty-demo", "--fps", "-1"].map(str::to_owned)).is_err());
    }
}
