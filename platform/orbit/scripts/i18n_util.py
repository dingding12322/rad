#!/usr/bin/env python3
"""Helpers for the flat `key: "value"` locale files.

Used by the little append/normalize scripts so en.yml is never double-quoted.
"""
import io


def read(path):
    """Return an ordered list of (key, unescaped value)."""
    out = []
    for line in io.open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line or line.startswith("_version"):
            continue
        key, _, raw = line.partition(": ")
        raw = raw.strip()
        if raw.startswith('"') and raw.endswith('"'):
            raw = raw[1:-1].replace('\\"', '"').replace("\\\\", "\\")
        out.append((key, raw))
    return out


def write(path, items):
    with io.open(path, "w", encoding="utf-8") as f:
        f.write("_version: 1\n")
        for key, value in items:
            f.write('%s: "%s"\n' % (key, value.replace("\\", "\\\\").replace('"', '\\"')))
