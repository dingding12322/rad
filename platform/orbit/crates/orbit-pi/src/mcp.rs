//! Orbit's MCP management layer.
//!
//! Pi owns MCP execution: it reads its own configuration files
//! (`~/.pi/agent/mcp.json` and `<project>/.pi/mcp.json`), connects servers,
//! invokes tools, and gates every call through its tool pipeline. Orbit owns
//! the *management* surface: a native settings page over exactly those files,
//! validation, scope awareness, secrets kept out of the config, connection
//! status and tool visibility via Pi's own `pi mcp list --json` probe, and a
//! safe restart that applies configuration changes to a live session without
//! losing it.
//!
//! There is deliberately no Orbit-side MCP protocol implementation here.
//! Transports, tool invocation, OAuth, and reconnection stay in Pi; this
//! module only decides which servers *should* run and reports what Pi says
//! happened. See `docs/mcp.md` for the full architecture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(crate) mod config;
pub(crate) mod manager;
pub(crate) mod secrets;

pub(crate) use manager::McpManager;

/// The masked stand-in shown for any secret value. Never a real secret, and
/// never written into a config file: an untouched mask round-trips to the
/// value it hides (see [`config::merge_secret_lines`]).
pub(crate) const MASK: &str = "••••••••";

/// Where an MCP server is defined — Pi's global config or the active
/// project's `.pi/mcp.json`. Project entries replace global entries with the
/// same name, matching Pi's own merge rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum McpScope {
    Global,
    Project,
}

impl McpScope {
    pub(crate) const ALL: [Self; 2] = [Self::Global, Self::Project];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }

    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Global => "mcp.scope_global",
            Self::Project => "mcp.scope_project",
        }
    }

    /// `~/.pi/agent/mcp.json` for global, `<workspace>/.pi/mcp.json` for the
    /// active project. `None` for a project scope with no workspace — the
    /// caller must resolve one before writing.
    pub(crate) fn config_path(self, home: &Path, workspace: Option<&Path>) -> Option<PathBuf> {
        match self {
            Self::Global => Some(home.join(".pi").join("agent").join("mcp.json")),
            Self::Project => workspace.map(|dir| dir.join(".pi").join("mcp.json")),
        }
    }
}

/// How the model reaches a server's tools. Mirrors Pi's `McpExposure`
/// exactly; the default is `codemode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum McpExposure {
    Codemode,
    CodemodeDeferred,
    Deferred,
    Direct,
    Hidden,
}

impl McpExposure {
    /// Every mode, in the order the form offers them.
    pub(crate) const ALL: [Self; 5] = [
        Self::Codemode,
        Self::CodemodeDeferred,
        Self::Deferred,
        Self::Direct,
        Self::Hidden,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Codemode => "codemode",
            Self::CodemodeDeferred => "codemode-deferred",
            Self::Deferred => "deferred",
            Self::Direct => "direct",
            Self::Hidden => "hidden",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }

    /// Pi's default: the key is omitted from the file rather than written.
    pub(crate) fn is_default(self) -> bool {
        self == Self::Codemode
    }

    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Codemode => "mcp.exposure_codemode",
            Self::CodemodeDeferred => "mcp.exposure_codemode_deferred",
            Self::Deferred => "mcp.exposure_deferred",
            Self::Direct => "mcp.exposure_direct",
            Self::Hidden => "mcp.exposure_hidden",
        }
    }

    pub(crate) fn description_key(self) -> &'static str {
        match self {
            Self::Codemode => "mcp.exposure_codemode_hint",
            Self::CodemodeDeferred => "mcp.exposure_codemode_deferred_hint",
            Self::Deferred => "mcp.exposure_deferred_hint",
            Self::Direct => "mcp.exposure_direct_hint",
            Self::Hidden => "mcp.exposure_hidden_hint",
        }
    }
}

/// Which kind of server the entry describes. The transport itself lives in
/// [`McpTransport`]; this is the small copyable discriminant the UI uses for
/// the form's segmented control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpTransportKind {
    Stdio,
    StreamableHttp,
}

