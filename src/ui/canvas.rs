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

use crate::core::config::Config;
use crate::core::gallery::Jump;
use crate::core::geometry::RectI;
use crate::core::pixels::MAX_TEXTURE_SIDE;
// 鸟瞰 / 浮层的配色取自全 ui 共用的令牌（[`crate::ui::tokens`]），本文件不自带色值。
// 别名引入：跟随主题那组是函数，名字短（`empty_bg` / `hint_text`），逐个 import 易撞名。
use super::chrome::{self, Arrow, Btn, Press, TitleBtn};
use super::plugin_settings::{LiveState, RunPrompt};
use super::tokens as tk;
use super::{filmstrip, settings};
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

pub struct ImageCanvas {
    viewer: Rc<RefCell<Viewer>>,
    chrome: Rc<Chrome>,
    /// 配置（设置对话框写、双击行为读）。
    config: Rc<RefCell<Config>>,
    /// 设置窗信号句柄（打开设置窗时填充，主窗轮询落盘）。
    hub: Hub,
    /// 最近一次 `paint` 拿到的内容矩形：事件回调里要算中心点，而事件不带几何信息。
    last: Cell<Rect>,
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
    /// 扣掉缩略图条之后**留给图片**的内容矩形。
    ///
    /// 条是常驻的一块地（不是盖在画面上的浮层），所以摆图、缩放锚点、鸟瞰几何
    /// 都用这一份；悬浮工具栏那一层仍用完整的 `last`（它本来就是浮层）。
    last_work: Cell<Rect>,
}

/// 滚轮翻页档每张图需要的增量（一格 WHEEL_DELTA）。
const WHEEL_STEP: i32 = 120;

/// 把一次滚轮增量并进累计器，攒够一格就给出**一个**翻页方向（`0` = 还没攒够）。
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

/// 左键挪动多少才算"拖动窗口"而不是"点一下"（逻辑 dp）。
///
/// **必须有这个阈值**：直接按下就拖会走系统的窗口移动循环，那是个模态循环，
/// 第二次按下进不了本控件——双击关窗（本应用的主力交互之一）当场失效。
/// 比系统双击允许的抖动略大一点：手抖不会被当成拖动，想拖也不会觉得迟钝。
const DRAG_SLOP: f32 = 4.0;

