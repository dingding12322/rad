//! Skill discovery — reads the same `SKILL.md` files pi loads.
//!
//! pi resolves skills from four auto-discovered roots, highest precedence
//! first (matching `PackageManager.addAutoDiscoveredResources`):
//!
//! 1. `<workspace>/.pi/skills`
//! 2. `.agents/skills` in the workspace and each ancestor up to the git root
//! 3. `~/.pi/agent/skills`
//! 4. `~/.agents/skills`
//!
//! A skill is a directory holding a `SKILL.md`; its YAML frontmatter supplies
//! a `name` (defaults to the directory name) and a required `description`.
//! `disable-model-invocation: true` keeps it out of the system prompt while
//! still listing it here. Duplicate names are deduped the way pi does — the
//! highest-precedence directory wins.
//!
//! The page can also enable/disable and delete skills. Disabling writes the
//! same `-<path>` override pi's own resource loader reads from the scope's
//! `settings.json` `skills` array (see [`set_enabled`]); deleting removes the
//! skill directory. Everything else here is read-only.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Map, Value};

/// Where a skill was discovered — the scope label shown on its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillScope {
    /// `<workspace>/.pi/skills` or an ancestor `.agents/skills`.
    Project,
    /// `~/.pi/agent/skills` — pi's own global skill store.
    Agent,
    /// `~/.agents/skills` — the user-wide agents store.
    User,
}

impl SkillScope {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SkillScope::Project => "Project",
            SkillScope::Agent => "Global",
            SkillScope::User => "User",
        }
    }
}

/// One discovered skill, flattened to what the settings page renders.
#[derive(Debug, Clone)]
pub(crate) struct Skill {
    pub name: String,
    /// Frontmatter description; empty when the skill declares none (pi will
    /// refuse to load it, so the page flags it).
    pub description: String,
    /// Absolute path to `SKILL.md`.
    pub file: PathBuf,
    pub scope: SkillScope,
    /// `disable-model-invocation: true` — listed, but not offered to the model.
    pub manual_only: bool,
    /// pi will load it — no `-<path>` override matches it.
    pub enabled: bool,
    /// `SKILL.md` size in bytes.
    pub size_bytes: u64,
    /// `SKILL.md` last-modified time.
    pub modified: Option<SystemTime>,
}

impl Skill {
    /// A skill pi will not load because its required description is missing.
    pub(crate) fn is_valid(&self) -> bool {
        !self.description.trim().is_empty()
    }

    /// The directory holding the skill's files.
    pub(crate) fn dir(&self) -> Option<&Path> {
        self.file.parent()
    }

    /// The slash command pi registers for this skill.
    pub(crate) fn invoke_command(&self) -> String {
        format!("/skill:{}", self.name)
    }
}

/// Discover every skill for `workspace`, deduped by name (project wins).
pub(crate) fn discover(workspace: &Path) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_files: HashSet<PathBuf> = HashSet::new();

    for (scope, root) in discovery_roots(workspace) {
        let mut files = Vec::new();
        collect_skill_files(&root, 0, &mut files);
        files.sort();
        for file in files {
            let Some(skill) = read_skill(&file, scope) else {
                continue;
            };
            let canonical = fs::canonicalize(&skill.file).unwrap_or_else(|_| skill.file.clone());
            if !seen_files.insert(canonical) {
                continue;
            }
            if !seen_names.insert(skill.name.clone()) {
                // Shadowed by a higher-precedence directory; pi would drop it.
                continue;
            }
            skills.push(skill);
        }
    }

    apply_enabled(&mut skills, workspace);

    skills.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.scope.label().cmp(b.scope.label()))
    });
    skills
}

/// Read the override patterns for each scope once and set `enabled` on every
/// skill. Only exact `+`/`-` entries are honoured; glob `!` excludes are left
/// to pi and would make a skill read as enabled here (rare, and the toggle
/// writes exact entries only).
fn apply_enabled(skills: &mut [Skill], workspace: &Path) {
    let user = read_skill_entries(&settings_path(SkillScope::Agent, workspace));
    let project = read_skill_entries(&settings_path(SkillScope::Project, workspace));
    for skill in skills {
        let (entries, base) = match skill.scope {
            SkillScope::Project => (&project, settings_base(SkillScope::Project, workspace)),
            SkillScope::Agent | SkillScope::User => {
                (&user, settings_base(SkillScope::Agent, workspace))
            }
        };
        skill.enabled = skill_enabled(&skill.file, entries, &base);
    }
}

