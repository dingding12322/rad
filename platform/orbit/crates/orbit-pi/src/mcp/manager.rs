//! The MCP manager: Orbit's single source of truth for configured servers,
//! their live state, and the secret store behind their `${NAME}` references.
//!
//! The manager owns no MCP protocol logic. It reads and writes Pi's own
//! `mcp.json` files through [`super::config`], hands Orbit's secrets to Pi as
//! environment variables at spawn time, and folds the result of Pi's own
//! `pi mcp list --json` probe into per-server runtime state. Connection
//! testing, tool discovery, and reconnection are therefore always Pi's
//! answer, never an Orbit approximation.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::config::{self, SecretUpdate};
use super::secrets::McpSecrets;
use super::{
    McpError, McpExposure, McpScope, McpServer, McpServerDef, McpServerRuntime, McpServerStatus,
};

/// Upper bound on one `pi mcp list --json` probe. Pi's own per-server request
/// timeout defaults to 60s and connections run in parallel, so this only
/// trips on a wedged process.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long Pi waits for the browser during `pi mcp login`. Mirrors Pi's own
/// default; Orbit waits a little longer for the process to exit cleanly.
pub(crate) const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// Slack over [`LOGIN_TIMEOUT`] before Orbit kills a stuck login process.
const LOGIN_GRACE: Duration = Duration::from_secs(30);
/// Cap on retained stderr from a failed probe or auth command.
const PROBE_STDERR_CAP: usize = 8 * 1024;

/// Everything `pi mcp list --json` reports, in Orbit's types.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpProbe {
    pub servers: Vec<McpProbeServer>,
    /// Pi's own config parse errors.
    pub errors: Vec<String>,
    /// Pi's note about ignored project config (untrusted project).
    pub note: Option<String>,
    /// The probe itself could not run or answer; everything else is moot.
    pub fatal: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct McpProbeServer {
    pub name: String,
    pub state: McpServerStatus,
    pub tools: Vec<String>,
    pub tool_exposure: BTreeMap<String, McpExposure>,
    pub resources: u64,
    pub resource_templates: u64,
    pub error: Option<String>,
}

/// One add/edit submission from the form.
#[derive(Clone, Debug)]
pub(crate) struct McpDraft {
    /// `(name, scope)` when editing an existing server.
    pub original: Option<(String, McpScope)>,
    pub name: String,
    pub scope: McpScope,
    pub description: Option<String>,
    pub def: McpServerDef,
    /// Literal secrets the form collected; stored before the config write.
    pub new_secrets: Vec<SecretUpdate>,
}

/// The MCP domain state the app holds. One instance lives on `OrbitApp`.
pub(crate) struct McpManager {
    home: PathBuf,
    workspace: Option<PathBuf>,
    global_path: PathBuf,
    project_path: Option<PathBuf>,
    servers: Vec<McpServer>,
    runtime: HashMap<String, McpServerRuntime>,
    /// Config parse/validation errors from the last load.
    errors: Vec<String>,
    /// Pi's errors/note from the last probe.
    probe_errors: Vec<String>,
    note: Option<String>,
    /// Pi's top-level `autoEnableCodemode`; `Some(false)` means codemode
    /// servers' tools may be unreachable, which the page warns about.
    auto_enable_codemode: Option<bool>,
    /// A probe is queued or in flight.
    loading: bool,
    probe_inflight: bool,
    last_probed: Option<Instant>,
    secrets: McpSecrets,
    metadata: McpMetadata,
    /// Hash of the config files this snapshot was loaded from. Spawned Pi
    /// processes are stamped with it (see `OrbitApp::mcp_stamp`).
    fingerprint: u64,
}

impl McpManager {
    /// Load the manager for `workspace` from the real user directories.
    pub(crate) fn load(workspace: Option<&Path>) -> Self {
        Self::load_in(&crate::platform::home_dir(), workspace)
    }

    /// Explicit-home constructor — the seam the tests drive.
    pub(crate) fn load_in(home: &Path, workspace: Option<&Path>) -> Self {
        let metadata = McpMetadata::load(home.join(".orbit-pi").join("mcp-metadata.json"));
        let mut manager = Self {
            home: home.to_path_buf(),
            workspace: workspace.map(Path::to_path_buf),
            global_path: home.join(".pi").join("agent").join("mcp.json"),
            project_path: None,
            servers: Vec::new(),
            runtime: HashMap::new(),
            errors: Vec::new(),
            probe_errors: Vec::new(),
            note: None,
            auto_enable_codemode: None,
            loading: false,
            probe_inflight: false,
            last_probed: None,
            secrets: McpSecrets::load_from(super::secrets::default_path_for(home)),
            metadata,
            fingerprint: 0,
        };
        manager.refresh();
        manager
    }

    /// Point the manager at a new workspace (project scope follows it) and
    /// re-read the files.
    pub(crate) fn set_workspace(&mut self, workspace: Option<&Path>) {
        let next = workspace.map(Path::to_path_buf);
        if self.workspace == next {
            return;
        }
        self.workspace = next;
        self.refresh();
    }

