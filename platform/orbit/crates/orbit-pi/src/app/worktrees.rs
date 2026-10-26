//! Worktrees page and controller.
//!
//! The feature surface for Git worktrees, driven entirely by
//! [`crate::worktree::WorktreeManager`]: discovery for the active repository,
//! creating a worktree from a new or existing branch, opening one as an
//! ordinary Orbit workspace, renaming/moving/locking/removing rows, and the
//! optional setup script. A worktree is a workspace like any other once
//! opened — Explorer, Git, Terminal, Review, and the agent all follow
//! `current_workspace`, so this module never duplicates their plumbing.
//!
//! Mutations are serialized per repository (`worktree_mutation_repo`) and every
//! one of them ends in a fresh [`OrbitApp::refresh_worktrees`], so the list,
//! the active workspace, and the UI reconcile after Git's answer — never
//! against a stale picture.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{AnyElement, MouseDownEvent, MouseUpEvent};

use super::helpers::*;
use super::*;
use crate::theme::tokens::{
    context_menu, input, modal, picker, popover, ButtonSize, DynamicSpacing, IconSize, Radius,
    RaisedExt, TextSize,
};
use crate::worktree::{self, Worktree, WorktreeError, WorktreeManager};
use crate::worktree_setup;

/// Watcher-driven re-list throttle while the page is open (§30): a burst of
/// file saves must not run `git worktree list` per event.
const WATCH_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// Page column width; matches the Settings body.
const PAGE_MAX_W: f32 = 720.;
/// The one-time "How worktrees work" card's key in `hints.json`.
pub(super) const WORKTREE_HOWTO_HINT_KEY: &str = "worktree_howto_dismissed";

impl OrbitApp {
    // ── page lifecycle ─────────────────────────────────────────────────

