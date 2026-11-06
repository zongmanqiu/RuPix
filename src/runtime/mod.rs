//! 运行时装配：常驻应用宿主 + 多个看图窗口 + 16ms 兜底轮询。
//!
//! 分层位置：`runtime` 是最顶层，允许引 windui（`ui`/`platform` 之外的最后接线点）。
//! 元素树在 `ui::window`，状态机在 `core::viewer`，解码在 `core::decode` + 解码器层——
//! 本模块只负责把它们接到 windui 的事件循环上。
//!
//! # 一个进程、一个托盘、N 个窗口（2026-11-04 用户裁定）
//!
//! 「多开」的口径是**窗口可以有多个，但进程与系统托盘都只有一个**。所以这里不再有
//! "主窗"这个概念：装配走 windui 的常驻模式（[`App::run_resident`]）——不建主窗，托盘
//! 挂在应用级消息宿主上，界面全部由 [`Window`] 按需建出、关掉即销毁。
//!
//! 由此得到三条硬性质，改代码时别推翻：
//!
//! 1. **每个看图窗口是平等的**：关掉任意一个只销毁它自己，没有"关错了就退程序"。
//! 2. **托盘只有一个**：它属于应用（进程），不属于任何窗口。窗口全关时托盘还在
//!    （`close_to_tray` 开着）或进程直接退出（关着）。
//! 3. **窗口状态必须一窗一份**：viewer / chrome / 解码线程 / 目录监视 / 差分量全在
//!    [`WinRt`] 里；跨窗口唯一的只有 [`Shared`]（配置、设置页句柄、窗口注册表）。
//!
//! # 时序
//!
//! 解码线程在窗口内容树建好之前就必须起好（`Viewer` 构造要它），而结果通道先建；
//! 每个窗口一条解码线程、一份回程（`DecodeSink`：Meta 通道 + 唤醒），互不串台。
//! ★ **契约边界（2026-11-05 裁决：按 DLL 串行，m229 落地）**：每 worker 一份私有后端 ⇒ 多开时
//! 两窗真并行解；同一枚 DLL 靠 per-DLL 有界等待的锁收口。正本 `docs/架构总览.md` §二。
//!
//! ★ 已知代价：m90 那条"回包直接叫醒一帧"的加速在常驻模式下用不了——`App::channel`
//! 的回调拿到的 `EventCtx` 背后没有具体窗口（零主窗），`mark_dirty` 无处可落。回包改由
//! 各窗口自己的 16ms 轮询收（实拍 ≈31 ms 一拍）。翻页上屏因此比单窗时代慢半拍，
//! 换来的是"一个进程、一个托盘"这条用户口径。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use windui::core::EventCtx;
use windui::prelude::*;

use crate::core::config::{clamp_window_size, Config, InitWindow};
use crate::core::decode::{DecodeSink, Decoder, Done, MetaEvent};
use crate::core::viewer::Viewer;
use crate::platform::dir_watch::{DirEvent, DirWatcher};
use crate::ui::canvas::handle_key;
use crate::ui::chrome::{Chrome, CopyWait};
use crate::ui::settings::Hub;
use crate::ui::window::build_root;

/// 兜底轮询的间隔（悬浮层显隐 / 启动动作补刀 / 回包收取 / 动画节拍）。
///
/// 请求值 16 ms 只是名义值：Windows 定时器按 15.6 ms 量化，`SetTimer(16)` 实际约
/// 31 ms 一拍。多窗口模式下这也是**回包上屏的唯一节拍**（见模块头的代价说明）。
const POLL: Duration = Duration::from_millis(16);

/// 窗口标题。所有看图窗口同一个名字：任务栏靠缩略图区分，标题栏是我们自绘的。
const TITLE: &str = "RuPix";

/// 建窗后补刀的启动动作（配置里定不下来的、只能在运行期下发的窗口操作）。
///
/// 为什么是队列而不是两个布尔旗：windui 的 `EventCtx` **只有一个窗口操作槽**
/// （`EventOutcome::window_op: Option<WindowOp>`），同一次分发里第二次请求会把
/// 第一次直接顶掉。启动首帧同时要发"置顶"和"不透明度"，后者把前者盖了——
/// 表现就是"设置里勾了启动时置顶，窗口没置顶、右键菜单里也没勾"。
#[derive(Clone, Copy, PartialEq, Eq)]
enum StartOp {
    Topmost,
    Fullscreen,
}

/// WindowOp 是单槽；返回实际是否提交，调用者据此更新 applied/pending。
fn submit_window_op(
    ctx: &mut EventCtx,
    window_op_sent: &mut bool,
    send: impl FnOnce(&mut EventCtx),
) -> bool {
    if *window_op_sent {
        return false;
    }
    send(ctx);
    *window_op_sent = true;
    true
}

/// 还原固定尺寸只适用于固定尺寸档，不能盖掉自然尺寸档。
fn needs_restored_size(
    was_maximized: bool,
    maximized: bool,
    fullscreen: bool,
    mode: InitWindow,
) -> bool {
    was_maximized && !maximized && !fullscreen && mode == InitWindow::Fixed
}

fn apply_pending_window(
    ctx: &mut EventCtx,
    sent: &mut bool,
    pending: &Cell<Option<(InitWindow, u32, u32)>>,
    applied: &Cell<(InitWindow, u32, u32)>,
    dims: Option<(u32, u32)>,
) -> bool {
    let Some(cur @ (mode, w, h)) = pending.get() else {
        return false;
    };
    let ws = ctx.window_state();
    if *sent || ws.maximized {
        return false;
    }
    let completed = match mode {
        InitWindow::Fullscreen => {
            ws.fullscreen || submit_window_op(ctx, sent, |ctx| ctx.toggle_fullscreen())
        }
        _ if ws.fullscreen => {
            submit_window_op(ctx, sent, |ctx| ctx.toggle_fullscreen());
            false // 退出全屏占一拍；尺寸待办仍在。
        }
        InitWindow::Fixed => {
            let (fw, fh) = crate::platform::window_frame::frameless_frame();
            submit_window_op(ctx, sent, |ctx| {
                ctx.set_window_size(w as i32 - fw, h as i32 - fh)
            })
        }
        InitWindow::Natural => {
            if let Some((w, h)) = dims.filter(|&(w, h)| w > 0 && h > 0) {
                let (w, h) = clamp_window_size(w, h);
                submit_window_op(ctx, sent, |ctx| ctx.set_window_size(w as i32, h as i32))
            } else {
                false // 未解码时不能消费自然尺寸待办。
            }
        }
    };
    if completed {
        pending.set(None);
        applied.set(cur);
    }
    completed
}

/// 关窗裁决的三条去路。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseAction {
    /// 取消这次关闭：既不关窗，也不隐藏（锁定态）。
    Cancel,
    /// 关掉这个窗口，进程继续（还有别的窗口，或「关闭到托盘」要求进程留在托盘）。
    Close,
    /// 关掉这个窗口，并且这是最后一个 → 整个进程退出。
    Quit,
}

/// 关窗裁决（`on_close_request` 的纯逻辑部分）：优先级 = 锁定 > 还有别的窗口 > 托盘 > 退出。
///
/// 单独成函数只为一件事——**可测**。多窗口之后这里多了一条判据（"还有别的窗口"），
/// 而它藏着一个不显眼的坑：若只看 `close_to_tray`，用户关掉三扇窗里的第一扇就会把整个
/// 程序带走。写死在闭包里就没人测得出来。
fn close_action(locked: bool, others_open: bool, close_to_tray: bool) -> CloseAction {
    if locked {
        CloseAction::Cancel
    } else if others_open || close_to_tray {
        // `close_to_tray` = 进程留在托盘（图标常显），窗口本身照样关掉：现状那条
        // "隐藏窗口 + 清图"的老路在多窗口下没法公平表达（隐藏的是哪一扇？），
        // 而它当初换来的是"唤回来是空窗"——现在关窗后再开也是空窗，观感一致。
        CloseAction::Close
    } else {
        CloseAction::Quit
    }
}

/// 把配置里的极简模式差分推给当前窗口；返回窗口态是否真的翻了（要不要重绘）。
///
/// ★ 判据是 `applied`（上次从配置推出去的值），**不是** `chrome.minimal_mode` 现值：
/// 菜单/快捷键改的是当前窗口的临时态、不落盘，拿现值比对会把它每帧覆写回去。
/// 单独成函数是为了能测这条"不覆盖临时切换"的不变量（m51-D）。
fn apply_minimal_mode(chrome: &Chrome, want: bool, applied: &Cell<bool>) -> bool {
    if applied.get() == want {
        return false;
    }
    applied.set(want);
    let changed = chrome.minimal_mode.get() != want;
    chrome.minimal_mode.set(want);
    changed
}

