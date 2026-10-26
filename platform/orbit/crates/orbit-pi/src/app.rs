//! The Orbit shell model: the `OrbitApp` struct (shared state), the
//! `Render`/`Focusable` entry points, and the feature modules that drive it.
//!
//! This file keeps the state model, the shared types, and the controller
//! wiring. Feature logic lives in descendant modules (see the map at the
//! bottom of the file), which can reach the struct's private fields directly:
//!
//! - [`runtime`] — pi process lifecycle, status/error, provider auth
//! - [`events`] — the heartbeat: event drain, responses, session watcher
//! - [`session`] — prompt/queue/turn lifecycle and navigation
//! - [`pickers`] — model, command-palette, branch, and workspace pickers
//! - [`pi_update_ui`] — the launch-time pi self-update: check, install, report
//! - [`composer_ops`] — autocomplete, attachments, add-menu, model chips
//! - [`sidebar`] — session/workspace sidebar rendering + row menus
//! - [`settings`] — Settings surface (General/Runtime/Agent/Skills/Plugins/Providers)
//! - [`skills_ui`] — Settings → Skills master-detail page + controllers
//! - [`toast_ui`] — the in-app toast stack: push/dismiss helpers and layer
//! - [`view`] — top-level chrome: sidebar, transcript, composer, status bar
//! - [`open_in`] — "open workspace in" app detection and menu
//! - [`helpers`] — icons, file glyphs, and small formatting helpers

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use gpui::{
    anchored, deferred, div, img, linear_color_stop, linear_gradient, list, point, prelude::*, px,
    radians, relative, uniform_list, AnchoredPositionMode, Animation, AnimationExt, AnyElement,
    App, ClipboardItem, Context, Corner, CursorStyle, DragMoveEvent, ElementId, Entity,
    ExternalPaths, FocusHandle, Focusable, FontWeight, Hsla, Image, ImageSource, IntoElement,
    ListAlignment, ListState, MouseButton, MouseDownEvent, MouseUpEvent, ObjectFit,
    PathPromptOptions, Pixels, Point, Render, Resource, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Subscription, TextAlign, Transformation, UniformListScrollHandle,
    Window,
};
use orbit_rpc::{
    CommandBody, ContextUsage, Event, PendingQueue, PiClient, QuotaReport, SessionState,
    SessionUsage,
};
use serde_json::Value;

use crate::access::AccessMode;
use crate::ask::{AskPrompt, AskQuestion};
use crate::auth::{AuthEffect, AuthManager, AuthSupport, LoginPhase, ProviderStatus};
use crate::branch_picker::BranchPicker;
use crate::bundled_extensions::BundledExtensions;
use crate::checkpoint;
use crate::command_palette::{self, CommandPalette, PaletteCommand, PaletteSnapshot};
use crate::composer::ComposerInput;
use crate::composer_send::{self, SendMode};
use crate::context_meter::{self, ContextMeterData, ContextPopup};
use crate::custom_ui::{CustomCancel, CustomFrame, CustomInput, CustomUi, CustomUiSurfaces};
use crate::dialog::{ApprovalRequest, Dialog, DialogRequest, DialogResponse};
use crate::git_panel::GitPanel;
use crate::mentions::{self, AcEntry, SharedAutocomplete, SlashCommand, Trigger, TriggerKind};
use crate::model_selector::{
    provider_icon, thinking_display, thinking_icon, ModelSelector, PickerKind,
};
use crate::notifications;
use crate::onboarding::{self, Dependency};
use crate::platform::{self, ExternalApp};
use crate::plugins::{PackageScope, PluginPackage, PluginUpdate};
use crate::providers::{self, CustomProvider};
use crate::quota::{QuotaManager, QuotaSupport};
use crate::sessions::{self, SessionInfo};
use crate::sidepane::{SidePane, SidePaneResize};
use crate::skills::Skill;
use crate::terminal::{TerminalPanel, TerminalResize};
use crate::theme::{self, Theme, ThemeMode};
use crate::toast;
use crate::transcript::{self, Transcript};
use crate::usage::page::UsagePage;
use crate::watch;
use crate::widgets::{ExtensionWidget, WidgetPlacement};
use crate::workflow::WorkflowMode;
use crate::workspace_mark::WorkspaceMark;
use crate::workspace_picker::{WorkspaceEntry, WorkspacePicker};

const SIDEBAR_DEFAULT_W: f32 = 248.;
const SIDEBAR_MIN_W: f32 = 200.;
/// Sessions shown under each workspace group before "Show more" appears.
const SIDEBAR_GROUP_SESSIONS_VISIBLE: usize = 3;
/// Room the side-pane resize keeps for the transcript column.
const PANE_MAX_RESERVE: f32 = 480.;

/// Drag marker for the sidebar resize handle (gpui typed drag state).
struct SidebarResize;

/// An invisible drag ghost — resizing leaves no floating preview.
struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
const CONTENT_MAX_W: f32 = 960.;

/// Rows shown in the `/`-command and `@`-file autocomplete menu.
const AUTOCOMPLETE_LIMIT: usize = 8;
/// Images that may ride along with one prompt.
const MAX_ATTACHMENTS: usize = 8;

/// How long a status message stays visible in the status bar.
const STATUS_MESSAGE_TTL: Duration = Duration::from_secs(6);

/// How often the app polls pi for new quota-bridge session entries. The
/// bridge appends only on change, so this is a cheap idempotent read.
const QUOTA_ENTRY_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Session start races the bridge's first fetch (it runs on `session_start`
/// too), so the first few polls use a short interval until a snapshot lands
/// — or the bootstrap budget runs out for an account with no providers.
const QUOTA_ENTRY_BOOTSTRAP_INTERVAL: Duration = Duration::from_secs(3);
const QUOTA_ENTRY_BOOTSTRAP_POLLS: u8 = 10;

/// Rows in the composer's "+" add menu (icon, label key, trailing hint).
const ADD_MENU_ITEMS: [(&str, &str, &str); 3] = [
    ("icons/image.svg", "composer.attach_image", ""),
    ("icons/file.svg", "composer.attach_file", ""),
    ("icons/at-sign.svg", "composer.mention_file", "@"),
];

/// Maximum sessions kept alive in the background. Beyond this, the
/// least-recently parked idle session is evicted (its process torn down);
/// running ones never are.
const MAX_LIVE_SESSIONS: usize = 6;

/// How long an idle parked session stays warm before its process is reaped.
/// Reusing a warm pi process makes re-opening a recent session instant
/// instead of paying Node startup again; the TTL bounds the memory cost.
const PARKED_IDLE_TTL: Duration = Duration::from_secs(300);

/// How long the session-details Update button shows its success check after a
/// rename commits, before reverting to the label.
const RENAME_FEEDBACK: Duration = Duration::from_millis(1400);

/// A session kept warm in the background: its own pi process, its own live
/// transcript, and its own agent-run state. Both running and idle sessions
/// are parked when the user switches away, so reopening is a resume (no
/// process spawn). Events keep draining every tick; a running session
/// resumes exactly where the stream left off.
struct ParkedSession {
    client: PiClient,
    transcript: Transcript,
    busy: bool,
    added: u64,
    removed: u64,
    /// MCP config fingerprint this parked process was spawned with. A stale
    /// process is dropped when the session is next opened, so it reloads the
    /// current servers instead of running the old ones.
    mcp_stamp: u64,
    /// Extension `setWidget` blocks live at park time, so switching back to a
    /// warm session restores them (a parked process never re-emits).
    widgets: Vec<ExtensionWidget>,
    /// The session's pending steering / follow-up queue at park time. pi only
    /// emits `queue_update` when the queue changes, so a warm resume would
    /// otherwise show nothing — and a stale restore would show messages pi
    /// already delivered. Drain keeps it current while parked.
    queue: PendingQueue,
    /// When this session was last parked; drives idle TTL reaping.
    parked_at: Instant,
}

/// An in-flight automatic retry (`auto_retry_start` → `auto_retry_end`):
/// which attempt pi is waiting on, out of how many, and the transient
/// provider error that triggered it.
struct RetryDetail {
    attempt: u64,
    max: Option<u64>,
    error: String,
}

/// The status bar's branch chip: the checked-out branch and its divergence
/// from upstream. `ahead_behind` is `None` without an upstream — unknown is
/// not the same fact as `↑0 ↓0`.
#[derive(PartialEq)]
struct BranchStatus {
    name: String,
    ahead_behind: Option<(usize, usize)>,
}

