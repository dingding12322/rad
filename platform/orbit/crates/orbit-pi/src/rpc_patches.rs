//! Auto-apply the RPC capability patches to the installed pi build.
//!
//! pi's RPC server lacks three capabilities Orbit uses: `ctx.ui.custom()`
//! (custom UI), `quota.*`, and `auth.*`. Each is a marker-delimited patch to
//! pi's bundled RPC chunk, shipped under `contrib/pi-*-rpc/` as an
//! `apply.mjs` script. Running them by hand before every launch — and again
//! after every `pi update`, which restores the pristine chunk — is the
//! footgun this module removes: it applies them just before Orbit spawns pi.
//!
//! The `contrib/` scripts stay the single source of truth. They are embedded
//! here with `include_str!` and materialized under `~/.orbit-pi/rpc-patches/`,
//! so the patch logic, markers, `.orbit-orig` backup, and revert keep living
//! in one place; this module only decides *when* to run them.
//!
//! The whole thing is best-effort: a missing `node`, a read-only global npm
//! install, or anchor drift after a `pi update` degrades to today's unpatched
//! behavior (custom UI absent, quota/auth fall back) rather than blocking
//! launch. A fast path skips the work entirely when the embedded sources and
//! the pi bundle are unchanged since the last successful apply.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CUSTOM_UI_APPLY: &str = include_str!("../../../contrib/pi-custom-ui-rpc/apply.mjs");
const CUSTOM_UI_HANDLER: &str =
    include_str!("../../../contrib/pi-custom-ui-rpc/custom-ui-handler.js");
const QUOTA_APPLY: &str = include_str!("../../../contrib/pi-quota-rpc/apply.mjs");
const QUOTA_HANDLER: &str = include_str!("../../../contrib/pi-quota-rpc/quota-handler.js");
const AUTH_APPLY: &str = include_str!("../../../contrib/pi-auth-rpc/apply.mjs");

/// How long a single `apply.mjs` run may take before it is killed. The scripts
/// only read a bundle and rewrite a string, so this is generous.
const PATCH_TIMEOUT: Duration = Duration::from_secs(20);

/// The two anchors every RPC-mode chunk carries. Used to locate the target
/// under `dist/bundle` without depending on pi's chunk filename, which hashes
/// with the build.
const ANCHOR_HELPERS: &str = "let handleCommand=async command=>{";
const ANCHOR_CASES: &str = "default:{let unknownCommand=command;return error(id,unknownCommand.type,`Unknown command: ${unknownCommand.type}`)}";

/// One embedded patch: its display name, its directory under
/// `~/.orbit-pi/rpc-patches/`, and the files the script reads next to itself.
struct Patch {
    name: &'static str,
    dir: &'static str,
    files: &'static [(&'static str, &'static str)],
}

const PATCHES: &[Patch] = &[
    Patch {
        name: "custom UI",
        dir: "custom-ui-rpc",
        files: &[
            ("apply.mjs", CUSTOM_UI_APPLY),
            ("custom-ui-handler.js", CUSTOM_UI_HANDLER),
        ],
    },
    Patch {
        name: "quota",
        dir: "quota-rpc",
        files: &[
            ("apply.mjs", QUOTA_APPLY),
            ("quota-handler.js", QUOTA_HANDLER),
        ],
    },
    Patch {
        name: "auth",
        dir: "auth-rpc",
        files: &[("apply.mjs", AUTH_APPLY)],
    },
];

/// What the last apply pass found, for the Settings → Runtime row.
#[derive(Debug, Clone)]
pub(crate) struct PatchReport {
    pub enabled: bool,
    /// The pi package root the patches target, once resolved. Surfaced in
    /// Settings so a saved path is visible and a stale one is obvious.
    pub package_root: Option<PathBuf>,
    /// Patches (re)applied on this launch.
    pub applied: Vec<&'static str>,
    /// Patches already present, skipped by the fast path.
    pub already: Vec<&'static str>,
    /// Why no patch ran, or which patch failed. `None` on the happy paths.
    pub error: Option<String>,
}

impl Default for PatchReport {
    fn default() -> Self {
        Self {
            enabled: true,
            package_root: None,
            applied: Vec::new(),
            already: Vec::new(),
            error: None,
        }
    }
}

impl PatchReport {
    /// A one-line human summary for the settings row.
    pub(crate) fn summary(&self) -> String {
        if let Some(error) = &self.error {
            return format!("Not applied — {error}");
        }
        if !self.enabled {
            return "Disabled".to_string();
        }
        if !self.applied.is_empty() {
            return format!("Applied on this launch: {}", self.applied.join(", "));
        }
        if !self.already.is_empty() {
            return "Active".to_string();
        }
        "Not applied".to_string()
    }
}

