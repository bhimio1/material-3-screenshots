//! The History page's data: what's in the screenshots and recordings
//! folders, plus recent captures that were only copied (kept in
//! `~/.cache/m3-screenshots/unsaved`, newest 30).

use crate::config::Config;
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const KEEP_UNSAVED: usize = 30;
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp"];
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "webm", "mov"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Screenshot,
    Recording,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
    pub modified: SystemTime,
    pub size: u64,
    /// Only on the clipboard (and in the cache), not in a folder.
    pub unsaved: bool,
}

pub fn unsaved_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| material_3_ui::sys::paths::home().join(".cache"))
        .join("m3-screenshots/unsaved")
}

/// Keep a copy of a capture that wasn't saved.
pub fn stash(img: &RgbaImage) -> Result<PathBuf, String> {
    stash_in(&unsaved_dir(), img)
}

fn stash_in(dir: &Path, img: &RgbaImage) -> Result<PathBuf, String> {
    let stem = chrono::Local::now().format("Capture_%Y-%m-%d_%H-%M-%S").to_string();
    let path = crate::naming::unique_path(dir, &stem, "png");
    crate::deliver::write_file(img, &path, 90)?;
    let mut old = scan(dir, Kind::Screenshot, true);
    if old.len() > KEEP_UNSAVED {
        sort(&mut old);
        for e in &old[KEEP_UNSAVED..] {
            let _ = std::fs::remove_file(&e.path);
        }
    }
    Ok(path)
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some(Kind::Screenshot)
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some(Kind::Recording)
    } else {
        None
    }
}

fn scan(dir: &Path, want: Kind, unsaved: bool) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter_map(|e| {
            let path = e.path();
            let kind = kind_of(&path).filter(|k| *k == want)?;
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some(Entry {
                path,
                kind,
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size: meta.len(),
                unsaved,
            })
        })
        .collect()
}

fn sort(list: &mut [Entry]) {
    list.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
}

/// Everything, newest first.
pub fn load(cfg: &Config) -> Vec<Entry> {
    load_from(&cfg.screenshots_dir(), &cfg.recordings_dir(), &unsaved_dir())
}

pub fn load_from(shots: &Path, recordings: &Path, unsaved: &Path) -> Vec<Entry> {
    let mut all = scan(shots, Kind::Screenshot, false);
    if recordings != shots {
        all.extend(scan(recordings, Kind::Recording, false));
    } else {
        all.extend(scan(shots, Kind::Recording, false));
    }
    all.extend(scan(unsaved, Kind::Screenshot, true));
    sort(&mut all);
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, age_secs: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn merges_and_sorts() {
        let d = tempfile::tempdir().unwrap();
        let (s, r, u) = (d.path().join("s"), d.path().join("r"), d.path().join("u"));
        touch(&s.join("a.png"), 30);
        touch(&s.join("notes.txt"), 1);
        touch(&r.join("b.mp4"), 10);
        touch(&u.join("c.png"), 20);
        let list = load_from(&s, &r, &u);
        let names: Vec<_> = list.iter().map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["b.mp4", "c.png", "a.png"]);
        assert_eq!(list[0].kind, Kind::Recording);
        assert!(list[1].unsaved && !list[2].unsaved);
    }

    #[test]
    fn same_folder_for_both() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("a.png"), 2);
        touch(&d.path().join("b.webm"), 1);
        assert_eq!(load_from(d.path(), d.path(), &d.path().join("none")).len(), 2);
    }

    #[test]
    fn stash_keeps_the_newest() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..KEEP_UNSAVED + 3 {
            touch(&d.path().join(format!("old{i}.png")), 1000 + i as u64);
        }
        let p = stash_in(d.path(), &RgbaImage::new(2, 2)).unwrap();
        assert!(p.is_file());
        assert_eq!(scan(d.path(), Kind::Screenshot, true).len(), KEEP_UNSAVED);
        assert!(!d.path().join(format!("old{}.png", KEEP_UNSAVED + 2)).exists());
    }
}