    /// Re-read both config files and refresh the domain state. Cheap (two
    /// small files) and safe to call from the UI thread.
    pub(crate) fn refresh(&mut self) {
        // A fixed or externally edited secret store should be picked up by the
        // same Refresh the page exposes for its config files.
        self.secrets.reload();
        self.metadata.reload();
        let load = config::load(&self.home, self.workspace.as_deref());
        self.global_path = load.global_path;
        self.project_path = load.project_path;
        let mut servers = load.servers;
        for server in &mut servers {
            server.description = self.metadata.description_for(
                server.scope,
                self.workspace.as_deref(),
                &server.name,
            );
        }
        self.errors = load.errors;
        // A sidecar file the page could not read is a first-class error, not
        // a silent empty default: writes are refused until it is fixed.
        if let Some(detail) = self.secrets.error() {
            self.errors
                .push(McpError::Io(detail.to_string()).user_message());
        }
        if let Some(detail) = self.metadata.error() {
            self.errors
                .push(McpError::Io(detail.to_string()).user_message());
        }
        self.auto_enable_codemode = load.auto_enable_codemode;
        self.fingerprint = config::fingerprint(&self.home, self.workspace.as_deref());
        self.servers = servers;
        // Drop runtime state for servers that no longer exist; a disabled
        // server's state is a configuration fact, not a probe result.
        let names: BTreeSet<String> = self
            .servers
            .iter()
            .map(|server| server.name.clone())
            .collect();
        self.runtime.retain(|name, _| names.contains(name));
        for server in &self.servers {
            let runtime = self.runtime.entry(server.name.clone()).or_default();
            if !server.def.enabled {
                runtime.status = McpServerStatus::Disabled;
                runtime.tools.clear();
                runtime.tool_exposure.clear();
                runtime.resources = 0;
                runtime.resource_templates = 0;
                runtime.error = None;
            }
        }
    }

    pub(crate) fn servers(&self) -> &[McpServer] {
        &self.servers
    }

    pub(crate) fn server(&self, name: &str) -> Option<&McpServer> {
        self.servers.iter().find(|server| server.name == name)
    }

    pub(crate) fn runtime(&self, name: &str) -> McpServerRuntime {
        self.runtime.get(name).cloned().unwrap_or_default()
    }

    pub(crate) fn errors(&self) -> &[String] {
        &self.errors
    }

    pub(crate) fn probe_errors(&self) -> &[String] {
        &self.probe_errors
    }

