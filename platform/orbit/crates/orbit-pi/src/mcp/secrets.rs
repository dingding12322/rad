//! Orbit's MCP secret store.
//!
//! Pi expands `${NAME}` references in `mcp.json` from its process
//! environment. Orbit keeps the values behind those references in a
//! permissions-restricted file (`~/.orbit-pi/mcp-secrets.json`, mode 0600 on
//! Unix) and injects them into every Pi spawn and every `pi mcp list` probe,
//! so the configuration file itself never needs a raw credential.
//!
//! The trust model is deliberately the same as Pi's own credential files
//! under `~/.pi/agent/`: local, user-scoped, never logged. Everything Orbit
//! displays or logs passes through [`McpSecrets::redact`]. A keychain-backed
//! store can replace this module behind the same API later.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::{config, McpError};

/// The secrets Orbit tracks for MCP `${NAME}` references.
pub(crate) struct McpSecrets {
    path: PathBuf,
    values: BTreeMap<String, String>,
    /// Why the store could not be read, if it could not. Writes are refused
    /// while this is set, so an unreadable file is never overwritten with an
    /// empty store.
    load_error: Option<String>,
}

impl McpSecrets {
    /// Load from an explicit path — the seam the manager and tests drive.
    pub(crate) fn load_from(path: PathBuf) -> Self {
        let (values, load_error) = read(&path);
        Self {
            path,
            values,
            load_error,
        }
    }

    /// The reason the store could not be read, for the MCP page's error list.
    pub(crate) fn error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Re-read the store from disk. The manager calls this on refresh so a
    /// fixed or externally edited file is picked up without restarting the
    /// app.
    pub(crate) fn reload(&mut self) {
        let (values, load_error) = read(&self.path);
        self.values = values;
        self.load_error = load_error;
    }

    #[cfg(test)]
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Store (or replace) one secret and persist immediately. A failed write
    /// leaves the in-memory value in place and reports the error; the caller
    /// must not write a config that references it then.
    pub(crate) fn set(&mut self, name: &str, value: &str) -> Result<(), McpError> {
        self.values.insert(name.to_string(), value.to_string());
        self.persist()
    }

    /// Remove one secret, if present (test-facing; production pruning goes
    /// through [`Self::prune_unreferenced`]).
    #[cfg(test)]
    pub(crate) fn remove(&mut self, name: &str) -> Result<(), McpError> {
        if self.values.remove(name).is_none() {
            return Ok(());
        }
        self.persist()
    }

    /// Every value, for injection into a child process environment. The key
    /// is the `${NAME}` reference's name.
    pub(crate) fn env(&self) -> Vec<(String, String)> {
        self.values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    /// Mask every known secret value found in `text`, plus any `Bearer`
    /// credential. Used on Pi-reported errors and connection stderr before
    /// they reach the UI or a log.
    pub(crate) fn redact(&self, text: &str) -> String {
        let known: Vec<String> = self.values.values().cloned().collect();
        config::scrub_error(text, &known)
    }

    /// Drop Orbit-generated secrets that no configured server references
    /// anymore — but only those in `namespaces`, the generated-name prefixes
    /// the current workspace owns. Secrets belonging to other workspaces (or
    /// names outside Orbit's generated prefixes) are never touched, so
    /// removing a server in one project cannot delete another project's
    /// credentials.
    pub(crate) fn prune_unreferenced(
        &mut self,
        referenced: &std::collections::BTreeSet<String>,
        namespaces: &[String],
    ) -> Result<(), McpError> {
        let stale: Vec<String> = self
            .values
            .keys()
            .filter(|name| {
                namespaces
                    .iter()
                    .any(|namespace| name.starts_with(namespace.as_str()))
                    && !referenced.contains(*name)
            })
            .cloned()
            .collect();
        if stale.is_empty() {
            return Ok(());
        }
        for name in stale {
            self.values.remove(&name);
        }
        self.persist()
    }

    fn persist(&self) -> Result<(), McpError> {
        // Never clobber a file this instance could not read: the in-memory
        // values do not include whatever it held.
        if let Some(detail) = &self.load_error {
            return Err(McpError::Io(detail.clone()));
        }
        let mut secrets = Map::new();
        for (name, value) in &self.values {
            secrets.insert(name.clone(), Value::String(value.clone()));
        }
        let mut root = Map::new();
        root.insert("version".into(), Value::Number(1.into()));
        root.insert("secrets".into(), Value::Object(secrets));
        let doc = Value::Object(root);
        let text = config::serialize(&doc, b"  ")?;
        config::write_atomic_secure(&self.path, &text)?;
        Ok(())
    }
}

/// The store's location under a given home: `<home>/.orbit-pi/mcp-secrets.json`.
pub(crate) fn default_path_for(home: &Path) -> PathBuf {
    home.join(".orbit-pi").join("mcp-secrets.json")
}

/// Read the store. A missing or empty file is an empty store; an unreadable or
/// malformed file is reported (and kept from being overwritten) rather than
/// silently degraded to empty.
fn read(path: &Path) -> (BTreeMap<String, String>, Option<String>) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (BTreeMap::new(), None),
        Err(err) => return (BTreeMap::new(), Some(store_error(path, err))),
    };
    if text.trim().is_empty() {
        return (BTreeMap::new(), None);
    }
    let doc: Value = match serde_json::from_str(&text) {
        Ok(doc) => doc,
        Err(err) => return (BTreeMap::new(), Some(store_error(path, err))),
    };
    let values = match doc.get("secrets") {
        None => BTreeMap::new(),
        Some(Value::Object(secrets)) => secrets
            .iter()
            .filter_map(|(name, value)| {
                value
                    .as_str()
                    .map(|value| (name.clone(), value.to_string()))
            })
            .collect(),
        Some(other) => {
            return (
                BTreeMap::new(),
                Some(store_error(
                    path,
                    format!("`secrets` must be an object, got {other}"),
                )),
            )
        }
    };
    (values, None)
}

