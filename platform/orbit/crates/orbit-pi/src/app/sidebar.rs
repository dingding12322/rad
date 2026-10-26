use super::helpers::*;
use super::*;

use gpui::{point, Pixels};

use crate::sessions::cap_chars;
use crate::shimmer::ShimmerText;
use crate::theme::tokens::{
    context_menu, list_item, picker, popover, BufferLineHeight, ButtonSize, DynamicSpacing,
    IconSize, TextSize,
};
use crate::workspace_mark::{self, WorkspaceMark, WorkspaceStatus};

/// Whether a workspace group is collapsed in the sidebar. The active
/// workspace is expanded by default; all others are collapsed unless the
/// user has toggled them.
pub(crate) fn is_workspace_group_collapsed(
    label: &str,
    working_label: &str,
    collapsed_workspaces: &HashSet<String>,
    expanded_workspace_groups: &HashSet<String>,
) -> bool {
    if label == working_label {
        collapsed_workspaces.contains(label)
    } else {
        !expanded_workspace_groups.contains(label)
    }
}

/// Toggle a workspace group's open/closed state against the defaults above.
pub(crate) fn toggle_workspace_group(
    label: String,
    working_label: &str,
    collapsed_workspaces: &mut HashSet<String>,
    expanded_workspace_groups: &mut HashSet<String>,
) {
    if label == working_label {
        if !collapsed_workspaces.remove(&label) {
            collapsed_workspaces.insert(label);
        }
    } else if !expanded_workspace_groups.remove(&label) {
        expanded_workspace_groups.insert(label);
    }
}

/// Sessions visible under one workspace group: at most `limit` rows, with
/// the open session swapped into the last slot when it falls outside.
pub(crate) fn visible_sessions_in_group(
    ixs: &[usize],
    sessions: &[SessionInfo],
    limit: usize,
    active_path: &Option<PathBuf>,
) -> Vec<usize> {
    if ixs.len() <= limit {
        return ixs.to_vec();
    }

    let mut indices: Vec<usize> = ixs.iter().take(limit).copied().collect();
    if let Some(active) = active_path {
        if let Some(active_ix) = sessions.iter().position(|s| &s.path == active) {
            if ixs.contains(&active_ix) && !indices.contains(&active_ix) {
                indices.pop();
                indices.push(active_ix);
                indices.sort_by_key(|ix| ixs.iter().position(|&i| i == *ix).unwrap_or(usize::MAX));
            }
        }
    }
    indices
}

/// Sidebar session list: `sessions` (newest-first, from disk) plus a
/// placeholder row for the open session when its file is not in the store
/// yet — see [`OrbitApp::sidebar_sessions`]. The placeholder carries the
/// workspace the pi process runs in, pi's live title when it has already
/// named the session, and a `now` stamp so it sorts to the top of the
/// sidebar. A session that hasn't started (`session_started` = false — no
/// user message sent yet) gets no placeholder: a draft is not listed.
pub(crate) fn sessions_with_placeholder(
    sessions: &[SessionInfo],
    current_path: Option<&Path>,
    current_title: Option<&str>,
    current_first_message: Option<&str>,
    current_workspace: Option<&Path>,
    session_started: bool,
) -> Vec<SessionInfo> {
    let mut rows = sessions.to_vec();
    let Some(path) = current_path else {
        return rows;
    };
    if rows.iter().any(|s| s.path == path) {
        return rows;
    }
    if !session_started {
        return rows;
    }
    let workspace = current_workspace
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    // The placeholder carries the same two lines a disk row would: pi's live
    // title (or the first message when pi hasn't named it yet) over the first
    // message preview.
    let first_message = cap_chars(current_first_message.unwrap_or_default(), 110);
    let title = current_title
        .filter(|title| !title.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if first_message.is_empty() {
                tr!("menu.new_task")
            } else {
                cap_chars(current_first_message.unwrap_or_default(), 80)
            }
        });
    rows.insert(
        0,
        SessionInfo {
            path: path.to_path_buf(),
            id: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            cwd: workspace,
            title,
            first_message,
            modified: SystemTime::now(),
        },
    );
    rows
}

/// Whether a session row's second line says anything its title does not.
///
/// pi titles a session from its first message, so on most rows the title *is*
/// that message cut short and the preview is the same sentence carried on — a
/// second line the eye reads and learns nothing from. The preview earns its
/// place only when it diverges, which is the case this keeps: a session pi named
/// itself ("Fix login bug") over the message it came from.
///
/// Compared on words rather than bytes, so the `…` a capped title ends with, its
/// collapsed whitespace, or its trailing punctuation cannot make one sentence
/// look like two.
pub(crate) fn preview_adds_information(title: &str, preview: &str) -> bool {
    let preview_words: Vec<&str> = preview.split_whitespace().collect();
    if preview_words.is_empty() {
        return false;
    }
    let title_words: Vec<&str> = title.split_whitespace().collect();
    // A session with no title yet has nothing to repeat, so anything it has is
    // news; only a real prefix means the preview is the title continued.
    title_words.is_empty() || !continues_the_title(&title_words, &preview_words)
}

/// Whether `preview` picks up exactly where `title` left off. `cap_chars` cuts
/// on the character, not the word, so the title's last word is often a fragment
/// of the preview's — that fragment, and the `…` that marks it, belong to the cap
/// rather than to the sentence.
fn continues_the_title(title: &[&str], preview: &[&str]) -> bool {
    let Some((last, head)) = title.split_last() else {
        return false;
    };
    if preview.len() < title.len() || head.iter().zip(preview).any(|(a, b)| a != b) {
        return false;
    }
    let last = last.trim_end_matches(['…', '.']);
    last.is_empty() || preview[title.len() - 1].starts_with(last)
}

/// Build the grouped sidebar session list from Orbit's own project list. A
/// workspace appears because the user added it, not because pi happens to
/// have sessions there; sessions in unlisted folders are omitted entirely
/// (they stay on disk). Each group shows at most
/// [`SIDEBAR_GROUP_SESSIONS_VISIBLE`] sessions plus one step of
/// [`SIDEBAR_GROUP_SESSIONS_VISIBLE`] per "Show more" click, so a long
/// history never lands in one go.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_sidebar_rows(
    sessions: &[SessionInfo],
    workspaces: &[PathBuf],
    // Sidebar ordering for the workspace groups, plus the per-workspace
    // added-at stamps used by [`WorkspaceSort::DateAdded`].
    sort: WorkspaceSort,
    added_at: &HashMap<PathBuf, SystemTime>,
    working_label: &str,
    collapsed_workspaces: &HashSet<String>,
    expanded_workspace_groups: &HashSet<String>,
    expanded_session_groups: &HashMap<String, usize>,
    pinned: &HashSet<PathBuf>,
    active_path: &Option<PathBuf>,
    // Sessions with a live pi process that is currently mid-run. A running
    // session stays visible under a collapsed group header, like the open
    // session, so active work is never lost behind a collapse.
    running_paths: &HashSet<PathBuf>,
) -> Vec<SideRow> {
    let mut side_rows: Vec<SideRow> = Vec::new();
    for (label, cwd, ixs) in sidebar_groups(
        sessions,
        workspaces,
        sort,
        added_at,
        pinned,
        active_path,
        running_paths,
    ) {
        emit_workspace_group(
            &mut side_rows,
            sessions,
            label,
            cwd,
            &ixs,
            working_label,
            collapsed_workspaces,
            expanded_workspace_groups,
            expanded_session_groups,
            pinned,
            active_path,
            running_paths,
        );
    }
    side_rows
}

/// Order rank for a session row: the open session leads (0), then pinned
/// rows (1), then live background runs (2), then everything else (3), which
/// keeps the store's newest-first order among equals (the sort is stable).
///
/// The open session must *outrank* a background run, not merely share the top
/// partition with it: `modified` moves on every `message` append, so two
/// "leading" sessions still trade places on each watcher reload while both
/// runs write. Ranking the open session first pins the row being read.
fn session_lead_rank(
    path: &Path,
    pinned: &HashSet<PathBuf>,
    active_path: &Option<PathBuf>,
    running_paths: &HashSet<PathBuf>,
) -> u8 {
    if active_path.as_deref() == Some(path) {
        0
    } else if pinned.contains(path) {
        1
    } else if running_paths.contains(path) {
        2
    } else {
        3
    }
}

/// One group per listed project, in the order the user added them — an
/// empty project still gets a header (and its `+`) so a task can start
/// there. Groups are keyed by label, so two paths with the same basename
/// share one header (the first listed path wins). Attaches each session to
/// its project; a folder that is not listed simply finds no group. Used by
/// [`build_sidebar_rows`].
fn sidebar_groups(
    sessions: &[SessionInfo],
    workspaces: &[PathBuf],
    sort: WorkspaceSort,
    added_at: &HashMap<PathBuf, SystemTime>,
    pinned: &HashSet<PathBuf>,
    active_path: &Option<PathBuf>,
    running_paths: &HashSet<PathBuf>,
) -> Vec<(String, PathBuf, Vec<usize>)> {
    let mut groups: Vec<(String, PathBuf, Vec<usize>)> = Vec::new();
    for ws in workspaces {
        let label = sessions::workspace_label(ws);
        if groups.iter().any(|(l, _, _)| *l == label) {
            continue;
        }
        groups.push((label, ws.clone(), Vec::new()));
    }
    for (ix, session) in sessions.iter().enumerate() {
        let label = sessions::workspace_label(&session.cwd);
        if let Some((_, _, ixs)) = groups.iter_mut().find(|(l, _, _)| *l == label) {
            ixs.push(ix);
        }
    }
    // Order the groups for the active sort mode before any rows are laid out;
    // a group's own session order is independent (pinned-first, then recency).
    sort_workspace_groups(&mut groups, sessions, sort, added_at, active_path);
    // Leading sessions rise to the top of their project group by rank — the
    // open one first, then pins, then live runs; recency order is preserved
    // within each rank (the sort is stable). Doing this here rather than in
    // `load_sessions` keeps an Orbit-owned preference out of the pi-store
    // scan — and means a leading session can never be hidden by the
    // per-group truncation below.
    for (_, _, ixs) in groups.iter_mut() {
        ixs.sort_by_key(|&ix| {
            session_lead_rank(&sessions[ix].path, pinned, active_path, running_paths)
        });
    }
    groups
}

