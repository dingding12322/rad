use super::helpers::*;
use super::*;

impl OrbitApp {
    /// Record a status message; the status bar surfaces it briefly (see
    /// [`STATUS_MESSAGE_TTL`]). Internal RPC chatter never calls this —
    /// only user-meaningful facts and failures.
    pub(super) fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_at = Some(Instant::now());
    }

    /// Record a command / protocol / extension error. Unlike the transient
    /// status line, errors persist (in a red banner) until dismissed or
    /// superseded, so a failure is never silently lost.
    pub(super) fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        // The banner renders on the chat and settings surfaces. The Git and
        // Usage pages own the main area, so an error raised while one of them
        // is open also surfaces as a toast — the failure is never off-screen.
        if self.git_open || self.usage_open {
            self.toast_error(message.clone());
        }
        self.error = Some(message);
    }

    /// Dismiss the current error banner.
    pub(super) fn dismiss_error(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        cx.notify();
    }

    /// A successful command clears the banner it previously raised (e.g. a
    /// retried `set_model`), so a fixed error doesn't linger.
    pub(super) fn clear_error_for(&mut self, command: &str) {
        if let Some(error) = &self.error {
            if error.starts_with(&humanize_command(command)) {
                self.error = None;
            }
        }
    }

    /// Surface a failed command's `error` — the docs' `success: false`
    /// contract. A `parse` command is pi's response to unparseable input.
    pub(super) fn on_command_failure(
        &mut self,
        command: &str,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if command == "parse" {
            self.set_error(tr!(
                "runtime.protocol_error",
                error = error.unwrap_or(&tr!("runtime.parse_failed"))
            ));
            return;
        }
        // A rejected follow-up/steer never entered pi's queue: drop the
        // optimistic chip and hand the text back so nothing is lost.
        if matches!(command, "follow_up" | "steer") {
            if let Some(text) = self.pending_follow_up.take() {
                if let Some(pos) = self.queue.follow_up.iter().position(|t| *t == text) {
                    self.queue.follow_up.remove(pos);
                }
                if self.input.read(cx).text().trim().is_empty() {
                    self.input.update(cx, |input, cx| input.set_text(text, cx));
                }
            }
        }
        // A rejected prompt never opened a turn. The run state was set
        // optimistically when the prompt went out, so unwind it here instead of
        // leaving the composer on Stop and the transcript waiting for an answer
        // that is not coming (see `submit_as`).
        if command == "prompt" {
            self.busy = false;
            self.transcript.abandon_reply();
            // pi is the authority on whether something else is still running;
            // `get_state` re-syncs `busy` from it.
            self.send(CommandBody::GetState, "get_state");
        }
        let fallback = tr!("runtime.unspecified_error");
        let detail = error.unwrap_or(&fallback);
        self.set_error(tr!(
            "helpers.command_failed_detail",
            label = humanize_command(command),
            detail = detail
        ));
    }

    /// Send a command to pi. Returns `false` when the write failed (or
    /// there is no process) so callers can keep the user's input intact.
    pub(super) fn send(&mut self, body: CommandBody, label: &str) -> bool {
        let Some(client) = self.client.as_ref() else {
            self.toast_warning(tr!("runtime.pi_not_running"));
            return false;
        };
        match client.send(body) {
            Ok(_) => true,
            Err(err) => {
                self.set_error(tr!("runtime.send_failed", label = label, error = err));
                false
            }
        }
    }

    /// Re-fetch the model catalog and thinking levels for the current session.
    pub(super) fn refresh_catalogs(&mut self) {
        self.send(CommandBody::GetAvailableModels, "get_available_models");
        self.send(
            CommandBody::GetAvailableThinkingLevels,
            "get_available_thinking_levels",
        );
        self.send(CommandBody::GetCommands, "get_commands");
    }

    /// Re-fetch the current context-window estimate. Cheap; call after
    /// settle, session switch, compaction, and model changes — never per tick.
    pub(super) fn refresh_context_stats(&mut self) {
        self.send(CommandBody::GetSessionStats, "get_session_stats");
    }

    /// Make `client` the active pi process, resetting uptime / exit state.
    ///
    /// Callers send their primary command (`switch_session` / `new_session` /
    /// `get_state`) *before* [`probe_auth`](Self::probe_auth), so the slow
    /// provider-auth/quota probes never queue in front of the session load.
    pub(super) fn adopt_client(&mut self, client: PiClient) {
        self.runtime = RuntimeStatus {
            started_at: Some(Instant::now()),
            alive: true,
            exited: false,
            error: None,
        };
        self.client = Some(client);
        // A new process starts a new session stream: entry ids from the old
        // process mean nothing here.
        self.reset_quota_entries();
    }

    /// Tear down the active pi process (dropping `PiClient` kills the child).
    pub(super) fn drop_client(&mut self) {
        // A process that exited on its own left its exit banner up; dropping
        // it supersedes that failure — a fresh process follows in the task /
        // restart paths, and an explicit stop acknowledges it.
        if self.runtime.exited {
            self.error = None;
        }
        self.client = None;
        self.runtime = RuntimeStatus::default();
        // Extension widgets belonged to the departing process.
        self.extension_widgets.clear();
        // A login in flight dies with its process; remember the provider so
        // the next `auth.list` can reconcile a completion that happened while
        // Orbit was reconnecting.
        self.auth.on_disconnect();
        self.quota.on_disconnect();
        self.reset_quota_entries();
    }

    /// Forget the bridge cursor and schedule an immediate poll. Called when
    /// the active session (and therefore the entry-id space) changes. The
    /// workflow mode is per session, so it resolves here too: a pending New
    /// Task choice is committed to the new id, otherwise the stored mode is
    /// loaded.
    pub(super) fn reset_quota_entries(&mut self) {
        self.quota_entries_cursor = None;
        self.quota_entries_inflight = false;
        self.quota_entries_bootstrap = QUOTA_ENTRY_BOOTSTRAP_POLLS;
        self.quota_entries_next_poll = Instant::now();
        if let Some(id) = self.session_id.clone() {
            match self.workflow_pending.take() {
                Some(mode) => {
                    crate::workflow::persist_for(&id, mode);
                    self.workflow_mode = mode;
                }
                None => self.workflow_mode = crate::workflow::load_for(&id),
            }
            // A different session is a different pushed-default record.
            self.default_model_pushed = DefaultModelPushed::default();
        }
    }

    /// The catalog entry for the configured default model, if the catalog has
    /// it. A removed or renamed model must never poison a session (fail soft);
    /// a default without a provider matches by id alone, mirroring pi's own
    /// fuzzy match.
    pub(super) fn default_model_entry(&self) -> Option<&ModelEntry> {
        let id = self.session_default.model_id.as_deref()?;
        self.available_models.iter().find(|model| {
            model.id == id
                && self
                    .session_default
                    .provider
                    .as_deref()
                    .is_none_or(|provider| model.provider == provider)
        })
    }

    /// The resolved default as `(provider, id)`.
    fn resolve_default_model(&self) -> Option<(String, String)> {
        self.default_model_entry()
            .map(|model| (model.provider.clone(), model.id.clone()))
    }

    /// Push the default model to the live session. One-shot: armed by a
    /// `new_session` birth and disarmed once there is nothing left that can be
    /// applied. An unset, unknown, or already-active model is skipped silently.
    ///
    /// May be called again when the model catalog arrives after the session
    /// (it is requested right after `get_state`); the `default_model_pushed`
    /// record keeps that from resending `set_model`.
    pub(super) fn apply_default_model(&mut self, cx: &mut Context<Self>) {
        if !self.default_model_armed {
            return;
        }
        let Some(session) = self.session_id.clone() else {
            return;
        };
        if self.client.is_none() {
            return;
        }
        if self.session_default.is_unset() {
            // Nothing to push; nothing to retry later.
            self.default_model_armed = false;
            return;
        }
        if self.default_model_pushed.session.as_deref() != Some(session.as_str()) {
            self.default_model_pushed = DefaultModelPushed {
                session: Some(session),
                ..DefaultModelPushed::default()
            };
        }
        let model = self.resolve_default_model();
        if model.is_none() && self.available_models.is_empty() {
            // The catalog that would let us honour the default hasn't loaded
            // yet; stay armed for the `get_available_models` response.
            return;
        }
        let model_live = model
            .as_ref()
            .is_some_and(|(provider, id)| self.model_id == *id && self.model_provider == *provider);
        if let Some((provider, id)) = &model {
            if !model_live {
                let sent = self.default_model_pushed.model.as_ref()
                    == Some(&(provider.clone(), id.clone()));
                if !sent {
                    // Sent directly, not through `set_model`: the public setter
                    // disarms the session-birth default, which the push itself
                    // must not do.
                    self.send(
                        CommandBody::SetModel {
                            model_id: id.clone(),
                            provider: provider.clone(),
                        },
                        "set_model",
                    );
                    self.default_model_pushed.model = Some((provider.clone(), id.clone()));
                    cx.notify();
                }
                // pi applies the switch asynchronously and resets the level to
                // the model's own default when it lands, so a thinking level
                // sent now would be lost. Stay armed: the `get_state` that
                // follows the `set_model` response calls us again with the new
                // model live.
                return;
            }
        } else {
            // A missing model disarms: a removed/renamed id must not make
            // every later catalog refresh retry (fail soft).
            self.default_model_armed = false;
            return;
        }
        // Only once the default model is live: the supported levels come from
        // its catalog entry, so a stale or unsupported level is skipped the
        // same way a missing model is.
        let levels = self
            .default_model_entry()
            .map(|model| model.thinking_levels.clone())
            .unwrap_or_default();
        let thinking = self
            .session_default
            .thinking
            .clone()
            .filter(|level| levels.iter().any(|candidate| candidate == level));
        if let Some(level) = thinking {
            if self.thinking_label != level
                && self.default_model_pushed.thinking.as_deref() != Some(level.as_str())
            {
                self.send(
                    CommandBody::SetThinkingLevel {
                        level: level.clone(),
                    },
                    "set_thinking_level",
                );
                self.default_model_pushed.thinking = Some(level);
                cx.notify();
            }
        }
        self.default_model_armed = false;
    }

    /// Write the default model and persist it. It takes effect on the next
    /// session; the composer chip can still change the live one.
    pub(super) fn set_default_model(&mut self, slot: crate::session_defaults::SessionDefault) {
        self.session_default = slot;
        self.session_default.persist();
    }

    /// Poll the active process for new quota-bridge session entries, at most
    /// once per [`QUOTA_ENTRY_POLL_INTERVAL`]. The bridge appends a snapshot
    /// only when the numbers change, so a quiet account returns no entries.
    pub(super) fn poll_quota_entries(&mut self) {
        if self.extensions.quota().is_none() && self.extensions.workflow().is_none() {
            return;
        }
        if self.quota_entries_inflight || self.quota_entries_next_poll > Instant::now() {
            return;
        }
        if !self.client.as_mut().is_some_and(|client| client.is_alive()) {
            return;
        }
        self.quota_entries_inflight = true;
        let interval = if self.quota_entries_bootstrap > 0 {
            self.quota_entries_bootstrap -= 1;
            QUOTA_ENTRY_BOOTSTRAP_INTERVAL
        } else {
            QUOTA_ENTRY_POLL_INTERVAL
        };
        self.quota_entries_next_poll = Instant::now() + interval;
        let sent = self.send(
            CommandBody::GetEntries {
                since: self.quota_entries_cursor.clone(),
            },
            "get_entries",
        );
        if !sent {
            // No response is coming; unblock the next attempt.
            self.quota_entries_inflight = false;
        }
    }

    /// Ask pi for provider auth capabilities. Harmless on a pi that doesn't
    /// implement `auth.*`: the failed `auth.list` response marks auth
    /// unsupported and the Providers page keeps the file/Terminal fallback.
    pub(super) fn probe_auth(&mut self) {
        let wanted = self.auth.on_reconnect();
        self.send(CommandBody::AuthList, "auth.list");
        for provider in wanted {
            self.send(CommandBody::AuthStatus { provider }, "auth.status");
        }
        self.refresh_quota();
    }

    /// Ask pi for account quota/balance/spend for every connected provider.
    /// Harmless on a pi build without the quota RPC: the failed response marks
    /// it unsupported and the cards simply omit quota meters.
    pub(super) fn refresh_quota(&mut self) {
        if self.quota.support() == QuotaSupport::Unsupported {
            return;
        }
        self.send(CommandBody::QuotaList { provider: None }, "quota.list");
    }

    /// Refresh capabilities and per-provider status against the *current*
    /// process without clearing what the card already knows. Used when the
    /// Providers page opens and on Refresh.
    pub(super) fn refresh_auth(&mut self) {
        if self.auth.support() == AuthSupport::Unsupported {
            return;
        }
        self.send(CommandBody::AuthList, "auth.list");
        self.refresh_quota();
        if self.auth.is_busy() {
            // A login is mid-flight; don't disturb it with status refreshes.
            return;
        }
        let providers: Vec<String> = self
            .auth
            .providers()
            .iter()
            .map(|provider| provider.id.clone())
            .collect();
        for provider in providers {
            self.send(CommandBody::AuthStatus { provider }, "auth.status");
        }
    }

    /// Perform the side effects the [`AuthManager`] emitted. The manager does
    /// no I/O; opening the browser and sending cancels happen here.
    pub(super) fn handle_auth_effects(&mut self, effects: Vec<AuthEffect>, cx: &mut Context<Self>) {
        if effects.is_empty() {
            return;
        }
        for effect in effects {
            match effect {
                AuthEffect::OpenUrl(url) => {
                    if let Err(err) = platform::open_url(&url) {
                        self.toast_warning(tr!("runtime.browser_failed", error = err));
                    }
                }
                AuthEffect::CancelLogin(session_id) => {
                    self.send(CommandBody::AuthCancel { session_id }, "auth.cancel");
                }
                AuthEffect::RefreshProviders => {
                    // The login/logout already took effect in pi; no restart
                    // banner is needed on the RPC path.
                    self.provider_auth_dirty = false;
                    if let Some(session) = self.auth.login() {
                        if session.phase == LoginPhase::Succeeded {
                            self.toast_success(tr!(
                                "runtime.signed_in_to",
                                provider = session.provider
                            ));
                        }
                    }
                    self.send(CommandBody::AuthList, "auth.list");
                    self.refresh_quota();
                    self.refresh_catalogs();
                    self.reload_custom_providers(cx);
                }
            }
        }
        cx.notify();
    }

    /// Start a provider login through the RPC namespace. `method` comes from
    /// the provider's discovered capability, never a hardcoded flow.
    pub(super) fn auth_start_login(
        &mut self,
        provider: String,
        name: String,
        method: String,
        cx: &mut Context<Self>,
    ) {
        if self.auth.support() == AuthSupport::Unsupported {
            // pi has no auth RPC: hand the login to the Terminal the way
            // the page has always done.
            self.provider_oauth_login(provider, name, cx);
            return;
        }
        let session_id = self.auth.start_login(&provider, &method);
        self.send(
            CommandBody::AuthLogin {
                provider,
                method,
                session_id: Some(session_id),
            },
            "auth.login",
        );
        cx.notify();
    }

    /// Cancel the active login and tell pi to tear its flow down.
    pub(super) fn auth_cancel_login(&mut self, cx: &mut Context<Self>) {
        if let Some(session_id) = self.auth.cancel_login() {
            self.send(CommandBody::AuthCancel { session_id }, "auth.cancel");
        }
        cx.notify();
    }

    pub(super) fn runtime_state(&self) -> RuntimeState {
        if self.client.is_none() {
            if self.runtime.error.is_some() {
                RuntimeState::Failed
            } else {
                RuntimeState::Stopped
            }
        } else if self.runtime.exited || !self.runtime.alive {
            RuntimeState::Exited
        } else {
            RuntimeState::Running
        }
    }

    /// Whether the active pi process is alive and has not exited — the only
    /// state in which commands sent over its stdin can still be answered. A
    /// never-started, stopped, or dead process all read as not live.
    pub(super) fn runtime_is_live(&self) -> bool {
        matches!(self.runtime_state(), RuntimeState::Running)
    }

    /// Spawn the pi process (Start button). No-op while one is running.
    pub(super) fn runtime_start(&mut self, cx: &mut Context<Self>) {
        if self.client.is_some() {
            return;
        }
        let cwd = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        match self.extensions.spawn(&cwd, true, &self.mcp.secret_env()) {
            Ok(client) => {
                self.adopt_client(client);
                self.mcp_stamp = self.mcp.fingerprint_for(Some(&cwd));
                self.send(CommandBody::GetState, "get_state");
                self.refresh_catalogs();
                // Capability probes queue after the state request.
                self.probe_auth();
                self.toast_info(tr!("runtime.pi_process_started"));
            }
            Err(err) => {
                let message = tr!("runtime.pi_spawn_failed", error = err);
                self.client = None;
                self.runtime.error = Some(message.clone());
                self.toast_error(message);
            }
        }
        cx.notify();
    }

    /// Stop the pi process (Stop button).
    pub(super) fn runtime_stop(&mut self, cx: &mut Context<Self>) {
        if self.client.is_none() {
            return;
        }
        self.drop_client();
        self.busy = false;
        // The prompt this process was running has no answer coming: close the
        // transcript's live row too, or it would spin on a dead pi.
        self.transcript.abandon_reply();
        self.toast_info(tr!("runtime.pi_process_stopped"));
        cx.notify();
    }

    /// Stop then start the pi process (Restart button).
    pub(super) fn runtime_restart(&mut self, cx: &mut Context<Self>) {
        self.drop_client();
        self.runtime_start(cx);
    }

    /// Register the active client under the session file pi reports. The
    /// startup process has no known path until pi's first state/stats
    /// response; a `new_session` re-keys it the same way (the handler clears
    /// `current_session_path` and the next response adopts the new file).
    /// An explicit switch never re-keys — its path is already claimed.
    pub(super) fn adopt_session_file(&mut self, file: PathBuf) {
        if self.current_session_path.is_none() {
            self.current_session_path = Some(file);
        }
    }

    /// Park a background session, evicting the least-recently parked idle one
    /// if over the cap. Running sessions are never evicted.
    pub(super) fn park(&mut self, path: PathBuf, mut parked: ParkedSession) {
        parked.parked_at = Instant::now();
        if self.lives.len() >= MAX_LIVE_SESSIONS {
            let victim = self
                .lives
                .iter()
                .filter(|(_, p)| !p.busy)
                .min_by_key(|(_, p)| p.parked_at)
                .map(|(k, _)| k.clone());
            if let Some(victim) = victim {
                self.lives.remove(&victim);
            }
        }
        self.lives.insert(path, parked);
    }

    /// Reap idle parked processes past [`PARKED_IDLE_TTL`]. Running sessions
    /// stay regardless of age. Called from the heartbeat; a no-op when nothing
    /// is parked.
    pub(super) fn reap_idle_parked(&mut self) {
        if self.lives.is_empty() && self.pending_parks.is_empty() {
            return;
        }
        let now = Instant::now();
        let alive = |parked: &ParkedSession| {
            parked.busy || now.duration_since(parked.parked_at) < PARKED_IDLE_TTL
        };
        self.lives.retain(|_, parked| alive(parked));
        self.pending_parks.retain(alive);
    }
}
