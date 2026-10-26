//! Archived sessions — Orbit's own set of session files hidden from the
//! sidebar's default view.
//!
//! The set is process-global (like pins) so the sidebar's build and render
//! paths can read and toggle it without threading state through GPUI
//! entities. It is persisted to `~/.orbit-pi/archived-sessions.json` as
//! `{ "sessions": ["<path>", …] }`, keyed by the session file's `.jsonl`
//! path — the same key the app already uses for the active session and the
//! live-process guards. Orbit-owned: archiving never writes into pi's session
//! files, and deleting the file drops the entry.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde_json::{json, Value};

/// The user's archived sessions, identified by their session-file path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchivedSessions {
    entries: Vec<PathBuf>,
}

impl ArchivedSessions {
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

    /// Membership lookups for the sidebar's build path (O(1) per row).
    pub fn paths(&self) -> HashSet<PathBuf> {
        self.entries.iter().cloned().collect()
    }

    /// Add or remove a session; returns the new archived state.
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

    /// Ensure a session is archived; returns whether it was newly added.
    /// Unlike [`toggle`], a repeated add is a no-op, so a bulk archive pass
    /// can never un-archive an already-archived session.
    pub fn add(&mut self, path: &Path) -> bool {
        if self.contains(path) {
            return false;
        }
        self.entries.push(path.to_path_buf());
        true
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
        .join("archived-sessions.json")
}

/// Lazily-loaded global store. Reads and writes lock briefly; the set is tiny.
static STORE: RwLock<Option<ArchivedSessions>> = RwLock::new(None);

fn with_store<R>(f: impl FnOnce(&mut ArchivedSessions) -> R) -> R {
    let mut guard = STORE.write().unwrap();
    f(guard.get_or_insert_with(ArchivedSessions::load))
}

/// A snapshot of the current archive set for rendering.
pub fn all() -> ArchivedSessions {
    with_store(|store| store.clone())
}

/// Whether a session path is archived.
pub fn contains(path: &Path) -> bool {
    with_store(|store| store.contains(path))
}

/// Ensure a session is archived and persist the change. Idempotent: an
/// already-archived path is left archived (see [`ArchivedSessions::add`]).
pub fn add(path: &Path) {
    with_store(|store| {
        if store.add(path) {
            store.persist();
        }
    });
}

/// Flip a session's archived state and persist the change.
pub fn toggle(path: &Path) {
    with_store(|store| {
        store.toggle(path);
        store.persist();
    });
}

/// Drop an archive entry — called when the session file is deleted.
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
        let mut archived = ArchivedSessions::default();
        assert!(archived.paths().is_empty());
        assert!(archived.toggle(Path::new("/store/a.jsonl")));
        assert!(archived.contains(Path::new("/store/a.jsonl")));
        assert!(!archived.toggle(Path::new("/store/a.jsonl")));
        assert!(archived.paths().is_empty());
    }

    #[test]
    fn identity_is_the_exact_path() {
        let archived = ArchivedSessions::from_paths(&["/store/a.jsonl"]);
        assert!(archived.contains(Path::new("/store/a.jsonl")));
        assert!(!archived.contains(Path::new("/store/b.jsonl")));
    }

    #[test]
    fn add_is_idempotent() {
        let mut archived = ArchivedSessions::default();
        assert!(archived.add(Path::new("/store/a.jsonl")));
        assert!(archived.contains(Path::new("/store/a.jsonl")));
        assert!(
            !archived.add(Path::new("/store/a.jsonl")),
            "a repeated add must not un-archive"
        );
        assert_eq!(archived.paths().len(), 1);
    }

    #[test]
    fn remove_forgets_an_entry() {
        let mut archived = ArchivedSessions::from_paths(&["/store/a.jsonl", "/store/b.jsonl"]);
        archived.remove(Path::new("/store/a.jsonl"));
        assert!(!archived.contains(Path::new("/store/a.jsonl")));
        assert!(archived.contains(Path::new("/store/b.jsonl")));
    }
}