pub struct OrbitApp {
    client: Option<PiClient>,
    /// Live state of the active pi process, surfaced in Settings → Runtime.
    runtime: RuntimeStatus,
    /// Result of auto-applying pi's RPC patches before this launch, surfaced
    /// in Settings → Runtime. See [`crate::rpc_patches`].
    rpc_patches: crate::rpc_patches::PatchReport,
    /// Sessions with a live pi process, keyed by session-file path. The
    /// active session lives in `client`/`transcript` above; this map holds
    /// the background ones (see `ParkedSession`).
    /// Sidebar width in pixels — adjusted by dragging its right edge.
    sidebar_width: Pixels,
    lives: HashMap<PathBuf, ParkedSession>,
    /// Live processes parked before pi claimed their session file — the user
    /// switched away mid-boot, or between `new_session` and the `get_state`
    /// reply that names the file. Drained every tick like `lives`; re-keyed
    /// into `lives` as soon as the file is known (issue #46).
    pending_parks: Vec<ParkedSession>,
    transcript: Transcript,
    sessions: Vec<SessionInfo>,
    /// Debounced watcher over pi's session store — sessions written by the
    /// CLI or another Orbit window refresh the sidebar without `cmd-r`.
    session_watcher: Option<sessions::SessionWatcher>,
    /// Watches the active workspace so Review and the Git page refresh when
    /// files or git state change without an RPC event.
    workspace_watcher: Option<watch::WorkspaceWatcher>,
    /// The directory `workspace_watcher` is pointed at, so a workspace move
    /// re-points the watch (and a failed start is not retried every tick).
    workspace_watch_dir: Option<PathBuf>,
    sidebar_list: ListState,
    sidebar_visible: bool,
    /// Bumped on every sidebar toggle. The render keys its slide animation on
    /// it, so an open→close→open cycle animates every time (a boolean key
    /// would let the repeat skip) while the first frame — generation 0 — draws
    /// the settled state with no launch animation.
    sidebar_slide_gen: u64,
    /// Whether a feature page (Files / Git / Usage / AI Review) was open on
    /// the previous frame. A full-page Review pane yields only to a page that
    /// *opens* under it, never to one that was already there when the reader
    /// maximized.
    feature_open_last: bool,
    /// Whether the Review pane was full-page on the previous frame. The
    /// rising edge opens the sessions sidebar, so a maximized review never
    /// strands the reader without a way to switch sessions.
    pane_full_last: bool,
    /// Whether the new-task backdrop was on screen on the previous frame —
    /// recorded by `render`, read by `tick`. It is the only surface whose
    /// dithered field is alive, so the heartbeat only repaints for it here.
    backdrop_live: bool,
    /// When the backdrop's field started moving; `None` while it is off
    /// screen. `tick` derives the phase from it, so the drift is measured in
    /// wall-clock seconds rather than in heartbeat ticks (whose cadence changes
    /// with the run state).
    backdrop_clock: Option<Instant>,
    /// The field's current phase, in seconds — read by the backdrop and frozen
    /// by Reduce Motion.
    backdrop_phase: f32,
    /// Keyboard cursor for the sessions sidebar: an index into the current
    /// sidebar rows. `None` until the sidebar takes keyboard focus (⌘⇧B);
    /// the row it names paints the focused surface in `render_side_row`.
    sidebar_cursor: Option<usize>,
    /// Focus handle carrying the `Sidebar` key context while the sidebar is
    /// being keyboard-navigated (↑/↓/⏎/Esc).
    sidebar_focus: FocusHandle,
    pub(crate) input: Entity<ComposerInput>,
    model_label: String,
    /// Pi model id of the active model (stable match key for the picker).
    model_id: String,
    /// Provider id of the active model (drives the brand glyph on the chip).
    model_provider: String,
    thinking_label: String,
    busy: bool,
    /// Pending steering + follow-up messages reported by pi's `queue_update`
    /// (and echoed by `clear_queue`). While the agent runs, messages sent from
    /// the composer are queued as follow-ups and shown here until delivered.
    queue: PendingQueue,
    /// A `clear_queue` was sent as part of Escape; restore the returned text
    /// into the composer when the response arrives (docs' interactive-Esc).
    restore_queue_on_clear: bool,
    /// Settings → Agent: `set_follow_up_mode` value.
    follow_up_mode: String,
    /// Settings → Agent: `set_auto_compaction` value (read back from state).
    auto_compaction: bool,
    /// Settings → Agent: `set_auto_retry` value. pi's `get_state` does not
    /// expose this, so it reflects the last value Orbit sent.
    auto_retry: bool,
    /// Settings → Agent: auto session titles. Persisted to
    /// `~/.orbit-pi/auto-title.json`, which the bundled title extension reads.
    auto_title: crate::auto_title::AutoTitleConfig,
    /// Settings → Privacy: the session-retention policy. Persisted to
    /// `~/.orbit-pi/session-retention.json`; applied once per launch.
    retention: crate::session_retention::RetentionConfig,
    /// One-shot latch: the launch retention pass must run exactly once.
    retention_ran: bool,
    /// Settings → Privacy: Orbit's scratch files in the system temp folder.
    /// Refreshed when the page opens and after a cleanup, so rendering never
    /// touches the filesystem.
    temp_files: crate::temp_files::TempFiles,
    /// Display name pi reports for the session (`get_state.sessionName`).
    session_name: Option<String>,
    /// pi is compacting right now (`get_state` / `compaction_*`).
    is_compacting: bool,
    /// pi is inside an automatic-retry delay (`auto_retry_*`). pi doesn't
    /// expose this in `get_state`, so it's event-driven.
    retrying: bool,
    /// Detail of the in-flight automatic retry (attempt counter + the
    /// provider's transient error), shown on the persistent run-status
    /// strip until the retry resolves.
    retry_detail: Option<RetryDetail>,
    /// The most recent command / protocol / extension error. Shown as a
    /// dismissible red banner until cleared — a failure is never dropped
    /// (docs: #error-handling).
    error: Option<String>,
    /// The in-app toast stack: action confirmations and notifications that
    /// land while the window is frontmost. Rendered bottom-right over every
    /// surface (see [`toast_ui`]).
    toasts: toast::Toasts,
    /// Text of the last optimistic follow-up. Cleared once pi confirms it in
    /// `queue_update`; restored to the composer if the command fails.
    pending_follow_up: Option<String>,
    /// Rename field in the session-details popover.
    session_name_input: Entity<ComposerInput>,
    status: String,
    /// When the current `status` message was set; the status bar shows it
    /// for [`STATUS_MESSAGE_TTL`] and then lets it lapse.
    status_at: Option<Instant>,
    current_title: Option<String>,
    current_workspace: Option<PathBuf>,
    /// Logo found at a conventional path in the current workspace, shown in
    /// the new-task page's folder field; `None` keeps the folder glyph.
    workspace_logo: Option<Arc<Image>>,
    /// Status-bar branch chip: the checked-out branch and its divergence from
    /// upstream, fetched off-thread so render never shells out to git.
    branch: Option<BranchStatus>,
    /// Generation of the newest branch fetch; a late result from an older
    /// fetch is discarded.
    branch_fetch: u64,
    /// Real line counts from edit/write tool calls this session.
    added: u64,
    removed: u64,
    focus: FocusHandle,
    /// Catalog of models reported by `get_available_models`.
    available_models: Vec<ModelEntry>,
    /// Thinking levels reported by `get_available_thinking_levels`.
    available_thinking_levels: Vec<String>,
    /// The open picker popup (model or thinking dropdown), if any. The
    /// kind travels with the entity; creating/dropping this *is* the
    /// open/closed state. Each popup is anchored above its own chip.
    model_selector: Option<(PickerKind, Entity<ModelSelector>)>,
    /// The window-wide command palette (⌘P / sidebar Search row), if open.
    command_palette: Option<Entity<CommandPalette>>,
    /// A blocking extension dialog (`extension_ui_request`), if one is open.
    /// pi holds the run until the answer is sent, so this is a modal surface.
    dialog: Option<Entity<Dialog>>,
    /// Focus the dialog (or its text field) on the next paint — `tick` has no
    /// window to focus with.
    dialog_focus_pending: bool,
    /// The open inline access-guard approval, if any: a compact bar above the
    /// composer rather than the modal above. pi holds the tool call until it
    /// is answered.
    approval: Option<ApprovalRequest>,
    /// Highlighted button in the approval bar (arrow keys + hover move it).
    approval_highlight: usize,
    /// Focus handle that carries the `Approval` key context while the bar is
    /// open (focus moves here so ↑/↓/Enter/Escape hit it).
    approval_focus: FocusHandle,
    /// Focus the approval bar on the next paint (`tick` has no window).
    approval_focus_pending: bool,
    /// The id of the running `ask_user_question` tool call, if any. Set on
    /// `tool_execution_start`, cleared on its `tool_execution_end`.
    ask_tool_id: Option<String>,
    /// The questionnaire the running ask tool was invoked with, parsed once
    /// so the panel can render structured questions/options without reading
    /// the request's flattened title back apart.
    ask_questions: Vec<AskQuestion>,
    /// The live inline questionnaire panel above the composer, answering the
    /// extension's `select` / `input` requests without a scrim modal.
    ask: Option<AskPrompt>,
    /// Set once the panel commits: the buffered answers replay to the
    /// extension one blocking request at a time, so incoming requests are
    /// answered here instead of opening a panel.
    ask_replay: Option<ask::AskReplay>,
    /// Focus handle carrying the `AskPanel` key context.
    ask_focus: FocusHandle,
    /// Focus the panel (or its text field) on the next paint.
    ask_focus_pending: bool,
    /// Extension `setWidget` text blocks (above/below the composer), keyed by
    /// the extension's `widgetKey`. Per-session, so it parks with the session.
    extension_widgets: Vec<ExtensionWidget>,
    /// Live `ctx.ui.custom()` surfaces, newest (last) owning focus; multiple
    /// may be open when a component nests.
    custom_ui: CustomUiSurfaces,
    /// Focus the top custom surface on the next paint — `tick` has no window.
    custom_ui_focus_pending: bool,
    /// Whether pi advertises the `custom` extension-UI capability in
    /// `get_state.capabilities`. Purely informational today: frames are
    /// handled whenever they arrive.
    custom_ui_supported: bool,
    /// Full-window image lightbox for a transcript attachment image. `None`
    /// is closed. Opened by clicking an image tile, dismissed by click or
    /// Escape.
    lightbox: Option<Arc<Image>>,
    /// Open in-transcript find (⌘F): a find field, the matching message
    /// indices, and the selected hit. `None` is closed.
    transcript_search: Option<search::TranscriptSearch>,
    /// Open row-actions menu in the sessions sidebar (which session's path
    /// plus whether the popup is showing the delete confirmation).
    session_menu: Option<SessionMenu>,
    /// Whether the settings surface replaces the main content area.
    settings_open: bool,
    /// Active section within the settings surface.
    settings_section: SettingsSection,
    /// Open dropdown on the settings surface (language / font sizes).
    settings_select: Option<SettingsSelect>,
    /// Filter text for the open settings dropdown.
    settings_filter: Entity<ComposerInput>,
    /// Keyboard cursor in the open settings dropdown: an original option
    /// index, so it survives filtering. `<Enter>` picks it; the list keeps
    /// it in view.
    settings_select_highlight: Option<usize>,
    /// Scroll handle for the settings dropdown's option list.
    settings_select_scroll: UniformListScrollHandle,
    /// Settings → Report a bug: the issue title.
    bug_report_title: Entity<ComposerInput>,
    /// Settings → Report a bug: the free-form context the model drafts from.
    bug_report_context: Entity<ComposerInput>,
    /// Settings → Report a bug: what happened (the report body).
    bug_report_what: Entity<ComposerInput>,
    /// Settings → Report a bug: steps to reproduce (optional).
    bug_report_steps: Entity<ComposerInput>,
    /// Settings → Report a bug: which kind of issue to file (bug / feature /
    /// other). Drives the GitHub label and the draft prompt.
    bug_report_kind: crate::issue_message::ReportKind,
    /// Settings → Report a bug: screenshots to attach. GitHub's API cannot
    /// upload binaries, so these are written to disk and the web form is
    /// opened for the user to drag them in.
    bug_report_screenshots: Vec<Attachment>,
    /// A draft is being generated off-thread with the active model.
    bug_report_generating: bool,
    /// A report is being filed off-thread; the form's submit control is busy.
    bug_report_busy: bool,
    /// The last filing failure, shown under the form until the next attempt.
    bug_report_error: Option<String>,
    /// Global settings search. Typing filters the section list down to the
    /// settings whose label or keywords match, across every section.
    settings_search: Entity<ComposerInput>,
    /// Re-render the settings surface as the search is typed.
    _settings_search_sub: Subscription,
    /// Settings → General: which notification channels are on (persisted to
    /// `~/.orbit-pi/notifications.json`), plus the last macOS permission
    /// read. The read drives the honest "blocked" / "unbundled" rows — a
    /// switch the OS is blocking must not look like it is working.
    notification_prefs: notifications::Prefs,
    notification_auth: notifications::DesktopAuth,
    notification_auth_pending: bool,
    /// Whether this window is frontmost. Notifications are held while it is:
    /// the transcript itself is the notification then. False when no window
    /// is open at all, so a run that settles then still notifies.
    window_active: bool,
    /// A banner click asked for the window; the next paint brings it forward
    /// (`tick` has no `Window` to activate with).
    activate_window_pending: bool,
    /// Keeps the window watchers alive (re-registered whenever a window
    /// opens): the activation observer that gates notifications, and the
    /// window-closed observer that clears that gate.
    _window_watchers: Vec<Subscription>,
    /// Visited sessions, oldest first — drives the top-bar back/forward
    /// navigation. `history_index` points at the active entry.
    session_history: Vec<SessionInfo>,
    history_index: usize,
    /// Active workspace groups the user has explicitly collapsed.
    collapsed_workspaces: HashSet<String>,
    /// Non-active workspace groups the user has explicitly expanded.
    expanded_workspace_groups: HashSet<String>,
    /// Workspace groups whose session list is expanded past
    /// [`SIDEBAR_GROUP_SESSIONS_VISIBLE`]. The value is how many extra
    /// sessions are revealed; each "Show more" click adds one step of
    /// [`SIDEBAR_GROUP_SESSIONS_VISIBLE`], so a long history grows three rows
    /// at a time instead of landing all at once.
    expanded_session_groups: HashMap<String, usize>,
    /// The projects Orbit lists in its sidebar — its own, user-curated folder
    /// list. pi owns the session files; this only records which folders the
    /// user added, persisted to `~/.orbit-pi/workspaces.json`. A workspace is
    /// added when the user picks it to work in; removing one drops only this
    /// entry and never touches pi.
    workspaces: Vec<PathBuf>,
    /// When each listed workspace was added to Orbit, keyed by path — the
    /// stable sort key for [`WorkspaceSort::DateAdded`]. Persisted back into
    /// `workspaces.json`; a workspace with no record falls back to the
    /// folder's creation time.
    workspace_added_at: HashMap<PathBuf, SystemTime>,
    /// Per-workspace sidebar mark (icon stem + tint key), keyed by path and
    /// persisted beside the project list. A workspace with no entry wears the
    /// default folder mark in the muted ink.
    workspace_marks: HashMap<PathBuf, WorkspaceMark>,
    /// How workspace groups are ordered in the sidebar. Persisted alongside
    /// the project list; changing it re-sorts the UI and never touches pi.
    workspace_sort: WorkspaceSort,
    /// How the sidebar arranges sessions (workspace / one list). Persisted
    /// beside the project list; a UI-only choice that never touches pi.
    sidebar_group_by: SidebarGroupBy,
    /// Which archived sessions the sidebar lists. Persisted beside the
    /// project list.
    sidebar_archived_filter: SidebarArchivedFilter,
    /// The sidebar's Projects-header view menu is open.
    sidebar_sort_menu: bool,
    /// Open row-actions menu on a workspace group header (which workspace's
    /// label + cwd). Mutually exclusive with `session_menu`.
    workspace_menu: Option<WorkspaceMenu>,
    /// Worktree preferences (`~/.orbit-pi/worktrees.json`), shared by
    /// Settings → Worktrees and the Worktrees page. Repository-local
    /// `.orbit/worktree.json` overrides the directory and setup script.
    worktree_config: crate::worktree::WorktreeConfig,
    /// Settings → Worktrees: default-location field.
    worktree_dir_input: Entity<ComposerInput>,
    _worktree_dir_sub: Subscription,
    /// Settings → Worktrees: setup-script field.
    worktree_script_input: Entity<ComposerInput>,
    _worktree_script_sub: Subscription,
    /// Keeps the create dialog's name→branch derivation observers alive.
    _worktree_name_sub: Subscription,
    _worktree_branch_sub: Subscription,
    /// Whether the Worktrees page replaces the chat area.
    worktrees_open: bool,
    /// The dismissible "How worktrees work" card was closed (persisted beside
    /// the other one-time hints).
    worktree_howto_dismissed: bool,
    /// Whether the status bar's "Work in" picker (Local / worktrees) is open.
    work_in_menu_open: bool,
    /// Highlighted row in the "Work in" picker.
    work_in_menu_highlight: usize,
    /// Focus handle that carries the `WorkInMenu` key context while the picker
    /// is open (focus moves here so ↑/↓/Enter/Escape hit it).
    work_in_menu_focus: FocusHandle,
    /// Linked worktrees Git reports for the active repository (main first).
    worktrees: Vec<crate::worktree::Worktree>,
    /// Main worktree root of the active repository, cached from the last
    /// worktree list so path resolution never shells out during render.
    worktree_repo_root: Option<PathBuf>,
    /// A worktree list or mutation is in flight.
    worktrees_busy: bool,
    /// Generation of the newest worktree list; a late result from an older
    /// list (workspace switched, page reopened) is discarded.
    worktree_fetch: u64,
    /// A list was requested while one was in flight; re-run it when the
    /// current one settles.
    worktree_refresh_pending: bool,
    /// Repository path of the in-flight worktree mutation; guards one
    /// mutating operation per repository at a time (§31).
    worktree_mutation_repo: Option<PathBuf>,
    /// Human-readable label of the in-flight worktree operation.
    worktree_operation: Option<String>,
    /// Last worktree failure, shown as the page's banner.
    worktrees_error: Option<String>,
    /// Inline validation error for the open dialog.
    worktree_dialog_error: Option<String>,
    /// Whether the page's Advanced (prune / repair) menu is open.
    worktree_advanced_open: bool,
    /// Earliest time a watcher-driven worktree re-list may run (§30).
    worktree_refresh_due: Option<Instant>,
    /// The open Worktrees dialog, if any.
    worktree_dialog: Option<WorktreeDialog>,
    /// Create dialog: worktree name.
    worktree_name_input: Entity<ComposerInput>,
    /// Create dialog: new-branch name.
    worktree_branch_input: Entity<ComposerInput>,
    /// Create dialog: new-branch start point.
    worktree_start_input: Entity<ComposerInput>,
    /// Rename / Move dialog: the new name or destination.
    worktree_field_input: Entity<ComposerInput>,
    /// Branches loaded for the create dialog's branch choices.
    worktree_branches: Vec<String>,
    /// Create dialog: use a new branch or an existing one.
    worktree_branch_mode: WorktreeBranchMode,
    /// Create dialog: the user edited the branch field, so the name no longer
    /// drives it.
    worktree_branch_touched: bool,
    /// Guards the name→branch derivation from marking the branch as
    /// user-edited when Orbit sets it programmatically.
    worktree_branch_programmatic: bool,
    /// Create dialog: the chosen existing branch.
    worktree_branch_choice: Option<String>,
    /// Create dialog: run the setup script for this worktree.
    worktree_run_setup: bool,
    /// Setup progress and captured output for the most recent creation.
    worktree_setup: Option<WorktreeSetupState>,
    /// Open row menu on the Worktrees page.
    worktree_menu: Option<WorktreeMenu>,
    /// Path of the active session file, for the sidebar highlight.
    current_session_path: Option<PathBuf>,
    /// When the popup was dismissed by an outside mouse-down; guards against
    /// the same click's mouse-up immediately re-opening it via the chip.
    menu_dismissed_at: Option<Instant>,
    /// Live context-window usage from `get_session_stats`. `None` when pi
    /// hasn't advertised a window (no model) or the command isn't supported.
    context: Option<ContextUsage>,
    /// Cumulative session token/cost totals from the same `get_session_stats`
    /// response. Unlike `context`, this spans the whole session (all turns,
    /// tools, and compaction summaries), so the popup can show cost + cache.
    session_usage: Option<SessionUsage>,
    /// Hover compact card vs click-to-open breakdown for the context ring.
    context_popup: ContextPopup,
    /// Shared `/`-command and `@`-file menu state between the composer
    /// (which owns ↑/↓) and this app (which owns Enter/Escape + rendering).
    autocomplete: SharedAutocomplete,
    /// Dismissed via outside mouse-down; cleared when the trigger changes.
    autocomplete_dismissed: bool,
    /// Trigger (kind + query) the autocomplete highlight was synced against.
    last_ac_trigger: Option<(TriggerKind, String)>,
    /// Slash commands reported by pi (`get_commands` — extensions + skills).
    slash_commands: Vec<SlashCommand>,
    /// Workspace files for `@`-mentions (cached per workspace).
    mention_files: Vec<String>,
    mention_files_workspace: Option<PathBuf>,
    /// Images queued to ride along with the next prompt (pasted or picked).
    attachments: Vec<Attachment>,
    /// Files are being dragged over the composer (external OS drag).
    file_drag_hovered: bool,
    /// Installed folder-capable apps for the header's "open in" control.
    open_in_apps: Rc<Vec<ExternalApp>>,
    /// Whether the open-in app picker dropdown is open.
    open_in_menu_open: bool,
    /// Filter text for the open-in menu.
    open_in_filter: Entity<ComposerInput>,
    /// Whether the composer's "+" add menu is open.
    add_menu_open: bool,
    /// Highlighted row in the add menu (arrow keys + hover move it).
    add_menu_highlight: usize,
    /// Focus handle that carries the `AddMenu` key context while the menu
    /// is open (focus moves here so ↑/↓/Enter/Escape hit the menu, then
    /// returns to the composer).
    add_menu_focus: FocusHandle,
    /// Git branch picker anchored to the status-bar branch chip.
    branch_picker: Option<Entity<BranchPicker>>,
    /// Folder selector anchored under the new-task page's workspace field.
    workspace_picker: Option<Entity<WorkspacePicker>>,
    /// A checkout/create is running on the background executor.
    branch_operation_pending: bool,
    /// Persisted open-in choices, resolved against the active workspace.
    open_in_prefs: platform::OpenInPrefs,
    /// Serializes preference writes so rapid selections cannot save out of order.
    open_in_save_task: Option<gpui::Task<()>>,
    /// Keeps the theme global observer alive so a settings toggle redraws.
    _theme_sub: Subscription,
    /// Keeps the composer observer alive: edits re-render the app so the
    /// send button's quiet/ready state tracks the text live.
    _input_sub: Subscription,
    /// Keeps the transcript `cmd-c` interceptor alive: a live transcript
    /// selection wins over the focused composer's own copy, but only while
    /// one exists (the composer keeps its copy otherwise).
    _copy_selection_sub: Subscription,
    /// Onboarding dependency check results (pi, node, git).
    deps: Vec<Dependency>,
    /// How the installed pi CLI is provided (official managed install, npm, or
    /// another package manager). Refreshed with the dependency probe and after
    /// every install/update, so the setup page and the updater agree.
    pi_install: crate::pi_install::Installation,
    /// A newer pi exists but this install is npm-managed and must be migrated
    /// to the official installer first. Holds the available version; cleared
    /// once migration succeeds. Never acted on without the user's click.
    pi_migration_required: Option<String>,
    /// True while the official installer runs on the background executor.
    pi_migrating: bool,
    /// Whether the setup page is open on request (Settings → About →
    /// Requirements) rather than because something is missing.
    setup_open: bool,
    /// The machine this build is running on. Probed at startup and on the
    /// setup page's Refresh (the OS probe runs a command, so it never happens
    /// on the render path); the setup page and Settings → About read it.
    host: platform::Host,
    /// Whether the setup page's Refresh check is in flight (spins the button).
    refreshing: bool,
    /// Whether the top-bar session-details popover is open.
    session_details_open: bool,
    /// Whether the top bar's overflow ("more") menu is open. Holds the
    /// surfaces that no longer earn a permanent chip: Session details,
    /// Explorer, the Terminal, and the Git page.
    header_more_open: bool,
    /// A popover-triggered title generation is in flight; the next
    /// `session_info_changed` seeds the rename field from its result.
    title_generating: bool,
    /// When the last successful session rename committed, so the popover's
    /// Update button can flash its check before reverting to the label. The
    /// stamp lets a second click extend the flash instead of clearing early.
    rename_saved_at: Option<Instant>,
    /// Whether the top-bar provider-quota popover is open.
    quota_popup_open: bool,
    /// Which pane the top-bar usage popover shows: provider quotas or MCP
    /// server status.
    quota_popup_tab: mcp_ui::QuotaPopupTab,
    /// A manual quota refresh is in flight: the popover's refresh button spins
    /// until the `quota.list` reply lands, or a short timeout clears it.
    quota_refreshing: bool,
    /// When the in-flight manual refresh started, so its spin is kept visible
    /// for a minimum duration even if the reply is immediate.
    quota_refresh_started: Option<Instant>,
    /// Whether the top-bar quota popover's "Hidden" group is expanded. The
    /// group lists providers the user hid from the list, so a mistoggle is
    /// one click to undo.
    quota_hidden_open: bool,
    /// Providers the user hid from the top-bar usage popover, from
    /// `~/.orbit-pi/hidden-quota-providers.json`, loaded at launch. The
    /// popover filters its cards against this; Settings still shows them.
    hidden_quota_providers: crate::quota_hidden::HiddenProviders,
    /// The `sessionId` pi reports for the active session (its task id).
    session_id: Option<String>,
    /// Current agent turn number for this session (0 = none yet).
    turn_count: usize,
    /// A turn's start checkpoint was captured and awaits its end.
    turn_open: bool,
    /// Latest turn with a captured end checkpoint (drives Review's Last Turn).
    latest_turn: Option<usize>,
    /// Right side pane — Review (git diff).
    sidepane: Entity<SidePane>,
    /// Keeps the right pane's observer alive: the pane can change its own
    /// width, full-page state, or collapse from its header, and the app's
    /// layout (sidebar, main column, terminal) must follow immediately.
    _sidepane_sub: Subscription,
    /// Keeps the file viewer's observer alive: the Explorer marks the active
    /// file from the viewer's selected tab.
    _file_viewer_sub: Subscription,
    /// Right dock — the workspace file tree (cmd-shift-e).
    project_panel: Entity<crate::explorer::ProjectPanel>,
    /// Full-page read-only file viewer (the Files surface).
    file_viewer: Entity<crate::explorer::FileViewer>,
    /// Bottom panel — an integrated shell (cmd-j).
    terminal_panel: Entity<TerminalPanel>,
    /// Whether the Git page replaces the chat area.
    git_open: bool,
    /// The full-page Git panel (tabs + commit bar).
    git_panel: Entity<GitPanel>,
    /// Whether the Usage page replaces the chat area.
    usage_open: bool,
    /// The Usage page: analytics over pi's own session store.
    usage: Entity<UsagePage>,
    /// Custom providers read from `~/.pi/agent/models.json` (cached; reloaded
    /// when the Providers page opens, on Refresh, and after a save/remove).
    custom_providers: Vec<CustomProvider>,
    /// Parse/read error from models.json, surfaced on the Providers page
    /// instead of silently overwriting a hand-edited file.
    custom_providers_error: Option<String>,
    /// Credentials read from `~/.pi/agent/auth.json` (cached alongside the
    /// custom providers).
    provider_auth: HashMap<String, providers::ProviderAuth>,
    /// Parse/read error from auth.json.
    provider_auth_error: Option<String>,
    /// Non-sensitive provider-authentication state driven by the `auth.*`
    /// RPC namespace. When pi doesn't advertise those commands this stays
    /// unsupported and the page keeps the Terminal login fallback.
    auth: AuthManager,
    /// Non-secret account quota/balance/spend per connected provider, from the
    /// `quota.*` RPC namespace and the bundled bridge extension's session
    /// entries. Empty when nothing has been reported yet.
    quota: QuotaManager,
    /// The bundled pi extensions (quota bridge + access guard), spawned with
    /// `--extension` on every session process.
    extensions: BundledExtensions,
    /// The active access mode. Persisted to `~/.orbit-pi/access.json`, which
    /// the guard extension reads fresh on every tool call.
    access_mode: AccessMode,
    /// Whether the composer's access-mode picker popover is open.
    access_menu_open: bool,
    /// Highlighted row in the access-mode picker (arrow keys + hover move it).
    access_menu_highlight: usize,
    /// Focus handle that carries the `AccessMenu` key context while the picker
    /// is open (focus moves here so ↑/↓/Enter/Escape hit it).
    access_menu_focus: FocusHandle,
    /// The active session's workflow mode (Plan / Build / Ask). Persisted per
    /// session to `~/.orbit-pi/workflow.json`, which the workflow extension
    /// reads fresh on every agent hook.
    workflow_mode: WorkflowMode,
    /// A mode chosen on the New Task page, before a session id exists to key
    /// it to.
    workflow_pending: Option<WorkflowMode>,
    /// The model new sessions start on, from
    /// `~/.orbit-pi/session-defaults.json`, loaded at launch.
    session_default: crate::session_defaults::SessionDefault,
    /// Whether the active session should still be moved onto the default
    /// model. Armed by a `new_session` birth; disarmed once the default has
    /// been pushed (or there is none), by a manual model/thinking choice, or
    /// when pi rejects the push — so a later catalog refresh never clobbers a
    /// manual composer choice.
    default_model_armed: bool,
    /// What `apply_default_model` has already sent for the active session,
    /// so the catalog refreshes that follow a session birth don't resend the
    /// same `set_model`.
    default_model_pushed: DefaultModelPushed,
    /// Whether the composer's workflow-mode picker popover is open.
    workflow_menu_open: bool,
    /// Highlighted row in the workflow-mode picker.
    workflow_menu_highlight: usize,
    /// Focus handle that carries the `WorkflowMenu` key context while open.
    workflow_menu_focus: FocusHandle,
    /// `get_entries` cursor for the bridge's quota snapshots. Entry ids are
    /// per-session, so this resets when the active session changes.
    quota_entries_cursor: Option<String>,
    /// One bridge poll in flight at a time.
    quota_entries_inflight: bool,
    /// Remaining fast (bootstrap) polls for a fresh session; 0 = steady state.
    quota_entries_bootstrap: u8,
    /// When the next bridge poll is due; throttles the 90 ms heartbeat.
    quota_entries_next_poll: Instant,
    /// True once a credential changed and pi needs a restart to load it
    /// (pi reads auth.json only at startup). Only used on the file-based
    /// fallback path; RPC logins take effect live.
    provider_auth_dirty: bool,
    /// Built-in catalog size per provider (from pi's bundled model data),
    /// loaded off-thread so unconfigured providers show a real count.
    provider_catalog_counts: HashMap<String, usize>,
    /// Authoritative provider metadata introspected from pi-ai (empty when
    /// unavailable — then the curated table is used instead).
    provider_metadata: Vec<providers::DynamicProvider>,
    /// The metadata load was kicked off (success or not) — avoids re-spawning
    /// on every reload.
    provider_metadata_loaded: bool,
    /// The open API-key editor, if any.
    provider_key_editor: Option<ProviderKeyEditor>,
    /// The open provider editor (add or edit), if any.
    provider_editor: Option<ProviderEditor>,
    /// Provider id awaiting inline remove confirmation.
    provider_remove_confirm: Option<String>,
    /// Provider id whose usage/quota popup is open.
    provider_usage_open: Option<String>,
    /// Refresh button spin state on the Providers page.
    providers_refreshing: bool,
    /// Search filter for the provider grid.
    provider_filter: Entity<ComposerInput>,
    /// Re-render the grid as the filter is typed.
    _provider_filter_sub: Subscription,
    /// Search filter for the Models page.
    models_filter: Entity<ComposerInput>,
    /// Re-render the model list as the filter is typed.
    _models_filter_sub: Subscription,
    /// Models page: show favorited models only (the second filter next to
    /// the search field).
    models_favorites_only: bool,
    /// Models page: provider scope. `None` shows every provider; `Some(id)`
    /// narrows the grid to one, mirroring the composer picker's provider
    /// chips.
    models_provider_filter: Option<String>,
    /// True while the Models page's "Update model list" action runs pi's
    /// catalog refresh and reloads the agent.
    models_refreshing: bool,
    /// Skills discovered for the current workspace (project + user scope).
    skills: Vec<Skill>,
    /// Installed pi packages (plugins) from user + project settings.
    plugins: Vec<PluginPackage>,
    /// A malformed settings file surfaced on the Plugins page.
    plugins_error: Option<String>,
    /// Source field for installing a new plugin.
    plugin_source_input: Entity<ComposerInput>,
    /// Install the next plugin into project scope instead of user scope.
    plugin_install_project: bool,
    /// Description of the plugin operation in flight, if any.
    plugin_action: Option<String>,
    /// Actionable update notice for installed npm/Git packages.
    plugin_update_prompt: Option<Vec<PluginUpdate>>,
    /// Workspace whose project-scoped packages were checked.
    plugin_update_workspace: Option<PathBuf>,
    /// One registry/Git probe in flight at a time.
    plugin_updates_checking: bool,
    /// Versions the user chose to skip, persisted between launches.
    plugin_update_skips: HashMap<String, String>,
    /// Manual-refresh feedback: the toolbar button turns until this instant,
    /// so an instant reload still acknowledges the click.
    plugin_refresh_spin_until: Option<Instant>,
    /// Plugin source awaiting inline remove confirmation.
    plugin_remove_confirm: Option<String>,
    /// Re-render the toolbar as the install field is typed.
    _plugin_source_sub: Subscription,
    /// Search filter for the installed-plugin list.
    plugins_filter: Entity<ComposerInput>,
    /// Re-render the plugin list as the filter is typed.
    _plugins_filter_sub: Subscription,
    /// Settings → Skills: filter field for the skill list.
    skills_filter: Entity<ComposerInput>,
    /// The skill selected in the master-detail page (its `SKILL.md` path).
    selected_skill: Option<PathBuf>,
    /// Cached `SKILL.md` body for the selected skill.
    skill_content: Option<String>,
    /// A skill directory awaiting delete confirmation.
    skill_delete_confirm: Option<PathBuf>,
    /// Re-render the skill list as the filter is typed.
    _skills_filter_sub: Subscription,
    /// Background updater status mirrored from the global: `Idle` (nothing to
    /// show), `Available` (the footer and settings offer the update),
    /// `Updating` (spinning until the app quits to install). The payload
    /// details stay on the worker; only the release version rides along.
    updater_status: crate::updater::UpdateStatus,
    /// The staged release's version (e.g. `0.0.3`), mirrored beside
    /// `updater_status` so the settings buttons can name the download.
    updater_version: Option<String>,
    /// The staged release's notes from the feed, mirrored for the update
    /// modal's changelog.
    updater_notes: Option<String>,
    /// The feed's releases, newest first, mirrored for the modal's Version
    /// History. Empty until the first check answers.
    updater_history: Vec<crate::updater::Release>,
    /// Whether the modal is showing Version History instead of the state's
    /// body. The dialog underneath is preserved, so Back returns to it.
    updater_history_open: bool,
    /// The open update modal, if any. `None` leaves the download control and
    /// the Check for Updates command running against `updater_status` alone.
    updater_dialog: Option<UpdateDialog>,
    /// Focus handle that carries the `UpdateDialog` key context while the
    /// modal is open, so Escape dismisses it instead of aborting the run.
    updater_dialog_focus: FocusHandle,
    /// Focus the update modal on the next paint (`tick` has no window).
    updater_dialog_focus_pending: bool,
    /// True while the pointer is over the sidebar updater pill, which expands
    /// it from the download icon into the "Update" label (the reference app's
    /// pattern).
    updater_button_hovered: bool,
    /// In-flight pill width and label cross-fade. The animation closure writes
    /// them, so a reversal mid-flight starts from the last painted frame
    /// instead of snapping back to the collapsed width.
    updater_button_width: Rc<Cell<f32>>,
    updater_button_label_reveal: Rc<Cell<f32>>,
    /// Width/reveal the current pill animation started from, plus a generation
    /// that keys `with_animation` so each hover change restarts it.
    updater_button_animation_from_width: f32,
    updater_button_animation_from_reveal: f32,
    updater_button_animation_generation: u64,
    /// Mirror of the persisted automatic-check preference, refreshed when the
    /// updater reports and on toggle, so frames never read the file.
    automatic_updates_enabled: bool,
    /// MCP management: config, secrets, and per-server runtime state for the
    /// Settings → MCP page. One source of truth; no MCP view state is
    /// duplicated elsewhere.
    mcp: crate::mcp::McpManager,
    /// Search filter for the MCP server list.
    mcp_filter: Entity<ComposerInput>,
    /// Re-render the list as the filter is typed.
    _mcp_filter_sub: Subscription,
    /// The MCP add/edit modal, if open.
    mcp_editor: Option<mcp_ui::McpEditor>,
    /// Focus handle carrying the `McpList` key context while the MCP page is
    /// open (↑/↓ move the row cursor, Enter edits, Space toggles).
    mcp_list_focus: FocusHandle,
    /// Focus the MCP list on the next paint (`set_settings_section` has no
    /// window to focus with).
    mcp_focus_pending: bool,
    /// Server name awaiting inline remove confirmation.
    mcp_remove_confirm: Option<String>,
    /// Keyboard cursor in the MCP list (server name; `None` until the page
    /// takes focus).
    mcp_cursor: Option<String>,
    /// Debounced probe due time.
    mcp_probe_at: Option<Instant>,
    /// Debounced Pi apply due time.
    mcp_apply_at: Option<Instant>,
    /// A configuration change arrived while a run was in flight; apply once
    /// the run settles.
    mcp_apply_pending: bool,
    /// The next apply must restart even when the config fingerprint is
    /// unchanged — used after sign-out, so a live session's access is dropped.
    mcp_apply_forced: bool,
    /// MCP config fingerprint the active Pi process was spawned with.
    mcp_stamp: u64,
    /// The server a "Test connection" result should toast for.
    mcp_test_target: Option<String>,
    /// The in-flight OAuth sign-in (server + its cancel flag), if any.
    mcp_auth: Option<mcp_ui::McpAuthJob>,
    /// The config files changed on disk outside Orbit (page notice).
    mcp_external_change: bool,
    /// Next external-change poll while the MCP section is open.
    mcp_external_check_at: Option<Instant>,
}

