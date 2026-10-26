//! Optional worktree setup scripts.
//!
//! A repository may carry `.orbit/worktree-setup.sh` (configurable). After
//! `git worktree add` succeeds, Orbit can run that script inside the new
//! worktree so a user's own steps — linking `.env`, sharing `node_modules`,
//! warming a build cache — happen before the worktree is opened. Orbit never
//! hardcodes any of those steps: the script is entirely user-controlled.
//!
//! The script sees two environment variables:
//!
//! ```text
//! ORBIT_ROOT_PATH=/Users/user/project
//! ORBIT_WORKTREE_PATH=/Users/user/project/.wt/113
//! ```
//!
//! Execution happens on the background executor (see `app/worktrees.rs`);
//! this module is the synchronous core so it is testable without GPUI. A
//! failed setup never removes the successfully created worktree.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a setup run produced. `success` is false when the process could not
/// be spawned, when the exit code is nonzero, or when the script is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOutcome {
    /// The script path that ran (or was attempted).
    pub script: PathBuf,
    /// Whether the exit code was 0 (or the process spawn failed).
    pub success: bool,
    /// Exit code, when the process actually ran.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl SetupOutcome {
    /// Combined output for the expandable detail view: first the stream name
    /// only when both streams have content, so a normal run reads cleanly.
    pub fn output(&self) -> String {
        match (self.stdout.trim().is_empty(), self.stderr.trim().is_empty()) {
            (true, true) => String::new(),
            (false, true) => self.stdout.clone(),
            (true, false) => self.stderr.clone(),
            (false, false) => format!("{}\n\n{}", self.stdout.trim_end(), self.stderr.trim_end()),
        }
    }

    /// A one-line description for logs and toasts.
    pub fn summary(&self) -> String {
        match (self.success, self.exit_code) {
            (true, _) => tr!("worktree.setup.succeeded"),
            (false, Some(code)) => tr!("worktree.setup.failed_exit", code = code),
            (false, None) => tr!("worktree.setup.failed_spawn"),
        }
    }
}

/// Build the platform command that runs `script`.
///
/// Unix runs the script through `/bin/sh`, so it works whether or not the
/// executable bit is set (a fresh checkout on Windows, or a hand-copied
/// script, is a common case). Windows shells cannot run a POSIX script;
/// `cmd /C` gives the same observable outcome — spawn failure or exit code —
/// instead of Orbit second-guessing the file.
fn script_command(script: &Path) -> Command {
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command.arg(script);
        command
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(script);
        command
    }
    #[cfg(not(any(unix, windows)))]
    {
        Command::new(script)
    }
}

/// Run `script` for the worktree at `worktree_path`, rooted at `repo_root`.
///
/// The child runs with the worktree as its cwd and inherits the environment.
/// `GIT_TERMINAL_PROMPT=0` keeps a script's Git commands from blocking the
/// background executor on a credential prompt.
pub fn run(script: &Path, repo_root: &Path, worktree_path: &Path) -> SetupOutcome {
    let mut command = script_command(script);
    command.current_dir(worktree_path);
    command.env("ORBIT_ROOT_PATH", repo_root);
    command.env("ORBIT_WORKTREE_PATH", worktree_path);
    command.env("GIT_TERMINAL_PROMPT", "0");
    orbit_rpc::hide_console(&mut command);
    match command.output() {
        Ok(output) => SetupOutcome {
            script: script.to_path_buf(),
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(error) => SetupOutcome {
            script: script.to_path_buf(),
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: tr!("worktree.setup.spawn_error", error = error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Self-cleaning temporary directory.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "orbit-worktree-setup-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn successful_script_receives_environment_variables() {
        let temp = TempDir::new();
        let worktree = temp.path.join("worktree");
        std::fs::create_dir_all(&worktree).expect("make worktree");
        let script = temp.path.join("setup.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$ORBIT_ROOT_PATH\" > root.txt\nprintf '%s' \"$ORBIT_WORKTREE_PATH\" > wt.txt\necho done\n",
        )
        .expect("write script");

        let outcome = run(&script, &temp.path, &worktree);
        assert!(outcome.success, "stderr: {}", outcome.stderr);
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.stdout.contains("done"));
        assert_eq!(
            std::fs::read_to_string(worktree.join("root.txt")).expect("root file"),
            temp.path.to_string_lossy()
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("wt.txt")).expect("wt file"),
            worktree.to_string_lossy()
        );
        assert!(!outcome.summary().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn failed_script_reports_exit_code_and_stderr() {
        let temp = TempDir::new();
        let worktree = temp.path.join("worktree");
        std::fs::create_dir_all(&worktree).expect("make worktree");
        let script = temp.path.join("setup.sh");
        std::fs::write(&script, "#!/bin/sh\necho boom >&2\nexit 7\n").expect("write script");

        let outcome = run(&script, &temp.path, &worktree);
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(7));
        assert!(outcome.stderr.contains("boom"));
        assert!(outcome.output().contains("boom"));
        assert_ne!(outcome.summary(), tr!("worktree.setup.succeeded"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_script_fails_without_touching_the_worktree() {
        let temp = TempDir::new();
        let worktree = temp.path.join("worktree");
        std::fs::create_dir_all(&worktree).expect("make worktree");
        std::fs::write(worktree.join("keep.txt"), "keep").expect("write file");

        let outcome = run(&temp.path.join("missing.sh"), &temp.path, &worktree);
        assert!(!outcome.success);
        assert!(worktree.join("keep.txt").exists());
        assert!(!outcome.output().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn script_runs_with_the_worktree_as_cwd() {
        let temp = TempDir::new();
        let worktree = temp.path.join("worktree");
        std::fs::create_dir_all(&worktree).expect("make worktree");
        let script = temp.path.join("setup.sh");
        std::fs::write(&script, "#!/bin/sh\npwd > cwd.txt\n").expect("write script");

        let outcome = run(&script, &temp.path, &worktree);
        assert!(outcome.success, "stderr: {}", outcome.stderr);
        let cwd = std::fs::read_to_string(worktree.join("cwd.txt")).expect("cwd file");
        // macOS reports the resolved /private path; compare canonical forms.
        assert_eq!(
            std::fs::canonicalize(cwd.trim()).expect("canonical cwd"),
            std::fs::canonicalize(&worktree).expect("canonical worktree")
        );
    }

    #[test]
    fn outcome_combines_both_streams() {
        let outcome = SetupOutcome {
            script: PathBuf::from("/tmp/setup.sh"),
            success: false,
            exit_code: Some(1),
            stdout: "out\n".into(),
            stderr: "err\n".into(),
        };
        assert_eq!(outcome.output(), "out\n\nerr");
    }
}
