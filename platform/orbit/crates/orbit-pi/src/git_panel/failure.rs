//! Structured Git failure classification and recovery actions.
//!
//! Raw `git` stderr is lossy as a user-facing signal: it names the plumbing
//! (`refs/heads/main`, `pre-receive hook declined`) rather than the situation,
//! and it never says what to do next. This module turns it into an
//! [`ActionError`] — a stable [`FailureKind`], a one-line human title, the
//! cleaned raw detail (kept in full so nothing is lost to truncation), and the
//! [`RecoveryAction`]s that are actually valid for that kind.
//!
//! Classification is pure and unit-tested; the Git page decides which actions
//! to render and how to run them. Order matters: server-side rejections
//! (secret scanning, size limits, protected branches) are matched before the
//! generic "push rejected", because their stderr also contains the generic
//! wording.

/// What kind of Git failure this is. Drives the title and the offered recovery
/// actions. Kept separate from the raw text so tests and callers never depend on
/// git's exact wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The remote moved ahead: a non-fast-forward push.
    PushRejected,
    /// Local and remote histories diverged (ahead *and* behind).
    Diverged,
    /// A merge stopped on conflicting hunks.
    MergeConflict,
    /// A rebase stopped on conflicting hunks.
    RebaseConflict,
    /// A checkout/switch/merge would clobber uncommitted local edits.
    LocalOverwritten,
    /// A rebase/merge could not start because the worktree is dirty.
    DirtyWorktree,
    /// Credentials were rejected by the remote.
    Auth,
    /// The push was blocked by server-side secret scanning.
    SecretScan,
    /// A file exceeds the remote's size limit.
    LargeFile,
    /// Git LFS rejected the push or a filter failed.
    Lfs,
    /// The branch is protected; changes must go through a pull request.
    Protected,
    /// A server-side (pre-receive) hook declined the push.
    PreReceiveHook,
    /// `origin` is gone or points somewhere unusable.
    RemoteMissing,
    /// The remote could not be reached.
    Network,
    /// There is no upstream branch to push to.
    NoUpstream,
    /// The directory is not a Git work tree.
    NotARepo,
    /// Anything we do not recognize.
    Generic,
}

impl FailureKind {
    /// Every recovery action worth offering for this failure, in the order the
    /// Git page should show them. Empty when there is nothing safe to do from
    /// inside Orbit.
    pub fn recovery_actions(self) -> Vec<RecoveryAction> {
        match self {
            // The remote has commits we lack. Pull when we are strictly behind;
            // merge (or rebase) when we diverged; force-with-lease only as an
            // explicit, destructive escape hatch.
            Self::PushRejected => vec![
                RecoveryAction::Pull,
                RecoveryAction::Merge,
                RecoveryAction::ForceWithLease,
            ],
            Self::Diverged => vec![RecoveryAction::Merge, RecoveryAction::Rebase],
            Self::MergeConflict => vec![
                RecoveryAction::OpenConflicts,
                RecoveryAction::AbortOperation,
                RecoveryAction::ContinueOperation,
            ],
            Self::RebaseConflict => vec![
                RecoveryAction::OpenConflicts,
                RecoveryAction::AbortOperation,
                RecoveryAction::ContinueOperation,
                RecoveryAction::SkipOperation,
            ],
            Self::LocalOverwritten => vec![RecoveryAction::AbortOperation],
            Self::Auth => vec![RecoveryAction::Reauthenticate, RecoveryAction::Retry],
            Self::Network => vec![RecoveryAction::Retry],
            Self::NoUpstream => vec![RecoveryAction::PublishBranch],
            // Server-side rejections need a code/config change first; there is
            // no safe in-app command to run.
            Self::SecretScan
            | Self::LargeFile
            | Self::Lfs
            | Self::Protected
            | Self::PreReceiveHook => Vec::new(),
            // A dirty worktree is a Phase 2 stash action; until then the error
            // text tells the user to commit or stash.
            Self::DirtyWorktree => Vec::new(),
            Self::RemoteMissing | Self::NotARepo | Self::Generic => Vec::new(),
        }
    }
}

