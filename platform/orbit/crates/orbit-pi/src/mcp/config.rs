//! Reading, validating, merging, and atomically writing Pi's `mcp.json`
//! files.
//!
//! Pi is the only consumer of these files, so the shape written here is
//! exactly Pi's documented schema — no Orbit-only fields ever land in
//! `mcp.json`. Entries Orbit does not understand are preserved verbatim when
//! a file is updated; invalid entries are skipped and reported the way Pi
//! itself reports them, so a hand-edited file never blocks the rest of the
//! page.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::{
    is_pure_reference, secret_references, McpError, McpExposure, McpOAuth, McpScope, McpServer,
    McpServerDef, McpTransport, MASK,
};

/// The merged view of Pi's global and project MCP configuration for one
/// workspace: project entries replace global entries with the same name,
/// matching [`loadMcpConfig`](https://pi.dev/docs/mcp).
pub(crate) struct McpConfigLoad {
    pub servers: Vec<McpServer>,
    /// Parse/validation errors, one per bad file or entry. Safe to display:
    /// values are scrubbed before they get here (see [`scrub_error`]).
    pub errors: Vec<String>,
    pub global_path: PathBuf,
    pub project_path: Option<PathBuf>,
    /// Pi's top-level `autoEnableCodemode`, when set. A project value
    /// overrides the global one.
    pub auto_enable_codemode: Option<bool>,
}

/// One server name as Pi accepts it: letters, digits, `_`, and `-`.
pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(tr!("mcp.error_name_required"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(tr!("mcp.error_name_charset"));
    }
    Ok(())
}

/// Validate a full definition before it is written. Returns an actionable
/// message naming the exact field.
pub(crate) fn validate_def(name: &str, def: &McpServerDef) -> Result<(), String> {
    validate_name(name)?;
    match &def.transport {
        McpTransport::Stdio { command, cwd, .. } => {
            if command.trim().is_empty() {
                return Err(tr!("mcp.error_command_required"));
            }
            if command.contains('\0') {
                return Err(tr!("mcp.error_command_invalid"));
            }
            if cwd.as_deref().is_some_and(|cwd| cwd.contains('\0')) {
                return Err(tr!("mcp.error_cwd_invalid"));
            }
        }
        McpTransport::StreamableHttp { url, .. } => {
            validate_url(url)?;
            if let Some(oauth) = &def.oauth {
                if oauth
                    .callback_port
                    .is_some_and(|port| port == 0 || port > u64::from(u16::MAX))
                {
                    return Err(tr!("mcp.error_oauth_port"));
                }
                if let Some(callback) = &oauth.callback_url {
                    validate_callback_url(callback)?;
                }
            }
        }
    }
    if def.timeout == Some(0) {
        return Err(tr!("mcp.error_timeout_positive"));
    }
    Ok(())
}

/// Pi only accepts `http`/`https` streamable-HTTP URLs; the legacy SSE
/// transport is rejected outright.
pub(crate) fn validate_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| tr!("mcp.error_url_invalid"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(tr!("mcp.error_url_scheme"));
    }
    Ok(())
}

/// OAuth redirect URIs must be loopback `http` URIs Pi can serve.
fn validate_callback_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| tr!("mcp.error_callback_invalid"))?;
    let loopback = matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if parsed.scheme() != "http" || !loopback {
        return Err(tr!("mcp.error_callback_loopback"));
    }
    Ok(())
}

/// Read the global and project configuration for `workspace`, merging project
/// entries over global ones by name.
pub(crate) fn load(home: &Path, workspace: Option<&Path>) -> McpConfigLoad {
    let global_path = McpScope::Global
        .config_path(home, workspace)
        .expect("global path always resolves");
    let project_path = McpScope::Project.config_path(home, workspace);
    let mut servers: BTreeMap<String, McpServer> = BTreeMap::new();
    let mut errors = Vec::new();

    let (global, global_errors, global_auto) = read_file(&global_path, McpScope::Global);
    errors.extend(global_errors);
    let mut auto_enable_codemode = global_auto;
    for server in global {
        servers.insert(server.name.clone(), server);
    }

    if let Some(path) = &project_path {
        let (project, project_errors, project_auto) = read_file(path, McpScope::Project);
        errors.extend(project_errors);
        // A project value overrides the global one, matching Pi.
        auto_enable_codemode = project_auto.or(auto_enable_codemode);
        for server in project {
            servers.insert(server.name.clone(), server);
        }
    }

    McpConfigLoad {
        servers: servers.into_values().collect(),
        errors,
        global_path,
        project_path,
        auto_enable_codemode,
    }
}

/// Read one config file. A missing file is not an error; a malformed file is
/// reported and ignored.
fn read_file(path: &Path, scope: McpScope) -> (Vec<McpServer>, Vec<String>, Option<bool>) {
    let mut errors = Vec::new();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), errors, None)
        }
        Err(err) => {
            errors.push(tr!(
                "mcp.error_read_file",
                path = path.display().to_string(),
                detail = err.to_string()
            ));
            return (Vec::new(), errors, None);
        }
    };
    if text.trim().is_empty() {
        return (Vec::new(), errors, None);
    }
    let doc: Value = match serde_json::from_str(&text) {
        Ok(doc) => doc,
        Err(err) => {
            errors.push(tr!(
                "mcp.error_parse_file",
                path = path.display().to_string(),
                detail = err.to_string()
            ));
            return (Vec::new(), errors, None);
        }
    };
    let Some(root) = doc.as_object() else {
        errors.push(tr!("mcp.error_shape", path = path.display().to_string()));
        return (Vec::new(), errors, None);
    };
    let auto = match root.get("autoEnableCodemode") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => {
            errors.push(tr!(
                "mcp.error_auto_codemode",
                path = path.display().to_string()
            ));
            None
        }
    };
    let Some(entries) = root.get("mcpServers") else {
        return (Vec::new(), errors, auto);
    };
    let Some(entries) = entries.as_object() else {
        errors.push(tr!(
            "mcp.error_servers_shape",
            path = path.display().to_string()
        ));
        return (Vec::new(), errors, auto);
    };
    let mut servers = Vec::new();
    for (name, value) in entries {
        match parse_def(value) {
            Ok(def) => servers.push(McpServer {
                name: name.clone(),
                scope,
                source: path.to_path_buf(),
                description: None,
                def,
            }),
            Err(detail) => errors.push(tr!(
                "mcp.error_entry",
                path = path.display().to_string(),
                name = name,
                detail = detail
            )),
        }
    }
    (servers, errors, auto)
}

