//! 查看器状态机：当前图 + 缩放 + 平移 + 解码需求的判定。
//!
//! 全部状态集中在一个 `RefCell` 里，由自定义控件（`ui::canvas`）与按钮/快捷键共享。
//!
//! # 缩放的两档语义
//!
//! - `fit = true`：倍率**由视口算出**（适应窗口），用户没手动介入过；窗口一改大小就跟着变。
//! - `fit = false`：倍率是用户显式定的 `zoom`；改窗口大小**不**改变倍率。
//!   混淆这两档的经典 bug 是"最大化窗口后图片突然变大"——因为把 fit 的推导结果写回了 zoom。
//!
//! # 位图的两档：解码档 vs 上屏档（2026-10-17 C 路线）
//!
//! [`Loaded::base_rgba`] 是**解码档**（解码器给回的那份像素），[`Loaded::image`] 是**上屏档**
//! （由 base 本地面积采样派生，尺寸记在 [`Loaded::displayed`]）。两者分开是刻意的：
//!
//! - 首解仍按**视口档**要（小图一步到位，大图省一份常驻）；
//! - 首解一落地，[`Self::tick`] 就**主动**补一次原图档升级（`send_decode(.., 0, 0, ..)`，
//!   不等用户放大——m58 实测慢翻臂 12/12 都在落地后发生），到手之后 base 就是整图，
//!   **此后所有缩放都只做本地派生，不再碰解码器**。为什么不等放大：实测阶梯
//!   30→50→80% 时"一次整解 + 两次本地换档"胜过"三次整解"（见下方数字与 §一 那条实测）。
//! - ★ 但这次升级**给翻页让路**（m58）：连续翻页时这一张已不在屏上，而一次原图档整解要
//!   独占唯一那条解码线程 150–500 ms 且不可抢占。判据见 [`Self::upgrade_can_wait`]。
//!
//! 为什么这样分：实测本解码器集不理 `target_w/target_h`（240×180 的缩略图也要整解一遍，
//! 见 本仓历史记录 §七），"按视口档要"省不到解码时间，
//! 只会换来"每档缩放整解一次"——放大糊图的根子。而本地派生 150–200 ms（6055² 样本）比
//! 再解一次快 2–7 倍，用的还是宿主这套面积采样：交给上屏层双线性，"适应窗口"那种大比例
//! 缩小会出摩尔纹。**缩放是显示层的活儿，不该是解码层的活儿。**

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::config::Config;
use crate::core::decode::{Decoder, Done, Meta, MetaEvent, RegionReq, ReqKind};
use crate::core::error::ViewError;
use crate::core::gallery::{Gallery, Jump};
use crate::core::geometry::{downscale, fit_dims, transformed, RectI};
use crate::core::pixels::{Anim, Decoded, Region, RgbaImage, MAX_PIXELS};
use crate::core::thumbs::Thumbs;

/// 缩放倍率上下限（相对**原图**，1.0 = 100%）。
pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 64.0;