impl McpTransportKind {
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Stdio => "mcp.transport_stdio",
            Self::StreamableHttp => "mcp.transport_http",
        }
    }

    pub(crate) fn hint_key(self) -> &'static str {
        match self {
            Self::Stdio => "mcp.stdio_hint",
            Self::StreamableHttp => "mcp.http_hint",
        }
    }
}

/// One server's transport, exactly as Pi defines it: stdio entries take
/// `command` / `args` / `env` / `cwd`; HTTP entries take `url` / `headers`.
/// `env` and `headers` values may be literal, `${NAME}` references, or
/// `!command` invocations — the same expansion Pi performs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    StreamableHttp {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl McpTransport {
    pub(crate) fn kind(&self) -> McpTransportKind {
        match self {
            Self::Stdio { .. } => McpTransportKind::Stdio,
            Self::StreamableHttp { .. } => McpTransportKind::StreamableHttp,
        }
    }

    /// The `env` (`Stdio`) or `headers` (`StreamableHttp`) map — the values
    /// that may hold secrets.
    pub(crate) fn secret_values(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Stdio { env, .. } => env,
            Self::StreamableHttp { headers, .. } => headers,
        }
    }

    /// Mutable access for callers assembling a definition (the editor and its
    /// tests).
    #[cfg(test)]
    pub(crate) fn secret_values_mut(&mut self) -> &mut BTreeMap<String, String> {
        match self {
            Self::Stdio { env, .. } => env,
            Self::StreamableHttp { headers, .. } => headers,
        }
    }
}

/// OAuth client settings for HTTP servers without an `Authorization` header.
/// Pi performs the sign-in; Orbit only round-trips the settings through the
/// form.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct McpOAuth {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub callback_port: Option<u64>,
    pub callback_url: Option<String>,
    pub scope: Option<String>,
}

impl McpOAuth {
    pub(crate) fn is_empty(&self) -> bool {
        self.client_id.is_none()
            && self.client_secret.is_none()
            && self.callback_port.is_none()
            && self.callback_url.is_none()
            && self.scope.is_none()
    }
}

/// One server definition as written to `mcp.json` — everything Pi reads,
/// with none of Orbit's identity (name / scope / source / description).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct McpServerDef {
    pub transport: McpTransport,
    pub exposure: McpExposure,
    pub tool_exposure: BTreeMap<String, McpExposure>,
    pub enabled: bool,
    pub timeout: Option<u64>,
    pub oauth: Option<McpOAuth>,
}

impl Default for McpServerDef {
    fn default() -> Self {
        Self {
            transport: McpTransport::Stdio {
                command: String::new(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
            },
            exposure: McpExposure::Codemode,
            tool_exposure: BTreeMap::new(),
            enabled: true,
            timeout: None,
            oauth: None,
        }
    }
}

/// One configured server: its Pi-visible definition plus Orbit's identity for
/// it. `description` is Orbit-only metadata kept in a sidecar file — Pi never
/// sees it and it is never written into `mcp.json`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct McpServer {
    pub name: String,
    pub scope: McpScope,
    /// The `mcp.json` that defines it (project entries may shadow a global
    /// entry of the same name; this is the project file then).
    pub source: PathBuf,
    pub description: Option<String>,
    pub def: McpServerDef,
}

impl McpServer {
    /// Whether Pi would use OAuth for this server: a streamable-HTTP server
    /// without an explicit `Authorization` header. Pi attempts OAuth exactly
    /// for these and reports `needs-auth` when the server rejects the
    /// connection; a server with an explicit header is never a candidate.
    pub(crate) fn oauth_candidate(&self) -> bool {
        match &self.def.transport {
            McpTransport::StreamableHttp { headers, .. } => !headers
                .keys()
                .any(|name| name.eq_ignore_ascii_case("authorization")),
            McpTransport::Stdio { .. } => false,
        }
    }
}

/// A server's live state as reported by `pi mcp list --json`. `Unknown` is
/// the honest state before the first probe; `Disabled` is a configured
/// choice, not an error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum McpServerStatus {
    #[default]
    Unknown,
    Disabled,
    Connecting,
    Connected,
    Disconnected,
    NeedsAuth,
    Failed,
}

