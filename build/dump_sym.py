# dump_sym.py —— 把 RuPix 自己写的崩溃现场解成"崩在哪个函数、哪一行"
#
# 为什么要有这支脚本：发布版 exe 是 `strip = true`，崩溃现场留在 `crash-*.log` 里的
# 只有「模块+偏移」；`.dmp` 里那串地址没有符号就等于一串十六进制。本机**没有**
# WinDbg/cdb（`Windows Kits\10\Debuggers` 不存在），而 `dbghelp.dll` 是系统自带的——
# 所以符号化这条路可以完全离线自走，不必等人装调试器。
#
# 用法（两条都行，第一条是日常路径）：
#   python build/dump_sym.py <temp\crash-YYYY-MM-DD.log>      # 直接读报告里的栈帧
#   python build/dump_sym.py <temp\crash-...-HHMMSS.dmp>      # 读转储，并自动找同名报告
# 额外参数：
#   --exe PATH      指定 exe（默认从报告里读，读不到就用转储里的模块路径）
#   --symbols DIR   pdb 归档目录（默认 <repo>\.temp\symbols；解码器 dll 的在它下面的 decoders\）
#   --decoders DIR   额外去哪儿找出事的解码器 dll（夹具目录 / 别的发布根）
#
# 它自己会做三件以前要手工做的事：
#   1. **按指纹找 pdb**：报告里那行 `PDB 配对 rupix-<字节数>-<时间>.pdb` 就是文件名；
#      找不到时按「字节数」缩小候选，再逐个**试解**（能解出行号才算配上，解不出就是
#      不同源）。绝不用"文件名看着像"当依据——旧现场的偏移拿到新 pdb 上是**错函数**。
#   2. **把 pdb 摆到 dbghelp 认的名字上**：exe 的 CodeView 记录写的是 `rupix.pdb`，
#      而归档文件带了指纹后缀；这里在临时目录建硬链接（同盘瞬间完成，失败才复制）。
#   3. **一次解完整条栈**：报告里每一帧 `模块+偏移` 都解；第三方模块（解码器 dll）从
#      `<归档>\decoders\<id>.pdb` 摆出符号（发布名 `jpeg_0.1.0.dll` 与 CodeView 名
#      `jpeg.pdb` **不是一个名字**，这层换算脚本自己做），摆不平的如实写"没找到符号"，不猜。
#
# 三个坑（都是别人踩过、这里照防的）：
#   1. `MINIDUMP_EXCEPTION` 里 `ExceptionAddress` 排在 `NumberParameters` **之前**，
#      整条 152 字节 ⇒ `ThreadContext.DataSize` 在流起始 +160、`.Rva` 在 +164。
#      取错偏移的后果是"寄存器全是 0"，栈回溯整个哑掉——看着像 dump 坏了。
#   2. `SymLoadModuleEx` 报 err 18 (ERROR_NO_MORE_FILES) 多半是**传的 exe 路径不存在**，
#      不是 dbghelp 不行。这里先 `os.path.isfile` 再调，并把 GetLastError 原样打出来。
#   3. `SYMBOL_INFO` 的真实布局里有 `Address/Register/Scope/Tag` 四个字段（见下面
#      结构体注释）。漏掉它们的后果**不是报错**，而是 `SymFromAddr` 返回成功、名字空、
#      Size 一坨垃圾——2026-10-25 之前本脚本就栽在这里，还据此得出了"debug=1 没有
#      函数名、要开 debug=2"的错误结论（实测 debug=1 名字齐全，pdb 21.9 MB；
#      debug=2 是 74.2 MB，白胖 3.4 倍）。见 `docs/踩坑记录.md` K135。
#
# x64 的 CONTEXT 偏移取自本机 SDK 头（`winnt.h` 的 `DECLSPEC_ALIGN(16) _CONTEXT`）：
# Rax 0x78 … Rsp 0x98、Rbp 0xA0、R15 0xF0、**Rip 0xF8**（0x100 往后是 Xmm，不是 Rip）。
#
# 输出保持 ASCII：这些行会被贴进工单/聊天，中文在这台机器的控制台上会花掉。

import ctypes
import os
import re
import struct
import sys

STREAM_NAMES = {
    1: "Unused", 3: "ThreadList", 4: "ModuleList", 5: "MemoryList", 6: "Exception",
    7: "SystemInfo", 8: "ThreadExList", 9: "Memory64List", 13: "ThreadInfoList",
    15: "HandleOperationList", 16: "Function", 17: "SystemMemory", 18: "ProcessSummary",
    19: "AssertionInfo", 20: "HeapList", 21: "UnloadedModule", 22: "MemoryInfoList",
    23: "CommentA", 24: "ThreadNames", 25: "CommentW", 26: "ModuleListEx",
}

EXC_CODES = {
    0xC0000005: "STATUS_ACCESS_VIOLATION (read/write bad address)",
    0xC0000094: "STATUS_INTEGER_DIVIDE_BY_ZERO",
    0xC00000FD: "STATUS_STACK_OVERFLOW",
    0xC0000374: "STATUS_HEAP_CORRUPTION",
    0xC0000409: "STATUS_STACK_BUFFER_OVERRUN (__fastfail /GS)",
    0x80000003: "STATUS_BREAKPOINT",
    0xE06D7363: "MSVC C++ exception (unwind path)",
}

# winnt.h: KPROCESSOR_STATE/CONTEXT 里我们要看的寄存器（固定偏移）
# 符号名缓冲区：嵌套 class 体看不见外层 class 的名字，所以常量放模块级。
NAME_CAP = 1024

# 仓库根（`<repo>/main/build/dump_sym.py` 上三层）：符号路径缩到这个根下，跨机器也读得懂。
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

