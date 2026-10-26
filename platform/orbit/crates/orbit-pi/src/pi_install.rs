//! Pi CLI install lifecycle: detect how `pi` was installed, install or migrate
//! it with pi's official installer, update it with pi's own updater, and
//! verify the result.
//!
//! Orbit never installs or updates the pi CLI through npm. On macOS and Linux
//! the official installer (`https://pi.dev/install.sh`) creates a pinned,
//! self-updating *managed* install under `~/.pi/agent/install`, a launcher at
//! `~/.pi/agent/bin/pi`, and an entrypoint symlink on the user's PATH. When it
//! finds an npm global install it migrates it: the managed install is created
//! first, then the npm package is removed. Later updates go through
//! `pi update`, which the installer itself documents.
//!
//! Windows has no managed installer (pi's installer supports macOS and Linux
//! only), so Orbit reports the documented command instead of running npm
//! itself.
//!
//! Everything here blocks; the app calls it from the background executor. No
//! function reads a credential or touches `~/.pi/agent` user data beyond the
//! install directory the official installer owns.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// pi's official installer, exactly as the project documents it.
pub const INSTALL_SCRIPT_URL: &str = "https://pi.dev/install.sh";
/// The copyable one-liner shown to a user who has no pi yet.
pub const INSTALL_COMMAND: &str = "curl -fsSL https://pi.dev/install.sh | sh";
/// The npm package that identifies a pi install inside a `node_modules` tree.
pub const PI_PACKAGE_NAME: &str = "@earendil-works/pi-coding-agent";
/// The marker the official installer writes into the managed install root.
const MANAGED_MARKER: &str = "managed-install.json";
/// Overrides the installer URL. Orbit's own seam for tests; pi's installer is
/// unchanged.
pub const INSTALL_URL_ENV: &str = "ORBIT_PI_INSTALL_URL";
/// Overrides the whole installer command. Orbit's own seam for tests.
pub const INSTALL_COMMAND_ENV: &str = "ORBIT_PI_INSTALL_CMD";

/// How the resolved `pi` was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// pi's official managed install: a `managed-install.json` marker in
    /// `<agent>/install` (or `PI_MANAGED_INSTALL_ROOT`).
    Managed { root: PathBuf },
    /// A global npm install (`node_modules/@earendil-works/pi-coding-agent`).
    Npm { package_root: PathBuf },
    /// Some other package manager (Nix, Homebrew, a hand-built shim, …).
    Other,
}

impl InstallKind {
    pub fn is_managed(&self) -> bool {
        matches!(self, Self::Managed { .. })
    }

    pub fn is_npm(&self) -> bool {
        matches!(self, Self::Npm { .. })
    }
}

/// A snapshot of the pi CLI on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    /// Resolved launcher, when one was found.
    pub bin: Option<PathBuf>,
    /// First line of `pi --version`, when it could be read.
    pub version: Option<String>,
    /// The install layout the launcher belongs to.
    pub kind: InstallKind,
    /// Whether `pi` on the process PATH resolves to this exact install.
    pub on_path: bool,
}

impl Installation {
    /// Nothing found — the shape used before the first probe and by tests.
    #[allow(dead_code)]
    pub fn not_installed() -> Self {
        Self {
            bin: None,
            version: None,
            kind: InstallKind::Other,
            on_path: false,
        }
    }

    pub fn installed(&self) -> bool {
        self.bin.is_some()
    }
}

/// What Orbit should do when the user asks to install/repair pi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Nothing is installed: run the official installer.
    Install,
    /// An official managed install is already present: do not make a second.
    AlreadyManaged,
    /// A non-managed pi owns the name (`npm`, Nix, Homebrew, …). Installing
    /// would create a competing copy, so the caller must migrate or refuse.
    Conflict(PathBuf),
}

/// What Orbit should do about a newer release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePlan {
    /// Nothing newer is published.
    UpToDate,
    /// A managed install: update through `pi update`.
    Update,
    /// An npm install: migrate with the official installer (needs the user's
    /// confirmation — the installer removes the npm package).
    Migrate,
    /// Orbit cannot update this install.
    Unsupported(UnsupportedReason),
}

/// Why an install cannot be updated by Orbit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedReason {
    /// Provided by another package manager (Nix, Homebrew, …).
    PackageManager,
    /// No version could be read from the installed launcher.
    Unknown,
}

