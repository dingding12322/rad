//! Plugin (pi package) discovery and management.
//!
//! pi's "packages" are its extensions — npm packages, git repos, or local
//! paths recorded in the `packages` array of `~/.pi/agent/settings.json`
//! (user scope) or `<workspace>/.pi/settings.json` (project scope). Each
//! source resolves to a managed install directory:
//!
//! - npm  → `<base>/npm/node_modules/<name>`
//! - git  → `<base>/git/<host>/<path>`
//! - local → the path itself, relative to `<base>`
//!
//! where `<base>` is `~/.pi/agent` for user scope and `<workspace>/.pi` for
//! project scope. Orbit reads those settings files and the install dirs, and
//! installs / removes / updates through the `pi` CLI itself — pi owns the
//! network fetch, lockfile, and settings write. Update checks inspect npm
//! registry metadata or a Git remote without changing installed package files.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long an install / update may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(240);
/// Update probes must not keep startup busy when a registry or Git host stalls.
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_PARALLEL_PROBES: usize = 4;
const SKIPPED_UPDATES_FILE: &str = "plugin-update-skips.json";

/// Where a package is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackageScope {
    /// `~/.pi/agent/settings.json`.
    User,
    /// `<workspace>/.pi/settings.json`.
    Project,
}

impl PackageScope {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PackageScope::User => "Global",
            PackageScope::Project => "Project",
        }
    }
}

/// The source family, shown as a badge on each card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackageKind {
    Npm,
    Git,
    Local,
}

impl PackageKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PackageKind::Npm => "npm",
            PackageKind::Git => "git",
            PackageKind::Local => "local",
        }
    }
}

/// One configured package, flattened to what the settings page renders.
#[derive(Debug, Clone)]
pub(crate) struct PluginPackage {
    /// The settings string, e.g. `npm:@ollama/pi-web-search`.
    pub source: String,
    /// Short display name (package or repo name).
    pub name: String,
    pub scope: PackageScope,
    pub kind: PackageKind,
    /// The directory pi installs this source into.
    pub install_path: PathBuf,
    /// The install directory exists on disk.
    pub installed: bool,
    /// `version` from the installed package's `package.json`, when readable.
    pub version: Option<String>,
}

/// One installed npm or Git package whose upstream has moved ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginUpdate {
    pub source: String,
    pub name: String,
    pub current: String,
    pub latest: String,
}

/// Read every configured package for `workspace`. The optional error string
/// is a malformed settings file; the returned list still holds whatever the
/// other file contributed, so one bad file never hides the rest.
pub(crate) fn discover(workspace: &Path) -> (Vec<PluginPackage>, Option<String>) {
    let mut packages = Vec::new();
    let mut error = None;
    let mut seen: Vec<String> = Vec::new();

    // Project scope first — it wins a duplicate source over user scope.
    for (scope, path) in [
        (PackageScope::Project, workspace.join(".pi/settings.json")),
        (PackageScope::User, agent_dir().join("settings.json")),
    ] {
        match read_packages(&path, scope, workspace) {
            Ok(list) => {
                for package in list {
                    if seen.contains(&package.source) {
                        continue;
                    }
                    seen.push(package.source.clone());
                    packages.push(package);
                }
            }
            Err(err) => {
                error.get_or_insert(err);
            }
        }
    }

    packages.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.scope.label().cmp(b.scope.label()))
    });
    (packages, error)
}

/// Install `source` via `pi install`, optionally into project scope.
pub(crate) fn install(source: &str, project: bool, workspace: &Path) -> Result<String, String> {
    let mut args = vec!["install".to_string(), source.to_string()];
    if project {
        // Trust the project-local settings for this command only; the app
        // already launches pi with `--approve`.
        args.push("-l".to_string());
        args.push("-a".to_string());
    }
    run_pi(&args, workspace)
}

/// Remove `source` from the scope it was installed in.
pub(crate) fn remove(source: &str, project: bool, workspace: &Path) -> Result<String, String> {
    let mut args = vec!["remove".to_string(), source.to_string()];
    if project {
        args.push("-l".to_string());
        args.push("-a".to_string());
    }
    run_pi(&args, workspace)
}

