//! Composer autocomplete — `/` command and `@` file mentions.
//!
//! Typing `/` at the very start of the composer (with the caret inside that
//! first word) opens a slash-command menu fed by pi's `get_commands`; typing
//! `@` after whitespace (or at the start) opens a workspace file menu. Both
//! are filtered live by the text between the sigil and the caret.
//!
//! The trigger lives entirely in the composer's text: detection is a pure
//! function of (content, cursor), so the popup opens/closes by itself as the
//! text changes — no extra state to keep in sync. Keyboard interaction is
//! negotiated through a shared `AutocompleteState`: the composer redirects
//! ↑/↓ to highlight movement, while Enter/Escape are intercepted by the app
//! (they already dispatch there as `Submit`/`AbortRun`).

use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

/// Which surface the active trigger opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// `/command` — caret is inside the first word of the composer.
    Slash,
    /// `@file` — the token ends at the caret, starts after whitespace.
    At,
}

/// An active autocomplete trigger: `content[start..end]` is the token being
/// completed (including the sigil); `query` is the text after the sigil.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub kind: TriggerKind,
    pub start: usize,
    pub end: usize,
    pub query: String,
}

/// Detect an autocomplete trigger at `cursor` in `content`.
///
/// - Slash: content starts with `/` and the caret sits before the first
///   whitespace — the whole first word is the token.
/// - At: an `@` preceded by line start or whitespace, with no whitespace
///   between it and the caret — `@query` is the token.
pub fn detect_trigger(content: &str, cursor: usize) -> Option<Trigger> {
    let cursor = cursor.min(content.len());
    // Slash only opens at the very start of the message.
    if let Some(rest) = content.strip_prefix('/') {
        let token_end = 1 + rest.find(char::is_whitespace).unwrap_or(rest.len());
        if cursor <= token_end {
            return Some(Trigger {
                kind: TriggerKind::Slash,
                start: 0,
                end: token_end,
                query: content[1..cursor].to_string(),
            });
        }
    }
    // At: last `@` before the caret; must start a token and run unbroken
    // (no whitespace) to the caret.
    let head = &content[..cursor];
    let at = head.rfind('@')?;
    if !(at == 0 || content[..at].ends_with(char::is_whitespace)) {
        return None;
    }
    let query = &content[at + 1..cursor];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    Some(Trigger {
        kind: TriggerKind::At,
        start: at,
        end: cursor,
        query: query.to_string(),
    })
}

/// Which composer token a span paints as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionKind {
    /// `/command` — the leading token of a message.
    Command,
    /// `@file` — a workspace reference.
    File,
}

/// A paint-colored span of the composer text. Byte offsets into the raw
/// content, so the caller can slice runs without disturbing the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionSpan {
    pub range: Range<usize>,
    pub kind: MentionKind,
}

/// Locate the tokens the composer paints in their own color: a leading
/// `/command` and every `@file` mention. Pure, sorted and non-overlapping,
/// so the editor's caret/selection model is untouched — this is paint only.
///
/// The rules mirror [`detect_trigger`]: a slash token only opens at the very
/// start of the message and must look like a command name (a path like
/// `/usr/local` stays plain); an `@` must sit at a token boundary and runs
/// unbroken to the next whitespace.
pub fn tokenize_mentions(content: &str) -> Vec<MentionSpan> {
    let mut spans = Vec::new();

    // Leading `/command`: the whole first word, only when every character
    // after the sigil reads as a command name.
    if content.starts_with('/') {
        let word_end = content.find(char::is_whitespace).unwrap_or(content.len());
        let name = &content[1..word_end];
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
        {
            spans.push(MentionSpan {
                range: 0..word_end,
                kind: MentionKind::Command,
            });
        }
    }

    // `@file` mentions: each `@` at a token boundary, running to whitespace.
    for (at, _) in content.match_indices('@') {
        if at != 0 && !content[..at].ends_with(char::is_whitespace) {
            continue;
        }
        let end = content[at..]
            .find(char::is_whitespace)
            .map(|off| at + off)
            .unwrap_or(content.len());
        if end > at + 1 {
            spans.push(MentionSpan {
                range: at..end,
                kind: MentionKind::File,
            });
        }
    }

    spans
}

