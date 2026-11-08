//! B1 后半：打开大目录时**枚举那一段**到底多贵（无 GUI，纯读盘判定）。
//!
//! 问题出处 `docs/待办.md` §13「B1 后半」：`Gallery::from_file` / `rescan` 在 UI 线程上同步
//! 枚举整个目录，排序那一半早已修（Schwartzian），枚举那半从没量过。当年写死的触发条件是
//! 「1000 图仍卡就先修枚举段」——**这一轮就是来给这个"仍卡"下一个数的**，不是来改代码的。
//!
//! 跑法（**在 `main/` 下**跑，夹具落在仓库根的 `../.temp/b1-dirs`，几秒造完）：
//!
//! ```text
//! RUPIX_DECODERS_DIR=../.build/decoders cargo run --release --example bench_dir_listing
//! ```
//!
//! 四条臂，都在**同一进程、同一批目录**里跑，臂序整体反序复跑一遍（K89 的配对定式）：
//! - **A 生产** —— 真的调 [`Gallery::from_file`]，包含 `declared_exts()` + `read_dir` +
//!   逐项 `Path::is_file()` + 后缀过滤 + 自然序排序 + 锚点定位。这是用户等的那件事。
//! - **A 拆段** —— 把 A 的四个动作各自单独计时（枚举 / stat / 过滤 / 排序），钱在哪儿一眼看见。
//! - **B 免 stat** —— 候选修法：判定"是不是文件"改用 `DirEntry::file_type()`，它读的是 Windows
//!   `FindNextFile` 已经递到手的 `WIN32_FIND_DATAW.dwFileAttributes`，**不再逐个
//!   `GetFileAttributesW`**；同时把便宜的后缀判据挪到贵的那条前面。**先逐项比清单一致**
//!   才允许谈加速（与 m65/m66 downscale 比臂同一条规矩）。
//! - **C 下限** —— 只 `read_dir` 把路径收进 `Vec`，不判定、不过滤、不排序：这段路的地板。
//!
//! ★ 口径诚实，两条都得记住：
//! 1. 第一趟（夹具刚造完、目录元数据冷）单独打一行并**不参与裁决**——本机冷读噪声能盖过整条腿。
//! 2. 反复枚举同一批目录会把系统目录缓存焐热，报的是"命中缓存时的成本"。用户**第一次**打开这个
//!    目录比这更贵，差多少本探针不猜（那要量真盘 + 冷缓存，不在这轮范围里）。

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rupix_lib::core::formats;
use rupix_lib::core::gallery::{Gallery, SortSpec};

/// 夹具根目录（仓库根 `.temp`，与编译中间产物同处；源码目录不留中间产物）。
const FIXTURE_ROOT: &str = "../.temp/b1-dirs";

/// 暖趟每臂重复次数（正反两序各一轮，共 `2 * REPS` 个读数）。
const REPS: usize = 7;

/// 一个夹具：目录 + 锚点 + 规模说明。
struct Case {
    label: &'static str,
    dir: PathBuf,
    /// 目录里带图片后缀的文件数（`read_dir` 要走的项数另有 `entries`）。
    images: usize,
    entries: usize,
}

/// 造夹具：`n` 张"图片" + `other` 个非图片项（真相册目录里也总有这些）。
///
/// 文件内容只写几个字节——`is_file()` 付的是**元数据**的钱，与大小无关；造 3000 个 1 B 文件比
/// 造 3000 张真图快两个数量级，而这段成本一分不变。
fn make_case(label: &'static str, n: usize, other: usize) -> Case {
    let dir = PathBuf::from(FIXTURE_ROOT).join(label);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("造夹具目录失败");
    // 名字里混前缀与变宽数字，逼 natural_key 多走分支（与 bench_gallery_sort 同形）。
    let prefixes = ["IMG_", "photo", "DSC", "p", "2024-06-", "截图_"];
    let exts = [".jpg", ".png", ".heic", ".webp"];
    for i in 0..n {
        let pre = prefixes[i % prefixes.len()];
        let ext = exts[(i / prefixes.len()) % exts.len()];
        // `.heic`/`.webp` 在没装对应解码器时不被认领：它们照样进枚举、被后缀过滤挡掉——
        // 真实目录就是这个形态，别把夹具做成"全部命中"。
        fs::write(dir.join(format!("{pre}{i:05}{ext}")), b"x").expect("造夹具文件失败");
    }
    for i in 0..other {
        fs::write(dir.join(format!("note-{i:04}.txt")), b"x").expect("造夹具文件失败");
        // 一枚**带图片后缀的子目录**：两个臂都得把它挡掉（"是不是文件"为假），
        // 这是"A 与 B 清单必须逐项一致"里最容易翻车的一项。
        if i == 0 {
            let _ = fs::create_dir(dir.join("album.jpg"));
        }
    }
    let entries = fs::read_dir(&dir).expect("读不到夹具目录").count();
    let images = n;
    Case {
        label,
        dir,
        images,
        entries,
    }
}

