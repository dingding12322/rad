//! Git worktrees as a first-class Orbit workspace concept.
//!
//! A Git worktree is a second working directory backed by the same repository.
//! Orbit models one as a [`Worktree`] and drives its lifecycle through a
//! [`WorktreeManager`]: discovery (`git worktree list --porcelain -z`),
//! creation from an existing branch or a new one, removal, moving, locking,
//! pruning, and repair. Everything goes through the `git worktree` porcelain —
//! nothing here writes `.git/worktrees` or `.git/config` directly.
//!
//! Three identities stay strictly separate:
//!
//! ```text
//! name   113
//! branch feature/issue-113
//! path   ~/Projects/my-project/.wt/113
//! ```
//!
//! The name is a user-chosen identifier, never derived from the branch. The
//! path is always constructed as `<worktree root>/<name>`, where the root
//! defaults to `.wt` under the repository and is configurable globally
//! (`~/.orbit-pi/worktrees.json`) and per repository
//! (`.orbit/worktree.json`).
//!
//! This module is pure Git I/O: no GPUI, no session state. UI code consumes it
//! through `app/worktrees.rs`, and a worktree becomes an ordinary Orbit
//! workspace by opening its path.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};

/// Default worktree directory, relative to the repository root.
pub const DEFAULT_DIRECTORY: &str = ".wt";
/// Default setup script, relative to the repository root.
pub const DEFAULT_SETUP_SCRIPT: &str = ".orbit/worktree-setup.sh";
/// Repository-local configuration path, relative to the repository root.
pub const REPO_CONFIG_PATH: &str = ".orbit/worktree.json";

/// A worktree failure a UI can act on. Raw Git output is preserved in
/// [`WorktreeError::Git`] for the detail view; the other variants name the
/// condition so the UI can offer the right next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeError {
    /// The path does not sit inside a Git work tree.
    NotARepository,
    /// The name is not a single, safe path component.
    InvalidName(String),
    /// The branch name is not a valid Git ref.
    InvalidBranch(String),
    /// Something already exists at the target path.
    PathExists(PathBuf),
    /// The branch is checked out by another worktree (Git allows one at a
    /// time). The UI offers to open that worktree instead.
    BranchCheckedOut { branch: String, path: PathBuf },
    /// The worktree is locked.
    Locked(PathBuf),
    /// The worktree has uncommitted changes; removal needs `--force`.
    Dirty(PathBuf),
    /// The path is not a registered worktree of this repository.
    NotAWorktree(PathBuf),
    /// Anything else Git reported, verbatim.
    Git(String),
}

impl WorktreeError {
    /// Wrap a failed Git invocation, preferring stderr and falling back to
    /// the exit status when Git was silent.
    fn git(output: &Output) -> Self {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Self::Git(tr!("git.exited_with", status = output.status.to_string()))
        } else {
            Self::Git(stderr)
        }
    }

    /// Render a path for human-readable error text.
    fn format_path(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotARepository => write!(f, "{}", tr!("worktree.error.not_a_repository")),
            Self::InvalidName(name) => {
                write!(f, "{}", tr!("worktree.error.invalid_name", name = name))
            }
            Self::InvalidBranch(branch) => {
                write!(
                    f,
                    "{}",
                    tr!("worktree.error.invalid_branch", branch = branch)
                )
            }
            Self::PathExists(path) => write!(
                f,
                "{}",
                tr!("worktree.error.path_exists", path = Self::format_path(path))
            ),
            Self::BranchCheckedOut { branch, path } => write!(
                f,
                "{}",
                tr!(
                    "worktree.error.branch_checked_out",
                    branch = branch,
                    path = Self::format_path(path)
                )
            ),
            Self::Locked(path) => write!(
                f,
                "{}",
                tr!("worktree.error.locked", path = Self::format_path(path))
            ),
            Self::Dirty(path) => write!(
                f,
                "{}",
                tr!("worktree.error.dirty", path = Self::format_path(path))
            ),
            Self::NotAWorktree(path) => write!(
                f,
                "{}",
                tr!(
                    "worktree.error.not_a_worktree",
                    path = Self::format_path(path)
                )
            ),
            Self::Git(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for WorktreeError {}

/// One worktree as Git reports it. See the module docs for the identity
/// rules; `name` is Orbit's identifier and never the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute path Git reported (the directory a workspace would open).
    pub path: PathBuf,
    /// Orbit-level identifier: the component under the worktree root for
    /// linked worktrees, otherwise the directory name. Never contains a
    /// path separator.
    pub name: String,
    /// Full commit id at HEAD.
    pub head: String,
    /// Short branch name (`feature/issue-113`), when on a branch.
    pub branch: Option<String>,
    /// HEAD is not on a branch.
    pub detached: bool,
    /// The main worktree (the repository itself — never removable).
    pub is_main: bool,
    /// A bare repository entry (`git worktree list` on a bare repo).
    pub bare: bool,
    /// Locked via `git worktree lock`; pruning and removal are refused.
    pub locked: bool,
    /// Lock reason, when one was recorded.
    pub lock_reason: Option<String>,
    /// Git considers the administrative files stale.
    pub prunable: bool,
    /// Prune reason Git reported.
    pub prune_reason: Option<String>,
}

impl Worktree {
    /// Short label for lists: a detached worktree reads `(detached)`.
    pub fn branch_label(&self) -> String {
        self.branch.clone().unwrap_or_else(|| {
            if self.bare {
                tr!("worktree.bare")
            } else {
                tr!("worktree.detached")
            }
        })
    }
}

/// Global worktree preferences, persisted to `~/.orbit-pi/worktrees.json`.
/// Repository-local `.orbit/worktree.json` overrides `directory` and
/// `setup_script`; the booleans stay global.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeConfig {
    /// Where new worktrees are created: relative to the repository root or
    /// absolute (`~` expands to the home directory).
    pub directory: String,
    /// Setup script path: relative to the repository root or absolute.
    pub setup_script: String,
    /// Run the setup script automatically after creation. When off, the
    /// create dialog still offers it per worktree.
    pub run_setup_auto: bool,
    /// Open a newly created worktree as the active workspace.
    pub open_after_create: bool,
    /// Repositories whose setup script the user explicitly allowed. Scripts
    /// execute arbitrary commands, so first use in a repository is an
    /// explicit confirmation (see `app/worktrees.rs`).
    pub allowed_setup_repos: Vec<String>,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            directory: DEFAULT_DIRECTORY.to_string(),
            setup_script: DEFAULT_SETUP_SCRIPT.to_string(),
            run_setup_auto: true,
            open_after_create: true,
            allowed_setup_repos: Vec::new(),
        }
    }
}

