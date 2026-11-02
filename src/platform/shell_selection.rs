//! 资源管理器/桌面「当前选中的文件」探测 + 前台窗口上下文判定（空格预览取件环节）。
//!
//! platform 层：纯 Win32/Shell COM，禁 windui。所有探测失败一律返回 `None`/`false`，
//! 调用方（`platform::spacehook`）以「这颗空格不归我们」收场——探测只增益、绝不吞键。
//! COM 线程模型：哨兵线程与 UI 线程各自 `ComGuard` 自理（钩子线程按需 STA；UI 线程
//! 早被 windui 以 STA 起过，拖放要用 OLE），`ComGuard` 对 S_FALSE 不收不碰。
//!
//! 取件链路（对齐 QuickLook `Shell32::GetCurrentSelection` 的双路分派）：
//! **资源管理器窗**走 `IShellWindows` 枚举（frame 与 `ShellTabWindowClass` 双匹配）→
//! `IServiceProvider::QueryService`（SID_STopLevelBrowser）→ `IShellBrowser::
//! QueryActiveShellView` → **`IShellView::GetItemObject(SVGIO_SELECTION)` 取
//! IDataObject、按 `CF_HDROP` 取首项**（QuickLook `HelperMethods::GetSelectedInternal`
//! 同款；`IFolderView::Items` 在本机 Win11 的 Explorer 视图实测抛 0x80070490，仅作
//! 桌面兜底，见 K38）；**桌面不在窗口列表里**，改走 `IShellWindows::FindWindowSW(SWC_DESKTOP)`
//! 单独一跳，前置守卫是前台类名 Progman/WorkerW 且内含 `SHELLDLL_DefView`（一轮注释宣称的
//! "桌面也是一枚外壳窗口、同一条链天然覆盖"经真机证伪——枚举里没有桌面，见 K38）。
//! zip 内等虚拟项转不出文件系统路径——按"没选中"放行。
//!
//! windows 0.62 没导出 `CLSID_ShellWindows`，GUID 手写（shlguid.h 著名值）。

use std::path::PathBuf;

use windows::core::{w, Interface, GUID};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{CoCreateInstance, IServiceProvider, CLSCTX_LOCAL_SERVER};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Shell::{
    IEnumIDList, IFolderView, IShellBrowser, IShellWindows, IWebBrowser2, SHGetPathFromIDListW,
    SID_STopLevelBrowser, SVGIO_SELECTION, SWC_DESKTOP, SWFO_NEEDDISPATCH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics,
    GetWindowThreadProcessId, GUITHREADINFO, SM_CXSCREEN, SM_CYSCREEN,
};

use crate::platform::recycle::ComGuard;

/// `CLSID_ShellWindows`（{9BA05972-F6A8-11CF-A442-00A0C90A8F39}）。
const CLSID_SHELL_WINDOWS: GUID = GUID::from_u128(0x9BA05972_F6A8_11CF_A442_00A0C90A8F39);

/// 前台窗口的键盘焦点是否处于「可输入上下文」。
///
/// 双判据（对齐 QuickLook `HelperMethods::IsCursorActivated`）：
/// - `hwndCaret` 非空：记事本/浏览器地址栏等传统输入框的可见插入符；
/// - `flags ≠ 0`（GTF_* 位：组合窗、候选窗、菜单展开等任一为真）：**IME 合成中的
///   权威信号**——微软拼音组词期应用的标准插入符会被 IME 自绘光标顶替
///   （`hwndCaret` 此时常为 null），只盯 caret 会把「正在打字」误判成放行对象。
///
/// 个别无 caret 也不开组合窗的富文本编辑器漏判属已知长尾（设置页总开关即总退路，
/// 见方案文档 §六长尾清单）。
pub fn is_typing_context() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let tid = GetWindowThreadProcessId(fg, None);
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if !GetGUIThreadInfo(tid, &mut info).is_ok() {
            return false;
        }
        info.flags.0 != 0 || !info.hwndCaret.is_invalid()
    }
}

