use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use knightty_app::config_spec::{
    config_reference_json, default_config_toml, generated_config_reference_path,
    generated_default_config_path,
};

mod demo_assets;

fn main() -> Result<(), XtaskError> {
    let mut args = env::args();
    let _program = args.next();
    let Some(command) = args.next() else {
        return Err(XtaskError::Usage);
    };

    match command.as_str() {
        "generate-config-docs" => {
            if args.next().is_some() {
                return Err(XtaskError::Usage);
            }
            generate_config_docs()
        }
        "demo-assets" => {
            let Some(action) = args.next() else {
                return Err(XtaskError::Usage);
            };
            if args.next().is_some() {
                return Err(XtaskError::Usage);
            }
            demo_assets::run(&workspace_root(), &action)?;
            Ok(())
        }
        _ => Err(XtaskError::UnknownCommand(command)),
    }
}

fn generate_config_docs() -> Result<(), XtaskError> {
    let workspace_root = workspace_root();
    let reference_path = generated_config_reference_path(&workspace_root);
    let default_config_path = generated_default_config_path(&workspace_root);

    write_generated_file(&reference_path, &config_reference_json())?;
    write_generated_file(&default_config_path, &default_config_toml())?;

    println!("generated {}", reference_path.display());
    println!("generated {}", default_config_path.display());
    Ok(())
}

fn write_generated_file(path: &Path, contents: &str) -> Result<(), XtaskError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask should live under crates/xtask")
        .to_path_buf()
}

#[derive(Debug)]
enum XtaskError {
    Usage,
    UnknownCommand(String),
    DemoAssets(demo_assets::DemoAssetError),
    Io(io::Error),
}

impl From<demo_assets::DemoAssetError> for XtaskError {
    fn from(error: demo_assets::DemoAssetError) -> Self {
        Self::DemoAssets(error)
    }
}

impl From<io::Error> for XtaskError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for XtaskError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage => write!(
                formatter,
                "usage: cargo run -p xtask -- generate-config-docs | demo-assets <build|preview>"
            ),
            Self::UnknownCommand(command) => write!(
                formatter,
                "unknown xtask command `{command}`; expected generate-config-docs or demo-assets"
            ),
            Self::DemoAssets(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for XtaskError {}
