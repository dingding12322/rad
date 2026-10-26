//! In-memory workspace tree for the Explorer.
//!
//! Pure data: [`TreeIndex`] is built once from a [`super::walk::Snapshot`],
//! and [`visible_rows`] flattens it for a virtualized list. Expand/collapse is
//! a set of paths, filter is a string, and neither mutates the index — so the
//! panel can re-render from immutable state and the whole model is unit-tested
//! without a window.
//!
//! Shape follows `review::tree_rows` (directory auto-expand, filter, collapse)
//! but is keyed by workspace-relative `/`-separated paths instead of diff file
//! indices.

use std::collections::{HashMap, HashSet};

use super::walk::Snapshot;

/// A file's git status as a single letter. Derived from [`crate::git::StatusRow`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusBadge {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl StatusBadge {
    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Untracked => 'U',
            Self::Conflicted => '!',
        }
    }

    /// Reduce a porcelain status row to the one badge a tree shows.
    pub fn from_status(row: &crate::git::StatusRow) -> Self {
        if row.untracked() {
            return Self::Untracked;
        }
        if row.index == 'U' || row.worktree == 'U' {
            return Self::Conflicted;
        }
        if row.index == 'D' || row.worktree == 'D' {
            return Self::Deleted;
        }
        if matches!(row.index, 'R' | 'C') || matches!(row.worktree, 'R' | 'C') {
            return Self::Renamed;
        }
        if row.index == 'A' {
            return Self::Added;
        }
        Self::Modified
    }
}

/// One directory or file in the tree. `path` is workspace-relative and
/// `/`-separated; the root's path is empty.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Node {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<Node>,
}

/// A built tree plus the decorations a render needs. Immutable after
/// [`TreeIndex::build`].
#[derive(Clone, Debug, Default)]
pub struct TreeIndex {
    pub root: Node,
    /// Workspace-relative path → badge for changed files only.
    pub badges: HashMap<String, StatusBadge>,
    /// Directories that contain a changed descendant (Zed's dot on folders).
    pub dirty_dirs: HashSet<String>,
    pub file_count: usize,
    /// The walker hit its entry cap; the tree is incomplete.
    pub truncated: bool,
}

impl TreeIndex {
    /// Build the tree from a flat snapshot and a badge map. Missing
    /// intermediate directories are synthesized, so a file always has its
    /// ancestors even if the walker's directory list is sparse.
    pub fn build(snapshot: Snapshot, badges: HashMap<String, StatusBadge>) -> Self {
        let mut root = Node {
            is_dir: true,
            ..Node::default()
        };
        for dir in &snapshot.dirs {
            let parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
            insert(&mut root, &parts, true);
        }
        for file in &snapshot.files {
            let parts: Vec<&str> = file.split('/').filter(|p| !p.is_empty()).collect();
            insert(&mut root, &parts, false);
        }
        sort_children(&mut root);

        let mut dirty_dirs = HashSet::new();
        for path in badges.keys() {
            for ancestor in ancestors(path) {
                dirty_dirs.insert(ancestor);
            }
        }

        Self {
            root,
            badges,
            dirty_dirs,
            file_count: snapshot.files.len(),
            truncated: snapshot.truncated,
        }
    }

    /// Every directory path in the tree — "expand all".
    pub fn all_dir_paths(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        collect_dirs(&self.root, &mut out);
        out
    }
}

fn insert(node: &mut Node, parts: &[&str], is_dir: bool) {
    let Some((name, rest)) = parts.split_first() else {
        return;
    };
    let path = if node.path.is_empty() {
        (*name).to_string()
    } else {
        format!("{}/{}", node.path, name)
    };
    let index = match node.children.iter().position(|child| child.name == *name) {
        Some(index) => index,
        None => {
            node.children.push(Node {
                name: (*name).to_string(),
                path: path.clone(),
                is_dir: rest.is_empty() && is_dir,
                children: Vec::new(),
            });
            node.children.len() - 1
        }
    };
    if rest.is_empty() {
        node.children[index].is_dir = is_dir;
    } else {
        node.children[index].is_dir = true;
        insert(&mut node.children[index], rest, is_dir);
    }
}

