//! GitHub issue / pull-request draft generation.
//!
//! Like [`crate::commit_message`], the primary path is a **one-shot, tool-free
//! pi call** (`pi -p --no-tools --no-session …`). It processes the prompt and
//! exits, so the working tree never touches the user's active session and no
//! tool can run in the workspace. Extensions stay enabled so a user's custom
//! providers resolve. When pi is missing, has no credentials, or times out,
//! [`heuristic`] derives an editable draft from the branch and its commits.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::gh_templates::RepoTemplate;
use crate::git;

const MAX_CONTEXT_BYTES: usize = 24 * 1024;
const MAX_COMMITS: usize = 30;
const TIMEOUT: Duration = Duration::from_secs(180);

/// Which draft the caller is asking for. Drives the prompt's framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftKind {
    Issue,
    PullRequest,
}

impl DraftKind {
    fn noun(self) -> &'static str {
        match self {
            Self::Issue => "GitHub issue",
            Self::PullRequest => "GitHub pull request",
        }
    }
}

/// A generated draft: one title line plus a Markdown body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub title: String,
    pub body: String,
}

impl Draft {
    fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.body.trim().is_empty()
    }
}

/// Everything the prompt knows about the repository, sampled once so the
/// blocking git reads happen together.
#[derive(Default)]
struct RepoContext {
    repo: String,
    branch: Option<String>,
    base: Option<String>,
    commits: Vec<String>,
    changed: Vec<String>,
}

/// Generate a draft for `kind`. Blocking; call on the background executor.
/// `hint` is the user's free-form notes (may be empty); `base` is the PR's
/// target branch when known; `template` is the repository template to fill,
/// when one is selected.
pub fn generate(
    cwd: &Path,
    provider: Option<&str>,
    model: Option<&str>,
    kind: DraftKind,
    hint: &str,
    base: Option<&str>,
    template: Option<&RepoTemplate>,
) -> Result<Draft, String> {
    let context = collect(cwd, base);
    let system = system_prompt(kind, template.is_some());
    let prompt = user_prompt(kind, hint, &context, template);
    let raw = run_pi(cwd, provider, model, &system, &prompt)?;
    process_response(&raw).ok_or_else(|| tr!("errors.no_draft"))
}

/// Which kind of report the in-app Settings → Report a bug form is filing.
/// Drives the GitHub label and the draft prompt's framing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportKind {
    #[default]
    Bug,
    Feature,
    Other,
}

impl ReportKind {
    /// The repository label to apply when one exists for this kind.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Bug => Some("bug"),
            Self::Feature => Some("enhancement"),
            Self::Other => None,
        }
    }

    /// How the prompt names the document.
    fn noun(self) -> &'static str {
        match self {
            Self::Bug => "GitHub bug report",
            Self::Feature => "GitHub feature request",
            Self::Other => "GitHub issue",
        }
    }

    /// The body rule, tailored per kind.
    fn body_rule(self) -> &'static str {
        match self {
            Self::Bug => {
                "The BODY is a concise description of the problem in GitHub-flavored Markdown: what the user was doing, what happened, and what was expected."
            }
            Self::Feature => {
                "The BODY is a concise description in GitHub-flavored Markdown: the problem or friction the request addresses, then the proposed solution."
            }
            Self::Other => {
                "The BODY is a concise description in GitHub-flavored Markdown of the issue or request."
            }
        }
    }
}

/// Generate a report draft (title + description) from the user's own context
/// for the in-app Settings → Report a bug form.
///
/// Unlike [`generate`], **no repository context is sampled**: an in-app report
/// describes Orbit itself, not the workspace that happens to be open, so the
/// user's notes are the only input. The description is drawn as plain prose
/// without headings, so the form can place it under its own section heading.
/// `kind` shapes the prompt (bug vs. feature vs. general). Blocking; call on
/// the background executor.
pub fn generate_report(
    cwd: &Path,
    provider: Option<&str>,
    model: Option<&str>,
    kind: ReportKind,
    context: &str,
) -> Result<Draft, String> {
    let system = report_system_prompt(kind);
    let prompt = report_user_prompt(kind, context);
    let raw = run_pi(cwd, provider, model, &system, &prompt)?;
    process_response(&raw).ok_or_else(|| tr!("errors.no_draft"))
}