/// 缩放停止后多久才动画面（升级解码或本地重派生）。
///
/// 120ms 是"滚轮已经停了"与"用户还在滚"的分界：更短会在连续滚动时反复触发重活
/// （升级是一次全图重解，稳态下是一次 150–200 ms 的本地面积采样），更长则停下来后画面糊得太久。
const REDECODE_DEBOUNCE: Duration = Duration::from_millis(120);
/// 旋转/镜像的"当场算"上限：源像素 + 目标像素之和低于它就走同步重建。
///
/// 按 m72 实测的搬运成本取保守值 3 ns/像素（36.7 MP 转置 75 ms、100 MP 200 ms），
/// 2.7 M ⇒ 约 8 ms：一帧的时间，用户看不出来。视口档（≈0.4 MP）远在门槛内，
/// 原图档（≥20 MP）远在门槛外，中间没有模糊地带。
const ORIENT_SYNC_PIXELS: u64 = 2_700_000;
/// 翻页之后，原图档升级要**让路**多久（m58）。
///
/// 判据来自两臂实测（本仓历史记录 §三）：慢翻臂间隔 1200 ms，每张的升级都该照发生；
/// 快翻臂间隔 150 ms，升级腿一次要占住唯一那条解码线程 150–500 ms，把下一张的首屏
/// 挤到队尾——而那张图此时已经不在屏上了。400 ms 落在两个读数之间：比快翻间隔长一个
/// 数量级（翻页期间基本发不出去），比"停下来看图"短得多（手一停就补清晰）。
const NAV_UPGRADE_GRACE: Duration = Duration::from_millis(400);
/// 一次性提示在浮条上停留多久（m50-C）：够读完一句短话，又不会赖着不走。
const NOTE_TTL: Duration = Duration::from_millis(2500);
/// 查询阶段元信息缓存的条数上限。
///
/// 条目很小（PathBuf + 两个 u32），但**没有上限的缓存就是慢性泄漏**：一晚上翻几万张
/// 不同目录的图，路径会一直攒着。到顶整表清空而不是做 LRU——这份缓存只用来“翻回来时
/// 先显示尺寸”，丢了只影响下一次 MetaEvent 到达前的一瞬，不值得再养一套淘汰账。
const META_CACHE_MAX: usize = 4096;
/// 换图请求等多久后，信息条开始报"已等 N 秒"（m71 / 待办 §18 ⑥）。
///
/// 3 s 是"正常解码不该超过"与"用户开始怀疑卡住"的分界：100 MP 级样本带原生降采样
/// 首屏是 0.1–0.2 s，不带降采样的格式也在 1–2 s 内；3 s 之后还没有回包，用户已经
/// 在盯着旧图发愣，此时把等待时长说出来比空转的"载入中…"有用。
const WAIT_NOTICE_AFTER: Duration = Duration::from_secs(3);
/// 同一枚主画面请求等多久后写一条 warn（只写一次）。
///
/// 10 s 远大于任何正常解码，落进日志是为了**事后取证**：用户报"卡住了"时，
/// 日志里能直接看到哪个文件、等了多久——而不是只有一行"线程已退出"。
const STALL_WARN_AFTER: Duration = Duration::from_secs(10);
/// 静图上按 `P` / `,` / `.` 时给的那句人话。
/// ★ 刻意不出现"动画"二字——`status_static_has_no_animation_tag` 守着"静图的状态栏
///   不许带动画字样"，措辞撞上它等于自己给自己埋雷。
pub(crate) const NOTE_STILL: &str = "这张图只有一帧";

/// 预读命中判定"够用"的阈值。
///
/// 预读存的是**视口档**，翻到那张时按这一档直接上屏（零延迟），再由 `tick` 升级原图档。
/// 只有视口在这期间变大过（拖窗口/换屏）才判它不够、丢掉重解——容忍 10% 以内的差距，
/// 免得每次翻页都白扔一份已经解好的图。主画面自身的档位判定已改为"base 是否整图"
/// （见模块头「位图的两档」），不再用这条比例闸。
const SHARP_ENOUGH: f32 = 0.90;

/// 预读窗口深度（**每个方向**预取几张）。
///
/// 业界对"切图瞬切"的公认答案是**预解码邻居**，不是金字塔：QuickImageViewer 把它印在
/// README 上当卖点（"Instant image switching ✅ pre-decoded neighbours"，`preloadLookaside`），
/// viewskater 用滑窗 + 字节预算 LRU 实现同一件事。原来是前/后各 1 张：只要用户按得比
/// 解码快一格，下一张就是冷解；多给后台一格余量，连续翻页就能一直吃到命中。
///
/// ★ 不是越大越好：解码线程是**串行**的，预取请求排在当前解码之后，加大深度只增加
/// 后台工作量，不会让单张更快——超过某个深度反而把当前图的重解挤到队尾。
const PREFETCH_DEPTH: usize = 2;