/// An image queued to ride along with the next prompt.
struct Attachment {
    name: String,
    mime: String,
    /// base64 payload (no `data:` prefix) — pi's prompt image shape.
    data: String,
    /// Decoded image for the chip thumbnail.
    preview: Option<Arc<gpui::Image>>,
    /// Raw byte length of the original file/paste.
    original_len: usize,
    /// Raw byte length of what is actually stored (after compression).
    stored_len: usize,
    /// The image was resized or re-encoded before it was queued.
    compressed: bool,
}

impl Attachment {
    fn from_image(image: &gpui::Image, index: usize) -> Self {
        let ext = match image.format {
            gpui::ImageFormat::Png => "png",
            gpui::ImageFormat::Jpeg => "jpg",
            gpui::ImageFormat::Webp => "webp",
            gpui::ImageFormat::Gif => "gif",
            gpui::ImageFormat::Bmp => "bmp",
            gpui::ImageFormat::Svg => "svg",
            gpui::ImageFormat::Tiff => "tiff",
        };
        Self::from_bytes(
            tr!("app.pasted_image", index = index + 1, ext = ext),
            &image.bytes,
            image.format,
        )
    }

    /// Images only — anything else returns `None` so a drop can fall back to
    /// referencing the file by path instead.
    fn from_path(path: &Path) -> Option<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)?;
        let format = match ext.as_str() {
            "png" => gpui::ImageFormat::Png,
            "jpg" | "jpeg" => gpui::ImageFormat::Jpeg,
            "webp" => gpui::ImageFormat::Webp,
            "gif" => gpui::ImageFormat::Gif,
            "bmp" => gpui::ImageFormat::Bmp,
            _ => return None,
        };
        let bytes = std::fs::read(path).ok()?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".into());
        Some(Self::from_bytes(name, &bytes, format))
    }

    /// Build an attachment from encoded image bytes, compressing at ingest so
    /// a large screenshot never rides the prompt (or the session history) at
    /// full size.
    fn from_bytes(name: String, bytes: &[u8], format: gpui::ImageFormat) -> Self {
        let compressed = crate::attachments::compress_image(bytes, format);
        let preview = Arc::new(gpui::Image::from_bytes(
            compressed.format,
            compressed.bytes.clone(),
        ));
        Self {
            name,
            mime: compressed.format.mime_type().to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&compressed.bytes),
            preview: Some(preview),
            original_len: bytes.len(),
            stored_len: compressed.bytes.len(),
            compressed: compressed.compressed,
        }
    }

    /// The wire shape pi's `prompt.images` expects.
    fn to_prompt_image(&self) -> Value {
        serde_json::json!({ "type": "image", "data": self.data, "mimeType": self.mime })
    }

    /// The decoded image bytes, for writing the file to disk (the bug report
    /// hands screenshots to GitHub's web editor, which is the only place
    /// GitHub accepts binary attachments).
    fn bytes(&self) -> Option<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.data)
            .ok()
    }
}

