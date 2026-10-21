//! Command-line parsing. Hand-rolled so a keybind's `m3-screenshots region`
//! starts in milliseconds and the grammar stays exactly what the shell binds.

use std::path::PathBuf;

pub const USAGE: &str = "\
Material 3 Screenshots

Usage:
  m3-screenshots                          open the app
  m3-screenshots region|window|output|all [options]
                                          take a screenshot
  m3-screenshots record region|window|output [--audio|--no-audio] [--toggle]
                                          start (or with --toggle, stop) a recording
  m3-screenshots record stop              stop the running recording
  m3-screenshots status [--json]          is a recording running?
  m3-screenshots edit FILE                annotate an image
  m3-screenshots history                  open the history page
  m3-screenshots settings                 open the settings page

Screenshot options:
  --edit           open the editor with the capture
  --copy           copy it to the clipboard
  --save           save it to the screenshots folder
                   (with neither --copy nor --save, the settings decide)
  --delay N        wait N seconds before capturing
  --freeze         freeze the screen while you select (default: settings)
  --no-freeze      don't freeze
  --active         window/output: take the focused one instead of picking
  --no-notify      don't show a notification

  -h, --help       show this help
  -V, --version    show the version
";

/// What to capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Region,
    Window,
    Output,
    All,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "region" | "area" => Mode::Region,
            "window" => Mode::Window,
            "output" | "screen" | "monitor" => Mode::Output,
            "all" | "full" => Mode::All,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Region => "region",
            Mode::Window => "window",
            Mode::Output => "output",
            Mode::All => "all",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShotArgs {
    pub edit: bool,
    pub copy: bool,
    pub save: bool,
    pub delay: u32,
    /// `None`: use the setting.
    pub freeze: Option<bool>,
    pub active: bool,
    pub notify: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecordAction {
    Start {
        mode: Mode,
        /// `None`: use the setting.
        audio: Option<bool>,
        toggle: bool,
        active: bool,
    },
    Stop,
}

/// Which page the window opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Capture,
    History,
    Settings,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    App(Page),
    Shot(Mode, ShotArgs),
    Record(RecordAction),
    Status { json: bool },
    Edit(PathBuf),
    Help,
    Version,
    /// Internal: watch a recorder process (see `record.rs`).
    Supervise(String),
}

pub fn parse<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        return Ok(Command::Help);
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        return Ok(Command::Version);
    }
    let Some(first) = args.first() else {
        return Ok(Command::App(Page::Capture));
    };
    let rest = &args[1..];
    match first.as_str() {
        "history" => no_more(rest, Command::App(Page::History)),
        "settings" => no_more(rest, Command::App(Page::Settings)),
        "app" | "open" => no_more(rest, Command::App(Page::Capture)),
        "status" => {
            let mut json = false;
            for a in rest {
                match a.as_str() {
                    "--json" => json = true,
                    other => return Err(format!("unknown option for status: {other}")),
                }
            }
            Ok(Command::Status { json })
        }
        "edit" => match rest {
            [file] => Ok(Command::Edit(PathBuf::from(file))),
            [] => Err("edit needs a file".into()),
            _ => Err("edit takes one file".into()),
        },
        "record" => parse_record(rest),
        "__supervise" => match rest {
            [spec] => Ok(Command::Supervise(spec.clone())),
            _ => Err("internal command misused".into()),
        },
        word => match Mode::parse(word) {
            Some(mode) => parse_shot(mode, rest),
            None => Err(format!("unknown command: {word}")),
        },
    }
}

fn no_more(rest: &[String], cmd: Command) -> Result<Command, String> {
    match rest.first() {
        Some(extra) => Err(format!("unexpected argument: {extra}")),
        None => Ok(cmd),
    }
}

fn parse_shot(mode: Mode, rest: &[String]) -> Result<Command, String> {
    let mut a = ShotArgs { notify: true, ..Default::default() };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--edit" | "-e" => a.edit = true,
            "--copy" | "-c" => a.copy = true,
            "--save" | "-s" => a.save = true,
            "--freeze" | "-f" => a.freeze = Some(true),
            "--no-freeze" => a.freeze = Some(false),
            "--active" | "-a" => a.active = true,
            "--no-notify" => a.notify = false,
            "--delay" | "-d" => {
                let n = it.next().ok_or("--delay needs a number of seconds")?;
                a.delay = parse_delay(n)?;
            }
            other => {
                if let Some(n) = other.strip_prefix("--delay=") {
                    a.delay = parse_delay(n)?;
                } else {
                    return Err(format!("unknown option: {other}"));
                }
            }
        }
    }
    if a.active && !matches!(mode, Mode::Window | Mode::Output) {
        return Err("--active only applies to window and output".into());
    }
    Ok(Command::Shot(mode, a))
}

