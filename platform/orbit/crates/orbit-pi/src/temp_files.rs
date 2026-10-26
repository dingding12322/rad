//! Scratch files in the system temp folder.
//!
//! The app groups its scratch files under `orbit-pi/` inside
//! [`std::env::temp_dir`] — bug-report screenshots, update downloads, and
//! helper scripts. Settings → Privacy lists that folder's contents and can
//! remove them, mirroring the session-retention controls. Entries a live Orbit
//! process may still be using are listed but never removed.
//!
//! Older Orbit versions wrote `orbit-*` entries directly into the temp root;
//! [`TempFiles::scan`] includes them so one cleanup pass can clear the
//! leftovers. The pi agent writes its own `pi-*` scratch files directly into
//! the temp root; those are listed separately and only removed after
//! [`AI_SCRATCH_GRACE`] without activity.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The folder Orbit owns inside the system temp directory.
const APP_TEMP_DIR: &str = "orbit-pi";

/// Legacy entries written directly into the temp root all start with this.
const LEGACY_TEMP_PREFIX: &str = "orbit-";

/// pi's own scratch files, all written directly into the temp root.
const AI_TEMP_PREFIXES: &[&str] = &[
    "pi-bash-",
    "pi-powershell-",
    "pi-mcp-",
    "pi-codemode-",
    "pi-editor-",
    "pi-share-",
    "pi-output-",
    "pi-tui-crash.log",
    "pi-tui-debug.log",
];

/// Keep AI scratch files touched this recently: a live pi run may still be
/// writing to its shell log or referencing a codemode image.
const AI_SCRATCH_GRACE: Duration = Duration::from_secs(60 * 60);

/// The most directory depth [`path_size`] walks. Temp entries are shallow by
/// design; the guard only keeps a stray symlink loop or a huge tree cheap.
const MAX_SIZE_DEPTH: usize = 4;

/// One file or directory Orbit left in the temp folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub bytes: u64,
    pub modified: SystemTime,
    /// Cleanup keeps this entry: a live process may need it, or an AI scratch
    /// file was touched recently.
    pub active: bool,
}

/// A snapshot of the temp folder, taken when Settings → Privacy opens and
/// after a cleanup. Rendering never touches the filesystem.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TempFiles {
    dir: PathBuf,
    entries: Vec<TempEntry>,
    ai_entries: Vec<TempEntry>,
}

/// The system temp directory Orbit and pi both write into.
pub fn root() -> PathBuf {
    std::env::temp_dir()
}

/// Orbit's dedicated temp folder: `<system temp>/orbit-pi`.
pub fn dir() -> PathBuf {
    root().join(APP_TEMP_DIR)
}

/// Create [`dir`] and return it, so a producer never writes a relative path
/// when the system temp directory is unavailable.
pub fn ensure_dir() -> std::io::Result<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl TempFiles {
    /// Scan the dedicated temp folder plus older `orbit-*` entries in the temp
    /// root, and pi's own `pi-*` scratch files.
    pub fn scan() -> Self {
        let root = root();
        let dir = root.join(APP_TEMP_DIR);
        Self::scan_in(&root, &dir)
    }

    fn scan_in(root: &Path, dir: &Path) -> Self {
        let mut entries = Vec::new();
        let mut ai_entries = Vec::new();
        if let Ok(children) = std::fs::read_dir(dir) {
            for child in children.flatten() {
                if let Some(entry) = read_entry(&child, false) {
                    entries.push(entry);
                }
            }
        }
        if let Ok(children) = std::fs::read_dir(root) {
            for child in children.flatten() {
                // The dedicated folder is ours already; don't list it as a
                // legacy entry or count its contents twice.
                if child.path() == dir {
                    continue;
                }
                let Ok(name) = child.file_name().into_string() else {
                    continue;
                };
                if name.starts_with(LEGACY_TEMP_PREFIX) {
                    if let Some(entry) = read_entry(&child, false) {
                        entries.push(entry);
                    }
                } else if AI_TEMP_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                {
                    if let Some(entry) = read_entry(&child, true) {
                        ai_entries.push(entry);
                    }
                }
            }
        }
        sort_newest_first(&mut entries);
        sort_newest_first(&mut ai_entries);
        Self {
            dir: dir.to_path_buf(),
            entries,
            ai_entries,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn entries(&self) -> &[TempEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.bytes).sum()
    }

    pub fn ai_entries(&self) -> &[TempEntry] {
        &self.ai_entries
    }

    pub fn ai_is_empty(&self) -> bool {
        self.ai_entries.is_empty()
    }

    pub fn ai_len(&self) -> usize {
        self.ai_entries.len()
    }

    pub fn ai_total_bytes(&self) -> u64 {
        self.ai_entries.iter().map(|entry| entry.bytes).sum()
    }
}

fn sort_newest_first(entries: &mut [TempEntry]) {
    entries.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| b.path.cmp(&a.path))
    });
}

