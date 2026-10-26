//! Transport coverage for the `extra_args` spawn seam that carries Orbit's
//! default session model (`--provider` / `--model`).
//!
//! A fake pi records its argv, so the test proves the flags reach the child
//! process and that an empty slice leaves the base `--mode rpc --approve` argv
//! byte-identical to before the seam existed. Unix-only because the fake server
//! is a shell script.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use orbit_rpc::PiClient;

/// Distinguishes the two tests' scratch dirs; `SystemTime` alone can return
/// the same nanosecond to tests that construct a `FakePi` in parallel.
static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct FakePi {
    _dir: PathBuf,
    bin: PathBuf,
    argv: PathBuf,
}

impl FakePi {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orbit-spawn-args-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let argv = dir.join("argv.txt");
        let bin = dir.join("fake-pi");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\ntmp='{argv}.tmp'\nprintf '%s\\n' \"$@\" > \"$tmp\"\nmv \"$tmp\" '{argv}'\n",
                argv = argv.display()
            ),
        )
        .expect("write fake pi");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake pi");
        Self {
            _dir: dir,
            bin,
            argv,
        }
    }

    fn read_argv(&self) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(raw) = std::fs::read_to_string(&self.argv) {
                // The script writes to a sibling file and renames it into
                // place, so a visible `argv.txt` is always complete.
                if !raw.is_empty() && raw.ends_with('\n') {
                    return raw.lines().map(str::to_string).collect();
                }
            }
            assert!(Instant::now() < deadline, "fake pi never recorded its argv");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn workspace() -> &'static Path {
    Path::new("/tmp")
}

#[test]
fn empty_extra_args_keep_the_base_argv() {
    let fake = FakePi::new();
    let _client =
        PiClient::spawn_with_bin_and_args(fake.bin.to_str().unwrap(), workspace(), None, &[])
            .expect("spawn fake pi");
    assert_eq!(
        fake.read_argv(),
        vec!["--mode", "rpc", "--approve"],
        "an empty extra-args slice must not change argv"
    );
}

#[test]
fn extra_args_are_appended_to_the_base_argv() {
    let fake = FakePi::new();
    let args: Vec<String> = [
        "--provider",
        "anthropic",
        "--model",
        "anthropic/claude-sonnet-4-5",
        "--thinking",
        "high",
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    let _client =
        PiClient::spawn_with_bin_and_args(fake.bin.to_str().unwrap(), workspace(), None, &args)
            .expect("spawn fake pi");
    assert_eq!(
        fake.read_argv(),
        vec![
            "--mode",
            "rpc",
            "--approve",
            "--provider",
            "anthropic",
            "--model",
            "anthropic/claude-sonnet-4-5",
            "--thinking",
            "high",
        ]
    );
}
