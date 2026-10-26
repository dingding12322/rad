//! Favorite models — a small, persisted shortlist surfaced as a scope in
//! the composer's model picker.
//!
//! The set is process-global (like the font/theme statics) so the picker can
//! read and toggle it without threading state through GPUI entities. It is
//! persisted to `~/.orbit-pi/favorites.json` as `{ "models": [{"provider",
//! "id"}, …] }`, keyed by provider + id so a rename or a catalog reshuffle
//! never strands a favorite.

use std::path::PathBuf;
use std::sync::RwLock;

use serde_json::{json, Value};

/// A user's favorited models, identified by provider + id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Favorites {
    entries: Vec<(String, String)>,
}

impl Favorites {
    fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(persist_path()) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Self::default();
        };
        let entries = value
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

    /// Add or remove a model; returns the new favorite state.
    pub fn toggle(&mut self, provider: &str, id: &str) -> bool {
        match self
            .entries
            .iter()
            .position(|(p, i)| p == provider && i == id)
        {
            Some(ix) => {
                self.entries.remove(ix);
                false
            }
            None => {
                self.entries.push((provider.to_string(), id.to_string()));
                true
            }
        }
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
        .join("favorites.json")
}

/// Lazily-loaded global store. Reads and writes lock briefly; the set is tiny.
static STORE: RwLock<Option<Favorites>> = RwLock::new(None);

fn with_store<R>(f: impl FnOnce(&mut Favorites) -> R) -> R {
    let mut guard = STORE.write().unwrap();
    f(guard.get_or_insert_with(Favorites::load))
}

/// A snapshot of the current favorites for rendering.
pub fn all() -> Favorites {
    with_store(|store| store.clone())
}

pub fn contains(provider: &str, id: &str) -> bool {
    with_store(|store| store.contains(provider, id))
}

/// Flip a model's favorite state and persist the change.
pub fn toggle(provider: &str, id: &str) {
    with_store(|store| {
        store.toggle(provider, id);
        store.persist();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_adds_then_removes() {
        let mut favorites = Favorites::default();
        assert!(favorites.is_empty());
        assert!(favorites.toggle("openai", "gpt-5"));
        assert!(favorites.contains("openai", "gpt-5"));
        assert!(!favorites.toggle("openai", "gpt-5"));
        assert!(favorites.is_empty());
    }

    #[test]
    fn identity_is_provider_and_id() {
        let favorites = Favorites::from_pairs(&[("openai", "gpt-5")]);
        // Same id under another provider is a different favorite.
        assert!(favorites.contains("openai", "gpt-5"));
        assert!(!favorites.contains("anthropic", "gpt-5"));
    }
}
