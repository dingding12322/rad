# Provider auth RPC (`auth.*`) — server contract

This document is the wire contract Orbit's Rust client implements for
first-class provider authentication. The **server side lives in pi** and must
reuse pi's existing provider authentication, OAuth flows, and credential store
(`~/.pi/agent/auth.json`). Orbit never implements a provider OAuth flow and
never receives a token.

Reference client implementation: `crates/orbit-rpc/src/types.rs` (wire types)
and `crates/orbit-pi/src/auth.rs` (`AuthManager` reducer).

## Secret boundary

Only non-sensitive material crosses this protocol:

- ✅ allowed: provider id/name, auth method ids/labels, credential *kind*
  (`oauth` / `api_key` / `none`), expiry timestamps, account labels, device
  codes, verification URLs, structured error codes, and login session ids.
- ❌ forbidden: access tokens, refresh tokens, client secrets, or any value
  that grants access. A login success event reports the credential *kind*, not
  its value.

The client parses only the fields it knows. A server that includes token
fields in a payload cannot get them into GPUI state, because no token field is
deserialized or stored.

## Commands

All commands are JSON objects on stdin with an optional `id` for correlation;
the response echoes the `id`.

### `auth.list`

Capability discovery. The UI renders provider cards and login buttons from
this response, so **clients never hardcode provider ids or flows**.

```json
{"id":"1","type":"auth.list"}
```

```json
{
  "id": "1",
  "type": "response",
  "command": "auth.list",
  "success": true,
  "data": {
    "providers": [
      {
        "id": "anthropic",
        "name": "Anthropic",
        "credential": "oauth",
        "authenticated": true,
        "methods": [
          {"id": "browser", "label": "Sign in with browser"},
          {"id": "device_code", "label": "Use a device code"},
          {"id": "api_key", "label": "API key"}
        ]
      }
    ]
  }
}
```

- `credential` and `authenticated` reflect the current `auth.json` state.
- `methods[].id` is the value the client passes back to `auth.login`. The
  server chooses the set; the client must not assume any particular method.
- Well-known method ids are `browser`, `device_code`, and `api_key`. Unknown
  ids are rendered with their given label and started the same way.

A server that does not implement `auth.*` returns
`{"success":false,"error":"Unknown command: auth.list"}`. Orbit treats that as
"unsupported" and keeps its legacy file/Terminal login path.

### `auth.status`

Per-provider credential facts, for refresh after a login/logout or on demand.

```json
{"id":"2","type":"auth.status","provider":"anthropic"}
```

```json
{
  "id": "2",
  "type": "response",
  "command": "auth.status",
  "success": true,
  "data": {
    "id": "anthropic",
    "authenticated": true,
    "credential": "oauth",
    "expiresAt": 1700000000000,
    "account": "user@example.com"
  }
}
```

`expiresAt` (epoch ms) and `account` are optional and never required.

### `auth.login`

Start a login. `method` comes from `auth.list`. The client generates
`sessionId` so the flow can be cancelled deterministically and reconciled
after a restart; the server must use it as the login session id and echo it on
every `auth.*` event for that session.

```json
{"id":"3","type":"auth.login","provider":"anthropic","method":"browser","sessionId":"orbit-auth-1a2b-1"}
```

```json
{
  "id": "3",
  "type": "response",
  "command": "auth.login",
  "success": true,
  "data": {"sessionId": "orbit-auth-1a2b-1", "status": "pending"}
}
```

The response acknowledges acceptance. Progress is asynchronous; the server
then streams events (below). A failure to start returns `success:false` with a
structured `error` (or a following `auth_login_failed` event).

### `auth.logout`

Remove the stored credential for a provider.

```json
{"id":"4","type":"auth.logout","provider":"anthropic"}
```

```json
{
  "id": "4",
  "type": "response",
  "command": "auth.logout",
  "success": true,
  "data": {"provider": "anthropic", "status": "signed_out"}
}
```

### `auth.cancel`

Cancel an in-flight login. Must abort any pending OAuth wait and must not
write a credential.

```json
{"id":"5","type":"auth.cancel","sessionId":"orbit-auth-1a2b-1"}
```

```json
{"id":"5","type":"response","command":"auth.cancel","success":true}
```