fn parse_delay(s: &str) -> Result<u32, String> {
    match s.parse::<u32>() {
        Ok(n) if n <= 600 => Ok(n),
        _ => Err(format!("--delay wants whole seconds (0–600), not {s:?}")),
    }
}

fn parse_record(rest: &[String]) -> Result<Command, String> {
    let Some(first) = rest.first() else {
        return Err("record needs region, window, output or stop".into());
    };
    if first == "stop" {
        return no_more(&rest[1..], Command::Record(RecordAction::Stop));
    }
    let mode = match Mode::parse(first) {
        Some(Mode::All) => return Err("record can't span every output; pick one".into()),
        Some(m) => m,
        None => return Err(format!("unknown recording target: {first}")),
    };
    let (mut audio, mut toggle, mut active) = (None, false, false);
    for arg in &rest[1..] {
        match arg.as_str() {
            "--audio" => audio = Some(true),
            "--no-audio" => audio = Some(false),
            "--toggle" | "-t" => toggle = true,
            "--active" | "-a" => active = true,
            other => return Err(format!("unknown option for record: {other}")),
        }
    }
    if active && mode == Mode::Region {
        return Err("--active only applies to window and output".into());
    }
    Ok(Command::Record(RecordAction::Start { mode, audio, toggle, active }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Command, String> {
        parse(s.split_whitespace())
    }

    #[test]
    fn no_arguments_opens_the_app() {
        assert_eq!(p(""), Ok(Command::App(Page::Capture)));
        assert_eq!(p("history"), Ok(Command::App(Page::History)));
        assert_eq!(p("settings"), Ok(Command::App(Page::Settings)));
    }

    #[test]
    fn screenshot_flags() {
        let Ok(Command::Shot(Mode::Region, a)) = p("region --edit --copy --delay 3 --freeze") else {
            panic!()
        };
        assert!(a.edit && a.copy && !a.save && a.notify);
        assert_eq!(a.delay, 3);
        assert_eq!(a.freeze, Some(true));

        let Ok(Command::Shot(Mode::Output, a)) = p("output --active --save --delay=2 --no-notify") else {
            panic!()
        };
        assert!(a.active && a.save && !a.notify);
        assert_eq!(a.delay, 2);
        assert_eq!(a.freeze, None);
        assert!(matches!(p("screen"), Ok(Command::Shot(Mode::Output, _))));
        assert!(matches!(p("all --no-freeze"), Ok(Command::Shot(Mode::All, ShotArgs { freeze: Some(false), .. }))));
    }

    #[test]
    fn screenshot_errors() {
        assert!(p("region --delay").is_err());
        assert!(p("region --delay soon").is_err());
        assert!(p("region --delay 9999").is_err());
        assert!(p("region --active").is_err());
        assert!(p("region --bogus").is_err());
        assert!(p("selfie").is_err());
    }

    #[test]
    fn recording() {
        assert_eq!(
            p("record region --audio --toggle"),
            Ok(Command::Record(RecordAction::Start {
                mode: Mode::Region,
                audio: Some(true),
                toggle: true,
                active: false
            }))
        );
        assert_eq!(p("record stop"), Ok(Command::Record(RecordAction::Stop)));
        assert!(matches!(
            p("record output --active --no-audio"),
            Ok(Command::Record(RecordAction::Start { mode: Mode::Output, audio: Some(false), active: true, .. }))
        ));
        assert!(p("record").is_err());
        assert!(p("record all").is_err());
        assert!(p("record region --active").is_err());
        assert!(p("record stop now").is_err());
    }

    #[test]
    fn misc() {
        assert_eq!(p("status --json"), Ok(Command::Status { json: true }));
        assert_eq!(p("edit /tmp/a.png"), Ok(Command::Edit("/tmp/a.png".into())));
        assert!(p("edit").is_err());
        assert_eq!(p("region --help"), Ok(Command::Help));
        assert_eq!(p("-V"), Ok(Command::Version));
        assert!(p("history now").is_err());
    }
}
