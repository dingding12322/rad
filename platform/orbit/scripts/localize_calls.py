#!/usr/bin/env python3
"""Localize direct GPUI text call sites and collect their English source.

This is intentionally conservative: it only rewrites string literals passed
*immediately* to known text-bearing element builders (`.child`, `.text`,
`.tooltip`, `.placeholder`, `.with_placeholder`, `.menu_title`, the `toast_*`
helpers, and `set_status`) in non-test code. Everything else (format strings,
`Some(..)` rows, struct fields) is handled by hand so no non-UI literal is
mistaken for copy.

It writes the touched Rust files in place and emits `key<TAB>english` lines to
stdout for the locale files.
"""
import os
import re
import sys

ROOT = "crates/orbit-pi/src"

# `.child("X")`, `.text("X")`, ... — capture the raw literal body.
CALL = r'\.(child|text|tooltip|placeholder|with_placeholder|menu_title)\s*\(\s*"((?:[^"\\]|\\.)*)"\s*(\.to_string\(\))?\s*\)'
TOAST = r'\b(toast_success|toast_info|toast_warn|toast_error)\s*\(\s*"((?:[^"\\]|\\.)*)"\s*\)'
STATUS = r'\bset_status\s*\(\s*"((?:[^"\\]|\\.)*)"\s*\)'
# `.child(format!("X"))` with no interpolation args.
FMT0 = r'\.child\(\s*format!\(\s*"((?:[^"\\]|\\.)*)"\s*\)\s*\)'

PATTERNS = [
    ("call", re.compile(CALL)),
    ("toast", re.compile(TOAST)),
    ("status", re.compile(STATUS)),
    ("fmt0", re.compile(FMT0)),
]

IDENT = re.compile(r"^[a-z0-9_\-./]+$")
SKIP_VALUES = {
    "×", "$", "A.", "…", "Orbit", "Orbit Pi", "orbit — pi",
    "a very long unwrapping label that will not shrink at all",
}


def unescape(raw: str) -> str:
    out = []
    i = 0
    while i < len(raw):
        c = raw[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        nxt = raw[i + 1] if i + 1 < len(raw) else ""
        if nxt == "n":
            out.append("\n")
            i += 2
        elif nxt == "t":
            out.append("\t")
            i += 2
        elif nxt in ('"', "\\", "'"):
            out.append(nxt)
            i += 2
        elif nxt == "u" and raw[i + 2 : i + 3] == "{":
            end = raw.index("}", i + 3)
            out.append(chr(int(raw[i + 3 : end], 16)))
            i = end + 1
        else:
            out.append(nxt)
            i += 2
    return "".join(out)


def is_copy(text: str) -> bool:
    if not text or text in SKIP_VALUES:
        return False
    if len(text) < 2 or len(text) > 200:
        return False
    if "\n" in text or "\t" in text:
        return False
    if "%{" in text:  # already a template
        return False
    if "{" in text or "}" in text:  # Rust format placeholder
        return False
    if IDENT.match(text):
        return False
    if text.endswith((".svg", ".png", ".json", ".yml", ".rs", ".ttf", ".icns", ".ico")):
        return False
    if "/" in text or "\\" in text:
        return False
    if not re.search(r"[A-Za-z]", text):
        return False
    # Copy-like: has a space, or reads as a word/phrase with punctuation.
    if " " in text:
        return True
    return bool(re.match(r"^[A-Z]", text)) and len(text) > 3


def slugify(text: str) -> str:
    text = text.lower()
    text = text.replace("…", "").replace("—", " ").replace("·", " ")
    text = re.sub(r"[^a-z0-9]+", "_", text)
    text = re.sub(r"_+", "_", text).strip("_")
    return text[:48] or "text"


def surface(path: str) -> str:
    base = os.path.basename(path)[:-3]
    rel = path[len(ROOT) + 1 :]
    if rel.startswith("app/"):
        return base
    return base


def main(apply: bool):
    all_keys = {}
    files = []
    for dp, _, fs in os.walk(ROOT):
        for f in fs:
            if f.endswith(".rs") and not f.endswith("_tests.rs"):
                files.append(os.path.join(dp, f))
    files.sort()

    for path in files:
        src = open(path, encoding="utf-8").read()
        # Split off the test module so we never touch fixtures/assertions.
        cut = len(src)
        for marker in ("\n#[cfg(test)]", "\nmod tests {"):
            ix = src.find(marker)
            if ix != -1:
                cut = min(cut, ix)
        head, tail = src[:cut], src[cut:]
        surface_name = surface(path)
        keys = {}

        def key_for(english: str) -> str:
            base = f"{surface_name}.{slugify(english)}"
            key = base
            n = 2
            while key in keys and keys[key] != english:
                key = f"{base}_{n}"
                n += 1
            keys[key] = english
            all_keys[key] = english
            return key

        def repl(m):
            value = unescape(m.group(1) if m.re.groups == 1 else m.group(2))
            if not is_copy(value):
                return m.group(0)
            key = key_for(value)
            if m.re is PATTERNS[0][1]:
                # Preserve a trailing `.to_string()` (harmless on String, but
                # keeps the diff minimal).
                return f".{m.group(1)}(tr!(\"{key}\"){m.group(3) or ''})"
            if m.re is PATTERNS[1][1]:
                return f'{m.group(1)}(tr!("{key}"))'
            if m.re is PATTERNS[2][1]:
                return f'set_status(tr!("{key}"))'
            return f'.child(tr!("{key}"))'

        for _, pat in PATTERNS:
            head = pat.sub(repl, head)
        if apply:
            open(path, "w", encoding="utf-8").write(head + tail)
        else:
            for line in head.splitlines():
                if 'tr!("' in line and line not in src:
                    pass

    for k in sorted(all_keys):
        print(f"{k}\t{all_keys[k]}")


if __name__ == "__main__":
    main("--apply" in sys.argv)