/// 与 `gallery::listing` 里的后缀判据同形（example 够不到 crate 私有函数，这里复刻）。
/// 修改 `gallery.rs` 的后缀判据时必须同步，否则 B 臂比的是另一套过滤。
fn ext_ok(p: &Path, declared: &HashSet<String>) -> bool {
    p.extension()
        .and_then(OsStr::to_str)
        .map(|e| declared.contains(&e.to_lowercase()))
        .unwrap_or(false)
}

/// B 臂：整段自己写（判定换 `file_type()`、过滤前置），其余与生产一致。
///
/// 排序用 `sort_by_cached_key` + 已删除的 `natural_key` 复刻：m275 起生产排序改走系统
/// `StrCmpLogicalW`（`gallery.rs`），这条臂的排序段**刻意停在被测那一刻的旧法**——
/// 它比的是"枚举那一段"的钱，排序对两边是同一份成本（`bench_gallery_sort` 已交代过）。
fn listing_no_stat(dir: &Path, declared: &HashSet<String>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for e in fs::read_dir(dir).expect("B 臂读目录失败") {
        let Ok(e) = e else { continue };
        // ★ 贵→便宜排序：先按后缀挡掉不认领的（纯字符串），剩下的才问"是不是文件"。
        let path = e.path();
        if !ext_ok(&path, declared) {
            continue;
        }
        // 这一行就是整条候选修法的**全部**内容：读缓存的属性，不额外 syscall。
        if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            files.push(path);
        }
    }
    files.sort_by_cached_key(|p| p.file_name().map(natural_key).unwrap_or_default());
    files
}

/// B2 臂：免 stat，但**只对 reparse point 回落**到 `Path::is_file()`。
///
/// 为什么要有这一支：`links` 夹具实测出 B 臂的代价——`DirEntry::file_type()` 对符号链接
/// 报的是**链接本身**（不是目标），于是"链到别处的 jpg"会被判成"不是文件"、从清单里**消失**。
/// 那不是性能差异，是用户看得见的正确性回归。B2 只对 `is_symlink()` 的那几项补一次
/// `is_file()`（普通文件仍然免 syscall），要验的是：**清单与 A 逐项一致，而 15× 还在不在**。
fn listing_lazy_stat(dir: &Path, declared: &HashSet<String>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for e in fs::read_dir(dir).expect("B2 臂读目录失败") {
        let Ok(e) = e else { continue };
        let path = e.path();
        if !ext_ok(&path, declared) {
            continue;
        }
        let ft = e.file_type();
        let is_file = match ft {
            Ok(t) if t.is_file() => true,
            // 链接顶着图片后缀时，目标才是裁决对象——只有这一支才付 syscall。
            Ok(t) if t.is_symlink() => path.is_file(),
            _ => false,
        };
        if is_file {
            files.push(path);
        }
    }
    files.sort_by_cached_key(|p| p.file_name().map(natural_key).unwrap_or_default());
    files
}