    /// Open the Worktrees page and load the repository's worktrees.
    pub(super) fn open_worktrees(&mut self, cx: &mut Context<Self>) {
        self.worktrees_open = true;
        // One main-area feature at a time (Git / Usage / Files / Review).
        if self.git_open {
            self.close_git(cx);
        }
        if self.usage_open {
            self.close_usage(cx);
        }
        self.session_details_open = false;
        self.settings_open = false;
        self.close_files(cx);
        self.sidepane.update(cx, |pane, cx| pane.close(cx));
        self.refresh_worktrees(cx);
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// Leave the Worktrees page.
    pub(super) fn close_worktrees(&mut self, cx: &mut Context<Self>) {
        self.worktrees_open = false;
        self.worktree_menu = None;
        self.worktree_advanced_open = false;
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// Palette / context-menu entry: toggle the page.
    pub(super) fn on_open_worktrees(
        &mut self,
        _: &crate::OpenWorktrees,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.worktrees_open {
            self.close_worktrees(cx);
        } else {
            self.open_worktrees(cx);
        }
    }

    /// A manager for the active workspace, carrying the cached main root so
    /// path resolution never runs Git on the UI thread.
    pub(super) fn worktree_manager(&self) -> Option<WorktreeManager> {
        let repo = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())?;
        let mut manager = WorktreeManager::new(repo).with_config(self.worktree_config.clone());
        if let Some(root) = &self.worktree_repo_root {
            manager = manager.with_root(root.clone());
        }
        Some(manager)
    }

    /// The main worktree root, from the last list when known.
    fn worktree_root_hint(&self) -> Option<PathBuf> {
        self.worktree_repo_root.clone()
    }

    /// Display label for the active repository.
    fn worktree_repo_label(&self) -> String {
        self.worktree_root_hint()
            .or_else(|| self.current_workspace.clone())
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default()
    }

    /// Re-list the active repository's worktrees. Safe to call at any time;
    /// while a list is in flight this remembers to run another when it
    /// settles, so a workspace switch cannot be served stale rows.
    pub(super) fn refresh_worktrees(&mut self, cx: &mut Context<Self>) {
        if self.worktrees_busy {
            self.worktree_refresh_pending = true;
            return;
        }
        let Some(manager) = self.worktree_manager() else {
            self.worktrees.clear();
            return;
        };
        self.worktrees_busy = true;
        self.worktree_refresh_pending = false;
        self.worktree_fetch = self.worktree_fetch.wrapping_add(1);
        let fetch = self.worktree_fetch;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let root = manager.repository_root().ok();
                    let list = manager.list();
                    (root, list)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.worktrees_busy = false;
                let stale = app.worktree_fetch != fetch;
                if !stale {
                    match result {
                        (root, Ok(worktrees)) => {
                            if root.is_some() {
                                app.worktree_repo_root = root;
                            }
                            app.worktrees = worktrees;
                            app.worktrees_error = None;
                        }
                        (_, Err(err)) => {
                            app.worktrees.clear();
                            app.worktrees_error = Some(err.to_string());
                        }
                    }
                }
                let rerun = app.worktree_refresh_pending || stale;
                app.worktree_refresh_pending = false;
                if rerun {
                    app.refresh_worktrees(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Watcher-driven re-list, throttled (§30).
    pub(super) fn refresh_worktrees_soon(&mut self, cx: &mut Context<Self>) {
        if !self.worktrees_open || self.worktrees_busy {
            return;
        }
        let now = Instant::now();
        if self.worktree_refresh_due.is_some_and(|due| now < due) {
            return;
        }
        self.worktree_refresh_due = Some(now + WATCH_REFRESH_INTERVAL);
        self.refresh_worktrees(cx);
    }

    /// The worktree row whose path matches `path`, if it is listed.
    fn listed_worktree(&self, path: &Path) -> Option<&Worktree> {
        self.worktrees
            .iter()
            .find(|worktree| worktree::same_path(&worktree.path, path))
    }

    /// Whether `path` is the active workspace.
    fn worktree_is_active(&self, path: &Path) -> bool {
        self.current_workspace
            .as_deref()
            .is_some_and(|active| worktree::same_path(active, path))
    }

    // ── create flow ────────────────────────────────────────────────────

    /// Open the Create Worktree dialog and load the branch choices.
    pub(super) fn open_create_worktree_dialog(&mut self, cx: &mut Context<Self>) {
        self.worktree_dialog = Some(WorktreeDialog::Create);
        self.worktree_dialog_error = None;
        self.worktree_branch_mode = WorktreeBranchMode::New;
        self.worktree_branch_choice = None;
        self.worktree_run_setup = self.worktree_config.run_setup_auto;
        // Reset the derivation state *after* clearing the fields, so the
        // branch observer firing on `clear` cannot leave it marked as
        // user-edited before the user types anything.
        self.worktree_name_input
            .update(cx, |input, cx| input.clear(cx));
        self.worktree_branch_input
            .update(cx, |input, cx| input.clear(cx));
        self.worktree_start_input
            .update(cx, |input, cx| input.clear(cx));
        self.worktree_branch_touched = false;
        self.worktree_branch_programmatic = false;

        // Branch choices load off-thread: listing branches and refs are Git
        // calls, and render must never wait on them.
        if let Some(manager) = self.worktree_manager() {
            cx.spawn(async move |this, cx| {
                let branches = cx
                    .background_executor()
                    .spawn(async move {
                        let repo = manager.repository_root().ok()?;
                        let mut branches = crate::git::list_branches(&repo).unwrap_or_default();
                        for reference in crate::git_ops::list_refs(&repo) {
                            if reference.kind == crate::git::RefKind::Remote {
                                branches.push(reference.name);
                            }
                        }
                        Some(branches)
                    })
                    .await;
                let _ = this.update(cx, |app, cx| {
                    if let Some(branches) = branches {
                        // Prefer a branch that is not checked out in the
                        // active workspace; that one is usually taken.
                        let active = app
                            .current_workspace
                            .as_deref()
                            .and_then(crate::git::current_branch);
                        app.worktree_branch_choice = branches
                            .iter()
                            .find(|branch| active.as_deref() != Some(branch.as_str()))
                            .cloned()
                            .or_else(|| branches.first().cloned());
                        app.worktree_branches = branches;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Close the open dialog without acting.
    pub(super) fn cancel_worktree_dialog(&mut self, cx: &mut Context<Self>) {
        self.worktree_dialog = None;
        self.worktree_dialog_error = None;
        cx.notify();
    }

    /// Dismiss when the scrim (outside the card) is pressed.
    pub(super) fn on_worktree_dialog_scrim(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_worktree_dialog(cx);
    }

    /// The dialog's primary button.
    pub(super) fn confirm_worktree_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.worktree_dialog.clone() else {
            return;
        };
        match dialog {
            WorktreeDialog::Create => self.submit_create_worktree(cx),
            WorktreeDialog::AllowSetup(pending) => {
                if let Some(root) = self.worktree_root_hint() {
                    self.worktree_config.allow_setup(&root);
                    let _ = self.worktree_config.persist();
                }
                self.worktree_dialog = None;
                self.perform_worktree_create(pending, cx);
            }
            WorktreeDialog::Rename { path } => self.submit_move_worktree(path, true, cx),
            WorktreeDialog::Move { path } => self.submit_move_worktree(path, false, cx),
            WorktreeDialog::Remove { path, dirty } => {
                self.worktree_dialog = None;
                let force = dirty;
                let label = tr!(
                    "worktree.operation.removing",
                    name = self.worktree_display_name(&path)
                );
                self.run_worktree_mutation(
                    label,
                    cx,
                    move |manager| manager.remove(&path, force),
                    |_app, _| {},
                );
            }
            WorktreeDialog::BranchInUse { path, .. } => {
                self.cancel_worktree_dialog(cx);
                self.open_worktree(&path, cx);
            }
            WorktreeDialog::Unavailable { .. } => {
                self.worktree_dialog = None;
                self.open_worktrees(cx);
            }
        }
    }

    fn worktree_display_name(&self, path: &Path) -> String {
        self.listed_worktree(path)
            .map(|worktree| worktree.name.clone())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    }

    /// Validate the create dialog and dispatch. A repository's first setup
    /// run goes through the explicit permission dialog; everything else runs
    /// straight away.
    fn submit_create_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = self.worktree_manager() else {
            self.worktree_dialog_error = Some(tr!("worktree.error.not_a_repository"));
            return;
        };
        let name = self.worktree_name_input.read(cx).text().trim().to_string();
        if let Err(err) = worktree::validate_name(&name) {
            self.worktree_dialog_error = Some(err.to_string());
            return;
        }
        let path = match manager.resolve_path(&name) {
            Ok(path) => path,
            Err(err) => {
                self.worktree_dialog_error = Some(err.to_string());
                return;
            }
        };
        let (branch, start_point) = match self.worktree_branch_mode {
            WorktreeBranchMode::New => (
                self.worktree_branch_input
                    .read(cx)
                    .text()
                    .trim()
                    .to_string(),
                {
                    let start = self.worktree_start_input.read(cx).text().trim().to_string();
                    (!start.is_empty()).then_some(start)
                },
            ),
            WorktreeBranchMode::Existing => match self.worktree_branch_choice.clone() {
                Some(branch) => (branch, None),
                None => {
                    self.worktree_dialog_error = Some(tr!("worktree.error.select_branch"));
                    return;
                }
            },
        };
        if branch.is_empty() {
            self.worktree_dialog_error = Some(tr!("git.branch_name_empty"));
            return;
        }
        // The setup script is a file check against the cached root, so no Git
        // runs on the UI thread here.
        let script = self
            .worktree_root_hint()
            .and_then(|root| self.worktree_config.effective_setup_script(&root));
        let run_setup = self.worktree_run_setup && script.is_some();
        let pending = PendingWorktreeCreate {
            name,
            path,
            branch,
            existing_branch: self.worktree_branch_mode == WorktreeBranchMode::Existing,
            start_point,
            run_setup,
        };
        let needs_permission = run_setup
            && !self
                .worktree_root_hint()
                .is_some_and(|root| self.worktree_config.setup_allowed(&root));
        if needs_permission {
            self.worktree_dialog = Some(WorktreeDialog::AllowSetup(pending));
            self.worktree_dialog_error = None;
            cx.notify();
            return;
        }
        self.perform_worktree_create(pending, cx);
    }

    /// Run `git worktree add` on the background executor, then optionally the
    /// setup script and the workspace open.
    fn perform_worktree_create(&mut self, pending: PendingWorktreeCreate, cx: &mut Context<Self>) {
        let Some(manager) = self.worktree_manager() else {
            return;
        };
        self.worktree_dialog = None;
        self.worktree_dialog_error = None;
        self.worktree_mutation_repo = Some(manager.repo().to_path_buf());
        self.worktree_operation = Some(tr!(
            "worktree.operation.creating",
            name = pending.name.clone()
        ));
        let open_after = self.worktree_config.open_after_create;
        let run_setup = pending.run_setup;
        cx.spawn(async move |this, cx| {
            let pending_bg = pending.clone();
            let (created, root, script) = cx
                .background_executor()
                .spawn(async move {
                    let root = manager.repository_root().ok();
                    let script = root
                        .as_deref()
                        .and_then(|root| manager.config().effective_setup_script(root));
                    let created = if pending_bg.existing_branch {
                        manager.create_from_existing_branch(
                            &pending_bg.name,
                            &pending_bg.path,
                            &pending_bg.branch,
                        )
                    } else {
                        manager.create_new_branch(
                            &pending_bg.name,
                            &pending_bg.path,
                            &pending_bg.branch,
                            pending_bg.start_point.as_deref(),
                        )
                    };
                    (created, root, script)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.worktree_mutation_repo = None;
                app.worktree_operation = None;
                match created {
                    Ok(worktree) => {
                        app.worktrees_error = None;
                        app.toast_success(tr!(
                            "worktree.created_toast",
                            name = pending.name.clone()
                        ));
                        if run_setup {
                            if let (Some(root), Some(script)) = (root, script) {
                                // Setup first, then open: the progress banner
                                // stays on the page until the script settles
                                // (§10), and a failed setup never auto-opens.
                                app.begin_worktree_setup(
                                    root,
                                    script,
                                    worktree.path.clone(),
                                    open_after,
                                    cx,
                                );
                            } else if open_after {
                                app.open_worktree(&worktree.path, cx);
                            }
                        } else if open_after {
                            app.open_worktree(&worktree.path, cx);
                        }
                        app.refresh_worktrees(cx);
                    }
                    Err(err) => {
                        app.finish_worktree_error(err, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run the setup script asynchronously, capturing its output. A failure
    /// never removes the worktree Git just created.
    fn begin_worktree_setup(
        &mut self,
        repo_root: PathBuf,
        script: PathBuf,
        worktree: PathBuf,
        open_after: bool,
        cx: &mut Context<Self>,
    ) {
        self.worktree_setup = Some(WorktreeSetupState {
            worktree: worktree.clone(),
            script: script.clone(),
            outcome: None,
            expanded: false,
            open_after,
        });
        self.spawn_setup_script(repo_root, script, worktree, open_after, cx);
    }

    fn spawn_setup_script(
        &mut self,
        repo_root: PathBuf,
        script: PathBuf,
        worktree: PathBuf,
        open_after: bool,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let run_path = worktree.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move { worktree_setup::run(&script, &repo_root, &run_path) })
                .await;
            let _ = this.update(cx, |app, cx| {
                let success = outcome.success;
                if let Some(state) = app.worktree_setup.as_mut() {
                    state.outcome = Some(outcome.clone());
                }
                if !success {
                    app.toast_warning(outcome.summary());
                } else {
                    app.toast_success(tr!("worktree.setup.succeeded_toast"));
                    if open_after {
                        // The created worktree opens only once its setup is done.
                        app.open_worktree(&worktree, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Retry the setup script from the progress banner.
    pub(super) fn retry_worktree_setup(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.worktree_setup.clone() else {
            return;
        };
        let Some(root) = self.worktree_root_hint() else {
            return;
        };
        self.worktree_setup = Some(WorktreeSetupState {
            worktree: state.worktree.clone(),
            script: state.script.clone(),
            outcome: None,
            expanded: state.expanded,
            open_after: state.open_after,
        });
        self.spawn_setup_script(root, state.script, state.worktree, state.open_after, cx);
        cx.notify();
    }

    /// Dismiss the setup progress banner.
    pub(super) fn dismiss_worktree_setup(&mut self, cx: &mut Context<Self>) {
        self.worktree_setup = None;
        cx.notify();
    }

    /// Expand/collapse the captured setup output.
    pub(super) fn toggle_worktree_setup_output(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.worktree_setup.as_mut() {
            state.expanded = !state.expanded;
        }
        cx.notify();
    }

    // ── row actions ────────────────────────────────────────────────────

    /// Open a worktree as the active Orbit workspace. Everything else —
    /// Explorer, Git, Terminal, Review, agent cwd — follows
    /// `current_workspace`, so no other wiring is needed.
    pub(super) fn open_worktree(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !path.is_dir() {
            self.worktrees_error = Some(tr!(
                "worktree.error.missing_directory",
                path = path.to_string_lossy()
            ));
            cx.notify();
            return;
        }
        let path = path.to_path_buf();
        self.add_workspace(path.clone(), cx);
        self.set_current_workspace(path, cx);
        self.close_worktrees(cx);
    }

    /// Toggle the row-actions popup.
    pub(super) fn toggle_worktree_menu(
        &mut self,
        path: PathBuf,
        at: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        if self
            .worktree_menu
            .as_ref()
            .is_some_and(|open| open.path == path)
        {
            self.worktree_menu = None;
        } else {
            self.worktree_menu = Some(WorktreeMenu { path, at });
        }
        cx.notify();
    }

    pub(super) fn dismiss_worktree_menu(&mut self, cx: &mut Context<Self>) {
        self.worktree_menu = None;
        cx.notify();
    }

    /// Toggle the toolbar "Advanced" menu. The click's mouse-down dismisses an
    /// open menu (`on_mouse_down_out`); without the gesture guard the same
    /// click's mouse-up would toggle it straight back open, so a second click
    /// never closed it.
    pub(super) fn toggle_worktree_advanced(&mut self, cx: &mut Context<Self>) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.worktree_advanced_open = !self.worktree_advanced_open;
        cx.notify();
    }

    /// Open the rename dialog with the current name prefilled. The Git
    /// branch is never renamed — only the directory.
    pub(super) fn start_rename_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let name = self.worktree_display_name(&path);
        self.worktree_field_input
            .update(cx, |input, cx| input.set_text(name, cx));
        self.worktree_dialog = Some(WorktreeDialog::Rename { path });
        self.worktree_dialog_error = None;
        self.worktree_menu = None;
        cx.notify();
    }

    /// Open the move dialog with the current parent prefilled.
    pub(super) fn start_move_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let parent = path
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.worktree_field_input
            .update(cx, |input, cx| input.set_text(parent, cx));
        self.worktree_dialog = Some(WorktreeDialog::Move { path });
        self.worktree_dialog_error = None;
        self.worktree_menu = None;
        cx.notify();
    }

    /// Commit a rename (`new_name = true`) or a move. Rename resolves the
    /// field as a name under the worktree root; Move resolves it as a
    /// destination directory.
    fn submit_move_worktree(&mut self, path: PathBuf, new_name: bool, cx: &mut Context<Self>) {
        let Some(manager) = self.worktree_manager() else {
            return;
        };
        let value = self.worktree_field_input.read(cx).text().trim().to_string();
        if value.is_empty() {
            self.worktree_dialog_error = Some(if new_name {
                tr!("worktree.error.invalid_name", name = value)
            } else {
                tr!("worktree.error.move_destination")
            });
            return;
        }
        let destination = if new_name {
            match manager.resolve_path(&value) {
                Ok(destination) => destination,
                Err(err) => {
                    self.worktree_dialog_error = Some(err.to_string());
                    return;
                }
            }
        } else {
            worktree::expand_path(
                self.worktree_root_hint()
                    .as_deref()
                    .unwrap_or(Path::new(".")),
                &value,
            )
        };
        if worktree::same_path(&path, &destination) {
            self.cancel_worktree_dialog(cx);
            return;
        }
        if destination.exists() {
            self.worktree_dialog_error = Some(tr!(
                "worktree.error.path_exists",
                path = destination.to_string_lossy()
            ));
            return;
        }
        self.worktree_dialog = None;
        let label = tr!(
            "worktree.operation.moving",
            name = self.worktree_display_name(&path)
        );
        self.worktree_operation = Some(label);
        self.worktree_mutation_repo = Some(manager.repo().to_path_buf());
        let old_path = path.clone();
        let destination_for_task = destination.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { manager.move_to(&old_path, &destination_for_task) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.worktree_operation = None;
                app.worktree_mutation_repo = None;
                match result {
                    Ok(()) => {
                        // A moved active workspace follows its directory so a
                        // running session is not stranded on a dead path.
                        if app.worktree_is_active(&path) {
                            app.set_current_workspace(destination.clone(), cx);
                        }
                        app.worktrees_error = None;
                    }
                    Err(err) => app.finish_worktree_error(err, cx),
                }
                app.refresh_worktrees(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask before removing: read the worktree's dirty state off-thread first,
    /// so the confirmation can say what is at stake.
    pub(super) fn start_remove_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.worktree_menu = None;
        let Some(manager) = self.worktree_manager() else {
            return;
        };
        let path_for_task = path.clone();
        cx.spawn(async move |this, cx| {
            let dirty = cx
                .background_executor()
                .spawn(async move { manager.is_dirty(&path_for_task).unwrap_or(false) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.worktree_dialog = Some(WorktreeDialog::Remove { path, dirty });
                app.worktree_dialog_error = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// Lock a worktree (`git worktree lock`).
    pub(super) fn lock_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.worktree_menu = None;
        let label = tr!(
            "worktree.operation.locking",
            name = self.worktree_display_name(&path)
        );
        self.run_worktree_mutation(
            label,
            cx,
            move |manager| manager.lock(&path, None),
            |_app, _| {},
        );
    }

    /// Unlock a worktree (`git worktree unlock`).
    pub(super) fn unlock_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.worktree_menu = None;
        let label = tr!(
            "worktree.operation.unlocking",
            name = self.worktree_display_name(&path)
        );
        self.run_worktree_mutation(
            label,
            cx,
            move |manager| manager.unlock(&path),
            |_app, _| {},
        );
    }

    /// `git worktree prune`: drop administrative entries for worktrees whose
    /// directories are gone.
    pub(super) fn prune_worktrees(&mut self, cx: &mut Context<Self>) {
        self.worktree_advanced_open = false;
        let label = tr!("worktree.operation.pruning");
        self.run_worktree_mutation(label, cx, |manager| manager.prune(), |_app, _| {});
    }

    /// `git worktree repair`: rebuild administrative files after a manual
    /// move.
    pub(super) fn repair_worktrees(&mut self, cx: &mut Context<Self>) {
        self.worktree_advanced_open = false;
        let label = tr!("worktree.operation.repairing");
        self.run_worktree_mutation(label, cx, |manager| manager.repair(), |_app, _| {});
    }

    /// Copy a worktree's path.
    pub(super) fn copy_worktree_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            path.to_string_lossy().into_owned(),
        ));
        self.worktree_menu = None;
        self.set_status(tr!("worktree.path_copied"));
        cx.notify();
    }

    /// Reveal a worktree in the OS file manager.
    pub(super) fn reveal_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.worktree_menu = None;
        crate::platform::reveal_in_file_manager(&path);
        cx.notify();
    }

    /// Serialize one mutating operation, then re-list. A second mutation
    /// while one runs is dropped, not queued: the row state it would target
    /// may already be gone (§31).
    fn run_worktree_mutation<F, D>(
        &mut self,
        label: String,
        cx: &mut Context<Self>,
        operation: F,
        done: D,
    ) where
        F: FnOnce(WorktreeManager) -> Result<(), WorktreeError> + Send + 'static,
        D: FnOnce(&mut OrbitApp, Result<(), WorktreeError>) + Send + 'static,
    {
        if self.worktree_mutation_repo.is_some() {
            return;
        }
        let Some(manager) = self.worktree_manager() else {
            return;
        };
        self.worktree_mutation_repo = Some(manager.repo().to_path_buf());
        self.worktree_operation = Some(label);
        self.worktrees_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { operation(manager) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.worktree_mutation_repo = None;
                app.worktree_operation = None;
                match &result {
                    Ok(()) => {
                        app.worktrees_error = None;
                        done(app, Ok(()));
                    }
                    Err(err) => {
                        let err = err.clone();
                        app.finish_worktree_error(err.clone(), cx);
                        done(app, Err(err));
                    }
                }
                app.refresh_worktrees(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Route a mutation error: an already-checked-out branch gets the
    /// actionable dialog, everything else the page banner.
    fn finish_worktree_error(&mut self, err: WorktreeError, cx: &mut Context<Self>) {
        if let WorktreeError::BranchCheckedOut { branch, path } = err {
            self.worktree_dialog = Some(WorktreeDialog::BranchInUse { branch, path });
            self.worktree_dialog_error = None;
        } else {
            self.worktrees_error = Some(err.to_string());
        }
        cx.notify();
    }

    // ── settings ───────────────────────────────────────────────────────

    /// Settings → Worktrees rows.
    pub(super) fn worktree_settings_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let repo_label = self.worktree_repo_label();
        let directory_hint = if repo_label.is_empty() {
            tr!("worktree.settings.directory_hint")
        } else {
            tr!(
                "worktree.settings.directory_hint_repo",
                repo = repo_label.clone()
            )
        };
        vec![
            self.settings_section(
                theme,
                &tr!("worktree.settings.manage_section"),
                vec![self.setting_row(
                    theme,
                    &tr!("worktree.settings.manage"),
                    Some(&tr!("worktree.settings.manage_hint")),
                    None,
                    Some(worktree_primary_button(
                        "worktree-settings-open",
                        tr!("worktree.settings.manage_open"),
                        theme,
                        this.clone(),
                        |app, cx| app.open_worktrees(cx),
                    )),
                )],
            ),
            self.settings_section(
                theme,
                &tr!("worktree.settings.location_section"),
                vec![
                    self.setting_row(
                        theme,
                        &tr!("worktree.settings.directory"),
                        Some(&directory_hint),
                        None,
                        Some(self.settings_text_field(&self.worktree_dir_input, &theme)),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("worktree.settings.setup_script"),
                        Some(&tr!("worktree.settings.setup_script_hint")),
                        None,
                        Some(self.settings_text_field(&self.worktree_script_input, &theme)),
                    ),
                ],
            ),
            self.settings_section(
                theme,
                &tr!("worktree.settings.creation_section"),
                vec![
                    self.setting_row(
                        theme,
                        &tr!("worktree.settings.run_setup_auto"),
                        Some(&tr!("worktree.settings.run_setup_auto_hint")),
                        None,
                        Some(self.settings_toggle(
                            "worktree-run-setup-auto",
                            self.worktree_config.run_setup_auto,
                            theme,
                            this.clone(),
                            Self::toggle_worktree_run_setup_auto,
                        )),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("worktree.settings.open_after_create"),
                        Some(&tr!("worktree.settings.open_after_create_hint")),
                        None,
                        Some(self.settings_toggle(
                            "worktree-open-after-create",
                            self.worktree_config.open_after_create,
                            theme,
                            this.clone(),
                            Self::toggle_worktree_open_after_create,
                        )),
                    ),
                ],
            ),
        ]
    }

    /// The Settings field for a worktree text input (fixed width so the two
    /// rows align).
    fn settings_text_field(&self, input: &Entity<ComposerInput>, theme: &Theme) -> AnyElement {
        input_field_frame(div(), theme)
            .w(px(240.))
            .bg(theme.background)
            .child(input.clone())
            .into_any_element()
    }

    pub(super) fn toggle_worktree_run_setup_auto(&mut self, cx: &mut Context<Self>) {
        self.worktree_config.run_setup_auto = !self.worktree_config.run_setup_auto;
        let _ = self.worktree_config.persist();
        cx.notify();
    }

    pub(super) fn toggle_worktree_open_after_create(&mut self, cx: &mut Context<Self>) {
        self.worktree_config.open_after_create = !self.worktree_config.open_after_create;
        let _ = self.worktree_config.persist();
        cx.notify();
    }

    /// The create dialog's per-worktree setup checkbox.
    pub(super) fn toggle_create_run_setup(&mut self, cx: &mut Context<Self>) {
        self.worktree_run_setup = !self.worktree_run_setup;
        cx.notify();
    }

    // ── rendering ──────────────────────────────────────────────────────

    /// Shared top-bar leading controls (Back + title) while the page is open.
    pub(super) fn worktrees_top_bar_leading(
        &self,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                press(
                    button_frame(div().id("worktrees-top-back"), &theme, ButtonSize::Medium)
                        .group(BUTTON_GROUP)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.surface_hover)),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| this.close_worktrees(cx)),
                )
                .child(icon(
                    "icons/arrow-left.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.text_secondary,
                ))
                .child(
                    div()
                        .text_color(theme.text_secondary)
                        .child(tr!("view.back")),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(icon(
                        "icons/branch.svg",
                        IconSize::Medium.px(&theme),
                        theme.text_secondary,
                    ))
                    .child(
                        div()
                            .text_size(TextSize::Large.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("worktree.page.title")),
                    ),
            )
            .into_any_element()
    }

    /// The full-page Worktrees surface.
    pub(super) fn render_worktrees_page(&self, cx: &Context<Self>) -> AnyElement {
        let theme = *theme::get(cx);
        let this = cx.entity();
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text_primary)
            .font_family(theme::ui_font_family())
            .child(self.worktrees_header(theme, this.clone(), cx))
            .child(
                div()
                    .id("worktrees-body")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            if this.worktree_menu.is_some() || this.worktree_advanced_open {
                                this.worktree_advanced_open = false;
                                this.dismiss_worktree_menu(cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(PAGE_MAX_W))
                            .mx_auto()
                            .px(DynamicSpacing::Base20.px(&theme))
                            .py(DynamicSpacing::Base20.px(&theme))
                            .debug_selector(|| "worktree-page-content".to_string())
                            .flex()
                            .flex_col()
                            .gap(DynamicSpacing::Base16.px(&theme))
                            .children(self.worktree_setup_banner(theme, this.clone(), cx))
                            .children(self.worktrees_error_banner(theme, this.clone()))
                            .children(self.worktree_howto_card(theme, this.clone()))
                            .child(
                                div()
                                    .debug_selector(|| "worktree-page-list".to_string())
                                    .child(self.worktrees_list(theme, this.clone(), cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Page header: repository context on the left, actions on the right.
    fn worktrees_header(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let repo_label = self.worktree_repo_label();
        let busy = self.worktree_operation.clone();
        let mut right = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme));
        if let Some(label) = busy {
            right = right.child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(spinner(
                        "worktree-op-spinner",
                        IconSize::Small.px(&theme),
                        theme.text_muted,
                        theme,
                    ))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(label),
                    ),
            );
        } else {
            right = right.child(worktree_icon_button(
                "worktrees-refresh",
                "icons/refresh.svg",
                theme,
                this.clone(),
                |app, cx| app.refresh_worktrees(cx),
            ));
        }
        let advanced = self.worktree_advanced_control(theme, this.clone(), cx);
        right = right.child(advanced).child(
            button_frame(div().id("worktrees-new"), &theme, ButtonSize::Medium)
                .border_1()
                .border_color(theme.border)
                .raised(theme.surface_elevated, &theme)
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|s| s.raised(theme.surface_hover, &theme))
                .child(icon(
                    "icons/plus.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.accent_text,
                ))
                .child(
                    div()
                        .text_color(theme.text_primary)
                        .child(tr!("worktree.page.new")),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.open_create_worktree_dialog(cx)
                    }),
                ),
        );

        div()
            .h(px(44.))
            .flex_none()
            .pl(DynamicSpacing::Base20.px(&theme))
            .pr(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(repo_label),
            )
            .child(right)
            .into_any_element()
    }

    /// The Advanced popover: prune stale worktrees and repair administrative
    /// files, kept off the main surface (§19).
    fn worktree_advanced_control(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open = self.worktree_advanced_open;
        let mut control = div().relative();
        if open {
            let popup_this = this.clone();
            control = control.child(
                div()
                    .absolute()
                    .top(px(0.))
                    .right(px(0.))
                    .size(px(0.))
                    .child(
                        // Anchor to the button's top-right (via a zero-size
                        // point) and drop the menu `MENU_OFFSET` below it,
                        // right-aligned — the same pattern as Settings' select
                        // and the Git "…" menu. `Window` mode (not `Local` +
                        // `snap_to_window`) is deliberate: that combo cannot
                        // flip, so in this top toolbar the off-screen "open
                        // above" position made the snap slide the menu down
                        // over its own button.
                        anchored()
                            .position_mode(AnchoredPositionMode::Window)
                            .anchor(Corner::TopRight)
                            .offset(point(
                                px(0.),
                                ButtonSize::Medium.height(&theme) + popover::MENU_OFFSET,
                            ))
                            .child(deferred(
                                picker_surface(div(), &theme)
                                    .debug_selector(|| "worktrees-advanced-popup".to_string())
                                    .w(px(240.))
                                    .py(picker::list_padding_y(&theme))
                                    .occlude()
                                    .on_mouse_down_out({
                                        let this = popup_this.clone();
                                        move |_, _, cx| {
                                            this.update(cx, |app, cx| {
                                                app.worktree_advanced_open = false;
                                                // Stamp the dismissal so the
                                                // button's mouse-up (the same
                                                // click) cannot toggle it back
                                                // open.
                                                app.menu_dismissed_at = Some(Instant::now());
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .child(worktree_menu_row(
                                        "wa-prune",
                                        "icons/trash.svg",
                                        tr!("worktree.advanced.prune"),
                                        theme,
                                        this.clone(),
                                        |app, cx| app.prune_worktrees(cx),
                                    ))
                                    .child(worktree_menu_row(
                                        "wa-repair",
                                        "icons/folder-sync.svg",
                                        tr!("worktree.advanced.repair"),
                                        theme,
                                        this,
                                        |app, cx| app.repair_worktrees(cx),
                                    )),
                            )),
                    ),
            );
        }
        control = control.child(
            press(
                button_frame(div().id("worktrees-advanced"), &theme, ButtonSize::Medium)
                    .debug_selector(|| "worktrees-advanced".to_string())
                    .border_1()
                    .border_color(theme.border)
                    .raised(
                        if open {
                            theme.surface_hover
                        } else {
                            theme.surface_elevated
                        },
                        &theme,
                    )
                    .cursor_pointer()
                    .hover(|s| s.raised(theme.surface_hover, &theme))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("worktree.page.advanced")),
                    )
                    .child(icon(
                        "icons/chevron-down.svg",
                        IconSize::XSmall.px(&theme),
                        theme.text_muted,
                    )),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.toggle_worktree_advanced(cx);
                }),
            ),
        );
        control.into_any_element()
    }

    /// The setup progress / result banner.
    fn worktree_setup_banner(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.worktree_setup.as_ref()?;
        let name = state
            .worktree
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut card = div()
            .w_full()
            .min_w_0()
            .bg(theme.surface_secondary)
            .border_1()
            .border_color(theme.border)
            .rounded(Radius::XLarge.px(&theme))
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme));

        let (glyph, glyph_color, title) = match &state.outcome {
            None => (
                "icons/loader.svg",
                theme.text_muted,
                tr!("worktree.setup.running"),
            ),
            Some(outcome) if outcome.success => (
                "icons/check.svg",
                theme.success,
                tr!("worktree.setup.succeeded"),
            ),
            Some(_) => (
                "icons/circle-x.svg",
                theme.error,
                tr!("worktree.setup.failed"),
            ),
        };
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(if state.outcome.is_none() {
                    spinner(
                        "worktree-setup-spin",
                        IconSize::Medium.px(&theme),
                        glyph_color,
                        theme,
                    )
                } else {
                    icon(glyph, IconSize::Medium.px(&theme), glyph_color).into_any_element()
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base02.px(&theme))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(tr!(
                                    "worktree.setup.script_label",
                                    name = name.clone(),
                                    script = state.script.to_string_lossy()
                                )),
                        ),
                ),
        );
        if let Some(outcome) = &state.outcome {
            if !outcome.success {
                card = card.child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(outcome.summary()),
                );
            }
            let mut actions = div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(worktree_text_button(
                    "worktree-setup-toggle-output",
                    if state.expanded {
                        tr!("worktree.setup.hide_output")
                    } else {
                        tr!("worktree.setup.show_output")
                    },
                    theme,
                    this.clone(),
                    |app, cx| app.toggle_worktree_setup_output(cx),
                ));
            if !outcome.success {
                actions = actions.child(worktree_text_button(
                    "worktree-setup-retry",
                    tr!("worktree.setup.retry"),
                    theme,
                    this.clone(),
                    |app, cx| app.retry_worktree_setup(cx),
                ));
            }
            let path = state.worktree.clone();
            actions = actions.child(worktree_text_button(
                "worktree-setup-open",
                tr!("worktree.setup.open_worktree"),
                theme,
                this.clone(),
                move |app, cx| app.open_worktree(&path, cx),
            ));
            actions = actions.child(worktree_text_button(
                "worktree-setup-dismiss",
                tr!("common.dismiss"),
                theme,
                this.clone(),
                |app, cx| app.dismiss_worktree_setup(cx),
            ));
            card = card.child(actions);
            if state.expanded {
                let output = outcome.output();
                card = card.child(
                    div()
                        .id("worktree-setup-output")
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(Radius::Medium.px(&theme))
                        .p(DynamicSpacing::Base08.px(&theme))
                        .font_family(theme::code_font_family())
                        .text_size(theme.code_px(11.))
                        .text_color(theme.text_secondary)
                        .child(if output.trim().is_empty() {
                            tr!("worktree.setup.no_output")
                        } else {
                            output
                        }),
                );
            }
        }
        Some(card.into_any_element())
    }

    /// The page-level error banner, with the raw Git text preserved.
    fn worktrees_error_banner(&self, theme: Theme, this: Entity<OrbitApp>) -> Option<AnyElement> {
        let error = self.worktrees_error.clone()?;
        Some(
            div()
                .w_full()
                .min_w_0()
                .bg(theme.error.opacity(0.08))
                .border_1()
                .border_color(theme.error.opacity(0.35))
                .rounded(Radius::XLarge.px(&theme))
                .px(DynamicSpacing::Base16.px(&theme))
                .py(DynamicSpacing::Base12.px(&theme))
                .flex()
                .items_start()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(icon(
                    "icons/circle-x.svg",
                    IconSize::Medium.px(&theme),
                    theme.error,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(theme::code_font_family())
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_primary)
                        .child(error),
                )
                .child(worktree_text_button(
                    "worktrees-error-dismiss",
                    tr!("common.dismiss"),
                    theme,
                    this,
                    |app, cx| {
                        app.worktrees_error = None;
                        cx.notify();
                    },
                ))
                .into_any_element(),
        )
    }

    /// A dismissible primer shown above the list: what a worktree is and how
    /// the pieces fit together. Once dismissed it stays gone (persisted with
    /// the other one-time hints), so the page then shows only the list.
    pub(super) fn worktree_howto_card(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> Option<AnyElement> {
        if self.worktree_howto_dismissed {
            return None;
        }
        let dismiss = button_frame(
            div().id("worktrees-howto-dismiss"),
            &theme,
            ButtonSize::Default,
        )
        .rounded(Radius::Small.px(&theme))
        .cursor_pointer()
        .hover(|s| s.bg(theme.overlay_strong))
        .tip(tr!("common.dismiss"))
        .child(icon(
            "icons/x.svg",
            ButtonSize::Default.icon_size().px(&theme),
            theme.text_muted,
        ))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| {
                app.worktree_howto_dismissed = true;
                crate::transcript::persist_hint(WORKTREE_HOWTO_HINT_KEY);
                cx.notify();
            });
        });
        Some(
            div()
                .debug_selector(|| "worktree-howto-card".to_string())
                .w_full()
                .min_w_0()
                .bg(theme.surface_secondary)
                .border_1()
                .border_color(theme.border)
                .rounded(Radius::XLarge.px(&theme))
                .px(DynamicSpacing::Base16.px(&theme))
                .py(DynamicSpacing::Base12.px(&theme))
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .child(icon(
                            "icons/info.svg",
                            IconSize::Small.px(&theme),
                            theme.text_muted,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_weight(FontWeight::MEDIUM)
                                .child(tr!("worktree.howto.title")),
                        )
                        .child(dismiss),
                )
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(tr!("worktree.howto.intro")),
                )
                .child(worktree_howto_line(theme, tr!("worktree.howto.create")))
                .child(worktree_howto_line(theme, tr!("worktree.howto.open")))
                .child(worktree_howto_line(theme, tr!("worktree.howto.setup")))
                .child(worktree_howto_line(theme, tr!("worktree.howto.rename")))
                .into_any_element(),
        )
    }

    /// The list of worktrees for the active repository.
    fn worktrees_list(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        if self.worktrees.is_empty() {
            rows.push(worktree_empty_state(
                theme,
                self.worktrees_busy,
                self.worktrees_error.is_none(),
            ));
        } else {
            for worktree in &self.worktrees {
                rows.push(self.worktree_row(worktree, theme, this.clone(), cx));
            }
        }
        self.settings_section(theme, &tr!("worktree.page.section"), rows)
    }

    /// One worktree row: a status marker, the name, its branch, its path, and
    /// the row-actions menu.
    fn worktree_row(
        &self,
        worktree: &Worktree,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let active = self.worktree_is_active(&worktree.path);
        let name = worktree.name.clone();
        let path = worktree.path.clone();
        let branch = worktree.branch_label();
        let menu_open = self
            .worktree_menu
            .as_ref()
            .filter(|menu| worktree::same_path(&menu.path, &worktree.path))
            .cloned();

        div()
            .id(ElementId::Name(
                format!("worktree-row-{}", path.to_string_lossy()).into(),
            ))
            .group("worktree-row")
            .relative()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .cursor_pointer()
            .when(active, |row| row.bg(theme.selection))
            .hover(|row| row.bg(theme.surface_hover))
            .on_mouse_down(MouseButton::Right, {
                let this = this.clone();
                let path = path.clone();
                move |event: &MouseDownEvent, _, cx| {
                    this.update(cx, |app, cx| {
                        app.toggle_worktree_menu(path.clone(), Some(event.position), cx)
                    });
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                let path = path.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| app.open_worktree(&path, cx));
                }
            })
            .child(worktree_markers(worktree, theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base02.px(&theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base06.px(&theme))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .child(name.clone()),
                            )
                            .when(active, |row| {
                                row.child(
                                    div()
                                        .px(DynamicSpacing::Base04.px(&theme))
                                        .rounded(Radius::Small.px(&theme))
                                        .bg(theme.accent.opacity(0.14))
                                        .text_size(TextSize::XSmall.px(&theme))
                                        .text_color(theme.accent_text)
                                        .child(tr!("worktree.row.active")),
                                )
                            })
                            .when(worktree.bare, |row| {
                                row.child(
                                    div()
                                        .text_size(TextSize::XSmall.px(&theme))
                                        .text_color(theme.text_muted)
                                        .child(tr!("worktree.bare")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base06.px(&theme))
                            .child(icon(
                                "icons/branch.svg",
                                IconSize::XSmall.px(&theme),
                                theme.text_muted,
                            ))
                            .child(
                                div()
                                    .font_family(theme::code_font_family())
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_secondary)
                                    .child(branch),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(theme::code_font_family())
                            .text_size(TextSize::XSmall.px(&theme))
                            .text_color(theme.text_muted)
                            .child(shorten_home(&path)),
                    ),
            )
            .child({
                let button_this = this.clone();
                let menu_this = this.clone();
                let path_for_menu = path.clone();
                let row_menu = menu_open.clone();
                icon_button_frame(
                    div().id(ElementId::Name(
                        format!("worktree-more-{}", path.to_string_lossy()).into(),
                    )),
                    &theme,
                    ButtonSize::Compact,
                )
                .group(BUTTON_GROUP)
                .relative()
                .tip(tr!("sidebar.more_actions"))
                .cursor_pointer()
                .opacity(if row_menu.is_some() { 1.0 } else { 0.0 })
                .group_hover("worktree-row", |style| style.opacity(1.))
                .active(|style| style.opacity(PRESS_DIM))
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    button_this.update(cx, |app, cx| {
                        app.toggle_worktree_menu(path_for_menu.clone(), None, cx)
                    });
                })
                .child(icon(
                    "icons/more.svg",
                    ButtonSize::Compact.icon_size().px(&theme),
                    theme.text_muted,
                ))
                .children(row_menu.map(|menu| worktree_row_menu(&menu, worktree, theme, menu_this)))
            })
            .into_any_element()
    }

    /// The Worktrees dialog layer (create / permission / rename / move /
    /// remove / branch-in-use), rendered above the app's other surfaces.
    pub(super) fn worktree_dialog_layer(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.worktree_dialog.as_ref()?;
        let body: AnyElement = match dialog {
            WorktreeDialog::Create => self.create_worktree_dialog_body(theme, this.clone(), cx),
            WorktreeDialog::AllowSetup(pending) => {
                self.allow_setup_dialog_body(pending, theme, this.clone())
            }
            WorktreeDialog::Rename { path } => {
                self.rename_worktree_dialog_body(path, theme, this.clone())
            }
            WorktreeDialog::Move { path } => {
                self.move_worktree_dialog_body(path, theme, this.clone())
            }
            WorktreeDialog::Remove { path, dirty } => {
                self.remove_worktree_dialog_body(path, *dirty, theme, this.clone())
            }
            WorktreeDialog::BranchInUse { branch, path } => {
                self.branch_in_use_dialog_body(branch, path, theme, this.clone())
            }
            WorktreeDialog::Unavailable { path } => {
                self.worktree_unavailable_dialog_body(path, theme, this.clone())
            }
        };
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
                .id("worktree-dialog-layer")
                .absolute()
                .inset_0()
                .occlude()
                .bg(scrim)
                .p(DynamicSpacing::Base24.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(Self::on_worktree_dialog_scrim),
                )
                .child(body)
                .into_any_element(),
        )
    }

    /// The shared dialog card frame.
    fn worktree_dialog_card(&self, title: String, theme: Theme, body: AnyElement) -> AnyElement {
        div()
            .w(px(520.))
            .max_h(px(560.))
            .bg(theme.surface_secondary)
            .border_1()
            .border_color(theme.border)
            .rounded(Radius::XLarge.px(&theme))
            .flex()
            .flex_col()
            .overflow_hidden()
            // The card's own hitbox blocks the scrim below it, so a click on
            // any field or button never reaches the dismiss handler.
            .occlude()
            .child(
                div()
                    .px(modal::header_padding_x(&theme))
                    .pt(modal::header_padding_top(&theme))
                    .pb(modal::header_padding_bottom(&theme))
                    .child(
                        div()
                            .text_size(TextSize::Large.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .id("worktree-dialog-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(modal::header_padding_x(&theme))
                    .pb(modal::header_padding_bottom(&theme))
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base12.px(&theme))
                    .child(body),
            )
            .into_any_element()
    }

    fn worktree_dialog_error(&self, theme: Theme) -> Option<AnyElement> {
        let error = self.worktree_dialog_error.clone()?;
        Some(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.error)
                .child(error)
                .into_any_element(),
        )
    }

    /// Create dialog body.
    fn create_worktree_dialog_body(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let name_field = worktree_field(
            &theme,
            tr!("worktree.field.name"),
            Some(&tr!("worktree.field.name_hint")),
            input_field_frame(div(), &theme)
                .w_full()
                .bg(theme.background)
                .child(self.worktree_name_input.clone()),
        );
        let mode = self.worktree_branch_mode;
        let mut branch_section = div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(worktree_field_label(
                &theme,
                tr!("worktree.field.branch"),
                Some(&tr!("worktree.field.branch_hint")),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .child(worktree_mode_chip(
                        "worktree-mode-new",
                        tr!("worktree.field.branch_new"),
                        mode == WorktreeBranchMode::New,
                        theme,
                        this.clone(),
                        |app, cx| {
                            app.worktree_branch_mode = WorktreeBranchMode::New;
                            cx.notify();
                        },
                    ))
                    .child(worktree_mode_chip(
                        "worktree-mode-existing",
                        tr!("worktree.field.branch_existing"),
                        mode == WorktreeBranchMode::Existing,
                        theme,
                        this.clone(),
                        |app, cx| {
                            app.worktree_branch_mode = WorktreeBranchMode::Existing;
                            cx.notify();
                        },
                    )),
            );
        if mode == WorktreeBranchMode::New {
            branch_section = branch_section
                .child(
                    input_field_frame(div(), &theme)
                        .w_full()
                        .bg(theme.background)
                        .child(self.worktree_branch_input.clone()),
                )
                .child(
                    input_field_frame(div(), &theme)
                        .w_full()
                        .bg(theme.background)
                        .child(self.worktree_start_input.clone()),
                )
                .child(
                    div()
                        .text_size(TextSize::XSmall.px(&theme))
                        .text_color(theme.text_muted)
                        .child(tr!("worktree.field.start_hint")),
                );
        } else if self.worktree_branches.is_empty() {
            branch_section = branch_section.child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!("worktree.field.loading_branches")),
            );
        } else {
            let mut list = div()
                .id("worktree-branch-list")
                .max_h(px(160.))
                .overflow_y_scroll()
                .border_1()
                .border_color(theme.border)
                .rounded(Radius::Large.px(&theme))
                .flex()
                .flex_col();
            for branch in &self.worktree_branches {
                let selected = self.worktree_branch_choice.as_deref() == Some(branch.as_str());
                let branch_click = branch.clone();
                let row_this = this.clone();
                list = list.child(
                    div()
                        .id(ElementId::Name(format!("worktree-branch-{branch}").into()))
                        .w_full()
                        .px(DynamicSpacing::Base12.px(&theme))
                        .py(DynamicSpacing::Base06.px(&theme))
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .cursor_pointer()
                        .when(selected, |row| row.bg(theme.selection))
                        .hover(|row| row.bg(theme.surface_hover))
                        .child(icon(
                            if selected {
                                "icons/check.svg"
                            } else {
                                "icons/branch.svg"
                            },
                            IconSize::Small.px(&theme),
                            if selected {
                                theme.accent_text
                            } else {
                                theme.text_muted
                            },
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(theme::code_font_family())
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(if selected {
                                    theme.text_primary
                                } else {
                                    theme.text_secondary
                                })
                                .child(branch.clone()),
                        )
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            row_this.update(cx, |app, cx| {
                                app.worktree_branch_choice = Some(branch_click.clone());
                                cx.notify();
                            });
                        }),
                );
            }
            branch_section = branch_section.child(list);
        }

        // Location preview: `<root>/<name>`, computed from the cached root so
        // no Git runs while typing.
        let location = match (
            self.worktree_root_hint(),
            self.worktree_name_input.read(cx).text(),
        ) {
            (Some(root), name) if !name.trim().is_empty() => {
                shorten_home(&worktree::expand_path(&root, name.trim()))
            }
            (Some(root), _) => shorten_home(&root),
            _ => String::new(),
        };
        let location_row = worktree_field(
            &theme,
            tr!("worktree.field.location"),
            None,
            div()
                .w_full()
                .px(DynamicSpacing::Base12.px(&theme))
                .py(DynamicSpacing::Base08.px(&theme))
                .font_family(theme::code_font_family())
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(if location.is_empty() {
                    tr!("worktree.field.location_unknown")
                } else {
                    location
                })
                .into_any_element(),
        );

        let script = self
            .worktree_root_hint()
            .and_then(|root| self.worktree_config.effective_setup_script(&root));
        let setup_row = script.map(|script| {
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(self.settings_toggle(
                    "worktree-create-run-setup",
                    self.worktree_run_setup,
                    theme,
                    this.clone(),
                    Self::toggle_create_run_setup,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base02.px(&theme))
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_secondary)
                                .child(tr!("worktree.field.run_setup")),
                        )
                        .child(
                            div()
                                .truncate()
                                .font_family(theme::code_font_family())
                                .text_size(TextSize::XSmall.px(&theme))
                                .text_color(theme.text_muted)
                                .child(shorten_home(&script)),
                        ),
                )
                .into_any_element()
        });

        let footer = worktree_dialog_footer(
            theme,
            tr!("view.cancel"),
            tr!("worktree.dialog.create"),
            this,
            |app, cx| app.cancel_worktree_dialog(cx),
            |app, cx| app.confirm_worktree_dialog(cx),
        );
        self.worktree_dialog_card(
            tr!("worktree.dialog.create_title"),
            theme,
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(name_field)
                .child(branch_section)
                .child(location_row)
                .children(setup_row)
                .children(self.worktree_dialog_error(theme))
                .child(footer)
                .into_any_element(),
        )
    }

    /// Setup-script permission dialog (§33).
    fn allow_setup_dialog_body(
        &self,
        pending: &PendingWorktreeCreate,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let script = self
            .worktree_root_hint()
            .and_then(|root| self.worktree_config.effective_setup_script(&root));
        let text = tr!(
            "worktree.allow_setup.body",
            name = pending.name.clone(),
            script = script
                .as_deref()
                .map(shorten_home)
                .unwrap_or_else(|| tr!("worktree.allow_setup.unknown_script"))
        );
        let footer = worktree_dialog_footer(
            theme,
            tr!("view.cancel"),
            tr!("worktree.allow_setup.allow"),
            this,
            |app, cx| app.cancel_worktree_dialog(cx),
            |app, cx| app.confirm_worktree_dialog(cx),
        );
        self.worktree_dialog_card(
            tr!("worktree.allow_setup.title"),
            theme,
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(text),
                )
                .child(footer)
                .into_any_element(),
        )
    }

    fn rename_worktree_dialog_body(
        &self,
        path: &Path,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let current = self.worktree_display_name(path);
        let field = worktree_field(
            &theme,
            tr!("worktree.field.new_name"),
            Some(&tr!("worktree.field.rename_hint")),
            input_field_frame(div(), &theme)
                .w_full()
                .bg(theme.background)
                .child(self.worktree_field_input.clone()),
        );
        let footer = worktree_dialog_footer(
            theme,
            tr!("view.cancel"),
            tr!("worktree.dialog.rename"),
            this,
            |app, cx| app.cancel_worktree_dialog(cx),
            |app, cx| app.confirm_worktree_dialog(cx),
        );
        self.worktree_dialog_card(
            tr!("worktree.dialog.rename_title", name = current),
            theme,
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(field)
                .children(self.worktree_dialog_error(theme))
                .child(footer)
                .into_any_element(),
        )
    }

    fn move_worktree_dialog_body(
        &self,
        path: &Path,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let current = self.worktree_display_name(path);
        let field = worktree_field(
            &theme,
            tr!("worktree.field.destination"),
            Some(&tr!("worktree.field.move_hint")),
            input_field_frame(div(), &theme)
                .w_full()
                .bg(theme.background)
                .child(self.worktree_field_input.clone()),
        );
        let footer = worktree_dialog_footer(
            theme,
            tr!("view.cancel"),
            tr!("worktree.dialog.move"),
            this,
            |app, cx| app.cancel_worktree_dialog(cx),
            |app, cx| app.confirm_worktree_dialog(cx),
        );
        self.worktree_dialog_card(
            tr!("worktree.dialog.move_title", name = current),
            theme,
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(field)
                .children(self.worktree_dialog_error(theme))
                .child(footer)
                .into_any_element(),
        )
    }

    fn remove_worktree_dialog_body(
        &self,
        path: &Path,
        dirty: bool,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let worktree = self.listed_worktree(path);
        let name = worktree
            .map(|worktree| worktree.name.clone())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let branch = worktree.map(Worktree::branch_label).unwrap_or_default();
        let body = if dirty {
            tr!("worktree.remove.dirty_body")
        } else {
            tr!("worktree.remove.clean_body")
        };
        let confirm_label = if dirty {
            tr!("worktree.remove.remove_anyway")
        } else {
            tr!("worktree.remove.remove")
        };
        let footer = worktree_dialog_footer(
            theme,
            tr!("view.cancel"),
            confirm_label,
            this,
            |app, cx| app.cancel_worktree_dialog(cx),
            |app, cx| app.confirm_worktree_dialog(cx),
        );
        self.worktree_dialog_card(
            tr!("worktree.remove.title"),
            theme,
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base02.px(&theme))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(name),
                        )
                        .child(
                            div()
                                .font_family(theme::code_font_family())
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .child(branch),
                        )
                        .child(
                            div()
                                .truncate()
                                .font_family(theme::code_font_family())
                                .text_size(TextSize::XSmall.px(&theme))
                                .text_color(theme.text_muted)
                                .child(shorten_home(path)),
                        ),
                )
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(if dirty {
                            theme.error
                        } else {
                            theme.text_secondary
                        })
                        .child(body),
                )
                .child(footer)
                .into_any_element(),
        )
    }

