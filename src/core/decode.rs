//! 后台解码调度：**一个解码线程 + 世代令牌**。
//!
//! 为什么解码必须离开 UI 线程：一张 4000×3000 的 JPEG 全解是几十毫秒级，同步做在
//! 按键回调里会直接卡住翻页手感；而在看图器里翻页是最频繁的动作。
//!
//! 为什么只用一个线程：
//! - 解码本身是单张串行的，多线程只在"批量生成缩略图"时才有收益，而我们一次只显示一张；
//! - 多个线程会让「世代令牌」退化成多令牌，错序判定的复杂度白白翻倍。
//!
//! 世代令牌（`seq`）的作用：**丢弃过期结果**。连按右方向键时，前几张的解码结果会在
//! 用户已经翻过去之后才回来；没有令牌的话，旧图会把新图盖掉，表现为"翻页后画面跳回去"。
//!
//! 令牌有**两处消费点**：UI 侧 `Viewer::apply` 判 `seq != want_seq` 丢弃过期 Done；
//! 解码线程侧 [`decode_loop`] 判 `Main`/`Prefetch` 的 `seq < main_floor` 直接**不发**
//! 解码（空串 Err 占位 Done）——后者省的是 CPU，前者省的是像素覆盖。这一刀出自
//! 早期「手感断点清单」的 A1 条目（m07，已随 m47 收编删除；落地数字见 `HISTORY.md` 2026-10-14 段）。
//!
//! 取队顺序：**主画面插队**（[`Queue::pop`]）。令牌只作废"还没开始解"的过期请求，
//! 拦不住"排在前面那张正在解"，所以缩放重解必须能越过队首的缩略图与预读——否则
//! 用户放大后要等一整排后台解码才拿到清晰图（2026-10-17 大图实测 0.37s → 1.98s）。

use std::cell::Cell;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender as StdSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::core::pixels::{Decoded, MAX_EDGE, MAX_PIXELS};

/// SVG 可视区域请求（只有 svg/svgz 路由会带）。
///
/// 矢量图的正确打开方式是「只渲染看到的、按设备像素 1:1」：位图路线"整图缩到
/// 上屏尺寸"对矢量在大倍率下必然撞内存硬顶出马赛克（整图 41MP 顶格 vs 视口 2MP）。
#[derive(Debug, Clone, Copy)]
pub struct RegionReq {
    /// 区域左上角（图像坐标，图像像素）。
    pub origin: (f32, f32),
    /// 区域大小（图像像素）。
    pub size: (f32, f32),
    /// 输出密度（设备像素/图像像素）。
    pub density: f32,
    /// 文档的**自然尺寸**（图像像素）。
    ///
    /// ★ 为什么区域请求要带它：区域渲染只给出一块，画布尺寸**不是**文档尺寸；
    /// 而上层的几何映射（视口 ↔ 图像坐标）用的是文档自然尺寸。宿主回填
    /// [`Decoded::natural`] 时要有它——解码器那边不用给（它知道自己的自然尺寸，
    /// 区域坐标本来就在那个坐标系里）。
    pub doc: (u32, u32),
    /// 要重渲的**第几张**（帧号，多页文档里就是页号）。
    ///
    /// ★ 这条不带上就是"放大第 50 页看见第 1 页"：区域通道按号重画，而请求里
    /// 没有号就只能默认 0。页与帧合流之后它同时服务两者（与 `IvRegionReq`
    /// 的 `page_index` 同值，见 `decoders::decode::try_decode_region`）。
    pub index: u32,
}

/// UI → 解码线程。
#[derive(Debug, Clone)]
struct Req {
    seq: u64,
    path: PathBuf,
    /// 上屏需要的像素上限（0 = 不限）。解码后端按此值降采样，省内存与带宽。
    max_w: u32,
    max_h: u32,
    /// SVG 可视区域请求；`None` = 整图。
    region: Option<RegionReq>,
    /// 动画帧序号（0 = 首帧/静态）。非 0 仅对动画格式的整图请求有意义。
    frame_index: u32,
    /// 裸数据参数（仅 `.raw` 等无魔数文件带；`None` = 常规解码）。
    raw: Option<RawParams>,
    /// 请求语义：主画面 / 预读 / 缩略图。决定取消策略（见 [`ReqKind`]）。
    kind: ReqKind,
    /// 显式开关：这一次要**原生降采样**（`REQ_ALLOW_SCALED`），与 `kind` 无关。
    ///
    /// 为什么单列一位而不是只看 `kind`：缩略图天然要缩放，但主画面的**首解**同样
    /// 要——首屏要的是快，清晰度由随后那一次原图档升级补上（见 `viewer::tick` 的
    /// 升级路径）。两者语义不同，混在 `ReqKind` 里会逼出"主画面永远不缩放"这种
    /// 一刀切结论（m35）。
    allow_scaled: bool,
}

/// 这一次请求最终递给后端的缩放意图：[`Req::allow_scaled`] 或 `kind` 自带。
///
/// ★ 到处都要用这个函数，不许各处自己写 `kind.wants_scaled()`：这一位决定后端拿到的
/// 是"可以只解档位"还是"必须整解"，漏了就会把降采样出来的档位位图当整图交上去，
/// 之后放大永远只能停在那张档位图——比不解还糟（m35）。
fn scaled_of(req: &Req) -> bool {
    req.allow_scaled || req.kind.wants_scaled()
}

/// 解码请求的语义类别——决定 decode_loop 里的**取消**行为。
///
/// 单线程串行解码下，用户连按方向键会积压过期请求；`Main` 与 `Prefetch` 都参与
/// 过期判定（seq 小于 `main_floor` 即作废），只有 `Main` 会推 `main_floor`。
/// `Thumb` 与主画面解耦，不参与取消——胶片条上一屏缩略图不会因主画面翻页而作废。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqKind {
    /// 主画面：用户当前看的那张（含 tick 的缩放/平移重解）。推 main_floor。
    Main,
    /// 相邻图预读：允许被新主画面作废，但不推 main_floor。
    Prefetch,
    /// 缩略图：始终解完。
    Thumb,
}

impl ReqKind {
    /// 这一类请求**自带**原生降采样意图吗（契约 `REQ_ALLOW_SCALED`，见规范 §十三）。
    ///
    /// 只有缩略图自带。判据不是"尺寸小就无所谓"，而是**谁在等这张图、等到的是什么**：
    ///
    /// - `Main` / `Prefetch`：默认仍不缩放——它们的清晰度口径由随后那次原图档升级
    ///   兜底，缩放与否交给**每次请求**的 [`Req::allow_scaled`] 决定（m35：首解要快，
    ///   升级要真）。历史上这里写死"主画面永不缩放"，代价是首屏必须先整解一遍。
    /// - `Thumb`：240×180 的一格，端到端实测 DCT 1/8 快 **1.50–1.70×**（两序同向），
    ///   省下的主要是那 78 MB 整图缓冲和整图→格子的 24.9 ms 缩小。
    ///
    /// 会不会真缩，取决于候选有没有声明 `CAP_SCALED`——落到 [`crate::decoders`] 那边
    /// 就是"没声明 ⇒ flags 保持 0"。声明名单由各 DLL 的 `iv_caps()` 自报，宿主不维护。
    pub fn wants_scaled(&self) -> bool {
        matches!(self, ReqKind::Thumb)
    }
}

/// 该扩展名是否需要**外部参数**（宽/高/布局）才能解码。
///
/// ★ 清单来自解码器自述（[`crate::core::formats`]，由解码器层启动时装入）：实现了可选导出
/// `iv_render_raw` 的解码器认领哪些扩展名，哪些就走参数窗。宿主不手写第二份
/// 后缀表——删掉解码器，裸数据路线随之消失，不会出现"弹了参数窗却没人接"的错位。
pub fn is_raw_ext(ext: &str) -> bool {
    crate::core::formats::is_raw(ext)
}

/// 裸像素解码参数（用户在参数窗里给的宽/高/布局）。
/// `layout` 取 `PF_*` 常量（含仅输入用的 `PF_BGRA8`），见规范 §十二。
///
/// 定义在 core：`Config` 要记住「上次使用值」（跨会话），解码器层只消费不定义——
/// 存储方永远不该依赖解码实现的类型布局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawParams {
    pub width: u32,
    pub height: u32,
    pub layout: u32,
}

/// 参数窗下拉的布局值（顺序与 UI 标签一致）。
pub const RAW_LAYOUTS: [u32; 12] = [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 12, 13];

/// 真正没有文件头、需要宽/高/布局的后缀。相机 RAW（CR2/3FR/NEF…）不在此列。
const HEADERLESS_EXTS: &[&str] = &[
    "bgra", "gray", "gry", "i420", "nv12", "rgb", "rgba", "uyvy", "yuv", "yuy2", "yv12",
];

pub fn is_headerless_ext(ext: &str) -> bool {
    HEADERLESS_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

pub fn is_headerless_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_headerless_ext)
}

/// 后缀 → 布局下标（[`RAW_LAYOUTS`] 的下标）。
pub fn layout_index_for_ext(ext: &str) -> Option<usize> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "gray" | "gry" => 0,
        "rgb" => 1,
        "rgba" => 2,
        "bgra" => 6,
        "yuy2" => 7,
        "uyvy" => 8,
        "i420" | "yuv" => 9,
        "yv12" => 10,
        "nv12" => 11,
        _ => return None,
    })
}

fn layout_bytes_per_pixel(layout_index: usize) -> Option<(u64, u64)> {
    Some(match layout_index {
        0 => (1, 1),
        1 => (3, 1),
        2 | 6 => (4, 1),
        3 => (2, 1),
        4 => (6, 1),
        5 => (8, 1),
        7 | 8 => (2, 1),
        9..=11 => (3, 2),
        _ => return None,
    })
}