/// 前台资源管理器（或桌面）当前选中的第一个文件系统路径。
///
/// `allowed` 收到**小写、无点**扩展名（口径与 `core::formats::declared_exts` 一致），
/// 由调用方决定哪些后缀算数——本模块不 import core 的清单，保持薄封装。
/// 多选只看第一项；第一项不是放行后缀就返回 `None`（不翻找其余选中项）。
///
/// 两条取链（对齐 QuickLook `Shell32::GetCurrentSelection` 的分派）：
/// 1. **资源管理器窗**：`IShellWindows.Item` 枚举，按前台 HWND 匹配；Win11 标签式
///    窗口另加 `ShellTabWindowClass` 子窗匹配（IShellWindows 条目可能登记的是
///    标签子窗而非最外层 frame，只比 frame 会全数漏配——真机实测坐实）。
/// 2. **桌面**：`IShellWindows.Item` 枚举**不含桌面**（桌面不在窗口列表里，
///    一轮模块头"Progman 同一条链天然覆盖"的断言是错的——真机实测坐实），必须走
///    `IShellWindows::FindWindowSW(SWC_DESKTOP)` 单独一跳。前置：前台类名是
///    Progman/WorkerW 且内含 `SHELLDLL_DefView`（壁纸层的 WorkerW 没有图标视图，
///    不加这层守卫会拿错选择集）。
pub fn foreground_selected_file(allowed: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    let _com = ComGuard::new();
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return None;
        }
        let shell: IShellWindows =
            CoCreateInstance(&CLSID_SHELL_WINDOWS, None, CLSCTX_LOCAL_SERVER).ok()?;

        // ── 链 1：资源管理器窗（含标签子窗双配）──
        let fg_raw = fg.0 as isize;
        // Win11 标签宿主窗：前台 frame 之下的 ShellTabWindowClass 子窗。IShellWindows
        // 条目 HWND 与之相等也算命中（QuickLook Shell32.cpp:143 同款 fallback）。
        let fg_tab = FindWindowExW(Some(fg), None, w!("ShellTabWindowClass"), None)
            .map(|h| h.0 as isize)
            .unwrap_or(0);
        let count = shell.Count().ok()?;
        for i in 0..count {
            let mut index = VARIANT::default();
            // VARIANT_0 是联合体且字段被 ManuallyDrop 包裹：整块装填（逐字段穿过
            // ManuallyDrop 会被编译器拒收）。VT_I4 + lVal = "按下标枚举"。
            index.Anonymous = windows::Win32::System::Variant::VARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(
                    windows::Win32::System::Variant::VARIANT_0_0 {
                        vt: VT_I4,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: windows::Win32::System::Variant::VARIANT_0_0_0 { lVal: i },
                    },
                ),
            };
            let Ok(dispatch) = shell.Item(&index) else {
                log::debug!("space-take: item {i} Item() 失败，跳过");
                continue;
            };
            // 匹配用 IWebBrowser2.HWND()——但 **cast 失败绝不能挡三跳正路**：
            // Win11 23H2+ 把顶层浏览窗的 IDispatch 换成了 IEffectiveBrowse 家族
            // （不再暴露 IWebBrowser2，真机日志实锤），窗口本身仍是正经 top-level
            // browser，IServiceProvider 三跳照常可达。h 留 0 表示"拿不到窗口句柄的
            // 新家族条目"，服务链里再用 IShellBrowser::GetWindow 补判（同源
            // QuickLook 的 `psb->GetWindow`，Shell32.cpp:168）。
            let mut h = 0isize;
            if let Ok(browser) = dispatch.cast::<IWebBrowser2>() {
                if let Ok(handle) = browser.HWND() {
                    // HWND 的 `.0` 本就是 isize（windows 0.62），再 as 一层是冗余转换。
                    h = handle.0;
                }
            }
            let matched = h == fg_raw || (fg_tab != 0 && h == fg_tab);
            if !matched && h != 0 {
                continue;
            }
            let Ok(sp) = dispatch.cast::<IServiceProvider>() else {
                log::debug!("space-take: item {i} 无 IServiceProvider，跳过");
                continue;
            };
            let Ok(browser_svc) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else {
                if h == 0 {
                    // 无 IWebBrowser2 的条目：IServiceProvider 也不是 browser——
                    // 桌面窗等在此被自然滤掉（桌面有专属跳转，见链2）。
                    continue;
                }
                log::debug!("space-take: item {i} QueryService(top-level browser) 失败");
                continue;
            };
            // 拿不到 h 的条目（新接口家族）在此用 IShellBrowser 自己的窗口做终判，
            // 等价 QuickLook 的 `psb->GetWindow` 匹配。
            if h == 0 {
                let bh = match browser_svc.GetWindow() {
                    Ok(w) => w.0 as isize,
                    Err(_) => continue,
                };
                if bh != fg_raw && !(fg_tab != 0 && bh == fg_tab) {
                    continue;
                }
            }
            if let Some(p) = selected_via_browser(&browser_svc, allowed) {
                return Some(p);
            }
            // 走到这里 = 本条目就是前台窗（h 匹配过，或 GetWindow 匹配过）却取不到
            // 选择集（地址栏编辑中等）：按「没选中」放行，不再翻找其余条目——
            // QuickLook 匹配到窗口后同样单点返回。
            return None;
        }

        // ── 链 2：桌面（仅当前台确实是图标层时）──
        if !foreground_is_desktop(fg) {
            log::debug!("space-take: 枚举未命中且前台非桌面图标层，放行");
            return None;
        }
        // QuickLook `getSelectedFromDesktop`：FindWindowSW(SWC_DESKTOP,
        // SWFO_NEEDDISPATCH) 直接拿桌面浏览器的 IDispatch。两个 loc 参数传空 VARIANT。
        let empty = VARIANT::default();
        let mut phwnd = 0i32;
        let Ok(desktop) =
            shell.FindWindowSW(&empty, &empty, SWC_DESKTOP, &mut phwnd, SWFO_NEEDDISPATCH)
        else {
            return None;
        };
        let Ok(sp) = desktop.cast::<IServiceProvider>() else {
            return None;
        };
        let Ok(browser_svc) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else {
            return None;
        };
        selected_via_browser(&browser_svc, allowed)
    }
}

