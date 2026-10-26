//! Pinned sessions — Orbit's own shortlist of session files, kept at the top
//! of their project group in the sidebar.
//!
//! The set is process-global (like the model favorites) so the sidebar's
//! build and render paths can read and toggle it without threading state
//! through GPUI entities. It is persisted to
//! `~/.orbit-pi/pinned-sessions.json` as `{ "sessions": ["<path>", …] }`,
//! keyed by the session file's `.jsonl` path — the same key the app already
//! uses for the active session and the live-process guards. Orbit-owned: a
//! pin is never written into pi's session files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde_json::{json, Value};

/// The user's pinned sessions, identified by their session-file path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pins {
    entries: Vec<PathBuf>,
}

impl Pins {
    fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(persist_path()) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Self::default();
        };
        let entries = value
            .get("sessions")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        Self { entries }
    }

    fn persist(&self) {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let sessions: Vec<String> = self
            .entries
            .iter()
            .map(|entry| entry.to_string_lossy().into_owned())
            .collect();
        let _ = std::fs::write(path, json!({ "sessions": sessions }).to_string());
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.entries.iter().any(|entry| entry == path)
    }

    /// Membership lookups for the sidebar's render path (O(1) per row).
    pub fn paths(&self) -> HashSet<PathBuf> {
        self.entries.iter().cloned().collect()
    }

    /// Add or remove a session; returns the new pinned state.
    pub fn toggle(&mut self, path: &Path) -> bool {
        match self.entries.iter().position(|entry| entry == path) {
            Some(ix) => {
                self.entries.remove(ix);
                false
            }
            None => {
                self.entries.push(path.to_path_buf());
                true
            }
        }
    }

    /// Forget a session (e.g. after its file was deleted).
    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|entry| entry != path);
    }

    #[cfg(test)]
    pub fn from_paths(paths: &[&str]) -> Self {
        Self {
            entries: paths.iter().map(PathBuf::from).collect(),
        }
    }
}

fn persist_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("pinned-sessions.json")
}

/// Lazily-loaded global store. Reads and writes lock briefly; the set is tiny.
static STORE: RwLock<Option<Pins>> = RwLock::new(None);

fn with_store<R>(f: impl FnOnce(&mut Pins) -> R) -> R {
    let mut guard = STORE.write().unwrap();
    f(guard.get_or_insert_with(Pins::load))
}

/// A snapshot of the current pins for rendering.
pub fn all() -> Pins {
    with_store(|store| store.clone())
}

/// Whether a session path is pinned.
pub fn contains(path: &Path) -> bool {
    with_store(|store| store.contains(path))
}

/// Flip a session's pinned state and persist the change.
pub fn toggle(path: &Path) {
    with_store(|store| {
        store.toggle(path);
        store.persist();
    });
}

/// Drop a pin — called when the session file is deleted.
pub fn remove(path: &Path) {
    with_store(|store| {
        if store.contains(path) {
            store.remove(path);
            store.persist();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_adds_then_removes() {
        let mut pins = Pins::default();
        assert!(pins.paths().is_empty());
        assert!(pins.toggle(Path::new("/store/a.jsonl")));
        assert!(pins.contains(Path::new("/store/a.jsonl")));
        assert!(!pins.toggle(Path::new("/store/a.jsonl")));
        assert!(pins.paths().is_empty());
    }

    #[test]
    fn identity_is_the_exact_path() {
        let pins = Pins::from_paths(&["/store/a.jsonl"]);
        assert!(pins.contains(Path::new("/store/a.jsonl")));
        assert!(!pins.contains(Path::new("/store/b.jsonl")));
    }

    #[test]
    fn remove_forgets_a_pin() {
        let mut pins = Pins::from_paths(&["/store/a.jsonl", "/store/b.jsonl"]);
        pins.remove(Path::new("/store/a.jsonl"));
        assert!(!pins.contains(Path::new("/store/a.jsonl")));
        assert!(pins.contains(Path::new("/store/b.jsonl")));
    }
}
