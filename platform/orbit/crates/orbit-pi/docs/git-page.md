# Git page

The Git page is a full main-area surface (`src/git_panel/`) that combines local
Git work with the GitHub CLI. It opens from the session-details **Commit or
push** row or the top-bar Git affordance; `escape` / **Back** returns.

## Worktrees

The page always operates on the **active workspace**, which may be a linked Git
worktree (`repo/.wt/113`) rather than the main repository. Every Git command
(`status`, `diff`, `branch`, `log`, `commit`, `push`, `pull`, staging, graph,
stash) runs with that path as its cwd, so the Changes/History/Graph tabs show
that worktree's branch and working tree. Worktree lifecycle — creation,
removal, moving, locking, pruning — lives on the Worktrees page
(`app/worktrees.rs`) and is not duplicated here; see
[worktrees.md](./worktrees.md).

## Tabs

| Tab | Source | What it does |
|---|---|---|
| **Changes** | `git status --porcelain` | Staged/unstaged lists with per-file stage/unstage and confirmed discard; Include-unstaged toggle; commit bar (branch menu, conventional-commit input, **Generate**). |
| **History** | `git log` | Commit rows with ref badges, author, relative time, paged. Clicking a row expands it in place: full body, author/committer facts, **Open on GitHub**, copy hash, and the changed files with a per-file actions menu. |
| **Graph** | `git log --branches HEAD` (plus `--remotes --tags` with **All refs**) | Lane graph laid out by `git::layout_graph`, with the same expandable commit detail as History. |
| **Stashes** | `git stash list` | The stash stack newest first: **Stash changes**, then pop / apply / drop per entry. |
| **Issues** | `gh issue …` | Open/Closed/All, search, label filter/picker, **New issue** (with **Generate**), list, and a detail view with the rendered Markdown body, comments, a comment composer, **Close/Reopen**, **Edit labels**, and **Open on GitHub**. |
| **Pull requests** | `gh pr …` | Open/Closed/Merged/All, search, **New pull request** (title, body, base picker, draft, **Generate**), list with CI rollup and review-decision chips, and a detail view with description, checks, commits, changed files, comments, reviews, **Approve** / **Request changes**, **Merge** (method + delete-branch confirm), **Close/Reopen**, and **Checkout branch**. |

The Issues and Pull requests tabs only appear when `gh` is installed. A missing
`gh` or a signed-out CLI renders an honest setup state instead of an empty tab.

## Commit bar

The action cluster follows Git state: uncommitted changes → **Commit** /
**Commit and push**; clean with unpushed commits → **Push** (**Publish branch**
when there is no upstream); clean and behind → **Pull**; diverged → **Merge**;
otherwise a quiet "Up to date". Push/pull never touch the message. A blank
Commit message auto-generates through `commit_message.rs`
(`pi -p --no-tools --no-session --no-extensions --no-skills --no-context-files`,
falling back to a local `type(scope): …` heuristic).

## Issue and pull-request draft generation

The **New issue** and **New pull request** forms have a **Generate** button beside their
heading. An optional **Notes** field steers the draft; leave it blank to infer the subject
from the repository context. Generation reuses the commit-message pattern: a one-shot,
tool-free `pi -p` call (`issue_message.rs`) that reads the current branch, the branch's
commits (against the chosen base for a PR), and its changed-file stat — never writing to
the working tree or the active session. The model is asked for a single fenced block with
the title on the first line and a Markdown body below it.

### Repository templates

Both forms discover the repository's own templates (`gh_templates.rs`, mirroring GitHub's
lookup): the `.github/ISSUE_TEMPLATE/` directory (Markdown templates with YAML front
matter, and the newer YAML issue forms, `config.yml` skipped) plus the legacy
single-file locations, and `.github/PULL_REQUEST_TEMPLATE.md` plus its variants. A form
with exactly one template applies it automatically; with several, a **Template** chip
opens a picker (including a **Blank** choice). Choosing a template prefills the title
prefix (`bug: `) and the body skeleton, and **Create** passes the template's `labels:` to
`gh`. An issue form is flattened into Markdown — each field's label becomes a heading,
its description a guidance comment, dropdown/checkbox options rendered as comments and
checkboxes — since Orbit edits plain Markdown rather than GitHub's dynamic form.

