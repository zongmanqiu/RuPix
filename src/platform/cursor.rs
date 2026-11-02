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

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

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
}
