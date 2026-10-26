#!/usr/bin/env python3
"""Localize the `setting_row` / `settings_section` label arguments.

These helpers take `&str` labels and `Option<&str>` descriptions, so the
literals are arguments rather than `.child(...)` calls. A small balanced-paren
scanner splits each call's top-level arguments and rewrites only the ones that
are exactly a string literal (the title) or `Some("literal")` (the
description/meta), leaving nested builder calls untouched.
"""
import os
import re
import sys

ROOT = "crates/orbit-pi/src"
CALLS = ("setting_row", "settings_section")
LITERAL = re.compile(r'^"((?:[^"\\]|\\.)*)"(\.to_string\(\))?$', re.S)
SOME = re.compile(r'^Some\(\s*"((?:[^"\\]|\\.)*)"\s*(\.to_string\(\))?\s*,?\s*\)$', re.S)


def unescape(raw):
    return (
        raw.replace('\\"', '"')
        .replace("\\n", "\n")
        .replace("\\\\", "\\")
    )


def slugify(text):
    text = text.lower().replace("…", "").replace("—", " ").replace("&", " and ")
    text = re.sub(r"[^a-z0-9]+", "_", text)
    return re.sub(r"_+", "_", text).strip("_")[:48] or "text"


def find_call(src, name, start):
    idx = src.find(name + "(", start)
    if idx == -1:
        return None
    i = idx + len(name) + 1
    depth = 1
    while i < len(src) and depth:
        c = src[i]
        if c == '"':
            i += 1
            while i < len(src) and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return idx, i
        i += 1
    return None


def split_args(body):
    """Yield (start, end) ranges of each top-level comma-separated argument."""
    spans = []
    depth = 0
    start = 0
    i = 0
    while i < len(body):
        c = body[i]
        if c == '"':
            i += 1
            while i < len(body) and body[i] != '"':
                i += 2 if body[i] == "\\" else 1
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            spans.append((start, i))
            start = i + 1
        i += 1
    spans.append((start, len(body)))
    return spans


def main(apply):
    keys = {}
    files = []
    for dp, _, fs in os.walk(ROOT):
        for f in fs:
            if f.endswith(".rs") and not f.endswith("_tests.rs"):
                files.append(os.path.join(dp, f))
    for path in files:
        src = open(path, encoding="utf-8").read()
        cut = len(src)
        for marker in ("\n#[cfg(test)]", "\nmod tests {"):
            ix = src.find(marker)
            if ix != -1:
                cut = min(cut, ix)
        head = src[:cut]
        surface = os.path.basename(path)[:-3]

        def key_for(english):
            base = f"{surface}.{slugify(english)}"
            k = base
            n = 2
            while k in keys and keys[k] != english:
                k = f"{base}_{n}"
                n += 1
            keys[k] = english
            return k

        changed = True
        cursor = 0
        out = ""
        while changed:
            found = None
            for name in CALLS:
                r = find_call(head, name, cursor)
                if r and (found is None or r[0] < found[0]):
                    found = r
            if not found:
                break
            start, end = found
            body = head[start + len(next(n for n in CALLS if head.startswith(n + "(", start))) + 1 : end]
            # Rewrite top-level args in place within `body`.
            pieces = []
            last = 0
            for a, b in split_args(body):
                arg = body[a:b]
                stripped = arg.strip()
                new = None
                m = LITERAL.match(stripped)
                if m and m.group(1):
                    new = f'&tr!("{key_for(unescape(m.group(1)))}")'
                else:
                    m = SOME.match(stripped)
                    if m and m.group(1):
                        new = f'Some(&tr!("{key_for(unescape(m.group(1)))}"))'
                if new is None:
                    continue
                lead = arg[: len(arg) - len(arg.lstrip())]
                trail = arg[len(arg.rstrip()) :]
                pieces.append((a, b, lead + new + trail))
            if pieces:
                nb = body
                for a, b, rep in reversed(pieces):
                    nb = nb[:a] + rep + nb[b:]
                head = head[: start + len([n for n in CALLS if head.startswith(n + "(", start)][0]) + 1] + nb + head[end:]
                cursor = start + 1
            else:
                cursor = start + 1
        if apply and head != src[:cut]:
            open(path, "w", encoding="utf-8").write(head + src[cut:])

    for k in sorted(keys):
        print(f"{k}\t{keys[k]}")


if __name__ == "__main__":
    main("--apply" in sys.argv)