/// Update a single installed `source`.
pub(crate) fn update(source: &str, workspace: &Path) -> Result<String, String> {
    let args = vec!["update".to_string(), source.to_string()];
    run_pi(&args, workspace)
}

/// Check installed npm and Git packages for newer upstream versions. Local
/// paths and explicitly pinned sources have no unambiguous "latest". Probes
/// are bounded in parallel so one slow registry cannot delay every package.
pub(crate) fn find_updates(
    workspace: &Path,
    skipped: &HashMap<String, String>,
) -> Vec<PluginUpdate> {
    let (packages, _) = discover(workspace);
    let packages: Vec<_> = packages
        .into_iter()
        .filter(|package| package.installed && package.kind != PackageKind::Local)
        .collect();
    let mut updates = Vec::new();
    for batch in packages.chunks(MAX_PARALLEL_PROBES) {
        let checked = std::thread::scope(|scope| {
            batch
                .iter()
                .map(|package| {
                    scope.spawn(move || find_package_update(package, workspace, skipped))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|task| task.join().ok().flatten())
                .collect::<Vec<_>>()
        });
        updates.extend(checked);
    }
    updates
}

fn find_package_update(
    package: &PluginPackage,
    workspace: &Path,
    skipped: &HashMap<String, String>,
) -> Option<PluginUpdate> {
    let (current, latest) = match package.kind {
        PackageKind::Npm => {
            let current = package.version.clone()?;
            let latest = latest_npm_version(&package.source, workspace)?;
            if !crate::pi_update::is_newer(&latest, &current) {
                return None;
            }
            (current, latest)
        }
        PackageKind::Git => git_revisions(&package.source, &package.install_path)?,
        PackageKind::Local => return None,
    };
    if current == latest || skipped.get(&package.source) == Some(&latest) {
        return None;
    }
    Some(PluginUpdate {
        source: package.source.clone(),
        name: package.name.clone(),
        current,
        latest,
    })
}

/// Read versions pi has already been offered and the user chose to skip.
pub(crate) fn load_skipped_updates() -> HashMap<String, String> {
    let Some(path) = skipped_updates_path() else {
        return HashMap::new();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist skips by source and upstream version, so the same update is not
/// offered again on every launch. A new version naturally appears again.
pub(crate) fn save_skipped_updates(skipped: &HashMap<String, String>) -> Result<(), String> {
    let path =
        skipped_updates_path().ok_or_else(|| "Could not locate the home directory".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Could not locate the Orbit settings directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let raw = serde_json::to_vec_pretty(skipped).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, raw).map_err(|error| error.to_string())?;
    fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

fn skipped_updates_path() -> Option<PathBuf> {
    Some(
        crate::platform::home_dir_opt()?
            .join(".orbit-pi")
            .join(SKIPPED_UPDATES_FILE),
    )
}

fn latest_npm_version(source: &str, workspace: &Path) -> Option<String> {
    let name = npm_update_name(source)?;
    let package_spec = format!("{name}@latest");
    let mut command = npm_view_command(&package_spec);
    command
        .env("PATH", probe_path())
        .env("NO_COLOR", "1")
        .env("CI", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(workspace);
    let (success, stdout, _) = run_probe(command)?;
    if !success {
        return None;
    }
    npm_version_from_output(&stdout)
}

#[cfg(windows)]
fn npm_view_command(package_spec: &str) -> Command {
    // Windows npm installs expose npm.cmd rather than an executable. This is
    // safe to pass through cmd because npm_update_name restricts the spec to
    // package-name characters (no shell operators, quotes, or whitespace).
    let mut command = Command::new("cmd.exe");
    command
        .arg("/D")
        .arg("/S")
        .arg("/C")
        .arg(format!("npm view --json {package_spec} version"));
    command
}

#[cfg(not(windows))]
fn npm_view_command(package_spec: &str) -> Command {
    let mut command = Command::new("npm");
    command
        .args(["view", "--json"])
        .arg(package_spec)
        .arg("version");
    command
}

fn npm_version_from_output(output: &str) -> Option<String> {
    let version = serde_json::from_str::<Value>(output)
        .ok()
        .and_then(|value| match value {
            Value::String(version) => Some(version),
            Value::Array(versions) => versions
                .into_iter()
                .find_map(|value| value.as_str().map(str::to_string)),
            Value::Object(object) => object
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .or_else(|| Some(output.trim().trim_matches('"').to_string()))?;
    version
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
        .then_some(version)
}

/// Only unpinned npm sources have a registry-defined update target. Restrict
/// the name alphabet before passing it to npm, even though no shell is used.
fn npm_update_name(source: &str) -> Option<String> {
    let spec = source.strip_prefix("npm:")?.trim();
    let name = npm_name(spec);
    if spec != name
        || name.is_empty()
        || name.starts_with('-')
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "@/._-".contains(character))
    {
        return None;
    }
    Some(name)
}

/// Compare the installed Git checkout with the default branch's advertised
/// HEAD. Repositories without a remote checkout are left alone.
fn git_revisions(source: &str, install_path: &Path) -> Option<(String, String)> {
    if source.contains('#') {
        return None;
    }
    let current = git_output(install_path, &["rev-parse", "HEAD"])?;
    let remote = git_output(install_path, &["remote", "get-url", "origin"])?;
    let mut command = Command::new("git");
    command
        .args(["ls-remote", "--symref", "--"])
        .arg(remote)
        .arg("HEAD")
        .env("PATH", probe_path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(install_path);
    let (success, stdout, _) = run_probe(command)?;
    if !success {
        return None;
    }
    let latest = git_head_revision(&stdout)?;
    Some((current, latest))
}

fn git_output(install_path: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .env("PATH", probe_path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(install_path);
    let (success, stdout, _) = run_probe(command)?;
    success
        .then(|| stdout.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_head_revision(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let revision = fields.next()?;
        (fields.next()? == "HEAD" && is_git_revision(revision)).then(|| revision.to_string())
    })
}

fn is_git_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn probe_path() -> std::ffi::OsString {
    let pi_bin = orbit_rpc::pi_binary();
    orbit_rpc::augmented_path(Path::new(&pi_bin).parent())
}

/// Run a metadata-only command with bounded execution time and drain both
/// pipes concurrently so a stalled registry cannot block the UI or deadlock.
fn run_probe(mut command: Command) -> Option<(bool, String, String)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    orbit_rpc::hide_console(&mut command);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let out_handle = std::thread::spawn(move || read_to_end(stdout));
    let err_handle = std::thread::spawn(move || read_to_end(stderr));
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                return None;
            }
        }
    };
    Some((
        status.success(),
        out_handle.join().ok()?,
        err_handle.join().ok()?,
    ))
}

fn read_packages(
    path: &Path,
    scope: PackageScope,
    workspace: &Path,
) -> Result<Vec<PluginPackage>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(tr!(
                "errors.could_not_read",
                path = path.display().to_string(),
                error = err
            ));
        }
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let root: Value = serde_json::from_str(&raw).map_err(|err| {
        tr!(
            "errors.not_valid_json",
            path = path.display().to_string(),
            error = err
        )
    })?;
    Ok(package_sources(&root)
        .into_iter()
        .map(|source| resolve(&source, scope, workspace))
        .collect())
}

/// Pull the package source strings out of a settings document. Entries are
/// strings (`"npm:foo"`) or objects with a `source` field.
fn package_sources(root: &Value) -> Vec<String> {
    let Some(list) = root.get("packages").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| match entry {
            Value::String(source) if !source.trim().is_empty() => Some(source.clone()),
            Value::Object(map) => map
                .get("source")
                .and_then(Value::as_str)
                .filter(|source| !source.trim().is_empty())
                .map(str::to_string),
            _ => None,
        })
        .collect()
}

fn resolve(source: &str, scope: PackageScope, workspace: &Path) -> PluginPackage {
    let parsed = parse_source(source);
    let install_path = parsed.install_path(scope, workspace);
    let installed = install_path.exists();
    let version = if installed {
        read_version(&install_path)
    } else {
        None
    };
    PluginPackage {
        source: source.to_string(),
        name: parsed.name().to_string(),
        scope,
        kind: parsed.kind(),
        install_path,
        installed,
        version,
    }
}

/// A parsed package source, before it is resolved against a scope.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    Npm {
        name: String,
    },
    Git {
        host: String,
        path: String,
        name: String,
    },
    Local {
        path: PathBuf,
    },
}