/// 已从外壳窗口取到 `IShellBrowser` 之后的公共取件段（QuickLook
/// `HelperMethods::GetSelectedInternal` 同款双路）：
/// ActiveShellView → **主路 `GetItemObject(SVGIO_SELECTION)` 取 IDataObject、走
/// CF_HDROP 取首项** → 非放行后缀/虚拟项/空选择一律 `None`。
///
/// 为什么主路不是 `IFolderView::Items`：本机 Win11 真机实测，Explorer 文件夹视图的
/// defview 对 `Items(SVGIO_SELECTION)` 抛 `0x80070490`（ERROR_NOT_FOUND），而
/// `GetItemObject` + `CF_HDROP` 一直可用——QuickLook 全程只走后者绝非偶然（K38）。
/// 兜底保留 Items 一条：桌面 defview 对它有响应，主路万一在某代外壳上缺席时不断链。
unsafe fn selected_via_browser(
    browser_svc: &IShellBrowser,
    allowed: &dyn Fn(&str) -> bool,
) -> Option<PathBuf> {
    let view = browser_svc.QueryActiveShellView().ok()?;
    if let Some(p) = selection_via_dataobject(&view, allowed) {
        return Some(p);
    }
    // 兜底：IFolderView::Items（桌面 defview 实测有响应；Explorer 视图会抛错，静默放行）。
    let Ok(view) = view.cast::<IFolderView>() else {
        return None;
    };
    let Ok(items) = view.Items::<IEnumIDList>(SVGIO_SELECTION) else {
        return None;
    };
    let mut slot: [*mut windows::Win32::UI::Shell::Common::ITEMIDLIST; 1] = [std::ptr::null_mut()];
    let mut fetched = 0u32;
    if items.Next(&mut slot, Some(&mut fetched)).is_err() || fetched != 1 {
        return None;
    }
    let pidl = slot[0];
    if pidl.is_null() {
        return None;
    }
    // 虚拟项（zip 内、此电脑等）转不出文件系统路径：按「没选中」放行。
    // 缓冲 260 = MAX_PATH，与 `SHGetPathFromIDListW` 自己的契约一致：它**不会**写越这条
    // 界，路径过长是返回 FALSE（不是截断），于是这里当「没选中」放行——超长路径的图要用
    // 主窗打开，空格预览不是给那种路径准备的。
    let mut out = None;
    let mut buf = [0u16; 260];
    if SHGetPathFromIDListW(pidl, &mut buf).as_bool() {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        out = take_if_allowed(String::from_utf16_lossy(&buf[..end]), allowed);
    }
    // PIDL 是 `IEnumIDList::Next` 按 CoTaskMemAlloc 给我们分配的，**用完必须归还**。
    // 以前两条 return 都把它漏了（审计第三批）：每条只有几十字节，但这条路径的用法就是
    // "用户在资源管理器里想按多少次空格就按多少次"——漏得小不等于不漏。
    windows::Win32::System::Com::CoTaskMemFree(Some(pidl.cast::<core::ffi::c_void>()));
    out
}

