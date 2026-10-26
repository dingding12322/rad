# Changelog

All notable changes to Orbit Pi are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.12] - 2026-10-24

### Added

- **Orbit Azure** and **Orbit Azure Light** are new themes, and they are the
  appearance default: a charcoal (or cool near-white) canvas with a restrained
  blue accent and accent-filled primary buttons. The original ember palette
  stays available as **Orbit** / **Orbit Light**, unchanged in identity —
  near-black canvas, ember accent, light primary buttons with dark ink. Saved
  preferences keep working: `orbit` still resolves to the ember palette and the
  old theme keys migrate as before.
- Primary buttons now read a dedicated fill role (`action`) rather than the
  accent, so a palette can use a neutral button fill without giving up its
  accent for selection washes, focus, tints, and accent-colored text.
- **Settings → Privacy → Temporary files** lists Orbit's scratch entries in
  its `orbit-pi` folder inside the system temp directory, with each file's size
  and age, an **Open folder** action, and **Clean up now**. The list covers
  bug-report screenshots, update downloads, and helper scripts, plus older
  top-level `orbit-*` leftovers, except entries a live Orbit process still
  needs.
- A separate **AI scratch files** group lists pi's own `pi-*` entries (shell
  output, codemode images, share/editor working files) and removes only those
  last used more than an hour ago. Neither section touches your workspace or
  pi's session store.
- **Linux arm64 builds** ship next to the x86_64 ones: a native `arm64` `.deb`
  and an `aarch64` `.tar.gz`, built on the same release run and published as
  their own update feed, so Orbit installs and updates natively on Arm machines
  instead of only on x86_64.

### Removed

- **Thirty-six ported editor themes** are gone from Settings → Appearance.
  Orbit's six palettes and **Flexoki** / **Flexoki Light** remain (plus the new
  **Orbit Azure** pair), and the theme
  dropdown now lists every palette for the mode in effect instead of hiding most
  of them behind an "All themes…" disclosure. A saved preference that names a
  retired theme still loads: dark keys fall back to Orbit, light keys to Orbit
  Light.

### Changed

- Color is a single semantic vocabulary now. `Theme` exposes the design system's
  complete surface/text/accent/status roles (`background`, `surface`,
  `surface_secondary`, `surface_elevated`, `surface_hover`, `surface_active`,
  `border`, `border_subtle`, `text_primary`, `text_secondary`, `text_muted`,
  `text_disabled`, `accent`, `accent_hover`, `accent_pressed`, `accent_fg`,
  `accent_text`, `selection`, `selection_fg`, `focus_ring`, `success`, `warning`,
  `error`, `error_fg`, `info`), and every component paints through them instead of
  the old per-palette names. The raw palette layer uses the same names, so a
  palette edit touches one place.
- Accent-colored text and icons read `accent_text` (a legible blue: `#6EA0FF` on
  charcoal, `#1F5DD8` on light) while `accent` stays the fill role; selected
  navigation and list rows paint `selection`, pressed/open controls paint
  `surface_active`, and keyboard focus paints `focus_ring` (with `button_frame` /
  `icon_button_frame` gaining a visible focus ring).
- Status colors are checked as text: light success/warning/error and dark
  Flexoki/Azure error were stepped until they clear 4.5:1 on the raised card, and
  `text_muted` was lifted in every palette so metadata on a card stops falling
  below AA. The default dark selection wash is `#2F3949` (one step below the
  reference `#303A4B`) so accent labels on the active row clear AA too.
- Disabled controls use `text_disabled` instead of a component-level
  `opacity(0.5)`, so a dimmed control's label stays predictable in both themes.
- The design-token spacing sweep is finished: every on-scale hard-coded
  padding / margin / gap in the app's chrome (top-level view, transcript row
  chrome, session strip, Sidepane, diff view, context meter, Explorer panel
  and viewer, update dialog, Skills page, custom-UI surfaces, Git panel
  widgets, dialogs) now resolves through `DynamicSpacing`, so all of it scales
  with the UI font size and Spacing Density setting like the already-migrated
  surfaces did. Optical 1–2px nudges, off-scale legacy rhythm values
  (10/14/18/22px), and bespoke geometry keep their fixed px.
- Repeated popover-width magic numbers became named constants (`BRANCH_MENU_W`,
  `ACTION_MENU_W` in the Git panel; `MODE_POPUP_W` for the Access / Workflow
  pickers), and a legacy NaN font-size value no longer panics startup — it
  migrates to the default UI font size.

### Added

- **Image attachments are compressed at ingest.** A large PNG/JPEG attached to a
  prompt is downscaled (longest edge 1400 px, never upscaled) and re-encoded as
  JPEG (quality steps down to fit a 512 KiB base64 payload), while images with an
  alpha channel become lossless WebP so transparency survives. Images already
  under both caps pass through untouched, and animated GIFs pass through so their
  frames survive. The status line reports the before/after size when an image
  shrank, and both the composer chip and bug-report screenshots use the
  compressed bytes — so a visual-debugging session can no longer balloon the
  request body (see `docs/provider-request-limits.md`).

### Changed

- **Session-store scans are O(changed files).** The sidebar's session metadata is
  now cached per file, keyed on the file's `(mtime, length)`, so a turn settle or
  a watcher reload re-reads only the session files that actually grew instead of
  rescanning the whole store (which could hold hundreds of MB). A watcher reload
  that changed nothing no longer rebuilds the sidebar or repaints.

- **The transcript's tok/s figure is labeled honestly.** It is an *average* over
  the whole provider request (prefill plus decode), so it now reads `tok/s avg`
  instead of implying instantaneous decode speed — the reason a long, mostly
  cached prompt still shows a lower number.

- **The navigation rail no longer rescans the message list every frame.** Its
  user-turn index is cached in `Transcript` and rebuilt only when the message
  list actually changes (messages only append or are rebuilt wholesale), so a
  long transcript does not pay an O(message-count) pass on every render.

### Debug

- **Per-turn diagnostic log.** Debug builds (`cargo run`) print one `[orbit:perf]`
  line to stderr when a turn settles — context window, cache read and hit rate,
  uncached prompt, output, duration, and average rate — so a drop in tok/s can be
  attributed to prefill vs. decode at a glance. This is local-only diagnostic
  output, compiled out of release builds; nothing leaves the process.

## [0.2.11] - 2026-10-21

### Added

- The new-task page's background is now a living ordered-dither field instead of a
  static dot grid. An 8×8 Bayer screen of four dot sizes over a soft density field
  covers the page edge to edge — densest in a band behind the composer, calmer but never
  empty at the window's borders and corners — and drifts slowly enough that dots cross
  their thresholds a few at a time, so
  the page breathes instead of strobing. It reads the same dither language the photo
  backdrop already speaks, costs no asset, follows the palette, and freezes under
  Reduce Motion. A configured background image still replaces it.
- The app monogram is screened into that field. Its mask is sampled from the mark the
  binary already ships (`app-icon.png`) rather than redrawn, so the backdrop cannot drift
  from the icon and a `cargo run` build never grows the Alpha pill; the glyphs hold a quiet
  density at rest and a soft band of light crosses them about every twenty seconds, filling
  each letter with the largest dot in turn. A window too small to hold the mark drops it.
