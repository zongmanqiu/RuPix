"""exe 体积分解：把 link.exe `/MAP` 的链接图按【crate × PE 段】两级汇总成账本。

用法（三步，任一步漏了就得到一本假账）：
    1) 单独跑一次带 /MAP 的构建（独立 target，不动 .build 的现行产物）：
        cd main && set CARGO_TARGET_DIR=..\\.temp\\target-map
        set RUSTFLAGS=-C target-feature=+crt-static -C link-arg=/MAP
        cargo build --release --bin rupix
       ★ RUSTFLAGS 必须把 `+crt-static` 一起写上：env 的那条**整条覆盖** .cargo/config.toml
         里的 rustflags，不是追加——漏了就量成另一种链接形态。
    2) python build/size_map.py            # 出账本（默认量 .build/rupix.exe + 上面的 map）
       python build/size_map.py groups     # 加：按分类合计（SVG 栈 / windui / 本项目 / 标准库…）
       python build/size_map.py top usvg    # 加：某 crate 的符号级前 30
       python build/size_map.py meta        # 加：.rdata 1.1 MB 的成分拆分
    3) 换臂复测：RUPIX_EXE / RUPIX_MAP 指到另一套 target 即可（如对照 -C debuginfo=0）。

★ 口径与限度（先读这段，数字才有意义）
1. **只用链接后的图**，不用 `.o`/`.rlib` 的大小：后者含链接器丢掉的死代码，
   拿它报账会把"依赖编了多少"说成"exe 里有多少"。
2. **MSVC 的 map 不给符号长度**，只给地址。所以每条符号的字节数 =
   同段内"下一条地址 − 本条地址"（段末那条记 0 并计入该段"未归账"）。后果：
   - 同址别名（rustc 常发 `f`/`o`/`__imp_` 几条同址记录）只保留第一条，误差在一个函数粒度内，
     但会偏向"文件里先出现的那条"；
   - 段内对齐填充算进它前面那条符号。
   每段"归账 vs 段实测"都打出来当**闭合检查**；差值只该是段末条 + 填充，对不上就是解析错。
3. 归属按 **Lib:Object 列**，即"这段字节物理上在哪个 crate 的编译单元里"。单态化进依赖的
   泛型代码算在**依赖**头上——这才是"换掉谁能让 exe 变小"的正确答案。
4. 段判定用 **RVA 落在哪个 PE 段的虚拟地址区间**，不看 map 的 `0001:` 编号：编号会把
   `.idata$5`/`.CRT$*` 这些合入段当成第一个名字，导致 `.rdata` 全记成 0。
5. `.pdata`（异常/展开表）与 `.reloc` **map 里没有符号行**，天生无法按 crate 归账；
   它们随函数条数增长，单列报账，不摊给任何 crate。
6. 本工具**只出体积，不出速度结论**。体积档位的历史取舍（`lto`、`opt-level`）正本在
   `docs/踩坑记录.md` 的 K53 与 `docs/产品定位与边界.md` §四，不在这里复述。
"""
from __future__ import annotations

import os
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MAP = ROOT / ".temp/target-map/release/deps/rupix.map"
DEFAULT_EXE = ROOT / ".build/rupix.exe"
MAP = Path(os.environ.get("RUPIX_MAP", DEFAULT_MAP))
EXE = Path(os.environ.get("RUPIX_EXE", DEFAULT_EXE))

SYM_ROW = re.compile(
    r"^\s*([0-9a-f]{4}):([0-9a-f]{8})\s+(\S+)\s+([0-9a-f]{16})\s+(?:(\w)\s+)?(\S.*)?$", re.I
)
SEC_TBL = re.compile(r"^\s*[0-9a-f]{4}:[0-9a-f]{8,}\s+[0-9a-f]{8}H\s", re.I)