impl Source {
    fn name(&self) -> &str {
        match self {
            Source::Npm { name } => name,
            Source::Git { name, .. } => name,
            Source::Local { path } => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("package"),
        }
    }

    fn kind(&self) -> PackageKind {
        match self {
            Source::Npm { .. } => PackageKind::Npm,
            Source::Git { .. } => PackageKind::Git,
            Source::Local { .. } => PackageKind::Local,
        }
    }

    fn install_path(&self, scope: PackageScope, workspace: &Path) -> PathBuf {
        let base = match scope {
            PackageScope::User => agent_dir(),
            PackageScope::Project => workspace.join(".pi"),
        };
        match self {
            Source::Npm { name } => base.join("npm").join("node_modules").join(name),
            Source::Git { host, path, .. } => base.join("git").join(host).join(path),
            Source::Local { path } => {
                if path.is_absolute() {
                    path.clone()
                } else {
                    base.join(path)
                }
            }
        }
    }
}

fn parse_source(raw: &str) -> Source {
    let source = raw.trim();
    if let Some(spec) = source.strip_prefix("npm:") {
        return Source::Npm {
            name: npm_name(spec),
        };
    }
    if let Some(rest) = source.strip_prefix("git:") {
        return parse_git(rest);
    }
    if let Some(rest) = source.strip_prefix("github:") {
        return parse_git(&format!("github.com/{rest}"));
    }
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = source.strip_prefix(scheme) {
            return parse_git(rest);
        }
    }
    let path = source.strip_prefix("./").unwrap_or(source);
    Source::Local {
        path: PathBuf::from(path),
    }
}

