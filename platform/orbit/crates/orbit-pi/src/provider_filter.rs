//! The composer's provider filter — the relay the model picker scopes to.
//!
//! Persisted to `~/.orbit-pi/provider-filter.json` so a chosen relay survives
//! restarts. An empty file (or `null`) means "no filter": the model picker
//! shows the full catalog, grouped by provider.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Read the persisted relay filter, `None` when unset or the file is absent.
pub(crate) fn load() -> Option<String> {
    // GUI tests must not depend on the developer machine's real filter —
    // a persisted relay would silently scope every test-opened model picker.
    #[cfg(test)]
    return None;
    #[cfg(not(test))]
    load_at(&path())
}

/// Persist the relay filter; `None` clears it. Best-effort like the other
/// Orbit state files — a failed write keeps the previous choice.
pub(crate) fn save(provider: Option<&str>) {
    #[cfg(test)]
    {
        let _ = provider;
    }
    #[cfg(not(test))]
    save_at(&path(), provider);
}

fn load_at(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value
        .get("provider")
        .and_then(Value::as_str)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
}

fn save_at(path: &Path, provider: Option<&str>) {
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json!({ "provider": provider }).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// `~/.orbit-pi/provider-filter.json`.
fn path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("provider-filter.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_round_trips_and_clears() {
        let dir = std::env::temp_dir().join(format!("orbit-provider-filter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider-filter.json");

        assert_eq!(load_at(&path), None);
        save_at(&path, Some("ccswitch-codex-timicc"));
        assert_eq!(load_at(&path).as_deref(), Some("ccswitch-codex-timicc"));
        save_at(&path, None);
        assert_eq!(load_at(&path), None);
    }
}
