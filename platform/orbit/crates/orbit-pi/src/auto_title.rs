//! Auto session titles — the setting Orbit's bundled title extension reads.
//!
//! Orbit ships `contrib/orbit-title-extension/` and loads it with
//! `pi --extension` on every session process. Once a session's first turn
//! settles, that extension asks a model for a short title from the first
//! exchange and calls `pi.setSessionName`, so the sidebar/top bar show a real
//! title instead of the raw first message.
//!
//! This module owns the two choices: whether to title at all, and which model
//! to ask. The active session model is the default; an override is a
//! `{provider, id}` pair picked in Settings → Agent. The config lives in
//! `~/.orbit-pi/auto-title.json` — the same file the extension reads — and is
//! parsed leniently so a malformed file can only fall back to the defaults.

use std::path::PathBuf;

use serde_json::{json, Value};

/// A model to ask for the title: the pi provider id and model id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TitleModel {
    pub provider: String,
    pub id: String,
}

/// Persisted auto-title preference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoTitleConfig {
    /// Generate a title after the first turn. On by default.
    pub enabled: bool,
    /// The model to ask. `None` means the session's active model.
    pub model: Option<TitleModel>,
}

impl Default for AutoTitleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: None,
        }
    }
}

impl AutoTitleConfig {
    /// Read `~/.orbit-pi/auto-title.json`, defaulting on any failure.
    pub fn load() -> Self {
        std::fs::read_to_string(store_path())
            .map(|raw| Self::from_json(&raw))
            .unwrap_or_default()
    }

    /// Parse the persisted shape. Unknown/missing fields fall back to the
    /// defaults; a half-written model is treated as "active model".
    pub fn from_json(raw: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        let enabled = value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let model = value.get("model").and_then(|model| {
            let provider = model.get("provider").and_then(Value::as_str)?;
            let id = model.get("id").and_then(Value::as_str)?;
            (!provider.is_empty() && !id.is_empty()).then(|| TitleModel {
                provider: provider.to_string(),
                id: id.to_string(),
            })
        });
        Self { enabled, model }
    }

    /// The wire shape the extension reads.
    pub fn to_json(&self) -> String {
        let model = self
            .model
            .as_ref()
            .map(|model| json!({ "provider": model.provider, "id": model.id }));
        json!({ "enabled": self.enabled, "model": model }).to_string()
    }

    /// Persist to `~/.orbit-pi/auto-title.json`. Best-effort: the extension
    /// simply keeps the previous/default setting if the write fails.
    pub fn persist(&self) {
        let path = store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, self.to_json());
    }
}

/// `~/.orbit-pi/auto-title.json` — the app writes it, the title extension
/// reads it.
pub(crate) fn store_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("auto-title.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_enabled_on_the_active_model() {
        let config = AutoTitleConfig::default();
        assert!(config.enabled);
        assert_eq!(config.model, None);
    }

    #[test]
    fn json_round_trips() {
        let config = AutoTitleConfig {
            enabled: false,
            model: Some(TitleModel {
                provider: "ollama".into(),
                id: "glm-5.3".into(),
            }),
        };
        assert_eq!(AutoTitleConfig::from_json(&config.to_json()), config);
    }

    #[test]
    fn malformed_json_falls_back_to_the_defaults() {
        assert_eq!(
            AutoTitleConfig::from_json("not json at all"),
            AutoTitleConfig::default()
        );
        assert_eq!(AutoTitleConfig::from_json("{}"), AutoTitleConfig::default());
    }

    #[test]
    fn a_partial_model_means_active_model() {
        // Missing provider / id / empty strings never fabricate a model.
        assert_eq!(
            AutoTitleConfig::from_json(r#"{"model":{"provider":"ollama"}}"#).model,
            None
        );
        assert_eq!(
            AutoTitleConfig::from_json(r#"{"model":{"provider":"","id":"x"}}"#).model,
            None
        );
        assert_eq!(
            AutoTitleConfig::from_json(r#"{"model":"ollama/glm"}"#).model,
            None
        );
    }

    #[test]
    fn explicit_false_disables() {
        assert!(!AutoTitleConfig::from_json(r#"{"enabled":false}"#).enabled);
    }

    #[test]
    fn store_path_is_the_file_the_extension_reads() {
        // Kept in sync with `CONFIG_FILE` in the JS extension.
        assert!(store_path().ends_with(".orbit-pi/auto-title.json"));
    }
}
