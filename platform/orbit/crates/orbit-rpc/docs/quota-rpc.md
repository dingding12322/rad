# Provider quota RPC (`quota.*`) — server contract

This document is the wire contract Orbit's Rust client implements for
account-level provider quota, balance, and spend. The **server side lives in
pi** and must resolve credentials through pi's own auth (`~/.pi/agent/auth.json`
and the provider's `resolve()`/`toAuth()` path), then query the provider's own
usage endpoint. Orbit never implements provider HTTP calls and never receives a
token.

Reference client implementation: `crates/orbit-rpc/src/types.rs` (wire types)
and `crates/orbit-pi/src/quota.rs` (`QuotaManager` reducer). The pi-side
handlers are injected by `contrib/pi-quota-rpc/apply.mjs`.

## Bridge path (default, no patch)

Orbit does not require the `quota.*` namespace. It ships a pi extension
(`contrib/orbit-quota-extension/`, materialized under
`~/.orbit-pi/quota-extension/` by `crates/orbit-pi/src/bundled_extensions.rs`)
and loads it on every session process with `pi --extension <index.js>`. The
extension resolves credentials through pi's own `ctx.modelRegistry`, queries
the same provider endpoints, and appends one normalized snapshot as a custom
session entry:

```json
{"type":"custom","customType":"orbit:quota","data":{"providers":[…]}}
```

Custom entries never enter the model's context. Orbit reads them with pi's
standard `get_entries` command, passing the previous entry id as `since` so a
poll returns only entries appended after it; the newest `orbit:quota` entry is
merged into the same `QuotaManager` as a `quota.list` response (bridge data
does not change `QuotaSupport`, so a patched pi is still probed). The extension
appends only when the snapshot changes — `fetchedAt` is ignored for that
comparison — so a quiet account does not grow the session file. Entry ids are
per-session: the client resets its cursor when `get_state` reports a different
`sessionId`, and drops the cursor and re-reads when pi answers `Entry not
found`.

`quota.list` remains fully supported and is used when the running pi
implements it; the bridge simply makes the feature work on stock pi.

## Secret boundary

Only non-sensitive, already-displayable material crosses this protocol:

- ✅ allowed: provider id/name, plan tier, window labels, percentages, token or
  request counts, allowance/limit counts, reset timestamps, monetary balances
  with currency, trailing spend, fetch time, and structured error text.
- ❌ forbidden: API keys, OAuth access/refresh tokens, account ids, device
  codes, raw provider payloads, or any value that grants access.

The client parses only the fields it knows. A server that includes secret
fields cannot get them into GPUI state, because no such field is deserialized
or stored.

## Command

### `quota.list`

Query one connected provider, or every provider that pi knows about.

```json
{"id":"1","type":"quota.list"}
{"id":"2","type":"quota.list","provider":"anthropic"}
```

```json
{
  "id": "1",
  "type": "response",
  "command": "quota.list",
  "success": true,
  "data": {
    "providers": [
      {
        "provider": "anthropic",
        "kind": "subscription",
        "plan": "Max",
        "windows": [
          {"id":"five_hour","label":"5-hour","usedPercent":6.0,"resetsAt":1738300000000},
          {"id":"weekly","label":"Weekly","usedPercent":35.0,"resetsAt":1738900000000}
        ],
        "balances": [],
        "fetchedAt": 1738290000000
      },
      {
        "provider": "deepseek",
        "kind": "balance",
        "windows": [],
        "balances": [{"label":"Available","amount":110.0,"currency":"CNY"}],
        "fetchedAt": 1738290000000
      },
      {
        "provider": "groq",
        "kind": "unsupported",
        "windows": [],
        "balances": [],
        "note": "Groq exposes no account usage API."
      }
    ]
  }
}
```

- The response is always `success: true` when the RPC namespace is supported,
  even if individual providers error or are unsupported; each report carries
  its own `kind` / `error`. A command-level failure means the namespace or the
  request itself failed.
- `kind` is one of `subscription`, `credits`, `balance`, `spend`, or
  `unsupported`.
- A report with no windows and no balances is valid and means "connected, but
  no usage surface" (render `note`).
- `error` is optional and, when present, may accompany stale windows. Error
  text must never echo credential material.

## Supported providers

Coverage is intentionally layered. Providers with a public usage surface get a
dedicated adapter; providers without one are reported `unsupported` rather than
approximated.

