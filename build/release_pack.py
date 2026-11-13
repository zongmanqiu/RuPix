#!/usr/bin/env python3
"""Build the two release ZIPs from `.build/`:

- `RuPix_<ver>.zip`      self-update shell: 4 files only.
- `RuPix_<ver>_full.zip` first-install/offline package: shell + baseline decoders.

Baseline (user decision 2026-11-13): classic, common, modern, pro, raw.
`doc` is deliberately NOT bundled (26.7 MB; still downloadable in-app), and
`texture` / `doc-legacy` stay optional downloads too.

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
BUILD = ROOT.parent / ".build"
OUT = ROOT.parent / ".temp" / "release-out"

BASELINE = ("classic", "common", "modern", "pro", "raw")
SHELL_FILES = ("rupix.exe", "LICENSE", "THIRD-PARTY-NOTICES.md", "README.txt")
FULL_DOCS = ("decoders.json", "plugins.json", "MCP接入指南.md")
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


def stage_shell(version: str) -> Path:
    stage = OUT / f"RuPix_{version}"
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True)
    for name in SHELL_FILES:
        copy_file(BUILD / name, stage / name)
    return stage


def stage_full(version: str, catalog: dict[str, dict]) -> Path:
    stage = OUT / f"RuPix_{version}_full"
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True)
    for name in SHELL_FILES:
        copy_file(BUILD / name, stage / name)

    docs = stage / "docs"
    docs.mkdir()
    for name in FULL_DOCS:
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
        # 同版本的许可/说明目录：有就带，没有不臆造。
        pkg = BUILD / "decoders" / f"{ident}_{ver}"
        if pkg.is_dir():
            shutil.copytree(pkg, decoders / pkg.name)
    return stage


def generate_formats(stage_full: Path, version: str) -> None:
    """Run the staged exe with only the bundled decoders, then render docs/formats.*."""
    exe = stage_full / "rupix.exe"
    docs = stage_full / "docs"
    sys.path.insert(0, str(ROOT / "build"))
    from deploy import generate_formats as render  # noqa: PLC0415

    errors, _payload = render(exe, docs, version)
    if errors:
        fail("生成 formats.md/json 失败：\n  " + "\n  ".join(errors))


def make_zip(seven: str, stage: Path, out: Path) -> None:
    if out.exists():
        out.unlink()
    run([seven, "a", *ZIP_FLAGS, str(out), stage.name], cwd=OUT)
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
    shell = stage_shell(version)
    full = stage_full(version, catalog)
    generate_formats(full, version)
    # `--list-formats` 会按运行期约定建 temp/ 日志；分发物里不许带运行期落点。
    shutil.rmtree(full / "temp", ignore_errors=True)
    shutil.rmtree(full / "cache", ignore_errors=True)

    shell_zip = OUT / f"RuPix_{version}.zip"
    full_zip = OUT / f"RuPix_{version}_full.zip"
    make_zip(seven, shell, shell_zip)
    make_zip(seven, full, full_zip)

    print(f"[ok] shell {shell_zip}  {shell_zip.stat().st_size} bytes")
    print(f"[ok] full  {full_zip}  {full_zip.stat().st_size} bytes")
    print("     baseline decoders: " + ", ".join(BASELINE))


if __name__ == "__main__":
    main()
