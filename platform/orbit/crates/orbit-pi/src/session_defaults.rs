//! The default session model — the model and thinking level every new Orbit
//! session starts on.
//!
//! Orbit persists one optional model (`provider` + `model_id`) and its
//! thinking level to `~/.orbit-pi/session-defaults.json`:
//!
//! ```jsonc
//! { "provider": "anthropic", "model_id": "claude-sonnet-4-5", "thinking": "high" }
//! ```
//!
//! An empty slot means "leave pi's own startup default alone" — Orbit never
//! forces a model the user did not pick. The file is Orbit-owned; it is
//! deliberately *not* pi's `~/.pi/agent/settings.json`, which is global to the
//! machine and would silently change terminal `pi` sessions.
//!
//! Two apply paths use this config (see `OrbitApp::apply_default_model`):
//! [`SessionDefault::cli_args`] for the process that hosts a session's first
//! turn, and `set_model` / `set_thinking_level` RPC for every later session
//! born inside a live process. Parsing is lenient so a hand-edited or
//! half-written file can only fall back to "no default"; the thinking level is
//! validated against the model's catalog entry when it is applied, never at
//! parse time.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The default model. `model_id` is the determinant; `provider` is optional
/// and lets pi resolve a model whose id exists on several providers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct SessionDefault {
    /// pi provider id (`anthropic`). `None` lets pi fuzzy-match the model.
    pub provider: Option<String>,
    /// pi model id (`claude-sonnet-4-5`). `None` means the slot is unset.
    pub model_id: Option<String>,
    /// Thinking level (`off` … `max`). Ignored without a model and validated
    /// against the model's catalog entry at apply time, never at parse time.
    pub thinking: Option<String>,
}

impl SessionDefault {
    /// Whether no default is configured.
    pub fn is_unset(&self) -> bool {
        self.model_id.is_none()
    }

    /// The extra `pi` CLI args this default implies at process spawn — empty
    /// when unset. `--model provider/id` keeps the provider explicit; a slot
    /// without a provider emits just `--model id` and lets pi fuzzy-match.
    /// `--thinking` is independent of the model suffix so the two stay
    /// readable; pi clamps the level to the model's capabilities.
    pub fn cli_args(&self) -> Vec<String> {
        let Some(id) = self.model_id.as_deref().filter(|id| !id.is_empty()) else {
            return Vec::new();
        };
        let mut args = Vec::new();
        match self
            .provider
            .as_deref()
            .filter(|provider| !provider.is_empty())
        {
            Some(provider) => {
                args.push("--provider".into());
                args.push(provider.to_string());
                args.push("--model".into());
                args.push(format!("{provider}/{id}"));
            }
            None => {
                args.push("--model".into());
                args.push(id.to_string());
            }
        }
        if let Some(level) = self.thinking.as_deref().filter(|level| !level.is_empty()) {
            args.push("--thinking".into());
            args.push(level.to_string());
        }
        args
    }

    /// Read `~/.orbit-pi/session-defaults.json`, defaulting on any failure.
    pub fn load() -> Self {
        Self::load_at(&store_path())
    }

    /// Persist to `~/.orbit-pi/session-defaults.json`. Best-effort: the app
    /// simply keeps the previous choice if the write fails.
    pub fn persist(&self) {
        self.write_at(&store_path());
    }

    /// Parse the persisted shape. Unknown keys are ignored and a malformed
    /// file loads as "no default" rather than erroring. The per-mode shape a
    /// pre-release build wrote has no top-level model, so it degrades to unset.
    pub fn from_json(raw: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        let Some(object) = value.as_object() else {
            return Self::default();
        };
        let text = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let model_id = text("model_id").or_else(|| text("modelId"));
        // A provider or thinking level without a model is not a default; drop
        // them so the persisted shape stays either a full model or nothing.
        let thinking = model_id.as_ref().and_then(|_| text("thinking"));
        Self {
            provider: model_id.as_ref().and_then(|_| text("provider")),
            model_id,
            thinking,
        }
    }

    /// The wire shape written to disk.
    pub fn to_json(&self) -> String {
        let value = json!({
            "provider": self.provider,
            "model_id": self.model_id,
            "thinking": self.thinking,
        });
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    }