/// The result of a post-install/post-update probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    pub installation: Installation,
}

impl Verification {
    /// True when the resolved install is the official managed one.
    pub fn managed(&self) -> bool {
        self.installation.kind.is_managed()
    }

    /// True when the resolved install is on the process PATH.
    pub fn on_path(&self) -> bool {
        self.installation.on_path
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Paths
// ───────────────────────────────────────────────────────────────────────────

/// pi's agent data directory: `PI_CODING_AGENT_DIR` or `~/.pi/agent`.
///
/// User data (credentials, sessions, settings, packages) lives here and is
/// never touched by Orbit. Only the official installer's `install/` subtree is
/// install state.
pub fn agent_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    crate::platform::home_dir().join(".pi").join("agent")
}

/// Managed install roots to check, most-specific first.
fn managed_root_candidates(agent_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = std::env::var_os("PI_MANAGED_INSTALL_ROOT").filter(|v| !v.is_empty()) {
        roots.push(PathBuf::from(dir));
    }
    let default = agent_dir.join("install");
    if !roots.contains(&default) {
        roots.push(default);
    }
    roots
}

/// A managed root is real only when the installer's marker is present and
/// valid — the same `kind` / `schemaVersion` / `layout` triple the installer
/// checks, so a stray `install/` directory is never mistaken for pi.
pub fn managed_marker_is_valid(root: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(root.join(MANAGED_MARKER)) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    value.get("kind").and_then(serde_json::Value::as_str) == Some("pi-managed-install")
        && value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            == Some(1)
        && value.get("layout").and_then(serde_json::Value::as_str) == Some("releases-v1")
}

/// The managed install root for a launcher, if the launcher belongs to one.
fn managed_root_for_bin(bin: &Path, agent_dir: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
    for root in managed_root_candidates(agent_dir) {
        if !managed_marker_is_valid(&root) {
            continue;
        }
        // Compare canonical paths on both sides: `/var` is a symlink to
        // `/private/var` on macOS, and a `~` may resolve elsewhere too.
        let root = fs::canonicalize(&root).unwrap_or(root);
        // Anything inside `<root>/releases/<v>/…`, or the launcher the
        // installer keeps beside it at `<agent>/bin/pi`.
        if real.starts_with(&root) {
            return Some(root);
        }
        if let Some(agent) = root.parent() {
            if real.starts_with(agent.join("bin")) {
                return Some(root);
            }
        }
    }
    None
}

/// The npm package root inside `node_modules`, walking up from a launcher.
fn walk_up_to_package_root(path: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let start = if real.is_dir() {
        real
    } else {
        real.parent()?.to_path_buf()
    };
    let mut dir = start;
    for _ in 0..12 {
        if is_pi_package(&dir) {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn is_pi_package(dir: &Path) -> bool {
    fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|name| name == PI_PACKAGE_NAME)
}

// ───────────────────────────────────────────────────────────────────────────
// Detection
// ───────────────────────────────────────────────────────────────────────────

/// Classify a resolved launcher against the agent directory. Pure (no env, no
/// spawn) so tests can drive it with a temp tree.
pub fn classify(bin: &Path, agent_dir: &Path) -> InstallKind {
    if let Some(root) = managed_root_for_bin(bin, agent_dir) {
        return InstallKind::Managed { root };
    }
    if let Some(package_root) = walk_up_to_package_root(bin) {
        return InstallKind::Npm { package_root };
    }
    InstallKind::Other
}

/// Whether `pi` on `path` resolves to the same real file as `real`.
fn resolves_on_path(real: &Path, path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(path).any(|dir| {
        let candidate = dir.join("pi");
        candidate.is_file()
            && fs::canonicalize(&candidate)
                .map(|resolved| resolved == real)
                .unwrap_or(false)
    })
}

/// Classify an already-located launcher without spawning anything.
pub fn detect_from_bin(bin: Option<PathBuf>) -> Installation {
    let agent = agent_dir();
    let kind = match bin.as_deref() {
        Some(path) => classify(path, &agent),
        None => InstallKind::Other,
    };
    let on_path = bin
        .as_deref()
        .map(|path| {
            let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            std::env::var_os("PATH")
                .map(|path_var| resolves_on_path(&real, &path_var))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    Installation {
        bin,
        version: None,
        kind,
        on_path,
    }
}

/// Probe the installed pi: resolve the launcher, read its version, and
/// classify it. Blocking (spawns `pi --version`).
pub fn detect() -> Installation {
    let bin = crate::onboarding::locate("pi");
    let mut installation = detect_from_bin(bin);
    installation.version = installation
        .bin
        .as_deref()
        .and_then(crate::onboarding::version_of);
    installation
}

/// Re-probe the expected executable and version after an install or update.
pub fn verify() -> Verification {
    Verification {
        installation: detect(),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Installer invocation
// ───────────────────────────────────────────────────────────────────────────

/// The program + args that run pi's official installer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
}

/// pi's managed installer supports macOS and Linux; other platforms must use
/// their own documented mechanism.
pub fn official_install_supported() -> bool {
    supported_platform(std::env::consts::OS)
}

fn supported_platform(os: &str) -> bool {
    matches!(os, "macos" | "linux")
}

/// Build the official installer invocation, honoring Orbit's test seams.
///
/// Default (macOS/Linux): `sh -c "curl -fsSL https://pi.dev/install.sh | sh"`.
/// Returns `None` on an unsupported platform so the caller can report the
/// documented command instead of invoking a shell installer that cannot work.
pub fn installer_invocation() -> Option<Invocation> {
    let command = std::env::var_os(INSTALL_COMMAND_ENV).filter(|v| !v.is_empty());
    let url = std::env::var_os(INSTALL_URL_ENV).filter(|v| !v.is_empty());
    installer_invocation_for(
        std::env::consts::OS,
        command.as_deref().map(|v| v.to_string_lossy().into_owned()),
        url.as_deref().map(|v| v.to_string_lossy().into_owned()),
    )
}

fn installer_invocation_for(
    os: &str,
    command_override: Option<String>,
    url_override: Option<String>,
) -> Option<Invocation> {
    if !supported_platform(os) {
        return None;
    }
    if let Some(command) = command_override.filter(|c| !c.trim().is_empty()) {
        return Some(Invocation {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), command],
        });
    }
    let url = url_override.unwrap_or_else(|| INSTALL_SCRIPT_URL.to_string());
    Some(Invocation {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), format!("curl -fsSL {url} | sh")],
    })
}

/// Run one installer invocation with a GUI-safe environment: no stdin, a
/// terminal-free session (so the installer's `/dev/tty` probes fail and it
/// uses its documented non-interactive defaults), and an augmented PATH so
/// `curl`/`node`/`npm` resolve from a bundled `.app`.
pub fn run_invocation(invocation: &Invocation) -> io::Result<Output> {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .env("PATH", installer_path())
        .env("PI_SKIP_VERSION_CHECK", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    orbit_rpc::hide_console(&mut command);
    detach_terminal(&mut command);
    command.output()
}

/// Install pi with the official installer. On an unsupported platform this
/// returns `Unsupported` rather than shelling out to a mechanism that cannot
/// work.
pub fn run_official_installer() -> io::Result<Output> {
    let invocation = installer_invocation().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "the official pi installer supports macOS and Linux only",
        )
    })?;
    run_invocation(&invocation)
}

