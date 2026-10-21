//! `~/.config/m3-screenshots/config.toml`.

use material_3_ui::sys::paths;
use material_3_ui::theme::ThemeSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }

    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(ImageFormat::Png),
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "webp" => Some(ImageFormat::Webp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RecorderBackend {
    /// gpu-screen-recorder when installed, else wf-recorder.
    #[default]
    Auto,
    GpuScreenRecorder,
    WfRecorder,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RecordQuality {
    Medium,
    #[default]
    High,
    VeryHigh,
    Ultra,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct RecorderSettings {
    pub backend: RecorderBackend,
    pub fps: u32,
    pub quality: RecordQuality,
    /// Record what's playing (the default sink's monitor).
    pub audio: bool,
    pub save_dir: String,
    pub filename_pattern: String,
    pub show_cursor: bool,
}

impl Default for RecorderSettings {
    fn default() -> Self {
        Self {
            backend: RecorderBackend::Auto,
            fps: 60,
            quality: RecordQuality::High,
            audio: false,
            save_dir: default_dir(dirs::video_dir(), "Videos", "Screencasts"),
            filename_pattern: "Recording_%Y-%m-%d_%H-%M-%S".into(),
            show_cursor: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub save_dir: String,
    /// strftime codes plus `{mode}` and `{window}`; see `naming.rs`.
    pub filename_pattern: String,
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    /// What a capture does when neither --copy nor --save is given.
    pub copy_to_clipboard: bool,
    pub save_to_disk: bool,
    pub freeze: bool,
    pub shutter_sound: bool,
    pub notifications: bool,
    /// The app's Capture page opens the editor after capturing.
    pub edit_after_capture: bool,
    pub terminal: String,
    pub recorder: RecorderSettings,
    pub appearance: ThemeSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            save_dir: default_dir(dirs::picture_dir(), "Pictures", "Screenshots"),
            filename_pattern: "Screenshot_%Y-%m-%d_%H-%M-%S".into(),
            format: ImageFormat::Png,
            jpeg_quality: 90,
            copy_to_clipboard: true,
            save_to_disk: true,
            freeze: true,
            shutter_sound: true,
            notifications: true,
            edit_after_capture: false,
            terminal: String::new(),
            recorder: RecorderSettings::default(),
            appearance: ThemeSettings::default(),
        }
    }
}

fn default_dir(xdg: Option<PathBuf>, fallback: &str, sub: &str) -> String {
    let base = xdg.unwrap_or_else(|| paths::home().join(fallback));
    paths::tilde(&base.join(sub))
}

pub fn path() -> PathBuf {
    paths::config_home().join("m3-screenshots/config.toml")
}

impl Config {
    /// Read the config; a missing or broken file gives the defaults (the
    /// CLI must never fail because of it).
    pub fn load() -> Self {
        Self::load_from(&path()).unwrap_or_default()
    }

    pub fn load_from(path: &std::path::Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        match toml::from_str(&text) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("m3-screenshots: ignoring {}: {e}", path.display());
                None
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn screenshots_dir(&self) -> PathBuf {
        paths::expand(&self.save_dir)
    }

    pub fn recordings_dir(&self) -> PathBuf {
        paths::expand(&self.recorder.save_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fill_in_defaults() {
        let c: Config = toml::from_str(
            "format = \"webp\"\n[recorder]\nfps = 30\nbackend = \"wf-recorder\"\n[appearance]\nscheme = \"dark\"\n",
        )
        .unwrap();
        assert_eq!(c.format, ImageFormat::Webp);
        assert_eq!(c.recorder.fps, 30);
        assert_eq!(c.recorder.backend, RecorderBackend::WfRecorder);
        assert_eq!(c.recorder.quality, RecordQuality::High);
        assert!(c.copy_to_clipboard && c.save_to_disk);
        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn default_folders() {
        let c = Config::default();
        assert!(c.save_dir.ends_with("/Screenshots"), "{}", c.save_dir);
        assert!(c.recorder.save_dir.ends_with("/Screencasts"));
        assert!(c.screenshots_dir().is_absolute());
    }

    #[test]
    fn formats() {
        assert_eq!(ImageFormat::from_path("a/b.JPEG".as_ref()), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_path("a/b.gif".as_ref()), None);
        assert_eq!(ImageFormat::Webp.extension(), "webp");
    }
}