/// B3 臂：**只在缓存属性已经说"是文件"时**免 stat，其余一律回落 `Path::is_file()`。
///
/// 与 B2 只差一件事：B2 对 `is_dir()`/`Unknown`/`Err` 直接判"不是文件"，而
/// `DirEntry::file_type()` 在非 Windows（NFS 之类）上**可能返回 `Unknown`**——那种环境下
/// B2 会把整目录的图全挡掉。本项目放弃 WIC 的理由就是可移植性，`core` 层不能留这种洞。
/// B3 因此只走"属性已经确认是文件"这条快路，其他情况一律照旧：**不可能少一项**。
fn listing_safe_no_stat(dir: &Path, declared: &HashSet<String>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for e in fs::read_dir(dir).expect("B3 臂读目录失败") {
        let Ok(e) = e else { continue };
        let path = e.path();
        if !ext_ok(&path, declared) {
            continue;
        }
        let is_file = match e.file_type() {
            Ok(t) if t.is_file() => true,
            // 目录 / 符号链接 / Unknown / 读不到属性：与旧写法一模一样。
            _ => path.is_file(),
        };
        if is_file {
            files.push(path);
        }
    }
    files.sort_by_cached_key(|p| p.file_name().map(natural_key).unwrap_or_default());
    files
}

/// 已删除的 `gallery.rs::natural_key` 的复刻（同 `bench_gallery_sort` 里那份）。
/// ★ m275 起**没有生产实现要同步**：现行排序走系统 `StrCmpLogicalW`。
/// m111（O17）那一版改动记录：数字段那两枚中间 String 换成 `write!` 直写排进键。
fn natural_key(s: &OsStr) -> String {
    use std::fmt::Write as _;
    let s = s.to_string_lossy().to_lowercase();
    let bytes = s.as_bytes();
    let mut key = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < bytes.len() {
        let is_digit = bytes[i].is_ascii_digit();
        let end = bytes[i..]
            .iter()
            .position(|c| c.is_ascii_digit() != is_digit)
            .unwrap_or(bytes.len() - i)
            + i;
        if is_digit {
            let num: u64 = s[i..end].parse().unwrap_or(u64::MAX);
            key.push('\u{1}');
            let _ = write!(key, "{num:020}");
            key.push('\u{1}');
            let _ = write!(key, "{}", end - i);
        } else {
            key.push_str(&s[i..end]);
        }
        i = end;
    }
    key
}

/// A 拆段：把"枚举 → 逐项 stat → 后缀过滤 → 排序"四段各自夹表。
///
/// 这四段合起来≈生产 `listing`，**但不含** `declared_exts()` 与锚点定位——那两笔另打一行。
/// ★ m111 复核勘误：`stat` 那条复刻的是 **m66 之前**的判据（逐项 `Path::is_file()`）。
///   今天的生产走 `DirEntry::file_type()` 快路，整段 `A 生产 from_file` 只 3.4–4.6 ms（3000 项），
///   这里 55–62 ms 那行**不是**现网在付的钱——量"排序段"仍然有效（同一段名字、同一枚键），
///   要读"整段开册"请用 `A 生产` 那一行。
fn listing_split(dir: &Path, declared: &HashSet<String>) -> (f64, f64, f64, f64) {
    let t0 = Instant::now();
    let paths: Vec<PathBuf> = fs::read_dir(dir)
        .expect("拆段读目录失败")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    let enum_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let flagged: Vec<bool> = paths.iter().map(|p| p.is_file()).collect();
    let stat_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let t2 = Instant::now();
    let mut files: Vec<PathBuf> = paths
        .iter()
        .zip(flagged)
        .filter_map(|(p, f)| {
            if f && ext_ok(p, declared) {
                Some(p.clone())
            } else {
                None
            }
        })
        .collect();
    let filter_ms = t2.elapsed().as_secs_f64() * 1000.0;

    let t3 = Instant::now();
    files.sort_by_cached_key(|p| p.file_name().map(natural_key).unwrap_or_default());
    let sort_ms = t3.elapsed().as_secs_f64() * 1000.0;

    black_box(&files);
    (enum_ms, stat_ms, filter_ms, sort_ms)
}

/// C 下限：只收路径。判"这段还能不能压"要有它的地板。
fn read_only(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .expect("C 臂读目录失败")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect()
}

fn sorted(v: &[f64]) -> Vec<f64> {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s
}

fn mn(v: &[f64]) -> f64 {
    sorted(v)[0]
}