- The terminal panel is tabbed. ⌘T opens another shell in the current
  workspace, ⌘W closes the active one (the panel closes with its last tab), and
  ⌘⇧] / ⌘⇧[ step between tabs — all scoped to the terminal, so Ctrl+Tab still
  reaches the shell and session cycling is untouched elsewhere. Where Ctrl+W and
  Ctrl+T belong to the shell (Windows, Linux), the same actions take
  Ctrl+Shift+W / Ctrl+Shift+T instead. A background
  tab keeps its PTY alive: a build left running in one tab keeps running while
  you work in another. Switching sessions no longer kills the shells either;
  new tabs follow the active workspace, existing ones stay where they are.
- Each tab carries its login-shell profile mark (zsh, bash, fish — the Simple
  Icons shell logos, CC0), with the shell name and start directory on hover.
  The panel also gained a proper resize handle above the tab strip, so the
  header row belongs to the tabs instead of doubling as a drag target.

### Fixed

- Sending a prompt now shows the run immediately. The composer used to stay on
  **Send** and the transcript stayed silent until pi's `agent_start` and the
  first streamed token — on a slow provider, or a pi that was still booting,
  the send looked dropped. The turn is now in flight from the moment you press
  Enter: the composer flips to Stop, the transcript shows the live
  “Working for…” row (its clock starting at the send), and a second Enter
  queues a follow-up instead of racing a second prompt. A prompt pi rejects
  unwinds that state, so nothing is left spinning.
- macOS: closing the window no longer strands the app. The Dock icon kept its
  running dot but a click did nothing — the reopen request macOS sends to a
  windowless app had no handler, so only a full quit brought a window back.
  Clicking the Dock icon (or the app's Dock tile) now opens the window again,
  on the session that was still open — the shell, the pi process behind it,
  and any stream that kept running while the window was closed all survive the
  close. The heartbeat runs on the app rather than the window, so a run that
  settles with no window open still posts its notification.

## [0.2.10] - 2026-10-16

### Changed

- Trim chrome that restates the default (#62)

## [0.2.9] - 2026-10-14

### Changed

- The composer's **model** and **thinking-level** pickers now read as one
  quiet list: option rows sit one hairline-free `Base01` step apart (matching
  the access/workflow menus) instead of a 6px gap with a hairline between
  every row, the provider scope row is a touch tighter, and each row's
  leading provider/reasoning tile stays neutral so ember marks only the
  selected row's check. A full-height popover is ~36px shorter, and the key
  legend can no longer wrap into a second line.

## [0.2.8] - 2026-10-12

### Added

- **Report a bug** from Settings → Report a bug: choose an **issue type**
  (bug / feature / other, which sets the GitHub label and the section names),
  paste the context into the field at the top, and **Generate draft** turns it
  into an issue title and description with the app's active model (falling back
  to the notes when pi is unavailable). The form attaches the build details —
  Orbit version, app ID, OS and architecture, install kind, pi CLI version, and
  the signed-in `gh` account — and creates the issue through the `gh` CLI when
  it is installed and signed in, otherwise opening GitHub's new-issue page with
  the title, body, and label prefilled. **Screenshots** can be attached too:
  they upload with `gh issue create --attach` when `gh` is signed in, and
  otherwise GitHub's web form opens with the images saved to a temp folder to
  drag in. The form is its own settings section (also reachable from the command
  palette), and its strings are localized in all ten shipped locales.

### Fixed

- Removing a workspace from the sidebar no longer leaves its heading behind.
  Workspace paths are canonicalized (symlinks resolved, so macOS `/tmp` and
  `/private/tmp` are one folder) when the project list is loaded, when a folder
  is added, and when the active workspace is set — so a session's recorded
  `cwd` always lands under its project, a legacy store that listed the same
  folder twice collapses to one entry, and one click removes the folder for
  good.

## [0.2.7] - 2026-10-09

### Added

- The Worktrees page opens with a short **How worktrees work** primer: what a
  worktree is, how to create and open one, the optional
  `.orbit/worktree-setup.sh` step, and the folder-vs-branch rule. It is
  dismissible and stays gone once closed.
- Git worktrees as first-class workspaces. A new Worktrees page (workspace
  header menu or the `Open Worktrees` command) lists every worktree Git
  reports, creates one from a new or existing branch (name, branch, and path
  are independent concepts), opens it as the active workspace — Explorer,
  terminal, Git page, Review, MCP scope, and the agent all follow its path —
  and supports rename (directory only, never the branch), move, lock/unlock,
  remove with a dirty-worktree confirmation, and advanced prune/repair. The
  status bar's **Work in** chip switches between the main working directory and
  any linked worktree and opens the create dialog with a suggested but fully
  editable (custom) worktree name; the default root is `repo/.wt`, configurable
  globally (Settings → Worktrees) and per repository (`.orbit/worktree.json`),
  and an optional setup script (default `.orbit/worktree-setup.sh`, explicitly
  allowed per repository on first use) runs asynchronously after creation with
  `ORBIT_ROOT_PATH` and `ORBIT_WORKTREE_PATH`, keeping the worktree on failure.
  Sessions record and restore the worktree path; an unavailable worktree is
  surfaced instead of silently falling back to the main repository. See
  `crates/orbit-pi/docs/worktrees.md`.

### Changed

- **Refresh** (⌘R, the File menu, or the command palette) now refreshes the
  whole app, not just the session list. It re-reads sessions and settings from
  disk, re-lists the active repository's worktrees, rebuilds the Explorer tree
  and the Review diff, refreshes Git status, re-scans the Usage analytics,
  re-reads the MCP config, and re-asks the running agent for its model/skill
  catalog, session stats, and provider auth/quota. The menu and palette entry
  is now labelled **Refresh** rather than **Refresh Sessions**.
- Quieter chrome on the work surfaces:
  - The sidebar's "star the project" card is gone — the same link stays in
    Settings → About.
  - Appearance drops the duplicate palette-swatch strip beside each theme
    picker.
  - The Usage page's cards lose their explanatory subtitles; the metric
    captions that carry data stay.
  - The status bar drops the redundant branch-count chip (the branch chip's
    own popover lists every branch) and hides the MCP indicator while every
    server is connected.
  - The composer's Plan/Build/Ask chip is the single home for the workflow
    scope: the new-task page no longer draws its own centred Mode control
    (a block of tiles in the empty state). The chip sits with the other
    agent controls on every composer, new task included, and still holds the
    choice as pending until the session id exists.
  - Settings → General states the notification trigger once — "When a run
    finishes or pi needs an answer" — instead of repeating it on each of the
    three channel rows.
  - The pull-request detail page drops its metadata rail, which only
    repeated the chips above it (review, checks, changes, branches). It now
    reads as one centred column, with the opened/updated times folded into
    the header line and the checkout/close/open actions moved below the
    review actions.
  - The pull-request list row now splits identity (number, title, labels,
    author, branches) from status (review, checks, diffstat, time), which
    reads down the right edge, and merged/closed rows carry their own state
    glyph instead of a recoloured open-PR icon.
  - The issue pages get the same treatment: the list row moves its comment
    count into the right status column, and the detail page drops its rail —
    assignees, the labels editor, and the updated time fold into the header
    as one centred reading column.
  - Inline code in the transcript keeps the body ink and its wash instead of
    painting the glyphs in the accent. A code-heavy answer used to become a
    field of ember and read as one long link; now only links carry the
    accent, and code is marked by the mono face and the chip wash.
  - The sidebar wordmark is held to a quieter size, so it labels the
    workbench instead of reading as the column's headline.
  - An expanded command tool call now reads as one terminal surface. The
    **Command** and **Output** sections sit on a single recessed code plane
    (`code_bg`) divided by a hairline, each with its label and copy control on
    a shared axis, instead of two label-plus-body groups floating on the card
    with only a gap between them. The command keeps its `$` prompt gutter, so
    a wrapped command hangs under the first line.
  - Settled tool activity is monochrome. Tool glyphs used to shift the accent
    into a per-kind hue (amber for run, cyan for explore, green for mutate,
    violet for web); a long transcript became a field of tinted badges. The
    glyph shape already names the kind, so settled work now uses one muted
    ink. Running still takes the accent and failure still takes red.
  - Transcript cards (tool, summary, thought, ask) use the plain `border`
    hairline instead of `border_strong`. `border_strong` is the focus/hover
    edge; using it at rest made every card read a step too loud.

### Fixed

- Hover fills no longer square off rounded cards. gpui 0.2.2 clips
  `overflow_hidden` to a rectangle, not the border radius, so a full-bleed
  child background painted square corners over the rounded edge. The tool
  card's header (the reported case), its error strip and detail surface, the
  summary card's header, the code-block title strip, and the MCP server rows'
  first/last entries now carry the matching corner radius themselves.
- Two `tr!` call sites bound the wrong placeholder name, so the literal token
  was printed instead of the value: the tool-detail and edit-diff fold labels
  showed `Show all %{count} lines` (the call bound `total` for a `%{count}`
  key), and a failed branch creation toasted `Branch create failed: %{err}`
  (the call bound `error`). Both now bind the declared name.
  `scripts/check_i18n.py` gained a placeholder pass so the class cannot
  recur — it validates every `tr!` call's bindings against its key's
  `%{…}` placeholders and exits non-zero on a mismatch.
- A truncated tool result no longer renders its **truncated** chip twice in
  the activity row; the chip was queued twice back-to-back.
- The transcript no longer writes a per-render `[card-dbg]` line to stderr
  for every read/edit/write tool card.
- The transcript's **tok/s** rate reads correctly again. It was measured
  client-side from the gaps between pi's streamed deltas, but pi emits the
  response in bursts, so that window collapsed to a fraction of the real
  generation and the rate ballooned (e.g. `600 tok/s` against a true ~100).
  The settled rate now uses pi's own per-call `durationMs`, and the live rate
  divides by the time since the turn's `turn_start` — the same clock basis —
  so the two figures agree.
- Baseten's provider-usage readout works again. Its billing summary requires
  an explicit ISO 8601 `start_date`/`end_date` range (31 days maximum), so the
  old `?window=30d` query returned HTTP 400 and the Providers page showed an
  error instead of a spend figure. The adapter now asks for a rolling 30-day
  window and sums the response's `dedicated_usage`, `training_usage`, and
  `model_apis_usage` totals — the shape Baseten actually returns — showing the
  combined spend with a per-category breakdown.
- The issue and pull-request detail headers no longer collapse their meta
  line to one character per line. The text beside the state chip is now
  `flex_1` + `whitespace_normal`, so it takes the column's remaining width
  and wraps there instead of shrinking inside a wrapping flex row and
  overrunning the rows beneath it.
- A review or comment card with no body (a bare approval, for example) no
  longer draws a stray hairline across its bottom — the header rule that
  separates a body from its header now appears only when there is a body.
- RPC patch auto-apply now locates the pi package for pi's **managed
  installer** as well as npm/pnpm/bun globals. The managed launcher at
  `<agent>/bin/pi` is not inside a `node_modules`, so the old ancestor walk
  never reached the package at
  `<agent>/install/releases/<version>/node_modules/@earendil-works/pi-coding-agent`
  and the Settings → Runtime row read *pi package not found*. Resolution now
  handles that layout on every platform — via `PI_MANAGED_INSTALL_ROOT`,
  `install/current-version`, or the newest release — searches the usual
  install roots when even the launcher is missing. The resolved path is
  recorded in `~/.orbit-pi/rpc-patches/config.json` (shown as **Package
  root** in Settings); the live launcher stays authoritative, so the row
  follows `pi update` to the new release dir and the saved path is only the
  fallback when no launcher resolves. Override for a single run with
  `PI_PACKAGE_ROOT`; the bundled `contrib/pi-*-rpc/apply.mjs` scripts honour it
  and fall back to the managed layout, so hand-runs work too.

## [0.2.6] - 2026-10-07

### Added

- The live **Working for…** line now shows the streaming output rate — e.g.
  **Working for 12s · 42 tok/s**. The rate is generation-only: output
  tokens over the time the model was actively decoding, so tool execution,
  round-trips, and provider stalls don't drag it down. Tokens come from the
  provider's cumulative `usage.output` when reported, with a `chars/4`
  estimate otherwise; the client-measured decode window carries through to
  the settled footer, where the `↑in ↓out · $cost` metric also shows the
  rate and the hover breakdown gains a **Rate** row.

### Fixed

- The cache hit rate no longer reads 100% for a session that wrote cache.
  pi normalizes the token buckets as disjoint, so the denominator now
  includes cache-write tokens — `cache reads / (reads + uncached input +
  writes)` — instead of only reads and uncached input. A share below the
  whole is also withheld from rounding up to a displayed `100%` (it shows
  `99.9%`), and cache rates now render to one decimal.

## [0.2.5] - 2026-10-06

### Changed

- Filled buttons now read as raised from the surface: each carries a soft
  top-lit gradient built from its own fill color plus a short contact/ambient
  shadow (`RaisedExt::raised` in `theme/tokens.rs`). Segmented controls get the
  gradient without the per-segment shadow; ghost icon buttons and disabled
  buttons stay flat.

### Fixed

- Switching to a session in another workspace no longer kills the one that is
  running. The MCP config fingerprint is workspace-specific, but the
  parked-session resume check compared it against the workspace being left,
  so every cross-workspace switch looked like an MCP change and dropped
  (killed) the parked process — truncating its stream mid-turn. The check now
  uses the target session's own workspace, and a busy run is never dropped for
  staleness (#46).
- A parked session no longer stalls mid-run when a tool asks for approval.
  Its `extension_ui_request` is now cancelled with the top-level
  `cancelled` flag pi expects; the previous payload nested it under `value`,
  so the extension kept waiting and the background run stayed `busy` forever
  (#46).

## [0.2.4] - 2026-10-02

### Added

- **⌘⇧G** opens the Git/GitHub page (and closes it again), and the **…**
  overflow menu now shows each row's shortcut — the Git row reads **⌘⇧G**.

### Changed

- The Git page's commit message box now matches the chat composer: the
  multi-line editor and its action row share one rounded, lifted surface, so
  the message reads as a text area instead of a one-line field. **Enter**
  inserts a newline and **⌘↵** / **Ctrl+Enter** commits.
- The top bar's right side now carries only what shows live state: the
  provider quota, **Open in**, the Review chip, and a single **…** menu. The
  Review chip folds together the side-panel toggle and the working tree's
  `+N −N` counts (one or the other); the new **…** menu holds **Session
  details**, **Explorer**, **Terminal**, and **Git**, each still on its
  shortcut and one click away.
- Stashes moved off the Git page's **Changes** tab onto their own **Stashes**
  tab (⌘⌥4), so **Changes** stays the review browser plus commit bar; the
  **Issues** and **Pull requests** tabs moved to ⌘⌥5 and ⌘⌥6.

### Fixed

- Switching to another session no longer kills a session that is still
  starting. A process pi has not named a session file for yet parks in a
  pending pool, keeps draining its run, and joins the warm pool under its
  file as soon as the boot handshake names it — instead of the incoming
  session's adopt dropping (and killing) it (#46).

## [0.2.3] - 2026-10-01

### Added

- Add MCP server management, settings UI, and status (#45)
- Check installed packages for updates and surface a notice (#43)

### Changed

- Feat/add star the repo button (#44)

## [0.2.2] - 2026-09-30

### Added

- Hover hints now carry the shortcut: the model and thinking chips, New Task,
  the sidebar's Search/Usage rows, the top-bar panel toggles, the Git page
  tabs, and the workspace **+** use registry-backed tooltips ("Choose Model
  (⌘⇧M)"), and hovering a session row shows the **⌘1…⌘9** slot that opens it.

- A single command registry now backs the keymap, the command palette, and
  Settings → Shortcuts, so a chord, its chip, and the action it runs can never
  drift apart. The palette gained **Rename Session**, **Pin / Unpin**, **Clone
  Session**, **Delete Session**, **Choose Model** (⌘⇧M), **Choose Thinking
  Level** (⌘⇧T), **Review Changes** (⌘⇧R), and **Keyboard Shortcuts** (⌘/).

- Session management is number- and cycle-addressable: **⌘1…⌘9**
  (**Ctrl+1…9**) open the Nth session in the sidebar's visible order, and
  **Ctrl+Tab / Ctrl+Shift+Tab** cycle to the next or previous session. The
  palette also lists **Next Session** and **Previous Session**.

- The Review pane is fully keyboard operable: `Tab` / `Shift+Tab` move between
  the workbench's focus stops, arrows walk the changed-files tree, `Enter`
  opens a file or toggles a folder, `n` / `p` jump between changed files,
  `[` / `]` jump between hunks, `e` / `c` expand or collapse every file, and
  `Escape` returns to the chat. The composer's autocomplete and the terminal
  keep `Tab` for their own use.

### Changed

- The Git page tabs moved from ⌘1…⌘5 to **⌘⌥1…⌘⌥5** (**Ctrl+Alt+1…5**) so
  the number keys can address sessions directly.

### Fixed

- Orbit and `orbit-rpc` also probe `~/.pi/agent/bin` for the `pi` launcher,
  alongside the npm/pnpm/bun home dirs they already checked. A bundled `.app`
  never inherits the shell PATH that contains it, so a pi installed there used
  to read as missing.

- Shortcuts that were advertised but macOS-only (`cmd-` editor clipboard and
  undo, Files close, Git page tabs) now use the platform modifier, so Windows
  and Linux get the same keys. The ⌘. abort chord was dead — gpui names the key
  `.`, not `period` — and now fires.

- Compaction and branch summaries now appear in the transcript as boundary
  cards: **Compaction — Compacted from 150K tokens** (click to expand the
  summary) and **Branch summary**. A successful compaction also re-reads pi's
  compacted context right away, so the summarized history is replaced by the
  summary the moment it lands instead of lingering until the session is
  reopened.

## [0.2.1] - 2026-09-29

### Changed

- Dropdown menus across the app now match the branch selector's popover
  style: one surface treatment, roomier rows with the same hover, and the
  current choice marked with the active fill and a check. This covers the Git
  page's Sync and "…" menus, changed-file and merge/rebase menus, the Review
  source menu, the Usage filter and row menus, the composer's add/mode
  popups, and the sidebar session menu; the branch, label, template, and
  Settings pickers now share the same row treatment. Right-click context
  menus stay compact.

### Fixed

- The Issue page's **Label** dropdown now closes when its trigger is clicked a
  second time. The popup dismissed on mouse-down and the trigger's mouse-up
  re-opened it on the same click; the trigger now honors the same gesture
  guard every other dropdown uses.

- **New Task** now starts a fresh session when pi has exited instead of doing
  nothing. With no turn in flight the button sent `new_session` to the live
  process, so while the reconnect banner was up the command went into the dead
  process's stdin and was never answered; it now spawns a replacement whenever
  the process is not alive, dropping the corpse and its stale exit banner. The
  sidebar's workspace **+** gets the same fix.

## [0.2.0] - 2026-09-28

### Added

- The Git page's **Issues** and **Pull requests** forms now discover the repository's own
  templates (`.github/ISSUE_TEMPLATE/` Markdown and YAML issue forms, plus
  `.github/PULL_REQUEST_TEMPLATE.md` and its variants). A lone template applies
  automatically; with several, a **Template** picker (including **Blank**) chooses one.
  The choice prefills the title prefix and body skeleton — a YAML issue form is flattened
  into Markdown — and its `labels:` ride along on **Create**.

- The Git page's **Issues** and **Pull requests** forms can now draft their title and
  Markdown body with a **Generate** button. Add optional notes (or leave them blank) and
  Orbit makes the same one-shot, tool-free `pi -p` call the commit-message generator uses,
  reading the branch, its commits, and its changed files; when pi is missing or has no
  credentials it falls back to a local heuristic built from those same facts. Nothing is
  created until you review the draft and press **Create**.

- Every icon-only button across the app now carries the same native tooltip the
  send / stop button has — a localized label, usually with its keyboard shortcut
  (e.g. `Toggle Terminal (⌘J)`). Covers the top bar, sidebar, transcript copy /
  diff controls, composer add button, window caption buttons, Explorer, Review
  pane, Git, Usage, terminal, settings, and the picker / dialog controls.

- The sidebar's Projects list now defaults to most recent activity, with a
  sort control on the header to order workspace groups by recently added, name
  (A–Z or Z–A), or session count, or fall back to the manual added order. The
  choice persists in `~/.orbit-pi/workspaces.json` and each project now records
  when it was added.

- Each project in the sidebar can now wear its own mark: pick from a curated
  set of the bundled HugeIcons and tint it with one of eight semantic colors
  (which follow the active palette). Reach it from the workspace header's `⋯`
  menu → **Icon & color**, with a Reset back to the folder default. The choice
  persists in `~/.orbit-pi/workspaces.json` beside the project list.

### Fixed

- Images in issue/PR bodies, comments, and transcripts now render. The markdown image
  element forced a definite `height: 0`, and gpui derives a missing width from a definite
  height before the image decodes (0 × ratio), so a loaded screenshot collapsed to a 0×0
  box with only its border visible. The height is left auto now: the image scales to the
  reading column and taffy keeps its aspect ratio.

- Screenshots in replies load again when GitHub stored them as signed
  `private-user-images.githubusercontent.com/…?jwt=…` URLs. That JWT expires within
  minutes, so the app got a 404 and showed the alt-text fallback; the renderer rewrites
  the URL to the durable `github.com/user-attachments/assets/<uuid>` form encoded in the
  filename, which serves the same image.

- GitHub issue/PR bodies and comments now render screenshots that GitHub stored as raw
  HTML `<img src="…">` tags when the line is inside a blockquote or list item — a quoted
  bug report is a common case. Previously only a standalone top-level `<img>` line became
  an image, so these showed the tag as text; the whole tag/its URL rendered as a link.

- The **New pull request** form's **Base branch** picker, and the **Template** pickers on
  both new-issue/new-PR forms, no longer float over the wrong part of the form or paint
  behind its fields. They are anchored to the chip that opens them (opening upward at the
  bottom of the PR form), draw above the form, and use the top branch selector's dropdown
  chip (glyph, value, chevron) and picker surface — list inset, rows, and search row for
  the branch list.

- The **Issues** tab's **Label** picker — the list filter chip and the detail view's
  **Edit labels** button — no longer opens at the page's top-right over the issue list.
  It anchors to the button that opened it, opens directly beneath it, and uses the same
  picker surface and rows as the branch selector.

- The conversation rail's active tick now follows every navigation path, not
  just wheel and scrollbar gestures. Clicking a tick and pressing ⌘↑/⌘↓ scroll
  the list programmatically, which never fired gpui's scroll handler, so the
  highlight stayed on the turn the reader left behind. Repeated ⌘↑/⌘↓ presses
  also walked a single turn and stalled; they now step through the transcript,
  and ⌘↑ from the live edge jumps back to the previous turn instead of doing
  nothing. The viewport hint wins while a run streams too, so scrolling up to
  read history moves the rail even mid-stream.

- The sidebar's project headers now stay pinned as you scroll through *any*
  expanded group, not just the active workspace. Previously only the open
  workspace's header was sticky, so a long list under the first project pinned
  while every other expanded project scrolled its header away.

- The "open in" menu's app icons are crisp and no longer tiny. macOS returns an
  icon whose largest representation is 1024×1024, and the menu embedded that PNG
  unchanged, so the renderer's single bilinear pass had to squeeze it into a
  14px slot — the blur. Icons are now rasterized to exactly the pixel size the
  menu draws (36×36 for the now-18px slot, matching a Retina blit 1:1), which
  also drops each embedded icon from roughly 1 MB to 2 KB.

## [0.0.19] - 2026-09-28

### Changed

- **The Review pane gained view controls.** The changed files collapse to their
  headers or expand back in one click (each header is its own toggle), long lines
  wrap or unwrap (unwrapping pans only the diff body — the file name and change
  counts stay fixed at the pane's edges), the diff switches between unified and
  side-by-side rows, and the pane can own the page — the session view yields and
  the sessions sidebar opens beside it — or minimize to a rail, returning to the
  docked side pane first. All icon toggles with tooltips.

### Added

- Settings → Agent → Behavior now lets Enter queue a follow-up (the existing
  default) or steer the running task. The preference persists across launches
  without changing pi’s settings or messages already queued.
- Compact Send/Steer/Queue hints on the right side of the composer footer,
  immediately left of the context-window indicator, plus send/steer/stop tooltips.
  Settings → Shortcuts reflects the selected default. Hints remain visible with
  an empty draft; the newline shortcut stays in Settings rather than adding
  clutter below the composer.
- Transcript tool cards show the file's language icon on `read` / `edit` /
  `write` rows and on the changed-files summary. `web_fetch` rows show the URL
  as a clickable link with the scheme hidden, and `web_search` rows show the
  query instead of the raw arguments JSON.
- Bare `http(s)://` URLs in user and assistant prose are links now: accent
  colored, underlined, marked with the HugeIcons external-link arrow, and
  opened in the default browser on click. Markdown `[label](url)` links keep
  their own label.

### Changed

- Alt/Option+Enter uses the opposite sending mode; ⌘⇧Enter / Ctrl+Shift+Enter
  remains an explicit steer. Idle sends still start a normal prompt, and
  Shift+Enter still inserts a new line. Steering and alternate-send shortcuts
  are scoped to the chat input so they cannot submit drafts from other fields.

### Fixed

- File icons render everywhere they are used (the Explorer tree, `@file`
  mention rows, attachment chips, and Git changed-file rows). The bundled Nerd
  Fonts symbols face was embedded in the app but never registered with the text
  system, so every devicon silently fell back to a text extension badge.

### Fixed

- Dragging the header of a window that is maximized on the primary display no
  longer locks the app up: the drag now starts the system's caption-move
  command instead of posting a non-client left-button message, which GPUI
  dispatches back into the drag region and the drag region answers with another
  post. Restoring and moving at the start of the drag behave like a native
  titlebar.
- The Git page's Back button leaves the page again: the leading controls (Back
  and the page title) now sit outside the window drag region, so pressing them
  no longer starts a window move that swallows the click. The empty bar after
  the controls still drags the window.

## [0.0.18] - 2026-09-26

### Added

- **Default session model** — Settings → Agent can pick the model and thinking level
  every new session starts on, whichever workflow mode it runs in. The choice lives
  in Orbit's own `~/.orbit-pi/session-defaults.json` (pi's global settings file is
  untouched), rides the spawn flags for a session's first turn and
  `set_model` / `set_thinking_level` RPC for every later session in a live process,
  and leaves pi's own default alone when unset. The thinking options are the levels
  pi itself derives for the selected model, so the control tracks the model rather
  than the live session. Resumed sessions keep the model in their file; the
  composer chip still changes model / thinking per session.
- **Zed design tokens** — `theme/tokens.rs` ports Zed's sizing system: dynamic
  spacing with Compact / Default / Comfortable density, text / headline / icon /
  button sizes, list, popover, context-menu, modal, tooltip, input and scrollbar
  metrics, the corner-radius scale, elevation shadows with `elevation_1/2/3`, and
  motion durations. Tokens equal Zed's default pixels at Orbit's defaults and
  follow the UI font size and Spacing Density settings.

### Changed

- Context menus (sidebar session / workspace, Explorer, Git sync / branch / file)
  now follow Zed's context-menu metrics: 200px minimum width, 23px entries,
  14px text and icons, Base04 / Base06 insets, `ListSeparator` spacing, an 8px
  radius with Zed's lighter popover shadow, and an 8px window-edge margin.
  Their sizes now scale with the UI font size.
- Tooltips use Zed's tooltip metrics: offset off the cursor, 14px text, and
  wrapping at 288px instead of overflowing on one line.
- Extension dialogs use Zed's modal layout: `ModalHeader` / section / footer
  insets, a Small headline, a 32px input field, 32px confirm buttons, and the
  four-layer modal shadow.
- The remaining floating modals — provider usage / API key / provider editor
  (Settings), the update dialog, and the custom-UI card — use Zed's
  `ModalSurface` (`elevation_3`): the four-layer shadow, an 8px radius, and
  `ModalHeader` / section / `ModalFooter` insets. The legacy heavier
  `popover_shadow` helper is gone.
- Sidebar session and workspace rows use Zed's list-item tokens: a 4px
  (`rounded_sm`) hover/selection radius, `TextSize` for title / label / metadata
  text, and `DynamicSpacing` for their padding and gaps.
- Transcript chrome follows the token scales: message-row padding, user-bubble
  radius / padding / text, tool and question card shells and headers, tool error
  strips, and the usage footer metric and breakdown card now read `TextSize`,
  `DynamicSpacing`, `Radius`, and `BufferLineHeight` instead of ad-hoc px.
- Settings chrome follows the token scales: the page header, section labels,
  grouped boards, and `setting_row` layout use `TextSize` / `DynamicSpacing` /
  `Radius`, the row separators use the 1px hairline token, and keycap chips size
  to `ButtonSize::Default` height. Setting cards and toolbars migrate next.
- Spacing is unified on `DynamicSpacing`. The ad-hoc `Theme::space` helper — which
  scaled with density but ignored the UI font size — is removed, and its ~185
  call sites (mostly the Git panel, Usage page, and Settings) now resolve through
  the token scale, so they follow the UI font size too.
- UI type is unified on `TextSize`. ~370 `.text_size(theme.ui_px(…))` sites snap
  onto the 10 / 12 / 14 / 16 scale (`XSmall` / `Small` / `Default` / `Large`); at
  most a 1px shift each. Sub-10px badge glyphs and 17px+ display headings, which
  the UI scale doesn't cover, keep their sizing.
- Corner radii are unified on `Radius`. gpui's fixed `rounded_sm/md/lg/xl` helpers
  and every on-scale `rounded(px(N))` (`2/4/6/8/12`) now resolve through the theme,
  so card, chip, and input corners also follow the UI font size.
- One-shot transitions adopt `AnimationDuration` (`Fast`, 150ms). Looping affordances
  (spinner, shimmer, streaming) and feedback timers keep their own cadences, which
  the three-value token scale doesn't cover.
- The settings page is fully on the token scales. Its side nav, headers, sections,
  rows, toolbars, select popups, cards, and modals now read `DynamicSpacing` /
  `TextSize` / `Radius` / `ButtonSize`; the last gpui spacing utilities and raw px
  literals are gone, leaving only bespoke geometry (avatar / dot / toggle sizes,
  fixed panel and modal widths).
- The Usage page (and its chart / heatmap / table / filters / tooltip modules) is on
  the same scales: all spacing, type, and on-scale radii resolve through the tokens;
  only chart and table geometry keeps its own px.
- Every input box follows the `InputField` tokens. The branch picker's create-branch
  field (previously frameless) now uses `input_field_frame`, and the Ask panel's
  custom-answer field uses `DynamicSpacing` offsets; all other single-line inputs
  already routed through `input_field_frame` / `picker_search_frame`. The main
  composer, file editor, and inline tree rename stay bespoke components.
- Every search box is the picker search row (`picker_search_frame`), matching the
  command palette: the settings / Git / Usage searches and the skills filter were
  boxed `InputField`s and now share the palette's flat 36px row, so all searches
  read identically. The Git panel's issue / pull-request search grows to fill its
  filter bar and shrinks when tight (min 160px) instead of a fixed 200px, so the
  Pulls tab no longer overflows with its extra state chip.
- The session-details title (rename) input follows the `InputField` tokens: its
  block uses `DynamicSpacing` / `input::gap` / `input_label` / `input_field_frame`,
  and the Generate-title and Update buttons size to `ButtonSize::Large` (32px) so
  the row lines up with the 32px field.
- Picker ↑/↓ now scroll the focused row into view. The command palette,
  workspace picker, and branch picker set a **positive** `ScrollHandle` offset,
  but gpui stores it **negative** once scrolled down (the model selector already
  negated it) — so the list never actually scrolled and the highlight walked
  off-screen, looking like ↑/↓ did nothing. They negate it now, and the palette
  also scrolls against its actual (window-capped) list height instead of the
  uncapped `list_max_h`. Two command-palette tests cover the ↑/↓ move and its
  scroll-into-view; the scroll test fails on the old sign.
- The model picker had the same ↑/↓-doesn't-stick symptom for four reasons,
  all now fixed. (1) Its open-time "pin the highlight to the active model"
  re-ran on **every** render for the first 400 ms (the hover-suppression
  window), so a ↓ pressed right after opening was silently snapped back; the
  pin now runs only on a genuine open / catalog change. (2) Its deferred scroll
  re-targeted the active model for up to 250 ms after open; it now follows the
  current highlight. (3) pi re-reports the whole catalog on every `get_state`,
  and `set_catalog` re-armed the pin and re-scrolled to the active model each
  time — so any periodic sync undid an ↑/↓ while the popup was open. An
  unchanged `set_catalog` is now a no-op. (4) The pin stayed armed until the
  popup's *first* render, so a ↑/↓ arriving before that frame (or right after a
  scope change / catalog refresh re-armed it) was undone by it; a deliberate
  ↑/↓ now cancels the pin. Covered end to end as well: a test opens the real
  `OrbitApp` with the real `bind_keys`, asserts the popup's filter actually
  holds focus, and steps the highlight through several presses.
- Picker rows no longer move the keyboard highlight on hover: the command
  palette, workspace picker, and extension-dialog option rows used `on_hover` to
  follow the pointer, and `on_hover` re-fires whenever hit-testing changes — so
  scrolling a row under a stationary pointer hijacked the highlight. They use
  `on_mouse_move` now, so only real pointer movement moves it.
- Single-line inputs no longer wrap. Every `ComposerInput` capped at
  `with_max_lines(1)` — the session-details name, the Git branch field, the
  provider / model / plugin / skill filters, the usage searches, the extension
  dialog, and the find bar — now also sets `with_wrap(false)`, so a long value is
  clipped at the edge instead of wrapping into a second row and painting the
  editor's vertical scrollbar. Non-wrapping text also scrolls horizontally to
  follow the caret, so typing past the field's width still shows what you type
  (clipped to the text area, so the code editor's gutter stays put).
- The Git panel's tab strip is on the §5 button language: each tab is a
  `button_frame(ButtonSize::Medium)` — the same 28px height, `Base08` padding,
  `Base04` gap, `button::RADIUS` (4px), and Default label as the branch chip and
  the panel's filter chips — so the tabs and the chips beside them read as one
  system. The tab strip and branch row are `Base40` tall, and the whole panel's
  spacing resolves through `DynamicSpacing` (one shared `tab_bar` for Changes /
  History / Graph / Issues / Pulls).
- The sidebar's New Task, Search, and Usage controls share one
  `button_frame(ButtonSize::Large)`: the hand-rolled 28px Search / Usage rows
  now match the New Task button's height, `Base08` padding, `button::RADIUS`
  (4px), and Default label, while keeping their ghost treatment (no fill or
  border, hover only) — one evenly sized stack.
- The model selector's option rows keep the picker's two-line token height. A
  capped list is a flex column, so without `flex_none` every row shrinks toward
  the `picker_entry` minimum (31px) instead of `picker::two_line_entry_height`
  (50px), leaving the 28px leading provider / thinking chip with almost no
  vertical padding. The no-match empty row, its provider-header gap, and the
  row's hover highlight are on the same tokens / `on_mouse_move` as the other
  pickers.

### Fixed

- The model and thinking chip pickers keep keyboard focus when opened from
  their composer chip. The composer box's own mouse-up handler (which focuses
  the input) is an ancestor of the chip and ran after it in gpui's bubble
  order, so ↑/↓/Enter/Escape went to the composer instead of the popup. The
  composer box now leaves focus alone while any of its popovers is open, and a
  click inside the model popup no longer bubbles out to it.
- The top-bar provider-quota popover no longer clips its last provider card
  when several accounts are connected: the card list scrolls inside the
  popover's height cap instead of stretching past it. The list now carries its
  own max height rather than living in a `flex_1` body — inside the deferred,
  anchored popover the available height is zero, where a flexible child
  collapses and the overflow is clipped.

## [0.0.17] - 2026-09-25

### Added

- **Workflow modes** — plan, build, or answer instead of one undifferentiated
  chat. A session is scoped **Plan**, **Build**, or **Ask** from the New Task
  page or the composer chip, per session. Plan and Ask are read-only: the
  bundled `orbit-workflow-extension` disables the write tools, gates `bash` to
  a read-only allowlist, and injects mode guidance, re-arming live sessions
  from `~/.orbit-pi/workflow.json` with no restart.
- **AI review agent** — a read-only reviewer over the current change set or the
  whole project, from the Review pane's sparkles menu or the command palette.
  It runs on its own pi process scoped to Ask mode (so the chat session and
  transcript are untouched), launched with `ORBIT_REVIEW=1` so the access guard
  never blocks its read-only `git diff`. Findings parse from a fenced JSON
  block into severity-chipped rows that scroll the diff to the offending file;
  the answer's prose is kept when no findings parse.

## [0.0.16] - 2026-09-24

### Added

- Add structured data, harden headers, and extend cache windows
- Rebuild landing around story sections and refresh OG

### Changed

- Feat/UI refinement (#23)

## [0.0.15] - 2026-09-23

### Added

- Per-tool glyph badges in the transcript, tinted by work kind, with a
  chip-style folded activity group
- Surface pi's capped-result facts on tool cards as a `truncated` chip
- Success check on the session-details Update button after a rename commits

### Changed

- Refactor the transcript UI: readable tool labels, a real shared spinner,
  shared button hover/press feedback, and redesigned git issue/PR details

## [0.0.14] - 2026-09-23

### Changed

- Auto-apply pi RPC patches and harden custom UI surfaces

## [0.0.13] - 2026-09-22

### Changed

- Feat/GitHub page features (#21)
- Feat/main updates (#20)
- Feat/GitHub page features (#19)
- Remember Open in preferences per workspace and add Rider support (#17)
- Update GitHub Sponsors username in FUNDING.yml (#18)
- Update GitHub Sponsors username in FUNDING.yml (#16)

## [0.0.12] - 2026-09-22

### Added

- Add ten new light palettes and toggle_knob role

### Fixed

- Keep the update modal's footer inside the card

## [0.0.11] - 2026-09-22

### Contributors

### Added

- **Ten more palettes** — the appearance catalog grows from 32 to 42. Two new
  Orbit-family light palettes join the set: **Orbit Paper**, a cooler neutral
  paper canvas that is the light counterpart to Orbit's warm off-white (same
  ember accent), and **Orbit Contrast**, a near-white, high-contrast palette
  with deeper ink and a stronger hairline for maximum legibility. The curated
  light counterparts of the editor themes also land — **Ayu Light**,
  **Catppuccin Latte**, **Flexoki Light**, **Gruvbox Light**, **Nord Light**,
  **One Light**, **Solarized Light**, and **Tokyonight Light** — so light is a
  family in the Appearance dropdown rather than a single option. The picker
  partitions the catalog by mode, so every choice it offers matches the active
  mode.

- **`toggle_knob` palette role** — a mode-aware token for the knob of a toggle
  switch, kept light in both modes so it reads on the accent (on) and the
  raised track (off). The knob previously used `text`, which painted a black
  dot on the light track in light mode.

### Changed

- The saved theme keys `one-light` and `flexoki-light` now resolve to the real
  **One Light** and **Flexoki Light** palettes instead of being folded into
  Orbit Light; the generic `light` alias still maps to Orbit Light.

### Fixed

- Clicking a radio row no longer leaves a stray accent focus ring behind.
  gpui 0.2 has no `:focus-visible`, so the automatic focus transfer on
  mouse-down is suppressed; the ring appears only for keyboard focus, while
  the row's own click still selects on mouse-up.

- **Dumitru Moloșnic** ([#11](https://github.com/imrj05/orbit/pull/11)) — light,
  dark, and system appearance modes; transcript table sizing, streaming
  scroll-position, and multiline command-preview fixes.

## [0.0.10] - 2026-09-21

### Added

- **Explorer** — a Zed-style file-and-folder explorer for the active workspace.
  A left **project panel** (`cmd-shift-e`, top-bar folder toggle, or the ⌘P
  row) renders the workspace as a virtualized tree: expand/collapse,
  case-insensitive filter, a hidden-files toggle, devicon glyphs, `M/A/D/R/U`
  git-status badges, and keyboard navigation (arrows, home/end,
  `cmd-←`/`cmd-→`, enter/space). The walk is gitignore-aware (nested
  `.gitignore`/`.ignore`, global and repo excludes; symlinks never followed)
  and runs off-thread with a bounded entry cap surfaced honestly as
  "truncated". Right-clicking a row offers Open, Open in Default App, Reveal
  in File Manager, and Copy Path / Relative Path. Clicking a file opens a
  full-page **Files** editor: a tab strip over syntax-highlighted
  code with line numbers, editable Markdown (with an Edit/Preview toggle), or
  an image, with a path/size/line-count toolbar and a save-status chip. Code,
  text, and Markdown files are editable and autosave to disk (debounced;
  `cmd-s` saves now); unsaved tabs carry a dot, and external changes are
  flagged rather than silently overwriting edits. Binary files, oversized
  files (2 MiB), truncated files, non-UTF-8 files, and directories get an
  honest notice instead of a lossy dump, and never enter the editor. The
  workspace watcher refreshes the tree and the open file on change. `cmd-w`
  closes the active tab (the last tab, or `cmd-shift-w`, closes the surface),
  and the toolbar X returns to the chat.

- **Editing** — the Explorer's editor (and the chat composer) supports
  **undo/redo** (`cmd-z` / `cmd-shift-z`), where a run of typing within 400 ms
  coalesces into a single undo step and a caret move starts a new one. History
  is bounded by both step count and total bytes so a whole-file buffer cannot
  balloon memory.

- **Explorer file operations** — create, rename, and delete files and folders
  from the project panel. **New File** and **New Folder** sit in the panel
  header and in a directory's right-click menu; **Rename** edits a row's name
  in place; **Delete** asks first and moves the item to the OS Trash on macOS
  (permanent delete elsewhere, with the confirmation worded to match). Names
  are validated before anything touches disk, a create never clobbers an
  existing file, and a rename refuses an occupied name. An operation is refused
  while an open Files tab under the target has unsaved edits, so it can never
  race an autosave or silently discard a buffer; a renamed file's open tab
  follows the new path, and a deleted one closes.

- **More code fonts** — the Code font picker now offers seven more bundled
  faces alongside the existing set: **DM Mono**, **IBM Plex Mono**,
  **Inconsolata**, **Noto Sans Mono**, **Space Mono**, **Anonymous Pro**, and
  **Martian Mono**. Like the rest of the catalog they ship as subset static
  TTFs, so a picked face resolves without an OS dependency.

### Fixed

- Explorer rows now fill the panel width, matching the Review pane's file tree.
  The selection/hover highlight no longer collapses to a narrow pill around the
  devicon, and git badges align to the right. This also fixes the inline rename
  and new-file prompts, which rendered as an empty bubble: the prompt input is
  a `ComposerInput`, which has no intrinsic width, so on a content-sized row
  its `flex_1` collapsed to zero. A regression test renders the real panel and
  asserts the prompt input keeps a usable width.

- The Explorer's row context menu opens at the pointer, flipping above/left
  when it would overflow the window. It was pinned to the panel's top-left, so
  a row's menu always appeared at the top of the tree regardless of where the
  row was right-clicked.

- Closing a file tab keeps keyboard focus inside the Files surface: it moves to
  the newly active editor, or to the surface itself for preview/read-only tabs,
  instead of being dropped with the removed editor. Previously focus was lost
  after the first close (when the next tab did not paint an editor), so `cmd-w`
  stopped working. Closing a tab to the left of the active one also selected the
  wrong neighbour.

- Search and filter placeholders (provider, model, settings, plugins, skills,
  side pane, git panel, usage, in-transcript find, branch/workspace pickers)
  now follow a language change immediately instead of keeping the language
  they were created in. Placeholders are resolved from translation keys at
  paint time.
- With no saved language preference, `System` now walks the OS's ordered
  preferred-language list and picks the first language Orbit ships (falling
  back through unshipped choices such as Hindi) instead of defaulting to
  English. The macOS bundle also declares its supported localizations.
- The setup page's dependency descriptions follow a language change without
  re-probing the host.

## [0.0.9] - 2026-09-21

### Added

- **Version History** in the update modal, opened from the update and
  up-to-date dialogs: every release the feed carries, newest first, each with
  its changelog and the running build marked **Current**. Appcasts now
  accumulate across releases (the newest 20 signed items), so the list grows
  with each release.

### Changed

- The update modal leads with the app icon, checks against an indeterminate
  progress bar, and names the current version when up to date.
- **Check for Updates** always opens the modal. A build that can't check
  (debug, or a binary outside a managed install) says so there instead of
  posting a toast; with `ORBIT_FORCE_UPDATER=1` and the signing key compiled
  in, such a build runs a real check-only flow without **Update now**.
- The update flow now runs through a single modal. A staged release and
  **Check for Updates…** both open it: the check shows a search progress bar,
  then the release's changelog with **Cancel** / **Update now**, while an
  already-staged release offers **Later** / **Update now**. The Settings
  update control is an icon-only download button, and the sidebar shows a
  20px download glyph that expands to **Update** on hover (the reference
  app's pattern); the changelog rides in each appcast item's `<description>`
  (written from `CHANGELOG.md` at release time).

## [0.0.8] - 2026-09-20

## [0.0.7] - 2026-09-20

### Added

- Interface localization via `rust-i18n`, shipped with ten locales — English,
  Simplified Chinese, Japanese, Korean, Spanish, French, German, Brazilian
  Portuguese, Russian, and Italian — plus a `System` option that follows the
  OS preferred language. Settings → Appearance → Language switches it live
  (including the native menu bar); `locales/en.yml` is the source of truth and
  `scripts/gen_locales.py` regenerates the other files from the glossary.

## [0.0.6] - 2026-09-19

### Added

- Clone a session straight from the sidebar — each session row's `…` menu
  gained **Clone session**, which duplicates that session on disk (fresh id,
  pi's `<timestamp>_<id>.jsonl` naming, entries copied verbatim) and drops the
  copy into the list to branch off. Unlike Delete, it only reads the source, so
  it works on any row — including one with a live pi process — without
  switching away from what you're doing. The ⌘P "Clone Session" command still
  clones the open session through pi.
- Usage page — a full-width **Daily activity** calendar heatmap: one cell per
  day across a trailing 12 months, shaded by the active metric (tokens,
  requests, cost, …), with month/weekday axes, a less→more legend, a hover
  readout, and click-to-scope to a single day (click again to restore the
  range). It is independent of the date range — a contribution graph needs a
  year to read — but the active workspace / model / provider / errors filters
  apply to every cell.

### Fixed

- App and composer shortcuts now bind GPUI's cross-platform `secondary`
  modifier (Cmd on macOS, Ctrl on Windows/Linux). They previously used
  `cmd-*`, which GPUI maps to the Windows/Super key on Windows, so
  copy/paste/cut/select-all and every other primary shortcut did nothing
  there. Shortcut hint chips now render the platform-correct label too.

## [0.0.5] - 2026-09-17

### Added

- Windows releases attach the bare `orbit-pi.exe` alongside the `.zip`, so the
  executable is a direct single-file download (`scripts/bundle-windows.ps1`).
- Linux releases attach a native `.deb` alongside the `.tar.gz`, installable
  with `apt install ./orbit-pi_*.deb` (`scripts/bundle-linux.sh`).

## [0.0.4] - 2026-09-17

## [0.0.3] - 2026-09-16

### Added

- Integrated terminal — a real login shell in a resizable bottom panel (⌘J or
  the top-bar toggle), independent of the right side pane. Built on
  `alacritty_terminal` for PTY and VT/ANSI emulation, rendered natively by
  GPUI: scrollback, click-drag selection with ⌘C/⌘V, bracketed paste, a
  blinking cursor, and a per-theme ANSI palette. The shell follows the active
  workspace and restarts when it changes.

## [0.0.2] - 2026-09-16

### Added

- Native macOS menu bar (Orbit / File / Edit / View) wired to the app's
  existing actions; ⌘N and the About surface now have a real home.

## [0.0.1] - 2026-09-14

### Added

- Chat-style `pi` sessions — streaming transcript, model selection, thinking
  effort, follow-up queueing, mid-run steering, and cancel.
- Sessions grouped by project, reopenable and clonable, with an Orbit-owned
  project list that leaves `pi`'s own session store untouched.
- Access guard — Supervised / Auto-accept edits / Full access, enforced by a
  bundled `tool_call` extension with an inline Allow once / Always allow this
  tool / Deny bar.
- Review pane with a live `git diff HEAD`, and a Git page for Changes /
  History / Graph with staging and commit.
- Workbench pages — usage, skills, plugins, models, providers, and appearance,
  read from `pi`'s on-disk data.
- In-transcript find (⌘F) and a full-window image lightbox.
- Native macOS app bundle, Developer-ID signed and notarizable.

[Unreleased]: https://github.com/imrj05/orbit/compare/v0.2.12...HEAD
[0.0.1]: https://github.com/imrj05/orbit/releases/tag/v0.0.1
[0.0.2]: https://github.com/imrj05/orbit/releases/tag/v0.0.2
[0.0.3]: https://github.com/imrj05/orbit/releases/tag/v0.0.3
[0.0.4]: https://github.com/imrj05/orbit/releases/tag/v0.0.4
[0.0.5]: https://github.com/imrj05/orbit/releases/tag/v0.0.5
[0.0.6]: https://github.com/imrj05/orbit/releases/tag/v0.0.6
[0.0.7]: https://github.com/imrj05/orbit/releases/tag/v0.0.7
[0.0.8]: https://github.com/imrj05/orbit/releases/tag/v0.0.8
[0.0.9]: https://github.com/imrj05/orbit/releases/tag/v0.0.9
[0.0.10]: https://github.com/imrj05/orbit/releases/tag/v0.0.10
[0.0.11]: https://github.com/imrj05/orbit/releases/tag/v0.0.11
[0.0.12]: https://github.com/imrj05/orbit/releases/tag/v0.0.12
[0.0.13]: https://github.com/imrj05/orbit/releases/tag/v0.0.13
[0.0.14]: https://github.com/imrj05/orbit/releases/tag/v0.0.14
[0.0.15]: https://github.com/imrj05/orbit/releases/tag/v0.0.15
[0.0.16]: https://github.com/imrj05/orbit/releases/tag/v0.0.16
[0.0.17]: https://github.com/imrj05/orbit/releases/tag/v0.0.17
[0.0.18]: https://github.com/imrj05/orbit/releases/tag/v0.0.18
[0.0.19]: https://github.com/imrj05/orbit/releases/tag/v0.0.19
[0.2.0]: https://github.com/imrj05/orbit/releases/tag/v0.2.0
[0.2.1]: https://github.com/imrj05/orbit/releases/tag/v0.2.1
[0.2.2]: https://github.com/imrj05/orbit/releases/tag/v0.2.2
[0.2.3]: https://github.com/imrj05/orbit/releases/tag/v0.2.3
[0.2.4]: https://github.com/imrj05/orbit/releases/tag/v0.2.4
[0.2.5]: https://github.com/imrj05/orbit/releases/tag/v0.2.5
[0.2.6]: https://github.com/imrj05/orbit/releases/tag/v0.2.6
[0.2.7]: https://github.com/imrj05/orbit/releases/tag/v0.2.7
[0.2.8]: https://github.com/imrj05/orbit/releases/tag/v0.2.8
[0.2.9]: https://github.com/imrj05/orbit/releases/tag/v0.2.9
[0.2.10]: https://github.com/imrj05/orbit/releases/tag/v0.2.10
[0.2.11]: https://github.com/imrj05/orbit/releases/tag/v0.2.11
[0.2.12]: https://github.com/imrj05/orbit/releases/tag/v0.2.12