# 分类：只按**实际出现的 crate 名**归，不自造类目；不认识的落"其他依赖"
GROUPS = {
    "SVG 栈（resvg 全家：解析/排版/光栅）": {
        "resvg", "usvg", "tiny_skia", "tiny_skia_path", "fontdb", "harfrust", "skrifa",
        "read_fonts", "font_types", "ttf_parser", "roxmltree", "simplecss", "svgtypes",
        "unicode_bidi", "unicode_vo", "unicode_script", "kurbo", "data_url", "xmlwriter",
        "base64", "polycool", "line_drawing", "float_cmp",
    },
    "windui（UI 框架）": {"windui", "winit", "softbuffer"},
    "本项目自己的代码": {"rupix", "rupix_lib", "rupix_abi", "vendor"},
    "Rust 标准库": {"std", "core", "alloc", "std_detect", "hashbrown", "smallvec",
                   "rustc_demangle", "panic_unwind", "panic_abort", "profiler_builtins",
                   "unwind", "object", "gimli", "adler", "adler2", "rustc_std_workspace_core"},
    "图像/压缩 crate": {"png", "miniz_oxide", "fdeflate", "crc32fast", "image", "bytemuck",
                        "imagesize", "rgb", "half", "simd_adler32", "flate2"},
    # 宿主的 `rupix.ini` 是手写 `split_once('=')` 解析，零依赖；这组 toml 是被
    # windui 的 `theme` / `i18n` 模块链进来的（m95 实读，别再写成"读 ini 走 toml"）。
    "windui 主题/i18n（toml）": {"toml", "toml_parser", "toml_datetime", "toml_writer", "winnow",
                    "serde", "serde_core", "indexmap", "equivalent", "hashlink"},
    "日志与终端输出": {"log", "env_logger", "env_filter", "anstream", "anstyle",
                       "anstyle_wincon", "anstyle_query", "anstyle_parse",
                       "is_terminal_windows", "colorchoice", "utf8parse", "strsim",
                       "termcolor"},
    "windows 绑定与 Win32": {"windows", "windows_core", "windows_targets", "windows_result",
                             "windows_strings", "windows_target_interfaces", "windows_sys",
                             "rfd", "memmap2", "libloading"},
    "其他依赖": set(),
}
CRT_LIBS = {"libcmt", "libucrt", "libvcruntime", "libvcruntime_app", "libcounters",
            "oldnames", "uuid"}


def group_of(crate: str) -> str:
    c = crate.lower()
    if c.startswith("crt:"):
        return "MSVC/UCRT 静态运行库"
    if c.startswith("imp:"):
        return "导入 thunk（系统 DLL 存根）"
    for g, members in GROUPS.items():
        if c in members:
            return g
    return "其他依赖"


KNOWN = set().union(*GROUPS.values()) | CRT_LIBS


def crate_of(objcol: str) -> str:
    """`libresvg-<h>:resvg-<h>.resvg.<h>-cgu.0.rcgu.o` → `resvg`；
    `libcmt:pow.obj` → `crt:libcmt`（CRT 按库归，按 .obj 名归会碎成 pow/atan2/write）；
    `kernel32:KERNEL32.dll` → `imp:kernel32`（导入 thunk，不是代码）。
    ★ 归档名带 `lib` 前缀（`liblibloading-<h>`）而 crate 叫 `libloading`：候选一律生成
      "整名 / 去 lib 前缀名"两种，优先取认得的那个，否则 libloading 会被切碎成 loading。"""
    objcol = re.sub(r"^\w\s+(?=\S)", "", objcol.strip())   # 类标志字母混进对象列时先摘掉
    lib, _, tail = objcol.partition(":")
    src = (tail or lib).strip()
    if src.lower().endswith(".dll"):
        return f"imp:{src.split('.')[0].lower()}"
    cands: list[str] = []
    for cand in (src, lib):
        m = re.match(r"([A-Za-z0-9_.]+?)[.-][0-9a-f]{16}", cand)
        if m:
            n = m.group(1).split(".")[0]
            cands += [n, n[3:]] if n.startswith("lib") else [n]
    for n in cands:
        if n.lower() in KNOWN:
            return n
    if cands:
        return cands[0]
    base = re.sub(r"\.(lib|exp|obj|o)$", "", lib.split("-")[0])
    if base.lower() in CRT_LIBS or (":" in objcol and not re.search(r"[0-9a-f]{16}", objcol)):
        return f"crt:{base.lower()}"
    # 没有"名字-16 进制哈希"形状的对象（如 `rupix.<25 位 base34>.rcgu.o`）：取首段
    return base.split(".")[0]