REG_OFFS = [("rax", 0x78), ("rcx", 0x80), ("rdx", 0x88), ("rbx", 0x90), ("rsp", 0x98),
            ("rbp", 0xA0), ("rsi", 0xA8), ("rdi", 0xB0), ("r8", 0xB8), ("r9", 0xC0),
            ("r10", 0xC8), ("r11", 0xD0), ("r12", 0xD8), ("r13", 0xE0), ("r14", 0xE8),
            ("r15", 0xF0), ("rip", 0xF8)]


# ───────────────────────────── minidump ─────────────────────────────
def read_streams(blob):
    sig, ver, count, dir_rva = struct.unpack_from("<IIII", blob, 0)
    if sig != 0x4D4D444D and sig != 0x504D444D:
        raise SystemExit("not a minidump: signature 0x%08X" % sig)
    return ver, [struct.unpack_from("<III", blob, dir_rva + i * 12) for i in range(count)]


def read_unicode(blob, rva):
    if rva + 4 > len(blob):
        return "(rva 0x%X out of dump - truncated?)" % rva
    n = struct.unpack_from("<I", blob, rva)[0]          # 字节数，不含 NUL
    end = min(rva + 4 + n, len(blob))
    return blob[rva + 4: end].decode("utf-16-le", "replace")


def read_modules(blob, rva):
    (n,) = struct.unpack_from("<I", blob, rva)
    mods = []
    for i in range(n):
        off = rva + 4 + i * 108
        base, size, _cksum, ts, name_rva = struct.unpack_from("<QIIII", blob, off)
        cv_size, cv_rva = struct.unpack_from("<II", blob, off + 76)
        mods.append({"base": base, "size": size, "ts": ts,
                     "name": read_unicode(blob, name_rva), "cv": (cv_rva, cv_size)})
    return mods


def read_cv(blob, cv_rva, cv_size):
    """CvRecord：只从里面**捞出以 .pdb 结尾的字符串**，不去拼 GUID。

    为什么退这一步：MS 文档里 CvRecord 的头部宽度在不同 dump 里见过两种写法（有/没有
    前置的 CvDataType），按其中一种拼出来的 age/guid 是"看着像但不是"的假数（实测
    age 解出来是 ASCII `in_s`）。假 GUID 比没 GUID 坏——它会让人以为配对成功了。
    真正的配对判据是**试解**：dbghelp 按 exe 内 CodeView 记录去找 pdb，能解出行号就是
    同源，解不出就是不同源（见 `pick_pdb`）。
    """
    if not cv_size or cv_rva + cv_size > len(blob):
        return {"pdb": None, "head": ""}
    rec = blob[cv_rva: cv_rva + cv_size]
    words = [w.decode("utf-8", "ignore") for w in rec.split(bytes([0])) if 4 < len(w) < 300]
    pdbs = [w for w in words if w.lower().endswith(".pdb")]
    return {"pdb": pdbs[-1] if pdbs else None, "head": rec[:16].hex(" ")}


def read_thread_names(blob, rva, size):
    """ThreadNamesStream = NumberOfThreads(4) + 每条 MINIDUMP_THREAD_NAME。

    MINIDUMP_THREAD_NAME = { ULONG32 ThreadId; RVA64 ThreadNameRva; }，**紧凑排列 12 字节**
    （RVA 紧跟在 ThreadId 后面，没有对齐填充）。2026-10-21 实测两种错读：
    按 8 字节一条 → 第一条碰巧还对、第二条起把模块路径当字符串；按 16 字节一条 →
    第一条的 RVA 就飞了。判据是流长度：4 + n×12 == size。
    条数在**首字**，不在 +4（那里是第一条的 ThreadId）。
    """
    if not size or size < 4:
        return []
    n = struct.unpack_from("<I", blob, rva)[0]
    out = []
    for i in range(min(n, 4096)):
        off = rva + 4 + i * 12
        if off + 12 > len(blob):
            break
        tid = struct.unpack_from("<I", blob, off)[0]
        srva = struct.unpack_from("<Q", blob, off + 4)[0]
        out.append((tid, read_unicode(blob, srva)))
    return out


# ───────────────────────────── PE ─────────────────────────────
def pe_bases(exe_path):
    """(ImageBase, SizeOfImage, AddressOfEntryPoint) —— 符号化只需要前两个。"""
    d = open(exe_path, "rb").read(4096)
    e_lfanew = struct.unpack_from("<I", d, 0x3C)[0]
    if d[e_lfanew:e_lfanew + 4] != b"PE\0\0":
        raise SystemExit("not a PE file: %s" % exe_path)
    opt = e_lfanew + 24
    magic = struct.unpack_from("<H", d, opt)[0]
    if magic == 0x20B:                      # PE32+
        entry, image_base = struct.unpack_from("<I", d, opt + 16)[0], \
            struct.unpack_from("<Q", d, opt + 24)[0]
    elif magic == 0x10B:                    # PE32
        entry, image_base = struct.unpack_from("<I", d, opt + 16)[0], \
            struct.unpack_from("<I", d, opt + 28)[0]
    else:
        raise SystemExit("unknown optional header magic 0x%X" % magic)
    size_of_image = struct.unpack_from("<I", d, opt + 56)[0]
    return image_base, size_of_image, entry


