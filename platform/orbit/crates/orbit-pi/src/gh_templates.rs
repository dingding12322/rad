//! Repository-provided issue and pull-request templates.
//!
//! GitHub looks for these in a handful of well-known places; Orbit mirrors that
//! lookup so the Git page's New issue / New pull request forms can prefill and
//! generate against the repository's own template. Both plain Markdown
//! templates (with optional YAML front matter) and the newer YAML "issue forms"
//! are supported; an issue form is flattened into a Markdown skeleton, since
//! Orbit renders a plain body field rather than GitHub's dynamic form.
//!
//! Everything here is pure filesystem reads and returns an empty list when the
//! repository has no templates — never an error.

use std::fs;
use std::path::{Path, PathBuf};

/// Read at most this much of a template; anything larger is not a template.
const MAX_TEMPLATE_BYTES: u64 = 256 * 1024;

/// The legacy single-file locations GitHub checks, in priority order.
const ISSUE_TEMPLATE_FILES: &[&str] = &[
    ".github/ISSUE_TEMPLATE.md",
    ".github/issue_template.md",
    ".github/ISSUE_TEMPLATE",
    "docs/ISSUE_TEMPLATE.md",
    "ISSUE_TEMPLATE.md",
];

/// The single-file pull-request template locations GitHub checks, in priority
/// order.
const PULL_TEMPLATE_FILES: &[&str] = &[
    ".github/PULL_REQUEST_TEMPLATE.md",
    ".github/pull_request_template.md",
    "docs/PULL_REQUEST_TEMPLATE.md",
    "PULL_REQUEST_TEMPLATE.md",
    "pull_request_template.md",
];

/// One repository template, flattened for the body field and the generator.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoTemplate {
    /// A human label (`Bug report`, from the form's `name:`), or the filename.
    pub name: String,
    /// Workspace-relative path, for identifying the source.
    pub path: String,
    /// The Markdown skeleton to prefill the body with and to hand the model.
    pub body: String,
    /// A title prefix the template asks for (`bug: `).
    pub title_prefix: Option<String>,
    /// Labels the template asks for.
    pub labels: Vec<String>,
}

impl RepoTemplate {
    /// A short label for a picker chip: the name, or the file stem.
    pub fn label(&self) -> String {
        if self.name.trim().is_empty() {
            humanize(
                Path::new(&self.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&self.path),
            )
        } else {
            self.name.clone()
        }
    }
}

/// The repository's issue templates, in GitHub's lookup order: the
/// `.github/ISSUE_TEMPLATE/` directory (minus `config.yml`) if it has any
/// templates, else the first legacy single-file template.
pub fn issue_templates(root: &Path) -> Vec<RepoTemplate> {
    let dir = root.join(".github").join("ISSUE_TEMPLATE");
    let from_dir = templates_in_dir(root, &dir, true);
    if !from_dir.is_empty() {
        return from_dir;
    }
    first_template_file(root, ISSUE_TEMPLATE_FILES)
}

/// The repository's pull-request templates: the `.github/PULL_REQUEST_TEMPLATE/`
/// directory if present, else the first single-file template.
pub fn pull_templates(root: &Path) -> Vec<RepoTemplate> {
    let dir = root.join(".github").join("PULL_REQUEST_TEMPLATE");
    let from_dir = templates_in_dir(root, &dir, false);
    if !from_dir.is_empty() {
        return from_dir;
    }
    first_template_file(root, PULL_TEMPLATE_FILES)
}

/// Every template file directly inside `dir`, sorted by filename. `issue` skips
/// `config.yml`/`config.yaml` and only accepts `.md`/`.yml`/`.yaml`.
fn templates_in_dir(root: &Path, dir: &Path, issue: bool) -> Vec<RepoTemplate> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect(),
        Err(_) => return Vec::new(),
    };
    paths.sort_by_key(|a| file_name(a));

    let mut out = Vec::new();
    for path in paths {
        let file = file_name(&path);
        let lower = file.to_ascii_lowercase();
        if issue && (lower == "config.yml" || lower == "config.yaml") {
            continue;
        }
        let Some(content) = read_text(&path) else {
            continue;
        };
        let rel = relative(root, &path);
        if lower.ends_with(".md") {
            out.push(parse_markdown(&content, &rel, &file));
        } else if issue && (lower.ends_with(".yml") || lower.ends_with(".yaml")) {
            out.push(parse_issue_form(&content, &rel, &file));
        }
    }
    out
}

/// The first existing template in `candidates`, parsed as Markdown.
fn first_template_file(root: &Path, candidates: &[&str]) -> Vec<RepoTemplate> {
    for candidate in candidates {
        let path = root.join(candidate);
        let Some(content) = read_text(&path) else {
            continue;
        };
        let rel = relative(root, &path);
        let file = file_name(&path);
        return vec![parse_markdown(&content, &rel, &file)];
    }
    Vec::new()
}