/// Parse one `mcpServers` entry into the domain model. Unknown keys are
/// ignored here (and preserved on write); a wrong shape in a known key is an
/// error, exactly as Pi reports it.
pub(crate) fn parse_def(value: &Value) -> Result<McpServerDef, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| tr!("mcp.error_entry_object"))?;
    let explicit_type = match obj.get("type") {
        None => None,
        Some(Value::String(kind)) => Some(kind.as_str()),
        Some(_) => return Err(tr!("mcp.error_type_string")),
    };
    if explicit_type == Some("sse") {
        return Err(tr!("mcp.error_sse_unsupported"));
    }
    let enabled = match obj.get("enabled") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(tr!("mcp.error_enabled_bool")),
    };
    let exposure = match obj.get("exposure") {
        None => McpExposure::Codemode,
        Some(Value::String(mode)) => McpExposure::parse(mode)
            .ok_or_else(|| tr!("mcp.error_exposure_invalid", mode = mode.clone()))?,
        Some(_) => return Err(tr!("mcp.error_exposure_string")),
    };
    let tool_exposure = match obj.get("toolExposure") {
        None => BTreeMap::new(),
        Some(Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (tool, value) in map {
                let Some(mode) = value.as_str() else {
                    return Err(tr!("mcp.error_tool_exposure_string", tool = tool.clone()));
                };
                let mode = McpExposure::parse(mode).ok_or_else(|| {
                    tr!(
                        "mcp.error_tool_exposure_invalid",
                        tool = tool.clone(),
                        mode = mode.to_string()
                    )
                })?;
                out.insert(tool.clone(), mode);
            }
            out
        }
        Some(_) => return Err(tr!("mcp.error_tool_exposure_object")),
    };
    let timeout = match obj.get("timeout") {
        None => None,
        Some(Value::Number(number)) => {
            let seconds = number.as_u64().filter(|seconds| *seconds > 0);
            seconds.ok_or_else(|| tr!("mcp.error_timeout_positive"))?;
            number.as_u64()
        }
        Some(_) => return Err(tr!("mcp.error_timeout_number")),
    };

    let transport = if let Some(url) = obj.get("url") {
        if explicit_type.is_some_and(|kind| !matches!(kind, "http" | "streamable-http")) {
            return Err(tr!("mcp.error_type_mismatch_url"));
        }
        let url = url
            .as_str()
            .ok_or_else(|| tr!("mcp.error_url_string"))?
            .to_string();
        validate_url(&url)?;
        let headers = string_map(obj.get("headers"), "headers")?;
        McpTransport::StreamableHttp { url, headers }
    } else if let Some(command) = obj.get("command") {
        if explicit_type.is_some_and(|kind| kind != "stdio") {
            return Err(tr!("mcp.error_type_mismatch_command"));
        }
        let command = command
            .as_str()
            .ok_or_else(|| tr!("mcp.error_command_string"))?
            .to_string();
        let args = match obj.get("args") {
            None => Vec::new(),
            Some(Value::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let Some(arg) = item.as_str() else {
                        return Err(tr!("mcp.error_args_strings"));
                    };
                    out.push(arg.to_string());
                }
                out
            }
            Some(_) => return Err(tr!("mcp.error_args_array")),
        };
        let env = string_map(obj.get("env"), "env")?;
        let cwd = match obj.get("cwd") {
            None => None,
            Some(Value::String(dir)) => Some(dir.clone()),
            Some(_) => return Err(tr!("mcp.error_cwd_string")),
        };
        McpTransport::Stdio {
            command,
            args,
            env,
            cwd,
        }
    } else {
        return Err(tr!("mcp.error_needs_command_or_url"));
    };

    let oauth = match obj.get("oauth") {
        None => None,
        Some(value) => Some(parse_oauth(value)?),
    };

    Ok(McpServerDef {
        transport,
        exposure,
        tool_exposure,
        enabled,
        timeout,
        oauth,
    })
}

fn parse_oauth(value: &Value) -> Result<McpOAuth, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| tr!("mcp.error_oauth_object"))?;
    let string_field = |key: &str| -> Result<Option<String>, String> {
        match obj.get(key) {
            None => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(tr!("mcp.error_oauth_field_string", field = key)),
        }
    };
    let callback_port = match obj.get("callbackPort") {
        None => None,
        Some(Value::Number(number)) => {
            let port = number
                .as_u64()
                .filter(|port| *port > 0 && *port <= u64::from(u16::MAX))
                .ok_or_else(|| tr!("mcp.error_oauth_port"))?;
            Some(port)
        }
        Some(_) => return Err(tr!("mcp.error_oauth_port")),
    };
    let oauth = McpOAuth {
        client_id: string_field("clientId")?,
        client_secret: string_field("clientSecret")?,
        callback_port,
        callback_url: string_field("callbackUrl")?,
        scope: string_field("scope")?,
    };
    if let Some(url) = &oauth.callback_url {
        validate_callback_url(url)?;
    }
    Ok(oauth)
}

