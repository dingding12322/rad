//! Off-thread workspace snapshot for the Explorer.
//!
//! One recursive pass per (workspace, hidden-toggle) change, run on the
//! background executor. The [`ignore`] crate — ripgrep's walker, already in
//! the dependency graph — is what makes nested `.gitignore` / `.ignore` /
//! global-ignore handling correct; a hand-rolled `read_dir` walk cannot read
//! a nested ignore file without descending into it first.
//!
//! Nothing here touches a frame: it returns plain relative paths that
//! [`super::tree`] turns into an in-memory index.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Hard caps so a pathological tree stays bounded. The panel surfaces
/// [`Snapshot::truncated`] honestly rather than silently dropping files.
pub const MAX_ENTRIES: usize = 50_000;
pub const MAX_DEPTH: usize = 24;

/// Workspace-relative paths (always `/`-separated) for every visible entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub dirs: Vec<String>,
    pub files: Vec<String>,
    /// The entry cap was hit; the tree is incomplete.
    pub truncated: bool,
}

impl Snapshot {
    pub fn len(&self) -> usize {
        self.dirs.len() + self.files.len()
    }
}

/// Recursively snapshot `root`, honoring ignore files. `show_hidden` reveals
/// dotfiles and dot-directories (Zed's hidden-files toggle).
///
/// Symlinks are never followed: a link back up the tree would otherwise walk
/// forever, and a project panel has no business resolving them.
pub fn snapshot(root: &Path, show_hidden: bool) -> Snapshot {
    let mut out = Snapshot::default();
    if !root.is_dir() {
        return out;
    }

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!show_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        // Honor `.gitignore` in a plain folder too, not only inside a repo.
        .require_git(false)
        .follow_links(false)
        .max_depth(Some(MAX_DEPTH));

    for result in builder.build() {
        let Ok(entry) = result else { continue };
        // Depth 0 is the workspace root itself.
        if entry.depth() == 0 {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative_path(relative) else {
            continue;
        };

        if file_type.is_dir() {
            out.dirs.push(relative);
        } else if file_type.is_file() {
            out.files.push(relative);
        } else {
            // Symlink (not followed) or other special file — skip it.
            continue;
        }

        if out.len() >= MAX_ENTRIES {
            out.truncated = true;
            break;
        }
    }

    out
}

/// A workspace-relative path as a `/`-separated string. `None` for the root
/// (empty) or a non-UTF-8 path.
fn relative_path(path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for component in path.components() {
        parts.push(component.as_os_str().to_str()?.to_owned());
    }
    Some(parts.join("/"))
}

/// Join a relative snapshot path back onto its workspace root.
pub fn absolute(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if !part.is_empty() {
            path.push(part);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-explorer-walk-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(dir.join("empty")).unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("README.md"), "# hi\n").unwrap();
        fs::write(dir.join("node_modules/pkg/index.js"), "x\n").unwrap();
        fs::write(dir.join(".hidden"), "x\n").unwrap();
        fs::write(dir.join(".gitignore"), "ignored.txt\nbuild/\n").unwrap();
        fs::write(dir.join("ignored.txt"), "x\n").unwrap();
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::write(dir.join("build/out.js"), "x\n").unwrap();
        dir
    }

    #[test]
    fn snapshot_honors_gitignore_and_keeps_empty_dirs() {
        let root = workspace("ignore");
        let snap = snapshot(&root, false);
        assert!(snap.files.contains(&"src/main.rs".to_string()));
        assert!(snap.files.contains(&"README.md".to_string()));
        assert!(!snap.files.contains(&"ignored.txt".to_string()));
        assert!(!snap.files.contains(&"build/out.js".to_string()));
        assert!(!snap.dirs.contains(&"build".to_string()));
        assert!(snap.dirs.contains(&"empty".to_string()));
        assert!(!snap.truncated);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_hides_dotfiles_until_toggled() {
        let root = workspace("hidden");
        let hidden = snapshot(&root, false);
        assert!(!hidden.files.contains(&".gitignore".to_string()));
        assert!(!hidden.files.contains(&".hidden".to_string()));

        let shown = snapshot(&root, true);
        assert!(shown.files.contains(&".hidden".to_string()));
        assert!(shown.files.contains(&".gitignore".to_string()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_does_not_follow_symlinks() {
        let root = workspace("symlink");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("src/loop")).unwrap();
            let snap = snapshot(&root, false);
            assert!(!snap.dirs.iter().any(|d| d.contains("loop")));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn absolute_rebuilds_a_relative_path() {
        let root = Path::new("/tmp/ws");
        assert_eq!(
            absolute(root, "src/main.rs"),
            Path::new("/tmp/ws/src/main.rs")
        );
        assert_eq!(absolute(root, ""), Path::new("/tmp/ws"));
    }
}
