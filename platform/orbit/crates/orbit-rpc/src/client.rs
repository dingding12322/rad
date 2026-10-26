//! Process lifecycle + JSONL transport for the pi CLI RPC protocol.
//!
//! One `PiClient` owns one `pi --mode rpc` child process:
//! - a **writer thread** drains queued commands onto stdin (JSON lines)
//! - a **reader thread** parses stdout lines into [`Event`]s, forwarding them
//!   to the shared event stream and routing `response` events by `id` to the
//!   `Receiver` each `send_command` returns
//! - a **stderr drain thread** keeps a capped ring of recent stderr lines so a
//!   chatty child can never block on a full pipe
//!
//! The protocol mandates strict LF framing; we use `read_until(b'\n')` on the
//! byte level so U+2028/U+2029 inside JSON strings are never treated as line
//! breaks (`std::io::BufRead::lines` is safe too, but byte-exact is explicit).

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Stdio},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context as _, Result};

use crate::types::{CommandBody, Event};

/// Environment override for the pi binary (default: `pi` on PATH).
pub const PI_BIN_ENV: &str = "PI_BIN";
/// Environment override for the pi *package root* — the directory holding
/// pi's `package.json`. The RPC patcher sets it so its bundled `apply.mjs`
/// scripts skip their own layout-dependent search.
pub const PI_PACKAGE_ROOT_ENV: &str = "PI_PACKAGE_ROOT";
/// The npm package name that identifies a pi install, and the directory it
/// occupies under `node_modules`.
const PI_PACKAGE_NAME: &str = "@earendil-works/pi-coding-agent";
/// Cap on retained stderr lines (dropped oldest first).
const STDERR_RING_CAP: usize = 200;

/// Absolute install locations probed for `pi` when it isn't on `PATH`. `pi` is
/// a Node script installed via Homebrew, so the launcher and `node` both live
/// under these `bin` dirs. A bundled `.app` is launched with a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), so we probe these to keep packaged
/// builds working without the user exporting anything.
const PI_SEARCH_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/opt/local/bin",
    "/usr/bin",
    "/bin",
];
/// Home-relative install dirs probed for `pi` when it isn't on `PATH` or in
/// [`PI_SEARCH_DIRS`]. `pi` ships via npm/pnpm/bun-style installs that put the
/// shim under the user's home (e.g. `~/.local/bin/pi`), which a bundled `.app`
/// never sees on its minimal PATH. Mirrors the onboarding dependency probe.
const HOME_SEARCH_DIRS: &[&str] = &[
    ".local/bin",
    ".volta/bin",
    ".local/share/mise/shims",
    ".asdf/shims",
    "Library/pnpm/bin",
    ".bun/bin",
    ".npm-global/bin",
    // pi's own shim dir; a launcher installed there is on the shell PATH but
    // not a bundled app's.
    ".pi/agent/bin",
    ".yarn/bin",
];
/// Dirs prepended to the child's PATH so `node` (and `pi`) resolve from a
/// bundled app launch.
const PATH_EXTRA_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];

/// The pi launcher names to probe, most specific first.
///
/// Windows npm installs expose `pi` as a `.cmd`/`.ps1` shim, never a real
/// `pi.exe`, so probing only `pi.exe` can never find a standard npm install.
/// Rust's `Command` runs `.cmd`/`.bat` through the command interpreter itself,
/// so the shim is spawned directly.
#[cfg(windows)]
const PI_BIN_NAMES: &[&str] = &["pi.exe", "pi.cmd", "pi.bat", "pi.ps1"];
#[cfg(not(windows))]
const PI_BIN_NAMES: &[&str] = &["pi"];

pub struct PiClient {
    child: Child,
    commands_tx: Sender<Outgoing>,
    events_rx: Receiver<Event>,
    pending: Arc<Mutex<HashMap<String, Sender<Event>>>>,
    stderr_ring: Arc<Mutex<VecDeque<String>>>,
    next_id: std::sync::atomic::AtomicU64,
}

struct Outgoing {
    wire: String,
}