/// Where a `/` command comes from — the scope badge shown on its row.
///
/// pi reports provenance in `source` (`extension`/`prompt`/`skill`) plus a
/// `sourceInfo` of `scope`/`origin`/`path`; Orbit folds that into the five
/// labels the autocomplete menu shows on the right of each command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandScope {
    /// pi's built-in commands and installed packages (`npm:`/`git:`) — part
    /// of the setup, available in every session.
    Builtin,
    /// A skill, invoked as `/skill:<name>`.
    Skill,
    /// One of Orbit's bundled extensions (materialized under `~/.orbit-pi/`).
    Orbit,
    /// A user-authored extension or prompt template (a top-level file in
    /// `~/.pi/agent/` or `.agents/`, not an installed package).
    Custom,
    /// Project-local (workspace `.pi/` or `.agents/`); the badge is the
    /// project folder's name.
    Project(String),
}

impl CommandScope {
    /// The badge text shown on the row (the project name is dynamic).
    pub fn label(&self) -> String {
        match self {
            Self::Builtin => tr!("autocomplete.scope_builtin"),
            Self::Skill => tr!("autocomplete.scope_skills"),
            Self::Orbit => tr!("autocomplete.scope_orbit"),
            Self::Custom => tr!("autocomplete.scope_custom"),
            Self::Project(name) => name.clone(),
        }
    }
}

/// Classify a `get_commands` entry into a scope badge. Pure so the mapping is
/// unit-testable; `workspace` names the project badge, `home` locates Orbit's
/// bundled extensions.
pub fn classify_scope(
    source: &str,
    scope: Option<&str>,
    origin: Option<&str>,
    path: Option<&str>,
    workspace: Option<&Path>,
    home: Option<&Path>,
) -> CommandScope {
    // Skills win over their scope: a project skill still reads as a skill.
    if source == "skill" {
        return CommandScope::Skill;
    }
    // Orbit's bundled extensions load with `--extension`, which pi reports as
    // temporary; the install path is the only reliable tell.
    if let (Some(home), Some(path)) = (home, path) {
        if Path::new(path).starts_with(home.join(".orbit-pi")) {
            return CommandScope::Orbit;
        }
    }
    if scope == Some("project") {
        let name = workspace
            .and_then(|dir| dir.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| tr!("autocomplete.scope_project"));
        return CommandScope::Project(name);
    }
    // Installed packages (`npm:`/`git:`) and pi's own built-ins are part of
    // the setup — "builtin". A synthetic path (`<inline:…>`, `<sdk:…>`) is
    // built in too. Only a user-authored top-level file is "custom".
    if origin == Some("package")
        || source == "builtin"
        || path.is_some_and(|path| path.starts_with('<'))
    {
        return CommandScope::Builtin;
    }
    if scope == Some("user") {
        return CommandScope::Custom;
    }
    CommandScope::Builtin
}

/// One slash command from pi's `get_commands` (extensions + skills).
#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    /// Where this command came from — rendered as the row's scope badge.
    pub scope: CommandScope,
}

/// One selectable row of the autocomplete menu.
#[derive(Debug, Clone)]
pub enum AcEntry {
    /// A pi command — commit inserts `/name ` into the composer.
    Command {
        name: String,
        description: String,
        scope: CommandScope,
    },
    /// A workspace file — commit inserts `@path ` (pi expands @-mentions).
    File { path: String },
}

/// Filter the catalog by `query` (case-insensitive substring; prefix
/// matches on the title rank first). Returns at most `limit` entries.
/// Fuzzy subsequence score: `Some(score)` when every char of `needle`
/// appears in `haystack` in order (case handled by the caller). Rewards
/// consecutive runs and word-boundary hits, penalizes gaps — so `appx`
/// matches `src/App.tsx` and `main` matches `src/main.rs` better than
/// `crates/app/main.rs`.
fn fuzzy_match(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let haystack: Vec<char> = haystack.chars().collect();
    let mut score = 0i32;
    let mut search_start = 0usize;
    let mut prev_hit: Option<usize> = None;
    for c in needle.chars() {
        let found = haystack[search_start..]
            .iter()
            .position(|h| *h == c)
            .map(|p| p + search_start)?;
        if prev_hit == Some(found.wrapping_sub(1)) {
            score += 3; // consecutive run
        }
        if found == 0 || haystack[found - 1] == '/' {
            score += 2; // start of the path or a segment
        }
        let gap = found - prev_hit.map(|p| p + 1).unwrap_or(0);
        score -= gap.min(12) as i32;
        prev_hit = Some(found);
        search_start = found + 1;
    }
    Some(score)
}