impl OrbitApp {
    /// Queue one prepared attachment, honoring the cap and reporting
    /// compression so a large screenshot visibly shrinks before it ships.
    fn push_attachment(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        if self.attachments.len() >= MAX_ATTACHMENTS {
            self.set_status(tr!("composer_ops.max_attachments", count = MAX_ATTACHMENTS));
            return;
        }
        if attachment.compressed {
            self.set_status(tr!(
                "composer_ops.image_compressed",
                original = crate::attachments::human_bytes(attachment.original_len),
                stored = crate::attachments::human_bytes(attachment.stored_len),
            ));
        }
        self.attachments.push(attachment);
        cx.notify();
    }
}

/// A model choice from the pi runtime catalog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) provider: String,
    /// Provider-reported context window in tokens, when the catalog exposes it.
    pub(crate) context_window: Option<u64>,
    /// The thinking levels the model supports, in pi's own order — derived
    /// from the catalog's `reasoning` flag and `thinkingLevelMap` exactly like
    /// pi's `getSupportedThinkingLevels`. A non-reasoning model is `["off"]`.
    pub(crate) thinking_levels: Vec<String>,
}

/// pi's thinking-level ladder, in the order pi itself uses.
const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The thinking levels a `get_available_models` entry supports — the
/// client-side mirror of pi-ai's `getSupportedThinkingLevels`. `xhigh` and
/// `max` are opt-in through `thinkingLevelMap`; the other levels are dropped
/// only when the map marks them `null`; a model without `reasoning` supports
/// only `off`.
pub(crate) fn catalog_thinking_levels(model: &Value) -> Vec<String> {
    if !model
        .get("reasoning")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return vec!["off".to_string()];
    }
    let map = model.get("thinkingLevelMap").and_then(Value::as_object);
    THINKING_LEVELS
        .iter()
        .filter(|level| match map.and_then(|map| map.get(**level)) {
            // Explicitly unsupported by the provider.
            Some(Value::Null) => false,
            // The extended levels are exposed only when the map defines them.
            None => !matches!(**level, "xhigh" | "max"),
            Some(_) => true,
        })
        .map(|level| level.to_string())
        .collect()
}

