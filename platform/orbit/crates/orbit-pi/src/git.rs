//! Local Git helpers: branch discovery for the status bar and the Review
//! panel's diff collection.
//!
//! Everything here is pure I/O and stays off the UI thread. The workspace is
//! compared through [`crate::checkpoint`] snapshots so untracked files, stage
//! state, and per-turn checkpoints are represented exactly.

use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{anyhow, bail};

use crate::checkpoint::{self, EMPTY_TREE};
use crate::review::Source;

/// One `git diff` capture: numstat summary plus the (possibly full-context)
/// patch text.
#[derive(Debug, Clone)]
pub struct ReviewDiff {
    pub numstat: String,
    pub patch: String,
    /// Whether the patch carries complete file context (enables gap
    /// expansion). False when the hydrated patch exceeded the safety cap and
    /// Git was re-run with 3 lines of context.
    pub complete_context: bool,
}

const MAX_HYDRATED_PATCH_BYTES: usize = 32 * 1024 * 1024;

/// A `git` invocation rooted at `cwd`, inheriting the environment.
///
/// `GIT_OPTIONAL_LOCKS=0` is essential here: without it, read-only commands
/// like `git status`/`git diff` refresh the index stat cache as a side effect,
/// writing `.git/index`. The workspace watcher keeps `.git/index`, so that
/// write would re-trigger a refresh forever. Optional locks disabled only
/// suppress opportunistic sub-operations; staging/commit still take their
/// required locks.
pub(crate) fn command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(cwd);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    orbit_rpc::hide_console(&mut command);
    command
}

/// Whether `cwd` sits inside a Git work tree.
pub fn is_repo(cwd: &Path) -> bool {
    run_git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map(|out| out == "true")
        .unwrap_or(false)
}

/// Current branch name, if any.
pub fn current_branch(cwd: &Path) -> Option<String> {
    run_git(cwd, &["branch", "--show-current"])
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| read_head_branch(cwd))
}

/// Local branches, newest commit first.
pub fn list_branches(cwd: &Path) -> Result<Vec<String>, String> {
    let out = run_git(
        cwd,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "refs/heads/",
            "--format=%(refname:short)",
        ],
    )?;
    Ok(out
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Switch to an existing branch, keeping the working tree when possible.
pub fn checkout_branch(cwd: &Path, branch: &str) -> Result<(), String> {
    if run_git(cwd, &["switch", branch]).is_ok() {
        return Ok(());
    }
    run_git(cwd, &["checkout", branch]).map(|_| ())
}

/// Branches most recently checked out, newest first. Derived from the HEAD
/// reflog (so it survives restarts); only names that still exist are returned.
pub fn recent_branches(cwd: &Path) -> Vec<String> {
    let out = run_git(cwd, &["reflog", "--format=%gs", "-n", "200"]).unwrap_or_default();
    let known: HashSet<String> = list_branches(cwd).unwrap_or_default().into_iter().collect();
    let mut seen = HashSet::new();
    let mut recent = Vec::new();
    for line in out.lines() {
        let Some(rest) = line.strip_prefix("checkout: moving from ") else {
            continue;
        };
        let Some((_, to)) = rest.rsplit_once(" to ") else {
            continue;
        };
        let to = to.trim();
        if to.is_empty() || !known.contains(to) {
            continue;
        }
        if seen.insert(to.to_string()) {
            recent.push(to.to_string());
        }
    }
    recent
}

/// Switch to the local branch that tracks `remote_ref` (e.g. `origin/feat/x`),
/// creating it if it does not exist yet. A lone name falls back to a plain
/// checkout.
pub fn checkout_remote_branch(cwd: &Path, remote_ref: &str) -> Result<(), String> {
    let Some((_, short)) = remote_ref.split_once('/') else {
        return checkout_branch(cwd, remote_ref);
    };
    if list_branches(cwd)?.iter().any(|b| b == short) {
        return checkout_branch(cwd, short);
    }
    if run_git(cwd, &["switch", "--track", "-c", short, remote_ref]).is_ok() {
        return Ok(());
    }
    run_git(cwd, &["checkout", "-b", short, "--track", remote_ref]).map(|_| ())
}

/// Create a branch from HEAD and check it out, carrying uncommitted changes.
pub fn create_and_checkout_branch(cwd: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(tr!("git.branch_name_empty"));
    }
    if name.contains([' ', '\t', '~', '^', ':', '?', '*', '[']) || name.contains("..") {
        return Err(tr!("git.branch_name_invalid"));
    }
    if run_git(cwd, &["switch", "-c", name]).is_ok() {
        return Ok(());
    }
    run_git(cwd, &["checkout", "-b", name]).map(|_| ())
}

/// Commit the staged changes with `message`. With `include_unstaged`, stage
/// everything first. Returns a short success note.
pub fn commit(cwd: &Path, message: &str, include_unstaged: bool) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err(tr!("git.enter_commit_message"));
    }
    if include_unstaged {
        run_git(cwd, &["add", "-A", "--", "."])?;
    } else {
        let staged = run_git(cwd, &["diff", "--cached", "--name-only"]).unwrap_or_default();
        if staged.trim().is_empty() {
            return Err(tr!("git.no_staged_changes"));
        }
    }
    run_git(cwd, &["commit", "-m", message])?;
    Ok(tr!("git.committed"))
}