/// 预读缓存上限（份）。装得下整个窗口即可：前后各 [`PREFETCH_DEPTH`]，另留 1 格余量。
const PREFETCH_CAP: usize = 2 * PREFETCH_DEPTH + 1;

/// 预读缓存的**字节预算**——把"只按条数封顶"升级成条数与字节的双闸（m34）。
///
/// 这里存的是**视口档**而不是原图（见 [`Viewer::cache_prefetch`]），所以这份账与图片
/// 原始大小无关：4K 满屏 ≈ 33 MiB/份、5K ≈ 59 MiB/份，256 MiB 分别放得下 7 / 4 份，
/// 驻留上限是**封死**的。
///
/// ★ 单份就超预算时**不许淘汰**（`cache_prefetch` 里用 `len() > 1` 守着）：那会把预读
/// 整个关掉，正是原注释警告过的"第二条闸反而把预读关掉"。
const PREFETCH_BYTES: usize = 256 * 1024 * 1024;

/// 鸟瞰缩略的边长上限。鸟瞰框本身只有 ~160 px，画一份原图尺寸的位图纯粹是浪费：
/// 6055×6055 那份要 139.9 MB，上屏层还得为它单独再上传一份预乘 Pixmap。
/// 512 已远超鸟瞰所需，换来的一份位图 ≤1 MB。
const NAV_THUMB_MAX: u32 = 512;

/// 一帧最多排多久（毫秒）。APNG 的 `delay_num/delay_den` 是 u32，畸形文件真会报出
/// 49 天一帧；那时"动图不动了"和"按 P 继续还是不动"都无从与实现缺陷区分。60 s 之上
/// 没有任何真实内容（最慢的幻灯片 GIF 也就十几秒），夹住它换的是"能往下动"。
const ANIM_MAX_DELAY_MS: u32 = 60_000;

/// 把解码器报的延时折算成排表用的 `Duration`：夹在 `[1, ANIM_MAX_DELAY_MS]`。
fn anim_wait(delay_ms: u32) -> Duration {
    Duration::from_millis(delay_ms.clamp(1, ANIM_MAX_DELAY_MS) as u64)
}

/// 交给编辑/分析型插件的那一份整图。
///
/// 用结构体而不是五元组：`(PathBuf, u32, u32, Arc<Vec<u8>>, (u8, bool))` 会被 clippy 判成
/// `type_complexity`，而拆个 `type` 别名只是让 lint 闭嘴——调用点照样要数下标，`orient`
/// 那个 `(u8,bool)` 到底是谁、什么时候必须用，全在字段名里才读得懂。
///
/// `Clone` 是**指计数克隆**（`Arc`）：一次建菜单要发给 N 枚插件就是 N 次 clone，
/// 不许有一份 32 MB 像素跟着复制。
#[derive(Clone)]
pub struct ForPlugin {
    pub path: PathBuf,
    /// **屏上朝向**下的宽度（旋转过就可能与原图对调）。
    pub width: u32,
    /// **屏上朝向**下的高度。
    pub height: u32,
    /// 与 `width×height` 对应的 RGBA8（已按 [`Self::orient`] 转正）。
    pub rgba: Arc<Vec<u8>>,
    /// 这一次转正用的 `(rot, flip)`。落地还原必须用**它**的逆变换，不是此刻的朝向——
    /// 理由与用例名见 [`crate::core::viewer::Viewer::install_edited_displayed`]。
    pub orient: (u8, bool),
}

