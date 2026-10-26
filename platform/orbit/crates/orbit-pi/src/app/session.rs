use super::helpers::*;
use super::mcp_ui::QuotaPopupTab;
use super::*;
use crate::app::helpers::{segmented_segment, SegmentPosition};
use crate::context_meter::context_ring;
use crate::quota::{is_five_hour_window, note_should_render, QuotaHeadline};
use crate::theme::tokens::{
    input, popover, ButtonSize, DynamicSpacing, IconSize, Radius, StyledExt, TextSize,
};
use crate::usage::tooltip::Tooltip;

/// A workspace-relative `/`-separated path for the Files toolbar. Falls back
/// to the absolute path when the file lies outside the workspace.
fn relative_display(root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// Minimum time a manual quota refresh keeps its spinner turning, so a reply
/// that lands in a few milliseconds still reads as a deliberate refresh rather
/// than a one-frame flicker.
const QUOTA_REFRESH_MIN_SPIN: Duration = Duration::from_millis(450);

/// Ceiling for a `quota.list` reply. Past it the spinner self-clears, so a
/// bridge-only pi (no `quota.list`) can never leave the button stuck spinning.
const QUOTA_REFRESH_TIMEOUT: Duration = Duration::from_millis(4000);

/// Top-bar provider-quota popover width: room for a provider name, its plan
/// badge, and a window row.
const QUOTA_POPOVER_W: f32 = 320.;
/// Largest total height of the quota popover before its card list scrolls.
const QUOTA_POPOVER_MAX_H: f32 = 460.;

/// How much a hidden provider card is dimmed inside the popover's "Hidden"
/// group: present and readable, but visibly out of the active list.
const HIDDEN_CARD_DIM: f32 = 0.6;

impl OrbitApp {
    /// True while a run is in flight: the busy flag, a streaming transcript, or
    /// a prompt that is still waiting on its first step.
    pub(super) fn is_running(&self) -> bool {
        self.busy || self.transcript.is_streaming() || self.transcript.is_awaiting_reply()
    }

    /// Whether the active model offers a thinking level to pick. pi answers
    /// `["off"]` for a model without reasoning, and the list is empty until
    /// `get_available_thinking_levels` replies, so neither case has a choice to
    /// offer — one rule, read by the composer chip and the command palette
    /// alike, so the chip and its shortcut can never disagree about whether the
    /// picker has options.
    pub(super) fn can_choose_thinking(&self) -> bool {
        self.available_thinking_levels.len() > 1
    }

    /// Whether New Task can reuse the active process in place with
    /// `new_session`: only when no turn is in flight and the process is
    /// alive. A run in flight must be parked so `new_session` cannot abort
    /// it; a dead process must be replaced by a fresh spawn instead of
    /// sending the command into a stdin nobody reads (issue #30).
    pub(super) fn can_reuse_session(&self) -> bool {
        !self.is_running() && self.runtime_is_live()
    }

    pub(super) fn submit(&mut self, text: String, cx: &mut Context<Self>) {
        self.submit_as(text, theme::get(cx).ui.composer_send_mode, cx);
    }

    /// Keyboard path for explicit steering, independent of the Enter preference.
    pub(super) fn on_steer(
        &mut self,
        _: &crate::SteerRun,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_composer_as(SendMode::Steer, window, cx);
    }

    pub(super) fn on_send_alternate(
        &mut self,
        _: &crate::SendAlternate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_composer_as(theme::get(cx).ui.composer_send_mode.opposite(), window, cx);
    }

    fn submit_composer_as(&mut self, mode: SendMode, window: &mut Window, cx: &mut Context<Self>) {
        // Enter can be the key that confirms an IME candidate; never submit a
        // composition in flight (the platform routes it to the IME first, but
        // other backends may dispatch the action anyway).
        {
            let input = self.input.read(cx);
            if input.is_composing() || !input.focus_handle(cx).is_focused(window) {
                return;
            }
        }
        if self.commit_autocomplete_if_open(cx) {
            return;
        }
        let text = self.input.read(cx).text();
        self.submit_as(text, mode, cx);
    }

    /// Send the composer's current text/attachments as a steer; used by the
    /// composer's steer control.
    pub(super) fn steer_current(&mut self, cx: &mut Context<Self>) {
        if self.input.read(cx).is_composing() {
            return;
        }
        let text = self.input.read(cx).text();
        self.submit_as(text, SendMode::Steer, cx);
    }

    /// The image payload pi expects on `prompt` / `follow_up` / `steer`.
    fn prompt_images(attachments: &[Attachment]) -> Option<Vec<Value>> {
        if attachments.is_empty() {
            None
        } else {
            Some(
                attachments
                    .iter()
                    .map(|a| a.to_prompt_image())
                    .collect::<Vec<_>>(),
            )
        }
    }

    pub(super) fn submit_as(&mut self, text: String, mode: SendMode, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        // Collect any paste that raced the submit tick.
        self.drain_pasted_images(cx);
        // pi's `follow_up`, `steer`, and `prompt` all carry images, so
        // attachments ride whatever we send.
        let attachments = std::mem::take(&mut self.attachments);
        // While the agent is running, steer redirects the live turn and
        // follow-up waits for it to settle. pi emits the user message into the
        // transcript when it is actually delivered; until then the queue bar
        // above the composer mirrors it.
        let running = self.is_running();
        let (body, label) = mode.command(running, text.clone(), Self::prompt_images(&attachments));
        if !self.send(body, label) {
            // Keep the prompt and attachments so nothing is lost.
            self.attachments = attachments;
            cx.notify();
            return;
        }
        if running {
            // Show it immediately; the next `queue_update` reconciles the list.
            match mode {
                SendMode::Steer => self.queue.steering.push(text),
                SendMode::FollowUp => {
                    self.queue.follow_up.push(text.clone());
                    self.pending_follow_up = Some(text);
                }
            }
            self.input.update(cx, |input, cx| input.clear(cx));
            cx.notify();
            return;
        }
        // Not running: a normal prompt starts a new turn. Sending it commits
        // the user to a folder, so make sure it is in Orbit's own sidebar
        // list — the launch-cwd path never picked one explicitly.
        if let Some(cwd) = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
        {
            self.add_workspace(cwd, cx);
        }
        self.begin_turn(cx);
        // The turn is in flight from here, not from pi's `agent_start`: the
        // composer flips to Stop, the heartbeat polls at 60 Hz, and every
        // `is_running()` gate agrees, so a slow provider (or a pi that is still
        // booting) can never look like a dropped keystroke. `agent_start`
        // re-sets the same flag, and every settle path clears it.
        self.busy = true;
        // Same for the transcript: the live "Working for…" row goes up now,
        // with its clock running from the send.
        self.transcript.expect_reply();
        // Show the prompt immediately — pi does not echo it back in RPC mode.
        // The decoded previews ride along so the chat window shows what was
        // attached (pi echoes/snapshots carry image blocks for reloads).
        self.transcript.append_user_message(
            &text,
            attachments
                .iter()
                .filter_map(|a| a.preview.clone())
                .collect(),
        );
        self.input.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    /// Start a new user turn: snapshot the workspace so Review's **Last Turn**
    /// can diff exactly what the agent changes, then let the prompt run.
    pub(super) fn begin_turn(&mut self, cx: &mut Context<Self>) {
        if self.turn_open {
            return;
        }
        let Some(session) = self.session_id.clone() else {
            return;
        };
        self.turn_count += 1;
        self.turn_open = true;
        let turn = self.turn_count;
        let cwd = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        cx.spawn(async move |_this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move { checkpoint::capture_turn_start(&cwd, &session, turn) })
                .await;
        })
        .detach();
    }

    /// A run settled: snapshot the workspace's end state so Review's **Last
    /// Turn** has a complete range, then mark Review stale. Without an open
    /// turn (e.g. a settled retry) it still refreshes Review.
    pub(super) fn finish_turn(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session_id.clone() else {
            self.turn_open = false;
            self.sidepane
                .update(cx, |pane, cx| pane.mark_review_stale(cx));
            self.git_panel
                .update(cx, |panel, cx| panel.mark_review_stale(cx));
            return;
        };
        if !self.turn_open {
            self.sidepane
                .update(cx, |pane, cx| pane.mark_review_stale(cx));
            self.git_panel
                .update(cx, |panel, cx| panel.mark_review_stale(cx));
            return;
        }
        // Close the turn synchronously so a second settle event can't start a
        // duplicate capture while this one is still running.
        self.turn_open = false;
        let turn = self.turn_count;
        let cwd = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { checkpoint::capture_turn(&cwd, &session, turn) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.turn_open = false;
                if result.is_ok() {
                    app.latest_turn = Some(turn);
                }
                app.sync_review_context(cx);
                app.sidepane
                    .update(cx, |pane, cx| pane.mark_review_stale(cx));
                app.git_panel
                    .update(cx, |panel, cx| panel.mark_review_stale(cx));
                cx.notify();
            });
        })
        .detach();
    }

    /// Forget turn checkpoints when the session or workspace changes.
    pub(super) fn reset_turns(&mut self) {
        self.turn_count = 0;
        self.turn_open = false;
        self.latest_turn = None;
        // The find hits belong to the previous session's transcript.
        self.transcript_search = None;
    }

    /// Forget the queued-message mirror — the queue belongs to the previous
    /// session/process. The next `queue_update`/`get_state` re-establishes it.
    pub(super) fn reset_queue(&mut self) {
        self.queue = PendingQueue::default();
        self.restore_queue_on_clear = false;
    }

    /// Park the active session's process so it stays warm in the background,
    /// whether it is mid-run or idle. Re-opening it resumes the same process
    /// (no Node spawn), so session switching is near-instant. Running parked
    /// sessions keep draining events; idle ones are reaped after
    /// [`PARKED_IDLE_TTL`](crate::app::PARKED_IDLE_TTL).
    ///
    /// A process that has not claimed a session file yet (pi still booting, or
    /// a `new_session` reply in flight) joins the pending pool instead:
    /// leaving it active would let the incoming `adopt_client` overwrite — and
    /// kill — a live run on the next switch (issue #46).
    pub(super) fn park_active_session(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        let busy = self.busy || self.transcript.is_streaming();
        let transcript = std::mem::replace(&mut self.transcript, Transcript::new());
        let widgets = std::mem::take(&mut self.extension_widgets);
        let parked = ParkedSession {
            client,
            transcript,
            busy,
            added: self.added,
            removed: self.removed,
            mcp_stamp: self.mcp_stamp,
            widgets,
            queue: self.queue.clone(),
            parked_at: Instant::now(),
        };
        match self.current_session_path.take() {
            Some(path) => self.park(path, parked),
            None => self.pending_parks.push(parked),
        }
    }

    /// Recover the newest completed turn from the persisted checkpoint refs,
    /// so Review's **Last Turn** is available immediately after a restart or
    /// session switch. Orbit keeps the fact in `refs/orbit/…` and reads it
    /// back here.
    pub(super) fn recover_latest_turn(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        let cwd = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let lookup_session = session.clone();
            let latest = cx
                .background_executor()
                .spawn(async move { checkpoint::latest_turn(&cwd, &lookup_session) })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.session_id.as_deref() != Some(session.as_str()) {
                    return;
                }
                app.latest_turn = latest;
                app.sync_review_context(cx);
                if let Some(latest) = latest {
                    // Continue numbering after the recovered turn so new refs
                    // never clobber the persisted ones.
                    app.turn_count = app.turn_count.max(latest);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn on_submit(
        &mut self,
        _: &crate::Submit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The session-details rename field shares the `Composer` key context;
        // Enter there commits the name instead of reaching the main composer.
        if self
            .session_name_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            self.rename_session(cx);
            return;
        }
        // Shared Composer fields must never submit the main chat draft.
        self.submit_composer_as(theme::get(cx).ui.composer_send_mode, window, cx);
    }

    /// Tab accepts the highlighted autocomplete entry; with the menu closed
    /// it is a no-op (gpui binds no tab navigation in the Composer context).
    pub(super) fn on_autocomplete_accept(
        &mut self,
        _: &crate::AutocompleteAccept,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Only the main composer owns the completion menu — picker filter
        // inputs share the `Composer` key context but must never reach it.
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        // With the menu open, Tab accepts the highlighted entry; with it
        // closed, Tab is the workbench's focus-traversal key and must not be
        // swallowed by this binding.
        if !self.commit_autocomplete_if_open(cx) {
            window.focus_next();
        }
    }

    pub(super) fn on_send_click(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input.read(cx).is_composing() {
            return;
        }
        if self.commit_autocomplete_if_open(cx) {
            return;
        }
        let text = self.input.read(cx).text();
        self.submit(text, cx);
    }

    pub(super) fn on_abort(
        &mut self,
        _: &crate::AbortRun,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Escape backs out of the topmost surface: the image lightbox, the
        // command palette (when focus somehow sits outside it), the
        // autocomplete menu first, then settings, popovers, then a running
        // agent.
        if self.lightbox.take().is_some() {
            cx.notify();
            return;
        }
        if self.transcript_search.is_some() {
            self.close_search(cx);
            return;
        }
        if self.command_palette.take().is_some() {
            self.input.read(cx).focus(window);
            cx.notify();
            return;
        }
        if self.workspace_picker.take().is_some() {
            self.input.read(cx).focus(window);
            cx.notify();
            return;
        }
        // The Worktrees dialog is a modal surface: Escape closes it before
        // anything underneath (menus, the page, a running agent).
        if self.worktree_dialog.is_some() {
            self.cancel_worktree_dialog(cx);
            return;
        }
        // A pending access-guard approval is answered first: escape denies it.
        if self.approval.is_some() {
            self.respond_to_approval(None, window, cx);
            return;
        }
        if self.autocomplete.borrow().open {
            self.autocomplete_dismissed = true;
            cx.notify();
            return;
        }
        if self.add_menu_open {
            self.close_add_menu(window, cx);
            return;
        }
        if self.access_menu_open {
            self.close_access_menu(window, cx);
            return;
        }
        if self.work_in_menu_open {
            self.close_work_in_menu(window, cx);
            return;
        }
        if self.git_open && self.git_panel.read(cx).has_modal() {
            self.git_panel
                .update(cx, |panel, cx| panel.dismiss_modal(cx));
            return;
        }
        // An issue/PR detail or new-form is a page inside the Git page: Escape
        // steps back to its list instead of leaving the whole page (and never
        // reaches the run-abort below).
        if self.git_open && self.git_panel.read(cx).has_transient_view() {
            self.git_panel
                .update(cx, |panel, cx| panel.close_transient_view(cx));
            return;
        }
        if self.git_open {
            self.close_git(cx);
            return;
        }
        if self.worktree_menu.take().is_some() || self.worktree_advanced_open {
            self.worktree_advanced_open = false;
            cx.notify();
            return;
        }
        if self.worktrees_open {
            self.close_worktrees(cx);
            return;
        }
        if self.settings_open {
            // Escape closes an open dropdown first, then leaves settings.
            if self.settings_select.take().is_some() {
                cx.notify();
                return;
            }
            self.settings_open = false;
            cx.notify();
            return;
        }
        if self.model_selector.is_some() {
            self.close_model_selector(window, cx);
            return;
        }
        if self.open_in_menu_open {
            self.open_in_menu_open = false;
            cx.notify();
            return;
        }
        if self.context_popup != ContextPopup::None {
            self.context_popup = ContextPopup::None;
            cx.notify();
            return;
        }
        // The session-details popover can hold keyboard focus (its rename
        // field), so Escape closes it before falling through to aborting a run.
        if self.session_details_open {
            self.session_details_open = false;
            self.input.read(cx).focus(window);
            cx.notify();
            return;
        }
        // The Review pane's source menu is closed by Escape before the key
        // falls through to aborting a run.
        if self.sidepane.read(cx).is_source_menu_open() {
            self.sidepane
                .update(cx, |pane, cx| pane.close_source_menu(cx));
            return;
        }
        // Interactive Esc: drop the pending queue first so its text can be
        // restored to the composer when the `clear_queue` response lands
        // (docs), then abort the run.
        if !self.queue.is_empty() {
            self.restore_queue_on_clear = true;
            self.send(CommandBody::ClearQueue, "clear_queue");
        }
        self.send(CommandBody::Abort, "abort");
        cx.notify();
    }

    pub(super) fn on_abort_mouse(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_abort(&crate::AbortRun, window, cx);
    }

    /// Close every main-area surface and dock so the new-task page owns the
    /// window. New Task is reachable while Review, Git, Files, Usage, the
    /// Explorer, the terminal, or Settings is up; without this the fresh task
    /// opens behind a panel that still covers the chat column.
    pub(super) fn close_surfaces_for_new_task(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.provider_editor = None;
        self.provider_key_editor = None;
        if self.git_open {
            self.close_git(cx);
        }
        if self.usage_open {
            self.close_usage(cx);
        }
        if self.file_viewer.read(cx).is_open() {
            self.close_files(cx);
        }
        self.sidepane.update(cx, |pane, cx| pane.close(cx));
        self.project_panel.update(cx, |panel, cx| panel.close(cx));
        self.terminal_panel.update(cx, |panel, cx| panel.close(cx));
        self.sync_terminal_active(cx);
    }

    pub(super) fn on_new_session(
        &mut self,
        _: &crate::NewSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A run in flight must not be sacrificed to start a new task. pi's
        // `new_session` replaces the session and calls `abort()` on the live
        // turn (`agent-session-runtime.teardownCurrent`), which surfaces as
        // "This operation was aborted". Park the running session instead — its
        // process keeps going in the background — and start the new task on a
        // fresh pi process. An idle session is cheap to reuse in place, but
        // only while its process is alive: a dead one (see issue #30) gets a
        // fresh process too rather than a command into the void.
        if !self.can_reuse_session() {
            let cwd = self
                .current_workspace
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            self.begin_new_task(cwd, window, cx);
            return;
        }
        self.close_surfaces_for_new_task(cx);
        self.send(CommandBody::NewSession, "new_session");
        self.input.read(cx).focus(window);
        cx.notify();
    }

    /// Duplicate the open session as-is (`clone`). pi creates the copy and
    /// switches the process onto it; the `clone` response handler reloads the
    /// transcript and re-keys the session.
    pub(super) fn clone_session(&mut self, cx: &mut Context<Self>) {
        self.send(CommandBody::CloneSession, "clone");
        cx.notify();
    }

    /// Plus on a workspace group: start a fresh session rooted at that cwd.
    pub(super) fn on_new_session_in_workspace(
        &mut self,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Same workspace with a live idle process: reuse it in place. A run
        // in flight is parked instead, so `new_session` never aborts it; a
        // dead process (or a different workspace) always needs its own.
        if self.current_workspace.as_ref() == Some(&cwd) && self.can_reuse_session() {
            self.close_surfaces_for_new_task(cx);
            self.send(CommandBody::NewSession, "new_session");
            self.input.read(cx).focus(window);
            cx.notify();
            return;
        }
        self.begin_new_task(cwd, window, cx);
    }

    /// Start a fresh task rooted at `cwd` on its own pi process, parking a
    /// live active session first so a running one keeps going in the
    /// background (a dead one is dropped). Shared by New Task and a
    /// workspace group's "+".
    pub(super) fn begin_new_task(
        &mut self,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Leaving this session: cancel any open blocking dialog first, so a
        // parked run never waits on a modal tied to the previous session.
        self.cancel_open_dialog(cx);
        // A new task owns the window: drop whatever surface was covering the
        // chat column before the fresh session paints.
        self.close_surfaces_for_new_task(cx);
        // A live session is parked so a run in flight keeps going in the
        // background. A dead one holds nothing worth keeping: drop it (its
        // exit banner goes with it) instead of parking a corpse.
        if self.runtime_is_live() {
            self.park_active_session();
        } else {
            self.drop_client();
        }

        self.busy = false;
        self.transcript.clear();
        self.current_title = None;
        self.reset_session_name(cx);
        self.current_session_path = None;
        self.added = 0;
        self.removed = 0;
        self.context = None;
        self.reset_turns();
        self.reset_queue();
        // A brand-new task starts unscoped (Build) and waits for its own id:
        // clearing the id keeps a mode chosen on the New Task page pending
        // instead of writing it to the session we just parked.
        self.session_id = None;
        self.workflow_pending = None;
        self.workflow_mode = WorkflowMode::default();
        self.add_workspace(cwd.clone(), cx);
        self.set_current_workspace(cwd.clone(), cx);

        match self.extensions.spawn(&cwd, true, &self.mcp.secret_env()) {
            Ok(client) => {
                self.adopt_client(client);
                self.mcp_stamp = self.mcp.fingerprint_for(Some(&cwd));
                self.send(CommandBody::NewSession, "new_session");
                self.refresh_catalogs();
                // Capability probes queue after the new-session request.
                self.probe_auth();
                self.set_status(tr!("session.new_session"));
            }
            Err(err) => {
                let message = tr!("runtime.pi_spawn_failed", error = err);
                self.client = None;
                self.runtime.error = Some(message.clone());
                self.set_status(message);
            }
        }
        self.input.read(cx).focus(window);
        cx.notify();
    }

    /// Open the OS folder picker and start (or restart) the task in the
    /// selected directory. Used from the new-task page and the status bar.
    pub(super) fn on_pick_folder_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browse_for_folder(window, cx);
    }

    /// The native folder dialog — the workspace picker's "Choose folder…" row
    /// and the status-bar chip both land here.
    ///
    /// The panel is opened *asynchronously*: GPUI holds a mutable borrow of this
    /// entity for the duration of the event handler, and the blocking
    /// `rfd::FileDialog::pick_folder` runs a nested main-thread modal loop that
    /// re-enters the app. A task queued on that loop that touches this same
    /// entity then panics with `already borrowed`. The async prompt returns
    /// immediately (a sheet, not a nested `runModal`) and resolves once the
    /// user answers, so the borrow is long gone.
    pub(super) fn browse_for_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let receiver = cx.update(|_window, app| {
                app.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some(tr!("session.choose_folder").into()),
                })
            });
            let Ok(receiver) = receiver else {
                return;
            };
            let folder = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(folder) = folder else {
                return;
            };
            let _ = this.update_in(cx, |app, window, cx| {
                app.start_task_in_folder(folder, window, cx);
            });
        })
        .detach();
    }

    /// Switch the live session. With `push`, the visit is recorded in the
    /// top-bar history (forward entries are dropped, like browser history).
    ///
    /// Each session gets its own pi process, so switching never interrupts a
    /// run: the outgoing session is *parked* mid-run (its process and live
    /// transcript keep going in the background — events drain every tick),
    /// and a parked target resumes exactly where it left off. Idle sessions
    /// are torn down and reload from disk when reopened.
    pub(super) fn switch_to_session(
        &mut self,
        session: SessionInfo,
        push: bool,
        cx: &mut Context<Self>,
    ) {
        // Opening a session means viewing the chat: leave whichever full-page
        // surface (GitHub / Usage) was covering the main area. Without this
        // the switch happens out of sight and the page stays up, so clicking
        // a session reads as a no-op.
        if self.git_open {
            self.close_git(cx);
        }
        if self.usage_open {
            self.close_usage(cx);
        }
        if self.current_session_path.as_ref() == Some(&session.path) {
            return;
        }
        // A blocking dialog belongs to the session being left; cancel it so
        // the now-parked run can settle instead of waiting on an unseen modal.
        self.cancel_open_dialog(cx);
        // ── park the outgoing session (running or idle) ──
        self.park_active_session();
        self.busy = false;
        self.added = 0;
        self.removed = 0;
        self.transcript = Transcript::new();
        // The queue belongs to the session we just left; the target's state
        // re-establishes it from `get_state`/`queue_update`.
        self.reset_queue();

        // ── activate the target ──
        // A parked process spawned before the latest MCP configuration change
        // is stale: drop it and cold-start so it loads the current servers.
        // The stamp is workspace-specific, so compare against the *target
        // session's* workspace — the manager is still pointed at the workspace
        // being left. Using the current fingerprint made every cross-workspace
        // switch look like a config change and drop (kill) the parked run.
        // A busy session is never dropped for staleness: the live run wins,
        // and the new config applies on its next idle reopen.
        let mcp_stamp = self.mcp.fingerprint_for(Some(&session.cwd));
        if let Some(parked) = self
            .lives
            .remove(&session.path)
            .filter(|parked| parked.busy || parked.mcp_stamp == mcp_stamp)
        {
            // Resume a background run. The parked transcript is already up
            // to date (its events drain every tick); anything buffered in
            // the process channel streams in from the next tick on.
            self.adopt_client(parked.client);
            self.transcript = parked.transcript;
            self.busy = parked.busy;
            self.added = parked.added;
            self.removed = parked.removed;
            self.extension_widgets = parked.widgets;
            // pi re-emits `queue_update` only on a change, so a warm resume
            // restores the pending bar from the drained mirror instead.
            self.queue = parked.queue;
            self.send(CommandBody::GetState, "get_state");
            self.refresh_context_stats();
            // A warm process already reported capabilities; a cheap refresh
            // keeps a long-parked session's auth/quota current.
            self.probe_auth();
        } else {
            // Cold session (its process was torn down): paint the stored
            // transcript from disk in the same frame so the switch never waits
            // on pi boot. `get_messages` supersedes it moments later.
            self.extension_widgets.clear();
            self.preview_session_transcript(session.path.clone(), cx);
            if !session.cwd.is_dir() {
                // The session's workspace is gone (a removed worktree, a
                // deleted folder). Never spawn the agent in a fallback
                // directory and never re-point the session silently (§22) —
                // show the unavailable notice instead.
                self.client = None;
                self.worktree_dialog = Some(WorktreeDialog::Unavailable {
                    path: session.cwd.clone(),
                });
            } else {
                // Spawn a dedicated pi process rooted at the session's
                // workspace and point it at the session file.
                let secret_env = self.mcp.secret_env();
                let spawned = self
                    .extensions
                    .spawn(&session.cwd, false, &secret_env)
                    .or_else(|_| {
                        self.extensions.spawn(
                            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                            false,
                            &secret_env,
                        )
                    });
                match spawned {
                    Ok(client) => {
                        self.adopt_client(client);
                        self.mcp_stamp = mcp_stamp;
                        self.send(
                            CommandBody::SwitchSession {
                                session_path: session.path.to_string_lossy().into_owned(),
                            },
                            "switch_session",
                        );
                        // `get_state` and the capability probes are sent from
                        // the switch_session success handler: querying state
                        // eagerly here races the switch (pi answers with the
                        // default model), and the probes only get queued after
                        // the load commands, so a slow auth/quota lookup can't
                        // delay the transcript.
                    }
                    Err(err) => {
                        let message = tr!("runtime.pi_spawn_failed", error = err);
                        self.client = None;
                        self.runtime.error = Some(message.clone());
                        self.set_status(message);
                    }
                }
            }
        }
        self.current_title = Some(session.title.clone());
        self.reset_session_name(cx);
        self.seed_session_name_input(cx);
        // Opening a session keeps its folder in Orbit's own sidebar list.
        self.add_workspace(session.cwd.clone(), cx);
        self.set_current_workspace(session.cwd.clone(), cx);
        self.current_session_path = Some(session.path.clone());
        if push {
            self.session_history.truncate(self.history_index + 1);
            let new_entry = self
                .session_history
                .last()
                .map(|last| last.path != session.path)
                .unwrap_or(true);
            if new_entry {
                self.session_history.push(session);
            }
            self.history_index = self.session_history.len().saturating_sub(1);
        }
        cx.notify();
    }

    /// Render a session's stored messages straight from disk, off the UI
    /// thread, so reopening a cold session shows content immediately instead
    /// of waiting for pi to boot and answer `switch_session` → `get_messages`.
    /// The authoritative snapshot replaces this when it lands; a preview that
    /// is already superseded (snapshot arrived, or the user switched again) is
    /// dropped.
    pub(super) fn preview_session_transcript(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let payload = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { sessions::read_messages_payload(&path) }
                })
                .await;
            let Some(payload) = payload else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                let still_active = app.current_session_path.as_ref() == Some(&path);
                if !still_active || !app.transcript.is_empty() {
                    return;
                }
                app.apply_messages_snapshot(&payload);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn on_open_session(&mut self, session: SessionInfo, cx: &mut Context<Self>) {
        self.switch_to_session(session, true, cx);
    }

    pub(super) fn on_history_back(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history_index > 0 {
            self.history_index -= 1;
            if let Some(session) = self.session_history.get(self.history_index).cloned() {
                self.switch_to_session(session, false, cx);
                return;
            }
        }
        cx.notify();
    }

    pub(super) fn on_history_forward(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history_index + 1 < self.session_history.len() {
            self.history_index += 1;
            if let Some(session) = self.session_history.get(self.history_index).cloned() {
                self.switch_to_session(session, false, cx);
                return;
            }
        }
        cx.notify();
    }

    /// Open the top-bar session-details popover. The chip that used to own
    /// this toggle is now a row in the overflow menu, so the open is a plain
    /// action; the popover still dismisses on an outside click.
    pub(super) fn open_session_details(&mut self, cx: &mut Context<Self>) {
        self.session_details_open = true;
        // Only one top-bar popover is meaningful at a time.
        self.header_more_open = false;
        self.quota_popup_open = false;
        // Seed the rename field from the live title so the input isn't empty
        // when pi auto-titled via `session_info_changed` and hasn't echoed
        // `sessionName` yet.
        self.seed_session_name_input(cx);
        cx.notify();
    }

    /// Flip the top bar's overflow menu. Mirrors the old info chip's guard so
    /// the click that closes the menu cannot reopen it on mouse-up.
    pub(super) fn on_header_more_click(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.header_more_open = !self.header_more_open;
        if self.header_more_open {
            // Only one top-bar popover is meaningful at a time.
            self.quota_popup_open = false;
            self.session_details_open = false;
            self.open_in_menu_open = false;
        }
        cx.notify();
    }

    /// Dismiss the top bar's overflow menu from a click outside it.
    pub(super) fn dismiss_header_more(&mut self, cx: &mut Context<Self>) {
        if self.header_more_open {
            self.menu_dismissed_at = Some(Instant::now());
            self.header_more_open = false;
            cx.notify();
        }
    }

    /// The rename row's **Generate title** control: a magic-wand button
    /// beside the name field that asks the bundled title extension to name
    /// the session from its conversation. It swaps to a spinner and goes
    /// inert while the command is in flight, so the wait reads as work rather
    /// than a dropped click. Hidden when the running pi did not expose the
    /// extension's command, so a stray `/generate-title` can never be sent as
    /// an ordinary user prompt.
    fn generate_title_button(&self, theme: Theme, this: Entity<OrbitApp>) -> Option<AnyElement> {
        if !self.can_generate_title() {
            return None;
        }
        let generating = self.title_generating;
        let mut button =
            icon_button_frame(div().id("sess-generate-title"), &theme, ButtonSize::Large)
                .group(BUTTON_GROUP)
                .border_1()
                .border_color(theme.border)
                .when(generating, |button| button.bg(theme.overlay))
                .when(!generating, |button| {
                    button.raised(theme.surface_elevated, &theme)
                });
        if !generating {
            button = button
                .cursor_pointer()
                .hover(|s| {
                    s.raised(theme.surface_hover, &theme)
                        .border_color(theme.border_strong)
                })
                .active(|s| s.opacity(PRESS_DIM))
                .tooltip({
                    let label = tr!("session.generate_title");
                    move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                })
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    this.update(cx, |app, cx| app.generate_session_title(cx));
                });
        }
        Some(
            button
                .child(if generating {
                    crate::app::spinner(
                        "sess-generate-title-spinner",
                        ButtonSize::Large.icon_size().px(&theme),
                        theme.accent_text,
                        theme,
                    )
                } else {
                    icon(
                        "icons/magic-wand.svg",
                        ButtonSize::Large.icon_size().px(&theme),
                        theme.text_secondary,
                    )
                    .into_any_element()
                })
                .into_any_element(),
        )
    }

    /// Whether the bundled title extension registered its command with pi.
    fn can_generate_title(&self) -> bool {
        self.slash_commands
            .iter()
            .any(|command| command.name == "generate-title")
    }

    /// Ask the bundled title extension to (re)name the open session. The
    /// message is an extension command, so pi runs it without adding a turn
    /// or transcript entry; its `setSessionName` comes back as
    /// `session_info_changed` (handled in `events.rs`).
    pub(super) fn generate_session_title(&mut self, cx: &mut Context<Self>) {
        if self.session_id.is_none() || !self.can_generate_title() {
            return;
        }
        self.title_generating = true;
        if !self.send(
            CommandBody::Prompt {
                message: "/generate-title".into(),
                images: None,
                streaming_behavior: None,
            },
            "generate_title",
        ) {
            self.title_generating = false;
        }
        cx.notify();
    }

    /// Open the Git card (Changes / History / Graph) and load it.
    pub(super) fn open_git(&mut self, cx: &mut Context<Self>) {
        self.git_open = true;
        // One main-area feature at a time.
        self.usage_open = false;
        self.worktrees_open = false;
        self.session_details_open = false;
        // Files is another main-area feature; leave it for the Git card.
        self.close_files(cx);
        // Review owns the right dock and would otherwise show beside the Git
        // page; opening Git replaces it. Opening Review closes Git in turn
        // (see `close_git_for_review`).
        self.sidepane.update(cx, |pane, cx| pane.close(cx));
        self.git_panel.update(cx, |panel, cx| panel.show(cx));
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// ⌘⇧G: open the Git page, or leave it when it is already open. The
    /// top-bar affordance lives in the overflow menu; this keeps the GitHub
    /// surface one chord away.
    pub(super) fn on_open_git(
        &mut self,
        _: &crate::OpenGit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_open {
            self.close_git(cx);
        } else {
            self.open_git(cx);
        }
    }

    /// Close the Git page and return to the chat.
    pub(super) fn close_git(&mut self, cx: &mut Context<Self>) {
        self.git_open = false;
        self.git_panel.update(cx, |panel, cx| panel.hide(cx));
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// Leave the Git page when the Review dock takes the window. The Git page
    /// is a main-area surface and Review owns the right dock, so leaving both
    /// mounted shows them side by side; opening either replaces the other.
    pub(super) fn close_git_for_review(&mut self, cx: &mut Context<Self>) {
        if self.git_open {
            self.git_open = false;
            self.git_panel.update(cx, |panel, cx| panel.hide(cx));
            self.sync_terminal_active(cx);
            cx.notify();
        }
    }

    /// Open Review (⌘⇧R and the palette), replacing the Git page.
    pub(super) fn open_review(&mut self, cx: &mut Context<Self>) {
        self.close_git_for_review(cx);
        self.sidepane.update(cx, |pane, cx| pane.show_review(cx));
    }

    /// Open the Usage card and let it load (or refresh) the session store.
    pub(super) fn open_usage(&mut self, cx: &mut Context<Self>) {
        self.usage_open = true;
        self.git_open = false;
        self.worktrees_open = false;
        self.session_details_open = false;
        // Files is another main-area feature; leave it for the Usage card.
        self.close_files(cx);
        self.usage.update(cx, |page, cx| page.open(cx));
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// Leave the Usage page.
    pub(super) fn close_usage(&mut self, cx: &mut Context<Self>) {
        self.usage_open = false;
        self.usage.update(cx, |page, cx| page.close(cx));
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// Sidebar nav row: Usage is a destination, toggled like Settings.
    pub(super) fn on_usage_nav_click(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.usage_open {
            self.close_usage(cx);
        } else {
            self.open_usage(cx);
        }
    }

    /// Open a session the Usage page named, by pi's session id (§29). The id
    /// is resolved against the loaded session list; a session the sidebar has
    /// not picked up yet forces one reload before giving up.
    pub(super) fn open_session_by_id(&mut self, id: &str, _: &mut Window, cx: &mut Context<Self>) {
        let mut target = self.sessions.iter().find(|s| s.id == id).cloned();
        if target.is_none() {
            self.sessions = sessions::load_sessions();
            target = self.sessions.iter().find(|s| s.id == id).cloned();
        }
        if let Some(session) = target {
            self.close_usage(cx);
            self.switch_to_session(session, true, cx);
            cx.notify();
        }
    }

    /// Open the session a clicked notification announced, by session-file
    /// path. The banner may outlive its session (deleted, or the list is
    /// stale); a missing target is a no-op rather than an error.
    pub(super) fn activate_session_from_notification(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.activate_window_pending = true;
        let mut target = self.sessions.iter().find(|s| s.path == path).cloned();
        if target.is_none() {
            self.sessions = sessions::load_sessions();
            target = self.sessions.iter().find(|s| s.path == path).cloned();
        }
        if let Some(session) = target {
            self.switch_to_session(session, true, cx);
        }
        cx.notify();
    }

    /// Forget a previous session's display name so it cannot leak into a
    /// new or switched session before `get_state` arrives.
    pub(super) fn reset_session_name(&mut self, cx: &mut Context<Self>) {
        self.session_name = None;
        self.title_generating = false;
        self.session_name_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
    }

    /// Fill the rename field from the live title. Used when the popover
    /// opens, and when switching sessions, so the input matches the header
    /// instead of a stale (or empty) `sessionName`.
    pub(super) fn seed_session_name_input(&mut self, cx: &mut Context<Self>) {
        let text = self
            .session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                self.current_title
                    .as_deref()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        if self.session_name_input.read(cx).text() != text {
            self.session_name_input
                .update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    /// The command palette's Rename Session: open the details popover (the
    /// header's info control owns the rename field) and put the caret in it.
    pub(super) fn begin_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quota_popup_open = false;
        self.session_details_open = true;
        self.seed_session_name_input(cx);
        let handle = self.session_name_input.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    /// The Update button's success glyph: a green check that scales and fades
    /// in when a rename commits. Reduce motion renders it statically.
    fn rename_check(theme: Theme, cx: &App) -> AnyElement {
        let svg = gpui::svg()
            .path("icons/check.svg")
            .flex_none()
            .size(IconSize::XSmall.px(&theme))
            .text_color(theme.action_fg);
        if theme::reduce_motion(cx) {
            return svg.into_any_element();
        }
        svg.with_animation(
            "sess-rename-check",
            Animation::new(Duration::from_millis(200)).with_easing(|d| 1.0 - (1.0 - d).powi(3)),
            |svg, d| {
                let scale = 0.5 + 0.5 * d;
                svg.opacity(d)
                    .with_transformation(Transformation::scale(gpui::size(scale, scale)))
            },
        )
        .into_any_element()
    }

    /// The top-bar info popover: active session's environment + identifiers.
    pub(super) fn render_session_details_popup(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.session_details_open {
            return None;
        }
        let theme = *theme::get(cx);
        let this = cx.entity();
        let session_id = self.session_id.clone().unwrap_or_default();
        let session_file = self
            .current_session_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let workspace = self
            .current_workspace
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let model = self.model_label.clone();
        let thinking = self.thinking_label.clone();
        // Set on a successful commit and cleared by its timer; drives the
        // button's brief success check.
        let rename_saved = self.rename_saved_at.is_some();

        // Header names the surface. The session's own title already lives in
        // the rename field below, so repeating it here only crowded the card;
        // the only line worth keeping is the no-session hint.
        let header = div()
            .px(DynamicSpacing::Base12.px(&theme))
            .py(DynamicSpacing::Base08.px(&theme))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base02.px(&theme))
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("session.session_details")),
            )
            .children(session_id.is_empty().then(|| {
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!("session.no_active"))
            }));

        // pi owns the name; Enter or Update commits it (`set_session_name`).
        let name_block = (!session_id.is_empty()).then(|| {
            div()
                .px(DynamicSpacing::Base12.px(&theme))
                .py(DynamicSpacing::Base08.px(&theme))
                .border_b_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .gap(input::gap(&theme))
                .child(input_label(tr!("session.name"), &theme).text_color(theme.text_muted))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .child(
                            input_field_frame(div(), &theme)
                                .flex_1()
                                .min_w_0()
                                .bg(theme.background)
                                .overflow_hidden()
                                .child(self.session_name_input.clone()),
                        )
                        .children(self.generate_title_button(theme, this.clone()))
                        .child({
                            let mut button =
                                button_frame(div().id("sess-rename"), &theme, ButtonSize::Large)
                                    .border_1()
                                    .cursor_pointer()
                                    .font_weight(FontWeight::MEDIUM)
                                    .on_mouse_up(MouseButton::Left, {
                                        let this = this.clone();
                                        move |_, _, cx| {
                                            this.update(cx, |app, cx| app.rename_session(cx));
                                        }
                                    });
                            if rename_saved {
                                // Commit succeeded: the label becomes a check
                                // that pops in, so the button confirms the
                                // rename instead of silently doing nothing.
                                button = button
                                    .raised(theme.success, &theme)
                                    .border_color(theme.success)
                                    .child(Self::rename_check(theme, cx));
                            } else {
                                button = button
                                    .raised(theme.surface_elevated, &theme)
                                    .border_color(theme.border)
                                    .text_color(theme.text_primary)
                                    .hover(|s| s.raised(theme.surface_hover, &theme))
                                    .child(tr!("session.update"));
                            }
                            button
                        }),
                )
        });

        let section_label = |label: &str| {
            div()
                .px(DynamicSpacing::Base12.px(&theme))
                .pt(DynamicSpacing::Base08.px(&theme))
                .pb(DynamicSpacing::Base04.px(&theme))
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(label.to_string())
        };

        let popup = div()
            .id("session-details-popup")
            .w(px(320.))
            .font_family(theme::ui_font_family())
            .elevation_2(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            // Clicks inside the card (the rename field, Update, copy) must
            // not bubble to the info button that toggles this popover.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out({
                let this = this.clone();
                move |_: &MouseDownEvent, _, cx: &mut App| {
                    this.update(cx, |app, cx| {
                        if app.session_details_open {
                            // Arm the click-through guard so this same click's
                            // mouse-up on the info button cannot reopen the
                            // popover it just dismissed.
                            app.menu_dismissed_at = Some(Instant::now());
                            app.session_details_open = false;
                            cx.notify();
                        }
                    });
                }
            })
            .child(header)
            .children(name_block)
            .child(section_label(&tr!("session.environment")))
            .child(
                div()
                    .id(ElementId::Name("sess-commit-push".into()))
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .on_click({
                        let this = this.clone();
                        move |_, _window, cx| {
                            this.update(cx, |app, cx| {
                                app.open_git(cx);
                            });
                        }
                    })
                    .child(icon(
                        "icons/branch.svg",
                        IconSize::Small.px(&theme),
                        theme.text_secondary,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_primary)
                            .child(tr!("session.commit_or_push")),
                    )
                    .child(icon(
                        "icons/chevron-right.svg",
                        IconSize::XSmall.px(&theme),
                        theme.text_muted,
                    )),
            )
            .child(
                div()
                    .id(ElementId::Name("sess-compare-branch".into()))
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| {
                            this.update(cx, |app, cx| {
                                app.toggle_branch_picker(window, cx);
                            });
                        }
                    })
                    .child(icon(
                        "icons/file-diff.svg",
                        IconSize::Small.px(&theme),
                        theme.text_secondary,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_primary)
                            .child(tr!("session.compare_branch")),
                    )
                    .child(icon(
                        "icons/chevron-right.svg",
                        IconSize::XSmall.px(&theme),
                        theme.text_muted,
                    )),
            )
            .child(
                div()
                    .mt(DynamicSpacing::Base04.px(&theme))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(section_label(&tr!("session.details"))),
            )
            .child(self.session_detail_row(0, &tr!("session.detail_id"), &session_id, theme))
            .child(self.session_detail_row(1, &tr!("session.detail_file"), &session_file, theme))
            .child(self.session_detail_row(2, &tr!("session.detail_workspace"), &workspace, theme))
            .child(self.session_detail_row(3, &tr!("session.detail_model"), &model, theme))
            .child(self.session_detail_row(4, &tr!("session.detail_thinking"), &thinking, theme))
            .child(div().h(px(6.)));

        Some(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .size(px(0.))
                .child(
                    anchored()
                        .position_mode(AnchoredPositionMode::Local)
                        .anchor(Corner::TopRight)
                        .offset(point(px(0.), popover::MENU_OFFSET))
                        .snap_to_window_with_margin(popover::WINDOW_MARGIN)
                        .child(deferred(popup)),
                )
                .into_any_element(),
        )
    }

    /// The top-bar quota pill: the rolling 5-hour limit of the provider the
    /// active model is using — provider mark, window label, percentage, and
    /// a context-style ring gauge — falling back to that provider's
    /// most-constrained window or balance, then to the account that will run
    /// out first. It stays provider-independent: everything comes from the
    /// normalized [`QuotaReport`] list, so a new adapter needs no UI change.
    /// `None` (hidden) when pi lacks `quota.*` or nothing has been reported.
    pub(super) fn render_quota_pill(
        &self,
        compact: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        // The pill is the only way into the usage popover, and the popover now
        // carries the MCP status pane too. Render it when either side has
        // something to show, so an MCP-only setup is not left without the
        // control. `None` (hidden) when pi lacks `quota.*` and no MCP server
        // is configured.
        let mcp_chip = self.mcp_chip_parts(*theme::get(cx));
        if self.quota.reports().is_empty() && mcp_chip.is_none() {
            return None;
        }
        let theme = *theme::get(cx);

        // A persistent glass chip rather than an invisible hit target: the
        // hairline border and ink wash read as a dedicated meter surface at
        // rest, and the hover/open state lifts it (deeper fill, stronger
        // border) instead of the chip appearing from nothing. It shares the
        // top bar's chip glass, so the meter sits in the same row as the
        // buttons without looking like one.
        let mut pill = press(header_chip(
            div()
                .id("top-quota")
                .group(BUTTON_GROUP)
                .relative()
                .h(px(HEADER_CTRL_H))
                // A Medium button's padding; the left keeps its extra 2px.
                .pl(ButtonSize::Medium.padding_x(&theme) + DynamicSpacing::Base02.px(&theme))
                .pr(ButtonSize::Medium.padding_x(&theme))
                .rounded_full()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .cursor_pointer(),
            &theme,
        ))
        .when(self.quota_popup_open, |s| header_lift(s, &theme))
        .on_mouse_up(MouseButton::Left, cx.listener(Self::on_quota_click))
        .children(self.render_quota_popup(cx));

        // Hairline between the account identity and the meter block: the
        // two halves of the chip answer different questions (whose usage /
        // how much is left) and deserve a visible seam.
        let divider = || div().flex_none().w(px(1.)).h(px(13.)).bg(theme.border);

        // The provider mark and name anchor every headline: the meter is
        // only truthful if the account it belongs to is named beside it.
        let provider_head = |report: &QuotaReport| {
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .child(icon_dyn(
                    provider_icon(&report.provider),
                    IconSize::XSmall.px(&theme),
                    provider_mark_color(&report.provider, &theme, theme.text_muted),
                ))
                .child(
                    div()
                        .max_w(px(96.))
                        .truncate()
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_secondary)
                        .child(providers::provider_display_name(&report.provider)),
                )
        };

        match self.quota.headline(&self.model_provider) {
            QuotaHeadline::Window { report, window } => {
                pill = pill.child(provider_head(report)).child(divider());
                // Drop the window label when the title bar is tight so the
                // session name still has room to truncate instead of colliding.
                if !compact {
                    pill = pill.child(
                        div()
                            .max_w(px(72.))
                            .truncate()
                            .text_size(TextSize::XSmall.px(&theme))
                            .text_color(theme.text_muted)
                            .child(if is_five_hour_window(window) {
                                "5h".to_string()
                            } else {
                                window.label.clone()
                            }),
                    );
                }
                if let Some(fraction) = window.fraction() {
                    // Tabular figures keep the number from jittering as
                    // usage ticks during a run, and the state tint (green →
                    // amber → red) makes the ring's verdict readable even
                    // without the gauge — the same pairing the composer's
                    // context meter uses.
                    let tint = quota_tint(fraction, theme);
                    pill = pill
                        .child(
                            div()
                                .font(crate::usage::view::num_font())
                                .text_size(TextSize::Small.px(&theme))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(tint)
                                .child(format!("{}%", (fraction * 100.0).round() as i32)),
                        )
                        .child(context_ring(fraction, tint, theme.surface_elevated));
                }
            }
            // A balance-only account (DeepSeek, OpenRouter) has no window to
            // meter; the amount is the whole story.
            QuotaHeadline::Balance { report, balance } => {
                let text = if balance.currency.is_empty() {
                    quota_amount(balance.amount)
                } else {
                    format!("{} {}", quota_amount(balance.amount), balance.currency)
                };
                pill = pill.child(provider_head(report)).child(divider()).child(
                    div()
                        .font(crate::usage::view::num_font())
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(text),
                );
            }
            // Nothing metered anywhere (notes, errors): the pill keeps the
            // popover reachable. With MCP servers configured it becomes the
            // MCP status chip — the pane behind it is now the only place
            // their state is visible from the top bar.
            QuotaHeadline::Quiet => match mcp_chip {
                Some((dot, label)) => {
                    pill = pill
                        .child(icon(
                            "icons/tools/mcp.svg",
                            IconSize::Small.px(&theme),
                            theme.text_secondary,
                        ))
                        .child(div().size(px(6.)).rounded_full().bg(dot))
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_secondary)
                                .child(label),
                        );
                }
                None => {
                    pill = pill
                        .child(icon(
                            "icons/spark.svg",
                            IconSize::Small.px(&theme),
                            theme.text_secondary,
                        ))
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_secondary)
                                .child(tr!("session.usage")),
                        );
                }
            },
        }

        Some(pill.into_any_element())
    }

    /// The quota popover: every connected provider's windows and balances,
    /// grouped by provider and read from the normalized model. Rendered only
    /// while open; anchored under the pill.
    pub(super) fn render_quota_popup(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.quota_popup_open {
            return None;
        }
        let theme = *theme::get(cx);
        let this = cx.entity();

        let reports = self.quota.reports();
        // The user's hide list only shapes the popover; Settings still shows
        // every connected provider. Split once so the header count and the
        // "Hidden" group agree, and so a hidden provider is one click back.
        let hidden = &self.hidden_quota_providers;
        let (visible, hidden_reports): (Vec<&QuotaReport>, Vec<&QuotaReport>) = reports
            .into_iter()
            .partition(|report| !hidden.contains(&report.provider));
        let count = visible.len();
        // One glyph that turns in place instead of swapping to a loader, so
        // the click never changes the control's shape. Active, it wears the
        // accent; idle, the shared hover group lifts its ink. Reduce Motion
        // keeps the spin off, but the accent still marks the active state.
        let refresh_icon = refresh_glyph(
            "quota-refresh-spin",
            ButtonSize::Medium.icon_size().px(&theme),
            self.quota_refreshing,
            theme.text_secondary,
            theme,
        );
        let refresh_button =
            icon_button_frame(div().id("quota-refresh"), &theme, ButtonSize::Medium)
                .group(BUTTON_GROUP)
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover))
                .active(|style| style.bg(theme.surface_active))
                .tooltip({
                    let label = tr!("common.refresh");
                    move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                })
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_quota_refresh))
                .child(refresh_icon);

        // Two panes in one popover: the original provider quotas and the MCP
        // server list. A segmented control keeps the switch in the header
        // without adding a row (the body cap math below depends on this
        // header's height).
        let tab = self.quota_popup_tab;
        let mut tabs = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base02.px(&theme));
        for (index, candidate) in QuotaPopupTab::ALL.into_iter().enumerate() {
            let selected = tab == candidate;
            tabs = tabs.child(
                segmented_segment(
                    div()
                        .id(ElementId::Name(
                            format!("quota-tab-{}", candidate.as_str()).into(),
                        ))
                        .debug_selector({
                            let tab = candidate.as_str().to_string();
                            move || format!("quota-tab-chip-{tab}")
                        }),
                    &theme,
                    ButtonSize::Default,
                    SegmentPosition::at(index, QuotaPopupTab::ALL.len()),
                )
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .border_color(theme.border)
                .when(selected, |chip| {
                    chip.raised_flat(theme.surface_active)
                        .text_color(theme.text_primary)
                })
                .when(!selected, |chip| {
                    chip.raised_flat(theme.surface_elevated)
                        .text_color(theme.text_secondary)
                        .hover(|style| style.raised_flat(theme.surface_hover))
                })
                .on_mouse_up(MouseButton::Left, {
                    let this = this.clone();
                    move |_, _, cx| {
                        this.update(cx, |app, cx| app.mcp_popup_tab(candidate, cx));
                    }
                })
                .child(tr!(candidate.label_key())),
            );
        }

        let header = div()
            .flex_none()
            .px(DynamicSpacing::Base12.px(&theme))
            .py(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(tabs)
            .child(div().flex_1())
            .child(match tab {
                QuotaPopupTab::Providers => div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("session.provider_count", count = count)),
                    )
                    .child(refresh_button)
                    .into_any_element(),
                QuotaPopupTab::Mcp => self.mcp_popup_summary(theme, this.clone(), cx),
            });

        // One card per provider, 8 px apart: the boundary between accounts
        // is what tells a glance which numbers belong together. The list is
        // the scroll region, content-sized up to the space the popover cap
        // leaves under the header. It must not lean on `flex_1`: inside the
        // deferred, anchored popover the available height is zero, where a
        // flexible child collapses and the cards get clipped instead (the
        // same reason the model picker's list carries its own `max_h`).
        // Ordinary children keep it independent of measured-layout sizing.
        let header_h = ButtonSize::Medium.height(&theme) + px(21.);
        let body_max_h = px(QUOTA_POPOVER_MAX_H) - header_h;
        let body: AnyElement = match tab {
            QuotaPopupTab::Providers => {
                let mut cards = div()
                    .id("quota-popup-body")
                    .debug_selector(|| "quota-popup-body".to_string())
                    .w_full()
                    .max_h(body_max_h)
                    .overflow_y_scroll()
                    .p(DynamicSpacing::Base08.px(&theme))
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .children(visible.iter().map(|report| {
                        quota_provider_card(
                            self,
                            report,
                            theme,
                            false,
                            Some(self.quota_visibility_button(&report.provider, false, cx)),
                        )
                    }));
                if !hidden_reports.is_empty() {
                    cards = cards.child(self.quota_hidden_section(&hidden_reports, theme, cx));
                }
                if visible.is_empty() && hidden_reports.is_empty() {
                    cards = cards.child(self.quota_popup_empty(theme, this.clone()));
                }
                cards.into_any_element()
            }
            QuotaPopupTab::Mcp => self.mcp_popup_panel(theme, body_max_h, this.clone(), cx),
        };

        let popup = div()
            .debug_selector(|| "quota-popup".to_string())
            .w(px(QUOTA_POPOVER_W))
            .max_h(px(QUOTA_POPOVER_MAX_H))
            .font_family(theme::ui_font_family())
            .elevation_2(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            // Clicks inside the card (the refresh button) must not bubble to
            // the pill that toggles this popover.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out({
                let this = this.clone();
                move |_: &MouseDownEvent, _, cx: &mut App| {
                    this.update(cx, |app, cx| {
                        if app.quota_popup_open {
                            // Arm the click-through guard so this same
                            // click's mouse-up on the pill cannot
                            // immediately re-open the popover.
                            app.menu_dismissed_at = Some(Instant::now());
                            app.quota_popup_open = false;
                            cx.notify();
                        }
                    });
                }
            })
            .child(header)
            .child(body);

        Some(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .size(px(0.))
                .child(
                    anchored()
                        .position_mode(AnchoredPositionMode::Local)
                        .anchor(Corner::TopRight)
                        .offset(point(px(0.), popover::MENU_OFFSET))
                        .snap_to_window_with_margin(popover::WINDOW_MARGIN)
                        .child(deferred(popup)),
                )
                .into_any_element(),
        )
    }

    /// The eye toggle on a provider card in the top-bar quota popover: hides
    /// the provider from the list, or restores one from the "Hidden" group.
    /// A Compact glyph so it sits inside a card header without inflating the
    /// row. The element id varies with the state so the visible and hidden
    /// forms never collide.
    fn quota_visibility_button(
        &self,
        provider: &str,
        hidden: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = *theme::get(cx);
        let (path, label) = if hidden {
            ("icons/eye.svg", tr!("session.show_provider"))
        } else {
            ("icons/eye-off.svg", tr!("session.hide_provider"))
        };
        let id = SharedString::from(format!(
            "quota-{}-{provider}",
            if hidden { "show" } else { "hide" }
        ));
        let provider = provider.to_string();
        icon_button_frame(div().id(id), &theme, ButtonSize::Compact)
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_hover))
            .active(|style| style.opacity(PRESS_DIM))
            .tooltip({
                let label = label.clone();
                move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(
                    move |this: &mut Self,
                          _: &MouseUpEvent,
                          _: &mut Window,
                          cx: &mut Context<Self>| {
                        this.hidden_quota_providers.toggle(&provider);
                        this.hidden_quota_providers.persist();
                        cx.notify();
                    },
                ),
            )
            .child(icon(
                path,
                ButtonSize::Compact.icon_size().px(&theme),
                theme.text_muted,
            ))
            .into_any_element()
    }

    /// The Providers pane with nothing to show (no quota reports yet, or a
    /// pi without `quota.*`). Points at the other pane rather than sitting
    /// empty — the pill that opened it is the MCP status chip in that case.
    fn quota_popup_empty(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        div()
            .w_full()
            .py(DynamicSpacing::Base16.px(&theme))
            .flex()
            .flex_col()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(icon(
                "icons/spark.svg",
                IconSize::Medium.px(&theme),
                theme.text_muted,
            ))
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("session.no_provider_usage")),
            )
            .child(
                button_frame(div().id("quota-empty-mcp"), &theme, ButtonSize::Compact)
                    .debug_selector(|| "quota-empty-mcp".to_string())
                    .cursor_pointer()
                    .font_weight(FontWeight::MEDIUM)
                    .raised(theme.action, &theme)
                    .text_color(theme.action_fg)
                    .hover(|style| style.raised(theme.action_hover, &theme))
                    .active(|style| style.raised(theme.action_pressed, &theme))
                    .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                        this.update(cx, |app, cx| {
                            app.mcp_popup_tab(QuotaPopupTab::Mcp, cx);
                        });
                    })
                    .child(tr!("session.show_mcp_servers")),
            )
            .into_any_element()
    }

    /// The folded "Hidden" group at the foot of the top-bar quota popover.
    /// Lists the providers the user removed from the list, each with a restore
    /// button, so hiding is a one-click round trip rather than a config hunt.
    /// A header row carries the count and folds the group.
    fn quota_hidden_section(
        &self,
        reports: &[&QuotaReport],
        theme: Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open = self.quota_hidden_open;
        let chevron = if open {
            "icons/chevron-down.svg"
        } else {
            "icons/chevron-right.svg"
        };
        let mut group = div()
            .debug_selector(|| "quota-hidden".to_string())
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .id("quota-hidden-toggle")
                    .debug_selector(|| "quota-hidden-toggle".to_string())
                    .w_full()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .px(DynamicSpacing::Base04.px(&theme))
                    .py(px(2.))
                    .rounded(Radius::Medium.px(&theme))
                    .hover(|style| style.bg(theme.surface_hover))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_quota_hidden_toggle))
                    .child(icon(chevron, IconSize::XSmall.px(&theme), theme.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .text_size(TextSize::XSmall.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_muted)
                            .child(tr!("session.hidden_providers", count = reports.len())),
                    ),
            );
        if open {
            group = group.children(reports.iter().map(|report| {
                div()
                    .w_full()
                    .opacity(HIDDEN_CARD_DIM)
                    .child(quota_provider_card(
                        self,
                        report,
                        theme,
                        true,
                        Some(self.quota_visibility_button(&report.provider, true, cx)),
                    ))
            }));
        }
        group.into_any_element()
    }

    /// Re-fetch account quota on demand from the popover's refresh button.
    /// Sends `quota.list` (the patched-pi path) and drains any bridge snapshot
    /// that has already landed, so the bridge path is not gated on the slow
    /// entry poll. The spinner turns until the reply lands — held for at least
    /// [`QUOTA_REFRESH_MIN_SPIN`] — or [`QUOTA_REFRESH_TIMEOUT`] lapses.
    pub(super) fn quota_refresh(&mut self, cx: &mut Context<Self>) {
        if self.quota_refreshing {
            return;
        }
        self.quota_refreshing = true;
        self.quota_refresh_started = Some(Instant::now());
        self.refresh_quota();
        self.quota_entries_next_poll = Instant::now();
        self.poll_quota_entries();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(QUOTA_REFRESH_TIMEOUT).await;
            let _ = this.update(cx, |app, cx| {
                if app.quota_refreshing {
                    app.quota_refreshing = false;
                    app.quota_refresh_started = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// End a manual refresh once its spin has been visible for
    /// [`QUOTA_REFRESH_MIN_SPIN`]. A reply that beats that floor schedules the
    /// remainder instead of snapping the spinner away.
    pub(super) fn finish_quota_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.quota_refreshing {
            return;
        }
        let elapsed = self
            .quota_refresh_started
            .map(|started| started.elapsed())
            .unwrap_or(QUOTA_REFRESH_MIN_SPIN);
        if elapsed >= QUOTA_REFRESH_MIN_SPIN {
            self.quota_refreshing = false;
            self.quota_refresh_started = None;
            cx.notify();
            return;
        }
        let wait = QUOTA_REFRESH_MIN_SPIN - elapsed;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |app, cx| {
                if app.quota_refreshing {
                    app.quota_refreshing = false;
                    app.quota_refresh_started = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Click handler for the popover's refresh button.
    pub(super) fn on_quota_refresh(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The header's control is contextual: it probes MCP while the MCP
        // tab is showing, and re-reads provider quotas otherwise.
        if self.quota_popup_tab == QuotaPopupTab::Mcp {
            self.mcp_refresh(cx);
        } else {
            self.quota_refresh(cx);
        }
    }

    /// Toggle the top-bar quota popover.
    pub(super) fn on_quota_click(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A dismissal from this same click's mouse-down must not
        // immediately re-open (see `toggle_session_menu`).
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.toggle_quota_popup(cx);
    }

    /// Open or close the top-bar usage popover. Opening picks the pane that
    /// matches the pill the user clicked: when the pill is showing the MCP
    /// status chip (nothing metered), the popover opens on the MCP pane
    /// instead of an empty provider list.
    pub(super) fn toggle_quota_popup(&mut self, cx: &mut Context<Self>) {
        self.quota_popup_open = !self.quota_popup_open;
        // The quota popover and the session-details popover share the top
        // bar; only one is meaningful at a time.
        if self.quota_popup_open {
            self.session_details_open = false;
            let theme = *theme::get(cx);
            let quiet = matches!(
                self.quota.headline(&self.model_provider),
                QuotaHeadline::Quiet
            );
            if quiet && self.mcp_chip_parts(theme).is_some() {
                self.quota_popup_tab = QuotaPopupTab::Mcp;
            }
        }
        cx.notify();
    }

    /// Fold or unfold the popover's "Hidden" provider group.
    pub(super) fn on_quota_hidden_toggle(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.quota_hidden_open = !self.quota_hidden_open;
        cx.notify();
    }

    /// A read-only identifier row in the session-details popover, with a copy
    /// affordance that copies `value` to the clipboard.
    pub(super) fn session_detail_row(
        &self,
        ix: usize,
        label: &str,
        value: &str,
        theme: Theme,
    ) -> impl IntoElement + use<> {
        let label = label.to_string();
        let value = value.to_string();
        let v = value.clone();
        div()
            .px(DynamicSpacing::Base12.px(&theme))
            .py(px(7.))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_primary)
                            .truncate()
                            .child(if value.is_empty() {
                                "—".to_string()
                            } else {
                                value
                            }),
                    ),
            )
            .child(
                icon_button_frame(div().id(("sess-copy", ix)), &theme, ButtonSize::Compact)
                    .tip(tr!("common.copy"))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .on_click(move |_, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(v.clone()));
                    })
                    .child(icon(
                        "icons/copy.svg",
                        ButtonSize::Compact.icon_size().px(&theme),
                        theme.text_muted,
                    )),
            )
    }

    pub(super) fn on_toggle_sidebar(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar(window, cx);
        cx.notify();
    }

    /// The keyboard/menu route to the sidebar toggle (cmd/ctrl-b, View ▸
    /// Toggle Sidebar). Shares [`Self::toggle_sidebar`] with the titlebar
    /// button, so both animate the same slide generation.
    pub(super) fn on_toggle_sidebar_action(
        &mut self,
        _: &crate::ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar(window, cx);
        cx.notify();
    }

    /// Flip the sessions sidebar and bump the slide generation, so the render
    /// animates the change (and every open→close→open cycle re-animates).
    /// Hiding the sidebar while it holds keyboard focus hands focus back to
    /// the composer — a clipped column must never swallow typing.
    pub(super) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = window.focused(cx) == Some(self.sidebar_focus.clone());
        self.sidebar_visible = !self.sidebar_visible;
        self.sidebar_slide_gen = self.sidebar_slide_gen.wrapping_add(1);
        if !self.sidebar_visible {
            self.sidebar_cursor = None;
            if had_focus {
                self.input.read(cx).focus(window);
            }
        }
    }

    /// The top-bar Review control: one chip for the right dock and the
    /// working tree's `+N −N` counts. With a dirty tree and the pane closed it
    /// opens Review on the **Uncommitted** diff, so the counts the chip shows
    /// are what it reveals; otherwise it is a plain toggle.
    pub(super) fn on_toggle_side_pane(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_side_pane(cx);
    }

    /// Flip the right dock, opening Review on the working tree's Uncommitted
    /// diff when it is dirty and the pane is closed. Split from the click
    /// handler so the overflow menu can call it without synthesizing an event.
    pub(super) fn toggle_side_pane(&mut self, cx: &mut Context<Self>) {
        let pane_open = self.sidepane.read(cx).is_open();
        if !pane_open {
            // Opening Review replaces the Git page, which shares the window.
            self.close_git_for_review(cx);
        }
        let dirty = self.added > 0 || self.removed > 0;
        self.sidepane.update(cx, |pane, cx| {
            if pane_open || !dirty {
                pane.toggle(cx);
            } else {
                pane.show_uncommitted(cx);
            }
        });
        cx.notify();
    }

    /// Flip the bottom terminal panel (cmd-j). Focus moves into the shell on
    /// open, so the next keystroke lands there rather than in the composer.
    pub(super) fn on_toggle_terminal(
        &mut self,
        _: &crate::ToggleTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_panel
            .update(cx, |panel, cx| panel.toggle(window, cx));
        cx.notify();
    }

    /// Open another shell tab in the current workspace (⌘T in the terminal).
    pub(super) fn on_new_terminal_tab(
        &mut self,
        _: &crate::NewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_panel
            .update(cx, |panel, cx| panel.new_tab(window, cx));
        cx.notify();
    }

    /// Close the active terminal tab (⌘W in the terminal). Its shell gets
    /// SIGHUP; the panel closes when it was the last tab.
    pub(super) fn on_close_terminal_tab(
        &mut self,
        _: &crate::CloseTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_panel
            .update(cx, |panel, cx| panel.close_active_tab(window, cx));
        cx.notify();
    }

    /// Cycle terminal tabs (⌘⇧[ / ⌘⇧]). Wraps at the ends and does nothing
    /// with fewer than two tabs.
    pub(super) fn on_next_terminal_tab(
        &mut self,
        _: &crate::NextTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_panel
            .update(cx, |panel, cx| panel.cycle_tab(1, window, cx));
        cx.notify();
    }

    pub(super) fn on_prev_terminal_tab(
        &mut self,
        _: &crate::PrevTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_panel
            .update(cx, |panel, cx| panel.cycle_tab(-1, window, cx));
        cx.notify();
    }

    /// Switch the Git page's tab (⌘1–⌘6). A no-op unless the page is open, so
    /// the shortcuts never surprise a chat session.
    pub(super) fn on_git_tab(&mut self, index: usize, _: &mut Window, cx: &mut Context<Self>) {
        if self.git_open {
            self.git_panel
                .update(cx, |panel, cx| panel.set_tab(index, cx));
        }
    }

    /// Flip the left project-panel dock (⌘⇧E / Ctrl+Shift+E).
    pub(super) fn on_toggle_project_panel(
        &mut self,
        _: &crate::ToggleProjectPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_project_panel(cx);
    }

    pub(super) fn toggle_project_panel(&mut self, cx: &mut Context<Self>) {
        let opening = !self.project_panel.read(cx).is_open();
        self.project_panel.update(cx, |panel, cx| panel.toggle(cx));
        if opening {
            // The Explorer and the Review pane are mutually exclusive right
            // docks: opening the tree closes Review.
            self.sidepane.update(cx, |pane, cx| pane.close(cx));
        }
        cx.notify();
    }

    /// Open a workspace file in the Files surface — the project panel's open
    /// callback. One main-area page at a time, like Git and Usage. The panel
    /// passes the workspace-relative path for the toolbar label; the app must
    /// not re-read the panel here — this runs inside the panel's own listener,
    /// so the entity is already leased and `read` would abort.
    pub(super) fn open_file_in_viewer(
        &mut self,
        path: PathBuf,
        display: String,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = false;
        self.git_open = false;
        self.usage_open = false;
        self.worktrees_open = false;
        self.file_viewer
            .update(cx, |viewer, cx| viewer.show(path, display, cx));
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// Run an Explorer file operation (new file/folder, rename, delete) on the
    /// background executor, then reconcile the tree and any open Files tabs.
    /// The panel only sends workspace-relative paths; everything that touches
    /// the filesystem happens here, off the UI thread.
    pub(super) fn on_file_op(
        &mut self,
        request: crate::explorer::FileOpRequest,
        cx: &mut Context<Self>,
    ) {
        use crate::explorer::{ops, walk, FileOpRequest};

        let Some(root) = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
        else {
            self.toast_error(tr!("explorer.err_no_workspace"));
            cx.notify();
            return;
        };

        match request {
            FileOpRequest::NewFile { dir, name } => {
                let parent = walk::absolute(&root, &dir);
                let display_root = root.clone();
                let create = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { ops::create_file(&parent, &create) })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        match result {
                            Ok(path) => {
                                app.clear_explorer_notice(cx);
                                app.project_panel
                                    .update(cx, |panel, cx| panel.mark_stale(cx));
                                let display = relative_display(&display_root, &path);
                                app.open_file_in_viewer(path, display, cx);
                                app.toast_success(tr!("explorer.created_file", name = name));
                            }
                            Err(error) => app.explorer_op_error(&error, cx),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            FileOpRequest::NewFolder { dir, name } => {
                let parent = walk::absolute(&root, &dir);
                let create = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { ops::create_dir(&parent, &create) })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        match result {
                            Ok(_) => {
                                app.clear_explorer_notice(cx);
                                app.project_panel
                                    .update(cx, |panel, cx| panel.mark_stale(cx));
                                app.toast_success(tr!("explorer.created_folder", name = name));
                            }
                            Err(error) => app.explorer_op_error(&error, cx),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            FileOpRequest::Rename { path, name } => {
                let target = walk::absolute(&root, &path);
                if self.file_viewer.read(cx).has_dirty_under(&target) {
                    self.toast_warning(tr!("explorer.err_unsaved"));
                    cx.notify();
                    return;
                }
                let display_root = root.clone();
                let old_display = path.clone();
                let rename_target = target.clone();
                let rename_name = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { ops::rename(&rename_target, &rename_name) })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        match result {
                            Ok(new_path) => {
                                let new_display = relative_display(&display_root, &new_path);
                                app.file_viewer.update(cx, |viewer, cx| {
                                    viewer.reconcile_rename(
                                        &target,
                                        &new_path,
                                        &old_display,
                                        &new_display,
                                        cx,
                                    )
                                });
                                app.clear_explorer_notice(cx);
                                app.project_panel
                                    .update(cx, |panel, cx| panel.mark_stale(cx));
                                app.toast_success(tr!("explorer.renamed", name = name));
                            }
                            Err(error) => app.explorer_op_error(&error, cx),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            FileOpRequest::Delete { path, is_dir: _ } => {
                let target = walk::absolute(&root, &path);
                if self.file_viewer.read(cx).has_dirty_under(&target) {
                    self.toast_warning(tr!("explorer.err_unsaved"));
                    cx.notify();
                    return;
                }
                let name = path.rsplit('/').next().unwrap_or(path.as_str()).to_string();
                let trash_target = target.clone();
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { crate::platform::trash_path(&trash_target) })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        match result {
                            Ok(()) => {
                                app.file_viewer
                                    .update(cx, |viewer, cx| viewer.close_under(&target, cx));
                                app.clear_explorer_notice(cx);
                                app.project_panel
                                    .update(cx, |panel, cx| panel.mark_stale(cx));
                                app.toast_success(tr!("explorer.deleted", name = name));
                            }
                            Err(error) => {
                                let message = format!("{}: {error}", tr!("explorer.err_delete"));
                                app.project_panel
                                    .update(cx, |panel, cx| panel.set_notice(Some(message), cx));
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// Clear the Explorer's operation-error strip.
    fn clear_explorer_notice(&mut self, cx: &mut Context<Self>) {
        self.project_panel
            .update(cx, |panel, cx| panel.set_notice(None, cx));
    }

    /// Surface a failed create/rename in the Explorer's notice strip.
    fn explorer_op_error(&mut self, error: &crate::explorer::ops::OpError, cx: &mut Context<Self>) {
        let message = error.message();
        self.project_panel
            .update(cx, |panel, cx| panel.set_notice(Some(message), cx));
    }

    /// Leave the Files surface and return to the chat.
    pub(super) fn close_files(&mut self, cx: &mut Context<Self>) {
        self.file_viewer.update(cx, |viewer, cx| viewer.hide(cx));
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// `cmd-w` while the Files surface owns the keyboard.
    pub(super) fn on_close_files(
        &mut self,
        _: &crate::CloseFiles,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_files(cx);
    }

    /// Hands the transcript's changed-files cards a way to open the side
    /// pane's Review tab on that run's **Last Turn** git diff (the pane lives
    /// in the app, the cards don't know that). Cheap to build per frame — an
    /// `Rc` closure over the entity and the latest captured turn.
    pub(super) fn review_opener(&self, cx: &Context<Self>) -> crate::transcript_view::ReviewOpener {
        let pane = self.sidepane.clone();
        let app = cx.entity().downgrade();
        let latest = self.latest_turn;
        Rc::new(move |_window, cx| {
            // Opening Review replaces the Git page, which shares the window.
            let _ = app.update(cx, |app, cx| app.close_git_for_review(cx));
            pane.update(cx, |pane, cx| pane.show_review_turn(latest, cx));
        })
    }

    /// Hands the transcript's attachment tiles a way to open the app's
    /// full-window image lightbox (the tiles don't own that surface).
    pub(super) fn image_opener(&self, cx: &Context<Self>) -> crate::transcript_view::ImageOpener {
        let this = cx.weak_entity();
        Rc::new(move |image, _window, cx| {
            this.update(cx, |app, cx| {
                app.lightbox = Some(image);
                cx.notify();
            })
            .ok();
        })
    }

    /// Whether one of the composer's anchored popovers currently owns the
    /// keyboard. See [`Self::on_composer_click`] for why this gates focus.
    pub(super) fn composer_popup_open(&self) -> bool {
        self.model_selector.is_some()
            || self.add_menu_open
            || self.access_menu_open
            || self.workflow_menu_open
    }

    pub(super) fn on_composer_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A composer-anchored popover (model/thinking picker, add menu, access
        // menu, workflow menu) rides a descendant of the composer box, so a
        // click that opens one — or a click on its search field — bubbles here
        // after the popover has grabbed focus. Pulling focus back to the
        // composer input would silently kill the popover's ↑/↓/Enter/Escape,
        // which is exactly what made the chip pickers feel dead next to the
        // command palette and branch picker. The popovers dismiss on an
        // outside mouse-down, so a click meant to close one has already cleared
        // these flags by the time the mouse-up lands.
        if self.composer_popup_open() {
            return;
        }
        self.input.read(cx).focus(window);
    }

    pub(super) fn on_refresh(
        &mut self,
        _: &crate::RefreshSessions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Sessions first: the CLI or another Orbit window may have written
        // since the list was last read.
        self.sessions = sessions::load_sessions();
        self.sync_session_menu(cx);
        // Worktrees for the active repository, when their page is up.
        if self.worktrees_open {
            self.refresh_worktrees(cx);
        }
        // The Explorer tree (a cheap no-op when it was never loaded).
        self.project_panel
            .update(cx, |panel, cx| panel.mark_stale(cx));
        // The Review pane's diff, only while it is showing.
        if self.sidepane.read(cx).is_open() {
            self.sidepane
                .update(cx, |pane, cx| pane.refresh_from_button(cx));
        }
        // Git status/branch (the panel itself no-ops when closed).
        self.git_panel.update(cx, |panel, cx| panel.refresh(cx));
        // Usage analytics when its page is open.
        if self.usage_open {
            self.usage.update(cx, |page, cx| page.refresh(cx));
        }
        // Settings-backed catalogs: MCP config/status and installed packages
        // are only meaningful while their section is on screen.
        if self.settings_open {
            match self.settings_section {
                SettingsSection::Mcp => self.mcp_refresh(cx),
                SettingsSection::Plugins => self.refresh_plugins(cx),
                _ => {}
            }
        }
        // The running agent can change under us (re-auth, model list, a
        // compaction moved the context estimate) — re-ask it directly.
        if self.client.is_some() {
            self.refresh_auth();
            self.refresh_catalogs();
            self.refresh_context_stats();
        }
        cx.notify();
    }

    /// `cmd-shift-c`: copy the newest assistant response — the keyboard
    /// mirror of the message footer's copy button. Lights the same green
    /// check in that row's footer.
    pub(super) fn on_copy_last_response(
        &mut self,
        _: &crate::CopyLastResponse,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((ix, text)) = self.transcript.last_response_text() else {
            self.toast_warning(tr!("session.no_response_to_copy"));
            cx.notify();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.transcript.mark_copied(ix);
        self.toast_success(tr!("session.copied_latest_response"));
        cx.notify();
    }

    /// `cmd-up` / `cmd-down`: jump between user turns — the keyboard mirror
    /// of the navigation rail. Jumping is also using the rail, so it
    /// dismisses the one-time rail hint.
    pub(super) fn on_prev_turn(
        &mut self,
        _: &crate::PrevTurn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transcript.jump_turn(-1);
        self.transcript.dismiss_rail_hint();
        cx.notify();
    }

    pub(super) fn on_next_turn(
        &mut self,
        _: &crate::NextTurn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transcript.jump_turn(1);
        self.transcript.dismiss_rail_hint();
        cx.notify();
    }
}

/// Meter/accent color for a consumed fraction. Green → amber → red at the
/// same thresholds the Settings quota meters use, so the two surfaces agree.
fn quota_tint(fraction: f32, theme: Theme) -> Hsla {
    if fraction >= 0.9 {
        theme.error
    } else if fraction >= 0.75 {
        theme.warning
    } else {
        theme.success
    }
}

/// Amount text for the quota surfaces: whole amounts drop the decimals so a
/// balance reads `110 CNY`, not `110.00 CNY`.
fn quota_amount(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Countdown copy for a window reset in the top-bar popover: `resets in
/// 3h 12m` is read at a glance where an absolute timestamp needs arithmetic.
/// Past a week the countdown stops helping, and a timestamp in the past is
/// stale, so both fall back to the absolute local time the Settings card
/// shows. `now_ms` is injected to keep the label a pure function.
fn quota_reset_hint(resets_at: i64, now_ms: i64) -> String {
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let remaining = resets_at.saturating_sub(now_ms);
    let hint = if remaining <= 0 {
        return tr!("session.resets_at", time = format_epoch_ms(resets_at));
    } else if remaining < MINUTE {
        tr!("session.in_lt_1m")
    } else if remaining < HOUR {
        tr!("session.in_minutes", minutes = remaining / MINUTE)
    } else if remaining < DAY {
        let hours = remaining / HOUR;
        let minutes = (remaining % HOUR) / MINUTE;
        if minutes == 0 {
            tr!("session.in_hours", hours = hours)
        } else {
            tr!("session.in_hours_minutes", hours = hours, minutes = minutes)
        }
    } else if remaining < 7 * DAY {
        let days = remaining / DAY;
        let hours = (remaining % DAY) / HOUR;
        if hours == 0 {
            tr!("session.in_days", days = days)
        } else {
            tr!("session.in_days_hours", days = days, hours = hours)
        }
    } else {
        return tr!("session.resets_at", time = format_epoch_ms(resets_at));
    };
    tr!("session.resets_in", hint = hint)
}

/// One provider card in the top-bar quota popover: a raised block carrying
/// the provider's mark, plan, windows, and balances, with the meter and its
/// countdown beside every window. Provider-independent: it renders whatever
/// the normalized report carries (windows, balances, note, error) and names
/// the provider by id only, never a bespoke label. `action` is the caller's
/// hide/show toggle, dropped into the header's trailing edge.
fn quota_provider_card(
    app: &OrbitApp,
    report: &QuotaReport,
    theme: Theme,
    hidden: bool,
    action: Option<AnyElement>,
) -> AnyElement {
    let name = providers::provider_display_name(&report.provider);
    let amount = quota_amount;

    let mut header = div()
        .flex()
        .items_center()
        .gap(px(7.))
        .child(icon_dyn(
            provider_icon(&report.provider),
            IconSize::Small.px(&theme),
            provider_mark_color(&report.provider, &theme, theme.text_muted),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(name),
        );
    if let Some(plan) = &report.plan {
        header = header.child(app.provider_badge(
            plan,
            theme.accent_text,
            theme.accent.opacity(0.12),
            theme,
        ));
    }
    if let Some(action) = action {
        header = header.child(action);
    }

    // The card fill is the ink wash rather than `surface_elevated`: the popup
    // itself already sits on that elevation, so a raised card would vanish
    // inside it. The wash steps off any surface in every palette.
    let card_selector = format!(
        "quota-card-{}-{}",
        if hidden { "hidden" } else { "visible" },
        report.provider
    );
    let mut block = div()
        .debug_selector(move || card_selector.clone())
        .w_full()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.overlay)
        .px(DynamicSpacing::Base12.px(&theme))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base08.px(&theme))
        .child(header);

    for window in &report.windows {
        let value = if let Some(percent) = window.used_percent {
            tr!("session.percent_used", percent = format!("{percent:.0}"))
        } else if let (Some(used), Some(limit)) = (window.used, window.limit) {
            tr!(
                "session.used_of_limit",
                used = amount(used),
                limit = amount(limit)
            )
        } else if let Some(used) = window.used {
            match &window.unit {
                Some(unit) => tr!("session.amount_unit", amount = amount(used), unit = unit),
                None => amount(used),
            }
        } else {
            continue;
        };

        let mut row = div().w_full().flex().flex_col().gap(px(5.)).child(
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(window.label.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(value),
                ),
        );

        if let Some(fraction) = window.fraction() {
            row = row.child(
                div()
                    .w_full()
                    .h(px(5.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(theme.surface_secondary)
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(quota_tint(fraction, theme))
                            .w(relative(fraction)),
                    ),
            );
        }
        if let Some(resets_at) = window.resets_at {
            row = row.child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(quota_reset_hint(
                        resets_at,
                        chrono::Local::now().timestamp_millis(),
                    )),
            );
        }
        block = block.child(row);
    }

    for balance in &report.balances {
        let text = if balance.currency.is_empty() {
            amount(balance.amount)
        } else {
            format!("{} {}", amount(balance.amount), balance.currency)
        };
        block = block.child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(balance.label.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(text),
                ),
        );
    }

    if let Some(error) = &report.error {
        block = block.child(
            div()
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.error)
                .child(error.clone()),
        );
    } else if note_should_render(report) {
        if let Some(note) = &report.note {
            block = block.child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_muted)
                    .child(note.clone()),
            );
        }
    }

    block.into_any_element()
}

#[cfg(test)]
mod quota_reset_tests {
    use super::*;

    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    #[test]
    fn reset_hint_reads_as_a_short_countdown() {
        let now = 1_000_000_000_000;
        assert_eq!(quota_reset_hint(now + 30_000, now), "resets in <1m");
        assert_eq!(quota_reset_hint(now + 42 * MINUTE, now), "resets in 42m");
        assert_eq!(quota_reset_hint(now + 2 * HOUR, now), "resets in 2h");
        assert_eq!(
            quota_reset_hint(now + 3 * HOUR + 12 * MINUTE, now),
            "resets in 3h 12m"
        );
        assert_eq!(quota_reset_hint(now + 3 * DAY, now), "resets in 3d");
        assert_eq!(
            quota_reset_hint(now + 2 * DAY + 4 * HOUR + 5 * MINUTE, now),
            "resets in 2d 4h"
        );
    }

    #[test]
    fn reset_hint_falls_back_to_the_absolute_time() {
        let now = 1_000_000_000_000;
        // More than a week out: a countdown stops being useful.
        let far = now + 8 * DAY;
        assert_eq!(
            quota_reset_hint(far, now),
            format!("resets {}", format_epoch_ms(far))
        );
        // A stale report (reset time already passed) also stays absolute.
        let past = now - DAY;
        assert_eq!(
            quota_reset_hint(past, now),
            format!("resets {}", format_epoch_ms(past))
        );
    }
}
