# pi `quota.*` RPC patch

Orbit shows account-level provider quota, balance, and spend on the Providers
page. pi's RPC server has no such command — it only exposes session-local
`get_session_stats` — so this patch adds one command, `quota.list`, to the
installed pi build.

The handler resolves each connected provider's credential through pi's own
`session.modelRuntime` (Orbit never receives a token), queries that provider's
usage endpoint, and returns a normalized, non-secret report. The client
contract is
[`../../crates/orbit-rpc/docs/quota-rpc.md`](../../crates/orbit-rpc/docs/quota-rpc.md).

## Apply

```sh
node contrib/pi-quota-rpc/apply.mjs
```

Then restart the agent: **Settings → Runtime → Restart**, or reopen Orbit.

The script resolves `pi` on `PATH` (override with `PI_BIN`), finds the bundled
RPC mode, and injects the handler from `quota-handler.js`. It is idempotent,
shares the `<file>.orbit-orig` backup with the auth patch, and prints what it
did.

## Revert

```sh
node contrib/pi-quota-rpc/apply.mjs --revert
```

Revert restores the original pre-Orbit bundle, which also removes the
`auth.*` patch; re-run `contrib/pi-auth-rpc/apply.mjs` afterwards if you still
want in-app sign-in.

## Provider coverage

Adapters are modeled on [`@narumitw/pi-usage`](https://www.npmjs.com/package/@narumitw/pi-usage)'s
verified provider reference. Providers without a usage surface report
`unsupported`; nothing is fabricated.

| Provider id | Kind | Source |
|---|---|---|
| `anthropic` (OAuth) | subscription | `/api/oauth/usage` |
| `openai-codex` | subscription | `/backend-api/wham/usage` |
| `github-copilot` | subscription | `/copilot_internal/user` |
| `kimi-coding` | subscription | `/coding/v1/usages` |
| `minimax`, `minimax-cn` | subscription / balance | `/v1/token_plan/remains`, `/account/query_balance` |
| `zai`, `zai-coding-cn` | subscription | `/api/monitor/usage/quota/limit` |
| `opencode`, `opencode-go` | subscription | `/zen[/go]/v1/usage` |
| `openrouter` | credits | `/api/v1/key` |
| `vercel-ai-gateway` | credits | `/v1/credits` |
| `deepseek` | balance | `/user/balance` |
| `moonshotai`, `moonshotai-cn` | balance | `/v1/users/me/balance` |
| `xai` (OAuth) | credits | consumer billing proxy |
| `fireworks` | spend | `/v1/accounts/{id}/billing/summary` |
| `baseten` | spend | `/v1/billing/usage_summary` |
| `google`, `google-vertex` | unsupported | no account quota API (AI Studio only) |
| `ollama` | subscription | `GET https://ollama.com/api/usage` (monthly credits) or the authenticated settings page (legacy session/weekly) |

`google`/`google-vertex` are registered explicitly so a connected account gets a
provider-specific explanation rather than the generic fallback. Gemini rate
limits are per Google Cloud project and visible only in AI Studio, so there is
no account-quota endpoint to call.

### Ollama Cloud

Ollama Cloud accounts are on one of two billing generations, and the adapter
handles both:

- **Current (usage credits)** — a single monthly dollar-credit pool metered in
  tokens. `GET https://ollama.com/api/usage` with a real cloud API key returns a
  0..1 `limits.monthly.usage` fraction. The reset is on the plan's subscription
  anniversary, which the payload does not expose, so no countdown is shown.
- **Legacy (GPU time)** — 5-hour session + weekly windows, rendered server-side
  on `https://ollama.com/settings`. These have no API, so the adapter fetches
  that page with a user-supplied session cookie and parses it via
  `OllamaCloudParser`.

Credentials are **explicit and user-supplied**, stored in `auth.json` (0600).
The session lives under its own key so it never shadows the `ollama` provider
credential:

```json
"ollama":               {"type":"api_key","key":"<real ollama.com key>"}             // current model
"ollama-cloud-session": {"type":"ollama_cloud_session","session":"__Secure-session=…"} // legacy page
```

The adapter never reads browser cookies, never sends the local `models.json`
placeholder (`api_key: "ollama"` pointing at `127.0.0.1`) to ollama.com, and
never logs the session. Set either in **Settings → Providers → Ollama**
(`Add API key` for the key, `Usage session` for the cookie header).

## Notes

- Handler adapters have network-free unit tests:
  `node --test contrib/pi-quota-rpc/quota-handler.test.mjs`.
- This edits a **global npm install**. Re-run `apply.mjs` after `pi update`.
- Requests use a 12 s timeout, reject redirects, and cache per provider for
  60 s (these endpoints rate-limit aggressively).
- Errors are sanitized: credential-shaped substrings are redacted before they
  can reach Orbit.
