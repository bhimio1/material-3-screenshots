//! Taking the picture: pick what to capture (slurp, hyprctl), then grab it
//! (grim).
//!
//! **Freezing.** With freeze on, the whole layout is grabbed *before* you
//! select, and `hyprpicker -r -z` shows that still frame while slurp runs on
//! top. The result is cropped from the first grab, so it shows exactly what
//! was on screen when the key was pressed: menus, tooltips and video frames
//! don't move or vanish while you drag. hyprpicker only provides the visual
//! freeze; without it the screen keeps moving during selection but the
//! picture is still the frozen one.

use crate::cli::{Mode, ShotArgs};
use crate::config::Config;
use crate::geometry::{self, Monitor, Rect};
use crate::hypr;
use image::RgbaImage;
use material_3_ui::sys::cmd;
use material_3_ui::theme::SlurpColors;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[derive(Debug)]
pub enum Error {
    /// The user pressed Escape in slurp.
    Cancelled,
    Failed(String),
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Failed(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Failed(s.into())
    }
}

pub struct Captured {
    pub image: RgbaImage,
    pub mode: Mode,
    /// The window's class or the output's name, for `{window}`.
    pub label: Option<String>,
}

/// What a selection resolved to.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub rect: Rect,
    pub label: Option<String>,
    /// Set when the target is exactly one output.
    pub output: Option<String>,
}

pub fn needs_selection(mode: Mode, active: bool) -> bool {
    match mode {
        Mode::Region => true,
        Mode::Window | Mode::Output => !active,
        Mode::All => false,
    }
}

pub fn take(mode: Mode, args: &ShotArgs, cfg: &Config) -> Result<Captured, Error> {
    if !cmd::exists("grim") {
        return Err(Error::Failed("grim isn't installed (pacman -S grim).".into()));
    }
    let monitors = hypr::monitors()?;
    let bounds = geometry::layout_bounds(&monitors).ok_or("No outputs.")?;
    let select = needs_selection(mode, args.active);
    let freeze = select && args.freeze.unwrap_or(cfg.freeze);
    let delay = Duration::from_secs(args.delay as u64);

    // With freezing, the delay comes first: it's the moment being frozen.
    if freeze && !delay.is_zero() {
        std::thread::sleep(delay);
    }
    let frozen = if freeze { Some(grab(&["-"])?) } else { None };
    let overlay = if freeze { FreezeOverlay::start() } else { None };
    let target = resolve(mode, args.active, &monitors, bounds, cfg);
    drop(overlay);
    let target = target?;
    if !freeze && !delay.is_zero() {
        std::thread::sleep(delay);
    }

    let image = match frozen {
        Some(full) => {
            let (x, y, w, h) = geometry::crop_in_capture(&target.rect, &bounds, full.width(), full.height())
                .ok_or("The selection is outside every output.")?;
            let mut img = image::imageops::crop_imm(&full, x, y, w, h).to_image();
            // A whole lower-scale output: back to its own pixels.
            if let Some(m) = target.output.as_ref().and_then(|n| monitors.iter().find(|m| &m.name == n)) {
                let (nw, nh) = native_size(m);
                if (nw, nh) != (img.width(), img.height()) {
                    img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::CatmullRom);
                }
            }
            img
        }
        None => match (&target.output, mode) {
            (_, Mode::All) => grab(&["-"])?,
            (Some(name), _) => grab(&["-o", name, "-"])?,
            (None, _) => grab(&["-g", &target.rect.to_grim(), "-"])?,
        },
    };
    Ok(Captured { image, mode, label: target.label })
}

fn native_size(m: &Monitor) -> (u32, u32) {
    let (w, h) = if m.transform % 2 == 1 { (m.height, m.width) } else { (m.width, m.height) };
    (w.max(1) as u32, h.max(1) as u32)
}