impl OrbitApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let autocomplete: SharedAutocomplete = Rc::new(std::cell::RefCell::new(
            mentions::AutocompleteState::default(),
        ));
        let input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_key_context("Composer ChatComposer")
                .with_autocomplete(autocomplete.clone())
        });
        // The chat input is the workbench's home tab stop; every other
        // ComposerInput (filters, rename fields, pickers) stays out of the
        // traversal order.
        input.read(cx).focus_handle(cx).tab_stop(true);
        // Filter fields for the settings dropdowns and the open-in menu.
        // `Composer Picker` keeps backspace/delete working while Enter/arrows
        // dispatch to the (unhandled) Picker actions rather than submitting.
        let settings_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("app.filter")
                .with_key_context("Composer Picker")
        });
        // The settings surface's own search: one field over every section's
        // rows, not the select popup's filter above.
        let settings_search = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("settings-search")
                .with_placeholder_key("app.search_settings")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let settings_search_sub = cx.observe(&settings_search, |_, _, cx| cx.notify());
        // Settings → Report a bug: the issue title, the description, and the
        // optional reproduction steps. The description fields carry the
        // `Editor` context so Enter inserts a newline rather than submitting
        // the chat composer.
        let bug_report_title = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("bug-report-title")
                .with_placeholder_key("bug_report.title_placeholder")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let bug_report_what = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("bug-report-what")
                .with_placeholder_key("bug_report.what_placeholder")
                .with_key_context("Editor")
                .with_max_lines(8)
        });
        let bug_report_context = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("bug-report-context")
                .with_placeholder_key("bug_report.context_placeholder")
                .with_key_context("Editor")
                .with_max_lines(8)
        });
        let bug_report_steps = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("bug-report-steps")
                .with_placeholder_key("bug_report.steps_placeholder")
                .with_key_context("Editor")
                .with_max_lines(6)
        });
        let open_in_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("app.filter")
                .with_key_context("Composer Picker")
        });
        let provider_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("provider-filter")
                .with_placeholder_key("app.search_providers")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let provider_filter_sub = cx.observe(&provider_filter, |_, _, cx| cx.notify());
        let models_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("models-filter")
                .with_placeholder_key("app.search_models")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let models_filter_sub = cx.observe(&models_filter, |_, _, cx| cx.notify());

        // Settings → Plugins: the install-source field. `Composer Picker`
        // keeps editing keys live; Enter is unhandled, so the Install button
        // is the only commit path.
        let plugin_source_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("plugin-source-input")
                .with_placeholder_key("app.plugin_source_placeholder")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let plugin_source_sub = cx.observe(&plugin_source_input, |_, _, cx| cx.notify());
        let plugins_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("plugins-filter")
                .with_placeholder_key("app.search_installed_plugins")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let plugins_filter_sub = cx.observe(&plugins_filter, |_, _, cx| cx.notify());

        // Settings → Skills: the list filter. `Composer Picker` keeps editing
        // keys live so typing filters the master list.
        let skills_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("skills-filter")
                .with_placeholder_key("app.search_skills")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let skills_filter_sub = cx.observe(&skills_filter, |_, _, cx| cx.notify());

        // Settings → MCP: the server-list filter.
        let mcp_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("mcp-filter")
                .with_placeholder_key("mcp.search_servers")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let mcp_filter_sub = cx.observe(&mcp_filter, |_, _, cx| cx.notify());

        // Session-details popover: rename field. Default `Composer` key context
        // keeps real text editing (selection, clipboard, arrows); Enter routes
        // to Submit, which commits the name instead of the composer while this
        // field holds focus (see `on_submit`).
        let session_name_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("session-name-input")
                .with_placeholder_key("app.session_name")
                .with_max_lines(1)
                .with_wrap(false)
        });

        // Spawn pi rooted at the folder the user last worked in. Launched
        // from Finder the process cwd is `/`, so the store is the real
        // default; a deleted folder falls back to cwd. Sessions live in the
        // real ~/.pi/agent/sessions so they are shared with the CLI.
        let workspace = load_last_workspace()
            .or_else(|| std::env::current_dir().ok())
            .map(|path| sessions::canonical_workspace_path(&path))
            .unwrap_or_else(|| PathBuf::from("."));
        let workspace_logo = crate::workspace_logo::load(&workspace);
        let extensions = BundledExtensions::install();
        // Teach the installed pi's RPC mode the capabilities Orbit uses
        // (custom UI, quota, auth) before spawning it. Best-effort and cached
        // across launches; see `rpc_patches`.
        let rpc_patches = crate::rpc_patches::apply_on_launch();
        // Load the access mode and write it back so the guard extension finds
        // the file on the very first tool call of the session.
        let access_mode = AccessMode::load();
        access_mode.persist();
        // MCP configuration and secrets must be loaded before the first spawn
        // so the child's environment carries every `${NAME}` reference the
        // config uses, and so the process can be stamped with the config it
        // was spawned from.
        let mcp = crate::mcp::McpManager::load(Some(&workspace));
        // Status/tool data comes from Pi itself; with servers configured, run
        // one deferred probe at launch so the status-bar chip and the MCP page
        // start from real state instead of "not checked".
        let mcp_probe_at = (!mcp.is_empty()).then(|| Instant::now() + Duration::from_secs(3));
        let (client, connect_error) = match extensions.spawn(&workspace, true, &mcp.secret_env()) {
            Ok(client) => (Some(client), String::new()),
            Err(err) => (None, tr!("runtime.pi_spawn_failed", error = err)),
        };
        let mcp_stamp = mcp.fingerprint();
        let runtime = RuntimeStatus {
            started_at: client.as_ref().map(|_| Instant::now()),
            alive: client.is_some(),
            exited: false,
            error: (!connect_error.is_empty()).then(|| connect_error.clone()),
        };

        // A palette switch must repaint every surface, including the child
        // entities GPUI renders independently of this view. The app itself
        // observes the global; the panels that own their own element trees are
        // notified explicitly so a theme change is never left half-applied.
        let theme_sub = cx.observe_global::<Theme>(|this, cx| {
            this.input.update(cx, |_, cx| cx.notify());
            if let Some((_, selector)) = &this.model_selector {
                selector.update(cx, |_, cx| cx.notify());
            }
            this.project_panel.update(cx, |_, cx| cx.notify());
            this.file_viewer.update(cx, |_, cx| cx.notify());
            this.sidepane.update(cx, |_, cx| cx.notify());
            this.terminal_panel.update(cx, |_, cx| cx.notify());
            this.git_panel.update(cx, |_, cx| cx.notify());
            this.usage.update(cx, |_, cx| cx.notify());
            if let Some(picker) = &this.branch_picker {
                picker.update(cx, |_, cx| cx.notify());
            }
            if let Some(picker) = &this.workspace_picker {
                picker.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        });

        // Composer edits notify only the input entity; re-sync the
        // slash/mention menu and re-render the app so the send button's
        // quiet/ready state tracks the text as you type.
        let input_sub = cx.observe(&input, |this, _, cx| {
            this.sync_autocomplete(cx);
            cx.notify();
        });

        // `cmd-c` with a live transcript selection copies that selection even
        // while the composer holds focus — an interceptor is the only hook
        // that runs before focus-path action dispatch, so the composer keeps
        // its own copy whenever the transcript has nothing selected.
        let copy_selection_sub = {
            let app = cx.entity().downgrade();
            cx.intercept_keystrokes(move |event, _window, cx| {
                let keystroke = &event.keystroke;
                if keystroke.key != "c"
                    || !keystroke.modifiers.platform
                    || keystroke.modifiers.shift
                    || keystroke.modifiers.alt
                    || keystroke.modifiers.control
                {
                    return;
                }
                let _ = app.update(cx, |app, cx| {
                    let Some(text) = app.transcript.selected_text() else {
                        return;
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    cx.stop_propagation();
                });
            })
        };

        // Probe the runtime pieces we need (pi, node, git) so the setup page
        // can show install commands when something is missing.
        let deps = onboarding::check_dependencies();
        // How pi was installed, derived from the launcher the probe already
        // resolved — no extra `pi --version` spawn at startup.
        let pi_install = {
            let mut installation = crate::pi_install::detect_from_bin(onboarding::locate("pi"));
            installation.version = deps
                .iter()
                .find(|dep| dep.bin == "pi")
                .and_then(|dep| dep.version.clone());
            installation
        };
        let host = platform::host();

        // Right side pane: Review (git diff).
        let sidepane = cx.new(SidePane::new);
        let sidepane_sub = cx.observe(&sidepane, |this, pane, cx| {
            // Review and the Explorer are mutually exclusive right docks.
            // Catch every path that opens Review, not just the top-bar toggle.
            if pane.read(cx).is_open() {
                this.project_panel.update(cx, |panel, cx| panel.close(cx));
            }
            // A full-width Review pane hides the terminal's chat column.
            this.sync_terminal_active(cx);
            cx.notify();
        });
        // Bottom panel: an integrated shell.
        let terminal_panel = cx.new(TerminalPanel::new);
        // Full-page Git panel (Changes / History / Graph).
        let git_panel = cx.new(GitPanel::new);
        // Usage analytics over pi's own session store.
        let usage = cx.new(UsagePage::new);
        // Right dock — the workspace file tree. A row click routes to the app,
        // which opens the Files surface; the panel stays viewer-agnostic.
        let app_weak = cx.entity().downgrade();
        let project_panel = cx.new(|cx| {
            crate::explorer::ProjectPanel::new(
                Rc::new({
                    let app_weak = app_weak.clone();
                    move |path, display, cx: &mut App| {
                        let _ = app_weak
                            .update(cx, |app, cx| app.open_file_in_viewer(path, display, cx));
                    }
                }),
                Rc::new({
                    let app_weak = app_weak.clone();
                    move |request, cx: &mut App| {
                        let _ = app_weak.update(cx, |app, cx| app.on_file_op(request, cx));
                    }
                }),
                Rc::new(move |cx: &mut App| {
                    // The panel closes itself inside its own listener (it already
                    // holds that entity's lease), so this only repaints the
                    // shell. Calling back into `project_panel.update` here would
                    // double-lease the entity and abort.
                    let _ = app_weak.update(cx, |_app, cx| cx.notify());
                }),
                cx,
            )
        });
        // Full-page read-only file viewer.
        let viewer_weak = cx.entity().downgrade();
        let file_viewer = cx.new(|cx| {
            crate::explorer::FileViewer::new(
                Rc::new(move |cx: &mut App| {
                    // The viewer hides itself inside its own listener (it already
                    // holds that entity's lease), so this only repaints the
                    // shell. Calling back into `file_viewer.update` here would
                    // double-lease the entity and abort.
                    let _ = viewer_weak.update(cx, |_app, cx| cx.notify());
                }),
                cx,
            )
        });
        let file_viewer_sub = cx.observe(&file_viewer, |this, viewer, cx| {
            let active = viewer.read(cx).active_display();
            this.project_panel
                .update(cx, |panel, cx| panel.set_active(active, cx));
            cx.notify();
        });

        let workspace_store = load_workspace_store();
        // Worktree preferences and the fields Settings → Worktrees edits.
        // Both fields persist on change; observers only fire on real text
        // edits, so loading never writes back.
        let worktree_config = crate::worktree::WorktreeConfig::load();
        let worktree_dir_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("worktree-dir-input")
                .with_text(worktree_config.directory.clone())
                .with_placeholder_key("worktree.settings.directory_placeholder")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let worktree_dir_sub = cx.observe(&worktree_dir_input, |this, input, cx| {
            let value = input.read(cx).text().trim().to_string();
            if !value.is_empty() && value != this.worktree_config.directory {
                this.worktree_config.directory = value;
                let _ = this.worktree_config.persist();
            }
            cx.notify();
        });
        let worktree_script_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("worktree-script-input")
                .with_text(worktree_config.setup_script.clone())
                .with_placeholder_key("worktree.settings.script_placeholder")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let worktree_script_sub = cx.observe(&worktree_script_input, |this, input, cx| {
            let value = input.read(cx).text().trim().to_string();
            if !value.is_empty() && value != this.worktree_config.setup_script {
                this.worktree_config.setup_script = value;
                let _ = this.worktree_config.persist();
            }
            cx.notify();
        });
        // Create / rename / move dialog fields. The create dialog's start
        // point defaults to HEAD (empty input).
        let worktree_name_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("worktree-name-input")
                .with_placeholder_key("worktree.field.name_placeholder")
                .with_key_context("Composer")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let worktree_branch_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("worktree-branch-input")
                .with_placeholder_key("worktree.field.branch_placeholder")
                .with_key_context("Composer")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let worktree_start_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("worktree-start-input")
                .with_placeholder_key("worktree.field.start_placeholder")
                .with_key_context("Composer")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let worktree_field_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("worktree-field-input")
                .with_key_context("Composer")
                .with_max_lines(1)
                .with_wrap(false)
        });
        // Create dialog: derive the new-branch name from the worktree name
        // until the user edits the branch themselves, so the quick "New
        // worktree" flow is one field to change (the custom name) without
        // silently overwriting a deliberate branch choice.
        let worktree_name_sub = cx.observe(&worktree_name_input, |this, input, cx| {
            if this.worktree_branch_touched {
                return;
            }
            let slug = crate::worktree::slugify(&input.read(cx).text());
            if slug.is_empty() {
                return;
            }
            this.worktree_branch_programmatic = true;
            this.worktree_branch_input
                .update(cx, |field, cx| field.set_text(format!("orbit/{slug}"), cx));
            this.worktree_branch_programmatic = false;
        });
        let worktree_branch_sub = cx.observe(&worktree_branch_input, |this, _, cx| {
            if !this.worktree_branch_programmatic {
                this.worktree_branch_touched = true;
            }
            cx.notify();
        });
        let mut app = Self {
            client,
            runtime,
            rpc_patches,
            sidebar_width: px(crate::layout::sidebar_width()
                .unwrap_or(SIDEBAR_DEFAULT_W)
                .max(SIDEBAR_MIN_W)),
            lives: HashMap::new(),
            pending_parks: Vec::new(),
            transcript: Transcript::new(),
            sessions: sessions::load_sessions(),
            session_watcher: sessions::SessionWatcher::start(),
            workspace_watcher: None,
            workspace_watch_dir: None,
            sidebar_list: ListState::new(0, ListAlignment::Top, px(44.)),
            sidebar_visible: true,
            sidebar_slide_gen: 0,
            feature_open_last: false,
            backdrop_live: false,
            backdrop_clock: None,
            backdrop_phase: 0.,
            pane_full_last: false,
            sidebar_cursor: None,
            sidebar_focus: cx.focus_handle().tab_stop(true),
            input,
            model_label: "…".into(),
            model_id: String::new(),
            model_provider: String::new(),
            thinking_label: "…".into(),
            busy: false,
            queue: PendingQueue::default(),
            restore_queue_on_clear: false,
            follow_up_mode: "one-at-a-time".into(),
            auto_compaction: true,
            auto_retry: true,
            auto_title: crate::auto_title::AutoTitleConfig::load(),
            retention: crate::session_retention::RetentionConfig::load(),
            retention_ran: false,
            temp_files: crate::temp_files::TempFiles::default(),
            session_name: None,
            is_compacting: false,
            retrying: false,
            retry_detail: None,
            error: None,
            toasts: toast::Toasts::new(),
            pending_follow_up: None,
            session_name_input,
            status: connect_error.clone(),
            status_at: (!connect_error.is_empty()).then(Instant::now),
            current_title: None,
            current_workspace: Some(workspace),
            workspace_logo,
            branch: None,
            branch_fetch: 0,
            added: 0,
            removed: 0,
            focus: cx.focus_handle(),
            available_models: Vec::new(),
            available_thinking_levels: Vec::new(),
            model_selector: None,
            command_palette: None,
            dialog: None,
            dialog_focus_pending: false,
            approval: None,
            approval_highlight: 0,
            approval_focus: cx.focus_handle(),
            approval_focus_pending: false,
            ask_tool_id: None,
            ask_questions: Vec::new(),
            ask: None,
            ask_replay: None,
            ask_focus: cx.focus_handle(),
            ask_focus_pending: false,
            extension_widgets: Vec::new(),
            custom_ui: CustomUiSurfaces::default(),
            custom_ui_focus_pending: false,
            custom_ui_supported: false,
            lightbox: None,
            transcript_search: None,
            session_menu: None,
            settings_open: false,
            settings_section: SettingsSection::General,
            settings_select: None,
            settings_filter,
            settings_search: settings_search.clone(),
            _settings_search_sub: settings_search_sub,
            settings_select_highlight: None,
            settings_select_scroll: UniformListScrollHandle::new(),
            bug_report_title,
            bug_report_what,
            bug_report_context,
            bug_report_steps,
            bug_report_kind: crate::issue_message::ReportKind::Bug,
            bug_report_screenshots: Vec::new(),
            bug_report_generating: false,
            bug_report_busy: false,
            bug_report_error: None,
            notification_prefs: notifications::Prefs::load(),
            notification_auth: notifications::DesktopAuth::Unknown,
            notification_auth_pending: false,
            window_active: true,
            activate_window_pending: false,
            _window_watchers: Vec::new(),
            session_history: Vec::new(),
            history_index: 0,
            collapsed_workspaces: HashSet::new(),
            expanded_workspace_groups: HashSet::new(),
            expanded_session_groups: HashMap::new(),
            workspaces: workspace_store.workspaces,
            workspace_added_at: workspace_store.added_at,
            workspace_marks: workspace_store.marks,
            workspace_sort: workspace_store.sort,
            sidebar_group_by: workspace_store.group_by,
            sidebar_archived_filter: workspace_store.archived_filter,
            sidebar_sort_menu: false,
            workspace_menu: None,
            worktree_config,
            worktree_dir_input: worktree_dir_input.clone(),
            _worktree_dir_sub: worktree_dir_sub,
            worktree_script_input: worktree_script_input.clone(),
            _worktree_script_sub: worktree_script_sub,
            worktrees_open: false,
            worktree_howto_dismissed: crate::transcript::hint_seen(
                worktrees::WORKTREE_HOWTO_HINT_KEY,
            ),
            work_in_menu_open: false,
            work_in_menu_highlight: 0,
            work_in_menu_focus: cx.focus_handle(),
            worktrees: Vec::new(),
            worktree_repo_root: None,
            worktrees_busy: false,
            worktree_fetch: 0,
            worktree_refresh_pending: false,
            worktree_mutation_repo: None,
            worktree_operation: None,
            worktrees_error: None,
            worktree_dialog_error: None,
            worktree_advanced_open: false,
            worktree_refresh_due: None,
            worktree_dialog: None,
            worktree_name_input: worktree_name_input.clone(),
            worktree_branch_input: worktree_branch_input.clone(),
            worktree_start_input: worktree_start_input.clone(),
            worktree_field_input: worktree_field_input.clone(),
            _worktree_name_sub: worktree_name_sub,
            _worktree_branch_sub: worktree_branch_sub,
            worktree_branches: Vec::new(),
            worktree_branch_mode: WorktreeBranchMode::New,
            worktree_branch_touched: false,
            worktree_branch_programmatic: false,
            worktree_branch_choice: None,
            worktree_run_setup: true,
            worktree_setup: None,
            worktree_menu: None,
            current_session_path: None,
            menu_dismissed_at: None,
            context: None,
            session_usage: None,
            context_popup: ContextPopup::None,
            autocomplete,
            autocomplete_dismissed: false,
            last_ac_trigger: None,
            slash_commands: Vec::new(),
            mention_files: Vec::new(),
            mention_files_workspace: None,
            attachments: Vec::new(),
            file_drag_hovered: false,
            open_in_apps: Rc::new(Vec::new()),
            open_in_menu_open: false,
            open_in_filter,
            add_menu_open: false,
            add_menu_highlight: 0,
            add_menu_focus: cx.focus_handle(),
            branch_picker: None,
            workspace_picker: None,
            branch_operation_pending: false,
            open_in_prefs: platform::OpenInPrefs::load(),
            open_in_save_task: None,
            _theme_sub: theme_sub,
            _input_sub: input_sub,
            _copy_selection_sub: copy_selection_sub,
            deps,
            pi_install,
            pi_migration_required: None,
            pi_migrating: false,
            setup_open: false,
            host,
            refreshing: false,
            session_details_open: false,
            header_more_open: false,
            title_generating: false,
            rename_saved_at: None,
            quota_popup_open: false,
            quota_popup_tab: mcp_ui::QuotaPopupTab::default(),
            quota_refreshing: false,
            quota_refresh_started: None,
            quota_hidden_open: false,
            hidden_quota_providers: crate::quota_hidden::HiddenProviders::load(),
            session_id: None,
            turn_count: 0,
            turn_open: false,
            latest_turn: None,
            sidepane,
            _sidepane_sub: sidepane_sub,
            project_panel,
            file_viewer,
            _file_viewer_sub: file_viewer_sub,
            terminal_panel,
            git_open: false,
            git_panel: git_panel.clone(),
            usage_open: false,
            usage: usage.clone(),
            custom_providers: Vec::new(),
            custom_providers_error: None,
            provider_auth: HashMap::new(),
            provider_auth_error: None,
            auth: AuthManager::new(),
            quota: QuotaManager::new(),
            extensions,
            access_mode,
            access_menu_open: false,
            access_menu_highlight: 0,
            access_menu_focus: cx.focus_handle(),
            workflow_mode: WorkflowMode::default(),
            workflow_pending: None,
            session_default: crate::session_defaults::SessionDefault::load(),
            default_model_armed: false,
            default_model_pushed: DefaultModelPushed::default(),
            workflow_menu_open: false,
            workflow_menu_highlight: 0,
            workflow_menu_focus: cx.focus_handle(),
            quota_entries_cursor: None,
            quota_entries_inflight: false,
            quota_entries_bootstrap: QUOTA_ENTRY_BOOTSTRAP_POLLS,
            quota_entries_next_poll: Instant::now(),
            provider_auth_dirty: false,
            provider_catalog_counts: HashMap::new(),
            provider_metadata: Vec::new(),
            provider_metadata_loaded: false,
            provider_key_editor: None,
            provider_editor: None,
            provider_remove_confirm: None,
            provider_usage_open: None,
            providers_refreshing: false,
            provider_filter: provider_filter.clone(),
            _provider_filter_sub: provider_filter_sub,
            models_filter: models_filter.clone(),
            _models_filter_sub: models_filter_sub,
            models_favorites_only: false,
            models_provider_filter: None,
            models_refreshing: false,
            skills: Vec::new(),
            plugins: Vec::new(),
            plugins_error: None,
            plugin_source_input: plugin_source_input.clone(),
            plugin_install_project: false,
            plugin_action: None,
            plugin_update_prompt: None,
            plugin_update_workspace: None,
            plugin_updates_checking: false,
            plugin_update_skips: crate::plugins::load_skipped_updates(),
            plugin_refresh_spin_until: None,
            plugin_remove_confirm: None,
            _plugin_source_sub: plugin_source_sub,
            plugins_filter: plugins_filter.clone(),
            _plugins_filter_sub: plugins_filter_sub,
            skills_filter: skills_filter.clone(),
            selected_skill: None,
            skill_content: None,
            skill_delete_confirm: None,
            _skills_filter_sub: skills_filter_sub,
            mcp,
            mcp_filter: mcp_filter.clone(),
            _mcp_filter_sub: mcp_filter_sub,
            mcp_editor: None,
            mcp_list_focus: cx.focus_handle(),
            mcp_focus_pending: false,
            mcp_remove_confirm: None,
            mcp_cursor: None,
            mcp_probe_at,
            mcp_apply_at: None,
            mcp_apply_pending: false,
            mcp_apply_forced: false,
            mcp_stamp,
            mcp_test_target: None,
            mcp_auth: None,
            mcp_external_change: false,
            mcp_external_check_at: None,
            updater_status: cx
                .try_global::<crate::updater::UpdaterState>()
                .and_then(|state| state.0.as_ref())
                .map(|updater| updater.status())
                .unwrap_or_default(),
            updater_version: cx
                .try_global::<crate::updater::UpdaterState>()
                .and_then(|state| state.0.as_ref())
                .and_then(|updater| updater.available_version()),
            updater_notes: cx
                .try_global::<crate::updater::UpdaterState>()
                .and_then(|state| state.0.as_ref())
                .and_then(|updater| updater.available_notes()),
            updater_history: cx
                .try_global::<crate::updater::UpdaterState>()
                .and_then(|state| state.0.as_ref())
                .map(|updater| updater.history())
                .unwrap_or_default(),
            updater_history_open: false,
            updater_dialog: std::env::var_os("ORBIT_OPEN_UPDATE_DIALOG")
                .is_some_and(|value| value == "1")
                .then(|| UpdateDialog::Available {
                    version: "0.0.11".into(),
                    notes: Some(
                        "### Contributors\n\n- **Dumitru Moloșnic** ([#11](https://github.com/imrj05/orbit/pull/11)) — light,\n  dark, and system appearance modes; transcript table sizing, streaming\n  scroll-position, and multiline command-preview fixes."
                            .into(),
                    ),
                    from_check: false,
                }),
            updater_dialog_focus: cx.focus_handle(),
            updater_dialog_focus_pending: false,
            updater_button_hovered: false,
            updater_button_width: Rc::new(Cell::new(updater_ui::UPDATER_PILL_COLLAPSED_W)),
            updater_button_label_reveal: Rc::new(Cell::new(0.)),
            updater_button_animation_from_width: updater_ui::UPDATER_PILL_COLLAPSED_W,
            updater_button_animation_from_reveal: 0.,
            updater_button_animation_generation: 0,
            automatic_updates_enabled: cx
                .try_global::<crate::updater::UpdaterState>()
                .and_then(|state| state.0.as_ref())
                .map(|updater| updater.automatically_checks_for_updates())
                .unwrap_or(false),
        };

        let app_weak = cx.entity().downgrade();
        // The Git page's per-file actions menu opens a file's diff in Review.
        // (Changed-file rows now expand their diff inline on the Changes tab.)
        // The Review pane replaces the Git page, so the diff gets the column
        // and the change list is not left behind.
        let review_sidepane = app.sidepane.clone();
        let review_app = app_weak.clone();
        app.git_panel.update(cx, |panel, _| {
            panel.set_open_file(Rc::new(move |path, _window, cx| {
                review_sidepane.update(cx, |pane, cx| pane.show_file(path, cx));
                let _ = review_app.update(cx, |app, cx| {
                    app.git_open = false;
                    cx.notify();
                });
            }));
        });
        // A conflicted (or history) file on the Git page opens in the Files
        // editor. `open_file_in_viewer` leaves the Git page, which is the
        // intended "resolve it, then continue the merge" flow.
        app.git_panel.update(cx, |panel, _| {
            panel.set_open_path(Rc::new(move |path, display, _window, cx| {
                let _ = app_weak.update(cx, |app, cx| {
                    app.open_file_in_viewer(path, display, cx);
                });
            }));
        });
        // The Git page's Back button leaves the page. The panel closes itself
        // first, so this only clears the app flag (never re-enters the panel).
        let app_weak = cx.entity().downgrade();
        app.git_panel.update(cx, |panel, _| {
            panel.set_on_close(Rc::new(move |_window, cx| {
                let _ = app_weak.update(cx, |app, cx| {
                    app.git_open = false;
                    app.sync_terminal_active(cx);
                    cx.notify();
                });
            }));
        });

        // The Usage page's two callbacks: leave the page, and open a session
        // it names (by pi session id, resolved against the loaded sessions).
        let app_weak = cx.entity().downgrade();
        app.usage.update(cx, |page, _| {
            page.set_on_close(Rc::new(move |_window, cx| {
                let _ = app_weak.update(cx, |app, cx| {
                    app.usage_open = false;
                    app.sync_terminal_active(cx);
                    cx.notify();
                });
            }));
        });
        let app_weak = cx.entity().downgrade();
        app.usage.update(cx, |page, _| {
            page.set_open_session(Rc::new(move |session_id, window, cx| {
                let _ = app_weak.update(cx, |app, cx| {
                    app.open_session_by_id(session_id, window, cx);
                });
            }));
        });
        // Warm the store scan at launch so the page is instant when opened.
        app.usage.update(cx, |page, cx| page.open(cx));

        // The theme observer above only fires on a *change*; the first sync
        // happens in `main`, so the initial paint is already themed.

        if app.client.is_some() {
            app.send(CommandBody::GetState, "get_state");
            app.send(CommandBody::GetAvailableModels, "get_available_models");
            app.send(
                CommandBody::GetAvailableThinkingLevels,
                "get_available_thinking_levels",
            );
            app.send(CommandBody::GetCommands, "get_commands");
            app.probe_auth();
        }
        // The branch chip tracks the launch workspace even before a session
        // is open.
        app.refresh_branch_status(cx);
        // Check for a newer pi in the background; a newer release installs
        // itself through pi's own updater (see `pi_update_ui`).
        app.check_pi_update_on_launch(cx);
        app.check_plugin_updates_on_launch(cx);
        // Route banner clicks back to their session (bundle builds only).
        notifications::init();
        // A first launch with notifications on shows macOS's permission
        // prompt, the way any native app announces them. Unbundled runs
        // no-op here; the settings row reports that state instead.
        if app.notification_prefs.desktop {
            notifications::request_permission();
        }
        // Seed the Review pane and terminal with the launch workspace before
        // the first frame; later changes arrive through
        // [`Self::sync_review_context`].
        app.sync_review_context(cx);
        app
    }

    /// Track whether the window is frontmost — the gate on background
    /// notifications. Registered from `main_window` after the window exists;
    /// `observe_window_activation` invokes the callback once at
    /// registration, so the field starts truthful.
    pub(crate) fn watch_window_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_active = window.is_window_active();
        let activation = cx.observe_window_activation(window, |app, window, _| {
            app.window_active = window.is_window_active();
        });
        // Closing the window ends the observer above on its last value — and
        // the window usually was frontmost when the user closed it — which
        // would keep a run that settles with no window open from notifying.
        let this = cx.weak_entity();
        let closed = cx.on_window_closed(move |cx| {
            this.update(cx, |app, _| app.window_active = false).ok();
        });
        self._window_watchers = vec![activation, closed];
    }

    /// Sidebar view of the session store: the sessions on disk plus a
    /// placeholder row for the open session while pi hasn't flushed its
    /// file yet. pi creates a session's `.jsonl` lazily — only when the
    /// first message is appended — so right after `new_session` the store
    /// holds nothing new and a disk-only list hides the session the user
    /// just started. A draft (nothing sent yet) stays hidden; once the
    /// first prompt lands in the transcript the placeholder shows it
    /// instantly at the top, and it disappears once the real row loads
    /// (same path ⇒ no duplicate).
    ///
    /// Only sessions inside Orbit's own project list reach the sidebar (and
    /// the ⌘P palette); everything else pi has on disk is left where it is.
    pub(super) fn sidebar_sessions(&self) -> Vec<SessionInfo> {
        let mut listed: Vec<SessionInfo> = self
            .sessions
            .iter()
            .filter(|session| {
                let cwd = normalize_workspace_path(&session.cwd.to_string_lossy());
                self.workspaces.contains(&cwd)
            })
            .cloned()
            .collect();
        // An explicit rename lives in memory until the next disk scan; keep
        // the open row in step with the header so the sidebar doesn't lag.
        if let Some(name) = self
            .session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if let Some(path) = &self.current_session_path {
                if let Some(session) = listed.iter_mut().find(|s| &s.path == path) {
                    session.title = name.to_string();
                }
            }
        }
        let first_message = self.transcript.first_user_message();
        sessions_with_placeholder(
            &listed,
            self.current_session_path.as_deref(),
            self.session_name
                .as_deref()
                .or(self.current_title.as_deref()),
            first_message.as_deref(),
            self.current_workspace.as_deref(),
            !self.transcript.is_empty(),
        )
    }

    pub(super) fn workspace_label(&self) -> String {
        self.current_workspace
            .as_ref()
            .map(|p| sessions::workspace_label(p))
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "workspace".into())
            })
    }

    /// Point the app at `cwd` and remember it for the next launch, so the
    /// new-task page opens on the last folder instead of the process cwd
    /// (which is `/` when the app is launched from Finder).
    pub(super) fn set_current_workspace(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        // Resolve symlinks once at the boundary so the active folder, the
        // persisted last-workspace, and every pi session spawned here all
        // share one identity (`/tmp` and `/private/tmp` are not two folders).
        let cwd = sessions::canonical_workspace_path(&cwd);
        persist_last_workspace(&cwd);
        self.workspace_logo = crate::workspace_logo::load(&cwd);
        // A different workspace may belong to a different repository; the
        // cached worktree root (used for name/path resolution) must not leak
        // across, and an in-flight list for the previous workspace must not
        // land. The next worktree list resolves the root again off-thread.
        self.worktree_repo_root = None;
        self.worktree_fetch = self.worktree_fetch.wrapping_add(1);
        self.current_workspace = Some(cwd.clone());
        self.sync_review_context(cx);
    }

    /// Push the workspace / session / turn context the Review pane and the
    /// terminal panel render against. Called from the transitions that change
    /// it — workspace pick, session switch, turn checkpoint — so `render`
    /// stays free of state writes.
    pub(super) fn sync_review_context(&mut self, cx: &mut Context<Self>) {
        let workspace = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok());
        self.sidepane
            .update(cx, |pane, cx| pane.set_workspace(workspace.clone(), cx));
        self.terminal_panel
            .update(cx, |panel, cx| panel.set_workspace(workspace.clone(), cx));
        let provider = self.model_provider.clone();
        let model = self.model_id.clone();
        self.git_panel.update(cx, |panel, cx| {
            panel.set_context(workspace.clone(), provider, model, cx);
        });
        self.project_panel
            .update(cx, |panel, cx| panel.set_workspace(workspace.clone(), cx));
        let session = self.session_id.clone();
        let turn = self.latest_turn;
        self.sidepane.update(cx, |pane, cx| {
            pane.set_turn_context(session.clone(), turn, cx)
        });
        self.git_panel.update(cx, |panel, cx| {
            panel.set_review_turn_context(session, turn, cx)
        });
    }

    /// Whether the terminal panel should poll: visible only while no full-page
    /// surface owns the window and Review is not full-width. Pushed from the
    /// page toggles and the sidepane observer instead of every render.
    pub(super) fn sync_terminal_active(&mut self, cx: &mut Context<Self>) {
        let pane_visible = self.sidepane.read(cx).is_open()
            && !self.settings_open
            && !self.usage_open
            && self.dependencies_ready();
        let pane_full = pane_visible && self.sidepane.read(cx).is_full_width();
        let visible = self.terminal_panel.read(cx).is_open()
            && !self.settings_open
            && !self.usage_open
            && !self.git_open
            && !self.worktrees_open
            && !pane_full;
        self.terminal_panel
            .update(cx, |panel, cx| panel.sync_active(visible, cx));
    }

    /// Add `cwd` to Orbit's project list if it isn't already there. Called
    /// whenever the user picks a folder to work in — starting a task there,
    /// browsing for one, or opening one of its sessions. Never writes to pi.
    pub(super) fn add_workspace(&mut self, cwd: PathBuf, _cx: &App) {
        let cwd = sessions::canonical_workspace_path(&cwd);
        if self.workspaces.iter().any(|w| w == &cwd) {
            return;
        }
        self.workspace_added_at
            .insert(cwd.clone(), SystemTime::now());
        self.workspaces.push(cwd);
        self.persist_workspace_prefs();
    }

    /// Write the sidebar's project list, each project's added-at stamp, the
    /// per-workspace marks, and the active sort mode back to
    /// `workspaces.json`.
    fn persist_workspace_prefs(&self) {
        persist_workspace_store(&WorkspaceStore {
            workspaces: self.workspaces.clone(),
            added_at: self.workspace_added_at.clone(),
            marks: self.workspace_marks.clone(),
            sort: self.workspace_sort,
            group_by: self.sidebar_group_by,
            archived_filter: self.sidebar_archived_filter,
        });
    }

    /// Set (or clear, with [`WorkspaceMark::default`]) a workspace's sidebar
    /// icon and tint, then persist. Orbit-owned: never touches pi.
    pub(super) fn set_workspace_mark(&mut self, cwd: PathBuf, mark: WorkspaceMark) {
        if mark.is_default() {
            self.workspace_marks.remove(&cwd);
        } else {
            self.workspace_marks.insert(cwd, mark);
        }
        self.persist_workspace_prefs();
    }

    /// Drop a project from Orbit's sidebar. pi's session files stay exactly
    /// where they are — only Orbit's own list changes. A symlinked spelling of
    /// the same folder (`/tmp` vs `/private/tmp`, or a legacy duplicate written
    /// before paths were canonicalized) is dropped with it, so one click removes
    /// the folder entirely instead of leaving a second empty heading behind.
    pub(super) fn remove_workspace(&mut self, cwd: &Path, _cx: &App) {
        if drop_workspace(&mut self.workspaces, cwd) {
            // Drop every removed entry's stamp and mark, not just the one that
            // matched by path.
            let remaining: HashSet<PathBuf> = self.workspaces.iter().cloned().collect();
            self.workspace_added_at.retain(|w, _| remaining.contains(w));
            self.workspace_marks.retain(|w, _| remaining.contains(w));
            self.persist_workspace_prefs();
        }
    }

    /// Refetch the status bar's branch chip off-thread: branch name plus
    /// ahead/behind against its upstream. Called at launch, when the workspace
    /// moves, when the workspace watcher reports git state changed (commit,
    /// checkout, fetch), and after an in-app branch switch — render must never
    /// spawn `git`.
    pub(super) fn refresh_branch_status(&mut self, cx: &mut Context<Self>) {
        self.branch_fetch = self.branch_fetch.wrapping_add(1);
        let fetch = self.branch_fetch;
        let cwd = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok());
        let Some(cwd) = cwd else {
            self.branch = None;
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let branch = cx
                .background_executor()
                .spawn(async move {
                    crate::git::current_branch(&cwd).map(|name| BranchStatus {
                        name,
                        ahead_behind: crate::git::ahead_behind(&cwd),
                    })
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                // A newer fetch (workspace move or fresh git state) superseded
                // this one; drop the stale result.
                if app.branch_fetch != fetch {
                    return;
                }
                if app.branch != branch {
                    app.branch = branch;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Focusable for OrbitApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ── sidebar model ──────────────────────────────────────────────────────────

enum SideRow {
    /// Workspace group header: label, session count, collapsed state.
    Workspace {
        label: String,
        count: usize,
        collapsed: bool,
        /// Workspace path — used to spawn a new session in this project.
        cwd: PathBuf,
    },
    /// Session row — index into the (newest-first) sessions list.
    Session(usize),
    /// Reveal the next batch of hidden sessions in a workspace group
    /// (`count` = the step size, at most one
    /// [`SIDEBAR_GROUP_SESSIONS_VISIBLE`]). `can_collapse` adds the
    /// right-side collapse affordance once the group has grown past the
    /// base cap.
    ShowMore {
        label: String,
        count: usize,
        can_collapse: bool,
    },
    /// Collapse a workspace group back to the truncated list.
    ShowLess { label: String },
}

/// How the sidebar orders workspace groups. Persisted alongside the project
/// list in `~/.orbit-pi/workspaces.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum WorkspaceSort {
    /// Insertion order — the order the user added projects to Orbit.
    Manual,
    /// Group label, A→Z (case-insensitive).
    AlphabeticalAsc,
    /// Group label, Z→A (case-insensitive).
    AlphabeticalDesc,
    /// Most recent session activity first.
    #[default]
    LastUpdated,
    /// Most recently added project first.
    DateAdded,
    /// Most sessions first.
    SessionCount,
}

impl WorkspaceSort {
    /// Every mode. The view menu offers only [`Self::MENU`], but the full set
    /// still round-trips through disk — pinned by the sort-store test.
    #[allow(dead_code)]
    pub(crate) const ALL: [WorkspaceSort; 6] = [
        Self::LastUpdated,
        Self::DateAdded,
        Self::AlphabeticalAsc,
        Self::AlphabeticalDesc,
        Self::SessionCount,
        Self::Manual,
    ];

    /// The modes the sidebar view menu offers. Kept minimal (matching the
    /// deepseek-harness menu); the remaining modes still parse from disk for
    /// anyone who set them earlier.
    pub(crate) const MENU: [WorkspaceSort; 2] = [Self::Manual, Self::LastUpdated];

    /// Stable key written to disk. Renaming one needs a migration in
    /// [`Self::from_key`], or the user's choice silently resets.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AlphabeticalAsc => "alphabetical",
            Self::AlphabeticalDesc => "alphabetical_desc",
            Self::LastUpdated => "last_updated",
            Self::DateAdded => "date_added",
            Self::SessionCount => "session_count",
        }
    }

    /// Parse a persisted key. A missing key (a store written before the sort
    /// control existed) and unknown values both fall back to the default,
    /// [`Self::LastUpdated`], matching the sidebar's out-of-the-box order.
    pub(crate) fn from_key(key: &str) -> Self {
        match key {
            "manual" => Self::Manual,
            "alphabetical" => Self::AlphabeticalAsc,
            "alphabetical_desc" => Self::AlphabeticalDesc,
            "last_updated" => Self::LastUpdated,
            "date_added" => Self::DateAdded,
            "session_count" => Self::SessionCount,
            _ => Self::default(),
        }
    }

    /// i18n key for the menu entry.
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Manual => "sidebar.sort_manual",
            Self::LastUpdated => "sidebar.sort_updated",
            Self::DateAdded => "sidebar.sort_added",
            Self::AlphabeticalAsc => "sidebar.sort_alphabetical",
            Self::AlphabeticalDesc => "sidebar.sort_alphabetical_desc",
            Self::SessionCount => "sidebar.sort_count",
        }
    }

    /// Icon shown on the menu entry.
    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            Self::Manual => "icons/task.svg",
            Self::LastUpdated => "icons/clock.svg",
            Self::DateAdded => "icons/plus.svg",
            Self::AlphabeticalAsc => "icons/chevron-up.svg",
            Self::AlphabeticalDesc => "icons/chevron-down.svg",
            Self::SessionCount => "icons/task.svg",
        }
    }
}

