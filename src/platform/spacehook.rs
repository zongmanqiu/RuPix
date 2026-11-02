//! 空格预览（QuickLook 式）的常驻哨兵：**低级键盘钩子 + 独立消息线程 + 原生分层预览窗**。
//!
//! 2026-10-17 二轮重做的两条病根（一轮方案真机实爆；判定链与 QuickLook 对齐的细节
//! 见 本仓历史记录 的二轮记录）：
//!
//! 1. **`RegisterHotKey(VK_SPACE)` 在系统层吞键，回注救不回来。** 一轮用注册裸空格 +
//!    `SendInput` 回注做打字放行——但回注事件的 vkCode 仍是空格，会**再次**命中这条
//!    热键：键在系统层就被拿走、只给我们发一条 WM_HOTKEY，想让收到它的应用从头到尾
//!    都收不到。防环时间戳只挡得住旗标自触发，救不回键本身。后果：开关开着，全系统
//!    的空格一律失灵（拼音选词、打字补空格全部打不出去）。
//! 2. **消费链挂在主窗的帧上，收进托盘即停摆。** WM_HOTKEY 旗标由 `App::on_interval`
//!    轮询消费、解码成品由 `App::channel` 投递——通道回调只在窗口的**帧**里排空，而
//!    隐藏/最小化的窗口不产生 WM_PAINT（windui 框架源码亲注的实测："TICK 照跑而
//!    CHANNEL 8 秒一条不来"）。主窗一收进托盘（正是空格预览九成五的实用场景），
//!    旗标没人取、成品没人收：预览永远开不出来，空格却照吞不误。
//!
//! QuickLook 的原始做法（参考项目）两条全绕开：`WH_KEYBOARD_LL`
//! 低级钩子里只做微秒级同步甄别（前台类名白名单 + 组合窗/插入符 + 修饰键 + 注入标记），
//! **从不吞键、从不回注**；触发动作投递给常驻消息线程干重活（外壳 COM 取选中项、解码、
//! 开窗）。本模块逐点对齐，另有一处更动：预览窗改用**本线程原生**的分层弹窗
//! （`UpdateLayeredWindow` 逐像素合成、透明 PNG 真透出桌面），不再经 UI 框架窗口系统
//! ——单例键/owned 随主窗显隐/通道帧泵这些牵连整个消失；Esc 在窗体键盘焦点内处理，
//! 一轮"预览期临时注册全局 Esc"（会把全系统 Esc 吞掉）不再需要。
//!
//! # 判定链（每颗裸空格 DOWN，顺序即语义；钩子内全部是本进程可见的 user32 调用）
//! 1. 注入键 → 放过（QuickLook 对 SendInput 回放的豁免同款）；
//! 2. 长按自重复 → 放过（QuickLook `_spaceIsDown`；连发不会反复开关预览）；
//! 3. 任一修饰键按下 → 放过；
//! 4. 前台 = 自家预览窗 → 投递"收起"；前台 = 自家其它窗 / 打字上下文 / 非外壳窗
//!    → 放过；外壳白名单且预览未开 → 投递"开"；开着的预览在前台之外按空格 → "收起"；
//! 5. 控制线程收到消息后才碰跨进程 COM（选中项、后缀核验、解码派发）——钩子回调
//!    始终微秒级返回，不会撞 `LowLevelHooksTimeout`（超时会被系统**静默摘钩**）。
//!
//! 分层：platform 禁 windui ✓（本文件零 UI 框架依赖）；对 core 只向下取格式清单与
//! 解码（core < platform 方向合法）。选中探测与显示器工作区复用
//! [`crate::platform::shell_selection`] 的经典 Shell COM 三跳（语言无关、不读本地化文本）。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBRUSH, HDC,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, FindWindowExW, GetClassNameW, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, GetWindowThreadProcessId, LoadCursorW, PostMessageW, RegisterClassW,
    SetCursor, SetForegroundWindow, SetWindowLongPtrW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, UpdateLayeredWindow, GWLP_USERDATA, HC_ACTION, HHOOK, IDC_ARROW,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_UP, MSG, ULW_ALPHA, WH_KEYBOARD_LL, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_SETCURSOR, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

/// 自定义消息（`WM_APP` 起是应用私有段，与框架自身的 +1/+2 分配距离足够）。
const MSG_ENABLE: u32 = WM_APP + 20;
const MSG_SPACE: u32 = WM_APP + 21;
const MSG_READY: u32 = WM_APP + 22;
/// `MSG_ENABLE` 的 wParam 编码。
const ON: usize = 1;
const OFF: usize = 0;

/// 预览图占前台显示器工作区的比例上限（与一轮口径一致，四周留一拍呼吸感）。
const WORK_AREA_RATIO: f32 = 0.85;