/// 主路：`GetItemObject(SVGIO_SELECTION)` → IDataObject → `CF_HDROP` 首项。
/// HDROP 拿不到（虚拟项/空选择）返回 `None`——与「没选中」同义放行。
/// CFSTR_SHELLIDLIST 那条备路不移植：它服务的是回收站等虚拟项的解析名，而本模块
/// 只认文件系统路径，转不出一样是 `None`，多一路白多一层。
unsafe fn selection_via_dataobject(
    view: &windows::Win32::UI::Shell::IShellView,
    allowed: &dyn Fn(&str) -> bool,
) -> Option<PathBuf> {
    use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::Ole::{ReleaseStgMedium, CF_HDROP};
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    let dao: IDataObject = view.GetItemObject(SVGIO_SELECTION).ok()?;
    let fe = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        // TYMED 的 `.0` 是 i32、FORMATETC.tymed 是 u32——常态字段宽不对齐，显式收口。
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(med) = dao.GetData(&fe) else {
        return None;
    };
    // GetData 成功即接管 medium：之后所有分支都要 ReleaseStgMedium
    // （TYMED_HGLOBAL 由它内部 GlobalFree，手动收尾必漏）。
    let mut med = med;
    //★ 只认 TYMED_HGLOBAL，并且句柄非空：外部进程给的 `IDataObject` 可以**合法地**返回
    //  别的 TYMED（第三方命名空间 / OneDrive / 回收站视图都这么干），直接把
    //  `u.hGlobal` 当 HDROP 用就是句柄类型混淆——访问违规落在 `extern "system"` 回调里
    //  = 整个进程无声没掉（K42）。medium 已被接管，任何提前返回都要先 Release。
    // 字段宽与 273 行同源：`STGMEDIUM.tymed` 是 u32、`TYMED_HGLOBAL.0` 是 i32，显式收口。
    let want = TYMED_HGLOBAL.0 as u32;
    if med.tymed != want || unsafe { med.u.hGlobal }.0.is_null() {
        // 先记下类型再归还：Release 之后字段内容不再可信。
        let got = med.tymed;
        unsafe { ReleaseStgMedium(&mut med) };
        log::debug!("spacehook：选择集的 STGMEDIUM 不是 HGLOBAL（tymed={got}），不接管");
        return None;
    }
    let hdrop = HDROP(unsafe { med.u.hGlobal }.0);
    // 条数查询：DragQueryFileW 的 ifile 传 0xFFFFFFFF（QuickLook 同款）。
    let count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
    let mut buf = [0u16; 260];
    let n = if count >= 1 {
        unsafe { DragQueryFileW(hdrop, 0, Some(&mut buf)) }
    } else {
        0
    };
    unsafe { ReleaseStgMedium(&mut med) };
    if n == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    take_if_allowed(String::from_utf16_lossy(&buf[..end]), allowed)
}

/// 后缀过滤的唯一出口：空串 / 无后缀 / 非放行后缀一律 `None`。
fn take_if_allowed(s: String, allowed: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    if s.is_empty() {
        return None;
    }
    let ext = std::path::Path::new(&s)
        .extension()?
        .to_string_lossy()
        .to_lowercase();
    if allowed(&ext) {
        Some(PathBuf::from(s))
    } else {
        None
    }
}

/// 前台窗口是否为「桌面图标层」：类名 Progman/WorkerW 且内含 `SHELLDLL_DefView`。
/// 壁纸层的 WorkerW 没有 defView，不放行——防 `SWC_DESKTOP` 兜底拿错选择集（QuickLook
/// `Shell32::GetFocusedWindowType` 的 DESKTOP 分支同款守卫）。
unsafe fn foreground_is_desktop(fg: HWND) -> bool {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(fg, &mut buf);
    if n <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    if class != "Progman" && class != "WorkerW" {
        return false;
    }
    FindWindowExW(Some(fg), None, w!("SHELLDLL_DefView"), None)
        .map(|h| !h.is_invalid())
        .unwrap_or(false)
}

/// 前台窗口所在显示器的工作区 `(x, y, w, h)`。取不到时退主屏全屏尺寸。
/// 预览窗的尺寸与居中都按这块区域算（UI 线程取好，工作线程只解码）。
pub fn monitor_work_area_of_foreground() -> (i32, i32, i32, i32) {
    unsafe {
        let fg = GetForegroundWindow();
        let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if monitor.is_invalid() || !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return (
                0,
                0,
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
            );
        }
        let r = info.rcWork;
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    }
}
