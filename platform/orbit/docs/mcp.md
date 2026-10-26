# MCP Integration

Orbit manages [Model Context Protocol](https://modelcontextprotocol.io) servers for the pi
agent. Pi owns MCP execution — transports, connections, tool invocation, OAuth, and retries.
Orbit owns the management surface: a native Settings → MCP page over Pi's own configuration
files, validation, secrets, status inspection, and the safe process restart that applies
configuration changes to a live session.

There is deliberately no MCP client in Orbit. Everything that would duplicate Pi stays in Pi.

## Architecture

```text
┌─────────────────────────────────────────────┐
│               Orbit UI (GPUI)               │
│   Settings → MCP · add/edit modal · palette │
├─────────────────────────────────────────────┤
│                McpManager                   │
│  config I/O · validation · secrets · state  │
├─────────────────────────────────────────────┤
│                 Pi process                  │
│        pi --mode rpc --approve              │
├─────────────────────────────────────────────┤
│             Pi MCP extension                │
│     stdio / streamable HTTP · tools         │
└─────────────────────────────────────────────┘
```

| Module | Responsibility |
|---|---|
| `crates/orbit-pi/src/mcp.rs` | Domain model: `McpServer`, `McpServerDef`, `McpTransport`, `McpExposure`, `McpScope`, `McpServerStatus`, `McpError`, secret-reference helpers, error redaction. |
| `crates/orbit-pi/src/mcp/config.rs` | Reads, validates, merges, and atomically writes Pi's `mcp.json` files. Preserves unknown entries and file indentation. |
| `crates/orbit-pi/src/mcp/secrets.rs` | The `${NAME}` secret store (`~/.orbit-pi/mcp-secrets.json`, mode 0600). Values are injected into Pi's environment; never written to `mcp.json` and never logged. |
| `crates/orbit-pi/src/mcp/manager.rs` | `McpManager`: one source of truth for the configured servers, their cached probe state, CRUD, the `pi mcp list --json` probe, and config fingerprints. |
| `crates/orbit-pi/src/app/mcp_ui.rs` | Settings → MCP rendering, the add/edit modal, actions, probe/apply scheduling, keyboard navigation. |

Pi's RPC protocol has **no MCP management commands**. Configuration is file-based, so Orbit
reads and writes exactly the files Pi reads, and applies changes by re-spawning Pi with the
same session. Status and tool lists come from Pi itself via `pi mcp list --json`.

## Configuration

Pi reads two files, and Orbit reads and writes the same two:

```text
~/.pi/agent/mcp.json     global scope
<project>/.pi/mcp.json   project scope (trusted projects; project entries replace global
                         entries with the same name)
```

The format is the shared `mcpServers` shape:

```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_TOKEN": "${MCP_GLOBAL_GITHUB_GITHUB_TOKEN}" }
    },
    "docs": {
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" },
      "exposure": "direct"
    }
  }
}
```

Supported fields (the only fields Orbit ever writes):

| Field | Notes |
|---|---|
| `command`, `args`, `env`, `cwd` | stdio servers. `command` is a single executable, never a shell string. |
| `url`, `headers`, `oauth` | streamable HTTP servers. The legacy SSE transport is not supported. |
| `enabled` | `false` keeps the entry without connecting it. The default (`true`) is omitted. |
| `exposure` | `codemode` (default), `codemode-deferred`, `deferred`, `direct`, or `hidden`. The default is omitted. |
| `toolExposure` | Per-tool exposure overrides; preserved by the edit form, edited in `mcp.json`. |
| `timeout` | Per-request timeout in seconds; preserved by the edit form. |
| top-level `autoEnableCodemode` | Preserved; `false` raises a warning when a server uses codemode exposure. |

Descriptions are **not** part of Pi's schema, so Orbit keeps them in
`~/.orbit-pi/mcp-metadata.json` and never writes them into `mcp.json`.

### Scopes

- **Global** — `~/.pi/agent/mcp.json`, available across all projects.
- **Project** — `<project>/.pi/mcp.json`, available only in that project.

Pi only loads project configuration in trusted projects. Orbit spawns every session with
`--approve` (project trust), so project servers always load in Orbit sessions. The
standalone `pi mcp list` probe follows Pi's persisted trust store; when a project is not
trusted there, the probe reports a note and Orbit shows it on the page rather than pretending
the project servers failed.

## Lifecycle

A configuration change made in Orbit follows one path:

```text
validate → persist (atomic write) → re-read state → probe → restart Pi if the running
process is stale → switch_session back to the same conversation
```

- Writes are atomic (temp file + rename), preserve unrelated entries and the file's
  indentation, and abort untouched on a parse or I/O error.
- The probe (`pi mcp list --json`) is debounced by ~500 ms and runs on the background
  executor; it never blocks rendering.
- The apply is debounced a little longer, coalesces rapid edits, and defers until an
  in-flight agent run settles so a restart can never abort work.
- Pi processes are stamped with the config fingerprint they were spawned with
  (`McpManager::fingerprint`). A restart happens only when the running process is actually
  stale; a parked session whose stamp is stale is dropped when it is next opened, so it
  cold-starts on the current configuration.

Restarting Pi to apply MCP changes preserves the session: Orbit keeps the session file and
sends `switch_session` to the new process, which reloads the conversation.

External edits to `mcp.json` (the agent, another editor, another Orbit window) are detected
by polling the two files while the MCP page is open. Orbit re-reads them, shows a notice, and
applies them to the running session only when the user asks — it never overwrites them.

## Secrets

`mcp.json` is a configuration file, not a credential store. Orbit writes `${NAME}` references
and keeps the values in `~/.orbit-pi/mcp-secrets.json`:

1. A literal value typed into the Environment or Headers field is stored under a generated
   name and the field is written as a `${NAME}` reference. Global secrets use
   `MCP_GLOBAL_<SERVER>_<FIELD>`; project secrets use
   `MCP_P<WORKSPACE_HASH>_<SERVER>_<FIELD>`, so two projects — or a project and the global
   scope — with the same server and field never share one stored value.
2. A value that already contains a `${NAME}` reference (or a `!command`) is written verbatim.
3. Every Pi spawn and every `pi mcp list` probe receives the store's values as environment
   variables, so Pi expands the references exactly as it would for a shell export.
4. Values are masked in the UI (`••••••••`); an untouched mask round-trips to the value it
   hides, and an untouched literal is migrated into the store on save.
5. Removing a server prunes the Orbit-generated secrets in the current workspace's
   namespaces (`MCP_GLOBAL_…` and the active project's `MCP_P…_`) that no remaining server
   references. Other projects' secrets and names outside those namespaces are never deleted.
6. Every error string that reaches the UI passes through redaction: known secret values and
   `Bearer` credentials are masked first.

The store is chmod `0600` on Unix and lives in the user profile on Windows — the same local,
user-scoped trust model as Pi's own credential files under `~/.pi/agent/`. A keychain-backed
store can replace `mcp/secrets.rs` behind the same API.

The store's values are injected into every agent process, exactly as shell exports would be;
Pi resolves them only where the configuration references them. Project trust is the boundary:
Orbit spawns sessions with `--approve`, so a project's configuration and extensions run with
the same access a terminal session would grant them. Keep credentials out of untrusted
projects.

## Logging

Orbit logs MCP lifecycle events (`config_updated`, `server_removed`,
`server_enabled`/`server_disabled`, `probe_started`/`probe_finished`, `pi_restarted`,
`pi_restart_failed`) as single-line `orbit-mcp event=<name> …` records on stderr. Every
detail passes through the same redaction as the UI, so a credential value can never reach a
log line. A terminal launch and Console.app see these records; release builds write no log
file of their own. Pi's server log messages are appended by Pi itself to
`~/.pi/agent/mcp.log`.

## Codemode and deferred exposure

`codemode` (the default) and `codemode-deferred` keep MCP tool declarations out of the model's
tool list. Those tools are instead callable from Pi's `codemode` tool — a JavaScript sandbox
where the model orchestrates several calls and returns only what it needs. `deferred` leaves
the tools undeclared until Pi's `tool_search` tool loads them.

Orbit does not implement either tool; Pi owns them. What Orbit does is:

- write the server's `exposure` (and any per-tool `toolExposure` overrides it had) to
  `mcp.json`; the Add/Edit form offers all five modes;
- leave `autoEnableCodemode` alone and warn on the page when it is `false` while a codemode
  server is enabled (the tools would be unreachable);
- keep its bundled workflow extension from disturbing them — Plan/Ask subtract only the
  write tools from the *current* active set, so a `codemode`/`tool_search` tool that Pi
  activated when a server connected survives a mode change;
- render `codemode`, `tool_search`, and the MCP resource tools with their own card icons and
  labels in the transcript.

So the app-side setup for codemode is: add the server, choose **Codemode** as its exposure
(the default), and make sure `autoEnableCodemode` is not set to `false`. Pi then activates the
`codemode` tool automatically once a codemode server connects.

## Sign in with OAuth

Pi is the OAuth client. An HTTP server **without** an `Authorization` header is OAuth-capable,
and no credentials belong in `mcp.json`. When such a server rejects the connection, the probe
reports `needs-auth`, the row shows **Sign-in required**, and a **Sign in** button appears next
to it.

- **Sign in** runs `pi mcp login <server>`: Pi opens the authorization page in the browser,
  serves the loopback callback, registers itself dynamically with the authorization server
  (unless a client is configured below), stores tokens in `~/.pi/agent/mcp-auth.json`, and
  refreshes them automatically. Orbit waits (up to five minutes) and offers **Cancel**, which
  kills the login process. On success the probe refreshes and the server turns green with its
  tool list.
- **Sign out** (in the expanded server detail) runs `pi mcp logout <server>`, deleting Pi's
  stored credentials, and restarts a running session so the live access is dropped too.

The form's collapsed **OAuth client settings** exist only for authorization servers that do
not support dynamic client registration:

| Field | `mcp.json` |
|---|---|
| Client ID | `oauth.clientId` |
| Client secret | `oauth.clientSecret` — stored in Orbit's secret store, written as `${NAME}` |
| Callback port | `oauth.callbackPort` — fixes the redirect URI to `http://127.0.0.1:<port>/callback` |
| Callback URL | `oauth.callbackUrl` — must be an `http` loopback URI |
| Scope | `oauth.scope` — space-separated scopes for servers that do not advertise them |

`pi mcp login` follows Pi's persisted project-trust store, so signing in to a **project-scoped**
OAuth server requires the project to be trusted by Pi's CLI; global servers have no such
requirement.

## Pi integration

- `BundledExtensions::spawn` passes the secret values into every session process
  (`spawn_with_extensions_and_env_and_args` in `orbit-rpc`).
- MCP tool calls arrive as ordinary tool events (`mcp__<server>__<tool>`), so they flow
  through the existing transcript, results, and permission pipeline. The transcript renders
  them as `MCP · <server>` cards with the tool name; results keep the normal
  collapse/expand behavior.
- Permission approval is Pi's tool pipeline plus Orbit's access guard
  (`contrib/orbit-guard-extension`), which classifies `mcp__…` tools as `mcp` and asks in
  every confined mode. "Always allow this tool" stores the exact tool name in
  `~/.orbit-pi/access-allow.json`.
- `pi mcp login <server>` (OAuth) remains available through Pi; Orbit round-trips `oauth`
  settings in `mcp.json` and shows `Sign-in required` from the probe.

## Where status shows

- **Settings → MCP** is the full page: every server, its tools, errors, and actions.
- The **top-bar usage pill** opens a popover with two tabs: **Providers** (the original quota
  cards) and **MCP**. The MCP pane is one compact row per server — status glyph, name,
  exposure/scope facts, tool count, a contextual **Sign in**/**Cancel** during OAuth, and the
  probe's error when one exists. Clicking a row opens Settings → MCP at that server. The pill
  renders when either side has something to show; with no provider quota reports it becomes
  the MCP status chip itself and the popover opens straight on the MCP pane, so the server
  names are never behind an empty Providers tab. An empty Providers pane offers **Show MCP
  servers** to switch.
- The **status bar** carries a quiet `MCP n/N` chip whenever servers are enabled; clicking it
  opens the page.
- **Transcript cards** show the actual invocations (`MCP · <server>` for direct and deferred
  tools, `Run code` for codemode scripts).

## RPC integration

No new RPC records were added. Orbit keeps the existing `PiClient` event dispatcher and
adds no parallel event pipeline:

- Tool start/update/end events already carry MCP tool names and results.
- Configuration changes are applied by restarting the RPC process and re-issuing
  `switch_session`; the session state is recovered from the session file.

If Pi later exposes MCP commands over RPC, `McpManager` can switch from the file/probe path
to them without touching the UI.

## Testing and manual verification

Unit tests cover parsing, serialization, validation, merging, atomic writes, scope moves,
enable/disable, secret storage/pruning/redaction, probe parsing, and fingerprint staleness
(`cargo test -p orbit-pi mcp`). The guard policy tests cover MCP classification
(`node --test contrib/orbit-guard-extension`).

Manual checks:

1. **Add a stdio server** — Settings → MCP → Add MCP Server; save. The page shows
   `Checking connections…`, then the server's real status and tool count.
2. **Disable / enable** — the toggle flips `enabled` in the defining file and restarts a
   live session; tools disappear from the model and come back.
3. **Test connection** — the refresh action next to a server shows a toast with Pi's own
   error text (stderr tail for stdio, HTTP status for remote).
4. **Project scope** — add a server with Scope: This Project; it appears under
   `This Project` and lives in `<project>/.pi/mcp.json`.
5. **Persisted restart** — quit and relaunch Orbit; the servers are still listed and
   reconnect.
6. **Kill a server** — the next probe reports `Disconnected`/`Failed`; Orbit and Pi stay
   responsive.

## Troubleshooting

- **`Connection failed` with `spawn … ENOENT`** — the stdio command is not installed or not
  on Pi's `PATH`. Install it (for example Node.js/npm for `npx`) or use an absolute path.
- **`Connection failed` with HTTP 401/403** — check the `Authorization` header and the
  `${NAME}` secret it references. Use Test connection; the toast carries Pi's status.
- **`Sign-in required`** — OAuth server. Use **Sign in** on the row (or `pi mcp login <server>`
  in a terminal); the next probe picks up the stored credentials.
- **Project servers missing from the probe** — the project is not trusted by the standalone
  `pi mcp list`. Orbit sessions pass `--approve`, so they still load; the page shows Pi's
  note.
- **Tools do not reach the model** — check the server's exposure and the global
  `autoEnableCodemode` setting. Codemode tools need codemode (or `tool_search`) enabled.
- **A config file is rejected** — the page lists the exact path and entry; fix the entry or
  remove it. Orbit never rewrites a malformed file.
- **Logs** — Pi appends server log messages to `~/.pi/agent/mcp.log`. Orbit adds no secret
  values to any log.

## Extending

The domain model is deliberately small so these can be added without redesign:

- **Presets / marketplace** — an `McpPreset` layer can render recommendations into the same
  add form and `McpServerDef`; nothing in `McpManager` needs to change.
- **OAuth sign-in UI** — call `pi mcp login` through the existing one-shot `run_pi` pattern
  and refresh the probe.
- **Per-session MCP selection** — sessions already stamp a config fingerprint; a session
  could instead pass an explicit config path or environment.
- **Keychain secrets** — replace `mcp/secrets.rs`'s file backend; the manager only uses
  `set`/`env`/`redact`/`prune_unreferenced`.
- **Live status** — if Pi gains MCP status events over RPC, `McpManager::apply_probe` is the
  single ingestion point.
