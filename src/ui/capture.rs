//! The Capture page: start a screenshot or a recording, see the recent ones.

use super::history::{HistoryEvent, HistoryView, Thumbs};
use crate::cli::Mode;
use crate::config::Config;
use crate::history::Kind;
use crate::record;
use crate::ui_ext::{action_tile, thumb_tile};
use egui::Ui;
use material_3_ui::common::{self, card, row};
use material_3_ui::icons::icons::*;
use material_3_ui::sys::cmd;
use material_3_ui::widgets::{self, ButtonKind};

/// The capture page's one-off choices (the defaults come from settings).
pub struct CaptureOptions {
    pub delay: u32,
    pub freeze: bool,
    pub edit: bool,
    pub audio: bool,
}

impl CaptureOptions {
    pub fn from_config(cfg: &Config) -> Self {
        Self { delay: 0, freeze: cfg.freeze, edit: cfg.edit_after_capture, audio: cfg.recorder.audio }
    }
}

pub enum CaptureEvent {
    /// Run the CLI with these arguments, then close the window.
    Spawn(Vec<String>),
    StopRecording,
    /// Install these packages in a terminal.
    Install(Vec<String>),
    OpenHistory,
    History(HistoryEvent),
}

/// The CLI arguments for a screenshot started from the page.
pub fn shot_args(mode: Mode, o: &CaptureOptions) -> Vec<String> {
    let mut a = vec![mode.as_str().to_string()];
    if o.delay > 0 {
        a.extend(["--delay".into(), o.delay.to_string()]);
    }
    if crate::capture::needs_selection(mode, false) {
        a.push(if o.freeze { "--freeze" } else { "--no-freeze" }.into());
    }
    if o.edit {
        a.push("--edit".into());
    }
    a
}

pub fn record_args(mode: Mode, o: &CaptureOptions) -> Vec<String> {
    vec!["record".into(), mode.as_str().into(), if o.audio { "--audio" } else { "--no-audio" }.into()]
}