    fn load_at(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|raw| Self::from_json(&raw))
            .unwrap_or_default()
    }

    fn write_at(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Temp-file + rename so a concurrent reader (or a crash) never sees a
        // partial file — the same convention `workflow.rs` uses.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, self.to_json()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// `~/.orbit-pi/session-defaults.json`.
pub(crate) fn store_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("session-defaults.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let unique = format!(
            "orbit-session-defaults-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir()
            .join(unique)
            .join("session-defaults.json")
    }

    #[test]
    fn empty_slot_emits_no_args() {
        assert!(SessionDefault::default().cli_args().is_empty());
        assert!(SessionDefault::default().is_unset());
    }

    #[test]
    fn model_with_provider_emits_provider_and_model() {
        let slot = SessionDefault {
            provider: Some("anthropic".into()),
            model_id: Some("claude-sonnet-4-5".into()),
            ..Default::default()
        };
        assert_eq!(
            slot.cli_args(),
            vec![
                "--provider",
                "anthropic",
                "--model",
                "anthropic/claude-sonnet-4-5",
            ]
        );
    }

    #[test]
    fn model_with_thinking_appends_the_level() {
        let slot = SessionDefault {
            provider: Some("anthropic".into()),
            model_id: Some("claude-sonnet-4-5".into()),
            thinking: Some("high".into()),
        };
        assert_eq!(
            slot.cli_args(),
            vec![
                "--provider",
                "anthropic",
                "--model",
                "anthropic/claude-sonnet-4-5",
                "--thinking",
                "high",
            ]
        );
    }

    #[test]
    fn provider_less_model_emits_just_the_model() {
        let slot = SessionDefault {
            model_id: Some("claude-sonnet-4-5".into()),
            ..Default::default()
        };
        assert_eq!(slot.cli_args(), vec!["--model", "claude-sonnet-4-5"]);
    }

    #[test]
    fn json_round_trips() {
        let slot = SessionDefault {
            provider: Some("anthropic".into()),
            model_id: Some("claude-sonnet-4-5".into()),
            thinking: Some("high".into()),
        };
        assert_eq!(SessionDefault::from_json(&slot.to_json()), slot);
        assert_eq!(
            SessionDefault::from_json(&SessionDefault::default().to_json()),
            SessionDefault::default()
        );
    }

    #[test]
    fn malformed_or_legacy_shapes_load_as_unset() {
        assert_eq!(
            SessionDefault::from_json("not json"),
            SessionDefault::default()
        );
        assert_eq!(SessionDefault::from_json("{}"), SessionDefault::default());
        assert_eq!(
            SessionDefault::from_json("[1,2]"),
            SessionDefault::default()
        );
        // A stray provider without a model is not a default.
        assert_eq!(
            SessionDefault::from_json(r#"{"provider":"anthropic"}"#),
            SessionDefault::default()
        );
        // A thinking level without a model would land on pi's own startup
        // model; the slot is empty instead.
        let slot = SessionDefault::from_json(r#"{"thinking":"high"}"#);
        assert!(slot.is_unset());
        assert!(slot.cli_args().is_empty());
        // The per-mode shape a pre-release build wrote is ignored, not
        // misread as a top-level model.
        assert_eq!(
            SessionDefault::from_json(
                r#"{"plan":{"provider":"anthropic","model_id":"claude-sonnet-4-5"}}"#
            ),
            SessionDefault::default()
        );
    }

    #[test]
    fn store_round_trips_on_disk() {
        let path = temp_path("store");
        let slot = SessionDefault {
            provider: Some("ollama".into()),
            model_id: Some("glm-5.3".into()),
            thinking: Some("low".into()),
        };
        slot.write_at(&path);
        assert_eq!(SessionDefault::load_at(&path), slot);
        // A malformed file degrades to empty, never an error.
        std::fs::write(&path, "{ broken").unwrap();
        assert_eq!(SessionDefault::load_at(&path), SessionDefault::default());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn store_path_is_orbit_owned() {
        // Deliberately not `~/.pi/agent/settings.json` — see the module docs.
        assert!(store_path().ends_with(".orbit-pi/session-defaults.json"));
    }
}
