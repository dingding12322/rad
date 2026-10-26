//! Conventional commit message generation.
//!
//! The primary path is a **one-shot, tool-free pi call**: `pi -p --no-tools
//! --no-session …` processes the prompt and exits, so the diff never touches
//! the user's active session and no tool can run in the workspace. Extensions
//! stay enabled so the user's custom providers resolve. If that fails (pi
//! missing, no credentials, timeout), [`heuristic`] derives a plain message
//! from the staged file statuses so the button always produces something
//! editable.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::git::{self, StatusRow};

const MAX_DIFF_BYTES: usize = 96 * 1024;
const MAX_ORIGINAL_BYTES: usize = 48 * 1024;
const MAX_TOTAL_ORIGINAL_BYTES: usize = 192 * 1024;
const TIMEOUT: Duration = Duration::from_secs(180);

/// Recent commit subjects sampled for style, kept separate so the prompt can
/// prefer the author's own conventions over the repository's.
#[derive(Default)]
struct RecentCommits {
    repository: Vec<String>,
    user: Vec<String>,
}

/// One changed file as the prompt sees it: its previous content (for context)
/// and its staged diff (the source of truth).
struct Change {
    path: String,
    original: Option<String>,
    diff: String,
}

/// Generate a conventional commit message from the staged diff (and the
/// unstaged diff as context). Blocking; call on the background executor.
pub fn generate(
    cwd: &Path,
    provider: Option<&str>,
    model: Option<&str>,
    staged: &str,
    unstaged: &str,
    rows: &[StatusRow],
) -> Result<String, String> {
    let changes = collect_changes(cwd, staged, rows);
    let recent = recent_commits(cwd);
    let repo_name = cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let branch = git::current_branch(cwd);
    let system = system_prompt();
    let prompt = user_prompt(
        &repo_name,
        branch.as_deref(),
        rows,
        &changes,
        unstaged,
        &recent,
    );
    let raw = run_pi(cwd, provider, model, &system, &prompt)?;
    let message = process_response(&raw).ok_or_else(|| tr!("commit_message.no_message"))?;
    Ok(normalize(&message))
}

/// A no-model fallback: a Conventional Commit `type(scope): description` plus a
/// bulleted body built from the changed file names — never a bare file count.
pub fn heuristic(rows: &[StatusRow]) -> String {
    let total = rows.len();
    let mut added = 0usize;
    let mut deleted = 0usize;
    let mut untracked = 0usize;
    for row in rows {
        if row.untracked() {
            untracked += 1;
        } else {
            match row.staged_badge() {
                'A' => added += 1,
                'D' => deleted += 1,
                _ => {}
            }
        }
    }
    let kind = if deleted > 0 && added == 0 && untracked == 0 && total == deleted {
        "chore"
    } else if added > 0 || untracked > 0 {
        "feat"
    } else {
        "chore"
    };
    let verb = if total > 0 && deleted == total {
        "remove"
    } else if total > 0 && added + untracked == total {
        "add"
    } else {
        "update"
    };
    let description = describe_change(rows, verb);
    let subject = match common_scope(rows) {
        Some(scope) => format!("{kind}({scope}): {description}"),
        None => format!("{kind}: {description}"),
    };
    match describe_body(rows) {
        Some(body) => format!("{subject}\n\n{body}"),
        None => subject,
    }
}

/// The fallback's subject tail: the changed areas named in prose, so it reads
/// as a description (`update git panel and commit message`) rather than a count.
fn describe_change(rows: &[StatusRow], verb: &str) -> String {
    let names = distinct_names(rows);
    match names.len() {
        0 => format!("{verb} files"),
        1 => format!("{verb} {}", names[0]),
        _ => format!("{verb} {} and {}", names[0], names[1]),
    }
}