/// Push the current branch, setting its upstream on the first push.
pub fn push(cwd: &Path) -> Result<String, String> {
    // With an upstream, a plain push is the only correct command (a failure is
    // auth/network, not a missing upstream).
    if run_git(cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok() {
        run_git(cwd, &["push"])?;
        return Ok(tr!("git.pushed"));
    }
    match current_branch(cwd) {
        Some(branch) => {
            run_git(cwd, &["push", "-u", "origin", &branch])?;
            Ok(tr!("git.pushed_and_set_upstream"))
        }
        None => Err(tr!("git.no_branch_to_push")),
    }
}

/// Push the current branch with `--force-with-lease`, never bare `--force`.
/// The lease aborts the push if the remote moved since our last fetch, so this
/// cannot silently discard someone else's commits.
pub fn push_force_with_lease(cwd: &Path) -> Result<String, String> {
    if run_git(cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok() {
        run_git(cwd, &["push", "--force-with-lease"])?;
        return Ok(tr!("git.force_pushed"));
    }
    match current_branch(cwd) {
        Some(branch) => {
            run_git(
                cwd,
                &["push", "--force-with-lease", "-u", "origin", &branch],
            )?;
            Ok(tr!("git.force_pushed_and_set_upstream"))
        }
        None => Err(tr!("git.no_branch_to_push")),
    }
}

/// Fast-forward the current branch from its upstream. Fails (with "not
/// possible to fast-forward") when the branch has diverged; use
/// [`merge_upstream`] for that case.
pub fn pull(cwd: &Path) -> Result<String, String> {
    run_git(cwd, &["pull", "--ff-only"])?;
    Ok(tr!("git.pulled"))
}

/// Fetch and merge the upstream into the current branch, tolerating
/// divergence. Unlike [`pull`], a real merge is performed, so conflicts are
/// reported instead of the branch being left untouched. Conflict output is
/// whatever `git` prints (surfaced in full by the Git page).
pub fn merge_upstream(cwd: &Path) -> Result<String, String> {
    // `--ff` overrides a `pull.ff = only` config that would otherwise refuse
    // the merge; `--no-rebase` keeps this a merge, never a rebase.
    run_git(cwd, &["pull", "--no-rebase", "--ff", "--no-edit"])?;
    Ok(tr!("git.merged"))
}

/// Rebase the current branch onto its upstream. `--autostash` carries a dirty
/// worktree across the rebase instead of refusing to start.
pub fn rebase_upstream(cwd: &Path) -> Result<String, String> {
    run_git(cwd, &["pull", "--rebase", "--autostash"])?;
    Ok(tr!("git.rebased"))
}

/// Update remote-tracking refs without touching the working tree. Used after
/// a rejected push so ahead/behind counts reflect the real remote state.
pub fn fetch(cwd: &Path) -> Result<(), String> {
    run_git(cwd, &["fetch", "--quiet"])?;
    Ok(())
}

/// Bring the current branch level with its upstream in one step: fetch, then
/// fast-forward (or merge, when diverged) if behind, then push if ahead. The
/// branch row's Sync button.
pub fn sync(cwd: &Path) -> Result<String, String> {
    run_git(cwd, &["fetch", "--quiet"])?;
    let Some((ahead, behind)) = ahead_behind(cwd) else {
        // No upstream yet: fetching is all there is to do.
        return Ok(tr!("git.fetched"));
    };
    if behind > 0 {
        if ahead > 0 {
            merge_upstream(cwd)?;
        } else {
            pull(cwd)?;
        }
    }
    if ahead_behind(cwd).is_some_and(|(ahead, _)| ahead > 0) {
        push(cwd)?;
    }
    Ok(tr!("git.synced"))
}

fn read_head_branch(cwd: &Path) -> Option<String> {
    let head = std::fs::read_to_string(cwd.join(".git/HEAD")).ok()?;
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/")
        .map(str::to_string)
        .or_else(|| head.get(..7).map(str::to_string))
}

/// Run a git command in `cwd`, returning trimmed stdout. Shared with the
/// branch picker and the Review side pane.
pub(crate) fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = command(cwd)
        .args(args)
        .output()
        .map_err(|err| tr!("git.not_available", error = err))?;
    if !output.status.success() {
        return Err(command_error(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git command, returning raw (untrimmed) stdout, or an error.
pub(crate) fn run_git_ok<I, S>(cwd: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = command(cwd).args(args).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        bail!("{}", command_error(&output));
    }
}

fn command_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        tr!("git.exited_with", status = output.status.to_string())
    } else {
        stderr
    }
}

// ── Review diff collection ─────────────────────────────────────────────────

/// Collect the workspace changes for `source` and parse them for render.
pub fn collect_review_diff(
    cwd: &Path,
    source: Source,
    session: Option<&str>,
) -> Result<ReviewDiff, String> {
    collect_review_diff_inner(cwd, source, session).map_err(|err| err.to_string())
}

fn collect_review_diff_inner(
    cwd: &Path,
    source: Source,
    session: Option<&str>,
) -> anyhow::Result<ReviewDiff> {
    ensure_repository(cwd)?;
    let (from, to) = resolve_range(cwd, source, session)?;
    let numstat = diff_output(cwd, &from, &to, &["--numstat"])?;
    let hydrated = diff_output(cwd, &from, &to, &["--unified=2147483647"])?;
    let (patch, complete_context) = if hydrated.len() <= MAX_HYDRATED_PATCH_BYTES {
        (hydrated, true)
    } else {
        (diff_output(cwd, &from, &to, &["--unified=3"])?, false)
    };
    Ok(ReviewDiff {
        numstat,
        patch,
        complete_context,
    })
}

fn resolve_range(
    cwd: &Path,
    source: Source,
    session: Option<&str>,
) -> anyhow::Result<(String, String)> {
    let head = head_or_empty(cwd);
    Ok(match source {
        Source::LastTurn { turn_count } => {
            let session =
                session.ok_or_else(|| anyhow!("no session is open for turn checkpoints"))?;
            if turn_count == 0 {
                bail!("the first checkpoint is a baseline, not a completed turn");
            }
            let from = [
                checkpoint::turn_diff_base_ref(session, turn_count),
                checkpoint::turn_start_ref(session, turn_count),
                checkpoint::checkpoint_ref(session, turn_count - 1),
            ]
            .into_iter()
            .find_map(|rev| checkpoint::resolve(cwd, &rev))
            .ok_or_else(|| anyhow!("the turn's starting checkpoint is unavailable"))?;
            let to = checkpoint::resolve(cwd, &checkpoint::checkpoint_ref(session, turn_count))
                .ok_or_else(|| anyhow!("the turn's ending checkpoint is unavailable"))?;
            (from, to)
        }
        Source::Uncommitted => (head, checkpoint::capture_worktree_commit(cwd)?),
        Source::Unstaged => (index_tree(cwd)?, checkpoint::capture_worktree_commit(cwd)?),
        Source::Staged => (head, index_tree(cwd)?),
        Source::Committed => (branch_base(cwd), head_or_empty(cwd)),
        Source::Branch => (branch_base(cwd), checkpoint::capture_worktree_commit(cwd)?),
    })
}

/// The tree the index would write — the "staged" snapshot. Does not modify
/// the index or the working tree.
fn index_tree(cwd: &Path) -> anyhow::Result<String> {
    // `git write-tree` rewrites the index (it records the cache-tree
    // extension) even with `GIT_OPTIONAL_LOCKS=0`. The workspace watcher keeps
    // `.git/index`, so writing the real one would retrigger the Git page's
    // refresh forever. Point Git at a throwaway copy instead.
    let copy = index_copy(cwd)?;
    let output = command(cwd)
        .env("GIT_INDEX_FILE", &copy)
        .args(["write-tree"])
        .output()
        .map_err(|err| anyhow!("failed to snapshot the Git index: {err}"))?;
    let _ = std::fs::remove_file(&copy);
    let _ = std::fs::remove_file(copy.with_extension("lock"));
    if output.status.success() {
        let tree = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !tree.is_empty() {
            return Ok(tree);
        }
    }
    // An unborn branch has no HEAD but may still have a usable empty tree.
    if checkpoint::has_head(cwd) {
        bail!("{}", command_error(&output));
    }
    Ok(EMPTY_TREE.to_owned())
}

/// A throwaway copy of the repository's index under its Git directory, where
/// the workspace watcher ignores every path it does not need. A missing index
/// needs no copy: Git reads an empty one, matching the unborn-branch case.
fn index_copy(cwd: &Path) -> anyhow::Result<PathBuf> {
    let git_dir = run_git_ok(cwd, ["rev-parse", "--absolute-git-dir"])?;
    let git_dir = PathBuf::from(git_dir.trim());
    let copy = git_dir.join(format!("orbit-index-read-{}", next_temp_suffix()));
    // A plain read + write, not `std::fs::copy`: on macOS the copy syscall
    // emits an FSEvents notification for the *source* `.git/index`, and the
    // workspace watcher keeps that path — so every Review load re-dirtied the
    // watch and re-triggered itself, forever.
    if let Ok(bytes) = std::fs::read(git_dir.join("index")) {
        std::fs::write(&copy, bytes)
            .map_err(|err| anyhow!("failed to snapshot the Git index: {err}"))?;
    }
    Ok(copy)
}

/// A per-process-unique suffix for temporary index copies (no `uuid` dep).
fn next_temp_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The merge base of `HEAD` and the repository's default branch (`main` /
/// `master`), when the current branch is not itself the default. Falls back
/// to `HEAD` when there is nothing to compare against.
fn branch_base(cwd: &Path) -> String {
    let current = current_branch(cwd);
    let branches = list_branches(cwd).unwrap_or_default();
    let default_branch = ["main", "master"].into_iter().find(|candidate| {
        current.as_deref() != Some(*candidate) && branches.iter().any(|b| b == candidate)
    });
    let Some(default_branch) = default_branch else {
        return head_or_empty(cwd);
    };
    match run_git_ok(cwd, ["merge-base", "HEAD", default_branch]) {
        Ok(base) if !base.trim().is_empty() => base.trim().to_owned(),
        _ => head_or_empty(cwd),
    }
}

fn head_or_empty(cwd: &Path) -> String {
    checkpoint::resolve(cwd, "HEAD").unwrap_or_else(|| EMPTY_TREE.to_owned())
}

fn diff_output(cwd: &Path, from: &str, to: &str, modes: &[&str]) -> anyhow::Result<String> {
    let output = command(cwd)
        .args([
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-ext-diff",
            "--no-color",
        ])
        .args(modes)
        .arg("--no-renames")
        .arg(from)
        .arg(to)
        .args(["--", "."])
        .output()
        .map_err(|err| anyhow!("failed to generate Git diff: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        bail!("{}", command_error(&output));
    }
}

fn ensure_repository(cwd: &Path) -> anyhow::Result<()> {
    let output = match command(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
    {
        Ok(output) => output,
        // A missing directory or unusable git binary is, for our purposes,
        // "not a repository".
        Err(_) => bail!("{}", tr!("git.not_a_repository")),
    };
    if output.status.success() {
        Ok(())
    } else {
        bail!("{}", tr!("git.not_a_repository"));
    }
}

// ── Git panel: working tree, staging, history, graph ───────────────────────

/// One changed file from `git status --porcelain`, with separate staged and
/// unstaged line deltas so it can appear in either list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub path: String,
    /// Previous path for a rename/copy.
    pub orig_path: Option<String>,
    /// Index (staged) status byte.
    pub index: char,
    /// Worktree (unstaged) status byte.
    pub worktree: char,
    pub staged_additions: u64,
    pub staged_deletions: u64,
    pub unstaged_additions: u64,
    pub unstaged_deletions: u64,
}

impl StatusRow {
    pub fn untracked(&self) -> bool {
        self.index == '?' || self.worktree == '?'
    }

    pub fn staged(&self) -> bool {
        matches!(self.index, 'M' | 'A' | 'D' | 'R' | 'C' | 'T')
    }

    pub fn unstaged(&self) -> bool {
        self.untracked() || matches!(self.worktree, 'M' | 'D' | 'T')
    }

    /// Badge for the unstaged list (`U` for untracked).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn change_badge(&self) -> char {
        if self.untracked() {
            'U'
        } else if self.worktree == ' ' {
            'M'
        } else {
            self.worktree
        }
    }

    /// Badge for the staged list.
    pub fn staged_badge(&self) -> char {
        if self.index == ' ' {
            'M'
        } else {
            self.index
        }
    }
}

/// Changed files with per-file line deltas. Staged and unstaged counts come
/// from separate `--numstat` runs; untracked files are counted from disk.
pub fn status_rows(cwd: &Path) -> Result<Vec<StatusRow>, String> {
    // Raw (untrimmed) output: `run_git` trims the whole stdout, which would eat
    // the leading space of the first porcelain line (" M path" → "M path") and
    // misread an unstaged edit as staged, with a path missing its first
    // character. `run_git_ok` keeps the output verbatim.
    let out = run_git_ok(
        cwd,
        [
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
    )
    .map_err(|err| err.to_string())?;
    let unstaged_stats = numstat_map(cwd, false);
    let staged_stats = numstat_map(cwd, true);
    let mut rows = Vec::new();
    for line in out.lines() {
        let bytes = line.as_bytes();
        if bytes.len() < 3 {
            continue;
        }
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        if index == '!' {
            continue;
        }
        let rest = &line[3..];
        let (path, orig_path) = match rest.split_once(" -> ") {
            Some((from, to)) => (to.to_string(), Some(from.to_string())),
            None => (rest.to_string(), None),
        };
        let (staged_additions, staged_deletions) =
            staged_stats.get(&path).copied().unwrap_or((0, 0));
        let (unstaged_additions, unstaged_deletions) = if index == '?' {
            (untracked_line_count(cwd, &path), 0)
        } else {
            unstaged_stats.get(&path).copied().unwrap_or((0, 0))
        };
        rows.push(StatusRow {
            path,
            orig_path,
            index,
            worktree,
            staged_additions,
            staged_deletions,
            unstaged_additions,
            unstaged_deletions,
        });
    }
    Ok(rows)
}

fn numstat_map(cwd: &Path, cached: bool) -> HashMap<String, (u64, u64)> {
    let mut args = vec![
        "-c",
        "core.quotePath=false",
        "diff",
        "--numstat",
        "--no-renames",
    ];
    if cached {
        args.push("--cached");
    }
    args.push("--");
    let out = run_git(cwd, &args).unwrap_or_default();
    out.lines()
        .filter_map(|line| {
            let mut columns = line.splitn(3, '\t');
            let added = columns.next()?;
            let removed = columns.next()?;
            let path = columns.next()?.to_string();
            Some((path, (added.parse().ok()?, removed.parse().ok()?)))
        })
        .collect()
}

/// Lines in an untracked file (bounded; binary and oversized files read 0).
fn untracked_line_count(cwd: &Path, path: &str) -> u64 {
    const CAP: u64 = 2 * 1024 * 1024;
    let full = cwd.join(path);
    let Ok(metadata) = std::fs::metadata(&full) else {
        return 0;
    };
    if !metadata.is_file() || metadata.len() > CAP {
        return 0;
    }
    let Ok(bytes) = std::fs::read(&full) else {
        return 0;
    };
    if bytes.contains(&0) {
        return 0;
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    if bytes.last().is_some_and(|byte| *byte != b'\n') {
        newlines + 1
    } else {
        newlines
    }
}

pub fn stage_paths(cwd: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    run_git(cwd, &args).map(|_| ())
}

pub fn stage_all(cwd: &Path) -> Result<(), String> {
    run_git(cwd, &["add", "-A", "--", "."]).map(|_| ())
}

pub fn unstage_paths(cwd: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["reset", "-q", "--"];
    args.extend(paths.iter().map(String::as_str));
    run_git(cwd, &args).map(|_| ())
}

pub fn unstage_all(cwd: &Path) -> Result<(), String> {
    run_git(cwd, &["reset", "-q"]).map(|_| ())
}

/// Discard working-tree changes: tracked files restore from the index, and
/// untracked files are removed. The UI confirms before calling this.
pub fn discard_paths(cwd: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut restore = vec!["checkout", "--"];
    restore.extend(paths.iter().map(String::as_str));
    let _ = run_git(cwd, &restore);
    let mut clean = vec!["clean", "-fdq", "--"];
    clean.extend(paths.iter().map(String::as_str));
    let _ = run_git(cwd, &clean);
    Ok(())
}

/// Discard every unstaged change: tracked files restore from the index and
/// untracked files are removed. Staged changes are left untouched. The UI
/// confirms before calling this.
pub fn discard_unstaged(cwd: &Path) -> Result<(), String> {
    // Restore the working tree from the index (so an unborn branch, whose
    // tracked files have no committed copy, simply fails harmlessly) and drop
    // the untracked files the working tree added.
    let _ = run_git(cwd, &["checkout", "--", "."]);
    run_git(cwd, &["clean", "-fdq"]).map(|_| ())
}

/// The staged patch text (for commit-message generation).
pub fn staged_patch(cwd: &Path) -> Result<String, String> {
    run_git(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--cached",
            "--no-color",
            "--no-renames",
            "--",
        ],
    )
}

/// The unstaged patch text (for commit-message generation).
pub fn unstaged_patch(cwd: &Path) -> Result<String, String> {
    run_git(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-renames",
            "--",
        ],
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

/// One commit for the History list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEntry {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub author_email: String,
    pub relative: String,
    pub subject: String,
    pub refs: Vec<RefLabel>,
}

pub fn history(cwd: &Path, limit: usize, skip: usize) -> Result<Vec<CommitEntry>, String> {
    let format = "%H%x1f%h%x1f%an%x1f%ae%x1f%ar%x1f%s%x1f%D";
    let out = run_git(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "log",
            "--date-order",
            &format!("--max-count={limit}"),
            &format!("--skip={skip}"),
            &format!("--format={format}"),
        ],
    )?;
    Ok(parse_history(&out))
}

fn parse_history(out: &str) -> Vec<CommitEntry> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.split('\u{1f}');
            let hash = fields.next()?.to_string();
            if hash.is_empty() {
                return None;
            }
            let short = fields.next()?.to_string();
            let author = fields.next()?.to_string();
            let author_email = fields.next()?.to_string();
            let relative = fields.next()?.to_string();
            let subject = fields.next()?.to_string();
            let refs = parse_refs(fields.next().unwrap_or(""));
            Some(CommitEntry {
                hash,
                short,
                author,
                author_email,
                relative,
                subject,
                refs,
            })
        })
        .collect()
}

