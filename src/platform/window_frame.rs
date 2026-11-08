//! 无边框窗口的外框反算。

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SystemParametersInfoW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
};

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

/// 主显示器**工作区**（减去任务栏）的物理像素尺寸。
///
/// m269：框架的 `set_client_size` 是**分别**夹宽高的，直接把 6000×4000 丢过去会得到
/// 一个 1920×1080 的 16:9 窗口，图片形状就变了。宿主侧得先按这份面积等比缩一次。
/// 查不到工作区时退回整屏尺寸，不让等比计算拿到 0。
pub fn work_area() -> (u32, u32) {
    let mut rc = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rc as *mut _ as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
    };
    if ok && rc.right > rc.left && rc.bottom > rc.top {
        ((rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32)
    } else {
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        (w.max(1) as u32, h.max(1) as u32)
    }
}
