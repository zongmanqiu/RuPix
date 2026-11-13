#!/usr/bin/env python3
"""Generate the host update manifest (update.json) for one release.

Only four fields are written: version / published / notes / urls.
There is deliberately NO sha256 and NO size: official-channel builds are
identified by file name + version only (user decision 2026-11-07).

Usage (from main/):
    python build/gen_update_json.py --version 0.2.0 \
        --notes ../.temp/release-out/RELEASE_NOTES.md
    python build/gen_update_json.py --version 0.2.0 --notes ... --out ../update.json
"""
import argparse
import json
import re
import sys
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION_RE = re.compile(r"^\d+(\.\d+)*$")
GITEE = "https://gitee.com/qiuzongman/RuPix/releases/download/{tag}/RuPix_{ver}.zip"
GITHUB = "https://github.com/zongmanqiu/RuPix/releases/download/{tag}/RuPix_{ver}.zip"


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="release version, e.g. 0.2.0 (tag = version)")
    ap.add_argument("--notes", required=True, help="UTF-8 file with the user-facing release notes")
    ap.add_argument("--published", default=date.today().isoformat())
    ap.add_argument("--out", default=str(ROOT / "update.json"))
    args = ap.parse_args()

    version = args.version.strip()
    if not VERSION_RE.match(version):
        fail(f"version is not numeric dotted segments: {version}")
    notes_path = Path(args.notes)
    if not notes_path.is_file():
        fail(f"notes file not found: {notes_path}")
    notes = notes_path.read_text(encoding="utf-8").strip()
    if not notes:
        fail("notes file is empty")

    data = {
        "version": version,
        "published": args.published,
        "notes": notes,
        "urls": {"gitee": GITEE.format(tag=version, ver=version),
                 "github": GITHUB.format(tag=version, ver=version)},
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"[ok] {out}  version={version}")
    for host, url in data["urls"].items():
        print(f"     {host}: {url}")
    print("     no sha256 / no size (file name + version only)")


if __name__ == "__main__":
    main()