def pe_layout(path: Path):
    """→ (image_base, [(段名, 虚拟地址, 虚拟尺寸, 原始字节)])，按段表顺序。"""
    d = path.read_bytes()
    e = int.from_bytes(d[0x3C:0x40], "little")          # e_lfanew
    coff = e + 4                                        # "PE\0\0" 之后
    opt = coff + 20                                     # COFF 头 20 字节
    opt_size = int.from_bytes(d[coff + 16:coff + 18], "little")
    magic = int.from_bytes(d[opt:opt + 2], "little")
    ib = opt + (24 if magic == 0x20B else 28)           # PE32 多一个 BaseOfData
    image_base = int.from_bytes(d[ib:ib + 8], "little")
    n = int.from_bytes(d[coff + 2:coff + 4], "little")
    secs = []
    for i in range(n):
        o = opt + opt_size + i * 40
        secs.append((
            d[o:o + 8].rstrip(b"\0").decode(errors="replace"),
            int.from_bytes(d[o + 12:o + 16], "little"),                  # VirtualAddress
            int.from_bytes(d[o + 8:o + 12], "little"),                   # VirtualSize
            int.from_bytes(d[o + 16:o + 20], "little"),                  # SizeOfRawData
        ))
    return image_base, secs


def load_rows():
    """→ ([(rva, 段名, crate, 符号名)] 升序去重, 段表)。"""
    image_base, secs = pe_layout(EXE)
    spans = [(name, va, va + max(vsize, raw)) for name, va, vsize, raw in secs]

    def sec_of(rva: int):
        for name, lo, hi in spans:
            if lo <= rva < hi:
                return name
        return None

    seen, rows = set(), []
    for ln in MAP.read_text(errors="replace").splitlines():
        if SEC_TBL.match(ln):
            continue
        m = SYM_ROW.match(ln)
        if not m or not m.group(6):
            continue
        obj = m.group(6).strip()
        if obj.startswith("<"):
            continue
        rva = int(m.group(4), 16) - image_base
        name = sec_of(rva)
        if name is None or rva in seen:
            continue
        seen.add(rva)
        rows.append((rva, name, crate_of(obj), m.group(3)))
    rows.sort()
    return rows, secs


def spans_of(rows):
    """每段：{(crate,段): 字节}、{(crate,段,符号): 字节}、{段: 归账合计}。"""
    by_sec: dict[str, list[tuple[int, str, str]]] = defaultdict(list)
    for rva, name, crate, sym in rows:
        by_sec[name].append((rva, crate, sym))
    per: dict[tuple[str, str], int] = defaultdict(int)
    per_sym: dict[tuple[str, str, str], int] = defaultdict(int)
    accounted: dict[str, int] = defaultdict(int)
    for name, lst in by_sec.items():
        for i in range(len(lst) - 1):
            span = lst[i + 1][0] - lst[i][0]
            per[(lst[i][1], name)] += span
            per_sym[(lst[i][1], name, lst[i][2])] += span
            accounted[name] += span
    return per, per_sym, accounted


def kind_of(sym: str) -> str:
    if sym.startswith("$ip2state$"):
        return "$ip2state$ 单步状态表"
    if sym.startswith("$cppxdata$"):
        return "$cppxdata$ 清理数据"
    if sym.startswith("$stateUnwindMap$"):
        return "$stateUnwindMap$ 展开表"
    if sym.startswith("switch.table"):
        return "switch 跳转表"
    if sym.startswith("anon."):
        return "匿名常量 / 字符串字面量"
    return "具名静态数据"


