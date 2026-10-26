//! Local Git operations that stop halfway and wait for the user: merges,
//! rebases, cherry-picks, reverts, and bisects.
//!
//! Git leaves its half-finished state in `.git` (a `MERGE_HEAD`, a
//! `rebase-merge/` directory, …). This module detects that state, lists the
//! files still in conflict, and runs the matching continue / abort / skip so
//! the Git page can present one consistent "operation in progress" bar instead
//! of a one-off dialog per command.
//!
//! Everything here is pure Git I/O and runs on the background executor.

use std::path::{Path, PathBuf};

use crate::git::{command, run_git, RefKind};

/// A Git operation Git has left half-finished, waiting for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InProgress {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
}

impl InProgress {
    /// The i18n key for this operation's banner label.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Merge => "git_panel.op_merge",
            Self::Rebase => "git_panel.op_rebase",
            Self::CherryPick => "git_panel.op_cherry_pick",
            Self::Revert => "git_panel.op_revert",
            Self::Bisect => "git_panel.op_bisect",
        }
    }

    /// Whether this operation supports `--skip` (rebase and cherry-pick do;
    /// merge, revert, and bisect do not).
    pub fn can_skip(self) -> bool {
        matches!(self, Self::Rebase | Self::CherryPick)
    }
}

/// The `.git` directory for `cwd`, resolved through Git so linked worktrees and
/// submodules land in the right place.
fn git_dir(cwd: &Path) -> Option<PathBuf> {
    let raw = run_git(cwd, &["rev-parse", "--git-dir"]).ok()?;
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(&raw);
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

/// Detect an in-progress operation from the marker files Git writes.
///
/// Order matters: a rebase can also have a `MERGE_HEAD`-like state in edge
/// cases, so the operation-specific directories are checked first.
pub fn operation_state(cwd: &Path) -> Option<InProgress> {
    let dir = git_dir(cwd)?;
    if dir.join("rebase-merge").is_dir() || dir.join("rebase-apply").is_dir() {
        return Some(InProgress::Rebase);
    }
    if dir.join("MERGE_HEAD").is_file() {
        return Some(InProgress::Merge);
    }
    if dir.join("CHERRY_PICK_HEAD").is_file() {
        return Some(InProgress::CherryPick);
    }
    if dir.join("REVERT_HEAD").is_file() {
        return Some(InProgress::Revert);
    }
    if dir.join("BISECT_LOG").is_file() {
        return Some(InProgress::Bisect);
    }
    None
}

/// Files with unresolved conflicts, in Git's own order. Empty when there is no
/// operation (or nothing is conflicted).
pub fn conflicted_files(cwd: &Path) -> Vec<String> {
    run_git(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--name-only",
            "--diff-filter=U",
            "-z",
        ],
    )
    .unwrap_or_default()
    .split('\0')
    .filter(|path| !path.is_empty())
    .map(str::to_string)
    .collect()
}

/// Git invokes `GIT_EDITOR` for a continue/abort that wants a commit message.
/// A `:` no-op keeps the operation non-interactive; a real editor would block
/// the background executor forever.
const NO_EDITOR: &[(&str, &str)] = &[("GIT_EDITOR", ":"), ("GIT_SEQUENCE_EDITOR", ":")];

