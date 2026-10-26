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

fn workspace_paths(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

fn group_labels(rows: &[SideRow]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r {
            SideRow::Workspace { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect()
}

/// Build rows with every test group explicitly expanded, so session order
/// never hides a group behind its default collapsed state.
fn rows_for(
    sessions: &[SessionInfo],
    workspaces: &[PathBuf],
    sort: WorkspaceSort,
    added_at: &HashMap<PathBuf, SystemTime>,
) -> Vec<SideRow> {
    let expanded: HashSet<String> = ["alpha", "beta", "gamma", "empty"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    build_sidebar_rows(
        sessions,
        workspaces,
        sort,
        added_at,
        "none",
        &HashSet::new(),
        &expanded,
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    )
}

#[test]
fn manual_keeps_the_project_list_order() {
    let sessions = vec![
        store_session("b1", "/work/beta", 10),
        store_session("a1", "/work/alpha", 20),
    ];
    let workspaces = workspace_paths(&["/work/alpha", "/work/beta"]);
    let rows = rows_for(
        &sessions,
        &workspaces,
        WorkspaceSort::Manual,
        &HashMap::new(),
    );
    assert_eq!(group_labels(&rows), vec!["alpha", "beta"]);
}

#[test]
fn last_updated_puts_the_newest_group_first() {
    let sessions = vec![
        store_session("b1", "/work/beta", 10),
        store_session("a1", "/work/alpha", 20),
    ];
    let workspaces = workspace_paths(&["/work/alpha", "/work/beta"]);
    let rows = rows_for(
        &sessions,
        &workspaces,
        WorkspaceSort::LastUpdated,
        &HashMap::new(),
    );
    assert_eq!(group_labels(&rows), vec!["alpha", "beta"]);
}

#[test]
fn last_updated_keys_on_the_newest_session_per_group() {
    // alpha's only session is old; beta has one older and one newer. The
    // group key must be beta's *max*, not whichever row happens to be first.
    let sessions = vec![
        store_session("a1", "/work/alpha", 5),
        store_session("b1", "/work/beta", 1),
        store_session("b2", "/work/beta", 30),
    ];
    let workspaces = workspace_paths(&["/work/alpha", "/work/beta"]);
    let rows = rows_for(
        &sessions,
        &workspaces,
        WorkspaceSort::LastUpdated,
        &HashMap::new(),
    );
    assert_eq!(group_labels(&rows), vec!["beta", "alpha"]);
}

#[test]
fn last_updated_sorts_an_empty_group_last() {
    let sessions = vec![store_session("b1", "/work/beta", 5)];
    let workspaces = workspace_paths(&["/work/empty", "/work/beta"]);
    let rows = rows_for(
        &sessions,
        &workspaces,
        WorkspaceSort::LastUpdated,
        &HashMap::new(),
    );
    assert_eq!(group_labels(&rows), vec!["beta", "empty"]);
}

#[test]
fn last_updated_keeps_the_open_sessions_group_first() {
    // beta has the newest activity (a background run writing), but alpha
    // holds the open session. Alpha must still lead, or the workspace the
    // user is working in slides down on every watcher reload.
    let sessions = vec![
        store_session("a1", "/work/alpha", 10),
        store_session("b1", "/work/beta", 30),
    ];
    let workspaces = workspace_paths(&["/work/alpha", "/work/beta"]);
    let expanded: HashSet<String> = ["alpha", "beta"].iter().map(|s| s.to_string()).collect();
    let active = Some(sessions[0].path.clone());
    let rows = build_sidebar_rows(
        &sessions,
        &workspaces,
        WorkspaceSort::LastUpdated,
        &HashMap::new(),
        "none",
        &HashSet::new(),
        &expanded,
        &HashMap::new(),
        &HashSet::new(),
        &active,
        &HashSet::new(),
    );
    assert_eq!(group_labels(&rows), vec!["alpha", "beta"]);
}

#[test]
fn active_session_leads_its_group_over_a_newer_run() {
    // Within one group the open session (oldest) must beat a running sibling
    // with newer activity; otherwise the two swap as each appends a message.
    let sessions = vec![
        store_session("a1", "/work/alpha", 10),
        store_session("a2", "/work/alpha", 30),
        store_session("a3", "/work/alpha", 20),
    ];
    let workspaces = workspace_paths(&["/work/alpha"]);
    let expanded: HashSet<String> = ["alpha"].iter().map(|s| s.to_string()).collect();
    let running: HashSet<PathBuf> = [sessions[1].path.clone()].into_iter().collect();
    let active = Some(sessions[0].path.clone());
    let rows = build_sidebar_rows(
        &sessions,
        &workspaces,
        WorkspaceSort::Manual,
        &HashMap::new(),
        "none",
        &HashSet::new(),
        &expanded,
        &HashMap::new(),
        &HashSet::new(),
        &active,
        &running,
    );
    let indices: Vec<usize> = rows
        .iter()
        .filter_map(|row| match row {
            SideRow::Session(ix) => Some(*ix),
            _ => None,
        })
        .collect();
    // Open session first, then the live run, then the idle sibling.
    assert_eq!(indices, vec![0, 1, 2]);
}

#[test]
fn alphabetical_orders_by_label_case_insensitively() {
    let workspaces = workspace_paths(&["/work/Zeta", "/work/apple", "/work/Banana"]);
    let rows = rows_for(
        &[],
        &workspaces,
        WorkspaceSort::AlphabeticalAsc,
        &HashMap::new(),
    );
    assert_eq!(group_labels(&rows), vec!["apple", "Banana", "Zeta"]);

    let rows = rows_for(
        &[],
        &workspaces,
        WorkspaceSort::AlphabeticalDesc,
        &HashMap::new(),
    );
    assert_eq!(group_labels(&rows), vec!["Zeta", "Banana", "apple"]);
}

#[test]
fn date_added_puts_the_most_recent_first() {
    let workspaces = workspace_paths(&["/work/alpha", "/work/beta", "/work/gamma"]);
    let added_at = HashMap::from([
        (
            PathBuf::from("/work/alpha"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(100),
        ),
        (
            PathBuf::from("/work/beta"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(300),
        ),
        (
            PathBuf::from("/work/gamma"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(200),
        ),
    ]);
    let rows = rows_for(&[], &workspaces, WorkspaceSort::DateAdded, &added_at);
    assert_eq!(group_labels(&rows), vec!["beta", "gamma", "alpha"]);
}

#[test]
fn session_count_puts_the_busiest_group_first() {
    let sessions = vec![
        store_session("a1", "/work/alpha", 1),
        store_session("b1", "/work/beta", 1),
        store_session("b2", "/work/beta", 1),
    ];
    let workspaces = workspace_paths(&["/work/alpha", "/work/beta"]);
    let rows = rows_for(
        &sessions,
        &workspaces,
        WorkspaceSort::SessionCount,
        &HashMap::new(),
    );
    assert_eq!(group_labels(&rows), vec!["beta", "alpha"]);
}

#[test]
fn sort_keys_round_trip_through_disk() {
    for sort in WorkspaceSort::ALL {
        assert_eq!(WorkspaceSort::from_key(sort.as_str()), sort);
    }
    // A missing key (a store predating the control) or an unknown one falls
    // back to the out-of-the-box order: most recent activity first.
    assert_eq!(
        WorkspaceSort::from_key("nonsense"),
        WorkspaceSort::LastUpdated
    );
    assert_eq!(WorkspaceSort::from_key(""), WorkspaceSort::LastUpdated);
}

#[test]
fn the_default_sort_is_last_updated() {
    assert_eq!(WorkspaceSort::default(), WorkspaceSort::LastUpdated);
}

/// The workspace store keeps each project's mark (icon stem + tint key) across
/// a load, omits it when unset, and still reads the legacy bare-path format.
#[test]
fn workspace_marks_round_trip_through_the_store() {
    let value: Value = serde_json::from_str(
        r#"{
            "sort": "manual",
            "workspaces": [
                { "path": "/work/alpha", "added_at": 10, "icon": "rocket-01", "tint": "accent" },
                { "path": "/work/beta", "added_at": 20 },
                "/work/gamma"
            ]
        }"#,
    )
    .expect("valid store JSON");

    let store = parse_workspace_store(&value);
    assert_eq!(store.workspaces.len(), 3);
    assert_eq!(store.sort, WorkspaceSort::Manual);
    assert_eq!(
        store.marks.get(&PathBuf::from("/work/alpha")),
        Some(&WorkspaceMark {
            icon: Some("rocket-01".into()),
            tint: Some("accent".into()),
        })
    );
    // No mark stored for beta or the legacy bare path.
    assert!(!store.marks.contains_key(&PathBuf::from("/work/beta")));
    assert!(!store.marks.contains_key(&PathBuf::from("/work/gamma")));
}