fn read_entry(child: &std::fs::DirEntry, ai_scratch: bool) -> Option<TempEntry> {
    let name = child.file_name().into_string().ok()?;
    if name.is_empty() {
        return None;
    }
    let metadata = std::fs::symlink_metadata(child.path()).ok()?;
    let is_dir = metadata.is_dir();
    let bytes = if is_dir {
        path_size(&child.path(), 0)
    } else {
        metadata.len()
    };
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let recent = ai_scratch
        && SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            < AI_SCRATCH_GRACE;
    Some(TempEntry {
        path: child.path(),
        active: recent || is_active(&name),
        name,
        is_dir,
        bytes,
        modified,
    })
}

/// What a cleanup pass did, for Settings' toast.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanOutcome {
    pub removed: usize,
    pub bytes: u64,
    pub skipped_active: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

/// Remove every Orbit temp entry in the snapshot except those a live process
/// may still be using. A failed removal is reported, never fatal.
pub fn clean(files: &TempFiles) -> CleanOutcome {
    clean_entries(&files.entries)
}

/// Remove pi's older scratch entries; recent files are kept so a live run is
/// never disturbed.
pub fn clean_ai(files: &TempFiles) -> CleanOutcome {
    clean_entries(&files.ai_entries)
}

fn clean_entries(entries: &[TempEntry]) -> CleanOutcome {
    let mut outcome = CleanOutcome::default();
    for entry in entries {
        if entry.active {
            outcome.skipped_active.push(entry.path.clone());
            continue;
        }
        match remove_entry(&entry.path) {
            Ok(()) => {
                outcome.removed += 1;
                outcome.bytes += entry.bytes;
            }
            // The snapshot is older than the click by however long the page
            // stayed open. Something already gone is not a failure, and freed
            // nothing, so it counts as removed without adding to the total.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                outcome.removed += 1;
            }
            Err(error) => outcome.failed.push((entry.path.clone(), error.to_string())),
        }
    }
    outcome
}

/// Whether `name` belongs to this running app (or a live Orbit process on
/// Unix). Update staging and login helper names embed a process id.
fn is_active(name: &str) -> bool {
    let Some(pid) = embedded_pid(name) else {
        return false;
    };
    pid == std::process::id() || process_alive(pid)
}

fn embedded_pid(name: &str) -> Option<u32> {
    // Current layout: `<temp>/orbit-pi/update-<pid>-<nonce>` and
    // `<temp>/orbit-pi/login-<pid>.command`.
    if let Some(rest) = name.strip_prefix("update-") {
        return rest.split('-').next()?.parse().ok();
    }
    if let Some(rest) = name.strip_prefix("login-") {
        return rest.strip_suffix(".command")?.parse().ok();
    }
    // Legacy layout: `<temp>/orbit-update-<pid>-<nonce>` and
    // `<temp>/orbit-<pid>.command`.
    if let Some(rest) = name.strip_prefix("orbit-update-") {
        return rest.split('-').next()?.parse().ok();
    }
    if let Some(rest) = name.strip_prefix(LEGACY_TEMP_PREFIX) {
        if let Some(pid) = rest.strip_suffix(".command") {
            return pid.parse().ok();
        }
    }
    None
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // `kill` takes a signed pid; 0 and out-of-range values address process
    // groups instead of a single process, so they are never "alive" here.
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // Signal 0 probes for existence without touching the process. `EPERM`
    // still means the process exists; the caller simply may not signal it.
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    // Windows has no dependency-free liveness probe here. The current
    // process's own entries are still protected by the id check above.
    false
}

