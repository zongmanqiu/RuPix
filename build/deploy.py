"""6_deploy.bat 的执行体：把 exe 原子地部署到 .build\\，并核对解码器就位。

职责八件（对齐 CurSeen deploy.py 的口径）：
  1. 定位 .temp\\target\\release\\rupix.exe（第一候选）；缺失 = 硬错误。
  2. **PE 图标门禁**：解析 exe 的 .rsrc，RT_GROUP_ICON 必须存在且档位
     = [16,24,32,48,64,256]；缺了 = 硬错误。资源管理器/任务管理器/
     固定快捷方式显示的就是这份嵌入图标（embed-resource 编译 assets\\rupix.rc），
     忘跑 gen_icon / rc.exe 链断了都会在这里拦住，不把没图标的 exe 部署出去。
  2b. **PE 版本门禁**：RT_VERSION 必须存在，键齐全，且数值与字符串都等于
     Cargo.toml 的 [package] version。缺它不是"不好看"：无版本资源的 exe 与打包器
     样本同形（杀软启发式更凶），而且用户报障时连"哪一版"都问不出来。
     两份 .rc 都走 `manifest_optional()`——rc.exe 找不到时只降级成 cargo 警告，
     所以这两道门禁是它们唯一的硬拦截点，必须装。
  3. **原子换名**覆盖 .build\\rupix.exe：先写 rupix.exe.new，旧文件改名
     rupix.exe.old，新文件顶上。直接 copy 会在「exe 正在运行」时必败——
     映像句柄不带写共享，这是实测过的坑，不要改回 copy /Y。
     SIZE + MTIME 对账：只证明「部署的是刚编出来的那个」。不记哈希（2026-11-02 用户口径：
     官方产物不需要指纹）；`shutil.copy2` 保留 mtime，源与目标两者都相同才算部署成功。
  4. 解码器清单核对：.build\\decoders\\ 里至少要有一个 *\\*.dll，
     且文件名要能被解析出 id/ver（m139 起不带 abi）；没有解码器的 .build 开图全靠
     内置 SVG，等于残废——按错误处理。
  5. **随包解码器清单**：把解码器仓根的 `decoders.json` 复制成
     `.build\\docs\\decoders.json`。宿主「解码器 / 可下载」卡进页只读它、不发网络
     （m148）；缺了它，离线用户看不到任何可下载项。
  6. **发布目录自洽两问**（m160，判据与红证都在 `--selftest` 里）：
     a) 随包清单与 `.build\\decoders\\<id>_<ver>.dll` 必须**同 id 同版本**（2026-11-04
        用户裁定：**不比字节**——同版本重编一次，MSVC 换 PDB GUID 字节就变，拿 size 对账
        证明不了"是不是最新"，只会每轮误报）。版本漂了会劝用户去下载一个已经装着的旧版本，
        硬错误拦住；
     b) `.build\\docs\\<id>_<ver>.zip` 里被清单取代的旧插件包删掉（插件仓的打包器只写不清），
        清单不认识的名字与非 zip 文件一个都不碰。
  7. **本机路径门禁两处**（判据同一条 `host_path_hits`，红证在 `--selftest`）：
     a) 即将部署的 rupix.exe —— 拦住"绕过 build\\4_build.bat 直接 cargo build"；
     b) 发布目录里**每一枚会到达用户手里的 PE**：随包 DLL、已装插件、以及插件 ZIP 的
        **内层**（ZIP 要解开扫，压缩流里看不见路径明文）。只查 exe 漏的就是子仓那两份：
        2026-11-02 宿主 exe 当场验到 0，而 `.build\\decoders` 里 8 枚 DLL 一把没重链。

跑法：`python build\\deploy.py --selftest` 只跑上面这些判据的红/绿夹具，不碰 `.build`。

退出码：0 = 部署成功且解码器就位；1 = 任一环失败。
"""
import json
import os
import re
import shutil
import sys
import time
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent          # main\build\
MAIN = HERE.parent                               # main\
ROOT = MAIN.parent                               # <project>\
SRC_EXE = ROOT / ".temp" / "target" / "release" / "rupix.exe"
OUT_DIR = ROOT / ".build"
OUT_EXE = OUT_DIR / "rupix.exe"
DECODER_DIR = OUT_DIR / "decoders"
# 宿主进「解码器」页只读这一份（`core::paths::docs_dir`），刷新时才联网覆盖。
DOCS_DIR = OUT_DIR / "docs"
CATALOG_DST = DOCS_DIR / "decoders.json"
CATALOG_SRC = ROOT / "RuPix-Decoder" / "decoders.json"
# 插件的随包清单与它的下载物（`docs/<id>_<ver>.zip`）：清单只认一版，旧版 zip 要跟着清。
PLUGIN_CATALOG = DOCS_DIR / "plugins.json"

# 产物命名约定（RuPix-Decoder\build.ps1 负责生成）：<id>_<ver>.dll
# m139 起文件名不再带 abi：宿主加载不解析文件名（扫 *.dll -> LoadLibrary ->
# iv_abi_version() 落区间才收），abi 的机器可读真源是导出函数 + decoders.json，
# 文件名那段既不被读也判断不了兼容性。这里只解析 id/ver 做同 id 双份提醒。
DLL_RE = re.compile(r"^(?P<id>.+)_(?P<ver>\d+\.\d+\.\d+)\.dll$")
# 插件包的命名与 DLL 同一套（`make_catalog.py` 写的就是 `<id>_<ver>.zip`）。
ZIP_RE = re.compile(r"^(?P<id>.+)_(?P<ver>\d+\.\d+\.\d+)\.zip$")

# ICO 档位：由 examples/gen_icon.rs 固定写死；与 brand 的同步测试同源。
EXPECTED_ICON_SIZES = [16, 24, 32, 48, 64, 256]