/// 把三个会话态（棋盘格 / 缩略图条 / 锁定）从配置对齐到当前窗口。
///
/// 与极简模式不同，这里**直接拿配置当真值**：三个开关的所有入口（菜单、浮条、
/// 标题栏、键盘）都当场写配置并推设置页（m53-L），不存在"只改窗口不改配置"的
/// 临时态，所以不存在把用户操作每帧覆写回去的风险。
fn apply_session_flags(chrome: &Chrome, cfg: &Config) -> bool {
    let mut changed = false;
    for (slot, want) in [
        (&chrome.checker, cfg.checker),
        (&chrome.filmstrip, cfg.filmstrip),
        (&chrome.locked, cfg.locked),
    ] {
        if slot.get() != want {
            slot.set(want);
            changed = true;
        }
    }
    changed
}

/// 把配置里的看图窗口配色推给渲染令牌（全局 `AtomicU8`），返回**是否真的变了**。
///
/// 与 [`apply_session_flags`] 分开成两个函数，因为它们的真值来源不同：那三个开关是
/// 窗口自己的信号（有"只改窗口不落盘"的临时态需要小心），主题则**只有配置一个真值**，
/// 令牌层是它的一次投影。分开还让测试能单独验"同值不重复重绘"。
///
/// 每帧调它是有意的：`set_theme` 是"同值返回 false"的幂等写，一次 relaxed 原子操作，
/// 比"记住上次推了什么"再比对省心，也不会漏掉任何一条改配置的路径（设置页下拉、
/// 手改 ini 后重载、将来的命令行参数）。
fn apply_theme(cfg: &Config) -> bool {
    crate::ui::tokens::set_theme(crate::ui::tokens::Theme::from_config(cfg.ui_theme))
}

/// 收解码线程的两条回包通道（主画面结果 + 查询元信息），返回是否需要重绘。
///
/// 唤醒 pump 与兜底轮询共用这一份：先到先收，`mpsc` 保证同一条回包只被收走一次。
fn drain_decode(v: &mut Viewer, done_rx: &Receiver<Done>, meta_rx: &Receiver<MetaEvent>) -> bool {
    let mut arrived = false;
    // 一次收干净：连按方向键时可能积压多个结果，只关心最后一个。
    while let Ok(done) = done_rx.try_recv() {
        // 看门狗销账（P1.1 C1）：回包到了，这笔就不算"卡住了"。
        // 过期早退的空串 Err Done 也走这里——它同样是回包，同样销账。
        crate::core::decode_watchdog::note_done(done.seq);
        arrived |= v.apply(done);
    }
    // 查询事件同帧一起收：先 Meta 后 Frame 是常态；Meta 落在 Frame 之后
    // 是解码器时序异常，`apply_meta` 会因 `loaded.is_some()` 忽略它。
    while let Ok(ev) = meta_rx.try_recv() {
        arrived |= v.apply_meta(ev);
    }
    arrived
}

/// 回包落地后推进状态机（防抖重解 / 本地换档 / 动画），返回是否需要重绘。
///
/// `tick` 只在窗口可见时跑：看不见的画面不值得烧 CPU（最小化期间尤其），
/// 恢复可见后 tick 自然把欠下的那份重解发了。`poll_levels` 照收——只是换个
/// 位图、不花钱，漏收会让恢复可见时先闪一下旧档。
fn advance(v: &mut Viewer, visible: bool) -> bool {
    let mut arrived = false;
    if visible {
        arrived |= v.tick();
    }
    arrived |= v.poll_levels();
    arrived
}

// ── 跨窗口共享的那一份 ────────────────────────────────────────

/// 进程内唯一：配置、设置页句柄、窗口注册表、全局任务节流戳。
struct Shared {
    config: Rc<RefCell<Config>>,
    hub: Hub,
    /// 打开着的看图窗口。**弱引用**——注册表不能把窗口吊住：窗口关掉时它的控件树与
    /// 回调要随框架一起释放，解码线程才会跟着停（`Decoder::drop` 关队列）。
    wins: RefCell<Vec<Weak<WinRt>>>,
    /// 窗口 id 发号器（1 起）。
    next_id: Cell<u64>,
    /// 最近一次处于激活态的窗口 id（0 = 一个都没有）。全局任务（安装轮询 / 插件回执 /
    /// 上次异常退出提示）在它的 `ctx` 上跑——那些会弹提示，必须落在用户正看的那扇窗。
    active: Cell<u64>,
    /// 「上次异常退出 → 建议安全模式」的一次性旗。
    hint_pending: Cell<bool>,
    /// 已经下发到托盘的显隐（装配期那份由 `Tray::start_hidden` 定）。
    tray_shown: Cell<bool>,
    /// 启动时刻：工作集裁剪的两刀按它计时。
    started_at: Instant,
    /// 工作集裁剪的调度状态：`trim_stage` 管启动两刀，`idle_trimmed` 管空闲裁剪。
    trim_stage: Cell<u8>,
    idle_trimmed: Cell<bool>,
    last_install_poll: Cell<Instant>,
}

impl Shared {
    /// 清掉已经销毁的窗口，返回仍活着的数量。
    ///
    /// ★ 顺手把 `active` 复位：那个 id 指着的窗口要是正好被关掉了，全局段就再没人跑
    /// （`is_active()` 对所有窗口都假）——安装轮询、插件回执、配置落盘会一起静默停摆。
    /// 置 0 之后下一拍由任意一个窗口的 `tick` 兜底认领。
    fn prune(&self) -> usize {
        let n = {
            let mut v = self.wins.borrow_mut();
            v.retain(|w| w.strong_count() > 0);
            v.len()
        };
        if n > 0 && self.get(self.active.get()).is_none() {
            self.active.set(0);
        }
        n
    }

    fn add(&self, w: &Rc<WinRt>) {
        self.wins.borrow_mut().push(Rc::downgrade(w));
    }

    /// 取一个新的窗口 id（1 起，进程内唯一）。
    fn alloc_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    fn get(&self, id: u64) -> Option<Rc<WinRt>> {
        self.wins
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|w| w.id == id)
    }

    /// 全部活着的窗口，按建窗顺序（先建的在前）。多开时托盘「显示窗口」逐扇唤回要用它。
    ///
    /// ★ 与 [`newest`](Self::newest) 同源：`Weak::upgrade` 成功才算活着。正在关闭的窗口
    /// 此刻仍能升级出来（框架回调还持有它），所以它也会被算进来——这与 `newest` 的既有
    /// 口径一致，不在这里另立判据。
    fn alive(&self) -> Vec<Rc<WinRt>> {
        self.prune();
        self.wins
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect()
    }

    /// 最近的窗口：优先 `active`，其次注册表里最后一个（`wins` 按建窗顺序追加）。
    fn newest(&self) -> Option<Rc<WinRt>> {
        self.prune();
        if let Some(w) = self.get(self.active.get()) {
            return Some(w);
        }
        self.wins
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .next_back()
    }
}

// ── 一个看图窗口 ──────────────────────────────────────────────

/// 一个看图窗口的全部运行期状态。**除 [`Shared`] 之外的一切都在这里**。
struct WinRt {
    id: u64,
    shared: Rc<Shared>,
    viewer: Rc<RefCell<Viewer>>,
    chrome: Rc<Chrome>,
    done_rx: Rc<RefCell<Receiver<Done>>>,
    meta_rx: Rc<RefCell<Receiver<MetaEvent>>>,
    applied_minimal: Cell<bool>,
    applied_stay: Cell<bool>,
    applied_window: Cell<(InitWindow, u32, u32)>,
    applied_opacity: Cell<u8>,
    startup_ops: RefCell<Vec<StartOp>>,
    was_maximized: Cell<bool>,
    natural_pending: Cell<bool>,
    size_settle: Cell<u32>,
    size_seen: Cell<(InitWindow, u32, u32)>,
    window_pending: Cell<Option<(InitWindow, u32, u32)>>,
    restored_size_pending: Cell<bool>,
    watcher: RefCell<Option<DirWatcher>>,
    watched_dir: RefCell<Option<PathBuf>>,
    watch_retry_at: Cell<Instant>,
    last_dir_rescan: Cell<Instant>,
    /// 本窗口自己的目录变化通道（两个窗口开在两个目录时不能互相串）。
    dir_tx: std::sync::mpsc::Sender<DirEvent>,
    dir_rx: RefCell<Receiver<DirEvent>>,
}