impl McpServerStatus {
    /// Parse Pi's `state` string. Unknown values stay `Unknown` rather than
    /// being invented into a state Pi never reported.
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "disabled" => Self::Disabled,
            "connecting" => Self::Connecting,
            "connected" => Self::Connected,
            "disconnected" | "closed" => Self::Disconnected,
            "needs-auth" => Self::NeedsAuth,
            "failed" => Self::Failed,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Unknown => "mcp.status_unknown",
            Self::Disabled => "mcp.disabled",
            Self::Connecting => "mcp.status_connecting",
            Self::Connected => "mcp.status_connected",
            Self::Disconnected => "mcp.status_disconnected",
            Self::NeedsAuth => "mcp.status_needs_auth",
            Self::Failed => "mcp.status_failed",
        }
    }
}

/// Cached probe result for one server: state, tool names, and the safe error
/// Pi reported. Orbit never fabricates entries — a server missing from the
/// probe stays at its previous runtime (or `Unknown`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct McpServerRuntime {
    pub status: McpServerStatus,
    pub tools: Vec<String>,
    /// Per-tool exposure overrides Pi reported (only tools differing from the
    /// server's own exposure).
    pub tool_exposure: BTreeMap<String, McpExposure>,
    pub resources: u64,
    pub resource_templates: u64,
    /// Pi's full connection error, already scrubbed of known secret values.
    pub error: Option<String>,
    pub checked_at: Option<Instant>,
}

/// A typed MCP failure. Every variant carries safe user-facing context and
/// never a secret value; [`McpError::user_message`] composes the message the
/// UI shows (and the docs recommend as "actionable"). Some variants are
/// produced by the UI's connection-test classification rather than by this
/// module directly, so the whole taxonomy stays part of the error contract.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum McpError {
    InvalidConfiguration(String),
    ServerNotFound(String),
    ProcessStartFailed(String),
    ConnectionFailed(String),
    AuthenticationFailed(String),
    Timeout(String),
    ConfigurationSyncFailed(String),
    PiUnavailable,
    UnsupportedTransport(String),
    /// The user cancelled a sign-in before the browser completed it.
    Cancelled,
    Io(String),
}

impl McpError {
    /// The actionable message shown in the UI. Context-specific reasons are
    /// composed by callers with the relevant command/URL; this is the typed
    /// fallback.
    pub(crate) fn user_message(&self) -> String {
        match self {
            Self::InvalidConfiguration(detail) => {
                tr!("mcp.error_invalid_configuration", detail = detail)
            }
            Self::ServerNotFound(name) => tr!("mcp.error_server_not_found", name = name),
            Self::ProcessStartFailed(detail) => {
                tr!("mcp.error_process_start_failed", detail = detail)
            }
            Self::ConnectionFailed(detail) => tr!("mcp.error_connection_failed", detail = detail),
            Self::AuthenticationFailed(detail) => {
                tr!("mcp.error_authentication_failed", detail = detail)
            }
            Self::Timeout(detail) => tr!("mcp.error_timeout", detail = detail),
            Self::ConfigurationSyncFailed(detail) => {
                tr!("mcp.error_config_sync_failed", detail = detail)
            }
            Self::PiUnavailable => tr!("mcp.error_pi_unavailable"),
            Self::UnsupportedTransport(detail) => {
                tr!("mcp.error_unsupported_transport", detail = detail)
            }
            Self::Cancelled => tr!("mcp.error_cancelled"),
            Self::Io(detail) => tr!("mcp.error_io", detail = detail),
        }
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.user_message())
    }
}

impl std::error::Error for McpError {}

impl McpError {
    /// Scrub each variant's detail through `redact`. Used before an error
    /// that may embed Pi's stderr (which can echo an environment or header
    /// value) reaches the UI or a log.
    pub(crate) fn redacted(self, redact: impl Fn(&str) -> String) -> Self {
        match self {
            Self::InvalidConfiguration(detail) => Self::InvalidConfiguration(redact(&detail)),
            Self::ServerNotFound(name) => Self::ServerNotFound(redact(&name)),
            Self::ProcessStartFailed(detail) => Self::ProcessStartFailed(redact(&detail)),
            Self::ConnectionFailed(detail) => Self::ConnectionFailed(redact(&detail)),
            Self::AuthenticationFailed(detail) => Self::AuthenticationFailed(redact(&detail)),
            Self::Timeout(detail) => Self::Timeout(redact(&detail)),
            Self::ConfigurationSyncFailed(detail) => Self::ConfigurationSyncFailed(redact(&detail)),
            Self::UnsupportedTransport(detail) => Self::UnsupportedTransport(redact(&detail)),
            Self::Io(detail) => Self::Io(redact(&detail)),
            Self::PiUnavailable | Self::Cancelled => self,
        }
    }
}

