//! Screen recording with gpu-screen-recorder (preferred) or wf-recorder.
//!
//! `record …` picks the target, then starts a small supervisor
//! (`m3-screenshots __supervise SPEC`) in its own process group and returns.
//! The supervisor runs the recorder, writes the state file the shell's
//! indicator watches, and when the recorder exits it removes the state and
//! sends the "Recording saved" notification. Stopping (`record stop`, or
//! `record … --toggle` while recording) sends the recorder SIGINT, which both
//! recorders treat as "finish the file".
//!
//! State: `$XDG_RUNTIME_DIR/m3-screenshots/recording.json`.

use crate::cli::{Mode, RecordAction};
use crate::config::{Config, RecordQuality, RecorderBackend};
use crate::geometry::{self, Rect};
use crate::notify::{Notification, Notifier, Outcome};
use crate::{capture, deliver, hypr, naming, notify};
use material_3_ui::sys::cmd;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    GpuScreenRecorder,
    WfRecorder,
}

impl Backend {
    pub fn program(self) -> &'static str {
        match self {
            Backend::GpuScreenRecorder => "gpu-screen-recorder",
            Backend::WfRecorder => "wf-recorder",
        }
    }
}

/// Everything the supervisor needs, passed as JSON.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Spec {
    pub backend: Backend,
    pub file: PathBuf,
    pub mode: String,
    /// A region (logical pixels) or, for a whole output, its name.
    pub rect: Option<Rect>,
    pub output: Option<String>,
    pub fps: u32,
    pub quality: RecordQuality,
    /// gpu-screen-recorder: "default_output"; wf-recorder: a PulseAudio
    /// source name (the default sink's `.monitor`).
    pub audio: Option<String>,
    pub cursor: bool,
    pub notify: bool,
}

/// What the state file holds.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct State {
    pub recording: bool,
    pub pid: u32,
    pub supervisor: u32,
    pub backend: Backend,
    pub file: PathBuf,
    pub mode: String,
    pub audio: bool,
    /// Unix seconds.
    pub started: i64,
}

// serde for the geometry type, kept here so geometry.rs stays plain.
impl Serialize for Rect {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_grim())
    }
}

impl<'de> Deserialize<'de> for Rect {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rect::parse(&s).ok_or_else(|| serde::de::Error::custom("bad rectangle"))
    }
}

pub fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::env::temp_dir().join(format!("m3-screenshots-{}", unsafe { libc::getuid() })));
    base.join("m3-screenshots")
}

pub fn state_path() -> PathBuf {
    runtime_dir().join("recording.json")
}

