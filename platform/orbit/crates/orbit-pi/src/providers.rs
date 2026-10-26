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

/// `~/.pi/agent/models.json`, matching the pi CLI's own store.
pub(crate) fn models_json_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".pi")
        .join("agent")
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

/// `~/.pi/agent/auth.json`, pi's credential store (0600).
pub(crate) fn auth_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".pi")
        .join("agent")
        .join("auth.json")
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
