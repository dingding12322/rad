//! Settings → MCP: the server management surface.
//!
//! The page lists the servers Pi actually loads (read from Pi's own
//! `mcp.json` files), shows Pi's own connection status and tool counts, and
//! offers add / edit / enable / disable / test / remove. Every mutation is
//! written to the file that defines the server, then Pi is restarted *only*
//! when a running process would otherwise keep using stale configuration —
//! and that restart preserves the session (see [`OrbitApp::mcp_apply_now`]).
//!
//! Rendering follows the standard Settings anatomy: a pinned toolbar over a
//! scrolling body of grouped cards (see `app/settings.rs`). The add/edit form
//! is a modal in the provider-editor mould, sharing the composer's input
//! entity so text editing, selection, and IME behave like every other field.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::helpers::*;
use super::*;
use crate::mcp::config::{self as mcp_config, SecretUpdate};
use crate::mcp::manager::McpProbe;
use crate::mcp::{
    McpError, McpExposure, McpOAuth, McpScope, McpServer, McpServerDef, McpServerStatus,
    McpTransport, McpTransportKind,
};
use crate::theme::tokens::{
    input, list_item, modal, ButtonSize, DynamicSpacing, IconSize, Radius, StyledExt, TextSize,
};

/// How long after a configuration change the probe is debounced, so several
/// quick edits share one `pi mcp list` run.
const MCP_PROBE_DEBOUNCE: Duration = Duration::from_millis(500);
/// How long after a configuration change the Pi apply is debounced. Slightly
/// before the probe so the page updates as the restart begins.
const MCP_APPLY_DEBOUNCE: Duration = Duration::from_millis(350);
/// How often the page re-checks the config files for external edits while the
/// MCP section is open. A stat of two small files.
const MCP_EXTERNAL_POLL: Duration = Duration::from_secs(60);

/// The MCP add/edit modal's open state. Inputs are `ComposerInput` entities so
/// they get real text editing (selection, IME, clipboard) for free.
pub(super) struct McpEditor {
    /// The server being edited, `None` when adding.
    pub(super) original: Option<McpServer>,
    pub(super) kind: McpTransportKind,
    pub(super) name: Entity<ComposerInput>,
    pub(super) description: Entity<ComposerInput>,
    pub(super) command: Entity<ComposerInput>,
    pub(super) args: Entity<ComposerInput>,
    pub(super) env: Entity<ComposerInput>,
    pub(super) url: Entity<ComposerInput>,
    pub(super) headers: Entity<ComposerInput>,
    pub(super) exposure: McpExposure,
    pub(super) scope: McpScope,
    /// OAuth client settings are opt-in and collapsed by default; most OAuth
    /// servers need none of them (Pi registers itself dynamically).
    pub(super) oauth_open: bool,
    pub(super) oauth_client_id: Entity<ComposerInput>,
    pub(super) oauth_client_secret: Entity<ComposerInput>,
    pub(super) oauth_callback_port: Entity<ComposerInput>,
    pub(super) oauth_callback_url: Entity<ComposerInput>,
    pub(super) oauth_scope: Entity<ComposerInput>,
    /// Validation/save error shown inside the modal.
    pub(super) error: Option<String>,
}

/// The in-flight OAuth sign-in: which server, and the flag the waiting
/// background task polls so Cancel can abort it.
pub(super) struct McpAuthJob {
    pub(super) server: String,
    cancel: Arc<AtomicBool>,
}

impl OrbitApp {
    // ── lifecycle ─────────────────────────────────────────────────────────

    /// Called when Settings → MCP opens: point the manager at the active
    /// workspace (project scope follows it) and schedule a fresh probe.
    pub(super) fn mcp_section_opened(&mut self, cx: &mut Context<Self>) {
        self.mcp.set_workspace(self.current_workspace.as_deref());
        self.schedule_mcp_probe(Duration::ZERO, cx);
        self.mcp_focus_pending = true;
        cx.notify();
    }

    /// Heartbeat hook: external-change detection, the debounced probe, and the
    /// debounced Pi apply. Cheap when nothing is due.
    pub(super) fn tick_mcp(&mut self, cx: &mut Context<Self>) {
        if self.mcp_settled_ws() != self.current_workspace {
            self.mcp.set_workspace(self.current_workspace.as_deref());
            self.schedule_mcp_probe(Duration::ZERO, cx);
        }
        // External edits (`mcp.json` touched outside Orbit) are picked up
        // read-only; applying them to a live session still needs the user's
        // apply/restart action, so nothing is overwritten behind their back.
        if self.settings_open
            && self.settings_section == SettingsSection::Mcp
            && self
                .mcp_external_check_at
                .is_none_or(|at| Instant::now() >= at)
        {
            self.mcp_external_check_at = Some(Instant::now() + MCP_EXTERNAL_POLL);
            if !self.mcp.fingerprint_matches(self.mcp.disk_fingerprint()) {
                self.mcp.refresh();
                self.mcp_external_change = true;
                self.schedule_mcp_probe(Duration::ZERO, cx);
            }
        }
        if let Some(at) = self.mcp_probe_at {
            if Instant::now() >= at && !self.mcp.is_probing() {
                self.mcp_probe_at = None;
                self.spawn_mcp_probe(cx);
            }
        }
        if let Some(at) = self.mcp_apply_at {
            if Instant::now() >= at {
                self.mcp_apply_at = None;
                if !self.busy && !self.is_compacting {
                    self.mcp_apply_now(cx);
                } else {
                    self.mcp_apply_pending = true;
                }
            }
        }
    }

    /// The workspace the manager is pointed at, without borrowing it.
    fn mcp_settled_ws(&self) -> Option<PathBuf> {
        self.mcp.workspace().map(Path::to_path_buf)
    }

    /// Queue a probe `delay` from now (debounced).
    pub(super) fn schedule_mcp_probe(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.mcp_probe_at = Some(Instant::now() + delay);
        cx.notify();
    }

