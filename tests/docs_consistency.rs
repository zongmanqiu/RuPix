//! 文档腐化门禁（CurSeen Phase 5 判据 ①–⑦ 的机器执行）。
//! 任何一条红 = 文档与代码脱节：修文档或同步更新登记，不许绕过本文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 判据⑥：main 根目录允许出现的 .md 白名单。新增根文档必须同步登记
/// 本白名单与 DOCS_MANIFEST.md。
const ROOT_MD_WHITELIST: &[&str] = &[
    "README.md",
    "AGENTS.md",
    "目录结构说明.md",
    "开发指南.md",
    "DOCS_MANIFEST.md",
    "HISTORY.md",
    "THIRD-PARTY-NOTICES.md",
];

/// 判据④：现行文档禁用的考古串。HISTORY.md（编年史，本职即记史）、
/// 待办.md（可能记录已收编项）与 `patches/` 补丁登记簿（只增不减，历史章节
/// 按设计引用已不存在的上游文件）豁免；历史内容归 `HISTORY.md`，现行文档只写现行约定。
const ARCHAEOLOGY: &[&str] = &["曾经", "历史上", "已删除", "已移除", "已废弃", "演进自"];

/// 判据⑤：篇幅上限（2026-10-11 用户裁定：不要小家子气，统一 5000 行）。
const MAX_DOC_LINES: usize = 5000;

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("读 {} 失败: {e}", p.display()))
}