/// Work out the rectangle for `mode`.
pub fn resolve(mode: Mode, active: bool, monitors: &[Monitor], bounds: Rect, cfg: &Config) -> Result<Target, Error> {
    let output_target = |m: &Monitor| Target { rect: m.logical(), label: Some(m.name.clone()), output: Some(m.name.clone()) };
    match (mode, active) {
        (Mode::All, _) => Ok(Target { rect: bounds, label: None, output: None }),
        (Mode::Output, true) => {
            let m = monitors.iter().find(|m| m.focused).unwrap_or(&monitors[0]);
            Ok(output_target(m))
        }
        (Mode::Window, true) => {
            let c = hypr::active_window()?;
            Ok(Target { rect: c.rect, label: Some(c.class), output: None })
        }
        (Mode::Output, false) => {
            let (rect, _) = slurp(&["-o", "-r"], "", cfg)?;
            // slurp -o returns the output's logical box; find which one.
            let m = monitors
                .iter()
                .max_by_key(|m| m.logical().intersect(&rect).map(|i| i.w as i64 * i.h as i64).unwrap_or(0))
                .ok_or("No outputs.")?;
            Ok(output_target(m))
        }
        (Mode::Window, false) | (Mode::Region, _) => {
            let clients = hypr::clients(monitors).unwrap_or_default();
            let boxes = window_boxes(&clients);
            if mode == Mode::Window && boxes.is_empty() {
                return Err(Error::Failed("There are no windows on screen.".into()));
            }
            // In region mode the windows are offered too: click one to
            // take it, or drag a rectangle.
            let extra: &[&str] = if mode == Mode::Window { &["-r"] } else { &[] };
            let (rect, label) = slurp(extra, &boxes, cfg)?;
            let rect = rect.intersect(&bounds).ok_or("The selection is outside every output.")?;
            Ok(Target { rect, label, output: None })
        }
    }
}

/// slurp's stdin format: one `x,y wxh label` per line.
pub fn window_boxes(clients: &[hypr::Client]) -> String {
    clients
        .iter()
        .map(|c| format!("{} {}\n", c.rect.to_grim(), c.class.replace(['\n', '\r'], " ")))
        .collect()
}

/// Split slurp's `x,y wxh label` answer.
pub fn parse_slurp(out: &str) -> Option<(Rect, Option<String>)> {
    let line = out.lines().next()?.trim();
    let rect = Rect::parse(line)?;
    let label = line.splitn(3, ' ').nth(2).map(str::trim).filter(|l| !l.is_empty()).map(String::from);
    Some((rect, label))
}

fn slurp(extra: &[&str], boxes: &str, cfg: &Config) -> Result<(Rect, Option<String>), Error> {
    if !cmd::exists("slurp") {
        return Err(Error::Failed("slurp isn't installed (pacman -S slurp).".into()));
    }
    let colors = SlurpColors::from_settings(&cfg.appearance);
    let mut args: Vec<&str> = vec!["-d", "-w", "2", "-f", "%x,%y %wx%h %l"];
    args.extend(colors.args());
    args.extend(extra);
    let mut child = Command::new("slurp")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("slurp: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(boxes.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("slurp: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    match parse_slurp(&text) {
        Some(sel) if out.status.success() => Ok(sel),
        _ => Err(Error::Cancelled),
    }
}

/// Run grim (PPM to stdout: no PNG compression on the hot path).
fn grab(args: &[&str]) -> Result<RgbaImage, Error> {
    let mut full = vec!["-t", "ppm"];
    full.extend(args);
    let out = Command::new("grim")
        .args(&full)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("grim: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error::Failed(if err.is_empty() { "grim failed.".into() } else { format!("grim: {err}") }));
    }
    let img = image::load_from_memory(&out.stdout).map_err(|e| format!("grim's output: {e}"))?;
    Ok(img.to_rgba8())
}

/// `hyprpicker -r -z`: a still copy of the screen under slurp.
struct FreezeOverlay(Child);

impl FreezeOverlay {
    fn start() -> Option<Self> {
        if !cmd::exists("hyprpicker") {
            return None;
        }
        let child = Command::new("hyprpicker")
            .args(["-r", "-z"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // Let it map before slurp, so slurp's layer lands on top.
        std::thread::sleep(Duration::from_millis(120));
        Some(Self(child))
    }
}

impl Drop for FreezeOverlay {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slurp_answers() {
        assert_eq!(parse_slurp("10,20 30x40 kitty\n"), Some((Rect::new(10, 20, 30, 40), Some("kitty".into()))));
        assert_eq!(parse_slurp("10,20 30x40 \n"), Some((Rect::new(10, 20, 30, 40), None)));
        assert_eq!(parse_slurp("10,20 30x40 Visual Studio Code"), Some((Rect::new(10, 20, 30, 40), Some("Visual Studio Code".into()))));
        assert_eq!(parse_slurp("selection cancelled\n"), None);
        assert_eq!(parse_slurp(""), None);
    }

    #[test]
    fn selection_needed() {
        assert!(needs_selection(Mode::Region, false));
        assert!(needs_selection(Mode::Window, false));
        assert!(!needs_selection(Mode::Window, true));
        assert!(!needs_selection(Mode::All, false));
    }

    #[test]
    fn boxes_for_slurp() {
        let c = hypr::Client {
            class: "foot\nx".into(),
            title: String::new(),
            rect: Rect::new(1, 2, 3, 4),
            workspace: 1,
            focus_history: 0,
        };
        assert_eq!(window_boxes(&[c]), "1,2 3x4 foot x\n");
    }
}