/// Recent commits authored by `author`, newest first, for the commit-message
/// style sample.
pub fn history_by_author(
    cwd: &Path,
    author: &str,
    limit: usize,
) -> Result<Vec<CommitEntry>, String> {
    let format = "%H%x1f%h%x1f%an%x1f%ae%x1f%ar%x1f%s%x1f%D";
    let out = run_git(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "log",
            "--date-order",
            &format!("--max-count={limit}"),
            &format!("--author={author}"),
            &format!("--format={format}"),
        ],
    )?;
    Ok(parse_history(&out))
}

/// The configured `user.name`, used to separate the author's own commits from
/// the rest of the repository's when sampling commit style.
pub fn user_name(cwd: &Path) -> Option<String> {
    run_git(cwd, &["config", "user.name"])
        .ok()
        .filter(|name| !name.is_empty())
}

/// A file's content at HEAD, for the commit-message prompt's ORIGINAL CODE
/// context. `None` when the path did not exist at HEAD (a new file) or looks
/// binary.
pub fn file_at_head(cwd: &Path, path: &str) -> Option<String> {
    let content = run_git(cwd, &["show", &format!("HEAD:{path}")]).ok()?;
    (!content.is_empty() && !content.contains('\u{0}')).then_some(content)
}

// ── History: commit detail, file history, filters ──────────────────────────