/// Lay one workspace group into rows: its header, then either the collapsed
/// always-visible rows or the capped session list with its overflow toggle.
#[allow(clippy::too_many_arguments)]
fn emit_workspace_group(
    side_rows: &mut Vec<SideRow>,
    sessions: &[SessionInfo],
    label: String,
    cwd: PathBuf,
    ixs: &[usize],
    working_label: &str,
    collapsed_workspaces: &HashSet<String>,
    expanded_workspace_groups: &HashSet<String>,
    expanded_session_groups: &HashMap<String, usize>,
    pinned: &HashSet<PathBuf>,
    active_path: &Option<PathBuf>,
    running_paths: &HashSet<PathBuf>,
) {
    let collapsed = is_workspace_group_collapsed(
        &label,
        working_label,
        collapsed_workspaces,
        expanded_workspace_groups,
    );
    side_rows.push(SideRow::Workspace {
        label: label.clone(),
        count: ixs.len(),
        collapsed,
        cwd,
    });
    if collapsed {
        // A collapsed group hides its sessions — except the open one,
        // any running (busy background) ones, and pinned ones. The live
        // session and a deliberate mark stay reachable under the header.
        // `ixs` is already leading-first, so the pinned rows keep their
        // place at the top.
        for &ix in ixs {
            let open = active_path.as_deref() == Some(sessions[ix].path.as_path());
            let running = running_paths.contains(&sessions[ix].path);
            if open || running || pinned.contains(&sessions[ix].path) {
                side_rows.push(SideRow::Session(ix));
            }
        }
        return;
    }

    // Sessions start at the base cap and grow one step per "Show more"
    // click, so a group with a long history reveals three rows at a time.
    let extra = expanded_session_groups.get(&label).copied().unwrap_or(0);
    let limit = SIDEBAR_GROUP_SESSIONS_VISIBLE.saturating_add(extra);
    let visible = visible_sessions_in_group(ixs, sessions, limit, active_path);
    for ix in &visible {
        side_rows.push(SideRow::Session(*ix));
    }

    let hidden_count = ixs.len().saturating_sub(visible.len());
    if hidden_count > 0 {
        side_rows.push(SideRow::ShowMore {
            label: label.clone(),
            count: hidden_count.min(SIDEBAR_GROUP_SESSIONS_VISIBLE),
            // Past the base cap the same row carries the collapse
            // affordance on its right edge, so the group never needs a
            // second toggle row.
            can_collapse: extra > 0,
        });
    } else if extra > 0 {
        // Everything is shown — keep one quiet row to collapse the group
        // back to the base cap.
        side_rows.push(SideRow::ShowLess { label });
    }
}

/// Build the flat sidebar list: every listed project's sessions in one
/// ungrouped list, with no workspace headers, show-more, or collapse rows.
/// Rows lead by rank — the open session, then pins, then live background
/// runs — and the rest keep the store's newest-first order (the sort is
/// stable). Used when the user turns workspace grouping off; the same
/// `SideRow::Session` rows mean the render and keyboard paths are unchanged.
pub(crate) fn build_flat_sidebar_rows(
    sessions: &[SessionInfo],
    pinned: &HashSet<PathBuf>,
    active_path: &Option<PathBuf>,
    running_paths: &HashSet<PathBuf>,
) -> Vec<SideRow> {
    let mut ixs: Vec<usize> = (0..sessions.len()).collect();
    // Stable: sessions rise by rank without disturbing the newest-first
    // order within each rank.
    ixs.sort_by_key(|&ix| {
        session_lead_rank(&sessions[ix].path, pinned, active_path, running_paths)
    });
    ixs.into_iter().map(SideRow::Session).collect()
}

/// Apply the archived-session filter. `Show` keeps every session, `Hide`
/// drops archived ones, and `Only` keeps just the archived ones. Runs before
/// row building so `SideRow::Session` indices stay aligned with the list the
/// render path receives.
pub(crate) fn filter_archived(
    sessions: Vec<SessionInfo>,
    filter: SidebarArchivedFilter,
    archived: &HashSet<PathBuf>,
) -> Vec<SessionInfo> {
    match filter {
        SidebarArchivedFilter::Show => sessions,
        SidebarArchivedFilter::Hide => sessions
            .into_iter()
            .filter(|session| !archived.contains(&session.path))
            .collect(),
        SidebarArchivedFilter::Only => sessions
            .into_iter()
            .filter(|session| archived.contains(&session.path))
            .collect(),
    }
}

/// Reorder `(label, cwd, session indices)` groups for the active sidebar
/// sort mode. `groups` is already deduped by label, so reordering never
/// merges two projects.
///
/// `LastUpdated` keys on each group's newest session activity — the group is
/// not necessarily in activity order after the pin sort, so it takes the max
/// rather than the first row — and finally floats the open session's group to
/// the top, so a background run elsewhere writing messages cannot displace
/// the workspace being worked in. `DateAdded` keys on the persisted added-at
/// stamp. Ties break on the label so the order is stable across reloads, and
/// unknown/empty keys sort last.
fn sort_workspace_groups(
    groups: &mut [(String, PathBuf, Vec<usize>)],
    sessions: &[SessionInfo],
    sort: WorkspaceSort,
    added_at: &HashMap<PathBuf, SystemTime>,
    active_path: &Option<PathBuf>,
) {
    if sort == WorkspaceSort::Manual {
        return;
    }
    let label_key = |label: &str| label.to_lowercase();
    let by_label = |a: &(String, PathBuf, Vec<usize>), b: &(String, PathBuf, Vec<usize>)| {
        label_key(&a.0).cmp(&label_key(&b.0))
    };
    match sort {
        WorkspaceSort::Manual => {}
        WorkspaceSort::AlphabeticalAsc => groups.sort_by(|a, b| by_label(a, b)),
        WorkspaceSort::AlphabeticalDesc => groups.sort_by(|a, b| by_label(b, a)),
        WorkspaceSort::LastUpdated => {
            groups.sort_by(|a, b| {
                group_updated(&b.2, sessions)
                    .cmp(&group_updated(&a.2, sessions))
                    .then_with(|| by_label(a, b))
            });
            // The open session's group leads; a stable pass so the other
            // groups keep their activity order.
            if let Some(active) = active_path {
                groups
                    .sort_by_key(|(_, _, ixs)| !ixs.iter().any(|&ix| &sessions[ix].path == active));
            }
        }
        WorkspaceSort::DateAdded => groups.sort_by(|a, b| {
            added_at
                .get(&b.1)
                .cmp(&added_at.get(&a.1))
                .then_with(|| by_label(a, b))
        }),
        WorkspaceSort::SessionCount => {
            groups.sort_by(|a, b| b.2.len().cmp(&a.2.len()).then_with(|| by_label(a, b)))
        }
    }
}

/// Newest session activity in a group, or `None` for an empty project. Empty
/// groups sort last under [`WorkspaceSort::LastUpdated`].
fn group_updated(ixs: &[usize], sessions: &[SessionInfo]) -> Option<SystemTime> {
    ixs.iter().map(|&ix| sessions[ix].modified).max()
}

/// The scrolled-to workspace's header pinned over the session list, plus how
/// far the next group's header has pushed it up.
pub(crate) struct StickyHeader {
    /// Row index of the pinned [`SideRow::Workspace`] header.
    pub ix: usize,
    /// Pixels to shift the pinned row up (0 while it sits at the list top).
    pub top_offset: Pixels,
}

/// Resolve the sticky header for the session list: the header of the project
/// group that owns the row at the top of the viewport. Every expanded group
/// pins — not just the active one — so whichever long project the reader has
/// scrolled into keeps its name visible. The header scrolls away with its
/// section once the next group's header takes over (same behaviour as the
/// side pane's sticky file header).
pub(crate) fn sticky_sidebar_header(list: &ListState, rows: &[SideRow]) -> Option<StickyHeader> {
    let scroll_top = list.logical_scroll_top();
    // The last header at or above the top row is the pinned section. Clamp a
    // past-the-end offset (the list reads that way while following) to the
    // final row so a short list still resolves its group.
    let scan_end = scroll_top.item_ix.min(rows.len().checked_sub(1)?);
    let header_ix = rows[..=scan_end]
        .iter()
        .rposition(|row| matches!(row, SideRow::Workspace { .. }))?;
    // A collapsed group shows only its always-visible rows (the open, running
    // or pinned sessions), so there is no long history to label — and pinning
    // its header would mask the previous group's rows.
    if matches!(
        rows.get(header_ix),
        Some(SideRow::Workspace {
            collapsed: true,
            ..
        })
    ) {
        return None;
    }
    if scroll_top.item_ix == header_ix && scroll_top.offset_in_item <= px(0.) {
        // The real header is exactly at the top — nothing to pin over it.
        return None;
    }
    // The section ends at the next workspace header (or the list end).
    let next_header_ix = rows[header_ix + 1..]
        .iter()
        .position(|row| matches!(row, SideRow::Workspace { .. }))
        .map(|offset| header_ix + 1 + offset);
    // Push the pinned header up as the next group's header arrives. Item
    // bounds are in window coordinates, so compare against the list's
    // viewport; unmeasured items are too far away to need a push.
    let top_offset = next_header_ix
        .and_then(|next| {
            let bounds = list.bounds_for_item(next)?;
            let viewport = list.viewport_bounds();
            let y_in_viewport = bounds.origin.y - viewport.origin.y;
            (y_in_viewport < bounds.size.height).then_some(y_in_viewport - bounds.size.height)
        })
        .unwrap_or(px(0.));
    Some(StickyHeader {
        ix: header_ix,
        top_offset,
    })
}

