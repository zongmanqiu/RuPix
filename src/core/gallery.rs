//! 目录内图片清单与导航。
//!
//! 打开一张图 = 打开它所在的**整个目录**：左右方向键在目录内翻页是看图器的基本盘，
//! 用户不会因为"只打开了一个文件"就接受不能翻。

use std::cmp::Ordering as Cmp;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

/// 目录清单的排序维度（ini 键 `sort_by`；右键「排序 ▸」四项、设置页同一个下拉）。
///
/// 判据是「文件自己的哪一个属性」：名字、字节数、创建时间、修改时间。**方向**不在这里，
/// 由 [`SortSpec::desc`] 单独一格——四个维度 × 两个方向，用户的心智模型是「先选按什么排、
/// 再选正序倒序」，与资源管理器的「排序方式 ▸」同构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    /// 文件名（默认）。自然序（`img2 < img10`），见 [`cmp_names`]。
    #[default]
    Name,
    /// 文件字节数。
    Size,
    /// 创建时间（`ftCreationTime`）。拷进来的旧图这个时间会是"拷贝那一刻"，
    /// 想按"什么时候拍的"排要看修改时间。
    Created,
    /// 修改时间（`ftLastWriteTime`）：用户最常说的"最新的那张"。
    Modified,
}

impl SortBy {
    /// ini 存储号（与菜单、设置页下拉同序）。
    pub fn from_index(i: i64) -> Self {
        match i {
            1 => SortBy::Size,
            2 => SortBy::Created,
            3 => SortBy::Modified,
            _ => SortBy::Name,
        }
    }

    pub fn index(self) -> i64 {
        match self {
            SortBy::Name => 0,
            SortBy::Size => 1,
            SortBy::Created => 2,
            SortBy::Modified => 3,
        }
    }

    /// 设置页下拉位置 / 菜单文案下标（与 [`Self::index`] 同序，但类型是 `usize`）。
    pub fn ui_index(self) -> usize {
        self.index() as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Name => "文件名",
            SortBy::Size => "大小",
            SortBy::Created => "创建日期",
            SortBy::Modified => "修改日期",
        }
    }

    pub const ALL: [SortBy; 4] = [
        SortBy::Name,
        SortBy::Size,
        SortBy::Created,
        SortBy::Modified,
    ];
}

/// 排序口径 = 维度 + 方向（ini 键 `sort_by` + `sort_desc`）。
///
/// `Copy` 且 `Default` 是**默认排序 = 文件名升序**这件事的落点：所有"没传口径"的
/// 构造（测试、探针、空清单）都自动落在与出厂一致的那一档，不会各写一份默认。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SortSpec {
    pub by: SortBy,
    /// 倒序。**整条比较反转**（主键与并列时的名字键一起翻），与资源管理器「倒序」同义。
    pub desc: bool,
}

impl SortSpec {
    /// 出厂口径：文件名升序。
    pub const DEFAULT: SortSpec = SortSpec {
        by: SortBy::Name,
        desc: false,
    };

    pub const fn new(by: SortBy, desc: bool) -> Self {
        Self { by, desc }
    }
}

/// 一个目录里的图片清单 + 当前位置。
#[derive(Debug, Clone, Default)]
pub struct Gallery {
    dir: PathBuf,
    files: Vec<PathBuf>,
    index: usize,
    /// 建这份清单时用的排序口径。`rescan` 照它重排——**清单自己记着自己是怎么排的**，
    /// 重扫不必再由调用方递一遍（递漏一次就会出现"翻个页顺序变了"）。
    sort: SortSpec,
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
    pub fn from_file(file: &Path, sort: SortSpec) -> ViewResult<Self> {
        // 拖进来的是**文件夹** = 打开这个文件夹。旧写法把它当"某张图"取 `parent()`，
        // 于是打开的是它旁边那个目录的第一张图。
        if file.is_dir() {
            return Self::from_dir(file, sort, false);
        }
        let dir = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let mut files = listing(&dir, sort)?;
        ensure_listed(&mut files, file, sort);
        // 锚点必在清单里（`ensure_listed` 刚保证过），这里的 `unwrap_or` 只是不让
        // 编译器替我们写 `unwrap`——`unlisted_anchor_becomes_the_current_item` 与
        // `listed_anchor_keeps_its_slot` 两条用例钉住两个分支。
        let index = files.iter().position(|p| same_path(p, file)).unwrap_or(0);
        Ok(Self {
            dir,
            files,
            index,
            sort,
        })
    }

