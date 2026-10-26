#!/usr/bin/env python3
"""Generate `crates/orbit-pi/locales/<locale>.yml` from en.yml + the glossary.

`en.yml` is the single source of truth for keys and English copy. The
glossary maps each English string to a translation per locale; anything not
yet translated falls back to English, matching rust-i18n's runtime fallback.

Run after editing en.yml or the glossary:

    python3 scripts/gen_locales.py
"""
import glob
import io
import importlib
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

# Glossary modules are split by surface (core, settings, ...) so each file
# stays reviewable. Later modules win on a key conflict.
GLOSSARY = {}
for module_path in sorted(glob.glob(os.path.join(HERE, "i18n_glossary*.py"))):
    module = importlib.import_module(os.path.basename(module_path)[:-3])
    for locale, table in getattr(module, "GLOSSARY", {}).items():
        GLOSSARY.setdefault(locale, {}).update(table)

LOCALES = os.path.join(HERE, "..", "crates", "orbit-pi", "locales")
EN = os.path.join(LOCALES, "en.yml")


def parse(path):
    """Parse the flat `key: "value"` subset of YAML we emit."""
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


def quote(value):
    return '"%s"' % value.replace("\\", "\\\\").replace('"', '\\"')


def main():
    en = parse(EN)
    english = {v for _, v in en}
    problems = 0
    for locale, table in GLOSSARY.items():
        missing = sorted(english - set(table))
        if missing:
            problems += len(missing)
            print(f"[{locale}] {len(missing)} untranslated string(s):", file=sys.stderr)
            for m in missing[:10]:
                print(f"    {m!r}", file=sys.stderr)
        path = os.path.join(LOCALES, f"{locale}.yml")
        with io.open(path, "w", encoding="utf-8") as f:
            f.write("_version: 1\n")
            for key, value in en:
                f.write(f"{key}: {quote(table.get(value, value))}\n")
        print(f"wrote {locale}.yml ({len(en)} keys, {len(missing)} fallback)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