/// The discovery roots, highest precedence first, as `(scope, dir)` pairs.
pub(crate) fn discovery_roots(workspace: &Path) -> Vec<(SkillScope, PathBuf)> {
    let mut roots = Vec::new();
    roots.push((SkillScope::Project, workspace.join(".pi/skills")));
    let user_agents = home_agents_skills();
    for dir in ancestor_agents_skills(workspace) {
        if dir != user_agents {
            roots.push((SkillScope::Project, dir));
        }
    }
    roots.push((SkillScope::Agent, agent_skills_dir()));
    roots.push((SkillScope::User, user_agents));
    roots
}

/// `~/.pi/agent/skills`.
pub(crate) fn agent_skills_dir() -> PathBuf {
    agent_dir().join("skills")
}

/// `~/.agents/skills`.
pub(crate) fn home_agents_skills() -> PathBuf {
    home_dir().join(".agents").join("skills")
}

fn agent_dir() -> PathBuf {
    home_dir().join(".pi").join("agent")
}

fn home_dir() -> PathBuf {
    crate::platform::home_dir()
}

/// `.agents/skills` in `start` and every ancestor up to the git root.
fn ancestor_agents_skills(start: &Path) -> Vec<PathBuf> {
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let git_root = find_git_root(&start);
    let mut dirs = Vec::new();
    let mut dir = start.as_path();
    loop {
        dirs.push(dir.join(".agents").join("skills"));
        if git_root.as_deref() == Some(dir) {
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    dirs
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Recursively collect `SKILL.md` files. A directory that holds one is a
/// skill root — pi does not descend past it.
fn collect_skill_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let skill_file = dir.join("SKILL.md");
    if skill_file.is_file() {
        out.push(skill_file);
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_skill_files(&path, depth + 1, out);
        }
    }
}

fn read_skill(file: &Path, scope: SkillScope) -> Option<Skill> {
    let content = fs::read_to_string(file).ok()?;
    let metadata = fs::metadata(file).ok();
    let frontmatter = parse_frontmatter(&content);
    let dir = file.parent()?.to_path_buf();
    let name = frontmatter
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "skill".to_string())
        });
    Some(Skill {
        name,
        description: frontmatter.description.unwrap_or_default(),
        file: file.to_path_buf(),
        scope,
        manual_only: frontmatter.disable_model_invocation,
        enabled: true,
        size_bytes: metadata.as_ref().map(fs::Metadata::len).unwrap_or(0),
        modified: metadata.and_then(|meta| meta.modified().ok()),
    })
}

// ── scope settings (enable / disable) ───────────────────────────────

/// The `settings.json` that holds this scope's skill overrides.
pub(crate) fn settings_path(scope: SkillScope, workspace: &Path) -> PathBuf {
    settings_base(scope, workspace).join("settings.json")
}

/// The directory override patterns are resolved against: pi's agent dir for
/// user scope, the project's `.pi` for project scope.
pub(crate) fn settings_base(scope: SkillScope, workspace: &Path) -> PathBuf {
    match scope {
        SkillScope::Project => workspace.join(".pi"),
        SkillScope::Agent | SkillScope::User => agent_dir(),
    }
}