def main() -> int:
    if not MAP.exists() or not EXE.exists():
        print(f"[ERROR] 缺输入：map={MAP} exe={EXE}\n"
              f"        先按文件头第 1 步跑一次带 /MAP 的构建，或用 RUPIX_MAP/RUPIX_EXE 指路。")
        return 1
    rows, secs = load_rows()
    per, per_sym, accounted = spans_of(rows)
    total = sum(accounted.values())
    vsize = {name: vs for name, _va, vs, _raw in secs}
    basis = sum(vsize.values())      # 百分比一律按"PE 段虚拟总量"，不按可归账字节

    print(f"输入      map {MAP.name} ← {MAP.parent}")
    print(f"          exe {EXE}（{EXE.stat().st_size:,} 字节）")
    print(f"符号行 {len(rows):,} 条（同址去重后），可归账字节 {total:,}")
    print("PE 段（虚拟/原始）：" + "  ".join(f"{n}={vs:,}/{raw:,}"
                                            for n, _va, vs, raw in secs))
    for name, _va, vs, raw in secs:
        a = accounted.get(name, 0)
        if vs:
            print(f"  闭合 {name:9s} 归账 {a:>9,} / 虚拟 {vs:>9,}"
                  f"（未归账 {vs - a:>8,} = 段末条 + 对齐填充；原始占盘 {raw:>9,}）")

    agg: dict[str, int] = defaultdict(int)
    for (c, _s), v in per.items():
        agg[c] += v
    print("\n--- 按 crate（前 40；百分比按 PE 段虚拟总量；"
          ".text=代码 .rdata=常量/字符串/静态数据 .data=可写静态）---")
    for c, v in sorted(agg.items(), key=lambda kv: -kv[1])[:40]:
        parts = " ".join(f"{s[1:]}={per[(c, s)] / 1024:6.1f}K"
                         for s in (".text", ".rdata", ".data") if per.get((c, s)))
        print(f"{v / 1024:8.1f} KB {100 * v / basis:5.1f}%  {c[:26]:26s} {parts}")
    tail = sum(v for _c, v in sorted(agg.items(), key=lambda kv: -kv[1])[40:])
    print(f"{'':8}  … 其余 {len(agg) - 40} 项合计 {tail / 1024:.1f} KB")

    if "groups" in sys.argv:
        print("\n--- 按分类合计 ---")
        g: dict[str, int] = defaultdict(int)
        for c, v in agg.items():
            g[group_of(c)] += v
        for k, v in sorted(g.items(), key=lambda kv: -kv[1]):
            print(f"{v / 1024:8.1f} KB {100 * v / basis:5.1f}%  {k}")
        rest = sum(v for n, v in vsize.items() if not accounted.get(n))
        print(f"{rest / 1024:8.1f} KB {100 * rest / basis:5.1f}%  "
              f"不可归 crate 的段（.pdata 展开表 + .reloc 重定位，map 无符号行）")

    if "meta" in sys.argv:
        print("\n--- 数据段成分（.rdata / .data；.text 全是代码，不拆）---")
        by_kind: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
        for (c, sec, s), v in per_sym.items():
            by_kind[sec][kind_of(s)] += v
        for sec in (".rdata", ".data"):
            for k, v in sorted(by_kind[sec].items(), key=lambda kv: -kv[1]):
                print(f"{v / 1024:8.1f} KB  {sec:7s} {k}")

    if len(sys.argv) > 1 and sys.argv[1] == "top":
        needle = sys.argv[2].lower()
        hits = [(v, s, sec) for (c, sec, s), v in per_sym.items() if needle in c.lower()]
        print(f"\n--- 符号级前 30（crate 含 {sys.argv[2]}）---")
        for v, s, sec in sorted(hits, key=lambda x: -x[0])[:30]:
            print(f"{v / 1024:7.1f} KB  {sec:7s} {s[:96]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