impl WorktreeConfig {
    /// `~/.orbit-pi/worktrees.json`.
    pub fn store_path() -> PathBuf {
        crate::platform::home_dir()
            .join(".orbit-pi")
            .join("worktrees.json")
    }

    /// Load the persisted settings, falling back to defaults on any error
    /// (a missing or malformed file must never block the feature).
    pub fn load() -> Self {
        let path = Self::store_path();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// Persist the settings, creating `~/.orbit-pi/` when needed.
    pub fn persist(&self) -> std::io::Result<()> {
        let path = Self::store_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload = serde_json::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(path, payload)
    }

    /// The effective worktree directory for `repo_root`: the repository's
    /// `.orbit/worktree.json` wins over the global default.
    pub fn effective_directory(&self, repo_root: &Path) -> PathBuf {
        let repo = WorktreeRepoConfig::load(repo_root);
        let raw = repo.directory.as_deref().unwrap_or(&self.directory);
        expand_path(repo_root, raw)
    }

    /// The effective setup script for `repo_root`, when one exists on disk.
    pub fn effective_setup_script(&self, repo_root: &Path) -> Option<PathBuf> {
        let repo = WorktreeRepoConfig::load(repo_root);
        let raw = repo.setup_script.as_deref().unwrap_or(&self.setup_script);
        let path = expand_path(repo_root, raw);
        path.is_file().then_some(path)
    }

    /// Whether the user has allowed running this repository's setup script.
    pub fn setup_allowed(&self, repo_root: &Path) -> bool {
        let key = setup_repo_key(repo_root);
        self.allowed_setup_repos.iter().any(|entry| entry == &key)
    }

    /// Record that the user allowed this repository's setup script.
    pub fn allow_setup(&mut self, repo_root: &Path) {
        let key = setup_repo_key(repo_root);
        if !self.allowed_setup_repos.iter().any(|entry| entry == &key) {
            self.allowed_setup_repos.push(key);
        }
    }
}

/// Canonical key for the allowed-setup list. Uses the repository root so a
/// symlinked path and its real path share one entry.
fn setup_repo_key(repo_root: &Path) -> String {
    canonical_or(repo_root).to_string_lossy().into_owned()
}

/// Repository-local worktree configuration (`.orbit/worktree.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeRepoConfig {
    /// Overrides the global default location.
    pub directory: Option<String>,
    /// Overrides the global setup script.
    pub setup_script: Option<String>,
}

impl WorktreeRepoConfig {
    /// `.orbit/worktree.json` under `repo_root`.
    pub fn path(repo_root: &Path) -> PathBuf {
        repo_root.join(REPO_CONFIG_PATH)
    }

    /// Load the repository configuration; a missing or malformed file means
    /// "no overrides", never an error the UI has to handle.
    pub fn load(repo_root: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(Self::path(repo_root)) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }
}

