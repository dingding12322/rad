//! Live check: catalog commands return the shapes the picker expects.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use orbit_rpc::{CommandBody, Event, PiClient};

fn repo_root() -> std::path::PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Mirror `live_pi.rs`: skip when `pi` is not installed (CI, clean machines).
fn pi_available() -> bool {
    let bin = std::env::var("PI_BIN").unwrap_or_else(|_| "pi".into());
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(&bin))
        .any(|p| p.is_file())
}

#[test]
fn live_catalog_round_trip() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }
    let client =
        PiClient::spawn(&repo_root(), Some(Path::new("/tmp/orbit-pi-sessions"))).expect("spawn pi");
    let _ = client.send(CommandBody::GetAvailableModels).expect("send");
    let _ = client
        .send(CommandBody::GetAvailableThinkingLevels)
        .expect("send2");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut models_seen = None;
    let mut levels_seen = None;
    while (models_seen.is_none() || levels_seen.is_none()) && Instant::now() < deadline {
        for ev in client.drain_events() {
            if let Event::Response {
                command,
                success,
                data,
                ..
            } = ev
            {
                eprintln!("response {command} success={success}");
                match command.as_str() {
                    "get_available_models" => models_seen = Some(data),
                    "get_available_thinking_levels" => levels_seen = Some(data),
                    _ => {}
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let models = models_seen.expect("models response");
    let models = models.expect("models data");
    let arr = models
        .get("models")
        .and_then(serde_json::Value::as_array)
        .expect("models array");
    eprintln!(
        "models: {} entries, first: {:?}",
        arr.len(),
        arr.first()
            .map(|m| (m["id"].as_str(), m["name"].as_str(), m["provider"].as_str()))
    );
    let levels = levels_seen.expect("levels response");
    eprintln!("levels: {levels:?}");
}