    /// Run the blocking `pi mcp list --json` probe on the background executor
    /// and fold the result back into the manager on the UI thread.
    fn spawn_mcp_probe(&mut self, cx: &mut Context<Self>) {
        let workspace = self
            .mcp
            .workspace()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.workspace_dir());
        let env = self.mcp.secret_env();
        self.mcp.begin_probe();
        cx.spawn(async move |this, cx| {
            let probe = cx
                .background_executor()
                .spawn(async move { crate::mcp::manager::probe(&workspace, &env) })
                .await;
            let _ = this.update(cx, |app, cx| app.on_mcp_probe(probe, cx));
        })
        .detach();
        cx.notify();
    }

    fn on_mcp_probe(&mut self, probe: McpProbe, cx: &mut Context<Self>) {
        self.mcp.apply_probe(probe);
        // A "Test connection" click toasts its own server's outcome once the
        // shared probe lands. A server removed before the probe finished has
        // no result to report.
        if let Some(name) = self.mcp_test_target.take() {
            if self.mcp.server(&name).is_none() {
                cx.notify();
                return;
            }
            let runtime = self.mcp.runtime(&name);
            match runtime.status {
                McpServerStatus::Connected => {
                    self.toast_success(tr!(
                        "mcp.connected_toast",
                        name = name,
                        count = runtime.tools.len()
                    ));
                }
                McpServerStatus::Disabled => {
                    self.toast_info(tr!("mcp.disabled_toast", name = name));
                }
                _ => {
                    let detail = runtime
                        .error
                        .clone()
                        .or_else(|| self.mcp.probe_errors().first().cloned())
                        .unwrap_or_else(|| tr!("mcp.no_details"));
                    let detail = match mcp_error_hint(&detail) {
                        Some(hint) => format!("{detail} {hint}"),
                        None => detail,
                    };
                    self.toast_error(tr!("mcp.test_failed_toast", name = name, detail = detail));
                }
            }
        }
        // The apply may have a probe result fresher than the one the user
        // acted on; nothing else to do here.
        cx.notify();
    }

    /// Any Orbit-made configuration change lands here: re-read state, then
    /// schedule the probe and the safe Pi apply.
    pub(super) fn mcp_config_changed(&mut self, cx: &mut Context<Self>) {
        self.mcp.refresh();
        self.mcp_external_change = false;
        self.schedule_mcp_probe(MCP_PROBE_DEBOUNCE, cx);
        self.mcp_apply_at = Some(Instant::now() + MCP_APPLY_DEBOUNCE);
        cx.notify();
    }

    /// Restart the active Pi process so it loads the current `mcp.json` while
    /// preserving the conversation. A run in flight defers the restart until
    /// it settles — the user is told once, quietly.
    pub(super) fn mcp_apply_now(&mut self, cx: &mut Context<Self>) {
        self.mcp_apply_pending = false;
        let forced = std::mem::take(&mut self.mcp_apply_forced);
        if self.client.is_none() || !self.runtime_is_live() {
            // Nothing running to refresh; the next spawn reads the new config.
            return;
        }
        if self.busy || self.is_compacting {
            self.mcp_apply_pending = true;
            // The deferred apply must remember it was forced (sign-out).
            self.mcp_apply_forced = forced;
            self.toast_info(tr!("mcp.apply_after_run"));
            cx.notify();
            return;
        }
        if !forced && self.mcp.fingerprint_matches(self.mcp_stamp) {
            return;
        }
        let Some(session_path) = self.current_session_path.clone() else {
            return;
        };
        if self.session_id.is_none() {
            // A brand-new process that has not answered `get_state` yet: the
            // next spawn already follows the new config; do not restart under
            // a session that does not exist.
            return;
        }
        let workspace = self.workspace_dir();
        self.drop_client();
        match self
            .extensions
            .spawn(&workspace, false, &self.mcp.secret_env())
        {
            Ok(client) => {
                self.adopt_client(client);
                self.mcp_stamp = self.mcp.fingerprint_for(Some(&workspace));
                self.send(
                    CommandBody::SwitchSession {
                        session_path: session_path.to_string_lossy().into_owned(),
                    },
                    "switch_session",
                );
                self.set_status(tr!("mcp.applied_restarting"));
                self.mcp.log_event(
                    "pi_restarted",
                    &format!("session=\"{}\"", session_path.display()),
                );
            }
            Err(err) => {
                let message = tr!("mcp.error_apply_spawn", error = err);
                self.mcp.log_event("pi_restart_failed", &message);
                self.runtime.error = Some(message.clone());
                self.toast_error(message);
            }
        }
        cx.notify();
    }

    /// Called on `agent_settled`: schedule the deferred MCP restart for the
    /// next heartbeat now that the run is over. Scheduling (rather than
    /// restarting inside the event drain) keeps the process swap out of the
    /// loop still processing that client's remaining events.
    pub(super) fn mcp_after_settle(&mut self, cx: &mut Context<Self>) {
        if self.mcp_apply_pending {
            self.mcp_apply_at = Some(Instant::now());
            cx.notify();
        }
    }

    /// Whether a running Pi process is behind the on-disk MCP configuration.
    pub(super) fn mcp_restart_needed(&self) -> bool {
        self.client.is_some()
            && self.runtime_is_live()
            && !self.mcp.fingerprint_matches(self.mcp_stamp)
    }

    /// Explicit "Apply to running session" from the page's banner.
    pub(super) fn mcp_apply_clicked(&mut self, cx: &mut Context<Self>) {
        self.mcp.refresh();
        self.mcp_external_change = false;
        self.mcp_apply_at = Some(Instant::now() + MCP_APPLY_DEBOUNCE);
        cx.notify();
    }

    /// Request a restart that must happen even when the config files did not
    /// change (after sign-out, so the live session drops its access).
    pub(super) fn mcp_force_apply(&mut self, cx: &mut Context<Self>) {
        self.mcp_apply_forced = true;
        self.mcp_apply_at = Some(Instant::now() + MCP_APPLY_DEBOUNCE);
        cx.notify();
    }

    // ── actions ───────────────────────────────────────────────────────────

    /// Re-read the files and probe immediately (the toolbar's Refresh).
    pub(super) fn mcp_refresh(&mut self, cx: &mut Context<Self>) {
        self.mcp.set_workspace(self.current_workspace.as_deref());
        self.mcp.refresh();
        self.mcp_external_change = false;
        self.schedule_mcp_probe(Duration::ZERO, cx);
    }

    pub(super) fn mcp_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_mcp_editor(None, McpTransportKind::Stdio, window, cx);
    }

    pub(super) fn mcp_edit(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.mcp.server(name).cloned() else {
            return;
        };
        let kind = server.def.transport.kind();
        self.open_mcp_editor(Some(server), kind, window, cx);
    }

    fn open_mcp_editor(
        &mut self,
        server: Option<McpServer>,
        kind: McpTransportKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_text = server
            .as_ref()
            .map(|server| server.name.clone())
            .unwrap_or_default();
        let description_text = server
            .as_ref()
            .and_then(|server| server.description.clone())
            .unwrap_or_default();
        let (command_text, args_text, env_text, url_text, headers_text, exposure, scope) =
            match &server {
                Some(server) => {
                    let exposure = server.def.exposure;
                    let scope = server.scope;
                    match &server.def.transport {
                        McpTransport::Stdio {
                            command, args, env, ..
                        } => (
                            command.clone(),
                            args.join("\n"),
                            mcp_config::env_lines(env),
                            String::new(),
                            String::new(),
                            exposure,
                            scope,
                        ),
                        McpTransport::StreamableHttp { url, headers } => (
                            String::new(),
                            String::new(),
                            String::new(),
                            url.clone(),
                            mcp_config::header_lines(headers),
                            exposure,
                            scope,
                        ),
                    }
                }
                None => (
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    McpExposure::Codemode,
                    McpScope::Global,
                ),
            };

        let oauth = server
            .as_ref()
            .and_then(|server| server.def.oauth.clone())
            .unwrap_or_default();
        let oauth_text = |value: &Option<String>| value.clone().unwrap_or_default();
        let oauth_secret_text = match &oauth.client_secret {
            Some(secret) => mcp_config::display_secret_value(secret).into_owned(),
            None => String::new(),
        };
        let oauth_port_text = oauth
            .callback_port
            .map(|port| port.to_string())
            .unwrap_or_default();
        let oauth_open = !oauth.is_empty();

        let field = |id: &'static str,
                     placeholder_key: &'static str,
                     lines: usize,
                     text: String,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                ComposerInput::new(cx)
                    .with_element_id(id)
                    .with_key_context("Composer Picker")
                    .with_max_lines(lines)
                    .with_placeholder_key(placeholder_key)
                    .with_text(text)
            })
        };

        let name = field("mcp-editor-name", "mcp.name_placeholder", 1, name_text, cx);
        let description = field(
            "mcp-editor-description",
            "mcp.description_placeholder",
            1,
            description_text,
            cx,
        );
        let command = field(
            "mcp-editor-command",
            "mcp.command_placeholder",
            1,
            command_text,
            cx,
        );
        let args = field("mcp-editor-args", "mcp.args_placeholder", 4, args_text, cx);
        let env = field("mcp-editor-env", "mcp.env_placeholder", 5, env_text, cx);
        let url = field("mcp-editor-url", "mcp.url_placeholder", 1, url_text, cx);
        let headers = field(
            "mcp-editor-headers",
            "mcp.headers_placeholder",
            5,
            headers_text,
            cx,
        );
        let oauth_client_id = field(
            "mcp-editor-oauth-client-id",
            "mcp.oauth_client_id_placeholder",
            1,
            oauth_text(&oauth.client_id),
            cx,
        );
        let oauth_client_secret = field(
            "mcp-editor-oauth-client-secret",
            "mcp.oauth_client_secret_placeholder",
            1,
            oauth_secret_text,
            cx,
        );
        let oauth_callback_port = field(
            "mcp-editor-oauth-callback-port",
            "mcp.oauth_callback_port_placeholder",
            1,
            oauth_port_text,
            cx,
        );
        let oauth_callback_url = field(
            "mcp-editor-oauth-callback-url",
            "mcp.oauth_callback_url_placeholder",
            1,
            oauth_text(&oauth.callback_url),
            cx,
        );
        let oauth_scope = field(
            "mcp-editor-oauth-scope",
            "mcp.oauth_scope_placeholder",
            1,
            oauth_text(&oauth.scope),
            cx,
        );
        window.focus(&name.read(cx).focus_handle(cx));
        self.mcp_editor = Some(McpEditor {
            original: server,
            kind,
            name,
            description,
            command,
            args,
            env,
            url,
            headers,
            exposure,
            scope,
            oauth_open,
            oauth_client_id,
            oauth_client_secret,
            oauth_callback_port,
            oauth_callback_url,
            oauth_scope,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn mcp_editor_close(&mut self, cx: &mut Context<Self>) {
        if self.mcp_editor.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn mcp_editor_cancel(
        &mut self,
        _: &crate::PickerCancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mcp_editor_close(cx);
    }

    pub(super) fn mcp_editor_confirm(
        &mut self,
        _: &crate::PickerConfirm,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mcp_editor_save(cx);
    }

    /// Validate the form and write it. Secrets typed as literals are stored in
    /// Orbit's secret store and referenced as `${NAME}` in `mcp.json`.
    pub(super) fn mcp_editor_save(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.mcp_editor.as_ref() else {
            return;
        };
        let original = editor.original.clone();
        let kind = editor.kind;
        let scope = editor.scope;
        let exposure = editor.exposure;
        let name = match &original {
            Some(server) => server.name.clone(),
            None => editor.name.read(cx).text().trim().to_string(),
        };
        let description = editor.description.read(cx).text().trim().to_string();
        let command = editor.command.read(cx).text().trim().to_string();
        let args_text = editor.args.read(cx).text();
        let env_text = editor.env.read(cx).text();
        let url = editor.url.read(cx).text().trim().to_string();
        let headers_text = editor.headers.read(cx).text();
        let oauth_client_id = editor.oauth_client_id.read(cx).text().trim().to_string();
        let oauth_client_secret_typed = editor
            .oauth_client_secret
            .read(cx)
            .text()
            .trim()
            .to_string();
        let oauth_callback_port_text = editor
            .oauth_callback_port
            .read(cx)
            .text()
            .trim()
            .to_string();
        let oauth_callback_url = editor.oauth_callback_url.read(cx).text().trim().to_string();
        let oauth_scope = editor.oauth_scope.read(cx).text().trim().to_string();

        let mut new_secrets: Vec<SecretUpdate> = Vec::new();
        // Namespace generated secret names by the scope they are saved under
        // (and, for project scope, the manager's workspace) so two projects —
        // or a project and the global scope — can never share one entry.
        let workspace = self.mcp.workspace().map(Path::to_path_buf);
        let mut resolve_map = |typed: BTreeMap<String, String>| {
            let mut resolved = BTreeMap::new();
            for (key, value) in typed {
                let original_value = original
                    .as_ref()
                    .and_then(|server| server.def.transport.secret_values().get(&key))
                    .map(String::as_str);
                let generated =
                    mcp_config::generated_secret_name(&name, &key, scope, workspace.as_deref());
                let resolution =
                    mcp_config::resolve_typed_secret(&value, original_value, &generated);
                if let Some(update) = resolution.store {
                    new_secrets.push(update);
                }
                resolved.insert(key, resolution.value);
            }
            resolved
        };

        let parsed = match kind {
            McpTransportKind::Stdio => {
                mcp_config::parse_env_lines(&env_text).map(|env| (env, BTreeMap::new()))
            }
            McpTransportKind::StreamableHttp => mcp_config::parse_header_lines(&headers_text)
                .map(|headers| (BTreeMap::new(), headers)),
        };
        let (env, headers) = match parsed {
            Ok(parts) => (resolve_map(parts.0), resolve_map(parts.1)),
            Err(error) => {
                self.set_mcp_editor_error(error, cx);
                return;
            }
        };

        let transport = match kind {
            McpTransportKind::Stdio => {
                let args = args_text
                    .lines()
                    .map(str::trim)
                    .filter(|arg| !arg.is_empty())
                    .map(str::to_string)
                    .collect();
                // `cwd` is not edited in the form; preserve it on update.
                let cwd = original
                    .as_ref()
                    .and_then(|server| match &server.def.transport {
                        McpTransport::Stdio { cwd, .. } => cwd.clone(),
                        McpTransport::StreamableHttp { .. } => None,
                    });
                McpTransport::Stdio {
                    command,
                    args,
                    env,
                    cwd,
                }
            }
            McpTransportKind::StreamableHttp => McpTransport::StreamableHttp { url, headers },
        };

        // OAuth client settings apply to HTTP servers only; an empty section
        // means `None` — Pi's dynamic client registration needs nothing.
        let non_empty = |value: String| (!value.is_empty()).then_some(value);
        let oauth: Option<McpOAuth> = if kind == McpTransportKind::StreamableHttp {
            let callback_port = if oauth_callback_port_text.is_empty() {
                None
            } else {
                match oauth_callback_port_text.parse::<u64>() {
                    Ok(port) if port > 0 && port <= u64::from(u16::MAX) => Some(port),
                    _ => {
                        self.set_mcp_editor_error(tr!("mcp.error_oauth_port"), cx);
                        return;
                    }
                }
            };
            let original_secret = original
                .as_ref()
                .and_then(|server| server.def.oauth.as_ref())
                .and_then(|oauth| oauth.client_secret.as_deref());
            let generated = mcp_config::generated_secret_name(
                &name,
                "OAuthClientSecret",
                scope,
                workspace.as_deref(),
            );
            let secret = mcp_config::resolve_typed_secret(
                &oauth_client_secret_typed,
                original_secret,
                &generated,
            );
            if let Some(update) = secret.store {
                new_secrets.push(update);
            }
            let empty = oauth_client_id.is_empty()
                && secret.value.is_empty()
                && callback_port.is_none()
                && oauth_callback_url.is_empty()
                && oauth_scope.is_empty();
            (!empty).then(|| McpOAuth {
                client_id: non_empty(oauth_client_id),
                client_secret: non_empty(secret.value),
                callback_port,
                callback_url: non_empty(oauth_callback_url),
                scope: non_empty(oauth_scope),
            })
        } else {
            None
        };

        let def = McpServerDef {
            transport,
            exposure,
            tool_exposure: original
                .as_ref()
                .map(|server| server.def.tool_exposure.clone())
                .unwrap_or_default(),
            enabled: original
                .as_ref()
                .map(|server| server.def.enabled)
                .unwrap_or(true),
            timeout: original.as_ref().and_then(|server| server.def.timeout),
            oauth,
        };
        let draft = crate::mcp::manager::McpDraft {
            original: original
                .as_ref()
                .map(|server| (server.name.clone(), server.scope)),
            name: name.clone(),
            scope,
            description: Some(description),
            def,
            new_secrets,
        };
        match self.mcp.define(&draft) {
            Ok(()) => {
                self.mcp_editor = None;
                self.toast_success(tr!("mcp.saved_toast", name = name));
                self.mcp_config_changed(cx);
            }
            Err(error) => self.set_mcp_editor_error(error.user_message(), cx),
        }
    }

    fn set_mcp_editor_error(&mut self, error: String, cx: &mut Context<Self>) {
        if let Some(editor) = self.mcp_editor.as_mut() {
            editor.error = Some(error);
        }
        cx.notify();
    }

    pub(super) fn mcp_toggle_server(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(server) = self.mcp.server(&name) else {
            return;
        };
        let enabled = !server.def.enabled;
        match self.mcp.set_enabled(&name, enabled) {
            Ok(()) => {
                self.toast_info(if enabled {
                    tr!("mcp.enabled_toast", name = name)
                } else {
                    tr!("mcp.disabled_toast", name = name)
                });
                self.mcp_config_changed(cx);
            }
            Err(error) => self.set_error(error.user_message()),
        }
        cx.notify();
    }

    /// Probe one server's connection. `pi mcp list` connects every enabled
    /// server at once, so this marks the server as the probe's toast target
    /// and runs the shared probe.
    pub(super) fn mcp_test_server(&mut self, name: String, cx: &mut Context<Self>) {
        if self.mcp.server(&name).is_none() {
            return;
        }
        self.mcp_test_target = Some(name);
        self.schedule_mcp_probe(Duration::ZERO, cx);
    }

    /// "Reconnect" verifies with a probe and, when the running process is
    /// behind the current configuration, restarts it so the server
    /// reconnects. Pi otherwise reconnects a dropped server lazily on the
    /// next call.
    pub(super) fn mcp_reconnect_server(&mut self, name: String, cx: &mut Context<Self>) {
        self.mcp_test_target = Some(name);
        self.schedule_mcp_probe(Duration::ZERO, cx);
        if self.mcp_restart_needed() {
            self.mcp_apply_at = Some(Instant::now() + MCP_APPLY_DEBOUNCE);
        }
        cx.notify();
    }

    pub(super) fn mcp_request_remove(&mut self, name: String, cx: &mut Context<Self>) {
        self.mcp_remove_confirm = Some(name);
        cx.notify();
    }

    pub(super) fn mcp_cancel_remove(&mut self, cx: &mut Context<Self>) {
        if self.mcp_remove_confirm.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn mcp_confirm_remove(&mut self, name: String, cx: &mut Context<Self>) {
        self.mcp_remove_confirm = None;
        match self.mcp.remove(&name) {
            Ok(()) => {
                self.toast_success(tr!("mcp.removed_toast", name = name));
                self.mcp_config_changed(cx);
            }
            Err(error) => self.set_error(error.user_message()),
        }
        cx.notify();
    }

    /// Reconnect every server: a restart of the active Pi process with the
    /// current configuration. This is the palette's "Reconnect All".
    pub(super) fn mcp_reconnect_all(&mut self, cx: &mut Context<Self>) {
        self.mcp.refresh();
        self.mcp.set_workspace(self.current_workspace.as_deref());
        self.schedule_mcp_probe(Duration::ZERO, cx);
        if self.mcp_restart_needed() {
            self.mcp_apply_at = Some(Instant::now() + MCP_APPLY_DEBOUNCE);
            self.toast_info(tr!("mcp.applying"));
        } else {
            self.toast_info(tr!("mcp.reconnecting"));
        }
        cx.notify();
    }

    // ── OAuth ─────────────────────────────────────────────────────────────

    /// Start Pi's browser sign-in for one OAuth server. The browser page is
    /// Pi's; Orbit waits (and can cancel) and refreshes the probe when the
    /// tokens land.
    pub(super) fn mcp_sign_in(&mut self, name: String, cx: &mut Context<Self>) {
        if self.mcp_auth.is_some() {
            return;
        }
        let Some(server) = self.mcp.server(&name) else {
            return;
        };
        if !server.oauth_candidate() {
            return;
        }
        let workspace = self.workspace_dir();
        let env = self.mcp.secret_env();
        let cancel = Arc::new(AtomicBool::new(false));
        self.mcp_auth = Some(McpAuthJob {
            server: name.clone(),
            cancel: Arc::clone(&cancel),
        });
        self.toast_info(tr!("mcp.sign_in_waiting", name = name));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::mcp::manager::login(
                        &workspace,
                        &env,
                        &name,
                        crate::mcp::manager::LOGIN_TIMEOUT,
                        Some(&cancel),
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| app.on_mcp_auth(result, cx));
        })
        .detach();
        cx.notify();
    }

    fn on_mcp_auth(&mut self, result: Result<(), McpError>, cx: &mut Context<Self>) {
        let Some(job) = self.mcp_auth.take() else {
            return;
        };
        let name = job.server;
        match result {
            Ok(()) => {
                self.toast_success(tr!("mcp.signed_in_toast", name = name));
                self.mcp.refresh();
                self.schedule_mcp_probe(Duration::from_millis(250), cx);
            }
            Err(McpError::Cancelled) => {
                self.toast_info(tr!("mcp.sign_in_cancelled", name = name));
            }
            Err(error) => {
                let error = error.redacted(|text| self.mcp.redact_text(text));
                self.toast_error(error.user_message());
            }
        }
        cx.notify();
    }

    /// Abort a waiting sign-in; the background task kills Pi's process.
    pub(super) fn mcp_cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.mcp_auth {
            job.cancel.store(true, Ordering::Relaxed);
            self.toast_info(tr!("mcp.sign_in_cancelling", name = job.server.clone()));
            cx.notify();
        }
    }

    /// Delete Pi's stored OAuth credentials for one server.
    pub(super) fn mcp_sign_out(&mut self, name: String, cx: &mut Context<Self>) {
        if self.mcp_auth.is_some() {
            return;
        }
        let workspace = self.workspace_dir();
        let env = self.mcp.secret_env();
        let task_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::mcp::manager::logout(&workspace, &env, &task_name) })
                .await;
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(()) => {
                        app.toast_success(tr!("mcp.signed_out_toast", name = name));
                        app.mcp.refresh();
                        app.schedule_mcp_probe(Duration::from_millis(250), cx);
                        // Signing out must also end a live session's access;
                        // Pi keeps the token in memory until the process
                        // restarts, so this apply is forced.
                        app.mcp_force_apply(cx);
                    }
                    Err(error) => {
                        let error = error.redacted(|text| app.mcp.redact_text(text));
                        app.toast_error(error.user_message());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ── rendering: toolbar ────────────────────────────────────────────────

    /// The status bar's MCP chip: enabled-server count and aggregate state.
    /// `None` when nothing is configured, so it costs no toolbar space for
    /// users who do not use MCP. Clicking opens Settings → MCP.
    pub(super) fn mcp_status_chip(&self, theme: Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let (dot, label) = self.mcp_chip_parts(theme)?;
        // A fully connected fleet is the steady state; the chip only earns
        // its place when a server needs attention, so the status bar stays
        // quiet when there is nothing to act on.
        if self.mcp_all_connected() {
            return None;
        }
        let entity = cx.entity();
        Some(
            button_frame(div().id("status-mcp"), &theme, ButtonSize::Default)
                .cursor_pointer()
                .hover(|style| {
                    style
                        .bg(theme.surface_hover)
                        .text_color(theme.text_secondary)
                })
                .tip(tr!("mcp.command_open"))
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    entity.update(cx, |app, cx| {
                        app.settings_open = true;
                        app.set_settings_section(SettingsSection::Mcp, cx);
                        app.sync_terminal_active(cx);
                    });
                })
                .child(icon(
                    "icons/tools/mcp.svg",
                    ButtonSize::Default.icon_size().px(&theme),
                    theme.text_muted,
                ))
                .child(div().size(px(6.)).rounded_full().bg(dot))
                .child(div().text_color(theme.text_secondary).child(label))
                .into_any_element(),
        )
    }

    /// The aggregate MCP state shared by the status-bar chip and the quiet
    /// top-bar usage pill: a tone dot plus a label that names the counts.
    /// `None` when no enabled server is configured.
    pub(super) fn mcp_chip_parts(&self, theme: Theme) -> Option<(Hsla, String)> {
        let enabled: Vec<&McpServer> = self
            .mcp
            .servers()
            .iter()
            .filter(|server| server.def.enabled)
            .collect();
        if enabled.is_empty() {
            return None;
        }
        let total = enabled.len();
        let mut connected = 0usize;
        let mut failed = 0usize;
        let mut needs_auth = 0usize;
        for server in &enabled {
            match self.mcp.runtime(&server.name).status {
                McpServerStatus::Connected => connected += 1,
                McpServerStatus::Failed => failed += 1,
                McpServerStatus::NeedsAuth => needs_auth += 1,
                _ => {}
            }
        }
        let (dot, label) = if failed > 0 {
            (
                theme.error,
                tr!(
                    "mcp.chip_failed",
                    connected = connected,
                    total = total,
                    failed = failed
                ),
            )
        } else if needs_auth > 0 {
            (
                theme.warning,
                tr!("mcp.chip_auth", connected = connected, total = total),
            )
        } else if connected == total {
            (
                theme.success,
                tr!("mcp.chip_connected", connected = connected, total = total),
            )
        } else {
            (
                theme.text_muted,
                tr!("mcp.chip_idle", connected = connected, total = total),
            )
        };
        Some((dot, label))
    }

    /// Whether every enabled MCP server is connected — the quiet steady state
    /// where the status-bar chip hides.
    pub(super) fn mcp_all_connected(&self) -> bool {
        let enabled: Vec<_> = self
            .mcp
            .servers()
            .iter()
            .filter(|server| server.def.enabled)
            .collect();
        !enabled.is_empty()
            && enabled.iter().all(|server| {
                matches!(
                    self.mcp.runtime(&server.name).status,
                    McpServerStatus::Connected
                )
            })
    }

    /// The pinned MCP toolbar: search, a status line, Refresh, and Add.
    pub(super) fn mcp_toolbar(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let search = picker_search_frame(div(), &theme)
            .w_full()
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.mcp_filter.clone()));

        let refresh = icon_button_frame(div().id("mcp-refresh"), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .tip(tr!("mcp.refresh"))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| this.update(cx, |app, cx| app.mcp_refresh(cx))
            })
            .child(refresh_glyph(
                "mcp-refresh-spin",
                ButtonSize::Large.icon_size().px(&theme),
                self.mcp.is_probing(),
                theme.text_secondary,
                theme,
            ));

        let add = button_frame(div().id("mcp-add"), &theme, ButtonSize::Large)
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .raised(theme.action, &theme)
            .text_color(theme.action_fg)
            .hover(|style| style.raised(theme.action_hover, &theme))
            .active(|style| style.raised(theme.action_pressed, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, window, cx| this.update(cx, |app, cx| app.mcp_add(window, cx))
            })
            .child(icon(
                "icons/plus.svg",
                ButtonSize::Large.icon_size().px(&theme),
                theme.action_fg,
            ))
            .child(tr!("mcp.add_server"));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(div().flex_1().min_w_0().child(search))
                    .child(refresh)
                    .child(add),
            )
            .child(self.mcp_status_line(theme))
            .into_any_element()
    }

    /// One muted line under the toolbar: probe state, paths, and the count.
    fn mcp_status_line(&self, theme: Theme) -> AnyElement {
        let label = if self.mcp.is_probing() {
            tr!("mcp.checking")
        } else if let Some(at) = self.mcp.last_probed() {
            tr!("mcp.checked_ago", seconds = at.elapsed().as_secs().max(1))
        } else if self.mcp.is_empty() {
            tr!("mcp.no_servers")
        } else {
            tr!("mcp.not_checked")
        };
        div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(label)
            .when(!self.mcp.servers().is_empty(), |row| {
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(tr!("mcp.server_count", count = self.mcp.servers().len())),
                )
            })
            .when(!self.mcp.servers().is_empty(), |row| {
                row.child(tr!("mcp.keys_hint"))
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(mcp_config_dir_label(self.mcp.global_path())),
            )
            .when_some(self.mcp.project_path(), |row, path| {
                row.child(div().min_w_0().truncate().child(tr!(
                    "mcp.project_dir_label",
                    path = mcp_config_dir_label(path)
                )))
            })
            .into_any_element()
    }

    // ── rendering: body ───────────────────────────────────────────────────

    /// The MCP body: errors, external-change notice, scope groups, or the
    /// empty state. Rendered by `settings_rows` for the MCP section.
    pub(super) fn mcp_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = Vec::new();

        // Orbit's parser and Pi's probe report the same file problems with
        // different wording; prefer Orbit's richer entry-level messages and
        // only fall back to Pi's when Orbit found nothing.
        let config_errors = self.mcp.errors();
        for error in config_errors {
            rows.push(self.mcp_notice_card(theme, tr!("mcp.config_error"), error));
        }
        if config_errors.is_empty() {
            for error in self.mcp.probe_errors() {
                rows.push(self.mcp_notice_card(theme, tr!("mcp.config_error"), error));
            }
        }
        if let Some(note) = self.mcp.note() {
            rows.push(self.mcp_notice_card(theme, tr!("mcp.project_untrusted"), note));
        }
        if self.mcp_external_change {
            rows.push(self.mcp_external_change_card(theme, this.clone(), cx));
        }
        if self.mcp_restart_needed() {
            rows.push(self.mcp_restart_card(theme, this.clone(), cx));
        }
        // With `autoEnableCodemode: false`, codemode-exposed tools are not
        // reachable by the model; say so rather than showing a green server
        // whose tools never appear.
        if self.mcp.auto_enable_codemode() == Some(false)
            && self.mcp.servers().iter().any(|server| {
                server.def.enabled
                    && matches!(
                        server.def.exposure,
                        McpExposure::Codemode | McpExposure::CodemodeDeferred
                    )
            })
        {
            rows.push(self.mcp_notice_card(
                theme,
                tr!("mcp.codemode_disabled_title"),
                &tr!("mcp.codemode_disabled_body"),
            ));
        }

        let needle = self.mcp_filter.read(cx).text().trim().to_lowercase();
        let mut visible: Vec<&McpServer> = self
            .mcp
            .servers()
            .iter()
            .filter(|server| mcp_server_matches(server, &needle))
            .collect();

        if self.mcp.is_empty() {
            rows.push(self.mcp_empty_card(theme, this.clone(), cx));
            return rows;
        }
        if visible.is_empty() {
            rows.push(self.setting_row(
                theme,
                &tr!("mcp.no_matches"),
                Some(&tr!("mcp.no_matches_hint")),
                None,
                None,
            ));
            return rows;
        }

        visible.sort_by(|a, b| a.scope.cmp(&b.scope).then(a.name.cmp(&b.name)));

        // The server list owns keyboard focus while the page is open: ↑/↓ walk
        // the rows, Enter opens the editor, Space toggles the server.
        let this_for_keys = this.clone();
        let mut list = div()
            .id("mcp-list")
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base20.px(&theme))
            .track_focus(&self.mcp_list_focus)
            .key_context("McpList")
            .on_key_down(move |event, window, cx| {
                let key = event.keystroke.key.clone();
                this_for_keys.update(cx, |app, cx| app.mcp_handle_key(&key, window, cx));
            });
        for scope in McpScope::ALL {
            let scoped: Vec<&McpServer> = visible
                .iter()
                .copied()
                .filter(|server| server.scope == scope)
                .collect();
            if scoped.is_empty() {
                continue;
            }
            // A row's hover fill is full-bleed inside the board, and gpui
            // clips children to a rectangle (not the border radius), so the
            // first and last rows carry the board's own corners — otherwise
            // the highlight squares off the rounded border.
            let count = scoped.len();
            let group: Vec<AnyElement> = scoped
                .iter()
                .enumerate()
                .map(|(ix, server)| {
                    self.mcp_server_row(server, ix == 0, ix + 1 == count, theme, this.clone(), cx)
                })
                .collect();
            list = list.child(self.settings_section(
                theme,
                &tr!(scope.label_key()),
                vec![self.settings_group(theme, group)],
            ));
        }
        rows.push(list.into_any_element());
        rows
    }

    /// The visible servers' names in display order — the keyboard cursor's
    /// index space.
    fn mcp_visible_names(&self, cx: &Context<Self>) -> Vec<String> {
        let needle = self.mcp_filter.read(cx).text().trim().to_lowercase();
        let mut servers: Vec<&McpServer> = self
            .mcp
            .servers()
            .iter()
            .filter(|server| mcp_server_matches(server, &needle))
            .collect();
        servers.sort_by(|a, b| a.scope.cmp(&b.scope).then(a.name.cmp(&b.name)));
        servers
            .into_iter()
            .map(|server| server.name.clone())
            .collect()
    }

    /// One key for the MCP list (the container's `on_key_down`).
    pub(super) fn mcp_handle_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The modal owns the keyboard while open.
        if self.mcp_editor.is_some() {
            return;
        }
        let names = self.mcp_visible_names(cx);
        if names.is_empty() {
            return;
        }
        match key {
            "up" | "down" => {
                let current = self
                    .mcp_cursor
                    .as_ref()
                    .and_then(|name| names.iter().position(|candidate| candidate == name));
                let next = match (current, key) {
                    (Some(index), "down") => (index + 1) % names.len(),
                    (Some(index), _) => (index + names.len() - 1) % names.len(),
                    (None, "down") => 0,
                    (None, _) => names.len() - 1,
                };
                self.mcp_cursor = Some(names[next].clone());
                cx.notify();
            }
            "enter" => {
                if let Some(name) = self.mcp_cursor.clone() {
                    self.mcp_edit(&name, window, cx);
                }
            }
            "space" => {
                if let Some(name) = self.mcp_cursor.clone() {
                    self.mcp_toggle_server(name, cx);
                }
            }
            "escape" if self.mcp_cursor.take().is_some() => cx.notify(),
            _ => {}
        }
    }

    /// One server row: identity and facts on the left, live status and
    /// actions on the right. Keyboard focus paints the same selected surface
    /// as the sidebar's cursor.
    fn mcp_server_row(
        &self,
        server: &McpServer,
        first: bool,
        last: bool,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let name = server.name.clone();
        let runtime = self.mcp.runtime(&name);
        let status = runtime.status;
        let selected = self.mcp_cursor == Some(server.name.clone());
        let confirming = self.mcp_remove_confirm.as_deref() == Some(server.name.as_str());
        let pending = self
            .mcp_auth
            .as_ref()
            .is_some_and(|job| job.server == server.name);

        let mut row = div()
            .id(ElementId::Name(format!("mcp-row-{}", server.name).into()))
            .debug_selector({
                let name = server.name.clone();
                move || format!("mcp-row-{name}")
            })
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_start()
            .gap(DynamicSpacing::Base16.px(&theme))
            .when(first, |row| row.rounded_t(Radius::XLarge.px(&theme)))
            .when(last, |row| row.rounded_b(Radius::XLarge.px(&theme)))
            .when(selected, |row| row.bg(theme.selection))
            .hover(|row| row.bg(theme.surface_hover))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                let name = name.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| {
                        app.mcp_cursor = Some(name.clone());
                        cx.notify();
                    });
                }
            });

        // ── left column: name, description, facts ──
        let mut left = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base04.px(&theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(TextSize::Default.px(&theme))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(server.name.clone()),
                    )
                    .child(mcp_chip(tr!(server.scope.label_key()), theme))
                    .child(mcp_chip(tr!(server.def.exposure.label_key()), theme)),
            );
        if let Some(description) = &server.description {
            left = left.child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(description.clone()),
            );
        }
        left = left.child(
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!(
                    "mcp.transport_label",
                    transport = mcp_transport_summary(&server.def.transport)
                ))
                .when(!server.def.enabled, |line| {
                    line.child(tr!("mcp.meta_disabled"))
                })
                .when(
                    server.def.enabled && runtime.status == McpServerStatus::Connected,
                    |line| line.child(tr!("mcp.tool_count", count = runtime.tools.len())),
                )
                .when(runtime.resources > 0, |line| {
                    line.child(tr!("mcp.resource_count", count = runtime.resources))
                }),
        );
        if let Some(error) = &runtime.error {
            left = left.child(
                div()
                    .max_w(px(560.))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.error)
                    .truncate()
                    .tip(error.clone())
                    .child(error.clone()),
            );
            if let Some(hint) = mcp_error_hint(error) {
                left = left.child(
                    div()
                        .max_w(px(560.))
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(hint),
                );
            }
        }
        // The selected row expands its tool list — the visibility Pi's probe
        // reports, not an Orbit-side enumeration.
        if selected && !runtime.tools.is_empty() {
            let mut tools = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(DynamicSpacing::Base04.px(&theme))
                .pt(DynamicSpacing::Base04.px(&theme));
            for tool in &runtime.tools {
                let exposure = runtime
                    .tool_exposure
                    .get(tool)
                    .copied()
                    .unwrap_or(server.def.exposure);
                let label = if exposure == server.def.exposure {
                    tool.clone()
                } else {
                    format!("{tool} · {}", tr!(exposure.label_key()))
                };
                tools = tools.child(mcp_chip(label, theme));
            }
            left = left.child(tools);
        }
        // An OAuth-capable server (HTTP without an Authorization header) gets
        // explicit sign-in / sign-out controls in its expanded detail; the
        // contextual Sign in button appears by the status when Pi asks for it.
        if selected && server.oauth_candidate() && !pending {
            let name_sign_in = name.clone();
            let name_sign_out = name.clone();
            left = left.child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .pt(DynamicSpacing::Base04.px(&theme))
                    .child(icon(
                        "icons/lock.svg",
                        IconSize::XSmall.px(&theme),
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!("mcp.oauth")),
                    )
                    .child(mcp_text_button(
                        format!("mcp-detail-sign-in-{name}"),
                        tr!("mcp.sign_in"),
                        theme,
                        false,
                        this.clone(),
                        move |app, _, cx| app.mcp_sign_in(name_sign_in.clone(), cx),
                    ))
                    .child(mcp_text_button(
                        format!("mcp-detail-sign-out-{name}"),
                        tr!("mcp.sign_out"),
                        theme,
                        false,
                        this.clone(),
                        move |app, _, cx| app.mcp_sign_out(name_sign_out.clone(), cx),
                    )),
            );
        }
        row = row.child(left);

        // ── right column: status pill + actions ──
        let mut right = div()
            .flex_none()
            .flex()
            .flex_col()
            .items_end()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(mcp_status_pill(&server.name, status, theme));

        if confirming {
            right = right.child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(mcp_text_button(
                        format!("mcp-remove-cancel-{name}"),
                        tr!("mcp.cancel"),
                        theme,
                        false,
                        this.clone(),
                        move |app, _, cx| app.mcp_cancel_remove(cx),
                    ))
                    .child(mcp_text_button(
                        format!("mcp-remove-confirm-{name}"),
                        tr!("mcp.remove"),
                        theme,
                        true,
                        this.clone(),
                        move |app, _, cx| app.mcp_confirm_remove(name.clone(), cx),
                    )),
            );
        } else {
            if pending {
                // Pi is waiting on the browser: show the wait and a way out.
                right = right.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .child(spinner(
                            ElementId::Name(format!("mcp-auth-{name}").into()),
                            IconSize::XSmall.px(&theme),
                            theme.accent_text,
                            theme,
                        ))
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .child(tr!("mcp.waiting_for_browser")),
                        )
                        .child(mcp_text_button(
                            format!("mcp-auth-cancel-{name}"),
                            tr!("mcp.cancel"),
                            theme,
                            false,
                            this.clone(),
                            move |app, _, cx| app.mcp_cancel_sign_in(cx),
                        )),
                );
            } else {
                // The one contextual auth action: a server Pi reported as
                // needing a sign-in gets its button right where the state is.
                if status == McpServerStatus::NeedsAuth {
                    let name_sign_in = name.clone();
                    right = right.child(mcp_text_button(
                        format!("mcp-sign-in-{name}"),
                        tr!("mcp.sign_in"),
                        theme,
                        true,
                        this.clone(),
                        move |app, _, cx| app.mcp_sign_in(name_sign_in.clone(), cx),
                    ));
                }
                let name_toggle = name.clone();
                let name_test = name.clone();
                let name_edit = name.clone();
                let name_remove = name.clone();
                let actions = div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(mcp_switch(
                        format!("mcp-enable-{name}"),
                        server.def.enabled,
                        theme,
                        this.clone(),
                        move |app, cx| app.mcp_toggle_server(name_toggle.clone(), cx),
                    ))
                    .child(mcp_icon_action(
                        format!("mcp-test-{name}"),
                        "icons/refresh.svg",
                        tr!("mcp.test_connection"),
                        theme,
                        this.clone(),
                        move |app, _, cx| app.mcp_test_server(name_test.clone(), cx),
                    ))
                    .child(mcp_icon_action(
                        format!("mcp-edit-{name}"),
                        "icons/pencil.svg",
                        tr!("mcp.edit"),
                        theme,
                        this.clone(),
                        move |app, window, cx| app.mcp_edit(&name_edit, window, cx),
                    ))
                    .child(mcp_icon_action(
                        format!("mcp-delete-{name}"),
                        "icons/trash.svg",
                        tr!("mcp.delete"),
                        theme,
                        this.clone(),
                        move |app, _, cx| app.mcp_request_remove(name_remove.clone(), cx),
                    ));
                right = right.child(actions);
            }
        }
        row = row.child(right);
        row.into_any_element()
    }

    fn mcp_empty_card(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        // Before the first probe lands, the empty state says it is checking
        // rather than claiming nothing is configured.
        let loading = self.mcp.is_loading();
        let mut card = div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base24.px(&theme))
            .flex()
            .flex_col()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(if loading {
                spinner(
                    "mcp-empty-spinner",
                    IconSize::XLarge.px(&theme),
                    theme.text_muted,
                    theme,
                )
            } else {
                icon(
                    "icons/tools/mcp.svg",
                    IconSize::XLarge.px(&theme),
                    theme.text_muted,
                )
                .into_any_element()
            })
            .child(
                div()
                    .text_size(TextSize::Default.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(if loading {
                        tr!("mcp.checking")
                    } else {
                        tr!("mcp.empty_title")
                    }),
            );
        if !loading {
            card = card
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(tr!("mcp.empty_hint")),
                )
                .child(
                    button_frame(div().id("mcp-empty-add"), &theme, ButtonSize::Medium)
                        .cursor_pointer()
                        .font_weight(FontWeight::MEDIUM)
                        .raised(theme.action, &theme)
                        .text_color(theme.action_fg)
                        .hover(|style| style.raised(theme.action_hover, &theme))
                        .active(|style| style.raised(theme.action_pressed, &theme))
                        .on_mouse_up(MouseButton::Left, {
                            let this = this.clone();
                            move |_, window, cx| this.update(cx, |app, cx| app.mcp_add(window, cx))
                        })
                        .child(tr!("mcp.add_server")),
                );
        }
        card.into_any_element()
    }

    fn mcp_notice_card(&self, theme: Theme, title: impl Into<String>, detail: &str) -> AnyElement {
        let title = title.into();
        div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.error.opacity(0.35))
            .bg(theme.error.opacity(0.06))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base04.px(&theme))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.error)
                    .child(title),
            )
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(detail.to_string()),
            )
            .into_any_element()
    }

    fn mcp_external_change_card(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let body = div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_secondary)
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base03.px(&theme))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("mcp.external_change_title")),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("mcp.external_change_body")),
                    ),
            )
            .child(mcp_text_button(
                "mcp-external-reload".to_string(),
                tr!("mcp.reload"),
                theme,
                false,
                this.clone(),
                move |app, _, cx| app.mcp_refresh(cx),
            ));
        body.into_any_element()
    }

    fn mcp_restart_card(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.accent.opacity(0.4))
            .bg(theme.accent.opacity(0.06))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base03.px(&theme))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("mcp.restart_title")),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("mcp.restart_body")),
                    ),
            )
            .child(mcp_text_button(
                "mcp-restart-apply".to_string(),
                tr!("mcp.apply_now"),
                theme,
                true,
                this.clone(),
                move |app, _, cx| app.mcp_apply_clicked(cx),
            ))
            .into_any_element()
    }

    // ── rendering: the add/edit modal ─────────────────────────────────────

    /// The MCP add/edit modal, or `None` when closed.
    pub(super) fn mcp_editor_layer(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.mcp_editor.as_ref()?;
        let editing = editor.original.is_some();
        let title = if editing {
            tr!("mcp.edit_server")
        } else {
            tr!("mcp.add_server")
        };

        let field =
            |label: String, hint: Option<String>, input: Entity<ComposerInput>| -> AnyElement {
                let mut column = div()
                    .flex()
                    .flex_col()
                    .gap(input::gap(&theme))
                    .child(
                        input_label(label, &theme)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_secondary),
                    )
                    .child(
                        input_field_frame(div(), &theme)
                            .w_full()
                            .bg(theme.background)
                            .items_start()
                            .child(input),
                    );
                if let Some(hint) = hint {
                    column = column.child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(hint),
                    );
                }
                column.into_any_element()
            };

        // Transport segmented control.
        let mut transport_chips = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme));
        for kind in [McpTransportKind::Stdio, McpTransportKind::StreamableHttp] {
            let selected = editor.kind == kind;
            transport_chips = transport_chips.child(
                button_frame(
                    div().id(ElementId::Name(
                        format!("mcp-editor-kind-{}", kind_label(kind)).into(),
                    )),
                    &theme,
                    ButtonSize::Default,
                )
                .border_1()
                .border_color(if selected {
                    theme.accent.opacity(0.5)
                } else {
                    theme.border
                })
                .raised(
                    if selected {
                        theme.accent.opacity(0.12)
                    } else {
                        theme.surface_elevated
                    },
                    &theme,
                )
                .text_color(if selected {
                    theme.accent_text
                } else {
                    theme.text_secondary
                })
                .cursor_pointer()
                .hover(|style| style.raised(theme.surface_hover, &theme))
                .on_mouse_up(MouseButton::Left, {
                    let this = this.clone();
                    move |_, _, cx| {
                        this.update(cx, |app, cx| {
                            if let Some(editor) = app.mcp_editor.as_mut() {
                                editor.kind = kind;
                                cx.notify();
                            }
                        });
                    }
                })
                .child(tr!(kind.label_key())),
            );
        }

        // Name (locked once defined — it is the file key).
        let identity: AnyElement = if editing {
            div()
                .flex()
                .flex_col()
                .gap(input::gap(&theme))
                .child(
                    input_label(tr!("mcp.name"), &theme)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_secondary),
                )
                .child(
                    div()
                        .font_family(theme::code_font_family())
                        .text_size(TextSize::Default.px(&theme))
                        .text_color(theme.text_primary)
                        .child(
                            editor
                                .original
                                .as_ref()
                                .map(|original| original.name.clone())
                                .unwrap_or_default(),
                        ),
                )
                .into_any_element()
        } else {
            field(
                tr!("mcp.name"),
                Some(tr!("mcp.name_hint")),
                editor.name.clone(),
            )
        };

        let transport_fields: AnyElement = match editor.kind {
            McpTransportKind::Stdio => div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(field(
                    tr!("mcp.command"),
                    Some(tr!("mcp.command_hint")),
                    editor.command.clone(),
                ))
                .child(field(
                    tr!("mcp.arguments"),
                    Some(tr!("mcp.args_hint")),
                    editor.args.clone(),
                ))
                .child(field(
                    tr!("mcp.environment"),
                    Some(tr!("mcp.env_hint")),
                    editor.env.clone(),
                ))
                .into_any_element(),
            McpTransportKind::StreamableHttp => {
                let mut oauth_fields = div()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base12.px(&theme));
                if editor.oauth_open {
                    oauth_fields = oauth_fields
                        .child(field(
                            tr!("mcp.oauth_client_id"),
                            None,
                            editor.oauth_client_id.clone(),
                        ))
                        .child(field(
                            tr!("mcp.oauth_client_secret"),
                            Some(tr!("mcp.oauth_client_secret_hint")),
                            editor.oauth_client_secret.clone(),
                        ))
                        .child(field(
                            tr!("mcp.oauth_callback_port"),
                            Some(tr!("mcp.oauth_callback_port_hint")),
                            editor.oauth_callback_port.clone(),
                        ))
                        .child(field(
                            tr!("mcp.oauth_callback_url"),
                            Some(tr!("mcp.oauth_callback_url_hint")),
                            editor.oauth_callback_url.clone(),
                        ))
                        .child(field(
                            tr!("mcp.oauth_scope"),
                            None,
                            editor.oauth_scope.clone(),
                        ));
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base12.px(&theme))
                    .child(field(
                        tr!("mcp.url"),
                        Some(tr!("mcp.url_hint")),
                        editor.url.clone(),
                    ))
                    .child(field(
                        tr!("mcp.headers"),
                        Some(tr!("mcp.headers_hint")),
                        editor.headers.clone(),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(DynamicSpacing::Base06.px(&theme))
                            .child(
                                div()
                                    .id("mcp-editor-oauth-toggle")
                                    .flex()
                                    .items_center()
                                    .gap(DynamicSpacing::Base06.px(&theme))
                                    .cursor_pointer()
                                    .on_mouse_up(MouseButton::Left, {
                                        let this = this.clone();
                                        move |_, _, cx| {
                                            this.update(cx, |app, cx| {
                                                if let Some(editor) = app.mcp_editor.as_mut() {
                                                    editor.oauth_open = !editor.oauth_open;
                                                    cx.notify();
                                                }
                                            });
                                        }
                                    })
                                    .child(icon(
                                        if editor.oauth_open {
                                            "icons/chevron-down.svg"
                                        } else {
                                            "icons/chevron-right.svg"
                                        },
                                        IconSize::XSmall.px(&theme),
                                        theme.text_muted,
                                    ))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_secondary)
                                            .child(tr!("mcp.oauth_settings")),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_muted)
                                    .child(tr!("mcp.oauth_settings_hint")),
                            )
                            .children(editor.oauth_open.then_some(oauth_fields)),
                    )
                    .into_any_element()
            }
        };

        // Exposure radio rows.
        let mut exposure_list = div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base06.px(&theme));
        for mode in McpExposure::ALL {
            let selected = editor.exposure == mode;
            exposure_list = exposure_list.child(
                div()
                    .id(ElementId::Name(
                        format!("mcp-editor-exposure-{}", mode.as_str()).into(),
                    ))
                    .w_full()
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base06.px(&theme))
                    .rounded(Radius::Medium.px(&theme))
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .focusable()
                    .focus(|style| style.border_color(theme.focus_ring))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .cursor_pointer()
                    .when(selected, |row| row.bg(theme.selection))
                    .hover(|row| row.bg(theme.surface_hover))
                    .on_key_down({
                        let this = this.clone();
                        move |event, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.update(cx, |app, cx| {
                                    if let Some(editor) = app.mcp_editor.as_mut() {
                                        editor.exposure = mode;
                                        cx.notify();
                                    }
                                });
                            }
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let this = this.clone();
                        move |_, _, cx| {
                            this.update(cx, |app, cx| {
                                if let Some(editor) = app.mcp_editor.as_mut() {
                                    editor.exposure = mode;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base08.px(&theme))
                            .child(icon(
                                if selected {
                                    "icons/circle-check.svg"
                                } else {
                                    "icons/circle-dot.svg"
                                },
                                IconSize::XSmall.px(&theme),
                                if selected {
                                    theme.accent_text
                                } else {
                                    theme.text_muted
                                },
                            ))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if selected {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(tr!(mode.label_key())),
                            ),
                    )
                    .child(
                        div()
                            .pl(DynamicSpacing::Base24.px(&theme))
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!(mode.description_key())),
                    ),
            );
        }

        // Scope radio rows.
        let mut scope_list = div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base06.px(&theme));
        for scope in McpScope::ALL {
            let selected = editor.scope == scope;
            scope_list = scope_list.child(
                div()
                    .id(ElementId::Name(
                        format!("mcp-editor-scope-{}", scope.as_str()).into(),
                    ))
                    .w_full()
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base06.px(&theme))
                    .rounded(Radius::Medium.px(&theme))
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .focusable()
                    .focus(|style| style.border_color(theme.focus_ring))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .cursor_pointer()
                    .when(selected, |row| row.bg(theme.selection))
                    .hover(|row| row.bg(theme.surface_hover))
                    .on_key_down({
                        let this = this.clone();
                        move |event, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.update(cx, |app, cx| {
                                    if let Some(editor) = app.mcp_editor.as_mut() {
                                        editor.scope = scope;
                                        cx.notify();
                                    }
                                });
                            }
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let this = this.clone();
                        move |_, _, cx| {
                            this.update(cx, |app, cx| {
                                if let Some(editor) = app.mcp_editor.as_mut() {
                                    editor.scope = scope;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base08.px(&theme))
                            .child(icon(
                                if selected {
                                    "icons/circle-check.svg"
                                } else {
                                    "icons/circle-dot.svg"
                                },
                                IconSize::XSmall.px(&theme),
                                if selected {
                                    theme.accent_text
                                } else {
                                    theme.text_muted
                                },
                            ))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if selected {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(tr!(scope.label_key())),
                            ),
                    )
                    .child(
                        div()
                            .pl(DynamicSpacing::Base24.px(&theme))
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!(match scope {
                                McpScope::Global => "mcp.scope_global_hint",
                                McpScope::Project => "mcp.scope_project_hint",
                            })),
                    ),
            );
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base16.px(&theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        input_label(tr!("mcp.transport"), &theme)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_secondary),
                    )
                    .child(transport_chips)
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!(editor.kind.hint_key())),
                    ),
            )
            .child(identity)
            .child(field(
                tr!("mcp.description"),
                Some(tr!("mcp.description_hint")),
                editor.description.clone(),
            ))
            .child(transport_fields)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(
                        input_label(tr!("mcp.exposure"), &theme)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_secondary),
                    )
                    .child(exposure_list),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(
                        input_label(tr!("mcp.scope"), &theme)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_secondary),
                    )
                    .child(scope_list),
            );
        if let Some(error) = &editor.error {
            body = body.child(
                div()
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .rounded(Radius::Large.px(&theme))
                    .bg(theme.error.opacity(0.1))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.error)
                    .child(error.clone()),
            );
        }

        let cancel = button_frame(div().id("mcp-editor-cancel"), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| this.update(cx, |app, cx| app.mcp_editor_close(cx))
            })
            .child(
                div()
                    .text_color(theme.text_secondary)
                    .child(tr!("mcp.cancel")),
            );
        let save = button_frame(div().id("mcp-editor-save"), &theme, ButtonSize::Large)
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .raised(theme.action, &theme)
            .text_color(theme.action_fg)
            .hover(|style| style.raised(theme.action_hover, &theme))
            .active(|style| style.raised(theme.action_pressed, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| this.update(cx, |app, cx| app.mcp_editor_save(cx))
            })
            .child(tr!("mcp.save"));

        let card = div()
            .w_full()
            .max_w(px(560.))
            .max_h(relative(1.))
            .elevation_3(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .font_family(theme::ui_font_family())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(Self::mcp_editor_cancel))
            .on_action(cx.listener(Self::mcp_editor_confirm))
            .child(
                div()
                    .px(modal::header_padding_x(&theme))
                    .pt(modal::header_padding_top(&theme))
                    .pb(modal::header_padding_bottom(&theme))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(modal::header_gap(&theme))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(modal::HEADLINE.px(&theme))
                            .line_height(modal::HEADLINE.line_height(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!("mcp.editor_subtitle")),
                    ),
            )
            .child(
                div()
                    .id("mcp-editor-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(modal::section_padding_x(&theme, false))
                    .py(DynamicSpacing::Base16.px(&theme))
                    .child(body),
            )
            .child(
                div()
                    .flex_none()
                    .px(modal::footer_padding(&theme))
                    .py(modal::footer_padding(&theme))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(modal::footer_gap(&theme))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(cancel)
                    .child(save),
            );

        let scrim = match theme.mode {
            ThemeMode::Dark => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.42,
            },
            ThemeMode::Light => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.22,
            },
        };
        Some(
            div()
                .id("mcp-editor-layer")
                .absolute()
                .inset_0()
                .occlude()
                .bg(scrim)
                .p(DynamicSpacing::Base24.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, {
                    let this = this;
                    move |_, _, cx| this.update(cx, |app, cx| app.mcp_editor_close(cx))
                })
                .child(card)
                .into_any_element(),
        )
    }
}