/// A next step the Git page can offer after a failure. The page owns how each
/// action runs; this enum only names the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Fast-forward the current branch from upstream (`git pull --ff-only`).
    Pull,
    /// Merge upstream into the current branch.
    Merge,
    /// Rebase the current branch onto upstream.
    Rebase,
    /// Overwrite the remote with the local branch, guarded by
    /// `--force-with-lease`.
    ForceWithLease,
    /// Abort an in-progress merge/rebase/cherry-pick.
    AbortOperation,
    /// Continue an in-progress merge/rebase/cherry-pick.
    ContinueOperation,
    /// Skip the current patch of a rebase or cherry-pick.
    SkipOperation,
    /// Open the conflicting files.
    OpenConflicts,
    /// Re-run the remote's authentication flow.
    Reauthenticate,
    /// Retry the failing command unchanged.
    Retry,
    /// Push the current branch and set its upstream.
    PublishBranch,
}

/// A failed Git operation, ready to render.
///
/// `title` is the short human summary (usually including the fix); `detail`
/// keeps the command's own output, including the multi-line `hint:` blocks a
/// rejected push or a merge conflict prints. Unlike the transient status line,
/// an `ActionError` is never truncated and never expires on a timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionError {
    pub kind: FailureKind,
    pub title: String,
    pub detail: String,
}

impl ActionError {
    /// Classify raw `git` stderr (or any error string) into an `ActionError`.
    pub fn from_raw(raw: &str) -> Self {
        let detail = clean_git_detail(raw);
        let haystack = raw.to_lowercase();
        let kind = classify(&haystack);
        let title = match kind {
            FailureKind::PushRejected => tr!("git_panel.fail_push_rejected"),
            FailureKind::Diverged => tr!("git_panel.fail_diverged"),
            FailureKind::MergeConflict => tr!("git_panel.fail_merge_conflicts"),
            FailureKind::RebaseConflict => tr!("git_panel.fail_rebase_conflicts"),
            FailureKind::LocalOverwritten => tr!("git_panel.fail_local_overwritten"),
            FailureKind::DirtyWorktree => tr!("git_panel.fail_dirty_worktree"),
            FailureKind::Auth => tr!("git_panel.fail_auth"),
            FailureKind::SecretScan => tr!("git_panel.fail_secret_scan"),
            FailureKind::LargeFile => tr!("git_panel.fail_large_file"),
            FailureKind::Lfs => tr!("git_panel.fail_lfs"),
            FailureKind::Protected => tr!("git_panel.fail_protected"),
            FailureKind::PreReceiveHook => tr!("git_panel.fail_pre_receive_hook"),
            FailureKind::RemoteMissing => tr!("git_panel.fail_remote_not_found"),
            FailureKind::Network => tr!("git_panel.fail_network"),
            FailureKind::NoUpstream => tr!("git_panel.fail_no_upstream"),
            FailureKind::NotARepo => tr!("git_panel.fail_not_a_repo"),
            FailureKind::Generic => tr!("git_panel.fail_generic"),
        };
        Self {
            kind,
            title,
            detail,
        }
    }

    /// The recovery actions valid for this failure.
    pub fn recovery_actions(&self) -> Vec<RecoveryAction> {
        self.kind.recovery_actions()
    }
}