impl WinRt {
    /// 建一个看图窗口的全部状态（还没建窗本身：那一步由 [`Self::request`] 交给框架）。
    fn create(shared: Rc<Shared>, initial: Option<PathBuf>) -> Rc<Self> {
        let (done_tx, done_rx) = channel::<Done>();
        let (meta_tx, meta_rx) = channel::<MetaEvent>();
        // 回程里那条唤醒现在是空的（模块头的代价说明）：常驻模式没有可被叫醒的窗口，
        // 回包由本窗 16ms 轮询收。留着这个接口是为了将来框架补上"按窗口投一帧"时
        // 只改这一处，而不是再翻一遍解码层。
        let sink = DecodeSink {
            meta: meta_tx,
            wake: Box::new(|| {}),
        };
        let decoder = Rc::new(Decoder::spawn_with_sink(done_tx, Some(sink)));
        let config = shared.config.clone();
        let viewer = Rc::new(RefCell::new(Viewer::new(decoder, config.clone())));
        let minimal = config.borrow().minimal_mode;
        let chrome = Chrome::new(minimal);
        {
            let c = config.borrow();
            chrome.checker.set(c.checker);
            chrome.filmstrip.set(c.filmstrip);
            chrome.locked.set(c.locked);
        }
        // 尺寸档的初值：装配期那份由 `startup_ops` / `natural_pending` 负责，
        // 故这里取配置值，首帧不会重复下发。
        let applied_window = Cell::new({
            let c = config.borrow();
            (c.init_window, c.window_w, c.window_h)
        });
        let applied_opacity = Cell::new(0u8);
        let startup_ops = RefCell::new({
            let c = config.borrow();
            let mut v = Vec::new();
            if c.stay_on_top {
                v.push(StartOp::Topmost);
            }
            if c.init_window == InitWindow::Fullscreen {
                v.push(StartOp::Fullscreen);
            }
            v
        });
        let has_initial = initial.is_some();
        let natural_pending =
            Cell::new(has_initial && config.borrow().init_window == InitWindow::Natural);
        let now = Instant::now();
        let (dir_tx, dir_rx) = channel::<DirEvent>();
        let me = Rc::new(Self {
            id: shared.alloc_id(),
            shared: shared.clone(),
            viewer,
            chrome,
            done_rx: Rc::new(RefCell::new(done_rx)),
            meta_rx: Rc::new(RefCell::new(meta_rx)),
            applied_minimal: Cell::new(minimal),
            applied_stay: Cell::new(config.borrow().stay_on_top),
            applied_window,
            applied_opacity,
            startup_ops,
            was_maximized: Cell::new(false),
            natural_pending,
            size_settle: Cell::new(0),
            size_seen: Cell::new({
                let c = config.borrow();
                (c.init_window, c.window_w, c.window_h)
            }),
            window_pending: Cell::new(None),
            restored_size_pending: Cell::new(false),
            watcher: RefCell::new(None),
            watched_dir: RefCell::new(None),
            watch_retry_at: Cell::new(now),
            last_dir_rescan: Cell::new(now),
            dir_tx,
            dir_rx: RefCell::new(dir_rx),
        });
        shared.add(&me);
        // 带图启动：裸数据会压入待办队列，由全局段补弹参数窗。
        if let Some(p) = initial {
            crate::ui::raw_dialog::open_path(&me.viewer, &me.shared.config, &p);
        }
        me
    }

    /// 把状态交给框架：一个无边看图窗口（内容树 / 轮询 / 快捷键 / 关窗拦截）。
    fn request(self: &Rc<Self>) -> WindowRequest {
        let (fw, fh) = crate::platform::window_frame::frameless_frame();
        let (w, h) = {
            let c = self.shared.config.borrow();
            (c.window_w as i32 - fw, c.window_h as i32 - fh)
        };
        let tick = self.clone();
        let key = self.clone();
        let close = self.clone();
        let content = self.clone();
        let activation = self.clone();
        let id = self.id;
        let request = Window::new(TITLE, w.max(100), h.max(100))
            .frameless(true)
            .bg(crate::ui::tokens::empty_bg())
            .min_size(100, 100)
            .centered(true)
            // 单例键只用来让托盘"显示窗口"命中一个已开的窗口（命中即激活）。
            .single(format!("viewer-{id}"))
            .icon(crate::ui::brand::app_icon())
            // 前台归属：全局任务（安装轮询 / 插件回执 / 上次异常退出提示）要落在
            // **用户此刻真在看的那一扇**上——那些会弹提示。
            .on_window_activated(move |_ctx, active| {
                if active {
                    activation.shared.active.set(activation.id);
                }
            })
            .on_interval(POLL, move |ctx| tick.tick(ctx))
            .on_shortcut(move |_ctx, ev| key.on_key(&ev))
            .on_close_request(move |ctx| close.close(ctx));
        request.content(move || {
            build_root(
                &content.viewer,
                &content.chrome,
                &content.shared.config,
                &content.shared.hub,
            )
        })
    }

    /// 本窗是否就是"用户此刻在看的那一扇"。全局任务只在这个窗口上跑。
    fn is_active(&self) -> bool {
        self.shared.active.get() == self.id
    }

    /// 16ms 主循环：先全局段（只在活动窗口跑），再窗口段。
    fn tick(self: &Rc<Self>, ctx: &mut EventCtx) {
        // 兜底：进程里第一个跑起来的窗口就是"用户在看的那扇"（`on_window_activated`
        // 只在**变化**时回调，新窗口刚建出来那一下不一定有这一声）。
        if self.shared.active.get() == 0 {
            self.shared.active.set(self.id);
        }
        if self.is_active() {
            self.global_tick(ctx);
        }
        self.window_tick(ctx);
    }