/// The persisted opt-in and the last resolved package root. A missing file
/// means enabled; `ORBIT_NO_RPC_PATCHES` in the environment wins over the file
/// so a single launch can opt out.
#[derive(Serialize, Deserialize)]
struct Config {
    #[serde(default = "default_true")]
    enabled: bool,
    /// The pi package root found by the last search. Saved so later launches
    /// skip the search; hand-edit to pin a nonstandard install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package_root: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            package_root: None,
        }
    }
}

fn default_true() -> bool {
    true
}

/// The identity of the pi bundle a successful apply was recorded against.
/// Size + mtime change when `pi update` (or a manual `--revert`) rewrites it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
struct TargetId {
    path: String,
    size: u64,
    mtime_ns: u128,
}

/// The fast-path memo written after a fully successful apply.
#[derive(Serialize, Deserialize, Default)]
struct AppliedState {
    /// Hash of the embedded patch sources. Changes when Orbit updates.
    fingerprint: u64,
    /// Every target chunk the scripts patched.
    targets: Vec<TargetId>,
}

/// Apply the RPC patches if needed. Called once, before pi is spawned.
pub(crate) fn apply_on_launch() -> PatchReport {
    let mut report = PatchReport {
        enabled: enabled(),
        ..Default::default()
    };
    if !report.enabled {
        return report;
    }

    let Some(base) = base_dir() else {
        report.error = Some("no home directory".to_string());
        return report;
    };
    let root = match resolved_package_root(&base) {
        Ok(root) => root,
        Err(error) => {
            report.error = Some(error);
            return report;
        }
    };
    let pi = PathBuf::from(orbit_rpc::pi_binary());
    report.package_root = Some(root.clone());

    let fingerprint = sources_fingerprint();
    // Fast path: nothing changed since the last successful apply, so neither
    // read the 4 MB bundle nor spawn node.
    if let Some(state) = read_state(&base) {
        if state.fingerprint == fingerprint
            && !state.targets.is_empty()
            && state.targets.iter().all(target_matches)
        {
            report.already = PATCHES.iter().map(|patch| patch.name).collect();
            return report;
        }
    }

    let Some(node) = crate::providers::node_binary() else {
        report.error = Some("node not found".to_string());
        return report;
    };

    let _lock = match Lock::acquire(&base) {
        Ok(lock) => lock,
        Err(error) => {
            report.error = Some(error);
            return report;
        }
    };

    let targets = find_targets(&root);
    if targets.is_empty() {
        report.error = Some("pi RPC bundle not found (layout changed?)".to_string());
        return report;
    }

    for patch in PATCHES {
        match run_patch(&node, &pi, &root, &base, patch) {
            Ok(()) => report.applied.push(patch.name),
            Err(error) => {
                report.error = Some(format!("{}: {error}", patch.name));
                break;
            }
        }
    }

    // Only memoize a clean pass. A failed patch must be retried next launch,
    // not cached as done.
    if report.error.is_none() {
        let state = AppliedState {
            fingerprint,
            targets: find_targets(&root)
                .iter()
                .filter_map(|path| target_id(path))
                .collect(),
        };
        write_state(&base, &state);
    }
    report
}

/// Whether auto-apply is enabled (env override, then the persisted file).
pub(crate) fn enabled() -> bool {
    if std::env::var_os("ORBIT_NO_RPC_PATCHES").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    base_dir()
        .map(|base| read_config(&base).enabled)
        .unwrap_or(true)
}

/// Persist the opt-in/opt-out, keeping the saved package root. Best-effort; a
/// write failure only means the next launch keeps the previous value.
pub(crate) fn set_enabled(enabled: bool) {
    let Some(base) = base_dir() else {
        return;
    };
    set_config_enabled(&base, enabled);
}

fn set_config_enabled(base: &Path, enabled: bool) {
    if fs::create_dir_all(base).is_err() {
        return;
    }
    let mut config = read_config(base);
    config.enabled = enabled;
    write_config(base, &config);
}