/// Read a `Record<string, string>` field (`env` / `headers`).
fn string_map(value: Option<&Value>, field: &str) -> Result<BTreeMap<String, String>, String> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (key, value) in map {
                let Some(text) = value.as_str() else {
                    return Err(tr!(
                        "mcp.error_string_map_value",
                        field = field.to_string(),
                        key = key.clone()
                    ));
                };
                out.insert(key.clone(), text.to_string());
            }
            Ok(out)
        }
        Some(_) => Err(tr!("mcp.error_string_map", field = field.to_string())),
    }
}

// ── writing ────────────────────────────────────────────────────────────────

/// One surgical change to an existing entry, mirroring what Pi's own
/// `/mcp` manager writes (defaults are removed, not spelled out).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpPatch {
    Enabled(bool),
}

/// Add or replace one server in `path`, creating the file (and its parent)
/// when missing. Returns `true` when an existing entry was replaced.
pub(crate) fn write_add(path: &Path, name: &str, def: &McpServerDef) -> Result<bool, McpError> {
    validate_def(name, def).map_err(McpError::InvalidConfiguration)?;
    let mut replaced = false;
    edit_file(path, |root| {
        let servers = root
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Map::new()));
        let servers = servers
            .as_object_mut()
            .ok_or_else(|| sync_error(path, "mcpServers is not an object"))?;
        if servers.contains_key(name) {
            replaced = true;
        }
        let entry = servers
            .entry(name.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let entry = entry
            .as_object_mut()
            .ok_or_else(|| sync_error(path, "the server entry is not an object"))?;
        apply_def(entry, def);
        Ok(())
    })?;
    Ok(replaced)
}

/// Remove one server. `Ok(false)` when the file does not define it.
pub(crate) fn write_remove(path: &Path, name: &str) -> Result<bool, McpError> {
    if !path.exists() {
        return Ok(false);
    }
    let mut removed = false;
    edit_file(path, |root| {
        if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
            removed = servers.remove(name).is_some();
        }
        Ok(())
    })?;
    Ok(removed)
}

/// Apply one `enabled` / `exposure` change in place.
pub(crate) fn write_patch(path: &Path, name: &str, patch: McpPatch) -> Result<(), McpError> {
    edit_file(path, |root| {
        let entry = root
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .and_then(|servers| servers.get_mut(name))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| McpError::ServerNotFound(name.to_string()))?;
        match patch {
            // Pi's convention: the default (`true`) is removed.
            McpPatch::Enabled(true) => {
                entry.remove("enabled");
            }
            McpPatch::Enabled(false) => {
                entry.insert("enabled".into(), Value::Bool(false));
            }
        }
        Ok(())
    })
}

/// Write `def` into an existing JSON object, preserving unknown fields and
/// dropping Pi fields that no longer apply to the transport.
fn apply_def(entry: &mut Map<String, Value>, def: &McpServerDef) {
    match &def.transport {
        McpTransport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            entry.insert("command".into(), Value::String(command.clone()));
            if args.is_empty() {
                entry.remove("args");
            } else {
                entry.insert(
                    "args".into(),
                    Value::Array(args.iter().cloned().map(Value::String).collect()),
                );
            }
            if env.is_empty() {
                entry.remove("env");
            } else {
                entry.insert("env".into(), string_map_value(env));
            }
            match cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
                Some(cwd) => {
                    entry.insert("cwd".into(), Value::String(cwd.to_string()));
                }
                None => {
                    entry.remove("cwd");
                }
            }
            entry.remove("url");
            entry.remove("headers");
            entry.remove("oauth");
            if entry.get("type").and_then(Value::as_str) != Some("stdio") {
                entry.remove("type");
            }
        }
        McpTransport::StreamableHttp { url, headers } => {
            entry.insert("url".into(), Value::String(url.clone()));
            if headers.is_empty() {
                entry.remove("headers");
            } else {
                entry.insert("headers".into(), string_map_value(headers));
            }
            match &def.oauth {
                Some(oauth) if !oauth.is_empty() => {
                    entry.insert("oauth".into(), oauth_value(oauth));
                }
                _ => {
                    entry.remove("oauth");
                }
            }
            entry.remove("command");
            entry.remove("args");
            entry.remove("env");
            entry.remove("cwd");
            match entry.get("type").and_then(Value::as_str) {
                Some("http" | "streamable-http") => {}
                _ => {
                    entry.remove("type");
                }
            }
        }
    }
    if def.exposure.is_default() {
        entry.remove("exposure");
    } else {
        entry.insert(
            "exposure".into(),
            Value::String(def.exposure.as_str().into()),
        );
    }
    if def.tool_exposure.is_empty() {
        entry.remove("toolExposure");
    } else {
        let mut map = Map::new();
        for (tool, mode) in &def.tool_exposure {
            map.insert(tool.clone(), Value::String(mode.as_str().into()));
        }
        entry.insert("toolExposure".into(), Value::Object(map));
    }
    match def.timeout {
        Some(seconds) => {
            entry.insert("timeout".into(), Value::Number(seconds.into()));
        }
        None => {
            entry.remove("timeout");
        }
    }
    if def.enabled {
        entry.remove("enabled");
    } else {
        entry.insert("enabled".into(), Value::Bool(false));
    }
}