/// The report system prompt. The body is deliberately heading-free: the form
/// owns the section structure.
fn report_system_prompt(kind: ReportKind) -> String {
    format!(
        "You are an AI assistant helping a user write a {noun}.\n\
         You turn a rough description into concise, specific prose a maintainer can act on.\n\n\
         # Rules:\n\
         1. The TITLE is a single line: specific, no trailing period, no `bug:`/`feat:`/`#` prefix, under ~72 characters.\n\
         2. {body_rule}\n\
         3. Do NOT add any Markdown headings (`#`) — the form supplies the section headings.\n\
         4. If the notes already describe how to reproduce the problem or how to verify the request, render those as a numbered list at the end of the body.\n\
         5. Do not invent facts the notes do not support. Do not mention that the text was generated, and add no meta-commentary.\n\
         6. Output EXACTLY one fenced ```text block: the title on the first line, a blank line, then the Markdown body. No other prose.\n",
        noun = kind.noun(),
        body_rule = kind.body_rule()
    )
}

/// The report user message: just the notes, plus the shape reminder.
fn report_user_prompt(kind: ReportKind, context: &str) -> String {
    let context = context.trim();
    let notes = if context.is_empty() {
        "(no notes provided)".to_string()
    } else {
        truncate(context, MAX_CONTEXT_BYTES)
    };
    format!(
        "<user-notes>\n{notes}\n</user-notes>\n\n\
         <reminder>\n\
         Write the {noun} from the notes above: one title line, a blank line, then the Markdown body with no headings.\n\
         ONLY return a single markdown code block, NO OTHER PROSE!\n\
         ```text\n\
         title goes here\n\n\
         body goes here\n\
         ```\n\
         </reminder>",
        noun = kind.noun()
    )
}

/// A no-model fallback: a title from the hint or branch, and a Markdown body.
/// With a repository template, the notes are dropped into its first section so
/// the result still follows the template's shape.
pub fn heuristic(
    cwd: &Path,
    kind: DraftKind,
    hint: &str,
    base: Option<&str>,
    template: Option<&RepoTemplate>,
) -> Draft {
    let context = collect(cwd, base);
    let hint = hint.trim();
    let title = heuristic_title(kind, hint, &context);
    let body = match template {
        Some(template) => fill_first_section(&template.body, &template_seed(hint, &context)),
        None => heuristic_body(kind, hint, &context),
    };
    Draft { title, body }
}