/// Run a `git` command in `cwd` with extra environment, returning trimmed
/// stdout or the trimmed stderr.
fn run(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<String, String> {
    let mut command = command(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .args(args)
        .output()
        .map_err(|err| tr!("git.not_available", error = err))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(tr!("git.exited_with", status = output.status.to_string()))
    } else {
        Err(stderr)
    }
}

/// Continue the in-progress operation after the user resolved conflicts.
pub fn continue_operation(cwd: &Path, op: InProgress) -> Result<String, String> {
    match op {
        // A merge's continue is a commit; `--no-edit` keeps the generated
        // merge message.
        InProgress::Merge => {
            run(cwd, &["merge", "--continue", "--no-edit"], NO_EDITOR)?;
            Ok(tr!("git_panel.operation_continued"))
        }
        InProgress::Rebase => {
            run(cwd, &["rebase", "--continue"], NO_EDITOR)?;
            Ok(tr!("git_panel.operation_continued"))
        }
        InProgress::CherryPick => {
            run(cwd, &["cherry-pick", "--continue"], NO_EDITOR)?;
            Ok(tr!("git_panel.operation_continued"))
        }
        InProgress::Revert => {
            run(cwd, &["revert", "--continue"], NO_EDITOR)?;
            Ok(tr!("git_panel.operation_continued"))
        }
        // Bisect has no continue: marking the current commit good/bad is a
        // deliberate choice the bar cannot make for the user.
        InProgress::Bisect => Err(tr!("git_panel.op_bisect_no_continue")),
    }
}

/// Abort the in-progress operation, restoring the pre-operation state.
pub fn abort_operation(cwd: &Path, op: InProgress) -> Result<String, String> {
    match op {
        InProgress::Merge => run(cwd, &["merge", "--abort"], NO_EDITOR)?,
        InProgress::Rebase => run(cwd, &["rebase", "--abort"], NO_EDITOR)?,
        InProgress::CherryPick => run(cwd, &["cherry-pick", "--abort"], NO_EDITOR)?,
        InProgress::Revert => run(cwd, &["revert", "--abort"], NO_EDITOR)?,
        InProgress::Bisect => run(cwd, &["bisect", "reset"], NO_EDITOR)?,
    };
    Ok(tr!("git_panel.operation_aborted"))
}

/// Skip the current patch of a rebase or cherry-pick.
pub fn skip_operation(cwd: &Path, op: InProgress) -> Result<String, String> {
    match op {
        InProgress::Rebase => run(cwd, &["rebase", "--skip"], NO_EDITOR)?,
        InProgress::CherryPick => run(cwd, &["cherry-pick", "--skip"], NO_EDITOR)?,
        _ => return Err(tr!("git_panel.op_cannot_skip")),
    };
    Ok(tr!("git_panel.operation_skipped"))
}

// ── refs, merge, rebase ────────────────────────────────────────────────

/// A ref that can be a merge or rebase target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEntry {
    /// Short name (`feature`, `origin/main`, `v1.2.0`).
    pub name: String,
    pub kind: RefKind,
}

/// Every ref a merge/rebase can target: local branches, remote-tracking
/// branches (minus the `origin/HEAD` symref), and tags, newest first.
pub fn list_refs(cwd: &Path) -> Vec<RefEntry> {
    let out = run_git(
        cwd,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
        ],
    )
    .unwrap_or_default();
    out.lines().filter_map(parse_ref).collect()
}

fn parse_ref(full: &str) -> Option<RefEntry> {
    if let Some(name) = full.strip_prefix("refs/heads/") {
        Some(RefEntry {
            name: name.to_string(),
            kind: RefKind::Branch,
        })
    } else if let Some(name) = full.strip_prefix("refs/remotes/") {
        // `origin/HEAD` is a symbolic pointer, not a target.
        (!name.ends_with("/HEAD")).then(|| RefEntry {
            name: name.to_string(),
            kind: RefKind::Remote,
        })
    } else {
        full.strip_prefix("refs/tags/").map(|name| RefEntry {
            name: name.to_string(),
            kind: RefKind::Tag,
        })
    }
}

/// How a merge should combine the target into the current branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    /// Fast-forward when possible, otherwise a merge commit.
    Merge,
    /// Always create a merge commit (`--no-ff`).
    NoFf,
    /// Stage the changes without committing (`--squash`).
    Squash,
}

/// Merge `reference` into the current branch.
pub fn merge_ref(cwd: &Path, reference: &str, mode: MergeMode) -> Result<String, String> {
    let mut args = vec!["merge", "--no-edit"];
    match mode {
        MergeMode::Merge => {}
        MergeMode::NoFf => args.push("--no-ff"),
        MergeMode::Squash => args.push("--squash"),
    }
    args.push(reference);
    run(cwd, &args, NO_EDITOR)?;
    Ok(tr!("git_panel.merged_ref", name = reference))
}

/// Rebase the current branch onto `reference`. `autostash` carries a dirty
/// worktree across the rebase instead of refusing to start.
pub fn rebase_ref(cwd: &Path, reference: &str, autostash: bool) -> Result<String, String> {
    let mut args = vec!["rebase"];
    if autostash {
        args.push("--autostash");
    }
    args.push(reference);
    run(cwd, &args, NO_EDITOR)?;
    Ok(tr!("git_panel.rebased_ref", name = reference))
}