/// Update an existing managed install through pi's own updater.
pub fn run_pi_update() -> io::Result<Output> {
    let bin = orbit_rpc::pi_binary();
    let mut command = Command::new(&bin);
    command
        .env("PATH", orbit_rpc::augmented_path(Path::new(&bin).parent()))
        .env("PI_SKIP_VERSION_CHECK", "1")
        .args(["update", "--no-approve"]);
    orbit_rpc::hide_console(&mut command);
    command.output()
}

/// Refresh pi's provider/model catalog through pi's documented command
/// (`pi update --models`). A running pi process keeps the catalog it loaded at
/// startup, so the caller reloads it by restarting the agent afterwards.
pub fn run_model_catalog_refresh() -> io::Result<Output> {
    let bin = orbit_rpc::pi_binary();
    let mut command = Command::new(&bin);
    command
        .env("PATH", orbit_rpc::augmented_path(Path::new(&bin).parent()))
        .env("PI_SKIP_VERSION_CHECK", "1")
        .args(["update", "--models", "--no-approve"]);
    orbit_rpc::hide_console(&mut command);
    command.output()
}

/// PATH handed to the installer: the parent PATH plus the usual install dirs,
/// so `node` (and the `npm` the managed install uses internally) resolve from
/// a bundled `.app`.
fn installer_path() -> OsString {
    orbit_rpc::augmented_path(None)
}

