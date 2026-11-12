//! 压缩包来源：把 `.zip` / `.cbz` 的**某一层**落成 `<exe 目录>/temp/` 下的真实文件。
//!
//! # 一次只开一层
//!
//! 打开 `1M_2.zip` 看到的是它**根下**那 13 张图，内层 `1M.zip` 只是清单外的一个
//! 「可进入项」；点进去才是内层那 11 张。子目录同理——`cover/001.jpg` 在打开外层时
//! **不出现**，`cover` 自己是一个可进入项。这与资源管理器一致：不展开就看不得里面。
//!
//! 早先那版是「一次性递归整棵树、压成一条序列」（m295 v0）。那是错的：外层与内层混在
//! 一条线上只能靠文件名猜层级，不看的层也要解压、要占盘，而且 5000 层嵌套会在被拒之前
//! 让**整包**打不开——哪怕外层那 13 张图完好。现在没人点进去的层一次都不碰。
//!
//! # 为什么落成真实文件
//!
//! `core::gallery::listing` 只做单层 `fs::read_dir`，解码请求也只吃 `PathBuf`。包内条目
//! 一旦是磁盘上的真文件，翻页 / 胶片条 / 排序 / 跳转 / 预读 / EXIF / 空格预览整条浏览链
//! 一个字都不用改。所以「进入下一层」在宿主眼里就是**换一个目录再 open 一次**。
//!
//! # 会话目录
//!
//! ```text
//! <exe>/temp/rupix-zip-<包哈希>/
//!   L<层哈希>/            ← 这一层落盘的图片 + subs.tsv + layer.tsv + .rupix-ready
//!   _nest/<层>-<序>.zip   ← 为了进入深层而解出来的中间包
//! ```
//!
//! 层目录按「路径段」单独哈希，所以来回切层不会互相覆盖。包一变（路径 + 大小 + 修改
//! 时刻）就是另一个会话目录，旧的留给退出清理。
//!
//! 清理只有一处：GUI 进程退出时的 `cleanup_all`。运行期**绝不扫别人的目录**——早先
//! 那版「展开时清掉未在册目录」实测把另一枚探针的成果删了（踩坑记录 K202）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::core::paths;

/// 认领的容器后缀（小写、无点）。给打开对话框的过滤器用。
///
/// ★ `.gz` / `.xz` / `.bz2` / `.zst` **单独出现不算包**——那是"压一个文件"，包里没有
/// "第 3 张图"这种东西。只有 `xxx.tar.gz` / `xxx.tgz` 才认。所以真正的判据是
/// `kind()`，这个清单只负责"对话框里看得见"。
pub const ARCHIVE_EXTS: &[&str] = &[
    "7z", "bz2", "cbr", "cb7", "cbz", "gz", "rar", "tar", "tgz", "xz", "zip", "zst",
];

/// 容器种类。`Tar` 带一层外压缩，因为 **tar 自己不压**。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Zip,
    SevenZ,
    Rar,
    Tar(Option<Comp>),
}

/// tar 外面套的那层流式压缩。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Comp {
    Gz,
    Xz,
    Bz2,
    Zst,
}

/// 路径 → 容器种类。认出复合后缀是这里唯一有坑的地方：`a.tar.gz` 的 stem 是 `a.tar`。
fn kind(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let (stem, ext) = name.rsplit_once('.')?;
    match ext {
        "zip" | "cbz" => Some(Kind::Zip),
        "7z" | "cb7" => Some(Kind::SevenZ),
        "rar" | "cbr" => Some(Kind::Rar),
        "tar" => Some(Kind::Tar(None)),
        "tgz" => Some(Kind::Tar(Some(Comp::Gz))),
        "gz" => is_tar_stem(stem).then_some(Kind::Tar(Some(Comp::Gz))),
        "xz" => is_tar_stem(stem).then_some(Kind::Tar(Some(Comp::Xz))),
        "bz2" => is_tar_stem(stem).then_some(Kind::Tar(Some(Comp::Bz2))),
        "zst" => is_tar_stem(stem).then_some(Kind::Tar(Some(Comp::Zst))),
        _ => None,
    }
}

fn is_tar_stem(stem: &str) -> bool {
    stem.ends_with(".tar")
}

const SESSION_PREFIX: &str = "rupix-zip-";
const READY_MARK: &str = ".rupix-ready";
const SUBS_FILE: &str = "subs.tsv";
const LAYER_FILE: &str = "layer.tsv";
const NEST_DIR: &str = "_nest";

/// 一层里允许落盘的条目数（不含子包）。
const MAX_ENTRIES: usize = 2000;
/// 一层里解压后的总字节。
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
/// 建树那一趟「顺手把别的层也落盘」的整会话预算（m302）。
///
/// 不是安全边界，是磁盘护栏：超过就停止预落其余层，切到它们时退回按需展开。
/// 单条 / 单层 / 层数的旧闸门照旧，这条只管「多写的那部分」。
const MAX_ALL_LAYERS_BYTES: u64 = 512 * 1024 * 1024;
/// 单个条目（一张图）的上限。
const MAX_ONE: u64 = 64 * 1024 * 1024;
/// 为了进入深层而解壳的中间包体积上限：它不是图，尺度另算。
const MAX_NEST: u64 = 512 * 1024 * 1024;
/// 一次能点进去的层数（每点一次 +1）。不是安全边界，只是防呆。
const MAX_SEGS: usize = 32;
/// RAR 的**单条**上限。unrar 只给"整块读进内存"的 `read()`，没有流式接口，
/// 所以这条闸门同时管图片与内层包——超了就跳过并计数，绝不让 `read()` 决定内存。
const MAX_RAR_ONE: u64 = MAX_NEST;
const NAME_CHARS: usize = 150;

/// 当前层里可以直接进下去的下一层。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubEntry {
    /// 显示名：目录带结尾 `/`，子包带原后缀。
    pub label: String,
    /// 进入时追加到路径段上的那一段（不带结尾 `/`）。
    pub seg: String,
    /// 是目录还是子包——决定显示后缀。
    pub dir: bool,
}

/// 一层展开的结果。
#[derive(Clone, Debug)]
pub struct Layer {
    /// 落盘后的目录（可以直接交给 `Gallery`）。
    pub dir: PathBuf,
    /// 这一层根下直接落了几张图。0 = 这一层没有图，界面不该把它当"打开失败"。
    pub images: usize,
    /// 这个目录是不是压缩包展开出来的。只有为真时「0 张」才合法（空层可以继续进子层）。
    pub from_archive: bool,
    /// 这一层里**认得出但打不开**的条数（加密、编码不支持）。
    ///
    /// ★ 非 0 必须让用户看见。全算法支持之后这个数通常还是 0，但一旦不是 0，
    /// 静默跳过就等于"打开一个包只显示 3 张，而包里其实有 50 张"——用户会以为图丢了。
    pub skipped: usize,
}

#[cfg(test)]
fn base_dir() -> Result<PathBuf, String> {
    Ok(paths::test_scratch_root().join("archive"))
}

#[cfg(not(test))]
fn base_dir() -> Result<PathBuf, String> {
    paths::diagnostics_dir().ok_or("取不到 exe 目录".to_string())
}

/// 打开对话框的过滤器：图片清单 + 容器后缀。
///
/// 不往 `gallery::known_exts()` 里塞，是因为那份清单还喂文件关联——容器不该被登记成关联。
pub fn pick_exts() -> Vec<String> {
    let mut v = crate::core::gallery::known_exts();
    v.extend(ARCHIVE_EXTS.iter().map(|e| (*e).to_string()));
    v.sort();
    v.dedup();
    v
}

pub fn is_archive(path: &Path) -> bool {
    kind(path).is_some()
}

/// `Viewer::open` 的唯一分流点：不是包就原样返回；是包就展开**根层**。
pub fn resolve(path: &Path) -> Result<Layer, String> {
    if !is_archive(path) {
        return Ok(Layer {
            dir: path.to_path_buf(),
            images: 0,
            from_archive: false,
            skipped: 0,
        });
    }
    expand(path, &[])
}

/// 进入 `source` 包里 `segs` 指向的那一层（`segs` 为空 = 根层）。
pub fn enter(source: &Path, segs: &[String]) -> Result<Layer, String> {
    if !is_archive(source) {
        return Err("不是压缩包".to_string());
    }
    expand(source, segs)
}

/// 当前层里可以直接进下去的项：直接子目录 + 直接子包。
///
/// 目录里的图**不在**这里展开——那正是「一次只开一层」的意思。
pub fn subs_of(dir: &Path) -> Vec<SubEntry> {
    read_subs(dir).items
}