/// Map a lowercased error haystack to a [`FailureKind`]. Split out so the
/// detection order is explicit and unit-testable.
fn classify(haystack: &str) -> FailureKind {
    let has = |needle: &str| haystack.contains(needle);
    if has("push protection")
        || has("gh013")
        || has("secret scanning")
        || has("push declined due to")
    {
        FailureKind::SecretScan
    } else if has("exceeds github's file size limit")
        || has("gh001")
        || has("file is") && has("mb; this exceeds")
    {
        FailureKind::LargeFile
    } else if has("git-lfs") || has("lfs filter") || has("this repository is over its data quota") {
        FailureKind::Lfs
    } else if has("protected branch") || has("gh006") {
        FailureKind::Protected
    } else if has("pre-receive hook declined") {
        FailureKind::PreReceiveHook
    } else if has("[rejected]")
        || has("non-fast-forward")
        || has("failed to push some refs")
        || has("fetch first")
    {
        FailureKind::PushRejected
    } else if has("not possible to fast-forward")
        || has("divergent branches")
        || has("need to specify how to reconcile")
    {
        FailureKind::Diverged
    } else if has("could not apply") || (has("rebase") && has("conflict")) {
        FailureKind::RebaseConflict
    } else if has("automatic merge failed") || has("merge conflict") || has("conflict") {
        FailureKind::MergeConflict
    } else if has("would be overwritten") || has("your local changes") {
        FailureKind::LocalOverwritten
    } else if has("cannot rebase")
        || has("cannot pull with rebase")
        || has("unstaged changes")
        || has("you have unstaged changes")
    {
        FailureKind::DirtyWorktree
    } else if has("authentication failed")
        || has("could not read username")
        || has("could not read password")
        || has("permission denied")
        || has("403 forbidden")
        || has("401 unauthorized")
    {
        FailureKind::Auth
    } else if has("repository not found")
        || has("does not appear to be a git repository")
        || has("couldn't find remote ref")
        || has("no such remote")
    {
        FailureKind::RemoteMissing
    } else if has("could not resolve host")
        || has("unable to access")
        || has("network is unreachable")
        || has("timed out")
    {
        FailureKind::Network
    } else if has("no upstream branch") || has("has no upstream branch") || has("no branch to push")
    {
        FailureKind::NoUpstream
    } else if has("not a git repository") {
        FailureKind::NotARepo
    } else {
        FailureKind::Generic
    }
}

