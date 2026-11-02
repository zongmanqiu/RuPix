"""分层依赖体检（只读）——执行 LAYER_ORDER / BARRED 定义的分层约束。

用法：
  python build/layering_scan.py            # 常规输出：分类计数 + 违规清单 + 汇总
  python build/layering_scan.py --json     # 机器可读：分类计数 + 明细 JSON
  python build/layering_scan.py --selftest # 规则自检（合成样例命中验证，门禁的牙齿）

分层（低 → 高）：core → platform → {decoders, extensions} → ui → runtime
规则：
  1. 高层可知道低层，低层**不得**知道高层；检查 use 树及正文 crate/self/super 显式路径。
  2. 禁引外部 crate 的层（可测性红线）：
       core     禁 windui / windows   —— 纯逻辑，单测必须能无头跑
       platform 禁 windui            —— 只做 Win32 薄封装，不碰 UI 框架
       decoders  禁 windui / windows  —— Win32 一律走 platform
       extensions 禁 windui / windows —— Win32 一律走 platform
       ui       禁 windows           —— 平台 API 只能经 platform
  3. 同层禁互引：decoders 与 extensions 平级，但互不 import（解码器不认扩展，扩展不认解码器）。
  4. 有 BARRED 规定的层同时检查 use 与正文外部路径；注释/字符串消隐。
     src 根装配文件无层且无 BARRED 规定，不判禁引（main 合法使用 windui）。
     按常规文件布局及内联 mod 解析相对路径；不解析别名/重导出、宏展开、cfg
     或 #[path] 重定位，不声称覆盖所有 Rust 引用。

分类计数：违规按 `direction` / `barred_use` / `barred_path` 三档分别报数（见
[`CATEGORIES`]）。**判定门限仍是「总数 = 0」**——分类只为人眼读日志时一眼看到
"哪一类在动"（历史某类回归、新增裸路径等），不引入基线冻结/告警降级那一套：
RuPix 违规历史 = 0，加基线文件等于给满分项目开小抄通道。
退出码：0 = 「违规 0 处」；1 = 有违规。

本脚本只做静态字符串扫描，不引第三方正则库。
"""
import json
import pathlib
import re
import sys

# 层名 → 层序（数字越大越高层）。与《目录结构说明.md》分层图一一对应。
LAYER_ORDER = {
    "core": 0,
    "platform": 1,
    "decoders": 2,
    "extensions": 2,
    "ui": 3,
    "runtime": 4,
}

# 违规分类的**固定顺序**，用于打印表头与 `--json` 里的 `by_category` 键序。
# 三档划分与上面的规则编号一一对应：方向违规（规则 1）、禁引 use 形态（规则 2）、
# 禁引裸路径形态（规则 3 里"补 use-only 漏"那一支）。新增分类请**尾追**——
# 头插会让旧日志的三段布局错位，肉眼 diff 反而更难读。
CATEGORIES = ("direction", "barred_use", "barred_path")

ROOT_DIR = pathlib.Path(__file__).resolve().parent.parent  # <main>/
SRC = ROOT_DIR / "src"

# 层 → 该层禁引的外部 crate。
BARRED = {
    "core": {"windui", "windows"},
    "platform": {"windui"},
    "decoders": {"windui", "windows"},
    "extensions": {"windui", "windows"},
    "ui": {"windows"},
}

# 同层禁互引：解码器与扩展是两条独立商品线，任何一方 import 另一方都说明边界被
# 抄近路绕过了（扩展不许参与解码路由，解码器也不许调用扩展）。
FORBIDDEN_PEERS = {("decoders", "extensions"), ("extensions", "decoders")}


def file_layer(rel: str):
    """取 `src/<X>/.../f.rs` 的第一个路径段 → 层序。src 根下文件返回 None（不判）。"""
    parts = rel.replace("\\", "/").split("/")
    if not parts:
        return None
    return LAYER_ORDER.get(parts[0])


def iter_rs():
    for p in sorted(SRC.rglob("*.rs")):
        yield p