/// Detach the child from the controlling terminal so the installer never
/// blocks on an interactive prompt. A GUI launched from Finder has no tty;
/// one launched from a terminal does, and the installer reads `/dev/tty`
/// directly. A new session has no controlling terminal.
#[cfg(unix)]
fn detach_terminal(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `setsid` is async-signal-safe. Failure (already a session
    // leader) is ignored — the child simply keeps the parent's terminal.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn detach_terminal(_command: &mut Command) {}

// ───────────────────────────────────────────────────────────────────────────
// Policy
// ───────────────────────────────────────────────────────────────────────────

/// Whether a fresh install should proceed, given what is already present.
///
/// Detecting first is what keeps Orbit from creating a second, competing pi.
pub fn plan_install(installed: &Installation) -> InstallOutcome {
    match installed.bin.as_deref() {
        None => InstallOutcome::Install,
        Some(bin) => match &installed.kind {
            InstallKind::Managed { .. } => InstallOutcome::AlreadyManaged,
            InstallKind::Npm { .. } | InstallKind::Other => {
                InstallOutcome::Conflict(bin.to_path_buf())
            }
        },
    }
}

/// Decide how to apply a newer release.
pub fn plan_update(installed: &Installation, latest: Option<&str>) -> UpdatePlan {
    let Some(current) = installed.version.as_deref() else {
        return if installed.installed() {
            UpdatePlan::Unsupported(UnsupportedReason::Unknown)
        } else {
            UpdatePlan::UpToDate
        };
    };
    let Some(latest) = latest else {
        return UpdatePlan::UpToDate;
    };
    if !crate::pi_update::is_newer(latest, current) {
        return UpdatePlan::UpToDate;
    }
    match &installed.kind {
        // The managed install pins its dependencies and updates itself.
        InstallKind::Managed { .. } => UpdatePlan::Update,
        // npm is not updated in place: migrate to the managed install, which
        // the official installer's non-interactive default does for us.
        InstallKind::Npm { .. } => UpdatePlan::Migrate,
        InstallKind::Other => UpdatePlan::Unsupported(UnsupportedReason::PackageManager),
    }
}

// The installer, launcher, and permission fixtures are POSIX; the whole
// suite is Unix-only, like the other fake-pi harnesses in this repo.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_dir(label: &str) -> PathBuf {
        let unique = format!(
            "orbit-pi-install-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_managed(root: &Path, version: &str) -> PathBuf {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join(MANAGED_MARKER),
            format!(
                r#"{{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1","entrypoint":{{"type":"symlink","path":"{}"}}}}"#,
                root.parent().unwrap().join("bin").join("pi").display()
            ),
        )
        .unwrap();
        fs::write(root.join("current-version"), format!("{version}\n")).unwrap();
        let launcher = root.parent().unwrap().join("bin").join("pi");
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        fs::write(&launcher, "#!/bin/sh\necho pi\n").unwrap();
        launcher
    }

    /// A realistic npm global layout: the package under `node_modules`, and
    /// the `<prefix>/bin/pi` launcher as a symlink into it (npm never writes a
    /// regular file there).
    fn write_npm_package(dir: &Path) -> PathBuf {
        let root = dir
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-coding-agent");
        fs::create_dir_all(root.join("dist").join("bundle")).unwrap();
        fs::write(
            root.join("package.json"),
            format!(r#"{{"name":"{PI_PACKAGE_NAME}","version":"0.99.2"}}"#),
        )
        .unwrap();
        let cli = root.join("dist").join("bundle").join("cli.js");
        fs::write(&cli, "#!/usr/bin/env node\n").unwrap();
        let launcher = dir.join("bin").join("pi");
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&cli, &launcher).unwrap();
        launcher
    }

    /// Build an [`Installation`] against a temp agent dir, since
    /// [`detect_from_bin`] classifies against the real `agent_dir()`.
    fn installation_at(bin: PathBuf, agent: &Path) -> Installation {
        let mut installation = Installation::not_installed();
        installation.kind = classify(&bin, agent);
        installation.bin = Some(bin);
        installation
    }

    fn write_script(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn official_invocation_uses_the_documented_script() {
        let inv = installer_invocation_for("macos", None, None).expect("macos supported");
        assert_eq!(inv.program, "sh");
        assert_eq!(
            inv.args,
            vec![
                "-c".to_string(),
                "curl -fsSL https://pi.dev/install.sh | sh".to_string()
            ]
        );
    }

    #[test]
    fn install_and_update_helpers_target_the_official_script() {
        // The default URL is the documented one, and the copyable command
        // matches it, so the UI hint and the executed command cannot drift.
        assert!(INSTALL_COMMAND.contains(INSTALL_SCRIPT_URL));
        assert_eq!(INSTALL_COMMAND, "curl -fsSL https://pi.dev/install.sh | sh");
    }

    #[test]
    fn unsupported_platforms_get_no_shell_installer() {
        assert!(installer_invocation_for("macos", None, None).is_some());
        assert!(installer_invocation_for("linux", None, None).is_some());
        assert!(installer_invocation_for("windows", None, None).is_none());
        assert!(installer_invocation_for("freebsd", None, None).is_none());
    }

    #[test]
    fn detection_separates_official_from_npm_and_other() {
        let dir = scratch_dir("detect");
        let agent = dir.join("agent");

        // Not installed.
        let missing = detect_from_bin(Some(dir.join("nowhere").join("pi")));
        assert!(matches!(missing.kind, InstallKind::Other));

        // Official managed install.
        let managed_launcher = write_managed(&agent.join("install"), "1.2.0");
        assert!(matches!(
            classify(&managed_launcher, &agent),
            InstallKind::Managed { .. }
        ));

        // npm global install in a separate tree.
        let npm_dir = dir.join("npm");
        let npm_launcher = write_npm_package(&npm_dir);
        assert!(matches!(
            classify(&npm_launcher, &agent),
            InstallKind::Npm { .. }
        ));

        // A hand-built shim belongs to neither.
        let other = dir.join("custom").join("pi");
        write_script(&other, "#!/bin/sh\necho pi\n");
        assert_eq!(classify(&other, &agent), InstallKind::Other);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn marker_validation_rejects_stray_directories() {
        let dir = scratch_dir("marker");
        assert!(!managed_marker_is_valid(&dir));

        fs::write(dir.join(MANAGED_MARKER), "not json").unwrap();
        assert!(!managed_marker_is_valid(&dir));

        fs::write(
            dir.join(MANAGED_MARKER),
            r#"{"kind":"something-else","schemaVersion":1,"layout":"releases-v1"}"#,
        )
        .unwrap();
        assert!(!managed_marker_is_valid(&dir));

        fs::write(
            dir.join(MANAGED_MARKER),
            r#"{"kind":"pi-managed-install","schemaVersion":2,"layout":"releases-v1"}"#,
        )
        .unwrap();
        assert!(!managed_marker_is_valid(&dir));

        fs::write(
            dir.join(MANAGED_MARKER),
            r#"{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1"}"#,
        )
        .unwrap();
        assert!(managed_marker_is_valid(&dir));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_install_prevents_a_second_managed_install() {
        assert_eq!(
            plan_install(&Installation::not_installed()),
            InstallOutcome::Install
        );

        let dir = scratch_dir("plan-install");
        let agent = dir.join("agent");
        let launcher = write_managed(&agent.join("install"), "1.2.0");
        let mut managed = installation_at(launcher, &agent);
        managed.version = Some("1.2.0".into());
        assert_eq!(plan_install(&managed), InstallOutcome::AlreadyManaged);

        let npm = installation_at(write_npm_package(&dir.join("npm")), &agent);
        assert!(matches!(plan_install(&npm), InstallOutcome::Conflict(_)));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_update_picks_the_mechanism_by_install_kind() {
        let mut managed = Installation::not_installed();
        managed.bin = Some(PathBuf::from("/tmp/pi"));
        managed.version = Some("1.0.0".into());
        managed.kind = InstallKind::Managed {
            root: PathBuf::from("/tmp/agent/install"),
        };
        assert_eq!(plan_update(&managed, Some("1.1.0")), UpdatePlan::Update);
        assert_eq!(plan_update(&managed, Some("1.0.0")), UpdatePlan::UpToDate);
        assert_eq!(plan_update(&managed, None), UpdatePlan::UpToDate);

        let mut npm = managed.clone();
        npm.kind = InstallKind::Npm {
            package_root: PathBuf::from("/tmp/node_modules/x"),
        };
        // npm is not updated in place: it must migrate to the official install.
        assert_eq!(plan_update(&npm, Some("1.1.0")), UpdatePlan::Migrate);
        assert_eq!(plan_update(&npm, Some("1.0.0")), UpdatePlan::UpToDate);

        let mut other = managed.clone();
        other.kind = InstallKind::Other;
        assert_eq!(
            plan_update(&other, Some("1.1.0")),
            UpdatePlan::Unsupported(UnsupportedReason::PackageManager)
        );
    }

    #[test]
    fn path_resolution_compares_real_targets() {
        let dir = scratch_dir("path");
        let real = dir.join("real").join("pi");
        write_script(&real, "#!/bin/sh\necho pi\n");
        let link = dir.join("bin").join("pi");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let real_canonical = fs::canonicalize(&real).unwrap();
        let path = std::env::join_paths([link.parent().unwrap()]).unwrap();
        assert!(resolves_on_path(&real_canonical, &path));

        let empty = std::env::join_paths([dir.join("elsewhere")]).unwrap();
        assert!(!resolves_on_path(&real_canonical, &empty));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_invocation_reports_success_and_failure() {
        let dir = scratch_dir("run");
        let marker = dir.join("ran");
        let ok = dir.join("ok.sh");
        write_script(
            &ok,
            &format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
        );
        let output = run_invocation(&Invocation {
            program: ok.to_string_lossy().into_owned(),
            args: vec![],
        })
        .expect("spawn");
        assert!(output.status.success());
        assert!(marker.exists(), "installer side effect ran");

        let bad = dir.join("bad.sh");
        write_script(&bad, "#!/bin/sh\necho 'Error: boom' >&2\nexit 7\n");
        let output = run_invocation(&Invocation {
            program: bad.to_string_lossy().into_owned(),
            args: vec![],
        })
        .expect("spawn");
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(
            crate::pi_update::failure_detail(&output.stdout, &output.stderr),
            "boom"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migration_via_the_fake_installer_keeps_user_data() {
        // The npm install is present; a fake installer stands in for the
        // official one and lays down the managed layout. Migration must not
        // touch the user's config, credentials, or sessions.
        let dir = scratch_dir("migrate");
        let agent = dir.join("agent");
        let npm_dir = dir.join("npm");
        write_npm_package(&npm_dir);

        // User data that must survive.
        fs::create_dir_all(agent.join("sessions")).unwrap();
        fs::write(
            agent.join("auth.json"),
            r#"{"deepseek":{"type":"api_key"}}"#,
        )
        .unwrap();
        fs::write(agent.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        fs::write(agent.join("sessions").join("s.jsonl"), "{}\n").unwrap();

        let installer = dir.join("fake-installer.sh");
        write_script(
            &installer,
            r#"#!/bin/sh
set -eu
agent="$1"
mkdir -p "$agent/install"
cat >"$agent/install/managed-install.json" <<'JSON'
{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1","entrypoint":{"type":"symlink","path":"$agent/bin/pi"}}
JSON
printf '1.3.0\n' >"$agent/install/current-version"
mkdir -p "$agent/bin"
printf '#!/bin/sh\necho 1.3.0\n' >"$agent/bin/pi"
chmod 755 "$agent/bin/pi"
exit 0
"#,
        );

        let output = run_invocation(&Invocation {
            program: installer.to_string_lossy().into_owned(),
            args: vec![agent.to_string_lossy().into_owned()],
        })
        .expect("spawn");
        assert!(output.status.success());

        // The managed install is recognized…
        let launcher = agent.join("bin").join("pi");
        assert!(matches!(
            classify(&launcher, &agent),
            InstallKind::Managed { .. }
        ));
        // …and the user's data is intact.
        assert!(agent.join("auth.json").is_file());
        assert!(agent.join("settings.json").is_file());
        assert!(agent.join("sessions").join("s.jsonl").is_file());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn version_of_reads_the_launcher() {
        let dir = scratch_dir("version");
        let pi = dir.join("pi");
        write_script(&pi, "#!/bin/sh\necho 9.9.9\n");
        assert_eq!(
            crate::onboarding::version_of(&pi),
            Some("9.9.9".to_string())
        );
        fs::remove_dir_all(&dir).ok();
    }
}