/// 用户开关（跨线程读写：UI 线程 `set_enabled` 写，钩子回调与本线程读）。
static ENABLED: AtomicBool = AtomicBool::new(false);
/// 控制窗句柄（仅本线程创建；数值发布后别的线程拿它投递）。
static CTRL_HWND: AtomicUsize = AtomicUsize::new(0);
/// 预览窗句柄（仅本线程创建/销毁；钩子回调用数值与前台句柄比对，O(1)）。
static PREVIEW_HWND: AtomicUsize = AtomicUsize::new(0);
/// 空格按住标志（长按自重复抑制——QuickLook `_spaceIsDown` 同款：DOWN 接管后置旗，
/// 连发直到 UP 一律放过，预览不会随按住被反复开关）。
static SPACE_DOWN: AtomicBool = AtomicBool::new(false);
/// 解码世代：连按空格时旧线程的成品作废（一轮就有的设计，原样保留）。
static GEN: AtomicU64 = AtomicU64::new(0);
// 本线程持有的钩子句柄（`MSG_ENABLE` 差分会装上/摘掉；仅本线程读写）。
thread_local! {
    static HOOK: std::cell::Cell<Option<HHOOK>> = const { std::cell::Cell::new(None) };
}
/// 钩子实际挂上了吗（`HOOKED` 是 `HOOK` TLS 的跨线程镜像：apply_enable 装/摘成功时
/// 写，`status()` 由别的线程读——TLS 本身只能同线程取，观测必须落原子）。
static HOOKED: AtomicBool = AtomicBool::new(false);
/// 上一次摘钩失败、句柄可能已经失效：下次开启时不要把它当成"还挂着"而跳过重装。
static HOOK_STALE: AtomicBool = AtomicBool::new(false);
/// 观测计数：钩子回调收到过多少次空格键事件（**含注入键**——`examples/space_probe`
/// 用注入一颗空格来证明"钩子链活着、事件到得了回调"；真实判定仍走注入豁免）。
/// 每事件一次 relaxed 自增，成本一个内存写，与总账（零 CPU 常驻）相称。
static SPACE_SEEN: AtomicU64 = AtomicU64::new(0);
// 两个窗口类在消息泵启动前一次性注册；进程级幂等（同名类重复注册系统报错但无害，
// 显式挡一层省得读代码的人担心"第二次注册会不会换掉过程地址"）。
static CLASSES_DONE: AtomicBool = AtomicBool::new(false);

/// 预览窗 `GWLP_USERDATA` 的随窗资源：ULW 用过的位图与兼容 DC。
/// `WM_CREATE` 从 `CREATESTRUCTW.lpCreateParams` 领取堆块的所有权，`WM_DESTROY` 归还。
/// DIB 位图内存本体归 DIB section 管（`DeleteObject` 时随之释放），这里的指针只做锚。
struct PreviewPayload {
    dc: HDC,
    bmp: *mut core::ffi::c_void,
}

/// 解码成品（工作线程 → 本线程，经 `MSG_READY` 的 lParam 传堆块所有权）。
struct Ready {
    /// 预乘 BGRA（`UpdateLayeredWindow` + `AC_SRC_ALPHA` 要求的合成格式）。
    premul: Vec<u8>,
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    gen: u64,
}

// ───────────────────────── 对外 API ─────────────────────────

/// 起哨兵线程（`runtime::run()` 装配期调用一次；失败只 Err 不 panic：调用方 warn 后
/// 功能静默不可用，其余一切照常）。线程阻塞在 `GetMessageW`，空闲零 CPU——与全项目
/// "常驻后台不烧电"的纪律一致。
pub fn start() -> Result<(), String> {
    std::thread::Builder::new()
        .name("rupix-spacehook".to_string())
        .spawn(thread_main)
        .map(|_join| ())
        .map_err(|e| format!("空格预览线程启动失败: {e}"))
}

/// 开关差分（UI 线程 16ms 轮询调用；同值零动作）。
///
/// 开 → 本线程装钩；关 → 摘钩 + 清按住态 + 收起预览。注册失败由本线程 warn 一次、
/// 不重试刷屏（裸空格场景的常驻钩子对手是 QuickLook 等程序，占用即常态；恢复点是
/// 用户下次翻转开关——一轮语义平替）。`ENABLED` 先落账再投递：线程尚未建好窗时
/// 投递失败也无关大碍——建好窗后直接按 `ENABLED` 现值自装配（补偿点在 `thread_main`）。
pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::SeqCst) == on {
        return;
    }
    post_enable(on);
}

/// 投递开关消息；false 表示控制窗还没建好（启动竞争窗口）。调用方无需补救——
/// 线程装配时以 `ENABLED` 现值为准，两边最终一致。
fn post_enable(on: bool) -> bool {
    let raw = CTRL_HWND.load(Ordering::SeqCst);
    if raw == 0 {
        return false;
    }
    unsafe {
        PostMessageW(
            Some(HWND(raw as *mut core::ffi::c_void)),
            MSG_ENABLE,
            WPARAM(if on { ON } else { OFF }),
            LPARAM(0),
        )
        .is_ok()
    }
}