/// 从后缀 + 文件长度反推首开参数。
///
/// 裸文件没有尺寸头，这是能做的最强推断：后缀定布局，长度定 `宽×高`，
/// 再在常见宽度里找整数高度。推不出返回 `None`，由参数窗兜底；绝不硬猜。
pub fn infer_raw_params_for(ext: &str, len: u64) -> Option<RawParams> {
    let index = layout_index_for_ext(ext)?;
    let (num, den) = layout_bytes_per_pixel(index)?;
    let total = len.checked_mul(den)?;
    let is_yuv = matches!(index, 7..=11);
    let mut best: Option<(f64, RawParams)> = None;
    // First try standard photo aspects. This turns a 3840x2160 dump into
    // 16:9 instead of the first width that happens to divide evenly.
    for aspect in [
        4.0 / 3.0,
        3.0 / 2.0,
        16.0 / 9.0,
        1.0,
        5.0 / 4.0,
        16.0 / 10.0,
    ] {
        let ideal = ((total as f64 / num as f64) * aspect).sqrt();
        let center = ideal as u64;
        for w in center.saturating_sub(8)..=center.saturating_add(8) {
            let denom = w * num;
            if w < 2 || total % denom != 0 {
                continue;
            }
            let h = total / denom;
            if h == 0
                || h > MAX_EDGE as u64
                || (is_yuv && (!w.is_multiple_of(2) || !h.is_multiple_of(2)))
                || w.saturating_mul(h) > MAX_PIXELS
            {
                continue;
            }
            let error = (w as f64 / h as f64 - aspect).abs() / aspect;
            if error <= 0.01 && best.as_ref().is_none_or(|(e, _)| error < *e) {
                best = Some((
                    error,
                    RawParams {
                        width: w as u32,
                        height: h as u32,
                        layout: RAW_LAYOUTS[index],
                    },
                ));
            }
        }
    }
    if let Some((_, params)) = best {
        return Some(params);
    }

    // No standard aspect (e.g. a deliberate strip): fall back to common widths.
    const WIDTHS: &[u64] = &[
        4096, 1920, 2048, 3840, 2560, 1280, 640, 1024, 1600, 3000, 3200, 4000, 5120, 5760, 6000,
        6144, 6400, 7680, 8000, 8192, 10000, 12000, 16384,
    ];
    for &w in WIDTHS {
        let denom = w * num;
        if total % denom != 0 {
            continue;
        }
        let h = total / denom;
        if h == 0
            || h > MAX_EDGE as u64
            || (is_yuv && (!w.is_multiple_of(2) || !h.is_multiple_of(2)))
            || w.saturating_mul(h) > MAX_PIXELS
        {
            continue;
        }
        return Some(RawParams {
            width: w as u32,
            height: h as u32,
            layout: RAW_LAYOUTS[index],
        });
    }
    None
}

/// 读文件长度并推断；文件系统错误按「推不出」处理。
pub fn infer_raw_params(path: &Path) -> Option<RawParams> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let len = std::fs::metadata(path).ok()?.len();
    infer_raw_params_for(ext, len)
}

// ── 裸数据参数的会话级存放（UI 线程写、读；解码线程只见请求快照）────
//
// 按路径记一份，**会话内常驻**（缩放/窗口变化会反复重解同一路径，参数必须
// 一直可取）。跨会话的「上次使用值」记忆在 [`Config`] 里，这里只管本会话。

fn raw_params_store() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, RawParams>> {
    static STORE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, RawParams>>,
    > = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 记录某路径的裸数据参数（参数窗确认时调用）。
pub fn set_raw_params(path: &Path, params: RawParams) {
    raw_params_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_path_buf(), params);
}

/// 读取某路径的裸数据参数（UI 线程发请求时快照进 `Req`）。
pub fn get_raw_params(path: &Path) -> Option<RawParams> {
    raw_params_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(path)
        .copied()
}

/// 取这次解码该用的裸数据参数：会话里已有（参数窗确认过 / 首开推断过）就直接还，
/// 否则按后缀 + 文件长度推断一份并写回 store；推不出返回 `None`，交给参数窗。
///
/// ★ 主画面（`viewer::schedule::send_decode`）与缩略图（`core::thumbs::ensure`）
///   **必须共用这一处**。裸像素没有文件头，`raw = None` 的请求在解码器侧只会得到
///   「没有尺寸信息」，于是同一张图主画面看得见、条上那一格永远空着
///   （2026-11-08 用户实报 `noise.bgra`，1M 目录里 11 个裸像素后缀全部中招）。
///   相机 RAW / 普通图走早退：它们有头，不需要也不该去反推宽高。
pub fn ensure_raw_params(path: &Path) -> Option<RawParams> {
    if let Some(p) = get_raw_params(path) {
        return Some(p);
    }
    if !is_headerless_path(path) {
        return None;
    }
    let p = infer_raw_params(path)?;
    set_raw_params(path, p);
    Some(p)
}

/// 解码线程 → UI。数据走 `std::sync::mpsc`（不会自己叫醒 UI）；
/// 主画面回包另经 [`DONE_WAKER`] 叫醒一帧，见 `decode_loop` 尾部的说明。
#[derive(Debug)]
pub struct Done {
    /// 与发起请求时相同的世代号。
    pub seq: u64,
    pub path: PathBuf,
    /// `Ok` 里带原图尺寸 + 位图；`Err` 是可直接显示给用户的人话。
    pub out: Result<Decoded, String>,
}

/// 查询阶段的元信息（**未经渲染确认**，仅用于提前反馈给 UI）。
///
/// 为什么单列一个事件：ABI 的 `iv_render` 是两阶段——`out=NULL` 时解码器只回尺寸
/// （HEIF 读 `ispe`、RAW 只读头，微秒级），随后 `out=buffer` 才真解码。
/// 对 header-only 解码够用的格式，把这一段结果**提前**告诉 UI，info bar 与
/// "按图片原始像素开窗"就能在整图解码前动起来；对必须整解才能报尺寸的格式
/// （JXL/AVIF），Meta 与 Frame 几乎同帧抵达，无害。
///
/// ★ 契约：**查询值仅作预览**，最终以 [`Decoded`]（渲染阶段回填）为准。宿主遇到两者
/// 不一致时以渲染值覆盖 UI（见 `Viewer::install`）；解码器应保证两阶段一致。
///
/// 只带尺寸：方向已在解码阶段折进 natural，帧数/页数另有权威来源
/// （动画走 [`Decoded::anim`]，多页尚无消费者）。复制过来只会变成"只写不读"的字段，
/// 并在缓存被占位值覆盖时埋下假数据。
#[derive(Debug, Clone, Copy)]
pub struct Meta {
    /// 应用 EXIF 方向**后**的自然尺寸。
    pub natural: (u32, u32),
}

/// 一次查询阶段的上抛事件（与 `Done` 并行、各自通道）。
#[derive(Debug)]
pub struct MetaEvent {
    pub seq: u64,
    pub path: PathBuf,
    pub meta: Meta,
}

// ── 查询阶段元信息的进程级通道 ──────────────────────────────
//
// 为什么用 OnceLock 静态而不是把 `Sender` 沿 `DecodeBackend::decode` 一路传下去：
// 那条链要穿过 5 层（decode_loop → decode_one → decode_route → with_backend →
// Host::try_decode → decode_with），签名会污染 ABI 抽象（Backend 不该知道 Meta）。
// OnceLock + 一次性 set 语义与 `BACKEND`/`SVG` 完全对齐——运行期注入点都在这一层，
// 单进程一次装配。多次 `runtime::run`（仅测试场景）不覆盖首发，测试若不关心 Meta
// 就完全忽略这条通道。
static META_TX: std::sync::OnceLock<StdSender<MetaEvent>> = std::sync::OnceLock::new();

/// 装入查询事件的接收端（幂等）。
///
/// ★ 多窗口（2026-11-04）：这只是**兜底**。GUI 每个看图窗口各有一条解码线程、
/// 各自带着自己的回程（见 [`DecodeSink`]），thread-local 优先——全局单槽只为
/// 「没有 sink 的直调路径」保留。
/// **当前宿主无调用方**（m282 全仓调用图确认）：与 [`install_done_waker`] 成对，
/// 后者仍有测试在用；删这个槽要连带动 [`publish_meta`] 的兜底分支，故保留。
pub fn install_meta_sender(tx: StdSender<MetaEvent>) {
    let _ = META_TX.set(tx);
}

/// 一条解码线程的**回程**：Meta 通道 + 主画面唤醒动作。
///
/// ★ 多窗口（2026-11-04）之前这两样是进程级单槽（`META_TX` / `DONE_WAKER`），
/// 因为进程里只有一条解码线程、一个窗口。多个看图窗口各起一条解码线程之后，
/// 单槽的后果是"第二个窗口的 Meta 全被丢进第一个窗口的队列、唤醒也只叫醒第一个窗口"——
/// 症状是第二窗的 info bar 与"按原始像素开窗"要等整图解完才动。
///
/// 把回程随线程走而不是继续加全局注册表：解码线程与窗口一一对应，回程天然属于线程；
/// 注册表还要额外的注销路径（窗口关掉要摘，漏了就是往已销毁窗口的通道里发）。
pub struct DecodeSink {
    pub meta: StdSender<MetaEvent>,
    pub wake: Box<dyn Fn() + Send + Sync>,
}

thread_local! {
    /// 本解码线程的回程（`decode_loop` 在进循环前装入）。非解码线程恒为 `None`。
    static SINK: std::cell::RefCell<Option<DecodeSink>> = const { std::cell::RefCell::new(None) };
    /// 本**解码 worker** 线程的 Meta 通道（2026-11-04）。
    ///
    /// Meta 是解码线程自己发的（`publish_meta_from_current_req` 在 `iv_render` 的查询
    /// 阶段），而回包与唤醒发生在监督者线程。两个线程各拿各自那一半，所以这里只放
    /// Sender —— 它是 `Clone`，wake 那半个（`Box<dyn Fn>`）只归监督者。
    static META_SINK: std::cell::RefCell<Option<StdSender<MetaEvent>>> =
        const { std::cell::RefCell::new(None) };
}

/// 发出一条查询事件。通道未装或接收端已释放时静默——Meta 只是"锦上添花"，
/// 拿不到就退回"Frame 到达再刷 UI"的旧行为，绝不因此拖慢或打断解码。
pub(crate) fn publish_meta(ev: MetaEvent) {
    // 解码 worker 优先（它才是真正在解的那条线程）。
    let worker_tx = META_SINK.with(|m| m.borrow().clone());
    if let Some(tx) = worker_tx {
        let _ = tx.send(ev);
        return;
    }
    // 直调路径（探针 / 示例 / 无 worker 的老单线程解码）。
    let local = SINK.with(|s| s.borrow().as_ref().map(|k| k.meta.clone()));
    match local {
        Some(tx) => {
            let _ = tx.send(ev);
        }
        None => {
            if let Some(tx) = META_TX.get() {
                let _ = tx.send(ev);
            }
        }
    }
}

// ── 主画面回包后的 UI 唤醒 ────────────────────────────────
//
// `std::sync::mpsc` 只搬运数据，**不会**叫醒 UI：没有这次唤醒，回包要等下一次
// `on_interval`。实测（m90）Windows 定时器按 15.6 ms 量化，请求的 16 ms 实际
// ~31 ms 一拍——小图翻页的"按键→上屏"里有一半是这段空等。
//
// 为什么不把 `Sender` 传下来：与 `META_TX` 同一条理由（穿过 5 层签名去拿一个
// 只有宿主关心的唤醒动作），且解码线程起在 `App` 之前，构造顺序上也拿不到。
static DONE_WAKER: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// 装入「主画面回包已入队」的唤醒动作（幂等）。由 `runtime::run` 在事件循环开始前调用。
///
/// ★ 多窗口（2026-11-04）：同 [`install_meta_sender`]，这是**兜底**；GUI 走
/// [`DecodeSink::wake`]（随解码线程、也就是随窗口），本槽只剩直调路径在用。
pub fn install_done_waker(w: Box<dyn Fn() + Send + Sync>) {
    let _ = DONE_WAKER.set(w);
}