# ───────────────────────────── dbghelp ─────────────────────────────
class Symbolizer:
    """一次 SymInitialize，加载若干模块，按 RVA 解名字与行号。

    名字与行号都从**同一份 pdb** 来；`debug = 1`（行表）就够——2026-10-25 实测
    `mainCRTStartup`、`parse_frames` 这类名字齐全。当年"要 debug = 2 才有名字"的结论
    是 `SYMBOL_INFO` 布局写错造成的（见文件头坑 3），不是档位问题。
    """

    class SymbolInfo(ctypes.Structure):
        # winnt.h 的真实布局（**不是**文档里那串老字段名）：
        #   SizeOfStruct(0) TypeIndex(4) Reserved[2](8) Index(24) Size(28) ModBase(32)
        #   Flags(40) Value(48) **Address(56) Register(64) Scope(68) Tag(72)**
        #   NameLen(76) MaxNameLen(80) Name(84)
        # 漏掉 Address/Register/Scope/Tag 这四个（16 字节）的后果不是报错，而是：
        # SymFromAddr 返回 TRUE、名字空、Size 一坨垃圾 —— 极难自查，2026-10-25 吃过。
        _fields_ = [("SizeOfStruct", ctypes.c_ulong), ("TypeIndex", ctypes.c_ulong),
                    ("Reserved", ctypes.c_uint64 * 2),
                    ("Index", ctypes.c_ulong), ("Size", ctypes.c_ulong),
                    ("ModBase", ctypes.c_uint64), ("Flags", ctypes.c_ulong),
                    ("Value", ctypes.c_uint64), ("Address", ctypes.c_uint64),
                    ("Register", ctypes.c_ulong), ("Scope", ctypes.c_ulong),
                    ("Tag", ctypes.c_ulong), ("NameLen", ctypes.c_ulong),
                    ("MaxNameLen", ctypes.c_ulong),
                    ("Name", ctypes.c_char * NAME_CAP)]

    class Line64(ctypes.Structure):
        # IMAGEHLP_LINE64 的真实布局（本机实测，不是文档里那套老字段名）：
        # SizeOfStruct(4) + pad(4) + Key(8) + LineNumber(4) + pad(4) + FileName(8) + Address(8)
        # 按"Size/FileNumber/LineNumber/Column/Address/FileName"排会拿 Key 当 LineNumber，
        # 于是"文件对、行号 0"——看着像行表坏了，其实是我读错了槽位。
        _fields_ = [("SizeOfStruct", ctypes.c_ulong), ("_pad", ctypes.c_ulong),
                    ("Key", ctypes.c_void_p), ("LineNumber", ctypes.c_ulong),
                    ("_pad2", ctypes.c_ulong), ("FileName", ctypes.c_char_p),
                    ("Address", ctypes.c_uint64)]

    def __init__(self, search_dirs):
        self.dll = ctypes.WinDLL("dbghelp")
        self.k32 = ctypes.WinDLL("kernel32")
        DWORD64, HANDLE = ctypes.c_uint64, ctypes.c_void_p
        d = self.dll
        d.SymSetOptions.argtypes = [ctypes.c_ulong]
        d.SymInitialize.argtypes = [HANDLE, ctypes.c_char_p, ctypes.c_bool]
        d.SymInitialize.restype = ctypes.c_bool
        d.SymLoadModuleExW.argtypes = [HANDLE, HANDLE, ctypes.c_wchar_p, ctypes.c_wchar_p,
                                       DWORD64, ctypes.c_ulong, ctypes.c_void_p, ctypes.c_ulong]
        d.SymLoadModuleExW.restype = DWORD64
        d.SymFromAddr.argtypes = [HANDLE, DWORD64, ctypes.POINTER(ctypes.c_ulong),
                                  ctypes.POINTER(self.SymbolInfo)]
        d.SymFromAddr.restype = ctypes.c_bool
        # 只有 ANSI 版：dbghelp **不导出** SymGetLineFromAddr64W（宏在头文件里，不在 dll 里）
        d.SymGetLineFromAddr64.argtypes = [HANDLE, DWORD64, ctypes.POINTER(ctypes.c_ulong),
                                           ctypes.POINTER(self.Line64)]
        d.SymGetLineFromAddr64.restype = ctypes.c_bool
        self.hproc = HANDLE(-1)                       # 伪句柄：GetCurrentProcess()
        # UNDNAME | LOAD_LINES。**不带 DEFERRED_LOADS**：那个旗会把 PDB 的装载推后到
        # 第一次查询，于是"pdb 根本找不到"会伪装成"这个地址查不出符号"（err 87）——
        # 诊断路径上这种延迟加载是帮倒忙。
        d.SymSetOptions(0x02 | 0x10)
        path = ";".join(os.path.abspath(p) for p in search_dirs if p)
        if not d.SymInitialize(self.hproc, path.encode("utf-8"), False):
            raise SystemExit("SymInitialize failed err=%d" % self.k32.GetLastError())
        self.loaded = {}

    def load(self, exe_path, base=None, size=0):
        """把 exe 载进符号空间，返回 (base, ok)。同一路径只载一次。"""
        key = os.path.abspath(exe_path)
        if key in self.loaded:
            return self.loaded[key]
        if not os.path.isfile(key):
            print("  ! exe not found: %s   <-- this is what err 18 usually means" % key)
            self.loaded[key] = (0, False)
            return self.loaded[key]
        image_base, size_of_image, _entry = pe_bases(key)
        if base is None:
            base = image_base
        got = self.dll.SymLoadModuleExW(self.hproc, None, key, None,
                                        ctypes.c_uint64(base),
                                        size or size_of_image, None, 0)
        err = self.k32.GetLastError()
        if not got:
            print("  ! SymLoadModuleExW failed for %s err=%d (18 = ERROR_NO_MORE_FILES)"
                  % (os.path.basename(key), err))
        self.loaded[key] = (got or image_base, bool(got))
        return self.loaded[key]

    def resolve(self, rva, module_path=None, base=None):
        """RVA → (name or None, (file, line, disp) or None)。两个 API 各自尽力。"""
        if module_path is not None:
            base, _ok = self.load(module_path, base)
        else:
            base = base or 0
        addr = ctypes.c_uint64(base + rva)
        ln = self.Line64()
        ln.SizeOfStruct = ctypes.sizeof(self.Line64)
        ldisp = ctypes.c_ulong()
        line = None
        if self.dll.SymGetLineFromAddr64(self.hproc, addr, ctypes.byref(ldisp), ctypes.byref(ln)):
            f = (ln.FileName or b"").decode("utf-8", "replace")
            line = (f, ln.LineNumber, ldisp.value)
        si = self.SymbolInfo()
        si.SizeOfStruct = 88      # = sizeof(SYMBOL_INFO)，给 80 直接 err=87
        si.MaxNameLen = NAME_CAP
        disp = ctypes.c_ulong()
        name = None
        if self.dll.SymFromAddr(self.hproc, addr, ctypes.byref(disp), ctypes.byref(si)):
            name = si.Name.decode("utf-8", "replace").strip() or None
        return name, line

    def close(self):
        self.dll.SymCleanup(self.hproc)