fn read_text(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_TEMPLATE_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.trim().is_empty())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

/// A workspace-relative, forward-slash path (so it reads the same on Windows).
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Parse a Markdown template, reading `name`/`title`/`labels` from YAML front
/// matter when present and treating the remainder as the body.
fn parse_markdown(content: &str, path: &str, fallback_name: &str) -> RepoTemplate {
    let (front, body) = split_front_matter(content);
    let mut name = None;
    let mut title = None;
    let mut labels = Vec::new();
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => {
                let value = scalar(value).trim().to_string();
                if !value.is_empty() {
                    name = Some(value);
                }
            }
            "title" => title = Some(scalar(value)),
            "labels" => {
                labels = scalar(value)
                    .split(',')
                    .map(str::trim)
                    .filter_map(list_item)
                    .collect()
            }
            _ => {}
        }
    }
    RepoTemplate {
        name: name.unwrap_or_else(|| humanize(fallback_name)),
        path: path.to_string(),
        body: body.trim().to_string(),
        title_prefix: title.filter(|value| !value.trim().is_empty()),
        labels,
    }
}

/// Split leading `---`-fenced YAML front matter from the body. Without a valid
/// fence the whole document is the body.
fn split_front_matter(content: &str) -> (String, String) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = content.lines();
    match lines.next() {
        Some(first) if first.trim() == "---" => {}
        _ => return (String::new(), content.to_string()),
    }
    let mut front = String::new();
    let mut body = String::new();
    let mut closed = false;
    for line in lines {
        if !closed && line.trim() == "---" {
            closed = true;
            continue;
        }
        if closed {
            body.push_str(line);
            body.push('\n');
        } else {
            front.push_str(line);
            front.push('\n');
        }
    }
    if !closed {
        return (String::new(), content.to_string());
    }
    (front, body)
}