/// The pi package root for this launch.
///
/// Resolved from the live launcher first, so it follows `pi update` to the new
/// release dir; a saved path only fills in when no launcher can be resolved
/// (e.g. a bundled app launched without a `PATH`). Every fresh success is
/// persisted so the Settings row shows it and the fallback stays current.
fn resolved_package_root(base: &Path) -> Result<PathBuf, String> {
    match orbit_rpc::pi_package_root() {
        Ok(root) => {
            save_package_root(base, &root);
            Ok(root)
        }
        Err(error) => {
            if let Some(root) = read_config(base)
                .package_root
                .and_then(|saved| orbit_rpc::pi_package_root_from(Path::new(&saved)))
            {
                return Ok(root);
            }
            Err(error)
        }
    }
}

fn save_package_root(base: &Path, root: &Path) {
    let Some(root) = root.to_str() else {
        return;
    };
    let mut config = read_config(base);
    if config.package_root.as_deref() == Some(root) {
        return;
    }
    config.package_root = Some(root.to_string());
    if fs::create_dir_all(base).is_err() {
        return;
    }
    write_config(base, &config);
}

fn write_config(base: &Path, config: &Config) {
    let path = base.join("config.json");
    match serde_json::to_vec_pretty(config) {
        Ok(bytes) => {
            if let Err(error) = fs::write(&path, bytes) {
                eprintln!(
                    "Orbit rpc patches: could not write {}: {error}",
                    path.display()
                );
            }
        }
        Err(error) => eprintln!(
            "Orbit rpc patches: could not serialize {}: {error}",
            path.display()
        ),
    }
}

fn base_dir() -> Option<PathBuf> {
    Some(
        crate::platform::home_dir_opt()?
            .join(".orbit-pi")
            .join("rpc-patches"),
    )
}

/// Every `.js` under `dist/bundle` that carries the RPC-mode anchors.
fn find_targets(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk_js(&root.join("dist").join("bundle"), &mut found);
    found
}

fn walk_js(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_js(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "js")
            && fs::read_to_string(&path)
                .is_ok_and(|text| text.contains(ANCHOR_HELPERS) && text.contains(ANCHOR_CASES))
        {
            found.push(path);
        }
    }
}

fn target_id(path: &Path) -> Option<TargetId> {
    let meta = fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(TargetId {
        path: path.to_string_lossy().into_owned(),
        size: meta.len(),
        mtime_ns,
    })
}

fn target_matches(target: &TargetId) -> bool {
    target_id(Path::new(&target.path)).as_ref() == Some(target)
}

