# Workflow Modes — Plan / Build / Ask (with todo progress)

Status: **Implemented** — all phases landed (see Progress below)
Owner: TBD
Decisions locked (recommended defaults, chosen because the user did not override them):

- **Per-session scope** — each session carries its own mode, chosen at creation and
  switchable mid-session; parked sessions keep independent modes.
- **Tools + system prompt** — Plan/Ask disable mutating tools through
  `pi.setActiveTools` and gate bash to a read-only allowlist, *and* inject
  mode guidance via `before_agent_start`.
- **New Task + composer chip** — initial mode on the New Task page, a composer
  chip to switch after the session starts.
- **Manual handoff** — the user flips the chip Plan → Build when the plan is ready.
- **Todo progress bar (Plan + Build)** — a plan becomes a live checklist shown at
  a slim strip above the composer, with `N/M` progress that updates as each step
  completes. Owned by the extension, rendered natively by Orbit.

## Progress

- **Phase 0 — done.** `src/workflow.rs`: `WorkflowMode` (Plan/Build/Ask, Build
  default), per-session store `~/.orbit-pi/workflow.json` (atomic write,
  `load_for`/`persist_for`/`prune`), and the `WorkflowTodos` reducer with 9 unit
  tests.
- **Phase 1 — done.** `contrib/orbit-workflow-extension/` with `policy.js`
  (tool subtraction, conservative bash allowlist, plan extraction, `[DONE:n]`
  marking, guidance) and `index.js` (session_start / before_agent_start /
  tool_call / turn_end / agent_end / context hooks); embedded in
  `bundled_extensions.rs`. 18 `node --test` tests green.
- **Phase 2 — done.** Composer workflow chip + keyboard popover, `WorkflowMenu`
  actions/keybindings, status/toast, read-only lock glyph, i18n keys.
- **Phase 3 — done.** `workflow_progress` strip above the composer (slim
  collapsed row + expandable checklist), fed by the shared `get_entries` poll.
- **Phase 4 — done.** New Task **Mode** field + `workflow_pending`, committed on
  the new session id.
- **Phase 5 — done.** Session-scoped plumbing: `reset_quota_entries` loads the
  mode / commits pending / resets todos; `prune_workflow_store` on list reload;
  resume via session entries.
- **Phase 6 — done.** Docs (AGENT / PRODUCT / INTENT D8 / README / CHANGELOG),
  locale regeneration.

## Problem

Today every Orbit session is one undifferentiated chat. The README roadmap calls
for sessions scoped as **Plan Mode** (explore and produce a plan, change
nothing), **Build Mode** (normal agent work — the default), or **Ask Mode**
(read-only question answering). This is distinct from, and orthogonal to,
**Access Modes**:

| | Access Mode | Workflow Mode |
|---|---|---|
| Question | *May this mutating call run?* | *What is this session for?* |
| Scope | Global policy, one file | Per session |
| Mechanism | `tool_call` confirmation guard | `setActiveTools` + prompt injection |
| Values | Supervised / Auto-accept edits / Full access | Plan / Build / Ask |

A session can be Plan + Supervised, or Build + Full access. The two features
must compose without fighting.

Beyond scoping, Plan and Build need to show **what the work is**: the plan as a
checklist, with progress that advances as steps complete.

## Relevant prior art in this repo

- **Access Modes** (`src/access.rs` + `contrib/orbit-guard-extension/`) — the
  template for a mode enum, a file the extension reads fresh, and a composer
  chip + keyboard popover.
- **Quota bridge** (`contrib/orbit-quota-extension/` + `src/quota.rs` +
  `app/events.rs::on_entries_response`) — the template for **extension → Orbit
  structured state**: the extension appends keyed custom session entries and
  Orbit polls `get_entries` with a cursor and reduces them into UI state. This
  is how todo progress reaches the app.
