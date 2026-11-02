//! 目录内图片清单与导航。
//!
//! 打开一张图 = 打开它所在的**整个目录**：左右方向键在目录内翻页是看图器的基本盘，
//! 用户不会因为"只打开了一个文件"就接受不能翻。

use std::fs;
use std::path::{Path, PathBuf};

use crate::core::error::{ViewError, ViewResult};
use crate::core::formats;

// 格式清单的两个来源（并集），按"谁是主要来源"排序：
//
// 1. **[`formats::declared_exts()`]（主要）** = 已加载解码器自述的扩展名并集
//    装了 `raw` 解码器就出现 `.cr2`，删掉 DLL 就消失——**清单与能力不可能脱节**。
//    历史上手写清单漏过 `.jxl`：本机能解、我们也写了解码器，只因没进清单，用户翻不到。
// 2. **内置库登记表**（`formats::BUILTIN_DECODERS`）= 不进解码器体系、由宿主自己认领的格式。
//    ★ 这份清单**只有那一处**：`formats::builtin_exts()` 从表派生（m56 之前它是
//    `gallery::BUILTIN_EXTS` 常量 + `formats::is_builtin_ext` 的 `matches!` + 路由里的
//    硬编码后缀，三处各写一遍）。
//
// 历史上还有第三来源 `LEGACY_WIC_EXTS`（WIC 兜底时代的能力保险），2026-10-10 随 WIC
// 兜底一并删除：解码全部走解码器后，清单只该反映解码器的真实能力。（2026-10-20 WIC 曾以
// **内置解码库**身份回归、只走用户点名且不进这份清单；2026-10-25 它整层删除——便携
// 看图器不调系统解码器。见 `docs/产品定位与边界.md` §5.1。）

/// 一个目录里的图片清单 + 当前位置。
#[derive(Debug, Clone, Default)]
pub struct Gallery {
    dir: PathBuf,
    files: Vec<PathBuf>,
    index: usize,
}