/// One changed file inside a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    /// A/M/D/T (renames are normalized away, like the rest of the page).
    pub status: char,
    pub additions: u64,
    pub deletions: u64,
}

/// Everything the commit-detail panel shows for one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub hash: String,
    pub short: String,
    pub subject: String,
    /// Everything after the subject line, trimmed.
    pub body: String,
    pub author: String,
    pub author_email: String,
    pub author_date: String,
    pub committer: String,
    pub committer_date: String,
    pub parents: Vec<String>,
    pub refs: Vec<RefLabel>,
    pub files: Vec<CommitFile>,
}

/// Full metadata plus the changed-file list for one commit.
pub fn commit_detail(cwd: &Path, rev: &str) -> Result<CommitDetail, String> {
    let format =
        "%H\u{1f}%h\u{1f}%s\u{1f}%an\u{1f}%ae\u{1f}%aI\u{1f}%cn\u{1f}%ce\u{1f}%cI\u{1f}%P\u{1f}%D\u{1f}%b";
    let out = run_git(cwd, &["show", "-s", &format!("--format={format}"), rev])?;
    let mut fields = out.splitn(12, '\u{1f}');
    let hash = fields.next().unwrap_or_default().to_string();
    if hash.is_empty() {
        return Err(tr!("git_panel.commit_unavailable"));
    }
    let detail = CommitDetail {
        short: fields.next().unwrap_or_default().to_string(),
        subject: fields.next().unwrap_or_default().to_string(),
        author: fields.next().unwrap_or_default().to_string(),
        author_email: fields.next().unwrap_or_default().to_string(),
        author_date: fields.next().unwrap_or_default().to_string(),
        committer: fields.next().unwrap_or_default().to_string(),
        committer_date: fields.next().unwrap_or_default().to_string(),
        parents: fields
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        refs: parse_refs(fields.next().unwrap_or("")),
        body: fields.next().unwrap_or("").trim().to_string(),
        files: commit_files(cwd, &hash)?,
        hash,
    };
    Ok(detail)
}

/// The changed files of a commit with numstat counts, in Git's order.
fn commit_files(cwd: &Path, rev: &str) -> Result<Vec<CommitFile>, String> {
    let name_status = run_git(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "show",
            "--name-status",
            "--format=",
            "--no-renames",
            rev,
        ],
    )?;
    let numstat = run_git(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "show",
            "--numstat",
            "--format=",
            "--no-renames",
            rev,
        ],
    )?;
    let mut stats: HashMap<String, (u64, u64)> = HashMap::new();
    for line in numstat.lines() {
        let mut columns = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        stats.insert(
            path.to_string(),
            (added.parse().unwrap_or(0), removed.parse().unwrap_or(0)),
        );
    }
    let mut files = Vec::new();
    for line in name_status.lines() {
        let Some((status, path)) = line.split_once('\t') else {
            continue;
        };
        let (additions, deletions) = stats.get(path).copied().unwrap_or((0, 0));
        files.push(CommitFile {
            path: path.to_string(),
            status: status.chars().next().unwrap_or('M'),
            additions,
            deletions,
        });
    }
    Ok(files)
}

/// Filters the History tab applies before `git log` runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    /// Include every local branch (`--all`), not just the current one.
    pub all_branches: bool,
    /// Only commits whose author matches this string.
    pub author: Option<String>,
    /// Only commits touching this path.
    pub path: Option<String>,
    /// Follow renames when `path` is set (`--follow`).
    pub follow: bool,
    /// Only commits whose message matches this substring (case-insensitive).
    pub grep: Option<String>,
}

impl HistoryFilter {
    /// Whether any filter is active (drives the "filtered" affordance).
    pub fn is_active(&self) -> bool {
        self.all_branches
            || self.author.as_deref().is_some_and(|v| !v.is_empty())
            || self.path.as_deref().is_some_and(|v| !v.is_empty())
            || self.grep.as_deref().is_some_and(|v| !v.is_empty())
    }
}