fn sort_children(node: &mut Node) {
    node.children.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    for child in &mut node.children {
        sort_children(child);
    }
}

fn collect_dirs(node: &Node, out: &mut HashSet<String>) {
    for child in &node.children {
        if child.is_dir {
            out.insert(child.path.clone());
            collect_dirs(child, out);
        }
    }
}

/// Every ancestor directory of a `/`-separated path, outermost first.
pub fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if part.is_empty() {
            continue;
        }
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);
        out.push(current.clone());
    }
    out
}

/// One painted row. Directories carry their expanded state so the list can
/// draw the right chevron without re-querying the set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RowKind {
    Dir { expanded: bool },
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub kind: RowKind,
    pub badge: Option<StatusBadge>,
    /// A directory holding a changed descendant.
    pub dirty: bool,
}

impl Row {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, RowKind::Dir { .. })
    }

    pub fn expanded(&self) -> bool {
        matches!(self.kind, RowKind::Dir { expanded: true })
    }
}

/// Flatten the tree into the rows the panel paints. With an empty filter this
/// is a plain expand/collapse walk; a non-empty filter reveals only matching
/// files, their ancestor directories, and — when a directory name matches —
/// that directory's whole subtree.
pub fn visible_rows(index: &TreeIndex, expanded: &HashSet<String>, filter: &str) -> Vec<Row> {
    let needle = filter.trim().to_lowercase();
    let mut rows = Vec::new();
    if needle.is_empty() {
        for child in &index.root.children {
            push_expanded(index, child, 0, expanded, &mut rows);
        }
    } else {
        for child in &index.root.children {
            push_matching(index, child, 0, &needle, &mut rows);
        }
    }
    rows
}

fn push_expanded(
    index: &TreeIndex,
    node: &Node,
    depth: usize,
    expanded: &HashSet<String>,
    out: &mut Vec<Row>,
) {
    let is_expanded = node.is_dir && expanded.contains(&node.path);
    out.push(row_for(index, node, depth, is_expanded));
    if is_expanded {
        for child in &node.children {
            push_expanded(index, child, depth + 1, expanded, out);
        }
    }
}

fn push_matching(
    index: &TreeIndex,
    node: &Node,
    depth: usize,
    needle: &str,
    out: &mut Vec<Row>,
) -> bool {
    let self_match = node.name.to_lowercase().contains(needle);
    if node.is_dir {
        if self_match {
            out.push(row_for(index, node, depth, true));
            for child in &node.children {
                push_all(index, child, depth + 1, out);
            }
            return true;
        }
        let mut child_rows = Vec::new();
        let mut any = false;
        for child in &node.children {
            any |= push_matching(index, child, depth + 1, needle, &mut child_rows);
        }
        if any {
            out.push(row_for(index, node, depth, true));
            out.extend(child_rows);
        }
        any
    } else if self_match {
        out.push(row_for(index, node, depth, false));
        true
    } else {
        false
    }
}

fn push_all(index: &TreeIndex, node: &Node, depth: usize, out: &mut Vec<Row>) {
    out.push(row_for(index, node, depth, node.is_dir));
    for child in &node.children {
        push_all(index, child, depth + 1, out);
    }
}

