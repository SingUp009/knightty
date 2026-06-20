mod animation;
mod canvas;
mod cli;
mod encoder;
mod metrics;
mod player;
mod raster;
mod terminal;

use std::process::ExitCode;

use cli::StartupAction;

fn main() -> ExitCode {
    match cli::parse_args(std::env::args()) {
        Ok(StartupAction::Help) => {
            print!("{}", cli::usage());
            ExitCode::SUCCESS
        }
        Ok(StartupAction::Run(config)) => match player::run(config) {
            Ok(report) => {
                if let Some(report) = report {
                    print!("{report}");
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("knightty-demo: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("knightty-demo: {error}");
            eprintln!();
            eprintln!("{}", cli::usage());
            ExitCode::FAILURE
        }
    }
}
