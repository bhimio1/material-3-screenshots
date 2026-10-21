//! What happens to a capture: encode, save, copy, shutter sound, and the
//! notification with Edit / Show in folder / Delete.

use crate::cli::ShotArgs;
use crate::config::{Config, ImageFormat};
use crate::notify::{self, Notifier, Outcome};
use material_3_ui::notify::Notification;
use crate::{capture::Captured, history, naming};
use image::{ImageEncoder, RgbaImage};
use material_3_ui::sys::{clipboard, cmd};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn encode(img: &RgbaImage, format: ImageFormat, jpeg_quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let res = match format {
        ImageFormat::Png => image::codecs::png::PngEncoder::new_with_quality(
            &mut out,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8),
        ImageFormat::Jpeg => {
            // JPEG has no alpha.
            let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality.clamp(1, 100)).write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
        }
        // The image crate's WebP encoder is lossless.
        ImageFormat::Webp => image::codecs::webp::WebPEncoder::new_lossless(&mut out).write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        ),
    };
    res.map_err(|e| format!("Couldn't encode the image: {e}"))?;
    Ok(out)
}


/// Write `img` to `path` in the format its extension names (PNG otherwise).
pub fn write_file(img: &RgbaImage, path: &Path, jpeg_quality: u8) -> Result<(), String> {
    let format = ImageFormat::from_path(path).unwrap_or(ImageFormat::Png);
    let bytes = encode(img, format, jpeg_quality)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("m3-part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Save into the screenshots folder under the configured pattern.
pub fn save(img: &RgbaImage, cfg: &Config, mode: &str, label: Option<&str>) -> Result<PathBuf, String> {
    let dir = cfg.screenshots_dir();
    let stem = naming::expand(&cfg.filename_pattern, chrono::Local::now(), mode, label);
    let path = naming::unique_path(&dir, &stem, cfg.format.extension());
    write_file(img, &path, cfg.jpeg_quality)?;
    Ok(path)
}

pub fn copy_image(img: &RgbaImage) -> Result<(), String> {
    clipboard::copy_bytes(&encode(img, ImageFormat::Png, 0)?, "image/png")
}

const SHUTTER_SOUNDS: &[&str] = &[
    "/usr/share/sounds/freedesktop/stereo/camera-shutter.oga",
    "/usr/share/sounds/freedesktop/stereo/screen-capture.oga",
];

pub fn shutter_sound() {
    let Some(sound) = SHUTTER_SOUNDS.iter().find(|p| Path::new(p).is_file()) else {
        return;
    };
    for player in ["pw-play", "paplay"] {
        if cmd::exists(player) {
            let _ = cmd::spawn_detached(player, &[sound]);
            return;
        }
    }
}

/// Open a folder in the Material 3 file manager when it's installed.
pub fn open_folder(dir: &Path) {
    let dir = dir.to_string_lossy();
    if cmd::exists("m3fm") {
        let _ = cmd::spawn_detached("m3fm", &[&dir]);
    } else {
        let _ = cmd::spawn_detached("xdg-open", &[&dir]);
    }
}

/// Open a file with the Material 3 Viewer, else the default app.
pub fn open_file(path: &Path) {
    let path = path.to_string_lossy();
    if cmd::exists("m3-viewer") {
        let _ = cmd::spawn_detached("m3-viewer", &[&path]);
    } else {
        let _ = cmd::spawn_detached("xdg-open", &[&path]);
    }
}

pub fn open_editor(path: &Path) {
    let exe = std::env::current_exe().unwrap_or_else(|_| "m3-screenshots".into());
    let _ = cmd::spawn_detached(&exe.to_string_lossy(), &["edit", &path.to_string_lossy()]);
}

/// The whole after-capture flow of the CLI. Returns the process exit code.
pub fn finish(shot: Captured, args: &ShotArgs, cfg: &Config) -> i32 {
    if cfg.shutter_sound {
        shutter_sound();
    }
    let explicit = args.copy || args.save;
    let (do_copy, do_save) = if explicit {
        (args.copy, args.save)
    } else if args.edit {
        (false, false)
    } else {
        (cfg.copy_to_clipboard, cfg.save_to_disk)
    };
    let mode = shot.mode.as_str();
    let label = shot.label.as_deref();

    let mut errors = Vec::new();
    let mut copied = false;
    if do_copy {
        match copy_image(&shot.image) {
            Ok(()) => copied = true,
            Err(e) => errors.push(e),
        }
    }
    let saved = if do_save {
        match save(&shot.image, cfg, mode, label) {
            Ok(p) => Some(p),
            Err(e) => {
                errors.push(e);
                None
            }
        }
    } else {
        None
    };
    if let Some(p) = &saved {
        println!("{}", p.display());
    }

    if args.edit {
        for e in &errors {
            eprintln!("m3-screenshots: {e}");
        }
        return crate::ui::run_editor(shot.image, saved, cfg.clone());
    }

    if !errors.is_empty() {
        let text = errors.join("\n");
        eprintln!("m3-screenshots: {text}");
        if cfg.notifications && args.notify {
            notify::error("Screenshot failed", &text);
        }
        if !copied && saved.is_none() {
            return 1;
        }
    }
    // Unsaved captures are kept in the cache for a while, so the
    // notification's Edit and the History page still have them.
    let file = match &saved {
        Some(p) => Some(p.clone()),
        None => history::stash(&shot.image).ok(),
    };
    if cfg.notifications && args.notify {
        notify_and_wait(file.as_deref(), saved.is_some(), copied, shot.image.width(), shot.image.height(), cfg);
    }
    0
}

fn notify_and_wait(file: Option<&Path>, saved: bool, copied: bool, w: u32, h: u32, cfg: &Config) {
    let Ok(notifier) = Notifier::connect() else {
        return;
    };
    let summary = match (saved, copied) {
        (true, true) => "Screenshot saved and copied",
        (true, false) => "Screenshot saved",
        (false, true) => "Screenshot copied",
        (false, false) => "Screenshot taken",
    };
    let mut body = format!("{w} × {h}");
    if saved {
        if let Some(name) = file.and_then(|f| f.file_name()) {
            body = format!("{}  ·  {body}", name.to_string_lossy());
        }
    }
    let mut actions = vec![("default".into(), "Open".into()), ("edit".into(), "Edit".into())];
    actions.push(if saved { ("folder".into(), "Show in folder".into()) } else { ("save".into(), "Save".into()) });
    actions.push(("delete".into(), "Delete".into()));
    let n = Notification {
        image: file.map(Path::to_path_buf),
        actions: if file.is_some() { actions } else { Vec::new() },
        ..notify::notification(summary, body, "camera-photo")
    };
    let Ok(id) = notifier.send(&n) else {
        return;
    };
    let Some(file) = file else {
        return;
    };
    // Stay around (in the background; the keybind returned long ago) so the
    // buttons work. Servers that keep notifications in a history panel can
    // still click them a while later.
    match notifier.wait(id, Duration::from_secs(300)) {
        Outcome::Action(key) => match key.as_str() {
            "edit" => open_editor(file),
            "folder" => open_folder(file.parent().unwrap_or(Path::new("/"))),
            "save" => match image::open(file).map_err(|e| e.to_string()).and_then(|img| save(&img.to_rgba8(), cfg, "region", None)) {
                Ok(p) => notify::send(&Notification {
                    image: Some(p.clone()),
                    timeout_ms: 4000,
                    ..notify::notification("Screenshot saved", p.display().to_string(), "camera-photo")
                }),
                Err(e) => notify::error("Couldn't save the screenshot", &e),
            },
            "delete" => {
                let _ = std::fs::remove_file(file);
                notifier.close(id);
            }
            _ => open_file(file),
        },
        Outcome::Closed | Outcome::TimedOut => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_every_format() {
        let img = RgbaImage::from_fn(8, 4, |x, y| image::Rgba([x as u8 * 30, y as u8 * 60, 9, 255]));
        for f in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Webp] {
            let bytes = encode(&img, f, 90).unwrap();
            let back = image::load_from_memory(&bytes).unwrap();
            assert_eq!((back.width(), back.height()), (8, 4), "{f:?}");
        }
        // PNG and lossless WebP round-trip exactly.
        let png = image::load_from_memory(&encode(&img, ImageFormat::Png, 0).unwrap()).unwrap();
        assert_eq!(png.to_rgba8(), img);
        let webp = image::load_from_memory(&encode(&img, ImageFormat::Webp, 0).unwrap()).unwrap();
        assert_eq!(webp.to_rgba8(), img);
    }

    #[test]
    fn saves_under_the_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            save_dir: dir.path().join("Shots").to_string_lossy().into_owned(),
            filename_pattern: "{mode}-{window}".into(),
            format: ImageFormat::Jpeg,
            ..Config::default()
        };
        let img = RgbaImage::new(3, 3);
        let a = save(&img, &cfg, "window", Some("kitty")).unwrap();
        let b = save(&img, &cfg, "window", Some("kitty")).unwrap();
        assert_eq!(a.file_name().unwrap(), "window-kitty.jpg");
        assert_eq!(b.file_name().unwrap(), "window-kitty-1.jpg");
        assert!(a.is_file() && b.is_file());
        assert!(!dir.path().join("Shots/window-kitty.m3-part").exists());
    }
}