fn store_error(path: &Path, reason: impl std::fmt::Display) -> String {
    format!("{}: {reason}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn temp_store(tag: &str) -> (PathBuf, McpSecrets) {
        let dir = std::env::temp_dir().join(format!(
            "orbit-mcp-secrets-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp-secrets.json");
        (path.clone(), McpSecrets::load_from(path))
    }

    #[test]
    fn set_round_trips_through_disk() {
        let (path, mut secrets) = temp_store("roundtrip");
        secrets.set("MCP_GITHUB_TOKEN", "ghp_secret").unwrap();
        let reloaded = McpSecrets::load_from(path);
        assert_eq!(reloaded.get("MCP_GITHUB_TOKEN"), Some("ghp_secret"));
        assert_eq!(
            reloaded.env(),
            vec![("MCP_GITHUB_TOKEN".to_string(), "ghp_secret".to_string())]
        );
    }

    #[test]
    fn remove_drops_the_entry() {
        let (path, mut secrets) = temp_store("remove");
        secrets.set("A", "1").unwrap();
        secrets.remove("A").unwrap();
        assert!(!McpSecrets::load_from(path).contains("A"));
    }

    #[test]
    fn redaction_masks_known_values_and_bearer_credentials() {
        let (_path, mut secrets) = temp_store("redact");
        secrets.set("MCP_TOKEN", "supersecretvalue").unwrap();
        let text = secrets.redact("failed with supersecretvalue and Bearer abcdefghijklmnop");
        assert!(!text.contains("supersecretvalue"));
        assert!(!text.contains("abcdefghijklmnop"));
    }

    #[test]
    fn pruning_only_touches_the_given_namespaces() {
        let (path, mut secrets) = temp_store("prune");
        secrets.set("MCP_GLOBAL_OWNED", "a").unwrap();
        secrets.set("MCP_P0000000000000001_OTHER", "b").unwrap();
        secrets.set("USER_PROVIDED", "c").unwrap();
        let referenced = BTreeSet::new();
        secrets
            .prune_unreferenced(&referenced, &["MCP_GLOBAL_".to_string()])
            .unwrap();
        let reloaded = McpSecrets::load_from(path);
        // Only the current workspace's global namespace is pruned; another
        // project's secret and user-provided names are untouched.
        assert!(!reloaded.contains("MCP_GLOBAL_OWNED"));
        assert!(reloaded.contains("MCP_P0000000000000001_OTHER"));
        assert!(reloaded.contains("USER_PROVIDED"));
    }

    #[test]
    fn a_malformed_store_is_reported_and_never_overwritten() {
        let (path, _) = temp_store("malformed");
        fs::write(&path, "not json").unwrap();
        let mut store = McpSecrets::load_from(path.clone());
        assert!(store.error().is_some());
        // A write is refused rather than clobbering the unreadable file.
        assert!(store.set("A", "1").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
    }

    #[test]
    fn a_missing_store_is_empty_and_writable() {
        let (path, store) = temp_store("missing");
        assert!(store.error().is_none());
        assert!(store.is_empty());
        let mut store = McpSecrets::load_from(path);
        store.set("A", "1").unwrap();
        assert_eq!(store.get("A"), Some("1"));
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (path, mut secrets) = temp_store("perms");
        secrets.set("A", "1").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