    /// 全局段：跨窗口只该做一次的事，而且要有 `ctx`（会弹提示的）。
    fn global_tick(&self, ctx: &mut EventCtx) {
        let shared = &self.shared;
        let config = shared.config.clone();
        let hub = shared.hub.clone();

        // 安装轮询（下载/卸载进度）：250 ms 一次，提示落在活动窗口上。
        if shared.last_install_poll.get().elapsed() >= Duration::from_millis(250) {
            shared.last_install_poll.set(Instant::now());
            crate::ui::settings::poll_installs(ctx, &config, &hub);
        }
        // 工作集裁剪（对齐 CurSeen）：把字体扫描 / DLL 加载 / 启动后才开的窗口（如
        // 设置）残留的可归还页还回系统，任务管理器数字落回个位数。仅裁可归还页，按需自动
        // 调回，对「常驻后台、间歇交互」的看图器零代价。两层触发：
        //  1) 启动两刀：2s / 10s 各裁一次兜底（首刀在字体扫描 ~0.9s 落定前可能裁不干净）。
        //  2) 空闲裁剪：启动 10s 后，用户空闲 >=1.5s 即裁一次、活动后重置——专门覆盖
        //     「启动后才开的窗口 / 关窗后残留的常驻控件树页」。
        let elapsed = shared.started_at.elapsed().as_millis() as u64;
        let stage = shared.trim_stage.get();
        if (stage == 0 && elapsed >= 2000) || (stage == 1 && elapsed >= 10000) {
            crate::platform::working_set::trim();
            shared.trim_stage.set(stage + 1);
        }
        if elapsed >= 10000 {
            if crate::platform::working_set::idle_millis() >= 1500 {
                if !shared.idle_trimmed.get() {
                    crate::platform::working_set::trim();
                    shared.idle_trimmed.set(true);
                }
            } else {
                shared.idle_trimmed.set(false);
            }
        }
        // 「上次异常退出」提示：只在**首帧**弹一次（`take` 即熄旗）。
        if shared.hint_pending.get() {
            shared.hint_pending.set(false);
            crate::ui::recovery_hint::show(ctx, crate::platform::panic_guard::log_path());
        }
        // 抓手光标按显示器 DPI 复核（脏读一次系统度量，代价可忽略）。
        crate::ui::hand_cursor::refresh_for_dpi();
        // 插件后台调用完成：UI 线程只取结果，不参与启动/等待/终止。
        for job in crate::extensions::poll() {
            match job.result {
                Ok(crate::extensions::run::JobOutput::Text {
                    text,
                    copied,
                    open_url,
                    open_image,
                }) => {
                    crate::ui::plugin_result::show(ctx, &job.id, &job.plugin, &text, copied);
                    // 打开动作在结果窗弹出之后做：先让用户看见"扫到什么"，
                    // 再让系统窗口跳出来；失败只 toast，不撤掉结果。
                    if let Some(url) = open_url {
                        match crate::platform::shell::open_target(&url) {
                            Ok(()) => log::info!("[plugin-open] id={} kind=url", job.id),
                            Err(e) => {
                                log::warn!("[plugin-open] id={} kind=url err={e}", job.id);
                                ctx.toast_err(format!("打开网址失败：{e}"));
                            }
                        }
                    }
                    if let Some((ext_name, bytes)) = open_image {
                        match crate::platform::shell::save_temp_image(&ext_name, &bytes) {
                            Ok(p) => {
                                // 插件回传的图开**新看图窗口**，不交系统关联：
                                // 本机把 `.png` 关联到 RuPix 自己时，二次启动会被单实例
                                // 收走、按多开开关分岔——没开多开就在"当前窗"换图，用户
                                // 正在看的那张就被插件产物顶掉了（2026-11-04 用户实测）。
                                // 宿主自己开新窗：行为与多开开关无关，也不再赌系统关联。
                                open_window(shared, ctx, Some(p));
                                log::info!("[plugin-open] id={} kind=image window=new", job.id);
                            }
                            Err(e) => {
                                log::warn!("[plugin-open] id={} kind=image err={e}", job.id);
                                ctx.toast_err(format!("打开图片失败：{e}"));
                            }
                        }
                    }
                }
                Ok(crate::extensions::run::JobOutput::Settings) => {
                    log::info!("[plugin-settings] id={} verdict=saved", job.id);
                }
                Ok(crate::extensions::run::JobOutput::Edited {
                    path,
                    width,
                    height,
                    rgba,
                    orient,
                }) => {
                    // 回传的是屏上朝向那一份，按**送入时冻结**的 orient 转回源朝向再装。
                    let applied = self
                        .viewer
                        .borrow_mut()
                        .install_edited_displayed(&path, width, height, rgba, orient);
                    if applied {
                        log::info!(
                            "[plugin-edit] id={} file={} size={}x{} rot={} flip={} verdict=applied",
                            job.id,
                            path.display(),
                            width,
                            height,
                            orient.0,
                            orient.1
                        );
                        // 出口位置由插件自己决定（run_ui 在参数窗、老插件在右键菜单），
                        // 这里不指路，免得指错。
                        ctx.toast_ok(format!("已应用「{}」的编辑（原图未覆盖）", job.plugin));
                    } else {
                        log::warn!(
                            "[plugin-edit] id={} file={} verdict=dropped（图已关闭）",
                            job.id,
                            path.display()
                        );
                        ctx.toast_err(format!(
                            "「{}」的编辑结果无法上屏（图已关闭？）",
                            job.plugin
                        ));
                    }
                }
                Err(e) => ctx.toast_err(format!("{}：{}", job.plugin, e)),
            }
        }
        // ★ 解码挂死熔断（2026-11-04）：worker 超时后把"当时正在用的解码器"记进 core 的
        //   熔断集（进程内已生效，`decoders::decode_with` 会挡在调用之前）。
        //
        //   **只在本会话停用，不写 ini**（2026-11-04 修正）。理由是 ABI 口径：
        //   解码器"可重入但不必 MT-safe"，宿主承诺**串行调用**。一次调用挂死不返回时，
        //   那条线程还留在 DLL 里，本进程里再进去一次就是并发违约——所以本会话必须挡住。
        //   但"这个解码器坏了"是**错误的推广**：挂的是这一次调用，进程一退，僵尸没了，
        //   重新启动再试一次是安全且值得的。写进 `decoders_disabled` 会把一次偶发事故
        //   变成"这个格式永远打不开"，而用户根本不知道是自己哪次操作造成的。
        for id in crate::core::decode::take_hung_decoders() {
            crate::decoders::add_disabled(&id);
            log::warn!("[decode-hang] id={id} verdict=disabled-for-this-session");
            ctx.toast_err(format!(
                "解码器「{id}」解图超时，本次运行不再使用它（重启后自动恢复）"
            ));
        }
        // 三振隔离：编排层连续计数达到上限的插件，这里写 ini 并告知用户。
        for id in crate::extensions::run::take_isolations() {
            let name = crate::extensions::discover_configured(&config.borrow())
                .into_iter()
                .find(|e| e.manifest.id == id)
                .map(|e| e.manifest.name)
                .unwrap_or_else(|| id.clone());
            {
                let mut c = config.borrow_mut();
                if !c.extensions_isolated.iter().any(|x| x == &id) {
                    c.extensions_isolated.push(id.clone());
                }
            }
            crate::core::config::mark_dirty();
            log::warn!("插件「{name}」({id}) 连续失败，已自动隔离");
            ctx.toast_err(format!("插件「{name}」连续失败，已隔离（设置→插件可解除）"));
        }
        // 设置窗改动 → 写回配置并落盘（差分轮询：读信号值比对，成本可忽略）。
        // ★ 把 `Handles` **拷出来**再干活，别握着 `Ref`：`if let Some(h) = hub.borrow()…`
        // 的临时量按 Rust 2021 的规矩要活到整个 if-let 体结束，等于让下面这一整段都占着
        // hub 的不可变借用——而其中 `refresh_decoders` 在扫解码器目录、`mark_dirty` 会触发
        // 重绘，任何一处回头 `hub.borrow_mut()`（关窗、重开设置窗）就是同线程 panic。
        // `Handles` 本来就 `Copy`（一组信号句柄），拷出来零成本。
        let hub_handles = hub.borrow().as_ref().copied();
        if let Some(h) = hub_handles {
            let mut c = config.borrow_mut();
            let dirty = h.sync(&mut c);
            h.refresh_labels(&c);
            drop(c);
            if dirty {
                ctx.mark_dirty();
            }
            // 解码器页：只在刚切到那一页时扫一次（不在主循环反复扫）。
            // ★ 一次扫两张卡（解码器列表 + 后缀行），两处口径不会各停在不同时刻。
            h.refresh_decoders(&config.borrow());
        }
        // 看图窗口配色（m200）：设置页换下拉 → 配置变了 → 这里下一帧换令牌并重绘。
        // 只影响浮层那一组颜色；设置页自己的配色是另一个人写的常量，不经过这里。
        if apply_theme(&config.borrow()) {
            ctx.mark_dirty();
        }
        // 配置保存防抖（M1）：UI 侧只置脏，这里到期统一落盘。
        crate::core::config::poll_save(&config.borrow());
        // 空格预览：开关差分（同值零动作）。接管判定/解码/开窗全在哨兵线程。
        crate::platform::spacehook::set_enabled(config.borrow().space_preview);
        // 属性窗跟随当前图：图片切换 / 解码完成时重算并刷新那扇窗。
        crate::ui::settings::refresh_open_metadata(&self.viewer);
    }