fn alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn signal(pid: u32, sig: libc::c_int) {
    if pid != 0 {
        // SAFETY: plain kill(2).
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
}

/// The running recording, if any. A stale file (recorder gone) is removed.
pub fn current() -> Option<State> {
    let path = state_path();
    let state: State = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    if alive(state.pid) {
        Some(state)
    } else {
        if !alive(state.supervisor) {
            let _ = std::fs::remove_file(&path);
        }
        None
    }
}

fn write_state(state: &State) -> Result<(), String> {
    let path = state_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(state).unwrap()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// end4-pC's own recording indicator reads `.record.enable` from the
/// shell's states.json; keep it in step when that file exists.
fn set_shell_indicator(on: bool) {
    let path = material_3_ui::sys::paths::home().join(".local/state/quickshell/states.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(obj) = json.as_object_mut() else {
        return;
    };
    let record = obj.entry("record").or_insert_with(|| serde_json::json!({}));
    if let Some(r) = record.as_object_mut() {
        r.insert("enable".into(), serde_json::Value::Bool(on));
        let tmp = path.with_extension("json.m3tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&json).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub fn pick_backend(pref: RecorderBackend, has: impl Fn(&str) -> bool) -> Result<Backend, String> {
    let gsr = has("gpu-screen-recorder");
    let wf = has("wf-recorder");
    match pref {
        RecorderBackend::GpuScreenRecorder if gsr => Ok(Backend::GpuScreenRecorder),
        RecorderBackend::WfRecorder if wf => Ok(Backend::WfRecorder),
        RecorderBackend::GpuScreenRecorder => Err("gpu-screen-recorder isn't installed.".into()),
        RecorderBackend::WfRecorder => Err("wf-recorder isn't installed (pacman -S wf-recorder).".into()),
        RecorderBackend::Auto if gsr => Ok(Backend::GpuScreenRecorder),
        RecorderBackend::Auto if wf => Ok(Backend::WfRecorder),
        RecorderBackend::Auto => Err("No screen recorder is installed: install gpu-screen-recorder or wf-recorder.".into()),
    }
}

/// The recorder's command line.
pub fn command(spec: &Spec) -> Vec<String> {
    let mut a: Vec<String> = vec![spec.backend.program().into()];
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    let fps = spec.fps.clamp(1, 240).to_string();
    match spec.backend {
        Backend::GpuScreenRecorder => {
            match (&spec.output, &spec.rect) {
                (Some(out), _) => push(&["-w", out]),
                (None, Some(r)) => push(&["-w", "region", "-region", &r.to_wxh_xy()]),
                (None, None) => push(&["-w", "screen"]),
            }
            let q = match spec.quality {
                RecordQuality::Medium => "medium",
                RecordQuality::High => "high",
                RecordQuality::VeryHigh => "very_high",
                RecordQuality::Ultra => "ultra",
            };
            push(&["-f", &fps, "-q", q, "-c", "mp4", "-cursor", if spec.cursor { "yes" } else { "no" }]);
            if let Some(src) = &spec.audio {
                push(&["-a", src]);
            }
            push(&["-o", &spec.file.to_string_lossy()]);
        }
        Backend::WfRecorder => {
            match (&spec.output, &spec.rect) {
                (Some(out), _) => push(&["-o", out]),
                (None, Some(r)) => push(&["-g", &r.to_grim()]),
                (None, None) => {}
            }
            let crf = match spec.quality {
                RecordQuality::Medium => "28",
                RecordQuality::High => "23",
                RecordQuality::VeryHigh => "20",
                RecordQuality::Ultra => "17",
            };
            push(&["-r", &fps, "-p", &format!("crf={crf}")]);
            if let Some(src) = &spec.audio {
                push(&[&format!("--audio={src}")]);
            }
            push(&["-f", &spec.file.to_string_lossy()]);
        }
    }
    a
}

fn default_sink_monitor() -> Option<String> {
    let sink = cmd::run("pactl", &["get-default-sink"]).ok()?;
    let sink = sink.trim();
    (!sink.is_empty()).then(|| format!("{sink}.monitor"))
}

/// `m3-screenshots record …`. Returns the exit code.
pub fn run(action: RecordAction, cfg: &Config) -> i32 {
    match action {
        RecordAction::Stop => match stop() {
            Ok(Some(file)) => {
                println!("{}", file.display());
                0
            }
            Ok(None) => {
                eprintln!("m3-screenshots: nothing is being recorded");
                1
            }
            Err(e) => fail("Couldn't stop the recording", &e, cfg),
        },
        RecordAction::Start { mode, audio, toggle, active } => {
            if let Some(state) = current() {
                if toggle {
                    return match stop() {
                        Ok(_) => {
                            println!("{}", state.file.display());
                            0
                        }
                        Err(e) => fail("Couldn't stop the recording", &e, cfg),
                    };
                }
                eprintln!("m3-screenshots: already recording {} (stop it with `m3-screenshots record stop`)", state.file.display());
                return 1;
            }
            match start(mode, audio.unwrap_or(cfg.recorder.audio), active, cfg) {
                Ok(file) => {
                    println!("{}", file.display());
                    0
                }
                Err(capture::Error::Cancelled) => 1,
                Err(capture::Error::Failed(e)) => fail("Couldn't start recording", &e, cfg),
            }
        }
    }
}

fn fail(summary: &str, e: &str, cfg: &Config) -> i32 {
    eprintln!("m3-screenshots: {e}");
    if cfg.notifications {
        notify::error(summary, e);
    }
    1
}

fn start(mode: Mode, audio: bool, active: bool, cfg: &Config) -> Result<PathBuf, capture::Error> {
    let backend = pick_backend(cfg.recorder.backend, cmd::exists)?;
    let monitors = hypr::monitors()?;
    let bounds = geometry::layout_bounds(&monitors).ok_or("No outputs.")?;
    let target = capture::resolve(mode, active, &monitors, bounds, cfg)?;
    let audio = if audio {
        match backend {
            Backend::GpuScreenRecorder => Some("default_output".to_string()),
            Backend::WfRecorder => default_sink_monitor(),
        }
    } else {
        None
    };
    let dir = cfg.recordings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stem = naming::expand(&cfg.recorder.filename_pattern, chrono::Local::now(), mode.as_str(), target.label.as_deref());
    let file = naming::unique_path(&dir, &stem, "mp4");
    let spec = Spec {
        backend,
        file: file.clone(),
        mode: mode.as_str().into(),
        rect: target.output.is_none().then_some(target.rect),
        output: target.output,
        fps: cfg.recorder.fps,
        quality: cfg.recorder.quality,
        audio,
        cursor: cfg.recorder.show_cursor,
        notify: cfg.notifications,
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let json = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(runtime_dir().join("recorder.log"));
    cmd::spawn_detached(&exe.to_string_lossy(), &["__supervise", &json])?;
    // Report back once the recorder is really running (or has failed).
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if current().is_some_and(|s| s.file == file) {
            return Ok(file);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    // The supervisor has already sent the "Recording failed" notification.
    eprintln!("m3-screenshots: {} didn't start. {}", backend.program(), log_tail());
    Err(capture::Error::Cancelled)
}

fn log_tail() -> String {
    let text = std::fs::read_to_string(runtime_dir().join("recorder.log")).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join("\n")
}

/// Stop the running recording and wait for the file to be finished.
pub fn stop() -> Result<Option<PathBuf>, String> {
    let Some(state) = current() else {
        return Ok(None);
    };
    signal(state.pid, libc::SIGINT);
    let deadline = Instant::now() + Duration::from_secs(15);
    while alive(state.pid) {
        if Instant::now() > deadline {
            signal(state.pid, libc::SIGTERM);
            return Err(format!("{} didn't stop in time; sent it SIGTERM.", state.backend.program()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The supervisor removes the state; do it ourselves if it's gone.
    let deadline = Instant::now() + Duration::from_secs(2);
    while state_path().exists() && alive(state.supervisor) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(30));
    }
    if !alive(state.supervisor) {
        let _ = std::fs::remove_file(state_path());
        set_shell_indicator(false);
    }
    Ok(Some(state.file))
}

/// `__supervise SPEC`: run the recorder until it exits.
pub fn supervise(spec_json: &str) -> i32 {
    let Ok(spec) = serde_json::from_str::<Spec>(spec_json) else {
        eprintln!("m3-screenshots: bad recording spec");
        return 2;
    };
    let argv = command(&spec);
    let dir = runtime_dir();
    let _ = std::fs::create_dir_all(&dir);
    let log = std::fs::File::create(dir.join("recorder.log")).ok();
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null());
    match log {
        Some(f) => command.stderr(f),
        None => command.stderr(Stdio::null()),
    };
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            if spec.notify {
                notify::error("Couldn't start recording", &format!("{}: {e}", argv[0]));
            }
            return 1;
        }
    };
    let state = State {
        recording: true,
        pid: child.id(),
        supervisor: std::process::id(),
        backend: spec.backend,
        file: spec.file.clone(),
        mode: spec.mode.clone(),
        audio: spec.audio.is_some(),
        started: chrono::Local::now().timestamp(),
    };
    if let Err(e) = write_state(&state) {
        eprintln!("m3-screenshots: couldn't write the recording state: {e}");
    }
    set_shell_indicator(true);
    let started = Instant::now();
    let status = child.wait();
    let _ = std::fs::remove_file(state_path());
    set_shell_indicator(false);

    let ok_file = std::fs::metadata(&spec.file).map(|m| m.len() > 0).unwrap_or(false);
    if !ok_file {
        if spec.notify {
            let why = match status {
                Ok(s) => format!("{} exited ({s}).\n{}", spec.backend.program(), log_tail()),
                Err(e) => e.to_string(),
            };
            notify::error("Recording failed", why.trim());
        }
        return 1;
    }
    if spec.notify {
        notify_saved(&spec.file, started.elapsed());
    }
    0
}

fn notify_saved(file: &Path, took: Duration) {
    let Ok(notifier) = Notifier::connect() else {
        return;
    };
    let secs = took.as_secs();
    let n = Notification {
        summary: "Recording saved".into(),
        body: format!(
            "{}  ·  {}:{:02}",
            file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            secs / 60,
            secs % 60
        ),
        icon: "media-record".into(),
        image: None,
        actions: vec![
            ("default".into(), "Play".into()),
            ("folder".into(), "Show in folder".into()),
            ("delete".into(), "Delete".into()),
        ],
        timeout_ms: 8000,
        urgency: 1,
    };
    let Ok(id) = notifier.send(&n) else {
        return;
    };
    if let Outcome::Action(key) = notifier.wait(id, Duration::from_secs(300)) {
        match key.as_str() {
            "folder" => deliver::open_folder(file.parent().unwrap_or(Path::new("/"))),
            "delete" => {
                let _ = std::fs::remove_file(file);
                notifier.close(id);
            }
            _ => deliver::open_file(file),
        }
    }
}

/// `m3-screenshots status [--json]`.
pub fn status(json: bool) -> i32 {
    let state = current();
    if json {
        let value = match &state {
            Some(s) => serde_json::json!({
                "recording": true,
                "file": s.file,
                "mode": s.mode,
                "backend": s.backend,
                "audio": s.audio,
                "started": s.started,
                "elapsed": chrono::Local::now().timestamp() - s.started,
                "pid": s.pid,
            }),
            None => serde_json::json!({ "recording": false }),
        };
        println!("{value}");
        return 0;
    }
    match state {
        Some(s) => {
            println!("recording {} ({}s)", s.file.display(), chrono::Local::now().timestamp() - s.started);
            0
        }
        None => {
            println!("idle");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(backend: Backend) -> Spec {
        Spec {
            backend,
            file: "/v/rec.mp4".into(),
            mode: "region".into(),
            rect: Some(Rect::new(10, 20, 300, 200)),
            output: None,
            fps: 60,
            quality: RecordQuality::High,
            audio: None,
            cursor: true,
            notify: true,
        }
    }

    #[test]
    fn gpu_screen_recorder_args() {
        let mut s = spec(Backend::GpuScreenRecorder);
        assert_eq!(
            command(&s).join(" "),
            "gpu-screen-recorder -w region -region 300x200+10+20 -f 60 -q high -c mp4 -cursor yes -o /v/rec.mp4"
        );
        s.output = Some("DP-1".into());
        s.audio = Some("default_output".into());
        s.quality = RecordQuality::Ultra;
        s.cursor = false;
        assert_eq!(
            command(&s).join(" "),
            "gpu-screen-recorder -w DP-1 -f 60 -q ultra -c mp4 -cursor no -a default_output -o /v/rec.mp4"
        );
    }

    #[test]
    fn wf_recorder_args() {
        let mut s = spec(Backend::WfRecorder);
        s.fps = 30;
        assert_eq!(command(&s), ["wf-recorder", "-g", "10,20 300x200", "-r", "30", "-p", "crf=23", "-f", "/v/rec.mp4"]);
        s.output = Some("eDP-1".into());
        s.audio = Some("alsa_output.pci.monitor".into());
        assert_eq!(
            command(&s).join(" "),
            "wf-recorder -o eDP-1 -r 30 -p crf=23 --audio=alsa_output.pci.monitor -f /v/rec.mp4"
        );
    }

    #[test]
    fn backend_choice() {
        let both = |_: &str| true;
        let only_wf = |p: &str| p == "wf-recorder";
        let none = |_: &str| false;
        assert_eq!(pick_backend(RecorderBackend::Auto, both), Ok(Backend::GpuScreenRecorder));
        assert_eq!(pick_backend(RecorderBackend::Auto, only_wf), Ok(Backend::WfRecorder));
        assert_eq!(pick_backend(RecorderBackend::WfRecorder, both), Ok(Backend::WfRecorder));
        assert!(pick_backend(RecorderBackend::GpuScreenRecorder, only_wf).is_err());
        assert!(pick_backend(RecorderBackend::Auto, none).is_err());
    }

    #[test]
    fn spec_round_trips() {
        let s = spec(Backend::WfRecorder);
        let back: Spec = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn liveness() {
        assert!(alive(std::process::id()));
        assert!(!alive(0));
    }
}
