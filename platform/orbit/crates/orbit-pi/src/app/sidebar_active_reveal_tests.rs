use super::sidebar::*;
use super::*;

use gpui::ListOffset;

fn store_session(name: &str, cwd: &str) -> SessionInfo {
    SessionInfo {
        path: PathBuf::from(format!("/store/{name}.jsonl")),
        id: name.into(),
        cwd: PathBuf::from(cwd),
        title: format!("{name} title"),
        first_message: "preview".into(),
        modified: SystemTime::UNIX_EPOCH,
    }
}

fn session_row_paths(rows: &[SideRow], sessions: &[SessionInfo]) -> Vec<PathBuf> {
    rows.iter()
        .filter_map(|r| match r {
            SideRow::Session(ix) => Some(sessions[*ix].path.clone()),
            _ => None,
        })
        .collect()
}

fn workspace_paths(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[test]
fn collapsed_workspace_pins_the_open_session() {
    // "alpha" is the working workspace (expanded by default); "beta" is
    // a foreign workspace, collapsed unless explicitly expanded. It
    // holds the open session: the group stays collapsed (siblings
    // hidden) but the open session's row stays pinned under the header.
    let sessions = vec![
        store_session("a1", "/work/alpha"),
        store_session("b1", "/work/beta"),
        store_session("b2", "/work/beta"),
    ];
    let active = Some(sessions[1].path.clone());
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha", "/work/beta"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &active,
        &HashSet::new(),
    );
    let visible = session_row_paths(&rows, &sessions);
    assert!(
        visible.contains(&sessions[1].path),
        "the open session stays visible in a collapsed workspace"
    );
    assert!(
        !visible.contains(&sessions[2].path),
        "its siblings stay hidden while the group is collapsed"
    );
    let beta_header = rows.iter().find_map(|r| match r {
        SideRow::Workspace {
            label, collapsed, ..
        } if label == "beta" => Some(*collapsed),
        _ => None,
    });
    assert_eq!(
        beta_header,
        Some(true),
        "the group header still reads as collapsed"
    );
}

#[test]
fn collapsed_workspace_keeps_a_running_session_visible() {
    // A background session mid-run is live work. Collapsing its workspace
    // keeps that one row under the header — like the open session — while
    // its idle siblings disappear.
    let sessions = vec![
        store_session("a1", "/work/alpha"),
        store_session("b1", "/work/beta"),
        store_session("b2", "/work/beta"),
    ];
    let running: HashSet<PathBuf> = HashSet::from([sessions[2].path.clone()]);
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha", "/work/beta"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &running,
    );
    let visible = session_row_paths(&rows, &sessions);
    assert!(
        visible.contains(&sessions[2].path),
        "a running session stays visible in a collapsed workspace"
    );
    assert!(
        !visible.contains(&sessions[1].path),
        "its idle siblings stay hidden while the group is collapsed"
    );
}

#[test]
fn collapsed_workspace_without_the_open_session_stays_closed() {
    let sessions = vec![
        store_session("a1", "/work/alpha"),
        store_session("b1", "/work/beta"),
    ];
    let active = Some(sessions[0].path.clone());
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha", "/work/beta"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &active,
        &HashSet::new(),
    );
    assert!(
        !session_row_paths(&rows, &sessions).contains(&sessions[1].path),
        "a collapsed foreign workspace keeps its sessions hidden"
    );
}

#[test]
fn manually_collapsed_working_workspace_pins_the_open_session() {
    // The user collapsed their own workspace by hand — the open session
    // still shows under the header; expanding brings back the rest.
    let sessions = vec![
        store_session("a1", "/work/alpha"),
        store_session("a2", "/work/alpha"),
    ];
    let active = Some(sessions[0].path.clone());
    let mut collapsed = HashSet::new();
    collapsed.insert("alpha".to_string());
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &collapsed,
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &active,
        &HashSet::new(),
    );
    let visible = session_row_paths(&rows, &sessions);
    assert!(
        visible.contains(&sessions[0].path),
        "the open session stays visible even when its workspace was collapsed by hand"
    );
    assert!(
        !visible.contains(&sessions[1].path),
        "the other sessions stay hidden until the header is expanded"
    );
}

#[test]
fn unlisted_workspace_stays_out_of_the_sidebar() {
    // Orbit owns the project list: a folder pi has sessions in is omitted
    // until the user adds it. Its sessions stay on disk, untouched.
    let sessions = vec![
        store_session("a1", "/work/alpha"),
        store_session("b1", "/work/beta"),
    ];
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    assert!(
        !rows
            .iter()
            .any(|r| matches!(r, SideRow::Workspace { label, .. } if label == "beta")),
        "an unlisted workspace leaves no header behind"
    );
    assert!(
        !session_row_paths(&rows, &sessions).contains(&sessions[1].path),
        "an unlisted workspace's sessions stay out of the sidebar"
    );
}

