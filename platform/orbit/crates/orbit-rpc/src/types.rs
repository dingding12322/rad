//! Typed model of the pi CLI RPC protocol (`pi --mode rpc`).
//!
//! Protocol reference: `packages/coding-agent/docs/rpc.md` in the pi repo.
//! Framing is strict JSONL: LF-only record delimiter, optional trailing `\r`.
//!
//! This module is intentionally permissive: every envelope is parsed loosely
//! and unknown fields/variants flow through as raw JSON so forward protocol
//! changes never break the client. Full typing of every message shape is
//! graduated into P2 once real event dumps are in from live runs.

use serde_json::Value;

// ───────────────────────────────────────────────────────────────────────────
// Commands (sent to stdin, one JSON line each)
// ───────────────────────────────────────────────────────────────────────────

/// A command envelope. `id` is optional for correlation on the wire, but the
/// client always adds one so every command gets a matched response.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Command {
    pub id: String,
    #[serde(flatten)]
    pub body: CommandBody,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandBody {
    /// Send a user prompt. `images` are `{type:"image", data: base64, mimeType}`.
    Prompt {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<Value>>,
        #[serde(rename = "streamingBehavior", skip_serializing_if = "Option::is_none")]
        streaming_behavior: Option<String>,
    },
    /// Interrupt the current assistant turn; queued messages keep running.
    Abort,
    /// Drop everything queued for the current session.
    ClearQueue,
    /// Inject a message into the running turn without aborting it. Delivered
    /// after the current assistant turn finishes its tool calls, before the
    /// next LLM call. Images are `{type:"image", data: base64, mimeType}`.
    Steer {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<Value>>,
    },
    /// Queue a follow-up message delivered only once the agent settles.
    FollowUp {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<Value>>,
    },
    /// How queued steering messages are delivered: `"all"` (every queued
    /// steer after the current step) or `"one-at-a-time"` (one per turn).
    SetSteeringMode {
        mode: String,
    },
    /// How queued follow-up messages are delivered: `"all"` or
    /// `"one-at-a-time"`.
    SetFollowUpMode {
        mode: String,
    },
    /// Compact the conversation now, optionally with custom instructions.
    Compact {
        #[serde(rename = "customInstructions", skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
    /// Toggle automatic compaction when the context nears full.
    SetAutoCompaction {
        enabled: bool,
    },
    /// Toggle automatic retry on transient provider errors.
    SetAutoRetry {
        enabled: bool,
    },
    /// Cancel an in-progress automatic retry.
    AbortRetry,
    /// Set the session's display name.
    SetSessionName {
        name: String,
    },
    /// Create a fresh session (new session id/file returned in the response).
    NewSession,
    /// Load a different session file.
    SwitchSession {
        #[serde(rename = "sessionPath")]
        session_path: String,
    },
    GetState,
    GetMessages,
    GetAvailableModels,
    SetModel {
        #[serde(rename = "modelId")]
        model_id: String,
        provider: String,
    },
    CycleModel,
    GetAvailableThinkingLevels,
    SetThinkingLevel {
        level: String,
    },
    CycleThinkingLevel,
    GetCommands,
    /// Token totals + current context-window usage for the open session.
    GetSessionStats,
    /// Message list (with `entryId`s) used to build a fork point.
    GetForkMessages,
    /// Session entries in append order, including custom entries written by
    /// extensions. `since` is an entry id cursor: only entries strictly after
    /// it are returned, so a client can poll without re-reading the session.
    GetEntries {
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    /// Rewind the session to just before `entry_id` (drops later turns).
    Fork {
        #[serde(rename = "entryId")]
        entry_id: String,
    },
    /// Duplicate the session as-is (fork with nothing removed).
    #[serde(rename = "clone")]
    CloneSession,

    // ── provider authentication ─────────────────────────────────────────
    /// List providers and the auth methods each supports. This is the
    /// capability-discovery command: the UI learns what to render from pi
    /// instead of hardcoding provider ids or flows.
    #[serde(rename = "auth.list")]
    AuthList,
    /// Current credential status for one provider (never token material).
    #[serde(rename = "auth.status")]
    AuthStatus {
        provider: String,
    },
    /// Begin a login. `method` is provider-defined (`browser`, `device_code`,
    /// `api_key`, …) and is discovered from `auth.list`. `session_id` is
    /// client-generated so the login can be cancelled deterministically and
    /// reconciled after a restart; pi echoes it on every `auth.*` event.
    #[serde(rename = "auth.login")]
    AuthLogin {
        provider: String,
        method: String,
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// Remove the stored credential for a provider.
    #[serde(rename = "auth.logout")]
    AuthLogout {
        provider: String,
    },
    /// Cancel an in-flight login session.
    #[serde(rename = "auth.cancel")]
    AuthCancel {
        #[serde(rename = "sessionId")]
        session_id: String,
    },

    // ── provider quota / usage ──────────────────────────────────────────
    /// Account-level quota, balance, or spend for connected providers. pi
    /// resolves each provider's credential and queries the provider's own
    /// usage endpoint; Orbit only ever receives normalized, non-secret
    /// figures. `provider` (when present) restricts the query to one id.
    #[serde(rename = "quota.list")]
    QuotaList {
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },

    /// Anything the typed enum does not cover yet; sent verbatim.
    Raw(Value),
}

impl Command {
    pub fn new(id: impl Into<String>, body: CommandBody) -> Self {
        Self {
            id: id.into(),
            body,
        }
    }

    /// Serialize to the JSONL wire form (single line, no trailing newline).
    pub fn to_wire(&self) -> anyhow::Result<String> {
        let value = match &self.body {
            CommandBody::Raw(payload) => {
                let mut value = payload.clone();
                // A payload may carry its own id (extension UI responses
                // reference the request's id) — only stamp ours when absent.
                if value.get("id").is_none() {
                    value["id"] = Value::String(self.id.clone());
                }
                value
            }
            _ => serde_json::to_value(self)?,
        };
        serde_json::to_string(&value).map_err(Into::into)
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Events (streamed to stdout as JSON lines)
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Event {
    /// Reply to a command (`{"type":"response","id":…,"command":…,"success":…}`).
    Response {
        id: String,
        command: String,
        success: bool,
        data: Option<Value>,
        error: Option<String>,
    },

    /// Streaming update for the assistant message currently being produced.
    MessageUpdate {
        usage: Option<Value>,
        assistant: Option<AssistantMessageEvent>,
    },

    AgentStart,
    AgentEnd {
        will_retry: bool,
    },
    AgentSettled,
    TurnStart,
    TurnEnd {
        value: Value,
    },
    MessageStart {
        value: Value,
    },
    MessageEnd {
        value: Value,
    },
    ToolExecutionStart {
        value: Value,
    },
    ToolExecutionUpdate {
        value: Value,
    },
    ToolExecutionEnd {
        value: Value,
    },
    BashExecutionUpdate {
        value: Value,
    },
    QueueUpdate {
        value: Value,
    },
    CompactionStart {
        value: Value,
    },
    CompactionEnd {
        value: Value,
    },
    AutoRetryStart {
        value: Value,
    },
    AutoRetryEnd {
        value: Value,
    },

    /// pi renamed the open session (`name` is `null` when cleared). This
    /// forwards it as an automatic session title.
    SessionInfoChanged {
        name: Option<String>,
    },

    /// Asynchronous provider-authentication progress. Carries no secrets:
    /// device codes and verification URLs are meant to be displayed, and the
    /// completion event names the credential kind, never its value.
    Auth(AuthEvent),

    ExtensionError {
        value: Value,
    },

    /// User interaction request (question dialogs, status widgets, …).
    /// Dialog methods (`select`/`confirm`/`input`/`editor`) expect an
    /// `extension_ui_response` on stdin with the matching `id`.
    ExtensionUiRequest {
        id: String,
        method: String,
        value: Value,
    },

    /// The pi process exited (reader loop hit EOF).
    ProcessExited,

    /// Everything we have not typed yet; kept verbatim.
    Unknown(Value),
}

/// The per-message delta carried inside `message_update`.
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    TextDelta {
        delta: String,
    },
    ThinkingDelta {
        delta: String,
    },
    ToolcallStart {
        value: Value,
    },
    ToolcallDelta {
        value: Value,
    },
    ToolcallEnd {
        value: Value,
    },
    /// e.g. `text_content_updated`, finished-message snapshots, new shapes.
    Other {
        kind: String,
        value: Value,
    },
}

// ───────────────────────────────────────────────────────────────────────────
// Provider authentication
// ───────────────────────────────────────────────────────────────────────────

/// One authentication method a provider supports, as advertised by
/// `auth.list`. `id` is the value sent back in `auth.login`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethodCapability {
    pub id: String,
    pub label: String,
}

/// A provider's authentication capability, discovered from `auth.list`.
///
/// The UI renders entirely from this: it never hardcodes a provider id or
/// assumes a particular flow. `credential` is the kind currently stored
/// (`oauth`, `api_key`, or empty/`none`), not the credential itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthProvider {
    pub id: String,
    pub name: String,
    /// `oauth` | `api_key` | `none` (unknown kinds pass through as-is).
    pub credential: String,
    pub authenticated: bool,
    pub methods: Vec<AuthMethodCapability>,
}

impl AuthProvider {
    /// Parse one provider object from an `auth.list` payload. Returns `None`
    /// when it has no usable id.
    pub fn from_value(value: &Value) -> Option<Self> {
        let id = value.get("id").and_then(Value::as_str)?.to_string();
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(&id)
            .to_string();
        let credential = value
            .get("credential")
            .or_else(|| value.get("credentialKind"))
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string();
        let authenticated = value
            .get("authenticated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let methods = value
            .get("methods")
            .and_then(Value::as_array)
            .map(|methods| {
                methods
                    .iter()
                    .filter_map(|method| {
                        let id = method.get("id").and_then(Value::as_str)?.to_string();
                        let label = method
                            .get("label")
                            .and_then(Value::as_str)
                            .filter(|label| !label.is_empty())
                            .unwrap_or(&id)
                            .to_string();
                        Some(AuthMethodCapability { id, label })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            id,
            name,
            credential,
            authenticated,
            methods,
        })
    }

    /// Whether OAuth (browser or device code) is among the advertised methods.
    pub fn supports_oauth(&self) -> bool {
        self.methods
            .iter()
            .any(|method| matches!(method.id.as_str(), "browser" | "device_code" | "oauth"))
    }

    /// Whether a direct API-key entry is among the advertised methods.
    pub fn supports_api_key(&self) -> bool {
        self.methods.iter().any(|method| method.id == "api_key")
    }

    /// The device-code method's id, if offered.
    pub fn device_code_method(&self) -> Option<&AuthMethodCapability> {
        self.methods
            .iter()
            .find(|method| method.id == "device_code")
    }
}

/// Structured error classification carried by `auth_login_failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthErrorCode {
    InvalidProvider,
    UnsupportedMethod,
    AlreadyInProgress,
    NotAuthenticated,
    Unsupported,
    LoginFailed,
    Timeout,
    Cancelled,
    Network,
    Storage,
    Internal,
    Unknown,
}

impl AuthErrorCode {
    /// Map a wire code to a typed variant. Unknown codes collapse to
    /// [`AuthErrorCode::Unknown`] so a new server code is never fatal.
    pub fn from_code(code: &str) -> Self {
        match code {
            "invalid_provider" => Self::InvalidProvider,
            "unsupported_method" => Self::UnsupportedMethod,
            "already_in_progress" => Self::AlreadyInProgress,
            "not_authenticated" => Self::NotAuthenticated,
            "unsupported" => Self::Unsupported,
            "login_failed" => Self::LoginFailed,
            "timeout" => Self::Timeout,
            "cancelled" | "canceled" => Self::Cancelled,
            "network_error" | "network" => Self::Network,
            "storage_error" | "storage" => Self::Storage,
            "internal_error" | "internal" => Self::Internal,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidProvider => "invalid_provider",
            Self::UnsupportedMethod => "unsupported_method",
            Self::AlreadyInProgress => "already_in_progress",
            Self::NotAuthenticated => "not_authenticated",
            Self::Unsupported => "unsupported",
            Self::LoginFailed => "login_failed",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Network => "network_error",
            Self::Storage => "storage_error",
            Self::Internal => "internal_error",
            Self::Unknown => "unknown",
        }
    }
}

/// A single asynchronous authentication event (`auth.*`).
#[derive(Debug, Clone, PartialEq)]
pub enum AuthEvent {
    /// A login session was accepted and is now running.
    LoginStarted {
        session_id: String,
        provider: String,
        method: String,
        expires_at: Option<i64>,
    },
    /// Browser flow: URL the OS opener should launch.
    LoginUrl {
        session_id: String,
        provider: String,
        url: String,
    },
    /// Device-code flow: the user code and verification URL to display.
    DeviceCode {
        session_id: String,
        provider: String,
        user_code: String,
        verification_uri: String,
        verification_uri_complete: Option<String>,
        expires_at: Option<i64>,
    },
    /// Optional progress note (e.g. "waiting for authorization").
    LoginWaiting {
        session_id: String,
        provider: String,
        message: Option<String>,
    },
    /// Login finished. Names the credential kind, never its value.
    LoginSucceeded {
        session_id: String,
        provider: String,
        method: Option<String>,
        credential: Option<String>,
        expires_at: Option<i64>,
    },
    LoginFailed {
        session_id: String,
        provider: String,
        code: AuthErrorCode,
        message: String,
    },
    LoginCancelled {
        session_id: String,
        provider: String,
    },
    /// One or more providers' stored credentials changed (login or logout).
    CredentialsChanged { providers: Vec<String> },
    /// A future `auth.*` event; the UI ignores it and the raw value survives.
    Other { kind: String, value: Value },
}

impl AuthEvent {
    fn from_value(value: Value) -> Self {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        let session_id = str_field(&value, "sessionId");
        let provider = str_field(&value, "provider");
        match kind {
            "auth_login_started" => Self::LoginStarted {
                session_id,
                provider,
                method: str_field(&value, "method"),
                expires_at: int_field(&value, "expiresAt"),
            },
            "auth_login_url" | "auth_browser_url" => Self::LoginUrl {
                session_id,
                provider,
                url: str_field(&value, "url"),
            },
            "auth_device_code" => Self::DeviceCode {
                session_id,
                provider,
                user_code: str_field(&value, "userCode"),
                verification_uri: str_field(&value, "verificationUri"),
                verification_uri_complete: opt_str_field(&value, "verificationUriComplete"),
                expires_at: int_field(&value, "expiresAt"),
            },
            "auth_login_waiting" => Self::LoginWaiting {
                session_id,
                provider,
                message: opt_str_field(&value, "message"),
            },
            "auth_login_succeeded" => Self::LoginSucceeded {
                session_id,
                provider,
                method: opt_str_field(&value, "method"),
                credential: opt_str_field(&value, "credential"),
                expires_at: int_field(&value, "expiresAt"),
            },
            "auth_login_failed" => Self::LoginFailed {
                session_id,
                provider,
                code: value
                    .get("code")
                    .and_then(Value::as_str)
                    .map(AuthErrorCode::from_code)
                    .unwrap_or(AuthErrorCode::Unknown),
                message: str_field(&value, "message"),
            },
            "auth_login_cancelled" => Self::LoginCancelled {
                session_id,
                provider,
            },
            "auth_credentials_changed" => Self::CredentialsChanged {
                providers: value
                    .get("providers")
                    .and_then(Value::as_array)
                    .map(|providers| {
                        providers
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            other => Self::Other {
                kind: other.to_string(),
                value,
            },
        }
    }

    /// The login session this event belongs to, when it is session-scoped.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::LoginStarted { session_id, .. }
            | Self::LoginUrl { session_id, .. }
            | Self::DeviceCode { session_id, .. }
            | Self::LoginWaiting { session_id, .. }
            | Self::LoginSucceeded { session_id, .. }
            | Self::LoginFailed { session_id, .. }
            | Self::LoginCancelled { session_id, .. } => Some(session_id),
            Self::CredentialsChanged { .. } | Self::Other { .. } => None,
        }
    }
}

/// Any `auth_*` event type routes through [`AuthEvent::from_value`].
pub fn is_auth_event(kind: &str) -> bool {
    kind.starts_with("auth_")
}

// ───────────────────────────────────────────────────────────────────────────
// Provider quota / usage
// ───────────────────────────────────────────────────────────────────────────

/// What a provider's report describes. Mirrors the union of provider billing
/// models: subscription windows, credits, a cash balance, or trailing spend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaKind {
    /// Subscription rate-limit windows (5h / weekly / monthly).
    Subscription,
    /// Prepaid credits (money or credit units) plus optional spend.
    Credits,
    /// Account cash balance, potentially in several currencies.
    Balance,
    /// Trailing spend over a fixed window (e.g. last 30 days).
    Spend,
    /// The provider is connected but exposes no usage surface.
    Unsupported,
}

impl QuotaKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Credits => "credits",
            Self::Balance => "balance",
            Self::Spend => "spend",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn from_wire(value: &str) -> Self {
        match value {
            "subscription" => Self::Subscription,
            "credits" => Self::Credits,
            "balance" => Self::Balance,
            "spend" => Self::Spend,
            _ => Self::Unsupported,
        }
    }
}

/// One metered window in a quota report. Either a percentage (`used_percent`)
/// or a counted pair (`used` / `limit`), never both fabricated.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindow {
    /// Stable machine id (e.g. `five_hour`, `weekly`); used for keying.
    pub id: String,
    /// Human label from the adapter (`5-hour`, `Weekly`).
    pub label: String,
    /// 0..=100, when the provider reports a percentage.
    pub used_percent: Option<f32>,
    /// Count consumed, when the provider reports counts.
    pub used: Option<f64>,
    /// Count allowance, when the provider reports counts.
    pub limit: Option<f64>,
    /// Unit for `used`/`limit` (`requests`, `credits`, `tokens`, …).
    pub unit: Option<String>,
    /// Absolute reset time in epoch milliseconds.
    pub resets_at: Option<i64>,
}

impl QuotaWindow {
    pub fn from_value(value: &Value) -> Option<Self> {
        let id = value.get("id").and_then(Value::as_str)?.to_string();
        let label = value
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.is_empty())
            .unwrap_or(&id)
            .to_string();
        let used_percent = value
            .get("usedPercent")
            .and_then(json_f64_or_null)
            .map(|p| p.clamp(0.0, 100.0) as f32);
        let used = value.get("used").and_then(json_f64_or_null);
        let limit = value.get("limit").and_then(json_f64_or_null);
        let unit = value
            .get("unit")
            .and_then(Value::as_str)
            .filter(|unit| !unit.is_empty())
            .map(str::to_owned);
        let resets_at = value.get("resetsAt").and_then(json_i64_or_null);
        Some(Self {
            id,
            label,
            used_percent,
            used,
            limit,
            unit,
            resets_at,
        })
    }

    /// Fraction consumed, 0..=1, for a meter fill. Derived from the percentage
    /// when present, otherwise from `used / limit`.
    pub fn fraction(&self) -> Option<f32> {
        if let Some(percent) = self.used_percent {
            return Some((percent / 100.0).clamp(0.0, 1.0));
        }
        let (used, limit) = (self.used?, self.limit?);
        (limit > 0.0).then(|| ((used / limit) as f32).clamp(0.0, 1.0))
    }
}

/// One monetary balance line (`Available`, `Cash`, `Granted`, …).
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaBalance {
    pub label: String,
    pub amount: f64,
    pub currency: String,
}

impl QuotaBalance {
    pub fn from_value(value: &Value) -> Option<Self> {
        let amount = value.get("amount").and_then(json_f64_or_null)?;
        let label = value
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.is_empty())
            .unwrap_or("Balance")
            .to_string();
        let currency = value
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Some(Self {
            label,
            amount,
            currency,
        })
    }
}