/// 取色器的一次采样：屏幕指向的那个像素的颜色 + 它在**原图**里的坐标。
///
/// 颜色取自「上屏位图」（所见即所得：当前显示档、旋转、镜像都算在内）；
/// 缩小/放大时那是本地重采样后的像素，不是文件里的原始字节——这是唯一诚实的
/// 口径（区域渲染时宿主手里根本只有切片，没有整图字节）。
/// 坐标则是**原图像素坐标**——降采样档、SVG 切片、旋转后这两套坐标并不相等，
/// 拿位图坐标冒充原图坐标是取色器最容易写错的一处。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub rgba: [u8; 4],
    pub at: (u32, u32),
}

/// 当前已上屏的一张图。
pub struct Loaded {
    pub path: PathBuf,
    /// 原图像素尺寸（状态栏与倍率计算都用它，不是位图尺寸）。
    pub natural: (u32, u32),
    /// 实际解码尺寸（可能因"缩到上屏尺寸"而小于原图）。也是旋转/翻转的**基准**尺寸。
    pub decoded: (u32, u32),
    /// 上屏位图 [`Self::image`] 是 base 按哪一档派生的（**未经旋转/镜像**的尺寸）。
    ///
    /// 脏判定就靠它：缩放后该档与 `image` 实际尺寸不一致才重派生，一致就一帧都不动。
    /// 不能拿 `image.width/height` 直接比——那份被 rot 交换过轴。
    pub displayed: (u32, u32),
    /// 解码原始像素（屏幕级，非预乘 RGBA8）。
    ///
    /// 保留它而不是只留 `Image`：旋转/翻转要在**像素层**做（windui `draw_image`
    /// 没有旋转/翻转参数），切换时拿它重算一份再重建 `Image`，即时生效不走解码线程。
    /// 而且它还是缩放派生的**源**（见模块头「位图的两档」）——档位来回改只重采样，不重解。
    ///
    /// ★ 尺寸账：首解与预读拿到的是视口档（几 MB），一次放大后升级成**原图档**——
    /// 顶格 `MAX_PIXELS` 时这一份就是 160 MB，比 `image` 那份大一到两个数量级。
    /// 这不是漏账而是本轮的设计（换来"缩放永不回头找解码器"），上限由 `MAX_PIXELS` 闸给定。
    ///
    /// `Arc<Vec<u8>>` 而不是 `Vec<u8>`：≥100% 档的 `image` **就是这一份**（共享、不复制），
    /// 见 [`Loaded::image`] 与 `RgbaImage::shared`；M2 把它挪到解码线程也要求跨线程引用计数。
    pub base_rgba: Arc<Vec<u8>>,
    /// 上屏位图（非预乘 RGBA8 + `gen` 代号）。随旋转/翻转重建（gen 随之 +1，
    /// 上屏层据此判断框架纹理是否需要重新上传）。
    ///
    /// 显示档 == 原图档时（放大到 ≥100%），它的像素与 [`Loaded::base_rgba`] 是同一块内存。
    pub image: RgbaImage,
    /// 位图覆盖范围：`None` = 整图；`Some(r)` = SVG 可视区域切片（见 [`Region`]）。
    pub region: Option<Region>,
    /// 动画状态（当前帧 / 总帧数 / 帧延迟）。静态图恒为 [`Anim::STATIC`]。
    ///
    /// 下一帧的推进由 [`Viewer::tick`] 按这里的三元组安排：`delay_ms > 0` 即动画，
    /// `count == 0` 表示总帧数未知（GIF 惰性数帧），推进时只 +1、由解码器侧在越界时回绕。
    pub anim: Anim,
    /// 鸟瞰整图缩略的像素（未变换 RGBA + 尺寸）：首帧 whole 解码就有。
    ///
    /// SVG 放大后主位图只是可视切片，直接拿去当鸟瞰缩略图，离视角远的部分
    /// 全是空白（2026-10-10 用户报障）。菠萝 NavigatorView 的语义是**鸟瞰永远
    /// 显示完整图片**——所以整图这份要单独留着，切片重渲不覆盖它。
    ///
    /// ★ 它是**缩略**而不是原图尺寸的副本：来源本身也先收到 [`NAV_THUMB_MAX`]
    /// 再存（[`Viewer::nav_thumb_of`]），真正被画的 [`Self::nav_image`] 同样边长封顶
    /// （512² ≈ 1 MB）。原来 `nav_image_of` 直接拿原图尺寸走 `transformed`——恒等变换
    /// 也要 `to_vec()`，6055×6055 上是 base 之外的**第二份** 139.9 MB，上屏层还要为它
    /// 再建一份预乘 Pixmap、每次换档重传 0.09–0.14 s
    /// （M9 查账到的就是这一笔，判据见 本仓历史记录）。
    ///
    /// ★ 为什么**来源**也要缩：SVG 放大后主位图换成可视切片，而这份"整图来源"还指着
    /// 首帧那次 whole 渲染的缓冲——4K 视口就是 ≈33 MB（8K ≈133 MB）为一个 ≤512 px 的
    /// 鸟瞰被永久钉住（2026-10-18 第四批审计）。先缩到 512 再存，画质一分不损：
    /// 鸟瞰本来就只画 512 档，而"先缩后转"与 `derive_level` 是同一条顺序。
    pub nav_thumb_rgba: Option<(Arc<Vec<u8>>, u32, u32)>,
    /// 鸟瞰实际画的那份位图；`None` = 退回主位图（来源本来就不比缩略档大）。
    ///
    /// 它只跟两件事有关：**整图来源**与 **rot/flip**。缩放/换档都不改变它，
    /// 所以 [`Viewer::rebuild_image`] 按 [`Self::nav_key`] 判断，不每次重建。
    pub nav_image: Option<RgbaImage>,
    /// `nav_image` 是按哪个朝向建的 `(rot, flip)`；装新解码结果时连来源一起刷新。
    pub nav_key: (u8, bool),
}