/// 这个目录是哪个包的哪一层。不是压缩包层就返回 `None`。
///
/// ★ 曾用于右键「本包 ▸」二级菜单的显隐判断；m300 撤掉那个菜单、改用左侧文件树后，
///   消费者只剩空层提示（据 `subs_of` 判「要不要指路」）与调试日志。
pub fn here(dir: &Path) -> Option<(PathBuf, Vec<String>)> {
    let s = fs::read_to_string(dir.join(LAYER_FILE)).ok()?;
    let mut lines = s.lines();
    let source = lines.next()?;
    if source.is_empty() {
        return None;
    }
    let segs_line = lines.next().unwrap_or("");
    let segs = if segs_line.is_empty() {
        Vec::new()
    } else {
        segs_line.split('\t').map(|x| x.to_string()).collect()
    };
    Some((PathBuf::from(source), segs))
}

// ─────────────────────────── 文件树（m300）───────────────────────────
//
// 树与「一次只开一层」不冲突：树只读**名字**，不展开内容。一次 `walk` 就能把整个
// 容器的条目表穷举完，条目名里的 `/` 直接给出目录层级——**目录层级是免费的**，
// 不必为此解任何一个字节。
//
// 早先的顾虑是"扫描整棵树会被 5000 层嵌套拖死"。实测这条只对**子包**成立：包里套包
// 得把内层解出来才能读它的条目表，所以每个子包都是一次真解壳；纯目录层级不吃这一口。
// 于是把它拆成两件事——结构一次建全（子包由 [`TreeBudget`] 兜住），显示一次只开一层。
// ★ m302 起图片不再「只落当前那一层」：首次建树那一趟就把各层的图都落进各自层目录
// （见 `ExtraSink`），变的只是**显示**仍一层一层。

const TREE_FILE: &str = "tree.tsv";
const TREE_MARK: &str = ".rupix-tree";
/// 建树时解出来的中间包。与进层时解壳的 `_nest/` 分开，免得清理时互相误伤。
const TREE_DIR: &str = "_tree";
/// 建树时最多解几个子包（每个子包 = 一次解壳 + 一次遍历）。
const MAX_TREE_PACKS: usize = 64;
/// 为了建树而落盘的子包总字节。
const MAX_TREE_BYTES: u64 = 512 * 1024 * 1024;
/// 树节点数上限。只是名字，纯防炸。
const MAX_TREE_NODES: usize = 20_000;

/// 树里一个节点 = 一个**可以进入的层**（目录或子包）。
///
/// 图片不进树：树回答的是"包里有哪些层"，不是"有哪些文件"——列全几千张图既没用，
/// 也把一棵本来几行的树撑成一本电话簿。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    /// 从包根到这里的路径段。**根节点为空**（= 包本身那一层）。
    pub segs: Vec<String>,
    /// 显示名：根节点是包文件名，其余是最后一段。
    pub label: String,
    /// 目录（true）还是子包（false）——只决定图标。
    pub dir: bool,
    /// 缩进层级：根 = 0，根的直接子项 = 1。
    pub depth: usize,
    /// 这一层**直接**有几张图（不含子层）。0 = 空层，照样可以进。
    pub images: usize,
}

/// 建树的预算。任何一项用完就停在能扫到的深度，并把 `#truncated` 写进树文件。
struct TreeBudget {
    packs_left: usize,
    bytes_left: u64,
    nodes_left: usize,
    truncated: bool,
}

/// 子包落盘的落脚点：`<会话>/_tree/<序>__<安全名>`。
///
/// 用序号而不是名字哈希当文件名，是因为**同一个包里可以有两个同名子包**（不同目录下
/// 都叫 `1.zip`）——按名字会互相覆盖，而它们的内容可能完全不同。
struct NestSink {
    dir: PathBuf,
    seq: usize,
    /// 条目名（`a/b.zip`）→ 落盘路径。
    map: BTreeMap<String, PathBuf>,
}

/// 建树与展开共用的上下文（一次遍历同时干三件事：建树、落这一层的图、记这一层的子项）。
struct TreeCtx<'a> {
    /// 这一次要落图的那一层（全局 segs）。
    want: &'a [String],
    /// 落图目录。
    layer_dir: &'a Path,
    /// 要不要递归子包把整棵树建出来。树已就绪时为 false——那时只需落图。
    want_tree: bool,
    out: Vec<TreeNode>,
    budget: TreeBudget,
    sink: NestSink,
    /// 命中 `want` 层的那次调用填进来。**只会有一次**：`want_rel` 判据天然唯一。
    acc: Option<LayerAcc>,
    /// 首次建树时顺手把别的层也落盘（m302）。树已就绪 ⇒ `None`，只落当前层。
    extra: Option<ExtraSink>,
}

/// `want` 那一层的落盘结果。
#[derive(Default)]
struct LayerAcc {
    images: usize,
    total: u64,
    skipped: usize,
    /// `subs.tsv` 的行（`d`/`a` + 标签 + 段）。
    subs: String,
}

/// 首次建树那一趟，把**别的层**的图也顺手落到各自的层目录（m302）。
///
/// 为什么值得：7z 的 solid 单块每 `walk` 一次都要从块头解（实测 91 MB 包约 1.9 s），
/// 而「切层」原来每次都重走一遍。建树反正要遍历整包、字节已经流过手，多做的只是
/// `fs::write`；换掉之后每个层都有就绪标记，切层不再解包。
///
/// ★ 只做加速，不做承诺：任何一条落盘失败或预算用完，那一层就不写就绪标记，
///   切进去自动退回按需展开 —— 优化失败不许变成「打开失败」或「少几张图」。
struct ExtraSink {
    session: PathBuf,
    /// 全局层路径（`segs.join("/")`，根层是空串）→ 该层的落盘账。
    layers: BTreeMap<String, LayerAcc>,
    bytes_left: u64,
    /// 预算耗尽后不再落盘。
    exhausted: bool,
    /// 因预算 / 闸门 / IO 被跳过的层：不许写就绪标记，否则会显示成「半截空层」。
    broken: BTreeSet<String>,
    /// 当前请求的那一层，元信息由 `expand` 主流程写，这里不碰。
    want_key: String,
}

impl ExtraSink {
    fn new(session: PathBuf, want: &[String]) -> Self {
        Self {
            session,
            layers: BTreeMap::new(),
            bytes_left: MAX_ALL_LAYERS_BYTES,
            exhausted: false,
            broken: BTreeSet::new(),
            want_key: want.join("/"),
        }
    }

    /// 空层也要在账上：它没图，但必须有目录与就绪标记，否则切进去仍会重扫整包。
    fn touch(&mut self, key: &str) {
        self.layers.entry(key.to_string()).or_default();
    }

    /// 给某个层记一条**直接**子项（目录 `d` / 子包 `a`），口径与 `acc.subs` 同源。
    fn sub(&mut self, key: &str, pack: bool, name: &str) {
        let line = if pack {
            format!("a\t{name}\t{name}\n")
        } else {
            format!("d\t{name}/\t{name}\n")
        };
        self.layers
            .entry(key.to_string())
            .or_default()
            .subs
            .push_str(&line);
    }

    /// 把一条不属于当前层的图落到它自己那一层。`r` 只能读一次：这一条本来要被
    /// `Skip` 掉，读走它不会让别的层少东西。
    fn drop(&mut self, key: &str, segs: &[String], last: &str, size: u64, r: &mut dyn Read) {
        if self.exhausted || size > self.bytes_left {
            self.exhausted = true;
            self.broken.insert(key.to_string());
            return;
        }
        // 单条 / 路径闸门只跳过这一条，不该把整包预落盘停摆。
        if size > MAX_ONE || !valid_entry(last) {
            return;
        }
        let fits = {
            let acc = self.layers.entry(key.to_string()).or_default();
            acc.images < MAX_ENTRIES && acc.total.saturating_add(size) <= MAX_TOTAL
        };
        if !fits {
            self.broken.insert(key.to_string());
            return;
        }
        let seq = self.layers.get(key).map_or(1, |a| a.images + 1);
        let dir = self.session.join(layer_key(segs));
        if fs::create_dir_all(&dir).is_err() {
            self.broken.insert(key.to_string());
            return;
        }
        let mut buf = Vec::new();
        if r.read_to_end(&mut buf).is_err()
            || fs::write(dir.join(flat_name(seq, last)), &buf).is_err()
        {
            self.broken.insert(key.to_string());
            return;
        }
        if let Some(acc) = self.layers.get_mut(key) {
            acc.images = seq;
            acc.total = acc.total.saturating_add(size);
        }
        self.bytes_left -= size;
    }