/// Sample repository identity, commits, and changed files in one pass.
fn collect(cwd: &Path, base: Option<&str>) -> RepoContext {
    let repo = cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let branch = git::current_branch(cwd);
    let range = base
        .filter(|base| !base.is_empty())
        .map(|base| format!("{base}..HEAD"));
    let commits = match &range {
        Some(range) => git::run_git(
            cwd,
            &["log", "--no-decorate", "--format=%s", "-n", "30", range],
        ),
        None => git::run_git(cwd, &["log", "--no-decorate", "--format=%s", "-n", "20"]),
    }
    .map(|out| {
        out.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(MAX_COMMITS)
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default();

    let mut changed = Vec::new();
    let stat_args: Vec<Vec<&str>> = match &range {
        Some(range) => vec![vec!["diff", "--stat", range]],
        None => vec![
            vec!["diff", "--stat", "HEAD"],
            vec!["diff", "--stat"],
            vec!["diff", "--stat", "--cached"],
        ],
    };
    for args in stat_args {
        if let Ok(out) = git::run_git(cwd, &args) {
            for line in out.lines().map(str::trim).filter(|line| !line.is_empty()) {
                if !changed.iter().any(|seen| seen == line) {
                    changed.push(line.to_string());
                }
            }
        }
    }
    changed.truncate(60);

    RepoContext {
        repo,
        branch,
        base: base.map(str::to_string),
        commits,
        changed,
    }
}

/// The system rules. Output shape mirrors [`crate::commit_message`]: a single
/// fenced block so the parser is identical in spirit. When a repository
/// template is supplied, a rule is added to fill it faithfully.
fn system_prompt(kind: DraftKind, has_template: bool) -> String {
    let mut prompt = format!(
        "You are an AI assistant helping a software developer write a {noun}.\n\
         You excel at turning sparse notes and repository context into concise, concrete prose that a maintainer can act on.\n\n\
         # Rules:\n\
         1. The TITLE is a single line: specific, no trailing period, no `Issue:`/`PR:`/`#` prefix, under ~72 characters.\n\
         2. The BODY is GitHub-flavored Markdown. Lead with the *why*, then the *what*.\n\
         3. For a pull request, include a `## Summary` section and a `## Testing` section when the context reveals how it was verified; otherwise omit Testing.\n\
         4. For an issue, include `## What happened` and `## Expected` sections when they fit the notes.\n\
         5. Use bullets and short headings. Do not pad. Do not invent facts that the provided context does not support.\n\
         6. Never mention that the text was generated, and never add meta-commentary.\n\
         7. Output EXACTLY one fenced ```text block: the title on the first line, a blank line, then the Markdown body. No other prose.\n",
        noun = kind.noun()
    );
    if has_template {
        prompt.push_str(
            "8. A REPOSITORY TEMPLATE is provided. Preserve its structure exactly: keep every heading and every `- [ ]` checkbox line as written, and write the content under the matching heading. Leave a checkbox unchecked unless the provided context proves it. Do not add, remove, or rename sections, and drop the template's guidance comments (`<!-- … -->`).\n",
        );
    }
    prompt
}

/// The user message: repository context, the user's notes, the template (when
/// one is selected), and a closing reminder.
fn user_prompt(
    kind: DraftKind,
    hint: &str,
    context: &RepoContext,
    template: Option<&RepoTemplate>,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("<repository-context>\n");
    prompt.push_str(&format!("Repository name: {}\n", context.repo));
    prompt.push_str(&format!(
        "Branch name: {}\n",
        context.branch.as_deref().unwrap_or("(detached HEAD)")
    ));
    if let Some(base) = &context.base {
        prompt.push_str(&format!("Target base branch: {base}\n"));
    }
    prompt.push_str("</repository-context>\n\n");

    let hint = hint.trim();
    if !hint.is_empty() {
        prompt.push_str("<user-notes>\n");
        prompt.push_str(&truncate(hint, MAX_CONTEXT_BYTES));
        prompt.push_str("\n</user-notes>\n\n");
    } else {
        prompt.push_str("<user-notes>\n(none — infer the subject from the repository context)\n</user-notes>\n\n");
    }

    if !context.commits.is_empty() {
        prompt.push_str("<commits>\n# COMMITS ON THIS BRANCH\n");
        for subject in &context.commits {
            prompt.push_str(&format!("- {subject}\n"));
        }
        prompt.push_str("</commits>\n\n");
    }

    if !context.changed.is_empty() {
        prompt.push_str("<changed-files>\n# CHANGED FILES\n");
        for line in &context.changed {
            prompt.push_str(line);
            prompt.push('\n');
        }
        prompt.push_str("</changed-files>\n\n");
    }

    if let Some(template) = template {
        prompt.push_str(&format!(
            "<template name=\"{}\">\n{}
</template>\n\n",
            template.label(),
            truncate(template.body.trim(), MAX_CONTEXT_BYTES)
        ));
    }

    let reminder_tail = if template.is_some() {
        "Fill in the REPOSITORY TEMPLATE above: one title line, a blank line, then the completed Markdown body with the template's headings and checkboxes preserved.\n"
    } else {
        "Write the draft from the context above: one title line, a blank line, then the Markdown body.\n"
    };
    prompt.push_str(&format!(
        "<reminder>\n\
         {reminder_tail}\
         The result is a {noun}.\n\
         ONLY return a single markdown code block, NO OTHER PROSE!\n\
         ```text\n\
         title goes here\n\n\
         body goes here\n\
         ```\n\
         </reminder>",
        noun = kind.noun()
    ));
    prompt
}

/// The fallback title: the user's first note line when present, else the
/// branch name read as words, else the latest commit subject.
fn heuristic_title(kind: DraftKind, hint: &str, context: &RepoContext) -> String {
    if let Some(line) = hint.lines().map(str::trim).find(|line| !line.is_empty()) {
        return clean_title(line);
    }
    if let Some(subject) = context.commits.first() {
        return clean_title(subject);
    }
    match &context.branch {
        Some(branch) => clean_title(&humanize(branch)),
        None => match kind {
            DraftKind::Issue => tr!("git_panel.draft_issue_fallback_title"),
            DraftKind::PullRequest => tr!("git_panel.draft_pull_fallback_title"),
        },
    }
}

/// The fallback body: a short summary, then the commit subjects and the
/// changed files, so the field is never blank.
fn heuristic_body(kind: DraftKind, hint: &str, context: &RepoContext) -> String {
    let mut body = String::new();
    let hint_rest = hint
        .lines()
        .skip(1)
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if !hint_rest.is_empty() {
        body.push_str(&hint_rest);
        body.push_str("\n\n");
    }
    let summary_heading = match kind {
        DraftKind::Issue => tr!("git_panel.draft_what_happened"),
        DraftKind::PullRequest => tr!("git_panel.draft_summary"),
    };
    body.push_str(&format!("## {summary_heading}\n\n"));
    if !context.commits.is_empty() {
        for subject in context.commits.iter().take(10) {
            body.push_str(&format!("- {subject}\n"));
        }
    } else if !hint.trim().is_empty() {
        body.push_str(&format!("- {}\n", hint.trim()));
    } else {
        body.push_str(&format!("- {}\n", tr!("git_panel.draft_placeholder")));
    }
    if !context.changed.is_empty() {
        let files_heading = tr!("git_panel.draft_changed_files");
        body.push_str(&format!("\n## {files_heading}\n\n"));
        for line in context.changed.iter().take(20) {
            body.push_str(&format!("- `{line}`\n"));
        }
    }
    body.trim_end().to_string()
}

/// A branch name read as words (`feat/git-issues-pr-ui` → `git issues pr ui`).
fn humanize(branch: &str) -> String {
    branch
        .rsplit('/')
        .next()
        .unwrap_or(branch)
        .replace(['-', '_'], " ")
        .trim()
        .to_string()
}

/// The text the heuristic drops into a template's first section: the user's
/// notes when present, else the branch's commit subjects.
fn template_seed(hint: &str, context: &RepoContext) -> String {
    if !hint.is_empty() {
        return hint.to_string();
    }
    context
        .commits
        .iter()
        .take(10)
        .map(|subject| format!("- {subject}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Insert `text` under a template's first Markdown heading, leaving the rest of
/// the template intact. With no heading, the text is prepended.
fn fill_first_section(template: &str, text: &str) -> String {
    if text.trim().is_empty() || template.trim().is_empty() {
        return template.trim().to_string();
    }
    let lines: Vec<&str> = template.lines().collect();
    let Some(heading) = lines
        .iter()
        .position(|line| line.trim_start().starts_with('#'))
    else {
        return format!("{}\n\n{}", text.trim(), template.trim());
    };
    let mut insert_at = heading + 1;
    while insert_at < lines.len() && lines[insert_at].trim().is_empty() {
        insert_at += 1;
    }
    let mut out: Vec<String> = lines[..insert_at]
        .iter()
        .map(|line| line.to_string())
        .collect();
    if out
        .last()
        .map(|line| !line.trim().is_empty())
        .unwrap_or(true)
    {
        out.push(String::new());
    }
    for line in text.trim().lines() {
        out.push(line.to_string());
    }
    if lines
        .get(insert_at)
        .map(|line| !line.trim().is_empty())
        .unwrap_or(false)
    {
        out.push(String::new());
    }
    for line in &lines[insert_at..] {
        out.push(line.to_string());
    }
    out.join("\n").trim_end().to_string()
}

/// Strip a list marker, a leading type prefix, and trailing punctuation so a
/// title reads as a title regardless of how it arrived.
fn clean_title(raw: &str) -> String {
    let line = raw
        .trim()
        .trim_start_matches(['-', '*', '•', '#'])
        .trim()
        .trim_matches(['"', '`', '\'']);
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .map(str::trim)
        .unwrap_or(line);
    let mut title = line.trim_end_matches(['.', ':', ';']).trim().to_string();
    if title.len() > 120 {
        let mut end = 120;
        while end > 0 && !title.is_char_boundary(end) {
            end -= 1;
        }
        title.truncate(end);
    }
    title
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [truncated]", &text[..end])
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
        // Extensions stay enabled so custom providers resolve (see
        // `commit_message::run_pi`); every capability that could run code is
        // still disabled.
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
                    return Err(tr!("errors.draft_generation_timed_out"));
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

/// Extract the title and body from the model output. Tolerates a preamble or
/// trailing prose around the fence, and a missing fence (title-first text).
fn process_response(raw: &str) -> Option<Draft> {
    let trimmed = raw.trim();
    let fenced = extract_fence(trimmed, "```text")
        .or_else(|| extract_fence(trimmed, "```markdown"))
        .or_else(|| extract_fence(trimmed, "```md"))
        .or_else(|| extract_fence(trimmed, "```"));
    let text = match fenced {
        Some(body) => body,
        None => {
            let raw = trimmed.trim_matches('"').trim();
            // A lone, empty fence is not a draft; reject it rather than
            // reading the marker itself as a title.
            if raw.starts_with("```") {
                return None;
            }
            raw.to_string()
        }
    };
    let mut lines = text.lines();
    let title = lines
        .by_ref()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(clean_title)
        .unwrap_or_default();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let draft = Draft { title, body };
    (!draft.is_empty()).then_some(draft)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_and_body_from_a_text_fence() {
        let draft = process_response(
            "```text\nfeat: add issue generator\n\n## Summary\n\n- drafts a title\n```",
        )
        .unwrap();
        assert_eq!(draft.title, "feat: add issue generator");
        assert!(draft.body.contains("## Summary"));
        assert!(draft.body.contains("- drafts a title"));
    }

    #[test]
    fn tolerates_preamble_and_trailing_prose() {
        let raw = "Here you go:\n```text\nFix the flaky login test\n\n## What happened\n\nIt fails 1 in 10 runs.\n```\nLet me know!";
        let draft = process_response(raw).unwrap();
        assert_eq!(draft.title, "Fix the flaky login test");
        assert!(draft.body.contains("## What happened"));
    }

    #[test]
    fn accepts_markdown_fence_and_title_first_prose() {
        let fenced = process_response(
            "```markdown\nAdd retry to the uploader\n\n## Summary\n\nRetries twice.\n```",
        )
        .unwrap();
        assert_eq!(fenced.title, "Add retry to the uploader");

        let prose =
            process_response("Add retry to the uploader\n\n## Summary\n\nRetries twice.").unwrap();
        assert_eq!(prose.title, "Add retry to the uploader");
        assert!(prose.body.contains("## Summary"));
    }

    #[test]
    fn clean_title_strips_prefixes_and_punctuation() {
        assert_eq!(clean_title("- Title: fix the thing."), "fix the thing");
        assert_eq!(clean_title("`Add the widget`"), "Add the widget");
        assert_eq!(clean_title("## feat: do stuff:"), "feat: do stuff");
    }

    #[test]
    fn process_response_rejects_blank_output() {
        assert!(process_response("   \n  ").is_none());
        assert!(process_response("```text\n```").is_none());
    }

    #[test]
    fn heuristic_uses_the_hint_first_line_as_the_title() {
        let context = RepoContext {
            repo: "orbit".into(),
            branch: Some("feat/git-issues-pr-ui".into()),
            base: Some("main".into()),
            commits: vec!["feat(git): tidy the issue list".into()],
            changed: vec!["git_panel/mod.rs | 20 +++++".into()],
        };
        assert_eq!(
            heuristic_title(
                DraftKind::PullRequest,
                "Improve the pages\n\nMore.",
                &context
            ),
            "Improve the pages"
        );
        let body = heuristic_body(
            DraftKind::PullRequest,
            "Improve the pages\n\nMore.",
            &context,
        );
        assert!(body.contains("More."));
        assert!(body.contains("## Summary"));
        assert!(body.contains("- feat(git): tidy the issue list"));
    }

    #[test]
    fn heuristic_falls_back_to_the_branch_or_latest_commit() {
        let context = RepoContext {
            repo: "orbit".into(),
            branch: Some("feat/git-issues-pr-ui".into()),
            base: None,
            commits: vec!["feat(git): tidy the issue list".into()],
            changed: Vec::new(),
        };
        assert_eq!(
            heuristic_title(DraftKind::Issue, "", &context),
            "feat(git): tidy the issue list"
        );
        let no_commits = RepoContext {
            branch: Some("fix/flaky-login".into()),
            ..RepoContext::default()
        };
        assert_eq!(
            heuristic_title(DraftKind::Issue, "", &no_commits),
            "flaky login"
        );
    }

    #[test]
    fn prompt_carries_context_and_reminder() {
        let context = RepoContext {
            repo: "orbit".into(),
            branch: Some("feat/x".into()),
            base: Some("main".into()),
            commits: vec!["feat: a".into()],
            changed: vec!["src/lib.rs | 2 +-".into()],
        };
        let prompt = user_prompt(DraftKind::PullRequest, "notes here", &context, None);
        assert!(prompt.contains("Repository name: orbit"));
        assert!(prompt.contains("Target base branch: main"));
        assert!(prompt.contains("<user-notes>"));
        assert!(prompt.contains("notes here"));
        assert!(prompt.contains("feat: a"));
        assert!(prompt.contains("src/lib.rs | 2 +-"));
        assert!(prompt.contains("ONLY return a single markdown code block"));

        let system = system_prompt(DraftKind::Issue, false);
        assert!(system.contains("GitHub issue"));
        assert!(system.contains("EXACTLY one fenced"));
        assert!(!system.contains("REPOSITORY TEMPLATE"));
    }

    #[test]
    fn template_is_embedded_and_filled_faithfully() {
        let template = RepoTemplate {
            name: "Bug report".into(),
            path: ".github/ISSUE_TEMPLATE/bug_report.yml".into(),
            body: "### What happened?\n\n<!-- describe -->\n\n### Steps\n\n- [ ] searched\n".into(),
            title_prefix: Some("bug: ".into()),
            labels: vec!["bug".into()],
        };
        let context = RepoContext {
            repo: "orbit".into(),
            commits: vec!["fix: guard input".into()],
            ..RepoContext::default()
        };
        let prompt = user_prompt(
            DraftKind::Issue,
            "the button is broken",
            &context,
            Some(&template),
        );
        assert!(
            prompt.contains("<template name=\"Bug report\">"),
            "{prompt}"
        );
        assert!(prompt.contains("### What happened?"));
        assert!(prompt.contains("Fill in the REPOSITORY TEMPLATE"));

        let system = system_prompt(DraftKind::Issue, true);
        assert!(system.contains("REPOSITORY TEMPLATE"));
        assert!(system.contains("checkbox"));

        // The heuristic drops the notes under the first heading, keeping the
        // rest of the template.
        let filled = fill_first_section(&template.body, "the button is broken");
        assert!(filled.starts_with("### What happened?\n\nthe button is broken"));
        assert!(filled.contains("- [ ] searched"));
    }

    #[test]
    fn fill_first_section_prepends_without_a_heading() {
        assert_eq!(
            fill_first_section("no headings here", "some notes"),
            "some notes\n\nno headings here"
        );
        assert_eq!(fill_first_section("body", ""), "body");
    }

    #[test]
    fn report_prompts_ask_for_a_heading_free_body_and_carry_the_notes() {
        let system = report_system_prompt(ReportKind::Bug);
        assert!(
            system.contains("Do NOT add any Markdown headings"),
            "{system}"
        );
        // The feature framing names the problem/proposal, not a bug.
        let feature = report_system_prompt(ReportKind::Feature);
        assert!(feature.contains("feature request"), "{feature}");
        assert!(feature.contains("proposed solution"), "{feature}");

        let prompt = report_user_prompt(ReportKind::Bug, "Crashes when I click Save");
        assert!(prompt.contains("Crashes when I click Save"), "{prompt}");
        assert!(prompt.contains("```text"), "{prompt}");
        // Empty notes still produce a usable prompt rather than a bare tag.
        assert!(report_user_prompt(ReportKind::Bug, "   ").contains("(no notes provided)"));
    }

    #[test]
    fn report_kinds_map_to_repository_labels() {
        assert_eq!(ReportKind::Bug.label(), Some("bug"));
        assert_eq!(ReportKind::Feature.label(), Some("enhancement"));
        assert_eq!(ReportKind::Other.label(), None);
        assert_eq!(ReportKind::default(), ReportKind::Bug);
    }
}