impl Gallery {
    /// 以「某张图」为锚点建清单：枚举它所在目录，把这张图设为当前项。
    ///
    /// ★ 传进来的**就是**要打开的：锚点不在扩展名清单里（无后缀、后缀没人认领、
    ///   甚至是个 `.txt`）也照样进清单并定位为当前项（见 [`ensure_listed`]）。
    ///   旧写法在这里是 `position(…).unwrap_or(0)` ⇒ 拖入 `说明.txt` 会**静默打开同目录
    ///   第一张别的图**且界面一句提示都没有（m45 缺陷 2）。现在"它到底是不是图片"由
    ///   解码层回答——它本来就有按内容认领的嗅探（`crate::decoders::Host::sniff_ranked`），
    ///   认不得就给"解码器「…」不认识这个文件"，判词指着的是用户真正点开的那个文件。
    pub fn from_file(file: &Path) -> ViewResult<Self> {
        // 拖进来的是**文件夹** = 打开这个文件夹。旧写法把它当"某张图"取 `parent()`，
        // 于是打开的是它旁边那个目录的第一张图。
        if file.is_dir() {
            let files = listing(file)?;
            if files.is_empty() {
                return Err(ViewError::NoImages(file.to_path_buf()));
            }
            return Ok(Self {
                dir: file.to_path_buf(),
                files,
                index: 0,
            });
        }
        let dir = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let mut files = listing(&dir)?;
        ensure_listed(&mut files, file);
        // 锚点必在清单里（`ensure_listed` 刚保证过），这里的 `unwrap_or` 只是不让
        // 编译器替我们写 `unwrap`——`unlisted_anchor_becomes_the_current_item` 与
        // `listed_anchor_keeps_its_slot` 两条用例钉住两个分支。
        let index = files.iter().position(|p| same_path(p, file)).unwrap_or(0);
        Ok(Self { dir, files, index })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 「这张图在不在本清单里」——在就返回它的序号（0-based），不在返回 `None`。
    ///
    /// ★ m101：`Viewer::open` 用它裁决"这次开图算不算换目录"。两条判据都刻意与
    ///   [`Self::from_file`] 同源，否则同一条路径会两次走进不同分支、缓存白留：
    ///   ① 目录归一化同一份（`parent()` 为空 ⇒ `"."`）；② 比较用 [`same_path`]
    ///   （Windows 忽略大小写与分隔符方向），不是 `Path` 的逐字节相等。
    ///
    /// 返回 `None` 的两种情况都退回完整重建：锚点不在清单里（外部刚拷进来的新文件、
    /// 或者它其实是个**文件夹**——文件夹的 `parent()` 是它的上一级，对不上本清单），
    /// 以及真的换了目录。
    pub fn slot_for(&self, file: &Path) -> Option<usize> {
        let dir = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !same_path(dir, &self.dir) {
            return None;
        }
        self.files.iter().position(|p| same_path(p, file))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// 两份清单的路径与顺序是否完全一致（比较口径同 [`same_path`]：Windows 忽略
    /// 大小写与分隔符方向）。
    ///
    /// `Viewer::rescan_dir` 用它判断"兄弟项改名 / 重排"：那种变化长度与当前项都没变，
    /// 只看 `len` 会漏掉，胶片条要等到下一次用户操作才刷新。
    pub fn same_listing(&self, other: &Self) -> bool {
        self.files.len() == other.files.len()
            && self
                .files
                .iter()
                .zip(&other.files)
                .all(|(a, b)| same_path(a, b))
    }

    /// 1 起的序号（给状态栏看）。
    pub fn position(&self) -> usize {
        self.index + 1
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    /// 取第 `idx` 项（不移动位置）。后台预读相邻图用。
    pub fn peek(&self, idx: usize) -> Option<&PathBuf> {
        self.files.get(idx)
    }

    /// 相对移动（`delta` 可为负）。到头返回 false，位置不变。
    pub fn step(&mut self, delta: isize) -> bool {
        if self.files.is_empty() {
            return false;
        }
        let n = self.files.len() as isize;
        let cur = self.index as isize;
        let next = cur + delta;
        if next < 0 || next >= n {
            return false;
        }
        self.index = next as usize;
        true
    }

    /// 相对移动 + **越界回绕**（翻页永远循环，用户拍板 2026-10-09）。
    ///
    /// 3/3 再下一张 → 1/3，1/3 再上一张 → 3/3；`delta` 大于目录长度也正确
    /// （`rem_euclid` 处理任意步长）。单张目录返回 false——回绕到自己等于没动，
    /// 调用方（[`crate::core::Viewer::step`]）据此决定要不要重置视图/重解码。
    pub fn step_looped(&mut self, delta: isize) -> bool {
        let n = self.files.len() as isize;
        if n <= 1 {
            return false;
        }
        let cur = self.index as isize;
        let next = (cur + delta).rem_euclid(n);
        if next as usize == self.index {
            return false;
        }
        self.index = next as usize;
        true
    }

    /// 跳到首 / 尾。
    pub fn jump(&mut self, to: Jump) {
        if self.files.is_empty() {
            return;
        }
        self.index = match to {
            Jump::First => 0,
            Jump::Last => self.files.len() - 1,
        };
    }

    /// 跳到第 `idx` 张（缩略图条点击用）。越界夹到首尾。
    ///
    /// 返回**位置是否真的变了**：跳到自己不该触发重解码/复位视图，否则点当前格
    /// 会把画面闪一下。
    pub fn jump_to(&mut self, idx: usize) -> bool {
        if self.files.is_empty() {
            return false;
        }
        let to = idx.min(self.files.len() - 1);
        if to == self.index {
            return false;
        }
        self.index = to;
        true
    }

    /// 重扫 `self.dir`，尽量保留当前位置（供目录被外部增删 / 改名后原地刷新）。
    ///
    /// - 当前项仍在目录里 → 定位到它（翻页位置不变，只是「3/5」的分母更新）。
    /// - 当前项被删 / 改名 → 落回它原本所在的相邻槽位（`min(old_index, len-1)`），
    ///   调用方据此决定是否重新解码。
    /// - 目录被清空 / 读不到 → 返回 `Err`（保持旧清单不动，等目录回来）。
    ///
    /// 与 [`Gallery::from_file`] 同源的枚举 + 过滤 + 自然序排序，保证"重扫出来的
    /// 清单"和"初次打开的清单"完全一致，不会因实现分叉出现顺序漂移。
    pub fn rescan(&self) -> ViewResult<Self> {
        let mut files = listing(&self.dir)?;
        let old_index = self.index;
        let anchor = self.current().cloned();
        // 锚点可能是"用户显式点开的那一个"，未必带图片后缀（见 [`ensure_listed`]）：
        // 它还在磁盘上就继续留在清单里，被外部删掉/改名才落回相邻槽位。
        if let Some(a) = &anchor {
            if a.is_file() {
                ensure_listed(&mut files, a);
            }
        }
        if files.is_empty() {
            return Err(ViewError::NoImages(self.dir.clone()));
        }
        let index = match anchor {
            Some(a) if files.iter().any(|p| same_path(p, &a)) => {
                // 红线豁免: 上行的 `any` 已确认 `a` 在 `files` 中，position 必为 Some。
                files.iter().position(|p| same_path(p, &a)).unwrap()
            }
            _ => old_index.min(files.len() - 1),
        };
        Ok(Self {
            dir: self.dir.clone(),
            files,
            index,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jump {
    First,
    Last,
}

/// 枚举一个目录里的图片清单：扩展名过滤 + 自然序。**`from_file` 与 `rescan` 共用**，
/// 否则两份清单会因实现分叉而顺序漂移（`rescan` 的注释早就写了这条要求）。
///
/// ★ 这里**不做**逐文件魔数嗅探：那是 O(目录项数 × 解码器数)，每次开图都要付一遍。
///   代价是"目录里其他无后缀但魔数认得的文件"不进网格（翻不到）——罕见形态不值这笔钱。
///   用户**显式点开**的那一个例外，由 [`ensure_listed`] 兜住。
fn listing(dir: &Path) -> ViewResult<Vec<PathBuf>> {
    // 清单只取一次（见 `is_image_ext_in` 的注释）。
    let declared = formats::declared_exts();
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| {
            let e = e.ok()?;
            let path = e.path();
            // 后缀判据在前、"是不是文件"在后：两条是 `&&` 关系，换顺序不改清单，
            // 但能让没人认领的项（`.txt`/`.md` 那一堆）根本不进入贵的那条。
            if !is_image_ext_in(&path, &declared) {
                return None;
            }
            // ★ 快路只走"枚举时缓存的属性已经说是文件"这一支，其余（目录、符号链接、
            //   Unknown、读不到）一律照旧 `Path::is_file()`——一次 `read_dir` 已经把
            //   `WIN32_FIND_DATAW` 的字节属性递到手了，普通文件不必再 `GetFileAttributesW`。
            //   实测（`examples/bench_dir_listing.rs`，同进程五臂、清单逐项比过才谈速度）：
            //   3000 项目录整段 54–66 ms → 3.4–3.8 ms，拆段里 stat 那条占 87%。
            //   **不能**直接把判据换成 `file_type().is_file()`：它判的是**链接本身**而不是
            //   目标，链到别处的 `.jpg` 会被判成"不是文件"、从清单里消失——那条回归由
            //   `links` 夹具当场抓出来（见 `gallery_symlinked_images_stay_listed`）。
            match e.file_type() {
                Ok(t) if t.is_file() => Some(path),
                _ => path.is_file().then_some(path),
            }
        })
        .collect();
    // 自然序排序（大小写不敏感）：数字段按数值比，`img2` 排在 `img10` 前；
    // 同名时数字位数少的在前（`img2` < `img02` < `img10`）。
    sort_natural(&mut files);
    Ok(files)
}

/// 保证 `file` 在清单里：不在（无后缀 / 后缀没人认领 / 根本不是图片）就插进去再重排。
///
/// 判据是"打开什么就该看见什么"，不是"这文件像不像图片"——后者由解码层给判词，
/// 而且它给的比我们猜的准（真的去嗅探过、也真的解过）。
fn ensure_listed(files: &mut Vec<PathBuf>, file: &Path) {
    if !files.iter().any(|p| same_path(p, file)) {
        files.push(file.to_path_buf());
        sort_natural(files);
    }
}

/// 自然序原地排序。Schwartzian transform：先为每项算一次键，比较器只做 String 比较。
///
/// 比较器内每次现算 `natural_key` 会随 O(n log n) 次比较**重复分配 String**——
/// 3000 项实测从 28ms 降到 1.8ms（前后对比数字在 `HISTORY.md` 2026-10-14 段，
/// 原始条目出自已随 m47 删除的「手感断点清单」B1.1）。
/// `file_name()` 为 None 时键是空串，与旧写法（`Option::None` 排最前）等价。
fn sort_natural(files: &mut Vec<PathBuf>) {
    let mut decorated: Vec<(String, PathBuf)> = files
        .drain(..)
        .map(|p| (p.file_name().map(natural_key).unwrap_or_default(), p))
        .collect();
    decorated.sort_by(|a, b| a.0.cmp(&b.0));
    files.extend(decorated.into_iter().map(|(_, p)| p));
}

/// 自然序键：把文件名切成「非数字段 / 数字段」交替序列。
///
/// 数字段编码为 `\u{1}零填充数值\u{1}长度` 字符串：数值**零填充到 u64 最大位数
/// (20)**，这样「字典序 = 数值序」——不填充则 "10" < "2"，`img10` 会错排到
/// `img2` 前（与本文档及 from_file 的声明相悖）；长度兜底同值时 `img2 < img02`
/// （前导零少者排前）；`\u{1}` 哨兵保证数字段永不与非数字文本段混排。
/// 非数字段按字符比（已 lowercase）。
fn natural_key(s: &std::ffi::OsStr) -> String {
    use std::fmt::Write as _;
    // Windows 文件名是合法 UTF-16，to_string_lossy 的损失仅限非法代理对（实际不会出现）。
    let s = s.to_string_lossy().to_lowercase();
    let bytes = s.as_bytes();
    let mut key = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < bytes.len() {
        let is_digit = bytes[i].is_ascii_digit();
        // 数字段/非数字段的边界都是 ASCII，按字节偏移切 utf8 安全。
        let end = bytes[i..]
            .iter()
            .position(|c| c.is_ascii_digit() != is_digit)
            .unwrap_or(bytes.len() - i)
            + i;
        if is_digit {
            // 零填充 20 位（u64 最大位数）：字符串序即数值序。超 20 位的数字段
            // parse 失败兜底 u64::MAX（20 位）仍对齐同一宽度。
            let num: u64 = s[i..end].parse().unwrap_or(u64::MAX);
            key.push('\u{1}');
            // ★ m111（O17）：`write!` 排进 `key` 本体。旧写法是
            // `push_str(&format!("{num:020}"))` + `push_str(&(end-i).to_string())`——每枚数字段
            // 先造两枚中间 String 再抄一遍，一册 3000 项白付 0.4–0.8 ms（读数见 本仓历史记录；
            // 两法交出的键**逐字节相同**，`examples/frame_cost_probe.rs` 的钉子拿真目录的顺序钉着）。
            // `let _ =` 是有意的：`String` 的 `fmt::Write` 只在分配失败时报错，那时进程早已 abort。
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

/// 完整的已知格式清单（内置 ∪ 解码器自述），排序去重。
///
/// 给"打开文件"对话框的过滤器、设置页的格式统计用。内置那半来自
/// [`formats::BUILTIN_DECODERS`]（[`formats::builtin_exts`]），不再另有一份常量。
pub fn known_exts() -> Vec<String> {
    let mut v: Vec<String> = formats::builtin_exts()
        .into_iter()
        .chain(formats::declared_exts())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// 目录扫描专用：一次取好清单再逐个判定。
///
/// `declared_exts()` 会加锁并克隆一份集合——目录里几千个文件时每个文件都取一次
/// 是纯浪费，所以扫目录走这个函数，只查一次。
pub(crate) fn is_image_ext_in(path: &Path, declared: &std::collections::HashSet<String>) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let e = ext.as_str();
    formats::is_builtin_ext(e) || declared.contains(e)
}

/// Windows 上路径比较忽略大小写与分隔符方向（`\\` 与 `/`）。
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gallery(n: usize) -> Gallery {
        Gallery {
            dir: PathBuf::from("."),
            files: (0..n).map(|i| PathBuf::from(format!("{i}.png"))).collect(),
            index: 0,
        }
    }

    /// 自然序：`img2` 排在 `img10` 前，前导零位数少者排前，大小写不敏感。
    /// 走生产同款 `sort_natural`，避免"测的和跑的不是一个"。
    #[test]
    fn natural_order_sorts_numerically() {
        let names = ["img10.png", "IMG2.png", "img02.png", "img1.png", "img.jpg"];
        let mut files: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
        sort_natural(&mut files);
        let got: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            got,
            // 数字段按数值序（零填充后字典序=数值序）；数字段哨兵 \u{1} 排在
            // 标点 '.' 之前：带数字段的名字优先于纯文本段（img.jpg 殿后）。
            vec!["img1.png", "IMG2.png", "img02.png", "img10.png", "img.jpg"]
        );
    }

    #[test]
    fn same_listing_compares_paths_and_order() {
        let a = gallery(3);
        assert!(a.same_listing(&gallery(3)));
        let mut reordered = gallery(3);
        reordered.files.swap(0, 1);
        assert!(!a.same_listing(&reordered), "顺序变了必须算不同");
        let mut case_only = gallery(3);
        case_only.files[0] = PathBuf::from("0.PNG");
        assert!(a.same_listing(&case_only), "Windows 路径比较忽略大小写");
    }

    /// 超长数字段不 panic（parse 失败兜底 u64::MAX，退化为字典序）。
    #[test]
    fn natural_order_huge_numbers_do_not_panic() {
        let k1 = natural_key(std::ffi::OsStr::new("99999999999999999999999.png"));
        let k2 = natural_key(std::ffi::OsStr::new("0.png"));
        assert!(k2 < k1);
    }

    /// **翻页永远循环**（用户拍板 2026-10-09）：3/3 再下一张 → 1/3，
    /// 1/3 再上一张 → 3/3；步长超过目录长度也正确。
    #[test]
    fn step_looped_wraps_both_ways() {
        let mut g = gallery(3);
        assert_eq!(g.position(), 1, "初始在 1/3");
        assert!(g.step_looped(1));
        assert_eq!(g.position(), 2);
        assert!(g.step_looped(1));
        assert_eq!(g.position(), 3, "到最后一张");
        assert!(g.step_looped(1), "3/3 再下一张必须回绕");
        assert_eq!(g.position(), 1, "回绕到 1/3");
        assert!(g.step_looped(-1), "1/3 再上一张必须回绕");
        assert_eq!(g.position(), 3, "回绕到 3/3");

        // 大步长：-4 等价于 -1（mod 3）。
        let mut g2 = gallery(3);
        assert!(g2.step_looped(-4));
        assert_eq!(g2.position(), 3);
    }

    /// 单张目录翻页返回 false（回绕到自己等于没动），位置不变。
    #[test]
    fn single_image_never_moves() {
        let mut g = gallery(1);
        assert!(!g.step_looped(1));
        assert!(!g.step_looped(-1));
        assert_eq!(g.position(), 1);
        // 空目录同理。
        let mut e = gallery(0);
        assert!(!e.step_looped(1));
    }

    /// `rescan` 保留当前文件：兄弟项被删，分母变小、当前图前的项没了，但当前图不变。
    #[test]
    fn rescan_keeps_current_when_sibling_removed() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix_rescan_{}", std::process::id()));
        // 先清掉可能的残留：pid 会被 Windows 回收，同名目录里留着上个进程的文件 ⇒ 用例继承别人的状态（m106）
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let paths: Vec<PathBuf> = ["a", "b", "c"]
            .iter()
            .map(|n| {
                let p = dir.join(format!("{n}.svg"));
                std::fs::File::create(&p).unwrap();
                p
            })
            .collect();
        let g = Gallery::from_file(&paths[1]).unwrap(); // 当前 b（2/3）
        assert_eq!(g.position(), 2);
        // 删掉 a（当前图之前的那张），分母变 2，b 前移成第 1 位。
        std::fs::remove_file(&paths[0]).unwrap();
        let r = g.rescan().unwrap();
        assert_eq!(r.len(), 2, "目录只剩两张");
        assert_eq!(r.position(), 1, "b 前移成第 1 位");
        assert!(same_path(r.current().unwrap(), &paths[1]), "当前图仍是 b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `rescan` 当前图被删：落回相邻槽位，且不再指向已删的文件。
    #[test]
    fn rescan_falls_back_when_current_deleted() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix_rescan2_{}", std::process::id()));
        // 先清掉可能的残留：pid 会被 Windows 回收，同名目录里留着上个进程的文件 ⇒ 用例继承别人的状态（m106）
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let paths: Vec<PathBuf> = ["a", "b", "c"]
            .iter()
            .map(|n| {
                let p = dir.join(format!("{n}.svg"));
                std::fs::File::create(&p).unwrap();
                p
            })
            .collect();
        let g = Gallery::from_file(&paths[1]).unwrap(); // 当前 b（2/3）
        std::fs::remove_file(&paths[1]).unwrap(); // 删掉当前图
        let r = g.rescan().unwrap();
        assert_eq!(r.len(), 2);
        assert!(
            !same_path(r.current().unwrap(), &paths[1]),
            "当前项不应是已删的 b"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// m50-B 的承重夹具：一个目录里两张图（`.svg` 走宿主自认领的内置清单，与解码器在不在
    /// 无关）+ 一个**没人认领**的 `说明.txt`。测试之间按 pid 分名，先清一遍再建，
    /// 免得上一轮崩在半路留下的文件把清单撑大（那会让"分母"断言变成偶然）。
    fn m50_dir(tag: &str) -> PathBuf {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix_m50{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["a.svg", "b.svg", "说明.txt"] {
            std::fs::File::create(dir.join(n)).unwrap();
        }
        dir
    }

    /// ★ 拖进来的文件不在扩展名清单里 ⇒ **它就是当前项**，而不是"同目录第一张别的图"。
    /// 旧写法是 `position(…).unwrap_or(0)`：这条会红在"当前项是 a.svg"。
    #[test]
    fn unlisted_anchor_becomes_the_current_item() {
        let dir = m50_dir("a");
        let txt = dir.join("说明.txt");
        let g = Gallery::from_file(&txt).unwrap();
        assert_eq!(g.len(), 3, "两张图 + 用户点开的那一个");
        assert_eq!(g.position(), 3, "自然序里 `说明.txt` 排在两张 .svg 之后");
        assert!(
            same_path(g.current().unwrap(), &txt),
            "当前项必须是用户点开的那个文件，不是别人的第一张"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 锚点本来就在清单里 ⇒ 位置照旧，不多插一份。
    #[test]
    fn listed_anchor_keeps_its_slot() {
        let dir = m50_dir("b");
        let g = Gallery::from_file(&dir.join("b.svg")).unwrap();
        assert_eq!(g.len(), 2, "没认领的后缀不该进网格");
        assert_eq!(g.position(), 2);
        assert!(same_path(g.current().unwrap(), &dir.join("b.svg")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 拖进来的是**文件夹** ⇒ 打开这个文件夹（旧写法取 `parent()`，开的是它旁边的图）。
    #[test]
    fn folder_opens_that_folder() {
        let dir = m50_dir("c");
        let g = Gallery::from_file(&dir).unwrap();
        assert!(same_path(g.dir(), &dir), "清单的目录就是被拖进来的那个");
        assert_eq!(g.len(), 2, "只有两张 .svg 进清单");
        assert_eq!(g.position(), 1, "当前项＝自然序第一张");
        assert!(same_path(g.current().unwrap(), &dir.join("a.svg")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件夹里一张图也没有 ⇒ 说人话（`NoImages` 那句本来就有），不静默开别的目录。
    #[test]
    fn folder_without_images_reports_no_images() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix_m50d_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::File::create(dir.join("notes.md")).unwrap();
        let e = Gallery::from_file(&dir).unwrap_err();
        assert!(
            matches!(e, ViewError::NoImages(ref p) if same_path(p, &dir)),
            "判词要指着这个目录，不是别的：{e}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重扫不许把"用户点开的那个没后缀的文件"从清单里弄丢（否则分母会自己变小、
    /// 当前项跳走）；被删掉的那一张仍然要落回相邻槽位——上面两条已有用例守着。
    #[test]
    fn rescan_keeps_an_unlisted_anchor() {
        let dir = m50_dir("e");
        let txt = dir.join("说明.txt");
        let g = Gallery::from_file(&txt).unwrap();
        let r = g.rescan().unwrap();
        assert_eq!(r.len(), 3, "锚点还在磁盘上，重扫不许丢");
        assert!(same_path(r.current().unwrap(), &txt));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 免 stat 的快路只许走"缓存属性已经说是文件"那一支：**链到别处的图片必须仍然进清单**。
    ///
    /// 钉的是 [`listing`] 里 `match e.file_type()` 的 `_` 分支。把它写成看着等价的
    /// `Ok(t) => t.is_file()`（`bench_dir_listing.rs` 的 B 臂）时，`sym` 会被判成
    /// "不是文件"、清单少一张——那是用户翻不到那张图的**正确性**回归，不是性能差异。
    /// 断链的 `ghost` 与顶着图片后缀的目录 `dirlink` 反过来**不许**进清单，
    /// 所以这条用例是双向的：既防"少图"，也防"多出一张打不开的东西"。
    ///
    /// 后缀用 `.svg` 而不是 `.jpg`：单测进程里没有解码器，`declared_exts()` 只剩内置登记表，
    /// 而登记表里自动认领的只有 resvg（位图一律不内置）。用 `.jpg` 会连清单都
    /// 是空的，判据根本没被执行（本轮真踩过：先红了才发现是夹具不在分支里）。
    #[cfg(windows)]
    #[test]
    fn gallery_symlinked_images_stay_listed() {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix_m66links_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("real.svg"), b"<svg/>").unwrap();
        std::fs::hard_link(dir.join("real.svg"), dir.join("hard.svg")).unwrap();
        std::fs::create_dir(dir.join("dirlink.svg")).unwrap();
        // ★ 目标一律给**绝对路径**：`CreateSymbolicLinkW` 原样存目标，相对路径是相对
        //   "链接所在目录"解析的。传相对路径会让三条链接全成断链，于是 A 与 B"都挡掉"，
        //   用例绿得毫无意义（本轮真栽过一次）。
        let sym_ok = symlink_file(dir.join("real.svg"), dir.join("sym.svg")).is_ok();
        let ghost_ok = symlink_file(dir.join("gone.svg"), dir.join("ghost.svg")).is_ok();
        let dirlink_ok = symlink_dir(dir.join("dirlink.svg"), dir.join("junction.svg")).is_ok();
        if !sym_ok || !ghost_ok {
            eprintln!("本机不允许创建符号链接（无权限或未开开发者模式）：跳过，不算通过");
            let _ = std::fs::remove_file(dir.join("hard.svg"));
            let _ = std::fs::remove_file(dir.join("real.svg"));
            let _ = std::fs::remove_dir(dir.join("dirlink.svg"));
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let mut names: Vec<String> = Gallery::from_file(&dir)
            .unwrap()
            .files
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["hard.svg", "real.svg", "sym.svg"],
            "链到真文件的 sym.svg 必须进清单（少一张=用户翻不到）；\
             断链 ghost.svg、目录 dirlink.svg{}必须挡掉",
            if dirlink_ok {
                "、指向真目录的 junction.svg"
            } else {
                ""
            }
        );
        for n in [
            "sym.svg",
            "junction.svg",
            "ghost.svg",
            "hard.svg",
            "real.svg",
        ] {
            let _ = std::fs::remove_file(dir.join(n));
        }
        let _ = std::fs::remove_dir(dir.join("dirlink.svg"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