impl PiClient {
    /// Spawn `pi --mode rpc` rooted at `workspace_dir`.
    ///
    /// `session_dir: None` uses pi's default storage (`~/.pi/agent/sessions/`)
    /// so sessions created here are the same ones the CLI sees. Pass an
    /// explicit dir to isolate (tests, throwaway demos).
    pub fn spawn(workspace_dir: &Path, session_dir: Option<&Path>) -> Result<Self> {
        Self::spawn_with_bin(&resolve_pi_bin(), workspace_dir, session_dir)
    }

    /// Spawn with extra extension files loaded via pi's `--extension` flag.
    ///
    /// Orbit uses this to load its bundled quota bridge without installing a
    /// package or writing settings: pi discovers the file at startup and the
    /// path travels with the app, so an update can never leave a stale entry
    /// behind in `~/.pi/agent/settings.json`.
    pub fn spawn_with_extensions(
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        extensions: &[PathBuf],
    ) -> Result<Self> {
        Self::spawn_with_bin_and_extensions(
            &resolve_pi_bin(),
            workspace_dir,
            session_dir,
            extensions,
        )
    }

    /// Spawn with extra startup flags appended to the base `--mode rpc
    /// --approve` argv. Orbit uses this for the default session model
    /// (`--provider` / `--model`).
    pub fn spawn_with_args(
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        args: &[String],
    ) -> Result<Self> {
        Self::spawn_with_bin_and_args(&resolve_pi_bin(), workspace_dir, session_dir, args)
    }

    /// [`spawn_with_extensions`](Self::spawn_with_extensions) plus extra
    /// startup flags (see [`spawn_with_args`](Self::spawn_with_args)).
    pub fn spawn_with_extensions_and_args(
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        extensions: &[PathBuf],
        args: &[String],
    ) -> Result<Self> {
        Self::spawn_inner(
            &resolve_pi_bin(),
            workspace_dir,
            session_dir,
            extensions,
            &[],
            args,
        )
    }

    /// Spawn with extra extension files **and** process-scoped environment
    /// variables. The variables apply to this child only; they never touch
    /// persisted state or the active session.
    pub fn spawn_with_extensions_and_env(
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        extensions: &[PathBuf],
        env: &[(&str, &str)],
    ) -> Result<Self> {
        Self::spawn_inner(
            &resolve_pi_bin(),
            workspace_dir,
            session_dir,
            extensions,
            env,
            &[],
        )
    }

    /// [`spawn_with_extensions_and_env`](Self::spawn_with_extensions_and_env)
    /// plus extra startup flags — the seam Orbit uses to inject MCP secret
    /// values (`${NAME}` references in `mcp.json`) together with the default
    /// session model.
    pub fn spawn_with_extensions_and_env_and_args(
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        extensions: &[PathBuf],
        env: &[(&str, &str)],
        args: &[String],
    ) -> Result<Self> {
        Self::spawn_inner(
            &resolve_pi_bin(),
            workspace_dir,
            session_dir,
            extensions,
            env,
            args,
        )
    }

    /// [`spawn_with_extensions`](Self::spawn_with_extensions) against a
    /// specific executable — the seam the transport tests drive.
    pub fn spawn_with_bin_and_extensions(
        bin: &str,
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        extensions: &[PathBuf],
    ) -> Result<Self> {
        Self::spawn_with_bin_and_extensions_and_args(
            bin,
            workspace_dir,
            session_dir,
            extensions,
            &[],
        )
    }

    /// [`spawn_with_bin_and_extensions`](Self::spawn_with_bin_and_extensions)
    /// plus extra startup flags. Test seam for the default session model.
    pub fn spawn_with_bin_and_extensions_and_args(
        bin: &str,
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        extensions: &[PathBuf],
        args: &[String],
    ) -> Result<Self> {
        Self::spawn_inner(bin, workspace_dir, session_dir, extensions, &[], args)
    }

