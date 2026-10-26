//! Turn checkpoints for a truthful **Last Turn** review.
//!
//! Adapted from Waku's `crates/waku-core/src/checkpoint.rs` (MIT). Before a
//! turn runs we snapshot the whole workspace (tracked *and* untracked files)
//! into a dangling commit with an isolated temporary index, then record it as
//! a ref under `refs/orbit/…`. The user's real index, HEAD, and working tree
//! are never modified.
//!
//! This is what lets Review diff exactly what one agent turn changed, even
//! after later edits, and keeps branch switches from being mis-attributed to
//! the turn. Ref names embed the pi session id so concurrent sessions in one
//! repository do not collide.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use anyhow::{anyhow, bail, Context as _};
use serde_json::Value;

use crate::git;

const TURN_START_METADATA_PREFIX: &str = "Orbit-Turn-Start: ";
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug)]
struct TurnStartMetadata {
    head: Option<String>,
    branch: Option<String>,
    refs: BTreeMap<String, String>,
}

impl TurnStartMetadata {
    fn to_value(&self) -> Value {
        let refs = self
            .refs
            .iter()
            .map(|(name, commit)| (name.clone(), Value::String(commit.clone())))
            .collect::<serde_json::Map<_, _>>();
        serde_json::json!({
            "head": self.head,
            "branch": self.branch,
            "refs": Value::Object(refs),
        })
    }

