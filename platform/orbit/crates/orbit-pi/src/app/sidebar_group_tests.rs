use super::sidebar::*;
use super::*;

fn store_session(name: &str, cwd: &str, modified_secs: u64) -> SessionInfo {
    SessionInfo {
        path: PathBuf::from(format!("/store/{name}.jsonl")),
        id: name.into(),
        cwd: PathBuf::from(cwd),
        title: format!("{name} title"),
        first_message: "preview".into(),
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(modified_secs),
    }
}

fn session_indices(rows: &[SideRow]) -> Vec<usize> {
    rows.iter()
        .filter_map(|row| match row {
            SideRow::Session(ix) => Some(*ix),
            _ => None,
        })
        .collect()
}

#[test]
fn flat_rows_hold_every_session_without_headers() {
    let sessions = vec![
        store_session("a1", "/work/alpha", 30),
        store_session("b1", "/work/beta", 20),
        store_session("a2", "/work/alpha", 10),
    ];
    let rows = build_flat_sidebar_rows(&sessions, &HashSet::new(), &None, &HashSet::new());

    // Every session lands as a row, in the store's order, and no group
    // headers, show-more, or collapse rows appear.
    assert_eq!(session_indices(&rows), vec![0, 1, 2]);
    assert!(rows.iter().all(|row| matches!(row, SideRow::Session(_))));
}

#[test]
fn flat_rows_lead_with_pinned_sessions() {
    let sessions = vec![
        store_session("a1", "/work/alpha", 30),
        store_session("b1", "/work/beta", 20),
        store_session("a2", "/work/alpha", 10),
    ];
    let pinned: HashSet<PathBuf> = [PathBuf::from("/store/a2.jsonl")].into_iter().collect();
    let rows = build_flat_sidebar_rows(&sessions, &pinned, &None, &HashSet::new());

    // The pinned session rises to the top; the rest keep newest-first order.
    assert_eq!(session_indices(&rows), vec![2, 0, 1]);
}

#[test]
fn flat_rows_keep_the_open_session_first_while_a_run_writes() {
    // The open session is the older of the two and the other has a live
    // background run with newer activity. If both merely "led", they would
    // trade places on every watcher reload as each appends a message; the
    // open session must outrank the run.
    let sessions = vec![
        store_session("active", "/work/alpha", 10),
        store_session("background", "/work/alpha", 30),
    ];
    let running: HashSet<PathBuf> = [sessions[1].path.clone()].into_iter().collect();
    let active = Some(sessions[0].path.clone());
    let rows = build_flat_sidebar_rows(&sessions, &HashSet::new(), &active, &running);
    assert_eq!(session_indices(&rows), vec![0, 1]);
}

#[test]
fn flat_rows_are_empty_without_sessions() {
    let rows = build_flat_sidebar_rows(&[], &HashSet::new(), &None, &HashSet::new());
    assert!(rows.is_empty());
}

#[test]
fn archived_filter_hides_shows_or_limits() {
    let sessions = vec![
        store_session("a1", "/work/alpha", 30),
        store_session("b1", "/work/beta", 20),
    ];
    let archived: HashSet<PathBuf> = [PathBuf::from("/store/a1.jsonl")].into_iter().collect();

    let hidden = filter_archived(sessions.clone(), SidebarArchivedFilter::Hide, &archived);
    assert_eq!(
        hidden.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["b1"]
    );

    let shown = filter_archived(sessions.clone(), SidebarArchivedFilter::Show, &archived);
    assert_eq!(shown.len(), 2, "show keeps archived sessions in place");

    let only = filter_archived(sessions, SidebarArchivedFilter::Only, &archived);
    assert_eq!(
        only.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["a1"]
    );
}

#[test]
fn workspace_store_defaults_to_grouped_and_hiding_archived() {
    let store = WorkspaceStore::default();
    assert_eq!(store.group_by, SidebarGroupBy::Workspace);
    assert_eq!(store.archived_filter, SidebarArchivedFilter::Hide);

    let parsed = parse_workspace_store(&serde_json::json!({ "workspaces": [] }));
    assert_eq!(parsed.group_by, SidebarGroupBy::Workspace);
    assert_eq!(parsed.archived_filter, SidebarArchivedFilter::Hide);
}

#[test]
fn workspace_store_reads_the_view_choices() {
    let parsed = parse_workspace_store(&serde_json::json!({
        "workspaces": [],
        "group_by": "one_list",
        "archived_filter": "only",
    }));
    assert_eq!(parsed.group_by, SidebarGroupBy::OneList);
    assert_eq!(parsed.archived_filter, SidebarArchivedFilter::Only);

    // A store written during the two-mode experiment still loads: the legacy
    // boolean maps onto the current enum.
    let legacy = parse_workspace_store(&serde_json::json!({
        "workspaces": [],
        "group_by_workspace": false,
    }));
    assert_eq!(legacy.group_by, SidebarGroupBy::OneList);

    // A store written while the tree mode existed falls back to grouped.
    let tree = parse_workspace_store(&serde_json::json!({
        "workspaces": [],
        "group_by": "workspace_tree",
    }));
    assert_eq!(tree.group_by, SidebarGroupBy::Workspace);

    // Non-string values fall back to the defaults.
    let bad = parse_workspace_store(&serde_json::json!({
        "workspaces": [],
        "group_by": 7,
        "archived_filter": false,
    }));
    assert_eq!(bad.group_by, SidebarGroupBy::Workspace);
    assert_eq!(bad.archived_filter, SidebarArchivedFilter::Hide);
}
