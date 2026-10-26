#!/usr/bin/env python3
"""Build the `[Unreleased]` section of CHANGELOG.md from git history.

Usage:
    python3 scripts/changelog-build.py                  # print a draft
    python3 scripts/changelog-build.py --write           # fill an empty [Unreleased]
    python3 scripts/changelog-build.py --write --force   # replace existing notes

`scripts/bump-version.sh` refuses to release when `[Unreleased]` has no
changes, so this is the fast path to a first draft: it groups the subjects of
the commits since the previous release tag into Keep a Changelog sections.

    feat                       -> ### Added
    fix                        -> ### Fixed
    perf / refactor / style /  -> ### Changed
    revert
    docs / chore / ci / build /-> skipped (not user-facing)
    test

Subjects that do not follow Conventional Commits land under `### Changed`
with their original wording, so nothing is lost. A trailing pull-request
reference is kept (`Refactor the panel (#22)`).

`--write` only touches an empty `[Unreleased]`; pass `--force` to overwrite
hand-written notes.
"""

import argparse
import re
import subprocess
import sys

UNRELEASED = "## [Unreleased]"

# Keep a Changelog order; first match wins.
SECTIONS = (
    ("Added", ("feat",)),
    ("Fixed", ("fix",)),
    ("Changed", ("perf", "refactor", "style", "revert")),
)
SKIP_TYPES = ("docs", "chore", "ci", "build", "test")

CONVENTIONAL = re.compile(r"^(?P<type>[a-z]+)(?:\([^)]*\))?!?:\s*(?P<desc>.+)$", re.I)
TRAILING_PR = re.compile(r"\s*\(#(\d+)\)\s*$")
# Release bumps and the changelog-credit bot are not user-facing notes.
SKIP_SUBJECT = re.compile(r"^(?:chore\(release\)|docs\(changelog\))", re.I)


def _git(args):
    try:
        return subprocess.run(
            ["git", *args], capture_output=True, text=True, check=True
        ).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def _previous_tag(ref):
    """The most recent tag reachable from `ref`'s parent, or None."""
    out = _git(["describe", "--tags", "--abbrev=0", f"{ref}^"])
    return out.strip() if out else None


def _commit_subjects(range_spec):
    out = _git(["log", "--no-merges", "--pretty=format:%s", range_spec])
    return [line for line in (out or "").split("\n") if line.strip()]


def _section_for(subject):
    match = CONVENTIONAL.match(subject)
    if not match:
        return "Changed", subject
    kind = match.group("type").lower()
    if kind in SKIP_TYPES:
        return None, None
    for name, kinds in SECTIONS:
        if kind in kinds:
            return name, match.group("desc").strip()
    return "Changed", match.group("desc").strip()


def _bullet(desc):
    desc = desc[:1].upper() + desc[1:]
    match = TRAILING_PR.search(desc)
    if match:
        return f"{desc[: match.start()].rstrip()} (#{match.group(1)})"
    return desc


def build_notes(from_ref, to_ref):
    previous = _previous_tag(to_ref)
    if from_ref:
        range_spec = f"{from_ref}..{to_ref}"
    elif previous:
        range_spec = f"{previous}..{to_ref}"
    else:
        range_spec = to_ref

    sections = {name: [] for name, _ in SECTIONS}
    for subject in _commit_subjects(range_spec):
        if SKIP_SUBJECT.match(subject):
            continue
        name, desc = _section_for(subject)
        if name:
            sections[name].append(_bullet(desc))

    body = []
    for name, _ in SECTIONS:
        if not sections[name]:
            continue
        body.append(f"### {name}")
        body.append("")
        body.extend(f"- {bullet}" for bullet in sections[name])
        body.append("")
    return "\n".join(body).strip() or None


def section_bounds(lines, heading):
    """Line range [start, end) of a `##` section body, or (None, None)."""
    start = next((i for i, l in enumerate(lines) if l.strip() == heading), None)
    if start is None:
        return None, None
    end = next(
        (i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")),
        len(lines),
    )
    return start, end


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--from", dest="from_ref", help="start ref (default: previous tag)")
    ap.add_argument("--to", default="HEAD", help="end ref (default: HEAD)")
    ap.add_argument("--write", action="store_true", help="write into CHANGELOG.md")
    ap.add_argument("--force", action="store_true", help="overwrite existing notes")
    ap.add_argument("--changelog", default="CHANGELOG.md", help="path to the changelog")
    args = ap.parse_args()

    notes = build_notes(args.from_ref, args.to)
    if not notes:
        print("no user-facing commits to build notes from", file=sys.stderr)
        return 1

    if not args.write:
        print(notes)
        return 0

    with open(args.changelog, encoding="utf-8") as fh:
        lines = fh.read().split("\n")

    start, end = section_bounds(lines, UNRELEASED)
    if start is None:
        print(f"{args.changelog} has no '{UNRELEASED}' heading", file=sys.stderr)
        return 1

    if "\n".join(lines[start + 1 : end]).strip() and not args.force:
        print(
            f"'{UNRELEASED}' already has notes; pass --force to replace them",
            file=sys.stderr,
        )
        return 1

    lines[start:end] = [UNRELEASED, ""] + notes.split("\n") + [""]
    out = re.sub(r"\n{3,}", "\n\n", "\n".join(lines))
    with open(args.changelog, "w", encoding="utf-8") as fh:
        fh.write(out)
    print(f"built '{UNRELEASED}' in {args.changelog}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
