//! 悬浮交互层（菠萝看图式）：左右翻页箭头 + 底部工具栏 + 显隐状态。
//!
//! 这层**全部画在 `ImageCanvas` 里**而不是做成 `Element` 子树，理由有二：
//!
//! 1. **事件连续性**。"悬停显示、静止 2 秒隐藏"需要画布拿到指针的每一次移动。
//!    若工具栏是画布之上的兄弟 `Element`，指针移到它上面时画布就收不到 Move，
//!    显隐计时器会把浮条从光标底下抽走——再触发 Enter 又弹回来，来回闪烁。
//!    一个控件吃下整窗事件后，显隐与悬停高亮都在同一份坐标里算，天然不闪。
//! 2. **布局确定**。浮条尺寸固定、贴视口边缘摆，不需要 measure/arrange 参与判定。
//!
//! 顶部标题区**也是画出来的**：指针落在顶条命中区按下时由画布直接发起窗口拖动
//! （`start_window_drag`，见 `ui::canvas`），最小化 / 最大化 / 关闭同理是画布命中区
//! （[`TitleBtn`]）。整窗因此只有一个吃事件的控件，显隐与悬停高亮都在同一份坐标里算；
//! [`FRAME`] 的贴边豁免只为"指针停在窗口边缘时浮层不该消失"这一件事服务。
//!
//! # 显隐规则
//!
//! - 画布收到 Enter/Move → 刷新 [`Chrome::last_move`]，隐藏时立即显示；
//! - 距上次移动超过 2 秒，**且**最后指针位置不在 [`FRAME`] 贴边区 → 隐藏；
//! - 贴边区豁免的副作用是"鼠标停在窗口边缘时浮层不消失"，这恰好也是友好的。
//! - 极简模式下归这条管的有四样：顶部标题栏、底部浮条、信息条、**左右翻页箭头**。
//!   箭头用的是另一对位置位 [`Chrome::hover_edge`]（该侧 [`ARROW_ZONE`] 边带内就浮现），
//!   而且**只隐身不缩热区**——`hit` 的圆照旧，鼠标用户靠"靠边就出现"找到它。
//!
//! ★ "立即"两个字是靠**标脏**兑现的，不是靠规则本身：[`Chrome::activity`] 与
//! [`Chrome::pointer_left`] 都返回"显隐位翻没翻、要不要重绘"，画布必须接着这个返回值
//! `mark_dirty`。当年那句"隐藏时立即显示"写在这里而返回值没人接，于是浮现和收走都要
//! 等下一个别的重绘源，真机上就是用户 2026-10-19 报的"不流畅、移开有时不隐藏"。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use windui::geometry::Rect;
use windui::render::image::{Fit, Image};
use windui::render::{Canvas, Paint};
use windui::signal::{signal, Signal};
use windui::spec::Align;
use windui::text::TextStyle;

use crate::core::hotkey::Hotkeys;
// 配色一律取自全 ui 共用的令牌（[`crate::ui::tokens`]）：本文件不自带色值。
// 用模块别名而不是逐个 import：跟随主题那一组是**函数**（运行期切主题），
// 名字又短又常见（`text` / `empty_bg`），逐个引入容易和局部变量撞名。
use super::tokens as tk;

/// 顶部拖动条高度（逻辑 dp）：画布据此判定"指针落在顶条里"以发起窗口拖动，
/// 也与画出来的那条标题区同高（见 `ui::canvas` 的 `start_window_drag` 分支）。
pub const STRIP_H: i32 = 40;

/// 自动隐藏前允许的静止时长。
const IDLE_HIDE: Duration = Duration::from_secs(2);

/// 裸滚轮翻页的最小间隔（A3 / m64）。
///
/// 为什么取 100 ms 这个**下界**（用户 2026-10-20 的原话："100ms 或者 200ms 之类的就够了，
/// 不能太长"）：故意转滚轮最快也就每秒 8–10 格，100 ms 正好让这条闸**不吃故意的格**，
/// 只压得住触控板一次惯性滑行（事件间隔常在 8–30 ms）。再长就要开始丢用户的真格子了。
pub const WHEEL_PACE: Duration = Duration::from_millis(100);

/// 键盘翻页的最小间隔（m81）。
///
/// 为什么取 350 ms：用户 2026-10-21 报的原话是「按住翻页快捷键，能够比较慢速的翻页，
/// 例如一秒两三张」——2.5 张/秒是 400 ms、3 张/秒是 333 ms，取中间值 350 ms（≈2.9 张/秒）。
///
/// 为什么判据是"同方向两键的间隔"而不是"这一下是不是自重复"：框架层只给 Alt 上送抬起
/// 事件（windui 平台层的 `handle_key_up` 对别的键直接返 `false` 交默认处理），普通键的
/// `WM_KEYUP` 与 `WM_KEYDOWN` 的 lParam"之前已按下"位都在边界上被丢掉，宿主根本拿不到
/// "这把键现在按着没有"。间隔是这条路上唯一量得到的读数，而它的表现正好够用：
/// 键盘自重复是**几十毫秒**一记（必然落进闸里），人手一记一记按是**每秒 2.9 次以下**
/// （天然不满足，一律当场翻）——要压的与要放的两边不重叠。
pub const KEY_PACE: Duration = Duration::from_millis(350);

/// 「复制当前（PNG）」**当场能编**的上限（像素数，m74）。
///
/// 分界线只看实测的"界面停多久"：`Compression::Fastest` 那一档编一张大约
/// **17 ns/像素**（`exif_padded_20M.jpg` 原图档 1800×1200 → 36 ms，
/// `tiff_100M.tif` 原图档 6055×6055 → 612 ms；复现见 `examples/copy_png_cost.rs`）。
/// 6 M 像素约等于一次 100 ms 的停顿——再大就该让后台线程去跑，而不是让窗口被系统标成"未响应"。
/// 阈值以下是有意**保持同步**的：一按就有回执、天然按顺序，不必多一条提示与竞态。
pub const COPY_SYNC_PIXELS: u64 = 6_000_000;

/// 「复制当前（PNG）」该当场编还是交给线程（判据与执行分开写，见 [`copy_route`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyRoute {
    /// 屏上这份不大：当场编完，一按就有回执。
    Sync,
    /// 超过上限：编码要停在 UI 线程上几十毫秒以上，交给一次性线程。
    Threaded,
}

/// 判据单独成函数，是因为它得能在**无头单测**里被钉住：一旦走到执行，就会真去写
/// 系统剪贴板——那是用户机器上的公共状态，不该由测试线程去占（更糟的是异步那一枚
/// 会在测试结束后才落地，把剪贴板留在谁也说不清的内容上）。
#[must_use]
pub const fn copy_route(pixels: u64) -> CopyRoute {
    if pixels > COPY_SYNC_PIXELS {
        CopyRoute::Threaded
    } else {
        CopyRoute::Sync
    }
}

/// 显隐判定时豁免的贴边区宽度（逻辑 dp）。
///
/// 覆盖三处：顶部拖动条（40dp）+ 余量；底部工具栏带；左右箭头带。
/// 指针停在这圈边框里，浮层保持可见——工具栏和箭头都在边框里，
/// 不会出现"悬停在按钮上正要点击，浮层却按计时器消失了"。
const FRAME: f32 = 56.0;

/// 悬停记录"过期"的漂移阈值（**物理屏幕像素**，曼哈顿距离）。
///
/// 判据来自 [`crate::platform::cursor`]：上一次指针事件发生的那一刻记下真实光标位置，
/// 定时器里再读一次——两个读数对不上就说明那段 `last_pos` 已经不是光标现在的位置了
/// （`WM_MOUSELEAVE` 的一次性旗被交互式拖动/缩放吞掉之后，本会话再也收不到"离开"通知）。
/// 阈值不为 0 是因为真光标在"静止"时也可能被系统抖出 1–2 px；取 8 是它的整数倍留量，
/// 而窗内任何真实移动都会在下一枚 `Move` 里把锚点刷新，误清只活一帧。
const STALE_DRIFT_PX: i32 = 8;

/// 放映进度推到哪了——**整颗底的擦除宽度**（自左向右，`0..=ARROW_R*2`）。
///
/// 判据抽成纯函数的理由：这格的形状是"底色一点点被推满、推满就翻页"，而不是里面加一根条
/// （用户 2026-10-31 驳回了我第一版：'我说的是整个按钮的底色变化'）。抽出来才谈得上被测：
/// `0 格 = 完全没推、满格 = 整个圆盘`，中间线性。
fn wipe_width(step: u32) -> f32 {
    (step.min(SLIDE_STEPS) as f32 / SLIDE_STEPS as f32) * (ARROW_R * 2.0)
}

/// 工具栏图标的字体。Win10/11 系统自带的图标字形字体（PUA 码位），
/// 与菠萝看图的工具栏一样用"纯图标 + 悬停提示"的极简形式。
const ICON_FONT: &str = "Segoe MDL2 Assets";

/// 工具栏上的一个按钮。
///
/// ★ 这一份枚举是**浮条与右键菜单「操作」子菜单的共同真源**：菜单项由
/// **底部浮条的按钮显不显示**（m245，ini 键 `show_toolbar_buttons` 的运行时镜像）。
///
/// ★ 唯一写点 = [`set_toolbar_buttons`]，由 `runtime::apply_session_flags` 从 `Config`
///   回灌；`Config` 才是正本（菜单与设置页都先写配置）。放在模块级而不是 `Chrome` 字段：
///   这是**全局**开关，而 [`Chrome::row`] / [`Chrome::pill`] / [`Chrome::btn_rect`] 都是
///   关联函数（命中、绘制、鸟瞰避让共 30 个调用点）——一处读，几何与命中天然同源，
///   不会出现「画没了但还点得着」。
static TOOLBAR_BUTTONS: AtomicBool = AtomicBool::new(true);

/// 写镜像，返回「值变没变」（调用方据此标脏）。
pub fn set_toolbar_buttons(on: bool) -> bool {
    TOOLBAR_BUTTONS.swap(on, Ordering::Relaxed) != on
}

/// ★ m269 无边框形态（ini 键 `borderless` 的运行时镜像）。
///
/// 与 `TOOLBAR_BUTTONS` 一样放模块级：**绘制、几何、命中三处都读同一个
/// `toolbar_buttons()`**，一处收口就不会出现「按没画了还点得着」。
/// 唯一写点 = [`set_borderless`]，由 `runtime::apply_session_flags` 从 `Config` 回灌。
static BORDERLESS: AtomicBool = AtomicBool::new(false);

/// 写镜像，返回「值变没变」（调用方据此标脏）。
pub fn set_borderless(on: bool) -> bool {
    BORDERLESS.swap(on, Ordering::Relaxed) != on
}

/// 当前是不是无边框形态。画布（不画底色、不摆缩略图条）与命中都读这一份。
pub fn borderless() -> bool {
    BORDERLESS.load(Ordering::Relaxed)
}

fn toolbar_buttons() -> bool {
    TOOLBAR_BUTTONS.load(Ordering::Relaxed) && !BORDERLESS.load(Ordering::Relaxed)
}

/// [`Btn::ALL`] 现生成，加一个新按钮两处同时出现，不会只改一半。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Out,
    In,
    Fit,
    Actual,
    Fullscreen,
    Rotate,
    Mirror,
    // ── 动画组（`Chrome::row` 按"是不是动图"整组显隐，浮条与右键菜单同步）──
    /// 后退一帧（走 [`crate::core::hotkey::Action::FramePrev`]）。
    FrameBack,
    /// 暂停 / 继续动画。**双状态按钮**：字形跟着状态换（播放中画 ⏸、暂停中画 ▶）。
    PlayPause,
    /// 前进一帧（走 [`crate::core::hotkey::Action::FrameNext`]）。
    FrameFwd,
}

/// 标题栏窗口按钮。
///
/// `ALL` 是**右→左序**（下标越大越靠左）：布局、命中、绘制三处都以它为
/// 唯一真源，加按钮只改这一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBtn {
    Minimize,
    Maximize,
    Close,
    /// 置顶（读写 `ws.topmost`；激活 = 钉立正 + 绿色。右键菜单入口已删，本钮唯一开关）。
    Topmost,
    /// 锁住：锁定后整窗交互封锁，再点解锁（会话态 `Chrome::locked`）。
    Lock,
}

impl TitleBtn {
    /// Windows 标准顺序贴右缘，新按钮向左排：× ▢ _ 🔒 📌（视觉左→右）。
    pub const ALL: [TitleBtn; 5] = [
        TitleBtn::Close,
        TitleBtn::Maximize,
        TitleBtn::Minimize,
        TitleBtn::Lock,
        TitleBtn::Topmost,
    ];

    /// 按钮字形。`maximized` 只影响最大化钮：非最大化画方框（ChromeMaximize），
    /// 最大化时画还原图标（ChromeRestore）——用户靠字形切换才能一眼读出窗口状态
    /// （windui `WindowState` 文档点名的用法）。
    fn glyph(self, locked: bool, maximized: bool) -> &'static str {
        match self {
            TitleBtn::Minimize => "\u{E921}",              // ChromeMinimize
            TitleBtn::Maximize if maximized => "\u{E923}", // ChromeRestore
            TitleBtn::Maximize => "\u{E922}",              // ChromeMaximize
            TitleBtn::Close => "\u{E8BB}",                 // ChromeClose
            TitleBtn::Topmost => "\u{1F4CC}", // 📌 状态用颜色表达（paint 读 topmost_on）
            TitleBtn::Lock if locked => "\u{1F512}", // 🔒
            TitleBtn::Lock => "\u{1F513}",    // 🔓
        }
    }

    /// 标题按钮字形所在字体：emoji 区码位 MDL2 没有，锁住走 Segoe UI Symbol
    /// （CurSeen 已验证该字体下字形单色、不回退彩色 emoji）。置顶钮不走字形
    /// ——图钉是图元手绘（见 [`crate::ui::pin::render_pins`]），无需字体。
    fn font_family(self) -> Option<&'static str> {
        match self {
            TitleBtn::Lock => Some("Segoe UI Symbol"),
            _ => None,
        }
    }
}

impl Btn {
    /// Segoe MDL2 Assets 图标码位（码位出自微软官方图标表）。
    ///
    /// `paused` 只影响 [`Btn::PlayPause`]：它是双状态按钮，靠字形切换读出当前态
    /// （与 [`TitleBtn::glyph`] 的 `locked`/`maximized` 同一手法）。
    fn glyph(self, paused: bool) -> &'static str {
        match self {
            Btn::Out => "\u{E71F}",        // ZoomOut
            Btn::In => "\u{E8A3}",         // ZoomIn
            Btn::Fit => "\u{E799}",        // AspectRatio
            Btn::Actual => "\u{E71E}",     // Zoom（1:1 原尺寸）
            Btn::Fullscreen => "\u{E740}", // FullScreen
            Btn::Rotate => "\u{E7AD}",     // Rotate
            // MDL2 没有专用"镜像"字形，Switch（双向对箭头）是视觉上最接近的。
            Btn::Mirror => "\u{E8AB}", // Switch
            // 上一帧 / 下一帧用 Previous / Next（竖线 + 单三角）：双三角的
            // Rewind / FastForward 读起来是"快退快进"，与"一次一步"不符。
            Btn::FrameBack => "\u{E892}", // Previous
            Btn::FrameFwd => "\u{E893}",  // Next
            // 翻页用**尖角括号**（ChevronLeft/Right）：与逐帧的"竖线 + 三角"一眼可分，
            // 方向也直白。★ 码位必须先确认 `segmdl2.ttf` 的 cmap 里真有字形：
            // PUA 区里既有空字形也有缺字，写错就是浮条上两个豆腐块——E89C 其实是信封、
            // E89D 整个不存在（这两个都试过，真机截图上是空框）。
            Btn::PlayPause if paused => "\u{E768}", // Play：暂停中，按下去继续
            Btn::PlayPause => "\u{E769}",           // Pause：播放中，按下去暂停
        }
    }

    /// 悬停提示文字（图标按钮没有文字标签，悬停时浮出小条说明）。
    ///
    /// 也是右键菜单「操作」子菜单的项名——两处同文使人一眼对得上。
    ///
    /// ★ m279：**从 [`Self::action`] 对应的 `Action::name()` 派生**，不再另抄一张表。
    /// 快捷键页（行名）、浮条（悬停名）、右键「操作 ▸」（项名）从此共用一份正本。
    /// 以前是三处各写一遍，于是「动画上一帧」与「上一帧」、「暂停/播放动画」与
    /// 「暂停/继续动画」这种漂移没人能发现。名字取短名：快捷键页里「上一张」与
    /// 「上一帧」已经靠"张 / 帧"分得开，「动画」是冗余前缀；播放中按下去是"继续"
    /// 而不是"播放"，故统一成「暂停/继续动画」。
    pub fn name(self) -> &'static str {
        self.action().name()
    }

    /// 这颗按钮对应的动作：名字、快捷键显示、执行分派都从这里认人。
    pub fn action(self) -> crate::core::hotkey::Action {
        use crate::core::hotkey::Action;
        match self {
            Btn::Out => Action::ZoomOut,
            Btn::In => Action::ZoomIn,
            Btn::Fit => Action::Fit,
            Btn::Actual => Action::ActualSize,
            Btn::Fullscreen => Action::Fullscreen,
            Btn::Rotate => Action::Rotate,
            Btn::Mirror => Action::Flip,
            Btn::FrameBack => Action::FramePrev,
            Btn::PlayPause => Action::PauseAnim,
            Btn::FrameFwd => Action::FrameNext,
        }
    }

    /// 对应快捷键的显示文本（右键菜单尾随列）。从 [`crate::core::hotkey::Hotkeys`]
    /// 读当前绑定，菜单里写错了读者会立刻发现，故特意放在 `Btn` 上而不是在菜单里另抄一份。
    ///
    /// 多个绑定用 " / " 连接；无任何绑定返回 `None`（不画快捷键列）。
    pub fn shortcut(self, hk: &Hotkeys) -> Option<String> {
        hk.display(self.action())
    }

    /// 工具栏按钮全集。浮条布局、命中测试、右键菜单「操作」子菜单都从这里生成。
    ///
    /// ★ 动画那三颗**必须连续且排在末尾**——[`Chrome::row`] 用"截掉尾段"表达显隐，
    /// 插到中间就得改成逐颗过滤。翻页两颗紧挨动画组之后，同样必须连续
    /// （与动画组互斥显隐，二者在 `ALL` 里各占一段连续区间）。
    /// m247：只留操作类按钮；棋盘格归「外观 ▸」、属性归右键顶层，两颗都不再是按钮。
    pub const ALL: [Btn; 10] = [
        Btn::Out,
        Btn::In,
        Btn::Fit,
        Btn::Actual,
        Btn::Fullscreen,
        Btn::Rotate,
        Btn::Mirror,
        Btn::FrameBack,
        Btn::PlayPause,
        Btn::FrameFwd,
    ];

    /// 这颗属不属于动画组（`pub` 给测试与右键菜单用）。
    pub fn is_anim(self) -> bool {
        matches!(self, Btn::FrameBack | Btn::PlayPause | Btn::FrameFwd)
    }
}

/// 翻页箭头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Prev,
    Next,
}