    /// Spawn a specific executable as the RPC server. [`spawn`](Self::spawn)
    /// resolves the real `pi`; this seam lets tests drive the transport with a
    /// scripted server and lets callers target an alternate pi build.
    pub fn spawn_with_bin(
        bin: &str,
        workspace_dir: &Path,
        session_dir: Option<&Path>,
    ) -> Result<Self> {
        Self::spawn_with_bin_and_args(bin, workspace_dir, session_dir, &[])
    }

    /// [`spawn_with_bin`](Self::spawn_with_bin) plus extra startup flags.
    pub fn spawn_with_bin_and_args(
        bin: &str,
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        args: &[String],
    ) -> Result<Self> {
        Self::spawn_inner(bin, workspace_dir, session_dir, &[], &[], args)
    }

    fn spawn_inner(
        bin: &str,
        workspace_dir: &Path,
        session_dir: Option<&Path>,
        extensions: &[PathBuf],
        env: &[(&str, &str)],
        extra_args: &[String],
    ) -> Result<Self> {
        let mut command = std::process::Command::new(bin);
        // `--approve` grants *project trust* (load project-local settings,
        // extensions, and skills) — it is not tool-call approval. This is the
        // isolated transport seam used by tests and callers that don't pass
        // extensions; the app spawns through `BundledExtensions`, which loads
        // the access guard that confirms tool calls per the active mode.
        command
            .args(["--mode", "rpc", "--approve"])
            // The default session model lands here (`--provider` / `--model`).
            // Empty for every existing caller, so argv is unchanged unless a
            // default is configured.
            .args(extra_args)
            .env("PI_SKIP_VERSION_CHECK", "1")
            // Augment PATH with Homebrew-style dirs so `node` (required by
            // pi's `#!/usr/bin/env node` shebang) resolves when launched from
            // a bundled `.app`. The resolved pi's own dir is included too, so
            // nvm/volta/mise installs find the `node` sitting beside it.
            .env("PATH", augmented_path(Path::new(bin).parent()))
            .current_dir(workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        if let Some(dir) = session_dir {
            command.arg("--session-dir").arg(dir);
        }
        for extension in extensions {
            command.arg("--extension").arg(extension);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn `{bin} --mode rpc`"))?;

        let stdin = child.stdin.take().context("pi stdin unavailable")?;
        let stdout = child.stdout.take().context("pi stdout unavailable")?;
        let stderr = child.stderr.take().context("pi stderr unavailable")?;

        let (commands_tx, commands_rx) = mpsc::channel::<Outgoing>();
        let (events_tx, events_rx) = mpsc::channel::<Event>();
        let pending = Arc::new(Mutex::new(HashMap::<String, Sender<Event>>::new()));
        let stderr_ring: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

        Self::spawn_writer(stdin, commands_rx);
        Self::spawn_reader(stdout, &events_tx, &pending);
        Self::spawn_stderr_drain(stderr, &stderr_ring);

        Ok(Self {
            child,
            commands_tx,
            events_rx,
            pending,
            stderr_ring,
            next_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Send a command and get the receiver for its matched `response` event.
    pub fn send(&self, body: CommandBody) -> Result<Receiver<Event>> {
        let id = self.next_id_fetch();
        let command = crate::types::Command::new(id.clone(), body);
        let wire = command.to_wire()?;
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, tx);
        self.commands_tx
            .send(Outgoing { wire })
            .map_err(|_| anyhow!("pi process is not running"))?;
        Ok(rx)
    }

    /// Convenience for commands whose response we inspect immediately.
    pub fn send_blocking(&self, body: CommandBody, timeout: std::time::Duration) -> Result<Event> {
        let rx = self.send(body)?;
        rx.recv_timeout(timeout)
            .map_err(|e| anyhow!("no response within {timeout:?}: {e}"))
    }

    /// The shared stream of all events (responses included).
    pub fn events(&self) -> &Receiver<Event> {
        &self.events_rx
    }

    /// Drain all events currently buffered on the shared stream.
    pub fn drain_events(&self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(event) = self.events_rx.try_recv() {
            out.push(event);
        }
        out
    }

    /// Drain recent stderr lines written by pi.
    pub fn drain_stderr(&self) -> Vec<String> {
        let mut ring = self
            .stderr_ring
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ring.drain(..).collect()
    }

    /// The most recent stderr lines (newest last), without draining them.
    pub fn recent_stderr(&self, limit: usize) -> Vec<String> {
        let ring = self
            .stderr_ring
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ring.iter().rev().take(limit).rev().cloned().collect()
    }

    pub fn child_pid(&self) -> u32 {
        self.child.id()
    }

    /// True while the pi process is still running.
    pub fn is_alive(&mut self) -> bool {
        self.child.try_wait().map(|s| s.is_none()).unwrap_or(false)
    }

    fn next_id_fetch(&self) -> String {
        let n = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        format!("orbit-{n}-{nanos:x}")
    }

    fn spawn_writer(mut stdin: ChildStdin, commands_rx: Receiver<Outgoing>) {
        thread::Builder::new()
            .name("orbit-pi-writer".into())
            .spawn(move || {
                while let Ok(outgoing) = commands_rx.recv() {
                    if write_json_line(&mut stdin, &outgoing.wire).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn pi writer thread");
    }

    fn spawn_reader(
        stdout: ChildStdout,
        events_tx: &Sender<Event>,
        pending: &Arc<Mutex<HashMap<String, Sender<Event>>>>,
    ) {
        let events_tx = events_tx.clone();
        // The pending map survives in the client; the reader needs its own Arc.
        let pending = Arc::clone(pending);
        let routed_tx = events_tx.clone();
        thread::Builder::new()
            .name("orbit-pi-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match reader.read_until(b'\n', &mut line) {
                        Ok(0) => break, // EOF — pi exited
                        Ok(_) => {
                            // Strip a single trailing \r per spec (accept \r\n),
                            // then any dangling \n.
                            let mut line = line.as_slice();
                            while line.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                                line = &line[..line.len() - 1];
                            }
                            if line.is_empty() {
                                continue;
                            }
                            let line = String::from_utf8_lossy(line);
                            let event = Event::parse_line(&line);
                            if let Event::Response { id, .. } = &event {
                                if let Some(tx) = pending
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .remove(id)
                                {
                                    let _ = tx.send(event.clone());
                                }
                            }
                            if let Event::Response { .. } = &event {
                                let _ = routed_tx.send(event);
                            } else {
                                let _ = events_tx.send(event);
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = events_tx.send(Event::ProcessExited);
            })
            .expect("spawn pi reader thread");
    }

    fn spawn_stderr_drain(stderr: ChildStderr, ring: &Arc<Mutex<VecDeque<String>>>) {
        let ring = Arc::clone(ring);
        thread::Builder::new()
            .name("orbit-pi-stderr".into())
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let mut ring = ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if ring.len() >= STDERR_RING_CAP {
                        ring.pop_front();
                    }
                    ring.push_back(line);
                }
            })
            .expect("spawn pi stderr thread");
    }

    /// Cancel an `extension_ui_request` the host cannot answer — a parked
    /// session has no dialog surface, so its blocked run must be released.
    /// pi resolves the extension's pending promise on the *top-level*
    /// `cancelled` flag; nesting it under `value` (as this helper used to)
    /// left the extension waiting and the parked run stuck busy forever.
    pub fn cancel_dialog(&self, id: &str) -> Result<()> {
        let wire =
            serde_json::json!({ "type": "extension_ui_response", "id": id, "cancelled": true });
        self.commands_tx
            .send(Outgoing {
                wire: serde_json::to_string(&wire)?,
            })
            .map_err(|_| anyhow!("pi process is not running"))
    }
}

fn write_json_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    stdin.write_all(line.as_bytes())?;
    stdin.write_all(b"\n")?;
    stdin.flush()
}

impl Drop for PiClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Break the writer thread out of its channel recv; the write to the
        // dead child's stdin errors and the thread exits.
        let _ = self.commands_tx.send(Outgoing {
            wire: String::new(),
        });
    }
}

/// Suppress the console window Windows attaches to a console child process
/// started by a GUI-subsystem parent.
///
/// Orbit's release build has no console of its own (`windows_subsystem`), so
/// without `CREATE_NO_WINDOW` every `pi.cmd`, `git`, or `node` it spawns pops
/// its own console window. `.cmd` shims (`pi.cmd`) are run through `cmd.exe`
/// by `std`, which is why the flashing window shows a command prompt. Call
/// this on a [`std::process::Command`] before spawning; it is a no-op on
/// non-Windows targets.
pub fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // `CREATE_NO_WINDOW`: run the child without a visible console.
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// The resolved `pi` executable path, for one-shot subprocesses (e.g. the
/// commit-message generator) that do not go through [`PiClient`].
pub fn pi_binary() -> String {
    resolve_pi_bin()
}

/// Resolve the `pi` executable to spawn.
///
/// `PI_BIN` (if set) wins. Otherwise we look for `pi` on `PATH`, then in the
/// common Homebrew/install dirs, then in the user's home install dirs (npm,
/// pnpm, bun, volta, mise, …), and finally fall back to the bare name so any
/// spawn error still names something meaningful.
fn resolve_pi_bin() -> String {
    pi_bin_candidates()
        .into_iter()
        .next()
        .map(|bin| bin.to_string_lossy().into_owned())
        .unwrap_or_else(|| PI_BIN_NAMES[0].to_string())
}

/// Every plausible `pi` launcher, most-preferred first: `PI_BIN`, then each
/// `PATH` dir, then the platform search dirs. Public so a caller that needs to
/// *inspect* the install (the RPC patcher) can, not just spawn it.
pub fn pi_bin_candidates() -> Vec<PathBuf> {
    let mut bins: Vec<PathBuf> = Vec::new();
    if let Ok(bin) = std::env::var(PI_BIN_ENV) {
        if !bin.is_empty() {
            push_unique(&mut bins, PathBuf::from(bin));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in PI_BIN_NAMES {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    push_unique(&mut bins, candidate);
                }
            }
        }
    }
    for dir in search_dirs() {
        for name in PI_BIN_NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                push_unique(&mut bins, candidate);
            }
        }
    }
    bins
}