fn walk_files(root: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_files(&p, ext, out);
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

fn walk_all_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// ① manifest 登记的文件必须真实存在（【待建】行豁免）。
#[test]
fn j1_manifest_registered_files_exist() {
    let manifest = main_dir().join("DOCS_MANIFEST.md");
    let text = read(&manifest);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.len() > 10,
        "DOCS_MANIFEST.md 不足 10 行（现 {}）",
        lines.len()
    );

    let mut missing = Vec::new();
    for line in lines {
        if line.contains("【待建】") {
            continue;
        }
        // 只看表格第一列（登记列）：行首 `| \`` 开头的单元格，防止第二列散文里的
        // 概念性回指（如「`PATCHES.md` 登记」）被当成真实登记项。
        let Some(rest) = line.trim_start().strip_prefix("| `") else {
            continue;
        };
        let Some(tok) = rest.split('`').next() else {
            continue;
        };
        if !tok.ends_with(".md") || tok.contains('*') {
            continue;
        }
        let rel = tok.trim_start_matches("../");
        // 登记簿只管 `main/` 内的文档：仓根或 `docs/` 之下任一位置命中即算存在。
        // ★ 以前这里还试 `main/` 的父目录，为的是能把第三方源码池里的 README 也登记进来——
        //   那等于让宿主文档体系去认领一份不在本仓、也不会随宿主一起克隆的文件。
        let candidates = [main_dir().join(rel), main_dir().join("docs").join(rel)];
        if !candidates.iter().any(|c| c.is_file()) {
            missing.push(tok.to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "DOCS_MANIFEST 登记但磁盘缺失: {missing:?}"
    );
}

// 第三节**按层分节**，行里的文件是相对该层的路径（`viewer/mod.rs` 是 core 层子目录）。
// 先按小节拼出完整相对路径再与磁盘比：只看 basename 时 `ui/mod.rs` 那一行会替
// `core/mod.rs` 充数，整层漏登记也照样绿——扩展层就是这么从 §一/§三 消失的。
fn structure_doc_paths(doc: &str) -> std::collections::BTreeSet<String> {
    const LAYERS: &[&str] = &[
        "core",
        "platform",
        "decoders",
        "extensions",
        "ui",
        "runtime",
    ];
    let mut out = std::collections::BTreeSet::new();
    let mut layer: Option<String> = None;
    for line in doc.lines() {
        let s = line.trim();
        if s.starts_with("### ") {
            layer = None;
            if let Some(name) = s.split('`').nth(1).map(|x| x.trim_end_matches('/')) {
                if LAYERS.contains(&name) {
                    layer = Some(name.to_string());
                }
            }
            continue;
        }
        let Some(rest) = s.strip_prefix("| `") else {
            continue;
        };
        let Some(tok) = rest.split('`').next() else {
            continue;
        };
        if !tok.ends_with(".rs") {
            continue;
        }
        out.insert(match &layer {
            Some(l) => format!("{l}/{tok}"),
            None => tok.to_string(),
        });
    }
    out
}

#[test]
fn j2_same_basename_does_not_cover_another_path() {
    let doc = "### 3.1 `core/`\n\n| `mod.rs` | 41 |\n| `viewer/mod.rs` | 387 |\n
               ### 3.5 `ui/`\n\n| `mod.rs` | 84 |\n";
    let got = structure_doc_paths(doc);
    assert!(
        got.contains("core/mod.rs"),
        "core 小节的 mod.rs 应拼成 core/mod.rs"
    );
    assert!(got.contains("core/viewer/mod.rs"), "层内子目录按层前缀拼");
    assert!(got.contains("ui/mod.rs"));
    assert!(
        !got.contains("decoders/mod.rs"),
        "没开小节的层不许被别处的同名行顶上"
    );
    // 散文里提到的 .rs 不算点名（只有表格首格反引号才是登记）。
    let prose = "| 见 `config_compat.rs` 的说法 | — |\n";
    assert!(
        !structure_doc_paths(prose).contains("config_compat.rs"),
        "首格不是纯路径时不该当成登记"
    );
}

/// ② 结构文档覆盖 src 全部 .rs 文件（按小节拼出的完整相对路径点名才算覆盖）。
#[test]
fn j2_structure_doc_covers_src() {
    let doc = read(&main_dir().join("目录结构说明.md"));
    let src = main_dir().join("src");
    let mut src_rs = Vec::new();
    walk_files(&src, "rs", &mut src_rs);
    assert!(!src_rs.is_empty(), "src 下没有 .rs？目录结构坏了");
    let listed = structure_doc_paths(&doc);
    let uncovered: Vec<String> = src_rs
        .iter()
        .filter_map(|p| {
            let rel = p
                .strip_prefix(&src)
                .expect("遍历结果必须位于 src 内")
                .to_string_lossy()
                .replace('\\', "/");
            (!listed.contains(&rel)).then_some(rel)
        })
        .collect();
    assert!(
        uncovered.is_empty(),
        "目录结构说明.md 未点名: {uncovered:?}"
    );
}

/// ③ 文档点名的 .rs 必须在仓内真实存在（防指向已删文件的悬空引用）。
/// patches/ 登记簿豁免——它引用的是上游 windui 文件，仓内无从校验。
#[test]
fn j3_docs_named_rs_exist() {
    let mut named = std::collections::BTreeSet::new();
    let mut mds = Vec::new();
    walk_files(&main_dir(), "md", &mut mds);
    for md in &mds {
        // 跳过快照/归档类目录下的 md（若存在）
        let rel = md.strip_prefix(main_dir()).unwrap_or(md).to_string_lossy();
        if rel.starts_with(".temp") || rel.starts_with(".bf") {
            continue;
        }
        // patches/ 是补丁登记簿（本职记史、只增不减）：历史章节按设计引用
        // 已不存在的上游文件（如 0.20.0 的 ui/containers.rs），与 HISTORY.md
        // 同类豁免；其引用的真伪由补丁生命周期（★ 标记 ↔ PATCHES.md）另行核对。
        if rel.replace('\\', "/").starts_with("patches/") {
            continue;
        }
        let text = read(md);
        for line in text.lines() {
            // 待建条目允许先立字据后补文件（如【待建】的测试）。
            if line.contains("待建") {
                continue;
            }
            let bytes = line.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
                {
                    i += 1;
                }
                if i > start
                    && i - start < 80
                    && bytes[start..i].ends_with(b".rs")
                    && bytes[start..i].len() > 4
                    && bytes[start].is_ascii_alphanumeric()
                {
                    named.insert(line[start..i].to_string());
                }
                if i == start {
                    i += 1;
                }
            }
        }
    }

    // 外部站点名豁免：j3 的本意是防「指向已删源文件的悬空引用」，而下面这些是
    // Rust 生态的**网站域名**，不是仓内文件——THIRD-PARTY-NOTICES.md 里的
    // 「docs.rs 实读」就是指该站点。扫描器按「`*.rs` token 必须在仓内存在同名
    // 文件」机械判定，会把域名误判成悬空引用。
    //
    // 边界（重要）：这里只放**确认是外部域名语义**的名字。绝不能用它掩盖真正的
    // 悬空源文件引用——若某个名字既可能是仓内文件又像域名，应当改文档措辞，
    // 而不是往这张表里塞。
    const EXTERNAL_SITE_NAMES: &[&str] = &["docs.rs", "crates.io", "rust-lang.org"];

    let mut existing = Vec::new();
    walk_files(&main_dir(), "rs", &mut existing);
    let names: std::collections::BTreeSet<String> = existing
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let dangling: Vec<&String> = named
        .iter()
        .filter(|n| !names.contains(*n))
        .filter(|n| !EXTERNAL_SITE_NAMES.contains(&n.as_str()))
        .collect();
    assert!(
        dangling.is_empty(),
        "文档点名但仓内不存在的 .rs: {dangling:?}"
    );
}

/// ④ 现行文档禁考古串（HISTORY.md / 待办.md 豁免）。
#[test]
fn j4_no_archaeology_in_current_docs() {
    let mut mds = Vec::new();
    walk_files(&main_dir(), "md", &mut mds);
    let mut hits = Vec::new();
    for md in &mds {
        let rel = md
            .strip_prefix(main_dir())
            .unwrap_or(md)
            .to_string_lossy()
            .into_owned();
        if rel.starts_with(".temp") || rel.starts_with(".bf") {
            continue;
        }
        let base = md.file_name().unwrap().to_string_lossy();
        if base == "HISTORY.md" || base == "待办.md" {
            continue;
        }
        // 补丁登记簿本职记史（只增不减）：迁移/删除记录是其登记内容本身。
        if rel.replace('\\', "/").starts_with("patches/") {
            continue;
        }
        let text = read(md);
        for word in ARCHAEOLOGY {
            if text.contains(word) {
                hits.push(format!("{rel} 命中「{word}」"));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "现行文档出现考古串（历史内容应移入 HISTORY.md）: {hits:?}"
    );
}

/// ⑤ 篇幅上限 5000 行。
#[test]
fn j5_doc_line_limits() {
    let mut targets: Vec<PathBuf> = ROOT_MD_WHITELIST
        .iter()
        .map(|f| main_dir().join(f))
        .collect();
    walk_files(&main_dir().join("docs"), "md", &mut targets);
    // ★ 本门禁只管 `main/` 自己的文档。以前这里还塞进解码器仓的 `AGENTS.md` 一起量行数，
    //   那是让宿主测试去读隔壁仓的文件：换了机器、或那仓没跟着克隆，这条判据就静默少一格
    //   （`read_to_string` 失败直接 `continue`）。各仓的文档上限归各仓自己的门禁。
    let mut over = Vec::new();
    for p in &targets {
        let Ok(text) = fs::read_to_string(p) else {
            continue;
        };
        let n = text.lines().count();
        if n > MAX_DOC_LINES {
            over.push(format!("{}: {n} 行", p.display()));
        }
    }
    assert!(over.is_empty(), "文档超过 {MAX_DOC_LINES} 行上限: {over:?}");
}

/// ⑥ 根 .md 白名单：main 根目录不得出现名单外的 .md。
#[test]
fn j6_root_md_whitelist() {
    let mut extra = Vec::new();
    let Ok(rd) = fs::read_dir(main_dir()) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() && p.extension().is_some_and(|x| x == "md") {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if !ROOT_MD_WHITELIST.contains(&name.as_str()) {
                extra.push(name);
            }
        }
    }
    assert!(
        extra.is_empty(),
        "main 根目录出现白名单外 .md（新增根文档须登记白名单+DOCS_MANIFEST）: {extra:?}"
    );
}

/// ⑦ 扫描器自检：layering_scan --selftest 必须绿（门禁自身的牙齿）。
#[test]
fn j7_layering_scanner_selftest() {
    let status = Command::new("python")
        .arg("build/layering_scan.py")
        .arg("--selftest")
        .current_dir(main_dir())
        .status()
        .expect("无法启动 python build/layering_scan.py");
    assert!(status.success(), "layering_scan --selftest 失败");
}

/// ⑧ 卫生体检自检：hygiene_scan --selftest 必须绿。四查（绝对路径 / 调试残留 /
/// TODO 真标记 / FFI 边界无解）规则改动都要合成样例证明"该命中的命中、
/// 不该命中的不命中"。
#[test]
fn j8_hygiene_scanner_selftest() {
    let status = Command::new("python")
        .arg("build/hygiene_scan.py")
        .arg("--selftest")
        .current_dir(main_dir())
        .status()
        .expect("无法启动 python build/hygiene_scan.py");
    assert!(status.success(), "hygiene_scan --selftest 失败");
}

/// ⑨ 结构文档第一节的计数，必须与 Rust **独立**走一遍 `src/` 的结果一致。
/// 交叉验证的意义：那些数字由 `structure_gen.py` 写进文档，若生成器自身算错
/// （行口径、分层归属、表格列位），j10 是它自己对自己，永远绿——只有第二个
/// 实现能抓出来。行数口径同样是 `\n` 计数（等价 `wc -l`）。
#[test]
fn j9_structure_doc_counts_match_disk() {
    // 独立统计：层目录名 → (文件数, 行数)；`src/*.rs` 归 "根文件"。
    let src = main_dir().join("src");
    let mut files = Vec::new();
    walk_files(&src, "rs", &mut files);
    let mut got: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    for p in &files {
        let rel = p.strip_prefix(&src).unwrap_or(p);
        let mut comps = rel.iter();
        let first = comps
            .next()
            .and_then(|c| c.to_str())
            .unwrap_or("")
            .to_string();
        let key = match comps.next() {
            // 只有一层路径 = `src/*.rs` = 根文件；否则按首段层名归桶。
            None => "根文件".to_string(),
            Some(_) => format!("{first}/"),
        };
        let n = fs::read(p)
            .map(|b| b.iter().filter(|c| **c == b'\n').count())
            .unwrap_or(0);
        let e = got.entry(key).or_insert((0, 0));
        e.0 += 1;
        e.1 += n;
    }
    let tf: usize = got.values().map(|v| v.0).sum();
    let tl: usize = got.values().map(|v| v.1).sum();

    let doc = read(&main_dir().join("目录结构说明.md"));
    let mut bad = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in doc.lines() {
        if line.starts_with("## 二、") {
            break; // 只看第一节总览表（符号索引里也有一行 **合计**，形状不同）
        }
        let cells: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        if cells.len() < 4 {
            continue;
        }
        let label = cells[1].trim_matches('`').trim_end_matches('/');
        let key = match label {
            "core" | "platform" | "decoders" | "extensions" | "ui" | "runtime" => {
                &format!("{label}/")
            }
            "根文件" => label,
            "**合计**" => {
                let (nf, nl) = (
                    parse_num(cells[cells.len() - 3]),
                    parse_num(cells[cells.len() - 2]),
                );
                if nf != tf || nl != tl {
                    bad.push(format!("合计行 {nf}/{nl} vs 磁盘 {tf}/{tl}"));
                }
                seen.insert("**合计**".to_string());
                continue;
            }
            _ => continue,
        };
        let Some((ef, el)) = got.get(key) else {
            continue;
        };
        let nf = parse_num(cells[cells.len() - 3]);
        let nl = parse_num(cells[cells.len() - 2]);
        if nf != *ef || nl != *el {
            bad.push(format!("{key} 文档 {nf}/{nl} vs 磁盘 {ef}/{el}"));
        }
        seen.insert(key.to_string());
    }
    let missing: Vec<&str> = [
        "core/",
        "platform/",
        "decoders/",
        "extensions/",
        "ui/",
        "runtime/",
        "根文件",
        "**合计**",
    ]
    .iter()
    .copied()
    .filter(|k| !seen.contains(*k))
    .collect();
    assert!(
        missing.is_empty() && bad.is_empty(),
        "结构文档第一节计数与磁盘不符（跑 `python build/structure_gen.py --write`）: \
         缺行 {missing:?} 错数 {bad:?}"
    );
}

fn parse_num(cell: &str) -> usize {
    cell.replace(',', "").trim().parse().unwrap_or(usize::MAX)
}

/// ⑩ 结构文档新鲜度：数字与第五节符号索引必须等于 `structure_gen.py` 的产物。
#[test]
fn j10_structure_doc_fresh() {
    let status = Command::new("python")
        .arg("build/structure_gen.py")
        .arg("--check")
        .current_dir(main_dir())
        .status()
        .expect("无法启动 python build/structure_gen.py");
    assert!(
        status.success(),
        "目录结构说明.md 与 src 脱节（数字或符号索引漂移）"
    );
}

/// ⑪ 生成器自检：structure_gen --selftest 必须绿。符号提取靠行级作用域栈而非完整
/// Rust 解析器，规则改动必须用合成样例证明归属与截断都对（否则索引会静默出错）。
#[test]
fn j11_structure_gen_selftest() {
    let status = Command::new("python")
        .arg("build/structure_gen.py")
        .arg("--selftest")
        .current_dir(main_dir())
        .status()
        .expect("无法启动 python build/structure_gen.py");
    assert!(status.success(), "structure_gen --selftest 失败");
}

/// ⑫ 体积数字只有一个正本：宪章 §4.1 的「当前 exe：」行必须等于刚构建出的 exe；
/// README 与待办**禁止**再写具体字节数或 MiB（它们只能指向账本）。
///
/// 取 exe 的顺序：先 `.temp/target/release`（流水线第 5 步测试时这是本轮刚构建的），
/// 再 `.build`（部署后的分发包）。两个都没有 = 明确失败——静默跳过会让这条门禁
/// 在别的机器上变成"0 用例照样退出 0"的幽灵条目（m39 的教训）。
#[test]
fn j12_exe_size_has_one_source() {
    let root = main_dir().parent().unwrap().to_path_buf();
    let exe = [
        root.join(".temp/target/release/rupix.exe"),
        root.join(".build/rupix.exe"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .unwrap_or_else(|| panic!("j12 找不到可核对的 exe（先构建宿主）"));
    let actual = fs::metadata(&exe)
        .unwrap_or_else(|e| panic!("读 {} 失败: {e}", exe.display()))
        .len();

    let ledger = read(&main_dir().join("docs/产品定位与边界.md"));
    let line = ledger
        .lines()
        .find(|l| l.contains("**当前 exe："))
        .expect("宪章 §4.1 缺「**当前 exe：**」机器锚，j12 无从核对");
    let rest = line.split("**当前 exe：").nth(1).expect("机器锚后半段缺失");
    let stated = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .collect::<String>()
        .replace(',', "");
    let stated: u64 = stated
        .parse()
        .unwrap_or_else(|_| panic!("「当前 exe：」后不是字节数：{line}"));
    assert_eq!(
        stated,
        actual,
        "宪章 §4.1 的当前体积 {stated} ≠ {} 实测 {actual}（跑 size_map 后同步那一行）",
        exe.display()
    );

    for name in ["README.md", "docs/待办.md"] {
        let text = read(&main_dir().join(name));
        let hits = size_figures(&text);
        assert!(
            hits.is_empty(),
            "{name} 不得写具体体积（只留宪章 §4.1 一处）: {hits:?}"
        );
    }
}

/// ⑬ `patches/` 是权威归档，`.3rd/` 是真编的工作树：归档里的每个 `src/**` 都要在工作树里
/// 存在且内容相同——**工作树缺文件也算漂移**（归档是升级时的重放依据，放行缺文件＝允许归档
/// 悄悄攒着一份谁也不编的文件）。只比归档侧存在的文件，不比反向新增。
/// 归档一旦与工作树分叉，PATCHES.md 就在描述一份不存在的代码，而编译照样过。
/// ★ m123 起这里是"归档 ↔ 工作副本"的**唯一定义点**：`build/hygiene_scan.py` 第 7 查刻意不重复它
///（同一判据立两个定义点＝改一个漏一个——`core/pixels.rs` 头注释记的那次 MAX_PIXELS 事故同形）。
#[test]
fn j13_patch_archive_matches_worktree() {
    let main = main_dir();
    let root = main.parent().expect("main 没有上级目录");
    let patches = main.join("patches");
    let third = root.join(".3rd");
    let mut libs = 0usize;
    let mut compared = 0usize;
    let mut drift = Vec::new();

    let entries =
        fs::read_dir(&patches).unwrap_or_else(|e| panic!("读 {} 失败: {e}", patches.display()));
    for e in entries.flatten() {
        let archived_lib = e.path();
        if !archived_lib.is_dir() {
            continue;
        }
        let Some(name) = archived_lib.file_name() else {
            continue;
        };
        let archived_src = archived_lib.join("src");
        if !archived_src.is_dir() {
            continue;
        }
        libs += 1;
        let work_src = third.join(name).join("src");
        let mut files = Vec::new();
        walk_all_files(&archived_src, &mut files);
        for archived in files {
            let rel = archived
                .strip_prefix(&archived_src)
                .expect("归档路径必在 src 之下");
            let work = work_src.join(rel);
            if !work.is_file() {
                drift.push(format!(
                    "{}/src/{}（工作树 .3rd 里不存在）",
                    name.to_string_lossy(),
                    rel.to_string_lossy().replace('\\', "/")
                ));
                continue;
            }
            compared += 1;
            let a = fs::read(&archived)
                .unwrap_or_else(|e| panic!("读 {} 失败: {e}", archived.display()));
            let w = fs::read(&work).unwrap_or_else(|e| panic!("读 {} 失败: {e}", work.display()));
            if a != w {
                drift.push(format!(
                    "{}/src/{}",
                    name.to_string_lossy(),
                    rel.to_string_lossy().replace('\\', "/")
                ));
            }
        }
    }

    println!("j13 compared {compared} file(s) across {libs} lib(s)");
    assert!(libs > 0, "patches/ 下没有任何带 src 的库，j13 无从比对");
    assert!(
        compared > 0,
        "j13 一个文件都没比上（patches ↔ .3rd 的路径映射坏了）"
    );
    assert!(
        drift.is_empty(),
        "patches 归档与 .3rd 工作树内容漂移（归档是权威，重放会丢功能）: {drift:?}"
    );
}

/// ⓮ 发布目录的自洽判据有钉子吗：`deploy.py --selftest` 必须绿。
///
/// 它钉的是两件事（正本 `build/deploy.py` 第 6 条）：随包 `docs/decoders.json` 与
/// `.build/decoders/<id>_<ver>.dll` 同 id 同版本同字节；`docs/<id>_<ver>.zip` 里被
/// `plugins.json` 取代的旧包要清掉。2026-10-29 实测发生过漂移（随包清单停在 10-28 的 4 条
/// 0.2.3，目录里已是 7 条 0.2.4），当时没有任何机器读这两处——所以这条不是形式：
/// 夹具里那条「版本漂移会红」用的就是那天真实的数字对。
#[test]
fn j14_deploy_selftest_proves_the_release_dir_checks() {
    let out = Command::new("python")
        .arg("build/deploy.py")
        .arg("--selftest")
        .current_dir(main_dir())
        .output()
        .expect("无法启动 python build/deploy.py --selftest");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "deploy.py --selftest 失败：\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("selftest: ok"),
        "deploy.py --selftest 没跑到终点（输出里没有终句）：{text}"
    );
}

/// 抓「5,016,064 B」「5,016,064 字节」「4.78 MiB」三种写法；数字与单位之间允许空格。
/// 只认 ASCII 字符边界，不会切坏多字节中文。
///
/// 判据只盯 **exe 量级**：字节写法要求 ≥7 位数字（`81 141 B` 的 dump 大小不算），
/// MiB 写法限定 3.0–6.0（`256 MiB` 的预读预算是另一种东西）。否则这条门禁会误伤
/// 文档里所有其他尺寸的叙述。
fn size_figures(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b',' || bytes[i] == b'.')
        {
            i += 1;
        }
        let token = &text[start..i];
        let raw_tail = text[i..].trim_start();
        let after_unit = raw_tail
            .strip_prefix('B')
            .or_else(|| raw_tail.strip_prefix("字节"));
        let digits = token.chars().filter(|c| c.is_ascii_digit()).count();
        let value: f64 = token.replace(',', "").parse().unwrap_or(-1.0);
        let byte_form = after_unit.is_some_and(|_| digits >= 7);
        let mib_form =
            token.contains('.') && raw_tail.starts_with("MiB") && (3.0..=6.0).contains(&value);
        if byte_form || mib_form {
            out.push(token.to_string());
        }
    }
    out
}

/// ⑮ windui 的滚轮事件必须把修饰键送上来（补丁 027）。
/// 宿主 `canvas.rs` 用 `p.mods.ctrl` 分派 Ctrl+滚轮、用 `p.mods.shift` 判条上一屏；
/// 平台层若还用 `PointerEvent::single`（Mods 全 false），这两条手势就一起失效。
fn assert_wheel_mods_forwarded(body: &str, what: &str) {
    assert!(
        body.contains("single_with"),
        "{what}：滚轮仍用 single()，修饰键全丢"
    );
    assert!(body.contains("ctrl:"), "{what}：Mods 没填 ctrl");
    assert!(body.contains("shift:"), "{what}：Mods 没填 shift");
}

#[test]
fn j15_wheel_event_carries_modifier_keys() {
    let main = main_dir();
    let root = main.parent().expect("main 没有上级目录");
    let p = root.join(".3rd/wind-ui-rust-0.23.0/src/platform/win32/mod.rs");
    let src = read(&p);
    let rest = src
        .split("unsafe fn handle_wheel")
        .nth(1)
        .unwrap_or_else(|| panic!("找不到 handle_wheel"));
    let end = rest.find("\n}\n").unwrap_or(rest.len());
    assert_wheel_mods_forwarded(&rest[..end], "现状");
}

/// 负向臂：补丁 027 之前的 handle_wheel 形状喂同一个断言，必须报红；
/// 也钉住断言本身——删掉 single_with 那一条，这里不再 panic 而失败。
#[test]
#[should_panic(expected = "修饰键全丢")]
fn j15_wheel_mods_reject_the_old_single_constructor() {
    let old = r#"
unsafe fn handle_wheel(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let delta = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
    dispatch_pointer_event(hwnd, PointerEvent::single(PointerKind::Wheel(delta), pt, MouseButton::Left));
}
"#;
    assert_wheel_mods_forwarded(old, "补丁 027 之前");
}

/// m269 / 补丁 028：无边框形态的平台侧落地判据。
///
/// 少任何一条都会退化成「空格看图」那种鼠标穿透窗，或者退化成不透明窗。
/// 逐字节归档另由 `j13` 守，这里只钉形状。
fn assert_transparent_bg_wired(src: &str, what: &str) {
    assert!(
        src.contains("transparent_bg: bool"),
        "{what}：后端没有形态标志"
    );
    assert!(
        src.contains("alpha_floor && a == 0 { 1 }"),
        "{what}：DIB 没有 alpha 下限 1（鼠标会穿透）"
    );
    assert!(
        src.contains("self.alpha == 255 && !self.transparent_bg"),
        "{what}：100% 不透明时仍会摘掉分层"
    );
    assert!(
        src.contains(
            "WindowOp::SetTransparentBackground(on)) => set_transparent_background(hwnd, on)"
        ),
        "{what}：WindowOp 没有落到 win32 执行体"
    );
    assert!(
        src.contains("tiny_skia::Color::from_rgba8(0, 0, 0, 0)"),
        "{what}：底色没有清成透明"
    );
    // ★ 宿主那一层必须被通知到：平台清完透明，`UiHost::render` 的整窗帧还会
    //   `pixmap.fill(self.bg)` 一遍，没这条就还是实心底色（真机实测症状）。
    assert!(
        src.contains("s.handler.set_bg_transparent(on)"),
        "{what}：没把透明交给宿主（整窗清底会把透明盖掉）"
    );
    // ★ 补丁 029：DWM 自己那圈 1px 边框。补丁 028 关的只是 Win10 那条自绘外框，
    //   Win11 上边框是 DWM 画的，且画在窗口矩形最外一圈像素之内（真机采样 (194,194,194)）。
    assert!(
        src.contains("set_dwm_frame_border(hwnd, !on)"),
        "{what}：DWM 那圈 1px 边框没关（透明窗四周会留灰线）"
    );
    assert!(
        src.contains("DWMWA_COLOR_NONE"),
        "{what}：DWM 边框色没有置成「无」"
    );
}

/// 宿主侧：`UiHost` 要真的能把整窗清底切成全透明。
fn assert_host_clear_goes_transparent(src: &str, what: &str) {
    assert!(
        src.contains("bg_transparent: bool"),
        "{what}：宿主没有透明清底标志"
    );
    assert!(
        src.contains("fn set_bg_transparent(&mut self, on: bool)"),
        "{what}：宿主没实现 set_bg_transparent"
    );
    assert!(
        src.contains("let bg = if self.bg_transparent {"),
        "{what}：整窗清底没走透明分支"
    );
    assert!(
        src.contains("Color::rgba(0, 0, 0, 0)"),
        "{what}：透明清底用的不是全透明"
    );
}

#[test]
fn j16_borderless_transparency_never_lets_mouse_through() {
    let main = main_dir();
    let root = main.parent().expect("main 没有上级目录");
    let base = root.join(".3rd/wind-ui-rust-0.23.0/src");
    assert_transparent_bg_wired(&read(&base.join("platform/win32/mod.rs")), "现状");
    assert_host_clear_goes_transparent(&read(&base.join("app/mod.rs")), "现状");
}

/// 负向臂：只漏掉「alpha 下限 1」这一刀的源码喂同一断言，必须报红——
/// 那是补丁 028 最容易丢、也最不容易被肉眼发现的一条。
#[test]
#[should_panic(expected = "DIB 没有 alpha 下限 1")]
fn j16_rejects_transparency_without_the_alpha_floor() {
    let old = r#"
struct SkiaBackend { alpha: u8, transparent_bg: bool }
unsafe fn paint_layered(...) {
    if (self.alpha == 255 && !self.transparent_bg) { return self.paint(hwnd, bg, handler); }
    pixmap.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 0));
    s.handler.set_bg_transparent(on);
}
fn copy_premul_rows(...) { let a = (s[3] as u32 * alpha as u32) / 255; }
unsafe fn run_window_op(...) {
    Some(WindowOp::SetTransparentBackground(on)) => set_transparent_background(hwnd, on),
}
"#;
    assert_transparent_bg_wired(old, "漏掉 alpha 下限的假补丁");
}