/// 悬浮层共享状态：画布（绘制 + 指针）、顶部条、定时器三处共用。
pub struct Chrome {
    /// 极简模式：开启时仅在悬停特定元素上时显示该元素；关闭时始终显示。
    pub minimal_mode: Cell<bool>,
    /// 棋盘格透明背景开关（右键菜单与工具栏共用；m53 起落盘，ini 键 `checker`）。
    pub checker: Cell<bool>,
    /// 取色器开关（m217，右键「本图 ▸ 取色器」）。会话临时态，同极简模式，不落盘。
    /// 只进菜单不进浮条——用户口径「一排按钮已经很多了」。
    /// 旋转角度（整数度数，m280）。会话临时态、按看图窗分格、**不落盘**。
    ///
    /// 显示层、`rotate_dialog` 的 stepper 与另存路径共用这一份信号；主窗
    /// `window_tick` 差分它来标脏。放在这里而不是 `Config`：这不是偏好设置，
    /// 是"这张图当前被转成什么样"，同取色器 / 极简模式。
    pub rotate_deg: Signal<f64>,
    /// 这份角度属于哪张图：换图后自动失效，不用定时器清、不会串到另一扇窗
    /// （与 `LiveTint.path` 同一条判据）。`None` = 从未设过角度。
    pub rotate_path: Signal<Option<PathBuf>>,
    /// `window_tick` 上一拍已处理过的角度，差分用（同值零动作、不空转重绘）。
    pub rotate_seen: Cell<f64>,
    pub picker: Cell<bool>,
    /// 取色器当前的指针位置（画布逻辑坐标）。关模式、换图、指针离窗时清掉；
    /// 浮层绘制与点击复制读的是同一份，不许另存第二正本。
    pub picker_at: Cell<Option<(f32, f32)>>,
    /// 缩略图条开关（T 键 / 右键菜单）。
    ///
    /// 会话态但**落盘**（m53-L，ini 键 `filmstrip`）：开着关窗，下次启动照样开着。
    pub filmstrip: Cell<bool>,
    /// 缩略图条贴左边（m253，ini 键 `filmstrip_left`）：会话镜像，
    /// 唯一写点是 `runtime::apply_session_flags`（与 `filmstrip` 同一条通道）。
    pub filmstrip_left: Cell<bool>,
    /// 条**刚**被打开或换了边、但窗口放不下 ⇒ 请下一帧的 `paint` 算一次放大目标（m254）。
    ///
    /// ★ 一次性旗：`apply_session_flags` 只在变化那一帧置位，`paint` 消费即清。
    ///   于是用户自己把窗口拉小（条自动隐藏）不会被反向顶大——那种拉不动的窗口很讨厌。
    pub strip_fit_request: Cell<bool>,
    /// 待落地的客户区放大目标（**物理像素**）：`paint` 写、`window_tick` 取走发
    /// `SetClientSize`。放这里而不是直接调 `ctx` —— `paint` 手上没有 `EventCtx`。
    pub strip_resize: Cell<Option<(i32, i32)>>,
    /// 锁定（🔒）：会话态但**落盘**（m53-L，ini 键 `locked`）。
    /// 锁定后整窗交互封锁，只留锁住按钮解锁——语义与拦截地图见
    /// `docs/superpowers/specs/2026-10-12-titlebar-topmost-lock-design.md`。
    pub locked: Cell<bool>,
    /// 当前图**有没有帧/页号**（`Anim::has_index()`）。由 canvas 每帧回写，**决定浮条上
    /// 有没有播放组那三颗**（[`Chrome::row`]）；「操作▸」菜单读同一份。
    ///
    /// ★ 静态位图 / 单张 SVG 恒为假：没有第二张就不给 `(1/1)`，也不给逐帧按钮；
    ///   只有真动图与多页文档为真（判据在 `core::pixels::Anim::has_index`）。
    pub anim: Cell<bool>,
    /// 动画当前是否暂停。只影响 [`Btn::PlayPause`] 的字形（双状态按钮要能读出当前态）。
    pub paused: Cell<bool>,
    /// 最近一次指针活动时刻。
    pub last_move: Cell<Instant>,
    /// 最近一次指针位置（逻辑坐标，窗口内）。
    pub last_pos: Cell<(f32, f32)>,
    /// 记 `last_pos` **那一刻**的真实光标屏幕坐标（物理像素，见 [`super::super::platform::cursor`]）。
    ///
    /// 这一对是判据的两条腿：`last_pos` 是"我以为光标在哪"，`cursor_anchor` 是"我下这个
    /// 结论时系统说光标在哪"。后者与当前读数对不上 ⇒ `last_pos` 是过期记录，不能再拿它
    /// 判贴边/浮条豁免（m55，修 §10 那条"浮条收不走"）。
    pub cursor_anchor: Cell<Option<(i32, i32)>>,
    /// 画布内按下、尚未抬起的工具栏按钮 / 箭头。
    pub pressed: Cell<Option<Press>>,
    /// 当前悬停的工具栏按钮 / 箭头（据此决定要不要重绘）。
    pub hover_btn: Cell<Option<Btn>>,
    pub hover_arrow: Cell<Option<Arrow>>,
    /// 悬停在标题栏区域（顶部 STRIP_H 高度）。
    pub hover_title: Cell<bool>,
    /// 悬停在工具栏区域（底部浮条）。
    pub hover_toolbar: Cell<bool>,
    /// 悬停在信息条区域。**常态下信息在浮条里，那一格由 [`Chrome::hover_toolbar`] 覆盖**；
    /// 这一位只为窄窗的"整行档"存在（那一行在浮条外面，见 [`info_place`]）。
    pub hover_info: Cell<bool>,
    /// 极简模式下左右**翻页箭头**各自浮不浮现：`(上一张那侧, 下一张那侧)`。
    ///
    /// 判据是指针落在该侧的 [`ARROW_ZONE`] 边带里，**不是**箭头那个圆——图形可以完全
    /// 不画，热区一寸不动（[`Chrome::hit`] 仍按圆判），否则"隐藏"就把鼠标用户的翻页
    /// 入口一起藏了（2026-10-19 用户翻案：既然要极简就追求到底，但悬停还是要出现）。
    /// 这一位与标题栏/浮条同一条链：`activity` 现算、翻转会报"要重绘"、`pointer_left` 清。
    /// [`Chrome::tick_hide`] 的**静止计时**不参与这一位：顶条/浮条要静止计时是因为"光标还在
    /// 上面但不动了"不该把栏抽走，而这一位本来就是纯位置判据——人还在边带里就该亮着。
    /// 但它那条**过期判据**参与（m55）：光标早已不在窗口里时，这一位同样滞留，
    /// 靠 `pointer_left` 一并清掉。
    pub hover_edge: Cell<(bool, bool)>,
    /// 悬停的标题栏窗口按钮。
    pub hover_title_btn: Cell<Option<TitleBtn>>,
    /// 按下的标题栏窗口按钮。
    pub pressed_title_btn: Cell<Option<TitleBtn>>,
    /// 信息条**实际**绘制矩形（窗口绝对坐标；draw 每帧回写，没画时为零矩形）。
    /// hover/tick_hide 读同一份几何——文本宽只有测量后才知道，命中区必须与画
    /// 出来的那块同源，否则固定估宽会把 hover 判到根本没画的地方。
    pub info_rect: Cell<(f32, f32, f32, f32)>,
    /// 上一帧**实际**用的状态格宽（0 = 这一帧没有格子：整行档，或没有状态文本）。
    ///
    /// 浮条现在跟着文本伸缩，于是"浮条在哪"不再只由窗口宽决定——命中、悬停、
    /// 鸟瞰让位**必须读同一份**，否则点的是画出来的位置、算的是另一套位置。
    /// 一帧的滞后只发生在文本换长度那一帧（翻页 / 暂停），代价是那一帧的按钮判定
    /// 偏半个格；比"两套几何各自为政"便宜得多。
    pub info_cell: Cell<f32>,
    /// 平移重绘的最小间隔（M8）。默认 = 显示器刷新周期，见
    /// [`crate::platform::display`]；测试里把它钉住，判据才不靠"这台机器跑得快"。
    pub pan_interval: Cell<Duration>,
    /// 上一帧画过的状态文本。只用来判"状态变了没有"，见 [`Chrome::wake_on_status_change`]。
    last_info: RefCell<String>,
    /// 状态文本最后一次变化时，浮条该亮到什么时候——[`Chrome::wake_on_status_change`] 的唯一凭证。
    ///
    /// ★ **必须独立于 `hover_toolbar` 那一格**。逐像素透明窗每次重绘都会让系统补一条指针消息，
    /// `activity` 于是每帧都把"指针在不在浮条上"重算成 `false` 并清掉 `hover_toolbar`；
    /// 而动画每 100 ms 又唤醒一次——两条通道抢同一格，浮条就在显示/隐藏之间高频翻转。
    /// 用户在极简模式下播 AVIF 序列时看到的"底部按钮栏一直闪"就是这条（实测见
    /// `build/pill_probe.ps1`：同一点在 `FF101010` 与 `FF1B1B1C` 之间交替）。
    /// 状态唤醒有自己的到期时刻，`activity` / `pointer_left` 都碰不到它。
    status_until: Cell<Option<Instant>>,
    /// 下一次允许平移重绘的最早时刻。
    pan_due: Cell<Instant>,
    /// 落在间隔里、被**挂起**的那一帧：pan 已写进状态机，只是还没画。
    pan_pending: Cell<bool>,
    /// 裸滚轮翻页的最小间隔（A3 / m64）。默认 [`WHEEL_PACE`]，测试里钉住它，
    /// 判据才不靠"这台机器跑得快"（与 `pan_interval` 同一定式）。
    wheel_interval: Cell<Duration>,
    /// 下一次允许翻页的最早时刻。
    wheel_due: Cell<Instant>,
    /// 压在闸里、等 [`tick_wheel_pace`](Self::tick_wheel_pace) 兜的那一下翻页方向
    /// （`0` = 没有）。**挂起而不是丢弃**：丢弃会让"滚了两格只翻一张"成为常态手感。
    wheel_pending: Cell<isize>,
    // ★ 这五个字段是 `pub(crate)` 而不是私有：`ui::canvas` 那条接线用例要在不 sleep 的
    // 前提下挪钟（与 `wheel_*` 同一定式，只是滚轮闸的用例本来就住在本模块，不必放开）。
    /// 键盘翻页的最小间隔（m81）。默认 [`KEY_PACE`]，测试里钉住它（与 `wheel_interval` 同一定式，
    /// 判据不许依赖"这台机器跑得快"）。
    pub(crate) key_interval: Cell<Duration>,
    /// 下一次允许键盘翻页的最早时刻。
    pub(crate) key_due: Cell<Instant>,
    /// 压在键盘闸里、等 [`tick_key_pace`](Self::tick_key_pace) 兜的那一下方向（`0` = 没有）。
    pub(crate) key_pending: Cell<isize>,
    /// 上一记键盘翻页的时刻与方向——"是不是按住"就判这两个数（见 [`Chrome::key_gate`]）。
    pub(crate) key_last: Cell<Instant>,
    pub(crate) key_last_dir: Cell<isize>,
    // ── 幻灯片放映（m167）。与上面两条闸同一定式：字段 `pub(crate)` 是给接线用例挪钟用的，
    // 判据不许靠"这台机器刚好跑得够慢"。
    /// 是否在放映。**会话态、不落盘**：关窗即停，"上次是不是在放"不是用户要记住的事。
    pub(crate) slideshow: Cell<bool>,
    /// 下一次自动翻页的最早时刻（进入放映时置为"现在"，之后每翻一次推一格）。
    pub(crate) slide_due: Cell<Instant>,
    /// 当前这一格的间隔（= 上一次 `tick_slideshow` 拿到的值）。进度条拿它当分母。
    pub(crate) slide_span: Cell<Duration>,
    /// 上一帧**宣布过**的进度格。`slide_step_tick` 靠它把"要不要重绘"压成"换格才刷"。
    pub(crate) slide_step_announced: Cell<u32>,
    /// 「复制当前（PNG）」那一枚还在后台编（m74）。它决定两件事：轮询要不要说话、
    /// 第二枚该不该被拒。
    ///
    /// **在飞位就是单飞闸**：第二枚在 `ui::canvas::copy_display` 里被 `Busy` 挡回去，
    /// 所以任何时刻最多只有一枚线程在飞——不需要代号来分辨"谁是最新"，旧的回执根本
    /// 不可能和新通道同时存在（每次登记都换一条新通道，旧 `Sender` 发送即失败）。
    copy_busy: Cell<bool>,
    /// 回执通道（线程 → UI 线程）。`Receiver` 住在 `RefCell` 里由 UI 线程独占
    /// （`Chrome` 本来就不是 `Send`，一次性线程只拿走 `Sender`）。
    copy_rx: RefCell<Option<Receiver<Result<(), String>>>>,
    /// 投出去那一刻给用户的那句话（带尺寸）。留在字段里而不是写死在调用点，
    /// 是为了让单测能读到"用户当时看见的话里说了这张多大"。
    copy_note: RefCell<String>,
    // 保存与复制必须分开收尾，不能把“已保存”误报为“已复制”。
    save_rx: RefCell<Option<Receiver<Result<(), String>>>>,
    save_notice: RefCell<Option<String>>,
    /// 置顶图钉的两张离屏位图（斜躺灰 / 立正绿），首次 paint 时惰性生成、终身缓存。
    /// 姿态语义与 CurSeen 一致：📌 字形原生斜躺，置顶时立正（见 `ui::pin`）。
    pins: RefCell<Option<(Image, Image)>>,
    /// 文本测量与截断的记账（见 [`TextMemo`]）。浮条每帧都要为同一串字量好几遍，
    /// 而字与宽只在翻页 / 改倍率 / 改窗宽 / 换 DPI 时才变。
    text: RefCell<TextMemo>,
}

/// 一帧里同一串文本会被量很多遍：状态格定宽量一次、标题截断量一次、提示条再量一次，
/// 而 [`clip_text`] 放不下时还要**二分前缀**，每一跳都量一次——现役后端（`d2d`）的
/// `measure_text` 每次都是真排版。文本与可用宽一帧与一帧之间绝大多数是同一份，
/// 所以按 (文本, 字号, dpi[, 可用宽]) 记几槽结果。
///
/// ★ 键里必须有 `dpi`：换屏（不同缩放率）后同一串字的逻辑宽会变，漏掉它就是把上一块屏
///   的数字画到这一块屏上（截断多一节或少一节，都是看得见的错）。
#[derive(Default)]
struct TextMemo {
    /// `(文本, 字号, dpi)` → 未截断的宽。
    widths: [Option<WidthSlot>; 4],
    /// `(文本, 字号, dpi, 可用宽)` → 截断后的串。
    clips: [Option<ClipSlot>; 4],
    /// 轮转下标：写满就回头挤最旧的一槽。一帧只写两三槽，轮转足以把过期形态挤出去，
    /// 不值得为它养一棵 LRU。
    next: Cell<usize>,
}

/// 一槽测量账：`(文本, 字号, dpi, 宽)`。
type WidthSlot = (String, f32, f32, f32);
/// 一槽截断账：`(文本, 字号, dpi, 可用宽, 截断结果)`。
type ClipSlot = (String, f32, f32, f32, String);

/// 画布内按下的对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Btn(Btn),
    Arrow(Arrow),
    TitleBtn(TitleBtn),
}

/// 「复制当前（PNG）」在后台那一枚的落地状态（m74），由
/// [`Chrome::tick_copy`] 每 16 ms 问一次。
///
/// ★ 类型里**只有一条"已经复制好"**：`Idle` / `Waiting` 都不许被调用方读成成功——
/// 复制是屏幕上看不见的副作用（AGENTS §八.7），一旦失败却一声不出，用户就以为手里有图。
#[derive(Debug, PartialEq, Eq)]
pub enum CopyWait {
    /// 没有在飞的复制。
    Idle,
    /// 还在编：这一帧什么都别说。
    Waiting,
    /// 那一枚已经进剪贴板。
    Copied,
    /// 那一枚失败了，带要如实报给用户的原因。
    Failed(String),
}

impl Chrome {
    pub fn new(minimal_mode: bool) -> Rc<Self> {
        std::rc::Rc::new(Self {
            minimal_mode: Cell::new(minimal_mode),
            rotate_deg: signal(0.0),
            rotate_path: signal(None),
            rotate_seen: Cell::new(0.0),
            picker: Cell::new(false),
            picker_at: Cell::new(None),
            checker: Cell::new(false),
            filmstrip: Cell::new(false),
            filmstrip_left: Cell::new(false),
            strip_fit_request: Cell::new(false),
            strip_resize: Cell::new(None),
            locked: Cell::new(false),
            anim: Cell::new(false),
            paused: Cell::new(false),
            last_move: Cell::new(Instant::now()),
            last_pos: Cell::new((f32::MAX, f32::MAX)),
            cursor_anchor: Cell::new(None),
            pressed: Cell::new(None),
            hover_btn: Cell::new(None),
            hover_arrow: Cell::new(None),
            hover_title: Cell::new(false),
            hover_toolbar: Cell::new(false),
            hover_info: Cell::new(false),
            hover_edge: Cell::new((false, false)),
            hover_title_btn: Cell::new(None),
            pressed_title_btn: Cell::new(None),
            info_rect: Cell::new((0.0, 0.0, 0.0, 0.0)),
            info_cell: Cell::new(0.0),
            pan_interval: Cell::new(Duration::from_secs_f64(
                1.0 / crate::platform::display::refresh_hz() as f64,
            )),
            last_info: RefCell::new(String::new()),
            status_until: Cell::new(None),
            pan_due: Cell::new(Instant::now()),
            pan_pending: Cell::new(false),
            wheel_interval: Cell::new(WHEEL_PACE),
            wheel_due: Cell::new(Instant::now()),
            wheel_pending: Cell::new(0),
            key_interval: Cell::new(KEY_PACE),
            key_due: Cell::new(Instant::now()),
            key_pending: Cell::new(0),
            key_last: Cell::new(Instant::now()),
            key_last_dir: Cell::new(0),
            slideshow: Cell::new(false),
            slide_due: Cell::new(Instant::now()),
            slide_span: Cell::new(Duration::ZERO),
            slide_step_announced: Cell::new(0),
            copy_busy: Cell::new(false),
            copy_rx: RefCell::new(None),
            copy_note: RefCell::new(String::new()),
            save_rx: RefCell::new(None),
            save_notice: RefCell::new(None),
            pins: RefCell::new(None),
            text: RefCell::new(TextMemo::default()),
        })
    }

    /// 取色器开关（m217）：菜单项、K 键共用这一处，`picker_at` 随手清掉，
    /// 免得下次开模式时有一枚停在旧位置的浮层。
    pub fn toggle_picker(&self) {
        self.picker.set(!self.picker.get());
        self.picker_at.set(None);
    }

    /// 关掉取色器（m217）：点画面复制、Esc、指针离窗后的收尾都走这一处。
    pub fn close_picker(&self) {
        self.picker.set(false);
        self.picker_at.set(None);
    }

