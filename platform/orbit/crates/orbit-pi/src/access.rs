//! Access mode — how much latitude the agent has, and where it is persisted.
//!
//! Orbit ships a bundled pi extension (`contrib/orbit-guard-extension/`) that
//! intercepts every `tool_call` and confirms the ones the active mode does not
//! auto-approve. The mode is the single source of truth on both sides:
//!
//! - the app reads/writes it here, persisting to `~/.orbit-pi/access.json`
//! - the extension reads the same file fresh on every tool call, so changing
//!   the mode re-arms live sessions without a restart
//!
//! Modes mirror the workbench convention (Supervised / Auto-accept edits /
//! Full access). `FullAccess` is the default so adding the guard does not
//! silently change how the agent behaves; the composer chip makes tightening
//! it one click.

use std::path::PathBuf;

use serde_json::json;

/// The active access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessMode {
    /// Ask before file changes and commands; reads pass through.
    Supervised,
    /// Auto-approve edits; ask before commands and other actions.
    AutoAcceptEdits,
    /// Allow commands and edits without prompts.
    #[default]
    FullAccess,
}

impl AccessMode {
    /// Every mode, in picker order.
    pub const ALL: [AccessMode; 3] = [Self::Supervised, Self::AutoAcceptEdits, Self::FullAccess];

    /// The wire/persistence id. Kept in sync with `MODES` in the extension's
    /// `policy.js`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Supervised => "supervised",
            Self::AutoAcceptEdits => "auto-accept-edits",
            Self::FullAccess => "full-access",
        }
    }

    /// Parse a stored id, falling back to [`AccessMode::default`] on anything
    /// unknown so a hand-edited file can never widen access by accident.
    pub fn from_wire(value: &str) -> Self {
        match value {
            "supervised" => Self::Supervised,
            "auto-accept-edits" => Self::AutoAcceptEdits,
            "full-access" => Self::FullAccess,
            _ => Self::default(),
        }
    }

    /// Short label for the composer chip and picker rows.
    pub fn label(self) -> String {
        match self {
            Self::Supervised => tr!("access.supervised"),
            Self::AutoAcceptEdits => tr!("access.auto_accept_edits"),
            Self::FullAccess => tr!("access.full_access"),
        }
    }

    /// One-line explanation shown under the label in the picker.
    pub fn description(self) -> String {
        match self {
            Self::Supervised => tr!("access.supervised_hint"),
            Self::AutoAcceptEdits => tr!("access.auto_accept_edits_hint"),
            Self::FullAccess => tr!("access.full_access_hint"),
        }
    }

    /// The bundled icon path for the chip and picker row.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Supervised | Self::FullAccess => "icons/lock.svg",
            Self::AutoAcceptEdits => "icons/compose.svg",
        }
    }

    /// Load the persisted mode, falling back to the default when the file is
    /// missing, unreadable, or malformed.
    pub fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(store_path()) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Self::default();
        };
        value
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .map(Self::from_wire)
            .unwrap_or_default()
    }

    /// Persist the mode to `~/.orbit-pi/access.json`. This is also the file
    /// the guard extension reads, so a write re-arms live sessions.
    pub fn persist(self) {
        let path = store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json!({ "mode": self.as_wire() }).to_string());
    }
}

/// `~/.orbit-pi/access.json` — the app and the guard extension share it.
fn store_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("access.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trips() {
        for mode in AccessMode::ALL {
            assert_eq!(AccessMode::from_wire(mode.as_wire()), mode);
        }
    }

    #[test]
    fn unknown_wire_falls_back_to_default() {
        assert_eq!(
            AccessMode::from_wire("bypass-everything"),
            AccessMode::default()
        );
        assert_eq!(AccessMode::from_wire(""), AccessMode::default());
    }

    #[test]
    fn every_mode_has_distinct_copy() {
        let labels: std::collections::HashSet<_> =
            AccessMode::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(labels.len(), AccessMode::ALL.len());
        for mode in AccessMode::ALL {
            assert!(!mode.description().is_empty());
            assert!(mode.icon().starts_with("icons/"));
        }
    }
}
