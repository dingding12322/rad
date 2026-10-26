//! The Explorer's project panel: a Zed-style workspace tree in a right dock.
//!
//! The panel owns the built [`TreeIndex`], the expand/collapse set, the filter
//! input, and its virtualized list. It never reads the filesystem on the
//! render path: [`ProjectPanel::set_workspace`] / [`ProjectPanel::mark_stale`]
//! kick a background snapshot, and rendering only walks the in-memory tree.
//!
//! Clicking a file calls the [`OpenHandler`] the app installed at
//! construction — the panel does not know about the file viewer.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, AnyElement, App, ClickEvent, Context, CursorStyle, Entity, FocusHandle,
    FontWeight, Hsla, KeyDownEvent, ListAlignment, ListState, MouseButton, MouseDownEvent, Pixels,
    Render, Subscription, Window,
};

use super::ops;
use super::tree::{self, Row, StatusBadge, TreeIndex};
use super::walk;
use crate::app::{
    button_frame, context_menu_entry, context_menu_separator, context_menu_surface, empty_state,
    file_badge, file_glyph, icon, icon_button_frame, nerd_font_family, picker_search_frame, press,
    refresh_glyph, EmptyFill, TipExt, BUTTON_GROUP,
};
use crate::composer::ComposerInput;
use crate::git;
use crate::platform;
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{
    context_menu, input, ButtonSize, DynamicSpacing, IconSize, Radius, StyledExt, TextSize,
};
use crate::theme::{self, Theme, ThemeMode};

/// Panel width defaults / drag clamps.
pub const PANEL_DEFAULT_W: f32 = 248.;
pub const PANEL_MIN_W: f32 = 172.;
pub const PANEL_MAX_W: f32 = 460.;

/// Fixed tree row height — 28px, the design system's navigation-row height.
const ROW_H: f32 = 28.;
const HEADER_H: f32 = 40.;
const FOOTER_H: f32 = 26.;

/// What a row click does when the app is told a file was picked. The string
/// is the workspace-relative path, used to label the file in the viewer.
pub type OpenHandler = Rc<dyn Fn(PathBuf, String, &mut App)>;

/// Drag marker for the panel's resize handle (gpui typed drag state).
pub struct ExplorerResize;

/// An invisible drag ghost — resizing leaves no floating preview.
struct ExplorerDragGhost;

impl Render for ExplorerDragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Which row the context menu targets, and where it was opened (window
/// coordinates) so the menu can be placed at the pointer.
struct PanelMenu {
    path: String,
    is_dir: bool,
    expanded: bool,
    position: gpui::Point<Pixels>,
}

/// A file operation the panel asks the app to perform. Paths and names are
/// workspace-relative (the app resolves them against the active workspace and
/// runs the I/O on the background executor), so the panel itself never touches
/// the filesystem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileOpRequest {
    NewFile { dir: String, name: String },
    NewFolder { dir: String, name: String },
    Rename { path: String, name: String },
    Delete { path: String, is_dir: bool },
}

/// The app's handler for a [`FileOpRequest`].
pub type FileOpHandler = Rc<dyn Fn(FileOpRequest, &mut App)>;

/// Which inline name prompt is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    NewFile,
    NewFolder,
    Rename { is_dir: bool },
}

/// A single inline name prompt — new file, new folder, or rename.
struct EntryEditor {
    kind: EntryKind,
    /// Parent directory (workspace-relative, `""` = root) a new entry lands
    /// in, or the renamed entry's directory.
    dir: String,
    /// The row path being renamed; `Some` renders the editor inside that row.
    target: Option<String>,
    input: Entity<ComposerInput>,
    /// A rejected name, shown in the notice banner until the user edits.
    error: Option<String>,
    _sub: Subscription,
}

/// A pending delete confirmation.
struct DeleteConfirm {
    path: String,
    name: String,
    is_dir: bool,
}

pub struct ProjectPanel {
    open: bool,
    width: Pixels,
    /// On Windows, reserve the top-right caption-controls inset in the header
    /// when this dock is the window's rightmost column.
    reserve_controls: bool,
    /// The workspace root the panel is showing.
    workspace: Option<PathBuf>,
    index: TreeIndex,
    /// Visible rows flattened from `index` + `expanded` + filter.
    rows: Vec<Row>,
    expanded: HashSet<String>,
    show_hidden: bool,
    /// A snapshot needs to run (workspace changed, watcher fired, hidden toggled).
    stale: bool,
    loading: bool,
    /// Guards a late snapshot from a previous request.
    generation: u64,
    error: Option<String>,
    list: ListState,
    /// Keyboard cursor: index into `rows`.
    cursor: Option<usize>,
    filter: Entity<ComposerInput>,
    _filter_sub: Subscription,
    focus: FocusHandle,
    /// Workspace-relative path of the file the app is showing.
    active: Option<String>,
    /// Right-click context menu, if open.
    menu: Option<PanelMenu>,
    /// When the menu was dismissed by an outside click; guards re-open.
    menu_dismissed_at: Option<Instant>,
    /// Inline name prompt (new file/folder or rename), if open.
    entry: Option<EntryEditor>,
    /// A pending delete confirmation, if any.
    pending_delete: Option<DeleteConfirm>,
    /// A background file-op failure, shown above the tree.
    notice: Option<String>,
    /// The app's file-open callback.
    on_open: OpenHandler,
    /// The app's file-operation callback.
    on_file_op: FileOpHandler,
    /// The app's close callback (the header's X).
    on_close: Rc<dyn Fn(&mut App)>,
}