# ── 许可与署名随发布物到达（m182）───────────────────────────────────────
# 两个子仓的包形都是「产物 + 许可全文 + 许可与源码位置说明」（解码器包内 `LICENSE` 与 README 的
# 「许可与源码」一节、插件包内 `LICENSE` 与 `LICENSE-NOTICE.md`）。宿主的发布物不是 ZIP 而是
# `.build\` 这一整目录，所以同等材料落在 exe 同级：用户打开文件夹第一眼就能看见，
# 而不是"许可只存在于源码仓里"。真源仍是 `main\LICENSE` 与 `main\THIRD-PARTY-NOTICES.md`，
# 这里只复制、不生成第二份。
# ★ **随包不放"源码位置"链接**：宿主仓在 Gitee/GitHub 上还是空的（无提交、无 tag），
#   写上去就是假话——那一格等源码真推上去再补，账在 `docs/待办.md`。
LICENSE_SRC = MAIN / "LICENSE"
NOTICES_SRC = MAIN / "THIRD-PARTY-NOTICES.md"
AGPL_MARK = "GNU AFFERO GENERAL PUBLIC LICENSE"

# 符号归档保留几枚（`.temp/symbols/`）：一枚带行表的 pdb ≈ 21 MiB，五枚 ≈ 105 MiB 封顶。
SYMBOL_KEPT = 5


