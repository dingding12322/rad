//! Provider-authentication state for Settings → Providers.
//!
//! Orbit never implements a provider OAuth flow and never sees a credential.
//! The pi RPC server owns the login (it reuses pi's provider auth and
//! `~/.pi/agent/auth.json`); this module is a **non-sensitive** reducer over
//! the `auth.*` command responses and events:
//!
//! - `auth.list` discovers which providers exist and which methods each one
//!   offers, so the UI never hardcodes provider ids or login flows.
//! - `auth.login` starts a session; the client generates the login session id
//!   so it can cancel deterministically and reconcile after a restart.
//! - `auth.*` events drive the card through Connecting / Device code /
//!   Success / Error / Cancelled.
//! - `auth.status` and `auth.logout` keep the stored-credential state honest.
//!
//! The reducer is pure: it touches no sockets and performs no I/O. It emits
//! [`AuthEffect`] values that the GPUI layer turns into OS actions (open the
//! browser) or RPC commands (`auth.cancel`). That keeps every state machine
//! testable without a window, a process, or a network.
//!
//! **Secret boundary:** the only auth material that ever appears here is
//! public display data — a device code, a verification URL, an expiry, an
//! account label, and the *kind* of credential (`oauth` / `api_key`). Access
//! and refresh tokens are never parsed, stored, or rendered.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use orbit_rpc::{AuthErrorCode, AuthEvent, AuthProvider};

/// How long a login may stay pending when pi does not report an expiry.
pub const DEFAULT_LOGIN_TTL: Duration = Duration::from_secs(300);
/// How long a terminal outcome stays on the card before auto-dismissing.
pub const DEFAULT_RESULT_TTL: Duration = Duration::from_secs(8);

/// Tunable deadlines (tests use short values for determinism).
#[derive(Debug, Clone, Copy)]
pub struct AuthTtls {
    pub login: Duration,
    pub result: Duration,
}

impl Default for AuthTtls {
    fn default() -> Self {
        Self {
            login: DEFAULT_LOGIN_TTL,
            result: DEFAULT_RESULT_TTL,
        }
    }
}

/// Whether pi exposes the auth RPC namespace at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSupport {
    /// Not probed yet (or the process restarted; probe again).
    Unknown,
    /// `auth.list` answered successfully.
    Supported,
    /// pi rejected `auth.list`; fall back to the Terminal login path.
    Unsupported,
}

/// A device-authorization challenge to display. Not a secret: it is meant to
/// be shown to the user and pasted into a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
}

/// Live credential facts for one provider (`auth.list` / `auth.status`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderStatus {
    pub authenticated: bool,
    /// `oauth`, `api_key`, or empty when unknown.
    pub credential: String,
    /// Absolute expiry in epoch milliseconds, when the server reports one.
    pub expires_at: Option<i64>,
    pub account: Option<String>,
}

/// Login lifecycle phase rendered by the provider card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginPhase {
    /// Request sent, pi has not acknowledged yet.
    Connecting,
    /// Browser flow: URL opened, waiting for the redirect.
    AwaitingBrowser,
    /// Device-code flow: code shown, waiting for authorization.
    AwaitingDeviceCode,
    /// Completed successfully.
    Succeeded,
    /// Failed with a structured error.
    Error,
    /// Cancelled by the user, a timeout, or a restart.
    Cancelled,
}

impl LoginPhase {
    /// Terminal phases are retained briefly (or until dismissed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Error | Self::Cancelled)
    }
}

/// One in-flight (or recently finished) login session.
#[derive(Debug, Clone)]
pub struct LoginSession {
    pub id: String,
    pub provider: String,
    pub method: String,
    pub phase: LoginPhase,
    /// Browser authorization URL, once known.
    pub url: Option<String>,
    /// Device-code challenge, once known.
    pub device_code: Option<DeviceCode>,
    /// Optional progress note from pi.
    pub message: Option<String>,
    /// Structured failure.
    pub error: Option<(AuthErrorCode, String)>,
    /// Absolute deadline for expiry (login TTL, or pi's `expiresAt`).
    pub deadline: Option<Instant>,
    /// When a terminal phase was reached (drives the result TTL).
    pub finished_at: Option<Instant>,
}