/// 负向臂 2：宿主那一刀漏掉时也必须报红。这条比 alpha 下限更隐蔽——
/// 平台侧四条全在、窗口也是分层窗，只有"透明区仍然是一片实心底色"这一个症状。
#[test]
#[should_panic(expected = "宿主没有透明清底标志")]
fn j16_rejects_a_host_that_keeps_painting_its_own_background() {
    let old = r#"
struct UiHost { bg: Color }
impl UiHost {
    fn render(&mut self, target: &mut dyn RenderTarget, size: Size) {
        target.begin_damage(None, self.bg);
    }
}
"#;
    assert_host_clear_goes_transparent(old, "没接透明清底的假宿主");
}

/// 负向臂 3（补丁 029）：透明做对了、自绘外框也关了，唯独没关 DWM 那圈 1px 边框。
/// 症状只有一条——透明窗四周一条灰线、图片还被切掉 1px，肉眼看像"没做干净"，
/// 却骗过前六条断言。用户实机就是这么撞上的。
#[test]
#[should_panic(expected = "DWM 那圈 1px 边框没关")]
fn j16_rejects_a_transparent_window_that_keeps_the_dwm_frame() {
    let old = r#"
struct SkiaBackend { transparent_bg: bool }
unsafe fn paint_layered(...) {
    if (self.alpha == 255 && !self.transparent_bg) { return self.paint(hwnd, bg, handler); }
    pixmap.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 0));
}
fn copy_premul_rows(...) {
    let a = if alpha_floor && a == 0 { 1 } else { a };
}
unsafe fn sync_outline(hwnd: HWND) {
    let color = (!transparent).then(|| win10_outline_color(active));
}
unsafe fn run_window_op(...) {
    Some(WindowOp::SetTransparentBackground(on)) => set_transparent_background(hwnd, on),
}
unsafe fn set_transparent_background(hwnd: HWND, on: bool) {
    s.handler.set_bg_transparent(on);
}
"#;
    assert_transparent_bg_wired(old, "只关了自绘外框的假补丁");
}