fn wake_for_main() {
    let local = SINK.with(|s| match s.borrow().as_ref() {
        Some(k) => {
            (k.wake)();
            true
        }
        None => false,
    });
    if !local {
        if let Some(w) = DONE_WAKER.get() {
            w();
        }
    }
}

// 当前解码请求的世代号（0 = 无上下文，如探针/直调）。
//
// 与 `CURRENT_PATH` 同生同灭：解码线程进入 `decode_one` 前设，出来后清。
// 之所以走 thread-local 而不是塞进 `DecodeBackend::decode` 参数：那条签名属于
// 依赖倒置的接缝（core↔decoders），多带"仅宿主自己用"的元信息会污染抽象。
thread_local! {
    static CURRENT_SEQ: Cell<u64> = const { Cell::new(0) };
    static CURRENT_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_current_req(seq: u64, path: &Path) {
    CURRENT_SEQ.with(|c| c.set(seq));
    CURRENT_PATH.with(|p| *p.borrow_mut() = Some(path.to_path_buf()));
}

pub(crate) fn clear_current_req() {
    CURRENT_SEQ.with(|c| c.set(0));
    CURRENT_PATH.with(|p| *p.borrow_mut() = None);
}

/// 从当前请求上下文发出一条查询事件；无上下文（seq=0）时 no-op。
///
/// 给解码器宿主在 `iv_render(out=NULL)` 成功后调用——seq 为 0 意味着调用方是
/// `probe_decode` 之类的直调路径，没有 UI 在等，硬发反而把过期的 seq 灌进队列。
pub(crate) fn publish_meta_from_current_req(meta: Meta) {
    let seq = CURRENT_SEQ.with(|c| c.get());
    if seq == 0 {
        return;
    }
    let Some(path) = CURRENT_PATH.with(|p| p.borrow().clone()) else {
        return;
    };
    publish_meta(MetaEvent { seq, path, meta });
}

/// 待解队列：一把锁 + 一个条件变量，出队时**主画面插队**。
///
/// # 为什么不用 `std::sync::mpsc`
///
/// 插队要求"取之前先看一眼队里有没有 `Main`"，也就是非阻塞轮询；而 std mpsc 的
/// `try_recv` 在**发送端已 drop、队里还有货**时会返回 `Disconnected` 并把接收端
/// 永久标记为已断，剩下的请求静默丢失（rust-lang/rust#34087）。2026-10-17 加插队
/// 时正是踩在这里——三条单测当场抓到"四条请求只解了一条"。自己持有 `VecDeque`
/// 就没有这个坑，顺带把"队列长度"这种将来要做背压的账也摆在明面上。
///
/// # 取法
///
/// 队里有 `Main` → 解它（缩略图与预读往后放）；否则按到达顺序 FIFO。队空则挂在
/// 条件变量上等，不空转。
///
/// 已作废的 `Main`（`seq` 落后于 floor）仍会被选中，但 `decode_loop` 下一行就短路
/// 成空回复，代价是一次出队——比重解便宜一万倍，不值得再加一层判据。
struct Queue {
    pending: Mutex<VecDeque<Req>>,
    have: Condvar,
    closed: AtomicU64,
}

impl Queue {
    fn new() -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
            have: Condvar::new(),
            closed: AtomicU64::new(0),
        }
    }

    /// 入队并唤醒工作线程。`false` = 队列已关（`Decoder` 已释放），请求被丢弃。
    fn push(&self, req: Req) -> bool {
        if self.closed.load(Ordering::Relaxed) != 0 {
            return false;
        }
        // 锁中毒只可能由解码线程 panic 造成，而每张解码都套了 `catch_unwind`；
        // 真撞上也不把整条链打死，拿回数据继续用（与 `config`/`with_backend` 同策）。
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(req);
        self.have.notify_one();
        true
    }

    /// 出队：`Main` 优先，其余按到达顺序。队列关闭且排空后返回 `None`。
    ///
    /// ★ 预读**不**插缩略图那一档（判废于 m99）：想过"胶片条排在队里的 1–3 枚缩略图会挡
    ///   住下一次翻页的预读"，三档节拍（150 / 250 / 400 ms）+ 60 张单圈语料都量不出命中率差
    ///   （两臂 94–95%，见 本仓历史记录）；而代价是确定的——连续翻手中格子填得更慢。
    ///   无读数的收益不付代价，故留在 FIFO。
    fn pop(&self) -> Option<Req> {
        let mut q = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(i) = q.iter().position(|r| r.kind == ReqKind::Main) {
                return q.remove(i);
            }
            if let Some(r) = q.pop_front() {
                return Some(r);
            }
            if self.closed.load(Ordering::Relaxed) != 0 {
                return None;
            }
            q = self.have.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// 关闭并叫醒挂在 `wait` 上的工作线程（`Decoder` 释放时调用）。
    fn close(&self) {
        self.closed.store(1, Ordering::Relaxed);
        self.have.notify_all();
    }

    /// 关闭**并丢掉还没开始的请求**。
    ///
    /// 窗口关掉之后没有人会收这些结果：留着它们，监督者会一笔一笔解完（几十秒的 CPU），
    /// 而且同格式的**其它窗口**得陪它排队——m230 的 S3 臂就是这么抓到的：关掉一扇窗后，
    /// 队列里还剩 4 条，看门狗在 10 s 后仍报 `pending=4 run_ms=-1 phase=idle`。
    ///
    /// 与 [`Queue::close`] 分开是有意的：`close` 的语义只是"不再收新请求、排空即退出"，
    /// 好几条队列用例依赖它；"连没开始的也不要"只发生在句柄真的没了的时候。
    fn discard_pending(&self) -> usize {
        // 先清队列再关闭：`close` 会唤醒等着的监督者，反过来写的话它醒来可能先取走一条
        // 本该丢掉的请求（窗口已经没了，谁也收不到那个结果）。
        //
        // 这里逐笔 `note_done`；**正在解的那一笔**由 `Decoder::drop` 按 owner 一次销账
        // （那一笔已经出队，队列里找不着它）。
        let dropped: Vec<u64> = {
            let mut q = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let seqs: Vec<u64> = q.iter().map(|r| r.seq).collect();
            q.clear();
            seqs
        };
        for &seq in &dropped {
            crate::core::decode_watchdog::note_done(seq);
        }
        self.close();
        dropped.len()
    }
}

/// 每份 `Decoder`（= 每扇窗）一个进程内编号：看门狗账是全局表，靠它分格
/// （窗口关掉时按 owner 一次销账，见 `decode_watchdog::forget_owner`）。
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// 解码器句柄（UI 线程侧）。廉价可克隆，放进 `Rc` 共享。
pub struct Decoder {
    queue: Arc<Queue>,
    /// 本句柄（= 本窗口）在看门狗账上的编号。
    owner: u64,
    next_seq: Cell<u64>,
    /// 最新主画面 `Main` 请求的 seq。decode_loop 见到 `Main`/`Prefetch` 且
    /// seq 小于此值即作废（早退发 Err Done），避免过期请求排全量解码队。
    /// UI 线程单侧写（Relaxed 足够——只要求"发送顺序上先落 floor 再发 req"）。
    main_floor: Arc<AtomicU64>,
}

impl Drop for Decoder {
    /// 句柄没了 = 不会再有人发请求，让解码线程从 `wait` 上醒来自收（否则它永远
    /// 挂着，线程与队列成进程内的僵尸）。**未开始的那几条一并丢掉**：窗口都关了，
    /// 解完也没人收，而同格式的其它窗口还要陪它们排队（m230 S3 实测）。
    fn drop(&mut self) {
        let dropped = self.queue.discard_pending();
        // 还挂在账上的只剩**正在解的那一笔**：它的结果发给一个已经关掉的通道，没人
        // 收得着 `note_done`，所以按窗口销账（m232）。不摘它，看门狗 10 s 后就为一张
        // "窗口都关了的图"报警——纯噪声最坏的后果是教人学会忽略真告警。
        let forgotten = crate::core::decode_watchdog::forget_owner(self.owner);
        log::info!(
            "[queue-discard] owner={} dropped={} forgotten={} rest={}",
            self.owner,
            dropped,
            forgotten,
            crate::core::decode_watchdog::pending_count()
        );
    }
}

impl Decoder {
    /// 起一个解码线程，结果从 `done_tx` 被 UI 线程的定时器取走。
    pub fn spawn_with(done_tx: StdSender<Done>) -> Self {
        Self::spawn_with_sink(done_tx, None)
    }

    /// 同 [`spawn_with`](Self::spawn_with)，但带上这条线程的回程（Meta 通道 + 唤行动作）。
    ///
    /// 每个看图窗口一条线程、一份回程（★ 多窗口，2026-11-04）。不带 sink 的旧入口照旧
    /// 可用：Meta 与唤醒落到进程级兜底槽，行为与单窗口时代一致。
    pub fn spawn_with_sink(done_tx: StdSender<Done>, sink: Option<DecodeSink>) -> Self {
        let queue = Arc::new(Queue::new());
        let main_floor = Arc::new(AtomicU64::new(0));
        let floor_for_thread = main_floor.clone();
        let q_for_thread = queue.clone();
        thread::Builder::new()
            .name("sv-decode".into())
            .spawn(move || decode_loop(q_for_thread, done_tx, floor_for_thread, sink))
            // 红线豁免: 起线程失败 = 系统资源耗尽（OOM 级），没有可降级的第二种活法——
            // 发送端还在只会让请求无限排队、图永远出不来，那才是真的装死；就地失败反而
            // 留下崩溃报告。这是 §八.1 的第三类例外，必须像这样写在违规行上方点名。
            .unwrap_or_else(|e| panic!("启动解码线程失败：{e}"));
        Self {
            queue,
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            next_seq: Cell::new(1),
            main_floor,
        }
    }

    /// 发起一次解码，返回它的世代号（用于丢弃过期结果）。
    ///
    /// 这是**缩略图**用的默认入口（`kind = Thumb`，永不被取消）。
    /// 主画面/预读走 `request_frame` / `request_raw` / `request_region`，
    /// 它们收 [`ReqKind`] 参数。
    pub fn request(&self, path: &Path, max_w: u32, max_h: u32) -> u64 {
        self.send(Req {
            seq: self.alloc_seq(),
            path: path.to_path_buf(),
            max_w,
            max_h,
            region: None,
            frame_index: 0,
            raw: None,
            kind: ReqKind::Thumb,
            allow_scaled: false,
        })
    }

