use super::sidebar::*;
use super::*;
use std::fs;

fn group_labels(rows: &[SideRow]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r {
            SideRow::Workspace { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect()
}

fn rows_for(workspaces: &[PathBuf]) -> Vec<SideRow> {
    build_sidebar_rows(
        &[],
        workspaces,
        WorkspaceSort::Manual,
        &HashMap::new(),
        "none",
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashSet::new(),
        &None,
        &HashSet::new(),
    )
}

#[test]
fn drop_workspace_removes_the_exact_path() {
    let mut workspaces = vec![PathBuf::from("/work/alpha"), PathBuf::from("/work/beta")];
    assert!(drop_workspace(&mut workspaces, Path::new("/work/alpha")));
    assert_eq!(workspaces, vec![PathBuf::from("/work/beta")]);

    // A path that isn't listed is a no-op.
    assert!(!drop_workspace(&mut workspaces, Path::new("/work/gamma")));
    assert_eq!(workspaces, vec![PathBuf::from("/work/beta")]);
}

#[test]
fn removed_workspace_leaves_no_sidebar_group() {
    let mut workspaces = vec![PathBuf::from("/work/alpha"), PathBuf::from("/work/beta")];
    // A workspace with no sessions still gets a header while it is listed.
    assert_eq!(group_labels(&rows_for(&workspaces)), vec!["alpha", "beta"]);

    drop_workspace(&mut workspaces, Path::new("/work/alpha"));
    assert_eq!(
        group_labels(&rows_for(&workspaces)),
        vec!["beta"],
        "the removed empty workspace must not leave its heading behind"
    );
}

/// Two spellings of one folder must not survive as two workspaces: removing
/// the one the user clicked has to take the other with it, or the second
/// header looks like the removal did nothing.
#[cfg(unix)]
#[test]
fn drop_workspace_removes_every_symlinked_spelling() {
    let root = std::env::temp_dir().join(format!("orbit-drop-ws-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let real = root.join("project");
    fs::create_dir_all(&real).unwrap();
    let link = root.join("shortcut");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let real = fs::canonicalize(&real).unwrap();
    let mut workspaces = vec![real.clone(), link.clone()];
    assert!(drop_workspace(&mut workspaces, &link));
    assert!(
        workspaces.is_empty(),
        "both spellings must be dropped: {workspaces:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

/// A legacy store that listed the same folder under two spellings collapses to
/// one entry when it is read back.
#[cfg(unix)]
#[test]
fn parse_workspace_store_collapses_symlinked_duplicates() {
    let root = std::env::temp_dir().join(format!("orbit-parse-ws-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let real = root.join("project");
    fs::create_dir_all(&real).unwrap();
    let link = root.join("shortcut");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let store = parse_workspace_store(&serde_json::json!({
        "workspaces": [link.to_string_lossy(), real.to_string_lossy()],
    }));
    assert_eq!(store.workspaces.len(), 1, "{:?}", store.workspaces);

    let _ = fs::remove_dir_all(&root);
}
