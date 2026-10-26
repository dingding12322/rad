//! Project logo detection for the new-task page — a conventional logo image
//! found in the workspace is shown in the folder field in place of the folder
//! glyph, and anything missing falls back to the folder icon.
//!
//! Deliberately shallow and deterministic: a fixed list of conventional
//! names, first match wins, no directory walk. `.orbit-pi/icon.*` is the
//! explicit override — drop a file there to pin a logo without touching the
//! project. The file is read once per workspace change; decoding stays with
//! gpui (the texture uploads on first paint).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::Image;

use crate::explorer::viewer::image_format;

/// Conventional logo stems, most specific first. Each is tried with every
/// [`EXTENSIONS`] entry before moving on.
const STEMS: &[&str] = &[
    ".orbit-pi/icon",
    ".orbit-pi/logo",
    "logo",
    "icon",
    "app-icon",
    "favicon",
    "assets/logo",
    "assets/icon",
    "assets/icons/logo",
    "assets/images/logo",
    "public/logo",
    "public/icon",
    "public/favicon",
    "static/logo",
    "static/icon",
    "src/assets/logo",
    "resources/icon",
    "src-tauri/icons/icon",
];

/// Decodable extensions, preferred first (`image_format` drops the rest).
const EXTENSIONS: &[&str] = &["svg", "png", "jpg", "jpeg", "webp"];

/// Upper bound on the logo file, so a stray huge image is skipped instead of
/// being read into memory.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The workspace's logo: the first conventional file that exists, is
/// non-empty, and fits [`MAX_BYTES`]. `None` means the caller keeps the
/// folder glyph.
pub fn load(workspace: &Path) -> Option<Arc<Image>> {
    let path = find(workspace)?;
    if std::fs::metadata(&path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let format = image_format(&path)?;
    let bytes = std::fs::read(&path).ok()?;
    (!bytes.is_empty()).then(|| Arc::new(Image::from_bytes(format, bytes)))
}

/// The conventional logo path in `workspace`, if one exists. Split from
/// [`load`] so the lookup order is unit-testable without decoding.
fn find(workspace: &Path) -> Option<PathBuf> {
    STEMS.iter().find_map(|stem| {
        EXTENSIONS.iter().find_map(|ext| {
            let path = workspace.join(format!("{stem}.{ext}"));
            path.is_file().then_some(path)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique scratch directory per call — tests share a process id.
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("orbit-logo-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, rel: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn missing_logo_is_none() {
        let dir = temp_dir("missing");
        assert_eq!(find(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn earliest_stem_wins_over_later_directories() {
        let dir = temp_dir("precedence");
        touch(&dir, "assets/icons/logo.png");
        touch(&dir, "logo.svg");
        assert_eq!(find(&dir), Some(dir.join("logo.svg")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_orbit_override_beats_root_logo() {
        let dir = temp_dir("override");
        touch(&dir, "logo.png");
        touch(&dir, ".orbit-pi/icon.svg");
        assert_eq!(find(&dir), Some(dir.join(".orbit-pi/icon.svg")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extension_preference_is_svg_then_png() {
        let dir = temp_dir("extension");
        touch(&dir, "logo.png");
        touch(&dir, "logo.svg");
        assert_eq!(find(&dir), Some(dir.join("logo.svg")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn directory_named_like_a_logo_is_not_a_hit() {
        let dir = temp_dir("dir");
        std::fs::create_dir_all(dir.join("logo.png")).unwrap();
        assert_eq!(find(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