    /// 发起一次动画帧解码（第 `frame_index` 帧）。见 [`crate::core::pixels::Anim`]。
    pub fn request_frame(
        &self,
        path: &Path,
        max_w: u32,
        max_h: u32,
        frame_index: u32,
        kind: ReqKind,
    ) -> u64 {
        self.send(Req {
            seq: self.alloc_seq(),
            path: path.to_path_buf(),
            max_w,
            max_h,
            region: None,
            frame_index,
            raw: None,
            kind,
            allow_scaled: false,
        })
    }

    /// 同 [`Self::request_frame`]，但点亮**原生降采样**（契约 `REQ_ALLOW_SCALED`）。
    ///
    /// 主画面首屏专用：target 是视口那一框，解码器（JPEG 走 DCT 1/2\|1/4\|1/8）直接解出
    /// 够用的档位，省掉"整解一遍再缩到视口"里那次完整整解。清晰度不受损——
    /// [`Decoded::natural`] 仍是**原图**尺寸，于是 `viewer::tick` 照旧判定"未到整图"
    /// 并补一次原图档升级，放大到 1:1 依旧是原图（m35）。
    pub fn request_frame_scaled(
        &self,
        path: &Path,
        max_w: u32,
        max_h: u32,
        frame_index: u32,
        kind: ReqKind,
    ) -> u64 {
        self.send(Req {
            seq: self.alloc_seq(),
            path: path.to_path_buf(),
            max_w,
            max_h,
            region: None,
            frame_index,
            raw: None,
            kind,
            allow_scaled: true,
        })
    }

    /// 发起一次 SVG 可视区域解码。见 [`RegionReq`]。
    pub fn request_region(&self, path: &Path, region: RegionReq, kind: ReqKind) -> u64 {
        self.send(Req {
            seq: self.alloc_seq(),
            path: path.to_path_buf(),
            max_w: 0,
            max_h: 0,
            region: Some(region),
            frame_index: 0,
            raw: None,
            kind,
            allow_scaled: false,
        })
    }

    /// 发起一次裸数据解码：参数此刻从会话 store 快照（参数窗确认后先
    /// `set_raw_params` 再调这里）。路径还没参数时 `raw = None`，解码线程
    /// 会给引导性错误。
    pub fn request_raw(&self, path: &Path, max_w: u32, max_h: u32, kind: ReqKind) -> u64 {
        self.send(Req {
            seq: self.alloc_seq(),
            path: path.to_path_buf(),
            max_w,
            max_h,
            region: None,
            frame_index: 0,
            raw: get_raw_params(path),
            kind,
            allow_scaled: false,
        })
    }

    /// 作废所有在途的 Main / Prefetch，并预留一个不会被任何后续请求使用的 seq。
    ///
    /// 用于「打开失败 / 目录被清空」：界面已经放弃这些回包，不能让晚到的旧图重新上屏。
    /// 预留 seq 只占号、不发请求；`viewer::apply` 拿它当 `want_seq`，旧回包一律判过期。
    pub fn cancel_pending(&self) -> u64 {
        let seq = self.alloc_seq();
        self.main_floor.fetch_max(seq, Ordering::Relaxed);
        seq
    }

    fn alloc_seq(&self) -> u64 {
        let seq = self.next_seq.get();
        self.next_seq.set(seq + 1);
        seq
    }

    fn send(&self, req: Req) -> u64 {
        let seq = req.seq;
        let path = req.path.clone();
        let kind = req.kind;
        // Main 请求先落 floor 再入队——反过来 decode_loop 可能先弹旧 req 时
        // floor 尚未更新，白解一张。Thumb/Prefetch 不动 floor：前者与主画面
        // 解耦，后者只是"给下一次翻页让路"，让位由 Main 推的 floor 决定。
        if req.kind == ReqKind::Main {
            self.main_floor.fetch_max(seq, Ordering::Relaxed);
        }
        // 入队失败只可能是队列已关（`Decoder` 正在释放）：那时发出去也没人收。
        if !self.queue.push(req) {
            log::warn!("解码线程已退出，请求被丢弃：{}", path.display());
        } else {
            // 看门狗登记（P1.1 C1）：这笔在途要等一个回包，回包不到就是"卡住了"的
            // 现场证据。销账在 `drain_decode`（回包的唯一消费口）。
            crate::core::decode_watchdog::note_sent(seq, kind, &path, self.owner);
        }
        seq
    }
}

// ── 解码后端与 SVG 渲染器的注入点（依赖倒置，core 不见解码器/windui）────

/// 位图解码后端（消费方定义接口、`crate::decoders::Host` 实现、启动时经 [`install_backend`] 装入）。
///
/// safe-mode 门、裸数据参数路由、"无人认领"的可行动提示都在实现侧——
/// 那是解码器策略，不是调度逻辑。
///
/// 今天有**三条**路由：常规 `iv_render`（整解）、常规 `iv_render` + `REQ_ALLOW_SCALED`
/// （原生降采样，只有声明了 `CAP_SCALED` 的解码器会真做）、参数化裸数据 `iv_render_raw`。
///
/// 第三条是 2026-10-18 才接上的（本仓历史记录 §缩略图档实测 1.50–1.70×）。
/// 它的开关是**每次请求的意图**（`allow_scaled`），不是 `max_w/max_h`：主画面也传远小于
/// 原生的视口尺寸，那里要的是清晰。能力位图（`iv_caps`）只回答"会不会"，这一位回答
/// "这次要不要"——两者缺一，解码器都不会动手缩放。
pub trait DecodeBackend: Send {
    /// 解一张位图（svg/svgz 不会走到这里：core 自己路由到 [`SvgDecodeFn`]）。
    ///
    /// `allow_scaled` = 这次请求接受**比 target 更小**的原生缩放结果（回填的 `natural`
    /// 随之变小，缩小补齐仍由宿主做）。见 [`ReqKind::wants_scaled`]。
    // 参数平铺是刻意的：与 ABI 请求面一一对应，包一层聚合结构只会让每个实现多一次解包。
    #[allow(clippy::too_many_arguments)]
    fn decode(
        &mut self,
        path: &Path,
        ext: &str,
        max_w: u32,
        max_h: u32,
        frame_index: u32,
        raw: Option<RawParams>,
        allow_scaled: bool,
    ) -> Result<Decoded, String>;

    /// 本次请求是**后台预取**还是用户正等的那一笔（m237）。
    ///
    /// 只影响一件事：跨窗共用同一枚解码器 DLL 时，排队拿调用权站哪一档（见
    /// `decoders::dll_lock::Class`）。预取必须让位——否则"没人等的下一张"会挡在
    /// "用户正等的那张"前面，实测能多等一整笔（26.7 MP ≈ 0.95 s）。
    ///
    /// 默认实现什么都不做：这条只有解码器层（有跨窗共享 DLL 的那层）需要关心。
    fn set_background(&mut self, _on: bool) {}

    /// 区域渲染（**ABI v6**）：把文档的**指定区域**按目标密度直接光栅化。
    ///
    /// 这是"矢量图放大到 4000% 也无损"的通道：画布恒为一屏大小，倍率再高也
    /// 撞不到像素硬顶；线条是矢量重画，不是位图插值放大。
    ///
    /// `None` = **没有解码器支持这条通道**（没实现 `iv_render_region`）——宿主据此
    /// 落回整幅路线（与今天同档，不算错）。返回 `Some(Err(..))` = 支持但这次失败。
    fn try_decode_region(
        &mut self,
        path: &Path,
        ext: &str,
        region: &RegionReq,
    ) -> Option<Result<Decoded, String>> {
        let _ = (path, ext, region);
        None
    }
}

/// SVG 渲染函数：签名固定，实现在 `ui::svg`（windui resvg），启动时注入。
///
/// ★ 这条槽**只服务内置 svg/svgz**：解码器认领的矢量格式一律由**解码器自己**渲染
/// （宪章 §5.2），宿主不再接受"解码器交出矢量文档、宿主用 resvg 渲染"那条路。
pub type SvgDecodeFn = fn(&Path, Option<&RegionReq>, u32, u32) -> Result<Decoded, String>;

static BACKEND: std::sync::OnceLock<std::sync::Mutex<Box<dyn DecodeBackend>>> =
    std::sync::OnceLock::new();
static SVG: std::sync::OnceLock<SvgDecodeFn> = std::sync::OnceLock::new();

/// 后端工厂：每个解码 worker 自建一份（2026-11-04 挂死恢复）。
///
/// **为什么必须有它**：`BACKEND` 那把全局 Mutex 的 guard 覆盖 probe + 查询 + 渲染
/// 整段 FFI。一旦某个解码器在 `iv_render` 里死循环，那条线程就**永久持有它**，之后
/// 任何 `with_backend`（包括重启出来的新 worker）全部堵死——这就是"打开一张图卡住，
/// 之后所有图都打不开"的机制（`decode_watchdog` 模块头的 §2 现场，2026-10-28 已定论）。
/// 让每个 worker 用自己那份后端，挂死者钉死的就只是它自己那一份。
pub type BackendFactory = fn() -> Box<dyn DecodeBackend>;

static BACKEND_FACTORY: std::sync::OnceLock<BackendFactory> = std::sync::OnceLock::new();

thread_local! {
    /// 本线程的私有后端（只有解码 worker 装它）。`None` = 走进程级单例那条老路。
    static LOCAL_BACKEND: std::cell::RefCell<Option<Box<dyn DecodeBackend>>> =
        const { std::cell::RefCell::new(None) };
}

/// 装入位图解码后端（幂等）。入口：`install_decode_stack`。
pub fn install_backend(b: Box<dyn DecodeBackend>) {
    let _ = BACKEND.set(std::sync::Mutex::new(b));
}

/// 装入后端工厂（幂等）。`install_decode_stack` 与 [`install_backend`] 一起调。
///
/// 工厂只在**解码 worker 重建时**被调用（正常启动一次、每次挂死恢复一次），所以它内部
/// 做一点点事（扫目录 / `LoadLibrary`）是可以接受的。
pub fn install_backend_factory(f: BackendFactory) {
    let _ = BACKEND_FACTORY.set(f);
}

/// 给**当前线程**装一份私有后端（解码 worker 启动时调）。没有工厂时静默跳过——
/// 那种装配（单测 / 裸示例）本来就走进程级单例。
fn install_local_backend() {
    let Some(f) = BACKEND_FACTORY.get() else {
        return;
    };
    LOCAL_BACKEND.with(|b| *b.borrow_mut() = Some(f()));
}

/// 装入 SVG 渲染函数（幂等）。入口：`install_decode_stack`。
pub fn set_svg_decoder(f: SvgDecodeFn) {
    let _ = SVG.set(f);
}