fn row_for(index: &TreeIndex, node: &Node, depth: usize, expanded: bool) -> Row {
    Row {
        path: node.path.clone(),
        name: node.name.clone(),
        depth,
        kind: if node.is_dir {
            RowKind::Dir { expanded }
        } else {
            RowKind::File
        },
        badge: index.badges.get(&node.path).copied(),
        dirty: index.dirty_dirs.contains(&node.path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(dirs: &[&str], files: &[&str]) -> Snapshot {
        Snapshot {
            dirs: dirs.iter().map(|s| s.to_string()).collect(),
            files: files.iter().map(|s| s.to_string()).collect(),
            truncated: false,
        }
    }

    fn index() -> TreeIndex {
        TreeIndex::build(
            snapshot(
                &["src", "src/app", "empty"],
                &["README.md", "src/main.rs", "src/app/view.rs"],
            ),
            HashMap::from([("src/main.rs".to_string(), StatusBadge::Modified)]),
        )
    }

    fn paths(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.path.as_str()).collect()
    }

    #[test]
    fn build_orders_dirs_first_then_case_insensitively() {
        let index = index();
        let names: Vec<&str> = index
            .root
            .children
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(names, vec!["empty", "src", "README.md"]);
    }

    #[test]
    fn collapsed_root_shows_only_top_level() {
        let index = index();
        let rows = visible_rows(&index, &HashSet::new(), "");
        assert_eq!(paths(&rows), vec!["empty", "src", "README.md"]);
        assert!(rows[1].is_dir());
        assert!(!rows[1].expanded());
        assert!(rows[1].dirty, "src holds a changed file");
        assert_eq!(rows[2].badge, None);
    }

    #[test]
    fn expanding_reveals_children_with_indentation() {
        let index = index();
        let expanded = HashSet::from(["src".to_string()]);
        let rows = visible_rows(&index, &expanded, "");
        assert_eq!(
            paths(&rows),
            vec!["empty", "src", "src/app", "src/main.rs", "README.md"]
        );
        assert_eq!(rows[2].depth, 1);
        assert!(rows[3].badge == Some(StatusBadge::Modified));
        // Depth-0 rows stay out of `src`'s subtree.
        assert_eq!(rows[1].depth, 0);
    }

    #[test]
    fn filter_reveals_matches_and_ancestors() {
        let index = index();
        let rows = visible_rows(&index, &HashSet::new(), "view");
        assert_eq!(paths(&rows), vec!["src", "src/app", "src/app/view.rs"]);
        assert!(
            rows[0].expanded(),
            "ancestor directories are force-expanded"
        );
        assert!(rows[2].badge.is_none());
    }

    #[test]
    fn filter_on_directory_reveals_whole_subtree() {
        let index = index();
        let rows = visible_rows(&index, &HashSet::new(), "app");
        assert_eq!(
            paths(&rows),
            vec!["src", "src/app", "src/app/view.rs"],
            "a matching directory expands its contents"
        );
    }

    #[test]
    fn filter_is_case_insensitive_and_prunes() {
        let index = index();
        let rows = visible_rows(&index, &HashSet::new(), "README");
        assert_eq!(paths(&rows), vec!["README.md"]);
        assert!(visible_rows(&index, &HashSet::new(), "nope").is_empty());
    }

    #[test]
    fn missing_intermediate_dirs_are_synthesized() {
        let index = TreeIndex::build(snapshot(&[], &["a/b/c.rs"]), HashMap::new());
        let rows = visible_rows(&index, &index.all_dir_paths(), "");
        assert_eq!(paths(&rows), vec!["a", "a/b", "a/b/c.rs"]);
    }

    #[test]
    fn dirty_dirs_are_ancestors_of_badged_files() {
        let index = index();
        assert!(index.dirty_dirs.contains("src"));
        assert!(!index.dirty_dirs.contains("empty"));
        assert_eq!(ancestors("a/b/c.rs"), vec!["a", "a/b"]);
    }

    #[test]
    fn all_dir_paths_expands_every_directory() {
        let index = index();
        let all = index.all_dir_paths();
        assert_eq!(all.len(), 3, "src, src/app, empty");
        assert_eq!(visible_rows(&index, &all, "").len(), 6);
    }

    #[test]
    fn status_badge_reduces_porcelain() {
        let mut row = crate::git::StatusRow {
            path: "x".into(),
            orig_path: None,
            index: ' ',
            worktree: '?',
            staged_additions: 0,
            staged_deletions: 0,
            unstaged_additions: 0,
            unstaged_deletions: 0,
        };
        assert_eq!(StatusBadge::from_status(&row), StatusBadge::Untracked);
        row.worktree = 'M';
        assert_eq!(StatusBadge::from_status(&row), StatusBadge::Modified);
        row.worktree = 'D';
        assert_eq!(StatusBadge::from_status(&row), StatusBadge::Deleted);
    }
}
