//! Session retention — Orbit's configurable policy for old sessions.
//!
//! When the user enables it, Orbit runs the policy once at launch: every
//! session with no *message activity* in the last `days` is either archived
//! (the default, reversible) or deleted outright. The switch, the action, and
//! the threshold live in `~/.orbit-pi/session-retention.json`, parsed
//! leniently so a malformed file only falls back to the defaults.
//!
//! Age is measured from [`SessionInfo::modified`] — the newest `message`
//! entry — never the session's creation timestamp or the file mtime, so a
//! session that was merely opened (bookkeeping appends) is not treated as
//! active.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

use serde_json::{json, Value};

use crate::sessions::{self, SessionInfo};

/// Thresholds (days) offered in Settings and accepted from the store.
pub const DAY_PRESETS: [u32; 7] = [7, 14, 30, 60, 90, 180, 365];
const DEFAULT_DAYS: u32 = 30;

/// What to do with a session past the threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetentionMode {
    /// Hide it from the default sidebar view; the file stays on disk.
    Archive,
    /// Remove the session file (and its pin/archive entries).
    Delete,
}

impl RetentionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Delete => "delete",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "delete" => Self::Delete,
            _ => Self::Archive,
        }
    }
}

/// Persisted retention preference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionConfig {
    /// Run the policy at launch. Off until the user opts in.
    pub enabled: bool,
    pub mode: RetentionMode,
    /// A session is eligible once it has been idle this many days.
    pub days: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: RetentionMode::Archive,
            days: DEFAULT_DAYS,
        }
    }
}

impl RetentionConfig {
    /// Read `~/.orbit-pi/session-retention.json`, defaulting on any failure.
    pub fn load() -> Self {
        std::fs::read_to_string(store_path())
            .map(|raw| Self::from_json(&raw))
            .unwrap_or_default()
    }

    /// Parse the persisted shape. Missing or illegal fields fall back to the
    /// defaults rather than erroring.
    pub fn from_json(raw: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        let enabled = value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mode = value
            .get("mode")
            .and_then(Value::as_str)
            .map(RetentionMode::from_key)
            .unwrap_or(RetentionMode::Archive);
        let days = value
            .get("days")
            .and_then(Value::as_u64)
            .and_then(|days| u32::try_from(days).ok())
            .filter(|days| DAY_PRESETS.contains(days))
            .unwrap_or(DEFAULT_DAYS);
        Self {
            enabled,
            mode,
            days,
        }
    }

    pub fn to_json(&self) -> String {
        json!({
            "enabled": self.enabled,
            "mode": self.mode.as_str(),
            "days": self.days,
        })
        .to_string()
    }

    /// Persist to `~/.orbit-pi/session-retention.json`. Best-effort: a failed
    /// write keeps the previous setting on disk and is reported on stderr.
    pub fn persist(&self) {
        let path = store_path();
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "Orbit retention: could not create {}: {error}",
                    parent.display()
                );
                return;
            }
        }
        if let Err(error) = std::fs::write(&path, self.to_json()) {
            eprintln!(
                "Orbit retention: could not write {}: {error}",
                path.display()
            );
        }
    }
}

/// `~/.orbit-pi/session-retention.json` — Orbit-owned policy, never written
/// into pi's session files.
pub fn store_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("session-retention.json")
}

/// What a retention pass did, for the launch toast and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RetentionOutcome {
    pub archived: usize,
    pub deleted: usize,
    pub skipped_live: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

impl RetentionOutcome {
    /// Sessions acted on (archived or deleted).
    pub fn touched(&self) -> usize {
        self.archived + self.deleted
    }

    /// Whether anything at all happened — decides if a toast is due.
    pub fn changed(&self) -> bool {
        self.touched() > 0 || !self.skipped_live.is_empty() || !self.failed.is_empty()
    }
}

/// Select and apply the policy. `live` holds session paths with a warm parked
/// process in this app; those are reported, never touched. `now` is injected
/// so the age boundary is testable.
pub fn run(config: &RetentionConfig, live: &HashSet<PathBuf>, now: SystemTime) -> RetentionOutcome {
    let candidates = sessions::older_than(config.days, now);
    apply(&candidates, config.mode, live)
}

/// Apply `mode` to an already age-selected candidate list. Split from [`run`]
/// so the live guard is testable without a real pi store.
pub fn apply(
    candidates: &[SessionInfo],
    mode: RetentionMode,
    live: &HashSet<PathBuf>,
) -> RetentionOutcome {
    let mut outcome = RetentionOutcome::default();
    for session in candidates {
        if live.contains(&session.path) {
            outcome.skipped_live.push(session.path.clone());
            continue;
        }
        match mode {
            RetentionMode::Archive => {
                crate::archive::add(&session.path);
                outcome.archived += 1;
            }
            RetentionMode::Delete => match std::fs::remove_file(&session.path) {
                Ok(()) => {
                    // A deleted session must not leave a stale pin or archive
                    // entry behind (same cleanup as the sidebar's delete).
                    crate::pins::remove(&session.path);
                    crate::archive::remove(&session.path);
                    outcome.deleted += 1;
                }
                Err(err) => outcome.failed.push((session.path.clone(), err.to_string())),
            },
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn defaults_are_off_archive_thirty() {
        let config = RetentionConfig::default();
        assert!(!config.enabled, "auto-run is opt-in");
        assert_eq!(config.mode, RetentionMode::Archive);
        assert_eq!(config.days, 30);
    }

    #[test]
    fn json_round_trips() {
        let config = RetentionConfig {
            enabled: true,
            mode: RetentionMode::Delete,
            days: 90,
        };
        assert_eq!(RetentionConfig::from_json(&config.to_json()), config);
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        assert_eq!(
            RetentionConfig::from_json("not json"),
            RetentionConfig::default()
        );
        // An off-preset day count and an unknown mode snap back to defaults.
        assert_eq!(RetentionConfig::from_json(r#"{"days":3}"#).days, 30);
        assert_eq!(
            RetentionConfig::from_json(r#"{"mode":"nuke"}"#).mode,
            RetentionMode::Archive
        );
    }

    #[test]
    fn live_sessions_are_skipped_in_both_modes() {
        let candidate = SessionInfo {
            path: PathBuf::from("/store/live.jsonl"),
            id: "live".into(),
            cwd: PathBuf::from("/tmp/ws"),
            title: "live".into(),
            first_message: "hi".into(),
            modified: SystemTime::now() - Duration::from_secs(60 * 86_400),
        };
        let live: HashSet<PathBuf> = [candidate.path.clone()].into_iter().collect();
        for mode in [RetentionMode::Archive, RetentionMode::Delete] {
            let outcome = apply(std::slice::from_ref(&candidate), mode, &live);
            assert!(outcome.changed());
            assert_eq!(
                outcome.touched(),
                0,
                "{mode:?} must not act on a live session"
            );
            assert_eq!(outcome.skipped_live, vec![candidate.path.clone()]);
            assert!(outcome.failed.is_empty());
        }
    }
}