/// The status a flat session row's workspace glyph conveys, from the row's
/// own state — the open session leads (open folder), then a run in flight,
/// then an archive. Workspace-level status is never used here, so siblings in
/// the same folder stay plain.
pub(crate) fn session_icon_status(active: bool, running: bool, archived: bool) -> WorkspaceStatus {
    if active {
        WorkspaceStatus::Current
    } else if running {
        WorkspaceStatus::Running
    } else if archived {
        WorkspaceStatus::Archived
    } else {
        WorkspaceStatus::Idle
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_side_row(
    rows: &Rc<Vec<SideRow>>,
    sessions_data: &Rc<Vec<SessionInfo>>,
    active_path: Option<&Path>,
    row_ix: usize,
    this: &Entity<OrbitApp>,
    agent_running: bool,
    running_paths: &Rc<HashSet<PathBuf>>,
    // Sessions the user pinned — they lead their project group and take a
    // small pin glyph on the title line.
    pinned_paths: &Rc<HashSet<PathBuf>>,
    // Sessions in the archive set — shown with a small archive glyph when the
    // filter includes them.
    archived_paths: &Rc<HashSet<PathBuf>>,
    // Flat "one list" mode has no workspace headers, so each row names its
    // workspace instead.
    show_workspace: bool,
    // Every session with a live (running or warm-idle) pi process. Guards
    // delete, which would otherwise let an alive process recreate the file.
    live_paths: &Rc<HashSet<PathBuf>>,
    // Per-workspace sidebar marks (icon + tint), keyed by path.
    marks: &Rc<HashMap<PathBuf, WorkspaceMark>>,
    // Resolved per-workspace glyphs: the user's mark icon, or the folder
    // status (current / running / archived-only / idle).
    workspace_icons: &Rc<HashMap<PathBuf, SharedString>>,
    session_menu: Option<&SessionMenu>,
    workspace_menu: Option<&WorkspaceMenu>,
    // Whether this row is the keyboard cursor (see `OrbitApp::sidebar_cursor`).
    // Painted with a focused surface, distinct from hover and selection.
    cursor: bool,
    theme: Theme,
) -> impl IntoElement {
    match &rows[row_ix] {
        SideRow::Workspace {
            label,
            count,
            collapsed,
            cwd,
        } => {
            let label = label.clone();
            let label_for_click = label.clone();
            let cwd_for_new = cwd.clone();
            let this_toggle = this.clone();
            let this_new = this.clone();
            let this_menu = this.clone();
            let menu = workspace_menu.filter(|m| m.label == label);
            let mark = marks.get(cwd).cloned().unwrap_or_default();
            let workspace_icon = workspace_icons
                .get(cwd)
                .cloned()
                .unwrap_or_else(|| workspace_mark::icon_path(mark.icon.as_deref()));
            // Outer shell: inter-group spacing only — horizontal inset comes
            // from the list's `px_2`, so the hover pill lines up with the
            // session rows' (inside the same container) and the chevron lands
            // under the "Projects" label. Hover lives on the inner card so the
            // highlight doesn't bleed into the padding. The header is the
            // parent row above its sessions: chevron + mark + name, the
            // session count pinned to the very end, and the row actions (a `…`
            // menu, then the new-session `+`) fading in to the count's left on
            // hover (all flex_none, so nothing shifts when they appear). The
            // name takes the session title's size — only bolder — so the group
            // reads as their parent, not as another section label; the mark
            // and chevron step up with it.
            div()
                .w_full()
                .pt(DynamicSpacing::Base12.px(&theme))
                .pb(DynamicSpacing::Base02.px(&theme))
                .group("workspace-row")
                .child(
                    div()
                        .w_full()
                        .h(px(26.))
                        .pl(DynamicSpacing::Base06.px(&theme))
                        .pr(DynamicSpacing::Base08.px(&theme))
                        .rounded(list_item::RADIUS.px(&theme))
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .cursor_pointer()
                        .when(cursor, |s| s.bg(theme.surface_hover))
                        .hover(|s| s.bg(theme.surface_hover))
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            let label = label_for_click.clone();
                            this_toggle.update(cx, |app, cx| {
                                let working = app.workspace_label();
                                toggle_workspace_group(
                                    label,
                                    &working,
                                    &mut app.collapsed_workspaces,
                                    &mut app.expanded_workspace_groups,
                                );
                                cx.notify();
                            });
                        })
                        // Right-click anywhere on the header opens its
                        // actions menu at the pointer, context-menu style.
                        .on_mouse_down(MouseButton::Right, {
                            let label = label.clone();
                            let cwd = cwd.clone();
                            let this = this.clone();
                            move |event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                let menu = WorkspaceMenu {
                                    label: label.clone(),
                                    cwd: cwd.clone(),
                                    at: Some(event.position),
                                    picker: false,
                                };
                                this.update(cx, |app, cx| {
                                    app.open_workspace_menu_at(menu, window, cx);
                                });
                            }
                        })
                        .child(icon(
                            if *collapsed {
                                "icons/chevron-right.svg"
                            } else {
                                "icons/chevron-down.svg"
                            },
                            IconSize::XSmall.px(&theme),
                            theme.text_muted,
                        ))
                        .child(icon_dyn(
                            workspace_icon,
                            IconSize::Medium.px(&theme),
                            workspace_mark::tint_color(&theme, mark.tint.as_deref()),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(TextSize::Default.px(&theme))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(label.clone()),
                        )
                        // Row actions, revealed on hover: a `…` menu
                        // (remove / copy path) beside the direct new-task `+`.
                        .child(workspace_menu_button(
                            label.clone(),
                            cwd.clone(),
                            menu,
                            mark.clone(),
                            this_menu.clone(),
                            theme,
                        ))
                        .child(
                            icon_button_frame(
                                div().id(ElementId::Name(format!("workspace-new-{label}").into())),
                                &theme,
                                ButtonSize::Compact,
                            )
                            .tip(crate::commands::tooltip(
                                crate::commands::CommandId::NewSession,
                                false,
                            ))
                            .cursor_pointer()
                            // Revealed on row hover — the quiet default
                            // keeps group headers to just label + count.
                            .opacity(0.)
                            .group_hover("workspace-row", |s| s.opacity(1.))
                            .hover(|s| s.bg(theme.surface_hover))
                            .on_mouse_up(MouseButton::Left, {
                                let this = this_new.clone();
                                move |_, window, cx| {
                                    cx.stop_propagation();
                                    let cwd = cwd_for_new.clone();
                                    this.update(cx, |app, cx| {
                                        app.on_new_session_in_workspace(cwd, window, cx);
                                    });
                                }
                            })
                            .child(icon(
                                "icons/plus.svg",
                                ButtonSize::Compact.icon_size().px(&theme),
                                theme.text_muted,
                            )),
                        )
                        // Session count, pinned to the header's right edge.
                        // Small, muted metadata — legible beside the parent row
                        // without competing with the name. A project with
                        // nothing in it says so by having no rows under it;
                        // a `0` here would only be a number doing no work.
                        .when(*count > 0, |header| {
                            header.child(
                                div()
                                    .flex_none()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_muted)
                                    .child(format!("{count}")),
                            )
                        }),
                )
                .into_any_element()
        }
        SideRow::ShowMore {
            label,
            count,
            can_collapse,
        } => {
            let this = this.clone();
            let label_for_click = label.clone();
            // One click reveals exactly one step (or the tail remainder), so
            // the group can never overshoot its session count.
            let step = *count;
            let can_collapse = *can_collapse;
            let this_for_collapse = this.clone();
            let label_for_collapse = label.clone();
            // Left side is the whole "show more" target; the collapse
            // chevron on the right is its own quiet button.
            let more = div()
                .flex_1()
                .h_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .cursor_pointer()
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    let label = label_for_click.clone();
                    this.update(cx, |app, cx| {
                        app.expanded_session_groups
                            .entry(label)
                            .and_modify(|extra| *extra += step)
                            .or_insert(step);
                        cx.notify();
                    });
                })
                .child(icon(
                    "icons/chevron-down.svg",
                    IconSize::XSmall.px(&theme),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(tr!("sidebar.show_count_more", count = count)),
                );
            let mut row = div()
                .w_full()
                .h(px(26.))
                .pl(px(22.))
                .pr(DynamicSpacing::Base04.px(&theme))
                .flex()
                .items_center()
                .rounded(list_item::RADIUS.px(&theme))
                .when(cursor, |s| s.bg(theme.surface_hover))
                .hover(|s| s.bg(theme.surface_hover))
                .child(more);
            if can_collapse {
                row = row.child(
                    icon_button_frame(div(), &theme, ButtonSize::Compact)
                        .tip(tr!("sidebar.show_less"))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_hover))
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let label = label_for_collapse.clone();
                            this_for_collapse.update(cx, |app, cx| {
                                app.expanded_session_groups.remove(&label);
                                cx.notify();
                            });
                        })
                        .child(icon(
                            "icons/chevron-up.svg",
                            IconSize::XSmall.px(&theme),
                            theme.text_muted,
                        )),
                );
            }
            row.into_any_element()
        }
        SideRow::ShowLess { label } => {
            let this = this.clone();
            let label_for_click = label.clone();
            div()
                .w_full()
                .h(px(26.))
                .pl(px(22.))
                .pr(DynamicSpacing::Base08.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .rounded(list_item::RADIUS.px(&theme))
                .cursor_pointer()
                .when(cursor, |s| s.bg(theme.surface_hover))
                .hover(|s| s.bg(theme.surface_hover))
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    let label = label_for_click.clone();
                    this.update(cx, |app, cx| {
                        app.expanded_session_groups.remove(&label);
                        cx.notify();
                    });
                })
                .child(icon(
                    "icons/chevron-up.svg",
                    IconSize::XSmall.px(&theme),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(tr!("sidebar.show_less")),
                )
                .into_any_element()
        }
        SideRow::Session(ix) => {
            let session = sessions_data[*ix].clone();
            let session_for_click = session.clone();
            let active = active_path == Some(session.path.as_path());
            // The row's place among the visible sessions is the number `⌘1…⌘9`
            // opens it by — the same order `navigable_sessions` cycles.
            let slot = rows[..row_ix]
                .iter()
                .filter(|row| matches!(row, SideRow::Session(_)))
                .count()
                + 1;
            let shortcut_hint = (slot <= 9).then(|| {
                tr!(
                    "sidebar.open_with_shortcut",
                    keys = crate::platform::shortcuts::label(&format!("secondary-{slot}"))
                )
            });
            // The open session runs live; parked (background) sessions run
            // in their own pi processes — both get the loader.
            let running = (active && agent_running) || running_paths.contains(&session.path);
            let pinned = pinned_paths.contains(&session.path);
            let archived = archived_paths.contains(&session.path);
            let this = this.clone();
            let this_for_row = this.clone();
            let this_for_menu = this.clone();
            let menu = session_menu.filter(|m| m.path == session.path);
            // Sessions with a live pi process (running or warm) must not be
            // deleted — the process would recreate the file mid-run.
            let deletable = !active && !running && !live_paths.contains(&session.path);
            // Running sessions lead with a small spinner and a shimmering
            // title (shadcn's Marker + `shimmer`); row actions stay available
            // on hover.
            let title = session_title(*ix, session.title.clone().into(), theme, active, running);
            // Indented under its workspace group so the list reads as a
            // tree. The open session takes the `selection` fill with `selection_fg`
            // ink — the same selected-destination grammar as the nav rows;
            // row actions are revealed on hover. A first-message preview adds
            // a second line only when it says something the title does not
            // ([`preview_adds_information`]); a title that is already the
            // prompt stays one line.
            // Outer item carries the inter-row spacing (padding) and the click
            // handler; the inner card holds the background/hover so the gap
            // between cards stays clear. Padding (not margin) is used because
            // the list measures each item's border-box — margins are dropped.
            let mut row = div()
                .id(ElementId::NamedInteger("side-session".into(), *ix as u64))
                .w_full()
                .py(DynamicSpacing::Base01.px(&theme))
                .cursor_pointer()
                .when_some(shortcut_hint, |row, hint| row.tip(hint))
                // Right-click anywhere on the row opens its actions menu at
                // the pointer, context-menu style (context menus are the
                // secondary-action surface; the `…` button stays for
                // discoverability).
                .on_mouse_down(MouseButton::Right, {
                    let session = session.clone();
                    let this = this.clone();
                    move |event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.update(cx, |app, cx| {
                            app.open_session_menu_at(
                                SessionMenu {
                                    path: session.path.clone(),
                                    title: session.title.clone(),
                                    deletable,
                                    confirm_delete: false,
                                    at: Some(event.position),
                                },
                                window,
                                cx,
                            );
                        });
                    }
                })
                .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                    let session = session_for_click.clone();
                    this_for_row.update(cx, |app, cx| {
                        // A click is a mouse hand-off: leave keyboard nav and
                        // hand focus back to the composer so typing resumes.
                        app.sidebar_cursor = None;
                        app.on_open_session(session, cx);
                        app.input.read(cx).focus(window);
                    });
                });
            let mut card = div()
                .group("srow")
                .relative()
                .w_full()
                .pl(px(22.))
                .pr(DynamicSpacing::Base08.px(&theme))
                .py(DynamicSpacing::Base04.px(&theme))
                .rounded(list_item::RADIUS.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .when(active, |card| {
                    card.bg(theme.selection).text_color(theme.selection_fg)
                })
                .when(cursor && !active, |card| card.bg(theme.surface_hover))
                .when(!active && !cursor, |card| {
                    card.hover(|s| s.bg(theme.surface_hover))
                });
            // Text column: title + actions, then the first-message preview
            // with the age. The title line is the row's anchor and is always
            // there; the preview is an expansion on it, so a row whose preview
            // is just the title continued drops to one line rather than
            // spending a second line re-reading the same sentence. Rows are
            // therefore one or two lines tall, and the age rides whichever
            // line is last.
            let age = sessions::relative_time(session.modified);
            let show_preview = preview_adds_information(&session.title, &session.first_message);
            // Flat list has no workspace headers — name the workspace on the
            // row instead. The glyph is the row's own status (open on the
            // active session, sync while running, archive when archived),
            // falling back to the workspace mark / plain folder.
            let workspace_label = sessions::workspace_label(&session.cwd);
            let mark = marks.get(&session.cwd).cloned().unwrap_or_default();
            let workspace_icon = workspace_mark::resolved_icon_path(
                &mark,
                session_icon_status(active, running, archived),
            );
            card = card.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(DynamicSpacing::Base02.px(&theme))
                    // Line 1 — title with the row actions pinned to its
                    // right: a small running spinner leads, the hover-revealed
                    // `…` menu trails. Wrapped in a flex row so the
                    // `flex_1 min_w_0` title takes the full column width and
                    // paints the name — instead of collapsing to "…" as a bare
                    // flex-column child.
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base06.px(&theme))
                            .when(running, |line| {
                                line.child(crate::app::running_spinner(
                                    ElementId::NamedInteger("side-spin".into(), *ix as u64),
                                    IconSize::XSmall.px(&theme),
                                    theme.accent_text,
                                    theme,
                                ))
                            })
                            .when(pinned, |line| {
                                line.child(icon(
                                    "icons/pin.svg",
                                    IconSize::Medium.px(&theme),
                                    theme.text_muted,
                                ))
                            })
                            .when(archived, |line| {
                                line.child(icon(
                                    "icons/archive.svg",
                                    IconSize::Medium.px(&theme),
                                    theme.text_muted,
                                ))
                            })
                            .child(title)
                            .when(active && show_workspace, |line| {
                                line.child(
                                    div()
                                        .flex_none()
                                        .text_size(TextSize::XSmall.px(&theme))
                                        .text_color(theme.accent_text)
                                        .child(tr!("sidebar.current")),
                                )
                            })
                            .child(session_menu_button(
                                *ix,
                                menu,
                                session.path.clone(),
                                session.title.clone(),
                                deletable,
                                this_for_menu,
                                theme,
                            ))
                            .when(!show_preview && !show_workspace, |line| {
                                line.child(
                                    div()
                                        .flex_none()
                                        .text_size(TextSize::XSmall.px(&theme))
                                        .text_color(theme.text_muted)
                                        .child(age.clone()),
                                )
                            }),
                    )
                    // Line 2 — the workspace name (flat list only), the
                    // first-message preview, and the age at the very end.
                    .when(show_preview || show_workspace, |col| {
                        col.child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(DynamicSpacing::Base06.px(&theme))
                                .when(show_workspace, |line| {
                                    line.child(icon_dyn(
                                        workspace_icon,
                                        IconSize::XSmall.px(&theme),
                                        workspace_mark::tint_color(&theme, mark.tint.as_deref()),
                                    ))
                                    .child(
                                        div()
                                            .flex_none()
                                            .max_w(px(120.))
                                            .truncate()
                                            .text_size(TextSize::XSmall.px(&theme))
                                            .text_color(theme.text_secondary)
                                            .child(workspace_label),
                                    )
                                })
                                .when(show_preview, |line| {
                                    line.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(TextSize::Small.px(&theme))
                                            .line_height(BufferLineHeight::Standard.relative())
                                            .text_color(if active {
                                                theme.text_secondary
                                            } else {
                                                theme.text_muted
                                            })
                                            .child(session.first_message.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(TextSize::XSmall.px(&theme))
                                        .text_color(if active {
                                            theme.text_secondary
                                        } else {
                                            theme.text_muted
                                        })
                                        .child(age),
                                ),
                        )
                    }),
            );
            row = row.child(card);
            row.into_any_element()
        }
    }
}