fn push_unique(bins: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !bins.contains(&candidate) {
        bins.push(candidate);
    }
}

/// Resolve the installed pi **package root** — the directory whose
/// `package.json` is named [`PI_PACKAGE_NAME`].
///
/// Both install layouts Orbit sees are handled, on every platform:
///
/// - **npm/pnpm/bun globals** — walk up from the launcher's real path to the
///   enclosing `node_modules/@earendil-works/pi-coding-agent`.
/// - **pi's managed installer** — the launcher is a script at `<agent>/bin/pi`,
///   so the package sits under
///   `<agent>/install/releases/<version>/node_modules/@earendil-works/pi-coding-agent`.
///   `PI_MANAGED_INSTALL_ROOT` and `install/current-version` are honoured.
///
/// `PI_PACKAGE_ROOT` wins when set. Otherwise every candidate launcher is tried
/// (`PI_BIN`, `PATH`, then the platform search dirs), then the managed install
/// roots directly. Callers persist a success so later launches skip the search.
pub fn pi_package_root() -> Result<PathBuf, String> {
    if let Some(root) = env_package_root()? {
        return Ok(root);
    }

    let mut launchers = Vec::new();
    for bin in pi_bin_candidates() {
        if let Some(root) = pi_package_root_from(&bin) {
            return Ok(root);
        }
        launchers.push(bin);
    }
    for install in install_root_candidates() {
        if let Some(root) = managed_package_root(&install) {
            return Ok(root);
        }
    }

    Err(if launchers.is_empty() {
        "pi package not found (no pi on PATH or in the usual install dirs)".to_string()
    } else {
        let looked = launchers
            .iter()
            .take(3)
            .map(|bin| bin.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("pi package not found from {looked}")
    })
}

/// The pi package root for a known path: the root itself, or a `pi` launcher
/// anywhere inside it. `None` when the path is not a pi install. Validates a
/// path saved by a previous launch without trusting it blindly.
pub fn pi_package_root_from(path: &Path) -> Option<PathBuf> {
    if is_pi_package(path) {
        return Some(path.to_path_buf());
    }
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(root) = walk_up_to_package_root(&real) {
        return Some(root);
    }
    // pi's managed launcher: `<agent>/bin/pi` → `<agent>/install/releases/<v>/…`.
    let dir = if real.is_dir() {
        real.as_path()
    } else {
        real.parent()?
    };
    dir.ancestors()
        .take(3)
        .find_map(|ancestor| managed_package_root(&ancestor.join("install")))
}

/// A `PI_PACKAGE_ROOT` value, validated. An explicitly set but invalid path is
/// an error rather than a silent fall-through, so a typo is visible.
fn env_package_root() -> Result<Option<PathBuf>, String> {
    let Some(value) = std::env::var_os(PI_PACKAGE_ROOT_ENV).filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    pi_package_root_from(&path)
        .map(Some)
        .ok_or_else(|| format!("{} is not a pi package root", path.display()))
}

/// Walk up from `start` to a directory whose `package.json` names the pi
/// package. Handles the npm/pnpm/bun global layout.
fn walk_up_to_package_root(start: &Path) -> Option<PathBuf> {
    let start = if start.is_dir() {
        start
    } else {
        start.parent()?
    };
    let mut dir = start.to_path_buf();
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

/// The package root inside a managed install dir (`…/.pi/agent/install`),
/// preferring the recorded `current-version` and falling back to the newest
/// release still carrying the pi package.
fn managed_package_root(install: &Path) -> Option<PathBuf> {
    let package = |version: &str| {
        install
            .join("releases")
            .join(version)
            .join("node_modules")
            .join(PI_PACKAGE_NAME)
    };
    if let Ok(raw) = fs::read_to_string(install.join("current-version")) {
        let version = raw.trim();
        if !version.is_empty() {
            let root = package(version);
            if is_pi_package(&root) {
                return Some(root);
            }
        }
    }
    let mut versions: Vec<String> = fs::read_dir(install.join("releases"))
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    versions.sort_by_key(|version| version_key(version));
    versions.iter().rev().find_map(|version| {
        let root = package(version);
        is_pi_package(&root).then_some(root)
    })
}

/// Managed-install roots to probe when no launcher could be resolved.
fn install_root_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(env) = std::env::var_os("PI_MANAGED_INSTALL_ROOT").filter(|value| !value.is_empty())
    {
        roots.push(PathBuf::from(env));
    }
    if let Some(home) = home_dir() {
        let root = home.join(".pi").join("agent").join("install");
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// `1.0.10` sorts after `1.0.9`, unlike a plain string compare.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Install dirs probed for `pi` when it isn't on `PATH`: the absolute
/// [`PI_SEARCH_DIRS`], the home-relative [`HOME_SEARCH_DIRS`], and (on
/// Windows) the npm/pnpm/bun global dirs.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = PI_SEARCH_DIRS.iter().map(PathBuf::from).collect();
    if let Some(home) = home_dir() {
        dirs.extend(HOME_SEARCH_DIRS.iter().map(|sub| home.join(sub)));
    }
    #[cfg(windows)]
    dirs.extend(windows_search_dirs());
    dirs
}

/// npm and friends put their global launchers under `%APPDATA%` or
/// `%LOCALAPPDATA%` on Windows, none of which appear in [`HOME_SEARCH_DIRS`].
#[cfg(windows)]
fn windows_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(&local).join("pnpm"));
        dirs.push(PathBuf::from(&local).join("Volta").join("bin"));
        dirs.push(PathBuf::from(&local).join("Yarn").join("bin"));
        dirs.push(PathBuf::from(&local).join("mise").join("shims"));
        dirs.push(
            PathBuf::from(&local)
                .join("Programs")
                .join("bun")
                .join("bin"),
        );
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        dirs.push(PathBuf::from(&profile).join(".bun").join("bin"));
        dirs.push(PathBuf::from(&profile).join(".volta").join("bin"));
    }
    dirs
}