- **Extension widgets** (`src/widgets.rs` + `app/view.rs::extension_widgets_*`)
  — a generic `ctx.ui.setWidget` text-block surface above/below the composer.
  **Not** used for the todo bar: raw styled lines cannot be a real progress UI.
  pi's own `plan-mode` example uses `setWidget`; we deliberately do better.

## Architecture

### 1. Mode model — `crates/orbit-pi/src/workflow.rs` (new)

Mirror `src/access.rs`, but **per session**.

```rust
pub enum WorkflowMode { Plan, Build, Ask }   // Build is Default

impl WorkflowMode {
    pub const ALL: [WorkflowMode; 3];
    pub fn as_wire(self) -> &'static str;      // "plan" | "build" | "ask"
    pub fn from_wire(value: &str) -> Self;     // unknown -> Build
    pub fn label(self) -> String;              // tr!("workflow.plan") …
    pub fn description(self) -> String;        // one-line picker hint
    pub fn icon(self) -> &'static str;         // icons/*.svg
    pub fn is_read_only(self) -> bool;         // Plan | Ask
    pub fn tracks_todos(self) -> bool;         // Plan | Build
}
```

Persistence — `~/.orbit-pi/workflow.json`, a **map keyed by pi session id**:

```json
{ "0192…-uuid": "plan", "0193…-uuid": "build" }
```

- `load_for(session_id) -> WorkflowMode` (missing/unknown → Build).
- `persist_for(session_id, mode)` — atomic (temp-file + rename), because the
  extension reads it fresh while a pi process is running.
- `set_pending(mode)` / `take_pending()` — the mode chosen on the New Task page
  before a session id exists. Applied when `new_session` / `get_state` returns
  the id.
- `prune(known_session_ids)` — drop entries whose session is gone (called when
  the session list refreshes), so the file cannot grow without bound.

Rationale for per-session over a single global value: `access.json` can be
global because approval policy is a property of the operator; workflow scope is
a property of the *task*. Up to six parked sessions run concurrently, so a
global file would leak Plan into a Build session.

### 2. Todo model — also `src/workflow.rs`

```rust
pub struct WorkflowTodo { pub step: u32, pub text: String, pub done: bool }

#[derive(Default)]
pub struct WorkflowTodos {
    todos: Vec<WorkflowTodo>,
    session: Option<String>,
}
impl WorkflowTodos {
    pub fn on_entries(&mut self, entries: &[serde_json::Value]) -> bool; // newest orbit:workflow-todos wins
    pub fn reset_for(&mut self, session: Option<&str>);                 // on session switch
    pub fn todos(&self) -> &[WorkflowTodo];
    pub fn progress(&self) -> (usize, usize);   // (done, total)
    pub fn current(&self) -> Option<&WorkflowTodo>; // first not-done
    pub fn is_empty(&self) -> bool;
}
```

Pure and unit-tested, exactly like `src/quota.rs`. Orbit **does not parse plan
text** — the extension owns parsing and step state, and hands Orbit a structured
snapshot. Orbit only reduces and paints.

### 3. Enforcement + todo state — `contrib/orbit-workflow-extension/` (new)

