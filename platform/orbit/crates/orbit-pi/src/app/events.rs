use super::*;
use crate::toast::ToastKind;

/// One tick's worth of results from draining a background session.
struct ParkedDrain {
    /// An event changed the parked state — repaint.
    changed: bool,
    /// The session is mid-run (drives the sidebar loader).
    busy: bool,
    /// The process named its session file; move it from the pending pool into
    /// `lives` under this path.
    rekey: Option<PathBuf>,
}

impl OrbitApp {
    /// Heartbeat (~90ms): drain protocol events into the UI. Returns whether a
    /// run is in flight so the caller can repaint faster while tokens stream.
    pub(crate) fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        // Let a lapsed status message disappear from the status bar.
        if self
            .status_at
            .is_some_and(|at| at.elapsed() >= STATUS_MESSAGE_TTL)
        {
            self.status_at = None;
            cx.notify();
        }
        // Retire toasts past their TTL (plus the fade-out, when animated).
        let toast_fade = if theme::reduce_motion(cx) {
            Duration::ZERO
        } else {
            crate::toast::FADE
        };
        if self.toasts.expire(Instant::now(), toast_fade) {
            cx.notify();
        }
        // The new-task page's dithered field is alive: advance its clock and
        // repaint. Only that page pays for it, and Reduce Motion freezes it.
        if self.advance_backdrop(theme::reduce_motion(cx)) {
            cx.notify();
        }
        // Session retention: once per launch, archive or delete sessions the
        // user's policy marks idle. A no-op until enabled in Settings.
        self.run_session_retention_once(cx);
        self.tick_background(cx);
        // MCP: external-change detection, the debounced `pi mcp list` probe,
        // and a deferred restart that applies new server configuration while
        // preserving the session.
        self.tick_mcp(cx);
        // Persist a settled panel layout (a drag writes once it stops).
        crate::layout::flush_if_settled();
        // Bound the warm-session pool: reap idle parked processes past the TTL.
        self.reap_idle_parked();
        // A banner click routes back to the session it announced.
        if let Some(path) = notifications::take_clicked_session() {
            self.activate_session_from_notification(PathBuf::from(path), cx);
        }
        // Background update checks and the install handoff report here.
        self.drain_updater_events(cx);
        // Sessions can be written by the CLI or another Orbit window; the
        // watcher has already scanned off-thread, so this only swaps the list.
        if let Some(reloaded) = self
            .session_watcher
            .as_ref()
            .and_then(sessions::SessionWatcher::take_reload)
        {
            // A reload that changed nothing (e.g. a bookkeeping append the
            // scan already reflected) must not rebuild the sidebar or repaint.
            if reloaded != self.sessions {
                self.sessions = reloaded;
                self.prune_workflow_store();
                self.sync_session_menu(cx);
                // The usage index is built from the same files; a write means
                // the analytics are stale (rate-limited inside the page).
                if self.usage_open {
                    self.usage.update(cx, |page, cx| page.mark_stale(cx));
                }
                cx.notify();
            }
        }
        // Usage: drain any finished scan and recompute if the filter or the
        // index moved. Cheap when nothing changed.
        self.usage.update(cx, |page, cx| page.sync(cx));
        // Workspace edits (pi, the user, or git) have no RPC event either;
        // refresh Review, the Git page, and the branch chip when the tree
        // changes.
        self.sync_workspace_watcher(cx);
        let workspace_dirty = self
            .workspace_watcher
            .as_ref()
            .is_some_and(watch::WorkspaceWatcher::take_dirty);
        if workspace_dirty {
            self.sidepane
                .update(cx, |pane, cx| pane.mark_review_stale(cx));
            self.git_panel.update(cx, |panel, cx| panel.refresh(cx));
            self.project_panel
                .update(cx, |panel, cx| panel.mark_stale(cx));
            if let Some(path) = self.file_viewer.read(cx).active_path() {
                self.file_viewer
                    .update(cx, |viewer, cx| viewer.reload(&path, cx));
            }
            self.refresh_branch_status(cx);
            // External `git worktree add`/`remove` lands as a workspace
            // change; re-list the page so Git's own worktrees appear (§30).
            self.refresh_worktrees_soon(cx);
            cx.notify();
        }
        // Expire a stalled login and auto-cancel it with pi.
        let had_login = self.auth.login().is_some();
        let auth_effects = self.auth.poll(Instant::now());
        self.handle_auth_effects(auth_effects, cx);
        if had_login || self.auth.login().is_some() {
            cx.notify();
        }
        // Keep the Runtime panel's liveness fresh (try_wait is cheap).
        if let Some(client) = self.client.as_mut() {
            self.runtime.alive = client.is_alive();
        }
        // Images pasted in the composer become message attachments.
        self.drain_pasted_images(cx);
        // Refresh the in-transcript find hits when the query or messages moved.
        self.sync_transcript_search(cx);
        // A drag that left the window clears gpui's active drag without any
        // element event seeing it — the heartbeat drops a stale highlight.
        if self.file_drag_hovered && !cx.has_active_drag() {
            self.file_drag_hovered = false;
            cx.notify();
        }
        let events = {
            let Some(client) = self.client.as_ref() else {
                return self.is_active();
            };
            client.drain_events()
        };
        // The bridge writes snapshots into the session file, not the event
        // stream; poll for new entries on a slow clock independent of events.
        self.poll_quota_entries();
        let copy_pending = self.transcript.prune_copy_feedback();
        // The one-time rail hint dismisses itself once its TTL lapses.
        if self.transcript.rail_hint_timed_out() {
            cx.notify();
        }
        if events.is_empty() {
            // Keep the live "Working for…" clock moving while a turn is open.
            if self.busy || self.transcript.is_streaming() || copy_pending {
                cx.notify();
            }
            return self.is_active();
        }