/// Strip a matching pair of surrounding quotes from a YAML scalar, preserving
/// inner whitespace (for titles like `"bug: "`).
fn scalar(value: &str) -> String {
    let value = value.trim();
    let bytes = value.as_bytes();
    if value.len() >= 2 {
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// A `labels:` entry, tolerating `[bug, ui]`, `bug, ui`, or `bug`.
fn list_item(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('[').trim_end_matches(']');
    let value = value.trim().trim_matches(['"', '\'']).trim();
    (!value.is_empty()).then(|| value.to_string())
}

// ── YAML issue forms ──────────────────────────────────────────────────

/// One `body:` entry of a YAML issue form, reduced to what the skeleton needs.
#[derive(Default)]
struct FormField {
    kind: String,
    label: Option<String>,
    description: Option<String>,
    placeholder: Option<String>,
    options: Vec<String>,
    value: Option<String>,
    render: Option<String>,
}

/// Flatten a GitHub issue form (`*.yml`) into a Markdown skeleton. Top-level
/// `name`/`title`/`labels` are read as metadata; each `body:` entry becomes a
/// heading, with descriptions as HTML comments and dropdown/checkbox options
/// rendered as comment lists and checkboxes.
fn parse_issue_form(content: &str, path: &str, fallback_name: &str) -> RepoTemplate {
    let lines: Vec<&str> = content.lines().collect();
    let mut name = None;
    let mut title = None;
    let mut labels = Vec::new();
    let mut fields: Vec<FormField> = Vec::new();
    let mut current: Option<FormField> = None;
    let mut in_body = false;
    let mut i = 0usize;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if !in_body {
            if trimmed == "body:" || trimmed.starts_with("body:") {
                in_body = true;
                i += 1;
                continue;
            }
            if !raw.starts_with([' ', '\t']) {
                if let Some((key, value)) = raw.split_once(':') {
                    match key.trim().to_ascii_lowercase().as_str() {
                        "name" => {
                            let value = scalar(value).trim().to_string();
                            if !value.is_empty() {
                                name = Some(value);
                            }
                        }
                        "title" => title = Some(scalar(value)),
                        "labels" => {
                            labels = scalar(value)
                                .split(',')
                                .map(str::trim)
                                .filter_map(list_item)
                                .collect()
                        }
                        _ => {}
                    }
                }
            }
            i += 1;
            continue;
        }

        // A new `body:` list entry.
        if let Some(kind) = trimmed.strip_prefix("- type:") {
            if let Some(field) = current.take() {
                fields.push(field);
            }
            current = Some(FormField {
                kind: kind.trim().to_ascii_lowercase(),
                ..FormField::default()
            });
            i += 1;
            continue;
        }

        let indent = raw.len() - raw.trim_start().len();
        let Some(field) = current.as_mut() else {
            i += 1;
            continue;
        };
        if let Some(value) = trimmed.strip_prefix("- label:") {
            field.options.push(scalar(value));
            i += 1;
        } else if let Some(value) = trimmed.strip_prefix("- ") {
            // A plain list item: a `dropdown` option (checkbox options carry a
            // `label:` and are handled just above).
            field.options.push(scalar(value));
            i += 1;
        } else if let Some(value) = trimmed.strip_prefix("label:") {
            field.label = Some(scalar(value));
            i += 1;
        } else if let Some(value) = trimmed.strip_prefix("description:") {
            field.description = Some(scalar(value));
            i += 1;
        } else if let Some(value) = trimmed.strip_prefix("render:") {
            field.render = Some(scalar(value));
            i += 1;
        } else if let Some(value) = trimmed.strip_prefix("placeholder:") {
            if is_block(value) {
                field.placeholder = Some(consume_block(&lines, &mut i, indent));
            } else {
                field.placeholder = Some(scalar(value));
                i += 1;
            }
        } else if let Some(value) = trimmed.strip_prefix("value:") {
            if is_block(value) {
                field.value = Some(consume_block(&lines, &mut i, indent));
            } else {
                field.value = Some(scalar(value));
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    if let Some(field) = current.take() {
        fields.push(field);
    }

    RepoTemplate {
        name: name.unwrap_or_else(|| humanize(fallback_name)),
        path: path.to_string(),
        body: render_issue_form(&fields),
        title_prefix: title.filter(|value| !value.trim().is_empty()),
        labels,
    }
}

/// Whether a YAML value introduces a literal/folded block (`|`, `>-`, …).
fn is_block(value: &str) -> bool {
    let value = value.trim();
    value.starts_with('|') || value.starts_with('>')
}

/// Collect a YAML block scalar's indented lines and dedent them. Leaves `i` on
/// the first line that is not part of the block.
fn consume_block(lines: &[&str], i: &mut usize, key_indent: usize) -> String {
    *i += 1;
    let start = *i;
    let mut end = start;
    while end < lines.len() {
        let raw = lines[end];
        if raw.trim().is_empty() {
            end += 1;
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if indent <= key_indent {
            break;
        }
        end += 1;
    }
    let block = &lines[start..end];
    let min_indent = block
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let text = block
        .iter()
        .map(|line| {
            if line.len() >= min_indent {
                &line[min_indent..]
            } else {
                line.trim_start()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    *i = end;
    text.trim_end().to_string()
}

/// Render the parsed form fields into a Markdown skeleton.
fn render_issue_form(fields: &[FormField]) -> String {
    let mut out = String::new();
    for field in fields {
        match field.kind.as_str() {
            "markdown" => {
                if let Some(value) = field.value.as_ref().filter(|v| !v.trim().is_empty()) {
                    out.push_str("<!--\n");
                    out.push_str(value.trim());
                    out.push_str("\n-->\n\n");
                }
            }
            "checkboxes" => {
                if let Some(label) = field.label.as_ref().filter(|l| !l.trim().is_empty()) {
                    out.push_str("### ");
                    out.push_str(label.trim());
                    out.push_str("\n\n");
                }
                for option in &field.options {
                    out.push_str("- [ ] ");
                    out.push_str(option);
                    out.push('\n');
                }
                out.push('\n');
            }
            _ => {
                if let Some(label) = field.label.as_ref() {
                    out.push_str("### ");
                    out.push_str(label.trim());
                    out.push_str("\n\n");
                }
                if let Some(description) = field.description.as_ref() {
                    out.push_str("<!-- ");
                    out.push_str(description.trim());
                    out.push_str(" -->\n\n");
                }
                if let Some(placeholder) = field.placeholder.as_ref() {
                    out.push_str(placeholder.trim());
                    out.push_str("\n\n");
                }
                if field.kind == "dropdown" && !field.options.is_empty() {
                    out.push_str("<!-- one of: ");
                    out.push_str(&field.options.join(", "));
                    out.push_str(" -->\n\n");
                }
                if field.render.as_deref() == Some("shell") {
                    out.push_str("```shell\n\n```\n\n");
                }
            }
        }
    }
    out.trim_end().to_string()
}

/// A filename reduced to a readable label (`bug_report.yml` → `Bug report`).
fn humanize(file_name: &str) -> String {
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    let stem = if !stem.chars().any(|c| c.is_lowercase()) {
        stem.to_ascii_lowercase()
    } else {
        stem.to_string()
    };
    let spaced = stem.replace(['_', '-'], " ");
    let mut chars = spaced.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "orbit-gh-templates-{}-{}",
            std::process::id(),
            name
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn reads_a_plain_markdown_pull_template() {
        let root = scratch("pr");
        fs::create_dir_all(root.join(".github")).unwrap();
        fs::write(
            root.join(".github/PULL_REQUEST_TEMPLATE.md"),
            "## Summary\n\n<!-- why -->\n\n## Checklist\n\n- [ ] tests\n",
        )
        .unwrap();
        let templates = pull_templates(&root);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].path, ".github/PULL_REQUEST_TEMPLATE.md");
        assert!(templates[0].body.starts_with("## Summary"));
        assert!(templates[0].title_prefix.is_none());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reads_front_matter_and_body_from_markdown() {
        let root = scratch("md");
        fs::create_dir_all(root.join(".github/ISSUE_TEMPLATE")).unwrap();
        fs::write(
            root.join(".github/ISSUE_TEMPLATE/bug.md"),
            "---\nname: Bug report\nabout: Something broke\ntitle: \"bug: \"\nlabels: bug, needs-triage\n---\n\nDescribe the bug.\n",
        )
        .unwrap();
        let templates = issue_templates(&root);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Bug report");
        assert_eq!(templates[0].title_prefix.as_deref(), Some("bug: "));
        assert_eq!(templates[0].labels, vec!["bug", "needs-triage"]);
        assert_eq!(templates[0].body, "Describe the bug.");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn flattens_a_yaml_issue_form_and_skips_config() {
        let root = scratch("form");
        fs::create_dir_all(root.join(".github/ISSUE_TEMPLATE")).unwrap();
        fs::write(
            root.join(".github/ISSUE_TEMPLATE/config.yml"),
            "blank_issues_enabled: true\n",
        )
        .unwrap();
        fs::write(
            root.join(".github/ISSUE_TEMPLATE/bug_report.yml"),
            "name: Bug report\ndescription: Something broke.\ntitle: \"bug: \"\nlabels: [\"bug\"]\nbody:\n\
             \x20 - type: markdown\n    attributes:\n      value: |\n        Thanks for reporting.\n\n\
             \x20 - type: textarea\n    id: what\n    attributes:\n      label: What happened?\n      description: A clear description.\n      placeholder: |\n        1. Open\n        2. Click\n    validations:\n      required: true\n\n\
             \x20 - type: dropdown\n    id: install\n    attributes:\n      label: How did you install?\n      options:\n        - cargo run\n        - DMG\n\n\
             \x20 - type: checkboxes\n    id: ack\n    attributes:\n      label: Acknowledgements\n      options:\n        - label: I searched existing issues\n          required: true\n",
        )
        .unwrap();

        let templates = issue_templates(&root);
        assert_eq!(templates.len(), 1, "config.yml must be skipped");
        let template = &templates[0];
        assert_eq!(template.name, "Bug report");
        assert_eq!(template.title_prefix.as_deref(), Some("bug: "));
        assert_eq!(template.labels, vec!["bug"]);
        assert!(
            template.body.contains("Thanks for reporting."),
            "{}",
            template.body
        );
        assert!(
            template.body.contains("### What happened?"),
            "{}",
            template.body
        );
        assert!(
            template.body.contains("<!-- A clear description. -->"),
            "{}",
            template.body
        );
        assert!(
            template.body.contains("1. Open\n2. Click"),
            "{}",
            template.body
        );
        assert!(
            template.body.contains("### How did you install?"),
            "{}",
            template.body
        );
        assert!(
            template.body.contains("<!-- one of: cargo run, DMG -->"),
            "{}",
            template.body
        );
        assert!(
            template.body.contains("### Acknowledgements"),
            "{}",
            template.body
        );
        assert!(
            template.body.contains("- [ ] I searched existing issues"),
            "{}",
            template.body
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_single_file_is_used_without_a_directory() {
        let root = scratch("legacy");
        fs::write(root.join("ISSUE_TEMPLATE.md"), "Report it here.\n").unwrap();
        let templates = issue_templates(&root);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].path, "ISSUE_TEMPLATE.md");
        assert!(templates[0].body.contains("Report it here."));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_templates_are_an_empty_list() {
        let root = scratch("none");
        assert!(issue_templates(&root).is_empty());
        assert!(pull_templates(&root).is_empty());
        fs::remove_dir_all(root).ok();
    }

    /// A regression guard against this repository's real templates (two YAML
    /// issue forms and a Markdown pull-request template).
    #[test]
    fn parses_this_repositorys_own_templates() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let issues = issue_templates(&root);
        assert!(
            issues.iter().any(|template| template.name == "Bug report"),
            "bug_report.yml should be discovered: {issues:#?}"
        );
        assert!(
            issues
                .iter()
                .any(|template| template.name == "Feature request"),
            "feature_request.yml should be discovered"
        );
        assert!(!issues
            .iter()
            .any(|template| template.path.ends_with("config.yml")));

        let pulls = pull_templates(&root);
        assert_eq!(pulls.len(), 1);
        assert!(pulls[0].body.contains("## Summary"), "{}", pulls[0].body);
        assert!(pulls[0].body.contains("- [ ] Bug fix"), "{}", pulls[0].body);
    }
}
