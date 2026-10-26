use super::helpers::*;
use super::sidebar::*;
use super::*;

use crate::commands::{self, CommandId};
use gpui::StyledText;

use crate::theme::tokens::{
    button, context_menu, picker, popover, AnimationDuration, ButtonSize, DynamicSpacing, IconSize,
    Radius, TextSize,
};
use crate::usage::tooltip::Tooltip;
use crate::widgets as ext_widgets;
use crate::workspace_mark::{self, WorkspaceMark};

/// Height of a page's top bar (DESIGN.md: 44px header rows). The new-task
/// backdrop is offset by it, so the picture starts below the title exactly
/// where it always has — this is the one value the two must agree on.
pub(super) const TOP_BAR_H: f32 = 44.;

/// Leading inset that clears the OS window buttons in the titlebar row: the
/// macOS traffic lights when the titlebar is transparent, or a plain edge
/// inset where the system titlebar holds them (see
/// [`platform::titlebar_options`]). The sidebar's drag strip and the main top
/// bar share it so the window's left controls never ride into the OS buttons.
pub(super) const TRAFFIC_LIGHT_CLEARANCE: f32 = platform::WINDOW_CONTROLS_CLEARANCE;

/// Space between the titlebar's left controls. Set to the same 8px the right
/// cluster spaces its chips with (`gap_2`), so the two ends of the bar share
/// one rhythm — a bare 2px let bordered chips read as one crowded block.
const TITLEBAR_CONTROLS_GAP: f32 = 8.;

/// Breathing room between the OS window buttons and the first titlebar
/// control. The traffic-light clearance alone ended flush against them, which
/// made the app's own controls read as part of the caption; the lead gives
/// the overlay its own left margin.
pub(super) const TITLEBAR_CONTROLS_LEAD: f32 = 12.;

/// Combined width of the titlebar's left controls (toggle + history) as laid
/// out by [`OrbitApp::titlebar_left_controls`]: three [`HEADER_CTRL_H`] boxes,
/// two [`TITLEBAR_CONTROLS_GAP`] gaps, and the trailing 6px gap.
pub(super) const TITLEBAR_CONTROLS_W: f32 = HEADER_CTRL_H * 3. + TITLEBAR_CONTROLS_GAP * 2. + 6.;

/// Space between the controls and the session title while the sidebar is
/// collapsed. The cluster's own trailing 6px pad is inside
/// [`TITLEBAR_CONTROLS_W`], so this is what actually separates the last chip
/// from the title — without it the two sit flush once the sidebar stops
/// providing the separation.
pub(super) const TITLEBAR_TITLE_GAP: f32 = 8.;

/// Where the main top bar's content starts when the sidebar is collapsed: past
/// the traffic lights, the controls' lead margin, the (overlaid) window
/// controls, and the gap that keeps the title off them.
pub(super) const TITLEBAR_LEADING: f32 =
    TRAFFIC_LIGHT_CLEARANCE + TITLEBAR_CONTROLS_LEAD + TITLEBAR_CONTROLS_W + TITLEBAR_TITLE_GAP;

/// Space kept between the last titlebar control and the sidebar's right edge
/// while the controls are right-aligned inside the sidebar's strip. The
/// sidebar's 6px resize handle lives against that edge, so the pad keeps the
/// chips off it (and off the sidebar's rounded boundary).
pub(super) const SIDEBAR_EDGE_PAD: f32 = 6.;

/// Where the titlebar's left controls sit. While the sidebar is open they
/// right-align inside its strip — [`SIDEBAR_EDGE_PAD`] off the column's right
/// edge, so they ride both a resize drag and the open/close slide. Collapsed,
/// there is no column to hug and the chips fall back to the fixed lead past
/// the OS window buttons.
///
/// Windows is the exception: it has no OS buttons on the left at all — the app
/// paints the caption itself, at the window's right
/// ([`platform::draws_window_controls`]) — so the cluster left-aligns on the
/// fixed lead whether the sidebar is open or not, the way a Windows app's own
/// titlebar controls sit. Nothing else changes there: the lead still clears
/// the window edge, and the sidebar's drag strip and wordmark live on other
/// rows of that column.
pub(super) fn titlebar_controls_left(sidebar_visible: bool, sidebar_width: f32) -> f32 {
    if sidebar_visible && !cfg!(windows) {
        sidebar_width - TITLEBAR_CONTROLS_W - SIDEBAR_EDGE_PAD
    } else {
        TRAFFIC_LIGHT_CLEARANCE + TITLEBAR_CONTROLS_LEAD
    }
}

/// Wrap a feature page (Files / Git / Usage) in the card the main column shows
/// below the shared top bar: a gap from the sidebar on the left, rounded
/// corners, a hairline, and a clipped body. Keeps the top bar and the session
/// context in place instead of swapping the whole column for an edge-to-edge
/// page.
fn feature_card(theme: Theme, body: AnyElement) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .pl(DynamicSpacing::Base12.px(&theme))
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(Radius::XLarge.px(&theme))
                .border_1()
                .border_color(theme.border)
                .child(body),
        )
        .into_any_element()
}

/// One row in the top bar's overflow menu: a picker entry with a leading
/// icon, a label, the command's platform chord, and a trailing check when its
/// surface is open. Reuses the picker's row metrics so it lines up with the
/// app's other dropdowns.
fn header_menu_row(
    id: &'static str,
    icon_path: &'static str,
    label: String,
    shortcut: Option<String>,
    active: bool,
    theme: Theme,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    press(
        picker_entry(div().id(id).group(BUTTON_GROUP), &theme)
            .h(picker::entry_height(&theme))
            .flex_none()
            .cursor_pointer()
            .text_color(theme.text_secondary)
            .hover(|s| s.bg(theme.surface_hover).text_color(theme.text_primary)),
    )
    .on_mouse_up(MouseButton::Left, listener)
    .child(icon(
        icon_path,
        context_menu::ICON.px(&theme),
        theme.text_muted,
    ))
    .child(div().flex_1().min_w_0().truncate().child(label))
    .children(shortcut.map(|chord| {
        div()
            .flex_none()
            .text_size(TextSize::XSmall.px(&theme))
            .text_color(theme.text_muted)
            .child(chord)
    }))
    .when(active, |row| {
        row.child(icon(
            "icons/check.svg",
            context_menu::ICON.px(&theme),
            theme.accent_text,
        ))
    })
    .into_any_element()
}

/// Duration of the sidebar collapse/expand slide.
const SIDEBAR_SLIDE_MS: u64 = 180;

/// The Access and Workflow mode picker popups share one width.
const MODE_POPUP_W: f32 = 300.;

/// Page content kept clear above the terminal panel — the transcript *and*
/// the composer, which sits between them. Dragging the panel's top edge to the
/// top of the window must not collapse the conversation behind the input.
const TERMINAL_MAIN_RESERVE: f32 = 320.;