def cargo_version() -> str:
    """`[package] version` ——版本资源门禁的对照基准（版本号唯一的真来源）。

    只认 `[package]` 段里的那一行：依赖表里也可能出现 `version = `，按整篇正则抓第一处
    会抓到别人的版本号。"""
    in_package = False
    for line in (MAIN / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s.startswith("["):
            in_package = s == "[package]"
        elif in_package and s.startswith("version"):
            m = re.match(r'version\s*=\s*"([^"]+)"', s)
            if m:
                return m.group(1)
    raise ValueError("Cargo.toml 的 [package] 段里找不到 version")


def fail(msg: str) -> int:
    print(f"  [ERROR] {msg}")
    return 1


def pe_rsrc_leaves(data: bytes, name: str) -> list[tuple[list[int], int, int]]:
    """解析 PE 资源树，返回全部叶子 `(id 路径 [type,id,lang], 数据偏移, 数据字节数)`。

    纯 std 实现，只认数字 id 的三层层级（type→id→lang）——够核对本项目自己嵌入的两份
    资源（RT_GROUP_ICON=14、RT_VERSION=16）；命名资源直接跳过。层级异常一律 raise
    ValueError，由调用方转硬错误：门禁宁可误报也不能静默放行。"""
    if data[:2] != b"MZ":
        raise ValueError(f"{name} 不是 PE（缺 MZ）")
    e_lfanew = int.from_bytes(data[0x3C:0x40], "little")
    if data[e_lfanew:e_lfanew + 4] != b"PE\0\0":
        raise ValueError(f"{name} 不是 PE（缺 PE 签名）")
    coff = e_lfanew + 4
    nsec = int.from_bytes(data[coff + 2:coff + 4], "little")
    opt_size = int.from_bytes(data[coff + 16:coff + 18], "little")
    opt = coff + 20
    magic = int.from_bytes(data[opt:opt + 2], "little")
    dd = opt + (112 if magic == 0x20B else 96)  # PE32+ 数据目录偏移不同
    rsrc_rva = int.from_bytes(data[dd + 16:dd + 20], "little")  # 目录表第 2 项 = .rsrc
    if not rsrc_rva:
        raise ValueError("无 .rsrc 目录（rsrc_rva=0）——exe 没有任何嵌入资源")

    secs = []
    sec0 = opt + opt_size
    for i in range(nsec):
        off = sec0 + i * 40
        vsize = int.from_bytes(data[off + 8:off + 12], "little")
        va = int.from_bytes(data[off + 12:off + 16], "little")
        rawsize = int.from_bytes(data[off + 16:off + 20], "little")
        rawptr = int.from_bytes(data[off + 20:off + 24], "little")
        secs.append((va, vsize, rawptr, rawsize))

    def rva2off(rva: int) -> int:
        for va, vsize, rawptr, rawsize in secs:
            if va <= rva < va + max(vsize, rawsize):
                return rawptr + rva - va
        raise ValueError(f"RVA {rva:#x} 落在任何节之外")

    leaves: list[tuple[list[int], int, int]] = []

    def walk(dir_rva: int, ids: list[int]) -> None:
        off = rva2off(dir_rva)
        nname = int.from_bytes(data[off + 12:off + 14], "little")
        nid = int.from_bytes(data[off + 14:off + 16], "little")
        for i in range(nname + nid):
            eoff = off + 16 + i * 8
            name = int.from_bytes(data[eoff:eoff + 4], "little")
            ent = int.from_bytes(data[eoff + 4:eoff + 8], "little")
            if name & 0x80000000:
                continue  # 命名资源（非数字 id）——本项目不用，跳过
            if ent & 0x80000000:
                walk(rsrc_rva + (ent & 0x7FFFFFFF), ids + [name])
            else:
                # 数据目录项：OffsetToData(4) + Size(4)。要的资源体在 OffsetToData 指向处。
                doff = rva2off(rsrc_rva + (ent & 0x7FFFFFFF))
                drva = int.from_bytes(data[doff:doff + 4], "little")
                size = int.from_bytes(data[doff + 4:doff + 8], "little")
                leaves.append((ids + [name], rva2off(drva), size))

    walk(rsrc_rva, [])
    return leaves


def pe_icon_sizes(leaves: list[tuple[list[int], int, int]], data: bytes) -> list[int]:
    """RT_GROUP_ICON 各条目尺寸（升序）；缺组 = raise。"""
    groups = [(i, off) for i, off, _ in leaves if i and i[0] == 14]  # RT_GROUP_ICON = 14
    if not groups:
        raise ValueError(".rsrc 里没有 RT_GROUP_ICON——exe 图标资源缺失")
    _, goff = groups[0]
    count = int.from_bytes(data[goff + 4:goff + 6], "little")
    sizes = []
    for i in range(count):
        e = goff + 6 + i * 14  # GRPICONDIRENTRY：宽、高各 1 字节，0 = 256
        w = data[e] or 256
        h = data[e + 1] or 256
        if w != h:
            raise ValueError(f"RT_GROUP_ICON 第 {i} 条 {w}×{h} 不是正方形")
        sizes.append(w)
    return sorted(sizes)


def _u16z(s: str) -> bytes:
    """VERSIONINFO 里的键与值都是 NUL 结尾的 UTF-16LE；对齐到 4 字节，**不保证落在偶偏移**，
    所以按步长扫字符串会错位（实测 `,CompanyName` 这种半个前缀）。整串比对才可靠。"""
    return s.encode("utf-16-le") + b"\x00\x00"


def pe_version_check(leaves: list[tuple[list[int], int, int]], data: bytes, want: str) -> str:
    """RT_VERSION 门禁：版本资源存在、键齐全、且数值与字符串都等于 Cargo.toml 的 version。

    查的是两类真实失误：① rc.exe 链断了或被 `manifest_optional` 静默降级（现象与图标
    同一条——没版本资源的 exe 交出去，杀软启发式更凶、用户报障问不出是哪一版）；
    ② 模板占位符没替换，属性页一片 `0.0.0`。返回一行给人看的摘要。"""
    hits = [(off, size) for ids, off, size in leaves if ids and ids[0] == 16]  # RT_VERSION
    if not hits:
        raise ValueError(".rsrc 里没有 RT_VERSION——exe 没有版本资源")
    off, size = hits[0]
    blob = data[off:off + size]
    need = ("VS_VERSION_INFO", "StringFileInfo", "VarFileInfo", "FileVersion",
            "ProductVersion", "ProductName", "FileDescription", "CompanyName",
            "OriginalFilename", "InternalName", "Translation")
    missing = [k for k in need if _u16z(k) not in blob]
    if missing:
        raise ValueError(f"RT_VERSION 缺项 {missing}（version.rc.in 与门禁不同步？）")
    # 固定信息区：签名 0xFEEF04BD 之后是结构版本 / FILEVERSION(MS,LS) / PRODUCTVERSION(MS,LS)。
    sig = blob.find(b"\xbd\x04\xef\xfe")
    if sig < 0:
        raise ValueError("RT_VERSION 里找不到 VS_FIXEDFILEINFO 签名")
    fms = int.from_bytes(blob[sig + 8:sig + 12], "little")
    got = f"{fms >> 16}.{fms & 0xFFFF}"
    if not want.startswith(got + "."):
        raise ValueError(f"RT_VERSION 数值 {got} != Cargo.toml version {want}")
    if _u16z(want) not in blob:
        raise ValueError(f"RT_VERSION 字符串里没有 {want}（属性页会显示 {got}）")
    return f"v{want} 数值 {got} 块 {size} B"


# ── 本机路径门禁（便携版不该带着作者的磁盘目录）────────────────────────────
# rustc 把 panic location / debug info 里的**绝对路径**原样写进二进制，所以没做
# `--remap-path-prefix` 的构建会把 `C:\Users\<名字>\...` 连同工程目录一起发给每个下载者。
# 用户 2026-11-02 在**已发布**的 8 枚 DLL 里 strings 出了这些（每个 DLL 3–1300 处），
# 起因就是构建脚本从来没传过 remap。修法在 `build/_common.bat`（宿主）与
# `RuPix-Decoder/workspace/tools/toolchain.ps1`（解码器）。
#
# 判据刻意窄：
#   · `X:\Users\`         —— 只要命中就是别人机器上的用户目录，没有正当理由；
#   · `X:\ … .rs`         —— 盘符开头的源码路径。remap 之后源码路径是相对形式
#                            （`.\src\lib.rs`），所以这条不会误伤。
HOST_PATH_RE = re.compile(rb"[a-z]:\\Users\\|[a-z]:\\[^\x00\r\n\"]{0,140}?\.rs", re.I)


def host_path_hits(data: bytes, limit: int = 3) -> list[str]:
    """产物里残留的本机绝对路径（最多回报 limit 条，够定位就行）。"""
    out: list[str] = []
    for m in HOST_PATH_RE.finditer(data):
        out.append(m.group(0).decode("utf-8", "replace"))
        if len(out) >= limit:
            break
    return out


def shipped_path_hits(out_dir: Path, limit_each: int = 1) -> tuple[list[str], int]:
    """发布目录里**每一枚会到达用户手里的 PE**（含插件 ZIP 的内层）过同一条判据。

    为什么不能只查 exe：2026-11-02 那轮加 remap 时宿主 exe 当场验到 0，而 `.build\\decoders`
    里 8 枚 DLL 一把没重链——「构建脚本改了」不等于「发到用户手里的字节改了」，只盯着 exe
    的门禁会一直报干净，而用户拿 `strings` 一查就有。解码器由子仓构建、插件由子仓打包，
    两条路都不经过上面那一步。
    ★ ZIP 必须**解开再扫**：压缩流里路径不是明文，拿整份 zip 扫字节是假通过。
    """
    hits: list[str] = []
    scanned = 0
    targets = sorted((out_dir / "decoders").glob("*.dll"))
    targets += sorted((out_dir / "plugins").rglob("*.exe"))
    for p in targets:
        scanned += 1
        found = host_path_hits(p.read_bytes(), limit_each)
        if found:
            hits.append(f"{p.relative_to(out_dir)}: {found[0]}")
    for zp in sorted((out_dir / "docs").glob("*.zip")):
        try:
            with zipfile.ZipFile(zp) as z:
                for n in z.namelist():
                    if not n.lower().endswith((".exe", ".dll")):
                        continue
                    scanned += 1
                    found = host_path_hits(z.read(n), limit_each)
                    if found:
                        hits.append(f"{zp.name}!{n}: {found[0]}")
        except (OSError, zipfile.BadZipFile) as e:
            hits.append(f"{zp.name}: 读不开（{e}）")
    return hits, scanned


def catalog_drift(catalog_path: Path, dll_dir: Path) -> tuple[list[str], list[str], list[str]]:
    """随包解码器清单 ↔ `.build\\decoders` 目录，返回 (必须红的差异, 只在清单里, 只在目录里)。

    ★ 判据只用 **id 与 version**（2026-11-04 用户裁定，**不比字节数**）：部署脚本不该把第三方解码器
    `LoadLibrary` 进自己的进程（那玩意儿崩了没人取证），而 `<id>_<ver>.dll` 与清单的
    `id`/`version` 已经够判身份。
    为什么删掉字节比对：MSVC 每次链接都会换 PDB GUID，**同一个版本重编一次字节就变**，
    拿 size 对账等于每轮重编都要重出清单，错一次就误报"大小不符"——它证明不了"是不是最新"，
    只证明"有没有重编过"。真正要守的是"清单说的版本 = 目录里的版本"，那正由 id+version 回答。
    `size` 字段仍留在清单里（设置页「可下载」卡要显示），只是不再当门禁判据。

    "只在清单里"不是缺陷而是设计——清单是**可下载**目录，允许列没随包的项；
    "只在目录里"也只是提醒（清单没重出，UI 会报「清单异常」）。
    真会红的只剩**同一个 id 两边版本不同**。
    """
    shipped = json.loads(catalog_path.read_bytes().decode("utf-8"))
    want = {(str(e.get("id")), str(e.get("version"))) for e in shipped}
    have: set[tuple[str, str]] = set()
    for d in sorted(dll_dir.glob("*.dll")):
        m = DLL_RE.match(d.name)
        if m:
            have.add((m["id"], m["ver"]))
    ids_in_dir = {i for (i, _v) in have}
    drift = []
    for cid, cver in sorted(want):
        if cid in ids_in_dir and (cid, cver) not in have:
            in_dir = "/".join(sorted(v for (i, v) in have if i == cid))
            drift.append(f"{cid}: 清单写 {cver}，目录里却是 {in_dir}")
    only_catalog = sorted(f"{i} {v}" for (i, v) in want if i not in ids_in_dir)
    only_dir = sorted(f"{i} {v}" for (i, v) in have if (i, v) not in want)
    return drift, only_catalog, only_dir


def prune_stale_plugin_zips(catalog_path: Path, docs_dir: Path) -> tuple[list[str], list[str]]:
    """删掉发布目录里被取代的插件包，返回 (删掉的, 删不掉的)。

    `RuPix-Plugin\\tools\\make_catalog.py` 每轮写 `docs\\<id>_<ver>.zip` 而**不清旧版**：
    0.1.1 → 0.1.2 之后目录里躺着两枚 zip，而 `plugins.json` 只认新的 ⇒ 旧那枚是纯垃圾，
    偏偏长得像"能用"。删除范围收得很死：只动**严格匹配 `<id>_<ver>.zip`**、**id 在清单里**、
    且**版本不等于清单那一版**的文件——清单不认识的名字一个都不碰
    （与宿主 `install.rs::drop_older_packages` 同口径：不删用户自己放的东西）。
    """
    entries = json.loads(catalog_path.read_bytes().decode("utf-8"))
    if not isinstance(entries, list):
        return [], []
    keep = {(str(e.get("id")), str(e.get("version"))) for e in entries}
    known_ids = {i for (i, _v) in keep}
    removed: list[str] = []
    stuck: list[str] = []
    for z in sorted(docs_dir.glob("*.zip")):
        m = ZIP_RE.match(z.name)
        if not m or m["id"] not in known_ids or (m["id"], m["ver"]) in keep:
            continue
        try:
            z.unlink()
            removed.append(z.name)
        except OSError as e:
            stuck.append(f"{z.name}: {e}")
    return removed, stuck


def license_gate(main_dir: Path, out_dir: Path, *, ship: bool = True) -> list[str]:
    """许可全文与第三方署名清单必须落进发布目录，而且**内容得真是那份东西**。

    为什么不满足于"文件在"：放一份空壳、或者误拷了别的许可证全文，比干脆不放更难被发现——
    收件人以为拿到了合规的包，实际拿到的是一句看起来像的话。所以三问都查：在不在、
    够不够长、标志串对不对（`LICENSE` 要见 AGPL 标题、清单要写明本项目的许可）。
    返回红名单，空 = 过。`ship=False` 只判不写（夹具用它比对形状）。
    """
    bad: list[str] = []
    specs = (
        ("LICENSE", AGPL_MARK, 30_000),
        ("THIRD-PARTY-NOTICES.md", "AGPL-3.0", 1_000),
    )
    for name, mark, floor in specs:
        src = main_dir / name
        if not src.is_file():
            bad.append(f"缺源文件 {src}")
            continue
        raw = src.read_bytes()
        if len(raw) < floor:
            bad.append(f"{name} 只有 {len(raw)} B（判据下限 {floor} B）——空壳不算送达")
            continue
        if mark not in raw.decode("utf-8", errors="replace"):
            bad.append(f"{name} 里没有 {mark!r}——放错了文件？")
            continue
        if ship:
            # 发布目录可能正被打开着看：与随包清单同一套做法，先 .tmp 再换名。
            dst = out_dir / name
            tmp = dst.with_name(dst.name + ".tmp")
            shutil.copy2(src, tmp)
            os.replace(tmp, dst)
    return bad


def selftest() -> int:
    """把上面三个判据的"会红的形状"钉在夹具里（跑法同 layering/hygiene 的 --selftest）。

    判据一 = 随包清单 ↔ 目录里的 DLL（差异臂刻意用 2026-10-29 真实发生过的那一对：清单写
    `common 0.2.3`、目录里躺 `common_0.2.4.dll`——那天随包的 `decoders.json` 比 DLL 慢了一次
    部署，设置页「可下载」卡会劝用户去装一个已经装着的旧版本）。
    判据二 = 发布目录里的旧插件包只按同 id 清，陌生名字与非 zip 一个都不碰。
    判据三（m182）= 许可全文与第三方署名清单随发布物到达，且**内容真是那份东西**。
    """
    root = ROOT / ".temp" / "rupix-test" / "deploy-selftest"
    for sub in ("decoders", "docs", "lic"):
        shutil.rmtree(root / sub, ignore_errors=True)
    (root / "decoders").mkdir(parents=True)
    (root / "docs").mkdir(parents=True)
    dll = root / "decoders" / "common_0.2.4.dll"
    dll.write_bytes(b"x" * 111)

    def write_catalog(path: Path, entries: list[dict]) -> None:
        path.write_text(json.dumps(entries, ensure_ascii=False), encoding="utf-8")

    checks = []
    # 1) 一致 ⇒ 不红
    write_catalog(
        root / "decoders.json", [{"id": "common", "version": "0.2.4", "size": 111}]
    )
    d, oc, od = catalog_drift(root / "decoders.json", root / "decoders")
    checks.append(("一致不红", d == [] and oc == [] and od == []))
    # 2) 同 id 版本不同 ⇒ 红，且点名是哪个 id
    write_catalog(
        root / "decoders.json", [{"id": "common", "version": "0.2.3", "size": 111}]
    )
    d, _oc, _od = catalog_drift(root / "decoders.json", root / "decoders")
    checks.append(("版本漂移会红", len(d) == 1 and d[0].startswith("common")))
    # 3) 版本对上、字节数不同 ⇒ **不红**（2026-11-04 用户裁定：只认版本与名称）。
    #    真机现场：doc 解码器同版本重编一次，PDB GUID 换了 ⇒ 字节变了，清单却没错。
    write_catalog(
        root / "decoders.json", [{"id": "common", "version": "0.2.4", "size": 999}]
    )
    d, _oc, _od = catalog_drift(root / "decoders.json", root / "decoders")
    checks.append(("字节漂移不红（只认 id+version）", d == []))
    # 4) 清单里有、目录里没有 = 正常的"可下载未随包"，不许红
    write_catalog(
        root / "decoders.json",
        [{"id": "common", "version": "0.2.4", "size": 111}, {"id": "extra", "version": "1.0.0", "size": 1}],
    )
    d, oc, _od = catalog_drift(root / "decoders.json", root / "decoders")
    checks.append(("未随包不算红", d == [] and oc == ["extra 1.0.0"]))
    # 5) 插件包：只删同 id 的旧版，清单不认识的名字与别的文件一个都不碰
    docs = root / "docs"
    (docs / "doodle_0.1.2.zip").write_bytes(b"new")
    (docs / "doodle_0.1.1.zip").write_bytes(b"old")
    (docs / "stranger_9.9.9.zip").write_bytes(b"not in catalogue")
    (docs / "README.md").write_text("keep", encoding="utf-8")
    write_catalog(docs / "plugins.json", [{"id": "doodle", "version": "0.1.2"}])
    removed, stuck = prune_stale_plugin_zips(docs / "plugins.json", docs)
    checks.append(
        (
            "旧包清掉、陌生名字留下",
            removed == ["doodle_0.1.1.zip"]
            and not stuck
            and (docs / "stranger_9.9.9.zip").is_file()
            and (docs / "README.md").is_file()
            and (docs / "doodle_0.1.2.zip").is_file(),
        )
    )
    # 6)~9) 许可与署名随包：判据不满足于"文件在"，要"真是那份东西"
    #（后三条用 ship=False，只判不写 ⇒ 夹具不往 .build 落文件，也不污染真发布目录）
    lic = root / "lic"
    lic_out = lic / "out"
    lic_out.mkdir(parents=True)

    def lic_src(name: str, license_body, notices_body) -> Path:
        d = lic / name
        d.mkdir(parents=True, exist_ok=True)
        if license_body is not None:
            (d / "LICENSE").write_text(license_body, encoding="utf-8")
        if notices_body is not None:
            (d / "THIRD-PARTY-NOTICES.md").write_text(notices_body, encoding="utf-8")
        return d

    ok_license = AGPL_MARK + "\n" + "x" * 40_000
    ok_notices = "本项目按 AGPL-3.0 授权\n" + "y" * 4_000
    checks.append(
        (
            "许可齐 ⇒ 不红且两份都落到发布目录",
            license_gate(lic_src("good", ok_license, ok_notices), lic_out) == []
            and (lic_out / "LICENSE").is_file()
            and (lic_out / "THIRD-PARTY-NOTICES.md").is_file(),
        )
    )
    # 缺许可全文（只剩署名清单）⇒ 红，并点名缺的是哪一份
    r = license_gate(lic_src("missing", None, ok_notices), lic_out, ship=False)
    checks.append(("缺许可全文会红并点名", len(r) == 1 and "缺源文件" in r[0]))
    # 空壳（文件在、几十个字写着"本项目 AGPL"）⇒ 红：这就是"放了等于没放"那一形
    r = license_gate(lic_src("shell", "AGPL-3.0 本项目采用\n", ok_notices), lic_out, ship=False)
    checks.append(("空壳许可会红", len(r) == 1 and "空壳" in r[0]))
    # 长度够但全文是别的许可证（误拷上游 LICENSE 是最可能的真实失误）⇒ 红
    r = license_gate(lic_src("wrong", "MIT License\n" + "z" * 40_000, ok_notices), lic_out, ship=False)
    checks.append(("放错许可证全文会红", len(r) == 1 and "放错了" in r[0]))
    lic_names = [n for n, _ in checks if "许可" in n]
    checks.append(("许可档装上 4 条（少一条＝新判据没接进夹具）", len(lic_names) == 4))
    # 10)~11) 本机路径门禁：真形状要红，remap 之后的相对路径不许误报
    checks.append(
        (
            "本机路径会红（用户目录 + 盘符源码路径）",
            host_path_hits(b"\x00C:\\Users\\someone\\.rustup\\toolchains\\x\\lib.rs\x00") != []
            and host_path_hits(b"\x00E:\\tmp\\proj\\main\\src\\canvas.rs\x00") != []
            # 盘符开头但不是源码、也不是用户目录 ⇒ 判据窄到只抓上面两类，不冤枉它
            and host_path_hits(b"\x00D:\\work\\main\\Cargo.toml\x00") == [],
        )
    )
    checks.append(
        (
            "remap 后的相对路径不误报",
            host_path_hits(b"\x00.\\src\\lib.rs\x00\x00./src/ui/canvas.rs\x00") == []
            and host_path_hits(b"\x00src/core/pixels.rs\x00") == [],
        )
    )
    # 12)~15) 随包产物（解码器 DLL + 插件 ZIP 内层）——只看 exe 漏的就是 2026-11-02 那 8 枚 DLL
    ship = root / "ship"
    for sub in ("decoders", "docs"):
        shutil.rmtree(ship / sub, ignore_errors=True)
    (ship / "decoders").mkdir(parents=True)
    (ship / "docs").mkdir(parents=True)
    dirty = b"\x00E:\\tmp\\proj\\abi\\src\\lib.rs\x00"
    clean = b"\x00.\\abi\\src\\lib.rs\x00~\\.rustup\\toolchains\\x\\kit.rs\x00"
    (ship / "decoders" / "common_0.2.6.dll").write_bytes(dirty)
    hits, n = shipped_path_hits(ship)
    checks.append(
        (
            "随包 DLL 带本机路径会红并点名",
            len(hits) == 1 and "common_0.2.6.dll" in hits[0] and n == 1,
        )
    )
    (ship / "decoders" / "common_0.2.6.dll").write_bytes(clean)
    checks.append(("随包 DLL remap 后不误报", shipped_path_hits(ship) == ([], 1)))
    # ★ 这枚 ZIP 用 DEFLATE 存：整份文件的字节里**看不见**路径明文。红证的意义在此——
    #   谁把这条判据改成"拿 zip 原始字节扫"，下面那条的 `host_path_hits(raw_zip) == []`
    #   这个前提照样成立、而它自己那一半立刻变假绿。
    with zipfile.ZipFile(ship / "docs" / "scan_0.1.0.zip", "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("scan.exe", dirty)
        z.writestr("README.md", "许可与源码位置见本节")
    raw_zip = (ship / "docs" / "scan_0.1.0.zip").read_bytes()
    hits2, n2 = shipped_path_hits(ship)
    checks.append(
        (
            "随包 ZIP 内层会红（不解压看不见，内层也计入扫描数）",
            host_path_hits(raw_zip) == []
            and any("scan_0.1.0.zip!scan.exe" in h for h in hits2)
            and n2 == 2,
        )
    )
    path_names = [name for name, _ in checks if name.startswith("随包")]
    checks.append(("随包产物门禁装上 3 条（少一条＝新判据没接进夹具）", len(path_names) == 3))
    bad = [n for n, ok in checks if not ok]
    for n, ok in checks:
        print(f"  {'ok  ' if ok else 'FAIL'} {n}")
    print(f"selftest: {'ok' if not bad else 'FAILED ' + ', '.join(bad)} ({len(checks)} cases)")
    return 0 if not bad else 1


def main() -> int:
    if not SRC_EXE.is_file():
        return fail(f"cargo output missing: {SRC_EXE}\n         run 4_build.bat first")

    # ── PE 资源门禁（部署前：图标 + 版本）────────────────────────────
    try:
        data = SRC_EXE.read_bytes()
        leaves = pe_rsrc_leaves(data, SRC_EXE.name)
        sizes = pe_icon_sizes(leaves, data)
    except ValueError as e:
        return fail(
            f"PE icon gate: {e}\n"
            "         rc/embed-resource 链路断了？查 cargo build 是否有"
            "‘exe 图标资源嵌入跳过’警告，或跑 cargo run --example gen_icon"
        )
    if sizes != EXPECTED_ICON_SIZES:
        return fail(
            f"PE icon gate: RT_GROUP_ICON 档位 {sizes} != {EXPECTED_ICON_SIZES}"
            "（gen_icon.rs 与 deploy.py 的档位表不同步？）"
        )
    print(f"  icon    : RT_GROUP_ICON {sizes}")
    try:
        note = pe_version_check(leaves, data, cargo_version())
    except ValueError as e:
        return fail(
            f"PE version gate: {e}\n"
            "         查 cargo build 是否有‘exe 版本资源嵌入跳过’警告；"
            "版本资源缺失的 exe 在杀软启发式里与打包器样本同形，"
            "而且用户报障时问不出是哪一版（.symbols 只能靠大小+时间配对）"
        )
    print(f"  version : RT_VERSION {note}")

    # ── 本机路径门禁 ──────────────────────────────────────────────
    # 兜住"有人绕过 4_build.bat 直接 cargo build"：那种产物会把作者的
    # `C:\Users\<名字>\...` 与工程目录一起发给所有下载者（2026-11-02 实测过的原状）。
    hp = host_path_hits(data)
    if hp:
        return fail(
            "host path gate: 产物里还留着本机绝对路径 —— "
            + " / ".join(hp)
            + "\n         RUSTFLAGS 的 --remap-path-prefix 没生效？直接 cargo build 会绕过"
            " build/_common.bat，请用 build\\4_build.bat"
        )
    print("  paths   : 本机绝对路径 0 处")

    OUT_DIR.mkdir(parents=True, exist_ok=True)

    # ── 原子换名部署 ────────────────────────────────────────────────
    new = OUT_EXE.with_suffix(".exe.new")
    old = OUT_EXE.with_suffix(".exe.old")
    shutil.copy2(SRC_EXE, new)
    if old.exists():
        old.unlink()
    if OUT_EXE.exists():
        OUT_EXE.rename(old)
    new.rename(OUT_EXE)
    if old.exists():
        old.unlink()  # 顶上成功后旧副本即弃；留着只会让人怀疑哪个是现行版

    # ── SIZE + MTIME 对账（不记哈希，见文件头第 3 条）──────────────
    s, d = SRC_EXE.stat(), OUT_EXE.stat()
    ok = s.st_size == d.st_size and int(s.st_mtime) == int(d.st_mtime)
    print(f"  exe     : {OUT_EXE}")
    print(f"  size    : {d.st_size} bytes  ({'MATCH' if ok else 'MISMATCH'})")
    print(f"  mtime   : {time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(d.st_mtime))}")
    if not ok:
        return fail("deployed exe does not match the freshly built one (size/mtime)")

    # ── 符号归档（崩溃转储的事后可读性靠这一步）───────────────────────
    # 发布版 exe 是 `strip = true`，`.dmp` 里的自己的帧全是 <unknown>；要精确到行，
    # 必须有**同一次构建**的 rupix.pdb 配对。配对判据用「exe 大小 + 构建时间」——
    # 字节哈希不行：源码一字不改重编它也会变（PE 时间戳 / 链接器 GUID 每次构建都不同）。
    # 落点选 `.temp\symbols\`（中间产物根，不进 .build：pdb 有 7.8MB，不该跟着绿色包走）。
    #
    # ★ 构建时间按**本地时区**（2026-10-25 前这里是 UTC/gmtime）。理由不是"哪个更正确"，
    #   是另一头已经定了口径：崩溃报告 / 会话前言里的 exe mtime 用
    #   `panic_guard::civil_from_secs`（本地），报告里新增的「PDB 配对」那一行也是本地
    #   紧凑串。两边口径不一致时，报障的人拿着 UTC 的文件名去对本地的报告，只能自己
    #   换算时区——换算错了就以为"符号配不上这个版本"（m128）。
    src_pdb = SRC_EXE.with_name("rupix.pdb")
    st = SRC_EXE.stat()
    stamp = time.strftime("%Y%m%d%H%M%S", time.localtime(st.st_mtime))
    utc_stamp = time.strftime("%Y%m%d%H%M%S", time.gmtime(st.st_mtime))
    print(f"  fingerprint: {st.st_size} bytes / {stamp} local (UTC {utc_stamp})  "
          f"(= exactly what the crash report prints; deploy switches to local, same clock)")
    if src_pdb.is_file():
        sym_dir = ROOT / ".temp" / "symbols"
        sym_dir.mkdir(parents=True, exist_ok=True)
        dst = sym_dir / f"rupix-{st.st_size}-{stamp}.pdb"
        shutil.copy2(src_pdb, dst)
        kept = sorted(sym_dir.glob("rupix-*.pdb"), key=lambda f: f.stat().st_mtime, reverse=True)
        # 归档量要设上限：`debug = 1`（行表）之后一枚 pdb 从 7.8 MB 涨到 **21.3 MB**，
        # 不设闸的话一天构建就能把 .temp/symbols 吃成 GB。exe 里记的是「字节数 + 构建时间」，
        # 最近几版足够配对；更早的现场真要查，重新构建那一个版本即可。
        for old in kept[SYMBOL_KEPT:]:
            old.unlink()
        print(f"  symbols : {dst}  ({len(kept[:SYMBOL_KEPT])} archived, "
              f"{sum(f.stat().st_size for f in kept[:SYMBOL_KEPT]) // 1048576} MiB total)")
        print(f"            .temp is disposable; archive this pdb if you still want to read old dumps")
    else:
        print(f"  symbols : MISSING {src_pdb} - a .dmp then only resolves system dll frames; build first")

    # ── 随包解码器清单（.build\docs\decoders.json）─────────────────────
    # 宿主进「解码器」页只读这份、不发网络（m148）。清单真源是解码器仓根的
    # `decoders.json`（`scripts/make_index.py` 从 DLL 自述生成），不在这里再存一份。
    DOCS_DIR.mkdir(parents=True, exist_ok=True)
    if CATALOG_SRC.is_file():
        raw = CATALOG_SRC.read_bytes()
        try:
            items = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            return fail(f"catalogue source is not valid UTF-8 JSON: {CATALOG_SRC} ({e})")
        if not isinstance(items, list) or not items:
            return fail(f"catalogue must be a non-empty JSON array: {CATALOG_SRC}")
        # 应用可能正开着读这份文件：先写 .tmp 再换名，读到的永远是完整的一份。
        tmp = CATALOG_DST.with_name(CATALOG_DST.name + ".tmp")
        shutil.copy2(CATALOG_SRC, tmp)
        os.replace(tmp, CATALOG_DST)
        print(f"  catalogue: {CATALOG_DST} ({len(items)} entries, {len(raw)} bytes)")
    elif CATALOG_DST.is_file():
        print(f"  catalogue: WARNING source missing, keeping existing {CATALOG_DST}")
    else:
        return fail(
            "local decoder catalogue missing: .build\\docs\\decoders.json\n"
            "         build the decoder repo first (RuPix-Decoder\\decoders.json),"
            " then re-run 6_deploy.bat"
        )

    # ── 解码器清单 ────────────────────────────────────────────────────
    if not DECODER_DIR.is_dir():
        return fail(f"decoder dir missing: {DECODER_DIR}\n         run 3_build_ext.bat first")
    dlls = sorted(DECODER_DIR.glob("*.dll"))
    if not dlls:
        return fail("no decoder DLLs in .build\\decoders - run 3_build_ext.bat")
    print(f"  decoders : {len(dlls)} DLL(s)")
    bad = []
    for d in dlls:
        m = DLL_RE.match(d.name)
        if m:
            print(f"            {d.name}  (id={m['id']} ver={m['ver']})")
        else:
            bad.append(d.name)
            print(f"            {d.name}  (WARNING: name breaks <id>_<ver>.dll convention)")
    if bad:
        # 命名不合规只是警告：宿主靠 iv_decoder_info() 自述，不靠文件名；
        # 但同 id 双份会重复加载，值得喊一声。
        print("  [WARNING] non-conventional DLL names above - check for duplicate ids")

    # ── 随包清单 ↔ 目录里的 DLL：同一份发布目录不许各说各话 ────────────────
    # 为什么会漂：解码器仓重建了 `.build\decoders` 却没重出 `RuPix-Decoder\decoders.json`。
    # 后果看得见——设置页「可下载」卡拿旧清单对新版 DLL，会劝用户去下载一个**已经装在
    # 目录里的旧版本**（2026-10-29 实测：随包那份停在 10-28 的 4 条 0.2.3，目录里是 7 条）。
    # 判据本体与红证见 `catalog_drift` / `selftest`（跑法：`python build/deploy.py --selftest`）。
    if CATALOG_DST.is_file():
        drift, only_catalog, only_dir = catalog_drift(CATALOG_DST, DECODER_DIR)
        if drift:
            return fail(
                "随包清单与 .build\\decoders 对不上：\n         "
                + "\n         ".join(drift)
                + "\n         修法：重出清单（RuPix-Decoder\\scripts\\make_index.py）后再部署，"
                  "**不要手改** .build\\docs\\decoders.json"
            )
        print(f"  catalogue check: 同 id 同版本（不比字节，{len(dlls)} 枚 DLL）")
        if only_catalog:
            print(f"            只在清单里（可下载、未随包）: {', '.join(only_catalog)}")
        if only_dir:
            print(f"            [WARNING] 只在目录里（清单没这一条）: {', '.join(only_dir)}")

    # ── 发布目录里的插件包：同 id 只留清单上那一版（判据见 prune_stale_plugin_zips）──
    if PLUGIN_CATALOG.is_file():
        removed, stuck = prune_stale_plugin_zips(PLUGIN_CATALOG, DOCS_DIR)
        for s in stuck:
            print(f"  [WARNING] 旧插件包删不掉（正被占用？）：{s}")
        if removed:
            print(f"  pruned stale plugin zips: {', '.join(removed)}")

    # ── 随包产物的本机路径（判据本体见 shipped_path_hits）──────────────────
    # exe 上面那一步已单独查过；这里管的是**不经宿主构建链**的那两份：随包 DLL 与插件 ZIP。
    shipped, n_scanned = shipped_path_hits(OUT_DIR)
    if shipped:
        return fail(
            "host path gate: 发布目录里的随包产物还带着本机绝对路径：\n         "
            + "\n         ".join(shipped[:8])
            + ("\n         （只报了前 8 条）" if len(shipped) > 8 else "")
            + "\n         修法 = 用带 remap 的入口**重编那一份**：宿主 build\\4_build.bat /"
              " 解码器 RuPix-Decoder\\build.ps1 / 插件 RuPix-Plugin\\build.ps1。"
              "改构建脚本不等于改产物——只换 .build 里的副本或只改脚本都会留这一格"
        )
    print(f"  paths   : 随包 {n_scanned} 枚 PE（含 ZIP 内层）本机绝对路径 0 处")

    # ── 许可与署名随发布物到达（m182；判据与四条红证见 license_gate / --selftest）──
    # 两个子仓的包形都是「产物 + 许可全文 + 许可与源码位置说明」，宿主的发布物是这一整目录，
    # 同等材料就落在 exe 同级。缺了它，用户拿到的是"只有 exe"的 AGPL 作品。
    bad_lic = license_gate(MAIN, OUT_DIR)
    if bad_lic:
        return fail(
            "发布目录缺许可材料，部署中止：\n         "
            + "\n         ".join(bad_lic)
            + "\n         真源 = main\\LICENSE 与 main\\THIRD-PARTY-NOTICES.md；"
              "补缺的那一份即可，**不要**手抄进 .build\\（那会变成第二真源）"
        )
    print("  license   : LICENSE + THIRD-PARTY-NOTICES.md 已随包（与两个子仓的包形同构）")

    print("  deploy OK")
    return 0


if __name__ == "__main__":
    # 控制台默认按系统码页（GBK）编码，中文判词会被吞成半个字节（同 K132 那条 `--json`
    # 的教训：读不回不等于没跑）。输出通道锁 UTF-8，与 `structure_gen.py` 同一做法。
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(selftest() if "--selftest" in sys.argv else main())