    /// The session's workspace is gone; show where it pointed and offer the
    /// Worktrees page. Nothing is re-pointed automatically (§22).
    fn worktree_unavailable_dialog_body(
        &self,
        path: &Path,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let footer = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(modal::footer_gap(&theme))
            .border_t_1()
            .border_color(theme.border)
            .pt(modal::footer_padding(&theme))
            .child(worktree_text_button(
                "worktree-unavailable-dismiss",
                tr!("common.dismiss"),
                theme,
                this.clone(),
                |app, cx| app.cancel_worktree_dialog(cx),
            ))
            .child(worktree_primary_button(
                "worktree-unavailable-open",
                tr!("worktree.unavailable.locate"),
                theme,
                this,
                |app, cx| {
                    app.worktree_dialog = None;
                    app.open_worktrees(cx);
                },
            ));
        self.worktree_dialog_card(
            tr!("worktree.unavailable.title"),
            theme,
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(tr!("worktree.unavailable.body")),
                )
                .child(
                    div()
                        .truncate()
                        .font_family(theme::code_font_family())
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(shorten_home(path)),
                )
                .child(footer)
                .into_any_element(),
        )
    }

    fn branch_in_use_dialog_body(
        &self,
        branch: &str,
        path: &Path,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let path = path.to_path_buf();
        let footer = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(modal::footer_gap(&theme))
            .border_t_1()
            .border_color(theme.border)
            .pt(modal::footer_padding(&theme))
            .child(worktree_text_button(
                "worktree-bi-cancel",
                tr!("view.cancel"),
                theme,
                this.clone(),
                |app, cx| app.cancel_worktree_dialog(cx),
            ))
            .child(worktree_primary_button(
                "worktree-bi-open",
                tr!("worktree.branch_in_use.open"),
                theme,
                this,
                move |app, cx| {
                    app.worktree_dialog = None;
                    app.open_worktree(&path, cx);
                },
            ));
        self.worktree_dialog_card(
            tr!("worktree.branch_in_use.title"),
            theme,
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(
                    div()
                        .font_family(theme::code_font_family())
                        .text_color(theme.text_primary)
                        .child(branch.to_string()),
                )
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(tr!("worktree.branch_in_use.body")),
                )
                .child(footer)
                .into_any_element(),
        )
    }
}