/// How the sidebar arranges sessions. `Workspace` groups them under one
/// header per project; `OneList` shows every session in a single flat list.
/// Persisted beside the project list in `workspaces.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SidebarGroupBy {
    #[default]
    Workspace,
    OneList,
}

impl SidebarGroupBy {
    pub(crate) const ALL: [SidebarGroupBy; 2] = [Self::Workspace, Self::OneList];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::OneList => "one_list",
        }
    }

    pub(crate) fn from_key(key: &str) -> Self {
        match key {
            "workspace" => Self::Workspace,
            "one_list" => Self::OneList,
            // A store written while the tree mode existed falls back to the
            // grouped default rather than an unknown state.
            _ => Self::default(),
        }
    }

    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Workspace => "sidebar.group_workspace",
            Self::OneList => "sidebar.group_one_list",
        }
    }

    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            Self::Workspace => "icons/folder.svg",
            Self::OneList => "icons/unified-view.svg",
        }
    }
}

/// Which archived sessions the sidebar lists. `Hide` (the default) drops
/// them, `Show` mixes them back into their places, and `Only` lists just the
/// archived ones. Persisted beside the project list; an Orbit-owned view
/// choice that never touches pi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SidebarArchivedFilter {
    #[default]
    Hide,
    Show,
    Only,
}