/// A stable-within-a-build hash over every embedded patch source. A change to
/// any script or handler forces one re-apply so the installed block refreshes.
fn sources_fingerprint() -> u64 {
    let mut hasher = DefaultHasher::new();
    for patch in PATCHES {
        patch.dir.hash(&mut hasher);
        for (name, contents) in patch.files {
            name.hash(&mut hasher);
            contents.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn state_path(base: &Path) -> PathBuf {
    base.join("state.json")
}

fn read_state(base: &Path) -> Option<AppliedState> {
    serde_json::from_str(&fs::read_to_string(state_path(base)).ok()?).ok()
}

fn write_state(base: &Path, state: &AppliedState) {
    let path = state_path(base);
    match serde_json::to_vec_pretty(state) {
        Ok(bytes) => {
            if let Err(error) = fs::write(&path, bytes) {
                eprintln!(
                    "Orbit rpc patches: could not write {}: {error}",
                    path.display()
                );
            }
        }
        Err(error) => eprintln!(
            "Orbit rpc patches: could not serialize {}: {error}",
            path.display()
        ),
    }
}

fn read_config(base: &Path) -> Config {
    fs::read_to_string(base.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Materialize one patch's scripts and run its `apply.mjs` against pi.
fn run_patch(node: &str, pi: &Path, root: &Path, base: &Path, patch: &Patch) -> Result<(), String> {
    let dir = base.join(patch.dir);
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    for (name, contents) in patch.files {
        write_if_changed(&dir.join(name), contents).map_err(|error| error.to_string())?;
    }

    let mut command = Command::new(node);
    command
        .arg(dir.join("apply.mjs"))
        .current_dir(&dir)
        // The scripts resolve pi from `PI_BIN`, else `command -v pi`, and the
        // package from `PI_PACKAGE_ROOT`, else their own walk. Pin both to what
        // Orbit resolved, so the managed-install layout works everywhere.
        .env(orbit_rpc::PI_BIN_ENV, pi)
        .env(orbit_rpc::PI_PACKAGE_ROOT_ENV, root)
        .env("PATH", orbit_rpc::augmented_path(pi.parent()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    orbit_rpc::hide_console(&mut command);
    run_with_timeout(command)
}

/// Run a command to completion, capturing stderr for the error path. Node's
/// startup is quick; a hang is killed at [`PATCH_TIMEOUT`].
fn run_with_timeout(mut command: Command) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run node: {error}"))?;
    let stdout = child.stdout.take().map(|pipe| {
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = pipe.take(64 * 1024).read_to_string(&mut sink);
            sink
        })
    });
    let stderr = child.stderr.take().map(|pipe| {
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = pipe.take(64 * 1024).read_to_string(&mut sink);
            sink
        })
    });

    let deadline = Instant::now() + PATCH_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let _ = stdout.and_then(|handle| handle.join().ok());
    let stderr = stderr
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    if status.success() {
        Ok(())
    } else if stderr.trim().is_empty() {
        Err(format!("exit {status}"))
    } else {
        Err(stderr.trim().to_string())
    }
}

/// Write only when the bytes differ, so a re-launch does not churn mtimes.
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    fs::write(path, contents)
}

/// A best-effort cross-instance lock. A stale lock (older than ten minutes,
/// e.g. a crashed launch) is broken rather than blocking forever.
struct Lock {
    path: PathBuf,
}

impl Lock {
    fn acquire(base: &Path) -> Result<Self, String> {
        fs::create_dir_all(base).map_err(|error| error.to_string())?;
        let path = base.join("lock");
        for attempt in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(600));
                    if stale && attempt == 0 {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    return Err("another Orbit instance is applying patches".to_string());
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("another Orbit instance is applying patches".to_string())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir() -> PathBuf {
        let unique = format!(
            "orbit-rpc-patches-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_chunks_by_anchor_not_name() {
        let dir = scratch_dir();
        let bundle = dir.join("dist").join("bundle").join("chunks");
        fs::create_dir_all(&bundle).unwrap();
        let anchor = format!("{ANCHOR_HELPERS}...{ANCHOR_CASES}...");
        fs::write(bundle.join("chunk-abc.js"), &anchor).unwrap();
        fs::write(bundle.join("other.js"), "no anchors here").unwrap();
        fs::write(bundle.join("readme.md"), &anchor).unwrap();

        let found = find_targets(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("chunk-abc.js"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_round_trips_enabled_and_saved_package_root() {
        let dir = scratch_dir();
        assert!(read_config(&dir).enabled, "missing config means enabled");
        assert_eq!(read_config(&dir).package_root, None);

        fs::write(dir.join("config.json"), r#"{"enabled":false}"#).unwrap();
        assert!(!read_config(&dir).enabled);

        fs::write(dir.join("config.json"), "not json").unwrap();
        assert!(read_config(&dir).enabled, "garbage falls back to enabled");

        // A discovered root is saved, and toggling the opt-in keeps it.
        let root = "/opt/pi/node_modules/@earendil-works/pi-coding-agent";
        save_package_root(&dir, Path::new(root));
        assert_eq!(read_config(&dir).package_root.as_deref(), Some(root));

        set_config_enabled(&dir, false);
        let config = read_config(&dir);
        assert!(!config.enabled);
        assert_eq!(config.package_root.as_deref(), Some(root));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_round_trips_and_matches_by_identity() {
        let dir = scratch_dir();
        let bundle = dir.join("dist").join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        let chunk = bundle.join("chunk.js");
        fs::write(&chunk, "x").unwrap();

        let state = AppliedState {
            fingerprint: 42,
            targets: vec![target_id(&chunk).unwrap()],
        };
        write_state(&dir, &state);
        let loaded = read_state(&dir).unwrap();
        assert_eq!(loaded.fingerprint, 42);
        assert!(loaded.targets.iter().all(target_matches));

        // Rewriting the chunk invalidates the memo.
        fs::write(&chunk, "changed content").unwrap();
        assert!(!loaded.targets.iter().all(target_matches));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lock_is_exclusive_and_releases_on_drop() {
        let dir = scratch_dir();
        let first = Lock::acquire(&dir).expect("first lock");
        assert!(Lock::acquire(&dir).is_err(), "second acquire is refused");
        drop(first);
        assert!(Lock::acquire(&dir).is_ok(), "drop releases the lock");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn embedded_scripts_are_the_real_contrib_patches() {
        // The included sources really are the marker-delimited patch scripts.
        assert!(CUSTOM_UI_APPLY.contains("orbit-custom-ui-rpc:begin"));
        assert!(QUOTA_APPLY.contains("orbit-quota-rpc:begin"));
        assert!(AUTH_APPLY.contains("orbit-auth-rpc:begin"));
        assert!(CUSTOM_UI_HANDLER.contains("capabilities"));
        assert!(QUOTA_HANDLER.contains("quota.list"));
    }
}
