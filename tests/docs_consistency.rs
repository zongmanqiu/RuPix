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
