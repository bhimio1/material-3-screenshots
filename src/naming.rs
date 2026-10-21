//! File names from the user's pattern: strftime codes (`%Y-%m-%d`, …) plus
//! `{mode}` (region, window, output, all, recording, edited) and `{window}`
//! (the window's class, or the output's name). A clash gets `-1`, `-2`, ….

use chrono::{DateTime, Local};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const TOKENS_HELP: &str = "strftime codes like %Y-%m-%d %H-%M-%S, plus {mode} and {window}";

/// Expand a pattern into a file stem (no folder, no extension).
pub fn expand(pattern: &str, time: DateTime<Local>, mode: &str, window: Option<&str>) -> String {
    let pattern = if pattern.trim().is_empty() {
        "Screenshot_%Y-%m-%d_%H-%M-%S"
    } else {
        pattern.trim()
    };
    let with_tokens = pattern
        .replace("{mode}", mode)
        .replace("{window}", window.filter(|w| !w.is_empty()).unwrap_or(mode));
    // chrono panics (in Display) on an invalid specifier; check first.
    let items: Vec<_> = chrono::format::StrftimeItems::new(&with_tokens).collect();
    let mut stem = String::new();
    if items.iter().any(|i| matches!(i, chrono::format::Item::Error)) {
        stem.push_str(&with_tokens);
    } else {
        let _ = write!(stem, "{}", time.format_with_items(items.into_iter()));
    }
    sanitize(&stem)
}

/// Keep a stem usable as one file name.
pub fn sanitize(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() {
        "Screenshot".into()
    } else {
        trimmed.chars().take(200).collect()
    }
}

/// `dir/stem.ext`, or `dir/stem-N.ext` when that's taken.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("some suffix is free")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 21, 9, 5, 7).unwrap()
    }

    #[test]
    fn default_pattern() {
        assert_eq!(expand("Screenshot_%Y-%m-%d_%H-%M-%S", t(), "region", None), "Screenshot_2026-10-21_09-05-07");
        assert_eq!(expand("  ", t(), "region", None), "Screenshot_2026-10-21_09-05-07");
    }

    #[test]
    fn tokens() {
        assert_eq!(expand("{mode}-{window}-%H%M", t(), "window", Some("kitty")), "window-kitty-0905");
        assert_eq!(expand("{window}", t(), "region", None), "region");
        // Slashes from a window class can't escape the folder.
        assert_eq!(expand("{window}", t(), "window", Some("org/gnome:App")), "org_gnome_App");
    }

    #[test]
    fn bad_specifiers_are_kept_literally() {
        assert_eq!(expand("shot %Q", t(), "all", None), "shot %Q");
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize("..."), "Screenshot");
        assert_eq!(sanitize("a\nb"), "a_b");
        assert_eq!(sanitize(&"x".repeat(500)).len(), 200);
    }

    #[test]
    fn unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = unique_path(dir.path(), "shot", "png");
        assert_eq!(a.file_name().unwrap(), "shot.png");
        std::fs::write(&a, b"").unwrap();
        let b = unique_path(dir.path(), "shot", "png");
        assert_eq!(b.file_name().unwrap(), "shot-1.png");
        std::fs::write(&b, b"").unwrap();
        assert_eq!(unique_path(dir.path(), "shot", "png").file_name().unwrap(), "shot-2.png");
    }
}