/// 运行状态快照（诊断面：`examples/space_probe` 逐环验证"钩子链是否活着"）。
/// 生产代码不读它——它只是把内部原子状态换个体面出口，成本约等于读五个原子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceStatus {
    /// 用户开关（config.space_preview 的镜像）。
    pub enabled: bool,
    /// 控制窗已建成（线程泵就绪）。
    pub ctrl_ready: bool,
    /// 键盘钩子实际在位（挂载成功且未摘）。
    pub hooked: bool,
    /// 回调累计收到的空格事件数（含注入——"发一颗键看计数涨不涨"即可证明钩子链）。
    pub space_seen: u64,
    /// 预览窗当前开着。
    pub preview_open: bool,
}

/// 取状态快照（任意线程可调）。
pub fn status() -> SpaceStatus {
    SpaceStatus {
        enabled: ENABLED.load(Ordering::Relaxed),
        ctrl_ready: CTRL_HWND.load(Ordering::Relaxed) != 0,
        hooked: HOOKED.load(Ordering::Relaxed),
        space_seen: SPACE_SEEN.load(Ordering::Relaxed),
        preview_open: PREVIEW_HWND.load(Ordering::Relaxed) != 0,
    }
}

// ───────────────────────── 纯逻辑判定（无头可测） ─────────────────────────

/// 一颗裸空格 DOWN 的介入意图。钩子据此决定是否投递；本枚举**不存在"吞键"档**——
/// 接管也只是"额外做一件事"，键仍原样发给前台窗口。QuickLook 同款：它的钩子返回
/// 恒为 CallNextHookEx。"永不吞键"从此是结构性质，不是要靠回注修补的行为承诺。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// 完全不介入。
    Pass,
    /// 收起已开的预览。
    Close,
    /// 尝试开预览（选中项/后缀核验在控制线程异步做，这里只代表"值得试"）。
    Open,
}

/// 纯函数判定核。参数与判序对齐 QuickLook：自家预览窗最优先（toggle 收）→ 自家
/// 其它窗永不 → 打字上下文永不（QuickLook `IsCursorActivated` 是它
/// `GetFocusedWindowType` 的第一道闸——资源管理器改名/搜索框里的空格永远归编辑框）
/// → 非外壳窗永不 → 预览已开按空格一律收 → 否则开。
pub fn engage(
    fg_is_preview: bool,
    fg_is_our_other_window: bool,
    typing: bool,
    fg_is_shell: bool,
    preview_open: bool,
) -> Intent {
    if fg_is_preview {
        return Intent::Close;
    }
    if fg_is_our_other_window || typing || !fg_is_shell {
        return Intent::Pass;
    }
    if preview_open {
        Intent::Close
    } else {
        Intent::Open
    }
}

// ───────────────────────── 钩子（微秒级，快进快出） ─────────────────────────