/// The hover-revealed '…' button on a quiet session row. Clicking it opens
/// the row's actions popup; propagation stops so the row's open handler
/// doesn't also fire.
pub(crate) fn session_menu_button(
    ix: usize,
    menu: Option<&SessionMenu>,
    path: PathBuf,
    title: String,
    deletable: bool,
    this: Entity<OrbitApp>,
    theme: Theme,
) -> impl IntoElement + use<> {
    let menu_open = menu.is_some();
    let this_for_popup = this.clone();
    icon_button_frame(
        div().id(ElementId::NamedInteger("side-more".into(), ix as u64)),
        &theme,
        ButtonSize::Compact,
    )
    .group(BUTTON_GROUP)
    .relative()
    .tip(tr!("sidebar.more_actions"))
    .cursor_pointer()
    // Hidden until the row (or the button itself) is hovered, or while
    // this row's menu is open. Icon-only, no background — a filled hover
    // square reads as a patch covering the row's right edge.
    .opacity(if menu_open { 1.0 } else { 0.0 })
    .group_hover("srow", |s| s.opacity(1.))
    .active(|s| s.opacity(PRESS_DIM))
    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
        // Keep the click from also opening the session via the row.
        cx.stop_propagation();
        let (path, title, deletable) = (path.clone(), title.clone(), deletable);
        this.update(cx, |app, cx| {
            app.toggle_session_menu(
                SessionMenu {
                    path,
                    title,
                    deletable,
                    confirm_delete: false,
                    at: None,
                },
                window,
                cx,
            );
        });
    })
    .child(icon(
        "icons/more.svg",
        ButtonSize::Compact.icon_size().px(&theme),
        theme.text_muted,
    ))
    // The dropdown hangs off a zero-size anchor pinned to the button's
    // top-left corner. The button centers its icon (`items_center` +
    // `justify_center`), and Taffy lays absolutely-positioned children out
    // with the container's alignment — without the pin, the popup's
    // static position is pulled toward the button's center and it opens
    // up-left of the trigger instead of just below it.
    .children(menu.map(|menu| {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size(px(0.))
            .child(session_menu_popup(menu, this_for_popup.clone(), theme))
    }))
}

/// A session row's title. Quiet rows render as one truncated line; a running
/// row paints the shadcn `shimmer` — a highlight band sweeping across the
/// glyphs — driven by `with_animation` (self-repainting, no app tick).
fn session_title(
    id: usize,
    title: SharedString,
    theme: Theme,
    active: bool,
    running: bool,
) -> AnyElement {
    let color = if active {
        theme.text_primary
    } else {
        theme.text_secondary
    };
    let weight = if active {
        FontWeight::MEDIUM
    } else {
        FontWeight::NORMAL
    };
    let size = TextSize::Default.px(&theme);
    let line_height = BufferLineHeight::Standard.relative();
    if !running {
        return div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(size)
            .line_height(line_height)
            .font_weight(weight)
            .text_color(color)
            .child(title)
            .into_any_element();
    }
    // Reduce-motion: keep the running signal (accent title) without the
    // perpetual sweep.
    if theme.ui.reduce_motion {
        return div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(size)
            .line_height(line_height)
            .font_weight(weight)
            .text_color(theme.accent_text)
            .child(title)
            .into_any_element();
    }
    // The sweep catches the accent: ink at the edges, accent through the band
    // (shadcn's highlight, spent on the one hue the system rations).
    let highlight = theme.accent;
    div()
        .flex_1()
        .min_w_0()
        .text_size(size)
        .line_height(line_height)
        .font_weight(weight)
        .child(ShimmerText::new(title, color, highlight).with_animation(
            ElementId::NamedInteger("side-shimmer".into(), id as u64),
            Animation::new(Duration::from_millis(2000)).repeat(),
            |mut text, delta| {
                text.phase = delta;
                text
            },
        ))
        .into_any_element()
}

// ── sidebar row-actions popup ──────────────────────────────────────────