# 只词法消隐，不解释 cfg / 宏展开 / 别名重导出 / #[path] 模块重定位。
# 保留换行与字符位置，诊断仍指向原源码；字符字面量不能吞掉 'a 生命周期。
LEXEME = re.compile(
    r'//[^\n]*|/\*|(?:br|cr|r)(?P<hashes>\#*)"|'
    r'''(?:b|c)?"(?:\\[\s\S]|[^"\\])*"|'''
    r"(?:b)?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|[^\n])|[^'\\\n])'"
)
TOKEN = re.compile(r"r#[A-Za-z_][A-Za-z_0-9]*|[A-Za-z_][A-Za-z_0-9]*|::|[^\s]")


def code_only(body):
    out = list(body)
    pos = 0
    while match := LEXEME.search(body, pos):
        start, end = match.span()
        if match.group() == "/*":
            depth = 1
            while depth and end < len(body):
                if body.startswith("/*", end):
                    depth += 1
                    end += 2
                elif body.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
        elif match.group("hashes") is not None:
            closer = '"' + match.group("hashes")
            close = body.find(closer, end)
            end = len(body) if close < 0 else close + len(closer)
        for i in range(start, end):
            if out[i] != "\n":
                out[i] = " "
        pos = end
    return "".join(out)


def module_path(rel):
    path = pathlib.PurePosixPath(rel.replace("\\", "/"))
    parts = list(path.parts[:-1])
    if path.stem not in ("", "mod") and (parts or path.stem not in ("lib", "main")):
        parts.append(path.stem)
    return parts


def path_records(body, rel=""):
    """返回 (路径段, 行, use或path, 所在模块)。只跟踪模块，不把函数块当模块。"""
    clean = code_only(body)
    tokens = []
    line, previous = 1, 0
    for match in TOKEN.finditer(clean):
        line += clean.count("\n", previous, match.start())
        tokens.append((match.group().removeprefix("r#"), line))
        previous = match.start()
    words = [t[0] for t in tokens]
    records = []
    scope = module_path(rel)
    stack = []

    def use_tree(i, prefix):
        start = i
        path = list(prefix)
        if i < len(words) and words[i] == "::":
            i += 1
        while i < len(words) and (words[i].isidentifier() or words[i] == "*"):
            path.append(words[i])
            i += 1
            if i >= len(words) or words[i] != "::":
                break
            i += 1
        if i < len(words) and words[i] == "{":
            i += 1
            while i < len(words) and words[i] not in ("}", ";"):
                if words[i] == ",":
                    i += 1
                else:
                    nxt = use_tree(i, path)
                    i = max(i + 1, nxt)  # 不合法/宏 token 也必须推进。
            if i < len(words) and words[i] == "}":
                i += 1
        elif path:
            records.append((path, tokens[start][1], "use", list(scope)))
        if i < len(words) and words[i] == "as":
            i += 2  # 别名不是另一条路径。
        return i

    i = 0
    while i < len(words):
        word = words[i]
        if word == "use":
            i = use_tree(i + 1, [])
            continue
        if word == "{":
            stack.append(list(scope))
            if i >= 2 and words[i - 2] == "mod" and words[i - 1].isidentifier():
                scope = scope + [words[i - 1]]
        elif word == "}" and stack:
            scope = stack.pop()
        elif word.isidentifier() and i + 1 < len(words) and words[i + 1] == "::":
            start = i
            path = [word]
            while i + 2 < len(words) and words[i + 1] == "::" and words[i + 2].isidentifier():
                path.append(words[i + 2])
                i += 2
            records.append((path, tokens[start][1], "path", list(scope)))
        i += 1
    return records


def internal_target(path, scope):
    if not path or path[0] not in ("crate", "self", "super"):
        return None
    resolved = [] if path[0] == "crate" else list(scope)
    rest = path[1:] if path[0] in ("crate", "self") else path
    for part in rest:
        if part == "super":
            if not resolved:
                return None  # 已越过 crate 根，不猜无效 Rust 的目标。
            resolved.pop()
        elif part != "self":
            resolved.append(part)
    return resolved[0] if resolved else None


def scan_uses(body: str, rel: str = ""):
    """兼容原自检接口：现在同时返回 use 树与正文显式相对/绝对路径的目标层。"""
    return [target for path, _, _, scope in path_records(body, rel)
            if (target := internal_target(path, scope)) is not None]