/// A normalized account-usage report for one provider. This is the entire
/// non-secret surface: no token, key, or account identifier is ever present.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaReport {
    pub provider: String,
    pub kind: QuotaKind,
    /// Plan tier label (`Max`, `Plus`, `Pro`), when the provider reports one.
    pub plan: Option<String>,
    pub windows: Vec<QuotaWindow>,
    pub balances: Vec<QuotaBalance>,
    /// Short, provider-supplied note (e.g. an unsupported reason).
    pub note: Option<String>,
    /// When the snapshot was taken, in epoch milliseconds.
    pub fetched_at: Option<i64>,
    /// A provider/query error; the report may still carry stale windows.
    pub error: Option<String>,
}

/// Present a provider-reported plan tier in its documented, canonical form
/// (`Pro`, `Max`, `Plus`). Providers are inconsistent about casing — Ollama's
/// `/api/usage` returns `pro` while its settings page returns `Pro` — so the
/// first character is upper-cased and the rest is left untouched. That keeps
/// the badge stable across providers without mangling an acronym or a
/// provider-specific label such as `ChatGPT Plus`.
fn plan_display_label(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

impl QuotaReport {
    /// Parse one report object from a `quota.list` payload.
    pub fn from_value(value: &Value) -> Option<Self> {
        let provider = value.get("provider").and_then(Value::as_str)?.to_string();
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .map(QuotaKind::from_wire)
            .unwrap_or(QuotaKind::Unsupported);
        let plan = value
            .get("plan")
            .and_then(Value::as_str)
            .filter(|plan| !plan.is_empty())
            .map(plan_display_label);
        let windows = value
            .get("windows")
            .and_then(Value::as_array)
            .map(|windows| windows.iter().filter_map(QuotaWindow::from_value).collect())
            .unwrap_or_default();
        let balances = value
            .get("balances")
            .and_then(Value::as_array)
            .map(|balances| {
                balances
                    .iter()
                    .filter_map(QuotaBalance::from_value)
                    .collect()
            })
            .unwrap_or_default();
        let note = value
            .get("note")
            .and_then(Value::as_str)
            .filter(|note| !note.is_empty())
            .map(str::to_owned);
        let fetched_at = value.get("fetchedAt").and_then(json_i64_or_null);
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
            .map(str::to_owned);
        Some(Self {
            provider,
            kind,
            plan,
            windows,
            balances,
            note,
            fetched_at,
            error,
        })
    }

    /// Whether this report has anything worth rendering.
    pub fn has_data(&self) -> bool {
        !self.windows.is_empty() || !self.balances.is_empty()
    }
}

/// Parse the `providers` array of a `quota.list` payload.
pub fn parse_quota_reports(data: &Value) -> Vec<QuotaReport> {
    data.get("providers")
        .and_then(Value::as_array)
        .map(|reports| reports.iter().filter_map(QuotaReport::from_value).collect())
        .unwrap_or_default()
}

fn json_i64_or_null(value: &Value) -> Option<i64> {
    if value.is_null() {
        return None;
    }
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|f| f as i64))
        .or_else(|| value.as_u64().and_then(|u| i64::try_from(u).ok()))
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn opt_str_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
    })
}

