#!/usr/bin/env bash
#
# Bump the app version, roll the changelog, commit, and push (CI tags + builds).
#
# Usage:
#   ./scripts/bump-version.sh 0.0.2
#
# What it does:
#   1. sets the version in both crates + the bundled pi extensions
#   2. refreshes Cargo.lock
#   3. moves CHANGELOG.md's [Unreleased] body under a new [<version>] heading
#      (if [Unreleased] is empty, it is first auto-built from git history via
#      scripts/changelog-build.py)
#   4. commits and pushes to main
#
# The version change is what CI watches: .github/workflows/release.yml sees it,
# builds every platform, creates the annotated tag v<version> at that commit,
# and drafts a GitHub Release on it from CHANGELOG.md.
#
# Env:
#   NO_PUSH=1                 commit locally, but don't push
#   ALLOW_EMPTY_CHANGELOG=1   release even when [Unreleased] is empty
#                             (notes then come from the git release log)
#
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="${1:-}"

if [ -z "$VERSION" ]; then
  echo "usage: $0 <x.y.z>" >&2
  exit 2
fi
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "not a plain x.y.z version: $VERSION" >&2
  exit 2
fi

REPO_URL="https://github.com/imrj05/orbit"

# 1 — Cargo crate versions (the app and its RPC client).
python3 - "$VERSION" <<'PY'
import re, sys
v = sys.argv[1]
for path in ("crates/orbit-pi/Cargo.toml", "crates/orbit-rpc/Cargo.toml"):
    with open(path) as fh:
        src = fh.read()
    src, n = re.subn(r'(?m)^version = ".*"$', f'version = "{v}"', src, count=1)
    if n != 1:
        sys.exit(f"no package version line found in {path}")
    with open(path, "w") as fh:
        fh.write(src)
    print(f"set {path} -> {v}")
PY

# 2 — bundled pi extensions.
python3 - "$VERSION" <<'PY'
import json, sys
v = sys.argv[1]
for path in (
    "contrib/orbit-guard-extension/package.json",
    "contrib/orbit-quota-extension/package.json",
):
    with open(path) as fh:
        data = json.load(fh)
    data["version"] = v
    with open(path, "w") as fh:
        fh.write(json.dumps(data, indent=2) + "\n")
    print(f"set {path} -> {v}")
PY

cargo update -p orbit-pi -p orbit-rpc >/dev/null

# 3 — roll the changelog.
python3 - "$VERSION" "$REPO_URL" <<'PY'
import datetime, os, re, subprocess, sys
v, repo = sys.argv[1], sys.argv[2]
date = datetime.date.today().isoformat()
path = "CHANGELOG.md"

def unreleased(lines):
    """Locate '## [Unreleased]' and return (start, end, body, meaningful)."""
    start = next(
        (i for i, l in enumerate(lines) if l.strip() == "## [Unreleased]"), None
    )
    if start is None:
        sys.exit("CHANGELOG.md has no '## [Unreleased]' heading")
    end = next(
        (i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")),
        len(lines),
    )
    body = "\n".join(lines[start + 1 : end]).strip()
    # The credit bot writes a `### Contributors`-only section; that is not a
    # change list, so it does not count as changelog content for this guard.
    meaningful = re.sub(r"(?m)^#+\s+Contributors\s*$", "", body).strip()
    return start, end, body, meaningful

with open(path) as fh:
    lines = fh.read().split("\n")
start, end, body, meaningful = unreleased(lines)

if not meaningful:
    if os.environ.get("ALLOW_EMPTY_CHANGELOG"):
        print(
            f"warning: '[Unreleased]' has no changes; releasing anyway "
            "(ALLOW_EMPTY_CHANGELOG set) — notes will be generated from git "
            "(scripts/release-notes.py --from-git)",
            file=sys.stderr,
        )
    else:
        # Auto-build the notes from commit subjects since the last tag.
        res = subprocess.run(
            [sys.executable, "scripts/changelog-build.py", "--write", "--force"],
            capture_output=True,
            text=True,
        )
        if res.returncode != 0:
            sys.exit(
                f"error: '## [Unreleased]' in CHANGELOG.md has no changes and "
                "could not be auto-built from git history:\n"
                + (res.stderr or res.stdout).strip()
                + "\nAdd a Keep-a-Changelog entry (Added / Changed / Fixed) "
                "first, or re-run with ALLOW_EMPTY_CHANGELOG=1 to fall back "
                "to the git-generated release log."
            )
        print(f"auto-built [Unreleased] from git history ({v})")
        with open(path) as fh:
            lines = fh.read().split("\n")
        start, end, body, meaningful = unreleased(lines)

block = ["## [Unreleased]", ""]
if body:
    block += [f"## [{v}] - {date}", "", body]
else:
    block += [f"## [{v}] - {date}"]
block += [""]  # blank line before the next section
lines[start:end] = block

out = "\n".join(lines)
out = re.sub(
    r"(?m)^\[Unreleased\]: .*$",
    f"[Unreleased]: {repo}/compare/v{v}...HEAD",
    out,
)
if f"[{v}]:" not in out:
    out = out.rstrip("\n") + f"\n[{v}]: {repo}/releases/tag/v{v}\n"

with open(path, "w") as fh:
    fh.write(out)
print(f"rolled CHANGELOG.md for {v}")
PY

# 4 — commit and push. Tagging and releasing are left to CI (release.yml), so
# the version change on main is the single trigger.
git add crates/orbit-pi/Cargo.toml crates/orbit-rpc/Cargo.toml Cargo.lock \
  contrib/orbit-guard-extension/package.json \
  contrib/orbit-quota-extension/package.json CHANGELOG.md
git commit -m "chore(release): v$VERSION"
echo "Committed v$VERSION"

if [ -n "${NO_PUSH:-}" ]; then
  echo "NO_PUSH set — push manually: git push origin HEAD"
else
  git push origin HEAD
  echo "Pushed v$VERSION — CI will tag it and start the release build."
fi