    /// 窗口段：本窗口自己的一切。
    fn window_tick(&self, ctx: &mut EventCtx) {
        let config = self.shared.config.clone();
        // 拖拽/命令行打开的裸数据：在这里补弹参数窗（那些入口没有 EventCtx）。
        //
        // ★ 必须放在**窗口段**而不是全局段（m232）：参数窗现在按窗口分格，全局段只有
        //   "活动的那一扇"在跑——于是后台窗口排的裸数据会一直等到它被点中才弹窗，
        //   用户看到的是"我拖进来的文件没反应"。窗口段每扇窗都跑，各取各的队。
        crate::ui::raw_dialog::poll_pending(ctx, &self.viewer, &config);
        // 一次性提示走顶部消息窗，不占浮条的状态格：那一格只放「图片进度 · 缩放百分比
        // [· 帧/页号]」，被一句话顶掉就丢了基础信息（用户 2026-11-06）。取走即清空，
        // 下一拍不会重复弹；每窗一份 Viewer，多开时各弹各的。
        if let Some(s) = self.viewer.borrow_mut().take_note() {
            ctx.toast(s);
        }
        // 启动置顶 / 全屏：一次只发一个，发完出队（见 `StartOp` 的说明）。
        let op = {
            let mut q = self.startup_ops.borrow_mut();
            if q.is_empty() {
                None
            } else {
                Some(q.remove(0))
            }
        };
        let mut window_op_sent = false;
        if let Some(op) = op {
            submit_window_op(ctx, &mut window_op_sent, |ctx| match op {
                StartOp::Topmost => ctx.toggle_topmost(),
                StartOp::Fullscreen => ctx.toggle_fullscreen(),
            });
        }
        // 极简模式即时生效：`sync` 刚把勾选写进配置，这一帧就推给当前窗口。
        let want_minimal = config.borrow().minimal_mode;
        if apply_minimal_mode(&self.chrome, want_minimal, &self.applied_minimal) {
            ctx.mark_dirty();
        }
        // 窗口置顶 / 窗口大小 / 固定尺寸即时生效（m54）：三项原来只影响下次
        // 开窗，设置页贴着「下次启动生效」。置顶只在**配置值**变过时动手，
        // 且按窗口现态决定翻不翻——标题栏 📌 是会话态，配置没变就不碰它。
        let ws = ctx.window_state();
        let mode = config.borrow().init_window;
        if needs_restored_size(self.was_maximized.get(), ws.maximized, ws.fullscreen, mode) {
            self.restored_size_pending.set(true);
        }
        if mode != InitWindow::Fixed {
            self.restored_size_pending.set(false);
        }
        self.was_maximized.set(ws.maximized || ws.fullscreen);
        let want_stay = config.borrow().stay_on_top;
        let stay_due = !window_op_sent && self.applied_stay.get() != want_stay;
        if stay_due
            && (ws.topmost == want_stay
                || submit_window_op(ctx, &mut window_op_sent, |ctx| ctx.toggle_topmost()))
        {
            self.applied_stay.set(want_stay);
        }
        // 尺寸档：下拉换档立即下发；文本框改尺寸等稳定期，避免中途值把窗口
        // 改成一条（`sync` 已经按「解析成功才写配置」过滤掉不完整输入）。
        {
            let (mode, w, h) = {
                let c = config.borrow();
                (c.init_window, c.window_w, c.window_h)
            };
            let last = self.applied_window.get();
            let cur = (mode, w, h);
            if cur != last {
                // 换档立即下发；固定尺寸文本框要等同一个值稳定 40 帧。
                if mode != last.0 {
                    // 用户已经改走别的档：启动期那枚"按首图开窗"的待办必须作废，
                    // 否则首图解码落地时它会抢走唯一的窗口操作位，把用户刚选的尺寸顶掉。
                    if mode != InitWindow::Natural {
                        self.natural_pending.set(false);
                    }
                    self.size_seen.set(cur);
                    self.window_pending.set(Some(cur));
                    self.size_settle.set(0);
                } else if cur != self.size_seen.get() {
                    self.window_pending.set(None);
                    self.size_seen.set(cur);
                    self.size_settle.set(0);
                } else if self.size_settle.get() >= 40 {
                    self.window_pending.set(Some(cur));
                } else {
                    self.size_settle.set(self.size_settle.get() + 1);
                }
            } else {
                self.window_pending.set(None);
                self.size_seen.set(cur);
                self.size_settle.set(0);
            }
        }
        let dims = self.viewer.borrow().info_dims();
        if apply_pending_window(
            ctx,
            &mut window_op_sent,
            &self.window_pending,
            &self.applied_window,
            dims,
        ) {
            self.natural_pending.set(false);
            self.restored_size_pending.set(false);
        }
        // 三个会话态（m53-L）：所有切换入口都当场写配置并推设置页，所以配置
        // 是唯一真值——这里每帧对齐一次，设置页勾「棋盘格」也就即时上屏。
        if apply_session_flags(&self.chrome, &config.borrow()) {
            ctx.mark_dirty();
        }
        // 不透明度：改动才下发（改样式/重算整帧都有成本，每帧重设没必要）。
        //
        // 菜单**不再**需要"展开期间强制 100%"的特判：呈现已改走逐像素
        // alpha（UpdateLayeredWindow），菜单面板由框架的 `opaque_overlays`
        // 整块豁免透明度——窗口保持半透明，只有菜单不透明。
        let want = config.borrow().opacity;
        if want != self.applied_opacity.get()
            && submit_window_op(ctx, &mut window_op_sent, |ctx| ctx.set_window_opacity(want))
        {
            self.applied_opacity.set(want);
        }
        let mut arrived = false;
        // 目录监控是否触发了清单变化（需在借出 viewer 的块外决定要不要 mark_dirty）。
        let mut dir_changed = false;
        {
            let mut v = self.viewer.borrow_mut();
            arrived |= drain_decode(&mut v, &self.done_rx.borrow(), &self.meta_rx.borrow());
            // 目录监控重臂：看图目录变了就换 watcher（新开图 / 参数窗确认 / 空格预览开图
            // 后，当前目录都变了）。比对放在收变化信号之前，保证新 watcher 装上后下一波
            // 变化能被收到。★ 借出来比（m106）：这一格每 16 ms 走一次，而目录几乎从不真变。
            let cur_dir = v.gallery_dir();
            let changed = {
                let watched = self.watched_dir.borrow();
                cur_dir != watched.as_deref()
            };
            if changed && Instant::now() >= self.watch_retry_at.get() {
                let owned = cur_dir.map(|p| p.to_path_buf());
                match owned.as_deref() {
                    Some(d) => {
                        *self.watcher.borrow_mut() = DirWatcher::new(d, self.dir_tx.clone()).ok();
                        if self.watcher.borrow().is_none() {
                            log::warn!("目录监控装配失败：{}", d.display());
                        }
                    }
                    None => *self.watcher.borrow_mut() = None,
                }
                // 装配失败时不要把新目录记成"已监控"：留着旧值，下一拍
                // `cur_dir != watched` 仍成立，就能重试；否则一次瞬时失败
                // （句柄被占、线程耗尽）会让该目录永久失去目录监控。
                if owned.is_none() || self.watcher.borrow().is_some() {
                    *self.watched_dir.borrow_mut() = owned;
                    self.watch_retry_at.set(Instant::now());
                } else {
                    self.watch_retry_at
                        .set(Instant::now() + std::time::Duration::from_secs(5));
                }
            }
            // 收目录变化信号：50ms 节流，一次只重扫一次——把资源管理器里一整波
            // 拷贝合并成一次 `read_dir`。在节流窗口内**不 drain**，让事件留在通道
            // 里，保证最后一波变化一定被扫到（不会因"恰好在窗口里被消费掉"而漏扫）。
            let now = Instant::now();
            if now.duration_since(self.last_dir_rescan.get()).as_millis() >= 50 {
                let mut dirty = false;
                while self.dir_rx.borrow_mut().try_recv().is_ok() {
                    dirty = true;
                }
                if dirty {
                    self.last_dir_rescan.set(now);
                    // `_shared`：同一次文件变动会被每扇窗各报一次，只让第一扇窗真读盘
                    // （清单缓存在 `core::gallery::listing_for_event`，按代次失效）。
                    if v.rescan_dir_shared() {
                        dir_changed = true;
                    }
                }
            }
            // ★ 最小化/不可见期间停止解码调度：看不见的画面不值得烧 CPU——
            //   尤其动画 GIF 会一直拉下一帧、缩放落定后的重解码也在此列。
            //   结果包照收（不积压队列），只是不再发起新的解码请求；
            //   窗口恢复后 tick 自然把欠下的那份重解发了。
            arrived |= advance(&mut v, ctx.window_state().visible);
            // 看门狗巡检（P1.1 C1）：解码线程卡死时这里不会收到回包，
            // 所以巡检放在**兜底轮询**里（而不是回包泵里——那正是不会响的地方）。
            // 到点只记一条 warn，不弹提示：阈值没有现场数据校准过，误报比不报更坏。
            crate::core::decode_watchdog::watchdog_check();
        }
        // 「永远居中」：侦测「最大化/全屏 → 常规」这一次跳变，把窗口复位成
        // 固定尺寸档的配置值并由平台层按工作区居中。`set_window_size` 在
        // 最大化态是空操作，所以必须等还原之后再发；跳变检测保证它只在
        // 真的还原那一帧触发一次。
        let ws = ctx.window_state();
        // 有待办的窗口动作时让路：同一帧只有一个窗口操作位，先发这个会把
        // 队列里那枚（全屏/图片尺寸）顶掉，而且 `window_pending` 已被清空、不会重试。
        if self.restored_size_pending.get()
            && !ws.maximized
            && !ws.fullscreen
            && self.window_pending.get().is_none()
        {
            // 尺寸**当帧从配置读**。过去这里用装配期捕获的那一份，于是「改了固定尺寸
            // → 最大化 → 还原」回到的是**旧**尺寸，新值非得重启才生效（审计 O3）。
            let (fw, fh) = crate::platform::window_frame::frameless_frame();
            let (cw, ch) = {
                let c = config.borrow();
                (c.window_w as i32 - fw, c.window_h as i32 - fh)
            };
            if submit_window_op(ctx, &mut window_op_sent, |ctx| ctx.set_window_size(cw, ch)) {
                self.restored_size_pending.set(false);
            }
        }
        // 图片尺寸档（init_window=3）：启动首图解码完成后按图片原始像素开窗
        // （windui 自带夹工作区+居中；最大化/全屏态它自己会跳过，等还原后
        // 下一轮再补）。翻页不跟随——置旗防重复。
        //
        // 判据改成**状态**（`info_dims` 有值）而不是"本帧有没有收到结果包"：
        // 本帧可能因为启动动作占着唯一的窗口操作位而发不出去（见 `StartOp`），
        // 按帧事件判据那一刀就永久丢了，按状态判据下一帧自然补上。
        if self.natural_pending.get() && !window_op_sent && !ws.maximized && !ws.fullscreen {
            let dims = self.viewer.borrow().info_dims();
            if let Some((w, h)) = dims {
                if w > 0 && h > 0 {
                    let (cw, ch) = clamp_window_size(w, h);
                    if submit_window_op(ctx, &mut window_op_sent, |ctx| {
                        ctx.set_window_size(cw as i32, ch as i32)
                    }) {
                        self.natural_pending.set(false);
                    }
                }
            }
        }
        // 悬浮层自动隐藏（静止 2s 且指针不在贴边区）。真实光标位置作为第二判据
        // 传进去：`last_pos` 可能已是过期记录（leave 通知被拖动/缩放吞掉）。
        let view_rect = self.viewer.borrow().view_rect();
        // ★ 光标只在**极简模式**才要（m107）：`tick_hide` 第一行就在传统模式直接 return，
        //   而默认档恰是传统模式 ⇒ 每拍一次 `GetCursorPos` 纯白花。
        let cursor = if self.chrome.minimal_mode.get() {
            crate::platform::cursor::screen_pos()
        } else {
            None
        };
        if self.chrome.tick_hide(view_rect, cursor) {
            ctx.mark_dirty();
        }
        // 平移限速的补帧（M8）：落在间隔里被挂起的那一帧到点就画出去。
        if self.chrome.tick_pan_pace() {
            ctx.mark_dirty();
        }
        // 滚轮翻页的节奏闸兜底（A3 / m64）。
        if let Some(dir) = self.chrome.tick_wheel_pace() {
            self.viewer.borrow_mut().step(dir);
            ctx.mark_dirty();
        }
        // 键盘翻页的节奏闸兜底（m81）。
        if let Some(dir) = self.chrome.tick_key_pace() {
            self.viewer.borrow_mut().step(dir);
            ctx.mark_dirty();
        }
        // 幻灯片放映（m167）：到点翻一张。间隔每次现读配置。
        let pace = std::time::Duration::from_secs(config.borrow().slide_interval as u64);
        if self.chrome.tick_slideshow(pace) {
            self.viewer.borrow_mut().step(1);
            ctx.mark_dirty();
        }
        // 进度横条**只在换了一格的那一帧**重绘（默认档 = 每间隔 8 次）。
        if self.chrome.slide_step_tick() {
            ctx.mark_dirty();
        }
        // 后台复制的回执（m74）：复制是无痕副作用，成与败都必须由这一帧说出去。
        match self.chrome.tick_copy() {
            CopyWait::Copied => ctx.toast_ok("整张画面已复制（PNG）"),
            CopyWait::Failed(e) => ctx.toast_err(e),
            CopyWait::Waiting | CopyWait::Idle => {}
        }
        if let Some(out) = self.chrome.tick_save() {
            match out {
                Ok(()) => ctx.toast_ok("涂鸦已另存（PNG）"),
                Err(e) => ctx.toast_err(e),
            }
        }
        if arrived || dir_changed {
            ctx.mark_dirty();
        }
    }

