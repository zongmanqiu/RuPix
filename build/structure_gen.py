r"""结构文档生成器——`目录结构说明.md` 的数字与符号索引由本脚本产出（唯一写入者）。

用法：
  python build/structure_gen.py --write     # 改了 src 之后跑：刷新数字 + 重写符号索引
  python build/structure_gen.py [--check]   # 只比对不写；有漂移 exit 1（2_scan.bat / j10）
  python build/structure_gen.py --selftest  # 提取器自检（合成 Rust；门禁 j11）

为什么要它（2026-10-17）：结构文档的计数与行数此前全选手改，实测已经腐化——文档声称
46 文件 / 18,234 行、ui 13 文件，磁盘实为 45 文件 / 19,108 行、ui 12 文件，decoders 的
行数在总览(1,844)与小节(1,973)还各说一遍。**可数的东西不许手写**。
j9 用 Rust 独立走一遍 `src/` 再对账本文件产出的数字，防的是本脚本自身算错。

口径：
  - 行数 = 文件中 `\n` 的个数（等价 `wc -l`；无结尾换行的文件少计 1，属口径固有）。
  - 分层 = `src/` 下首个路径段；`src/*.rs` 归「根文件」，不判层。
  - 符号索引只收生产区：`#[cfg(test)]` 之后整段不计（惯例「生产在前、tests 在末尾」，
    见 AGENTS.md §八.5，hygiene_scan 依赖同一条）。

符号提取是行级 + 作用域栈，不是完整 Rust 解析器：只在「栈空」或「栈顶是 impl/trait/mod」
时收项，所以函数体内的嵌套 fn / 闭包不进索引。花括号深度跳过注释与字符串字面量
（含 `r"…"` / `r#"…"#` / `b"…"`），三件事由 `--selftest` 钉住。
"""
import pathlib
import re
import sys

ROOT_DIR = pathlib.Path(__file__).resolve().parent.parent  # <main>/
SRC = ROOT_DIR / "src"
DOC = ROOT_DIR / "目录结构说明.md"

LAYERS = ("core", "platform", "decoders", "extensions", "ui", "runtime")
BEGIN = "<!-- GEN:SYMBOLS:BEGIN 本区间由 build/structure_gen.py 生成，勿手改 -->"
END = "<!-- GEN:SYMBOLS:END -->"

CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
VIS = r"(?P<vis>pub(?:\([^)]*\))?\s+)?"
# fn 声明必须带名字，所以 `field: unsafe extern "C" fn(..)` 这类函数字段类型天然不匹配。
FN_RE = re.compile(
    r"^" + VIS + r"(?P<mods>(?:const\s+|async\s+|unsafe\s+|extern\s+\"[^\"]*\"\s+)*)fn\s+(?P<name>[A-Za-z_]\w*)"
)
TYPE_RE = re.compile(
    r"^" + VIS + r"(?P<kind>struct|enum|trait|union|type)\s+(?P<name>[A-Za-z_]\w*)"
)
# 顶层 static / const（只在模块级收，函数体内的局部 const 不进索引）。
DATA_RE = re.compile(
    r"^" + VIS + r"(?P<kind>static|const)\s+(?P<name>[A-Za-z_]\w*)\s*[:=]"
)
IMPL_RE = re.compile(r"^impl(?:\s*<[^>]*>)?\s*(?P<rest>[^{;]*)")
MOD_BLOCK = re.compile(r"^pub(?:\([^)]*\))?\s*mod\s+(?P<name>[A-Za-z_]\w*)\s*\{")
ATTRIBUTE_LINE = re.compile(r"^\s*#\[")
DOC_LINE = re.compile(r"^\s*///\s?(?P<text>.*)$")
# 字符串前缀：b"…" / r"…" / r#"…"#（u 前缀是 Unicode 字符串，本项目不用）。
STR_PREFIX = re.compile(r'(?P<kind>b|r)?(?P<hashes>#+)"')

SUMMARY_MAX = 78


def line_count(path: pathlib.Path) -> int:
    """`wc -l` 口径：数 `\n`。"""
    return path.read_bytes().count(b"\n")


def fmt(n: int) -> str:
    return f"{n:,}"


