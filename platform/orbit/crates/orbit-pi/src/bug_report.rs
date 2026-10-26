//! In-app bug reporting.
//!
//! Settings → Report a bug collects a description, attaches the machine's
//! build details (Orbit version, bundle identifier, host OS, pi CLI version,
//! and the signed-in `gh` user), and files a GitHub issue against the
//! project's own repository. Filing goes through the `gh` CLI when it is
//! installed and signed in — see [`crate::gh`] — and falls back to opening
//! GitHub's new-issue page with the report prefilled when it is not. Orbit
//! never stores a GitHub token either way.

use std::path::{Path, PathBuf};

use crate::gh;
use crate::issue_message::{self, ReportKind};

/// The repository every in-app report is filed against.
pub const REPO: &str = "imrj05/orbit";

/// The bundle identifier in `[package.metadata.bundle]`. Reported alongside the
/// version so a bug is never ambiguous with a renamed or forked build.
pub const APP_ID: &str = "dev.orbit.pi";

/// The build facts attached to every report. All are read once, off the UI
/// thread, at submission time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics {
    pub orbit_version: String,
    pub app_id: String,
    pub os: String,
    pub arch: String,
    pub pi_version: Option<String>,
    pub gh_version: Option<String>,
    pub gh_user: Option<String>,
    /// `App bundle` when running from a packaged `.app`, `Source build`
    /// otherwise.
    pub install: String,
}

impl Diagnostics {
    /// Probe the machine. Runs subprocesses (the host probe, `pi --version`,
    /// and `gh`), so callers run it on the background executor.
    pub fn collect(cwd: &Path) -> Self {
        let host = crate::platform::host();
        let pi_version = crate::onboarding::locate("pi")
            .and_then(|binary| crate::onboarding::version_of(&binary));
        let gh_status = gh::status(cwd);
        let gh_version = (!gh_status.detail.trim().is_empty()).then(|| gh_status.detail.clone());
        let gh_user = gh_status
            .authenticated
            .then(|| gh::current_user(cwd).ok())
            .flatten()
            .map(|user| user.login);
        Self {
            orbit_version: env!("CARGO_PKG_VERSION").to_string(),
            app_id: APP_ID.to_string(),
            os: host.label,
            arch: host.arch.to_string(),
            pi_version,
            gh_version,
            gh_user,
            install: install_kind(),
        }
    }

    /// The Markdown block appended under the reporter's own text.
    fn markdown(&self) -> String {
        let mut rows: Vec<(&str, String)> = vec![
            ("Orbit", self.orbit_version.clone()),
            ("App ID", self.app_id.clone()),
            ("OS", self.os.clone()),
            ("Architecture", self.arch.clone()),
            ("Install", self.install.clone()),
        ];
        if let Some(pi) = self.pi_version.as_deref().filter(|v| !v.is_empty()) {
            rows.push(("pi CLI", pi.to_string()));
        }
        if let Some(gh) = self.gh_version.as_deref().filter(|v| !v.is_empty()) {
            rows.push(("gh CLI", gh.to_string()));
        }
        if let Some(user) = self.gh_user.as_deref().filter(|v| !v.is_empty()) {
            rows.push(("Filed by", format!("@{user}")));
        }
        let mut out = String::from("<details>\n<summary>Diagnostics</summary>\n\n");
        for (label, value) in rows {
            out.push_str(&format!("- **{label}:** {value}\n"));
        }
        out.push_str("\n</details>\n");
        out
    }
}

/// What filing a report did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filed {
    /// `gh` created the issue; the URL is its location.
    Created(String),
    /// GitHub's web new-issue page was prepared with the report prefilled and
    /// the caller opens it. `screenshots` is the temp directory the attached
    /// images were written to, when the report carried any — GitHub only
    /// accepts binary attachments in its web editor, so the user drags them in
    /// from there.
    Browser {
        url: String,
        screenshots: Option<PathBuf>,
    },
}

