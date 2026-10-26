//! Hidden quota providers — the top-bar usage popover's user-curated
//! shortlist of accounts the user does not want to see there.
//!
//! The set lives on the app (like [`crate::session_defaults::SessionDefault`])
//! so the popover reads it without a lock on the render path and tests can
//! seed it directly. It is persisted to
//! `~/.orbit-pi/hidden-quota-providers.json` as `{ "providers": ["cursor-acp",
//! …] }`, keyed by pi provider id so a catalog reshuffle never strands an
//! entry. Orbit-owned: this only hides the provider from the top-bar popover,
//! never from Settings → Providers or from pi itself.
//!
//! A hidden provider stays reachable: the popover keeps a collapsed "Hidden"
//! group so a mistoggle costs one click, not a hunt through the config.

use std::path::PathBuf;

use serde_json::{json, Value};

/// The provider ids hidden from the top-bar usage popover.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HiddenProviders {
    entries: Vec<String>,
}

impl HiddenProviders {
    /// Read the persisted set, defaulting on any failure so a hand-edited or
    /// half-written file can only mean "nothing hidden".
    pub fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(persist_path()) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Self::default();
        };
        let entries = value
            .get("providers")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .filter(|provider| !provider.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self { entries }
    }

    /// Write the set, best-effort (a failed write never blocks the UI).
    pub fn persist(&self) {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json!({ "providers": self.entries }).to_string());
    }

    /// Whether a provider is hidden from the popover.
    pub fn contains(&self, provider: &str) -> bool {
        self.entries.iter().any(|entry| entry == provider)
    }

    /// Flip a provider's hidden state; returns the new hidden state.
    pub fn toggle(&mut self, provider: &str) -> bool {
        match self.entries.iter().position(|entry| entry == provider) {
            Some(ix) => {
                self.entries.remove(ix);
                false
            }
            None => {
                self.entries.push(provider.to_string());
                true
            }
        }
    }

    #[cfg(test)]
    pub fn from_providers(providers: &[&str]) -> Self {
        Self {
            entries: providers.iter().map(|p| p.to_string()).collect(),
        }
    }
}

fn persist_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("hidden-quota-providers.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_adds_then_removes() {
        let mut hidden = HiddenProviders::default();
        assert!(!hidden.contains("cursor-acp"));
        assert!(hidden.toggle("cursor-acp"));
        assert!(hidden.contains("cursor-acp"));
        assert!(!hidden.toggle("cursor-acp"));
        assert!(!hidden.contains("cursor-acp"));
    }

    #[test]
    fn identity_is_the_provider_id() {
        let hidden = HiddenProviders::from_providers(&["cursor-acp"]);
        assert!(hidden.contains("cursor-acp"));
        assert!(!hidden.contains("deepseek"));
    }

    #[test]
    fn distinct_providers_are_independent() {
        let mut hidden = HiddenProviders::from_providers(&["cursor-acp"]);
        assert!(hidden.toggle("deepseek"));
        assert!(hidden.contains("cursor-acp"));
        assert!(hidden.contains("deepseek"));
    }
}