fn string_map_value(map: &BTreeMap<String, String>) -> Value {
    let mut out = Map::new();
    for (key, value) in map {
        out.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(out)
}

fn oauth_value(oauth: &McpOAuth) -> Value {
    let mut map = Map::new();
    if let Some(value) = &oauth.client_id {
        map.insert("clientId".into(), Value::String(value.clone()));
    }
    if let Some(value) = &oauth.client_secret {
        map.insert("clientSecret".into(), Value::String(value.clone()));
    }
    if let Some(value) = oauth.callback_port {
        map.insert("callbackPort".into(), Value::Number(value.into()));
    }
    if let Some(value) = &oauth.callback_url {
        map.insert("callbackUrl".into(), Value::String(value.clone()));
    }
    if let Some(value) = &oauth.scope {
        map.insert("scope".into(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// Read → mutate → atomically rewrite one file, preserving unrelated content
/// and the file's detected indentation. A malformed file is never partially
/// overwritten: the parse error is returned and the file is left untouched.
fn edit_file(
    path: &Path,
    edit: impl FnOnce(&mut Map<String, Value>) -> Result<(), McpError>,
) -> Result<(), McpError> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(McpError::Io(tr!(
                "mcp.error_read_file",
                path = path.display().to_string(),
                detail = err.to_string()
            )));
        }
    };
    let mut doc: Value = if existing.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&existing).map_err(|err| {
            McpError::ConfigurationSyncFailed(tr!(
                "mcp.error_parse_file",
                path = path.display().to_string(),
                detail = err.to_string()
            ))
        })?
    };
    let root = doc.as_object_mut().ok_or_else(|| {
        McpError::ConfigurationSyncFailed(tr!("mcp.error_shape", path = path.display().to_string()))
    })?;
    if root
        .get("mcpServers")
        .is_some_and(|servers| !servers.is_object())
    {
        return Err(McpError::ConfigurationSyncFailed(tr!(
            "mcp.error_servers_shape",
            path = path.display().to_string()
        )));
    }
    edit(root)?;
    let indent = detect_indent(&existing);
    let text = serialize(&doc, &indent)?;
    write_atomic(path, &text)
}

fn sync_error(path: &Path, detail: &str) -> McpError {
    McpError::ConfigurationSyncFailed(tr!(
        "mcp.error_sync_detail",
        path = path.display().to_string(),
        detail = detail.to_string()
    ))
}

/// Serialize with the file's own indentation (Pi does the same), so a
/// hand-formatted file is not reformatted just because Orbit touched one
/// entry.
pub(crate) fn serialize(doc: &Value, indent: &[u8]) -> Result<String, McpError> {
    let mut bytes = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    serde::Serialize::serialize(doc, &mut serializer)
        .map_err(|err| McpError::Io(err.to_string()))?;
    bytes.push(b'\n');
    String::from_utf8(bytes).map_err(|err| McpError::Io(err.to_string()))
}

/// The indentation of the first indented line: tabs win as-is, spaces are
/// copied verbatim. Defaults to two spaces.
fn detect_indent(text: &str) -> Vec<u8> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.len() == line.len() {
            continue;
        }
        let indent = &line[..line.len() - trimmed.len()];
        if indent.chars().all(|c| c == ' ' || c == '\t') {
            return indent.as_bytes().to_vec();
        }
    }
    b"  ".to_vec()
}

/// Write `contents` to `path` through a same-directory temp file so a crash
/// can never leave a half-written config. On Windows (where rename does not
/// replace an existing file) the destination is parked as `.bak` for the
/// swap and restored if the rename fails.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), McpError> {
    write_atomic_with_mode(path, contents, None)
}

/// [`write_atomic`] for files that hold credentials: the temp file is made
/// owner-only *before* it is renamed into place, so a secret is never briefly
/// readable under the process umask while the write is in flight (the same
/// ordering as `providers::write_json_secure`).
pub(crate) fn write_atomic_secure(path: &Path, contents: &str) -> Result<(), McpError> {
    write_atomic_with_mode(path, contents, Some(0o600))
}

fn write_atomic_with_mode(path: &Path, contents: &str, mode: Option<u32>) -> Result<(), McpError> {
    /// Distinguishes concurrent writers inside one process (tests run in
    /// parallel); the name is still scoped to the target file's directory.
    static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|err| McpError::Io(err.to_string()))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mcp.json".into());
    let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{file_name}.orbit-{}-{seq}.tmp",
        std::process::id()
    ));
    fs::write(&temp, contents).map_err(|err| {
        let _ = fs::remove_file(&temp);
        McpError::Io(err.to_string())
    })?;
    #[cfg(unix)]
    {
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(mode)).map_err(|err| {
                let _ = fs::remove_file(&temp);
                McpError::Io(err.to_string())
            })?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }

    #[cfg(windows)]
    {
        let backup = parent.join(format!("{file_name}.bak"));
        let had_destination = path.exists();
        if had_destination {
            let _ = fs::remove_file(&backup);
            fs::rename(path, &backup).map_err(|err| {
                let _ = fs::remove_file(&temp);
                McpError::Io(err.to_string())
            })?;
        }
        match fs::rename(&temp, path) {
            Ok(()) => {
                if had_destination {
                    let _ = fs::remove_file(&backup);
                }
                Ok(())
            }
            Err(err) => {
                if had_destination {
                    let _ = fs::rename(&backup, path);
                }
                let _ = fs::remove_file(&temp);
                Err(McpError::Io(err.to_string()))
            }
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(&temp, path).map_err(|err| {
            let _ = fs::remove_file(&temp);
            McpError::Io(err.to_string())
        })
    }
}