// ── top-bar usage popover ─────────────────────────────────────────────────

/// Which pane the top-bar usage popover shows. `Providers` is the original
/// quota view; `Mcp` is the server-status view added beside it, so status is
/// one click from the always-visible usage pill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum QuotaPopupTab {
    #[default]
    Providers,
    Mcp,
}

impl QuotaPopupTab {
    pub(super) const ALL: [Self; 2] = [Self::Providers, Self::Mcp];

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Providers => "providers",
            Self::Mcp => "mcp",
        }
    }

    pub(super) fn label_key(self) -> &'static str {
        match self {
            Self::Providers => "session.tab_providers",
            Self::Mcp => "session.tab_mcp",
        }
    }
}

impl OrbitApp {
    /// Switch the usage popover's pane. Selecting MCP points the manager at
    /// the active workspace and refreshes a stale probe, so the tab never
    /// shows old state without saying so.
    pub(super) fn mcp_popup_tab(&mut self, tab: QuotaPopupTab, cx: &mut Context<Self>) {
        if self.quota_popup_tab == tab {
            return;
        }
        self.quota_popup_tab = tab;
        if tab == QuotaPopupTab::Mcp {
            self.mcp.set_workspace(self.current_workspace.as_deref());
            let stale = self
                .mcp
                .last_probed()
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(20));
            if stale && !self.mcp.is_probing() {
                self.schedule_mcp_probe(Duration::ZERO, cx);
            }
        }
        cx.notify();
    }

    /// Leave the popover and open Settings → MCP, optionally selecting one
    /// server so the detail (tools, OAuth actions) is already expanded.
    pub(super) fn mcp_popup_open_settings(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.quota_popup_open = false;
        self.settings_open = true;
        self.mcp_cursor = name;
        self.set_settings_section(SettingsSection::Mcp, cx);
        self.sync_terminal_active(cx);
    }

    /// The right side of the popover header on the MCP tab: enabled-server
    /// count and the probe control, in the same spin-in-place refresh shape
    /// the providers view uses.
    pub(super) fn mcp_popup_summary(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        // Configured count, not just enabled: the pane lists every server, so
        // "1/3 connected" stays truthful when some are disabled. With no
        // servers the count would read "0/0", so only the probe control shows.
        let total = self.mcp.servers().len();
        let connected = self
            .mcp
            .servers()
            .iter()
            .filter(|server| self.mcp.runtime(&server.name).status == McpServerStatus::Connected)
            .count();
        let refresh = icon_button_frame(div().id("quota-mcp-refresh"), &theme, ButtonSize::Medium)
            .group(BUTTON_GROUP)
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_hover))
            .active(|style| style.bg(theme.surface_active))
            .tip(tr!("mcp.refresh"))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| this.update(cx, |app, cx| app.mcp_refresh(cx))
            })
            .child(refresh_glyph(
                "quota-mcp-refresh-spin",
                ButtonSize::Medium.icon_size().px(&theme),
                self.mcp.is_probing(),
                theme.text_secondary,
                theme,
            ));
        div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .children((total > 0).then(|| {
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(tr!(
                        "mcp.popup_connected",
                        connected = connected,
                        total = total
                    ))
            }))
            .child(refresh)
            .into_any_element()
    }

    /// The MCP pane of the usage popover: one row per configured server, the
    /// probe's real state, and a way into the full page. Sized to the same
    /// body cap the providers pane uses, so the popover never grows past it.
    pub(super) fn mcp_popup_panel(
        &self,
        theme: Theme,
        max_height: Pixels,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut body = div()
            .id("quota-mcp-body")
            .debug_selector(|| "quota-mcp-body".to_string())
            .w_full()
            .max_h(max_height)
            .overflow_y_scroll()
            .p(DynamicSpacing::Base08.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base02.px(&theme));
        if self.mcp.servers().is_empty() {
            return body
                .child(self.mcp_popup_empty(theme, this))
                .into_any_element();
        }
        for server in self.mcp.servers() {
            body = body.child(self.mcp_popup_row(server, theme, this.clone(), cx));
        }
        body.child(self.mcp_popup_footer(theme, this))
            .into_any_element()
    }

    fn mcp_popup_empty(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        div()
            .w_full()
            .py(DynamicSpacing::Base16.px(&theme))
            .flex()
            .flex_col()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(icon(
                "icons/tools/mcp.svg",
                IconSize::Medium.px(&theme),
                theme.text_muted,
            ))
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("mcp.empty_title")),
            )
            .child(
                div()
                    .max_w(px(220.))
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_muted)
                    .text_center()
                    .child(tr!("mcp.empty_hint")),
            )
            .child(mcp_popup_action(
                "quota-mcp-empty-open".to_string(),
                tr!("mcp.popup_open_settings"),
                theme,
                true,
                this,
                move |app, cx| app.mcp_popup_open_settings(None, cx),
            ))
            .into_any_element()
    }

    /// One compact server row: status glyph, name + facts, and the one
    /// contextual action the state asks for (tool count, Sign in, or the
    /// probe's error). The whole row opens Settings → MCP at that server.
    fn mcp_popup_row(
        &self,
        server: &McpServer,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let name = server.name.clone();
        let name_sign_in = name.clone();
        let runtime = self.mcp.runtime(&name);
        let status = runtime.status;
        let pending = self
            .mcp_auth
            .as_ref()
            .is_some_and(|job| job.server == server.name);
        let tone = if pending {
            theme.info
        } else {
            mcp_status_color(status, theme)
        };

        let glyph: AnyElement = if pending || status == McpServerStatus::Connecting {
            spinner(
                ElementId::Name(format!("quota-mcp-spin-{name}").into()),
                IconSize::XSmall.px(&theme),
                tone,
                theme,
            )
        } else {
            icon(mcp_status_glyph(status), IconSize::XSmall.px(&theme), tone).into_any_element()
        };

        let mut facts = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(
                // This cell is measured at zero width before its final
                // width settles, and GPUI's `truncate()` latches the shape
                // of that first measure — every name came out as "…". Clip a
                // content-sized line instead: the outer cell hides overflow,
                // the inner one keeps the name's real width so tests can
                // see it.
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .flex()
                    .child(
                        div()
                            .flex_none()
                            .debug_selector({
                                let name = name.clone();
                                move || format!("quota-mcp-name-{name}")
                            })
                            .text_size(TextSize::Small.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if server.def.enabled {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .child(server.name.clone()),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_muted)
                    .child(mcp_popup_meta(server)),
            );
        if let Some(error) = &runtime.error {
            facts = facts.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.error)
                    .tip(error.clone())
                    .child(error.clone()),
            );
        }

        let right: AnyElement = if pending {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base04.px(&theme))
                .child(
                    div()
                        .text_size(TextSize::XSmall.px(&theme))
                        .text_color(theme.text_muted)
                        .child(tr!("mcp.waiting_for_browser")),
                )
                .child(mcp_popup_action(
                    format!("quota-mcp-auth-cancel-{name}"),
                    tr!("mcp.cancel"),
                    theme,
                    false,
                    this.clone(),
                    move |app, cx| app.mcp_cancel_sign_in(cx),
                ))
                .into_any_element()
        } else {
            match status {
                McpServerStatus::Connected => div()
                    .flex_none()
                    .font(crate::usage::view::num_font())
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(tr!("mcp.popup_tools", count = runtime.tools.len()))
                    .into_any_element(),
                McpServerStatus::NeedsAuth => mcp_popup_action(
                    format!("quota-mcp-sign-in-{name}"),
                    tr!("mcp.sign_in"),
                    theme,
                    true,
                    this.clone(),
                    move |app, cx| app.mcp_sign_in(name_sign_in.clone(), cx),
                ),
                McpServerStatus::Failed => {
                    mcp_popup_status(tr!("mcp.status_failed"), theme.error, theme)
                }
                McpServerStatus::Disabled => {
                    mcp_popup_status(tr!("mcp.disabled"), theme.text_muted, theme)
                }
                McpServerStatus::Connecting => {
                    mcp_popup_status(tr!("mcp.status_connecting"), theme.text_muted, theme)
                }
                McpServerStatus::Disconnected | McpServerStatus::Unknown => {
                    mcp_popup_status(tr!(status.label_key()), theme.text_muted, theme)
                }
            }
        };

        div()
            .id(ElementId::Name(format!("quota-mcp-row-{name}").into()))
            .debug_selector({
                let name = name.clone();
                move || format!("quota-mcp-row-{name}")
            })
            .w_full()
            .px(DynamicSpacing::Base08.px(&theme))
            .py(DynamicSpacing::Base06.px(&theme))
            .rounded(list_item::RADIUS.px(&theme))
            .flex()
            .items_start()
            .gap(DynamicSpacing::Base08.px(&theme))
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_hover))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| {
                        app.mcp_popup_open_settings(Some(name.clone()), cx)
                    });
                }
            })
            .child(div().flex_none().pt(px(1.)).child(glyph))
            .child(facts)
            .child(right)
            .into_any_element()
    }

    fn mcp_popup_footer(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let checked = if self.mcp.is_probing() {
            tr!("mcp.checking")
        } else if let Some(at) = self.mcp.last_probed() {
            tr!("mcp.checked_ago", seconds = at.elapsed().as_secs().max(1))
        } else {
            tr!("mcp.not_checked")
        };
        div()
            .w_full()
            .mt(DynamicSpacing::Base04.px(&theme))
            .pt(DynamicSpacing::Base06.px(&theme))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(mcp_popup_action(
                "quota-mcp-open-settings".to_string(),
                tr!("mcp.popup_open_settings"),
                theme,
                false,
                this,
                move |app, cx| app.mcp_popup_open_settings(None, cx),
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_muted)
                    .child(checked),
            )
            .into_any_element()
    }
}