// ── "Work in" picker (status bar) ─────────────────────────────────────

/// One row of the status bar's "Work in" picker.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkInRow {
    /// The repository's main working directory.
    Local,
    /// A linked worktree, by its index in `OrbitApp::worktrees`.
    Worktree(usize),
    /// Create a new worktree (opens the create dialog).
    New,
}

impl OrbitApp {
    /// The "Work in" rows for the active repository: Local, then every linked
    /// worktree, then New. `None` when the active workspace is not a Git
    /// repository, so the control hides and non-worktree projects are
    /// unchanged (§39).
    fn work_in_rows(&self) -> Option<Vec<WorkInRow>> {
        if self.worktrees.is_empty() {
            return None;
        }
        let mut rows = vec![WorkInRow::Local];
        for (ix, worktree) in self.worktrees.iter().enumerate() {
            if !worktree.is_main {
                rows.push(WorkInRow::Worktree(ix));
            }
        }
        rows.push(WorkInRow::New);
        Some(rows)
    }

    /// The main worktree path, when one is known.
    fn main_worktree_path(&self) -> Option<PathBuf> {
        self.worktrees
            .iter()
            .find(|worktree| worktree.is_main)
            .map(|worktree| worktree.path.clone())
    }

    /// Whether the active workspace is the repository's main working
    /// directory (or a path inside it).
    fn work_in_is_local(&self) -> bool {
        let (Some(active), Some(main)) =
            (self.current_workspace.as_deref(), self.main_worktree_path())
        else {
            return false;
        };
        worktree::same_path(active, &main) || active.starts_with(&main)
    }