/// 判据⑰：同一功能在用户看得见的地方只许有一个名字。
///
/// 范围 = UI 源码的**非注释行** + README。内部标识符（ini 键 `space_preview`、
/// 模块 `spacehook`、类型 `RawParams`）与源码注释里的技术术语不在此列：
/// 改它们会破坏兼容与检索，那不是文案问题。HISTORY / 待办按体例要记录旧名，同样豁免。
const RETIRED_UI_LABELS: &[(&str, &str)] = &[
    ("上一帧", "动画上一帧"),
    ("下一帧", "动画下一帧"),
    ("暂停/继续动画", "暂停/播放动画"),
    ("空格看图", "空格预览"),
    ("RAW 参数", "裸数据参数"),
    ("适应长图", "长图模式"),
    ("最小化到托盘", "关闭到托盘"),
];

const LABEL_SURFACES: &[&str] = &[
    "src/core/hotkey.rs",
    "src/ui/chrome.rs",
    "src/ui/canvas.rs",
    "src/ui/settings/options.rs",
    "src/ui/settings/shortcuts.rs",
    "src/ui/raw_dialog.rs",
    "README.md",
];

/// 统一后的名字必须真的落在正本里——只查"旧名不出现"会被"两边都删掉"骗过。
const UNIFIED_ANCHORS: &[(&str, &str)] = &[
    ("src/core/hotkey.rs", "Action::FramePrev => \"上一帧\""),
    (
        "src/core/hotkey.rs",
        "Action::PauseAnim => \"暂停/继续动画\"",
    ),
    ("src/ui/settings/options.rs", "check_row(\"空格看图\""),
    ("src/ui/raw_dialog.rs", "Window::new(\"RAW 参数\""),
    ("README.md", "**适应长图**"),
    ("README.md", "**最小化到托盘**"),
];

