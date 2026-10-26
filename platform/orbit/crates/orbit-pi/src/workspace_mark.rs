//! Per-workspace sidebar marks: the curated HugeIcons and the semantic tints a
//! project header can wear. Orbit-owned, persisted inside
//! `~/.orbit-pi/workspaces.json` beside the project list, and resolved against
//! the active [`Theme`] at render time so a mark never fights a palette.

use gpui::{Hsla, SharedString};

use crate::theme::Theme;

/// One workspace's stored mark. Both halves are optional; the defaults (the
/// folder glyph, muted ink) apply when a field is unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceMark {
    /// Embedded HugeIcons file stem, e.g. `rocket-01`.
    pub icon: Option<String>,
    /// Stable tint key — see [`TINTS`].
    pub tint: Option<String>,
}

impl WorkspaceMark {
    /// Whether the mark is the unstyled default. Such an entry is dropped
    /// instead of persisted, so an untouched workspace writes no extra keys.
    pub fn is_default(&self) -> bool {
        self.icon.is_none() && self.tint.is_none()
    }
}

/// A curated subset of the embedded HugeIcons set that reads as a project
/// mark. Kept to a compact grid in the picker; the folder-open / sync /
/// archive glyphs double as the automatic status marks below.
pub(crate) const PROJECT_ICONS: &[&str] = &[
    "folder",
    "folder-open",
    "folder-sync",
    "folder-archive",
    "branch",
    "rocket-01",
    "star",
    "spark",
    "cloud",
    "terminal",
    "monitor",
    "server-stack",
    "git-fork",
    "github",
    "image",
    "lock",
    "clock",
    "tag-01",
    "task",
    "compose",
    "magic-wand",
    "start-up",
    "extensions",
    "chat",
    "archive",
    "contrast",
    "side-by-side",
];

/// One row of the picker's icon grid.
pub(crate) const ICON_COLUMNS: usize = 6;

/// A tint resolved against the active theme.
type TintFn = fn(&Theme) -> Hsla;

/// The semantic tints, in picker order. The key is stable on disk; the color
/// follows the active palette. `syn_*` tokens are reused as a dependable
/// spread of hues that every palette tunes to stay legible.
pub(crate) const TINTS: &[(&str, TintFn)] = &[
    ("default", |theme| theme.text_muted),
    ("accent", |theme| theme.accent),
    ("green", |theme| theme.success),
    ("red", |theme| theme.error),
    ("yellow", |theme| theme.warning),
    ("blue", |theme| theme.syn_function),
    ("purple", |theme| theme.syn_type),
    ("pink", |theme| theme.syn_string),
];

/// The ink for a persisted tint key, falling back to the muted default.
pub(crate) fn tint_color(theme: &Theme, key: Option<&str>) -> Hsla {
    key.and_then(|key| {
        TINTS
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, color)| color(theme))
    })
    .unwrap_or(theme.text_muted)
}

/// The asset path for a persisted icon stem, falling back to the folder
/// glyph. Stems are validated against [`PROJECT_ICONS`] so a hand-edited store
/// can never point the renderer at an arbitrary asset.
pub(crate) fn icon_path(icon: Option<&str>) -> SharedString {
    match icon.filter(|stem| PROJECT_ICONS.contains(stem)) {
        Some(stem) => format!("icons/{stem}.svg").into(),
        None => "icons/folder.svg".into(),
    }
}

/// A workspace's live state, resolved to a folder glyph when the user has not
/// picked a custom mark icon. Ordered by priority: the first match wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceStatus {
    /// The active session, or the workspace it is working in.
    Current,
    /// A running session, or a workspace with at least one mid-run.
    Running,
    /// An archived session, or a workspace whose sessions are all archived.
    Archived,
    /// Nothing special — the plain folder.
    Idle,
}

impl WorkspaceStatus {
    fn icon_stem(self) -> &'static str {
        match self {
            Self::Current => "folder-open",
            Self::Running => "folder-sync",
            Self::Archived => "folder-archive",
            Self::Idle => "folder",
        }
    }
}

/// The mark's own icon when the user picked one; otherwise the glyph for the
/// workspace's status. A custom mark always wins over the status default, so
/// status never overrides a deliberate choice.
pub(crate) fn resolved_icon_path(mark: &WorkspaceMark, status: WorkspaceStatus) -> SharedString {
    match mark.icon.as_deref() {
        Some(stem) if PROJECT_ICONS.contains(&stem) => format!("icons/{stem}.svg").into(),
        _ => format!("icons/{}.svg", status.icon_stem()).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_stems_and_keys_fall_back_to_the_defaults() {
        assert_eq!(icon_path(None), SharedString::from("icons/folder.svg"));
        assert_eq!(
            icon_path(Some("../../etc/passwd")),
            SharedString::from("icons/folder.svg")
        );
        assert_eq!(
            icon_path(Some("rocket-01")),
            SharedString::from("icons/rocket-01.svg")
        );

        let theme = crate::theme::Theme::default();
        assert_eq!(tint_color(&theme, Some("nonsense")), theme.text_muted);
        assert_eq!(tint_color(&theme, None), theme.text_muted);
        assert_eq!(tint_color(&theme, Some("accent")), theme.accent);
    }

    #[test]
    fn every_curated_icon_exists_in_the_embedded_set() {
        for stem in PROJECT_ICONS {
            let path = format!("icons/{stem}.svg");
            let bytes =
                <crate::assets::Assets as gpui::AssetSource>::load(&crate::assets::Assets, &path)
                    .expect("asset source reads");
            assert!(bytes.is_some(), "missing icon asset: {path}");
        }
    }

    #[test]
    fn status_icons_apply_only_without_a_custom_mark() {
        let idle = WorkspaceMark::default();
        assert_eq!(
            resolved_icon_path(&idle, WorkspaceStatus::Current),
            SharedString::from("icons/folder-open.svg")
        );
        assert_eq!(
            resolved_icon_path(&idle, WorkspaceStatus::Running),
            SharedString::from("icons/folder-sync.svg")
        );
        assert_eq!(
            resolved_icon_path(&idle, WorkspaceStatus::Archived),
            SharedString::from("icons/folder-archive.svg")
        );
        assert_eq!(
            resolved_icon_path(&idle, WorkspaceStatus::Idle),
            SharedString::from("icons/folder.svg")
        );

        // A user-picked icon wins over the status glyph.
        let custom = WorkspaceMark {
            icon: Some("rocket-01".into()),
            tint: None,
        };
        assert_eq!(
            resolved_icon_path(&custom, WorkspaceStatus::Current),
            SharedString::from("icons/rocket-01.svg")
        );
    }

    #[test]
    fn default_mark_is_recognised() {
        assert!(WorkspaceMark::default().is_default());
        assert!(!WorkspaceMark {
            icon: Some("star".into()),
            tint: None,
        }
        .is_default());
        assert!(!WorkspaceMark {
            icon: None,
            tint: Some("accent".into()),
        }
        .is_default());
    }
}