impl SidebarArchivedFilter {
    pub(crate) const ALL: [SidebarArchivedFilter; 3] = [Self::Hide, Self::Show, Self::Only];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hide => "hide",
            Self::Show => "show",
            Self::Only => "only",
        }
    }

    pub(crate) fn from_key(key: &str) -> Self {
        match key {
            "hide" => Self::Hide,
            "show" => Self::Show,
            "only" => Self::Only,
            _ => Self::default(),
        }
    }

    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Hide => "sidebar.filter_hide_archived",
            Self::Show => "sidebar.filter_show_archived",
            Self::Only => "sidebar.filter_only_archived",
        }
    }

    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            Self::Hide => "icons/eye-off.svg",
            Self::Show => "icons/archive.svg",
            Self::Only => "icons/archive.svg",
        }
    }
}

/// `~/.orbit-pi/workspaces.json` — the folders Orbit lists in its sidebar,
/// when each was added, and how the sidebar orders them. Orbit-owned: pi owns
/// the session files, this only records which projects the user added.
fn workspaces_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("workspaces.json")
}

/// The parsed workspaces store. Keeping `added_at` beside the path list gives
/// [`WorkspaceSort::DateAdded`] a stable key across relaunches.
#[derive(Debug, Default)]
struct WorkspaceStore {
    workspaces: Vec<PathBuf>,
    added_at: HashMap<PathBuf, SystemTime>,
    marks: HashMap<PathBuf, WorkspaceMark>,
    sort: WorkspaceSort,
    /// How the sidebar groups sessions. A missing key keeps the default
    /// (workspace headers).
    group_by: SidebarGroupBy,
    /// Which archived sessions the sidebar lists.
    archived_filter: SidebarArchivedFilter,
}

/// Read the project list, tolerating the pre-timestamp format where
/// `workspaces` was a bare array of path strings.
fn load_workspace_store() -> WorkspaceStore {
    let Ok(raw) = fs::read_to_string(workspaces_path()) else {
        return WorkspaceStore::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return WorkspaceStore::default();
    };
    parse_workspace_store(&value)
}

/// Parse a workspaces store from JSON — the testable half of
/// [`load_workspace_store`], with the file read factored out.
fn parse_workspace_store(value: &Value) -> WorkspaceStore {
    let mut store = WorkspaceStore {
        sort: value
            .get("sort")
            .and_then(Value::as_str)
            .map(WorkspaceSort::from_key)
            .unwrap_or_default(),
        // `group_by` is the current key; the legacy boolean is honored so a
        // store written during the two-mode experiment still loads.
        group_by: value
            .get("group_by")
            .and_then(Value::as_str)
            .map(SidebarGroupBy::from_key)
            .or_else(|| {
                value
                    .get("group_by_workspace")
                    .and_then(Value::as_bool)
                    .map(|grouped| {
                        if grouped {
                            SidebarGroupBy::Workspace
                        } else {
                            SidebarGroupBy::OneList
                        }
                    })
            })
            .unwrap_or_default(),
        archived_filter: value
            .get("archived_filter")
            .and_then(Value::as_str)
            .map(SidebarArchivedFilter::from_key)
            .unwrap_or_default(),
        ..WorkspaceStore::default()
    };
    let Some(entries) = value.get("workspaces").and_then(Value::as_array) else {
        return store;
    };
    for entry in entries {
        let (path, added_at, mark) = match entry {
            // Legacy format: a bare path string.
            Value::String(raw) => (
                sessions::canonical_workspace_path(Path::new(raw)),
                None,
                WorkspaceMark::default(),
            ),
            // Current format: `{ "path": …, "added_at": <unix secs>,
            // "icon": <stem>, "tint": <key> }`.
            Value::Object(map) => {
                let Some(raw) = map.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let added = map
                    .get("added_at")
                    .and_then(Value::as_u64)
                    .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
                let mark = WorkspaceMark {
                    icon: map.get("icon").and_then(Value::as_str).map(str::to_string),
                    tint: map.get("tint").and_then(Value::as_str).map(str::to_string),
                };
                (
                    sessions::canonical_workspace_path(Path::new(raw)),
                    added,
                    mark,
                )
            }
            _ => continue,
        };
        if store.workspaces.contains(&path) {
            continue;
        }
        let added_at = added_at.unwrap_or_else(|| workspace_created_at(&path));
        store.added_at.insert(path.clone(), added_at);
        if !mark.is_default() {
            store.marks.insert(path.clone(), mark);
        }
        store.workspaces.push(path);
    }
    store
}

/// Best-effort creation time for a workspace with no recorded added-at (a
/// store written before timestamps were tracked). The epoch fallback sorts
/// unknown folders last rather than jumping them to the top.
fn workspace_created_at(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|meta| meta.created())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn persist_workspace_store(store: &WorkspaceStore) {
    let path = workspaces_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let workspaces: Vec<Value> = store
        .workspaces
        .iter()
        .map(|p| {
            let added_at = store
                .added_at
                .get(p)
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mark = store.marks.get(p).cloned().unwrap_or_default();
            // Omit unset mark fields so projects without a custom look keep
            // the store's original two-key shape.
            let mut entry = serde_json::Map::new();
            entry.insert("path".into(), serde_json::json!(p.to_string_lossy()));
            entry.insert("added_at".into(), serde_json::json!(added_at));
            if let Some(icon) = mark.icon {
                entry.insert("icon".into(), serde_json::json!(icon));
            }
            if let Some(tint) = mark.tint {
                entry.insert("tint".into(), serde_json::json!(tint));
            }
            Value::Object(entry)
        })
        .collect();
    let payload = serde_json::json!({
        "workspaces": workspaces,
        "sort": store.sort.as_str(),
        "group_by": store.group_by.as_str(),
        "archived_filter": store.archived_filter.as_str(),
    });
    let _ = fs::write(path, payload.to_string());
}

/// `~/.orbit-pi/last-workspace.json` — the folder the last task ran in. The
/// process cwd is `/` when the app is launched from Finder/Dock, so the
/// new-task page restores this instead of showing `/`. Orbit-owned, like the
/// workspaces list.
fn last_workspace_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("last-workspace.json")
}

/// The remembered folder, if the store is readable and the folder still
/// exists on disk.
fn load_last_workspace() -> Option<PathBuf> {
    let raw = fs::read_to_string(last_workspace_path()).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    let path = sessions::canonical_workspace_path(Path::new(value.get("path")?.as_str()?));
    path.is_dir().then_some(path)
}

fn persist_last_workspace(path: &Path) {
    let file = last_workspace_path();
    if let Some(parent) = file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let path = sessions::canonical_workspace_path(path);
    let payload = serde_json::json!({ "path": path.to_string_lossy() });
    let _ = fs::write(file, payload.to_string());
}

/// A workspace path without a trailing separator, so it compares equal to
/// pi's `cwd` values (which never carry one). An empty remainder (`/`) is
/// kept as-is.
fn normalize_workspace_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim_end_matches(std::path::MAIN_SEPARATOR);
    if trimmed.is_empty() {
        PathBuf::from(raw)
    } else {
        PathBuf::from(trimmed)
    }
}

/// Drop `cwd` (and any symlinked spelling of the same folder) from
/// `workspaces`, returning whether anything was removed. Pure list edit — the
/// caller owns persistence and the per-workspace metadata cleanup, which keeps
/// the removal rule testable without touching disk.
fn drop_workspace(workspaces: &mut Vec<PathBuf>, cwd: &Path) -> bool {
    let target = sessions::canonical_workspace_path(cwd);
    let before = workspaces.len();
    workspaces.retain(|w| w.as_path() != cwd && sessions::canonical_workspace_path(w) != target);
    workspaces.len() != before
}

/// State of the row-actions popup in the sessions sidebar: which session
/// it belongs to (by path), what the row can offer, and whether the popup
/// is currently showing the delete confirmation.
#[derive(Clone, PartialEq)]
struct SessionMenu {
    path: PathBuf,
    /// Copy of the row title for the confirm copy.
    title: String,
    /// Sessions with a live pi process (active or parked) must not be
    /// deleted — the process would recreate the file mid-run.
    deletable: bool,
    /// The popup is showing the delete confirmation instead of the menu.
    confirm_delete: bool,
    /// Right-click origin in window coordinates: the popup floats at the
    /// pointer, context-menu style. `None` anchors it below the row's `…`
    /// button (the pointer-triggered path).
    at: Option<Point<Pixels>>,
}

/// State of the row-actions popup on a workspace group header: which
/// workspace it belongs to (by group label + cwd).
#[derive(Clone, PartialEq)]
struct WorkspaceMenu {
    label: String,
    cwd: PathBuf,
    /// Right-click origin in window coordinates (see [`SessionMenu::at`]).
    at: Option<Point<Pixels>>,
    /// The menu is showing the icon/tint picker instead of the action list.
    picker: bool,
}

/// Sections of the settings surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    General,
    Appearance,
    Shortcuts,
    Privacy,
    Runtime,
    Agent,
    Worktrees,
    Providers,
    Models,
    Skills,
    Plugins,
    Mcp,
    About,
    ReportBug,
}