    /// 遍历结束后写各层元信息。当前层由 `expand` 主流程写（那份 `subs.tsv` 还带
    /// 整个容器的 `skipped` 计数），这里不覆盖。
    fn finish(self, source: &Path) {
        for (key, acc) in &self.layers {
            if key == &self.want_key || self.broken.contains(key) {
                continue;
            }
            let segs: Vec<String> = if key.is_empty() {
                Vec::new()
            } else {
                key.split('/').map(str::to_string).collect()
            };
            let dir = self.session.join(layer_key(&segs));
            if fs::create_dir_all(&dir).is_err() {
                continue;
            }
            if write_layer(&dir, source, &segs).is_err() {
                continue;
            }
            let tsv = format!(
                "#images {}\n#skipped {}\n{}",
                acc.images, acc.skipped, acc.subs
            );
            if fs::write(dir.join(SUBS_FILE), &tsv).is_err() {
                continue;
            }
            let _ = fs::write(dir.join(READY_MARK), b"1");
        }
    }
}

/// 读整棵层树（先序）。不是压缩包层返回空。
///
/// 树文件在**会话目录**（不是层目录）：一棵树覆盖同一会话的所有层，进层不用重扫。
pub fn tree_of(dir: &Path) -> Vec<TreeNode> {
    let Some(session) = session_of(dir) else {
        return Vec::new();
    };
    parse_tree(&session.join(TREE_FILE))
}

/// 树有没有被预算截断（还有没扫到的子包）。界面据此提一句"还有层没列出"。
pub fn tree_truncated(dir: &Path) -> bool {
    let Some(session) = session_of(dir) else {
        return false;
    };
    fs::read_to_string(session.join(TREE_FILE))
        .map(|s| s.lines().any(|l| l.trim() == "#truncated 1"))
        .unwrap_or(false)
}

/// 层目录 → 会话目录。判据是 `layer.tsv`（确认它真是包层）+ `tree.tsv` 存在。
fn session_of(dir: &Path) -> Option<PathBuf> {
    here(dir)?;
    let p = dir.parent()?;
    p.join(TREE_FILE).is_file().then(|| p.to_path_buf())
}

fn parse_tree(path: &Path) -> Vec<TreeNode> {
    let Ok(s) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for l in s.lines() {
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        // 行格式：`depth \t kind \t images \t label`。★ segs **不入盘**——靠 depth 用栈
        // 推出来，省一半体积，也免得"两处各记一份路径"写歪。
        let mut p = l.split('\t');
        let (Some(d), Some(k), Some(n), Some(label)) = (p.next(), p.next(), p.next(), p.next())
        else {
            continue;
        };
        let (Ok(depth), Ok(images)) = (d.parse::<usize>(), n.parse::<usize>()) else {
            continue;
        };
        if depth == 0 {
            // 根节点不占栈：它的 segs 就是空（包本身那一层）。
            out.push(TreeNode {
                segs: Vec::new(),
                label: label.to_string(),
                dir: false,
                depth: 0,
                images,
            });
            continue;
        }
        stack.truncate(depth - 1);
        stack.push(label.to_string());
        out.push(TreeNode {
            segs: stack.clone(),
            label: label.to_string(),
            dir: k == "d",
            depth,
            images,
        });
    }
    out
}

fn write_tree(path: &Path, nodes: &[TreeNode], truncated: bool) -> Result<(), String> {
    let mut s = String::new();
    s.push_str(if truncated {
        "#truncated 1\n"
    } else {
        "#truncated 0\n"
    });
    for n in nodes {
        s.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            n.depth,
            if n.dir { "d" } else { "a" },
            n.images,
            n.label
        ));
    }
    fs::write(path, s).map_err(|e| format!("写树索引失败：{e}"))
}

/// 扫一个容器的**整棵**层级，并把 `want` 那一层的图落盘。
///
/// `base` 是这个容器在包里的路径段（根容器为空，子包是"外层路径 + 子包名"），
/// `label` 只在 `base` 为空时用于根节点。
/// ★ 返回**本容器根层**的图数：子包节点自己也是一个层，而它在父容器的 `imgs` 里
/// 只算"一个包"（0 张图）——那个数得由它内部的这一次扫描给回来。
fn build_tree(
    container: &Path,
    base: &[String],
    label: &str,
    ctx: &mut TreeCtx<'_>,
) -> Result<usize, String> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    let mut packs: BTreeSet<String> = BTreeSet::new();
    let mut imgs: BTreeMap<String, usize> = BTreeMap::new();

    // `want` 相对**这个容器**的层路径。空串 = 本容器根层，`None` = want 不在这里。
    let want_rel: Option<String> =
        if ctx.want.len() >= base.len() && ctx.want[..base.len()] == *base {
            Some(ctx.want[base.len()..].join("/"))
        } else {
            None
        };

    let mut acc = LayerAcc::default();

    let st = walk(container, &mut |name, size, r| {
        let parts: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() || name.chars().any(|c| c.is_control()) {
            return Ok(Keep::Skip);
        }
        // 中间各级都是目录：`a/b/c.png` ⇒ `a` 与 `a/b`。**目录层是免费的**——条目名
        // 里本来就带着 `/`，不必为此解任何一个字节。
        for i in 1..parts.len() {
            dirs.insert(parts[..i].join("/"));
        }
        // ★ 名字以 `/` 收尾 = **目录条目**本身（zip 与 tar 都有这种"只占个名字"的条目）。
        //   滤掉空段之后它长得和普通文件一模一样（`a/` ⇒ `["a"]`），不单独认出来就会把
        //   它当成"根层的一张图"，连带把这一层的张数算多。
        if name.ends_with('/') {
            dirs.insert(parts.join("/"));
            return Ok(Keep::Skip);
        }
        let len = parts.len();
        let last = parts[len - 1];
        let parent = parts[..len - 1].join("/");
        let on_want = want_rel.as_deref() == Some(parent.as_str());

        if Path::new(last).extension().is_some() && is_archive(Path::new(last)) {
            let key = parts.join("/");
            // 子包：判断"是不是目录"要等整张条目表收齐（叫 `x.zip` 的目录确实存在），
            // 所以先按包登记并落盘；收齐后再拿 `dirs` 证伪、把白解的字节删掉。
            if ctx.want_tree
                && ctx.budget.packs_left > 0
                && size <= ctx.budget.bytes_left
                && size <= MAX_NEST
            {
                ctx.budget.packs_left -= 1;
                ctx.budget.bytes_left -= size;
                ctx.sink.seq += 1;
                let p = ctx
                    .sink
                    .dir
                    .join(format!("{}_{}", ctx.sink.seq, sanitize(last)));
                let mut f = File::create(&p).map_err(|e| format!("落盘子包失败：{e}"))?;
                std::io::copy(r, &mut f).map_err(|e| format!("读取子包 {name} 失败：{e}"))?;
                ctx.sink.map.insert(key.clone(), p);
            } else if size > MAX_NEST
                || size > ctx.budget.bytes_left
                || (ctx.want_tree && ctx.budget.packs_left == 0)
            {
                // ★ m306：`packs_left` 用完是最常见的超预算路径。旧写法只查字节，
                //   第 65 个子包既不展开、也不置位 ⇒ 树里显示成「空包」，界面与
                //   `--list` 的 `truncated` 都报 false。★ 必须带 `want_tree`：
                //   树已就绪、只落当前层那一趟本来就不建树，不该把它算成截断。
                ctx.budget.truncated = true;
            }
            packs.insert(key);
            return Ok(Keep::Skip);
        }

        // ★ 两件事必须分开：
        //   ① **落盘**沿用 m296 的口径——当前层里只要不是包就落，扩展名交给
        //      `gallery` 再过滤（它才是"什么算图"的正本）。
        //   ② **树上的数字**只用同一个口径数"这一层有几项"，**不查解码器清单**：
        //      `known_exts()` 取决于运行时加载了哪些 DLL，把它当结构判据会让同一棵树
        //      在不同机器上给出不同的数字（测试环境里它干脆是空的）。
        *imgs.entry(parent.clone()).or_insert(0) += 1;
        if !on_want {
            // m302：建树那一趟顺手把这一条落到**它自己那一层**。全局层路径 =
            // 容器在包里的路径段 `base` + 条目在本容器内的目录路径 `parent`。
            // 这一条本来就要被 Skip，读走它的字节不会让任何别的层少东西。
            if let Some(extra) = ctx.extra.as_mut() {
                let mut segs = base.to_vec();
                if !parent.is_empty() {
                    segs.extend(parent.split('/').map(str::to_string));
                }
                let key = segs.join("/");
                extra.touch(&key);
                extra.drop(&key, &segs, last, size, r);
            }
            return Ok(Keep::Skip);
        }
        // 路径穿越 / 盘符只对**要落盘的那一层**检查：别层的名字只用来画树，不进文件系统，
        // 而一个带 `:` 的条目名（漫画包里不少见）不该把整个包拒掉。
        if !valid_entry(last) {
            return Err(format!("压缩包含非法路径：{name}"));
        }
        if size > MAX_ONE {
            return Err(format!("单个条目超过 {MAX_ONE} 字节：{last}"));
        }
        // 上限按**这一层要落盘的张数**算，不是整个 archive 的条目数：一个 5000 张的
        // 大包进到某个只有十几张的子目录，不该被整包规模拒掉。
        if acc.images >= MAX_ENTRIES {
            return Err(format!("这一层条目数超过上限 {MAX_ENTRIES}"));
        }
        acc.total = acc.total.saturating_add(size);
        if acc.total > MAX_TOTAL {
            return Err(format!("这一层解压后超过 {MAX_TOTAL} 字节"));
        }
        acc.images += 1;
        let mut buf = Vec::new();
        r.read_to_end(&mut buf)
            .map_err(|e| format!("读取条目 {last} 失败：{e}"))?;
        fs::write(ctx.layer_dir.join(flat_name(acc.images, last)), &buf)
            .map_err(|e| format!("写展开条目失败：{e}"))?;
        Ok(Keep::Take)
    })?;
    acc.skipped = st.total();

    // 被 `dirs` 证伪的"子包"其实是目录：撤登记，落好的字节也删掉（白解一个）。
    for d in &dirs {
        if packs.remove(d) {
            if let Some(p) = ctx.sink.map.remove(d) {
                let _ = fs::remove_file(p);
            }
        }
    }

    if base.is_empty() {
        ctx.out.push(TreeNode {
            segs: Vec::new(),
            label: label.to_string(),
            dir: false,
            depth: 0,
            images: imgs.get("").copied().unwrap_or(0),
        });
    }

    // `BTreeSet<String>` 的字典序**就是**树的先序：父路径是子路径的前缀，前缀必先出。
    let mut keys: BTreeSet<String> = BTreeSet::new();
    keys.extend(dirs.iter().cloned());
    keys.extend(packs.iter().cloned());

    // `want` 层的**直接**子项：段数比 want 多一段、且以 want 为前缀。
    let want_depth = match want_rel.as_deref() {
        Some("") => 1,
        Some(w) => w.split('/').count() + 1,
        None => 0,
    };

    for key in keys {
        let segs_here: Vec<String> = key.split('/').map(str::to_string).collect();
        let under_want = segs_here.len() == want_depth
            && match want_rel.as_deref() {
                Some("") => true,
                Some(w) => {
                    key.len() > w.len()
                        && key.starts_with(w)
                        && key.as_bytes().get(w.len()) == Some(&b'/')
                }
                None => false,
            };

        let mut segs = base.to_vec();
        segs.extend(segs_here.iter().cloned());
        let is_pack = packs.contains(&key);
        let images = imgs.get(&key).copied().unwrap_or(0);
        let name = segs_here.last().cloned().unwrap_or_default();

        // m302：① 每个层（含没有图的空层）都要有就绪目录，否则切进去仍会重扫整包；
        // ② 顺手把这一层登记成父层的直接子项，供切层后的右键 / 树定位用。
        if let Some(extra) = ctx.extra.as_mut() {
            extra.touch(&segs.join("/"));
            let mut parent_segs = base.to_vec();
            parent_segs.extend(segs_here[..segs_here.len() - 1].iter().cloned());
            extra.sub(&parent_segs.join("/"), is_pack, &name);
        }

        if under_want {
            if is_pack {
                acc.subs.push_str(&format!("a\t{name}\t{name}\n"));
            } else {
                acc.subs.push_str(&format!("d\t{name}/\t{name}\n"));
            }
        }

        if ctx.budget.nodes_left == 0 {
            ctx.budget.truncated = true;
            break;
        }
        ctx.budget.nodes_left -= 1;
        let at = ctx.out.len();
        ctx.out.push(TreeNode {
            segs: segs.clone(),
            label: name,
            dir: !is_pack,
            depth: segs.len(),
            images,
        });

        if !is_pack {
            continue;
        }
        let Some(file) = ctx.sink.map.get(&key).cloned() else {
            // 预算用完 / 树已就绪：节点照样保留（用户看得见它存在），只是没展开。
            continue;
        };
        if segs.len() > MAX_SEGS {
            ctx.budget.truncated = true;
            continue;
        }
        // 子包里的层路径接在子包自己那段之后，`depth` 因此天然连上。
        match build_tree(&file, &segs, &key, ctx) {
            // 子包自己那一层有几张图，只有它内部的扫描知道：父容器眼里它只是"一个包"。
            Ok(n) => ctx.out[at].images = n,
            Err(e) => {
                // 单个子包扫不动不算致命：它自己那一层照样能进（`enter` 走另一条路）。
                log::debug!("[archive] 子包 {key} 展开失败：{e}");
                ctx.budget.truncated = true;
            }
        }
    }

    let root_images = imgs.get("").copied().unwrap_or(0);
    if want_rel.is_some() && ctx.acc.is_none() {
        ctx.acc = Some(acc);
    }
    Ok(root_images)
}

