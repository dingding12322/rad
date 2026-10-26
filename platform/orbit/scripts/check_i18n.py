#!/usr/bin/env python3
"""Fail if a `tr!(...)` in Orbit is misused.

Two checks:

1. Every `tr!("key")` used in the source has an entry in `locales/en.yml`.
   rust-i18n would otherwise render the raw key.
2. Every `tr!("key", name = ...)` binds exactly the placeholders the key
   declares (`%{name}`). An unbound placeholder stays literal in the output,
   e.g. `Show all %{count} lines` when the call binds `total` for a `%{count}`
   key (`total` vs `count`), or `Branch create failed: %{err}` when it binds
   `error` for `%{err}`.

Run from the repo root:

    python3 scripts/check_i18n.py

Exit status is 1 when a key is missing or a placeholder does not match, so CI
can gate on it.
"""
import io
import os
import re
import sys

ROOT = "crates/orbit-pi/src"
EN = "crates/orbit-pi/locales/en.yml"
# `tr!` but not `include_str!`, `write_str!`, etc.: the leading character must
# not be an identifier byte.
KEY = re.compile(r'(?<![A-Za-z0-9_])tr!\(\s*"((?:[^"\\]|\\.)*)"')
# `tr!` / `tr_cow!` with a literal key, matched against the whole file so
# multi-line calls are seen.
CALL = re.compile(r'(?<![A-Za-z0-9_])tr(?:_cow)?!\(\s*"((?:[^"\\]|\\.)*)"')
PLACEHOLDER = re.compile(r"%\{(\w+)\}")
# A binding is `name =` not followed by `>` (so `=>` match arms don't count).
BINDING = re.compile(r"([a-z_][a-z0-9_]*)\s*=(?!>)")


def declared_placeholders():
    """key -> the set of `%{name}` placeholders its en.yml value declares."""
    result = {}
    for line in io.open(EN, encoding="utf-8"):
        match = re.match(r'^([\w.]+):\s*"(.*)"\s*$', line.rstrip("\n"))
        if match:
            result[match.group(1)] = set(PLACEHOLDER.findall(match.group(2)))
    return result


def bound_names(src, call_start):
    """The `name =` bindings of one call, at the call's own paren depth.

    Nested calls (a `tr!` inside another `tr!`'s argument) are skipped, so
    only the bindings that belong to this key are returned.
    """
    start = src.index("(", call_start)
    depth = 0
    end = start
    while end < len(src):
        char = src[end]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
            if depth == 0:
                break
        end += 1
    body = src[start + 1 : end]
    names = set()
    depth = 0
    i = 0
    while i < len(body):
        char = body[i]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        elif depth == 0:
            match = BINDING.match(body, i)
            if match:
                names.add(match.group(1))
                i = match.end()
                continue
        i += 1
    return names


def main():
    used = {}
    for dirpath, _, files in os.walk(ROOT):
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(dirpath, name)
            for line_no, line in enumerate(io.open(path, encoding="utf-8"), 1):
                for match in KEY.finditer(line):
                    used.setdefault(match.group(1), []).append(f"{path}:{line_no}")

    defined = set()
    for line in io.open(EN, encoding="utf-8"):
        line = line.rstrip("\n")
        if line and not line.startswith("_version"):
            defined.add(line.partition(": ")[0])

    missing = sorted(key for key in used if key not in defined)
    unused = sorted(key for key in defined if key not in used)
    for key in missing:
        print(f"MISSING en.yml key: {key}  (used at {used[key][0]})")
    if unused:
        print(f"note: {len(unused)} en.yml key(s) not referenced in source (ok if built dynamically)")

    # Placeholder pass: a call's bindings must match its key's placeholders.
    declared = declared_placeholders()
    mismatches = []
    for dirpath, _, files in os.walk(ROOT):
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(dirpath, name)
            src = io.open(path, encoding="utf-8").read()
            for match in CALL.finditer(src):
                key = match.group(1)
                wants = declared.get(key)
                if not wants:
                    continue
                got = bound_names(src, match.start())
                if got != wants:
                    line_no = src.count("\n", 0, match.start()) + 1
                    mismatches.append((path, line_no, key, got, wants))
    for path, line_no, key, got, wants in mismatches:
        print(
            f"PLACEHOLDER MISMATCH {path}:{line_no}  {key}: "
            f"binds {sorted(got) or 'nothing'}, key declares {sorted(wants)}"
        )

    print(
        f"{len(used)} keys used, {len(defined)} defined, {len(missing)} missing, "
        f"{len(mismatches)} placeholder mismatch(es)"
    )
    return 1 if missing or mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