#[test]
fn listed_workspace_without_sessions_gets_a_header() {
    // A project with no sessions yet still lists, so a task can be started
    // there from its `+`.
    let sessions = vec![store_session("a1", "/work/alpha")];
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha", "/work/empty"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    assert!(
        rows.iter().any(|r| matches!(
            r,
            SideRow::Workspace { label, count, cwd, .. }
                if label == "empty" && *count == 0 && cwd == &PathBuf::from("/work/empty")
        )),
        "an empty listed workspace still gets a header with its cwd"
    );
}

#[test]
fn groups_follow_the_project_list_order() {
    // The sidebar is Orbit's own folder listing: groups keep the order the
    // user added them, not newest-session order.
    let sessions = vec![
        store_session("b1", "/work/beta"),
        store_session("a1", "/work/alpha"),
    ];
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha", "/work/beta"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "none",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    let labels: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            SideRow::Workspace { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["alpha", "beta"]);
}

#[test]
fn pinned_sessions_lead_their_group_and_beat_truncation() {
    // More sessions in the group than fit collapsed. The pinned session is
    // the oldest row, so without pinning it would be truncated away; the pin
    // must lift it to the top of its group and keep it visible, while the
    // rest stay in recency order.
    let sessions: Vec<SessionInfo> = (0..SIDEBAR_GROUP_SESSIONS_VISIBLE + 2)
        .map(|i| store_session(&format!("a{i}"), "/work/alpha"))
        .collect();
    let oldest = sessions.last().unwrap().path.clone();
    let pinned: HashSet<PathBuf> = HashSet::from([oldest.clone()]);
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &pinned,
        &None,
        &HashSet::new(),
    );
    let visible = session_row_paths(&rows, &sessions);
    assert_eq!(visible.first(), Some(&oldest), "the pin leads the group");
    assert_eq!(
        visible.len(),
        SIDEBAR_GROUP_SESSIONS_VISIBLE,
        "the group still shows its visible cap"
    );
    assert_eq!(
        visible[1], sessions[0].path,
        "unpinned rows keep recency order after the pin"
    );
}

#[test]
fn pinned_sessions_stay_visible_in_a_collapsed_workspace() {
    // Foreign projects are collapsed by default; a pin is a deliberate mark
    // and must survive that — under its header, alongside the open row.
    let sessions = vec![
        store_session("b1", "/work/beta"),
        store_session("b2", "/work/beta"),
    ];
    let pinned: HashSet<PathBuf> = HashSet::from([sessions[1].path.clone()]);
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha", "/work/beta"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &pinned,
        &None,
        &HashSet::new(),
    );
    let visible = session_row_paths(&rows, &sessions);
    assert_eq!(visible, vec![sessions[1].path.clone()]);
}

