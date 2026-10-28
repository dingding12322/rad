//! Provider configuration — Orbit's read/write surface for pi's
//! `~/.pi/agent/models.json`.
//!
//! pi reads custom providers and built-in overrides from this file; Orbit
//! edits the same file, so an entry added here works in the CLI too and one
//! added from the CLI shows up here on refresh. The boundary is deliberate:
//! Orbit only ever touches the `providers` object and leaves every other
//! top-level key untouched, so hand-written config survives a round trip.
//!
//! Nothing here talks to the pi process. `get_available_models` reports what
//! the *running* agent advertises; this module reports what the *file* says.
//! The Providers page combines both to tell "Active" from "Configured but
//! not loaded".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Map, Value};

/// The pi CLI's own `models.json` store under the harness's agent dir.
pub(crate) fn models_json_path() -> PathBuf {
    orbit_rpc::agent_dir()
        .join("models.json")
}

/// One provider entry from `models.json`, flattened to what the UI edits.
#[derive(Debug, Clone)]
pub(crate) struct CustomProvider {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) base_url: String,
    pub(crate) api: String,
    /// A key (or `$ENV` / `!command`) is stored in the file.
    pub(crate) has_api_key: bool,
    pub(crate) model_ids: Vec<String>,
}

/// Read the `providers` object. A missing file is an empty list; malformed
/// JSON is surfaced so the page can warn instead of silently overwriting it.
pub(crate) fn read_custom() -> Result<Vec<CustomProvider>, String> {
    read_custom_at(&models_json_path())
}