/// 低级键盘钩子回调。运行在本模块线程的输入上下文里；超过 `LowLevelHooksTimeout`
/// 不返回会被系统**静默摘钩**——所以这里只做本地 user32 判定 + PostMessage，
/// 跨进程重活全部留在控制线程。本钩子从不返回"消费"：键永远继续传递。
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 || lparam.0 == 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    if ENABLED.load(Ordering::Relaxed) && kb.vkCode == VK_SPACE.0 as u32 {
        // 观测点放在注入豁免**之前**：探针要回答的是"键盘事件能不能到这个回调"。
        SPACE_SEEN.fetch_add(1, Ordering::Relaxed);
        if kb.flags.contains(LLKHF_INJECTED) {
            // 合成键不参与（QuickLook 对 SendInput 回放的豁免同款）：别的程序注入的
            // 空格不该替用户开预览，也彻底免除一切"回注→再命中"的循环。
            return CallNextHookEx(None, code, wparam, lparam);
        }
        if kb.flags.contains(LLKHF_UP) {
            SPACE_DOWN.store(false, Ordering::Relaxed);
        } else {
            if SPACE_DOWN.load(Ordering::Relaxed) {
                // 长按自重复：这串 DOWN 的第一颗已处理过，连发一律放过。
                // ★ 但 UP 可能在安全桌面 / 另一条钩子链 / 会话切换里丢掉；旗一旦
                // 卡死，本会话所有空格都不再响应。自重复发生时物理键必然还按着，
                // 所以这里用同步键态复核一次：已松开就自愈，继续按正常 DOWN 处理。
                if unsafe { GetAsyncKeyState(VK_SPACE.0 as i32) } & (-32768) != 0 {
                    return CallNextHookEx(None, code, wparam, lparam);
                }
                SPACE_DOWN.store(false, Ordering::Relaxed);
            }
            if unsafe { modifiers_held() } {
                // 带修饰的空格不归我们（QuickLook：`Modifiers != None` 跳过；Win 键
                // 压着连裸空格也不碰——开始菜单搜索框正在吃这颗键）。
                return CallNextHookEx(None, code, wparam, lparam);
            }
            match hook_engage() {
                Intent::Pass => {}
                Intent::Close | Intent::Open => {
                    SPACE_DOWN.store(true, Ordering::Relaxed);
                    // 意图（收/开）不在钩子里定夺：控制线程按当下的 `PREVIEW_HWND`
                    // 重新裁决即可——两颗消息之间用户什么都做得出，现场状态才算数。
                    notify_space();
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// 四个修饰键任一按下（同步查按键态；全部内存级调用，无跨进程）。
unsafe fn modifiers_held() -> bool {
    let held = |vk: i32| unsafe { GetAsyncKeyState(vk) & (-32768) != 0 };
    // -32768 == 0x8000 的有符号形式——GetAsyncKeyState 返回 i16，高位为"当前按下"。
    held(VK_CONTROL.0 as i32)
        || held(VK_MENU.0 as i32)
        || held(VK_SHIFT.0 as i32)
        || held(VK_LWIN.0 as i32)
        || held(VK_RWIN.0 as i32)
}

/// 实时接管判定：前台句柄/类名 + 本线程可见的两个原子读 + 打字判据。
/// 无 COM、无跨进程查询（QuickLook 在钩子里跑的也是这档廉价检查）。
fn hook_engage() -> Intent {
    let preview_raw = PREVIEW_HWND.load(Ordering::Acquire);
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return Intent::Pass;
    }
    let fg_is_preview = preview_raw != 0 && fg.0 as usize == preview_raw;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    let fg_is_our_other_window = !fg_is_preview && pid == std::process::id();
    let fg_is_shell = unsafe { is_shell_foreground(fg) };
    engage(
        fg_is_preview,
        fg_is_our_other_window,
        crate::platform::shell_selection::is_typing_context(),
        fg_is_shell,
        preview_raw != 0,
    )
}

/// 前台是否资源管理器/桌面。类名口径取自 QuickLook `Shell32::GetFocusedWindowType`，
/// 只留我们支持的 EXPLORER/DESKTOP 两档；Progman/WorkerW 必须内含 `SHELLDLL_DefView`
/// 才算真桌面——Win10/11 贴壁纸的那张 WorkerW 类名相同但没有它，不守卫会把"壁纸"
/// 认成桌面（选中项探测随之拿到无意义结果）。
unsafe fn is_shell_foreground(fg: HWND) -> bool {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(fg, &mut buf);
    if n <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buf[..n as usize]);
    match class.as_str() {
        "CabinetWClass" | "ExploreWClass" => true,
        "Progman" | "WorkerW" => FindWindowExW(Some(fg), None, w!("SHELLDLL_DefView"), None)
            .map(|h| !h.is_invalid())
            .unwrap_or(false),
        _ => false,
    }
}

/// 把接管投递给控制线程（钩子回调与它的泵同线程；消息本身就是跨队列的安全交接）。
fn notify_space() {
    let raw = CTRL_HWND.load(Ordering::Acquire);
    if raw == 0 {
        return;
    }
    unsafe {
        let _ = PostMessageW(
            Some(HWND(raw as *mut core::ffi::c_void)),
            MSG_SPACE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

// ───────────────────────── 控制线程：泵 + 重活 ─────────────────────────

/// 线程主循环：注册类 → 建 message-only 控制窗 → 按开关现状装配钩子 → 阻塞消息泵。
/// `GetMessageW` 返回 0/-1（WM_QUIT / 错误）即自然收尾——线程随进程消亡，钩子回收。
fn thread_main() {
    unsafe {
        let hmod = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("spacehook：模块句柄获取失败，空格预览不可用: {e}");
                return;
            }
        };
        if !CLASSES_DONE.swap(true, Ordering::SeqCst) {
            register_class(hmod, w!("RuPixSpacePreviewCtl"), Some(ctrl_wnd_proc), None);
            // 预览窗必须显式箭头光标：类光标留 NULL 时，DefWindowProc 应答
            // WM_SETCURSOR 走 SetCursor(NULL)，Windows 11 下悬停呈现忙光标
            //（旋转圆圈，K40）。
            let arrow = LoadCursorW(None, IDC_ARROW);
            register_class(
                hmod,
                w!("RuPixSpacePreview"),
                Some(preview_wnd_proc),
                arrow.ok(),
            );
        }
        let ctrl = match CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("RuPixSpacePreviewCtl"),
            w!("RuPixSpacePreviewCtl"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hmod.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("spacehook：控制窗创建失败，空格预览不可用: {e}");
                return;
            }
        };
        CTRL_HWND.store(ctrl.0 as usize, Ordering::SeqCst);
        // 补偿点：`set_enabled` 在建窗前的动作可能没投递到消息队列——按现值直接装配。
        apply_enable(ENABLED.load(Ordering::SeqCst));
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 注册窗口类。无背景刷：控制窗不可见，预览窗内容整个经 ULW 提交、从不走 GDI 绘制。
/// `cursor` 为 None 时类光标留空（控制窗不可见无所谓）；预览窗传显式箭头。
/// `proc` 用 windows 的 WNDPROC（= `Option<unsafe extern "system" fn(..)>`）原形传，
/// 调用点以 `Some(具体过程)` 显式过桥。
unsafe fn register_class(
    hmod: impl Into<windows::Win32::Foundation::HINSTANCE>,
    name: windows::core::PCWSTR,
    proc: Option<
        unsafe extern "system" fn(
            HWND,
            u32,
            windows::Win32::Foundation::WPARAM,
            windows::Win32::Foundation::LPARAM,
        ) -> LRESULT,
    >,
    cursor: Option<windows::Win32::UI::WindowsAndMessaging::HCURSOR>,
) {
    let wc = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: hmod.into(),
        lpszClassName: name,
        hbrBackground: HBRUSH::default(),
        hCursor: cursor.unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassW(&wc);
}

/// 控制窗过程。三类自定义消息全在这条线程串行处理——钩子回调刚返回、对输入预算的
/// 占用已交还，此处才碰跨进程 COM 与开窗。一轮的判定/投递挂在主窗帧上（收进托盘
/// 即停摆），这条链与主窗显隐从此再无关系。
unsafe extern "system" fn ctrl_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        MSG_ENABLE => apply_enable(wparam.0 == ON),
        MSG_SPACE => on_space(),
        MSG_READY => on_ready_raw(lparam),
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

/// 开 → 装钩（幂等：已装不重复）；关 → 摘钩 + 清按住态 + 收预览。
/// 只在钩子线程调用（句柄与线程共生，别处碰是未定义）。
unsafe fn apply_enable(on: bool) {
    if on {
        // 重新开钩时先清按住态：关掉期间可能漏掉 UP，不能带着旧旗进来。
        SPACE_DOWN.store(false, Ordering::Relaxed);
        // 摘钩失败留下的句柄可能已经失效；丢弃它重装，别让 HOOKED 永远粘在 true。
        let stale = HOOK_STALE.swap(false, Ordering::SeqCst);
        let already = HOOK.with(|c| {
            if stale {
                if let Some(old) = c.take() {
                    // 句柄可能已死；活着就顺手摘掉，避免两条钩子同时收键。
                    let _ = unsafe { UnhookWindowsHookEx(old) };
                }
            }
            c.get().is_some()
        });
        if !already {
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
                Ok(h) => {
                    HOOK.with(|c| c.set(Some(h)));
                    HOOKED.store(true, Ordering::SeqCst);
                }
                Err(e) => {
                    log::warn!("spacehook：键盘钩子挂载失败（可能有其它常驻钩子程序在场）: {e}")
                }
            }
        }
    } else {
        if let Some(h) = HOOK.with(|c| c.take()) {
            match UnhookWindowsHookEx(h) {
                Ok(()) => {
                    HOOKED.store(false, Ordering::SeqCst);
                    HOOK_STALE.store(false, Ordering::SeqCst);
                }
                // 摘钩失败 = 钩子**还在**，Space 仍然被我们吃着。以前这里无条件写
                // `HOOKED=false`，于是 `status()` 对外说"已关闭"而输入还在被拦（审计
                // 第三批）：谎报比失败更难查。把句柄放回去，保持"仍在钩"的真实状态，
                // 让下一次关闭或退出时重试。
                Err(e) => {
                    log::warn!("spacehook：摘钩失败，空格接管仍在生效，状态如实报为开启：{e}");
                    HOOK.with(|c| c.set(Some(h)));
                    // 失败也可能是句柄早已被系统摘掉；下次开启时强制重装一次。
                    HOOK_STALE.store(true, Ordering::SeqCst);
                }
            }
        }
        SPACE_DOWN.store(false, Ordering::Relaxed);
        // 作废所有在途解码：否则关掉后重新打开时，旧一代的成品会通过 `on_ready_raw`
        // 的 ENABLED/GEN 双闸，把关闭前选中的图弹出来并抢前台。
        GEN.fetch_add(1, Ordering::SeqCst);
        close_preview();
    }
}

/// 空格接管执行：有预览收起；无预览做真取件——跨进程 COM 查询发生在这里，
/// 不在钩子里（QuickLook 的/native 与托管进程同理分工：重活儿异步）。
/// 取不到合格图片 → 静默返回：键已原样放行，用户无感（QuickLook 同款选择——
/// 在资源管理器按空格没选中图本来就没有任何事发生）。
unsafe fn on_space() {
    // 消息可能排在上一条 OFF 之后才被泵到：开关已经关了就不再取件。
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    if PREVIEW_HWND.load(Ordering::Acquire) != 0 {
        close_preview();
        return;
    }
    // 跨进程取件（IShellWindows COM）不能在这个线程做：它同时是低级键盘钩子的
    // 宿主线程，钩子回调要等这条线程泵消息。COM 在 Explorer 新窗口上可能停几秒，
    // 足以让 Windows 按 `LowLevelHooksTimeout` 静默摘钩。取件与解码一起交给一次性
    // worker；控制线程只做 `PREVIEW_HWND` 判定和投递。
    dispatch_pick_and_decode();
}

/// 取件 + 解码派发：整段跨进程 COM 与解码都在一次性 worker 里（不占控制线程，
/// 更不占 UI 线程），成品带世代回投。控制线程只保留窗口状态判定。
///
/// 已知小缺口（记录在案，与一轮同款）：预览窗出现前连按第二颗空格，第一份成品
/// 会被世代令牌照常作废、按第二颗的选中项再试一次——用户视角是"按太慢，只开一次"，
/// 可接受。（磁盘解码缓存撤掉后，"第二次开同一张会秒出"这句不再成立——两次都是真解码。）
fn dispatch_pick_and_decode() {
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let ctrl = CTRL_HWND.load(Ordering::SeqCst);
    let spawned = std::thread::spawn(move || {
        // 清单口径与主窗一致：内置（SVG/SVGZ）∪ 解码器自述扩展名的并集，且排除要弹
        // 参数窗的 raw 类（这类文件走空格预览没有意义，参数窗才解得开）。
        let allowed = |ext: &str| crate::core::formats::is_previewable(ext);
        let Some(path) = crate::platform::shell_selection::foreground_selected_file(&allowed)
        else {
            return;
        };
        let (mx, my, mw, mh) = crate::platform::shell_selection::monitor_work_area_of_foreground();
        let box_w = ((mw as f32) * WORK_AREA_RATIO).max(1.0) as u32;
        let box_h = ((mh as f32) * WORK_AREA_RATIO).max(1.0) as u32;
        let Ok(decoded) = crate::core::decode::probe_decode(&path, box_w, box_h) else {
            // 解码失败静默：清单认领了却解不出属解码器层错误路径（主窗打开同样会
            // 报错），预览不出窗即可——一轮口径原样保留。
            return;
        };
        if GEN.load(Ordering::SeqCst) != gen {
            return; // 连按空格：这份已过期。
        }
        let bmp = decoded.bitmap;
        let (w, h) = (bmp.width.max(1) as i32, bmp.height.max(1) as i32);
        let premul = rgba_to_premul_bgra(&bmp.rgba);
        let x = mx + (mw - w) / 2;
        let y = my + (mh - h) / 2;
        let ready = Box::new(Ready {
            premul,
            w,
            h,
            x,
            y,
            gen,
        });
        // 成品交给控制线程上屏；`Box::into_raw` 的所有权**要么**转移给消息处理方，
        // **要么**在投递失败时由我们收回。以前写的是 `let _ = PostMessageW(..)`：
        // 控制线程已退出 / 队列满时投递失败，这张整图的堆块就没人负责（审计第三批）——
        // 预览一次几 MB 到几十 MB，正是要省内存的那条路径。
        let raw = Box::into_raw(ready);
        let posted = unsafe {
            PostMessageW(
                Some(HWND(ctrl as *mut core::ffi::c_void)),
                MSG_READY,
                WPARAM(0),
                LPARAM(raw as isize),
            )
        };
        if posted.is_err() {
            drop(unsafe { Box::from_raw(raw) });
            log::debug!("spacehook：成品回投失败（控制线程已没？），位图就地释放");
        }
    });
    // 一次性解码线程：成品回投后即弃。`std::thread::spawn` 无失败返回（OS 线程
    // 资源耗尽是进程级末日，与一轮 preview.rs 裸 spawn 同口径，不为其加防御）。
    let _handle = spawned;
}

/// RGBA（直通）→ **预乘 BGRA**（ULW_ALPHA 要求的合成格式）。每像素整数运算；
/// alpha=0 的颜色像素全清零（透出的就是真桌面，不做黑底近似）。
fn rgba_to_premul_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        if a == 0 {
            out.extend_from_slice(&[0u8; 4]);
        } else {
            let p = |c: u8| ((c as u32 * a as u32) / 255) as u8;
            out.push(p(b));
            out.push(p(g));
            out.push(p(r));
            out.push(a);
        }
    }
    out
}

/// `MSG_READY`：lParam 是工作线程传来的堆块指针。先无条件收权（`Box::from_raw`），
/// 再判有效性——过期件也要把内存还掉，不许泄漏。
unsafe fn on_ready_raw(lparam: LPARAM) {
    let ptr = lparam.0 as *mut Ready;
    if ptr.is_null() {
        return;
    }
    let ready = unsafe { *Box::from_raw(ptr) };
    if !ENABLED.load(Ordering::SeqCst) || ready.gen != GEN.load(Ordering::SeqCst) {
        return; // 开关已关 / 已有更新按键在途：作废。
    }
    if PREVIEW_HWND.load(Ordering::Acquire) != 0 {
        return; // 已有窗口（收窗与成品赛跑的极端次序）：不叠加第二扇。
    }
    open_preview(ready);
}

// ───────────────────────── 预览窗（本线程原生分层弹窗） ─────────────────────────

/// 开预览：先建分层 DIB 并填充（失败免建窗，少一次抖动），再开窗、ULW 提交、尝试
/// 送进前台。任何一步失败静默放弃（下颗按键自愈），**不弹任何错误**。
unsafe fn open_preview(ready: Ready) {
    let hmod = match GetModuleHandleW(None) {
        Ok(h) => h,
        Err(_) => return,
    };
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ready.w,
            biHeight: -ready.h, // top-down，与成品行序一致
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bmp = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("spacehook：预览位图创建失败: {e}");
            return;
        }
    };
    if bits.is_null() {
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        return;
    }
    // DIB 内存不保证零初始化（windui 同款实测定论）：先全清再拷贝——解码尾数
    // 万一不满整窗时，留白呈全透明而不是杂色。
    let bytes = (ready.w as usize)
        .saturating_mul(ready.h as usize)
        .saturating_mul(4);
    std::ptr::write_bytes(bits.cast::<u8>(), 0, bytes);
    let copy = ready.premul.len().min(bytes);
    std::ptr::copy_nonoverlapping(ready.premul.as_ptr(), bits.cast::<u8>(), copy);
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        return;
    }
    let _ = SelectObject(dc, HGDIOBJ(bmp.0));

    // 随窗资源堆块的所有权：lpCreateParams → WM_CREATE → GWLP_USERDATA →
    // WM_DESTROY 一条线交还；建窗失败就地回收。
    let payload = Box::into_raw(Box::new(PreviewPayload { dc, bmp: bmp.0 }));
    let hwnd = match CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TOPMOST,
        w!("RuPixSpacePreview"),
        w!("RuPix 预览"),
        WS_POPUP | WS_VISIBLE,
        ready.x,
        ready.y,
        ready.w,
        ready.h,
        None,
        None,
        Some(hmod.into()),
        Some(payload as *const core::ffi::c_void),
    ) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("spacehook：预览窗创建失败: {e}");
            // 资源还没进 WM_CREATE 线：手动逐样归还，堆块也不例外。
            let _ = DeleteDC(dc);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            drop(unsafe { Box::from_raw(payload) });
            return;
        }
    };

    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255, // 逐像素 alpha 已在 DIB 里
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let dst = POINT {
        x: ready.x,
        y: ready.y,
    };
    let src = POINT { x: 0, y: 0 };
    let size = SIZE {
        cx: ready.w,
        cy: ready.h,
    };
    // pptDst/psize 必须显式给（一轮自框架层抄来的实测经验：给 NULL 会
    // "返回成功却不上屏"，首帧定格成最迷惑的死法）。
    if UpdateLayeredWindow(
        hwnd,
        None,
        Some(&dst),
        Some(&size),
        Some(dc),
        Some(&src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    )
    .is_err()
    {
        // 样式带 LAYERED 却提交失败 → 会留下一张透明废窗，直接销毁；下颗空格自愈。
        let _ = DestroyWindow(hwnd);
        return;
    }
    PREVIEW_HWND.store(hwnd.0 as usize, Ordering::Release);
    activate_soft(hwnd);
}