/// `git log` under a [`HistoryFilter`], newest first.
pub fn history_filtered(
    cwd: &Path,
    filter: &HistoryFilter,
    limit: usize,
    skip: usize,
) -> Result<Vec<CommitEntry>, String> {
    let format = "%H%x1f%h%x1f%an%x1f%ae%x1f%ar%x1f%s%x1f%D";
    let mut args: Vec<String> = ["-c", "core.quotePath=false", "log", "--date-order"]
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    if filter.all_branches {
        args.push("--all".to_string());
    }
    if let Some(author) = filter.author.as_deref().filter(|v| !v.trim().is_empty()) {
        args.push(format!("--author={author}"));
    }
    if let Some(grep) = filter.grep.as_deref().filter(|v| !v.trim().is_empty()) {
        args.push("-i".to_string());
        args.push(format!("--grep={grep}"));
    }
    args.push(format!("--max-count={limit}"));
    args.push(format!("--skip={skip}"));
    args.push(format!("--format={format}"));
    let path = filter.path.as_deref().filter(|v| !v.trim().is_empty());
    if let Some(path) = path {
        if filter.follow {
            args.push("--follow".to_string());
        }
        args.push("--".to_string());
        args.push(path.to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_git(cwd, &refs)?;
    Ok(parse_history(&out))
}

fn parse_refs(decor: &str) -> Vec<RefLabel> {
    decor
        .split(", ")
        .filter(|part| !part.is_empty())
        .map(|raw| {
            if let Some(branch) = raw.strip_prefix("HEAD -> ") {
                RefLabel {
                    name: branch.to_string(),
                    kind: RefKind::Head,
                }
            } else if let Some(tag) = raw.strip_prefix("tag: ") {
                RefLabel {
                    name: tag.to_string(),
                    kind: RefKind::Tag,
                }
            } else if raw.contains('/') {
                RefLabel {
                    name: raw.to_string(),
                    kind: RefKind::Remote,
                }
            } else {
                RefLabel {
                    name: raw.to_string(),
                    kind: RefKind::Branch,
                }
            }
        })
        .collect()
}

// ── Git panel: the all-branches graph ──────────────────────────────────────

/// One commit for the Graph tab: the History fields plus parent hashes, so the
/// UI can lay out branch lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCommit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub author_email: String,
    pub relative: String,
    pub subject: String,
    pub parents: Vec<String>,
    pub refs: Vec<RefLabel>,
}

/// Every local branch's commits, newest first, each with its parents. Unlike
/// [`history`] (the current branch only), the Graph tab shows all local
/// branches so merges and parallel work get their own lanes. Internal refs
/// (remotes, tags, checkpoints) are deliberately excluded.
pub fn graph_history(cwd: &Path, limit: usize, all_refs: bool) -> Result<Vec<GraphCommit>, String> {
    let format = "%H%x1f%h%x1f%an%x1f%ae%x1f%ar%x1f%s%x1f%P%x1f%D";
    let mut args = vec!["-c", "core.quotePath=false", "log", "--branches", "HEAD"];
    // "All refs" extends the graph to remote-tracking branches and tags, so
    // commits that only exist on a teammate's branch or a release tag appear.
    if all_refs {
        args.push("--remotes");
        args.push("--tags");
    }
    let max_count = format!("--max-count={limit}");
    let format_arg = format!("--format={format}");
    args.push("--date-order");
    args.push(&max_count);
    args.push(&format_arg);
    let out = run_git(cwd, &args)?;
    Ok(out.lines().filter_map(parse_graph_commit).collect())
}

fn parse_graph_commit(line: &str) -> Option<GraphCommit> {
    let mut fields = line.split('\u{1f}');
    let hash = fields.next()?.to_string();
    if hash.is_empty() {
        return None;
    }
    let short = fields.next()?.to_string();
    let author = fields.next()?.to_string();
    let author_email = fields.next()?.to_string();
    let relative = fields.next()?.to_string();
    let subject = fields.next()?.to_string();
    let parents = fields
        .next()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let refs = parse_refs(fields.next().unwrap_or(""));
    Some(GraphCommit {
        hash,
        short,
        author,
        author_email,
        relative,
        subject,
        parents,
        refs,
    })
}

/// A laid-out Graph row: the commit plus the lane geometry for one line of the
/// drawing. Lanes are zero-based columns; the renderer maps them to x pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub commit: GraphCommit,
    /// Column the commit's node sits in.
    pub node_lane: usize,
    /// Total columns this row occupies (at least `node_lane + 1`).
    pub lane_count: usize,
    /// Columns with a line spanning the full row height (every lane that
    /// merely passes the commit by).
    pub verticals: Vec<usize>,
    /// Whether a line enters the node from the row above (false for a branch
    /// tip that starts here).
    pub node_has_incoming: bool,
    /// Columns each parent connects to, drawn from the node to the row below.
    pub parents: Vec<usize>,
}

/// Lay out [`GraphCommit`]s (newest first, as `git log` returns them) into
/// per-row lane geometry.
///
/// A lane holds the hash of the commit it is still waiting to print. A commit
/// claims the lane that already expects it, or takes a new one when it is a
/// branch tip. Each parent then reuses the node's lane when it is free and
/// otherwise takes the nearest free column to the right, which keeps branch
/// lines vertical wherever possible. Parents that fall outside the fetched
/// window are ignored so an old root doesn't trail a line into nothing.
pub fn layout_graph(commits: &[GraphCommit]) -> Vec<GraphRow> {
    use std::collections::HashSet;

    let known: HashSet<&str> = commits.iter().map(|commit| commit.hash.as_str()).collect();
    // lane -> hash of the commit still expected in it (None = free column).
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let node_lane = match lanes
            .iter()
            .position(|lane| lane.as_deref() == Some(commit.hash.as_str()))
        {
            Some(lane) => lane,
            None => {
                lanes.push(None);
                lanes.len() - 1
            }
        };
        let top = lanes.clone();
        let incoming = top[node_lane].is_some();
        lanes[node_lane] = None;

        let visible: Vec<&String> = commit
            .parents
            .iter()
            .filter(|parent| known.contains(parent.as_str()))
            .collect();
        let mut parents = Vec::with_capacity(visible.len());
        for (ix, parent) in visible.iter().enumerate() {
            if let Some(lane) = lanes
                .iter()
                .position(|slot| slot.as_deref() == Some(parent.as_str()))
            {
                parents.push(lane);
                continue;
            }
            let lane = if ix == 0 && lanes[node_lane].is_none() {
                node_lane
            } else {
                match lanes
                    .iter()
                    .enumerate()
                    .skip(node_lane + 1)
                    .find(|(_, slot)| slot.is_none())
                    .map(|(lane, _)| lane)
                {
                    Some(lane) => lane,
                    None => {
                        lanes.push(None);
                        lanes.len() - 1
                    }
                }
            };
            lanes[lane] = Some(parent.to_string());
            parents.push(lane);
        }

        // Only lanes that were already active above this row pass through;
        // a lane born here grows out of the node instead (see `parents`).
        let verticals = (0..lanes.len())
            .filter(|&lane| lane != node_lane && top.get(lane).is_some_and(|slot| slot.is_some()))
            .collect();
        // Drop trailing free columns, but never the node's own column, so the
        // node can't fall outside the gutter.
        while lanes.len() > node_lane + 1 && lanes.last() == Some(&None) {
            lanes.pop();
        }
        let lane_count = lanes
            .len()
            .max(node_lane + 1)
            .max(parents.iter().map(|lane| lane + 1).max().unwrap_or(0));

        rows.push(GraphRow {
            commit: commit.clone(),
            node_lane,
            lane_count,
            verticals,
            node_has_incoming: incoming,
            parents,
        });
    }
    rows
}

