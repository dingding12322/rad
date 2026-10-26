//! Right side pane — the workbench's second column: a
//! **Review** panel showing the workspace's git changes.
//!
//! The diff reads from a selectable [`review::Source`] — the last agent turn
//! (from [`crate::checkpoint`] snapshots), uncommitted, unstaged, staged,
//! committed, or the whole branch. The patch is parsed into a virtualized list
//! of rows with sticky file headers and expandable context gaps, colored from
//! [`crate::highlight`] tokens. A filterable changed-files tree sits alongside
//! it and hides on narrow panes.
//!
//! The pane is a GPUI [`Entity`] owned by [`crate::app::OrbitApp`], so it can
//! hold its own list state and background-job state.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, CursorStyle, Entity, FontWeight,
    ListAlignment, ListOffset, ListState, MouseDownEvent, Pixels, Render, ScrollHandle,
    StatefulInteractiveElement, Window,
};

use crate::app::{
    button_frame, context_menu_separator, empty_state, file_glyph, icon, icon_button_frame,
    nerd_font_family, picker_entry, picker_search_frame, picker_surface, refresh_glyph, EmptyFill,
    TipExt, BUTTON_GROUP, PRESS_DIM,
};
use crate::composer::ComposerInput;
use crate::diff_view::{
    diff_gutter_width, render_code_row, render_file_header, render_hunk_header, render_meta,
    render_section_header, render_split_row, SplitPairing,
};
use crate::git;
use crate::review::{self, ExpansionDirection, GapPosition, LineKind, Snapshot, Source};
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{
    context_menu, input, picker, popover, ButtonSize, DynamicSpacing, IconSize, Radius, TextSize,
};
use crate::theme::{self, Theme, ThemeMode};
use crate::usage::tooltip::Tooltip;

/// Pane width defaults / drag clamps.
const PANE_DEFAULT_W: f32 = 460.;
const PANE_MIN_W: f32 = 300.;
/// Ceiling a left-edge drag can reach. Full width is exempt from this cap — it
/// deliberately takes the whole column beside the sidebar.
const PANE_MAX_W: f32 = 900.;
/// Below this width the ±stats collapse out of the toolbar.
const STATS_MIN_PANE_W: f32 = 380.;
/// Below this width the changed-files tree is hidden (responsive).
const TREE_MIN_PANE_W: f32 = 440.;
/// Directory tree column width range.
const TREE_MIN_COL_W: f32 = 180.;
const TREE_MAX_COL_W: f32 = 240.;
/// Review diff row metrics. The row painters live in [`crate::diff_view`], so
/// the pane and the Review page's preview share them; only the gap height is
/// the pane's alone (the page's preview lists whole files).
const REVIEW_GAP_HEIGHT: f32 = 32.;
/// The pane's header and toolbar rows; the dropdowns hang off their buttons.
const PANE_ROW_H: f32 = 40.;

/// How long the Review refresh button spins after a click, so a fast diff
/// read still reads as acknowledged (the same floor Settings uses).
const REFRESH_FEEDBACK: Duration = Duration::from_millis(650);

/// Drag marker for the side-pane resize handle (gpui typed drag state).
pub struct SidePaneResize;

/// Callback an embedded host installs so a review file header carries
/// per-file actions (stage / unstage / discard) beside the path. It is called
/// during render and returns the action row, or `None` for a file with no
/// actions.
pub type FileActions = std::rc::Rc<dyn Fn(&review::File, Theme) -> Option<AnyElement>>;

/// Callback an embedded host installs so a grouped section's strip carries
/// its own bulk action (the Git page's Stage all on Changes, Unstage all on
/// Staged). It is called during render with the section's label and whether
/// the row is the sticky copy, so the host can mint unique element ids.
pub type SectionActions = std::rc::Rc<dyn Fn(&str, bool, Theme) -> Option<AnyElement>>;

pub struct SidePane {
    /// Whether the pane is shown at all (toggled from the top bar).
    open: bool,
    /// Render as an embedded review browser instead of a docked pane: the
    /// chrome (width, resize handle, and the full-width/close controls) is
    /// dropped and width decisions use [`SidePane::available_width`], so the
    /// same view can fill another surface (the Git page's Changes tab).
    embedded: bool,
    /// Pane width in pixels — adjusted by dragging its left edge.
    width: Pixels,

    /// Workspace the pane operates on (kept in sync by the app).
    workspace: Option<PathBuf>,
    /// pi session id — keys the turn checkpoints behind `Last Turn`.
    session: Option<String>,
    /// Latest completed turn for this session, if any.
    latest_turn: Option<usize>,

    // ── Review ──
    review: Option<Arc<Snapshot>>,
    review_loading: bool,
    /// Set when a run settles (or the workspace changes); the diff reloads
    /// next time the pane is visible.
    review_stale: bool,
    review_error: Option<String>,
    /// Manual-refresh feedback: the header button spins until this instant,
    /// so a click is acknowledged even when the diff read finishes instantly.
    refresh_spin_until: Option<Instant>,
    /// Which git snapshot is shown; changing it reloads the diff.
    source: Source,
    /// Monotonic id so a slow load for a previous source can be discarded.
    load_generation: u64,
    /// The source filter dropdown is open.
    source_menu_open: bool,
    /// Guards against the outside-dismiss click's mouse-up re-opening the
    /// menu through the chip.
    menu_dismissed_at: Option<Instant>,
    /// Virtualized diff rows.
    diff_list: ListState,
    /// The unwrapped diff's horizontal scroller. The pane reads it while
    /// rendering to keep the file header fixed at the viewport's edges.
    h_scroll: ScrollHandle,
    /// Diff presentation: wrap long rows and side-by-side layout.
    wrap: bool,
    split: bool,
    /// Files whose diff rows are hidden; only their headers draw. Empty means
    /// every changed file is expanded (the loaded state).
    collapsed_files: HashSet<usize>,
    /// Unified-line → side-by-side mapping, rebuilt whenever the line stream
    /// changes (load, gap expansion, file collapse).
    split_pairing: SplitPairing,
    /// Widest row in monospace cells, for an unwrapped row's scroll range.
    content_cells: usize,
    /// The window width Full width expands to — set by the app every render,
    /// so it tracks a sidebar toggle or a window resize.
    available_width: Pixels,
    /// Per-file actions (stage / unstage / discard) an embedded host installs.
    file_actions: Option<FileActions>,
    /// Bulk actions an embedded host installs on each grouped section strip
    /// (Stage all on Changes, Unstage all on Staged).
    section_actions: Option<SectionActions>,
    /// The width Full width returns to.
    restore_width: Pixels,
    full_width: bool,

    // ── Changed-files tree ──
    /// User toggle (still auto-hidden on narrow panes).
    tree_open: bool,
    tree_filter: Entity<ComposerInput>,
    tree_list: ListState,
    /// Directories explicitly expanded; every directory a fresh snapshot
    /// introduces is auto-expanded.
    expanded_paths: HashSet<String>,
    /// File whose diff is highlighted.
    selected_file: Option<usize>,
    /// A file to select once the next snapshot loads (Git page → Review).
    pending_select: Option<String>,
    /// Cached visible tree rows (kept in step with `tree_list`).
    tree_rows: Vec<review::TreeRow>,
    /// Keyboard cursor within the tree.
    tree_cursor: Option<usize>,
    /// Keyboard cursor for hunk navigation (`]` / `[`) — the diff line the
    /// next/previous hunk search starts from. `None` starts at the selected
    /// file's header.
    diff_cursor: Option<usize>,
    tree_focus: gpui::FocusHandle,
    /// Cached filter text + dirty flag so the tree rebuilds only on change.
    last_tree_filter: String,
    tree_dirty: bool,
}