/// 在后端上跑一次解码。未装入时返回可诊断的错误，不 panic——
/// 启动顺序错位只影响解码，不该把进程带崩。
fn with_backend<T>(f: impl FnOnce(&mut dyn DecodeBackend) -> T) -> Result<T, String> {
    // ★ 解码 worker 线程：用**自己那份**后端（见 [`BackendFactory`]）。
    if LOCAL_BACKEND.with(|b| b.borrow().is_some()) {
        return LOCAL_BACKEND.with(|b| match b.borrow_mut().as_mut() {
            Some(backend) => Ok(f(&mut **backend)),
            None => Err("解码后端未安装（启动装配遗漏 install_decode_stack）".into()),
        });
    }
    let Some(m) = BACKEND.get() else {
        return Err("解码后端未安装（启动装配遗漏 install_decode_stack）".into());
    };
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    Ok(f(&mut **g))
}

// ── 挂死恢复：单笔超时 + 熔断（2026-11-04，用户口径「永久解决」）───────
//
// 现场（`.build/temp/rupix-2026-11-04.log`，一天 301 条 `[decode-watchdog]`）：
// `noise.avifs` 让解码线程 20 秒不回包，之后 `noise.bmp` / `noise.cr2` 的请求全排在
// 后面——用户看到的是"打开新图片卡住了"，而 UI 本身没死（定时器还在跑）。
//
// `decode_watchdog` 只把挂死变成可读的日志，**救不活线程**。这里补上恢复：
//
// 1. **单笔超时**：监督者（`decode_loop`）把请求交给 worker 线程，`recv_timeout` 等；
//    到点就放弃这一笔，给 UI 回一条可读的 Err（画面出错误文案，而不是永远"载入中…"）。
// 2. **熔断**：超时那一刻正在用的解码器 id（由 `decoders::decode_with` 上报）记进
//    进程级挂死集，**本会话任何路径都不再调它**——僵尸线程还在那个 DLL 里转，
//    再进去一次就是 ABI 明令禁止的重入（「可重入但不必 MT-safe」）。
// 3. **换 worker**：丢弃旧 worker（线程泄漏一条，见下）并起新的；新 worker 是新的
//    后端实例（[`BackendFactory`]），不会被僵尸持有的锁堵住。
//
// 遗留代价（如实记）：每次挂死泄漏一条线程 + 它那份后端 + 一份 mmap，直到进程退出。
// `ExitProcess` 先杀线程再跑 DETACH，只有当僵尸恰好在 loader 锁里才可能拖慢退出——
// 这个风险在引入本机制之前就已经存在（挂死时线程同样挂在 DLL 里）。
//
// 阈值为什么 20 秒：`decode_watchdog` 的首报是 10 秒（它只记录），这里再等一倍——
// 误杀一个"合理慢"的解码（网络盘 mmap 页入）比多等 10 秒更糟。用户可改
// ini `decode_timeout_s`（CONFIG_ONLY_KEYS，见 `core::config`）。

static DECODE_TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(20_000);

/// 正在解码的那个解码器 id（由 `decoders::decode_with` 上报）。
///
/// 为什么用全局而不是 thread-local：读它的**不是**解码线程，而是超时后的监督者
/// （worker 已经卡死，问不了它）。写入频率 = 每笔解码一次，代价是一次短锁。
static CURRENT_DECODER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 这一笔解码**走到哪一段了**（诊断坐标）。
///
/// 为什么需要它：`[decode-watchdog]` 只说"某个文件卡了 10 秒"，把它引向解码器——可
/// 「读文件 / 探测 / 补查尺寸 / 查询 / 渲染 / 后处理」里只有**渲染**是外来的 FFI，
/// 其余全是宿主的活。没有这一位，一个宿主的死循环和一个解码器的死循环长得一模一样，
/// 只能靠猜（2026-11-04 就是这么猜偏的）。
///
/// 写方是 worker（每段一次 `store`，Relaxed 足够——它只是"最后见到的那一段"），
/// 读方是**任何**线程：`decode_watchdog` 的巡检、监督者超时后的那条日志。
static PHASE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(PHASE_IDLE);

/// 没在解码（空闲 / 刚起步）。
pub const PHASE_IDLE: u8 = 0;
/// 打开文件、判空、按登记表路由。
pub const PHASE_OPEN: u8 = 1;
/// 读文件（mmap 或整读）。
pub const PHASE_READ: u8 = 2;
/// `iv_probe`（认不认这个文件）。
pub const PHASE_PROBE: u8 = 3;
/// 补查原图尺寸（`iv_render` 查询）。
pub const PHASE_NATURAL: u8 = 4;
/// 补查文档自然尺寸（零目标查询，矢量用）。
pub const PHASE_DOC: u8 = 5;
/// 正式查询（`iv_render` + `out=NULL`）。
pub const PHASE_QUERY: u8 = 6;
/// 正式渲染（`iv_render` + 像素缓冲）——**唯一的外来代码段**。
pub const PHASE_RENDER: u8 = 7;
/// 后处理（RGBA 归一 / 转正 / 缩放 / ICC）——纯宿主代码。
pub const PHASE_POST: u8 = 8;
/// 区域重渲（`iv_render_region`，矢量放大走这条）。
pub const PHASE_REGION: u8 = 9;
/// 内置矢量库（resvg）——宿主自己的代码，不是 DLL。
pub const PHASE_SVG: u8 = 10;

/// 记下"这一笔走到哪了"。只在 worker 里调；`Relaxed` 足够（见 [`PHASE`]）。
pub(crate) fn set_phase(p: u8) {
    PHASE.store(p, std::sync::atomic::Ordering::Relaxed);
}

/// 当前阶段的人话名字（日志用）。
pub fn phase_name(p: u8) -> &'static str {
    match p {
        PHASE_OPEN => "open",
        PHASE_READ => "read",
        PHASE_PROBE => "probe",
        PHASE_NATURAL => "natural",
        PHASE_DOC => "doc-natural",
        PHASE_QUERY => "query",
        PHASE_RENDER => "render",
        PHASE_POST => "post",
        PHASE_REGION => "region",
        PHASE_SVG => "svg",
        _ => "idle",
    }
}

/// 读出当前阶段（人话）。挂死现场那一条日志用它。
pub fn current_phase() -> &'static str {
    phase_name(PHASE.load(std::sync::atomic::Ordering::Relaxed))
}