    /// 量一次文本宽，走过 [`TextMemo`] 的账：同一串字 + 同一字号 + 同一 DPI 只量一次。
    pub fn measure_w(&self, canvas: &mut dyn Canvas, text: &str, ts: &TextStyle) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let (size, dpi) = (ts.size, canvas.dpi_scale());
        let hit = self
            .text
            .borrow()
            .widths
            .iter()
            .flatten()
            .find(|(t, s, d, _)| t == text && *s == size && *d == dpi)
            .map(|(_, _, _, w)| *w);
        if let Some(w) = hit {
            return w;
        }
        let w = canvas.measure_text(text, ts).w as f32;
        let mut m = self.text.borrow_mut();
        let i = m.next.get() % m.widths.len();
        m.widths[i] = Some((text.to_string(), size, dpi, w));
        m.next.set(i + 1);
        w
    }

    /// 单行截断，走过 [`TextMemo`] 的账（口径与 [`clip_text`] 完全一致，只是不再重算）。
    ///
    /// 快路不止一条：量出的宽既然已经记着，"整串放得下"这一档**连二分都不用进**；
    /// 真放不下的那一档（病态长名 / 病态长状态串）才用截断结果那槽。
    pub fn clip(&self, canvas: &mut dyn Canvas, text: &str, ts: &TextStyle, avail: f32) -> String {
        if text.is_empty() || avail <= 0.0 {
            return String::new();
        }
        let (size, dpi) = (ts.size, canvas.dpi_scale());
        let clipped = {
            let m = self.text.borrow();
            m.clips
                .iter()
                .flatten()
                .find(|(t, s, d, a, _)| t == text && *s == size && *d == dpi && *a == avail)
                .map(|(_, _, _, _, out)| out.clone())
        };
        if let Some(out) = clipped {
            return out;
        }
        let full_w = self.measure_w(canvas, text, ts);
        if full_w <= avail {
            return text.to_string();
        }
        let out = clip_text(canvas, text, ts, avail);
        let mut m = self.text.borrow_mut();
        let i = m.next.get() % m.clips.len();
        m.clips[i] = Some((text.to_string(), size, dpi, avail, out.clone()));
        m.next.set(i + 1);
        out
    }

    /// 平移**确实变了**之后问一句：这一帧该不该画（M8 拖动限速）。
    ///
    /// `true` = 该 `mark_dirty`，同时把下一次允许的时刻推后一个间隔；`false` = 还在
    /// 间隔内，pan 已经写进状态机了，只是这一帧不画，由 [`tick_pan_pace`](Self::tick_pan_pace)
    /// 到点补、或 [`flush_pan`](Self::flush_pan) 在松手时补。
    ///
    /// ★ 是**挂起**而不是丢弃：丢弃会让画面停在松手前一步的位置，那是看得见的"差一点"，
    /// 而挂起的代价最多一帧（60 Hz 下 16 ms），呈现上根本没有可感知的差别——这条界线
    /// 就是这次改动"省 CPU 但不换手感"的全部依据，改之前先重读 M8 的表。
    pub fn pan_dirty(&self) -> bool {
        let now = Instant::now();
        if now < self.pan_due.get() {
            self.pan_pending.set(true);
            return false;
        }
        self.pan_due.set(now + self.pan_interval.get());
        self.pan_pending.set(false);
        true
    }

    /// 16 ms 轮询里调：挂起的那一帧到点了没有。返回 `true` = 该重绘（画的是最新 pan）。
    ///
    /// 为什么要有这一条而不是"下一条鼠标消息自然补上"：手停下之后就没有下一条了。
    pub fn tick_pan_pace(&self) -> bool {
        if !self.pan_pending.get() {
            return false;
        }
        let now = Instant::now();
        if now < self.pan_due.get() {
            return false;
        }
        self.pan_due.set(now + self.pan_interval.get());
        self.pan_pending.set(false);
        true
    }

    /// 手势收尾（松手）：把挂起的那一帧立刻交出去，画面才停在用户真正松手的位置。
    pub fn flush_pan(&self) -> bool {
        if !self.pan_pending.replace(false) {
            return false;
        }
        true
    }

    /// 裸滚轮翻页问一句：这一下该**立刻翻**，还是压在闸里等兜（A3 / m64）。
    ///
    /// `true` = 立刻翻，并把下一次允许的时刻推后一个 [`Chrome::wheel_interval`]；
    /// `false` = 还在间隔里，方向已记进 `wheel_pending`，由 [`tick_wheel_pace`](Self::tick_wheel_pace)
    /// 到点兜出去。`dir` 是 `±1`，与 `viewer::step` 同口径。
    ///
    /// ★ 又是**挂起而不是丢弃**（与 M8 平移限速同一条界线）：丢弃会让"快滚两格只翻一张"
    /// 变成常态手感，而挂起的代价最多一个间隔。触控板一次惯性滑行照样被压到每 100 ms 一张——
    /// 它后面一直有事件，兜上的永远是**最后**那一下的方向。
    pub fn wheel_gate(&self, dir: isize) -> bool {
        // 用户亲手滚了，放映就该让位（见 [`Chrome::slide_stop`]）。
        self.slide_stop();
        let now = Instant::now();
        if now < self.wheel_due.get() {
            self.wheel_pending.set(dir);
            return false;
        }
        self.wheel_due.set(now + self.wheel_interval.get());
        self.wheel_pending.set(0);
        true
    }

    /// 16 ms 轮询里调：压在闸里的那一下到点了没有。返回 `Some(dir)` = 该翻这一张。
    ///
    /// 为什么要有这一条而不是"下一条滚轮消息自然补上"：手停下之后就没有下一条了——
    /// 那一下翻页会**永远**丢掉，比限速本身难看得多（与 [`tick_pan_pace`](Self::tick_pan_pace) 同理）。
    pub fn tick_wheel_pace(&self) -> Option<isize> {
        let dir = self.wheel_pending.get();
        if dir == 0 || Instant::now() < self.wheel_due.get() {
            return None;
        }
        self.wheel_pending.set(0);
        Some(dir)
    }

    /// 键盘翻页问一句：这一下该**立刻翻**，还是压在闸里等兜（m81）。
    ///
    /// 判据是**同方向两键的间隔**：短于 [`Chrome::key_interval`] 就当是按住自重复，压到
    /// 每个间隔一张；间隔不小于它的就是一记一记的手按，一律当场翻、不排队、不欠账。
    /// 换了方向也算新的一记——上一把键压出来的闸不许吃掉这一记（这条与"手按不该丢"同侧）。
    ///
    /// ★ 仍然是**挂起而不是丢弃**（与 [`wheel_gate`](Self::wheel_gate) 同一条界线）：压在闸里的
    /// 方向记在 `key_pending`，由 [`tick_key_pace`](Self::tick_key_pace) 到点兜出去。松手前
    /// 最后那一下如果不兜，用户按了一次却什么也没发生，比限速本身难看得多。
    pub fn key_gate(&self, dir: isize) -> bool {
        // 用户亲手按键（哪怕这一下被闸压住），放映都得让位（见 [`Chrome::slide_stop`]）。
        self.slide_stop();
        let now = Instant::now();
        let interval = self.key_interval.get();
        let held = self.key_last_dir.get() == dir
            && now.saturating_duration_since(self.key_last.get()) < interval;
        self.key_last.set(now);
        self.key_last_dir.set(dir);
        if !held {
            self.key_due.set(now + interval);
            self.key_pending.set(0);
            return true;
        }
        if now < self.key_due.get() {
            self.key_pending.set(dir);
            return false;
        }
        self.key_due.set(now + interval);
        self.key_pending.set(0);
        true
    }

    /// 16 ms 轮询里调：压在键盘闸里的那一下到点了没有。与 [`tick_wheel_pace`](Self::tick_wheel_pace)
    /// 成对——按住松开之后就没有下一条按键消息了，只有这条轮询能兜住最后那一步。
    ///
    /// ★ 兜出去这一张的同时**把闸推后一个间隔**：这一翻与按键触发的那翻是同一件事，
    /// 不推就等于"轮询翻一张 + 紧接着下一记按键再翻一张"。实测就是这个形状——40 ms 洪流里
    /// 上屏间隔呈 `+353ms / +15ms` 成对，实际速率 5.7 张/秒，比承诺的"一秒两三张"快一倍。
    pub fn tick_key_pace(&self) -> Option<isize> {
        let dir = self.key_pending.get();
        let now = Instant::now();
        if dir == 0 || now < self.key_due.get() {
            return None;
        }
        self.key_due.set(now + self.key_interval.get());
        self.key_pending.set(0);
        Some(dir)
    }

    /// WASD 方向编码 → 平移符号（`sx`, `sy`），与 [`crate::core::viewer::Viewer::pan_nudge`]
    /// 的约定一致：`sy = +1` 是"看上方"（图像下移）。canvas 的动作臂专用这一份表——
    /// 符号写反的症状是按 W 画面往上飘（看的是图的下缘），越按越离谱。
    pub(crate) fn wasd_signs(dir: u8) -> (f32, f32) {
        match dir {
            0 => (0.0, 1.0),  // W：上看
            1 => (1.0, 0.0),  // A：左看
            2 => (0.0, -1.0), // S：下看
            3 => (-1.0, 0.0), // D：右看
            _ => (0.0, 0.0),
        }
    }

    /// 切换幻灯片放映。快捷键、右键顶层「幻灯片放映」菜单项两条路都进这里——只有一套语义，
    /// 不存在"键会停、菜单不会"这种半截实现（与动画三颗同一条纪律）。
    ///
    /// 进入时把到点时刻直接设成"现在"：按下就该**立刻**看见下一张，而不是先空等一个
    /// 间隔——那三秒的沉默会让人以为没生效又按一次（然后变成关掉）。
    pub fn toggle_slideshow(&self) -> bool {
        let on = !self.slideshow.get();
        self.slideshow.set(on);
        if on {
            self.slide_due.set(Instant::now());
        }
        log::info!(
            "[slideshow] verdict={}",
            if on { "start" } else { "stop:toggle" }
        );
        on
    }

    /// 是否在放映（菜单项的名字据此换「幻灯片放映」/「停止放映」）。
    pub fn slideshow_on(&self) -> bool {
        self.slideshow.get()
    }

    /// **任何手动输入都先停放映**：键盘闸、滚轮闸、浮条按钮、翻页箭头、快捷键动作
    /// 五处各调一次（每一处都是"用户亲手插手翻页"的证据）。
    ///
    /// 为什么这条比"再按一次关掉"重要：定时器与用户抢同一张图时，用户翻过去的那一张
    /// 会被下一格定时翻回来——那是最难解释的手感 bug，比少一个开关难查得多。
    /// 没在放就静默返回：这条挂在每一记输入上，不该每次都写一行日志。
    pub fn slide_stop(&self) {
        if !self.slideshow.get() {
            return;
        }
        self.slideshow.set(false);
        log::info!("[slideshow] verdict=stop reason=input");
    }

    /// 16 ms 轮询里调：到点了没有。到点就把闸往后推 `interval` 并返回 `true`，
    /// 调用方负责翻一张（走 [`crate::core::viewer::Viewer::step`] 那同一条翻页路，
    /// 目录末尾自然回绕——放映不引入第二套翻页语义）。
    ///
    /// 间隔由调用方每次传进来（配置项，放映中途改也照生效），`Chrome` 不揣着 `Config`。
    pub fn tick_slideshow(&self, interval: Duration) -> bool {
        if !self.slideshow.get() {
            return false;
        }
        let now = Instant::now();
        if now < self.slide_due.get() {
            return false;
        }
        self.slide_due.set(now + interval);
        // 进度条的分母：间隔在设置页被改过，下一张起就用新值（不记录就会拿旧间隔画新格子）。
        self.slide_span.set(interval);
        true
    }

    /// 当前进度落在第几格（`0..=SLIDE_STEPS`，一格 = 一列像素）。**没在放映一律 0**（进度色不许留在屏上）。
    ///
    /// 用"还剩多少"反推，而不是自己数帧：这样设置页改间隔、机器卡了一下都不会让条与
    /// 真正翻页的时刻脱钩——条走完就是翻页，两者由同一个 `slide_due` 决定。
    pub fn slide_step(&self) -> u32 {
        if !self.slideshow.get() {
            return 0;
        }
        let span = self.slide_span.get();
        if span.is_zero() {
            // 刚进入放映、还没翻过第一张：这一格没有分母可用，画满格以外的都不对，就画 0。
            return 0;
        }
        let remaining = self
            .slide_due
            .get()
            .saturating_duration_since(Instant::now())
            .min(span);
        let done = span - remaining;
        let per = span.as_millis().max(1) as u64;
        (((done.as_millis() as u64) * SLIDE_STEPS as u64) / per).min(SLIDE_STEPS as u64) as u32
    }

    /// 16 ms 轮询里调：**只在换了一格的那一帧**返回 `true`（= 要重绘）。
    ///
    /// 为什么不直接每帧标脏：整扇窗口是逐像素分层的（`UpdateLayeredWindow`），一次重绘
    /// 就是整块缓冲重合成；跟着帧钟每秒约 60 次去刷一层颜色，反着"低占用"这把尺子。
    /// 8 格/间隔把重绘次数压到每间隔 8 次——**代价的量化与视觉的连续在这里是同一件事**。
    pub fn slide_step_tick(&self) -> bool {
        let step = self.slide_step();
        if step == self.slide_step_announced.get() {
            return false;
        }
        self.slide_step_announced.set(step);
        true
    }

    /// 另存当前编辑结果为 PNG：**编码与写盘都在一次性线程上**。
    ///
    /// 为什么不能留在对话框回调里：那是在 UI 线程上编一张整图（1620×1080 默认档实测
    /// 828 ms），而失败如果只写日志，保存框已经关了——用户会以为已经存上（无痕失败）。
    /// 结果由 [`Self::tick_save`] 交回主轮询，成与败都在那一帧说出去。
    pub fn start_save_png(
        &self,
        dest: Option<std::path::PathBuf>,
        w: u32,
        h: u32,
        rgba: std::sync::Arc<Vec<u8>>,
    ) {
        let Some(dest) = dest else { return };
        if self.save_rx.borrow().is_some() {
            *self.save_notice.borrow_mut() = Some("正在保存上一张涂鸦，请稍后重试".into());
            return;
        }
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("save-png".into())
            .spawn(move || {
                // 只带走像素快照，保存成功/失败都不能清掉 viewer 的编辑覆盖。
                // 另存是后台线程，不必为速度让体积（见 `encode_png_compact` 的注释）。
                //
                // ★ 落盘一律用 `fs::write`（= `File::create`，Windows 上是 `CREATE_ALWAYS`，
                //   **先截断**）。别改成复用句柄的 `OpenOptions::new().write(true)`：那样
                //   覆盖一张比原来短的图时，旧数据的尾巴会留在新文件后面，裁掉的部分能
                //   整块恢复回来——这就是 aCropalypse（CVE-2023-21036 / CVE-2023-28303，
                //   Windows 截图工具与 Pixel Markup 都栽在这上面）。
                //   我们**没有**原地保存的入口（只有「另存为」），所以本就没这个洞；
                //   这条注释是给下一次改动的：把它改成不截断就当场开洞。
                let out =
                    crate::platform::clipboard::encode_png_compact(w, h, &rgba).and_then(|bytes| {
                        std::fs::write(&dest, bytes).map_err(|e| format!("写入失败：{e}"))
                    });
                match &out {
                    Ok(()) => log::info!("[plugin-edit] verdict=saved file={}", dest.display()),
                    Err(e) => log::warn!(
                        "[plugin-edit] verdict=save-failed file={} err={e}",
                        dest.display()
                    ),
                }
                let _ = tx.send(out.map_err(|e| format!("另存为失败：{e}")));
            });
        match spawned {
            Ok(_) => *self.save_rx.borrow_mut() = Some(rx),
            Err(e) => {
                log::warn!("[plugin-edit] verdict=spawn-failed err={e}");
                *self.save_notice.borrow_mut() = Some(format!("后台保存起不动：{e}"));
            }
        }
    }

    pub fn tick_save(&self) -> Option<Result<(), String>> {
        if let Some(e) = self.save_notice.take() {
            return Some(Err(e));
        }
        let rx = self.save_rx.take()?;
        match rx.try_recv() {
            Ok(out) => Some(out),
            Err(TryRecvError::Empty) => {
                *self.save_rx.borrow_mut() = Some(rx);
                None
            }
            Err(TryRecvError::Disconnected) => Some(Err("后台保存未完成，请重试".into())),
        }
    }

    /// 有复制还在后台编。第二枚该不该被拒、`tick_copy` 该不该说话，都问这一条。
    ///
    /// 这一位就是**单飞闸**：它拦住第二枚的登记，于是通道里永远只有一枚在飞，
    /// 回执天然没有"新旧"可分辨的余地。★ 复制与另存各用一位：两条腿可以同时在跑，
    /// 互相不许顶掉对方的回执。
    pub fn copy_in_flight(&self) -> bool {
        self.copy_busy.get()
    }

    /// 登记一枚已经投出去的后台复制，并交出回执通道（`Sender` 由调用方搬进线程）。
    pub fn copy_posted(&self, note: impl Into<String>) -> Sender<Result<(), String>> {
        let (tx, rx) = channel();
        *self.copy_rx.borrow_mut() = Some(rx);
        *self.copy_note.borrow_mut() = note.into();
        self.copy_busy.set(true);
        tx
    }

    /// 线程起不来（`spawn` 失败）：撤掉在飞位。调用方必须把这一条**如实报出去**，
    /// 不许退回"当场同步编"——那等于把本轮要消掉的那次长停顿又请回来。
    pub fn copy_reset(&self) {
        self.copy_busy.set(false);
        *self.copy_rx.borrow_mut() = None;
        self.copy_note.borrow_mut().clear();
    }

    /// 投出去那一刻给用户看的那句话（带尺寸，见 `ui::canvas` 的 `copy_display`）。
    pub fn copy_note(&self) -> String {
        self.copy_note.borrow().clone()
    }

    /// 16 ms 轮询里调：后台那枚复制落地了没有。
    ///
    /// 通道里只可能有一枚（[`Chrome::copy_in_flight`] 拦住了第二枚），所以回执不编号：
    /// 收到的那一条就是当前那一次的落地。收的时候把 `Receiver` 从 `RefCell` 里
    /// `take()` 出来、收完再放回去：借住期间再碰 `copy_rx` 就是 `RefCell` 冲突，
    /// 而 UI 线程里的 panic 等于窗口没了。
    pub fn tick_copy(&self) -> CopyWait {
        if !self.copy_busy.get() {
            return CopyWait::Idle;
        }
        let rx = match self.copy_rx.take() {
            Some(r) => r,
            None => {
                self.copy_reset();
                return CopyWait::Idle;
            }
        };
        let out = match rx.try_recv() {
            Ok(Ok(())) => CopyWait::Copied,
            Ok(Err(e)) => CopyWait::Failed(e),
            Err(TryRecvError::Empty) => CopyWait::Waiting,
            // `Sender` 被丢掉而没发回执 = 那一枚线程半路没了（panic 才会）。
            // 停在 `Waiting` 就是把在飞位永久占住：此后每次复制都被"还在进行"拒掉，
            // 而且是静默的，用户只能重启。所以这里如实报一条并把在飞位清掉。
            Err(TryRecvError::Disconnected) => CopyWait::Failed("后台复制没能完成，请重试".into()),
        };
        match out {
            // 还没落地：把通道放回原位，下一轮接着收。
            CopyWait::Waiting => *self.copy_rx.borrow_mut() = Some(rx),
            _ => self.copy_reset(),
        }
        out
    }

    /// 状态文本变了 → 把浮条叫醒。返回 true = 刚从"没显示"切到"显示"（调用方该标脏）。
    ///
    /// 为什么需要这一条：极简模式下浮条靠**指针悬停**显示，而逐帧浏览是按 `,` `.` 键——
    /// 鼠标一动不动，2 秒后浮条收走，帧号（屏幕上唯一的载体，AGENTS §八.7）就跟着没了。
    /// 借 [`Chrome::hover_toolbar`] 这一位而不是新起一条可见性通道：`tick_hide` 到点
    /// 自己会把它清掉，于是"亮一会儿再淡出"，与手停下来的表现完全一致。
    /// 只在**文本真的变了**时才叫醒：动画播放期每帧都重绘，状态串却没变。
    pub fn wake_on_status_change(&self, info: &str) -> bool {
        {
            let last = self.last_info.borrow();
            if *last == info {
                return false;
            }
        }
        *self.last_info.borrow_mut() = info.to_string();
        // 空状态（例如错误只留中央那一格没内容）不叫醒：亮一条空浮条是纯打扰。
        if info.is_empty() {
            return false;
        }
        // 记**自己的**到期时刻，不碰 `hover_toolbar`：那一格归 `activity` 管，逐像素透明窗
        // 每帧重绘都会补一条指针消息把它判成 false（见字段注释）。返回值仍是"从暗到亮"
        // 这一个翻转，调用方据此标脏。
        let was = self.status_alive();
        self.status_until.set(Some(Instant::now() + IDLE_HIDE));
        !was
    }

    /// 「状态刚变、浮条该亮着」是否还在有效期。
    ///
    /// 与 [`Chrome::hover_toolbar`] 是**两条独立通道**：前者由状态文本驱动（动画每帧刷新，
    /// 于是播放期一直亮），后者由指针位置驱动（每帧重算，指针离开浮条就灭）。
    /// 绘制里两者取"或"，谁亮都算亮——这正是"亮一会儿再淡出"的原意。
    pub fn status_alive(&self) -> bool {
        matches!(self.status_until.get(), Some(t) if t > Instant::now())
    }

    /// 指针落在**顶部标题栏那一条**里吗。
    ///
    /// 比画出来的 [`STRIP_H`] 高 8px（迟滞）：贴着边缘按下不该因 1px 之差掉出标题栏。
    /// 用窗口绝对坐标，不随 layout 变化。
    ///
    /// ★ 这条判据在这里只写一份，三个消费方共用：极简模式下的悬停显隐
    /// （[`Chrome::activity`]）、静止收起（[`Chrome::tick_hide`]）、以及左键手势的分工
    /// （`ui::canvas` 里"标题栏那一条永远拖窗口"）。各算一遍就会出现
    /// "标题栏的按钮亮着、而那一按拖的是图片"——同一块面积两套说法。
    pub fn in_title_strip(pos: (f32, f32), view: (f32, f32, f32, f32)) -> bool {
        let (abs_x, abs_y) = pos;
        let (vx, _, vw, _) = view;
        // ★ m257：x 的两端都按**视口原点**算。原来右端写成 `abs_x < vw`（相对宽当绝对上界），
        // 缩略图条贴左时 `vx=112`、`vw` 少 112，于是最右侧那一条（正是五颗标题钮所在）
        // 被判成「不在标题栏」——极简模式下悬停那里不浮现，可 `hit` 的按钮矩形是绝对的、
        // 点下去照样关窗（2026-11-08 用户实机报障）。底条时 `vx=0`，两者恰好相等，
        // 这个错从引入贴左起就藏着。
        abs_y >= 0.0 && abs_y < (STRIP_H + 8) as f32 && abs_x >= vx && abs_x < vx + vw
    }

    /// 指针活动（Enter/Move/Down 时调用）。更新各区域悬停状态。
    ///
    /// `pos` 是窗口绝对坐标；`view` 提供视口宽高用于工具栏/信息条定位。
    /// 标题栏直接用窗口绝对 Y 坐标判断，彻底解耦 layout 变化。
    ///
    /// 返回**这一位的翻转会不会改变画面**：`paint` 就是按 `hover_title/toolbar/info`
    /// 这三位决定那两条栏画不画，所以调用方必须据此标脏。
    /// ★ 2026-10-19 用户报"极简模式浮现不流畅、移开有时收不走"，根因就有这条——
    ///   返回值当年没接，浮现/收走只能等**别的**标脏源（跨过某颗按钮的高亮、2 秒定时器、
    ///   解出一帧图），于是慢半拍、时有时无。
    /// 传统模式下那三位不参与显隐，返回恒 `false`：报了就是白刷一整帧。
    /// 按钮级高亮（`hover_btn` 等）另有比较，不在这里。
    #[must_use = "true = 显隐位翻了、画面要变；吞掉就等于浮现慢半拍"]
    pub fn activity(&self, pos: (f32, f32), view: (f32, f32, f32, f32)) -> bool {
        self.last_move.set(Instant::now());
        self.last_pos.set(pos);
        // 同时留下"下这个结论时真实光标在哪"。读数拿不到就当没有锚点：
        // [`Chrome::tick_hide`] 缺锚点时不清悬停态，免得把"读失败"当成"光标飞走"。
        self.cursor_anchor
            .set(crate::platform::cursor::screen_pos());
        let (vx, vy, vw, vh) = view;
        let (abs_x, abs_y) = pos;
        // 标题栏：判据的单一来源见 [`Chrome::in_title_strip`]
        let title = Self::in_title_strip(pos, view);
        // 工具栏/信息条/箭头边带：仍需相对 view 定位（随窗口大小变化）
        let (x, y) = (abs_x - vx, abs_y - vy);
        let (px, py, pw, ph) = Chrome::pill(view, self.anim.get(), self.info_cell.get());
        let toolbar = x >= px && x < px + pw && y >= py && y < py + ph;
        // 信息条：跟着 draw 每帧回写的**真实**矩形走（窄窗整行档的宽是现量的，
        // 而且这帧可能根本没画——零矩形）。固定估宽（旧 MAX_W=200）与画出来的那块错位：
        // 文本短时 hover 比画的宽，文本长/窗口窄没画时 hover 还在亮。
        // 常态那一格在浮条里面，上面 `hover_toolbar` 已经覆盖它了。
        // 判据与"点它能不能开跳转窗"共用 [`Chrome::in_info_cell`]。
        let info = self.in_info_cell(pos);
        // 翻页箭头：该侧 [`ARROW_ZONE`] 边带内就浮现。用边带而不是箭头那个圆，是为了
        // "图形完全不画"的时候鼠标还能凭"靠边就出现"找到它；圆只管点击（`hit` 不变）。
        // 边带宽度与浮条让位用的是同一个常量，避免出现两套"靠边"的线。
        let inside = x >= 0.0 && y >= 0.0 && x < vw && y < vh;
        let edge = (inside && x < ARROW_ZONE, inside && x > vw - ARROW_ZONE);
        // ★ 每位分别 `replace` 之后再合并，不许写成一条 `||` 链：`||` 会短路，
        //   第一位变了后面几位就根本不覆写，状态从此与几何脱节（同一个坑见 [`Self::pointer_left`]）。
        let title_changed = self.hover_title.replace(title) != title;
        let toolbar_changed = self.hover_toolbar.replace(toolbar) != toolbar;
        let info_changed = self.hover_info.replace(info) != info;
        let edge_changed = self.hover_edge.replace(edge) != edge;
        self.minimal_mode.get()
            && (title_changed || toolbar_changed || info_changed || edge_changed)
    }

    /// 指针离开画布：把悬停位**全部**清掉，返回要不要重绘（调用方据此标脏）。
    ///
    /// ★ 逐位赋值之后再合并，不写成一条 `||` 链——这是当年真实存在的缺陷：`||` 短路，
    /// 只要第一位（`hover_btn`）变了，后面几位根本不会被覆写。后果正对上用户 2026-10-19
    /// 那句"鼠标移开后有时候没能及时隐藏"：**光标恰好停在某颗按钮或翻页箭头上离开时**，
    /// `hover_toolbar` 仍挂在 `true`，浮条就再也收不走；而 [`Self::tick_hide`] 读的是滞留的
    /// `last_pos`（还在那条栏里）⇒ 判成贴边豁免 ⇒ 连 2 秒那道兜底都不发生。
    /// 传统模式下显隐那三位不参与画面（照样要清，状态得跟几何一致），只是报重绘时不算它们。
    #[must_use = "true = 悬停态清掉了、画面要变；吞掉就是浮条收不走"]
    pub fn pointer_left(&self) -> bool {
        let btn = self.hover_btn.replace(None).is_some();
        let arrow = self.hover_arrow.replace(None).is_some();
        let title_btn = self.hover_title_btn.replace(None).is_some();
        // 按下后把指针拖出窗口、在外面松手：收不到 Up，按下位会永久挂着。
        // 后果不只是按钮一直像被按住——`canvas` 的平移判据要求 `pressed` 为空，
        // 于是 Ctrl/中键拖动也一起失灵，直到用户再点一下别处。
        let pressed = self.pressed.replace(None).is_some();
        let pressed_title = self.pressed_title_btn.replace(None).is_some();
        let title = self.hover_title.replace(false);
        let toolbar = self.hover_toolbar.replace(false);
        let info = self.hover_info.replace(false);
        let edge = self.hover_edge.replace((false, false));
        let visible_bits = title || toolbar || info || edge.0 || edge.1;
        btn || arrow
            || title_btn
            || pressed
            || pressed_title
            || (self.minimal_mode.get() && visible_bits)
    }

    /// 定时检查：静止够久且指针不在任何悬停区域 → 清除悬停状态。返回 true 表示有变化。
    /// 仅在极简模式下生效；传统模式不自动清除。
    /// `viewport` 是完整视口矩形 (vx, vy, vw, vh)。`last_pos` 存的是绝对坐标。
    ///
    /// `cursor` 是**本帧**读到的真实光标屏幕坐标（物理像素）。它是 `last_pos` 的哨兵：
    /// 与记 `last_pos` 时那个锚点差出 [`STALE_DRIFT_PX`] 以上，说明这条记录已经过期，
    /// 此时**不能**再按它给贴边/浮条豁免——那正是"浮条永远收不走"的成因（m55 / §10 ①）。
    /// 位置作参数注入而不是在这里直接调平台，为的是这条判据能被单测摆布（与
    /// `core::config` 的防抖时钟同一定式）。
    pub fn tick_hide(&self, viewport: (f32, f32, f32, f32), cursor: Option<(i32, i32)>) -> bool {
        // 传统模式：不自动清除悬停
        if !self.minimal_mode.get() {
            return false;
        }
        // 状态唤醒到期得**有人报这一帧**：`activity` 只在指针动时标脏，靠它兜就等于
        // 浮条到期后停在屏幕上不动。这一位不归指针管，所以在这里单独收。
        let expired = match self.status_until.get() {
            Some(t) if t <= Instant::now() => {
                self.status_until.set(None);
                true
            }
            _ => false,
        };
        // 过期判据走在静止计时之前：光标早就离开栏了，还要等 2 秒才收是纯粹的迟钝。
        if let (Some(now), Some(anchor)) = (cursor, self.cursor_anchor.get()) {
            if (now.0 - anchor.0).abs() + (now.1 - anchor.1).abs() > STALE_DRIFT_PX {
                return self.pointer_left() || expired;
            }
        }
        if self.last_move.get().elapsed() < IDLE_HIDE {
            return expired;
        }
        let (abs_x, abs_y) = self.last_pos.get();
        let (_, _, vw, vh) = viewport;
        // 标题栏：与 activity() 同一份判据（单一来源 [`Chrome::in_title_strip`]）
        let in_title = Self::in_title_strip(self.last_pos.get(), viewport);
        // 工具栏/信息条/边框：需相对坐标
        let (vx, vy, _, _) = viewport;
        let x = abs_x - vx;
        let y = abs_y - vy;
        let in_frame = x <= FRAME || x >= vw - FRAME || y <= FRAME || y >= vh - FRAME;
        let (px, py, pw, ph) = Chrome::pill(viewport, self.anim.get(), self.info_cell.get());
        let in_toolbar = x >= px && x < px + pw && y >= py && y < py + ph;
        // 信息条：与 hover 同源，读 draw 回写的真实矩形（零矩形 = 这帧没画，不算命中）
        let in_info = self.in_info_cell(self.last_pos.get());
        if in_title || in_toolbar || in_info || in_frame {
            return expired;
        }
        // 清除所有悬停状态
        if self.hover_title.get() || self.hover_toolbar.get() || self.hover_info.get() {
            self.hover_title.set(false);
            self.hover_toolbar.set(false);
            self.hover_info.set(false);
            return true;
        }
        expired
    }

    // ── 几何 ────────────────────────────────────────────────────

    /// 这一帧浮条上的按钮（顺序即槽位）：没有帧/页号的图不给播放组（判据
    /// `Anim::has_index()`，静态位图一律没有）；**窗口窄到放不下
    /// "满组 + 状态格"时也不给**——快捷键 `,` `.` `P` 一直有效，而把按钮挤出窗口是
    /// 既点不着又难看，状态格又比按钮金贵（用户 2026-10-18 拍板"并进浮条"）。
    ///
    /// ★ 只有**一组**：多页文档与动图共用这三颗（页与帧在宿主里是同一个号，
    ///   见 [`crate::core::pixels::Anim`]）。所以返回"前段 + 尾段"两个切片就够——
    ///   动画组是 [`Btn::ALL`] 末尾连续三颗，静态图只取 `..ANIM_START`。
    pub fn row(anim: bool, vw: f32) -> (&'static [Btn], Option<&'static [Btn]>) {
        // m245：隐藏按钮 = 这一行没有按钮。`pill` / `btn_rect` / `hit` / 鸟瞰避让
        // 全部经由本函数推导，所以一处收口就够，别在调用点各判一次。
        // m247：动画组是 `ALL` 末尾连续三颗，静态图直接截掉尾段；`Info` 撤走后
        // 不再有尾段按钮，第二个返回值恒为 `None`（保留接口给调用点链拼接）。
        if !toolbar_buttons() {
            return (&[], None);
        }
        if anim && vw >= FULL_ROW_MIN_W {
            (Btn::ALL.as_slice(), None)
        } else {
            (&Btn::ALL[..ANIM_START], None)
        }
    }

    /// 当前这一行的颗数。
    fn row_len(anim: bool, vw: f32) -> usize {
        let (head, tail) = Self::row(anim, vw);
        head.len() + tail.map_or(0, <[Btn]>::len)
    }

    /// **满格**预留宽：给"必须稳定、不许跟着文本跳"的邻居用（鸟瞰缩略图避让）。
    ///
    /// 格宽自适应之后，按当前文本让位会让缩略图随每次翻页挪一格；这里按上限留，
    /// 一次留够，之后不动。
    pub(crate) fn cell_reserve(vw: f32) -> f32 {
        match info_place(vw, toolbar_buttons()) {
            InfoPlace::Pill => INFO_CELL_MAX,
            InfoPlace::Above => 0.0,
        }
    }

    /// 工具栏浮条的矩形（视口内、底部居中）。
    ///
    /// 公用到 **canvas**：右下角的鸟瞰缩略图要躲开它（窄窗时两者会叠在一起）。
    /// 宽度按**当前这一行有几颗 + 状态格这一帧多宽**推导——此前写死 6 个按钮，加旋转/镜像后浮条没跟着
    /// 变宽，最后两个按钮悬在背景外面（用户报障）。动画组按图种显隐之后，"颗数"不再
    /// 等于 `Btn::ALL.len()`，所以必须由调用方把 `anim` 传进来。
    ///
    /// `cell` 由**上一帧画出来的那份**提供（[`Chrome::info_cell`]）：格宽跟着文本伸缩，
    /// 命中与绘制必须同一份几何，不能在这里重新量一次。
    pub(crate) fn pill(view: (f32, f32, f32, f32), anim: bool, cell: f32) -> (f32, f32, f32, f32) {
        let (_, _, vw, vh) = view;
        let n = Self::row_len(anim, vw) as f32;
        // n=0（m245 隐藏按钮）时没有「按钮之间的间隙」，负 gap 会把胶囊算窄一格。
        let gap = if n > 0.0 { (n - 1.0) * BTN_GAP } else { 0.0 };
        let w = cell + 8.0 * 2.0 + n * BTN_W + gap;
        let h = 40.0;
        ((vw - w) / 2.0, vh - h - 12.0, w, h)
    }

    /// 某按钮在视口内的矩形；`None` = 这一帧它根本不该出现（播放组隐藏中）。
    pub fn btn_rect(
        b: Btn,
        view: (f32, f32, f32, f32),
        anim: bool,
        cell: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        let (px, py, _, ph) = Self::pill(view, anim, cell);
        let (_, vw, _) = (px, view.2, view.3);
        let (head, tail) = Self::row(anim, vw);
        let i =
            head.iter().position(|&x| x == b).or_else(|| {
                tail.and_then(|s| s.iter().position(|&x| x == b).map(|p| p + head.len()))
            })? as f32;
        Some((
            px + cell + 8.0 + i * (BTN_W + BTN_GAP),
            py + (ph - BTN_H) / 2.0,
            BTN_W,
            BTN_H,
        ))
    }

    /// 箭头的圆心。
    fn arrow_center(a: Arrow, view: (f32, f32, f32, f32)) -> (f32, f32) {
        let (_, _, vw, vh) = view;
        let x = match a {
            Arrow::Prev => 16.0 + ARROW_R,
            Arrow::Next => vw - 16.0 - ARROW_R,
        };
        (x, vh / 2.0)
    }

    /// 标题栏按钮矩形（视口右上角）。
    fn title_btn_rect(tb: TitleBtn, view: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let (_, _, vw, _) = view;
        const BTN_W: f32 = 40.0;
        const BTN_H: f32 = STRIP_H as f32;
        let i = TitleBtn::ALL.iter().position(|&x| x == tb).unwrap_or(0) as f32;
        let x = vw - (i + 1.0) * (BTN_W + 0.0);
        (x, 0.0, BTN_W, BTN_H)
    }

    /// 指针落在状态格（`12/340`）里吗？
    ///
    /// 读 `paint` 每帧回写的**真实**矩形：零矩形 = 这帧根本没画（极简模式收起 /
    /// 无边框形态 / 没有状态文本），天然不可点也不可悬停，调用点不用各判一次。
    ///
    /// ★ 存的是**画布绝对坐标**（`paint` 里经 [`to_canvas`] 换算后才回写），所以这里
    ///   直接拿窗口绝对坐标比、**不再减视口原点**。贴左缩略图条时 `vx=112`，减一次就
    ///   整块左移一条——这正是 m255 那族"画在哪点不到哪"的老毛病，底条（`vx=0`）看不出来。
    pub fn in_info_cell(&self, pos: (f32, f32)) -> bool {
        let (ix, iy, iw, ih) = self.info_rect.get();
        iw > 0.0 && pos.0 >= ix && pos.0 < ix + iw && pos.1 >= iy && pos.1 < iy + ih
    }

    /// 命中测试：视口内某点落在哪个按钮 / 箭头 / 标题栏按钮上。
    pub fn hit(&self, pos: (f32, f32), view: (f32, f32, f32, f32)) -> Option<Press> {
        // 调用方给的是窗口绝对坐标；下面所有几何（title_btn_rect/btn_rect/arrow_center）
        // 都是视口相对坐标。先减一次原点，命中才和绘制同一空间。
        let pos = (pos.0 - view.0, pos.1 - view.1);
        // 锁定：整窗只认锁住按钮（解锁唯一入口）——其余命中一律 None，
        // 拖窗/工具栏/箭头自然全断（用户拍板 2026-10-12，spec §5）。
        if self.locked.get() {
            let (x, y, w, h) = Self::title_btn_rect(TitleBtn::Lock, view);
            return if pos.0 >= x && pos.0 < x + w && pos.1 >= y && pos.1 < y + h {
                Some(Press::TitleBtn(TitleBtn::Lock))
            } else {
                None
            };
        }
        // m269：无边框形态没有自绘 chrome，热区也必须一起撤 —— 否则「画没了
        // 还点得着」。画布手势（切图 / 缩放 / 拖动 / 右键）不经过这里，全部照常。
        if borderless() {
            return None;
        }
        // 命中顺序必须跟**绘制顺序相反**（后画的在上）：浮条 → 箭头 → 标题栏。
        // 正常尺寸三者不重叠；窗口压到 100–119 高时箭头圆会压到标题按钮上，
        // 旧顺序（标题优先）会让“点箭头却关窗”。
        let anim = self.anim.get();
        let cell = self.info_cell.get();
        let (head, tail) = Chrome::row(anim, view.2);
        for b in head.iter().chain(tail.unwrap_or(&[])).copied() {
            let Some((x, y, w, h)) = Chrome::btn_rect(b, view, anim, cell) else {
                continue;
            };
            if pos.0 >= x && pos.0 < x + w && pos.1 >= y && pos.1 < y + h {
                return Some(Press::Btn(b));
            }
        }
        for a in [Arrow::Prev, Arrow::Next] {
            let (cx, cy) = Self::arrow_center(a, view);
            let dx = pos.0 - cx;
            let dy = pos.1 - cy;
            if dx * dx + dy * dy <= (ARROW_R + 4.0) * (ARROW_R + 4.0) {
                return Some(Press::Arrow(a));
            }
        }
        // 标题栏按钮最后测（最右上角，按 Windows 标准顺序从右向左）。
        for &tb in TitleBtn::ALL.iter().rev() {
            let (x, y, w, h) = Self::title_btn_rect(tb, view);
            if pos.0 >= x && pos.0 < x + w && pos.1 >= y && pos.1 < y + h {
                return Some(Press::TitleBtn(tb));
            }
        }
        None
    }
}

