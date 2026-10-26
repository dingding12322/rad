//! Probe: what does pi do when `switch_session` is sent mid-run?
//!
//! Spawns a real `pi --mode rpc`, starts a turn, switches sessions while it
//! streams, and logs every event so we can see whether the old run keeps
//! going, aborts, or the switch is rejected.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use orbit_rpc::{CommandBody, Event, PiClient};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn pi_available() -> bool {
    let bin = std::env::var("PI_BIN").unwrap_or_else(|_| "pi".into());
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(&bin))
        .any(|p| p.is_file())
}

#[test]
fn probe_switch_session_mid_run() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }

    let client = PiClient::spawn(&repo_root(), Some(Path::new("/tmp/orbit-switch-probe")))
        .expect("spawn pi");

    // Start a turn that takes a few seconds.
    client
        .send(CommandBody::Prompt {
            message: "Count slowly from 1 to 20, one number per line.".into(),
            images: None,
            streaming_behavior: None,
        })
        .expect("send prompt");

    // Wait for agent_start, then switch sessions.
    let mut switched = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !switched && Instant::now() < deadline {
        for event in client.drain_events() {
            if matches!(event, Event::AgentStart) {
                eprintln!(">>> agent_start seen, sending switch_session");
                let rx = client
                    .send(CommandBody::SwitchSession {
                        session_path: "/tmp/orbit-switch-probe/other-session.jsonl".into(),
                    })
                    .expect("send switch_session");
                // Log the switch response.
                for _ in 0..20 {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(ev) => eprintln!("SWITCH RESPONSE: {ev:?}"),
                        Err(_) => break,
                    }
                }
                switched = true;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(switched, "never saw agent_start");

    // Now watch what streams after the switch.
    let watch = Instant::now() + Duration::from_secs(45);
    let mut deltas_after_switch = 0;
    let mut settled_after_switch = false;
    while Instant::now() < watch {
        for event in client.drain_events() {
            match &event {
                Event::MessageUpdate {
                    assistant: Some(orbit_rpc::AssistantMessageEvent::TextDelta { delta }),
                    ..
                } => {
                    deltas_after_switch += 1;
                    eprint!("{delta}");
                }
                Event::AgentSettled => {
                    settled_after_switch = true;
                    eprintln!("\n>>> agent_settled after switch");
                }
                Event::AgentEnd { .. } => eprintln!("\n>>> agent_end after switch"),
                Event::Response {
                    command,
                    success,
                    error,
                    ..
                } => eprintln!("\n>>> response {command} success={success} error={error:?}"),
                Event::ProcessExited => {
                    eprintln!("\n>>> PROCESS EXITED after switch");
                    eprintln!(
                        "RESULT: deltas_after_switch={deltas_after_switch} settled={settled_after_switch}"
                    );
                    return;
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!("\nRESULT: deltas_after_switch={deltas_after_switch} settled={settled_after_switch}");
}
