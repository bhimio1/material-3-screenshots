//! The Settings page.

use crate::config::{Config, ImageFormat, RecordQuality, RecorderBackend};
use crate::naming;
use egui::Ui;
use material_3_ui::common::{self, card, row};
use material_3_ui::icons::icons::*;
use material_3_ui::prefs;
use material_3_ui::sys::{arch, cmd, term};
use material_3_ui::theme::ThemeManager;
use material_3_ui::widgets::{self, ButtonKind};
use std::collections::HashSet;

pub enum SettingsEvent {
    ThemeChanged,
    Toast(String, bool),
}

/// (program, package, what for, required)
const TOOLS: &[(&str, &str, &str, bool)] = &[
    ("grim", "grim", "Takes the screenshots", true),
    ("slurp", "slurp", "Region, window and screen selection", true),
    ("wl-copy", "wl-clipboard", "Copying to the clipboard", true),
    ("hyprpicker", "hyprpicker", "Shows the frozen screen while you select", false),
    ("gpu-screen-recorder", "gpu-screen-recorder", "Hardware-accelerated recording (preferred)", false),
    ("wf-recorder", "wf-recorder", "Recording when gpu-screen-recorder isn't installed", false),
    ("ffmpegthumbnailer", "ffmpegthumbnailer", "Thumbnails for recordings in History", false),
];

fn folder_row(ui: &mut Ui, title: &str, value: &mut String) {
    row(ui, title, "", |ui| {
        if widgets::icon_button(ui, ICON_FOLDER_OPEN, "Open", true).clicked() {
            let dir = material_3_ui::sys::paths::expand(value);
            let _ = std::fs::create_dir_all(&dir);
            crate::deliver::open_folder(&dir);
        }
        ui.add(egui::TextEdit::singleline(value).desired_width(280.0));
    });
}