// 按钮尺寸常量（逻辑 dp）。7 键时浮条总宽 7×34 + 6×3 + 2×8 = 272，
// 最小窗口（360）内放得下；`BTN_W` 定 34 就是为这条，再宽会顶破 360。
/// 满组 10 颗 + 2 颗余量（与旧 14 颗同比例）：`8×2 + 12×BTN_W + 11×BTN_GAP`。
/// 余量是刻意的：门槛宁可宽一点，也不让动画组在放不下的窗口里挤掉信息格。
const FULL_ROW_W: f32 = 8.0 * 2.0 + 12.0 * 34.0 + 11.0 * 3.0;
/// 7 颗（收掉末尾动画组）时的浮条宽——状态格的门槛照它算。
const NARROW_ROW_W: f32 = 8.0 * 2.0 + 7.0 * 34.0 + 6.0 * 3.0;
/// 动画组在 `Btn::ALL` 里的起点（末尾连续三颗）。
const ANIM_START: usize = 7;
const BTN_W: f32 = 34.0;
const BTN_H: f32 = 28.0;
const BTN_GAP: f32 = 3.0;
const ARROW_R: f32 = 20.0;
/// 放映进度分成几格 = **圆盘的直径像素数**（[`Chrome::slide_step`] 的返回值域）。
///
/// 一格一列像素是**能画出的最细粒度**：d2d 后端的 `clip_rect` 走
/// `PushAxisAlignedClip` + `ALIASED`（整数矩形遮罩，见 `.3rd` 里那段注释），根本没有半像素
/// 可插，所以"平滑"的上限就是 40 列。用户 2026-10-31 嫌 8 格僵硬，那是我自己加的节流，
/// 不是技术限制——去掉它。
/// ★ 量化本身仍然要留着（重绘只在换格那一帧）：整扇窗是逐像素分层的，跟着 16 ms 轮询
///   每帧刷就是每秒约 60 次全窗重合成。按像素量化把次数上限钉在**几何**上而不是**帧钟**上：
///   默认 3 秒间隔 = 40 次 / 3 秒 ≈ 13 次/秒（8 格时是 2.7 次，每帧刷是 60 次）。
const SLIDE_STEPS: u32 = (ARROW_R * 2.0) as u32;
/// 左右箭头各从视口边沿吃掉的宽：16 边距 + 直径 40 + 8 呼吸 = 64。
///
/// 画布中央那句错误提示要照它留边（`ui::canvas`）：错误不再同时出现在信息条里，
/// 中央那一处就是**唯一**载体，被箭头压住半截等于没有（2026-10-18 504 宽实测
/// "…已损坏： PNG 库报错…"两头都骑在箭头上）。
///
/// 同一个宽度还兼任**极简模式下箭头的浮现边带**（[`Chrome::hover_edge`]）：让"浮条避让的线"
/// 和"箭头现身靠的边"是同一根，避免出现两套"靠边"各算各的。
pub const ARROW_ZONE: f32 = 16.0 + ARROW_R * 2.0 + 8.0;

/// 标题栏左内边距（文件名从这里起）。
const TITLE_PAD: f32 = 12.0;
/// 标题栏右侧让给窗口按钮的宽：5 × 40 = 200，再加 8 留白 = 208。
/// （旧写法把 `vw - 220` 这个魔数直接写在绘制处，等价于 `TITLE_PAD + 208`。）
const TITLE_BTN_RESERVE: f32 = 208.0;

/// 信息条这一帧落在哪儿（判据见 [`info_place`]）。
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub(crate) enum InfoPlace {
    /// 浮条左端那一格（常态位：状态是 chrome 的信息，跟着 chrome 走）。
    Pill,
    /// 浮条上方整行（只有浮条塞不下这一格时才用——窄窗口）。
    Above,
}

/// 状态文本框**上限** = 212：唯一的实测输入。
///
/// 为什么必须有上限：`draw_text` 的 rect 宽**同时是折行宽**（软硬两后端都把 `rect.w`
/// 交给排版器），文本比框宽就在那 40 px 高的浮条里长出第二行——不报错、不截断，只是把
/// 整条胶囊撑坏。212 覆盖**现实最坏** `1234/1234 · 4000×3000 · 载入中…`（实测 209 px，
/// 同批帧号形态 207）；再往上的病态输入（一个目录 12345 张 → 232、一张图 12345 帧 → 237）
/// 走 `clip_text` 收成省略号，宁可少看一位数也不为它多付浮条宽。
/// 改状态文案格式就重跑 `examples/text_measure.rs`；这条数有单测拿真实排版守着
/// （`tests::the_status_cell_actually_fits_the_worst_string`），不许只活在注释里。
const INFO_TEXT_MAX: f32 = 212.0;
/// 文本左内边距（浮条自身那 8 px 胶囊边距之外再留的）。
const INFO_PAD_L: f32 = 8.0;
/// 文本 → 分隔线 → 第一颗按钮 两侧的呼吸。
const INFO_GAP_L: f32 = 6.0;
const INFO_GAP_R: f32 = 6.0;
/// 一格 = 胶囊左内边距 8 + 文本左留白 + 文本 + 呼吸 + 1 px 分隔线 + 呼吸。
const fn info_cell_w(text_w: f32) -> f32 {
    8.0 + INFO_PAD_L + text_w + INFO_GAP_L + 1.0 + INFO_GAP_R
}
/// 满格宽（文本顶到上限时）——**档位判据只看它**，见 [`info_place`]。
const INFO_CELL_MAX: f32 = info_cell_w(INFO_TEXT_MAX);
/// 整行档与视口边缘、与浮条之间的间距。
const INFO_GAP: f32 = 8.0;
/// 状态进浮条所需的最小视口宽：9 颗按钮 + **满格**。窄于此退到浮条上方整行。
const PILL_INFO_MIN_W: f32 = NARROW_ROW_W + INFO_CELL_MAX;
/// 动画三颗与状态格**同时**在场所需宽：12 颗 + 满格。
///
/// 状态优先于动画按钮：587–698 这一段窗口里动图少三颗按钮（`,` `.` `P` 与右键菜单
/// 照旧可用），换来的是状态不必另起一行压在画上——用户 2026-10-18 报的就是那一行。
const FULL_ROW_MIN_W: f32 = FULL_ROW_W + INFO_CELL_MAX;

/// 信息条落位判据（纯算术，单测直接驱动它，不必造画布）。
///
/// 状态**只有一个常态位**：浮条左端那一格（用户 2026-10-18 拍板"并进浮条"）。
/// 格宽**跟着文本伸缩**（用户 2026-10-18 复看：定宽在信息右侧留下一大片空白），
/// 但**档位只按满格判**——也就是"能不能放下最坏那条"，不看这一帧实际多长。
/// 为什么：档位若按实际宽判，暂停动图（82→153 px）就会让信息从浮条里翻到上面一行，
/// 那正是用户报过的"上升一行"。**横向滑动是自适应的固有代价，纵向跳档不是。**
/// 整行档仍保留：窄窗里状态没地方放 = 帧号彻底看不见，比占一行更糟。
fn info_place(vw: f32, buttons: bool) -> InfoPlace {
    // m245：按钮一隐，浮条里再没有别的东西跟信息争宽 ⇒ 任何窗宽都留在浮条里，
    // 由 paint 用 Align::Center 居中；Above 那行只是「有按钮 + 窄窗」的退路。
    if !buttons || vw >= PILL_INFO_MIN_W {
        InfoPlace::Pill
    } else {
        InfoPlace::Above
    }
}

