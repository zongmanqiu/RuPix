//! 真实光标的屏幕位置（物理像素）——给 UI 一个**不依赖消息流**的第三方读数。
//!
//! 为什么需要它：`WM_MOUSELEAVE` 的申请是一次性的，而那枚旗在框架侧只有收到 leave
//! 才复位。一次交互式拖动/缩放把 leave 吞掉之后，本会话再也等不到"指针离开了"的通知，
//! UI 里记着的最后一次 `Move` 位置就成了**过期记录**——浮条据此判定"指针还停在栏上"，
//! 永远收不走（`docs/待办.md` §10 那条残余症状）。
//!
//! 这里只提供读数，不提供判据：判据（漂移多少算过期）在 `ui::chrome`，因为它要跟着
//! 悬停几何走，也得能被单测注入。取**增量**而不是绝对坐标，所以本模块不需要窗口句柄、
//! 也不需要 DPI 换算。
//!
//! # 自绘抓手光标
//!
//! Windows 的系统光标库里**没有**"张开的手 / 握拳"（`IDC_HAND` 是食指指向的
//! "可点击"手），所以抓手必须自己造：SVG 由 `ui::hand_cursor`（那边能用 windui 的
//! resvg）光栅化成 RGBA，本模块负责把 RGBA 变成 `HCURSOR` 并切换。
//! 分层上这一步只能在 platform 做——`ui` 层禁引 `windows`。
//!
//! 为什么不用框架的 `Widget::cursor()` 通道：windui 的形状枚举里没有抓手；更要紧的是
//! 它在拖动时 `SetCapture`，而**捕获期间系统不再发 `WM_SETCURSOR`**（MSDN 明写），
//! 光靠那条通道拿不到"按住"这一刻。

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, MonitorFromPoint, ScreenToClient, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForWindow, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetCursorPos, GetSystemMetrics, LoadCursorW, SetCursor,
    HCURSOR, HICON, ICONINFO, IDC_ARROW, IDC_CROSS, SM_CXCURSOR,
};

/// 当前光标在**虚拟屏幕**上的物理坐标。失败返回 `None`（调用方按"没有读数"处理，
/// 不能拿 (0,0) 当真值——那是屏幕左上角，会误判成"光标飞走了"）。
pub fn screen_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // 唯一的 unsafe 点是 Win32 API 调用本身：`GetCursorPos` 只在成功时写 `p`。
    match unsafe { GetCursorPos(&mut p) } {
        Ok(()) => Some((p.x, p.y)),
        Err(_) => None,
    }
}

/// 当前光标在**指定窗口客户区**的逻辑坐标。
///
/// 文件拖放回调只给路径、不给落点；拖放结束那一刻系统光标就在落点上，所以这里
/// 从系统光标反算。窗口句柄由调用方给（`ui` 从 `windui::platform::native_window_handle`
/// 取），本层不依赖 UI 框架。拿不到句柄/坐标就返回 `None`，调用方回落到主图。
pub fn client_pos_logical(hwnd: isize) -> Option<(f32, f32)> {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let mut p = POINT::default();
    if unsafe { GetCursorPos(&mut p) }.is_err() {
        return None;
    }
    if !unsafe { ScreenToClient(hwnd, &mut p) }.as_bool() {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    let s = dpi / 96.0;
    Some((p.x as f32 / s, p.y as f32 / s))
}

/// 系统光标边长（物理像素）。win32 按这个尺寸挑/缩光标位图，据此光栅化才不糊。
///
/// ★ 按**光标所在那块屏**的 DPI 换算，不能只读 `SM_CXCURSOR`：后者给的是"系统 DPI"
/// 下的值，把窗口拖到另一块不同缩放的显示器后会一直返回旧尺寸，光标就保持旧块头。
pub fn system_size() -> u32 {
    let s = match cursor_monitor_dpi() {
        Some(dpi) => unsafe { GetSystemMetricsForDpi(SM_CXCURSOR, dpi) },
        None => unsafe { GetSystemMetrics(SM_CXCURSOR) },
    };
    if s <= 0 {
        32
    } else {
        s as u32
    }
}

/// 光标所在显示器的**有效 DPI**。取不到返回 `None`（调用方退回系统度量）。
fn cursor_monitor_dpi() -> Option<u32> {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    let mon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    if mon.is_invalid() {
        return None;
    }
    let (mut dx, mut dy) = (0u32, 0u32);
    unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.ok()?;
    (dx > 0).then_some(dx)
}

/// 已造好的手型光标。存 `isize` 不存 `HCURSOR`：句柄是裸指针，而静态量要求 `Sync`，
/// 指针本身不满足。
///
/// 用 `Mutex` 而不是 `OnceLock`：DPI 变化时要**丢掉重造**（[`drop_hands`]），
/// `OnceLock` 塞进去就再也换不掉了。
static GRAB_CURSOR: Mutex<Option<isize>> = Mutex::new(None);
static GRABBING_CURSOR: Mutex<Option<isize>> = Mutex::new(None);

fn slot(dragging: bool) -> &'static Mutex<Option<isize>> {
    if dragging {
        &GRABBING_CURSOR
    } else {
        &GRAB_CURSOR
    }
}