    pub(crate) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Pi's `autoEnableCodemode` setting, when the file sets one.
    pub(crate) fn auto_enable_codemode(&self) -> Option<bool> {
        self.auto_enable_codemode
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    pub(crate) fn is_probing(&self) -> bool {
        self.probe_inflight
    }

    pub(crate) fn last_probed(&self) -> Option<Instant> {
        self.last_probed
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The config fingerprint for an arbitrary workspace — the stamp a pi
    /// process spawned there carries. [`fingerprint`](Self::fingerprint) only
    /// covers the workspace the manager is currently pointed at, and that lags
    /// the session being switched to (see `tick_mcp`), so it must not be used
    /// to judge a parked session from another workspace.
    pub(crate) fn fingerprint_for(&self, workspace: Option<&Path>) -> u64 {
        config::fingerprint(&self.home, workspace)
    }

    pub(crate) fn global_path(&self) -> &Path {
        &self.global_path
    }

    pub(crate) fn project_path(&self) -> Option<&Path> {
        self.project_path.as_deref()
    }

    pub(crate) fn workspace(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    /// The secret name/value pairs injected into every Pi spawn and probe.
    pub(crate) fn secret_env(&self) -> Vec<(String, String)> {
        self.secrets.env()
    }

    /// Mark a probe as started (the app kicks the blocking worker off-thread).
    pub(crate) fn begin_probe(&mut self) {
        self.probe_inflight = true;
        self.loading = true;
        self.log_event("probe_started", "");
    }

    /// Structured lifecycle log on stderr. Orbit ships no log file; these
    /// lines surface in a terminal launch and Console.app.
    /// To avoid leaking sensitive or untrusted data, dynamic detail payloads
    /// are intentionally not emitted.
    /// Pi's own server log messages live in `~/.pi/agent/mcp.log`.
    pub(crate) fn log_event(&self, event: &str, _detail: &str) {
        eprintln!("orbit-mcp event={event}");
    }

    /// Fold one probe result into the runtime cache. Servers Pi did not
    /// report keep their previous state rather than being invented into
    /// "disconnected".
    pub(crate) fn apply_probe(&mut self, probe: McpProbe) {
        self.probe_inflight = false;
        self.loading = false;
        self.last_probed = Some(Instant::now());
        self.note = probe.note;
        self.probe_errors = probe
            .errors
            .iter()
            .map(|error| self.secrets.redact(error))
            .collect();

        if let Some(fatal) = probe.fatal {
            let fatal = self.secrets.redact(&fatal);
            self.probe_errors.insert(0, fatal.clone());
            for server in &self.servers {
                if !server.def.enabled {
                    continue;
                }
                let runtime = self.runtime.entry(server.name.clone()).or_default();
                runtime.status = McpServerStatus::Failed;
                runtime.error = Some(fatal.clone());
                runtime.checked_at = Some(Instant::now());
            }
            return;
        }

        for entry in probe.servers {
            if self.server(&entry.name).is_none() {
                continue;
            }
            let runtime = self.runtime.entry(entry.name).or_default();
            runtime.status = entry.state;
            runtime.tools = entry.tools;
            runtime.tool_exposure = entry.tool_exposure;
            runtime.resources = entry.resources;
            runtime.resource_templates = entry.resource_templates;
            runtime.error = entry.error.map(|error| self.secrets.redact(&error));
            runtime.checked_at = Some(Instant::now());
        }
        for server in &self.servers {
            if server.def.enabled {
                continue;
            }
            let runtime = self.runtime.entry(server.name.clone()).or_default();
            runtime.status = McpServerStatus::Disabled;
            runtime.tools.clear();
            runtime.error = None;
            runtime.checked_at = Some(Instant::now());
        }
        let connected = self
            .runtime
            .values()
            .filter(|runtime| runtime.status == McpServerStatus::Connected)
            .count();
        self.log_event(
            "probe_finished",
            &format!("connected={connected} configured={}", self.servers.len()),
        );
    }

    /// Add or update a server. Secrets are stored first so a successful
    /// config write can never reference a value that was not saved; a failed
    /// config write leaves the previous file (and the orphaned secret, which
    /// pruning removes later) untouched.
    pub(crate) fn define(&mut self, draft: &McpDraft) -> Result<(), McpError> {
        config::validate_name(&draft.name).map_err(McpError::InvalidConfiguration)?;
        config::validate_def(&draft.name, &draft.def).map_err(McpError::InvalidConfiguration)?;
        for update in &draft.new_secrets {
            self.secrets.set(&update.name, &update.value)?;
        }

        let Some(target) = draft
            .scope
            .config_path(&self.home, self.workspace.as_deref())
        else {
            return Err(McpError::InvalidConfiguration(tr!(
                "mcp.error_project_without_workspace"
            )));
        };
        config::write_add(&target, &draft.name, &draft.def)?;

        let mut outcome = Ok(());
        if let Some((original_name, original_scope)) = &draft.original {
            if *original_scope != draft.scope || *original_name != draft.name {
                let Some(source) =
                    original_scope.config_path(&self.home, self.workspace.as_deref())
                else {
                    return Err(McpError::InvalidConfiguration(tr!(
                        "mcp.error_project_without_workspace"
                    )));
                };
                if let Err(error) = config::write_remove(&source, original_name) {
                    outcome = Err(error);
                } else if let Err(error) =
                    self.metadata
                        .remove(*original_scope, self.workspace.as_deref(), original_name)
                {
                    outcome = Err(error);
                }
            }
        }
        if let Err(error) = self.metadata.set(
            draft.scope,
            self.workspace.as_deref(),
            &draft.name,
            draft.description.as_deref(),
        ) {
            outcome = Err(error);
        }
        // Re-read even when a metadata/follow-up write failed, so the UI
        // always reflects what is actually on disk.
        self.refresh();
        self.log_event(
            "config_updated",
            &format!("server=\"{}\" scope={}", draft.name, draft.scope.as_str()),
        );
        outcome
    }

    /// Remove a server and prune the Orbit-generated secrets only it
    /// referenced.
    pub(crate) fn remove(&mut self, name: &str) -> Result<(), McpError> {
        let server = self
            .server(name)
            .cloned()
            .ok_or_else(|| McpError::ServerNotFound(name.to_string()))?;
        config::write_remove(&server.source, name)?;
        let mut outcome = Ok(());
        if let Err(error) = self
            .metadata
            .remove(server.scope, self.workspace.as_deref(), name)
        {
            outcome = Err(error);
        }
        // Re-read even when the metadata write failed so the UI matches disk.
        self.refresh();
        // Prune after the refresh so the remaining servers are authoritative.
        // Any Orbit-generated secret in this workspace's namespaces that no
        // remaining server references is stale — whether the removed server
        // referenced it or an earlier edit already orphaned it. Secrets from
        // other projects (and user-provided names) are never touched.
        let mut referenced: BTreeSet<String> = BTreeSet::new();
        for remaining in &self.servers {
            referenced.extend(config::definition_references(&remaining.def));
        }
        let mut namespaces = vec![config::secret_namespace(McpScope::Global, None)];
        if self.workspace.is_some() {
            namespaces.push(config::secret_namespace(
                McpScope::Project,
                self.workspace.as_deref(),
            ));
        }
        self.secrets.prune_unreferenced(&referenced, &namespaces)?;
        self.log_event("server_removed", &format!("server=\"{name}\""));
        outcome
    }

    pub(crate) fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<(), McpError> {
        let server = self
            .server(name)
            .cloned()
            .ok_or_else(|| McpError::ServerNotFound(name.to_string()))?;
        if server.def.enabled == enabled {
            return Ok(());
        }
        config::write_patch(&server.source, name, config::McpPatch::Enabled(enabled))?;
        self.refresh();
        self.log_event(
            if enabled {
                "server_enabled"
            } else {
                "server_disabled"
            },
            &format!("server=\"{name}\""),
        );
        Ok(())
    }

    /// Whether the given fingerprint matches the configuration on disk — the
    /// check that decides whether a running Pi process is stale.
    pub(crate) fn fingerprint_matches(&self, stamp: u64) -> bool {
        stamp == self.fingerprint
    }

    /// The config files' current hash without mutating any state. The MCP
    /// page polls this (cheaply) to notice edits made outside Orbit.
    pub(crate) fn disk_fingerprint(&self) -> u64 {
        config::fingerprint(&self.home, self.workspace.as_deref())
    }

    /// Scrub known secret values from any text before display or logging.
    pub(crate) fn redact_text(&self, text: &str) -> String {
        self.secrets.redact(text)
    }
}

// ── the probe ──────────────────────────────────────────────────────────────

/// Run `pi mcp list --json` in `workspace` with Orbit's secrets in the
/// environment. Blocking: call it on `cx.background_executor()`.
pub(crate) fn probe(workspace: &Path, env: &[(String, String)]) -> McpProbe {
    probe_with_agent_dir(workspace, env, None)
}

/// [`probe`] with an explicit `PI_CODING_AGENT_DIR`. The live test drives a
/// throwaway configuration through the real pi CLI without touching the
/// user's store; production callers pass `None` and follow the environment.
pub(crate) fn probe_with_agent_dir(
    workspace: &Path,
    env: &[(String, String)],
    agent_dir: Option<&Path>,
) -> McpProbe {
    let mut command = pi_mcp_command(&["mcp", "list", "--json"], workspace, env);
    if let Some(dir) = agent_dir {
        command.env("PI_CODING_AGENT_DIR", dir);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return McpProbe {
                fatal: Some(tr!("mcp.error_pi_spawn", detail = err.to_string())),
                ..McpProbe::default()
            };
        }
    };
    let (stdout, stderr, timed_out, _) = drain_child(&mut child, PROBE_TIMEOUT, None);
    if timed_out {
        return McpProbe {
            fatal: Some(tr!("mcp.error_probe_timeout")),
            ..McpProbe::default()
        };
    }
    let text = String::from_utf8_lossy(&stdout);
    if text.trim().is_empty() {
        // `pi mcp list` exits 1 when a server fails but still prints JSON; an
        // empty stdout means the command itself refused to run.
        let detail = String::from_utf8_lossy(&stderr);
        let detail = detail.trim();
        return McpProbe {
            fatal: Some(if detail.is_empty() {
                tr!("mcp.error_probe_empty")
            } else {
                tr!("mcp.error_probe_failed", detail = truncate(detail, 400))
            }),
            ..McpProbe::default()
        };
    }
    match parse_probe_json(&text) {
        Ok((servers, errors, note)) => McpProbe {
            servers,
            errors,
            note,
            fatal: None,
        },
        Err(detail) => McpProbe {
            fatal: Some(tr!("mcp.error_probe_parse", detail = detail)),
            ..McpProbe::default()
        },
    }
}

/// The shared `pi mcp <args>` invocation: the resolved binary, an augmented
/// `PATH` (a bundled `.app` lacks `node`), quiet non-interactive env, and
/// Orbit's secret values so `${NAME}` references resolve exactly as they do
/// in a session.
fn pi_mcp_command(args: &[&str], workspace: &Path, env: &[(String, String)]) -> Command {
    let bin = orbit_rpc::pi_binary();
    let mut command = Command::new(&bin);
    command
        .args(args)
        .env("PI_SKIP_VERSION_CHECK", "1")
        .env("PATH", orbit_rpc::augmented_path(Path::new(&bin).parent()))
        .env("NO_COLOR", "1")
        .env("CI", "1")
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    orbit_rpc::hide_console(&mut command);
    for (key, value) in env {
        command.env(key, value);
    }
    command
}

/// Run `pi mcp login <server>`: Pi opens the authorization page in the
/// browser, waits for the loopback callback, and stores the tokens. Blocking;
/// call it on `cx.background_executor()`. `cancel` lets the UI abort a
/// waiting browser — the child is killed and [`McpError::Cancelled`] returned.
pub(crate) fn login(
    workspace: &Path,
    env: &[(String, String)],
    server: &str,
    browser_timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<(), McpError> {
    let seconds = browser_timeout.as_secs().max(1).to_string();
    let mut command = pi_mcp_command(
        &["mcp", "login", server, "--timeout", &seconds],
        workspace,
        env,
    );
    run_auth_command(
        &mut command,
        browser_timeout + LOGIN_GRACE,
        cancel,
        server,
        AuthOp::SignIn,
    )
}

/// Run `pi mcp logout <server>`, deleting Pi's stored OAuth credentials.
pub(crate) fn logout(
    workspace: &Path,
    env: &[(String, String)],
    server: &str,
) -> Result<(), McpError> {
    let mut command = pi_mcp_command(&["mcp", "logout", server], workspace, env);
    run_auth_command(&mut command, PROBE_TIMEOUT, None, server, AuthOp::SignOut)
}

/// Which half of the OAuth lifecycle a command performs; only the error copy
/// differs.
#[derive(Clone, Copy)]
enum AuthOp {
    SignIn,
    SignOut,
}

impl AuthOp {
    fn timeout_key(self) -> &'static str {
        match self {
            Self::SignIn => "mcp.error_sign_in_timeout",
            Self::SignOut => "mcp.error_sign_out_timeout",
        }
    }

    fn failed_key(self) -> &'static str {
        match self {
            Self::SignIn => "mcp.error_sign_in_failed",
            Self::SignOut => "mcp.error_sign_out_failed",
        }
    }
}

fn run_auth_command(
    command: &mut Command,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    server: &str,
    op: AuthOp,
) -> Result<(), McpError> {
    let mut child = command.spawn().map_err(|err| {
        McpError::ProcessStartFailed(tr!("mcp.error_pi_spawn", detail = err.to_string()))
    })?;
    let (stdout, stderr, stopped, status) = drain_child(&mut child, timeout, cancel);
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err(McpError::Cancelled);
    }
    if stopped {
        return Err(McpError::Timeout(tr!(op.timeout_key(), name = server)));
    }
    if status.is_some_and(|status| status.success()) {
        // `login` prints progress (and sometimes the authorization URL) on
        // stdout; Orbit only needs the exit status. Stdout stays consumed so
        // the pipe can never wedge.
        return Ok(());
    }
    let stderr_text = String::from_utf8_lossy(&stderr);
    let stdout_text = String::from_utf8_lossy(&stdout);
    let stderr_text = stderr_text.trim();
    let detail = if stderr_text.is_empty() {
        stdout_text.trim().to_string()
    } else {
        stderr_text.to_string()
    };
    Err(McpError::AuthenticationFailed(tr!(
        op.failed_key(),
        name = server,
        detail = truncate(&detail, 400)
    )))
}