/// The actions popup anchored to a session row: Copy path / Reveal in
/// Finder / Delete, or the delete confirmation once armed. Painted via
/// `deferred` + `anchored` (same convention as the composer pickers),
/// dismissed by any outside mouse-down.
pub(crate) fn session_menu_popup(
    menu: &SessionMenu,
    this: Entity<OrbitApp>,
    theme: Theme,
) -> AnyElement {
    let deletable = menu.deletable;
    let confirm = menu.confirm_delete;
    let pinned = crate::pins::contains(&menu.path);
    let archived = crate::archive::contains(&menu.path);

    let body: AnyElement = if confirm {
        // Delete confirmation — the destructive step gets a named victim.
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base02.px(&theme))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("sidebar.delete_this_session")),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("sidebar.removes_the_session_file_from_disk")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(
                        button_frame(div().id("menu-cancel"), &theme, ButtonSize::Default)
                            .raised(theme.surface_elevated, &theme)
                            .cursor_pointer()
                            .hover(|s| s.raised(theme.surface_hover, &theme))
                            .text_color(theme.text_secondary)
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    this.update(cx, |app, cx| app.on_menu_cancel(cx));
                                }
                            })
                            .child(tr!("sidebar.cancel")),
                    )
                    .child(
                        button_frame(div().id("menu-confirm-delete"), &theme, ButtonSize::Default)
                            .raised(theme.error, &theme)
                            .cursor_pointer()
                            .hover(|s| s.raised(theme.error_hover, &theme))
                            .text_color(theme.error_fg)
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    this.update(cx, |app, cx| app.on_menu_delete_confirm(cx));
                                }
                            })
                            .child(tr!("sidebar.delete")),
                    ),
            )
            .into_any_element()
    } else {
        div()
            .w_full()
            .flex()
            .flex_col()
            .py(picker::list_padding_y(&theme))
            .gap(DynamicSpacing::Base01.px(&theme))
            .child(menu_item(
                "menu-pin",
                "icons/pin.svg",
                if pinned {
                    tr!("sidebar.unpin_session")
                } else {
                    tr!("sidebar.pin_session")
                },
                theme,
                this.clone(),
                false,
                |app, cx| app.on_menu_toggle_pin(cx),
            ))
            .child(menu_item(
                "menu-archive",
                "icons/archive.svg",
                if archived {
                    tr!("sidebar.unarchive_session")
                } else {
                    tr!("sidebar.archive_session")
                },
                theme,
                this.clone(),
                false,
                |app, cx| app.on_menu_toggle_archive(cx),
            ))
            .child(menu_item(
                "menu-copy-path",
                "icons/copy.svg",
                tr!("sidebar.copy_path"),
                theme,
                this.clone(),
                false,
                |app, cx| app.on_menu_copy_path(cx),
            ))
            .child(menu_item(
                "menu-reveal",
                "icons/folder.svg",
                tr!("sidebar.reveal_in_finder"),
                theme,
                this.clone(),
                false,
                |app, cx| app.on_menu_reveal(cx),
            ))
            .child(menu_item(
                "menu-clone",
                "icons/git-fork.svg",
                tr!("sidebar.clone_session"),
                theme,
                this.clone(),
                false,
                |app, cx| app.on_menu_clone_session(cx),
            ))
            .when(deletable, |menu| {
                menu.child(context_menu_separator(&theme)).child(menu_item(
                    "menu-delete",
                    "icons/trash.svg",
                    tr!("sidebar.delete_session_menu"),
                    theme,
                    this.clone(),
                    true,
                    |app, cx| app.on_menu_delete_request(cx),
                ))
            })
            .into_any_element()
    };

    // The branch selector's picker shell sizes the action list to its
    // entries; the delete confirmation keeps its own fixed card.
    let popup = picker_surface(div(), &theme)
        .when(confirm, |pop| {
            pop.w(px(210.)).p(DynamicSpacing::Base12.px(&theme))
        })
        .flex()
        .flex_col()
        .overflow_hidden()
        .occlude()
        // Any mouse-down outside dismisses; clicks inside are stopped by
        // the item handlers, so they never read as "outside".
        .on_mouse_down_out({
            let this = this.clone();
            move |_, _, cx| {
                this.update(cx, |app, cx| {
                    // Arm the gesture guard so this same click's mouse-up
                    // cannot immediately re-open the menu.
                    app.menu_dismissed_at = Some(Instant::now());
                    app.session_menu = None;
                    cx.notify();
                })
            }
        })
        .child(body);

    // Float the popup: `anchored` takes it out of the layout (no other row
    // moves). A right-clicked menu lands at the pointer in window
    // coordinates; the '…' button's menu pins its top-left corner just
    // below the trigger (a `ButtonSize::Compact` button, so its height plus
    // `popover::MENU_OFFSET` drops the top edge Zed's menu gap under it,
    // left-aligned to the trigger — the standard app dropdown position). `deferred` paints it above the rest of the list — same
    // convention as the composer chip pickers. `snap_to_window` keeps it
    // inside the window near the edges.
    let anchor = if let Some(at) = menu.at {
        anchored().anchor(Corner::TopLeft).position(at)
    } else {
        anchored()
            .position_mode(AnchoredPositionMode::Local)
            .anchor(Corner::TopLeft)
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

/// The hover-revealed '…' button on a workspace group header. Clicking it
/// opens the header's actions popup; propagation stops so the header's
/// collapse toggle doesn't also fire.
pub(crate) fn workspace_menu_button(
    label: String,
    cwd: PathBuf,
    menu: Option<&WorkspaceMenu>,
    mark: WorkspaceMark,
    this: Entity<OrbitApp>,
    theme: Theme,
) -> impl IntoElement + use<> {
    let this_for_popup = this.clone();
    let label_for_click = label.clone();
    let cwd_for_click = cwd.clone();
    icon_button_frame(
        div().id(ElementId::Name(format!("workspace-more-{label}").into())),
        &theme,
        ButtonSize::Compact,
    )
    .group(BUTTON_GROUP)
    .relative()
    .tip(tr!("sidebar.more_actions"))
    .cursor_pointer()
    // Revealed on row hover, or while this header's menu is open.
    .opacity(if menu.is_some() { 1.0 } else { 0.0 })
    .group_hover("workspace-row", |s| s.opacity(1.))
    .active(|s| s.opacity(PRESS_DIM))
    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
        cx.stop_propagation();
        let menu = WorkspaceMenu {
            label: label_for_click.clone(),
            cwd: cwd_for_click.clone(),
            at: None,
            picker: false,
        };
        this.update(cx, |app, cx| app.toggle_workspace_menu(menu, window, cx));
    })
    .child(icon(
        "icons/more.svg",
        ButtonSize::Compact.icon_size().px(&theme),
        theme.text_muted,
    ))
    .children(menu.map(|menu| {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size(px(0.))
            .child(workspace_menu_popup(
                menu,
                mark.clone(),
                this_for_popup.clone(),
                theme,
            ))
    }))
}

/// One row of the workspace header menu, on the branch selector's picker
/// metrics ([`picker_entry`]): a leading glyph beside the label, with the
/// picker row's inset, padding, height, and radius. The click closes the menu
/// through the app action it runs.
fn workspace_menu_item<C, L>(
    id: &'static str,
    icon_path: &'static str,
    label: L,
    theme: Theme,
    this: Entity<OrbitApp>,
    on_click: C,
) -> impl IntoElement + use<C, L>
where
    C: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
    L: Into<SharedString>,
{
    let label: SharedString = label.into();
    picker_entry(div().id(id), &theme)
        .h(picker::entry_height(&theme))
        .flex_none()
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_hover))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            this.update(cx, |app, cx| (on_click)(app, cx));
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
                .truncate()
                .text_color(theme.text_secondary)
                .child(label),
        )
}

/// The actions popup anchored to a workspace header: Copy path / Icon & color /
/// Remove from sidebar, or — once `Icon & color` is chosen — the picker itself.
/// Uses the branch selector's picker surface and row metrics; deferred and
/// anchored like the session row menu, dismissed by any outside mouse-down.
pub(crate) fn workspace_menu_popup(
    menu: &WorkspaceMenu,
    mark: WorkspaceMark,
    this: Entity<OrbitApp>,
    theme: Theme,
) -> AnyElement {
    let body: AnyElement = if menu.picker {
        workspace_appearance_picker(menu, mark, this.clone(), theme)
    } else {
        // The action rows wear the branch selector's picker metrics — the
        // same Base04 inset, Base06 inner padding, 31px row, and Base01 row
        // gap — so the workspace menu and the branch/sort popovers read as
        // one family.
        div()
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base01.px(&theme))
            .child(workspace_menu_item(
                "wm-copy-path",
                "icons/copy.svg",
                tr!("sidebar.copy_path"),
                theme,
                this.clone(),
                |app, cx| app.on_workspace_copy_path(cx),
            ))
            .child(workspace_menu_item(
                "wm-worktrees",
                "icons/branch.svg",
                tr!("worktree.page.title"),
                theme,
                this.clone(),
                |app, cx| {
                    app.workspace_menu = None;
                    app.open_worktrees(cx);
                },
            ))
            .child(workspace_menu_item(
                "wm-icon-color",
                "icons/contrast.svg",
                tr!("sidebar.icon_and_color"),
                theme,
                this.clone(),
                |app, cx| app.on_workspace_open_picker(cx),
            ))
            .child(workspace_menu_item(
                "wm-remove",
                "icons/minus.svg",
                tr!("sidebar.remove_from_sidebar"),
                theme,
                this.clone(),
                |app, cx| app.on_workspace_remove(cx),
            ))
            .into_any_element()
    };

    // Same picker surface as the branch selector (and the sidebar's sort
    // menu). One fixed width across both modes means opening the icon picker
    // never resizes the popover.
    let popup = picker_surface(div(), &theme)
        .w(workspace_picker_width(&theme))
        .flex()
        .flex_col()
        .overflow_hidden()
        .occlude()
        .on_mouse_down_out({
            let this = this.clone();
            move |_, _, cx| {
                this.update(cx, |app, cx| {
                    // Arm the gesture guard so this same click's mouse-up
                    // cannot immediately re-open the menu.
                    app.menu_dismissed_at = Some(Instant::now());
                    app.workspace_menu = None;
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
            .anchor(Corner::TopLeft)
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

/// A grid cell's side: a square hit target that holds an [`IconSize::Medium`]
/// glyph with even padding all round.
fn workspace_picker_cell(theme: &Theme) -> Pixels {
    DynamicSpacing::Base24.px(theme)
}

/// Spacing between grid cells and tint swatches.
fn workspace_picker_gap(theme: &Theme) -> Pixels {
    DynamicSpacing::Base06.px(theme)
}

/// The inset from the surface edge to the picker's content. Used for the
/// header, swatches, and grid alike so their left and right edges line up.
fn workspace_picker_padding_x(theme: &Theme) -> Pixels {
    DynamicSpacing::Base16.px(theme)
}

/// The width the picker surface needs — the icon grid plus its horizontal
/// padding, so the content fills the popup exactly with no trailing gap.
fn workspace_picker_width(theme: &Theme) -> Pixels {
    let columns = workspace_mark::ICON_COLUMNS as f32;
    workspace_picker_cell(theme) * columns
        + workspace_picker_gap(theme) * (columns - 1.)
        + workspace_picker_padding_x(theme) * 2.
}

/// The icon-and-color picker shown in place of the workspace menu: a row of
/// semantic tint dots over a grid of curated HugeIcons. Picking either writes
/// the mark and persists it; `Reset` clears both back to the folder default.
fn workspace_appearance_picker(
    menu: &WorkspaceMenu,
    mark: WorkspaceMark,
    this: Entity<OrbitApp>,
    theme: Theme,
) -> AnyElement {
    let cell = workspace_picker_cell(&theme);
    let gap = workspace_picker_gap(&theme);

    let mut dots = div().flex().w_full().justify_between();
    for (key, color_of) in workspace_mark::TINTS {
        let selected = mark
            .tint
            .as_deref()
            .map_or(*key == "default", |tint| tint == *key);
        let cwd = menu.cwd.clone();
        let mark = mark.clone();
        let this = this.clone();
        dots = dots.child(
            div()
                .id(ElementId::Name(format!("wm-tint-{key}").into()))
                .size(IconSize::Medium.px(&theme))
                .flex_none()
                .rounded_full()
                .bg(color_of(&theme))
                .border_2()
                .border_color(if selected {
                    theme.text_primary
                } else {
                    theme.border_strong
                })
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    let next = WorkspaceMark {
                        icon: mark.icon.clone(),
                        tint: Some((*key).to_string()),
                    };
                    this.update(cx, |app, cx| {
                        app.set_workspace_mark(cwd.clone(), next);
                        cx.notify();
                    });
                }),
        );
    }

    let ink = workspace_mark::tint_color(&theme, mark.tint.as_deref());
    let mut grid = div().flex().flex_col().gap(gap);
    for row in workspace_mark::PROJECT_ICONS.chunks(workspace_mark::ICON_COLUMNS) {
        let mut cells = div().flex().gap(gap);
        for stem in row {
            let selected = mark
                .icon
                .as_deref()
                .map_or(*stem == "folder", |icon| icon == *stem);
            let cwd = menu.cwd.clone();
            let mark = mark.clone();
            let this = this.clone();
            cells = cells.child(
                div()
                    .id(ElementId::Name(format!("wm-icon-{stem}").into()))
                    .size(cell)
                    .flex_none()
                    .rounded(list_item::RADIUS.px(&theme))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(theme.selection))
                    .hover(|s| s.bg(theme.surface_hover))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        let next = WorkspaceMark {
                            icon: Some((*stem).to_string()),
                            tint: mark.tint.clone(),
                        };
                        this.update(cx, |app, cx| {
                            app.set_workspace_mark(cwd.clone(), next);
                            cx.notify();
                        });
                    })
                    .child(icon_dyn(
                        workspace_mark::icon_path(Some(stem)),
                        IconSize::Medium.px(&theme),
                        ink,
                    )),
            );
        }
        grid = grid.child(cells);
    }

    let reset_cwd = menu.cwd.clone();
    let this_reset = this.clone();
    let header = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(tr!("sidebar.icon_and_color")),
        )
        .child(
            div()
                .id("wm-mark-reset")
                .flex_none()
                .cursor_pointer()
                .px(DynamicSpacing::Base04.px(&theme))
                .py(DynamicSpacing::Base01.px(&theme))
                .rounded(list_item::RADIUS.px(&theme))
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .hover(|s| s.bg(theme.surface_hover))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    this_reset.update(cx, |app, cx| {
                        app.set_workspace_mark(reset_cwd.clone(), WorkspaceMark::default());
                        cx.notify();
                    });
                })
                .child(tr!("sidebar.reset")),
        );

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base08.px(&theme))
        .px(workspace_picker_padding_x(&theme))
        .py(DynamicSpacing::Base08.px(&theme))
        .child(header)
        .child(dots)
        .child(grid)
        .into_any_element()
}