/// GUI 进程退出时收走**全部** `rupix-zip-*` 会话目录（含上一次被杀进程留下的残留）。
///
/// 只在退出时做这一件事：运行期绝不去删别人的目录。单实例闸门保证同一时刻只有一个
/// GUI 进程，所以此刻这些目录没有别的读者；失败一律静默，残留留到下一次退出再收。
pub fn cleanup_all() {
    let Ok(base) = base_dir() else {
        return;
    };
    let Ok(rd) = fs::read_dir(&base) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(SESSION_PREFIX) {
            let _ = fs::remove_dir_all(&p);
        }
    }
}

/// 展开 `segs` 指向的那一层：只落**这一层根下**的图片，子目录与子包只登记不展开。
///
/// 顺带把整棵层树建出来（m300）。三件事共用**同一趟遍历**——7z 的 solid 单块扫一遍
/// 要 2.8 s（实测 91 MB 包），分两趟就是双倍等待，所以宁可让这里一次干完。
fn expand(source: &Path, segs: &[String]) -> Result<Layer, String> {
    let session = session_dir(source)?;
    let layer = session.join(layer_key(segs));

    // 就绪标记 = 「这一层已经完整落盘」的证据，跨进程成立：包没变就还是同一个层目录。
    if layer.join(READY_MARK).is_file() {
        let s = read_subs(&layer);
        return Ok(Layer {
            dir: layer,
            images: s.images,
            from_archive: true,
            skipped: s.skipped,
        });
    }
    if layer.exists() {
        let _ = fs::remove_dir_all(&layer);
    }

    let nest = session.join(NEST_DIR);
    fs::create_dir_all(&nest).map_err(|e| format!("建会话目录失败：{e}"))?;
    fs::create_dir_all(&layer).map_err(|e| format!("建层目录失败：{e}"))?;
    // 建树时解出来的中间包单独放 `_tree/`，与"进层时解壳"的 `_nest/` 分开：两边
    // 命名规则与生命周期不同，混在一处会在清理时互相误伤。
    let tree_nest = session.join(TREE_DIR);
    let _ = fs::create_dir_all(&tree_nest);

    // 树文件在就说明这个会话已经扫过整包 ⇒ 不再递归子包，只落这一层的图。
    let want_tree = !session.join(TREE_MARK).is_file();
    let mut ctx = TreeCtx {
        want: segs,
        layer_dir: &layer,
        want_tree,
        out: Vec::new(),
        budget: TreeBudget {
            packs_left: MAX_TREE_PACKS,
            bytes_left: MAX_TREE_BYTES,
            nodes_left: MAX_TREE_NODES,
            truncated: false,
        },
        sink: NestSink {
            dir: tree_nest,
            seq: 0,
            map: BTreeMap::new(),
        },
        acc: None,
        // 只有「这一趟要建整棵树」时才顺手预落别的层：树已就绪说明本会话早就扫过，
        // 那次该落的已经落过了，再落一遍纯属白写盘。
        extra: want_tree.then(|| ExtraSink::new(session.clone(), segs)),
    };

    // 目标层可能在内层包里：先把沿途的子包解出来，`build_tree` 从那个容器开始扫，
    // `base` 就是那个容器在包里的路径。
    let (target, base) = open_layer(source, segs, &nest)?;
    if let Err(e) = build_tree(&target, &base, &root_label(source), &mut ctx) {
        let _ = fs::remove_dir_all(&layer);
        return Err(e);
    }
    let Some(acc) = ctx.acc.take() else {
        let _ = fs::remove_dir_all(&layer);
        return Err("这一层扫不动：目标层不在容器里".to_string());
    };
    // m302：把别的层的就绪标记一并写完。此后切层在 `expand` 开头就命中，不再
    // `walk` 整个容器。写失败只丢加速，不影响当前层。
    if let Some(extra) = ctx.extra.take() {
        extra.finish(source);
    }

    write_layer(&layer, source, segs)?;
    let tsv = format!(
        "#images {}\n#skipped {}\n{}",
        acc.images, acc.skipped, acc.subs
    );
    fs::write(layer.join(SUBS_FILE), &tsv).map_err(|e| format!("写层索引失败：{e}"))?;
    fs::write(layer.join(READY_MARK), b"1").map_err(|e| format!("写就绪标记失败：{e}"))?;

    // 树只在第一次写；之后各层共用同一份。写失败不算打开失败——图已经在盘上了。
    if want_tree {
        match write_tree(&session.join(TREE_FILE), &ctx.out, ctx.budget.truncated) {
            Ok(()) => {
                let _ = fs::write(session.join(TREE_MARK), b"1");
            }
            Err(e) => log::debug!("[archive] 写树失败（不影响看图）：{e}"),
        }
    }

    Ok(Layer {
        dir: layer,
        images: acc.images,
        from_archive: true,
        skipped: acc.skipped,
    })
}