/// Read a child's pipes on helper threads and wait for it with a deadline or
/// a cancellation flag. Returns `(stdout, stderr, stopped, status)`; `stopped`
/// is true when the wait was cut short (timeout or cancellation) and the
/// process was terminated.
fn drain_child(
    child: &mut Child,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> (Vec<u8>, Vec<u8>, bool, Option<ExitStatus>) {
    use std::io::Read as _;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_thread = stdout.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_thread = stderr.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let mut stopped = false;
    let mut status = None;
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            stopped = true;
            terminate_child(child);
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(40));
            }
            Ok(None) => {
                stopped = true;
                terminate_child(child);
                let grace = Instant::now() + Duration::from_secs(3);
                while Instant::now() < grace {
                    match child.try_wait() {
                        Ok(Some(_)) | Err(_) => break,
                        Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Err(_) => {
                stopped = true;
                let _ = child.kill();
                break;
            }
        }
    }
    let mut out = stdout_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    let mut err = stderr_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    if err.len() > PROBE_STDERR_CAP {
        err = err[err.len() - PROBE_STDERR_CAP..].to_vec();
    }
    out.shrink_to_fit();
    (out, err, stopped, status)
}

/// Parse the `pi mcp list --json` document.
#[allow(clippy::type_complexity)]
fn parse_probe_json(
    text: &str,
) -> Result<(Vec<McpProbeServer>, Vec<String>, Option<String>), String> {
    let doc: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let entries = doc
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| "the probe did not report a `servers` array".to_string())?;
    let mut servers = Vec::new();
    for entry in entries {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let state = entry
            .get("state")
            .and_then(Value::as_str)
            .map(McpServerStatus::parse)
            .unwrap_or_default();
        let tools = entry
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let tool_exposure = entry
            .get("toolExposure")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(tool, mode)| {
                        mode.as_str()
                            .and_then(McpExposure::parse)
                            .map(|mode| (tool.clone(), mode))
                    })
                    .collect()
            })
            .unwrap_or_default();
        servers.push(McpProbeServer {
            name: name.to_string(),
            state,
            tools,
            tool_exposure,
            resources: entry.get("resources").and_then(Value::as_u64).unwrap_or(0),
            resource_templates: entry
                .get("resourceTemplates")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            error: entry
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    let errors = doc
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let note = doc.get("note").and_then(Value::as_str).map(str::to_string);
    Ok((servers, errors, note))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// Ask the child to exit before the hard kill. Pi closes its stdio MCP
/// servers' process groups on a graceful shutdown; a bare `kill` would leave
/// them orphaned. Non-Unix falls back to `kill` (TerminateProcess).
fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: `child.id()` is a live pid this process spawned; the kernel
        // delivers the signal, and failure is a no-op.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

// ── Orbit-only metadata (descriptions) ─────────────────────────────────────

/// Server descriptions are Orbit UX metadata with no Pi equivalent, so they
/// live in their own file (`~/.orbit-pi/mcp-metadata.json`) and never enter
/// `mcp.json`.
struct McpMetadata {
    path: PathBuf,
    descriptions: BTreeMap<String, String>,
    /// Why the file could not be read; writes are refused while set.
    load_error: Option<String>,
}

impl McpMetadata {
    fn load(path: PathBuf) -> Self {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Self {
                    path,
                    descriptions: BTreeMap::new(),
                    load_error: None,
                }
            }
            Err(err) => {
                let load_error = format!("{}: {err}", path.display());
                return Self {
                    path,
                    descriptions: BTreeMap::new(),
                    load_error: Some(load_error),
                };
            }
        };
        if text.trim().is_empty() {
            return Self {
                path,
                descriptions: BTreeMap::new(),
                load_error: None,
            };
        }
        let parsed = serde_json::from_str::<Value>(&text).map_err(|err| err.to_string());
        let descriptions = parsed.and_then(|doc| match doc.get("descriptions") {
            None => Ok(BTreeMap::new()),
            Some(Value::Object(map)) => Ok(map
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()),
            Some(other) => Err(format!("`descriptions` must be an object, got {other}")),
        });
        match descriptions {
            Ok(descriptions) => Self {
                path,
                descriptions,
                load_error: None,
            },
            Err(err) => {
                let load_error = format!("{}: {err}", path.display());
                Self {
                    path,
                    descriptions: BTreeMap::new(),
                    load_error: Some(load_error),
                }
            }
        }
    }

    fn error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Re-read the file from disk, so a fixed or externally edited metadata
    /// file is picked up by the same Refresh that re-reads the config.
    fn reload(&mut self) {
        *self = Self::load(self.path.clone());
    }

    fn key(scope: McpScope, workspace: Option<&Path>, name: &str) -> String {
        format!(
            "{}|{}|{}",
            scope.as_str(),
            workspace
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            name
        )
    }

    fn description_for(
        &self,
        scope: McpScope,
        workspace: Option<&Path>,
        name: &str,
    ) -> Option<String> {
        self.descriptions
            .get(&Self::key(scope, workspace, name))
            .cloned()
    }

    fn set(
        &mut self,
        scope: McpScope,
        workspace: Option<&Path>,
        name: &str,
        description: Option<&str>,
    ) -> Result<(), McpError> {
        let key = Self::key(scope, workspace, name);
        let Some(description) = description.filter(|text| !text.trim().is_empty()) else {
            if self.descriptions.remove(&key).is_none() {
                return Ok(());
            }
            return self.persist();
        };
        if self.descriptions.get(&key).map(String::as_str) == Some(description) {
            return Ok(());
        }
        self.descriptions.insert(key, description.to_string());
        self.persist()
    }

    fn remove(
        &mut self,
        scope: McpScope,
        workspace: Option<&Path>,
        name: &str,
    ) -> Result<(), McpError> {
        if self
            .descriptions
            .remove(&Self::key(scope, workspace, name))
            .is_none()
        {
            return Ok(());
        }
        self.persist()
    }

    fn persist(&self) -> Result<(), McpError> {
        if let Some(detail) = &self.load_error {
            return Err(McpError::Io(detail.clone()));
        }
        let mut descriptions = serde_json::Map::new();
        for (key, value) in &self.descriptions {
            descriptions.insert(key.clone(), Value::String(value.clone()));
        }
        let mut root = serde_json::Map::new();
        root.insert("version".into(), Value::Number(1.into()));
        root.insert("descriptions".into(), Value::Object(descriptions));
        let text = config::serialize(&Value::Object(root), b"  ")?;
        config::write_atomic(&self.path, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpTransport;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "orbit-mcp-manager-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_manager(tag: &str) -> (PathBuf, PathBuf, McpManager) {
        let root = temp_root(tag);
        let home = root.join("home");
        let workspace = root.join("project");
        fs::create_dir_all(&workspace).unwrap();
        let manager = McpManager::load_in(&home, Some(&workspace));
        (home, workspace, manager)
    }

    fn stdio_def(command: &str) -> McpServerDef {
        McpServerDef {
            transport: McpTransport::Stdio {
                command: command.into(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
            },
            ..McpServerDef::default()
        }
    }

    fn draft(name: &str, scope: McpScope, command: &str) -> McpDraft {
        McpDraft {
            original: None,
            name: name.into(),
            scope,
            description: Some(format!("{name} description")),
            def: stdio_def(command),
            new_secrets: Vec::new(),
        }
    }

    #[test]
    fn add_update_enable_disable_remove_round_trip() {
        let (_home, _workspace, mut manager) = test_manager("crud");
        manager
            .define(&draft("github", McpScope::Global, "npx"))
            .unwrap();
        assert_eq!(manager.servers().len(), 1);
        assert_eq!(
            manager.server("github").unwrap().description.as_deref(),
            Some("github description")
        );

        // Update the same server in place.
        let mut update = draft("github", McpScope::Global, "uvx");
        update.original = Some(("github".into(), McpScope::Global));
        update.description = Some("updated".into());
        manager.define(&update).unwrap();
        match &manager.server("github").unwrap().def.transport {
            McpTransport::Stdio { command, .. } => assert_eq!(command, "uvx"),
            other => panic!("expected stdio, got {other:?}"),
        }
        assert_eq!(
            manager.server("github").unwrap().description.as_deref(),
            Some("updated")
        );

        manager.set_enabled("github", false).unwrap();
        assert!(!manager.server("github").unwrap().def.enabled);
        assert_eq!(manager.runtime("github").status, McpServerStatus::Disabled);
        manager.set_enabled("github", true).unwrap();
        assert!(manager.server("github").unwrap().def.enabled);

        manager.remove("github").unwrap();
        assert!(manager.servers().is_empty());
    }

    #[test]
    fn project_scope_lands_in_the_project_file() {
        let (home, workspace, mut manager) = test_manager("project");
        manager
            .define(&draft("local", McpScope::Project, "node"))
            .unwrap();
        let project = workspace.join(".pi").join("mcp.json");
        assert!(project.exists());
        let global = home.join(".pi").join("agent").join("mcp.json");
        assert!(!global.exists());
        assert_eq!(manager.server("local").unwrap().scope, McpScope::Project);
    }

    #[test]
    fn moving_scope_removes_the_old_entry() {
        let (_home, workspace, mut manager) = test_manager("move");
        manager
            .define(&draft("tool", McpScope::Global, "a"))
            .unwrap();
        let mut moved = draft("tool", McpScope::Project, "b");
        moved.original = Some(("tool".into(), McpScope::Global));
        manager.define(&moved).unwrap();
        assert_eq!(manager.servers().len(), 1);
        assert_eq!(manager.server("tool").unwrap().scope, McpScope::Project);
        // The global file no longer defines it.
        let global = manager.global_path().to_path_buf();
        let doc: Value = serde_json::from_str(&fs::read_to_string(&global).unwrap()).unwrap();
        assert!(doc["mcpServers"].get("tool").is_none());
        assert!(workspace.join(".pi").join("mcp.json").exists());
    }

    #[test]
    fn secrets_are_stored_before_the_config_write() {
        let (_home, _workspace, mut manager) = test_manager("secrets");
        let mut draft = draft("github", McpScope::Global, "npx");
        draft
            .def
            .transport
            .secret_values_mut()
            .insert("TOKEN".into(), "${MCP_GITHUB_TOKEN}".into());
        draft.new_secrets.push(SecretUpdate {
            name: "MCP_GITHUB_TOKEN".into(),
            value: "ghp_secret".into(),
        });
        manager.define(&draft).unwrap();
        assert_eq!(manager.secrets.get("MCP_GITHUB_TOKEN"), Some("ghp_secret"));
        assert_eq!(
            manager.secret_env(),
            vec![("MCP_GITHUB_TOKEN".to_string(), "ghp_secret".to_string())]
        );
    }

    #[test]
    fn removing_a_server_prunes_only_its_generated_secrets() {
        let (_home, _workspace, mut manager) = test_manager("prune");
        let first_secret = config::generated_secret_name("first", "TOKEN", McpScope::Global, None);
        let shared_secret =
            config::generated_secret_name("first", "SHARED", McpScope::Global, None);
        let mut first = draft("first", McpScope::Global, "a");
        first
            .def
            .transport
            .secret_values_mut()
            .insert("TOKEN".into(), format!("${{{first_secret}}}"));
        first.new_secrets.push(SecretUpdate {
            name: first_secret.clone(),
            value: "one".into(),
        });
        manager.define(&first).unwrap();
        // The shared secret is referenced by both servers.
        for name in ["first", "second"] {
            let mut d = draft(name, McpScope::Global, name);
            d.original = manager.server(name).map(|s| (name.to_string(), s.scope));
            d.def
                .transport
                .secret_values_mut()
                .insert("SHARED".into(), format!("${{{shared_secret}}}"));
            manager.define(&d).unwrap();
        }
        manager.secrets.set(&shared_secret, "shared").unwrap();

        manager.remove("first").unwrap();
        assert!(!manager.secrets.contains(&first_secret));
        assert!(manager.secrets.contains(&shared_secret));
        assert_eq!(manager.secrets.get(&shared_secret), Some("shared"));
    }

    #[test]
    fn removing_in_one_project_keeps_another_projects_secrets() {
        let root = temp_root("cross-workspace");
        let home = root.join("home");
        let project_a = root.join("a");
        let project_b = root.join("b");
        fs::create_dir_all(&project_a).unwrap();
        fs::create_dir_all(&project_b).unwrap();

        // Project B stores a generated secret for its server.
        let secret_b =
            config::generated_secret_name("tool", "TOKEN", McpScope::Project, Some(&project_b));
        let mut manager_b = McpManager::load_in(&home, Some(&project_b));
        let mut b = draft("tool", McpScope::Project, "node");
        b.def
            .transport
            .secret_values_mut()
            .insert("TOKEN".into(), format!("${{{secret_b}}}"));
        b.new_secrets.push(SecretUpdate {
            name: secret_b.clone(),
            value: "b-secret".into(),
        });
        manager_b.define(&b).unwrap();

        // Project A removes one of its servers. Its prune must not reach
        // into project B's namespace.
        let mut manager_a = McpManager::load_in(&home, Some(&project_a));
        manager_a
            .define(&draft("other", McpScope::Project, "node"))
            .unwrap();
        manager_a.remove("other").unwrap();

        assert!(manager_a.secrets.contains(&secret_b));
        assert_eq!(manager_a.secrets.get(&secret_b), Some("b-secret"));
    }

    #[test]
    fn an_unreadable_secrets_store_is_reported_not_clobbered() {
        let root = temp_root("broken-secrets");
        let home = root.join("home");
        fs::create_dir_all(home.join(".orbit-pi")).unwrap();
        let path = crate::mcp::secrets::default_path_for(&home);
        fs::write(&path, "not json").unwrap();

        let mut manager = McpManager::load_in(&home, None);
        assert!(
            manager
                .errors()
                .iter()
                .any(|error| error.contains("mcp-secrets.json")),
            "the unreadable store must surface: {:?}",
            manager.errors()
        );

        // A save that would store a literal secret is refused rather than
        // overwriting the unreadable file.
        let secret_name = config::generated_secret_name("github", "TOKEN", McpScope::Global, None);
        let mut d = draft("github", McpScope::Global, "npx");
        d.def
            .transport
            .secret_values_mut()
            .insert("TOKEN".into(), format!("${{{secret_name}}}"));
        d.new_secrets.push(SecretUpdate {
            name: secret_name,
            value: "ghp_secret".into(),
        });
        let error = manager.define(&d).unwrap_err();
        assert!(matches!(error, McpError::Io(_)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "not json");

        // Fixing the file and refreshing clears the error and allows saves.
        fs::write(&path, "{\"version\":1,\"secrets\":{}}").unwrap();
        manager.refresh();
        assert!(
            !manager
                .errors()
                .iter()
                .any(|error| error.contains("mcp-secrets.json")),
            "a fixed store must clear the error: {:?}",
            manager.errors()
        );
        manager.define(&d).unwrap();
    }

    #[test]
    fn an_unreadable_metadata_file_is_reported_and_clears_on_refresh() {
        let root = temp_root("broken-metadata");
        let home = root.join("home");
        fs::create_dir_all(home.join(".orbit-pi")).unwrap();
        let path = home.join(".orbit-pi").join("mcp-metadata.json");
        fs::write(&path, "not json").unwrap();

        let mut manager = McpManager::load_in(&home, None);
        assert!(
            manager
                .errors()
                .iter()
                .any(|error| error.contains("mcp-metadata.json")),
            "the unreadable metadata must surface: {:?}",
            manager.errors()
        );

        fs::write(&path, "{\"version\":1,\"descriptions\":{}}").unwrap();
        manager.refresh();
        assert!(!manager
            .errors()
            .iter()
            .any(|error| error.contains("mcp-metadata.json")));
        manager
            .define(&draft("github", McpScope::Global, "npx"))
            .unwrap();
    }

    #[test]
    fn invalid_definitions_are_rejected_before_any_write() {
        let (_home, _workspace, mut manager) = test_manager("invalid");
        let mut bad = draft("bad", McpScope::Global, "");
        assert!(matches!(
            manager.define(&bad).unwrap_err(),
            McpError::InvalidConfiguration(_)
        ));
        bad.name = "bad name".into();
        bad.def = stdio_def("npx");
        assert!(matches!(
            manager.define(&bad).unwrap_err(),
            McpError::InvalidConfiguration(_)
        ));
        assert!(manager.servers().is_empty());
    }

    #[test]
    fn apply_probe_folds_status_tools_and_scrubbed_errors() {
        let (_home, _workspace, mut manager) = test_manager("probe");
        manager
            .define(&draft("github", McpScope::Global, "npx"))
            .unwrap();
        manager.secrets.set("MCP_TOKEN", "topsecretvalue").unwrap();

        manager.begin_probe();
        assert!(manager.is_probing());
        manager.apply_probe(McpProbe {
            servers: vec![McpProbeServer {
                name: "github".into(),
                state: McpServerStatus::Failed,
                tools: vec![],
                tool_exposure: BTreeMap::new(),
                resources: 0,
                resource_templates: 0,
                error: Some("stderr said topsecretvalue".into()),
            }],
            errors: vec!["bad entry with topsecretvalue".into()],
            note: Some("project not trusted".into()),
            fatal: None,
        });
        let runtime = manager.runtime("github");
        assert_eq!(runtime.status, McpServerStatus::Failed);
        assert!(!runtime.error.unwrap().contains("topsecretvalue"));
        assert!(!manager.probe_errors()[0].contains("topsecretvalue"));
        assert_eq!(manager.note(), Some("project not trusted"));
        assert!(!manager.is_probing());
    }

    #[test]
    fn a_fatal_probe_marks_enabled_servers_failed() {
        let (_home, _workspace, mut manager) = test_manager("fatal");
        manager.define(&draft("a", McpScope::Global, "a")).unwrap();
        manager.define(&draft("b", McpScope::Global, "b")).unwrap();
        manager.set_enabled("b", false).unwrap();
        manager.apply_probe(McpProbe {
            fatal: Some("pi not found".into()),
            ..McpProbe::default()
        });
        assert_eq!(manager.runtime("a").status, McpServerStatus::Failed);
        assert_eq!(manager.runtime("b").status, McpServerStatus::Disabled);
    }

    #[test]
    fn refresh_picks_up_external_configuration_changes() {
        let (_home, workspace, mut manager) = test_manager("refresh");
        assert!(manager.servers().is_empty());
        // An external editor writes the project config while Orbit is open.
        let path = workspace.join(".pi").join("mcp.json");
        config::write_add(&path, "external", &stdio_def("node")).unwrap();
        manager.refresh();
        assert_eq!(manager.servers().len(), 1);
        let server = manager.server("external").unwrap();
        assert_eq!(server.scope, McpScope::Project);
        assert_eq!(server.source, path);
        // An external disable reads back as a configuration fact, not a probe
        // result — and the UI state flips with it.
        config::write_patch(&path, "external", config::McpPatch::Enabled(false)).unwrap();
        manager.refresh();
        assert!(!manager.server("external").unwrap().def.enabled);
        assert_eq!(
            manager.runtime("external").status,
            McpServerStatus::Disabled
        );
        // An external removal clears it from the domain state too.
        config::write_remove(&path, "external").unwrap();
        manager.refresh();
        assert!(manager.servers().is_empty());
        assert_eq!(manager.runtime("external").status, McpServerStatus::Unknown);
    }

    #[test]
    fn fingerprint_tracks_files_and_staleness() {
        let (_home, _workspace, mut manager) = test_manager("stamp");
        let stamp = manager.fingerprint();
        assert!(manager.fingerprint_matches(stamp));
        manager
            .define(&draft("github", McpScope::Global, "npx"))
            .unwrap();
        assert!(!manager.fingerprint_matches(stamp));
    }

    #[test]
    fn probe_json_parses_pis_shape() {
        let text = r#"{
          "servers": [
            {
              "name": "github",
              "scope": "project",
              "source": "/p/.pi/mcp.json",
              "enabled": true,
              "exposure": "codemode",
              "transport": "npx -y server",
              "state": "connected",
              "tools": ["search_repositories", "get_issue"],
              "toolExposure": { "get_issue": "direct" },
              "resources": 2,
              "resourceTemplates": 1
            },
            {
              "name": "broken",
              "state": "failed",
              "tools": [],
              "error": "spawn ENOENT"
            }
          ],
          "errors": ["bad entry"],
          "note": "ignored project config"
        }"#;
        let (servers, errors, note) = parse_probe_json(text).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].state, McpServerStatus::Connected);
        assert_eq!(servers[0].tools.len(), 2);
        assert_eq!(servers[0].tool_exposure["get_issue"], McpExposure::Direct);
        assert_eq!(servers[0].resources, 2);
        assert_eq!(servers[1].state, McpServerStatus::Failed);
        assert_eq!(servers[1].error.as_deref(), Some("spawn ENOENT"));
        assert_eq!(errors, vec!["bad entry"]);
        assert_eq!(note.as_deref(), Some("ignored project config"));
    }

    #[test]
    fn probe_json_tolerates_missing_optional_fields() {
        let (servers, errors, note) = parse_probe_json(r#"{"servers":[]}"#).unwrap();
        assert!(servers.is_empty() && errors.is_empty() && note.is_none());
        assert!(parse_probe_json("not json").is_err());
    }

    #[test]
    fn probe_json_requires_a_servers_array() {
        // A different shape from a changed or failing pi must be a probe
        // error, not an empty success that leaves stale status on screen.
        assert!(parse_probe_json("{}").is_err());
        assert!(parse_probe_json(r#"{"servers": {}}"#).is_err());
        assert!(parse_probe_json(r#"{"servers": null}"#).is_err());
    }

    /// A minimal stdio MCP server: `initialize`, `tools/list`, `tools/call`.
    const ECHO_SERVER_JS: &str = r#"
process.stdin.setEncoding('utf8');
let buf = '';
process.stdin.on('data', (chunk) => {
  buf += chunk;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i).trim();
    buf = buf.slice(i + 1);
    if (!line) continue;
    let msg; try { msg = JSON.parse(line); } catch { continue; }
    if (msg.method === 'initialize') {
      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result: {
        protocolVersion: '2024-11-05', capabilities: { tools: {} },
        serverInfo: { name: 'orbit-echo', version: '1.0.0' } } }) + '\n');
    } else if (msg.method === 'tools/list') {
      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result: { tools: [
        { name: 'echo', description: 'Echo', inputSchema: { type: 'object' } } ] } }) + '\n');
    } else if (msg.id !== undefined) {
      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result: { content: [] } }) + '\n');
    }
  }
});
"#;

    /// Live end-to-end: a config Orbit wrote is read back and connected by
    /// the real installed pi (`pi mcp list --json`), and the probe result
    /// parses into the domain model with the server's tool list. Ignored by
    /// default; run with `cargo test -p orbit-pi --bin orbit-pi
    /// live_probe_reads -- --ignored`.
    #[test]
    #[ignore = "requires the installed pi CLI and node"]
    fn live_probe_reads_a_server_orbit_wrote() {
        let root = temp_root("live");
        let workspace = root.join("project");
        let agent = root.join("agent");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&agent).unwrap();
        let server = root.join("echo.mjs");
        fs::write(&server, ECHO_SERVER_JS).unwrap();

        let mut def = stdio_def("node");
        if let McpTransport::Stdio { args, .. } = &mut def.transport {
            args.push(server.display().to_string());
        }
        config::write_add(&agent.join("mcp.json"), "echo", &def).unwrap();

        let outcome = probe_with_agent_dir(&workspace, &[], Some(&agent));
        assert!(outcome.fatal.is_none(), "fatal: {:?}", outcome.fatal);
        let echo = outcome
            .servers
            .iter()
            .find(|server| server.name == "echo")
            .expect("pi reported the Orbit-written server");
        assert_eq!(
            echo.state,
            McpServerStatus::Connected,
            "error: {:?}",
            echo.error
        );
        assert!(echo.tools.contains(&"echo".to_string()));
    }
}