impl ProjectPanel {
    pub fn new(
        on_open: OpenHandler,
        on_file_op: FileOpHandler,
        on_close: Rc<dyn Fn(&mut App)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("explorer.filter_files")
                .with_key_context("Composer Picker")
        });
        let filter_sub = cx.observe(&filter, |this, _, cx| {
            this.refresh_rows(cx);
            cx.notify();
        });
        Self {
            open: false,
            width: px(crate::layout::explorer_width()
                .unwrap_or(PANEL_DEFAULT_W)
                .clamp(PANEL_MIN_W, PANEL_MAX_W)),
            reserve_controls: false,
            workspace: None,
            index: TreeIndex::default(),
            rows: Vec::new(),
            expanded: HashSet::new(),
            show_hidden: false,
            stale: false,
            loading: false,
            generation: 0,
            error: None,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            cursor: None,
            filter,
            _filter_sub: filter_sub,
            focus: cx.focus_handle().tab_stop(true),
            active: None,
            menu: None,
            menu_dismissed_at: None,
            entry: None,
            pending_delete: None,
            notice: None,
            on_open,
            on_file_op,
            on_close,
        }
    }

    // ── state entry points (called from the app shell) ─────────────────

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn width(&self) -> Pixels {
        self.width
    }

    pub fn set_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        let width = width.clamp(px(PANEL_MIN_W), px(PANEL_MAX_W));
        if width != self.width {
            self.width = width;
            crate::layout::set_explorer_width(f32::from(width));
            cx.notify();
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        if !self.open {
            self.menu = None;
        } else {
            self.ensure_loaded(cx);
        }
        cx.notify();
    }

    /// Close the dock if it is open. Used while the Review pane is open — the
    /// two right docks are mutually exclusive.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.menu = None;
            cx.notify();
        }
    }

    /// The app calls this every render; a changed workspace rebuilds the tree.
    pub fn set_workspace(&mut self, workspace: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        self.expanded.clear();
        self.index = TreeIndex::default();
        self.rows.clear();
        self.cursor = None;
        self.active = None;
        self.error = None;
        self.entry = None;
        self.pending_delete = None;
        self.notice = None;
        self.list.reset(0);
        self.stale = true;
        self.ensure_loaded(cx);
    }

    /// The workspace watcher fired (or a run settled): rebuild off-thread.
    pub fn mark_stale(&mut self, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            self.stale = true;
            self.ensure_loaded(cx);
        }
    }

    /// Highlight the file the viewer is showing, and reveal it.
    pub fn set_active(&mut self, active: Option<String>, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if let Some(path) = self.active.clone() {
            self.reveal_path(&path);
            self.refresh_rows(cx);
            self.reveal_active(cx);
        }
        cx.notify();
    }

    // ── loading ────────────────────────────────────────────────────────

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.stale || self.loading {
            return;
        }
        let Some(root) = self.workspace.clone() else {
            self.stale = false;
            return;
        };
        self.stale = false;
        self.loading = true;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let show_hidden = self.show_hidden;

        cx.spawn(async move |this, cx| {
            let (snapshot, badges) = cx
                .background_executor()
                .spawn(async move {
                    let snapshot = walk::snapshot(&root, show_hidden);
                    // Git status is a subprocess; it must not run on the UI thread.
                    let badges = git::status_rows(&root)
                        .map(|rows| {
                            rows.into_iter()
                                .map(|row| (row.path.clone(), StatusBadge::from_status(&row)))
                                .collect()
                        })
                        .unwrap_or_default();
                    (snapshot, badges)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.index = TreeIndex::build(snapshot, badges);
                this.loading = false;
                this.error = None;
                this.refresh_rows(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // ── rows ───────────────────────────────────────────────────────────

    fn refresh_rows(&mut self, cx: &Context<Self>) {
        let filter = self.filter.read(cx).text();
        let rows = tree::visible_rows(&self.index, &self.expanded, &filter);
        if rows.len() != self.list.item_count() {
            self.list.reset(rows.len());
        }
        self.rows = rows;
        self.cursor = self.cursor.filter(|index| *index < self.rows.len());
        // A rename prompt whose row vanished (an external change, a filter)
        // has nowhere to render; retire it rather than stranding the keyboard.
        if let Some(target) = self.entry.as_ref().and_then(|entry| entry.target.clone()) {
            if !self.rows.iter().any(|row| row.path == target) {
                self.entry = None;
            }
        }
    }

    fn toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.refresh_rows(cx);
        cx.notify();
    }

    fn expand_all(&mut self, cx: &mut Context<Self>) {
        self.expanded = self.index.all_dir_paths();
        self.refresh_rows(cx);
        cx.notify();
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.refresh_rows(cx);
        cx.notify();
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        if row.is_dir() {
            self.toggle_dir(row.path, cx);
        } else {
            self.open_file(row.path, cx);
        }
    }

    fn open_file(&mut self, relative: String, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.clone() else {
            return;
        };
        self.active = Some(relative.clone());
        (self.on_open)(walk::absolute(&root, &relative), relative, cx);
        cx.notify();
    }

    /// Expand every ancestor of `path` so its row becomes visible.
    fn reveal_path(&mut self, path: &str) {
        for ancestor in tree::ancestors(path) {
            self.expanded.insert(ancestor);
        }
    }

    /// Scroll the active/cursor row into view.
    fn reveal_active(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.active.as_deref() else {
            return;
        };
        if let Some(index) = self.rows.iter().position(|row| row.path == active) {
            self.list.scroll_to_reveal_item(index);
            cx.notify();
        }
    }

    // ── keyboard ───────────────────────────────────────────────────────

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        // The inline prompt owns the keyboard while it is open (Enter/Escape
        // are bound to the `ExplorerEntry` context); don't also drive the tree.
        if self.entry.is_some() {
            return;
        }
        let key = event.keystroke.key.as_str();
        let platform = event.keystroke.modifiers.platform;
        // Escape first: close the row menu, or clear a non-empty filter, and
        // keep the key from falling through to the global abort action.
        if key == "escape" {
            if self.menu.is_some() {
                self.dismiss_menu(cx);
                cx.stop_propagation();
            } else if !self.filter.read(cx).text().is_empty() {
                self.filter.update(cx, |input, cx| input.clear(cx));
                self.refresh_rows(cx);
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        match key {
            "down" => {
                let next = self.cursor.map_or(0, |ix| (ix + 1).min(count - 1));
                self.move_cursor(next, cx);
            }
            "up" => {
                let next = self.cursor.map_or(0, |ix| ix.saturating_sub(1));
                self.move_cursor(next, cx);
            }
            "home" => self.move_cursor(0, cx),
            "end" => self.move_cursor(count - 1, cx),
            "right" if platform => self.expand_all(cx),
            "left" if platform => self.collapse_all(cx),
            "right" => {
                if let Some(index) = self.cursor {
                    if self.rows.get(index).is_some_and(Row::is_dir) {
                        let expanded = self.rows[index].expanded();
                        if !expanded {
                            let path = self.rows[index].path.clone();
                            self.toggle_dir(path, cx);
                        }
                    }
                }
            }
            "left" => {
                if let Some(index) = self.cursor {
                    if self.rows.get(index).is_some_and(Row::is_dir) && self.rows[index].expanded()
                    {
                        let path = self.rows[index].path.clone();
                        self.toggle_dir(path, cx);
                    }
                }
            }
            "enter" | "space" => {
                if let Some(index) = self.cursor {
                    self.activate(index, cx);
                }
            }
            _ => {}
        }
    }

    fn move_cursor(&mut self, index: usize, cx: &mut Context<Self>) {
        self.cursor = Some(index);
        self.list.scroll_to_reveal_item(index);
        cx.notify();
    }

    // ── context menu ───────────────────────────────────────────────────

    fn open_menu(&mut self, row: Row, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        // A right-click anywhere takes focus, so retire any open prompt.
        self.entry = None;
        let is_dir = row.is_dir();
        let expanded = row.expanded();
        self.menu = Some(PanelMenu {
            path: row.path,
            is_dir,
            expanded,
            position,
        });
        cx.notify();
    }

    fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.menu_dismissed_at = Some(Instant::now());
            cx.notify();
        }
    }

    fn menu_target_path(&self) -> Option<PathBuf> {
        let menu = self.menu.as_ref()?;
        let root = self.workspace.clone()?;
        Some(walk::absolute(&root, &menu.path))
    }

    // ── file operations ────────────────────────────────────────────────

    /// The directory a new entry should land in: the selected directory, the
    /// parent of the selected file, or the workspace root.
    fn selected_dir(&self) -> String {
        let Some(row) = self.cursor.and_then(|index| self.rows.get(index)) else {
            return String::new();
        };
        if row.is_dir() {
            row.path.clone()
        } else {
            parent_of(&row.path)
        }
    }

    fn start_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.selected_dir();
        self.start_new_file_in(dir, window, cx);
    }

    fn start_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.selected_dir();
        self.start_new_folder_in(dir, window, cx);
    }

    fn start_new_file_in(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_entry(
            EntryKind::NewFile,
            dir,
            None,
            "",
            "explorer.name_file",
            window,
            cx,
        );
    }

    fn start_new_folder_in(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_entry(
            EntryKind::NewFolder,
            dir,
            None,
            "",
            "explorer.name_folder",
            window,
            cx,
        );
    }

    fn start_rename(
        &mut self,
        path: String,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = path.rsplit('/').next().unwrap_or(path.as_str()).to_string();
        let dir = parent_of(&path);
        self.begin_entry(
            EntryKind::Rename { is_dir },
            dir,
            Some(path.clone()),
            &name,
            "explorer.name_rename",
            window,
            cx,
        );
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            self.list.scroll_to_reveal_item(index);
        }
    }

    /// Open an inline prompt and focus it. Replaces any prompt already open.
    #[allow(clippy::too_many_arguments)]
    fn begin_entry(
        &mut self,
        kind: EntryKind,
        dir: String,
        target: Option<String>,
        initial: &str,
        placeholder_key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("explorer-entry-input")
                .with_placeholder_key(placeholder_key)
                .with_key_context("Composer ExplorerEntry")
                .with_max_lines(1)
                .with_wrap(false)
                .with_text(initial)
        });
        let sub = cx.observe(&input, |this, _, cx| {
            // Clear a rejected-name error as soon as the user edits.
            if let Some(entry) = this.entry.as_mut() {
                if entry.error.take().is_some() {
                    cx.notify();
                }
            }
        });
        self.menu = None;
        self.pending_delete = None;
        self.notice = None;
        self.entry = Some(EntryEditor {
            kind,
            dir,
            target,
            input: input.clone(),
            error: None,
            _sub: sub,
        });
        input.read(cx).focus(window);
        cx.notify();
    }

    fn commit_entry(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.entry.as_ref() else {
            return;
        };
        let raw = entry.input.read(cx).text();
        let name = match ops::validate_name(&raw) {
            Ok(name) => name,
            Err(error) => {
                let message = error.message();
                if let Some(entry) = self.entry.as_mut() {
                    entry.error = Some(message);
                }
                cx.notify();
                return;
            }
        };
        let request = match entry.kind {
            EntryKind::NewFile => FileOpRequest::NewFile {
                dir: entry.dir.clone(),
                name,
            },
            EntryKind::NewFolder => FileOpRequest::NewFolder {
                dir: entry.dir.clone(),
                name,
            },
            EntryKind::Rename { .. } => FileOpRequest::Rename {
                path: entry.target.clone().unwrap_or_default(),
                name,
            },
        };
        let handler = self.on_file_op.clone();
        self.entry = None;
        self.notice = None;
        cx.notify();
        handler(request, cx);
    }

    fn cancel_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry.take().is_some() {
            window.focus(&self.focus);
            cx.notify();
        }
    }

    fn on_entry_confirm(
        &mut self,
        _: &crate::ExplorerEntryConfirm,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_entry(cx);
    }

    fn on_entry_cancel(
        &mut self,
        _: &crate::ExplorerEntryCancel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_entry(window, cx);
    }

    fn request_delete(&mut self, path: String, is_dir: bool, cx: &mut Context<Self>) {
        let name = path.rsplit('/').next().unwrap_or(path.as_str()).to_string();
        self.menu = None;
        self.pending_delete = Some(DeleteConfirm { path, name, is_dir });
        cx.notify();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.pending_delete.take() else {
            return;
        };
        let handler = self.on_file_op.clone();
        self.notice = None;
        cx.notify();
        handler(
            FileOpRequest::Delete {
                path: target.path,
                is_dir: target.is_dir,
            },
            cx,
        );
    }

    fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        if self.pending_delete.take().is_some() {
            cx.notify();
        }
    }

    /// Show (or clear) a background file-op failure above the tree.
    pub fn set_notice(&mut self, notice: Option<String>, cx: &mut Context<Self>) {
        if self.notice != notice {
            self.notice = notice;
            cx.notify();
        }
    }
}