fn read_custom_at(path: &Path) -> Result<Vec<CustomProvider>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(tr!(
                "errors.could_not_read",
                path = path.display().to_string(),
                error = err
            ))
        }
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let root: Value =
        serde_json::from_str(&raw).map_err(|err| tr!("errors.models_json_invalid", error = err))?;
    let Some(providers) = root.get("providers").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(providers.len());
    for (id, entry) in providers {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        out.push(CustomProvider {
            id: id.clone(),
            name: entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string),
            base_url: entry
                .get("baseUrl")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            api: entry
                .get("api")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            has_api_key: entry
                .get("apiKey")
                .and_then(Value::as_str)
                .is_some_and(|key| !key.is_empty()),
            model_ids: entry
                .get("models")
                .and_then(Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter_map(|model| {
                            model.get("id").and_then(Value::as_str).map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Write (or update) one provider entry, preserving every other key in the
/// file. `models` is omitted when empty — for a built-in provider that keeps
/// its catalog; for a custom provider the caller validates non-empty first.
///
/// An empty `api_key` keeps whatever key the file already had, so re-saving
/// a provider never wipes a stored secret.
pub(crate) fn write_provider(
    id: &str,
    name: Option<&str>,
    base_url: &str,
    api: &str,
    api_key: Option<&str>,
    model_ids: &[String],
) -> Result<(), String> {
    write_provider_at(
        &models_json_path(),
        id,
        name,
        base_url,
        api,
        api_key,
        model_ids,
    )
}

/// Import CCSwitch's OpenAI/Codex providers into pi's shared provider files.
/// CCSwitch keeps the provider payload in SQLite; using its read-only CLI
/// export avoids a new database dependency while keeping secrets out of logs.
pub(crate) fn import_ccswitch() -> Result<(usize, usize), String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| tr!("providers.ccswitch_home_missing"))?;
    let database = home.join(".cc-switch").join("cc-switch.db");
    if !database.is_file() {
        return Err(tr!(
            "providers.ccswitch_database_missing",
            path = database.display().to_string()
        ));
    }
    let output = std::process::Command::new("sqlite3")
        .arg("-json")
        .arg(&database)
        .arg("SELECT p.app_type, p.id, p.name, p.settings_config, (SELECT e.url FROM provider_endpoints e WHERE e.provider_id = p.id AND e.app_type = p.app_type LIMIT 1) AS endpoint_url FROM providers p WHERE p.app_type = 'codex' ORDER BY p.name, p.id;")
        .output()
        .map_err(|err| tr!("providers.ccswitch_read_failed", error = err))?;
    if !output.status.success() {
        return Err(tr!("providers.ccswitch_read_failed", error = "sqlite3 exited with an error"));
    }
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout)
        .map_err(|err| tr!("providers.ccswitch_read_failed", error = err))?;

    // Earlier builds imported every app_type and guessed endpoints from the
    // vendor homepage; drop those leftovers so only the current codex pass
    // remains.
    ccswitch_remove_stale()?;

    let mut imported = 0;
    let mut skipped = 0;
    for row in rows {
        let Some(source_id) = row.get("id").and_then(Value::as_str) else {
            skipped += 1;
            continue;
        };
        let config = row
            .get("settings_config")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or(Value::Null);
        let endpoint_url = row.get("endpoint_url").and_then(Value::as_str);
        let Some((api, base_url, api_key)) = ccswitch_mapping(&config, endpoint_url) else {
            skipped += 1;
            continue;
        };
        let models = ccswitch_models(&config, &base_url, api_key.as_deref(), None);
        if models.is_empty() {
            skipped += 1;
            continue;
        }
        let id = ccswitch_provider_id(source_id, "codex");
        let model_ids: Vec<String> = models.iter().map(|model| model.id.clone()).collect();
        write_provider(
            &id,
            row.get("name").and_then(Value::as_str),
            &base_url,
            api,
            None,
            &model_ids,
        )?;
        write_provider_models_full(&id, &models)?;
        if let Some(key) = api_key {
            write_api_key(&id, &key)?;
        }
        imported += 1;
    }
    Ok((imported, skipped))
}

/// Remove providers this import no longer manages: earlier builds also wrote
/// `ccswitch-claude-*` / `ccswitch-gemini-*` entries from the same CCSwitch
/// database, and the user scoped the import to Codex relays only.
fn ccswitch_remove_stale() -> Result<(), String> {
    ccswitch_remove_stale_at(&models_json_path(), &auth_path())
}

fn ccswitch_remove_stale_at(models_path: &Path, auth_path: &Path) -> Result<(), String> {
    const STALE_PREFIXES: [&str; 2] = ["ccswitch-claude-", "ccswitch-gemini-"];
    if let Ok(raw) = std::fs::read_to_string(models_path) {
        if let Ok(mut root) = serde_json::from_str::<Value>(&raw) {
            let mut changed = false;
            if let Some(providers) = root.get_mut("providers").and_then(Value::as_object_mut) {
                let stale: Vec<String> = providers
                    .keys()
                    .filter(|id| STALE_PREFIXES.iter().any(|prefix| id.starts_with(prefix)))
                    .cloned()
                    .collect();
                for id in stale {
                    providers.remove(&id);
                    changed = true;
                }
            }
            if changed {
                write_json_secure(models_path, &root)?;
            }
        }
    }
    if let Ok(raw) = std::fs::read_to_string(auth_path) {
        if let Ok(mut root) = serde_json::from_str::<Value>(&raw) {
            let mut changed = false;
            if let Some(entries) = root.as_object_mut() {
                let stale: Vec<String> = entries
                    .keys()
                    .filter(|id| STALE_PREFIXES.iter().any(|prefix| id.starts_with(prefix)))
                    .cloned()
                    .collect();
                for id in stale {
                    entries.remove(&id);
                    changed = true;
                }
            }
            if changed {
                write_json_secure(auth_path, &root)?;
            }
        }
    }
    Ok(())
}


/// The provider's model list: every model the relay exposes via
/// `GET {base_url}/models`, with the model the Codex config currently
/// selects pinned to the front (the relay list may not contain it verbatim).
/// Falls back to just the configured model when the relay does not answer.
#[derive(Clone, Debug)]
struct CcswitchModel {
	id: String,
	context_window: Option<u64>,
}

/// The provider's model list, in priority order:
/// 1. the model the Codex config currently selects (pinned first, with the
///    context window its TOML declares)
/// 2. CCSwitch's own model catalog (`~/.codex/cc-switch-model-catalog.json`)
/// 3. Codex's official model cache (`~/.codex/models_cache.json`)
/// 4. whatever the relay itself lists on `GET {base_url}/models`
///
/// `codex_catalog_dir` overrides the `~/.codex` directory tests pin; `None`
/// reads the real one.
fn ccswitch_models(
	config: &Value,
	base_url: &str,
	api_key: Option<&str>,
	codex_catalog_dir: Option<&Path>,
) -> Vec<CcswitchModel> {
	let config_text = config.get("config").and_then(Value::as_str).unwrap_or("");
	let mut models: Vec<CcswitchModel> = Vec::new();
	let mut add = |id: &str, context_window: Option<u64>| {
		let id = id.trim();
		if id.is_empty() {
			return;
		}
		if let Some(existing) = models.iter_mut().find(|model| model.id == id) {
			if existing.context_window.is_none() {
				existing.context_window = context_window;
			}
			return;
		}
		models.push(CcswitchModel {
			id: id.to_string(),
			context_window,
		});
	};
	// 1. The configured model, with the context window its TOML declares.
	let current = ccswitch_toml_str(config_text, "model").unwrap_or_default();
	let window = ccswitch_toml_u64(config_text, "model_context_window");
	add(current, window);
	// 2/3. Catalogs under ~/.codex (CCSwitch's, then Codex's own cache).
	let codex_dir = match codex_catalog_dir {
		Some(dir) => dir.to_path_buf(),
		None => std::env::var_os("HOME")
			.or_else(|| std::env::var_os("USERPROFILE"))
			.map(PathBuf::from)
			.map(|home| home.join(".codex"))
			.unwrap_or_default(),
	};
	for file in ["cc-switch-model-catalog.json", "models_cache.json"] {
		let Ok(raw) = std::fs::read_to_string(codex_dir.join(file)) else {
			continue;
		};
		let Ok(value) = serde_json::from_str::<Value>(&raw) else {
			continue;
		};
		for model in value.get("models").and_then(Value::as_array).into_iter().flatten() {
			// Codex hides its retired aliases (gpt-5, gpt-5.1, the -mini
			// predecessors…): only the models it still lists are usable.
			if model.get("visibility").and_then(Value::as_str) == Some("hide") {
				continue;
			}
			let id = model.get("slug").and_then(Value::as_str);
			let context_window = model
				.get("context_window")
				.and_then(Value::as_u64)
				.or_else(|| model.get("max_context_window").and_then(Value::as_u64));
			if let Some(id) = id {
				add(id, context_window);
			}
		}
	}
	// 4. The relay's own list, for names the catalogs do not know yet.
	if let Some(key) = api_key {
		for id in ccswitch_relay_model_ids(base_url, key) {
			add(&id, None);
		}
	}
	models
}

/// Replace a provider's models array with full entries (id + optional
/// context window) so context estimates follow the Codex catalog values
/// instead of the 128k default. Every Codex catalog model reasons.
fn write_provider_models_full(id: &str, models: &[CcswitchModel]) -> Result<(), String> {
	let path = models_json_path();
	let Ok(raw) = std::fs::read_to_string(&path) else {
		return Ok(());
	};
	let mut root: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
	let entries: Vec<Value> = models
		.iter()
		.map(|model| {
			let mut entry = Map::new();
			entry.insert("id".into(), Value::String(model.id.clone()));
			if let Some(window) = model.context_window {
				entry.insert("contextWindow".into(), Value::from(window));
			}
			entry.insert("reasoning".into(), Value::Bool(true));
			Value::Object(entry)
		})
		.collect();
	if let Some(provider) = root
		.get_mut("providers")
		.and_then(|providers| providers.get_mut(id))
		.and_then(Value::as_object_mut)
	{
		provider.insert("models".into(), Value::Array(entries));
	}
	write_json_secure(&path, &root)
}

/// `GET {base_url}/models` on the OpenAI-compatible relay. Best-effort: a
/// relay that is offline or non-standard simply contributes no models.
fn ccswitch_relay_model_ids(base_url: &str, api_key: &str) -> Vec<String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(_) => return Vec::new(),
    };
    let response = match client.get(url).bearer_auth(api_key).send() {
        Ok(response) => response,
        Err(_) => return Vec::new(),
    };
    if !response.status().is_success() {
        return Vec::new();
    }
    let text = match response.text() {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    let body: Value = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(_) => return Vec::new(),
    };
    let mut ids = Vec::new();
    if let Some(entries) = body.get("data").and_then(Value::as_array) {
        for entry in entries {
            if let Some(id) = entry.get("id").and_then(Value::as_str) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

fn ccswitch_provider_id(source_id: &str, app_type: &str) -> String {
    let id = source_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect::<String>();
    format!(
        "ccswitch-{app_type}-{}",
        if id.is_empty() { "provider" } else { &id }
    )
}

/// Resolve one CCSwitch Codex relay into (api, base_url, api_key).
///
/// The Codex config TOML names a provider (`model_provider = "timi_cc"`) and
/// carries its real endpoint in `[model_providers.timi_cc]`: `base_url` plus
/// `wire_api` ("responses" or "chat"). Relays authenticated with ChatGPT
/// OAuth instead of an API key are skipped — pi cannot reuse those tokens as
/// a plain API key.
fn ccswitch_mapping(
    config: &Value,
    endpoint_url: Option<&str>,
) -> Option<(&'static str, String, Option<String>)> {
    let config_text = config.get("config").and_then(Value::as_str)?;
    let nested_auth = ccswitch_value(config.get("auth"));
    let api_key = nested_auth
        .as_ref()
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .map(str::to_string)?;

    let model_provider = ccswitch_toml_str(config_text, "model_provider")?;
    let section = ccswitch_toml_section(config_text, model_provider)?;
    let base_url = ccswitch_toml_str(section, "base_url")
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .or_else(|| {
            endpoint_url
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string)
        })?;
    let api = match ccswitch_toml_str(section, "wire_api") {
        Some("chat") => "openai-completions",
        _ => "openai-responses",
    };
    Some((api, base_url, Some(api_key)))
}

/// The body of `[model_providers.<section>]` in a Codex config TOML, up to
/// the next table header.
fn ccswitch_toml_section<'a>(raw: &'a str, section: &str) -> Option<&'a str> {
    let header = format!("[model_providers.{section}]");
    let start = raw.find(&header)? + header.len();
    let rest = &raw[start..];
    let end = rest.find("\n[").unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// `key = "value"` inside a TOML fragment (first match wins).
fn ccswitch_toml_str<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key} = \"");
    let start = raw.find(&needle)? + needle.len();
    let rest = &raw[start..];
    Some(&rest[..rest.find('"')?])
}

/// `key = 123` inside a TOML fragment (first match wins).
fn ccswitch_toml_u64(raw: &str, key: &str) -> Option<u64> {
    for line in raw.lines() {
        let Some(rest) = line.trim().strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=')?.trim_start();
        let digits = rest.split(|c: char| !c.is_ascii_digit()).next()?;
        if !digits.is_empty() {
            return digits.parse().ok();
        }
    }
    None
}

fn ccswitch_value(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Object(object)) => Some(Value::Object(object.clone())),
        Some(Value::String(raw)) => serde_json::from_str(raw).ok(),
        _ => None,
    }
}

