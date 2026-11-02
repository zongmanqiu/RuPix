//! 运行时装配：App 构建 + 主画面回包唤醒 + 16ms 兜底轮询（悬浮层显隐 / 启动动作补刀）。
//!
//! 分层位置：`runtime` 是最顶层，允许引 windui（`ui`/`platform` 之外的最后接线点）。
//! 元素树在 `ui::window`，状态机在 `core::viewer`，解码在 `core::decode` + 解码器层——
//! 本模块只负责把它们接到 windui 的事件循环上。
//!
//! 装配顺序有讲究：**解码线程要在建 `Viewer` 之前起好**，而结果通道要先建。
//! 结果数据仍走 `std::sync::mpsc`（解码线程起在 `App` 之前，拿不到 `App` 的 `Sender`）；
//! 但**主画面回包**额外经一枚 `App::channel::<()>` 标记叫醒 UI —— 只靠 `on_interval`
//! 收包，回包要等下一拍定时器（实测 Windows 定时器按 15.6 ms 量化，请求的 16 ms
//! 实际 ≈31 ms 一拍，m90）。缩略图/预读不叫醒：那是后台填充，不值得让静止窗口白出帧。
//! 定时器保留为**兜底**：窗口隐藏/最小化时没有帧、通道 pump 不跑，回包仍由它收。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use windui::core::EventCtx;
use windui::prelude::*;

use crate::core::config::{clamp_window_size, Config, InitWindow};
use crate::core::decode::{install_done_waker, install_meta_sender, Decoder, Done, MetaEvent};
use crate::core::viewer::Viewer;
use crate::platform::dir_watch::{DirEvent, DirWatcher};
use crate::ui::canvas::handle_key;
use crate::ui::chrome::{Chrome, CopyWait};
use crate::ui::settings::Hub;
use crate::ui::window::{build_root, DARK};

/// 兜底轮询的间隔（悬浮层显隐 / 启动动作补刀 / 隐藏窗口时的回包收取）。
///
/// **不再是主画面回包的上屏延迟来源**：主画面回包由
/// [`crate::core::decode::install_done_waker`] 直接叫醒一帧（m90）。这里保留它做三件事：
/// 窗口隐藏时没有帧、通道 pump 不跑，回包仍要有人收；悬浮层显隐与启动两刀这类
/// "没有事件也要推进"的检查；以及动画 / 防抖重解的节拍。
///
/// 请求值 16 ms 只是名义值：Windows 定时器按 15.6 ms 量化（`Sleep(1)` ×30 实测中位
/// 15.24 ms），`SetTimer(16)` 实际约 31 ms 一拍。兜底职责不依赖这个精度。
const POLL: Duration = Duration::from_millis(16);

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
    /// 隐藏进系统托盘（「关闭到托盘」开着）。
    Hide,
    /// 真退出。
    Exit,
}