/// Filter the catalog by `query` with fuzzy (subsequence) matching —
/// basename matches outrank path matches, higher scores rank first, ties
/// keep catalog order. Returns at most `limit` entries.
pub fn filter_entries(
    query: &str,
    files: &[String],
    commands: &[SlashCommand],
    limit: usize,
) -> Vec<AcEntry> {
    let q = query.to_lowercase();
    let mut scored: Vec<(i32, usize, AcEntry)> = Vec::new();
    for (ix, command) in commands.iter().enumerate() {
        let name = command.name.to_lowercase();
        if let Some(score) = fuzzy_match(&q, &name) {
            scored.push((
                score + 10,
                ix,
                AcEntry::Command {
                    name: command.name.clone(),
                    description: command.description.clone(),
                    scope: command.scope.clone(),
                },
            ));
        }
    }
    for (ix, path) in files.iter().enumerate() {
        let basename = path.rsplit('/').next().unwrap_or(path).to_lowercase();
        // A basename hit (weighted 2×) outranks the same hit deep in the
        // path, so `app` surfaces `src/App.tsx` over `crates/app/…`.
        let score = fuzzy_match(&q, &basename)
            .map(|s| s * 2)
            .into_iter()
            .chain(fuzzy_match(&q, &path.to_lowercase()))
            .max();
        if let Some(score) = score {
            scored.push((
                score,
                ix + commands.len(),
                AcEntry::File { path: path.clone() },
            ));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, _, entry)| entry).collect()
}