impl Event {
    /// Parse one JSONL line into an [`Event`]. Non-object or empty lines are
    /// treated as unknown rather than fatal, matching pi's own tolerance.
    pub fn parse_line(line: &str) -> Event {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Event::Unknown(Value::String(line.to_string()));
        };
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Event {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return Event::Unknown(value);
        };
        // `auth.*` events are a self-contained namespace; route them before
        // the main match so adding auth event types never touches it.
        if is_auth_event(kind) {
            return Event::Auth(AuthEvent::from_value(value));
        }
        match kind {
            "response" => {
                let id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let command = value
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let success = value
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Event::Response {
                    id,
                    command,
                    success,
                    data: value.get("data").cloned(),
                    error: value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }
            }
            "message_update" => {
                let assistant = value.get("assistantMessageEvent").cloned().map(|v| {
                    let ev = v.get("type").and_then(Value::as_str).unwrap_or("");
                    match ev {
                        "text_delta" => AssistantMessageEvent::TextDelta {
                            delta: v
                                .get("delta")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        },
                        "thinking_delta" => AssistantMessageEvent::ThinkingDelta {
                            delta: v
                                .get("delta")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        },
                        "toolcall_start" => AssistantMessageEvent::ToolcallStart { value: v },
                        "toolcall_delta" => AssistantMessageEvent::ToolcallDelta { value: v },
                        "toolcall_end" => AssistantMessageEvent::ToolcallEnd { value: v },
                        other => AssistantMessageEvent::Other {
                            kind: other.to_string(),
                            value: v,
                        },
                    }
                });
                Event::MessageUpdate {
                    usage: value.get("usage").cloned(),
                    assistant,
                }
            }
            "agent_start" => Event::AgentStart,
            "agent_end" => Event::AgentEnd {
                will_retry: value
                    .get("willRetry")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            "agent_settled" => Event::AgentSettled,
            "turn_start" => Event::TurnStart,
            "turn_end" => Event::TurnEnd { value },
            "message_start" => Event::MessageStart { value },
            "message_end" => Event::MessageEnd { value },
            "tool_execution_start" => Event::ToolExecutionStart { value },
            "tool_execution_update" => Event::ToolExecutionUpdate { value },
            "tool_execution_end" => Event::ToolExecutionEnd { value },
            "bash_execution_update" => Event::BashExecutionUpdate { value },
            "queue_update" => Event::QueueUpdate { value },
            "compaction_start" => Event::CompactionStart { value },
            "compaction_end" => Event::CompactionEnd { value },
            "auto_retry_start" => Event::AutoRetryStart { value },
            "auto_retry_end" => Event::AutoRetryEnd { value },
            "session_info_changed" => Event::SessionInfoChanged {
                name: value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
            },
            "extension_error" => Event::ExtensionError { value },
            "extension_ui_request" => Event::ExtensionUiRequest {
                id: value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                method: value
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                value,
            },
            _ => Event::Unknown(value),
        }
    }