/// 造一次手型光标并缓存。`rgba` 必须是 `size×size` 的非预乘 RGBA8。
///
/// 返回 false 表示造不出来（调用方保持原样即可——光标是锦上添花，不该让主流程失败）。
pub fn install_hand(dragging: bool, rgba: &[u8], size: u32, hotspot: (i32, i32)) -> bool {
    // 中毒也要拿到值：这里不维护跨锁的不变量，panic 传染只会让光标永久失效。
    let mut cell = match slot(dragging).lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if cell.is_some() {
        return true;
    }
    let Some(cur) = make_cursor(rgba, size, hotspot) else {
        return false;
    };
    *cell = Some(cur.0 as isize);
    true
}

/// 切到已缓存的某只手。未安装时返回 false（调用方不强求）。
pub fn use_hand(dragging: bool) -> bool {
    match slot(dragging).lock() {
        Ok(cell) => match *cell {
            Some(h) => {
                // `SetCursor` 返回的是**旧**光标句柄（失败时为 null），不是 Result，
                // 而且没有"失败要处理"的分支——这里只关心"设过了"。
                unsafe {
                    let _ = SetCursor(Some(HCURSOR(h as *mut _)));
                }
                true
            }
            None => false,
        },
        Err(_) => false,
    }
}

/// 已装好的某只手的光标句柄。给上层拿去登记"自绘光标覆盖"用（platform 层不许引 windui，
/// 所以登记动作只能发生在 `ui::hand_cursor`）。
pub fn handle_of(dragging: bool) -> Option<isize> {
    match slot(dragging).lock() {
        Ok(cell) => *cell,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// 丢掉两枚手型光标句柄（DPI 变了，要按新尺寸重造）。
///
/// ★ 调用方必须**先把光标切走**（[`use_arrow`]）：`DestroyIcon` 一枚正被 `SetCursor`
/// 挂着的句柄，屏幕上那枚就成了野指针。
pub fn drop_hands() {
    for cell in [&GRAB_CURSOR, &GRABBING_CURSOR] {
        let mut guard = match cell.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(h) = guard.take() {
            unsafe {
                let _ = DestroyIcon(HICON(h as *mut _));
            }
        }
    }
}

/// 切回系统箭头（离开可拖区域 / 指针离窗）。
pub fn use_arrow() {
    unsafe {
        if let Ok(c) = LoadCursorW(None, IDC_ARROW) {
            let _ = SetCursor(Some(c));
        }
    }
}

/// 取色器模式的十字光标（m217）。与抓手互斥：取色时左键是「取样」不是「拖」，
/// 光标若还是松手/抓手，用户会按着拖图而不是点一下读数。
pub fn use_cross() {
    unsafe {
        if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
            let _ = SetCursor(Some(c));
        }
    }
}

/// RGBA8 → 32bpp DIB → `CreateIconIndirect`。
///
/// 掩码位图全 0：单色掩码里 0 表示"看彩色位图"，透明与否全交给 alpha 通道。
/// 两个位图只在造的过程中活着——`CreateIconIndirect` 会把像素拷进系统对象。
fn make_cursor(rgba: &[u8], size: u32, hot: (i32, i32)) -> Option<HCURSOR> {
    let s = size as i32;
    if s <= 0 || rgba.len() != (s as usize * s as usize * 4) {
        return None;
    }
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: s,
            // 负高度 = 顶行优先，与 RGBA 的行序一致（不用翻转）。
            biHeight: -s,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return None;
        };
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(color.0));
            return None;
        }
        let px = (s as usize) * (s as usize);
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, px * 4);
        for i in 0..px {
            let p = i * 4;
            dst[p] = rgba[p + 2];
            dst[p + 1] = rgba[p + 1];
            dst[p + 2] = rgba[p];
            dst[p + 3] = rgba[p + 3];
        }
        let mask = CreateBitmap(s, s, 1, 1, None);
        let info = ICONINFO {
            fIcon: false.into(),
            xHotspot: hot.0.clamp(0, s - 1) as u32,
            yHotspot: hot.1.clamp(0, s - 1) as u32,
            hbmMask: mask,
            hbmColor: color,
        };
        let created = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        created.map(|h| HCURSOR(h.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 读数在桌面会话里必然可用；更重要的是它**两次调用之间不会突然变成 None**，
    /// 否则判据会把"读不到"当成"光标飞走"而反复清悬停态。
    #[test]
    fn screen_pos_is_readable_and_stable() {
        let a = screen_pos();
        let b = screen_pos();
        assert!(a.is_some() && b.is_some(), "桌面会话里 GetCursorPos 应可用");
        // 允许真光标在两次读数之间被人挪动（共享桌面），只要求两边都有值。
        assert_eq!(a.is_some(), b.is_some());
    }

    /// 光标的制造路径要真的有产物：这一步失败的话抓手永远不出现，
    /// 而且失败是静默的（`install_hand` 只在内部返回 false）。
    #[test]
    fn make_cursor_accepts_rgba() {
        let rgba = vec![255u8; 32 * 32 * 4];
        let h = make_cursor(&rgba, 32, (16, 16)).expect("32×32 纯 RGBA 应能造出光标");
        assert!(!h.0.is_null(), "CreateIconIndirect 返回了空句柄");
        unsafe {
            let _ = DestroyIcon(h.into());
        }
    }

    /// 尺寸与字节数不匹配时必须拒收，而不是拿越界/错位的像素去造光标。
    #[test]
    fn make_cursor_rejects_wrong_size() {
        let rgba = vec![0u8; 10 * 10 * 4];
        assert!(make_cursor(&rgba, 32, (0, 0)).is_none(), "长度不符应当拒收");
    }

    /// 光标所在显示器的 DPI 要读得到：它决定光栅化边长。
    ///
    /// 取不到就会退回系统度量——那正是"拖到另一块不同缩放的屏还按旧尺寸画"的老路。
    #[test]
    fn monitor_dpi_is_readable() {
        let dpi = cursor_monitor_dpi().expect("桌面会话里应能读到光标所在显示器的 DPI");
        assert!((48..=768).contains(&dpi), "显示器 DPI 异常：{dpi}");
    }

    /// 系统光标尺寸是个可用读数：造光标时要按它光栅化，取到 0 会让 DIB 建不出来。
    #[test]
    fn system_size_is_sane() {
        let s = system_size();
        assert!((16..=256).contains(&s), "系统光标边长异常：{s}");
    }
}