The **Generate** prompt hands the model the selected template and asks it to fill every
heading faithfully, preserving the template's checkboxes and sections. Without pi, the
heuristic drops the notes (or the branch's commit subjects) into the template's first
section.

When pi is missing, unauthenticated, or times out, the button falls back to a local
heuristic: the title comes from the first note line, else the latest commit subject, else
the branch name read as words; the body lists the branch's commits and changed files under
`## Summary` / `## What happened`. Either way the result lands in the editable title and
body fields, and **Create** is still a separate, deliberate press.

The Issues and Pull requests lists are built to scan: labels collapse to a `+N` chip, each
row ends in a fixed relative-time column, and closed items are de-emphasized. The issue
detail's reading-column + metadata-rail layout is now shared by the pull-request detail,
whose rail summarizes review, checks, changes, branches, and the timeline.

The branch chip opens a menu with **Merge branch…**, **Rebase onto…**, every
local branch (checkout; delete uses safe `-d` first, then a confirmed `-D`),
**New branch…**, and **Rename current branch…**. Merge can fast-forward, create
a merge commit, or squash; rebase autostashes a dirty worktree.

## Failure classification and recovery

All Git failures run through `git_panel/failure.rs`, which classifies raw stderr
into an `ActionError { kind, title, detail }`. The banner keeps the full output
(copyable) and offers the recovery actions that are valid for that kind:

- push rejected → **Pull**, **Merge**, **Force push (with lease)**
- diverged → **Merge**, **Rebase**
- merge/rebase conflicts → **Resolve conflicts**, **Abort**, **Continue**,
  **Skip** (rebase/cherry-pick only)
- auth → **Re-authenticate** (copies `gh auth login`), **Retry**
- no upstream → **Publish branch**; network → **Retry**

Force push and abort are confirmed. Force push always uses
`--force-with-lease`, never bare `--force`.

## Operation in progress

`git_ops::operation_state` detects a half-finished merge, rebase, cherry-pick,
revert, or bisect from the marker files Git writes under `.git`. When one is
active a single bar sits above every tab with **Continue**, **Skip** (where
valid), **Abort**, the conflict count, and clickable conflicted-file chips that
open the file in the Files editor. Continue is disabled until the conflicts are
resolved and staged.

## History detail and file actions

Selecting a commit (History or Graph) loads `git::commit_detail` — full message
body, author/committer/dates, parents, refs, and the changed files with
`--numstat` counts. Each file's `⋯` menu offers:

- **Open file** (Files editor, working tree)
- **Open diff** (Review pane, working tree)
- **Reveal in File Manager**
- **Copy path**
- **File history** (filters History to `git log --follow -- <path>`)

The History filter bar has an **All branches** toggle, removable author and path
chips (click an author name to filter by them), and **Clear**.

## Keyboard

- `⌘1`…`⌘6` switch tabs while the page is open (a no-op elsewhere)
- `escape` dismisses a modal, then the branch/ref/file popovers, then leaves the
  page via the Back affordance

## GitHub CLI dependency

All host features shell out to `gh` (`src/gh.rs`) and parse its `--json` output
into typed models. Orbit never stores a GitHub token and never calls
`api.github.com` directly. `GH_BIN` overrides the binary; the minimum supported
`gh` is 2.20.0. Commands run with `GH_PROMPT_DISABLED=1`, `GH_PAGER=cat`,
`NO_COLOR=1`, and no update notifier, off the UI thread.

`gh pr checkout` rewrites the working tree, so it is refused while the worktree
is dirty or a merge/rebase is in progress.

## Limits and deferred work

- Commit/PR changed-file lists render at most `MAX_FILE_ROWS` (300) rows and
  summarize the rest.
- Issue and PR lists are bounded at `gh`'s 50-result page and are not yet
  virtualized.
- Not yet wired: `git blame`, opening a file/diff at a specific revision (the
  Review pane and Files viewer are working-tree only), per-hunk staging,
  assignee editing on issues, label/assignee editing on PRs, a neutral review
  comment, and an inline PR diff.
