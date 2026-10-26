# AGENT.md — Working on Orbit

Project guidance for agents and humans. Read this before touching the repo.

When this file conflicts with `INTENT.md` on architecture choices, **`INTENT.md` wins**.
Update `INTENT.md` if a recorded decision (D1–D6) changes.

## What this project is

**Orbit** is a native desktop workbench for the [pi coding agent](https://github.com/earendil-works/pi) —
a chat-style GUI rendered **entirely in Rust** on **GPUI** (Zed's GPU-accelerated UI framework),
speaking the **pi CLI's RPC protocol** directly over stdio.

The reference architecture is [Waku](https://github.com/egoist/waku): native Rust UI on the GPU,
pi as a child process, no web, no webview, no Node daemon.

**Product docs:** `PRODUCT.md` (positioning, capabilities, constraints) · `README.md` (status, setup) ·
`INTENT.md` (decisions and rationale).

## Hard rules

1. **No web anywhere in the UI.** Do not reintroduce React, Vite, Tauri, webviews, DOM, Tailwind, or Node tooling. New UI is GPUI only. The legacy `src-tauri/` tree has been deleted; never rebuild it.
2. **The pi CLI is the only agent runtime.** Spawn `pi --mode rpc` as a child process per open session and speak newline-delimited JSON over stdio (`crates/orbit-rpc`). There is no Node daemon.
3. **GPUI is pre-1.0 and pinned.** `crates/orbit-pi` uses `gpui = { version = "0.2.2", features = ["runtime_shaders"] }`. Upgrade deliberately on a schedule, never track `main`.
4. **Performance is a product requirement.** The transcript and sidebar are virtualized (`list()`). The UI drains RPC events on a ~90 ms heartbeat — never block a frame with I/O. Syntax highlighting, when it lands, must be paint-only so streaming never reflows.
5. **Trust pi's truth.** Render real RPC events and on-disk state; nothing decorative pretending to be functional. Access modes the protocol can't deliver are shown as unavailable (or as a static fact), not faked. The sidebar **Search** row opens the command palette (sessions/commands/settings) — it is not a transcript-content search; do not wire a fake one.
6. **A library call is looked up, not remembered.** Every dependency is pinned to the version in `Cargo.toml` / `marketing/package.json`. Read that version's docs (or its source) and follow the call site already in the repo before writing a new one — see *Library reference* below. If the docs and the repo disagree, the repo wins; if both leave you guessing, ask instead of inventing.
7. **Shipping changes the docs, not just the code.** When a feature ships, update `INTENT.md` → *Implementation status* (and add or revise a `D#` decision when the architecture changed), refresh the `README.md` features/roadmap, and add a `CHANGELOG.md` `[Unreleased]` entry. A feature is not done until `INTENT.md` describes it.

## Repo layout

```
Cargo.toml              workspace: orbit-pi, orbit-rpc
crates/orbit-pi/        GPUI app — window, shell, chat, settings
  src/main.rs           bootstrap, assets, keybindings, native menu (localized)
  src/main_window.rs    opens the one main window and answers macOS's Dock reopen request
                        (`Application::on_reopen`); owns the `Shell` global that parks the `OrbitApp`
                        entity so a reopened window resumes the same session and pi process, and
                        spawns the heartbeat on the app (not the window) so a parked session keeps
                        draining while no window is open
  src/commands.rs       command registry (D13): keymap rows, palette metadata, and the shortcut reference
  src/i18n.rs           AppLanguage + locale detection; the `tr!`/`tr_cow!` macros
  locales/              translations: en.yml is the source of truth, one file per locale
                        (generated from en.yml + scripts/i18n_glossary*.py)
  src/app.rs            OrbitApp state model + shared types + controller wiring (module map in the file header)
  src/backdrop.rs       the new-task page's backdrop: a painted ordered-dither field (8×8 Bayer screen,
                        four dot sizes) with the app monogram screened into it and a light that crosses it
  src/app/runtime.rs    pi process lifecycle, status/error, provider auth
  src/app/events.rs     heartbeat event drain, RPC response routing, session/workspace watchers
  src/app/session.rs    prompt/queue/turn lifecycle, abort, session switching + navigation
  src/app/pickers.rs    model, command-palette, branch, and new-task workspace pickers
  src/app/composer_ops.rs autocomplete, attachments, "+" add-menu, model/thinking chips
  src/app/sidebar.rs    sidebar rows + session/workspace row menus; project-list prefs
  src/app/settings.rs   Settings surface: General/Runtime/Agent/Skills/Plugins/Models/Appearance/Providers/About (+ provider CRUD, plugin install, model list)
  src/app/skills_ui.rs  Settings → Skills master-detail page and its controllers
  src/app/mcp_ui.rs     Settings → MCP: server list, add/edit modal, probe/apply lifecycle
  src/mcp.rs            MCP domain model: servers, transports, exposure, status, typed errors,
                        secret-reference parsing and error redaction (no MCP protocol code)
  src/mcp/config.rs     reads/writes/validates/merges pi's own mcp.json files (atomic, surgical)
  src/mcp/secrets.rs    MCP `${NAME}` secret store (~/.orbit-pi/mcp-secrets.json, 0600)
  src/mcp/manager.rs    McpManager: one source of truth + `pi mcp list --json` probe + fingerprints
  docs/mcp.md           MCP architecture, configuration, secrets, lifecycle, troubleshooting
  src/app/updater_ui.rs signed-updater modal (search/Version History/Update now), rows, pill, event drain
  src/app/search.rs     in-transcript find (⌘F): matches, counts, prev/next, row wash
  src/app/toast_ui.rs   in-app toast stack: push/dismiss helpers + the bottom-right layer
  src/app/view.rs       top-level chrome: sidebar, transcript, composer, status bar, onboarding, image lightbox
  src/app/open_in.rs    installed-editor/terminal detection and the "open in" menu
  src/app/worktrees.rs  Worktrees page + controller: list/create/open/rename/move/lock/remove, setup progress, settings rows
  src/app/pi_update_ui.rs launch-time pi self-update: background check, install, toasts
  src/app/helpers.rs    shared UI primitives (icon/file glyphs, the loading spinner, the floating-surface chrome, the empty/error state) + small formatting helpers
  src/auth.rs           non-sensitive provider-auth state machine for the auth.* RPC namespace (login/cancel/timeout/restart recovery)
  src/transcript.rs     virtualized messages (snapshot + stream)
  src/transcript_view.rs Waku-style transcript paint (rail, activity cards, copy); plain-GPUI message rows (no component library)
  src/message_scroller.rs tail-following list + jump-to-latest (original, plain gpui ListState)
  src/composer.rs       multi-line EntityInputHandler (wraps, auto-grows, scrolls)
  src/model_selector.rs model / thinking picker popover
  src/notifications.rs  background notifications + channel prefs in ~/.orbit-pi/notifications.json: desktop banners (UNUserNotificationCenter; its delegate routes a clicked banner back to the session; osascript fallback when unbundled), system alert sound, and in-app toasts (the frontmost-window stand-in for the banner)
  src/toast.rs          the toast stack model: kinds, TTLs, fade, dedupe, cap (painted by app/toast_ui.rs)
  src/pi_update.rs      pi self-update check + delegated install: latest-version fetch (pi.dev), version ordering, `pi update self`, failure extraction
  src/command_palette.rs  ⌘P / ⌘K command palette (sections, fuzzy match, modal scrim layer)
  src/workspace_picker.rs new-task folder selector (recent folders + native browse)
  src/workspace_logo.rs workspace logo lookup for the new-task folder field (conventional paths, `.orbit-pi/icon.*` override, folder glyph fallback)
  src/workspace_mark.rs per-workspace sidebar mark: curated HugeIcons + semantic tints, persisted in workspaces.json
  src/worktree.rs       Git worktree lifecycle: `WorktreeManager` (list/create/remove/move/lock/unlock/prune/repair), porcelain parsing, name validation, worktree root + `.orbit/worktree.json`
  src/worktree_setup.rs optional worktree setup script: resolution, `ORBIT_ROOT_PATH`/`ORBIT_WORKTREE_PATH`, captured stdout/stderr/exit code (async host in app/worktrees.rs)
  src/sessions.rs       reads ~/.pi/agent/sessions (+ debounced session-store watcher)
  src/watch.rs          shared debounced fs watching (workspace tree for Review/Git)
  src/explorer/         workspace file tree + file editor
    walk.rs             gitignore-aware background snapshot (`ignore` crate)
    tree.rs             pure tree: expand/collapse, filter, git badges
    panel.rs            ProjectPanel entity: right dock, virtualized tree
    viewer.rs           FileViewer entity: editable code/text, markdown/image views, autosave
  docs/explorer.md      Explorer design (project panel + Files surface)
  docs/worktrees.md     Git worktrees as first-class workspaces: creation, naming, roots, setup scripts, sessions, locking, external worktrees
  src/assets.rs         include_dir AssetSource (SVGs + app icon)
  src/app_icon.rs       dock icon (macOS; Alpha in debug builds) + Settings → About mark
  src/review.rs         git diff model: sources, parsing, context gaps, changed-files tree
  src/checkpoint.rs     per-turn git snapshot refs (refs/orbit/…) backing Review's Last Turn
  src/highlight.rs      paint-only syntax lexer for diff lines (Waku port, 16 languages)
  src/sidepane.rs       Review pane: virtualized diff, sticky file headers, tree, source menu
  src/git.rs            git plumbing: branch discovery, review diffs, status/staging/history/graph
  src/git_panel.rs      full-page Git surface: Changes / History / Graph + commit bar
  src/terminal.rs       bottom terminal panel (⌘J): tabbed shells (one PTY per tab, background tabs keep
                        running) with each tab's login-shell profile mark; alacritty_terminal VT grid,
                        GPUI canvas render, keys/selection/paste
  src/theme/tokens.rs   Zed design tokens (DynamicSpacing, TextSize, IconSize, ButtonSize, ElevationIndex + elevation_1/2/3, component metrics) resolved through the Theme; see INTENT.md D11
  src/providers.rs      provider catalog + models.json / auth.json read/write for Settings → Providers (provider metadata introspected from pi-ai; curated table is the fallback)
  src/quota.rs          account quota/balance/spend reducer over the `quota.*` RPC and the bridge's session entries (no secrets, no I/O)
  src/access.rs         access mode (Supervised / Auto-accept edits / Full access): persistence to `~/.orbit-pi/access.json`, which the guard extension reads
  src/layout.rs         persisted workbench layout: resizable panel sizes in `~/.orbit-pi/layout.json` (recorded on drag, flushed by the heartbeat once a drag settles)
  src/workflow.rs       workflow mode (Plan / Build / Ask): per-session persistence to `~/.orbit-pi/workflow.json`, which the workflow extension reads
  src/bundled_extensions.rs  materializes the bundled pi extensions (quota bridge + access guard + auto-title + workflow) under `~/.orbit-pi/` and spawns every session with `--extension`
  src/commit_message.rs one-shot, tool-free `pi -p` conventional-commit generation
  assets/icons/         HugeIcons SVGs (MIT) + provider brand marks
  assets/fonts/         SymbolsNerdFont-Regular.ttf
  assets/app-icon.png   512px app mark (from assets/icons/logo-icon.png)
  dev-assets/alpha-app-icon.png  512px Alpha mark; macOS debug-only asset overlay (scripts/make-alpha-icon.sh)
crates/orbit-rpc/       pi CLI process + JSONL protocol
  src/client.rs         spawn, writer/reader/stderr threads, kill-on-drop
  src/types.rs          CommandBody / Event (permissive serde) + auth.* / quota.* wire types
  docs/auth-rpc.md      provider-auth RPC server contract (secrets never cross it)
  docs/quota-rpc.md     provider-quota RPC server contract (normalized, non-secret)
  tests/live_pi.rs      live get_state / prompt / process-alive (skip if no pi)
  tests/live_catalog.rs live get_available_models + thinking levels
  tests/auth_fake_pi.rs scripted-server coverage of the auth.* round trip
  tests/quota_fake_pi.rs scripted-server coverage of the quota.list round trip + `--extension` forwarding
contrib/pi-auth-rpc/    apply.mjs: inject auth.* into an installed pi build
                        (browser OAuth until pi ships the server upstream)
contrib/pi-quota-rpc/    apply.mjs + quota-handler.js: inject quota.list into an
                        installed pi build (per-provider usage adapters)
                        quota-handler.test.mjs: node --test coverage of the
                        network-free adapters (unsupported + dispatch)
contrib/orbit-quota-extension/  Orbit's bundled pi extension (loaded with
                        `pi --extension`): fetches quota via pi's own auth and
                        appends normalized `orbit:quota` session entries.
                        adapters.js is the extension twin of quota-handler.js;
                        index.test.mjs / adapters.test.mjs are `node --test`
contrib/orbit-guard-extension/  Orbit's bundled pi extension (loaded with
                        `pi --extension`): hooks `tool_call`, asks the user
                        (via `ctx.ui.select` → `extension_ui_request`) before
                        mutating calls the active access mode does not
                        auto-approve, offering Allow once / Always allow this
                        tool / Deny. policy.js holds the decision table and
                        the `~/.orbit-pi/access-allow.json` allowlist;
                        policy.test.mjs / index.test.mjs are `node --test`
contrib/orbit-title-extension/  Orbit's bundled pi extension (loaded with
                        `pi --extension`): after a session's first turn
                        settles, asks a model for a short title from the
                        first exchange and sets it with `pi.setSessionName`.
                        Reads `~/.orbit-pi/auto-title.json` (the model is the
                        active session model unless Settings → Agent picks
                        one); title.js is the pure helper set and both
                        files are `node --test` covered
contrib/orbit-workflow-extension/  Orbit's bundled pi extension (loaded with
                        `pi --extension`): scopes a session to Plan/Build/Ask.
                        Plan and Ask drop the write tools (`pi.setActiveTools`),
                        gate `bash` to a read-only allowlist on `tool_call`,
                        and inject mode guidance on `before_agent_start`.
                        Reads the per-session mode from `~/.orbit-pi/workflow.json`
                        fresh on every hook; policy.js is the pure decision
                        table and both files are `node --test` covered
assets/icons/           logo-icon.png app-icon source + icon.icns / icon.ico / icon.png
                        + alpha-logo.png / alpha-logo.icns (macOS debug builds)
PRODUCT.md  INTENT.md  README.md  AGENT.md  PRIVACY.md
```

Workspace edition is **2021**. Prefer **Rust 1.94+**. Root `Cargo.lock` is the lockfile; ignore a nested `crates/orbit-pi/Cargo.lock` if present.

Override the pi binary with `PI_BIN` (default: `pi` on `PATH`).

## Library reference — read the docs before writing usage

Every dependency below is pinned. Do not write a call from memory: look up the
pinned version, then follow the pattern the repo already uses. Look-up order:

1. **This repo.** `rg "gpui::" crates/orbit-pi/src` finds call sites; the file map
   above says which module owns what.
2. **The pinned source** (the authoritative API surface):
   `~/.cargo/registry/src/index.crates.io-*/<crate>-<version>/`, or locally rendered
   docs with `cargo doc -p <crate> --no-deps --open`.
3. **docs.rs at the pinned version:** `https://docs.rs/<crate>/<version>`. Thin or
   missing for pre-1.0 crates — then (2) is the doc.
4. **Upstream precedent:** Zed `crates/gpui` for GPUI, the Waku file map below for
   patterns. Precedent, not a spec.

### Rust — UI and app runtime (`crates/orbit-pi`)

| Crate | Owns here | Docs | Repo rules |
|---|---|---|---|
| `gpui` 0.2.2 | every element, window, input, list, popover | https://docs.rs/gpui/0.2.2 · Zed `crates/gpui` | Pinned pre-1.0: read the 0.2.2 source, never `main`. See *GPUI 0.2.2 API notes*. `list()` needs a definite height; rows are plain flex trees, no component library. |
| `rust-i18n` 4.2 | `t!` lookups, YAML locales | https://docs.rs/rust-i18n/4.2.0 | `locales/en.yml` is the source of truth; other locales are generated. Placeholder binding names must match the key exactly. Never hard-code user-facing English. |
| `rfd` 0.15 | file pickers (attach, screenshots, export) | https://docs.rs/rfd/0.15.0 | Use `AsyncFileDialog`, or gpui's `prompt_for_paths` for folders. **Never** the blocking `FileDialog`: it runs a nested main-thread modal loop that re-enters the app and panics with `already borrowed`. |
| `include_dir` 0.7 | `assets/` as a `gpui::AssetSource` | https://docs.rs/include_dir/0.7.0 | Packaged builds must not read assets from cwd. |
| `ignore` 0.4 | gitignore-aware workspace walk | https://docs.rs/ignore/0.4.0 | Walk off the UI thread and post a snapshot; do not hand-roll `read_dir`. |
| `notify-debouncer-mini` 0.7 | session-store and workspace watchers | https://docs.rs/notify-debouncer-mini/0.7.0 | Debounce, scan off-thread, only drain a list on the heartbeat. |
| `serde` / `serde_json` 1 | JSONL envelopes, on-disk stores | https://serde.rs | Permissive protocol parsing: unknown shapes pass through as raw JSON; no `deny_unknown_fields` on RPC types. |
| `anyhow` 1 | context on fallible plumbing | https://docs.rs/anyhow/1 | `Context` at boundaries; no `unwrap`/`expect` in a render path or on RPC data. |
| `ed25519-dalek` 2 | release-signature verification | https://docs.rs/ed25519-dalek/2 | Verify before writing bytes to disk. No skip or fallback path. |
| `chrono` 0.4 | local timestamps | https://docs.rs/chrono/0.4 | `default-features = false` + `clock`; formatting only. |
| `base64` 0.22 | image payloads over RPC | https://docs.rs/base64/0.22 | Prompt attachments only, never state files. |
| `image` 0.25 | backdrop decode, resize, blur, dither | https://docs.rs/image/0.25 | Decode once into the cache (`dither::background()`), bounded by `MAX_EDGE`/`BLUR_MAX`; never re-decode per frame. |
| `devicons` 0.6 | file-type glyphs | https://docs.rs/devicons/0.6 | Needs the bundled Nerd Font; resolve the family once per frame, fall back to text. |
| `unicode-segmentation` 1 | grapheme clusters | https://docs.rs/unicode-segmentation/1 | Caret and selection math is grapheme-based; never index a string by byte. |
| `futures` 0.3 | `BoxFuture` + oneshot for the image loader | https://docs.rs/futures/0.3 | Only the pieces gpui's traits need. |
| `reqwest` (as `zed-reqwest`) 0.12.15-zed | update feeds, avatars, GitHub metadata | https://docs.rs/zed-reqwest | Same crate version gpui links — never add a second `reqwest`. Off-thread, and only traffic the user asked for (see *Privacy*). |
| `url` 2.5 | update-feed URLs, `termy://` deep links, MCP validation | https://docs.rs/url/2.5 | Parse, never string-concat a URL. |
| `alacritty_terminal` 0.26 | terminal grid + VT/ANSI | https://docs.rs/alacritty_terminal/0.26 | `default-features = false`. It owns parsing and the grid; `terminal.rs` owns PTY threads and paint. No second terminal crate. |
| `crossbeam-channel` 0.5 | PTY → UI event channel | https://docs.rs/crossbeam-channel/0.5 | Nothing on the writer thread may block on the UI. |
| `flate2` 1 / `tar` 0.4 | unpack the signed Unix update | https://docs.rs/tar/0.4 | Unpack into a fresh directory; verify before swapping. |
| `libc` 0.2 | `kill(pid, 0)` liveness probe | https://docs.rs/libc/0.2 | Unix only, smallest possible surface. |

### Rust — platform

| Crate | Owns here | Docs | Repo rules |
|---|---|---|---|
| `cocoa` `=0.26.0` | Dock icon when running unbundled | https://docs.rs/cocoa/0.26.0 | Pinned to gpui's own pin; `#[cfg(target_os = "macos")]` only. |
| `objc2` 0.6 + `objc2-app-kit` / `objc2-foundation` / `objc2-user-notifications` 0.3 + `block2` 0.6 | window chrome, "open in" icons, notification banners, locale | https://docs.rs/objc2/0.6 | One objc2 generation across the tree; enable only the framework features actually used; every `unsafe` block carries a comment. |
| `windows-sys` 0.61 | Win32 shell, GDI, window messages | https://docs.rs/windows-sys/0.61 | `#[cfg(windows)]`; features are opt-in. |
| `raw-window-handle` 0.6 | HWND for window drag and the caption | https://docs.rs/raw-window-handle/0.6 | Must stay on gpui's version. |
| `embed-resource` 3 (build) | exe icon, version metadata, manifest | https://docs.rs/embed-resource/3 | gpui's `windows-manifest` feature stays off — a second `RT_MANIFEST` fails CVTRES with `CVT1100`. |

`crates/orbit-rpc` depends on `serde`, `serde_json`, and `anyhow` only. Keep it
that way: it is a transport, not a place for UI, HTTP, or runtime helpers.

### Node — bundled pi extensions (`contrib/`)

Zero npm dependencies: plain ESM, Web Platform APIs, and `node --test` for
coverage. The extension API is documented **in the installed pi package**, not
the web — read it before adding a hook:

```
~/.pi/agent/install/releases/$(cat ~/.pi/agent/install/current-version)/node_modules/@earendil-works/pi-coding-agent/docs/
  extensions.md        hooks, ctx.ui, ctx.modelRegistry, setActiveTools
  rpc-extension-ui.md  how ctx.ui.* travels over RPC as extension_ui_request
  rpc.md, settings.md, security.md
```

Rules: fail closed (block the tool call when no UI can confirm), read
`~/.orbit-pi/*.json` fresh on every hook so a settings change re-arms live
sessions, keep the decision table in a pure module (`policy.js`, `title.js`,
`adapters.js`) with a `.test.mjs` beside it, and never add a package — these
files are materialized under `~/.orbit-pi/` and run in the user's pi process.

### Frontend — `marketing/` only

`next` 16.3.5 · `react` / `react-dom` 19.2.8 · `@base-ui/react` 1.8 ·
`@hugeicons/react` · `tailwindcss` v4 · `shadcn` 4 · `tw-animate-css`.
Docs: https://nextjs.org/docs · https://react.dev · https://base-ui.com/react ·
https://tailwindcss.com/docs · https://ui.shadcn.com.

This tree is the landing page, not the app: nothing here may be imported by
`crates/orbit-pi` (Hard rule 1). Use the existing primitives in
`marketing/src/components/ui` and the site's tokens in `globals.css`; do not add
a second component library.

### CLIs Orbit shells out to

| Tool | Used for | Rules |
|---|---|---|
| `pi` | the agent runtime, `pi mcp list --json`, `pi update self` | One child per open session. Docs: the installed package path above; override with `PI_BIN`. |
| `git` | every Git operation, worktrees, plugins from git URLs | No `git2`/libgit2. Run off-thread, read the exit code and stderr, and surface them. |
| `gh` | issues, pull requests, auth status, attachments | May be missing or signed out: branch on that instead of assuming, and fall back to the web flow. |
| `node` | extension and plugin installs, `node --test` | The first-run check reports it missing; never shell to it from a render path. |
| `python3` | repo scripts, stdlib only | `scripts/gen_locales.py`, `scripts/check_i18n.py`, codemods. Not a runtime dependency. |
| `cargo` | build, test, clippy, `cargo doc` | `cargo build --workspace` stays warning-free. |

### Already solved in-repo — do not add a crate for these

- **Markdown/GFM** — the renderer in `transcript_view.rs` is a Waku port and must
  stay paint-only so streaming never reflows.
- **Syntax highlighting** — `highlight.rs` (paint-only diff/code lexer).
- **Charts** — Usage is painted directly in `src/usage/`.
- **Git** — the `git` CLI, not libgit2.
- **State management / DI / component library / a second HTTP client / tokio /
  an editor widget** — gpui entities, `app/helpers.rs`, `widgets.rs`,
  `zed-reqwest`, gpui's smol runtime, and `ComposerInput` cover these.

No new dependency without a stated reason (also in *Verification*). When one is
justified, pin it, say in the PR why the in-repo path or an existing crate does
not cover it, and add it to the table above.

## Privacy

Orbit collects no telemetry. There is no analytics crate, no installation
identifier, and no network reporting of product usage; the app talks to the
network only for things the user asked for — update checks, GitHub metadata
for the UI, and the model providers the agent is configured to call. Do not
reintroduce an analytics path. See [`PRIVACY.md`](PRIVACY.md) for what leaves
the machine.

## Localization (i18n)

UI copy is translated through [rust-i18n](https://github.com/longbridgeapp/rust-i18n).
`locales/en.yml` is the **source of truth** — every key and its English text.
The other nine locale files are generated, never hand-edited:

```
python3 scripts/gen_locales.py        # regenerate locales/*.yml + report gaps
cargo test -p orbit-pi i18n           # completeness guard
python3 scripts/check_i18n.py         # key + placeholder guard
```

- **Ships:** English, 简体中文 (`zh-CN`), 日本語 (`ja`), 한국어 (`ko`),
  Español (`es`), Français (`fr`), Deutsch (`de`), Português do Brasil
  (`pt-BR`), Русский (`ru`), Italiano (`it`) — plus `System`, which resolves
  through the OS preferred language (`AppLanguage::from_locale_id`).
- **Call sites:** wrap literals with the crate-root macros — `tr!("key")` for
  plain text, `tr!("key", count = n)` for `%{count}` interpolation, and
  `tr_cow!("key")` only on hot render paths that borrow. Never hard-code
  user-facing English in a render path; add a key instead. The binding name
  must match the key's placeholder exactly (`count = n`, not `total = n`):
  rust-i18n leaves an unbound `%{name}` literal in the rendered string.
  `scripts/check_i18n.py` fails on a key/placeholder mismatch, so run it after
  touching call sites.
- **Translations** live in `scripts/i18n_glossary*.py`, keyed by the exact
  English string from `en.yml`. Split by surface (core, settings, palette,
  transcript). A string with no entry falls back to English.
- **Keys** are `surface.slug` (`settings.general`, `transcript.thinking`) so a
  translator sees context; identical English can appear under several keys.
- **Native menu:** `set_app_menus` rebuilds the macOS menu bar when the
  language changes (`theme::set_ui_prefs` calls it).
- **Placeholders:** a `ComposerInput` placeholder must be set with
  `.with_placeholder_key("surface.slug")` (plus `.with_placeholder_var` for
  `%{…}` values), not `.with_placeholder(tr!("…"))`. The key resolves at paint
  time, so long-lived search fields (settings/provider/model filters, the
  side pane, git panel, usage page) follow a language change without being
  rebuilt. `.with_placeholder` is only for literal, untranslated text such as
  `sk-…` or a URL example.

### Adding a string

1. Wrap the literal with `tr!("surface.slug")` at the call site.
2. Add `surface.slug: "English"` to `locales/en.yml` (append; keep key order).
3. Add `"English": "translation"` under each locale in the matching
   `scripts/i18n_glossary_*.py`.
4. Run `python3 scripts/gen_locales.py` and `cargo test -p orbit-pi i18n`.

The codemods in `scripts/` (`localize_calls.py`, `localize_settings.py`,
`localize_call_args.py`) wrap common GPUI builder patterns in bulk; run one,
merge its key TSV into `en.yml`, then translate. They skip test code and
`*_tests.rs` on purpose.

## Current state

The product is the GPUI app (`cargo run -p orbit-pi`). The former React/Vite/Tauri
stack is gone; do not resurrect it.

### Shipped architecture

| Area | Source | Current behavior |
|---|---|---|
| Shell | `app.rs`, `view.rs` | Native desktop shell with sidebar, top bar, transcript, composer, status bar, contextual panes, and platform window chrome. |
| Sessions | `sessions.rs`, `app/session.rs` | Reads pi sessions from `~/.pi/agent/sessions/`; supports new/open/switch/clone, workspace grouping, parked background processes, and session persistence. |
| RPC | `crates/orbit-rpc/` | One `pi --mode rpc` child per live session, JSONL over stdio, permissive event parsing, typed commands for the currently used protocol surface. |
| Transcript | `transcript.rs`, `transcript_view.rs`, `message_scroller.rs` | Virtualized streaming transcript with tail-following, jump-to-latest, tool activity, markdown/code rendering, find, and error states. |
| Composer | `composer.rs`, `app/composer_ops.rs` | Multiline input, commands, file mentions, image attachments, queued follow-ups, steer, abort, and access/workflow controls. |
| Explorer / Files | `src/explorer/` | Gitignore-aware workspace tree, keyboard navigation, file editor, Markdown/image views, autosave, file operations, and honest binary/oversized/non-UTF-8 states. |
| Review / Git | `review.rs`, `sidepane.rs`, `git.rs`, `git_panel.rs`, `worktree.rs`, `worktree_setup.rs`, `app/worktrees.rs` | Diff review, changed-file navigation, Git status/history/graph, staging, commit, branch operations, GitHub issue/PR workflows where `gh` is available, and Git worktrees as first-class workspaces (optional setup script, sessions rooted in the worktree). |
| Agent controls | `access.rs`, `workflow.rs` | Supervised/Auto-accept edits/Full access and Plan/Build/Ask workflow modes. These are extension-backed controls, not a sandbox. |
| Settings | `app/settings.rs`, `providers.rs`, `auth.rs`, `quota.rs`, `skills_ui.rs` | General/runtime/agent/worktrees/skills/plugins/models/appearance/providers/about/shortcuts plus provider auth, model catalog, quota, skills and plugin management. |
| Native infrastructure | `notifications.rs`, `pi_update.rs`, `app/open_in.rs` | Desktop notifications, pi self-update, open-in-editor/terminal integration, and native menus. |

### Important implementation truths

- Session truth lives in pi's session store and live RPC events.
- Orbit must not fabricate capabilities, metrics, state, provider information, or
  successful operations.
- Access modes are an extension-backed confirmation guard, not a sandbox.
- Plan/Build/Ask are extension-backed workflow modes.
- Git/file/network work that can block rendering must run off the UI thread.
- Long lists and transcripts must remain virtualized.
- The command palette and keyboard shortcuts must invoke the same underlying actions
  as menus/buttons.
- Theme values come from the shared theme system; components must not introduce
  hardcoded palette values.
- Sizes come from `theme::tokens` (Zed's design tokens, D11): new chrome uses
  `DynamicSpacing` / `TextSize` / `IconSize` / `ButtonSize` / `Radius` and
  `elevation_2` / `elevation_3` instead of px literals. Context menus build on
  `context_menu_surface` / `context_menu_entry` / `context_menu_separator`
  (`app/helpers.rs`).
- Public releases cannot assume the author's providers, models, paths, or machine state.
- Provider HTTP bodies have hard caps (measured: `ollama.com/v1` accepts ~7.5 MB and
  returns `413` at ~8 MB; much larger bodies surface as `400 failed to read request
  body`). `read` images stay in the transcript as base64, so long visual-debug
  sessions hit the cap. Fix with `inputLimits.images.resize` via `modelOverrides` in
  `~/.pi/agent/models.json` and a fresh session — see
  `docs/provider-request-limits.md`. Retrying cannot fix it.

### Known gaps

- Rewind/fork UI is not shipped; clone is available.
- Full screen-reader/focus accessibility work remains incomplete.
- GPUI API changes must be handled deliberately because the pinned version is
  pre-1.0.
- Mermaid is not natively rendered by GPUI; use the documented fallback until a
  renderer is intentionally added.
- Do not document roadmap work as shipped until it exists in the repository.

## How the app runs

1. `OrbitApp::new` spawns `PiClient::spawn(cwd, None)` so sessions land in pi's default store (`~/.pi/agent/sessions/`), shared with the CLI. Tests pass `Some("/tmp/…")` via `--session-dir`.
2. On connect it sends `get_state`, `get_available_models`, `get_available_thinking_levels`.
3. `main_window.rs` opens the one window and starts its `spawn` loop: every **90 ms** call `OrbitApp::tick` → `PiClient::drain_events` (`try_recv`, never blocking). The loop is bound to the window, so a reopen restarts it; the `OrbitApp` entity itself is parked in the `Shell` global so the pi process and transcript outlive a close.
4. Transcript data has two inlets into the same model: `get_messages` rebuilds; `message_start` / `message_update` / `message_end` mutate live.
5. One `PiClient` = one child. Drop kills the process. `ProcessExited` is surfaced in the status line.
6. A few seconds after launch the pi self-update check runs off-thread (`pi_update.rs`): a newer pi installs through `pi update self` and reports via toasts; up-to-date or offline stays quiet.

## GPUI 0.2.2 API notes (learned on this repo — save re-deriving them)

> **Reference: Zed.** Use Zed source code as a reference when a task concerns GPUI implementation — layout and styling idioms, focus and key dispatch, virtualized lists, menus and popovers, window and platform behavior — or when an in-house `src/ui` primitive needs a proven native precedent. Zed is the canonical GPUI codebase; read its crates, and read the gpui revision pinned in `Cargo.toml` so the APIs match what Waku builds against.

These compiled and ran against the pinned version. When in doubt, check
`~/.cargo/registry/src/.../gpui-0.2.2/` (source is the docs) and `examples/` inside it.

- **Bootstrap** (`main.rs`):
  ```rust
  Application::new().with_assets(assets::Assets).run(|cx: &mut App| {
      cx.text_system().add_fonts(vec![/* Cow<[u8]> font bytes */]).unwrap();
      let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
      cx.open_window(WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(bounds)),
          titlebar: Some(TitlebarOptions {
              title: Some(SharedString::from("Orbit Pi")),
              appears_transparent: true,
              traffic_light_position: Some(point(px(12.), px(13.))),
              ..Default::default()
          }),
          focus: true, ..Default::default()
      }, |window, cx| { /* Entity<OrbitApp> */ }).unwrap();
      cx.activate(true);
  });
  ```
  Size the window from `cx.primary_display()` (a hardcoded 1240×840 gets clamped top-left on small/scaled screens). `open_window`'s callback is `FnOnce(&mut Window, &mut App) -> Entity<V>`; create entities with `cx.new(|cx| …)`.
- **App icon:** source is `assets/icons/logo-icon.png`. `icon.icns` is for bundled `.app`s (`package.metadata.bundle`); `scripts/make-dmg.sh` ships it under `dev.orbit.pi`. `cargo run` has no bundle, so `app_icon::set_dock_icon()` calls AppKit `setApplicationIconImage` with the embedded 512 PNG, and Settings → About paints the same PNG via `img(app_icon::ASSET)`. **macOS debug builds swap in the Alpha mark** (`assets/icons/alpha-logo.png` → derived `alpha-logo.icns` + `crates/orbit-pi/dev-assets/alpha-app-icon.png`, regenerated by `scripts/make-alpha-icon.sh`; a debug-only asset overlay, so release binaries do not embed it): `debug_assertions` selects `alpha-app-icon.png` for the Dock and About, and `scripts/run-bundled.sh` builds the separate `Orbit Pi Alpha` / `dev.orbit.pi.alpha` bundle from the Alpha icns. Release builds, DMG, and CI still take the production branches (Windows/Linux debug builds keep the production mark).
- **DMG background:** `assets/icons/background_660x400.tiff` is generated from the master art `assets/icons/background_660x400.png` by `scripts/make-dmg-background.sh`, which bakes a frosted label plate behind each icon. Finder draws icon-view labels in **black** whenever the window has a background picture or colour (only the default background adapts to Dark Mode; no `.DS_Store`/AppleScript field controls label colour), so the dark art has to carry the light area the labels sit on rather than relying on white text. Plate positions must track `DMG_APP_X`/`DMG_DROP_X`/`DMG_ICON_Y` in `scripts/make-dmg.sh`.
- **macOS chrome:** `appears_transparent` + `traffic_light_position` — no objc2. Drag regions: `.window_control_area(WindowControlArea::Drag)` on the sidebar strip and the top-bar spacer. When the sidebar is hidden, pad the top bar so controls clear the traffic lights (`pl` ≈ 76 px).
- **Virtualized lists** (transcript **and** sidebar):
  - Transcript uses `message_scroller.rs` (`ListAlignment::Bottom`, 400px overdraw) — 0.2.2 has no `FollowMode::Tail`, so a past-the-end `scroll_to` is the equivalent. Append while following sticks to the live edge; `remeasure_items` grows the streaming row; scroll away shows **Jump to latest**.
  - Rows are plain GPUI flex trees in `transcript_view.rs` (the `Message` component port was removed — no component library anywhere): Start/End alignment, avatar disc, content column, footer. The footer sits outside the avatar row so the 32px disc stays flush with content; the footer is indented `avatar + row gap` to line up with the content column.
  - Sidebar: `ListState::new(count, ListAlignment::Top, overdraw_px)`.
  - `list(state.clone(), move |ix, window, cx| … .into_any_element())` — the closure is `'static`, so capture `Rc<RefCell<_>>` / `Rc<Vec<_>>`, never `&self`.
  - Stick-to-latest: `scroll_to(ListOffset { item_ix: len, offset_in_item: px(0.) })` (past-the-end).
  - Pin tracking: `set_scroll_handler` — for `Bottom` alignment, `is_scrolled` is "not at bottom", and it only fires for real user scrolls, not programmatic `scroll_to`.
- **`list()` needs a definite height:** default `ListSizingBehavior` is **`Auto`** (no intrinsic height). As a plain block child it lays out at height 0 → `prepaint_items` clears its item list → **nothing paints** (while request_layout's overdraw probe still renders ~3 items — the telltale symptom). Fix: `.h_full()` on the list inside a `flex_1` + `min_h_0` parent. Never wrap a `list()` in an outer `overflow_y_scroll()` div — the list scrolls itself.
- **Heartbeat / async:** `window.spawn(cx, async move |cx: &mut AsyncWindowContext| { loop { Timer::after(dur).await; view.update(cx, |this, cx| this.tick(cx)).ok(); } }).detach()`. `AsyncWindowContext` derefs to `AsyncApp` and implements `AppContext`. `pub use smol::Timer` is at the gpui crate root.
- **Rendering:** `impl Render { fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement }`.
- **Events:** handlers are `cx.listener(Self::method)`; the listener bound is `Fn(&Event, &mut Window, &mut App)` (not `Context<T>`). `.hover(|s| …)` for hover styles.
- **Text wrapping:** `.whitespace_normal()` exists in 0.2.2 and is what the assistant transcript uses. There is still **no `WhiteSpace::PreWrap`**. For preformatted blocks (thinking, code), render **one `div()` per line** with a fixed height (e.g. `div().h(px(17.)).child(line)`).
- **Any-elements:** `.into_any()` is on `Element` (concrete types); on an opaque `impl IntoElement` return use `.into_any_element()`.
- **Custom text input:** there is no multi-line editor widget we use. `ComposerInput` is an `EntityInputHandler` adapted from gpui's `input` example (grapheme-aware caret, clipboard, IME stubbed). Keep it single-line until a deliberate composer upgrade (INTENT.md risk #3).
- **Key-binding precedence:** bindings are ordered by dispatch-tree depth (deepest/focused node first); ties at the same depth break by **registration order (later wins)**. Bindings with no context count as the deepest depth. The picker filter uses `.with_key_context("Composer Picker")` so backspace still edits, and picker `enter`/`escape`/`arrows` are registered **after** composer bindings in `main.rs` so they win over `Submit` / `AbortRun`.
- **Popovers:** `anchored().position_mode(AnchoredPositionMode::Local).anchor(Corner::BottomLeft).offset(…).snap_to_window()` + `deferred(entity)` paints a floating popup. `on_mouse_down_out` dismisses. **Click-through:** the same click's mouse-up would re-open the chip — `OrbitApp` swallows toggles for ~200 ms after an outside mouse-down dismiss (`menu_dismissed_at`). Layered `BoxShadow`es (`vec![contact, ambient]`) read as a real modal surface.
- **Picker lists:** **do not use `uniform_list` inside a `deferred` + `anchored` popover** — measured layout can collapse to height 0. Use `max_h` + `overflow_y_scroll` + `ScrollHandle` (Zed `ContextMenu` shape). Keyboard nav must `set_offset` to keep the highlighted row in view. `.on_hover` / `.on_click` live on *stateful* elements (`.id(...)` required).
- **Assets:** implement `gpui::AssetSource` over `include_dir!` so packaged builds don't depend on cwd. SVG icons: `img("icons/….svg")` via helpers in `app.rs`. Provider marks live under `assets/icons/providers/`.
- **Fonts:** `cx.text_system().add_fonts` for embedded `Symbols Nerd Font` (devicons) and the curated catalog under `assets/fonts/bundled/`. `assets::register_zed_fonts` loads IBM Plex Sans / Lilex; `assets::register_bundled_fonts` recursively loads the bundled faces so `theme::UI_FONTS` / `CODE_FONTS` resolve without an OS dependency. Each family is a statically instanced, subset 400/500/600/700 TTF with a normalized name table (one typographic family, unique PostScript names) so font-kit weight selection works.

## Keybindings (current)

| Keys | Action | Context |
|---|---|---|
| `cmd-q` | Quit | global |
| `cmd-n` | New session | global |
| `cmd-r` | Reload session list from disk | global |
| `cmd-,` | Settings | global |
| `cmd-p` | Command palette (sessions / commands / settings) | global |
| `cmd-shift-e` | Toggle project panel (Explorer) | global |
| `cmd-w` | Close the active file tab (last tab closes the Files surface) | `Files` |
| `cmd-shift-w` | Close the Files surface | `Files` |
| `cmd-s` | Save the active file now (autosave is on anyway) | `Editor` |
| `cmd-z` / `cmd-shift-z` | Undo / redo | `Composer` / `Editor` |
| `cmd-shift-c` | Copy the newest assistant response (footer copy mirror) | global |
| `cmd-up` / `cmd-down` | Jump to previous / next user turn (rail mirror) | global |
| `escape` / `cmd-.` | Close settings → close picker → `abort` | global / Composer |
| `enter` / `cmd-enter` | Submit prompt | `Composer` |
| `shift-enter` | Newline | `Composer` |
| `enter` / `escape` / `up` / `down` | Confirm / cancel / move | `Picker` (registered after Composer) |
| `enter` / `escape` / `up` / `down` | Run / close / move | `AddMenu` (composer "+" menu; registered after Picker) |
| `enter` / `escape` / `up` / `down` | Select / close / move | `AccessMenu` (composer access-mode picker) |
| `enter` / `escape` / `up` / `down` | Confirm / deny / move | `Approval` (inline access-guard bar) |

## Pi CLI RPC protocol (official spec)

- Start: `pi --mode rpc [--provider …] [--model …] [--name …]` — see `packages/coding-agent/docs/rpc.md` in the pi repo.
- **Framing is strict JSONL, LF only.** Split on `\n`; strip a trailing `\r`; never use a reader that splits on `U+2028/U+2029`. `PiClient` uses `read_until(b'\n')`.
- **Typed in `CommandBody` today:** `prompt` (with `images` + `streamingBehavior`), `abort`, `clear_queue`, `steer`/`follow_up` (with `images`), `set_steering_mode`/`set_follow_up_mode`, `compact`, `set_auto_compaction`/`set_auto_retry`/`abort_retry`, `set_session_name`, `new_session`, `switch_session`, `get_state`, `get_messages`, `get_available_models`, `set_model`, `cycle_model`, `get_available_thinking_levels`, `set_thinking_level`, `cycle_thinking_level`, `get_commands`, `get_session_stats`, `get_fork_messages`, `fork`, `clone`, `auth.list`, `auth.status`, `auth.login`, `auth.logout`, `auth.cancel`, plus `Raw(Value)` for anything else.
- **Used by the UI today:** `prompt`/`follow_up` (text + images), `abort`, `clear_queue`, `set_follow_up_mode`, `compact`, `set_auto_compaction`/`set_auto_retry`/`abort_retry`, `set_session_name`, `new_session`, `switch_session`, `get_state`, `get_messages`, `get_entries` (bridge quota snapshots, incremental `since` cursor), `get_available_models`, `set_model`, `get_available_thinking_levels`, `set_thinking_level`, `get_session_stats`, and the `auth.*` family when the running pi advertises it (falls back to file/Terminal login otherwise). `SessionState` reads `steeringMode`/`followUpMode`/`autoCompactionEnabled`/`sessionName`/`isCompacting`/`pendingMessageCount` from `get_state`; `PendingQueue` mirrors `queue_update`. `steer` is sent by the composer's steer controls; `clone` by ⌘P. `set_steering_mode`, `cycle_model`, `cycle_thinking_level`, `get_fork_messages`, and `fork` remain typed but unwired.
- **Not typed / not wired yet:** `get_tree`, `bash`/`abort_bash`, `export_html`, `get_last_assistant_text`, and `new_session.parentSession`.
- **Error handling:** a `response` with `success: false` carries an `error` string. The app never swallows it: `on_command_failure` surfaces it in a dismissible error banner (chat and settings) and re-reads `get_state` to reconcile optimistic UI after `set_model`/thinking/rename failures. A `parse` command, `extension_error` events, failed compaction (`compaction_end.errorMessage`), exhausted `auto_retry_end`, and `send` write failures all route to the same banner. A successful retry of the same command clears its banner (`clear_error_for`). **Provider/LLM failures are not responses at all**: pi sets `stopReason: "error"` + `errorMessage` on the final assistant message and emits it via `message_start`/`message_end` (see pi-agent-core `agent-loop.js`). `transcript::message_error` extracts it; the app raises `Agent error: …` and `transcript_view` renders the same text as a red card inline, so a failed turn is never an empty row.
- Events → stdout JSON lines: `agent_start/end/settled`, `turn_start/end`, `message_start/update/end` (`assistantMessageEvent`: `text_delta`, `thinking_delta`, `toolcall_start/delta/end`), `tool_execution_*`, `bash_execution_update`, `queue_update`, `compaction_*`, `auto_retry_*`, `extension_ui_request`, `extension_error`, typed `auth.*` (`Event::Auth`), plus `Unknown` for forward-compat.
- **Provider authentication:** `auth.*` is a first-class RPC namespace. The client reducer is `orbit-pi/src/auth.rs`; the wire types are in `orbit-rpc/src/types.rs`; the server contract (which pi must implement by reusing its provider OAuth + `auth.json`) is `crates/orbit-rpc/docs/auth-rpc.md`. Tokens never cross RPC or GPUI state. **Stock pi 0.85.1 has no server side** — without it Orbit falls back to `pi /login` in Terminal. To get browser OAuth on an installed pi now, run `node contrib/pi-auth-rpc/apply.mjs` (idempotent; `--revert` restores the backup) and restart the agent. `auth_fake_pi.rs` covers the round trip against a scripted server.
- **Provider quota:** `quota.list` is the account-usage sibling of `auth.*`. The client reducer is `orbit-pi/src/quota.rs`; the wire types (`QuotaReport`/`QuotaWindow`/`QuotaBalance`/`QuotaKind`) are in `orbit-rpc/src/types.rs`; the server contract is `crates/orbit-rpc/docs/quota-rpc.md`. pi resolves the credential, verifies the provider origin, rejects redirects, and returns only normalized percentages/counts/resets/amounts — no token, key, or account id. Coverage follows `@narumitw/pi-usage`'s verified reference: Anthropic, ChatGPT Codex, GitHub Copilot, Kimi, MiniMax, Z.AI, OpenCode Go, OpenRouter, Vercel AI Gateway, DeepSeek, Moonshot, xAI, Fireworks, Baseten. Google/Gemini is registered explicitly as `unsupported` (no account-quota API — AI Studio-only). **Ollama Cloud** has a real adapter for both billing generations: the current monthly-credit model via `GET https://ollama.com/api/usage` with a user-supplied cloud API key, and the legacy 5-hour/weekly model via the authenticated `ollama.com/settings` page parsed behind an isolated `OllamaCloudParser` with a user-supplied session cookie. Credentials are explicit (`auth.json`, 0600; type `api_key` or `ollama_cloud_session`), entered in Settings → Providers → Ollama; the local `models.json` placeholder is never sent to ollama.com and browser cookies are never auto-extracted. Every other provider reports `unsupported`. Reports are per-provider cached 60 s (these endpoints rate-limit). **The default path needs no patch:** `bundled_extensions.rs` materializes the bundled `contrib/orbit-quota-extension/` under `~/.orbit-pi/quota-extension/` and every session process is spawned with `pi --extension <index.js>`; the extension fetches through pi's `ctx.modelRegistry`, appends one `orbit:quota` custom entry **only when the snapshot changes** (never in LLM context), and the app polls `get_entries` with a per-session `since` cursor (60 s, plus `agent_settled`, throttled by `QUOTA_ENTRY_POLL_INTERVAL`). Entry ids are per-session: the cursor resets on a `sessionId` change, a dead cursor is dropped and re-read, and bridge data never flips `QuotaSupport` (so a patched pi is still probed). `node contrib/pi-quota-rpc/apply.mjs` remains available for the `quota.list` server side; `quota_fake_pi.rs` covers the round trip and `--extension` forwarding, and `contrib/orbit-quota-extension/*.test.mjs` (run `node --test`) covers the bridge end to end offline, and `contrib/pi-quota-rpc/quota-handler.test.mjs` covers the patch adapters.
- **User interaction:** `extension_ui_request` (`select`/`confirm`/`input`/`editor`; also fire-and-forget `notify`/`setStatus`/`setWidget`/`setTitle`) on stdout; answer with `extension_ui_response` on stdin. `dialog.rs` + `app/dialogs.rs` render the dialog methods as a blocking modal card and reply (`cancelled` on Escape/scrim). `notify` renders as an in-app toast, `setStatus` surfaces on the status bar, and `set_editor_text` seeds the composer; `setWidget`/`setTitle` are terminal chrome and ignored. `ask_user_question`'s multi-select arrives as an `input` of comma-separated option numbers (pi's RPC fallback), so it is selectable too.
- Envelopes parse **loosely**; unknown shapes flow through as raw JSON so protocol additions don't crash the client.
- **Access guard (resolved without the protocol):** pi has no built-in per-tool permission surface in RPC mode and no sandbox (`pi docs/security.md`); its only tool gate is an extension's `tool_call` hook. Orbit ships one (`contrib/orbit-guard-extension/`) that prompts for mutating calls through `ctx.ui.select` — Allow once / Always allow this tool / Deny — which *does* surface over RPC as an `extension_ui_request` and renders as Orbit's inline approval bar. `--approve`/`--no-approve` remain **project trust** (load project-local settings/extensions/skills), a separate concern not yet wired to the access chip. There is no "Auto" approval mode: the installed pi exposes no `modelRegistry.streamSimple` for extensions, so shipping one would be fake.

## Plan (P0–P6) — see `INTENT.md` for decisions, rationale, estimates, and phase gates

### P0–P2 — Foundation (done)
The transport, workspace, process lifecycle, core data layer, sessions, catalog,
settings, and initial native shell are implemented. See the source files above for the
current implementation; do not use this section as a changelog.

### P3 — Core chat (bulk done)
- ✅ Virtualized transcript, stick-to-latest. ⬜ stream veil (Waku `md/veil.rs`).
- ✅ Event drain on the ~90 ms heartbeat coalesces commits. ⬜ explicit ≤8.3 Hz streaming pipeline.
- ✅ Markdown: GFM tables/lists/alerts, one `StyledText` per block, highlight-as-paint, copyable code blocks.
- ✅ Multi-line composer, file attach → base64 images, `/`-command + `@`-file autocomplete, **steer**.
- ✅ Extension dialogs (`select`/`confirm`/`input`/`editor`) as a native modal. ⬜ richer per-tool renderers (bash/thinking dedicated; edit/todo/plan/search/mcp are generic). ✅ tool approval via the access guard's confirm dialog.
- ✅ Diff viewer + per-turn `Last Turn` checkpoints; ✅ image lightbox. Changed files render in the tail summary card **by design** (not per message). ✅ ⌘F transcript find.

### P4 — Workbench (substantially done)
- ✅ Git page (Changes / History / Graph + commit bar, commit-message generation). ✅ Providers: built-in catalog, API-key + OAuth, models.json CRUD, refresh. ✅ Skills, ✅ Plugins (+ search), ✅ **Models** page, ✅ Usage charts + filters + export. ✅ Theme/UI persistence (Orbit store). ✅ Explorer: left project panel (gitignore-aware tree, git badges, filter, hidden toggle) + full-page Files editor (editable, syntax-highlighted code/text with debounced autosave; markdown/image; binary/size guards) + file operations (New File/Folder, Rename, Delete-to-Trash).

### P5 — Mermaid + accessibility (partial)
- ✅ Mermaid fences stay copyable code blocks with an honest label (never faked). ✅ Reduce-motion setting honored by shimmer/spinners/drop fade. ✅ tool approval (`tool_call` guard + access modes; not a sandbox). ⬜ focus rings / screen-reader labeling.

### P6 — Test & ship (partial)
- ✅ Unit + live tests (skip cleanly without `pi`), ✅ 10k-message model-layer perf test, ✅ signed `.app`/DMG + optional notarization/stapling, ✅ GitHub Actions CI. ⬜ on-device scroll-perf benchmark, streaming state-machine edge coverage.

## Waku reference file map (pattern reuse, not copy — their gpui is a fork)

| Need | Waku file(s) |
|---|---|
| Transcript + streaming + veil | `src/app/transcript_view.rs`, `src/app/streaming.rs`, `src/md/veil.rs` |
| Markdown renderer / highlight | `src/md/render.rs`, `src/md/highlight.rs`, `src/md/parser.rs` |
| pi RPC transport (adapt to raw protocol) | `crates/waku-core/src/driver/pi.rs` |
| Review diff sources, parsing, sticky headers, changed-files tree | `src/review_diff.rs`, `src/app/right_panel.rs`, `crates/waku-core/src/workspace.rs` (`resolve_diff_range`) |
| Per-turn checkpoints (Last Turn) | `crates/waku-core/src/checkpoint.rs` |
| Diff syntax highlighting | `src/md/highlight.rs` |
| Integrated terminal (PTY + emulator) | `src/terminal.rs`, `crates/waku-core/src/terminal.rs` — both build on `alacritty_terminal` |
| Theme/chrome/sidebar/sessions | `src/theme.rs`, `src/app/window_chrome.rs`, `src/app/sidebar.rs`, `src/app/sessions.rs` |
| Usage/charts | `src/app/usage_page.rs`, `src/app/usage_meter.rs` |

## Definition of done (whole migration)

Every feature in `PRODUCT.md` → `Capabilities` runs natively in the GPUI app against the pi CLI with the same behavior as the legacy UI; legacy `src-tauri` deleted; `cargo run -p orbit-pi` is the only way to launch.

**For an individual feature:** it runs natively in GPUI against the pi CLI, `cargo build --workspace` stays clean, tests cover the happy path, and the docs describe it — `INTENT.md` → *Implementation status* (plus a `D#` decision when the architecture changed), the `README.md` features/roadmap, and a `CHANGELOG.md` `[Unreleased]` entry. Update these in the same change that ships the feature, never as a follow-up.

## Verification

- `cargo build --workspace` must stay clean (zero warnings) after every change.
- `cargo test --workspace` — unit tests + live pi integration tests. `live_pi.rs`, `live_catalog.rs`, and the other live tests skip when `pi` is missing; `orbit-pi` session-store tests skip when `~/.pi/agent/sessions` is empty. CI builds every change on macOS, Linux (ubuntu-22.04), and Windows, plus clippy on macOS (`.github/workflows/ci.yml`); tests run locally.
- Run the app: `cargo run -p orbit-pi`. Check: sessions list from disk, new session (`cmd-n`), prompt streams into the transcript, model/thinking pickers, abort (`escape`), settings (`cmd-,`), sidebar toggle. Notifications need an app bundle — `scripts/run-bundled.sh` builds the debug binary into the ad-hoc-signed `Orbit Pi Alpha` (bundle id `dev.orbit.pi.alpha`, Alpha icon) and opens it.
- Windows: the same `cargo run -p orbit-pi` builds and runs (GPUI renders through DXGI here). `gpui`'s default `windows-manifest` feature is off in `crates/orbit-pi/Cargo.toml` — it links a second `RT_MANIFEST` resource beside the one `build.rs` embeds, and CVTRES fails the link with `CVT1100: duplicate resource`. `scripts/bundle-windows.ps1` builds the Inno Setup installer (`scripts/installer/orbit-pi.iss`); it needs Inno Setup 6 (`ISCC.exe`) and, per-user under `%LocalAppData%\Programs`, no elevation. Desktop notifications and the alert sound are still macOS-only stubs. Window chrome is the app's own: the header row carries minimize / maximize / close, drawing a restore glyph when `window.is_maximized()`, and the header strips drag the window through `platform::start_window_drag`. Check them by clicking — `IsIconic`/`IsZoomed` flip, closing exits the process — and check dragging by pressing a header strip and moving the pointer; both were broken while the presses were routed through GPUI's `WindowControlArea` path, which this app's focusable root makes unusable (see the Shell row).
- No `unsafe` without a comment; no new dependencies without a stated reason.