/// Tidy raw Git output for display: trim trailing whitespace, drop repeated
/// blank lines, and never hand the banner an empty string.
fn clean_git_detail(raw: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut prev_blank = false;
    for line in raw.lines() {
        let line = line.trim_end();
        let blank = line.is_empty();
        if blank && prev_blank {
            continue;
        }
        lines.push(line);
        prev_blank = blank;
    }
    let joined = lines.join("\n").trim().to_string();
    if joined.is_empty() {
        tr!("git_panel.git_no_error_message")
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(raw: &str) -> FailureKind {
        ActionError::from_raw(raw).kind
    }

    #[test]
    fn rejected_push_is_explained() {
        let raw = "To https://github.com/acme/repo.git\n \
                   ! [rejected]        main -> main (fetch first)\n\
                   error: failed to push some refs to 'https://github.com/acme/repo.git'\n\
                   hint: Updates were rejected because the remote contains work\n\
                   hint: that you do not have locally.";
        let error = ActionError::from_raw(raw);
        assert_eq!(error.kind, FailureKind::PushRejected);
        assert!(error.title.contains("Push rejected"), "{}", error.title);
        // The full multi-line stderr survives for the banner to wrap.
        assert!(error.detail.contains("failed to push some refs"));
        assert!(error.detail.contains("Updates were rejected"));
        assert!(error
            .recovery_actions()
            .contains(&RecoveryAction::ForceWithLease));
    }

    #[test]
    fn diverged_and_conflict_failures_are_recognized() {
        let diverged = ActionError::from_raw("fatal: Not possible to fast-forward, aborting.");
        assert_eq!(diverged.kind, FailureKind::Diverged);
        assert!(diverged.title.contains("diverged"), "{}", diverged.title);

        let raw = "CONFLICT (content): Merge conflict in src/lib.rs\n\
                   Automatic merge failed; fix conflicts and then commit the result.";
        let conflict = ActionError::from_raw(raw);
        assert_eq!(conflict.kind, FailureKind::MergeConflict);
        assert!(
            conflict.title.contains("Merge conflicts"),
            "{}",
            conflict.title
        );
        assert!(conflict.detail.contains("Automatic merge failed"));
        assert_eq!(
            conflict.recovery_actions(),
            vec![
                RecoveryAction::OpenConflicts,
                RecoveryAction::AbortOperation,
                RecoveryAction::ContinueOperation
            ]
        );
    }

    #[test]
    fn rebase_conflicts_offer_skip() {
        let raw = "error: could not apply 1a2b3c4... change\n\
                   CONFLICT (content): Merge conflict in src/lib.rs";
        let error = ActionError::from_raw(raw);
        assert_eq!(error.kind, FailureKind::RebaseConflict);
        assert!(error.title.contains("Rebase"), "{}", error.title);
        assert_eq!(
            error.recovery_actions(),
            vec![
                RecoveryAction::OpenConflicts,
                RecoveryAction::AbortOperation,
                RecoveryAction::ContinueOperation,
                RecoveryAction::SkipOperation
            ]
        );
    }

    #[test]
    fn auth_and_network_failures_are_recognized() {
        let auth = ActionError::from_raw(
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/acme/repo.git/'",
        );
        assert_eq!(auth.kind, FailureKind::Auth);
        assert!(
            auth.title.contains("Authentication failed"),
            "{}",
            auth.title
        );
        assert_eq!(
            auth.recovery_actions(),
            vec![RecoveryAction::Reauthenticate, RecoveryAction::Retry]
        );

        let network = ActionError::from_raw(
            "fatal: unable to access 'https://github.com/acme/repo.git/': Could not resolve host: github.com",
        );
        assert_eq!(network.kind, FailureKind::Network);
        assert!(network.title.contains("Network error"), "{}", network.title);
        assert_eq!(network.recovery_actions(), vec![RecoveryAction::Retry]);
    }

    #[test]
    fn server_side_rejections_are_distinguished() {
        // Secret scanning's stderr also says "failed to push some refs", so it
        // must win over the generic rejected-push classification.
        let secret = "remote: error: GH013: Repository rule violations found\n\
                      remote: — Push cannot contain secrets\n\
                      error: failed to push some refs";
        assert_eq!(kind(secret), FailureKind::SecretScan);

        let large = "remote: error: File big.bin is 150.00 MB; this exceeds GitHub's file size limit of 100.00 MB\n\
                     remote: error: GH001: Large files detected.";
        assert_eq!(kind(large), FailureKind::LargeFile);

        let protected =
            "remote: error: GH006: Protected branch update failed for refs/heads/main.\n\
                         ! [remote rejected] main -> main (protected branch hook declined)";
        assert_eq!(kind(protected), FailureKind::Protected);

        let hook = "remote: error: pre-receive hook declined\n\
                    ! [remote rejected] main -> main (pre-receive hook declined)";
        assert_eq!(kind(hook), FailureKind::PreReceiveHook);

        let lfs = "fatal: git-lfs filter-process failed";
        assert_eq!(kind(lfs), FailureKind::Lfs);

        // None of these offer an in-app command; the fix is code/config.
        for raw in [secret, large, protected, hook, lfs] {
            assert!(
                ActionError::from_raw(raw).recovery_actions().is_empty(),
                "{raw}"
            );
        }
    }

    #[test]
    fn dirty_worktree_suggests_committing_first() {
        let error =
            ActionError::from_raw("error: cannot pull with rebase: You have unstaged changes.");
        assert_eq!(error.kind, FailureKind::DirtyWorktree);
        assert!(error.title.contains("Uncommitted"), "{}", error.title);
    }

    #[test]
    fn no_upstream_suggests_publishing() {
        let error = ActionError::from_raw("fatal: The current branch has no upstream branch");
        assert_eq!(error.kind, FailureKind::NoUpstream);
        assert_eq!(
            error.recovery_actions(),
            vec![RecoveryAction::PublishBranch]
        );
    }

    #[test]
    fn unknown_failures_are_generic_and_offer_no_actions() {
        let error = ActionError::from_raw("fatal: something entirely unforeseen");
        assert_eq!(error.kind, FailureKind::Generic);
        assert!(error.recovery_actions().is_empty());
    }

    #[test]
    fn clean_git_detail_collapses_blank_runs_and_defaults() {
        assert_eq!(clean_git_detail(""), "git exited without an error message");
        assert_eq!(
            clean_git_detail("  \n \n"),
            "git exited without an error message"
        );
        assert_eq!(clean_git_detail("one\n\n\n\ntwo\n"), "one\n\ntwo",);
    }
}
