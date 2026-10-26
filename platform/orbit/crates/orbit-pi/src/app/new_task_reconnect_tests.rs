//! Regression tests for issue #30: with pi disconnected (the process exited,
//! the reconnect banner is up), the sidebar's New Task button did nothing.
//! `on_new_session` only checked whether a run was in flight, so it sent
//! `new_session` into the dead process's stdin — a write nobody answers —
//! instead of spawning a fresh process. The decision is `can_reuse_session`:
//! reuse only when the process is alive and idle; otherwise start a task on a
//! fresh process.
#![cfg(unix)]

use super::*;
use crate::theme::{Theme, ThemeId};
use std::os::unix::fs::PermissionsExt as _;

/// A fake pi that swallows stdin into `stdin.log` and blocks, so the
/// transport sees a live child without spawning the real CLI. Deleted when
/// the test ends; the app's `PiClient` kills the child first.
struct FakePi {
    dir: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl FakePi {
    fn start() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "orbit-new-task-{}-{:?}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create fake-pi dir");
        let bin = dir.join("fake-pi");
        let log = dir.join("stdin.log");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\nexec cat >> \"{}\"\n", log.display()),
        )
        .expect("write fake pi");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake pi");
        Self { dir, bin, log }
    }

    /// The lines the app wrote to pi's stdin, once the writer thread flushes.
    fn stdin(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let text = std::fs::read_to_string(&self.log).unwrap_or_default();
            if !text.is_empty() || Instant::now() > deadline {
                return text;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for FakePi {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn test_app(cx: &mut gpui::TestAppContext) -> Entity<OrbitApp> {
    cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    })
}

/// Install `fake` as the active pi process, as a live spawn would.
fn adopt_fake_pi(app: &mut OrbitApp, fake: &FakePi) {
    let workspace = app
        .current_workspace
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let client = PiClient::spawn_with_bin(fake.bin.to_str().expect("utf-8 path"), &workspace, None)
        .expect("spawn fake pi");
    app.adopt_client(client);
}

/// What `ProcessExited` leaves behind: the client is still held while the
/// runtime records the death. New Task must not `send` into that stdin.
#[gpui::test]
fn an_exited_pi_process_never_reuses_the_session(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    cx.update(|cx| {
        app.update(cx, |app, _| {
            let workspace = app
                .current_workspace
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            let dead = PiClient::spawn_with_bin("/usr/bin/false", &workspace, None)
                .expect("spawn /usr/bin/false");
            app.client = Some(dead);
            app.runtime.alive = false;
            app.runtime.exited = true;

            assert!(!app.runtime_is_live());
            assert!(
                !app.can_reuse_session(),
                "a dead pi process cannot honor new_session"
            );
        });
    });
}

/// A runtime that was never started (or was stopped): no process to reuse.
#[gpui::test]
fn a_missing_pi_process_never_reuses_the_session(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    cx.update(|cx| {
        app.update(cx, |app, _| {
            app.drop_client();
            assert!(!app.can_reuse_session());
        });
    });
}

/// A run in flight: `new_session` would abort the live turn, so the task
/// must start on its own parked-and-replaced process.
#[gpui::test]
fn a_running_turn_never_reuses_the_session(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    let fake = FakePi::start();
    cx.update(|cx| {
        app.update(cx, |app, _| {
            adopt_fake_pi(app, &fake);
            assert!(app.can_reuse_session(), "an idle live process is reusable");
            app.busy = true;
            assert!(!app.can_reuse_session(), "a live turn is not reusable");
        });
    });
}

/// The warm path stays warm: an idle live process takes `new_session` in
/// place — no second spawn, and the command reaches pi's stdin.
#[gpui::test]
fn an_idle_live_session_reuses_the_process_for_the_new_task(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    let fake = FakePi::start();
    let started_at = cx.update(|cx| {
        app.update(cx, |app, _| {
            adopt_fake_pi(app, &fake);
            app.runtime.started_at
        })
    });
    let cx = cx.add_empty_window();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.can_reuse_session());
            app.on_new_session(&crate::NewSession, window, cx);
        });
    });
    let stdin = fake.stdin();
    assert!(
        stdin.contains("\"type\":\"new_session\""),
        "expected new_session on pi's stdin, got: {stdin:?}"
    );
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            assert_eq!(
                app.runtime.started_at, started_at,
                "reusing the live process must not spawn another"
            );
        });
    });
}

/// Dropping the dead process supersedes the exit banner it left behind, so
/// starting a fresh task does not keep a stale "pi process exited" on screen.
#[gpui::test]
fn dropping_an_exited_process_clears_its_exit_banner(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    cx.update(|cx| {
        app.update(cx, |app, _| {
            app.runtime.exited = true;
            app.set_error(tr!("events.process_exited"));
            app.drop_client();
            assert!(app.client.is_none());
            assert!(
                app.error.is_none(),
                "the banner described the process that was just dropped"
            );
        });
    });
}

/// The clear is narrow: a healthy process dropping for any reason must not
/// take an unrelated error banner with it.
#[gpui::test]
fn dropping_a_healthy_process_keeps_unrelated_errors(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    cx.update(|cx| {
        app.update(cx, |app, _| {
            app.set_error("agent error: no API key");
            app.drop_client();
            assert_eq!(app.error.as_deref(), Some("agent error: no API key"));
        });
    });
}