    fn on_key(&self, ev: &windui::event::KeyEvent) -> bool {
        if !ev.pressed {
            return false;
        }
        // 注：`ShortcutCtx` 只有"交还焦点/请求关闭"两个动作，开不了文件对话框，
        // 故 Ctrl+O 那类要弹窗的入口放在画布右键菜单里。
        handle_key(
            None,
            // 窗口级兜底没有画布那份 `LiveState`：实时型插件的热键在这里退回"起进程"，
            // 主路径是画布聚焦时的 `on_event`（正常看图就在那里）。
            None,
            &self.viewer,
            &self.chrome,
            &self.shared.config,
            &self.shared.hub,
            ev,
        )
    }

    /// 关窗裁决。返回 `true` = 放行关闭。
    /// 这扇窗真的要走了：把它名下「每窗一份」的东西清掉（m232）。
    ///
    /// 现在只有两处：待弹的裸数据参数（`raw_dialog`）与属性窗信号格（`metadata`）。
    /// 都按 `singleton::window_scope` 分格，所以清理是"摘掉这一格"，不碰别人的。
    fn release_window_scoped(&self) {
        let scope = crate::ui::singleton::window_scope(&self.viewer);
        crate::ui::raw_dialog::forget_window(scope);
        crate::ui::settings::forget_metadata_window(scope);
    }

    fn close(&self, ctx: &mut EventCtx) -> bool {
        let config = self.shared.config.clone();
        crate::core::config::flush_save(&config.borrow());
        let others = self.shared.prune() > 1;
        let act = close_action(
            self.chrome.locked.get(),
            others,
            config.borrow().close_to_tray,
        );
        // 这条判据留一行 **info**：排查"窗口不见了但进程还在"时，第一个要问的就是
        // "到底是哪一支拦下了关窗"，而它只看三个布尔量。
        log::info!(
            "[close] id={} locked={} others={} close_to_tray={} action={:?}",
            self.id,
            self.chrome.locked.get(),
            others,
            config.borrow().close_to_tray,
            act
        );
        match act {
            // 锁定（🔒）吞掉用户侧关窗（Alt+F4 / 系统 ×／自绘 ×，语义：拦截器返回
            // false = 取消）。应用主动关不走本决策链，不受影响。
            CloseAction::Cancel => false,
            // 还有别的窗口 / 关到托盘：关这一扇，进程不动。托盘图标由
            // `close_to_tray` 决定在不在（显隐在应用级轮询里跟随配置）。
            CloseAction::Close => {
                self.release_window_scoped();
                true
            }
            // 最后一扇且没开「关闭到托盘」：连进程一起退。
            // ★ 走补丁 025 的 `quit_app`——常驻模式没有主窗可 `request_close`，
            //   这条是应用级退出的唯一入口（与托盘「退出」同一条路）。
            CloseAction::Quit => {
                ctx.quit_app();
                true
            }
        }
    }
}