impl SidePane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let tree_filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("sidepane.filter_files")
                .with_key_context("Composer Picker")
        });
        let width = px(crate::layout::sidepane_width()
            .unwrap_or(PANE_DEFAULT_W)
            .max(PANE_MIN_W));
        Self {
            open: false,
            embedded: false,
            width,
            available_width: width,
            file_actions: None,
            section_actions: None,
            restore_width: width,
            full_width: false,
            workspace: None,
            session: None,
            latest_turn: None,
            review: None,
            review_loading: false,
            review_stale: true,
            review_error: None,
            refresh_spin_until: None,
            source: Source::default(),
            load_generation: 0,
            source_menu_open: false,
            menu_dismissed_at: None,
            diff_list: ListState::new(0, ListAlignment::Top, px(400.)),
            h_scroll: ScrollHandle::new(),
            wrap: true,
            split: false,
            collapsed_files: HashSet::new(),
            split_pairing: SplitPairing::default(),
            content_cells: 0,
            tree_open: true,
            tree_filter,
            tree_list: ListState::new(0, ListAlignment::Top, px(200.)),
            expanded_paths: HashSet::new(),
            selected_file: None,
            pending_select: None,
            tree_rows: Vec::new(),
            tree_cursor: None,
            diff_cursor: None,
            tree_focus: cx.focus_handle().tab_stop(true),
            last_tree_filter: String::new(),
            tree_dirty: true,
        }
    }

    // ── state entry points (called from the app shell) ─────────────────

    /// Show/hide the pane (top-bar toggle).
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        if !self.open {
            self.source_menu_open = false;
        } else if self.review_stale && !self.review_loading {
            self.load_review(cx);
        }
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Mark this instance as an embedded review browser rather than the docked
    /// pane. The caller (the Git page) owns its lifecycle; embedded mode keeps
    /// itself open so `set_workspace` / `mark_review_stale` still load the
    /// diff.
    pub fn set_embedded(&mut self, embedded: bool, cx: &mut Context<Self>) {
        if self.embedded == embedded {
            return;
        }
        self.embedded = embedded;
        if embedded {
            self.source = Source::Uncommitted;
        }
        // The diff loads lazily through the normal `show_review` /
        // `set_workspace` path, not here.
        self.review_stale = true;
        cx.notify();
    }

    /// Install the per-file action row an embedded host wants beside each
    /// changed file's header (stage / unstage / discard). Called whenever the
    /// host's status changes, so the row it builds reflects the latest state.
    pub fn set_file_actions(&mut self, actions: FileActions, cx: &mut Context<Self>) {
        self.file_actions = Some(actions);
        cx.notify();
    }

    /// Install the per-section bulk actions an embedded host wants on the
    /// grouped strips (the Git page's stage-all / unstage-all). Pass `None`
    /// to clear them when the host has no changes to act on.
    pub fn set_section_actions(&mut self, actions: Option<SectionActions>, cx: &mut Context<Self>) {
        self.section_actions = actions;
        cx.notify();
    }

    /// Whether the pane has taken the window's full width (the chat column
    /// and the sessions sidebar yield to it).
    pub fn is_full_width(&self) -> bool {
        self.full_width
    }

    /// Leave Full width and restore the docked width. The app calls this when
    /// a feature page opens or the sessions sidebar is toggled — both need the
    /// window back.
    pub fn leave_full_width(&mut self, cx: &mut Context<Self>) {
        if !self.full_width {
            return;
        }
        self.full_width = false;
        let width = self.restore_width.max(px(PANE_MIN_W));
        self.apply_width(width, cx);
    }

    pub fn width(&self) -> Pixels {
        self.width
    }

    /// The width the pane's responsive decisions (stats, tree) use: its docked
    /// width, or the host's available width when embedded.
    fn pane_width(&self) -> Pixels {
        if self.embedded {
            self.available_width
        } else {
            self.width
        }
    }

    /// Whether the source filter dropdown is open (Escape closes it before
    /// falling through to `abort`).
    pub fn is_source_menu_open(&self) -> bool {
        self.source_menu_open
    }

    /// Close the source filter dropdown (Escape / app-level dismiss).
    pub fn close_source_menu(&mut self, cx: &mut Context<Self>) {
        if self.source_menu_open {
            self.source_menu_open = false;
            self.menu_dismissed_at = Some(Instant::now());
            cx.notify();
        }
    }

    /// Drag-resize from the pane's left edge. A drag leaves Full width and
    /// becomes the width every later restore returns to, clamped to the drag
    /// range. Full width itself is not a drag and is not capped here.
    pub fn set_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        self.full_width = false;
        let clamped = width.clamp(px(PANE_MIN_W), px(PANE_MAX_W));
        if clamped != self.width {
            self.width = clamped;
            self.restore_width = clamped;
            crate::layout::set_sidepane_width(f32::from(clamped));
            cx.notify();
        }
    }

    /// The window width the pane may fill — the app sets it every render, so
    /// Full width tracks a sidebar toggle or a window resize live, and a
    /// docked pane never outgrows the space left beside the sidebar.
    pub fn set_available_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        let width = width.max(px(PANE_MIN_W));
        let changed = (self.available_width - width).abs() >= px(0.5);
        if changed {
            self.available_width = width;
        }
        if self.full_width {
            // Full width fills the ceiling, so it follows a window resize.
            if changed {
                self.apply_width(width, cx);
            }
            return;
        }
        // A window that shrank under a docked pane tightens it to fit; the
        // user's stored width is untouched and comes back with a wider window.
        if self.width > self.available_width {
            self.apply_width(self.available_width, cx);
        }
    }

    fn apply_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if width != self.width {
            self.width = width;
            cx.notify();
        }
    }

    /// The width Full width returns to: the pane's width before the first
    /// transient mode, not the transient width itself.
    fn remember_restore_width(&mut self) {
        if !self.full_width {
            self.restore_width = self.width;
        }
    }

    /// Expand the pane to the window's full width (the chat column and the
    /// sessions sidebar yield to it), or restore the width the reader had
    /// dialed in.
    pub(crate) fn toggle_full_width(&mut self, cx: &mut Context<Self>) {
        if self.full_width {
            self.leave_full_width(cx);
            return;
        }
        self.remember_restore_width();
        self.full_width = true;
        let width = self.available_width.max(px(PANE_MIN_W));
        self.apply_width(width, cx);
    }

    /// Toggle wrapped and unwrapped diff rows.
    fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        cx.notify();
    }

    /// Toggle unified and side-by-side rows. Heights change without a width
    /// change, so the list must re-measure; the reader keeps their line.
    fn toggle_split(&mut self, cx: &mut Context<Self>) {
        self.split = !self.split;
        self.remeasure_diff_rows();
        cx.notify();
    }

    /// Collapse every changed file to its header, or expand them all back.
    fn toggle_all_files(&mut self, cx: &mut Context<Self>) {
        let Some(files) = self.review.as_ref().map(|review| review.files.len()) else {
            return;
        };
        if files == 0 {
            return;
        }
        if self.collapsed_files.is_empty() {
            self.collapsed_files.extend(0..files);
        } else {
            self.collapsed_files.clear();
        }
        self.remeasure_diff_rows();
        cx.notify();
    }

    /// Hide one file's diff rows, or bring them back.
    fn toggle_file(&mut self, file_index: usize, cx: &mut Context<Self>) {
        if !self.collapsed_files.remove(&file_index) {
            self.collapsed_files.insert(file_index);
        }
        self.remeasure_diff_rows();
        cx.notify();
    }

    /// Force the virtualized list to re-measure every row after a height-only
    /// change (collapsing files, switching layouts), keeping the reader's row.
    fn remeasure_diff_rows(&mut self) {
        let top = self.diff_list.logical_scroll_top();
        self.diff_list.reset(self.diff_list.item_count());
        self.diff_list.scroll_to(top);
    }

    /// Keep the selected file's header in view across a line-count change.
    fn scroll_to_selected_file(&mut self) {
        if let Some(line) = self
            .selected_file
            .and_then(|index| self.review.as_ref()?.files.get(index)?.diff_line)
        {
            self.diff_list.scroll_to(ListOffset {
                item_ix: line,
                offset_in_item: px(0.),
            });
        }
    }

    /// Rebuild the split pairing after the line stream changes.
    fn rebuild_split_pairing(&mut self) {
        self.split_pairing = self
            .review
            .as_ref()
            .map_or_else(SplitPairing::default, |review| {
                SplitPairing::for_lines(&review.lines)
            });
    }

    /// Refresh what a view toggle reads: how wide an unwrapped row is and how
    /// unified rows pair into side-by-side ones.
    fn rebuild_view_metrics(&mut self) {
        match self.review.as_ref() {
            Some(review) => self.content_cells = review.max_content_cells(),
            None => self.content_cells = 0,
        }
        self.rebuild_split_pairing();
    }

    /// Keep the pane's workspace in sync with the app (called every frame;
    /// cheap no-op when unchanged). A workspace change invalidates Review.
    pub fn set_workspace(&mut self, workspace: Option<PathBuf>, cx: &mut Context<Self>) {
        if workspace != self.workspace {
            self.workspace = workspace;
            self.review_stale = true;
            if self.open {
                self.load_review(cx);
            }
            cx.notify();
        }
    }

    /// Keep the session id + latest turn in sync so `Last Turn` can resolve.
    pub fn set_turn_context(
        &mut self,
        session: Option<String>,
        latest_turn: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let session_changed = session != self.session;
        let latest_changed = latest_turn != self.latest_turn;
        if !session_changed && !latest_changed {
            return;
        }
        self.session = session;
        self.latest_turn = latest_turn;
        // A session swap can strand the pane on `Last Turn`; fall back to the
        // always-available uncommitted view rather than showing an error.
        if session_changed
            && matches!(self.source, Source::LastTurn { .. })
            && self.last_turn_source().is_none()
        {
            self.source = Source::Uncommitted;
        }
        self.review_stale = true;
        if self.open && !self.review_loading {
            self.load_review(cx);
        }
        cx.notify();
    }

    /// A run settled — Review is stale; reload immediately when visible.
    pub fn mark_review_stale(&mut self, cx: &mut Context<Self>) {
        self.review_stale = true;
        if self.open && !self.review_loading {
            self.load_review(cx);
        }
    }

    /// Close the pane if it is open. Used when the Explorer opens — the two
    /// right docks are mutually exclusive.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.source_menu_open = false;
        // A pane that comes back later must not come back as a full-width
        // sheet; restore the width the reader dialed in.
        self.full_width = false;
        self.width = self.restore_width.max(px(PANE_MIN_W));
        cx.notify();
    }

    /// Open Review on the working tree's **Uncommitted** changes — the
    /// top-bar `+N -M` diff-stat chip's action.
    pub fn show_uncommitted(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        if self.source != Source::Uncommitted {
            self.source = Source::Uncommitted;
            self.source_menu_open = false;
            self.clear_diff();
        }
        self.review_stale = true;
        if !self.review_loading {
            self.load_review(cx);
        }
        cx.notify();
    }

    /// Open Review on the working tree and select `path` — the Git page's
    /// changed-file rows call this so a row opens its diff.
    pub fn show_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.open = true;
        if self.source != Source::Uncommitted {
            self.source = Source::Uncommitted;
            self.clear_diff();
        }
        self.pending_select = Some(path);
        self.review_stale = true;
        if !self.review_loading {
            self.load_review(cx);
        }
        cx.notify();
    }

    /// Open the pane on Review — wired to the transcript's changed-files
    /// cards' "Review" buttons and the command palette.
    pub fn show_review(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        if self.review_stale && !self.review_loading {
            self.load_review(cx);
        }
        cx.notify();
    }

    // ── Review ─────────────────────────────────────────────────────────

    /// Load the workspace's git diff off-thread.
    fn load_review(&mut self, cx: &mut Context<Self>) {
        let cwd = self
            .workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let source = self.source;
        let session = self.session.clone();
        // The embedded Git browser groups the working tree into a Staged
        // section and a Changes (unstaged) section; picking a specific source
        // from the filter narrows it to that one capture instead.
        let grouped = self.embedded && source == Source::Uncommitted;
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        self.review_loading = true;
        self.review_stale = false;
        // Paint the loading state on the click's own frame; without this the
        // spinner can be replaced by the result before it is ever drawn.
        cx.notify();
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    if grouped {
                        let staged = git::collect_review_diff(&cwd, Source::Staged, None)?;
                        let unstaged = git::collect_review_diff(&cwd, Source::Unstaged, None)?;
                        let sections = [
                            review::SectionInput {
                                label: tr!("git_panel.section_staged"),
                                icon: Some("icons/minus.svg"),
                                numstat: &staged.numstat,
                                patch: &staged.patch,
                                complete_context: staged.complete_context,
                            },
                            review::SectionInput {
                                label: tr!("git_panel.section_changes"),
                                icon: Some("icons/plus.svg"),
                                numstat: &unstaged.numstat,
                                patch: &unstaged.patch,
                                complete_context: unstaged.complete_context,
                            },
                        ];
                        return Ok::<_, String>(review::parse_sections(
                            Source::Uncommitted,
                            &sections,
                        ));
                    }
                    let data = git::collect_review_diff(&cwd, source, session.as_deref())?;
                    Ok(review::parse_collected(
                        source,
                        &data.numstat,
                        &data.patch,
                        data.complete_context,
                    ))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // A newer load (e.g. the source changed while this ran) wins.
                if this.load_generation != generation {
                    return;
                }
                this.review_loading = false;
                match parsed {
                    Ok(snapshot) => {
                        this.apply_snapshot(snapshot);
                        this.review_error = None;
                    }
                    Err(error) => {
                        if this.review.is_none() {
                            this.review_error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let previous_dirs = self.review.as_ref().map_or_else(HashSet::new, |snapshot| {
            review::directory_paths(&snapshot.files)
        });
        let previous_path = self
            .selected_file
            .and_then(|index| self.review.as_ref()?.files.get(index))
            .map(|file| file.path.clone());

        let directories = review::directory_paths(&snapshot.files);
        if self.review.is_some() {
            self.expanded_paths
                .retain(|path| directories.contains(path));
            self.expanded_paths
                .extend(directories.difference(&previous_dirs).cloned());
        } else {
            self.expanded_paths = directories;
        }
        let pending = self.pending_select.take();
        self.selected_file = pending
            .as_deref()
            .and_then(|path| snapshot.files.iter().position(|file| file.path == path))
            .or_else(|| {
                previous_path
                    .as_deref()
                    .and_then(|path| snapshot.files.iter().position(|file| file.path == path))
            })
            .or_else(|| (!snapshot.files.is_empty()).then_some(0));
        if let Some(index) = self.selected_file {
            if let Some(line) = snapshot.files.get(index).and_then(|file| file.diff_line) {
                self.diff_list.scroll_to(ListOffset {
                    item_ix: line,
                    offset_in_item: px(0.),
                });
            }
        }
        self.tree_cursor = None;
        self.diff_cursor = None;
        self.diff_list.reset(snapshot.lines.len());
        let file_count = snapshot.files.len();
        self.review = Some(Arc::new(snapshot));
        // The Git page's Changes tab opens collapsed — every file starts as a
        // header row, so the list reads as an overview first. The docked
        // Review pane still opens fully expanded.
        if self.embedded {
            self.collapsed_files = (0..file_count).collect();
        } else {
            self.collapsed_files.clear();
        }
        self.rebuild_view_metrics();
        self.mark_tree_dirty();
    }

    /// Recompute the visible tree rows against the snapshot + filter. The
    /// rebuild only runs when the filter text or tree structure changed, so a
    /// scroll frame is O(visible rows), not O(files).
    fn sync_tree_rows(&mut self, cx: &Context<Self>) {
        let filter = self.tree_filter.read(cx).text();
        if !self.tree_dirty && filter == self.last_tree_filter {
            return;
        }
        self.tree_dirty = false;
        self.last_tree_filter = filter.clone();
        let rows = self.review.as_ref().map_or_else(Vec::new, |snapshot| {
            review::tree_rows(&snapshot.files, &self.expanded_paths, &filter)
        });
        self.set_tree_rows(rows);
    }

    fn mark_tree_dirty(&mut self) {
        self.tree_dirty = true;
    }

    fn set_tree_rows(&mut self, rows: Vec<review::TreeRow>) {
        if rows.len() != self.tree_list.item_count() {
            self.tree_list.reset(rows.len());
        }
        self.tree_rows = rows;
    }

    fn refresh_review(&mut self, cx: &mut Context<Self>) {
        if !self.review_loading {
            self.load_review(cx);
        }
    }

    /// Refresh from the header button: same reload, plus a short minimum spin
    /// so the click is visibly acknowledged even when the diff is instant.
    pub(crate) fn refresh_from_button(&mut self, cx: &mut Context<Self>) {
        self.refresh_review(cx);
        self.refresh_spin_until = Some(Instant::now() + REFRESH_FEEDBACK);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_FEEDBACK).await;
            let _ = this.update(cx, |pane, cx| {
                // A second click extends the floor; only the last timer clears.
                if pane
                    .refresh_spin_until
                    .is_some_and(|until| Instant::now() >= until)
                {
                    pane.refresh_spin_until = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    // ── source filter ──────────────────────────────────────────────────

    fn toggle_source_menu(&mut self, cx: &mut Context<Self>) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(at) = self.menu_dismissed_at.take() {
            if at.elapsed() < GESTURE {
                return;
            }
        }
        self.source_menu_open = !self.source_menu_open;
        cx.notify();
    }

    fn dismiss_source_menu(&mut self, cx: &mut Context<Self>) {
        self.close_source_menu(cx);
    }

    fn set_source(&mut self, source: Source, cx: &mut Context<Self>) {
        self.source_menu_open = false;
        self.menu_dismissed_at = Some(Instant::now());
        if source == self.source {
            cx.notify();
            return;
        }
        self.source = source;
        self.clear_diff();
        self.load_review(cx);
        cx.notify();
    }

    /// Drop the rendered diff + tree so a source change paints a fresh load.
    fn clear_diff(&mut self) {
        self.selected_file = None;
        self.tree_cursor = None;
        self.review = None;
        self.review_error = None;
        self.expanded_paths.clear();
        self.collapsed_files.clear();
        self.rebuild_view_metrics();
        self.mark_tree_dirty();
        self.set_tree_rows(Vec::new());
        self.diff_list.reset(0);
    }

    /// Open Review on the **Last Turn** git diff — what the transcript's
    /// changed-files cards' Review buttons call, so the panel shows only the
    /// changes that turn made rather than the whole working tree.
    pub fn show_review_turn(&mut self, turn: Option<usize>, cx: &mut Context<Self>) {
        self.open = true;
        let target = turn
            .filter(|turn| *turn > 0)
            .map(|turn_count| Source::LastTurn { turn_count });
        match target {
            Some(target) => {
                if self.source != target {
                    self.source = target;
                    self.clear_diff();
                }
            }
            None => {
                // No turn checkpoint yet — fall back to the working tree
                // instead of stranding the panel on an unavailable source.
                if matches!(self.source, Source::LastTurn { .. }) {
                    self.source = Source::Uncommitted;
                    self.clear_diff();
                }
            }
        }
        self.review_stale = true;
        if !self.review_loading {
            self.load_review(cx);
        }
        cx.notify();
    }

    /// The latest completed turn this session can diff (drives the menu's
    /// `Last Turn` availability).
    fn last_turn_source(&self) -> Option<Source> {
        self.latest_turn
            .filter(|turn| *turn > 0)
            .map(|turn_count| Source::LastTurn { turn_count })
    }

    fn source_label(&self, source: Source) -> String {
        match source {
            Source::LastTurn { turn_count } if self.latest_turn == Some(turn_count) => {
                tr!("sidepane.last_turn")
            }
            Source::LastTurn { turn_count } => tr!("sidepane.turn_n", count = turn_count),
            Source::Uncommitted => tr!("sidepane.uncommitted"),
            Source::Unstaged => tr!("git_panel.unstaged"),
            Source::Staged => tr!("git_panel.staged"),
            Source::Committed => tr!("sidepane.committed"),
            Source::Branch => tr!("sidepane.branch"),
        }
    }

    // ── changed-files tree ─────────────────────────────────────────────

    fn toggle_tree(&mut self, cx: &mut Context<Self>) {
        self.tree_open = !self.tree_open;
        cx.notify();
    }

    fn toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.expanded_paths.remove(&path) {
            self.expanded_paths.insert(path);
        }
        self.mark_tree_dirty();
        self.sync_tree_rows(cx);
        cx.notify();
    }

    fn select_file(&mut self, file_index: usize, cx: &mut Context<Self>) {
        self.selected_file = Some(file_index);
        // Hunk navigation restarts from the newly selected file.
        self.diff_cursor = None;
        // Picking a file is an ask to see it; a collapsed row would scroll to
        // its header and show nothing.
        if self.collapsed_files.remove(&file_index) {
            self.remeasure_diff_rows();
        }
        self.scroll_to_selected_file();
        cx.notify();
    }

    fn on_filter_cancel(
        &mut self,
        _: &crate::PickerCancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_source_menu(cx);
        self.tree_filter.update(cx, |input, cx| input.clear(cx));
        self.sync_tree_rows(cx);
        cx.notify();
    }

    /// Move the tree cursor one row (registry-bound ↑/↓).
    pub(super) fn review_tree_move(&mut self, direction: isize, cx: &mut Context<Self>) {
        let rows_len = self.tree_rows.len();
        if rows_len == 0 {
            return;
        }
        let next = if direction > 0 {
            self.tree_cursor.map_or(0, |ix| (ix + 1).min(rows_len - 1))
        } else {
            self.tree_cursor.map_or(0, |ix| ix.saturating_sub(1))
        };
        self.move_tree_cursor(next, cx);
    }

    /// Enter/Space on the cursor row: toggle a directory, open a file.
    pub(super) fn review_tree_toggle(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.tree_cursor {
            match self.tree_rows.get(index).cloned() {
                Some(review::TreeRow::Directory { path, .. }) => self.toggle_dir(path, cx),
                Some(review::TreeRow::File { file_index, .. }) => self.select_file(file_index, cx),
                None => {}
            }
        }
    }

    /// `n` / `p`: select the next or previous changed file.
    pub(super) fn review_move_file(&mut self, direction: isize, cx: &mut Context<Self>) {
        let count = self.review.as_ref().map_or(0, |review| review.files.len());
        if count == 0 {
            return;
        }
        let current = self.selected_file.unwrap_or(0).min(count - 1);
        let next = if direction > 0 {
            (current + 1).min(count - 1)
        } else {
            current.saturating_sub(1)
        };
        self.select_file(next, cx);
    }

    /// `]` / `[`: jump the diff to the next or previous hunk header. The
    /// selection follows the hunk's file so the tree and the diff agree.
    pub(super) fn review_move_hunk(&mut self, direction: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = self.review.as_ref() else {
            return;
        };
        let start = self.diff_cursor.or_else(|| {
            self.selected_file
                .and_then(|ix| snapshot.files.get(ix))
                .and_then(|file| file.diff_line)
        });
        let current = start.unwrap_or(0);
        let target = if direction > 0 {
            (current + 1..snapshot.lines.len()).find(|&ix| is_hunk_start(&snapshot.lines, ix))
        } else {
            (0..current)
                .rev()
                .find(|&ix| is_hunk_start(&snapshot.lines, ix))
        };
        let Some(ix) = target else {
            return;
        };
        self.diff_cursor = Some(ix);
        self.selected_file = snapshot.lines[ix].file_index.into();
        self.diff_list.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    /// `e`: expand every changed file's diff.
    pub(super) fn review_expand_all(&mut self, cx: &mut Context<Self>) {
        if self.review.as_ref().map_or(0, |review| review.files.len()) == 0 {
            return;
        }
        self.collapsed_files.clear();
        self.remeasure_diff_rows();
        cx.notify();
    }

    /// `c`: collapse every changed file to its header.
    pub(super) fn review_collapse_all(&mut self, cx: &mut Context<Self>) {
        let files = self.review.as_ref().map_or(0, |review| review.files.len());
        if files == 0 {
            return;
        }
        self.collapsed_files.extend(0..files);
        self.remeasure_diff_rows();
        cx.notify();
    }

    fn move_tree_cursor(&mut self, index: usize, cx: &mut Context<Self>) {
        self.tree_cursor = Some(index);
        self.tree_list.scroll_to_reveal_item(index);
        cx.notify();
    }

    // ── rendering ──────────────────────────────────────────────────────

    fn body(&mut self, window: &Window, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        // Embedded drops the header row (there is no tree toggle or refresh
        // there) and the changed-files tree: the host surface shows the source
        // toolbar and the diff list, exactly like the Review pane's flat view.
        if !self.embedded {
            self.sync_tree_rows(cx);
        }
        let truncated = self
            .review
            .as_ref()
            .is_some_and(|snapshot| snapshot.truncated);
        let (added, removed) = self
            .review
            .as_ref()
            .map(|snapshot| (snapshot.additions, snapshot.deletions))
            .unwrap_or((0, 0));
        let compact = self.pane_width() < px(STATS_MIN_PANE_W);
        let tree_available = !self.embedded && self.pane_width() >= px(TREE_MIN_PANE_W);
        let tree_visible = tree_available && self.tree_open;

        // Header row: title + tree toggle + refresh + the panel-window
        // controls (full width, close). Embedded drops the panel-window
        // controls — they belong to the dock, not the host surface.
        let mut head = div()
            .h(px(PANE_ROW_H))
            .flex()
            .items_center()
            .gap_1()
            .pl(DynamicSpacing::Base12.px(&theme))
            // The top bar above owns the caption buttons; the dock below it
            // never shares their row.
            .pr(DynamicSpacing::Base06.px(&theme))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("sidepane.review")),
            )
            .children(tree_available.then(|| {
                icon_button_frame(div().id("review-tree-toggle"), &theme, ButtonSize::Default)
                    .group(BUTTON_GROUP)
                    .tip(tr!("sidepane.toggle_tree"))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .active(|s| s.opacity(PRESS_DIM))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_tree(cx)))
                    .child(icon(
                        "icons/folder.svg",
                        ButtonSize::Default.icon_size().px(&theme),
                        if self.tree_open {
                            theme.text_primary
                        } else {
                            theme.text_muted
                        },
                    ))
            }))
            .child(
                icon_button_frame(div().id("review-refresh"), &theme, ButtonSize::Default)
                    .group(BUTTON_GROUP)
                    .tip(tr!("common.refresh"))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .active(|s| s.opacity(PRESS_DIM))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.refresh_from_button(cx);
                    }))
                    .child(refresh_glyph(
                        "review-spinner",
                        ButtonSize::Default.icon_size().px(&theme),
                        self.review_loading || self.refresh_spin_until.is_some(),
                        theme.text_muted,
                        theme,
                    )),
            );

        if !self.embedded {
            head = head
                .child(self.view_toggle(
                    "review-full-width",
                    ButtonSize::Default,
                    if self.full_width {
                        "icons/arrow-shrink.svg"
                    } else {
                        "icons/arrow-expand.svg"
                    },
                    self.full_width,
                    true,
                    if self.full_width {
                        tr!("sidepane.restore_width")
                    } else {
                        tr!("sidepane.full_width")
                    },
                    Self::toggle_full_width,
                    theme,
                    cx,
                ))
                .child(
                    icon_button_frame(div().id("pane-close"), &theme, ButtonSize::Default)
                        .group(BUTTON_GROUP)
                        .tip(tr!("common.close"))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_hover))
                        .active(|s| s.opacity(PRESS_DIM))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close(cx)))
                        .child(icon(
                            "icons/x.svg",
                            ButtonSize::Default.icon_size().px(&theme),
                            theme.text_muted,
                        )),
                );
        }

        // The source filter belongs to the docked Review pane. The Git page's
        // embedded browser always shows the working tree split into Staged and
        // Changes sections, so its chip is dropped.
        let source_chip = (!self.embedded).then(|| {
            button_frame(div().id("review-source"), &theme, ButtonSize::Medium)
                .group(BUTTON_GROUP)
                .border_1()
                .border_color(if self.source_menu_open {
                    theme.border_strong
                } else {
                    theme.border
                })
                .raised(theme.surface_elevated, &theme)
                .cursor_pointer()
                .hover(|s| s.raised(theme.surface_hover, &theme))
                .active(|s| s.opacity(PRESS_DIM))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_source_menu(cx)))
                .child(icon(
                    "icons/file-diff.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .max_w(px(120.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(self.source_label(self.source)),
                )
                .child(icon(
                    "icons/chevron-down.svg",
                    IconSize::XSmall.px(&theme),
                    theme.text_muted,
                ))
                .into_any_element()
        });

        // Toolbar: the source filter chip, the diff view toggles, and the
        // live ±stats.
        let toolbar = div()
            .h(px(PANE_ROW_H))
            .flex()
            .items_center()
            .gap_2()
            .px(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .children(source_chip)
            .child(div().flex_1())
            .when(truncated, |row| {
                row.child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.warning)
                        .child(tr!("sidepane.partial")),
                )
            })
            // An embedded browser gives every grouped section strip its own
            // copy of the view toggles, so its toolbar drops them.
            .children(
                (!self.embedded)
                    .then(|| self.view_toggle_group("review", ButtonSize::Default, theme, cx)),
            )
            .children((!compact).then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .child(div().text_color(theme.success).child(format!("+{added}")))
                    .child(div().text_color(theme.error).child(format!("-{removed}")))
            }));

        let content = self.render_content(window, theme, tree_visible, cx);

        // Embedded drops the dock title row and the toolbar: the host surface
        // already names the view, and every grouped section strip carries the
        // browser controls. Only a truncation warning still needs its own row.
        let mut column = div().flex_1().min_h_0().flex().flex_col();
        if self.embedded {
            if truncated {
                column = column.child(
                    div()
                        .h(px(PANE_ROW_H))
                        .flex()
                        .items_center()
                        .px(DynamicSpacing::Base12.px(&theme))
                        .border_b_1()
                        .border_color(theme.border)
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.warning)
                        .child(tr!("sidepane.partial")),
                );
            }
        } else {
            column = column.child(head).child(toolbar);
        }
        column.child(content).into_any_element()
    }

    /// One icon-only view toggle. The glyph and tooltip name the action, the
    /// ink marks the active mode, and an inert toggle stays visible so the
    /// toolbar never reflows around it.
    #[allow(clippy::too_many_arguments)]
    fn view_toggle(
        &self,
        id: impl Into<gpui::ElementId>,
        size: ButtonSize,
        icon_path: &'static str,
        active: bool,
        enabled: bool,
        tooltip: String,
        action: fn(&mut SidePane, &mut Context<SidePane>),
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = if !enabled {
            theme.text_disabled
        } else if active {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let button = icon_button_frame(div().id(id), &theme, size)
            .group(BUTTON_GROUP)
            .child(icon(icon_path, size.icon_size().px(&theme), color));
        if !enabled {
            return button.into_any_element();
        }
        button
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_hover))
            .active(|style| style.opacity(PRESS_DIM))
            .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| action(this, cx)))
            .into_any_element()
    }

    /// The three view toggles — expand/collapse all, wrap, and split — as one
    /// row. The toolbar hosts them, and an embedded browser's grouped section
    /// strips each carry a copy so both sections offer the same options;
    /// `prefix` keeps every copy's element ids unique, and `size` lets the
    /// strips line their toggles up with the taller labeled actions.
    fn view_toggle_group(
        &self,
        prefix: &str,
        size: ButtonSize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = |slug: &str| gpui::ElementId::Name(format!("{prefix}-{slug}").into());
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                self.view_toggle(
                    id("files"),
                    size,
                    if self.collapsed_files.is_empty() {
                        "icons/collapse-all.svg"
                    } else {
                        "icons/expand-all.svg"
                    },
                    false,
                    self.review
                        .as_ref()
                        .is_some_and(|review| !review.files.is_empty()),
                    if self.collapsed_files.is_empty() {
                        tr!("sidepane.collapse_all")
                    } else {
                        tr!("sidepane.expand_all")
                    },
                    Self::toggle_all_files,
                    theme,
                    cx,
                ),
            )
            .child(self.view_toggle(
                id("wrap"),
                size,
                "icons/text-wrap.svg",
                self.wrap,
                true,
                if self.wrap {
                    tr!("sidepane.unwrap_lines")
                } else {
                    tr!("sidepane.wrap_lines")
                },
                Self::toggle_wrap,
                theme,
                cx,
            ))
            .child(self.view_toggle(
                id("split"),
                size,
                if self.split {
                    "icons/unified-view.svg"
                } else {
                    "icons/side-by-side.svg"
                },
                self.split,
                true,
                if self.split {
                    tr!("sidepane.unified_view")
                } else {
                    tr!("sidepane.split_view")
                },
                Self::toggle_split,
                theme,
                cx,
            ))
            .into_any_element()
    }

    fn render_content(
        &mut self,
        window: &Window,
        theme: Theme,
        tree_visible: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let diff = if self.review_loading && self.review.is_none() {
            empty_state(
                theme,
                "icons/loader.svg",
                &tr!("sidepane.loading_changes"),
                None,
                EmptyFill::Grow,
            )
        } else if let Some(error) = self.review_error.as_deref() {
            empty_state(
                theme,
                "icons/stop.svg",
                &tr!("sidepane.changes_unavailable"),
                Some(error),
                EmptyFill::Grow,
            )
        } else if let Some(snapshot) = self.review.clone() {
            if snapshot.files.is_empty() {
                let empty = self.source.empty_description();
                empty_state(
                    theme,
                    "icons/file-diff.svg",
                    &tr!("sidepane.no_changes"),
                    Some(&empty),
                    EmptyFill::Grow,
                )
            } else {
                self.render_diff(window, snapshot, theme, cx)
            }
        } else {
            empty_state(
                theme,
                "icons/file-diff.svg",
                &tr!("sidepane.no_changes"),
                None,
                EmptyFill::Grow,
            )
        };

        let mut content = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_row()
            .child(diff);
        if tree_visible && self.review.is_some() && self.review_error.is_none() {
            content = content.child(self.render_tree(theme, cx));
        }
        content.into_any_element()
    }

    fn render_diff(
        &self,
        window: &Window,
        snapshot: Arc<Snapshot>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity().downgrade();
        // Unwrapped rows want their natural width. The list gets a minimum
        // equal to the widest row and a scroll container lets the pointer pan
        // it; the file header is counter-shifted by the pan so its name and
        // counts stay fixed at the viewport's edges (see `pinned_row`), and
        // the sticky header sits outside the scroller and never pans.
        let min_w = if self.wrap {
            px(0.)
        } else {
            self.unwrapped_content_width(window, theme)
        };
        let list_el = gpui::list(self.diff_list.clone(), move |index, _window, cx| {
            entity
                .upgrade()
                .map(|entity| entity.update(cx, |this, cx| this.render_diff_line(index, cx)))
                .unwrap_or_else(|| div().into_any_element())
        })
        .h_full()
        .w_full()
        .min_w(min_w);
        let rows = if self.wrap {
            list_el.into_any_element()
        } else {
            div()
                .id("review-diff-scroll")
                .size_full()
                .overflow_scroll()
                .track_scroll(&self.h_scroll)
                .child(list_el)
                .into_any_element()
        };

        let sticky = self.render_sticky_header(&snapshot, theme, cx);
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .relative()
            .overflow_hidden()
            .child(rows)
            .children(sticky)
            .into_any_element()
    }

    /// A row that must not pan with unwrapped content. The file header keeps
    /// its name and counts at the viewport's edges: the row is sized to the
    /// scroller's visible width and counter-shifted by its horizontal offset.
    /// The shift lives on an inner wrapper — the virtualized list paints item
    /// roots at its own origins, so an inset on the root itself is ignored.
    fn pinned_row(&self, row: AnyElement) -> AnyElement {
        if self.wrap {
            return row;
        }
        let offset = self.h_scroll.offset().x;
        let viewport = self.h_scroll.bounds().size.width;
        div()
            .w_full()
            .child(div().w(viewport).left(-offset).child(row))
            .into_any_element()
    }

    /// The horizontal extent an unwrapped row needs: the widest row in
    /// monospace cells at the diff font's advance, plus the row chrome. Split
    /// view gives each half that extent, so it doubles.
    fn unwrapped_content_width(&self, window: &Window, theme: Theme) -> Pixels {
        let cell = crate::diff_view::code_cell_width(window, &theme);
        let row = cell * self.content_cells.max(1) as f32 + px(diff_gutter_width() + 22.);
        if self.split {
            row * 2. + px(1.)
        } else {
            row
        }
    }

    /// The pinned copy of the current file's header. It lives outside the
    /// horizontal scroller, so it never pans: the name and counts stay fixed
    /// at the viewport's edges while the code slides beneath.
    fn render_sticky_header(
        &self,
        snapshot: &Snapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let scroll_top = self.diff_list.logical_scroll_top();
        let mut layers = div().absolute().top_0().left_0().w_full();
        let mut has_layer = false;

        // A grouped list pins its current section divider; the file header
        // then stacks directly beneath it. An ungrouped list keeps only the
        // file header, as before.
        let section = if snapshot.is_grouped() {
            snapshot.section_headers_around(scroll_top.item_ix)
        } else {
            None
        };
        let section_needs = section.is_some_and(|(section_index, _)| {
            section_index < scroll_top.item_ix
                || (section_index == scroll_top.item_ix && scroll_top.offset_in_item > px(0.))
        });
        let section_push = section
            .map(|(_, next_section)| self.sticky_push_offset(next_section))
            .unwrap_or(px(0.));
        // Once the next divider starts pushing this section off, its file
        // header goes with it — pinning one would leave it hovering over the
        // incoming divider and the rows that belong to it.
        let section_leaving = section_push < px(0.);

        // The file header layer is added first so the section divider, added
        // after it, always stays on top while the two overlap mid-transition.
        if !section_leaving {
            if let Some((header_index, next_header_index)) =
                snapshot.file_headers_around(scroll_top.item_ix)
            {
                // Once the section divider is pinned the file header beneath it
                // is covered, so it must pin too even when it sits exactly at
                // the top.
                let file_needs = section_needs
                    || header_index < scroll_top.item_ix
                    || (header_index == scroll_top.item_ix && scroll_top.offset_in_item > px(0.));
                if file_needs {
                    if let Some(line) = snapshot.lines.get(header_index) {
                        let push = self.sticky_push_offset(next_header_index).min(section_push);
                        let section_height = if snapshot.is_grouped() {
                            px(crate::diff_view::REVIEW_SECTION_HEADER_HEIGHT)
                        } else {
                            px(0.)
                        };
                        layers = layers.child(
                            div()
                                .absolute()
                                .top(section_height + push)
                                .left_0()
                                .w_full()
                                .child(self.file_header_row(line.file_index, true, theme, cx)),
                        );
                        has_layer = true;
                    }
                }
            }
        }

        if section_needs {
            if let Some((section_index, _)) = section {
                if let Some(LineKind::SectionHeader {
                    label,
                    icon,
                    count,
                    additions,
                    deletions,
                }) = snapshot.lines.get(section_index).map(|line| &line.kind)
                {
                    let actions = self.section_strip_actions(label, true, theme, cx);
                    layers =
                        layers.child(div().absolute().top(section_push).left_0().w_full().child(
                            render_section_header(
                                label, *icon, *count, *additions, *deletions, actions, theme,
                            ),
                        ));
                    has_layer = true;
                }
            }
        }

        has_layer.then(|| layers.into_any_element())
    }

    /// The action row an embedded host hangs off a grouped section strip.
    /// `sticky` tells the host whether this is the pinned copy, so its
    /// element ids stay unique against the in-list row.
    fn section_action_row(&self, label: &str, sticky: bool, theme: Theme) -> Option<AnyElement> {
        self.section_actions
            .as_ref()
            .and_then(|build| build(label, sticky, theme))
    }

    /// The full action row a grouped section strip carries: the host's bulk
    /// action (stage / unstage all) followed by the same view toggles the
    /// toolbar shows, so both sections offer every option.
    fn section_strip_actions(
        &self,
        label: &str,
        sticky: bool,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut row = div().flex().flex_none().items_center().gap_2();
        if let Some(action) = self.section_action_row(label, sticky, theme) {
            row = row.child(action);
        }
        let prefix = format!(
            "review-section-{label}-{}",
            if sticky { "sticky" } else { "row" }
        );
        Some(
            row.child(self.view_toggle_group(&prefix, ButtonSize::Medium, theme, cx))
                .into_any_element(),
        )
    }

    /// How far a sticky row `next_index` pushes the current one up as it
    /// enters the viewport: zero until it overlaps, then negative by the
    /// overlap. `None` when there is no following row.
    fn sticky_push_offset(&self, next_index: Option<usize>) -> Pixels {
        next_index
            .and_then(|next_index| {
                let bounds = self.diff_list.bounds_for_item(next_index)?;
                let viewport = self.diff_list.viewport_bounds();
                let y_in_viewport = bounds.origin.y - viewport.origin.y;
                (y_in_viewport < bounds.size.height).then_some(y_in_viewport - bounds.size.height)
            })
            .unwrap_or(px(0.))
    }

    /// One changed file's header, made actionable: its chevron says whether
    /// the file's rows follow, and a click toggles them. `sticky` separates
    /// the pinned copy's element id from the in-list row's, so a partially
    /// scrolled header never registers two live handlers on one id.
    fn file_header_row(
        &self,
        file_index: usize,
        sticky: bool,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(file) = self
            .review
            .as_ref()
            .and_then(|review| review.files.get(file_index))
            .cloned()
        else {
            return div().into_any_element();
        };
        let collapsed = self.collapsed_files.contains(&file_index);
        let nerd = nerd_font_family(cx);
        let dark = theme.mode == ThemeMode::Dark;
        let mut row = render_file_header(&file, theme, nerd.as_ref(), dark, collapsed)
            .id(gpui::ElementId::Name(
                format!(
                    "review-file-{}-{file_index}",
                    if sticky { "sticky" } else { "row" }
                )
                .into(),
            ))
            .debug_selector(move || {
                if sticky {
                    "review-file-header-sticky"
                } else {
                    "review-file-header"
                }
                .to_string()
            })
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_hover))
            .on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_file(file_index, cx)),
            );
        // An embedded host (the Git page) can hang stage / unstage / discard
        // actions off the header; they reveal on hover so the row stays clean.
        if self.embedded {
            if let Some(actions) = self
                .file_actions
                .as_ref()
                .and_then(|build| build(&file, theme))
            {
                row = row.group("review-file-header").child(actions);
            }
        }
        row.into_any_element()
    }

    fn render_diff_line(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = self.review.as_ref() else {
            return div().into_any_element();
        };
        let Some(line) = snapshot.lines.get(index) else {
            return div().into_any_element();
        };
        if snapshot.files.get(line.file_index).is_none() {
            return div().into_any_element();
        }
        let theme = *theme::get(cx);
        match &line.kind {
            LineKind::FileHeader => {
                self.pinned_row(self.file_header_row(line.file_index, false, theme, cx))
            }
            // The group divider stays put even while every file beneath it is
            // collapsed, so it is handled before the collapse gate.
            LineKind::SectionHeader {
                label,
                icon,
                count,
                additions,
                deletions,
            } => {
                let actions = self.section_strip_actions(label, false, theme, cx);
                self.pinned_row(
                    render_section_header(
                        label, *icon, *count, *additions, *deletions, actions, theme,
                    )
                    .into_any_element(),
                )
            }
            // A collapsed file keeps its header and nothing else.
            _ if self.collapsed_files.contains(&line.file_index) => {
                div().h(px(0.)).into_any_element()
            }
            LineKind::Gap(gap) => self.render_gap(index, gap.clone(), theme, cx),
            LineKind::HunkHeader => render_hunk_header(&line.content, theme, self.wrap),
            LineKind::Meta => render_meta(&line.content, theme, self.wrap),
            LineKind::Context | LineKind::Addition | LineKind::Deletion => {
                if !self.split {
                    return render_code_row(line, theme, self.wrap);
                }
                if self
                    .split_pairing
                    .secondary
                    .get(index)
                    .copied()
                    .unwrap_or(false)
                {
                    // The paired deletion's row already draws this line; its
                    // own row collapses so the pair reads as one row.
                    return div().h(px(0.)).into_any_element();
                }
                let partner = self.split_pairing.partner.get(index).copied().flatten();
                match (&line.kind, partner) {
                    (LineKind::Addition, _) => render_split_row(None, Some(line), theme, self.wrap),
                    (LineKind::Deletion, Some(partner)) => {
                        render_split_row(Some(line), snapshot.lines.get(partner), theme, self.wrap)
                    }
                    (LineKind::Deletion, None) => {
                        render_split_row(Some(line), None, theme, self.wrap)
                    }
                    // Context rides both halves, numbered per side.
                    _ => render_split_row(Some(line), Some(line), theme, self.wrap),
                }
            }
        }
    }

    fn render_gap(
        &self,
        index: usize,
        gap: review::Gap,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chunked = gap.count() > review::DEFAULT_EXPANSION_LINE_COUNT as u32;
        let directions: &[ExpansionDirection] = match (gap.position, chunked) {
            (GapPosition::Leading, _) => &[ExpansionDirection::End],
            (GapPosition::Trailing, _) => &[ExpansionDirection::Start],
            (GapPosition::Between, false) => &[ExpansionDirection::Both],
            (GapPosition::Between, true) => &[ExpansionDirection::Start, ExpansionDirection::End],
        };
        let expandable = gap.is_expandable();
        let two = directions.len() > 1;
        let mut gutter = div()
            .w(px(46.))
            .h_full()
            .flex_none()
            .flex()
            .when(two, |gutter| gutter.flex_col())
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.overlay);
        if expandable {
            for (button_index, direction) in directions.iter().copied().enumerate() {
                gutter = gutter.child(
                    div()
                        .id(gpui::ElementId::Name(
                            format!("review-gap-{}-{index}-{button_index}", gap.id).into(),
                        ))
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(two && button_index == 0, |button| {
                            button
                                .h(px(16.))
                                .flex_none()
                                .border_b_1()
                                .border_color(theme.border)
                        })
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.overlay_strong))
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            let direction = if event.modifiers().shift {
                                ExpansionDirection::All
                            } else {
                                direction
                            };
                            this.expand_gap(index, direction, cx);
                        }))
                        .child(icon(
                            gap_icon(direction),
                            IconSize::Indicator.px(&theme),
                            theme.text_muted,
                        )),
                );
            }
        }
        let label = div()
            .id(gpui::ElementId::Name(
                format!("review-gap-label-{}", gap.id).into(),
            ))
            .flex_1()
            .h_full()
            .min_w_0()
            .px(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .bg(theme.overlay)
            .when(expandable, |label| {
                label
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.overlay_strong))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        let direction = if event.modifiers().shift {
                            ExpansionDirection::All
                        } else {
                            ExpansionDirection::Both
                        };
                        this.expand_gap(index, direction, cx);
                    }))
            })
            .child(tr!("sidepane.unmodified_lines", count = gap.count()));

        div()
            .h(px(REVIEW_GAP_HEIGHT))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .child(gutter)
            .child(label)
            .into_any_element()
    }

    fn expand_gap(
        &mut self,
        line_index: usize,
        direction: ExpansionDirection,
        cx: &mut Context<Self>,
    ) {
        let expansion = self
            .review
            .as_mut()
            .and_then(|snapshot| Arc::make_mut(snapshot).expand_gap(line_index, direction));
        if let Some(expansion) = expansion {
            self.diff_list
                .splice(line_index..line_index + 1, expansion.replacement_count);
            // The line stream shifted under the pairing, and a cap-limited
            // reveal can leave another gap behind.
            self.rebuild_view_metrics();
            cx.notify();
        }
    }

    fn render_tree(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity().downgrade();
        let tree_el = gpui::list(self.tree_list.clone(), move |index, _window, cx| {
            entity
                .upgrade()
                .map(|entity| entity.update(cx, |this, cx| this.render_tree_row(index, cx)))
                .unwrap_or_else(|| div().into_any_element())
        })
        .size_full()
        .py(DynamicSpacing::Base04.px(&theme));

        let column_w = (f32::from(self.width) * 0.42).clamp(TREE_MIN_COL_W, TREE_MAX_COL_W);
        let filter_row = picker_search_frame(div(), &theme)
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.tree_filter.clone()));

        div()
            .w(px(column_w))
            .flex_none()
            .h_full()
            .min_h_0()
            .border_l_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(filter_row)
            .child(
                div()
                    .id("review-tree")
                    .track_focus(&self.tree_focus)
                    .key_context("ReviewDiffTree")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .on_action(cx.listener(|this, _: &crate::ReviewTreeNext, _, cx| {
                        this.review_tree_move(1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewTreePrev, _, cx| {
                        this.review_tree_move(-1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewTreeToggle, _, cx| {
                        this.review_tree_toggle(cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewFileNext, _, cx| {
                        this.review_move_file(1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewFilePrev, _, cx| {
                        this.review_move_file(-1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewHunkNext, _, cx| {
                        this.review_move_hunk(1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewHunkPrev, _, cx| {
                        this.review_move_hunk(-1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewExpandAll, _, cx| {
                        this.review_expand_all(cx)
                    }))
                    .on_action(cx.listener(|this, _: &crate::ReviewCollapseAll, _, cx| {
                        this.review_collapse_all(cx)
                    }))
                    .child(tree_el),
            )
            .into_any_element()
    }

    fn render_tree_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.tree_rows.get(index).cloned() else {
            return div().h(px(30.)).into_any_element();
        };
        let Some(snapshot) = self.review.as_ref() else {
            return div().h(px(30.)).into_any_element();
        };
        let theme = *theme::get(cx);
        let cursor = self.tree_cursor == Some(index);
        match row {
            review::TreeRow::Directory {
                path,
                name,
                depth,
                expanded,
            } => div()
                .w_full()
                .h(px(30.))
                .px(DynamicSpacing::Base06.px(&theme))
                .flex()
                .items_center()
                .child(
                    div()
                        .id(gpui::ElementId::Name(format!("review-dir-{path}").into()))
                        .h(px(26.))
                        .flex_1()
                        .min_w_0()
                        .pl(px(7. + depth as f32 * 14.))
                        .pr(px(7.))
                        .rounded(Radius::Medium.px(&theme))
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .cursor_pointer()
                        .when(cursor, |row| row.bg(theme.overlay_strong))
                        .when(!cursor, |row| row.hover(|row| row.bg(theme.surface_hover)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.tree_focus);
                            this.tree_cursor = Some(index);
                            this.toggle_dir(path.clone(), cx);
                        }))
                        .child(icon(
                            if expanded {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/chevron-right.svg"
                            },
                            IconSize::Indicator.px(&theme),
                            theme.text_muted,
                        ))
                        .child(icon(
                            "icons/folder.svg",
                            IconSize::Small.px(&theme),
                            theme.text_muted,
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(TextSize::Small.px(&theme))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(name),
                        ),
                )
                .into_any_element(),
            review::TreeRow::File { file_index, depth } => {
                let Some(file) = snapshot.files.get(file_index) else {
                    return div().h(px(30.)).into_any_element();
                };
                let name = file
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&file.path)
                    .to_owned();
                let selected = self.selected_file == Some(file_index);
                let (status, status_color) = match file.status {
                    review::FileStatus::Added => ("A", theme.success),
                    review::FileStatus::Deleted => ("D", theme.error),
                    review::FileStatus::Binary => ("B", theme.warning),
                    review::FileStatus::Modified => ("M", theme.warning),
                };
                let nerd = nerd_font_family(cx);
                let dark = theme.mode == ThemeMode::Dark;
                let fallback = icon(
                    "icons/file.svg",
                    IconSize::Small.px(&theme),
                    theme.text_muted,
                )
                .into_any_element();
                let glyph = file_glyph(
                    &file.path,
                    dark,
                    nerd.as_ref(),
                    IconSize::Small.px(&theme),
                    fallback,
                );
                div()
                    .w_full()
                    .h(px(30.))
                    .px(DynamicSpacing::Base06.px(&theme))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id(gpui::ElementId::Name(
                                format!("review-file-{file_index}").into(),
                            ))
                            .h(px(26.))
                            .flex_1()
                            .min_w_0()
                            .pl(px(23. + depth as f32 * 14.))
                            .pr(px(7.))
                            .rounded(Radius::Medium.px(&theme))
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base06.px(&theme))
                            .cursor_pointer()
                            .when(selected, |row| row.bg(theme.selection))
                            .when(!selected && cursor, |row| row.bg(theme.surface_hover))
                            .when(!selected && !cursor, |row| {
                                row.hover(|row| row.bg(theme.surface_hover))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.tree_focus);
                                this.tree_cursor = Some(index);
                                this.select_file(file_index, cx);
                            }))
                            .child(glyph)
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(if selected {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(name),
                            )
                            .child(
                                div()
                                    .w(px(18.))
                                    .h(px(18.))
                                    .flex_none()
                                    .rounded(Radius::Small.px(&theme))
                                    .border_1()
                                    .border_color(status_color.opacity(0.65))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(TextSize::Small.px(&theme))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(status_color)
                                    .child(status),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    /// The source filter dropdown, painted over the pane body.
    fn source_menu(&self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.source_menu_open || self.embedded {
            return None;
        }
        let mut menu = picker_surface(div().id("review-source-menu"), &theme)
            .absolute()
            // Below the toolbar's source button.
            .top(menu_top(
                // Embedded drops the head row, so the toolbar is flush with the
                // pane top; docked panes push it down by the head row's height
                // plus its 1px bottom border.
                if self.embedded { 0. } else { 1. + PANE_ROW_H },
                ButtonSize::Medium,
                &theme,
            ))
            .left(px(10.))
            .w(px(200.))
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base01.px(&theme))
            .occlude()
            .on_mouse_down_out(
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.dismiss_source_menu(cx)),
            );

        let last_turn = self.last_turn_source();
        menu = menu.child(source_row(
            &tr!("sidepane.last_turn"),
            last_turn.is_some(),
            last_turn.is_some() && last_turn == Some(self.source),
            last_turn,
            theme,
            cx,
        ));
        menu = menu.child(separator(theme));
        for choice in [Source::Uncommitted, Source::Unstaged, Source::Staged] {
            menu = menu.child(source_row(
                &self.source_label(choice),
                true,
                choice == self.source,
                Some(choice),
                theme,
                cx,
            ));
        }
        menu = menu.child(separator(theme));
        for choice in [Source::Committed, Source::Branch] {
            menu = menu.child(source_row(
                &self.source_label(choice),
                true,
                choice == self.source,
                Some(choice),
                theme,
                cx,
            ));
        }
        Some(menu.into_any_element())
    }
}

fn source_row(
    label: &str,
    enabled: bool,
    selected: bool,
    choice: Option<Source>,
    theme: Theme,
    cx: &mut Context<SidePane>,
) -> AnyElement {
    let row = picker_entry(
        div().id(gpui::ElementId::Name(
            format!("review-source-{label}").into(),
        )),
        &theme,
    )
    .h(picker::entry_height(&theme))
    .flex_none()
    .when(enabled, |row| row.cursor_pointer())
    .when(selected, |row| row.bg(theme.selection))
    .when(enabled && !selected, |row| {
        row.hover(|s| s.bg(theme.surface_hover))
    })
    .when(enabled && choice.is_some(), |row| {
        row.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            if let Some(choice) = choice {
                this.set_source(choice, cx);
            }
        }))
    })
    .child(
        div()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(if !enabled {
                theme.text_muted
            } else if selected {
                theme.text_primary
            } else {
                theme.text_secondary
            })
            .child(label.to_string()),
    )
    .when(!enabled, |row| {
        // Zed's `ml_4` before a trailing hint, less the row gap already there.
        row.child(
            div()
                .flex_none()
                .ml(context_menu::keybinding_gap(&theme) - context_menu::icon_gap(&theme))
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("sidepane.no_turns_yet")),
        )
    })
    .when(selected, |row| {
        row.child(icon(
            "icons/check.svg",
            context_menu::ICON.px(&theme),
            theme.accent_text,
        ))
    });
    row.into_any_element()
}

fn separator(theme: Theme) -> AnyElement {
    context_menu_separator(&theme).into_any_element()
}

/// The top of a dropdown hung from a button centered in the pane row that
/// starts `row_top` px below the pane: the button's bottom edge plus Zed's
/// menu offset.
fn menu_top(row_top: f32, trigger: ButtonSize, theme: &Theme) -> Pixels {
    px(row_top + PANE_ROW_H / 2.) + trigger.height(theme) * 0.5 + popover::MENU_OFFSET
}

// ── diff row rendering ─────────────────────────────────────────────────────

fn gap_icon(direction: ExpansionDirection) -> &'static str {
    match direction {
        // A leading gap reveals context below it; a trailing gap above.
        ExpansionDirection::Start | ExpansionDirection::Both | ExpansionDirection::All => {
            "icons/chevron-down.svg"
        }
        ExpansionDirection::End => "icons/chevron-up.svg",
    }
}

// ── shared helpers ─────────────────────────────────────────────────────────

impl Source {
    /// Reader-facing empty-state copy for each source.
    fn empty_description(self) -> String {
        match self {
            Source::LastTurn { .. } => tr!("sidepane.empty_last_turn"),
            Source::Uncommitted => tr!("sidepane.empty_uncommitted"),
            Source::Unstaged => tr!("sidepane.empty_unstaged"),
            Source::Staged => tr!("sidepane.empty_staged"),
            Source::Committed => tr!("sidepane.empty_committed"),
            Source::Branch => tr!("sidepane.empty_branch"),
        }
    }
}

impl Render for SidePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Embedded: paint the browser directly, filling its host surface.
        if self.embedded {
            let theme = *theme::get(cx);
            let body = self.body(window, theme, cx);
            return div()
                .id("embedded-review")
                .debug_selector(|| "embedded-review".to_string())
                .relative()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full()
                .bg(theme.background)
                .flex()
                .flex_col()
                .child(body)
                .children(self.source_menu(theme, cx))
                .on_action(cx.listener(Self::on_filter_cancel))
                .into_any_element();
        }
        if !self.open {
            return div().into_any_element();
        }
        let theme = *theme::get(cx);
        let body = self.body(window, theme, cx);
        div()
            .id("side-pane")
            .debug_selector(|| "side-pane".to_string())
            .relative()
            .flex_none()
            .w(self.width)
            .h_full()
            .pl(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .id("side-pane-resize-handle")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    // Sit on the card's left border (the 12px gutter, minus
                    // half the handle width), not the panel's outer edge, so
                    // the edge users see is the edge they can grab.
                    .left(px(9.))
                    .w(px(6.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|style| style.bg(theme.accent.opacity(0.4)))
                    .on_drag(SidePaneResize, |_, _, _, cx| cx.new(|_| DragGhost)),
            )
            // The dock reads as a rounded card, like the Git/Usage cards in
            // the main column. The menus stay outside this clipped shell so
            // they are not cut off at its corners.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Radius::XLarge.px(&theme))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(body),
            )
            .children(self.source_menu(theme, cx))
            .on_action(cx.listener(Self::on_filter_cancel))
            .into_any_element()
    }
}

/// An invisible drag ghost — resizing leaves no floating preview.
struct DragGhost;

/// Whether `ix` begins a hunk: an explicit hunk header, or the first
/// addition/deletion of a change run. Full-context snapshots carry no
/// `HunkHeader` rows, so the run boundary is the signal both shapes share.
fn is_hunk_start(lines: &[review::Line], ix: usize) -> bool {
    match lines[ix].kind {
        LineKind::HunkHeader => true,
        LineKind::Addition | LineKind::Deletion => {
            let previous_is_change = ix > 0
                && lines[ix - 1].file_index == lines[ix].file_index
                && matches!(lines[ix - 1].kind, LineKind::Addition | LineKind::Deletion);
            !previous_is_change
        }
        _ => false,
    }
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeId;
    use gpui::point;

    fn pane(cx: &mut gpui::TestAppContext) -> Entity<SidePane> {
        cx.update(|cx| {
            cx.set_global(Theme::for_id(ThemeId::Orbit));
            cx.new(SidePane::new)
        })
    }

    /// A 10-line file with one change, so collapsing leaves a leading and a
    /// trailing gap.
    fn gap_patch() -> String {
        let mut patch = String::from(
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             index 1111111..2222222 100644\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,10 +1,10 @@\n",
        );
        for line in 1..=5 {
            patch.push_str(&format!(" context line {line}\n"));
        }
        patch.push_str("-change();\n+changed();\n");
        for line in 6..=10 {
            patch.push_str(&format!(" context line {line}\n"));
        }
        patch
    }

    #[gpui::test]
    fn full_width_and_dock_share_one_restore_point(cx: &mut gpui::TestAppContext) {
        let pane = pane(cx);
        cx.update(|cx| {
            pane.update(cx, |pane, cx| {
                pane.set_available_width(px(1200.), cx);
                pane.set_width(px(500.), cx);

                pane.toggle_full_width(cx);
                assert_eq!(pane.width(), px(1200.));
                assert!(pane.full_width);

                // `leave_full_width` — the app's feature-page and sidebar
                // path — docks back to the width the reader dialed in.
                pane.leave_full_width(cx);
                assert_eq!(pane.width(), px(500.));
                assert!(!pane.full_width);

                // While full width, the pane tracks a resized window.
                pane.toggle_full_width(cx);
                pane.set_available_width(px(900.), cx);
                assert_eq!(pane.width(), px(900.));

                // Docked, a window that shrinks tightens the pane to fit.
                pane.leave_full_width(cx);
                assert_eq!(pane.width(), px(500.));
                pane.set_available_width(px(420.), cx);
                assert_eq!(pane.width(), px(420.));
            });
        });
    }

    /// A left-edge drag cannot grow past the drag ceiling. Full width is a
    /// separate path with no such cap — it deliberately takes the whole
    /// column beside the sidebar.
    #[gpui::test]
    fn a_drag_resize_is_capped_but_full_width_is_not(cx: &mut gpui::TestAppContext) {
        let pane = pane(cx);
        cx.update(|cx| {
            pane.update(cx, |pane, cx| {
                pane.set_available_width(px(1400.), cx);
                pane.set_width(px(1200.), cx);
                assert_eq!(
                    pane.width(),
                    px(PANE_MAX_W),
                    "a drag is capped at the drag ceiling"
                );

                pane.toggle_full_width(cx);
                assert_eq!(pane.width(), px(1400.));
                assert!(pane.full_width);
            });
        });
    }

    #[gpui::test]
    fn collapse_all_hides_every_file_body_and_expand_all_restores_them(
        cx: &mut gpui::TestAppContext,
    ) {
        let pane = pane(cx);
        let snapshot = crate::review::parse_collected(
            Source::Uncommitted,
            "1\t1\tsrc/lib.rs\n",
            &gap_patch(),
            true,
        );
        let files = snapshot.files.len();
        cx.update(|cx| {
            pane.update(cx, |pane, cx| {
                pane.apply_snapshot(snapshot);
                assert!(pane.collapsed_files.is_empty());

                pane.toggle_all_files(cx);
                assert_eq!(pane.collapsed_files.len(), files);

                pane.toggle_all_files(cx);
                assert!(pane.collapsed_files.is_empty());
            });
        });
    }

    #[gpui::test]
    fn review_keyboard_navigation_moves_between_files(cx: &mut gpui::TestAppContext) {
        let pane = pane(cx);
        let patch = format!(
            "{}\n\
             diff --git a/src/other.rs b/src/other.rs\n\
             index 3333333..4444444 100644\n\
             --- a/src/other.rs\n\
             +++ b/src/other.rs\n\
             @@ -1,3 +1,3 @@\n\
             -old();\n\
             +new();\n",
            gap_patch()
        );
        let snapshot = crate::review::parse_collected(
            Source::Uncommitted,
            "1\t1\tsrc/lib.rs\n1\t1\tsrc/other.rs\n",
            &patch,
            true,
        );
        assert_eq!(
            snapshot.files.len(),
            2,
            "fixture must hold two changed files"
        );
        cx.update(|cx| {
            pane.update(cx, |pane, cx| {
                pane.apply_snapshot(snapshot);
                pane.review_move_file(1, cx);
                assert_eq!(pane.selected_file, Some(1));
                pane.review_move_file(1, cx);
                assert_eq!(pane.selected_file, Some(1), "clamps at the last file");
                pane.review_move_file(-1, cx);
                assert_eq!(pane.selected_file, Some(0));

                pane.review_collapse_all(cx);
                assert_eq!(pane.collapsed_files.len(), 2);
                pane.review_expand_all(cx);
                assert!(pane.collapsed_files.is_empty());

                // Hunk navigation follows the diff into the next file.
                pane.select_file(0, cx);
                pane.review_move_hunk(1, cx);
                assert_eq!(pane.selected_file, Some(0));
                pane.review_move_hunk(1, cx);
                assert_eq!(pane.selected_file, Some(1));
            });
        });
    }

    #[gpui::test]
    fn one_file_collapses_independently_and_selecting_it_expands_it(cx: &mut gpui::TestAppContext) {
        let pane = pane(cx);
        let snapshot = crate::review::parse_collected(
            Source::Uncommitted,
            "1\t1\tsrc/lib.rs\n",
            &gap_patch(),
            true,
        );
        cx.update(|cx| {
            pane.update(cx, |pane, cx| {
                pane.apply_snapshot(snapshot);

                pane.toggle_file(0, cx);
                assert!(pane.collapsed_files.contains(&0));

                // Picking the file in the tree is an ask to see its diff.
                pane.select_file(0, cx);
                assert!(pane.collapsed_files.is_empty());
            });
        });
    }

    #[gpui::test]
    fn wrap_and_split_toggles_flip_their_state(cx: &mut gpui::TestAppContext) {
        let pane = pane(cx);
        cx.update(|cx| {
            pane.update(cx, |pane, cx| {
                assert!(pane.wrap);
                pane.toggle_wrap(cx);
                assert!(!pane.wrap);

                assert!(!pane.split);
                pane.toggle_split(cx);
                assert!(pane.split);
            });
        });
    }

    /// Every view mode lays out its real element tree without panicking —
    /// split rows, collapsed files, and the unwrapped scroller only exist at
    /// render time.
    #[gpui::test]
    fn every_view_mode_draws(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
        let cx = cx.add_empty_window();
        let pane = cx.update(|_, cx| cx.new(SidePane::new));
        cx.update(|_, cx| {
            pane.update(cx, |pane, cx| {
                pane.open = true;
                pane.set_available_width(px(900.), cx);
                pane.apply_snapshot(crate::review::parse_collected(
                    Source::Uncommitted,
                    "1\t1\tsrc/lib.rs\n",
                    &gap_patch(),
                    true,
                ));
            });
        });
        let viewport = || gpui::size(px(900.), px(800.));
        for (wrap, split, collapse) in [
            (true, false, false),
            (false, false, false),
            (true, true, false),
            (false, true, false),
            (true, true, true),
            (false, true, true),
        ] {
            cx.update(|_, cx| {
                pane.update(cx, |pane, cx| {
                    pane.wrap = wrap;
                    pane.split = split;
                    let should_collapse = collapse && pane.collapsed_files.is_empty();
                    if should_collapse || (!collapse && !pane.collapsed_files.is_empty()) {
                        pane.toggle_all_files(cx);
                    }
                    cx.notify();
                });
            });
            let _ = cx.draw(point(px(0.), px(0.)), viewport(), |_, _| pane.clone());
        }
    }

    /// Unwrapped text keeps the file header fixed: its name and change counts
    /// stay at the viewport's edges while only the code pans sideways beneath.
    #[gpui::test]
    fn unwrapped_file_header_stays_fixed_while_code_pans(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
        let cx = cx.add_empty_window();
        let pane = cx.update(|_, cx| cx.new(SidePane::new));
        let long = "x".repeat(200);
        let patch = format!(
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             index 1111111..2222222 100644\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,3 +1,3 @@\n\
              context one\n\
             -old line\n\
             +{long}\n\
              context two\n"
        );
        cx.update(|_, cx| {
            pane.update(cx, |pane, cx| {
                pane.open = true;
                pane.set_width(px(460.), cx);
                pane.wrap = false;
                pane.apply_snapshot(crate::review::parse_collected(
                    Source::Uncommitted,
                    "1\t1\tsrc/lib.rs\n",
                    &patch,
                    false,
                ));
            });
        });
        let viewport = || gpui::size(px(900.), px(700.));
        let _ = cx.draw(point(px(0.), px(0.)), viewport(), |_, _| pane.clone());

        let scroller = cx.update(|_, cx| pane.read(cx).h_scroll.bounds());
        assert!(
            scroller.size.width < px(700.),
            "the scroller is the pane-sized viewport, got {:?}",
            scroller.size.width
        );
        let header = cx
            .debug_bounds("review-file-header")
            .expect("the in-list header is laid out");
        assert!(
            (header.size.width - scroller.size.width).abs() < px(1.),
            "the header spans the visible pane, not the wide content: {:?} vs {:?}",
            header.size.width,
            scroller.size.width
        );

        // Pan the content right: the offset moves, the header must not.
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: point(px(200.), px(400.)),
            delta: gpui::ScrollDelta::Pixels(point(px(-240.), px(0.))),
            ..Default::default()
        });
        let _ = cx.draw(point(px(0.), px(0.)), viewport(), |_, _| pane.clone());
        let offset = cx.update(|_, cx| pane.read(cx).h_scroll.offset().x);
        assert!(
            offset < px(0.),
            "the scroller must pan right, got {offset:?}"
        );
        let after = cx
            .debug_bounds("review-file-header")
            .expect("the in-list header is still laid out");
        assert!(
            (after.origin.x - header.origin.x).abs() < px(1.),
            "the header is fixed: {:?} -> {:?}",
            header.origin.x,
            after.origin.x
        );
    }
}