/// Resolve a configured path: `~` expands to the home directory, an absolute
/// path stays as-is, anything else is relative to `base` (the repository).
pub fn expand_path(base: &Path, raw: &str) -> PathBuf {
    let raw = raw.trim();
    if raw == "~" {
        return crate::platform::home_dir();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return crate::platform::home_dir().join(rest);
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Whether `name` is a legal worktree identifier: exactly one normal path
/// component. Rejects empty names, `.`/`..`, separators, and — on Windows —
/// the characters and reserved device names a directory cannot take.
pub fn validate_name(name: &str) -> Result<&str, WorktreeError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    let mut components = Path::new(name).components();
    let Some(Component::Normal(_)) = components.next() else {
        return Err(WorktreeError::InvalidName(name.to_string()));
    };
    if components.next().is_some() {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    // `Component::Normal` does not reject trailing separators on every
    // platform, and a name must never contain one.
    if name.contains('/') || name.contains('\\') {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    if name.contains('\0') {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    if cfg!(windows) {
        if name.contains(['<', '>', ':', '"', '|', '?', '*']) {
            return Err(WorktreeError::InvalidName(name.to_string()));
        }
        if name.ends_with('.') || name.ends_with(' ') {
            return Err(WorktreeError::InvalidName(name.to_string()));
        }
        let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
        const RESERVED: [&str; 22] = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        if RESERVED.contains(&stem.as_str()) {
            return Err(WorktreeError::InvalidName(name.to_string()));
        }
    }
    Ok(name)
}

/// Slugify arbitrary text into a safe worktree name / branch segment: ASCII
/// alphanumerics, `.`, and `_` survive (lowercased), every other run becomes a
/// single `-`, and leading/trailing separators are trimmed. Empty input, or
/// input with nothing usable, yields an empty string so callers can fall back.
///
/// Used only for *defaults* — a branch suggestion derived from a worktree
/// name. It never rewrites an explicit user choice.
pub fn slugify(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out.trim_matches('-').trim_matches('.').to_string()
}

/// Resolve a candidate path against `.`/`..` components without touching the
/// filesystem, so containment can be checked before the path exists.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Canonicalize when possible, falling back to the deepest existing
/// ancestor's canonical path plus the remaining components (so a target
/// directory that does not exist yet still compares equal across symlinks),
/// then to the lexical path.
fn canonical_or(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let normalized = normalize_lexically(path);
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current: &Path = &normalized;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(canonical) = parent.canonicalize() {
            let mut out = canonical;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        if parent.as_os_str().is_empty() {
            break;
        }
        current = parent;
    }
    normalized
}

/// Whether two paths refer to the same location, tolerating symlinks
/// (`/tmp` vs `/private/tmp` on macOS) and `.`/`..`.
pub fn same_path(a: &Path, b: &Path) -> bool {
    canonical_or(a) == canonical_or(b)
}

/// Whether `candidate` is inside `root`, tolerating macOS's `/var` vs
/// `/private/var` symlink split when the target directory does not exist yet.
fn is_within(candidate: &Path, root: &Path) -> bool {
    let candidate = normalize_lexically(candidate);
    if candidate.starts_with(root) {
        return true;
    }
    let root = canonical_or(root);
    let Some(parent) = candidate.parent() else {
        return false;
    };
    canonical_or(parent).starts_with(&root)
}

/// The `git` invoker for `cwd`, reusing [`crate::git::command`] so
/// environment handling (`GIT_OPTIONAL_LOCKS=0`), console hiding, and the
/// Git executable match every other Orbit Git call.
fn command(cwd: &Path) -> Command {
    crate::git::command(cwd)
}

/// Run `git` in `cwd`, returning raw stdout or a mapped error.
fn run(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, WorktreeError> {
    run_with(cwd, args, &[])
}

/// Run `git` with extra environment, returning raw stdout.
fn run_with(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<Vec<u8>, WorktreeError> {
    let mut cmd = command(cwd);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd
        .args(args)
        .output()
        .map_err(|err| WorktreeError::Git(tr!("git.not_available", error = err)))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(WorktreeError::git(&output))
    }
}

/// Run `git`, returning trimmed UTF-8 stdout.
fn run_text(cwd: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let bytes = run(cwd, args)?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Parse `git worktree list --porcelain -z` output.
///
/// In `-z` mode every field is NUL-terminated and each record ends with an
/// extra NUL, so the split stream contains an empty token between records.
/// `worktree_root` is the configured worktree directory, used to name linked
/// worktrees after their component under it; `None` falls back to the
/// directory name (useful for parser tests).
pub fn parse_worktree_list(raw: &str, worktree_root: Option<&Path>) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    let mut current: Option<PartialWorktree> = None;
    for field in raw.split('\0') {
        if field.is_empty() {
            // An empty token closes the current record.
            if let Some(partial) = current.take() {
                out.push(partial.finish(worktree_root));
            }
            continue;
        }
        if field.starts_with("worktree ") {
            // A new path opens a record; a malformed input without the
            // closing empty token still yields the parsed worktree.
            if let Some(partial) = current.take() {
                out.push(partial.finish(worktree_root));
            }
            current = Some(PartialWorktree::new(field));
            continue;
        }
        if let Some(partial) = current.as_mut() {
            partial.consume(field);
        }
    }
    if let Some(partial) = current {
        out.push(partial.finish(worktree_root));
    }
    for (index, worktree) in out.iter_mut().enumerate() {
        // `git worktree list` always reports the main worktree first.
        worktree.is_main = index == 0;
    }
    out
}

/// In-progress record while parsing porcelain output.
struct PartialWorktree {
    path: PathBuf,
    head: String,
    branch: Option<String>,
    detached: bool,
    bare: bool,
    locked: bool,
    lock_reason: Option<String>,
    prunable: bool,
    prune_reason: Option<String>,
}

impl PartialWorktree {
    fn new(field: &str) -> Self {
        Self {
            path: PathBuf::from(field.strip_prefix("worktree ").unwrap_or_default()),
            head: String::new(),
            branch: None,
            detached: false,
            bare: false,
            locked: false,
            lock_reason: None,
            prunable: false,
            prune_reason: None,
        }
    }

    fn consume(&mut self, field: &str) {
        if let Some(head) = field.strip_prefix("HEAD ") {
            self.head = head.to_string();
        } else if let Some(reference) = field.strip_prefix("branch ") {
            // `branch refs/heads/<name>` — store the short name.
            self.branch = Some(
                reference
                    .strip_prefix("refs/heads/")
                    .unwrap_or(reference)
                    .to_string(),
            );
        } else if field == "detached" {
            self.detached = true;
        } else if field == "bare" {
            self.bare = true;
        } else if field == "locked" {
            self.locked = true;
        } else if let Some(reason) = field.strip_prefix("locked ") {
            self.locked = true;
            self.lock_reason = Some(reason.to_string());
        } else if field == "prunable" {
            self.prunable = true;
        } else if let Some(reason) = field.strip_prefix("prunable ") {
            self.prunable = true;
            self.prune_reason = Some(reason.to_string());
        }
    }

    fn finish(self, worktree_root: Option<&Path>) -> Worktree {
        Worktree {
            name: name_for(&self.path, worktree_root),
            path: self.path,
            head: self.head,
            branch: self.branch,
            detached: self.detached,
            is_main: false,
            bare: self.bare,
            locked: self.locked,
            lock_reason: self.lock_reason,
            prunable: self.prunable,
            prune_reason: self.prune_reason,
        }
    }
}

/// Orbit's name for a worktree path: the component directly under the
/// configured worktree root, else the directory name.
fn name_for(path: &Path, worktree_root: Option<&Path>) -> String {
    if let Some(root) = worktree_root {
        if let Ok(relative) = path.strip_prefix(root) {
            let mut components = relative.components();
            if let (Some(Component::Normal(name)), None) = (components.next(), components.next()) {
                return name.to_string_lossy().into_owned();
            }
        }
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Lifecycle owner for one repository's worktrees. Stateless beyond the
/// repository path and the global configuration, so concurrent UI work can
/// hold/rebuild it freely; callers serialize mutations ([`crate::app`]
/// guards one at a time per repository).
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    repo: PathBuf,
    /// Cached main worktree root (see [`Self::with_root`]); resolved through
    /// Git on first use when absent.
    root: Option<PathBuf>,
    config: WorktreeConfig,
}

impl WorktreeManager {
    /// A manager rooted at `repo` (the repository, the main worktree, or any
    /// linked worktree) using default configuration.
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            root: None,
            config: WorktreeConfig::default(),
        }
    }

    /// Supply the main worktree root already resolved by an earlier
    /// [`Self::list`] call, so UI-side path resolution never runs Git during
    /// render.
    pub fn with_root(mut self, root: PathBuf) -> Self {
        self.root = Some(root);
        self
    }

    /// Use explicit settings instead of the built-in defaults. The UI passes
    /// the loaded `~/.orbit-pi/worktrees.json`.
    pub fn with_config(mut self, config: WorktreeConfig) -> Self {
        self.config = config;
        self
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    pub fn config(&self) -> &WorktreeConfig {
        &self.config
    }

    /// Whether the repo argument is inside a Git work tree at all.
    pub fn is_repository(&self) -> bool {
        crate::git::is_repo(&self.repo)
    }

    /// The main worktree root. `git worktree list` reports it first, which
    /// also resolves the correct root when the manager is opened from a
    /// linked worktree.
    pub fn repository_root(&self) -> Result<PathBuf, WorktreeError> {
        if let Some(root) = &self.root {
            return Ok(root.clone());
        }
        // A directory that is not a work tree gets the actionable
        // `NotARepository` error instead of Git's stderr; a bare repository
        // has no work tree but does have worktree metadata, so it passes.
        if !self.is_repository() {
            let bare = run_text(&self.repo, &["rev-parse", "--is-bare-repository"])
                .map(|value| value == "true")
                .unwrap_or(false);
            if !bare {
                return Err(WorktreeError::NotARepository);
            }
        }
        let raw = match run(&self.repo, &["worktree", "list", "--porcelain", "-z"]) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => run_text(&self.repo, &["worktree", "list", "--porcelain"])?,
        };
        parse_worktree_list(&raw, None)
            .into_iter()
            .next()
            .map(|worktree| worktree.path)
            .ok_or(WorktreeError::NotARepository)
    }

    /// The configured directory that holds this repository's worktrees
    /// (`.wt` by default). Relative values resolve against the main root.
    pub fn worktree_root(&self) -> Result<PathBuf, WorktreeError> {
        let root = self.repository_root()?;
        Ok(normalize_lexically(&self.config.effective_directory(&root)))
    }

    /// Like [`Self::worktree_root`], but creates the directory when it is
    /// missing, so `git worktree add` never fails on a fresh repository.
    pub fn ensure_worktree_root(&self) -> Result<PathBuf, WorktreeError> {
        let root = self.worktree_root()?;
        std::fs::create_dir_all(&root).map_err(|err| {
            WorktreeError::Git(format!("failed to create {}: {err}", root.display()))
        })?;
        Ok(root)
    }

    /// Validate `name` and resolve it to `<worktree root>/<name>`. The result
    /// is guaranteed to stay inside the configured root.
    pub fn resolve_path(&self, name: &str) -> Result<PathBuf, WorktreeError> {
        let name = validate_name(name)?;
        let root = self.worktree_root()?;
        let path = normalize_lexically(&root.join(name));
        if !is_within(&path, &root) {
            return Err(WorktreeError::InvalidName(name.to_string()));
        }
        Ok(path)
    }

    /// All worktrees Git reports, main first. Works for worktrees created
    /// outside Orbit — discovery is Git's own list.
    pub fn list(&self) -> Result<Vec<Worktree>, WorktreeError> {
        let root = self.repository_root()?;
        let worktree_root = normalize_lexically(&self.config.effective_directory(&root));
        match run(&self.repo, &["worktree", "list", "--porcelain", "-z"]) {
            Ok(bytes) => Ok(parse_worktree_list(
                &String::from_utf8_lossy(&bytes),
                Some(&worktree_root),
            )),
            Err(WorktreeError::Git(_)) => {
                // Older Git builds without `-z` still parse: newline fields,
                // blank line between records. Reuse the same record parser
                // by translating to NULs.
                let text = run_text(&self.repo, &["worktree", "list", "--porcelain"])?;
                let mut normalized = String::new();
                for line in text.lines() {
                    normalized.push_str(line);
                    normalized.push('\0');
                    if line.is_empty() {
                        normalized.push('\0');
                    }
                }
                Ok(parse_worktree_list(&normalized, Some(&worktree_root)))
            }
            Err(err) => Err(err),
        }
    }

    /// Find the worktree currently holding `branch`.
    pub fn find_by_branch(&self, branch: &str) -> Result<Option<Worktree>, WorktreeError> {
        Ok(self
            .list()?
            .into_iter()
            .find(|worktree| worktree.branch.as_deref() == Some(branch)))
    }

    /// Find the worktree whose path is `path`.
    pub fn find_by_path(&self, path: &Path) -> Result<Option<Worktree>, WorktreeError> {
        Ok(self
            .list()?
            .into_iter()
            .find(|worktree| same_path(&worktree.path, path)))
    }

    /// Whether `path` has staged, unstaged, or untracked changes. Used to ask
    /// before a forced removal; never mutates anything.
    pub fn is_dirty(&self, path: &Path) -> Result<bool, WorktreeError> {
        let output = run_text(path, &["status", "--porcelain"])?;
        Ok(!output.is_empty())
    }

    /// Validation shared by the create paths: repository, name, path
    /// containment, target absence, and branch availability.
    fn check_create_target(
        &self,
        name: &str,
        path: &Path,
        branch: Option<&str>,
    ) -> Result<(), WorktreeError> {
        if !self.is_repository() {
            return Err(WorktreeError::NotARepository);
        }
        let root = self.ensure_worktree_root()?;
        let path = normalize_lexically(path);
        if !is_within(&path, &root) {
            return Err(WorktreeError::InvalidName(name.to_string()));
        }
        if path.exists() {
            return Err(WorktreeError::PathExists(path));
        }
        if let Some(branch) = branch {
            if let Some(existing) = self.find_by_branch(branch)? {
                return Err(WorktreeError::BranchCheckedOut {
                    branch: branch.to_string(),
                    path: existing.path,
                });
            }
        }
        Ok(())
    }

    /// Return the worktree matching `path` after a successful mutation.
    fn resolve_after(&self, path: &Path) -> Result<Worktree, WorktreeError> {
        self.find_by_path(path)?
            .ok_or_else(|| WorktreeError::NotAWorktree(path.to_path_buf()))
    }

    /// Create a worktree for an existing branch:
    /// `git worktree add <path> <branch>`. The branch is not created or
    /// renamed; if it is already checked out elsewhere, the error carries
    /// that worktree's path.
    pub fn create(&self, name: &str, path: &Path, branch: &str) -> Result<Worktree, WorktreeError> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Err(WorktreeError::InvalidBranch(branch.to_string()));
        }
        self.check_create_target(name, path, Some(branch))?;
        let path = normalize_lexically(path);
        let path_str = path.to_string_lossy().into_owned();
        run(&self.repo, &["worktree", "add", &path_str, "--", branch])?;
        self.resolve_after(&path)
    }

    /// Alias for [`Self::create`], named by the create dialog's intent.
    pub fn create_from_existing_branch(
        &self,
        name: &str,
        path: &Path,
        branch: &str,
    ) -> Result<Worktree, WorktreeError> {
        self.create(name, path, branch)
    }

    /// Create a worktree and a new branch from `start_point` (HEAD when
    /// `None`): `git worktree add -b <branch> <path> [<start>]`.
    pub fn create_new_branch(
        &self,
        name: &str,
        path: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<Worktree, WorktreeError> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Err(WorktreeError::InvalidBranch(branch.to_string()));
        }
        // Git's own ref validator, so Orbit and Git cannot disagree about
        // what a valid branch name is.
        if run_text(&self.repo, &["check-ref-format", "--branch", branch]).is_err() {
            return Err(WorktreeError::InvalidBranch(branch.to_string()));
        }
        // A brand-new branch cannot be checked out elsewhere yet; `None`
        // skips that pre-check.
        self.check_create_target(name, path, None)?;
        let path = normalize_lexically(path);
        let path_str = path.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "add", "-b", branch, &path_str];
        let start = start_point.map(str::trim).filter(|point| !point.is_empty());
        if let Some(start) = start {
            args.push("--");
            args.push(start);
        }
        run(&self.repo, &args)?;
        self.resolve_after(&path)
    }

    /// Remove a worktree with `git worktree remove`. Refuses a locked or
    /// dirty worktree unless `force` is set, so a caller must have asked the
    /// user first. The branch itself is never deleted.
    pub fn remove(&self, path: &Path, force: bool) -> Result<(), WorktreeError> {
        let Some(worktree) = self.find_by_path(path)? else {
            return Err(WorktreeError::NotAWorktree(path.to_path_buf()));
        };
        if worktree.is_main {
            return Err(WorktreeError::NotAWorktree(path.to_path_buf()));
        }
        if worktree.locked {
            return Err(WorktreeError::Locked(worktree.path));
        }
        if !force && worktree.path.exists() && self.is_dirty(&worktree.path)? {
            return Err(WorktreeError::Dirty(worktree.path));
        }
        let path_str = worktree.path.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path_str);
        run(&self.repo, &args)?;
        Ok(())
    }

    /// Relocate a worktree with `git worktree move`. The branch and the
    /// worktree name in Git are untouched; only the directory moves. Orbit's
    /// display name follows the new directory under the root.
    pub fn move_to(&self, path: &Path, new_path: &Path) -> Result<(), WorktreeError> {
        let Some(worktree) = self.find_by_path(path)? else {
            return Err(WorktreeError::NotAWorktree(path.to_path_buf()));
        };
        let from = worktree.path.to_string_lossy().into_owned();
        let to = normalize_lexically(new_path).to_string_lossy().into_owned();
        run(&self.repo, &["worktree", "move", &from, &to])?;
        Ok(())
    }

    /// Lock a worktree so pruning and removal refuse to touch it:
    /// `git worktree lock [--reason <reason>] <path>`.
    pub fn lock(&self, path: &Path, reason: Option<&str>) -> Result<(), WorktreeError> {
        let Some(worktree) = self.find_by_path(path)? else {
            return Err(WorktreeError::NotAWorktree(path.to_path_buf()));
        };
        if worktree.locked {
            return Ok(());
        }
        let path_str = worktree.path.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "lock"];
        let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());
        if let Some(reason) = reason {
            args.push("--reason");
            args.push(reason);
        }
        args.push(&path_str);
        run(&self.repo, &args)?;
        Ok(())
    }

    /// Unlock a worktree: `git worktree unlock <path>`.
    pub fn unlock(&self, path: &Path) -> Result<(), WorktreeError> {
        let Some(worktree) = self.find_by_path(path)? else {
            return Err(WorktreeError::NotAWorktree(path.to_path_buf()));
        };
        if !worktree.locked {
            return Ok(());
        }
        let path_str = worktree.path.to_string_lossy().into_owned();
        run(&self.repo, &["worktree", "unlock", &path_str])?;
        Ok(())
    }

    /// Remove stale administrative entries for worktrees whose directories
    /// are gone: `git worktree prune`.
    pub fn prune(&self) -> Result<(), WorktreeError> {
        run(&self.repo, &["worktree", "prune"])?;
        Ok(())
    }

    /// Repair worktree administrative files after a manual move:
    /// `git worktree repair`.
    pub fn repair(&self) -> Result<(), WorktreeError> {
        run(&self.repo, &["worktree", "repair"])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A self-cleaning temporary directory (no tempfile dependency).
    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "orbit-worktree-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create temp repo dir");
            Self { path }
        }

        /// `git` in the repository, panicking on failure.
        fn git(&self, args: &[&str]) -> String {
            let output = StdCommand::new("git")
                .current_dir(&self.path)
                .args(args)
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        /// Initialize a repository with one commit on `main`.
        fn init(&self) {
            self.git(&["init", "-q", "-b", "main", "."]);
            self.git(&["config", "user.email", "test@orbit.dev"]);
            self.git(&["config", "user.name", "Orbit Test"]);
            std::fs::write(self.path.join("README.md"), "hello\n").expect("write file");
            self.git(&["add", "."]);
            self.git(&["commit", "-qm", "initial"]);
        }

        fn manager(&self) -> WorktreeManager {
            WorktreeManager::new(&self.path)
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Worktree tests need a real `git`. Skip (rather than fail) when it is
    /// unavailable so the suite stays green in minimal environments.
    fn git_available() -> bool {
        StdCommand::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn require_git() -> bool {
        git_available()
    }

    #[test]
    fn slugify_makes_safe_defaults() {
        assert_eq!(slugify("feat/ui-make-minimal"), "feat-ui-make-minimal");
        assert_eq!(slugify("113"), "113");
        assert_eq!(slugify("Fix: Login Timeout!"), "fix-login-timeout");
        assert_eq!(slugify("a__b.c"), "a__b.c");
        assert_eq!(slugify("--"), "");
        assert_eq!(slugify("..."), "");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn valid_names_are_single_components() {
        for name in [
            "113",
            "127",
            "feature-login",
            "issue-113",
            "client-a",
            "my_task",
            "a.b",
        ] {
            assert_eq!(validate_name(name), Ok(name), "{name} should be valid");
        }
        // Surrounding whitespace is trimmed like every Orbit identifier.
        assert_eq!(validate_name("  113 "), Ok("113"));
    }

    #[test]
    fn invalid_names_are_rejected() {
        for name in [
            "", " ", ".", "..", "../foo", "foo/bar", "foo\\bar", "foo\0bar",
        ] {
            assert!(
                matches!(validate_name(name), Err(WorktreeError::InvalidName(_))),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn parser_reads_normal_branch_worktree() {
        let raw = "worktree /repo\0HEAD abc123\0branch refs/heads/main\0\0worktree /repo/.wt/113\0HEAD def456\0branch refs/heads/feature/issue-113\0\0";
        let list = parse_worktree_list(raw, Some(Path::new("/repo/.wt")));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "repo");
        assert!(list[0].is_main);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].name, "113");
        assert!(!list[1].is_main);
        assert_eq!(list[1].branch.as_deref(), Some("feature/issue-113"));
        assert_eq!(list[1].path, PathBuf::from("/repo/.wt/113"));
        assert!(!list[1].detached);
        assert!(!list[1].locked);
        assert!(!list[1].prunable);
    }

    #[test]
    fn parser_reads_detached_worktree() {
        let raw = "worktree /repo\0HEAD abc123\0branch refs/heads/main\0\0worktree /repo/.wt/det\0HEAD def456\0detached\0\0";
        let list = parse_worktree_list(raw, Some(Path::new("/repo/.wt")));
        assert_eq!(list[1].branch, None);
        assert!(list[1].detached);
        assert_eq!(list[1].branch_label(), tr!("worktree.detached"));
    }

    #[test]
    fn parser_reads_locked_worktree_with_reason() {
        let raw = "worktree /repo\0HEAD abc123\0branch refs/heads/main\0\0worktree /repo/.wt/127\0HEAD def456\0branch refs/heads/fix/login\0locked on usb drive\0\0";
        let list = parse_worktree_list(raw, Some(Path::new("/repo/.wt")));
        assert!(list[1].locked);
        assert_eq!(list[1].lock_reason.as_deref(), Some("on usb drive"));
    }

    #[test]
    fn parser_reads_prunable_worktree() {
        let raw = "worktree /repo\0HEAD abc123\0branch refs/heads/main\0\0worktree /repo/.wt/gone\0HEAD def456\0branch refs/heads/tmp\0prunable gitdir file points to non-existent location\0\0";
        let list = parse_worktree_list(raw, Some(Path::new("/repo/.wt")));
        assert!(list[1].prunable);
        assert_eq!(
            list[1].prune_reason.as_deref(),
            Some("gitdir file points to non-existent location")
        );
    }

    #[test]
    fn parser_reads_bare_repository() {
        let raw = "worktree /repo.git\0HEAD abc123\0bare\0\0";
        let list = parse_worktree_list(raw, None);
        assert_eq!(list.len(), 1);
        assert!(list[0].bare);
        assert!(list[0].is_main);
    }

    #[test]
    fn parser_falls_back_to_directory_name_outside_the_root() {
        let raw = "worktree /repo\0HEAD abc123\0branch refs/heads/main\0\0worktree /elsewhere/experiment\0HEAD def456\0branch refs/heads/x\0\0";
        let list = parse_worktree_list(raw, Some(Path::new("/repo/.wt")));
        assert_eq!(list[1].name, "experiment");
    }

    #[test]
    fn parser_handles_unterminated_final_record() {
        let raw = "worktree /repo\0HEAD abc123\0branch refs/heads/main";
        let list = parse_worktree_list(raw, None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn name_is_not_branch_or_path() {
        let worktree = Worktree {
            path: PathBuf::from("/repo/.wt/113"),
            name: "113".into(),
            head: "abc".into(),
            branch: Some("feature/issue-113".into()),
            detached: false,
            is_main: false,
            bare: false,
            locked: false,
            lock_reason: None,
            prunable: false,
            prune_reason: None,
        };
        assert_ne!(worktree.name, worktree.branch.clone().unwrap());
        assert_ne!(worktree.name, worktree.path.to_string_lossy());
    }

    #[test]
    fn effective_directory_prefers_repo_config() {
        let repo = TempRepo::new();
        repo.init();
        let config = WorktreeConfig::default();
        assert_eq!(
            config.effective_directory(&repo.path),
            repo.path.join(".wt")
        );
        std::fs::create_dir_all(repo.path.join(".orbit")).expect("make .orbit");
        std::fs::write(
            WorktreeRepoConfig::path(&repo.path),
            r#"{ "directory": "worktrees" }"#,
        )
        .expect("write repo config");
        assert_eq!(
            config.effective_directory(&repo.path),
            repo.path.join("worktrees")
        );
    }

    #[test]
    fn absolute_directory_stays_absolute() {
        let repo = TempRepo::new();
        let absolute = repo.path.join("somewhere-else");
        let config = WorktreeConfig {
            directory: absolute.to_string_lossy().into_owned(),
            ..WorktreeConfig::default()
        };
        assert_eq!(config.effective_directory(&repo.path), absolute);
    }

    #[test]
    fn setup_script_resolution_requires_a_file() {
        let repo = TempRepo::new();
        let config = WorktreeConfig::default();
        assert_eq!(config.effective_setup_script(&repo.path), None);
        let script = repo.path.join(DEFAULT_SETUP_SCRIPT);
        std::fs::create_dir_all(script.parent().expect("script parent")).expect("make .orbit");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").expect("write script");
        assert_eq!(config.effective_setup_script(&repo.path), Some(script));
    }

    #[test]
    fn manager_resolves_paths_inside_the_root() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();
        let resolved = manager.resolve_path("113").expect("resolve");
        assert!(
            same_path(&resolved, &repo.path.join(".wt/113")),
            "{resolved:?} should be the repo's .wt/113"
        );
        let resolved = manager.resolve_path(" 127 ").expect("resolve");
        assert!(same_path(&resolved, &repo.path.join(".wt/127")));
        assert!(matches!(
            manager.resolve_path("../escape"),
            Err(WorktreeError::InvalidName(_))
        ));
    }

    #[test]
    fn lifecycle_create_list_move_lock_remove_prune() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();

        // Create from an existing branch that is not checked out anywhere.
        // (`main` is taken by the main worktree, so it must not be reused.)
        repo.git(&["branch", "feature/base"]);
        manager
            .create("113", &repo.path.join(".wt/113"), "feature/base")
            .expect("create existing");
        let list = manager.list().expect("list");
        assert_eq!(list.len(), 2, "main plus the linked worktree");
        assert_eq!(
            list[0].name,
            repo.path.file_name().unwrap().to_string_lossy()
        );
        assert!(list[0].is_main);

        // Create with a new branch.
        let created = manager
            .create_new_branch(
                "127",
                &repo.path.join(".wt/127"),
                "fix/login-timeout",
                Some("main"),
            )
            .expect("create new branch");
        assert_eq!(created.name, "127");
        assert_eq!(created.branch.as_deref(), Some("fix/login-timeout"));

        let list = manager.list().expect("list");
        assert_eq!(list.len(), 3);
        assert!(list[0].is_main);

        // The branch and the name stay separate.
        assert_ne!(list[1].name, list[1].branch.clone().unwrap());

        // Move (rename) without touching the branch.
        manager
            .move_to(&repo.path.join(".wt/127"), &repo.path.join(".wt/issue-127"))
            .expect("move");
        let moved = manager
            .find_by_path(&repo.path.join(".wt/issue-127"))
            .expect("find moved")
            .expect("moved exists");
        assert_eq!(moved.name, "issue-127");
        assert_eq!(moved.branch.as_deref(), Some("fix/login-timeout"));

        // Lock and unlock.
        manager
            .lock(&repo.path.join(".wt/issue-127"), Some("on usb"))
            .expect("lock");
        let locked = manager
            .find_by_path(&repo.path.join(".wt/issue-127"))
            .expect("find")
            .expect("exists");
        assert!(locked.locked);
        assert_eq!(locked.lock_reason.as_deref(), Some("on usb"));
        assert!(matches!(
            manager.remove(&repo.path.join(".wt/issue-127"), false),
            Err(WorktreeError::Locked(_))
        ));
        manager
            .unlock(&repo.path.join(".wt/issue-127"))
            .expect("unlock");
        assert!(
            !manager
                .find_by_path(&repo.path.join(".wt/issue-127"))
                .expect("find")
                .expect("exists")
                .locked
        );

        // Remove.
        manager
            .remove(&repo.path.join(".wt/issue-127"), false)
            .expect("remove");
        assert_eq!(manager.list().expect("list").len(), 2);

        // Prune finds nothing; it still succeeds.
        manager.prune().expect("prune");
    }

    #[test]
    fn create_rejects_existing_branch_checked_out_elsewhere() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();
        manager
            .create_new_branch(
                "113",
                &repo.path.join(".wt/113"),
                "feature/issue-113",
                Some("main"),
            )
            .expect("create");
        let error = manager
            .create("127", &repo.path.join(".wt/127"), "feature/issue-113")
            .expect_err("branch is taken");
        match error {
            WorktreeError::BranchCheckedOut { branch, path } => {
                assert_eq!(branch, "feature/issue-113");
                assert!(same_path(&path, &repo.path.join(".wt/113")));
            }
            other => panic!("expected BranchCheckedOut, got {other:?}"),
        }
    }

    #[test]
    fn create_rejects_existing_path_and_missing_repository() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();
        std::fs::create_dir_all(repo.path.join(".wt/113")).expect("make dir");
        assert!(matches!(
            manager.create("113", &repo.path.join(".wt/113"), "main"),
            Err(WorktreeError::PathExists(_))
        ));

        let not_a_repo = TempRepo::new();
        let manager = WorktreeManager::new(&not_a_repo.path);
        assert!(matches!(
            manager.create("1", &not_a_repo.path.join(".wt/1"), "main"),
            Err(WorktreeError::NotARepository)
        ));
        assert!(matches!(manager.list(), Err(WorktreeError::NotARepository)));
    }

    #[test]
    fn remove_refuses_dirty_worktree_until_forced() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();
        manager
            .create_new_branch(
                "113",
                &repo.path.join(".wt/113"),
                "feature/issue-113",
                Some("main"),
            )
            .expect("create");
        std::fs::write(repo.path.join(".wt/113/dirty.txt"), "wip\n").expect("dirty file");
        assert!(manager
            .is_dirty(&repo.path.join(".wt/113"))
            .expect("dirty check"));
        match manager.remove(&repo.path.join(".wt/113"), false) {
            Err(WorktreeError::Dirty(path)) => {
                assert!(same_path(&path, &repo.path.join(".wt/113")));
            }
            other => panic!("expected Dirty, got {other:?}"),
        }
        manager
            .remove(&repo.path.join(".wt/113"), true)
            .expect("forced remove");
        assert_eq!(manager.list().expect("list").len(), 1);
    }

    #[test]
    fn remove_rejects_main_and_unknown_paths() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();
        assert!(matches!(
            manager.remove(&repo.path, false),
            Err(WorktreeError::NotAWorktree(_))
        ));
        assert!(matches!(
            manager.remove(&repo.path.join(".wt/nope"), false),
            Err(WorktreeError::NotAWorktree(_))
        ));
    }

    #[test]
    fn new_branch_validation_uses_git_ref_format() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();
        assert!(matches!(
            manager.create_new_branch(
                "1",
                &repo.path.join(".wt/1"),
                "bad branch name",
                Some("main")
            ),
            Err(WorktreeError::InvalidBranch(_))
        ));
    }

    #[test]
    fn repair_succeeds_on_a_healthy_repository() {
        if !require_git() {
            return;
        }
        let repo = TempRepo::new();
        repo.init();
        let manager = repo.manager();
        manager
            .create_new_branch(
                "113",
                &repo.path.join(".wt/113"),
                "feature/issue-113",
                Some("main"),
            )
            .expect("create");
        manager.repair().expect("repair");
        assert_eq!(manager.list().expect("list").len(), 2);
    }

    #[test]
    fn config_round_trips_through_disk() {
        let repo = TempRepo::new();
        let path = repo.path.join("worktrees.json");
        let mut config = WorktreeConfig {
            directory: "worktrees".into(),
            setup_script: "scripts/setup.sh".into(),
            run_setup_auto: false,
            open_after_create: false,
            allowed_setup_repos: Vec::new(),
        };
        config.allow_setup(&repo.path);
        let payload = serde_json::to_string(&config).expect("serialize");
        std::fs::write(&path, payload).expect("write");
        let raw = std::fs::read_to_string(&path).expect("read");
        let parsed: WorktreeConfig = serde_json::from_str(&raw).expect("parse");
        assert_eq!(parsed, config);
    }

    #[test]
    fn setup_allowed_keys_are_canonical() {
        let repo = TempRepo::new();
        let mut config = WorktreeConfig::default();
        assert!(!config.setup_allowed(&repo.path));
        config.allow_setup(&repo.path);
        assert!(config.setup_allowed(&repo.path));
        // Recording twice must not duplicate the entry.
        config.allow_setup(&repo.path);
        assert_eq!(config.allowed_setup_repos.len(), 1);
    }
}