// ── stash ──────────────────────────────────────────────────────────────

/// One `git stash` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// The `stash@{n}` index.
    pub index: usize,
    /// The stash subject (`WIP on main: …` or the `-m` message).
    pub message: String,
}

/// The stash stack, newest first.
pub fn list_stashes(cwd: &Path) -> Vec<StashEntry> {
    let out = run_git(cwd, &["stash", "list", "--format=%gd%x1f%gs"]).unwrap_or_default();
    out.lines()
        .filter_map(|line| {
            let (rev, message) = line.split_once('\u{1f}')?;
            let index = rev
                .strip_prefix("stash@{")?
                .strip_suffix('}')?
                .parse()
                .ok()?;
            Some(StashEntry {
                index,
                message: message.to_string(),
            })
        })
        .collect()
}

/// Stash the working tree (optionally including untracked files).
pub fn stash_push(
    cwd: &Path,
    message: Option<&str>,
    include_untracked: bool,
) -> Result<String, String> {
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("--include-untracked");
    }
    if let Some(message) = message {
        if !message.trim().is_empty() {
            args.push("-m");
            args.push(message.trim());
        }
    }
    run(cwd, &args, NO_EDITOR)?;
    Ok(tr!("git_panel.stashed"))
}

fn stash_rev(index: usize) -> String {
    format!("stash@{{{index}}}")
}

/// Apply a stash and drop it from the stack.
pub fn stash_pop(cwd: &Path, index: usize) -> Result<String, String> {
    run(cwd, &["stash", "pop", &stash_rev(index)], NO_EDITOR)?;
    Ok(tr!("git_panel.stash_popped"))
}

/// Apply a stash, keeping it on the stack.
pub fn stash_apply(cwd: &Path, index: usize) -> Result<String, String> {
    run(cwd, &["stash", "apply", &stash_rev(index)], NO_EDITOR)?;
    Ok(tr!("git_panel.stash_applied"))
}

/// Drop a stash without applying it.
pub fn stash_drop(cwd: &Path, index: usize) -> Result<String, String> {
    run(cwd, &["stash", "drop", &stash_rev(index)], NO_EDITOR)?;
    Ok(tr!("git_panel.stash_dropped"))
}

// ── branches ───────────────────────────────────────────────────────────

/// Rename the branch `from` to `to`.
pub fn rename_branch(cwd: &Path, from: &str, to: &str) -> Result<String, String> {
    let to = to.trim();
    if to.is_empty() {
        return Err(tr!("git.branch_name_empty"));
    }
    run(cwd, &["branch", "-m", from, to], &[])?;
    Ok(tr!("git_panel.branch_renamed", name = to))
}