/// The hover-revealed view control on the sidebar's header. Opens the
/// sidebar-options menu anchored below: how sessions are grouped, ordered,
/// and filtered (each active choice checked).
pub(crate) fn sidebar_options_button(
    group_by: SidebarGroupBy,
    sort: WorkspaceSort,
    archived_filter: SidebarArchivedFilter,
    open: bool,
    this: Entity<OrbitApp>,
    theme: Theme,
) -> impl IntoElement + use<> {
    let this_for_popup = this.clone();
    icon_button_frame(div().id("sidebar-options"), &theme, ButtonSize::Compact)
        .relative()
        .tip(tr!("sidebar.options"))
        .cursor_pointer()
        // Always visible: the sidebar's one view control, not a hover reveal.
        .when(open, |button| button.bg(theme.surface_active))
        .hover(|s| s.bg(theme.surface_hover))
        .active(|s| s.opacity(PRESS_DIM))
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            this.update(cx, |app, cx| app.toggle_sidebar_sort_menu(window, cx));
        })
        .child(icon(
            "icons/sort.svg",
            ButtonSize::Compact.icon_size().px(&theme),
            theme.text_muted,
        ))
        .children(open.then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size(px(0.))
                .child(sidebar_options_popup(
                    group_by,
                    sort,
                    archived_filter,
                    this_for_popup,
                    theme,
                ))
        }))
}

/// One row of the sidebar-options menu, on the picker metrics the branch
/// selector uses: a leading glyph, the label, and a trailing check when the
/// choice is active. The closure is the action the row runs.
fn sidebar_choice_item<C>(
    id: &'static str,
    icon_path: &'static str,
    label: String,
    active: bool,
    this: Entity<OrbitApp>,
    theme: Theme,
    on_click: C,
) -> impl IntoElement
where
    C: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
{
    picker_entry(div().id(id), &theme)
        .h(picker::entry_height(&theme))
        .flex_none()
        .cursor_pointer()
        .when(active, |row| {
            row.bg(theme.selection).text_color(theme.selection_fg)
        })
        .when(!active, |row| row.hover(|s| s.bg(theme.surface_hover)))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            this.update(cx, |app, cx| on_click(app, cx));
        })
        .child(icon(
            icon_path,
            context_menu::ICON.px(&theme),
            if active {
                theme.text_primary
            } else {
                theme.text_muted
            },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(if active {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .child(label),
        )
        .when(active, |row| {
            row.child(icon(
                "icons/check.svg",
                context_menu::ICON.px(&theme),
                theme.accent_text,
            ))
        })
}

/// Width of the sidebar-options popover. Wider than the branch picker's
/// compact width so the long "All conversations (show archived)" row reads
/// without truncating.
const OPTIONS_POPOVER_W: f32 = 300.;

/// The sidebar-options menu, anchored below the header control. Uses the
/// picker surface (search-less) and `picker_entry` rows so it matches the
/// branch selector; an outside mouse-down dismisses it. Three sections:
/// Group by, Order by, and Filter sessions.
pub(crate) fn sidebar_options_popup(
    group_by: SidebarGroupBy,
    sort: WorkspaceSort,
    archived_filter: SidebarArchivedFilter,
    this: Entity<OrbitApp>,
    theme: Theme,
) -> AnyElement {
    let popup = picker_surface(div(), &theme)
        .w(px(OPTIONS_POPOVER_W))
        .flex()
        .flex_col()
        .overflow_hidden()
        .occlude()
        .on_mouse_down_out({
            let this = this.clone();
            move |_, _, cx| {
                this.update(cx, |app, cx| {
                    // Arm the gesture guard so this same click's mouse-up
                    // cannot immediately re-open the menu.
                    app.menu_dismissed_at = Some(Instant::now());
                    app.sidebar_sort_menu = false;
                    cx.notify();
                })
            }
        })
        .child(
            div()
                .py(picker::list_padding_y(&theme))
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base01.px(&theme))
                .child(menu_header(tr!("sidebar.group_by"), &theme))
                .children(SidebarGroupBy::ALL.into_iter().map(|option| {
                    sidebar_choice_item(
                        option.as_str(),
                        option.icon_path(),
                        tr!(option.label_key()),
                        option == group_by,
                        this.clone(),
                        theme,
                        move |app, cx| app.set_sidebar_group_by(option, cx),
                    )
                }))
                .child(menu_header(tr!("sidebar.order_by"), &theme))
                .children(WorkspaceSort::MENU.into_iter().map(|option| {
                    sidebar_choice_item(
                        option.as_str(),
                        option.icon_path(),
                        tr!(option.label_key()),
                        option == sort,
                        this.clone(),
                        theme,
                        move |app, cx| app.set_workspace_sort(option, cx),
                    )
                }))
                .child(menu_header(tr!("sidebar.filter_sessions"), &theme))
                .children(SidebarArchivedFilter::ALL.into_iter().map(|option| {
                    sidebar_choice_item(
                        option.as_str(),
                        option.icon_path(),
                        tr!(option.label_key()),
                        option == archived_filter,
                        this.clone(),
                        theme,
                        move |app, cx| app.set_sidebar_archived_filter(option, cx),
                    )
                })),
        );

    anchored()
        .position_mode(AnchoredPositionMode::Local)
        .anchor(Corner::TopLeft)
        .offset(point(
            px(0.),
            ButtonSize::Compact.height(&theme) + popover::MENU_OFFSET,
        ))
        .snap_to_window_with_margin(popover::WINDOW_MARGIN)
        .child(deferred(popup))
        .into_any_element()
}

/// One entry of the sidebar's session / workspace menus, on Zed's context
/// menu metrics ([`context_menu_entry`]).
pub(crate) fn menu_item<C, L>(
    id: &'static str,
    icon_path: &'static str,
    label: L,
    theme: Theme,
    this: Entity<OrbitApp>,
    danger: bool,
    on_click: C,
) -> impl IntoElement + use<C, L>
where
    C: Fn(&mut OrbitApp, &mut Context<OrbitApp>) + 'static,
    L: Into<SharedString>,
{
    let label: SharedString = label.into();
    let on_click = on_click;
    let (hover_bg, text_color, icon_color) = if danger {
        (theme.error_hover, theme.error_fg, theme.error_fg)
    } else {
        (theme.overlay, theme.text_secondary, theme.text_muted)
    };
    picker_entry(div().id(id), &theme)
        .h(picker::entry_height(&theme))
        .flex_none()
        .cursor_pointer()
        .when(danger, |s| s.bg(theme.error))
        .hover(move |s| s.bg(hover_bg))
        .text_color(text_color)
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            this.update(cx, |app, cx| (on_click)(app, cx));
        })
        .child(icon(icon_path, context_menu::ICON.px(&theme), icon_color))
        .child(div().flex_1().min_w_0().truncate().child(label.to_string()))
}

/// Reveal a session file in the OS file manager (macOS first, matching the
/// product's platform stance; other platforms get the containing folder).
pub(crate) fn reveal_in_file_manager(path: PathBuf) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(not(target_os = "macos"))]
    if let Some(dir) = path.parent() {
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
        let _ = dir;
    }
}

/// Empty-state panel for a fresh pi store: what this space is for and how
/// to fill it. No cards, no illustration — one calm statement.
pub(crate) fn empty_sessions_state(theme: Theme) -> impl IntoElement + use<> {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .px(DynamicSpacing::Base20.px(&theme))
        .pb(DynamicSpacing::Base40.px(&theme))
        .child(
            div()
                .size(px(36.))
                .rounded_full()
                .bg(theme.surface_elevated)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(
                    "icons/spark.svg",
                    IconSize::Medium.px(&theme),
                    theme.accent_text,
                )),
        )
        .child(
            div()
                .text_size(TextSize::Default.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_secondary)
                .child(tr!("sidebar.no_projects_yet")),
        )
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .text_align(TextAlign::Center)
                .child(tr!("sidebar.pick_a_folder_to_start_your_first_task")),
        )
}