/// The row's muted fact line: exposure, scope, and OAuth capability.
fn mcp_popup_meta(server: &McpServer) -> String {
    let mut parts = vec![
        tr!(server.def.exposure.label_key()),
        tr!(server.scope.label_key()),
    ];
    if !server.def.enabled {
        parts.push(tr!("mcp.disabled"));
    }
    if server.oauth_candidate() {
        parts.push(tr!("mcp.oauth"));
    }
    parts.join(" · ")
}

fn mcp_popup_status(label: String, color: Hsla, theme: Theme) -> AnyElement {
    div()
        .flex_none()
        .text_size(TextSize::XSmall.px(&theme))
        .text_color(color)
        .child(label)
        .into_any_element()
}

/// A compact action inside a popover row. Stops propagation so tapping it
/// never also triggers the row's open-settings click.
fn mcp_popup_action(
    id: String,
    label: String,
    theme: Theme,
    primary: bool,
    this: Entity<OrbitApp>,
    action: impl Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
) -> AnyElement {
    let action = Rc::new(action);
    let mut button = button_frame(
        div().id(ElementId::Name(id.into())),
        &theme,
        ButtonSize::Compact,
    )
    .cursor_pointer()
    .font_weight(FontWeight::MEDIUM);
    button = if primary {
        button
            .raised(theme.action, &theme)
            .text_color(theme.action_fg)
            .hover(|style| style.raised(theme.action_hover, &theme))
            .active(|style| style.raised(theme.action_pressed, &theme))
    } else {
        button
            .raised(theme.overlay, &theme)
            .text_color(theme.text_secondary)
            .hover(|style| {
                style
                    .raised(theme.overlay_strong, &theme)
                    .text_color(theme.text_primary)
            })
    };
    press(button)
        .child(label)
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            this.update(cx, |app, cx| action(app, cx));
        })
        .into_any_element()
}

