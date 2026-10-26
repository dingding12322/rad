#!/usr/bin/env python3
"""Generate and sign Orbit's Sparkle-style appcasts for the in-app updater.

Reads the release artifacts from a directory, Ed25519-signs each one, and
writes one `appcast-<os>-<arch>.xml` per platform/architecture that has a
matching artifact. The files are attached to the release as assets, where the
app fetches them through GitHub's `releases/latest/download/` alias.

The signature and format match what `crates/orbit-pi/src/updater.rs` verifies:
`sparkle:edSignature` is the base64 Ed25519 signature over the *artifact bytes*
(the same signing model Sparkle uses), not over the XML.

Usage:
    python3 scripts/appcast.py \
        --version 0.0.3 --tag v0.0.3 --repo imrj05/orbit \
        --assets artifacts --out artifacts

Signing key:
    ORBIT_UPDATE_PRIVATE_KEY holds the base64 of an Ed25519 private key PEM.
    Generate one with `openssl genpkey -algorithm ed25519` (see
    `docs` in CONTRIBUTING.md → Releasing). Signing uses the `openssl` CLI, so
    no Python crypto dependency is needed.
"""

import argparse
import base64
import glob
import os
import re
import subprocess
import sys
import tempfile
from urllib.parse import quote
from xml.sax.saxutils import escape

# Feed name -> artifact globs, most specific first. macOS feeds prefer the
# arch-native archive and fall back to the universal one; Windows feeds only
# exist once an installer is built.
FEEDS = {
    "macos-aarch64": ["*-arm64.tar.gz", "*-universal.tar.gz"],
    "macos-x86_64": ["*-universal.tar.gz", "*-x86_64.tar.gz"],
    "linux-aarch64": ["orbit-pi-*-aarch64-unknown-linux-gnu.tar.gz"],
    "linux-x86_64": ["orbit-pi-*-x86_64-unknown-linux-gnu.tar.gz"],
    "windows-aarch64": ["Orbit-Pi-*-aarch64-Setup.exe"],
    "windows-x86_64": ["Orbit-Pi-*-x86_64-Setup.exe"],
}

RSS_HEADER = (
    '<?xml version="1.0" standalone="yes"?>\n'
    '<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" '
    'version="2.0">\n'
    "  <channel>\n"
    "    <title>Orbit Pi</title>\n"
)
RSS_FOOTER = "  </channel>\n</rss>\n"

ITEM_RE = re.compile(r"<item>.*?</item>", re.DOTALL)
VERSION_RE = re.compile(r"<sparkle:shortVersionString>(.*?)</sparkle:shortVersionString>")
MAX_ITEMS = 20


def private_key_file(cli_path):
    """Materialize the signing key, preferring an explicit path over the env."""
    if cli_path:
        return cli_path, None
    encoded = os.environ.get("ORBIT_UPDATE_PRIVATE_KEY", "").strip()
    if not encoded:
        sys.exit(
            "error: no signing key. Set ORBIT_UPDATE_PRIVATE_KEY (base64 of an "
            "Ed25519 PEM) or pass --key-file."
        )
    handle = tempfile.NamedTemporaryFile("wb", suffix=".pem", delete=False)
    try:
        handle.write(base64.b64decode(encoded))
    except Exception as error:  # noqa: BLE001 - report and exit
        sys.exit(f"error: ORBIT_UPDATE_PRIVATE_KEY is not base64: {error}")
    finally:
        handle.close()
    return handle.name, handle.name


def sign(key_file, artifact):
    """Base64 Ed25519 signature over the artifact's exact bytes."""
    with tempfile.TemporaryDirectory() as tmp:
        signature = os.path.join(tmp, "sig")
        try:
            subprocess.run(
                [
                    "openssl", "pkeyutl", "-sign", "-rawin",
                    "-inkey", key_file, "-in", artifact, "-out", signature,
                ],
                check=True,
                capture_output=True,
            )
        except FileNotFoundError:
            sys.exit("error: `openssl` is required to sign appcasts")
        except subprocess.CalledProcessError as error:
            sys.exit(error.stderr.decode().strip() or "error: openssl signing failed")
        with open(signature, "rb") as fh:
            return base64.b64encode(fh.read()).decode()


def pick_artifact(assets, globs):
    for pattern in globs:
        matches = sorted(glob.glob(os.path.join(assets, pattern)))
        if matches:
            return matches[0]
    return None


def item_xml(version, url, length, signature, notes=None):
    url = escape(url, {'"': "&quot;"})
    description = (
        f"      <description>{escape(notes)}</description>\n" if notes else ""
    )
    return (
        "    <item>\n"
        f"      <title>{escape(version)}</title>\n"
        f"      <sparkle:shortVersionString>{escape(version)}</sparkle:shortVersionString>\n"
        f"{description}"
        f'      <enclosure url="{url}" length="{length}" '
        f'type="application/octet-stream" sparkle:edSignature="{signature}" />\n'
        "    </item>\n"
    )


def merge(existing_path, new_item, version):
    """Newest first, dropping any earlier entry for the same version."""
    previous = []
    if os.path.exists(existing_path):
        with open(existing_path, encoding="utf-8") as fh:
            for block in ITEM_RE.findall(fh.read()):
                match = VERSION_RE.search(block)
                if match and match.group(1) == version:
                    continue
                previous.append(block.rstrip("\n") + "\n")
    return new_item + "".join(previous[: MAX_ITEMS - 1])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", required=True, help="owner/name")
    parser.add_argument("--assets", default="artifacts")
    parser.add_argument("--out", default="artifacts")
    parser.add_argument("--key-file", help="Ed25519 private key PEM")
    parser.add_argument(
        "--notes-file",
        help="Release notes to embed as each item's <description> (the "
        "update modal renders them as the changelog)",
    )
    args = parser.parse_args()

    notes = None
    if args.notes_file and os.path.exists(args.notes_file):
        with open(args.notes_file, encoding="utf-8") as fh:
            notes = fh.read().strip() or None
        if notes is None:
            print(f"  note  {args.notes_file} is empty; appcasts ship without notes")

    key_file, cleanup = private_key_file(args.key_file)
    os.makedirs(args.out, exist_ok=True)
    wrote = skipped = 0

    try:
        for feed, globs in FEEDS.items():
            artifact = pick_artifact(args.assets, globs)
            if artifact is None:
                print(f"  skip  {feed}: no artifact matching {globs}")
                skipped += 1
                continue
            length = os.path.getsize(artifact)
            name = os.path.basename(artifact)
            url = f"https://github.com/{args.repo}/releases/download/{args.tag}/{quote(name)}"
            signature = sign(key_file, artifact)

            path = os.path.join(args.out, f"appcast-{feed}.xml")
            body = merge(
                path,
                item_xml(args.version, url, length, signature, notes),
                args.version,
            )
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(RSS_HEADER + body + RSS_FOOTER)
            print(f"  wrote {path}  ({name}, {length} bytes)")
            wrote += 1
    finally:
        if cleanup:
            os.unlink(cleanup)

    print(f"{wrote} appcast(s) written, {skipped} skipped")
    if wrote == 0:
        sys.exit("error: no artifacts matched any feed")


if __name__ == "__main__":
    main()