# 系统自带的 dll：它们的帧**本来**就不该解（微软的 pdb 得连符号服务器），
# 每帧标一句 "no symbols" 会把真正要看的自家帧淹掉。
SYSTEM_MODULES = {
    "ntdll.dll", "kernelbase.dll", "kernel32.dll", "user32.dll", "gdi32.dll", "win32u.dll",
    "ole32.dll", "combase.dll", "shcore.dll", "uxtheme.dll", "dwmapi.dll", "msvcrt.dll",
    "ucrtbase.dll", "advapi32.dll", "shell32.dll", "shlwapi.dll", "sechost.dll",
    "rpcrt4.dll", "comctl32.dll", "imm32.dll", "msctf.dll", "d3d11.dll", "dxgi.dll",
    "setupapi.dll", "cfgmgr32.dll", "bcryptprimitives.dll", "powrprof.dll", "wintrust.dll",
    "crypt32.dll", "oleaut32.dll", "dwrite.dll", "d2d1.dll", "textshaping.dll",
    "gdiplus.dll", "windowscodecs.dll", "propsys.dll", "ntmarta.dll", "msvcp140.dll",
    "vcruntime140.dll", "vcruntime140_1.dll", "msimg32.dll", "version.dll",
}


def short_path(p):
    """pdb 里的绝对路径 → `src/ui/canvas/mod.rs` 这种**能分辨**的形状。

    只留 basename 是不够的：本仓有十几个 `mod.rs`，`mod.rs:2324` 等于没说是哪个。
    优先切到 `src/` 起（跨机器一致，别人的构建目录名不该出现在报告里）。
    """
    p = p.replace("\\", "/")
    low = p.lower()
    root = REPO_ROOT.replace("\\", "/").lower() + "/"
    i = low.find(root)
    if i >= 0:
        return p[i + len(root):]
    for marker in ("/main/src/", "/src/"):
        j = low.find(marker)
        if j >= 0:
            return p[j + len(marker) - 4:]        # 从 "src/" 起
    parts = [x for x in p.split("/") if x]
    return "/".join(parts[-3:])


# ───────────────────────────── 报告解析 ─────────────────────────────
FRAME_RE = re.compile(r"^\s*#(\d+)\s+(\S+?)\+0x([0-9A-Fa-f]+)(.*)$")
EXE_RE = re.compile(r"^exe\s+(.+?)（(\d+) 字节，构建时间 (.+?)）", re.M)
PAIR_RE = re.compile(r"^PDB 配对\s+(\S+\.pdb)", re.M)
RVA_RE = re.compile(r"^异常地址\s+0x([0-9A-Fa-f]+)（RVA 0x([0-9A-Fa-f]+)）", re.M)
BASE_RE = re.compile(r"^exe 基址\s+0x([0-9A-Fa-f]+)", re.M)
THREAD_RE = re.compile(r"^出错线程\s+(\d+)（(.*?)）", re.M)
CODE_RE = re.compile(r"^异常码\s+0x([0-9A-Fa-f]{8})(.*)$", re.M)


def selftest():
    """合成样例自证（`python build/dump_sym.py --selftest`）。

    为什么要有它：这脚本是**取证链的最后一步**，平时没人跑——真到要用它的时候，
    一次悄悄的回归（比如发布名换算改坏）就让人对着一堆十六进制发呆。这里只验
    **不依赖本机符号库**的纯逻辑与一例合成目录，所以任何机器上都能跑、秒级返回。
    """
    import tempfile

    fails = []

    def expect(cond, msg):
        if not cond:
            fails.append(msg)

    # 发布名 → id：这是"崩在哪个解码器"的唯一线索，错了整条链就指错人。
    expect(decoder_id("jpeg_0.1.0.dll") == "jpeg", "jpeg_0.1.0.dll -> jpeg")
    expect(decoder_id("crashplug.dll") == "crashplug", "没有 ABI 段的整段留下")
    expect(decoder_id("core_0.3.0.dll") == "core", "版本号带点也要剥干净")
    # 候选顺序：**同版本归档优先于 `<id>.pdb`（最新）**——顺序反了，老版本的崩溃
    # 就会拿最新那枚 pdb 去试，然后诚实地"解不出"，看着像没归档。
    with tempfile.TemporaryDirectory() as d:
        for n in ("jpeg_0.1.0.pdb", "jpeg.pdb"):
            open(os.path.join(d, n), "wb").close()
        src = decoder_pdb_sources("jpeg_0.1.0.dll", d)
        expect(len(src) == 2, "两枚候选都要在")
        expect(os.path.basename(src[0]) == "jpeg_0.1.0.pdb",
               "同版本那枚必须排在 `<id>.pdb` 前面")
        expect(decoder_pdb_sources("nope_0.1.0.dll", d) == [], "没有的解码器不许凭空造候选")
    # 报告解析：SEH 与 panic 两种正文都要认得出帧与指纹。
    body = ("=== RuPix 崩溃报告：未处理异常（SEH） ===\n"
            "exe         C:\\rupix.exe（5128704 字节，构建时间 2026-10-25 02:03:58）\n"
            "PDB 配对    rupix-5128704-20261025020358.pdb（…）\n"
            "异常地址    0x00007FF600001234（RVA 0x1234）\n"
            "         #00 rupix.exe+0x1234\n"
            "         #01 jpeg_0.1.0.dll+0x158F9\n")
    r = parse_report(body)
    expect(r is not None, "SEH 正文要认得出")
    expect(r["rva"] == 0x1234, "RVA 要从报告里读出来")
    expect(r["pdb"] == "rupix-5128704-20261025020358.pdb", "PDB 配对那行要读出来")
    expect(len(r["frames"]) == 2 and r["frames"][1]["module"] == "jpeg_0.1.0.dll",
           "帧要逐条解析出来（含解码器那一帧）")
    expect(parse_report("nothing here\n") is None, "不是报告的文本要返回 None，不能硬解")

    if fails:
        for f in fails:
            print(" [x] selftest: " + f)
        print("\nselftest failures: %d" % len(fails))
        return 1
    print("selftest: ok")
    return 0