pub fn run(initial: Option<PathBuf>) {
    // 图标字形：把系统自带的 Segoe MDL2 Assets 注册为私有区（PUA）回退字体。
    // 注册后所有文本里的 PUA 码位（如设置→解码器页搜索框的 E721 放大镜）自动落到
    // 这个字体渲染——DirectWrite 的系统 fallback 对 PUA 无脚本归属、匹配不到，
    // 必须显式注册（windui 文档口径）。须在 App::run 之前：渲染引擎在窗口创建时
    // 读取注册表快照。注册失败（老系统缺文件）只是图标退化为方框，其余文本不受影响。
    // 兜底不用 "C:\\Windows" 字面量（hygiene_scan 的 abs_path 规则扫盘符+反斜杠）：
    // WINDIR 缺失时用 SYSTEMDRIVE（通常 "C:"，无反斜杠不触发）拼出同一路径；
    // 双缺时退空串，注册失败仅图标退化为方框，与原行为一致。
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| {
        format!(
            "{}\\Windows",
            std::env::var("SYSTEMDRIVE").unwrap_or_default()
        )
    });
    let mdl2 = format!("{windir}\\Fonts\\segmdl2.ttf");
    let _ = windui::text::register_private_use_font(&mdl2, "Segoe MDL2 Assets");
    // 解码栈装配（幂等）：解码器后端 + SVG 渲染器。命令行入口通常已装过，
    // 但直接以库方式起窗（示例/测试）时这里必须自装。
    crate::install_decode_stack();
    // SVG 字体库后台预热（m187）：索引缓存回放 ~11 ms（仅本机首次是整扫），发生在
    // 独立线程上。就绪后首解走完整解析单趟——文字与图形同帧。放手不 join：线程只
    // 读 OnceLock/文件，退出路径零纠缠（§七 Drop 纪律）；就绪位由解码侧按需读。
    std::thread::Builder::new()
        .name("svg-font-warm".into())
        .spawn(crate::ui::svg::warm_fonts)
        .map(std::mem::forget)
        .ok();
    // ⓪ 配置先行：启动置顶与双击行为都从这来。
    let config = Rc::new(RefCell::new(Config::load()));
    // 把落盘的解码器开关灌进全局禁用集（解码线程会读它）。
    crate::decoders::set_disabled(config.borrow().decoders_disabled.clone());
    // 单笔解码超时（挂死恢复，2026-11-04）：0 = 关掉恢复机制（无限等，老行为）。
    crate::core::decode::set_decode_timeout(config.borrow().decode_timeout_s);
    // 各扩展名的用户首选解码器同样灌进全局表（解码线程会读它；失效条目
    // 由 candidate_for_ext 静默回退自动，见 crate::decoders::preferred_candidate）。
    crate::decoders::set_preferred(config.borrow().decoders_preferred.clone());
    // 空格预览（QuickLook 式）：独立哨兵线程（低级键盘钩子 + 原生分层预览窗）。
    // 线程消息泵与主窗显隐解耦——零窗口常驻正是本功能的主场。失败只 warn：功能
    // 静默不可用，其余一切照常。
    if let Err(e) = crate::platform::spacehook::start() {
        log::warn!("空格预览哨兵线程装配失败: {e}");
    }
    let hub: Hub = Rc::new(RefCell::new(None));
    let to_tray = config.borrow().close_to_tray;

    let shared = Rc::new(Shared {
        config: config.clone(),
        hub: hub.clone(),
        wins: RefCell::new(Vec::new()),
        next_id: Cell::new(1),
        active: Cell::new(0),
        hint_pending: Cell::new(
            crate::platform::session::take_prior_aborted() && !crate::decoders::safe_mode(),
        ),
        tray_shown: Cell::new(to_tray),
        started_at: Instant::now(),
        trim_stage: Cell::new(0),
        idle_trimmed: Cell::new(false),
        last_install_poll: Cell::new(Instant::now()),
    });

    // ★ 常驻模式（`run_resident`）：不建主窗。界面全部由 `Window` 建出，
    //   托盘挂在应用级消息宿主上——这正是"一个进程、一个托盘、N 个窗口"的形状。
    //
    // 托盘恒装配（图标按开关显隐）：零窗口时它也是唯一入口（开窗 / 退出），
    // 所以不能等用户勾上「关闭到托盘」才装。
    let mut app = App::resident(TITLE)
        .icon(crate::ui::brand::app_icon())
        .tray(crate::ui::tray::build(
            to_tray,
            config.clone(),
            hub.clone(),
            {
                let shared = shared.clone();
                Rc::new(move |tctx: &mut TrayCtx| {
                    // 多开时逐扇唤回（最近在用的那扇压最上），单窗时就是那一条。
                    for req in wake_requests(&shared, false, None) {
                        tctx.open_window(req);
                    }
                })
            },
            {
                let shared = shared.clone();
                Rc::new(move |tctx: &mut TrayCtx| {
                    for req in wake_requests(&shared, true, None) {
                        tctx.open_window(req);
                    }
                })
            },
        ));
    let tray_handle = app.tray_handle();

    // 二次启动转发：与 `main` 的 `claim_instance` 用同一 `APP_ID`。
    // 常驻模式下没有主窗可激活（`activate(0)` 自行跳过），argv 转发与回调照常——
    // **这正是多开的语义分岔点**：
    //   · `allow_multiple` 关 → 在最近的窗口里换图（历史行为，永远只有一个窗口）；
    //   · `allow_multiple` 开 → 新开一个窗口（同一进程、同一个托盘）。
    let second_tx = app.channel::<Vec<String>>({
        let shared = shared.clone();
        let config = config.clone();
        move |ctx, argv| {
            let path = crate::initial_path(&argv);
            match path {
                Some(p) => {
                    log::info!("二次启动打开：{}", p.display());
                    if config.borrow().allow_multiple {
                        open_window(&shared, ctx, Some(p));
                    } else if let Some(w) = shared.newest() {
                        crate::ui::raw_dialog::open_path(&w.viewer, &config, &p);
                        // 把（可能被别的窗口盖住的）那扇窗提到前台：同一个单例键再开一次，
                        // 平台命中即激活、配置被丢弃。
                        ctx.open_window(w.request());
                    } else {
                        open_window(&shared, ctx, Some(p));
                    }
                }
                None => {
                    log::info!("二次启动：未带文件，仅唤回窗口");
                    for req in wake_requests(&shared, false, None) {
                        ctx.open_window(req);
                    }
                }
            }
        }
    });
    app = app.single_instance(crate::APP_ID, move |argv| {
        let _ = second_tx.send(argv);
    });

    // 应用级轮询（常驻模式下挂在应用级消息宿主上，零窗口也照跑）：
    // 这里只做**不需要窗口 `ctx`** 的事——配置落盘、设置同步、托盘显隐、
    // 零窗口时的退出裁决。要弹提示的那部分在活动窗口的 `global_tick` 里。
    app = app.on_interval(POLL, {
        let shared = shared.clone();
        let config = config.clone();
        let hub = hub.clone();
        let tray_handle = tray_handle.clone();
        move |_ctx| {
            let alive = shared.prune();
            if alive == 0 {
                shared.active.set(0);
            }
            // 托盘图标跟着「关闭到托盘」显隐。**只在翻转时下发**——`set_visible` 会踢一帧。
            let want_tray = config.borrow().close_to_tray;
            if shared.tray_shown.get() != want_tray {
                shared.tray_shown.set(want_tray);
                tray_handle.set_visible(want_tray);
            }
            // 设置窗的改动（零窗口时也可能开着）→ 写配置并落盘。
            let handles = hub.borrow().as_ref().copied();
            if let Some(h) = handles {
                let mut c = config.borrow_mut();
                let _ = h.sync(&mut c);
                drop(c);
            }
            crate::core::config::poll_save(&config.borrow());
            // 主题令牌（零窗口时也要跟上，下一次开窗才是对的配色）。
            let _ = apply_theme(&config.borrow());
            // 空进程兜底：`close_to_tray` 关着却又一个窗口都没有 —— 没窗口就没入口，
            // 留着就是任务管理器里一枚点不动的进程。直接结束（与托盘「退出」同义）。
            if alive == 0 && !config.borrow().close_to_tray {
                windui::platform::post_quit();
            }
        }
    });

    // 启动：先把第一个窗口建出来（带命令行文件），再进消息循环。
    // 走托盘/热键那条旁路队列（`App::start_window` 的同一形状），保证"窗口在进循环
    // 之前就排上"，而不是等第一拍定时器——那会让双击图片后黑屏 30 ms。
    //
    // ★ 必须走 `start_window_request()`，**不能**写成
    //   `let first = WinRt::create(..); app = app.start_window(first.request());`
    //   ——那行局部变量会活到 `run()` 的栈帧结束，也就是 `run_resident()` 返回、
    //   进程退出为止：第一扇窗的运行期状态（图、缩略图条、目录表、解码线程）
    //   因此**永远不释放**。实测后果：关掉它之后 `Shared::newest()` 仍能升级出它，
    //   托盘「显示窗口」把这扇"已经关掉"的窗口原样再开一次，旧图就回来了
    //   （用户报的"关到托盘再打开，图片没清除"正是这个）。
    app = app.start_window(start_window_request(&shared, initial.clone()));

    app.run_resident();

    // 退出兜底（K44）：权威落盘原来**只**挂在 `on_close_request` 上，而托盘「退出」走的是
    // 框架 `quit_app() → DestroyWindow`——那条路不经过 `WM_CLOSE`。放在 `run()` 返回之后：
    // 无论从哪条路退出，落盘必然执行且只此一处。
    crate::core::config::flush_save(&config.borrow());
    // 空格预览的低级钩子挂在系统钩子链上。显式停用把收权从「赌进程消亡时系统替我摘」
    // 变成一句明写的动作；投递走控制线程（摘钩必须在装配它的那条线程上）。
    crate::platform::spacehook::set_enabled(false);
}

/// 托盘 / 二次启动要的那批窗口请求。
///
/// - `force_new == true` → 只有一条：**新建**一扇。
/// - 否则 → 把手上**所有**窗口各提交一次。每条的 `.single` 键（`viewer-{id}`）在平台
///   登记表里命中时只把它 `show_and_activate` 到前台，配置被丢弃（`FnOnce` 内容构建器
///   随之作废），不会重建窗口、也不会丢窗口状态。最近在用的那扇排在**最后一笔**，于是
///   它是最后被激活的，压在最上层。
/// - 一个窗口都没有时退化成新建一扇：零窗口常驻里这是唯一入口。
///
/// ★ 多开之前这里只返回 `newest()` 一条，多开后点托盘只回得来最近那一扇，其余窗口
/// 仍被盖着——用户读到的是"我的窗丢了"。改为逐扇唤回后行为与单窗/零窗口一脉相承：
/// 点托盘 = 手上的图都回来。
fn wake_requests(
    shared: &Rc<Shared>,
    force_new: bool,
    initial: Option<PathBuf>,
) -> Vec<WindowRequest> {
    if force_new {
        return vec![new_window_request(shared, initial)];
    }
    let mut wins = shared.alive();
    if wins.is_empty() {
        return vec![new_window_request(shared, initial)];
    }
    // 最近在用的那扇挪到最后一笔：平台按批内顺序逐个 `show_and_activate`，
    // 最后一笔落地 = 最上层（用户期待的"回到我在看的那扇"）。
    let top = shared.active.get();
    if let Some(i) = wins.iter().position(|w| w.id == top) {
        let w = wins.remove(i);
        wins.push(w);
    }
    log::info!(
        "唤回窗口 {} 扇（最上一扇 id={}）",
        wins.len(),
        wins.last().map(|w| w.id).unwrap_or(0)
    );
    wins.iter().map(|w| w.request()).collect()
}