/// A hosted forge whose commit permalink shape is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forge {
    Github,
    Gitlab,
    Bitbucket,
}

/// The `origin` remote mapped to an HTTPS web base, when it points at a forge
/// we can build a commit permalink for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWeb {
    pub base: String,
    pub forge: Forge,
    /// Repository owner (the segment before the repo name).
    pub owner: String,
    /// Repository name (the last path segment).
    pub name: String,
}

impl RemoteWeb {
    /// The browser URL for one commit on this remote.
    pub fn commit_url(&self, hash: &str) -> String {
        match self.forge {
            Forge::Github => format!("{}/commit/{hash}", self.base),
            Forge::Gitlab => format!("{}/-/commit/{hash}", self.base),
            Forge::Bitbucket => format!("{}/commits/{hash}", self.base),
        }
    }
}

/// Resolve `origin` to a web base. Handles the three remote URL shapes users
/// actually have: `git@host:owner/repo.git`, `https://host/owner/repo(.git)`,
/// and `ssh://git@host[:port]/owner/repo(.git)`. Only known forges qualify;
/// self-hosted or local remotes get `None` rather than a link that may 404.
pub fn remote_web(cwd: &Path) -> Option<RemoteWeb> {
    let raw = run_git(cwd, &["remote", "get-url", "origin"]).ok()?;
    let (host, path) = normalize_remote(&raw)?;
    let forge = match host.as_str() {
        "github.com" | "www.github.com" => Forge::Github,
        "gitlab.com" | "www.gitlab.com" => Forge::Gitlab,
        "bitbucket.org" | "www.bitbucket.org" => Forge::Bitbucket,
        _ => return None,
    };
    let (owner, name) = path.rsplit_once('/')?;
    Some(RemoteWeb {
        base: format!("https://{host}/{path}"),
        forge,
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// Turn a Git remote URL into `(host, path)`, where `path` is the cleaned
/// repository path (`owner/repo`). Strips credentials, a port, a trailing
/// slash, and a `.git` suffix.
fn normalize_remote(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return None;
    }
    let (host, path) = if let Some((_, rest)) = raw.split_once("://") {
        // scheme://[user[:token]@]host[:port]/path
        let rest = rest
            .rsplit_once('@')
            .map(|(_, after)| after)
            .unwrap_or(rest);
        let (hostport, path) = rest.split_once('/')?;
        let host = hostport.split(':').next()?;
        (host.to_string(), path.to_string())
    } else {
        // scp-like: [user@]host:path
        let after_user = raw.rsplit_once('@').map(|(_, after)| after).unwrap_or(raw);
        let (host, path) = after_user.split_once(':')?;
        (host.to_string(), path.to_string())
    };
    let path = path.trim_matches('/').trim_end_matches(".git");
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host, path.to_string()))
}