/// Hash of both config files' raw bytes — the fingerprint stamped onto every
/// spawned Pi process so Orbit can tell whether that process is running the
/// configuration currently on disk.
pub(crate) fn fingerprint(home: &Path, workspace: Option<&Path>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in [
        McpScope::Global.config_path(home, workspace),
        McpScope::Project.config_path(home, workspace),
    ]
    .into_iter()
    .flatten()
    {
        path.hash(&mut hasher);
        match fs::read(&path) {
            Ok(bytes) => bytes.hash(&mut hasher),
            Err(_) => 0u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

// ── secret-bearing field helpers ───────────────────────────────────────────

/// One secret the user typed that should move into Orbit's secret store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SecretUpdate {
    pub name: String,
    pub value: String,
}

/// The outcome of resolving one secret-bearing field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SecretResolution {
    /// What gets written to `mcp.json` (literal, `${REF}`, or `!command`).
    pub value: String,
    /// A new secret to store, when a literal was entered.
    pub store: Option<SecretUpdate>,
}

/// Resolve one env/header value typed in the form:
///
/// - an untouched [`MASK`] keeps the original value (a literal original is
///   moved into the secret store and referenced instead);
/// - a `${NAME}` reference, a wrapper containing one (`Bearer ${NAME}`), or a
///   `!command` is written as typed;
/// - any other non-empty value is stored under `generated_name` and written
///   as `${generated_name}`.
pub(crate) fn resolve_typed_secret(
    typed: &str,
    original: Option<&str>,
    generated_name: &str,
) -> SecretResolution {
    if typed.is_empty() {
        return SecretResolution {
            value: String::new(),
            store: None,
        };
    }
    if typed == MASK {
        return match original {
            Some(original) => resolve_typed_secret(original, None, generated_name),
            None => SecretResolution {
                value: String::new(),
                store: None,
            },
        };
    }
    if is_pure_reference(typed) || typed.contains("${") || typed.starts_with('!') {
        return SecretResolution {
            value: typed.to_string(),
            store: None,
        };
    }
    SecretResolution {
        value: format!("${{{generated_name}}}"),
        store: Some(SecretUpdate {
            name: generated_name.to_string(),
            value: typed.to_string(),
        }),
    }
}

/// How one existing value is rendered in a form field. A reference or
/// `!command` is shown as-is; a literal is masked. Empty stays empty.
pub(crate) fn display_secret_value(value: &str) -> Cow<'_, str> {
    if value.is_empty() || value.starts_with('!') || value.contains("${") {
        Cow::Borrowed(value)
    } else {
        Cow::Borrowed(MASK)
    }
}

/// The prefix Orbit-generated secret names use for one scope/workspace,
/// including the `_` that joins it to the sanitized server name. Global
/// secrets share one namespace; project secrets embed a stable hash of the
/// workspace path, so two projects — or a project and the global scope — with
/// the same server and field can never share one store entry.
pub(crate) fn secret_namespace(scope: McpScope, workspace: Option<&Path>) -> String {
    match scope {
        McpScope::Global => "MCP_GLOBAL_".to_string(),
        McpScope::Project => {
            let path = workspace
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            format!("MCP_P{:016X}_", stable_hash(&path))
        }
    }
}

/// The secret-store name generated for one server field. Namespaced by scope
/// and workspace and suffixed with the sanitized server and field, so two
/// servers using `TOKEN` — even in different projects — cannot collide.
pub(crate) fn generated_secret_name(
    server: &str,
    field: &str,
    scope: McpScope,
    workspace: Option<&Path>,
) -> String {
    let mut name = secret_namespace(scope, workspace);
    for ch in server.chars() {
        name.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
    }
    name.push('_');
    for ch in field.chars() {
        name.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
    }
    name.to_ascii_uppercase()
}

/// FNV-1a over the string's bytes. Stable across builds (unlike
/// `DefaultHasher`), because the hash is embedded in persisted `${NAME}`
/// references and a toolchain upgrade must not orphan them.
fn stable_hash(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Parse `KEY=value` lines. Blank lines and `#` comments are skipped; a line
/// without `=` is an error naming the line.
pub(crate) fn parse_env_lines(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(tr!("mcp.error_env_line", line = index + 1));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(tr!("mcp.error_env_line", line = index + 1));
        }
        out.insert(key.to_string(), value.trim().to_string());
    }
    Ok(out)
}

