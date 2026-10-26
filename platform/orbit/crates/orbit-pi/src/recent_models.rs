//! Recently used models — a small, persisted MRU shortlist surfaced as a
//! section in the composer's model picker and as its own scope.
//!
//! The list is process-global (like `favorites`) so the picker can read and
//! record it without threading state through GPUI entities. It is persisted
//! to `~/.orbit-pi/recent-models.json` as `{ "models": [{"provider","id"}, …] }`
//! most-recent first, keyed by provider + id so a rename or a catalog reshuffle
//! never strands an entry.

use std::path::PathBuf;
use std::sync::RwLock;

use serde_json::{json, Value};

/// How many recently used models to remember.
pub const MAX_RECENTS: usize = 5;

/// A most-recently-used list of models, identified by provider + id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recents {
    /// Most-recent first, at most [`MAX_RECENTS`] long.
    entries: Vec<(String, String)>,
}

impl Recents {
    fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(persist_path()) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Self::default();
        };
        let mut entries: Vec<(String, String)> = value
            .get("models")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|entry| {
                        let provider = entry.get("provider").and_then(Value::as_str)?;
                        let id = entry.get("id").and_then(Value::as_str)?;
                        Some((provider.to_string(), id.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.truncate(MAX_RECENTS);
        Self { entries }
    }

    fn persist(&self) {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let models: Vec<Value> = self
            .entries
            .iter()
            .map(|(provider, id)| json!({ "provider": provider, "id": id }))
            .collect();
        let _ = std::fs::write(path, json!({ "models": models }).to_string());
    }

    pub fn contains(&self, provider: &str, id: &str) -> bool {
        self.entries.iter().any(|(p, i)| p == provider && i == id)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries in most-recent-first order.
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    /// Move a model to the front (inserting it if new) and cap the list.
    pub fn touch(&mut self, provider: &str, id: &str) {
        self.entries.retain(|(p, i)| !(p == provider && i == id));
        self.entries
            .insert(0, (provider.to_string(), id.to_string()));
        self.entries.truncate(MAX_RECENTS);
    }

    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self {
            entries: pairs
                .iter()
                .map(|(p, id)| (p.to_string(), id.to_string()))
                .collect(),
        }
    }
}

fn persist_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("recent-models.json")
}

/// Lazily-loaded global store. Reads and writes lock briefly; the list is tiny.
static STORE: RwLock<Option<Recents>> = RwLock::new(None);

fn with_store<R>(f: impl FnOnce(&mut Recents) -> R) -> R {
    let mut guard = STORE.write().unwrap();
    f(guard.get_or_insert_with(Recents::load))
}

/// A snapshot of the current recents for rendering.
pub fn all() -> Recents {
    with_store(|store| store.clone())
}

/// Record a model as the most recently used, persisting only when the order
/// actually changes (re-selecting the already-leading model is a no-op).
pub fn record(provider: &str, id: &str) {
    with_store(|store| {
        let before = store.clone();
        store.touch(provider, id);
        if *store != before {
            store.persist();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_moves_a_model_to_the_front() {
        let mut recents = Recents::from_pairs(&[("openai", "gpt-5"), ("anthropic", "claude")]);
        recents.touch("anthropic", "claude");
        // Already leading: order is unchanged.
        assert_eq!(
            recents.entries(),
            [
                ("anthropic".to_string(), "claude".to_string()),
                ("openai".to_string(), "gpt-5".to_string())
            ]
        );
        recents.touch("openai", "gpt-5");
        assert_eq!(recents.entries()[0].0, "openai");
    }

    #[test]
    fn touch_inserts_new_and_caps_the_list() {
        let mut recents = Recents::default();
        for ix in 0..(MAX_RECENTS + 3) {
            recents.touch("p", &format!("m{ix}"));
        }
        assert_eq!(recents.entries().len(), MAX_RECENTS);
        // Newest first, oldest evicted.
        assert_eq!(recents.entries()[0].1, format!("m{}", MAX_RECENTS + 2));
        assert!(!recents.contains("p", "m0"));
    }

    #[test]
    fn identity_is_provider_and_id() {
        let recents = Recents::from_pairs(&[("openai", "gpt-5")]);
        assert!(recents.contains("openai", "gpt-5"));
        assert!(!recents.contains("anthropic", "gpt-5"));
    }
}