// 编辑覆盖账本（涂鸦等编辑型插件回写），见 [`edited`] 模块。

/// 一份预读完成的解码结果（等待用户翻到它）。
pub(super) struct Prefetched {
    pub(super) path: PathBuf,
    pub(super) img: Decoded,
}

impl Prefetched {
    /// 这份位图占多少字节（`w * h * 4`，RGBA8）。供字节预算 [`PREFETCH_BYTES`] 计费。
    ///
    /// 按**已解码位图**算而不是按原图算：缓存里存的就是这一份，原图多大与驻留无关。
    pub(super) fn bytes(&self) -> usize {
        self.img.bitmap.width as usize * self.img.bitmap.height as usize * 4
    }
}

pub struct Viewer {
    pub(super) decoder: Rc<Decoder>,
    /// 共享配置（循环翻页 / 长图适应读这里）。设置窗改动即时生效。
    pub(super) config: Rc<RefCell<Config>>,
    pub(super) gallery: Option<Gallery>,
    pub(super) loaded: Option<Loaded>,
    /// 已应用的编辑覆盖（path → 覆盖），见 [`edited`] 模块；只存内存，会话级。
    edited: std::collections::HashMap<PathBuf, edited::EditedKeep>,
    pub(super) error: Option<String>,
    /// 查询阶段（`iv_render(out=NULL)`）先到的元信息，仅在 `loaded` 为空时
    /// 作为 info bar / 「按原始像素开窗」的**预览**来源；渲染结果一到就作废。
    /// 契约：查询值不作数，以 [`Decoded.natural`] 为准（见 `decode::Meta`）。
    pub(super) preview_meta: Option<crate::core::decode::Meta>,

    /// 查询阶段元信息的进程级缓存（path → Meta）。
    ///
    /// 翻页到之前看过的文件时，不必等 MetaEvent 到达就能立即显示尺寸——
    /// 命中时 [`Self::navigate`] 直接从缓存取回填进 [`Self::preview_meta`]。
    /// `install` 落地后用渲染阶段的权威值覆盖缓存（契约：查询值仅预览）。
    pub(super) meta_cache: std::collections::HashMap<PathBuf, crate::core::decode::Meta>,

