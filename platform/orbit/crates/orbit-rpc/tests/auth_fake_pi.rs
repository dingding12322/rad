//! End-to-end transport test for the provider-auth RPC namespace.
//!
//! The real `pi` binary does not (yet) implement `auth.*`, so this drives the
//! actual [`PiClient`] against a tiny scripted RPC server. It proves the
//! commands serialize onto stdin and the asynchronous `auth.*` events route
//! through the shared event stream and parse into typed [`AuthEvent`]s.
//! Unix-only because the fake server is a shell script.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use orbit_rpc::{AuthEvent, AuthProvider, CommandBody, Event, PiClient};

const FAKE_PI: &str = r#"#!/bin/sh
# Echo the command id back on responses (pi's correlation contract).
emit() {
  printf '%s\n' "$1" | sed "s/^{/{\"id\":\"$id\",/"
}
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"auth.list"'*)
      emit '{"type":"response","command":"auth.list","success":true,"data":{"providers":[{"id":"anthropic","name":"Anthropic","credential":"none","authenticated":false,"methods":[{"id":"browser","label":"Sign in with browser"},{"id":"device_code","label":"Use a device code"}]},{"id":"openai-codex","name":"ChatGPT (Codex)","credential":"none","authenticated":false,"methods":[{"id":"device_code","label":"Device code"}]}]}}'
      ;;
    *'"type":"auth.status"'*)
      emit '{"type":"response","command":"auth.status","success":true,"data":{"id":"anthropic","authenticated":true,"credential":"oauth"}}'
      ;;
    *'"type":"auth.login"'*)
      emit '{"type":"response","command":"auth.login","success":true,"data":{"sessionId":"sess-1","status":"pending"}}'
      printf '%s\n' '{"type":"auth_login_started","sessionId":"sess-1","provider":"anthropic","method":"browser"}'
      printf '%s\n' '{"type":"auth_login_url","sessionId":"sess-1","provider":"anthropic","url":"https://claude.ai/oauth?code=abc"}'
      printf '%s\n' '{"type":"auth_login_succeeded","sessionId":"sess-1","provider":"anthropic","method":"browser","credential":"oauth"}'
      ;;
    *'"type":"auth.logout"'*)
      emit '{"type":"response","command":"auth.logout","success":true,"data":{"provider":"anthropic","status":"signed_out"}}'
      ;;
    *'"type":"auth.cancel"'*)
      emit '{"type":"response","command":"auth.cancel","success":true}'
      printf '%s\n' '{"type":"auth_login_cancelled","sessionId":"sess-1","provider":"anthropic"}'
      ;;
  esac
done
"#;

fn fake_pi() -> (PathBuf, tempdir::TempDir) {
    let dir = tempdir::TempDir::new("orbit-auth-rpc").expect("temp dir");
    let path = dir.path().join("fake-pi");
    std::fs::write(&path, FAKE_PI).expect("write fake pi");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake pi");
    }
    (path, dir)
}

/// Minimal owned temp dir so the test keeps no extra dev-dependency.
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

/// Drain the shared stream until `done` is satisfied or the timeout lapses,
/// returning every auth event seen.
fn collect_auth_events(
    client: &PiClient,
    timeout: Duration,
    done: impl Fn(&AuthEvent) -> bool,
) -> Vec<AuthEvent> {
    let deadline = std::time::Instant::now() + timeout;
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        for event in client.drain_events() {
            if let Event::Auth(auth) = event {
                let finished = done(&auth);
                seen.push(auth);
                if finished {
                    return seen;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    seen
}

fn response(client: &PiClient, body: CommandBody) -> Event {
    let rx = client.send(body).expect("send");
    rx.recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|err| panic!("response: {err}; stderr={:?}", client.recent_stderr(20)))
}

#[test]
fn routes_auth_commands_and_async_events() {
    let (bin, _dir) = fake_pi();
    let workspace = std::env::temp_dir();
    let client =
        PiClient::spawn_with_bin(bin.to_str().unwrap(), &workspace, None).expect("spawn fake pi");

    // auth.list → capability discovery the UI renders from.
    match response(&client, CommandBody::AuthList) {
        Event::Response {
            command,
            success,
            data,
            ..
        } => {
            assert_eq!(command, "auth.list");
            assert!(success);
            let providers: Vec<AuthProvider> = data
                .as_ref()
                .and_then(|data| data.get("providers"))
                .and_then(serde_json::Value::as_array)
                .expect("providers")
                .iter()
                .filter_map(AuthProvider::from_value)
                .collect();
            assert_eq!(providers.len(), 2);
            assert!(providers[0].supports_oauth());
            assert_eq!(
                providers[1].device_code_method().map(|m| m.id.as_str()),
                Some("device_code")
            );
        }
        other => panic!("expected auth.list response, got {other:?}"),
    }
    let _ = client.drain_events();

    // auth.login → acknowledgement plus the streaming browser flow.
    match response(
        &client,
        CommandBody::AuthLogin {
            provider: "anthropic".into(),
            method: "browser".into(),
            session_id: Some("client-sess".into()),
        },
    ) {
        Event::Response {
            command,
            success,
            data,
            ..
        } => {
            assert_eq!(command, "auth.login");
            assert!(success);
            assert_eq!(
                data.as_ref()
                    .and_then(|d| d.get("sessionId"))
                    .and_then(|v| v.as_str()),
                Some("sess-1")
            );
        }
        other => panic!("expected auth.login response, got {other:?}"),
    }
    let events = collect_auth_events(&client, Duration::from_secs(3), |event| {
        matches!(event, AuthEvent::LoginSucceeded { .. })
    });
    assert!(
        matches!(events.first(), Some(AuthEvent::LoginStarted { .. })),
        "first event should be login started, got {events:?}"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, AuthEvent::LoginUrl { url, .. } if url.contains("claude.ai"))
        ),
        "browser URL should stream through, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AuthEvent::LoginSucceeded { credential, .. } if credential.as_deref() == Some("oauth"))),
        "login success should carry only the credential kind, got {events:?}"
    );

    // auth.status
    match response(
        &client,
        CommandBody::AuthStatus {
            provider: "anthropic".into(),
        },
    ) {
        Event::Response {
            command,
            success,
            data,
            ..
        } => {
            assert_eq!(command, "auth.status");
            assert!(success);
            assert_eq!(
                data.as_ref()
                    .and_then(|d| d.get("credential"))
                    .and_then(|v| v.as_str()),
                Some("oauth")
            );
        }
        other => panic!("expected auth.status response, got {other:?}"),
    }
    let _ = client.drain_events();

    // auth.cancel → cancellation event.
    match response(
        &client,
        CommandBody::AuthCancel {
            session_id: "sess-1".into(),
        },
    ) {
        Event::Response {
            command, success, ..
        } => {
            assert_eq!(command, "auth.cancel");
            assert!(success);
        }
        other => panic!("expected auth.cancel response, got {other:?}"),
    }
    let events = collect_auth_events(&client, Duration::from_secs(3), |event| {
        matches!(event, AuthEvent::LoginCancelled { .. })
    });
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AuthEvent::LoginCancelled { session_id, .. } if session_id == "sess-1")),
        "cancel should stream a cancellation, got {events:?}"
    );

    // auth.logout
    match response(
        &client,
        CommandBody::AuthLogout {
            provider: "anthropic".into(),
        },
    ) {
        Event::Response {
            command, success, ..
        } => {
            assert_eq!(command, "auth.logout");
            assert!(success);
        }
        other => panic!("expected auth.logout response, got {other:?}"),
    }
}