/// The `skills` array entries from a settings file: plain custom paths and
/// `+`/`-`/`!` override patterns, as strings.
fn read_skill_entries(path: &Path) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    root.get("skills")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    entry.as_str().map(str::to_string).or_else(|| {
                        entry
                            .get("path")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// pi's `isEnabledByOverrides` reduced to exact `+`/`-` paths: a matching
/// `-` wins over a matching `+`, and no match means enabled.
fn skill_enabled(file: &Path, entries: &[String], base: &Path) -> bool {
    if entries
        .iter()
        .filter_map(|entry| entry.strip_prefix('-'))
        .any(|pattern| exact_path_match(file, pattern, base))
    {
        return false;
    }
    if entries
        .iter()
        .filter_map(|entry| entry.strip_prefix('+'))
        .any(|pattern| exact_path_match(file, pattern, base))
    {
        return true;
    }
    true
}

/// Match a `-`/`+` override against a skill's `SKILL.md` or its directory,
/// resolving relative patterns against the scope base.
fn exact_path_match(file: &Path, pattern: &str, base: &Path) -> bool {
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let pattern_path = Path::new(pattern);
    let absolute = if pattern_path.is_absolute() {
        pattern_path.to_path_buf()
    } else {
        base.join(pattern_path)
    };
    if absolute == file {
        return true;
    }
    file.parent().is_some_and(|dir| absolute == dir)
}

/// Enable or disable `skill` by writing/removing a `-<path>` override in the
/// scope's settings file. Every other key — and every other skill entry — is
/// preserved. A skill that is enabled has no override.
pub(crate) fn set_enabled(skill: &Skill, workspace: &Path, enabled: bool) -> Result<(), String> {
    write_override(skill, workspace, enabled)
}

/// Drop any exact override that pointed at `skill` (used after a delete, so
/// no dangling `-<path>` entry is left behind).
pub(crate) fn clear_overrides(skill: &Skill, workspace: &Path) -> Result<(), String> {
    if !settings_path(skill.scope, workspace).exists() {
        return Ok(());
    }
    write_override(skill, workspace, true)
}

fn write_override(skill: &Skill, workspace: &Path, enabled: bool) -> Result<(), String> {
    let path = settings_path(skill.scope, workspace);
    // Enabling removes an entry: with no settings file there is nothing to do
    // (and nothing worth creating).
    if enabled && !path.exists() {
        return Ok(());
    }
    let mut root: Map<String, Value> = match fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => {
            let value: Value = serde_json::from_str(&raw).map_err(|err| {
                tr!(
                    "errors.not_valid_json",
                    path = path.display().to_string(),
                    error = err
                )
            })?;
            match value {
                Value::Object(map) => map,
                _ => return Err(format!("{} is not a JSON object", path.display())),
            }
        }
        Ok(_) => Map::new(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(err) => {
            return Err(tr!(
                "errors.could_not_read",
                path = path.display().to_string(),
                error = err
            ))
        }
    };

    if root.get("skills").is_some_and(|value| !value.is_array()) {
        // An older pi schema stored `skills` as an object — never clobber it.
        return Err(tr!("skills.settings_not_array"));
    }
    let mut entries: Vec<Value> = root
        .get("skills")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Drop any existing exact override for this skill (both the file and its
    // directory, `+` or `-`) so toggling is idempotent.
    let before = entries.len();
    entries.retain(|entry| {
        let Some(text) = entry.as_str() else {
            return true;
        };
        let is_override = text.starts_with('+') || text.starts_with('-');
        !(is_override
            && exact_path_match(
                &skill.file,
                &text[1..],
                &settings_base(skill.scope, workspace),
            ))
    });
    let removed = entries.len() != before;
    if enabled && !removed {
        // Nothing to strip and no new override to add — leave the file alone.
        return Ok(());
    }
    if !enabled {
        entries.push(Value::String(format!("-{}", skill.file.display())));
    }
    root.insert("skills".to_string(), Value::Array(entries));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            tr!(
                "errors.could_not_create",
                path = parent.display().to_string(),
                error = err
            )
        })?;
    }
    let mut serialized = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|err| tr!("errors.could_not_serialize_settings", error = err))?;
    serialized.push('\n');
    fs::write(&path, serialized).map_err(|err| {
        tr!(
            "errors.could_not_write",
            path = path.display().to_string(),
            error = err
        )
    })
}

/// Delete a skill: remove its directory (or, for a skill declared directly at
/// a discovery root, just its `SKILL.md`), then drop any override.
pub(crate) fn delete(skill: &Skill, workspace: &Path) -> Result<(), String> {
    let Some(dir) = skill.dir() else {
        return Err(tr!("skills.no_parent_dir"));
    };
    // Never remove a discovery root itself — only the skill folder under it.
    if dir.file_name().and_then(|name| name.to_str()) == Some("skills") {
        fs::remove_file(&skill.file).map_err(|err| {
            tr!(
                "errors.could_not_delete",
                path = skill.file.display().to_string(),
                error = err
            )
        })?;
    } else {
        fs::remove_dir_all(dir).map_err(|err| {
            tr!(
                "errors.could_not_delete",
                path = dir.display().to_string(),
                error = err
            )
        })?;
    }
    let _ = clear_overrides(skill, workspace);
    Ok(())
}