    /// 预读在途请求的世代号（同一时刻至多一个，见 [`Self::request_prefetch`]）。
    ///
    /// ★ 为什么坚持"一次一个在途"：解码线程串行，多枚预读请求只会排在队里，
    /// 不会更快，却会把当前图的重解挤到队尾——预取的目的正是**不打扰**当前这张。
    pub(super) prefetch_seq: Option<u64>,
    /// 本轮预算淘汰后停填；只有重新规划邻图才复位，动画/升级回包不能重启。
    pub(super) prefetch_stopped: bool,
    /// 已预读完成的邻居图（LRU：尾端最近，字节预算见 [`PREFETCH_BYTES`]）。
    pub(super) prefetched: Vec<Prefetched>,
    /// 上次翻页的方向（+1 前进 / -1 后退 / 0 未定向，如直接打开或跳转）。
    ///
    /// 连续翻页的方向是**可预测**的：刚往前翻，下一张多半还是往前。
    /// [`Self::request_prefetch`] 据此把同方向的窗口开得更深，赌对了就全程零解码。
    pub(super) last_dir: i8,
    /// 主画面请求发出的时刻（埋点用，见 [`Self::apply`] 的端到端耗时）。
    ///
    /// 只有这一段能量化"用户实际等了多久"——`decoders` 侧的 `[decode-trace]` 只覆盖
    /// 解码内部五段，量不到"没命中预读、在解码线程里排队"的那部分。
    ///
    /// 用 `Cell` 而不是 `Option<Instant>` 字段：[`Self::send_decode`] 是 `&self`
    /// （它的调用方大多正持有从 `self.gallery` 借出来的 `&Path`，改成 `&mut self`
    /// 会四处撞借用），而写时刻只是记一个 `Copy` 值，不需要可变借用。
    pub(super) main_sent_at: Cell<Option<Instant>>,

    /// 当前这枚主画面请求的"卡住"warn 是否已经写过（m71）。
    ///
    /// 每发一枚新的 Main 请求就清零；`tick` 只在越过 [`STALL_WARN_AFTER`] 时写一次，
    /// 免得每 16 ms 的轮询把日志刷满。与 `main_sent_at` 成对：后者为空时这个位无意义。
    pub(super) stall_logged: Cell<bool>,

    /// 状态栏上一次已经报出去的"已等 N 秒"里的 N（m71）。
    ///
    /// 本项目"没人标脏就不合成"，而等待秒数是**时间的函数**——不靠这里在整秒处
    /// 主动返一次"要重绘"，状态栏会永远停在请求发出时那句话（真机取证：日志 10 s
    /// 已写出 warn，截图上状态栏仍是「1/3 · 载入中…」）。存整数秒是刻意的：
    /// 每秒只重绘一次，不把卡住变成 60 fps 的烧电。
    pub(super) wait_shown_secs: Cell<Option<u64>>,

    /// 底部缩略图条的解码账本（在途请求 + 内存缓存）。见 [`Thumbs`]。
    pub(super) thumbs: Thumbs,