/// 本会话已熔断的解码器 id（**判据**：`is_hung_decoder` 读它，整个会话有效）。
static HUNG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 已经交给 UI 的熔断 id（`take_hung_decoders` 去重用）。
///
/// 为什么不直接把 `HUNG` 取空：那会让 `is_hung_decoder` 在 UI 取走的那一刻失效，
/// 而 UI 补进 `decoders::add_disabled` 是**另一条**路径（见 runtime）——中间那几微秒
/// 足够让一个新请求挤进去。两道保险就该各管各的，别互相拆台。
static REPORTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 设定单笔解码超时（秒）。`runtime` 启动时按 ini 灌入，0 = 不超时（关闭恢复机制）。
pub fn set_decode_timeout(secs: u64) {
    DECODE_TIMEOUT_MS.store(
        secs.saturating_mul(1000),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// 当前单笔解码超时。`None` = 关掉了恢复机制（无限等）。
///
/// 解码器 DLL 那把"同一枚串行"的锁也读它（`decoders::dll_lock`）：宿主对"一次解码最多等多久"
/// 只该有一个定义，两处各写一个数就是第二正本。
pub fn decode_timeout() -> Option<Duration> {
    match DECODE_TIMEOUT_MS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => None,
        ms => Some(Duration::from_millis(ms)),
    }
}

/// 上报"下一步要进哪个解码器"。由 `decoders::decode_with` 在拿到 id 后立刻调。
pub(crate) fn report_current_decoder(id: &str) {
    let mut g = CURRENT_DECODER.lock().unwrap_or_else(|e| e.into_inner());
    *g = Some(id.to_string());
}

/// 当前解码器 id 的人读形式（诊断日志用；`current_decoder` 是它的私有底座）。
pub fn current_decoder_name() -> Option<String> {
    current_decoder()
}

fn current_decoder() -> Option<String> {
    CURRENT_DECODER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 该解码器是否已在本次会话被熔断（挂死过）。`decoders::decode_with` 用它挡在调用之前。
pub fn is_hung_decoder(id: &str) -> bool {
    HUNG.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|x| x == id)
}

/// 取走**尚未上报**的熔断 id（`runtime` 取走 → 写进 `decoders_disabled` 落盘 + 提示）。
///
/// 取走之后 `is_hung_decoder` 仍然为真（那是本会话的判据，不是待办队列）。
pub fn take_hung_decoders() -> Vec<String> {
    let hung = HUNG.lock().unwrap_or_else(|e| e.into_inner());
    let mut rep = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
    let fresh: Vec<String> = hung
        .iter()
        .filter(|id| !rep.iter().any(|x| x == *id))
        .cloned()
        .collect();
    for id in &fresh {
        rep.push(id.clone());
    }
    fresh
}

/// 清空熔断集与上报记录（单测用）。
pub fn clear_hung_decoders() {
    HUNG.lock().unwrap_or_else(|e| e.into_inner()).clear();
    REPORTED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn note_hung(id: &str) {
    let mut g = HUNG.lock().unwrap_or_else(|e| e.into_inner());
    if !g.iter().any(|x| x == id) {
        g.push(id.to_string());
        log::warn!("[decode-hang] id={id} verdict=quarantined");
    }
}

/// 一笔解码作业（worker → 监督者）。
struct JobDone {
    out: Result<Decoded, String>,
}

/// 解码 worker：一条长驻线程 + 一份**私有后端**（[`BackendFactory`]）。
///
/// 为什么它必须可丢弃：挂死在 FFI 里的线程**没有可靠的取消原语**（Rust 没有；
/// `TerminateThread` 会漏锁、留下半初始化的堆与 DLL 状态）。能做的只有"不再等它"——
/// 超时后丢掉它的句柄（线程泄漏一条栈），换一条新线程继续服务。
struct Worker {
    job_tx: StdSender<Req>,
    res_rx: Receiver<JobDone>,
}

impl Worker {
    fn spawn(meta: Option<StdSender<MetaEvent>>) -> Self {
        let (job_tx, job_rx) = channel::<Req>();
        let (res_tx, res_rx) = channel::<JobDone>();
        // 句柄直接丢掉 = detach：超时之后我们**不再等**这条线程（也没有可靠的取消
        // 原语），它的栈与后端随进程退出才回收。见类型文档的遗留代价。
        let _ = thread::Builder::new()
            .name("sv-decode-w".into())
            // 红线豁免: 起线程失败 = 资源耗尽（OOM 级），没有第二种活法——不解码只会让
            // 画面永远停在"载入中"，就地失败反而留下崩溃报告（与 `spawn_with` 同一条）。
            .spawn(move || worker_main(job_rx, res_tx, meta))
            .unwrap_or_else(|e| panic!("启动解码 worker 失败：{e}"));
        Self { job_tx, res_rx }
    }

    /// 跑一笔。`None` = 超时，或 worker 已经没了（调用方负责熔断与换 worker）。
    fn run(&self, req: &Req, timeout: Option<Duration>) -> Option<Result<Decoded, String>> {
        self.job_tx.send(req.clone()).ok()?;
        let got = match timeout {
            Some(t) => self.res_rx.recv_timeout(t),
            // 关掉恢复机制（`decode_timeout_s=0`）：老行为，无限等。
            None => self
                .res_rx
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(j) => Some(j.out),
            Err(_) => None,
        }
    }
}

/// worker 线程主体：装回程与私有后端，然后一笔一笔解。
fn worker_main(
    job_rx: Receiver<Req>,
    res_tx: StdSender<JobDone>,
    meta: Option<StdSender<MetaEvent>>,
) {
    META_SINK.with(|m| *m.borrow_mut() = meta);
    // 私有后端：挂死时随这条线程一起废掉，不牵连同进程的其它 worker
    //（进程级单例那把锁会被僵尸永久持有，这是本机制存在的全部理由）。
    install_local_backend();
    while let Ok(req) = job_rx.recv() {
        set_current_req(req.seq, &req.path);
        let out = decode_one(&req);
        // 一笔结束就归零：否则"上一笔卡在 render"会一直挂在字段上，下一次挂死
        // 读到的阶段就是假的（诊断字段最怕的错不是没有，而是**过期**）。
        set_phase(PHASE_IDLE);
        clear_current_req();
        if res_tx.send(JobDone { out }).is_err() {
            break;
        }
    }
}

/// 超时那一笔给用户的说法：必须点明"哪个解码器被停用、怎么恢复"。
fn hang_error(req: &Req, id: Option<&str>, timeout: Option<Duration>) -> String {
    let name = req
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| req.path.display().to_string());
    let secs = timeout.map(|t| t.as_secs()).unwrap_or(0);
    match id {
        Some(id) => format!(
            "「{name}」解码超时（{secs} 秒没有返回）：已停用解码器「{id}」（设置 → 解码器 里可重新启用），\
             其余图片不受影响"
        ),
        None => format!("「{name}」解码超时（{secs} 秒没有返回），已跳过这次解码"),
    }
}

/// 解码线程主体：取出请求 → 判过期 → 解码 → 回包，直到队列关闭或 UI 侧收包端没了。
///
/// 出队即**主画面插队**（见 [`Queue::pop`]）。为什么必须插队：这条线程一次只解一张，
/// 而队里常态躺着缩略图（在途 ≤ `thumbs::MAX_INFLIGHT`=3）和刚装完图顺手发的预读
/// （在途 ≤1，见 `viewer::request_prefetch` 的 `prefetch_seq`）——按到达顺序它们都排在
/// 用户刚按下去
/// 的那次缩放重解**前面**。实测 100 MB 级样本、2 张缩略图在前：主图重解 0.37s → 1.98s，
/// 放大后要盯着糊图看两秒；目录里全是大图时更久。插队后同场景实测 0.25s。
fn decode_loop(
    q: Arc<Queue>,
    done_tx: StdSender<Done>,
    main_floor: Arc<AtomicU64>,
    sink: Option<DecodeSink>,
) {
    // 本线程的回程（Meta 通道 + 主画面唤醒）。装在线程局部而不是每次调用都查一遍
    // 全局注册表：这条线程只服务一个窗口，回程在它活着期间不变（★ 多窗口，2026-11-04）。
    // Meta 那一半要转交给 worker（它才是真正在解的那条线程）。
    let meta_tx = sink.as_ref().map(|s| s.meta.clone());
    SINK.with(|s| *s.borrow_mut() = sink);
    // 监督者**自己不解码**：所有 FFI 都在 worker 里跑，这样"挂死"才有地方可逃
    //（见 `Worker` 与模块内「挂死恢复」小节的说明）。
    let timeout = decode_timeout();
    let mut worker = Worker::spawn(meta_tx.clone());
    // 取证开关与 `decoders` 侧的 `[decode-trace]` 同名同闸（`RUPIX_DECODE_TRACE=1`）。
    // 为什么单加这一行：`[decode-trace]` 只说"解了一次"，说不出**是谁发起的**——
    // 主画面 / 预读 / 缩略图三条路混在同一个文件名的多条读数里，翻页命中率这类
    // 问题就只能靠猜（m58 实测时撞上的）。带上 `kind` + 请求档位 + 作废位才可判读。
    // 字段一律 `key=value`（m59）：分隔符是 `=` 与空格而不是中文词，一次性脚本才
    // 不用先跟 GBK 控制台打一架。
    let trace = std::env::var_os("RUPIX_DECODE_TRACE").is_some();
    while let Some(req) = q.pop() {
        // 过期早退：`Main`/`Prefetch` 类请求的 seq 已被更新的 Main 追过——不再
        // 解像素，直接发一个 Err Done 让 UI 记账清位（`prefetch_seq` / 过期丢弃）。
        // `Thumb` 不参与（与主画面解耦，缩略图队列不会被翻页作废）。
        //
        // 判据用 `<` 不是 `<=`：`Main` 请求发送前已 fetch_max 把自己 seq 落进
        // floor，`req.seq == floor` 是当前主画面，必须真解。
        let cancelled = matches!(req.kind, ReqKind::Main | ReqKind::Prefetch)
            && req.seq < main_floor.load(Ordering::Relaxed);
        // 从这一刻起才算"解码耗时"（在排队的那一段另计）：看门狗日志靠这一位把
        // "堵在队列"和"堵在 FFI"分开，混在一起就只会得出"七个格式同时挂死"。
        crate::core::decode_watchdog::note_picked(req.seq);
        if trace {
            log::debug!(
                "[decode-req] kind={:?} file={} tier={}x{} scaled={} seq={} verdict={}",
                req.kind,
                req.path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                req.max_w,
                req.max_h,
                req.allow_scaled,
                req.seq,
                if cancelled { "作废" } else { "解" }
            );
        }
        let out = if cancelled {
            // Done 用 Err 空串占位——`viewer::apply` 的过期分支只看 seq，不进错误
            // 弹窗；预读分支见 Err 不 cache，只清 prefetch_seq。空串确保万一路径
            // 撞进错误显示也不出乱码。
            Err(String::new())
        } else {
            match worker.run(&req, timeout) {
                Some(out) => out,
                None => {
                    // 超时（或 worker 已经没了）：**熔断 → 弃置 → 换新 → 给用户人话**。
                    //
                    // 顺序有讲究：熔断必须发生在起新 worker **之前**吗？不必——新 worker
                    // 是新的后端实例，它进不进那个 DLL 由 `decode_with` 的
                    // `is_hung_decoder` 挡（本会话立即生效）；先记熔断再换线程只是让日志
                    // 的因果顺序好读。
                    let hung = current_decoder();
                    if let Some(id) = hung.as_deref() {
                        note_hung(id);
                    }
                    log::warn!(
                        "[decode-timeout] file={} kind={:?} id={} secs={} phase={} verdict=restart-worker",
                        req.path.display(),
                        req.kind,
                        hung.as_deref().unwrap_or("-"),
                        timeout.map(|t| t.as_secs()).unwrap_or(0),
                        current_phase()
                    );
                    worker = Worker::spawn(meta_tx.clone());
                    Err(hang_error(&req, hung.as_deref(), timeout))
                }
            }
        };
        // 先记类别再 move 走 `req`：唤醒判据在 send 之后还要用。
        let kind = req.kind;
        let done = Done {
            seq: req.seq,
            path: req.path,
            out,
        };
        if done_tx.send(done).is_err() {
            // UI 线程没了（窗口关闭），正常退出。
            break;
        }
        // 只有**当前主画面的有效回包**叫醒 UI：那是用户正等的那一张。缩略图/预读
        // 是后台填充，作废的早退回包不改变画面——叫醒它们只会让静止的窗口白出帧。
        if kind == ReqKind::Main && !cancelled {
            wake_for_main();
        }
    }
}

/// 单张解码：**永不 panic 出线程**。
///
/// 坏图把解码线程打挂一次的后果是"之后所有图片都打不开"，比"这一张打不开"严重得多。
/// 故每张都套 `catch_unwind`（发布配置刻意保留了 `unwind`，见 `Cargo.toml` 的注释）。
///
/// 路由只有两条：**内置库按登记表认领 → 内置实现；其余 → 后端**。
/// 今天内置库里只有 resvg（svg/svgz 两枚矢量后缀）。
/// ★ **位图一条都不内置**（2026-10-25 用户裁决，`platform::wic` 已整层删除）：
///   曾经还有一枚"只走点名"的 WIC，它调的是系统解码器——便携看图器不干这事，
///   同一张图在两台机器上必须解出同一个结果（宪章 §5.1）。
///   （2026-10-10 删的是**WIC 兜底**，2026-10-20 它以"可被点名的内置库"回归、
///   2026-10-25 又整层删掉——三段历史都不改"兜底永远不做"这条底线。）
fn decode_one(req: &Req) -> Result<Decoded, String> {
    set_phase(PHASE_OPEN);
    // 排队优先级（m237）：预取让位给用户正等的那一笔——跨窗共用同一枚 DLL 时，
    // "没人等的下一张"不该挡在前面。后端实例是长驻的，所以**每次请求都要设**。
    // 拿不到后端（未装配 / 正在重启）就算了：后面那次调用自己会报错。
    let _ = with_backend(|b| b.set_background(req.kind == ReqKind::Prefetch));
    // 0 字节没有可解码的字节：在路由之前直说。交给后端的话，解码器探测只会答
    // 「不认识这个文件」——把"文件是空的"说成"格式不认"，排查方向就反了。
    if std::fs::metadata(&req.path).is_ok_and(|m| m.len() == 0) {
        return Err("文件是空的（0 字节）".into());
    }
    let ext = req
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    // 这里**没有**磁盘解码缓存那一层了（2026-10-19 撤，正本 本仓历史记录 与
    // `HISTORY.md`）：它存的是未压缩 RGBA，一张顶格图 140–160 MB，实测写盘要 +214 ms
    // 而这笔时间直接加在用户等画面的那一段里；参照项目里做磁盘缓存的四家存的都是
    // 压缩档（WebP/QOI）且限尺寸。跨会话重开省的那 100–340 ms 不值这个形态。
    // 会话内的加速由另外两层负责，都不落盘：`viewer` 的邻图预读（视口档，RAM）与
    // 换档本地派生（整图已在内存）。
    catch_unwind(AssertUnwindSafe(|| decode_route(req, &ext)))
        .unwrap_or_else(|_| Err("解码时发生内部错误（已拦截，不影响后续图片）".into()))
}

/// 真正的解码路由：内置库（按登记表认领）交内置实现，其余交后端。见 [`decode_one`] 的口径说明。
fn decode_route(req: &Req, ext: &str) -> Result<Decoded, String> {
    // 内置路**按登记表认领**，不再在这里硬编码后缀（m56）：清单真源是
    // [`crate::core::formats::BUILTIN_DECODERS`]，`gallery` 的后缀判定与设置页的候选
    // 都从同一张表派生，所以"清单里有、路由不接"这种半拉子状态结构上不可能出现。
    // `auto_claim=false` 的内置库在这里**隐身**——它们只走"用户点名"那条路，而那条路
    // 今天已经没有位图实现了（WIC 于 2026-10-25 删除），所以登记表里 `false` 的那枚
    // 若还认领着后缀，就是"登记表与路由脱节"，会在下面的 `other` 分支里报错而不是静默。
    if let Some(b) = crate::core::formats::builtin_auto_owner(ext) {
        // id → 注入槽。今天一枚（resvg），每枚内置库各有一个自己的槽与自己的签名，
        // 硬并成一种只会让实现侧解包。
        match b.id {
            "resvg" => {
                set_phase(PHASE_SVG);
                // 矢量要按**目标尺寸**光栅化才有无损放大，与"位图解码后缩放"本质不同，
                // 所以它带 region 而不是 frame_index/allow_scaled。
                let f = SVG.get().ok_or_else(|| {
                    format!("内置解码库「{}」未装入（启动装配遗漏 install_decode_stack）", b.id)
                })?;
                return f(&req.path, req.region.as_ref(), req.max_w, req.max_h);
            }
            other => {
                return Err(format!(
                    "内置解码库「{other}」在登记表里标了 auto_claim，但路由没有它的分支（登记表与路由脱节）"
                ))
            }
        }
    }

    // 区域渲染路线（**ABI v6**）：宿主带着"看得见的那一块"来要图时，解码器若实现了
    // `iv_render_region`，就走这条——画布恒为一屏大小，倍率再高也顶得住，线条是矢量
    // 重画（无损放大）。这是"放大到 4000% 也无损"的**唯一**通道。
    //
    // ★ 没有解码器支持时（`None`）**落回下面的整幅路线**：那不是错，只是"这张图
    //   没有区域通道"——表现与今天的位图放大同档（略糊），不会打不开。
    // ★ 这里绝不"按区域去要整幅渲染的解码器"：没有区域接口的解码器会把**整篇**缩进
    //   那一小块——那是**内容错误**，不是"糊一点"。判据在
    //   `core::formats::is_region_renderable`（viewer 据此决定要不要发区域请求）。
    if let Some(r) = req.region.as_ref() {
        if let Some(res) = with_backend(|b| b.try_decode_region(&req.path, ext, r))
            .ok()
            .flatten()
        {
            return res;
        }
    }

    // 位图全部交给后端（解码器宿主，以及被用户点名的内置库）。safe-mode 门、裸数据参数
    // 路由、"无人认领"的提示都在实现侧——那是解码器策略，不是调度逻辑。
    with_backend(|b| {
        b.decode(
            &req.path,
            ext,
            req.max_w,
            req.max_h,
            req.frame_index,
            req.raw,
            scaled_of(req),
        )
    })?
}

/// 探针：不经 UI 直接走一遍与打开图片**完全相同**的解码链路（SVG → 注入的渲染器；其余 → 后端）。
///
/// 存在的唯一理由是验收与排障——让"解码器到底接通没有"在没有 GUI 交互时也能被断言。
/// `max_w` / `max_h` 传 0 表示要原始尺寸。
///
/// 后端是进程级单例（[`install_backend`] 装一次全局复用）：连续调用（冒烟 / 基准）
/// 不重复扫目录与 LoadLibrary，与真实应用"Host 常驻"的实况一致。
///
/// ★ `raw` 取**会话 store**（[`get_raw_params`]）而不是恒 `None`：裸像素没有文件头，
/// 不给参数就解不开；空格预览是独立线程直接调本函数、不经过 `Viewer`，恒 `None`
/// 会让 `.uyvy` / `.rgb` 这类文件"双击能开、按空格没反应"（用户实测）。
/// 非裸数据这条永远是 `None`，对普通图片零影响。
pub fn probe_decode(path: &Path, max_w: u32, max_h: u32) -> Result<Decoded, String> {
    let req = Req {
        seq: 0,
        path: path.to_path_buf(),
        max_w,
        max_h,
        region: None,
        frame_index: 0,
        raw: get_raw_params(path),
        kind: ReqKind::Main,
        allow_scaled: false,
    };
    decode_one(&req)
}

/// 同 [`probe_decode`]，但点亮**原生降采样**（[`Decoder::request_frame_scaled`] 的
/// 同步版）。基准用：量"首屏档位解"与"整解"的真实差（m35）。
pub fn probe_scaled(path: &Path, max_w: u32, max_h: u32) -> Result<Decoded, String> {
    let req = Req {
        seq: 0,
        path: path.to_path_buf(),
        max_w,
        max_h,
        region: None,
        frame_index: 0,
        raw: None,
        kind: ReqKind::Main,
        allow_scaled: true,
    };
    decode_one(&req)
}

/// 缩略图口径的探针：与 [`probe_decode`] 走**同一条生产链路**，但请求语义换成
/// [`ReqKind::Thumb`]——带缩放意图，与真实缩略图请求逐字节同形。
///
/// 存在的理由只有一个：缩放腿的账必须在生产链路上量（读文件 + probe + 查询阶段 +
/// 解码 + `fit_to` 补齐），库层面的对比 本仓历史记录 §缩略图档已经做过。
pub fn probe_thumb(path: &Path, max_w: u32, max_h: u32) -> Result<Decoded, String> {
    let req = Req {
        seq: 0,
        path: path.to_path_buf(),
        max_w,
        max_h,
        region: None,
        frame_index: 0,
        raw: None,
        kind: ReqKind::Thumb,
        allow_scaled: false,
    };
    decode_one(&req)
}

/// 同 [`probe_decode`]，但解码动画的第 `frame_index` 帧（0 起）。
///
/// 仅用于动画基准测量（顺序播放 / 循环回绕）——真实 UI 走 `Decoder` 的
/// `request_frame` 异步链路，不依赖本函数。生产解码逻辑不受影响。
pub fn probe_decode_frame(
    path: &Path,
    max_w: u32,
    max_h: u32,
    frame_index: u32,
) -> Result<Decoded, String> {
    let req = Req {
        seq: 0,
        path: path.to_path_buf(),
        max_w,
        max_h,
        region: None,
        frame_index,
        raw: None,
        kind: ReqKind::Main,
        allow_scaled: false,
    };
    decode_one(&req)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    /// 测试专用：一条**不跑解码线程**的 `Decoder`，让别的模块能稳定读到入队的请求。
    /// 生产路径仍只有 `spawn_with` 一条真线程，这里只是同一个结构体的裸构造，
    /// 不存在第二套发送行为。
    pub(crate) fn stub_decoder() -> Decoder {
        Decoder {
            queue: Arc::new(Queue::new()),
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            next_seq: Cell::new(1),
            main_floor: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 队列快照（路径 / 裸数据参数 / 请求种类 / 目标框）：缩略图那侧靠它验「走的是哪条通道」，
    /// 而不是靠 `inflight.len()` 这种两种写法都绿的间接量。
    pub(crate) fn pending_reqs(
        d: &Decoder,
    ) -> Vec<(PathBuf, Option<RawParams>, ReqKind, u32, u32)> {
        d.queue
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|r| (r.path.clone(), r.raw, r.kind, r.max_w, r.max_h))
            .collect()
    }

    /// ★ 负向验证：有文件头的图不进裸数据通道，也不许被反推宽高。
    /// 挂错判据（把 `is_headerless_path` 写成恒真）时这条先红。
    #[test]
    fn ensure_raw_params_leaves_headered_files_alone() {
        let p = PathBuf::from("thumbs-gate/camera.nef");
        assert!(ensure_raw_params(&p).is_none(), "相机 RAW 不该走裸数据推断");
        assert!(get_raw_params(&p).is_none(), "更不许往会话 store 里塞参数");
        let jpg = PathBuf::from("thumbs-gate/plain.jpg");
        assert!(ensure_raw_params(&jpg).is_none());
    }

    /// 参数窗确认过的值优先于推断（用户改过的宽高不能被自动推断覆盖）。
    #[test]
    fn ensure_raw_params_keeps_the_users_value() {
        let p = PathBuf::from("thumbs-gate/user.bgra");
        let want = RawParams {
            width: 640,
            height: 480,
            layout: 8,
        };
        set_raw_params(&p, want);
        assert_eq!(ensure_raw_params(&p), Some(want));
    }

    #[test]
    fn rawpx_inference_uses_suffix_layout_and_length() {
        let p = infer_raw_params_for("bgra", 4096 * 66 * 4).expect("bgra");
        assert_eq!((p.width, p.height, p.layout), (4096, 66, RAW_LAYOUTS[6]));
        let p = infer_raw_params_for("i420", 4096 * 178 * 3 / 2).expect("i420");
        assert_eq!((p.width, p.height, p.layout), (4096, 178, RAW_LAYOUTS[9]));
        let p = infer_raw_params_for("bgra", 604 * 456 * 4).expect("4:3 bgra");
        assert_eq!((p.width, p.height), (604, 456));
        assert!(infer_raw_params_for("bgra", 4096 * 66 * 4 + 1).is_none());
        assert!(infer_raw_params_for("3fr", 4096 * 66 * 4).is_none());
    }

    #[test]
    fn camera_raw_is_not_headerless() {
        assert!(is_headerless_ext("bgra"));
        assert!(is_headerless_ext("YUV"));
        assert!(!is_headerless_ext("3fr"));
        assert!(!is_headerless_ext("raw"));
    }

    fn req(seq: u64, kind: ReqKind) -> Req {
        Req {
            seq,
            path: PathBuf::from(format!("p{seq}.png")),
            max_w: 100,
            max_h: 100,
            region: None,
            frame_index: 0,
            raw: None,
            kind,
            allow_scaled: false,
        }
    }

    // ── 挂死恢复（2026-11-04，用户口径「永久解决」）────────────────

    /// 没有回包的 worker：`run` 必须在超时后返回 `None`（= 放弃这一笔、换 worker），
    /// 而不是把监督者一起等死。这是"打开一张图卡住、之后全都卡住"的直接解药。
    #[test]
    fn worker_reports_timeout_when_nobody_replies() {
        // 收端直接丢掉 = 永远不会有回包（模拟挂死在 FFI 里的那条线程）。
        let (job_tx, _job_rx) = channel::<Req>();
        let (_res_tx, res_rx) = channel::<JobDone>();
        let w = Worker { job_tx, res_rx };
        let t0 = std::time::Instant::now();
        assert!(
            w.run(&req(1, ReqKind::Main), Some(Duration::from_millis(20)))
                .is_none(),
            "没有回包就该判超时"
        );
        assert!(
            t0.elapsed() >= Duration::from_millis(15),
            "必须真的等满超时，而不是立刻放弃"
        );
    }

    /// 正常路径：worker 回了包就原样交出去（超时机制不能把好路径也判死）。
    #[test]
    fn worker_returns_the_reply() {
        let (job_tx, job_rx) = channel::<Req>();
        let (res_tx, res_rx) = channel::<JobDone>();
        let h = thread::spawn(move || {
            let _ = job_rx.recv();
            let _ = res_tx.send(JobDone {
                out: Err("boom".into()),
            });
        });
        let w = Worker { job_tx, res_rx };
        let got = w.run(&req(2, ReqKind::Main), Some(Duration::from_secs(1)));
        assert!(matches!(got, Some(Err(m)) if m == "boom"));
        let _ = h.join();
    }

    /// 熔断集：重复熔断只记一条；上报一次后不再重复上报；**取走之后判据仍然有效**
    /// （否则 UI 取走那一瞬间会漏进一个新请求）。
    #[test]
    fn hung_set_quarantines_for_the_whole_session() {
        clear_hung_decoders();
        assert!(!is_hung_decoder("modern"));
        note_hung("modern");
        note_hung("modern");
        assert!(is_hung_decoder("modern"));
        assert_eq!(take_hung_decoders(), vec!["modern".to_string()]);
        assert!(
            is_hung_decoder("modern"),
            "取走只是「已上报」，判据必须留到会话结束"
        );
        assert!(take_hung_decoders().is_empty(), "同一笔不该上报两次");
        clear_hung_decoders();
        assert!(!is_hung_decoder("modern"));
    }

    /// 超时秒数：0 = 关掉恢复机制（无限等，老行为）。
    #[test]
    fn zero_timeout_disables_recovery() {
        set_decode_timeout(20);
        assert_eq!(decode_timeout(), Some(Duration::from_secs(20)));
        set_decode_timeout(0);
        assert!(decode_timeout().is_none());
        // 复原全局量，别留给后面的用例。
        set_decode_timeout(20);
    }

    /// 超时文案必须点名"哪个解码器被停用"并给出恢复入口——用户看不到这句就只能猜。
    #[test]
    fn hang_error_names_the_decoder_and_the_way_back() {
        let msg = hang_error(
            &req(3, ReqKind::Main),
            Some("modern"),
            Some(Duration::from_secs(20)),
        );
        assert!(msg.contains("modern"), "要点名解码器：{msg}");
        assert!(msg.contains("20 秒"), "要说清等了多久：{msg}");
        assert!(msg.contains("设置"), "要给恢复入口：{msg}");
        assert!(msg.contains("p3.png"), "要点名文件：{msg}");
    }

    /// 装好请求并**关闭**的队列：`decode_loop` / `pop` 排空后即返回，测试不会挂在
    /// 条件变量上等一个永远不来的下一个请求。
    fn closed_queue(reqs: impl IntoIterator<Item = Req>) -> Arc<Queue> {
        let q = Arc::new(Queue::new());
        for r in reqs {
            assert!(q.push(r), "关闭前入队必须成功");
        }
        q.close();
        q
    }

    /// 关窗（`Decoder` 释放）后，队列里**还没开始**的请求要一起丢掉。
    ///
    /// m230 的 S3 臂实测：关掉一扇窗之后仍有 4 条请求卡在队列里，看门狗 10 s 后还在报
    /// `pending=4 run_ms=-1 phase=idle`——没人收结果，同格式的另一扇窗还得陪它们排。
    #[test]
    fn discarding_a_closed_queue_drops_queued_requests() {
        let q = Arc::new(Queue::new());
        let before = crate::core::decode_watchdog::pending_count();
        for (seq, kind) in [(1u64, ReqKind::Main), (2, ReqKind::Prefetch)] {
            let r = req(seq, kind);
            // 入队时 `Decoder::send` 会记这一笔；这里手工记，模拟"已经进了看门狗账"。
            crate::core::decode_watchdog::note_sent(r.seq, r.kind, &r.path, 0);
            assert!(q.push(r), "关窗前入队必须成功");
        }
        q.discard_pending();
        assert!(q.pop().is_none(), "关窗后不该再解没人要的图");
        assert_eq!(
            crate::core::decode_watchdog::pending_count(),
            before,
            "丢弃的请求必须同时从看门狗账上销掉（否则 10 s 后会为关掉的窗口报警）"
        );
    }

    /// 主画面插队：先塞两张缩略图，随后到的 `Main` 必须先被取走。
    ///
    /// 这条断言就是这次修复存在的理由：用户缩放那次重解原本排在缩略图/预读后面，
    /// 实测 100 MB 级样本上从 0.37s 拖到 1.98s（放大后一直糊着看）。
    #[test]
    fn main_jumps_ahead_of_queued_background_work() {
        let q = closed_queue([
            req(1, ReqKind::Thumb),
            req(2, ReqKind::Thumb),
            req(3, ReqKind::Main),
            req(4, ReqKind::Prefetch),
        ]);
        let mut order = Vec::new();
        while let Some(r) = q.pop() {
            order.push((r.seq, r.kind));
        }
        assert_eq!(
            order.first(),
            Some(&(3, ReqKind::Main)),
            "Main 必须第一个出队：{order:?}"
        );
        // 插队不许丢请求（这条正是 std mpsc `try_recv` 踩过的那个坑：四条只回来一条），
        // 也不该顺手打乱其余的到达顺序——缩略图条按序填格。
        // ★ m99 试过让 `Prefetch` 也插缩略图一档，三档节拍 + 60 张单圈语料量不出命中率差
        //   （两臂 94–95%），这条期望值因此原样留着（判废见 `Queue::pop` 的注释）。
        assert_eq!(
            &order[1..],
            &[
                (1, ReqKind::Thumb),
                (2, ReqKind::Thumb),
                (4, ReqKind::Prefetch)
            ],
            "其余按 FIFO：{order:?}"
        );
    }

    /// 过期早退：`Main` / `Prefetch` 的 seq 已被更新的 Main 追过时，
    /// `decode_loop` 不发解码，直接以空串 Err 发 Done 让 UI 记账清位。
    /// 用空串而非"未装后端"错，是因为空串**不可能**是真解码的结果——
    /// 单测据此断言"这条路径没进 decode_one"，不依赖 backend 是否装。
    #[test]
    fn stale_main_and_prefetch_are_cancelled_without_decode() {
        let (done_tx, done_rx) = channel::<Done>();
        let floor = Arc::new(AtomicU64::new(100));
        // 关闭的队列让 decode_loop 处理完这两条就退出。
        decode_loop(
            closed_queue([req(50, ReqKind::Main), req(99, ReqKind::Prefetch)]),
            done_tx,
            floor,
            None,
        );

        let d1 = done_rx.recv().expect("应有 50 号 Done");
        assert_eq!(d1.seq, 50);
        assert!(
            matches!(&d1.out, Err(s) if s.is_empty()),
            "过期 Main 应发空串 Err 早退，实际 {d1:?}"
        );

        let d2 = done_rx.recv().expect("应有 99 号 Done");
        assert_eq!(d2.seq, 99);
        assert!(
            matches!(&d2.out, Err(s) if s.is_empty()),
            "过期 Prefetch 应发空串 Err 早退"
        );

        // decode_loop 结束后 done_tx 已 drop → recv 立即 Err，队列空。
        assert!(done_rx.recv().is_err(), "不应再有第三条 Done");
    }

    /// 边界：`seq == floor` 是当前主画面，**不**取消（用 `<` 不用 `<=`）。
    /// Thumb 与主画面解耦，任何 seq 都不取消——这条由后续断言间接验证：
    /// Thumb 会走到 decode_one，返回非空 Err（未装 backend），不是空串。
    #[test]
    fn current_main_and_thumb_are_not_cancelled() {
        let (done_tx, done_rx) = channel::<Done>();
        let floor = Arc::new(AtomicU64::new(10));
        decode_loop(
            closed_queue([req(10, ReqKind::Main), req(11, ReqKind::Thumb)]),
            done_tx,
            floor,
            None,
        );

        // 两条都必须走真解码：未装 backend 时是"解码后端未安装…"的非空 Err。
        for seq in [10u64, 11] {
            let d = done_rx.recv().expect("应有 Done");
            assert_eq!(d.seq, seq);
            assert!(
                matches!(&d.out, Err(s) if !s.is_empty()),
                "seq={seq} 不该被取消，应走真解码并拿到非空 Err"
            );
        }
    }

    /// 0 字节文件要说真因。过去它落到解码器探测，用户看到「解码器「jpeg」不认识这个
    /// 文件」——把"文件是空的"说成"格式不认"，排查方向整个指反。
    #[test]
    fn empty_file_says_it_is_empty() {
        let p = crate::core::paths::test_scratch_root()
            .join(format!("rupix_empty_{}.png", std::process::id()));
        std::fs::write(&p, b"").expect("写空夹具");
        let r = probe_decode(&p, 0, 0);
        let _ = std::fs::remove_file(&p);
        let msg = r.expect_err("空文件不该解码成功");
        assert!(
            msg.contains("空的"),
            "0 字节文件应直说为空，而不是转述解码器认不认：{msg}"
        );
    }

    /// 主画面回包必须叫醒 UI，缩略图/预读不许。
    ///
    /// 这条测试守的是 m90 的**收益本身**：`std::sync::mpsc` 不会自己叫醒 UI，漏掉
    /// 这次唤醒就退回"等下一拍定时器"（实测 ≈31 ms）。反过来，若缩略图也唤醒，
    /// 目录里每解一格就白出一帧——静止窗口的省电设计当场作废。
    ///
    /// 变异证明：删掉 `wake_for_main()` 调用 → 第一条断言红；把判据改成
    /// `kind != Thumb` → 第二条断言红。
    #[test]
    fn only_current_main_result_wakes_the_ui() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static WAKES: AtomicUsize = AtomicUsize::new(0);
        install_done_waker(Box::new(|| {
            WAKES.fetch_add(1, Ordering::SeqCst);
        }));

        let (done_tx, done_rx) = channel::<Done>();
        let floor = Arc::new(AtomicU64::new(10));
        // Thumb 11 / Prefetch 12 都**不**作废（12 >= floor），会真走 decode 并回包；
        // 只有 Main 10 是当前主画面。
        decode_loop(
            closed_queue([
                req(10, ReqKind::Main),
                req(11, ReqKind::Thumb),
                req(12, ReqKind::Prefetch),
            ]),
            done_tx,
            floor,
            None,
        );
        assert_eq!(
            WAKES.load(Ordering::SeqCst),
            1,
            "三条回包里只该有当前 Main 唤醒 UI"
        );
        for _ in 0..3 {
            done_rx.recv().expect("三条回包都应入队");
        }
    }
}