/// 尽力把预览窗送成前台键盘焦点（Esc 收窗依赖这一步；送不进去也无碍——收窗还有
/// 空格 toggle 一条路，钩子不看焦点，只看前台句柄）。经典借道：AttachThreadInput
/// 挂到前台线程的输入队列 → BringWindowToTop + SetForegroundWindow → 解挂。
/// Win11 对抢前台的限制可能整条落空，落空也只是"窗可见但无焦点"（TOPMOST 保证可见）。
unsafe fn activate_soft(hwnd: HWND) {
    let fg = GetForegroundWindow();
    let ft = GetWindowThreadProcessId(fg, None);
    let cur = GetCurrentThreadId();
    let attached = ft != 0 && ft != cur && AttachThreadInput(ft, cur, true).as_bool();
    let _ = BringWindowToTop(hwnd);
    let _ = SetForegroundWindow(hwnd);
    if attached {
        let _ = AttachThreadInput(ft, cur, false);
    }
}

/// 收起预览（幂等）。Alt+F4 那条路不走这里、走窗体自己的销毁链，句柄统一在
/// `WM_DESTROY` 里复位（见 `preview_wnd_proc`，不会给全局留下悬柄）。
unsafe fn close_preview() {
    let raw = PREVIEW_HWND.swap(0, Ordering::AcqRel);
    if raw != 0 {
        let _ = DestroyWindow(HWND(raw as *mut core::ffi::c_void));
    }
}

