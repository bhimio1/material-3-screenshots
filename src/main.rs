//! Material 3 Screenshots: `m3-screenshots`. See `cli::USAGE`.

mod annotate;
mod capture;
mod cli;
mod config;
mod deliver;
mod geometry;
mod history;
mod hypr;
mod naming;
mod notify;
mod record;
mod ui;

use cli::Command;
use config::Config;

fn main() {
    let code = match cli::parse(std::env::args().skip(1)) {
        Err(e) => {
            eprintln!("m3-screenshots: {e}\n\n{}", cli::USAGE);
            2
        }
        Ok(cmd) => run(cmd),
    };
    std::process::exit(code);
}

fn run(cmd: Command) -> i32 {
    match cmd {
        Command::Help => {
            print!("{}", cli::USAGE);
            0
        }
        Command::Version => {
            println!("m3-screenshots {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Status { json } => record::status(json),
        Command::Supervise(spec) => record::supervise(&spec),
        Command::Record(action) => record::run(action, &Config::load()),
        Command::Shot(mode, args) => {
            let cfg = Config::load();
            match capture::take(mode, &args, &cfg) {
                Ok(shot) => deliver::finish(shot, &args, &cfg),
                Err(capture::Error::Cancelled) => 1,
                Err(capture::Error::Failed(e)) => {
                    eprintln!("m3-screenshots: {e}");
                    if cfg.notifications && args.notify {
                        notify::error("Screenshot failed", &e);
                    }
                    1
                }
            }
        }
        Command::Edit(path) => match image::open(&path) {
            Ok(img) => ui::run_editor(img.to_rgba8(), Some(path), Config::load()),
            Err(e) => {
                eprintln!("m3-screenshots: {}: {e}", path.display());
                1
            }
        },
        Command::Import(file, args) => {
            let cfg = Config::load();
            let decoded = match &file {
                Some(path) => image::open(path).map_err(|e| format!("{}: {e}", path.display())),
                None => {
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes)
                        .map_err(|e| e.to_string())
                        .and_then(|_| image::load_from_memory(&bytes).map_err(|e| format!("stdin: {e}")))
                }
            };
            match decoded {
                Ok(img) => {
                    let shot = capture::Captured { image: img.to_rgba8(), mode: cli::Mode::Region, label: None };
                    deliver::finish(shot, &args, &cfg)
                }
                Err(e) => {
                    eprintln!("m3-screenshots: {e}");
                    1
                }
            }
        }
        Command::App(page) => ui::run_app(page, Config::load()),
    }
}