/// 树根显示什么：包的文件名。取不到就退回一个中性词（不 panic）。
fn root_label(source: &Path) -> String {
    source
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "压缩包".to_string())
}

/// 遍历回调的处置。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// 已把字节读进自己的缓冲，框架不必再管。
    Take,
    /// 不要这条，继续。
    Skip,
    /// 后面的条目都不需要了。能提前停的容器立刻停；7z 交给 `SkipBlock`
    /// （solid 单块下这是唯一能省时间的法子）。
    Done,
}

/// 遍历时统计到的「认得出但打不开」的条目。
#[derive(Default)]
struct WalkStats {
    /// 编码不支持：如 zip 里用 LZMA / PPMd / ZSTD 压的条目（本构建认 Stored + Deflated）。
    unsupported: usize,
    /// 加密条目。
    encrypted: usize,
    /// 太大而放弃的条目（RAR 只给整块读，必须有个闸）。
    oversized: usize,
}

impl WalkStats {
    fn total(&self) -> usize {
        self.unsupported + self.encrypted + self.oversized
    }
}

/// ZIP 读取失败里，哪些属于「这一项认得出但打不开」——只该少一张，不该拖垮整层。
///
/// ★ m304：`ZipArchive::by_index` **在这一步**就会拒掉不支持的压缩方法
///   （LZMA / PPMd / ZSTD，以及部分加密条目），旧写法的 `?` 会让整个容器打不开。
///   按**错误类型**分类，不靠错误文本；`Io` / `InvalidArchive` / `InvalidPassword`
///   这些真损坏或真错误一律照旧上抛，不许被静默吞掉。
fn skippable_zip_error(e: &zip::result::ZipError) -> bool {
    matches!(
        e,
        zip::result::ZipError::CompressionMethodNotSupported(_)
            | zip::result::ZipError::UnsupportedArchive(_)
    )
}

/// **顺序**遍历容器的全部条目。这是四种读取模型的最小公分母：
///
/// - zip 有中央目录、本来可以随机读，但顺序走一遍同样快；
/// - 7z 常见 `Solid = +` 单块，读**任意一条**都要从块头解（实测 91 MB 包 1.84 s/条），
///   只能顺序，且只能靠 `SkipBlock` 提前收尾；
/// - **tar 没有中央目录**，要列目录必须从头扫到尾，1 GB 的 tar.gz 就是全解一遍；
/// - RAR 的头部也是流式排列，一条一条读，`skip` 在非 solid 包上才是 seek。
///
/// 回调拿到 `(包内名, 声明大小, reader)`。没读完的字节由各实现自己收尾
/// （7z 是框架 `io::copy` 到 sink，其余格式丢弃即可）。
fn walk<F>(path: &Path, f: &mut F) -> Result<WalkStats, String>
where
    F: FnMut(&str, u64, &mut dyn Read) -> Result<Keep, String>,
{
    match kind(path) {
        Some(Kind::Zip) => walk_zip(path, f),
        Some(Kind::SevenZ) => walk_7z(path, f),
        Some(Kind::Rar) => walk_rar(path, f),
        Some(Kind::Tar(c)) => walk_tar(path, c, f),
        None => Err(format!("不认识的压缩包格式：{}", path.display())),
    }
}

fn walk_zip<F>(path: &Path, f: &mut F) -> Result<WalkStats, String>
where
    F: FnMut(&str, u64, &mut dyn Read) -> Result<Keep, String>,
{
    let fh = File::open(path).map_err(|e| format!("打开 ZIP 失败：{e}"))?;
    let mut z = zip::ZipArchive::new(fh).map_err(|e| format!("ZIP 解析失败：{e}"))?;
    let mut st = WalkStats::default();
    for i in 0..z.len() {
        let mut e = match z.by_index(i) {
            Ok(e) => e,
            // ★ m304：`by_index` 本身就会因不支持的压缩方法报错；`?` 上抛会让
            //   **整包打不开**（实测 3 张图里 1 枚 LZMA 就是 RC 3、0 层落地）。
            //   这类条目只该少一张、计入 skipped，其余图照常看。
            Err(e) if skippable_zip_error(&e) => {
                st.unsupported += 1;
                continue;
            }
            Err(e) => return Err(format!("读取 ZIP 第 {} 项失败：{e}", i + 1)),
        };
        // ★ m300：**目录条目也要交给回调**——文件树靠它看见"空目录"（没有子文件的
        //   目录没有别的条目能代表它）。名字自带 `/`（zip 规范），回调按后缀分辨。
        //   `size` 是 0、reader 是空的，`extract_one` 拿名字一比就跳过，不会误读。
        // 本构建认 Stored / Deflated；加密项与其它压缩方式跳过这一项，不拖垮整层。
        if !matches!(
            e.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        ) {
            st.unsupported += 1;
            continue;
        }
        let name = e.name().replace('\\', "/");
        let size = e.size();
        if f(&name, size, &mut e)? == Keep::Done {
            break;
        }
    }
    Ok(st)
}

fn walk_7z<F>(path: &Path, f: &mut F) -> Result<WalkStats, String>
where
    F: FnMut(&str, u64, &mut dyn Read) -> Result<Keep, String>,
{
    let mut rd = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|e| format!("7z 解析失败：{e}"))?;
    let mut inner: Option<String> = None;
    let res = rd.for_each_entries(|e, r| {
        // 目录条目也交给回调（名字补成 `xxx/`）：文件树靠它看见空目录。
        let is_dir = e.is_directory();
        let mut name = e.name().replace('\\', "/");
        if is_dir && !name.ends_with('/') {
            name.push('/');
        }
        let size = if is_dir { 0 } else { e.size() };
        match f(&name, size, r) {
            Ok(Keep::Done) => Ok(false),
            Ok(_) => Ok(true),
            Err(msg) => {
                inner = Some(msg);
                Ok(false)
            }
        }
    });
    // 回调的错误优先：它说的是"这一层哪里不对"，容器的错误多半是它的副作用。
    if let Some(msg) = inner {
        return Err(msg);
    }
    res.map_err(|e| format!("7z 解压失败：{e}"))?;
    Ok(WalkStats::default())
}

fn walk_rar<F>(path: &Path, f: &mut F) -> Result<WalkStats, String>
where
    F: FnMut(&str, u64, &mut dyn Read) -> Result<Keep, String>,
{
    let mut st = WalkStats::default();
    let mut a = unrar::Archive::new(path)
        .open_for_processing()
        .map_err(|e| format!("RAR 打开失败：{e}"))?;
    loop {
        let h = match a.read_header() {
            Ok(Some(h)) => h,
            Ok(None) => break,
            Err(e) => return Err(format!("RAR 读头失败：{e}")),
        };
        if !h.entry().is_file() {
            // 目录条目交给回调（名字补成 `xxx/`）：文件树靠它看见空目录。RAR 的目录
            // 不能 `read()`，所以给一个空 reader。
            if h.entry().is_directory() {
                let mut name = h.entry().filename.to_string_lossy().replace('\\', "/");
                if !name.ends_with('/') {
                    name.push('/');
                }
                let mut empty = std::io::Cursor::new(Vec::new());
                if f(&name, 0, &mut empty)? == Keep::Done {
                    break;
                }
            }
            a = h.skip().map_err(|e| format!("RAR 跳过失败：{e}"))?;
            continue;
        }
        let name = h.entry().filename.to_string_lossy().replace('\\', "/");
        let size = h.entry().unpacked_size;
        // ★ unrar 只提供"整块读进内存"的 `read()`，没有流式接口。所以必须自己设闸：
        //   否则一个声明 4 GB 的恶意条目会在 `read()` 里直接吃掉内存。
        if h.entry().is_encrypted() {
            st.encrypted += 1;
            a = h.skip().map_err(|e| format!("RAR 跳过失败：{e}"))?;
            continue;
        }
        if size > MAX_RAR_ONE {
            st.oversized += 1;
            a = h.skip().map_err(|e| format!("RAR 跳过失败：{e}"))?;
            continue;
        }
        let (data, rest) = h.read().map_err(|e| format!("RAR 解压 {name} 失败：{e}"))?;
        a = rest;
        let mut cur = std::io::Cursor::new(data);
        if f(&name, size, &mut cur)? == Keep::Done {
            break;
        }
    }
    Ok(st)
}