def brace_delta(line: str):
    """返回 (opens, closes)：只数代码，跳过 `//` `/* */` 与字符串/字符字面量。"""
    opens = closes = 0
    i, n = 0, len(line)
    while i < n:
        c = line[i]
        if c == "/" and i + 1 < n and line[i + 1] in "/*":
            if line[i + 1] == "/":
                break
            j = line.find("*/", i + 2)
            i = n if j < 0 else j + 2
            continue
        if c in "br":
            m = STR_PREFIX.match(line, i)
            if m:
                close = '"' + (m.group("hashes") or "")
                # 非 raw（无 # 的 b"…" 与普通 "…"）才有反斜杠转义
                if m.group("hashes") or m.group("kind") == "r":
                    j = line.find(close, i + m.end() - i)
                    i = n if j < 0 else j + len(close)
                else:
                    i = _skip_escaped(line, i + 1)
                continue
        if c == '"':
            i = _skip_escaped(line, i + 1)
            continue
        if c == "'":
            # 字符字面量 `'{'` 才需要跳过；生命周期 `'a` 后面没有闭合引号，跳过即可。
            j = line.find("'", i + 1)
            i = (j + 1) if 0 < j <= i + 4 else i + 1
            continue
        if c == "{":
            opens += 1
        elif c == "}":
            closes += 1
        i += 1
    return opens, closes


def _skip_escaped(line: str, i: int) -> int:
    """从开引号之后走到闭引号之后，处理反斜杠转义；未闭合则返回行尾。"""
    n = len(line)
    while i < n:
        if line[i] == "\\":
            i += 2
            continue
        if line[i] == '"':
            return i + 1
        i += 1
    return n


def summary_above(lines: list, idx: int) -> str:
    """取声明紧邻上方的 `///` 文档注释首句；跳过属性行与空行。

    `/// # Safety` 这类 doc 小标题不算摘要——它后面那句才是读者要的内容，
    而索引里出现「# Safety」等于没说话。
    """
    i = idx - 1
    found = []
    while i >= 0:
        s = lines[i].strip()
        if not s or ATTRIBUTE_LINE.match(s):
            i -= 1
            continue
        m = DOC_LINE.match(s)
        if not m:
            break
        found.append(m.group("text").strip())
        i -= 1
    if not found:
        return ""
    # found 自下而上收集：从最上面一行开始找第一条有正文、且不是 `#` 小标题的行
    top = next((t for t in reversed(found) if t and not t.startswith("#")), "")
    for sep in ("——", "。", "；", "; "):
        cut = top.find(sep)
        if cut > 0:
            top = top[:cut]
            break
    return top.strip()[:SUMMARY_MAX].replace("|", "\\|")


def item_label(stripped: str):
    """本行是否为被索引项；返回 `(kind_label, name, opens_block)` 或 None。

    `kind_label` 保留 `pub` / `pub(crate)` 与 `unsafe`——93 处 unsafe 是本仓的风险面，
    索引里把它们标出来，检索「这条边界谁负责」才有一跳可达的入口。
    """
    m = FN_RE.match(stripped)
    if m:
        vis = (m.group("vis") or "").strip()
        unsafe = "unsafe" in (m.group("mods") or "")
        kind = " ".join(x for x in (vis, "unsafe" if unsafe else "", "fn") if x)
        return kind, m.group("name"), True
    m = TYPE_RE.match(stripped)
    if m:
        kind = f"pub {m.group('kind')}" if m.group("vis") else m.group("kind")
        return kind, m.group("name"), False
    m = DATA_RE.match(stripped)
    if m:
        kind = f"pub {m.group('kind')}" if m.group("vis") else m.group("kind")
        return kind, m.group("name"), False
    return None


def extract_symbols(text: str):
    """返回 `[(标签, 行号, 摘要)]`。标签如 `pub fn Host::try_decode` / `struct Decoded`。"""
    lines = text.splitlines()
    items = []
    frames = []  # 作用域栈：kind ∈ impl / trait / mod / type / fn / block
    for idx, raw in enumerate(lines, start=1):
        if CFG_TEST.match(raw):
            break
        stripped = raw.strip()
        if not stripped or stripped.startswith("//"):
            continue
        top = frames[-1] if frames else None
        container = top if (top and top["kind"] in ("impl", "trait", "mod")) else None
        if top is None or container is not None:
            got = item_label(stripped)
            if got:
                kind, name, _ = got
                label = f"{kind} {name}"
                if container and container["kind"] in ("impl", "trait"):
                    label = f"{kind} {container['name']}::{name}"
                items.append((label, idx, summary_above(lines, idx - 1)))

        opens, closes = brace_delta(raw)
        if opens == closes:
            continue
        if opens > closes:
            frames.append(_frame_for(stripped))
        else:
            for _ in range(closes - opens):
                if frames:
                    frames.pop()
    return items