/// File a report. Tries `gh issue create` on [`REPO`] first and falls back to
/// a prefilled GitHub URL when `gh` is missing or signed out. A genuine `gh`
/// failure is returned as `Err` so the form can show it.
///
/// `screenshots` (name + bytes) are written to a temp directory and uploaded
/// with `gh issue create --attach`; when `gh` is unavailable, or too old for
/// `--attach`, the web form is opened instead and the saved files are handed
/// back in [`Filed::Browser::screenshots`] for the user to drag in.
pub fn file(
    cwd: &Path,
    kind: ReportKind,
    title: &str,
    what: &str,
    steps: &str,
    diagnostics: &Diagnostics,
    screenshots: &[(String, Vec<u8>)],
) -> Result<Filed, String> {
    if screenshots.is_empty() {
        let body = issue_body(kind, what, steps, diagnostics, false);
        let status = gh::status(cwd);
        if status.installed && status.authenticated {
            return gh::create_issue(cwd, Some(REPO), title, &body, &kind_labels(kind), &[], &[])
                .map(Filed::Created);
        }
        return Ok(Filed::Browser {
            url: new_issue_url(title, &body, kind.label()),
            screenshots: None,
        });
    }

    // Screenshots: write them out, then let `gh` upload them. The body stays
    // free of a screenshots section because `--attach` appends the images.
    let directory = save_screenshots(screenshots)?;
    let paths: Vec<PathBuf> = screenshots
        .iter()
        .map(|(name, _)| directory.join(sanitize_name(name)))
        .collect();
    let status = gh::status(cwd);
    if status.installed && status.authenticated {
        let body = issue_body(kind, what, steps, diagnostics, false);
        match gh::create_issue(
            cwd,
            Some(REPO),
            title,
            &body,
            &kind_labels(kind),
            &[],
            &paths,
        ) {
            Ok(url) => return Ok(Filed::Created(url)),
            // A `gh` too old for `--attach` failed before creating anything;
            // fall through to the web form rather than dropping the shots.
            Err(err) if err.contains("unknown flag") || err.contains("unknown shorthand") => {}
            // Any other failure is real (the issue may even have been created
            // with the attachments that did upload); surface it instead of
            // silently opening a second form.
            Err(err) => return Err(err),
        }
    }
    let body = issue_body(kind, what, steps, diagnostics, true);
    Ok(Filed::Browser {
        url: new_issue_url(title, &body, kind.label()),
        screenshots: Some(directory),
    })
}

/// The repository labels for a report kind, as `gh` expects them.
fn kind_labels(kind: ReportKind) -> Vec<String> {
    kind.label().map(str::to_string).into_iter().collect()
}

/// Write the report's screenshots to a fresh temp directory. `gh issue create
/// --attach` uploads them from there, and the browser fallback points the user
/// at the same files to drag in. Returns the directory. Blocking; call off the
/// UI thread.
pub fn save_screenshots(files: &[(String, Vec<u8>)]) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let dir = crate::temp_files::ensure_dir()
        .map_err(|err| err.to_string())?
        .join(format!("bug-report-{stamp}"));
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    for (name, bytes) in files {
        std::fs::write(dir.join(sanitize_name(name)), bytes).map_err(|err| err.to_string())?;
    }
    Ok(dir)
}

/// A screenshot filename safe to join onto the temp directory: the final path
/// component, with anything but letters, digits, dot, dash, and underscore
/// replaced so a crafted name cannot escape the directory.
fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "screenshot.png".to_string()
    } else {
        cleaned
    }
}

/// Generate a report draft (title + description) from the user's own context
/// with the app's active model, falling back to a local draft when pi is
/// unavailable. Blocking; call on the background executor.
pub fn draft(
    cwd: &Path,
    provider: Option<&str>,
    model: Option<&str>,
    kind: ReportKind,
    context: &str,
) -> issue_message::Draft {
    match issue_message::generate_report(cwd, provider, model, kind, context) {
        Ok(draft) => draft,
        Err(_) => fallback_draft(context),
    }
}

/// The no-model fallback: the first note line as the title, the notes as the
/// description.
fn fallback_draft(context: &str) -> issue_message::Draft {
    let context = context.trim();
    let title = context
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            line.trim_end_matches(['.', ':', ';'])
                .chars()
                .take(72)
                .collect::<String>()
        })
        .unwrap_or_default();
    issue_message::Draft {
        title,
        body: context.to_string(),
    }
}

/// The Markdown body of a report: the reporter's own sections, then the
/// machine-readable diagnostics the maintainers ask for on the issue form.
/// `screenshots` adds a section for images the user will drag in from GitHub's
/// web editor (the API cannot attach them).
pub fn issue_body(
    kind: ReportKind,
    what: &str,
    steps: &str,
    diagnostics: &Diagnostics,
    screenshots: bool,
) -> String {
    let (what_heading, steps_heading) = match kind {
        ReportKind::Bug => ("What happened?", "Steps to reproduce"),
        ReportKind::Feature => ("Problem", "Proposed solution"),
        ReportKind::Other => ("Details", "Additional details"),
    };
    let steps = steps.trim();
    let steps = if steps.is_empty() {
        "_No response._"
    } else {
        steps
    };
    let mut body = format!("## {what_heading}\n\n{what}\n\n## {steps_heading}\n\n{steps}\n");
    if screenshots {
        body.push_str("\n## Screenshots\n\n<!-- Drag the screenshots here to attach them. -->\n");
    }
    format!("{body}\n---\n\n{}", diagnostics.markdown())
}

/// GitHub's new-issue page for [`REPO`], prefilled with the title, body, and
/// label the in-app form collected.
pub fn new_issue_url(title: &str, body: &str, label: Option<&str>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("title", title);
    serializer.append_pair("body", body);
    if let Some(label) = label {
        serializer.append_pair("labels", label);
    }
    format!(
        "https://github.com/{REPO}/issues/new?{}",
        serializer.finish()
    )
}