fn med(v: &[f64]) -> f64 {
    let s = sorted(v);
    s[s.len() / 2]
}

/// **等价性专用夹具**：把四种"链接形态"混在一个目录里，逐项比 A（`Path::is_file()`）与
/// B（`DirEntry::file_type().is_file()`）的清单。
///
/// 为什么必须单独比这一组：候选修法换掉的是"是不是文件"这一条判据的来源——
/// `Path::is_file()` 会**跟着链接走**（`GetFileAttributesW` 解引用），而 `DirEntry::file_type()`
/// 读的是枚举时缓存的属性。两者在符号链接上**可能不同**，而"清单少一张图"是用户能看见的
/// 正确性回归，不是性能问题。这一组只报数、不 assert：不一致就是这条修法的代价，写出来给人看。
#[cfg(windows)]
fn links_case(declared: &HashSet<String>) {
    use std::os::windows::fs::{symlink_dir, symlink_file};
    let dir = PathBuf::from(FIXTURE_ROOT).join("links");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("造链接夹具失败");
    // ★ 链接目标一律用**绝对路径**：`CreateSymbolicLinkW` 把目标原样存下，相对目标是
    //   相对"链接所在目录"解析的。上一版传的是 `../.temp/.../real.jpg`，结果三条符号链接
    //   全成了断链 ⇒ A 与 B 都挡掉，等价性"通过"得毫无意义（夹具必须落在被判据管的那条分支里）。
    let dir = fs::canonicalize(&dir).expect("取夹具绝对路径失败");
    let other = fs::canonicalize(PathBuf::from(FIXTURE_ROOT).join("small-300"))
        .expect("取对照目录绝对路径失败");
    fs::write(dir.join("real.jpg"), b"x").expect("写夹具失败");
    let mut made: Vec<&str> = vec!["real.jpg"];
    // ① 硬链接（不是 reparse point，属性就是普通文件）
    if fs::hard_link(dir.join("real.jpg"), dir.join("hard.jpg")).is_ok() {
        made.push("hard.jpg");
    }
    // ② 文件符号链接：A 判"是文件"（跟着走），B 读缓存属性——这一条是全部风险所在。
    if symlink_file(dir.join("real.jpg"), dir.join("sym.jpg")).is_ok() {
        made.push("sym.jpg");
    }
    // ③ 断链的符号链接（目标不存在）：A 判"不是文件"。
    if symlink_file(dir.join("gone.jpg"), dir.join("ghost.jpg")).is_ok() {
        made.push("ghost.jpg");
    }
    // ④ 目录符号链接顶着图片后缀：两边都该挡掉。
    if symlink_dir(&other, dir.join("dirlink.jpg")).is_ok() {
        made.push("dirlink.jpg");
    }
    // **夹具活性自检**：这条不成立就整组不算数——"两边都挡掉"不是等价性，是没测到。
    let alive = dir.join("sym.jpg").is_file();
    let a = Gallery::from_file(&dir, SortSpec::DEFAULT).expect("A 臂读链接夹具失败");
    let b = listing_no_stat(&dir, declared);
    let mut a_names: Vec<String> = (0..a.len())
        .map(|i| {
            a.peek(i)
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_default()
        })
        .collect();
    let mut b_names: Vec<String> = b
        .iter()
        .map(|p| {
            p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let b2 = listing_lazy_stat(&dir, declared);
    let b3 = listing_safe_no_stat(&dir, declared);
    let mut b2_names: Vec<String> = b2
        .iter()
        .map(|p| {
            p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let mut b3_names: Vec<String> = b3
        .iter()
        .map(|p| {
            p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    a_names.sort();
    b_names.sort();
    b2_names.sort();
    b3_names.sort();
    println!(
        "\n等价性夹具 links（造出 {} 项：{}）",
        made.len(),
        made.join(", ")
    );
    // 逐形态判词：这是"为什么 B 少一项"的唯一证据，别只给两行清单让人猜。
    println!("  逐项判定（A=is_file，B=file_type，B2=symlink 回落 stat）：");
    for name in &made {
        let p = dir.join(name);
        let e_is_file = p.is_file();
        let cached = fs::read_dir(&dir)
            .expect("读链接夹具失败")
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == std::ffi::OsStr::new(name))
            .and_then(|e| e.file_type().ok());
        let (c_file, c_dir, c_link) = match cached {
            Some(t) => (t.is_file(), t.is_dir(), t.is_symlink()),
            None => (false, false, false),
        };
        println!(
            "    {name:<12} A:{:>1}  缓存属性{{file:{c_file}, dir:{c_dir}, link:{c_link}}}  B:{:>1}  B2:{:>1}  B3:{:>1}",
            if e_is_file { "Y" } else { "." },
            if c_file { "Y" } else { "." },
            if c_file || (c_link && e_is_file) { "Y" } else { "." },
            if c_file || e_is_file { "Y" } else { "." },
        );
    }
    println!("  A （is_file）    ：{a_names:?}");
    println!("  B （file_type）  ：{b_names:?}");
    println!("  B2（懒 stat）     ：{b2_names:?}");
    println!("  B3（保守懒 stat） ：{b3_names:?}");
    println!(
        "  夹具活性：{}（sym.jpg 必须是 A 认得的**真文件**，否则这组等价性不算数）",
        if alive {
            "在"
        } else {
            "★ 不在 —— 本组作废"
        }
    );
    println!(
        "  B  与 A 逐项一致：{}",
        if !alive {
            "不判（夹具没活）"
        } else if a_names == b_names {
            "是"
        } else {
            "否 ★（符号链接的图会消失 = 正确性回归）"
        }
    );
    println!(
        "  B2 与 A 逐项一致：{}",
        if !alive {
            "不判"
        } else if a_names == b2_names {
            "是"
        } else {
            "否 ★"
        }
    );
    println!(
        "  B3 与 A 逐项一致：{}",
        if !alive {
            "不判（夹具没活）"
        } else if a_names == b3_names {
            "是 ★ 推荐形态：不可能比生产少一项"
        } else {
            "否 ★"
        }
    );
}

#[cfg(not(windows))]
fn links_case(_declared: &HashSet<String>) {
    println!("\n等价性夹具 links：本机不是 Windows，跳过（不拿空跑当读数）");
}

fn main() {
    rupix_lib::platform::logging::install_logging();
    // 生产形态：先装解码器栈，让 `declared_exts()` 拿到真实清单，而不是内置兜底清单——
    // 夹具里 `.heic`/`.webp` 认领与否直接改变过滤命中率。
    rupix_lib::install_decode_stack();
    let declared = formats::declared_exts();
    println!("已声明后缀 {} 项：{:?}", declared.len(), {
        let mut d: Vec<&String> = declared.iter().collect();
        d.sort();
        d
    });

    let cases = vec![
        make_case("small-300", 300, 40),
        make_case("n1000", 1000, 120),
        make_case("n3000", 3000, 400),
    ];

    for c in &cases {
        println!(
            "\n夹具 {}：{} 项（其中带图片后缀 {} 个）",
            c.label, c.entries, c.images
        );
        // ── 冷趟：只报，不参与裁决 ────────────────────────────────
        let t = Instant::now();
        let cold_a = Gallery::from_file(&c.dir, SortSpec::DEFAULT).expect("生产臂读不到夹具");
        let cold_a_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let cold_b = listing_no_stat(&c.dir, &declared);
        let cold_b_ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "  冷趟（夹具刚造完，只报不判）：A {:.1} ms / B {:.1} ms",
            cold_a_ms, cold_b_ms
        );

        // ── 一致性：先逐项比清单，再谈速度 ────────────────────────
        // 这三臂在这些夹具里必须逐项相同（`links` 夹具才暴露出 B 的分歧，见下面 links_case）。
        let cold_b2 = listing_lazy_stat(&c.dir, &declared);
        let cold_b3 = listing_safe_no_stat(&c.dir, &declared);
        for (tag, got) in [("B", &cold_b), ("B2", &cold_b2), ("B3", &cold_b3)] {
            assert_eq!(
                cold_a.len(),
                got.len(),
                "{tag} 臂清单长度与生产不一致：{} vs {}",
                cold_a.len(),
                got.len()
            );
            for (i, p) in got.iter().enumerate() {
                let a = cold_a.peek(i).expect("A 臂清单越界");
                assert_eq!(
                    a,
                    p,
                    "{tag} 臂第 {i} 项与生产不一致：{} vs {}",
                    a.display(),
                    p.display()
                );
            }
        }

        // ── 暖趟：A / B / B2 / B3 / C 正反两序 + 拆段 ─────────────
        let mut a_ms = Vec::new();
        let mut b_ms = Vec::new();
        let mut b2_ms = Vec::new();
        let mut b3_ms = Vec::new();
        let mut c_ms = Vec::new();
        let mut dec_ms = Vec::new();
        let mut seg: Vec<(f64, f64, f64, f64)> = Vec::new();
        for pass in 0..2 {
            for _ in 0..REPS {
                let order: [usize; 5] = if pass == 0 {
                    [0, 1, 2, 3, 4]
                } else {
                    [4, 3, 2, 1, 0]
                };
                for which in order {
                    let t = Instant::now();
                    match which {
                        0 => {
                            let g =
                                Gallery::from_file(&c.dir, SortSpec::DEFAULT).expect("A 臂失败");
                            a_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                            black_box(g.len());
                        }
                        1 => {
                            let v = listing_no_stat(&c.dir, &declared);
                            b_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                            black_box(v.len());
                        }
                        2 => {
                            let v = listing_lazy_stat(&c.dir, &declared);
                            b2_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                            black_box(v.len());
                        }
                        3 => {
                            let v = listing_safe_no_stat(&c.dir, &declared);
                            b3_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                            black_box(v.len());
                        }
                        _ => {
                            let v = read_only(&c.dir);
                            c_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                            black_box(v.len());
                        }
                    }
                }
                let t = Instant::now();
                let d = formats::declared_exts();
                dec_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                black_box(d.len());
                if pass == 0 {
                    seg.push(listing_split(&c.dir, &declared));
                }
            }
        }
        let (e1, s1, f1, o1) = (
            mn(&seg.iter().map(|x| x.0).collect::<Vec<_>>()),
            mn(&seg.iter().map(|x| x.1).collect::<Vec<_>>()),
            mn(&seg.iter().map(|x| x.2).collect::<Vec<_>>()),
            mn(&seg.iter().map(|x| x.3).collect::<Vec<_>>()),
        );
        println!(
            "  A 生产 from_file   min {:>6.1} / med {:>6.1} ms",
            mn(&a_ms),
            med(&a_ms)
        );
        println!(
            "  A 拆段             枚举 {:>5.1} + stat {:>5.1} + 过滤 {:>5.1} + 排序 {:>5.1} ms",
            e1, s1, f1, o1
        );
        println!(
            "  B 免 stat          min {:>6.1} / med {:>6.1} ms（A/B med {:.2}×）",
            mn(&b_ms),
            med(&b_ms),
            med(&a_ms) / med(&b_ms)
        );
        println!(
            "  B2 懒 stat         min {:>6.1} / med {:>6.1} ms（A/B2 med {:.2}×）",
            mn(&b2_ms),
            med(&b2_ms),
            med(&a_ms) / med(&b2_ms)
        );
        println!(
            "  B3 保守懒 stat     min {:>6.1} / med {:>6.1} ms（A/B3 med {:.2}×）★ 推荐形态",
            mn(&b3_ms),
            med(&b3_ms),
            med(&a_ms) / med(&b3_ms)
        );
        println!(
            "  C 只收路径（下限） min {:>6.1} / med {:>6.1} ms",
            mn(&c_ms),
            med(&c_ms)
        );
        println!(
            "  declared_exts()    min {:>6.3} ms（每次开图/重扫都付）",
            mn(&dec_ms)
        );
    }
    println!(
        "\n读数口径：min/med 取暖趟 {} 次×正反两序；A 含 `declared_exts` + 锚点定位，B/C 不含。",
        REPS
    );
    links_case(&declared);
    println!("夹具留在 {FIXTURE_ROOT}，要重造删掉该目录再跑。");
}