def parse_report(text):
    """一份报告 → dict。一个 log 里可能累了好几份，取**最后一份**（最新那次崩溃 / panic）。

    两种正文都认：
      - `=== RuPix 崩溃报告 …`（SEH 通道，带异常码/异常地址/RVA/基址）
      - `<时间> thread `x` panicked at 文件:行:列 …`（panic 通道，没有异常码与基址）
    panic 通道自 m129 起也带 `exe …（N 字节，构建时间 …）` 与 `PDB 配对 …` 两行——
    否则它末尾那句"怎么查"是敲不通的（脚本找符号全靠这两行）。
    """
    cut = text.rfind("=== RuPix 崩溃报告")
    if cut < 0:
        idx = text.rfind(" panicked at ")
        if idx < 0:
            return None
        cut = text.rfind("\n", 0, idx) + 1      # 回到这一行的行首
    body = text[cut:]
    out = {"raw": body, "frames": [], "exe": None, "size": None, "built": None,
           "pdb": None, "rva": None, "addr": None, "base": None, "tid": None,
           "thread": None, "code": None, "note": ""}
    m = EXE_RE.search(body)
    if m:
        out["exe"], out["size"], out["built"] = m.group(1).strip(), int(m.group(2)), m.group(3)
    m = PAIR_RE.search(body)
    if m:
        out["pdb"] = m.group(1)
    m = RVA_RE.search(body)
    if m:
        out["addr"], out["rva"] = int(m.group(1), 16), int(m.group(2), 16)
    m = BASE_RE.search(body)
    if m:
        out["base"] = int(m.group(1), 16)
    m = THREAD_RE.search(body)
    if m:
        out["tid"], out["thread"] = int(m.group(1)), m.group(2)
    m = CODE_RE.search(body)
    if m:
        out["code"], out["note"] = int(m.group(1), 16), m.group(2).strip()
    for line in body.splitlines():
        fm = FRAME_RE.match(line)
        if fm:
            out["frames"].append({"idx": int(fm.group(1)), "module": fm.group(2),
                                  "off": int(fm.group(3), 16),
                                  "exc": "异常地址" in fm.group(4)})
    return out


# ───────────────────────────── pdb 配对 ─────────────────────────────
def link_as(pdb_path, scratch_dir, name="rupix.pdb"):
    """把归档 pdb 摆到 dbghelp 认的名字上。

    exe 的 CodeView 记录写死 `rupix.pdb`，而归档文件带指纹后缀。同盘用硬链接
    （瞬间、零占用），跨盘才复制。
    """
    os.makedirs(scratch_dir, exist_ok=True)
    dst = os.path.join(scratch_dir, name)
    if os.path.exists(dst):
        try:
            os.remove(dst)
        except OSError:
            pass
    try:
        os.link(pdb_path, dst)
    except OSError:
        import shutil
        shutil.copy2(pdb_path, dst)
    return dst


# 解码器 dll 的发布名带版本段（`jpeg_0.1.0.dll`），而 pdb 的 CodeView 名是
# **编译时的 crate 名**（`jpeg.pdb`）——dbghelp 只认后者。归档按 `<id>.pdb` 存。
DECODER_SUFFIX_RE = re.compile(r"_[\d.]+\.dll$", re.I)


def decoder_id(mod):
    """`jpeg_0.1.0.dll` → `jpeg`；没有版本段的（如夹具 `crashplug.dll`）整段留下。"""
    m = DECODER_SUFFIX_RE.search(mod)
    return mod[:m.start()] if m else os.path.splitext(mod)[0]


def decoder_pdb_sources(mod, archive):
    """一枚解码器的候选 pdb，按**该先试哪个**排好：同版本归档 → `<id>.pdb`（最新）。

    发布名里就带着版本（`jpeg_0.1.0.dll`），所以"崩的是哪一版"不用额外记——
    报告里那一行模块名本身就是线索。先试同名归档（正好是那一版），试不开才退回
    `<id>.pdb`（发布时留的最新一枚）。
    """
    pid = decoder_id(mod)
    stem = os.path.splitext(mod)[0]
    return [p for p in dict.fromkeys([os.path.join(archive, stem + ".pdb"),
                                      os.path.join(archive, pid + ".pdb")])
            if os.path.isfile(p)]