// ── controller ────────────────────────────────────────────────────
impl OrbitApp {
    /// Open (or toggle closed) the row-actions popup for a session row.
    pub(super) fn toggle_session_menu(
        &mut self,
        menu: SessionMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Same-click dismissal must not re-open (see toggle_session_picker).
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        if self.session_menu.as_ref() == Some(&menu) {
            self.session_menu = None;
            cx.notify();
            return;
        }
        self.dismiss_transient_popups(window, cx);
        self.workspace_menu = None;
        self.session_menu = Some(menu);
        cx.notify();
    }

    /// Open the session row menu from a right-click, floating it at the
    /// pointer ([`SessionMenu::at`]). Unlike the `…` button's toggle this
    /// always (re)positions the menu — a right-click while another row's menu
    /// is up moves it to the new row instead of dismissing — and skips the
    /// gesture guard, which only exists for the left-click dismiss/reopen
    /// race.
    pub(super) fn open_session_menu_at(
        &mut self,
        menu: SessionMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transient_popups(window, cx);
        self.workspace_menu = None;
        self.session_menu = Some(menu);
        cx.notify();
    }

    /// Dismiss window-level overlays (palette, pickers, popup selectors) so a
    /// row menu never opens underneath one — every open path above routes
    /// through here.
    fn dismiss_transient_popups(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette.take().is_some() {
            self.input.read(cx).focus(window);
        }
        if self.branch_picker.take().is_some() {
            self.input.read(cx).focus(window);
        }
        if self.workspace_picker.take().is_some() {
            self.input.read(cx).focus(window);
        }
        if self.model_selector.is_some() {
            self.close_model_selector(window, cx);
        }
        self.sidebar_sort_menu = false;
    }

    pub(super) fn on_menu_copy_path(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &self.session_menu {
            cx.write_to_clipboard(ClipboardItem::new_string(
                menu.path.to_string_lossy().into_owned(),
            ));
        }
        self.session_menu = None;
        cx.notify();
    }

    pub(super) fn on_menu_reveal(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &self.session_menu {
            reveal_in_file_manager(menu.path.clone());
        }
        self.session_menu = None;
        cx.notify();
    }

    /// Duplicate the session the open row menu belongs to, so it shows up as
    /// its own row to branch off. The source is only read, so — unlike Delete —
    /// this is safe for a session with a live pi process.
    pub(super) fn on_menu_clone_session(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.session_menu.take() else {
            return;
        };
        match sessions::clone_session_file(&menu.path) {
            Ok(_) => {
                self.sessions = sessions::load_sessions();
                self.prune_workflow_store();
            }
            Err(err) => self.toast_error(tr!("sidebar.clone_failed", error = err)),
        }
        cx.notify();
    }

    /// Pin or unpin the session the open row menu belongs to. The pin lives
    /// in Orbit's own store (`~/.orbit-pi/pinned-sessions.json`); pi's
    /// session file is never touched.
    pub(super) fn on_menu_toggle_pin(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.session_menu.take() {
            crate::pins::toggle(&menu.path);
        }
        cx.notify();
    }

    /// Archive or unarchive the session whose row menu is open, then re-run
    /// the archived filter so the row appears or leaves per the view choice.
    pub(super) fn on_menu_toggle_archive(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.session_menu.take() {
            crate::archive::toggle(&menu.path);
        }
        self.clamp_sidebar_cursor(cx);
        cx.notify();
    }

    /// First Delete click: swap the popup to the confirmation state.
    pub(super) fn on_menu_delete_request(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.session_menu.as_mut() {
            menu.confirm_delete = true;
            cx.notify();
        }
    }

