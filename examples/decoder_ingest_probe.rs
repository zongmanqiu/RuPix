//! 临时探针：解码器**接入 + 解码**真机验证（宿主侧）。
//!
//! 回答一个问题：**宿主能不能正常接入发行目录里的解码器 DLL，并把它们的格式解出来。**
//!
//! 三段：
//! 1. **接入面**：逐枚 DLL 读自述（`iv_decoder_info` / `iv_caps` / `iv_metadata`）——
//!    这一步过不了，后面什么都不用谈（ABI 不匹配、缺导出、库加载失败都会在这里露头）。
//! 2. **解码面**：拿**真实样本**（解码器仓 `testdata/golden`）走**生产路由**
//!    （`core::decode::probe_decode`，即宿主平时打开图片的那条路），逐个出图。
//! 3. **归属面**：样本后缀 → 声明认领它的 DLL → 实测出图，三者对账。
//!
//! ★ 走的是生产路由而不是自己 `LoadLibrary`：只有生产路由能证明
//! "宿主 + 真实 DLL"这一对组合是通的（含选路、两阶段、缓冲分配、SEH 隔离）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rupix_lib::core::decode::probe_decode;

/// 样本目录 = 命令行第一参数。★ **不写死本机绝对路径**：样本在别的仓里，写死等于把这台机器的
/// 目录结构编进探针——换台机器就指到空目录，还一路"看着正常"地报 0 个样本。
fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        println!("用法：cargo run --release --example decoder_ingest_probe -- <样本目录>");
        return;
    };

    // ── 1. 接入面 ────────────────────────────────────────────────
    let entries = rupix_lib::decoders::list_decoders();
    println!("══ 1. 接入面（读自述）══");
    if entries.is_empty() {
        println!("  ✗ 一枚 DLL 都没接上——检查 RUPIX_DECODERS_DIR / decoders 目录");
        return;
    }
    let mut ext_owner: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in &entries {
        println!(
            "  · id={:<8} file={:<22} size={:<9} health={:?} enabled={} 后缀={} raw={} vector={} region={}",
            e.id,
            e.file,
            e.size,
            e.health,
            e.enabled,
            e.extensions.len(),
            e.has_raw,
            e.has_vector,
            e.has_vector && !e.vector_exts.is_empty(),
        );
        if !e.vector_exts.is_empty() {
            println!("      矢量后缀 = {:?}", e.vector_exts);
        }
        for x in &e.extensions {
            ext_owner
                .entry(x.trim_start_matches('.').to_ascii_lowercase())
                .or_default()
                .insert(e.id.clone());
        }
    }

    // ── 2. 解码面 ────────────────────────────────────────────────
    println!("\n══ 2. 解码面（真实样本走生产路由）══");
    if !dir.is_dir() {
        println!("  ✗ 样本目录不存在：{}", dir.display());
        return;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    collect(&dir, &mut files);
    files.sort();
    println!("  样本目录：{}（{} 个文件）", dir.display(), files.len());

    let mut ok = 0usize;
    let mut fail = 0usize;
    let mut skipped = 0usize;
    let mut hits: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut covered: BTreeSet<String> = BTreeSet::new();

    for f in &files {
        let Some(ext) = f.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        let owners = ext_owner.get(&ext).cloned().unwrap_or_default();
        if owners.is_empty() {
            skipped += 1;
            continue;
        }
        match probe_decode(f, 0, 0) {
            Ok(d) => {
                ok += 1;
                for o in &owners {
                    *hits.entry(o.clone()).or_default() += 1;
                    covered.insert(o.clone());
                }
                // 首像素非全零 = 真解出了东西（不是一张空图）。
                let head = d
                    .bitmap
                    .rgba
                    .get(0..4)
                    .map(|p| p.to_vec())
                    .unwrap_or_default();
                println!(
                    "  ✓ {:<34} 后缀={:<5} 认领={:<14} natural={}x{} 画布={}x{} 首像素={:?}",
                    f.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    ext,
                    owners.iter().cloned().collect::<Vec<_>>().join(","),
                    d.natural.0,
                    d.natural.1,
                    d.bitmap.width,
                    d.bitmap.height,
                    head
                );
            }
            Err(e) => {
                fail += 1;
                failures.push(format!("{}：{e}", f.display()));
                println!(
                    "  ✗ {:<34} 后缀={:<5} 认领={:<14} {e}",
                    f.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    ext,
                    owners.iter().cloned().collect::<Vec<_>>().join(",")
                );
            }
        }
    }

    // ── 3. 归属面 ────────────────────────────────────────────────
    println!("\n══ 3. 归属面（谁接上了、谁没被样本覆盖）══");
    for e in &entries {
        let n = hits.get(&e.id).copied().unwrap_or(0);
        println!("  · {:<8} 样本命中 {n} 个", e.id);
    }
    let uncovered: Vec<String> = entries
        .iter()
        .map(|e| e.id.clone())
        .filter(|id| !covered.contains(id))
        .collect();
    if !uncovered.is_empty() {
        println!(
            "  （未被样本覆盖：{}——只是没有现成样本，不是接入失败）",
            uncovered.join(", ")
        );
    }

    println!("\n══ 结论 ══");
    println!("  解码成功 {ok} 个 / 失败 {fail} 个 / 非解码后缀跳过 {skipped} 个");
    if fail == 0 {
        println!("  ★ 宿主能正常接入发行目录里的全部 DLL，样本全部出图");
    } else {
        println!("  ⚠ 有 {fail} 个样本没解出来：");
        for x in &failures {
            println!("    - {x}");
        }
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}
