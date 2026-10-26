# Contributing to Orbit

Thanks for your interest in Orbit. This guide covers how to set up the project,
what we expect from changes, and how to get them merged.

## Before you start

- **Open an issue first** for anything larger than a bug fix or a small
  documentation change. Architectural work is recorded in `INTENT.md`, so a
  quick discussion saves rework.
- Read [`AGENT.md`](AGENT.md) before touching the code. It is the project's
  convention and protocol reference. When it conflicts with
  [`INTENT.md`](INTENT.md) on architecture, `INTENT.md` wins.
- By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to contribute

- Report bugs with the [issue form](https://github.com/imrj05/orbit/issues/new/choose).
- Propose features with the feature-request form.
- Improve docs (`README.md`, `PRODUCT.md`, `AGENT.md`, `INTENT.md`).
- Submit pull requests for open roadmap items.

## Development setup

### Prerequisites

- [Rust](https://rustup.rs/) (1.94+ preferred)
- Xcode command line tools (the app renders with Metal on macOS)
- The [pi coding agent CLI](https://github.com/earendil-works/pi) installed and
  authenticated — Orbit drives pi as a child process, so live tests and the app
  need it. Tests that require `pi` skip cleanly when it is absent.

### Run

```bash
git clone https://github.com/imrj05/orbit.git
cd orbit
cargo run -p orbit-pi
```

### Verify before opening a PR

```bash
cargo build --workspace      # must stay clean: zero warnings
cargo test --workspace       # unit tests + live pi integration tests
cargo clippy --workspace --all-targets
```

`cargo build` with zero warnings is a stated project invariant. CI runs build
and clippy on macOS (`.github/workflows/ci.yml`); tests are run locally, not in
CI.

For UI changes, launch the app (`cargo run -p orbit-pi`) and confirm the
affected surface works. Notifications need an app bundle — use
`scripts/run-bundled.sh`. For a rebuild-and-relaunch loop on save,
`cargo install bacon && bacon run`.

## Project invariants

These are non-negotiable; a PR that breaks one will be asked to change.

1. **No web in the UI.** New UI is GPUI only. Do not reintroduce React, Vite,
   Tauri, webviews, a DOM, Tailwind, or Node tooling.
2. **The pi CLI is the only agent runtime.** Speak its JSONL RPC over stdio
   (`crates/orbit-rpc`). There is no Node daemon.
3. **GPUI is pinned.** `gpui = "0.2.2"`. Upgrade deliberately, never track `main`.
4. **Performance is a product requirement.** Virtualize large lists and never
   block a frame with I/O.
5. **Trust pi's truth.** Render real RPC events and on-disk state; never fake a
   control that does not work.
6. **No `unsafe` without a comment; no new dependency without a stated reason.**

Full detail lives in `AGENT.md` → *Hard rules*.

## Commit and branch conventions

The history follows [Conventional Commits](https://www.conventionalcommits.org/):
`type(scope): summary` in the imperative mood.

```
feat(access-guard): prompt inline with allowlistable tool approvals
fix(transcript): keep tail-follow after remeasure
docs: add contributing guide
chore: update 10 files
```

Common types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `chore`.
Use the module name as the scope where it helps (`transcript`, `review`,
`access-guard`, `git-panel`, …).

Branch names are short and descriptive:

```
feat/parallel-sessions
fix/sidebar-watcher
docs/open-source-files
```

When a pull request from an outside contributor is merged into `main`, the
maintainer credits them by adding a bullet to the `### Contributors` list under
`[Unreleased]` in `CHANGELOG.md`:

```
python3 scripts/changelog-contributors.py \
  --pr 63 --login bougieL --title "fix(macos): reopen the window from the Dock"
```

The script is idempotent (a PR already credited anywhere in the changelog is
left alone), and `scripts/bump-version.sh` rolls the list into the release
section, so the credit ships with the release notes.

## Pull requests

1. Fork the repository and branch from `main`.
2. Keep the change focused; unrelated cleanup belongs in its own PR.
3. Fill in the pull-request template checklist.
4. Make sure build, tests, and clippy pass locally.
5. Update the docs in the same PR: `INTENT.md` → *Implementation status* (and a `D#`
   decision if the architecture changed), the `README.md` features/roadmap, and an
   `[Unreleased]` `CHANGELOG.md` entry. A shipped feature is not done until the docs
   describe it.
6. Push and open the PR. Link the issue it addresses.

A maintainer will review. Expect comments on architecture fit, performance, and
the invariants above.

## Releasing

The version lives in `crates/orbit-pi/Cargo.toml` and is the single source of
truth. Changing it on `main` is all it takes: CI tags the version and builds
the release. Notes come from `CHANGELOG.md`.

1. Add what changed under `## [Unreleased]` in `CHANGELOG.md` (Keep a Changelog
   sections: Added / Changed / Fixed). `scripts/bump-version.sh` **refuses to
   bump when `[Unreleased]` has no changes**. For a first draft from the commits
   since the last tag, run `scripts/changelog-build.py --write`, then edit the
   bullets into prose; `--force` replaces notes that are already there. When
   `[Unreleased]` is intentionally left empty, override the guard to fall back
   to the generated log:

   ```bash
   ALLOW_EMPTY_CHANGELOG=1 ./scripts/bump-version.sh 0.0.2
   ```

   With the override (or any release whose `[Unreleased]` stayed empty), the
   notes are synthesized in the Next.js release-log style — `### Core Changes` /
   `### Documentation Changes` / `### Misc Changes`, bullets ending `: #<PR>`,
   and a `### Credits` line — from the commits since the previous tag
   (`scripts/release-notes.py --from-git`). A real `[Unreleased]` entry is still
   preferred.
2. Bump and push:

   ```bash
   ./scripts/bump-version.sh 0.0.2
   ```

   That sets the version in both crates and the bundled pi extensions, refreshes
   `Cargo.lock`, rolls `[Unreleased]` under a new `[0.0.2]` heading, then commits
   `chore(release): v0.0.2` and pushes to `main`. `NO_PUSH=1` stops before the
   push. Editing the version in `crates/orbit-pi/Cargo.toml` and pushing by hand
   works the same way.

3. `.github/workflows/release.yml` sees the new version on `main` and builds
   every platform. Tags are never hand-cut: once the builds finish, the
   workflow creates the annotated tag `v0.0.2` at the commit it built, so every
   tag points at a commit whose artifacts exist. Running the workflow by hand
   (Actions → Release → Run workflow) rebuilds a leftover draft; a published
   release is never rebuilt.

4. The workflow then:

   - resolves the version from `crates/orbit-pi/Cargo.toml` (the source of truth);
   - **macOS** — builds the arm64 and universal `.app`s, signs them with your
     Developer ID, notarizes and staples each `.app` and DMG, and emits an
     updater `.tar.gz` for each (`scripts/make-dmg.sh`);
   - **Linux** — builds `orbit-pi` and packages a `.tar.gz` plus a native `.deb`
     (`scripts/bundle-linux.sh`);
   - **Windows** — builds `orbit-pi.exe` and ships it both bare and in a `.zip`
     (`scripts/bundle-windows.ps1`);
   - **drafts** a GitHub Release on tag `v0.0.2` whose notes are the `[0.0.2]`
     section of `CHANGELOG.md` (`scripts/release-notes.py`; an empty section
     falls back to the Next.js-style release log built from commits since the
     previous tag). The same notes ship as an `Orbit-Pi-0.0.2.md` release asset.

   Review the draft and publish it. macOS signing needs the Apple secrets listed
   at the top of the workflow file; without them the macOS job builds unsigned
   DMGs, and if Apple refuses the notary credentials it falls back to signed but
   un-notarized DMGs with a warning. Windows and Linux are `continue-on-error`
   while those platforms are validated.

5. The same job signs each artifact and attaches the update feeds to the
   release itself (`scripts/appcast.py`), so a published release serves them at
   `https://github.com/imrj05/orbit/releases/latest/download/appcast-<os>-<arch>.xml`.
   Nothing is committed back to `main`.

Assets and the bundled pi extensions are compiled into the binary
(`include_dir!` / `include_str!`), so every artifact is self-contained.

### In-app updates

The updater verifies every downloaded artifact against an Ed25519 public key
compiled into the binary (`ORBIT_UPDATE_PUBLIC_KEY`). Generate the keypair once:

```bash
mkdir -p "$HOME/.config/orbit"
openssl genpkey -algorithm ed25519 -out "$HOME/.config/orbit/orbit-update.pem"

# Public key → repository secret ORBIT_UPDATE_PUBLIC_KEY
openssl pkey -in "$HOME/.config/orbit/orbit-update.pem" -pubout -outform DER | tail -c 32 | base64

# Private key → repository secret ORBIT_UPDATE_PRIVATE_KEY
base64 -i "$HOME/.config/orbit/orbit-update.pem"
```

Add both under Settings → Secrets and variables → Actions, and keep that file
somewhere safe — lose it and you can never sign another update. With no public
key the updater stays dormant, and debug builds never update themselves.

Never place the private key inside the repository tree: it is gitignored, but
`git add -f` and backup/sync tools do not respect that. CI fails on any tracked
`*.pem`/`*.key`, and the key belongs in `~/.config/orbit/` (mode `0600`).

To exercise the flow from a dev build, compile the public key in and force the
updater on:

```bash
ORBIT_UPDATE_PUBLIC_KEY="$(openssl pkey -in "$HOME/.config/orbit/orbit-update.pem" -pubout -outform DER | tail -c 32 | base64)" \
ORBIT_FORCE_UPDATER=1 cargo run
```

A run outside a managed install checks and opens the modal (search, changelog,
Version History) but offers no **Update now** — installing needs the bundled
`.app`. **Check for Updates** on a build with no updater at all opens the same
modal with an explanation, never a toast.

Each feed's `<enclosure>` must point at an artifact the updater can install —
a Sparkle appcast cannot describe an architecture, which is why there is one
feed per target:

| Platform | Feed points at |
|---|---|
| macOS | `Orbit-Pi-<version>-universal.tar.gz`, one top-level `Orbit Pi.app` |
| Linux | `orbit-pi-<version>-<triple>.tar.gz`, containing `bin/orbit-pi` |
| Windows | a `*-Setup.exe` installer — **not built yet**; the current zip cannot be installed in place |

## Licensing of contributions

Orbit is licensed under the [Apache License 2.0](LICENSE). Unless you state
otherwise, any contribution you submit is licensed under the same terms, with
no additional conditions (Apache-2.0, section 5).

## Reporting security issues

Do **not** open a public issue for a security vulnerability. See
[`SECURITY.md`](SECURITY.md) for how to report it privately.
