//! 无边框窗口的外框反算。

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::WindowsAndMessaging::{WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW};

/// 无边框窗口在 windui 里按 `WS_OVERLAPPEDWINDOW` 反算外框（`frame_size_for_client`）：
/// 那一圈系统边框实际不显示，却会算进窗口尺寸，使**可见窗口**比请求值大一圈。
/// 这里取回该边框的逻辑像素尺寸（在 96 DPI 基准上量，随 DPI 等比放大），
/// 供请求尺寸减去它，让可见窗口恰好是所需大小。
pub fn frameless_frame() -> (i32, i32) {
    const BASE: i32 = 1000;
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: BASE,
        bottom: BASE,
    };
    unsafe {
        let _ =
            AdjustWindowRectExForDpi(&mut rc, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0), 96);
    }
    (rc.right - rc.left - BASE, rc.bottom - rc.top - BASE)
}
