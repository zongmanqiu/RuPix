//! 图片画布：一个自定义 `Widget`，负责"把当前图按当前倍率画到视口里"以及鼠标交互。
//!
//! 菠萝看图式 UI 的主战场：画布吃下**整窗**的指针事件——图片绘制、棋盘格透明背景、
//! 左右翻页箭头、底部工具栏（`ui::chrome`，直接画在本控件里）与"悬停显示、静止隐藏"
//! 的显隐状态机都在这一层。
//!
//! # 为什么自己写控件而不用 `ImageView`
//!
//! `ImageView` 只有 `Fit::Contain`（永远适应容器），做不了"放大后拖着看"。
//! 看图器的缩放/平移是它的本体功能，必须自己算目标矩形。
//!
//! # 为什么悬浮层画在画布里而不是做成 `Element` 子树
//!
//! 见 `ui::chrome` 模块头注释——一句话：指针移到兄弟 `Element` 浮层上时画布收不到
//! Move，"悬停显示/静止隐藏"会闪烁；一个控件吃下整窗事件就没有这个问题。
//!
//! # 鼠标手势的分工
//!
//! 无边框窗口里"拖 == 动东西"的候选有两个：拖动窗口本体、平移图片。二者抢同一块面积，
//! 分工按下表。**左键归谁是一个开关**（ini 键 `drag_image`，默认开）——
//! 2026-10-08 那次拍的是"拖动窗口是本体"，2026-11-03 用户加了开关：
//! "允许我们只拖动图片，而窗口画布不变。但是在标题栏按住依然是可以拖动窗口的"。
//!
//! | 手势（`drag_image` **开**，默认）  | 效果                                   |
//! |-------------------------------------|----------------------------------------|
//! | 左键拖动 · 标题栏那一条             | 移动窗口（永远，不受开关影响）         |
//! | 左键拖动 · 画布里                     | **平移图片**，窗口不动；图边能越过窗口边，但永远还剩 24dp 在画面里 |
//! | 左键拖动 · 空窗（没有图 / 视口未量出）| 移动窗口（拖不动东西时不堵这条路）                        |
//! | 左键拖动 · 缩略图条里               | **平移缩略图条**（看更远的图），不换图、不动画面、不动窗口（m258） |
//! | 滚轮 · 缩略图条里                    | 平移缩略图条一格；Shift = 一屏（不看四行滚轮设置，条上没有缩放入口）|
//! | 拖右下角鸟瞰图里的视口框            | 改变视角（平移图片）                   |
//! | Ctrl+左键拖动 / 中键拖动            | 平移图片（兜底手势，不看开关）         |
//! | 右键                                | 上下文菜单                             |
//!
//! ★ **真全屏例外**（m238 修复轮）：窗口铺满显示器后不许被拖走 —— 画布里照常平移
//! 图片，顶条那一条与空窗一律不响应（判据见 [`left_drag`] 的 `fullscreen` 分支）。
//!
//! `drag_image` **关** = 回到 2026-10-08 那套：任意位置左键拖 = 移动窗口，
//! 平移图片只剩 Ctrl+左键、中键与鸟瞰三条。判据本体是 [`left_drag`]（纯函数，有单测）。
//!
//! 另外左键**挪够 4dp 才升级成拖窗口**（[`DRAG_SLOP`]）：按下即拖会去走系统的窗口移动
//! 模态循环，第二次按下就再也送不进本控件，双击关窗当场失效。
//!
//! # 状态放在哪
//!
//! 控件被 `Element` 以 `Box<dyn Widget>` 持有，`paint` 只给 `&self`、`on_event` 才给
//! `&mut self`——而按钮回调在控件外面。故真实状态放在 `Rc<RefCell<Viewer>>` 与
//! `Rc<Chrome>` 里，控件与快捷键/定时器共享同一份。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windui::core::{EventCtx, Widget};
use windui::event::{
    window_state, Event, Key, KeyEvent, MenuItem, MouseButton, PointerEvent, PointerKind,
    WindowState,
};
use windui::geometry::{Point, Rect, Size};
use windui::render::image::Fit;
use windui::render::{Canvas, Paint};
use windui::spec::Align;
use windui::style::Style;
use windui::text::{TextEngine, TextStyle};

use std::sync::Arc;

use crate::core::config::{AnimSpeed, Config};
use crate::core::gallery::{Jump, SortBy};
use crate::core::geometry::RectI;
use crate::core::pixels::MAX_TEXTURE_SIDE;
// 鸟瞰 / 浮层的配色取自全 ui 共用的令牌（[`crate::ui::tokens`]），本文件不自带色值。
// 别名引入：跟随主题那组是函数，名字短（`empty_bg` / `hint_text`），逐个 import 易撞名。
use super::chrome::{self, Arrow, Btn, Press, TitleBtn};
use super::companion::{
    companion_path_label, pair_step, CompanionPane, CompanionSource, LinkMode, Pane,
};
use super::plugin_settings::{LiveState, RunPrompt};
use super::tokens as tk;
use super::{filmstrip, settings, tree};
use super::{to_key_input, Chrome, Hub, Strip};
use crate::core::hotkey::Action;
use crate::core::pixels::RgbaImage;
use crate::core::viewer::Viewer;
use crate::platform::clipboard;
use windui::render::image::Image;

/// 棋盘格格子边长（逻辑 dp）。深浅两灰，与常见图像编辑器一致。
const CHECKER: f32 = 12.0;
// 底色 / 棋盘格 / 提示文字 / 错误文字都走主题访问函数（`tk::empty_bg()` 那一组）：
// 它们是"压在我们自己的底上"的颜色，深色主题与浅色主题各有一套。
// 只有双层描边是常量（`tk::tk::STROKE_DARK` / `tk::tk::STROKE_LIGHT`）——它压在用户图片上，
// 职责是"任意图片上都看得见"，不跟主题翻。

/// 鸟瞰缩略图盒（菠萝 NavigatorView setFixedSize(220, 160) 同尺寸），右上角。
const NAV_W: f32 = 220.0;
const NAV_H: f32 = 160.0;
const NAV_MARGIN: f32 = 12.0;

/// 辅图解码上限（单边物理像素 / 总像素）。
///
/// 辅图是右栏的对照视图，不是主画面：放大到 4× 时仍按“铺满整张图”的纹理拉伸，
/// 真按 zoom × DPI 去解一张上亿像素的图，UI 线程要拷贝、显卡要上传，反而卡。
/// 9 MP 足以覆盖 4K 屏半栏 1.25× 余量，再大就交给纹理放大。
const COMPANION_MAX_SIDE: f32 = 4096.0;
const COMPANION_MAX_PIXELS: f32 = 9_000_000.0;

type CompanionDrag = ((f32, f32), (f32, f32));

/// 主/辅 pane 的**最近一次绘制几何**，与拖放回调共享。
///
/// `windui` 的文件拖放回调只给路径、不给落点；但落点决定“开在主图还是辅图”。
/// 回调拿系统光标（拖放结束时光标就在落点）换算客户区坐标，再用这份几何判 pane。
#[derive(Clone)]
pub(crate) struct PaneGeom {
    pub primary: Rc<Cell<Rect>>,
    pub companion: Rc<Cell<Rect>>,
}

impl PaneGeom {
    pub(crate) fn new() -> Self {
        Self {
            primary: Rc::new(Cell::new(Rect::new(0, 0, 0, 0))),
            companion: Rc::new(Cell::new(Rect::new(0, 0, 0, 0))),
        }
    }

    /// 拖放落点是否在辅图 pane 内（辅图未开时矩形为零，恒假）。
    pub(crate) fn hits_companion(&self, x: i32, y: i32) -> bool {
        self.companion
            .get()
            .contains(windui::geometry::Point { x, y })
    }
}

pub struct ImageCanvas {
    viewer: Rc<RefCell<Viewer>>,
    chrome: Rc<Chrome>,
    /// 配置（设置对话框写、双击行为读）。
    config: Rc<RefCell<Config>>,
    /// 设置窗信号句柄（打开设置窗时填充，主窗轮询落盘）。
    hub: Hub,
    /// 最近一次 `paint` 拿到的内容矩形：事件回调里要算中心点，而事件不带几何信息。
    last: Cell<Rect>,
    /// 与窗口根节点的拖放回调共享的 pane 几何（判拖放落点）。
    pane_geom: PaneGeom,
    /// 鸟瞰缩略图状态：图显示尺寸超出视口时（且设置开启）显示导航图，拖动可平移主视图。
    nav_drag: Cell<bool>,
    /// 左键按下、但还没挪够阈值的起点。
    ///
    /// 越过阈值才把它升级成"拖动窗口"（[`DRAG_SLOP`]）——**按下即拖会把双击吞掉**：
    /// 系统移动用的是自己的模态循环，第二次按下再也收不到，双击关窗就没了。
    lmb_pending: Cell<Option<(f32, f32)>>,
    /// 滚轮翻页档的碎增量累计（高分辨率滚轮一格不到 ±120，攒够再翻）。
    wheel_acc: Cell<i32>,
    /// 主位图的上屏纹理缓存（按 gen 代号判脏，见 [`ImageCache`]）。
    main_img: RefCell<ImageCache>,
    /// 主辅视图的辅图：目标/异步成品。`Rc<RefCell>` 让「跳转到…帧/页」
    /// 对话框的 'static 闭包也能安全地写回它。
    companion: Rc<RefCell<CompanionPane>>,
    /// 辅图的上屏纹理缓存。
    comp_img: RefCell<ImageCache>,
    /// 主图实际可用的矩形（开主辅视图时是左半；关时与 `last_work` 相同）。
    /// 绘制/缩放锚点用它；chrome/胶片条/命中仍用 `last_work`。
    primary_work: Cell<Rect>,
    /// 辅图实际可用的矩形（开主辅视图时是右半；关时零矩形）。
    comp_work: Cell<Rect>,
    /// 辅图自己的缩放倍率（1.0 = 适应 pane）与平移。
    /// 用 `Rc<Cell>` 是为了右键「画面 ▸」菜单的闭包也能拿到同一份状态：
    /// 活动窗是辅图时，菜单里的放大/缩小/适应/1:1 必须和工具栏走同一条路。
    comp_zoom: Rc<Cell<f32>>,
    comp_pan: Rc<Cell<(f32, f32)>>,
    /// 上一帧辅图“适应 pane”时的倍率（`comp_zoom == 1.0` 对应的图像 scale）。
    /// 工具栏「1:1」与键盘 ActualSize 要靠它把辅图切到原图像素。
    comp_fit: Rc<Cell<f32>>,
    /// 辅图左键拖动：起点 + 起始 pan。
    comp_drag: Cell<Option<CompanionDrag>>,
    /// 当前辅图目标键（路径/页）；变了就重置缩放/平移。
    comp_target_key: RefCell<Option<(PathBuf, Option<u32>)>>,
    /// 上一帧辅图整图的目标矩形（鸟瞰几何/拖动映射用）。
    comp_dst: Cell<Rect>,
    /// 正在拖辅图的鸟瞰视口框。
    comp_nav_drag: Cell<bool>,
    /// 交换主辅后，新主图需要落到的页/帧（等首帧解码完成后由 paint 兑现）。
    pending_main_page: Cell<Option<(PathBuf, u32)>>,
    /// 鸟瞰缩略位图的上屏纹理缓存。
    nav_img: RefCell<ImageCache>,
    /// 实时调色状态（每窗一份）：参数窗写、本画布 paint 读。
    live: LiveState,
    /// 施加调色后的主图 / 鸟瞰纹理，按 (源 gen, 六个参数) 判脏。
    tint_main: RefCell<TintCache>,
    tint_nav: RefCell<TintCache>,

    // ── 底部缩略图条 ──
    /// 条的开关从 `chrome.filmstrip` 读；这里存**上一帧算出的格子几何**，供事件
    /// 回调做命中（事件不带几何，与 `last` 同样的理由）。
    strip_items: RefCell<Vec<(usize, Rect)>>,
    /// 条上按下的格子（下标）。抬起时仍在同一格才算点击。
    strip_pressed: Cell<Option<usize>>,
    /// 悬停的格子（下标），只用于高亮。
    strip_hover: Cell<Option<usize>>,
    /// 缩略图的上屏纹理缓存。
    strip: Strip,
    /// 条的滚动锚点（m258）：`None` = 跟着当前图居中（老行为），
    /// `Some(first)` = 用户拖过/滚过这里，条不再自动回正。
    strip_first: Cell<Option<usize>>,
    /// 条上正在进行的拖动：起点、起始首格、格距、主轴方向。`None` = 没在拖。
    strip_drag: Cell<Option<StripDrag>>,
    /// 本次拖动已越过阈值（[`DRAG_SLOP`]）⇒ 抬起时不再当"点格子"。
    strip_dragged: Cell<bool>,
    /// 上一帧画出的条几何（**视口绝对坐标**）：事件回调据此判断"指针在不在条上"、
    /// 以及滚动指示的热区。与 `strip_items` 一样只由 `paint` 写——命中与绘制同源。
    strip_at: Cell<Option<filmstrip::Placement>>,
    /// 正在拖滚动指示的滑块（m259）。`None` = 没在拖。
    strip_scrub: Cell<Option<StripScrub>>,
    /// 上一帧的每屏格数（条内滚轮"一屏"要用）与目录长度（判断能不能滚）。
    strip_cap: Cell<usize>,
    strip_len: Cell<usize>,
    /// 上一帧的边：换边（底 ↔ 左）时清锚点——两个方向格数不同，旧位置没有意义。
    strip_side: Cell<Option<filmstrip::Side>>,
    /// 上一帧清单所属的目录（m260）：换目录时清锚点。同一目录内换图**不清**——
    /// 用户要的就是"滚到很远处再点一张"。
    strip_dir: RefCell<Option<PathBuf>>,
    /// 上一次见到的排序代数（m275）。**重排之后"用户滚到的首格"指的是另一张图**，
    /// 所以和换目录一样要归零——不归零的话，换个排序就像被条拽到别处去了一次。
    strip_sort: Cell<u64>,
    /// 扣掉缩略图条之后**留给图片**的内容矩形。
    ///
    /// 条是常驻的一块地（不是盖在画面上的浮层），所以摆图、缩放锚点、鸟瞰几何
    /// 都用这一份；悬浮工具栏那一层仍用完整的 `last`（它本来就是浮层）。
    last_work: Cell<Rect>,

    // ── 文件树（m300）──
    /// 整包层树（先序）。只在"这一层来自压缩包"时非空——普通目录不画树。
    tree_nodes: RefCell<Vec<crate::core::archive::TreeNode>>,
    /// 这批节点属于哪个层目录。换层 / 换文件时重读并把滚动归零：
    /// 树的顺序跟着包走，旧首行在新树里指的是另一层。
    tree_for: RefCell<Option<PathBuf>>,
    /// 当前层在包里的路径段（`here()` 的结果）。与节点一起缓存——`here` 要读
    /// `layer.tsv`，不能每帧读一次。
    tree_segs: RefCell<Option<Vec<String>>>,
    /// 上一帧画出的树几何（**视口绝对坐标**）。`None` = 这一帧没画树，
    /// 事件回调据此决定拦不拦滚轮、点不点得中。
    tree_at: Cell<Option<Rect>>,
    /// 树的首行下标（滚动）。
    tree_first: Cell<usize>,
    /// 悬停的节点下标，只用于高亮。
    tree_hover: Cell<Option<usize>>,
    /// 这棵树有没有被层级预算截断（m306）。与节点同一批缓存：只在换层 / 换文件时
    /// 跟着 `tree.tsv` 重读一次，不是每帧一次文件系统调用。
    tree_truncated: Cell<bool>,
    /// 正在拖树的滚动指示（m307）。`None` = 没在拖。
    tree_scrub: Cell<Option<TreeScrub>>,
}

/// 滚轮翻页档每张图需要的增量（一格 WHEEL_DELTA）。
const WHEEL_STEP: i32 = 120;

/// 把一次滚轮增量并进累计器，攒够一格就给出**一个**滚轮符号（`0` = 还没攒够）。
/// 这是滚轮方向（向上 `+1` / 向下 `-1`），**不是**清单步进方向；交给
/// `Viewer::step` 或缩略图条之前必须经 [`wheel_nav_dir`] 翻号。
///
/// 单独抽出来只为一句：这条判据（碎增量 → 整格）与那条节奏闸（`Chrome::wheel_gate`）
/// 是两件事，测试要能各自钉住自己那件。★ 攒够就把累计器**清零**而不是扣一格——
/// 一次事件里攒出两格（240）也只翻一张，翻页速度由节奏闸封顶，不由增量总数封顶。
fn wheel_detent(acc: i32, delta: i32) -> (i32, isize) {
    let sum = acc + delta;
    if sum <= -WHEEL_STEP || sum >= WHEEL_STEP {
        (0, sum.signum() as isize)
    } else {
        (sum, 0)
    }
}

/// 滚轮符号 → 清单步进方向（m268：翻页与缩略图条共用这一处）。
///
/// Windows 的 `WM_MOUSEWHEEL` 向上滚给 `+120`、向下滚给 `-120`；清单惯例是
/// **向下滚 = 看后面**（`step(+1)` / 条首格往大走），所以把 [`wheel_detent`]
/// 的符号交给 `Viewer::step` 之前必须先翻号。m259 只在条上写了 `- dir`，翻页
/// 这一路漏了：向上滚反而换下一张，用户实机报「ctrl+向下滚轮，应该是看后面的
/// 图片？」。
/// ★ 抽成函数而不是两处各写一个负号：条与翻页从此共用同一根轴，谁再把裸
/// `dir` 喂给 `step` 就是把两套方向写回来了。
fn wheel_nav_dir(dir: isize) -> isize {
    -dir
}

/// m278：四行滚轮各读自己那一档，优先级 Ctrl > Alt > Shift > 裸滚轮。
///
/// 抽成纯函数是为了让「按修饰键选档」能被单测直接量，不必驱动真实窗口——
/// 不透明度档会碰 `EventCtx::set_window_opacity`，那种探针在测试夹具里不稳。
/// 下拉只有一张（[`crate::core::config::WheelAction::ALL`]），一个值管上下两个方向。
fn wheel_action_for(c: &Config, mods: &windui::event::Mods) -> crate::core::config::WheelAction {
    if mods.ctrl {
        c.ctrl_wheel
    } else if mods.alt {
        c.alt_wheel
    } else if mods.shift {
        c.shift_wheel
    } else {
        c.wheel
    }
}

/// 左键挪动多少才算"拖动窗口"而不是"点一下"（逻辑 dp）。
///
/// **必须有这个阈值**：直接按下就拖会走系统的窗口移动循环，那是个模态循环，
/// 第二次按下进不了本控件——双击关窗（本应用的主力交互之一）当场失效。
/// 比系统双击允许的抖动略大一点：手抖不会被当成拖动，想拖也不会觉得迟钝。
const DRAG_SLOP: f32 = 4.0;

/// 无边框窗四缘的缩放命中带（逻辑 dp）。
///
/// 与 windui `handle_nchittest` 的 `RESIZE_BORDER_LOGICAL` 同一个数：那一圈归
/// **非客户区**，按下是缩放、光标是系统给的 `IDC_SIZE*`，既不能拖图，也不该被
/// 宿主的自绘光标覆盖顶掉（2026-11-08 用户实报「移到边框线没反应」）。
const RESIZE_BAND_DP: f32 = 8.0;

/// 这次移动算不算"拖动"（越过 [`DRAG_SLOP`]）。
///
/// 单独抽成纯函数是为了能单测：**双击期间的手抖绝不能被当成拖窗口**，
/// 一旦判错，双击关窗（本应用的主力交互）就再也没机会发生了。
fn beyond_slop(start: (f32, f32), pos: (f32, f32)) -> bool {
    let dx = pos.0 - start.0;
    let dy = pos.1 - start.1;
    dx * dx + dy * dy >= DRAG_SLOP * DRAG_SLOP
}

/// 缩略图条拖动（m258）：条是**独立滚动的列表**，左键拖它 = 去看更远的图。
///
/// 记 `pitch` 而不是每帧重算：拖动过程中格数固定 ⇒ 格距不变，按下那一刻的正是
/// 用户手指下那一格的宽度（均分布局下，只剩两三张时一格会变宽，不能写死常量）。
#[derive(Clone, Copy)]
struct StripDrag {
    start: (f32, f32),
    first: usize,
    pitch: f32,
    /// 底条看横位移、左条看纵位移（只沿主轴生效，另一维的抖动不算）。
    horizontal: bool,
}

/// 拖动位移 → 新的窗口首格（纯函数，供单测）。
///
/// 往左/往上拖 = 想看**后面**的图 ⇒ 首格减小；反向同理。按整格取整：半格以内不动，
/// "拖一点点就窜一格"是错的手感。夹紧交给调用方给的 `max_first`（`len - n`）。
fn strip_drag_first(start_first: usize, delta: f32, pitch: f32, max_first: usize) -> usize {
    let steps = (delta / pitch.max(1.0)).round() as isize;
    (start_first as isize - steps).clamp(0, max_first as isize) as usize
}

/// 拖滚动指示的滑块（m259）：与"按格平移"分开，因为它按**行程比例**映射——
/// 目录几百张时，拖一格滑块要跳过几十张，按格算根本到不了。
#[derive(Clone, Copy)]
struct StripScrub {
    /// 命中时指针在主轴上的位置。
    grab_at: f32,
    /// 命中时滑块起点（主轴）。
    thumb0: f32,
    /// 滑块能走的行程（轨道长 - 滑块长）；`0` = 滑块占满，拖不动。
    span: f32,
    max_first: usize,
    /// 底条看横轴、左条看纵轴。
    horizontal: bool,
}

/// 正在拖树滚动指示的滑块（m307）。与 [`StripScrub`] 同构，只是树只有纵向。
#[derive(Clone, Copy)]
struct TreeScrub {
    /// 按下那一刻指针的 y。
    grab_at: f32,
    /// 按下那一刻滑块顶的 y。
    thumb0: f32,
    /// 滑块能走的行程（轨道长 - 滑块长）；`0` = 滑块占满，拖不动。
    span: f32,
    max_first: usize,
}

/// 拖动 → 首行（纯函数）。`grab_at` 是按下时的偏移 ⇒ 跟手不跳。
fn tree_scrub_first(s: &TreeScrub, y: f32) -> usize {
    if s.span <= 0.0 {
        return 0;
    }
    let t = (y - s.grab_at + s.thumb0).clamp(0.0, s.span);
    (t / s.span * s.max_first as f32).round() as usize
}

/// 点轨道空白 → 首行：让**滑块中心**落到指针处，与拖动共用同一套行程映射。
fn tree_scrub_jump(y: f32, track0: f32, thumb_len: f32, span: f32, max_first: usize) -> usize {
    if span <= 0.0 {
        return 0;
    }
    let t = (y - track0 - thumb_len / 2.0).clamp(0.0, span);
    (t / span * max_first as f32).round() as usize
}

/// 滑块拖动 → 首格（纯函数）。`grab` 是按下那一刻指针距滑块起点的偏移 ⇒ 跟手不跳。
fn scrub_first(s: &StripScrub, pos: (f32, f32)) -> usize {
    if s.span <= 0.0 {
        return 0;
    }
    let main = if s.horizontal { pos.0 } else { pos.1 };
    let t = (main - s.grab_at + s.thumb0).clamp(0.0, s.span);
    let ratio = t / s.span;
    (ratio * s.max_first as f32).round() as usize
}

/// 点轨道空白 → 首格：让**滑块中心**落到指针处（与拖动共用同一套行程映射，
/// 所以"点一下再拖"不会先跳一截）。
fn scrub_jump(
    pos: (f32, f32),
    track0: f32,
    thumb_len: f32,
    span: f32,
    max_first: usize,
    horizontal: bool,
) -> usize {
    if span <= 0.0 {
        return 0;
    }
    let main = if horizontal { pos.0 } else { pos.1 };
    let t = (main - track0 - thumb_len / 2.0).clamp(0.0, span);
    (t / span * max_first as f32).round() as usize
}

/// 左键按下之后"这一拖归谁"。判据抽成纯函数的理由与 [`beyond_slop`] 同一条：
/// 它是手感的全部，测试要能钉住，不能靠"跑起来拖一下看看"。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum LeftDrag {
    /// 移动窗口（越过 [`DRAG_SLOP`] 才真的交给系统模态循环）。
    Window,
    /// 平移图片（`Viewer::begin_drag` 开局，Move 里跟手）。
    PanImage,
    /// 不响应：**真全屏**里窗口铺满且固定，这一拖既不搬窗口也不搬图。
    None,
}

/// 分工规则（ini 键 `drag_image`，默认开）。三条**全部**满足才拖图片，任一不满足
/// 就回老行为拖窗口——**不许把窗口拖走的那条路堵死**：
///
/// - `drag_image`：用户自己要的开关；关掉就是 2026-10-08 那套拍板。
/// - `can_pan`：这张图**此刻拖得动**。★ 2026-11-03 用户第三、第四条口径连着放宽到这里：
///   图比窗口小也拖得动，而且**图边允许越过窗口边**（拖到只剩一枚衣角），
///   上限本体是 `Viewer::pan_limits` → `overscroll`（横向对称；纵向自 m290 起**不对称**——
///   顶上被标题栏遮住那一侧要多让出标题栏的高度，见 `Viewer::chrome_top`）。
///   仍然拖不动的只剩"还没有图"（空窗）与"视口还没量出来"两种——那时那一按留着拖窗口，
///   **不许把拖窗口这条路堵死**。
/// - `!in_title`：指针不在顶部标题栏那一条里——**标题栏永远带走窗口**（判据单一来源
///   见 [`Chrome::in_title_strip`]）。
///
/// 后三项是无边框形态（`chrome::borderless()`）与"图溢出视口"的输入，见下面那段。
fn left_drag(
    drag_image: bool,
    can_pan: bool,
    in_title: bool,
    fullscreen: bool,
    borderless: bool,
    overflows: bool,
) -> LeftDrag {
    // ★ m238 修复轮（用户 2026-11-05 实机）：真全屏时窗口铺满显示器且**不许被拖走**。
    // 收起顶条只改了绘制、关掉系统圆角只改了 DWM 属性，都没动这条手势判据 —— 于是
    // 顶缘按下仍会把全屏窗拖离显示器，看起来还是「假全屏」。全屏时：画布里照常平移
    // 图片，顶条那一条与「拖不动图的空窗」一律不响应，不留「能拖」的假象。
    if fullscreen {
        return if drag_image && can_pan && !in_title {
            LeftDrag::PanImage
        } else {
            LeftDrag::None
        };
    }
    // ★ m272（用户 2026-11-08 实机：「无边框拖窗口只有顶部那条 40dp 的隐形热区，
    //   没有任何提示」）：无边框形态整套自绘 chrome 全隐，**它没有标题栏**——顶部
    //   那条只是 `in_title_strip` 的几何残留，谁也看不见它，不会有人去试。
    //   那个形态下**整块画面就是标题栏**：左键一律搬窗口。
    //
    //   唯一的例外是图**真的溢出视口**（按了 `1`=100%、或滚轮放大过）：那时不拖图
    //   就没法看画外那半张，于是回落到平移；想搬窗口先缩回适应。这与普通窗口完全
    //   同构——放大后画面拖图、缩小时画面留给窗口——所以不必为无边框另立一套心智。
    //   `in_title` 在这里无所谓：两种结果都含 Window。
    if borderless {
        return if drag_image && overflows {
            LeftDrag::PanImage
        } else {
            LeftDrag::Window
        };
    }
    if drag_image && can_pan && !in_title {
        LeftDrag::PanImage
    } else {
        LeftDrag::Window
    }
}

/// 双击该做什么：**标题栏恒等于「最大化 / 恢复」**，画面区才按 `double_click` 设置。
///
/// ★ 2026-11-08 用户报「秒退」：默认 `double_click=1` 是**关窗**，而双击判定当年对
/// 标题栏和画面区一视同仁 —— 于是双击顶条就把整扇窗收了（日志里只有一次
/// `[close] action=Close`，无 panic，看着就像进程凭空消失）。
///
/// 标题栏是特殊区域：它承载的是**窗口级动作**（关闭 / 最大化 / 最小化 / 锁 / 置顶），
/// 双击那里在任何系统里都轮到「最大化 / 恢复」，不该被「双击图片关窗」这条产品设定
/// 接管。判据只此一处，别在分支里各写一遍。
fn double_click_action(
    in_title: bool,
    cfg: crate::core::config::DblClick,
) -> crate::core::config::DblClick {
    use crate::core::config::DblClick;
    if in_title {
        DblClick::Maximize
    } else {
        cfg
    }
}

/// 上屏纹理缓存：按 [`RgbaImage::gen`] 代号判脏，仅代号变化才重新上传 windui `Image`。
///
/// viewer 每帧 `clone()` 位图会深拷像素（几 MB/帧），改成持引用 + 这里按 gen
/// 命中旧纹理——未变换的翻页与缩放，上传成本为零。
///
/// 缩略图条（`ui::filmstrip`）复用同一份实现：那是"同一件事的第二处"，再写一遍
/// 迟早两份行为分叉。
pub(crate) struct ImageCache {
    gen: u64,
    img: Option<Image>,
    /// 放大超出位图像素时是否禁用插值（windui 补丁 011 的 `Image::with_pixelated`）。
    ///
    /// 只有**主画布**该置：>100% 时上屏位图就是原图档位，1 源像素被拉成 N×N 设备像素，
    /// 双线性会把像素格抹成糊面——用户报的"放大到最后全是马赛克、看不清像素格"正是这里。
    /// 鸟瞰与缩略图条不置：那里的放大是"把小图塞进大框"的展示性拉伸，糊比锯齿不难看，
    /// 而且它们绝大多数时候走的是降采样方向，本标志对降采样无效。
    pixelated: bool,
}

impl ImageCache {
    pub(crate) const fn new() -> Self {
        Self {
            gen: 0,
            img: None,
            pixelated: false,
        }
    }

    /// 声明"放大不得插值"（主画布专用，见 `pixelated` 字段）。
    #[must_use]
    pub(crate) const fn pixelated(mut self) -> Self {
        self.pixelated = true;
        self
    }

    /// 取（必要时重新上传的）windui 位图；上传失败时**清空并记账**：
    /// 留旧纹理会让 ①gen 不推进 → 每帧重试刷错误日志；②上一张图被画到新图的
    /// 目标框里（错图）。清空后本轮留白，等 src.gen 再变才重试。
    ///
    /// ★ **超 GPU 纹理上限的位图不进上传流程**（[`MAX_TEXTURE_SIDE`]）：D3D11 建不出
    ///   16384 以上的单边，而 `Image::from_rgba` 会**先把整份缓冲预乘复制一遍**才去建纹理
    ///   ⇒ 一张 256 MB 的位图白拷一次才失败。判据放在最前面：超限直接拒（并记账），
    ///   不白烧那次拷贝。正常路径上这个闸不该响——显示位图已由 `core::derive` 降过边。
    pub(crate) fn obtain(&mut self, src: &RgbaImage) -> Option<&Image> {
        if self.gen != src.gen {
            if src.width > MAX_TEXTURE_SIDE || src.height > MAX_TEXTURE_SIDE {
                log::error!(
                    "位图单边超过显卡纹理上限（{}×{} > {MAX_TEXTURE_SIDE}）：不上传（gen={}）",
                    src.width,
                    src.height,
                    src.gen
                );
                self.img = None;
                self.gen = src.gen;
                return None;
            }
            match Image::from_rgba(src.width, src.height, &src.rgba)
                // ★ 补丁 011：标志随本缓存实例，与位图内容无关——同一张图在鸟瞰里
                // 仍走双线性，在主画布里才保持硬边。
                .map(|img| img.with_pixelated(self.pixelated))
            {
                Ok(img) => {
                    self.img = Some(img);
                    self.gen = src.gen;
                }
                Err(e) => {
                    log::error!(
                        "位图上传失败：{e}（gen={}，本轮留白，待下次换代重试）",
                        src.gen
                    );
                    self.img = None;
                    self.gen = src.gen;
                }
            }
        }
        self.img.as_ref()
    }
}

/// 显示层纹理：键 = (源位图 gen, 六个参数, Gamma, 旋转角度)，变一次才重算一次 + 重新上传。
///
/// 只变换**已经派生到显示档**的那份位图（视口尺寸量级），原图/base 一字节不动：
/// 8 MP 整图每帧重算会卡住拖动，而显示档在适应窗口时通常不到 2 MP；查表版变换
/// 又比逐像素除法快一个量级。原图档放大到 100% 以上时才等于整图，那时一帧仍只算一次。
struct TintCache {
    /// (显示档 gen, 六个 gain/bias, Gamma, 角度)。四个都相同才复用上一帧的位图。
    /// ★ 角度进键是「不累积重采样」的落点：每次都从 `src` 重算，绝不用上一帧输出。
    key: Option<(u64, [i32; 6], i32, i32)>,
    img: Option<RgbaImage>,
    upload: ImageCache,
}

impl TintCache {
    const fn new() -> Self {
        Self {
            key: None,
            img: None,
            upload: ImageCache::new(),
        }
    }

    /// 主窗用：>100% 放大时保持硬边，与未调色那一档的 `pixelated` 一致，
    /// 否则一开滤镜像素格就糊成一片，看着像"换了一张图"。
    const fn new_pixelated() -> Self {
        Self {
            key: None,
            img: None,
            upload: ImageCache::new().pixelated(),
        }
    }

    fn obtain(&mut self, src: &RgbaImage, v: [i32; 6], gamma: i32, deg: i32) -> Option<&Image> {
        if self.key != Some((src.gen, v, gamma, deg)) {
            let px = crate::core::pixels::channel_gamma_lut(
                &src.rgba,
                [v[0], v[1], v[2]],
                [v[3], v[4], v[5]],
                gamma,
            );
            // 顺序与 `edit_actions` 的另存路径一致：先调色、后旋转。
            // 于是「窗里看到的」==「另存出去的」（pixels.rs 的 parity 判据）。
            let px = if deg != 0 {
                crate::core::geometry::rotated(&px, src.width, src.height, deg)
            } else {
                px
            };
            self.img = Some(RgbaImage::new(src.width, src.height, px));
            self.key = Some((src.gen, v, gamma, deg));
        }
        let img = self.img.as_ref()?;
        self.upload.obtain(img)
    }
}

/// core 的 [`RectI`] → windui `Rect`（同构 1:1 转换）。
fn to_rect(r: RectI) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

/// windui `Rect` → core 的 [`RectI`]（[`to_rect`] 的逆变换）。
fn from_rect(r: Rect) -> RectI {
    RectI::new(r.x, r.y, r.w, r.h)
}

/// 鸟瞰缩略真正画在哪个矩形（纯算术，可单测）。
///
/// `inner` 是盒子里**内白描边以内**的那块，`idst` 是要画的位图在屏上的目标矩形，
/// `dst` 是整图在屏上的目标矩形。两者相同（位图、以及已建好整图缩略的 SVG）就是
/// 铺满 `inner`；不同（首帧之前退回主位图的 SVG 切片）按比例落位，且绝不画到盒外。
///
/// ★ m103：**两根轴各自归一，不许用一个比例乘来乘去**。旧写法
/// `s = thumb.w / dst.w`，然后 `th = idst.h * s`——盒宽是浮点截断过的整数，用它反推
/// 出来的 `s` 比真比例小一点点，再乘到**另一根轴**上就放大成好几个像素：
/// 600×4000 的竖条图，盒 22×152，画出来只有 22×147，下缘露出 5 px 的盒底/描边；
/// 1234×2977 那格差 1 px（实拍：左、上贴到图缘，右、下空一圈）。用户看到的
/// "鸟瞰边缘有空白/暗边、原图那里明明有内容"就是这一块。
/// 现在 `x` 只由 `dst.w → inner.w` 映射、`y` 只由 `dst.h → inner.h` 映射：源是整图时
/// 两根轴各自正好铺满；再配**原点 `floor`、远端 `ceil`、夹回 `inner`**，切片态也不越界。
fn nav_draw_rect(inner: Rect, idst: Rect, dst: Rect) -> Rect {
    let sx = inner.w as f32 / dst.w as f32;
    let sy = inner.h as f32 / dst.h as f32;
    let l = (inner.x as f32 + (idst.x - dst.x) as f32 * sx).floor() as i32;
    let t = (inner.y as f32 + (idst.y - dst.y) as f32 * sy).floor() as i32;
    let r = (l as f32 + idst.w as f32 * sx).ceil() as i32;
    let b = (t as f32 + idst.h as f32 * sy).ceil() as i32;
    // 夹回 inner：右端跟着左端的钳位走，否则左右各钳一次会让宽度少算一格。
    let (l, t) = (l.max(inner.x), t.max(inner.y));
    let (r, b) = (
        r.min(inner.x + inner.w).max(l + 1),
        b.min(inner.y + inner.h).max(t + 1),
    );
    Rect::new(l, t, r - l, b - t)
}

/// 画一枚鸟瞰：盒底 + 双层描边 + 完整缩略图 + 当前视口框。
///
/// 主图和辅图共用，`content` 是这一侧的视口（主图左半 / 辅图右半），
/// `dst` 是整图在屏上的目标矩形，`idst` 是缩略图源图的对应矩形。
fn paint_nav(
    canvas: &mut dyn Canvas,
    box_r: Rect,
    thumb: Rect,
    content: Rect,
    dst: Rect,
    tex: Option<&Image>,
    idst: Rect,
) {
    canvas.fill_rect(
        box_r.x as f32,
        box_r.y as f32,
        box_r.w as f32,
        box_r.h as f32,
        &Paint::fill(tk::pill_bg()),
    );
    canvas.stroke_round_rect(
        box_r.x as f32 - 1.0,
        box_r.y as f32 - 1.0,
        box_r.w as f32 + 2.0,
        box_r.h as f32 + 2.0,
        0.0,
        1.0,
        &Paint::fill(tk::STROKE_DARK),
    );
    canvas.stroke_round_rect(
        box_r.x as f32,
        box_r.y as f32,
        box_r.w as f32,
        box_r.h as f32,
        0.0,
        1.0,
        &Paint::fill(tk::STROKE_LIGHT),
    );
    let inner = if thumb.w >= 3 && thumb.h >= 3 {
        Rect::new(thumb.x + 1, thumb.y + 1, thumb.w - 2, thumb.h - 2)
    } else {
        thumb
    };
    if let Some(tex) = tex {
        let draw = nav_draw_rect(inner, idst, dst);
        canvas.draw_image(tex, draw, Fit::Fill, 0.0, 1.0);
    }
    let u0 = ((content.x as f32 - dst.x as f32) / dst.w as f32).clamp(0.0, 1.0);
    let u1 = ((content.x as f32 + content.w as f32 - dst.x as f32) / dst.w as f32).clamp(0.0, 1.0);
    let v0 = ((content.y as f32 - dst.y as f32) / dst.h as f32).clamp(0.0, 1.0);
    let v1 = ((content.y as f32 + content.h as f32 - dst.y as f32) / dst.h as f32).clamp(0.0, 1.0);
    let fx = inner.x as f32 + u0 * inner.w as f32;
    let fy = inner.y as f32 + v0 * inner.h as f32;
    let fw = ((u1 - u0) * inner.w as f32).max(4.0);
    let fh = ((v1 - v0) * inner.h as f32).max(4.0);
    canvas.fill_rect(fx, fy, fw, fh, &Paint::fill(tk::NAV_FILL));
    canvas.stroke_round_rect(
        fx - 1.0,
        fy - 1.0,
        fw + 2.0,
        fh + 2.0,
        0.0,
        1.0,
        &Paint::fill(tk::STROKE_DARK),
    );
    canvas.stroke_round_rect(fx, fy, fw, fh, 0.0, 1.0, &Paint::fill(tk::STROKE_LIGHT));
}

/// 「另存为…」的默认文件名：原图主名 + `.png`。
///
/// 以前写死「涂鸦.png」，用户每存一张都要自己重命名一遍——原图叫
/// `IMG_1234.jpg` 时那个默认名一点信息量都没有。
fn default_png_name(cur: &std::path::Path) -> String {
    let stem = cur
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "涂鸦".to_string());
    format!("{stem}.png")
}

/// 编辑态的两个出口（另存为 / 还原）。
///
/// 三处共用同一份实现：右键菜单（老插件）、运行参数窗（`run_ui` 插件）、插件热键。
/// 造在这里而不是参数窗里，是因为参数窗不该认识 `Viewer` / `Chrome`。
fn edit_actions(
    viewer: &Rc<RefCell<Viewer>>,
    chrome: &Rc<Chrome>,
    live: &LiveState,
    cur: &std::path::Path,
) -> super::plugin_settings::RunActions {
    let v = viewer.clone();
    let ch = chrome.clone();
    let live_save = live.clone();
    // 闭包要求 'static：把路径搬成 owned，别把函数的引用带进去。
    let cur_owned = cur.to_path_buf();
    let cur_save = cur_owned.clone();
    let save: Rc<dyn Fn(&mut EventCtx)> = Rc::new(move |ctx| {
        // 另存拿到的是**屏上朝向**那一份，与主菜单里的老入口同款。
        let Some(edit) = v.borrow().whole_image_for_plugin() else {
            ctx.toast_err("屏上没有可另存的整图（切片态或未载入）");
            return;
        };
        let (w, h, rgba) = (edit.width, edit.height, edit.rgba.clone());
        // 实时调色只在显示层，没进 base_rgba：另存必须把同一组参数算进整图，
        // 否则存出去的还是没调过的原图（用户看到的是调过的）。
        let t = live_save
            .tint
            .get()
            .filter(|t| !t.is_neutral() && edit.path == t.path);
        let out = match t {
            Some(t) => Arc::new(crate::core::pixels::channel_gamma_lut(
                rgba.as_slice(),
                t.gain(),
                t.bias(),
                t.gamma(),
            )),
            None => rgba,
        };
        // ★ 显示层只在上屏档转过；整图这里补同一角度，且顺序仍是「先调色后旋转」。
        // 少这一步，用户看到的是转过的图、另存出去的却是原角度（parity 判据）。
        let deg = if ch.rotate_path.get().as_deref() == Some(cur_save.as_path()) {
            super::rotate_dialog::angle_of(ch.rotate_deg.get())
        } else {
            0
        };
        let out = if deg != 0 {
            Arc::new(crate::core::geometry::rotated(out.as_slice(), w, h, deg))
        } else {
            out
        };
        let ch = ch.clone();
        let name = default_png_name(&cur_save);
        ctx.request_save_file(
            windui::prelude::PickDialog::new()
                .title("另存为 PNG")
                .file_name(name),
            move |dest| ch.start_save_png(dest, w, h, out),
        );
    });
    let v2 = viewer.clone();
    let cur_r = cur_owned;
    let live_revert = live.clone();
    let ch_revert = chrome.clone();
    let revert: Rc<dyn Fn(&mut EventCtx)> = Rc::new(move |ctx| {
        // 三个来源一起还原：显示层调色 + 旋转角度 + 插件正式写回的编辑覆盖。
        let had_tint = live_revert.tint.get().is_some();
        let had_rotate = ch_revert.rotate_deg.get() != 0.0;
        ch_revert.rotate_deg.set(0.0);
        ch_revert.rotate_path.set(None);
        live_revert.tint.set(None);
        let r = live_revert.reset.get();
        live_revert.reset.set(r + 1);
        let restored = v2.borrow_mut().revert_edited(&cur_r);
        if had_tint || had_rotate || restored {
            log::info!(
                "[plugin-edit] verdict=reverted file={} tint={} rotate={} overlay={}",
                cur_r.display(),
                had_tint,
                had_rotate,
                restored
            );
            ctx.toast_ok("已还原");
        } else {
            ctx.toast_ok("当前没有需要还原的编辑");
        }
    });
    super::plugin_settings::RunActions { save, revert }
}

impl ImageCanvas {
    /// 与窗口根节点的拖放回调共享 pane 几何；`new` 默认自建一份，测试不受影响。
    pub(crate) fn with_pane_geom(mut self, geom: PaneGeom) -> Self {
        self.pane_geom = geom;
        self
    }

    pub fn new(
        viewer: Rc<RefCell<Viewer>>,
        chrome: Rc<Chrome>,
        config: Rc<RefCell<Config>>,
        hub: Hub,
    ) -> Self {
        Self {
            viewer,
            chrome,
            config,
            hub,
            last: Cell::new(Rect::new(0, 0, 1, 1)),
            pane_geom: PaneGeom::new(),
            nav_drag: Cell::new(false),
            lmb_pending: Cell::new(None),
            wheel_acc: Cell::new(0),
            main_img: RefCell::new(ImageCache::new().pixelated()),
            companion: Rc::new(RefCell::new(CompanionPane::new())),
            comp_img: RefCell::new(ImageCache::new()),
            primary_work: Cell::new(Rect::new(0, 0, 1, 1)),
            comp_work: Cell::new(Rect::new(0, 0, 0, 0)),
            comp_zoom: Rc::new(Cell::new(1.0)),
            comp_pan: Rc::new(Cell::new((0.0, 0.0))),
            comp_fit: Rc::new(Cell::new(1.0)),
            comp_drag: Cell::new(None),
            comp_target_key: RefCell::new(None),
            comp_dst: Cell::new(Rect::new(0, 0, 0, 0)),
            comp_nav_drag: Cell::new(false),
            pending_main_page: Cell::new(None),
            nav_img: RefCell::new(ImageCache::new()),
            live: LiveState::new(),
            tint_main: RefCell::new(TintCache::new_pixelated()),
            tint_nav: RefCell::new(TintCache::new()),
            strip_items: RefCell::new(Vec::new()),
            strip_pressed: Cell::new(None),
            strip_hover: Cell::new(None),
            strip: Strip::new(),
            strip_first: Cell::new(None),
            strip_drag: Cell::new(None),
            strip_dragged: Cell::new(false),
            strip_at: Cell::new(None),
            strip_scrub: Cell::new(None),
            strip_cap: Cell::new(1),
            strip_len: Cell::new(0),
            strip_side: Cell::new(None),
            strip_dir: RefCell::new(None),
            strip_sort: Cell::new(0),
            last_work: Cell::new(Rect::new(0, 0, 1, 1)),
            tree_nodes: RefCell::new(Vec::new()),
            tree_for: RefCell::new(None),
            tree_segs: RefCell::new(None),
            tree_at: Cell::new(None),
            tree_first: Cell::new(0),
            tree_hover: Cell::new(None),
            tree_truncated: Cell::new(false),
            tree_scrub: Cell::new(None),
        }
    }

    fn center(&self) -> (f32, f32) {
        // 用扣掉缩略图条之后的矩形：条占了底部一条，画面中心就该往上抬，
        // 否则滚轮缩放与鸟瞰导航的锚点都会偏。主辅视图开时用主图左半。
        let c = self.view_work();
        (c.x as f32 + c.w as f32 / 2.0, c.y as f32 + c.h as f32 / 2.0)
    }

    /// 主图实际可用的矩形：`paint` 每帧写 `primary_work`；测试/首帧还没画时
    /// 回落到 `last_work`，不改变旧几何。
    fn view_work(&self) -> Rect {
        let p = self.primary_work.get();
        if p.w <= 1 && p.h <= 1 {
            self.last_work.get()
        } else {
            p
        }
    }

    /// 当前是否显示底部缩略图条（决定要不要扣布局高度）。
    fn strip_on(&self) -> bool {
        self.chrome.filmstrip.get() && self.viewer.borrow().gallery_len() > 0
    }

    /// 鸟瞰几何：`Some((导航盒, 缩略图区域))`。仅当图显示尺寸超出视口时。
    /// paint（绘制层）与事件（命中/拖动）共用同一份几何，保证两处一致。
    /// 鸟瞰几何：鸟瞰锚在 `content`，躲工具栏按 `view` 算。
    /// 主辅视图下 `content` 是左/右半 pane，`view` 是整块 `work`。
    fn nav_geom_in(&self, content: Rect, view: Rect, dst: Rect) -> Option<(Rect, Rect)> {
        // 未超出视口（含 1px 容差）不显示——鸟瞰只服务于"放得下"到"放不下"的边界。
        if dst.w <= content.w + 1 && dst.h <= content.h + 1 {
            return None;
        }
        // 缩略图等比 contain 进上限（各留 8dp），**比例与图像严格一致**。
        let s = ((NAV_W - 8.0) / dst.w as f32).min((NAV_H - 8.0) / dst.h as f32);
        // ★ `.max(1.0)` 不是装饰：极端长宽比（100×16384 这类条图，两道像素闸都拦不到它）
        // 等比缩完短边不足 1 px，`as i32` 截成 0 → 命中区宽 0，`nav_hit` 的
        // `pos < box.x + 0` 恒假，鸟瞰**永远点不到**；而绘制侧另有 `max(1.0)`，
        // 于是"看得见 1 px、点不到"。比例仍严格等于图像比例（2026-10-12 拍板），
        // 只是不许塌成 0。
        let tw = (dst.w as f32 * s).max(1.0);
        let th = (dst.h as f32 * s).max(1.0);
        // 锚点沿用 nav_box 的右下定位（含躲工具栏的上抬逻辑），缩略图贴齐其右下。
        let anchor = Self::nav_box_in(content, view);
        let tx = anchor.x as f32 + anchor.w as f32 - tw;
        let ty = anchor.y as f32 + anchor.h as f32 - th;
        // 盒 = 缩略图本身（2026-10-12 用户拍板）：黑白双层框直接贴图缘，替代
        // 早先的 6dp 暗色留白（那圈深色底在白框内侧形成好几 px 的暗边，多余）。
        // 比例即图像比例，盒内没有任何留白。
        let box_r = Rect::new(tx as i32, ty as i32, tw as i32, th as i32);
        Some((box_r, Rect::new(tx as i32, ty as i32, tw as i32, th as i32)))
    }

    /// 鸟瞰盒的位置：**右下角**（菠萝 NavigatorView 摆在右下角；这个功能本来就是
    /// 给"转视角"用的，放在画面右下比压住标题信息好）。
    ///
    /// 唯一例外是**躲底部工具栏**：窄窗口下浮条（居中）会横到右下角来，压上去就
    /// 变成"缩略图挡住了半透明的按钮"。这时整体上抬到浮条之上；抬不进去（窗口太矮）
    /// 就维持右下——浮条是半透明浮层且会自动隐藏，盖一点比跑出窗口好。
    /// 鸟瞰盒：锚在 `content` 右下角，躲工具栏用 `view` 的满格几何。
    fn nav_box_in(content: Rect, view: Rect) -> Rect {
        let view = (view.x as f32, view.y as f32, view.w as f32, view.h as f32);
        // 保守取"动画组也在"的那一行：这里若跟随当前显隐，切到动图的一刻缩略图
        // 会跟着往上跳一格——避让位置必须是稳定的。
        let (px, py, pw, ph) = Chrome::pill(view, true, Chrome::cell_reserve(view.2));
        let bx = content.x as f32 + content.w as f32 - NAV_W - NAV_MARGIN;
        let mut by = content.y as f32 + content.h as f32 - NAV_H - NAV_MARGIN;
        // 重叠判定不分浮条当前是否可见：它出现的位置是固定的，若按可见态算，
        // 显隐一变缩略图就会跟着跳。
        let overlap_x = bx < px + pw;
        let overlap_y = by < py + ph && by + NAV_H > py;
        if overlap_x && overlap_y {
            let raised = py - 8.0 - NAV_H;
            if raised >= content.y as f32 + chrome::STRIP_H as f32 {
                by = raised;
            }
        }
        Rect::new(bx as i32, by as i32, NAV_W as i32, NAV_H as i32)
    }

    /// 鸟瞰拖动：把缩略图上的点映射回图像，让该点对准视口中心。
    /// `dst` 是调用瞬间的图像显示矩形（pan 会变，比例不变，一次换算即可）。
    ///
    /// 返回**画面是否真的变了**：拖到图边缘时 pan 被钳在原地，此时不必重绘（M6）。
    fn nav_pan_to(&self, dst: Rect, thumb: Rect, mouse: (f32, f32)) -> bool {
        let u = ((mouse.0 - thumb.x as f32) / thumb.w as f32).clamp(0.0, 1.0);
        let v = ((mouse.1 - thumb.y as f32) / thumb.h as f32).clamp(0.0, 1.0);
        let img_pt = (
            dst.x as f32 + u * dst.w as f32,
            dst.y as f32 + v * dst.h as f32,
        );
        let center = self.center();
        let cur = self.viewer.borrow().pan();
        self.viewer
            .borrow_mut()
            .set_pan((cur.0 + center.0 - img_pt.0, cur.1 + center.1 - img_pt.1))
    }

    /// 视口内当前的鸟瞰几何（事件路径用；`last` 是最近 paint 存下的内容矩形）。
    fn nav_hit(&self, pos: (f32, f32)) -> Option<(Rect, Rect)> {
        // 主辅视图开时，主图鸟瞰在左半 pane，不在整块 work 的右下角。
        let content = self.view_work();
        let v = self.viewer.borrow();
        let dst = v.dst_rect(from_rect(content))?;
        let geom = self.nav_geom_in(content, self.last_work.get(), to_rect(dst))?;
        let (box_r, _) = geom;
        let inside = pos.0 >= box_r.x as f32
            && pos.0 < (box_r.x + box_r.w) as f32
            && pos.1 >= box_r.y as f32
            && pos.1 < (box_r.y + box_r.h) as f32;
        if inside {
            Some(geom)
        } else {
            None
        }
    }

    /// 鼠标在鸟瞰图上：拖动无论出了盒子都应该继续跟随（开了头的拖动不该中途断），
    /// 故这里不看命中、只重算几何。
    ///
    /// ★ 每一句都先把 Option/几何绑成局部变量：`if let` 的临时值会活到整个语句结束，
    /// 写成 `if let Some(dst) = self.viewer.borrow().dst_rect(..)` 的话，块里
    /// `nav_pan_to` 的 `borrow_mut` 直接撞上一个还活着的 `Ref`。（见 [`Self::nav_grab`]）
    ///
    /// 返回**画面是否真的变了**（几何算不出来时也算不出"变"，返 `false`）。
    fn nav_pan_at(&self, pos: (f32, f32)) -> bool {
        let content = self.view_work();
        let dst = self.viewer.borrow().dst_rect(from_rect(content));
        let Some(dst) = dst else { return false };
        let Some((_, thumb)) = self.nav_geom_in(content, self.last_work.get(), to_rect(dst)) else {
            return false;
        };
        self.nav_pan_to(to_rect(dst), thumb, pos)
    }

    /// 辅图鸟瞰几何：只在该 pane 里图显示尺寸超出视口时出现。
    fn comp_nav_geom(&self) -> Option<(Rect, Rect)> {
        if !self.chrome.companion.get() {
            return None;
        }
        let dst = self.comp_dst.get();
        if dst.w <= 0 || dst.h <= 0 {
            return None;
        }
        self.nav_geom_in(self.comp_work.get(), self.last_work.get(), dst)
    }

    /// 拖辅图鸟瞰：把缩略图上的点映射回辅图，让该点对准辅图 pane 中心。
    fn comp_nav_pan_to(&self, dst: Rect, thumb: Rect, mouse: (f32, f32)) -> bool {
        let u = ((mouse.0 - thumb.x as f32) / thumb.w as f32).clamp(0.0, 1.0);
        let v = ((mouse.1 - thumb.y as f32) / thumb.h as f32).clamp(0.0, 1.0);
        let img_pt = (
            dst.x as f32 + u * dst.w as f32,
            dst.y as f32 + v * dst.h as f32,
        );
        let c = self.comp_work.get();
        let center = (c.x as f32 + c.w as f32 / 2.0, c.y as f32 + c.h as f32 / 2.0);
        let pan = self.comp_pan.get();
        let next = (pan.0 + center.0 - img_pt.0, pan.1 + center.1 - img_pt.1);
        if next == pan {
            return false;
        }
        self.comp_pan.set(next);
        // 同一帧可能还会来 Move；先把映射用的 dst 跟着 pan 推一格，
        // 否则连续拖动会按旧矩形反复算，视口框追不上鼠标。
        self.comp_dst.set(Rect::new(
            dst.x + (next.0 - pan.0).round() as i32,
            dst.y + (next.1 - pan.1).round() as i32,
            dst.w,
            dst.h,
        ));
        true
    }

    /// 鸟瞰一击：命中就把视口中心挪到该点。返回是否落在鸟瞰盒上。
    ///
    /// # 为什么必须单独写成这个函数
    ///
    /// 它内部要用 `nav_pan_to`（那里对 viewer `borrow_mut`）。而 Rust 里
    /// `if let Some(x) = cell.borrow().f() { ... }` 这种写法，`Ref` 会**活到整个
    /// if-let 语句结束**（含块内），于是块里的 `borrow_mut` 就是
    /// `RefCell already borrowed` panic。
    ///
    /// 这类 panic 在本应用里格外致命：调用栈在 `extern "system"` 的 wnd_proc 里，
    /// 展开不出去就只能 abort——表象是"点一下整个窗口就没了"（2026-10-08 实测，
    /// `temp\crash-<今天>.log` 第一条就是它）。想避免这类 panic，唯一办法是
    /// **让 Ref 在进 if 之前就释放**：先 `let opt = ...borrow()...;` 再对局部变量解构。
    fn nav_grab(&self, pos: (f32, f32)) -> bool {
        if self.nav_hit(pos).is_none() {
            return false;
        }
        // 一击总要重绘（视口框与画面都要跟着动），所以这里不看 `nav_pan_at` 的返回值：
        // 它回答的是"pan 变了没有"，而调用方关心的是"这一击归我接管了"。
        self.nav_pan_at(pos);
        true
    }

    /// 底栏当前是否归辅图：活动窗是辅图、且辅图真有页/帧号。
    fn companion_owns_index(&self) -> bool {
        !chrome::borderless()
            && !self.chrome.companion_sync_view.get()
            && self.chrome.companion.get()
            && self.chrome.active_pane.get() == Pane::Companion
            && self.companion.borrow().has_index()
    }

    /// 活动辅图的逐页/逐帧键：命中就翻辅图自己的页，不碰主图。
    fn companion_frame_key(&self, k: &KeyEvent) -> bool {
        if !self.companion_owns_index() {
            return false;
        }
        let Some(ki) = to_key_input(k) else {
            return false;
        };
        let delta = match self.config.borrow().hotkeys.match_action(&ki) {
            Some(Action::FramePrev) => -1,
            Some(Action::FrameNext) => 1,
            _ => return false,
        };
        self.companion.borrow_mut().step_index(delta)
    }

    /// 活动窗是不是辅图（只有开主辅视图时才有意义）。
    fn companion_view_active(&self) -> bool {
        !chrome::borderless()
            && !self.chrome.companion_sync_view.get()
            && self.chrome.companion.get()
            && self.chrome.active_pane.get() == Pane::Companion
    }

    /// 活动辅图是否处于「自由翻页」：左右方向键/边缘箭头改翻辅图自己的清单。
    fn companion_free(&self) -> bool {
        self.companion_view_active() && self.chrome.companion_link.get() == LinkMode::Free
    }

    /// 自由翻页的文件导航键：`Prev` / `Next`（方向键、PageUp/PageDown）。
    /// 页/帧键（`,`/`.`）由 [`Self::companion_frame_key`] 单独处理。
    fn companion_file_key(&self, k: &KeyEvent) -> bool {
        if !self.companion_free() {
            return false;
        }
        let Some(ki) = to_key_input(k) else {
            return false;
        };
        let delta = match self.config.borrow().hotkeys.match_action(&ki) {
            Some(Action::Prev) => -1,
            Some(Action::Next) => 1,
            _ => return false,
        };
        self.companion.borrow_mut().step_file(delta)
    }

    /// 活动辅图的缩放/平移键：ZoomIn/Out、Fit、ActualSize、WASD 平移。
    /// 文件翻页键由 [`Self::companion_file_key`] 单独处理。
    fn companion_view_key(&self, k: &KeyEvent) -> bool {
        if !self.companion_view_active() {
            return false;
        }
        let Some(ki) = to_key_input(k) else {
            return false;
        };
        let action = self.config.borrow().hotkeys.match_action(&ki);
        let Some(action) = action else {
            return false;
        };
        let z = self.comp_zoom.get();
        let mut handled = true;
        match action {
            Action::ZoomIn => self.comp_zoom.set((z * 1.25).clamp(0.1, 8.0)),
            Action::ZoomOut => self.comp_zoom.set((z / 1.25).clamp(0.1, 8.0)),
            Action::Fit => {
                self.comp_zoom.set(1.0);
                self.comp_pan.set((0.0, 0.0));
            }
            Action::ActualSize => {
                let fit = self.comp_fit.get().max(0.0001);
                self.comp_zoom.set((1.0 / fit).clamp(0.1, 8.0));
                self.comp_pan.set((0.0, 0.0));
            }
            Action::PanUp | Action::PanLeft | Action::PanDown | Action::PanRight => {
                let (sx, sy) = Chrome::wasd_signs(wasd_dir(action));
                let (px, py) = self.comp_pan.get();
                self.comp_pan.set((px + sx * 40.0, py + sy * 40.0));
            }
            _ => handled = false,
        }
        handled
    }

    /// 执行一次悬浮层按钮/箭头/标题栏按钮的动作。调用点保证命中仍成立。
    fn exec(&self, ctx: &mut EventCtx, press: Press) {
        match press {
            Press::Btn(b)
                if self.companion_view_active()
                    && matches!(b, Btn::Out | Btn::In | Btn::Fit | Btn::Actual) =>
            {
                companion_zoom_button(
                    b,
                    &self.chrome,
                    &self.comp_zoom,
                    &self.comp_pan,
                    &self.comp_fit,
                );
            }
            Press::Btn(Btn::FrameBack) if self.companion_owns_index() => {
                self.chrome.slide_stop();
                self.companion.borrow_mut().step_index(-1);
            }
            Press::Btn(Btn::FrameFwd) if self.companion_owns_index() => {
                self.chrome.slide_stop();
                self.companion.borrow_mut().step_index(1);
            }
            // 辅图底栏没有播放钮；真被旧几何命中也不影响主图播放状态。
            Press::Btn(Btn::PlayPause) if self.companion_owns_index() => {
                self.chrome.slide_stop();
            }
            Press::Btn(b) => exec_btn(ctx, &self.viewer, &self.chrome, b),
            Press::Arrow(Arrow::Prev) if self.companion_free() => {
                self.chrome.slide_stop();
                self.companion.borrow_mut().step_file(-1);
            }
            Press::Arrow(Arrow::Next) if self.companion_free() => {
                self.chrome.slide_stop();
                self.companion.borrow_mut().step_file(1);
            }
            Press::Arrow(Arrow::Prev) => {
                self.chrome.slide_stop();
                self.viewer.borrow_mut().step(-1);
            }
            Press::Arrow(Arrow::Next) => {
                self.chrome.slide_stop();
                self.viewer.borrow_mut().step(1);
            }
            Press::TitleBtn(tb) => exec_title_btn(ctx, &self.chrome, &self.config, &self.hub, tb),
        }
        ctx.mark_dirty();
    }

    /// 右键菜单：**文件 → 对画面做事 → 界面 → 设置**四块，顶层 9 项（m327）。
    ///
    /// ```text
    /// 文件 ▸      打开… / 系统菜单 ── 复制×3 ── 属性…
    /// 最近 ▸      最近打开（≤100 条，最新在前；m325）
    /// 排序 ▸      文件名 / 大小 / 创建日期 / 修改日期 ── 倒序（m275）
    /// ─────
    /// 画面 ▸      缩小 / 放大 / 适应窗口 / 1:1 原尺寸 / 全屏 / 旋转 90° / 水平镜像 /
    ///            上一帧 / 暂停·继续动画 / 下一帧 ── 左键拖图 / 幻灯片放映 / 动图/文档播放速度 ▸
    /// 本图 ▸      RAW 参数… / 跳转到…帧/页 / 旋转角度… / 取色器
    /// 插件 ▸      外部进程命令
    /// ─────
    /// 界面 ▸      显示操作按钮 / 极简模式 / 棋盘格背景 / 缩略图条 ▸ / 主题色 ▸ /
    ///            不透明度 ▸（m262 起组内不画分隔线）
    /// 主辅视图 ▸  开关 / 来源 / 联动（无边框形态不出现）
    /// ─────
    /// 设置…
    /// ```
    ///
    /// ★ **m327：14 项 → 9 项，层级没变**。竞品里 12–16 项是常态（gwenview / qView /
    ///   nomacs 都是 16），但它们的顶层大半是**平铺的动作**；我们 14 项里有 8 项是子菜单，
    ///   等于"点什么都得再进一层"。所以这一轮不是再加一层，而是把**散在同一块里的
    ///   单项收进已有的组**。
    /// ★ **两级是硬约束**：本地样本 13 个右键菜单没有一家超过两级（ImageGlass 的三级只
    ///   出现在它的**主菜单**里，右键反而把通道子菜单拍平成一级）。所以「最近 ▸」「排序 ▸」
    ///   必须留顶层——它们本身是子菜单，塞进「文件 ▸」就是三级。
    /// ★ 「排序 ▸」m327 第一版曾拍平进「文件 ▸」，用户实测后要求独立出来，位置定在
    ///   「最近 ▸」下面——两者都是"接下来看哪张"：最近**往回**挑，排序**往前**排。
    ///
    /// 「文件 ▸」= **磁盘层**：点了都不改屏幕上的像素。打开（读盘）、系统菜单（把文件
    /// 交给外壳）、复制（写剪贴板）、属性（读元数据）全在这一条轴上；排序也是磁盘层，
    /// 但 m327 按用户要求独立成顶层一项（见 `sort_menu`）。
    /// ★ 「复制」m327 **拍平**进本组：它当年做成子菜单是为省顶层空间，现在整块已在
    ///   「文件 ▸」里，再套一层只是把一次悬停换成两次点击。
    /// 属性仍在块尾——资源管理器里它永远是最后一项。
    ///
    /// ★ m327：**「系统菜单」「属性…」没有进「本图 ▸」**，两条判据：
    ///   ① 置灰口径——本图四项按"图解没解出来"置灰；系统菜单/属性只要路径在就该能用
    ///      （解码失败、失败占位、文件坏了，属性照样要能看，文件照样要能交给别的程序）。
    ///   ② 轴不同——本图改的是像素与取景；系统菜单把文件交给外壳、属性读磁盘元数据，
    ///      两者都不碰像素。混进去之后这一组就没有一句统一的开关判据了。
    ///
    /// 「本图 ▸」（m276 由「工具 ▸」改名）收的是**只作用于当前这张图**、且**不进浮条**的
    /// 低频功能：改它的裸数据解码参数、在它自己的帧/页里按号定位、换一个取景角度、
    /// 取它的一个像素。与「画面 ▸」的分界不是"对不对这张图做事"，而是**有没有浮条
    /// 按钮**——画面 ▸ 线前 10 项严格等于 `Btn::ALL`，这一组只进菜单、不占浮条空间。
    ///
    /// 「画面 ▸」（m327 由「画面 ▸」改名）：**对屏幕上这块画面做的事**——缩放、适应、
    /// 全屏、旋转、镜像、翻帧，加上"怎么拖、怎么自动翻"两颗。旧名「操作」把"看图"
    /// 说成了"操作软件"，而这十颗都是看一眼就想按的简单动作；「画面」说的是对象，
    /// 不是动作的抽象层级。★ 横线之后的两项没有浮条按钮（浮条几何由 `Btn::ALL` 驱动），
    /// 但它们同属"画面"：拖图是拖画面，放映是让画面自己翻。
    ///
    /// 「界面 ▸」= 原「外观 ▸」（m246）改名：**这一窗显示什么 / 长什么样**，与设置页
    /// 「界面」卡同序。★ 放映与左键拖图 m327 起搬进「画面 ▸」——它们是"对画面做的事"
    /// （动词），不是"显示成什么样"（名词）；搬走之后本组回到纯观感。
    ///
    /// 装配出口会调 [`align_menu_columns`] 把**每一级**的标签左沿对齐：勾选列是
    /// 逐级判定的，纯动作列表（复制 / 插件）不补就会比别的子菜单少缩进一列。
    ///
    /// 「画面」子菜单的前 10 项由工具栏的 [`Btn::ALL`] **自动生成**（见 [`exec_btn`]），
    /// 底部按钮栏有什么、菜单里就有什么，加按钮不会只改一处。
    /// 也因此原来顶层的「全屏 / 棋盘格 / 水平翻转」不再单独占一行——它们在画面里。
    /// ★ 对账口径（2026-11-02 立，m327 修订）：**线前 10 项严格等于浮条**；线后两项
    ///   是浮条放不下的（放映）与不是按钮的（拖图），单独成段，不混进按钮序列。
    fn context_menu(&self, ctx: &mut EventCtx, at: Point) {
        let ws = window_state();
        let has_file = self.viewer.borrow().current_path().is_some();
        let has_image = self.viewer.borrow().image().is_some();

        // ── 四块 · 三条线（m327 重排项序；线的条数一条没动）──
        //
        // 规矩：**每条分隔线都要能说出它切开的两种东西是什么**，说不出来就该删线或归并。
        //
        //   ① 磁盘层      文件 ▸ / 最近 ▸ / 排序 ▸             ← 三件都不改屏幕像素
        //   ── 线①：磁盘 → 像素 ──
        //   ② 对画面做事  画面 ▸ / 本图 ▸ / 插件 ▸
        //   ── 线②：做事 → 状态 ──
        //   ③ 状态开关    视图 ▸ / 主辅视图 ▸                ← 改的是持续态，勾选即是真相
        //   ── 线③：功能与设置入口之间 ──
        //   ④ 设置…
        //
        // ★ 别为"对齐设置页四卡"再加线：那四张卡是**配置**的分类，这里是**动作**的分类，
        //   两套轴不同源。要加项时先答"它属于①②③哪一块"；答不上来，才轮到讨论加线。

        let mut items = Vec::with_capacity(10);
        items.push(MenuItem::submenu(
            "文件",
            self.file_items(has_file, has_image),
        ));
        items.push(self.recent_menu());
        items.push(self.sort_menu());
        items.push(MenuItem::separator());
        items.push(MenuItem::submenu("画面", self.ops_items(ws)));
        items.push(MenuItem::submenu(
            "本图",
            self.current_image_items(has_image),
        ));
        items.push(MenuItem::submenu("插件", self.plugin_items()));
        items.push(MenuItem::separator());
        items.push(MenuItem::submenu("界面", self.appearance_items()));

        // ── 主辅视图（m322）：开关 / 来源 / 联动方式收进子菜单 ──
        // 无边框形态不开放：它没有 chrome/标题栏，分栏与工具栏都无处落，
        // 强行开放只会多一套要适配的布局。
        // ★ m327 起旁边那组已经叫「视图」，这一项仍叫「主辅视图」：两者撞名，
        //   但改名的连带面（文档、设置页、m322 的整套叫法）比省两个字贵；
        //   真要动，等下一次碰主辅视图时一起改。
        if !chrome::borderless() {
            items.push(MenuItem::submenu("主辅视图", self.companion_items()));
        }

        items.push(MenuItem::separator());
        items.push({
            let mut it = MenuItem::run(
                "设置…",
                {
                    let config = self.config.clone();
                    let hub = self.hub.clone();
                    move |ctx| settings::open_settings(ctx, &config, &hub)
                },
                false,
            );
            it.shortcut = self.config.borrow().hotkeys.display(Action::Settings);
            it
        });
        align_menu_columns(&mut items);
        ctx.show_context_menu(at, items);
    }

    /// 「文件 ▸」：磁盘层（m327 由顶层五片叶子 + 一个子菜单合并而来）。
    ///
    /// 判据沿用 m287——**点了不改屏幕上的像素**。打开（读盘）、系统菜单（把文件交给
    /// 外壳）、复制（写剪贴板）、属性（读元数据）都在这一条轴上；改像素的在「画面 ▸」
    /// 「本图 ▸」。排序也是磁盘层，但 m327 按用户要求独立成顶层一项（见 `sort_menu`）。
    ///
    /// ★ 块内顺序：打开 / 系统菜单（两条都是"拿这个文件当对象"，内容一个归我们、
    ///   一个归外壳）→ 复制（写出去）→ 属性（读它是什么）。属性留在块尾，照资源管理器。
    fn file_items(&self, has_file: bool, has_image: bool) -> Vec<MenuItem> {
        let mut out = Vec::with_capacity(12);

        // ── 打开 ──
        out.push({
            let mut it = MenuItem::run(
                "打开…",
                {
                    let v = self.viewer.clone();
                    let cfg = self.config.clone();
                    move |ctx| {
                        let v2 = v.clone();
                        let cfg2 = cfg.clone();
                        // 过滤器 = 完整图片清单 + 容器后缀（zip / cbz）：装了新解码器，
                        // 打开对话框里就多出那些后缀。容器后缀只加在这一处过滤器上，
                        // 不进 `gallery::known_exts()` —— 那份清单还喂文件关联。
                        // `filter` 内部立即把切片转成自有 Vec，传局部量的引用是安全的。
                        let exts = crate::core::archive::pick_exts();
                        ctx.request_pick_file(
                            windui::prelude::PickDialog::new()
                                .title("打开图片")
                                .filter("图片", &exts),
                            move |p| {
                                if let Some(p) = p {
                                    // 裸像素首开自动推断；推不出时 open_path 才压队列由主窗补弹。
                                    super::raw_dialog::open_path(&v2, &cfg2, &p);
                                }
                            },
                        );
                    }
                },
                false,
            );
            it.shortcut = self.config.borrow().hotkeys.display(Action::Open);
            it
        });

        // ── 系统菜单（m313，紧跟「打开…」）──
        //
        // 这一项弹出的**不是我们的菜单**：宿主调 explorer 自己的 `IContextMenu`，
        // 弹出来的是资源管理器里对着这个文件右键的那一份。理由是内容根本不在我们
        // 手里——「打开方式」「发送到」「添加到压缩包」「用杀毒软件扫描」这些动词由
        // 系统与第三方外壳扩展在注册表里登记，装了谁就多几项，换台机器就不一样。
        // 复刻一份注定是"我这台机器上长这样"，所以这里只**转发**：宿主选位置、外壳
        // 出内容、选中哪个由哪个动词自己执行（`platform::shell::FileMenu`）。
        //
        // ★ 与「打开…」的分界不是"都跟打开有关"，而是**谁在做主**：那个是宿主自己的
        //   打开对话框（带我们的过滤器，换来一张图继续看）；这个是**对磁盘上这个
        //   文件做事**（用别的程序打开 / 压缩 / 扫描 / 发到别处）。两者都在磁盘层，
        //   所以紧挨着排；它排在「复制」前头，因为「复制」本身就是系统菜单里的一项。
        //
        // ★ 没有当前文件时置灰（空窗只可能从托盘开出来）：系统菜单是"针对这个文件"
        //   的，没有文件就没有对象，这一项连弹都不该弹。
        out.push(
            MenuItem::run(
                "系统菜单",
                {
                    let v = self.viewer.clone();
                    move |ctx| {
                        let Some(path) = v.borrow().current_path() else {
                            return; // 入口已置灰，这里只是兜底
                        };
                        let Some(hwnd) = windui::platform::native_window_handle() else {
                            ctx.toast_err("系统菜单：取不到窗口句柄");
                            return;
                        };
                        // 一行 info：菜单的内容不归我们，落日志的只有"谁被交给系统了"
                        // （菜单里选了什么由外壳自己执行，宿主看不到也不该看到）。
                        log::info!("系统菜单：{}", path.display());
                        // 两段走：**先装**（只读盘 + 查注册表，失败当场给一句人话），
                        // **后弹**（模态，自己会跑一圈消息循环）。后者必须等这一轮事件
                        // 分发完全返回——在回调里直接弹，菜单的消息循环会重入窗口过程，
                        // 那里再取一次窗口状态就是 `&mut` 别名（windui 铁律，见
                        // `EventCtx::defer_blocking` 的注释）。
                        match crate::platform::shell::FileMenu::for_path(hwnd, &path) {
                            Ok(menu) => ctx.defer_blocking(move || {
                                if let Err(e) = menu.show(hwnd) {
                                    log::warn!("系统菜单：{e}");
                                }
                            }),
                            Err(e) => ctx.toast_err(e),
                        }
                    }
                },
                false,
            )
            .enabled(has_file),
        );

        out.push(MenuItem::separator());

        // ── 复制（m248 起是子菜单，m327 拍平）──
        // 2026-11-03 用户口径「三个复制收纳到"复制"里面，向右弹出，这样比较节省空间」：
        // 同一次只选一种口径（画面 PNG / 文件本身 / 路径文本），平铺会摊成菜单最长的三段。
        // ★ m327 改判：这条口径的前提是"顶层空间不够"。现在整块搬进「文件 ▸」，空间
        //   已经省下来了；再套一层子菜单只会把一次悬停换成两次点击，反而更难选。
        //   快捷键仍挂在「复制当前」与「复制文件路径」上，拍平不影响按键。
        out.append(&mut self.copy_items(has_image, has_file));

        out.push(MenuItem::separator());

        // ── 属性…（m247）：从浮条「!」与「画面 ▸」收编到顶层，m327 起在「文件 ▸」块尾 ──
        // 文件类最后一项：读的是磁盘上这个文件的元数据与 EXIF，不依赖画面解没解出来。
        // 快捷键 Action::Properties 走 exec_action 那条独立路径，行为不变。
        out.push({
            let v = self.viewer.clone();
            let mut it = MenuItem::run("属性…", move |ctx| settings::open_metadata(ctx, &v), false);
            it.shortcut = self.config.borrow().hotkeys.display(Action::Properties);
            it
        });

        // ── 清除本书阅读位置（m333）──
        // 「书」不是「图」：目录 / 压缩包层 / PDF 各是一本。清除后**当前画面不动**，
        // 只是下次重开不再回到这里；`remove_with_state` 还会抑制同状态的防抖回写，
        // 否则刚清完下一拍又被保存回来。
        out.push({
            let v = self.viewer.clone();
            let mut it = MenuItem::run(
                "清除本书阅读位置",
                move |ctx| {
                    let Some(state) = v.borrow().reading_state() else {
                        ctx.toast_err("当前没有可清除的阅读位置");
                        return;
                    };
                    crate::core::reading::remove_with_state(&state);
                    log::info!("清除阅读位置：{}", state.key);
                    ctx.toast_ok("已清除本书阅读位置");
                },
                false,
            );
            it.enabled = has_image;
            it
        });

        out
    }

    /// 「最近 ▸」（m325）：只服务"再打开一次刚才那张"。
    ///
    /// 列表来自 `core::recent`，最多 100 条，最新在前。点一条 = 与「打开…」走同一条
    /// `raw_dialog::open_path`（裸像素也先自动推断）。路径失效时不移出菜单装没看见——
    /// 提示一句并把它从列表里摘掉，下次打开就干净了。
    ///
    /// ★ m327：必须留在**顶层**。它本身是子菜单，收进「文件 ▸」就是三级；样本里
    ///   没有一家右键菜单做到三级。（qView 也是这么放的：菜单栏里 Recents 在 File 下，
    ///   右键里它自己占一行。）
    fn recent_menu(&self) -> MenuItem {
        let recent = crate::core::recent::list();
        let mut sub = Vec::new();
        if recent.is_empty() {
            sub.push(MenuItem::run("（还没有）", |_| {}, false));
        } else {
            for path in &recent {
                let target = path.clone();
                let v = self.viewer.clone();
                let cfg = self.config.clone();
                let label = crate::core::recent::label(path);
                sub.push(MenuItem::run(
                    label,
                    move |ctx| {
                        if !target.is_file() {
                            crate::core::recent::remove(&target);
                            ctx.toast_err(format!(
                                "文件已不存在，已从最近列表移除：{}",
                                target.display()
                            ));
                            ctx.mark_dirty();
                            return;
                        }
                        super::raw_dialog::open_path(&v, &cfg, &target);
                    },
                    false,
                ));
            }
            sub.push(MenuItem::separator());
            sub.push(MenuItem::run(
                "清空最近",
                |ctx| {
                    crate::core::recent::clear();
                    ctx.toast_ok("已清空最近打开");
                    ctx.mark_dirty();
                },
                false,
            ));
        }
        MenuItem::submenu("最近", sub).enabled(!recent.is_empty())
    }

    /// 「排序 ▸」（m275；m327 从「文件 ▸」里独立出来，紧跟「最近 ▸」）。
    ///
    /// ini 键 `sort_by` + `sort_desc`。四项维度 + 一条分隔线 + 倒序：先选按什么排、
    /// 再定方向，与资源管理器「排序方式 ▸」同构（那边也是四个维度 + 一个升降序）。
    ///
    /// ★ m327 第一版曾把它拍平进「文件 ▸」，用户实测后要求独立：「排序改的是下一张是谁，
    ///   值得自己占一行」。位置定在「最近 ▸」下面——两者都是"接下来看哪张"：最近
    ///   **往回**挑，排序**往前**排。
    ///
    /// ★ 与「跳转到…帧/页」是一对，但**分居两处**：排序是**组间**导航（下一张是谁），
    ///   跳转是**组内**导航（这张的第几帧 / 页），后者只属于这张图 ⇒ 留在「本图 ▸」。
    ///   别为了"配对好看"把两者合并到一处。
    ///
    /// 勾选态直接读配置（唯一正本）；改完**当场**重排——`Viewer::sync_sort` 在
    /// 下一次 tick（16ms 内）对齐，重排拿当前这张当锚点，画面不闪、不重解码。
    fn sort_menu(&self) -> MenuItem {
        let cur = self.config.borrow().sort_spec();
        let mut sub = Vec::with_capacity(6);
        for by in SortBy::ALL {
            let cfg = self.config.clone();
            let hub = self.hub.clone();
            sub.push(
                MenuItem::run(
                    by.label(),
                    move |ctx| {
                        persist_session(&cfg, &hub, |c| c.sort_by = by);
                        ctx.mark_dirty();
                    },
                    cur.by == by,
                )
                // 四项都要留勾选列：只给勾着的那项 `.check` 会让整列随选项跳。
                .check(cur.by == by),
            );
        }
        sub.push(MenuItem::separator());
        let cfg = self.config.clone();
        let hub = self.hub.clone();
        sub.push(
            MenuItem::run(
                "倒序",
                move |ctx| {
                    let on = !cfg.borrow().sort_desc;
                    persist_session(&cfg, &hub, |c| c.sort_desc = on);
                    ctx.mark_dirty();
                },
                cur.desc,
            )
            .check(cur.desc),
        );
        MenuItem::submenu("排序", sub)
    }

    /// 「主辅视图 ▸」子菜单（m322）：开关 / 来源 / 辅图翻页 / 交换主辅。
    /// 活动窗不在这里——点击左/右 pane 就直接切，菜单项是重复入口（用户 2026-11-12 拍板删）。
    /// 主图标题与底栏不受影响；这里只改右栏辅图。
    /// 第三参数是 `checked`，不是 `enabled`：动作项一律 `false`，开关项再显式设
    /// `checkable/checked`，禁用项只碰 `enabled`。
    fn companion_items(&self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let on = self.chrome.companion.get();
        items.push({
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "显示辅图",
                move |ctx| {
                    let on = !chrome.companion.get();
                    chrome.companion.set(on);
                    if !on {
                        chrome.active_pane.set(Pane::Primary);
                    }
                    ctx.mark_dirty();
                },
                false,
            );
            it.checkable = true;
            it.checked = on;
            it.shortcut = self.config.borrow().hotkeys.display(Action::Companion);
            it
        });

        let source = self.chrome.companion_source.get();
        let picked = self.chrome.companion_picked.borrow().clone();
        let mut source_items = Vec::new();
        source_items.push({
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "主序列下一张",
                move |ctx| {
                    chrome.companion_source.set(CompanionSource::NextInPrimary);
                    ctx.mark_dirty();
                },
                false,
            );
            it.checkable = true;
            it.checked = source == CompanionSource::NextInPrimary;
            it
        });
        source_items.push({
            let chrome = self.chrome.clone();
            let label = match picked.as_deref() {
                Some(p) => format!("自选辅图…（{}）", companion_path_label(p)),
                None => "自选辅图…".to_string(),
            };
            let mut it = MenuItem::run(
                label,
                move |ctx| {
                    let exts = crate::core::archive::pick_exts();
                    let chrome2 = chrome.clone();
                    ctx.request_pick_file(
                        windui::prelude::PickDialog::new()
                            .title("自选辅图")
                            .filter("图片", &exts),
                        move |p| {
                            if let Some(p) = p {
                                chrome2.companion_source.set(CompanionSource::OwnSequence);
                                *chrome2.companion_picked.borrow_mut() = Some(p);
                                chrome2.companion.set(true);
                            }
                        },
                    );
                },
                false,
            );
            it.checkable = true;
            it.checked = source == CompanionSource::OwnSequence;
            it
        });
        items.push(MenuItem::submenu("辅图来源", source_items));

        let link = self.chrome.companion_link.get();
        let mut flip = Vec::new();
        flip.push({
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "跟随主图",
                move |ctx| {
                    chrome.companion_link.set(LinkMode::Linked);
                    ctx.mark_dirty();
                },
                false,
            );
            it.checkable = true;
            it.checked = link == LinkMode::Linked;
            it
        });
        flip.push({
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "固定不动",
                move |ctx| {
                    chrome.companion_link.set(LinkMode::Pinned);
                    ctx.mark_dirty();
                },
                false,
            );
            it.checkable = true;
            it.checked = link == LinkMode::Pinned;
            it
        });
        flip.push({
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "自由翻页",
                move |ctx| {
                    chrome.companion_link.set(LinkMode::Free);
                    ctx.mark_dirty();
                },
                false,
            );
            it.checkable = true;
            it.checked = link == LinkMode::Free;
            it
        });
        items.push(MenuItem::submenu("辅图翻页", flip));

        items.push({
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "同步缩放/平移",
                move |ctx| {
                    let on = !chrome.companion_sync_view.get();
                    chrome.companion_sync_view.set(on);
                    if on {
                        // 同步视图由主图驱动：活动窗收回主图，辅图只做镜像。
                        chrome.active_pane.set(Pane::Primary);
                    }
                    ctx.mark_dirty();
                },
                false,
            );
            it.checkable = true;
            it.checked = self.chrome.companion_sync_view.get();
            it
        });

        items.push({
            let chrome = self.chrome.clone();
            let can_swap = self.companion.borrow().current_target().is_some();
            let mut it = MenuItem::run(
                "交换主辅",
                move |ctx| {
                    chrome.companion_swap.set(true);
                    ctx.mark_dirty();
                },
                false,
            );
            it.enabled = can_swap;
            it
        });
        items
    }
    /// 交换主辅：把当前辅图变成主图，旧主图钉成辅图。
    ///
    /// 页式文件也允许交换：主图落到辅图当前页，旧主图那一页成为新的辅图页。
    /// 同文件不同页也走同一套（只是主图用 `jump_anim`，不重开文件）。
    fn swap_companion(&self) {
        if !self.chrome.companion.get() {
            return;
        }
        let Some(target) = self.companion.borrow().current_target().cloned() else {
            return;
        };
        let (old, old_page) = {
            let v = self.viewer.borrow();
            let Some(path) = v.current_path() else {
                return;
            };
            (path, v.anim_info().map(|a| a.index).unwrap_or(0))
        };
        let target_page = target.page.unwrap_or(0);
        if old == target.path {
            if old_page == target_page {
                return;
            }
            // 同一文件里的两页：主图按号跳，辅图钉回旧页。
            self.viewer.borrow_mut().jump_anim(target_page);
        } else {
            self.viewer.borrow_mut().open(&target.path);
            if target.page.is_some() {
                // 首帧解码完成后才能按号跳；paint 每帧兑一次。
                self.pending_main_page
                    .set(Some((target.path.clone(), target_page)));
            }
        }
        let sort = self.config.borrow().sort_spec();
        self.companion.borrow_mut().pin_to(&old, sort, old_page);
        self.chrome
            .companion_source
            .set(CompanionSource::OwnSequence);
        *self.chrome.companion_picked.borrow_mut() = Some(old);
        self.chrome.companion_link.set(LinkMode::Pinned);
        self.chrome.active_pane.set(Pane::Primary);
    }

    /// 兑现交换后的“新主图落到第 N 页”：解码器回填总数之前不动，到点后只跳一次。
    fn apply_pending_main_page(&self) {
        let Some((path, page)) = self.pending_main_page.take() else {
            return;
        };
        let mut v = self.viewer.borrow_mut();
        if v.current_path().as_deref() != Some(path.as_path()) {
            self.pending_main_page.set(Some((path, page)));
            return;
        }
        match v.anim_info() {
            None => self.pending_main_page.set(Some((path, page))),
            Some(a) if a.has_index() && page < a.count => {
                v.jump_anim(page);
            }
            Some(_) => {}
        }
    }

    /// 「界面 ▸」：**改变画面上出现什么 / 长什么样**的六项开关（m246 立；m327 起
    /// 放映与拖图搬去「画面 ▸」，本组回到纯观感）。
    ///
    /// ★ m262：**本函数内部不画分隔线**。六项是一条轴（显隐四件 → 整窗观感两件），
    ///   与设置页「界面」卡（卡片里本来也没有分隔线）版式对齐。
    ///
    /// 与设置 → 选项 → 界面卡同一条轴：显示操作按钮（浮条按钮，m263 提到组首 —— 这组里
    /// 最常切的一颗）、极简模式（chrome 显隐）、棋盘格背景、缩略图条（底部常驻区）、
    /// 主题色、不透明度（整窗 alpha）。组内先看使用频次，再排几何尺度。
    /// 全部读写同一份 `Config`，`runtime::apply_session_flags` 每帧把配置推给窗口信号。
    /// 棋盘格只在本菜单与设置页「界面」卡（m247 已从浮条 / 「画面 ▸」撤走）。
    /// ★ m327：放映与左键拖图不在这里——它们是"对画面做的事"（动词），属于「画面 ▸」；
    ///   本组只收"显示成什么样"（名词）。
    /// 不透明度保留自己的档位子菜单：十档铺平会把这里撑成一长列，三级反而好找。
    fn appearance_items(&self) -> Vec<MenuItem> {
        let mut out = Vec::new();

        // ── 显示操作按钮（m245，ini 键 show_toolbar_buttons；m263 提到组首）──
        // 这组里最常切的一颗：右键是「正盯着图顺手切」，先给最常切的那颗。
        // 按钮对应的操作在右键「画面 ▸」里一颗不缺，信息（页码 / 缩放 / 帧号）
        // 却只有浮条这一处载体 ⇒ 只隐按钮、留信息并水平居中。归「界面 ▸」：
        // 改的是画面上出现什么（m246 口径）。
        out.push({
            let on = self.config.borrow().show_toolbar_buttons;
            let cfg = self.config.clone();
            let hub = self.hub.clone();
            let mut it = MenuItem::run(
                "显示操作按钮",
                move |ctx| {
                    let on = !cfg.borrow().show_toolbar_buttons;
                    persist_session(&cfg, &hub, |c| c.show_toolbar_buttons = on);
                    super::chrome::set_toolbar_buttons(on);
                    // 不提示：按钮当场消失/出现，屏幕自己已经说了。
                    ctx.mark_dirty();
                },
                on,
            );
            it.checkable = true;
            it.checked = on;
            it
        });

        // ── 极简模式：勾选即写配置（m330）──
        // 同一个开关也在设置页「界面」卡。m329 之前这里只改 `chrome`，重启就回原样
        // —— 同一个名字两套行为。现在菜单 / 快捷键 / 设置页三处都写同一份
        // `minimal_mode`；`chrome` 只是本窗镜像，当场推一下少等一帧。
        out.push({
            let on = self.config.borrow().minimal_mode;
            let chrome = self.chrome.clone();
            let cfg = self.config.clone();
            let hub = self.hub.clone();
            let mut it = MenuItem::run(
                "极简模式",
                move |ctx| {
                    let on = !cfg.borrow().minimal_mode;
                    chrome.minimal_mode.set(on);
                    persist_session(&cfg, &hub, |c| c.minimal_mode = on);
                    // 不提示：元素当场消失/出现，屏幕自己已经说了（2026-10-16 清理）。
                    ctx.mark_dirty();
                },
                on,
            );
            it.checkable = true;
            it.checked = on;
            it.shortcut = self.config.borrow().hotkeys.display(Action::Minimal);
            it
        });

        // ── 棋盘格背景（m246，ini 键 checker）──
        //
        // m247 起棋盘格只在这里和设置页「界面」卡；浮条与「画面 ▸」里那颗已撤走
        // （显示开关不是操作）。这里带勾选态，读写同一份 `Config`、走 `persist_session`，
        // 并当场推窗口信号，切换当下就重绘。
        out.push({
            let on = self.config.borrow().checker;
            let chrome = self.chrome.clone();
            let cfg = self.config.clone();
            let hub = self.hub.clone();
            let mut it = MenuItem::run(
                "棋盘格背景",
                move |ctx| {
                    let on = !cfg.borrow().checker;
                    chrome.checker.set(on);
                    persist_session(&cfg, &hub, |c| c.checker = on);
                    ctx.mark_dirty();
                },
                on,
            );
            it.checkable = true;
            it.checked = on;
            it.shortcut = self.config.borrow().hotkeys.display(Action::Checker);
            it
        });

        // ── 缩略图条：关 / 下方 / 左侧（m254 三态，m262 与设置页共用一根轴）──
        //
        // ★ m254 之前这里是两个并列勾选，于是"勾了左侧却没开条"是个能点出来的死状态。
        //   位置是**属性**不是第二个开关 ⇒ 三态收进一个子菜单，勾当前项。
        //   m262 起设置页也改成同一个三选一下拉，档位 / 标签 / 翻译全走
        //   `settings::{FILMSTRIP_POS, filmstrip_index, filmstrip_bools}`，不再各写一套。
        //   快捷键 T 仍只切开关（只翻 `filmstrip`），挂在父项上。
        out.push({
            let cur = settings::filmstrip_index(&self.config.borrow());
            let mut items = Vec::new();
            for (idx, label) in settings::FILMSTRIP_POS.iter().enumerate() {
                let chrome = self.chrome.clone();
                let cfg = self.config.clone();
                let hub = self.hub.clone();
                items.push(
                    MenuItem::run(
                        *label,
                        move |ctx| {
                            // 选位置必然把条打开（idx 0 才是关）——死状态从入口上就没法产生。
                            let (film, left) = settings::filmstrip_bools(idx);
                            chrome.filmstrip.set(film);
                            chrome.filmstrip_left.set(left);
                            persist_session(&cfg, &hub, |c| {
                                c.filmstrip = film;
                                c.filmstrip_left = left;
                            });
                            // 不提示：条当场出现/消失/挪边，屏幕自己已经说了。
                            ctx.mark_dirty();
                        },
                        cur == idx,
                    )
                    .check(cur == idx),
                );
            }
            let mut it = MenuItem::submenu("缩略图条", items);
            it.shortcut = self.config.borrow().hotkeys.display(Action::Filmstrip);
            it
        });

        // ── 主题色（m247，ini 键 ui_theme）：与设置页下拉同一份 Config ──
        // runtime::apply_theme 每帧幂等推令牌，写完配置下一帧就换色；
        // persist_session 里 push_to_hub 把设置页下拉同步过来，不需要第二条通道。
        let cur_theme = self.config.borrow().ui_theme;
        let theme_hub = self.hub.clone();
        let mut theme_items = Vec::new();
        for t in crate::core::config::UiTheme::ALL {
            theme_items.push(
                MenuItem::run(
                    t.label(),
                    {
                        let cfg = self.config.clone();
                        let hub = theme_hub.clone();
                        let chrome = self.chrome.clone();
                        move |ctx| {
                            persist_session(&cfg, &hub, |c| c.ui_theme = t);
                            // 令牌必须在这里就换：菜单动作的 ctx 属于“借来的”目标控件，
                            // 菜单关闭那一帧是整窗重绘，若等 16ms 定时器再推令牌，
                            // 用户看到的就是“点完没变、下一次右键才变”。
                            tk::set_theme(tk::Theme::from_config(t));
                            // 按旧主题色预渲染的位图（标题栏图钉）不能跟着换。
                            chrome.invalidate_theme_caches();
                            windui::anim::request_repaint();
                            ctx.mark_dirty_all();
                        }
                    },
                    cur_theme == t,
                )
                .check(cur_theme == t),
            );
        }
        out.push(MenuItem::submenu("主题色", theme_items));

        // ── 不透明度：10%~100%，10% 一档，勾选当前档（ini 键：opacity）──
        let cur = self.config.borrow().opacity;
        let hub = self.hub.clone();
        let mut op_items = Vec::new();
        for pct in (1..=10).map(|i| i * 10) {
            op_items.push(
                MenuItem::run(
                    format!("{pct}%"),
                    {
                        let config = self.config.clone();
                        let hub = hub.clone();
                        move |ctx| {
                            let v = {
                                let mut c = config.borrow_mut();
                                c.opacity = pct;
                                pct
                            };
                            crate::core::config::mark_dirty();
                            settings::push_to_hub(&hub, &config.borrow());
                            ctx.set_window_opacity(v);
                            // 不提示：窗口当场变透，屏幕自己已经说了。
                            ctx.mark_dirty();
                        }
                    },
                    cur == pct,
                )
                .check(cur == pct),
            );
        }
        out.push(MenuItem::submenu("不透明度", op_items));
        // 无边框形态没有 chrome/标题栏/缩略图条：这几个开关改了也看不见，
        // 留在菜单里只会变成“点了没反应”。主题色同样没有承载面。
        if chrome::borderless() {
            out.retain(|it| {
                !matches!(
                    it.label.as_str(),
                    "显示操作按钮" | "极简模式" | "棋盘格背景" | "缩略图条" | "主题色"
                )
            });
        }
        out
    }

    /// 「插件」子菜单：按 `<exe>/plugins/*/manifest.json` 动态生成。
    ///
    /// 只列启用中的命令型插件；编辑/面板型暂未接入，留灰项让用户知道它存在。
    /// `min_host` 不满足的插件照列但置灰并标注原因；正在运行的插件条目就地
    /// 变「取消」，避免用户在 30s 响应超时前干等。
    /// 点击只负责启动后台任务（`extensions::start_command`），结果由 runtime 的
    /// 16ms 轮询取回并弹提示——这里绝不能同步等插件，否则菜单一关窗口就冻住。
    fn plugin_items(&self) -> Vec<MenuItem> {
        let list = crate::extensions::discover_configured(&self.config.borrow());
        if list.is_empty() {
            let mut it = MenuItem::run("未安装插件", |_ctx| {}, false);
            it.enabled = false;
            return vec![it];
        }

        let path = self.viewer.borrow().current_path();
        // ★ 整图**每次建菜单只取一份**，不在下面那个 for 里取：旋转过之后
        //   `whole_image_for_plugin` 要做一次整图像素变换（上限档就是 32 MB），
        //   装在循环里就是"有几枚插件转几遍"——m166 第零格埋的下，这一格收回来。
        //   取不到（切片态 / 屏上没图）就是 `None`：命令型照旧只发路径，
        //   编辑型那一项变灰。
        //
        // ★ **这里不许过 8 MP 闸**（2026-11-07 用户报的 4080×4080 3FR 分析不了）：
        //   "能不能送"的判据只在 `pixels_to_send`，命令型超上限还有抽样那条腿
        //   （声明了 `image.sampled` 就发抽好的）。在菜单上先滤一道，等于把抽样
        //   从界面上剪掉——命令型永远收到 `None`，报出来还是「请求里没有整图像素」
        //   这种指错方向的话（日志里 `reason=no-image`，可图明明就在）。
        //   编辑型要的"超上限就变灰"由它自己那一支判，见下面的 `edit`。
        let whole = self.viewer.borrow().whole_image_for_plugin();
        // 另存 / 还原的两份动作只实现一次：右键菜单、运行参数窗、插件热键三条路共用。
        // 参数窗本身不认识 Viewer / Chrome，所以两颗闭包造在这里（见 [`edit_actions`]）。
        let edit_actions: Option<super::plugin_settings::RunActions> = path
            .as_ref()
            .map(|cur| edit_actions(&self.viewer, &self.chrome, &self.live, cur));
        // `run_ui` 编辑插件把两个出口收进了自己的参数窗，右键不再重复挂。
        let ui_managed = list.iter().any(|e| {
            e.enabled
                && !e.blocked
                && e.manifest.run_ui
                && matches!(e.manifest.kind, crate::core::ext::Kind::Editor)
        });
        let mut out = Vec::new();
        for ext in list.into_iter().filter(|e| e.enabled) {
            let name = ext.manifest.name.clone();
            if ext.blocked {
                // 宿主版本低于 manifest.min_host：可见但不给入口，原因写在标签里。
                let mut it = MenuItem::run(
                    format!("{name}（需宿主 {}+）", ext.manifest.min_host),
                    |_ctx| {},
                    false,
                );
                it.enabled = false;
                out.push(it);
                continue;
            }
            let id = ext.manifest.id.clone();
            if crate::extensions::run::is_running(&id) {
                out.push(MenuItem::run(
                    format!("取消「{name}」"),
                    move |_ctx| {
                        // 取消的可见结果由轮询送回的「已取消」提示承担，这里不另发 toast。
                        crate::extensions::run::cancel(&id);
                    },
                    false,
                ));
                continue;
            }
            if ext.isolated {
                // 三振隔离：可见但不可点，解除入口在设置→插件详情窗。
                let mut it = MenuItem::run(format!("{name}（已隔离）"), |_ctx| {}, false);
                it.enabled = false;
                out.push(it);
                continue;
            }
            match ext.manifest.kind {
                crate::core::ext::Kind::Panel => {
                    // 宿主托管面板（P0.1b-4 的面板腿）需要真实用例才有接口可验，
                    // 先留灰项让用户知道它存在。
                    let mut it = MenuItem::run(format!("{name}（暂不支持）"), |_ctx| {}, false);
                    it.enabled = false;
                    out.push(it);
                }
                crate::core::ext::Kind::Editor => {
                    // 声明 `run_ui` 的插件：点菜单弹宿主渲染的参数窗。
                    // 有 `preview` 声明 = 拖动时看图窗本尊实时变、不起进程；
                    // 老插件（无声明）仍按「应用」起进程，加法式字段不翻旧账。
                    let run_ui = ext.manifest.run_ui && !ext.manifest.settings.is_empty();
                    let has_live_op = run_ui && ext.manifest.preview.is_some();
                    // ★ 8 MP 闸只为「整图经共享内存发给插件进程」（32 MB 映射）而设。
                    //   实时型只改显示层、一个像素都不出进程，拿这道闸拦它会让
                    //   4080×4080 这类图整个菜单项灰掉——2026-11-06 用户报的
                    //   「调完就变灰、无法继续调整」正是这道闸错位：整图与上屏位图
                    //   不是一回事，实时调色只吃上屏那一份。
                    // 编辑型**必须**整图、且**必须**在上限内：回写是同尺寸整档，
                    // 抽样那一套只服务"只读统计"，编辑没有抽样这回事。
                    let edit = whole
                        .as_ref()
                        .filter(|x| crate::extensions::run::pixels_within_limit(x.width, x.height))
                        .cloned();
                    if !has_live_op && edit.is_none() {
                        // 上限数字从 `EDITOR_MAX_PIXELS` 算，不写死：写死的那一份没有任何
                        // 机器读过，改数时只会留下菜单在撒谎（run.rs 的报错串本来就是算的）。
                        let cap = crate::extensions::run::EDITOR_MAX_PIXELS / 1_000_000;
                        let mut it =
                            MenuItem::run(format!("{name}（需整图 ≤{cap}MP）"), |_ctx| {}, false);
                        it.enabled = false;
                        out.push(it);
                        continue;
                    }
                    let ext_ui = ext.clone();
                    let ext_launch = ext.clone();
                    let edit_launch = edit.clone();
                    let launch: Rc<dyn Fn(&mut EventCtx) -> bool> = Rc::new(move |ctx| {
                        // 实时型不该走到这里；真走到了说明有入口误接「应用」，
                        // 如实拒绝，别拿一张空图去起插件进程。
                        let Some(edit) = edit_launch.clone() else {
                            ctx.toast_err("这张图超过 8 MP，整图编辑不可用");
                            return false;
                        };
                        match crate::extensions::start_editor(ext_launch.clone(), edit) {
                            // 成功不弹 toast：提示与关窗抢同一帧是「秒退」最可疑的一环，
                            // 而且图变了本身就是最好的回执。失败留在窗里可重试。
                            Ok(()) => true,
                            Err(e) => {
                                ctx.toast_err(e);
                                false
                            }
                        }
                    });
                    let launch_menu = launch.clone();
                    if run_ui {
                        let cfg = self.config.clone();
                        let actions = edit_actions.clone();
                        let live = self.live.clone();
                        let cur_path = path.clone().unwrap_or_default();
                        let legacy_launch = launch.clone();
                        out.push(MenuItem::run(
                            name.clone(),
                            move |ctx| {
                                super::plugin_settings::show_run_prompt(
                                    ctx,
                                    &ext_ui,
                                    &cfg,
                                    RunPrompt {
                                        state: live.clone(),
                                        path: cur_path.clone(),
                                        on_apply: if has_live_op {
                                            None
                                        } else {
                                            Some(legacy_launch.clone())
                                        },
                                        actions: actions.clone(),
                                    },
                                );
                            },
                            false,
                        ));
                    } else {
                        out.push(MenuItem::run(
                            name.clone(),
                            move |ctx| {
                                launch_menu(ctx);
                            },
                            false,
                        ));
                    }
                }
                crate::core::ext::Kind::Command => {
                    let needs_image = ext
                        .manifest
                        .has_capability(crate::core::ext::Capability::ImageRead);
                    let can_run = !needs_image || path.is_some();
                    // 能力最小化：没声明 image.read 的插件连路径都不发
                    //（echo 这类纯诊断插件不该看见用户在看什么）。
                    let img = if needs_image { path.clone() } else { None };
                    // 像素同样按能力分流——判据写在 `start_command` 里（那枚插件是不是
                    // image.read 声明者），不在 UI 再抄一遍；这里只管"有没有这一份"。
                    let pixels = whole.clone();
                    // 菜单尾随列：插件自报的热键（动态热键的可见面）。
                    // 闭包会把 `ext` 搬走，所以这行要在搬之前取好。
                    let hotkey_label = ext.manifest.hotkey.map(|h| h.pretty());
                    let run_ui = ext.manifest.run_ui && !ext.manifest.settings.is_empty();
                    let ext_ui = ext.clone();
                    let launch: Rc<dyn Fn(&mut EventCtx) -> bool> = Rc::new(move |ctx| {
                        match crate::extensions::start_command(
                            ext.clone(),
                            img.clone(),
                            None,
                            pixels.clone(),
                        ) {
                            Ok(()) => true,
                            Err(e) => {
                                ctx.toast_err(e);
                                false
                            }
                        }
                    });
                    let launch_menu = launch.clone();
                    let mut it = if run_ui {
                        let cfg = self.config.clone();
                        MenuItem::run(
                            name.clone(),
                            move |ctx| {
                                super::plugin_settings::show_run_prompt(
                                    ctx,
                                    &ext_ui,
                                    &cfg,
                                    RunPrompt {
                                        state: LiveState::new(),
                                        path: std::path::PathBuf::new(),
                                        on_apply: Some(launch.clone()),
                                        actions: None,
                                    },
                                );
                            },
                            false,
                        )
                    } else {
                        MenuItem::run(
                            name.clone(),
                            move |ctx| {
                                launch_menu(ctx);
                            },
                            false,
                        )
                    };
                    it.enabled = can_run;
                    it.shortcut = hotkey_label;
                    out.push(it);
                }
            }
        }
        // 当前图有未保存的编辑覆盖：给出另存/还原（编辑结果只存内存）。
        // ★ `run_ui` 编辑插件的这两个出口已经收进它自己的参数窗，右键不再重复挂
        //   —— 用户 2026-11-06 裁决「确定应用后多出来这两个，是错的」。
        if let Some(cur) = self.viewer.borrow().current_path() {
            let edited = self.viewer.borrow().is_edited(&cur);
            if edited && !ui_managed {
                if let Some(a) = &edit_actions {
                    let save = a.save.clone();
                    let revert = a.revert.clone();
                    out.push(MenuItem::run("另存为（PNG）…", move |ctx| save(ctx), false));
                    out.push(MenuItem::run("还原", move |ctx| revert(ctx), false));
                }
            }
        }
        if out.is_empty() {
            let mut it = MenuItem::run("无可用插件", |_ctx| {}, false);
            it.enabled = false;
            out.push(it);
        }
        out
    }

    /// 「本图 ▸」子菜单：三项都只作用于当前这张图，且都不进浮条。
    ///
    /// m276 由「工具 ▸」改名并收编两项；前身是 m217 只有取色器一项的「工具 ▸」。
    ///
    /// 两个"打开一个窗"的常驻项在前、按原本在顶层的先后排（RAW 参数 → 跳转），
    /// 会话开关「取色器」垫底；中间一条分隔线把"改这张图的参数 / 定位到这张图的
    /// 某一帧页"与"切换一种指针模式"分开，三者不是同一种动作。
    ///
    /// 前两项**常驻 + 置灰**：入口永远找得到（用户不必先猜"这功能在不在"），
    /// 但静态图上点不开。跳转判据是 `count > 1` 而不是 `has_index()`——单帧动图
    /// （`delay>0` 但只有一帧）没有第二张可去；RAW 参数判据是"无头裸像素"，
    /// 相机 RAW 有完整文件头，参数窗对它们没有意义。
    ///
    /// 取色器是**会话临时态**（不像极简模式那样落盘）：开关与指针位置都在 `Chrome`，
    /// 菜单回调、画布事件、paint 三者读同一份；切换时清掉旧指针位置，免得下次
    /// 一开模式就有一枚停在别处的浮层。没有图时置灰——空窗没有像素可采样。
    fn current_image_items(&self, has_image: bool) -> Vec<MenuItem> {
        let mut out = Vec::with_capacity(4);

        // ── RAW 参数…：从顶层文件类收编（m276）──
        {
            let path = self.viewer.borrow().current_path();
            let enabled = super::raw_dialog::menu_enabled(path.as_deref());
            let v = self.viewer.clone();
            let cfg = self.config.clone();
            let mut it = MenuItem::run(
                "RAW 参数…",
                move |ctx| super::raw_dialog::open_current_params(ctx, &v, &cfg),
                false,
            );
            it.enabled = enabled;
            out.push(it);
        }

        // ── 跳转到…帧/页：从顶层浏览区收编（m276 改名）──
        // 名字里写明"帧/页"，免得被当成"跳到清单里第几张"——那是对一组图做的事，
        // 归「排序 ▸」与前后翻页；这一项只在同一张图的帧/页之间挪。
        {
            let comp_active =
                self.chrome.companion.get() && self.chrome.active_pane.get() == Pane::Companion;
            let count = if comp_active {
                self.companion.borrow().anim_info().map(|a| a.count)
            } else {
                self.viewer.borrow().anim_info().map(|a| a.count)
            };
            let v = self.viewer.clone();
            let comp = self.companion.clone();
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "跳转到…帧/页",
                move |ctx| {
                    if chrome.companion.get() && chrome.active_pane.get() == Pane::Companion {
                        super::goto_dialog::open_companion(ctx, &v, &comp);
                    } else {
                        super::goto_dialog::open(ctx, &v);
                    }
                },
                false,
            );
            it.enabled = count.is_some_and(|c| c > 1);
            out.push(it);
        }

        // ── 旋转角度…（m280）：整数度数 + ± 微调，输入即变 ──
        // 与上面两项同属「打开一个关于这张图的窗」，排在分隔线之前；
        // 不进浮条也不进「画面 ▸」（那一组线前 10 项严格等于 `Btn::ALL`，m247 铁律）。
        {
            let chrome = self.chrome.clone();
            let v = self.viewer.clone();
            let mut it = MenuItem::run(
                "旋转角度…",
                move |ctx| super::rotate_dialog::open(ctx, &chrome, &v),
                false,
            );
            it.enabled = has_image;
            out.push(it);
        }

        out.push(MenuItem::separator());

        // ── 取色器（m217）：本组原有的一项 ──
        {
            let on = self.chrome.picker.get();
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "取色器",
                move |ctx| {
                    chrome.toggle_picker();
                    ctx.mark_dirty();
                },
                on,
            );
            it.checkable = true;
            it.checked = on;
            it.enabled = has_image;
            it.shortcut = self.config.borrow().hotkeys.display(Action::Picker);
            out.push(it);
        }
        out
    }

    fn copy_items(&self, has_image: bool, has_file: bool) -> Vec<MenuItem> {
        let mut out = Vec::with_capacity(3);
        out.push({
            let mut it = MenuItem::run(
                "复制当前（PNG）",
                {
                    let v = self.viewer.clone();
                    let chrome = self.chrome.clone();
                    move |ctx| match copy_display(&chrome, &v) {
                        CopyNow::Nothing => {}
                        CopyNow::Sync(Ok(())) => ctx.toast_ok("当前画面已复制（PNG）"),
                        CopyNow::Sync(Err(e)) => ctx.toast_err(e),
                        CopyNow::Posted(note) => ctx.toast_ok(note),
                        CopyNow::Busy => ctx.toast_err("上一次复制还在进行，等它结束再按"),
                        CopyNow::SpawnFailed(e) => ctx.toast_err(e),
                    }
                },
                false,
            );
            it.enabled = has_image;
            it.shortcut = self.config.borrow().hotkeys.display(Action::CopyImage);
            it
        });
        out.push({
            let mut it = MenuItem::run(
                "复制原文件",
                {
                    let v = self.viewer.clone();
                    move |ctx| {
                        // 复制磁盘上的文件本身（CF_HDROP）：粘贴到资源管理器就是原文件。
                        // 不重解码——大图不会因为这一下卡住 UI。
                        let path = v.borrow().current_path();
                        if let Some(p) = path {
                            match clipboard::copy_file(&p) {
                                Ok(()) => ctx.toast_ok("原文件已复制到剪贴板"),
                                Err(e) => ctx.toast_err(e),
                            }
                        }
                    }
                },
                false,
            );
            it.enabled = has_file;
            it
        });
        out.push({
            let mut it = MenuItem::run(
                "复制文件路径",
                {
                    let v = self.viewer.clone();
                    move |ctx| {
                        let path = v.borrow().current_path();
                        if let Some(p) = path {
                            // 回执说话：占用剪贴板时报错，而不是无条件说"已复制"（O7）。
                            match clipboard::copy_text(&p.to_string_lossy()) {
                                Ok(()) => ctx.toast_ok("路径已复制到剪贴板"),
                                Err(e) => ctx.toast_err(e),
                            }
                        }
                    }
                },
                false,
            );
            it.enabled = has_file;
            it.shortcut = self.config.borrow().hotkeys.display(Action::CopyPath);
            it
        });
        out
    }

    fn ops_items(&self, ws: WindowState) -> Vec<MenuItem> {
        // ★ 菜单**恒列** `Btn::ALL`：浮条为了美观把没有帧/页号的播放组藏起来，
        //   菜单不需要美观，缺项只会让人以为功能被删了（用户 2026-11-06）。
        //   从 Viewer 现算，不读 `chrome.anim`（那是绘制时回灌的 Cell，可能慢一帧）。
        let has_index = self.viewer.borrow().has_index();
        let mut out = Vec::with_capacity(Btn::ALL.len());
        for (b, enabled) in ops_row(has_index) {
            let mut it = MenuItem::run(
                b.name(),
                {
                    let v = self.viewer.clone();
                    let chrome = self.chrome.clone();
                    let zoom = self.comp_zoom.clone();
                    let pan = self.comp_pan.clone();
                    let fit = self.comp_fit.clone();
                    move |ctx| {
                        if !companion_zoom_button(b, &chrome, &zoom, &pan, &fit) {
                            exec_btn(ctx, &v, &chrome, b);
                        }
                        ctx.mark_dirty();
                    }
                },
                false,
            );
            it.enabled = enabled;
            if let Some(sc) = b.shortcut(&self.config.borrow().hotkeys) {
                it.shortcut = Some(sc);
            }
            // 开关类给勾选列：m247 后只剩全屏这一颗带状态。
            let state = if b == Btn::Fullscreen {
                Some(ws.fullscreen)
            } else {
                None
            };
            if let Some(on) = state {
                it.checkable = true;
                it.checked = on;
            }
            out.push(it);
        }
        // ── 线后两项：没有浮条按钮，但同属"对画面做的事"（m327）──
        //
        // ★ 线前 10 项**严格等于** `Btn::ALL`（同一份正本、同一顺序）；这条线之后才是
        //   "菜单独有"的部分。对账时按"线前 = 浮条"读，不会混。
        //   菜单恒列 `Btn::ALL` 全量；浮条另按 `Chrome::row` 裁剪，两者允许不一致——
        //   差集只是"变灰"，不是"消失"。
        // ★ 为什么放在这里：拖图是拖画面（只改视口），放映是让画面自己翻；两项都是
        //   "对画面做的事"，不是"显示成什么样"。它们曾经和显示开关挤在同一组，
        //   用户实测后指出"怪怪的"——那是名词/动词混在一组，不是位置不对。
        out.push(MenuItem::separator());

        // ── 左键拖图：指针交互的开关（ini 键：drag_image）──
        // 2026-11-03 用户点名进菜单。理由成立：这条改的是手感，而手感不对的时候人正
        // 盯着画面——不该要先开设置页才能试另一种分工。
        // 勾选态**直接读 `config.drag_image`**，不像 `filmstrip` 那样另存一份会话位：
        // canvas 每次按下都现读配置，多存一份就是第二正本（还会漂）。
        // 不标脏：切换不改这一帧的任何像素，下一次按下自然按新分工走。
        out.push({
            let on = self.config.borrow().drag_image;
            let cfg = self.config.clone();
            let hub = self.hub.clone();
            let mut it = MenuItem::run(
                "左键拖图",
                move |_ctx| {
                    let on = !cfg.borrow().drag_image;
                    persist_session(&cfg, &hub, |c| c.drag_image = on);
                },
                on,
            );
            it.checkable = true;
            it.checked = on;
            it
        });

        // ── 幻灯片放映：名字随状态换（放映中读「停止放映」），与 `Btn::PlayPause` 同规矩 ──
        //
        // 它不在 `Btn::ALL` 里（浮条上没有这一颗：MDL2 的 PUA 字形是赌局，而浮条几何由
        // `Btn::ALL` 驱动，多一颗要连动命中与窄窗排布），因此只能作为菜单项存在；
        // 行为仍走 `exec_action` 那唯一入口——键与菜单同一份代码，不存在"菜单里有、点了没反应"。
        out.push({
            let mut it = MenuItem::run(
                if self.chrome.slideshow_on() {
                    "停止放映"
                } else {
                    "幻灯片放映"
                },
                {
                    let v = self.viewer.clone();
                    let chrome = self.chrome.clone();
                    let cfg = self.config.clone();
                    let hub = self.hub.clone();
                    move |ctx| {
                        exec_action(
                            Some(ctx),
                            &v,
                            &chrome,
                            &cfg,
                            &hub,
                            crate::core::hotkey::Action::Slideshow,
                        );
                        ctx.mark_dirty();
                    }
                },
                false,
            );
            it.shortcut = self
                .config
                .borrow()
                .hotkeys
                .display(crate::core::hotkey::Action::Slideshow);
            it
        });

        // ── 动图/文档播放速度 ▸（m332 建，m335 改名）──
        // 动图切帧与分页文档切页共用 `viewer::anim_wait` 这一个折算点，所以两者都
        // 受倍速影响；只有静图 / 单页（没有可推进的延时）才置灰。
        // ★ 与设置页「看图 → 动图/文档播放速度」写同一份 `Config.anim_speed`，重启后保留；
        //   改完只重排当前帧之后的时刻，不重解当前帧。
        out.push({
            let info = self.viewer.borrow().anim_info();
            let enabled = info.is_some_and(|a| a.delay_ms > 0);
            let cur = self.config.borrow().anim_speed;
            let mut sub = Vec::with_capacity(AnimSpeed::ALL.len());
            for speed in AnimSpeed::ALL {
                let cfg = self.config.clone();
                let hub = self.hub.clone();
                let checked = cur == speed;
                sub.push(
                    MenuItem::run(
                        speed.label(),
                        move |ctx| {
                            persist_session(&cfg, &hub, |c| c.anim_speed = speed);
                            ctx.mark_dirty();
                        },
                        checked,
                    )
                    .check(checked),
                );
            }
            let mut it = MenuItem::submenu("动图/文档播放速度", sub);
            it.enabled = enabled;
            it
        });

        out
    }
}

/// 「画面 ▸」的项序与启用态：菜单**恒列** [`Btn::ALL`]，不随窗口宽或播放组显隐删项；
/// 只有当前图没有帧/页号时，播放组那三颗变灰（`enabled=false`，框架保证不可点击）。
///
/// ★ 与 [`Chrome::row`] 的分工：浮条按美观裁剪，菜单按"功能永远找得到"列全。
///   两边读同一份 `has_index()`，表现不同是刻意的产品裁决（用户 2026-11-06）。
fn ops_row(has_index: bool) -> Vec<(Btn, bool)> {
    Btn::ALL
        .iter()
        .copied()
        .map(|b| (b, !b.is_anim() || has_index))
        .collect()
}

/// 让右键菜单**每一级**都留出同一条勾选列，标签左沿才对得齐。
///
/// windui 的 `level_width` 逐级判 `items.iter().any(|it| it.checkable)`：本级有开关项
/// 才让出 `MENU_CHECK_W + MENU_GAP`（24dp），没有就贴左。于是「画面」「本图」「界面」
/// 「不透明度」这些含开关的级会缩进，而「复制」「插件」这类纯动作列表不会——同一棵菜单里
/// 文字左沿差一列，看起来就是插件名"没留间隔"。
/// 补位递归到每一级，包括「外观 ▸ 不透明度 ▸ 档位」这样的三级。
///
/// 装配出口统一补位：本级没有开关项时，给第一个非分隔线项声明 `checkable`，
/// `checked` 保持 false（绘制只在 `checked` 时画勾，不会多出一枚没来由的对勾）。
/// `checkable` 在框架里只是布局与画勾的标记：`is_actionable` 不看它，点击也只执行
/// 项自己的 `action`、不会自动翻转勾选态，所以补位不改变任何一项的行为。
/// 以后新增子菜单也不必各自记得这件事，见 `tests::menu_columns_aligned_on_every_level`。
fn align_menu_columns(level: &mut [MenuItem]) {
    for it in level.iter_mut() {
        if !it.submenu.is_empty() {
            align_menu_columns(&mut it.submenu);
        }
    }
    if level.iter().any(|it| it.checkable) {
        return;
    }
    let Some(first) = level.iter_mut().find(|it| !it.separator) else {
        return;
    };
    first.checkable = true;
}

/// 活动辅图的缩放按钮（工具栏与右键「画面 ▸」共用）。
/// 返回 `true` = 这一颗归辅图，调用方不要再转给主图。
fn companion_zoom_button(
    b: Btn,
    chrome: &Chrome,
    zoom: &Cell<f32>,
    pan: &Cell<(f32, f32)>,
    fit: &Cell<f32>,
) -> bool {
    if chrome::borderless()
        || chrome.companion_sync_view.get()
        || !chrome.companion.get()
        || chrome.active_pane.get() != Pane::Companion
    {
        return false;
    }
    if !matches!(b, Btn::Out | Btn::In | Btn::Fit | Btn::Actual) {
        return false;
    }
    chrome.slide_stop();
    let z = zoom.get();
    match b {
        Btn::Out => zoom.set((z / 1.25).clamp(0.1, 8.0)),
        Btn::In => zoom.set((z * 1.25).clamp(0.1, 8.0)),
        Btn::Fit => {
            zoom.set(1.0);
            pan.set((0.0, 0.0));
        }
        Btn::Actual => {
            let fit = fit.get().max(0.0001);
            zoom.set((1.0 / fit).clamp(0.1, 8.0));
            pan.set((0.0, 0.0));
        }
        _ => {}
    }
    true
}

/// 执行一次工具栏按钮的动作：**浮条点击与右键菜单「画面」子菜单共用的唯一入口**。
///
/// 以前菜单项各自抄一份行为，加一个按钮要改两处、漏一处就变成"菜单里有这一项，
/// 点了却没反应"。现在两侧都走这里，加按钮只要动 [`Btn`] 一处。
/// 会话态开关的统一写回点（m53-L）：配置是唯一真值，先写它再推设置页，
/// runtime 每帧把配置对齐回 `Chrome`。
///
/// 为什么不在各处直接 `chrome.xxx.set()`：设置窗开着时它的复选框每帧
/// `Handles::sync` 会把信号值写进配置——菜单改了 `Chrome` 却没改配置，
/// 下一帧就被旧复选框覆写回去，用户会看到"点了又弹回来"。
fn persist_session(cfg: &Rc<RefCell<Config>>, hub: &Hub, apply: impl FnOnce(&mut Config)) {
    {
        let mut c = cfg.borrow_mut();
        apply(&mut c);
    }
    crate::core::config::mark_dirty();
    settings::push_to_hub(hub, &cfg.borrow());
}

fn exec_btn(ctx: &mut EventCtx, viewer: &Rc<RefCell<Viewer>>, chrome: &Rc<Chrome>, b: Btn) {
    // 手动按浮条任何一颗都算"用户插手了画面"，幻灯片当场让位（见 `Chrome::slide_stop`）。
    chrome.slide_stop();
    // 全屏要改窗口：`toggle_fullscreen` 同步改窗口尺寸 →
    // resize/paint 被就地重放一遍 → 那里再 `viewer.borrow()`，而下面那行正握着 `borrow_mut`
    // → 同线程重入直接 panic（UI 线程没了 = 整个查看器没了）。它压根不需要 `Viewer`。
    if b == Btn::Fullscreen {
        ctx.toggle_fullscreen();
        return;
    }
    let mut v = viewer.borrow_mut();
    match b {
        Btn::Out => {
            let z = v.scale() / 1.25;
            v.zoom_at_center(z);
        }
        Btn::In => {
            let z = v.scale() * 1.25;
            v.zoom_at_center(z);
        }
        Btn::Fit => v.set_fit(true),
        Btn::Actual => v.zoom_actual(),
        // 动画三颗调的就是 `,` `.` `P` 那三个动作调的同一批 `Viewer` 方法，
        // 不存在"键会自动暂停、按钮不会"这种半截实现。
        Btn::FrameBack => {
            v.step_anim(-1);
        }
        Btn::FrameFwd => {
            v.step_anim(1);
        }
        Btn::PlayPause => v.toggle_anim_pause(),
        // Fullscreen 在函数开头就分流走了（不许带着 viewer 的可变借用车窗动作）。
        // **什么都不做**：这里放 `unreachable!` 等于把
        // "上游改了分流、这里没同步"这种看得见的小 bug 升级成整个查看器没了。
        Btn::Fullscreen => {}
        Btn::Rotate => {
            v.rotate_cw();
        }
        Btn::Mirror => {
            v.toggle_flip();
        }
    }
}

fn exec_title_btn(
    ctx: &mut EventCtx,
    chrome: &Rc<Chrome>,
    cfg: &Rc<RefCell<Config>>,
    hub: &Hub,
    tb: TitleBtn,
) {
    // 锁定纵深防御：hit 已只认锁钮，这里再拦一道——自绘 × 走
    // `ctx.request_close()` 会绕过 `on_close_request`，必须自拦。
    if chrome.locked.get() && tb != TitleBtn::Lock {
        return;
    }
    match tb {
        TitleBtn::Minimize => ctx.minimize(),
        TitleBtn::Maximize => ctx.toggle_maximize(),
        TitleBtn::Close => ctx.request_close(),
        // 读写 ws.topmost（窗口样式位真值）——右键菜单入口已删，本钮是唯一开关。
        // ★ 显式标脏：置顶是**唯一不会引起系统级重排**的窗口钮（最小化/最大化/关闭
        //   都由系统改尺寸或显隐，重绘自然跟着来），不标这一下，本轮分发结束后画布
        //   没有重绘理由。用户报过"点了不刷新、移开鼠标才变"，指的就是这一档。
        TitleBtn::Topmost => {
            // m330：与设置页「窗口置顶」写同一份 `stay_on_top`——不再有"只改本窗"的
            // 第二正本。真正的窗口样式由 runtime 的 `applied_stay` 差分下发（16ms 内）。
            let on = !cfg.borrow().stay_on_top;
            persist_session(cfg, hub, |c| c.stay_on_top = on);
            ctx.mark_dirty();
        }
        TitleBtn::Lock => {
            let on = !chrome.locked.get();
            chrome.locked.set(on);
            persist_session(cfg, hub, |c| c.locked = on);
            // 锁定瞬间清掉按下残留；hover 留着（指针还在锁钮上才有反馈）。
            chrome.pressed_title_btn.set(None);
            ctx.mark_dirty();
        }
    }
}

/// 缩略图条占掉一条边之后留给画面的那块地（`last_work` 的正本）。
///
/// ★ 与 `filmstrip::layout` 的 `Placement.view` 是**同一份 `content` 的两种推导**：
/// 这里减掉条、那里画条。贴左时两边都必须从 `content.x` 起算——若一边拿 `work`
/// 再扣一次，条就会画进画面里、或者画面被扣掉两条宽。
///
/// m300：左边又多了一条文件树。★ 树在**最外侧**（`content.x` 起），条在树右边，
/// 所以两者共用 [`strip_area`] 作起点——谁再各自从 `content` 扣一遍，条就会压到树上。
fn work_of(content: Rect, side: Option<filmstrip::Side>, tree_w: i32) -> Rect {
    let area = strip_area(content, tree_w);
    let (mut x, y, mut w, mut h) = (area.x, area.y, area.w, area.h);
    match side {
        Some(filmstrip::Side::Bottom) => h = (h as f32 - filmstrip::STRIP_H).max(1.0) as i32,
        Some(filmstrip::Side::Left) => {
            x += filmstrip::STRIP_W as i32;
            w = (w as f32 - filmstrip::STRIP_W).max(1.0) as i32;
        }
        None => {}
    }
    Rect::new(x, y, w, h)
}

/// 辅图解码框：`pane` 尺寸 × `detail`，再按单边 / 总像素上限**等比**缩。
///
/// 等比是关键：两根轴各自 clamps 会改变长宽比，解码器再按 box 适配一次，
/// 屏幕上看到的比例没错，但白烧一块面积。`detail` 已由调用方做过下限/上限。
fn companion_decode_box(pane: Rect, detail: f32) -> (u32, u32) {
    let mut w = (pane.w as f32 * detail).ceil().max(1.0);
    let mut h = (pane.h as f32 * detail).ceil().max(1.0);
    let side = w.max(h);
    if side > COMPANION_MAX_SIDE {
        let k = COMPANION_MAX_SIDE / side;
        w *= k;
        h *= k;
    }
    let px = w * h;
    if px > COMPANION_MAX_PIXELS {
        let k = (COMPANION_MAX_PIXELS / px).sqrt();
        // ★ 这里用 floor 不用 ceil：乘完已经贴着上限，再向上取整就会多出一行/一列，
        //   总像素闸当场失效（测试 `companion_decode_box_caps_keep_aspect...` 钉着）。
        w = (w * k).floor().max(1.0);
        h = (h * k).floor().max(1.0);
    }
    (w as u32, h as u32)
}

/// 扣掉文件树之后、留给“缩略图条 + 画面”的那块宽度。
///
/// 树不显示时 `tree_w = 0`，逐字节等于 m300 之前的行为。
fn strip_area(content: Rect, tree_w: i32) -> Rect {
    Rect::new(
        content.x + tree_w,
        content.y,
        (content.w - tree_w).max(1),
        content.h,
    )
}

impl Widget for ImageCanvas {
    fn measure(&self, _avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        // 由父容器的 `fill()` 决定尺寸，控件本身不贡献固有尺寸。
        Size::ZERO
    }

    fn paint(
        &self,
        _bounds: Rect,
        content: Rect,
        _focused: bool,
        _enabled: bool,
        canvas: &mut dyn Canvas,
        _style: &Style,
    ) {
        self.last.set(content);
        // 交换主辅请求（菜单置位）：在读取任何几何之前换掉主图，避免本帧用旧路径。
        if self.chrome.companion_swap.replace(false) {
            self.swap_companion();
        }
        // 交换到页式文件时，等首帧回填总数后落到辅图那一页（只跳一次）。
        self.apply_pending_main_page();
        self.comp_work.set(Rect::new(0, 0, 0, 0));
        self.pane_geom.companion.set(Rect::new(0, 0, 0, 0));
        // 缩略图条占掉一条边（它是常驻的地，不是浮层）：摆图、悬浮层、命中测试
        // 一律用扣掉之后的 `work`——**画在哪就要点在哪**，三者共用一份几何。
        // m253：贴底扣高、贴左扣宽（`filmstrip_left`），两根主轴共用一套格子。
        // ── 文件树（m300）：只在压缩包层里出现，永远贴最外侧 ──
        //
        // 树是**整包一棵**（一次建全，见 `core::archive`），同一包的任意层共用 ⇒ 只有
        // 换层 / 换文件才重读。★ 不能每帧调 `tree_of`：它要读 `layer.tsv` 才知道
        // "这一层是不是包层"，那是每帧一次文件系统调用。
        let gallery_dir = self.viewer.borrow().gallery_dir().map(Path::to_path_buf);
        let reload = self.tree_for.borrow().as_deref() != gallery_dir.as_deref();
        if reload {
            // 一次拿到「这棵树」与「当前层在哪」，两处都只在换层 / 换文件时算。
            let (nodes, cur) = match gallery_dir.as_deref() {
                Some(d) => match crate::core::archive::here(d) {
                    Some((_, segs)) => (crate::core::archive::tree_of(d), Some(segs)),
                    // 普通目录：不是包层，没有树。
                    None => (Vec::new(), None),
                },
                None => (Vec::new(), None),
            };
            *self.tree_nodes.borrow_mut() = nodes;
            // ★ m306：截断标记与上面那批节点同源，同一趟 reload 一起读走。
            //   `tree_truncated` 对普通目录返回 false（`session_of` 直接 None）。
            self.tree_truncated.set(
                gallery_dir
                    .as_deref()
                    .is_some_and(crate::core::archive::tree_truncated),
            );
            *self.tree_segs.borrow_mut() = cur;
            *self.tree_for.borrow_mut() = gallery_dir;
            self.tree_first.set(0);
            self.tree_hover.set(None);
        }
        // 无边框形态下 chrome 全隐（含标题栏、缩略图条、信息条），树一起收：那时
        // 整块画面就是标题栏，留一条常驻面板就不叫无边框了（判据同缩略图条）。
        let tree_w = if chrome::borderless() || self.tree_nodes.borrow().is_empty() {
            0
        } else {
            tree::width_for(content.w as f32) as i32
        };
        // ★ 条占的是**树的右边**，所以"放不放得下"要按扣掉树之后的宽算——拿整块
        //   `content.w` 去判会得出"放得下"，然后条压到树上。
        let body = strip_area(content, tree_w);

        let strip_side = filmstrip::Side::from_left(self.chrome.filmstrip_left.get());
        // m269：无边框形态下 chrome 全隐，条画出来就是一块多余的浮层 ⇒ 不显示
        // （不改配置，关掉形态后条照原样回来）。判据在 `fits` 之前，换边归零逻辑照旧走。
        let strip_on = self.strip_on()
            && !chrome::borderless()
            && filmstrip::fits(strip_side, body.w as f32, content.h as f32);
        // 条的主轴长度：底条按宽度、左条按高度——都用**扣掉树之后**那块，
        // 因为条画的就是 [`strip_area`] 的那一条（不是 `work`，也不是整块 `content`）。
        let strip_axis = match strip_side {
            filmstrip::Side::Bottom => body.w as f32,
            filmstrip::Side::Left => content.h as f32,
        };
        // 换边 / 关条 ⇒ 锚点归零（m258）：两个方向每屏格数不同，旧首格没有意义；
        // 关掉再打开时"跟着当前图"才符合直觉（没人会记得上次滚到哪）。
        let side_now = if strip_on { Some(strip_side) } else { None };
        if self.strip_side.replace(side_now) != side_now {
            self.strip_first.set(None);
            self.strip_drag.set(None);
            self.strip_dragged.set(false);
        }
        // 换目录 ⇒ 锚点归零（m260）：条记的是"这个目录里的浏览位置"，换个目录后旧位置
        // 没有意义（还可能把条停在几百张之外，让人以为条坏了）。判据是**目录**而不是
        // 格数——同目录里增删文件不算换目录，同一目录内换图更不动它（用户要的正是
        // "滚到很远再点一张"）。
        let dir_changed = {
            let v = self.viewer.borrow();
            let prev = self.strip_dir.borrow();
            match (prev.as_deref(), v.gallery_dir()) {
                (None, None) => false,
                (Some(a), Some(b)) => a != b,
                _ => true,
            }
        };
        // 重排（m275）与换目录同一条处置：条记的是"这个顺序里的浏览位置"，
        // 顺序换了之后同一个槽位已经是另一张图。
        let sort_changed = {
            let v = self.viewer.borrow();
            let epoch = v.sort_epoch();
            let stale = self.strip_sort.get() != epoch;
            if stale {
                self.strip_sort.set(epoch);
            }
            stale
        };
        if dir_changed || sort_changed {
            let dir = self.viewer.borrow().gallery_dir().map(Path::to_path_buf);
            *self.strip_dir.borrow_mut() = dir;
            self.strip_first.set(None);
            self.strip_drag.set(None);
            self.strip_dragged.set(false);
        }
        let work = work_of(content, strip_on.then_some(strip_side), tree_w);
        // m254：用户**刚**打开条 / 换了边，但窗口放不下 ⇒ 先把窗口放大到刚好放得下，
        // 下一帧条自然出现（`apply_session_flags` 只置一次请求，这里消费即清）。
        // ★ 用户自己把窗口拉小（条自动隐藏）不会被反向顶大——那种"拉不动"的窗口很讨厌。
        // 最大化 / 全屏时 `set_window_size` 由框架自行忽略，条先不显示，还原后自动出现。
        if self.chrome.strip_fit_request.get() {
            self.chrome.strip_fit_request.set(false);
            // m269：无边框形态下条本就不显示，别为了"放得下条"把窗口顶大
            // （那时候画面里根本没有那条）。
            if self.chrome.filmstrip.get() && !strip_on && !chrome::borderless() {
                let dpi = canvas.dpi_scale().max(0.5);
                // 要放得下的是「树 + 条 + 一点画面」，所以按扣掉树之后那块去算，
                // 再把树宽加回来——否则窗口刚好放得下条时树又把画面挤回去。
                let (tw, th) = filmstrip::grow_to_fit(strip_side, body.w as f32, content.h as f32);
                let tw = tw + tree_w as f32;
                self.chrome
                    .strip_resize
                    .set(Some(((tw * dpi).round() as i32, (th * dpi).round() as i32)));
            }
        }
        self.last_work.set(work);
        // 主辅视图（m322）：默认关；窗口够宽才分栏，太窄退回单栏。
        // `last_work` 仍是 chrome/胶片条/命中的整块；主图只用 `primary_work`。
        let companion_on = self.chrome.companion.get() && work.w >= 720 && !chrome::borderless();
        let companion_gap = if companion_on { 1 } else { 0 };
        let primary_work = if companion_on {
            Rect::new(
                work.x,
                work.y,
                ((work.w - companion_gap) / 2).max(1),
                work.h,
            )
        } else {
            work
        };
        self.primary_work.set(primary_work);
        self.pane_geom.primary.set(primary_work);
        let (vx, vy, vw, vh) = (work.x as f32, work.y as f32, work.w as f32, work.h as f32);
        let (checker_a, checker_b) = (tk::checker_a(), tk::checker_b());
        let checker_on = self.chrome.checker.get();

        // ① 底：棋盘格（透明背景开关）或近黑空场。
        // m269：无边框形态不画底色 —— 让 pixmap 的透明底直接上屏（补丁 028 负责
        // 逐像素合成 + alpha 下限 1），图片透明区因此透出桌面。
        if !chrome::borderless() {
            if checker_on {
                let mut y = vy - (vy % CHECKER);
                while y < vy + vh {
                    let mut x = vx - (vx % CHECKER);
                    let row = ((y - vy) / CHECKER) as i32;
                    while x < vx + vw {
                        let col = ((x - vx) / CHECKER) as i32;
                        let c = if (row + col) % 2 == 0 {
                            checker_a
                        } else {
                            checker_b
                        };
                        let w = CHECKER.min(vx + vw - x);
                        let h = CHECKER.min(vy + vh - y);
                        canvas.fill_rect(x, y, w, h, &Paint::fill(c));
                        x += CHECKER;
                    }
                    y += CHECKER;
                }
            } else {
                canvas.fill_rect(vx, vy, vw, vh, &Paint::fill(tk::empty_bg()));
            }
        }

        // ② 图 + 空场提示 / 错误。
        // 实时调色只对声明的那张图生效：换图后 `path` 对不上，效果自动失效，
        // 不用定时器清，也不会串到另一扇窗（每窗一份 `LiveState`）。
        let live = {
            let v = self.viewer.borrow();
            self.live.tint.get().filter(|t| {
                !t.is_neutral() && v.current_path().as_deref() == Some(t.path.as_path())
            })
        };
        // 旋转角度（m280）：绑路径，换图后旧值自动失效（与 `LiveTint.path` 同判据）。
        let rotate_deg = {
            let v = self.viewer.borrow();
            let bound = self.chrome.rotate_path.get();
            if bound.is_some() && bound.as_deref() == v.current_path().as_deref() {
                super::rotate_dialog::angle_of(self.chrome.rotate_deg.get())
            } else {
                0
            }
        };
        let hint_ts = TextStyle::new(14.0);
        let mut nav: Option<(Rect, Rect)> = None;
        // m290：平移上限要按**看得见的部分**算。标题栏是画在画布顶部的浮层，`view`
        // 里含它那一块（摆图居中用的就是整块），所以这里单独告诉 core「顶上被挡了多少」。
        // 判据与 `chrome::paint` 同源（`title_strip_h`），不另写一遍。
        let chrome_top = chrome::title_strip_h(
            window_state().fullscreen,
            self.chrome.minimal_mode.get(),
            &self.chrome,
        );
        {
            let mut v = self.viewer.borrow_mut();
            v.on_viewport(
                primary_work.x as f32,
                primary_work.y as f32,
                primary_work.w as f32,
                primary_work.h as f32,
                canvas.dpi_scale(),
            );
            v.set_chrome_top(chrome_top);
            // ★ 画的是**位图**的目标矩形（SVG 切片 ≠ 整图 dst），而鸟瞰几何仍用整图 dst。
            let base_dst = v.dst_rect(from_rect(primary_work));
            let img_src = v.image();
            let img_dst = v.image_dst(from_rect(primary_work));
            let error = v.error_text();
            if let (Some(src), Some(idst)) = (img_src, img_dst) {
                // 目标矩形已经是我们算好的等比尺寸，故用 `Fill` 直铺即可（不会拉伸变形）。
                // gen 代号没变就不重新上传纹理（未变换的翻页/缩放零成本命中缓存）。
                let mut cache = self.main_img.borrow_mut();
                let mut tinted = self.tint_main.borrow_mut();
                let shown = if let Some(t) = &live {
                    tinted.obtain(src, t.values, t.gamma, rotate_deg)
                } else if rotate_deg != 0 {
                    // 没调色但转了角度：仍要走 tinted 那一趟，只不过参数取中性。
                    tinted.obtain(
                        src,
                        crate::core::pixels::CHANNEL_NEUTRAL,
                        crate::core::pixels::GAMMA_NEUTRAL,
                        rotate_deg,
                    )
                } else {
                    cache.obtain(src)
                };
                if let Some(img) = shown {
                    // 主图也按自己的 pane 裁剪：以前只裁辅图，主图平移能侵到右栏，
                    // 而辅图越不过去，两边手感不对称。现在两块地各自裁齐。
                    canvas.save();
                    canvas.clip_rect(primary_work);
                    canvas.draw_image(img, to_rect(idst), Fit::Fill, 0.0, 1.0);
                    canvas.restore();
                } else if src.width > MAX_TEXTURE_SIDE || src.height > MAX_TEXTURE_SIDE {
                    // ★ 显卡放不下这张图 ⇒ **给一句人话**，而不是留一片空白。
                    //   （正常路径上显示位图已由 `core::derive` 降过边，这里是兜底。）
                    let msg = format!(
                        "这张图太大，显卡放不下（单边上限 {MAX_TEXTURE_SIDE} 像素，实际 {}×{}）",
                        src.width, src.height
                    );
                    let z = chrome::ARROW_ZONE as i32;
                    let slot = Rect::new(
                        primary_work.x + z,
                        primary_work.y,
                        (primary_work.w - 2 * z).max(60),
                        primary_work.h,
                    );
                    canvas.draw_text(&msg, slot, tk::error_text(), Align::Center, &hint_ts);
                }
                drop(tinted);
                drop(cache);
                // 鸟瞰缩略图（菠萝 NavigatorView）：显示尺寸超出视口时出现。
                // 几何锚在**整图** dst 上（位图是切片时映射关系也不变，见 viewer 的
                // 归一化坐标设计）。位图存在则整图 dst 必然存在。
                if let Some(bd) = base_dst {
                    nav = self.nav_geom_in(
                        if companion_on { primary_work } else { work },
                        work,
                        to_rect(bd),
                    );
                }
            } else if let Some(e) = error {
                // 中央这一句现在是错误的**唯一**载体（状态格里不再重复一遍），所以两头
                // 必须让开翻页箭头：504 宽实测文案两头各骑在一个箭头上，读不全。
                // 让宽之后 DirectWrite 自动折行（软硬两后端都按 rect.w 排版、纵向仍居中），
                // 长错误变成两行而不是一行两头截断。
                let z = chrome::ARROW_ZONE as i32;
                let slot = Rect::new(
                    primary_work.x + z,
                    primary_work.y,
                    (primary_work.w - 2 * z).max(60),
                    primary_work.h,
                );
                canvas.draw_text(&e, slot, tk::error_text(), Align::Center, &hint_ts);
            } else {
                // 首图还在解 = 说"载入中"，不许写"把图片拖进来"（m71）：后者是
                // **空窗口**的邀请，摆在已经打开的文件上就是谎话——真机上那张卡住
                // 13 s 的截图中央正是这句。
                //
                // 两句的**颜色也分开**（2026-11-02 用户口径"这句感觉要设置红色比较好"）：
                // 空窗是"等你做事"，灰字容易被当成背景装饰略过，用红请人动手；
                // "载入中…"是"已经在做事"，红字反而像出错了，保持灰。
                let (hint, color) = if v.awaiting_first_bitmap() {
                    ("载入中…", tk::hint_text())
                } else {
                    ("把图片拖进来 · 右键打开", tk::hint_invite())
                };
                canvas.draw_text(hint, primary_work, color, Align::Center, &hint_ts);
            }
        }

        // ③ 鸟瞰层：右下角缩略图 + 当前视口框。全程直角（用户拍板 2026-10-10）。
        //   主辅视图开时，主图的鸟瞰锚在**左半 pane**，不再占整窗右下角。
        let nav_content = if companion_on { primary_work } else { work };
        if let Some((box_r, thumb)) = nav {
            let v = self.viewer.borrow();
            let nav_img = v.nav_image_dst(from_rect(nav_content));
            let box_dst = v.dst_rect(from_rect(nav_content));
            if let Some(dst) = box_dst {
                let dst = to_rect(dst);
                // 主图鸟瞰画的是 ≤512 的整图缩略（不是主位图），仍按 idst↔dst 映射。
                let mut cache = self.nav_img.borrow_mut();
                let mut tinted = self.tint_nav.borrow_mut();
                let (idst, shown) = match nav_img {
                    Some((src, idst)) => {
                        let shown = if let Some(t) = &live {
                            tinted.obtain(src, t.values, t.gamma, rotate_deg)
                        } else if rotate_deg != 0 {
                            tinted.obtain(
                                src,
                                crate::core::pixels::CHANNEL_NEUTRAL,
                                crate::core::pixels::GAMMA_NEUTRAL,
                                rotate_deg,
                            )
                        } else {
                            cache.obtain(src)
                        };
                        (to_rect(idst), shown)
                    }
                    None => (dst, None),
                };
                paint_nav(canvas, box_r, thumb, nav_content, dst, shown, idst);
            }
        }

        // ④ 主辅视图：右栏辅图。主图已经画完；这里只画辅图、分隔线和徽标。
        if companion_on {
            let active = self.chrome.active_pane.get();
            let comp_work = Rect::new(
                primary_work.x + primary_work.w + companion_gap,
                work.y,
                (work.w - primary_work.w - companion_gap).max(1),
                work.h,
            );
            self.comp_work.set(comp_work);
            self.pane_geom.companion.set(comp_work);
            self.comp_dst.set(Rect::new(0, 0, 0, 0));
            let (primary_pos, next_path, next_page, peek2) = {
                let v = self.viewer.borrow();
                // ★ 辅图的“位置”始终用**文件在清单里的下标**。页式文件翻页时
                //   `gallery_pos` 不变，自选辅图才不会把页号当成文件号、跳到别的文件。
                let primary_pos = v.gallery_pos().saturating_sub(1);
                let peek2 = v.peek_path(2);
                if let Some(a) = v.anim_info().filter(|a| a.paged && a.count > 1) {
                    (
                        primary_pos,
                        v.current_path(),
                        Some((a.index + 1) % a.count),
                        peek2,
                    )
                } else {
                    (primary_pos, v.peek_path(1), None, peek2)
                }
            };
            {
                let mut c = self.companion.borrow_mut();
                let sort = self.config.borrow().sort_spec();
                c.set_source(
                    self.chrome.companion_source.get(),
                    self.chrome.companion_picked.borrow().as_deref(),
                    sort,
                );
                let pair = c.resolve_target(
                    self.chrome.companion_link.get(),
                    primary_pos,
                    next_path,
                    next_page,
                );
                // 预读提示：NextInPrimary 没有自己的清单，由这里算“下一项的下一项”。
                let hint = if self.chrome.companion_source.get() == CompanionSource::NextInPrimary
                    && self.chrome.companion_link.get() != LinkMode::Free
                {
                    match pair.as_ref() {
                        Some((path, Some(page))) => c
                            .anim_info()
                            .filter(|a| a.count > 0)
                            .map(|a| (path.clone(), Some((page + 1) % a.count))),
                        Some((_, None)) => peek2.clone().map(|p| (p, None)),
                        None => None,
                    }
                } else {
                    None
                };
                c.set_prefetch_hint(hint);
                // 解码框按**物理像素**给：1.0 就是 pane 在屏上的真实像素。
                // 不再乘 1.25 余量——那会让每次翻页多解 56% 的像素，
                // 翻页卡顿的主因；放大到 1.5 倍以上时另有按需重解。
                // 只乘逻辑宽会让高 DPI 屏上的 PDF 字发虚。上限由
                // `companion_decode_box` 统一收口，不让 4× 缩放把整图解成上亿像素。
                let dpi = canvas.dpi_scale().max(0.5);
                let detail = (dpi * self.comp_zoom.get().max(1.0)).min(6.0);
                let (max_w, max_h) = companion_decode_box(comp_work, detail);
                let target = pair.map(|(path, page)| super::companion::CompanionTarget {
                    path,
                    page,
                    max_w,
                    max_h,
                });
                c.set_target(target);
                c.poll();
            }
            let key = self
                .companion
                .borrow()
                .current_target()
                .map(|t| (t.path.clone(), t.page));
            let prev_key = self.comp_target_key.replace(key.clone());
            // 同一文件翻页/换帧只换内容，不重置用户已经调好的缩放与平移；
            // 换文件（或从有目标变成没目标）才归位。
            let reset_view = match (&prev_key, &key) {
                (Some((a, _)), Some((b, _))) => a != b,
                (None, None) => false,
                _ => true,
            };
            if reset_view {
                self.comp_zoom.set(1.0);
                self.comp_pan.set((0.0, 0.0));
                self.comp_drag.set(None);
                self.comp_nav_drag.set(false);
            }
            // 分隔线：主/辅之间一像素，压在图片上也要看得见。
            canvas.fill_rect(
                (primary_work.x + primary_work.w) as f32,
                work.y as f32,
                companion_gap as f32,
                work.h as f32,
                &Paint::fill(tk::STROKE_DARK),
            );
            // ★ 不 clone 位图：这里每帧都会走到，整张 RgbaImage 深拷一次
            //   （百万像素级就是几 MB/帧）正是辅图缩放卡顿的主因。
            //   只借住 `CompanionPane`，绘制期间没有任何地方会再对它 `borrow_mut`。
            let companion = self.companion.borrow();
            let image = companion.image();
            if let Some(img) = image {
                // ★ 显示比例取**原图尺寸**，不取解码位图尺寸：解码器的原生降采样
                //   偶尔会给出与 natural 不同的宽高比，按位图尺寸算 dst 会顺着
                //   坏比例继续拉伸。纹理仍用位图，`Fit::Fill` 会把内容拉回正确比例。
                let (nw, nh) = companion
                    .natural()
                    .filter(|(w, h)| *w > 0 && *h > 0)
                    .unwrap_or((img.width, img.height));
                let fit = (comp_work.w as f32 / nw.max(1) as f32)
                    .min(comp_work.h as f32 / nh.max(1) as f32);
                self.comp_fit.set(fit.max(0.0001));
                // 同步视图：直接用主图的**实际比例 + 像素偏移**，整组一起缩放/平移。
                let (scale, pan) = if self.chrome.companion_sync_view.get() {
                    let v = self.viewer.borrow();
                    (v.scale(), v.pan())
                } else {
                    (
                        fit * self.comp_zoom.get().clamp(0.1, 8.0),
                        self.comp_pan.get(),
                    )
                };
                let dw = (nw as f32 * scale).max(1.0);
                let dh = (nh as f32 * scale).max(1.0);
                let dst = Rect::new(
                    comp_work.x + ((comp_work.w as f32 - dw) / 2.0 + pan.0).round() as i32,
                    comp_work.y + ((comp_work.h as f32 - dh) / 2.0 + pan.1).round() as i32,
                    dw.round().max(1.0) as i32,
                    dh.round().max(1.0) as i32,
                );
                let comp_nav = self.nav_geom_in(comp_work, work, dst);
                self.comp_dst.set(dst);
                let mut cache = self.comp_img.borrow_mut();
                if let Some(tex) = cache.obtain(img) {
                    canvas.save();
                    canvas.clip_rect(comp_work);
                    canvas.draw_image(tex, dst, Fit::Fill, 0.0, 1.0);
                    canvas.restore();
                    if let Some((box_r, thumb)) = comp_nav {
                        paint_nav(canvas, box_r, thumb, comp_work, dst, Some(tex), dst);
                    }
                }
            } else {
                let (msg, color) = {
                    let c = self.companion.borrow();
                    if c.current_target().is_none() {
                        ("没有下一张", tk::hint_text())
                    } else if c.failed() {
                        ("辅图无法解码", tk::error_text())
                    } else {
                        ("辅图载入中…", tk::hint_text())
                    }
                };
                let slot = Rect::new(
                    comp_work.x + 8,
                    comp_work.y + comp_work.h / 2 - 10,
                    (comp_work.w - 16).max(40),
                    20,
                );
                canvas.draw_text(msg, slot, color, Align::Center, &hint_ts);
            }
            // ★ 徽标必须让开标题栏：`comp_work.y` 是窗口内容顶，而标题栏是
            //   后画的半透明浮层（40dp）——钉在 +8 就等于把「辅」埋在标题栏底下，
            //   浅色主题的白条会把它洗成一片白。`chrome_top` 与浮层绘制同源。
            //
            // 样式保持原设计：主题胶囊 + 主题文字。不按活动态换绿底，也不在
            // 旁边挂文件名——活动窗由 pane 四周那圈白描边表达，文件名底栏已有。
            let pane_top = comp_work.y + chrome_top as i32 + 8;
            let badge = Rect::new(comp_work.x + 8, pane_top, 22, 18);
            canvas.fill_round_rect(
                badge.x as f32,
                badge.y as f32,
                badge.w as f32,
                badge.h as f32,
                4.0,
                &Paint::fill(tk::pill_bg()),
            );
            canvas.draw_text(
                "辅",
                badge,
                tk::overlay_text(),
                Align::Center,
                &TextStyle::new(12.0),
            );
            let primary_badge = Rect::new(
                primary_work.x + 8,
                primary_work.y + chrome_top as i32 + 8,
                22,
                18,
            );
            canvas.fill_round_rect(
                primary_badge.x as f32,
                primary_badge.y as f32,
                primary_badge.w as f32,
                primary_badge.h as f32,
                4.0,
                &Paint::fill(tk::pill_bg()),
            );
            canvas.draw_text(
                "主",
                primary_badge,
                tk::overlay_text(),
                Align::Center,
                &TextStyle::new(12.0),
            );
            let active_rect = match active {
                Pane::Primary => primary_work,
                Pane::Companion => comp_work,
            };
            canvas.stroke_round_rect(
                active_rect.x as f32 + 0.5,
                active_rect.y as f32 + 0.5,
                (active_rect.w - 1).max(1) as f32,
                (active_rect.h - 1).max(1) as f32,
                0.0,
                1.0,
                &Paint::fill(tk::STROKE_LIGHT),
            );
        }

        // ④ 悬浮层（箭头 + 工具栏 + 信息条）。三者同一条门控：极简模式下都靠悬停浮现
        //    （箭头看 `Chrome::hover_edge` 那两位，见 `chrome::paint`），传统模式常驻。
        // 辅图是活动窗：底栏状态、顶栏文件名都跟活动窗走；只有辅图真有
        // 页/帧号时才挂「上一页/下一页」，且永远不挂播放钮（播放属于主图）。
        let comp_active =
            self.chrome.companion.get() && self.chrome.active_pane.get() == Pane::Companion;
        let comp_index = comp_active && self.companion.borrow().has_index();
        let comp_free = comp_active
            && self.chrome.companion_link.get() == LinkMode::Free
            && self.companion.borrow().gallery_len() > 1;
        let comp_scale = if self.chrome.companion_sync_view.get() {
            self.viewer.borrow().scale()
        } else {
            self.comp_fit.get() * self.comp_zoom.get().clamp(0.1, 8.0)
        };
        let v = self.viewer.borrow();
        // NextInPrimary 没有自己的清单：用主图清单推算“辅图在第几张”。
        let comp_file = if self.companion.borrow().gallery_len() > 0 {
            None
        } else {
            let len = v.gallery_len();
            if len == 0 {
                None
            } else {
                let paged = v.anim_info().is_some_and(|a| a.paged && a.count > 1);
                let pos = v.gallery_pos();
                let next = if paged {
                    pos
                } else if pos >= len {
                    1
                } else {
                    pos + 1
                };
                Some((next, len))
            }
        };
        let comp_status =
            comp_active.then(|| self.companion.borrow().status(comp_scale, comp_file));
        let info = comp_status.clone().unwrap_or_else(|| v.status());
        let has_prev = if comp_free { true } else { v.has_prev() };
        let has_next = if comp_free { true } else { v.has_next() };
        let title = if comp_active {
            self.companion
                .borrow()
                .file_title()
                .unwrap_or_else(|| v.title())
        } else {
            v.title()
        };
        // 动画组该不该出现、暂停钮画 ▶ 还是 ⏸——都从 viewer 现算，不另存一份真相。
        // 这里只回灌不标脏：状态一变（首帧落地 / 按 P）那条路径自己会标，本轮绘制
        // 就在标脏之后发生，再标一次是空转。
        //
        // ★ `chrome.anim` 的语义是"**有没有帧/页号**"（= `has_index()`），不是"是不是
        //   动图"（= `is_animated()`）。静态位图没有号 ⇒ 播放组整组不出现；只有真动图
        //   与多页文档才挂。字形仍按真实播放态走：没在播 → ▶，动图 → ⏸/▶ 按 `anim_paused`。
        let (has_index, paused, index_pair) = if comp_active {
            (comp_index, true, comp_index)
        } else {
            (
                v.has_index(),
                if v.is_animated() {
                    v.anim_paused()
                } else {
                    true
                },
                false,
            )
        };
        drop(v);
        self.chrome.anim.set(has_index);
        self.chrome.index_pair.set(index_pair);
        self.chrome.paused.set(paused);
        // 状态文本一变就把浮条叫醒：逐帧是按键操作，鼠标不动，极简模式下不叫的话
        // 帧号（屏幕上唯一的载体）根本没地方显示。判据见 `Chrome::wake_on_status_change`。
        self.chrome.wake_on_status_change(&info);
        let ws = window_state();
        let minimal = self.chrome.minimal_mode.get(); // 使用运行时有效值（配置 + 临时切换）
        chrome::paint(
            canvas,
            (vx, vy, vw, vh),
            &self.chrome,
            &info,
            has_prev,
            has_next,
            ws.fullscreen,
            ws.topmost,
            ws.maximized,
            minimal,
            &title,
        );

        // ⑤ 文件树（m300）：常驻在画面最外侧，已在前面从摆图区里扣掉。
        //
        // 画在 chrome **之后**：树不占 `work`，两者根本不重叠（chrome 的几何全在
        // `work` 里推导），先后只影响"谁盖谁"这种不可能发生的情况。
        self.tree_at.set(None);
        if tree_w > 0 {
            let nodes = self.tree_nodes.borrow();
            let p = tree::panel(content, tree_w as f32);
            let cur = self
                .tree_segs
                .borrow()
                .as_ref()
                .and_then(|s| nodes.iter().position(|n| &n.segs == s));
            // 滚动夹一次：换层后节点数变少时，旧首行可能越界（那时整棵树都是空的，
            // 夹到 0 比留一个越界值安全）。
            let cap = tree::capacity(p.h, self.tree_truncated.get());
            let max = tree::max_first(nodes.len(), cap);
            let first = self.tree_first.get().min(max);
            self.tree_first.set(first);
            tree::paint(
                canvas,
                p,
                &nodes,
                cur,
                first,
                self.tree_hover.get(),
                self.tree_truncated.get(),
            );
            self.tree_at.set(Some(p));
        }

        // ⑥ 底部缩略图条（常驻的一块地，已在前面从摆图区里扣掉）。
        *self.strip_items.borrow_mut() = if strip_on {
            // 先向状态机要"这一屏该显示哪几张"（顺带为缺图的发起后台解码），
            // 再把格子摆开。m100：路径全程按引用走，一帧一个 `PathBuf` 都不克隆。
            let cap = filmstrip::capacity(strip_side, strip_axis);
            // m258：条是**独立滚动的列表**。锚点由拖动/滚轮写入；`None` 时保持老行为
            // （当前图居中，见 `strip_window_first`）。★ m260：`Some` 时条完全听用户的——
            // 当前图可以不在视野里（用户要的正是"拖到很远再点一张"）。
            let (first, n) = self
                .viewer
                .borrow_mut()
                .strip_window(cap, self.strip_first.get());
            let len = self.viewer.borrow().gallery_len();
            self.strip_cap.set(n.max(1));
            self.strip_len.set(len);
            // gallery_pos() 返回 1-based，filmstrip 内部用 0-based 索引比较，需减 1
            let cur = self.viewer.borrow().gallery_pos().saturating_sub(1);
            // 条画在**完整 content** 的那一条上（扣掉的那条正是它占的地，用 work 就
            // 叠回画面上了）。★ 贴左时 `work.x` 已经右移了一条宽 ⇒ 这里必须重新取
            // `content.x` / `content.w`，不能拿上面那组 `vx`/`vw`（那是 work 的）。
            // ★ m300：条画在 [`strip_area`] 那一块（= `content` 扣掉左边的文件树），
            //   不是整块 `content`——否则条会压到树上、而画面又被扣掉两条宽。
            let at = filmstrip::Placement::new(
                strip_side,
                (body.x as f32, body.y as f32, body.w as f32, body.h as f32),
            )
            .with_tree(tree_w > 0);
            let rects = filmstrip::layout(at, n);
            // 事件回调（命中 / 拖动 / 滚轮 / 滚动指示 / 光标）要判断"指针在不在条上"：
            // 把 `Placement` 整份存下来，`panel` / `layout` 都能从它推——与 `strip_items`
            // 同一条理由：绘制与命中共用同一帧几何。
            self.strip_at.set(Some(at));
            let v = self.viewer.borrow();
            let slots: Vec<filmstrip::Slot<'_>> = (0..n)
                .filter_map(|i| {
                    let idx = first + i;
                    v.strip_path(idx).map(|p| filmstrip::Slot {
                        idx,
                        rect: rects[i],
                        path: p,
                    })
                })
                .collect();
            self.strip
                .paint(canvas, at, &slots, cur, self.strip_hover.get(), |p| {
                    v.thumb(p)
                });
            // 滚动指示（m258，m261 改位）：目录长于一屏才出现。底条压**条上缘**
            // （缩略图之上，避开窗口底部那圈缩放边框）、左条压靠画面的右缘；
            // 视觉 6dp、命中热区 16dp，不加按钮（按钮会吃掉一格且要维护四个态）。
            if let Some((track, thumb)) = filmstrip::scroll_bar(at, first, n, len) {
                canvas.fill_round_rect(
                    track.x as f32,
                    track.y as f32,
                    track.w as f32,
                    track.h as f32,
                    1.0,
                    &Paint::fill(tk::strip_track()),
                );
                canvas.fill_round_rect(
                    thumb.x as f32,
                    thumb.y as f32,
                    thumb.w as f32,
                    thumb.h as f32,
                    1.0,
                    &Paint::fill(tk::strip_thumb()),
                );
            }
            slots.iter().map(|s| (s.idx, s.rect)).collect()
        } else {
            self.strip_at.set(None);
            self.strip_cap.set(1);
            self.strip_len.set(0);
            // m265：条没画 ⇒ 没有「可见的那一屏」，预读退回纯邻图（每帧比较，不变即返回）。
            self.viewer.borrow_mut().set_prefetch_strip(0, 0);
            Vec::new()
        };

        // ⑥ 取色器浮层（m217）：最后画，压在包括鸟瞰/缩略图条在内的所有图层之上。
        // 判据与点击复制共用 `Viewer::pick_at` —— 屏上显示哪一格，就复制哪一格。
        if let (true, Some(pos)) = (self.chrome.picker.get(), self.chrome.picker_at.get()) {
            if let Some(pk) = self
                .viewer
                .borrow()
                .pick_at(from_rect(self.view_work()), pos)
            {
                let [r, g, b, _] = pk.rgba;
                let hex = format!("#{r:02X}{g:02X}{b:02X}");
                let text = format!("{hex} · rgb({r}, {g}, {b}) · ({}, {})", pk.at.0, pk.at.1);
                let ts = TextStyle::new(12.0);
                let tw = self.chrome.measure_w(canvas, &text, &ts);
                let (sw, gap, pad, box_h) = (16.0, 8.0, 9.0, 28.0);
                let box_w = sw + gap + tw + pad * 2.0;
                // 跟随光标但不许出内容矩形：贴右/下边时翻到指针另一侧。
                let mut x = pos.0 + 16.0;
                let mut y = pos.1 + 18.0;
                if x + box_w > vx + vw - 4.0 {
                    x = (pos.0 - box_w - 16.0).max(vx + 4.0);
                }
                if y + box_h > vy + vh - 4.0 {
                    y = (pos.1 - box_h - 12.0).max(vy + 4.0);
                }
                canvas.fill_round_rect(x, y, box_w, box_h, 6.0, &Paint::fill(tk::pill_bg()));
                canvas.stroke_round_rect(
                    x - 1.0,
                    y - 1.0,
                    box_w + 2.0,
                    box_h + 2.0,
                    6.0,
                    1.0,
                    &Paint::fill(tk::STROKE_DARK),
                );
                canvas.fill_rect(
                    x + pad,
                    y + (box_h - sw) / 2.0,
                    sw,
                    sw,
                    &Paint::fill(tk::to_color4([r, g, b, 255])),
                );
                let slot = Rect::new(
                    (x + pad + sw + gap) as i32,
                    y as i32,
                    (tw + 2.0) as i32,
                    box_h as i32,
                );
                canvas.draw_text(&text, slot, tk::overlay_text(), Align::Center, &ts);
            }
        }
    }
    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        match ev {
            Event::Pointer(p) => self.on_pointer(ctx, p),
            // 键盘：焦点在本控件时走这里；焦点被按钮拿走时由 `App::on_shortcut` 兜底。
            // 两条路互斥（本控件消费了就不会再进 shortcut），不会重复处理。
            Event::Key(k) => {
                if !k.pressed {
                    return false;
                }
                // 锁定：键盘全封。返回 true = 已消费，Esc 也就轮不到框架的关窗兜底。
                if self.chrome.locked.get() {
                    return true;
                }
                // 取色模式（m217）：Esc 优先退出模式，不让「Esc 关窗」把窗口一起带走。
                // 其它键照常派发——切图/缩放时取色器保持开启，方便连续取色。
                if self.chrome.picker.get() && k.key == Key::Escape {
                    self.chrome.close_picker();
                    ctx.mark_dirty();
                    return true;
                }
                // 自由翻页：左右方向键先归辅图自己的清单。
                if self.companion_file_key(k) {
                    ctx.mark_dirty();
                    return true;
                }
                // 活动辅图有页/帧号时，逐页键先归辅图（与底栏那两颗同源）。
                if self.companion_frame_key(k) {
                    ctx.mark_dirty();
                    return true;
                }
                // 活动辅图的缩放/平移键也归辅图；主图不动。
                if self.companion_view_key(k) {
                    ctx.mark_dirty();
                    return true;
                }
                // 先给用户绑定的动作让路：Esc 若被绑到某动作，handle_key 命中并消费执行。
                let handled = handle_key(
                    Some(ctx),
                    Some(&self.live),
                    &self.viewer,
                    &self.chrome,
                    &self.config,
                    &self.hub,
                    k,
                );
                if handled {
                    ctx.mark_dirty();
                    return true;
                }
                // 没被任何绑定命中：Esc 兜底——全屏中先退全屏，否则交给框架默认（关窗）。
                // 这条安全网保证极简模式无标题栏时用户仍能退出（见 window.rs）。
                if k.key == Key::Escape {
                    if window_state().fullscreen {
                        ctx.toggle_fullscreen();
                        ctx.mark_dirty();
                        return true;
                    }
                    return false;
                }
                false
            }
        }
    }

    /// 键盘可达：本控件是唯一需要处理按键的，回车/方向键靠 Tab 焦点走到这里。
    fn focusable(&self) -> bool {
        true
    }

    /// 接收右键：右键菜单是画布的核心交互（windui 默认不把非左键投递给控件，
    /// 不开这个开关 `on_pointer` 根本收不到右键 Down，菜单永远不弹）。
    fn wants_right_click(&self) -> bool {
        true
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

impl ImageCanvas {
    /// 悬浮层该用的视口：`last_work`（扣掉缩略图条后剩下的那块）。`paint` 摆工具栏、
    /// 箭头、信息条用的就是它，命中/悬停必须同一份几何。
    fn work_tuple(&self) -> (f32, f32, f32, f32) {
        let w = self.last_work.get();
        (w.x as f32, w.y as f32, w.w as f32, w.h as f32)
    }

    /// 悬浮层指针活动（Enter/Move/按下共用）。**与 [`ImageCanvas::paint`] 同一份
    /// 几何**：开条时 `last_work` 比整块 content 矮一个 [`filmstrip::STRIP_H`]，
    /// 拿错那份命中区就整体下沉 96px（报障：点工具栏没反应、点缩略图反而触发按钮）。
    #[must_use = "true = 悬浮层显隐位翻了，必须接着去 mark_dirty（理由见 `Chrome::activity`）"]
    fn chrome_activity(&self, pos: (f32, f32)) -> bool {
        self.chrome.activity(pos, self.work_tuple())
    }

    /// 悬浮层命中（Down/Up 共用），几何约定同 [`Self::chrome_activity`]。
    fn chrome_hit(&self, pos: (f32, f32)) -> Option<Press> {
        self.chrome.hit(pos, self.work_tuple())
    }

    /// 收掉面板上的「可点」提示（m312，锁定时用）。
    ///
    /// 树那一行、条那一格还亮着，锁住的窗口看着像还能点——锁定只吞事件不清
    /// 悬停的话，指针一停下就停在那块高亮上，直到解锁才消失。
    fn clear_panel_hover(&self, ctx: &mut EventCtx) {
        let tree = self.tree_hover.replace(None).is_some();
        let strip = self.strip_hover.replace(None).is_some();
        if tree || strip {
            ctx.mark_dirty();
        }
    }

    /// 指针落在缩略图条背板里吗（用 `paint` 写下的那一帧几何，**视口绝对坐标**）。
    fn in_strip_panel(&self, pos: (f32, f32)) -> bool {
        let Some(at) = self.strip_at.get() else {
            return false;
        };
        let r = filmstrip::panel(at);
        pos.0 >= r.x as f32
            && pos.0 < (r.x + r.w) as f32
            && pos.1 >= r.y as f32
            && pos.1 < (r.y + r.h) as f32
    }

    /// 指针落在四缘缩放带里吗（按**整块 content** 的边缘算，不是 `work` 的）。
    fn in_resize_band(&self, pos: (f32, f32)) -> bool {
        let c = self.last.get();
        pos.0 < c.x as f32 + RESIZE_BAND_DP
            || pos.0 >= (c.x + c.w) as f32 - RESIZE_BAND_DP
            || pos.1 < c.y as f32 + RESIZE_BAND_DP
            || pos.1 >= (c.y + c.h) as f32 - RESIZE_BAND_DP
    }

    /// 指针落在「能拖图」的那块地上吗。
    ///
    /// `last_work` 是扣掉缩略图条之后留给画面的矩形；再与「content 去掉四缘缩放带」
    /// 求交，条、窗口边框、条与画面之间的空白就全被排除。标题栏另由
    /// [`Chrome::in_title_strip`] 判（它永远拖窗口，见 [`left_drag`]）。
    /// ★ 这三者以前都算「可拖区」，于是缩略图上也是松手、拖条时还是抓手，
    ///   自绘覆盖一路盖到窗口边框，把系统缩放光标吃掉了。
    fn in_pan_area(&self, pos: (f32, f32)) -> bool {
        let w = self.last_work.get();
        let c = self.last.get();
        let left = (c.x as f32 + RESIZE_BAND_DP).max(w.x as f32);
        let top = (c.y as f32 + RESIZE_BAND_DP).max(w.y as f32);
        let right = (c.x as f32 + c.w as f32 - RESIZE_BAND_DP).min((w.x + w.w) as f32);
        let bottom = (c.y as f32 + c.h as f32 - RESIZE_BAND_DP).min((w.y + w.h) as f32);
        pos.0 >= left && pos.0 < right && pos.1 >= top && pos.1 < bottom
    }

    /// 命中滚动指示的热区：返回 `(轨道, 滑块)`（绘制用的矩形）。
    ///
    /// 判据全部走 `filmstrip::scroll_at`（一屏放得下就 `None`）——热区、绘制、能不能滚
    /// 三者同源，不许各算一遍。
    fn strip_scroll_hit(&self, pos: (f32, f32)) -> Option<(Rect, Rect)> {
        let at = self.strip_at.get()?;
        filmstrip::scroll_at(
            at,
            self.strip_shown_first(),
            self.strip_cap.get(),
            self.strip_len.get(),
            pos,
        )
    }

    /// 条能不能滚：目录长于一屏。与 `filmstrip::scroll_bar` 同一条判据——
    /// 指示条出现 = 能滚，两者不许各自算一遍。
    fn strip_scrollable(&self) -> bool {
        self.strip_len.get() > self.strip_cap.get()
    }

    /// 当前**显示**的窗口首格（拖动/滚轮的起点）。`strip_items` 的第一格就是它。
    ///
    /// 为什么不直接读锚点：锚点可能被夹紧（目录变短 / 换边），而 `strip_items` 是
    /// **这一帧真画在屏上**的那一批——用户眼睛看到、手指下的才是起点。
    fn strip_shown_first(&self) -> usize {
        self.strip_items
            .borrow()
            .first()
            .map(|(idx, _)| *idx)
            .unwrap_or(0)
    }

    /// 左键按下的归属分派（Down 分支只调这一句）。
    ///
    /// ★ 抽成方法不是为了好看，是为了**把接线钉住**：[`left_drag`] 是纯函数，它绿了
    /// 不代表调用点真在用它（用例见 `left_press_wires_the_switch_to_the_pan_state`）。
    /// `ctrl` = 按住 Ctrl：那是 2026-10-08 起的平移兜底手势，**不看开关**一律平移。
    ///
    /// 先算成局部变量再分支：`self.viewer.borrow()` 的 `Ref` 若活进分支里，
    /// 那句 `borrow_mut()` 就是 `already borrowed` panic（§七 的禁写形状）。
    fn press_left(&self, pos: (f32, f32), view: (f32, f32, f32, f32), ctrl: bool) {
        // 无边框那两问也在这里一次取好：`chrome::borderless()` 是模块级镜像（不借
        // viewer），`can_pan` / `overflows` 借 viewer 的 `Ref`——与本函数下面那次
        // `borrow_mut()` 必须错开，所以拷成 bool 再往下传。
        let (borderless, can_pan, overflows) = {
            let v = self.viewer.borrow();
            (chrome::borderless(), v.can_pan(), v.overflows())
        };
        let mode = left_drag(
            self.config.borrow().drag_image,
            can_pan,
            Chrome::in_title_strip(pos, view),
            window_state().fullscreen,
            borderless,
            overflows,
        );
        if ctrl || mode == LeftDrag::PanImage {
            self.viewer.borrow_mut().begin_drag(pos);
        } else if mode == LeftDrag::Window {
            // 只有这一支会记待定拖窗位；全屏的 `None` 什么都不记，Move 里
            // 也就不会有那一记 `start_window_drag()`。
            self.lmb_pending.set(Some(pos));
        }
    }

    /// 按当前指针位置与拖动状态换光标：**只有能拖图的那块地**才给手型。
    ///
    /// ★ 判据与 [`left_drag`] **同源**：光标说"这一拖归图片"，按下去就一定是拖图。
    ///   两边各写一套的话会出现"光标是手、按下却把窗口拖走了"。
    ///
    /// 浮条/左右箭头/标题钮上不接手——那里点的是控件；标题栏那一条也留箭头，
    /// 因为那里永远拖窗口（[`left_drag`] 的第三项）。
    /// ★ m261（用户 2026-11-08）：缩略图条与四缘缩放带**都不接手**。条是独立列表，
    ///   拖它归列表不归画面；边框那 8dp 是非客户区，自绘覆盖会把系统的缩放光标顶掉。
    fn sync_hand(&self, pos: (f32, f32)) {
        // 取色模式优先（m217）：十字光标，且不许被下面的抓手/箭头覆盖。
        // 光标说"这一拖归图片"、按下去却是取样，是最刺眼的语义错位。
        if self.chrome.picker.get() {
            crate::platform::cursor::use_cross();
            return;
        }
        // 图片拖动中不看位置：指针可能已经拖出画布，手型要一直握着。
        // ★ 条拖动 / 拖滚动指示**不在此列**：那两样拖的是列表，给抓手就会让
        //   "手型停在条上"一路带到窗口边框（缩放光标出不来的直接成因）。
        if self.viewer.borrow().dragging() {
            super::hand_cursor::apply(Some(true));
            return;
        }
        // 四缘缩放带：只撤覆盖，光标交给框架（WM_SETCURSOR 已经设好了 IDC_SIZE*）。
        // 走 `apply(None)` 会顺手 `use_arrow()`，那一帧就把缩放光标顶成箭头。
        if self.in_resize_band(pos) {
            super::hand_cursor::release();
            return;
        }
        if self.chrome_hit(pos).is_some() {
            super::hand_cursor::apply(None);
            return;
        }
        // 缩略图条 / 条外空白：不是能拖图的地，一律箭头。
        if !self.in_pan_area(pos) {
            super::hand_cursor::apply(None);
            return;
        }
        let in_title = Chrome::in_title_strip(pos, self.work_tuple());
        let (borderless, can_pan, overflows) = {
            let v = self.viewer.borrow();
            (chrome::borderless(), v.can_pan(), v.overflows())
        };
        let mode = left_drag(
            self.config.borrow().drag_image,
            can_pan,
            in_title,
            window_state().fullscreen,
            borderless,
            overflows,
        );
        super::hand_cursor::apply(match mode {
            LeftDrag::PanImage => Some(false),
            LeftDrag::Window | LeftDrag::None => None,
        });
    }

    fn on_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> bool {
        let pos = (p.pos.x as f32, p.pos.y as f32);
        // ★ m312：锁定 = **整窗**交互封锁，在这里收成一道闸。
        //   以前这条判据散在各个分支里（滚轮一处、缩略图条一处、右键几处），
        //   漏了**文件树**：锁着窗口点树上的某一行照样换层，条上照样换图
        //   （用户实报「锁住窗口的时候，目录和缩略图好像没有一起锁住」）。
        //   漏的根因不是忘了写，是「每加一块地就得记得再加一次判断」——
        //   收成一道闸之后，新面板天然被锁住，不必逐个补。
        //   只放行标题栏那颗锁钮（`Chrome::hit` 锁定时也只认它），别的全吞。
        //   ★ 必须**吞掉**（return true），不能 return false：漏下去会落到
        //   「拖窗口 / 双击关窗」那条兜底路——锁着的窗口被拖走、被关掉更不像话。
        if self.chrome.locked.get() {
            let unlock = matches!(p.kind, PointerKind::Down | PointerKind::Up)
                && p.button == MouseButton::Left
                && self.chrome_hit(pos) == Some(Press::TitleBtn(TitleBtn::Lock));
            if !unlock {
                // 滚轮沿用旧口径：即便锁着也算「用户插手了画面」，先把放映停掉再吞。
                if matches!(p.kind, PointerKind::Wheel(_)) {
                    self.chrome.slide_stop();
                }
                // 悬停一并收掉：树那一行还亮着，锁着的窗口看着像还能点。
                self.clear_panel_hover(ctx);
                return true;
            }
        }
        // 主辅视图：点左/右任一 pane 切换活动窗。点在 chrome 浮层上时照旧让给按钮。
        if p.kind == PointerKind::Down
            && p.button == MouseButton::Left
            && self.chrome.companion.get()
            && self.chrome_hit(pos).is_none()
        {
            if self.comp_work.get().contains(p.pos) {
                if self.chrome.companion_sync_view.get() {
                    // 同步视图：右栏按下也交给主图平移，辅图下一帧镜像。
                    self.chrome.active_pane.set(Pane::Primary);
                } else {
                    self.chrome.active_pane.set(Pane::Companion);
                    // ★ 顶部标题栏那一条永远拖窗口，不是拖图。旧写法只要落在右 pane
                    //   就先开始 `comp_drag`，于是标题栏空白处按下去只会平移到图片，
                    //   窗口拖不动（用户实报「活动窗的标题栏没有交互」）。
                    //   标题钮已由 `chrome_hit` 先挑走，这里剩的是纯标题栏空白。
                    if !Chrome::in_title_strip(pos, self.work_tuple()) {
                        // 辅图鸟瞰优先于拖图：拖的是视口框，不是整张图。
                        if let Some((box_r, thumb)) = self.comp_nav_geom() {
                            if box_r.contains(p.pos) {
                                self.comp_nav_drag.set(true);
                                self.comp_drag.set(None);
                                self.comp_nav_pan_to(self.comp_dst.get(), thumb, pos);
                                ctx.mark_dirty();
                                return true;
                            }
                        }
                        self.comp_drag.set(Some((pos, self.comp_pan.get())));
                        ctx.mark_dirty();
                        return true;
                    }
                }
            }
            if self.primary_work.get().contains(p.pos)
                && self.chrome.active_pane.get() == Pane::Companion
            {
                self.chrome.active_pane.set(Pane::Primary);
                ctx.mark_dirty();
            }
        }
        if self.comp_nav_drag.get() {
            match p.kind {
                PointerKind::Move => {
                    if let Some((_, thumb)) = self.comp_nav_geom() {
                        self.comp_nav_pan_to(self.comp_dst.get(), thumb, pos);
                    }
                    ctx.mark_dirty();
                    return true;
                }
                PointerKind::Up => {
                    self.comp_nav_drag.set(false);
                    return true;
                }
                _ => {}
            }
        }
        if let Some((start, start_pan)) = self.comp_drag.get() {
            match p.kind {
                PointerKind::Move => {
                    self.comp_pan
                        .set((start_pan.0 + pos.0 - start.0, start_pan.1 + pos.1 - start.1));
                    ctx.mark_dirty();
                    return true;
                }
                PointerKind::Up => {
                    self.comp_drag.set(None);
                    return true;
                }
                _ => {}
            }
        }
        match p.kind {
            PointerKind::Wheel(delta) => {
                // ★ 放映让位挂在**滚轮事件本身**上，不挂在翻页节奏闸上：默认档里滚轮是缩放
                // （默认档 `wheel == Zoom`），那条路根本不进 `wheel_gate`——2026-10-31 用户实测
                // "滚轮没用、方向键有用"就是这个错位。缩放同样是"用户插手了画面"。
                self.chrome.slide_stop();
                if self.chrome.companion.get()
                    && self.chrome.companion_sync_view.get()
                    && self.comp_work.get().contains(p.pos)
                    && self.chrome_hit(pos).is_none()
                {
                    // 同步视图：在右栏滚轮也改主图比例，辅图下一帧镜像。
                    self.chrome.active_pane.set(Pane::Primary);
                    let z =
                        self.viewer.borrow().scale() * if delta > 0 { 1.25 } else { 1.0 / 1.25 };
                    self.viewer.borrow_mut().zoom_at_center(z);
                    ctx.mark_dirty();
                    return true;
                }
                if self.chrome.companion.get()
                    && self.comp_work.get().contains(p.pos)
                    && self.chrome_hit(pos).is_none()
                {
                    self.chrome.active_pane.set(Pane::Companion);
                    let z = self.comp_zoom.get();
                    let nz = if delta > 0 { z * 1.25 } else { z / 1.25 };
                    self.comp_zoom.set(nz.clamp(0.1, 8.0));
                    ctx.mark_dirty();
                    return true;
                }
                // 锁定已在 `on_pointer` 入口统一吞掉，这里不必再判一次。
                // 缩略图条（m259）：条内滚轮 = 翻列表，**不换图**。
                // 这里不看 `config.wheel`——列表上的滚轮就是滚动列表，条上也没有
                // 可缩放的画布；Shift+滚轮 = 一屏。碎增量同样攒够一格才算（`wheel_detent`）。
                // ★ 方向（m259 订正、m268 与翻页共用一根轴）：Windows 向上滚给 +120，
                //   列表惯例是"向下滚 = 看后面" ⇒ 先经 [`wheel_nav_dir`] 翻号。
                //   第一版写成 `+ dir`，于是"往下滚想看后面"反而在开头卡死，用户实报
                //   "滚轮对滚动条不起作用"就是这一句。
                // 文件树（m300）：树内滚轮 = 滚树，**不换图、不缩放**。树常常比一屏长
                // （一个包几十层），没有这条就只能靠把窗口拉高。方向与缩略图条同轴
                // （向下滚 = 往树的下方看），共用 [`wheel_nav_dir`] 翻号。
                if let Some(tp) = self.tree_at.get() {
                    if tp.contains(windui::geometry::Point {
                        x: pos.0 as i32,
                        y: pos.1 as i32,
                    }) {
                        let (acc, dir) = wheel_detent(self.wheel_acc.get(), delta);
                        self.wheel_acc.set(acc);
                        if dir != 0 {
                            let cap = tree::capacity(tp.h, self.tree_truncated.get());
                            let max = tree::max_first(self.tree_nodes.borrow().len(), cap);
                            let from = self.tree_first.get();
                            let next = (from as isize
                                + wheel_nav_dir(dir) * (if p.mods.shift { 5 } else { 1 }))
                            .clamp(0, max as isize) as usize;
                            if self.tree_first.replace(next) != next {
                                ctx.mark_dirty();
                            }
                        }
                        return true;
                    }
                }
                if self.in_strip_panel(pos) && self.strip_scrollable() {
                    let (acc, dir) = wheel_detent(self.wheel_acc.get(), delta);
                    self.wheel_acc.set(acc);
                    if dir != 0 {
                        let n = self.strip_cap.get();
                        let step = if p.mods.shift { n } else { 1 } as isize;
                        let max_first = self.strip_len.get().saturating_sub(n) as isize;
                        let from = self.strip_first.get().unwrap_or(self.strip_shown_first());
                        let first = (from as isize + wheel_nav_dir(dir) * step).clamp(0, max_first)
                            as usize;
                        if self.strip_first.replace(Some(first)) != Some(first) {
                            ctx.mark_dirty();
                        }
                    }
                    return true;
                }
                // ── 滚轮行为（m278）：四档定制的**成对动作**，一个下拉管上下两个方向 ──
                // Ctrl / Alt / Shift 各读自己那一档，都不按才走裸滚轮档。缩略图条内
                // 那一支已在上面 return（条上滚轮恒滚条、Shift 一屏），不会来抢。
                let wa = wheel_action_for(&self.config.borrow(), &p.mods);
                match wa {
                    // 放大 / 缩小：保留指针锚点与连续缩放，不改走离散 Action。
                    crate::core::config::WheelAction::Zoom => {
                        let c = self.center();
                        self.viewer.borrow_mut().zoom_by_wheel(delta, pos, c);
                    }
                    // 下一张 / 上一张：与 m278 之前**同一条路径**（碎增量 + 节奏闸），
                    // 老 ini 的 wheel_behavior=1 / ctrl_wheel=1 行为一字不变。
                    crate::core::config::WheelAction::Navigate => {
                        // 每格（±120）翻一张；碎增量累计后按符号翻（判据见 [`wheel_detent`]）。
                        let (acc, dir) = wheel_detent(self.wheel_acc.get(), delta);
                        self.wheel_acc.set(acc);
                        if dir != 0 {
                            // ★ 方向（m268）：`dir` 是滚轮符号，不是清单方向。向下滚（-120）
                            //   要看后面 ⇒ 经 [`wheel_nav_dir`] 翻成 `+1`。节奏闸里存的也必须是
                            //   翻好的步进方向，否则 `tick_wheel_pace` 兜出去的那一下会反着翻。
                            let step_dir = wheel_nav_dir(dir);
                            // 节奏闸（A3 / m64）：`WHEEL_PACE` 内最多翻一张，压在闸里的那一下
                            // 由 runtime 的 16 ms 轮询兜出去 —— **挂起不是丢弃**。
                            if self.chrome.wheel_gate(step_dir) {
                                self.viewer.borrow_mut().step(step_dir);
                            }
                        }
                    }
                    // 不透明度 + / −：新增档。同样过碎增量与节奏闸，免得触控板一格跳十档。
                    // 向上滚 = +、向下滚 = −，与 `WheelAction::pair()` 登记的反向一致。
                    crate::core::config::WheelAction::Opacity => {
                        let (acc, dir) = wheel_detent(self.wheel_acc.get(), delta);
                        self.wheel_acc.set(acc);
                        if dir != 0 && self.chrome.wheel_gate(dir) {
                            let a = if dir > 0 {
                                Action::OpacityUp
                            } else {
                                Action::OpacityDown
                            };
                            exec_action(
                                Some(ctx),
                                &self.viewer,
                                &self.chrome,
                                &self.config,
                                &self.hub,
                                a,
                            );
                        }
                    }
                    // 无（新增两行的默认）：吞掉这一记，不缩放不翻页不调透明度。
                    crate::core::config::WheelAction::None => {}
                }
                ctx.mark_dirty();
                true
            }
            PointerKind::Enter | PointerKind::Move => {
                // 光标先于一切分支：无论这一帧被哪一支吞掉，手型都要跟上指针。
                self.sync_hand(pos);
                // 取色模式（m217）：只记指针位置，其余悬停/拖图/按钮高亮一律不碰——
                // 那一套会在鼠标掠过浮条时反复整窗重绘，还会把左键解释成拖窗口。
                // 浮层必须跟着光标走，所以位置一变就标脏；同一位置复读不重画。
                if self.chrome.picker.get() {
                    if self.chrome.picker_at.get() != Some(pos) {
                        self.chrome.picker_at.set(Some(pos));
                        ctx.mark_dirty();
                    }
                    return true;
                }
                // 与 paint 同一份几何（`last_work`）：开条时工具栏/箭头/信息条上移一条，
                // 悬停、命中、绘制三者必须同进同出，否则点得到的画不到、画得到的点不到。
                // 显隐位一翻就当场标脏——"悬停立即浮现、移开立即收走"只有这一个触发点，
                // 不接就等于让浮条等别的重绘源（当年的"不流畅"就是这么来的）。
                if self.chrome_activity(pos) {
                    ctx.mark_dirty();
                }
                // 拖树的滚动指示（m307）：按行程比例走，指针跑到面板外也不会断。
                if let Some(s) = self.tree_scrub.get() {
                    let first = tree_scrub_first(&s, pos.1);
                    if self.tree_first.replace(first) != first {
                        ctx.mark_dirty();
                    }
                    return true;
                }
                // 文件树悬停（m300）：只在这一行的归属变了才重绘——树是一屏几十行，
                // 每次 Move 都标脏会把整窗画面拖进无谓重绘。
                let th = match self.tree_at.get() {
                    Some(tp) => {
                        let inside = tp.contains(windui::geometry::Point {
                            x: pos.0 as i32,
                            y: pos.1 as i32,
                        });
                        if inside {
                            let nodes = self.tree_nodes.borrow();
                            let cap = tree::capacity(tp.h, self.tree_truncated.get());
                            let first =
                                self.tree_first.get().min(tree::max_first(nodes.len(), cap));
                            tree::hit(tp, nodes.len(), first, pos, self.tree_truncated.get())
                        } else {
                            None
                        }
                    }
                    None => None,
                };
                if self.tree_hover.replace(th) != th {
                    ctx.mark_dirty();
                }
                // 缩略图条悬停高亮：只在这一格的归属变了才重绘。
                // 坐标已统一：filmstrip::layout 和 paint 坝均使用 work rect 坐标系
                // 直接使用 pos 进行命中测试，无需额外补偿
                let sh = filmstrip::hit(&self.strip_items.borrow(), pos);
                if self.strip_hover.replace(sh) != sh {
                    ctx.mark_dirty();
                }
                // 拖滚动指示的滑块（m259）：按行程比例走，指针跑到条外也不会断。
                if let Some(s) = self.strip_scrub.get() {
                    let first = scrub_first(&s, pos);
                    if self.strip_first.replace(Some(first)) != Some(first) {
                        ctx.mark_dirty();
                    }
                    return true;
                }
                // 缩略图条拖动（m258）：条内左键拖 = 平移条本身（看更远的图）。
                // ★ 必须排在拖图/拖窗之前：条上那一拖的语义是"翻列表"，而
                //   `press_left` 会把它判成 PanImage（拖条却动图，老行为）。
                if let Some(d) = self.strip_drag.get() {
                    let delta = if d.horizontal {
                        pos.0 - d.start.0
                    } else {
                        pos.1 - d.start.1
                    };
                    if self.strip_dragged.get() || beyond_slop(d.start, pos) {
                        self.strip_dragged.set(true);
                        let max_first = self.strip_len.get().saturating_sub(self.strip_cap.get());
                        let first = strip_drag_first(d.first, delta, d.pitch, max_first);
                        if self.strip_first.replace(Some(first)) != Some(first) {
                            ctx.mark_dirty();
                        }
                    }
                    return true;
                }
                // 鸟瞰拖动中：缩略图上的点跟随鼠标（优先级最高，吞掉拖图）。
                // 只有 pan 真的变了才标脏——拖到图边缘时 pan 钳在原地，画面一个像素
                // 都不动，原来每个 move 都整窗重绘一遍（M6）。
                // 真的变了再过一道限速：手速 120/s 时屏幕只有 60 Hz，多画的那几十帧
                // 呈现不出去、纯烧 CPU（M8）。
                if self.nav_drag.get() {
                    if self.nav_pan_at(pos) && self.chrome.pan_dirty() {
                        ctx.mark_dirty();
                    }
                    return true;
                }
                // 左键按下后挪够了距离：这一击定性为"拖窗口"，交给系统模态循环。
                //
                // 为什么在这里而不是按下时：见 [`DRAG_SLOP`]（双击不能被吞）。
                if let Some(start) = self.lmb_pending.get() {
                    if beyond_slop(start, pos) {
                        self.lmb_pending.set(None);
                        ctx.start_window_drag();
                        return true;
                    }
                }
                // 悬停高亮：与上次不同才重绘（纯 Move 不必整窗重画）。
                // ★ 三位分别赋值后再比，不许写成一条 `||`：`||` 短路会让后两位根本不
                //   被覆写，于是"从按钮上移到箭头/标题钮"那一跨，旧的高亮留在原处不掉。
                let hit = self.chrome_hit(pos);
                let (hb, ha, htb) = match hit {
                    Some(Press::Btn(b)) => (Some(b), None, None),
                    Some(Press::Arrow(a)) => (None, Some(a), None),
                    Some(Press::TitleBtn(tb)) => (None, None, Some(tb)),
                    None => (None, None, None),
                };
                let btn_changed = self.chrome.hover_btn.replace(hb) != hb;
                let arrow_changed = self.chrome.hover_arrow.replace(ha) != ha;
                let title_changed = self.chrome.hover_title_btn.replace(htb) != htb;
                if btn_changed || arrow_changed || title_changed {
                    ctx.mark_dirty();
                }
                // 平移图片的跟手帧。三种开局都汇到这一条：Ctrl+左键、中键（都不看开关），
                // 以及 `drag_image` 开着时的裸左键（[`left_drag`] 判成 PanImage 那一支）。
                // 已经开局的平移不因中途松开 Ctrl 而断，故判据里带上 `dragging()`。
                let panning = self.viewer.borrow().dragging();
                if p.kind == PointerKind::Move
                    && self.chrome.pressed.get().is_none()
                    && (panning || p.mods.ctrl || p.button == MouseButton::Middle)
                {
                    let changed = self.viewer.borrow_mut().drag_to(pos);
                    if changed && self.chrome.pan_dirty() {
                        ctx.mark_dirty();
                    }
                    return self.viewer.borrow().dragging();
                }
                true
            }
            PointerKind::Leave => {
                // 指针离开窗口（框架发一枚落在一切之外的 Move，这里收到 Leave）：悬停位
                // **全部**清掉并当场重绘。★ 原来那句注释写的"显隐交给定时器"是错的：
                // 定时器读的是滞留的 `last_pos`（还在那条栏里）⇒ 判成贴边豁免 ⇒ 永不消失，
                // 而清除那一路又被 `||` 短路挡住（逐位清的理由见 [`Chrome::pointer_left`]）。
                //
                // ★ 这里还要收掉**所有按下/拖动位**：窗口没有 SetCapture，指针拖出窗口后
                // 在外面松手时收不到 Up。只清悬停位会留下 `nav_drag`/`lmb_pending`/
                // `viewer.dragging()`——回到窗口后不动按钮也会继续拖图/拖窗，直到下一次点击。
                let was_nav = self.nav_drag.replace(false);
                let was_strip = self.strip_pressed.replace(None).is_some();
                let was_dragging_strip = self.strip_drag.replace(None).is_some();
                let was_scrub = self.strip_scrub.replace(None).is_some();
                let was_tree_scrub = self.tree_scrub.replace(None).is_some();
                self.strip_dragged.set(false);
                let was_pending = self.lmb_pending.replace(None).is_some();
                let was_comp_drag = self.comp_drag.replace(None).is_some();
                let was_comp_nav = self.comp_nav_drag.replace(false);
                let was_drag = self.viewer.borrow().dragging();
                if was_drag {
                    self.viewer.borrow_mut().end_drag();
                }
                let left = self.chrome.pointer_left();
                let strip = self.strip_hover.replace(None).is_some();
                if left
                    || strip
                    || was_nav
                    || was_strip
                    || was_dragging_strip
                    || was_scrub
                    || was_tree_scrub
                    || was_pending
                    || was_drag
                    || was_comp_drag
                    || was_comp_nav
                {
                    ctx.mark_dirty();
                }
                // 指针离窗：清掉取色采样点（回来时第一记 Move 会重新落点），
                // 手型交回系统箭头；取色模式仍开着，光标保持十字。
                self.chrome.picker_at.set(None);
                if self.chrome.picker.get() {
                    crate::platform::cursor::use_cross();
                } else {
                    super::hand_cursor::apply(None);
                }
                false
            }
            PointerKind::Down if p.button == MouseButton::Left => {
                // 按下就是插手（拖图/点按钮都算）； bare 移动**不算**——否则光标停在窗口里
                // 就把放映掐死了，而那不该是判据。
                self.chrome.slide_stop();
                // 取色模式（m217）：点画面 = 取色并复制 HEX，随即退出模式。
                // 浮条/标题钮/箭头仍让给原逻辑——点按钮不该被误当成取样。
                if self.chrome.picker.get() && self.chrome_hit(pos).is_none() {
                    let pick = self
                        .viewer
                        .borrow()
                        .pick_at(from_rect(self.view_work()), pos);
                    self.chrome.close_picker();
                    match pick {
                        Some(pk) => {
                            let hex =
                                format!("#{:02X}{:02X}{:02X}", pk.rgba[0], pk.rgba[1], pk.rgba[2]);
                            match clipboard::copy_text(&hex) {
                                Ok(()) => ctx.toast_ok(format!("已复制 {hex}")),
                                Err(e) => ctx.toast_err(e),
                            }
                        }
                        None => ctx.toast_err("这里没有图像像素"),
                    }
                    ctx.mark_dirty();
                    return true;
                }
                // 文件树（m300）：点一行 = 进那一层。命中与绘制共用 `tree_at`
                // （同一帧几何），所以点得到的一定看得见、看得见的一定点得到。
                if let Some(tp) = self.tree_at.get() {
                    let inside = tp.contains(windui::geometry::Point {
                        x: pos.0 as i32,
                        y: pos.1 as i32,
                    });
                    if inside {
                        // ★ m307：先判滚动指示——点在条上不是「进那一层」。
                        let notice = self.tree_truncated.get();
                        let n = self.tree_nodes.borrow().len();
                        let cap = tree::capacity(tp.h, notice);
                        let cur_first = self.tree_first.get().min(tree::max_first(n, cap));
                        if let Some((track, thumb)) =
                            tree::scroll_at(tp, cur_first, cap, n, notice, pos)
                        {
                            let span = (track.h - thumb.h).max(0) as f32;
                            let max_first = tree::max_first(n, cap);
                            let first = if thumb.contains(windui::geometry::Point {
                                x: pos.0 as i32,
                                y: pos.1 as i32,
                            }) {
                                tree_scrub_first(
                                    &TreeScrub {
                                        grab_at: pos.1,
                                        thumb0: thumb.y as f32,
                                        span,
                                        max_first,
                                    },
                                    pos.1,
                                )
                            } else {
                                tree_scrub_jump(
                                    pos.1,
                                    track.y as f32,
                                    thumb.h as f32,
                                    span,
                                    max_first,
                                )
                            };
                            self.tree_first.set(first);
                            // 跳转之后滑块换了位置 ⇒ `thumb0` 跟着换，否则再拖会先弹回。
                            let thumb0 = track.y as f32
                                + if max_first > 0 && span > 0.0 {
                                    (first as f32 / max_first as f32) * span
                                } else {
                                    0.0
                                };
                            self.tree_scrub.set(Some(TreeScrub {
                                grab_at: pos.1,
                                thumb0,
                                span,
                                max_first,
                            }));
                            ctx.mark_dirty();
                            return true;
                        }
                        let (segs, same) = {
                            let nodes = self.tree_nodes.borrow();
                            let cap = tree::capacity(tp.h, self.tree_truncated.get());
                            let first =
                                self.tree_first.get().min(tree::max_first(nodes.len(), cap));
                            match tree::hit(tp, nodes.len(), first, pos, self.tree_truncated.get())
                            {
                                Some(i) => {
                                    let s = nodes[i].segs.clone();
                                    let same =
                                        self.tree_segs.borrow().as_deref() == Some(s.as_slice());
                                    (Some(s), same)
                                }
                                None => (None, false),
                            }
                        };
                        // 点自己那一层不重开：那是"原地刷新"，只会让画面白闪一下。
                        if let (Some(segs), false) = (segs, same) {
                            let src = self
                                .viewer
                                .borrow()
                                .gallery_dir()
                                .and_then(crate::core::archive::here)
                                .map(|(s, _)| s);
                            if let Some(src) = src {
                                let v = self.viewer.clone();
                                open_layer_in(&v, &src, &segs);
                                ctx.mark_dirty();
                            }
                        }
                        return true;
                    }
                }
                if self.chrome_activity(pos) {
                    ctx.mark_dirty();
                }
                // 鸟瞰命中（优先于工具栏与拖图）：按下即导航并进入拖动态。
                //
                // ★ 走 `nav_grab` 而不是在这里内联 —— 那里明确了"调用期间不得持有
                // viewer 的 Ref"这条铁律，理由见它的注释。写成本作用域内联的
                // `if let Some(dst) = self.viewer.borrow()...` 就是一次进程崩溃。
                if !self.chrome.locked.get() && self.nav_grab(pos) {
                    self.nav_drag.set(true);
                    ctx.mark_dirty();
                    return true;
                }
                if let Some(hit) = self.chrome_hit(pos) {
                    match hit {
                        Press::TitleBtn(tb) => {
                            self.chrome.pressed_title_btn.set(Some(tb));
                        }
                        _ => {
                            self.chrome.pressed.set(Some(hit));
                        }
                    }
                    ctx.mark_dirty();
                    return true;
                }
                // 状态格（m275）：`(12/340)` 不只是看得见的进度，还是一颗**按钮**——
                // 点它弹「跳转到…帧/页」窗（与右键入口同名）。
                //
                // 判据用 `paint` 回写的真实矩形（[`Chrome::in_info_cell`]）：这帧没画 =
                // 零矩形 = 点不到（无边框形态 / 极简模式已收起 / 没有状态文本都自动落进来）。
                // 只有**有帧/页号**的图才吃掉这一击（`chrome.anim` 的语义就是 `has_index`，
                // 见 `paint` 里那条注释）：静态位图上按状态文字照旧是"拖窗口"，不该白吞。
                //
                // 排在这里：`chrome.hit` 已经把浮条按钮与箭头挑走了，状态格是浮条里
                // 剩下那一段；而 `press_left` 还没记账，所以这一击不会被当成"拖窗口"。
                if !self.chrome.locked.get()
                    && self.chrome.anim.get()
                    && self.chrome.in_info_cell(pos)
                {
                    // 辅图活动时状态格是辅图的页号：打开辅图自己的跳转窗，
                    // 不再把用户带进主图的对话框。
                    if self.companion_owns_index() {
                        super::goto_dialog::open_companion(ctx, &self.viewer, &self.companion);
                        return true;
                    }
                    super::goto_dialog::open(ctx, &self.viewer);
                    ctx.mark_dirty();
                    return true;
                }
                // 缩略图条（m258）：条是**独立滚动的列表**。
                //  - 按在条上（含格子之间的缝）就接管这一击：条内左键拖 = 平移条；
                //  - 老代码只拦"命中格子"，落在缝里的那一按会落到 `press_left`，
                //    于是"按住条往左拖，动的却是主画面"——那是对不上的手感。
                // ★ 仍排在双击关窗**之前**：条是常驻的一块地，落在它上面的一击属于
                //   "选图 / 翻列表"，不该被双击关窗吃掉（也没道理去拖窗口）。
                if !self.chrome.locked.get() && self.in_strip_panel(pos) {
                    // ① 滚动指示的热区优先（m259）：滑块按**行程比例**走，比按格平移精确。
                    //    用户实测的原始报障是"2dp 太细，鼠标对不准，而且滚轮也不动它"——
                    //    所以热区放到 16dp，并支持"点在轨道空白处直接跳过去"。
                    if let Some((track, thumb)) = self.strip_scroll_hit(pos) {
                        let side = self.strip_side.get().unwrap_or(filmstrip::Side::Bottom);
                        let horizontal = side == filmstrip::Side::Bottom;
                        let (t0, tlen, thlen) = if horizontal {
                            (track.x as f32, track.w as f32, thumb.w as f32)
                        } else {
                            (track.y as f32, track.h as f32, thumb.h as f32)
                        };
                        let max_first = self.strip_len.get().saturating_sub(self.strip_cap.get());
                        let span = (tlen - thlen).max(0.0);
                        let on_thumb = if horizontal {
                            pos.0 >= thumb.x as f32 && pos.0 < (thumb.x + thumb.w) as f32
                        } else {
                            pos.1 >= thumb.y as f32 && pos.1 < (thumb.y + thumb.h) as f32
                        };
                        let first = if on_thumb {
                            self.strip_shown_first()
                        } else {
                            scrub_jump(pos, t0, thlen, span, max_first, horizontal)
                        };
                        self.strip_first.set(Some(first));
                        // 跳转之后滑块在新位置 ⇒ `thumb0` 也要跟着换，否则再拖会先弹回。
                        let thumb0 = t0
                            + if max_first > 0 && span > 0.0 {
                                (first as f32 / max_first as f32) * span
                            } else {
                                0.0
                            };
                        self.strip_scrub.set(Some(StripScrub {
                            grab_at: if horizontal { pos.0 } else { pos.1 },
                            thumb0,
                            span,
                            max_first,
                            horizontal,
                        }));
                        self.strip_pressed.set(None);
                        ctx.mark_dirty();
                        return true;
                    }
                    // ② 条内其余位置：记格子 + 按格平移（m258）。
                    self.strip_pressed
                        .set(filmstrip::hit(&self.strip_items.borrow(), pos));
                    if self.strip_scrollable() {
                        let side = self.strip_side.get().unwrap_or(filmstrip::Side::Bottom);
                        self.strip_drag.set(Some(StripDrag {
                            start: pos,
                            first: self.strip_shown_first(),
                            pitch: filmstrip::pitch(side, &self.strip_items.borrow()[..]),
                            horizontal: side == filmstrip::Side::Bottom,
                        }));
                        self.strip_dragged.set(false);
                    }
                    ctx.mark_dirty();
                    return true;
                }
                // 双击：标题栏恒等「最大化 / 恢复」，画面区按设置对话框选的行为
                // （菠萝默认「关窗」，与我们的默认配置一致）。
                if p.click_count >= 2 {
                    if self.chrome.locked.get() {
                        return true; // 锁定：双击也不许关窗/最大化/全屏
                    }
                    // 标题栏判据与「拖窗口」共用 `Chrome::in_title_strip`（单一来源）。
                    let in_title = Chrome::in_title_strip(pos, self.work_tuple());
                    // 先拷出枚举、让 `Ref` 在这一行就结束：下面三个 `ctx.*` 都会同步动窗口
                    // （关闭 / 最大化 / 全屏），那条路上框架可能回头再借一次 config。
                    let dbl = double_click_action(in_title, self.config.borrow().dbl_click);
                    match dbl {
                        crate::core::config::DblClick::Nothing => {}
                        crate::core::config::DblClick::CloseWindow => ctx.request_close(),
                        crate::core::config::DblClick::Maximize => ctx.toggle_maximize(),
                        crate::core::config::DblClick::Fullscreen => ctx.toggle_fullscreen(),
                    }
                    return true;
                }
                // Ctrl+左键一直是"平移图片"的兜底手势，不看开关（见 Move 分支同款判据）。
                // 裸左键归谁由 [`left_drag`] 判：开关开着、这张图有可平移余量、且不在标题栏
                // 那一条里 ⇒ 拖图片；否则照旧先记账，挪够阈值升级成拖窗口。
                if self.chrome.locked.get() {
                    return true; // 锁定：吞掉这一击，不记拖窗账、不开平移
                }
                self.press_left(pos, self.work_tuple(), p.mods.ctrl);
                // 按下即换手：`press_left` 已经把平移局面打开了（判成 PanImage 时），
                // 所以此刻读 `dragging()` 拿到的就是这一按的结果。
                self.sync_hand(pos);
                true
            }
            // 中键拖动平移图片（不看修饰键）。
            PointerKind::Down if p.button == MouseButton::Middle => {
                if self.chrome.locked.get() {
                    return true; // 锁定：中键平移也不留
                }
                if self.chrome_activity(pos) {
                    ctx.mark_dirty();
                }
                self.viewer.borrow_mut().begin_drag(pos);
                true
            }
            PointerKind::Up if p.button == MouseButton::Left => {
                // 没挪够阈值的一击（单击 / 双击序列）：清账，不当拖动看。
                self.lmb_pending.set(None);
                // 拖树的滚动指示（m307）：松手即收，绝不当成"点行进层"。
                if self.tree_scrub.take().is_some() {
                    self.sync_hand(pos);
                    ctx.mark_dirty();
                    return true;
                }
                // 拖滚动指示的滑块（m259）：松手即收，绝不当作"点格子"。
                if self.strip_scrub.take().is_some() {
                    self.strip_pressed.set(None);
                    self.sync_hand(pos);
                    ctx.mark_dirty();
                    return true;
                }
                // 缩略图条（m258）：拖动收尾。真拖过 ⇒ 吞掉这一击（不换图）；
                // 没越过阈值 ⇒ 继续往下走"点格子"，条上单击照旧选图。
                if self.strip_drag.take().is_some() && self.strip_dragged.replace(false) {
                    self.strip_pressed.set(None);
                    self.sync_hand(pos);
                    ctx.mark_dirty();
                    return true;
                }
                if self.nav_drag.take() {
                    // 松手这一帧必须画出去：手停下之后没有"下一条 move"来补挂了。
                    if self.chrome.flush_pan() {
                        ctx.mark_dirty();
                    }
                    return true;
                }
                // 处理标题栏按钮释放
                if let Some(tb) = self.chrome.pressed_title_btn.take() {
                    if self.chrome_hit(pos) == Some(Press::TitleBtn(tb)) {
                        exec_title_btn(ctx, &self.chrome, &self.config, &self.hub, tb);
                    } else {
                        ctx.mark_dirty();
                    }
                    return true;
                }
                if let Some(press) = self.chrome.pressed.take() {
                    // 抬起时仍命中同一目标才算点击（与系统按钮行为一致）。
                    if self.chrome_hit(pos) == Some(press) {
                        self.exec(ctx, press);
                    } else {
                        ctx.mark_dirty();
                    }
                    return true;
                }
                // 缩略图条：抬起仍在同一格才跳（与工具栏按钮同一套约定）。
                if let Some(i) = self.strip_pressed.take() {
                    // 坐标已统一：filmstrip::layout 和 paint 坝均使用 work rect 坐标系
                    // 直接使用 pos 进行命中测试，无需额外补偿
                    if filmstrip::hit(&self.strip_items.borrow(), pos) == Some(i) {
                        let idx = self.strip_items.borrow().get(i).map(|(idx, _)| *idx);
                        if let Some(idx) = idx {
                            self.viewer.borrow_mut().goto(idx);
                        }
                    }
                    ctx.mark_dirty();
                    return true;
                }
                let was = self.viewer.borrow().dragging();
                self.viewer.borrow_mut().end_drag();
                // 松手就把手型放回去：此时 `dragging()` 已归位，`sync_hand` 会判成
                // "还在可拖区 ⇒ 松手"或"不在 ⇒ 箭头"。**不能等下一次 Move**——
                // 松手时指针常常是静止的，那一帧没有 Move 来补。
                self.sync_hand(pos);
                // 平移手势收尾：同鸟瞰拖动，挂起的那一帧要在松手这一下画出去。
                if self.chrome.flush_pan() {
                    ctx.mark_dirty();
                }
                was
            }
            PointerKind::Up if p.button == MouseButton::Middle => {
                let was = self.viewer.borrow().dragging();
                self.viewer.borrow_mut().end_drag();
                // 松手就把手型放回去：此时 `dragging()` 已归位，`sync_hand` 会判成
                // "还在可拖区 ⇒ 松手"或"不在 ⇒ 箭头"。**不能等下一次 Move**——
                // 松手时指针常常是静止的，那一帧没有 Move 来补。
                self.sync_hand(pos);
                // 平移手势收尾：同鸟瞰拖动，挂起的那一帧要在松手这一下画出去。
                if self.chrome.flush_pan() {
                    ctx.mark_dirty();
                }
                was
            }
            PointerKind::Down if p.button == MouseButton::Right => {
                if self.chrome.locked.get() {
                    return true; // 锁定：不弹菜单
                }
                if self.chrome_activity(pos) {
                    ctx.mark_dirty();
                }
                self.context_menu(ctx, p.pos);
                true
            }
            _ => false,
        }
    }
}

/// 键盘动作表。**焦点在画布与在按钮上时都要能翻页**，故两处共用这一张表。
///
/// 返回是否消费：未消费的键继续走框架的默认行为（Escape 关窗、Tab 换焦点…）。
///
/// 键→动作的映射改走 [`crate::core::hotkey::Hotkeys`]（设置页可改、唯一性由
/// `Hotkeys::set_binding` 保证），不再写死在 `match` 里。
///
/// `ctx` 仅在「全屏」这类需要改窗口态的动作里用到，故用 `Option`：测试里没有
/// `EventCtx` 时传 `None`，此时全屏动作静默放行（交给框架默认处理），不影响其它动作。
pub fn handle_key(
    mut ctx: Option<&mut EventCtx>,
    // 每窗一份的实时调色状态：热键打开参数窗时要和看图窗共用同一份（`None` 只出现在
    // 没有画布上下文的兜底路径与测试里，此时退回"起进程"的老路）。
    live: Option<&LiveState>,
    viewer: &Rc<RefCell<Viewer>>,
    chrome: &Rc<Chrome>,
    cfg: &Rc<RefCell<Config>>,
    hub: &Hub,
    ev: &KeyEvent,
) -> bool {
    // 锁定：键盘全封。返回 true = 已消费——`App::on_shortcut` 这条兜底路
    // 排在框架 Tab/Escape 之前，消费掉框架就收不到 Esc 关窗键。
    if chrome.locked.get() {
        return true;
    }
    let Some(ki) = to_key_input(ev) else {
        return false;
    };
    // 借出快捷键表做匹配，**先放掉借用**再执行：CopyImage/Properties 等动作执行时
    // 要再借 viewer / 开窗口，若此处 Ref 还活着会撞 `already borrowed`（UI 线程 panic = 窗口消失）。
    let matched = cfg.borrow().hotkeys.match_action(&ki);
    match matched {
        Some(a) => {
            // 键盘翻页限速（m81）：按住自重复压到每个 KEY_PACE 一张，一记一记的手按不动。
            // 这里返回 true = 已消费，但**不等于翻过了**：被压住的方向记在闸里，由
            // `Chrome::tick_key_pace` 到点兜出去（同 m64 滚轮闸那条界线：挂起，不丢步）。
            let nav = match a {
                Action::Prev => Some(-1),
                Action::Next => Some(1),
                _ => None,
            };
            if let Some(dir) = nav {
                if !chrome.key_gate(dir) {
                    return true;
                }
            }
            exec_action(ctx, viewer, chrome, cfg, hub, a)
        }
        None => {
            // 动态（插件）热键：插件在 manifest 里声明的快捷键。
            // 内置快捷键优先——上面已经没命中，才轮到插件。
            // 借用只在这一个块里：把插件 id 复制出来，后面还要再借 cfg / viewer。
            let plugin_id = {
                let c = cfg.borrow();
                crate::extensions::hotkey_table(&c)
                    .match_plugin(&ki)
                    .map(|e| e.plugin_id.clone())
            };
            let Some(plugin_id) = plugin_id else {
                return false;
            };
            // ★ 实时参数型编辑插件：热键与右键菜单走同一条路——打开它自己的参数窗。
            //   `run_by_hotkey` 只会 `start_command`，对调色这类插件等于"跑一遍没有
            //   界面的滤镜"，用户看不到任何可调的东西（2026-11-06 用户要"快捷键设置"）。
            //   没有画布上下文（`live`/`ctx` 缺一）时退回老路，不假装开过窗。
            if let (Some(state), Some(ctx_ref)) = (live, ctx.as_mut()) {
                let target = crate::extensions::discover_configured(&cfg.borrow())
                    .into_iter()
                    .find(|e| e.manifest.id == plugin_id);
                if let Some(ext) = target {
                    if ext.manifest.run_ui && ext.manifest.preview.is_some() {
                        let cur = viewer.borrow().current_path();
                        // 另存 / 还原与右键菜单同源：从热键进来也不该少这两颗按钮。
                        let actions = cur.as_ref().map(|c| edit_actions(viewer, chrome, state, c));
                        super::plugin_settings::show_run_prompt(
                            ctx_ref,
                            &ext,
                            cfg,
                            RunPrompt {
                                state: state.clone(),
                                path: cur.unwrap_or_default(),
                                on_apply: None,
                                actions,
                            },
                        );
                        log::info!("[plugin-hotkey] id={} verdict=ui", ext.manifest.id);
                        return true;
                    }
                }
            }
            let path = viewer.borrow().current_path();
            // 快捷键拉起的那一路也要能给出整图（与右键菜单同一份判据、同一次变换）。
            // ★ 同样**不在这里过 8 MP 闸**：右键菜单那条注释说明了理由，
            //   判据只在 `pixels_to_send`——两处各滤一道就是两个正本。
            let pixels = viewer.borrow().whole_image_for_plugin();
            let outcome = {
                let c = cfg.borrow();
                crate::extensions::run_by_hotkey(&c, &ki, path, pixels)
            };
            match outcome {
                Ok(msg) => {
                    log::info!("[plugin-hotkey] {msg}");
                    if let Some(ctx) = ctx {
                        ctx.toast_ok(msg);
                    }
                }
                Err(e) => {
                    log::warn!("[plugin-hotkey] {e}");
                    if let Some(ctx) = ctx {
                        ctx.toast_err(e);
                    }
                }
            }
            true
        }
    }
}

/// 「复制当前（PNG）」该对用户说什么——**这一步不碰 `ctx`**。
///
/// 为什么要绕这一层：两条调用路的 ctx 类型不一样（菜单项在闭包里拿到 `EventCtx`，
/// 快捷键那条是 `Option<&mut EventCtx>`），塞进同一个签名不如让两边各把这句话发出去，
/// 于是"同一口径"落在**行为**上（同一条判据、同一份文案），而不是落在同一个函数签名上。
enum CopyNow {
    /// 屏上没有图（菜单本来就该是 disabled，这里算防御）。
    Nothing,
    /// 小图：当场编完了，结果就是要说的那个结果。
    Sync(Result<(), String>),
    /// 大图：已经交给一次性线程，先给用户这句（带尺寸）；
    /// 落地那一刻由 16 ms 轮询兜回来（[`Chrome::tick_copy`]）。
    Posted(String),
    /// 上一枚还在编。如实说"这一次没排上"——悄悄丢掉就是用户最难查的"按了没反应"。
    Busy,
    /// 线程起不来。**不**退回当场同步编（那等于把本轮要消掉的那次长停顿请回来），
    /// 也不假装成功。
    SpawnFailed(String),
}

/// 复制屏上那份位图：≤ [`chrome::COPY_SYNC_PIXELS`] 当场编，超过就交给后台线程。
///
/// 分界的理由全是实测：默认压缩档下 1620×1080 要 **828 ms**、36.7 MP 要 **8.05 秒**
/// （全都停在 UI 线程上，8 秒那一档 Windows 会把窗口标成"未响应"）；换成
/// `Compression::Fastest` 之后是 25 ms 与 612 ms。也就是说**改档治掉了日常形态**，
/// 而 100% 看巨图那一形仍然要挪线程才停得干净。
fn copy_decision(pixels: u64, busy: bool) -> Option<chrome::CopyRoute> {
    // 单飞闸必须先于大小分流，否则新小图会被旧大图的后台结果覆盖。
    if busy {
        None
    } else {
        Some(chrome::copy_route(pixels))
    }
}

fn copy_display(chrome: &Chrome, viewer: &RefCell<Viewer>) -> CopyNow {
    let img = {
        let v = viewer.borrow();
        v.image().map(|i| (i.width, i.height, Arc::clone(&i.rgba)))
    };
    let Some((w, h, rgba)) = img else {
        return CopyNow::Nothing;
    };
    let px = w as u64 * h as u64;
    match copy_decision(px, chrome.copy_in_flight()) {
        Some(chrome::CopyRoute::Sync) => return CopyNow::Sync(clipboard::copy_png(w, h, &rgba)),
        None => return CopyNow::Busy,
        Some(chrome::CopyRoute::Threaded) => {}
    }
    let note = format!("正在后台复制整张画面（{w}×{h}）…");
    let tx = chrome.copy_posted(note.clone());
    let spawned = std::thread::Builder::new().spawn(move || {
        // 走到这里只可能有一枚在飞：第二枚在上面那条 `copy_in_flight` 就被拒了，
        // 所以线程不必问"我还是最新的那一枚吗"，编完直接占剪贴板。
        let out = match clipboard::encode_png(w, h, &rgba) {
            Err(e) => Err(e),
            Ok(png) => clipboard::put_png(&png, w, h, &rgba),
        };
        let _ = tx.send(out);
    });
    match spawned {
        Ok(_) => CopyNow::Posted(note),
        Err(e) => {
            chrome.copy_reset();
            CopyNow::SpawnFailed(format!("后台复制起不动：{e}"))
        }
    }
}

/// 执行一个快捷键动作。与 `handle_key` 分离：被 `on_event`（有 `EventCtx`）与
/// `App::on_shortcut`（兜底，`ctx` 为 `ShortcutCtx` 而非 `EventCtx`）共用同一份语义。
///
/// 需要 `EventCtx` 的动作（开对话框 / 改窗口态 / 复制 / 属性窗 / 设置窗 / 不透明度）在
/// **兜底路径拿不到 ctx** 时返回 `false` 放行给框架，与旧版「全屏」同款处理；焦点在画布
/// （`on_event`）时 ctx 为 `Some`，正常执行。只依赖 `chrome`/`viewer` 的动作两路都能执行。
fn exec_action(
    ctx: Option<&mut EventCtx>,
    viewer: &Rc<RefCell<Viewer>>,
    chrome: &Rc<Chrome>,
    cfg: &Rc<RefCell<Config>>,
    hub: &Hub,
    a: Action,
) -> bool {
    // 用户亲手按下一记快捷键就算"插手画面"，幻灯片当场让位（见 `Chrome::slide_stop`）。
    // 唯一的例外是放映自己——它的开与关由下面那一臂的 toggle 表达，不该先停再开。
    if a != Action::Slideshow {
        chrome.slide_stop();
    }
    match a {
        // ── 仅动 viewer：各臂内部临时 borrow_mut，绝不跨臂持有借用 ──
        Action::Prev => {
            let pair = pair_step(chrome.companion_source.get(), chrome.companion_link.get());
            let paired = !chrome::borderless()
                && chrome.companion.get()
                && pair > 1
                && !viewer.borrow().anim_info().is_some_and(|a| a.paged);
            viewer.borrow_mut().step(if paired { -pair } else { -1 });
            true
        }
        Action::Next => {
            let pair = pair_step(chrome.companion_source.get(), chrome.companion_link.get());
            let paired = !chrome::borderless()
                && chrome.companion.get()
                && pair > 1
                && !viewer.borrow().anim_info().is_some_and(|a| a.paged);
            viewer.borrow_mut().step(if paired { pair } else { 1 });
            true
        }
        Action::Companion => {
            // 无边框不开放主辅视图：单图极简形态没有 pane/chrome 可承载。
            if chrome::borderless() {
                return true;
            }
            let on = !chrome.companion.get();
            chrome.companion.set(on);
            if !on {
                chrome.active_pane.set(Pane::Primary);
            }
            if let Some(ctx) = ctx {
                ctx.mark_dirty();
            }
            true
        }
        // 幻灯片放映：**这里只开关，不翻页**。到点由 16 ms 轮询的 `Chrome::tick_slideshow`
        // 发那一步，走的还是键盘/滚轮同一条 `Viewer::step`——所以放映天然继承"末尾回绕"，
        // 也不需要第二条翻页实现，更不需要一个定时器线程（宪章：不引常驻线程）。
        Action::Slideshow => {
            chrome.toggle_slideshow();
            // 状态一翻就要重绘：右侧那颗持续亮起的箭头是放映唯一的状态面（无文字提示）。
            if let Some(ctx) = ctx {
                ctx.mark_dirty();
            }
            true
        }
        Action::First => {
            viewer.borrow_mut().jump(Jump::First);
            true
        }
        Action::Last => {
            viewer.borrow_mut().jump(Jump::Last);
            true
        }
        // 以视口中心为锚点缩放：没有锚点时（按钮触发）让画面中心保持不动。
        Action::ZoomIn => {
            let mut v = viewer.borrow_mut();
            let z = v.scale() * 1.25;
            v.zoom_at_center(z);
            true
        }
        Action::ZoomOut => {
            let mut v = viewer.borrow_mut();
            let z = v.scale() / 1.25;
            v.zoom_at_center(z);
            true
        }
        // 与工具栏 `Btn::Fit`、右键菜单「适应窗口」同义：置真，不是 toggle。
        // 要 1:1 有独立的 `1`（ActualSize）；三个入口各说各话会让"已经在适应窗口
        // 时按 0 反而变成原尺寸"成为暗坑（m51-E）。
        Action::Fit => {
            viewer.borrow_mut().set_fit(true);
            true
        }
        Action::ActualSize => {
            viewer.borrow_mut().zoom_actual();
            true
        }
        Action::Rotate => {
            viewer.borrow_mut().rotate_cw();
            true
        }
        Action::Flip => {
            viewer.borrow_mut().toggle_flip();
            true
        }
        Action::PauseAnim => {
            viewer.borrow_mut().toggle_anim_pause();
            true
        }
        // 逐帧浏览：按下即进暂停态（不暂停的话，手动退一步立刻又被播放调度拽回去）。
        // 明写 `true`：本函数的 bool 是「这个键消费了没」，画布据此决定要不要再让
        // `App::on_shortcut` 走一遍——早先这里透传了 `step_anim` 的返回值，等于报"没消费"，
        // 同一次按键被两条路各执行一次（按一下退两帧）。
        Action::FramePrev => {
            let pair = pair_step(chrome.companion_source.get(), chrome.companion_link.get());
            let paired = !chrome::borderless()
                && chrome.companion.get()
                && pair > 1
                && viewer.borrow().anim_info().is_some_and(|a| a.paged);
            viewer
                .borrow_mut()
                .step_anim(if paired { -pair as i32 } else { -1 });
            true
        }
        Action::FrameNext => {
            let pair = pair_step(chrome.companion_source.get(), chrome.companion_link.get());
            let paired = !chrome::borderless()
                && chrome.companion.get()
                && pair > 1
                && viewer.borrow().anim_info().is_some_and(|a| a.paged);
            viewer
                .borrow_mut()
                .step_anim(if paired { pair as i32 } else { 1 });
            true
        }
        // WASD 平移（m216）：一记一步，范围与拖动共用同一份钳位（`pan_limits`）。
        // 连按到边随做随撤（用户实测不好用，m216 同日），不再有状态机与轮询兜底。
        // 返回 `true` = 已消费，画布不再让 `App::on_shortcut` 重复走一遍——透传
        // 返回值的历史教训见 FramePrev 上方。
        Action::PanUp | Action::PanLeft | Action::PanDown | Action::PanRight => {
            let (sx, sy) = Chrome::wasd_signs(wasd_dir(a));
            let _ = viewer.borrow_mut().pan_nudge(sx, sy);
            true
        }
        // 缩略图条：会话态开关（m53 起落盘，见 `persist_session`）。
        Action::Filmstrip => {
            let on = !chrome.filmstrip.get();
            chrome.filmstrip.set(on);
            persist_session(cfg, hub, |c| c.filmstrip = on);
            true
        }
        Action::Checker => {
            let on = !chrome.checker.get();
            chrome.checker.set(on);
            persist_session(cfg, hub, |c| c.checker = on);
            true
        }
        // 取色器（m217）：与右键「本图 ▸ 取色器」共用一个 toggle，不另写一份状态。
        Action::Picker => {
            chrome.toggle_picker();
            if let Some(c) = ctx {
                c.mark_dirty();
            }
            true
        }
        Action::Minimal => {
            // m330：与菜单 / 设置页写同一份 `minimal_mode`（旧版这里也只在 chrome 里翻）。
            let on = !cfg.borrow().minimal_mode;
            chrome.minimal_mode.set(on);
            persist_session(cfg, hub, |c| c.minimal_mode = on);
            // 不提示：与 `Action::Lock`（锁窗口）同款——状态切换当场可见，
            // 屏幕自己已经说了。锁窗口从来不提示，也从没人抱怨看不见。
            if let Some(c) = ctx {
                c.mark_dirty();
            }
            true
        }
        Action::Lock => {
            let on = !chrome.locked.get();
            chrome.locked.set(on);
            persist_session(cfg, hub, |c| c.locked = on);
            chrome.pressed_title_btn.set(None);
            if let Some(c) = ctx {
                c.mark_dirty();
            }
            true
        }
        // ── 需要 EventCtx：兜底路径放行（返回 false）──
        Action::Fullscreen => ctx.is_some_and(|c| {
            c.toggle_fullscreen();
            true
        }),
        // m330：与标题栏 📌 / 设置页写同一份 `stay_on_top`；窗口样式由 runtime 差分下发。
        Action::Topmost => {
            let on = !cfg.borrow().stay_on_top;
            persist_session(cfg, hub, |c| c.stay_on_top = on);
            true
        }
        Action::Minimize => ctx.is_some_and(|c| {
            c.minimize();
            true
        }),
        Action::Maximize => ctx.is_some_and(|c| {
            c.toggle_maximize();
            true
        }),
        Action::Close => ctx.is_some_and(|c| {
            // Esc 出厂绑到本动作。全屏里先退全屏（看图器通用约定 + 极简模式安全网），
            // 不在全屏才真关窗——否则按一次 Esc 会在全屏查看时直接退出程序。
            if window_state().fullscreen {
                c.toggle_fullscreen();
            } else {
                c.request_close();
            }
            true
        }),
        Action::CopyImage => {
            let Some(c) = ctx else { return false };
            // 与右键菜单那条路共用 `copy_display`：同一份判据（小图当场编、
            // 大图交给后台线程），两份文案不该各自漂。
            match copy_display(chrome, viewer) {
                CopyNow::Nothing => {}
                CopyNow::Sync(Ok(())) => c.toast_ok("当前画面已复制（PNG）"),
                CopyNow::Sync(Err(e)) => c.toast_err(e),
                CopyNow::Posted(note) => c.toast_ok(note),
                CopyNow::Busy => c.toast_err("上一次复制还在进行，等它结束再按"),
                CopyNow::SpawnFailed(e) => c.toast_err(e),
            }
            true
        }
        Action::CopyPath => {
            let Some(c) = ctx else { return false };
            let path = viewer.borrow().current_path();
            if let Some(p) = path {
                match clipboard::copy_text(&p.to_string_lossy()) {
                    Ok(()) => c.toast_ok("路径已复制到剪贴板"),
                    Err(e) => c.toast_err(e),
                }
            }
            true
        }
        Action::Properties => match ctx {
            Some(c) => {
                settings::open_metadata(c, viewer);
                true
            }
            None => false,
        },
        Action::Settings => match ctx {
            Some(c) => {
                settings::open_settings(c, cfg, hub);
                true
            }
            None => false,
        },
        // 打开对话框：与右键菜单「打开…」同一份实现（裸数据无参数弹窗压队列补弹）。
        Action::Open => {
            let Some(c) = ctx else { return false };
            let v2 = viewer.clone();
            let cfg2 = cfg.clone();
            let exts = crate::core::archive::pick_exts();
            c.request_pick_file(
                windui::prelude::PickDialog::new()
                    .title("打开图片")
                    .filter("图片", &exts),
                move |p| {
                    if let Some(p) = p {
                        super::raw_dialog::open_path(&v2, &cfg2, &p);
                    }
                },
            );
            true
        }
        // 不透明度 ±1 档（10%）：读当前值步进、量化、落盘 + 推设置页 + 上屏。
        Action::OpacityUp | Action::OpacityDown => {
            let Some(c) = ctx else { return false };
            let step: i16 = if a == Action::OpacityUp { 10 } else { -10 };
            let cur = cfg.borrow().opacity as i16;
            let pct = crate::core::config::quantize_opacity((cur + step).clamp(
                crate::core::config::OPACITY_MIN as i16,
                crate::core::config::OPACITY_MAX as i16,
            ) as u8);
            {
                let mut cc = cfg.borrow_mut();
                cc.opacity = pct;
            }
            crate::core::config::mark_dirty();
            settings::push_to_hub(hub, &cfg.borrow());
            c.set_window_opacity(pct);
            // 不提示：窗口当场变透，屏幕自己已经说了。
            true
        }
    }
}

/// 进压缩包某一层的统一动作：展开 `source` 里 `segs` 指向的那一层再打开。
///
/// 入口是**左侧文件树**点一行（m300 撤掉右键「本包 ▸」之后只剩这一处）。
/// 空层不在这里拦——[`Viewer::open_layer`] 认它（画面空 + 一句提示，树里还能继续往下）。
/// 展开失败只报错、不清画面：这一步没成，用户还在原处。
fn open_layer_in(viewer: &Rc<RefCell<Viewer>>, source: &Path, segs: &[String]) {
    match crate::core::archive::enter(source, segs) {
        Ok(layer) => viewer.borrow_mut().open_layer(&layer),
        Err(e) => viewer.borrow_mut().report_error(e),
    }
}

/// WASD 四个动作 → 方向编码（0=上 1=左 2=下 3=右）。
/// 符号表在 [`Chrome::wasd_signs`]——这里只认动作，不碰符号，两份表各管一半。
fn wasd_dir(a: Action) -> u8 {
    match a {
        Action::PanUp => 0,
        Action::PanLeft => 1,
        Action::PanDown => 2,
        Action::PanRight => 3,
        _ => u8::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本文件的生产代码文本（`include_str!` 会把测试自身也包进来）。
    fn production_src() -> &'static str {
        let all = include_str!("canvas.rs");
        let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
        &all[..cut]
    }

    /// m263：共同序的**唯一正本**。设置页与右键标签不完全同名，但期望序只写这一处；
    /// 负向臂也取同一份常量，免得有人把期望序改回旧序就能悄悄绕过钉子。
    const APPEARANCE_PAGE_KEYS: [&str; 6] = [
        "\"显示操作按钮\"",
        "\"极简模式\"",
        "\"棋盘格背景\"",
        "\"缩略图条\"",
        "\"主题色\"",
        "\"窗口不透明度\"",
    ];
    const APPEARANCE_MENU_KEYS: [&str; 6] = [
        "\"显示操作按钮\"",
        "\"极简模式\"",
        "\"棋盘格背景\"",
        "\"缩略图条\"",
        "\"主题色\"",
        "\"不透明度\"",
    ];

    /// 滤掉整行注释，只留正文：注释里也会写项序，但那不是接线。
    fn code_lines(src: &str) -> String {
        src.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 按期望序在正文里**递增**找标签：缺一项、改名、或把某颗挪到前面都会红。
    fn assert_order(src: &str, keys: &[&str], what: &str) {
        let mut at = 0;
        for k in keys {
            let found = src[at..]
                .find(k)
                .unwrap_or_else(|| panic!("{what} 里找不到 {k}（项被删了或改名了？）"));
            at += found + k.len();
        }
    }

    /// m262：设置页「界面」卡与右键「界面 ▸」前六项必须是同一条项序。
    /// 两处各排各的、时间一长必漂；这是源码级接线钉子，改一边就红。
    #[test]
    fn appearance_order_matches_between_settings_and_menu() {
        let page = code_lines(include_str!("settings/options.rs"));
        let page = page
            .split("fn appearance_body")
            .nth(1)
            .expect("设置页界面卡");
        assert_order(page, &APPEARANCE_PAGE_KEYS, "设置页界面卡");
        let menu = code_lines(production_src());
        let menu = menu
            .split("fn appearance_items")
            .nth(1)
            .expect("右键外观子菜单");
        assert_order(menu, &APPEARANCE_MENU_KEYS, "右键外观子菜单");
    }

    /// m263 负向臂：拿 m262 的旧序（显示操作按钮排第四）喂同一个断言，必须报红。
    /// 也钉住常量本身——把 APPEARANCE_PAGE_KEYS 改回旧序，这里不再 panic 而失败。
    #[test]
    #[should_panic(expected = "里找不到")]
    fn appearance_order_rejects_show_buttons_in_the_fourth_place() {
        let old = [
            "\"极简模式\"",
            "\"棋盘格背景\"",
            "\"缩略图条\"",
            "\"显示操作按钮\"",
            "\"主题色\"",
            "\"窗口不透明度\"",
        ]
        .join("\n");
        assert_order(&old, &APPEARANCE_PAGE_KEYS, "m262 旧序模拟");
    }

    /// m327：顶层 9 项、四块、三条线。项序就是阅读顺序：文件 → 最近 → 排序 →（线）→
    /// 画面 → 本图 → 插件 →（线）→ 视图 → 主辅视图 →（线）→ 设置。
    ///
    /// ★ 「最近 ▸」「排序 ▸」紧跟「文件 ▸」：三者都是"挑下一步看什么"；但它们必须
    ///   留在顶层——本身是子菜单，收进「文件 ▸」就成了三级，而本地 13 个竞品样本里
    ///   没有一家右键菜单超过两级。
    const TOP_MENU_KEYS: [&str; 9] = [
        "\"文件\"",
        "self.recent_menu()",
        "self.sort_menu()",
        "\"画面\"",
        "\"本图\"",
        "\"插件\"",
        "\"界面\"",
        "\"主辅视图\"",
        "\"设置…\"",
    ];

    /// m327：「文件 ▸」组内顺序：打开 / 系统菜单 →（线）→ 复制×3 →（线）→
    /// 属性 → 清除本书阅读位置（m333）。
    /// `复制` 已拍平，不再有自己的子菜单标签；锚点用 `self.copy_items(`。
    /// 排序已独立成顶层一项，不在本组。
    const FILE_GROUP_MENU_KEYS: [&str; 5] = [
        "\"打开…\"",
        "\"系统菜单\"",
        "self.copy_items(",
        "\"属性…\"",
        "\"清除本书阅读位置\"",
    ];

    /// m327：「排序 ▸」组内 = 四个维度 →（线）→ 倒序。
    const SORT_MENU_KEYS: [&str; 3] = ["SortBy::ALL", "MenuItem::separator()", "\"倒序\""];

    /// m327：「画面 ▸」= 线前 10 项（`Btn::ALL`）→（线）→ 拖图 / 放映。
    /// 前段接的是 `ops_row`；线后两项必须按这个顺序出现。
    const OPS_TAIL_MENU_KEYS: [&str; 4] = [
        "ops_row(has_index)",
        "\"左键拖图\"",
        "\"停止放映\"",
        "\"动图/文档播放速度\"",
    ];

    /// m276 / m280：「本图 ▸」组内四项的次序。三个"打开一个窗"的常驻项在前、
    /// 会话开关垫底；改名 / 挪位 / 少一项都会红。
    const CURRENT_IMAGE_MENU_KEYS: [&str; 4] = [
        "\"RAW 参数…\"",
        "\"跳转到…帧/页\"",
        "\"旋转角度…\"",
        "\"取色器\"",
    ];

    /// 顶层不许再出现收编进「本图 ▸」的两项，也不许留旧名「工具 ▸」；
    /// 组内四项按 [`CURRENT_IMAGE_MENU_KEYS`] 递增。正向喂生产源码，负向臂喂旧布局。
    fn assert_current_image_menu(src: &str) {
        let menu = src.split("fn context_menu").nth(1).expect("右键菜单");
        let top = menu.split("fn file_items").next().unwrap_or(menu);
        // 只认标签与接线名，不认整串 `MenuItem::submenu("本图", …)`：
        // `cargo fmt` 会把这行按参数折成多行，整串匹配会假红。
        assert!(
            top.contains("\"本图\"") && top.contains("self.current_image_items("),
            "「本图 ▸」不在顶层或没接到组函数"
        );
        assert!(
            !top.contains("\"工具\"") && !top.contains("self.tool_items("),
            "旧名「工具 ▸」还挂在顶层"
        );
        assert!(
            !top.contains("\"RAW 参数…\""),
            "RAW 参数… 仍是顶层项，没有收进「本图 ▸」"
        );
        assert!(
            !top.contains("\"跳转到…帧/页\""),
            "跳转仍是顶层项，没有收进「本图 ▸」"
        );
        let group = src
            .split("fn current_image_items")
            .nth(1)
            .expect("本图子菜单");
        let group = group.split("fn copy_items").next().unwrap_or(group);
        assert_order(group, &CURRENT_IMAGE_MENU_KEYS, "本图 ▸ 组内");
    }

    #[test]
    fn top_level_is_nine_items_and_each_group_keeps_its_order() {
        let src = code_lines(production_src());
        let menu = src.split("fn context_menu").nth(1).expect("右键菜单");
        let top = menu.split("fn file_items").next().unwrap_or(menu);
        assert_order(top, &TOP_MENU_KEYS, "右键菜单顶层");
        assert!(
            !top.contains("\"外观\"") && !top.contains("\"操作\""),
            "m327 起顶层叫「界面」「画面」，旧名不该再出现"
        );
        assert!(
            !top.contains("\"左键拖图\"") && !top.contains("\"幻灯片放映\""),
            "放映与拖图还在顶层，没有收进「画面 ▸」"
        );

        let group = src.split("fn file_items").nth(1).expect("文件子菜单");
        let group = group.split("\n    fn ").next().unwrap_or(group);
        assert_order(group, &FILE_GROUP_MENU_KEYS, "文件 ▸ 组内");
        assert!(
            !group.contains("SortBy::ALL"),
            "排序被摊回「文件 ▸」了：m327 起它是顶层独立一项"
        );

        let sort = src.split("fn sort_menu").nth(1).expect("排序子菜单");
        let sort = sort.split("\n    fn ").next().unwrap_or(sort);
        assert_order(sort, &SORT_MENU_KEYS, "排序 ▸ 组内");

        let ops = src.split("fn ops_items").nth(1).expect("画面子菜单");
        assert_order(ops, &OPS_TAIL_MENU_KEYS, "画面 ▸ 组内");

        assert_current_image_menu(&src);
    }

    /// 负向臂①（m327）：把顶层按 m326 的旧布局摊回 14 项，9 项序钉必须当场红——
    /// 否则这次合并就只是"代码挪了、钉子没跟上"。
    #[test]
    #[should_panic(expected = "右键菜单顶层 里找不到")]
    fn top_level_rejects_the_pre_m327_fourteen_item_layout() {
        let old = [
            "fn context_menu(&self) {",
            "    MenuItem::run(\"打开…\", |_| {}, false);",
            "    MenuItem::run(\"最近\", |_| {}, false);",
            "    MenuItem::run(\"系统菜单\", |_| {}, false);",
            "    MenuItem::submenu(\"复制\", vec![]);",
            "    MenuItem::submenu(\"排序\", vec![]);",
            "    MenuItem::run(\"属性…\", |_| {}, false);",
            "    MenuItem::submenu(\"操作\", vec![]);",
            "    MenuItem::submenu(\"本图\", vec![]);",
            "    MenuItem::submenu(\"插件\", vec![]);",
            "    MenuItem::submenu(\"外观\", vec![]);",
            "    MenuItem::run(\"幻灯片放映\", |_| {}, false);",
            "    MenuItem::submenu(\"主辅视图\", vec![]);",
            "    MenuItem::run(\"左键拖图\", |_| {}, false);",
            "    MenuItem::run(\"设置…\", |_| {}, false);",
            "}",
            "fn file_items(&self) {}",
        ]
        .join("\n");
        let top = old.split("fn file_items").next().unwrap_or(&old);
        assert_order(top, &TOP_MENU_KEYS, "右键菜单顶层");
    }

    /// 负向臂②：模拟 m275 的旧布局——组已经叫「本图」了，但 RAW 参数仍摊在顶层。
    /// 专打"顶层不许再出现收编项"这一条，必须当场红。
    #[test]
    #[should_panic(expected = "RAW 参数… 仍是顶层项")]
    fn current_image_group_rejects_pre_m276_top_level_layout() {
        let old = [
            "fn context_menu(&self) {",
            "    MenuItem::run(\"RAW 参数…\", |_| {}, false);",
            "    MenuItem::submenu(\"本图\", self.current_image_items(true));",
            "    MenuItem::submenu(\"界面\", vec![]);",
            "}",
            "fn file_items(&self) {}",
            "fn current_image_items(&self) {",
            "    \"RAW 参数…\"; \"跳转到…帧/页\"; \"取色器\";",
            "}",
            "fn copy_items(&self) {}",
        ]
        .join("\n");
        assert_current_image_menu(&old);
    }

    /// 负向臂④：组内少一项（只剩 RAW 参数 + 取色器）也要红，别让项序钉子
    /// 变成"只检查前两项"的空壳。
    #[test]
    #[should_panic(expected = "里找不到")]
    fn current_image_group_rejects_losing_the_goto_item() {
        let old = [
            "fn context_menu(&self) {",
            "    MenuItem::submenu(\"本图\", self.current_image_items(true));",
            "    MenuItem::submenu(\"界面\", vec![]);",
            "}",
            "fn file_items(&self) {}",
            "fn current_image_items(&self) {",
            "    \"RAW 参数…\"; \"取色器\";",
            "}",
            "fn copy_items(&self) {}",
        ]
        .join("\n");
        assert_current_image_menu(&old);
    }

    /// m273：卡内项序的两条硬约束——与外观那条钉子同住一处，共用 [`code_lines`]。
    ///
    /// ① 窗口卡：[`WINDOW_ROWS`] 既是序也是相邻判据。「启动窗口」与「窗口尺寸」中间
    ///    一旦再插进任何一行（m269 就插过一次「无边框」），"选档 → 填宽高 → 置灰"
    ///    这条读法就断成两截，断言当场红。
    /// ② 画面卡：[`BROWSE_ROWS`] 勾选在前、两个数值行收在末尾
    ///    （动图/文档播放速度 → 幻灯片间隔），数值行夹在勾选中间会打断连读。
    /// ③ 系统卡：[`SYSTEM_ROWS`] 只留下载源与文件关联入口（m328 起空格两项在窗口卡）。
    const WINDOW_ROWS: [&str; 8] = [
        "\"启动窗口\"",
        "\"窗口尺寸 (宽, 高)\"",
        "\"无边框\"",
        "\"窗口置顶\"",
        "\"最小化到托盘\"",
        "\"允许多开\"",
        "\"空格看图\"",
        "\"长按空格松手收起\"",
    ];
    const BROWSE_ROWS: [&str; 6] = [
        "\"适应长图\"",
        "\"左键拖图\"",
        "\"图片预读\"",
        "\"动图/文档自动播放\"",
        "\"动图/文档播放速度\"",
        "\"幻灯片间隔 (秒)\"",
    ];
    const SYSTEM_ROWS: [&str; 2] = ["\"下载源\"", "\"文件关联\""];

    #[test]
    fn options_cards_keep_their_row_order() {
        let page = code_lines(include_str!("settings/options.rs"));
        let window = page
            .split("fn window_body")
            .nth(1)
            .and_then(|s| s.split("fn browse_body").next())
            .expect("设置页窗口卡");
        assert_order(window, &WINDOW_ROWS, "设置页窗口卡");
        let browse = page
            .split("fn browse_body")
            .nth(1)
            .and_then(|s| s.split("fn system_body").next())
            .expect("设置页画面卡");
        assert_order(browse, &BROWSE_ROWS, "设置页画面卡");
        let system = page.split("fn system_body").nth(1).expect("设置页系统卡");
        assert_order(system, &SYSTEM_ROWS, "设置页系统卡");
    }

    /// m273 负向臂：拿 m272 的旧序喂同一个断言，必须报红——那正是"无边框插在尺寸中间"
    /// 的现场。也钉住常量本身：把 WINDOW_ROWS 改回旧序，这里不再 panic 而失败。
    #[test]
    #[should_panic(expected = "里找不到")]
    fn window_order_rejects_borderless_between_the_size_rows() {
        let old = [
            "\"窗口置顶\"",
            "\"启动窗口\"",
            "\"无边框\"",
            "\"窗口尺寸 (宽, 高)\"",
            "\"最小化到托盘\"",
            "\"允许多开\"",
        ]
        .join("\n");
        assert_order(&old, &WINDOW_ROWS, "m272 旧序模拟");
    }

    /// m265：设置页那一行叫「图片预读」，注释里写明两条来源；写回旧称会红。
    #[test]
    fn options_labels_picture_prefetch_with_two_sources() {
        let src = include_str!("settings/options.rs");
        assert!(src.contains("check_row(\"图片预读\""), "设置页标签没改");
        assert!(
            !src.contains("check_row(\"邻图预读\""),
            "旧称还在 UI 字面量里"
        );
        assert!(
            src.contains("邻图 ±N + 缩略图条可见的那一屏"),
            "注释必须写明下注集合的两条来源"
        );
    }

    /// m266：「图片预读」归哪张卡的**唯一断言**。先查它有没有跑回「系统」卡，
    /// 再查它是不是真在「画面」卡（m273 前叫「看图与操作」）；两条来源的注释必须跟着
    /// 行一起挪。
    fn assert_prefetch_card(src: &str, what: &str) {
        let browse = src
            .split("fn browse_body")
            .nth(1)
            .and_then(|s| s.split("fn system_body").next())
            .unwrap_or_else(|| panic!("{what}：找不到 browse_body 段"));
        let system = src
            .split("fn system_body")
            .nth(1)
            .unwrap_or_else(|| panic!("{what}：找不到 system_body 段"));
        assert!(
            !system.contains("check_row(\"图片预读\""),
            "{what}：「图片预读」又写回系统卡了"
        );
        assert!(
            browse.contains("check_row(\"图片预读\""),
            "{what}：画面卡里没有「图片预读」"
        );
        assert!(
            browse.contains("邻图 ±N + 缩略图条可见的那一屏"),
            "{what}：两条来源的注释没跟着行一起挪"
        );
    }

    #[test]
    fn picture_prefetch_lives_in_the_browse_card() {
        assert_prefetch_card(include_str!("settings/options.rs"), "现状");
    }

    /// m266 负向臂：拿 m265 的旧版式（预读行在 system_body）喂同一个断言，必须报红。
    /// 也钉住断言本身——把 system 那一条删掉，这里不再 panic 而失败。
    #[test]
    #[should_panic(expected = "又写回系统卡了")]
    fn picture_prefetch_rejects_the_system_card() {
        let old = r#"
fn browse_body(h: &Handles) -> Element {
    Element::col().child(check_row("适应长图", h.long_fit))
}
fn system_body(h: &Handles) -> Element {
    Element::col()
        .child(check_row("图片预读", h.prefetch))
        .child(option_row("下载源", Element::dropdown(...)))
}
"#;
        assert_prefetch_card(old, "m265 旧版式模拟");
    }

    /// m262 / m327：外观六项自己是一条轴、不画分隔线；「画面 ▸」把浮条十颗与
    /// 拖图/放映分成两段，中间**只许一条线**。三态档位必须走共用轴，不许就地硬编码。
    #[test]
    fn view_and_ops_groups_keep_one_separator_each() {
        let src = production_src();
        let body = src
            .split("fn appearance_items")
            .nth(1)
            .expect("右键外观六项");
        let end = body.find("\n    fn ").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            !body.contains("MenuItem::separator()"),
            "外观六项内部不许画分隔线（它们本来是一条轴）"
        );
        let ops = src.split("fn ops_items").nth(1).expect("右键画面子菜单");
        let ops = ops.split("fn ops_row").next().unwrap_or(ops);
        assert_eq!(
            ops.matches("MenuItem::separator()").count(),
            1,
            "「画面 ▸」只许一条线：浮条十颗 ── 拖图/放映"
        );
        assert!(body.contains("settings::FILMSTRIP_POS"), "标签要取共用表");
        assert!(
            body.contains("settings::filmstrip_index"),
            "当前档要取共用翻译"
        );
        assert!(
            body.contains("settings::filmstrip_bools"),
            "写回要取共用翻译"
        );
        assert!(
            !body.contains("(0usize, \"关闭\")"),
            "不许在菜单里再硬编码一份三态表"
        );
    }

    /// ★ m256：双击分支必须**先问标题栏**、再问 `double_click` 设置。
    ///
    /// 这条是**接线**钉子：`double_click_action` 自己的用例绿了，不代表分支真在用它
    /// （本仓已经栽过两次：m255 的 `to_canvas`、早先的 `left_drag`）。判据写成源码锚点，
    /// 因为这一段的行为要靠构造一次真双击 + 真窗口才能观测。
    #[test]
    fn double_click_wiring_asks_the_title_strip_first() {
        let src = production_src();
        let ask = src
            .find("let in_title = Chrome::in_title_strip(pos, self.work_tuple());")
            .expect("双击分支必须自己判标题栏（判据单一来源 in_title_strip）");
        let act = src
            .find("double_click_action(in_title, self.config.borrow().dbl_click)")
            .expect("双击结论必须经 double_click_action，不许在分支里手写 match 前的特例");
        assert!(
            ask < act,
            "标题栏判据必须写在取 double_click 之前，否则设置又盖回标题栏"
        );
        assert!(
            !src.contains("let dbl = self.config.borrow().dbl_click;"),
            "裸取 double_click 的旧写法不许回来：那条路会让标题栏跟着设置走"
        );
    }

    /// 显示层调色只在上屏位图 gen / 六个参数 / Gamma 变化时才重算一次，不每帧重扫整图。
    /// m280：角度进缓存键；连调 1°→2° 都从**同一份 src** 重算，
    /// 绝不拿上一次的输出再转一次（`geometry::rotated_is_not_accumulated` 的几何负向臂
    /// 在这里落到显示层）。
    #[test]
    fn angle_changes_recompute_from_source_not_from_previous_frame() {
        use crate::core::pixels::{CHANNEL_NEUTRAL, GAMMA_NEUTRAL};
        let src = RgbaImage::new(4, 4, (0..64).map(|i| (i * 3) as u8).collect());
        let mut cache = TintCache::new();
        assert!(cache
            .obtain(&src, CHANNEL_NEUTRAL, GAMMA_NEUTRAL, 1)
            .is_some());
        assert_eq!(
            cache.key,
            Some((src.gen, CHANNEL_NEUTRAL, GAMMA_NEUTRAL, 1))
        );
        let g1 = cache.img.as_ref().unwrap().gen;

        assert!(cache
            .obtain(&src, CHANNEL_NEUTRAL, GAMMA_NEUTRAL, 2)
            .is_some());
        assert_ne!(cache.img.as_ref().unwrap().gen, g1, "角度变了必须重算");
        assert_eq!(
            cache.img.as_ref().unwrap().rgba.as_slice(),
            crate::core::geometry::rotated(&src.rgba, 4, 4, 2).as_slice(),
            "2° 必须是从源图一次算出来的，不是把 1° 的输出再转 1°"
        );

        // 同角度再来一次：命中缓存，一次都不重算。
        let g2 = cache.img.as_ref().unwrap().gen;
        assert!(cache
            .obtain(&src, CHANNEL_NEUTRAL, GAMMA_NEUTRAL, 2)
            .is_some());
        assert_eq!(cache.img.as_ref().unwrap().gen, g2);
    }

    #[test]
    fn tint_cache_only_recomputes_on_a_real_change() {
        use crate::core::pixels::GAMMA_NEUTRAL as G;
        let src = RgbaImage::new(2, 1, vec![0, 10, 200, 255, 100, 50, 20, 128]);
        let v: [i32; 6] = [-100, 100, 100, 255, 0, 0];
        let mut cache = TintCache::new();
        assert!(cache.obtain(&src, v, G, 0).is_some());
        assert_eq!(cache.key, Some((src.gen, v, G, 0)));
        let first = cache.img.as_ref().unwrap();
        assert_eq!(
            &first.rgba[..],
            &[255, 10, 200, 255, 155, 50, 20, 128],
            "只反相红通道，alpha 与 G/B 原样"
        );
        let gen = first.gen;

        // 同 gen + 同参数 + 同 Gamma：一次都不该重算。
        assert!(cache.obtain(&src, v, G, 0).is_some());
        assert_eq!(cache.img.as_ref().unwrap().gen, gen);

        // 参数变了：重算，且新像素按新参数来。
        let v2: [i32; 6] = [100, 100, 100, 0, 0, 0];
        assert!(cache.obtain(&src, v2, G, 0).is_some());
        assert_eq!(cache.key, Some((src.gen, v2, G, 0)));
        assert_ne!(cache.img.as_ref().unwrap().gen, gen);
        assert_eq!(&cache.img.as_ref().unwrap().rgba[..], &src.rgba[..]);

        // ★ 只动 Gamma 也必须重算，且暗部按曲线抬起来（第七格不能漏进缓存键）。
        let gen_g = cache.img.as_ref().unwrap().gen;
        assert!(cache.obtain(&src, v2, 180, 0).is_some());
        assert_eq!(cache.key, Some((src.gen, v2, 180, 0)));
        assert_ne!(cache.img.as_ref().unwrap().gen, gen_g);
        assert_eq!(&cache.img.as_ref().unwrap().rgba[..4], &[0, 42, 223, 255]);

        // 源位图换代（翻页/缩放）：同样必须重算。
        let next = RgbaImage::new(2, 1, vec![7, 8, 9, 255, 10, 11, 12, 255]);
        let gen2 = cache.img.as_ref().unwrap().gen;
        assert!(cache.obtain(&next, v2, 180, 0).is_some());
        assert_eq!(cache.key, Some((next.gen, v2, 180, 0)));
        assert_ne!(cache.img.as_ref().unwrap().gen, gen2);
    }

    /// ★ 用户 2026-11-06：右键「画面 ▸」的播放三颗**永远存在**；没有帧/页号时
    /// 变灰且不可点，而不是从菜单里消失（浮条才按美观隐藏）。
    #[test]
    fn ops_menu_keeps_anim_buttons_but_disables_them_without_index() {
        let row = ops_row(false);
        assert_eq!(row.len(), Btn::ALL.len(), "菜单项不许少一颗");
        for (b, enabled) in &row {
            assert_eq!(*enabled, !b.is_anim(), "{b:?} 的启用态不对");
        }
        let live = ops_row(true);
        assert!(live.iter().all(|(_, e)| *e), "有帧/页号时全菜单可点");
        // 框架契约：enabled=false 的项既不响应点击，也不会被当动作派发。
        let grey = MenuItem::run("上一帧", |_| {}, false).enabled(false);
        assert!(!grey.is_actionable(), "灰项必须不可点");
        let live_item = MenuItem::run("上一帧", |_| {}, false).enabled(true);
        assert!(live_item.is_actionable(), "亮项必须可点");
    }

    #[test]
    fn menu_columns_aligned_on_every_level() {
        // 纯动作列表（复制 / 插件）本来不含开关项；含开关的级不该被多余改动。
        // 根级这里全用动作项，刻意覆盖补位分支（真实菜单根级自带「左键拖图」开关项）；
        // 「外观 ▸ 不透明度 ▸ 档位」是收编后新出现的三级，递归必须够得着。
        let mut items = vec![
            MenuItem::separator(),
            MenuItem::run("打开…", |_| {}, false),
            MenuItem::submenu("插件", vec![MenuItem::run("色阶分析", |_| {}, false)]),
            MenuItem::submenu(
                "外观",
                vec![
                    MenuItem::run("极简模式", |_| {}, true),
                    MenuItem::separator(),
                    MenuItem::submenu("不透明度", vec![MenuItem::run("50%", |_| {}, true)]),
                ],
            ),
            MenuItem::run("幻灯片放映", |_| {}, false),
        ];
        align_menu_columns(&mut items);
        let has_checks = |v: &[MenuItem]| v.iter().any(|it| it.checkable);
        assert!(has_checks(&items), "根级要留列");
        assert!(has_checks(&items[2].submenu), "纯动作子菜单也要留列");
        assert!(has_checks(&items[3].submenu), "外观子菜单照旧留列");
        assert!(
            has_checks(&items[3].submenu[2].submenu),
            "三级档位列表也要留列"
        );
        assert!(
            items[2].submenu.iter().all(|it| !it.checked),
            "补位不能凭空多出一枚对勾"
        );
        assert!(!items[0].checkable, "分隔线不参与补位");
        assert!(!items[3].submenu[1].checkable, "子菜单分隔线不参与补位");
    }

    #[test]
    fn host_audit_copy_busy_blocks_small_and_large() {
        assert_eq!(copy_decision(1, true), None);
        assert_eq!(copy_decision(chrome::COPY_SYNC_PIXELS + 1, true), None);
        assert_eq!(copy_decision(1, false), Some(chrome::CopyRoute::Sync));
    }

    #[test]
    fn host_audit_real_copy_display_rejects_small_image_while_busy() {
        // 先验证纯闸，回归时在触碰真实剪贴板之前判红。
        assert_eq!(copy_decision(400 * 300, true), None);
        let (canvas, viewer) = zoomed_canvas();
        let tx = canvas.chrome.copy_posted("old large image");
        assert!(matches!(
            copy_display(&canvas.chrome, &viewer),
            CopyNow::Busy
        ));
        assert!(canvas.chrome.copy_in_flight());
        tx.send(Ok(())).unwrap();
        assert_eq!(canvas.chrome.tick_copy(), chrome::CopyWait::Copied);
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    /// m268：滚轮符号 ≠ 清单方向。向上滚（+1）看前面，向下滚（-1）看后面；
    /// 条上 m259 已由真机签收的 `- dir * step` 必须与新函数逐值等价。
    #[test]
    fn wheel_nav_dir_maps_wheel_down_to_the_next_picture() {
        assert_eq!(wheel_nav_dir(1), -1, "向上滚该看前面");
        assert_eq!(wheel_nav_dir(-1), 1, "向下滚该看后面");
        assert_eq!(wheel_nav_dir(0), 0);
        for d in [-2isize, -1, 0, 1, 2] {
            assert_eq!(wheel_nav_dir(d), -d, "不得改坏条上的方向");
        }
    }

    /// m268 源码级同源判据：条与翻页两支必须走同一根方向轴，且节奏闸与
    /// `Viewer::step` 收的都是**翻好的步进方向**（欠账由 runtime 兜出去时也不能反）。
    fn assert_wheel_nav_uses_one_axis(src: &str, what: &str) {
        let start = src
            .find("PointerKind::Wheel(delta) => {")
            .unwrap_or_else(|| panic!("{what}：滚轮分支不见了"));
        let end = src[start..]
            .find("PointerKind::Enter | PointerKind::Move")
            .unwrap_or_else(|| panic!("{what}：滚轮分支没有终点"))
            + start;
        let body = &src[start..end];
        assert!(
            body.contains("wheel_nav_dir(dir) * step"),
            "{what}：缩略图条没走共用的方向轴"
        );
        assert!(
            body.contains("let step_dir = wheel_nav_dir(dir);"),
            "{what}：翻页没翻号（把滚轮符号直接喂给了 step）"
        );
        assert!(
            body.contains("wheel_gate(step_dir)"),
            "{what}：节奏闸存的必须是翻好的步进方向"
        );
        assert!(
            body.contains("step(step_dir)"),
            "{what}：Viewer::step 收的必须是翻好的步进方向"
        );
        assert!(
            !body.contains("step(dir)"),
            "{what}：不许把滚轮符号直接喂给 step"
        );
    }

    #[test]
    fn wheel_nav_uses_one_axis_in_both_branches() {
        assert_wheel_nav_uses_one_axis(production_src(), "现状");
    }

    /// 负向臂 A：把翻号删掉（m267 的形状）必须报红。
    #[test]
    #[should_panic(expected = "翻页没翻号")]
    fn wheel_nav_rejects_an_unflipped_step() {
        let old =
            production_src().replace("let step_dir = wheel_nav_dir(dir);", "let step_dir = dir;");
        assert_wheel_nav_uses_one_axis(&old, "旧写法");
    }

    /// 负向臂 B：只翻 `step`、不翻节奏闸，被闸挂起的那一下仍会反着翻。
    #[test]
    #[should_panic(expected = "节奏闸存的必须是翻好的步进方向")]
    fn wheel_nav_rejects_a_gate_that_keeps_the_raw_wheel_sign() {
        let old = production_src().replace("wheel_gate(step_dir)", "wheel_gate(dir)");
        assert_wheel_nav_uses_one_axis(&old, "闸里存旧符号");
    }

    /// 滚轮碎增量攒够一格才给方向（A3 的前半条判据；节奏闸在 `Chrome::wheel_gate` 那条测试里）。
    #[test]
    fn wheel_detent_needs_a_full_detent() {
        // 高分辨率滚轮一格不到 ±120：三次 40 才攒出一张。
        assert_eq!(wheel_detent(0, 40), (40, 0));
        assert_eq!(wheel_detent(40, 40), (80, 0));
        assert_eq!(wheel_detent(80, 40), (0, 1), "攒够一格只该给一次方向");
        // 一次给两格（240）也只翻一张，剩下的**不追补**——速度由节奏闸封顶。
        assert_eq!(wheel_detent(0, 240), (0, 1));
        // 反向同理；变号时旧累计照参与（不静默丢掉用户已经滚过的那半格）。
        assert_eq!(wheel_detent(0, -120), (0, -1));
        assert_eq!(wheel_detent(70, -200), (0, -1));
        assert_eq!(wheel_detent(-119, 1), (-118, 0), "差 1 就不算一格");
    }

    /// ★ m258：条内拖动换算。往左/往上拖 = 往后翻；整格取整；两端夹紧。
    #[test]
    fn strip_drag_moves_one_grid_per_pitch() {
        assert_eq!(strip_drag_first(10, 0.0, 102.0, 50), 10, "没动就不动");
        assert_eq!(
            strip_drag_first(10, 102.0, 102.0, 50),
            9,
            "往右拖一格 = 看前面一格"
        );
        assert_eq!(
            strip_drag_first(10, -204.0, 102.0, 50),
            12,
            "往左拖两格 = 往后两格"
        );
        assert_eq!(
            strip_drag_first(10, 50.0, 102.0, 50),
            10,
            "半格以内不动——拖一点点就窜一格是错的手感"
        );
        assert_eq!(strip_drag_first(0, 5000.0, 102.0, 50), 0, "到顶不越界");
        assert_eq!(strip_drag_first(50, -5000.0, 102.0, 50), 50, "到底不越界");
        assert_eq!(
            strip_drag_first(10, -3.0, 1.0, 50),
            13,
            "格距保底 1，不会除零"
        );
    }

    /// ★ m259：拖滚动指示的滑块 —— 整条行程映射整份清单（大目录靠它才到得了远处）。
    #[test]
    fn scrubbing_the_bar_maps_the_whole_track_to_the_whole_list() {
        let s = StripScrub {
            grab_at: 100.0,
            thumb0: 40.0,
            span: 300.0,
            max_first: 90,
            horizontal: true,
        };
        // 指针不动 ⇒ 滑块不动（40/300×90 = 12）。
        assert_eq!(scrub_first(&s, (100.0, 0.0)), 12);
        // 拖到行程末端 ⇒ 末屏首格；拖回起点 ⇒ 0。
        assert_eq!(scrub_first(&s, (400.0, 0.0)), 90);
        assert_eq!(scrub_first(&s, (60.0, 0.0)), 0);
        // 行程为 0（滑块占满轨道）⇒ 恒 0，不除零。
        let zero = StripScrub { span: 0.0, ..s };
        assert_eq!(scrub_first(&zero, (500.0, 0.0)), 0);
        // 竖排看纵轴。
        let v = StripScrub {
            horizontal: false,
            ..s
        };
        assert_eq!(scrub_first(&v, (0.0, 400.0)), 90);
    }

    /// ★ m259：点轨道空白 —— 滑块中心落到指针处（与拖动共用同一套行程映射）。
    #[test]
    fn clicking_the_track_jumps_to_that_spot() {
        // 轨道从 100 起、长 300，滑块宽 60 ⇒ 行程 240。
        assert_eq!(scrub_jump((100.0, 0.0), 100.0, 60.0, 240.0, 90, true), 0);
        assert_eq!(scrub_jump((400.0, 0.0), 100.0, 60.0, 240.0, 90, true), 90);
        // 中点 ⇒ 一半（浮点取整容 1 格）。
        let mid = scrub_jump((250.0, 0.0), 100.0, 60.0, 240.0, 90, true);
        assert!((44..=46).contains(&mid), "中点应在 45 附近，实际 {mid}");
        // 竖排看纵轴。
        assert_eq!(scrub_jump((0.0, 400.0), 100.0, 60.0, 240.0, 90, false), 90);
    }

    /// ★ m258 接线钉子：条内左键必须**先**被条接管，不许落到 `press_left`。
    ///
    /// 纯函数绿 ≠ 调用点接了它（本仓踩过两次：`left_drag` 与 `double_click`）。
    /// 老代码只拦"命中格子"，落在格子缝里的一按会走拖图 ⇒ 在条上拖却动了主画面。
    #[test]
    fn strip_drag_is_claimed_before_the_pan_dispatch() {
        let src = include_str!("canvas.rs");
        let down = src
            .find("PointerKind::Down if p.button == MouseButton::Left")
            .expect("左键按下分支");
        let rest = &src[down..];
        let in_strip = rest
            .find("self.in_strip_panel(pos)")
            .expect("条内判据必须出现在左键分支里");
        let drag = rest
            .find("self.strip_drag.set(Some(StripDrag")
            .expect("拖动起点要落进 strip_drag");
        let press = rest.find("self.press_left(pos").expect("拖图分派");
        assert!(
            in_strip < press && drag < press,
            "条内判据与拖动起点都必须排在 press_left 之前，否则拖条会动主画面"
        );
    }

    /// ★ m260 接线钉子：换目录必须清锚点，同一目录内换图**不许**清。
    ///
    /// 不清的后果很具体：从 A 目录（条已滚到第 300 张）双击打开 B 目录的图，新目录也许
    /// 只有十几张 ⇒ 条被夹到尽头、当前图根本不在视野里，看着像"条坏了"。反过来若把清
    /// 零挂到"换图"上，用户辛苦滚到的位置一点就飞回去——所以判据必须是**目录**。
    #[test]
    fn changing_the_directory_resets_the_strip_anchor() {
        let src = include_str!("canvas.rs");
        let i = src.find("let dir_changed =").expect("换目录判据不见了");
        let tail = &src[i..];
        let j = tail.find("if dir_changed {").expect("判据没接到清零动作上");
        assert!(
            tail[..j].contains("gallery_dir"),
            "判据必须问目录（`gallery_dir`），不许拿格数或别的近似值顶替"
        );
        let block = &tail[j..];
        let end = block.find("\n        }\n").expect("清零块没闭合");
        assert!(
            block[..end].contains("self.strip_first.set(None)"),
            "换目录要把滚动锚点归零，否则条停在旧目录的位置上"
        );
    }

    /// ★ m261：手型只在能拖图的地上生效；缩略图条与四缘缩放带都得排除。
    ///
    /// `strip_canvas()` 的 `last` = 800×600、`last_work` = 800×504（底下 96 是条）。
    /// 负向臂：画面正中必须**仍然**判可拖——收得太狠会把拖图功能一起关掉。
    #[test]
    fn pan_area_excludes_the_strip_and_the_resize_band() {
        let c = strip_canvas();
        assert!(c.in_pan_area((400.0, 250.0)), "画面正中要能拖图");
        assert!(c.in_pan_area((400.0, 500.0)), "紧贴条上沿仍属画面");
        assert!(!c.in_pan_area((400.0, 550.0)), "缩略图条不接手");
        assert!(!c.in_pan_area((4.0, 250.0)), "左缘 8dp 是缩放带");
        assert!(!c.in_pan_area((796.0, 250.0)), "右缘 8dp 是缩放带");
        assert!(!c.in_pan_area((400.0, 4.0)), "上缘 8dp 是缩放带");
        assert!(!c.in_pan_area((400.0, 504.0)), "条与画面的分界归条");
        assert!(c.in_resize_band((400.0, 596.0)), "窗口底边那圈要认出来");
        assert!(!c.in_resize_band((400.0, 550.0)), "条中间不是缩放带");
    }

    /// ★ m261 接线钉子：纯几何绿 ≠ `sync_hand` 真的用了它。
    ///
    /// 负向臂两条：不许再把 `strip_drag` / `strip_scrub` 塞回"拖动中=抓手"那一段，
    /// 也不许把 `in_strip_panel && strip_scrollable` 那条"条上给开手"的分枝写回来。
    #[test]
    fn hand_cursor_is_gated_by_the_pan_area() {
        let src = include_str!("canvas.rs");
        let start = src.find("fn sync_hand(&self").expect("光标分派不见了");
        let tail = &src[start..];
        let end = tail.find("fn on_pointer").expect("分派函数没闭合");
        let body = &tail[..end];
        let gate = body
            .find("if !self.in_pan_area(pos)")
            .expect("必须先拿可拖区闸住手型");
        let hand = body
            .find("LeftDrag::PanImage => Some(false)")
            .expect("开手那一支不见了");
        assert!(gate < hand, "闸必须排在给手型之前");
        assert!(
            body.contains("if self.in_resize_band(pos)"),
            "边框带要单独撤覆盖，不能顺手设成箭头"
        );
        assert!(
            !body.contains("self.strip_drag.get().is_some()")
                && !body.contains("self.strip_scrub.get().is_some()")
                && !body.contains("self.in_strip_panel(pos) && self.strip_scrollable()"),
            "拖条与条上悬停都不许再给手型"
        );
    }

    /// ★ m256：标题栏双击恒等「最大化 / 恢复」，**不看** `double_click` 设置。
    ///
    /// 用户 2026-11-08 实机「秒退」：默认 `double_click=CloseWindow`，双击顶条把窗收了。
    /// 这条钉的是**判据本身**，接线由 `double_click_wiring_asks_the_title_strip_first`
    /// 按源码锚点兜住（纯函数绿 ≠ 调用点接了它，这是本仓踩过两次的老坑）。
    #[test]
    fn title_bar_double_click_always_toggles_maximize() {
        use crate::core::config::DblClick;
        for cfg in [
            DblClick::Nothing,
            DblClick::CloseWindow,
            DblClick::Maximize,
            DblClick::Fullscreen,
        ] {
            assert_eq!(
                double_click_action(true, cfg),
                DblClick::Maximize,
                "标题栏双击必须恒为最大化/恢复，不能跟着 double_click 走"
            );
            assert_eq!(
                double_click_action(false, cfg),
                cfg,
                "画面区照旧按 double_click 设置"
            );
        }
    }

    /// 拖拽升级成"拖窗口"的阈值**不许**把手抖算进去——判错一次，双击关窗就没了。
    #[test]
    fn double_click_jitter_is_not_a_drag() {
        let origin = (100.0, 100.0);
        // 手抖 1~3dp：双击照样发生。
        for jitter in [0.0, 1.0, 2.0, 3.0] {
            assert!(
                !beyond_slop(origin, (origin.0 + jitter, origin.1)),
                "{jitter}dp 的抖动被当成了拖窗口"
            );
        }
        // 真的挪了：升级成拖窗口。
        assert!(beyond_slop(origin, (origin.0 + 5.0, origin.1)));
        assert!(beyond_slop(origin, (origin.0 - 20.0, origin.1 + 3.0)));
    }

    /// m238 修复轮：真全屏里窗口不许被拖走（用户实机「全屏还能拖标题栏」）。
    #[test]
    fn left_drag_never_moves_the_window_in_fullscreen() {
        use LeftDrag::{None as NoDrag, PanImage, Window};
        assert_eq!(
            left_drag(true, true, false, true, false, false),
            PanImage,
            "画布里仍可平移图"
        );
        assert_eq!(
            left_drag(true, true, true, true, false, false),
            NoDrag,
            "顶条那一条不再拖窗口"
        );
        assert_eq!(
            left_drag(false, true, false, true, false, false),
            NoDrag,
            "开关关掉也不拖窗口"
        );
        assert_eq!(
            left_drag(true, false, false, true, false, false),
            NoDrag,
            "空窗也不拖窗口"
        );
        assert_eq!(left_drag(false, true, true, true, false, false), NoDrag);
        // 非全屏回归：顶条永远带走窗口，别把旧行为改坏。
        assert_eq!(left_drag(true, true, true, false, false, false), Window);
        assert_eq!(left_drag(false, false, false, false, false, false), Window);
    }

    /// 左键这一拖归谁：`drag_image` 开关 × 有没有可平移余量 × 是不是标题栏，**八格全钉**。
    ///
    /// 只有一格（开关开 + 拖得动 + 画布内）是本次新增的行为，其余七格一律回到拖窗口——
    /// 用户要的是"能只拖图片"，不是"把拖窗口这条路堵死"：标题栏那一条永远带走窗口，
    /// 拖不动东西（只有"还没有图"这一种）时也留着拖窗口。
    /// ★ "拖得动"到 2026-11-03 第四条口径时已经放宽到**几乎总是真**，判据本体见
    /// `Viewer::pan_limits` → `overscroll`（并集的一半减一枚衣角；纵向自 m290 起还要
    /// 避开顶上那条自绘标题栏，见 `Viewer::chrome_top`）。
    #[test]
    fn left_drag_yields_the_window_unless_there_is_room_in_the_canvas() {
        use LeftDrag::{PanImage, Window};
        assert_eq!(
            left_drag(true, true, false, false, false, false),
            PanImage,
            "本次功能的全部新行为"
        );
        assert_eq!(
            left_drag(true, true, true, false, false, false),
            Window,
            "标题栏永远拖窗口"
        );
        assert_eq!(
            left_drag(true, false, false, false, false, false),
            Window,
            "空窗（没有图）时别堵死拖窗口"
        );
        assert_eq!(left_drag(true, false, true, false, false, false), Window);
        assert_eq!(
            left_drag(false, true, false, false, false, false),
            Window,
            "开关关掉就是 2026-10-08 那套分工"
        );
        assert_eq!(left_drag(false, true, true, false, false, false), Window);
        assert_eq!(left_drag(false, false, false, false, false, false), Window);
        assert_eq!(left_drag(false, false, true, false, false, false), Window);
    }

    /// m272：无边框形态**没有标题栏**，整块画面就是搬窗口的地。
    ///
    /// 用户 2026-11-08 实机原话：「无边框下拖窗口只有顶部那条 40dp 的隐形热区，
    /// 没有任何提示」——那条热区是 `in_title_strip` 的几何残留，画面上根本看不见，
    /// 等于那个形态搬不动窗口。新判据只留一条分界：图**溢出视口**（100% / 放大过）
    /// 时归图片，否则归窗口；顶部那条在无边框下不再有特殊含义。
    ///
    /// 真全屏（m238）仍然先判：全屏 + 无边框也不许把窗口拖离显示器。
    #[test]
    fn borderless_drags_the_whole_canvas_as_a_title_bar() {
        use LeftDrag::{None as NoDrag, PanImage, Window};
        // 常态（图适应视口）：画面任意位置都搬窗口，包括顶部那条。
        assert_eq!(
            left_drag(true, true, false, false, true, false),
            Window,
            "无边框开图：整块画面搬窗口"
        );
        assert_eq!(
            left_drag(true, true, true, false, true, false),
            Window,
            "顶部那条也搬窗口——这个形态没有标题栏可谈"
        );
        // 图真的溢出：回落到平移图片（与普通窗口"放大后画面拖图"同构）。
        assert_eq!(
            left_drag(true, true, false, false, true, true),
            PanImage,
            "100% / 放大过之后才拖图"
        );
        // 开关关掉：无边框下更该一律搬窗口，别留"拖不动"的形态。
        assert_eq!(
            left_drag(false, true, false, false, true, true),
            Window,
            "关掉 drag_image 就是纯搬窗口"
        );
        // 空窗（没有图 ⇒ overflows 恒假）：搬窗口。
        assert_eq!(left_drag(true, false, false, false, true, false), Window);
        // 全屏优先：无边框 + 全屏照样不许拖走窗口，但画布仍可平移。
        assert_eq!(
            left_drag(true, true, false, true, true, false),
            PanImage,
            "全屏 + 无边框：画布仍可平移图片"
        );
        assert_eq!(
            left_drag(true, false, false, true, true, false),
            NoDrag,
            "全屏 + 无边框 + 拖不动：不响应，不留能拖的假象"
        );
    }

    /// 接线自检：`ImageCanvas::press_left` 真的按 [`left_drag`] 的结论开局。
    ///
    /// 纯函数绿了不代表线接对——写错一个分支方向，用例照过、手感照错，而这正是
    /// "按下拖的是图片还是窗口"这种**只有用户能发现**的缺陷。四臂各钉一条：
    /// 默认档平移 / 关掉记账 / 标题栏记账 / Ctrl 不看开关。
    #[test]
    fn left_press_wires_the_switch_to_the_pan_state() {
        let (canvas, shared) = zoomed_canvas(); // 图 400×300 已放大到超出 200×150 视口
        let view = (0.0, 0.0, 200.0, 150.0);
        assert!(
            canvas.config.borrow().drag_image,
            "本例的前提是默认档（开）"
        );

        // ① 开 + 有余量 + 画布内 ⇒ 开平移局，且不记拖窗口的账。
        canvas.press_left((50.0, 80.0), view, false);
        assert!(shared.borrow().dragging(), "画布内按下该开平移局");
        assert!(
            canvas.lmb_pending.get().is_none(),
            "拖窗口那笔账不该记，否则抬起前就被升级成拖窗口"
        );

        // ② 同一枚画布把开关关掉 ⇒ 反过来：记账、不开局。
        shared.borrow_mut().end_drag();
        canvas.config.borrow_mut().drag_image = false;
        canvas.press_left((50.0, 80.0), view, false);
        assert!(!shared.borrow().dragging(), "开关关掉就不该开平移局");
        assert_eq!(
            canvas.lmb_pending.get(),
            Some((50.0, 80.0)),
            "该记下拖窗口的起点"
        );

        // ③ 开关开着，但落在标题栏那一条里 ⇒ 那一按永远带走窗口。
        canvas.config.borrow_mut().drag_image = true;
        canvas.lmb_pending.set(None);
        canvas.press_left((50.0, 10.0), view, false);
        assert!(!shared.borrow().dragging(), "标题栏那一按不许变成拖图片");
        assert!(canvas.lmb_pending.get().is_some());

        // ④ Ctrl+左键是兜底平移手势，**不看开关**（2026-10-08 起）。
        canvas.config.borrow_mut().drag_image = false;
        canvas.lmb_pending.set(None);
        canvas.press_left((50.0, 80.0), view, true);
        assert!(shared.borrow().dragging(), "Ctrl+左键一直平移");

        // ⑤ 第三条口径的新行为：**图比窗口小也拖图**（旧口径在这里会退回拖窗口）。
        // 400×300 的图缩到 100×75，落在 200×150 的视口里 ⇒ 四周都是空白，仍该拖得动。
        shared.borrow_mut().end_drag();
        canvas.config.borrow_mut().drag_image = true;
        canvas.lmb_pending.set(None);
        shared.borrow_mut().set_fit(false);
        shared.borrow_mut().zoom_at_center(0.25);
        assert!(
            shared.borrow().can_pan(),
            "夹具的前提：图确实比视口小（100×75 落在 200×150 里）"
        );
        canvas.press_left((50.0, 80.0), view, false);
        assert!(
            shared.borrow().dragging(),
            "小图也该开平移局——用户 2026-11-03 第三条口径"
        );
        assert!(canvas.lmb_pending.get().is_none(), "拖窗口那笔账不该记");
    }

    /// 「复制 ▸」的三项：名字、顺序、灰项判据、以及键位提示还在不在。
    ///
    /// 菜单文本是用户唯一看得见的东西，写错字或漏掉一项**没有任何别的门禁会响**；
    /// 而从平铺改成子菜单，最容易丢的正是挂在第一项与第三项上的快捷键提示。
    #[test]
    fn copy_submenu_carries_the_three_copy_actions() {
        let (canvas, _v) = zoomed_canvas();
        // 默认档这两个键位是**空的**（`hotkeys=…copy_img:;copy_path:`），所以先自己绑上，
        // 否则下面的 `is_some()` 断言测的是配置而不是接线。
        canvas.config.borrow_mut().hotkeys =
            crate::core::hotkey::Hotkeys::from_ini("copy_img:C;copy_path:V");
        let items = canvas.copy_items(true, true);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["复制当前（PNG）", "复制原文件", "复制文件路径"],
            "「复制 ▸」的内容或顺序变了"
        );
        let cfg = canvas.config.borrow();
        assert!(
            items[0].shortcut.is_some() && items[2].shortcut.is_some(),
            "本例的前提是两项都绑到了键"
        );
        assert_eq!(
            items[0].shortcut,
            cfg.hotkeys.display(Action::CopyImage),
            "第一项的键位提示必须来自 CopyImage"
        );
        assert_eq!(
            items[2].shortcut,
            cfg.hotkeys.display(Action::CopyPath),
            "第三项的键位提示必须来自 CopyPath（收进子菜单最容易接错的一格）"
        );
        assert_eq!(
            items[1].shortcut, None,
            "「复制原文件」从来没有键位，别在这里冒出第二个来源"
        );
        drop(cfg);
        assert!(
            items[0].enabled && items[1].enabled && items[2].enabled,
            "有图有文件时三项都该可点"
        );
        // 灰项各归各：没图 ⇒ 只有「复制当前」关掉；没文件 ⇒ 后两项关掉。
        let no_image = canvas.copy_items(false, true);
        assert!(!no_image[0].enabled, "没图不该能复制画面");
        assert!(no_image[1].enabled && no_image[2].enabled);
        let no_file = canvas.copy_items(true, false);
        assert!(no_file[0].enabled);
        assert!(
            !no_file[1].enabled && !no_file[2].enabled,
            "没打开文件时那两项都该灰着"
        );
    }

    /// m327 接线钉子：源码级测试只认标签在不在对应函数里；这条走真实构造函数，
    /// 钉住“复制三项确实**平铺**在文件组里”，且属性仍在块尾。
    #[test]
    fn file_group_flattens_copy_and_keeps_properties_last() {
        let (canvas, _v) = zoomed_canvas();
        let items = canvas.file_items(true, true);
        let labels: Vec<&str> = items
            .iter()
            .filter(|it| !it.separator)
            .map(|it| it.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec![
                "打开…",
                "系统菜单",
                "复制当前（PNG）",
                "复制原文件",
                "复制文件路径",
                "属性…",
                "清除本书阅读位置",
            ],
            "文件 ▸ 的项序或拍平结果变了"
        );
        assert_eq!(
            items.iter().filter(|it| it.separator).count(),
            2,
            "文件 ▸ 两条线：打开层 / 复制"
        );
        assert!(items[0].enabled, "“打开…”永远可点");
        assert!(
            items
                .last()
                .is_some_and(|it| it.label == "清除本书阅读位置"),
            "清除阅读位置是 m333 新块尾；属性仍紧挨在它前面"
        );
        assert!(
            !labels.contains(&"文件名"),
            "排序已独立成顶层一项，不许再摊回文件组"
        );
    }

    /// m330：设置窗里出现过的开关，任何入口都必须写同一份 ini。
    /// 极简模式与窗口置顶曾各有“只改 chrome / 只改窗口态”的旁路；这条钉子按
    /// 写回形状计数，菜单 / 快捷键 / 标题栏少一条就红。
    #[test]
    fn settings_backed_toggles_write_config_from_every_entry() {
        let src = production_src();
        assert!(
            src.matches("|c| c.minimal_mode = on").count() >= 2,
            "极简模式至少要有菜单 + 快捷键两条写回"
        );
        assert!(
            src.matches("|c| c.stay_on_top = on").count() >= 2,
            "窗口置顶至少要有标题栏 + 快捷键两条写回"
        );
    }

    /// m327：排序独立成组 = 四个维度 → 一条线 → 倒序；五项都带勾选列。
    #[test]
    fn sort_menu_lists_four_dimensions_then_desc() {
        let (canvas, _v) = zoomed_canvas();
        let item = canvas.sort_menu();
        assert_eq!(item.label, "排序");
        let sub = &item.submenu;
        assert_eq!(sub.len(), 6, "4 维 + 分隔线 + 倒序");
        let labels: Vec<&str> = sub
            .iter()
            .filter(|it| !it.separator)
            .map(|it| it.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["文件名", "大小", "创建日期", "修改日期", "倒序"],
            "排序 ▸ 的项序变了"
        );
        assert!(
            sub.iter().filter(|it| !it.separator).all(|it| it.checkable),
            "五项都要留勾选列"
        );
    }

    /// 鸟瞰盒必须在**右下角**，且始终整个落在视口内。
    ///
    /// 这是"抽屉不会跑出窗口"的自检：右下角 + 抬起来躲工具栏两段位移叠加后，
    /// 很容易把盒子顶到窗口外面去（曾经右上角版本就不用做这个算术）。
    #[test]
    fn nav_box_sits_inside_the_viewport() {
        for (w, h) in [(360, 260), (520, 300), (1040, 700), (1920, 1080)] {
            let content = Rect::new(0, 0, w, h);
            let b = ImageCanvas::nav_box_in(content, content);
            assert!(
                b.x >= content.x && b.y >= content.y,
                "{w}x{h}: 盒子左上角出界"
            );
            assert!(
                b.x + b.w <= content.x + content.w && b.y + b.h <= content.y + content.h,
                "{w}x{h}: 盒子 {b:?} 跑出了视口"
            );
        }
    }

    /// 极端长宽比（条图）不许把鸟瞰盒压成 0 宽 / 0 高：命中区一旦为 0，
    /// `nav_hit` 的 `pos < box.x + 0` 恒假，鸟瞰就变成"看得见却永远点不到"
    /// （绘制侧另有 `max(1.0)`，两边对不上）。比例仍须等于图像比例。
    #[test]
    fn nav_geom_never_collapses_to_a_zero_hit_box() {
        let canvas = strip_canvas();
        let content = Rect::new(0, 0, 800, 600);
        for (dw, dh) in [(100, 16384), (16384, 100), (1, 4000), (4000, 1)] {
            let dst = Rect::new(0, 0, dw, dh);
            let (nav, thumb) = canvas
                .nav_geom_in(content, content, dst)
                .unwrap_or_else(|| panic!("{dw}x{dh} 显示尺寸远超视口，该出鸟瞰"));
            assert!(
                nav.w >= 1 && nav.h >= 1,
                "{dw}x{dh}: 命中盒 {nav:?} 塌成 0，鸟瞰点不到"
            );
            assert_eq!(nav, thumb, "盒与缩略图必须是同一份几何");
        }
    }

    /// ★ m103 的主判据：**源是整图时，画的面必须与盒逐像素相等**。
    ///
    /// 用户报"鸟瞰边缘有空白/暗边，原图那里明明有内容"。实拍定位到的形状是
    /// 1234×2977（盒 63×151）：左缘、上缘贴到图缘，右缘与下缘各空 1 格——那两格
    /// 露的是盒底与内白描边。根因在绘制层把盒宽再除一次 `dst.w` 又乘回 `idst.w`：
    /// `idst == dst` 时数学上等于原值，f32 往返差半个 ULP，`as i32` 向零截断就丢 1 px。
    /// 这条用例把那个形状连同一大片真实组合一起钉住：任何一组 (视口, 显示尺寸) 生成的
    /// 盒，源为整图时画出来的矩形必须**正好等于**盒。
    #[test]
    fn nav_whole_picture_fills_the_box_exactly() {
        let canvas = strip_canvas();
        let content = Rect::new(0, 0, 900, 660);
        // 报障那几格（600×4000 是量出来差 5 px 的那一格，1234×2977 是实拍差 1 px 的那一格）。
        for (dw, dh) in [
            (1234, 2977),
            (2600, 1800),
            (4000, 600),
            (600, 4000),
            (2000, 2000),
            (6055, 6055),
            (100, 16384),
            (16384, 100),
        ] {
            let dst = Rect::new(0, 0, dw, dh);
            let (_, box_r) = canvas
                .nav_geom_in(content, content, dst)
                .unwrap_or_else(|| panic!("{dw}x{dh} 远超视口，该出鸟瞰"));
            assert_eq!(
                nav_draw_rect(box_r, dst, dst),
                box_r,
                "{dw}x{dh}：盒 {box_r:?} 没被整图铺满，边缘会露出盒底/描边"
            );
        }
        // 再把真实组合铺开扫一遍：显示尺寸从刚超出视口到 8K，横竖两轴各挑一档。
        // 两根轴都还没超出视口的组合不出鸟瞰（`nav_geom` 的判据），跳过。
        let mut swept = 0;
        for dw in (901..8000).step_by(37) {
            for dh in (661..8000).step_by(53) {
                let dst = Rect::new(0, 0, dw, dh);
                let Some((_, box_r)) = canvas.nav_geom_in(content, content, dst) else {
                    continue;
                };
                swept += 1;
                assert_eq!(
                    nav_draw_rect(box_r, dst, dst),
                    box_r,
                    "{dw}x{dh}：盒 {box_r:?} 没铺满"
                );
            }
        }
        assert!(
            swept > 10_000,
            "铺开扫的组合数不对，这条用例没在做事：{swept}"
        );
    }

    /// 切片态（首帧之前退回主位图）按比例落位，并且**不许画到盒外**：左端越界时
    /// 夹回盒的左缘、宽度跟着缩短，而不是留一个越出盒的矩形。
    #[test]
    fn nav_slice_lands_inside_the_box_and_never_spills() {
        let inner = Rect::new(0, 0, 60, 60);
        let dst = Rect::new(0, 0, 120, 120);
        // 整图右下那 1/4 的切片：按比例 = 盒的右下 1/4。
        assert_eq!(
            nav_draw_rect(inner, Rect::new(60, 60, 60, 60), dst),
            Rect::new(30, 30, 30, 30)
        );
        // 切片左端在整图之外（SVG 那份带 30% 边距的可视切片就会这样）：夹回盒内。
        let r = nav_draw_rect(inner, Rect::new(-40, 0, 80, 40), dst);
        assert_eq!(r, Rect::new(0, 0, 20, 20), "越界部分该夹掉，剩下的按比例留");
        assert!(r.x >= inner.x && r.y >= inner.y);
        assert!(r.x + r.w <= inner.x + inner.w && r.y + r.h <= inner.y + inner.h);
        // 极端条图：盒只有 1 px 宽也不许算出 0 或负的矩形（画不出图 = 面板上一片黑）。
        let thin = Rect::new(500, 20, 1, 152);
        let r = nav_draw_rect(
            thin,
            Rect::new(0, 0, 100, 16384),
            Rect::new(0, 0, 100, 16384),
        );
        assert_eq!(r, thin, "源是整图时 1 px 的盒也要铺满");
    }

    /// 「适应窗口」的键盘入口必须与按钮/菜单同义。夹具**必须**先退出适应档：
    /// `Viewer` 初始就是 `fit=true`，两次按键都停在同一个态上，测不出入口分叉。
    #[test]
    fn fit_key_means_fit_not_a_toggle() {
        use crate::core::decode::Decoder;
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let v = Rc::new(RefCell::new(Viewer::new(
            Rc::new(Decoder::spawn_with(tx)),
            Rc::clone(&cfg),
        )));
        let chrome = Chrome::new(false);
        let hub: Hub = Rc::new(RefCell::new(None));
        v.borrow_mut().set_fit(false);

        let zero = KeyEvent {
            key: Key::Char('0'),
            pressed: true,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        };
        assert!(handle_key(None, None, &v, &chrome, &cfg, &hub, &zero));
        assert!(v.borrow().is_fit(), "第一次按 0 应进入适应窗口");
        assert!(handle_key(None, None, &v, &chrome, &cfg, &hub, &zero));
        assert!(
            v.borrow().is_fit(),
            "已经在适应窗口时再按 0 仍是适应窗口——按钮/菜单就是置真，键不许是 toggle"
        );
    }

    /// m81 接线：按住翻页键的洪流里只该翻一记（欠的那一下由轮询兜出去），
    /// 一记一记的手按每一记都翻。
    ///
    /// 为什么要这条而不是只测 `Chrome::key_gate`：闸本身测得再细，**接线漏了测试照样全绿**
    /// ——挂在 `handle_key` 里还是挂在 `exec_action` 里、菜单那条路该不该过闸，只有走一遍
    /// 真实的按键事件才看得出来。观察量用 `gallery_pos`，它是"到底翻没翻"的唯一硬证据。
    #[test]
    fn held_flip_key_is_paced_and_a_deliberate_tap_is_not() {
        use crate::core::decode::Decoder;
        use std::time::{Duration, Instant};
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix-m81-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut paths = Vec::new();
        for n in ["a", "b", "c"] {
            let p = dir.join(format!("{n}.svg"));
            std::fs::File::create(&p).unwrap();
            paths.push(p);
        }
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let v = Rc::new(RefCell::new(Viewer::new(
            Rc::new(Decoder::spawn_with(tx)),
            Rc::clone(&cfg),
        )));
        v.borrow_mut().open(&paths[0]);
        let chrome = Chrome::new(false);
        let hub: Hub = Rc::new(RefCell::new(None));
        let right = || KeyEvent {
            key: Key::Right,
            pressed: true,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        };

        // ① 第一记：当场翻（手按不该有任何延迟）。`gallery_pos` 是 1-based（"2/3"）。
        assert!(handle_key(None, None, &v, &chrome, &cfg, &hub, &right()));
        assert_eq!(v.borrow().gallery_pos(), 2, "第一记右键该翻到第二张");

        // ② 背靠背的第二记 = 按住自重复：消费掉（返回 true），但**不翻**，且欠着。
        assert!(handle_key(None, None, &v, &chrome, &cfg, &hub, &right()));
        assert_eq!(
            v.borrow().gallery_pos(),
            2,
            "一个周期内的自重复不该再翻一记"
        );
        assert_eq!(
            chrome.key_pending.get(),
            1,
            "压住的那一下必须还欠着，不能丢"
        );

        // ③ 到点后由 16 ms 轮询兜出去（这里做 runtime 那条 tick 的等价动作）。
        chrome
            .key_due
            .set(Instant::now() - Duration::from_millis(1));
        let owed = chrome.tick_key_pace();
        assert_eq!(owed, Some(1), "欠的那一下该被轮询兜出来");
        v.borrow_mut().step(owed.unwrap());
        assert_eq!(v.borrow().gallery_pos(), 3, "兜出去的那一下真要翻页");

        // ④ 手按（上一记在 400 ms 前）**即使闸还没到点**也必须当场翻。
        //    这条断言把"按间隔限速"与"无脑限速"分开：只测前者会漏掉后者（破坏实验里
        //    把判据强写成 `held = true`，上一版这条是绿的——因为它跟在一次兜底之后，
        //    key_due 已在过去，任何写法都放行）。
        let now = Instant::now();
        chrome.key_due.set(now + chrome::KEY_PACE);
        chrome.key_last.set(now - Duration::from_millis(400));
        assert!(handle_key(None, None, &v, &chrome, &cfg, &hub, &right()));
        assert_eq!(
            v.borrow().gallery_pos(),
            1,
            "循环翻回第一张——手按没被闸吃掉"
        );
        assert_eq!(chrome.key_pending.get(), 0, "故意慢按不该留下欠账");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 锁定时键盘全封：任何键都返回 true（=已消费），且不许有业务副作用。
    /// 消费是关键——`App::on_shortcut` 排在框架 Tab/Escape 兜底之前，
    /// 返回 true 才能吞掉 Esc 关窗键。
    #[test]
    fn locked_handle_key_consumes_everything() {
        use crate::core::decode::Decoder;
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let v = Rc::new(RefCell::new(Viewer::new(
            Rc::new(Decoder::spawn_with(tx)),
            Rc::clone(&cfg),
        )));
        let chrome = Chrome::new(false);
        let hub: Hub = Rc::new(RefCell::new(None));

        // 未锁对照组：T 正常切换（证明测试不是空转）。
        let t_ev = KeyEvent {
            key: Key::Char('t'),
            pressed: true,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        };
        assert!(handle_key(None, None, &v, &chrome, &cfg, &hub, &t_ev));
        assert!(chrome.filmstrip.get());

        chrome.locked.set(true);
        for key in [
            Key::Left,
            Key::Char('f'),
            Key::Char('t'),
            Key::Escape,
            Key::Char('+'),
        ] {
            let ev = KeyEvent {
                key,
                pressed: true,
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            };
            assert!(
                handle_key(None, None, &v, &chrome, &cfg, &hub, &ev),
                "锁定时 {key:?} 应被消费"
            );
        }
        // 副作用检查：锁定期间 T 没把条切回去。
        assert!(chrome.filmstrip.get(), "锁定时 T 不该切换缩略图条");
    }

    /// 造一个"图已加载且放大到超出视口"的画布：**不碰真实解码**（直接注入一张合成位图），
    /// 因为这条用例要跑在 `cargo test` 里，解码线程在这里既慢又脆。
    fn zoomed_canvas() -> (ImageCanvas, Rc<RefCell<Viewer>>) {
        use crate::core::decode::{Decoder, Done};
        use crate::core::pixels::{Anim, Bitmap, Decoded};

        let (tx, _rx) = std::sync::mpsc::channel::<Done>();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let mut v = Viewer::new(Rc::new(Decoder::spawn_with(tx)), cfg.clone());
        let (w, h) = (400u32, 300u32);
        let done = Done {
            seq: 0,
            path: std::path::PathBuf::from("test.png"),
            out: Ok(Decoded {
                natural: (w, h),
                region: None,
                anim: Anim::STATIC,
                bitmap: Bitmap {
                    width: w,
                    height: h,
                    rgba: vec![0x80u8; (w * h * 4) as usize],
                },
            }),
        };
        assert!(v.apply(done), "合成位图没能装上");
        v.on_viewport(0.0, 0.0, 200.0, 150.0, 1.0);
        v.set_fit(false); // 固化当前倍率后放大到超出视口，鸟瞰才会出现。
        v.zoom_at_center(2.0);
        let shared = Rc::new(RefCell::new(v));
        let hub: Hub = Rc::new(RefCell::new(None));
        let canvas = ImageCanvas::new(shared.clone(), Chrome::new(false), cfg, hub);
        // `last` 是完整内容矩形（悬浮层用），`last_work` 是扣掉缩略图条之后留给
        // 图片的那块（摆图/鸟瞰/缩放锚点用）。没开条时两者相等。
        canvas.last.set(Rect::new(0, 0, 200, 150));
        canvas.last_work.set(Rect::new(0, 0, 200, 150));
        (canvas, shared)
    }

    /// 拖鸟瞰时的**重绘请求次数**（M6）：pan 被钳在原地就不许再发起重绘。
    ///
    /// 判据不是"看着顺不顺"而是次数：用户在真机量到"拖动鸟瞰 CPU 稳定 5–6%"，其中
    /// 往边界外继续拖那一段画面一个像素都没变，原来每个 move 都整窗重绘一遍。
    /// 这里走生产的事件入口 `on_pointer`，用 `windui::testing::run_with_ctx` 借一个真
    /// `EventCtx`，读 `DispatchResult.repaint`——标脏链路整个参与，不是替身函数。
    ///
    /// ★ `pan_interval` 钉成 0：本用例测的是 M6 那条"pan 没变就不画"，M8 的限速装在
    /// 它后面，不关掉就会把"每步一帧"挡成"每周期一帧"，两条律就分不开了。
    #[test]
    fn nav_drag_only_repaints_when_pan_actually_moves() {
        use std::time::Duration;
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let (mut canvas, viewer) = zoomed_canvas();
        canvas.chrome.pan_interval.set(Duration::ZERO);
        canvas.nav_drag.set(true);
        let content = canvas.last.get();
        let dst = to_rect(viewer.borrow().dst_rect(from_rect(content)).unwrap());
        let (_box, thumb) = canvas.nav_geom_in(content, content, dst).unwrap();
        let at = |u: f32, v: f32| {
            PointerEvent::single(
                PointerKind::Move,
                Point::new(
                    (thumb.x as f32 + u * thumb.w as f32) as i32,
                    (thumb.y as f32 + v * thumb.h as f32) as i32,
                ),
                MouseButton::Left,
            )
        };
        let repaint = |c: &mut ImageCanvas, ev: &PointerEvent| -> bool {
            windui::testing::run_with_ctx(|ctx| {
                c.on_pointer(ctx, ev);
            })
            .repaint
        };
        // 预热一拍：悬停态在这一步才稳定，不算进拖动链路的账。
        let _ = repaint(&mut canvas, &at(0.5, 0.5));

        // 盒内连续拖动：每一步 pan 都真的在动，八步就得八次重绘（少一次就是拖动坏了）。
        let mut moved = 0;
        for k in 1..9 {
            if repaint(&mut canvas, &at(0.2 + k as f32 * 0.05, 0.5)) {
                moved += 1;
            }
        }
        assert_eq!(moved, 8, "盒内拖动每步都该重绘");

        // 拖到边界外：`nav_pan_to` 把 u 钳在 1.0，先钳到位（这一步画面确实动了），
        // 之后鼠标继续往外走，位置一个不变。
        assert!(
            repaint(&mut canvas, &at(1.6, 0.5)),
            "第一次越界仍是一次真实移动"
        );
        let mut wasted = 0;
        for k in 1..7 {
            if repaint(&mut canvas, &at(1.6 + k as f32 * 0.2, 0.5)) {
                wasted += 1;
            }
        }
        assert_eq!(wasted, 0, "钳位后的空转拖动不许再发起重绘（M6）");
    }

    /// ★ 放映让位必须挂在**输入事件本身**上，而不是只挂在翻页节奏闸里。
    /// 复现的是用户 2026-10-31 实测的那一半：「方向键有用，滚轮没用」——默认档
    /// `wheel = Zoom` 那条分支根本不进 `wheel_gate`，把 stop 放在闸里就对缩放用户完全失效。
    /// 夹具走生产入口 `on_pointer`（配置分流与锁定判断一起过），不用替身函数。
    #[test]
    fn wheel_and_press_gestures_stop_slideshow() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let (mut canvas, _viewer) = zoomed_canvas();
        let send = |c: &mut ImageCanvas, kind: PointerKind| {
            let ev = PointerEvent::single(kind, Point::new(400, 300), MouseButton::Left);
            windui::testing::run_with_ctx(|ctx| {
                c.on_pointer(ctx, &ev);
            });
        };

        // ① 滚轮：默认档是**缩放**，不进翻页闸——这条以前没人接，用户就看见"滚轮没用"。
        canvas.chrome.toggle_slideshow();
        assert!(canvas.chrome.slideshow_on(), "夹具：先进入放映");
        send(&mut canvas, PointerKind::Wheel(120));
        assert!(
            !canvas.chrome.slideshow_on(),
            "滚一下滚轮（默认档=缩放）必须当场停放映"
        );

        // ② 左键按下（拖图起手）：同样算插手画面。
        canvas.chrome.toggle_slideshow();
        send(&mut canvas, PointerKind::Down);
        assert!(!canvas.chrome.slideshow_on(), "按下左键之后必须停");
    }

    /// ★ m267：Ctrl+滚轮必须走它自己那一档，不许被当成裸滚轮。
    /// 用户实机「设置是切换图片，实际没效果」：windui 的 WM_MOUSEWHEEL 此前用
    /// `PointerEvent::single`（Mods 全 false），宿主再怎么读 p.mods.ctrl 都白搭。
    /// 用「缩放倍率有没有变」当探针：翻页不改 scale，缩放改 scale。
    fn wheel_changes_scale(
        wheel: crate::core::config::Wheel,
        ctrl_wheel: crate::core::config::CtrlWheel,
        mods: windui::event::Mods,
    ) -> bool {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let (mut canvas, viewer) = zoomed_canvas();
        {
            let mut c = canvas.config.borrow_mut();
            c.wheel = wheel;
            c.ctrl_wheel = ctrl_wheel;
        }
        let before = viewer.borrow().scale();
        let ev = PointerEvent::single_with(
            PointerKind::Wheel(120),
            Point::new(100, 75),
            MouseButton::Left,
            mods,
        );
        windui::testing::run_with_ctx(|ctx| {
            canvas.on_pointer(ctx, &ev);
        });
        let after = viewer.borrow().scale();
        (after - before).abs() > 1e-6
    }

    #[test]
    fn ctrl_wheel_routes_through_the_pointer_mods() {
        use windui::event::Mods;
        let zoom = crate::core::config::WheelAction::Zoom;
        let flip = crate::core::config::WheelAction::Navigate;
        let c_zoom = crate::core::config::WheelAction::Zoom;
        let c_flip = crate::core::config::WheelAction::Navigate;
        let none = Mods::default();
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        // 默认档：裸滚轮缩放、Ctrl+滚轮翻页。
        assert!(
            wheel_changes_scale(zoom, c_flip, none),
            "裸滚轮（默认档）该缩放"
        );
        assert!(
            !wheel_changes_scale(zoom, c_flip, ctrl),
            "Ctrl+滚轮不该缩放，得翻页"
        );
        // 反配档：裸滚轮翻页、Ctrl+滚轮缩放，两条开关各管各的。
        assert!(
            !wheel_changes_scale(flip, c_zoom, none),
            "裸滚轮（反配档）不该缩放"
        );
        assert!(
            wheel_changes_scale(flip, c_zoom, ctrl),
            "Ctrl+滚轮（反配档）该缩放"
        );
    }

    /// m278 判据本体：四行滚轮各读自己那一档，不许四行都读裸滚轮档。
    fn assert_four_wheel_rows(
        c: &Config,
        pick: impl Fn(&windui::event::Mods) -> crate::core::config::WheelAction,
        what: &str,
    ) {
        use windui::event::Mods;
        let none = Mods::default();
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        assert_eq!(pick(&none), c.wheel, "{what}：裸滚轮档");
        assert_eq!(pick(&alt), c.alt_wheel, "{what}：Alt+滚轮被裸滚轮档吃掉了");
        assert_eq!(
            pick(&shift),
            c.shift_wheel,
            "{what}：Shift+滚轮被裸滚轮档吃掉了"
        );
    }

    #[test]
    fn four_wheel_rows_are_independent_and_pairs_are_opposite() {
        let d = Config::default();
        // 出厂：裸滚轮缩放、Ctrl 换图、Alt / Shift 关闭。
        assert_eq!(
            wheel_action_for(&d, &windui::event::Mods::default()),
            crate::core::config::WheelAction::Zoom
        );
        let c = Config {
            alt_wheel: crate::core::config::WheelAction::Opacity,
            shift_wheel: crate::core::config::WheelAction::Navigate,
            ..Config::default()
        };
        assert_four_wheel_rows(&c, |m| wheel_action_for(&c, m), "现状");
        // 一个值管上下两个方向：向下是下拉里写的那一头，向上是反向。
        assert_eq!(
            crate::core::config::WheelAction::Navigate.pair(),
            Some((Action::Next, Action::Prev)),
            "选「下一张 / 上一张」时另一方向必须是上一张"
        );
        assert_eq!(
            crate::core::config::WheelAction::Opacity.pair(),
            Some((Action::OpacityDown, Action::OpacityUp))
        );
        assert_eq!(
            crate::core::config::WheelAction::None.pair(),
            None,
            "「无」不该有反向动作"
        );
    }

    /// 负向臂：m278 之前的「四行都读裸滚轮档」喂同一个判据，必须当场红。
    #[test]
    #[should_panic(expected = "Alt+滚轮被裸滚轮档吃掉了")]
    fn four_wheel_rows_reject_the_single_row_dispatch() {
        let c = Config {
            alt_wheel: crate::core::config::WheelAction::Opacity,
            ..Config::default()
        };
        assert_four_wheel_rows(&c, |_| c.wheel, "旧实现");
    }

    /// m267 负向臂：平台层送 Mods::default()（补丁 027 之前的形状）时，
    /// 「Ctrl+滚轮不该缩放」必须报红；同时也钉住断言本身有没有真在判。
    #[test]
    #[should_panic(expected = "Ctrl+滚轮不该缩放")]
    fn ctrl_wheel_rejects_a_wheel_event_without_mods() {
        use windui::event::Mods;
        assert!(
            !wheel_changes_scale(
                crate::core::config::WheelAction::Zoom,
                crate::core::config::WheelAction::Navigate,
                Mods::default()
            ),
            "Ctrl+滚轮不该缩放"
        );
    }

    /// 指针拖出窗口再松手：所有按下/拖动位必须一起收掉，不能带回窗口继续拖。
    #[test]
    fn pointer_leave_cancels_stuck_drag_and_press_state() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let (mut canvas, viewer) = zoomed_canvas();
        canvas.nav_drag.set(true);
        canvas.lmb_pending.set(Some((10.0, 10.0)));
        canvas.strip_pressed.set(Some(2));
        viewer.borrow_mut().begin_drag((10.0, 10.0));
        assert!(viewer.borrow().dragging());

        let ev = PointerEvent::single(PointerKind::Leave, Point::new(0, 0), MouseButton::Left);
        windui::testing::run_with_ctx(|ctx| {
            canvas.on_pointer(ctx, &ev);
        });

        assert!(!canvas.nav_drag.get(), "鸟瞰拖动态没被收掉");
        assert_eq!(canvas.lmb_pending.get(), None, "待定拖窗位没被清掉");
        assert_eq!(canvas.strip_pressed.get(), None, "缩略图按下位没被清掉");
        assert!(!viewer.borrow().dragging(), "图片平移态没被收掉");
    }

    /// 平移限速（M8）：一个刷新周期内**只画一帧**，但最后那一帧**不许丢**。
    ///
    /// 为什么值得加这道闸：M8 实测手速 120 消息/s 时应用画到 **90–119 帧/s**，而接这块
    /// 屏的是 60 Hz——多出来的三到五成一个也没呈现出去，整机 4.2–5.0% 全花在那儿。
    /// 判据仍是"重绘次数"而不是 CPU：CPU 属于真机（见计划 M8 那张表），帧数属于这里。
    /// 与上一条 M6 用例同一套路子——走生产入口 `on_pointer`，读 `DispatchResult.repaint`。
    #[test]
    fn nav_drag_paces_to_one_repaint_per_period_but_keeps_the_last() {
        use std::time::Duration;
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let (mut canvas, viewer) = zoomed_canvas();
        // 一万年一个周期：这一臂里除了起手那一帧，后面每一步都该落到"挂起"。
        canvas.chrome.pan_interval.set(Duration::from_secs(36_000));
        canvas.nav_drag.set(true);
        let content = canvas.last.get();
        let dst = to_rect(viewer.borrow().dst_rect(from_rect(content)).unwrap());
        let (_box, thumb) = canvas.nav_geom_in(content, content, dst).unwrap();
        let at = |u: f32, v: f32| {
            PointerEvent::single(
                PointerKind::Move,
                Point::new(
                    (thumb.x as f32 + u * thumb.w as f32) as i32,
                    (thumb.y as f32 + v * thumb.h as f32) as i32,
                ),
                MouseButton::Left,
            )
        };
        let repaint = |c: &mut ImageCanvas, ev: &PointerEvent| -> bool {
            windui::testing::run_with_ctx(|ctx| {
                c.on_pointer(ctx, ev);
            })
            .repaint
        };

        assert!(
            repaint(&mut canvas, &at(0.2, 0.5)),
            "起手那一帧不许被限速挡住"
        );
        let pan_after_first = viewer.borrow().pan();
        let mut extra = 0;
        for k in 1..9 {
            if repaint(&mut canvas, &at(0.2 + k as f32 * 0.05, 0.5)) {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "同一周期内一步都不许多画（M8 省的就是这些帧）");
        // 限速挡的是"画"，不是"记账"：pan 必须一路跟着鼠标走，否则补出来的那一帧
        // 画的是旧位置，用户会看见画面比手慢一整段。
        assert_ne!(
            viewer.borrow().pan(),
            pan_after_first,
            "被挂起的 move 也得把 pan 写进状态机"
        );
        // 松手：挂着的那一帧当场交出去。手停下之后不会再有"下一条 move"来补。
        let up = PointerEvent::single(
            PointerKind::Up,
            Point::new(
                (thumb.x as f32 + 0.6 * thumb.w as f32) as i32,
                (thumb.y as f32 + 0.5 * thumb.h as f32) as i32,
            ),
            MouseButton::Left,
        );
        assert!(repaint(&mut canvas, &up), "松手必须把挂着的那帧补出去");
        assert!(
            !repaint(&mut canvas, &up),
            "没挂着时不许虚报，否则每次松手白重绘一整窗"
        );
    }

    /// **回归 2026-10-08 的那次崩溃**：点鸟瞰图时 `Down` 曾把 `self.viewer.borrow()` 的
    /// `Ref` 活到 if-let 块里，块内 `nav_pan_to` 再 `borrow_mut` → `already borrowed`
    /// → panic 出不去 `extern "system"` 的 wnd_proc → 进程直接没了（表象：窗口消失）。
    ///
    /// 这条用例把那一次的调用序列（命中 → 导航 → 继续拖动）整个走一遍：
    /// 只要谁再内联成"在借用期间改 state"，这里立刻 panic，测试会红。
    #[test]
    fn clicking_the_navigator_does_not_double_borrow() {
        let (canvas, viewer) = zoomed_canvas();
        let content = canvas.last.get();
        let dst = viewer
            .borrow()
            .dst_rect(from_rect(content))
            .expect("图应该画得出来");
        let (box_r, _thumb) = canvas
            .nav_geom_in(content, content, to_rect(dst))
            .expect("图比视口大时鸟瞰必须出现");

        // 命中盒子中心。
        let center = (
            box_r.x as f32 + box_r.w as f32 / 2.0,
            box_r.y as f32 + box_r.h as f32 / 2.0,
        );
        assert!(canvas.nav_hit(center).is_some(), "盒子中心竟然没命中鸟瞰");
        let before = viewer.borrow().pan();
        assert!(canvas.nav_grab(center), "nav_grab 应当接管这一击");

        // 缩略图上偏左上角再拉一次（相当于拖动中途）：这是当年真正崩的那句。
        let drift = (
            box_r.x as f32 + box_r.w as f32 * 0.25,
            box_r.y as f32 + box_r.h as f32 * 0.25,
        );
        canvas.nav_pan_at(drift);
        let after = viewer.borrow().pan();
        assert_ne!(before, after, "点鸟瞰后视角没动，导航是坏的");
    }

    /// 鸟瞰盒**不许压住底部工具栏浮条**。
    ///
    /// 窄窗口里居中的浮条会一路横到最右侧，和右下角的盒子撞在一起——表现是
    /// "缩略图盖住了半透明按钮，剩下的半个按钮还点得着"，属于一眼可见的坏。
    #[test]
    fn nav_box_never_covers_the_toolbar_pill() {
        for (w, h) in [(360, 260), (520, 300), (1040, 700), (1920, 1080)] {
            let content = Rect::new(0, 0, w, h);
            let b = ImageCanvas::nav_box_in(content, content);
            // 与 nav_box 同一口径：按"动画组也在"的最宽一行避让。
            let (px, py, pw, ph) = Chrome::pill(
                (0.0, 0.0, w as f32, h as f32),
                true,
                Chrome::cell_reserve(w as f32),
            );
            let pill = Rect::new(px as i32, py as i32, pw as i32, ph as i32);
            assert!(
                !overlaps(b, pill),
                "{w}x{h}: 鸟瞰盒 {b:?} 压住了工具栏 {:?}",
                (px, py, pw, ph)
            );
        }
    }

    /// 开条时的画布：`last` 是整块 content、`last_work` 是扣掉条后剩下的那块——
    /// `paint` 用后者摆工具栏/缩略图，按下/抬起曾错用前者（报障的几何就差这 96px）。
    fn strip_canvas() -> ImageCanvas {
        use crate::core::decode::{Decoder, Done};
        let (tx, _rx) = std::sync::mpsc::channel::<Done>();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let v = Viewer::new(Rc::new(Decoder::spawn_with(tx)), cfg.clone());
        let hub: Hub = Rc::new(RefCell::new(None));
        let canvas = ImageCanvas::new(Rc::new(RefCell::new(v)), Chrome::new(false), cfg, hub);
        canvas.last.set(Rect::new(0, 0, 800, 600));
        canvas
            .last_work
            .set(Rect::new(0, 0, 800, 600 - filmstrip::STRIP_H as i32));
        canvas
    }

    /// 回归 2026-10-12：开条时**画在哪就要点在哪**。
    ///
    /// `paint` 拿 `last_work`（扣掉 96px 的条）摆工具栏；按下若拿整块 content 去
    /// 命中，命中区整体下沉 96px——绘制出来的按钮点不中，落点反而砸进缩略图里。
    #[test]
    fn plugin_submenu_lists_a_discovered_command_plugin() {
        let c = strip_canvas();
        let root = crate::core::paths::plugin_dir().expect("extension dir");
        let dir = root.join("com.rupix.test.echo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create plugin dir");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{
                "schema":1,"id":"com.rupix.test.echo","name":"测试插件",
                "version":"1.0.0","protocol":1,"min_host":"0.0.1",
                "kind":"command","entry":"echo.exe",
                "capabilities":[],"platforms":["windows-x64"]
            }"#,
        )
        .expect("write manifest");

        let items = c.plugin_items();
        assert_eq!(items.len(), 1, "应列出刚发现的命令型插件");
        assert!(items[0].enabled, "无 image.read 的插件不依赖当前图片");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_submenu_has_a_readable_empty_state() {
        let c = strip_canvas();
        let items = c.plugin_items();
        assert_eq!(items.len(), 1, "空目录只应有一枚空态项");
        assert!(!items[0].enabled, "空态项必须不可点");
    }

    #[test]
    fn pressing_where_the_toolbar_is_painted_hits_that_button() {
        let canvas = strip_canvas();
        let work = canvas.work_tuple();
        for anim in [false, true] {
            canvas.chrome.anim.set(anim);
            for b in Btn::ALL {
                let Some((bx, by, bw, bh)) =
                    Chrome::btn_rect(b, work, anim, canvas.chrome.info_cell.get())
                else {
                    assert!(
                        !anim && b.is_anim(),
                        "{b:?} 在 anim={anim} 的行上却拿不到矩形"
                    );
                    assert_eq!(
                        canvas.chrome_hit((0.0, work.3 - 20.0)),
                        None,
                        "隐藏的那几颗不许在浮条上留下命中区"
                    );
                    continue;
                };
                let c = (bx + bw / 2.0, by + bh / 2.0);
                assert_eq!(
                    canvas.chrome_hit(c),
                    Some(Press::Btn(b)),
                    "画在 {c:?} 的按钮按下去没打中自己（anim={anim}）"
                );
            }
        }
    }

    /// 按下时的悬停刷新必须同一份几何：点绘制出来的浮条要能亮起 hover，
    /// 否则按下瞬间悬停状态会指到别处（工具栏该亮不亮、该灭不灭）。
    #[test]
    fn pressing_where_the_toolbar_is_painted_counts_as_toolbar_hover() {
        let canvas = strip_canvas();
        let (px, py, pw, ph) = Chrome::pill(
            canvas.work_tuple(),
            canvas.chrome.anim.get(),
            canvas.chrome.info_cell.get(),
        );
        // 这条用例只验悬停位落对了地方；返回值不算账（夹具是传统模式，恒 false）。
        let _ = canvas.chrome_activity((px + pw / 2.0, py + ph / 2.0));
        assert!(
            canvas.chrome.hover_toolbar.get(),
            "点在绘制的浮条中心却没算作悬停"
        );
    }

    /// 极简模式的**浮现与收走，必须发生在改变显隐位的那一枚事件里**。
    ///
    /// 与 M6/M8 那两条同一套路子：走生产入口 `on_pointer`，读 `DispatchResult.repaint`，
    /// 标脏链路整个参与。回归 2026-10-19 用户报"悬停浮现不流畅、移开有时不隐藏"——
    /// `Chrome::activity` 的返回值当年没人接，浮条只能等**别的**标脏源（跨过某颗按钮的
    /// 高亮、2 秒定时器、解出一帧图），于是慢半拍、时有时无。
    /// ★ 夹具落在 `Enter|Move` 分支里，判据才真的被验到；只测 `activity` 本身挡不住接线丢掉。
    #[test]
    fn minimal_overlay_reveals_and_hides_on_the_move_that_flips_it() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let mut canvas = strip_canvas();
        canvas.chrome.minimal_mode.set(true);
        let mv = |c: &mut ImageCanvas, x: i32, y: i32| -> bool {
            let ev = PointerEvent::single(PointerKind::Move, Point::new(x, y), MouseButton::Left);
            windui::testing::run_with_ctx(|ctx| {
                c.on_pointer(ctx, &ev);
            })
            .repaint
        };
        // 画面正中（三条栏都不亮）→ 顶条：显隐位翻了，这一枚 Move 就得发起重绘。
        assert!(!mv(&mut canvas, 400, 300), "起手就在暗处，不该报重绘");
        assert!(
            mv(&mut canvas, 400, 10),
            "移进顶条这一枚就该重绘——浮现慢半拍就是这里漏的"
        );
        assert!(canvas.chrome.hover_title.get(), "顶条悬停位没跟上");
        // 同一条栏里继续横挪：位没翻 ⇒ 一帧都不许多刷（流畅的另一半是别空转）。
        assert!(
            !mv(&mut canvas, 500, 12),
            "顶条内挪鼠标不该发起重绘（每枚 Move 白刷一整帧是另一种不流畅）"
        );
        // 移回正中：又翻一次 ⇒ 收走也当场画。
        assert!(
            mv(&mut canvas, 400, 300),
            "移开顶条这一枚就该重绘——该隐的隐不早"
        );
        assert!(!canvas.chrome.hover_title.get(), "移开之后顶条还挂在悬停态");
    }

    /// 箭头边带那一翻也必须**当场**重绘：极简模式下箭头是"靠边才出现"的图形，
    /// 慢半拍就是用户嘴里的"我明明靠边了怎么什么都没有"（2026-10-19 翻案同一轮）。
    ///
    /// ★ 夹具选在 `(20, 300)`：它在箭头圆（心 36、半径 24）**外面**、在边带（64）里面，
    ///   所以这一枚 Move 里除了 `hover_edge` 没有任何别的位置会变——报上来的重绘只可能是
    ///   边带那一位给的，测不到就是没接上，不会冒充。
    #[test]
    fn moving_into_the_arrow_edge_band_repaints_immediately() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let mut canvas = strip_canvas();
        canvas.chrome.minimal_mode.set(true);
        let mv = |c: &mut ImageCanvas, x: i32, y: i32| -> bool {
            let ev = PointerEvent::single(PointerKind::Move, Point::new(x, y), MouseButton::Left);
            windui::testing::run_with_ctx(|ctx| {
                c.on_pointer(ctx, &ev);
            })
            .repaint
        };
        assert!(!mv(&mut canvas, 400, 300), "画面正中不该报重绘");
        assert!(
            mv(&mut canvas, 20, 300),
            "进左边带这一枚就该重绘——箭头要当场出现"
        );
        assert_eq!(canvas.chrome.hover_edge.get(), (true, false));
        assert!(
            mv(&mut canvas, 400, 300),
            "出边带这一枚就该重绘——箭头要当场收走"
        );
        assert_eq!(canvas.chrome.hover_edge.get(), (false, false));
    }

    /// **压在按钮上**把指针移出窗口：浮条要跟着一起收走。
    ///
    /// 回归同一条报障的另一半：`Leave` 分支原写成一条 `hover_btn...is_some() || hover_toolbar...`
    /// 的链，`||` 一短路，第一位变了后面几位就根本不覆写 ⇒ 恰好停在按钮上离开时
    /// `hover_toolbar` 还挂在 `true`，而 `tick_hide` 读滞留的 `last_pos`（仍在浮条里）判成
    /// 贴边豁免 ⇒ 永远不隐。所以夹具**必须**先把 `hover_btn` 点亮，否则测不到那条短路。
    #[test]
    fn leaving_while_hovering_a_button_clears_the_pill_too() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let mut canvas = strip_canvas();
        canvas.chrome.minimal_mode.set(true);
        let mv = |c: &mut ImageCanvas, x: i32, y: i32| {
            let ev = PointerEvent::single(PointerKind::Move, Point::new(x, y), MouseButton::Left);
            windui::testing::run_with_ctx(|ctx| {
                c.on_pointer(ctx, &ev);
            })
            .repaint
        };
        // 落到浮条中心：`hover_toolbar` 亮，且这一枚要报重绘（浮现接上了）。
        let (px, py, pw, ph) = Chrome::pill(
            canvas.work_tuple(),
            canvas.chrome.anim.get(),
            canvas.chrome.info_cell.get(),
        );
        assert!(mv(
            &mut canvas,
            (px + pw / 2.0) as i32,
            (py + ph / 2.0) as i32
        ));
        // 再把"正好压在按钮上"这一位也点亮（短路链的第一位）。
        canvas.chrome.hover_btn.set(Some(Btn::In));
        canvas.chrome.hover_arrow.set(Some(Arrow::Prev));
        canvas.strip_hover.set(Some(2));
        // 指针离开窗口（框架发一枚落在一切之外的 Move，控件收到 Leave）。
        let leave = PointerEvent::single(PointerKind::Leave, Point::new(-1, -1), MouseButton::Left);
        let repainted = windui::testing::run_with_ctx(|ctx| {
            canvas.on_pointer(ctx, &leave);
        })
        .repaint;
        assert!(repainted, "Leave 清了悬停态却不重绘，屏幕就还挂着浮条");
        assert!(
            !canvas.chrome.hover_toolbar.get()
                && canvas.chrome.hover_btn.get().is_none()
                && canvas.chrome.hover_arrow.get().is_none()
                && !canvas.chrome.hover_info.get()
                && !canvas.chrome.hover_title.get()
                && canvas.strip_hover.get().is_none(),
            "Leave 之后仍有悬停位没被清掉（`||` 短路回来了）"
        );
    }

    /// ★ m253：条换边之后，「扣哪一维」只许由 `work_of` 一处分派。
    ///
    /// 两边都必须从**完整 content** 推导：底条占满宽、左条占满高。这条钉子盯的是
    /// "拿 work 再扣一次"那类错——那种错在底条上看不出来（work 宽 == content 宽），
    /// 一贴左就立刻把画面扣窄两条。
    #[test]
    fn strip_side_decides_which_dimension_is_taken() {
        let c = Rect::new(10, 20, 900, 700);

        let b = work_of(c, Some(filmstrip::Side::Bottom), 0);
        assert_eq!((b.x, b.y, b.w), (10, 20, 900), "底条只扣高，宽度原样");
        assert_eq!(b.h, 700 - filmstrip::STRIP_H as i32);
        assert_eq!(b.x + b.w, c.x + c.w);

        let l = work_of(c, Some(filmstrip::Side::Left), 0);
        assert_eq!((l.y, l.h), (20, 700), "左条只扣宽，高度原样");
        assert_eq!(l.x, 10 + filmstrip::STRIP_W as i32, "画面要让出左边一条");
        assert_eq!(l.w, 900 - filmstrip::STRIP_W as i32);
        assert_eq!(l.y + l.h, c.y + c.h);

        // 没有条时原样还回去。
        let none = work_of(c, None, 0);
        assert_eq!((none.x, none.y, none.w, none.h), (10, 20, 900, 700));

        // 极端窄窗：扣到最后不许出现 0 宽/0 高（除零会造出 NaN 坐标）。
        let tiny = Rect::new(0, 0, 10, 10);
        assert!(work_of(tiny, Some(filmstrip::Side::Left), 0).w >= 1);
        assert!(work_of(tiny, Some(filmstrip::Side::Bottom), 0).h >= 1);
        assert!(work_of(tiny, None, 0).w >= 1);
    }

    /// m300：文件树在**最外侧**，条在它右边。树和条同时开时必须各扣一条、互不重叠
    /// ——早先两边各自从 `content` 扣一遍，条就会压在树上、而画面被扣掉两条宽。
    #[test]
    fn tree_sits_outside_the_strip_and_both_are_taken_out() {
        let c = Rect::new(10, 20, 900, 700);
        let tw = 180;
        let body = strip_area(c, tw);
        assert_eq!((body.x, body.w), (10 + 180, 900 - 180), "树占最外侧那一条");

        // 条贴左：条在树右边，画面再往右让一条 —— 两条互不重叠。
        let w = work_of(c, Some(filmstrip::Side::Left), tw);
        assert_eq!(w.x, 10 + 180 + filmstrip::STRIP_W as i32);
        assert_eq!(w.w, 900 - 180 - filmstrip::STRIP_W as i32);
        assert_eq!(w.h, 700, "左条不扣高");
        assert!(
            w.x >= body.x + filmstrip::STRIP_W as i32,
            "条的右缘必须已经越过树"
        );

        // 条贴底：两者正交，画面只扣宽（树）和高（条）。
        let w = work_of(c, Some(filmstrip::Side::Bottom), tw);
        assert_eq!(w.x, 10 + 180);
        assert_eq!(w.w, 900 - 180);
        assert_eq!(w.h, 700 - filmstrip::STRIP_H as i32);
        assert_eq!(w.y, 20);

        // 没有树时逐字节等于 m300 之前的行为。
        assert_eq!(
            work_of(c, Some(filmstrip::Side::Left), 0),
            filmstrip_work_of_old(c, filmstrip::Side::Left)
        );
    }

    /// 树必须整体待在画面左缘之外：它和画面、缩略图条三者不许有任何一个像素重叠。
    /// 这条是 m300 布局的负向臂——早先两边各自从 `content` 扣一遍时，条会压到树上。
    #[test]
    fn the_tree_panel_never_overlaps_the_view_or_the_strip() {
        let c = Rect::new(0, 0, 900, 700);
        let tw = 180;
        let p = tree::panel(c, tw as f32);
        let area = strip_area(c, tw);
        assert_eq!(p.right(), area.x, "树的右缘正好是「条 + 画面」那条的起点");
        let w = work_of(c, Some(filmstrip::Side::Left), tw);
        assert!(p.right() <= w.x, "树必须整体在画面左缘之外");
        // 底条时画面起点紧贴树右缘（正交，不额外让宽）。
        let wb = work_of(c, Some(filmstrip::Side::Bottom), tw);
        assert_eq!(wb.x, p.right());
    }

    /// m300：树上的按下必须被树**吞掉**——不进拖窗口、不进拖图、也不碰缩略图条。
    /// 几何由测试自己摆（与 `strip_items` 那套同款：`paint` 只负责写，命中读同一份）。
    #[test]
    fn a_click_on_the_tree_never_becomes_a_canvas_gesture() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;

        let mut canvas = strip_canvas();
        // 没有清单时 `gallery_dir()` 是 None ⇒ 点了也不会真去换层（`src` 取不到），
        // 正好单测"吞事件"这一半。
        *canvas.tree_nodes.borrow_mut() = (0..4)
            .map(|i| crate::core::archive::TreeNode {
                segs: if i == 0 {
                    Vec::new()
                } else {
                    vec![format!("n{i}")]
                },
                label: format!("n{i}"),
                dir: true,
                depth: i,
                images: i,
            })
            .collect();
        let panel = tree::panel(Rect::new(0, 0, 800, 600), tree::TREE_W);
        canvas.tree_at.set(Some(panel));

        let r = tree::row_rect(panel, 2);
        let at = Point::new(r.x + 30, r.y + 10);
        let down = Event::Pointer(PointerEvent::single(
            PointerKind::Down,
            at,
            MouseButton::Left,
        ));
        // ★ 判据是 `on_event` 的**返回值**（已消费），不是 `DispatchResult::repaint`：
        // 点自己那一层刻意不重开（见 `same` 那句），所以这一下可以什么都不脏——
        // 拿 repaint 当"吞掉了"会把这个正确的行为判成 bug（本条单测第一版就是这么红的）。
        let eaten = std::cell::Cell::new(false);
        let feed = |c: &mut ImageCanvas, e: &Event| -> bool {
            windui::testing::run_with_ctx(|ctx| {
                eaten.set(c.on_event(ctx, e));
            });
            eaten.get()
        };
        assert!(feed(&mut canvas, &down), "树上的按下必须被吞掉");
        assert_eq!(canvas.strip_pressed.get(), None, "不许顺带按下某一格缩略图");
        assert!(!canvas.nav_drag.get(), "不许把树上的按下当成鸟瞰拖动");

        // 反向臂：同一点、树没开（`tree_at` 为 None）时必须落成**画面手势**。
        // 少了这一臂，"被吞掉"可能只是被别处顺手吃了（比如那块地正好压在某个
        // 热区上），本条就测不到树——它只会一直绿着。
        let mut bare = strip_canvas();
        *bare.tree_nodes.borrow_mut() = canvas.tree_nodes.borrow().clone();
        bare.tree_at.set(None);
        assert!(feed(&mut bare, &down), "没有树时这一击也该被画面接手");
        let acted = bare.lmb_pending.get().is_some()
            || bare.viewer.borrow().dragging()
            || bare.strip_pressed.get().is_some()
            || bare.nav_drag.get();
        assert!(acted, "树没开时同一点必须落成画面手势（树外不许被白吞）");
        assert_eq!(canvas.lmb_pending.get(), None, "树的点击不许留下待定拖窗位");
    }

    /// ★ m312：锁定 = **整窗**封锁，文件树与缩略图条必须一起停摆。
    ///
    /// 改前这条判据散在各分支里：滚轮一处、缩略图条一处、右键几处，**树漏了**——
    /// 锁住窗口点树上的某一行照样换层；条那一格虽然拦住了点击，悬停高亮却还亮着，
    /// 看着照样像能点（用户实报「锁住窗口的时候，目录和缩略图好像没有一起锁住」）。
    #[test]
    fn locked_window_swallows_the_tree_and_the_strip_too() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;

        let mut canvas = strip_canvas();
        *canvas.tree_nodes.borrow_mut() = (0..4)
            .map(|i| crate::core::archive::TreeNode {
                segs: if i == 0 {
                    Vec::new()
                } else {
                    vec![format!("n{i}")]
                },
                label: format!("n{i}"),
                dir: true,
                depth: i,
                images: i,
            })
            .collect();
        let panel = tree::panel(Rect::new(0, 0, 800, 600), tree::TREE_W);
        canvas.tree_at.set(Some(panel));
        let r = tree::row_rect(panel, 2);
        let at = Point::new(r.x + 30, r.y + 10);

        let eaten = std::cell::Cell::new(false);
        let feed = |c: &mut ImageCanvas, e: &Event| -> bool {
            windui::testing::run_with_ctx(|ctx| {
                eaten.set(c.on_event(ctx, e));
            });
            eaten.get()
        };
        let ev = |kind, at| Event::Pointer(PointerEvent::single(kind, at, MouseButton::Left));

        // 负向臂（**先做**）：解锁时同一点要能亮起悬停——证明它真落在树行上，
        // 否则下面那些"没亮"可能只是这个夹具压根没命中树，整条用例在空转。
        assert!(feed(&mut canvas, &ev(PointerKind::Move, at)));
        assert_eq!(canvas.tree_hover.get(), Some(2), "夹具没落在第 2 行上");

        canvas.chrome.locked.set(true);

        // ① 移动：吞掉，并且**不许**在树上留悬停（锁着的窗口看着像还能点）。
        assert!(feed(&mut canvas, &ev(PointerKind::Move, at)));
        assert_eq!(canvas.tree_hover.get(), None, "锁定后树上还留着悬停高亮");

        // ② 条那一格同理：先把悬停点亮，锁着再划过来必须被清掉。
        canvas.strip_hover.set(Some(1));
        assert!(feed(&mut canvas, &ev(PointerKind::Move, at)));
        assert_eq!(canvas.strip_hover.get(), None, "锁定后条上还留着悬停高亮");

        // ③ 按下：吞掉，且不许起任何画面手势 / 拖树状态。
        assert!(feed(&mut canvas, &ev(PointerKind::Down, at)));
        assert_eq!(
            canvas.lmb_pending.get(),
            None,
            "锁定后按下不许起拖动/双击那套"
        );
        assert_eq!(
            canvas.strip_pressed.get(),
            None,
            "锁定后不许按下某一格缩略图"
        );
        assert!(!canvas.nav_drag.get(), "锁定后不许把按下当成鸟瞰拖动");
        assert!(
            canvas.tree_scrub.get().is_none(),
            "锁定后不许进入拖树滚动条的状态"
        );

        // ④ 解锁钮必须照常放行——吞成铁板的话，锁了就永远出不来。
        //   位置从 `chrome_hit` 自己扫出来，不手抄几何：锁定时它只认这一颗，
        //   扫得到也顺带证明"别的钮都不可命中"这条前提还成立。
        let lock_at = (0..800)
            .find_map(|x| {
                let q = (x as f32 + 0.5, 12.0);
                (canvas.chrome_hit(q) == Some(Press::TitleBtn(TitleBtn::Lock))).then_some(q)
            })
            .expect("锁定时标题栏里该有且只有锁钮可命中");
        assert!(feed(
            &mut canvas,
            &ev(
                PointerKind::Down,
                Point::new(lock_at.0 as i32, lock_at.1 as i32)
            )
        ));
        assert_eq!(
            canvas.chrome.pressed_title_btn.get(),
            Some(TitleBtn::Lock),
            "解锁钮被那道闸一起吞了：锁死出不来"
        );
    }

    /// m300 之前的算式原样抄一份：树宽为 0 时新实现必须与它完全一致。
    fn filmstrip_work_of_old(content: Rect, side: filmstrip::Side) -> Rect {
        match side {
            filmstrip::Side::Bottom => Rect::new(
                content.x,
                content.y,
                content.w,
                (content.h as f32 - filmstrip::STRIP_H).max(1.0) as i32,
            ),
            filmstrip::Side::Left => Rect::new(
                content.x + filmstrip::STRIP_W as i32,
                content.y,
                (content.w as f32 - filmstrip::STRIP_W).max(1.0) as i32,
                content.h,
            ),
        }
    }

    /// 缩略图格里的任何一点都不许被当成工具栏按钮——条开着时命中区下沉 96px
    /// 正好压进格子里（报障：点缩略图反而触发工具栏按钮，chrome 命中抢先于条）。
    #[test]
    fn pressing_a_filmstrip_item_never_hits_a_toolbar_button() {
        let canvas = strip_canvas();
        // `paint` 把格子摆在「完整视口」底部：与它同一份 view。
        let items = filmstrip::layout(
            filmstrip::Placement::new(filmstrip::Side::Bottom, (0.0, 0.0, 800.0, 600.0)),
            filmstrip::capacity(filmstrip::Side::Bottom, 800.0),
        );
        assert!(!items.is_empty());
        for (i, r) in items.iter().enumerate() {
            for fy in [0.1, 0.5, 0.9] {
                for fx in [0.1, 0.5, 0.9] {
                    let p = (r.x as f32 + r.w as f32 * fx, r.y as f32 + r.h as f32 * fy);
                    assert!(
                        canvas.chrome_hit(p).is_none(),
                        "缩略图第 {} 格的 {p:?} 按下去命中了工具栏",
                        i + 1
                    );
                }
            }
        }
    }

    // ── 取色器模式（m217）──────────────────────────────────────────
    //
    // 全走生产事件入口 on_event：状态位、重绘请求、以及"别把取样解释成拖窗口"
    // 都是可数的判据，不靠肉眼。

    #[test]
    fn picker_mode_records_position_then_closes_on_click_or_esc() {
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        let (mut canvas, _viewer) = zoomed_canvas();
        let chrome = canvas.chrome.clone();
        chrome.picker.set(true);

        let at = (100.0_f32, 75.0_f32);
        let pointer = |kind| {
            Event::Pointer(PointerEvent::single(
                kind,
                Point::new(at.0 as i32, at.1 as i32),
                MouseButton::Left,
            ))
        };
        let key = |k| {
            Event::Key(KeyEvent {
                key: k,
                pressed: true,
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            })
        };
        let feed = |c: &mut ImageCanvas, e: &Event| -> bool {
            windui::testing::run_with_ctx(|ctx| {
                c.on_event(ctx, e);
            })
            .repaint
        };

        // ① Move 记下位置并标脏；同一位置复读不重画一整帧（浮层跟随光标是唯一
        //    必须跟着 Move 重绘的东西，位置没动就不该有第二次）。
        assert!(feed(&mut canvas, &pointer(PointerKind::Move)));
        assert_eq!(chrome.picker_at.get(), Some(at));
        assert!(
            !feed(&mut canvas, &pointer(PointerKind::Move)),
            "同一位置不该再重画"
        );

        // ② 左键：取样后立刻退出模式，且不进拖窗口/拖图那条路。
        assert!(feed(&mut canvas, &pointer(PointerKind::Down)));
        assert!(!chrome.picker.get(), "点完就该退出取色模式");
        assert_eq!(chrome.picker_at.get(), None);
        assert_eq!(
            canvas.lmb_pending.get(),
            None,
            "取样不该被解释成拖窗口（lmb_pending 是那条路的起点）"
        );

        // ③ Esc 优先退出：不让「Esc 关窗」的动作把窗口一起带走。
        chrome.picker.set(true);
        assert!(feed(&mut canvas, &key(Key::Escape)));
        assert!(!chrome.picker.get());
        assert_eq!(chrome.picker_at.get(), None);

        // ④ 非 Esc 键照常派发且**不关模式**——切图/缩放时可连续取色。
        chrome.picker.set(true);
        assert!(feed(&mut canvas, &key(Key::Char('t'))));
        assert!(chrome.picker.get(), "T 键不该把取色器关掉");
    }

    #[test]
    fn current_image_submenu_keeps_four_items_and_the_picker_switch() {
        let (canvas, _) = zoomed_canvas();
        let items = canvas.current_image_items(true);
        assert_eq!(items.len(), 5, "本图组 = 三项常驻 + 分隔线 + 取色器");
        assert_eq!(items[0].label, "RAW 参数…");
        assert_eq!(items[1].label, "跳转到…帧/页");
        assert_eq!(items[2].label, "旋转角度…", "m280 新增的实时旋转入口");
        assert!(items[3].separator, "三项动作与开关之间留一条分隔线");
        assert_eq!(items[4].label, "取色器");

        // 前两项按格式置灰：测试画布是静态合成图，既不是无头裸像素、
        // 也没有第二帧/页可去。第三项只要求「有图」，所以是可点的。
        assert!(
            !items[0].enabled,
            "普通图片不该能改 RAW 参数（相机 RAW 同理）"
        );
        assert!(!items[1].enabled, "静态图没有第二帧/页可跳");
        assert!(items[2].enabled, "有图就能转，不挑格式");

        let it = &items[4];
        assert!(it.checkable, "菜单里要能看出开没开");
        assert!(!it.checked, "默认关");
        assert!(it.enabled, "有图时可点");
        let sc = it.shortcut.as_deref().expect("菜单项要显示 K 快捷键");
        assert!(
            sc.to_uppercase().contains('K'),
            "快捷键提示该含 K，实得 {sc}"
        );
        assert!(
            !canvas.current_image_items(false)[4].enabled,
            "空窗没有像素可采样，取色器必须置灰"
        );
    }

    /// m322：来源与辅图翻页都是“二选一 ▸”子菜单；`交换主辅`是动作项，
    /// 不能像开关那样带勾选（第三参是 `checked` 不是 `enabled`）。
    #[test]
    fn companion_menu_uses_submenus_and_swap_is_never_checked() {
        let (canvas, _) = zoomed_canvas();
        canvas.chrome.companion.set(true);
        *canvas.chrome.companion_picked.borrow_mut() =
            Some(std::path::PathBuf::from(r"C:\pics\sub\a.jpg"));

        let items = canvas.companion_items();
        assert!(
            !items.iter().any(|i| i.separator),
            "主辅视图子菜单条目不多，分隔线只会把一组功能切碎"
        );
        assert!(
            !items.iter().any(|i| i.label.starts_with("活动窗")),
            "活动窗只靠点击 pane 切换，菜单不再重复放入口"
        );
        let source = items
            .iter()
            .find(|i| i.label == "辅图来源")
            .expect("来源应是子菜单");
        assert_eq!(source.submenu.len(), 2, "来源只有两种：下一张 / 自选");
        assert_eq!(source.submenu[0].label, "主序列下一张");
        assert!(source.submenu[0].checkable && source.submenu[0].checked);
        assert!(
            source.submenu[1].label.contains(r"sub\a.jpg"),
            "自选项要带来源路径，实得 {}",
            source.submenu[1].label
        );
        assert!(!source.submenu[1].checked);

        let flip = items
            .iter()
            .find(|i| i.label == "辅图翻页")
            .expect("辅图翻页应是子菜单");
        assert_eq!(flip.submenu.len(), 3, "辅图翻页有跟随 / 固定 / 自由");
        assert_eq!(flip.submenu[0].label, "跟随主图");
        assert_eq!(flip.submenu[1].label, "固定不动");
        assert_eq!(flip.submenu[2].label, "自由翻页");
        assert!(flip.submenu[0].checked, "默认跟随");
        assert!(
            !items.iter().any(|i| i.label == "同步缩放"),
            "同步缩放对尺寸不同的图没有普遍意义，已撤掉"
        );

        let swap = items
            .iter()
            .find(|i| i.label == "交换主辅")
            .expect("交换主辅应在菜单里");
        assert!(!swap.checkable, "交换主辅是动作，不是开关");
        assert!(!swap.checked, "动作项永远不带勾");
        let sync = items
            .iter()
            .find(|i| i.label == "同步缩放/平移")
            .expect("同步视图应在菜单里");
        assert!(sync.checkable && !sync.checked, "同步视图默认关");
    }

    /// 活动辅图时，工具栏/菜单的放大、缩小、适应、1:1 必须改辅图，不能偷偷改主图。
    #[test]
    fn companion_zoom_buttons_route_only_when_companion_is_active() {
        let chrome = Chrome::new(false);
        let zoom = std::cell::Cell::new(1.0);
        let pan = std::cell::Cell::new((10.0, 10.0));
        let fit = std::cell::Cell::new(0.5);
        // 主图活动：一颗都不接。
        assert!(!companion_zoom_button(Btn::In, &chrome, &zoom, &pan, &fit));
        assert_eq!(zoom.get(), 1.0);
        // 辅图活动：+ 一档。
        chrome.companion.set(true);
        chrome.active_pane.set(Pane::Companion);
        assert!(companion_zoom_button(Btn::In, &chrome, &zoom, &pan, &fit));
        assert_eq!(zoom.get(), 1.25);
        // 1:1 = 1 / fit；平移归位。
        assert!(companion_zoom_button(
            Btn::Actual,
            &chrome,
            &zoom,
            &pan,
            &fit
        ));
        assert_eq!(zoom.get(), 2.0);
        assert_eq!(pan.get(), (0.0, 0.0));
        // 非缩放按钮不归它。
        assert!(!companion_zoom_button(
            Btn::FrameFwd,
            &chrome,
            &zoom,
            &pan,
            &fit
        ));
    }

    /// 拖放落到右 pane 时进辅图；几何为零（辅图未开）时一律不进。
    #[test]
    fn pane_geom_routes_drop_by_companion_rect() {
        let g = PaneGeom::new();
        assert!(!g.hits_companion(600, 100), "辅图未开时右半边不算辅图");
        g.companion.set(Rect::new(500, 0, 500, 600));
        assert!(g.hits_companion(600, 100));
        assert!(!g.hits_companion(499, 100));
        assert!(!g.hits_companion(1200, 100));
    }

    /// 辅图解码框的上限必须**等比**生效：单边与总像素两道闸都不能把长宽比拉歪。
    #[test]
    fn companion_decode_box_caps_keep_aspect_and_never_collapse() {
        let pane = Rect::new(0, 0, 800, 600);
        assert_eq!(companion_decode_box(pane, 1.0), (800, 600));
        let (w, h) = companion_decode_box(pane, 8.0);
        assert!(w <= COMPANION_MAX_SIDE as u32 && h <= COMPANION_MAX_SIDE as u32);
        assert!((w as f32 / h as f32 - 800.0 / 600.0).abs() < 0.02);
        assert!(w as u64 * h as u64 <= COMPANION_MAX_PIXELS as u64);
        let (w, h) = companion_decode_box(Rect::new(0, 0, 1, 1), 0.1);
        assert!(w >= 1 && h >= 1, "极小 pane 不许算出 0×0");
    }
}