impl Render for OrbitApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Cleared here and set again only where the new-task backdrop is drawn,
        // so a frame that covers it (Settings, a feature page) stops the
        // heartbeat repainting for a field nobody can see.
        self.backdrop_live = false;
        // A clicked banner asks for the window; bring it forward on the frame
        // that follows the click (macOS activates the app, but a minimized
        // window would otherwise stay behind).
        if self.activate_window_pending {
            self.activate_window_pending = false;
            window.activate_window();
        }
        // Model / thinking choices are fixed for the duration of a turn. A
        // picker left open when a run begins is dropped so its list cannot
        // still change the model mid-turn; the chips render disabled.
        if self.is_running() {
            self.close_model_selector(window, cx);
        }
        let theme = *theme::get(cx);
        // Sessions the sidebar lists, plus a placeholder for the open one
        // when its file is not on disk yet. The archived filter is applied
        // here so row indices match the list the render path receives; the
        // unfiltered list still drives the per-workspace status glyphs.
        let all_sessions = self.sidebar_sessions();
        let archived_set: HashSet<PathBuf> = crate::archive::all().paths();
        let sidebar_sessions = filter_archived(
            all_sessions.clone(),
            self.sidebar_archived_filter,
            &archived_set,
        );
        // A pinned session leads its project group; the sidebar sorts and
        // marks pinned rows from this snapshot (Orbit-owned state).
        let pinned: Rc<HashSet<PathBuf>> = Rc::new(crate::pins::all().paths());
        // Archived sessions, from the Orbit-owned set — rows in "show
        // archived" mode wear a small archive glyph.
        let archived_paths: Rc<HashSet<PathBuf>> = Rc::new(archived_set);
        // Flat "one list" mode names each row's workspace instead of grouping.
        let show_workspace = self.sidebar_group_by == SidebarGroupBy::OneList;
        // Parked (background) sessions mid-run — they keep their row under a
        // collapsed workspace header, like the open session, so a live task
        // is never hidden by a collapse. Also drives the running loader.
        let running_paths: Rc<HashSet<PathBuf>> = Rc::new(
            self.lives
                .iter()
                .filter(|(_, parked)| parked.busy)
                .map(|(path, _)| path.clone())
                .collect(),
        );
        // Per-workspace glyph: the user's mark icon when set, otherwise a
        // folder status (current / running / archived-only / idle).
        let running_workspaces: HashSet<PathBuf> = sidebar_sessions
            .iter()
            .filter(|session| running_paths.contains(&session.path))
            .map(|session| session.cwd.clone())
            .collect();
        let archived_only_workspaces: HashSet<PathBuf> = {
            let mut totals: HashMap<PathBuf, (usize, usize)> = HashMap::new();
            for session in &all_sessions {
                let entry = totals.entry(session.cwd.clone()).or_insert((0, 0));
                entry.0 += 1;
                if archived_paths.contains(&session.path) {
                    entry.1 += 1;
                }
            }
            totals
                .into_iter()
                .filter(|(_, (total, archived))| *total > 0 && total == archived)
                .map(|(workspace, _)| workspace)
                .collect()
        };
        let workspace_icons: Rc<HashMap<PathBuf, gpui::SharedString>> = Rc::new(
            self.workspaces
                .iter()
                .map(|workspace| {
                    let mark = self
                        .workspace_marks
                        .get(workspace)
                        .cloned()
                        .unwrap_or_default();
                    let status = if Some(workspace.as_path()) == self.current_workspace.as_deref() {
                        workspace_mark::WorkspaceStatus::Current
                    } else if running_workspaces.contains(workspace) {
                        workspace_mark::WorkspaceStatus::Running
                    } else if archived_only_workspaces.contains(workspace) {
                        workspace_mark::WorkspaceStatus::Archived
                    } else {
                        workspace_mark::WorkspaceStatus::Idle
                    };
                    (
                        workspace.clone(),
                        workspace_mark::resolved_icon_path(&mark, status),
                    )
                })
                .collect(),
        );
        let side_rows = Rc::new(self.sidebar_rows(&sidebar_sessions, &pinned));
        let old = self.sidebar_list.item_count();
        if old != side_rows.len() {
            self.sidebar_list.splice(0..old, side_rows.len());
        }
        let sessions_data = Rc::new(sidebar_sessions);
        let active_path = Rc::new(self.current_session_path.clone());
        let session_menu = Rc::new(self.session_menu.clone());
        let workspace_menu = Rc::new(self.workspace_menu.clone());
        let sidebar_cursor = self.sidebar_cursor;
        let this = cx.entity();
        // The open session's agent activity drives the sidebar's running
        // loader (parked runs come in via `running_paths` above).
        let agent_running = self.busy || self.transcript.is_streaming();
        // Every live process (running or warm-idle) — guards delete.
        let live_paths: Rc<HashSet<PathBuf>> = Rc::new(self.lives.keys().cloned().collect());
        // Per-workspace sidebar marks (icon + tint), read by every header row.
        let marks: Rc<HashMap<PathBuf, WorkspaceMark>> = Rc::new(self.workspace_marks.clone());
        // Pinned header for the expanded workspace group currently at the top
        // of the session list: it stays there while that group's own sessions
        // scroll, and the next group's header pushes it away (see
        // `sticky_sidebar_header`). Its row is rendered twice while pinned (the
        // real one scrolls under the overlay), so the pinned index also tells
        // the list to skip that row's workspace menu — the overlay owns the
        // single open popup.
        let sticky = sticky_sidebar_header(&self.sidebar_list, &side_rows);
        let sticky_ix = sticky.as_ref().map(|sticky| sticky.ix);
        let sticky_header = sticky.map(|sticky| {
            div()
                .absolute()
                .top(sticky.top_offset)
                .left_0()
                .w_full()
                .bg(theme.surface)
                .child(render_side_row(
                    &side_rows,
                    &sessions_data,
                    active_path.as_deref(),
                    sticky.ix,
                    &this,
                    agent_running,
                    &running_paths,
                    &pinned,
                    &archived_paths,
                    show_workspace,
                    &live_paths,
                    &marks,
                    &workspace_icons,
                    session_menu.as_ref().as_ref(),
                    workspace_menu.as_ref().as_ref(),
                    sidebar_cursor == Some(sticky.ix),
                    theme,
                ))
                .into_any_element()
        });

        let workspace_label = self.workspace_label();
        let review_workspace = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok());
        // The rail gates on the main area's width (872px transcript
        // container), which excludes the sessions sidebar when visible.
        let viewport = window.viewport_size();
        // The right side pane is hidden while settings/onboarding own the
        // main area (same rule as the sessions sidebar).
        let pane_open = self.sidepane.read(cx).is_open();
        let pane_visible =
            pane_open && !self.settings_open && !self.usage_open && self.dependencies_ready();
        // Feature pages (Files / Git / Usage) own the main column, so a
        // full-page Review pane yields to the one that just opened rather
        // than covering it.
        let viewer_open = self.file_viewer.read(cx).is_open();
        let feature_open = viewer_open || self.git_open || self.usage_open || self.worktrees_open;
        // A page that opens under a full-page Review pane gets the window
        // back; one that was already open does not block maximizing over it.
        if pane_visible
            && feature_open
            && !self.feature_open_last
            && self.sidepane.read(cx).is_full_width()
        {
            self.sidepane
                .update(cx, |pane, cx| pane.leave_full_width(cx));
        }
        self.feature_open_last = feature_open;
        // Expanding the Review pane hides the session view, but it opens the
        // sessions sidebar: switching sessions stays one click away while the
        // review owns the page. The sidebar snaps open with the pane rather
        // than sliding, so the session view never peeks through mid-slide.
        let pane_full = pane_visible && self.sidepane.read(cx).is_full_width();
        if pane_full && !self.pane_full_last && !self.sidebar_visible {
            self.sidebar_visible = true;
        }
        self.pane_full_last = pane_full;
        let sidebar_shown = self.sidebar_visible && !self.settings_open;

        // Bottom terminal panel. The chat branch is what renders it, so the
        // full-page surfaces (settings / Git / Usage) hide it by construction;
        // `terminal_visible` exists to stop a hidden shell requesting frames.
        // A full-page Review pane collapses the chat column, terminal
        // included, so it counts as hidden too.
        let terminal_visible = self.terminal_panel.read(cx).is_open()
            && !self.settings_open
            && !self.usage_open
            && !self.git_open
            && !self.worktrees_open
            && !pane_full;
        // The embedded Review browser on the Git page's Changes tab lays out
        // against the page width, so its stats and tree follow the column.
        let git_review_width = viewport.width
            - if sidebar_shown {
                self.sidebar_width
            } else {
                px(0.)
            };
        // Leading inset the full-window pages (Git/Usage/Files) give their
        // headers while the sidebar is collapsed. They now sit in a card below
        // the top bar, so they only need the normal page padding — which is
        // already their default; nothing to sync per frame.
        self.git_panel
            .update(cx, |panel, cx| panel.set_review_width(git_review_width, cx));
        // ── project panel (right dock) ── hidden while Settings owns the
        // window, like the sessions sidebar. Sync the workspace each render;
        // a change rebuilds the tree off-thread. The Explorer and the Review
        // pane are mutually exclusive right docks: while Review is open the
        // tree stays closed.
        let explorer_visible =
            self.project_panel.read(cx).is_open() && !self.settings_open && !pane_open;
        let explorer_width = if explorer_visible {
            self.project_panel.read(cx).width()
        } else {
            px(0.)
        };
        // The pane's ceiling is the window minus the sidebar and the Explorer;
        // Full width expands to it and collapses the chat column. The Explorer
        // is closed whenever the pane opens, so this is usually the sidebar's
        // width alone.
        let pane_ceiling = viewport.width
            - if sidebar_shown {
                self.sidebar_width
            } else {
                px(0.)
            }
            - explorer_width;
        self.sidepane
            .update(cx, |pane, cx| pane.set_available_width(pane_ceiling, cx));
        // Read the pane's width after the ceiling update: a full-width pane
        // has just grown and this frame's layout must see it.
        let pane_width = if pane_visible {
            self.sidepane.read(cx).width()
        } else {
            px(0.)
        };
        let main_width = viewport.width
            - if sidebar_shown {
                self.sidebar_width
            } else {
                px(0.)
            }
            - explorer_width
            - pane_width;
        // Composer toolbar compaction: below this column width the access
        // pill drops out and the model label clamps (Send stays reachable).
        let composer_compact = (main_width - px(32.)).min(px(CONTENT_MAX_W)) < px(600.);
        // The Usage page lays itself out against the real main-area width, so
        // its tables and grids never overflow the column it is given.
        if self.usage_open {
            let width = f32::from(main_width);
            self.usage
                .update(cx, |page, cx| page.set_main_width(width, cx));
        }

        // ── top-bar right controls ──
        // When Review owns the right edge, or the chat column is tight, the
        // title and the chips collide. Compact the quota label and drop the
        // +/− chip (Review already shows the same stats).
        // The top bar spans the whole main area, so the docks do not squeeze
        // it — compact only when the main area itself is tight.
        let compact_chrome = f32::from(main_width + explorer_width + pane_width) < 720.;
        // While a feature card is open, its title moves up into the shared top
        // bar (see `GitPanel::top_bar_leading` / `UsagePage::top_bar_leading`);
        // the card's own chrome (branch selector, filters) stays with the view
        // it scopes.
        let git_leading = self.git_open.then(|| {
            self.git_panel
                .update(cx, |panel, cx| panel.top_bar_leading(theme, cx))
        });
        let usage_leading = self.usage_open.then(|| {
            self.usage
                .update(cx, |page, cx| page.top_bar_leading(theme, cx))
        });
        let worktrees_leading = self
            .worktrees_open
            .then(|| self.worktrees_top_bar_leading(theme, cx));
        let mut top_controls = div().flex_none().flex().items_center().gap_2();
        // Provider quota — a compact, provider-independent headroom meter.
        // Hidden entirely on a pi without `quota.*` (or when no provider
        // reports anything), so the bar never shows a fabricated value.
        top_controls = top_controls.children(self.render_quota_pill(compact_chrome, cx));
        top_controls = top_controls.children(self.render_open_in_control(cx));
        top_controls = top_controls
            // Review dock — the side-panel toggle and the working tree's
            // change counts folded into one chip
            // (see `render_review_panel_control`).
            .child(self.render_review_panel_control(theme, pane_visible, cx))
            // The remaining surfaces (Session details, Explorer, Terminal, Git)
            // sit behind one "…" menu so the bar keeps only what shows live
            // state: the quota, open-in, and Review. See `render_header_more`.
            .child(self.render_header_more(theme, explorer_visible, terminal_visible, cx));

        // A blocking extension dialog owns the keyboard while it is open. Focus
        // it (or its text field) once, on the first frame it appears — `tick`
        // has no window to focus with.
        let dialog_layer = self.dialog.clone();
        if let Some(dialog) = &dialog_layer {
            if self.dialog_focus_pending {
                self.dialog_focus_pending = false;
                window.focus(&dialog.read(cx).focus_handle(cx));
            }
        }
        // A custom-UI surface owns the keyboard while it is open; the newest
        // (top of the stack) takes focus, and closing the last hands focus back
        // to the composer so typing continues.
        if self.custom_ui_focus_pending {
            self.custom_ui_focus_pending = false;
            match self.custom_ui.last() {
                Some(ui) => window.focus(&ui.read(cx).focus_handle(cx)),
                None => self.input.read(cx).focus(window),
            }
        }
        // The inline approval bar owns the keyboard the same way.
        if self.approval_focus_pending {
            self.approval_focus_pending = false;
            window.focus(&self.approval_focus);
        }
        // The update modal owns the keyboard (Escape dismisses) once open.
        if self.updater_dialog_focus_pending {
            self.updater_dialog_focus_pending = false;
            window.focus(&self.updater_dialog_focus);
        }
        // The inline ask panel owns the keyboard while it is open.
        if self.ask_focus_pending {
            self.ask_focus_pending = false;
            let focus = self.ask_focus.clone();
            window.focus(&focus);
        }
        // Settings → MCP hands the keyboard to the server list so ↑/↓ walk
        // the rows without a mouse.
        if self.mcp_focus_pending {
            self.mcp_focus_pending = false;
            if self.settings_open && self.settings_section == SettingsSection::Mcp {
                window.focus(&self.mcp_list_focus);
            }
        }

        div()
            .size_full()
            .flex()
            .relative()
            .bg(theme.background)
            .text_color(theme.text_primary)
            .font_family(theme::ui_font_family())
            // Dropping files anywhere in the window attaches them (the
            // composer highlights when the drag passes over it).
            .on_drop(cx.listener(Self::on_file_drop))
            // ── sidebar ── (hidden while the settings surface is open —
            // settings is a full-window surface with its own nav, like the
            // reference UI). The panel stays mounted and slides: an animated
            // outer width clips a fixed-width inner column, so the content
            // never reflows mid-slide. The titlebar row above is outside the
            // panel, so the controls hold their place while it moves.
            .children((!self.settings_open).then(|| {
                let open = sidebar_shown;
                let panel_w = f32::from(self.sidebar_width);
                let panel = div()
                    .flex_none()
                    .h_full()
                    .overflow_hidden()
                    .bg(theme.surface)
                    .child(
                        div()
                            .id("sidebar")
                            .relative()
                            .w(self.sidebar_width)
                            .h_full()
                            .flex()
                            .flex_col()
                            // Keyboard navigation: while this column holds
                            // focus (⌘⇧B) the `Sidebar` context routes the
                            // arrows/Enter/Escape to the row cursor.
                            .track_focus(&self.sidebar_focus)
                            .key_context("Sidebar")
                            // traffic-light strip (drag region); the window's
                            // left controls float above it in the titlebar
                            // overlay — right-aligned against this column's
                            // edge while the sidebar is open.
                            .child(window_drag_region(div().h(px(TOP_BAR_H)).w_full()))
                            // Resize handle: drag the sidebar's right edge to
                            // adjust its width. Kept fully inside the panel so
                            // the slide wrapper's clip doesn't halve its hit
                            // area. The drag-move listener lives on the root so
                            // the drag keeps tracking beyond the handle.
                            .child(
                                div()
                                    .id("sidebar-resize-handle")
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .right_0()
                                    .w(px(6.))
                                    .cursor(CursorStyle::ResizeLeftRight)
                                    .hover(|style| style.bg(theme.accent.opacity(0.4)))
                                    .on_drag(SidebarResize, |_, _, _, cx| cx.new(|_| DragGhost)),
                            )
                            // brand — the wordmark, centred over the nav
                            // column and held to a quiet size. White mark on
                            // dark sidebars; the dark-ink mark on light ones,
                            // where the white wordmark vanishes.
                            .child(
                                div()
                                    .px_3()
                                    .pt(px(2.))
                                    .pb_3()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(embedded_image_w(
                                        if theme.mode == ThemeMode::Light {
                                            crate::app_icon::LOGO_DARK_ASSET
                                        } else {
                                            crate::app_icon::LOGO_ASSET
                                        },
                                        px(76.),
                                    )),
                            )
                            // nav — the primary action (New Task); the
                            // utility icons live in the footer
                            .child(
                                div()
                                    .px_3()
                                    .pt_1()
                                    .pb_2()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(self.sidebar_new_task_button(theme, cx)),
                            )
                            // session list (scrolls), grouped by workspace — or
                            // one flat list when grouping is off. The section
                            // header stays mounted either way so the view
                            // toggle is reachable even with no sessions.
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    // section label — anchors the list below the nav;
                                    // the view control on its right opens the options
                                    .child(
                                        div()
                                            .px(px(14.))
                                            .pb(px(2.))
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(TextSize::Small.px(&theme))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(theme.text_muted)
                                                    .child(
                                                        if self.sidebar_group_by
                                                            == SidebarGroupBy::OneList
                                                        {
                                                            tr!("sidebar.all_sessions")
                                                        } else {
                                                            tr!("sidebar.projects")
                                                        },
                                                    ),
                                            )
                                            .child(sidebar_options_button(
                                                self.sidebar_group_by,
                                                self.workspace_sort,
                                                self.sidebar_archived_filter,
                                                self.sidebar_sort_menu,
                                                cx.entity(),
                                                theme,
                                            )),
                                    )
                                    .child(if side_rows.is_empty() {
                                        empty_sessions_state(theme).into_any_element()
                                    } else {
                                        div()
                                            .id("sidebar-sessions")
                                            .flex_1()
                                            .min_h_0()
                                            .px_2()
                                            .relative()
                                            .child(
                                                div()
                                                    .w_full()
                                                    .h_full()
                                                    .relative()
                                                    // Clips the pinned header as
                                                    // the next group pushes it
                                                    // up past the list top.
                                                    .overflow_hidden()
                                                    .child(
                                                        list(
                                                            self.sidebar_list.clone(),
                                                            move |ix, _window, cx| {
                                                                let row_workspace_menu =
                                                                    if sticky_ix == Some(ix) {
                                                                        None
                                                                    } else {
                                                                        workspace_menu
                                                                            .as_ref()
                                                                            .as_ref()
                                                                    };
                                                                render_side_row(
                                                                    &side_rows,
                                                                    &sessions_data,
                                                                    active_path.as_deref(),
                                                                    ix,
                                                                    &this,
                                                                    agent_running,
                                                                    &running_paths,
                                                                    &pinned,
                                                                    &archived_paths,
                                                                    show_workspace,
                                                                    &live_paths,
                                                                    &marks,
                                                                    &workspace_icons,
                                                                    session_menu.as_ref().as_ref(),
                                                                    row_workspace_menu,
                                                                    sidebar_cursor == Some(ix),
                                                                    *theme::get(cx),
                                                                )
                                                                .into_any_element()
                                                            },
                                                        )
                                                        .w_full()
                                                        .h_full(),
                                                    )
                                                    .children(sticky_header),
                                            )
                                            .into_any_element()
                                    }),
                            )
                            // footer — connection status, then the utility
                            // icons on the last edge, set off from the session
                            // list by a hairline
                            .child(
                                div()
                                    .id("sidebar-footer")
                                    .debug_selector(|| "sidebar-footer".to_string())
                                    .h(px(44.))
                                    .px_3()
                                    .border_t_1()
                                    .border_color(theme.border)
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(DynamicSpacing::Base06.px(&theme))
                                            .child(div().size(px(6.)).rounded_full().bg(
                                                if self.client.is_some() {
                                                    theme.success
                                                } else {
                                                    theme.error
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .text_size(TextSize::Small.px(&theme))
                                                    .text_color(theme.text_muted)
                                                    .child(if self.client.is_some() {
                                                        tr!("status.connected")
                                                    } else {
                                                        tr!("status.offline")
                                                    }),
                                            ),
                                    )
                                    .child(div().flex_1())
                                    .when_some(
                                        self.sidebar_updater_button(theme, cx),
                                        |footer, button| {
                                            footer.child(button).child(div().w(px(8.)))
                                        },
                                    )
                                    .child(self.sidebar_footer_actions(theme, cx)),
                            ),
                    );
                let gen = self.sidebar_slide_gen;
                if gen == 0 || theme::reduce_motion(cx) {
                    // Settled state — including the first frame, so the panel
                    // doesn't slide open on launch.
                    panel
                        .w(px(if open { panel_w } else { 0. }))
                        .when(open, |p| p.border_r_1().border_color(theme.border))
                        .into_any_element()
                } else {
                    panel
                        .with_animation(
                            ElementId::Name(format!("sidebar-slide-{gen}").into()),
                            Animation::new(Duration::from_millis(SIDEBAR_SLIDE_MS))
                                .with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                            move |el, d| {
                                let w = if open {
                                    panel_w * d
                                } else {
                                    panel_w * (1.0 - d)
                                };
                                let el = el.w(px(w));
                                if w > 0.5 {
                                    el.border_r_1().border_color(theme.border)
                                } else {
                                    el
                                }
                            },
                        )
                        .into_any_element()
                }
            }))
            // ── main ──
            .child(if self.settings_open {
                // Settings owns the whole column — it has its own nav.
                self.render_settings(cx).into_any_element()
            } else if !self.dependencies_ready() {
                // Missing runtime pieces (pi / node): show the setup page
                // with install commands instead of the empty composer.
                self.render_onboarding(cx).into_any_element()
            } else if self.setup_open {
                // The same page on request, from Settings → About.
                self.render_onboarding(cx).into_any_element()
            } else {
                // The shared top bar is always the column's first row. A
                // feature page (Files / Git / Usage) opens as a card below it;
                // the chat fills the column itself.
                let empty = self.transcript.is_empty();
                // `new_task_backdrop` sets `backdrop_live` when it actually
                // paints the field; nothing above needs to guess.
                // top bar — the window's left controls float over it in the
                // titlebar overlay, so its leading clears them when the sidebar
                // is collapsed. The drag spacer between the title and the right
                // cluster drags the window.
                let main_top_bar = {
                    let bar = div()
                        .h(px(TOP_BAR_H))
                        .flex_none()
                        .w_full()
                        .flex()
                        .items_center()
                        // The bar spans the main area to the window's right
                        // edge, so it owns the caption buttons.
                        .pr(px(if platform::draws_window_controls() {
                            platform::WINDOW_CONTROLS_W
                        } else {
                            12.
                        }))
                        .child({
                            let left = div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .h_full()
                                .flex()
                                .items_center();
                            if let Some(leading) =
                                git_leading.or(usage_leading).or(worktrees_leading)
                            {
                                // A feature page's leading controls (Back and the
                                // page title) are interactive, so they sit outside
                                // the drag region — pressing them must not start a
                                // window move. The bar after them still drags the
                                // window.
                                left.child(leading)
                                    .child(window_drag_region(div().flex_1().min_w_0().h_full()))
                            } else {
                                window_drag_region(left).child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .truncate()
                                        .text_size(TextSize::Default.px(&theme))
                                        .text_color(theme.text_secondary)
                                        .child(session_display_title(
                                            self.session_name.as_deref(),
                                            self.current_title.as_deref(),
                                        )),
                                )
                            }
                        })
                        .child(top_controls);
                    let gen = self.sidebar_slide_gen;
                    if gen == 0 || theme::reduce_motion(cx) {
                        bar.pl(px(if sidebar_shown { 20. } else { TITLEBAR_LEADING }))
                            .into_any_element()
                    } else {
                        let expanding = sidebar_shown;
                        bar.with_animation(
                            ElementId::Name(format!("topbar-lead-{gen}").into()),
                            Animation::new(Duration::from_millis(SIDEBAR_SLIDE_MS))
                                .with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                            move |el, d| {
                                let (from, to) = if expanding {
                                    (TITLEBAR_LEADING, 20.)
                                } else {
                                    (20., TITLEBAR_LEADING)
                                };
                                el.pl(px(from + (to - from) * d))
                            },
                        )
                        .into_any_element()
                    }
                };
                let body: AnyElement = if pane_full {
                    // The expanded Review pane owns the page; the chat is not
                    // just covered, it is skipped — same as a feature page.
                    div().into_any_element()
                } else if viewer_open {
                    self.file_viewer.clone().into_any_element()
                } else if self.git_open {
                    self.git_panel.clone().into_any_element()
                } else if self.usage_open {
                    self.usage.clone().into_any_element()
                } else if self.worktrees_open {
                    self.render_worktrees_page(cx).into_any_element()
                } else {
                    // chat body — transcript/empty, composer, terminal
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .flex_col()
                        .relative()
                        // transcript (centered column) or empty state
                        .child(if empty {
                            self.render_empty_state(main_width, cx).into_any_element()
                        } else {
                            div()
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .relative()
                                .child(self.transcript.render(
                                    review_workspace.as_deref(),
                                    window.viewport_size().height,
                                    main_width,
                                    Some(self.review_opener(cx)),
                                    Some(self.image_opener(cx)),
                                    self.search_hits(),
                                    self.search_active(),
                                    cx,
                                ))
                                // In-transcript find bar (⌘F), floating over the
                                // top-right of the transcript.
                                .children(self.transcript_search_bar(cx))
                                .into_any_element()
                        })
                        // floating composer + status bar — one centered column
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .px_4()
                                .pb_4()
                                // One centered column: composer + status bar share
                                // the same max width so the folder/meta row always
                                // aligns to the composer's edges.
                                .child(
                                    div()
                                        // A stable element identity keeps the
                                        // popovers and inline bars below from
                                        // re-keying when a sibling appears, so
                                        // their animations never restart mid-way.
                                        .id("composer-column")
                                        .max_w(px(CONTENT_MAX_W))
                                        .w_full()
                                        .flex()
                                        .flex_col()
                                        // `/`-command and `@`-file menu — anchored
                                        // above the composer box (same deferred
                                        // + anchored pattern as the chip pickers)
                                        .children(self.autocomplete_popup(cx))
                                        // Command/protocol failures, above the
                                        // queue and composer.
                                        .children(self.error_banner(theme, cx))
                                        // In-flight retry / compaction state —
                                        // persistent while active, never a
                                        // transient status line.
                                        .children(self.run_status_strip(cx))
                                        // Access-guard approval — inline, above
                                        // the queue and composer (no scrim modal).
                                        .children(self.ask_panel(cx))
                                        .children(self.approval_bar(cx))
                                        // Queued follow-ups wait here (sticky above
                                        // the composer) until the task finishes.
                                        .children(self.queue_bar(cx))
                                        // Extension `setWidget` blocks placed
                                        // above the editor.
                                        .children(self.extension_widgets_above(cx))
                                        // composer box — the picker popups are
                                        // anchored above their own chips
                                        .child(
                                            div()
                                                .id("composer-box")
                                                .w_full()
                                                .relative()
                                                .bg(theme.surface_secondary)
                                                .border_1()
                                                .border_color(if self.file_drag_hovered {
                                                    theme.accent
                                                } else {
                                                    theme.border
                                                })
                                                .rounded(Radius::XXLarge.px(&theme))
                                                .shadow(theme.composer_shadow())
                                                .px(DynamicSpacing::Base16.px(&theme))
                                                .pt(DynamicSpacing::Base12.px(&theme))
                                                .pb(DynamicSpacing::Base12.px(&theme))
                                                // Base interface font for the input
                                                // (scales with the UI font-size
                                                // setting); the editor inherits it.
                                                .text_size(TextSize::Default.px(&theme))
                                                .flex()
                                                .flex_col()
                                                .gap(DynamicSpacing::Base16.px(&theme))
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(Self::on_composer_click),
                                                )
                                                .on_drag_move(cx.listener(Self::on_file_drag_move))
                                                .children(self.attachments_row(cx))
                                                .child(self.input.clone())
                                                .child(self.composer_row(composer_compact, cx))
                                                // Drop-target overlay: fades
                                                // in over the box while files are
                                                // dragged across it. Absolute, so
                                                // highlighting never shifts layout.
                                                .children(self.file_drag_hovered.then(|| {
                                                    let overlay = div()
                                                        .absolute()
                                                        .inset_0()
                                                        .rounded(Radius::XXLarge.px(&theme))
                                                        .bg(theme.surface_secondary.opacity(0.92))
                                                        .border_1()
                                                        .border_color(theme.accent)
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .gap_2()
                                                        .child(icon(
                                                            "icons/plus.svg",
                                                            IconSize::Small.px(&theme),
                                                            theme.accent_text,
                                                        ))
                                                        .child(
                                                            div()
                                                                .text_size(
                                                                    TextSize::Small.px(&theme),
                                                                )
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .text_color(theme.accent_text)
                                                                .child(tr!(
                                                                    "composer.drop_to_attach"
                                                                )),
                                                        );
                                                    if theme::reduce_motion(cx) {
                                                        overlay.into_any_element()
                                                    } else {
                                                        overlay
                                                            .with_animation(
                                                                "drop-overlay",
                                                                Animation::new(
                                                                    Duration::from_millis(120),
                                                                ),
                                                                |overlay, delta| {
                                                                    overlay.opacity(delta)
                                                                },
                                                            )
                                                            .into_any_element()
                                                    }
                                                })),
                                        )
                                        // Extension `setWidget` blocks placed
                                        // below the editor.
                                        .children(self.extension_widgets_below(cx))
                                        .child(self.status_bar(&workspace_label, cx)),
                                ),
                        )
                        // bottom terminal panel — the last row of the page, under
                        // the composer, so it hugs the window's bottom edge
                        .children(
                            terminal_visible
                                .then(|| self.terminal_panel.clone().into_any_element()),
                        )
                        .into_any_element()
                };
                // The top bar stays put; a feature page opens as a card below
                // it, the chat fills the column itself.
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .min_h_0()
                    .relative()
                    .children(if feature_open || pane_full {
                        None
                    } else {
                        self.new_task_backdrop(theme, self.backdrop_phase, empty)
                    })
                    .child(main_top_bar)
                    // The top bar spans the whole main area; the docks and
                    // the chat/feature sit side by side below it.
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .flex()
                            .relative()
                            .child(if pane_full {
                                div().into_any_element()
                            } else if feature_open {
                                feature_card(theme, body)
                            } else {
                                body
                            })
                            .children(
                                explorer_visible
                                    .then(|| self.project_panel.clone().into_any_element()),
                            )
                            .children(
                                pane_visible.then(|| self.sidepane.clone().into_any_element()),
                            ),
                    )
                    .into_any_element()
            })
            // ── titlebar controls ── a fixed overlay pinned just past the
            // macOS traffic lights, above both the sidebar and the main column.
            // Shown on every surface except Settings, which owns its own nav
            // column — including the Git/Usage pages when the sidebar is
            // collapsed, so the toggle (and history arrows) stay reachable;
            // those pages inset their own headers to clear it. The controls
            // right-align inside the sidebar's strip when it is open (hugging
            // its edge, and tracking it through a resize or the slide) and
            // fall back to the fixed lead when it is collapsed; on Windows,
            // which has no left-hand OS buttons, they take that fixed lead
            // whether it is open or not. The container
            // is not itself a hitbox, so the drag strip beneath still drags the
            // window in the gaps between buttons while each button takes its
            // own clicks.
            .children((!self.settings_open).then(|| {
                let pinned = titlebar_controls_left(false, f32::from(self.sidebar_width));
                let hugging = titlebar_controls_left(true, f32::from(self.sidebar_width));
                let controls = div()
                    .absolute()
                    .top_0()
                    .h(px(TOP_BAR_H))
                    .flex()
                    .items_center()
                    .child(self.titlebar_left_controls(theme, cx));
                let gen = self.sidebar_slide_gen;
                if gen == 0 || theme::reduce_motion(cx) {
                    controls
                        .left(px(if sidebar_shown { hugging } else { pinned }))
                        .into_any_element()
                } else {
                    // Same easing and duration as the panel's own slide, so the
                    // chips ride the edge instead of snapping to it.
                    let expanding = sidebar_shown;
                    controls
                        .with_animation(
                            ElementId::Name(format!("titlebar-lead-{gen}").into()),
                            Animation::new(Duration::from_millis(SIDEBAR_SLIDE_MS))
                                .with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                            move |el, d| {
                                let (from, to) = if expanding {
                                    (pinned, hugging)
                                } else {
                                    (hugging, pinned)
                                };
                                el.left(px(from + (to - from) * d))
                            },
                        )
                        .into_any_element()
                }
            }))
            // ── window caption buttons ── the app owns the caption on the
            // platforms where it draws it (`platform::draws_window_controls`),
            // so these sit above *every* surface — settings, git, usage,
            // onboarding, and the session view — and only the headers that
            // reach the window's right edge keep their content clear of them.
            // Drawn before the deferred layers, so an open popover still wins.
            .children(platform::draws_window_controls().then(|| {
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h(px(TOP_BAR_H))
                    .flex()
                    .items_center()
                    .child(window_controls(theme, window.is_maximized()))
                    .into_any_element()
            }))
            // ── command palette (⌘P) — a full-window deferred layer above
            // every other floating surface; the entity renders its own
            // absolute scrim + centered card.
            .children(
                self.command_palette
                    .clone()
                    .map(|palette| command_palette::layer(palette).into_any_element()),
            )
            // ── extension dialog (select / confirm / input / editor) — a
            // blocking modal above every other surface; pi holds the run until
            // the user answers. It cancels the incoming request otherwise.
            .children(dialog_layer.map(|dialog| crate::dialog::layer(dialog).into_any_element()))
            // ── extension custom UI (`ctx.ui.custom`) — the component's own
            // rendered frames, stacked with the newest on top.
            .children(
                self.custom_ui
                    .iter()
                    .map(|ui| crate::custom_ui::layer(ui.clone()).into_any_element()),
            )
            // ── worktrees dialog — create / rename / move / remove,
            // opened from the page and its row menus.
            .children(self.worktree_dialog_layer(theme, cx.entity(), cx))
            // ── update modal — the search, changelog, and install decision,
            // opened by the download control and Check for Updates. Below the
            // extension dialog (a run blocks on it) and the lightbox.
            .children(self.updater_dialog_layer(window, cx))
            // ── image lightbox — full-window, above everything; opened from a
            // transcript image tile, dismissed by click or Escape.
            .children(
                self.lightbox
                    .clone()
                    .map(|image| self.lightbox_layer(image, cx)),
            )
            // ── toasts — the in-app stack, topmost so a notification is
            // never buried by whatever surface happens to be open.
            .children(self.toast_layer(cx))
            .track_focus(&self.focus_handle(cx))
            // Sidebar resize: fires for every mouse move while the handle
            // drag is active, wherever the pointer travels.
            .on_drag_move(cx.listener(
                |app: &mut Self,
                 event: &DragMoveEvent<SidebarResize>,
                 _: &mut Window,
                 cx: &mut Context<Self>| {
                    let max = event.bounds.size.width - px(400.);
                    let width = event
                        .event
                        .position
                        .x
                        .clamp(px(SIDEBAR_MIN_W), max.max(px(SIDEBAR_MIN_W)));
                    if width != app.sidebar_width {
                        app.sidebar_width = width;
                        crate::layout::set_sidebar_width(f32::from(width));
                        cx.notify();
                    }
                },
            ))
            .on_drag_move(cx.listener(
                |app: &mut Self,
                 event: &DragMoveEvent<crate::explorer::ExplorerResize>,
                 _: &mut Window,
                 cx: &mut Context<Self>| {
                    // The dock sits between the main column and the Review
                    // pane, so its width is the pointer's distance to its
                    // right edge — which the pane owns when it is open.
                    let pane = if app.sidepane.read(cx).is_open()
                        && !app.settings_open
                        && !app.usage_open
                        && app.dependencies_ready()
                    {
                        app.sidepane.read(cx).width()
                    } else {
                        px(0.)
                    };
                    let width = event.bounds.size.width - pane - event.event.position.x;
                    app.project_panel
                        .update(cx, |panel, cx| panel.set_width(width, cx));
                },
            ))
            .on_drag_move(cx.listener(
                |app: &mut Self,
                 event: &DragMoveEvent<SidePaneResize>,
                 _: &mut Window,
                 cx: &mut Context<Self>| {
                    // The pane hugs the window's right edge, so its width is
                    // the distance from the pointer to that edge.
                    let width = event.bounds.size.width - event.event.position.x;
                    let max = event.bounds.size.width - px(PANE_MAX_RESERVE);
                    app.sidepane.update(cx, |pane, cx| {
                        pane.set_width(width.min(max), cx);
                    });
                },
            ))
            .on_drag_move(cx.listener(
                |app: &mut Self,
                 event: &DragMoveEvent<TerminalResize>,
                 _: &mut Window,
                 cx: &mut Context<Self>| {
                    // The panel hugs the window's bottom edge, so its height is
                    // the distance from the pointer to that edge, capped so the
                    // transcript above it keeps a usable height.
                    let max = (event.bounds.size.height - px(TERMINAL_MAIN_RESERVE))
                        .max(px(TERMINAL_MAIN_RESERVE));
                    let height = (event.bounds.size.height - event.event.position.y).min(max);
                    app.terminal_panel
                        .update(cx, |panel, cx| panel.set_height(height, cx));
                },
            ))
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_steer))
            .on_action(cx.listener(Self::on_send_alternate))
            .on_action(cx.listener(Self::on_autocomplete_accept))
            .on_action(cx.listener(Self::on_abort))
            .on_action(cx.listener(Self::on_new_session))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_copy_last_response))
            .on_action(cx.listener(Self::on_prev_turn))
            .on_action(cx.listener(Self::on_next_turn))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_open_about))
            .on_action(cx.listener(Self::on_toggle_usage))
            .on_action(cx.listener(Self::on_toggle_project_panel))
            .on_action(cx.listener(Self::on_close_files))
            .on_action(cx.listener(Self::on_toggle_terminal))
            .on_action(cx.listener(Self::on_new_terminal_tab))
            .on_action(cx.listener(Self::on_close_terminal_tab))
            .on_action(cx.listener(Self::on_next_terminal_tab))
            .on_action(cx.listener(Self::on_prev_terminal_tab))
            .on_action(cx.listener(Self::on_toggle_sidebar_action))
            .on_action(cx.listener(Self::on_focus_sessions))
            .on_action(cx.listener(Self::on_open_session_slot))
            .on_action(cx.listener(Self::on_next_session))
            .on_action(cx.listener(Self::on_prev_session))
            .on_action(cx.listener(Self::on_sidebar_prev))
            .on_action(cx.listener(Self::on_sidebar_next))
            .on_action(cx.listener(Self::on_sidebar_home))
            .on_action(cx.listener(Self::on_sidebar_end))
            .on_action(cx.listener(Self::on_sidebar_confirm))
            .on_action(cx.listener(Self::on_sidebar_close))
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .on_action(cx.listener(Self::on_check_for_updates))
            .on_action(cx.listener(Self::on_toggle_search))
            .on_action(cx.listener(Self::on_toggle_model_menu))
            .on_action(cx.listener(Self::on_toggle_thinking_menu))
            .on_action(cx.listener(Self::on_review_changes))
            .on_action(cx.listener(Self::on_open_git))
            .on_action(cx.listener(Self::on_open_shortcut_help))
            .on_action(cx.listener(Self::on_review_close))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            .on_action(cx.listener(|this, _: &crate::GitTabChanges, window, cx| {
                this.on_git_tab(0, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::GitTabHistory, window, cx| {
                this.on_git_tab(1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::GitTabGraph, window, cx| {
                this.on_git_tab(2, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::GitTabStashes, window, cx| {
                this.on_git_tab(3, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::GitTabIssues, window, cx| {
                this.on_git_tab(4, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::GitTabPulls, window, cx| {
                this.on_git_tab(5, window, cx)
            }))
    }
}

impl OrbitApp {
    /// The extension widgets placed above the composer, if any.
    pub(super) fn extension_widgets_above(&self, cx: &Context<Self>) -> Option<AnyElement> {
        self.extension_widgets_element(WidgetPlacement::AboveEditor, cx)
    }

    /// The extension widgets placed below the composer, if any.
    pub(super) fn extension_widgets_below(&self, cx: &Context<Self>) -> Option<AnyElement> {
        self.extension_widgets_element(WidgetPlacement::BelowEditor, cx)
    }

    /// Render every keyed `setWidget` block for one placement as a small
    /// monospace card. The lines keep the extension's own SGR coloring (see
    /// [`crate::widgets`]), so a todo list or status block reads as intended.
    fn extension_widgets_element(
        &self,
        placement: WidgetPlacement,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let items: Vec<&ExtensionWidget> = self
            .extension_widgets
            .iter()
            .filter(|widget| widget.placement == placement && !widget.lines.is_empty())
            .collect();
        if items.is_empty() {
            return None;
        }
        let theme = *theme::get(cx);
        let font = ext_widgets::widget_font();
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base06.px(&theme));
        for widget in items {
            let mut card = div()
                .id(ElementId::Name(format!("ext-widget-{}", widget.key).into()))
                .w_full()
                .px(DynamicSpacing::Base12.px(&theme))
                .py(DynamicSpacing::Base08.px(&theme))
                .rounded(Radius::XLarge.px(&theme))
                .border_1()
                .border_color(theme.border)
                .bg(theme.code_bg)
                .font_family(theme::code_font_family())
                .text_size(theme.code_px(12.))
                .line_height(theme.code_px(18.))
                .text_color(theme.code_text)
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base02.px(&theme));
            for line in &widget.lines {
                let (text, runs) = ext_widgets::styled_line(line, &theme, &font);
                card = card.child(StyledText::new(text).with_runs(runs));
            }
            column = column.child(card);
        }
        Some(column.into_any_element())
    }

    /// Bottom row inside the composer: the "+" add menu and the access-mode
    /// chip on the left; model / thinking chips and the round send button on
    /// the right. `compact` (narrow window) drops the access chip and clamps
    /// the model label so Send always stays reachable.
    pub(super) fn composer_row(
        &self,
        compact: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id("composer-row")
            .flex()
            .items_center()
            .gap_2()
            .child(self.add_menu_button(cx))
            // Access mode: a real control now — it selects the guard policy
            // the bundled extension enforces. First thing to yield when the
            // row gets narrow.
            .when(!compact, |row| row.child(self.access_chip(cx)))
            // Workflow mode: the session's scope (Plan / Build / Ask), enforced
            // by the workflow extension. Sits beside the access chip on every
            // composer, including the new-task page — the scope belongs with
            // the other agent controls, not as a control block in the page's
            // empty state.
            .when(!compact, |row| row.child(self.workflow_chip(cx)))
            .child(div().flex_1())
            .child(self.model_chip(compact, cx))
            .children(self.thinking_chip(cx))
            .children(self.steer_button(cx))
            .child(self.send_button(cx))
    }

    /// The access-mode chip: a lock glyph, the active mode's label, and a
    /// caret that turns over when the picker opens. Ghost style until
    /// hovered/open, matching the model chip.
    pub(super) fn access_chip(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        // Full access is the default, so its name is not news: the chip keeps the
        // glyph alone and names the mode on hover instead. A tightened mode
        // (Supervised, Auto-accept edits) is a deliberate departure from the
        // default, so that one keeps its label — there the label is the warning,
        // and it should be readable without hovering.
        //
        // The rule reads the mode only, never the open menu: a chip that grew a
        // label on click would shove the workflow chip sideways under the
        // pointer, and the open picker already names the active mode.
        let label = self.access_mode.label();
        let terse = self.access_mode == AccessMode::default();
        div()
            .flex()
            .flex_col()
            .items_start()
            .children(self.access_popup(cx))
            .child(
                button_frame(div().id("access-chip"), &theme, ButtonSize::Default)
                    .debug_selector(|| "access-chip".to_string())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .when(self.access_menu_open, |chip| {
                        chip.bg(theme.surface_active).text_color(theme.text_primary)
                    })
                    .tip(label.clone())
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(Self::on_access_trigger_click),
                    )
                    .child(icon(
                        self.access_mode.icon(),
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_muted,
                    ))
                    .when(!terse, |chip| {
                        chip.child(
                            div()
                                .debug_selector(|| "access-chip-label".to_string())
                                .text_color(if self.access_menu_open {
                                    theme.text_primary
                                } else {
                                    theme.text_secondary
                                })
                                .child(label),
                        )
                        .child(Self::chip_caret(
                            self.access_menu_open,
                            theme.text_primary,
                            "access-caret-turn",
                            cx,
                        ))
                    }),
            )
    }

    /// The shared chip caret. It turns a half-turn when the picker opens —
    /// animated on open (reduce-motion aware) so the turn reads as a
    /// transition, and resting flat when closed so there is no reverse
    /// flicker. `fg` is the caret color while open.
    pub(super) fn chip_caret(
        open: bool,
        fg: gpui::Hsla,
        animation_id: &'static str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = *theme::get(cx);
        let svg = gpui::svg()
            .path("icons/chevron-down.svg")
            .flex_none()
            .size(IconSize::XSmall.px(&theme))
            .text_color(if open { fg } else { theme.text_muted })
            .into_any_element();
        if !open {
            return svg;
        }
        if theme::reduce_motion(cx) {
            return gpui::svg()
                .path("icons/chevron-down.svg")
                .flex_none()
                .size(IconSize::XSmall.px(&theme))
                .text_color(fg)
                .with_transformation(Transformation::rotate(radians(std::f32::consts::PI)))
                .into_any_element();
        }
        gpui::svg()
            .path("icons/chevron-down.svg")
            .flex_none()
            .size(IconSize::XSmall.px(&theme))
            .text_color(fg)
            .with_animation(
                animation_id,
                Animation::new(AnimationDuration::Fast.duration())
                    .with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                |svg, d| {
                    svg.with_transformation(Transformation::rotate(radians(
                        std::f32::consts::PI * d,
                    )))
                },
            )
            .into_any_element()
    }

    /// The access-mode picker popup, while open. Minimal rows: an icon tile,
    /// the mode name over its one-line description, and an accent check on the
    /// active mode. The popup owns the keyboard via the `AccessMenu` context
    /// and eases in rather than popping.
    pub(super) fn access_popup(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.access_menu_open {
            return None;
        }
        let theme = *theme::get(cx);
        let this = cx.weak_entity();
        let mut list = div().w_full().flex().flex_col();
        for (ix, mode) in AccessMode::ALL.iter().enumerate() {
            let mode = *mode;
            let highlighted = ix == self.access_menu_highlight;
            let selected = mode == self.access_mode;
            let this = this.clone();
            let entry = div().id(ElementId::NamedInteger("access-row".into(), ix as u64));
            list = list.child(
                picker_entry(entry, &theme)
                    .cursor_pointer()
                    .when(selected || highlighted, |row| row.bg(theme.selection))
                    .when(!selected && !highlighted, |row| {
                        row.hover(|style| style.bg(theme.surface_hover))
                    })
                    .on_hover(move |hovered, _, cx| {
                        if *hovered {
                            this.update(cx, |app, cx| {
                                if app.access_menu_highlight != ix {
                                    app.access_menu_highlight = ix;
                                    cx.notify();
                                }
                            })
                            .ok();
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let this = cx.weak_entity();
                        move |_, window, cx| {
                            this.update(cx, |app, cx| app.run_access_menu_item(ix, window, cx))
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
                                mode.icon(),
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
                                    .child(mode.label()),
                            )
                            .child(
                                div()
                                    .mt(DynamicSpacing::Base02.px(&theme))
                                    .whitespace_normal()
                                    .text_size(picker::SECONDARY_TEXT.px(&theme))
                                    .text_color(if selected || highlighted {
                                        theme.text_primary.opacity(0.75)
                                    } else {
                                        theme.text_muted
                                    })
                                    .child(mode.description()),
                            ),
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
            .w(px(MODE_POPUP_W))
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base01.px(&theme))
            .overflow_hidden()
            .occlude()
            .key_context("AccessMenu")
            .track_focus(&self.access_menu_focus)
            .on_action(cx.listener(Self::on_access_menu_next))
            .on_action(cx.listener(Self::on_access_menu_prev))
            .on_action(cx.listener(Self::on_access_menu_confirm))
            .on_action(cx.listener(Self::on_access_menu_close))
            .on_mouse_down_out(cx.listener(|app, _, window, cx| {
                app.menu_dismissed_at = Some(Instant::now());
                app.close_access_menu(window, cx);
            }))
            .child(list);

        let popup: AnyElement = if theme::reduce_motion(cx) {
            popup.into_any_element()
        } else {
            popup
                .with_animation(
                    "access-menu-in",
                    Animation::new(Duration::from_millis(130))
                        .with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                    |el, d| el.opacity(d),
                )
                .into_any_element()
        };

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

    /// The workflow-mode chip: the active session's scope, opening the
    /// Plan / Build / Ask picker. Ghost style until hovered/open, matching the
    /// access and model chips.
    pub(super) fn workflow_chip(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        div()
            .flex()
            .flex_col()
            .items_start()
            .children(self.workflow_popup(cx))
            .child(
                button_frame(div().id("workflow-chip"), &theme, ButtonSize::Default)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .when(self.workflow_menu_open, |chip| {
                        chip.bg(theme.surface_active).text_color(theme.text_primary)
                    })
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(Self::on_workflow_trigger_click),
                    )
                    .child(icon(
                        self.workflow_mode.icon(),
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .text_color(if self.workflow_menu_open {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .child(self.workflow_mode.label()),
                    )
                    // Read-only modes cannot edit; the lock says so at a glance,
                    // matching the access chip's motif.
                    .when(self.workflow_mode.is_read_only(), |chip| {
                        chip.child(icon(
                            "icons/lock.svg",
                            IconSize::Indicator.px(&theme),
                            theme.text_muted,
                        ))
                    })
                    .child(Self::chip_caret(
                        self.workflow_menu_open,
                        theme.text_primary,
                        "workflow-caret-turn",
                        cx,
                    )),
            )
    }

    /// The workflow-mode picker popup, while open. Same construction as the
    /// access popup: an icon tile, the mode over its one-line hint, and an
    /// accent check on the active mode.
    pub(super) fn workflow_popup(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.workflow_menu_open {
            return None;
        }
        let theme = *theme::get(cx);
        let this = cx.weak_entity();
        let mut list = div().w_full().flex().flex_col();
        for (ix, mode) in WorkflowMode::ALL.iter().enumerate() {
            let mode = *mode;
            let highlighted = ix == self.workflow_menu_highlight;
            let selected = mode == self.workflow_mode;
            let this = this.clone();
            let entry = div().id(ElementId::NamedInteger("workflow-row".into(), ix as u64));
            list = list.child(
                picker_entry(entry, &theme)
                    .cursor_pointer()
                    .when(selected || highlighted, |row| row.bg(theme.selection))
                    .when(!selected && !highlighted, |row| {
                        row.hover(|style| style.bg(theme.surface_hover))
                    })
                    .on_hover(move |hovered, _, cx| {
                        if *hovered {
                            this.update(cx, |app, cx| {
                                if app.workflow_menu_highlight != ix {
                                    app.workflow_menu_highlight = ix;
                                    cx.notify();
                                }
                            })
                            .ok();
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let this = cx.weak_entity();
                        move |_, window, cx| {
                            this.update(cx, |app, cx| app.run_workflow_menu_item(ix, window, cx))
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
                                mode.icon(),
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
                                    .child(mode.label()),
                            )
                            .child(
                                div()
                                    .mt(DynamicSpacing::Base02.px(&theme))
                                    .whitespace_normal()
                                    .text_size(picker::SECONDARY_TEXT.px(&theme))
                                    .text_color(if selected || highlighted {
                                        theme.text_primary.opacity(0.75)
                                    } else {
                                        theme.text_muted
                                    })
                                    .child(mode.description()),
                            ),
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
            .w(px(MODE_POPUP_W))
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base01.px(&theme))
            .overflow_hidden()
            .occlude()
            .key_context("WorkflowMenu")
            .track_focus(&self.workflow_menu_focus)
            .on_action(cx.listener(Self::on_workflow_menu_next))
            .on_action(cx.listener(Self::on_workflow_menu_prev))
            .on_action(cx.listener(Self::on_workflow_menu_confirm))
            .on_action(cx.listener(Self::on_workflow_menu_close))
            .on_mouse_down_out(cx.listener(|app, _, window, cx| {
                app.menu_dismissed_at = Some(Instant::now());
                app.close_workflow_menu(window, cx);
            }))
            .child(list);

        let popup: AnyElement = if theme::reduce_motion(cx) {
            popup.into_any_element()
        } else {
            popup
                .with_animation(
                    "workflow-menu-in",
                    Animation::new(Duration::from_millis(130))
                        .with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                    |el, d| el.opacity(d),
                )
                .into_any_element()
        };

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

    /// Quiet inline hints immediately left of the context meter, including
    /// with an empty draft so running-send modes remain discoverable.
    fn composer_send_hints(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        div()
            .id("composer-send-hints")
            .debug_selector(|| "composer-send-hints".to_string())
            .min_w_0()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .text_color(theme.text_muted)
            .children(
                composer_send::shortcut_hints(theme.ui.composer_send_mode, self.is_running())
                    .into_iter()
                    .map(|(keys, action)| {
                        // Match the adjacent footer controls without adding click behavior.
                        button_frame(div(), &theme, ButtonSize::Default)
                            .flex_shrink()
                            .min_w_0()
                            .child(
                                div()
                                    .debug_selector(|| "composer-hint-keys".to_string())
                                    .flex_none()
                                    .when(cfg!(target_os = "macos"), |keys| {
                                        keys.text_size(ButtonSize::Default.icon_size().px(&theme))
                                    })
                                    .child(keys),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "composer-hint-label".to_string())
                                    .truncate()
                                    .child(action),
                            )
                    }),
            )
    }

    /// While a run is in flight and the composer holds something to send, a
    /// quiet steer control sits beside Stop: it injects the text into the live
    /// turn (`steer`) rather than queuing a follow-up. Hidden when idle or
    /// empty, since there is nothing to redirect.
    pub(super) fn steer_button(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = *theme::get(cx);
        if !self.is_running() {
            return None;
        }
        let has_text =
            !self.input.read(cx).text().trim().is_empty() || !self.attachments.is_empty();
        if !has_text {
            return None;
        }
        Some(
            icon_button_frame(div().id("steer-btn"), &theme, ButtonSize::Medium)
                .group(BUTTON_GROUP)
                .rounded_full()
                .raised(theme.overlay, &theme)
                .hover(|s| s.raised(theme.overlay_strong, &theme))
                .active(|s| s.opacity(PRESS_DIM))
                .cursor_pointer()
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.steer_current(cx)),
                )
                .tooltip({
                    let keys = format!(
                        "{}, {}",
                        SendMode::Steer.shortcut_label(theme.ui.composer_send_mode),
                        platform::shortcuts::label(platform::shortcuts::STEER),
                    );
                    let label = tr!("composer.steer_tooltip", keys = keys);
                    move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                })
                .child(icon(
                    "icons/arrow-up-right.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.accent_text,
                ))
                .into_any_element(),
        )
    }

    /// The composer's "+" button and its add menu (anchored above the
    /// button, same deferred pattern as the chip pickers). The menu carries
    /// the `AddMenu` key context, so ↑/↓/Enter/Escape drive it while open.
    pub(super) fn add_menu_button(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        div()
            .flex()
            .flex_col()
            .items_start()
            .children(self.add_menu_popup(cx))
            .child(
                icon_button_frame(div().id("attach-chip"), &theme, ButtonSize::Default)
                    .tip(tr!("composer.add"))
                    .group(BUTTON_GROUP)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .active(|s| s.opacity(PRESS_DIM))
                    .when(self.add_menu_open, |b| {
                        b.bg(theme.surface_active).text_color(theme.text_primary)
                    })
                    .child(icon(
                        "icons/plus.svg",
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_muted,
                    ))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_add_trigger_click)),
            )
    }

    /// The add menu popup, while open. Real actions only: attach an image,
    /// attach any file (by path at the caret), or start an @-mention.
    pub(super) fn add_menu_popup(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.add_menu_open {
            return None;
        }
        let theme = *theme::get(cx);
        let this = cx.weak_entity();
        let mut list = div().w_full().flex().flex_col();
        for (ix, (icon_path, label, hint)) in ADD_MENU_ITEMS.iter().enumerate() {
            let highlighted = ix == self.add_menu_highlight;
            let this = this.clone();
            let entry = div().id(ElementId::NamedInteger("add-menu-row".into(), ix as u64));
            list = list.child(
                picker_entry(entry, &theme)
                    .h(picker::entry_height(&theme))
                    .flex_none()
                    .cursor_pointer()
                    .when(highlighted, |row| row.bg(theme.selection))
                    .hover(|style| style.bg(theme.surface_hover))
                    .on_hover(move |hovered, _, cx| {
                        if *hovered {
                            this.update(cx, |app, cx| {
                                if app.add_menu_highlight != ix {
                                    app.add_menu_highlight = ix;
                                    cx.notify();
                                }
                            })
                            .ok();
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let this = cx.weak_entity();
                        move |_, window, cx| {
                            this.update(cx, |app, cx| app.run_add_menu_item(ix, window, cx))
                                .ok();
                        }
                    })
                    .child(icon(
                        icon_path,
                        context_menu::ICON.px(&theme),
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(if highlighted {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .child(tr!(*label)),
                    )
                    .when(!hint.is_empty(), |row| {
                        row.child(
                            div()
                                .ml(context_menu::keybinding_gap(&theme)
                                    - context_menu::icon_gap(&theme))
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .child(*hint),
                        )
                    }),
            );
        }

        let popup = picker_surface(div(), &theme)
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base01.px(&theme))
            .overflow_hidden()
            .occlude()
            // The menu owns the keyboard while open (`AddMenu` bindings in
            // main.rs sit deeper than the global escape/enter).
            .key_context("AddMenu")
            .track_focus(&self.add_menu_focus)
            .on_action(cx.listener(Self::on_add_menu_next))
            .on_action(cx.listener(Self::on_add_menu_prev))
            .on_action(cx.listener(Self::on_add_menu_confirm))
            .on_action(cx.listener(Self::on_add_menu_close))
            .on_mouse_down_out(cx.listener(|app, _, window, cx| {
                // Arm the click-through guard so the same click's mouse-up
                // on the "+" button doesn't re-open the menu.
                app.menu_dismissed_at = Some(Instant::now());
                app.close_add_menu(window, cx);
            }))
            .child(list);

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

    /// The anchored popup for `kind`, when that picker is open. `deferred`
    /// paints it on top of everything; `anchored` takes it out of the layout
    /// and pins its bottom-left corner just above the chip, flipping at the
    /// window edges via `snap_to_window_with_margin`.
    pub(super) fn chip_popup(&self, kind: PickerKind) -> Option<impl IntoElement + use<>> {
        self.model_selector
            .clone()
            .and_then(|(open_kind, selector)| {
                (open_kind == kind).then(move || {
                    anchored()
                        .position_mode(AnchoredPositionMode::Local)
                        .anchor(Corner::BottomLeft)
                        .offset(point(px(0.), -popover::MENU_OFFSET))
                        .snap_to_window_with_margin(popover::WINDOW_MARGIN)
                        .child(deferred(selector))
                })
            })
    }

    /// The model chip: provider glyph + model name + caret. Ghost style —
    /// configuration is secondary to the prompt, so chips carry no border
    /// or fill until hovered/open. `compact` clamps the label so narrow
    /// windows keep Send reachable.
    pub(super) fn model_chip(&self, compact: bool, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        // A run in flight dims the chip and swallows clicks until it settles.
        let disabled = self.is_running();
        let open = !disabled && self.picker_is_open(PickerKind::Model);
        div()
            .flex()
            .flex_col()
            .items_start()
            .children(self.chip_popup(PickerKind::Model))
            .child(
                button_frame(div().id("model-chip"), &theme, ButtonSize::Default)
                    .when(!disabled, |chip| {
                        chip.cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .when(open, |chip| chip.bg(theme.surface_active))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(Self::on_model_trigger_click),
                            )
                    })
                    .when(disabled, |chip| chip.cursor_not_allowed())
                    .tip(commands::tooltip(CommandId::ChooseModel, false))
                    .child(icon_dyn(
                        provider_icon(&self.model_provider),
                        ButtonSize::Default.icon_size().px(&theme),
                        provider_mark_color(
                            &self.model_provider,
                            &theme,
                            if disabled {
                                theme.text_disabled
                            } else {
                                theme.text_muted
                            },
                        ),
                    ))
                    .child(
                        div()
                            .max_w(px(if compact { 120. } else { 220. }))
                            .truncate()
                            .text_color(if disabled {
                                theme.text_disabled
                            } else if open {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .child(self.model_label.clone()),
                    )
                    .child(Self::chip_caret(
                        open,
                        theme.text_primary,
                        "model-caret-turn",
                        cx,
                    )),
            )
    }

    /// The thinking-level chip: level icon + reasoning level + caret. Ghost
    /// style, same hierarchy as the model chip.
    ///
    /// Absent when the active model offers nothing to pick. pi reports `["off"]`
    /// for a model without reasoning, and an empty list before the levels have
    /// arrived, so a chip there would advertise a setting that cannot move. It
    /// appears with the first real ladder, and ⌘T plus the palette's Choose
    /// Thinking Level keep the picker reachable whenever it has options.
    pub(super) fn thinking_chip(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.can_choose_thinking() {
            return None;
        }
        let theme = *theme::get(cx);
        // A run in flight dims the chip and swallows clicks until it settles.
        let disabled = self.is_running();
        let open = !disabled && self.picker_is_open(PickerKind::Thinking);
        Some(
            div()
                .flex()
                .flex_col()
                .items_start()
                .children(self.chip_popup(PickerKind::Thinking))
                .child(
                    button_frame(div().id("thinking-chip"), &theme, ButtonSize::Default)
                        .debug_selector(|| "thinking-chip".to_string())
                        .when(!disabled, |chip| {
                            chip.cursor_pointer()
                                .hover(|s| s.bg(theme.surface_hover))
                                .when(open, |chip| chip.bg(theme.surface_active))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(Self::on_thinking_trigger_click),
                                )
                        })
                        .when(disabled, |chip| chip.cursor_not_allowed())
                        .tip(commands::tooltip(CommandId::ChooseThinking, false))
                        .child({
                            let (path, _) = thinking_icon(&self.thinking_label, &theme);
                            icon(
                                path,
                                ButtonSize::Default.icon_size().px(&theme),
                                if disabled {
                                    theme.text_disabled
                                } else {
                                    theme.text_muted
                                },
                            )
                        })
                        .child(
                            div()
                                .text_color(if disabled {
                                    theme.text_disabled
                                } else if open {
                                    theme.text_primary
                                } else {
                                    theme.text_secondary
                                })
                                .child(thinking_display(&self.thinking_label)),
                        )
                        .child(Self::chip_caret(
                            open,
                            theme.text_primary,
                            "thinking-caret-turn",
                            cx,
                        )),
                )
                .into_any_element(),
        )
    }

    /// The new-task backdrop layer: the configured dithered image, or the
    /// default dot grid, absolutely filling the whole chat column below the
    /// top bar — behind the empty state, the floating composer, and the
    /// status bar alike, so the picture is continuous behind all three.
    ///
    /// It lives here rather than inside the empty state because the empty
    /// state is only the `flex_1` slot above the composer: a backdrop scoped
    /// to it ended at the composer's top edge and left the bottom of the
    /// window on a flat slab, at every fade setting including "None". The
    /// chat page deliberately has none.
    pub(super) fn new_task_backdrop(
        &mut self,
        theme: Theme,
        phase: f32,
        empty: bool,
    ) -> Option<AnyElement> {
        if !empty {
            return None;
        }
        let backdrop = Self::dither_backdrop(theme);
        let dithered = backdrop.is_some();
        // Only the procedural field animates: a configured photo is a still,
        // so the heartbeat must not repaint this page for it. `render` clears
        // the flag before this runs, so a frame that skips the backdrop leaves
        // it false.
        self.backdrop_live = !dithered;
        Some(
            div()
                .id("new-task-backdrop")
                .debug_selector(|| "new-task-backdrop".to_string())
                .absolute()
                .top(px(TOP_BAR_H))
                .left_0()
                .right_0()
                .bottom_0()
                .when(!dithered, |layer| {
                    layer.child(crate::backdrop::layer(theme, phase))
                })
                .children(backdrop)
                .into_any_element(),
        )
    }

    /// The configured dithered background image, absolutely filling its
    /// parent — the new-task column, via [`Self::new_task_backdrop`].
    pub(super) fn dither_backdrop(theme: Theme) -> Option<AnyElement> {
        Self::backdrop_image(crate::dither::background(), crate::dither::tuning(), theme)
    }

    /// Wrap a dithered image so it fills its parent and nothing else, then
    /// drop it into the page: a bottom gradient to `background` so the picture
    /// fades out under the composer instead of ending on a hard edge. The
    /// fade height comes from the backdrop's own tuning (Settings →
    /// Appearance); `None` skips the gradient entirely rather than painting
    /// a zero-height one.
    ///
    /// The wrapper clips: gpui's `ObjectFit::Cover` scales the image up and
    /// centers it, returning bounds *larger* than the element whenever the
    /// ratios differ — a 16:9 image in a narrower main area painted its
    /// overflow over the sessions sidebar until this clip was added.
    pub(super) fn backdrop_image(
        image: Option<std::sync::Arc<gpui::RenderImage>>,
        tuning: crate::dither::Tuning,
        theme: Theme,
    ) -> Option<AnyElement> {
        image.map(|image| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .overflow_hidden()
                .child(img(image).size_full().object_fit(ObjectFit::Cover))
                .when(tuning.fade > 0., |backdrop| {
                    backdrop.child(
                        div()
                            .id("backdrop-fade")
                            .debug_selector(|| "backdrop-fade".to_string())
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            // Proportional, so the drop stays put as the window
                            // resizes instead of turning into a band.
                            .h(relative(tuning.fade))
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(theme.background.opacity(0.), 0.),
                                linear_color_stop(theme.background.opacity(tuning.fade_peak()), 1.),
                            )),
                    )
                })
                .into_any_element()
        })
    }

    /// The dithered field at rest, for surfaces that show it as a thumbnail
    /// (Settings → Appearance's backdrop strip) rather than the live page.
    pub(super) fn dot_backdrop(theme: Theme) -> impl IntoElement + use<> {
        crate::backdrop::layer(theme, 0.)
    }

    /// New-task empty state — minimal onboarding over the backdrop the caller
    /// paints (the configured dithered image, or the dot grid): one headline,
    /// a ghost workspace row, and the composer below for input.
    ///
    /// `main_width` comes from the caller (window minus sidebar/pane) because
    /// the field below gets a definite width: `w_full().max_w(_)` chains
    /// nested under the centered column resolve their percentages against the
    /// unclamped page width in this gpui/taffy stack, which painted the card
    /// to the window's right edge. `width_for_window` keeps the field and the
    /// picker popover the same width from one source of truth.
    pub(super) fn render_empty_state(
        &self,
        main_width: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        let field_w = px(crate::workspace_picker::width_for_window(main_width.into()));
        let cwd = self
            .current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let folder_name = sessions::workspace_label(&cwd);
        let path_label = cwd.to_string_lossy().into_owned();
        let picker_open = self.workspace_picker.is_some();
        // A conventional project logo takes the folder glyph's seat; no logo
        // found → folder, as before.
        let workspace_icon: AnyElement = match &self.workspace_logo {
            Some(logo) => img(ImageSource::Image(logo.clone()))
                .size(IconSize::Custom(18. / 16.).px(&theme))
                .flex_none()
                .rounded(Radius::Small.px(&theme))
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => icon(
                "icons/folder.svg",
                IconSize::Small.px(&theme),
                theme.text_secondary,
            )
            .into_any_element(),
        };

        // The backdrop belongs to the caller (`new_task_backdrop`, painted by
        // the chat column in `render`): one layer spans this state, the
        // floating composer, and the status bar. The chat page has none.
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .overflow_hidden()
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(px(crate::workspace_picker::PAGE_PAD))
                    .pb(px(88.))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(crate::workspace_picker::FIELD_MAX_W))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(DynamicSpacing::Base20.px(&theme))
                            // Mark over the dot grid: a quiet raised disc,
                            // not an accent wash. The accent is reserved for the
                            // open picker and the caret, not decoration.
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(56.))
                                    .rounded_full()
                                    .bg(theme.surface_elevated)
                                    .border_1()
                                    .border_color(theme.border)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon(
                                        "icons/start-up.svg",
                                        IconSize::Custom(24. / 16.).px(&theme),
                                        theme.text_secondary,
                                    )),
                            )
                            // Title block — one idea, one line of guidance.
                            // 20px is the scale's display step (DESIGN.md).
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(DynamicSpacing::Base06.px(&theme))
                                    .child(
                                        div()
                                            .text_size(theme.ui_px(20.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(tr!("view.start_a_task")),
                                    )
                                    .child(
                                        div()
                                            .text_size(TextSize::Default.px(&theme))
                                            .text_color(theme.text_muted)
                                            .text_align(TextAlign::Center)
                                            .child(tr!("workspace.pick_workspace_hint")),
                                    ),
                            )
                            // Workspace — a labeled select field, not a ghost
                            // row. Click opens the workspace picker (recent
                            // folders, filter, browse) anchored below; the
                            // border takes the accent while it's open.
                            //
                            // Definite `w` (not `w_full().max_w()`): percent
                            // widths nested under the centered `max_w` column
                            // resolve against the unclamped page width here,
                            // and the card painted to the window's edge.
                            .child(
                                div()
                                    .w(field_w)
                                    .flex()
                                    .flex_col()
                                    .gap(DynamicSpacing::Base06.px(&theme))
                                    .child(
                                        div()
                                            .px(px(2.))
                                            .text_size(TextSize::XSmall.px(&theme))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_muted)
                                            .child(tr!("view.workspace")),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .w_full()
                                            .child(
                                                div()
                                                    .id("pick-folder")
                                                    .w_full()
                                                    .pl(DynamicSpacing::Base08.px(&theme))
                                                    .pr(px(10.))
                                                    .py(px(7.))
                                                    .rounded(Radius::Large.px(&theme))
                                                    .border_1()
                                                    .border_color(if picker_open {
                                                        theme.accent.opacity(0.55)
                                                    } else {
                                                        theme.border
                                                    })
                                                    .bg(theme.surface_elevated)
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(10.))
                                                    .cursor_pointer()
                                                    .hover(|s| {
                                                        s.border_color(theme.border_strong)
                                                            .bg(theme.overlay)
                                                    })
                                                    .on_mouse_up(
                                                        MouseButton::Left,
                                                        cx.listener(|app, _, window, cx| {
                                                            app.toggle_workspace_picker(window, cx);
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .size(px(28.))
                                                            .flex_none()
                                                            .rounded(Radius::Large.px(&theme))
                                                            .bg(theme.overlay)
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .child(workspace_icon),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(1.))
                                                            .child(
                                                                div()
                                                                    .text_size(
                                                                        TextSize::Default
                                                                            .px(&theme),
                                                                    )
                                                                    .font_weight(FontWeight::MEDIUM)
                                                                    .text_color(theme.text_primary)
                                                                    .truncate()
                                                                    .child(folder_name),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_size(
                                                                        TextSize::Small.px(&theme),
                                                                    )
                                                                    .text_color(theme.text_muted)
                                                                    .truncate()
                                                                    .child(path_label),
                                                            ),
                                                    )
                                                    // Open state: the affordance
                                                    // answers in accent, matching
                                                    // the border.
                                                    .child(icon(
                                                        "icons/chevron-down.svg",
                                                        IconSize::XSmall.px(&theme),
                                                        if picker_open {
                                                            theme.accent_text
                                                        } else {
                                                            theme.text_muted
                                                        },
                                                    )),
                                            )
                                            .children(self.workspace_picker_popup()),
                                    ),
                            ),
                    ),
            )
    }

    /// Whether every required runtime dependency is installed.
    pub(super) fn dependencies_ready(&self) -> bool {
        onboarding::all_required_installed(&self.deps)
    }

    /// Leave the setup page. Only reachable on request: the page also shows
    /// when something is missing, and then there is nothing to go back to.
    pub(super) fn close_setup(&mut self, cx: &mut Context<Self>) {
        self.setup_open = false;
        cx.notify();
    }

    /// Re-run the dependency probe and, if `pi` just became available, spawn
    /// the agent client. Runs the probe off the main thread and spins the
    /// setup page's Refresh button while it's in flight.
    pub(super) fn refresh_setup(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return; // ignore double-clicks while a refresh is running
        }
        self.refreshing = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let (deps, host, pi_install) = cx
                .background_executor()
                .spawn(async {
                    // Short pause so the spinner reads as "working" rather than
                    // a flash before the probe returns.
                    std::thread::sleep(Duration::from_millis(250));
                    let deps = onboarding::check_dependencies();
                    let mut installation =
                        crate::pi_install::detect_from_bin(onboarding::locate("pi"));
                    installation.version = deps
                        .iter()
                        .find(|dep| dep.bin == "pi")
                        .and_then(|dep| dep.version.clone());
                    (deps, platform::host(), installation)
                })
                .await;
            let ready = onboarding::all_required_installed(&deps);
            let _ = this.update(cx, |app, cx| {
                app.deps = deps;
                app.host = host;
                app.pi_install = pi_install;
                app.refreshing = false;
                app.sync_terminal_active(cx);
                if ready && app.client.is_none() {
                    let workspace = app
                        .current_workspace
                        .clone()
                        .or_else(|| std::env::current_dir().ok())
                        .unwrap_or_else(|| PathBuf::from("."));
                    match app
                        .extensions
                        .spawn(&workspace, true, &app.mcp.secret_env())
                    {
                        Ok(client) => {
                            app.client = Some(client);
                            app.mcp_stamp = app.mcp.fingerprint_for(Some(&workspace));
                            app.send(CommandBody::GetState, "get_state");
                            app.refresh_catalogs();
                            app.toast_success(tr!("view.connected"));
                        }
                        Err(err) => app.toast_error(tr!("runtime.pi_spawn_failed", error = err)),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Full-page setup screen shown when a required dependency is missing.
    pub(super) fn render_onboarding(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        let missing = onboarding::missing_required_count(&self.deps);
        // Opened from Settings → About on a provisioned machine, the page is
        // a report rather than a checklist, so it says so.
        let asked_for = missing == 0;

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .overflow_hidden()
            .child(Self::dot_backdrop(theme))
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(DynamicSpacing::Base24.px(&theme))
                    .pb(DynamicSpacing::Base40.px(&theme))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(560.))
                            .flex()
                            .flex_col()
                            .gap(px(18.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .size(px(44.))
                                            .rounded_full()
                                            .bg(theme.accent.opacity(0.12))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(icon(
                                                "icons/spark.svg",
                                                IconSize::Custom(18. / 16.).px(&theme),
                                                theme.accent_text,
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(theme.ui_px(22.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(tr!("view.set_up_orbit")),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(420.))
                                            .text_size(TextSize::Default.px(&theme))
                                            .text_color(theme.text_muted)
                                            .text_align(TextAlign::Center)
                                            .child(if asked_for {
                                                tr!("view.setup_complete_hint")
                                            } else {
                                                tr!("view.setup_missing_hint")
                                            }),
                                    ),
                            )
                            .child(div().flex().flex_col().gap(px(10.)).children(
                                self.deps.iter().enumerate().map(|(ix, dep)| {
                                    self.render_dependency_row(dep, ix, cx).into_any_element()
                                }),
                            ))
                            .child(self.render_host_facts(cx))
                            // An npm-managed pi cannot update in place; the
                            // official installer migrates it. The click is the
                            // confirmation before it removes the npm package.
                            .when(self.pi_install.kind.is_npm(), |col| {
                                col.child(self.render_pi_migration(cx))
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(14.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(DynamicSpacing::Base06.px(&theme))
                                            .text_size(TextSize::Small.px(&theme))
                                            .text_color(theme.text_muted)
                                            .child(
                                                // Green once nothing is
                                                // missing: the page doubles
                                                // as a health report.
                                                div().size(px(8.)).rounded_full().bg(
                                                    if missing > 0 {
                                                        theme.error
                                                    } else {
                                                        theme.success
                                                    },
                                                ),
                                            )
                                            .child(if missing > 0 {
                                                tr!("setup.missing_count", count = missing)
                                            } else {
                                                tr!("setup.ready")
                                            }),
                                    )
                                    .child(
                                        // Medium, like the Done button beside it.
                                        button_frame(
                                            div().id("refresh-setup"),
                                            &theme,
                                            ButtonSize::Medium,
                                        )
                                        .group(BUTTON_GROUP)
                                        .when(self.refreshing, |b| b.opacity(0.55))
                                        .when(!self.refreshing, |b| {
                                            b.cursor_pointer()
                                                .hover(|s| s.bg(theme.surface_hover))
                                                .active(|s| s.bg(theme.surface_active))
                                        })
                                        .on_click({
                                            let this = cx.entity();
                                            move |_, _window, cx| {
                                                this.update(cx, |app, cx| {
                                                    app.refresh_setup(cx);
                                                });
                                            }
                                        })
                                        .child(refresh_glyph(
                                            "refresh-spin",
                                            ButtonSize::Medium.icon_size().px(&theme),
                                            self.refreshing,
                                            theme.text_secondary,
                                            theme,
                                        ))
                                        .child(
                                            div().text_color(theme.text_secondary).child(
                                                if self.refreshing {
                                                    tr!("common.checking")
                                                } else {
                                                    tr!("common.refresh")
                                                },
                                            ),
                                        ),
                                    )
                                    // Only offered when the page was opened on
                                    // request — with something missing, the
                                    // setup page is the app.
                                    .when(asked_for, |footer| {
                                        footer.child(self.runtime_button(
                                            "setup-done",
                                            &tr!("common.done"),
                                            true,
                                            theme,
                                            cx.entity(),
                                            OrbitApp::close_setup,
                                        ))
                                    }),
                            ),
                    ),
            )
    }

    /// The machine and the paths Orbit reads and writes, under the
    /// dependency list: the OS it is running on, pi's session store, and
    /// Orbit's own config directory.
    ///
    /// Nothing here is installable, so these are quiet rows rather than
    /// dependency cards — but they are what the install commands above have
    /// to work against, and the first thing to check when the app comes up
    /// with an empty sidebar.
    pub(super) fn render_host_facts(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        let mut facts = onboarding::host_facts(&self.host);
        // The pi install rows follow the OS and store paths: how pi arrived,
        // where it resolves, and its version.
        facts.extend(onboarding::pi_install_facts(&self.pi_install));
        div()
            .w_full()
            // A panel that hugs its rows: the setup page centres its column, so
            // a shrinkable child here would be clipped instead of grown.
            .flex_none()
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .children(facts.iter().enumerate().map(|(ix, fact)| {
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base12.px(&theme))
                    .px(px(14.))
                    .py(DynamicSpacing::Base08.px(&theme))
                    // Rows are separated by a hairline, so the group reads as
                    // one panel rather than three.
                    .when(ix > 0, |row| row.border_t_1().border_color(theme.border))
                    .child(
                        div()
                            .w(px(88.))
                            .flex_none()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(fact.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(fact.value.clone()),
                    )
                    .children(fact.note.clone().map(|note| {
                        div()
                            .flex_none()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(if fact.alert {
                                theme.error
                            } else {
                                theme.text_muted
                            })
                            .child(note)
                    }))
            }))
    }

    /// The npm → official-install migration panel. Shown only when pi was
    /// installed with npm; the button is the explicit confirmation before the
    /// official installer removes the npm package. User data is untouched.
    pub(super) fn render_pi_migration(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        let migrating = self.pi_migrating;
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .px(px(14.))
            .py(DynamicSpacing::Base12.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(TextSize::Default.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("pi_update.migrate_title")),
            )
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!("pi_update.migrate_note")),
            )
            .child(
                button_frame(div().id("pi-migrate"), &theme, ButtonSize::Medium)
                    .group(BUTTON_GROUP)
                    .when(migrating, |b| b.opacity(0.55))
                    .when(!migrating, |b| {
                        b.cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .active(|s| s.bg(theme.surface_active))
                    })
                    .on_click({
                        let this = cx.entity();
                        move |_, _window, cx| {
                            this.update(cx, |app, cx| app.migrate_pi_install(cx));
                        }
                    })
                    .child(div().text_color(theme.text_secondary).child(if migrating {
                        tr!("pi_update.migrating")
                    } else {
                        tr!("pi_update.migrate")
                    })),
            )
    }

    /// One dependency row: status dot, name + detail, and either a version
    /// chip (installed) or the install command with a copy affordance.
    pub(super) fn render_dependency_row(
        &self,
        dep: &Dependency,
        ix: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        let status_color = if dep.installed {
            theme.success
        } else {
            theme.error
        };

        div()
            .w_full()
            .px(px(14.))
            .py(DynamicSpacing::Base12.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(div().size(px(9.)).rounded_full().bg(status_color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base03.px(&theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base08.px(&theme))
                            .child(
                                div()
                                    .text_size(TextSize::Default.px(&theme))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .child(dep.name),
                            )
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_muted)
                                    .child(if dep.required {
                                        tr!("setup.required")
                                    } else {
                                        tr!("setup.optional")
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!(dep.detail_key)),
                    ),
            )
            .child(if dep.installed {
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.success)
                    .child(icon(
                        "icons/check.svg",
                        IconSize::XSmall.px(&theme),
                        theme.success,
                    ))
                    .child(
                        dep.version
                            .clone()
                            .unwrap_or_else(|| tr!("setup.installed")),
                    )
                    .into_any_element()
            } else {
                let cmd = dep.install_hint;
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(
                        div()
                            .px(DynamicSpacing::Base08.px(&theme))
                            .py(DynamicSpacing::Base04.px(&theme))
                            .rounded(Radius::Small.px(&theme))
                            .bg(theme.code_bg)
                            .border_1()
                            .border_color(theme.border)
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.code_text)
                            .child(cmd),
                    )
                    .child(
                        icon_button_frame(div().id(("copy", ix)), &theme, ButtonSize::Default)
                            .tip(tr!("common.copy"))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .on_click(move |_, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(cmd.to_string()));
                            })
                            .child(icon(
                                "icons/copy.svg",
                                ButtonSize::Default.icon_size().px(&theme),
                                theme.text_secondary,
                            )),
                    )
                    .into_any_element()
            })
    }

    /// Status bar under the composer: workspace / branch on the left,
    /// sending hints followed by used-context percent + ring on the right.
    pub(super) fn status_bar(
        &self,
        workspace_label: &str,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        div()
            .pt_1p5()
            .w_full()
            .flex()
            .items_center()
            .gap_4()
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(
                button_frame(div().id("status-workspace"), &theme, ButtonSize::Default)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover).text_color(theme.text_secondary))
                    .active(|s| s.bg(theme.surface_active).text_color(theme.text_primary))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_pick_folder_click))
                    .child(icon(
                        "icons/folder.svg",
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_muted,
                    ))
                    .child(workspace_label.to_string()),
            )
            // "Work in": Local / linked worktree / New worktree. Hidden when
            // the active workspace is not a Git repository.
            .children(self.work_in_chip(cx))
            .children(self.branch.as_ref().map(|branch| {
                let open = self.branch_picker.is_some();
                let pending = self.branch_operation_pending;
                let branch_button = div().id("status-branch");
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .relative()
                            .child(
                                button_frame(branch_button, &theme, ButtonSize::Default)
                                    .when(!pending, |chip| chip.cursor_pointer())
                                    .when(open, |chip| {
                                        chip.bg(theme.surface_active).text_color(theme.text_primary)
                                    })
                                    .when(!open && !pending, |chip| {
                                        chip.hover(|s| {
                                            s.bg(theme.surface_hover)
                                                .text_color(theme.text_secondary)
                                        })
                                    })
                                    .when(pending, |chip| chip.opacity(0.6))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|app, _, window, cx| {
                                            if !app.branch_operation_pending {
                                                app.toggle_branch_picker(window, cx);
                                            }
                                        }),
                                    )
                                    .child(icon(
                                        "icons/branch.svg",
                                        ButtonSize::Default.icon_size().px(&theme),
                                        theme.text_muted,
                                    ))
                                    .child(branch.name.clone())
                                    .children(
                                        branch
                                            .ahead_behind
                                            // `↑0 ↓0` is a row of arrows that
                                            // says nothing; the counts appear
                                            // when there is something to push
                                            // or pull.
                                            .filter(|(ahead, behind)| *ahead > 0 || *behind > 0)
                                            .map(|(ahead, behind)| {
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(DynamicSpacing::Base06.px(&theme))
                                                    .child(format!("↑{ahead}"))
                                                    .child(format!("↓{behind}"))
                                            }),
                                    ),
                            )
                            .children(self.branch_picker_popup()),
                    )
                    .into_any_element()
            }))
            // A quiet MCP indicator: only when servers are configured, and
            // only a dot + count — clicking opens Settings → MCP. Color is
            // never the only signal (the label names the state).
            .children(self.mcp_status_chip(theme, cx))
            .child(div().flex_1())
            // Transient status (send failures, attachment limits, branch
            // results): fresh messages only — the tick lets them lapse.
            .children(
                self.status_at
                    .is_some_and(|at| at.elapsed() < STATUS_MESSAGE_TTL)
                    .then(|| {
                        div()
                            .max_w(px(320.))
                            .truncate()
                            .text_color(theme.text_muted)
                            .child(self.status.clone())
                    }),
            )
            // While a run is in flight the hints carry the one thing that is
            // not obvious from the composer: which chord steers the live turn
            // and which queues a follow-up. Idle they only restate the send
            // button a few pixels above, so they stay out of the chrome until
            // there is something to teach.
            .children(self.is_running().then(|| self.composer_send_hints(cx)))
            .child(
                div()
                    .debug_selector(|| "composer-context-indicator".to_string())
                    .flex_none()
                    .child(self.context_button(cx)),
            )
    }

    pub(super) fn context_button(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let entity = cx.entity();
        let theme = *theme::get(cx);
        // Manual compaction is offered only while pi is connected: without a
        // session process there is nothing to compact.
        let on_compact = self.client.is_some().then(|| {
            Rc::new(
                |app: &mut OrbitApp, _window: &mut Window, cx: &mut Context<OrbitApp>| {
                    app.compact_now(cx);
                },
            ) as Rc<dyn Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>)>
        });
        context_meter::context_control(
            ContextMeterData {
                usage: self.context.as_ref(),
                session: self.session_usage.as_ref(),
                conversation_est: self.transcript.estimated_tokens(),
            },
            self.context_popup,
            &entity,
            theme,
            |app, hovered, cx| {
                if app.context_popup == ContextPopup::Details {
                    return;
                }
                app.context_popup = if hovered {
                    ContextPopup::Hover
                } else {
                    ContextPopup::None
                };
                cx.notify();
            },
            |app, _, cx| {
                const GESTURE: Duration = Duration::from_millis(200);
                if let Some(dismissed) = app.menu_dismissed_at.take() {
                    if dismissed.elapsed() < GESTURE {
                        return;
                    }
                }
                app.context_popup = if app.context_popup == ContextPopup::Details {
                    ContextPopup::None
                } else {
                    ContextPopup::Details
                };
                if app.context_popup == ContextPopup::Details {
                    app.refresh_context_stats();
                }
                cx.notify();
            },
            self.is_compacting,
            on_compact,
            |app, _, cx| {
                app.menu_dismissed_at = Some(Instant::now());
                app.context_popup = ContextPopup::None;
                cx.notify();
            },
        )
    }

    /// The window's left titlebar controls: sidebar toggle + session history.
    /// They sit beside the macOS traffic lights in the sidebar's drag strip
    /// when the sessions sidebar is open, and fall back to the main top bar
    /// (clearing the lights) when it is hidden. Windows has no OS buttons on
    /// that side, so the cluster stays left-aligned there either way.
    pub(super) fn titlebar_left_controls(
        &self,
        theme: Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let back_enabled = self.history_index > 0;
        let forward_enabled = self.history_index + 1 < self.session_history.len();
        // The three controls wear the same glass chips as the right cluster
        // ([`header_icon_button`]), so the whole 44px bar reads as one row:
        // a fixed square hit box with a centered Medium icon keeps the toggle
        // and the chevrons optically even (a bare `p_1` lets the toggle's
        // pill drift wider than the chevrons).
        div()
            .flex()
            .items_center()
            .gap(px(TITLEBAR_CONTROLS_GAP))
            .pr(DynamicSpacing::Base06.px(&theme))
            .child(
                header_ghost_button(
                    "toggle-sidebar",
                    &theme,
                    icon(
                        "icons/layout-left.svg",
                        ButtonSize::Medium.icon_size().px(&theme),
                        theme.text_secondary,
                    ),
                )
                .block_mouse_except_scroll()
                .tooltip({
                    let label = commands::tooltip(CommandId::ToggleSidebar, self.sidebar_visible);
                    move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                })
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_toggle_sidebar)),
            )
            .child(
                div()
                    .id("history-back")
                    .block_mouse_except_scroll()
                    .size(px(HEADER_CTRL_H))
                    .rounded(px(HEADER_CTRL_R))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(back_enabled, |b| {
                        b.cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_history_back))
                    })
                    .tooltip({
                        let label = tr!("view.back");
                        move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                    })
                    .child(icon(
                        "icons/arrow-left.svg",
                        ButtonSize::Medium.icon_size().px(&theme),
                        if back_enabled {
                            theme.text_secondary
                        } else {
                            theme.text_muted
                        },
                    )),
            )
            .child(
                div()
                    .id("history-forward")
                    .block_mouse_except_scroll()
                    .size(px(HEADER_CTRL_H))
                    .rounded(px(HEADER_CTRL_R))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(forward_enabled, |b| {
                        b.cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_history_forward))
                    })
                    .tooltip({
                        let label = tr!("view.forward");
                        move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                    })
                    .child(icon(
                        "icons/arrow-right.svg",
                        ButtonSize::Medium.icon_size().px(&theme),
                        if forward_enabled {
                            theme.text_secondary
                        } else {
                            theme.text_muted
                        },
                    )),
            )
    }

    /// The right dock's single top-bar affordance: the side-panel toggle and
    /// the working tree's `+N −N` change counts, folded into one chip. The
    /// counts already opened Review on the uncommitted diff, so a separate
    /// panel toggle beside them was the same job twice. Dirty — and with the
    /// pane and the Git page closed — the chip wears the counts instead of the
    /// glyph; clean, it falls back to the `panel-right` icon. Either way it
    /// flips the pane and stays lifted while the pane is visible.
    pub(super) fn render_review_panel_control(
        &self,
        theme: Theme,
        pane_visible: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        // The counts stay off while the pane or the Git page already shows the
        // same diff, matching the standalone chip's old visibility rule.
        let dirty = (self.added > 0 || self.removed > 0) && !pane_visible && !self.git_open;
        let chip = press(header_chip(
            div()
                .id("toggle-side-pane")
                .group(BUTTON_GROUP)
                .h(px(HEADER_CTRL_H))
                .min_w(px(HEADER_CTRL_H))
                .px(if dirty {
                    ButtonSize::Medium.padding_x(&theme)
                } else {
                    px(0.)
                })
                .rounded(px(HEADER_CTRL_R))
                .flex()
                .items_center()
                .justify_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .cursor_pointer(),
            &theme,
        ))
        // One or the other, never both: the counts stand in for the glyph
        // while the tree is dirty, so the chip cannot grow two things wide.
        .children((!dirty).then(|| {
            icon(
                "icons/panel-right.svg",
                ButtonSize::Medium.icon_size().px(&theme),
                if pane_visible {
                    theme.text_primary
                } else {
                    theme.text_secondary
                },
            )
        }))
        .children(dirty.then(|| {
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.success)
                        .child(format!("+{}", self.added)),
                )
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.error)
                        .child(format!("-{}", self.removed)),
                )
        }));
        let chip = if pane_visible {
            header_lift(chip, &theme)
        } else {
            chip
        };
        chip.tip(commands::tooltip(CommandId::ToggleSidePanel, pane_visible))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_toggle_side_pane))
    }

    /// The top bar's overflow menu: one "…" control holding the surfaces that
    /// no longer earn a permanent chip — Session details, Explorer, the
    /// Terminal, the Git page, and Worktrees. It keeps the bar to the controls
    /// that carry live state (quota, open-in, Review, this), with the rest one
    /// click away and still on their shortcuts. Session details' popover is
    /// mounted here so it keeps its top-right anchor now that its info chip is
    /// gone.
    pub(super) fn render_header_more(
        &self,
        theme: Theme,
        explorer_visible: bool,
        terminal_visible: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.header_more_open;
        let lifted = open || self.session_details_open;
        div()
            .relative()
            .children(self.render_session_details_popup(cx))
            .children(self.render_header_more_popup(theme, explorer_visible, terminal_visible, cx))
            .child(
                header_icon_button(
                    "header-more",
                    &theme,
                    lifted,
                    icon(
                        "icons/more.svg",
                        ButtonSize::Medium.icon_size().px(&theme),
                        if lifted {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        },
                    ),
                )
                .tip(tr!("sidebar.more_actions"))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_header_more_click)),
            )
    }

    /// The overflow menu itself, anchored under the "…" chip. Rows mirror the
    /// command registry's titles, so the menu, the palette, and the shortcut
    /// help can never drift apart.
    pub(super) fn render_header_more_popup(
        &self,
        theme: Theme,
        explorer_visible: bool,
        terminal_visible: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.header_more_open {
            return None;
        }
        let menu = picker_surface(div().id("header-more-popup"), &theme)
            .w(px(220.))
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base01.px(&theme))
            .occlude()
            .on_mouse_down_out(
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.dismiss_header_more(cx)),
            )
            .child(header_menu_row(
                "header-more-session",
                "icons/info.svg",
                tr!("session.session_details"),
                None,
                self.session_details_open,
                theme,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.open_session_details(cx)),
            ))
            .child(header_menu_row(
                "header-more-explorer",
                "icons/folder.svg",
                commands::title(CommandId::ToggleProjectPanel, explorer_visible),
                commands::spec(CommandId::ToggleProjectPanel).shortcut(),
                explorer_visible,
                theme,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.header_more_open = false;
                    this.toggle_project_panel(cx);
                }),
            ))
            .child(header_menu_row(
                "header-more-terminal",
                "icons/terminal.svg",
                commands::title(CommandId::ToggleTerminal, terminal_visible),
                commands::spec(CommandId::ToggleTerminal).shortcut(),
                terminal_visible,
                theme,
                cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    this.header_more_open = false;
                    this.on_toggle_terminal(&crate::ToggleTerminal, window, cx);
                }),
            ))
            .child(header_menu_row(
                "header-more-git",
                "icons/github.svg",
                commands::title(CommandId::OpenGit, false),
                commands::spec(CommandId::OpenGit).shortcut(),
                self.git_open,
                theme,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.header_more_open = false;
                    if this.git_open {
                        this.close_git(cx);
                    } else {
                        this.open_git(cx);
                    }
                }),
            ))
            // Worktrees is a surface, not live state, so it belongs here rather
            // than in the status row — which is why the "Work in" chip can stay
            // out of the way at the main checkout. The row keeps the registry's
            // icon and title, so the menu and the palette cannot drift.
            .child(header_menu_row(
                "header-more-worktrees",
                "icons/branch.svg",
                commands::title(CommandId::OpenWorktrees, false),
                commands::spec(CommandId::OpenWorktrees).shortcut(),
                self.worktrees_open,
                theme,
                cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    this.header_more_open = false;
                    this.on_open_worktrees(&crate::OpenWorktrees, window, cx);
                }),
            ));
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
                        .child(deferred(menu)),
                )
                .into_any_element(),
        )
    }

    /// The sidebar's primary action: a raised New Task button in the same
    /// `ButtonSize::Large` frame as the Search and Usage rows, marked by its
    /// accent icon and the ⌘N shortcut hint.
    pub(super) fn sidebar_new_task_button(
        &self,
        theme: Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        button_frame(div().id("sidebar-new-session"), &theme, ButtonSize::Large)
            .group(BUTTON_GROUP)
            .w_full()
            .tip(commands::tooltip(CommandId::NewSession, false))
            .raised(theme.surface_elevated, &theme)
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|s| {
                s.raised(theme.surface_hover, &theme)
                    .border_color(theme.border_strong)
            })
            .active(|s| s.opacity(PRESS_DIM))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, w, cx| {
                    this.on_new_session(&crate::NewSession, w, cx)
                }),
            )
            .child(icon(
                "icons/task-add-01.svg",
                ButtonSize::Large.icon_size().px(&theme),
                theme.accent_text,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("view.new_task")),
            )
            .child(
                div()
                    .flex_none()
                    .ml(button::keybinding_gap(&theme))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(crate::platform::shortcuts::label(
                        crate::platform::shortcuts::NEW_SESSION,
                    )),
            )
    }

    /// The sidebar footer's utility icons: Search (command palette), Usage,
    /// and Settings. Icon-only — each glyph is labelled by its tooltip, which
    /// also names its chord — so the column keeps labels for the primary
    /// action and the session list alone.
    pub(super) fn sidebar_footer_actions(
        &self,
        theme: Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        // Search — opens the command palette (⌘P).
        let search = icon_button_frame(div().id("sidebar-search"), &theme, ButtonSize::Medium)
            .tip(commands::tooltip(CommandId::ToggleCommandPalette, false))
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_hover))
            .active(|s| s.opacity(PRESS_DIM))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, w, cx| this.toggle_command_palette(w, cx)),
            )
            .child(icon(
                "icons/search.svg",
                ButtonSize::Medium.icon_size().px(&theme),
                theme.text_muted,
            ));
        // Usage — opens (or leaves) the Usage page (⌘U).
        let usage = icon_button_frame(div().id("sidebar-usage"), &theme, ButtonSize::Medium)
            .tip(commands::tooltip(CommandId::ToggleUsage, false))
            .when(self.usage_open, |btn| btn.bg(theme.surface_active))
            .cursor_pointer()
            .hover(|s| {
                s.bg(if self.usage_open {
                    theme.surface_active
                } else {
                    theme.surface_hover
                })
            })
            .active(|s| s.opacity(PRESS_DIM))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_usage_nav_click))
            .child(icon(
                "icons/chart-analysis.svg",
                ButtonSize::Medium.icon_size().px(&theme),
                if self.usage_open {
                    theme.text_primary
                } else {
                    theme.text_muted
                },
            ));
        // Settings — toggles the Settings surface (⌘,).
        let settings = icon_button_frame(div().id("settings"), &theme, ButtonSize::Medium)
            .tip(commands::tooltip(CommandId::OpenSettings, false))
            .when(self.settings_open, |btn| btn.bg(theme.surface_active))
            .cursor_pointer()
            .hover(|s| {
                s.bg(if self.settings_open {
                    theme.surface_active
                } else {
                    theme.surface_hover
                })
            })
            .active(|s| s.opacity(PRESS_DIM))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_settings_gear_click))
            .child(icon(
                "icons/settings.svg",
                ButtonSize::Medium.icon_size().px(&theme),
                if self.settings_open {
                    theme.text_primary
                } else {
                    theme.text_muted
                },
            ));
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(search)
            .child(usage)
            .child(settings)
    }

    /// ⌘U: open the Usage page, or leave it if it is already open.
    pub(super) fn on_toggle_usage(
        &mut self,
        _: &crate::ToggleUsage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.usage_open {
            self.close_usage(cx);
        } else {
            self.open_usage(cx);
        }
    }

    /// The command palette's Choose Model (⌘⇧M): open the model popover.
    pub(super) fn on_toggle_model_menu(
        &mut self,
        _: &crate::ToggleModelMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_picker(PickerKind::Model, window, cx);
    }

    /// The command palette's Choose Thinking Level (⌘⇧T).
    pub(super) fn on_toggle_thinking_menu(
        &mut self,
        _: &crate::ToggleThinkingMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_picker(PickerKind::Thinking, window, cx);
    }

    /// ⌘⇧R: open the Review pane on the current diff. The pane owns the
    /// reveal/toggle rules; the app only asks for it.
    pub(super) fn on_review_changes(
        &mut self,
        _: &crate::ReviewChanges,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_review(cx);
    }

    /// ⌘/: open Settings → Shortcuts — the full keyboard reference.
    pub(super) fn on_open_shortcut_help(
        &mut self,
        _: &crate::OpenShortcutHelp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = true;
        self.set_settings_section(SettingsSection::Shortcuts, cx);
        self.sync_terminal_active(cx);
    }

    /// Escape from the Review tree: leave keyboard navigation and hand focus
    /// back to the composer, matching the sidebar's Escape behavior.
    pub(super) fn on_review_close(
        &mut self,
        _: &crate::ReviewClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.read(cx).focus(window);
        cx.notify();
    }

    /// Tab: step to the next workbench surface (composer → sidebar → review
    /// tree → terminal → Explorer → back).
    pub(super) fn on_focus_next(
        &mut self,
        _: &crate::FocusNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focus_traversal_blocked() {
            return;
        }
        window.focus_next();
        cx.notify();
    }

    /// Shift-Tab: step to the previous workbench surface.
    pub(super) fn on_focus_prev(
        &mut self,
        _: &crate::FocusPrev,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focus_traversal_blocked() {
            return;
        }
        window.focus_prev();
        cx.notify();
    }

    /// A keyboard-modal surface owns the input while it is open; Tab must not
    /// walk the focus out from under it.
    fn focus_traversal_blocked(&self) -> bool {
        self.command_palette.is_some()
            || self.model_selector.is_some()
            || self.branch_picker.is_some()
            || self.workspace_picker.is_some()
            || self.dialog.is_some()
            || self.lightbox.is_some()
            || self.updater_dialog.is_some()
            || self.session_menu.is_some()
            || self.workspace_menu.is_some()
            || self.sidebar_sort_menu
            || self.settings_select.is_some()
    }

    pub(super) fn send_button(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = *theme::get(cx);
        if self.busy {
            icon_button_frame(div().id("stop-btn"), &theme, ButtonSize::Medium)
                .rounded_full()
                .raised(theme.error, &theme)
                .hover(|s| s.raised(theme.error_hover, &theme))
                .active(|s| s.raised(theme.error, &theme))
                .cursor_pointer()
                .text_color(theme.error_fg)
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_abort_mouse))
                .tooltip({
                    let label = tr!(
                        "composer.shortcut_hint",
                        keys = platform::shortcuts::label(platform::shortcuts::STOP),
                        action = tr!("shortcut.stop")
                    );
                    move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                })
                .child(icon(
                    "icons/stop.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.action_fg,
                ))
        } else {
            // Nothing to send yet: the button stays clickable (submit
            // no-ops on empty) but reads as quiet until there's a message
            // or an attachment.
            let empty = self.input.read(cx).text().trim().is_empty() && self.attachments.is_empty();
            icon_button_frame(div().id("send-btn"), &theme, ButtonSize::Medium)
                .rounded_full()
                .when(empty, |btn| btn.bg(theme.overlay))
                .when(!empty, |btn| {
                    btn.raised(theme.action, &theme)
                        .hover(|s| s.raised(theme.action_hover, &theme))
                        .active(|s| s.raised(theme.action_pressed, &theme))
                        .cursor_pointer()
                })
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_send_click))
                .tooltip({
                    let label = tr!(
                        "composer.shortcut_hint",
                        keys = platform::shortcuts::label(platform::shortcuts::SEND),
                        action = tr!("composer.send")
                    );
                    move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
                })
                .child(icon(
                    "icons/send.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    if empty {
                        theme.text_muted
                    } else {
                        theme.action_fg
                    },
                ))
        }
    }

    /// Persistent run-status strip above the composer: a calm, single-line
    /// account of what pi is doing between turns — waiting out a transient
    /// provider error (with the attempt counter and a real Cancel that sends
    /// `abort_retry`) or compacting the conversation context. Unlike the
    /// status bar's transient messages, this stays up for the whole state.
    pub(super) fn run_status_strip(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = *theme::get(cx);
        if let Some(retry) = &self.retry_detail {
            let attempt = match retry.max {
                Some(max) => tr!("runtime.attempt_of", attempt = retry.attempt, max = max),
                None => tr!("runtime.attempt", attempt = retry.attempt),
            };
            return Some(
                div()
                    .w_full()
                    .mb(DynamicSpacing::Base08.px(&theme))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(Radius::XLarge.px(&theme))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_elevated)
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .size(px(6.))
                            .flex_none()
                            .rounded_full()
                            .bg(theme.warning),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!(
                                "runtime.retrying",
                                attempt = attempt,
                                error = retry.error
                            )),
                    )
                    .child(
                        button_frame(div().id("cancel-retry"), &theme, ButtonSize::Compact)
                            .text_color(theme.text_secondary)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover).text_color(theme.text_primary))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.abort_retry(cx);
                                }),
                            )
                            .child(tr!("view.cancel")),
                    )
                    .into_any_element(),
            );
        }
        if self.is_compacting {
            return Some(
                div()
                    .w_full()
                    .mb(DynamicSpacing::Base08.px(&theme))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(Radius::XLarge.px(&theme))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_elevated)
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .size(px(6.))
                            .flex_none()
                            .rounded_full()
                            .bg(theme.accent),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("view.preparing_conversation_context")),
                    )
                    .into_any_element(),
            );
        }
        None
    }

    /// The inline `ask_user_question` panel: the live question and its options
    /// docked above the composer. It answers the extension's `select` /
    /// `input` requests in place, so a questionnaire never covers the
    /// transcript with a scrim. Every question is buffered locally — Back and
    /// Next step through them so an earlier answer can be changed — and only
    /// the final Submit replays them to the extension. The panel owns the
    /// `AskPanel` key context (↑/↓ move, ⏎ advances, esc declines); its text
    /// field carries `AskInput` so Enter advances rather than submitting the
    /// composer.
    pub(super) fn ask_panel(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let prompt = self.ask.as_ref()?;
        let question = prompt.question()?;
        let theme = *theme::get(cx);
        let submitted = prompt.submitted;
        let cursor = prompt.cursor;
        let total = prompt.total();
        let multi = prompt.is_multi();
        let highlighted = prompt.highlighted();

        let mut card = div()
            .id("ask-panel")
            .w_full()
            .mb(DynamicSpacing::Base08.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.accent.opacity(0.3))
            .bg(theme.surface_elevated)
            .flex()
            .flex_col()
            .overflow_hidden()
            .key_context("AskPanel")
            .track_focus(&self.ask_focus)
            .on_action(cx.listener(Self::on_ask_next))
            .on_action(cx.listener(Self::on_ask_prev))
            .on_action(cx.listener(Self::on_ask_confirm))
            .on_action(cx.listener(Self::on_ask_submit))
            .on_action(cx.listener(Self::on_ask_close));

        // ── header: icon, header chip, progress ──
        let mut header = div()
            .px(px(14.))
            .pt(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme));
        header = header.child(
            div()
                .flex_none()
                .size(px(24.))
                .rounded(Radius::Medium.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.accent.opacity(0.16))
                .child(icon(
                    "icons/task.svg",
                    IconSize::Small.px(&theme),
                    theme.accent_text,
                )),
        );
        if !question.header.trim().is_empty() {
            header = header.child(
                div()
                    .flex_none()
                    .px(px(7.))
                    .py(px(2.))
                    .rounded(Radius::Small.px(&theme))
                    .bg(theme.overlay_strong)
                    .text_size(TextSize::XSmall.px(&theme))
                    .line_height(theme.ui_px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_secondary)
                    .child(SharedString::from(question.header.clone())),
            );
        }
        header = header.child(div().flex_1());
        if total > 1 {
            header = header.child(
                div()
                    .flex_none()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!("ask.question_of", current = cursor + 1, total = total)),
            );
        }
        // A visible dismiss affordance beside the keyboard hint.
        header = header.child(
            icon_button_frame(div().id("ask-close"), &theme, ButtonSize::Compact)
                .tip(tr!("common.close"))
                .cursor_pointer()
                .hover(|style| style.bg(theme.overlay_strong))
                .child(icon(
                    "icons/x.svg",
                    ButtonSize::Compact.icon_size().px(&theme),
                    theme.text_muted,
                ))
                .on_click(cx.listener(|this, _, window, cx| this.ask_cancel(window, cx))),
        );
        card = card.child(header);

        // ── the question ──
        card = card.child(
            div()
                .px(px(14.))
                .pt(DynamicSpacing::Base08.px(&theme))
                .pb(px(10.))
                .whitespace_normal()
                .text_size(TextSize::Default.px(&theme))
                .line_height(theme.ui_px(19.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(SharedString::from(question.question.clone())),
        );

        // ── body ──
        let mut rows = div()
            .px(DynamicSpacing::Base08.px(&theme))
            .pb(DynamicSpacing::Base08.px(&theme))
            .flex()
            .flex_col()
            .gap(px(2.));
        for (ix, option) in question.options.iter().enumerate() {
            let row_highlighted = !submitted && ix == highlighted;
            let checked = multi
                && prompt
                    .checked
                    .get(cursor)
                    .and_then(|row| row.get(ix))
                    .copied()
                    .unwrap_or(false);
            let chosen = row_highlighted || checked;
            // A number reads as a question rather than a menu; multi
            // additionally marks its toggles with a trailing check.
            let marker = div()
                .flex_none()
                .w(px(16.))
                .text_size(TextSize::Small.px(&theme))
                .line_height(theme.ui_px(17.))
                .text_color(if chosen {
                    theme.accent_text
                } else {
                    theme.text_muted
                })
                .child(format!("{}. ", ix + 1));
            let mut copy = div().min_w_0().flex_1().flex().flex_col().gap(px(1.));
            copy = copy.child(
                div()
                    .whitespace_normal()
                    .text_size(TextSize::Small.px(&theme))
                    .line_height(theme.ui_px(17.))
                    .text_color(if chosen {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    })
                    .child(SharedString::from(option.label.clone())),
            );
            // The description only under the focused row keeps the list
            // compact so every choice stays visible at a glance.
            if row_highlighted && !option.description.trim().is_empty() {
                copy = copy.child(
                    div()
                        .whitespace_normal()
                        .text_size(TextSize::Small.px(&theme))
                        .line_height(theme.ui_px(16.))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(option.description.clone())),
                );
            }
            let mut row = div()
                .id(ElementId::NamedInteger("ask-option".into(), ix as u64))
                .w_full()
                .min_w_0()
                .px(px(10.))
                .py(DynamicSpacing::Base06.px(&theme))
                .rounded(Radius::Medium.px(&theme))
                .border_1()
                .border_color(if row_highlighted {
                    theme.border_strong
                } else {
                    gpui::transparent_black()
                })
                .when(row_highlighted, |row| row.bg(theme.selection))
                .when(!submitted, |row| row.cursor_pointer())
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base04.px(&theme))
                .hover(|style| style.bg(theme.overlay_strong))
                .child(marker)
                .child(copy);
            if multi && checked {
                row = row.child(icon(
                    "icons/check.svg",
                    IconSize::XSmall.px(&theme),
                    theme.accent_text,
                ));
            }
            if !submitted {
                row = row.on_click(cx.listener(move |this, _, _, cx| this.ask_choose(ix, cx)));
            }
            rows = rows.child(row);
        }
        // Single-select questions carry a trailing "Type something." row.
        if !multi {
            let trailing = question.options.len();
            let row_highlighted = !submitted && trailing == highlighted;
            let mut row = div()
                .id("ask-trailing")
                .w_full()
                .min_w_0()
                .px(px(10.))
                .py(px(7.))
                .rounded(Radius::Medium.px(&theme))
                .border_1()
                .border_color(if row_highlighted {
                    theme.border_strong
                } else {
                    gpui::transparent_black()
                })
                .when(row_highlighted, |row| row.bg(theme.selection))
                .when(!submitted, |row| row.cursor_pointer())
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .hover(|style| style.bg(theme.overlay_strong))
                .child(icon(
                    "icons/compose.svg",
                    IconSize::XSmall.px(&theme),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .line_height(theme.ui_px(17.))
                        .text_color(if row_highlighted {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        })
                        .child(tr!("view.type_something")),
                );
            if !submitted {
                row =
                    row.on_click(cx.listener(move |this, _, _, cx| this.ask_choose(trailing, cx)));
            }
            rows = rows.child(row);
        }
        card = card.child(rows);

        // The focused option's preview (single-select + preview only).
        // Labeled so authored mockups read as preview content, not a
        // second interface.
        if !multi {
            if let Some(option) = question.options.get(highlighted) {
                if let Some(preview) = option.preview.as_deref() {
                    let preview = if preview.chars().count() > 600 {
                        let mut capped: String = preview.chars().take(600).collect();
                        capped.push('…');
                        capped
                    } else {
                        preview.to_string()
                    };
                    card = card.child(
                        div()
                            .mx(DynamicSpacing::Base12.px(&theme))
                            .mb(px(10.))
                            .rounded(Radius::Large.px(&theme))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px(px(10.))
                                    .py(px(5.))
                                    .flex()
                                    .items_center()
                                    .gap(DynamicSpacing::Base06.px(&theme))
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .child(icon(
                                        "icons/eye.svg",
                                        IconSize::XSmall.px(&theme),
                                        theme.text_muted,
                                    ))
                                    .child(
                                        div()
                                            .text_size(TextSize::XSmall.px(&theme))
                                            .line_height(theme.ui_px(14.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_muted)
                                            .child(tr!("ask.preview", label = option.label)),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(10.))
                                    .py(DynamicSpacing::Base08.px(&theme))
                                    .font_family(theme::code_font_family())
                                    .text_size(theme.code_px(11.))
                                    .line_height(theme.code_px(16.))
                                    .text_color(theme.text_secondary)
                                    .whitespace_normal()
                                    .child(SharedString::from(preview)),
                            ),
                    );
                }
            }
        }

        // The custom-answer field: always on multi questions, and on a
        // single-select question once its "Type something." row is active.
        if prompt.custom_active() {
            card = card.child(
                input_field_frame(div(), &theme)
                    .mx(DynamicSpacing::Base12.px(&theme))
                    .mb(DynamicSpacing::Base08.px(&theme))
                    .border_color(theme.border_strong)
                    .bg(theme.surface_secondary)
                    .child(prompt.input.clone()),
            );
        }

        // ── footer: hint + Back / Next (Submit on the last question) ──
        let hint = if submitted {
            tr!("ask.sending")
        } else if multi {
            tr!("ask.hint_multi")
        } else {
            tr!("ask.hint_single")
        };
        let mut back = button_frame(div().id("ask-back"), &theme, ButtonSize::Medium)
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(gpui::transparent_black())
            .raised(theme.overlay, &theme)
            .text_color(theme.text_secondary);
        let back_disabled = submitted || cursor == 0;
        if back_disabled {
            back = back.opacity(0.45);
        } else {
            back = back
                .cursor_pointer()
                .hover(|style| {
                    style
                        .raised(theme.overlay_strong, &theme)
                        .text_color(theme.text_primary)
                })
                .on_click(cx.listener(|this, _, window, cx| this.ask_prev_question(window, cx)));
        }
        back = back.child(tr!("view.back"));

        let last = cursor + 1 >= total;
        let next_label = if last {
            tr!("ask.submit")
        } else {
            tr!("ask.next")
        };
        let mut next = button_frame(div().id("ask-next"), &theme, ButtonSize::Medium)
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(gpui::transparent_black())
            .raised(theme.accent.opacity(0.16), &theme)
            .text_color(theme.accent_text);
        if submitted {
            next = next.opacity(0.45);
        } else {
            next = next
                .cursor_pointer()
                .hover(|style| style.raised(theme.accent.opacity(0.26), &theme))
                .on_click(cx.listener(|this, _, window, cx| this.ask_next_question(window, cx)));
        }
        next = next.child(next_label);

        card = card.child(
            div()
                .h(px(46.))
                .px(DynamicSpacing::Base12.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(hint),
                )
                .child(back)
                .child(next),
        );

        Some(card.into_any_element())
    }

    /// The inline access-guard approval: a compact bar directly above the
    /// composer asking whether a mutating tool call may run. Rendered here
    /// rather than as the scrim modal so the transcript stays visible and the
    /// answer sits next to the composer. Each option is a button; the bar also
    /// owns the `Approval` key context (↑/↓ move, ⏎ confirms, esc denies). It
    /// reveals by animating its own height so the page never jumps.
    pub(super) fn approval_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        /// Fixed row height — a single-line prompt, so the reveal can animate
        /// a known height and the surrounding layout can never reflow abruptly.
        const BAR_H: f32 = 48.;
        const GAP: f32 = 8.;

        let request = self.approval.as_ref()?;
        let theme = *theme::get(cx);
        // An MCP tool's raw name is `mcp__server__tool`; the bar names the
        // server and tool instead so a permission decision is readable.
        let heading =
            if let Some((server, tool)) = crate::transcript_view::mcp_tool_parts(&request.tool) {
                tr!("mcp.allow_tool", server = server, tool = tool)
            } else if request.tool.trim().is_empty() {
                tr!("approval.permission_needed")
            } else {
                tr!("approval.allow_tool", tool = request.tool)
            };
        let detail = request.detail.clone();

        let mut buttons = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme));
        for (ix, option) in request.options.iter().enumerate() {
            let highlighted = ix == self.approval_highlight;
            let is_deny = option.eq_ignore_ascii_case("deny");
            let is_primary = !is_deny && !option.to_ascii_lowercase().contains("always");
            let id = ElementId::NamedInteger("approval-option".into(), ix as u64);
            let mut button = button_frame(div().id(id), &theme, ButtonSize::Medium)
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .border_1()
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| this.approval_choose(ix, window, cx)),
                );
            // The accent is a text/icon tone, never a full fill here: the
            // affirmative is an accent-washed tile with accent ink, matching the
            // One Accent Rule.
            button = if is_primary {
                button
                    .raised(theme.accent.opacity(0.16), &theme)
                    .text_color(theme.accent_text)
                    .hover(|s| s.raised(theme.accent.opacity(0.26), &theme))
            } else if is_deny {
                button
                    .raised(theme.overlay, &theme)
                    .text_color(theme.text_secondary)
                    .hover(|s| {
                        s.raised(theme.error.opacity(0.14), &theme)
                            .text_color(theme.error)
                    })
            } else {
                button
                    .raised(theme.overlay, &theme)
                    .text_color(theme.text_secondary)
                    .hover(|s| {
                        s.raised(theme.overlay_strong, &theme)
                            .text_color(theme.text_primary)
                    })
            };
            // The keyboard cursor reads as a strong border.
            button = button.border_color(if highlighted {
                theme.border_strong
            } else {
                gpui::transparent_black()
            });
            buttons = buttons.child(button.child(option.clone()));
        }

        let bar = div()
            .id("approval-bar")
            .w_full()
            .h(px(BAR_H))
            .mb(px(GAP))
            .px(DynamicSpacing::Base12.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.accent.opacity(0.3))
            .bg(theme.surface_elevated)
            .flex()
            .items_center()
            .gap(px(10.))
            .overflow_hidden()
            .key_context("Approval")
            .track_focus(&self.approval_focus)
            .on_action(cx.listener(Self::on_approval_next))
            .on_action(cx.listener(Self::on_approval_prev))
            .on_action(cx.listener(Self::on_approval_confirm))
            .on_action(cx.listener(Self::on_approval_close))
            .child(
                div()
                    .flex_none()
                    .size(px(28.))
                    .rounded(Radius::Large.px(&theme))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.accent.opacity(0.16))
                    .child(icon(
                        "icons/lock.svg",
                        IconSize::Small.px(&theme),
                        theme.accent_text,
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
                            .text_size(TextSize::Small.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(heading),
                    )
                    .when(!detail.trim().is_empty(), |col| {
                        col.child(
                            div()
                                .mt(px(1.))
                                .font_family(theme::code_font_family())
                                .truncate()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .child(detail),
                        )
                    }),
            )
            .child(buttons);

        // Reveal by easing the bar's own height (and its gap) from zero, so the
        // prompt unfolds in place instead of the page snapping down a row.
        let bar: AnyElement = if theme::reduce_motion(cx) {
            bar.into_any_element()
        } else {
            bar.with_animation(
                "approval-in",
                Animation::new(Duration::from_millis(170)).with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                move |el, d| el.max_h(px(BAR_H * d)).mb(px(GAP * d)).opacity(d),
            )
            .into_any_element()
        };
        Some(bar)
    }

    /// The pending queue pi is holding, shown as a bar directly above the
    /// composer. While a task is running, messages sent from the composer are
    /// queued as follow-ups here and delivered once the task finishes;
    /// `queue_update` mirrors the list live. One raised card in the sibling
    /// strips' shape, a quiet label header, and flat hairline-separated rows.
    pub(super) fn queue_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.queue.is_empty() {
            return None;
        }
        let theme = *theme::get(cx);
        let count = self.queue.len();
        let only_steering = self.queue.follow_up.is_empty();
        // The label names the queue's dominant state in the app's 11px label
        // style; the full delivery sentence stays on the tooltip so the
        // header can be one quiet line.
        let label = if only_steering {
            tr!("view.queue_steering")
        } else {
            tr!("view.queue_queued")
        };
        let detail = if only_steering {
            tr!("view.steering_turn", count = count)
        } else if self.queue.steering.is_empty() {
            tr!("view.queued_after_task", count = count)
        } else {
            tr!("view.steering_plus_followups", count = count)
        };

        let mut rows: Vec<AnyElement> = Vec::with_capacity(count);
        for text in &self.queue.steering {
            rows.push(queue_row(&tr!("view.queue_steer"), text, false, theme));
        }
        for text in &self.queue.follow_up {
            rows.push(queue_row(&tr!("view.queue_follow_up"), text, true, theme));
        }

        Some(
            div()
                .id("queue-bar")
                .debug_selector(|| "queue-bar".to_string())
                .w_full()
                .mb(DynamicSpacing::Base08.px(&theme))
                .rounded(Radius::XLarge.px(&theme))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_elevated)
                .flex()
                .flex_col()
                // Clip the rows' hairline separators to the rounded corners.
                .overflow_hidden()
                .child(
                    // A clock, `QUEUED · 2`, and the one action: a compact
                    // dismiss. No second button, no sentence in the header.
                    div()
                        .h(px(30.))
                        .px(DynamicSpacing::Base12.px(&theme))
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .child(icon(
                            "icons/clock.svg",
                            IconSize::XSmall.px(&theme),
                            theme.text_muted,
                        ))
                        .child(
                            div()
                                .id("queue-label")
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .tip(detail)
                                .text_size(TextSize::XSmall.px(&theme))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_muted)
                                .child(format!("{label} · {count}").to_uppercase()),
                        )
                        .child(
                            icon_button_frame(
                                div()
                                    .id("clear-queue")
                                    .debug_selector(|| "clear-queue".to_string()),
                                &theme,
                                ButtonSize::Compact,
                            )
                            .group(BUTTON_GROUP)
                            .tip(tr!("view.clear_queue"))
                            .cursor_pointer()
                            .text_color(theme.text_muted)
                            .hover(|s| s.bg(theme.surface_hover).text_color(theme.text_primary))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_clear_queue))
                            .child(icon(
                                "icons/x.svg",
                                ButtonSize::Compact.icon_size().px(&theme),
                                theme.text_muted,
                            )),
                        ),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    /// The dismissible error banner: a command / protocol / extension failure
    /// surfaced per the docs' error contract. Persists until dismissed. A
    /// dead pi process offers a one-click Reconnect instead of sending the
    /// user to Settings; every banner can copy its text for a bug report.
    pub(super) fn error_banner(&self, theme: Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let message = self.error.as_ref()?;
        let copy_text = message.clone();
        let reconnect = self.runtime.exited;
        Some(
            div()
                .w_full()
                .mb(DynamicSpacing::Base08.px(&theme))
                .bg(theme.error.opacity(0.1))
                .border_1()
                .border_color(theme.error.opacity(0.45))
                .rounded(Radius::XLarge.px(&theme))
                .px(DynamicSpacing::Base12.px(&theme))
                .py(px(9.))
                .flex()
                .items_center()
                .gap_2()
                .child(icon(
                    "icons/info.svg",
                    IconSize::Medium.px(&theme),
                    theme.error,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_primary)
                        .child(message.clone()),
                )
                .when(reconnect, |banner| {
                    banner.child(
                        button_frame(div().id("reconnect-runtime"), &theme, ButtonSize::Default)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .cursor_pointer()
                            .raised(theme.overlay, &theme)
                            .hover(|s| s.raised(theme.overlay_strong, &theme))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    this.runtime_restart(cx);
                                }),
                            )
                            .child(tr!("view.reconnect")),
                    )
                })
                .child(
                    icon_button_frame(div().id("copy-error"), &theme, ButtonSize::Compact)
                        .tip(tr!("common.copy"))
                        .cursor_pointer()
                        .text_color(theme.text_secondary)
                        .hover(|s| s.bg(theme.surface_hover).text_color(theme.text_primary))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                            }),
                        )
                        .child(icon(
                            "icons/copy.svg",
                            ButtonSize::Compact.icon_size().px(&theme),
                            theme.text_secondary,
                        )),
                )
                .child(
                    icon_button_frame(div().id("dismiss-error"), &theme, ButtonSize::Compact)
                        .tip(tr!("common.dismiss"))
                        .cursor_pointer()
                        .text_size(TextSize::Default.px(&theme))
                        .text_color(theme.text_secondary)
                        .hover(|s| s.bg(theme.surface_hover).text_color(theme.text_primary))
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::dismiss_error))
                        .child(icon(
                            "icons/x.svg",
                            ButtonSize::Compact.icon_size().px(&theme),
                            theme.text_secondary,
                        )),
                )
                .into_any_element(),
        )
    }

    /// Discard the pending queue without aborting the run. The composer text
    /// stays untouched (unlike Escape, which restores queued text).
    pub(super) fn on_clear_queue(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restore_queue_on_clear = false;
        self.send(CommandBody::ClearQueue, "clear_queue");
        cx.notify();
    }

    /// Full-window image lightbox: the clicked attachment at `Contain` scale
    /// over a dimmed scrim. Any click (or Escape) closes it. No new surface
    /// for the app — it reads `OrbitApp::lightbox`, set by `image_opener`.
    pub(super) fn lightbox_layer(
        &self,
        image: std::sync::Arc<Image>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = *theme::get(cx);
        let scrim = theme.scrim_media();
        div()
            .id("image-lightbox")
            .debug_selector(|| "image-lightbox".to_string())
            .absolute()
            .inset_0()
            .occlude()
            .bg(scrim)
            .p(DynamicSpacing::Base48.px(&theme))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.lightbox = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(ImageSource::Image(image))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .into_any_element()
    }
}