/// The user's home directory (`HOME`, falling back to `USERPROFILE`).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// The PATH handed to a pi child: the resolved binary's dir plus common
/// install dirs prepended to whatever PATH the parent process has. Public so
/// every direct pi spawn (updater, version probe) resolves `node` from a
/// bundled `.app`, not just the RPC session.
pub fn augmented_path(bin_dir: Option<&Path>) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = bin_dir {
        if !dir.as_os_str().is_empty() {
            dirs.push(dir.to_path_buf());
        }
    }
    dirs.extend(PATH_EXTRA_DIRS.iter().map(PathBuf::from));
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
}

#[cfg(test)]
mod tmp_resolve_probe {
    use super::*;
    #[test]
    fn tmp_probe_resolve() {
        println!("PI_BIN_NAMES = {PI_BIN_NAMES:?}");
        println!("candidates = {:?}", pi_bin_candidates());
        println!("resolved = {}", resolve_pi_bin());
        println!("package_root = {:?}", pi_package_root());
    }
}

#[cfg(test)]
mod package_root_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_dir() -> PathBuf {
        let unique = format!(
            "orbit-rpc-package-root-{}-{}",
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

    fn write_pi_package(root: &Path) {
        fs::create_dir_all(root.join("dist").join("bundle")).unwrap();
        fs::write(
            root.join("package.json"),
            format!(r#"{{"name":"{PI_PACKAGE_NAME}","version":"0.0.0"}}"#),
        )
        .unwrap();
        fs::write(
            root.join("dist").join("bundle").join("cli.js"),
            "#!/usr/bin/env node",
        )
        .unwrap();
    }

    #[test]
    fn walks_up_to_an_npm_global_package() {
        let dir = scratch_dir();
        let root = dir
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-coding-agent");
        write_pi_package(&root);
        let bin = root.join("dist").join("bundle").join("cli.js");

        assert_eq!(
            pi_package_root_from(&bin),
            Some(fs::canonicalize(&root).unwrap())
        );
        assert_eq!(pi_package_root_from(&dir.join("missing")), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolves_a_managed_install_from_its_launcher() {
        // pi's managed layout: the launcher at <agent>/bin/pi is a script, not
        // inside node_modules, so the package is only reachable via
        // <agent>/install/current-version.
        let dir = scratch_dir();
        let agent = dir.join("agent");
        let bin = agent.join("bin").join("pi");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, "#!/bin/sh\n").unwrap();

        let root = agent
            .join("install")
            .join("releases")
            .join("1.0.4")
            .join("node_modules")
            .join(PI_PACKAGE_NAME);
        write_pi_package(&root);
        fs::write(agent.join("install").join("current-version"), "1.0.4\n").unwrap();

        assert_eq!(
            pi_package_root_from(&bin),
            Some(fs::canonicalize(&root).unwrap())
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn managed_uses_current_version_then_newest_release() {
        let dir = scratch_dir();
        let install = dir.join("install");
        fs::create_dir_all(&install).unwrap();
        let install = fs::canonicalize(&install).unwrap();
        let root = |version: &str| {
            install
                .join("releases")
                .join(version)
                .join("node_modules")
                .join(PI_PACKAGE_NAME)
        };
        write_pi_package(&root("1.0.3"));
        write_pi_package(&root("1.0.10"));

        // No current-version: the newest release wins, compared numerically so
        // `1.0.10` beats `1.0.9` rather than sorting as a string.
        assert_eq!(managed_package_root(&install), Some(root("1.0.10")));

        // current-version wins when present.
        fs::write(install.join("current-version"), "1.0.3").unwrap();
        assert_eq!(managed_package_root(&install), Some(root("1.0.3")));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn version_key_orders_numerically() {
        let mut versions = vec![
            "1.0.9".to_string(),
            "1.0.10".to_string(),
            "1.0.2".to_string(),
        ];
        versions.sort_by_key(|version| version_key(version));
        assert_eq!(versions, vec!["1.0.2", "1.0.9", "1.0.10"]);
    }
}