def _frame_for(stripped: str) -> dict:
    m = IMPL_RE.match(stripped)
    if m and not stripped.startswith("type"):
        rest = m.group("rest").strip()
        target = rest.split(" for ")[-1].strip() if " for " in rest else rest
        return {"kind": "impl", "name": target.split("<")[0].strip().split(" ")[-1] or "?"}
    m = MOD_BLOCK.match(stripped)
    if m:
        return {"kind": "mod", "name": m.group("name")}
    m = TYPE_RE.match(stripped)
    if m:
        return {"kind": m.group("kind"), "name": m.group("name")}
    m = FN_RE.match(stripped)
    if m:
        return {"kind": "fn", "name": m.group("name")}
    return {"kind": "block", "name": ""}


def scan_src():
    """`{层: {相对 posix 路径: 行数}}`，含 root 一桶（`src/*.rs`）。"""
    buckets = {k: {} for k in LAYERS}
    buckets["root"] = {}
    for p in sorted(SRC.rglob("*.rs")):
        rel = p.relative_to(SRC).as_posix()
        head = rel.split("/")[0]
        buckets[head if head in LAYERS else "root"][rel] = line_count(p)
    return buckets


def bucket_key(layer: str, rel_name: str) -> str:
    return rel_name if layer == "root" else f"{layer}/{rel_name}"


# ---------------------------------------------------------------- 文档渲染


def symbol_rows(buckets) -> list:
    rows = []
    for layer in LAYERS + ("root",):
        for rel in sorted(buckets[layer]):
            text = (SRC / rel).read_text(encoding="utf-8")
            for label, lineno, summary in extract_symbols(text):
                rows.append((rel, lineno, label, summary))
    return rows


def render_symbol_table(rows) -> list:
    out = [BEGIN, "| 文件 | 行 | 项 | 摘要（`///` 首句） |", "|---|---|---|---|"]
    for rel, lineno, label, summary in rows:
        out.append(f"| {rel} | {lineno} | `{label}` | {summary} |")
    out.append(f"| **合计** | | **{len(rows)} 项** | |")
    out.append(END)
    return out


def set_right_cells(line: str, values: dict) -> str:
    """按「倒数第 k 格」改表格单元；列数不符（散文里混了竖线）则原样返回。"""
    parts = line.split("|")
    for pos, val in values.items():
        if abs(pos) >= len(parts):
            return line
        parts[pos] = f" {val} "
    return "|".join(parts)


def dir_counts() -> dict:
    """§2.3 那两张非 src 目录的计数（唯一写入者就是本脚本）。

    为什么也归生成器管：`examples/` 手写的那一格停在 33，磁盘已经 40 —— 与 K157 同一族病
    （多处描述同一事实，只有总数那维被机器读过）。数的是**文件**，`__pycache__` 之类
    目录不算（它们本来就不该在仓里，混进来会把计数变成噪声）。
    """
    out = {}
    for name, pattern in (("examples", "*.rs"), ("build", "*")):
        d = ROOT_DIR / name
        out[name] = sum(1 for p in d.iterdir() if p.is_file() and p.match(pattern)) if d.is_dir() else 0
    return out


