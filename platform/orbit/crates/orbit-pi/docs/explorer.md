# Explorer — project panel + file viewer

Design for Orbit's file-and-folder explorer: a Zed-style project panel for
understanding a workspace's structure, plus an editor for reading and changing
file contents. Approved direction: **right dock + full-page Files surface**,
**gitignore-aware** walking with a hidden-files toggle, and **debounced
autosave** for text files.

Status: **implemented** (phases 1–6). Text and code files open in an editable,
syntax-highlighted buffer that writes back to disk; Markdown, images, binary,
and oversized files stay read-only. Files and folders can be created, renamed,
and deleted (to the Trash) from the panel. Later ideas (quick-open, directory
folding, sticky scroll) remain open — see [Rollout](#rollout).

## Goals

- Browse the active workspace as a folder tree — expand/collapse, folder and
  file glyphs, git status badges, filter, hidden-files toggle.
- Open a file, read it, and edit it: line-numbered editing with syntax
  highlighting for code, rendered Markdown for `.md` (with an Edit/Preview
toggle), and honest handling of
  binary, oversized, and non-UTF-8 files.
- Save edits without ceremony: debounced autosave (plus `cmd-s`), atomic
  writes, dirty markers, and a visible save status.
- Stay inside Orbit's hard rules: GPUI only, no I/O on a frame, virtualized
  lists, paint-only syntax highlighting, every user-facing string in
  `locales/en.yml`.
- Leave room for Zed-parity later (quick-open, git decorations, file
  operations) without re-architecting.

Non-goals for this pass: multi-select, drag-and-drop, multi-worktree. Editing
covers **valid-UTF-8 text files only**: truncated and non-UTF-8 files stay
read-only so a save can never destroy bytes the viewer never held.

## Where it lives

```
┌──────────┬────────────────────────────────┬───────────┬──────────┐
│ Sessions │ chat transcript  /  Files page  │ Explorer  │  Review  │
│ sidebar  │                                 │ (right    │  side    │
│          │  ┌ tabs ─────────────────────┐  │  dock)    │  pane    │
│          │  │ main.rs ×   app.rs ×      │  │  ▾ src    │          │
│          │  ├───────────────────────────┤  │    main.rs│          │
│          │  │  1  use gpui::*;          │  │    app.rs │          │
│          │  │  2  fn main() {           │  │  ▸ assets │          │
│          │  │  3      ...               │  │           │          │
└──────────┴────────────────────────────────┴───────────┴──────────┘
```

- **Explorer** is a second right dock between the main column and the Review
  pane, with its own resize handle (mirrors the sidebar's drag pattern).
  Toggle: top-bar button, `cmd-shift-e` (Zed's binding), command palette row.
- **Files page** is a full-page surface in the main area, dispatched like
  `git_open` / `usage_open` / `settings_open`. It owns a tab strip so several
  files stay open; the chat is one toggle away, exactly as Git and Usage work.
- The Review side pane keeps its current dock and is unaffected.

The explorer follows the active session's workspace (`OrbitApp::current_workspace`)
and shows a workspace header with a switcher when several projects exist.

## Layering

Mirrors `src/usage/` so no I/O ever touches a frame:

```
explorer/
  walk.rs     off-thread snapshot: gitignore-aware tree + git-status map
  tree.rs     DirNode/Row model, expand/collapse, filter, visible-row flatten
  panel.rs    ProjectPanel entity — virtualized list, keyboard nav, context menu
  viewer.rs   FileViewer entity — off-thread read + write, editor, highlight
```

```
workspace dir
    ↓  walk::snapshot   (background thread; ignore crate)
TreeIndex { nodes, status badges, truncated }
    ↓  tree::visible_rows(expanded, filter)
[Row]  →  panel.rs  (GPUI list(), fixed row height)
file path
    ↓  viewer::load     (background thread; size + binary guards)
FileContent { text, lines, lang, mode }  →  viewer.rs
    ↓  ComposerInput (gutter, no wrap, syntax runs)
edits
    ↓  viewer::save_file (background thread; temp file + rename)
workspace dir
```

## Data layer

### Walking (`walk.rs`)

Use the `ignore` crate (ripgrep's walker) — already a transitive dependency in
`Cargo.lock`, promoted to a direct one with a stated reason in `Cargo.toml`:

- `.hidden(!show_hidden)` — dotfiles and dot-dirs hidden by default, toggled
  from the panel toolbar.
- `.git_ignore(true)`, `.git_global(true)`, `.git_exclude(true)` — honors the
  repo's own `.gitignore` (including nested ones), global ignore, and
  `.git/info/exclude`.
- `.follow_links(false)` — no symlink cycles, ever.
- Caps: `max_depth` and a hard entry cap (`MAX_ENTRIES`) so a pathological tree
  stays bounded; `TreeIndex.truncated` surfaces the cap honestly.

The snapshot runs once per (workspace, show_hidden) change on the background
executor. Unlike lazy per-directory reads, one snapshot is what makes nested
`.gitignore` correct: the walker reads every level's ignore file as it descends.
Rebuilds are debounced through the existing `watch::WorkspaceWatcher` (already
filters `.git` internals, `target`, `node_modules`, editor cruft), so typing in
a file does not rebuild on every keystroke.

Directories are inserted into the tree even when they contain no visible files,
so empty-but-real folders are not silently dropped.

### Tree model (`tree.rs`)

```rust
pub struct TreeIndex {
    pub root: Node,
    pub badges: HashMap<String, StatusBadge>, // workspace-relative path → M/A/D/U
    pub truncated: bool,
}

pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub children: Vec<Node>, // dirs first, then files, both case-insensitive
}

pub enum Row { Dir { .. expanded }, File { .. } }

pub fn visible_rows(index: &TreeIndex, expanded: &HashSet<PathBuf>, filter: &str) -> Vec<Row>;
```

Flattening is the same shape as `review::tree_rows`: a directory emits a row,
then its children when expanded; a non-empty filter auto-expands matches and
prunes non-matching subtrees. Pure functions over immutable state, unit-tested.

Git badges come from `git::status_rows` (already off-thread), reduced to a
`path → char` map (`U` untracked, `M` modified, `A` added, `D` deleted, `R`
renamed). Files inherit an "inside a changed dir" dot on their parent directory,
like Zed.

### Viewer / editor (`viewer.rs`)

```rust
pub enum Mode { Code { lang: Lang }, Markdown, Text, Image, Binary, TooLarge }

pub struct FileContent {
    pub path: PathBuf,
    pub mode: Mode,
    pub text: String,        // decoded bytes, verbatim (seeds the editor)
    pub lines: Vec<String>,  // read-only rendering (truncated / lossy text)
    pub tokens: Option<Vec<Vec<Token>>>, // paint-only, computed off-thread
    pub bytes: u64,
    pub truncated: bool,
    pub lossy: bool,         // invalid UTF-8; never editable
}
```

- **Guards before reading:** reject directories, cap at `MAX_FILE_BYTES`
  (e.g. 2 MiB) → `TooLarge` with the size shown; NUL-byte sniff → `Binary` with
  a hex/“cannot preview” notice. Never allocate a multi-GB string.
- **Editable modes:** `Mode::Code` and `Mode::Text` open in a `ComposerInput`
  configured with a line-number gutter, no wrapping, and fill-height. The
  buffer is seeded from `FileContent::text`; edits are tracked by the input's
  monotonic `revision` counter.
- **Editing:** `ComposerInput` provides undo/redo (`cmd-z` / `cmd-shift-z`,
  coalescing a typing run into one step) as well as the text actions.
- **Autosave:** the first edit marks the tab dirty and arms a 500 ms debounce.
  A flush grabs the buffer text on the UI thread and writes it off-thread with
  `save_file` (sibling temp file + rename, direct-write fallback). `cmd-s`
  flushes immediately; closing a tab or the surface flushes every dirty tab.
  The toolbar shows `Saved / Unsaved / Saving… / Save failed`.
- **External changes:** the workspace watcher still drives `reload`. A reload
  keeps the old content on screen (no spinner flash), then compares disk text
  to the last read/written text. A tab with unsaved edits keeps its buffer; if
  the disk text really changed underneath it the toolbar flags *Changed on
  disk*, otherwise our own save echoing back through the watcher is ignored.
- **Read-only modes:** Markdown opens in the editor and can be toggled to the
  rendered preview, which reads the *live* buffer so edits show immediately
  (rendering uses `transcript_view::render_markdown_document`; images reuse the
  `gpui::Image` loader). Binary/oversized/truncated/non-UTF-8 files render a
  notice or a read-only highlighted body.
- **Syntax highlighting:** live in the editor via the same `highlight::tokenize`
  lexer the transcript and read-only viewer use; Markdown has no lexer and
  stays rendered.

### File operations (`ops.rs` + `panel.rs`)

`ops.rs` is pure and synchronous — the app runs it on the background executor,
so no I/O touches a frame. Names are validated before anything reaches the
filesystem (empty, `.`/`..`, separators, control characters, and over-long
names are rejected with a locale keyed message), `create_*` use
`create_new`/`create_dir` so they never clobber, and `rename` refuses an
existing destination.

The panel owns the inline prompts: **New File** / **New Folder** from the
header or a directory's context menu, **Rename** from any row's context menu
(edited in place), and **Delete** via a confirmation card. The panel sends a
[`FileOpRequest`] (workspace-relative) to the app, which resolves the absolute
path, performs the operation, then reconciles the tree and any open Files tabs
(`FileViewer::reconcile_rename` / `close_under`). Rename and delete are refused
while a tab under the target has unsaved edits, so an operation can never race
the autosave or discard a buffer. Delete moves to the OS Trash on macOS
(`platform::trash_path`, `NSFileManager`) and falls back to a permanent delete
elsewhere — the confirmation is worded from `TRASH_IS_RECOVERABLE`, so it never
promises a restore the platform cannot deliver.

## UI

### ProjectPanel

- Header: workspace name, hidden-files toggle, collapse-all, refresh.
- Filter field: a `ComposerInput` (the app's input entity) that filters rows.
- Body: `list()` virtualization, fixed 28 px rows, reusing `file_glyph` for
  icons. Row hover/selection colors match `sidepane.rs` (`overlay` /
  `overlay_strong`).
- Keyboard: `↑`/`↓` move, `→` expand, `←` collapse, `home`/`end` jump,
  `enter` opens (or toggles a dir), `cmd-→`/`cmd-←` expand/collapse all.
  Focus handle carries a `ProjectPanel` key context.
- Footer/status: entry count, "truncated" note when the cap hit.
- Context menu: opened at the pointer. A directory offers Expand/Collapse,
  New File, New Folder, Rename, Delete, Open in Default App, Reveal, and Copy
  Path / Relative Path; a file offers Open, Rename, Delete, Open in Default
  App, Reveal, and Copy Path / Relative Path.

### FileViewer (Files page)

- Tab strip: open paths, active tab, close button per tab, and an accent dot
  for unsaved edits. `cmd-w` closes the active tab (the whole surface when it
  was the last tab); `cmd-shift-w` closes the surface. Closing flushes first.
- Focus: opening, closing, or switching a tab moves focus to the new active
  editor; when that tab is a preview or read-only body the surface itself takes
  focus, so the `Files` shortcuts (`cmd-w`, `cmd-s`) keep working instead of
  being dropped with the removed editor.
- Toolbar: breadcrumb of the path, file size, line count, and a status chip —
  autosave state for editable files, a lock/read-only hint otherwise.
- Body: editable `ComposerInput` (gutter, no wrap, fill) for code/text and
  for Markdown source (toggle to preview); the preview renders in a centered
  column like the Skills page; Binary/TooLarge render a centered notice;
  truncated/lossy text renders the read-only highlighted list.
- Empty state: “Select a file to view” with the tree’s keyboard hint.

## Integration points

| Concern | File | Change |
|---|---|---|
| Module registration | `src/main.rs` | `mod explorer;` |
| Actions + keys | `src/main.rs` | `ToggleProjectPanel`, `OpenFileFinder`; `cmd-shift-e` |
| App state | `src/app.rs` | `project_panel: Entity<ProjectPanel>`, `file_viewer: Entity<FileViewer>`, `files_open: bool`, `explorer_width: Pixels`, `explorer_visible: bool` |
| Toggle/open/close | `src/app/session.rs` | helpers beside `open_git` / `close_git` |
| Layout + dispatch | `src/app/view.rs` | dock after sidebar; resize drag-move listener; Files branch in the `:612` dispatch chain; top-bar toggle |
| Command palette | `src/command_palette.rs` | `PaletteCommand::ToggleProjectPanel`, `OpenFileFinder`; snapshot flags |
| Watching | `src/app/events.rs` | consume `workspace_watcher` dirty → mark explorer stale |
| i18n | `locales/en.yml` + `scripts/gen_locales.py` | new `explorer.*` keys |
| Docs | `AGENT.md`, `PRODUCT.md`, `CHANGELOG.md` | file map + capability + entry |

## Performance rules

- Snapshot, git status, file reads, **and file writes** are
  background-executor jobs; the UI only ever indexes in-memory state.
- Rows are a flat `Vec` rendered through `list()`; frame cost is independent of
  tree size.
- The editor re-tokenizes the buffer per paint through the memoized
  `highlight::tokenize_cached`; the same cache keys the read-only viewer and
  transcript blocks.
- Autosave is debounced (500 ms) and coalesced through a save epoch, so typing
  never queues a write per keystroke.
- The `WorkspaceWatcher` already debounces and filters; reuse it rather than
  starting a second watch. A dirty signal re-reads the active file; the
  completion distinguishes our own save from an external change.

## Risks

- **Huge repositories** → entry cap + `truncated` flag; never walk without a
  bound.
- **Symlinks** → never followed.
- **Non-UTF-8 / binary / huge files** → explicit `Binary` / `TooLarge` modes
  with the real size, never a lossy dump. Lossy and truncated text is
  read-only, so a save cannot corrupt it.
- **Watch storms** → debounced watcher, filtered paths (existing behaviour);
  our own writes echo back and are recognised by comparing disk text to the
  last written text.
- **Crash mid-write** → saves go to a sibling temp file and rename, so the
  real file is never left half-written.
- **GPUI pre-1.0** → only `list()`/`uniform_list`, `ComposerInput`, and the
  existing icon/highlight helpers; no new GPUI surface area.
- **`ignore` dependency** → already in the lock file; documented reason.

## Testing

- `tree.rs`: flattening with expand/collapse, dirs-first ordering, filter
  pruning, empty-dir retention, badge mapping.
- `walk.rs`: `.gitignore` respected, hidden toggle, entry cap/truncation,
  symlinks not followed.
- `viewer.rs`: binary sniff, size cap, UTF-8-lossy decode, language mapping,
  editability gating (lossy/truncated stay read-only), atomic save round-trip.
- `composer.rs`: geometry tests cover the gutter/fill/syntax editor path
  (paint + gutter-aware hit-testing) alongside the existing caret tests.
- i18n completeness (`cargo test -p orbit-pi i18n`).
- `cargo build --workspace` clean (zero warnings); `cargo test --workspace`.

## Rollout

1. ✅ **Data layer** — `explorer/walk.rs` + `explorer/tree.rs`, unit-tested in
   isolation (`cargo test -p orbit-pi explorer`).
2. ✅ **Project panel** — `explorer/panel.rs` + `OrbitApp` field + right dock +
   resize + toggle + filter + keyboard nav + git badges.
3. ✅ **File viewer** — `explorer/viewer.rs` + Files page dispatch + tab strip +
   highlight/markdown/image/binary modes.
4. ✅ **Integration** — command palette, `cmd-shift-e` / `cmd-w`, watcher-driven
   refresh, open-in-editor/reveal, i18n keys, docs.
5. ✅ **Editing** — `ComposerInput` syntax/gutter/wrap/fill modes + per-tab
   editors + debounced autosave (`cmd-s`) + dirty/conflict status.
6. ✅ **File operations** — New File / New Folder / Rename (inline name prompts)
   and Delete (confirmed, moved to the Trash) from the panel header and row
   context menu; `ops.rs` validates names and never clobbers; the app
   reconciles open Files tabs and refuses while a buffer is dirty.
7. ⬜ **Later (out of scope)** — quick-open (`cmd-p`), directory folding, sticky
   scroll, git-diff-vs-head on file select.