/// Walk the workspace for @-mentionable files (relative, `/`-separated).
/// Skips VCS/dependency/build dirs and dot directories, caps depth and
/// count so huge trees stay responsive.
pub fn list_workspace_files(root: &Path) -> Vec<String> {
    const MAX_FILES: usize = 4_000;
    const MAX_DEPTH: usize = 12;
    const SKIP_DIRS: &[&str] = &[
        ".git",
        ".pi",
        ".hg",
        ".svn",
        "node_modules",
        "target",
        "dist",
        "build",
        "vendor",
        "__pycache__",
        ".next",
        ".cache",
    ];
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= MAX_FILES || depth > MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if !name.starts_with('.') && out.len() < MAX_FILES {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// Shared keyboard-negotiation state between the composer (which owns
/// ↑/↓) and the app (which owns Enter/Escape and renders the menu).
#[derive(Debug, Default)]
pub struct AutocompleteState {
    pub open: bool,
    pub highlighted: usize,
    pub count: usize,
}

impl AutocompleteState {
    /// Cycle the highlight, wrapping at both ends.
    pub fn move_highlight(&mut self, delta: i32) {
        if self.count == 0 {
            self.highlighted = 0;
            return;
        }
        let count = self.count as i32;
        self.highlighted = (self.highlighted as i32 + delta).rem_euclid(count) as usize;
    }
}

pub type SharedAutocomplete = Rc<std::cell::RefCell<AutocompleteState>>;

/// File-type badge for an @-mention row: a short extension label plus
/// language-brand colors tuned for dark and light surfaces (like VS Code's
/// colored file icons — `tsx`/`ts` blue, `jsx`/`js` yellow, `html` orange…).
/// Returns `(label, dark_hex, light_hex)`; unknown extensions fall back to
/// their (uppercased) extension or a generic gray file.
pub fn file_type_badge(path: &str) -> (&'static str, u32, u32) {
    // Lowercased extension (no dot). Filenames without extensions —
    // Dockerfile, Makefile, .env — get named badges below.
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    if ext == path.to_lowercase() {
        // No dot at all — treat as an extensionless file (Makefile-style).
        return ("FILE", 0x8A8A94, 0x6E6E78);
    }
    match ext.as_str() {
        // Docs
        "md" | "markdown" | "mdx" => ("MD", 0x519ABA, 0x3B72A8),
        "txt" => ("TXT", 0x8A8A94, 0x6E6E78),
        "pdf" => ("PDF", 0xD93831, 0xB02A24),
        // TypeScript family
        "ts" => ("TS", 0x3178C6, 0x2A66A8),
        "tsx" => ("TSX", 0x3178C6, 0x2A66A8),
        "d.ts" => ("TS", 0x3178C6, 0x2A66A8),
        // JavaScript family
        "js" | "mjs" | "cjs" => ("JS", 0xF1E05A, 0xA9941A),
        "jsx" => ("JSX", 0xF1E05A, 0xB29310),
        "json" | "jsonc" | "json5" => ("JSON", 0xCBCB41, 0x8F8F1E),
        // Web
        "html" | "htm" => ("HTML", 0xE44D26, 0xC43D1B),
        "css" => ("CSS", 0x563D7C, 0x563D7C),
        "scss" | "sass" => ("SCSS", 0xC6538C, 0xA8447A),
        "less" => ("LESS", 0x2A5D8F, 0x2A5D8F),
        "vue" => ("VUE", 0x41B883, 0x35986C),
        "svelte" => ("SVELTE", 0xFF3E00, 0xD63400),
        "astro" => ("ASTRO", 0xFF5D01, 0xD64E00),
        // Systems / backend
        "rs" => ("RS", 0xDEA584, 0xB5744F),
        "py" => ("PY", 0x3572A5, 0x2A5B85),
        "go" => ("GO", 0x00ADD8, 0x008DB2),
        "rb" => ("RB", 0xCC342D, 0xA82A24),
        "php" => ("PHP", 0x4F5D95, 0x414C7C),
        "java" => ("JAVA", 0xB07219, 0x8E5B13),
        "kt" | "kts" => ("KT", 0xA97BFF, 0x7C4FD9),
        "swift" => ("SWIFT", 0xF05138, 0xC93F28),
        "c" | "h" => ("C", 0x8D8D9B, 0x62626E),
        "cpp" | "hpp" | "cc" | "cxx" => ("C++", 0xF34B7D, 0xD22A5E),
        "cs" => ("CS", 0x178600, 0x106B00),
        // Shell / config
        "sh" | "bash" | "zsh" | "fish" => ("SH", 0x89E051, 0x4E9424),
        "toml" => ("TOML", 0x9C4121, 0x7C331A),
        "yaml" | "yml" => ("YML", 0xCB171E, 0xA11218),
        "lock" => ("LOCK", 0x7D7D7D, 0x6E6E78),
        "env" => ("ENV", 0xECD53F, 0x8F8414),
        // Data / query
        "sql" => ("SQL", 0xDD7C37, 0xB85F20),
        "csv" | "tsv" => ("CSV", 0x237346, 0x1B5C38),
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => ("IMG", 0xA074C4, 0x7C4FA0),
        "svg" => ("SVG", 0xFFB13B, 0xD68F1F),
        // Known-but-unlisted extensions get a neutral badge; anything else
        // falls back to a generic file.
        "hbs" => ("HBS", 0x8A8A94, 0x6E6E78),
        "gitignore" | "gitattributes" => ("GIT", 0xF14E32, 0xD13E22),
        _ => ("FILE", 0x8A8A94, 0x6E6E78),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_scope_folds_pi_provenance() {
        let home = Path::new("/home/u");
        let workspace = Path::new("/work/orbit");
        let classify = |source, scope, origin, path| {
            classify_scope(source, scope, origin, path, Some(workspace), Some(home))
        };
        // Skills win over their scope, project or user.
        assert_eq!(
            classify("skill", Some("project"), None, None),
            CommandScope::Skill
        );
        assert_eq!(
            classify("skill", Some("user"), None, None),
            CommandScope::Skill
        );
        // Orbit's bundled extensions live under `~/.orbit-pi/` and arrive as
        // temporary CLI extensions.
        assert_eq!(
            classify(
                "extension",
                Some("temporary"),
                Some("top-level"),
                Some("/home/u/.orbit-pi/title-extension/index.js")
            ),
            CommandScope::Orbit
        );
        // Project commands badge with the workspace folder name.
        assert_eq!(
            classify(
                "extension",
                Some("project"),
                Some("top-level"),
                Some("/work/orbit/.pi/extensions/x.ts")
            ),
            CommandScope::Project("orbit".to_string())
        );
        // User-scoped, hand-written extensions/prompts read as custom; an
        // installed package of the same scope is built in.
        assert_eq!(
            classify(
                "extension",
                Some("user"),
                Some("top-level"),
                Some("/home/u/.pi/agent/extensions/x.ts")
            ),
            CommandScope::Custom
        );
        assert_eq!(
            classify(
                "extension",
                Some("user"),
                Some("package"),
                Some("/home/u/.pi/agent/npm/node_modules/x/index.ts")
            ),
            CommandScope::Builtin
        );
        // Anything synthetic or unplaceable falls back to builtin.
        assert_eq!(
            classify(
                "extension",
                Some("temporary"),
                None,
                Some("<inline:llama.cpp>")
            ),
            CommandScope::Builtin
        );
        assert_eq!(classify("", None, None, None), CommandScope::Builtin);
    }

    #[test]
    fn slash_triggers_only_at_start_before_whitespace() {
        let t = detect_trigger("/rev", 4).unwrap();
        assert_eq!(t.kind, TriggerKind::Slash);
        assert_eq!(t.query, "rev");
        assert_eq!(t.start, 0);
        assert_eq!(t.end, 4);
        // Query cut at the caret.
        assert_eq!(detect_trigger("/rev", 2).unwrap().query, "r");
        // Caret past the first word closes the trigger.
        assert!(detect_trigger("/rev ", 5).is_none());
        // `/` not at the start is plain text.
        assert!(detect_trigger("run /rev", 8).is_none());
        // Empty query right after the sigil still triggers.
        assert_eq!(detect_trigger("/", 1).unwrap().query, "");
        // Trigger survives command names with trailing content later on.
        assert!(detect_trigger("/review focus on app.rs", 2).is_some());
    }

    #[test]
    fn at_trigger_runs_from_token_start_to_caret() {
        let t = detect_trigger("look at @src/comp", 17).unwrap();
        assert_eq!(t.kind, TriggerKind::At);
        assert_eq!(t.start, 8);
        assert_eq!(t.query, "src/comp");
        // Whitespace between @ and caret closes it.
        assert!(detect_trigger("a b @c d", 8).is_none());
        // Mid-word @ does not trigger ("b@" preceded by "c").
        assert!(detect_trigger("abc@x", 5).is_none());
        // @ at the very start triggers.
        assert_eq!(detect_trigger("@", 1).unwrap().query, "");
        // A second @ supersedes the first.
        let t = detect_trigger("@one @tw", 8).unwrap();
        assert_eq!(t.start, 5);
        assert_eq!(t.query, "tw");
        // Newline also starts a token (multi-line composer).
        let t = detect_trigger("done\n@re", 8).unwrap();
        assert_eq!(t.query, "re");
    }

    #[test]
    fn tokenize_colors_leading_command_and_files() {
        let spans = tokenize_mentions("/review fix @src/main.rs and @lib.rs");
        assert_eq!(
            spans,
            vec![
                MentionSpan {
                    range: 0..7,
                    kind: MentionKind::Command,
                },
                MentionSpan {
                    range: 12..24,
                    kind: MentionKind::File,
                },
                MentionSpan {
                    range: 29..36,
                    kind: MentionKind::File,
                },
            ]
        );
    }

    #[test]
    fn tokenize_leaves_paths_and_emails_plain() {
        // A leading path is not a command; a mid-word `@` is not a mention.
        assert!(tokenize_mentions("/usr/local/bin").is_empty());
        assert!(tokenize_mentions("mail me at a@b.com").is_empty());
        // A lone sigil has no payload yet.
        assert!(tokenize_mentions("/").is_empty());
        assert!(tokenize_mentions("@").is_empty());
    }

    #[test]
    fn tokenize_handles_multiline_and_repeats() {
        let spans = tokenize_mentions("done\n@re @two/three");
        assert_eq!(
            spans.iter().map(|s| s.range.clone()).collect::<Vec<_>>(),
            vec![5..8, 9..19]
        );
        // A `@` inside an existing mention does not open a second span.
        assert_eq!(tokenize_mentions("@a@b").len(), 1);
    }

    #[test]
    fn filter_ranks_prefix_matches_first() {
        let files = vec![
            "crates/app/main.rs".to_string(),
            "src/main.rs".to_string(),
            "README.md".to_string(),
            "main.py".to_string(),
        ];
        let entries = filter_entries("main", &files, &[], 10);
        // Basename prefix matches first (stable by catalog order), then
        // path-contains matches.
        let title = |e: &AcEntry| match e {
            AcEntry::Command { name, .. } => name.clone(),
            AcEntry::File { path } => path.clone(),
        };
        assert_eq!(
            entries.iter().map(title).collect::<Vec<_>>(),
            // All three basenames start with "main" — rank ties break by
            // catalog order. "README.md" (contains-match) is excluded.
            vec![
                "crates/app/main.rs".to_string(),
                "src/main.rs".to_string(),
                "main.py".to_string()
            ]
        );
    }

    #[test]
    fn filter_is_fuzzy_and_basename_first() {
        // Subsequence match: no exact substring of "appx" appears in the
        // path, yet fuzzy finds a-p-p-x in order.
        let files = vec!["src/App.tsx".to_string()];
        assert_eq!(filter_entries("appx", &files, &[], 10).len(), 1);
        // Basename hits outrank path hits: `app` ranks src/App.tsx (basename)
        // above crates/app/main.rs (path-only hit).
        let files = vec!["crates/app/main.rs".to_string(), "src/App.tsx".to_string()];
        assert_eq!(
            filter_entries("app", &files, &[], 10)
                .first()
                .map(|e| match e {
                    AcEntry::File { path } => path.as_str(),
                    _ => "",
                }),
            Some("src/App.tsx")
        );
        // No subsequence → no match ("zzq" can't be spelled from these).
        assert!(filter_entries("zzq", &files, &[], 10).is_empty());
    }

    #[test]
    fn filter_is_case_insensitive_and_capped() {
        let files = vec!["Big/FILE.Rs".to_string()];
        assert_eq!(filter_entries("file", &files, &[], 10).len(), 1);
        assert!(filter_entries("zzz", &files, &[], 10).is_empty());
        let many: Vec<String> = (0..50).map(|i| format!("f{i}.rs")).collect();
        assert_eq!(filter_entries("", &many, &[], 8).len(), 8);
    }

    #[test]
    fn workspace_walk_skips_ignorable_dirs() {
        let root = std::env::temp_dir().join(format!("orbit-mentions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join(".git/refs")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(root.join(".env"), "").unwrap();
        std::fs::write(root.join("node_modules/pkg/i.js"), "").unwrap();
        std::fs::write(root.join(".git/refs/x"), "").unwrap();
        let files = list_workspace_files(&root);
        assert_eq!(files, vec!["src/lib.rs".to_string()]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn highlight_wraps_both_ways() {
        let mut s = AutocompleteState {
            open: true,
            highlighted: 0,
            count: 3,
        };
        s.move_highlight(-1);
        assert_eq!(s.highlighted, 2);
        s.move_highlight(1);
        assert_eq!(s.highlighted, 0);
        s.move_highlight(5);
        assert_eq!(s.highlighted, 2);
        s.count = 0;
        s.move_highlight(1);
        assert_eq!(s.highlighted, 0);
    }

    #[test]
    fn file_badges_map_by_extension() {
        // Requested types: md, jsx, html, ts, tsx — each gets its own badge.
        for (path, want) in [
            ("README.md", "MD"),
            ("src/App.jsx", "JSX"),
            ("index.html", "HTML"),
            ("src/lib.ts", "TS"),
            ("src/App.tsx", "TSX"),
            ("app.rs", "RS"),
            ("main.py", "PY"),
            ("styles.scss", "SCSS"),
            ("logo.png", "IMG"),
            ("Cargo.toml", "TOML"),
        ] {
            let (label, _, _) = file_type_badge(path);
            assert_eq!(label, want, "badge for {path}");
        }
        // Same-family extensions share a color (TS/TSX blue, JS/JSX yellow).
        let (_, ts_dark, ts_light) = file_type_badge("a.ts");
        let (_, tsx_dark, tsx_light) = file_type_badge("b.tsx");
        assert_eq!(ts_dark, tsx_dark);
        assert_eq!(ts_light, tsx_light);
        // Case-insensitive; extensionless files get the generic badge.
        assert_eq!(file_type_badge("NOTES.MD").0, "MD");
        assert_eq!(file_type_badge("Makefile").0, "FILE");
        // Dark and light palettes both carry a color.
        let (_, dark, light) = file_type_badge("x.html");
        assert_ne!(dark, 0);
        assert_ne!(light, 0);
    }
}