    /// Confirmed: remove the session file from disk and refresh the list.
    pub(super) fn on_menu_delete_confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.session_menu.take() {
            // Defensive: a warm parked process would recreate the file.
            if self.lives.contains_key(&menu.path) {
                self.toast_warning(tr!("sidebar.live_process_delete"));
                cx.notify();
                return;
            }
            if let Err(err) = fs::remove_file(&menu.path) {
                self.toast_error(tr!("sidebar.delete_failed", error = err));
            } else {
                // A deleted session must not leave a stale pin behind.
                crate::pins::remove(&menu.path);
                crate::archive::remove(&menu.path);
            }
            self.sessions = sessions::load_sessions();
            self.prune_workflow_store();
            cx.notify();
        }
    }

    pub(super) fn on_menu_cancel(&mut self, cx: &mut Context<Self>) {
        self.session_menu = None;
        cx.notify();
    }

    /// Open (or toggle closed) the sidebar's workspace-sort menu. Follows the
    /// same gesture guard as the row menus, so the dismissing click cannot
    /// immediately re-open it.
    pub(super) fn toggle_sidebar_sort_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        if self.sidebar_sort_menu {
            self.sidebar_sort_menu = false;
            cx.notify();
            return;
        }
        self.dismiss_transient_popups(window, cx);
        self.session_menu = None;
        self.workspace_menu = None;
        self.sidebar_sort_menu = true;
        cx.notify();
    }

    /// Switch the workspace ordering and persist the choice with the project
    /// list. Re-sorting is pure UI: pi's session files are never touched.
    pub(super) fn set_workspace_sort(&mut self, sort: WorkspaceSort, cx: &mut Context<Self>) {
        self.sidebar_sort_menu = false;
        if self.workspace_sort != sort {
            self.workspace_sort = sort;
            self.persist_workspace_prefs();
        }
        cx.notify();
    }

    /// Choose how the sidebar arranges sessions, persisting the choice beside
    /// the project list. Regrouping is pure UI: pi's session files are never
    /// touched.
    pub(super) fn set_sidebar_group_by(
        &mut self,
        group_by: SidebarGroupBy,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_sort_menu = false;
        if self.sidebar_group_by != group_by {
            self.sidebar_group_by = group_by;
            self.persist_workspace_prefs();
        }
        self.clamp_sidebar_cursor(cx);
        cx.notify();
    }

    /// Choose which archived sessions the sidebar lists and persist it.
    pub(super) fn set_sidebar_archived_filter(
        &mut self,
        filter: SidebarArchivedFilter,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_sort_menu = false;
        if self.sidebar_archived_filter != filter {
            self.sidebar_archived_filter = filter;
            self.persist_workspace_prefs();
        }
        self.clamp_sidebar_cursor(cx);
        cx.notify();
    }

    /// Open (or toggle closed) the row-actions popup for a workspace header.
    pub(super) fn toggle_workspace_menu(
        &mut self,
        menu: WorkspaceMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Same-click dismissal must not re-open (see toggle_session_menu).
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        if self.workspace_menu.as_ref() == Some(&menu) {
            self.workspace_menu = None;
            cx.notify();
            return;
        }
        self.dismiss_transient_popups(window, cx);
        self.session_menu = None;
        self.workspace_menu = Some(menu);
        cx.notify();
    }

    /// Open the workspace header menu from a right-click, floating it at the
    /// pointer ([`WorkspaceMenu::at`]). See [`Self::open_session_menu_at`] for
    /// why this bypasses the toggle's gesture guard and equality check.
    pub(super) fn open_workspace_menu_at(
        &mut self,
        menu: WorkspaceMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transient_popups(window, cx);
        self.session_menu = None;
        self.workspace_menu = Some(menu);
        cx.notify();
    }

    pub(super) fn on_workspace_copy_path(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &self.workspace_menu {
            cx.write_to_clipboard(ClipboardItem::new_string(
                menu.cwd.to_string_lossy().into_owned(),
            ));
        }
        self.workspace_menu = None;
        cx.notify();
    }

    /// Swap the open workspace menu for its icon/tint picker, in place.
    pub(super) fn on_workspace_open_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.workspace_menu.as_mut() {
            menu.picker = true;
        }
        cx.notify();
    }

    /// Remove the workspace the open header menu belongs to from Orbit's own
    /// project list. pi's session files are never touched — the folder can be
    /// added again by picking it to work in.
    pub(super) fn on_workspace_remove(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.workspace_menu.take() else {
            return;
        };
        self.remove_workspace(&menu.cwd, cx);
        // Forget the dropped group's view state so re-adding it starts fresh.
        self.collapsed_workspaces.remove(&menu.label);
        self.expanded_workspace_groups.remove(&menu.label);
        self.expanded_session_groups.remove(&menu.label);
        cx.notify();
    }

    /// Keep the row menu honest: close it when its session vanishes from
    /// the store (deleted externally, session ended, …).
    pub(super) fn sync_session_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &self.session_menu {
            if !menu.path.exists() {
                self.session_menu = None;
                cx.notify();
            }
        }
    }

    // ── keyboard navigation ───────────────────────────────────────────

    /// The sidebar rows as currently laid out — grouped by workspace, or one
    /// flat list when grouping is off. Shared by the render path and keyboard
    /// navigation so the cursor can never point at a row the user cannot see.
    pub(super) fn sidebar_rows(
        &self,
        sessions: &[SessionInfo],
        pinned: &HashSet<PathBuf>,
    ) -> Vec<SideRow> {
        let working = self.workspace_label();
        let running: HashSet<PathBuf> = self
            .lives
            .iter()
            .filter(|(_, parked)| parked.busy)
            .map(|(path, _)| path.clone())
            .collect();
        match self.sidebar_group_by {
            SidebarGroupBy::OneList => {
                build_flat_sidebar_rows(sessions, pinned, &self.current_session_path, &running)
            }
            SidebarGroupBy::Workspace => build_sidebar_rows(
                sessions,
                &self.workspaces,
                self.workspace_sort,
                &self.workspace_added_at,
                &working,
                &self.collapsed_workspaces,
                &self.expanded_workspace_groups,
                &self.expanded_session_groups,
                pinned,
                &self.current_session_path,
                &running,
            ),
        }
    }

    /// The sidebar's session list with the archived filter applied — the same
    /// list the render path paints, so keyboard navigation stays in step.
    pub(super) fn sidebar_sessions_filtered(&self) -> Vec<SessionInfo> {
        let sessions = self.sidebar_sessions();
        let archived: HashSet<PathBuf> = crate::archive::all().paths();
        filter_archived(sessions, self.sidebar_archived_filter, &archived)
    }

    /// The sidebar rows with the pinned snapshot read fresh — the same
    /// grouping, ordering, and collapse state the render path uses.
    fn sidebar_rows_for_nav(&self) -> Vec<SideRow> {
        let sessions = self.sidebar_sessions_filtered();
        let pinned: HashSet<PathBuf> = crate::pins::all().paths();
        self.sidebar_rows(&sessions, &pinned)
    }

    /// Row index of the open session in `rows`, when it is visible.
    fn active_session_row(&self, rows: &[SideRow]) -> Option<usize> {
        let sessions = self.sidebar_sessions();
        let active = self.current_session_path.as_ref()?;
        let session_ix = sessions
            .iter()
            .position(|session| &session.path == active)?;
        rows.iter()
            .position(|row| matches!(row, SideRow::Session(ix) if *ix == session_ix))
    }

    /// ⌘⇧B: reveal the sidebar if hidden, then focus it and place the cursor
    /// on the open session (or the first row).
    pub(super) fn on_focus_sessions(
        &mut self,
        _: &crate::FocusSessions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Settings is a full-window surface with its own nav; the sidebar is
        // not mounted, so there is nothing to focus.
        if self.settings_open {
            return;
        }
        if !self.sidebar_visible {
            self.sidebar_visible = true;
            self.sidebar_slide_gen = self.sidebar_slide_gen.wrapping_add(1);
        }
        let rows = self.sidebar_rows_for_nav();
        self.sidebar_cursor =
            (!rows.is_empty()).then(|| self.active_session_row(&rows).unwrap_or(0));
        window.focus(&self.sidebar_focus);
        cx.notify();
    }

    /// Visible sessions in the order the sidebar navigates them — the
    /// numbering `⌘1…⌘9` and session cycling share.
    pub(super) fn navigable_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sidebar_sessions();
        self.sidebar_rows_for_nav()
            .iter()
            .filter_map(|row| match row {
                SideRow::Session(ix) => sessions.get(*ix).cloned(),
                _ => None,
            })
            .collect()
    }

    /// `⌘1…⌘9`: open the Nth session the sidebar shows. A slot past the end
    /// is a no-op, so a stray number never switches the session by accident.
    pub(super) fn open_session_slot(&mut self, slot: usize, cx: &mut Context<Self>) {
        if slot == 0 {
            return;
        }
        let Some(session) = self.navigable_sessions().into_iter().nth(slot - 1) else {
            return;
        };
        self.on_open_session(session, cx);
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab: cycle through the visible sessions, wrapping
    /// at both ends. With a single session there is nowhere to go.
    pub(super) fn cycle_session(&mut self, direction: isize, cx: &mut Context<Self>) {
        let sessions = self.navigable_sessions();
        if sessions.len() < 2 {
            return;
        }
        let current = self
            .current_session_path
            .as_ref()
            .and_then(|path| sessions.iter().position(|session| &session.path == path));
        let next = match current {
            Some(ix) => (ix as isize + direction).rem_euclid(sessions.len() as isize) as usize,
            None if direction > 0 => 0,
            None => sessions.len() - 1,
        };
        self.on_open_session(sessions[next].clone(), cx);
    }

    /// Action handler for ⌘1…⌘9.
    pub(super) fn on_open_session_slot(
        &mut self,
        action: &crate::OpenSessionSlot,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_session_slot(action.slot, cx);
    }

    /// Action handler for Ctrl+Tab.
    pub(super) fn on_next_session(
        &mut self,
        _: &crate::NextSession,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_session(1, cx);
    }

    /// Action handler for Ctrl+Shift+Tab.
    pub(super) fn on_prev_session(
        &mut self,
        _: &crate::PrevSession,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_session(-1, cx);
    }

    pub(super) fn on_sidebar_prev(
        &mut self,
        _: &crate::SidebarPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_sidebar_cursor(|ix| ix.saturating_sub(1), cx);
    }

    pub(super) fn on_sidebar_next(
        &mut self,
        _: &crate::SidebarNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_sidebar_cursor(|ix| ix.saturating_add(1), cx);
    }

    pub(super) fn on_sidebar_home(
        &mut self,
        _: &crate::SidebarHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_sidebar_cursor(|_| 0, cx);
    }

    pub(super) fn on_sidebar_end(
        &mut self,
        _: &crate::SidebarEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_sidebar_cursor(|_| usize::MAX, cx);
    }

    /// Move the cursor by `step`, clamped to the visible rows, and scroll it
    /// into view.
    fn move_sidebar_cursor(&mut self, step: impl Fn(usize) -> usize, cx: &mut Context<Self>) {
        let count = self.sidebar_rows_for_nav().len();
        self.sidebar_cursor = clamp_cursor(step(self.sidebar_cursor.unwrap_or(0)), count);
        if let Some(next) = self.sidebar_cursor {
            self.sidebar_list.scroll_to_reveal_item(next);
        }
        cx.notify();
    }

    /// Enter on the cursor row: open a session (returning focus to the
    /// composer), toggle a workspace group, or reveal/collapse a group's
    /// overflow — the same actions the row's click affordances perform.
    pub(super) fn on_sidebar_confirm(
        &mut self,
        _: &crate::SidebarConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.sidebar_cursor else {
            return;
        };
        let rows = self.sidebar_rows_for_nav();
        let Some(row) = rows.get(index) else {
            return;
        };
        match row {
            SideRow::Session(ix) => {
                let sessions = self.sidebar_sessions();
                if let Some(session) = sessions.get(*ix).cloned() {
                    self.on_open_session(session, cx);
                }
                // The row is a destination: hand focus back to the composer
                // so typing resumes immediately.
                self.sidebar_cursor = None;
                self.input.read(cx).focus(window);
            }
            SideRow::Workspace { label, .. } => {
                let working = self.workspace_label();
                toggle_workspace_group(
                    label.clone(),
                    &working,
                    &mut self.collapsed_workspaces,
                    &mut self.expanded_workspace_groups,
                );
                self.clamp_sidebar_cursor(cx);
            }
            SideRow::ShowMore { label, count, .. } => {
                self.expanded_session_groups
                    .entry(label.clone())
                    .and_modify(|extra| *extra += count)
                    .or_insert(*count);
                self.clamp_sidebar_cursor(cx);
            }
            SideRow::ShowLess { label } => {
                self.expanded_session_groups.remove(label);
                self.clamp_sidebar_cursor(cx);
            }
        }
        cx.notify();
    }

    /// Esc: dismiss an open row menu first, otherwise leave keyboard
    /// navigation and hand focus back to the composer.
    pub(super) fn on_sidebar_close(
        &mut self,
        _: &crate::SidebarClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_menu.take().is_some()
            || self.workspace_menu.take().is_some()
            || self.sidebar_sort_menu
        {
            self.sidebar_sort_menu = false;
            cx.notify();
            return;
        }
        self.sidebar_cursor = None;
        self.input.read(cx).focus(window);
        cx.notify();
    }

    /// The command palette's Delete Session: reveal the sidebar on the active
    /// session's row and arm the same confirmation the row menu shows, so the
    /// destructive step still needs an explicit confirm.
    pub(super) fn arm_active_session_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.current_session_path.clone() else {
            return;
        };
        let sessions = self.sidebar_sessions();
        let Some(session_ix) = sessions.iter().position(|session| session.path == path) else {
            return;
        };
        let session = sessions[session_ix].clone();
        if !self.sidebar_visible {
            self.sidebar_visible = true;
            self.sidebar_slide_gen = self.sidebar_slide_gen.wrapping_add(1);
        }
        let rows = self.sidebar_rows_for_nav();
        self.sidebar_cursor = rows
            .iter()
            .position(|row| matches!(row, SideRow::Session(ix) if *ix == session_ix));
        if let Some(ix) = self.sidebar_cursor {
            self.sidebar_list.scroll_to_reveal_item(ix);
        }
        self.session_menu = Some(SessionMenu {
            path: session.path.clone(),
            title: session.title,
            deletable: !self.lives.contains_key(&session.path),
            confirm_delete: true,
            at: None,
        });
        window.focus(&self.sidebar_focus);
        cx.notify();
    }

    /// Keep the cursor inside the rows after one collapses the list under it.
    fn clamp_sidebar_cursor(&mut self, cx: &mut Context<Self>) {
        let count = self.sidebar_rows_for_nav().len();
        self.sidebar_cursor = self.sidebar_cursor.and_then(|ix| clamp_cursor(ix, count));
        cx.notify();
    }
}

/// Clamp a cursor position into `count` rows: `None` when there are none, the
/// nearest valid index otherwise.
fn clamp_cursor(index: usize, count: usize) -> Option<usize> {
    (count > 0).then(|| index.min(count - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_clamps_to_the_visible_rows() {
        assert_eq!(clamp_cursor(0, 0), None);
        assert_eq!(clamp_cursor(3, 2), Some(1));
        assert_eq!(clamp_cursor(1, 5), Some(1));
    }

    /// The row keeps its second line only when the preview diverges from the
    /// title. pi's auto-title is the first message, so the repeat is the common
    /// case, not the edge.
    #[test]
    fn a_preview_that_only_continues_the_title_is_not_worth_a_line() {
        // pi's title is the message cut short: same sentence, nothing added.
        assert!(!preview_adds_information(
            "can you update the screenshot of the",
            "can you update the screenshot of the app on the marketing page",
        ));
        // The placeholder case: title and preview are the same string.
        assert!(!preview_adds_information(
            "Explain the project structure.",
            "Explain the project structure.",
        ));
        // A capped title ends with an ellipsis and the preview does not; that
        // difference alone must not buy a second line.
        assert!(!preview_adds_information(
            "Fixing the OAuth redirect loop…",
            "Fixing the OAuth redirect loop in the callback handler",
        ));
        // `cap_chars` cuts mid-word, so the title's last word is a fragment of
        // the preview's. The fragment is the cap's, not the sentence's.
        assert!(!preview_adds_information(
            "can you update the screenshot of the ap…",
            "can you update the screenshot of the app on the marketing page",
        ));
        // Collapsed whitespace and a newline are formatting, not content.
        assert!(!preview_adds_information(
            "Tidy the release notes",
            "Tidy the   release\nnotes before tagging",
        ));
    }

    #[test]
    fn a_preview_that_says_something_new_keeps_its_line() {
        // pi named the session itself: the message is new information.
        assert!(preview_adds_information(
            "Fix login bug",
            "the oauth redirect is looping",
        ));
        // Sharing an opening word is not repeating a sentence.
        assert!(preview_adds_information(
            "Fix the login flow",
            "The oauth redirect is looping in production",
        ));
        // No preview at all is nothing to show.
        assert!(!preview_adds_information("Fix login bug", "   "));
        assert!(!preview_adds_information("Fix login bug", ""));
        // A session with no title yet has nothing to repeat.
        assert!(preview_adds_information("", "anything at all"));
    }
}