fn walk_tar<F>(path: &Path, comp: Option<Comp>, f: &mut F) -> Result<WalkStats, String>
where
    F: FnMut(&str, u64, &mut dyn Read) -> Result<Keep, String>,
{
    let fh = File::open(path).map_err(|e| format!("打开 tar 失败：{e}"))?;
    let br = std::io::BufReader::new(fh);
    // ★ 全程**流式**，绝不把整条流解进内存：1 GB 的 tar.xz 会直接爆。
    //   所以 xz 用 `lzma_rust2::XzReader` 而不是 `lzma_rs::xz_decompress(&mut BufRead)`。
    let src: Box<dyn Read> = match comp {
        None => Box::new(br),
        Some(Comp::Gz) => Box::new(flate2::read::GzDecoder::new(br)),
        Some(Comp::Xz) => Box::new(lzma_rust2::XzReader::new(br, false)),
        Some(Comp::Bz2) => Box::new(bzip2::read::BzDecoder::new(br)),
        Some(Comp::Zst) => Box::new(
            ruzstd::decoding::StreamingDecoder::new(br)
                .map_err(|e| format!("zstd 解析失败：{e}"))?,
        ),
    };
    let mut a = tar::Archive::new(src);
    // tar 条目没有"编码不支持"这回事：压缩在外层，解不开就在上面那步报错。
    let st = WalkStats::default();
    let entries = a.entries().map_err(|e| format!("tar 解析失败：{e}"))?;
    for e in entries {
        let mut e = e.map_err(|e| format!("tar 条目失败：{e}"))?;
        let ty = e.header().entry_type();
        let is_dir = ty.is_dir();
        // 只认普通文件与目录：链接 / 设备节点不属于"这一层有什么"。
        if !is_dir && !ty.is_file() {
            continue;
        }
        let mut name = e
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .map_err(|e| format!("tar 条目路径失败：{e}"))?;
        // tar 的目录路径**不带**尾斜杠（与 zip 相反），这里补齐，让回调只有一套判据。
        if is_dir && !name.ends_with('/') {
            name.push('/');
        }
        let size = if is_dir { 0 } else { e.size() };
        if f(&name, size, &mut e)? == Keep::Done {
            break;
        }
    }
    Ok(st)
}

/// 打开 `source` 里 `segs` 指向的那一层，返回 `(目标容器, 该层内的目录前缀)`。
///
/// 遇到包段就把它从上一层解出来落到 `_nest/`，再对那个文件重新按后缀分派——这样
/// 7z 里套 zip、zip 里套 tar 都能进，也不必把整包读进内存。
/// 第二个返回值是**这个容器在包里的路径段**（`base`）——建树要靠它把层路径接上：
/// 进了 `a/b.zip` 之后，容器内的 `c/` 在包里的完整路径是 `a/b.zip/c`。
fn open_layer(
    source: &Path,
    segs: &[String],
    nest: &Path,
) -> Result<(PathBuf, Vec<String>), String> {
    if segs.len() > MAX_SEGS {
        return Err(format!("进入层数超过上限 {MAX_SEGS}"));
    }
    let mut cur = source.to_path_buf();
    let mut prefix = String::new();
    let mut base: Vec<String> = Vec::new();

    for (i, seg) in segs.iter().enumerate() {
        // 叫 `x.zip` 的**目录**也存在，所以先试着按条目取一次；取不到再当前缀。
        // 只扫一趟：`extract_one` 返回 `None` 就等于"它是目录"。
        if is_archive(Path::new(seg)) {
            let name = format!("{prefix}{seg}");
            if let Some(bytes) = extract_one(&cur, &name, MAX_NEST)? {
                // ★ 必须保留原后缀：`kind()` 靠后缀分派，一律写成 `.zip` 会把内层
                //   7z / tar 当 zip 解。`sanitize` 留着点号，所以 `1__inner.7z` 仍可识别。
                let p = nest.join(format!("{i}__{}", sanitize(seg)));
                fs::write(&p, &bytes).map_err(|e| format!("落盘内层包失败：{e}"))?;
                // 换了容器 ⇒ 此前走过的段（含 `prefix` 里那些目录）都是**容器路径**。
                base.extend(
                    prefix
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                );
                base.push(seg.clone());
                cur = p;
                prefix.clear();
                continue;
            }
        }
        prefix.push_str(seg);
        prefix.push('/');
    }
    Ok((cur, base))
}

/// 从容器里取出单个条目的字节；没有这一项返回 `Ok(None)`（调用方据此判定它是目录）。
fn extract_one(source: &Path, name: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let mut got: Option<Vec<u8>> = None;
    walk(source, &mut |n, size, r| {
        if n != name {
            return Ok(Keep::Skip);
        }
        if size > limit {
            return Err(format!("内层包超过 {limit} 字节：{name}"));
        }
        let mut buf = Vec::new();
        r.read_to_end(&mut buf)
            .map_err(|e| format!("读取 {name} 失败：{e}"))?;
        got = Some(buf);
        Ok(Keep::Done)
    })?;
    Ok(got)
}

/// 路径穿越 / 盘符 / 空段 / 控制字符一律拒绝。ZIP 里目录只是名字带 `/` 的条目。
fn valid_entry(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.chars().any(|c| c.is_control())
        && name
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && !p.contains(':'))
}

/// 把层内相对名折成单层文件名：序号 + 原名。
///
/// 序号保证清单次序就是包内条目次序；扩展名必须留着，否则 `gallery::listing` 不认这一项。
fn flat_name(seq: usize, rest: &str) -> String {
    let s: String = rest
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim_matches(|c: char| c == ' ' || c == '.').to_string();
    let (stem, ext) = match s.rfind('.') {
        Some(i) if i > 0 => (&s[..i], &s[i..]),
        _ => (s.as_str(), ""),
    };
    let head: String = stem.chars().take(NAME_CHARS).collect();
    let tail: String = ext.chars().take(24).collect();
    format!("{seq:04}__{head}{tail}")
}

/// 只留安全字符，给 `_nest/` 的中间包起名用。
fn sanitize(seg: &str) -> String {
    seg.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect()
}

/// `subs.tsv` 解析结果。
#[derive(Default)]
struct Subs {
    images: usize,
    /// 认得出但打不开的条数（加密 / 编码不支持 / 太大）。
    skipped: usize,
    items: Vec<SubEntry>,
}

/// 解析 `subs.tsv`。`#` 开头是元信息行。
fn read_subs(dir: &Path) -> Subs {
    let Ok(s) = fs::read_to_string(dir.join(SUBS_FILE)) else {
        return Subs::default();
    };
    let mut out = Subs::default();
    for l in s.lines() {
        if let Some(n) = l.strip_prefix("#images ") {
            out.images = n.trim().parse().unwrap_or(0);
            continue;
        }
        if let Some(n) = l.strip_prefix("#skipped ") {
            out.skipped = n.trim().parse().unwrap_or(0);
            continue;
        }
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let mut p = l.split('\t');
        let (Some(kind), Some(label), Some(seg)) = (p.next(), p.next(), p.next()) else {
            continue;
        };
        out.items.push(SubEntry {
            label: label.to_string(),
            seg: seg.to_string(),
            dir: kind == "d",
        });
    }
    out
}

fn write_layer(layer: &Path, source: &Path, segs: &[String]) -> Result<(), String> {
    let t = format!("{}\n{}\n", source.to_string_lossy(), segs.join("\t"));
    fs::write(layer.join(LAYER_FILE), t).map_err(|e| format!("写层信息失败：{e}"))
}

/// 层目录名：路径段各自喂进同一个 FNV，段间补 `/`。空段（根层）得到 `L<种子>`。
fn layer_key(segs: &[String]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for seg in segs {
        for b in seg.as_bytes() {
            h = fnv(h, *b);
        }
        h = fnv(h, b'/');
    }
    format!("L{h:016x}")
}

fn fnv(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(0x1000_0000_01b3)
}