    pub(super) fit: bool,
    pub(super) zoom: f32,
    /// **屏上**那份位图的朝向：顺时针累计旋转（0–3，×90°）。
    ///
    /// 所有几何（`dst_rect`、`scale`／`fit_scale_in`、SVG 正/逆映射）都按它算——屏上画的像素就是
    /// 这个朝向，拿目标朝向去算会把旧图拉伸。改它只有一个地方：新位图真正上屏时。
    pub(super) rot: u8,
    /// **屏上**的水平镜像（对旋转**之后**的画面生效）。
    pub(super) flip: bool,
    /// 用户按出来的**目标**朝向（R / M）。与 `rot/flip` 分开是因为大图的重建要
    /// 几十到一百多毫秒：先记目标、把活交给派生线程，屏上像素到了再换 `rot/flip`。
    /// 小图（`ORIENT_SYNC_PIXELS` 以下）仍然当场算，两个值同步落地。
    pub(super) want_rot: u8,
    pub(super) want_flip: bool,
    /// 图片中心相对视口中心的偏移（逻辑像素）。
    pub(super) pan: (f32, f32),
    /// 最近一次 `paint` 的内容矩形（逻辑坐标：x, y, w, h）。
    ///
    /// 存整块而不是只存宽高，是因为缩放需要**绝对**坐标做锚点：
    /// 窗口在屏幕上挪过之后，只有相对的"中心在 (w/2, h/2)"算不出鼠标下面那一点。
    pub(super) view: (f32, f32, f32, f32),
    pub(super) dpi: f32,

    /// 当前"想要"的结果世代号；不匹配的回包一律丢弃。
    pub(super) want_seq: u64,
    /// 需要重新解码的时刻（防抖用）。`None` = 无需重新解码。
    pub(super) need_at: Option<Instant>,
    /// 最近一次**翻页**的时刻（m58，原图档升级让路给翻页的判据）。
    ///
    /// 为什么只记翻页不记缩放/平移：让路的理由正是"用户的手还在翻页，这一张已经不
    /// 在屏上了"。缩放与平移不改变"当前这张就是用户看的那张"，所以它们不该重置宽限期。
    pub(super) last_nav_at: Cell<Option<Instant>>,
    /// 已经为这张图发过**原图档**升级请求的 `(路径, 那一枚请求的 seq)`（见 [`Self::tick`]）。
    ///
    /// 为什么需要这道闩：升级请求要的是"整图"（框 0 = 原始尺寸），而解码器若不如实给到
    /// 整图（未来按 `target` 降采样的解码器、或解码器本身的尺寸自限），`decoded` 就永远
    /// 小于 `natural`——没有这道闩，`install → mark_need → 再请求 → install` 会转成
    /// 每秒一次的解码死循环。换图（`reset_view`）即清。
    ///
    /// ★ 为什么要连 `seq` 一起记：只记路径的话，"请求被后来的主画面请求顶掉、根本没落地"
    /// 这一种情形也会被闩住，于是这张图永久拿不到原图档。带上 seq 后闩的读法就是
    /// 「那一枚还在途」，被顶掉即失效。
    pub(super) native_requested: Option<(PathBuf, u64)>,

    /// 上一发**矢量缩放重渲**（m189）要的目标档 `(路径, 目标像素)`。
    ///
    /// 只服务"按目标重画但没有区域通道"的矢量（[`crate::core::formats::needs_zoom_rerender`]）：
    /// 它们放大后必须请解码器按更大的 target 整幅重画。这道账防两件事——
    /// ① 解码器不理 `target`（画出来仍小于目标）时，16 ms 一轮的 tick 不会无限重发；
    /// ② 同一次缩放里抖出来的重复请求：目标没比上次大出一档（25%）就再等。
    /// 换图（`reset_view`）即清。
    pub(super) vector_zoom_req: Option<(PathBuf, (u32, u32))>,

    /// 装图代号：每次 [`Self::install`] 自增。派生线程的回包带着它，换了图之后
    /// 晚到的旧档一律丢弃——旧图像素盖在新图上就是"闪回上一张"。
    pub(super) epoch: u64,
    /// 档位派生线程。**首次异步换档才起**（`Viewer::new` 不起线程：单测与探针会
    /// 造几十份 Viewer，每条都挂着就是几十个僵尸）。
    pub(super) levels: Option<crate::core::derive::LevelThread>,
    /// 在途那一档的指纹 `(epoch, want, rot, flip)`。同一档不重复投递——否则 16 ms
    /// 一次的轮询会把同一件活儿不停塞给线程，而它本来就追不上。
    pub(super) inflight: Option<(u64, (u32, u32), u8, bool)>,