def scan_bare_crate(body: str, crate: str):
    return sorted({ln for path, ln, kind, _ in path_records(body)
                   if kind == "path" and path[0] == crate})


def scan_file(body, rel):
    """分类及 BARRED 沿用；根装配文件没有层，也没有另设禁引表。"""
    fl = file_layer(rel)
    if fl is None:
        return []
    fl_name = rel.replace("\\", "/").split("/")[0]
    violations = []
    seen = set()
    for path, line, kind, scope in path_records(body, rel):
        target = internal_target(path, scope)
        peer = (fl_name, target) in FORBIDDEN_PEERS
        if peer or LAYER_ORDER.get(target, -1) > fl:
            reason = "平级禁互引" if peer else "低层不得引用高层"
            category = "direction"
            message = f"{fl_name} → {target} {reason}"
        elif path[0] in BARRED.get(fl_name, set()):
            category = "barred_use" if kind == "use" else "barred_path"
            message = f"{fl_name} 层禁引外部 crate `{path[0]}`（{kind} 形态）"
        else:
            continue
        key = (category, line, message)
        if key not in seen:
            seen.add(key)
            violations.append((category, f"src/{rel}:{line}: `{'::'.join(path)}` —— {message}"))
    return violations


def selftest():
    """自检：合成样例必须被方向/禁引规则正确命中（tests/docs_consistency.rs 判据⑦）。"""
    fails = []

    def expect(cond, msg):
        if not cond:
            fails.append(msg)

    expect(LAYER_ORDER["ui"] > LAYER_ORDER["core"], "层序 ui > core")
    expect(
        LAYER_ORDER["extensions"] == LAYER_ORDER["decoders"],
        "extensions 与 decoders 平级",
    )
    expect(
        ("decoders", "extensions") in FORBIDDEN_PEERS
        and ("extensions", "decoders") in FORBIDDEN_PEERS,
        "decoders/extensions 必须双向禁互引",
    )
    expect("ui" in scan_uses("use crate::ui::canvas;\n"), "scan_uses 应抓 use crate::ui")
    expect("ui" in scan_uses("use crate::{core::x, ui::y};\n"), "scan_uses 应抓花括号形态")
    expect(scan_uses("// use crate::ui::canvas;\n").count("ui") == 0, "注释 use 不应计入")

    expect(
        [c for c, _ in scan_file("use windui::Window;\n", "core/mod.rs")] == ["barred_use"],
        "core 禁引应命中 use windui::",
    )
    expect(scan_bare_crate("let img = windui::Image::new();\n", "windui") == [1], "裸路径 windui:: 应报第 1 行")
    expect(scan_bare_crate("// windui::Image\n", "windui") == [], "注释行应豁免")
    expect(scan_bare_crate("use windui::Window;\n", "windui") == [], "use 行不重复报裸路径")

    bad_paths = [
        "pub use crate::{\n core::{x, y},\n ui::{canvas, brand},\n};",
        "fn f() { crate::ui::canvas(); }",
        "use super::super::ui::{canvas, brand};",
        "fn f() { super::super::ui::canvas(); }",
        "mod nested { use super::super::super::ui::canvas; }",
        "use {crate::{core::x, ui::{self, canvas}}, crate::runtime::run};",
    ]
    for body in bad_paths:
        expect("ui" in scan_uses(body, "core/sample.rs"), f"方向漏报: {body}")
    for body in [
        "/* use crate::ui::canvas; /* nested */ crate::ui::x(); */",
        'let s = "use crate::ui::canvas; windui::Image";',
        'let s = br###"crate::ui::x(); windui::Image"###;',
        "use self::ui::x; use super::ui::x;",
        "mod nested { use super::super::ui::x; }",
        "use crate::core::{ui::{self, x}, y};",
    ]:
        expect("ui" not in scan_uses(body, "core/sample.rs"), f"方向误报: {body}")
        expect(scan_bare_crate(body, "windui") == [], f"字符串/注释误报: {body}")
    expect(scan_bare_crate("pub use windui::{\n Image, Window\n};", "windui") == [],
           "多行 pub use 不重复报裸路径")
    expect(scan_bare_crate("crate::core::windui::x();", "windui") == [],
           "本地同名模块不应被当成外部 crate")
    for body in bad_paths:
        hits = scan_file(body, "core/sample.rs")
        expect(bool(hits) and all(c == "direction" for c, _ in hits),
               f"真实方向判定必须拒绝: {body}")
    for rel, body, categories in [
        ("decoders/mod.rs", "use crate::extensions::x;", ["direction"]),
        ("extensions/mod.rs", "super::decoders::x();", ["direction"]),
        ("core/mod.rs", "use super::{ui::{self as u, canvas}};", ["direction"]),
        ("core/mod.rs", "pub use {windui::{Image, Window}, windows as win};",
         ["barred_use", "barred_use"]),
        ("ui/mod.rs", "fn f() { ::windows::x(); }", ["barred_path"]),
        ("core/mod.rs", "// x\nfn f() { /* inline */ windui::x(); }", ["barred_path"]),
        ("runtime/mod.rs", "use crate::{ui::x, core::y};", []),
        ("ui/mod.rs", "use windui::Window;", []),
        ("main.rs", "windui::warm(); use crate::ui::x;", []),
        ("lib.rs", "use crate::runtime::run;", []),
        ("core/main.rs", "super::ui::x();", []),
        ("core/lib.rs", "super::ui::x();", []),
        ("core/mod.rs", 'let s = r##"use windows::X; crate::ui::x"##;', []),
        ("core/mod.rs", 'let s = "escaped \\\" windui::x"; /* windows::X */', []),
        ("core/mod.rs", "fn f<'a>() { let x = 'a'; crate::ui::x(); }", ["direction"]),
        ("core/mod.rs", "use notwindui::X; use crate::core::windows::X;", []),
        ("core/mod.rs", "mod a { mod b { fn f() { super::super::super::ui::x(); } } }",
         ["direction"]),
    ]:
        hits = scan_file(body, rel)
        expect([c for c, _ in hits] == categories, f"分类/作用域: {rel}: {body}: {hits}")
    hits = scan_file("/* comment\n nested /* x */ */\nwindui::x();", "core/mod.rs")
    expect(len(hits) == 1 and ":3:" in hits[0][1], "消隐后必须保留原行号")

    if fails:
        for f in fails:
            print(" [x] selftest: " + f)
        print(f"\nselftest failures: {len(fails)}")
        return 1
    print("selftest: ok")
    return 0