/// A side effect the UI must perform. `AuthManager` never does I/O itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEffect {
    /// Open a URL using the OS default browser.
    OpenUrl(String),
    /// Send `auth.cancel` for this login session id.
    CancelLogin(String),
    /// Re-run `auth.list` (credentials or discovered providers changed).
    RefreshProviders,
}

/// The non-sensitive authentication reducer.
pub struct AuthManager {
    support: AuthSupport,
    providers: Vec<AuthProvider>,
    statuses: HashMap<String, ProviderStatus>,
    login: Option<LoginSession>,
    /// A provider whose login was interrupted by a disconnect; reconciled
    /// against fresh `auth.list`/`auth.status` data on reconnect.
    recovering: Option<String>,
    seq: u64,
    ttls: AuthTtls,
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthManager {
    pub fn new() -> Self {
        Self::with_ttls(AuthTtls::default())
    }

    pub fn with_ttls(ttls: AuthTtls) -> Self {
        Self {
            support: AuthSupport::Unknown,
            providers: Vec::new(),
            statuses: HashMap::new(),
            login: None,
            recovering: None,
            seq: 0,
            ttls,
        }
    }

    // ── reads (the UI renders from these) ──────────────────────────────

    pub fn support(&self) -> AuthSupport {
        self.support
    }

    pub fn providers(&self) -> &[AuthProvider] {
        &self.providers
    }

    pub fn provider(&self, id: &str) -> Option<&AuthProvider> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    pub fn status(&self, id: &str) -> Option<&ProviderStatus> {
        self.statuses.get(id)
    }

    pub fn login(&self) -> Option<&LoginSession> {
        self.login.as_ref()
    }

    /// The login session for `provider`, if one is active or just finished.
    pub fn login_for(&self, provider: &str) -> Option<&LoginSession> {
        self.login
            .as_ref()
            .filter(|session| session.provider == provider)
    }

    /// True while a login is actually waiting on the user.
    pub fn is_busy(&self) -> bool {
        self.login
            .as_ref()
            .is_some_and(|session| !session.phase.is_terminal())
    }

    // ── requests (the app builds RPC commands from these) ──────────────

    /// Start a login session and return its client-generated id. The caller
    /// sends `auth.login { provider, method, sessionId }`.
    pub fn start_login(&mut self, provider: &str, method: &str) -> String {
        let id = self.next_session_id();
        self.login = Some(LoginSession {
            id: id.clone(),
            provider: provider.to_string(),
            method: method.to_string(),
            phase: LoginPhase::Connecting,
            url: None,
            device_code: None,
            message: None,
            error: None,
            deadline: Some(Instant::now() + self.ttls.login),
            finished_at: None,
        });
        id
    }

    /// Cancel the active login. Returns the session id to send in
    /// `auth.cancel`, or `None` when there is nothing to cancel.
    pub fn cancel_login(&mut self) -> Option<String> {
        let session = self.login.as_mut()?;
        if session.phase.is_terminal() {
            // Dismiss the card instead of cancelling an already-finished login.
            self.login = None;
            return None;
        }
        let id = session.id.clone();
        session.phase = LoginPhase::Cancelled;
        session.deadline = None;
        session.finished_at = Some(Instant::now());
        Some(id)
    }

    /// Dismiss a finished login card without touching pi.
    pub fn dismiss_result(&mut self) {
        if self
            .login
            .as_ref()
            .is_some_and(|session| session.phase.is_terminal())
        {
            self.login = None;
        }
    }

    // ── responses ──────────────────────────────────────────────────────

    /// Apply the `auth.list` response.
    pub fn on_list_response(
        &mut self,
        success: bool,
        data: Option<&serde_json::Value>,
        error: Option<&str>,
    ) {
        if !success {
            let message = error.unwrap_or("auth.list failed");
            if is_unsupported_error(message) {
                self.support = AuthSupport::Unsupported;
            }
            return;
        }
        self.support = AuthSupport::Supported;
        let providers = data
            .and_then(|data| data.get("providers"))
            .and_then(serde_json::Value::as_array)
            .map(|providers| {
                providers
                    .iter()
                    .filter_map(AuthProvider::from_value)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Merge into the status map so `auth.status` detail survives a list
        // refresh while the capability list itself is authoritative.
        for provider in &providers {
            let status = self.statuses.entry(provider.id.clone()).or_default();
            status.authenticated = provider.authenticated;
            if !provider.credential.is_empty() {
                status.credential = provider.credential.clone();
            }
        }
        self.providers = providers;
        self.reconcile_recovery();
    }

    /// Apply the `auth.status` response for one provider.
    pub fn on_status_response(&mut self, success: bool, data: Option<&serde_json::Value>) {
        if !success {
            return;
        }
        let Some(status) = data
            .and_then(|data| data.get("provider").or(Some(data)))
            .and_then(|value| value.as_object().map(|_| value))
        else {
            return;
        };
        let Some(id) = status.get("id").and_then(serde_json::Value::as_str) else {
            return;
        };
        let mut parsed = ProviderStatus {
            authenticated: status
                .get("authenticated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            credential: status
                .get("credential")
                .or_else(|| status.get("credentialKind"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("none")
                .to_string(),
            expires_at: status.get("expiresAt").and_then(serde_json::Value::as_i64),
            account: status
                .get("account")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        };
        // `auth.status` without `authenticated` infers from the credential.
        if status.get("authenticated").is_none() {
            parsed.authenticated = !parsed.credential.is_empty() && parsed.credential != "none";
        }
        self.statuses.insert(id.to_string(), parsed);
        self.reconcile_recovery();
    }

    /// Apply the `auth.login` acknowledgement.
    pub fn on_login_response(
        &mut self,
        success: bool,
        data: Option<&serde_json::Value>,
        error: Option<&str>,
    ) {
        if !success {
            let message = error.unwrap_or(&tr!("auth.login_failed")).to_string();
            if let Some(session) = self.login.as_mut() {
                session.phase = LoginPhase::Error;
                session.deadline = None;
                session.finished_at = Some(Instant::now());
                session.error = Some((classify_error(&message), message));
            }
            return;
        }
        // pi may confirm with the session id; adopt it if ours differs so
        // subsequent events still correlate.
        if let Some(server_id) = data
            .and_then(|data| data.get("sessionId"))
            .and_then(serde_json::Value::as_str)
        {
            if let Some(session) = self.login.as_mut() {
                session.id = server_id.to_string();
            }
        }
        if let Some(session) = self.login.as_mut() {
            if session.phase == LoginPhase::Connecting {
                session.phase = awaiting_phase(&session.method);
            }
        }
    }

    /// Apply the `auth.logout` response.
    pub fn on_logout_response(&mut self, success: bool, provider: &str) {
        if !success {
            return;
        }
        self.statuses.insert(
            provider.to_string(),
            ProviderStatus {
                authenticated: false,
                credential: "none".into(),
                expires_at: None,
                account: None,
            },
        );
        // A logout ends any login for the same provider.
        if self
            .login
            .as_ref()
            .is_some_and(|session| session.provider == provider && !session.phase.is_terminal())
        {
            self.dismiss_result();
            self.login = None;
        }
    }

    // ── asynchronous events ────────────────────────────────────────────

    /// Apply one `auth.*` event. Returns effects to perform.
    pub fn on_event(&mut self, event: AuthEvent) -> Vec<AuthEffect> {
        let mut effects = Vec::new();
        match event {
            AuthEvent::LoginStarted {
                session_id,
                provider,
                method,
                expires_at,
            } => {
                let deadline = self.deadline(expires_at);
                if let Some(session) = self.session_mut(&session_id) {
                    session.method = method;
                    if !session.phase.is_terminal() {
                        session.phase = awaiting_phase(&session.method);
                    }
                    session.deadline = Some(deadline);
                } else {
                    // Started event raced ahead of our bookkeeping; adopt it.
                    self.login = Some(LoginSession {
                        id: session_id,
                        provider,
                        phase: awaiting_phase(&method),
                        method,
                        url: None,
                        device_code: None,
                        message: None,
                        error: None,
                        deadline: Some(deadline),
                        finished_at: None,
                    });
                }
            }
            AuthEvent::LoginUrl {
                session_id, url, ..
            } => {
                if let Some(session) = self.session_mut(&session_id) {
                    session.url = Some(url.clone());
                    if !session.phase.is_terminal() {
                        session.phase = LoginPhase::AwaitingBrowser;
                    }
                }
                if !url.is_empty() {
                    effects.push(AuthEffect::OpenUrl(url));
                }
            }
            AuthEvent::DeviceCode {
                session_id,
                user_code,
                verification_uri,
                verification_uri_complete,
                expires_at,
                ..
            } => {
                let deadline = self.deadline(expires_at);
                if let Some(session) = self.session_mut(&session_id) {
                    session.device_code = Some(DeviceCode {
                        user_code,
                        verification_uri,
                        verification_uri_complete,
                    });
                    if !session.phase.is_terminal() {
                        session.phase = LoginPhase::AwaitingDeviceCode;
                    }
                    session.deadline = Some(deadline);
                }
            }
            AuthEvent::LoginWaiting {
                session_id,
                message,
                ..
            } => {
                if let Some(session) = self.session_mut(&session_id) {
                    session.message = message;
                }
            }
            AuthEvent::LoginSucceeded {
                session_id,
                provider,
                credential,
                expires_at,
                ..
            } => {
                self.finish(&session_id, LoginPhase::Succeeded, None);
                let kind = credential.unwrap_or_else(|| "oauth".to_string());
                self.statuses.insert(
                    provider.clone(),
                    ProviderStatus {
                        authenticated: true,
                        credential: kind,
                        expires_at,
                        account: None,
                    },
                );
                if let Some(capability) =
                    self.providers.iter_mut().find(|entry| entry.id == provider)
                {
                    capability.authenticated = true;
                    capability.credential = self
                        .statuses
                        .get(&provider)
                        .map(|status| status.credential.clone())
                        .unwrap_or_default();
                }
                effects.push(AuthEffect::RefreshProviders);
            }
            AuthEvent::LoginFailed {
                session_id,
                code,
                message,
                ..
            } => {
                self.finish(&session_id, LoginPhase::Error, Some((code, message)));
            }
            AuthEvent::LoginCancelled { session_id, .. } => {
                self.finish(&session_id, LoginPhase::Cancelled, None);
            }
            AuthEvent::CredentialsChanged { .. } => {
                effects.push(AuthEffect::RefreshProviders);
            }
            AuthEvent::Other { .. } => {}
        }
        effects
    }

    // ── lifecycle ──────────────────────────────────────────────────────

    /// The pi process went away (exit, restart, or stop). Drop in-flight
    /// logins, forget the capability probe, and remember a provider to
    /// reconcile once a fresh process answers `auth.list`.
    pub fn on_disconnect(&mut self) {
        if let Some(session) = self.login.as_mut() {
            if !session.phase.is_terminal() {
                self.recovering = Some(session.provider.clone());
                session.phase = LoginPhase::Cancelled;
                session.deadline = None;
                session.finished_at = Some(Instant::now());
                session.message = Some(tr!("auth.interrupted_by_restart"));
            }
        }
        self.support = AuthSupport::Unknown;
        self.providers.clear();
        // Keep `statuses` so the card can still show the last known state
        // until the next `auth.list` refreshes it.
    }

    /// Force a status re-query decision after a reconnect. Returns the
    /// providers whose status should be refreshed eagerly.
    pub fn on_reconnect(&mut self) -> Vec<String> {
        self.support = AuthSupport::Unknown;
        let mut wanted: Vec<String> = Vec::new();
        if let Some(provider) = self.recovering.clone() {
            wanted.push(provider);
        }
        wanted
    }

    /// Advance time: expire a stale login and auto-dismiss finished results.
    /// Returns effects (a timeout emits `auth.cancel`).
    pub fn poll(&mut self, now: Instant) -> Vec<AuthEffect> {
        let mut effects = Vec::new();
        let Some(session) = self.login.as_mut() else {
            return effects;
        };
        if session.phase.is_terminal() {
            let expired = session
                .finished_at
                .is_some_and(|at| now.duration_since(at) >= self.ttls.result);
            if expired {
                self.login = None;
            }
            return effects;
        }
        if session.deadline.is_some_and(|deadline| now >= deadline) {
            session.phase = LoginPhase::Error;
            session.deadline = None;
            session.finished_at = Some(now);
            session.error = Some((AuthErrorCode::Timeout, tr!("auth.sign_in_timed_out")));
            effects.push(AuthEffect::CancelLogin(session.id.clone()));
        }
        effects
    }

    // ── internals ──────────────────────────────────────────────────────

    fn next_session_id(&mut self) -> String {
        self.seq += 1;
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("orbit-auth-{ms:x}-{:x}", self.seq)
    }

    fn deadline(&self, expires_at: Option<i64>) -> Instant {
        let now_instant = Instant::now();
        let Some(epoch_ms) = expires_at else {
            return now_instant + self.ttls.login;
        };
        let now_epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let remaining_ms = epoch_ms.saturating_sub(now_epoch_ms).max(0) as u64;
        let remaining = Duration::from_millis(remaining_ms);
        // Never let a bogus far-future value pin the session forever, and
        // never let a zero remaining expire before the user can react.
        now_instant + remaining.min(self.ttls.login).max(Duration::from_secs(1))
    }

    fn session_mut(&mut self, session_id: &str) -> Option<&mut LoginSession> {
        self.login
            .as_mut()
            .filter(|session| session.id == session_id)
    }

    fn finish(
        &mut self,
        session_id: &str,
        phase: LoginPhase,
        error: Option<(AuthErrorCode, String)>,
    ) {
        if let Some(session) = self.session_mut(session_id) {
            session.phase = phase;
            session.deadline = None;
            session.finished_at = Some(Instant::now());
            session.error = error;
        }
    }

    /// If a restart interrupted a login and fresh data now shows the provider
    /// authenticated, surface a success instead of leaving a stale cancel.
    fn reconcile_recovery(&mut self) {
        let Some(provider) = self.recovering.clone() else {
            return;
        };
        let authenticated = self
            .statuses
            .get(&provider)
            .map(|status| status.authenticated)
            .unwrap_or(false);
        if authenticated {
            if let Some(session) = self.login.as_mut() {
                if session.provider == provider {
                    session.phase = LoginPhase::Succeeded;
                    session.finished_at = Some(Instant::now());
                    session.error = None;
                    session.message = Some(tr!("auth.completed_while_restarting"));
                }
            }
            self.recovering = None;
        } else if !self.providers.is_empty() {
            // Fresh capability data arrived and the provider is still not
            // authenticated — the interrupted attempt is genuinely gone.
            self.recovering = None;
        }
    }
}

fn awaiting_phase(method: &str) -> LoginPhase {
    match method {
        "device_code" | "deviceCode" => LoginPhase::AwaitingDeviceCode,
        _ => LoginPhase::AwaitingBrowser,
    }
}

/// Map a free-form server error into a structured code. pi's `auth_login_failed`
/// event carries a proper `code`; this is the fallback for a failed response.
fn classify_error(message: &str) -> AuthErrorCode {
    let lower = message.to_ascii_lowercase();
    for code in [
        "invalid_provider",
        "unsupported_method",
        "already_in_progress",
        "not_authenticated",
        "timeout",
        "cancelled",
        "network_error",
        "storage_error",
        "internal_error",
    ] {
        if lower.contains(code) {
            return AuthErrorCode::from_code(code);
        }
    }
    if lower.contains("timed out") || lower.contains("expired") {
        AuthErrorCode::Timeout
    } else {
        AuthErrorCode::LoginFailed
    }
}

fn is_unsupported_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("unknown command") || lower.contains("unsupported")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list_payload() -> serde_json::Value {
        json!({
            "providers": [
                {"id":"anthropic","name":"Anthropic","credential":"none","authenticated":false,
                 "methods":[{"id":"browser","label":"Browser"},{"id":"api_key","label":"API key"}]},
                {"id":"openai-codex","name":"ChatGPT (Codex)","credential":"none","authenticated":false,
                 "methods":[{"id":"device_code","label":"Device code"}]}
            ]
        })
    }

    #[test]
    fn discovers_capabilities_and_never_hardcodes_providers() {
        let mut auth = AuthManager::new();
        assert_eq!(auth.support(), AuthSupport::Unknown);
        auth.on_list_response(true, Some(&list_payload()), None);
        assert_eq!(auth.support(), AuthSupport::Supported);
        assert_eq!(auth.providers().len(), 2);
        assert!(auth.provider("anthropic").unwrap().supports_oauth());
        assert_eq!(
            auth.provider("openai-codex")
                .unwrap()
                .device_code_method()
                .map(|m| m.id.as_str()),
            Some("device_code")
        );
        // An unknown provider is discoverable purely from the payload.
        auth.on_list_response(
            true,
            Some(&json!({"providers":[{"id":"future","name":"Future","methods":[]}]})),
            None,
        );
        assert!(auth.provider("future").is_some());
    }

    #[test]
    fn unsupported_pi_marks_fallback() {
        let mut auth = AuthManager::new();
        auth.on_list_response(false, None, Some("Unknown command: auth.list"));
        assert_eq!(auth.support(), AuthSupport::Unsupported);
        assert!(auth.providers().is_empty());
    }

    #[test]
    fn browser_login_opens_url_and_succeeds() {
        let mut auth = AuthManager::new();
        auth.on_list_response(true, Some(&list_payload()), None);
        let id = auth.start_login("anthropic", "browser");
        assert!(auth.is_busy());

        let effects = auth.on_event(AuthEvent::LoginStarted {
            session_id: id.clone(),
            provider: "anthropic".into(),
            method: "browser".into(),
            expires_at: None,
        });
        assert!(effects.is_empty());
        assert_eq!(auth.login().unwrap().phase, LoginPhase::AwaitingBrowser);

        let effects = auth.on_event(AuthEvent::LoginUrl {
            session_id: id.clone(),
            provider: "anthropic".into(),
            url: "https://claude.ai/oauth".into(),
        });
        assert_eq!(
            effects,
            vec![AuthEffect::OpenUrl("https://claude.ai/oauth".into())]
        );
        assert_eq!(
            auth.login().unwrap().url.as_deref(),
            Some("https://claude.ai/oauth")
        );

        let effects = auth.on_event(AuthEvent::LoginSucceeded {
            session_id: id,
            provider: "anthropic".into(),
            method: Some("browser".into()),
            credential: Some("oauth".into()),
            expires_at: Some(1_700_000_000_000),
        });
        assert_eq!(effects, vec![AuthEffect::RefreshProviders]);
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Succeeded);
        assert!(auth.status("anthropic").unwrap().authenticated);
        assert_eq!(auth.status("anthropic").unwrap().credential, "oauth");
    }

    #[test]
    fn device_code_login_records_the_challenge() {
        let mut auth = AuthManager::new();
        auth.on_list_response(true, Some(&list_payload()), None);
        let id = auth.start_login("openai-codex", "device_code");
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Connecting);
        auth.on_event(AuthEvent::DeviceCode {
            session_id: id,
            provider: "openai-codex".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://auth.openai.com/device".into(),
            verification_uri_complete: None,
            expires_at: None,
        });
        let session = auth.login().unwrap();
        assert_eq!(session.phase, LoginPhase::AwaitingDeviceCode);
        let device = session.device_code.as_ref().unwrap();
        assert_eq!(device.user_code, "ABCD-1234");
        assert!(
            session.url.is_none(),
            "device flow must not auto-open a URL"
        );
    }

    #[test]
    fn structured_failure_is_retained() {
        let mut auth = AuthManager::new();
        let id = auth.start_login("xai", "browser");
        auth.on_event(AuthEvent::LoginFailed {
            session_id: id,
            provider: "xai".into(),
            code: AuthErrorCode::Network,
            message: "connection reset".into(),
        });
        let session = auth.login().unwrap();
        assert_eq!(session.phase, LoginPhase::Error);
        assert_eq!(session.error.as_ref().unwrap().0, AuthErrorCode::Network);
        assert!(!auth.is_busy());
    }

    #[test]
    fn cancellation_transitions_and_reports_id() {
        let mut auth = AuthManager::new();
        let id = auth.start_login("anthropic", "browser");
        let cancelled = auth.cancel_login();
        assert_eq!(cancelled.as_deref(), Some(id.as_str()));
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Cancelled);

        // Cancelling a finished result just dismisses it.
        let again = auth.cancel_login();
        assert!(again.is_none());
        assert!(auth.login().is_none());
    }

    #[test]
    fn timeout_emits_cancel_effect() {
        let mut auth = AuthManager::with_ttls(AuthTtls {
            login: Duration::from_millis(5),
            result: Duration::from_secs(8),
        });
        let id = auth.start_login("anthropic", "browser");
        let now = Instant::now();
        // Before the deadline: nothing happens.
        assert!(auth.poll(now).is_empty());
        let effects = auth.poll(now + Duration::from_millis(20));
        assert_eq!(effects, vec![AuthEffect::CancelLogin(id)]);
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Error);
        assert_eq!(
            auth.login().unwrap().error.as_ref().unwrap().0,
            AuthErrorCode::Timeout
        );
    }

    #[test]
    fn result_auto_dismisses_after_ttl() {
        let mut auth = AuthManager::with_ttls(AuthTtls {
            login: Duration::from_secs(300),
            result: Duration::from_millis(5),
        });
        let id = auth.start_login("anthropic", "browser");
        auth.on_event(AuthEvent::LoginCancelled {
            session_id: id,
            provider: "anthropic".into(),
        });
        let now = Instant::now();
        assert!(auth.login().is_some());
        assert!(auth.poll(now + Duration::from_millis(20)).is_empty());
        assert!(auth.login().is_none());
    }

    #[test]
    fn login_response_adopts_server_session_id() {
        let mut auth = AuthManager::new();
        auth.start_login("anthropic", "browser");
        auth.on_login_response(true, Some(&json!({"sessionId":"server-1"})), None);
        assert_eq!(auth.login().unwrap().id, "server-1");
        assert_eq!(auth.login().unwrap().phase, LoginPhase::AwaitingBrowser);
        // Events with the server id still correlate.
        auth.on_event(AuthEvent::LoginSucceeded {
            session_id: "server-1".into(),
            provider: "anthropic".into(),
            method: None,
            credential: None,
            expires_at: None,
        });
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Succeeded);
    }