#[test]
fn show_more_reveals_one_step_at_a_time() {
    // A long history must never land in one go: each "Show more" click
    // reveals one step of ten. Past the base cap the same row carries the
    // collapse affordance, so the group never needs two toggle rows.
    let total = SIDEBAR_GROUP_SESSIONS_VISIBLE * 3;
    let sessions: Vec<SessionInfo> = (0..total)
        .map(|i| store_session(&format!("a{i}"), "/work/alpha"))
        .collect();
    let workspaces = workspace_paths(&["/work/alpha"]);

    // `(step, can_collapse)`: what one click reveals and whether the row
    // also offers the right-side collapse chevron.
    let show_more = |rows: &[SideRow]| -> Option<(usize, bool)> {
        rows.iter().find_map(|r| match r {
            SideRow::ShowMore {
                count,
                can_collapse,
                ..
            } => Some((*count, *can_collapse)),
            _ => None,
        })
    };
    let show_less =
        |rows: &[SideRow]| -> bool { rows.iter().any(|r| matches!(r, SideRow::ShowLess { .. })) };

    // Fresh group: the base cap, one step on offer, nothing to collapse.
    let rows = build_sidebar_rows(
        &sessions,
        &workspaces,
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    assert_eq!(
        session_row_paths(&rows, &sessions).len(),
        SIDEBAR_GROUP_SESSIONS_VISIBLE
    );
    assert_eq!(
        show_more(&rows),
        Some((SIDEBAR_GROUP_SESSIONS_VISIBLE, false))
    );
    assert!(!show_less(&rows), "nothing to collapse before expanding");

    // One click in: two steps visible, another step (not the rest) on offer,
    // and the same row now carries the collapse chevron.
    let expanded = HashMap::from([("alpha".to_string(), SIDEBAR_GROUP_SESSIONS_VISIBLE)]);
    let rows = build_sidebar_rows(
        &sessions,
        &workspaces,
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &expanded,
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    assert_eq!(
        session_row_paths(&rows, &sessions).len(),
        SIDEBAR_GROUP_SESSIONS_VISIBLE * 2
    );
    assert_eq!(
        show_more(&rows),
        Some((SIDEBAR_GROUP_SESSIONS_VISIBLE, true))
    );
    assert!(
        !show_less(&rows),
        "the collapse lives on the show-more row, not a second row"
    );

    // Fully expanded: the whole group, no step left, one quiet collapse row.
    let expanded = HashMap::from([("alpha".to_string(), total - SIDEBAR_GROUP_SESSIONS_VISIBLE)]);
    let rows = build_sidebar_rows(
        &sessions,
        &workspaces,
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &expanded,
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    assert_eq!(session_row_paths(&rows, &sessions).len(), total);
    assert_eq!(show_more(&rows), None);
    assert!(show_less(&rows));
}

#[test]
fn show_more_step_never_overshoots_the_tail() {
    // The last step is trimmed to the sessions actually left, so the stored
    // expansion can never push the group past its own count.
    let total = SIDEBAR_GROUP_SESSIONS_VISIBLE + 3;
    let sessions: Vec<SessionInfo> = (0..total)
        .map(|i| store_session(&format!("a{i}"), "/work/alpha"))
        .collect();
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    let step = rows.iter().find_map(|r| match r {
        SideRow::ShowMore { count, .. } => Some(*count),
        _ => None,
    });
    assert_eq!(step, Some(3), "the offer is capped at the rows that remain");
}

/// Rows for a two-workspace sidebar with both groups expanded: alpha's
/// header sits on row 0, beta's on row 3, each followed by two sessions.
fn sticky_rows() -> Vec<SideRow> {
    let sessions = vec![
        store_session("a1", "/work/alpha"),
        store_session("a2", "/work/alpha"),
        store_session("b1", "/work/beta"),
        store_session("b2", "/work/beta"),
    ];
    build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha", "/work/beta"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &HashSet::new(),
        &HashSet::from(["beta".to_string()]),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    )
}

/// Resolve the sticky header against a list scrolled to `(item_ix, offset)`.
fn sticky_at(rows: &[SideRow], item_ix: usize, offset: f32) -> Option<(usize, Pixels)> {
    let list = ListState::new(rows.len(), ListAlignment::Top, px(0.));
    list.scroll_to(ListOffset {
        item_ix,
        offset_in_item: px(offset),
    });
    sticky_sidebar_header(&list, rows).map(|s| (s.ix, s.top_offset))
}

#[test]
fn sticky_header_pins_the_scrolled_group() {
    let rows = sticky_rows();
    // Scrolled into alpha's own sessions, its header pins at the top.
    assert_eq!(sticky_at(&rows, 0, 5.), Some((0, px(0.))));
    assert_eq!(sticky_at(&rows, 1, 0.), Some((0, px(0.))));
    assert_eq!(sticky_at(&rows, 2, 0.), Some((0, px(0.))));
    // Exactly at the top the real header already sits there — no overlay.
    assert_eq!(sticky_at(&rows, 0, 0.), None);
    // Once beta's header takes the top, alpha's header scrolls away and
    // beta's pins in its place.
    assert_eq!(sticky_at(&rows, 3, 0.), None);
    assert_eq!(sticky_at(&rows, 3, 2.), Some((3, px(0.))));
    assert_eq!(sticky_at(&rows, 4, 0.), Some((3, px(0.))));
}

#[test]
fn sticky_header_pins_a_non_active_expanded_group() {
    // The working workspace is alpha, but the reader scrolled beta's long
    // list open. Beta's header must pin too — the old lookup only pinned
    // the active group, which made the first workspace look uniquely sticky.
    let rows = sticky_rows();
    assert_eq!(sticky_at(&rows, 4, 0.), Some((3, px(0.))));
}

#[test]
fn sticky_header_skips_collapsed_groups() {
    // A collapsed group shows only its always-visible rows, so there is no
    // long history to label and nothing pins.
    let sessions = vec![store_session("a1", "/work/alpha")];
    let collapsed = HashSet::from(["alpha".to_string()]);
    let rows = build_sidebar_rows(
        &sessions,
        &workspace_paths(&["/work/alpha"]),
        WorkspaceSort::Manual,
        &HashMap::new(),
        "alpha",
        &collapsed,
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    );
    assert_eq!(sticky_at(&rows, 0, 10.), None);
    // An empty list has no header to pin.
    assert_eq!(sticky_at(&[], 0, 10.), None);
}