/// The fallback's body: one past-tense bullet per change group, matching the
/// generated shape (subject, blank line, then `- ` bullets ending with a
/// period).
fn describe_body(rows: &[StatusRow]) -> Option<String> {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for row in rows {
        let name = humanize(&row.path);
        match if row.untracked() {
            'A'
        } else {
            row.staged_badge()
        } {
            'A' => added.push(name),
            'D' => removed.push(name),
            _ => changed.push(name),
        }
    }
    let mut lines: Vec<String> = Vec::new();
    if !changed.is_empty() {
        lines.push(format!("- Updated {}.", join_names(&changed)));
    }
    if !added.is_empty() {
        lines.push(format!("- Added {}.", join_names(&added)));
    }
    if !removed.is_empty() {
        lines.push(format!("- Removed {}.", join_names(&removed)));
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

/// Distinct human-readable names, in first-seen order.
fn distinct_names(rows: &[StatusRow]) -> Vec<String> {
    let mut names = Vec::new();
    for row in rows {
        let name = humanize(&row.path);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// A file path reduced to a spaced, human word (`commit_message.rs` →
/// `commit message`), falling back to the raw path when there is no stem.
fn humanize(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().replace(['_', '-'], " "))
        .unwrap_or_else(|| path.to_string())
}

/// `a, b, and c` — an Oxford-comma list for the fallback prose.
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// One fallback summary line: `M path (+n -m)`, untracked files marked `U`.
fn describe_row(row: &StatusRow) -> String {
    let badge = if row.untracked() {
        'U'
    } else {
        row.staged_badge()
    };
    let (adds, dels) = if row.untracked() || row.staged_additions + row.staged_deletions == 0 {
        (row.unstaged_additions, row.unstaged_deletions)
    } else {
        (row.staged_additions, row.staged_deletions)
    };
    let stat = if adds > 0 || dels > 0 {
        format!(" (+{adds} -{dels})")
    } else {
        String::new()
    };
    format!("{badge} {}{stat}", row.path)
}

fn common_scope(rows: &[StatusRow]) -> Option<String> {
    let mut parts = rows
        .iter()
        .map(|row| row.path.split('/').collect::<Vec<_>>());
    let first = parts.next()?;
    if first.len() < 2 {
        return None;
    }
    let mut common = first.len() - 1;
    for parts in parts {
        let mut shared = 0;
        while shared < common && shared + 1 < parts.len() && parts[shared] == first[shared] {
            shared += 1;
        }
        common = shared;
        if common == 0 {
            return None;
        }
    }
    (common >= 1).then(|| first[common - 1].to_string())
}

/// A compact, ordered list of the staged files with their change kind and line
/// deltas. Gives the model the file-level map even when the raw diff is
/// truncated, and pairs with the diff so the body can name what actually
/// changed.
fn staged_summary(rows: &[StatusRow]) -> String {
    rows.iter()
        .map(|row| format!("  {}", describe_row(row)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The system rules, mirroring VS Code Copilot's git commit message prompt.
fn system_prompt() -> String {
    String::from(
        "You are an AI programming assistant, helping a software developer come up with the best git commit message for their code changes.\n\
         You excel in interpreting the purpose behind code changes to craft succinct, clear commit messages that adhere to the repository's guidelines.\n\n\
         # First, think step-by-step:\n\
         1. Analyze the CODE CHANGES thoroughly to understand what's been modified.\n\
         2. Use the ORIGINAL CODE to understand the context of the CODE CHANGES. Use the line numbers to map the CODE CHANGES to the ORIGINAL CODE.\n\
         3. Identify the purpose of the changes to answer the *why* for the commit message, also considering the optionally provided RECENT USER COMMITS.\n\
         4. Review the provided RECENT REPOSITORY COMMITS to identify established commit message conventions. Focus on the format and style, ignoring commit-specific details like refs, tags, and authors.\n\
         5. Generate a thoughtful and succinct commit message for the given CODE CHANGES. It MUST follow the established writing conventions.\n\
         6. Remove any meta information like issue references, tags, or author names from the commit message. The developer will add them.\n\
         7. Now only show your message, wrapped with a single markdown ```text codeblock! Do not provide any explanations or details.\n",
    )
}

/// The user message, mirroring VS Code Copilot's tagged prompt structure:
/// repository context, recent commits, per-file original code + diff, and the
/// closing reminder.
fn user_prompt(
    repo_name: &str,
    branch: Option<&str>,
    rows: &[StatusRow],
    changes: &[Change],
    unstaged: &str,
    recent: &RecentCommits,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("<repository-context>\n# REPOSITORY DETAILS:\n");
    prompt.push_str(&format!("Repository name: {repo_name}\n"));
    prompt.push_str(&format!(
        "Branch name: {}\n",
        branch.unwrap_or("(detached HEAD)")
    ));
    prompt.push_str("</repository-context>\n\n");

    if !recent.user.is_empty() {
        prompt.push_str(
            "<user-commits>\n# RECENT USER COMMITS (For reference only, do not copy!):\n",
        );
        for subject in &recent.user {
            prompt.push_str(&format!("- {subject}\n"));
        }
        prompt.push_str("</user-commits>\n\n");
    }
    if !recent.repository.is_empty() {
        prompt.push_str(
            "<recent-commits>\n# RECENT REPOSITORY COMMITS (For reference only, do not copy!):\n",
        );
        for subject in &recent.repository {
            prompt.push_str(&format!("- {subject}\n"));
        }
        prompt.push_str("</recent-commits>\n\n");
    }

    prompt.push_str("<changes>\n# CHANGED FILES (status and line counts):\n");
    prompt.push_str(&staged_summary(rows));
    let mut budget = MAX_TOTAL_ORIGINAL_BYTES;
    for change in changes {
        prompt.push_str(&format!("\n<file path=\"{}\">\n", change.path));
        prompt.push_str("<original-code>\n# ORIGINAL CODE:\n");
        match &change.original {
            Some(original) if budget > 0 => {
                let original = truncate(original, MAX_ORIGINAL_BYTES.min(budget));
                budget = budget.saturating_sub(original.len());
                prompt.push_str(&format!("// File: {}\n", change.path));
                prompt.push_str(&original);
                prompt.push('\n');
            }
            Some(_) => prompt.push_str("// (original code omitted: prompt budget exhausted)\n"),
            None => prompt.push_str(&format!(
                "// File: {}\n// (new file — no previous version)\n",
                change.path
            )),
        }
        prompt.push_str("</original-code>\n<code-changes>\n# CODE CHANGES:\n```diff\n");
        prompt.push_str(&truncate(&change.diff, MAX_DIFF_BYTES));
        prompt.push_str("\n```\n</code-changes>\n</file>\n");
    }
    if !unstaged.trim().is_empty() {
        prompt.push_str("\n# UNSTAGED CHANGES (context only, not part of this commit):\n```diff\n");
        prompt.push_str(&truncate(unstaged, MAX_DIFF_BYTES));
        prompt.push_str("\n```\n");
    }
    prompt.push_str("</changes>\n\n");

    prompt.push_str(
        "<reminder>\n\
         Now generate a commit message that describes the CODE CHANGES.\n\
         DO NOT COPY commits from RECENT COMMITS, but use them as reference for the commit style.\n\
         ONLY return a single markdown code block, NO OTHER PROSE!\n\
         ```text\n\
         commit message goes here\n\
         ```\n\
         </reminder>",
    );
    prompt
}

/// Sample the last 5 repository commits and the last 5 commits by the current
/// author, matching VS Code's commit-message context.
fn recent_commits(cwd: &Path) -> RecentCommits {
    let repository = git::history(cwd, 5, 0)
        .map(recent_subjects)
        .unwrap_or_default();
    let user = git::user_name(cwd)
        .and_then(|name| git::history_by_author(cwd, &name, 5).ok())
        .map(recent_subjects)
        .unwrap_or_default();
    RecentCommits { repository, user }
}

fn recent_subjects(entries: Vec<git::CommitEntry>) -> Vec<String> {
    entries
        .into_iter()
        .map(|commit| commit.subject)
        .filter(|subject| !subject.is_empty())
        .collect()
}

/// Pair every changed file with its previous content and its staged diff.
fn collect_changes(cwd: &Path, staged: &str, rows: &[StatusRow]) -> Vec<Change> {
    let mut changes: Vec<Change> = split_diffs(staged)
        .into_iter()
        .map(|(path, diff)| Change {
            original: git::file_at_head(cwd, &path),
            path,
            diff,
        })
        .collect();
    // Binary files and anything git omitted still appear, with an empty diff.
    for row in rows {
        if !changes.iter().any(|change| change.path == row.path) {
            changes.push(Change {
                path: row.path.clone(),
                original: git::file_at_head(cwd, &row.path),
                diff: String::new(),
            });
        }
    }
    changes
}

/// Split a unified diff into `(path, chunk)` pairs at `diff --git` boundaries.
fn split_diffs(patch: &str) -> Vec<(String, String)> {
    let mut diffs: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(done) = current.take() {
                diffs.push(done);
            }
            let path = line
                .trim_end()
                .rsplit(" b/")
                .next()
                .unwrap_or_default()
                .to_string();
            current = Some((path, line.to_string()));
        } else if let Some((_, diff)) = current.as_mut() {
            diff.push_str(line);
        }
    }
    if let Some(done) = current.take() {
        diffs.push(done);
    }
    diffs
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [diff truncated]", &text[..end])
}

fn run_pi(
    cwd: &Path,
    provider: Option<&str>,
    model: Option<&str>,
    system: &str,
    prompt: &str,
) -> Result<String, String> {
    let bin = orbit_rpc::pi_binary();
    let mut command = Command::new(&bin);
    command
        .arg("-p")
        // Extensions are deliberately left enabled: custom providers
        // (ollama-cloud, clinepass, …) are installed as pi extensions, so
        // `--no-extensions` makes `--provider`/`--model` fail with "Unknown
        // provider" and the call falls back to the generic heuristic. Every
        // other capability that could run code is still disabled here.
        .args([
            "--no-tools",
            "--no-session",
            "--no-skills",
            "--no-context-files",
            "--no-approve",
        ])
        .arg("--system-prompt")
        .arg(system)
        .env("PI_SKIP_VERSION_CHECK", "1")
        // `pi` is `#!/usr/bin/env node`; a bundled `.app` PATH lacks `node`.
        .env("PATH", orbit_rpc::augmented_path(Path::new(&bin).parent()))
        .env("NO_COLOR", "1")
        .env("CI", "1")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(provider) = provider.filter(|value| !value.is_empty()) {
        command.args(["--provider", provider]);
    }
    if let Some(model) = model.filter(|value| !value.is_empty()) {
        command.args(["--model", model]);
    }
    command.arg("--").arg(prompt);
    orbit_rpc::hide_console(&mut command);

    let mut child = command
        .spawn()
        .map_err(|err| tr!("errors.failed_to_run", bin = bin, error = err))?;
    let stdout = child.stdout.take().ok_or("pi stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("pi stderr unavailable")?;
    let out_handle = std::thread::spawn(move || read_to_end(stdout));
    let err_handle = std::thread::spawn(move || read_to_end(stderr));

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = out_handle.join().unwrap_or_default();
                let err = err_handle.join().unwrap_or_default();
                return if status.success() {
                    Ok(out)
                } else {
                    Err(tr!(
                        "errors.pi_exited_with",
                        status = status.to_string(),
                        stderr = first_line(&err)
                    ))
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(tr!("errors.commit_generation_timed_out"));
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(err) => return Err(tr!("errors.pi_process_error", error = err)),
        }
    }
}

fn read_to_end(mut reader: impl Read) -> String {
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    String::from_utf8_lossy(&buffer).into_owned()
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(&tr!("commit_message.no_error_output"))
        .to_string()
}

/// Extract the first fenced ```text block from the model output, mirroring
/// VS Code Copilot's `processGeneratedCommitMessage`. Unlike a strict prefix
/// match this tolerates a preamble or trailing prose around the fence, which
/// models often add despite the instructions. Falls back to the raw text when
/// no fence is present.
fn process_response(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let fenced = extract_fence(trimmed, "```text").or_else(|| extract_fence(trimmed, "```"));
    let text = fenced.unwrap_or_else(|| trimmed.trim_matches('"').trim().to_string());
    (!text.is_empty()).then_some(text)
}

/// The body of the first fence opened with `marker` at the start of a line.
fn extract_fence(text: &str, marker: &str) -> Option<String> {
    let start = text.find(marker)?;
    if start != 0 && !text[..start].ends_with('\n') {
        return None;
    }
    let rest = &text[start + marker.len()..];
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let end = rest.find("\n```")?;
    let body = rest[..end].trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// Force the model output into the app's commit shape regardless of how closely
/// it followed instructions: one subject line, a blank line, then past-tense
/// body bullets that each start with `- ` and end with a period.
fn normalize(message: &str) -> String {
    let mut lines = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let Some(subject) = lines.next() else {
        return String::new();
    };
    let subject = strip_bullet(subject).trim_end_matches('.').trim();
    let mut body: Vec<String> = Vec::new();
    for line in lines {
        let line = strip_bullet(line).trim();
        if line.is_empty() {
            continue;
        }
        let mut sentence = capitalize(line);
        if !sentence.ends_with(['.', '!', '?']) {
            sentence.push('.');
        }
        body.push(format!("- {sentence}"));
    }
    if body.is_empty() {
        subject.to_string()
    } else {
        format!("{subject}\n\n{}", body.join("\n"))
    }
}

/// Remove a leading `- `, `* `, `• `, or `1.`/`1)` list marker.
fn strip_bullet(line: &str) -> &str {
    let trimmed = line
        .trim_start_matches(['-', '*', '•', '–', '—'])
        .trim_start();
    let digits = trimmed.len()
        - trimmed
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .len();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if let Some(rest) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            return rest.trim_start();
        }
    }
    trimmed
}

fn capitalize(line: &str) -> String {
    let mut chars = line.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_single_text_fence() {
        assert_eq!(
            process_response("```text\nfeat(ui): add git panel\n```").as_deref(),
            Some("feat(ui): add git panel")
        );
        assert_eq!(
            process_response("\"fix: guard empty input\"").as_deref(),
            Some("fix: guard empty input")
        );
        // A preamble and trailing prose around the fence are tolerated.
        assert_eq!(
            process_response(
                "Here is the message:\n```text\nfix: guard input\n```\nLet me know if you want changes."
            )
            .as_deref(),
            Some("fix: guard input")
        );
        assert_eq!(process_response("   \n  "), None);
    }

    #[test]
    fn splits_a_patch_into_per_file_chunks() {
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@\n-old\n+new\n\
                     diff --git a/b.rs b/b.rs\n--- /dev/null\n+++ b/b.rs\n@@\n+new\n";
        let diffs = split_diffs(patch);
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].0, "a.rs");
        assert_eq!(diffs[1].0, "b.rs");
        assert!(diffs[1].1.contains("+new"));
    }

    #[test]
    fn heuristic_names_the_shared_scope() {
        let rows = vec![
            crate::git::StatusRow {
                path: "crates/orbit-pi/src/a.rs".into(),
                orig_path: None,
                index: 'M',
                worktree: ' ',
                staged_additions: 1,
                staged_deletions: 0,
                unstaged_additions: 0,
                unstaged_deletions: 0,
            },
            crate::git::StatusRow {
                path: "crates/orbit-pi/src/b.rs".into(),
                orig_path: None,
                index: 'A',
                worktree: ' ',
                staged_additions: 3,
                staged_deletions: 0,
                unstaged_additions: 0,
                unstaged_deletions: 0,
            },
        ];
        let message = heuristic(&rows);
        assert!(message.starts_with("feat(src):"), "{message}");
    }

    #[test]
    fn heuristic_reads_as_a_conventional_description() {
        let rows = vec![
            crate::git::StatusRow {
                path: "crates/orbit-pi/src/git_panel.rs".into(),
                orig_path: None,
                index: 'M',
                worktree: ' ',
                staged_additions: 4,
                staged_deletions: 1,
                unstaged_additions: 0,
                unstaged_deletions: 0,
            },
            crate::git::StatusRow {
                path: "crates/orbit-pi/src/commit_message.rs".into(),
                orig_path: None,
                index: 'A',
                worktree: ' ',
                staged_additions: 3,
                staged_deletions: 0,
                unstaged_additions: 0,
                unstaged_deletions: 0,
            },
        ];
        let message = heuristic(&rows);
        // Conventional subject with a description, not a file count.
        assert_eq!(
            message.lines().next().unwrap(),
            "feat(src): update git panel and commit message"
        );
        // One past-tense bullet per change group, each ending with a period.
        assert!(
            message.contains("\n\n- Updated git panel.\n- Added commit message."),
            "{message}"
        );
        assert!(!message.contains("- M "), "{message}");
        assert!(!message.contains("2 files"), "{message}");
    }

    #[test]
    fn normalizes_bullets_and_trailing_periods_into_the_commit_shape() {
        let raw = "- feat: animate the sidebar\n\n* the sidebar now slides open\n1. the toggle floats above\nGit status no longer marks unstaged edits as staged.";
        assert_eq!(
            normalize(raw),
            "feat: animate the sidebar\n\n\
             - The sidebar now slides open.\n\
             - The toggle floats above.\n\
             - Git status no longer marks unstaged edits as staged."
        );
    }

    #[test]
    fn normalize_keeps_a_subject_only_message() {
        assert_eq!(
            normalize("fix: guard empty input."),
            "fix: guard empty input"
        );
        assert_eq!(normalize("   \n "), "");
    }

    #[test]
    fn prompt_follows_the_vscode_structure() {
        let rows = vec![crate::git::StatusRow {
            path: "src/lib.rs".into(),
            orig_path: None,
            index: 'M',
            worktree: ' ',
            staged_additions: 2,
            staged_deletions: 0,
            unstaged_additions: 0,
            unstaged_deletions: 0,
        }];
        let changes = vec![Change {
            path: "src/lib.rs".into(),
            original: Some("fn old() {}\n".into()),
            diff: "diff --git a/src/lib.rs b/src/lib.rs\n".into(),
        }];
        let recent = RecentCommits {
            repository: vec!["feat: add the review pane".into()],
            user: vec!["fix: guard empty input".into()],
        };
        let user = user_prompt("orbit", Some("main"), &rows, &changes, "", &recent);
        assert!(user.contains("<repository-context>"), "{user}");
        assert!(user.contains("Repository name: orbit"), "{user}");
        assert!(user.contains("Branch name: main"), "{user}");
        assert!(user.contains("<user-commits>"), "{user}");
        assert!(user.contains("<recent-commits>"), "{user}");
        assert!(user.contains("feat: add the review pane"), "{user}");
        assert!(user.contains("  M src/lib.rs (+2 -0)"), "{user}");
        assert!(user.contains("<original-code>"), "{user}");
        assert!(user.contains("fn old() {}"), "{user}");
        assert!(user.contains("<code-changes>"), "{user}");
        assert!(user.contains("```diff"), "{user}");
        assert!(user.contains("<reminder>"), "{user}");
        assert!(
            user.contains("ONLY return a single markdown code block"),
            "{user}"
        );

        let system = system_prompt();
        assert!(system.contains("think step-by-step"), "{system}");
        assert!(
            system.contains("single markdown ```text codeblock"),
            "{system}"
        );
    }
}