fn write_provider_at(
    path: &Path,
    id: &str,
    name: Option<&str>,
    base_url: &str,
    api: &str,
    api_key: Option<&str>,
    model_ids: &[String],
) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(tr!("settings.provider_id_required"));
    }
    let mut root = read_root_at(path)?;
    let providers = root
        .as_object_mut()
        .expect("read_root always returns an object")
        .entry("providers")
        .or_insert_with(|| Value::Object(Map::new()));
    let providers = providers
        .as_object_mut()
        .ok_or_else(|| tr!("providers.models_json_providers_not_object"))?;

    let existing = providers.get(id).and_then(Value::as_object).cloned();
    let mut entry = existing.clone().unwrap_or_default();

    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => {
            entry.insert("name".into(), Value::String(name.to_string()));
        }
        None => {
            entry.remove("name");
        }
    }
    if base_url.trim().is_empty() {
        entry.remove("baseUrl");
    } else {
        entry.insert("baseUrl".into(), Value::String(base_url.trim().to_string()));
    }
    if api.trim().is_empty() {
        entry.remove("api");
    } else {
        entry.insert("api".into(), Value::String(api.trim().to_string()));
    }
    match api_key {
        Some(key) if !key.trim().is_empty() => {
            entry.insert("apiKey".into(), Value::String(key.trim().to_string()));
        }
        // Blank field: keep the stored key if one exists, otherwise leave it out.
        _ => {}
    }
    if !model_ids.is_empty() {
        let models: Vec<Value> = model_ids
            .iter()
            .map(|model_id| {
                // Preserve the existing model object (contextWindow, compat,
                // cost…) when only the id matches; otherwise start minimal.
                existing
                    .as_ref()
                    .and_then(|entry| entry.get("models"))
                    .and_then(Value::as_array)
                    .and_then(|models| {
                        models.iter().find(|model| {
                            model.get("id").and_then(Value::as_str) == Some(model_id.as_str())
                        })
                    })
                    .cloned()
                    .unwrap_or_else(|| {
                        let mut model = Map::new();
                        model.insert("id".into(), Value::String(model_id.clone()));
                        Value::Object(model)
                    })
            })
            .collect();
        entry.insert("models".into(), Value::Array(models));
    }

    if entry.is_empty() {
        // Nothing left to say about this provider — drop the key rather than
        // leaving an empty object behind.
        providers.remove(id);
    } else {
        providers.insert(id.to_string(), Value::Object(entry));
    }
    write_root_at(path, &root)
}

/// Remove a provider entry. Removing a built-in override restores pi's own
/// defaults; removing a custom provider drops it entirely.
pub(crate) fn remove_provider(id: &str) -> Result<(), String> {
    remove_provider_at(&models_json_path(), id)
}

fn remove_provider_at(path: &Path, id: &str) -> Result<(), String> {
    let mut root = read_root_at(path)?;
    let Some(providers) = root
        .as_object_mut()
        .and_then(|root| root.get_mut("providers"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    providers.remove(id);
    write_root_at(path, &root)
}

/// Read the whole file as a JSON object, defaulting to `{"providers": {}}`.
fn read_root_at(path: &Path) -> Result<Value, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::json!({ "providers": {} }));
        }
        Err(err) => {
            return Err(tr!(
                "errors.could_not_read",
                path = path.display().to_string(),
                error = err
            ))
        }
    };
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({ "providers": {} }));
    }
    match serde_json::from_str::<Value>(&raw) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err(tr!("providers.models_json_not_object")),
        Err(err) => Err(tr!("errors.models_json_invalid", error = err)),
    }
}

/// Pretty-print and atomically replace the file (temp + rename), so a crash
/// mid-write can never leave a truncated config behind.
fn write_root_at(path: &Path, root: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            tr!(
                "errors.could_not_create",
                path = parent.display().to_string(),
                error = err
            )
        })?;
    }
    let pretty = serde_json::to_string_pretty(root)
        .map_err(|err| tr!("errors.could_not_serialize_models_json", error = err))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{pretty}\n")).map_err(|err| {
        tr!(
            "errors.could_not_write",
            path = tmp.display().to_string(),
            error = err
        )
    })?;
    std::fs::rename(&tmp, path).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        tr!(
            "errors.could_not_replace",
            path = path.display().to_string(),
            error = err
        )
    })
}

/// The driven harness's credential store (0600). Follows the harness's own
/// agent-dir resolution (rad ships configDir `.rad`).
pub(crate) fn auth_path() -> PathBuf {
    orbit_rpc::agent_dir().join("auth.json")
}

/// `auth.json` key holding the Ollama Cloud session cookie. Deliberately not a
/// provider id: pi treats any stored credential under a provider id as
/// authoritative, so an unrecognized `ollama_cloud_session` type under `ollama`
/// would shadow that provider's own (placeholder) key and make pi report
/// `Provider is not configured: ollama`.
const OLLAMA_SESSION_KEY: &str = "ollama-cloud-session";

/// The provider ids Ollama Cloud usage attaches to: the local Ollama
/// endpoint's id, and the `ollama-cloud` id used by the third-party
/// `pi-ollama-cloud-provider` package. Orbit treats both as the same account
/// for the session editor, sign-out, and quota display.
pub(crate) fn is_ollama_cloud_provider(id: &str) -> bool {
    id == "ollama" || id == "ollama-cloud"
}

/// A provider pi ships with, independent of whether it is authenticated.
/// Mirrors `builtinProviders()` in `@earendil-works/pi-ai` plus the docs'
/// env-var / OAuth table, so Orbit can list every provider individually.
pub(crate) struct BuiltinProvider {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    /// Environment variable pi reads for an API key ("" when OAuth-only).
    pub(crate) env_var: &'static str,
    /// Storing an API key in `auth.json` works for this provider.
    pub(crate) api_key: bool,
    /// `/login <id>` offers a subscription/OAuth flow.
    pub(crate) oauth: bool,
    /// Extra setup the key alone does not cover ("" when none).
    pub(crate) note: &'static str,
}

