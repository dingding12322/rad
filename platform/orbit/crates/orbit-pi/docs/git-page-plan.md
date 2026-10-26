# Git Page — Full GitHub Client Plan

Status: Phase 0 complete (structure + foundations); Phases 1–7 pending
Owner: TBD
Decisions locked: `gh` CLI only · read + full write issues/PRs · GitHub only · guided merge/rebase commands + recovery banner

## Progress

- **Phase 0 — done.** `git_panel.rs` split into `git_panel/{mod,failure,widgets}.rs`;
  structured `ActionError` / `FailureKind` / `RecoveryAction` in `failure.rs`;
  `gh.rs` added (binary resolution, `--version`/`auth status` probe, typed
  `GhStatus`, `parse_version`); Issues/Pull requests tabs appear when `gh` is
  installed and show an honest install/sign-in empty state until Phases 5–6
  land their browsers. `en.yml` + all generated locales updated.
  Deviation: the host tabs are hidden when `gh` is not installed, so a repo
  without the CLI keeps the page exactly as it was before. The typed issue/PR
  models land with the phases that consume them.
- **Phase 1 — done.** `git_ops.rs` added (operation-state detection from `.git`
  markers, conflicted-file listing, continue/abort/skip for merge/rebase/
  cherry-pick/revert/bisect, all non-interactive via a `:` editor).
  `FailureKind` widened to the server-side families (secret scan, large file,
  LFS, protected branch, pre-receive hook) plus rebase conflicts and dirty
  worktrees, with per-kind `RecoveryAction`s. The failure banner now renders
  recovery buttons (Pull, Merge, Rebase, force-with-lease, publish, re-auth,
  retry, resolve/abort/continue/skip); force push and abort are confirmed.
  A single operation bar sits above every tab with Continue / Skip / Abort,
  the conflict count, and clickable conflicted-file chips that open the Files
  editor (new `OpenPath` callback wired in `app.rs`). `git.rs` gained
  `push_force_with_lease` (lease proven by a local-remote test) and
  `rebase_upstream`. `cargo test -p orbit-pi` green (621 tests).
- **Phase 2 — done (core).** `git_ops.rs` gained `list_refs` (local, remote,
  tag; `origin/HEAD` excluded), `merge_ref` (`Merge`/`NoFf`/`Squash`),
  `rebase_ref` (autostash when the worktree is dirty), the stash stack
  (`list/push/pop/apply/drop`), and `rename_branch`/`delete_branch`. The branch
  menu now opens **Merge branch…** / **Rebase onto…** pickers (with a
  fast-forward / merge-commit / squash toggle), lists every non-current branch
  with a safe-`-d`-then-confirmed-`-D` delete, and adds **New branch…** /
  **Rename current branch…** prompts. The Changes tab carries a collapsible
  **Stashes** section (stash all, pop, apply, drop). Deferred to a later pass:
  per-hunk (`stage_patch`) staging, "Compare to current", and a set-upstream
  UI. `cargo test -p orbit-pi` green (625 tests).
- **Phase 3 — done (core).** `git.rs` gained `commit_detail` (+ `CommitDetail`/
  `CommitFile`), `history_filtered` (+ `HistoryFilter`, with `--follow`), and
  tests for all three. History rows now expand in place into a detail panel:
  full commit body, author/committer/date facts, **Open on GitHub** and copy
  hash, and the changed-file list with per-file ± counts. A file actions menu
  (the `⋯` on each file) opens the file in the Files editor, opens its diff in
  Review, reveals it in the File Manager, copies the path, or filters History
  to that file's `--follow` history. The filter bar has an **All branches**
  toggle, removable author and path chips (click an author name to filter by
  them), and a Clear action. Deferred: the `git blame` view, "open file/diff at
  this revision" (needs range support in the Review pane + Files viewer), and a
  free-text message-grep input. `cargo test -p orbit-pi` green (628 tests).