/// Delete a branch. `force` uses `-D` (discards unmerged commits); the caller
/// confirms before passing `true`.
pub fn delete_branch(cwd: &Path, name: &str, force: bool) -> Result<String, String> {
    let flag = if force { "-D" } else { "-d" };
    run(cwd, &["branch", flag, name], &[])?;
    Ok(tr!("git_panel.branch_deleted", name = name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn git_ok(cwd: &Path, args: &[&str]) {
        let status = command(cwd).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn repository() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orbit-git-ops-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init", "--quiet", "--initial-branch=main"]);
        git_ok(&root, &["config", "user.name", "Orbit Test"]);
        git_ok(&root, &["config", "user.email", "orbit@example.com"]);
        fs::write(root.join("a.txt"), "base\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "--quiet", "-m", "base"]);
        root
    }

    #[test]
    fn no_operation_on_a_clean_repo() {
        let root = repository();
        assert_eq!(operation_state(&root), None);
        assert!(conflicted_files(&root).is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_conflicted_merge_is_detected_with_its_files() {
        let root = repository();
        git_ok(&root, &["switch", "-c", "feature"]);
        fs::write(root.join("a.txt"), "feature\n").unwrap();
        git_ok(&root, &["commit", "--quiet", "-am", "feature"]);
        git_ok(&root, &["switch", "main"]);
        fs::write(root.join("a.txt"), "main\n").unwrap();
        git_ok(&root, &["commit", "--quiet", "-am", "main"]);
        // The merge must fail: both sides changed the same line.
        let status = command(&root)
            .args(["merge", "--no-edit", "feature"])
            .status()
            .unwrap();
        assert!(!status.success(), "merge should conflict");

        assert_eq!(operation_state(&root), Some(InProgress::Merge));
        assert_eq!(conflicted_files(&root), vec!["a.txt".to_string()]);

        // Aborting restores the pre-merge state and clears the markers.
        let note = abort_operation(&root, InProgress::Merge).unwrap();
        assert!(!note.is_empty());
        assert_eq!(operation_state(&root), None);

        // Continuing without resolving fails with a conflict message.
        let status = command(&root)
            .args(["merge", "--no-edit", "feature"])
            .status()
            .unwrap();
        assert!(!status.success());
        let error = continue_operation(&root, InProgress::Merge).unwrap_err();
        assert!(!error.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn skip_is_only_offered_where_git_supports_it() {
        assert!(InProgress::Rebase.can_skip());
        assert!(InProgress::CherryPick.can_skip());
        assert!(!InProgress::Merge.can_skip());
        assert!(!InProgress::Revert.can_skip());
        assert!(!InProgress::Bisect.can_skip());
    }

    #[test]
    fn bisect_has_no_continue() {
        let root = repository();
        assert!(continue_operation(&root, InProgress::Bisect).is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn refs_include_locals_remotes_and_tags_but_not_origin_head() {
        let root = repository();
        git_ok(&root, &["branch", "feature"]);
        git_ok(&root, &["tag", "v1.0.0"]);
        // A remote-tracking ref and the `origin/HEAD` symref, without a network.
        git_ok(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git_ok(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );

        let refs = list_refs(&root);
        let kind = |name: &str| refs.iter().find(|entry| entry.name == name).map(|e| e.kind);
        assert_eq!(kind("main"), Some(RefKind::Branch));
        assert_eq!(kind("feature"), Some(RefKind::Branch));
        assert_eq!(kind("origin/main"), Some(RefKind::Remote));
        assert_eq!(kind("v1.0.0"), Some(RefKind::Tag));
        assert_eq!(kind("origin/HEAD"), None, "origin/HEAD is not a target");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn merge_and_rebase_onto_a_branch() {
        let root = repository();
        git_ok(&root, &["switch", "-c", "feature"]);
        fs::write(root.join("b.txt"), "feature\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "--quiet", "-m", "feature"]);
        git_ok(&root, &["switch", "main"]);

        let note = merge_ref(&root, "feature", MergeMode::NoFf).unwrap();
        assert!(!note.is_empty());
        assert_eq!(operation_state(&root), None);
        assert!(root.join("b.txt").exists(), "the merge brought the file in");

        // A second branch, rebased onto main after main advances.
        git_ok(&root, &["switch", "-c", "work"]);
        fs::write(root.join("c.txt"), "work\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "--quiet", "-m", "work"]);
        git_ok(&root, &["switch", "main"]);
        fs::write(root.join("d.txt"), "main\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "--quiet", "-m", "main2"]);
        git_ok(&root, &["switch", "work"]);
        rebase_ref(&root, "main", false).unwrap();
        assert_eq!(operation_state(&root), None);
        assert!(root.join("c.txt").exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stash_round_trip() {
        let root = repository();
        fs::write(root.join("a.txt"), "changed\n").unwrap();
        stash_push(&root, Some("wip"), false).unwrap();
        let stashes = list_stashes(&root);
        assert_eq!(stashes.len(), 1);
        assert!(stashes[0].message.contains("wip"));
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "base\n");

        stash_pop(&root, 0).unwrap();
        assert!(list_stashes(&root).is_empty());
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "changed\n");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rename_and_delete_branch() {
        let root = repository();
        git_ok(&root, &["branch", "old"]);
        rename_branch(&root, "old", "new").unwrap();
        let branches = run_git(&root, &["branch", "--format=%(refname:short)"]).unwrap();
        assert!(branches.lines().any(|name| name == "new"));
        assert!(!branches.lines().any(|name| name == "old"));

        delete_branch(&root, "new", false).unwrap();
        let branches = run_git(&root, &["branch", "--format=%(refname:short)"]).unwrap();
        assert!(!branches.lines().any(|name| name == "new"));
        fs::remove_dir_all(root).ok();
    }
}