/// 这次移动算不算"拖动"（越过 [`DRAG_SLOP`]）。
///
/// 单独抽成纯函数是为了能单测：**双击期间的手抖绝不能被当成拖窗口**，
/// 一旦判错，双击关窗（本应用的主力交互）就再也没机会发生了。
fn beyond_slop(start: (f32, f32), pos: (f32, f32)) -> bool {
    let dx = pos.0 - start.0;
    let dy = pos.1 - start.1;
    dx * dx + dy * dy >= DRAG_SLOP * DRAG_SLOP
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
///   上限本体是 `Viewer::pan_limits` → `overscroll`。仍然拖不动的只剩"还没有图"（空窗）
///   与"视口还没量出来"两种——那时那一按留着拖窗口，**不许把拖窗口这条路堵死**。
/// - `!in_title`：指针不在顶部标题栏那一条里——**标题栏永远带走窗口**（判据单一来源
///   见 [`Chrome::in_title_strip`]）。
fn left_drag(drag_image: bool, can_pan: bool, in_title: bool, fullscreen: bool) -> LeftDrag {
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
    if drag_image && can_pan && !in_title {
        LeftDrag::PanImage
    } else {
        LeftDrag::Window
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

/// 实时调色纹理：键 = (源位图 gen, 六个参数)，变一次才重算一次 + 重新上传。
///
/// 只变换**已经派生到显示档**的那份位图（视口尺寸量级），原图/base 一字节不动：
/// 8 MP 整图每帧重算会卡住拖动，而显示档在适应窗口时通常不到 2 MP；查表版变换
/// 又比逐像素除法快一个量级。原图档放大到 100% 以上时才等于整图，那时一帧仍只算一次。
struct TintCache {
    /// (显示档 gen, 六个 gain/bias, Gamma)。三个都相同才复用上一帧的位图。
    key: Option<(u64, [i32; 6], i32)>,
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

    fn obtain(&mut self, src: &RgbaImage, v: [i32; 6], gamma: i32) -> Option<&Image> {
        if self.key != Some((src.gen, v, gamma)) {
            let px = crate::core::pixels::channel_gamma_lut(
                &src.rgba,
                [v[0], v[1], v[2]],
                [v[3], v[4], v[5]],
                gamma,
            );
            self.img = Some(RgbaImage::new(src.width, src.height, px));
            self.key = Some((src.gen, v, gamma));
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
    let revert: Rc<dyn Fn(&mut EventCtx)> = Rc::new(move |ctx| {
        // 两个来源一起还原：显示层调色 + 插件正式写回的编辑覆盖。
        let had_tint = live_revert.tint.get().is_some();
        live_revert.tint.set(None);
        let r = live_revert.reset.get();
        live_revert.reset.set(r + 1);
        let restored = v2.borrow_mut().revert_edited(&cur_r);
        if had_tint || restored {
            log::info!(
                "[plugin-edit] verdict=reverted file={} tint={} overlay={}",
                cur_r.display(),
                had_tint,
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
            nav_drag: Cell::new(false),
            lmb_pending: Cell::new(None),
            wheel_acc: Cell::new(0),
            main_img: RefCell::new(ImageCache::new().pixelated()),
            nav_img: RefCell::new(ImageCache::new()),
            live: LiveState::new(),
            tint_main: RefCell::new(TintCache::new_pixelated()),
            tint_nav: RefCell::new(TintCache::new()),
            strip_items: RefCell::new(Vec::new()),
            strip_pressed: Cell::new(None),
            strip_hover: Cell::new(None),
            strip: Strip::new(),
            last_work: Cell::new(Rect::new(0, 0, 1, 1)),
        }
    }

    fn center(&self) -> (f32, f32) {
        // 用扣掉缩略图条之后的矩形：条占了底部一条，画面中心就该往上抬，
        // 否则滚轮缩放与鸟瞰导航的锚点都会偏。
        let c = self.last_work.get();
        (c.x as f32 + c.w as f32 / 2.0, c.y as f32 + c.h as f32 / 2.0)
    }

    /// 当前是否显示底部缩略图条（决定要不要扣布局高度）。
    fn strip_on(&self) -> bool {
        self.chrome.filmstrip.get() && self.viewer.borrow().gallery_len() > 0
    }

    /// 鸟瞰几何：`Some((导航盒, 缩略图区域))`。仅当图显示尺寸超出视口时。
    /// paint（绘制层）与事件（命中/拖动）共用同一份几何，保证两处一致。
    fn nav_geom(&self, content: Rect, dst: Rect) -> Option<(Rect, Rect)> {
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
        let anchor = Self::nav_box(content);
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
    fn nav_box(content: Rect) -> Rect {
        let view = (
            content.x as f32,
            content.y as f32,
            content.w as f32,
            content.h as f32,
        );
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
        let content = self.last_work.get();
        let v = self.viewer.borrow();
        let dst = v.dst_rect(from_rect(content))?;
        let geom = self.nav_geom(content, to_rect(dst))?;
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
        let content = self.last_work.get();
        let dst = self.viewer.borrow().dst_rect(from_rect(content));
        let Some(dst) = dst else { return false };
        let Some((_, thumb)) = self.nav_geom(content, to_rect(dst)) else {
            return false;
        };
        self.nav_pan_to(to_rect(dst), thumb, pos)
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

    /// 执行一次悬浮层按钮/箭头/标题栏按钮的动作。调用点保证命中仍成立。
    fn exec(&self, ctx: &mut EventCtx, press: Press) {
        match press {
            Press::Btn(b) => exec_btn(ctx, &self.viewer, &self.chrome, b),
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

    /// 右键菜单——与菠萝看图同构，按分组与可达性重排过：
    ///
    /// ```text
    /// 打开…
    /// 复制 ▸                    画面 PNG / 原文件 / 文件路径（m248 上移，与「打开」同为文件类）
    /// RAW 参数…                 ← 常驻；仅无头裸像素可点（相机 RAW / 普通图片置灰）
    /// 属性…                     图片信息窗（m247 从浮条收编到顶层）
    /// ─────
    /// 操作 ▸            **严格等于底部浮条**（m247 后只收操作类，同一份 `Btn::ALL`）
    /// 工具 ▸                    宿主内置低频辅助（取色器等）
    /// 插件 ▸                    外部进程命令
    /// ─────
    /// 外观 ▸                    极简模式 / 棋盘格背景 / 缩略图条 / 显示按钮（勾选）/
    ///                        主题色 ▸ 深色·浅色 / 不透明度 ▸ 10~100%
    /// 幻灯片放映 / 停止放映
    /// 左键拖图（勾选）
    /// ─────
    /// 设置…（「关于」不再单列，它是设置窗五页里的最后一项，见 `settings::rows::NAV_LABELS`）
    /// ```
    ///
    /// 「外观 ▸」收的是**改变画面上出现什么 / 长什么样**的开关（m246 改名并补齐）：
    /// 与设置 → 选项 → 外观卡同一条轴。放映改的是看图行为，左键拖图改的是手势分工，
    /// 都不是外观，所以留在顶层；放映还因为不在浮条 `Btn::ALL` 里而必须随手可及（见下方 ★）。
    ///
    /// 装配出口会调 [`align_menu_columns`] 把**每一级**的标签左沿对齐：勾选列是
    /// 逐级判定的，纯动作列表（复制 / 插件）不补就会比别的子菜单少缩进一列。
    ///
    /// 「操作」子菜单由工具栏的 [`Btn::ALL`] **自动生成**（见 [`exec_btn`]），
    /// 底部按钮栏有什么、菜单里就有什么，加按钮不会只改一处。
    /// 也因此原来顶层的「全屏 / 棋盘格 / 水平翻转」不再单独占一行——它们在操作里。
    /// ★ 反过来也成立（2026-11-02）：**菜单里不许有浮条上没有的动作**。幻灯片放映
    ///   因为浮条放不下而只能当菜单项，所以它搬到顶层、不进「操作」——否则那条对账就破了，
    ///   用户看到的是"菜单里的操作和底部按钮对不上"。
    fn context_menu(&self, ctx: &mut EventCtx, at: Point) {
        let ws = window_state();
        let has_file = self.viewer.borrow().current_path().is_some();
        let has_image = self.viewer.borrow().image().is_some();

        let mut items = Vec::new();

        // ── 打开 ──
        items.push({
            let mut it = MenuItem::run(
                "打开…",
                {
                    let v = self.viewer.clone();
                    let cfg = self.config.clone();
                    move |ctx| {
                        let v2 = v.clone();
                        let cfg2 = cfg.clone();
                        // 过滤器用完整清单（内置 + 解码器自述 + 遗留）：装了新解码器，
                        // 打开对话框里就多出那些后缀。`filter` 内部立即把切片转成自有
                        // Vec，传这个局部量的引用是安全的。
                        let exts = crate::core::gallery::known_exts();
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

        // ── 复制（二级菜单，向右弹出）──
        // m248：紧跟「打开…」——两者都针对文件，文件类四项（打开 / 复制 / RAW 参数 /
        // 属性）在第一个分隔线之前连成一块。2026-11-03 用户口径「三个复制收纳到
        // "复制"里面，向右弹出，这样比较节省空间」：同一次只选一种口径（画面 PNG /
        // 文件本身 / 路径文本），平铺会摊成菜单最长的三段。快捷键仍挂在「复制当前」
        // 与「复制文件路径」上，收进子菜单不影响按键——显示的是同一份 hotkeys。
        items.push(MenuItem::submenu(
            "复制",
            self.copy_items(has_image, has_file),
        ));

        // ── 新建窗口：**不做**（2026-11-05 用户裁定）──
        //
        // 多开开启时这里曾多出一项「新建窗口」，但它的产物是一扇**没有图**的空窗：
        // 想再开一张图，直接双击/拖放本来就会新开一扇（`allow_multiple` 的语义），
        // 所以这一项是绕远路。托盘菜单里那一项保留——它在零窗口时是唯一入口。

        // 裸数据参数：**常驻**，只有当前文件确实是无头裸像素时才可点。
        // 相机 RAW（CR2/NEF…）有完整文件头，参数窗对它们没有意义，所以同样置灰；
        // 这样用户永远找得到入口，又不会在普通图片上误触。
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
            items.push(it);
        }

        // ── 属性…（m247）：从浮条「!」与「操作 ▸」收编到顶层 ──
        // 与「RAW 参数…」同组：都是"打开一个关于这张图的信息窗"。
        // 快捷键 Action::Properties 走 exec_action 那条独立路径，行为不变。
        items.push({
            let v = self.viewer.clone();
            let mut it = MenuItem::run("属性…", move |ctx| settings::open_metadata(ctx, &v), false);
            it.shortcut = self.config.borrow().hotkeys.display(Action::Properties);
            it
        });

        items.push(MenuItem::separator());

        // ── 操作：浮条按钮全集，菜单恒列；没有帧/页号时那三颗变灰 ──
        items.push(MenuItem::submenu("操作", self.ops_items(ws)));

        // ── 工具：宿主内置的低频辅助（m217）──
        // 夹在「操作」（恒列 `Btn::ALL`，与浮条按宽度/帧页号的裁剪不同）与「插件」之间。
        // 这组只进菜单不进浮条——用户口径「一排按钮已经很多了，太占空间」。
        items.push(MenuItem::submenu("工具", self.tool_items(has_image)));

        // ── 插件：只列已发现且启用的命令型插件；空目录给可读空态 ──
        items.push(MenuItem::submenu("插件", self.plugin_items()));

        items.push(MenuItem::separator());

        // ── 开关区（勾选态即时反映）──
        //
        // 「窗口置顶」入口已移到标题栏置顶钮（📌，读写同一 `ws.topmost`），
        // 菜单里不再重复。
        // ── 外观 ▸：改画面长什么样的开关收成一个子菜单（m246 改名）──
        items.push(MenuItem::submenu("外观", self.appearance_items()));

        // ── 幻灯片放映：改的是"怎么浏览这一组图"，不是窗口外观，所以留在顶层 ──
        //
        // 它不在 `Btn::ALL` 里（浮条上没有这一颗：MDL2 的 PUA 字形是赌局，而浮条几何由
        // `Btn::ALL` 驱动，多一颗要连动命中与窄窗排布），因此只能作为菜单项存在；
        // 行为仍走 `exec_action` 那唯一入口——键与菜单同一份代码，不存在"菜单里有、点了没反应"。
        // 名字随状态换（放映中读「停止放映」），双状态写法与 `Btn::PlayPause` 同一条规矩。
        items.push({
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

        // ── 左键拖图：指针交互的开关（ini 键：drag_image），留在顶层 ──
        //
        // 2026-11-03 用户点名进菜单（"和极简模式那些放一起"）。理由成立：这条改的是手感，
        // 而手感不对的时候人正盯着画面——不该要先开设置页才能试另一种分工。
        // 2026-11-04 收进子菜单、11-07 改名「外观 ▸」时，本项按手势分工 ≠ 外观留在顶层。
        // 勾选态**直接读 `config.drag_image`**，不像 `filmstrip` 那样另存一份会话位：
        // canvas 每次按下都现读配置，多存一份就是第二正本（还会漂）。
        // 不标脏：切换不改这一帧的任何像素，下一次按下自然按新分工走。
        items.push({
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

    /// 「外观 ▸」子菜单：只放**改变画面上出现什么 / 长什么样**的开关（m246）。
    ///
    /// 与设置 → 选项 → 外观卡同一条轴：极简模式（chrome 显隐）、棋盘格背景、
    /// 缩略图条（底部常驻区）、显示按钮（浮条按钮）、不透明度（整窗 alpha）。
    /// 全部读写同一份 `Config`，`runtime::apply_session_flags` 每帧把配置推给窗口信号。
    /// 棋盘格只在本菜单与设置页「外观」卡（m247 已从浮条 / 「操作 ▸」撤走）。
    /// 放映改的是浏览行为、左键拖图改的是手势分工，留在顶层不进来。
    /// 不透明度保留自己的档位子菜单：十档铺平会把这里撑成一长列，三级反而好找。
    fn appearance_items(&self) -> Vec<MenuItem> {
        let mut out = Vec::new();

        // ── 极简模式：当前窗口临时切换（不落盘）──
        out.push({
            let on = self.chrome.minimal_mode.get();
            let chrome = self.chrome.clone();
            let mut it = MenuItem::run(
                "极简模式",
                move |ctx| {
                    let on = !chrome.minimal_mode.get();
                    chrome.minimal_mode.set(on);
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
        // m247 起棋盘格只在这里和设置页「外观」卡；浮条与「操作 ▸」里那颗已撤走
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

        // ── 缩略图条：底部一排缩略图，点一格直接跳过去 ──
        //
        // 会话态（同棋盘格），m53 起落盘：开着关窗，下次启动照样开着。
        out.push({
            let on = self.chrome.filmstrip.get();
            let chrome = self.chrome.clone();
            let cfg = self.config.clone();
            let hub = self.hub.clone();
            let mut it = MenuItem::run(
                "缩略图条",
                move |ctx| {
                    let on = !chrome.filmstrip.get();
                    chrome.filmstrip.set(on);
                    persist_session(&cfg, &hub, |c| c.filmstrip = on);
                    // 不提示：条当场出现/消失，屏幕自己已经说了。
                    ctx.mark_dirty();
                },
                on,
            );
            it.checkable = true;
            it.checked = on;
            it.shortcut = self.config.borrow().hotkeys.display(Action::Filmstrip);
            it
        });

        // ── 显示按钮（m245，ini 键 show_toolbar_buttons）──
        // 按钮对应的操作在右键「操作 ▸」里一颗不缺，信息（页码 / 缩放 / 帧号）
        // 却只有浮条这一处载体 ⇒ 只隐按钮、留信息并水平居中。归「外观 ▸」：
        // 改的是画面上出现什么（m246 口径）。
        out.push({
            let on = self.config.borrow().show_toolbar_buttons;
            let cfg = self.config.clone();
            let hub = self.hub.clone();
            let mut it = MenuItem::run(
                "显示按钮",
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

        out.push(MenuItem::separator());

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
                        move |ctx| {
                            persist_session(&cfg, &hub, |c| c.ui_theme = t);
                            ctx.mark_dirty();
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

    /// 「操作」子菜单的项：由 [`Btn::ALL`] 生成，动作与浮条按钮同一份代码。
    /// 「复制 ▸」的三项：画面 PNG / 磁盘上的文件本身 / 路径文本。
    ///
    /// 从主菜单搬进子菜单只是**层级**变化（2026-11-03 用户要省空间），行为一字未改：
    /// 灰项判据（`has_image` / `has_file`）、回执 toast、以及"占用剪贴板时报错而不是
    /// 无条件说已复制"那条都原样跟着走。
    /// 「工具」子菜单（m217）：宿主内置的低频辅助，只进菜单不进浮条。
    ///
    /// 取色器是**会话临时态**（同极简模式，不落盘）：开关与指针位置都在 `Chrome`，
    /// 菜单回调、画布事件、paint 三者读同一份；切换时清掉旧指针位置，免得下次
    /// 一开模式就有一枚停在别处的浮层。没有图时置灰——空窗没有像素可采样。
    fn tool_items(&self, has_image: bool) -> Vec<MenuItem> {
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
        vec![it]
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
                    move |ctx| {
                        exec_btn(ctx, &v, &chrome, b);
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
        // ★「幻灯片放映」**不在这里**（2026-11-02 用户要求）：它留在顶层，要随手可及。
        //   菜单恒列 `Btn::ALL` 全量；浮条另按 `Chrome::row` 裁剪，两者允许不一致——
        //   差集只是"变灰"，不是"消失"。
        out
    }
}

/// 「操作 ▸」的项序与启用态：菜单**恒列** [`Btn::ALL`]，不随窗口宽或播放组显隐删项；
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
/// 才让出 `MENU_CHECK_W + MENU_GAP`（24dp），没有就贴左。于是「操作」「工具」「窗口」
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

/// 执行一次工具栏按钮的动作：**浮条点击与右键菜单「操作」子菜单共用的唯一入口**。
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
            ctx.toggle_topmost();
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
        // 缩略图条占掉底部一条（它是常驻的地，不是浮层）：摆图、悬浮层、命中测试
        // 一律用扣掉之后的 `work`——**画在哪就要点在哪**，三者共用一份几何。
        let strip_on = self.strip_on() && filmstrip::fits(content.h as f32);
        let work = if strip_on {
            Rect::new(
                content.x,
                content.y,
                content.w,
                (content.h as f32 - filmstrip::STRIP_H).max(1.0) as i32,
            )
        } else {
            content
        };
        self.last_work.set(work);
        let (vx, vy, vw, vh) = (work.x as f32, work.y as f32, work.w as f32, work.h as f32);
        let (checker_a, checker_b) = (tk::checker_a(), tk::checker_b());
        let checker_on = self.chrome.checker.get();

        // ① 底：棋盘格（透明背景开关）或近黑空场。
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

        // ② 图 + 空场提示 / 错误。
        // 实时调色只对声明的那张图生效：换图后 `path` 对不上，效果自动失效，
        // 不用定时器清，也不会串到另一扇窗（每窗一份 `LiveState`）。
        let live = {
            let v = self.viewer.borrow();
            self.live.tint.get().filter(|t| {
                !t.is_neutral() && v.current_path().as_deref() == Some(t.path.as_path())
            })
        };
        let hint_ts = TextStyle::new(14.0);
        let mut nav: Option<(Rect, Rect)> = None;
        {
            let mut v = self.viewer.borrow_mut();
            v.on_viewport(vx, vy, vw, vh, canvas.dpi_scale());
            // ★ 画的是**位图**的目标矩形（SVG 切片 ≠ 整图 dst），而鸟瞰几何仍用整图 dst。
            let base_dst = v.dst_rect(from_rect(work));
            let img_src = v.image();
            let img_dst = v.image_dst(from_rect(work));
            let error = v.error_text();
            if let (Some(src), Some(idst)) = (img_src, img_dst) {
                // 目标矩形已经是我们算好的等比尺寸，故用 `Fill` 直铺即可（不会拉伸变形）。
                // gen 代号没变就不重新上传纹理（未变换的翻页/缩放零成本命中缓存）。
                let mut cache = self.main_img.borrow_mut();
                let mut tinted = self.tint_main.borrow_mut();
                let shown = if let Some(t) = &live {
                    tinted.obtain(src, t.values, t.gamma)
                } else {
                    cache.obtain(src)
                };
                if let Some(img) = shown {
                    canvas.draw_image(img, to_rect(idst), Fit::Fill, 0.0, 1.0);
                } else if src.width > MAX_TEXTURE_SIDE || src.height > MAX_TEXTURE_SIDE {
                    // ★ 显卡放不下这张图 ⇒ **给一句人话**，而不是留一片空白。
                    //   （正常路径上显示位图已由 `core::derive` 降过边，这里是兜底。）
                    let msg = format!(
                        "这张图太大，显卡放不下（单边上限 {MAX_TEXTURE_SIDE} 像素，实际 {}×{}）",
                        src.width, src.height
                    );
                    let z = chrome::ARROW_ZONE as i32;
                    let slot = Rect::new(work.x + z, work.y, (work.w - 2 * z).max(60), work.h);
                    canvas.draw_text(&msg, slot, tk::error_text(), Align::Center, &hint_ts);
                }
                drop(tinted);
                drop(cache);
                // 鸟瞰缩略图（菠萝 NavigatorView）：显示尺寸超出视口时出现。
                // 几何锚在**整图** dst 上（位图是切片时映射关系也不变，见 viewer 的
                // 归一化坐标设计）。位图存在则整图 dst 必然存在。
                if let Some(bd) = base_dst {
                    nav = self.nav_geom(work, to_rect(bd));
                }
            } else if let Some(e) = error {
                // 中央这一句现在是错误的**唯一**载体（状态格里不再重复一遍），所以两头
                // 必须让开翻页箭头：504 宽实测文案两头各骑在一个箭头上，读不全。
                // 让宽之后 DirectWrite 自动折行（软硬两后端都按 rect.w 排版、纵向仍居中），
                // 长错误变成两行而不是一行两头截断。
                let z = chrome::ARROW_ZONE as i32;
                let slot = Rect::new(work.x + z, work.y, (work.w - 2 * z).max(60), work.h);
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
                canvas.draw_text(hint, work, color, Align::Center, &hint_ts);
            }
        }

        // ③ 鸟瞰层：右下角缩略图 + 当前视口框。全程直角（用户拍板 2026-10-10）。
        if let Some((box_r, thumb)) = nav {
            // 盒底。
            canvas.fill_rect(
                box_r.x as f32,
                box_r.y as f32,
                box_r.w as f32,
                box_r.h as f32,
                &Paint::fill(tk::pill_bg()),
            );
            // 盒框双层高对比描边（与视口框同款，2026-10-12 用户拍板）：
            // 内白 1px + 外黑 1px——鸟瞰面板压在任意底色的图片上都能看清边界。
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
            // 缩略图：**鸟瞰永远显示完整图片**（菠萝 NavigatorView 语义）。画的
            // 是 viewer 那份 ≤512 的整图缩略（`nav_image_dst`），不是主位图——
            // 主位图在 ≥100% 时是原图档位，拿它当鸟瞰要为一整张 6055×6055 再传
            // 一份预乘纹理（实测 139.9 MB、0.09–0.14 s，M9 查账）。仍需缩放映射进
            // thumb（idst 与 thumb 两套坐标，直接用会画在原位）。
            let v = self.viewer.borrow();
            let nav = v.nav_image_dst(from_rect(work));
            let box_dst = v.dst_rect(from_rect(work));
            // 盒的最外 1 px 是内白描边的地，缩略与视口框都在它**以内**——四条边一律如此。
            // ★ m103：以前图画到 box 上，左右上下不对称（截断让右/下各少 1 px，于是
            // 那两条边露出描边/盒底）。极端长宽比的条图盒子短边只有 1–2 px，抠不出
            // 这一圈，就整盒画（宁可不描边，也不要 0 宽的图）。
            let inner = if thumb.w >= 3 && thumb.h >= 3 {
                Rect::new(thumb.x + 1, thumb.y + 1, thumb.w - 2, thumb.h - 2)
            } else {
                thumb
            };
            if let (Some((src, idst)), Some(dst)) = (nav, box_dst) {
                let (idst, dst) = (to_rect(idst), to_rect(dst));
                let draw = nav_draw_rect(inner, idst, dst);
                let mut cache = self.nav_img.borrow_mut();
                let mut tinted = self.tint_nav.borrow_mut();
                let shown = if let Some(t) = &live {
                    tinted.obtain(src, t.values, t.gamma)
                } else {
                    cache.obtain(src)
                };
                if let Some(img) = shown {
                    canvas.draw_image(img, draw, Fit::Fill, 0.0, 1.0);
                }
            }
            // 视口框：content 在整图 dst 中的归一化范围映射到缩略所在的 `inner`
            // （与图对齐，不是与盒对齐——差的那 1 px 描边不该算进视角位置）。直角描边。
            if let Some(dst) = box_dst {
                let dst = to_rect(dst);
                let u0 = ((work.x as f32 - dst.x as f32) / dst.w as f32).clamp(0.0, 1.0);
                let u1 =
                    ((work.x as f32 + work.w as f32 - dst.x as f32) / dst.w as f32).clamp(0.0, 1.0);
                let v0 = ((work.y as f32 - dst.y as f32) / dst.h as f32).clamp(0.0, 1.0);
                let v1 =
                    ((work.y as f32 + work.h as f32 - dst.y as f32) / dst.h as f32).clamp(0.0, 1.0);
                let fx = inner.x as f32 + u0 * inner.w as f32;
                let fy = inner.y as f32 + v0 * inner.h as f32;
                let fw = ((u1 - u0) * inner.w as f32).max(4.0);
                let fh = ((v1 - v0) * inner.h as f32).max(4.0);
                canvas.fill_rect(fx, fy, fw, fh, &Paint::fill(tk::NAV_FILL));
                // 视口框双层高对比描边（用户拍板 2026-10-12）：内白 1px + 外黑 1px
                // ——黑图片上白线可见、白图片上黑线可见，单色框在极端底色下会隐身。
                // 直角描边 = 圆角半径 0（windui 只有圆角描边原语）。
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
        }

        // ④ 悬浮层（箭头 + 工具栏 + 信息条）。三者同一条门控：极简模式下都靠悬停浮现
        //    （箭头看 `Chrome::hover_edge` 那两位，见 `chrome::paint`），传统模式常驻。
        let v = self.viewer.borrow();
        let info = v.status();
        let has_prev = v.has_prev();
        let has_next = v.has_next();
        let title = v.title();
        // 动画组该不该出现、暂停钮画 ▶ 还是 ⏸——都从 viewer 现算，不另存一份真相。
        // 这里只回灌不标脏：状态一变（首帧落地 / 按 P）那条路径自己会标，本轮绘制
        // 就在标脏之后发生，再标一次是空转。
        //
        // ★ `chrome.anim` 的语义是"**有没有帧/页号**"（= `has_index()`），不是"是不是
        //   动图"（= `is_animated()`）。静态位图没有号 ⇒ 播放组整组不出现；只有真动图
        //   与多页文档才挂。字形仍按真实播放态走：没在播 → ▶，动图 → ⏸/▶ 按 `anim_paused`。
        let has_index = v.has_index();
        let anim = v.is_animated();
        let paused = if anim { v.anim_paused() } else { true };
        drop(v);
        self.chrome.anim.set(has_index);
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

        // ⑤ 底部缩略图条（常驻的一块地，已在前面从摆图区里扣掉）。
        *self.strip_items.borrow_mut() = if strip_on {
            // 先向状态机要"这一屏该显示哪几张"（顺带为缺图的发起后台解码），
            // 再把格子摆开。m100：路径全程按引用走，一帧一个 `PathBuf` 都不克隆。
            let cap = filmstrip::capacity(vw);
            let (first, n) = self.viewer.borrow_mut().strip_window(cap);
            // gallery_pos() 返回 1-based，filmstrip 内部用 0-based 索引比较，需减 1
            let cur = self.viewer.borrow().gallery_pos().saturating_sub(1);
            // 条画在**完整视口**的底部（扣掉的那条正是它占的地，用 work 就叠回画面上了）。
            let rects = filmstrip::layout((vx, vy, vw, content.h as f32), n);
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
            self.strip.paint(
                canvas,
                (vx, vy, vw, content.h as f32),
                &slots,
                cur,
                self.strip_hover.get(),
                |p| v.thumb(p),
            );
            slots.iter().map(|s| (s.idx, s.rect)).collect()
        } else {
            Vec::new()
        };

        // ⑥ 取色器浮层（m217）：最后画，压在包括鸟瞰/缩略图条在内的所有图层之上。
        // 判据与点击复制共用 `Viewer::pick_at` —— 屏上显示哪一格，就复制哪一格。
        if let (true, Some(pos)) = (self.chrome.picker.get(), self.chrome.picker_at.get()) {
            if let Some(pk) = self.viewer.borrow().pick_at(from_rect(work), pos) {
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

    /// 左键按下的归属分派（Down 分支只调这一句）。
    ///
    /// ★ 抽成方法不是为了好看，是为了**把接线钉住**：[`left_drag`] 是纯函数，它绿了
    /// 不代表调用点真在用它（用例见 `left_press_wires_the_switch_to_the_pan_state`）。
    /// `ctrl` = 按住 Ctrl：那是 2026-10-08 起的平移兜底手势，**不看开关**一律平移。
    ///
    /// 先算成局部变量再分支：`self.viewer.borrow()` 的 `Ref` 若活进分支里，
    /// 那句 `borrow_mut()` 就是 `already borrowed` panic（§七 的禁写形状）。
    fn press_left(&self, pos: (f32, f32), view: (f32, f32, f32, f32), ctrl: bool) {
        let mode = left_drag(
            self.config.borrow().drag_image,
            self.viewer.borrow().can_pan(),
            Chrome::in_title_strip(pos, view),
            window_state().fullscreen,
        );
        if ctrl || mode == LeftDrag::PanImage {
            self.viewer.borrow_mut().begin_drag(pos);
        } else if mode == LeftDrag::Window {
            // 只有这一支会记待定拖窗位；全屏的 `None` 什么都不记，Move 里
            // 也就不会有那一记 `start_window_drag()`。
            self.lmb_pending.set(Some(pos));
        }
    }

    /// 按当前指针位置与拖动状态换光标：拖动中=抓手、可拖区悬停=松手、其余=箭头。
    ///
    /// ★ 判据与 [`left_drag`] **同源**：光标说"这一拖归图片"，按下去就一定是拖图。
    /// 两边各写一套的话会出现"光标是手、按下却把窗口拖走了"。
    ///
    /// 浮条/左右箭头/标题钮上不接手——那里点的是控件；标题栏那一条也留箭头，
    /// 因为那里永远拖窗口（[`left_drag`] 的第三项）。
    fn sync_hand(&self, pos: (f32, f32)) {
        // 取色模式优先（m217）：十字光标，且不许被下面的抓手/箭头覆盖。
        // 光标说"这一拖归图片"、按下去却是取样，是最刺眼的语义错位。
        if self.chrome.picker.get() {
            crate::platform::cursor::use_cross();
            return;
        }
        // 拖动中不看位置：指针可能已经拖出画布，手型要一直握着。
        if self.viewer.borrow().dragging() {
            super::hand_cursor::apply(Some(true));
            return;
        }
        if self.chrome_hit(pos).is_some() {
            super::hand_cursor::apply(None);
            return;
        }
        let in_title = Chrome::in_title_strip(pos, self.work_tuple());
        let can_pan = self.viewer.borrow().can_pan();
        let mode = left_drag(
            self.config.borrow().drag_image,
            can_pan,
            in_title,
            window_state().fullscreen,
        );
        super::hand_cursor::apply(match mode {
            LeftDrag::PanImage => Some(false),
            LeftDrag::Window | LeftDrag::None => None,
        });
    }

    fn on_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> bool {
        let pos = (p.pos.x as f32, p.pos.y as f32);
        match p.kind {
            PointerKind::Wheel(delta) => {
                // ★ 放映让位挂在**滚轮事件本身**上，不挂在翻页节奏闸上：默认档里滚轮是缩放
                // （`config.wheel == Zoom`），那条路根本不进 `wheel_gate`——2026-10-31 用户实测
                // "滚轮没用、方向键有用"就是这个错位。缩放同样是"用户插手了画面"。
                self.chrome.slide_stop();
                if self.chrome.locked.get() {
                    return true; // 锁定：吞掉滚轮，不缩放不翻页
                }
                // 滚轮行为（设置项，菠萝 MouseWheelBehavior）：缩放或翻页。
                // Ctrl+滚轮走独立开关（config.ctrl_wheel），不按 Ctrl 时走 config.wheel——
                // 两条各管各的，用户想"裸滚轮翻页 + Ctrl 缩放"或反之都能配。
                let zoom = if p.mods.ctrl {
                    self.config.borrow().ctrl_wheel == crate::core::config::CtrlWheel::Zoom
                } else {
                    self.config.borrow().wheel == crate::core::config::Wheel::Zoom
                };
                if zoom {
                    let c = self.center();
                    self.viewer.borrow_mut().zoom_by_wheel(delta, pos, c);
                } else {
                    // 每格（±120）翻一张；碎增量累计后按符号翻（判据见 [`wheel_detent`]）。
                    let (acc, dir) = wheel_detent(self.wheel_acc.get(), delta);
                    self.wheel_acc.set(acc);
                    if dir != 0 {
                        // 再过一道节奏闸（A3 / m64）：`WHEEL_PACE` 内最多翻一张，压在闸里的
                        // 那一下由 runtime 的 16 ms 轮询兜出去（`Chrome::tick_wheel_pace`）——
                        // **挂起不是丢弃**，丢弃会让"快滚两格只翻一张"变成常态手感。
                        if self.chrome.wheel_gate(dir) {
                            self.viewer.borrow_mut().step(dir);
                        }
                    }
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
                // 缩略图条悬停高亮：只在这一格的归属变了才重绘。
                // 坐标已统一：filmstrip::layout 和 paint 坝均使用 work rect 坐标系
                // 直接使用 pos 进行命中测试，无需额外补偿
                let sh = filmstrip::hit(&self.strip_items.borrow(), pos);
                if self.strip_hover.replace(sh) != sh {
                    ctx.mark_dirty();
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
                let was_pending = self.lmb_pending.replace(None).is_some();
                let was_drag = self.viewer.borrow().dragging();
                if was_drag {
                    self.viewer.borrow_mut().end_drag();
                }
                let left = self.chrome.pointer_left();
                let strip = self.strip_hover.replace(None).is_some();
                if left || strip || was_nav || was_strip || was_pending || was_drag {
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
                        .pick_at(from_rect(self.last_work.get()), pos);
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
                // 缩略图条：按下记格子，抬起时仍在同一格才跳。
                //
                // 排在双击关窗**之前**是刻意的：条是常驻的一块地，落在它上面的一击
                // 属于"选图"，不该被双击关窗吃掉（也没道理去拖窗口）。
                // 坐标已统一：filmstrip::layout 和 paint 均使用 work rect 坐标系
                // 直接使用 pos 进行命中测试，无需额外补偿
                if !self.chrome.locked.get() {
                    if let Some(i) = filmstrip::hit(&self.strip_items.borrow(), pos) {
                        self.strip_pressed.set(Some(i));
                        ctx.mark_dirty();
                        return true;
                    }
                }
                // 双击：按设置对话框选的行为。菠萝默认「关窗」，与我们的默认配置一致。
                if p.click_count >= 2 {
                    if self.chrome.locked.get() {
                        return true; // 锁定：双击也不许关窗/最大化/全屏
                    }
                    // 先拷出枚举、让 `Ref` 在这一行就结束：下面三个 `ctx.*` 都会同步动窗口
                    // （关闭 / 最大化 / 全屏），那条路上框架可能回头再借一次 config。
                    let dbl = self.config.borrow().dbl_click;
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
            viewer.borrow_mut().step(-1);
            true
        }
        Action::Next => {
            viewer.borrow_mut().step(1);
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
            viewer.borrow_mut().step_anim(-1);
            true
        }
        Action::FrameNext => {
            viewer.borrow_mut().step_anim(1);
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
        // 取色器（m217）：与右键「工具 ▸ 取色器」共用一个 toggle，不另写一份状态。
        Action::Picker => {
            chrome.toggle_picker();
            if let Some(c) = ctx {
                c.mark_dirty();
            }
            true
        }
        Action::Minimal => {
            let on = !chrome.minimal_mode.get();
            chrome.minimal_mode.set(on);
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
        Action::Topmost => ctx.is_some_and(|c| {
            c.toggle_topmost();
            true
        }),
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
            let exts = crate::core::gallery::known_exts();
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

    /// 显示层调色只在上屏位图 gen / 六个参数 / Gamma 变化时才重算一次，不每帧重扫整图。
    #[test]
    fn tint_cache_only_recomputes_on_a_real_change() {
        use crate::core::pixels::GAMMA_NEUTRAL as G;
        let src = RgbaImage::new(2, 1, vec![0, 10, 200, 255, 100, 50, 20, 128]);
        let v: [i32; 6] = [-100, 100, 100, 255, 0, 0];
        let mut cache = TintCache::new();
        assert!(cache.obtain(&src, v, G).is_some());
        assert_eq!(cache.key, Some((src.gen, v, G)));
        let first = cache.img.as_ref().unwrap();
        assert_eq!(
            &first.rgba[..],
            &[255, 10, 200, 255, 155, 50, 20, 128],
            "只反相红通道，alpha 与 G/B 原样"
        );
        let gen = first.gen;

        // 同 gen + 同参数 + 同 Gamma：一次都不该重算。
        assert!(cache.obtain(&src, v, G).is_some());
        assert_eq!(cache.img.as_ref().unwrap().gen, gen);

        // 参数变了：重算，且新像素按新参数来。
        let v2: [i32; 6] = [100, 100, 100, 0, 0, 0];
        assert!(cache.obtain(&src, v2, G).is_some());
        assert_eq!(cache.key, Some((src.gen, v2, G)));
        assert_ne!(cache.img.as_ref().unwrap().gen, gen);
        assert_eq!(&cache.img.as_ref().unwrap().rgba[..], &src.rgba[..]);

        // ★ 只动 Gamma 也必须重算，且暗部按曲线抬起来（第七格不能漏进缓存键）。
        let gen_g = cache.img.as_ref().unwrap().gen;
        assert!(cache.obtain(&src, v2, 180).is_some());
        assert_eq!(cache.key, Some((src.gen, v2, 180)));
        assert_ne!(cache.img.as_ref().unwrap().gen, gen_g);
        assert_eq!(&cache.img.as_ref().unwrap().rgba[..4], &[0, 42, 223, 255]);

        // 源位图换代（翻页/缩放）：同样必须重算。
        let next = RgbaImage::new(2, 1, vec![7, 8, 9, 255, 10, 11, 12, 255]);
        let gen2 = cache.img.as_ref().unwrap().gen;
        assert!(cache.obtain(&next, v2, 180).is_some());
        assert_eq!(cache.key, Some((next.gen, v2, 180)));
        assert_ne!(cache.img.as_ref().unwrap().gen, gen2);
    }

    /// ★ 用户 2026-11-06：右键「操作 ▸」的播放三颗**永远存在**；没有帧/页号时
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
            left_drag(true, true, false, true),
            PanImage,
            "画布里仍可平移图"
        );
        assert_eq!(
            left_drag(true, true, true, true),
            NoDrag,
            "顶条那一条不再拖窗口"
        );
        assert_eq!(
            left_drag(false, true, false, true),
            NoDrag,
            "开关关掉也不拖窗口"
        );
        assert_eq!(
            left_drag(true, false, false, true),
            NoDrag,
            "空窗也不拖窗口"
        );
        assert_eq!(left_drag(false, true, true, true), NoDrag);
        // 非全屏回归：顶条永远带走窗口，别把旧行为改坏。
        assert_eq!(left_drag(true, true, true, false), Window);
        assert_eq!(left_drag(false, false, false, false), Window);
    }

    /// 左键这一拖归谁：`drag_image` 开关 × 有没有可平移余量 × 是不是标题栏，**八格全钉**。
    ///
    /// 只有一格（开关开 + 拖得动 + 画布内）是本次新增的行为，其余七格一律回到拖窗口——
    /// 用户要的是"能只拖图片"，不是"把拖窗口这条路堵死"：标题栏那一条永远带走窗口，
    /// 拖不动东西（只有"还没有图"这一种）时也留着拖窗口。
    /// ★ "拖得动"到 2026-11-03 第四条口径时已经放宽到**几乎总是真**，判据本体见
    /// `Viewer::pan_limits` → `overscroll`（并集的一半减一枚衣角）。
    #[test]
    fn left_drag_yields_the_window_unless_there_is_room_in_the_canvas() {
        use LeftDrag::{PanImage, Window};
        assert_eq!(
            left_drag(true, true, false, false),
            PanImage,
            "本次功能的全部新行为"
        );
        assert_eq!(
            left_drag(true, true, true, false),
            Window,
            "标题栏永远拖窗口"
        );
        assert_eq!(
            left_drag(true, false, false, false),
            Window,
            "空窗（没有图）时别堵死拖窗口"
        );
        assert_eq!(left_drag(true, false, true, false), Window);
        assert_eq!(
            left_drag(false, true, false, false),
            Window,
            "开关关掉就是 2026-10-08 那套分工"
        );
        assert_eq!(left_drag(false, true, true, false), Window);
        assert_eq!(left_drag(false, false, false, false), Window);
        assert_eq!(left_drag(false, false, true, false), Window);
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

    /// 鸟瞰盒必须在**右下角**，且始终整个落在视口内。
    ///
    /// 这是"抽屉不会跑出窗口"的自检：右下角 + 抬起来躲工具栏两段位移叠加后，
    /// 很容易把盒子顶到窗口外面去（曾经右上角版本就不用做这个算术）。
    #[test]
    fn nav_box_sits_inside_the_viewport() {
        for (w, h) in [(360, 260), (520, 300), (1040, 700), (1920, 1080)] {
            let content = Rect::new(0, 0, w, h);
            let b = ImageCanvas::nav_box(content);
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
                .nav_geom(content, dst)
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
                .nav_geom(content, dst)
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
                let Some((_, box_r)) = canvas.nav_geom(content, dst) else {
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
        let (_box, thumb) = canvas.nav_geom(content, dst).unwrap();
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
        let (_box, thumb) = canvas.nav_geom(content, dst).unwrap();
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
            .nav_geom(content, to_rect(dst))
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
            let b = ImageCanvas::nav_box(content);
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
                "version":"1.0.0","protocol":1,"min_host":"0.1.0",
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

    /// 缩略图格里的任何一点都不许被当成工具栏按钮——条开着时命中区下沉 96px
    /// 正好压进格子里（报障：点缩略图反而触发工具栏按钮，chrome 命中抢先于条）。
    #[test]
    fn pressing_a_filmstrip_item_never_hits_a_toolbar_button() {
        let canvas = strip_canvas();
        // `paint` 把格子摆在「完整视口」底部：与它同一份 view。
        let items = filmstrip::layout((0.0, 0.0, 800.0, 600.0), filmstrip::capacity(800.0));
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
    fn tool_submenu_exposes_picker_as_a_checkable_shortcut_item() {
        let (canvas, _) = zoomed_canvas();
        let items = canvas.tool_items(true);
        assert_eq!(items.len(), 1, "工具组目前只有取色器一项");
        let it = &items[0];
        assert_eq!(it.label, "取色器");
        assert!(it.checkable, "菜单里要能看出开没开");
        assert!(!it.checked, "默认关");
        assert!(it.enabled, "有图时可点");
        let sc = it.shortcut.as_deref().expect("菜单项要显示 K 快捷键");
        assert!(
            sc.to_uppercase().contains('K'),
            "快捷键提示该含 K，实得 {sc}"
        );
        assert!(
            !canvas.tool_items(false)[0].enabled,
            "空窗没有像素可采样，取色器必须置灰"
        );
    }
}