/// The skill's markdown body, frontmatter stripped and line endings normalized.
pub(crate) fn read_body(file: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(file).map_err(|err| {
        tr!(
            "errors.could_not_read",
            path = file.display().to_string(),
            error = err
        )
    })?;
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Ok(normalized);
    }
    let Some(end) = normalized[3..].find("\n---") else {
        return Ok(normalized);
    };
    Ok(normalized[3 + end + 4..]
        .trim_start_matches('\n')
        .to_string())
}

/// `~`-shorten an absolute path for display.
pub(crate) fn display_path(path: &Path) -> String {
    if let Ok(rest) = path.strip_prefix(home_dir()) {
        return format!("~/{}", rest.to_string_lossy());
    }
    path.to_string_lossy().into_owned()
}

/// Human-readable byte size for the detail facts table.
pub(crate) fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < KB * KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{:.1} MB", value / (KB * KB))
    }
}

#[derive(Debug, Default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    disable_model_invocation: bool,
}

/// Parse the leading `---` YAML block for the three keys pi reads. A real
/// YAML parser is overkill: skill frontmatter is flat scalars, and anything
/// more exotic is left to pi where it matters.
fn parse_frontmatter(content: &str) -> Frontmatter {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut frontmatter = Frontmatter::default();
    if !normalized.starts_with("---") {
        return frontmatter;
    }
    let Some(end) = normalized[3..].find("\n---") else {
        return frontmatter;
    };
    let yaml = &normalized[4..3 + end];
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        if raw.starts_with(' ') || raw.starts_with('\t') || raw.trim().is_empty() {
            i += 1;
            continue;
        }
        if raw.trim_start().starts_with('#') {
            i += 1;
            continue;
        }
        let Some((key, value)) = raw.split_once(':') else {
            i += 1;
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if is_block_scalar(value) {
            let folded = value.starts_with('>');
            let (parts, next) = collect_indented(&lines, i + 1);
            let joined = parts.join(if folded { " " } else { "\n" });
            apply_frontmatter_value(&mut frontmatter, key, joined.trim());
            i = next;
            continue;
        }
        // A plain scalar may continue on following, more-indented lines —
        // common for long `description:` blocks.
        if value.is_empty() && is_scalar_key(key) {
            let (parts, next) = collect_indented(&lines, i + 1);
            let joined = parts.join(" ");
            if !joined.trim().is_empty() {
                apply_frontmatter_value(&mut frontmatter, key, joined.trim());
                i = next;
                continue;
            }
        }
        apply_frontmatter_value(&mut frontmatter, key, value);
        i += 1;
    }
    frontmatter
}

/// The keys that hold a scalar, so an empty value may fold with indented
/// continuation lines. (`metadata:` also has indented lines, but it is a
/// mapping we ignore.)
fn is_scalar_key(key: &str) -> bool {
    matches!(key, "name" | "description" | "disable-model-invocation")
}

/// Collect the more-indented lines after `start`, ending at the first line
/// that is not indented and not blank.
fn collect_indented(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut parts = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            parts.push(String::new());
            i += 1;
        } else if line.starts_with(' ') || line.starts_with('\t') {
            parts.push(line.trim().to_string());
            i += 1;
        } else {
            break;
        }
    }
    (parts, i)
}

fn is_block_scalar(value: &str) -> bool {
    matches!(value, "|" | ">")
        || value.starts_with("|-")
        || value.starts_with(">-")
        || value.starts_with("|+")
        || value.starts_with(">+")
}

fn apply_frontmatter_value(frontmatter: &mut Frontmatter, key: &str, value: &str) {
    match key {
        "name" => frontmatter.name = unquote(value),
        "description" => frontmatter.description = unquote(value),
        "disable-model-invocation" => {
            frontmatter.disable_model_invocation = value.eq_ignore_ascii_case("true")
        }
        _ => {}
    }
}

fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    let unquoted = if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            &value[1..value.len() - 1]
        } else {
            value
        }
    } else {
        value
    };
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_frontmatter() {
        let fm = parse_frontmatter("---\nname: review\ndescription: Review code\n---\nbody");
        assert_eq!(fm.name.as_deref(), Some("review"));
        assert_eq!(fm.description.as_deref(), Some("Review code"));
        assert!(!fm.disable_model_invocation);
    }

    #[test]
    fn parses_quoted_and_manual_invocation() {
        let fm = parse_frontmatter(
            "---\nname: \"my skill\"\ndescription: 'Does things'\ndisable-model-invocation: true\n---\n",
        );
        assert_eq!(fm.name.as_deref(), Some("my skill"));
        assert_eq!(fm.description.as_deref(), Some("Does things"));
        assert!(fm.disable_model_invocation);
    }

    #[test]
    fn parses_block_description() {
        let fm = parse_frontmatter(
            "---\nname: x\ndescription: |\n  First line\n  second line\n---\nbody",
        );
        assert_eq!(fm.description.as_deref(), Some("First line\nsecond line"));
    }

    #[test]
    fn parses_plain_multiline_description() {
        let fm = parse_frontmatter(
            "---\nname: x\ndescription:\n  First part\n  and the rest.\nlicense: MIT\n---\n",
        );
        assert_eq!(fm.description.as_deref(), Some("First part and the rest."));
    }

    #[test]
    fn missing_description_stays_empty() {
        let fm = parse_frontmatter("---\nname: x\n---\n");
        assert!(fm.description.is_none());
    }

    #[test]
    fn non_frontmatter_is_ignored() {
        let fm = parse_frontmatter("# Heading\n\ndescription: nope");
        assert!(fm.name.is_none());
        assert!(fm.description.is_none());
    }

    #[test]
    fn strips_frontmatter_from_body() {
        let dir = std::env::temp_dir().join(format!("orbit-skill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("SKILL.md");
        std::fs::write(&file, "---\nname: x\ndescription: y\n---\n# Body\n").unwrap();
        let body = read_body(&file).unwrap();
        assert_eq!(body, "# Body\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exact_overrides_match_file_or_dir() {
        let base = Path::new("/base");
        let file = Path::new("/base/skills/foo/SKILL.md");
        assert!(exact_path_match(file, "/base/skills/foo", base));
        assert!(exact_path_match(file, "/base/skills/foo/SKILL.md", base));
        assert!(exact_path_match(file, "./skills/foo/SKILL.md", base));
        assert!(!exact_path_match(file, "/base/skills/bar", base));
    }

    #[test]
    fn disable_wins_over_enable() {
        let base = Path::new("/base");
        let file = Path::new("/base/skills/foo/SKILL.md");
        let entries = vec![
            "+/base/skills/foo".to_string(),
            "-/base/skills/foo".to_string(),
        ];
        assert!(!skill_enabled(file, &entries, base));
        assert!(skill_enabled(file, &[], base));
        assert!(skill_enabled(
            file,
            &["+/base/skills/foo".to_string()],
            base
        ));
    }

    #[test]
    fn set_enabled_round_trips_project_settings() {
        let workspace =
            std::env::temp_dir().join(format!("orbit-skill-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        let skill = Skill {
            name: "foo".into(),
            description: "d".into(),
            file: workspace.join(".agents/skills/foo/SKILL.md"),
            scope: SkillScope::Project,
            manual_only: false,
            enabled: true,
            size_bytes: 0,
            modified: None,
        };

        set_enabled(&skill, &workspace, false).unwrap();
        let path = settings_path(SkillScope::Project, &workspace);
        let entries = read_skill_entries(&path);
        assert!(!skill_enabled(
            &skill.file,
            &entries,
            &settings_base(SkillScope::Project, &workspace)
        ));

        set_enabled(&skill, &workspace, true).unwrap();
        let entries = read_skill_entries(&path);
        assert!(skill_enabled(
            &skill.file,
            &entries,
            &settings_base(SkillScope::Project, &workspace)
        ));

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(12_687), "12.4 KB");
        assert_eq!(format_size(2_097_152), "2.0 MB");
    }
}