def update_numbers(text: str, buckets) -> str:
    counts = dir_counts()
    totals = {k: (len(v), sum(v.values())) for k, v in buckets.items()}
    grand_files = sum(c for c, _ in totals.values())
    grand_lines = sum(n for _, n in totals.values())
    lines = text.splitlines()
    cur = None  # 当前 3.x 小节所属层
    for i, line in enumerate(lines):
        s = line.strip()
        m = re.match(r"^\|\s*`?(core|platform|decoders|extensions|ui|runtime)/?`?\s*\|", s)
        if m:
            cnt, num = totals[m.group(1)]
            lines[i] = set_right_cells(line, {-3: str(cnt), -2: fmt(num)})
            continue
        if s.startswith("| **合计** |") and "项" not in s:
            lines[i] = set_right_cells(line, {-3: str(grand_files), -2: fmt(grand_lines)})
            continue
        # 第一节表里的「根文件」行：它不带反引号（`src/lib.rs` / `main.rs` 不是目录），
        # 上面那条 `| \`core/\` |` 的正则够不着它——曾经因此变成手写值，改过 src 根文件
        # 后它一直不更新，是 j9（独立走一遍磁盘）把它抓出来的。口径与其余各行一致。
        if s.startswith("| 根文件 |"):
            cnt, num = totals["root"]
            lines[i] = set_right_cells(line, {-3: str(cnt), -2: fmt(num)})
            continue
        if "# 应 =" in line:
            lines[i] = re.sub(r"# 应 =.*$", f"# 应 = {grand_files}", line)
            continue
        # §2.3「非 src 子目录一览」里的 `examples/`（N） / `build/`（N）：同样是产物，不是手写事实。
        m4 = re.match(r"^\|\s*`(examples|build)/`（(\d+)）", s)
        if m4:
            lines[i] = re.sub(
                r"（\d+）", f"（{counts[m4.group(1)]}）", line, count=1
            )
            continue
        if s.startswith("### 3."):
            m2 = re.search(r"`([a-z]+)/`", s)
            cur = m2.group(1) if (m2 and m2.group(1) in LAYERS) else ("root" if "根文件" in s else None)
            if cur:
                cnt, num = totals[cur]
                lines[i] = re.sub(
                    r"\d+ 文件 / [\d,]+ 行", f"{cnt} 文件 / {fmt(num)} 行", line
                )
            continue
        if s.startswith("1. **计数相等**"):
            seq = " + ".join(str(totals[k][0]) for k in LAYERS) + f" + {totals['root'][0]}"
            lines[i] = re.sub(r"：[\d +]+ = \*\*\d+", f"：{seq} = **{grand_files}", line)
            continue
        if s.startswith("## 三、"):
            lines[i] = re.sub(r"（\d+ 个", f"（{grand_files} 个", line)
            continue
        if cur and s.startswith("| `"):
            m3 = re.match(r"^\|\s*`([A-Za-z_0-9]+\.rs)`\s*\|", s)
            if m3:
                val = buckets[cur].get(bucket_key(cur, m3.group(1)))
                if val is not None:
                    parts = line.split("|")
                    lines[i] = "|".join([parts[0], parts[1], f" {fmt(val)} "] + parts[3:])
    return "\n".join(lines) + ("\n" if text.endswith("\n") else "")


def build(doc_text: str, buckets) -> str:
    out = update_numbers(doc_text, buckets)
    lines = out.splitlines()
    if BEGIN not in lines or END not in lines:
        raise SystemExit(
            "[ERROR] 文档里没有符号索引区间标记，无法生成。\n"
            f"        请在 {DOC.name} 第五节放上一对空标记后重跑：\n"
            f"        {BEGIN}\n        {END}"
        )
    lines[lines.index(BEGIN) : lines.index(END) + 1] = render_symbol_table(
        symbol_rows(buckets)
    )
    return "\n".join(lines) + ("\n" if doc_text.endswith("\n") else "")


# ---------------------------------------------------------------- 自检