/// Every built-in provider, in the order the pi docs list them.
pub(crate) const BUILTIN_PROVIDERS: &[BuiltinProvider] = &[
    BuiltinProvider { id: "anthropic", name: "Anthropic", env_var: "ANTHROPIC_API_KEY", api_key: true, oauth: true, note: "" },
    BuiltinProvider { id: "openai", name: "OpenAI", env_var: "OPENAI_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "openai-codex", name: "ChatGPT (Codex)", env_var: "", api_key: false, oauth: true, note: "ChatGPT Plus/Pro subscription." },
    BuiltinProvider { id: "github-copilot", name: "GitHub Copilot", env_var: "COPILOT_GITHUB_TOKEN", api_key: true, oauth: true, note: "" },
    BuiltinProvider { id: "xai", name: "xAI", env_var: "XAI_API_KEY", api_key: true, oauth: true, note: "" },
    BuiltinProvider { id: "openrouter", name: "OpenRouter", env_var: "OPENROUTER_API_KEY", api_key: true, oauth: true, note: "" },
    BuiltinProvider { id: "radius", name: "Radius", env_var: "RADIUS_API_KEY", api_key: true, oauth: true, note: "" },
    BuiltinProvider { id: "kimi-coding", name: "Kimi For Coding", env_var: "KIMI_API_KEY", api_key: true, oauth: true, note: "" },
    BuiltinProvider { id: "ant-ling", name: "Ant Ling", env_var: "ANT_LING_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "azure-openai-responses", name: "Azure OpenAI", env_var: "AZURE_OPENAI_API_KEY", api_key: true, oauth: false, note: "Also needs AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME." },
    BuiltinProvider { id: "deepseek", name: "DeepSeek", env_var: "DEEPSEEK_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "nvidia", name: "NVIDIA NIM", env_var: "NVIDIA_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "google", name: "Google Gemini", env_var: "GEMINI_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "google-vertex", name: "Google Vertex", env_var: "GOOGLE_CLOUD_API_KEY", api_key: true, oauth: false, note: "Or Application Default Credentials plus GOOGLE_CLOUD_PROJECT and GOOGLE_CLOUD_LOCATION." },
    BuiltinProvider { id: "amazon-bedrock", name: "Amazon Bedrock", env_var: "AWS_BEARER_TOKEN_BEDROCK", api_key: true, oauth: false, note: "Or ambient AWS credentials (profile, IAM keys, SSO)." },
    BuiltinProvider { id: "mistral", name: "Mistral", env_var: "MISTRAL_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "groq", name: "Groq", env_var: "GROQ_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "cerebras", name: "Cerebras", env_var: "CEREBRAS_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "cloudflare-ai-gateway", name: "Cloudflare AI Gateway", env_var: "CLOUDFLARE_API_KEY", api_key: true, oauth: false, note: "Also needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_GATEWAY_ID." },
    BuiltinProvider { id: "cloudflare-workers-ai", name: "Cloudflare Workers AI", env_var: "CLOUDFLARE_API_KEY", api_key: true, oauth: false, note: "Also needs CLOUDFLARE_ACCOUNT_ID." },
    BuiltinProvider { id: "vercel-ai-gateway", name: "Vercel AI Gateway", env_var: "AI_GATEWAY_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "zai", name: "ZAI Coding Plan (Global)", env_var: "ZAI_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "zai-coding-cn", name: "ZAI Coding Plan (China)", env_var: "ZAI_CODING_CN_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "opencode", name: "OpenCode Zen", env_var: "OPENCODE_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "opencode-go", name: "OpenCode Go", env_var: "OPENCODE_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "huggingface", name: "Hugging Face", env_var: "HF_TOKEN", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "fireworks", name: "Fireworks", env_var: "FIREWORKS_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "together", name: "Together AI", env_var: "TOGETHER_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "baseten", name: "Baseten", env_var: "BASETEN_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "minimax", name: "MiniMax", env_var: "MINIMAX_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "minimax-cn", name: "MiniMax (China)", env_var: "MINIMAX_CN_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "moonshotai", name: "Moonshot AI", env_var: "MOONSHOT_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "moonshotai-cn", name: "Moonshot AI (China)", env_var: "MOONSHOT_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "qwen-token-plan", name: "Qwen Token Plan", env_var: "QWEN_TOKEN_PLAN_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "qwen-token-plan-individual", name: "Qwen Token Plan (Individual)", env_var: "QWEN_TOKEN_PLAN_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "qwen-token-plan-cn", name: "Qwen Token Plan (China)", env_var: "QWEN_TOKEN_PLAN_CN_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "xiaomi", name: "Xiaomi MiMo", env_var: "XIAOMI_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "xiaomi-token-plan-cn", name: "Xiaomi MiMo Token Plan (China)", env_var: "XIAOMI_TOKEN_PLAN_CN_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "xiaomi-token-plan-ams", name: "Xiaomi MiMo Token Plan (Amsterdam)", env_var: "XIAOMI_TOKEN_PLAN_AMS_API_KEY", api_key: true, oauth: false, note: "" },
    BuiltinProvider { id: "xiaomi-token-plan-sgp", name: "Xiaomi MiMo Token Plan (Singapore)", env_var: "XIAOMI_TOKEN_PLAN_SGP_API_KEY", api_key: true, oauth: false, note: "" },
];

/// Look up a built-in provider by id.
pub(crate) fn builtin(id: &str) -> Option<&'static BuiltinProvider> {
    BUILTIN_PROVIDERS.iter().find(|provider| provider.id == id)
}

/// Built-in catalog size per provider, read from pi's bundled model data.
///
/// pi's CLI and RPC only expose *configured* models, so this is the only way
/// to show what an unauthenticated provider would offer. Discovery is
/// best-effort: a missing or relocated package degrades to an empty map and
/// the card simply omits the catalog count.
static BUILTIN_CATALOG_COUNTS: OnceLock<HashMap<String, usize>> = OnceLock::new();

/// Lazily read (once) the built-in catalog counts. Safe to call from any
/// thread; the first call does the file IO.
pub(crate) fn builtin_catalog_counts() -> &'static HashMap<String, usize> {
    BUILTIN_CATALOG_COUNTS.get_or_init(|| {
        pi_ai_data_dir()
            .and_then(|dir| read_catalog_counts(&dir))
            .unwrap_or_default()
    })
}

/// Locate `@earendil-works/pi-ai/dist/providers/data` by walking up the
/// install tree of the resolved `pi` binary. Works for npm/yarn/pnpm layouts
/// because the walk checks every ancestor's `node_modules`.
fn pi_ai_data_dir() -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(orbit_rpc::pi_binary()).ok()?;
    for ancestor in canonical.ancestors() {
        let candidate = ancestor
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-ai")
            .join("dist")
            .join("providers")
            .join("data");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Extra setup guidance for providers whose key alone isn't enough. Sourced
/// from the curated table; `""` when there is nothing to add.
pub(crate) fn provider_note(id: &str) -> &'static str {
    builtin(id).map(|provider| provider.note).unwrap_or("")
}

/// Translate a provider note for display. The curated table stores English
/// copy; this maps each note to its localized string.
pub(crate) fn localize_note(note: &str) -> String {
    match note {
        "" => String::new(),
        "ChatGPT Plus/Pro subscription." => tr!("providers.note_chatgpt_codex"),
        "Also needs AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME." => {
            tr!("providers.note_azure_openai")
        }
        "Or Application Default Credentials plus GOOGLE_CLOUD_PROJECT and GOOGLE_CLOUD_LOCATION." => {
            tr!("providers.note_google_vertex")
        }
        "Or ambient AWS credentials (profile, IAM keys, SSO)." => {
            tr!("providers.note_amazon_bedrock")
        }
        "Also needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_GATEWAY_ID." => {
            tr!("providers.note_cloudflare_gateway")
        }
        "Also needs CLOUDFLARE_ACCOUNT_ID." => tr!("providers.note_cloudflare_workers_ai"),
        other => other.to_string(),
    }
}

/// Provider metadata introspected from pi-ai — authoritative, so Orbit does
/// not need a hardcoded table to stay current with pi releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DynamicProvider {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) oauth: bool,
    pub(crate) api_key: bool,
    pub(crate) env_vars: Vec<String>,
}

static DYNAMIC_PROVIDERS: OnceLock<Option<Vec<DynamicProvider>>> = OnceLock::new();

/// Introspect pi's built-in providers by running a tiny node script against
/// `@earendil-works/pi-ai`. `None` when node or the package can't be found;
/// callers fall back to [`BUILTIN_PROVIDERS`]. Safe to call from any thread.
pub(crate) fn dynamic_providers() -> Option<&'static [DynamicProvider]> {
    DYNAMIC_PROVIDERS
        .get_or_init(|| {
            let data_dir = pi_ai_data_dir()?;
            // data → providers → dist → pi-ai → @earendil-works → node_modules
            // → the package whose node_modules resolves the bare specifier.
            let env_keys = data_dir.parent()?.parent()?.join("env-api-keys.js");
            let package_dir = data_dir.ancestors().nth(6)?.to_path_buf();
            let node = node_binary()?;
            run_introspection(&node, &package_dir, &env_keys)
        })
        .as_deref()
}

/// The `node` executable, from PATH then the usual install dirs (a bundled
/// `.app` launches with a minimal PATH). Shared with the RPC-patch runner.
pub(crate) fn node_binary() -> Option<String> {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    for dir in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/opt/local/bin",
        "/usr/bin",
        "/bin",
    ] {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Reads `builtinProviders()` and reports id / name / OAuth / API-key support
/// plus the env vars each provider reads. `findEnvKeys` is fed a proxy that
/// answers every name, which makes it return the full candidate list rather
/// than only the variables currently set.
const INTROSPECT_SCRIPT: &str = r#"
import { builtinProviders } from '@earendil-works/pi-ai/providers/all';
import { pathToFileURL } from 'node:url';
let findEnvKeys = () => undefined;
try {
  const mod = await import(pathToFileURL(process.env.PI_AI_ENV_KEYS).href);
  if (typeof mod.findEnvKeys === 'function') findEnvKeys = mod.findEnvKeys;
} catch {}
const anyEnv = new Proxy({}, { get: (_, k) => (typeof k === 'string' ? 'x' : undefined) });
const out = builtinProviders().map((p) => ({
  id: p.id,
  name: p.name ?? p.id,
  oauth: !!(p.auth && p.auth.oauth),
  apiKey: !!(p.auth && p.auth.apiKey),
  envVars: findEnvKeys(p.id, anyEnv) ?? [],
}));
console.log(JSON.stringify(out));
"#;

fn run_introspection(
    node: &str,
    package_dir: &Path,
    env_keys: &Path,
) -> Option<Vec<DynamicProvider>> {
    let mut command = std::process::Command::new(node);
    command
        .arg("--input-type=module")
        .arg("-e")
        .arg(INTROSPECT_SCRIPT)
        .current_dir(package_dir)
        .env("PI_AI_ENV_KEYS", env_keys);
    orbit_rpc::hide_console(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_dynamic(std::str::from_utf8(&output.stdout).ok()?)
}

/// Parse the introspection JSON into providers (empty/absent → `None`).
fn parse_dynamic(raw: &str) -> Option<Vec<DynamicProvider>> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    let items = value.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let id = item.get("id").and_then(Value::as_str)?;
        out.push(DynamicProvider {
            id: id.to_string(),
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string(),
            oauth: item.get("oauth").and_then(Value::as_bool).unwrap_or(false),
            api_key: item.get("apiKey").and_then(Value::as_bool).unwrap_or(false),
            env_vars: item
                .get("envVars")
                .and_then(Value::as_array)
                .map(|vars| {
                    vars.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    (!out.is_empty()).then_some(out)
}

/// Count models in each `<provider>.json` catalog. The files are shaped
/// `{ "<api>": { "<modelId>": {…} } }`, so the count is the sum of the inner
/// objects' lengths (a provider may speak more than one API).
fn read_catalog_counts(dir: &Path) -> Option<HashMap<String, usize>> {
    let mut out = HashMap::new();
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let count: usize = value
            .as_object()
            .map(|apis| {
                apis.values()
                    .filter_map(|api| api.as_object().map(|models| models.len()))
                    .sum()
            })
            .unwrap_or(0);
        if count > 0 {
            out.insert(id.to_string(), count);
        }
    }
    Some(out)
}

/// Credential kind stored for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthKind {
    ApiKey,
    OAuth,
    /// The Ollama Cloud settings-page cookie. Stored under a non-provider key
    /// ([`OLLAMA_SESSION_KEY`]) and surfaced as provider `ollama` for the UI.
    OllamaSession,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderAuth {
    pub(crate) kind: AuthKind,
}

/// Read `auth.json` into provider id → credential kind. A missing file is an
/// empty map; malformed JSON is surfaced so the page can warn.
pub(crate) fn read_auth() -> Result<HashMap<String, ProviderAuth>, String> {
    read_auth_at(&auth_path())
}

fn read_auth_at(path: &Path) -> Result<HashMap<String, ProviderAuth>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => {
            return Err(tr!(
                "errors.could_not_read",
                path = path.display().to_string(),
                error = err
            ))
        }
    };
    if raw.trim().is_empty() {
        return Ok(HashMap::new());
    }
    let root: Value =
        serde_json::from_str(&raw).map_err(|err| tr!("errors.auth_json_invalid", error = err))?;
    let Some(entries) = root.as_object() else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    let mut has_ollama_session = false;
    for (id, entry) in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let kind = match entry.get("type").and_then(Value::as_str) {
            Some("ollama_cloud_session") => {
                has_ollama_session = true;
                continue;
            }
            Some("oauth") => AuthKind::OAuth,
            Some("api_key") => AuthKind::ApiKey,
            // Older/partial entries: infer from the fields present.
            _ if entry.contains_key("refresh") || entry.contains_key("access") => AuthKind::OAuth,
            _ if entry.contains_key("key") => AuthKind::ApiKey,
            _ => continue,
        };
        out.insert(id.clone(), ProviderAuth { kind });
    }
    // The session cookie lives outside the provider credential; surface it
    // under both Ollama ids so whichever card exists (the local `ollama`
    // endpoint or the third-party `ollama-cloud` provider) shows it and
    // offers Disconnect. A real credential under either id wins.
    if has_ollama_session {
        for id in ["ollama", "ollama-cloud"] {
            out.entry(id.to_string()).or_insert(ProviderAuth {
                kind: AuthKind::OllamaSession,
            });
        }
    }
    Ok(out)
}

/// Store an API key for `id` in `auth.json`, preserving every other entry.
/// The file is written atomically with `0600` permissions.
pub(crate) fn write_api_key(id: &str, key: &str) -> Result<(), String> {
    write_api_key_at(&auth_path(), id, key)
}

fn write_api_key_at(path: &Path, id: &str, key: &str) -> Result<(), String> {
    let id = id.trim();
    let key = key.trim();
    if id.is_empty() {
        return Err(tr!("settings.provider_id_required"));
    }
    if key.is_empty() {
        return Err(tr!("providers.api_key_required"));
    }
    let mut root = read_auth_root_at(path)?;
    let entries = root
        .as_object_mut()
        .ok_or_else(|| tr!("providers.auth_json_not_object"))?;
    let mut entry = Map::new();
    entry.insert("type".into(), Value::String("api_key".into()));
    entry.insert("key".into(), Value::String(key.to_string()));
    entries.insert(id.to_string(), Value::Object(entry));
    write_json_secure(path, &root)
}

/// Remove a provider's stored credential (sign out).
pub(crate) fn remove_auth(id: &str) -> Result<(), String> {
    remove_auth_at(&auth_path(), id)
}

/// Store an Ollama Cloud usage session (a `Cookie:` header the user pasted
/// from their own signed-in browser) for provider id `ollama`. Written to
/// `auth.json` under [`OLLAMA_SESSION_KEY`] with `0600`; the value is only ever
/// sent to `ollama.com` by the pi-side quota handler and never logged or shown
/// again.
///
/// The entry deliberately lives *outside* the `ollama` provider credential so
/// it can never shadow the local endpoint's placeholder key in pi's auth
/// resolver. It remains a distinct type from an API key, so the handler can
/// tell the current monthly-credit API path (real key) from the legacy
/// session/weekly settings-page path (session).
pub(crate) fn write_ollama_cloud_session(session: &str) -> Result<(), String> {
    write_ollama_cloud_session_at(&auth_path(), session)
}

fn write_ollama_cloud_session_at(path: &Path, session: &str) -> Result<(), String> {
    let session = session.trim();
    if session.is_empty() {
        return Err(tr!("providers.session_cookie_required"));
    }
    // A pasted value must look like a cookie header, not an API key. Reject
    // anything without a `name=value` pair so a stray key can't be stored as
    // a session (and leak into a Cookie header).
    if !session.contains('=') {
        return Err("Paste the Cookie header (e.g. `__Secure-session=…`).".into());
    }
    let mut root = read_auth_root_at(path)?;
    let entries = root
        .as_object_mut()
        .ok_or_else(|| tr!("providers.auth_json_not_object"))?;
    // Repair a cookie written by an older build under the provider id; leaving
    // it there keeps the local endpoint broken.
    let legacy_session = entries
        .get("ollama")
        .and_then(Value::as_object)
        .is_some_and(|entry| {
            entry.get("type").and_then(Value::as_str) == Some("ollama_cloud_session")
        });
    if legacy_session {
        entries.remove("ollama");
    }
    let mut entry = Map::new();
    entry.insert("type".into(), Value::String("ollama_cloud_session".into()));
    entry.insert("session".into(), Value::String(session.to_string()));
    entries.insert(OLLAMA_SESSION_KEY.to_string(), Value::Object(entry));
    write_json_secure(path, &root)
}

/// Remove only the Ollama Cloud session cookie entry, leaving any provider
/// credential under `ollama` in place. Sign-out calls this because pi's own
/// logout only knows the provider key.
pub(crate) fn remove_ollama_session() -> Result<(), String> {
    remove_ollama_session_at(&auth_path())
}

fn remove_ollama_session_at(path: &Path) -> Result<(), String> {
    let mut root = read_auth_root_at(path)?;
    if let Some(entries) = root.as_object_mut() {
        entries.remove(OLLAMA_SESSION_KEY);
    }
    write_json_secure(path, &root)
}

fn remove_auth_at(path: &Path, id: &str) -> Result<(), String> {
    let mut root = read_auth_root_at(path)?;
    if let Some(entries) = root.as_object_mut() {
        entries.remove(id);
    }
    write_json_secure(path, &root)
}

fn read_auth_root_at(path: &Path) -> Result<Value, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::json!({}));
        }
        Err(err) => {
            return Err(tr!(
                "errors.could_not_read",
                path = path.display().to_string(),
                error = err
            ))
        }
    };
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    match serde_json::from_str::<Value>(&raw) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err(tr!("providers.auth_json_not_object")),
        Err(err) => Err(tr!("errors.auth_json_invalid", error = err)),
    }
}

