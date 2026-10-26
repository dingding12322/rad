#!/usr/bin/env python3
"""Pull a single version's notes out of a Keep-a-Changelog CHANGELOG.md.

Usage:
    python3 scripts/release-notes.py CHANGELOG.md 0.0.1
    python3 scripts/release-notes.py CHANGELOG.md 0.0.1 --from-git

Prints the section body (without its heading) to stdout and exits 0, or exits
1 when the changelog has no section for that version. Ported from the reference
Waku app's `scripts/changelog.ts`.

`--from-git` adds a fallback in the style of the Next.js release log: when the
version has no (or an empty) section in the changelog, the notes are built from
the commits between the previous tag and this version's tag, grouped into
`### Core Changes` / `### Documentation Changes` / `### Misc Changes`, each
bullet keeping its raw title with a trailing `: #<PR>`, and a `### Credits`
line ("Huge thanks to @a, @b, and @c for helping!"). The changelog is still
preferred whenever it has real content.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys

# Next.js-style sections, in emit order.
SECTIONS = ["Core Changes", "Documentation Changes", "Misc Changes"]

# A commit touching the app is a Core change; docs/marketing is Documentation;
# anything else (scripts, CI, contrib) is Misc. First match wins, top to bottom.
CORE_PREFIXES = ("crates/",)
DOC_PREFIXES = ("docs/", "marketing/")
DOC_SUFFIXES = (".md",)

# Release bumps and the changelog-credit bot are not user-facing notes.
SKIP_SUBJECT = re.compile(r"^(?:chore\(release\)|docs\(changelog\))", re.IGNORECASE)

# A trailing "(#123)" becomes ": #123", the way Next.js writes PR references.
TRAILING_PR = re.compile(r"\s*\(#(\d+)\)\s*$")

MENTION = re.compile(r"@[a-z\d](?:[a-z\d-]{0,38})", re.IGNORECASE)


def heading_version(line: str):
    """The version token from a level-2 heading, or None.

    Handles `## [0.0.1] - 2026-09-14`, `## 0.0.1`, `## v0.0.1`, etc.
    """
    match = re.match(r"^##\s+(.+)$", line)  # level 2 only
    if not match:
        return None
    token = match.group(1).strip().split()[0]  # before any " - date"
    token = token.strip("[]")  # strip [ ]
    return token[1:] if token[:1] in ("v", "V") else token  # strip leading v


def extract_release_notes(changelog: str, version: str):
    lines = changelog.split("\n")

    start = None
    for i, line in enumerate(lines):
        if heading_version(line) == version:
            start = i + 1
            break
    if start is None:
        return None

    end = len(lines)
    for i in range(start, len(lines)):
        if re.match(r"^##\s+", lines[i]):
            end = i
            break

    body = "\n".join(lines[start:end]).strip()
    return body or None


def _run_git(args):
    try:
        return subprocess.run(
            ["git", *args],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def _previous_tag(ref: str):
    """The most recent tag reachable from `ref`'s parent, or None."""
    out = _run_git(["describe", "--tags", "--abbrev=0", f"{ref}^"])
    return out.strip() if out else None


def _tag_exists(tag: str) -> bool:
    return _run_git(["rev-parse", "--verify", "--quiet", f"refs/tags/{tag}"]) is not None


def _commit_entries(range_spec: str):
    """`[(sha, subject, [paths]), ...]` for the range, merge commits excluded."""
    out = _run_git(
        [
            "log",
            "--no-merges",
            "--pretty=format:__C__%x1f%H%x1f%s",
            "--name-only",
            range_spec,
        ]
    )
    if out is None:
        return []
    commits: list[tuple[str, str, list[str]]] = []
    for line in out.split("\n"):
        if line.startswith("__C__\x1f"):
            _, sha, subject = line.split("\x1f", 2)
            commits.append((sha, subject, []))
        elif line.strip() and commits:
            commits[-1][2].append(line.strip())
    return commits