fn remove_entry(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Best-effort recursive byte count. Symlinks are counted as links, never
/// followed, and unreadable children are skipped.
fn path_size(path: &Path, depth: usize) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return metadata.len();
    }
    if depth >= MAX_SIZE_DEPTH {
        return 0;
    }
    let Ok(children) = std::fs::read_dir(path) else {
        return 0;
    };
    children
        .flatten()
        .map(|child| path_size(&child.path(), depth + 1))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "orbit-temp-files-test-{}-{nanos}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_dir(root: &Path) -> PathBuf {
        root.join(APP_TEMP_DIR)
    }

    #[test]
    fn scan_lists_the_app_folder_and_legacy_entries() {
        let root = scratch();
        let dir = app_dir(&root);
        std::fs::create_dir_all(dir.join("bug-report-1")).unwrap();
        std::fs::write(dir.join("bug-report-1").join("a.png"), b"12345").unwrap();
        std::fs::write(dir.join("update-1-0.log"), b"up").unwrap();
        std::fs::write(root.join("orbit-legacy.log"), b"old").unwrap();
        std::fs::write(root.join("other-app.log"), b"noise").unwrap();

        let files = TempFiles::scan_in(&root, &dir);
        let mut names: Vec<&str> = files.entries().iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["bug-report-1", "orbit-legacy.log", "update-1-0.log"]
        );
        assert!(files
            .entries()
            .iter()
            .any(|e| e.name == "bug-report-1" && e.is_dir && e.bytes == 5));
        assert_eq!(files.total_bytes(), 10);
        // The dedicated folder itself is never listed as a legacy entry.
        assert!(!files.entries().iter().any(|e| e.name == APP_TEMP_DIR));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn clean_removes_app_and_legacy_entries_but_keeps_foreign_files() {
        let root = scratch();
        let dir = app_dir(&root);
        std::fs::create_dir_all(dir.join("bug-report-1")).unwrap();
        std::fs::write(dir.join("bug-report-1").join("a.png"), b"img").unwrap();
        // A pid that cannot be live keeps this entry cleanable regardless of
        // the machine the test runs on.
        std::fs::write(dir.join("update-4294967295-0.log"), b"up").unwrap();
        std::fs::write(root.join("orbit-legacy.log"), b"old").unwrap();
        std::fs::write(root.join("keep.txt"), b"keep").unwrap();

        let files = TempFiles::scan_in(&root, &dir);
        let outcome = clean(&files);

        assert_eq!(outcome.removed, 3);
        assert_eq!(outcome.bytes, 8);
        assert!(!dir.join("bug-report-1").exists());
        assert!(!dir.join("update-4294967295-0.log").exists());
        assert!(!root.join("orbit-legacy.log").exists());
        assert!(root.join("keep.txt").exists());
        // The app folder itself stays for the next write.
        assert!(dir.is_dir());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn clean_keeps_entries_this_process_is_using() {
        let root = scratch();
        let dir = app_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join(format!("login-{}.command", std::process::id()));
        let legacy = root.join(format!("orbit-{}.command", std::process::id()));
        std::fs::write(&current, b"#!/bin/zsh").unwrap();
        std::fs::write(&legacy, b"#!/bin/zsh").unwrap();
        std::fs::write(dir.join("bug-report-1"), b"shot").unwrap();

        let files = TempFiles::scan_in(&root, &dir);
        assert_eq!(files.entries().iter().filter(|e| e.active).count(), 2);
        let outcome = clean(&files);

        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.skipped_active.len(), 2);
        assert!(current.exists());
        assert!(legacy.exists());
        assert!(!dir.join("bug-report-1").exists());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scan_separates_ai_scratch_files_from_orbit_entries() {
        let root = scratch();
        let dir = app_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.join("orbit-legacy.log"), b"old").unwrap();
        std::fs::write(root.join("pi-bash-abc.log"), b"123").unwrap();
        std::fs::create_dir_all(root.join("pi-share-abc")).unwrap();
        std::fs::write(root.join("pi-share-abc").join("session.html"), b"12").unwrap();
        // A file that merely shares the `pi-` prefix but is not pi scratch.
        std::fs::write(root.join("pi-logo.png"), b"noise").unwrap();
        std::fs::write(root.join("other-app.log"), b"noise").unwrap();

        let files = TempFiles::scan_in(&root, &dir);
        let mut orbit: Vec<&str> = files.entries().iter().map(|e| e.name.as_str()).collect();
        orbit.sort_unstable();
        assert_eq!(orbit, ["orbit-legacy.log"]);
        let mut ai: Vec<&str> = files.ai_entries().iter().map(|e| e.name.as_str()).collect();
        ai.sort_unstable();
        assert_eq!(ai, ["pi-bash-abc.log", "pi-share-abc"]);
        assert_eq!(files.ai_total_bytes(), 5);

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ai_cleanup_keeps_recent_files_and_removes_old_ones() {
        let root = scratch();
        let dir = app_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let recent = root.join("pi-bash-recent.log");
        let old = root.join("pi-bash-old.log");
        std::fs::write(&recent, b"new").unwrap();
        std::fs::write(&old, b"old").unwrap();
        set_modified(&old, Duration::from_secs(2 * 60 * 60));

        let files = TempFiles::scan_in(&root, &dir);
        assert_eq!(files.ai_entries().iter().filter(|e| e.active).count(), 1);
        let outcome = clean_ai(&files);

        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.skipped_active, vec![recent.clone()]);
        assert!(recent.exists());
        assert!(!old.exists());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn clean_treats_a_vanished_entry_as_removed() {
        let root = scratch();
        let dir = app_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let gone = dir.join("bug-report-1");
        std::fs::write(&gone, b"shot").unwrap();

        let files = TempFiles::scan_in(&root, &dir);
        std::fs::remove_file(&gone).unwrap();
        let outcome = clean(&files);

        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.bytes, 0);
        assert!(outcome.failed.is_empty());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn embedded_pids_only_come_from_update_and_login_names() {
        assert_eq!(embedded_pid("update-42-9"), Some(42));
        assert_eq!(embedded_pid("login-42.command"), Some(42));
        assert_eq!(embedded_pid("orbit-update-42-9"), Some(42));
        assert_eq!(embedded_pid("orbit-42.command"), Some(42));
        assert_eq!(embedded_pid("bug-report-42"), None);
        assert_eq!(embedded_pid("other-42.command"), None);
    }

    fn set_modified(path: &Path, age: Duration) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let when = SystemTime::now() - age;
        let times = std::fs::FileTimes::new()
            .set_modified(when)
            .set_accessed(when);
        file.set_times(times).unwrap();
    }
}