/// `(ahead, behind)` relative to the branch's upstream, or `None`.
pub fn ahead_behind(cwd: &Path) -> Option<(usize, usize)> {
    let out = run_git(
        cwd,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    )
    .ok()?;
    let mut parts = out.split_whitespace();
    let ahead = parts.next()?.parse().ok()?;
    let behind = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

/// Whether the repository has at least one commit (HEAD resolves).
pub fn has_commits(cwd: &Path) -> bool {
    run_git(cwd, &["rev-parse", "--verify", "HEAD"]).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint;
    use std::fs;

    fn git_ok(cwd: &Path, args: &[&str]) {
        let status = command(cwd).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// Reads must not take optional index locks, or `git status` rewrites
    /// `.git/index` and the workspace watcher re-fires endlessly.
    #[test]
    fn git_runs_without_optional_locks() {
        let envs: Vec<_> = command(Path::new("."))
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(|v| v.to_os_string())))
            .collect();
        assert!(
            envs.iter().any(|(key, value)| key == "GIT_OPTIONAL_LOCKS"
                && value.as_deref() == Some(OsStr::new("0"))),
            "GIT_OPTIONAL_LOCKS=0 must be set on every git invocation"
        );
    }

    fn repository() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        // A pid+nanos sum can collide on a coarse clock, letting parallel
        // tests share (and tear down) one repo; a counter is unique per run.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orbit-git-collect-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init", "--quiet", "--initial-branch=main"]);
        git_ok(&root, &["config", "user.name", "Orbit Test"]);
        git_ok(&root, &["config", "user.email", "orbit@example.com"]);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "fn baseline() {}\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "--quiet", "-m", "baseline"]);
        root
    }

    #[test]
    fn status_rows_split_staged_and_unstaged_with_counts() {
        let root = repository();
        fs::write(root.join("src/lib.rs"), "fn staged() {}\n").unwrap();
        git_ok(&root, &["add", "src/lib.rs"]);
        fs::write(root.join("src/other.rs"), "fn untracked() {}\n").unwrap();

        let rows = status_rows(&root).unwrap();
        let staged = rows.iter().find(|row| row.path == "src/lib.rs").unwrap();
        assert!(staged.staged());
        assert_eq!(staged.staged_badge(), 'M');
        assert_eq!(staged.staged_additions, 1);
        let untracked = rows.iter().find(|row| row.path == "src/other.rs").unwrap();
        assert!(untracked.untracked());
        assert_eq!(untracked.change_badge(), 'U');
        assert_eq!(untracked.unstaged_additions, 1);

        // Stage everything, then everything is staged and nothing unstaged.
        stage_all(&root).unwrap();
        let rows = status_rows(&root).unwrap();
        assert!(rows.iter().all(StatusRow::staged));
        assert!(!rows.iter().any(StatusRow::unstaged));
        fs::remove_dir_all(root).ok();
    }

    /// The Git page's Changes tab reloads its diff on every `git status`
    /// refresh; the staged/unstaged sources call `index_tree`. If that rewrote
    /// `.git/index`, the workspace watcher would fire, reload the diff, and
    /// rewrite the index again — forever. `git write-tree` does rewrite the
    /// index (its cache-tree extension) even with `GIT_OPTIONAL_LOCKS=0`.
    #[test]
    fn index_tree_reads_without_rewriting_the_index() {
        use std::time::{Duration, SystemTime};

        let root = repository();
        let index = root.join(".git/index");
        // Pin the mtime so a rewrite is detectable regardless of clock or
        // filesystem timestamp granularity.
        let pinned = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&index)
            .unwrap()
            .set_modified(pinned)
            .unwrap();

        let tree = index_tree(&root).unwrap();
        assert!(!tree.is_empty());
        assert_eq!(
            fs::metadata(&index).unwrap().modified().unwrap(),
            pinned,
            "index_tree must read the index without rewriting it"
        );
        // The throwaway copy must not linger next to the real index.
        let leftovers: Vec<_> = fs::read_dir(root.join(".git"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("orbit-index-read-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary index left behind: {leftovers:?}"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn remote_urls_normalize_to_host_and_path() {
        // scp-like, https, ssh with port, trailing slash, credentials.
        assert_eq!(
            normalize_remote("git@github.com:orbit/pi.git"),
            Some(("github.com".into(), "orbit/pi".into()))
        );
        assert_eq!(
            normalize_remote("https://github.com/orbit/pi.git"),
            Some(("github.com".into(), "orbit/pi".into()))
        );
        assert_eq!(
            normalize_remote("ssh://git@github.com:22/orbit/pi.git"),
            Some(("github.com".into(), "orbit/pi".into()))
        );
        assert_eq!(
            normalize_remote("https://user:token@github.com/orbit/pi/"),
            Some(("github.com".into(), "orbit/pi".into()))
        );
        // Local paths and unknown hosts yield no base.
        assert_eq!(normalize_remote("/tmp/not-a-remote"), None);
        assert_eq!(normalize_remote("file:///tmp/repo"), None);
    }

    #[test]
    fn commit_urls_follow_the_forge() {
        let github = RemoteWeb {
            base: "https://github.com/orbit/pi".into(),
            forge: Forge::Github,
            owner: "orbit".into(),
            name: "pi".into(),
        };
        let gitlab = RemoteWeb {
            base: "https://gitlab.com/orbit/pi".into(),
            forge: Forge::Gitlab,
            owner: "orbit".into(),
            name: "pi".into(),
        };
        let bitbucket = RemoteWeb {
            base: "https://bitbucket.org/orbit/pi".into(),
            forge: Forge::Bitbucket,
            owner: "orbit".into(),
            name: "pi".into(),
        };
        assert_eq!(
            github.commit_url("abc123"),
            "https://github.com/orbit/pi/commit/abc123"
        );
        assert_eq!(
            gitlab.commit_url("abc123"),
            "https://gitlab.com/orbit/pi/-/commit/abc123"
        );
        assert_eq!(
            bitbucket.commit_url("abc123"),
            "https://bitbucket.org/orbit/pi/commits/abc123"
        );
    }

    #[test]
    fn history_includes_the_baseline_commit() {
        let root = repository();
        let history = history(&root, 20, 0).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].subject, "baseline");
        assert!(!history[0].short.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn graph_history_spans_branches_and_lays_out_the_merge() {
        let root = repository();
        git_ok(&root, &["switch", "-c", "feature"]);
        fs::write(root.join("src/lib.rs"), "fn feature() {}\n").unwrap();
        git_ok(&root, &["commit", "--quiet", "-am", "feature work"]);
        git_ok(&root, &["switch", "main"]);
        git_ok(
            &root,
            &[
                "merge",
                "--no-ff",
                "--quiet",
                "-m",
                "merge feature",
                "feature",
            ],
        );

        let commits = graph_history(&root, 50, false).unwrap();
        assert_eq!(commits.len(), 3, "all local branches are reachable");
        assert!(commits.iter().any(|c| c.subject == "merge feature"));

        let rows = layout_graph(&commits);
        assert_eq!(rows.len(), commits.len());

        let merge = rows
            .iter()
            .find(|row| row.commit.subject == "merge feature")
            .unwrap();
        // The merge forks: two parents, each on its own lane.
        assert_eq!(merge.parents.len(), 2);
        assert_ne!(merge.parents[0], merge.parents[1]);

        for row in &rows {
            assert!(row.node_lane < row.lane_count, "{:?}", row.commit.subject);
            assert!(row.verticals.iter().all(|lane| *lane < row.lane_count));
            assert!(row.parents.iter().all(|lane| *lane < row.lane_count));
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn layout_graph_ignores_parents_outside_the_window() {
        // A truncated window: the newest commit's parent is not in the list.
        let commits = vec![GraphCommit {
            hash: "a".into(),
            short: "a".into(),
            author: "Orbit".into(),
            author_email: "orbit@example.com".into(),
            relative: "now".into(),
            subject: "head".into(),
            parents: vec!["missing".into()],
            refs: Vec::new(),
        }];
        let rows = layout_graph(&commits);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].parents.is_empty());
        assert_eq!(rows[0].lane_count, 1);
    }

    #[test]
    fn layout_graph_handles_an_octopus_merge() {
        let commit = |hash: &str, parents: Vec<&str>| GraphCommit {
            hash: hash.into(),
            short: hash.into(),
            author: "Orbit".into(),
            author_email: "orbit@example.com".into(),
            relative: "now".into(),
            subject: hash.into(),
            parents: parents.into_iter().map(str::to_string).collect(),
            refs: Vec::new(),
        };
        let commits = vec![
            commit("m", vec!["a", "b", "c"]),
            commit("a", vec![]),
            commit("b", vec![]),
            commit("c", vec![]),
        ];
        let rows = layout_graph(&commits);
        let merge = &rows[0];
        assert_eq!(merge.parents.len(), 3);
        assert_ne!(merge.parents[0], merge.parents[1]);
        assert_ne!(merge.parents[1], merge.parents[2]);
        assert_ne!(merge.parents[0], merge.parents[2]);
        assert!(merge.lane_count >= 3);
    }

    #[test]
    fn graph_all_refs_includes_remote_only_commits() {
        let root = repository();
        git_ok(&root, &["switch", "-c", "temp"]);
        fs::write(root.join("remote.txt"), "remote\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "--quiet", "-m", "remote only"]);
        let hash = run_git(&root, &["rev-parse", "HEAD"]).unwrap();
        git_ok(&root, &["switch", "main"]);
        git_ok(&root, &["branch", "-D", "temp"]);
        // Keep the commit alive through a remote-tracking ref only.
        git_ok(&root, &["update-ref", "refs/remotes/origin/feature", &hash]);

        let local = graph_history(&root, 50, false).unwrap();
        assert!(!local.iter().any(|commit| commit.subject == "remote only"));
        let all = graph_history(&root, 50, true).unwrap();
        assert!(all.iter().any(|commit| commit.subject == "remote only"));
        let rows = layout_graph(&all);
        assert_eq!(rows.len(), all.len());
        for row in &rows {
            assert!(row.node_lane < row.lane_count);
        }
        fs::remove_dir_all(root).ok();
    }

    fn summary(data: &ReviewDiff) -> (usize, u64, u64) {
        let mut files = 0;
        let mut additions = 0;
        let mut deletions = 0;
        for line in data.numstat.lines().filter(|line| !line.is_empty()) {
            let mut fields = line.splitn(3, '\t');
            additions += fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            deletions += fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            files += 1;
        }
        (files, additions, deletions)
    }

    fn collect(root: &Path, source: Source, session: Option<&str>) -> ReviewDiff {
        collect_review_diff(root, source, session).unwrap()
    }

    #[test]
    fn source_modes_compare_consistent_git_snapshots() {
        let root = repository();
        git_ok(&root, &["switch", "-c", "feature"]);
        fs::write(root.join("src/lib.rs"), "fn committed() {}\n").unwrap();
        git_ok(&root, &["add", "src/lib.rs"]);
        git_ok(&root, &["commit", "--quiet", "-m", "feature"]);
        fs::write(
            root.join("src/lib.rs"),
            "fn committed() {}\nfn staged() {}\n",
        )
        .unwrap();
        git_ok(&root, &["add", "src/lib.rs"]);
        fs::write(
            root.join("src/lib.rs"),
            "fn committed() {}\nfn staged() {}\nfn unstaged() {}\n",
        )
        .unwrap();
        fs::write(root.join("new file.txt"), "untracked\n").unwrap();

        let committed = collect(&root, Source::Committed, None);
        let staged = collect(&root, Source::Staged, None);
        let unstaged = collect(&root, Source::Unstaged, None);
        let uncommitted = collect(&root, Source::Uncommitted, None);
        let branch = collect(&root, Source::Branch, None);

        assert_eq!(summary(&committed), (1, 1, 1));
        assert_eq!(summary(&staged), (1, 1, 0));
        assert_eq!(summary(&unstaged), (2, 2, 0), "unstaged includes untracked");
        assert_eq!(summary(&uncommitted), (2, 3, 0));
        assert_eq!(summary(&branch), (2, 4, 1));
        assert!(complete_patch(&uncommitted).contains("fn unstaged"));
        fs::remove_dir_all(root).ok();
    }

    fn complete_patch(data: &ReviewDiff) -> String {
        data.patch.clone()
    }

    #[test]
    fn last_turn_uses_captured_checkpoints_not_the_live_worktree() {
        let root = repository();
        let session = "git-collect-session";
        checkpoint::capture_turn(&root, session, 0).unwrap();
        checkpoint::capture_turn_start(&root, session, 1).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "fn baseline() {}\nfn from_turn() {}\n",
        )
        .unwrap();
        checkpoint::capture_turn(&root, session, 1).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "fn baseline() {}\nfn from_turn() {}\nfn after_turn() {}\n",
        )
        .unwrap();

        let data = collect(&root, Source::LastTurn { turn_count: 1 }, Some(session));
        assert_eq!(summary(&data), (1, 1, 0));
        assert!(data.patch.contains("from_turn"));
        assert!(!data.patch.contains("after_turn"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn last_turn_without_checkpoints_reports_an_error() {
        let root = repository();
        let error = collect_review_diff(
            &root,
            Source::LastTurn { turn_count: 1 },
            Some("missing-session"),
        )
        .unwrap_err();
        assert!(error.contains("checkpoint"), "{error}");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn off_repo_reports_not_a_repository() {
        let error = collect_review_diff(
            Path::new("/definitely/not/a/repo/orbit"),
            Source::Uncommitted,
            None,
        )
        .unwrap_err();
        let expected = tr!("git.not_a_repository");
        assert!(error.contains(expected.as_str()), "{error}");
    }

    #[test]
    fn status_rows_keeps_the_first_lines_index_column() {
        // The first porcelain line for a worktree edit starts with a space
        // (" M path"). `run_git` trims the whole stdout, so a trimmed read
        // would drop that column — misreading the edit as staged and the path
        // as "argo.lock" (missing its first character). Cargo.lock sorts first.
        let root = repository();
        fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
        git_ok(&root, &["add", "Cargo.lock"]);
        git_ok(&root, &["commit", "--quiet", "-m", "add lock"]);
        fs::write(root.join("Cargo.lock"), "version = 5\n").unwrap();

        let rows = status_rows(&root).unwrap();
        let row = rows.iter().find(|row| row.path == "Cargo.lock").unwrap();
        assert!(!row.staged(), "a worktree edit is not staged");
        assert!(row.unstaged());
        assert_eq!(row.change_badge(), 'M');
        assert_eq!(row.unstaged_additions, 1);
        assert_eq!(row.unstaged_deletions, 1);
        fs::remove_dir_all(root).ok();
    }

    /// A leased force push may overwrite a *diverged* remote, but never a
    /// remote that moved since the last fetch. This is the safety property that
    /// separates `--force-with-lease` from bare `--force`.
    #[test]
    fn force_push_uses_the_lease_and_refuses_a_stale_remote() {
        let base = repository();
        let stem = base.file_name().unwrap().to_string_lossy().into_owned();
        let parent = base.parent().unwrap().to_path_buf();
        let remote = parent.join(format!("{stem}-remote.git"));
        let clone = parent.join(format!("{stem}-clone"));

        git_ok(
            &base,
            &["init", "--bare", "--quiet", remote.to_str().unwrap()],
        );
        git_ok(
            &base,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_ok(&base, &["push", "--quiet", "-u", "origin", "main"]);

        let status = command(&base)
            .args([
                "clone",
                "--quiet",
                "--branch",
                "main",
                remote.to_str().unwrap(),
                clone.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "clone failed");
        git_ok(&clone, &["config", "user.name", "Other"]);
        git_ok(&clone, &["config", "user.email", "other@example.com"]);
        fs::write(clone.join("other.txt"), "other\n").unwrap();
        git_ok(&clone, &["add", "."]);
        git_ok(&clone, &["commit", "--quiet", "-m", "other"]);
        git_ok(&clone, &["push", "--quiet"]);

        // base diverges and fetches: the lease is satisfied, so a plain push
        // is rejected but the leased force push is allowed.
        fs::write(base.join("local.txt"), "local\n").unwrap();
        git_ok(&base, &["add", "."]);
        git_ok(&base, &["commit", "--quiet", "-m", "local"]);
        git_ok(&base, &["fetch", "--quiet"]);
        assert!(push(&base).is_err(), "a diverged push must be rejected");
        assert!(push_force_with_lease(&base).is_ok());

        // The remote moves again without base fetching; the lease is now stale
        // and the force push must be refused.
        fs::write(clone.join("other.txt"), "other2\n").unwrap();
        git_ok(&clone, &["commit", "--quiet", "-am", "other2"]);
        // base's leased force push rewrote the remote, so the clone's own push
        // needs to overwrite it too; this is test setup, not the property under
        // test.
        git_ok(&clone, &["push", "--quiet", "--force"]);
        fs::write(base.join("local.txt"), "local2\n").unwrap();
        git_ok(&base, &["commit", "--quiet", "-am", "local2"]);
        assert!(
            push_force_with_lease(&base).is_err(),
            "--force-with-lease must refuse a remote that moved since the last fetch"
        );

        fs::remove_dir_all(&base).ok();
        fs::remove_dir_all(&remote).ok();
        fs::remove_dir_all(&clone).ok();
    }

    #[test]
    fn commit_detail_reports_metadata_and_files() {
        let root = repository();
        let head = run_git(&root, &["rev-parse", "HEAD"]).unwrap();
        let detail = commit_detail(&root, &head).unwrap();
        assert_eq!(detail.subject, "baseline");
        assert_eq!(detail.author, "Orbit Test");
        assert!(detail.short.len() >= 7);
        let file = detail
            .files
            .iter()
            .find(|file| file.path == "src/lib.rs")
            .unwrap();
        assert_eq!(file.status, 'A');
        assert_eq!(file.additions, 1);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn history_follows_one_path_across_renames() {
        let root = repository();
        fs::write(
            root.join("src/lib.rs"),
            "fn baseline() {}\nfn second() {}\n",
        )
        .unwrap();
        git_ok(&root, &["commit", "--quiet", "-am", "touch lib"]);
        let entries = history_filtered(
            &root,
            &HistoryFilter {
                path: Some("src/lib.rs".into()),
                follow: true,
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert!(entries.iter().any(|commit| commit.subject == "touch lib"));
        assert!(entries.len() >= 2);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn history_filters_by_author_and_path() {
        let root = repository();
        let all = history_filtered(&root, &HistoryFilter::default(), 10, 0).unwrap();
        assert_eq!(all.len(), 1);

        let authored = history_filtered(
            &root,
            &HistoryFilter {
                author: Some("Orbit Test".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert_eq!(authored.len(), 1);

        let missing = history_filtered(
            &root,
            &HistoryFilter {
                author: Some("Nobody".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert!(missing.is_empty());

        let by_path = history_filtered(
            &root,
            &HistoryFilter {
                path: Some("src/lib.rs".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert_eq!(by_path.len(), 1);
        let no_path = history_filtered(
            &root,
            &HistoryFilter {
                path: Some("nope.txt".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert!(no_path.is_empty());
        fs::remove_dir_all(root).ok();
    }
}