def main():
    # 违规按分类装桶：每条形如 `(category, message)`——分类用来计数，message 用来打印。
    # 用 list 而不是 dict-of-list 是因为**输出顺序**要按文件遍历顺序稳定（`iter_rs` 已 sorted），
    # 分类计数从这一条列表里再聚合，避免"三桶各自有序、拼起来跨桶乱序"。
    violations = []
    for p in iter_rs():
        rel = p.relative_to(SRC).as_posix()
        body = p.read_text(encoding="utf-8", errors="replace")
        violations.extend(scan_file(body, rel))

    # 分类计数：一次遍历聚合，`CATEGORIES` 里没有的分类不会出现（`main` 里只 append 三档）。
    by_cat = {c: 0 for c in CATEGORIES}
    for cat, _ in violations:
        by_cat[cat] += 1
    total = len(violations)

    if "--json" in sys.argv:
        print(
            json.dumps(
                {
                    "by_category": by_cat,
                    "count": total,
                    "violations": [msg for _, msg in violations],
                },
                ensure_ascii=False,
                indent=2,
            )
        )
        # `--json` 是机器读的路径，**判定不变**：总数非 0 仍返回 1（外部脚本能拿到退出码）。
        return 0 if total == 0 else 1

    # 人读输出：**先**打印分类计数（哪怕全 0），让趋势在日志里永远占一行；
    # 违规清单跟在后面（当前项目 = 空）。
    print("layering violations by category:")
    for c in CATEGORIES:
        print(f"  {c:<12}: {by_cat[c]}")
    if violations:
        print()
        for _, msg in violations:
            print(" [x] " + msg)
        print(f"\nviolations: {total}")
        return 1
    print("violations: 0")
    return 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    sys.exit(main())