        let mut refresh_sessions = false;
        for event in &events {
            match event {
                Event::AgentStart => {
                    self.busy = true;
                }
                // An extension dialog blocks the run until the client answers.
                // Render it natively (select / confirm / input / editor); the
                // user's reply on the modal unblocks pi.
                Event::ExtensionUiRequest { id, method, value } => {
                    self.handle_extension_ui_request(id.clone(), method, value, cx);
                    // A dialog blocks the run until answered — the one state a
                    // user who walked away cannot otherwise discover.
                    self.notify_input_needed(method, value);
                    cx.notify();
                }
                Event::SessionInfoChanged { name } => {
                    // pi renamed the session (its own auto-title, the popover's
                    // Generate title, or the rename field echoing back).
                    // `session_name` wins over `current_title` in the header,
                    // so both move together or a re-title would not show.
                    match name {
                        Some(name) => {
                            self.current_title = Some(name.clone());
                            // Seed the rename field from the new name, unless
                            // the user is mid-edit; a manual generation owns
                            // the field it asked to refresh.
                            let seed = self.session_name.is_none() || self.title_generating;
                            self.session_name = Some(name.clone());
                            if seed {
                                self.session_name_input
                                    .update(cx, |input, cx| input.set_text(name, cx));
                            }
                        }
                        None => self.session_name = None,
                    }
                    self.title_generating = false;
                    refresh_sessions = true;
                }
                Event::Auth(event) => {
                    let effects = self.auth.on_event(event.clone());
                    self.handle_auth_effects(effects, cx);
                }
                Event::AutoRetryStart { value } => {
                    // A transient provider error is not a user-facing failure:
                    // it rides the quiet run-status strip (with the attempt
                    // counter) until the retry resolves — never the banner.
                    self.retrying = true;
                    self.retry_detail = Some(RetryDetail {
                        attempt: value.get("attempt").and_then(Value::as_u64).unwrap_or(1),
                        max: value.get("maxAttempts").and_then(Value::as_u64),
                        error: value
                            .get("errorMessage")
                            .and_then(Value::as_str)
                            .unwrap_or(&tr!("events.transient_error"))
                            .to_string(),
                    });
                }
                Event::AutoRetryEnd { value } => {
                    self.retrying = false;
                    self.retry_detail = None;
                    let success = value.get("success").and_then(|v| v.as_bool());
                    if success == Some(false) {
                        let error = value
                            .get("finalError")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| tr!("events.retries_exhausted"));
                        self.set_error(tr!("events.auto_retry_failed", error = error));
                    }
                }
                Event::QueueUpdate { value } => {
                    self.queue = PendingQueue::from_value(value);
                    // pi confirmed the optimistic follow-up; stop tracking it.
                    let confirmed = self
                        .pending_follow_up
                        .as_ref()
                        .is_some_and(|pending| self.queue.follow_up.iter().any(|t| t == pending));
                    if confirmed {
                        self.pending_follow_up = None;
                    }
                }
                Event::ExtensionError { value } => {
                    let path = value
                        .get("extensionPath")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let name = Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .filter(|n| !n.is_empty())
                        .unwrap_or("extension");
                    let hook = value
                        .get("event")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| tr!("events.unknown"));
                    let error = value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| tr!("events.unknown_error"));
                    self.set_error(tr!(
                        "events.extension_failed",
                        name = name,
                        hook = hook,
                        error = error
                    ));
                    // A failed title command never sends its
                    // `session_info_changed`, so drop the in-flight flag here.
                    self.title_generating = false;
                }
                Event::AgentSettled => {
                    self.busy = false;
                    self.retrying = false;
                    self.retry_detail = None;
                    self.pending_follow_up = None;
                    // A settled run has no queued continuation left; clear the
                    // bar even if the final `queue_update` was missed.
                    self.queue = PendingQueue::default();
                    refresh_sessions = true;
                    self.refresh_context_stats();
                    #[cfg(debug_assertions)]
                    self.log_turn_diagnostics();
                    // The bridge appends its post-turn snapshot at settle;
                    // read it now instead of waiting for the slow poll.
                    self.quota_entries_next_poll = Instant::now();
                    self.poll_quota_entries();
                    // Capture the turn's end checkpoint.
                    self.finish_turn(cx);
                    // An MCP configuration change deferred during the run
                    // applies now, with the session preserved.
                    self.mcp_after_settle(cx);
                    // The run ended: announce it if the user is elsewhere.
                    let path = self.current_session_path.clone();
                    let title = self
                        .current_title
                        .clone()
                        .or_else(|| self.session_name.clone());
                    let summary = self.transcript.latest_turn_summary();
                    self.notify_turn_finished(path.as_deref(), title.as_deref(), summary.as_ref());
                }
                Event::CompactionStart { .. } => {
                    // The run-status strip carries the in-progress state;
                    // no transient status line that would lapse mid-compact.
                    self.is_compacting = true;
                }
                Event::CompactionEnd { value } => {
                    self.is_compacting = false;
                    let aborted = value.get("aborted").and_then(Value::as_bool) == Some(true);
                    // A failed compaction carries `errorMessage` (docs).
                    if let Some(error) = value.get("errorMessage").and_then(Value::as_str) {
                        self.set_error(tr!("events.compaction_failed", error = error));
                    } else if aborted {
                        self.set_status(tr!("events.compaction_aborted"));
                    } else {
                        // Success: pi replaced the summarized span with a
                        // compaction summary plus the kept entries. Re-read
                        // the projection so the boundary card lands in the
                        // transcript (and pre-compaction rows drop).
                        self.send(CommandBody::GetMessages, "get_messages");
                    }
                    // Post-compaction usage is unknown until the next turn;
                    // refresh so the meter can show an empty/unknown state.
                    self.refresh_context_stats();
                }
                Event::ProcessExited => {
                    self.busy = false;
                    self.retrying = false;
                    self.retry_detail = None;
                    self.pending_follow_up = None;
                    self.queue = PendingQueue::default();
                    // The run is gone; its dialog (if any) can never be
                    // answered, so drop it without a reply.
                    self.dialog = None;
                    self.dialog_focus_pending = false;
                    self.approval = None;
                    self.approval_focus_pending = false;
                    self.runtime.alive = false;
                    self.runtime.exited = true;
                    self.auth.on_disconnect();
                    self.set_error(tr!("events.process_exited"));
                }
                Event::MessageEnd { value } => {
                    // A failed LLM call ends the assistant message with
                    // `stopReason: "error"` + `errorMessage` (e.g. an
                    // unsupported model). Surface it in the banner; the
                    // transcript renders the same text inline.
                    if let Some(error) = transcript::message_error(value) {
                        self.set_error(tr!("events.agent_error", error = error));
                    } else if self.error.as_deref().is_some_and(|error| {
                        error.starts_with(tr!("events.agent_error_prefix").as_str())
                    }) {
                        // The next attempt produced a message — clear the
                        // stale agent-error banner.
                        self.error = None;
                    }
                    // Real edit stats from finalized tool calls.
                    let (a, r) = transcript::diff_from_message(value);
                    self.added += a;
                    self.removed += r;
                }
                Event::Response {
                    command,
                    success,
                    data,
                    error,
                    id: _,
                } => {
                    self.on_response(
                        command,
                        *success,
                        data.as_ref(),
                        error.as_deref(),
                        &mut refresh_sessions,
                        cx,
                    );
                }
                // A running `ask_user_question` tool is tracked so its
                // `select` / `input` primitives route to the inline panel.
                Event::ToolExecutionStart { value } => {
                    self.on_ask_tool_start(value, cx);
                }
                Event::ToolExecutionEnd { value } => {
                    self.on_ask_tool_end(value, cx);
                }
                _ => {}
            }
            if self.transcript.apply_event(event) {
                cx.notify();
            }
        }
        if refresh_sessions {
            self.sessions = sessions::load_sessions();
            self.prune_workflow_store();
            self.sync_session_menu(cx);
            cx.notify();
        }
        // Responses can change app state without touching the transcript
        // (model/thinking labels, catalogs, status) — always redraw a frame
        // in which events were processed so those changes become visible.
        cx.notify();
        self.is_active()
    }

    /// Step the new-task backdrop's field. Returns whether anything changed and
    /// the window should repaint.
    ///
    /// The phase is wall-clock seconds, not a tick counter: the heartbeat's
    /// cadence changes with the run state, and the field must not speed up
    /// while something else streams. The clock only runs while the page is
    /// actually on screen (`render` records that), so a backdrop nobody can see
    /// costs nothing. Reduce Motion holds the frame it was showing — a halftone
    /// is a perfectly good still.
    fn advance_backdrop(&mut self, reduce_motion: bool) -> bool {
        if !self.backdrop_live || reduce_motion {
            self.backdrop_clock = None;
            return false;
        }
        // Resume from the phase already on screen rather than snapping back to
        // zero: a page the reader returns to keeps the screen it left.
        if self.backdrop_clock.is_none() {
            let resumed = Duration::from_secs_f32(self.backdrop_phase);
            self.backdrop_clock = Some(Instant::now() - resumed);
        }
        let Some(clock) = self.backdrop_clock else {
            return false;
        };
        self.backdrop_phase = clock.elapsed().as_secs_f32();
        true
    }

    /// True while a run is in flight. The heartbeat polls fast (instead of the
    /// idle ~90 ms cadence) when this holds, so streamed tokens land smoothly
    /// rather than in visible batches, and drops back the moment it settles.
    fn is_active(&self) -> bool {
        self.busy || self.transcript.is_streaming() || self.retrying || self.is_compacting
    }

    /// Debug-build-only, local per-turn diagnostic (not telemetry: it never
    /// leaves the process, never touches the network, and is compiled out of
    /// release builds). Prints the context window, this turn's cache hit,
    /// uncached prompt, output, duration, and average rate so a "lower tok/s"
    /// can be attributed to prefill vs. decode without guessing.
    #[cfg(debug_assertions)]
    fn log_turn_diagnostics(&self) {
        let Some(usage) = self.transcript.last_message_usage() else {
            return;
        };
        let window = self
            .context
            .as_ref()
            .map(|c| c.context_window)
            .unwrap_or(0);
        let cache_pct = usage
            .cache_read_percent()
            .map(|p| format!("{p:.1}%"))
            .unwrap_or_else(|| "n/a".into());
        let duration = self.transcript.last_message_duration();
        let tps = duration
            .filter(|d| d.as_secs_f64() >= 1.0 && usage.output > 0)
            .map(|d| usage.output as f64 / d.as_secs_f64());
        eprintln!(
            "[orbit:perf] turn settled — window {window} tokens | cache {} ({cache_pct} hit) | prompt {} uncached | output {} | {}ms | {} tok/s avg",
            usage.cache_read,
            usage.input,
            usage.output,
            duration
                .map(|d| d.as_millis().to_string())
                .unwrap_or_else(|| "?".into()),
            tps.map(|t| format!("{t:.0}")).unwrap_or_else(|| "?".into()),
        );
    }

    /// Drain background (parked) sessions. Their runs continue in their own
    /// pi processes; events keep their transcripts current, so reopening a
    /// parked session resumes the live stream exactly where it left off.
    pub(super) fn tick_background(&mut self, cx: &mut Context<Self>) {
        if self.lives.is_empty() && self.pending_parks.is_empty() {
            return;
        }
        let mut changed = false;
        let mut any_busy = false;
        let mut dead: Vec<PathBuf> = Vec::new();
        // Settles to announce once the `lives` borrow (and the map itself)
        // are no longer held.
        let mut finished: Vec<(PathBuf, Option<String>, Option<transcript::TurnSummary>)> =
            Vec::new();
        for (path, parked) in self.lives.iter_mut() {
            let drained =
                Self::drain_parked_session(parked, Some(path), &self.sessions, &mut finished);
            changed |= drained.changed;
            any_busy |= drained.busy;
            if !parked.client.is_alive() {
                dead.push(path.clone());
            }
        }
        for path in dead {
            self.lives.remove(&path);
        }
        // Pending parks: live processes whose session file pi has not named
        // yet. They drain like any parked session (a run started during boot
        // keeps going) and join `lives` under their file the moment the boot
        // handshake reveals it. A process that never claims a file is dropped
        // only when it exits or the idle TTL reaps it.
        let mut rekeyed: Vec<(PathBuf, ParkedSession)> = Vec::new();
        let mut ix = 0;
        while ix < self.pending_parks.len() {
            let drained = Self::drain_parked_session(
                &mut self.pending_parks[ix],
                None,
                &self.sessions,
                &mut finished,
            );
            changed |= drained.changed;
            any_busy |= drained.busy;
            if !self.pending_parks[ix].client.is_alive() {
                self.pending_parks.remove(ix);
                changed = true;
            } else if let Some(path) = drained.rekey {
                let parked = self.pending_parks.remove(ix);
                rekeyed.push((path, parked));
                changed = true;
            } else {
                ix += 1;
            }
        }
        for (path, parked) in rekeyed {
            self.park(path, parked);
        }
        // A parked run that settled while the user was elsewhere still wants
        // saying. The settle also flips the sidebar's running loader, so the
        // final repaint is unconditional.
        for (path, title, summary) in finished {
            self.notify_turn_finished(Some(&path), title.as_deref(), summary.as_ref());
            changed = true;
        }
        if changed || any_busy {
            // Repaint while a background run is live (sidebar loader phase,
            // park-state changes) even though the visible transcript's
            // active client produced no events this tick.
            cx.notify();
        }
    }

    /// Drain one parked session's buffered events into its transcript and
    /// report what changed. `path` is `None` for a pending park: such a
    /// session also answers the boot handshake, because only the active view
    /// used to — a `new_session`/`clone` reply asks the process for
    /// `get_state`, and the reply naming `sessionFile` re-keys it into `lives`
    /// (issue #46).
    fn drain_parked_session(
        parked: &mut ParkedSession,
        path: Option<&Path>,
        sessions: &[SessionInfo],
        finished: &mut Vec<(PathBuf, Option<String>, Option<transcript::TurnSummary>)>,
    ) -> ParkedDrain {
        let mut changed = false;
        let mut rekey = None;
        for event in parked.client.drain_events() {
            match &event {
                Event::AgentStart => parked.busy = true,
                // `agent_settled` is the real settle (queued steering /
                // follow-up / retry can continue past `agent_end`).
                Event::AgentSettled => {
                    parked.busy = false;
                    // A settled run has no queued continuation left; clear
                    // the mirror even if the final `queue_update` was missed.
                    parked.queue = PendingQueue::default();
                    // A pending session has no sidebar row yet, so there is
                    // no path to name in a notification; it is re-keyed
                    // moments later and notices from then on.
                    if let Some(path) = path {
                        finished.push((
                            path.to_path_buf(),
                            sessions
                                .iter()
                                .find(|session| session.path == path)
                                .map(|session| session.title.clone()),
                            parked.transcript.latest_turn_summary(),
                        ));
                    }
                }
                Event::ProcessExited => {
                    parked.busy = false;
                    parked.queue = PendingQueue::default();
                }
                // pi emits `queue_update` only when the queue changes, so a
                // parked session must mirror it here to restore the pending
                // bar on resume — and to drop messages pi already delivered.
                Event::QueueUpdate { value } => {
                    parked.queue = PendingQueue::from_value(value);
                }
                // A parked session has no visible dialog surface; cancel
                // so its blocked run can settle (an active session renders
                // the dialog in `tick` above).
                Event::ExtensionUiRequest { id, .. } => {
                    let _ = parked.client.cancel_dialog(id);
                    continue;
                }
                Event::MessageEnd { value } => {
                    // Real edit stats from finalized tool calls.
                    let (a, r) = transcript::diff_from_message(value);
                    parked.added += a;
                    parked.removed += r;
                }
                Event::Response {
                    command,
                    success,
                    data,
                    ..
                } if path.is_none() && *success => {
                    let file = data
                        .as_ref()
                        .and_then(|data| data.get("sessionFile"))
                        .and_then(Value::as_str)
                        .map(PathBuf::from);
                    match command.as_str() {
                        "new_session" | "clone" => {
                            // A clone inherits the source transcript in the
                            // active view, which clears it on the reply; the
                            // parked one never saw that, so clear it here.
                            if command == "clone" {
                                parked.transcript = Transcript::new();
                                changed = true;
                            }
                            match file {
                                Some(file) => rekey = Some(file),
                                // pi creates the file but does not echo it on
                                // this reply; the follow-up `get_state` names
                                // it (same order the active view uses).
                                None => {
                                    let _ = parked.client.send(CommandBody::GetState);
                                }
                            }
                        }
                        "get_state" | "get_session_stats" => rekey = file,
                        _ => {}
                    }
                }
                _ => {}
            }
            changed |= parked.transcript.apply_event(&event);
        }
        ParkedDrain {
            changed,
            busy: parked.busy,
            rekey,
        }
    }

    /// Replace the transcript with a `get_messages`-shaped snapshot and
    /// rebuild the top-bar diff counts from it. Shared by pi's authoritative
    /// `get_messages` response and the disk preview that paints a cold session
    /// before pi answers.
    pub(super) fn apply_messages_snapshot(&mut self, data: &Value) {
        self.transcript.load_from(data);
        self.added = 0;
        self.removed = 0;
        if let Some(messages) = data.get("messages").and_then(Value::as_array) {
            for message in messages {
                let (a, r) = transcript::diff_from_message(message);
                self.added += a;
                self.removed += r;
            }
        }
    }

    pub(super) fn on_response(
        &mut self,
        command: &str,
        success: bool,
        data: Option<&serde_json::Value>,
        error: Option<&str>,
        refresh_sessions: &mut bool,
        cx: &mut Context<Self>,
    ) {
        // Provider-auth commands are handled here so success/failure and the
        // no-payload replies (`auth.cancel`) never fall through the
        // data-required branch below.
        if command.starts_with("auth.") {
            self.on_auth_response(command, success, data, error, cx);
            return;
        }
        // Account quota/balance/spend from the `quota.*` namespace; carries no
        // secrets, so it merges straight into the cache.
        if command == "quota.list" {
            self.quota.on_response(success, data, error);
            // Stop the manual refresh's spin, held for its minimum duration so
            // an immediate reply still reads as a deliberate refresh.
            self.finish_quota_refresh(cx);
            cx.notify();
            return;
        }
        // The bundled bridge appends quota snapshots as session entries; poll
        // responses merge them and must not surface a stale-cursor error as a
        // user-facing banner.
        if command == "get_entries" {
            self.on_entries_response(success, data, error, cx);
            return;
        }
        // Error handling (docs #error-handling): a failed command carries an
        // `error` string. Surface it, unwind command-specific optimistic
        // state, and stop — success paths below assume `data` is valid.
        if !success {
            self.on_command_failure(command, error, cx);
            match command {
                "compact" => {
                    self.is_compacting = false;
                    self.refresh_context_stats();
                }
                // A failed switch can never complete the armed default push;
                // disarm so a later manual pick of the same model cannot be
                // overridden by the configured thinking level.
                "set_model" => {
                    self.default_model_armed = false;
                    self.send(CommandBody::GetState, "get_state");
                }
                // These changed local UI optimistically; re-read pi's state.
                "cycle_model"
                | "set_thinking_level"
                | "cycle_thinking_level"
                | "set_session_name" => {
                    self.send(CommandBody::GetState, "get_state");
                }
                _ => {}
            }
            return;
        }
        // A successful retry clears the banner it raised earlier.
        self.clear_error_for(command);
        // Commands whose replies carry no payload (e.g. `set_thinking_level`
        // answers `{"success":true}` with no `data`). They still need their
        // follow-up state refresh, so they are matched BEFORE the
        // data-required branch below.
        match command {
            "set_model" => {
                self.send(CommandBody::GetState, "get_state");
                self.send(
                    CommandBody::GetAvailableThinkingLevels,
                    "get_available_thinking_levels",
                );
                return;
            }
            "cycle_model" | "set_thinking_level" | "cycle_thinking_level" => {
                self.send(CommandBody::GetState, "get_state");
                return;
            }
            // Agent-control commands answer with no payload; nothing more to do.
            "set_steering_mode"
            | "set_follow_up_mode"
            | "set_auto_compaction"
            | "set_auto_retry"
            | "abort_retry"
            | "set_session_name"
            | "steer"
            | "follow_up" => {
                return;
            }
            // `compact` also has a result payload on success; clearing the
            // compacting state belongs before the data gate.
            "compact" => {
                self.is_compacting = false;
                self.toast_info(tr!("events.context_compacted"));
                self.refresh_context_stats();
                self.send(CommandBody::GetState, "get_state");
                return;
            }
            // `clone` duplicates the session and moves the process onto the
            // copy. Adopt the new file/id when pi reports them, then reload.
            "clone" => {
                self.transcript.clear();
                self.current_title = None;
                self.reset_session_name(cx);
                self.current_session_path = None;
                self.added = 0;
                self.removed = 0;
                self.context = None;
                self.session_usage = None;
                self.reset_turns();
                self.reset_queue();
                self.session_id = None;
                // A clone mirrors its source session; it is not put on the
                // default model.
                self.default_model_armed = false;
                if let Some(data) = data.as_ref() {
                    if let Some(file) = data.get("sessionFile").and_then(Value::as_str) {
                        self.adopt_session_file(PathBuf::from(file));
                    }
                    if let Some(id) = data.get("sessionId").and_then(Value::as_str) {
                        self.session_id = Some(id.to_string());
                        self.reset_quota_entries();
                    }
                }
                self.sync_review_context(cx);
                *refresh_sessions = true;
                self.send(CommandBody::GetMessages, "get_messages");
                self.send(CommandBody::GetState, "get_state");
                self.refresh_catalogs();
                return;
            }
            _ => {}
        }

        let Some(data) = data else { return };
        match command {
            "get_state" => {
                if let Some(model) = data.get("model") {
                    if let Some(name) = model.get("name").and_then(serde_json::Value::as_str) {
                        self.model_label = name.to_string();
                    }
                    if let Some(id) = model.get("id").and_then(serde_json::Value::as_str) {
                        self.model_id = id.to_string();
                    }
                    if let Some(provider) =
                        model.get("provider").and_then(serde_json::Value::as_str)
                    {
                        self.model_provider = provider.to_string();
                    }
                }
                // The Git page labels its context with the provider/model.
                self.sync_review_context(cx);
                if let Some(level) = data
                    .get("thinkingLevel")
                    .and_then(serde_json::Value::as_str)
                {
                    self.thinking_label = level.to_string();
                }
                if let Some(file) = data.get("sessionFile").and_then(Value::as_str) {
                    self.adopt_session_file(PathBuf::from(file));
                }
                if let Some(id) = data.get("sessionId").and_then(Value::as_str) {
                    if self.session_id.as_deref() != Some(id) {
                        self.session_id = Some(id.to_string());
                        // Entry ids are per-session; a different session means
                        // the bridge cursor must start over.
                        self.reset_quota_entries();
                        self.recover_latest_turn(cx);
                        self.sync_review_context(cx);
                    }
                }
                // Agent control surface: streaming/compaction state, queue
                // modes, auto-compaction, and the session display name.
                let state = SessionState::from_value(data);
                self.busy = state.is_streaming || state.is_compacting;
                self.is_compacting = state.is_compacting;
                self.follow_up_mode = state.follow_up_mode;
                self.auto_compaction = state.auto_compaction_enabled;
                match &state.session_name {
                    Some(name) if self.session_name.as_deref() != Some(name.as_str()) => {
                        self.session_name = Some(name.clone());
                        let name = name.clone();
                        self.session_name_input
                            .update(cx, |input, cx| input.set_text(name, cx));
                    }
                    None if self.session_name.is_some() => {
                        self.session_name = None;
                        self.seed_session_name_input(cx);
                    }
                    _ => {}
                }
                self.sync_model_selector(cx);
                self.refresh_context_stats();
                // Capability probe: a pi that advertises `custom` will stream
                // `method:"custom"` frames for `ctx.ui.custom()`.
                if let Some(methods) = data
                    .get("capabilities")
                    .and_then(|caps| caps.get("extension_ui"))
                    .and_then(Value::as_array)
                {
                    self.custom_ui_supported = methods
                        .iter()
                        .any(|method| method.as_str() == Some("custom"));
                }
                // A `new_session` birth may still need its default model
                // pushed; this is a no-op otherwise.
                self.apply_default_model(cx);
            }
            "get_messages" => {
                self.apply_messages_snapshot(data);
                self.refresh_context_stats();
            }
            "get_available_models" => {
                self.available_models = data
                    .get("models")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                let id = m.get("id").and_then(serde_json::Value::as_str)?;
                                let name = m.get("name").and_then(serde_json::Value::as_str)?;
                                let provider =
                                    m.get("provider").and_then(serde_json::Value::as_str)?;
                                Some(ModelEntry {
                                    id: id.into(),
                                    name: name.into(),
                                    provider: provider.into(),
                                    context_window: m
                                        .get("contextWindow")
                                        .and_then(serde_json::Value::as_u64),
                                    thinking_levels: catalog_thinking_levels(m),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.sync_model_selector(cx);
                // The model catalog arrived after the session: retry a default
                // that could not be resolved yet.
                self.apply_default_model(cx);
            }
            "get_available_thinking_levels" => {
                self.available_thinking_levels = data
                    .get("levels")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                self.sync_model_selector(cx);
            }
            "get_commands" => {
                let home = crate::platform::home_dir_opt();
                let workspace = self.current_workspace.clone();
                self.slash_commands = data
                    .get("commands")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| {
                                let name = c.get("name").and_then(Value::as_str)?;
                                let description =
                                    c.get("description").and_then(Value::as_str).unwrap_or("");
                                // `source` + `sourceInfo` carry the command's
                                // provenance; Orbit folds it into a scope badge.
                                let source = c.get("source").and_then(Value::as_str).unwrap_or("");
                                let info = c.get("sourceInfo");
                                let scope =
                                    info.and_then(|i| i.get("scope")).and_then(Value::as_str);
                                let origin =
                                    info.and_then(|i| i.get("origin")).and_then(Value::as_str);
                                let path = info.and_then(|i| i.get("path")).and_then(Value::as_str);
                                Some(SlashCommand {
                                    name: name.to_string(),
                                    description: description.to_string(),
                                    scope: mentions::classify_scope(
                                        source,
                                        scope,
                                        origin,
                                        path,
                                        workspace.as_deref(),
                                        home.as_deref(),
                                    ),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // A `/` menu already open picks up the fresh command list.
                self.sync_autocomplete(cx);
            }
            "get_session_stats" => {
                self.context = ContextUsage::from_stats(data);
                self.session_usage = SessionUsage::from_stats(data);
                if let Some(file) = data.get("sessionFile").and_then(Value::as_str) {
                    self.adopt_session_file(PathBuf::from(file));
                }
            }
            "switch_session" => {
                if success {
                    // A resumed session keeps the model in its file (D-D).
                    self.default_model_armed = false;
                    self.reset_turns();
                    self.reset_queue();
                    self.sync_review_context(cx);
                    // Load the session *before* the slower auth/quota probes,
                    // which would otherwise queue ahead of `get_messages` and
                    // push the transcript out by a network round trip.
                    self.send(CommandBody::GetMessages, "get_messages");
                    self.send(CommandBody::GetState, "get_state");
                    self.refresh_catalogs();
                    self.probe_auth();
                }
            }
            "new_session" => {
                self.transcript.clear();
                self.current_title = None;
                self.reset_session_name(cx);
                self.current_session_path = None;
                self.added = 0;
                self.removed = 0;
                self.context = None;
                self.session_usage = None;
                // Widgets belong to the process that sent them.
                self.extension_widgets.clear();
                self.reset_turns();
                self.reset_queue();
                self.sync_review_context(cx);
                *refresh_sessions = true;
                // A fresh session is born on the default model; `set_model`
                // is pushed once `get_state` reports the new id (see
                // `apply_default_model`).
                self.default_model_armed = true;
                self.send(CommandBody::GetState, "get_state");
                self.refresh_catalogs();
            }
            // `clear_queue` echoes the text it dropped so Escape can put it
            // back in the composer (docs' interactive-Esc behavior).
            "clear_queue" => {
                let queue = PendingQueue::from_value(data);
                if self.restore_queue_on_clear {
                    self.restore_queue_on_clear = false;
                    if let Some(text) = queue.restore_text() {
                        self.input.update(cx, |input, cx| input.set_text(text, cx));
                    }
                }
                // `queue_update` mirrors the same fact; this keeps the row
                // correct even if the event is missed.
                self.queue = queue;
            }
            _ => {
                let _ = success;
            }
        }
    }

    /// Apply a provider-auth command response to the [`AuthManager`] and act
    /// on any effects it returns.
    pub(super) fn on_auth_response(
        &mut self,
        command: &str,
        success: bool,
        data: Option<&serde_json::Value>,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        match command {
            "auth.list" => {
                let before = self.auth.support();
                self.auth.on_list_response(success, data, error);
                if self.auth.support() != before && self.auth.support() == AuthSupport::Unsupported
                {
                    self.set_status(tr!("events.auth_rpc_missing"));
                }
            }
            "auth.status" => self.auth.on_status_response(success, data),
            "auth.login" => {
                self.auth.on_login_response(success, data, error);
                // Acknowledgement can arrive after a `started` event; make
                // sure the card reflects the manager either way.
            }
            "auth.logout" => {
                if let Some(provider) = data
                    .and_then(|data| data.get("provider"))
                    .and_then(serde_json::Value::as_str)
                {
                    self.auth.on_logout_response(success, provider);
                } else if success {
                    // Some servers answer without echoing the provider; the
                    // UI already refreshed via `auth_credentials_changed`.
                }
            }
            "auth.cancel" => {
                // Nothing to reconcile: the event stream confirms cancellation.
            }
            _ => {}
        }
        cx.notify();
    }

    /// Apply a `get_entries` poll for the quota bridge. A cursor can dangle
    /// when the session file changed underneath it; that is recovered by
    /// dropping the cursor and re-reading, never by a user-facing error.
    pub(super) fn on_entries_response(
        &mut self,
        success: bool,
        data: Option<&serde_json::Value>,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.quota_entries_inflight = false;
        if !success {
            let stale_cursor = self.quota_entries_cursor.is_some()
                && error.is_some_and(|error| error.contains("Entry not found"));
            if stale_cursor {
                self.quota_entries_cursor = None;
                self.quota_entries_next_poll = Instant::now();
                self.poll_quota_entries();
            }
            return;
        }
        let Some(entries) = data
            .and_then(|data| data.get("entries"))
            .and_then(serde_json::Value::as_array)
        else {
            return;
        };
        // The cursor is the last appended entry id: entries arrive in append
        // order, so the next poll asks for strictly newer ones only.
        if let Some(id) = entries
            .last()
            .and_then(|entry| entry.get("id"))
            .and_then(serde_json::Value::as_str)
        {
            self.quota_entries_cursor = Some(id.to_string());
        }
        if self.quota.on_entries(entries) {
            // A snapshot landed: no need for fast bootstrap polls.
            self.quota_entries_bootstrap = 0;
            cx.notify();
        }
    }

    /// Move images the composer collected (clipboard paste) into the
    /// attachment queue. Called on tick and again at submit so a paste
    /// immediately followed by Enter still attaches.
    pub(super) fn drain_pasted_images(&mut self, cx: &mut Context<Self>) {
        if !self.input.read(cx).has_pasted_images() {
            return;
        }
        let pasted = self
            .input
            .update(cx, |input, _| std::mem::take(&mut input.pasted_images));
        for image in &pasted {
            if self.attachments.len() >= MAX_ATTACHMENTS {
                self.set_status(tr!("composer_ops.max_attachments", count = MAX_ATTACHMENTS));
                break;
            }
            let index = self.attachments.len();
            self.push_attachment(Attachment::from_image(image, index), cx);
        }
        if !pasted.is_empty() {
            cx.notify();
        }
    }

    /// Post the "turn finished" notification for a settle. Nothing is posted
    /// while the window is frontmost except the in-app toast, and a turn the
    /// user aborted holds no news. `session` is the file path that a banner
    /// click routes back to.
    pub(super) fn notify_turn_finished(
        &mut self,
        session: Option<&Path>,
        title: Option<&str>,
        summary: Option<&transcript::TurnSummary>,
    ) {
        let Some(summary) = summary else { return };
        if summary.aborted {
            return;
        }
        let (subtitle, fallback, kind) = if summary.failed {
            (
                tr!("events.notify_turn_failed"),
                tr!("events.notify_turn_failed_body"),
                ToastKind::Error,
            )
        } else {
            (
                tr!("events.notify_turn_finished"),
                tr!("events.notify_turn_finished_body"),
                ToastKind::Success,
            )
        };
        let body = if summary.body.trim().is_empty() {
            fallback.as_str()
        } else {
            summary.body.as_str()
        };
        self.post_notification(session, title, &subtitle, body, kind);
    }

    /// A run is blocked on an extension dialog — the one event a user cannot
    /// discover once they have left the window.
    fn notify_input_needed(&mut self, method: &str, value: &Value) {
        if !matches!(method, "select" | "confirm" | "input" | "editor") {
            return;
        }
        let title = self
            .current_title
            .clone()
            .or_else(|| self.session_name.clone());
        let waiting_body = tr!("events.notify_waiting_body");
        let body = value
            .get("title")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or(&waiting_body);
        let session = self.current_session_path.clone();
        self.post_notification(
            session.as_deref(),
            title.as_deref(),
            &tr!("events.notify_waiting"),
            body,
            ToastKind::Warning,
        );
    }

    /// Deliver a background notification through every enabled channel.
    /// While the window is frontmost the banner and sound stay quiet — the
    /// in-app toast carries the event instead.
    fn post_notification(
        &mut self,
        session: Option<&Path>,
        title: Option<&str>,
        subtitle: &str,
        body: &str,
        kind: ToastKind,
    ) {
        let prefs = self.notification_prefs;
        if !prefs.desktop && !prefs.sound && !prefs.toasts {
            return;
        }
        if self.window_active {
            if prefs.toasts {
                let title = title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or("Orbit Pi");
                self.push_toast(
                    kind,
                    title,
                    Some(notifications::preview(
                        body,
                        notifications::BODY_PREVIEW_CHARS,
                    )),
                );
            }
            return;
        }
        if prefs.desktop {
            let title = title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Orbit Pi");
            notifications::notify(session, title, subtitle, body);
        }
        if prefs.sound {
            notifications::play_sound();
        }
    }

    /// Re-point the workspace watcher when the active workspace moves.
    /// Comparing the last-attempted dir (rather than the live watcher) means
    /// a backend that failed to start is retried on the next workspace
    /// change, not every heartbeat. The branch chip belongs to the workspace,
    /// so it refetches here too.
    pub(super) fn sync_workspace_watcher(&mut self, cx: &mut Context<Self>) {
        if self.workspace_watch_dir == self.current_workspace {
            return;
        }
        self.workspace_watch_dir = self.current_workspace.clone();
        self.workspace_watcher = self
            .current_workspace
            .as_deref()
            .and_then(watch::WorkspaceWatcher::start);
        self.refresh_branch_status(cx);
        // The status bar's "Work in" chip and the Worktrees page both read the
        // repository's worktree list, so keep it current on every workspace
        // move (one background `git worktree list`).
        self.refresh_worktrees(cx);
    }
}
