#!/usr/bin/env python3
"""Verify both release mirrors against update.json before announcing a release.

Checks ONLY the two agreed verdicts:
  1. file names: URL tail == RuPix_<version>.zip, ZIP entries are the four
     whitelisted names, top directory == RuPix_<version>/
  2. version: the version parsed from the URL / ZIP top directory matches
     update.json's version.
No sha256, no size, no byte comparison (user decision 2026-11-07).

Usage (from main/):
    python build/check_update_asset.py
    python build/check_update_asset.py --local ../.temp/release-out/RuPix_0.1.0.zip
"""
import argparse
import io
import json
import sys
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "update.json"
WHITELIST = {"rupix.exe", "LICENSE", "THIRD-PARTY-NOTICES.md", "README.txt"}
MAX_BYTES = 64 * 1024 * 1024  # memory guard, NOT a size verdict
UA = "RuPix release check"


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def check_bytes(name: str, data: bytes, version: str) -> bool:
    if len(data) < 4 or data[:2] != b"PK":
        print(f"[x] {name}: not a ZIP")
        return False
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            names = z.namelist()
    except zipfile.BadZipFile as e:
        print(f"[x] {name}: bad ZIP ({e})")
        return False
    top = f"RuPix_{version}/"
    seen = set()
    for raw in names:
        n = raw.replace("\\", "/")
        if n.startswith("/") or any(p in ("", ".", "..") for p in n.split("/")):
            if n.endswith("/"):
                continue
            print(f"[x] {name}: illegal path {n}")
            return False
        if n.endswith("/"):
            if n != top:
                print(f"[x] {name}: unexpected directory {n}")
                return False
            continue
        rel = n[len(top):] if n.startswith(top) else n
        if rel not in WHITELIST:
            print(f"[x] {name}: file not whitelisted: {rel}")
            return False
        if rel in seen:
            print(f"[x] {name}: duplicate file {rel}")
            return False
        seen.add(rel)
    if "rupix.exe" not in seen:
        print(f"[x] {name}: rupix.exe missing")
        return False
    print(f"[ok] {name}: file names + version {version} match ({len(seen)} files)")
    return True


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default=str(DEFAULT_MANIFEST))
    ap.add_argument("--local", help="check a local ZIP instead of downloading both mirrors")
    args = ap.parse_args()

    m = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
    version = str(m.get("version", ""))
    if not version:
        fail("update.json has no version")
    expected = f"RuPix_{version}.zip"
    urls = m.get("urls", {})

    if args.local:
        p = Path(args.local)
        if not p.is_file():
            fail(f"local ZIP missing: {p}")
        if p.name != expected:
            fail(f"file name mismatch: {p.name} != {expected}")
        sys.exit(0 if check_bytes("local", p.read_bytes(), version) else 1)

    ok = True
    for host in ("gitee", "github"):
        url = urls.get(host, "")
        if not url or not url.endswith("/" + expected):
            print(f"[x] {host}: URL file name mismatch (want {expected})")
            ok = False
            continue
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=30) as resp:
                data = resp.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                print(f"[x] {host}: response exceeds the memory guard")
                ok = False
                continue
            ok = check_bytes(host, data, version) and ok
        except Exception as e:  # network / decode failure -> report, never crash
            print(f"[x] {host}: download failed: {e}")
            ok = False
    if not ok:
        fail("mirrors not ready: do not announce the release / do not push update.json")
    print(f"[ok] both mirrors agree with update.json ({version})")


if __name__ == "__main__":
    main()