/// 关窗裁决（`on_close_request` 的纯逻辑部分）：优先级 = 锁定 > 托盘 > 退出。
///
/// 单独成函数只为一件事——**可测**。这个优先级里藏着一个不显眼的坑：锁定态若落成
/// `Hide`，被锁的窗口会在用户点 × 时**悄悄收进托盘**（预期是"点了没反应"），而窗口
/// 藏起来之后用户根本不知道去哪找它。写死在闭包里就没人测得出来。
fn close_action(locked: bool, close_to_tray: bool) -> CloseAction {
    if locked {
        CloseAction::Cancel
    } else if close_to_tray {
        CloseAction::Hide
    } else {
        CloseAction::Exit
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
/// `tick` 只在窗口可见时跑：看不见的画面不值得烧 CPU（收进托盘期间尤其），
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
    // 各扩展名的用户首选解码器同样灌进全局表（解码线程会读它；失效条目
    // 由 candidate_for_ext 静默回退自动，见 crate::decoders::preferred_candidate）。
    crate::decoders::set_preferred(config.borrow().decoders_preferred.clone());
    // 空格预览（QuickLook 式）：独立哨兵线程（低级键盘钩子 + 原生分层预览窗）。
    // 不注册热键、不动钩子；开关由 16ms 轮询差分发 `set_enabled`（见下方间隔回调）。
    // 线程消息泵与主窗显隐解耦——收进托盘正是本功能的主场。失败只 warn：功能
    // 静默不可用，其余一切照常。方案正本见 本仓历史记录
    if let Err(e) = crate::platform::spacehook::start() {
        log::warn!("空格预览哨兵线程装配失败: {e}");
    }
    let hub: Hub = Rc::new(RefCell::new(None));

    // ① 结果通道 → ② 解码线程 → ③ 状态机（顺序不可换）。
    let (done_tx, done_rx) = channel::<Done>();
    // 查询阶段的元信息通道：解码线程在 `iv_render(out=NULL)` 成功后先发 Meta、
    // 再进渲染。UI 拿到 Meta 就让 info bar / 「按原始像素开窗」提前动一步，
    // Frame 到了再按 `Decoded.natural` 校准（契约：查询值仅预览，渲染值为准）。
    // 通道未装 = 探针/单测路径，`publish_meta` 静默 no-op。
    let (meta_tx, meta_rx) = channel::<MetaEvent>();
    install_meta_sender(meta_tx);
    // 接收端包一层 `Rc<RefCell<…>>`：唤醒 pump 与兜底轮询两条路都要收同一个通道
    // （先到先收，`mpsc` 保证不重不漏），而 `Receiver` 不能 clone。
    let done_rx = Rc::new(RefCell::new(done_rx));
    let meta_rx = Rc::new(RefCell::new(meta_rx));
    // 目录监控通道：watcher 线程往 `dir_tx` 发「目录变了」，16ms 轮询从 `dir_rx` 收。
    // 去抖（50ms）在轮询里做，一次 tick 只重扫一次。
    let (dir_tx, dir_rx) = channel::<DirEvent>();
    let watcher: std::cell::RefCell<Option<DirWatcher>> = std::cell::RefCell::new(None);
    let watched_dir: std::cell::RefCell<Option<PathBuf>> = std::cell::RefCell::new(None);
    // 目录监控装配失败后的下次重试时刻：失败不写 `watched_dir`，但也不能每 16ms
    // 重开一次句柄、刷一条 warn——失败后退 5 秒再试。
    let watch_retry_at = Cell::new(Instant::now());
    let decoder = Rc::new(Decoder::spawn_with(done_tx));
    let viewer = Rc::new(RefCell::new(Viewer::new(decoder, config.clone())));
    let minimal = config.borrow().minimal_mode;
    let chrome = Chrome::new(minimal);
    {
        // 三个会话态（m53-L）随配置恢复：上次关窗时开着棋盘格/缩略图条/锁定，
        // 这次启动照样开着。锁定也恢复——解锁钮仍在标题栏，不会把人锁死。
        let c = config.borrow();
        chrome.checker.set(c.checker);
        chrome.filmstrip.set(c.filmstrip);
        chrome.locked.set(c.locked);
    }
    // 设置页「极简模式」勾选要当场作用于当前窗口（m51-D）。记的是**上次从配置推给
    // chrome 的值**，不是 chrome 现值：菜单/快捷键的临时切换不回写配置，拿现值比对
    // 会把它每帧覆写掉。
    let applied_minimal = Cell::new(minimal);

    // 设置页「窗口置顶 / 窗口大小 / 固定尺寸」当场生效（m54）：记的是**上次从
    // 配置推给窗口的值**，不是窗口现态——标题栏 📌、F 全屏改的是会话态，按现态
    // 每帧对齐会把它们覆写回去。装配期那份由 `startup_ops` / `natural_pending`
    // 负责，故初值直接取配置，首帧不会重复下发。
    let applied_stay = Cell::new(config.borrow().stay_on_top);
    let applied_window = Cell::new({
        let c = config.borrow();
        (c.init_window, c.window_w, c.window_h)
    });
    // 固定尺寸文本框是边打边写配置的：**同一个值**稳定 40 帧（≈0.64 s）再下发，
    // 否则输到「1200, 6」就会被 clamp 成 1200×100 并当场把窗口改成一条。
    // `size_seen` 记上一帧看到的值——`size_settle` 只在"本帧与上帧相同"时数数，
    // 用户还在敲字就重新计时，不是"从第一次偏离数满 40 帧"。
    let size_settle = Cell::new(0u32);
    let size_seen = Cell::new({
        let c = config.borrow();
        (c.init_window, c.window_w, c.window_h)
    });
    // 待下发的尺寸档。全屏退出与改尺寸不能挤在同一次分发里（windui 只有一个
    // 窗口操作槽），所以退出全屏那一帧把它留到下一帧。
    let window_pending: Cell<Option<(InitWindow, u32, u32)>> = Cell::new(None);

    // 是否带图启动（`initial` 下面就被 move，先取快照）。
    let has_initial = initial.is_some();

    if let Some(p) = initial {
        // 裸数据会压入待办队列，由下面的 interval 补弹参数窗。
        crate::ui::raw_dialog::open_path(&viewer, &config, &p);
    }

    let root = build_root(&viewer, &chrome, &config, &hub);

    // 「启动时置顶 / 启动即全屏」没法在建窗前声明（WindowOp 是运行时动作）：
    // 建窗后由 16ms 轮询补刀，**一帧只发一个**（见 [`StartOp`] 上的说明）。
    //
    // 置顶取的是 `stay_on_top` 这一**默认值**：它决定每个新开的看图窗口要不要置顶，
    // 与右键菜单那次"只改当前窗口"的临时切换互不相干（菜单那次不落盘）。
    let startup_ops: Rc<RefCell<Vec<StartOp>>> = Rc::new(RefCell::new({
        let c = config.borrow();
        let mut v = Vec::new();
        if c.stay_on_top {
            v.push(StartOp::Topmost);
        }
        if c.init_window == InitWindow::Fullscreen {
            v.push(StartOp::Fullscreen);
        }
        v
    }));
    // 上一帧窗口是否处于最大化/全屏：用来侦测「最大化→还原」这一次跳变，
    // 还原后把窗口复位成固定尺寸档的配置值并居中（见间隔回调里的处理）。
    let was_maximized = Cell::new(false);
    // 图片尺寸档：启动首图解码完成后按图片原始像素开窗（一次性，翻页不跟随；
    // 无参启动保持空白窗，不适用——见用户拍板 2026-10-10）。尺寸会先夹进
    // [WINDOW_MIN, WINDOW_MAX]（下限 100×100），再由平台夹到显示器工作区。
    let natural_pending =
        Cell::new(has_initial && config.borrow().init_window == InitWindow::Natural);
    // 首包之后记录**已经下发到窗口**的不透明度：配置被设置窗改过才需要重新下发。
    let applied_opacity = Rc::new(Cell::new(0u8));

    // 可见窗口 = 固定尺寸档配置值（load 时已 clamp 进 100..=8192）：请求尺寸先
    // 扣掉无边框窗口那圈不可见的系统边框。
    let (fw, fh) = crate::platform::window_frame::frameless_frame();
    let (win_w, win_h) = {
        let c = config.borrow();
        (c.window_w as i32 - fw, c.window_h as i32 - fh)
    };
    // 「关闭到托盘」不再要求重启：关闭请求在 `on_close_request` 里**按当前配置**裁决
    // （见链尾），托盘则**恒装配**、图标随开关显隐——窗口一旦藏起来就只能靠托盘唤回，
    // 托盘不在场等于唤不回来，所以不能等到用户勾上才去装。
    //
    // 框架的 `App::hide_on_close` 用不上：它是装配期定死的布尔，"后来才勾上"这条路上
    // 它既不知道、也没法改（装配期也没有托盘可指）。
    let to_tray = config.borrow().close_to_tray;
    // 已经下发到托盘的显隐（装配期那份由 `Tray::start_hidden` 定），用来判"开关真的翻了"。
    let tray_shown = Cell::new(to_tray);

    // 「上次异常退出 → 本次启动建议安全模式」的**一次性触发**旗：
    // 会话戳（`temp\rupix.run`）残留 = 上次进程没走到 `close_session`。这里在**建窗后、
    // 首帧 interval** 里才弹提示（`EventCtx` 只在那时才有），弹完立即熄旗。
    // `--safe-mode` 命令行启动直接不弹——用户已经用命令行止损过一次，弹窗等于重复提问。
    let hint_pending =
        Cell::new(crate::platform::session::take_prior_aborted() && !crate::decoders::safe_mode());

    let mut app = App::new("RuPix", win_w, win_h)
        // ★ 自有图标（靛蓝底板 + 山峰）。此前是 `windui::icon::brand_icon()`——
        //   那是**框架**自带的天蓝 W 图标，不是 RuPix 的脸，详见 `ui::brand` 模块头。
        .icon(crate::ui::brand::app_icon())
        .frameless()
        .bg(DARK)
        .min_size(100, 100)
        .centered()
        // 开关关着时用 `start_hidden`：图标连登记都不做，通知区不会出现它。
        .tray(crate::ui::tray::build(to_tray, config.clone(), hub.clone()));
    // 显隐托盘图标的运行期句柄（下面的轮询按开关翻转时下发）。
    let tray_handle = app.tray_handle();

    // 单实例 + 二次运行传参：与 `main` 的 `claim_instance` 用同一 `APP_ID`。
    // 二次实例（双击另一张图 / 再点一次图标）把整段 argv 经框架转发到这里；
    // 本回调无 EventCtx（跑在独立的 message-only 窗口 wndproc 里），故只把 argv
    // 交给通道，由下一帧的 UI 线程在安全上下文里开图。框架在 `on_second` 返回后会
    // 自动把（可能藏在托盘里的）主窗口 ShowWindow 出来并提到前台——所以**永远只有一个
    // 窗口、一个托盘**，多开被彻底挡在进程外。
    let second_tx = app.channel::<Vec<String>>({
        let viewer = viewer.clone();
        let config = config.clone();
        move |ctx, argv| {
            if let Some(p) = crate::initial_path(&argv) {
                crate::ui::raw_dialog::open_path(&viewer, &config, &p);
            }
            // 兜底：无论带不带文件，都确保窗口在前台（框架已自动 ShowWindow，双保险）。
            ctx.show_window();
        }
    });
    app = app.single_instance(crate::APP_ID, move |argv| {
        let _ = second_tx.send(argv);
    });

    // 主画面回包 → 立刻唤醒 UI（m90）。
    //
    // 数据仍走 `done_rx`（解码线程起在 `App` 之前，拿不到 `Sender`）；这条通道只是
    // 「有回包了」的一枚标记：`Sender::send` 会投一帧 `WM_APP_WAKE`，宿主在
    // `begin_frame` 里**先排空通道再渲染**，于是回包在这一帧就上屏，而不是等下一拍
    // 定时器（实测 ≈31 ms）。
    //
    // `advance` 必须跟着跑：`tick` 负责防抖后的升级重解与本地换档，收完回包不推进
    // 状态机就会停在"解出来了却不再升级"。
    let wake_tx = app.channel::<()>({
        let v = viewer.clone();
        let done_rx = done_rx.clone();
        let meta_rx = meta_rx.clone();
        move |ctx, ()| {
            let visible = ctx.window_state().visible;
            let arrived = {
                let mut v = v.borrow_mut();
                let mut arrived = drain_decode(&mut v, &done_rx.borrow(), &meta_rx.borrow());
                arrived |= advance(&mut v, visible);
                arrived
            };
            if arrived {
                ctx.mark_dirty();
            }
        }
    });
    install_done_waker(Box::new({
        let wake_tx = wake_tx.clone();
        move || {
            let _ = wake_tx.send(());
        }
    }));
    // 装配期可能已经有回包落地（启动首图在 `App` 建好前就发了请求）：补一枚标记，
    // 保证它随第一帧上屏，而不是等第一拍兜底轮询。无回包时这一帧什么都不做。
    let _ = wake_tx.send(());

    // 空格预览成品投递：2026-10-17 起整条链（判定→解码→开窗）住进
    // platform::spacehook 的常驻线程，与 windui 的 channel/帧泵再无关系——
    // 一轮经 `App::channel` 回 UI 开单例窗的写法，在"主窗收进托盘、通道停摆"
    // 的实测里被判死（模块头有全记录）。
    app.on_interval(POLL, {
        let v = viewer.clone();
        let chrome = chrome.clone();
        let config = config.clone();
        let hub = hub.clone();
        let applied_opacity = applied_opacity.clone();
        let applied_stay = applied_stay.clone();
        let applied_window = applied_window.clone();
        let size_settle = size_settle.clone();
        let size_seen = size_seen.clone();
        let window_pending = window_pending.clone();
        let was_maximized = was_maximized.clone();
        let natural_pending = natural_pending.clone();
        let startup_ops = startup_ops.clone();
        let tray_handle = tray_handle.clone();
        let tray_shown = tray_shown.clone();
        let hint_pending = hint_pending.clone();
        // 工作集裁剪的调度状态（见下方间隔回调内的裁剪逻辑）：`trim_stage` 管启动两刀，
        // `idle_trimmed` 管空闲裁剪（一次空闲只裁一刀，用户活动后重置）。
        let trim_stage = Cell::new(0u8);
        let idle_trimmed = Cell::new(false);
        // 目录监控重扫的节流戳：限制重扫频率（见下方间隔回调内目录监控段）。
        let last_dir_rescan = Cell::new(Instant::now());
        let last_install_poll = Cell::new(Instant::now());
        let restored_size_pending = Cell::new(false);
        let started_at = Instant::now();
        move |ctx| {
            if last_install_poll.get().elapsed() >= Duration::from_millis(250) {
                last_install_poll.set(Instant::now());
                crate::ui::settings::poll_installs(ctx, &config, &hub);
            }
            // 工作集裁剪（对齐 CurSeen）：把字体扫描 / DLL 加载 / 启动后才开的窗口（如
            // 设置）残留的可归还页还回系统，任务管理器数字落回个位数。仅裁可归还页，按需自动
            // 调回，对「常驻后台、间歇交互」的看图器零代价。两层触发：
            //  1) 启动两刀：2s / 10s 各裁一次兜底（首刀在字体扫描 ~0.9s 落定前可能裁不干净）。
            //  2) 空闲裁剪（修复「设置窗 10MB」）：启动 10s 后，用户空闲 >=1.5s 即裁一次、
            //     活动后重置——专门覆盖「启动后才开的窗口 / 关窗后残留的常驻控件树页」，而这
            //     正是原先两刀永远覆盖不到的（两刀在启动 10s 后就停止，之后再开后开的窗没人裁）。
            let elapsed = started_at.elapsed().as_millis() as u64;
            let stage = trim_stage.get();
            if (stage == 0 && elapsed >= 2000) || (stage == 1 && elapsed >= 10000) {
                crate::platform::working_set::trim();
                trim_stage.set(stage + 1);
            }
            if elapsed >= 10000 {
                if crate::platform::working_set::idle_millis() >= 1500 {
                    if !idle_trimmed.get() {
                        crate::platform::working_set::trim();
                        idle_trimmed.set(true);
                    }
                } else {
                    idle_trimmed.set(false);
                }
            }
            // 「上次异常退出」提示：只在**首帧**弹一次（`take` 即熄旗），不阻塞下面
            // 常规窗口动作——弹窗是 `open_window`，与 `WindowOp` 单槽是两条路径。
            if hint_pending.get() {
                hint_pending.set(false);
                crate::ui::recovery_hint::show(ctx, crate::platform::panic_guard::log_path());
            }
            // 拖拽/命令行打开的裸数据：在这里补弹参数窗（那些入口没有 EventCtx）。
            crate::ui::raw_dialog::poll_pending(ctx, &v, &config);
            // 插件后台调用完成：UI 线程只取结果，不参与启动/等待/终止。
            // 命令型文本进结果窗；编辑型回写套成 viewer 编辑覆盖（原图不覆盖）。
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
                            match crate::platform::shell::save_temp_image(&ext_name, &bytes)
                                .and_then(|p| {
                                    crate::platform::shell::open_target(&p.to_string_lossy())
                                }) {
                                Ok(()) => log::info!("[plugin-open] id={} kind=image", job.id),
                                Err(e) => {
                                    log::warn!("[plugin-open] id={} kind=image err={e}", job.id);
                                    ctx.toast_err(format!("打开图片失败：{e}"));
                                }
                            }
                        }
                    }
                    Ok(crate::extensions::run::JobOutput::Settings) => {
                        // 设置窗开与关都在屏幕上，回传的设置已由编排层合并落盘。
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
                        let applied =
                            v.borrow_mut()
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
                            ctx.toast_ok(format!(
                                "已应用「{}」的编辑（原图未覆盖；插件菜单可另存/还原）",
                                job.plugin
                            ));
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
            // 启动置顶 / 全屏：一次只发一个，发完出队（见 `StartOp` 的说明）。
            // `startup_busy` 为真时本帧不再下发别的窗口操作，否则会把刚排到的
            // 这个顶掉；不透明度那类"改了才下发"的下一帧自然补上。
            let op = {
                let mut q = startup_ops.borrow_mut();
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
            // 极简模式即时生效：`sync` 刚把勾选写进配置，这一帧就推给当前窗口。
            // 只在**配置值**变过时推（见 `applied_minimal` 的说明）。
            let want_minimal = config.borrow().minimal_mode;
            if apply_minimal_mode(&chrome, want_minimal, &applied_minimal) {
                ctx.mark_dirty();
            }
            // 窗口置顶 / 窗口大小 / 固定尺寸即时生效（m54）：三项原来只影响下次
            // 开窗，设置页贴着「下次启动生效」。置顶只在**配置值**变过时动手，
            // 且按窗口现态决定翻不翻——标题栏 📌 是会话态，配置没变就不碰它。
            let ws = ctx.window_state();
            let mode = config.borrow().init_window;
            if needs_restored_size(was_maximized.get(), ws.maximized, ws.fullscreen, mode) {
                restored_size_pending.set(true);
            }
            if mode != InitWindow::Fixed {
                restored_size_pending.set(false);
            }
            was_maximized.set(ws.maximized || ws.fullscreen);
            let want_stay = config.borrow().stay_on_top;
            let stay_due = !window_op_sent && applied_stay.get() != want_stay;
            if stay_due
                && (ws.topmost == want_stay
                    || submit_window_op(ctx, &mut window_op_sent, |ctx| ctx.toggle_topmost()))
            {
                applied_stay.set(want_stay);
            }
            // 尺寸档：下拉换档立即下发；文本框改尺寸等稳定期，避免中途值把窗口
            // 改成一条（`sync` 已经按「解析成功才写配置」过滤掉不完整输入）。
            {
                let (mode, w, h) = {
                    let c = config.borrow();
                    (c.init_window, c.window_w, c.window_h)
                };
                let last = applied_window.get();
                let cur = (mode, w, h);
                if cur != last {
                    // 换档立即下发；固定尺寸文本框要等同一个值稳定 40 帧。
                    if mode != last.0 {
                        // 用户已经改走别的档：启动期那枚"按首图开窗"的待办必须作废，
                        // 否则首图解码落地时它会抢走唯一的窗口操作位，把用户刚选的尺寸顶掉。
                        if mode != InitWindow::Natural {
                            natural_pending.set(false);
                        }
                        size_seen.set(cur);
                        window_pending.set(Some(cur));
                        size_settle.set(0);
                    } else if cur != size_seen.get() {
                        window_pending.set(None);
                        size_seen.set(cur);
                        size_settle.set(0);
                    } else if size_settle.get() >= 40 {
                        window_pending.set(Some(cur));
                    } else {
                        size_settle.set(size_settle.get() + 1);
                    }
                } else {
                    window_pending.set(None);
                    size_seen.set(cur);
                    size_settle.set(0);
                }
            }
            let dims = v.borrow().info_dims();
            if apply_pending_window(
                ctx,
                &mut window_op_sent,
                &window_pending,
                &applied_window,
                dims,
            ) {
                natural_pending.set(false);
                restored_size_pending.set(false);
            }
            // 三个会话态（m53-L）：所有切换入口都当场写配置并推设置页，所以配置
            // 是唯一真值——这里每帧对齐一次，设置页勾「棋盘格」也就即时上屏。
            if apply_session_flags(&chrome, &config.borrow()) {
                ctx.mark_dirty();
            }
            // 配置保存防抖（M1）：UI 侧只置脏，这里到期统一落盘。
            crate::core::config::poll_save(&config.borrow());
            // 「关闭到托盘」即时生效：托盘图标跟着开关显隐（上面 sync 刚把新值写进配置，
            // 所以拨复选框的这一帧图标就变了）。**只在翻转时下发**——`set_visible` 会
            // 踢一帧，每帧都发等于把"静止不重绘"的省电设计废掉。
            let want_tray = config.borrow().close_to_tray;
            if tray_shown.get() != want_tray {
                tray_shown.set(want_tray);
                tray_handle.set_visible(want_tray);
            }
            // 不透明度：改动才下发（改样式/重算整帧都有成本，每帧重设没必要）。
            //
            // 菜单**不再**需要"展开期间强制 100%"的特判：呈现已改走逐像素
            // alpha（UpdateLayeredWindow），菜单面板由框架的 `opaque_overlays`
            // 整块豁免透明度——窗口保持半透明，只有菜单不透明。
            let want = config.borrow().opacity;
            if want != applied_opacity.get()
                && submit_window_op(ctx, &mut window_op_sent, |ctx| ctx.set_window_opacity(want))
            {
                applied_opacity.set(want);
            }
            let mut arrived = false;
            // 目录监控是否触发了清单变化（需在借出 `v` 的块外决定要不要 mark_dirty）。
            let mut dir_changed = false;
            {
                let mut v = v.borrow_mut();
                arrived |= drain_decode(&mut v, &done_rx.borrow(), &meta_rx.borrow());
                // 目录监控重臂：看图目录变了就换 watcher（新开图 / 单实例传参 /
                // 空格预览开图后，当前目录都变了）。比对放在收变化信号之前，保证
                // 新 watcher 装上后下一波变化能被收到。
                // ★ 借出来比（m106）：这一格每 16 ms 走一次，而目录几乎从不真变——
                //   旧写法每拍先抄一份 `PathBuf` 再比，那一份是白抄的。只有**判出来变了**
                //   那一拍才落一份（要交给 `DirWatcher`，它要 owned 路径）。
                let cur_dir = v.gallery_dir();
                let changed = {
                    let watched = watched_dir.borrow();
                    cur_dir != watched.as_deref()
                };
                if changed && Instant::now() >= watch_retry_at.get() {
                    let owned = cur_dir.map(|p| p.to_path_buf());
                    match owned.as_deref() {
                        Some(d) => {
                            *watcher.borrow_mut() = DirWatcher::new(d, dir_tx.clone()).ok();
                            if watcher.borrow().is_none() {
                                log::warn!("目录监控装配失败：{}", d.display());
                            }
                        }
                        None => *watcher.borrow_mut() = None,
                    }
                    // 装配失败时不要把新目录记成"已监控"：留着旧值，下一拍
                    // `cur_dir != watched` 仍成立，就能重试；否则一次瞬时失败
                    // （句柄被占、线程耗尽）会让该目录永久失去目录监控。
                    if owned.is_none() || watcher.borrow().is_some() {
                        *watched_dir.borrow_mut() = owned;
                        watch_retry_at.set(Instant::now());
                    } else {
                        watch_retry_at.set(Instant::now() + std::time::Duration::from_secs(5));
                    }
                }
                // 收目录变化信号：50ms 节流，一次只重扫一次——把资源管理器里一整波
                // 拷贝合并成一次 `read_dir`。在节流窗口内**不 drain**，让事件留在通道
                // 里，保证最后一波变化一定被扫到（不会因"恰好在窗口里被消费掉"而漏扫）。
                let now = Instant::now();
                if now.duration_since(last_dir_rescan.get()).as_millis() >= 50 {
                    let mut dirty = false;
                    while dir_rx.try_recv().is_ok() {
                        dirty = true;
                    }
                    if dirty {
                        last_dir_rescan.set(now);
                        if v.rescan_dir() {
                            dir_changed = true;
                        }
                    }
                }
                // ★ 收进托盘期间停止解码调度：看不见的画面不值得烧 CPU——
                //   尤其动画 GIF 会一直拉下一帧、缩放落定后的重解码也在此列。
                //   结果包照收（不积压队列），只是不再发起新的解码请求；
                //   窗口恢复后 tick 自然把欠下的那份重解发了。
                //   判据用框架的 `window_state().visible` 而不是自建标志：隐藏也可能
                //   由框架内部发生（ESC 关窗、hide_on_close），自建标志迟早失准。
                arrived |= advance(&mut v, ctx.window_state().visible);
                // 看门狗巡检（P1.1 C1）：解码线程卡死时这里不会收到回包，
                // 所以巡检放在**兜底轮询**里（而不是回包泵里——那正是不会响的地方）。
                // 到点只记一条 warn，不弹提示：阈值没有现场数据校准过，误报比不报更坏
                // （用户会学会忽略它）。见 core::decode_watchdog 模块头。
                crate::core::decode_watchdog::watchdog_check();
            }
            // 「永远居中」：侦测「最大化/全屏 → 常规」这一次跳变，把窗口复位成
            // 固定尺寸档的配置值并由平台层按工作区居中。`set_window_size` 在
            // 最大化态是空操作，所以必须等还原之后再发；跳变检测保证它只在
            // 真的还原那一帧触发一次。
            let ws = ctx.window_state();
            // 有待办的窗口动作时让路：同一帧只有一个窗口操作位，先发这个会把
            // 队列里那枚（全屏/图片尺寸）顶掉，而且 `window_pending` 已被清空、不会重试。
            if restored_size_pending.get()
                && !ws.maximized
                && !ws.fullscreen
                && window_pending.get().is_none()
            {
                // 尺寸**当帧从配置读**。过去这里用装配期捕获的那一份 `win_w/win_h`，
                // 于是「改了固定尺寸 → 最大化 → 还原」回到的是**旧**尺寸，新值非得重启
                // 才生效——配置不生效又没有一句提示（审计 O3）。
                let (fw, fh) = crate::platform::window_frame::frameless_frame();
                let (cw, ch) = {
                    let c = config.borrow();
                    (c.window_w as i32 - fw, c.window_h as i32 - fh)
                };
                if submit_window_op(ctx, &mut window_op_sent, |ctx| ctx.set_window_size(cw, ch)) {
                    restored_size_pending.set(false);
                }
            }
            // 图片尺寸档（init_window=3）：启动首图解码完成后按图片原始像素开窗
            // （windui 自带夹工作区+居中；最大化/全屏态它自己会跳过，等还原后
            // 下一轮再补）。尺寸先夹进 [WINDOW_MIN, WINDOW_MAX]（下限 100×100），
            // 平台再把窗口夹到显示器工作区——实际就是「图片多大就开多大，但最小
            // 100×100、最大不超过当前显示器可用面积」。置旗防重复——翻页不跟随。
            //
            // 判据改成**状态**（`info_dims` 有值）而不是"本帧有没有收到结果包"：
            // 本帧可能因为启动动作占着唯一的窗口操作位而发不出去（见 `StartOp`），
            // 按帧事件判据那一刀就永久丢了，按状态判据下一帧自然补上。
            if natural_pending.get() && !window_op_sent && !ws.maximized && !ws.fullscreen {
                let dims = v.borrow().info_dims();
                if let Some((w, h)) = dims {
                    if w > 0 && h > 0 {
                        let (cw, ch) = clamp_window_size(w, h);
                        if submit_window_op(ctx, &mut window_op_sent, |ctx| {
                            ctx.set_window_size(cw as i32, ch as i32)
                        }) {
                            natural_pending.set(false);
                        }
                    }
                }
            }
            // 空格预览：开关差分（同值零动作）。接管判定/解码/开窗全在哨兵线程，
            // 这里只做一件事——配置翻转时通知它装/摘钩子（判定矩阵见
            // platform::spacehook 模块头）。
            crate::platform::spacehook::set_enabled(config.borrow().space_preview);
            // 悬浮层自动隐藏（静止 2s 且指针不在贴边区）。真实光标位置作为第二判据
            // 传进去：`last_pos` 可能已是过期记录（leave 通知被拖动/缩放吞掉），
            // 对不上锚点就直接收，不再等 2 秒（m55 / 待办 §10 ①）。
            let view_rect = v.borrow().view_rect();
            // ★ 光标只在**极简模式**才要（m107）：`tick_hide` 第一行就在传统模式直接 return，
            //   而默认档恰是传统模式 ⇒ 每拍一次 `GetCursorPos`（K80 实测 779–807 ns/次）纯白花。
            //   省下来那一格交 `None`——"传统模式对光标完全无感"有用例钉着
            //   （`classic_mode_never_needs_a_cursor_reading`），没有那道判据这个省略就是猜。
            let cursor = if chrome.minimal_mode.get() {
                crate::platform::cursor::screen_pos()
            } else {
                None
            };
            if chrome.tick_hide(view_rect, cursor) {
                ctx.mark_dirty();
            }
            // 平移限速的补帧（M8）：落在间隔里被挂起的那一帧到点就画出去。手停下之后
            // 不会再有"下一条 move"来补，只有这条轮询能兜住它——不兜就是画面停在松手
            // 前一步的位置，那是看得见的差一点。
            if chrome.tick_pan_pace() {
                ctx.mark_dirty();
            }
            // 滚轮翻页的节奏闸兜底（A3 / m64）：压在 `WHEEL_PACE` 里的那一下，手停下之后就
            // 不会再有下一条滚轮消息来补——不兜就是"滚两格只翻一张"，那是看得见的丢步。
            if let Some(dir) = chrome.tick_wheel_pace() {
                v.borrow_mut().step(dir);
                ctx.mark_dirty();
            }
            // 键盘翻页的节奏闸兜底（m81）：压在 `KEY_PACE` 里的那一下，松手之后就
            // 不会再有下一条按键消息来补——不兜就是"按住最后那一步白按"，与滚轮同一条界线。
            if let Some(dir) = chrome.tick_key_pace() {
                v.borrow_mut().step(dir);
                ctx.mark_dirty();
            }
            // 幻灯片放映（m167）：到点翻一张。**没有定时器线程**——这同一条 16 ms 轮询
            // 已经在跑，翻页走的也是键盘/滚轮那同一个 `Viewer::step`（末尾天然回绕）。
            // 间隔每次现读配置：放映中途在设置页改档，下一张就按新档停。
            let pace = std::time::Duration::from_secs(config.borrow().slide_interval as u64);
            if chrome.tick_slideshow(pace) {
                v.borrow_mut().step(1);
                ctx.mark_dirty();
            }
            // 进度横条**只在换了一格的那一帧**重绘（默认档 = 每间隔 8 次）。整扇窗口是逐像素
            // 分层的，每帧去刷一条 3 px 的横条等于每秒六十次重合成全窗——那是反着"低占用"的。
            if chrome.slide_step_tick() {
                ctx.mark_dirty();
            }
            // 后台复制的回执（m74）：那一枚 PNG 编码跑在一次性线程上，落地时屏幕上
            // 一点变化都没有——复制是无痕副作用，成与败都必须由这一帧说出去，
            // 否则用户手里到底有没有图，只有程序自己知道。
            match chrome.tick_copy() {
                CopyWait::Copied => ctx.toast_ok("整张画面已复制（PNG）"),
                CopyWait::Failed(e) => ctx.toast_err(e),
                CopyWait::Waiting | CopyWait::Idle => {}
            }
            if let Some(out) = chrome.tick_save() {
                match out {
                    Ok(()) => ctx.toast_ok("涂鸦已另存（PNG）"),
                    Err(e) => ctx.toast_err(e),
                }
            }
            // 属性窗跟随当前图：图片切换 / 解码完成时重算并刷新那扇窗（无属性窗时无副作用）。
            crate::ui::settings::refresh_open_metadata(&v);
            if arrived || dir_changed {
                ctx.mark_dirty();
            }
        }
    })
    .on_shortcut({
        let v = viewer.clone();
        let chrome = chrome.clone();
        let config = config.clone();
        let hub = hub.clone();
        move |_ctx, ev| {
            if !ev.pressed {
                return false;
            }
            // 注：`ShortcutCtx` 只有"交还焦点/请求关闭"两个动作，开不了文件对话框，
            // 故 Ctrl+O 那类要弹窗的入口放在画布右键菜单里。
            // 这里拿不到 `EventCtx`（只有 `ShortcutCtx`），故要改窗口态 / 开抽屉的动作在
            // 「焦点不在画布」的兜底情形下静默放行；焦点在画布时由 `on_event` 正常处理。
            handle_key(None, &v, &chrome, &config, &hub, &ev)
        }
    })
    .content(root)
    // 退出权威保存（M1）：防抖没等到的改动在这里兜底落盘。
    .on_close_request({
        let config = config.clone();
        let chrome = chrome.clone();
        move |ctx| {
            crate::core::config::flush_save(&config.borrow());
            // 三层优先级与理由见 [`close_action`]。**「关闭到托盘」当场看配置**，
            // 所以设置页那个复选框立即生效（不必重启）。
            let act = close_action(chrome.locked.get(), config.borrow().close_to_tray);
            // 这条判据留一行 **info**：今天三次"窗口不见了但进程还在"的排查都卡在
            // "到底是哪一支拦下了关窗"，而它只看两个布尔量。放在 debug 等于每次都要
            // 为了这一句重跑一遍（m59 把它提到默认档）。
            log::info!(
                "[close] locked={} close_to_tray={} action={:?}",
                chrome.locked.get(),
                config.borrow().close_to_tray,
                act
            );
            match act {
                // 锁定（🔒）吞掉用户侧关窗（Alt+F4 / 系统 ×／自绘 ×，语义：拦截器返回
                // false = 取消，见 windui event.rs）；应用主动关（ctx.request_close）
                // 不走本决策链，不受影响。
                CloseAction::Cancel => false,
                // 隐藏走 `ctx.hide_window()`——与框架 `hide_on_close` 内部留下的
                // `WindowOp::Hide` 是同一条管道，只是判据从"装配期布尔"换成了"此刻的配置"。
                // 回来的路是托盘图标：它由上面的轮询与该开关同步显隐，勾上即出现。
                CloseAction::Hide => {
                    ctx.hide_window();
                    false
                }
                // 未开托盘：放行 = 真退出（`on_close_request` 返回 true）。
                CloseAction::Exit => true,
            }
        }
    })
    .run();

    // 退出兜底（K44）：权威落盘原来**只**挂在 `on_close_request` 上，而托盘「退出」走的是
    // 框架 `quit_app() → DestroyWindow`——`DestroyWindow` 派发的是 `WM_DESTROY`，**不经过
    // `WM_CLOSE`**，那条回调压根不跑。于是从托盘退出时，防抖窗口（5s）里没来得及落盘的
    // 偏好改动静默丢失。放在 `run()` 返回之后：无论从哪条路退出，落盘必然执行且只此一处。
    crate::core::config::flush_save(&config.borrow());
    // 空格预览的低级钩子挂在系统钩子链上。显式停用把收权从「赌进程消亡时系统替我摘」
    // 变成一句明写的动作；投递走控制线程（摘钩必须在装配它的那条线程上，见
    // `spacehook::post_enable`），控制线程已停时投递失败无害——不退化于现状。
    crate::platform::spacehook::set_enabled(false);
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 关窗三条去路与优先级：锁定 > 托盘 > 退出。
    ///
    /// 第二条断言是这条测试存在的理由：锁定态若被判成 `Hide`，被锁的窗口会在用户点 ×
    /// 时从任务栏消失（本该是"点了没反应"），而那时用户已经找不到它了。
    #[test]
    fn close_decision_prioritises_lock_over_tray() {
        assert_eq!(
            close_action(true, true),
            CloseAction::Cancel,
            "锁定态只取消，不得顺手收进托盘"
        );
        assert_eq!(close_action(true, false), CloseAction::Cancel);
        assert_eq!(close_action(false, true), CloseAction::Hide);
        assert_eq!(close_action(false, false), CloseAction::Exit);
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
}