/// The `${NAME}` references inside one value, if the whole value is a single
/// reference (`${TOKEN}`) or a wrapper around one (`Bearer ${TOKEN}`).
/// `!command` values name no reference.
pub(crate) fn secret_references(value: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            refs.push(name.to_string());
        }
        rest = &after[end + 1..];
    }
    refs
}

/// Whether a value is exactly one `${NAME}` reference. Those are displayed
/// as-is (they name a secret, they are not one); everything else in a secret
/// position is masked.
pub(crate) fn is_pure_reference(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("${") && trimmed.ends_with('}') && secret_references(trimmed).len() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_references_reads_whole_and_wrapped_values() {
        assert_eq!(secret_references("${TOKEN}"), vec!["TOKEN"]);
        assert_eq!(
            secret_references("Bearer ${DOCS_TOKEN}"),
            vec!["DOCS_TOKEN"]
        );
        assert_eq!(
            secret_references("${A}-${B}"),
            vec!["A".to_string(), "B".to_string()]
        );
        assert!(secret_references("!echo hi").is_empty());
        assert!(secret_references("literal").is_empty());
        assert!(secret_references("${not a name}").is_empty());
        assert!(secret_references("${1BAD}").is_empty());
    }

    #[test]
    fn pure_references_are_distinguished_from_wrappers() {
        assert!(is_pure_reference("${TOKEN}"));
        assert!(!is_pure_reference("Bearer ${TOKEN}"));
        assert!(!is_pure_reference("literal"));
    }

    #[test]
    fn exposure_round_trips_every_mode() {
        for mode in McpExposure::ALL {
            assert_eq!(McpExposure::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(McpExposure::parse("sse"), None);
        assert!(McpExposure::Codemode.is_default());
    }

    #[test]
    fn scope_paths_match_pi() {
        let home = Path::new("/home/u");
        assert_eq!(
            McpScope::Global.config_path(home, None),
            Some(PathBuf::from("/home/u/.pi/agent/mcp.json"))
        );
        assert_eq!(
            McpScope::Project.config_path(home, Some(Path::new("/work/p"))),
            Some(PathBuf::from("/work/p/.pi/mcp.json"))
        );
        assert_eq!(McpScope::Project.config_path(home, None), None);
    }

    #[test]
    fn status_parsing_maps_pis_states() {
        assert_eq!(
            McpServerStatus::parse("needs-auth"),
            McpServerStatus::NeedsAuth
        );
        assert_eq!(
            McpServerStatus::parse("closed"),
            McpServerStatus::Disconnected
        );
        assert_eq!(McpServerStatus::parse("bogus"), McpServerStatus::Unknown);
    }

    #[test]
    fn oauth_candidates_follow_pis_rule() {
        let server = |transport| McpServer {
            name: "docs".into(),
            scope: McpScope::Global,
            source: PathBuf::from("/tmp/mcp.json"),
            description: None,
            def: McpServerDef {
                transport,
                ..McpServerDef::default()
            },
        };
        // HTTP without an Authorization header: Pi attempts OAuth.
        assert!(server(McpTransport::StreamableHttp {
            url: "https://example.com/mcp".into(),
            headers: BTreeMap::new(),
        })
        .oauth_candidate());
        // Any casing of the header opts out, matching Pi's check.
        assert!(!server(McpTransport::StreamableHttp {
            url: "https://example.com/mcp".into(),
            headers: BTreeMap::from([("authorization".into(), "Bearer x".into())]),
        })
        .oauth_candidate());
        assert!(!server(McpTransport::StreamableHttp {
            url: "https://example.com/mcp".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer x".into())]),
        })
        .oauth_candidate());
        // stdio never uses OAuth.
        assert!(!server(McpTransport::Stdio {
            command: "npx".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        })
        .oauth_candidate());
    }
}