// ── small UI helpers ───────────────────────────────────────────────────────

fn kind_label(kind: McpTransportKind) -> &'static str {
    match kind {
        McpTransportKind::Stdio => "stdio",
        McpTransportKind::StreamableHttp => "http",
    }
}

/// The status tone. Color is never the only signal — every call site pairs it
/// with a glyph and a label.
fn mcp_status_color(status: McpServerStatus, theme: Theme) -> Hsla {
    match status {
        McpServerStatus::Connected => theme.success,
        McpServerStatus::Failed => theme.error,
        McpServerStatus::NeedsAuth => theme.warning,
        McpServerStatus::Connecting => theme.info,
        McpServerStatus::Disabled | McpServerStatus::Disconnected | McpServerStatus::Unknown => {
            theme.text_muted
        }
    }
}

fn mcp_status_glyph(status: McpServerStatus) -> &'static str {
    match status {
        McpServerStatus::Connected => "icons/circle-check.svg",
        McpServerStatus::Failed => "icons/circle-x.svg",
        McpServerStatus::NeedsAuth => "icons/lock.svg",
        McpServerStatus::Connecting => "icons/loader.svg",
        McpServerStatus::Disabled => "icons/circle-dot.svg",
        McpServerStatus::Disconnected | McpServerStatus::Unknown => "icons/circle-dot.svg",
    }
}