- **Phase 4 — done (core).** `graph_history` gained an `all_refs` flag
  (`--branches HEAD` plus `--remotes --tags`), and the Graph tab has an
  **All refs** toggle. Graph rows now behave like History rows: clicking expands
  the same commit-detail panel (full body, changed files, per-file actions),
  the forge link moved to its own icon button, and a chevron shows expansion.
  New tests: an octopus merge lays out three distinct parent lanes, and
  `--all-refs` includes a commit that only a remote-tracking ref points to.
  Deferred: visually muting lanes for remote-only commits (needs a reliable
  local-reachability set; ref badges already distinguish them).
  `cargo test -p orbit-pi` green (630 tests).
- **Phase 5 — done (core).** `gh.rs` gained `GhUser`/`GhLabel`/`GhComment`/
  `GhIssue`, `IssueFilter`/`IssueState`, `list_issues`, `view_issue`,
  `create_issue`, `comment_issue`, `edit_issue_labels`, `close_issue`,
  `reopen_issue`, `list_labels`, and `relative_time`, with serde tests against
  real `gh --json` shapes. The Issues tab is real: Open/Closed/All chips, a
  search field, a repository-label filter/picker (clicking a label in an open
  issue toggles it on the issue), a **New issue** form (title + Markdown body),
  list rows (state, labels, author avatar, comment count, relative time), and a
  detail view with the rendered Markdown body, every comment, a comment
  composer, **Close/Reopen**, **Edit labels**, and **Open on GitHub**. Setup
  states still cover a missing or signed-out `gh`. Deferred: assignee editing,
  and virtualizing the issue list (bounded at `gh`'s 50-result page).
  `cargo test -p orbit-pi` green (633 tests).
- **Phase 6 — done (core).** `gh.rs` gained `GhCheck` (CheckRun + StatusContext,
  normalized to a `CheckBucket`), `GhReview`, `GhPullCommit`, `GhPullFile`,
  `GhPull` (+ `checks_summary`/`is_open`/`is_merged`), `PrFilter`/`PrState`,
  `MergeMethod`, `ReviewAction`, and `list_pulls`, `view_pull`, `create_pull`,
  `comment_pull`, `review_pull`, `merge_pull`, `close_pull`, `reopen_pull`,
  `checkout_pull`, with serde tests for the PR and check shapes. The Pulls tab
  is real: Open/Closed/Merged/All chips, search, a **New pull request** form
  (title, Markdown body, base-branch picker, draft toggle), list rows with CI
  rollup and review-decision chips, and a detail view with the Markdown
  description, checks (each linking out), commits, changed files (diff opens in
  Review), comments, submitted reviews, a comment/review composer,
  **Approve** / **Request changes**, **Merge** behind a method + delete-branch
  confirmation, **Close/Reopen**, **Checkout branch**, and **Open on GitHub**.
  `gh pr checkout` is refused unless the worktree is clean and no operation is
  in progress. Deferred: an inline PR diff (files open the working-tree diff
  via Review), label/assignee editing on PRs, and a neutral review-comment
  action. `cargo test -p orbit-pi` green (636 tests).
- **Phase 7 — done (core).** Keyboard: `⌘1`–`⌘5` switch the Git page's tabs
  (handled globally but a no-op unless the page is open). Reduce motion now
  renders the Git page's spinner statically. Performance: the `gh` probe is
  cached (first load, workspace change, and the manual refresh only) instead of
  running two subprocesses on every tab switch; commit/PR changed-file lists
  cap at `MAX_FILE_ROWS` (300) and summarize the rest; issue/PR lists stay
  bounded at `gh`'s 50-result page. i18n: every new key is in `en.yml` and
  propagated to all nine generated locales (English fallback where a glossary
  entry does not exist yet), and the completeness test is green. Docs:
  `crates/orbit-pi/docs/git-page.md` documents the tabs, recovery model,
  operation state machine, `gh` requirement, and limits, and the `AGENT.md` Git
  page row is rewritten to match. Deferred: real translations for the new
  strings (glossary-driven), virtualizing the issue/PR lists, a keyboard row
  cursor, `git blame`, open-at-revision, per-hunk staging, and assignee
  editing. `cargo test -p orbit-pi` green (636 tests); clippy clean apart from
  the pre-existing `section_header` lint.

All seven phases are now implemented. The plan remains the reference for the
feature's shape; `docs/git-page.md` is the user-facing design note.

## Goal

Turn the existing three-tab Git page (`Changes` / `History` / `Graph`) into a
complete, local-first Git + GitHub workbench: commit and stage with confidence,
merge/rebase with guided recovery, deeply inspect history and its files, and
read/write issues and pull requests without leaving Orbit.

Everything stays GPUI-only (hard rule #1), all Git/`gh` I/O runs off the UI
thread (hard rule #4), and no host data is ever faked when `gh` is missing or
unauthenticated (hard rule #5).

## Current baseline (what exists today)

- `src/git.rs` (~1335 lines) — pure plumbing: `status_rows`, `stage_paths`,
  `unstage_paths`, `discard_paths`, `commit`, `push`, `pull`, `merge_upstream`,
  `fetch`, `history`, `history_by_author`, `graph_history`, `layout_graph`,
  `remote_web`, `ahead_behind`, `file_at_head`.
- `src/git_panel.rs` (~2916 lines) — one `GitPanel` entity with tabs `Changes`,
  `History`, `Graph`; commit bar; branch menu; failure banner with
  `classify_git_failure`; `spawn_data` helper for off-thread work.
- `src/commit_message.rs` — one-shot `pi -p` conventional-commit generation with
  heuristic fallback.
- App wiring: `app/session.rs::open_git`, `app/view.rs` renders the panel when
  `git_open`; `app.rs` installs `set_open_file` (opens a file diff in the Review
  side pane) and `set_on_close`.
- No host API client exists. `remote_web` only builds commit permalinks for
  GitHub/GitLab/Bitbucket.

## Design principles

1. **One operation state machine.** Merge, rebase, cherry-pick, revert, and
   stash conflicts all surface through the same "operation in progress" bar and
   the same recovery model. Never a one-off dialog per command.
2. **Errors are actionable, not just readable.** A failed Git command becomes a
   typed `ActionError { kind, title, detail, actions }` where `actions` are real
   buttons (Pull, Merge, Force-with-lease, Rebase --continue, Set upstream,
   Re-authenticate, Retry). The raw stderr stays copyable.
3. **`gh` is the only host transport** (decision). It owns auth and the API. We
   shell out with `--json`, parse into typed structs, and cache. If `gh` is
   absent or unauthenticated, the Issues/Pulls tabs render an honest setup empty
   state with the exact install/auth command and a copy button.
4. **Local Git works with zero network and zero `gh`.** Host tabs are additive.
5. **Everything testable without a network.** `gh` parsing is pure (serde),
   operation-state detection is pure (reads `.git` files), failure
   classification is pure. `gh` invocation is covered with a fake `gh` script
   on `PATH` (mirrors `crates/orbit-rpc/tests/auth_fake_pi.rs`).

## New / changed modules

| Path | Change |
|---|---|
| `src/gh.rs` | **New.** `gh` binary resolution, auth probe, typed issue/PR models, list/view/create/comment/edit/close/reopen/merge/review/checkout wrappers. Pure parsers + thin command runner. |
| `src/git_ops.rs` | **New.** Local operations not in `git.rs`: merge a ref, rebase a ref, continue/abort/skip, operation-state detection, stash, branch create/rename/delete/upstream, patch-based hunk staging. |
| `src/git_panel/` | **Split** the current `git_panel.rs` into `mod.rs` (entity + state + helpers), `changes.rs`, `history.rs`, `graph.rs`, `issues.rs`, `pulls.rs`, `commit_bar.rs`, `failure.rs` (banner + recovery actions). Keeps each file reviewable while adding five tabs. |
| `src/git.rs` | Extend: commit detail (`show` with message body + file list + numstat), file history (`log --follow`), blame, history filters, refs listing with kinds, patch building for hunk staging. |
| `src/platform.rs` | Already has `reveal_in_file_manager`; reuse. Add nothing unless a GitHub URL helper is needed. |
| `src/app.rs`, `src/app/session.rs`, `src/app/view.rs` | Wire new callbacks (open file at revision, open in explorer, checkout PR branch, open URL), and command-palette entries. |
| `locales/en.yml` + `scripts/i18n_glossary*.py` | Every new string. Regenerate all locales. |
| `AGENT.md` + `docs/git-page.md` | Update the Git page row and document the surface. |

## Phases

### Phase 0 — Refactor + foundations (no user-visible change)

- Split `git_panel.rs` into `src/git_panel/` with `mod.rs` re-exporting
  `GitPanel`; move `commit_row`, `graph_row`, `ref_badge`, etc. to their tab
  modules. Mechanical move only; behavior unchanged.
- Introduce `ActionError` and `RecoveryAction` in `git_panel/failure.rs`.
  Replace `GitFailure`/`set_failure` internals while keeping the banner's
  current look; `classify_git_failure` moves here and returns a structured
  value.
- Add `src/gh.rs` skeleton: `gh_binary()` (env `GH_BIN` override, then PATH,
  mirroring `orbit_rpc::pi_binary`), `run_gh(cwd, args) -> Result<String>`
  (uses `orbit_rpc::hide_console`, `GH_PROMPT_DISABLED=1`,
  `GH_NO_UPDATE_NOTIFIER=1`, `NO_COLOR=1`), `auth_status()`, and empty typed
  models. Unit-test parsers with JSON fixtures.
- Add `GitTab::{Issues, Pulls}` variants and tab scaffolding behind a
  `gh_available` flag; render a setup empty state for now.

Exit gate: existing tests green, `cargo clippy` clean, page looks identical.

### Phase 1 — Push/merge error handling + recovery (explicit request)

- Expand `classify_git_failure` into kinds: `PushRejected`, `Diverged`,
  `MergeConflict`, `RebaseConflict`, `LocalOverwritten`, `Protected`,
  `PreReceiveHook`, `Auth`, `RemoteMissing`, `Network`, `Lfs`, `LargeFile`,
  `SecretScan`, `NoUpstream`, `DirtyWorktree`, `Generic`. Each maps to a title,
  the full detail, and a list of `RecoveryAction`s.
- Failure banner gets action buttons:
  - `PushRejected` → **Pull**, **Merge**, **Force push (with lease)**
    (force confirms and uses `--force-with-lease`, never bare `--force`).
  - `Diverged` → **Merge**, **Rebase**.
  - `*Conflict` → **Open conflicts**, **Abort operation**, **Continue**.
  - `Auth` → **Re-authenticate** (runs `gh auth login` in the terminal panel or
    opens credential help), **Retry**.
  - `NoUpstream` → **Publish branch**.
  - `Network` → **Retry**.
- Operation-in-progress bar, shown above the tab body whenever
  `git_ops::operation_state(cwd)` reports merge/rebase/cherry-pick/revert/bisect:
  a labeled strip with **Continue**, **Abort**, **Skip** (where valid) and the
  conflict file count. It is the single source of truth for "you are mid-merge".
- On any rejected push, still `git fetch` (already done) and refresh branch
  state so the recovery actions are correct.
- Tests: every kind/action mapping, force-with-lease command shape, operation
  detection from synthetic `.git` dirs.

### Phase 2 — Merge, rebase, stash, branch management

- `git_ops.rs`:
  - `merge_ref(cwd, ref, MergeMode::{Merge, NoFf, Squash})`,
    `merge_continue`, `merge_abort`.
  - `rebase_ref(cwd, ref)`, `rebase_continue`, `rebase_abort`, `rebase_skip`;
    `--autostash` when the worktree is dirty and the user confirms.
  - `operation_state(cwd) -> Option<InProgress>` from `.git/MERGE_HEAD`,
    `.git/rebase-merge`, `.git/rebase-apply`, `CHERRY_PICK_HEAD`,
    `REVERT_HEAD`, `BISECT_LOG`.
  - `stash_push/pop/apply/list/drop`, `branches_with_kind()` (local/remote/tag),
    `create_branch`, `rename_branch`, `delete_branch` (safe `-d` default, `-D`
    behind confirmation), `set_upstream`.
  - Patch-based `stage_patch` / `unstage_patch` for per-hunk and per-line
    staging (`git apply --cached` on a constructed diff), so the Changes tab
    gets hunk-level buttons without an interactive `add -p`.
- UI:
  - Commit bar's merge area becomes a **Merge/Rebase** split with a target ref
    picker (local branches, remote branches, tags). "Merge branch…" and
    "Rebase onto…" entries.
  - Branch menu gains create/rename/delete/set-upstream and "Compare to
    current".
  - Stash section in Changes (collapsed by default): list, pop, apply, drop.
  - Conflict files open in the Files editor (decision: no 3-way resolver);
    the in-progress bar links to them.

### Phase 3 — Proper history + history files + open on explorer

- `git.rs` additions:
  - `commit_detail(cwd, hash)` → full message (subject + body), author/committer
    with dates, parents, refs, and per-file `--numstat`.
  - `commit_files(cwd, hash)` → `name-status` + numstat rows.
  - `file_history(cwd, path, limit, skip)` → `log --follow -- path`.
  - `blame(cwd, path, rev)` → parsed line/author/commit rows.
  - `history_filtered(cwd, HistoryFilter { author, path, grep, since, until,
    branch/all })`.
  - `refs_with_kinds(cwd)` → local/remote/tag labels for the graph toggle.
- History tab:
  - Filters row (author, path, message grep, date, all-branches toggle).
  - Commit rows expand in place to a file list (name-status + ± counts); a
    chevron toggles. Row click selects the commit into a detail pane (right
    side on wide layouts, inline on narrow).
  - Commit detail: full body, hash + copy, author/committer, **Open on GitHub**,
    changed files with per-file actions.
- File actions (context menu on any history file row, and on the commit detail
  file list):
  - **Open file** — working tree via the Files surface
    (app callback `on_open_path`).
  - **Open at this revision** — read-only Files view loaded from
    `git show <rev>:<path>`; new `viewer` entry point / callback.
  - **Open diff** — existing `on_open_file` → Review pane (`SidePane::show_file`).
  - **Reveal in File Manager** — `platform::reveal_in_file_manager`.
  - **Copy path** / **Copy relative path**.
  - **File history** — switches History to `--follow` filtered on that path.
  - **Blame** — opens a blame view (read-only list: line, short hash, author).
- Tests: `commit_detail` parsing, `--follow` range plumbing, name-status
  parsing, and a test that every file-action maps to the right target path.

### Phase 4 — Graph depth

- `graph_history` gains an `include_remotes` / `include_tags` option
  (`git log --branches --remotes --tags --date-order`).
- Show tag badges (new `RefKind::Tag` already exists) and remote branch tips;
  remote-only commits get a muted lane.
- Clicking a graph node selects the same commit-detail pane as History, so the
  file actions work there too.
- Toggle chip: **Local** / **All refs**. Persisted per repo in
  `~/.orbit-pi/ui.json` (or session-only if persistence is out of scope).
- Keep the existing pure `layout_graph`; add a test for octopus merges and for
  remote refs appearing without breaking lane geometry.

### Phase 5 — GitHub Issues tab (`gh`)

- `gh.rs` issue API (all with `--repo <owner/repo>` when known):
  - `list_issues(cwd, IssueFilter { state, label, assignee, search, limit })`
    via `gh issue list --json number,title,state,author,labels,assignees,
    comments,createdAt,updatedAt,body,url`.
  - `view_issue(cwd, number)` → body + comments + labels + assignees + linked
    PRs.
  - `create_issue(title, body, labels, assignees)`,
    `comment_issue(number, body)`,
    `edit_issue(number, add/remove labels, add/remove assignees)`,
    `close_issue(number, reason)`, `reopen_issue(number)`.
- Issues tab:
  - Filter bar (Open/Closed/All, label, assignee, search), virtualized list
    (`list()`), row meta: number, title, labels, author avatar, comment count,
    relative time, state chip.
  - Detail: rendered markdown body + comments (reuse
    `transcript_view::render_markdown_document`), comment composer, label and
    assignee editors, close/reopen, **Open on GitHub**, copy number/URL.
  - **New issue** composer (title + markdown body, label/assignee pickers).
- Auth/setup empty states: `gh` missing → install command; `gh` present but not
  authenticated → `gh auth login` command; not a GitHub repo → explanatory
  state. Each with a copy button (and a **Run in terminal** action that reuses
  the bottom terminal panel).

### Phase 6 — GitHub Pull Requests tab (`gh`)

- `gh.rs` PR API:
  - `list_pulls(cwd, PrFilter { state, base, head, author, search, limit })`
    via `gh pr list --json number,title,state,isDraft,author,labels,
    reviewDecision,mergeable,mergeStateStatus,headRefName,baseRefName,
    statusCheckRollup,additions,deletions,changedFiles,updatedAt,url`.
  - `view_pull(number)` → body, comments, reviews, checks, commits, files.
  - `create_pull(title, body, base, head, draft, labels)`.
  - `comment_pull`, `review_pull(approve|request_changes|comment, body)`.
  - `merge_pull(number, method: Merge|Squash|Rebase, delete_branch, auto)`.
  - `close_pull`, `reopen_pull`, `checkout_pull(number)`.
- Pulls tab:
  - List with state/base/author/search filters; rows show CI status rollup,
    review decision, draft state, ± and file counts.
  - Detail: markdown description, timeline (comments + reviews), checks list
    (pass/fail/pending with links), commits, changed files (reusing the Phase 3
    file actions), and a diff view (`gh pr diff` → Review pane parser).
  - Actions: **Create PR** from the current branch (base picker), comment, review
    (approve / request changes / comment), **Merge** with method choice and
    delete-branch toggle, close/reopen, **Checkout branch** (`gh pr checkout`,
    then refresh local state).
  - Guard mutations behind confirmations where destructive (merge/close/delete
    branch); show an optimistic busy state and reconcile from `gh` after.

### Phase 7 — Polish, i18n, docs, verification

- i18n: add every new key to `locales/en.yml`, add translations to the matching
  `scripts/i18n_glossary_*.py`, run `python3 scripts/gen_locales.py`, then
  `cargo test -p orbit-pi i18n`. No hard-coded English in render paths.
- Keyboard: `⌘1`–`⌘5` switch tabs when the Git page is focused; `Enter` opens a
  selected row; `Escape` backs out of detail → list → page (already does page).
  Respect `theme.ui.reduce_motion` for spinners and any list transitions.
- Empty/loading/error states for every list and detail (honest, actionable).
- Performance: virtualize issue/PR lists and commit file lists; cache `gh`
  lists with a short TTL + manual refresh; never block a frame.
- Docs: update the `AGENT.md` Git page row and add `docs/git-page.md` covering
  tabs, `gh` requirement, recovery model, and the operation state machine.
- Verification:
  - `cargo test -p orbit-pi` (unit + pure parsers + fake `gh`).
  - `cargo clippy --workspace --all-targets -- -D warnings`.
  - Manual matrix: repo with no remote; GitHub remote without `gh`; with `gh`
    but logged out; logged in; detached HEAD; unborn branch; dirty worktree
    during rebase; a rejected push; a real conflict; an issue create/close; a
    PR create/approve/merge.

## Risks / open questions

- **`gh` version skew.** `--json` field availability changes across v2.x.
  Mitigation: probe `gh --version`, request only fields known to the installed
  version, and surface a clear "update gh" error instead of a parse panic.
- **`gh pr checkout` mutates the working tree.** Must refuse when the worktree
  is dirty or mid-operation, and go through the same dirty-worktree guard as
  checkout/rebase.
- **Force push safety.** Only `--force-with-lease`, always behind a typed or
  explicit confirmation; never bare `--force`.
- **Markdown safety.** Reuse the existing transcript renderer, which is already
  local and does not execute HTML.
- **Rate limits / cost.** `gh` handles auth and pagination; lists are capped and
  cached. Detail views fetch on demand.
- **Scope of "manage".** Locked to full write for issues and PRs (create,
  comment, label, assign, close/reopen, review, merge). Repo settings, releases,
  Actions, and org management are explicitly out of scope for this plan.