    fn from_value(value: &Value) -> Self {
        Self {
            head: value.get("head").and_then(Value::as_str).map(str::to_owned),
            branch: value
                .get("branch")
                .and_then(Value::as_str)
                .map(str::to_owned),
            refs: value
                .get("refs")
                .and_then(Value::as_object)
                .map(|object| {
                    object
                        .iter()
                        .filter_map(|(name, commit)| {
                            commit
                                .as_str()
                                .map(|commit| (name.clone(), commit.to_owned()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// Ref-safe session key: pi session ids are UUID-shaped, but never trust that
/// enough to build a ref name from unchecked text.
pub fn session_key(session: &str) -> String {
    session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn checkpoint_ref(session: &str, turn_count: usize) -> String {
    format!(
        "refs/orbit/session-{}-turn-{turn_count}",
        session_key(session)
    )
}

pub fn turn_start_ref(session: &str, turn_count: usize) -> String {
    format!(
        "refs/orbit/session-{}-turn-start-{turn_count}",
        session_key(session)
    )
}

pub fn turn_diff_base_ref(session: &str, turn_count: usize) -> String {
    format!(
        "refs/orbit/session-{}-turn-diff-{turn_count}",
        session_key(session)
    )
}

/// Snapshot the workspace as it is accepted for a turn, before the provider
/// starts. Distinct from the previous turn's ending checkpoint so a branch
/// switch or terminal edit between turns is not attributed to either response.
pub fn capture_turn_start(cwd: &Path, session: &str, turn_count: usize) -> anyhow::Result<()> {
    if !is_git_repository(cwd) {
        return Ok(());
    }

    let head = resolve(cwd, "HEAD");
    let branch = symbolic_head(cwd);
    let refs = repository_refs(cwd)?;
    let metadata = TurnStartMetadata {
        head: head.clone(),
        branch,
        refs,
    };
    let message = format!(
        "Orbit turn start snapshot\n\n{TURN_START_METADATA_PREFIX}{}",
        serde_json::to_string(&metadata.to_value())?
    );
    let mut parents = Vec::new();
    let mut seen = HashSet::new();
    if let Some(head) = head.as_ref() {
        if seen.insert(head.clone()) {
            parents.push(head.clone());
        }
    }
    for commit in metadata.refs.values() {
        if seen.insert(commit.clone()) {
            parents.push(commit.clone());
        }
    }
    let commit = capture_worktree_commit_from(cwd, head.as_deref(), &message, &parents)?;
    let start_ref = turn_start_ref(session, turn_count);
    let baseline_ref = checkpoint_ref(session, turn_count.saturating_sub(1));
    let mut commands = format!("update {start_ref} {commit}\n");
    if !has_ref(cwd, &baseline_ref) {
        commands.push_str(&format!("update {baseline_ref} {commit}\n"));
    }
    update_refs(cwd, commands)
}

/// Snapshot where a turn ended and record the branch-aware diff base the
/// Review `Last Turn` source compares against.
pub fn capture_turn(cwd: &Path, session: &str, turn_count: usize) -> anyhow::Result<()> {
    if !is_git_repository(cwd) {
        return Ok(());
    }
    let end_branch = symbolic_head(cwd);
    let end_head = resolve(cwd, "HEAD");
    let end_commit =
        capture_worktree_commit_from(cwd, end_head.as_deref(), "Orbit worktree snapshot", &[])?;
    git_output(
        cwd,
        [
            "update-ref",
            &checkpoint_ref(session, turn_count),
            &end_commit,
        ],
    )?;

    let start_ref = turn_start_ref(session, turn_count);
    if turn_count > 0 && has_ref(cwd, &start_ref) {
        prepare_turn_diff_base(
            cwd,
            session,
            turn_count,
            &start_ref,
            end_head.as_deref(),
            end_branch.as_deref(),
        )?;
    }
    Ok(())
}

/// Capture the current worktree and untracked files as a dangling commit,
/// using an isolated temporary index so nothing in the user's repository is
/// staged, unstaged, or mutated.
pub fn capture_worktree_commit(cwd: &Path) -> anyhow::Result<String> {
    if !is_git_repository(cwd) {
        bail!("worktree snapshots require a Git repository");
    }
    let head = resolve(cwd, "HEAD");
    capture_worktree_commit_from(cwd, head.as_deref(), "Orbit worktree snapshot", &[])
}

fn capture_worktree_commit_from(
    cwd: &Path,
    head: Option<&str>,
    message: &str,
    parents: &[String],
) -> anyhow::Result<String> {
    let common_dir = common_dir(cwd)?;
    let temporary_index = common_dir.join(format!("orbit-checkpoint-index-{}", next_temp_suffix()));

    let result = (|| {
        if let Some(head) = head {
            git_with_index(cwd, &temporary_index, ["read-tree", head])?;
        }
        git_with_index(cwd, &temporary_index, ["add", "-A", "--", "."])?;
        let tree = git_with_index(cwd, &temporary_index, ["write-tree"])?
            .trim()
            .to_owned();
        if tree.is_empty() {
            bail!("git write-tree returned no object id");
        }
        let mut arguments = vec![
            "commit-tree".to_owned(),
            tree,
            "-m".to_owned(),
            message.to_owned(),
        ];
        for parent in parents {
            arguments.push("-p".to_owned());
            arguments.push(parent.clone());
        }
        let commit = git_with_identity_and_index(cwd, &temporary_index, &arguments)?
            .trim()
            .to_owned();
        if commit.is_empty() {
            bail!("git commit-tree returned no object id");
        }
        Ok(commit)
    })();

    let _ = fs::remove_file(&temporary_index);
    let _ = fs::remove_file(temporary_index.with_extension("lock"));
    result
}

fn prepare_turn_diff_base(
    cwd: &Path,
    session: &str,
    turn_count: usize,
    start_ref: &str,
    end_head: Option<&str>,
    end_branch: Option<&str>,
) -> anyhow::Result<String> {
    let metadata = turn_start_metadata(cwd, start_ref)?;
    let same_line = match (metadata.branch.as_deref(), end_branch) {
        (Some(start), Some(end)) => start == end,
        (None, None) => metadata.head.as_deref() == end_head,
        _ => false,
    };

    let commit = if same_line {
        resolve(cwd, start_ref)
            .ok_or_else(|| anyhow!("turn starting checkpoint `{start_ref}` is unavailable"))?
    } else {
        let target_base = target_branch_start(cwd, start_ref, &metadata, end_head, end_branch)?;
        virtual_branch_start(cwd, start_ref, metadata.head.as_deref(), &target_base)?
    };
    git_output(
        cwd,
        [
            "update-ref",
            &turn_diff_base_ref(session, turn_count),
            &commit,
        ],
    )?;
    Ok(commit)
}

fn turn_start_metadata(cwd: &Path, start_ref: &str) -> anyhow::Result<TurnStartMetadata> {
    let message = git_output(cwd, ["show", "-s", "--format=%B", start_ref])?;
    let encoded = message
        .lines()
        .find_map(|line| line.strip_prefix(TURN_START_METADATA_PREFIX))
        .ok_or_else(|| anyhow!("turn starting checkpoint metadata is unavailable"))?;
    let value: Value =
        serde_json::from_str(encoded).context("invalid turn starting checkpoint metadata")?;
    Ok(TurnStartMetadata::from_value(&value))
}

fn target_branch_start(
    cwd: &Path,
    start_ref: &str,
    metadata: &TurnStartMetadata,
    end_head: Option<&str>,
    end_branch: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(branch) = end_branch {
        if let Some(commit) = metadata.refs.get(branch) {
            return Ok(commit.clone());
        }
    }
    let Some(end_head) = end_head else {
        return empty_tree_commit(cwd);
    };
    let new_commits = git_output(
        cwd,
        [
            "rev-list",
            "--first-parent",
            "--reverse",
            end_head,
            "--not",
            start_ref,
        ],
    )?;
    let Some(first_new_commit) = new_commits.lines().find(|line| !line.trim().is_empty()) else {
        return Ok(end_head.to_owned());
    };
    resolve(cwd, &format!("{first_new_commit}^1")).map_or_else(|| empty_tree_commit(cwd), Ok)
}

fn virtual_branch_start(
    cwd: &Path,
    start_ref: &str,
    start_head: Option<&str>,
    target_base: &str,
) -> anyhow::Result<String> {
    let Some(start_head) = start_head else {
        return Ok(target_base.to_owned());
    };
    if start_head == target_base {
        return resolve(cwd, start_ref)
            .ok_or_else(|| anyhow!("turn starting checkpoint `{start_ref}` is unavailable"));
    }
    if git_output(cwd, ["diff", "--name-only", start_head, start_ref])?
        .trim()
        .is_empty()
    {
        return Ok(target_base.to_owned());
    }

    // Recreate the state Git would have after carrying the user's pre-turn
    // dirty files onto the target branch. Comparing against the raw target tip
    // would otherwise attribute those already-present edits to the response.
    let output = git_output(
        cwd,
        [
            "merge-tree",
            "--write-tree",
            "--merge-base",
            start_head,
            target_base,
            start_ref,
        ],
    )?;
    let tree = output
        .lines()
        .next()
        .map(str::trim)
        .filter(|tree| !tree.is_empty())
        .ok_or_else(|| anyhow!("git merge-tree returned no tree"))?;
    commit_tree(cwd, tree, "Orbit turn diff base", &[])
}

fn empty_tree_commit(cwd: &Path) -> anyhow::Result<String> {
    commit_tree(cwd, EMPTY_TREE, "Orbit empty turn diff base", &[])
}

fn commit_tree(
    cwd: &Path,
    tree: &str,
    message: &str,
    parents: &[String],
) -> anyhow::Result<String> {
    let common_dir = common_dir(cwd)?;
    let temporary_index = common_dir.join(format!("orbit-checkpoint-index-{}", next_temp_suffix()));
    let mut arguments = vec![
        "commit-tree".to_owned(),
        tree.to_owned(),
        "-m".to_owned(),
        message.to_owned(),
    ];
    for parent in parents {
        arguments.push("-p".to_owned());
        arguments.push(parent.clone());
    }
    git_with_identity_and_index(cwd, &temporary_index, &arguments)
        .map(|commit| commit.trim().to_owned())
}

pub fn has_ref(cwd: &Path, git_ref: &str) -> bool {
    resolve(cwd, git_ref).is_some()
}

/// Highest turn number with an ending checkpoint ref for `session`.
///
/// Each turn's checkpoint lives in Git refs, so the `Last Turn` source is
/// available again the moment a session is reopened. Orbit recovers the same
/// fact by listing
/// them — this is what makes `Last Turn` survive a restart or session switch.
pub fn latest_turn(cwd: &Path, session: &str) -> Option<usize> {
    let prefix = format!("refs/orbit/session-{}-turn-", session_key(session));
    let output = git::run_git_ok(
        cwd,
        ["for-each-ref", "--format=%(refname)", &format!("{prefix}*")],
    )
    .ok()?;
    output
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix.as_str()))
        // `…-turn-start-N` and `…-turn-diff-N` are not ending checkpoints.
        .filter(|suffix| !suffix.starts_with("start-") && !suffix.starts_with("diff-"))
        .filter_map(|suffix| suffix.parse::<usize>().ok())
        .max()
}

/// Resolve a revision to a commit id, or `None`.
pub fn resolve(cwd: &Path, revision: &str) -> Option<String> {
    let output = git::command(cwd)
        .args(["rev-parse", "--verify", &format!("{revision}^{{commit}}")])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub fn has_head(cwd: &Path) -> bool {
    git::command(cwd)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn is_git_repository(cwd: &Path) -> bool {
    git::command(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn symbolic_head(cwd: &Path) -> Option<String> {
    let output = git::command(cwd)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|branch| !branch.is_empty())
}

fn repository_refs(cwd: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let output = git_output(
        cwd,
        [
            "for-each-ref",
            "--format=%(refname)%09%(objecttype)%09%(objectname)%09%(*objecttype)%09%(*objectname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().split('\t');
            let refname = fields.next()?;
            let object_type = fields.next()?;
            let object = fields.next()?;
            let peeled_type = fields.next().unwrap_or_default();
            let peeled = fields.next().unwrap_or_default();
            let commit = if object_type == "commit" {
                object
            } else if peeled_type == "commit" {
                peeled
            } else {
                return None;
            };
            (!refname.is_empty() && !commit.is_empty())
                .then(|| (refname.to_owned(), commit.to_owned()))
        })
        .collect())
}

fn common_dir(cwd: &Path) -> anyhow::Result<PathBuf> {
    let common_dir = git::run_git_ok(cwd, ["rev-parse", "--git-common-dir"])?;
    let common_dir = common_dir.trim();
    if common_dir.is_empty() {
        bail!("git did not return its common directory");
    }
    let common_dir = PathBuf::from(common_dir);
    Ok(if common_dir.is_absolute() {
        common_dir
    } else {
        cwd.join(common_dir)
    })
}

fn update_refs(cwd: &Path, commands: String) -> anyhow::Result<()> {
    if commands.is_empty() {
        return Ok(());
    }
    let mut child = git::command(cwd)
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to execute git")?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("git update-ref stdin is unavailable"))?
        .write_all(commands.as_bytes())
        .context("failed to send ref updates to git")?;
    let output = child.wait_with_output().context("failed to execute git")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("{}", command_error(&output))
    }
}

fn git_output<I, S>(cwd: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git::command(cwd)
        .args(args)
        .output()
        .context("failed to execute git")?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        bail!("{}", command_error(&output))
    }
}

fn git_with_index<I, S>(cwd: &Path, index: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_with_environment(cwd, index, args, false)
}

fn git_with_identity_and_index<I, S>(cwd: &Path, index: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_with_environment(cwd, index, args, true)
}

fn git_with_environment<I, S>(
    cwd: &Path,
    index: &Path,
    args: I,
    identity: bool,
) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git::command(cwd);
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_INDEX_FILE", index);
    if identity {
        command
            .env("GIT_AUTHOR_NAME", "Orbit")
            .env("GIT_AUTHOR_EMAIL", "orbit@localhost")
            .env("GIT_COMMITTER_NAME", "Orbit")
            .env("GIT_COMMITTER_EMAIL", "orbit@localhost");
    }
    let output = command.output().context("failed to execute git")?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        bail!("{}", command_error(&output))
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

/// A per-process-unique suffix for temporary index paths (no `uuid` dep).
fn next_temp_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    format!(
        "{nanos:x}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ok(cwd: &Path, args: &[&str]) {
        let status = git::command(cwd).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_text(cwd: &Path, args: &[&str]) -> String {
        let output = git::command(cwd).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn repository() -> PathBuf {
        let root = std::env::temp_dir().join(format!("orbit-checkpoints-{}", next_temp_suffix()));
        fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init", "--quiet", "--initial-branch=main"]);
        git_ok(&root, &["config", "user.name", "Orbit Test"]);
        git_ok(&root, &["config", "user.email", "orbit@example.com"]);
        fs::write(root.join("tracked.txt"), "baseline\n").unwrap();
        git_ok(&root, &["add", "tracked.txt"]);
        git_ok(&root, &["commit", "--quiet", "-m", "baseline"]);
        root
    }

    #[test]
    fn captures_tracked_and_untracked_files_without_touching_the_index() {
        let root = repository();
        let session = "session-1";
        capture_turn_start(&root, session, 1).unwrap();
        fs::write(root.join("tracked.txt"), "changed\n").unwrap();
        fs::write(root.join("new.txt"), "new\n").unwrap();
        fs::write(root.join("staged.txt"), "staged\n").unwrap();
        git_ok(&root, &["add", "staged.txt"]);
        capture_turn(&root, session, 1).unwrap();

        // The user's index still holds exactly what they staged.
        assert_eq!(
            git_text(&root, &["diff", "--cached", "--name-only"]),
            "staged.txt"
        );
        assert!(has_ref(&root, &turn_start_ref(session, 1)));
        assert!(has_ref(&root, &checkpoint_ref(session, 1)));
        assert!(has_ref(&root, &turn_diff_base_ref(session, 1)));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn each_turn_diffs_only_its_own_changes() {
        let root = repository();
        let session = "session-2";
        capture_turn_start(&root, session, 1).unwrap();
        fs::write(root.join("first.txt"), "first\n").unwrap();
        capture_turn(&root, session, 1).unwrap();

        capture_turn_start(&root, session, 2).unwrap();
        fs::write(root.join("second.txt"), "second\n").unwrap();
        capture_turn(&root, session, 2).unwrap();

        // The second turn compares against its own start, not the first turn.
        let base = turn_diff_base_ref(session, 2);
        let end = checkpoint_ref(session, 2);
        let names = git_text(&root, &["diff", "--name-only", &base, &end]);
        assert_eq!(names, "second.txt");

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn latest_turn_lists_the_sessions_ending_checkpoints() {
        let root = repository();
        let session = "session-latest";
        assert_eq!(latest_turn(&root, session), None);

        capture_turn_start(&root, session, 1).unwrap();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        capture_turn(&root, session, 1).unwrap();
        assert_eq!(latest_turn(&root, session), Some(1));

        capture_turn_start(&root, session, 2).unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        capture_turn(&root, session, 2).unwrap();
        assert_eq!(latest_turn(&root, session), Some(2));

        // A different session has none.
        assert_eq!(latest_turn(&root, "other"), None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn branch_switch_during_a_turn_is_not_attributed_to_the_turn() {
        let root = repository();
        git_ok(&root, &["switch", "--quiet", "-c", "feature"]);
        fs::write(root.join("feature.txt"), "feature\n").unwrap();
        git_ok(&root, &["add", "feature.txt"]);
        git_ok(&root, &["commit", "--quiet", "-m", "feature"]);
        git_ok(&root, &["switch", "--quiet", "main"]);

        let session = "session-3";
        capture_turn_start(&root, session, 1).unwrap();
        git_ok(&root, &["switch", "--quiet", "feature"]);
        capture_turn(&root, session, 1).unwrap();

        let base = turn_diff_base_ref(session, 1);
        let end = checkpoint_ref(session, 1);
        let names = git_text(&root, &["diff", "--name-only", &base, &end]);
        assert!(
            names.trim().is_empty(),
            "a branch switch alone is not a file edit: {names:?}"
        );

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn worktree_capture_keeps_dirty_files_on_the_target_branch_out_of_the_turn() {
        let root = repository();
        let session = "session-4";
        capture_turn_start(&root, session, 1).unwrap();
        git_ok(&root, &["switch", "--quiet", "-c", "feature"]);
        fs::write(root.join("tracked.txt"), "changed on feature\n").unwrap();
        capture_turn(&root, session, 1).unwrap();

        let base = turn_diff_base_ref(session, 1);
        let end = checkpoint_ref(session, 1);
        let names = git_text(&root, &["diff", "--name-only", &base, &end]);
        assert_eq!(names, "tracked.txt");

        fs::remove_dir_all(root).ok();
    }
}