def pick_decoder_pdb(mod, dll, rva, archive, scratch_root):
    """为一枚解码器挑出**真能解**的那枚 pdb，返回 (归档路径, scratch 目录) 或 (None, None)。

    判据与宿主那侧同源：dbghelp 的 **CodeView GUID 校验**。GUID 不符它就拒绝给行号，
    于是"归档里躺着一枚别的版本的 pdb"不会变成"编一个错函数名"。
    ★ 每次试解都**只**给这一枚候选（K136：搜索路径上有多个候选时，"解得出"是整条路径
    的属性，不是某一枚文件的属性）——所以每个候选一个独立目录、一个独立 Symbolizer。
    """
    pid = decoder_id(mod)
    for i, src in enumerate(decoder_pdb_sources(mod, archive)):
        d = os.path.join(scratch_root, "plug-%s-%d" % (pid, i))
        link_as(src, d, pid + ".pdb")
        sym = Symbolizer([d])
        try:
            sym.load(dll)
            _name, line = sym.resolve(rva, dll)
        finally:
            sym.close()
        if line:
            return src, d
    return None, None


def stage_decoder_pdbs(frames, exe_name, symbols_dir, scratch_root, dll_of):
    """给报告里每一枚解码器模块挑 pdb 并摆好，返回 (搜索目录 or None, {模块: 状态})。

    状态三值：`ok`（挑到了）/ `no-archive`（归档里根本没有这枚解码器）/ `other-build`
    （有，但 GUID 都对不上——是另外一版，不给行是对的）。
    """
    archive = os.path.join(symbols_dir, "decoders")
    if not os.path.isdir(archive):
        return None, {}
    out = os.path.join(scratch_root, "decoders")
    have = {}
    for mod in sorted({f["module"] for f in frames}):
        if mod.lower() == exe_name or mod.lower() in SYSTEM_MODULES:
            continue
        dll = dll_of(mod)
        if not dll:
            have[mod] = "no-archive"
            continue
        off = next(f["off"] for f in frames if f["module"] == mod)
        src, _d = pick_decoder_pdb(mod, dll, off, archive, scratch_root)
        if not src:
            have[mod] = "no-archive" if not decoder_pdb_sources(mod, archive) else "other-build"
            continue
        # 摆进**共享**目录供最后统一解栈：两种名字都摆（形态 B 可能跟着发布名走）。
        for name in dict.fromkeys([decoder_id(mod) + ".pdb",
                                   os.path.splitext(mod)[0] + ".pdb"]):
            link_as(src, out, name)
        have[mod] = "ok"
    return (out if any(v == "ok" for v in have.values()) else None), have


def resolve_hits(exe_path, rva, extra_dirs):
    """按给定搜索路径试解靶子 RVA，返回"有没有解出行号"。

    每次都新建/销毁 Symbolizer：dbghelp 的符号句柄是**进程级单例**，复用一个实例
    会把上一次载入的模块带进下一次，"这枚 pdb 解不解得开"就成了上一次的结论。
    """
    dirs = [d for d in extra_dirs if d] + [os.path.dirname(os.path.abspath(exe_path))]
    sym = Symbolizer(dirs)
    try:
        sym.load(exe_path)
        _name, line = sym.resolve(rva, exe_path)
        return bool(line)
    finally:
        sym.close()


def pick_pdb(exe_path, expect_name, symbols_dir, rva, scratch_root):
    """挑出**真能解**的那枚 pdb，返回 (pdb_path, scratch_dir) 或 (None, None)。

    判据是试解，不是文件名：同一份 exe 的旧 pdb 也能被 dbghelp 载入（它只按 CodeView
    的 GUID 校验，配不上就静默不给行号），所以"载入成功"不能当配对成功。这里要求
    **至少解出一行**才算。

    ★ 顺序是硬要求：**先看 exe 自己旁边那一枚**。示例探针的 `panic_probe.pdb` 就躺在
    exe 同一目录、GUID 天然对得上；若先扫归档，一枚 GUID 不匹配的归档 pdb 会被 dbghelp
    拒载、真正在干活的是旁边那枚，脚本却把功劳记到归档那枚头上——输出就成了
    「342 KB 的 `panic_probe.exe` 配 `rupix-5128704-….pdb`」，看着正是本节发誓不做的
    "拿错版本硬解"。旁边那枚解不出来才轮到归档，那时"解得出"必然是归档的功劳。
    """
    # dbghelp 只按 exe 的 CodeView 记录里的**基名**去找（产品是 `rupix.pdb`，
    # 示例探针是 `panic_probe.pdb`）——所以摆到 scratch 里的名字要跟着 exe 走。
    want = os.path.splitext(os.path.basename(exe_path))[0] + ".pdb"
    alongside = os.path.join(os.path.dirname(os.path.abspath(exe_path)), want)
    if os.path.isfile(alongside) and resolve_hits(exe_path, rva, []):
        print("  try %-44s (%-16s) -> %s" % (want, "alongside exe", "line hit"))
        return alongside, None
    cands = []
    if expect_name:
        exact = os.path.join(symbols_dir, expect_name)
        if os.path.isfile(exact):
            cands.append((exact, "report fingerprint"))
    if symbols_dir and os.path.isdir(symbols_dir):
        rest = sorted([p for p in
                       (os.path.join(symbols_dir, f) for f in os.listdir(symbols_dir)
                        if f.lower().endswith(".pdb"))],
                      key=lambda p: os.path.getmtime(p), reverse=True)
        for p in rest:
            if all(os.path.abspath(p) != os.path.abspath(c[0]) for c in cands):
                cands.append((p, "newest-first scan"))
    tried = []
    for pdb, why in cands:
        scratch = os.path.join(scratch_root, "pdb-%d" % len(tried))
        link_as(pdb, scratch, want)
        hit = resolve_hits(exe_path, rva, [scratch])
        tried.append((os.path.basename(pdb), why, hit))
        print("  try %-44s (%-16s) -> %s"
              % (os.path.basename(pdb), why, "line hit" if hit else "no line"))
        if hit:
            return pdb, scratch
    if tried:
        print("  ! no candidate pdb resolved any line; the exe is probably a different build")
    return None, None