    /// A short, single-line human description used by the dev UI and logs.
    pub fn one_line(&self) -> String {
        match self {
            Event::Response {
                command, success, ..
            } => {
                format!(
                    "response → {command} ({})",
                    if *success { "ok" } else { "err" }
                )
            }
            Event::MessageUpdate { assistant, .. } => match assistant {
                Some(AssistantMessageEvent::TextDelta { delta }) => {
                    format!("text+ {}", delta_summary(delta))
                }
                Some(AssistantMessageEvent::ThinkingDelta { delta }) => {
                    format!("think+ {}", trim(delta, 48))
                }
                Some(AssistantMessageEvent::ToolcallStart { value }) => {
                    format!("tool→ {}", value)
                }
                Some(AssistantMessageEvent::ToolcallDelta { .. }) => "tool args…".into(),
                Some(AssistantMessageEvent::ToolcallEnd { value }) => format!("tool ✓ {value}"),
                Some(AssistantMessageEvent::Other { kind, .. }) => format!("msg:{kind}"),
                None => "message_update".into(),
            },
            Event::AgentStart => "agent_start".into(),
            Event::AgentEnd { will_retry } => format!("agent_end (retry={will_retry})"),
            Event::SessionInfoChanged { name } => {
                format!(
                    "session rename → {}",
                    name.as_deref().unwrap_or("(cleared)")
                )
            }
            Event::Auth(event) => auth_one_line(event),
            Event::AgentSettled => "agent_settled ✓".into(),
            Event::ExtensionUiRequest { method, .. } => format!("ui request: {method}"),
            Event::ProcessExited => "pi exited".into(),
            Event::Unknown(v) => trim(&v.to_string(), 80),
            other => trim(&format!("{other:?}"), 80),
        }
    }
}

fn delta_summary(delta: &str) -> String {
    let t = trim(delta, 48);
    if t.is_empty() {
        "(empty)".into()
    } else {
        t
    }
}

fn auth_one_line(event: &AuthEvent) -> String {
    match event {
        AuthEvent::LoginStarted {
            provider, method, ..
        } => {
            format!("auth started: {provider} ({method})")
        }
        AuthEvent::LoginUrl { provider, .. } => format!("auth url: {provider}"),
        AuthEvent::DeviceCode { provider, .. } => format!("auth device code: {provider}"),
        AuthEvent::LoginWaiting { provider, .. } => format!("auth waiting: {provider}"),
        AuthEvent::LoginSucceeded { provider, .. } => format!("auth ✓ {provider}"),
        AuthEvent::LoginFailed { provider, code, .. } => {
            format!("auth ✗ {provider} ({})", code.as_str())
        }
        AuthEvent::LoginCancelled { provider, .. } => format!("auth cancelled: {provider}"),
        AuthEvent::CredentialsChanged { providers } => {
            format!("auth credentials changed: {}", providers.join(", "))
        }
        AuthEvent::Other { kind, .. } => format!("auth:{kind}"),
    }
}

/// Current context-window snapshot from `get_session_stats`.
///
/// `tokens` / `percent` are `None` immediately after compaction until the
/// next assistant response establishes a fresh baseline. `contextUsage` itself
/// is omitted when no model (or no window) is set — then [`from_stats`]
/// returns `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub context_window: u64,
    pub percent: Option<f64>,
}

impl ContextUsage {
    /// Parse the `contextUsage` object on a `get_session_stats` payload.
    pub fn from_stats(data: &Value) -> Option<Self> {
        let usage = data.get("contextUsage")?;
        if usage.is_null() {
            return None;
        }
        let context_window = json_u64(usage.get("contextWindow")?)?;
        if context_window == 0 {
            return None;
        }
        let tokens = usage.get("tokens").and_then(json_u64_or_null);
        let percent = match usage.get("percent").and_then(json_f64_or_null) {
            Some(p) => Some(p),
            None => tokens.map(|t| (t as f64 / context_window as f64) * 100.0),
        };
        Some(Self {
            tokens,
            context_window,
            percent,
        })
    }

    /// Fill fraction for a meter, 0..=1. `None` when the estimate is unknown.
    pub fn fraction(&self) -> Option<f32> {
        self.percent.map(|p| (p / 100.0).clamp(0.0, 1.0) as f32)
    }
}

/// Cumulative token and cost totals for the whole session, from
/// `get_session_stats` (`tokens` + `cost`).
///
/// This is deliberately separate from [`ContextUsage`]: `ContextUsage` is a
/// point-in-time snapshot of what currently fills the window, while this is
/// the running total across every assistant turn, tool call, and
/// compaction/branch summary in the session. Cache counters are provider
/// cache reads/writes (`cacheRead` / `cacheWrite`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// pi's `tokens.total`, falling back to the sum of the four buckets when
    /// the field is absent.
    pub total: u64,
    /// Total session cost (USD). `None` when pi omits the field or reports a
    /// non-numeric shape.
    pub cost: Option<f64>,
}

impl SessionUsage {
    /// Parse the `tokens` / `cost` fields of a `get_session_stats` payload.
    /// Returns `None` when pi reports no token totals at all.
    pub fn from_stats(data: &Value) -> Option<Self> {
        let tokens = data.get("tokens")?;
        if tokens.is_null() {
            return None;
        }
        let input = tokens.get("input").and_then(json_u64).unwrap_or(0);
        let output = tokens.get("output").and_then(json_u64).unwrap_or(0);
        let cache_read = tokens.get("cacheRead").and_then(json_u64).unwrap_or(0);
        let cache_write = tokens.get("cacheWrite").and_then(json_u64).unwrap_or(0);
        let total = tokens
            .get("total")
            .and_then(json_u64)
            .unwrap_or_else(|| input + output + cache_read + cache_write);
        let cost = data.get("cost").and_then(json_f64_or_null);
        Some(Self {
            input,
            output,
            cache_read,
            cache_write,
            total,
            cost,
        })
    }

    /// Share of prompt tokens served from the provider cache across the
    /// session: `cacheRead / (input + cacheRead + cacheWrite)`, as `0..=100`.
    /// `None` when no prompt tokens were processed.
    pub fn cache_read_percent(&self) -> Option<f32> {
        cache_hit_rate(self.input, self.cache_read, self.cache_write)
    }
}

/// Token and cost usage for a single assistant message (or tool result), as
/// pi reports it on `message_end` / `turn_end` / `get_messages`.
///
/// `total` uses `totalTokens` when present and otherwise sums the four
/// buckets. `cost` is pi's computed USD cost for the message; `None` when the
/// provider/model has no cost table (local models) or the field is absent.
/// `from_value` returns `None` for absent/null/zero usage so the UI never
/// renders a `↑0 ↓0` row for providers that don't report it.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub cost: Option<f64>,
}