/// 预览窗过程。无子控件、无输入框，其余消息一律默认处理。分层窗不经 WM_PAINT
/// （内容由 ULW 提交）。空格不在这里处理——收窗走钩子：键原样路过钩子、就算
/// 落进本窗也只是被 DefWindowProc 丢弃；让语义唯一走钩子那条路。
unsafe extern "system" fn preview_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW;
            if !cs.is_null() {
                let p = unsafe { (*cs).lpCreateParams };
                if !p.is_null() {
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, p as isize) };
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // Alt+F4 或其它 WM_CLOSE 源：走与收起同一条销毁路径。
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if raw != 0 {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let payload = Box::from_raw(raw as *mut PreviewPayload);
                    let _ = DeleteDC(payload.dc);
                    let _ = DeleteObject(HGDIOBJ(payload.bmp));
                }
            }
            // 全局句柄只在本窗就是"当前预览"时清：迟到的旧窗销毁不许抹掉
            // 新开预览的句柄（compare-exchange 恰好区分这两种世界）。
            let _ = PREVIEW_HWND.compare_exchange(
                hwnd.0 as usize,
                0,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 as u32 == VK_ESCAPE.0 as u32 {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // 钉死箭头：类光标已是 IDC_ARROW，这里再显式应答一次——DefWindowProc 的
            // 默认链经 SetCursor(NULL)，Windows 11 下悬停会呈现忙光标（旋转圆圈，
            // K40）。应答任何 `fSet` 都重新设一次，把忙态清掉。
            unsafe {
                if let Ok(h) = LoadCursorW(None, IDC_ARROW) {
                    let _ = SetCursor(Some(h));
                }
            }
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判定矩阵（QuickLook 行为的可测化）：自家预览最优先收起；自家其它窗、打字中、
    /// 非外壳窗一律放行；合法外壳场景随预览状态切换开/收。
    #[test]
    fn engage_matrix_matches_quicklook() {
        assert_eq!(
            engage(true, false, false, false, true),
            Intent::Close,
            "预览在前台按空格必须收起"
        );
        // fg_is_preview 优先于"其他窗永不"：预览窗属于本进程，但它不是"其它窗"。
        assert_eq!(engage(true, true, true, false, true), Intent::Close);
        // 自家其它窗（主窗/设置窗/详情窗）即便外壳+无打字也不接管——一轮"自家放行"臂。
        assert_eq!(engage(false, true, false, true, false), Intent::Pass);
        assert_eq!(engage(false, true, false, true, true), Intent::Pass);
        // 打字上下文（可见插入符，或 IME 组合窗 GTF 置位）永不接管：空格原样归编辑框。
        assert_eq!(engage(false, false, true, true, false), Intent::Pass);
        assert_eq!(engage(false, false, true, true, true), Intent::Pass);
        // 非外壳窗（Word/浏览器/游戏……）永不接管——"永不吞键"的结构性保证。
        assert_eq!(engage(false, false, false, false, false), Intent::Pass);
        // 外壳 + 预览未开 → Open（选中项与后缀核验在异步侧）；
        assert_eq!(engage(false, false, false, true, false), Intent::Open);
        // 外壳 + 预览已开 → Close（QuickLook 的 Toggle）。
        assert_eq!(engage(false, false, false, true, true), Intent::Close);
    }

    /// 长按旗语义：置位/清零与钩子内的短路条件一致（不执行 FFI，只测旗的可用性）。
    #[test]
    fn space_down_latch_roundtrips() {
        SPACE_DOWN.store(false, Ordering::SeqCst);
        assert!(!SPACE_DOWN.load(Ordering::SeqCst));
        SPACE_DOWN.store(true, Ordering::SeqCst);
        assert!(
            SPACE_DOWN.load(Ordering::SeqCst),
            "DOWN 接管后必须压制自重复"
        );
        SPACE_DOWN.store(false, Ordering::SeqCst);
        assert!(!SPACE_DOWN.load(Ordering::SeqCst), "UP 必须清旗");
    }

    /// 预乘转换：不透明仅换通道顺序；alpha=0 全清；半透明整数预乘；尾数不 panic。
    #[test]
    fn rgba_to_premul_converts_correctly() {
        let rgba = vec![
            255u8, 0, 0, 255, // 不透明红 → [0,0,255,255]
            0, 255, 0, 0, // 全透明 → 全零
            200, 100, 50, 128, // 半透明 → b·128/255, g·128/255, r·128/255, 128
        ];
        let out = rgba_to_premul_bgra(&rgba);
        assert_eq!(out.len(), 12);
        assert_eq!(&out[0..4], &[0, 0, 255, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
        let p = |c: u32| ((c * 128) / 255) as u8;
        assert_eq!(&out[8..12], &[p(50), p(100), p(200), 128]);
        assert!(rgba_to_premul_bgra(&[1, 2, 3]).is_empty());
        assert!(rgba_to_premul_bgra(&[]).is_empty());
    }

    /// 外壳类名表（QuickLook 口径的静态锚）：四个类名与 defView 守卫不能从源码里漂走。
    #[test]
    fn shell_class_table_names_survive_refactor() {
        let src = include_str!("spacehook.rs");
        for cls in ["CabinetWClass", "ExploreWClass", "Progman", "WorkerW"] {
            assert!(src.contains(cls), "外壳类名表缺少 {cls}");
        }
        assert!(src.contains("SHELLDLL_DefView"), "桌面 defView 守卫不能丢");
    }
}