pub fn show(
    ui: &mut Ui,
    cfg: &Config,
    opts: &mut CaptureOptions,
    recording: Option<&record::State>,
    history: &mut HistoryView,
    thumbs: &mut Thumbs,
) -> Vec<CaptureEvent> {
    let p = widgets::palette(ui);
    let mut events = Vec::new();
    history.refresh(ui.ctx(), cfg, false);
    common::page(ui, ICON_SCREENSHOT_REGION, "Capture", "Take a screenshot or record your screen.", |ui| {
        ui.add_space(8.0);
        let missing: Vec<&str> = [("grim", "grim"), ("slurp", "slurp"), ("wl-copy", "wl-clipboard")]
            .into_iter()
            .filter(|(program, _)| !cmd::exists(program))
            .map(|(_, package)| package)
            .collect();
        if !missing.is_empty() {
            let text = format!("Capturing needs {}, which isn't installed.", missing.join(", "));
            common::banner(ui, ICON_WARNING, &text, false, |ui| {
                if widgets::button(ui, ButtonKind::Filled, Some(ICON_DOWNLOAD), "Install").clicked() {
                    events.push(CaptureEvent::Install(missing.iter().map(|s| s.to_string()).collect()));
                }
            });
        }
        if let Some(rec) = recording {
            let secs = (chrono::Local::now().timestamp() - rec.started).max(0);
            let text = format!(
                "Recording {}:{:02} · {}",
                secs / 60,
                secs % 60,
                rec.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            );
            common::banner(ui, ICON_FIBER_MANUAL_RECORD, &text, false, |ui| {
                if widgets::button(ui, ButtonKind::Danger, Some(ICON_STOP), "Stop").clicked() {
                    events.push(CaptureEvent::StopRecording);
                }
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }

        card(ui, "Screenshot", |ui| {
            let gap = 10.0;
            let w = ((ui.available_width() - gap * 3.0) / 4.0).floor();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (mode, icon, title, sub, accent) in [
                    (Mode::Region, ICON_SCREENSHOT_REGION, "Region", "Drag, or click a window", true),
                    (Mode::Window, ICON_SELECT_WINDOW, "Window", "Click a window", false),
                    (Mode::Output, ICON_SCREENSHOT_MONITOR, "Screen", "Click a screen", false),
                    (Mode::All, ICON_DESKTOP_WINDOWS, "All screens", "Everything at once", false),
                ] {
                    if action_tile(ui, icon, title, sub, w, accent).clicked() {
                        events.push(CaptureEvent::Spawn(shot_args(mode, opts)));
                    }
                }
            });
            ui.add_space(8.0);
            row(ui, "Delay", "Wait before the picture is taken.", |ui| {
                widgets::segmented(ui, &mut opts.delay, &[(0, ICON_BOLT, "Off"), (3, ICON_TIMER, "3 s"), (5, ICON_TIMER, "5 s"), (10, ICON_TIMER, "10 s")]);
            });
            ui.separator();
            let freeze_desc = if cmd::exists("hyprpicker") {
                "Keep the screen still while you select."
            } else {
                "Keep the screen still while you select (install hyprpicker to see it frozen)."
            };
            row(ui, "Freeze the screen", freeze_desc, |ui| {
                widgets::switch(ui, &mut opts.freeze);
            });
            ui.separator();
            row(ui, "Open the editor", "Annotate the capture before saving it.", |ui| {
                widgets::switch(ui, &mut opts.edit);
            });
        });

        card(ui, "Record", |ui| {
            let gap = 10.0;
            let w = ((ui.available_width() - gap * 2.0) / 3.0).floor();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (mode, icon, title, sub) in [
                    (Mode::Region, ICON_SCREEN_RECORD, "Region", "Drag an area"),
                    (Mode::Window, ICON_SELECT_WINDOW, "Window", "Click a window"),
                    (Mode::Output, ICON_VIDEOCAM, "Screen", "Click a screen"),
                ] {
                    if action_tile(ui, icon, title, sub, w, false).clicked() && recording.is_none() {
                        events.push(CaptureEvent::Spawn(record_args(mode, opts)));
                    }
                }
            });
            ui.add_space(8.0);
            row(ui, "Record audio", "What's playing on your default output.", |ui| {
                widgets::switch(ui, &mut opts.audio);
            });
            let backend = record::pick_backend(cfg.recorder.backend, cmd::exists);
            match backend {
                Ok(b) => common::note(ui, &format!("Recording with {} at {} fps. Stop with the same key, or `m3-screenshots record stop`.", b.program(), cfg.recorder.fps)),
                Err(e) => common::banner(ui, ICON_WARNING, &e, false, |_| {}),
            }
        });

        let recent: Vec<_> = history.entries.iter().take(4).cloned().collect();
        if !recent.is_empty() {
            card(ui, "Recent", |ui| {
                let gap = 10.0;
                let w = ((ui.available_width() - gap * 3.0) / 4.0).floor();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for e in &recent {
                        let tex = thumbs.get(ui.ctx(), e);
                        let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let when = chrono::DateTime::<chrono::Local>::from(e.modified).format("%H:%M").to_string();
                        let (icon, badge) = match e.kind {
                            Kind::Screenshot => (ICON_IMAGE, None),
                            Kind::Recording => (ICON_MOVIE, Some(ICON_PLAY_ARROW)),
                        };
                        if thumb_tile(ui, tex.as_ref(), icon, &name, &when, w, badge).clicked() {
                            match e.kind {
                                Kind::Screenshot => events.push(CaptureEvent::History(HistoryEvent::Edit(e.path.clone()))),
                                Kind::Recording => crate::deliver::open_file(&e.path),
                            }
                        }
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, ButtonKind::Text, Some(ICON_HISTORY), "See all").clicked() {
                        events.push(CaptureEvent::OpenHistory);
                    }
                });
            });
        }

        card(ui, "Keyboard shortcuts", |ui| {
            common::note(ui, "end4-pC binds these for you. Elsewhere, add them to your Hyprland config:");
            ui.add_space(4.0);
            let code = "hl.bind(\"Print\", hl.dsp.exec_cmd(\"m3-screenshots output --active\"))\n\
                        hl.bind(\"SUPER + SHIFT + S\", hl.dsp.exec_cmd(\"m3-screenshots region\"))\n\
                        hl.bind(\"SUPER + SHIFT + E\", hl.dsp.exec_cmd(\"m3-screenshots region --edit\"))\n\
                        hl.bind(\"SUPER + SHIFT + R\", hl.dsp.exec_cmd(\"m3-screenshots record region --toggle\"))";
            egui::Frame::new()
                .fill(p.surface_container_highest)
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(code).monospace().size(12.5).color(p.on_surface));
                });
        });
    });
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_builds_cli_arguments() {
        let o = CaptureOptions { delay: 3, freeze: false, edit: true, audio: true };
        assert_eq!(shot_args(Mode::Region, &o), ["region", "--delay", "3", "--no-freeze", "--edit"]);
        // No selection, so no freeze flag.
        assert_eq!(shot_args(Mode::All, &CaptureOptions { delay: 0, freeze: true, edit: false, audio: false }), ["all"]);
        assert_eq!(record_args(Mode::Output, &o), ["record", "output", "--audio"]);
        // Everything the page builds parses.
        for args in [shot_args(Mode::Window, &o), record_args(Mode::Region, &o)] {
            assert!(crate::cli::parse(args).is_ok());
        }
    }
}
