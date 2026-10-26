# pi `auth.*` RPC patch

Orbit talks to `pi --mode rpc` for **all** provider authentication. pi's RPC
server does not ship the `auth.*` commands yet, so without this patch Orbit
falls back to the legacy path: it launches `pi /login <provider>` in
Terminal.app instead of opening the browser.

This directory contains a small, reproducible patch that adds the five
`auth.*` commands to the **installed** pi build by reusing pi's own
`ModelRuntime` and `~/.pi/agent/auth.json`. It is a stop-gap until pi ships
the handlers upstream. The client-side contract is
[`../../crates/orbit-rpc/docs/auth-rpc.md`](../../crates/orbit-rpc/docs/auth-rpc.md).

## Apply

```sh
node contrib/pi-auth-rpc/apply.mjs
```

Then restart the agent: **Settings → Runtime → Restart**, or reopen Orbit.

The script resolves `pi` on `PATH` (override with `PI_BIN`), finds the bundled
RPC mode, and injects the handlers. It is idempotent, writes a
`<file>.orbit-orig` backup next to the patched file, and prints what it did.

## Revert

```sh
node contrib/pi-auth-rpc/apply.mjs --revert
```

## What it adds

| Command | Behavior |
|---|---|
| `auth.list` | Advertises every provider from `ModelRuntime.getProviders()` with `credential`, `authenticated`, and login `methods` (`browser`, `device_code`, `api_key`). The UI renders entirely from this. |
| `auth.status` | Credential kind + configured state for one provider. |
| `auth.login` | Runs `ModelRuntime.login(provider, type, interaction)` in the background, mapping pi's `AuthInteraction` callbacks onto `auth_login_started` / `auth_login_url` / `auth_device_code` / `auth_login_waiting` / `auth_login_succeeded` / `auth_login_failed`. |
| `auth.logout` | `ModelRuntime.logout(provider)`. |
| `auth.cancel` | Aborts the session's `AbortController`, which unwinds the OAuth wait. |

Prompt handling keeps browser flows unattended: OAuth method `select` prompts
are answered from the requested method (`browser`/`device_code`), a blank
`text` answer accepts GitHub Enterprise's `github.com` default, and
`manual_code` prompts stay pending so the localhost callback (or device poll)
can win. `secret` (API-key) prompts are rejected — Orbit edits API keys
through its own provider editor.

## Notes

- This edits a **global npm install**. Re-run `apply.mjs` after `pi update`;
  it detects the new bundle and patches it again.
- Tokens never cross the patch boundary: only the credential *kind*, device
  codes, and verification URLs are emitted. The full credential is written by
  pi's own `ModelRuntime` into `auth.json` (0600).