def _section_for(paths) -> str:
    if any(path.startswith(CORE_PREFIXES) for path in paths):
        return "Core Changes"
    if any(path.startswith(DOC_PREFIXES) or path.endswith(DOC_SUFFIXES) for path in paths):
        return "Documentation Changes"
    return "Misc Changes"


def _bullet(subject: str) -> str:
    match = TRAILING_PR.search(subject)
    if match:
        return f"{subject[: match.start()].rstrip()}: #{match.group(1)}"
    return subject


def _github_handles(previous, ref) -> list[str]:
    """Commit authors' GitHub logins for the range, via `gh` when available.

    Best-effort: no `gh`, no auth, or a failed call simply yields no credits.
    """
    if not previous or not shutil.which("gh"):
        return []
    # The compare API needs a real revision, not `HEAD`; resolve to a commit
    # SHA (`^{commit}` peels an annotated tag, which `rev-parse` alone does not).
    head = (_run_git(["rev-parse", f"{ref}^{{commit}}"]) or "").strip() or ref
    slug = os.environ.get("GITHUB_REPOSITORY", "").strip()
    if not slug:
        remote = _run_git(["remote", "get-url", "origin"]) or ""
        match = re.search(r"github\.com[:/]([^/]+/[^/.]+)", remote)
        slug = match.group(1) if match else ""
    if not slug:
        return []
    try:
        out = subprocess.run(
            [
                "gh",
                "api",
                f"repos/{slug}/compare/{previous}...{head}",
                "--jq",
                ".commits[].author.login",
            ],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        return []
    return [line.strip() for line in out.splitlines() if line.strip()]


def _credits_line(handles: list[str]):
    # Dedupe while keeping first-seen order, and always show the `@`.
    unique = [f"@{handle.lstrip('@')}" for handle in dict.fromkeys(handles)]
    if not unique:
        return None
    if len(unique) == 1:
        return f"Huge thanks to {unique[0]} for helping!"
    return f"Huge thanks to {', '.join(unique[:-1])}, and {unique[-1]} for helping!"


def generate_from_git(version: str):
    """Next.js-style notes built from the commits since the previous tag.

    The range is `<previous-tag>..<v version>`; the version tag is used when it
    exists, otherwise HEAD (the workflow tags the exact commit it built).
    """
    tag = f"v{version}"
    ref = tag if _tag_exists(tag) else "HEAD"
    previous = _previous_tag(ref)
    range_spec = f"{previous}..{ref}" if previous else ref

    commits = _commit_entries(range_spec)
    if not commits:
        return None

    sections: dict[str, list[str]] = {name: [] for name in SECTIONS}
    handles: list[str] = []
    for _, subject, paths in commits:
        if SKIP_SUBJECT.match(subject):
            continue
        handles.extend(MENTION.findall(subject))
        sections[_section_for(paths)].append(_bullet(subject))

    handles.extend(_github_handles(previous, ref))

    body: list[str] = []
    for name in SECTIONS:
        if not sections[name]:
            continue
        body.append(f"### {name}")
        body.append("")
        body.extend(f"- {bullet}" for bullet in sections[name])
        body.append("")

    credits = _credits_line(handles)
    if credits:
        body.append("### Credits")
        body.append("")
        body.append(credits)
        body.append("")

    return "\n".join(body).strip() or None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("changelog", help="path to CHANGELOG.md")
    ap.add_argument("version", help="version token, e.g. 0.0.2")
    ap.add_argument(
        "--from-git",
        action="store_true",
        help="fall back to commits when the changelog section is empty",
    )
    args = ap.parse_args()

    try:
        with open(args.changelog, encoding="utf-8") as fh:
            changelog = fh.read()
    except FileNotFoundError:
        changelog = ""

    notes = extract_release_notes(changelog, args.version)
    if not notes and args.from_git:
        notes = generate_from_git(args.version)
    if not notes:
        return 1
    print(notes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