    /// The chip label: `Local` at the main worktree, the worktree's name in a
    /// linked one. `None` when there is no repository.
    fn work_in_label(&self) -> Option<String> {
        if self.worktrees.is_empty() {
            return None;
        }
        if self.work_in_is_local() {
            return Some(tr!("worktree.work_in.local"));
        }
        let name = self.current_workspace.as_deref().and_then(|active| {
            self.worktrees
                .iter()
                .find(|worktree| worktree::same_path(&worktree.path, active))
                .filter(|worktree| !worktree.is_main)
        });
        Some(match name {
            Some(worktree) => worktree.name.clone(),
            None => tr!("worktree.work_in.local"),
        })
    }

    /// Index of the row representing the active workspace, for the initial
    /// highlight.
    fn work_in_selected_ix(&self, rows: &[WorkInRow]) -> usize {
        rows.iter()
            .position(|row| match row {
                WorkInRow::Local => self.work_in_is_local(),
                WorkInRow::Worktree(ix) => self
                    .worktrees
                    .get(*ix)
                    .is_some_and(|worktree| self.worktree_is_active(&worktree.path)),
                WorkInRow::New => false,
            })
            .unwrap_or(0)
    }

    /// Toggle the "Work in" picker. Mutually exclusive with the composer and
    /// status-bar popovers it would overlap.
    pub(super) fn toggle_work_in_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.work_in_menu_open {
            self.close_work_in_menu(window, cx);
            return;
        }
        self.branch_picker = None;
        self.model_selector = None;
        self.context_popup = ContextPopup::None;
        self.access_menu_open = false;
        self.workflow_menu_open = false;
        self.add_menu_open = false;
        self.open_in_menu_open = false;
        self.worktree_menu = None;
        let rows = self.work_in_rows().unwrap_or_default();
        self.work_in_menu_highlight = self.work_in_selected_ix(&rows);
        self.work_in_menu_open = true;
        window.focus(&self.work_in_menu_focus);
        cx.notify();
    }

    pub(super) fn close_work_in_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.work_in_menu_open {
            self.work_in_menu_open = false;
            self.input.read(cx).focus(window);
            cx.notify();
        }
    }

    pub(super) fn on_work_in_trigger_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The click's mouse-down dismisses any open menu; without this guard
        // the same click's mouse-up would toggle it straight back open.
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.toggle_work_in_menu(window, cx);
    }

    pub(super) fn on_work_in_next(
        &mut self,
        _: &crate::WorkInMenuNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.work_in_rows().map_or(0, |rows| rows.len());
        if count > 0 {
            self.work_in_menu_highlight = (self.work_in_menu_highlight + 1) % count;
        }
        cx.notify();
    }

    pub(super) fn on_work_in_prev(
        &mut self,
        _: &crate::WorkInMenuPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.work_in_rows().map_or(0, |rows| rows.len());
        if count > 0 {
            self.work_in_menu_highlight = (self.work_in_menu_highlight + count - 1) % count;
        }
        cx.notify();
    }

    pub(super) fn on_work_in_confirm(
        &mut self,
        _: &crate::WorkInMenuConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = self.work_in_menu_highlight;
        self.run_work_in_item(ix, window, cx);
    }

    pub(super) fn on_work_in_close(
        &mut self,
        _: &crate::WorkInMenuClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_work_in_menu(window, cx);
    }

    /// Run row `ix` of the "Work in" picker.
    pub(super) fn run_work_in_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rows) = self.work_in_rows() else {
            self.close_work_in_menu(window, cx);
            return;
        };
        let Some(row) = rows.get(ix).cloned() else {
            self.close_work_in_menu(window, cx);
            return;
        };
        match row {
            WorkInRow::Local => {
                self.work_in_menu_open = false;
                if self.work_in_is_local() {
                    return;
                }
                if let Some(main) = self.main_worktree_path() {
                    self.open_worktree(&main, cx);
                }
            }
            WorkInRow::Worktree(worktree_ix) => {
                self.work_in_menu_open = false;
                if let Some(path) = self.worktrees.get(worktree_ix).map(|w| w.path.clone()) {
                    if !self.worktree_is_active(&path) {
                        self.open_worktree(&path, cx);
                    }
                }
            }
            WorkInRow::New => {
                self.work_in_menu_open = false;
                self.open_new_worktree_from_work_in(window, cx);
            }
        }
    }

    /// The quick "New worktree" flow: open the create dialog with a suggested
    /// name (fully editable — the custom-name option) and a new branch derived
    /// from it, focused so the name can be typed over immediately.
    pub(super) fn open_new_worktree_from_work_in(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_create_worktree_dialog(cx);
        let suggestion = self.suggest_worktree_name();
        self.worktree_name_input
            .update(cx, |input, cx| input.set_text(suggestion, cx));
        let handle = self.worktree_name_input.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    /// A free worktree name: the active branch slug, else `worktree`, with a
    /// numeric suffix until it is unused.
    fn suggest_worktree_name(&self) -> String {
        let base = self
            .branch
            .as_ref()
            .map(|branch| worktree::slugify(&branch.name))
            .filter(|slug| !slug.is_empty())
            .unwrap_or_else(|| "worktree".to_string());
        let root = self.worktree_root_hint();
        let mut candidate = base.clone();
        let mut n = 1;
        loop {
            let listed = self
                .worktrees
                .iter()
                .any(|worktree| worktree.name == candidate);
            let on_disk = root
                .as_deref()
                .map(|root| worktree::expand_path(root, &candidate).exists())
                .unwrap_or(false);
            if !listed && !on_disk {
                return candidate;
            }
            n += 1;
            candidate = format!("{base}-{n}");
        }
    }

    /// The status bar's "Work in" chip: the active location when it is a linked
    /// worktree, `None` at the repository's main checkout ("Local" is the
    /// default) and `None` when there is no repository at all.
    pub(super) fn work_in_chip(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let label = self.work_in_label()?;
        // "Local" is the default — the repository's main checkout — and the
        // default earns no pixels, the same rule the access chip follows. The
        // chip appears in a linked worktree, where it names a non-default
        // location and is the way back out. Nothing is lost by hiding it at the
        // main checkout: the Worktrees page creates and switches worktrees, and
        // it sits in the top bar's "…" menu with the other surfaces.
        if self.work_in_is_local() {
            return None;
        }
        let theme = *theme::get(cx);
        let open = self.work_in_menu_open;
        // Only reachable from a linked worktree, so the mark is the branch
        // glyph rather than the monitor the main checkout used to show.
        let icon_path = "icons/branch.svg";
        Some(
            div()
                .flex()
                .flex_col()
                .items_start()
                .children(self.work_in_popup(cx))
                .child(
                    button_frame(div().id("status-work-in"), &theme, ButtonSize::Default)
                        .cursor_pointer()
                        .when(!open, |chip| {
                            chip.hover(|s| {
                                s.bg(theme.surface_hover).text_color(theme.text_secondary)
                            })
                        })
                        .when(open, |chip| {
                            chip.bg(theme.surface_active).text_color(theme.text_primary)
                        })
                        .tip(tr!("worktree.work_in.title"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(Self::on_work_in_trigger_click),
                        )
                        .child(icon(
                            icon_path,
                            ButtonSize::Default.icon_size().px(&theme),
                            theme.text_muted,
                        ))
                        .child(
                            div()
                                .text_color(if open {
                                    theme.text_primary
                                } else {
                                    theme.text_muted
                                })
                                .child(label),
                        )
                        .child(Self::chip_caret(
                            open,
                            theme.text_primary,
                            "work-in-caret-turn",
                            cx,
                        )),
                )
                .into_any_element(),
        )
    }

    /// The "Work in" popover, while open: a titled list of Local / linked
    /// worktrees / New worktree, driven by the `WorkInMenu` key context.
    pub(super) fn work_in_popup(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.work_in_menu_open {
            return None;
        }
        let theme = *theme::get(cx);
        let rows = self.work_in_rows().unwrap_or_default();
        let selected_ix = self.work_in_selected_ix(&rows);
        let this = cx.weak_entity();
        let mut list = div().w_full().flex().flex_col();
        for (ix, row) in rows.iter().enumerate() {
            let highlighted = ix == self.work_in_menu_highlight;
            let selected = ix == selected_ix;
            let (icon_path, title, subtitle): (&'static str, String, Option<String>) = match row {
                WorkInRow::Local => (
                    "icons/monitor.svg",
                    tr!("worktree.work_in.local"),
                    Some(tr!("worktree.work_in.local_hint")),
                ),
                WorkInRow::Worktree(worktree_ix) => {
                    let worktree = self.worktrees.get(*worktree_ix);
                    (
                        "icons/branch.svg",
                        worktree.map(|w| w.name.clone()).unwrap_or_default(),
                        worktree.map(Worktree::branch_label),
                    )
                }
                WorkInRow::New => (
                    "icons/plus.svg",
                    tr!("worktree.work_in.new"),
                    Some(tr!("worktree.work_in.new_hint")),
                ),
            };
            let this_row = this.clone();
            let entry = div().id(ElementId::NamedInteger("work-in-row".into(), ix as u64));
            list = list.child(
                picker_entry(entry, &theme)
                    .cursor_pointer()
                    .when(selected || highlighted, |row| row.bg(theme.selection))
                    .when(!selected && !highlighted, |row| {
                        row.hover(|style| style.bg(theme.surface_hover))
                    })
                    .on_hover(move |hovered, _, cx| {
                        if *hovered {
                            this_row
                                .update(cx, |app, cx| {
                                    if app.work_in_menu_highlight != ix {
                                        app.work_in_menu_highlight = ix;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let this = this.clone();
                        move |_, window, cx| {
                            this.update(cx, |app, cx| app.run_work_in_item(ix, window, cx))
                                .ok();
                        }
                    })
                    .child(
                        div()
                            .flex_none()
                            .size(px(26.))
                            .rounded(Radius::Medium.px(&theme))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(if selected || highlighted {
                                theme.text_primary.opacity(0.14)
                            } else {
                                theme.overlay
                            })
                            .child(icon(
                                icon_path,
                                context_menu::ICON.px(&theme),
                                if selected {
                                    theme.accent_text
                                } else {
                                    theme.text_muted
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if selected || highlighted {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(title),
                            )
                            .children(subtitle.map(|subtitle| {
                                div()
                                    .mt(DynamicSpacing::Base02.px(&theme))
                                    .truncate()
                                    .font_family(theme::code_font_family())
                                    .text_size(picker::SECONDARY_TEXT.px(&theme))
                                    .text_color(if selected || highlighted {
                                        theme.text_primary.opacity(0.75)
                                    } else {
                                        theme.text_muted
                                    })
                                    .child(subtitle)
                            })),
                    )
                    .when(selected, |row| {
                        row.child(icon(
                            "icons/check.svg",
                            context_menu::ICON.px(&theme),
                            theme.accent_text,
                        ))
                    }),
            );
        }
        let popup = picker_surface(div(), &theme)
            .w(px(280.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .key_context("WorkInMenu")
            .track_focus(&self.work_in_menu_focus)
            .on_action(cx.listener(Self::on_work_in_next))
            .on_action(cx.listener(Self::on_work_in_prev))
            .on_action(cx.listener(Self::on_work_in_confirm))
            .on_action(cx.listener(Self::on_work_in_close))
            .on_mouse_down_out(cx.listener(|app, _, window, cx| {
                app.menu_dismissed_at = Some(Instant::now());
                app.close_work_in_menu(window, cx);
            }))
            .child(menu_header(tr!("worktree.work_in.title"), &theme))
            .child(
                div()
                    .py(picker::list_padding_y(&theme))
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base01.px(&theme))
                    .child(list),
            );
        Some(
            anchored()
                .position_mode(AnchoredPositionMode::Local)
                .anchor(Corner::BottomLeft)
                .offset(point(px(0.), -popover::MENU_OFFSET))
                .snap_to_window_with_margin(popover::WINDOW_MARGIN)
                .child(deferred(popup))
                .into_any_element(),
        )
    }
}

// ── render helpers ─────────────────────────────────────────────────────

/// One row's popup menu: open / reveal / copy, rename / move, lock / unlock,
/// and remove. The main worktree hides every mutating action.
fn worktree_row_menu(
    menu: &WorktreeMenu,
    worktree: &Worktree,
    theme: Theme,
    this: Entity<OrbitApp>,
) -> AnyElement {
    let path = worktree.path.clone();
    let open_path = path.clone();
    let reveal_path = path.clone();
    let copy_path = path.clone();
    let rename_path = path.clone();
    let move_path = path.clone();
    let lock_path = path.clone();
    let remove_path = path.clone();
    let mut body = div()
        .py(picker::list_padding_y(&theme))
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base01.px(&theme))
        .child(worktree_menu_row(
            "wm-open",
            "icons/arrow-up-right.svg",
            tr!("worktree.menu.open"),
            theme,
            this.clone(),
            move |app, cx| app.open_worktree(&open_path, cx),
        ))
        .child(worktree_menu_row(
            "wm-reveal",
            "icons/folder-open.svg",
            tr!("worktree.menu.reveal"),
            theme,
            this.clone(),
            move |app, cx| app.reveal_worktree(reveal_path.clone(), cx),
        ))
        .child(worktree_menu_row(
            "wm-copy",
            "icons/copy.svg",
            tr!("sidebar.copy_path"),
            theme,
            this.clone(),
            move |app, cx| app.copy_worktree_path(copy_path.clone(), cx),
        ));
    if !worktree.is_main {
        body = body
            .child(worktree_menu_row(
                "wm-rename",
                "icons/pencil.svg",
                tr!("worktree.menu.rename"),
                theme,
                this.clone(),
                move |app, cx| app.start_rename_worktree(rename_path.clone(), cx),
            ))
            .child(worktree_menu_row(
                "wm-move",
                "icons/arrow-expand.svg",
                tr!("worktree.menu.move"),
                theme,
                this.clone(),
                move |app, cx| app.start_move_worktree(move_path.clone(), cx),
            ));
        body = if worktree.locked {
            body.child(worktree_menu_row(
                "wm-unlock",
                "icons/lock.svg",
                tr!("worktree.menu.unlock"),
                theme,
                this.clone(),
                move |app, cx| app.unlock_worktree(lock_path.clone(), cx),
            ))
        } else {
            body.child(worktree_menu_row(
                "wm-lock",
                "icons/lock.svg",
                tr!("worktree.menu.lock"),
                theme,
                this.clone(),
                move |app, cx| app.lock_worktree(lock_path.clone(), cx),
            ))
        };
        body = body.child(worktree_menu_row(
            "wm-remove",
            "icons/trash.svg",
            tr!("worktree.menu.remove"),
            theme,
            this.clone(),
            move |app, cx| app.start_remove_worktree(remove_path.clone(), cx),
        ));
    }
    let popup = picker_surface(div(), &theme)
        .w(px(220.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .occlude()
        .on_mouse_down_out({
            let this = this.clone();
            move |_, _, cx| {
                this.update(cx, |app, cx| {
                    app.menu_dismissed_at = Some(Instant::now());
                    app.worktree_menu = None;
                    cx.notify();
                })
            }
        })
        .child(body);
    let anchor = if let Some(at) = menu.at {
        anchored().anchor(Corner::TopLeft).position(at)
    } else {
        anchored()
            .position_mode(AnchoredPositionMode::Local)
            .anchor(Corner::TopRight)
            .offset(point(
                px(0.),
                ButtonSize::Compact.height(&theme) + popover::MENU_OFFSET,
            ))
    };
    anchor
        .snap_to_window_with_margin(popover::WINDOW_MARGIN)
        .child(deferred(popup))
        .into_any_element()
}

/// The worktree state marker: locked / prunable / main / linked.
fn worktree_markers(worktree: &Worktree, theme: Theme) -> AnyElement {
    let mut row = div()
        .w(px(20.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center();
    if worktree.locked {
        row = row.child(icon(
            "icons/lock.svg",
            IconSize::Small.px(&theme),
            theme.text_secondary,
        ));
    } else if worktree.prunable {
        row = row.child(icon(
            "icons/circle-x.svg",
            IconSize::Small.px(&theme),
            theme.error,
        ));
    } else if worktree.is_main {
        row = row.child(div().size(px(8.)).rounded_full().bg(theme.accent));
    } else {
        row = row.child(
            div()
                .size(px(8.))
                .rounded_full()
                .border_1()
                .border_color(theme.text_muted),
        );
    }
    row.into_any_element()
}

/// One bullet in the "How worktrees work" card.
fn worktree_howto_line(theme: Theme, text: String) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(DynamicSpacing::Base08.px(&theme))
        .child(div().flex_none().text_color(theme.text_muted).child("•"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_secondary)
                .child(text),
        )
        .into_any_element()
}

/// Empty state for the worktrees list.
fn worktree_empty_state(theme: Theme, busy: bool, has_no_error: bool) -> AnyElement {
    div()
        .w_full()
        .py(DynamicSpacing::Base24.px(&theme))
        .flex()
        .flex_col()
        .items_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .child(if busy {
            spinner(
                "worktrees-empty-spin",
                IconSize::Medium.px(&theme),
                theme.text_muted,
                theme,
            )
        } else {
            icon(
                "icons/branch.svg",
                IconSize::Medium.px(&theme),
                theme.text_muted,
            )
            .into_any_element()
        })
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(if busy {
                    tr!("worktree.empty.loading")
                } else if has_no_error {
                    tr!("worktree.empty.title")
                } else {
                    tr!("worktree.empty.unavailable")
                }),
        )
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("worktree.empty.hint")),
        )
        .into_any_element()
}

/// A labelled field in a worktree dialog.
fn worktree_field(
    theme: &Theme,
    label: String,
    hint: Option<&str>,
    control: impl IntoElement,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(input::gap(theme))
        .child(worktree_field_label(theme, label, hint))
        .child(control)
        .into_any_element()
}

fn worktree_field_label(theme: &Theme, label: String, hint: Option<&str>) -> AnyElement {
    let mut column = div()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base02.px(theme))
        .child(
            input_label(label, theme)
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_secondary),
        );
    if let Some(hint) = hint {
        column = column.child(
            div()
                .text_size(TextSize::XSmall.px(theme))
                .text_color(theme.text_muted)
                .child(hint.to_string()),
        );
    }
    column.into_any_element()
}

/// A dialog footer: Cancel on the left, the primary action on the right.
fn worktree_dialog_footer<C, P>(
    theme: Theme,
    cancel_label: String,
    confirm_label: String,
    this: Entity<OrbitApp>,
    on_cancel: C,
    on_confirm: P,
) -> AnyElement
where
    C: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
    P: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
{
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(modal::footer_gap(&theme))
        .border_t_1()
        .border_color(theme.border)
        .pt(modal::footer_padding(&theme))
        .child(worktree_text_button(
            "worktree-dialog-cancel",
            cancel_label,
            theme,
            this.clone(),
            on_cancel,
        ))
        .child(worktree_primary_button(
            "worktree-dialog-confirm",
            confirm_label,
            theme,
            this,
            on_confirm,
        ))
        .into_any_element()
}

/// A quiet button in a worktree surface.
fn worktree_text_button<F>(
    id: &'static str,
    label: String,
    theme: Theme,
    this: Entity<OrbitApp>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
{
    button_frame(div().id(id), &theme, ButtonSize::Medium)
        .border_1()
        .border_color(theme.border)
        .raised(theme.surface_elevated, &theme)
        .cursor_pointer()
        .hover(|s| s.raised(theme.surface_hover, &theme))
        .text_color(theme.text_secondary)
        .child(label)
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| action(app, cx));
        })
        .into_any_element()
}

/// A quiet icon button (page refresh).
fn worktree_icon_button<F>(
    id: &'static str,
    icon_path: &'static str,
    theme: Theme,
    this: Entity<OrbitApp>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
{
    button_frame(div().id(id), &theme, ButtonSize::Medium)
        .border_1()
        .border_color(theme.border)
        .raised(theme.surface_elevated, &theme)
        .cursor_pointer()
        .hover(|s| s.raised(theme.surface_hover, &theme))
        .tip(tr!("common.refresh"))
        .child(icon(
            icon_path,
            ButtonSize::Medium.icon_size().px(&theme),
            theme.text_secondary,
        ))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| action(app, cx));
        })
        .into_any_element()
}

/// The dialog's primary action.
fn worktree_primary_button<F>(
    id: &'static str,
    label: String,
    theme: Theme,
    this: Entity<OrbitApp>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
{
    button_frame(div().id(id), &theme, ButtonSize::Medium)
        .border_1()
        .border_color(theme.accent.opacity(0.5))
        .raised(theme.accent.opacity(0.14), &theme)
        .cursor_pointer()
        .hover(|s| s.raised(theme.accent.opacity(0.2), &theme))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.accent_text)
        .child(label)
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| action(app, cx));
        })
        .into_any_element()
}

