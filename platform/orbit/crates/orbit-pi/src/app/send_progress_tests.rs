//! Regression tests: sending a prompt from the composer must put the app into a
//! running state *immediately*, not when pi's first event arrives.
//!
//! The busy flag used to be set only on `agent_start`, and the transcript's live
//! "Working for…" row only appeared once a step streamed — so between Enter and
//! the first token (a slow provider, a booting pi) the composer still read
//! "Send" and the transcript sat silent. Both are now armed by `submit_as`.
#![cfg(unix)]

use super::*;
use crate::theme::{Theme, ThemeId};
use std::os::unix::fs::PermissionsExt as _;

/// A pi that swallows stdin: the write succeeds, the process stays alive, and
/// no event ever comes back — the exact window the optimistic state must cover.
struct SilentPi {
    dir: PathBuf,
    bin: PathBuf,
}

impl SilentPi {
    fn start(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "orbit-send-{tag}-{}-{:?}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create silent-pi dir");
        let bin = dir.join("silent-pi");
        std::fs::write(&bin, "#!/bin/sh\nexec cat >/dev/null\n").expect("write silent pi");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("chmod silent pi");
        Self { dir, bin }
    }
}

impl Drop for SilentPi {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn app_with_silent_pi(cx: &mut gpui::TestAppContext) -> (Entity<OrbitApp>, SilentPi) {
    let fake = SilentPi::start("progress");
    let app = cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    });
    cx.update(|cx| {
        app.update(cx, |app, _| {
            let workspace = std::env::temp_dir();
            let client =
                PiClient::spawn_with_bin(fake.bin.to_str().expect("utf-8 path"), &workspace, None)
                    .expect("spawn silent pi");
            app.adopt_client(client);
        })
    });
    (app, fake)
}

/// The send itself flips the app to running: the composer shows Stop and the
/// transcript shows the live row before pi says anything.
#[gpui::test]
fn submitting_a_prompt_marks_the_run_in_flight_at_once(cx: &mut gpui::TestAppContext) {
    let (app, _fake) = app_with_silent_pi(cx);
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            assert!(!app.is_running(), "idle before the send");

            app.submit_as("run the tests".to_owned(), SendMode::FollowUp, cx);

            assert!(
                app.busy,
                "the composer flips to Stop on the send, not later"
            );
            assert!(!app.transcript.is_streaming(), "no token has arrived yet");
            assert!(
                app.transcript.is_awaiting_reply(),
                "the live row is up while the answer is pending"
            );
            assert!(app.is_running(), "a pending prompt is a run in flight");
        });
    });
}

/// While that first prompt is pending, a second Enter is a follow-up, not a
/// second prompt: the same decision the app makes once pi is visibly running.
#[gpui::test]
fn a_second_prompt_while_pending_queues_as_a_follow_up(cx: &mut gpui::TestAppContext) {
    let (app, _fake) = app_with_silent_pi(cx);
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            app.submit_as("first".to_owned(), SendMode::FollowUp, cx);
            app.submit_as("second".to_owned(), SendMode::FollowUp, cx);

            assert_eq!(app.queue.follow_up, vec!["second".to_owned()]);
            assert_eq!(app.pending_follow_up.as_deref(), Some("second"));
        });
    });
}

/// A prompt pi rejects never opened a turn: the run state unwinds instead of
/// leaving the composer on Stop forever.
#[gpui::test]
fn a_rejected_prompt_clears_the_run_state(cx: &mut gpui::TestAppContext) {
    let (app, _fake) = app_with_silent_pi(cx);
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            app.submit_as("run the tests".to_owned(), SendMode::FollowUp, cx);
            assert!(app.busy);

            app.on_command_failure("prompt", Some("model not available"), cx);

            assert!(!app.busy, "the composer returns to Send");
            assert!(!app.transcript.is_awaiting_reply(), "the live row goes");
            assert!(
                app.error.is_some(),
                "the failure still reaches the user as a banner"
            );
        });
    });
}