/// Atomic write with owner-only permissions (matching pi's own `0600`).
fn write_json_secure(path: &Path, root: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            tr!(
                "errors.could_not_create",
                path = parent.display().to_string(),
                error = err
            )
        })?;
    }
    let pretty = serde_json::to_string_pretty(root)
        .map_err(|err| tr!("errors.could_not_serialize_auth_json", error = err))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{pretty}\n")).map_err(|err| {
        tr!(
            "errors.could_not_write",
            path = tmp.display().to_string(),
            error = err
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(|err| {
            let _ = std::fs::remove_file(&tmp);
            tr!(
                "errors.could_not_write",
                path = tmp.display().to_string(),
                error = err
            )
        })?;
    }
    std::fs::rename(&tmp, path).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        tr!(
            "errors.could_not_replace",
            path = path.display().to_string(),
            error = err
        )
    })
}

/// Human label for a provider id (`amazon-bedrock` → `Amazon Bedrock`), with
/// the handful of brands that don't title-case cleanly spelled out.
pub(crate) fn provider_display_name(id: &str) -> String {
    if let Some(provider) = builtin(id) {
        return provider.name.to_string();
    }
    let known = match id {
        "openai" => "OpenAI",
        "openai-codex" => "OpenAI Codex",
        "xai" => "xAI",
        "zai" => "Z.ai",
        "zai-coding-cn" => "Z.ai Coding (CN)",
        "moonshotai" => "Moonshot AI",
        "moonshotai-cn" => "Moonshot AI (CN)",
        "minimax" => "MiniMax",
        "minimax-cn" => "MiniMax (CN)",
        "nvidia" => "NVIDIA",
        "ollama" => "Ollama",
        "ollama-cloud" => "Ollama Cloud",
        "github-copilot" => "GitHub Copilot",
        "cloudflare-ai-gateway" => "Cloudflare AI Gateway",
        "cloudflare-workers-ai" => "Cloudflare Workers AI",
        "amazon-bedrock" => "Amazon Bedrock",
        "azure-openai-responses" => "Azure OpenAI",
        "google-vertex" => "Google Vertex",
        "vercel-ai-gateway" => "Vercel AI Gateway",
        "kimi-coding" => "Kimi Coding",
        "opencode" => "opencode",
        "opencode-go" => "opencode Go",
        "ant-ling" => "Ant Ling",
        "baseten" => "Baseten",
        "cerebras" => "Cerebras",
        "deepseek" => "DeepSeek",
        "fireworks" => "Fireworks",
        "google" => "Google",
        "groq" => "Groq",
        "huggingface" => "Hugging Face",
        "mistral" => "Mistral",
        "openrouter" => "OpenRouter",
        "together" => "Together",
        "xiaomi" => "Xiaomi",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    id.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Brand ink for a provider's mark, as `(dark, light)` hex — the same
/// per-mode shape as `mentions::file_type_badge`, so identity color reads on
/// both canvases.
///
/// `None` means the brand *is* ink, not hue — OpenAI's wordmark, Ollama,
/// opencode, xAI, Vercel, llama.cpp, and the rest — and the caller keeps its
/// neutral. Marks are monochrome SVGs (GPUI rasterizes them as a single-color
/// mask), so this is a tint, not a multi-color logo; only the identity hue
/// comes through.
pub(crate) fn provider_brand(provider: &str) -> Option<(u32, u32)> {
    // The catalog-index filename is not a real provider id; Ollama Cloud
    // shares Ollama's mark.
    let provider = match provider {
        "ollama-cloud" => "ollama",
        ".manifest" => "manifest",
        other => other,
    };
    Some(match provider {
        "amazon-bedrock" => (0xFF9900, 0xC77700),
        "ant-ling" => (0x1677FF, 0x0E5FCC),
        "anthropic" => (0xD97757, 0xB55035),
        "azure-openai-responses" => (0x0078D4, 0x005A9E),
        "baseten" => (0x16A34A, 0x128040),
        "cerebras" => (0xF15A29, 0xC7461C),
        "cloudflare-ai-gateway" | "cloudflare-workers-ai" => (0xF38020, 0xC25E00),
        "deepseek" => (0x4D6BFE, 0x3A52C9),
        "fireworks" => (0x8B5CF6, 0x6D28D9),
        "google" | "google-vertex" => (0x4285F4, 0x1A73E8),
        "groq" => (0xF55036, 0xC93A22),
        "huggingface" => (0xFFD21E, 0xA87800),
        "kimi-coding" | "moonshotai" | "moonshotai-cn" => (0x3D5AFE, 0x2E45C9),
        "minimax" | "minimax-cn" => (0xFF4D4F, 0xCC3B3D),
        "mistral" => (0xFA520F, 0xC63D00),
        "nvidia" => (0x76B900, 0x5A8C00),
        "openrouter" => (0x6467F2, 0x5053C9),
        "qwen-token-plan" | "qwen-token-plan-cn" | "qwen-token-plan-individual" => {
            (0x615CED, 0x4E49C9)
        }
        "together" => (0x0F6FFF, 0x0B57CC),
        "xiaomi" | "xiaomi-token-plan-ams" | "xiaomi-token-plan-cn" | "xiaomi-token-plan-sgp" => {
            (0xFF6900, 0xCC5400)
        }
        "zai" | "zai-coding-cn" => (0x3B5BFF, 0x2A43CC),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_toml_provider_section() {
        let config_text = "model = \"gpt-5.6-sol\"\nmodel_provider = \"timi_cc\"\nmodel_context_window = 200000\n\n[model_providers.timi_cc]\nbase_url = \"https://api.78code.cc/v1\"\nwire_api = \"responses\"\nmodel_reasoning_effort = \"high\"\n\n[features]\nmemories = false\n";
        assert_eq!(ccswitch_toml_str(config_text, "model_provider"), Some("timi_cc"));
        assert_eq!(ccswitch_toml_u64(config_text, "model_context_window"), Some(200000));
        let section = ccswitch_toml_section(config_text, "timi_cc").unwrap();
        assert_eq!(ccswitch_toml_str(section, "base_url"), Some("https://api.78code.cc/v1"));
        assert_eq!(ccswitch_toml_str(section, "wire_api"), Some("responses"));
        // The next table header terminates the section: no bleed-through.
        assert_eq!(ccswitch_toml_str(section, "memories"), None);
    }

    #[test]
    fn maps_codex_relay_from_model_provider_section() {
        let config = serde_json::json!({
            "auth": { "OPENAI_API_KEY": "sk-test", "auth_mode": "apikey" },
            "config": "model = \"gpt-5.6-sol\"\nmodel_provider = \"relay\"\n\n[model_providers.relay]\nbase_url = \"https://api.relay.test/v1\"\nwire_api = \"responses\"\n"
        });
        let (api, base_url, key) = ccswitch_mapping(&config, Some("https://vendor.homepage.test")).unwrap();
        assert_eq!(api, "openai-responses");
        // The section endpoint wins over the vendor-homepage fallback.
        assert_eq!(base_url, "https://api.relay.test/v1");
        assert_eq!(key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn skips_chatgpt_oauth_entries_without_an_api_key() {
        let config = serde_json::json!({
            "auth": { "auth_mode": "chatgpt" },
            "config": "model = \"gpt-5.6\"\n"
        });
        assert!(ccswitch_mapping(&config, None).is_none());
    }

    #[test]
    fn models_put_the_configured_model_first() {
        let dir = std::env::temp_dir().join(format!("orbit-ccswitch-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = serde_json::json!({
            "config": "model = \"gpt-5.6-sol\"\nmodel_provider = \"relay\"\nmodel_context_window = 278528\n\n[model_providers.relay]\nbase_url = \"https://api.relay.test/v1\"\nwire_api = \"responses\"\n"
        });
        let models = ccswitch_models(&config, "https://api.relay.test/v1", Some("sk-test"), Some(&dir));
        // The relay is offline and the catalog dir is empty: the configured
        // model alone remains, carrying its declared context window.
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].context_window, Some(278528));
    }

    #[test]
    fn models_merge_codex_catalogs_behind_the_configured_model() {
        let dir = std::env::temp_dir().join(format!("orbit-ccswitch-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("cc-switch-model-catalog.json"),
            r#"{"models":[{"slug":"gpt-5.6-terra","context_window":278528},{"slug":"gpt-5.6-sol","context_window":278528}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("models_cache.json"),
            r#"{"models":[{"slug":"gpt-5.3-codex","max_context_window":272000,"visibility":"list"},{"slug":"gpt-5","visibility":"hide"},{"slug":"gpt-5.6-sol","max_context_window":278528}]}"#,
        )
        .unwrap();
        let config = serde_json::json!({
            "config": "model = \"gpt-5.6-terra\"\nmodel_provider = \"relay\"\n\n[model_providers.relay]\nbase_url = \"https://api.relay.test/v1\"\nwire_api = \"responses\"\n"
        });
        let models = ccswitch_models(&config, "https://api.relay.test/v1", Some("sk-test"), Some(&dir));
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-5.6-terra", "gpt-5.6-sol", "gpt-5.3-codex"]);
        // Hidden (retired) aliases are dropped.
        assert!(!ids.contains(&"gpt-5"));
        assert_eq!(models[0].context_window, Some(278528));
        // The cache's duplicate gpt-5.6-sol did not reset the catalog window.
        assert_eq!(models[1].context_window, Some(278528));
    }

    #[test]
    fn stale_ccswitch_prefixes_are_removed_from_stores() {
        let dir = std::env::temp_dir().join(format!("orbit-ccswitch-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let models_path = dir.join("models.json");
        let auth_path = dir.join("auth.json");
        std::fs::write(&models_path, r#"{"providers":{"keep":{"baseUrl":"x"},"ccswitch-claude-old":{"baseUrl":"y"}}}"#).unwrap();
        std::fs::write(&auth_path, r#"{"keep":{"type":"api_key"},"ccswitch-claude-old":{"type":"api_key"}}"#).unwrap();
        ccswitch_remove_stale_at(&models_path, &auth_path).unwrap();
        let models: Value = serde_json::from_str(&std::fs::read_to_string(&models_path).unwrap()).unwrap();
        let auth: Value = serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert!(models["providers"].get("keep").is_some());
        assert!(models["providers"].get("ccswitch-claude-old").is_none());
        assert!(auth.get("keep").is_some());
        assert!(auth.get("ccswitch-claude-old").is_none());
    }


    fn temp_models(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("orbit-providers-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("models.json")
    }

    #[test]
    fn brand_aliases_and_skips_monochrome_brands() {
        // Aliases share the underlying brand's ink.
        assert_eq!(provider_brand("google-vertex"), provider_brand("google"));
        assert_eq!(
            provider_brand("moonshotai-cn"),
            provider_brand("moonshotai")
        );
        assert_eq!(provider_brand("ollama-cloud"), provider_brand("ollama"));
        // Distinct brands get distinct hues…
        assert!(provider_brand("deepseek").is_some());
        assert_ne!(provider_brand("deepseek"), provider_brand("nvidia"));
        // …and ink-only brands stay neutral. OpenAI's knot and Codex are
        // black/white wordmarks: the theme ink, never a tinted hue.
        assert!(provider_brand("openai").is_none());
        assert!(provider_brand("openai-codex").is_none());
        assert!(provider_brand("opencode-go").is_none());
        assert!(provider_brand("my-gateway").is_none());
    }

    #[test]
    fn humanizes_unknown_ids() {
        assert_eq!(provider_display_name("my-gateway"), "My Gateway");
        assert_eq!(provider_display_name("openai"), "OpenAI");
        assert_eq!(provider_display_name("amazon-bedrock"), "Amazon Bedrock");
        assert_eq!(
            provider_display_name("qwen-token-plan-individual"),
            "Qwen Token Plan (Individual)"
        );
    }

    #[test]
    fn discovers_pi_catalog_when_installed() {
        // Skips on machines without pi; asserts real counts where it is present.
        if pi_ai_data_dir().is_none() {
            return;
        }
        let counts = builtin_catalog_counts();
        assert!(
            counts.get("anthropic").copied().unwrap_or(0) > 0,
            "anthropic's built-in catalog should be discovered"
        );
        assert!(
            counts.len() > 20,
            "expected most providers to have catalog data, got {}",
            counts.len()
        );
    }

    #[test]
    fn parses_introspected_providers() {
        let raw = r#"[{"id":"anthropic","name":"Anthropic","oauth":true,"apiKey":true,"envVars":["ANTHROPIC_API_KEY"]},{"id":"openai-codex","name":"OpenAI Codex","oauth":true,"apiKey":false,"envVars":[]}]"#;
        let parsed = parse_dynamic(raw).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "Anthropic");
        assert!(parsed[0].oauth && parsed[0].api_key);
        assert_eq!(parsed[0].env_vars, vec!["ANTHROPIC_API_KEY"]);
        assert!(parsed[1].oauth && !parsed[1].api_key);
        assert!(parse_dynamic("[]").is_none());
        assert!(parse_dynamic("not json").is_none());
    }

    #[test]
    fn introspects_pi_providers_when_installed() {
        // Skips when node/pi-ai aren't present; asserts real metadata where they are.
        let Some(providers) = dynamic_providers() else {
            return;
        };
        assert!(providers.len() >= 30, "got {} providers", providers.len());
        let anthropic = providers.iter().find(|p| p.id == "anthropic").unwrap();
        assert!(anthropic.oauth && anthropic.api_key);
        assert!(anthropic.env_vars.iter().any(|v| v == "ANTHROPIC_API_KEY"));
        // OAuth-only provider has no API key.
        let codex = providers.iter().find(|p| p.id == "openai-codex").unwrap();
        assert!(codex.oauth && !codex.api_key);
    }

    #[test]
    fn counts_models_per_catalog_file() {
        let dir = std::env::temp_dir().join(format!("orbit-pi-ai-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("anthropic.json"),
            r#"{"anthropic-messages":{"a":{},"b":{}}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("google.json"),
            r#"{"google-generative-ai":{"x":{}},"google-vertex":{"y":{}}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("ignored.txt"), "nope").unwrap();
        let counts = read_catalog_counts(&dir).unwrap();
        assert_eq!(counts["anthropic"], 2);
        assert_eq!(counts["google"], 2);
        assert!(!counts.contains_key("ignored"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_every_builtin_with_unique_ids() {
        let mut ids: Vec<&str> = BUILTIN_PROVIDERS.iter().map(|p| p.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "builtin provider ids must be unique");
        assert!(
            count >= 40,
            "expected the full builtin catalog, got {count}"
        );
        // OAuth-only providers advertise no API key.
        let codex = builtin("openai-codex").unwrap();
        assert!(codex.oauth && !codex.api_key);
        assert!(builtin("anthropic").unwrap().oauth);
    }

    #[test]
    fn round_trips_api_keys_and_preserves_other_credentials() {
        let dir = std::env::temp_dir().join(format!("orbit-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai":{"type":"oauth","refresh":"r","access":"a","expires":1}}"#,
        )
        .unwrap();

        write_api_key_at(&path, "anthropic", "sk-ant-test").unwrap();
        let auth = read_auth_at(&path).unwrap();
        assert_eq!(auth["anthropic"].kind, AuthKind::ApiKey);
        assert_eq!(auth["openai"].kind, AuthKind::OAuth);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "auth.json must stay owner-only");
        }

        remove_auth_at(&path, "anthropic").unwrap();
        let auth = read_auth_at(&path).unwrap();
        assert!(!auth.contains_key("anthropic"));
        assert!(auth.contains_key("openai"));
    }

    #[test]
    fn rejects_empty_api_keys() {
        let dir = std::env::temp_dir().join(format!("orbit-auth-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        assert!(write_api_key_at(&path, "anthropic", "   ").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn stores_ollama_cloud_session_outside_the_provider_credential() {
        let dir = std::env::temp_dir().join(format!("orbit-ollama-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        // Keep an unrelated provider's credential intact.
        write_api_key_at(&path, "anthropic", "sk-ant-test").unwrap();

        write_ollama_cloud_session_at(&path, "__Secure-session=abc123; cf_clearance=xyz").unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let root: Value = serde_json::from_str(&raw).unwrap();
        // Never under the provider id: an unknown credential type there makes
        // pi report the local endpoint as unconfigured.
        assert!(
            root.get("ollama").is_none(),
            "session must not shadow the provider credential"
        );
        let session = root.get(OLLAMA_SESSION_KEY).expect("session entry");
        assert_eq!(session.get("type").unwrap(), "ollama_cloud_session");
        assert_eq!(
            session.get("session").unwrap(),
            "__Secure-session=abc123; cf_clearance=xyz"
        );
        // The UI surfaces it under both Ollama ids, distinct from a key.
        let auth = read_auth_at(&path).unwrap();
        assert_eq!(auth["ollama"].kind, AuthKind::OllamaSession);
        assert_eq!(auth["ollama-cloud"].kind, AuthKind::OllamaSession);
        // The unrelated key survives the write.
        assert!(root.get("anthropic").is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "auth.json must stay owner-only");
        }

        // Signing out of `ollama` clears the session without touching the key.
        write_api_key_at(&path, "ollama", "real-cloud-key").unwrap();
        remove_ollama_session_at(&path).unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(root.get(OLLAMA_SESSION_KEY).is_none());
        assert!(root.get("ollama").is_some());
    }

    #[test]
    fn a_real_ollama_key_wins_over_the_session_in_the_ui() {
        let dir = std::env::temp_dir().join(format!("orbit-ollama-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        write_api_key_at(&path, "ollama", "real-cloud-key").unwrap();
        write_ollama_cloud_session_at(&path, "__Secure-session=abc123").unwrap();
        // Both coexist; the provider credential stays the API key.
        let auth = read_auth_at(&path).unwrap();
        assert_eq!(auth["ollama"].kind, AuthKind::ApiKey);
    }

    #[test]
    fn rewrites_a_legacy_session_stored_under_the_provider_id() {
        let dir = std::env::temp_dir().join(format!("orbit-ollama-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        std::fs::write(
            &path,
            r#"{"ollama":{"type":"ollama_cloud_session","session":"__Secure-session=old"}}"#,
        )
        .unwrap();

        write_ollama_cloud_session_at(&path, "__Secure-session=new").unwrap();

        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(root.get("ollama").is_none(), "legacy entry removed");
        assert_eq!(root[OLLAMA_SESSION_KEY]["session"], "__Secure-session=new");
    }

    #[test]
    fn rejects_a_session_value_without_a_cookie_pair() {
        let dir = std::env::temp_dir().join(format!("orbit-ollama-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        // A bare API key has no `name=value` pair and must not be stored as a
        // session (it would otherwise leak into a Cookie header).
        assert!(write_ollama_cloud_session_at(&path, "abc123realkey").is_err());
        assert!(write_ollama_cloud_session_at(&path, "   ").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn round_trips_and_preserves_unrelated_config() {
        let path = temp_models("roundtrip");
        std::fs::write(&path, r#"{"other":{"keep":true},"providers":{}}"#).unwrap();

        write_provider_at(
            &path,
            "my-gateway",
            Some("My Gateway"),
            "https://api.example.com/v1",
            "openai-completions",
            Some("sk-test"),
            &["m1".to_string(), "m2".to_string()],
        )
        .unwrap();
        let list = read_custom_at(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "my-gateway");
        assert_eq!(list[0].name.as_deref(), Some("My Gateway"));
        assert_eq!(list[0].model_ids, vec!["m1", "m2"]);
        assert!(list[0].has_api_key);

        // A blank key keeps the stored secret; other keys survive.
        write_provider_at(
            &path,
            "my-gateway",
            None,
            "https://api.example.com/v2",
            "openai-completions",
            None,
            &["m1".to_string()],
        )
        .unwrap();
        let list = read_custom_at(&path).unwrap();
        assert_eq!(list[0].base_url, "https://api.example.com/v2");
        assert!(list[0].has_api_key);
        assert_eq!(list[0].model_ids, vec!["m1"]);
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["other"]["keep"], Value::Bool(true));

        remove_provider_at(&path, "my-gateway").unwrap();
        assert!(read_custom_at(&path).unwrap().is_empty());
    }

    #[test]
    fn preserves_per_model_fields_by_id() {
        let path = temp_models("per-model");
        std::fs::write(
            &path,
            r#"{"providers":{"p":{"baseUrl":"https://x/v1","models":[{"id":"m1","contextWindow":128000,"reasoning":true}]}}}"#,
        )
        .unwrap();
        write_provider_at(
            &path,
            "p",
            None,
            "https://x/v1",
            "openai-completions",
            None,
            &["m1".to_string(), "m2".to_string()],
        )
        .unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let models = root["providers"]["p"]["models"].as_array().unwrap();
        assert_eq!(models[0]["contextWindow"], Value::from(128000));
        assert_eq!(models[0]["reasoning"], Value::Bool(true));
        assert_eq!(models[1]["id"], Value::String("m2".into()));
    }

    #[test]
    fn malformed_json_is_an_error_not_a_wipe() {
        let path = temp_models("malformed");
        std::fs::write(&path, "{not json").unwrap();
        assert!(read_custom_at(&path).is_err());
        assert!(write_provider_at(
            &path,
            "x",
            None,
            "https://x/v1",
            "openai-completions",
            None,
            &["m".to_string()],
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }
}