impl MessageUsage {
    /// Parse a message's `usage` value. Handles both the assistant shape
    /// (`cost` as an object) and the `message_update` shape (`totalTokens`).
    pub fn from_value(value: Option<&Value>) -> Option<Self> {
        let usage = value?;
        if usage.is_null() {
            return None;
        }
        let input = usage.get("input").and_then(json_u64).unwrap_or(0);
        let output = usage.get("output").and_then(json_u64).unwrap_or(0);
        let cache_read = usage.get("cacheRead").and_then(json_u64).unwrap_or(0);
        let cache_write = usage.get("cacheWrite").and_then(json_u64).unwrap_or(0);
        let total = usage
            .get("totalTokens")
            .and_then(json_u64)
            .unwrap_or_else(|| input + output + cache_read + cache_write);
        // Per-message cost is `{input, output, cacheRead, cacheWrite, total}`;
        // tolerate a plain number for forward compatibility.
        let cost = usage
            .get("cost")
            .and_then(json_f64_or_null)
            .or_else(|| usage.get("cost")?.get("total").and_then(json_f64));
        // Skip zero usage (streaming `message_update` before the provider
        // reports, or a model with no cost table) so nothing is fabricated.
        if total == 0 && !cost.is_some_and(|c| c > 0.0) {
            return None;
        }
        Some(Self {
            input,
            output,
            cache_read,
            cache_write,
            total,
            cost,
        })
    }

    /// Fold another message's counters into this one (turn aggregation).
    pub fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total += other.total;
        self.cost = match (self.cost, other.cost) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }

    /// Share of prompt tokens served from the provider cache:
    /// `cacheRead / (input + cacheRead + cacheWrite)`, as `0..=100`. `None`
    /// when no prompt tokens were processed, so the UI never shows a
    /// meaningless `0%`.
    pub fn cache_read_percent(&self) -> Option<f32> {
        cache_hit_rate(self.input, self.cache_read, self.cache_write)
    }
}

/// The slice of `get_state` the UI tracks, with pi's own defaults for fields
/// that may be absent. Unknown values fall back rather than failing.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionState {
    pub is_streaming: bool,
    pub is_compacting: bool,
    /// `"all"` | `"one-at-a-time"`.
    pub steering_mode: String,
    /// `"all"` | `"one-at-a-time"`.
    pub follow_up_mode: String,
    pub auto_compaction_enabled: bool,
    pub session_name: Option<String>,
    pub pending_message_count: u64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            is_streaming: false,
            is_compacting: false,
            steering_mode: "one-at-a-time".into(),
            follow_up_mode: "one-at-a-time".into(),
            auto_compaction_enabled: true,
            session_name: None,
            pending_message_count: 0,
        }
    }
}

impl SessionState {
    /// Parse the `data` payload of a `get_state` response. Missing fields keep
    /// their pi default so an older pi that omits them still yields a sane
    /// snapshot.
    pub fn from_value(data: &Value) -> Self {
        let mut state = Self::default();
        if let Some(value) = data.get("isStreaming").and_then(Value::as_bool) {
            state.is_streaming = value;
        }
        if let Some(value) = data.get("isCompacting").and_then(Value::as_bool) {
            state.is_compacting = value;
        }
        if let Some(mode) = data.get("steeringMode").and_then(Value::as_str) {
            state.steering_mode = mode.to_string();
        }
        if let Some(mode) = data.get("followUpMode").and_then(Value::as_str) {
            state.follow_up_mode = mode.to_string();
        }
        if let Some(enabled) = data.get("autoCompactionEnabled").and_then(Value::as_bool) {
            state.auto_compaction_enabled = enabled;
        }
        state.session_name = data
            .get("sessionName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        if let Some(count) = data.get("pendingMessageCount").and_then(json_u64) {
            state.pending_message_count = count;
        }
        state
    }
}

/// The pending steering / follow-up queue from a `queue_update` event or a
/// `clear_queue` response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingQueue {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
}

impl PendingQueue {
    pub fn from_value(value: &Value) -> Self {
        Self {
            steering: string_list(value.get("steering")),
            follow_up: string_list(value.get("followUp")),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_up.is_empty()
    }

    pub fn len(&self) -> usize {
        self.steering.len() + self.follow_up.len()
    }

    /// The queued text joined back together (steering first), for restoring
    /// into the composer after `clear_queue`.
    pub fn restore_text(&self) -> Option<String> {
        let mut parts: Vec<&str> = Vec::with_capacity(self.len());
        parts.extend(self.steering.iter().map(String::as_str));
        parts.extend(self.follow_up.iter().map(String::as_str));
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| value.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
}

fn json_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_u64().map(|n| n as f64))
}

fn json_u64_or_null(value: &Value) -> Option<u64> {
    if value.is_null() {
        None
    } else {
        json_u64(value)
    }
}

fn json_f64_or_null(value: &Value) -> Option<f64> {
    if value.is_null() {
        None
    } else {
        json_f64(value)
    }
}

/// Provider cache hit rate for prompt tokens: `cache_read / (input +
/// cache_read)` scaled to `0..=100`. `None` when the prompt was empty (a turn
/// with no input at all), which keeps the UI from claiming `0%`.
fn cache_hit_rate(input: u64, cache_read: u64, cache_write: u64) -> Option<f32> {
    // pi normalizes every provider so `input` is the *uncached* prompt; the
    // whole prompt is therefore uncached input + cache reads + cache writes.
    let prompt = input + cache_read + cache_write;
    (prompt > 0).then(|| (cache_read as f64 / prompt as f64 * 100.0) as f32)
}