// ── rendering ──────────────────────────────────────────────────────────

/// The parent directory of a `/`-separated workspace path (`""` at the root).
fn parent_of(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

fn badge_color(badge: StatusBadge, theme: &Theme) -> Hsla {
    match badge {
        StatusBadge::Added | StatusBadge::Untracked => theme.success,
        StatusBadge::Modified => theme.warning,
        StatusBadge::Deleted => theme.error,
        StatusBadge::Renamed => theme.accent_text,
        StatusBadge::Conflicted => theme.error,
    }
}

impl ProjectPanel {
    fn header(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let title = self
            .workspace
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| tr!("explorer.files"));
        div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .pl(DynamicSpacing::Base12.px(&theme))
            .pr(px(if self.reserve_controls {
                crate::platform::WINDOW_CONTROLS_W
            } else {
                6.
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(title),
            )
            .child(ghost_icon(
                &theme,
                "explorer-new-file",
                "icons/file-plus.svg",
                theme.text_muted,
                tr!("explorer.new_file"),
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.start_new_file(window, cx);
                }),
            ))
            .child(ghost_icon(
                &theme,
                "explorer-new-folder",
                "icons/folder-plus.svg",
                theme.text_muted,
                tr!("explorer.new_folder"),
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.start_new_folder(window, cx);
                }),
            ))
            .child(
                press(
                    icon_button_frame(
                        div().id("explorer-refresh").group(BUTTON_GROUP),
                        &theme,
                        ButtonSize::Default,
                    )
                    .tip(tr!("common.refresh"))
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.surface_hover)),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.mark_stale(cx);
                }))
                .child(refresh_glyph(
                    "explorer-refresh-spin",
                    ButtonSize::Default.icon_size().px(&theme),
                    self.loading,
                    theme.text_muted,
                    theme,
                )),
            )
            .child(ghost_icon(
                &theme,
                "explorer-collapse",
                "icons/chevron-up.svg",
                theme.text_muted,
                tr!("explorer.collapse"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.collapse_all(cx)),
            ))
            .child(ghost_icon(
                &theme,
                "explorer-hidden",
                if self.show_hidden {
                    "icons/eye.svg"
                } else {
                    "icons/eye-off.svg"
                },
                if self.show_hidden {
                    theme.text_primary
                } else {
                    theme.text_muted
                },
                if self.show_hidden {
                    tr!("explorer.hide_hidden")
                } else {
                    tr!("explorer.show_hidden")
                },
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.show_hidden = !this.show_hidden;
                    this.stale = true;
                    this.ensure_loaded(cx);
                    cx.notify();
                }),
            ))
            .child(ghost_icon(
                &theme,
                "explorer-close",
                "icons/x.svg",
                theme.text_muted,
                tr!("explorer.hide"),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    // Close here — this listener already holds the panel's
                    // lease, so the app callback must not re-enter `update`
                    // on this entity (that would double-lease and abort).
                    let on_close = this.on_close.clone();
                    this.close(cx);
                    on_close(cx);
                }),
            ))
            .into_any_element()
    }

    fn filter_row(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let has_text = !self.filter.read(cx).text().is_empty();
        picker_search_frame(div(), &theme)
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.filter.clone()))
            .when(has_text, |row| {
                row.child(ghost_icon(
                    &theme,
                    "explorer-filter-clear",
                    "icons/x.svg",
                    theme.text_muted,
                    tr!("view.clear_filters"),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.filter.update(cx, |input, cx| input.clear(cx));
                        this.refresh_rows(cx);
                        cx.notify();
                    }),
                ))
            })
            .into_any_element()
    }

    fn tree(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity().downgrade();
        let list = gpui::list(self.list.clone(), move |index, _window, cx| {
            entity
                .upgrade()
                .map(|entity| entity.update(cx, |this, cx| this.render_row(index, cx)))
                .unwrap_or_else(|| div().into_any_element())
        })
        .size_full()
        .py(DynamicSpacing::Base04.px(&theme));
        let no_files_detail = tr!("explorer.no_files_detail");
        let body: AnyElement = if self.loading && self.rows.is_empty() {
            loading_state(&theme)
        } else if let Some(error) = &self.error {
            empty_state(
                theme,
                "icons/stop.svg",
                &tr!("explorer.load_error"),
                Some(error.as_str()),
                EmptyFill::Full,
            )
        } else if self.rows.is_empty() {
            empty_state(
                theme,
                "icons/folder.svg",
                &tr!("explorer.no_files"),
                Some(no_files_detail.as_str()),
                EmptyFill::Full,
            )
        } else {
            list.into_any_element()
        };
        div()
            .id("explorer-tree")
            .track_focus(&self.focus)
            .key_context("ProjectPanel")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key_down(event, cx)),
            )
            .on_action(cx.listener(Self::on_entry_confirm))
            .on_action(cx.listener(Self::on_entry_cancel))
            .children(self.entry_row(theme))
            .children(self.notice_banner(theme))
            .child(div().flex_1().min_h_0().relative().child(body))
            .into_any_element()
    }

    /// The synthetic "new file / new folder" row, pinned above the tree while
    /// its prompt is open. A rename prompt renders inside its own row instead.
    fn entry_row(&self, theme: Theme) -> Option<AnyElement> {
        let entry = self.entry.as_ref()?;
        if entry.target.is_some() {
            return None;
        }
        let (icon_path, indent) = match entry.kind {
            EntryKind::NewFolder => ("icons/folder-plus.svg", 7.0),
            _ => ("icons/file-plus.svg", 23.0),
        };
        let mut row = div()
            .h(px(ROW_H))
            .flex_none()
            .mx(DynamicSpacing::Base04.px(&theme))
            .pl(px(indent))
            .pr(DynamicSpacing::Base08.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(icon(
                icon_path,
                IconSize::Small.px(&theme),
                theme.text_muted,
            ));
        if !entry.dir.is_empty() {
            row = row.child(
                div()
                    .flex_none()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(format!("{}/", entry.dir)),
            );
        }
        Some(
            row.child(div().flex().flex_1().min_w_0().child(entry.input.clone()))
                .into_any_element(),
        )
    }

    /// A one-line error strip above the tree: a rejected name or a failed
    /// background operation.
    fn notice_banner(&self, theme: Theme) -> Option<AnyElement> {
        let message = self
            .entry
            .as_ref()
            .and_then(|entry| entry.error.clone())
            .or_else(|| self.notice.clone())?;
        Some(
            div()
                .flex_none()
                .px(px(10.))
                .py(px(5.))
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.error.opacity(0.12))
                .text_size(TextSize::Small.px(&theme))
                .line_height(theme.ui_px(15.))
                .text_color(theme.error)
                .child(message)
                .into_any_element(),
        )
    }

    fn footer(&self, theme: Theme, _cx: &mut Context<Self>) -> AnyElement {
        let label = if self.index.truncated {
            tr!("explorer.files_truncated", count = self.index.file_count)
        } else {
            tr!("explorer.files_count", count = self.index.file_count)
        };
        div()
            .h(px(FOOTER_H))
            .flex_none()
            .px(DynamicSpacing::Base12.px(&theme))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .text_size(TextSize::Small.px(&theme))
            .text_color(if self.index.truncated {
                theme.warning
            } else {
                theme.text_muted
            })
            .child(label)
            .into_any_element()
    }

    fn render_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(index).cloned() else {
            return div().h(px(ROW_H)).into_any_element();
        };
        let theme = *theme::get(cx);
        let cursor = self.cursor == Some(index);
        let active = self.active.as_deref() == Some(row.path.as_str());
        let nerd = nerd_font_family(cx);
        let dark = theme.mode == ThemeMode::Dark;
        // Dirs lead with a chevron + folder; files with a devicon. The file
        // indent offsets that difference so names align in one column.
        let indent = (if row.is_dir() { 7.0 } else { 23.0 }) + row.depth as f32 * 14.0;
        let icon_color = if active {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let name_color = if active {
            theme.text_primary
        } else {
            theme.text_secondary
        };

        let mut content = div()
            .id(gpui::ElementId::Name(
                format!("explorer-row-{}", row.path).into(),
            ))
            .w_full()
            .h(px(ROW_H))
            .pl(px(indent))
            .pr(DynamicSpacing::Base08.px(&theme))
            .rounded(Radius::Medium.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme));
        // A row with an open rename prompt owns the keyboard; it must not also
        // activate the file or open its context menu on click.
        let renaming = self
            .entry
            .as_ref()
            .is_some_and(|entry| entry.target.as_deref() == Some(row.path.as_str()));
        let entry_input = renaming.then(|| {
            self.entry
                .as_ref()
                .expect("renaming implies an entry")
                .input
                .clone()
        });
        if renaming {
            content = content.bg(theme.surface_active);
        } else {
            content = content
                .cursor_pointer()
                // Selected (open in the viewer) wins; the keyboard cursor is a
                // quieter wash; pointer hover is the lightest step.
                .when(active, |el| el.bg(theme.selection))
                .when(!active && cursor, |el| el.bg(theme.overlay))
                .when(!active && !cursor, |el| {
                    el.hover(|el| el.bg(theme.surface_hover))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    // Clicking away from an open prompt cancels it rather than
                    // activating the row under the pointer.
                    if this.entry.is_some() {
                        this.cancel_entry(window, cx);
                        return;
                    }
                    window.focus(&this.focus);
                    this.cursor = Some(index);
                    this.activate(index, cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener({
                        let row = row.clone();
                        move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus);
                            this.cursor = Some(index);
                            this.open_menu(row.clone(), event.position, cx);
                        }
                    }),
                );
        }

        if row.is_dir() {
            content = content
                .child(icon(
                    if row.expanded() {
                        "icons/chevron-down.svg"
                    } else {
                        "icons/chevron-right.svg"
                    },
                    IconSize::Indicator.px(&theme),
                    icon_color,
                ))
                .child(icon(
                    "icons/folder.svg",
                    IconSize::Small.px(&theme),
                    icon_color,
                ));
            if let Some(input) = entry_input {
                content = content.child(div().flex().flex_1().min_w_0().child(input));
            } else {
                content = content.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(name_color)
                        .child(row.name.clone()),
                );
                if row.dirty {
                    content = content.child(
                        div()
                            .size(px(5.))
                            .flex_none()
                            .rounded_full()
                            .bg(theme.warning),
                    );
                }
            }
        } else {
            let fallback = file_badge(&row.path, theme);
            let glyph = file_glyph(
                &row.path,
                dark,
                nerd.as_ref(),
                IconSize::Small.px(&theme),
                fallback,
            );
            content = content.child(glyph);
            if let Some(input) = entry_input {
                content = content.child(div().flex().flex_1().min_w_0().child(input));
            } else {
                content = content.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(name_color)
                        .child(row.name.clone()),
                );
                if let Some(badge) = row.badge {
                    let color = badge_color(badge, &theme);
                    content = content.child(
                        div()
                            .flex_none()
                            .text_size(TextSize::Small.px(&theme))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(badge.letter().to_string()),
                    );
                }
            }
        }

        // The list measures each row with a definite width, but a bare flex
        // row sizes to its content — so `flex_1` children (the name, and the
        // rename `ComposerInput`, which has no intrinsic width) would collapse.
        // A full-width wrapper gives them the row's width to fill; the 4px side
        // inset rides on this wrapper instead of a margin on the row.
        div()
            .w_full()
            .px(DynamicSpacing::Base04.px(&theme))
            .child(content)
            .into_any_element()
    }

    fn context_menu(&self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        if self
            .menu_dismissed_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(150))
        {
            return None;
        }
        let target = self.menu_target_path()?;
        let relative = menu.path.clone();
        let is_dir = menu.is_dir;
        let expanded = menu.expanded;
        let mut popup = context_menu_surface(div().id("explorer-context-menu"), &theme)
            .w(px(220.))
            .flex()
            .flex_col()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.dismiss_menu(cx);
            }));

        if is_dir {
            // A directory's first action is its expand/collapse toggle.
            let (label, icon_path) = if expanded {
                (tr!("explorer.collapse"), "icons/chevron-down.svg")
            } else {
                (tr!("explorer.expand"), "icons/chevron-right.svg")
            };
            popup = popup
                .child(menu_row(
                    theme,
                    icon_path,
                    label,
                    None,
                    cx.listener({
                        let relative = relative.clone();
                        move |this, _: &ClickEvent, _, cx| {
                            this.dismiss_menu(cx);
                            this.toggle_dir(relative.clone(), cx);
                        }
                    }),
                ))
                .child(separator(theme))
                .child(menu_row(
                    theme,
                    "icons/file-plus.svg",
                    tr!("explorer.new_file"),
                    None,
                    cx.listener({
                        let dir = relative.clone();
                        move |this, _: &ClickEvent, window, cx| {
                            this.dismiss_menu(cx);
                            this.start_new_file_in(dir.clone(), window, cx);
                        }
                    }),
                ))
                .child(menu_row(
                    theme,
                    "icons/folder-plus.svg",
                    tr!("explorer.new_folder"),
                    None,
                    cx.listener({
                        let dir = relative.clone();
                        move |this, _: &ClickEvent, window, cx| {
                            this.dismiss_menu(cx);
                            this.start_new_folder_in(dir.clone(), window, cx);
                        }
                    }),
                ))
                .child(separator(theme))
                .child(menu_row(
                    theme,
                    "icons/pencil.svg",
                    tr!("explorer.rename"),
                    None,
                    cx.listener({
                        let relative = relative.clone();
                        move |this, _: &ClickEvent, window, cx| {
                            this.dismiss_menu(cx);
                            this.start_rename(relative.clone(), true, window, cx);
                        }
                    }),
                ))
                .child(menu_row(
                    theme,
                    "icons/trash.svg",
                    tr!("explorer.delete"),
                    None,
                    cx.listener({
                        let relative = relative.clone();
                        move |this, _: &ClickEvent, _, cx| {
                            this.request_delete(relative.clone(), true, cx);
                        }
                    }),
                ))
                .child(separator(theme));
        } else {
            popup = popup
                .child(menu_row(
                    theme,
                    "icons/file.svg",
                    tr!("explorer.open"),
                    None,
                    cx.listener({
                        let relative = relative.clone();
                        move |this, _: &ClickEvent, _, cx| {
                            this.dismiss_menu(cx);
                            this.open_file(relative.clone(), cx);
                        }
                    }),
                ))
                .child(separator(theme))
                .child(menu_row(
                    theme,
                    "icons/pencil.svg",
                    tr!("explorer.rename"),
                    None,
                    cx.listener({
                        let relative = relative.clone();
                        move |this, _: &ClickEvent, window, cx| {
                            this.dismiss_menu(cx);
                            this.start_rename(relative.clone(), false, window, cx);
                        }
                    }),
                ))
                .child(menu_row(
                    theme,
                    "icons/trash.svg",
                    tr!("explorer.delete"),
                    None,
                    cx.listener({
                        let relative = relative.clone();
                        move |this, _: &ClickEvent, _, cx| {
                            this.request_delete(relative.clone(), false, cx);
                        }
                    }),
                ))
                .child(separator(theme));
        }
        popup = popup
            .child(menu_row(
                theme,
                "icons/arrow-up-right.svg",
                tr!("explorer.open_default"),
                None,
                cx.listener({
                    let target = target.clone();
                    move |this, _: &ClickEvent, _, cx| {
                        platform::open_path_default(&target);
                        this.dismiss_menu(cx);
                    }
                }),
            ))
            .child(menu_row(
                theme,
                "icons/folder.svg",
                tr!("explorer.reveal"),
                None,
                cx.listener({
                    let target = target.clone();
                    move |this, _: &ClickEvent, _, cx| {
                        platform::reveal_in_file_manager(&target);
                        this.dismiss_menu(cx);
                    }
                }),
            ))
            .child(separator(theme))
            .child(menu_row(
                theme,
                "icons/copy.svg",
                tr!("explorer.copy_path"),
                None,
                cx.listener({
                    let text = target.to_string_lossy().to_string();
                    move |this, _: &ClickEvent, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                        this.dismiss_menu(cx);
                    }
                }),
            ))
            .child(menu_row(
                theme,
                "icons/copy.svg",
                tr!("explorer.copy_relative_path"),
                None,
                cx.listener({
                    let text = relative.clone();
                    move |this, _: &ClickEvent, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                        this.dismiss_menu(cx);
                    }
                }),
            ));
        Some(
            // Place the menu at the pointer (window coordinates), flipping it
            // above/left when it would overflow the window — the default
            // `SwitchAnchor` fit mode. Previously it was pinned to the panel's
            // top-left regardless of where the row was clicked.
            gpui::anchored()
                .anchor(gpui::Corner::TopLeft)
                .position(menu.position)
                .child(popup)
                .into_any_element(),
        )
    }

    /// The delete confirmation, painted over the panel. Worded from
    /// [`platform::TRASH_IS_RECOVERABLE`] so it never promises a restore the
    /// platform cannot deliver.
    fn delete_prompt(&self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.pending_delete.as_ref()?;
        let body_key = match (target.is_dir, platform::TRASH_IS_RECOVERABLE) {
            (true, true) => "explorer.delete_body_dir_trash",
            (true, false) => "explorer.delete_body_dir_permanent",
            (false, true) => "explorer.delete_body_file_trash",
            (false, false) => "explorer.delete_body_file_permanent",
        };
        let card = div()
            .w_full()
            .max_w(px(250.))
            .elevation_2(&theme)
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .occlude()
            .on_mouse_down_out(
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.cancel_delete(cx)),
            )
            .child(
                div()
                    .text_size(TextSize::Default.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("explorer.delete_title", name = target.name.clone())),
            )
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .line_height(theme.ui_px(16.))
                    .whitespace_normal()
                    .text_color(theme.text_secondary)
                    .child(crate::i18n::translate(body_key)),
            )
            .child(
                div()
                    .mt(px(2.))
                    .flex()
                    .justify_end()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(prompt_button(
                        theme,
                        "explorer-delete-cancel",
                        tr!("explorer.cancel"),
                        false,
                        cx.listener(|this, _: &ClickEvent, _, cx| this.cancel_delete(cx)),
                    ))
                    .child(prompt_button(
                        theme,
                        "explorer-delete-confirm",
                        tr!("explorer.delete"),
                        true,
                        cx.listener(|this, _: &ClickEvent, _, cx| this.confirm_delete(cx)),
                    )),
            );
        Some(
            div()
                .absolute()
                .inset_0()
                .px(px(10.))
                .bg(theme.surface.opacity(0.72))
                .flex()
                .items_center()
                .justify_center()
                .child(card)
                .into_any_element(),
        )
    }
}

