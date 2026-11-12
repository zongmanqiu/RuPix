#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""生成 `main/docs/格式覆盖.md`：可下载档位的格式并集。

真源两处，本脚本只做视图、不存第二份手写表：
  1. `RuPix-Decoder/decoders.json`（解码器仓的发布清单：id / 名称 / 版本 / ABI / formats）；
  2. `main/src/core/formats.rs` 的 `BUILTIN_DECODERS`（宿主内置，今天只有 resvg 的 svg/svgz）。

它回答的是「可下载档位并集」；**某个发布包实际能开什么**以
`rupix.exe --list-formats` 为准（发布包内 `docs/formats.md` 由 deploy.py 用同一命令生成）。
本文件头写死这条分工，避免读者把仓库页当成安装包承诺。

用法：
  python build/gen_formats.py            # 重写 docs/格式覆盖.md
  python build/gen_formats.py --check    # 只比对，漂移则 exit 1
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent          # main\build\
MAIN = HERE.parent                               # main\
ROOT = MAIN.parent                               # 项目根
CATALOG = ROOT / "RuPix-Decoder" / "decoders.json"
FORMATS_RS = MAIN / "src" / "core" / "formats.rs"
OUT = MAIN / "docs" / "格式覆盖.md"


def builtin_exts() -> list[str]:
    """从 `BUILTIN_DECODERS` 的 `extensions: &[...]` 读内置后缀。

    不在这里硬编码 svg/svgz：内置登记表是唯一真源，表变了而脚本没变时，
    `--check` 会立刻显示差异，而不是悄悄少列一条。
    """
    text = FORMATS_RS.read_text(encoding="utf-8")
    at = text.find("pub const BUILTIN_DECODERS")
    if at < 0:
        raise SystemExit(f"找不到 BUILTIN_DECODERS：{FORMATS_RS}")
    m = re.search(r"extensions:\s*&\[(.*?)\]", text[at:], re.S)
    if not m:
        raise SystemExit(f"BUILTIN_DECODERS 里找不到 extensions: &[...]：{FORMATS_RS}")
    exts = re.findall(r'"([^"]+)"', m.group(1))
    if not exts:
        raise SystemExit("BUILTIN_DECODERS 的 extensions 解析为空")
    return sorted({e.lower() for e in exts})


def load_catalog() -> list[dict]:
    raw = json.loads(CATALOG.read_text(encoding="utf-8"))
    if not isinstance(raw, list) or not raw:
        raise SystemExit(f"清单必须是非空 JSON 数组：{CATALOG}")
    out: list[dict] = []
    for i, entry in enumerate(raw):
        if not isinstance(entry, dict):
            raise SystemExit(f"清单第 {i + 1} 项不是对象")
        for key in ("id", "name", "version", "abi", "formats"):
            if key not in entry:
                raise SystemExit(f"清单第 {i + 1} 项缺字段 {key!r}")
        exts = [e.strip().lower() for e in str(entry["formats"]).split(";") if e.strip()]
        if not exts:
            raise SystemExit(f"清单档位 {entry['id']} 的 formats 为空")
        out.append(
            {
                "id": str(entry["id"]),
                "name": str(entry["name"]),
                "version": str(entry["version"]),
                "abi": entry["abi"],
                "desc": str(entry.get("desc", "")),
                "formats": sorted(set(exts)),
            }
        )
    return out


def render() -> tuple[str, int, int, int]:
    builtin = builtin_exts()
    decoders = load_catalog()
    owner: dict[str, list[str]] = {e: ["内置"] for e in builtin}
    for d in decoders:
        for e in d["formats"]:
            owner.setdefault(e, []).append(d["id"])
    unique = sorted(owner)

    lines: list[str] = []
    lines.append("# RuPix 格式覆盖（可下载档位）")
    lines.append("")
    lines.append("> **生成物，勿手改。** 生成源：`RuPix-Decoder/decoders.json`（解码器仓的发布清单）")
    lines.append("> 与 `main/src/core/formats.rs` 的内置登记表。重生成：`python build/gen_formats.py`。")
    lines.append(">")
    lines.append("> 本页是**可下载档位的并集**，不承诺某个安装包已经装了哪些 DLL。")
    lines.append("> 想知道**手头这个包**实际能开什么：运行 `rupix.exe --list-formats`，")
    lines.append("> 或看发布包里的 `docs/formats.md`（部署时用同一命令按实际 DLL 生成）。")
    lines.append("")
    lines.append("## 汇总")
    lines.append("")
    lines.append(f"- 内置（宿主 resvg，不依赖 DLL）：`{'` `'.join(builtin)}`")
    lines.append(f"- 可下载解码器：**{len(decoders)}** 档")
    lines.append(f"- 唯一扩展名：**{len(unique)}** 个（内置 + 各档并集）")
    lines.append("")
    lines.append("| id | 名称 | 版本 | ABI | 后缀数 | 说明 |")
    lines.append("|---|---|---:|---:|---:|---|")
    for d in decoders:
        lines.append(
            f"| `{d['id']}` | {d['name']} | {d['version']} | {d['abi']} | "
            f"{len(d['formats'])} | {d['desc']} |"
        )
    lines.append("")
    lines.append("## 后缀 → 档位")
    lines.append("")
    lines.append("同一后缀被多档认领时按 ` / ` 并列（例如 `raw` 与裸像素档）。")
    lines.append("")
    lines.append("| 后缀 | 档位 |")
    lines.append("|---|---|")
    for e in unique:
        lines.append(f"| `{e}` | {' / '.join(owner[e])} |")
    lines.append("")
    lines.append("## 各档后缀")
    lines.append("")
    lines.append(f"### 内置 · resvg")
    lines.append("")
    lines.append("`" + "` `".join(builtin) + "`")
    lines.append("")
    for d in decoders:
        lines.append(f"### `{d['id']}` · {d['name']}")
        lines.append("")
        lines.append("`" + "` `".join(d["formats"]) + "`")
        lines.append("")
    lines.append("---")
    lines.append("")
    lines.append(
        "口径：后缀均为小写、无点；同一后缀可在多档出现（按解码器自述，不去重归属）。"
    )
    lines.append("")
    body = "\n".join(lines)
    return body, len(builtin), len(decoders), len(unique)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="只比对，不写盘")
    args = ap.parse_args()

    body, n_builtin, n_decoders, n_unique = render()
    if args.check:
        if not OUT.is_file():
            print(f"缺少生成物：{OUT}（跑 python build/gen_formats.py）", file=sys.stderr)
            return 1
        old = OUT.read_text(encoding="utf-8")
        if old != body:
            print(f"格式覆盖页已漂移：{OUT}（重跑 python build/gen_formats.py）", file=sys.stderr)
            return 1
        print(f"格式覆盖: ok（内置 {n_builtin} / 档位 {n_decoders} / 后缀 {n_unique}）")
        return 0

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(body, encoding="utf-8")
    print(f"格式覆盖: {OUT}")
    print(f"  内置 {n_builtin} / 档位 {n_decoders} / 唯一后缀 {n_unique}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