/// 新建一扇并交出它的开窗请求。
fn new_window_request(shared: &Rc<Shared>, initial: Option<PathBuf>) -> WindowRequest {
    let w = WinRt::create(shared.clone(), initial);
    log::info!("新开看图窗口 id={}（现有 {} 个）", w.id, shared.prune());
    w.request()
}

/// 建出**启动窗口**并把它的开窗请求交回去。
///
/// 存在的唯一理由是把窗口状态的作用域关在函数里：`WinRt` 只能被框架的窗口回调持有，
/// 调用方手里不该留任何 `Rc`。写成一个函数而不是内联，是为了让"要不要留一份"
/// 这件事**写不出来**——2026-11-04 那个 `let first = ...` 就是留了一份（K189）。
fn start_window_request(shared: &Rc<Shared>, initial: Option<PathBuf>) -> WindowRequest {
    let w = WinRt::create(shared.clone(), initial);
    log::info!("启动窗口 id={}", w.id);
    // `request()` 之后这里就是最后一个强引用，返回即释放；此后窗口状态由框架那侧的
    // 回调持有，窗口关掉就连同它一起销毁。
    w.request()
}

/// 开一个新的看图窗口（不看注册表，永远新建）。
fn open_window(shared: &Rc<Shared>, ctx: &mut EventCtx, initial: Option<PathBuf>) {
    ctx.open_window(new_window_request(shared, initial));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::UiTheme;

    #[test]
    fn host_audit_window_slot_preserves_first_and_retries_next_tick() {
        let pending = Cell::new(true);
        let mut sent = false;
        let first = windui::testing::run_with_ctx(|ctx| {
            assert!(submit_window_op(ctx, &mut sent, |ctx| ctx.toggle_topmost()));
            if submit_window_op(ctx, &mut sent, |ctx| ctx.set_window_opacity(80)) {
                pending.set(false);
            }
        });
        assert!(pending.get(), "被占槽的动作必须保留待办");
        assert!(matches!(
            first.window_op,
            Some(windui::event::WindowOp::ToggleTopmost)
        ));
        sent = false;
        let next = windui::testing::run_with_ctx(|ctx| {
            if submit_window_op(ctx, &mut sent, |ctx| ctx.set_window_opacity(80)) {
                pending.set(false);
            }
        });
        assert!(next.window_op.is_some());
        assert!(!pending.get());
    }

    #[test]
    fn host_audit_natural_restore_never_requests_fixed_size() {
        assert!(!needs_restored_size(
            true,
            false,
            false,
            InitWindow::Natural
        ));
        assert!(needs_restored_size(true, false, false, InitWindow::Fixed));
        assert!(!needs_restored_size(true, true, false, InitWindow::Fixed));
    }

    /// 关窗三条去路与优先级：锁定 > 还有别的窗口 > 关闭到托盘 > 退出。
    ///
    /// 第二组断言是这条测试存在的理由：多窗口之后，"关掉三扇窗里的第一扇"绝不能
    /// 把整个程序带走——那正是老实现（隐藏窗口 + 单窗口模型）在用户手里炸出来的样子。
    #[test]
    fn close_decision_keeps_the_process_alive_while_other_windows_open() {
        assert_eq!(
            close_action(true, true, true),
            CloseAction::Cancel,
            "锁定态只取消，不得顺手关掉"
        );
        assert_eq!(close_action(true, false, true), CloseAction::Cancel);
        assert_eq!(
            close_action(false, true, false),
            CloseAction::Close,
            "还有别的窗口时，关这一扇不该退进程（哪怕没开托盘）"
        );
        assert_eq!(
            close_action(false, false, true),
            CloseAction::Close,
            "最后一扇 + 关闭到托盘：窗口关掉，进程留在托盘"
        );
        assert_eq!(
            close_action(false, false, false),
            CloseAction::Quit,
            "最后一扇且没开托盘：连进程一起退"
        );
    }

    /// 极简模式的两层态：菜单/快捷键是**当前窗口的临时切换**（配置不变），
    /// 设置页勾选是配置值。轮询只许在**配置值**变过时推——拿 chrome 现值比
    /// 会把临时切换每帧推回去（m51-D 的核心不变量）。
    #[test]
    fn minimal_mode_sync_keeps_the_menu_toggle_until_config_changes() {
        let chrome = Chrome::new(false);
        let applied = Cell::new(false);

        // 菜单把当前窗口临时切成极简：配置没变，这一帧不许推回去。
        chrome.minimal_mode.set(true);
        assert!(!apply_minimal_mode(&chrome, false, &applied));
        assert!(
            chrome.minimal_mode.get(),
            "配置值未变时，轮询不许覆盖菜单的临时切换"
        );

        // 设置页把配置翻成开：推进 applied（窗口已经是极简，不需要重绘）。
        assert!(!apply_minimal_mode(&chrome, true, &applied));
        assert!(applied.get(), "配置值必须被记住，下一次翻转才推得动");

        // 取消勾选：当场退出极简，并报告需要重绘。
        assert!(apply_minimal_mode(&chrome, false, &applied));
        assert!(!chrome.minimal_mode.get(), "取消勾选要当场生效");
        assert!(!applied.get());
    }

    /// 三个会话态从配置对齐到窗口：设置页勾上棋盘格的那一帧就上屏；
    /// 没变化时不报重绘（静止省电的老规矩）。
    #[test]
    fn session_flags_follow_config() {
        let chrome = Chrome::new(false);
        let mut cfg = Config::default();
        assert!(!apply_session_flags(&chrome, &cfg), "全默认不该报重绘");

        cfg.checker = true;
        cfg.filmstrip = true;
        cfg.locked = true;
        assert!(apply_session_flags(&chrome, &cfg), "翻转要报重绘");
        assert!(chrome.checker.get() && chrome.filmstrip.get() && chrome.locked.get());
        assert!(!apply_session_flags(&chrome, &cfg), "已对齐后不该反复重绘");

        cfg.checker = false;
        assert!(apply_session_flags(&chrome, &cfg));
        assert!(!chrome.checker.get(), "取消勾选棋盘格要当场生效");
        assert!(chrome.filmstrip.get(), "其余两位不该被顺手改掉");
    }

    /// 看图窗口配色随配置推给渲染令牌：首次报重绘、同值不重复报、换档立刻生效。
    ///
    /// ★ 令牌是**全局** `AtomicU8`，本用例一定把状态复原——单线程跑（仓规约
    ///   `--test-threads=1`）时它不会和别的用例交叉，但复原是不给别人留坑的底线。
    #[test]
    fn theme_follows_config_and_reports_once() {
        // ★ 不假设"这台机器此刻是什么主题"：tokens 那边有它自己的用例，跑在前面的
        //   已经改过全局值。先把令牌钉到深色，后面的断言才与执行顺序无关。
        crate::ui::tokens::set_theme(crate::ui::tokens::Theme::Dark);
        let mut cfg = Config::default();
        assert!(!apply_theme(&cfg), "已经一致时不该报重绘");
        assert!(!apply_theme(&cfg), "同值不该反复重绘");
        assert_eq!(
            crate::ui::tokens::theme(),
            crate::ui::tokens::Theme::Dark,
            "默认档必须是深色"
        );

        cfg.ui_theme = UiTheme::Light;
        assert!(apply_theme(&cfg), "换主题要报重绘");
        assert_eq!(
            crate::ui::tokens::theme(),
            crate::ui::tokens::Theme::Light,
            "下拉换成浅色后令牌必须跟着换"
        );

        // 复原：全局状态不留给后面的用例。
        cfg.ui_theme = UiTheme::Dark;
        assert!(apply_theme(&cfg));
        assert_eq!(crate::ui::tokens::theme(), crate::ui::tokens::Theme::Dark);
    }
}