/// A small prompt button — quiet by default, `crit` for a destructive action.
fn prompt_button(
    theme: Theme,
    id: &'static str,
    label: String,
    destructive: bool,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (bg, fg) = if destructive {
        (theme.error, theme.error_fg)
    } else {
        (theme.surface_elevated, theme.text_primary)
    };
    press(
        button_frame(div().id(id).group(BUTTON_GROUP), &theme, ButtonSize::Medium)
            .cursor_pointer()
            .raised(bg, &theme)
            .hover(|el| el.opacity(0.9)),
    )
    .on_click(listener)
    .child(
        div()
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg)
            .child(label),
    )
    .into_any_element()
}

/// A header/toolbar ghost icon control: a Default icon button, hover fill
/// only.
fn ghost_icon(
    theme: &Theme,
    id: &'static str,
    path: &'static str,
    color: Hsla,
    label: String,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    press(
        icon_button_frame(div().id(id).group(BUTTON_GROUP), theme, ButtonSize::Default)
            .tip(label)
            .cursor_pointer()
            .hover(|el| el.bg(theme.surface_hover)),
    )
    .on_click(listener)
    .child(icon(path, ButtonSize::Default.icon_size().px(theme), color))
    .into_any_element()
}

fn separator(theme: Theme) -> AnyElement {
    context_menu_separator(&theme).into_any_element()
}