    #[test]
    fn failed_login_response_classifies_error() {
        let mut auth = AuthManager::new();
        auth.start_login("xai", "browser");
        auth.on_login_response(false, None, Some("invalid_provider: nope"));
        assert_eq!(
            auth.login().unwrap().error.as_ref().unwrap().0,
            AuthErrorCode::InvalidProvider
        );
    }

    #[test]
    fn disconnect_cancels_in_flight_login_and_recovery_reconciles() {
        let mut auth = AuthManager::new();
        auth.on_list_response(true, Some(&list_payload()), None);
        let id = auth.start_login("anthropic", "browser");
        auth.on_event(AuthEvent::LoginStarted {
            session_id: id,
            provider: "anthropic".into(),
            method: "browser".into(),
            expires_at: None,
        });
        auth.on_disconnect();
        assert_eq!(auth.support(), AuthSupport::Unknown);
        assert!(auth.providers().is_empty());
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Cancelled);
        assert_eq!(
            auth.login().unwrap().message.as_deref(),
            Some("interrupted by restart")
        );

        // On reconnect the fresh list shows the provider authenticated:
        // the interrupted attempt is reconciled to a success.
        assert_eq!(auth.on_reconnect(), vec!["anthropic".to_string()]);
        let mut payload = list_payload();
        payload["providers"][0]["authenticated"] = json!(true);
        payload["providers"][0]["credential"] = json!("oauth");
        auth.on_list_response(true, Some(&payload), None);
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Succeeded);
    }

    #[test]
    fn disconnect_then_unauthenticated_recovers_to_idle() {
        let mut auth = AuthManager::new();
        auth.on_list_response(true, Some(&list_payload()), None);
        auth.start_login("anthropic", "browser");
        auth.on_disconnect();
        auth.on_reconnect();
        auth.on_list_response(true, Some(&list_payload()), None);
        // Not authenticated: the recovering marker clears, the cancel stands.
        assert_eq!(auth.login().unwrap().phase, LoginPhase::Cancelled);
        assert_eq!(auth.on_reconnect(), Vec::<String>::new());
    }

    #[test]
    fn status_merge_and_logout() {
        let mut auth = AuthManager::new();
        auth.on_list_response(true, Some(&list_payload()), None);
        auth.on_status_response(
            true,
            Some(&json!({"provider":{"id":"anthropic","authenticated":true,"credential":"oauth","expiresAt":42,"account":"me@example.com"}})),
        );
        let status = auth.status("anthropic").unwrap();
        assert!(status.authenticated);
        assert_eq!(status.expires_at, Some(42));
        assert_eq!(status.account.as_deref(), Some("me@example.com"));

        auth.on_logout_response(true, "anthropic");
        assert!(!auth.status("anthropic").unwrap().authenticated);
    }

    #[test]
    fn credentials_changed_event_requests_refresh() {
        let mut auth = AuthManager::new();
        let effects = auth.on_event(AuthEvent::CredentialsChanged {
            providers: vec!["anthropic".into()],
        });
        assert_eq!(effects, vec![AuthEffect::RefreshProviders]);
    }

    #[test]
    fn provider_parse_ignores_token_material() {
        // A malicious or over-sharing server must not get tokens into state.
        let provider = AuthProvider::from_value(&json!({
            "id": "anthropic",
            "name": "Anthropic",
            "accessToken": "sk-ant-secret-access",
            "refreshToken": "sk-ant-secret-refresh",
            "methods": []
        }))
        .unwrap();
        let rendered = format!("{provider:?}");
        assert!(!rendered.contains("secret-access"));
        assert!(!rendered.contains("secret-refresh"));
    }

    #[test]
    fn event_racing_ahead_of_bookkeeping_is_adopted() {
        let mut auth = AuthManager::new();
        // No start_login call: the event creates the session.
        auth.on_event(AuthEvent::LoginStarted {
            session_id: "server-2".into(),
            provider: "anthropic".into(),
            method: "browser".into(),
            expires_at: None,
        });
        let session = auth.login().unwrap();
        assert_eq!(session.id, "server-2");
        assert_eq!(session.phase, LoginPhase::AwaitingBrowser);
    }
}