## Events

Events stream on stdout like every other pi event. They carry no `id`
correlation and no secrets.

| Event | Meaning |
|---|---|
| `auth_login_started` | Session accepted and running. |
| `auth_login_url` | Browser flow: open this URL. |
| `auth_device_code` | Device flow: display this code and URL. |
| `auth_login_waiting` | Optional progress note. |
| `auth_login_succeeded` | Finished; credential stored. |
| `auth_login_failed` | Finished with a structured error. |
| `auth_login_cancelled` | Cancelled (user, timeout, or shutdown). |
| `auth_credentials_changed` | One or more providers' credentials changed. |

```json
{"type":"auth_login_started","sessionId":"s1","provider":"anthropic","method":"browser","expiresAt":1700000000000}
{"type":"auth_login_url","sessionId":"s1","provider":"anthropic","url":"https://claude.ai/oauth?code=..."}
{"type":"auth_device_code","sessionId":"s2","provider":"openai-codex","userCode":"ABCD-1234","verificationUri":"https://auth.openai.com/device","verificationUriComplete":"https://auth.openai.com/device?code=ABCD-1234","expiresAt":1700000000000}
{"type":"auth_login_waiting","sessionId":"s1","provider":"anthropic","message":"Waiting for authorization…"}
{"type":"auth_login_succeeded","sessionId":"s1","provider":"anthropic","method":"browser","credential":"oauth","expiresAt":1700003600000}
{"type":"auth_login_failed","sessionId":"s1","provider":"anthropic","code":"timeout","message":"The sign-in request timed out."}
{"type":"auth_login_cancelled","sessionId":"s1","provider":"anthropic"}
{"type":"auth_credentials_changed","providers":["anthropic","openai"]}
```

For the **browser** flow, the server emits `auth_login_url`; the Orbit client
opens it with the OS default browser. For the **device-code** flow, the server
emits `auth_device_code`; the client displays the code and offers to open the
verification URL. The client also independently enforces a login deadline
(default 5 minutes, or the event's `expiresAt`) and cancels on timeout.

## Structured error codes

`auth_login_failed.code` (and the failure `error` string where practical):

| Code | Meaning |
|---|---|
| `invalid_provider` | Unknown provider id. |
| `unsupported_method` | Provider does not offer that method. |
| `already_in_progress` | A login for this provider exists. |
| `not_authenticated` | Status/logout for an unauthenticated provider. |
| `unsupported` | Auth RPC not available on this build. |
| `login_failed` | Provider rejected or aborted the login. |
| `timeout` | The login expired. |
| `cancelled` | Cancelled. |
| `network_error` | Transport failure talking to the provider. |
| `storage_error` | Could not read/write `auth.json`. |
| `internal_error` | Anything else. |

Unknown codes are treated as `unknown` by the client and are never fatal.

## Lifecycle guarantees the client relies on

- **Login session ids** are stable for the life of a login and echoed on every
  session-scoped event, so out-of-order responses/events still correlate.
- **Cancellation** always produces `auth_login_cancelled` (or a
  `cancelled`/`timeout` failure) and never writes a credential.
- **Restart recovery:** if the pi process dies mid-login, Orbit remembers the
  provider, re-runs `auth.list` on the next process, and reconciles the
  interrupted attempt to a success if the credential is now present.
- **Live credentials:** a successful `auth.login`/`auth.logout` should take
  effect in the running pi process, so the client refreshes models/status
  without forcing a restart.

## Implementing the server (pi)

The handler belongs in `modes/rpc/rpc-mode.ts` and must:

1. Register the five command types above.
2. Reuse pi's `AuthStorage` / `ModelRuntime` credential store
   (`~/.pi/agent/auth.json`) for reads and writes; do not introduce a second
   credential store.
3. Reuse pi-ai's provider `oauth.login` / `refreshToken` / `getApiKey`
   implementations and its `OAuthLoginCallbacks` (`onSelect`, `onOpenUrl`,
   `onDeviceCode`, `onPrompt`), mapping those callbacks onto the `auth.*`
   events here.
4. Never serialize token material; emit only the credential kind on success.
5. Honor `auth.cancel` with an `AbortController` wired into the OAuth wait, and
   emit a terminal event exactly once.