def selftest() -> int:
    cases = [
        (
            "impl Host {\n    pub fn try_decode(&self) -> u32 {\n"
            "        let m = match x { A{} => 1 };\n        0\n    }\n}\n",
            ["pub fn Host::try_decode"],
            ["fn A", "pub struct"],
        ),
        (
            "pub struct Decoded {\n    pub width: u32,\n}\nfn helper(a: &str) {}\n"
            "#[cfg(test)]\nmod tests {\n    fn hidden_test() {}\n}\n",
            ["pub struct Decoded", "fn helper"],
            ["hidden_test", "width"],
        ),
        (
            'fn f() { let s = format!("{}", 1); let c = \'}\'; }\nfn g() {}\n'
            "const DEFAULT_INI: &str = \"x\";\n",
            ["fn f", "fn g", "const DEFAULT_INI"],
            ["pub fn f", "DEFAULT_INI::"],
        ),
        (
            "trait T {\n    fn call(&self);\n}\nfn outer() {\n    fn nested() {}\n}\n",
            ["fn T::call", "fn outer"],
            ["nested"],
        ),
        (
            "pub(crate) unsafe fn raw(&self) {\n    if x {\n        y();\n    }\n}\n"
            "pub fn after(&self) {}\n",
            ["pub(crate) unsafe fn raw", "pub fn after"],
            ["unsafe fn block::"],
        ),
        ('fn b() { let r = r"{ not a brace"; }\nfn c() {}\n', ["fn b", "fn c"], []),
        (
            "impl E {\n    pub const ALL: [E; 2] = [A, B];\n    fn v(self) {}\n}\n",
            ["pub const E::ALL", "fn E::v"],
            ["pub const ALL"],
        ),
    ]
    bad = 0
    for src, must, must_not in cases:
        got = " | ".join(f"{lb}@{ln}" for lb, ln, _ in extract_symbols(src))
        for token in must:
            if token not in got:
                print(f" [x] selftest 漏收 {token!r} -> {got}")
                bad += 1
        for token in must_not:
            if token in got:
                print(f" [x] selftest 多收 {token!r} -> {got}")
                bad += 1
    # 摘要必须取 /// 最上句、砍到第一个句号，且属性行不打断回看。
    src = "#[inline]\n/// 装解码结果 —— 细节。\n/// 第二行。\nfn f() {}\n"
    got = extract_symbols(src)
    if len(got) != 1 or got[0][2] != "装解码结果":
        print(f" [x] selftest 摘要取错 -> {got}")
        bad += 1
    # `/// # Safety` 是 doc 小标题，不能当成摘要。
    src = "/// # Safety\n/// 调用方须保证 ptr 有效\nunsafe fn g(p: *const u8) {}\n"
    got = extract_symbols(src)
    if len(got) != 1 or got[0][2] != "调用方须保证 ptr 有效":
        print(f" [x] selftest 摘要吃了 `#` 小标题 -> {got}")
        bad += 1
    # §2.3 的目录计数也归生成器管：手写那一格曾停在 33 而磁盘已经 40（K157 同族病）。
    c = dir_counts()
    sample = "| `examples/`（33） | 性能实测 |\n| `build/`（19） | 六步 `.bat` |\n"
    got = update_numbers(sample, {k: {} for k in LAYERS} | {"root": {}})
    for name in ("examples", "build"):
        if f"（{c[name]}）" not in got:
            print(f" [x] selftest §2.3 计数没刷：{name} 期望（{c[name]}）-> {got!r}")
            bad += 1
    print("structure_gen selftest:", "ok" if bad == 0 else f"{bad} 处失败")
    return 0 if bad == 0 else 1


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--selftest" in sys.argv:
        return selftest()
    if not DOC.is_file():
        print(f"[ERROR] 找不到结构文档: {DOC}")
        return 1
    buckets = scan_src()
    original = DOC.read_text(encoding="utf-8")
    rebuilt = build(original, buckets)
    if rebuilt == original:
        files = sum(len(v) for v in buckets.values())
        rows = len(symbol_rows(buckets))
        print(f"structure doc in sync with src（{files} 个 .rs / 符号索引 {rows} 项）")
        return 0
    if "--write" in sys.argv:
        # newline="\n" 是硬要求：Windows 上 `Path.write_text` 默认把 `\n` 翻成 `\r\n`，
        # 而 `.gitattributes` 声明 `*.md text eol=lf` ⇒ 每次重生成都会在提交时冒
        # "CRLF will be replaced by LF" 警告，且工作区与索引长期不一致（m283 修）。
        with DOC.open("w", encoding="utf-8", newline="\n") as f:
            f.write(rebuilt)
        print("structure doc refreshed ->", DOC.name)
        return 0
    old, new = original.splitlines(), rebuilt.splitlines()
    diff = [
        (i + 1, a, b)
        for i, (a, b) in enumerate(zip(old, new))
        if a != b
    ]
    print("structure doc DRIFTED -- run: python build/structure_gen.py --write")
    for no, a, b in diff[:20]:
        print(f"  L{no}\n    文档: {a[:120]}\n    应为: {b[:120]}")
    if len(old) != len(new):
        print(f"  行数: 文档 {len(old)} vs 应为 {len(new)}")
    print(f"drift lines: {len(diff)}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