fn mcp_status_pill(id: &str, status: McpServerStatus, theme: Theme) -> AnyElement {
    let color = mcp_status_color(status, theme);
    let glyph: AnyElement = if status == McpServerStatus::Connecting {
        spinner(
            ElementId::Name(format!("mcp-connecting-{id}").into()),
            IconSize::XSmall.px(&theme),
            color,
            theme,
        )
    } else {
        icon(mcp_status_glyph(status), IconSize::XSmall.px(&theme), color).into_any_element()
    };
    div()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base04.px(&theme))
        .text_size(TextSize::Small.px(&theme))
        .text_color(color)
        .child(glyph)
        .child(tr!(status.label_key()))
        .into_any_element()
}

/// A short, actionable hint appended to Pi's own connection error. Pi's text
/// names the failure; this names the usual fix.
fn mcp_error_hint(error: &str) -> Option<String> {
    let lower = error.to_lowercase();
    if lower.contains("enoent") || lower.contains("not found") {
        Some(tr!("mcp.hint_command_missing"))
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("authentication")
    {
        Some(tr!("mcp.hint_auth"))
    } else if lower.contains("timed out") || lower.contains("timeout") {
        Some(tr!("mcp.hint_timeout"))
    } else if lower.contains("fetch failed")
        || lower.contains("econnrefused")
        || lower.contains("network")
        || lower.contains("dns")
    {
        Some(tr!("mcp.hint_network"))
    } else {
        None
    }
}

fn mcp_chip(label: String, theme: Theme) -> AnyElement {
    div()
        .flex_none()
        .h(DynamicSpacing::Base20.px(&theme))
        .px(DynamicSpacing::Base06.px(&theme))
        .rounded(Radius::Medium.px(&theme))
        .bg(theme.surface_elevated)
        .border_1()
        .border_color(theme.border)
        .flex()
        .items_center()
        .text_size(TextSize::XSmall.px(&theme))
        .text_color(theme.text_muted)
        .child(label)
        .into_any_element()
}

/// The per-row enable switch. Mirrors `settings_toggle` but routes through an
/// app entity so a server name can ride along.
fn mcp_switch(
    id: String,
    on: bool,
    theme: Theme,
    this: Entity<OrbitApp>,
    action: impl Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
) -> AnyElement {
    let action = std::rc::Rc::new(action);
    div()
        .id(ElementId::Name(id.into()))
        .w(px(36.))
        .h(DynamicSpacing::Base20.px(&theme))
        .rounded_full()
        .p(DynamicSpacing::Base02.px(&theme))
        .border_1()
        .border_color(theme.border)
        .flex()
        .items_center()
        .cursor_pointer()
        .when(on, |t| t.bg(theme.accent).justify_end())
        .when(!on, |t| t.bg(theme.surface_elevated).justify_start())
        .hover(|t| t.border_color(theme.border_strong))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| action(app, cx));
        })
        .child(div().size(px(14.)).rounded_full().bg(theme.toggle_knob))
        .into_any_element()
}

