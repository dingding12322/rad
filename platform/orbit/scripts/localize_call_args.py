#!/usr/bin/env python3
"""Wrap every string literal inside one argument of a builder call.

For call shapes where the label is the first expression of a builder — e.g.
`PaletteItem::command("New Session", ...)` or the `if visible { "Hide" } else {
"Show" }` variant — rewriting all literals in that argument keeps the diff
small and the key list complete.

Usage: localize_call_args.py <file> <call_name> <arg_index>
"""
import os
import re
import sys

LITERAL = re.compile(r'"((?:[^"\\]|\\.)*)"')


def unescape(raw):
    return raw.replace('\\"', '"').replace("\\n", "\n").replace("\\\\", "\\")


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
                return idx, idx + len(name) + 1, i
        i += 1
    return None


def split_args(body):
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


def main(path, name, arg_index, apply):
    src = open(path, encoding="utf-8").read()
    cut = len(src)
    for marker in ("\n#[cfg(test)]", "\nmod tests {"):
        ix = src.find(marker)
        if ix != -1:
            cut = min(cut, ix)
    head, tail = src[:cut], src[cut:]
    surface = os.path.basename(path)[:-3]
    keys = {}

    def key_for(english):
        base = f"{surface}.{slugify(english)}"
        k = base
        n = 2
        while k in keys and keys[k] != english:
            k = f"{base}_{n}"
            n += 1
        keys[k] = english
        return k

    cursor = 0
    while True:
        found = find_call(head, name, cursor)
        if not found:
            break
        start, body_start, end = found
        body = head[body_start:end]
        spans = split_args(body)
        a, b = spans[arg_index]
        arg = body[a:b]
        new_arg = arg
        for m in reversed(list(LITERAL.finditer(arg))):
            if m.group(1):
                new_arg = (
                    new_arg[: m.start()]
                    + f'tr!("{key_for(unescape(m.group(1)))}")'
                    + new_arg[m.end() :]
                )
        head = head[:body_start] + body[:a] + new_arg + body[b:] + head[end:]
        cursor = start + 1

    if apply:
        open(path, "w", encoding="utf-8").write(head + tail)
    for k in sorted(keys):
        print(f"{k}\t{keys[k]}")


if __name__ == "__main__":
    apply = "--apply" in sys.argv
    args = [a for a in sys.argv[1:] if a != "--apply"]
    main(args[0], args[1], int(args[2]), apply)