fn trim(s: &str, max: usize) -> String {
    let s = s.replace('\n', "\\n");
    if s.chars().count() > max {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    } else {
        s
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Tests — against the real wire captures from the P0 probe
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_get_state_response() {
        // Captured live from `pi --mode rpc` + get_state.
        let line = r#"{"id":"probe-1","type":"response","command":"get_state","success":true,"data":{"model":{"id":"glm-5.3-flash:cloud","name":"glm-5.3-flash:cloud","api":"openai-completions","provider":"ollama","baseUrl":"http://127.0.0.1:11434/v1","reasoning":true},"thinkingLevel":"high","isStreaming":false,"sessionId":"01a07aa2-d867-753c-98c3-0518c4879639","messageCount":0}}"#;
        let ev = Event::parse_line(line);
        match ev {
            Event::Response {
                id,
                command,
                success,
                data,
                ..
            } => {
                assert_eq!(id, "probe-1");
                assert_eq!(command, "get_state");
                assert!(success);
                let data = data.expect("data present");
                assert_eq!(data["thinkingLevel"], "high");
                assert_eq!(data["model"]["provider"], "ollama");
            }
            other => panic!("expected response, got {other:?}"),
        }
    }

    #[test]
    fn parses_message_update_textdelta() {
        let line = r#"{"type":"message_update","usage":{"input":1,"output":2},"assistantMessageEvent":{"type":"text_delta","delta":"Hello"}}"#;
        match Event::parse_line(line) {
            Event::MessageUpdate {
                assistant: Some(AssistantMessageEvent::TextDelta { delta }),
                ..
            } => {
                assert_eq!(delta, "Hello")
            }
            other => panic!("expected text delta, got {other:?}"),
        }
    }

    #[test]
    fn command_wire_serializes_with_id() {
        let cmd = Command::new(
            "req-1",
            CommandBody::Prompt {
                message: "hello".into(),
                images: None,
                streaming_behavior: None,
            },
        );
        let wire = cmd.to_wire().unwrap();
        // JSON objects are unordered; compare semantics, not key order.
        let parsed: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed["id"], "req-1");
        assert_eq!(parsed["type"], "prompt");
        assert_eq!(parsed["message"], "hello");
        assert_eq!(parsed.get("images"), None);
    }

    #[test]
    fn steer_serializes_with_message() {
        let wire = Command::new(
            "st1",
            CommandBody::Steer {
                message: "use tokio instead".into(),
                images: None,
            },
        )
        .to_wire()
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed["type"], "steer");
        assert_eq!(parsed["message"], "use tokio instead");
        assert!(parsed.get("images").is_none());
    }

    #[test]
    fn steer_and_follow_up_carry_images() {
        let images = vec![serde_json::json!({
            "type": "image",
            "data": "aGk=",
            "mimeType": "image/png"
        })];

        let steer = Command::new(
            "st2",
            CommandBody::Steer {
                message: "look at this".into(),
                images: Some(images.clone()),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&steer).unwrap();
        assert_eq!(parsed["type"], "steer");
        assert_eq!(parsed["images"][0]["mimeType"], "image/png");

        let follow = Command::new(
            "fu1",
            CommandBody::FollowUp {
                message: "after you're done".into(),
                images: Some(images),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&follow).unwrap();
        assert_eq!(parsed["type"], "follow_up");
        assert_eq!(parsed["message"], "after you're done");
        assert_eq!(parsed["images"][0]["data"], "aGk=");
    }

    #[test]
    fn agent_control_commands_serialize_to_pi_wire_types() {
        let steering = Command::new("m1", CommandBody::SetSteeringMode { mode: "all".into() })
            .to_wire()
            .unwrap();
        let parsed: Value = serde_json::from_str(&steering).unwrap();
        assert_eq!(parsed["type"], "set_steering_mode");
        assert_eq!(parsed["mode"], "all");

        let follow = Command::new(
            "m2",
            CommandBody::SetFollowUpMode {
                mode: "one-at-a-time".into(),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&follow).unwrap();
        assert_eq!(parsed["type"], "set_follow_up_mode");
        assert_eq!(parsed["mode"], "one-at-a-time");

        let compact = Command::new(
            "c1",
            CommandBody::Compact {
                custom_instructions: Some("focus on code".into()),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&compact).unwrap();
        assert_eq!(parsed["type"], "compact");
        assert_eq!(parsed["customInstructions"], "focus on code");

        let compact_default = Command::new(
            "c2",
            CommandBody::Compact {
                custom_instructions: None,
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&compact_default).unwrap();
        assert!(parsed.get("customInstructions").is_none());

        let auto_compact = Command::new("a1", CommandBody::SetAutoCompaction { enabled: false })
            .to_wire()
            .unwrap();
        let parsed: Value = serde_json::from_str(&auto_compact).unwrap();
        assert_eq!(parsed["type"], "set_auto_compaction");
        assert_eq!(parsed["enabled"], false);

        let auto_retry = Command::new("a2", CommandBody::SetAutoRetry { enabled: true })
            .to_wire()
            .unwrap();
        let parsed: Value = serde_json::from_str(&auto_retry).unwrap();
        assert_eq!(parsed["type"], "set_auto_retry");
        assert_eq!(parsed["enabled"], true);

        let abort_retry = Command::new("a3", CommandBody::AbortRetry)
            .to_wire()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&abort_retry).unwrap()["type"],
            "abort_retry"
        );

        let rename = Command::new(
            "n1",
            CommandBody::SetSessionName {
                name: "my-feature-work".into(),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&rename).unwrap();
        assert_eq!(parsed["type"], "set_session_name");
        assert_eq!(parsed["name"], "my-feature-work");
    }

    #[test]
    fn fork_family_serializes_like_pi_expects() {
        let fork = Command::new(
            "f1",
            CommandBody::Fork {
                entry_id: "turn-2".into(),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&fork).unwrap();
        assert_eq!(parsed["type"], "fork");
        assert_eq!(parsed["entryId"], "turn-2");

        let clone = Command::new("f2", CommandBody::CloneSession)
            .to_wire()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&clone).unwrap();
        assert_eq!(parsed["type"], "clone");
    }

    #[test]
    fn session_rename_parses_and_trims() {
        assert!(matches!(
            Event::parse_line(r#"{"type":"session_info_changed","name":"  Named by pi  "}"#),
            Event::SessionInfoChanged { name: Some(n) } if n == "Named by pi"
        ));
        assert!(matches!(
            Event::parse_line(r#"{"type":"session_info_changed","name":null}"#),
            Event::SessionInfoChanged { name: None }
        ));
    }

    #[test]
    fn tolerates_unknown_events() {
        let ev = Event::parse_line(r#"{"type":"brand_new_shape","thing":1}"#);
        assert!(matches!(ev, Event::Unknown(_)));
    }

    #[test]
    fn framing_strips_carriage_return() {
        let line = "{\"type\":\"agent_start\"}\r";
        assert!(matches!(Event::parse_line(line), Event::AgentStart));
    }

    #[test]
    fn get_session_stats_serializes() {
        let wire = Command::new("s1", CommandBody::GetSessionStats)
            .to_wire()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed["id"], "s1");
        assert_eq!(parsed["type"], "get_session_stats");
    }

    #[test]
    fn context_usage_from_stats_payload() {
        let data: serde_json::Value = serde_json::from_str(
            r#"{"contextUsage":{"tokens":60000,"contextWindow":200000,"percent":30}}"#,
        )
        .unwrap();
        let usage = ContextUsage::from_stats(&data).expect("contextUsage");
        assert_eq!(usage.tokens, Some(60_000));
        assert_eq!(usage.context_window, 200_000);
        assert_eq!(usage.percent, Some(30.0));
        assert!((usage.fraction().unwrap() - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn context_usage_null_after_compaction() {
        let data: serde_json::Value = serde_json::from_str(
            r#"{"contextUsage":{"tokens":null,"contextWindow":200000,"percent":null}}"#,
        )
        .unwrap();
        let usage = ContextUsage::from_stats(&data).expect("window still known");
        assert_eq!(usage.tokens, None);
        assert_eq!(usage.percent, None);
        assert_eq!(usage.fraction(), None);
    }

    #[test]
    fn context_usage_omitted_without_model() {
        let data = serde_json::json!({"tokens":{"total":10}});
        assert!(ContextUsage::from_stats(&data).is_none());
    }

    #[test]
    fn session_usage_parses_full_get_session_stats_payload() {
        // Shape from the RPC docs' `get_session_stats` example.
        let data: Value = serde_json::from_str(
            r#"{"tokens":{"input":50000,"output":10000,"cacheRead":40000,
                "cacheWrite":5000,"total":105000},"cost":0.45,
                "contextUsage":{"tokens":60000,"contextWindow":200000,"percent":30}}"#,
        )
        .unwrap();
        let usage = SessionUsage::from_stats(&data).expect("session usage");
        assert_eq!(usage.input, 50_000);
        assert_eq!(usage.output, 10_000);
        assert_eq!(usage.cache_read, 40_000);
        assert_eq!(usage.cache_write, 5_000);
        assert_eq!(usage.total, 105_000);
        assert_eq!(usage.cost, Some(0.45));
    }

    #[test]
    fn session_usage_total_falls_back_to_bucket_sum() {
        let data = serde_json::json!({
            "tokens": {"input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 1}
        });
        let usage = SessionUsage::from_stats(&data).expect("session usage");
        assert_eq!(usage.total, 18);
        assert_eq!(usage.cost, None);
    }

    #[test]
    fn session_usage_absent_without_tokens() {
        assert!(SessionUsage::from_stats(&serde_json::json!({"cost": 0.1})).is_none());
        assert!(SessionUsage::from_stats(&serde_json::json!({"tokens": null})).is_none());
    }

    #[test]
    fn message_usage_parses_assistant_shape_with_cost_object() {
        // AssistantMessage.usage from the RPC docs.
        let usage: Value = serde_json::from_str(
            r#"{"input":100,"output":50,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0003,"output":0.00075,"cacheRead":0,
                "cacheWrite":0,"total":0.00105}}"#,
        )
        .unwrap();
        let parsed = MessageUsage::from_value(Some(&usage)).expect("usage");
        assert_eq!(parsed.input, 100);
        assert_eq!(parsed.output, 50);
        assert_eq!(parsed.total, 150);
        assert_eq!(parsed.cost, Some(0.00105));
    }

    #[test]
    fn message_usage_prefers_total_tokens_and_sums() {
        let with_total = serde_json::json!({
            "input": 10, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 101
        });
        let parsed = MessageUsage::from_value(Some(&with_total)).expect("usage");
        assert_eq!(parsed.total, 101);

        let without_total = serde_json::json!({
            "input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 1
        });
        let parsed = MessageUsage::from_value(Some(&without_total)).expect("usage");
        assert_eq!(parsed.total, 18);
        assert_eq!(parsed.cost, None);
    }

    #[test]
    fn message_usage_ignores_zero_and_missing() {
        assert!(MessageUsage::from_value(None).is_none());
        assert!(MessageUsage::from_value(Some(&Value::Null)).is_none());
        let zero = serde_json::json!({
            "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0
        });
        assert!(MessageUsage::from_value(Some(&zero)).is_none());
    }

    #[test]
    fn message_usage_add_folds_turn_steps() {
        let mut a = MessageUsage {
            input: 10,
            output: 5,
            cache_read: 1,
            cache_write: 2,
            total: 18,
            cost: Some(0.01),
        };
        let b = MessageUsage {
            input: 20,
            output: 8,
            cache_read: 3,
            cache_write: 4,
            total: 35,
            cost: None,
        };
        a.add(&b);
        assert_eq!(a.input, 30);
        assert_eq!(a.output, 13);
        assert_eq!(a.cache_read, 4);
        assert_eq!(a.cache_write, 6);
        assert_eq!(a.total, 53);
        assert_eq!(a.cost, Some(0.01));
    }

    #[test]
    fn cache_read_percent_is_hit_rate_over_prompt_tokens() {
        let cached = MessageUsage {
            input: 10,
            output: 5,
            cache_read: 90,
            cache_write: 0,
            total: 105,
            cost: None,
        };
        assert_eq!(cached.cache_read_percent(), Some(90.0));

        let miss = MessageUsage {
            input: 100,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            total: 105,
            cost: None,
        };
        assert_eq!(miss.cache_read_percent(), Some(0.0));

        let empty = MessageUsage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            total: 0,
            cost: None,
        };
        assert_eq!(empty.cache_read_percent(), None);

        let session = SessionUsage {
            input: 50,
            output: 0,
            cache_read: 50,
            cache_write: 0,
            total: 100,
            cost: None,
        };
        assert_eq!(session.cache_read_percent(), Some(50.0));

        // Cache writes are prompt tokens too: reading a fully written cache
        // back is a 50% hit rate, not 100%.
        let written = MessageUsage {
            input: 0,
            output: 0,
            cache_read: 100,
            cache_write: 100,
            total: 200,
            cost: None,
        };
        assert_eq!(written.cache_read_percent(), Some(50.0));
    }

    #[test]
    fn session_state_parses_agent_control_fields() {
        let data: Value = serde_json::from_str(
            r#"{"isStreaming":true,"isCompacting":false,"steeringMode":"all",
               "followUpMode":"one-at-a-time","autoCompactionEnabled":false,
               "sessionName":"  my-feature-work  ","pendingMessageCount":2}"#,
        )
        .unwrap();
        let state = SessionState::from_value(&data);
        assert!(state.is_streaming);
        assert!(!state.is_compacting);
        assert_eq!(state.steering_mode, "all");
        assert_eq!(state.follow_up_mode, "one-at-a-time");
        assert!(!state.auto_compaction_enabled);
        assert_eq!(state.session_name.as_deref(), Some("my-feature-work"));
        assert_eq!(state.pending_message_count, 2);

        // An older/simpler payload keeps pi's defaults.
        let sparse = SessionState::from_value(&serde_json::json!({"sessionId":"x"}));
        assert_eq!(sparse.steering_mode, "one-at-a-time");
        assert!(sparse.auto_compaction_enabled);
        assert!(sparse.session_name.is_none());
        assert_eq!(sparse.pending_message_count, 0);
    }

    #[test]
    fn pending_queue_parses_and_restores_text() {
        let value = serde_json::json!({
            "steering": ["Focus on error handling"],
            "followUp": ["After that, summarize"]
        });
        let queue = PendingQueue::from_value(&value);
        assert_eq!(queue.steering, vec!["Focus on error handling"]);
        assert_eq!(queue.follow_up, vec!["After that, summarize"]);
        assert_eq!(queue.len(), 2);
        assert!(!queue.is_empty());
        assert_eq!(
            queue.restore_text().as_deref(),
            Some("Focus on error handling\n\nAfter that, summarize")
        );
        assert!(PendingQueue::default().restore_text().is_none());
    }

    // ── provider authentication protocol ───────────────────────────────

    #[test]
    fn auth_commands_serialize_to_pi_wire_types() {
        let list = Command::new("a1", CommandBody::AuthList).to_wire().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&list).unwrap()["type"],
            "auth.list"
        );

        let status = Command::new(
            "a2",
            CommandBody::AuthStatus {
                provider: "anthropic".into(),
            },
        )
        .to_wire()
        .unwrap();
        let status: Value = serde_json::from_str(&status).unwrap();
        assert_eq!(status["type"], "auth.status");
        assert_eq!(status["provider"], "anthropic");

        let login = Command::new(
            "a3",
            CommandBody::AuthLogin {
                provider: "openai-codex".into(),
                method: "device_code".into(),
                session_id: Some("orbit-login-1".into()),
            },
        )
        .to_wire()
        .unwrap();
        let login: Value = serde_json::from_str(&login).unwrap();
        assert_eq!(login["type"], "auth.login");
        assert_eq!(login["provider"], "openai-codex");
        assert_eq!(login["method"], "device_code");
        assert_eq!(login["sessionId"], "orbit-login-1");

        let login_without_session = Command::new(
            "a4",
            CommandBody::AuthLogin {
                provider: "anthropic".into(),
                method: "browser".into(),
                session_id: None,
            },
        )
        .to_wire()
        .unwrap();
        let login_without_session: Value = serde_json::from_str(&login_without_session).unwrap();
        assert!(login_without_session.get("sessionId").is_none());

        let logout = Command::new(
            "a5",
            CommandBody::AuthLogout {
                provider: "anthropic".into(),
            },
        )
        .to_wire()
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&logout).unwrap()["type"],
            "auth.logout"
        );

        let cancel = Command::new(
            "a6",
            CommandBody::AuthCancel {
                session_id: "orbit-login-1".into(),
            },
        )
        .to_wire()
        .unwrap();
        let cancel: Value = serde_json::from_str(&cancel).unwrap();
        assert_eq!(cancel["type"], "auth.cancel");
        assert_eq!(cancel["sessionId"], "orbit-login-1");
    }

    #[test]
    fn parses_auth_login_lifecycle_events() {
        let started = Event::parse_line(
            r#"{"type":"auth_login_started","sessionId":"s1","provider":"anthropic","method":"browser","expiresAt":1700000000000}"#,
        );
        match started {
            Event::Auth(AuthEvent::LoginStarted {
                session_id,
                provider,
                method,
                expires_at,
            }) => {
                assert_eq!(session_id, "s1");
                assert_eq!(provider, "anthropic");
                assert_eq!(method, "browser");
                assert_eq!(expires_at, Some(1_700_000_000_000));
            }
            other => panic!("expected login started, got {other:?}"),
        }

        let url = Event::parse_line(
            r#"{"type":"auth_login_url","sessionId":"s1","provider":"anthropic","url":"https://claude.ai/oauth?code=1"}"#,
        );
        match url {
            Event::Auth(AuthEvent::LoginUrl { url, .. }) => {
                assert!(url.starts_with("https://claude.ai/oauth"))
            }
            other => panic!("expected login url, got {other:?}"),
        }

        let device = Event::parse_line(
            r#"{"type":"auth_device_code","sessionId":"s2","provider":"openai-codex","userCode":"ABCD-1234","verificationUri":"https://auth.openai.com/device","verificationUriComplete":"https://auth.openai.com/device?code=ABCD-1234"}"#,
        );
        match device {
            Event::Auth(AuthEvent::DeviceCode {
                user_code,
                verification_uri,
                verification_uri_complete,
                ..
            }) => {
                assert_eq!(user_code, "ABCD-1234");
                assert_eq!(verification_uri, "https://auth.openai.com/device");
                assert_eq!(
                    verification_uri_complete.as_deref(),
                    Some("https://auth.openai.com/device?code=ABCD-1234")
                );
            }
            other => panic!("expected device code, got {other:?}"),
        }

        let succeeded = Event::parse_line(
            r#"{"type":"auth_login_succeeded","sessionId":"s1","provider":"anthropic","method":"browser","credential":"oauth","expiresAt":1700003600000}"#,
        );
        match succeeded {
            Event::Auth(AuthEvent::LoginSucceeded {
                provider,
                credential,
                ..
            }) => {
                assert_eq!(provider, "anthropic");
                assert_eq!(credential.as_deref(), Some("oauth"));
            }
            other => panic!("expected login succeeded, got {other:?}"),
        }

        let failed = Event::parse_line(
            r#"{"type":"auth_login_failed","sessionId":"s3","provider":"xai","code":"timeout","message":"expired"}"#,
        );
        match failed {
            Event::Auth(AuthEvent::LoginFailed {
                session_id,
                code,
                message,
                ..
            }) => {
                assert_eq!(session_id, "s3");
                assert_eq!(code, AuthErrorCode::Timeout);
                assert_eq!(message, "expired");
            }
            other => panic!("expected login failed, got {other:?}"),
        }

        let cancelled = Event::parse_line(
            r#"{"type":"auth_login_cancelled","sessionId":"s4","provider":"xai"}"#,
        );
        assert!(matches!(
            cancelled,
            Event::Auth(AuthEvent::LoginCancelled { session_id, .. }) if session_id == "s4"
        ));

        let changed = Event::parse_line(
            r#"{"type":"auth_credentials_changed","providers":["anthropic","openai"]}"#,
        );
        match changed {
            Event::Auth(AuthEvent::CredentialsChanged { providers }) => {
                assert_eq!(providers, vec!["anthropic", "openai"]);
            }
            other => panic!("expected credentials changed, got {other:?}"),
        }
    }

    #[test]
    fn unknown_auth_event_is_forward_compatible() {
        let ev = Event::parse_line(r#"{"type":"auth_quantum_entangled","sessionId":"s9"}"#);
        match ev {
            Event::Auth(AuthEvent::Other { kind, .. }) => {
                assert_eq!(kind, "auth_quantum_entangled")
            }
            other => panic!("expected auth other, got {other:?}"),
        }
    }

    #[test]
    fn auth_error_codes_round_trip_and_default_to_unknown() {
        for code in [
            "invalid_provider",
            "unsupported_method",
            "already_in_progress",
            "not_authenticated",
            "unsupported",
            "login_failed",
            "timeout",
            "cancelled",
            "network_error",
            "storage_error",
            "internal_error",
        ] {
            assert_eq!(AuthErrorCode::from_code(code).as_str(), code, "{code}");
        }
        assert_eq!(
            AuthErrorCode::from_code("some_future_code"),
            AuthErrorCode::Unknown
        );
        // pi has historically spelled cancel both ways.
        assert_eq!(
            AuthErrorCode::from_code("canceled"),
            AuthErrorCode::Cancelled
        );
    }

    #[test]
    fn parses_auth_list_provider_capabilities() {
        let raw = r#"{"providers":[
            {"id":"anthropic","name":"Anthropic","credential":"oauth","authenticated":true,
             "methods":[{"id":"browser","label":"Sign in with browser"},{"id":"api_key","label":"API key"}]},
            {"id":"openai-codex","name":"ChatGPT (Codex)","credential":"none","authenticated":false,
             "methods":[{"id":"device_code","label":"Device code"}]}
        ]}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let providers: Vec<AuthProvider> = value["providers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(AuthProvider::from_value)
            .collect();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].id, "anthropic");
        assert!(providers[0].authenticated);
        assert!(providers[0].supports_oauth());
        assert!(providers[0].supports_api_key());
        assert_eq!(providers[0].methods[0].label, "Sign in with browser");
        assert!(!providers[1].authenticated);
        assert_eq!(
            providers[1].device_code_method().map(|m| m.id.as_str()),
            Some("device_code")
        );
        // Missing name/credential fields degrade safely.
        let sparse = AuthProvider::from_value(&serde_json::json!({"id":"x"})).unwrap();
        assert_eq!(sparse.name, "x");
        assert_eq!(sparse.credential, "none");
        assert!(sparse.methods.is_empty());
    }

    #[test]
    fn auth_event_session_ids_are_extractable() {
        let ev = AuthEvent::LoginFailed {
            session_id: "abc".into(),
            provider: "x".into(),
            code: AuthErrorCode::LoginFailed,
            message: "nope".into(),
        };
        assert_eq!(ev.session_id(), Some("abc"));
        let changed = AuthEvent::CredentialsChanged {
            providers: vec!["x".into()],
        };
        assert_eq!(changed.session_id(), None);
    }

    #[test]
    fn quota_list_serializes_with_optional_provider() {
        let all = Command::new("q1", CommandBody::QuotaList { provider: None })
            .to_wire()
            .unwrap();
        let parsed: Value = serde_json::from_str(&all).unwrap();
        assert_eq!(parsed["type"], "quota.list");
        assert!(parsed.get("provider").is_none());

        let one = Command::new(
            "q2",
            CommandBody::QuotaList {
                provider: Some("anthropic".into()),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&one).unwrap();
        assert_eq!(parsed["type"], "quota.list");
        assert_eq!(parsed["provider"], "anthropic");
    }

    #[test]
    fn get_entries_serializes_with_optional_cursor() {
        let all = Command::new("e1", CommandBody::GetEntries { since: None })
            .to_wire()
            .unwrap();
        let parsed: Value = serde_json::from_str(&all).unwrap();
        assert_eq!(parsed["type"], "get_entries");
        assert!(parsed.get("since").is_none());

        let incremental = Command::new(
            "e2",
            CommandBody::GetEntries {
                since: Some("abc123".into()),
            },
        )
        .to_wire()
        .unwrap();
        let parsed: Value = serde_json::from_str(&incremental).unwrap();
        assert_eq!(parsed["type"], "get_entries");
        assert_eq!(parsed["since"], "abc123");
    }

    #[test]
    fn parses_quota_reports_with_windows_and_balances() {
        let raw = r#"{"providers":[
            {"provider":"anthropic","kind":"subscription","plan":"Max",
             "windows":[
               {"id":"five_hour","label":"5-hour","usedPercent":6.0,"resetsAt":1738300000000},
               {"id":"weekly","label":"Weekly","used":42,"limit":100,"unit":"requests"}
             ],
             "balances":[],
             "fetchedAt":1738290000000},
            {"provider":"deepseek","kind":"balance","windows":[],
             "balances":[{"label":"Available","amount":110.0,"currency":"CNY"}]},
            {"provider":"groq","kind":"unsupported","note":"no usage API"}
        ]}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let reports = parse_quota_reports(&value);
        assert_eq!(reports.len(), 3);

        let anthropic = &reports[0];
        assert_eq!(anthropic.kind, QuotaKind::Subscription);
        assert_eq!(anthropic.plan.as_deref(), Some("Max"));
        assert_eq!(anthropic.windows.len(), 2);
        assert_eq!(anthropic.windows[0].used_percent, Some(6.0));
        assert!((anthropic.windows[0].fraction().unwrap() - 0.06).abs() < 1e-6);
        assert_eq!(anthropic.windows[1].fraction(), Some(0.42));
        assert_eq!(anthropic.windows[0].resets_at, Some(1_738_300_000_000));

        let deepseek = &reports[1];
        assert_eq!(deepseek.kind, QuotaKind::Balance);
        assert_eq!(deepseek.balances[0].amount, 110.0);
        assert_eq!(deepseek.balances[0].currency, "CNY");

        assert!(!reports[2].has_data());
        assert_eq!(reports[2].note.as_deref(), Some("no usage API"));
    }

    #[test]
    fn canonicalizes_lowercase_plan_labels() {
        // Ollama's API reports `pro`; its settings page reports `Pro`. The
        // badge must read the same either way, and acronyms must survive.
        for (raw, expected) in [
            ("pro", "Pro"),
            ("max", "Max"),
            ("Pro", "Pro"),
            ("ChatGPT Plus", "ChatGPT Plus"),
        ] {
            let value: Value = serde_json::from_str(&format!(
                r#"{{"providers":[{{"provider":"ollama","kind":"subscription","plan":"{raw}","windows":[]}}]}}"#
            ))
            .unwrap();
            let reports = parse_quota_reports(&value);
            assert_eq!(reports[0].plan.as_deref(), Some(expected), "raw: {raw}");
        }
    }
}