fn session_dir(path: &Path) -> Result<PathBuf, String> {
    let base = base_dir()?;
    let meta = fs::metadata(path).map_err(|e| format!("读取压缩包属性失败：{e}"))?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // FNV-1a：只把哈希写进目录名，不把用户的原路径落到 temp 里。
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().as_bytes() {
        h = fnv(h, *b);
    }
    for b in meta.len().to_le_bytes() {
        h = fnv(h, b);
    }
    for b in stamp.to_le_bytes() {
        h = fnv(h, b);
    }
    Ok(base.join(format!("{SESSION_PREFIX}{h:016x}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                // 以 `/` 收尾的名字是**目录条目**（zip 与 tar 都有）：`start_file` 会拒它，
                // 得走 `add_directory`。文件树正是靠这类条目才看得见"空目录"。
                if name.ends_with('/') {
                    w.add_directory(name.trim_end_matches('/'), opts).ok();
                    continue;
                }
                w.start_file(*name, opts).ok();
                w.write_all(data).ok();
            }
            w.finish().ok();
        }
        buf
    }

    /// 造一个真包落在 scratch 里，返回路径。
    fn pkg(name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let dir = paths::test_scratch_root().join("archive-cases");
        fs::create_dir_all(&dir).ok();
        let p = dir.join(name);
        fs::write(&p, zip_of(entries)).ok();
        p
    }

    #[test]
    fn one_layer_at_a_time_outer_sees_only_its_own_images() {
        let inner = zip_of(&[("ch1/007.png", b"inner"), ("008.png", b"inner8")]);
        let p = pkg(
            "outer.cbz",
            &[
                ("cover/001.jpg", b"cover"),
                ("010.png", b"ten"),
                ("inner.zip", &inner),
            ],
        );
        let l = expand(&p, &[]).expect("根层展开");
        // 根下直接图只有 010.png；cover 里的那张与内层包都不算。
        assert_eq!(l.images, 1, "外层只该有 1 张直接图");
        let subs = subs_of(&l.dir);
        let segs: Vec<&str> = subs.iter().map(|s| s.seg.as_str()).collect();
        assert_eq!(segs, vec!["cover", "inner.zip"], "子目录与子包各一条");
        assert!(subs[0].dir, "cover 是目录");
        assert!(!subs[1].dir, "inner.zip 是子包");
    }

    #[test]
    fn entering_the_inner_archive_yields_its_own_layer() {
        let inner = zip_of(&[("ch1/007.png", b"inner"), ("008.png", b"inner8")]);
        let p = pkg("nest.cbz", &[("010.png", b"ten"), ("inner.zip", &inner)]);

        let deep = expand(&p, &["inner.zip".to_string()]).expect("内层展开");
        assert_eq!(deep.images, 1, "内层根下只有 008.png");
        assert_eq!(
            subs_of(&deep.dir)
                .iter()
                .map(|s| s.seg.clone())
                .collect::<Vec<_>>(),
            vec!["ch1".to_string()]
        );

        let ch = expand(&p, &["inner.zip".to_string(), "ch1".to_string()]).expect("ch1 展开");
        assert_eq!(ch.images, 1);
    }

    #[test]
    fn an_empty_layer_is_legal_and_reports_zero() {
        let inner = zip_of(&[("deep/001.png", b"x")]);
        let p = pkg("empty.cbz", &[("inner.zip", &inner)]);
        // 根层一张直接图都没有：这不是错误，界面据此不把它当"打开失败"。
        let l = expand(&p, &[]).expect("空层不该报错");
        assert_eq!(l.images, 0);
        assert!(l.from_archive);
        assert_eq!(subs_of(&l.dir).len(), 1);
    }

    #[test]
    fn layers_are_hashed_apart_so_switching_back_reuses_the_dir() {
        let inner = zip_of(&[("001.png", b"x")]);
        let p = pkg("reuse.cbz", &[("010.png", b"ten"), ("inner.zip", &inner)]);
        let a = expand(&p, &[]).expect("根层");
        let b = expand(&p, &["inner.zip".to_string()]).expect("内层");
        assert_ne!(a.dir, b.dir, "两层必须是不同目录，否则来回切会互相覆盖");
        let again = expand(&p, &[]).expect("根层再来一次");
        assert_eq!(a.dir, again.dir, "同一层要复用同一个目录");
        assert_eq!(again.images, 1);
    }

    #[test]
    fn here_reports_source_and_segs_for_the_menu() {
        let inner = zip_of(&[("001.png", b"x")]);
        let p = pkg("here.cbz", &[("inner.zip", &inner)]);
        let l = expand(&p, &["inner.zip".to_string()]).expect("内层");
        let (src, segs) = here(&l.dir).expect("层目录要能说出自己来自哪");
        assert_eq!(src, p);
        assert_eq!(segs, vec!["inner.zip".to_string()]);
        // 普通目录不是层。
        assert!(here(&paths::test_scratch_root()).is_none());
    }

    #[test]
    fn flat_name_keeps_the_extension_for_the_gallery_filter() {
        let n = flat_name(1, "c.jpg");
        assert!(n.ends_with(".jpg"), "丢了扩展名就进不了目录清单：{n}");
        assert!(n.starts_with("0001__"));
    }

    #[test]
    fn traversal_and_drive_prefixes_are_rejected() {
        assert!(!valid_entry("../escape.png"));
        assert!(!valid_entry("/abs.png"));
        assert!(!valid_entry("C:/x.png"));
        assert!(!valid_entry("a//b.png"));
        assert!(!valid_entry("a\tb.png"), "制表符会串掉 subs.tsv");
        assert!(valid_entry("sub/001.png"));
    }

    #[test]
    fn a_plain_file_is_not_an_archive_layer() {
        let l = resolve(Path::new("some/photo.png")).expect("非包原样返回");
        assert!(!l.from_archive);
        assert_eq!(l.images, 0);
    }

    #[test]
    fn archive_exts_are_case_insensitive_and_pick_list_has_no_duplicates() {
        assert!(is_archive(Path::new("a.CBZ")));
        assert!(is_archive(Path::new("a.CB7")));
        assert!(is_archive(Path::new("a.zip")));
        assert!(!is_archive(Path::new("a.png")));
        let exts = pick_exts();
        assert_eq!(exts.iter().filter(|e| e.as_str() == "zip").count(), 1);
        assert_eq!(exts.iter().filter(|e| e.as_str() == "cbz").count(), 1);
        assert_eq!(exts.iter().filter(|e| e.as_str() == "cb7").count(), 1);
    }

    /// ★ 复合后缀是这里唯一有坑的地方：`.gz` 单独不是包（那是压一个文件，没有
    /// "第 3 张图"），只有 `xxx.tar.gz` 才算。
    #[test]
    fn only_tar_wrapped_compression_counts_as_an_archive() {
        assert!(kind(Path::new("a.tar.gz")).is_some(), ".tar.gz 是包");
        assert!(kind(Path::new("a.TAR.GZ")).is_some(), "大小写不敏感");
        assert!(kind(Path::new("a.tgz")).is_some(), ".tgz 是同一个东西");
        assert!(
            kind(Path::new("a.gz")).is_none(),
            "裸 .gz 不是包，对话框里看得见但打开要报清楚"
        );
        assert!(kind(Path::new("a.tar.xz")).is_some());
        assert!(kind(Path::new("a.xz")).is_none());
        assert!(kind(Path::new("a.7z")).is_some());
        assert!(kind(Path::new("a.rar")).is_some());
        assert!(kind(Path::new("a.cbr")).is_some());
    }

    fn tar_of(entries: &[(&str, &[u8])], gz: bool) -> Vec<u8> {
        let mut raw = Vec::new();
        {
            let mut b = tar::Builder::new(&mut raw);
            for (name, data) in entries {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, *data).ok();
            }
            b.finish().ok();
        }
        if !gz {
            return raw;
        }
        let mut out = Vec::new();
        {
            let mut e = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
            e.write_all(&raw).ok();
            e.finish().ok();
        }
        out
    }

    fn write_case(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = paths::test_scratch_root().join("archive-cases");
        fs::create_dir_all(&dir).ok();
        let p = dir.join(name);
        fs::write(&p, bytes).ok();
        p
    }

    /// tar 走的是**另一条**读取路径（无中央目录、纯顺序流），必须证明它和 zip 一样
    /// 一次只开一层。
    #[test]
    fn a_tarball_expands_one_layer_at_a_time() {
        let body = [
            ("001.png", b"one".as_slice()),
            ("002.png", b"two".as_slice()),
            ("sub/003.png", b"three".as_slice()),
        ];
        for (name, gz) in [("layer.tar", false), ("layer.tar.gz", true)] {
            let p = write_case(name, &tar_of(&body, gz));
            let l = expand(&p, &[]).unwrap_or_else(|e| panic!("{name} 展开失败：{e}"));
            assert_eq!(l.images, 2, "{name} 根层只有两张直接图");
            assert_eq!(
                subs_of(&l.dir)
                    .iter()
                    .map(|s| s.seg.clone())
                    .collect::<Vec<_>>(),
                vec!["sub".to_string()],
                "{name} 子目录只登记不展开"
            );
            assert_eq!(l.skipped, 0, "{name} 不该有跳过的条目");

            let deep = expand(&p, &["sub".to_string()]).expect("进子目录");
            assert_eq!(deep.images, 1, "{name} 子目录里那一张");
        }
    }

    // ── 文件树（m300）──

    /// 树要点：**没进过的层也要在**。目录层级来自条目名里的 `/`，不需要解开任何字节，
    /// 所以"一次只开一层"说的是图片，不是结构。
    #[test]
    fn the_tree_lists_layers_nobody_opened() {
        let p = pkg(
            "tree-levels.cbz",
            &[
                ("a/b/c.png", b"c"),
                ("a/d.jpg", b"d"),
                ("e.png", b"e"),
                ("noimg/", b""),
            ],
        );
        let l = expand(&p, &[]).expect("根层展开");
        let got: Vec<(String, usize, bool)> = tree_of(&l.dir)
            .into_iter()
            .map(|n| (n.segs.join("/"), n.images, n.dir))
            .collect();
        assert_eq!(
            got,
            vec![
                (String::new(), 1, false), // 根：e.png
                ("a".into(), 1, true),     // a：d.jpg
                ("a/b".into(), 1, true),   // a/b：c.png
                ("noimg".into(), 0, true), // 空目录条目：在树上，但这一层没图
            ],
            "树要先序、每层直接图数、目录/子包标记都对"
        );
        assert!(!tree_truncated(&l.dir), "这么小的包不该被预算截断");
    }

    /// 子包要**解出来再扫一遍**才看得到里面——这是树唯一花钱的地方，也是它必须能工作的
    /// 地方（漫画包的 `1M_2.zip` 套 `1M.zip` 就是这种形状）。
    #[test]
    fn the_tree_descends_into_inner_archives() {
        let inner = zip_of(&[("ch1/007.png", b"i"), ("008.png", b"i8")]);
        let p = pkg("tree-nest.cbz", &[("010.png", b"t"), ("inner.zip", &inner)]);
        let l = expand(&p, &[]).expect("根层展开");
        let t = tree_of(&l.dir);
        let path: Vec<String> = t
            .iter()
            .map(|n| {
                if n.segs.is_empty() {
                    ".".to_string()
                } else {
                    n.segs.join("/")
                }
            })
            .collect();
        assert_eq!(path, vec![".", "inner.zip", "inner.zip/ch1"]);
        assert_eq!(t[0].images, 1, "根层 010.png");
        assert_eq!(t[1].images, 1, "内层根 008.png");
        assert_eq!(t[2].images, 1, "内层 ch1/007.png");
        assert!(!t[1].dir, "inner.zip 是子包，不是目录");
    }

    /// 叫 `x.zip` 的**目录**确实存在（`open_layer` 里早就为此先试条目再试前缀）。
    /// 判"是包还是目录"必须等整张条目表收齐——先按包登记、事后用 `dirs` 证伪。
    #[test]
    fn a_directory_named_like_an_archive_stays_a_directory() {
        let p = pkg("tree-trick.cbz", &[("x.zip/001.png", b"a")]);
        let l = expand(&p, &[]).expect("根层展开");
        let t = tree_of(&l.dir);
        assert_eq!(t.len(), 2, "根 + x.zip 一层");
        assert_eq!(t[1].segs, vec!["x.zip".to_string()]);
        assert!(t[1].dir, "它后面还有条目 ⇒ 是目录，不是子包");
        assert_eq!(t[1].images, 1);
    }

    /// m302：预落盘的层必须写出完整元信息与就绪标记；预算耗尽的层**不许**写标记，
    /// 否则切进去看到的是「半截空层」，而不是退回按需展开。
    #[test]
    fn extra_sink_marks_only_complete_layers_ready() {
        let session = base_dir().unwrap().join("m302-extra-sink");
        let _ = fs::remove_dir_all(&session);
        fs::create_dir_all(&session).unwrap();
        let src = Path::new("demo.7z"); // 只被写进 layer.tsv，不必是真文件
        let hand = vec!["hand".to_string()];
        let broken = vec!["broken".to_string()];
        let empty = vec!["empty".to_string()];

        let mut s = ExtraSink::new(session.clone(), &[]);
        s.touch("hand");
        s.sub("hand", true, "inner.zip");
        s.sub("hand", false, "ch1");
        let mut a = std::io::Cursor::new(b"AAA".to_vec());
        s.drop("hand", &hand, "a.png", 3, &mut a);
        let mut b = std::io::Cursor::new(b"BB".to_vec());
        s.drop("hand", &hand, "b.png", 2, &mut b);
        // 先落一张、再把预算压到 0 ⇒ 这一层不完整，整层作废。
        let mut c0 = std::io::Cursor::new(b"C0".to_vec());
        s.drop("broken", &broken, "c0.png", 2, &mut c0);
        s.bytes_left = 0;
        let mut c1 = std::io::Cursor::new(b"C1".to_vec());
        s.drop("broken", &broken, "c1.png", 2, &mut c1);
        // 空层没有任何条目，照样该有标记（否则切进去仍会重扫整包）。
        s.touch("empty");
        s.finish(src);

        let hd = session.join(layer_key(&hand));
        assert!(hd.join(READY_MARK).is_file(), "完整层要有就绪标记");
        let subs = read_subs(&hd);
        assert_eq!(subs.images, 2, "两张图都要记进 #images");
        assert_eq!(subs.items.len(), 2, "直接子项两条");
        assert!(!subs.items[0].dir && subs.items[0].label == "inner.zip");
        assert!(subs.items[1].dir && subs.items[1].label == "ch1/");
        assert_eq!(
            here(&hd).map(|(_, s)| s),
            Some(hand.clone()),
            "layer.tsv 要能回推出层路径"
        );

        let bd = session.join(layer_key(&broken));
        assert!(!bd.join(READY_MARK).is_file(), "半截层不许写就绪标记");
        assert!(
            session.join(layer_key(&empty)).join(READY_MARK).is_file(),
            "空层也要有就绪标记"
        );
        // 负向臂：当前请求的层由 `expand` 主流程写，`finish` 不许越界覆盖。
        assert!(!session.join(layer_key(&[])).join(READY_MARK).is_file());
        let _ = fs::remove_dir_all(&session);
    }

    /// m304：ZIP 的「认得出但打不开」只跳过这一项，真损坏不许被吞。
    /// 端到端由 `.temp/m304_skip_probe.py` 的实验臂 / 对照臂复验（RC 3 → RC 0）。
    #[test]
    fn zip_skips_unsupported_entries_but_not_real_damage() {
        use zip::result::ZipError;
        // 正向：不支持的压缩方法、需要密码的加密条目 ⇒ 少一张，别拖垮整层。
        assert!(skippable_zip_error(
            &ZipError::CompressionMethodNotSupported(14)
        ));
        assert!(skippable_zip_error(&ZipError::UnsupportedArchive(
            ZipError::PASSWORD_REQUIRED
        )));
        // 负向臂：坏头、密码不对、找不到条目 ⇒ 一律上抛，不许静默吞掉。
        // 去掉这道闸，损坏的包会变成「悄悄少几张」，比报错更难查。
        assert!(!skippable_zip_error(&ZipError::InvalidArchive(
            "bad header".into()
        )));
        assert!(!skippable_zip_error(&ZipError::FileNotFound));
        assert!(!skippable_zip_error(&ZipError::InvalidPassword));
    }

    /// m306：子包预算用完必须写 #truncated 1。旧写法只查 size / bytes，
    /// `packs_left == 0` 落空 ⇒ 第 65 个子包在树里显示成空包、界面与命令面都不报。
    #[test]
    fn exhausted_pack_budget_is_reported_as_truncated() {
        let inner = zip_of(&[("p.png", b"x")]);
        let names: Vec<String> = (0..65).map(|i| format!("s{i:02}.zip")).collect();
        let ents: Vec<(&str, &[u8])> = names
            .iter()
            .map(|n| (n.as_str(), inner.as_slice()))
            .collect();
        let p = pkg("budget65.zip", &ents);
        let l = resolve(&p).expect("包该能打开");
        assert!(tree_truncated(&l.dir), "第 65 个子包没展开，必须报截断");

        // 负向臂：64 个正好用满预算，不许误报。
        let p64 = pkg("budget64.zip", &ents[..64]);
        let l64 = resolve(&p64).expect("包该能打开");
        assert!(!tree_truncated(&l64.dir), "没超预算不许误报");
    }

    /// 源码钉子：`want_tree` 不许被删——树已就绪、只落当前层那一趟不建树，
    /// 若也按「packs 用完」置位，同一个包第二次进层会凭空多出一条截断提示。
    #[test]
    fn pack_budget_flag_requires_an_actual_tree_walk() {
        // ★ 本文件前面还有一处 `#[cfg(test)] fn base_dir()`，不能按第一次出现的
        //   `#[cfg(test)]` 切「生产段」——那样只会拿到卷首 143 行。钉全文即可。
        let all = include_str!("archive.rs");
        assert!(all.contains("ctx.want_tree && ctx.budget.packs_left == 0"));
    }
}