/// One Explorer context-menu entry, on Zed's context menu metrics.
fn menu_row(
    theme: Theme,
    icon_path: &'static str,
    label: String,
    hint: Option<String>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    context_menu_entry(
        div().id(gpui::ElementId::Name(
            format!("explorer-menu-{label}").into(),
        )),
        &theme,
    )
    .cursor_pointer()
    .hover(|el| el.bg(theme.surface_hover))
    .on_click(listener)
    .child(icon(
        icon_path,
        context_menu::ICON.px(&theme),
        theme.text_muted,
    ))
    .child(
        div()
            .min_w_0()
            .flex_1()
            .truncate()
            .text_color(theme.text_secondary)
            .child(label),
    )
    .children(hint.map(|hint| {
        // Zed's `ml_4` before a keybinding, less the row gap already there.
        div()
            .flex_none()
            .ml(context_menu::keybinding_gap(&theme) - context_menu::icon_gap(&theme))
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(hint)
    }))
    .into_any_element()
}

/// A page-level spinner; static under reduce-motion.
fn loading_state(theme: &Theme) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .gap(DynamicSpacing::Base08.px(theme))
        .child(crate::app::spinner(
            "explorer-loading",
            IconSize::Small.px(theme),
            theme.text_muted,
            *theme,
        ))
        .child(
            div()
                .text_size(TextSize::Small.px(theme))
                .text_color(theme.text_muted)
                .child(tr!("explorer.loading")),
        )
        .into_any_element()
}

