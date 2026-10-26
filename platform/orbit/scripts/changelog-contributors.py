#!/usr/bin/env python3
"""Credit a merged contributor under [Unreleased] in CHANGELOG.md.

Usage:
    python3 scripts/changelog-contributors.py \
        --pr 11 --login dimasalmaz --title "Fix transcript scrolling"

Appends one bullet to the `### Contributors` list inside the `[Unreleased]`
section, creating that heading if it is missing. The next
`scripts/bump-version.sh x.y.z` rolls `[Unreleased]` into the new version's
section, so the credit travels into the GitHub Release notes and the in-app
update modal with no extra step.

Idempotent: a pull request that already appears anywhere in the changelog is
left untouched, so a re-run (or a backfilled manual entry) is a no-op.
"""

import argparse
import re
import sys
import textwrap

UNRELEASED = "## [Unreleased]"
CONTRIBUTORS = "### Contributors"


def section_bounds(lines, heading):
    """Line range [start, end) of a `##` section body's heading line."""
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
    ap.add_argument("--pr", required=True, type=int, help="merged pull-request number")
    ap.add_argument("--login", required=True, help="contributor's GitHub login")
    ap.add_argument("--title", required=True, help="pull-request title")
    ap.add_argument("--repo", default="imrj05/orbit", help="owner/name for links")
    ap.add_argument("--changelog", default="CHANGELOG.md", help="path to the changelog")
    args = ap.parse_args()

    with open(args.changelog, encoding="utf-8") as fh:
        text = fh.read()
    lines = text.split("\n")

    if f"/pull/{args.pr}" in text:
        print(f"#{args.pr} is already credited; nothing to do")
        return 0

    start, end = section_bounds(lines, UNRELEASED)
    if start is None:
        print(f"{args.changelog} has no '{UNRELEASED}' heading", file=sys.stderr)
        return 1

    bullet = (
        f"- **[@{args.login}](https://github.com/{args.login})** "
        f"([#{args.pr}](https://github.com/{args.repo}/pull/{args.pr})) — {args.title}"
    )
    wrapped = textwrap.wrap(
        bullet,
        width=80,
        subsequent_indent="  ",
        break_long_words=False,
        break_on_hyphens=False,
    )

    sub = next(
        (i for i in range(start + 1, end) if lines[i].strip() == CONTRIBUTORS), None
    )
    if sub is None:
        # Append a fresh subsection at the end of [Unreleased], after any
        # Added/Changed/Fixed prose.
        at = end
        while at - 1 > start and lines[at - 1].strip() == "":
            at -= 1
        lines[at:at] = ["", CONTRIBUTORS, ""] + wrapped + [""]
    else:
        # Append to the existing list, before the next subsection or the end.
        at = next((i for i in range(sub + 1, end) if lines[i].startswith("### ")), end)
        while at - 1 > sub and lines[at - 1].strip() == "":
            at -= 1
        lines[at:at] = wrapped + [""]

    # One blank line between blocks, whatever the source spacing was.
    out = re.sub(r"\n{3,}", "\n\n", "\n".join(lines))
    with open(args.changelog, "w", encoding="utf-8") as fh:
        fh.write(out)
    print(f"credited @{args.login} for #{args.pr} under [Unreleased]")
    return 0


if __name__ == "__main__":
    sys.exit(main())