/// How this build was installed. A packaged macOS app runs from
/// `<App>.app/Contents/MacOS/`; anything else is a source build (or a Linux /
/// Windows artifact), which is enough to route a bug.
fn install_kind() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    if exe.to_string_lossy().contains(".app/Contents/MacOS") {
        "App bundle".to_string()
    } else {
        "Source build".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostics() -> Diagnostics {
        Diagnostics {
            orbit_version: "1.2.3".into(),
            app_id: APP_ID.into(),
            os: "macOS 15.2".into(),
            arch: "aarch64".into(),
            pi_version: Some("0.9.0".into()),
            gh_version: Some("gh version 2.40.1".into()),
            gh_user: Some("octocat".into()),
            install: "App bundle".into(),
        }
    }

    #[test]
    fn body_carries_the_reporter_text_and_the_build_facts() {
        let body = issue_body(
            ReportKind::Bug,
            "It crashed",
            "1. open\n2. click",
            &diagnostics(),
            false,
        );
        assert!(body.starts_with("## What happened?\n\nIt crashed"));
        assert!(body.contains("## Steps to reproduce\n\n1. open\n2. click"));
        // The maintainers' checklist items are attached, not typed by hand.
        assert!(body.contains("**Orbit:** 1.2.3"), "{body}");
        assert!(body.contains("**App ID:** dev.orbit.pi"), "{body}");
        assert!(body.contains("**pi CLI:** 0.9.0"), "{body}");
        assert!(body.contains("**Filed by:** @octocat"), "{body}");
    }

    #[test]
    fn feature_bodies_use_problem_and_solution_headings() {
        let body = issue_body(
            ReportKind::Feature,
            "Pasting a file is slow",
            "Add a progress bar",
            &diagnostics(),
            false,
        );
        assert!(
            body.starts_with("## Problem\n\nPasting a file is slow"),
            "{body}"
        );
        assert!(
            body.contains("## Proposed solution\n\nAdd a progress bar"),
            "{body}"
        );
    }

    #[test]
    fn screenshots_add_a_drag_target_section() {
        let body = issue_body(ReportKind::Bug, "Broken", "", &diagnostics(), true);
        assert!(body.contains("## Screenshots"), "{body}");
        // The section is absent unless the report carries screenshots.
        let without = issue_body(ReportKind::Bug, "Broken", "", &diagnostics(), false);
        assert!(!without.contains("## Screenshots"), "{without}");
    }

    #[test]
    fn missing_steps_render_the_no_response_placeholder() {
        let body = issue_body(ReportKind::Bug, "Broken", "   ", &diagnostics(), false);
        assert!(
            body.contains("## Steps to reproduce\n\n_No response._"),
            "{body}"
        );
    }

    #[test]
    fn optional_diagnostics_are_omitted_when_absent() {
        let mut diags = diagnostics();
        diags.pi_version = None;
        diags.gh_user = None;
        let body = issue_body(ReportKind::Bug, "Broken", "", &diags, false);
        assert!(!body.contains("pi CLI"), "{body}");
        assert!(!body.contains("Filed by"), "{body}");
    }

    #[test]
    fn the_browser_fallback_url_is_prefilled_and_encoded() {
        let url = new_issue_url("bug: it crashed", "line one\nline two & more", Some("bug"));
        assert!(url.starts_with("https://github.com/imrj05/orbit/issues/new?"));
        assert!(url.contains("title=bug%3A+it+crashed"), "{url}");
        assert!(url.contains("body=line+one%0Aline+two+%26+more"), "{url}");
        assert!(url.contains("labels=bug"), "{url}");
        // Other carries no label, so none is appended.
        assert!(!new_issue_url("t", "b", None).contains("labels="));
    }

    #[test]
    fn save_screenshots_writes_named_files_into_a_fresh_directory() {
        let files = vec![
            ("shot one.png".to_string(), vec![1, 2, 3]),
            ("../../escape.png".to_string(), vec![4, 5]),
        ];
        let dir = save_screenshots(&files).unwrap();
        assert!(dir.join("shot_one.png").exists());
        assert_eq!(
            std::fs::read(dir.join("shot_one.png")).unwrap(),
            vec![1, 2, 3]
        );
        // A crafted name keeps only its final component, so it cannot escape.
        assert_eq!(sanitize_name("../../escape.png"), "escape.png");
        assert!(dir.join("escape.png").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fallback_draft_uses_the_first_line_as_the_title() {
        let draft = fallback_draft("Crashes on launch.\n\nIt dies before the window opens.");
        assert_eq!(draft.title, "Crashes on launch");
        assert!(draft.body.contains("It dies before the window opens."));
    }

    #[test]
    fn fallback_draft_truncates_a_long_title() {
        let draft = fallback_draft(&"x".repeat(200));
        assert_eq!(draft.title.chars().count(), 72);
    }
}
