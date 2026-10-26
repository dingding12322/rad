//! End-to-end transport test for the provider-quota RPC namespace.
//!
//! The real `pi` binary does not implement `quota.*` without the
//! `contrib/pi-quota-rpc` patch, so this drives the actual [`PiClient`] against
//! a tiny scripted RPC server. It proves `quota.list` serializes onto stdin and
//! the response parses into typed [`QuotaReport`]s. Unix-only because the fake
//! server is a shell script.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use orbit_rpc::{parse_quota_reports, CommandBody, Event, PiClient, QuotaKind};

const FAKE_PI: &str = r#"#!/bin/sh
emit() {
  printf '%s\n' "$1" | sed "s/^{/{\"id\":\"$id\",/"
}
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"quota.list"'*)
      emit '{"type":"response","command":"quota.list","success":true,"data":{"providers":[{"provider":"anthropic","kind":"subscription","plan":"Max","windows":[{"id":"five_hour","label":"5-hour","usedPercent":6.0,"resetsAt":1738300000000}],"balances":[],"fetchedAt":1738290000000},{"provider":"deepseek","kind":"balance","windows":[],"balances":[{"label":"Available","amount":110.0,"currency":"CNY"}]}]}}'
      ;;
  esac
done
"#;

fn fake_pi() -> (PathBuf, tempdir::TempDir) {
    let dir = tempdir::TempDir::new("orbit-quota-rpc").expect("temp dir");
    let path = dir.path().join("fake-pi");
    std::fs::write(&path, FAKE_PI).expect("write fake pi");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod fake pi");
    (path, dir)
}

mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(prefix: &str) -> std::io::Result<Self> {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
            std::fs::create_dir_all(&path)?;
            Ok(Self(path))
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn routes_quota_list_and_parses_reports() {
    let (bin, _dir) = fake_pi();
    let workspace = std::env::temp_dir();
    let client =
        PiClient::spawn_with_bin(bin.to_str().unwrap(), &workspace, None).expect("spawn fake pi");

    let rx = client
        .send(CommandBody::QuotaList { provider: None })
        .expect("send quota.list");
    let event = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|err| panic!("response: {err}; stderr={:?}", client.recent_stderr(20)));

    match event {
        Event::Response {
            command,
            success,
            data,
            ..
        } => {
            assert_eq!(command, "quota.list");
            assert!(success);
            let reports = parse_quota_reports(data.as_ref().expect("data"));
            assert_eq!(reports.len(), 2);
            let anthropic = &reports[0];
            assert_eq!(anthropic.kind, QuotaKind::Subscription);
            assert_eq!(anthropic.plan.as_deref(), Some("Max"));
            assert_eq!(anthropic.windows[0].id, "five_hour");
            assert_eq!(reports[1].balances[0].currency, "CNY");
        }
        other => panic!("expected quota.list response, got {other:?}"),
    }
}

#[test]
fn forwards_extension_paths_as_pi_flags() {
    // A fake pi that records its argv, so the test proves Orbit's bundled
    // quota bridge reaches the child process without any settings writes.
    let dir = tempdir::TempDir::new("orbit-extension-args").expect("temp dir");
    let argv_file = dir.path().join("argv.txt");
    let bin = dir.path().join("fake-pi");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            argv_file.display()
        ),
    )
    .expect("write fake pi");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod fake pi");

    let extension = dir.path().join("orbit-quota-bridge.js");
    let _client = PiClient::spawn_with_bin_and_extensions(
        bin.to_str().unwrap(),
        &std::env::temp_dir(),
        None,
        std::slice::from_ref(&extension),
    )
    .expect("spawn fake pi");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let argv = loop {
        if let Ok(argv) = std::fs::read_to_string(&argv_file) {
            // The shell creates the file before writing; an empty or
            // newline-less read is a partial write, keep waiting.
            if !argv.is_empty() && argv.ends_with('\n') {
                break argv;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fake pi never recorded its argv"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let args: Vec<&str> = argv.lines().collect();
    assert!(args.contains(&"--mode") && args.contains(&"rpc"));
    let flag = args
        .iter()
        .position(|arg| *arg == "--extension")
        .expect("--extension flag");
    assert_eq!(args[flag + 1], extension.to_string_lossy());
}