/// One row of the Advanced popover.
fn worktree_menu_row<F>(
    id: &'static str,
    icon_path: &'static str,
    label: String,
    theme: Theme,
    this: Entity<OrbitApp>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
{
    picker_entry(div().id(id), &theme)
        .h(picker::entry_height(&theme))
        .flex_none()
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_hover))
        .child(icon(
            icon_path,
            context_menu::ICON.px(&theme),
            theme.text_muted,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.text_secondary)
                .child(label),
        )
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            this.update(cx, |app, cx| action(app, cx));
        })
        .into_any_element()
}

/// A mode chip in the create dialog (New branch / Existing branch).
fn worktree_mode_chip<F>(
    id: &'static str,
    label: String,
    selected: bool,
    theme: Theme,
    this: Entity<OrbitApp>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
{
    button_frame(div().id(id), &theme, ButtonSize::Default)
        .border_1()
        .border_color(if selected {
            theme.accent.opacity(0.5)
        } else {
            theme.border
        })
        .raised(
            if selected {
                theme.accent.opacity(0.14)
            } else {
                theme.surface_elevated
            },
            &theme,
        )
        .cursor_pointer()
        .hover(|s| s.raised(theme.surface_hover, &theme))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if selected {
            theme.accent_text
        } else {
            theme.text_secondary
        })
        .child(label)
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| action(app, cx));
        })
        .into_any_element()
}

/// `/Users/user/...` reads `~/...` on every surface that shows a path.
fn shorten_home(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let home = crate::platform::home_dir();
    let home = home.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(home.as_ref()) {
        if rest.is_empty() {
            return "~".to_string();
        }
        if let Some(rest) = rest.strip_prefix(std::path::MAIN_SEPARATOR) {
            return format!("~/{rest}");
        }
    }
    raw.into_owned()
}