    /// 建一个「目录清单」。`allow_empty` 为真时，空目录也返回一个「有清单、零项」的对象。
    ///
    /// 只有压缩包的**层**用得上：那一层的子目录 / 子包才是可进入项，画面空着是正常态
    /// （用户停在 `1M.zip/ch1` 这种只有子目录的地方，右键还能继续往下），不是"打开失败"。
    /// 普通目录仍走 [`Self::from_file`]，空目录照旧报 `NoImages`——那是缺陷不是常态。
    pub fn from_dir(dir: &Path, sort: SortSpec, allow_empty: bool) -> ViewResult<Self> {
        let files = listing(dir, sort)?;
        if files.is_empty() && !allow_empty {
            return Err(ViewError::NoImages(dir.to_path_buf()));
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            files,
            index: 0,
            sort,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 本清单的排序口径（右键菜单据此打勾，重扫据此重排）。
    pub fn sort(&self) -> SortSpec {
        self.sort
    }

    /// 换一个排序口径重排本清单，**锚点（当前这张）不变**。
    ///
    /// 返回 `true` = 顺序真的变了（调用方据此清掉按**槽位**记的界面状态，例如
    /// 缩略图条"用户滚到的首格"——重排之后那个槽位已经是另一张图）。
    /// 目录读不到（被拔盘 / 删了）时保留旧清单：排序偏好换了不该让画面空掉。
    pub fn set_sort(&mut self, sort: SortSpec) -> bool {
        if self.sort == sort {
            return false;
        }
        self.sort = sort;
        if self.dir.as_os_str().is_empty() {
            return false;
        }
        match self.rescan() {
            Ok(g) => {
                let changed = !self.same_listing(&g);
                *self = g;
                changed
            }
            Err(_) => false,
        }
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
    /// 调用方（`crate::core::Viewer::step`）据此决定要不要重置视图/重解码。
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
    /// 与 [`Gallery::from_file`] 同源的枚举 + 过滤 + 排序（口径取本清单自己的），保证"重扫出来的
    /// 清单"和"初次打开的清单"完全一致，不会因实现分叉出现顺序漂移。
    pub fn rescan(&self) -> ViewResult<Self> {
        self.rescan_with(listing(&self.dir, self.sort)?)
    }

    /// 用**别处刚扫出来的**清单重建（[`listing_for_event`] 的复用分支走这里）。
    ///
    /// 克隆一份 `Vec` 而不共享 `Arc`：`rescan_with` 可能要往里插锚点（[`ensure_listed`]），
    /// 而缓存里那份要留给后面的窗口。1200 项实测克隆 ~30 µs，比一次 `read_dir` 便宜两个量级。
    pub fn rescan_from(&self, files: std::sync::Arc<Vec<PathBuf>>) -> ViewResult<Self> {
        self.rescan_with((*files).clone())
    }

    fn rescan_with(&self, mut files: Vec<PathBuf>) -> ViewResult<Self> {
        let old_index = self.index;
        let anchor = self.current().cloned();
        // 锚点可能是"用户显式点开的那一个"，未必带图片后缀（见 [`ensure_listed`]）：
        // 它还在磁盘上就继续留在清单里，被外部删掉/改名才落回相邻槽位。
        if let Some(a) = &anchor {
            if a.is_file() {
                ensure_listed(&mut files, a, self.sort);
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
            sort: self.sort,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jump {
    First,
    Last,
}

/// 枚举时顺手收下的那一份**排序素材**。
///
/// 三个字段全部来自 `read_dir` 已经递到手的 `WIN32_FIND_DATAW`（见 [`listing`] 里的
/// 取证注释），所以"按大小/时间排"**不多付一次系统调用**。
struct Found {
    path: PathBuf,
    /// 字节数。
    size: u64,
    /// 创建 / 修改时刻（UNIX 纪元起的纳秒；读不到或早于 1970 记 `None`，升序时排末位）。
    created: Option<u64>,
    modified: Option<u64>,
}

/// 一次枚举的临时产物，只在 [`listing`] 里活一趟。
type FoundList = Vec<Found>;

/// 枚举一个目录里的图片清单：扩展名过滤 + 排序。**`from_file` 与 `rescan` 共用**，
/// 否则两份清单会因实现分叉而顺序漂移（`rescan` 的注释早就写了这条要求）。
///
/// ★ 这里**不做**逐文件魔数嗅探：那是 O(目录项数 × 解码器数)，每次开图都要付一遍。
///   代价是"目录里其他无后缀但魔数认得的文件"不进网格（翻不到）——罕见形态不值这笔钱。
///   用户**显式点开**的那一个例外，由 [`ensure_listed`] 兜住。
pub(crate) fn listing(dir: &Path, sort: SortSpec) -> ViewResult<Vec<PathBuf>> {
    // 清单只取一次（见 `is_image_ext_in` 的注释）。
    let declared = formats::declared_exts();
    let mut found: FoundList = Vec::new();
    for e in fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let path = e.path();
        // 后缀判据在前、"是不是文件"在后：两条是 `&&` 关系，换顺序不改清单，
        // 但能让没人认领的项（`.txt`/`.md` 那一堆）根本不进入贵的那条。
        if !is_image_ext_in(&path, &declared) {
            continue;
        }
        // ★ 快路只走"枚举时缓存的属性已经说是文件"这一支，其余（目录、符号链接、
        //   Unknown、读不到）一律照旧再来一次 `metadata`——一次 `read_dir` 已经把
        //   `WIN32_FIND_DATAW` 的字节属性递到手了，普通文件不必再 `GetFileAttributesW`。
        //   实测（`examples/bench_dir_listing.rs`，同进程五臂、清单逐项比过才谈速度）：
        //   3000 项目录整段 54–66 ms → 3.4–3.8 ms，拆段里 stat 那条占 87%。
        //   **不能**直接把判据换成 `file_type().is_file()`：它判的是**链接本身**而不是
        //   目标，链到别处的 `.jpg` 会被判成"不是文件"、从清单里消失——那条回归由
        //   `links` 夹具当场抓出来（见 `gallery_symlinked_images_stay_listed`）。
        // ★ 排序素材（大小 / 两个时间）就在**这一次** `metadata()` 里一起拿走：
        //   std 在 Windows 上的 `DirEntry::metadata` 是 `Ok(self.data.into())`——纯结构体
        //   拷贝，零系统调用。反过来，出了这个循环再按**路径**补 `fs::metadata()` 就是
        //   一条真 syscall：3000 项实测 5.5 ms → 41.3 ms（差 14 倍，读法见
        //   `docs/待办.md` §55）。**判据：元数据只在枚举循环里取一次。**
        let meta = match e.file_type() {
            Ok(t) if t.is_file() => e.metadata().ok(),
            _ => fs::metadata(&path).ok().filter(|m| m.is_file()),
        };
        let Some(meta) = meta else { continue };
        found.push(Found {
            size: meta.len(),
            created: epoch_nanos(meta.created().ok()),
            modified: epoch_nanos(meta.modified().ok()),
            path,
        });
    }
    Ok(sort_found(found, sort))
}

/// 清单里那一项的文件名（非 UTF-8 时损失性转换）。排序、状态行、缩略图条都按它认人。
fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `SystemTime` → UNIX 纪元起的纳秒。取不到（或早于 1970）记 `None`——排序时排末位，
/// 而不是伪造一个 0 让人误以为"这张比 1970 还老"。
fn epoch_nanos(t: Option<std::time::SystemTime>) -> Option<u64> {
    let d = t?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_nanos().min(u64::MAX as u128) as u64)
}

/// 名字比较：自然序（数字段按**数值**比），并列再按码点定序。
///
/// ### 为什么不用系统的 `StrCmpLogicalW`（m275 试过又退回）
///
/// 它才是"与资源管理器逐字同序"的权威口径，实测 3000 项 2.8 ms 也不贵。但两条硬伤：
/// ① `build/layering_scan.py` 的**可测性红线**——`core` 层禁引 `windows`（纯逻辑、单测
/// 必须能无头跑），这不是风格偏好，是"低层不认识平台"的分层底线；
/// ② 它随系统 locale 变：同一份清单换台机器可能排出另一种顺序，`gallery` 的用例也就
/// 跟着漂——用例一旦依赖机器状态，它就不再是判据。
///
/// 代价是**与资源管理器不完全同序**（实测 `a b` / `a-b` / `a_b` 的先后与它不同），
/// 但这只影响"清单次序与文件夹窗口像不像"，不影响 `img2 < img10` 这条真正要命的自然后序。
///
/// ### 实现
///
/// 双指针**原地比**，不建键（旧实现每项都 `format!` 一串零填充数字，3000 项要多花
/// 一半时间）：数字段只比（去前导零后的位数，再逐位）；其余按 `to_ascii_lowercase`
/// 后的字节比——UTF-8 字节序与码点序一致，中文也照排。
fn cmp_names(a: &str, b: &str) -> Cmp {
    cmp_natural(a, b).then_with(|| a.cmp(b))
}

/// 自然序本体。并列（`a01` 与 `a1`、只差大小写）交给 [`cmp_names`] 的码点兜底：
/// **不许留并列**，否则同名的两项会按 `read_dir` 的枚举次序排，"倒序"看起来像随机。
fn cmp_natural(a: &str, b: &str) -> Cmp {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        // 一方读完：**先读完的排前**（`img.png` 在 `img1.png` 之前），两边同时读完 = 相等。
        //
        // ★ 不能用"比两个串的总长"代替：走过数字段之后两个下标可能已经错开
        // （`a1.png` 与 `a01.png` 逐字节长度差 1，但规范化后是同一串），拿总长比会
        // 判出差一截的先后——实测就是这样把 `a1` 判到了 `a01` 前面。
        if i >= ab.len() || j >= bb.len() {
            return (j >= bb.len()).cmp(&(i >= ab.len()));
        }
        if ab[i].is_ascii_digit() && bb[j].is_ascii_digit() {
            let (mut ie, mut je) = (i, j);
            while ie < ab.len() && ab[ie].is_ascii_digit() {
                ie += 1;
            }
            while je < bb.len() && bb[je].is_ascii_digit() {
                je += 1;
            }
            // 去前导零再比：位数不同时位数多的大；位数相同才逐字节比。
            // 这样 `007` 与 `7` 相等（交给码点兜底定序），`10` 一定大于 `9`。
            let (mut p, mut q) = (i, j);
            while p < ie && ab[p] == b'0' {
                p += 1;
            }
            while q < je && bb[q] == b'0' {
                q += 1;
            }
            let ord = (ie - p)
                .cmp(&(je - q))
                .then_with(|| ab[p..ie].cmp(&bb[q..je]));
            if ord != Cmp::Equal {
                return ord;
            }
            i = ie;
            j = je;
            continue;
        }
        let (ca, cb) = (ab[i].to_ascii_lowercase(), bb[j].to_ascii_lowercase());
        if ca != cb {
            return ca.cmp(&cb);
        }
        i += 1;
        j += 1;
    }
}

/// 按口径排序并交出路径。Schwartzian 变体：名字与元数据先备好，比较器只读不建。
fn sort_found(found: FoundList, sort: SortSpec) -> Vec<PathBuf> {
    // 名字先取一次（`to_string_lossy` 每项一次也就够了）：比较器里现取 file_name
    // 就是 O(n log n) 次 `Path::file_name` + 解码，3000 项实测能从 1.8 ms 翻到 5 ms 以上。
    let names: Vec<String> = found.iter().map(|f| file_name_of(&f.path)).collect();
    let mut order: Vec<usize> = (0..found.len()).collect();
    order.sort_by(|&a, &b| {
        let primary = match sort.by {
            SortBy::Name => Cmp::Equal,
            SortBy::Size => found[a].size.cmp(&found[b].size),
            SortBy::Created => found[a].created.cmp(&found[b].created),
            SortBy::Modified => found[a].modified.cmp(&found[b].modified),
        };
        // 并列一律用名字定序：少了这一条，同大小 / 同秒的一批图会按枚举次序排，
        // "倒序"看起来就像随机。
        let ord = primary.then_with(|| cmp_names(&names[a], &names[b]));
        if sort.desc {
            ord.reverse()
        } else {
            ord
        }
    });
    let mut found = found;
    order
        .into_iter()
        .map(|i| std::mem::take(&mut found[i].path))
        .collect()
}

// ── 事件驱动重扫的"同批只扫一次" ─────────────────────────────────────────────
//
// 为什么需要：每扇看图窗都有自己的 `DirWatcher`，所以一次文件变动会被**每扇窗各报一次**，
// 同一次变动在同一瞬间被重扫 N 遍，全落在同一条 UI 线程上。实测 1200 项目录、5 扇窗：
// 5 条重扫合计 8.25 ms（`build/dir_rescan_probe.ps1`）；按 3401 项 3.5 ms/次
// （`examples/bench_dir_listing.rs`）外推，1 万项目录 5 扇窗就是 ~50 ms——一次可见的顿挫。
//
// ★ 只服务**事件驱动**那条路（`Viewer::rescan_dir_shared`）。`Gallery::rescan` 与
//   `from_file` 照旧真读盘：它们的语义是"现在去看这个目录"。第一版把缓存塞进了枚举函数
//   本身，四条既有用例当场把它抓出来（删掉文件后 `rescan` 看不见，等于凭空造陈旧）——
//   收藏别人的扫描结果可以，冒充自己的扫描结果不行。
//
// 失效判据只有一条：**代次**。任何物理变化由 watcher 线程递增（[`note_dir_change`]），
// 所以"上一批事件的结果"绝不会被下一批复用。不需要时间窗：这一格只在重扫被调用时读，
// 而重扫只由事件触发。

/// 「某个被监控的目录变了」的进程级代次，由 [`note_dir_change`] 递增。
static DIR_EPOCH: AtomicU64 = AtomicU64::new(0);

/// 真实枚举次数（复用不计数）。只服务取证行 `[dir-rescan] shared=`：让"这次是复用还是
/// 真扫"从推断变成读数。
static SCAN_COUNT: AtomicU64 = AtomicU64::new(0);

struct BatchListing {
    epoch: u64,
    dir: PathBuf,
    /// 这一份是按哪个口径排的。**必须进复用判据**：两扇窗共用一份缓存，
    /// 而口径是全局配置——端口径不同就会把别人的顺序塞给你。
    sort: SortSpec,
    files: Arc<Vec<PathBuf>>,
}

static BATCH: Mutex<Option<BatchListing>> = Mutex::new(None);

/// 叫一声"这个进程里某个被监控的目录变了"。**只由 `DirWatcher` 的监控线程调用**。
///
/// N 扇窗就有 N 个 watcher，于是一次变化会在这里递增 N 次。这是故意的：代次只需单调，
/// 多推几次只会让复用更早失效（偏保守），不会更晚。
pub fn note_dir_change() {
    DIR_EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// 进程内已发生的真实枚举次数。
pub fn scan_count() -> u64 {
    SCAN_COUNT.load(Ordering::Relaxed)
}

/// 事件驱动重扫的清单来源：本代次里已经有人扫过这个目录就把那份交出来，否则真扫并登记。
///
/// 返回的第二项 = "这次真的读了盘"（取证行用它区分复用与真扫）。
pub fn listing_for_event(dir: &Path, sort: SortSpec) -> ViewResult<(Arc<Vec<PathBuf>>, bool)> {
    // 代次在枚举**之前**读：万一枚举期间来了新事件，登记进去的是旧代次，后面的窗口会
    // 重扫一遍（偏保守）。反过来写会让"扫到一半、变了一半"的结果被当成最新的一份。
    let epoch = DIR_EPOCH.load(Ordering::Relaxed);
    {
        let guard = BATCH.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = guard.as_ref() {
            if b.epoch == epoch && b.dir == dir && b.sort == sort {
                return Ok((Arc::clone(&b.files), false));
            }
        }
    }
    // 锁只护着那一格，不护读盘本身：单 UI 线程下没有竞争者，而将来若把打开图片挪到
    // 后台线程，也不会有人被堵在别人的 `read_dir` 上。
    let files = Arc::new(listing(dir, sort)?);
    SCAN_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut guard = BATCH.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(BatchListing {
        epoch,
        dir: dir.to_path_buf(),
        sort,
        files: Arc::clone(&files),
    });
    Ok((files, true))
}

/// 保证 `file` 在清单里：不在（无后缀 / 后缀没人认领 / 根本不是图片）就插进去再重排。
///
/// 判据是"打开什么就该看见什么"，不是"这文件像不像图片"——后者由解码层给判词，
/// 而且它给的比我们猜的准（真的去嗅探过、也真的解过）。
///
/// 插进来的这一枚要按**同一个口径**重新定位：按名字排时它就是一次重排；按大小 /
/// 时间排时得先拿到它自己的元数据——单个文件一次 `fs::metadata` 是这条冷路径上
/// 唯一付得起的一笔（整目录那条热路径见 [`listing`] 的取证注释）。
fn ensure_listed(files: &mut Vec<PathBuf>, file: &Path, sort: SortSpec) {
    if files.iter().any(|p| same_path(p, file)) {
        return;
    }
    let mut found: FoundList = files
        .drain(..)
        .map(|p| Found {
            size: 0,
            created: None,
            modified: None,
            path: p,
        })
        .collect();
    // 原有项的元数据要补：按大小 / 时间排时，缺了它那一项会掉到最前面。
    if sort.by != SortBy::Name {
        for f in found.iter_mut() {
            if let Ok(m) = fs::metadata(&f.path) {
                f.size = m.len();
                f.created = epoch_nanos(m.created().ok());
                f.modified = epoch_nanos(m.modified().ok());
            }
        }
    }
    let m = fs::metadata(file).ok();
    found.push(Found {
        size: m.as_ref().map(|m| m.len()).unwrap_or(0),
        created: epoch_nanos(m.as_ref().and_then(|m| m.created().ok())),
        modified: epoch_nanos(m.as_ref().and_then(|m| m.modified().ok())),
        path: file.to_path_buf(),
    });
    files.extend(sort_found(found, sort));
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

    /// 一次性临时目录（名字带 pid + 纳秒，跑多少轮都不撞）。
    fn scratch(tag: &str) -> PathBuf {
        let p = crate::core::paths::test_scratch_root().join(format!(
            "rupix-listing-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("临时目录要能建");
        p
    }

    /// 同代次里两扇窗的重扫只该真读一次盘，第二扇拿到的是**同一份**清单。
    ///
    /// 这是多开那条优化的核心：一次文件变动会被每扇窗各报一次，而它们看的是同一张目录表。
    #[test]
    fn one_event_batch_is_scanned_once() {
        let dir = scratch("same");
        std::fs::write(dir.join("a.svg"), b"<svg/>").expect("写夹具");
        note_dir_change(); // 让此前任何登记作废，起点干净
        let before = scan_count();
        let (a, scanned_a) = listing_for_event(&dir, SortSpec::DEFAULT).expect("第一扇窗要能枚举");
        let (b, scanned_b) = listing_for_event(&dir, SortSpec::DEFAULT).expect("第二扇窗也要能");
        assert!(scanned_a, "第一扇窗必须真扫");
        assert!(!scanned_b, "第二扇窗该复用，不该再读盘");
        assert_eq!(scan_count() - before, 1, "同一代次只该真扫一次");
        assert!(
            Arc::ptr_eq(&a, &b),
            "复用要拿到同一份清单，不许有第二份内存"
        );
        assert_eq!(a.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 来了新事件就必须重扫——复用的是"这一批的结果"，不是"永远的结果"。
    #[test]
    fn a_new_event_forces_a_fresh_scan() {
        let dir = scratch("fresh");
        std::fs::write(dir.join("a.svg"), b"<svg/>").expect("写夹具");
        note_dir_change();
        let before = scan_count();
        let (a, _) = listing_for_event(&dir, SortSpec::DEFAULT).expect("先扫一次");
        // 物理变化：新文件 + 代次递增（生产里由 DirWatcher 的监控线程做这两件事）。
        std::fs::write(dir.join("b.svg"), b"<svg/>").expect("写新夹具");
        note_dir_change();
        let (b, scanned) = listing_for_event(&dir, SortSpec::DEFAULT).expect("再扫一次");
        assert!(scanned, "新事件之后必须真扫");
        assert!(!Arc::ptr_eq(&a, &b), "新事件之后不许复用旧清单");
        assert_eq!(scan_count() - before, 2);
        assert_eq!(b.len(), 2, "新扫必须看见刚加进去的那个文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不同目录各算各的：这一格按整条路径匹配，不许把一个目录的清单借给另一个。
    #[test]
    fn two_directories_do_not_share_a_batch() {
        let d1 = scratch("d1");
        let d2 = scratch("d2");
        std::fs::write(d1.join("only-in-one.svg"), b"<svg/>").expect("写夹具");
        std::fs::write(d2.join("only-in-two.svg"), b"<svg/>").expect("写夹具");
        note_dir_change();
        let before = scan_count();
        let (a, _) = listing_for_event(&d1, SortSpec::DEFAULT).expect("扫第一个");
        let (b, scanned_b) = listing_for_event(&d2, SortSpec::DEFAULT).expect("扫第二个");
        assert!(scanned_b, "换了目录就得真扫");
        assert_eq!(scan_count() - before, 2, "两个目录就是两次枚举");
        assert_eq!(
            a[0].file_name().map(|n| n.to_owned()),
            Some("only-in-one.svg".into())
        );
        assert_eq!(
            b[0].file_name().map(|n| n.to_owned()),
            Some("only-in-two.svg".into())
        );
        let _ = std::fs::remove_dir_all(&d1);
        let _ = std::fs::remove_dir_all(&d2);
    }

    /// ★ 回归钉子：`Gallery::rescan`（"现在去看这个目录"）**永远不许**拿上一批的清单。
    ///
    /// 第一版把复用塞进了枚举函数本身，四条既有用例当场红（删掉文件后 `rescan` 看不见）。
    /// 这条把那个形态单独钉死，免得以后又"顺手"把缓存下沉一层。
    #[test]
    fn rescan_never_serves_a_stale_batch() {
        let dir = scratch("nostale");
        let f = dir.join("a.svg");
        std::fs::write(&f, b"<svg/>").expect("写夹具");
        std::fs::write(dir.join("b.svg"), b"<svg/>").expect("写夹具");
        let g = Gallery::from_file(&f, SortSpec::DEFAULT).expect("先建清单");
        assert_eq!(g.len(), 2);
        // 让事件那条路先扫一遍，并把它登记在本代次里……
        note_dir_change();
        let _ = listing_for_event(&dir, SortSpec::DEFAULT).expect("事件路径扫一次");
        // ……然后磁盘变了（生产里 watcher 会 bump；这里直接调 rescan 模拟"就是现在"）。
        std::fs::remove_file(dir.join("b.svg")).expect("删夹具");
        let g2 = g.rescan().expect("重扫");
        assert_eq!(g2.len(), 1, "rescan 必须看见磁盘现状，不许拿上一批的清单");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 内存里的假清单（不碰磁盘），给纯逻辑用例用。
    fn gallery(n: usize) -> Gallery {
        Gallery {
            dir: PathBuf::from("."),
            files: (0..n).map(|i| PathBuf::from(format!("{i}.png"))).collect(),
            index: 0,
            sort: SortSpec::DEFAULT,
        }
    }

    /// 只按名字排的一组（不碰盘）：`sort_found` 与生产同一条路。
    fn sorted_names(names: &[&str]) -> Vec<String> {
        let found: FoundList = names
            .iter()
            .map(|n| Found {
                path: PathBuf::from(n),
                size: 0,
                created: None,
                modified: None,
            })
            .collect();
        sort_found(found, SortSpec::DEFAULT)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    /// 名字序 = **自然序**（数字段按数值），这是我们自己的口径，不依赖系统 locale。
    ///
    /// 期望值是照 [`cmp_natural`] 的规则算的，不是"我觉得应该"：数字段按（去前导零后的
    /// 位数，再逐位）排，所以 `img1` < `IMG2`（`2` > `1`）< `img10`（两位数）；
    /// 非数字段按小写字节比，所以 `img.` 打头；`IMG2` 与 `img02` 数值相同 ⇒ 并列，
    /// 由码点兜底（大写在前）。
    #[test]
    fn name_order_is_natural_and_deterministic() {
        assert_eq!(
            sorted_names(&["img10.png", "IMG2.png", "img02.png", "img1.png", "img.jpg"]),
            vec!["img.jpg", "img1.png", "IMG2.png", "img02.png", "img10.png"]
        );
        // 只差大小写：自然序判成并列 ⇒ 由码点兜底定序（大写在前）。**必须是确定的那一种**：
        // 留并列就等于让 `read_dir` 的枚举次序决定画面顺序，同一批文件换个盘符就反过来。
        assert_eq!(
            sorted_names(&["b.jpg", "B.jpg", "a.JPG", "a.jpg"]),
            vec!["a.JPG", "a.jpg", "B.jpg", "b.jpg"]
        );
        // 前导零不同、数值相同：并列 ⇒ 码点兜底（`a01` 在 `a1` 前，因为 '0' < '1'）。
        assert_eq!(
            sorted_names(&["a1.png", "a01.png", "a001.png"]),
            vec!["a001.png", "a01.png", "a1.png"]
        );
    }

    /// 任意长的数字段都不许 panic，而且仍是**数值**序（不是字符串序）。
    ///
    /// 旧实现零填充到 u64 宽度、超长就兜底 `u64::MAX`；现在只比"去前导零后的位数，
    /// 再逐位"，23 位的数照排不误，也不会有任何溢出路径。
    #[test]
    fn huge_digit_runs_sort_numerically_without_panicking() {
        assert_eq!(
            sorted_names(&["99999999999999999999999.png", "0.png"]),
            vec!["0.png", "99999999999999999999999.png"]
        );
        // 位数相同才逐位比：21 位里最后一位不同的两个数。
        assert_eq!(
            sorted_names(&["100000000000000000002.png", "100000000000000000001.png"]),
            vec!["100000000000000000001.png", "100000000000000000002.png"]
        );
    }

    /// 按大小排：真的读到了字节数（不是全 0 的占位），并列时按名字定序。
    #[test]
    fn size_order_reads_real_lengths_and_breaks_ties_by_name() {
        let dir = scratch("size");
        std::fs::write(dir.join("big.svg"), vec![0u8; 300]).expect("写夹具");
        std::fs::write(dir.join("mid.svg"), vec![0u8; 200]).expect("写夹具");
        std::fs::write(dir.join("small_b.svg"), vec![0u8; 100]).expect("写夹具");
        std::fs::write(dir.join("small_a.svg"), vec![0u8; 100]).expect("写夹具");
        let asc = |desc: bool| -> Vec<String> {
            Gallery::from_file(&dir, SortSpec::new(SortBy::Size, desc))
                .unwrap()
                .files
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                .collect()
        };
        assert_eq!(
            asc(false),
            vec!["small_a.svg", "small_b.svg", "mid.svg", "big.svg"],
            "升序：小的在前；同大小按名字（small_a < small_b）"
        );
        assert_eq!(
            asc(true),
            vec!["big.svg", "mid.svg", "small_b.svg", "small_a.svg"],
            "倒序是整条比较翻转（含并列时的名字次序），与资源管理器「倒序」同义"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 换口径重排：**当前这张还是这张**，位置跟着新顺序走。
    ///
    /// 这条是整件事的用户价值所在——换排序不该把正在看的图换掉，也不该让画面闪。
    #[test]
    fn set_sort_reorders_but_keeps_the_current_file() {
        let dir = scratch("resort");
        std::fs::write(dir.join("a.svg"), vec![0u8; 10]).expect("写夹具");
        std::fs::write(dir.join("b.svg"), vec![0u8; 30]).expect("写夹具");
        std::fs::write(dir.join("c.svg"), vec![0u8; 20]).expect("写夹具");
        let mut g = Gallery::from_file(&dir.join("a.svg"), SortSpec::DEFAULT).unwrap();
        assert_eq!(g.current().unwrap().file_name().unwrap(), "a.svg");
        assert_eq!(g.position(), 1, "文件名升序：a 在第 1 张");
        assert!(g.set_sort(SortSpec::new(SortBy::Size, false)), "顺序该变");
        assert_eq!(
            g.current().unwrap().file_name().unwrap(),
            "a.svg",
            "换排序不许换掉当前这张"
        );
        assert_eq!(g.position(), 1, "a 最小（10 字节），按大小升序仍是第 1 张");
        assert!(g.set_sort(SortSpec::new(SortBy::Size, true)), "方向变了");
        assert_eq!(
            g.current().unwrap().file_name().unwrap(),
            "a.svg",
            "倒序之后当前项仍是 a"
        );
        assert_eq!(g.position(), 3, "a 最小，倒序落到最后一张");
        assert!(
            !g.set_sort(SortSpec::new(SortBy::Size, true)),
            "同口径不该重排"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let g = Gallery::from_file(&paths[1], SortSpec::DEFAULT).unwrap(); // 当前 b（2/3）
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
        let g = Gallery::from_file(&paths[1], SortSpec::DEFAULT).unwrap(); // 当前 b（2/3）
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
        let g = Gallery::from_file(&txt, SortSpec::DEFAULT).unwrap();
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
        let g = Gallery::from_file(&dir.join("b.svg"), SortSpec::DEFAULT).unwrap();
        assert_eq!(g.len(), 2, "没认领的后缀不该进网格");
        assert_eq!(g.position(), 2);
        assert!(same_path(g.current().unwrap(), &dir.join("b.svg")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 拖进来的是**文件夹** ⇒ 打开这个文件夹（旧写法取 `parent()`，开的是它旁边的图）。
    #[test]
    fn folder_opens_that_folder() {
        let dir = m50_dir("c");
        let g = Gallery::from_file(&dir, SortSpec::DEFAULT).unwrap();
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
        let e = Gallery::from_file(&dir, SortSpec::DEFAULT).unwrap_err();
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
        let g = Gallery::from_file(&txt, SortSpec::DEFAULT).unwrap();
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
        let mut names: Vec<String> = Gallery::from_file(&dir, SortSpec::DEFAULT)
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