/// An icon-only row action with a tooltip and press feedback.
fn mcp_icon_action(
    id: String,
    glyph_path: &'static str,
    tip: String,
    theme: Theme,
    this: Entity<OrbitApp>,
    action: impl Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static,
) -> AnyElement {
    let action = std::rc::Rc::new(action);
    press(
        div()
            .id(ElementId::Name(id.into()))
            .w(px(26.))
            .h(px(26.))
            .rounded(Radius::Medium.px(&theme))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_hover)),
    )
    .tip(tip)
    .child(icon(
        glyph_path,
        IconSize::Small.px(&theme),
        theme.text_muted,
    ))
    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
        this.update(cx, |app, cx| action(app, window, cx));
    })
    .into_any_element()
}

/// A compact labelled button used by the notice cards and the remove
/// confirmation.
fn mcp_text_button(
    id: String,
    label: String,
    theme: Theme,
    primary: bool,
    this: Entity<OrbitApp>,
    action: impl Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static,
) -> AnyElement {
    let action = std::rc::Rc::new(action);
    let mut button = button_frame(
        div().id(ElementId::Name(id.into())),
        &theme,
        ButtonSize::Medium,
    )
    .cursor_pointer()
    .font_weight(FontWeight::MEDIUM);
    button = if primary {
        button
            .raised(theme.action, &theme)
            .text_color(theme.action_fg)
            .hover(|style| style.raised(theme.action_hover, &theme))
            .active(|style| style.raised(theme.action_pressed, &theme))
    } else {
        button
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .text_color(theme.text_secondary)
            .hover(|style| style.raised(theme.surface_hover, &theme))
    };
    press(button)
        .child(label)
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            this.update(cx, |app, cx| action(app, window, cx));
        })
        .into_any_element()
}