/// Parse a git source that has already had its scheme / `git:` prefix
/// removed — `github.com/owner/repo`, `git@github.com:owner/repo`, etc.
fn parse_git(rest: &str) -> Source {
    let rest = rest.split('#').next().unwrap_or(rest);
    let rest = rest.trim().trim_start_matches("git@");
    let (host, path) = if let Some((host, path)) = rest.split_once(':') {
        (host.to_string(), path.trim_start_matches('/').to_string())
    } else if let Some((host, path)) = rest.split_once('/') {
        (host.to_string(), path.to_string())
    } else {
        (String::new(), rest.to_string())
    };
    let path = path.trim().trim_end_matches(".git").to_string();
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    Source::Git { host, path, name }
}

/// `@scope/name@version` / `name@version` → the bare package name.
fn npm_name(spec: &str) -> String {
    let spec = spec.trim();
    if spec.starts_with('@') {
        if let Some(slash) = spec.find('/') {
            if let Some(at) = spec[slash..].find('@') {
                return spec[..slash + at].to_string();
            }
        }
        return spec.to_string();
    }
    spec.split_once('@')
        .map(|(name, _)| name)
        .unwrap_or(spec)
        .to_string()
}

fn read_version(install_path: &Path) -> Option<String> {
    let raw = fs::read_to_string(install_path.join("package.json")).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn agent_dir() -> PathBuf {
    crate::platform::home_dir().join(".pi").join("agent")
}

/// Run `pi <args>` from `workspace`, returning stdout on success. Mirrors the
/// commit-message runner: blocking, so call it on the background executor.
fn run_pi(args: &[String], workspace: &Path) -> Result<String, String> {
    let bin = orbit_rpc::pi_binary();
    let mut command = Command::new(&bin);
    command
        .args(args)
        .env("PI_SKIP_VERSION_CHECK", "1")
        // `pi` is `#!/usr/bin/env node`; a bundled `.app` PATH lacks `node`.
        .env("PATH", orbit_rpc::augmented_path(Path::new(&bin).parent()))
        .env("NO_COLOR", "1")
        .env("CI", "1")
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    orbit_rpc::hide_console(&mut command);

    let mut child = command
        .spawn()
        .map_err(|err| tr!("errors.failed_to_run", bin = bin, error = err))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| tr!("plugins.stdout_unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| tr!("plugins.stderr_unavailable"))?;
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
                    Err(first_line(&err))
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(tr!("errors.pi_timed_out", args = args.join(" ")));
                }
                std::thread::sleep(Duration::from_millis(50));
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
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_npm_sources() {
        assert_eq!(
            parse_source("npm:@ollama/pi-web-search"),
            Source::Npm {
                name: "@ollama/pi-web-search".into()
            }
        );
        assert_eq!(
            parse_source("npm:pi-tracker@1.2.3"),
            Source::Npm {
                name: "pi-tracker".into()
            }
        );
        assert_eq!(
            parse_source("npm:@scope/pkg@2.0.0"),
            Source::Npm {
                name: "@scope/pkg".into()
            }
        );
    }

    #[test]
    fn reads_npm_view_version_shapes() {
        assert_eq!(
            npm_version_from_output(r#"["1.2.3"]"#),
            Some("1.2.3".into())
        );
        assert_eq!(npm_version_from_output(r#""2.3.4""#), Some("2.3.4".into()));
        assert_eq!(npm_version_from_output("not a version"), None);
    }

    #[test]
    fn only_unpinned_npm_sources_are_checked() {
        assert_eq!(
            npm_update_name("npm:pi-web-search"),
            Some("pi-web-search".into())
        );
        assert_eq!(
            npm_update_name("npm:@scope/pi-ext"),
            Some("@scope/pi-ext".into())
        );
        assert_eq!(npm_update_name("npm:pi-web-search@1.2.3"), None);
        assert_eq!(npm_update_name("npm:--help"), None);
    }

    #[test]
    fn extracts_a_valid_git_head_from_ls_remote_output() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let output = format!("ref: refs/heads/main\tHEAD\n{hash}\tHEAD\n");
        assert_eq!(git_head_revision(&output).as_deref(), Some(hash));
        assert_eq!(git_head_revision("bad\tHEAD\n"), None);
        assert!(!is_git_revision("not-a-hash"));
    }

    #[test]
    fn parses_git_sources() {
        assert_eq!(
            parse_source("git:github.com/earendil-works/pi-review"),
            Source::Git {
                host: "github.com".into(),
                path: "earendil-works/pi-review".into(),
                name: "pi-review".into(),
            }
        );
        assert_eq!(
            parse_source("https://github.com/user/repo.git"),
            Source::Git {
                host: "github.com".into(),
                path: "user/repo".into(),
                name: "repo".into(),
            }
        );
        assert_eq!(
            parse_source("git:git@github.com:user/repo"),
            Source::Git {
                host: "github.com".into(),
                path: "user/repo".into(),
                name: "repo".into(),
            }
        );
    }

    #[test]
    fn parses_local_sources() {
        assert_eq!(
            parse_source("./plugins/demo"),
            Source::Local {
                path: PathBuf::from("plugins/demo")
            }
        );
    }

    #[test]
    fn resolves_install_paths() {
        let workspace = Path::new("/tmp/work");
        let npm = parse_source("npm:@scope/pkg");
        assert_eq!(
            npm.install_path(PackageScope::Project, workspace),
            PathBuf::from("/tmp/work/.pi/npm/node_modules/@scope/pkg")
        );
        let git = parse_source("git:github.com/o/r");
        assert_eq!(
            git.install_path(PackageScope::User, workspace),
            agent_dir().join("git/github.com/o/r")
        );
    }

    #[test]
    fn reads_string_and_object_packages() {
        let raw = r#"{"packages":["npm:a",{"source":"git:github.com/x/y"},42,"  "]}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            package_sources(&value),
            vec!["npm:a".to_string(), "git:github.com/x/y".to_string()]
        );
    }
}