fn visible_lines(src: &str) -> String {
    src.lines()
        // 只扫**生产区**：`#[cfg(test)]` 之后的负向臂按设计会复刻旧名做假数据，
        // 那不是用户看得见的文案，扫进去会自己把自己判红。
        .take_while(|l| !l.trim_start().starts_with("#[cfg(test)]"))
        .filter(|l| {
            let s = l.trim_start();
            !s.is_empty() && !(s.starts_with("//") || s.starts_with("*") || s.starts_with("/*!"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_no_retired_label(surfaces: &[(String, String)], what: &str) {
    for (new, old) in RETIRED_UI_LABELS {
        for (path, text) in surfaces {
            assert!(
                !text.contains(old),
                "{what}：{path} 出现退役文案「{old}」（统一为「{new}」）"
            );
        }
    }
}

#[test]
fn j17_user_visible_labels_share_one_name() {
    let mut surfaces = Vec::new();
    for rel in LABEL_SURFACES {
        let full = main_dir().join(rel);
        surfaces.push((rel.to_string(), visible_lines(&read(&full))));
    }
    assert_no_retired_label(&surfaces, "现状");
    for (rel, anchor) in UNIFIED_ANCHORS {
        let full = main_dir().join(rel);
        assert!(
            read(&full).contains(anchor),
            "{rel} 里找不到统一后的文案锚点「{anchor}」"
        );
    }
}

/// 负向臂：把旧名塞回任一界面，同一判据必须当场红。
#[test]
#[should_panic(expected = "出现退役文案「动画上一帧」")]
fn j17_rejects_a_label_that_drifted_back() {
    let fake = vec![(
        "src/core/hotkey.rs".to_string(),
        "Action::FramePrev => \"动画上一帧\"".to_string(),
    )];
    assert_no_retired_label(&fake, "旧实现");
}

/// 判据⑲（E5 重定义后下沉）：工作区根 `AGENTS.md` 是**本地工作区地图**（给本机 AI
/// 助手看，不随三仓上传），它与 `main/AGENTS.md` 是「地图 ↔ 正本」的分工，
/// **不做逐节等同**——旧 E5 的"两份逐节对读"判据随这个口径作废。地图的本职是
/// "不骗人"：它点名的每个仓内路径都必须真实存在。本查扫描根地图文本，提取带仓
/// 前缀（`main/`、`RuPix-Decoder/`、`RuPix-Plugin/`、`.build/`、`.plan/`、`.temp/`、
/// `.3rd/`、`.ex/`、`.bf/`、`.workbuddy/`）的路径（正反斜杠都认），逐一断言存在。
/// 边界：叙述性**状态**的漂移（如某天地图里写"本仓无 .git"这类时效句）不在此查
/// ——那靠文档对读轮次兜底，本查只管"指得到 / 指不到"。段内非 ASCII 只认常用
/// 汉字区（U+4E00–9FFF），全角标点（，：（）等）一律断段——否则
/// "`association.rs`，仅 HKCU" 会把逗号吃进路径造成误报。
/// 失明兜底：一张路径都提取不到 = 地图格式大改，报红不静默。
///
/// 从地图文本提取带仓前缀的仓内路径：去重保序、统一 `/`、去尾分隔符。
/// 裸前缀（"不在 `main` 管辖内"）与无仓前缀的路径（`patches/`、`target/`）不算。
fn workspace_map_paths(text: &str) -> Vec<String> {
    const PREFIXES: &[&str] = &[
        "RuPix-Decoder",
        "RuPix-Plugin",
        "main",
        ".build",
        ".plan",
        ".temp",
        ".3rd",
        ".ex",
        ".bf",
        ".workbuddy",
    ];
    let bytes = text.as_bytes();
    // 前缀的词边界：前一个字符不许是字母 / 数字 / 下划线 / 点（`domain/`、`x.3rd` 不算）。
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.';
    // 路径段字符：ASCII 词字符 + `-`，以及常用汉字区（U+4E00–U+9FFF，UTF-8 为
    // E4–E9 开头的三字节）。全角标点与符号区不在其列——它们必须断段。
    let seg_len = |bytes: &[u8]| -> usize {
        let c = bytes[0];
        if c.is_ascii() {
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'-' {
                1
            } else {
                0
            }
        } else if (0xE4..=0xE9).contains(&c)
            && bytes.len() >= 3
            && (bytes[1] & 0xC0) == 0x80
            && (bytes[2] & 0xC0) == 0x80
        {
            3
        } else {
            0
        }
    };
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // 非 ASCII 字符整体跳过：前缀全是 ASCII，不可能从多字节内部开始；
        // 按 UTF-8 首字节跳整字符，保证 i 永远落在字符边界上（text[i..] 切片不炸）。
        if bytes[i] >= 0x80 {
            i += match bytes[i] {
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF7 => 4,
                _ => 1, // 孤立续字节（不应出现），防御性跳过
            };
            continue;
        }
        // 尝试在 i 处匹配某个仓前缀；词中（`domain/`）与裸前缀（"不在 `main` 管辖"）不算路径。
        // 前缀集合内互不为真前缀，同一位置命中即停。
        let mut matched: Option<&str> = None;
        for p in PREFIXES {
            if !text[i..].starts_with(p) {
                continue;
            }
            let word_inside = i > 0 && is_word(bytes[i - 1]);
            let after = &text[i + p.len()..];
            if !word_inside && (after.starts_with('/') || after.starts_with('\\')) {
                matched = Some(p);
            }
            break;
        }
        let Some(p) = matched else {
            i += 1;
            continue;
        };
        // 收集 前缀 + (分隔符 + 段)*；分隔符后没有段就停在分隔符前（尾斜杠形式）。
        let mut end = i + p.len();
        while end < bytes.len() && (bytes[end] == b'/' || bytes[end] == b'\\') {
            let mut j = end + 1;
            while j < bytes.len() {
                let n = seg_len(&bytes[j..]);
                if n == 0 {
                    break;
                }
                j += n;
            }
            if j == end + 1 {
                break;
            }
            end = j;
        }
        let path = text[i..end].replace('\\', "/");
        let path = path.trim_end_matches('/').to_string();
        if !out.contains(&path) {
            out.push(path);
        }
        i = end.max(i + 1);
    }
    out
}

fn broken_map_paths(root: &std::path::Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|p| !root.join(p).exists())
        .cloned()
        .collect()
}

#[test]
fn j19_workspace_root_agents_map_is_truthful() {
    let root = main_dir()
        .parent()
        .expect("main 没有上级目录")
        .to_path_buf();
    let map = read(&root.join("AGENTS.md"));
    let paths = workspace_map_paths(&map);
    assert!(
        paths.len() >= 10,
        "j19：根地图里一处仓内路径都没提取到（格式大改？本查失明）——{paths:?}"
    );
    let broken = broken_map_paths(&root, &paths);
    assert!(
        broken.is_empty(),
        "工作区根 AGENTS.md（本地地图）点名的路径在磁盘上不存在：{broken:?}\
         ——地图不骗人是它的本职：修地图，或把目录建回来"
    );
}

/// 负向臂：幽灵路径必须被抓，裸前缀（无斜杠）不算路径，相等集合不误报。
#[test]
fn j19_rejects_a_map_pointing_at_ghosts() {
    let root = main_dir()
        .parent()
        .expect("main 没有上级目录")
        .to_path_buf();
    let fake =
        "正本在 `main/AGENTS.md`，快照进 `.bf/`；不在 `main` 管辖内；另有 `main/不存在/幽灵.md`。";
    let paths = workspace_map_paths(fake);
    assert_eq!(
        paths,
        vec!["main/AGENTS.md", ".bf", "main/不存在/幽灵.md"],
        "提取顺序与内容不对：{paths:?}"
    );
    let broken = broken_map_paths(&root, &paths);
    assert_eq!(
        broken,
        vec!["main/不存在/幽灵.md"],
        "幽灵路径必须被抓：{broken:?}"
    );
}

/// 判据⑱（`开发指南.md` §五 E2 下沉）：README 技术栈表的 windows feature 集
/// = `Cargo.toml` 里 `windows` 依赖实际声明的 features 数组，**双向集合判等**。
///
/// 为什么值得一条硬门禁：feature 少列一条，换台机器照 README 对账就编不过；
/// 截断写法（实测抓到 `Win32_System_Pipes` 被写成 `Pipes`）数个数也数不出来
/// ——所以判据是**逐条集合判等**，不是计数。README 那一行的固定表述
/// （「共 **N 条 feature**」「下面已省掉各自的 `Win32_` 前缀：」）是本判据的解析
/// 契约：锚不到、列表空、数组空，一律按"本查失明"报红——静默通过比报错更危险。
/// 行内那个 `N` 也在对账之列：它是第二个正本，漂了照样红。
///
/// 两侧集合判差。纯函数，负向臂直接喂合成样本。
/// 返回违规消息：cargo 有 readme 无 = 缺列；readme 有 cargo 无 = 多列/截断。
fn feature_set_diff(
    cargo: &std::collections::BTreeSet<String>,
    readme: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let mut bad = Vec::new();
    for f in cargo.difference(readme) {
        bad.push(format!(
            "README 技术栈表缺 feature {f}（换台机器照 README 对账就编不过；名单真源是 Cargo.toml）"
        ));
    }
    for f in readme.difference(cargo) {
        bad.push(format!(
            "README 技术栈表多了 feature {f} —— Cargo.toml 里没有（截断写法如 Pipes？写全名，或从 Cargo.toml 确认真名）"
        ));
    }
    bad
}

/// 从 `Cargo.toml` 文本里取 `windows` 依赖的 features 数组全集。
/// 数组条目是逐行 `"Win32_xxx",`（行尾带 `#` 注释），先剥注释再收引号串。
fn cargo_windows_features(cargo_toml: &str) -> std::collections::BTreeSet<String> {
    let start = cargo_toml
        .find("windows = {")
        .unwrap_or_else(|| panic!("j18：Cargo.toml 里找不到 windows 依赖（本查失明）"));
    let seg = &cargo_toml[start..];
    let fkey = seg
        .find("features")
        .unwrap_or_else(|| panic!("j18：windows 依赖里没有 features 键（本查失明）"));
    let open_rel = seg[fkey..]
        .find('[')
        .unwrap_or_else(|| panic!("j18：features 数组的 [ 找不到（本查失明）"));
    let after = &seg[fkey + open_rel + 1..];
    let body = &after[..after
        .find(']')
        .unwrap_or_else(|| panic!("j18：features 数组的 ] 找不到（本查失明）"))];
    let mut out = std::collections::BTreeSet::new();
    for raw in body.lines() {
        let line = match raw.find('#') {
            Some(i) => &raw[..i], // 数组里的行尾注释不参与
            None => raw,
        };
        let mut parts = line.split('"');
        parts.next(); // 首个引号之前的残段
        while let Some(item) = parts.next() {
            let item = item.trim();
            if !item.is_empty() {
                out.insert(item.to_string());
            }
            parts.next(); // 跳过配对引号之后的段
        }
    }
    out
}

/// 从 README 技术栈表的 windows 行取 feature 全集（条目省略 `Win32_` 前缀，按
/// 行内声明补回），并顺带核对行内「共 **N 条 feature**」的计数。
fn readme_windows_features(readme: &str) -> std::collections::BTreeSet<String> {
    let line = readme
        .lines()
        .find(|l| l.contains("windows 0.62") && l.contains("已省掉各自的"))
        .unwrap_or_else(|| {
            panic!(
                "j18：README 技术栈表里找不到 windows 行（锚点句「windows 0.62 / 已省掉各自的」漂了？本查失明）"
            )
        });
    let declared = line
        .split("共 **")
        .nth(1)
        .and_then(|s| s.split(" 条 feature").next())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or_else(|| {
            panic!(
                "j18：README windows 行的「共 **N 条 feature**」读不出来（计数表述漂了？本查失明）"
            )
        });
    let list = line
        .split("前缀：")
        .nth(1)
        .unwrap_or_else(|| {
            panic!("j18：README windows 行找不到「前缀：」锚（列表表述漂了？本查失明）")
        })
        .split('）')
        .next()
        .unwrap_or_else(|| panic!("j18：README windows 行的列表段没有收尾「）」（本查失明）"));
    let mut out = std::collections::BTreeSet::new();
    for tok in list.split('/') {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        let full = if t.starts_with("Win32_") {
            t.to_string()
        } else {
            format!("Win32_{t}")
        };
        out.insert(full);
    }
    assert_eq!(
        out.len(),
        declared,
        "README windows 行自报「共 {declared} 条 feature」，实际列了 {} 条——计数本身漂了（它是第二个正本）",
        out.len()
    );
    out
}

#[test]
fn j18_readme_windows_features_match_cargo_toml() {
    let main = main_dir();
    let cargo = cargo_windows_features(&read(&main.join("Cargo.toml")));
    assert!(
        !cargo.is_empty(),
        "j18：Cargo.toml 的 windows features 数组没解析出任何条目（写法变了，本查失明）"
    );
    let readme = readme_windows_features(&read(&main.join("README.md")));
    assert!(
        !readme.is_empty(),
        "j18：README windows 行没解析出任何条目（列表写法变了，本查失明）"
    );
    let drift = feature_set_diff(&cargo, &readme);
    assert!(
        drift.is_empty(),
        "README 技术栈表与 Cargo.toml 的 feature 集不一致：\n{}",
        drift.join("\n")
    );
}

/// 负向臂（E2 的两个真实事故形状）：少列一条、截断写法，同一判据必须当场红。
#[test]
fn j18_rejects_missing_and_truncated_features() {
    let cargo: std::collections::BTreeSet<String> = ["Win32_System_Pipes", "Win32_Foundation"]
        .into_iter()
        .map(String::from)
        .collect();
    // 形状一：README 漏列一条（换台机器照 README 对账就编不过）。
    let readme: std::collections::BTreeSet<String> =
        ["Win32_Foundation"].into_iter().map(String::from).collect();
    let drift = feature_set_diff(&cargo, &readme);
    assert!(
        drift.len() == 1 && drift[0].contains("缺 feature Win32_System_Pipes"),
        "漏列必须报缺：{drift:?}"
    );
    // 形状二：截断写法 `Pipes`（历史实测事故）——补前缀后与真名对不上，
    // 两侧各报一条：多一个 Win32_Pipes、缺一个 Win32_System_Pipes。
    let truncated: std::collections::BTreeSet<String> = ["Win32_Foundation", "Pipes"]
        .into_iter()
        .map(String::from)
        .collect();
    let truncated = truncated
        .into_iter()
        .map(|t| {
            if t.starts_with("Win32_") {
                t
            } else {
                format!("Win32_{t}")
            }
        })
        .collect::<std::collections::BTreeSet<String>>();
    let drift = feature_set_diff(&cargo, &truncated);
    assert!(
        drift.len() == 2
            && drift.iter().any(|m| m.contains("多了 feature Win32_Pipes"))
            && drift
                .iter()
                .any(|m| m.contains("缺 feature Win32_System_Pipes")),
        "截断写法必须两侧都报：{drift:?}"
    );
    // 相等集合不许报（防误伤）。
    let same: std::collections::BTreeSet<String> = cargo.clone();
    assert!(feature_set_diff(&cargo, &same).is_empty(), "相等集合不该报");
}