A fourth bundled extension, loaded with `--extension` on every session exactly
like the guard. Split for offline tests (the guard's `policy.js` pattern).

**`policy.js`** — pure decision table:

```js
export const MODES = ["plan", "build", "ask"];
export const DEFAULT_MODE = "build";
export const TODO_ENTRY_TYPE = "orbit:workflow-todos";

// Per mode: tools to force off, and the bash allowlist.
// Derive the base tool set from pi.getActiveTools() and *subtract*, never
// hardcode the full set (pi owns tool names; unknown names are ignored).
export const DISABLED_TOOLS = { plan: ["edit", "write"], ask: ["edit", "write"], build: [] };
export function allowedTools(mode, active) { /* active minus disabled */ }
export function isSafeCommand(command) { /* conservative shell parse */ }
export function decide(mode, toolName, input) { /* "allow" | "block" */ }
export function guidance(mode, todos) { /* system-prompt addendum, or "" */ }
export function extractPlanSteps(text) { /* numbered steps under a `Plan:` header */ }
export function markCompletedSteps(text, todos) { /* apply [DONE:n] tags */ }
```

- `isSafeCommand` must reject compound commands. Block if **any** segment split
  on `;`, `&&`, `||`, `|` is not allowlisted, and reject redirects (`>`, `>>`)
  and command substitution (`$(`, backticks). Adapt the conservative patterns
  from pi's `examples/extensions/plan-mode/utils.ts`; do not vendor the file.
- Allowlist (read-only): `cat head tail less more grep rg find fd ls pwd tree
  wc file stat du df sort uniq diff which whereis uname whoami id date uptime ps
  jq bat eza`, `git status|log|diff|show|branch|remote|ls-*`, `npm list|outdated`,
  `yarn info`. This is a guard, not a sandbox — same honesty caveat as Access
  Modes.
- `extractPlanSteps`: find the last `Plan:` header in an assistant message and
  parse `N. text` lines; reuse pi's `cleanStepText` normalizations. Returns
  `[{step, text, done:false}]`.
- `markCompletedSteps`: apply `[DONE:n]` tags found in assistant text.
- `guidance`:
  - Plan → read-only exploration; produce a numbered plan under a `Plan:`
    header; do not make changes.
  - Build → if todos exist, list the remaining steps and instruct: execute in
    order, and include `[DONE:n]` after finishing step `n`.
  - Ask → answer from the codebase; do not modify anything; no plan unless asked.

**`index.js`**:

- Resolve the mode **fresh on every hook**:
  `mode = readStore(ctx.sessionManager.getSessionId()) ?? process.env.ORBIT_WORKFLOW_MODE ?? "build"`.
  Reading fresh is what lets a mid-session chip change re-arm the live process
  with no restart — the contract the access guard relies on.
- `session_start`: reconstruct todos from the branch (`ctx.sessionManager
  .getBranch()`, scanning `message` tool results and the newest
  `orbit:workflow-todos` custom entry — branch-aware, so a rewind is correct);
  apply `pi.setActiveTools(allowedTools(...))`.
- `before_agent_start`: re-resolve; if the mode changed, re-apply tools; return
  `{ message: { customType: "orbit-workflow-context", display: false,
  content: guidance(mode, todos) } }` when non-empty. Use the hidden
  custom-message form (not `systemPrompt`) so the transcript keeps recording
  structured sections and prefix caching survives.
- `tool_call`: defense in depth. Even with `setActiveTools`, block `edit`/`write`
  in Plan/Ask and block unsafe `bash`, returning
  `{ block: true, reason: "Plan mode: …" }`. Catches a tool pi preflighted
  before the tool set changed.
- `turn_end` (`event.message`) and `agent_end` (`event.messages`): update the
  todo list — in Plan/Build, re-extract steps from a fresh `Plan:` section if
  present; mark `[DONE:n]` steps complete. Persist with
  `pi.appendEntry("orbit:workflow-todos", { todos })` **only when the snapshot
  changed**, so the entry log stays small.
- `context`: when the mode is Build, filter out stale
  `orbit-workflow-context` messages (pi's example does this) so a finished Plan
  phase does not keep steering a Build session.
- `appendEntry("orbit-workflow", { mode })` whenever the mode changes, so the
  mode is recorded in pi's own session file and survives resume even if the
  Orbit store is lost. Never sent to the model.
- Fail-safe: any resolve/read failure degrades to **Build** (the neutral mode),
  never to a silently-weaker read-only state.

Tests (`node --test`, like the guard): `policy.test.mjs` (allowlist, compound
rejection, tool subtraction, plan extraction, `[DONE:n]` marking, guidance per
mode) and `index.test.mjs` (hook wiring with a fake `pi` + fake
`ctx.sessionManager`, including entry append-on-change and session reconstruction).

### 4. Bundle wiring — `crates/orbit-pi/src/bundled_extensions.rs`

- `const WORKFLOW_INDEX_JS` / `WORKFLOW_POLICY_JS` via `include_str!`.
- `workflow: Option<PathBuf>` field, `install_workflow_extension()`, accessor,
  and add it to the `spawn` extension vector.
- Optional: pass `ORBIT_WORKFLOW_MODE` on the spawned `Command` as the
  pre-store fallback; the pending mode is otherwise written before the first
  prompt (see Phase 5).
- Update the module header's list of bundled extensions.

### 5. Entry poll — extend the quota path, don't duplicate it

`src/app/events.rs::on_entries_response` already polls `get_entries` with a
cursor and feeds `self.quota`. Feed `self.workflow_todos` from the same array:

```rust
let changed_quota = self.quota.on_entries(entries);
let changed_todos = self.workflow_todos.on_entries(entries);
if changed_quota || changed_todos { cx.notify(); }
```

Reset `workflow_todos` on session switch (where `quota_entries_cursor` resets
today) so a stale plan never paints on another session.

### 6. App state + UI

`src/app.rs` — beside the access-mode cluster:

```rust
workflow_mode: WorkflowMode,             // active session's resolved mode
workflow_pending: Option<WorkflowMode>,  // New Task choice before a session id
workflow_menu_open: bool,
workflow_menu_highlight: usize,
workflow_menu_focus: FocusHandle,
workflow_todos: WorkflowTodos,
workflow_todos_expanded: bool,
```

`src/app/composer_ops.rs` — copy the access-menu handlers, renamed:
`toggle_workflow_menu`, `close_workflow_menu`, `on_workflow_menu_next/prev/
confirm/close`, `run_workflow_menu_item`, `set_workflow_mode`. Plus
`toggle_workflow_todos`. `set_workflow_mode` persists for the current session id
(or sets pending), re-applies nothing in Rust (the extension re-reads), and
posts the honest status/toast (`workflow.mode_set` /
`workflow.mode_set_unavailable` when the extension failed to install).

`src/app/view.rs`:

- `workflow_chip(cx)` + `workflow_popup(cx)` — same construction as
  `access_chip` / `access_popup` (icon tile, label over hint, accent check,
  hover-follows-highlight, caret turn). Different icon per mode.
- Add `.child(self.workflow_chip(cx))` to the `composer_row` controls
  (next to `access_chip`, before the `flex_1` spacer), gated by `!compact`.
- `workflow_progress(cx)` — the plan strip, rendered **above the composer**,
  in the `composer-column` after `extension_widgets_above` and before the
  composer box. Hidden when `workflow_todos.is_empty()` or the mode is Ask.
- New Task page: a labeled **Mode** select field beside the Workspace field,
  reusing the workspace select-field styling (`view.rs` ~1970–2160). It writes
  `workflow_pending`.

`src/app/session.rs`:

- On new session / `get_state` returning a `sessionId`: if `workflow_pending`
  is set, `persist_for(id, mode)` and clear pending; else
  `workflow_mode = load_for(id)`. Reset `workflow_todos` for the session.
- On session switch (including warm-pool resume): reload `workflow_mode` and
  reset todos for the target session.
- On session list refresh: `WorkflowMode::prune(known_ids)`.

### 7. The todo progress strip above the composer (Plan + Build)

A single `workflow_progress` element with two states, rendered directly above
the composer (after the extension widgets, before the composer box). Shown
whenever a plan exists and the mode is Plan or Build (never Ask). Kept quiet
on purpose so it does not compete with the composer.

**Collapsed (default)** — one comfortable row: the current step on the left, a
meter + count + chevron on the right. A slim 14px gutter and 12–13px type keep
it the same weight as the app's other panels — a real section, not a cramped
status line.

```
┌────────────────────────────────────────────────────────────┐
│  ▤  Add the reducer tests        ▓▓▓▓░░░░  3/14   ›        │
└────────────────────────────────────────────────────────────┘
```

- Gutter `px(14)`, 30px header row, 12px gaps; the whole row is one click
  target. Hover is the repo's row-hover token `theme.bg_hover` over a pill
  inset by the gutter — not an overlay wash bleeding to the card edges — and
  the `BUTTON_GROUP` hover brightens the icons for extra feedback.
- The meter is 72×6px: a visible track (`overlay_strong`) with an accent fill
  (success tint once all done), so 0-of-N still reads.
- Current step truncates at 12.5px; when all are done the row shows a green
  check and "All steps complete".
- Count is 12px medium; `done/total` sits beside the meter.

**Expanded** — the checklist appears below a hairline separator:

- Rows are 12.5px with 14px icons, `px(6) py(4)` and 12px gaps, sharing the
  header's 6px inner inset so icons and text line up on one grid.
- Hierarchy: the current step is at full strength on an accent-tinted row,
  pending steps are a shade back, and completed rows recede behind a strike.
- Long plan steps clamp to two lines, so rows keep a regular rhythm.
- Capped height (260px) with its own scroll; the transcript keeps its space.

The `[DONE:n]` tags themselves are protocol, not prose: `transcript.rs` strips
them when an assistant message is decoded (live finalize and `get_messages`
reload alike), so completion shows as a checkmark here and never as raw tags in
the transcript, its copy, or its search.

Placement: above the composer, persistent like the queue bar, not modal — not a
⌘J-style resizable panel, and not below the composer where it read as bloat.

### 8. i18n

Add to `locales/en.yml` (source of truth): `workflow.plan`, `workflow.build`,
`workflow.ask`, `workflow.plan_hint`, `workflow.build_hint`, `workflow.ask_hint`,
`workflow.mode_set`, `workflow.mode_set_unavailable`,
`workflow.todos_progress` (`%{done}/%{total}`), `workflow.todos_done`
("All steps complete"), and `view.mode`.
Then add the English strings to the matching `scripts/i18n_glossary_*.py`, run
`python3 scripts/gen_locales.py`, and add the new keys to the
`newly_localized_ui_keys_translate` guard in `src/i18n.rs`. (The new keys fall
back to English until glossary translations land, matching the repo's other
fallbacks.)

### 9. Docs

- `AGENT.md`: repo-layout rows for `src/workflow.rs` and
  `contrib/orbit-workflow-extension/`; a `Workflow modes` row in the Current
  state table (mode chip + todo bar); mention the entry poll.
- `PRODUCT.md`: move Workflow modes from Roadmap to Current with the final
  behavior, including the progress bar.
- `INTENT.md`: add a decision entry (D8) for extension-based workflow scoping
  and the `orbit:workflow-todos` entry contract, noting the protocol gap (no RPC
  for tools/prompt) and the per-session store.
- `README.md`: tick the contributor-checklist item and move the roadmap row to
  Shipped.
- `CHANGELOG.md`: `## [Unreleased]` → `### Added`.

## Behavior matrix

| | Plan | Build (default) | Ask |
|---|---|---|---|
| `edit` / `write` | disabled + blocked | normal | disabled + blocked |
| other active tools | kept (read/grep/find/ls/task) | all | kept |
| `bash` | read-only allowlist | normal | read-only allowlist |
| prompt guidance | numbered plan under `Plan:` | execute remaining steps, tag `[DONE:n]` | answer only, no changes/plan |
| todo bar | shown; `[DONE:n]` also advances it | shown; tracks `[DONE:n]` | hidden |
| persisted | session entry + store | session entry + store | session entry + store |
| on resume | mode + todos restored | mode + todos restored | mode restored |

## Phases

| Phase | Deliverable | Exit gate |
|---|---|---|
| 0 | `src/workflow.rs`: mode enum + per-session store + `prune` + `WorkflowTodos` reducer, unit-tested | `cargo test -p orbit-pi workflow` green; no UI yet |
| 1 | `contrib/orbit-workflow-extension/`: `policy.js` (allowlist, plan extraction, `[DONE:n]`) + `index.js` hooks + node tests; bundle wiring | `node --test` green; in a live session a Plan session cannot `edit`, an unsafe `bash` is blocked, and `orbit:workflow-todos` entries appear |
| 2 | Composer chip + popup + status/toast + i18n keys | Chip switches mode mid-run and the running pi re-arms without restart |
| 3 | **Todo progress bar**: `workflow_progress` collapsed + expanded, entry-poll wiring, reduce-motion, step-flash | A plan paints 0/N, advances to N/N as `[DONE:n]` land, and collapses/expands |
| 4 | New Task **Mode** field + `workflow_pending` | A task started as Plan is Plan on first prompt; Build default unchanged |
| 5 | Session-scoped plumbing: load on switch, persist on create, resume, prune, todos reset | Two parked sessions hold different modes *and* plans; reopening restores each |
| 6 | Docs, CHANGELOG, roadmap ticks, polish | `cargo clippy --workspace --all-targets` clean; docs consistent |

## Risks / open questions

- **Tool-name drift.** Tool names belong to pi. Derive the base set from
  `pi.getActiveTools()` and subtract `edit`/`write`; never hardcode the full
  set. The `tool_call` block is the backstop for a renamed write tool.
- **Bash allowlist is a guard, not a sandbox.** Compound commands, redirects,
  and command substitution must be rejected conservatively; a false *allow* is
  worse than a false block. State this in the UI as Access Modes do.
- **`[DONE:n]` compliance.** Progress depends on the model tagging completed
  steps. Mitigations: state it plainly in `guidance`, inject the remaining list
  each Build turn, and *also* accept an explicit `Plan:` re-emit. If this proves
  unreliable, the fallback is an extension-registered `todo` tool (pi's
  `examples/extensions/todo.ts` pattern) that the model calls to toggle steps —
  structured, at the cost of a tool call. Manual check-off in the UI is a later
  enhancement (it needs a UI → extension channel, e.g. a control file the
  extension reads fresh).
- **Mode × Access interaction.** Plan + Full access still cannot edit (tools are
  gone); Plan + Supervised still prompts for the bash calls that survive the
  allowlist. Document precedence: workflow narrows the tool set first, access
  mode then governs what remains. No change to `access.rs`.
- **Mid-session re-arm race.** The extension reads the store fresh per hook,
  matching the guard's proven pattern; a mode change during a streaming turn
  applies at the next `before_agent_start`/`tool_call`. Documented; no restart.
- **Todo entry churn.** Append only on change, and Orbit's cursor ignores
  already-seen entries, so a long Build does not grow the poll payload.
  Branch/rewind correctness comes from reconstructing over `getBranch()`.
- **Session id timing.** A brand-new session's id arrives after `new_session`.
  The New Task mode rides `workflow_pending` and is written before the first
  prompt; `session_start` may briefly arm Build, which `before_agent_start`
  corrects. The optional spawn env var removes even that window.
- **Resume vs. store divergence.** The session entries are authoritative for pi;
  the Orbit store is authoritative for the app. On conflict (e.g. a mode changed
  from the pi CLI), load from the store and let the extension append a fresh
  entry — one source of truth per process, reconciled on switch.
- **Out of scope:** automatic Plan → Execute handoff (manual chip switch stays),
  manual checkbox toggling, per-step timing/estimates, and any new RPC surface.

## Verification

- `cargo test -p orbit-pi workflow` and `cargo test -p orbit-pi i18n`.
- `node --test contrib/orbit-workflow-extension/*.test.mjs`.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- Manual matrix: new task in each mode; switch Plan → Build mid-run; a plan
  paints and its progress advances as steps complete; reopen a parked Plan
  session (mode + todos restored); resume after quit; Plan bash blocked vs.
  allowed; Plan `edit` blocked; Ask answers without touching files; Build
  unchanged; access guard still prompts as before.