| Provider id | Kind | Source |
|---|---|---|
| `anthropic` (OAuth) | subscription | `/api/oauth/usage` |
| `openai-codex` | subscription | `/backend-api/wham/usage` |
| `github-copilot` | subscription | `/copilot_internal/user` |
| `kimi-coding` | subscription | `/coding/v1/usages` |
| `minimax`, `minimax-cn` | subscription / balance | `/v1/token_plan/remains`, `/account/query_balance` |
| `zai`, `zai-coding-cn` | subscription | `/api/monitor/usage/quota/limit` |
| `opencode`, `opencode-go` | subscription | `/zen/go/v1/usage` |
| `openrouter` | credits | `/api/v1/key` |
| `vercel-ai-gateway` | credits | `/v1/credits` |
| `deepseek` | balance | `/user/balance` |
| `moonshotai`, `moonshotai-cn` | balance | `/v1/users/me/balance` |
| `xai` (OAuth) | credits | consumer billing proxy |
| `fireworks` | spend | `/v1/accounts/{id}/billing/summary` |
| `baseten` | spend | `/v1/billing/usage_summary` |
| `google`, `google-vertex` | unsupported | no account quota API (AI Studio only) |
| `ollama`, `ollama-cloud` | subscription | `/api/usage` (usage-credit or legacy windows) merged with authenticated `/settings` (reset times) |
| everything else | unsupported | — |

`google`/`google-vertex` are registered explicitly so a connected account gets a
provider-specific `note` rather than the generic fallback. Gemini limits are per
Google Cloud project (AI Studio shows usage; the API only returns 429 on
exhaustion), so there is no account-quota endpoint to call.

### Ollama Cloud

Ollama Cloud has two billing generations. The **current** model is a monthly
usage-credit pool; `GET https://ollama.com/api/usage` returns a `limits` object
whose buckets are a 0..1 `usage` fraction plus per-model request counts. The
buckets have flip-flopped: `session` + `weekly` through 2026-09-02, a single
`monthly` bucket around 2026-09-03, and `session` + `weekly` again since
2026-09-07, so all three are optional and whichever are present are rendered.
The monthly reset is the subscription anniversary, absent from the payload, so
the server omits a countdown rather than guessing.

The **legacy** model exposes a 5-hour session window, a weekly window, and reset
timestamps only on the authenticated `https://ollama.com/settings` page; the
server fetches that page with a user-supplied session cookie and parses it
behind an isolated `OllamaCloudParser` (an unstable integration that returns a
structured `error` instead of throwing when the markup changes).

`/api/usage` never exposes reset timestamps. When an account stores **both** a
cloud key and a session cookie, the server queries both and merges: percentages
come from the API (authoritative for the current account), and each matching
window's `resetsAt` is copied from the settings page by window id. If either
fetch fails, the usable report is returned unchanged rather than guessing. A
key-only account therefore has no countdown; its report carries a `note`
explaining that a session cookie is required, which the client renders beneath
the meters when no window exposes a reset.

The credential is explicit and comes from `auth.json` (never a browser cookie).
The session lives under its own key so it can never shadow the `ollama` provider
credential — pi treats any stored credential under a provider id as
authoritative, so an unknown type there would break the local endpoint. Both
Ollama provider ids resolve the same account: the local endpoint's `ollama`
and the `ollama-cloud` id registered by the third-party
`pi-ollama-cloud-provider` package. The session is stored once and read for
either id:

```json
"ollama":               {"type":"api_key","key":"<real ollama.com key>"}
"ollama-cloud":         {"type":"api_key","key":"<real ollama.com key>"}
"ollama-cloud-session": {"type":"ollama_cloud_session","session":"__Secure-session=…"}
```

The server must never send the local `models.json` placeholder
(`api_key: "ollama"`) to `ollama.com`, never auto-extract browser cookies, and
never log the session value.

Adapters must verify the response origin before sending a credential, reject
redirects, and never follow a credential to a custom/proxy host.

## Caching

Provider usage endpoints rate-limit aggressively (the Anthropic OAuth usage
endpoint returns HTTP 429 under polling). The server must cache per provider
for a short TTL (recommended 60 s), honor `Retry-After`, and never fetch on a
render tick. Orbit additionally only sends `quota.list` when the Providers page
opens, on Refresh, and after a credential change.

## Implementing the server (pi)

The handler lives in `contrib/pi-quota-rpc/apply.mjs` (marker-injected into the
installed pi bundle, same mechanism as `contrib/pi-auth-rpc`). Each command
must:

1. Resolve the provider credential with `session.modelRuntime.getAuth(id)`.
2. Reject custom/proxy origins and redirects before sending the credential.
3. Map the provider payload into the normalized shape above.
4. Emit no token material, account id, or raw payload.
5. Cache the normalized report; return cached data (with the original
   `fetchedAt`) until the TTL lapses.