/// Render env pairs back to `KEY=value` lines.
pub(crate) fn env_lines(map: &BTreeMap<String, String>) -> String {
    map.iter()
        .map(|(key, value)| format!("{key}={}", display_secret_value(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse `Name: value` lines (HTTP headers). Blank lines and `#` comments
/// are skipped. A value may contain `:`; only the first one splits.
pub(crate) fn parse_header_lines(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(tr!("mcp.error_header_line", line = index + 1));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(tr!("mcp.error_header_line", line = index + 1));
        }
        out.insert(key.to_string(), value.trim().to_string());
    }
    Ok(out)
}

/// Render header pairs back to `Name: value` lines.
pub(crate) fn header_lines(map: &BTreeMap<String, String>) -> String {
    map.iter()
        .map(|(name, value)| format!("{name}: {}", display_secret_value(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `${NAME}` reference in a definition's env/headers — the set secret
/// pruning considers when a server is removed.
pub(crate) fn definition_references(def: &McpServerDef) -> Vec<String> {
    let mut out = Vec::new();
    for value in def.transport.secret_values().values() {
        out.extend(secret_references(value));
    }
    if let Some(oauth) = &def.oauth {
        if let Some(secret) = &oauth.client_secret {
            out.extend(secret_references(secret));
        }
    }
    out
}

/// Scrub a Pi-reported error before it reaches the UI or logs. Known secret
/// values are masked; `Bearer`/`Authorization` credentials are masked even
/// when Orbit never held the value.
pub(crate) fn scrub_error(text: &str, known_secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in known_secrets.iter().filter(|secret| secret.len() >= 4) {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), MASK);
        }
    }
    out = scrub_bearer(&out);
    out
}

fn scrub_bearer(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("Bearer ") {
        out.push_str(&rest[..pos]);
        out.push_str("Bearer ");
        let after = &rest[pos + "Bearer ".len()..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
            .unwrap_or(after.len());
        if end >= 8 {
            out.push_str(MASK);
        } else {
            out.push_str(&after[..end]);
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "orbit-mcp-config-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn home(dir: &Path) -> PathBuf {
        dir.join("home")
    }

    fn stdio_def(command: &str) -> McpServerDef {
        McpServerDef {
            transport: McpTransport::Stdio {
                command: command.into(),
                args: vec!["-y".into(), "server".into()],
                env: BTreeMap::new(),
                cwd: None,
            },
            ..McpServerDef::default()
        }
    }

    #[test]
    fn parses_a_stdio_server() {
        let value = serde_json::json!({
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
            "env": {"TOKEN": "${TOKEN}"},
            "cwd": "~/work",
            "exposure": "direct",
            "timeout": 30
        });
        let def = parse_def(&value).unwrap();
        match &def.transport {
            McpTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
                assert_eq!(env["TOKEN"], "${TOKEN}");
                assert_eq!(cwd.as_deref(), Some("~/work"));
            }
            other => panic!("expected stdio, got {other:?}"),
        }
        assert_eq!(def.exposure, McpExposure::Direct);
        assert_eq!(def.timeout, Some(30));
        assert!(def.enabled);
    }

    #[test]
    fn parses_a_streamable_http_server() {
        let value = serde_json::json!({
            "type": "streamable-http",
            "url": "https://example.com/mcp",
            "headers": {"Authorization": "Bearer ${TOKEN}"},
            "enabled": false
        });
        let def = parse_def(&value).unwrap();
        match &def.transport {
            McpTransport::StreamableHttp { url, headers } => {
                assert_eq!(url, "https://example.com/mcp");
                assert_eq!(headers["Authorization"], "Bearer ${TOKEN}");
            }
            other => panic!("expected http, got {other:?}"),
        }
        assert!(!def.enabled);
        assert_eq!(def.exposure, McpExposure::Codemode);
    }

    #[test]
    fn parses_and_round_trips_oauth_settings() {
        let value = serde_json::json!({
            "url": "https://mcp.example.com/mcp",
            "oauth": {
                "clientId": "my-client",
                "clientSecret": "${EXAMPLE_SECRET}",
                "callbackPort": 8765,
                "callbackUrl": "http://localhost:8080/oauth/callback",
                "scope": "read write"
            }
        });
        let def = parse_def(&value).unwrap();
        let oauth = def.oauth.as_ref().unwrap();
        assert_eq!(oauth.client_id.as_deref(), Some("my-client"));
        assert_eq!(oauth.client_secret.as_deref(), Some("${EXAMPLE_SECRET}"));
        assert_eq!(oauth.callback_port, Some(8765));
        assert_eq!(
            oauth.callback_url.as_deref(),
            Some("http://localhost:8080/oauth/callback")
        );
        assert_eq!(oauth.scope.as_deref(), Some("read write"));

        let dir = temp_dir("oauth-roundtrip");
        let path = dir.join("mcp.json");
        write_add(&path, "example", &def).unwrap();
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parse_def(&doc["mcpServers"]["example"]).unwrap(), def);
    }

    #[test]
    fn rejects_invalid_oauth_shapes() {
        assert!(parse_def(&serde_json::json!({
            "url": "https://example.com/mcp",
            "oauth": "yes"
        }))
        .is_err());
        assert!(parse_def(&serde_json::json!({
            "url": "https://example.com/mcp",
            "oauth": { "callbackPort": 0 }
        }))
        .is_err());
        assert!(parse_def(&serde_json::json!({
            "url": "https://example.com/mcp",
            "oauth": { "callbackPort": 70000 }
        }))
        .is_err());
        // A non-loopback callback URI is rejected the way Pi rejects it.
        let bad_callback = McpServerDef {
            transport: McpTransport::StreamableHttp {
                url: "https://example.com/mcp".into(),
                headers: BTreeMap::new(),
            },
            oauth: Some(McpOAuth {
                callback_url: Some("https://evil.example.com/callback".into()),
                ..McpOAuth::default()
            }),
            ..McpServerDef::default()
        };
        assert!(validate_def("example", &bad_callback).is_err());
        // A zero/oversized port is rejected on write too.
        let bad_port = McpServerDef {
            oauth: Some(McpOAuth {
                callback_port: Some(0),
                ..McpOAuth::default()
            }),
            ..bad_callback.clone()
        };
        assert!(validate_def("example", &bad_port).is_err());
    }

    #[test]
    fn rejects_pis_rejected_shapes() {
        assert!(parse_def(&serde_json::json!({"type": "sse", "url": "http://x/sse"})).is_err());
        assert!(parse_def(&serde_json::json!({"url": "ftp://x"})).is_err());
        assert!(parse_def(&serde_json::json!({"exposure": "sometimes"})).is_err());
        assert!(parse_def(&serde_json::json!({"command": "npx", "args": "oops"})).is_err());
        assert!(parse_def(&serde_json::json!({"timeout": 0, "command": "x"})).is_err());
        assert!(parse_def(&serde_json::json!({"command": "x", "type": "http"})).is_err());
        assert!(parse_def(&serde_json::json!({})).is_err());
    }

    #[test]
    fn round_trips_a_definition_through_serialization() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("mcp.json");
        let mut def = stdio_def("npx");
        def.transport.secret_values_mut().insert(
            "GITHUB_TOKEN".into(),
            "${MCP_GLOBAL_GITHUB_GITHUB_TOKEN}".into(),
        );
        def.exposure = McpExposure::Deferred;
        def.timeout = Some(45);
        write_add(&path, "github", &def).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        let parsed = parse_def(&doc["mcpServers"]["github"]).unwrap();
        assert_eq!(parsed, def);
        // Defaults are omitted, not spelled out.
        assert!(doc["mcpServers"]["github"].get("enabled").is_none());
    }

    #[test]
    fn add_replaces_only_the_named_entry_and_preserves_the_rest() {
        let dir = temp_dir("preserve");
        let path = dir.join("mcp.json");
        fs::write(
            &path,
            r#"{
  "autoEnableCodemode": false,
  "mcpServers": {
    "keep": { "command": "keep-server", "args": ["--x"] },
    "other": { "url": "https://example.com/mcp" }
  },
  "unknownTopLevel": 7
}
"#,
        )
        .unwrap();
        let replaced = write_add(&path, "other", &stdio_def("replacement")).unwrap();
        assert!(replaced);
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["autoEnableCodemode"], false);
        assert_eq!(doc["unknownTopLevel"], 7);
        assert_eq!(doc["mcpServers"]["keep"]["command"], "keep-server");
        assert_eq!(doc["mcpServers"]["other"]["command"], "replacement");
        assert!(doc["mcpServers"]["other"].get("url").is_none());
    }

    #[test]
    fn patch_enabled_removes_the_default() {
        let dir = temp_dir("patch");
        let path = dir.join("mcp.json");
        write_add(&path, "github", &stdio_def("npx")).unwrap();
        write_patch(&path, "github", McpPatch::Enabled(false)).unwrap();
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["github"]["enabled"], false);

        write_patch(&path, "github", McpPatch::Enabled(true)).unwrap();
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(doc["mcpServers"]["github"].get("enabled").is_none());
    }

    #[test]
    fn remove_is_surgical() {
        let dir = temp_dir("remove");
        let path = dir.join("mcp.json");
        write_add(&path, "a", &stdio_def("a")).unwrap();
        write_add(&path, "b", &stdio_def("b")).unwrap();
        assert!(write_remove(&path, "a").unwrap());
        assert!(!write_remove(&path, "a").unwrap());
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(doc["mcpServers"].get("a").is_none());
        assert!(doc["mcpServers"].get("b").is_some());
    }

    #[test]
    fn malformed_file_is_never_partially_overwritten() {
        let dir = temp_dir("malformed");
        let path = dir.join("mcp.json");
        let original = "{ not json";
        fs::write(&path, original).unwrap();
        let err = write_add(&path, "github", &stdio_def("npx")).unwrap_err();
        assert!(matches!(err, McpError::ConfigurationSyncFailed(_)));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn merge_lets_project_override_global() {
        let dir = temp_dir("merge");
        let home = home(&dir);
        let workspace = dir.join("project");
        fs::create_dir_all(workspace.join(".pi")).unwrap();
        let global = McpScope::Global.config_path(&home, None).unwrap();
        write_add(&global, "shared", &stdio_def("global-cmd")).unwrap();
        write_add(&global, "only-global", &stdio_def("global-only")).unwrap();
        let project = McpScope::Project
            .config_path(&home, Some(&workspace))
            .unwrap();
        write_add(&project, "shared", &stdio_def("project-cmd")).unwrap();

        let load = load(&home, Some(&workspace));
        assert!(load.errors.is_empty(), "{:?}", load.errors);
        let shared = load
            .servers
            .iter()
            .find(|server| server.name == "shared")
            .unwrap();
        assert_eq!(shared.scope, McpScope::Project);
        match &shared.def.transport {
            McpTransport::Stdio { command, .. } => assert_eq!(command, "project-cmd"),
            other => panic!("expected stdio, got {other:?}"),
        }
        assert!(load
            .servers
            .iter()
            .any(|server| server.name == "only-global"));
    }

    #[test]
    fn invalid_entries_are_skipped_not_fatal() {
        let dir = temp_dir("invalid-entry");
        let home = home(&dir);
        let path = McpScope::Global.config_path(&home, None).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"mcpServers": {"good": {"command": "ok"}, "bad": {"nope": true}}}"#,
        )
        .unwrap();
        let load = load(&home, None);
        assert_eq!(load.servers.len(), 1);
        assert_eq!(load.servers[0].name, "good");
        assert_eq!(load.errors.len(), 1);
        assert!(load.errors[0].contains("bad"));
    }

    #[test]
    fn indentation_is_preserved_on_write() {
        let dir = temp_dir("indent");
        let path = dir.join("mcp.json");
        fs::write(
            &path,
            "{\n\t\"mcpServers\": {\n\t\t\"keep\": {\n\t\t\t\"command\": \"x\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();
        write_add(&path, "new", &stdio_def("y")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\t\"keep\""), "{text}");
        assert!(text.contains("\t\t\"new\""), "{text}");
    }

    #[test]
    fn env_and_header_lines_round_trip() {
        let mut env = BTreeMap::new();
        env.insert("TOKEN".into(), "${MCP_X_TOKEN}".into());
        env.insert("PLAIN".into(), "value".into());
        let text = env_lines(&env);
        assert_eq!(text, "PLAIN=••••••••\nTOKEN=${MCP_X_TOKEN}");
        let parsed = parse_env_lines(&text).unwrap();
        assert_eq!(parsed["TOKEN"], "${MCP_X_TOKEN}");
        assert_eq!(parsed["PLAIN"], MASK);

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer ${TOKEN}".into());
        headers.insert("X-Api-Key".into(), "secret-value".into());
        let text = header_lines(&headers);
        let parsed = parse_header_lines(&text).unwrap();
        assert_eq!(parsed["Authorization"], "Bearer ${TOKEN}");
        assert_eq!(parsed["X-Api-Key"], MASK);

        assert!(parse_env_lines("NO_EQUALS").is_err());
        assert!(parse_header_lines("NoColon").is_err());
    }

    #[test]
    fn typed_literals_are_stored_and_referenced() {
        // A fresh literal is stored and referenced.
        let resolution = resolve_typed_secret("ghp_secret", None, "MCP_G_TOKEN");
        assert_eq!(resolution.value, "${MCP_G_TOKEN}");
        assert_eq!(
            resolution.store,
            Some(SecretUpdate {
                name: "MCP_G_TOKEN".into(),
                value: "ghp_secret".into()
            })
        );
        // A reference is kept as typed.
        assert_eq!(
            resolve_typed_secret("${TOKEN}", None, "MCP_G_TOKEN").value,
            "${TOKEN}"
        );
        // A wrapper is kept as typed.
        assert_eq!(
            resolve_typed_secret("Bearer ${TOKEN}", None, "MCP_G_TOKEN").value,
            "Bearer ${TOKEN}"
        );
        // An untouched mask moves an existing literal into the store.
        let resolution = resolve_typed_secret(MASK, Some("old_secret"), "MCP_G_TOKEN");
        assert_eq!(resolution.value, "${MCP_G_TOKEN}");
        assert_eq!(resolution.store.unwrap().value, "old_secret");
        // An untouched mask keeps an existing reference.
        assert_eq!(
            resolve_typed_secret(MASK, Some("${OLD}"), "MCP_G_TOKEN").value,
            "${OLD}"
        );
    }

    #[test]
    fn generated_names_are_scoped_to_server_and_field() {
        assert_eq!(
            generated_secret_name("github", "TOKEN", McpScope::Global, None),
            "MCP_GLOBAL_GITHUB_TOKEN"
        );
        assert_eq!(
            generated_secret_name("my-server", "Authorization", McpScope::Global, None),
            "MCP_GLOBAL_MY_SERVER_AUTHORIZATION"
        );
    }

    #[test]
    fn generated_names_separate_scopes_and_workspaces() {
        let project_a = generated_secret_name(
            "github",
            "TOKEN",
            McpScope::Project,
            Some(Path::new("/work/a")),
        );
        let project_b = generated_secret_name(
            "github",
            "TOKEN",
            McpScope::Project,
            Some(Path::new("/work/b")),
        );
        let global = generated_secret_name("github", "TOKEN", McpScope::Global, None);
        assert_ne!(project_a, global);
        assert_ne!(project_a, project_b);
        assert!(project_a.starts_with("MCP_P"));
        assert!(project_a.ends_with("_GITHUB_TOKEN"));
        // Deterministic, so a persisted `${NAME}` keeps resolving across runs.
        assert_eq!(
            project_a,
            generated_secret_name(
                "github",
                "TOKEN",
                McpScope::Project,
                Some(Path::new("/work/a")),
            )
        );
        assert_eq!(
            secret_namespace(McpScope::Global, Some(Path::new("/work/a"))),
            "MCP_GLOBAL_"
        );
    }

    #[test]
    fn known_secrets_are_scrubbed_from_errors() {
        let scrubbed = scrub_error(
            "connect failed: Authorization: Bearer abcdefghijklmnop (token ghp_deadbeef)",
            &["ghp_deadbeef".to_string()],
        );
        assert!(!scrubbed.contains("ghp_deadbeef"));
        assert!(!scrubbed.contains("abcdefghijklmnop"));
        assert!(scrubbed.contains(MASK));
    }

    #[test]
    fn fingerprint_changes_when_a_file_changes() {
        let dir = temp_dir("fingerprint");
        let home = home(&dir);
        let before = fingerprint(&home, None);
        let path = McpScope::Global.config_path(&home, None).unwrap();
        write_add(&path, "github", &stdio_def("npx")).unwrap();
        let after = fingerprint(&home, None);
        assert_ne!(before, after);
    }

    #[test]
    fn secret_references_collects_env_headers_and_oauth() {
        let mut def = McpServerDef {
            transport: McpTransport::StreamableHttp {
                url: "https://example.com/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer ${DOCS}".into())]),
            },
            oauth: Some(McpOAuth {
                client_secret: Some("${OAUTH_SECRET}".into()),
                ..McpOAuth::default()
            }),
            ..McpServerDef::default()
        };
        def.transport
            .secret_values_mut()
            .insert("TOKEN".into(), "${TOKEN}".into());
        let refs = definition_references(&def);
        assert!(refs.contains(&"DOCS".to_string()));
        assert!(refs.contains(&"TOKEN".to_string()));
        assert!(refs.contains(&"OAUTH_SECRET".to_string()));
    }

    #[test]
    fn write_atomic_leaves_no_temp_file() {
        let dir = temp_dir("atomic");
        let path = dir.join("mcp.json");
        write_atomic(&path, "{\"mcpServers\":{}}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"mcpServers\":{}}");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_secure_makes_the_file_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("atomic-secure");
        let path = dir.join("mcp-secrets.json");
        write_atomic_secure(&path, "{\"secrets\":{}}").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