    /// 动画下一帧的上屏时刻。`None` = 非动画 / 下一帧已在途。
    ///
    /// ★ **只由 [`Self::install`] 安排**（解码结果上屏成功且 delay>0 才排下一帧），
    /// tick 只消费——否则解码慢于帧延迟时会排队堆积请求，画面越追越乱。
    pub(super) anim_next_at: Option<Instant>,
    /// 动画时间线上"下一帧**该显示**的时刻"（m75）。`None` = 时间线作废，下一帧重新起表。
    ///
    /// 为什么要单独一份而不是直接用 [`Self::anim_next_at`]：那一枚是"发请求的闸"，
    /// `tick_anim` 一触发就把它清空（同一时刻只许有一个在途请求），而时间线必须跨过
    /// 这一次触发继续走。排表基准错了会直接改到用户看见的东西：从"上一帧落地"起算，
    /// 周期就成了 **成本 + 延时**——实测 25 fps 的 GIF 屏上只有 13 fps（小图）到 9 fps（3 MP）。
    /// 正确写法是从"上一帧应显示的时刻"接着加延时，落后时夹到"现在"（立刻发、不补等），
    /// 于是周期 = `max(延时, 成本)`：文件多快就多快，追不上就自然降速，两种都不堆积。
    pub(super) anim_due_at: Option<Instant>,
    /// 动画暂停（P 键切换）。只停推进，不影响其他查看操作。
    pub(super) anim_paused: bool,
    /// 逐帧按键的**在途目标帧**：`Some(n)` = 已经按到第 n 帧、回包还没上屏。
    ///
    /// ★ 为什么需要它：目标帧号原本按"当前显示的这一帧"算，而后退一步可能要付一次
    /// 「重建 + 快进」（几十到两百毫秒）。这段时间里 `loaded` 还是旧帧，连按就每次都算出
    /// 同一个目标 → 真机实测按 91 下只退 1 帧。有了在途目标，连按就是连续 91 个不同的
    /// 目标，旧回包由 `apply` 的 seq 判据丢掉，最后那一个落地。
    /// 任何一帧上屏（[`Self::install`]）都把它清空——此后以"屏幕上的帧"为准。
    pub(super) step_target: Option<u32>,
    /// 一次性提示（m50-C）：按了"当前这张图做不到的动作"时挂上来，`status()` 在
    /// [`NOTE_TTL`] 之内优先显示它。
    ///
    /// ★ 为什么用"时刻"而不是"下一个动作清除"：后者要在每一处状态变更上挂清除点
    ///   （翻页、缩放、滚轮、按钮……），漏一处就有一句提示赖在浮条上不走；而浮条本来
    ///   就是每帧重算的。也不用计时器/新的重绘源：任何一次重绘都会重新比对，过期自然
    ///   回落正常行；换图 / 翻页这类"用户已经在看新东西"的时刻仍显式清掉（见
    ///   [`Self::open`] 与 [`Self::navigate`]）。
    pub(super) note: Option<(&'static str, Instant)>,

    pub(super) drag: Option<Drag>,

    /// 当前图的世代号（每次打开新图 / 翻页 / 当前图解码完成上屏 自增）。
    /// 属性窗据此判断"当前图是否已变"——变了就重算并刷新那扇窗的内容，
    /// 不必每帧轮询。与 `want_seq`（解码请求序号）无关，这是给 UI 看的"版本戳"。
    pub(super) image_gen: u64,
}

pub(super) struct Drag {
    /// 按下时的指针位置（逻辑，绝对坐标）。
    pub(super) start: (f32, f32),
    /// 按下时的平移量。
    pub(super) origin: (f32, f32),
}

mod edited;
mod nav;
mod overlay;
mod paint;
mod prefetch;
mod present;
mod schedule;
mod transform;
mod zoom;

#[cfg(test)]
mod tests;