pub fn show(ui: &mut Ui, cfg: &mut Config, theme: &ThemeManager, installed: Option<&HashSet<String>>) -> Vec<SettingsEvent> {
    let mut events = Vec::new();
    common::page(ui, ICON_SETTINGS, "Settings", "Where captures go and what happens after one.", |ui| {
        card(ui, "Saving", |ui| {
            folder_row(ui, "Screenshots folder", &mut cfg.save_dir);
            ui.separator();
            let preview = naming::expand(&cfg.filename_pattern, chrono::Local::now(), "region", Some("kitty"));
            row(ui, "File name", &format!("{}. Example: {preview}.{}", naming::TOKENS_HELP, cfg.format.extension()), |ui| {
                ui.add(egui::TextEdit::singleline(&mut cfg.filename_pattern).desired_width(280.0));
            });
            ui.separator();
            row(ui, "Format", "PNG is lossless; JPEG is smallest; WebP here is lossless and smaller than PNG.", |ui| {
                widgets::segmented(
                    ui,
                    &mut cfg.format,
                    &[(ImageFormat::Png, ICON_IMAGE, "PNG"), (ImageFormat::Jpeg, ICON_PHOTO_CAMERA, "JPEG"), (ImageFormat::Webp, ICON_WEB_ASSET, "WebP")],
                );
            });
            if cfg.format == ImageFormat::Jpeg {
                ui.separator();
                row(ui, "JPEG quality", "", |ui| {
                    ui.add(egui::Slider::new(&mut cfg.jpeg_quality, 50..=100));
                });
            }
        });

        card(ui, "After a capture", |ui| {
            row(ui, "Copy to the clipboard", "When a key binding doesn't say (--copy / --save).", |ui| {
                widgets::switch(ui, &mut cfg.copy_to_clipboard);
            });
            ui.separator();
            row(ui, "Save to the folder", "", |ui| {
                widgets::switch(ui, &mut cfg.save_to_disk);
            });
            ui.separator();
            row(ui, "Freeze the screen while selecting", "What's captured is the moment you pressed the key.", |ui| {
                widgets::switch(ui, &mut cfg.freeze);
            });
            ui.separator();
            row(ui, "Open the editor from the Capture page", "", |ui| {
                widgets::switch(ui, &mut cfg.edit_after_capture);
            });
            ui.separator();
            row(ui, "Notification", "With Edit, Show in folder and Delete buttons.", |ui| {
                widgets::switch(ui, &mut cfg.notifications);
            });
            ui.separator();
            let sound = if std::path::Path::new("/usr/share/sounds/freedesktop/stereo/camera-shutter.oga").is_file() {
                "Plays the freedesktop camera shutter (pw-play or paplay)."
            } else {
                "Needs sound-theme-freedesktop."
            };
            row(ui, "Shutter sound", sound, |ui| {
                widgets::switch(ui, &mut cfg.shutter_sound);
            });
        });

        card(ui, "Recording", |ui| {
            row(ui, "Recorder", "Auto prefers gpu-screen-recorder (hardware encoding).", |ui| {
                widgets::segmented(
                    ui,
                    &mut cfg.recorder.backend,
                    &[
                        (RecorderBackend::Auto, ICON_AUTO_AWESOME, "Auto"),
                        (RecorderBackend::GpuScreenRecorder, ICON_MEMORY, "GPU"),
                        (RecorderBackend::WfRecorder, ICON_VIDEOCAM, "wf-recorder"),
                    ],
                );
            });
            ui.separator();
            row(ui, "Frame rate", "", |ui| {
                widgets::segmented(ui, &mut cfg.recorder.fps, &[(30, ICON_SPEED, "30 fps"), (60, ICON_SPEED, "60 fps")]);
            });
            ui.separator();
            row(ui, "Quality", "Higher is sharper and bigger.", |ui| {
                widgets::segmented(
                    ui,
                    &mut cfg.recorder.quality,
                    &[
                        (RecordQuality::Medium, ICON_HD, "Medium"),
                        (RecordQuality::High, ICON_HD, "High"),
                        (RecordQuality::VeryHigh, ICON_HIGH_QUALITY, "Very high"),
                        (RecordQuality::Ultra, ICON_HIGH_QUALITY, "Ultra"),
                    ],
                );
            });
            ui.separator();
            row(ui, "Record audio by default", "The default output's monitor (PipeWire).", |ui| {
                widgets::switch(ui, &mut cfg.recorder.audio);
            });
            ui.separator();
            row(ui, "Show the cursor", "gpu-screen-recorder only.", |ui| {
                widgets::switch(ui, &mut cfg.recorder.show_cursor);
            });
            ui.separator();
            folder_row(ui, "Recordings folder", &mut cfg.recorder.save_dir);
            ui.separator();
            row(ui, "File name", naming::TOKENS_HELP, |ui| {
                ui.add(egui::TextEdit::singleline(&mut cfg.recorder.filename_pattern).desired_width(280.0));
            });
        });

        if prefs::appearance_card(ui, &mut cfg.appearance, theme) {
            events.push(SettingsEvent::ThemeChanged);
        }

        card(ui, "Tools", |ui| {
            for (program, package, purpose, required) in TOOLS {
                let present = cmd::exists(program);
                let sub = if present {
                    format!("{purpose} · installed")
                } else if *required {
                    format!("{purpose} · required, not installed")
                } else {
                    format!("{purpose} · optional")
                };
                common::list_row(ui, if present { ICON_CHECK } else { ICON_DOWNLOAD }, present, package, &sub, |ui| {
                    let have_pkg = installed.is_some_and(|s| s.contains(*package));
                    if !present && !have_pkg && widgets::button(ui, ButtonKind::Tonal, None, "Install").clicked() {
                        let script = arch::install_command(&[package], arch::AurHelper::Auto);
                        match term::run(&cfg.terminal, &script) {
                            Ok(t) => events.push(SettingsEvent::Toast(format!("Installing {package} in {t}"), false)),
                            Err(e) => events.push(SettingsEvent::Toast(e, true)),
                        }
                    }
                });
            }
            ui.separator();
            row(ui, "Terminal", "Where installs run. Empty picks one (kitty, foot, …).", |ui| {
                ui.add(egui::TextEdit::singleline(&mut cfg.terminal).hint_text("auto").desired_width(200.0));
            });
            ui.add_space(4.0);
            common::note(ui, &format!("Config: {}", material_3_ui::sys::paths::tilde(&crate::config::path())));
        });
    });
    events
}
