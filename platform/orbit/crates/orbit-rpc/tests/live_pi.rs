//! Live integration test for the pi RPC transport — the P0 round-trip proof.
//!
//! Spawns the real `pi --mode rpc` binary and drives a full request/response
//! cycle plus a prompt, asserting the event pipeline works end to end.
//!
//! Requires the `pi` CLI on PATH (or `PI_BIN`). Skips gracefully if missing.

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
    which(&bin)
}

fn which(cmd: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(cmd))
        .any(|p| p.is_file())
}

#[test]
fn live_get_state_round_trip() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }

    let client =
        PiClient::spawn(&repo_root(), Some(Path::new("/tmp/orbit-pi-sessions"))).expect("spawn pi");
    let rx = client.send(CommandBody::GetState).expect("send get_state");

    let deadline = Instant::now() + Duration::from_secs(15);
    let state = loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Response {
                command,
                success,
                data,
                ..
            }) => {
                assert_eq!(command, "get_state");
                assert!(success, "get_state failed");
                break data.expect("get_state data");
            }
            Ok(_) => continue, // other events first (e.g. ui requests)
            Err(_) if Instant::now() > deadline => panic!("get_state timed out"),
            Err(_) => continue,
        }
    };

    assert!(state.get("sessionId").is_some(), "no sessionId in state");
    assert!(
        state.get("model").is_some() || state.get("thinkingLevel").is_some(),
        "expected model/thinking info in state: {state}"
    );
    eprintln!("✅ get_state ok — session: {}", state["sessionId"]);
}

#[test]
fn live_process_stays_alive_with_default_session_dir() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }
    let mut client = PiClient::spawn(&repo_root(), None).expect("spawn pi");
    std::thread::sleep(Duration::from_secs(4));
    assert!(client.is_alive(), "pi died within 4s (default session dir)");
    eprintln!("✅ pi alive after 4s (default session dir)");
}

#[test]
fn live_prompt_streams_agent_events() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }

    let client =
        PiClient::spawn(&repo_root(), Some(Path::new("/tmp/orbit-pi-sessions"))).expect("spawn pi");
    client
        .send(CommandBody::Prompt {
            message: "Reply with just the word OK.".into(),
            images: None,
            streaming_behavior: None,
        })
        .expect("send prompt");

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut saw_agent = false;
    let mut saw_text = false;
    let mut settled = false;

    while Instant::now() < deadline {
        for event in client.drain_events() {
            match &event {
                Event::AgentStart => saw_agent = true,
                Event::MessageUpdate {
                    assistant: Some(orbit_rpc::AssistantMessageEvent::TextDelta { delta }),
                    ..
                } => {
                    saw_text = true;
                    eprint!("{delta}");
                }
                Event::AgentSettled => settled = true,
                _ => {}
            }
        }
        if settled {
            break;
        }
        std::thread::sleep(Duration::from_millis(60));
    }

    assert!(saw_agent, "expected agent_start");
    assert!(saw_text, "expected text deltas");
    assert!(settled, "agent did not settle within 90s");
    eprintln!("\n✅ prompt completed: agent_start → text stream → agent_settled");
}
