#!/usr/bin/env python3
"""Build the single RuPix release package.

One package only: `RuPix_<ver>.zip` = exe + LICENSE + notices + README +
`classic` / `common` / `modern` / `pro` / `raw` (each DLL **and** its
`<id>_<version>/` license/metadata folder) + `docs/decoders.json` /
`plugins.json` / `MCP接入指南.md`.

There is no "normal vs full" split. The four-file auto-update envelope used in
0.4.0 is retired; 0.3.x auto-update is intentionally paused for this release
because its updater rejects any ZIP entry outside its four-file whitelist.
The next release will make the updater accept this single package.

Usage (from main/):
    python build/release_pack.py
    python build/release_pack.py --version 0.4.0

Only 7-Zip standard ZIP/Deflate is allowed; see `开发指南.md` §打包铁律.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROJECT = ROOT.parent
BUILD = PROJECT / ".build"
DECODER_PACKAGES = PROJECT / ".temp" / "decoder-packages"
OUT = PROJECT / ".temp" / "release-out"

BASELINE = ("classic", "common", "modern", "pro", "raw")
SHELL_FILES = ("rupix.exe", "LICENSE", "THIRD-PARTY-NOTICES.md", "README.txt")
PORTABLE_DOCS = ("decoders.json", "plugins.json", "MCP接入指南.md")
ZIP_FLAGS = ("-tzip", "-mx=9", "-mtm=off", "-mta=off", "-mtc=off")


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def cargo_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'(?m)^version\s*=\s*"([^"]+)"', text)
    if not m:
        fail("Cargo.toml 里找不到 [package] version")
    return m.group(1)


def find_7za() -> str:
    for name in ("7za", "7za.exe"):
        p = shutil.which(name)
        if p:
            return p
    fail("找不到 7za。发布包必须用 7-Zip 标准 ZIP/Deflate，禁止退回别的工具。")


def run(cmd: list[str], cwd: Path | None = None) -> None:
    proc = subprocess.run(cmd, cwd=str(cwd) if cwd else None)
    if proc.returncode != 0:
        fail(f"命令失败（exit {proc.returncode}）：{' '.join(cmd)}")


def copy_file(src: Path, dst: Path) -> None:
    if not src.is_file():
        fail(f"缺文件：{src}")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def decoder_catalog() -> dict[str, dict]:
    path = BUILD / "docs" / "decoders.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        fail(f"读 {path} 失败：{e}")
    if not isinstance(data, list):
        fail("decoders.json 顶层应为数组")
    return {str(item.get("id")): item for item in data if isinstance(item, dict)}


def stage_package(version: str, catalog: dict[str, dict]) -> Path:
    stage = OUT / f"RuPix_{version}"
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True)
    for name in SHELL_FILES:
        copy_file(BUILD / name, stage / name)

    docs = stage / "docs"
    docs.mkdir()
    for name in PORTABLE_DOCS:
        copy_file(BUILD / "docs" / name, docs / name)

    decoders = stage / "decoders"
    decoders.mkdir()
    for ident in BASELINE:
        item = catalog.get(ident)
        if not item:
            fail(f"decoders.json 里没有基础解码器 {ident}")
        ver = str(item.get("version") or "")
        file = f"{ident}_{ver}.dll"
        copy_file(BUILD / "decoders" / file, decoders / file)
        # 每枚 DLL 必须带同版本文件夹（许可 / 元数据 / README）。来源是解码器仓
        # `package_decoders.py` 的 stage，不是 `.build/decoders` 里可能过期的旧目录。
        pkg = DECODER_PACKAGES / f"{ident}_{ver}.stage" / f"{ident}_{ver}"
        if not pkg.is_dir():
            fail(
                f"缺解码器许可目录：{pkg}（先在解码器仓跑 build.ps1 / package_decoders.py）"
            )
        shutil.copytree(pkg, decoders / pkg.name)
    return stage


def make_zip(seven: str, stage: Path, out: Path) -> None:
    if out.exists():
        out.unlink()
    run([seven, "a", *ZIP_FLAGS, str(out), stage.name], cwd=stage.parent)
    listing = subprocess.run(
        [seven, "l", "-slt", str(out)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if listing.returncode != 0:
        fail(f"7za l 失败：{out}")
    if "Path = " not in listing.stdout:
        fail(f"ZIP 里没有条目：{out}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", default=None, help="默认取 Cargo.toml")
    args = ap.parse_args()

    version = (args.version or cargo_version()).strip()
    own = cargo_version()
    if version != own:
        fail(f"--version {version} 与 Cargo.toml {own} 不一致")
    if not BUILD.is_dir():
        fail(f"发布目录不存在：{BUILD}（先跑 6_deploy）")
    OUT.mkdir(parents=True, exist_ok=True)
    seven = find_7za()

    catalog = decoder_catalog()
    stage = stage_package(version, catalog)
    zip_path = OUT / f"RuPix_{version}.zip"
    make_zip(seven, stage, zip_path)

    print(f"[ok] package {zip_path}  {zip_path.stat().st_size} bytes")
    print("     baseline decoders: " + ", ".join(BASELINE))
    print("     each DLL has its <id>_<version>/ license folder")


if __name__ == "__main__":
    main()