# ───────────────────────────── main ─────────────────────────────
def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    # 头注释承诺"输出保持 ASCII"：控制台是 GBK 时，线程名/文件名里一个非 GBK 字符就会
    # 让 print 抛 UnicodeEncodeError、整条符号化在最后一步前挂掉（2026-10-21 实测）。
    sys.stdout.reconfigure(errors="backslashreplace")

    if "--selftest" in sys.argv:
        return selftest()
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    opts = {}
    for a in sys.argv[1:]:
        if a.startswith("--") and "=" in a:
            k, v = a[2:].split("=", 1)
            opts[k] = v
    if not args:
        raise SystemExit(__doc__)
    # 兼容老写法 `dump_sym.py <dump> <exe> [符号目录]`：仓库里多处文档/脚本按那个顺序调过，
    # 突然只认 `--exe` 会让旧命令**静默**丢掉 exe（然后报"exe not found"）。
    if len(args) > 1 and "exe" not in opts:
        opts["exe"] = args[1]
    if len(args) > 2 and "symbols" not in opts:
        opts["symbols"] = args[2]
    target = os.path.abspath(args[0])
    # 仓库根要上三层（少一层会指到 main，找不到 .temp），见模块级 REPO_ROOT
    repo = REPO_ROOT
    symbols_dir = opts.get("symbols") or os.path.join(repo, ".temp", "symbols")

    report = None
    dmp = None
    if target.lower().endswith(".dmp"):
        dmp = target
        guess = re.sub(r"-\d{6}\.dmp$", ".log", target)     # 同一目录的当日报告
        if os.path.isfile(guess):
            report = parse_report(open(guess, "rb").read().decode("utf-8", "replace"))
            print("report    : %s (sibling of the dump)" % os.path.basename(guess))
    else:
        report = parse_report(open(target, "rb").read().decode("utf-8", "replace"))
        if report is None:
            raise SystemExit("no '=== RuPix 崩溃报告' block in %s" % os.path.basename(target))
        # 报告旁边可能有本次转储（名字带时分秒）；有就顺带把寄存器也读出来
        d = os.path.dirname(target)
        stem = os.path.splitext(os.path.basename(target))[0]
        cands = sorted([os.path.join(d, f) for f in os.listdir(d)
                        if f.startswith(stem + "-") and f.endswith(".dmp")],
                       key=os.path.getmtime)
        dmp = cands[-1] if cands else None

    if report:
        if report["code"] is None:
            print("crash     : %s  (panic channel - no exception code)"
                  % (report["built"] or "?"))
        else:
            print("crash     : %s  thread %s(%s)  code 0x%08X %s"
                  % (report["built"] or "?", report["tid"], report["thread"],
                     report["code"], EXC_CODES.get(report["code"], "")))
            if report["note"]:
                print("            %s" % report["note"])
        print("frames    : %d captured, exception rva %s"
              % (len(report["frames"]),
                 ("0x%X" % report["rva"]) if report["rva"] else "-"))

    if dmp:
        blob = open(dmp, "rb").read()
        ver, streams = read_streams(blob)
        print("dump      : %s (%d bytes, format 0x%08X)"
              % (os.path.basename(dmp), len(blob), ver))
        names_e = next(((s, r) for st, s, r in streams if st == 24), None)
        if names_e:
            tn = read_thread_names(blob, names_e[1], names_e[0])
            if tn:
                print("threads   : " + ", ".join("%d=%s" % (t, n) for t, n in tn))
        exc = next((r for st, _, r in streams if st == 6), None)
        mods = read_modules(blob, next((r for st, _, r in streams if st == 4), 0)) \
            if any(st == 4 for st, _, _ in streams) else []
        if exc is not None:
            (tid,) = struct.unpack_from("<I", blob, exc)
            code = struct.unpack_from("<I", blob, exc + 8)[0] & 0xFFFFFFFF
            addr = struct.unpack_from("<Q", blob, exc + 24)[0]
            npar = struct.unpack_from("<I", blob, exc + 32)[0]
            rw = struct.unpack_from("<Q", blob, exc + 40)[0] if npar >= 1 else None
            fault = struct.unpack_from("<Q", blob, exc + 48)[0] if npar >= 2 else None
            ctx_size, ctx_rva = struct.unpack_from("<II", blob, exc + 160)  # 坑 1
            print("exception : thread %d  code 0x%08X %s"
                  % (tid, code, EXC_CODES.get(code, "(unlisted)")))
            print("            addr 0x%016X  faulting 0x%016X (%s)"
                  % (addr, fault if fault is not None else 0,
                     {0: "read", 1: "write"}.get(rw, "-")))
            hit = next((m for m in mods if m["base"] <= addr < m["base"] + m["size"]), None)
            if hit:
                print("module    : %s  base 0x%012X  **RVA 0x%X**"
                      % (os.path.basename(hit["name"]), hit["base"], addr - hit["base"]))
            if ctx_rva and ctx_size >= 0x100:
                regs = {nm: struct.unpack_from("<Q", blob, ctx_rva + off)[0]
                        for nm, off in REG_OFFS}
                print("registers : " + " ".join("%s=0x%012X" % (nm, regs[nm])
                                                for nm in ("rip", "rsp", "rbp", "rax", "rcx", "rdx")))
            if report is None and hit:
                # 只有转储、没有报告：至少把异常地址这一帧解出来
                report = {"frames": [{"idx": 0, "module": os.path.basename(hit["name"]),
                                      "off": addr - hit["base"], "exc": True}],
                          "rva": addr - hit["base"], "exe": hit["name"],
                          "size": hit["size"], "built": None,
                          "pdb": read_cv(blob, *hit["cv"])["pdb"],
                          "base": hit["base"], "tid": tid, "thread": None,
                          "code": code, "note": ""}

    if not report or not report["frames"]:
        print("\nnothing to symbolize: no frames in the report and no dump")
        return

    exe_path = opts.get("exe") or report.get("exe")
    if not exe_path or not os.path.isfile(exe_path):
        print("\nsymbolize : SKIPPED - exe not found: %s" % exe_path)
        return
    rva = report.get("rva")
    if not rva:
        # 试解靶子：没有异常地址（panic 通道）就取**自己模块**的第一帧。不能随便拿
        # frames[0]——那可能是系统 dll，拿它对 pdb 试解会永远"解不出"，把好 pdb 判成坏。
        exe_name = os.path.basename(exe_path).lower()
        own = [f["off"] for f in report["frames"] if f["module"].lower() == exe_name]
        rva = own[0] if own else report["frames"][0]["off"]
    exe_name = os.path.basename(exe_path).lower()

    print("\npdb       : %s" % (report.get("pdb") or "(report did not name one)"))
    print("matching  : %s" % symbols_dir)
    scratch_root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                                ".temp", "symscratch")
    scratch_root = os.path.abspath(scratch_root)
    pdb, scratch = pick_pdb(exe_path, report.get("pdb"), symbols_dir, rva, scratch_root)
    if not pdb:
        sys.exit(2)          # 非零退出码：脚本化调用（门禁/批处理）要能看出"没解出来"
    # scratch 为空 = 用 exe 自己旁边那枚（示例探针 / 开发机刚编完的常规情形），
    # 明说来源：读者不该看到"342 KB 的 exe 配了 rupix 的 pdb"这种看着像错配的行。
    print("paired    : %s%s" % (os.path.basename(pdb),
                                " (alongside the exe)" if scratch is None else ""))

    exe_dir = os.path.dirname(os.path.abspath(exe_path))
    dll_cache = {}

    def find_dll(mod):
        """模块名 → dll 文件。`--decoders` 指的那处优先（夹具常在别处），其次 exe 同目录
        与它的 decoders\\。同一模块只找一次。"""
        if mod not in dll_cache:
            cands = []
            if opts.get("decoders"):
                cands.append(os.path.join(opts["decoders"], mod))
            cands += [os.path.join(exe_dir, mod), os.path.join(exe_dir, "decoders", mod)]
            dll_cache[mod] = next((c for c in cands if os.path.isfile(c)), None)
        return dll_cache[mod]

    decoder_dir, decoder_have = stage_decoder_pdbs(report["frames"], exe_name,
                                                symbols_dir, scratch_root, find_dll)
    staged = sorted(m for m, v in decoder_have.items() if v == "ok")
    if staged:
        print("decoders   : %s  (pdb from %s)"
              % (", ".join(staged), os.path.join(symbols_dir, "decoders")))

    sym = Symbolizer([d for d in (scratch, decoder_dir,
                                  os.path.dirname(os.path.abspath(exe_path))) if d])
    try:
        sym.load(exe_path)
        print("\nstack -> source")
        hits = 0
        for f in report["frames"]:
            mod = f["module"].lower()
            tag = ""
            if mod == exe_name:
                name, line = sym.resolve(f["off"], exe_path)
            else:
                name, line = None, None
                dll = find_dll(f["module"])
                if dll:
                    name, line = sym.resolve(f["off"], dll)
                    # ★ 名字可能来自 **dll 的导出表**（dbghelp 没 pdb 也能给出 `iv_probe`），
                    #   所以"有名字"不等于"配上了"。行号不出来时把原因照实写在后面。
                    state = decoder_have.get(f["module"])
                    if name or line:
                        tag = " [%s]" % os.path.basename(dll)
                        if not line and state in ("other-build", "no-archive"):
                            tag += "  (%s)" % ("no pdb archived for this decoder"
                                               if state == "no-archive"
                                               else "archived pdb is another build")
                    elif mod not in SYSTEM_MODULES:
                        # 分两种"没解出来"：归档里根本没有这枚解码器的 pdb（该去 build.ps1 归档），
                        # 还是有但 GUID 对不上（这是**另一版**解码器，不给行是对的）。
                        tag = "  (no pdb archived for this decoder)" \
                            if decoder_have.get(f["module"]) == "no-archive" \
                            else "  (archived pdb is another build)"
                elif mod not in SYSTEM_MODULES:
                    tag = "  (no symbols for this module)"
            where = ""
            if line:
                hits += 1
                where = "  %s:%d +0x%X" % (short_path(line[0]), line[1], line[2])
            elif mod == exe_name:
                where = "  (no line for this rva)"
            label = name or ("<%s+0x%X>" % (f["module"], f["off"]))
            mark = "  <- exception addr" if f["exc"] else ""
            print("  #%02d  %-28s %s%s%s%s"
                  % (f["idx"], "%s+0x%X" % (f["module"], f["off"]), label, where, tag, mark))
        print("\nresolved  : %d / %d frames" % (hits, len(report["frames"])))
        if hits == 0:
            print("            nothing resolved - the pdb is from another build, "
                  "or the report predates the module+offset format")
            sys.exit(2)
    finally:
        sym.close()


if __name__ == "__main__":
    main()
