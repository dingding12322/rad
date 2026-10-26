//! Compare get_state model fields with get_available_models entries.

use std::time::{Duration, Instant};

use orbit_rpc::{CommandBody, Event, PiClient};

fn pi_available() -> bool {
    let bin = std::env::var("PI_BIN").unwrap_or_else(|_| "pi".into());
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(&bin))
        .any(|p| p.is_file())
}

#[test]
fn live_state_model_matches_catalog() {
    if !pi_available() {
        eprintln!("skipping: pi CLI not found");
        return;
    }

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let client = PiClient::spawn(&root, None).expect("spawn");

    let _ = client.send(CommandBody::GetState).expect("state");
    let _ = client
        .send(CommandBody::GetAvailableModels)
        .expect("models");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut state_model = None;
    let mut catalog = None;

    while (state_model.is_none() || catalog.is_none()) && Instant::now() < deadline {
        for ev in client.drain_events() {
            if let Event::Response { command, data, .. } = ev {
                match command.as_str() {
                    "get_state" => state_model = data.and_then(|d| d.get("model").cloned()),
                    "get_available_models" => catalog = data,
                    _ => {}
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let state_model = state_model.expect("state model");
    let catalog = catalog.expect("catalog");
    let arr = catalog["models"].as_array().expect("models array");

    let id = state_model["id"].as_str().unwrap_or("");
    let name = state_model["name"].as_str().unwrap_or("");
    let provider = state_model["provider"].as_str().unwrap_or("");

    eprintln!("STATE: id={id:?} name={name:?} provider={provider:?}");

    let found = arr.iter().find(|m| {
        m["id"].as_str() == Some(id)
            || m["name"].as_str() == Some(name)
            || m["id"].as_str() == Some(name)
    });
    eprintln!("CATALOG MATCH: {found:?}");

    assert!(
        found.is_some(),
        "state model not found in catalog — id={id:?} name={name:?}"
    );
}