/// 单行截断：放不下就二分保留能塞进 `avail_w` 的前缀并加省略号。
///
/// 为什么宿主自己算：windui 的截断住在 `Label` 控件里（`ui/text_fit.rs`，`pub(crate)`），
/// 而标题栏是 chrome **直绘**的（`ui/window.rs` 里那份控件版仅作存档）。直绘没有截断
/// 能力，实测窄窗里长文件名会**折行画到图片上**——第二行压在画面中央，还与窗口按钮
/// 叠在一起（2026-10-18 `PrintWindow` 取证）。二分依赖"保留越多越宽"的单调性，
/// 对横排文字成立；放不下任何前缀时退化成只有一个 `…`。
fn clip_text(canvas: &mut dyn Canvas, text: &str, ts: &TextStyle, avail_w: f32) -> String {
    if avail_w <= 0.0 {
        return String::new();
    }
    if (canvas.measure_text(text, ts).w as f32) <= avail_w {
        return text.to_string();
    }
    const ELL: &str = "…";
    if (canvas.measure_text(ELL, ts).w as f32) > avail_w {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let cand: String = chars[..mid].iter().collect();
        let fits = (canvas.measure_text(&format!("{cand}{ELL}"), ts).w as f32) <= avail_w;
        if fits {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let kept: String = chars[..lo].iter().collect();
    format!("{kept}{ELL}")
}

/// 顶条（自绘标题栏 + 5 颗窗口按钮）这一帧画不画。**m238**。
///
/// ★ 全屏时**默认不画**——用户口径「全屏不该还留着标题栏」。但顶缘悬停仍能把它
///   唤出：与极简模式共用同一份 `hover_title`，而那位由 `in_title_strip` 的几何
///   独立算出，不看这一帧画没画，所以不会「收了就唤不回」。全屏下 5 颗窗口按钮
///   （置顶 / 锁定 / 最小化 / 最大化 / 关闭）因此仍然可达。
/// ★ 非全屏的判据一字未动：传统模式常驻，极简模式只靠悬停。
fn title_visible(fullscreen_on: bool, minimal_mode: bool, hover_title: bool) -> bool {
    if fullscreen_on {
        hover_title
    } else {
        !minimal_mode || hover_title
    }
}

/// 这一帧标题栏实际占掉的高度（逻辑像素）；不画时为 0。**m290**。
///
/// 与 [`paint`] 里那句 `show_title` 是**同一条判据**——两处必须同源，`paint` 也走这个
/// 函数。分开写就会重演 m255 / m275 那族坐标错位：平移上限按一个高度算、标题栏按另一个
/// 高度画，于是衣角要么被吃掉、要么多留一截。
///
/// 唯一消费方是 `ui::canvas`，喂给 [`Viewer::set_chrome_top`](crate::core::viewer::Viewer)。
pub fn title_strip_h(fullscreen_on: bool, minimal_mode: bool, chrome: &Chrome) -> f32 {
    let show =
        !borderless() && title_visible(fullscreen_on, minimal_mode, chrome.hover_title.get());
    if show {
        STRIP_H as f32
    } else {
        0.0
    }
}

/// 视口**相对**坐标 → 画布坐标（= 窗口绝对坐标）。
///
/// ★ 与 [`Chrome::hit`] 里的 `pos - view.0` **互为逆运算**：画在哪、点哪就必须命中哪。
///   底条时 `vx/vy` 恒为 0，这条恒等式怎么写都看不出来；贴左（m253）之后 `vx=112`，
///   方向写反就是整块界面平移一条、而命中仍在原位（2026-11-08 用户报障）。
fn to_canvas(view: (f32, f32, f32, f32), rel: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (vx, vy, _, _) = view;
    (rel.0 + vx, rel.1 + vy, rel.2, rel.3)
}

/// 画悬浮层：箭头 + 工具栏 + 信息小条。
///
/// `view` 是画布摆图/悬浮层共用的视口矩形 (x, y, w, h)；`info` 是右侧信息文本（"3/12 · 100%"）；
/// `has_prev/has_next` 控制箭头是否画；其余是开关态。
/// `minimal_mode` 为 true 时：各元素仅在悬停时显示；false 时：始终显示。
#[allow(clippy::too_many_arguments)] // 11 个参数：绘制需要的开关态，合并成结构体反而更难读
pub fn paint(
    canvas: &mut dyn Canvas,
    view: (f32, f32, f32, f32),
    chrome: &Chrome,
    info: &str,
    has_prev: bool,
    has_next: bool,
    fullscreen_on: bool,
    topmost_on: bool,
    maximized_on: bool,
    minimal_mode: bool,
    title: &str,
) {
    let (vx, vy, vw, _vh) = view;
    // 视口**相对**坐标 → 画布坐标。下面所有几何（title_btn_rect / btn_rect /
    // arrow_center / pill / above）都是相对视口原点的，这里统一搬一次。
    let local = |x: f32, y: f32, w: f32, h: f32| to_canvas(view, (x, y, w, h));
    let small = TextStyle::new(12.0);
    // 图标字形：指定图标字体族，PUA 码位才会落在这个字体里（默认字体是方块）。
    let icon_ts = TextStyle {
        family: Some(ICON_FONT),
        ..TextStyle::new(17.0)
    };
    // 计算各元素是否显示
    // m269/m271：无边框形态把**所有** chrome 都收掉——顶条、底部浮条（按钮与信息一起）、
    // 缩略图条、底色。留下的是画面本身和全部操作入口（右键 / 快捷键 / 拖动 / 缩放），
    // 这正是它和「空格预览」的分界：那边连操作都没有。
    let bl = borderless();
    // m290：判据收进 `title_strip_h`，与"给平移上限算标题栏挡了多少"共用同一份。
    // 两处各写一遍就会错位——衣角要么被吃掉、要么多留一截。
    let show_title = title_strip_h(fullscreen_on, minimal_mode, chrome) > 0.0;
    // 悬停信息条也算"在浮条上"：整行档（窄窗）就住在那儿。不认它的后果是极简模式下
    // 指针停在状态那一行 2 秒后整组消失、且再也悬不回来（`info_rect` 只有画出来才非零）。
    // 三条来源取或：常驻（传统模式）／指针在浮条上（`activity` 现算）／状态刚变
    // （`wake_on_status_change` 自己的到期时刻，指针通道清不掉它）。
    let show_toolbar = !bl
        && (!minimal_mode
            || chrome.hover_toolbar.get()
            || chrome.hover_info.get()
            || chrome.status_alive());
    // 状态跟着浮条走（用户 2026-10-18 拍板"并进浮条"）：浮条在它就画，浮条收它就收。
    // 不再单独跟着标题栏显示——那一档已经取消。
    let show_info = show_toolbar && !info.is_empty();

    // ── 浮条几何 + 信息条落位 ──
    //
    // **档位**只看视口宽（[`info_place`]，按满格判），**格宽**跟着这一帧的文本伸缩
    // （用户 2026-10-18 复看：定宽在信息右侧留下一大片空白）。两者分开，纵向就不会跳。
    // 量宽与绘制用**同一份 style**（旧写法量 12 号、画 13 号，胶囊比字窄一圈）。
    let info_ts = TextStyle::new(13.0);
    let buttons = toolbar_buttons(); // m245：隐藏后信息独占浮条并水平居中
    let place = info_place(vw, buttons);
    let title_room = vw - TITLE_BTN_RESERVE - TITLE_PAD;
    // 每帧先清零再按需回写：hover / tick_hide 读到的永远是这帧**实际画出来**的那块
    // （没画 = 零矩形 = 不可悬停）。
    chrome.info_rect.set((0.0, 0.0, 0.0, 0.0));
    // 一次测量喂两档：格子要按它定宽，整行档要按它定胶囊宽。
    // ★ 不看 `show_info`：几何必须与"这帧画没画"无关，否则极简模式下浮条一亮起来
    //   整排按钮就挪位（隐藏时 cell=0、显示时 cell=110 → 每次悬停都抖一下）。
    // 量宽走 `Chrome::measure_w`：同一串字同一 DPI 每帧只排一次（见 [`TextMemo`]）。
    let text_w = chrome.measure_w(canvas, info, &info_ts);
    let info_w = text_w + 16.0; // 整行档那粒胶囊的宽（左右各 8）
    let cell = if place == InfoPlace::Pill && text_w > 0.0 {
        info_cell_w(text_w.min(INFO_TEXT_MAX))
    } else {
        0.0
    };
    chrome.info_cell.set(cell);
    let (px, py, pw, ph) = Chrome::pill(view, chrome.anim.get(), cell);
    // 整行档（窄窗）的矩形就地定下：浮条上的悬停提示要照它让位，否则两条叠在一起。
    const INFO_H: f32 = 40.0;
    let above = if place == InfoPlace::Above && show_info {
        let w = info_w.min(vw - 2.0 * INFO_GAP);
        // m245：常态贴左是给按钮让位；按钮一隐，整行档的信息胶囊也水平居中。
        // ★ 这里是**视口相对**坐标（整块 `above` 后面统一过 `local`），`iy` 也一样——
        //   旧写法把 `vy` 先加进 `iy`，`local` 又加一次，成了 `2*vy`（底条 `vy=0`
        //   看不出来，与 m255 那族是同一个错）。
        let ix = if buttons { INFO_GAP } else { (vw - w) / 2.0 };
        let iy = py - INFO_H - INFO_GAP;
        // 回写的必须是**画布绝对坐标**（`in_info_cell` 拿绝对坐标比）。
        chrome.info_rect.set(to_canvas(view, (ix, iy, w, INFO_H)));
        Some((ix, iy, w))
    } else {
        None
    };

    // ── 标题栏（顶部拖动区 + 标题 + 窗口按钮）──
    if show_title {
        // 标题栏背景
        canvas.fill_rect(vx, vy, vw, STRIP_H as f32, &Paint::fill(tk::pill_bg()));
        let title_ts = TextStyle::new(13.0);
        // 标题文本：左对齐，右侧让出 5 颗窗口按钮。**必须截断**：直绘的 `draw_text`
        // 不截断，长文件名会折行画到图片上，第二行压在画面中央（`clip_text` 的注释里
        // 记着取证）。状态已经搬进浮条，这里不再为它让宽。
        let shown = chrome.clip(canvas, title, &title_ts, title_room);
        canvas.draw_text(
            &shown,
            Rect::new(
                (vx + TITLE_PAD) as i32,
                vy as i32,
                title_room as i32,
                STRIP_H,
            ),
            tk::overlay_text(),
            Align::Start,
            &title_ts,
        );
        // 窗口按钮（📌 🔒 最小化 最大化 ×），按 Windows 标准顺序从右向左绘制
        let locked = chrome.locked.get();
        for &tb in TitleBtn::ALL.iter().rev() {
            let (bx, by, bw, bh) = Chrome::title_btn_rect(tb, view);
            let (lx, ly, lw, lh) = local(bx, by, bw, bh);
            let hovered = chrome.hover_title_btn.get() == Some(tb);
            let pressed = chrome.pressed_title_btn.get() == Some(tb);
            // 锁定时除锁钮外全是"禁用"：不画 hover/pressed 底。
            let active = !locked || tb == TitleBtn::Lock;
            if active && (hovered || pressed) {
                let c = if pressed {
                    tk::btn_press()
                } else if tb == TitleBtn::Close {
                    // 关闭按钮悬停时红色背景
                    tk::BTN_DANGER_HOVER
                } else {
                    tk::btn_hover()
                };
                canvas.fill_rect(lx, ly, lw, lh, &Paint::fill(c));
            }
            let fg = match tb {
                // 锁住态：绿色高亮（同 Checker/Fullscreen/置顶 激活模式）。
                TitleBtn::Lock if locked => tk::overlay_text_active(),
                _ if !active => tk::overlay_text_muted(),
                _ => tk::overlay_text(),
            };
            let (tx, ty, tw, th) = local(bx, by, bw, bh);
            if tb == TitleBtn::Topmost {
                // 置顶：📌 字形离屏预渲染位图（ui::pin，CurSeen 同款姿态语义）——
                // 未置顶斜躺（字形原生姿态），已置顶立正 + 绿。
                let mut pins = chrome.pins.borrow_mut();
                if pins.is_none() {
                    *pins = super::pin::render_pins(tk::overlay_text(), tk::overlay_text_active());
                }
                if let Some((tilted, upright)) = pins.as_ref() {
                    let img = if topmost_on { upright } else { tilted };
                    let d = 26.0;
                    canvas.draw_image(
                        img,
                        Rect::new(
                            (tx + (tw - d) / 2.0) as i32,
                            (ty + (th - d) / 2.0) as i32,
                            d as i32,
                            d as i32,
                        ),
                        Fit::Contain,
                        0.0,
                        1.0,
                    );
                }
                continue;
            }
            let family = tb.font_family().unwrap_or(ICON_FONT);
            canvas.draw_text(
                tb.glyph(locked, maximized_on),
                Rect::new(tx as i32, ty as i32, tw as i32, th as i32),
                fg,
                Align::Center,
                &TextStyle {
                    family: Some(family),
                    ..TextStyle::new(14.0)
                },
            );
        }
    }

    // ── 翻页箭头 ──
    //
    // 传统模式常驻；极简模式下按 [`Chrome::hover_edge`] 那两位浮现（指针落在该侧边带里才画）。
    // ★ 隐藏只是**不画**，[`Chrome::hit`] 的圆一寸没缩——鼠标用户靠"边上有东西会出来"找到它，
    //   而不是靠猜坐标。2026-10-09 那次拍板（"箭头永远存在"）由用户自己于 2026-10-19 翻案：
    //   既然要极简就追求到底，但悬停时还是要出现。
    let (edge_prev, edge_next) = chrome.hover_edge.get();
    for (a, has, glyph) in [(Arrow::Prev, has_prev, "‹"), (Arrow::Next, has_next, "›")] {
        let revealed = match a {
            Arrow::Prev => edge_prev,
            Arrow::Next => edge_next,
        };
        // 放映中：右侧那颗「下一张」**持续亮起并强制可见**（用户 2026-10-31 点名的形状，
        // 替代文字提示）。它本身就是"下一张"的按钮，亮着读起来就是"自动在按下一张"。
        // ★ 极简模式本来把箭头收成"悬停才出现"，这里给放映开一个例外：需要被记住的状态
        //   不许放进会自动消失的浮层（§八.7 同一条判据）——隐藏等于没有指示。
        //   热区一寸没动（[`Chrome::hit`] 与绘制同源），变的只是这一颗画不画、用哪档底色。
        let sliding = chrome.slideshow_on() && matches!(a, Arrow::Next);
        if !has || bl || (minimal_mode && !(revealed || sliding)) {
            continue;
        }
        let (cx, cy) = Chrome::arrow_center(a, view);
        let hovered = chrome.hover_arrow.get() == Some(a);
        let pressed = chrome.pressed.get() == Some(Press::Arrow(a));
        // 常态底只有三档（按下 / 悬停 / 常态）。放映**不占一档**——它是叠在上面的进度层，
        // 所以鼠标经过正在推进的那颗时，底下照样是悬停档，读得出"这颗我可以点"。
        let bg = if pressed {
            tk::arrow_press()
        } else if hovered {
            tk::arrow_hover()
        } else {
            tk::pill_bg()
        };
        let (x, y, w, h) = local(cx - ARROW_R, cy - ARROW_R, ARROW_R * 2.0, ARROW_R * 2.0);
        canvas.fill_circle(x + ARROW_R, y + ARROW_R, ARROW_R, &Paint::fill(bg));
        // ★ 放映进度：**整颗底自左向右**被进度色推满，推满即翻页（用户 2026-10-31 定的形状，
        //   不是我第一版那样在里面加一根条）。要"圆的左截"必须 `save → clip_rect → 画 → restore`
        //   成对：这块画布整段浮条共用，漏一次 restore 裁剪就泄漏出去把后面的东西一起裁掉。
        //   所以这是全文件唯一成对裁剪的地方，四行写死在原地、不抽 helper——抽出去就看不出配对了。
        if sliding {
            let step = chrome.slide_step();
            if step > 0 {
                // 向上取整：满格那一帧的 clip 必须盖住整颗，差一列会在右缘留一道常态底。
                let ww = wipe_width(step).ceil() as i32;
                canvas.save();
                canvas.clip_rect(Rect::new(x as i32, y as i32, ww, h as i32));
                canvas.fill_circle(
                    x + ARROW_R,
                    y + ARROW_R,
                    ARROW_R,
                    &Paint::fill(tk::SLIDING_BG),
                );
                canvas.restore();
            }
        }
        // 字形**永远最后画** ⇒ 进度层遮不到箭头（用户特别点了这一条）。
        let ts = TextStyle::new(22.0);
        canvas.draw_text(
            glyph,
            Rect::new((x) as i32, (y) as i32, w as i32, h as i32),
            tk::overlay_text(),
            Align::Center,
            &ts,
        );
    }

    // ── 工具栏浮条 + 悬停提示 ──
    //
    // 浮条与上面的翻页箭头现在是同一条门控（极简 = 只在悬停时出现，2026-10-19 用户翻案）。
    // 浮条矩形 `px/py/pw/ph` 已在前面"浮条几何 + 信息条落位"那段算出（状态格占不占位
    // 参与总宽，两处用的是同一个 `Chrome::pill`）。
    // m245：按钮全隐且信息另起一行时，浮条里什么都没有 ⇒ 不画这粒空胶囊。
    if show_toolbar && (buttons || place == InfoPlace::Pill) {
        let (lx, ly, lw, lh) = local(px, py, pw, ph);
        canvas.fill_round_rect(lx, ly, lw, lh, 10.0, &Paint::fill(tk::pill_bg()));

        // ── 状态格：浮条左端那段定宽位 ──
        //
        // 格宽 = 这一帧文本的实际宽（上限 [`INFO_TEXT_MAX`]），信息右边不留空白；
        // 分隔线贴着格子右端，所以它跟着文本走，按钮再往右。
        // 文本框按上限夹住 → 常态既不截断也不折行；只有病态输入（一个目录上万张 /
        // 一张图上万帧）才会被 `clip_text` 收成省略号，那也比把胶囊撑出第二行好。
        if show_info && place == InfoPlace::Pill {
            if buttons {
                let tw = cell - 8.0 - INFO_PAD_L - INFO_GAP_L - 1.0 - INFO_GAP_R;
                let tx = px + 8.0 + INFO_PAD_L;
                let shown = chrome.clip(canvas, info, &info_ts, tw);
                let (lx, ly, lw, lh) = local(tx, py, tw, ph);
                canvas.draw_text(
                    &shown,
                    Rect::new(lx as i32, ly as i32, lw as i32, lh as i32),
                    tk::overlay_text_muted(),
                    Align::Start,
                    &info_ts,
                );
                let (sx, sy, sw, sh) = local(tx + tw + INFO_GAP_L, py + 10.0, 1.0, ph - 20.0);
                canvas.fill_rect(sx, sy, sw, sh, &Paint::fill(tk::pill_sep()));
                // 绝对坐标（见 [`Chrome::in_info_cell`]）：点状态格开跳转窗靠它。
                chrome.info_rect.set(local(tx, py, tw, ph));
            } else {
                // m245：只剩信息 ⇒ 整条浮条的宽度都给它，文字水平居中，分隔线不存在。
                // 文本框 = 浮条内缩 8，Align::Center 使文本中心恰为浮条（= 窗口）中心。
                let inner_w = pw - 16.0;
                let shown = chrome.clip(canvas, info, &info_ts, inner_w);
                let (lx, ly, lw, lh) = local(px + 8.0, py, inner_w, ph);
                canvas.draw_text(
                    &shown,
                    Rect::new(lx as i32, ly as i32, lw as i32, lh as i32),
                    tk::overlay_text_muted(),
                    Align::Center,
                    &info_ts,
                );
                chrome.info_rect.set(local(px, py, pw, ph));
            }
        }

        let anim = chrome.anim.get();
        let paused = chrome.paused.get();
        let (head, tail) = Chrome::row(anim, vw);
        for b in head.iter().chain(tail.unwrap_or(&[])).copied() {
            let Some((bx, by, bw, bh)) = Chrome::btn_rect(b, view, anim, cell) else {
                continue;
            };
            let hovered = chrome.hover_btn.get() == Some(b);
            let pressed = chrome.pressed.get() == Some(Press::Btn(b));
            if hovered || pressed {
                let (hx, hy, hw, hh) = local(bx, by, bw, bh);
                let c = if pressed {
                    tk::btn_press()
                } else {
                    tk::btn_hover()
                };
                canvas.fill_round_rect(hx, hy, hw, hh, 6.0, &Paint::fill(c));
            }
            let glyph = b.glyph(paused);
            // 开关类按钮激活时给一点绿色，扫一眼能看出状态（m247 后只剩全屏）。
            let color = if b == Btn::Fullscreen && fullscreen_on {
                tk::overlay_text_active()
            } else {
                tk::overlay_text()
            };
            let (tx, ty, tw, th) = local(bx, by, bw, bh);
            canvas.draw_text(
                glyph,
                Rect::new(tx as i32, ty as i32, tw as i32, th as i32),
                color,
                Align::Center,
                &icon_ts,
            );
        }

        // ── 悬停提示小条：图标按钮的"tooltip"，浮在工具栏上方 ──
        // 悬停中的那颗若正好被隐藏（切图后动画组收掉），拿不到矩形就不画提示条。
        let tip = chrome
            .hover_btn
            .get()
            .and_then(|b| Chrome::btn_rect(b, view, chrome.anim.get(), cell).map(|r| (b, r)));
        if let Some((b, (bx, _by, bw, _))) = tip {
            let text = b.name();
            let w = chrome.measure_w(canvas, text, &small) + 16.0;
            let h = 24.0;
            // 以按钮为中心，横向夹在视口内；顶上放不下就并入按钮内（不画）。
            // `bx` 是**视口相对**坐标（`btn_rect` 的返回值），夹紧边界同样要相对——
            // 混进 `vx` 的话，贴左时提示条会被推到右边去（与 2026-11-08 同一类错）。
            let tx = (bx + (bw - w) / 2.0).max(4.0).min(vw - w - 4.0);
            // 窄窗那一档，状态整行就住在浮条上方；提示条再往上让一层，否则两条叠在
            // 一起（整行是后画的，会把提示条盖掉半截）。
            let ty = match above {
                Some((_ix, iy, _w)) => iy - h - 6.0,
                None => py - h - 6.0,
            };
            // `ty` 与 `above.iy` 都是**视口相对**坐标（与上面 `tx` 同一份约定）：
            // "顶上放得下"的边界是视口顶 = 0，不是 `vy`（底条 `vy=0` 时两者恰好相同）。
            if ty > 0.0 {
                let (lx, ly, lw, lh) = local(tx, ty, w, h);
                canvas.fill_round_rect(lx, ly, lw, lh, 6.0, &Paint::fill(tk::pill_bg()));
                canvas.draw_text(
                    text,
                    Rect::new(lx as i32, ly as i32, lw as i32, lh as i32),
                    tk::overlay_text(),
                    Align::Center,
                    &small,
                );
            }
        }
    }

    // ── 信息条：浮条塞不下那一格时的那一整行 ──
    //
    // 常态位（浮条左端的定宽格）在上面浮条那一段就画完了，这里只处理 `Above`。
    // 为什么不留"放不下就整条隐藏"：状态是帧号/百分比的唯一载体，裁掉它 = 把刚加的
    // "第几帧"静默吞掉（AGENTS §八.7 看不见 → 必须显示）。
    if let Some((ix, iy, w)) = above {
        let (lx, ly, lw, lh) = local(ix, iy, w, INFO_H);

        // 背景：同工具栏风格
        canvas.fill_round_rect(lx, ly, lw, lh, 10.0, &Paint::fill(tk::pill_bg()));

        // 文本：垂直居中、水平居中。宽度是照**同一份 style** 量出来的（旧写法量 12 号、
        // 画 13 号，胶囊比字窄一圈，居中的字两头各溢出去一点）；只有窗口比文本还窄时
        // 才会真截断——不截就会折行画到胶囊外，与标题栏文件名那一档同一类缺陷。
        let shown = chrome.clip(canvas, info, &info_ts, (w - 16.0).max(0.0));
        canvas.draw_text(
            &shown,
            Rect::new(lx as i32, ly as i32, lw as i32, INFO_H as i32),
            tk::overlay_text_muted(),
            Align::Center,
            &info_ts,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::geometry::Color;

    /// m279 判据本体：十颗按钮的名字必须与它对应动作的名字**逐字相同**。
    /// 快捷键页读 `Action::name()`、浮条与右键「操作 ▸」读 `Btn::name()`——
    /// 以前两张表各写一遍，改一处忘一处就是用户看到的「动画上一帧 / 上一帧」。
    fn assert_btn_names_follow_actions(pick: impl Fn(Btn) -> &'static str, what: &str) {
        for b in Btn::ALL {
            assert_eq!(
                pick(b),
                b.action().name(),
                "{what}：「{}」的文案与动作名脱节",
                b.action().name()
            );
        }
    }

    #[test]
    fn btn_names_share_one_source_with_action_names() {
        assert_btn_names_follow_actions(|b| b.name(), "现状");
    }

    /// 负向臂：模拟 m279 之前「另抄一张表且其中一条用旧名」的形状，判据必须当场红。
    #[test]
    #[should_panic(expected = "文案与动作名脱节")]
    fn btn_names_reject_a_second_hardcoded_table() {
        assert_btn_names_follow_actions(
            |b| {
                if b == Btn::FrameBack {
                    "动画上一帧"
                } else {
                    b.name()
                }
            },
            "旧实现",
        );
    }

    /// ★ 用户 2026-11-06：静态位图没有帧/页号 ⇒ 浮条与「操作▸」菜单都不挂播放组。
    #[test]
    fn play_group_only_when_the_image_has_an_index() {
        let wide = FULL_ROW_MIN_W + 200.0;
        let (head, tail) = Chrome::row(false, wide);
        let shown: Vec<Btn> = head.iter().chain(tail.unwrap_or(&[])).copied().collect();
        assert!(!shown.contains(&Btn::FrameBack), "没有号就不给上一帧");
        assert!(!shown.contains(&Btn::PlayPause), "没有号就不给播放");
        assert!(!shown.contains(&Btn::FrameFwd), "没有号就不给下一帧");
        let (head, tail) = Chrome::row(true, wide);
        let shown: Vec<Btn> = head.iter().chain(tail.unwrap_or(&[])).copied().collect();
        assert!(
            shown.contains(&Btn::FrameBack)
                && shown.contains(&Btn::PlayPause)
                && shown.contains(&Btn::FrameFwd),
            "真动图 / 多页文档三颗齐全"
        );
    }

    /// m238：全屏默认收起顶条、顶缘悬停仍唤得出；非全屏行为一字不变。
    /// 八个组合全列出来，防止日后有人把某个分支「顺手简化」掉。
    #[test]
    fn title_visible_fullscreen_hides_but_hover_recalls() {
        // 全屏：默认不画，悬停才画（极简与否都一样）。
        assert!(
            !title_visible(true, false, false),
            "全屏+传统：不该还留顶条"
        );
        assert!(
            title_visible(true, false, true),
            "全屏+顶缘悬停：要能唤回按钮"
        );
        assert!(!title_visible(true, true, false), "全屏+极简：也不该有顶条");
        assert!(
            title_visible(true, true, true),
            "全屏+极简+悬停：同样唤得回"
        );
        // 非全屏：与改动前的 `!minimal_mode || hover_title` 逐项一致。
        assert!(title_visible(false, false, false), "传统模式常驻");
        assert!(title_visible(false, false, true), "传统模式悬停也仍画");
        assert!(!title_visible(false, true, false), "极简模式平时收起");
        assert!(title_visible(false, true, true), "极简模式悬停唤出");
    }

    #[test]
    fn host_audit_save_error_is_reported_separately_from_copy() {
        let c = Chrome::new(false);
        let copy = c.copy_posted("copy still running");
        // 非法尺寸会在编码阶段失败，不会创建任何文件，更不接触剪贴板。
        c.start_save_png(
            Some(crate::core::paths::test_scratch_root()),
            0,
            0,
            std::sync::Arc::new(Vec::new()),
        );
        let until = Instant::now() + Duration::from_secs(5);
        let result = loop {
            if let Some(out) = c.tick_save() {
                break Some(out);
            }
            if Instant::now() >= until {
                break None;
            }
            std::thread::yield_now();
        };
        assert!(matches!(result, Some(Err(_))), "保存失败不能只写日志");
        assert_eq!(c.tick_copy(), CopyWait::Waiting);
        assert_eq!(c.tick_save(), None);
        c.start_save_png(None, 1, 1, std::sync::Arc::new(vec![255; 4]));
        assert_eq!(c.tick_save(), None, "取消不能提示");
        drop(copy);
    }

    /// 状态格**跟着文本伸缩**（用户 2026-10-18 复看：定宽在信息右侧留下一大片空白），
    /// 但**档位只按满格判**——纵向因此不会跳，代价是横向会随文本滑（居中浮条的固有性质）。
    #[test]
    fn the_status_cell_tracks_the_text_while_the_tier_tracks_the_window() {
        // ① 档位只看视口宽：门槛 = 9 颗(346) + 满格。
        assert_eq!(info_place(PILL_INFO_MIN_W, true), InfoPlace::Pill);
        assert_eq!(info_place(PILL_INFO_MIN_W - 1.0, true), InfoPlace::Above);
        assert_eq!(
            info_place(360.0, true),
            InfoPlace::Above,
            "最小窗放不下满格"
        );
        // m245：按钮隐藏后连最小窗也留在浮条里居中，不再另起一行压在画上。
        assert_eq!(info_place(360.0, false), InfoPlace::Pill);
        // ② 格宽 = 文本宽 + 固定内边距：短状态不留空白，最长也就顶到满格。
        let short = info_cell_w(82.0); // 日常那条 "8/184 · 117%"
        let long = info_cell_w(212.0); // 最坏那条
        assert!(
            long - short > 100.0,
            "格宽必须真的跟着文本走，否则就是又写回了定宽"
        );
        assert_eq!(INFO_CELL_MAX, long);
        // ③ 浮条总宽 = 格 + 按钮行；按钮整排右移恰好一格。
        let wide = (0.0, 0.0, 1200.0, 600.0);
        assert_eq!(Chrome::pill(wide, false, short).2, NARROW_ROW_W + short);
        assert_eq!(
            Chrome::btn_rect(Btn::Out, wide, false, long).map(|r| r.0),
            Some(Chrome::pill(wide, false, long).0 + 8.0 + long)
        );
        // ④ 不变式：按**满格**算都不许撑破视口——文本再长也不会把按钮挤出窗口。
        for vw in [
            360.0,
            PILL_INFO_MIN_W - 1.0,
            PILL_INFO_MIN_W,
            FULL_ROW_MIN_W - 1.0,
            FULL_ROW_MIN_W,
            1920.0,
        ] {
            for anim in [false, true] {
                let v = (0.0, 0.0, vw, 600.0);
                let w = Chrome::pill(v, anim, Chrome::cell_reserve(vw)).2;
                assert!(
                    w <= vw,
                    "{vw} 宽 {}态浮条宽 {w}，撑破了",
                    if anim { "动图" } else { "静态" }
                );
            }
        }
    }

    /// ★ 这条是"上限不许拍脑袋"的机器版本：拿**真实排版**量最坏状态串，装不进文本框就红。
    /// 改文案格式、改字号、改测量口径都会立刻暴露——`draw_text` 的 rect 宽**同时是折行宽**，
    /// 文本超出夹取宽度的后果不是截断而是浮条里长出第二行（不报错、不截断）。
    /// **判据必须能真的失败**（K54）。
    /// 再往上的病态输入（一个目录 12345 张 → 232 px、一张图 12345 帧 → 237 px）**故意不兜**，
    /// 那种串走 `clip_text` 收成省略号；要兜就得给整条浮条多付 25 px。
    /// DWrite 不需要窗口，windui 自带测试同样这么跑。
    #[test]
    fn the_status_cell_actually_fits_the_worst_string() {
        use tiny_skia::Pixmap;
        use windui::render::SkiaCanvas;
        use windui::text::{DWriteEngine, TextEngine};
        let mut pm = Pixmap::new(8, 8).unwrap();
        let mut eng = DWriteEngine::new();
        eng.set_scale(1.0);
        let mut cx = SkiaCanvas::with_text(&mut pm, &mut eng, 1.0);
        let ts = TextStyle::new(13.0);
        for worst in [
            "1234/1234 · 100% (1234/1234)",
            "1234/1234 · 4000×3000 · 载入中…",
        ] {
            let w = cx.measure_text(worst, &ts).w as f32;
            assert!(
                w <= INFO_TEXT_MAX,
                "最坏状态串 {worst:?} 实测 {w} px，文本框上限只有 {INFO_TEXT_MAX} px —— 会折行"
            );
        }
    }

    /// 状态一变就把浮条叫醒——极简模式下**键盘**逐帧时唯一的可见性来源（鼠标不动，
    /// 悬停位永远不亮，帧号就没地方显示）。文本没变不许重复叫，空状态不许叫。
    #[test]
    fn status_change_wakes_the_pill_once() {
        let c = Chrome::new(true);
        assert!(c.wake_on_status_change("1/3 · 100%"), "新状态应当叫醒浮条");
        assert!(c.status_alive(), "叫醒 = 这一帧该画浮条");
        assert!(
            !c.wake_on_status_change("1/3 · 100%"),
            "同样的文本不算变化（动画播放期每帧都重绘）"
        );
        assert!(
            !c.wake_on_status_change(""),
            "空状态（错误只留中央）没有可看的内容"
        );
        assert!(c.status_alive(), "空状态不该把已经亮着的浮条掐掉");
        assert!(!c.hover_toolbar.get(), "空状态不借指针通道，指针位保持原样");
        // 浮条还亮着时再变文本：返回 false 是**对的**——它只报"从暗到亮"这一个翻转，
        // 字变了由状态路径自己重绘。但续期必须发生，否则动图播到一半浮条会自己灭。
        assert!(
            !c.wake_on_status_change("2/3 · 100%"),
            "已经亮着，不算从暗到亮"
        );
        assert!(c.status_alive(), "文本还在变就必须续期");
    }

    /// 极简模式播 AVIF 序列时"底部按钮栏一直闪"的回归（2026-11-02 用户报障）。
    ///
    /// 真实时序：动画每 100 ms 换一帧 ⇒ 状态文本每帧都变；同一时刻逐像素透明窗每帧重绘
    /// 都会补一条指针消息 ⇒ `activity` 每帧把指针判成"不在浮条上"并把 `hover_toolbar`
    /// 清掉。两条通道共用那一格时，浮条就在"状态唤醒"与"指针清空"之间高频翻转。
    /// 分开之后：指针怎么清都碰不到状态唤醒。
    #[test]
    fn pointer_channel_cannot_cancel_the_status_wake() {
        let c = Chrome::new(true);
        let view = (0.0, 0.0, 800.0, 600.0);
        for i in 0..12 {
            let _ = c.wake_on_status_change(&format!("1/1 · 100% ({}/120)", i + 1));
            // 指针一直停在画布中央，每帧都被判成"不在浮条上"
            let _ = c.activity((400.0, 300.0), view);
            assert!(c.status_alive(), "第 {i} 帧：状态唤醒被指针通道掐掉了");
            assert!(
                !c.hover_toolbar.get(),
                "第 {i} 帧：指针在画布上时 hover 位本来就该是 false"
            );
        }
    }

    /// 标题栏那一条的边界。左键手势分工（`ui::canvas::left_drag` 的调用方）与
    /// 悬停显隐读的是同一份判据，所以边界必须钉在这里，不能钉在调用方。
    #[test]
    fn title_strip_band_is_top_48_dp_and_inside_width() {
        let view = (0.0, 0.0, 800.0, 600.0);
        assert!(Chrome::in_title_strip((10.0, 0.0), view), "顶边");
        assert!(
            Chrome::in_title_strip((10.0, (STRIP_H + 7) as f32), view),
            "迟滞那 8dp 之内仍算标题栏"
        );
        assert!(
            !Chrome::in_title_strip((10.0, (STRIP_H + 8) as f32), view),
            "出了那一条就交给画布"
        );
        assert!(!Chrome::in_title_strip((10.0, 300.0), view), "画布正中");
        assert!(
            !Chrome::in_title_strip((-1.0, 10.0), view),
            "窗口外不许算标题栏"
        );
        assert!(!Chrome::in_title_strip((800.0, 10.0), view), "右缘是开区间");
    }

    /// ★ m257：**每一颗标题钮的中心都必须落在标题栏那一条里**，底条与贴左两种视口都要成立。
    ///
    /// 自 2026-11-08 用户实机报障反推：贴左时 `vw` 少一条宽，而 x 判据拿它当绝对上界，
    /// 于是最右那条（五颗钮全在里面）被判成「不在标题栏」——极简模式悬停不浮现，
    /// 可 `hit` 的矩形是绝对的、点下去照样关窗。判据写成「钮在哪、标题栏就得在哪」，
    /// 不依赖任何具体数字，以后再挪几何也不会漏。
    #[test]
    fn every_title_button_lies_inside_the_title_strip_for_both_viewports() {
        for view in [(0.0, 0.0, 900.0, 600.0), (112.0, 0.0, 788.0, 600.0)] {
            for tb in TitleBtn::ALL {
                let (x, y, w, h) = Chrome::title_btn_rect(tb, view);
                // 矩形是视口的相对坐标，判据吃的是窗口绝对坐标 ⇒ 搬一次。
                let center = (view.0 + x + w / 2.0, view.1 + y + h / 2.0);
                assert!(
                    Chrome::in_title_strip(center, view),
                    "{tb:?} 的中心 {center:?} 不在标题栏里（view={view:?}）"
                );
            }
        }
        // 贴左时左侧那一条是缩略图条，不属于标题栏。
        assert!(
            !Chrome::in_title_strip((111.0, 10.0), (112.0, 0.0, 788.0, 600.0)),
            "缩略图条那一条不算标题栏"
        );
    }

    /// 「操作」子菜单按 [`Btn::ALL`] 生成，故每个按钮都必须自带标签与字形——
    /// 少一个就是菜单里出现空行或豆腐块。
    #[test]
    fn every_button_is_labelled_and_drawable() {
        for b in Btn::ALL {
            assert!(!b.name().is_empty(), "{b:?} 缺菜单名");
            assert!(!b.glyph(false).is_empty(), "{b:?} 缺字形");
            assert!(!b.glyph(true).is_empty(), "{b:?} 缺暂停态字形");
        }
    }

    /// 动画组三颗必须**连续且排在末尾**——[`Chrome::row`] 用"截掉尾段"表达显隐，
    /// 谁往中间插一颗就把这个前提悄悄破掉（表现是浮条少一颗或重影）。
    #[test]
    fn anim_group_is_contiguous_and_at_the_tail() {
        let idx: Vec<usize> = Btn::ALL
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_anim())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            idx,
            vec![ANIM_START, ANIM_START + 1, ANIM_START + 2],
            "动画组必须是末尾连号"
        );
        assert_eq!(ANIM_START + 3, Btn::ALL.len(), "动画组之后不许再有按钮");
    }

    /// 双状态按钮：播放中画 ⏸、暂停中画 ▶，两个字不能相同。
    #[test]
    fn play_pause_glyph_is_two_state() {
        assert_ne!(Btn::PlayPause.glyph(false), Btn::PlayPause.glyph(true));
        // 其余按钮不受 `paused` 影响（不然翻页键会跟着变形）。
        for b in Btn::ALL {
            if b == Btn::PlayPause {
                continue;
            }
            assert_eq!(b.glyph(false), b.glyph(true), "{b:?} 不该是双状态");
        }
    }

    /// 行的颗数与浮条宽度：静态 7 颗、可播放（动图或多页文档）10 颗；
    /// **窗口放不下"满组 + 状态格"时播放组也退回**——状态格优先，因为按钮还有
    /// 快捷键与右键菜单，状态没有第二处。
    ///
    /// ★ 只有两组，不是一个图种一组：页与帧在宿主里是同一个号
    /// （[`crate::core::pixels::Anim`]），多页文档拿到的是**同一组**按钮。
    #[test]
    fn row_follows_playability_and_width() {
        let count = |anim: bool, vw: f32| Chrome::row_len(anim, vw);
        assert_eq!(count(false, 1920.0), 7);
        assert_eq!(count(true, 1920.0), 10);
        assert_eq!(
            count(true, FULL_ROW_MIN_W - 1.0),
            7,
            "放不下 10 颗 + 格子就不给播放组"
        );
        assert_eq!(count(true, FULL_ROW_MIN_W), 10);
        assert_eq!(count(true, 360.0), 7, "窄窗不许把按钮挤出窗口");
        // anim=true 时显示 10 颗（7 + 3 播放），宽度 = 格 + 16 + 10×34 + 9×3
        let (_x, _y, w, _h) = Chrome::pill(
            (0.0, 0.0, 1920.0, 1080.0),
            true,
            Chrome::cell_reserve(1920.0),
        );
        let w10 = 8.0 * 2.0 + 10.0 * 34.0 + 9.0 * 3.0;
        assert_eq!(w, w10 + INFO_CELL_MAX, "10 颗 + 满格");
    }

    /// 浮条总宽必须在**最小窗口**（360，见 `ui::window` 的 `min_size`）内放得下。
    ///
    /// `BTN_W` 定 34 是为了满组在 360 的最小窗宽里放得下——正是当初加
    /// 旋转/镜像按钮时踩过的坑（用户报"新按钮没有背景"）。
    #[test]
    fn pill_fits_min_window() {
        // 动图态 + **满格**一起量：窄到放不下 10 颗时 `Chrome::row` 自己退回 7 颗，
        // 而格宽自适应之后"最坏那一帧"就是满格，只有按它判才兜得住。
        let (_x, _y, w, _h) =
            Chrome::pill((0.0, 0.0, 360.0, 260.0), true, Chrome::cell_reserve(360.0));
        assert!(w <= 360.0, "浮条宽 {w} 撑破了最小窗宽 360");
    }

    /// 平移限速（M8）：一个周期只放行一帧、到点由轮询补、松手立刻补、没挂起时不许虚报。
    ///
    /// 三个入口都在这条用例里走一遍，是因为它们各自的"漏"长得不一样：漏 `tick_pan_pace`
    /// 画面停在松手前一步，漏 `flush_pan` 同样、但只在拖完就松手时看得见，`pan_dirty`
    /// 放行条件写反则整条拖动直接不刷新。时间用真 `Instant` + 20ms 间隔，比挂钟好读。
    #[test]
    fn pan_pace_allows_one_frame_per_period_and_never_drops_the_last() {
        let c = Chrome::new(false);
        let tick = Duration::from_millis(20);
        c.pan_interval.set(tick);

        assert!(c.pan_dirty(), "起手那一下不该被限速挡住");
        for _ in 0..6 {
            assert!(
                !c.pan_dirty(),
                "同一周期内的 move 不许再画（M8 省的就是这些）"
            );
        }
        assert!(!c.tick_pan_pace(), "还没到点就补帧，等于没限速");

        std::thread::sleep(tick + Duration::from_millis(4));
        assert!(c.tick_pan_pace(), "到点必须把挂着的那帧补出去");
        assert!(!c.tick_pan_pace(), "补完就没了，不许每个轮询都重画一整窗");
        assert!(!c.pan_dirty(), "补帧之后重新进入周期，周期内仍不画");

        std::thread::sleep(tick + Duration::from_millis(4));
        assert!(c.pan_dirty(), "下一个周期应当重新放行");
        assert!(!c.pan_dirty());
        assert!(c.flush_pan(), "松手要把挂起的那帧交出去");
        assert!(!c.flush_pan(), "没挂起时不许虚报，否则每次松手白重绘一整窗");
    }

    /// 每个按钮都能命中自己：不算 "Hit a button returns itself" 那种啰嗦断言，
    /// 只钉住"浮条中央成芬布局算出的矩形自己认得到"这一条。
    #[test]
    fn every_button_hits_its_own_rect() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        for anim in [false, true] {
            chrome.anim.set(anim);
            for b in Btn::ALL {
                let Some((x, y, w, h)) = Chrome::btn_rect(b, view, anim, chrome.info_cell.get())
                else {
                    assert!(!anim && b.is_anim(), "{b:?} 在 anim={anim} 下不该隐藏");
                    continue;
                };
                let hit = chrome.hit((x + w / 2.0, y + h / 2.0), view);
                assert_eq!(hit, Some(Press::Btn(b)), "{b:?} 命中不了自己");
            }
        }
        chrome.anim.set(false);
    }

    /// 5 颗标题按钮：各自矩形中央必须命中自己（与工具栏同款口径）。
    /// ★ m255：`to_canvas`（画）与 `hit` 的 `pos - view.0`（点）必须互逆。
    ///
    /// 底条时 `view.0 == 0`，方向写反也看不出来；缩略图条贴左后 `view.0 = 112`，
    /// 写反就是「按钮画在左边、点右上角却能关窗」（2026-11-08 用户报障）。
    /// 这条钉子故意用 `vx != 0`，就是这个坑的专用哨兵。
    /// 本文件的生产代码文本（`include_str!` 会把测试自身也包进来）。
    fn production_src() -> &'static str {
        let all = include_str!("chrome.rs");
        let cut = all
            .find(
                "
#[cfg(test)]",
            )
            .unwrap_or(all.len());
        &all[..cut]
    }

    /// ★ m255：绘制坐标只能经 `to_canvas` 搬（相对 → 绝对），不许在本地再减一次原点。
    ///
    /// 这条是**接线**钉子：`to_canvas` 自己的用例绿了，不代表 `paint` 真在用它——
    /// 旧写法 `|x, y, w, h| (x - vx, y - vy, w, h)` 就这么藏了一整年（底条时
    /// `vx/vy` 恒为 0，减 0 看不出来），缩略图条一贴左就整块平移一条。
    #[test]
    fn paint_moves_geometry_through_to_canvas_only() {
        let src = production_src();
        assert!(
            src.contains("to_canvas(view, (x, y, w, h))"),
            "paint 的 local 必须走 to_canvas（相对 → 绝对）"
        );
        assert!(
            !src.contains("h: f32| (x - vx"),
            "绘制不许再减视口原点：底条看不出来，贴左就整块平移一条"
        );
    }

    #[test]
    fn what_is_painted_is_what_gets_hit_when_the_origin_is_not_zero() {
        let view = (112.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        for tb in TitleBtn::ALL {
            let rel = Chrome::title_btn_rect(tb, view);
            let (ax, ay, aw, ah) = to_canvas(view, rel);
            assert_eq!(
                chrome.hit((ax + aw / 2.0, ay + ah / 2.0), view),
                Some(Press::TitleBtn(tb)),
                "{tb:?} 画在 ({ax}, {ay}) 却点不到"
            );
            // 少加一次原点（= 把方向写反）的位置不该命中任何标题按钮。
            assert_ne!(
                chrome.hit((rel.0 + rel.2 / 2.0, ay + ah / 2.0), view),
                Some(Press::TitleBtn(tb)),
                "{tb:?} 未加原点的位置不该命中"
            );
        }
    }

    /// ★ m275：状态格（`12/340`）是颗按钮（点它开「跳转到…帧/页」窗），所以"画在哪"必须
    /// 就是"点在哪"。贴左缩略图条时 `vx=112`——`info_rect` 若存**相对**坐标，整块
    /// 命中区就左移一条：点真格没反应，点左边的图反而弹窗（m255 那族的老毛病）。
    ///
    /// 两条判据各对应绘制里的两档：`Pill`（常态，格子住在浮条左端）与 `Above`
    /// （窄窗，整个浮条塞不下，单独占浮条上方一行）。**必须拿 `Chrome::pill` 现算的
    /// 相对几何去比**，不能只比"在视口内"——格子在浮条中部时，相对坐标也照样落在
    /// 视口里，那样写这条用例就永远不会红。
    #[test]
    fn status_cell_is_hittable_exactly_where_it_is_painted() {
        // ── 常态档：格子跟着浮条走 ──
        let view = (112.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(false);
        let mut spy = MeasureCount::new();
        paint(
            &mut spy,
            view,
            &chrome,
            "12/340 · 100%",
            true,
            true,
            false,
            false,
            false,
            false,
            "一张图.jpg",
        );
        let (ix, iy, iw, ih) = chrome.info_rect.get();
        assert!(iw > 0.0 && ih > 0.0, "状态格得真画出来，实得 {iw}×{ih}");
        let (px, _, pw, _) = Chrome::pill(view, chrome.anim.get(), chrome.info_cell.get());
        assert!(
            ix >= px + view.0 - 0.5 && ix + iw <= px + view.0 + pw,
            "状态格得落在浮条（画布 x {}…{}）里，实得 {}…{}：存了相对坐标就整块左移 {}",
            px + view.0,
            px + view.0 + pw,
            ix,
            ix + iw,
            view.0
        );
        let mid = (ix + iw / 2.0, iy + ih / 2.0);
        assert!(chrome.in_info_cell(mid), "画在哪就要点在哪");
        assert!(
            !chrome.in_info_cell((mid.0 - view.0, mid.1)),
            "少加一次视口原点的位置不许命中——那正是旧写法错开的那一格"
        );
        // 零矩形 = 这帧没画（极简收起 / 无边框 / 没状态文本）：不许还点得着。
        chrome.info_rect.set((0.0, 0.0, 0.0, 0.0));
        assert!(!chrome.in_info_cell(mid), "没画出来的时候不许命中");

        // ── 窄窗档（`Above`）：整条浮条只有一格宽，信息单独占上面一行 ──
        let narrow = (112.0, 0.0, 400.0, 300.0);
        let chrome = Chrome::new(false);
        paint(
            &mut spy,
            narrow,
            &chrome,
            "12/340 · 100%",
            true,
            true,
            false,
            false,
            false,
            false,
            "一张图.jpg",
        );
        let (ix, iy, iw, ih) = chrome.info_rect.get();
        assert!(iw > 0.0, "窄窗这一档也该把状态格画出来");
        assert!(
            ix >= narrow.0 && ix + iw <= narrow.0 + narrow.2,
            "窄窗档的状态格跑到视口外：x={ix} w={iw}（这一档存的是纯相对坐标）"
        );
        assert!(
            chrome.in_info_cell((ix + iw / 2.0, iy + ih / 2.0)),
            "窄窗档画在哪也要点在哪"
        );
    }

    #[test]
    fn every_title_button_hits_its_own_rect() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        for tb in TitleBtn::ALL {
            let (x, y, w, h) = Chrome::title_btn_rect(tb, view);
            let hit = chrome.hit((x + w / 2.0, y + h / 2.0), view);
            assert_eq!(hit, Some(Press::TitleBtn(tb)), "{tb:?} 命中不了自己");
        }
    }

    /// 右锚、不重叠：ALL 是右→左序，左边一颗的右缘必须 ≤ 右边一颗的左缘；
    /// 关闭钮贴视口右缘。
    #[test]
    fn title_buttons_are_right_anchored_without_overlap() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let vw = view.2;
        let (cx, _, cw, _) = Chrome::title_btn_rect(TitleBtn::Close, view);
        assert!((cx + cw - vw).abs() < f32::EPSILON, "关闭钮该贴右缘");
        for pair in TitleBtn::ALL.windows(2) {
            let (rx, _, _rw, _) = Chrome::title_btn_rect(pair[0], view); // 更靠右
            let (lx, _, lw, _) = Chrome::title_btn_rect(pair[1], view); // 更靠左
            assert!(
                lx + lw <= rx + f32::EPSILON,
                "{:?}/{:?} 矩形重叠",
                pair[0],
                pair[1]
            );
        }
    }

    /// 字形与字体：锁字形随锁定态切换、走 Segoe UI Symbol；最大化字形随
    /// 窗口状态在方框/还原间切换；置顶不走字形
    /// （📌 字形离屏 GDI 预渲染成位图，姿态 + 颜色即状态，见 `ui::pin`）。
    #[test]
    fn title_button_glyphs_and_fonts_reflect_state() {
        for tb in TitleBtn::ALL {
            assert!(!tb.glyph(false, false).is_empty(), "{tb:?} 缺未锁字形");
            assert!(!tb.glyph(true, true).is_empty(), "{tb:?} 缺锁定字形");
        }
        assert_eq!(TitleBtn::Lock.glyph(false, false), "\u{1F513}"); // 🔓
        assert_eq!(TitleBtn::Lock.glyph(true, false), "\u{1F512}"); // 🔒
        assert_eq!(
            TitleBtn::Topmost.glyph(false, false),
            TitleBtn::Topmost.glyph(true, true)
        );
        // 最大化钮：常规=方框，最大化=还原图标（两态必须真的不同）。
        assert_eq!(TitleBtn::Maximize.glyph(false, false), "\u{E922}");
        assert_eq!(TitleBtn::Maximize.glyph(false, true), "\u{E923}");
        assert_eq!(
            TitleBtn::Lock.font_family(),
            Some("Segoe UI Symbol"),
            "Lock 应走 Symbol 字体"
        );
        for tb in [
            TitleBtn::Topmost,
            TitleBtn::Minimize,
            TitleBtn::Maximize,
            TitleBtn::Close,
        ] {
            assert_eq!(tb.font_family(), None, "{tb:?} 不走 Symbol 字体");
        }
    }

    /// 锁定禁用矩阵（移植 CurSeen `lock_disables_everything_but_add_and_lock`）：
    /// 锁定后整窗只有锁住按钮命中得到——标题栏其余钮、工具栏、箭头一并哑掉。
    #[test]
    fn locked_hit_only_recognizes_the_lock_button() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        chrome.locked.set(true);
        for tb in TitleBtn::ALL {
            let (x, y, w, h) = Chrome::title_btn_rect(tb, view);
            let want = (tb == TitleBtn::Lock).then_some(Press::TitleBtn(TitleBtn::Lock));
            assert_eq!(
                chrome.hit((x + w / 2.0, y + h / 2.0), view),
                want,
                "锁定时 {tb:?} 的命中不对"
            );
        }
        for b in Btn::ALL {
            let Some((x, y, w, h)) = Chrome::btn_rect(b, view, false, chrome.info_cell.get())
            else {
                continue;
            };
            assert_eq!(
                chrome.hit((x + w / 2.0, y + h / 2.0), view),
                None,
                "锁定时工具栏 {b:?} 不该命中"
            );
        }
        for a in [Arrow::Prev, Arrow::Next] {
            let (cx, cy) = Chrome::arrow_center(a, view);
            assert_eq!(
                chrome.hit((cx, cy), view),
                None,
                "锁定时箭头 {a:?} 不该命中"
            );
        }
    }

    /// 显隐位**翻了**才报「画面要变」；在同一条栏里来回挪不报（一帧都不许多刷）。
    ///
    /// 这条是 2026-10-19「极简模式浮现不流畅」那一半的判据：`activity` 的返回值当年
    /// 根本没人接，浮现与收走只能等别的标脏源。
    #[test]
    fn activity_reports_only_the_flips_that_change_the_picture() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        assert!(
            !chrome.activity((400.0, 300.0), view),
            "起手就在暗处，没有位可翻"
        );
        assert!(
            chrome.activity((400.0, 10.0), view),
            "进顶条这一下必须报重绘"
        );
        assert!(
            !chrome.activity((500.0, 12.0), view),
            "同一条栏里横挪，位没翻就不该刷帧"
        );
        assert!(
            chrome.activity((400.0, 300.0), view),
            "出顶条这一下必须报重绘"
        );
        // 传统模式：三条栏常驻，那几位不参与显隐 ⇒ 一律不报；但状态照样算对（命中/几何要用）。
        let classic = Chrome::new(false);
        assert!(
            !classic.activity((400.0, 10.0), view),
            "传统模式报重绘就是白刷一整帧"
        );
        assert!(classic.hover_title.get(), "传统模式也得把悬停位算对");
    }

    /// 指针离开时**六位悬停状态一起清**，一位都不许留。
    ///
    /// 回归 2026-10-19 的另一半：`Leave` 分支原写成一条 `hover_btn…is_some() || hover_toolbar…`
    /// 的链，`||` 短路 ⇒ 只要第一位变了，后面五位根本不覆写。表现就是用户那句
    /// 「鼠标移开后有时候没能及时隐藏」——**恰好停在按钮/箭头上离开**时 `hover_toolbar`
    /// 还挂在 `true`，而 `tick_hide` 读滞留的 `last_pos`（仍在那条栏里）判成贴边豁免，
    /// 于是连 2 秒那道兜底都不发生。夹具先把第一位点亮，否则测不到那条短路。
    #[test]
    fn pointer_left_clears_every_hover_bit_not_just_the_first() {
        let chrome = Chrome::new(true);
        chrome.hover_btn.set(Some(Btn::In));
        chrome.hover_arrow.set(Some(Arrow::Prev));
        chrome.hover_title_btn.set(Some(TitleBtn::Close));
        chrome.hover_title.set(true);
        chrome.hover_toolbar.set(true);
        chrome.hover_info.set(true);
        chrome.hover_edge.set((true, true));
        chrome.pressed.set(Some(Press::Btn(Btn::In)));
        chrome.pressed_title_btn.set(Some(TitleBtn::Close));
        assert!(chrome.pointer_left(), "清了悬停态却不报重绘");
        assert_eq!(chrome.hover_btn.get(), None);
        assert_eq!(chrome.hover_arrow.get(), None);
        assert_eq!(chrome.hover_title_btn.get(), None);
        assert!(!chrome.hover_title.get(), "顶条悬停位没被清掉");
        assert!(
            !chrome.hover_toolbar.get(),
            "浮条悬停位没被清掉 ⇒ 那条栏永远收不走"
        );
        assert!(!chrome.hover_info.get(), "信息条悬停位没被清掉");
        assert_eq!(
            chrome.hover_edge.get(),
            (false, false),
            "箭头边带位没被清掉"
        );
        assert!(chrome.pressed.get().is_none(), "按钮按下位没被清掉");
        assert!(
            chrome.pressed_title_btn.get().is_none(),
            "标题按钮按下位没被清掉"
        );
        // 再离开一次：已经全暗，不该再报重绘（别把 Leave 刷成空转）。
        assert!(!chrome.pointer_left(), "没有东西可清却报要重绘");
    }

    /// 小窗口里箭头圆压到标题按钮上：命中顺序必须跟绘制顺序相反，箭头优先。
    #[test]
    fn arrow_hit_wins_over_title_button_in_a_short_window() {
        let chrome = Chrome::new(false);
        let view = (0.0, 0.0, 200.0, 100.0);
        // 该点同时在 Close 标题按钮矩形与 Next 箭头圆内（见 `arrow_center`/`title_btn_rect`）。
        assert_eq!(
            chrome.hit((180.0, 35.0), view),
            Some(Press::Arrow(Arrow::Next)),
            "压在一起的箭头应赢过标题按钮（后者先画、在下层）"
        );
    }

    /// 悬停记录**过期**时不再给豁免：光标早离开浮条了，那条栏却要永远亮着。
    ///
    /// 回归 `docs/待办.md` §10 ①：框架侧 `mouse_tracked` 是一次性旗，只在收到
    /// `WM_MOUSELEAVE` 时复位；一次交互式拖动/缩放把 leave 吞掉之后本会话再也收不到
    /// "离开"通知 ⇒ `pointer_left` 没人调，`tick_hide` 读滞留的 `last_pos`（还在浮条里）
    /// 判成豁免，连 2 秒那道兜底都不发生。新判据＝比一下"记 `last_pos` 那一刻的真实
    /// 光标位置"和"现在它在哪"，差出阈值就是记录过期。
    ///
    /// ★ 三条臂都要有：**读数相同仍豁免**（不误清"人真停在栏上"）、**读数不同立刻收**
    /// （这是修的那一半）、**边界内不动**（阈值不是 0，系统抖动不该把栏抽走）。
    #[test]
    fn stale_cursor_record_revokes_the_pill_exemption() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        let (px, py, pw, ph) = Chrome::pill(view, chrome.anim.get(), chrome.info_cell.get());
        let on_pill = (px + pw / 2.0, py + ph / 2.0);
        assert!(chrome.activity(on_pill, view), "进浮条该报重绘");
        assert!(chrome.hover_toolbar.get(), "夹具：浮条位得真的点亮");
        // 静止早已超过 2 秒：旧代码在这里判的是"last_pos 还在浮条里" ⇒ 永远不收。
        chrome.last_move.set(Instant::now() - IDLE_HIDE * 3);
        chrome.cursor_anchor.set(Some((1000, 1000)));

        // 臂①：真实光标仍在锚点上 ⇒ 豁免照旧（人真停在栏上不该被抽走）。
        assert!(
            !chrome.tick_hide(view, Some((1000, 1000))),
            "读数一致却收了浮条 ⇒ 悬停点击时被抢走"
        );
        assert!(chrome.hover_toolbar.get(), "臂① 不该动状态");
        // 臂②：漂移在阈值内（系统抖动）⇒ 仍然不收。
        assert!(
            !chrome.tick_hide(view, Some((1000 + STALE_DRIFT_PX, 1000))),
            "阈值内的抖动不该把栏抽走"
        );
        // 臂③：差出阈值 ⇒ 记录判死，立刻清并报重绘。
        assert!(
            chrome.tick_hide(view, Some((1000 + STALE_DRIFT_PX + 1, 1000))),
            "光标已经飞了，那条栏还收不走就是 §10 那条缺陷"
        );
        assert!(!chrome.hover_toolbar.get(), "过期判据没把浮条位清掉");
    }

    /// 过期判据走的是**整串清除**（`pointer_left`），箭头边带那两位也跟着收。
    ///
    /// 只清 `hover_title/toolbar/info` 三位是当年那条兜底的全部范围，而极简模式
    /// 2026-10-19 之后多了一对箭头浮现位——它们同样是"离开时没人通知"的受害者。
    #[test]
    fn stale_cursor_record_also_clears_the_arrow_band() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        assert!(chrome.activity((30.0, 300.0), view), "进左边带该报重绘");
        assert_eq!(chrome.hover_edge.get(), (true, false), "夹具：左边带点亮");
        chrome.cursor_anchor.set(Some((500, 500)));
        assert!(
            chrome.tick_hide(view, Some((500, 500 + STALE_DRIFT_PX + 40))),
            "过期判据没报重绘"
        );
        assert_eq!(
            chrome.hover_edge.get(),
            (false, false),
            "箭头浮现位滞留在 true——同一族缺陷的另一个受害者"
        );
    }

    /// **没有读数就不下判**：读不到光标 ≠ 光标飞走了。
    ///
    /// 反过来自证：`activity` 一被调用就把锚点填上（生产链路的读数来源接活的证据）。
    #[test]
    fn no_cursor_reading_never_revokes_the_exemption() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        let (px, py, pw, ph) = Chrome::pill(view, chrome.anim.get(), chrome.info_cell.get());
        assert!(chrome.activity((px + pw / 2.0, py + ph / 2.0), view));
        chrome.last_move.set(Instant::now() - IDLE_HIDE * 3);
        // 没有锚点（还从没在指针事件里记过）：不下判据。
        chrome.cursor_anchor.set(None);
        assert!(
            !chrome.tick_hide(view, Some((9999, 9999))),
            "没有锚点时把读数当真值会误清"
        );
        // 有锚点但这帧读失败：同样不下判据。
        chrome.cursor_anchor.set(Some((1000, 1000)));
        assert!(
            !chrome.tick_hide(view, None),
            "GetCursorPos 失败被当成光标飞走 = 每次失败都闪一下"
        );
        assert!(chrome.hover_toolbar.get());
        // 生产侧接线自证：真实 `activity` 会填锚点（这里读的是本机桌面会话的真光标）。
        let probe = Chrome::new(true);
        let _ = probe.activity((400.0, 300.0), view);
        assert!(
            probe.cursor_anchor.get().is_some(),
            "`activity` 没接上光标读数——判据在生产里根本没活"
        );
    }

    /// 悬停态快照的形状：标题栏位、浮条位、信息条位、两侧边带，加三枚可选高亮。
    type HoverBits = (
        bool,
        bool,
        bool,
        (bool, bool),
        Option<Btn>,
        Option<Arrow>,
        Option<TitleBtn>,
    );

    /// 悬停态的**全量快照**（只读，不覆写）。传统模式那条判据要看的是"清没清"，
    /// 而 `pointer_left()` 的返回值本身带 `minimal_mode` 因子，报不出这件事，所以这里
    /// 把七位一起抄下来比。`Option` 那三位连值一起抄：不止"有没有悬停"，悬停在哪颗也算。
    fn bits(chrome: &Chrome) -> HoverBits {
        (
            chrome.hover_title.get(),
            chrome.hover_toolbar.get(),
            chrome.hover_info.get(),
            chrome.hover_edge.get(),
            chrome.hover_btn.get(),
            chrome.hover_arrow.get(),
            chrome.hover_title_btn.get(),
        )
    }

    /// ★ 传统模式（**默认档**）对光标读数**完全无感**——这是 m107 那个省略的凭据：
    /// runtime 在非极简模式下干脆不调 `GetCursorPos`（每拍一次是纯白花，K80 量过 779–807 ns），
    /// 只把 `None` 交进来。要敢这么省，得先证明"给过期读数""给 `None`""压根没这一格"三种喂法
    /// 在传统模式下**连悬停态都不许有差别**。**把 `tick_hide` 第一行那道传统模式 return 摘掉，
    /// 比状态这两条当场红**（变异 M-minimal 记在 本仓历史记录；只比返回值的写法被实测证伪过，
    /// 摘掉门闸它照样绿）。
    #[test]
    fn classic_mode_never_needs_a_cursor_reading() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(false);
        let (px, py, pw, ph) = Chrome::pill(view, chrome.anim.get(), chrome.info_cell.get());
        // 先在传统模式下把悬停态立起来（`activity` 四位是无条件覆写的，只有返回值受档位管）。
        // 这一步不是铺垫，是判据的一半：不比状态就只能比 `false == false`，那什么都钉不住。
        assert!(
            !chrome.activity((px + pw / 2.0, py + ph / 2.0), view),
            "传统模式里 `activity` 恒不报重绘——它要是报了，runtime 那条省光标的理由就得连它一起改"
        );
        chrome.cursor_anchor.set(Some((1000, 1000)));
        chrome.last_move.set(Instant::now() - IDLE_HIDE * 3);
        assert!(
            chrome.hover_toolbar.get(),
            "样本没把浮条立起来 ⇒ 后面比的是两个 false，用例是空转的"
        );
        let before = bits(&chrome);
        // 一份"早就离开栏"的过期读数 + 静止计时也过期：传统模式仍不许动任何东西
        assert!(
            !chrome.tick_hide(view, Some((9999, 9999))),
            "传统模式里过期光标读数会收走浮条 ⇒ runtime 不取光标就不安全"
        );
        // ★ 真正的凭据在**状态**而不在返回值：`pointer_left()` 的返回值本身就带
        //   `minimal_mode` 因子，传统模式下"清了"与"没清"都报 false ——只比返回值的那条
        //   断言对着摘掉门闸的变异照样绿（m107 实测）。清没清要看那几位有没有被覆写。
        assert_eq!(
            before,
            bits(&chrome),
            "传统模式里 tick_hide 不许改写任何悬停位——runtime 非极简省掉那格光标读数的凭据"
        );
        assert!(
            !chrome.tick_hide(view, None),
            "传统模式里给 None 也不许有动作（runtime 省掉的那一格就是这个形状）"
        );
        assert_eq!(before, bits(&chrome), "给 None 那一格同样不许动状态");
    }

    /// 极简模式的翻页箭头：**浮现看边带、点击看圆，两者不是一回事**。
    ///
    /// 这条钉的是 2026-10-19 翻案后的新口径（"既然要极简就追求到底，但悬停还是要出现"）：
    /// 画面正中两枚都不画，但**不画不等于点不着**（箭头的圆照旧命中，否则就是把鼠标用户的
    /// 翻页入口删了）；靠边（哪怕还在圆外）就浮现；同一侧里挪动不多刷一帧；回正中立刻收走；
    /// 离开窗口两位一起清。
    #[test]
    fn arrow_reveal_is_the_edge_band_while_the_click_circle_stays_put() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        assert!(
            !chrome.activity((400.0, 300.0), view),
            "起手在正中，没有位可翻"
        );
        assert_eq!(
            chrome.hover_edge.get(),
            (false, false),
            "画面正中不该浮现任何一枚箭头"
        );
        let (cx, cy) = Chrome::arrow_center(Arrow::Prev, view);
        assert_eq!(
            chrome.hit((cx, cy), view),
            Some(Press::Arrow(Arrow::Prev)),
            "箭头隐身了就点不着——那是删入口，不是极简"
        );
        // x=30 在带内（也在圆内）；下面 x=20,y=320 那点**在圆外仍在带内** ⇒ 浮现看的是带不是圆。
        assert!(
            chrome.activity((30.0, 300.0), view),
            "进左边带这一下必须报重绘"
        );
        assert_eq!(
            chrome.hover_edge.get(),
            (true, false),
            "左边带该只浮现上一张那侧"
        );
        assert!(
            !chrome.activity((20.0, 320.0), view),
            "同一侧里挪动不该再刷一帧"
        );
        let (nx, ny) = Chrome::arrow_center(Arrow::Next, view);
        assert_eq!(
            chrome.hit((nx, ny), view),
            Some(Press::Arrow(Arrow::Next)),
            "右边那枚热区不许跟着显隐一起缩"
        );
        assert_eq!(
            chrome.hover_edge.get(),
            (true, false),
            "右箭头点得着 ≠ 右箭头该浮现"
        );
        assert!(
            chrome.activity((400.0, 300.0), view),
            "回正中要立刻收走并报重绘"
        );
        assert_eq!(chrome.hover_edge.get(), (false, false));
        assert!(chrome.activity((780.0, 300.0), view));
        assert_eq!(
            chrome.hover_edge.get(),
            (false, true),
            "右边带该只浮现下一张那侧"
        );
        let _ = chrome.activity((400.0, 300.0), view);
        let _ = chrome.activity((30.0, 300.0), view);
        assert!(chrome.pointer_left());
        assert_eq!(
            chrome.hover_edge.get(),
            (false, false),
            "离开窗口还有箭头亮着"
        );
        // 传统模式：箭头常驻 ⇒ 位照样更新，但不报重绘（报了就是白刷一帧）。
        let classic = Chrome::new(false);
        assert!(
            !classic.activity((30.0, 300.0), view),
            "传统模式边带变化不参与显隐"
        );
        assert_eq!(
            classic.hover_edge.get(),
            (true, false),
            "位还是要算对（命中与后续复用都读它）"
        );
    }

    /// 只记圆形坐标的记录型画布：本文件这条判据要问的就是"**这枚圆画没画**"，
    /// 其余画全丢弃（`draw_text` 在无文字引擎时本来就是空操作，量文字给个粗估即可）。
    struct SpyCircles {
        circles: Vec<(f32, f32, f32)>,
    }

    impl Canvas for SpyCircles {
        fn dpi_scale(&self) -> f32 {
            1.0
        }
        fn save(&mut self) {}
        fn restore(&mut self) {}
        fn clip_rect(&mut self, _r: Rect) {}
        fn push_layer(&mut self, _opacity: f32) {}
        fn pop_layer(&mut self) {}
        fn fill_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _paint: &Paint) {}
        fn fill_round_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _r: f32, _p: &Paint) {}
        fn stroke_round_rect(
            &mut self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _r: f32,
            _line: f32,
            _p: &Paint,
        ) {
        }
        fn draw_line(&mut self, _x0: f32, _y0: f32, _x1: f32, _y1: f32, _w: f32, _p: &Paint) {}
        fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, _paint: &Paint) {
            self.circles.push((cx, cy, r));
        }
        fn draw_shadow(
            &mut self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _r: f32,
            _blur: f32,
            _c: Color,
        ) {
        }
        fn draw_image(&mut self, _img: &Image, _dst: Rect, _fit: Fit, _r: f32, _o: f32) {}
        fn draw_text(&mut self, _t: &str, _r: Rect, _c: Color, _a: Align, _ts: &TextStyle) {}
        fn measure_text(&mut self, text: &str, ts: &TextStyle) -> windui::geometry::Size {
            windui::geometry::Size::new(
                (text.chars().count() as f32 * ts.size * 0.6).ceil() as i32,
                ts.size.ceil() as i32,
            )
        }
    }

    /// ★ "隐藏箭头"必须是**真的没画**，"悬停要出现"必须真的画出来。
    ///
    /// 前面那几条只验状态位（`hover_edge`）；状态位对 ≠ 屏幕上对——门控写在 `paint` 里，
    /// 所以判据也得打在 `paint` 上。数的是**落在两个圆心上的圆**，别处的圆（标题栏里
    /// 那枚手绘图钉）不算，传统模式顶条亮着也不会误判。
    #[test]
    fn minimal_mode_really_does_not_paint_the_arrows_until_the_band_is_entered() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let (px, py) = Chrome::arrow_center(Arrow::Prev, view);
        let (nx, ny) = Chrome::arrow_center(Arrow::Next, view);
        let arrow_circles = |chrome: &Chrome| -> usize {
            let mut spy = SpyCircles {
                circles: Vec::new(),
            };
            paint(
                &mut spy,
                view,
                chrome,
                "",
                true,
                true,
                false,
                false,
                false,
                chrome.minimal_mode.get(),
                "",
            );
            spy.circles
                .iter()
                .filter(|(cx, cy, _)| {
                    ((cx - px).abs() < 0.6 && (cy - py).abs() < 0.6)
                        || ((cx - nx).abs() < 0.6 && (cy - ny).abs() < 0.6)
                })
                .count()
        };
        let chrome = Chrome::new(true);
        let _ = chrome.activity((400.0, 300.0), view);
        assert_eq!(arrow_circles(&chrome), 0, "极简模式画面正中：一枚都不该画");
        let _ = chrome.activity((30.0, 300.0), view);
        assert_eq!(arrow_circles(&chrome), 1, "左边带：只画上一张那枚");
        let _ = chrome.activity((780.0, 300.0), view);
        assert_eq!(
            arrow_circles(&chrome),
            1,
            "右边带：只画下一张那枚（左边那枚要跟走）"
        );
        let _ = chrome.activity((400.0, 300.0), view);
        assert_eq!(arrow_circles(&chrome), 0, "回正中：两枚都得没");
        // 传统模式：两枚常驻，一个都不许少。
        let classic = Chrome::new(false);
        assert_eq!(arrow_circles(&classic), 2, "传统模式箭头常驻");
        // 只有一张图时进边带也不许凭空画（`has_prev/has_next` 这道闸在显隐之前，别改顺序）。
        let alone = Chrome::new(true);
        let _ = alone.activity((30.0, 300.0), view);
        let mut spy = SpyCircles {
            circles: Vec::new(),
        };
        paint(
            &mut spy, view, &alone, "", false, false, false, false, false, true, "",
        );
        assert_eq!(spy.circles.len(), 0, "没有邻居就没有箭头，浮现与否都一样");
    }

    /// ★ 放映**唯一的状态面**：右侧那颗「下一张」持续亮起并强制可见（没有文字提示）。
    /// 钉的是"画没画"这一半：极简模式本来把箭头收成悬停才出现，放映必须把它顶出来——
    /// 指示藏在会自动消失的浮层里就等于没有指示（§八.7 同一条判据）。
    /// 左侧那颗不许被连带点亮（它亮着会被读成"也在往上翻"）。
    #[test]
    fn slideshow_keeps_the_next_arrow_visible() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let (px, py) = Chrome::arrow_center(Arrow::Prev, view);
        let (nx, ny) = Chrome::arrow_center(Arrow::Next, view);
        let drawn = |chrome: &Chrome| -> (bool, bool) {
            let mut spy = SpyCircles {
                circles: Vec::new(),
            };
            paint(
                &mut spy, view, chrome, "", true, true, false, false, false, true, "",
            );
            let near = |tx: f32, ty: f32| {
                spy.circles
                    .iter()
                    .any(|(cx, cy, _)| (cx - tx).abs() < 0.6 && (cy - ty).abs() < 0.6)
            };
            (near(px, py), near(nx, ny))
        };

        let chrome = Chrome::new(true);
        assert_eq!(
            drawn(&chrome),
            (false, false),
            "夹具：极简模式画面正中，两枚都不该画"
        );
        chrome.toggle_slideshow();
        assert_eq!(
            drawn(&chrome),
            (false, true),
            "放映中只该点亮右侧「下一张」那一颗"
        );
        chrome.toggle_slideshow();
        assert_eq!(
            drawn(&chrome),
            (false, false),
            "关掉放映之后那颗必须回到不画"
        );
    }

    /// ★ 进度擦除宽度（`wipe_width`）：形状是**整颗底自左向右被推满**，不是我第一版那样
    /// 在圆里加一根条（用户 2026-10-31 驳回："我说的是整个按钮的底色变化"）。
    /// 所以判据是三件事：起点为零、满格正好是整个圆盘直径、中间线性且不回头。
    #[test]
    fn wipe_width_sweeps_the_whole_disc_left_to_right() {
        let full = ARROW_R * 2.0;
        // 一格 = 一列像素：这是 `clip_rect` 能给出的**最细边界**（整数矩形遮罩、ALIASED），
        // 所以"平滑"的上限就是圆盘直径那么多格，再多也只能靠插值——我们不做插值。
        assert_eq!(SLIDE_STEPS as f32, full, "格数必须等于圆盘直径像素数");
        assert!(
            (wipe_width(1) - wipe_width(0) - 1.0).abs() < 1e-4,
            "第一格就该推进一列，用户 2026-10-31 嫌僵硬的那处就是这里"
        );
        assert_eq!(wipe_width(0), 0.0, "第 0 格一点都没推");
        assert!(
            (wipe_width(SLIDE_STEPS) - full).abs() < 1e-4,
            "满格必须盖住整颗"
        );
        assert!(
            (wipe_width(SLIDE_STEPS / 2) - full / 2.0).abs() < 1e-4,
            "半格该正好在直径中点"
        );
        let mut last = -1.0;
        for step in 0..=SLIDE_STEPS {
            let w = wipe_width(step);
            assert!(w > last, "第 {step} 格不该比上一格更窄（进度不许回头）");
            assert!(w <= full + 1e-4, "第 {step} 格推过了圆盘");
            last = w;
        }
        // 脏值兜底：越界的格数也只能推到满，不能推出圆外。
        assert!((wipe_width(SLIDE_STEPS * 4) - full).abs() < 1e-4);
    }

    /// ★ 进度格数（`slide_step`）必须**由"还剩多久"反推**，而不是自己数帧：这样设置页改间隔、
    /// 机器卡一下、甚至这一帧被别的重绘挤掉，都不会让进度与真正翻页的时刻脱钩——
    /// 条走完就是翻页，两者读的是同一个 `slide_due`。
    /// 挪钟不 sleep，与 pan/wheel/key 三条闸同一定式。
    #[test]
    fn slide_step_counts_down_from_the_remaining_time() {
        let chrome = Chrome::new(false);
        assert_eq!(chrome.slide_step(), 0, "没在放就是空条");
        chrome.toggle_slideshow();
        chrome.slide_span.set(Duration::from_secs(8));
        let now = Instant::now();
        // 间隔 8 秒、40 格 ⇒ 每过 1 秒走 `SLIDE_STEPS/8` 格（写成比例而不是硬数：
        // 格的总数由圆盘直径决定，改半径时这条用例跟着成立，而不是悄悄失效）。
        for (remain_s, want) in [
            (8, 0),
            (7, SLIDE_STEPS / 8),
            (6, SLIDE_STEPS / 4),
            (5, SLIDE_STEPS * 3 / 8),
            (3, SLIDE_STEPS * 5 / 8),
            (1, SLIDE_STEPS * 7 / 8),
            (0, SLIDE_STEPS),
        ] {
            chrome
                .slide_due
                .set(now + Duration::from_secs(remain_s as u64));
            assert_eq!(
                chrome.slide_step(),
                want,
                "还剩 {remain_s}s（间隔 8s）该是第 {want} 格"
            );
        }
        // 间隔被改成 4 秒：同一个"还剩 2 秒"该走到中点那一格，而不是继续按 8 秒算。
        chrome.slide_span.set(Duration::from_secs(4));
        chrome
            .slide_due
            .set(Instant::now() + Duration::from_secs(2));
        assert_eq!(
            chrome.slide_step(),
            SLIDE_STEPS / 2,
            "改间隔后分母必须跟着换"
        );
    }

    /// ★ 重绘只在**换格**那一帧发生：同一格里连续问 20 次，只许有一次说要刷。
    /// 这条是"低占用"的根据——整扇窗口逐像素分层，每帧重合成一次是有代价的，
    /// 每间隔画八跳与每帧画六十跳是两种产品。
    #[test]
    fn slide_step_only_announces_changes() {
        let chrome = Chrome::new(false);
        chrome.slide_span.set(Duration::from_secs(8));
        chrome.toggle_slideshow();
        chrome
            .slide_due
            .set(Instant::now() + Duration::from_secs(4));
        assert!(chrome.slide_step_tick(), "第一次读到新格子该说要重绘");
        for _ in 0..20 {
            assert!(
                !chrome.slide_step_tick(),
                "同一格里不许再刷——那等于每帧重画全窗"
            );
        }
        chrome
            .slide_due
            .set(Instant::now() + Duration::from_secs(3));
        assert!(chrome.slide_step_tick(), "换了一格才该刷一次");
        // 关掉放映：格子归零，必须再刷一次把进度色擦掉（不许留在屏上）。
        chrome.toggle_slideshow();
        assert!(chrome.slide_step_tick(), "关掉后归零也得刷一次");
        assert_eq!(chrome.slide_step(), 0, "关态必须是空条");
    }

    /// 只数 `measure_text` 调用次数的记录型画布：O1 那条判据问的就是"同一串字每帧排几次"。
    /// 排版估法与 [`SpyCircles`] 同一份，宽度只用来决定截不截断。
    struct MeasureCount {
        measures: usize,
    }

    impl MeasureCount {
        fn new() -> Self {
            Self { measures: 0 }
        }
        fn paint_frame(&mut self, chrome: &Chrome, info: &str, title: &str) -> usize {
            let before = self.measures;
            paint(
                self,
                (0.0, 0.0, 800.0, 600.0),
                chrome,
                info,
                true,
                true,
                false,
                false,
                false,
                false,
                title,
            );
            self.measures - before
        }
    }

    impl Canvas for MeasureCount {
        fn dpi_scale(&self) -> f32 {
            1.0
        }
        fn save(&mut self) {}
        fn restore(&mut self) {}
        fn clip_rect(&mut self, _r: Rect) {}
        fn push_layer(&mut self, _opacity: f32) {}
        fn pop_layer(&mut self) {}
        fn fill_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _paint: &Paint) {}
        fn fill_round_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _r: f32, _p: &Paint) {}
        fn stroke_round_rect(
            &mut self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _r: f32,
            _line: f32,
            _p: &Paint,
        ) {
        }
        fn draw_line(&mut self, _x0: f32, _y0: f32, _x1: f32, _y1: f32, _w: f32, _p: &Paint) {}
        fn fill_circle(&mut self, _cx: f32, _cy: f32, _r: f32, _paint: &Paint) {}
        fn draw_shadow(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _r: f32, _b: f32, _c: Color) {
        }
        fn draw_image(&mut self, _img: &Image, _dst: Rect, _fit: Fit, _r: f32, _o: f32) {}
        fn draw_text(&mut self, _t: &str, _r: Rect, _c: Color, _a: Align, _ts: &TextStyle) {}
        fn measure_text(&mut self, text: &str, ts: &TextStyle) -> windui::geometry::Size {
            self.measures += 1;
            windui::geometry::Size::new(
                (text.chars().count() as f32 * ts.size * 0.6).ceil() as i32,
                ts.size.ceil() as i32,
            )
        }
    }

    /// ★ O1 的判据：**字没变就不许再排**。
    ///
    /// 浮条一帧里为同一串字排很多次（状态格定宽 + 标题截断的二分 + 提示条），而字与宽
    /// 只在翻页 / 改倍率 / 改窗宽 / 换 DPI 时才变。这条用例钉三件事：
    /// ① 第一帧确实排了（不许是"根本没走到排版"的假绿）；② 原样重画一帧，排版次数为 0；
    /// ③ 换了文本必须重新排（缓存不许把上一张的文件名画在这一张上）。
    /// 旧写法（没有 `TextMemo`，每帧现排）在这一条下**第 ② 步就红**。
    #[test]
    fn unchanged_text_is_not_re_shaped_frame_after_frame() {
        let chrome = Chrome::new(false);
        let mut spy = MeasureCount::new();
        let title = "一张名字长得放不下需要截断的照片_2026-10-23_婚礼现场全景.jpg";
        let first = spy.paint_frame(&chrome, "3/9 · 100%", title);
        assert!(first > 1, "第一帧要真排版过才对，实测 {first} 次");
        let again = spy.paint_frame(&chrome, "3/9 · 100%", title);
        assert_eq!(
            again, 0,
            "同一串字、同一个宽：第二帧一次都不该再排，实排 {again} 次"
        );
        // 翻页 = 状态串与标题都换了，必须重新量。
        let changed = spy.paint_frame(&chrome, "4/9 · 100%", "下一张.jpg");
        assert!(changed > 0, "换了文本还复用旧账 = 把上一张的字画在这一张上");
    }

    /// ★ 记账不许改变截断结果：缓存命中那一支与现算必须逐字符相同。
    #[test]
    fn the_text_memo_returns_the_same_string_as_recomputing() {
        let chrome = Chrome::new(false);
        let mut spy = MeasureCount::new();
        let ts = TextStyle::new(13.0);
        let long = "1234567890".repeat(12);
        let direct = clip_text(&mut spy, &long, &ts, 90.0);
        let a = chrome.clip(&mut spy, &long, &ts, 90.0);
        let b = chrome.clip(&mut spy, &long, &ts, 90.0);
        assert_eq!(a, direct, "第一次走账的结果就该与现算逐字相同");
        assert_eq!(b, a, "命中缓存不许换字");
        assert!(
            direct.ends_with('…'),
            "放不下时必须有省略号，实得 {direct:?}"
        );
        // 宽变了 ⇒ 必须重算（不许把窄档的截断发到宽档上）。
        let wider = chrome.clip(&mut spy, &long, &ts, 9000.0);
        assert_eq!(wider, long, "放得下时就该是整串原文");
    }

    /// ★ 键里必须有 DPI：把窗拖到另一块缩放率的屏上，同一串字的逻辑宽会变，
    /// 漏掉 DPI 就是把上一块屏的量出的宽与截断发到这一块屏上（表现为文字两头溢出或
    /// 短一截）。这条用例用两枚不同 `dpi_scale` 的画布互撞同一串字。
    #[test]
    fn the_text_memo_key_includes_dpi() {
        let chrome = Chrome::new(false);
        let mut one = MeasureCount::new();
        let ts = TextStyle::new(13.0);
        let text = "3/9 · 100%";
        let w1 = chrome.measure_w(&mut one, text, &ts);
        // 同一 DPI：第二次必须走账（一次都不排）。
        let before = one.measures;
        assert_eq!(chrome.measure_w(&mut one, text, &ts), w1);
        assert_eq!(one.measures, before, "同 DPI 同串字不许重排");
        // 换 DPI：必须重排量出的宽（`TwiceDpi` 把 `dpi_scale` 报成 2.0）。
        let mut two = TwiceDpi::new();
        let w2 = chrome.measure_w(&mut two, text, &ts);
        assert_eq!(
            two.measures, 1,
            "换了 DPI 还复用旧账 = 用上一块屏的宽画这一块的字号"
        );
        assert_ne!(
            w1, w2,
            "两档 DPI 的估宽本来就该不同（用例的前提，不是产品结论）"
        );
    }

    /// `MeasureCount` 的换屏版：只有 `dpi_scale` 不同。
    struct TwiceDpi {
        measures: usize,
    }

    impl TwiceDpi {
        fn new() -> Self {
            Self { measures: 0 }
        }
    }

    impl Canvas for TwiceDpi {
        fn dpi_scale(&self) -> f32 {
            2.0
        }
        fn save(&mut self) {}
        fn restore(&mut self) {}
        fn clip_rect(&mut self, _r: Rect) {}
        fn push_layer(&mut self, _opacity: f32) {}
        fn pop_layer(&mut self) {}
        fn fill_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _paint: &Paint) {}
        fn fill_round_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _r: f32, _p: &Paint) {}
        fn stroke_round_rect(
            &mut self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _r: f32,
            _line: f32,
            _p: &Paint,
        ) {
        }
        fn draw_line(&mut self, _x0: f32, _y0: f32, _x1: f32, _y1: f32, _w: f32, _p: &Paint) {}
        fn fill_circle(&mut self, _cx: f32, _cy: f32, _r: f32, _paint: &Paint) {}
        fn draw_shadow(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _r: f32, _b: f32, _c: Color) {
        }
        fn draw_image(&mut self, _img: &Image, _dst: Rect, _fit: Fit, _r: f32, _o: f32) {}
        fn draw_text(&mut self, _t: &str, _r: Rect, _c: Color, _a: Align, _ts: &TextStyle) {}
        fn measure_text(&mut self, text: &str, ts: &TextStyle) -> windui::geometry::Size {
            self.measures += 1;
            // 与 `MeasureCount` 故意不同：DPI 越高、估宽越大，用例才分得出"重排了没有"。
            windui::geometry::Size::new(
                (text.chars().count() as f32 * ts.size * 0.75).ceil() as i32,
                ts.size.ceil() as i32,
            )
        }
    }

    /// ★ m74 的阈值判据：日常形态必须留在**当场**那条路，巨图必须走线程。
    ///
    /// 数字全是实测来的（`examples/copy_png_cost.rs`，Fastest 档约 17 ns/像素）：
    /// 1620×1080 那档当场编 25 ms，6055×6055 要 612 ms。判据单独成函数就是为了这条
    /// 用例能无头跑——一旦走到执行，就会真去写系统剪贴板。
    #[test]
    fn copy_route_keeps_everyday_sizes_on_the_ui_thread() {
        assert_eq!(
            copy_route(1620 * 1080),
            CopyRoute::Sync,
            "1080p 窗里看普通照片那一档就该当场编完（实测 25 ms）"
        );
        assert_eq!(
            copy_route(1800 * 1200),
            CopyRoute::Sync,
            "2.2 MP 原图当场编完（实测 36 ms）"
        );
        assert_eq!(
            copy_route(6055 * 6055),
            CopyRoute::Threaded,
            "36.7 MP 当场要停 0.6 秒，必须挪走"
        );
        // 边界本身也要有归属：正好等于上限算"当场"。
        assert_eq!(copy_route(COPY_SYNC_PIXELS), CopyRoute::Sync);
        assert_eq!(copy_route(COPY_SYNC_PIXELS + 1), CopyRoute::Threaded);
    }

    /// ★ 投出去那一刻的提示里**说了这张多大**，而落地之前一句"已复制"都不许出现。
    #[test]
    fn copy_note_promises_the_size_and_waits_before_it_speaks() {
        let chrome = Chrome::new(false);
        assert_eq!(chrome.tick_copy(), CopyWait::Idle, "没复制过就该一声不出");
        let tx = chrome.copy_posted("正在后台复制整张画面（6055×6055）…");
        assert!(
            chrome.copy_note().contains("6055×6055"),
            "投出去那句话得说清复制了多大：{}",
            chrome.copy_note()
        );
        assert!(chrome.copy_in_flight(), "登记之后就是在飞");
        assert_eq!(
            chrome.tick_copy(),
            CopyWait::Waiting,
            "回执没到就什么都不说——早说一句就是假成功"
        );
        chrome.copy_reset();
        assert!(
            !chrome.copy_in_flight() && chrome.copy_note().is_empty(),
            "reset 得把在飞位与提示一起收掉"
        );
        assert_eq!(chrome.tick_copy(), CopyWait::Idle);
        // 通道已经随 reset 作废：那一枚线程的回执不该再被谁读走。
        let _ = tx.send(Ok(()));
    }

    /// ★ 成功只有一条路能说出来：回执是 `Ok` 的那一帧，而且**只说一次**。
    ///
    /// 落定之后在飞位必须清掉——清不掉就是把"复制"这一功能永久锁死（下一次一按就是
    /// "还在进行"，而用户看得见的那句假话已经没有了）。
    #[test]
    fn copied_is_only_ever_said_on_the_ok_receipt() {
        let chrome = Chrome::new(false);
        let tx = chrome.copy_posted("正在后台复制整张画面（6055×6055）…");
        assert_eq!(
            chrome.tick_copy(),
            CopyWait::Waiting,
            "回执没到就什么都不能说"
        );
        tx.send(Ok(())).expect("送回执");
        assert_eq!(
            chrome.tick_copy(),
            CopyWait::Copied,
            "落地那一刻才许说已复制"
        );
        assert!(!chrome.copy_in_flight(), "落定之后不许还占着在飞位");
        assert_eq!(chrome.tick_copy(), CopyWait::Idle, "同一枚只能说一次");
    }

    /// ★ 线程半路没了（只有 panic 会把 `Sender` 丢掉而不发回执）：如实报一条并把闸放开。
    ///
    /// 停在 `Waiting` 就是**静默的永久锁死**——屏幕上不再有下半句，而每次按键都被拒。
    #[test]
    fn a_dead_thread_unarms_the_gate_instead_of_waiting_forever() {
        let chrome = Chrome::new(false);
        let tx = chrome.copy_posted("正在后台复制整张画面（6055×6055）…");
        drop(tx);
        match chrome.tick_copy() {
            CopyWait::Failed(e) => assert!(!e.is_empty(), "得给用户一句原因：{e}"),
            other => panic!("线程没了要说失败，实际 {other:?}"),
        }
        assert!(!chrome.copy_in_flight(), "闸必须放开，下一次还能按");
        assert_eq!(chrome.tick_copy(), CopyWait::Idle);
    }

    /// ★ 被换掉的那枚线程**送不进来**：登记换的是新通道，旧 `Sender` 一送就失败。
    ///
    /// 生产里第二枚根本登记不了（`ui::canvas::copy_display` 先问 [`Chrome::copy_in_flight`]
    /// 并把这一次拒成 `Busy`），所以这条是闸的**兜底**：万一有旧线程还活着，它也不许
    /// 去占用户的剪贴板。失败原因要原样递出去——剪贴板被占用是真会发生的。
    #[test]
    fn a_superseded_receipt_cannot_be_sent_and_a_landing_unarms_the_gate() {
        let chrome = Chrome::new(false);
        let old_tx = chrome.copy_posted("第一次（很大）");
        let new_tx = chrome.copy_posted("第二次（很大）");
        assert!(
            old_tx.send(Err("旧那枚不许说话".into())).is_err(),
            "被顶掉的线程必须送不进来——送进来了就会盖掉新那枚的图"
        );
        assert_eq!(
            chrome.tick_copy(),
            CopyWait::Waiting,
            "活着的那枚还没编完就该一声不出"
        );
        assert!(chrome.copy_in_flight(), "在飞位不许被旧那枚的清掉");
        new_tx
            .send(Err("剪贴板被其他程序占用".into()))
            .expect("送新回执");
        match chrome.tick_copy() {
            CopyWait::Failed(e) => assert!(e.contains("剪贴板"), "原因要原样递给用户：{e}"),
            other => panic!("这一枚是失败，实际 {other:?}"),
        }
        assert!(!chrome.copy_in_flight(), "落定之后闸要放开");
        assert_eq!(chrome.tick_copy(), CopyWait::Idle);
    }

    /// A3 / m64：裸滚轮翻页的节奏闸——**压住再多下也只兜最后那一下的方向，且不丢**。
    ///
    /// 时间口径与 `pan_interval` 同一定式：间隔在测试里钉死、`wheel_due` 手工挪钟，
    /// 不靠 sleep（"这台机器跑得快"从来不是判据）。
    #[test]
    fn wheel_gate_paces_flips_but_never_drops_the_last_one() {
        let chrome = Chrome::new(false);
        chrome.wheel_interval.set(Duration::from_millis(100));
        let past = Instant::now() - Duration::from_millis(1);
        // 闸开着：第一格立刻翻，并把下一次允许的时刻推后一个间隔。
        chrome.wheel_due.set(past);
        assert!(chrome.wheel_gate(1), "闸开着该立刻翻");
        assert_eq!(chrome.tick_wheel_pace(), None, "刚翻完不该还欠着");
        // 紧跟着连滚 5 格：全压在闸里，一次都不许翻，但**欠着一张**。
        for _ in 0..5 {
            assert!(!chrome.wheel_gate(1), "100 ms 内不许再翻");
        }
        assert_eq!(
            chrome.wheel_pending.get(),
            1,
            "被压住的那一下必须还欠着，不能丢"
        );
        // 到点：轮询兜出**恰好一次**，欠的还清（第二次兜是空）。
        chrome.wheel_due.set(past);
        assert_eq!(
            chrome.tick_wheel_pace(),
            Some(1),
            "到点该把欠的那一下兜出去"
        );
        assert_eq!(chrome.tick_wheel_pace(), None, "同一笔欠款不许兜两次");
        // 反向滑行：兜的是最后那一下的方向，不是第一次的。
        chrome.wheel_due.set(past);
        assert!(chrome.wheel_gate(-1));
        assert!(!chrome.wheel_gate(-1));
        assert_eq!(chrome.wheel_pending.get(), -1);
        chrome.wheel_due.set(past);
        assert_eq!(chrome.tick_wheel_pace(), Some(-1));
        // 故意慢滚（间隔大于闸）：每格都当场翻，一个都不该欠着——这条是"闸不能太长"的根据。
        for _ in 0..4 {
            chrome.wheel_due.set(past);
            assert!(chrome.wheel_gate(1));
            assert_eq!(chrome.tick_wheel_pace(), None);
        }
    }

    /// m81：键盘翻页闸——按住自重复压到每个间隔一张，一记一记的手按一个都不许压。
    ///
    /// 时间口径与滚轮闸同一定式：间隔与"上一记的时刻"手工挪钟，不靠 sleep
    /// （"这台机器跑得快"从来不是判据）。
    #[test]
    fn key_gate_paces_holds_and_passes_deliberate_taps() {
        let chrome = Chrome::new(false);
        let pace = Duration::from_millis(350);
        chrome.key_interval.set(pace);

        // 默认常数必须是用户要的那一档（一秒两三张）。这条断言是常数的第二正本：
        // 将来谁把它改成 30 ms 或 2 s，在这里就红，而不是等到有人抱怨手感。
        let hz = 1.0 / KEY_PACE.as_secs_f64();
        assert!(
            hz > 2.0 && hz < 3.5,
            "KEY_PACE 该落在每秒 2–3.5 张，实测 {hz:.2}"
        );

        // ① 手按（上一记在 400 ms 前）：当场翻，且不欠账。
        chrome
            .key_last
            .set(Instant::now() - Duration::from_millis(400));
        chrome.key_last_dir.set(1);
        assert!(chrome.key_gate(1), "间隔不小于闸的手按必须当场翻");
        assert_eq!(chrome.tick_key_pace(), None, "刚翻完不该还欠着");

        // ② 自重复洪流：背靠背连打 10 记（间隔 ~0 ms < 闸），一次都不许当场翻，
        //    但方向必须欠着——丢弃就等于用户按的那一下凭空蒸发。
        for _ in 0..10 {
            assert!(!chrome.key_gate(1), "闸没到点不许翻");
        }
        assert_eq!(chrome.key_pending.get(), 1, "被压住的那一下必须还欠着");

        // ③ 到点：洪流里每满一个间隔仍然能翻一记，且欠账当场清。
        chrome
            .key_due
            .set(Instant::now() - Duration::from_millis(1));
        assert!(chrome.key_gate(1), "到点了洪流里该翻一记");
        assert_eq!(chrome.key_pending.get(), 0, "翻出去就不该还欠着");

        // ④ 欠款由 16 ms 轮询兜：到点兜恰好一次，第二次是空（同一笔不许兜两次）。
        chrome.key_pending.set(1);
        chrome
            .key_due
            .set(Instant::now() - Duration::from_millis(1));
        assert_eq!(chrome.tick_key_pace(), Some(1), "到点该把欠的那一下兜出去");
        assert_eq!(chrome.tick_key_pace(), None, "同一笔欠款不许兜两次");
        // ④' 兜出去那一翻**必须把闸推后一个间隔**：轮询翻的那一张与按键翻的那一张是同一件事。
        //     ★ 这条断言承重（撤掉 `tick_key_pace` 里的顺延就红）：撤掉时实测 40 ms 洪流的上屏
        //     间隔呈 `+353ms / +15ms` 成对——承诺每秒 2.9 张，跑成 5.7 张。
        assert!(!chrome.key_gate(1), "刚兜出去一张，紧接着这一记该被闸压住");

        // ⑤ 换了方向就是新的一记：按住右再按一下左，不该被右那把键压住的闸吃掉。
        chrome.key_last_dir.set(1);
        chrome.key_last.set(Instant::now());
        chrome.key_due.set(Instant::now() + pace);
        assert!(chrome.key_gate(-1), "反向那一下是独立的一记，不该排队");

        // ⑥ 故意慢按（每记间隔都大于闸）连打 5 记：全部当场翻、一次都不欠——
        //    这条是"闸不许吃掉真按"的根据，也是它和"无脑限速"的分界。
        for _ in 0..5 {
            chrome
                .key_last
                .set(Instant::now() - pace - Duration::from_millis(1));
            assert!(chrome.key_gate(1), "间隔大于闸的手按不该被压");
            assert_eq!(chrome.tick_key_pace(), None);
        }
    }

    /// 方向表第二正本：W/A/S/D 的符号必须与"视口往哪边看"一致——符号写反的症状
    /// 是按 W 画面往上飘（图移上去 = 看到下方），越按越离谱。
    #[test]
    fn wasd_signs_match_viewport_directions() {
        let (sx, sy) = Chrome::wasd_signs(0); // W：上看 → 图下移 → pan.y 增
        assert_eq!((sx, sy), (0.0, 1.0));
        let (sx, sy) = Chrome::wasd_signs(1); // A：左看 → 图右移 → pan.x 增
        assert_eq!((sx, sy), (1.0, 0.0));
        let (sx, sy) = Chrome::wasd_signs(2); // S：下看
        assert_eq!((sx, sy), (0.0, -1.0));
        let (sx, sy) = Chrome::wasd_signs(3); // D：右看
        assert_eq!((sx, sy), (-1.0, 0.0));
    }

    /// 幻灯片放映（m167）的到点判定。它**没有自己的线程**——整条功能就是这个 tick 加
    /// 一条 16 ms 轮询里的调用，所以判据必须能在不 sleep 的前提下逐条钉住：
    /// 关态永不发、进入当场发第一张（按下就该看见画面变化，而不是先空等三秒）、
    /// 之后每满一个间隔才发一次、关掉之后闸里不留欠账。
    #[test]
    fn slideshow_ticks_once_per_interval() {
        let chrome = Chrome::new(false);
        let pace = Duration::from_secs(3);
        assert!(!chrome.tick_slideshow(pace), "没在放就不该发");

        assert!(chrome.toggle_slideshow(), "第一次按下去就是开");
        assert!(
            chrome.tick_slideshow(pace),
            "进入放映当场该翻第一张——空等一个间隔会让人以为没生效"
        );
        assert!(
            !chrome.tick_slideshow(pace),
            "刚翻完这一张，下一个间隔没到不许再翻"
        );

        // 挪钟而不是 sleep：与 pan/wheel/key 三条闸同一定式，判据不许靠"这台机器刚好慢"。
        chrome
            .slide_due
            .set(Instant::now() - Duration::from_millis(1));
        assert!(chrome.tick_slideshow(pace), "到点了该发");
        assert!(!chrome.tick_slideshow(pace), "发过一次就重新排程，不许连发");

        assert!(!chrome.toggle_slideshow(), "再按一次是关");
        chrome
            .slide_due
            .set(Instant::now() - Duration::from_millis(1));
        assert!(!chrome.tick_slideshow(pace), "关掉之后到点也不许发");
    }

    /// **m245 隐藏按钮栏**：浮条收窄到只剩信息、仍水平居中；按钮几何与命中一起消失。
    /// 这条钉的是「画没了但还点得着」那类回归——`row` 是几何与命中的唯一入口。
    #[test]
    fn hiding_buttons_leaves_a_centered_info_only_pill() {
        let view = (0.0, 0.0, 1200.0, 800.0);
        let cell = 120.0;
        let _ = set_toolbar_buttons(true);
        let (x, _, w, _) = Chrome::pill(view, false, cell);
        assert!(w > cell + 16.0, "有按钮时胶囊必须更宽：{w}");
        assert_eq!(x, (1200.0 - w) / 2.0, "常态下浮条居中");
        assert_eq!(Chrome::row(false, 1200.0).0.len(), 7);
        assert!(Chrome::btn_rect(Btn::Out, view, false, cell).is_some());

        assert!(set_toolbar_buttons(false), "从开到关应报变化（要标脏）");
        let (x2, _, w2, _) = Chrome::pill(view, false, cell);
        assert_eq!(w2, cell + 16.0, "隐藏后只剩信息格 + 左右 8：{w2}");
        assert_eq!(x2, (1200.0 - w2) / 2.0, "隐藏后仍水平居中");
        assert!(Chrome::row(false, 1200.0).0.is_empty());
        assert!(Chrome::row(true, 1200.0).0.is_empty(), "动图也不给按钮");
        assert!(
            Chrome::btn_rect(Btn::Out, view, false, cell).is_none(),
            "按钮没画出来就不该有命中矩形"
        );
        let chrome = Chrome::new(false);
        let (bx, by, _, _) = Chrome::pill(view, false, cell);
        assert!(
            !matches!(chrome.hit((bx + 4.0, by + 20.0), view), Some(Press::Btn(_))),
            "藏在信息格里的位置不该命中按钮"
        );

        // 模块级镜像必须复位，别污染同进程其它用例（门禁单线程跑）。
        assert!(set_toolbar_buttons(true));
        assert_eq!(Chrome::pill(view, false, cell).2, w, "复位后回到常态宽度");
    }

    /// 用户一插手，放映必须让位。两条手动入口各钉一次（键盘闸、滚轮闸）——
    /// 这条不是贴心，是**防抢图**：定时器与用户翻同一张时，用户刚翻过去的那一张会被
    /// 下一格翻回来，那是最难解释也最难复现的手感 bug。
    ///
    /// ★ 被闸**压住**的那一记也算插手：用户按键的那一下就是意图，哪怕它还欠着没翻。
    #[test]
    fn manual_nav_gates_stop_slideshow() {
        let chrome = Chrome::new(false);
        chrome.toggle_slideshow();
        assert!(chrome.slideshow_on(), "按键之前应在放映");
        chrome.key_gate(1);
        assert!(!chrome.slideshow_on(), "键盘翻页之后必须停");

        let chrome = Chrome::new(false);
        chrome.toggle_slideshow();
        chrome.wheel_gate(1);
        assert!(!chrome.slideshow_on(), "滚轮翻页之后必须停");

        // 没在放时不该发声也不该写日志：这条挂在每一记输入上。
        let chrome = Chrome::new(false);
        chrome.slide_stop();
        assert!(!chrome.slideshow_on());
    }
}