impl Render for ProjectPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        self.ensure_loaded(cx);
        // The filter is read here; the observer marks rows dirty on change.
        self.refresh_rows(cx);
        let theme = *theme::get(cx);
        div()
            .id("project-panel")
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
                    .id("explorer-resize-handle")
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
                    .on_drag(ExplorerResize, |_, _, _, cx| cx.new(|_| ExplorerDragGhost)),
            )
            // The dock reads as a rounded card, like the Git/Usage cards in
            // the main column. The popups stay outside this clipped shell so
            // they are not cut off at its corners.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Radius::XLarge.px(&theme))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(self.header(theme, cx))
                    .child(self.filter_row(theme, cx))
                    .child(self.tree(theme, cx))
                    .child(self.footer(theme, cx)),
            )
            .children(self.context_menu(theme, cx))
            .children(self.delete_prompt(theme, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parent_of_splits_workspace_paths() {
        assert_eq!(parent_of("src/main.rs"), "src");
        assert_eq!(parent_of("main.rs"), "");
        assert_eq!(parent_of("a/b/c.rs"), "a/b");
    }

    /// A `ComposerInput` has no intrinsic width, so on a content-sized row its
    /// `flex_1` collapses to zero and the inline prompt shows only the devicon.
    /// Render the real panel and assert both prompts get a usable width.
    #[gpui::test]
    fn inline_prompt_input_fills_the_row(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join("orbit-explorer-panel-prompt");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let cx = cx.add_empty_window();
        cx.update(|_, app| app.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));

        let open: OpenHandler = Rc::new(|_, _, _| {});
        let op: FileOpHandler = Rc::new(|_, _| {});
        let panel = cx.update(|_, app| {
            app.new(|cx| ProjectPanel::new(open, op, Rc::new(|_: &mut App| {}), cx))
        });

        let snapshot = walk::snapshot(&root, false);
        cx.update(|window, app| {
            panel.update(app, |panel, cx| {
                panel.open = true;
                panel.width = px(280.);
                panel.workspace = Some(root.clone());
                panel.index = TreeIndex::build(snapshot, HashMap::new());
                panel.stale = false;
                panel.refresh_rows(cx);
                // Rename renders inside its row; New File renders above the
                // tree. Both wrap the same prompt input.
                panel.start_rename("main.rs".to_string(), false, window, cx);
            });
        });
        let drawn = panel.clone();
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(280.), px(600.)),
            {
                let drawn = drawn.clone();
                move |_, _| drawn.clone()
            },
        );
        assert_prompt_has_width(cx, "rename");

        cx.update(|window, app| {
            panel.update(app, |panel, cx| panel.start_new_file(window, cx));
        });
        let drawn = panel.clone();
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(280.), px(600.)),
            {
                let drawn = drawn.clone();
                move |_, _| drawn.clone()
            },
        );
        assert_prompt_has_width(cx, "new file");
    }

    fn assert_prompt_has_width(cx: &mut gpui::VisualTestContext, label: &str) {
        let bounds = cx
            .debug_bounds("explorer-entry-input")
            .unwrap_or_else(|| panic!("the {label} prompt input was not rendered"));
        assert!(
            f32::from(bounds.size.width) > 40.,
            "the {label} prompt input collapsed to {}px wide",
            f32::from(bounds.size.width)
        );
    }
}