/// Which branch source the Create Worktree dialog uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeBranchMode {
    /// A brand-new branch, optionally from a start point.
    New,
    /// A branch that already exists (local or remote-tracking).
    Existing,
}

/// A create request held while the user confirms a repository's setup
/// script. Scripts run arbitrary commands, so the first run in a repository
/// is an explicit permission (never a silent side effect of creating).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingWorktreeCreate {
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    /// The branch already exists (check it out) vs. is created here.
    pub existing_branch: bool,
    /// Start point for a new branch; `None` means the repository's HEAD.
    pub start_point: Option<String>,
    pub run_setup: bool,
}

/// The open Worktrees dialog, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeDialog {
    /// Name / branch / location / setup fields.
    Create,
    /// First setup-script run in this repository.
    AllowSetup(PendingWorktreeCreate),
    /// Rename the worktree directory (never the Git branch).
    Rename { path: PathBuf },
    /// Move the worktree directory somewhere else.
    Move { path: PathBuf },
    /// Remove a worktree; `dirty` drives the "uncommitted changes" copy and
    /// the forced-removal confirmation.
    Remove { path: PathBuf, dirty: bool },
    /// The chosen branch already has a worktree elsewhere; offer to open it.
    BranchInUse { branch: String, path: PathBuf },
    /// The workspace a session points at is gone (e.g. its worktree was
    /// removed). The session is never silently re-pointed at another
    /// workspace (§22).
    Unavailable { path: PathBuf },
}

/// Setup-script progress for the most recent worktree creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeSetupState {
    pub worktree: PathBuf,
    pub script: PathBuf,
    /// `None` while the script runs.
    pub outcome: Option<crate::worktree_setup::SetupOutcome>,
    /// Whether the captured output is expanded.
    pub expanded: bool,
    /// Open the worktree when the script succeeds (the create dialog's
    /// "open after creation" setting, deferred until setup finishes).
    pub open_after: bool,
}

/// Open row-actions popup on the Worktrees page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorktreeMenu {
    pub path: PathBuf,
    /// Right-click origin in window coordinates (see [`SessionMenu::at`]).
    pub at: Option<Point<Pixels>>,
}

/// The update modal's state. One surface serves both entry points: the
/// download control starts at [`Self::Available`] (a signed release is
/// already staged), while Check for Updates starts at [`Self::Checking`] and
/// lands on `Available`, `UpToDate`, or `Failed` when the worker reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateDialog {
    /// A check is in flight; the modal shows "Searching for a new version…".
    Checking,
    /// A signed release is staged and ready. `from_check` picks the secondary
    /// button's label: a check offers Cancel, the download control offers
    /// Later.
    Available {
        version: String,
        notes: Option<String>,
        from_check: bool,
    },
    /// The check completed and this build is current.
    UpToDate,
    /// The check or install failed, with the message to show inline.
    Failed(String),
    /// This build can't check for updates itself — a dev run or a binary
    /// outside a managed install. The modal explains instead of toasting.
    Unavailable,
}

/// The provider editor's open state. Inputs are `ComposerInput` entities so
/// they get real text editing (selection, IME, clipboard) for free.
struct ProviderEditor {
    /// Existing provider id, or `None` when adding a new one.
    original_id: Option<String>,
    /// Present in the live catalog — a built-in, or a custom provider pi has
    /// already loaded. Allows saving with an empty model list (an override).
    in_catalog: bool,
    id: Entity<ComposerInput>,
    name: Entity<ComposerInput>,
    base_url: Entity<ComposerInput>,
    api_key: Entity<ComposerInput>,
    models: Entity<ComposerInput>,
    /// Selected API family (chips above the fields).
    api: String,
    /// A key is already stored in models.json (placeholder + hint).
    had_api_key: bool,
    error: Option<String>,
}

/// What a [`ProviderKeyEditor`] is collecting. Ollama Cloud needs both a
/// monthly-credit API key and an optional legacy session; everything else uses
/// the plain API-key form.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderKeyKind {
    ApiKey,
    /// A pasted `Cookie:` header for Ollama Cloud's settings-page usage.
    OllamaCloudSession,
}

/// The API-key editor's open state (one field, provider-scoped).
struct ProviderKeyEditor {
    provider_id: String,
    provider_name: String,
    /// Supports OAuth too, so the modal can offer the sign-in path as well.
    oauth: bool,
    note: &'static str,
    kind: ProviderKeyKind,
    key: Entity<ComposerInput>,
    error: Option<String>,
}

/// API families pi can speak for a custom endpoint, shown as chips.
const PROVIDER_APIS: [&str; 9] = [
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
    "google-generative-ai",
    "mistral-conversations",
    "amazon-bedrock",
    "azure-openai-responses",
    "openai-codex-responses",
    "google-vertex",
];

/// One row of the Providers grid — the built-in catalog merged with the live
/// model catalog, models.json, and auth.json.
struct ProviderView {
    id: String,
    name: String,
    /// Has models in the live catalog (pi is serving it right now).
    active: bool,
    model_count: usize,
    /// Built-in catalog size from pi's bundled data (0 when unknown).
    catalog_count: usize,
    /// Has an entry in models.json (custom, or a built-in override).
    custom: bool,
    has_api_key: bool,
    base_url: String,
    api: String,
    /// pi ships this provider (vs. a models.json-only custom endpoint).
    builtin: bool,
    /// `/login <id>` offers a subscription/OAuth flow.
    oauth: bool,
    /// Storing an API key in auth.json works for this provider.
    api_key: bool,
    /// Environment variables pi reads for this provider's key.
    env_names: Vec<String>,
    /// The one that is actually set in this process, if any.
    env_var: Option<String>,
    note: &'static str,
    /// Credential in auth.json, if any.
    auth: Option<providers::ProviderAuth>,
    /// Live credential facts from the `auth.*` RPC namespace, when pi
    /// supports it. Preferred over the on-disk `auth` snapshot.
    live_status: Option<ProviderStatus>,
    /// Account quota/balance/spend from the `quota.*` RPC namespace, when
    /// available and connected.
    quota: Option<QuotaReport>,
    /// The provider's env var is set in this process's environment.
    env_authed: bool,
}

impl ProviderView {
    /// Any credential present (RPC status, auth.json, or environment).
    fn connected(&self) -> bool {
        self.live_status
            .as_ref()
            .is_some_and(|status| status.authenticated)
            || self.auth.is_some()
            || self.env_authed
    }

    /// The credential kind to display: live RPC status wins over the file.
    fn credential_kind(&self) -> Option<&str> {
        if let Some(status) = &self.live_status {
            if status.authenticated {
                return Some(status.credential.as_str());
            }
        }
        self.auth.map(|auth| match auth.kind {
            providers::AuthKind::OAuth => "oauth",
            providers::AuthKind::ApiKey => "api_key",
            providers::AuthKind::OllamaSession => "session",
        })
    }
}

/// Render an epoch-millisecond expiry as a short local date/time.
fn format_epoch_ms(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(datetime) => datetime.format("%b %-d %H:%M").to_string(),
        None => "—".to_string(),
    }
}

/// Human label for a discovered auth method. pi's own label wins; otherwise a
/// sensible default keyed off the method id (never off a provider id).
fn method_label(id: &str, label: &str, connected: bool) -> String {
    if !label.is_empty() && label != id {
        return label.to_string();
    }
    match id {
        "browser" | "oauth" => {
            if connected {
                tr!("settings.reconnect")
            } else {
                tr!("settings.sign_in")
            }
        }
        "device_code" => tr!("settings.use_device_code"),
        other => {
            let mut chars = other.replace(['_', '-'], " ").chars().collect::<Vec<_>>();
            if let Some(first) = chars.first_mut() {
                first.make_ascii_uppercase();
            }
            chars.into_iter().collect()
        }
    }
}

/// A provider-card button action, dispatched through one handler so the card
/// can build buttons without a closure per action.
#[derive(Clone)]
enum ProviderAction {
    SignIn {
        id: String,
        name: String,
    },
    /// Start a login through the `auth.*` RPC namespace using a discovered
    /// method id (`browser`, `device_code`, …).
    AuthStart {
        id: String,
        name: String,
        method: String,
    },
    /// Cancel the active login for a provider.
    AuthCancel,
    /// Dismiss a finished login card.
    AuthDismiss,
    /// Open a device-code / verification URL in the browser.
    AuthOpenUrl(String),
    /// Copy a value (device code) to the clipboard.
    AuthCopy(String),
    EditKey {
        id: String,
        name: String,
        oauth: bool,
        note: &'static str,
    },
    /// Open the Ollama Cloud session editor (a pasted cookie header) — the
    /// legacy session/weekly path, distinct from the monthly-credit API key.
    /// Carries the provider id that opened it (`ollama` or `ollama-cloud`).
    EditOllamaSession {
        id: String,
        name: String,
    },
    SignOut {
        id: String,
    },
    /// Open the provider's usage/quota popup (windows, balances, spend).
    ShowUsage {
        id: String,
    },
    Configure {
        id: String,
    },
    Remove {
        id: String,
    },
    ConfirmRemove {
        id: String,
    },
    CancelRemove,
    SaveKey,
    Restart,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderButtonStyle {
    Primary,
    Ghost,
    Danger,
}

/// Live state of the active pi agent process, for Settings → Runtime.
#[derive(Default)]
struct RuntimeStatus {
    /// When the active process was adopted (drives the uptime readout).
    started_at: Option<Instant>,
    /// Whether the process is still running.
    alive: bool,
    /// Whether it exited on its own (as opposed to being stopped here).
    exited: bool,
    /// The last spawn failure, if any.
    error: Option<String>,
}

/// The runtime panel's headline state.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeState {
    Running,
    Exited,
    Stopped,
    Failed,
}

/// The `set_model` / `set_thinking_level` already sent to put the active
/// session on the default model, so a follow-up catalog refresh doesn't
/// resend them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DefaultModelPushed {
    session: Option<String>,
    model: Option<(String, String)>,
    thinking: Option<String>,
}

/// Which dropdown is open on the settings surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSelect {
    Theme(ThemeMode),
    Language,
    UiFontSize,
    TerminalFont,
    EditorFont,
    SpacingDensity,
    UiFontFamily,
    CodeFontFamily,
    BackdropCell,
    BackdropFade,
    /// The model the auto-title extension asks (Settings → Agent).
    TitleModel,
    /// The model new sessions start on (Settings → Agent).
    DefaultModel,
    /// The default model's thinking level (Settings → Agent).
    DefaultThinking,
    /// The session's active model (Settings → Models).
    ActiveModel,
    /// The Models page's provider scope filter.
    ModelProvider,
    /// The session-retention action and threshold (Settings → Privacy).
    RetentionMode,
    RetentionDays,
}

// ── feature modules ───────────────────────────────────────────────────────
// `app.rs` keeps the `OrbitApp` model, the shared types, and the controller
// wiring. Rendering and feature-specific logic live in child modules; they
// are descendants of `app`, so they reach private fields/methods directly.
mod ask;
mod bug_report_ui;
mod composer_ops;
mod dialogs;
mod events;
pub(crate) mod helpers;
mod mcp_ui;
mod open_in;
mod pi_update_ui;
mod pickers;
mod plugin_update_ui;
mod retention_ui;
mod runtime;
mod search;
mod session;
mod settings;
mod sidebar;
mod skills_ui;
mod temp_ui;
mod toast_ui;
mod updater_ui;
mod view;
mod worktrees;

#[cfg(test)]
mod backdrop_layout_tests;
#[cfg(test)]
mod composer_layout_tests;
#[cfg(test)]
mod devicons_tests;
#[cfg(test)]
mod error_label_tests;
#[cfg(test)]
mod new_task_panels_tests;
#[cfg(test)]
mod new_task_reconnect_tests;
#[cfg(test)]
mod popup_layout_tests;
#[cfg(test)]
mod refresh_tests;
#[cfg(test)]
mod send_progress_tests;
#[cfg(test)]
mod session_default_apply_tests;
#[cfg(test)]
mod session_park_tests;
#[cfg(test)]
mod sidebar_active_reveal_tests;
#[cfg(test)]
mod sidebar_group_tests;
#[cfg(test)]
mod sidebar_placeholder_tests;
#[cfg(test)]
mod sidebar_remove_tests;
#[cfg(test)]
mod sidebar_sort_tests;
#[cfg(test)]
mod sidepane_full_width_tests;
#[cfg(test)]
mod theme_switch_tests;
#[cfg(test)]
mod titlebar_layout_tests;
#[cfg(test)]
mod window_watch_tests;
#[cfg(test)]
mod worktree_chip_tests;
#[cfg(test)]
mod worktree_howto_tests;

// `icon` and friends are part of the crate-wide UI kit; keep their original
// `crate::app::…` paths stable for the other modules that import them.
pub(crate) use crate::theme::tokens::RaisedExt;
pub(crate) use helpers::{
    button_frame, context_menu_entry, context_menu_separator, context_menu_surface, empty_state,
    file_badge, file_glyph, icon, icon_button_frame, icon_dyn, input_field_frame, menu_header,
    nerd_font_family, picker_entry, picker_search_frame, picker_surface, press,
    provider_mark_color, refresh_glyph, running_spinner, spinner, EmptyFill, TipExt, BUTTON_GROUP,
    PRESS_DIM,
};
use sidebar::sessions_with_placeholder;

#[cfg(test)]
mod catalog_thinking_tests {
    use super::catalog_thinking_levels;
    use serde_json::json;

    #[test]
    fn non_reasoning_model_supports_only_off() {
        assert_eq!(catalog_thinking_levels(&json!({})), vec!["off"]);
        assert_eq!(
            catalog_thinking_levels(&json!({"reasoning": false})),
            vec!["off"]
        );
    }

    #[test]
    fn reasoning_model_without_a_map_gets_the_base_ladder() {
        assert_eq!(
            catalog_thinking_levels(&json!({"reasoning": true})),
            vec!["off", "minimal", "low", "medium", "high"]
        );
    }

    #[test]
    fn map_drops_explicit_nulls_and_gates_extended_levels() {
        // The DeepSeek shape: only low/high/max are mapped; the nulls are
        // unsupported and `xhigh` is not offered at all.
        assert_eq!(
            catalog_thinking_levels(&json!({
                "reasoning": true,
                "thinkingLevelMap": {"off": null, "minimal": null, "low": "low", "medium": null, "high": "high", "max": "max"}
            })),
            vec!["low", "high", "max"]
        );
        // A level the map does not mention stays available (`off` here).
        assert_eq!(
            catalog_thinking_levels(&json!({
                "reasoning": true,
                "thinkingLevelMap": {"minimal": null, "medium": null}
            })),
            vec!["off", "low", "high"]
        );
        // `xhigh`/`max` need a mapping even when nothing else is null.
        assert_eq!(
            catalog_thinking_levels(&json!({
                "reasoning": true,
                "thinkingLevelMap": {"xhigh": "xhigh"}
            })),
            vec!["off", "minimal", "low", "medium", "high", "xhigh"]
        );
    }
}
