//! Live coverage for the agent-control RPC commands added for the Agent
//! settings surface: queue modes, auto-compaction/auto-retry toggles,
//! `clear_queue`, `set_session_name`, and `steer`/`follow_up` framing.
//!
//! These commands do not need an LLM call, so the test is cheap and
//! deterministic. Requires the `pi` CLI on PATH (or `PI_BIN`); skips when
//! missing, like `live_pi.rs`.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use orbit_rpc::{CommandBody, Event, PiClient};
use serde_json::Value;

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

/// Send a command and return `(success, data)` for its response, skipping
/// unrelated events. Panics on timeout.
fn round_trip(client: &PiClient, body: CommandBody, command: &str) -> (bool, Option<Value>) {
    let rx = client.send(body).expect("send command");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Response {
                command: got,
                success,
                data,
                ..
            }) => {
                assert_eq!(got, command, "response command mismatch");
                return (success, data);
            }
            Ok(_) => continue,
            Err(_) if Instant::now() > deadline => panic!("{command} timed out"),
            Err(_) => continue,
        }
    }
}

#[test]
fn live_agent_control_commands_round_trip() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }

    let client =
        PiClient::spawn(&repo_root(), Some(Path::new("/tmp/orbit-pi-sessions"))).expect("spawn pi");

    // Queue delivery modes. These are per-process and reset when pi exits.
    let (ok, _) = round_trip(
        &client,
        CommandBody::SetSteeringMode { mode: "all".into() },
        "set_steering_mode",
    );
    assert!(ok, "set_steering_mode failed");

    let (ok, _) = round_trip(
        &client,
        CommandBody::SetFollowUpMode {
            mode: "one-at-a-time".into(),
        },
        "set_follow_up_mode",
    );
    assert!(ok, "set_follow_up_mode failed");

    // Toggles.
    for enabled in [false, true] {
        let (ok, _) = round_trip(
            &client,
            CommandBody::SetAutoCompaction { enabled },
            "set_auto_compaction",
        );
        assert!(ok, "set_auto_compaction({enabled}) failed");
        let (ok, _) = round_trip(
            &client,
            CommandBody::SetAutoRetry { enabled },
            "set_auto_retry",
        );
        assert!(ok, "set_auto_retry({enabled}) failed");
    }

    // Rename, then confirm `get_state` reflects it (and the steering mode).
    let (ok, _) = round_trip(
        &client,
        CommandBody::SetSessionName {
            name: "orbit-live-test".into(),
        },
        "set_session_name",
    );
    assert!(ok, "set_session_name failed");

    let (ok, data) = round_trip(&client, CommandBody::GetState, "get_state");
    assert!(ok, "get_state failed");
    let data = data.expect("get_state data");
    assert_eq!(data["steeringMode"], "all");
    assert_eq!(data["followUpMode"], "one-at-a-time");
    assert_eq!(data["sessionName"], "orbit-live-test");

    // `clear_queue` reports the (empty) pending queues and is safe while idle.
    let (ok, data) = round_trip(&client, CommandBody::ClearQueue, "clear_queue");
    assert!(ok, "clear_queue failed");
    let data = data.expect("clear_queue data");
    assert!(data["steering"].is_array(), "steering queue: {data}");
    assert!(data["followUp"].is_array(), "follow-up queue: {data}");

    eprintln!("✅ agent-control commands round-trip against real pi");
}

/// A failed command must report `success: false` with a non-empty `error`
/// string — the shape the app's error banner consumes (docs #error-handling).
#[test]
fn live_failed_command_reports_error() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }

    let client =
        PiClient::spawn(&repo_root(), Some(Path::new("/tmp/orbit-pi-sessions"))).expect("spawn pi");

    let rx = client
        .send(CommandBody::SetModel {
            provider: "no-such-provider".into(),
            model_id: "no-such-model".into(),
        })
        .expect("send set_model");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Response {
                command,
                success,
                error,
                ..
            }) => {
                assert_eq!(command, "set_model");
                assert!(!success, "invalid model should fail");
                let error = error.expect("failure carries an error string");
                assert!(!error.is_empty(), "error string is empty");
                eprintln!("✅ failed command reported: {error}");
                break;
            }
            Ok(_) => continue,
            Err(_) if Instant::now() > deadline => panic!("set_model timed out"),
            Err(_) => continue,
        }
    }
}
