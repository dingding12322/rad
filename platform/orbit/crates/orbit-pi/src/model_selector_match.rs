//! Selection matching helpers — tested against live pi catalog shapes.

use crate::app::ModelEntry;

/// Normalize model identifiers for fuzzy comparison (case, spaces, hyphens).
fn normalize_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether `model` is the active session model.
pub(crate) fn is_model_selected(
    model: &ModelEntry,
    current_model: &str,
    current_model_id: &str,
    current_model_provider: &str,
) -> bool {
    let provider_ok = current_model_provider.is_empty()
        || model.provider.eq_ignore_ascii_case(current_model_provider);
    if !current_model_provider.is_empty() && !provider_ok {
        return false;
    }

    let keys = [normalize_key(&model.id), normalize_key(&model.name)];
    for current in [current_model_id, current_model] {
        if current.is_empty() {
            continue;
        }
        let needle = normalize_key(current);
        if keys.iter().any(|key| key == &needle) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, provider: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            name: name.into(),
            provider: provider.into(),
            context_window: None,
            thinking_levels: Vec::new(),
        }
    }

    #[test]
    fn matches_by_catalog_id() {
        let model = entry("qwen3-coder-flash", "Qwen3 Coder Flash", "opencode-go");
        assert!(is_model_selected(
            &model,
            "Qwen3 Coder Flash",
            "qwen3-coder-flash",
            "opencode-go",
        ));
    }

    #[test]
    fn matches_when_state_name_differs_by_hyphens() {
        let model = entry("minimax-m2.5", "MiniMax-M2.5", "opencode-go");
        assert!(is_model_selected(
            &model,
            "MiniMax M2.5",
            "minimax-m2.5",
            "opencode-go",
        ));
    }

    #[test]
    fn matches_by_name_when_id_empty() {
        let model = entry("glm-5.3-flash:cloud", "glm-5.3-flash:cloud", "ollama");
        assert!(is_model_selected(
            &model,
            "glm-5.3-flash:cloud",
            "",
            "ollama"
        ));
    }

    #[test]
    fn matches_live_glm_shape() {
        let model = entry("glm-5.3-flash:cloud", "GLM 5.3 Flash", "ollama");
        assert!(is_model_selected(
            &model,
            "GLM 5.3 Flash",
            "glm-5.3-flash:cloud",
            "ollama",
        ));
        assert!(is_model_selected(&model, "GLM 5.3 Flash", "", "ollama",));
    }

    #[test]
    fn rejects_different_provider() {
        let model = entry("gpt-4", "GPT-4", "openai");
        assert!(!is_model_selected(&model, "GPT-4", "gpt-4", "anthropic",));
    }
}