/// Free function view of a server's transport, for the row's meta line.
fn mcp_transport_summary(transport: &McpTransport) -> String {
    match transport {
        McpTransport::Stdio { command, args, .. } => {
            let mut parts = vec![command.clone()];
            parts.extend(args.iter().take(3).cloned());
            let mut text = parts.join(" ");
            if args.len() > 3 {
                text.push_str(" …");
            }
            text
        }
        McpTransport::StreamableHttp { url, .. } => url.clone(),
    }
}

/// The config directory label under the toolbar.
fn mcp_config_dir_label(path: &Path) -> String {
    path.parent()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether a server matches the page filter (name, description, command, or
/// URL).
fn mcp_server_matches(server: &McpServer, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if server.name.to_lowercase().contains(needle) {
        return true;
    }
    if server
        .description
        .as_deref()
        .is_some_and(|description| description.to_lowercase().contains(needle))
    {
        return true;
    }
    match &server.def.transport {
        McpTransport::Stdio { command, args, .. } => {
            command.to_lowercase().contains(needle)
                || args.iter().any(|arg| arg.to_lowercase().contains(needle))
        }
        McpTransport::StreamableHttp { url, .. } => url.to_lowercase().contains(needle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(scope: McpScope, name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            scope,
            source: PathBuf::from("/tmp/mcp.json"),
            description: Some("Access GitHub".into()),
            def: McpServerDef {
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                    env: BTreeMap::new(),
                    cwd: None,
                },
                ..McpServerDef::default()
            },
        }
    }

    #[test]
    fn filter_matches_name_description_and_transport() {
        let server = server(McpScope::Global, "github");
        assert!(mcp_server_matches(&server, ""));
        assert!(mcp_server_matches(&server, "git"));
        assert!(mcp_server_matches(&server, "access github"));
        assert!(mcp_server_matches(&server, "npx"));
        assert!(mcp_server_matches(&server, "modelcontextprotocol"));
        assert!(!mcp_server_matches(&server, "sentry"));
    }

    #[test]
    fn transport_summary_shortens_long_arg_lists() {
        let server = server(McpScope::Global, "github");
        let summary = mcp_transport_summary(&server.def.transport);
        assert_eq!(summary, "npx -y @modelcontextprotocol/server-github");
        let long = McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
            env: BTreeMap::new(),
            cwd: None,
        };
        assert!(mcp_transport_summary(&long).ends_with('…'));
        let http = McpTransport::StreamableHttp {
            url: "https://example.com/mcp".into(),
            headers: BTreeMap::new(),
        };
        assert_eq!(mcp_transport_summary(&http), "https://example.com/mcp");
    }

    #[test]
    fn status_glyphs_never_rely_on_color_alone() {
        // Every status has a distinct glyph/label pair — the color is an
        // accent, not the signal.
        let theme = Theme::dark();
        for status in [
            McpServerStatus::Unknown,
            McpServerStatus::Disabled,
            McpServerStatus::Connecting,
            McpServerStatus::Connected,
            McpServerStatus::Disconnected,
            McpServerStatus::NeedsAuth,
            McpServerStatus::Failed,
        ] {
            assert!(!mcp_status_glyph(status).is_empty());
            assert!(!tr!(status.label_key()).is_empty());
            let _ = mcp_status_color(status, theme);
        }
        assert_eq!(
            mcp_status_color(McpServerStatus::Connected, theme),
            theme.success
        );
        assert_eq!(
            mcp_status_color(McpServerStatus::Failed, theme),
            theme.error
        );
    }

    #[test]
    fn connection_errors_get_an_actionable_hint() {
        // Pi's own error is kept and a fix is appended; unknown failures get
        // no invented hint.
        assert!(mcp_error_hint("spawn npx ENOENT").is_some());
        assert!(mcp_error_hint("HTTP 401 Unauthorized").is_some());
        assert!(mcp_error_hint("fetch failed").is_some());
        assert!(mcp_error_hint("request timed out").is_some());
        assert!(mcp_error_hint("something unusual happened").is_none());
    }
}
