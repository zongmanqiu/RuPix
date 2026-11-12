//! 跨线程按窗唤醒：给指定窗口补投一条 `WM_TIMER`，让它的 `on_interval` 立刻跑一拍。
//!
//! # 为什么是 `WM_TIMER`
//!
//! windui 的 `WM_TIMER` 分派只按 `wParam` 做 `index = id - 1`，然后调用
//! `on_interval_fired(index)`；消息是不是系统定时器产生的，它不区分。宿主的看图窗口
//! 只注册了两个 interval：第一个是 16ms 兜底轮询（`POLL`），第二个是 1 小时的
//! keep-alive，专门用来当唤醒目标。解码线程回包后 `PostMessageW(hwnd, WM_TIMER, 2)`
//! 就能让该窗立即跑一次完整 `tick`（收解码回包 → 应用 → 标脏 → 下一帧上屏），
//! 不必等 16ms（实拍约 31ms）的定时器。
//!
//! # 边界
//!
//! - **只在 UI 线程之外调用**：解码监督者在 `Done` 入 `mpsc` 之后调它；`PostMessage`
//!   本身线程安全。
//! - 句柄 0（窗口还没建好）直接返回 `false`，由兜底轮询接住。
//! - 窗口已销毁时 `PostMessageW` 失败是正常竞态，不报错、不重试；解码线程随后因
//!   `done_tx` 发送失败自行退出。
//! - 这里不碰 `WM_PAINT`：`on_interval` 回调返回 `true` 时 windui 自己会
//!   `InvalidateRect`，脏区/动画/预读的收尾仍走原路径。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_TIMER};

/// 向 `hwnd` 补投一条「第 `timer_id` 个 interval 到点」的消息。
///
/// `timer_id` 是 1 起的平台定时器 id（与 windui `on_interval` 的注册顺序一一对应）。
/// 返回是否成功投递；句柄为 0 或窗口已销毁时返回 `false`。
pub fn post_interval_tick(hwnd: isize, timer_id: u32) -> bool {
    if hwnd == 0 {
        return false;
    }
    // SAFETY: HWND 来自 windui 的 `native_window_handle()`（UI 线程写、解码线程读）；
    // PostMessageW 对已销毁句柄只返回错误，不会解引用。消息参数是纯值。
    unsafe {
        PostMessageW(
            Some(HWND(hwnd as *mut std::ffi::c_void)),
            WM_TIMER,
            WPARAM(timer_id as usize),
            LPARAM(0),
        )
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 = 窗口还没建好：必须静默返回 false，绝不对空句柄发消息。
    #[test]
    fn zero_handle_is_a_noop() {
        assert!(!post_interval_tick(0, 2));
    }

    /// 已销毁/伪造的句柄：PostMessage 失败即可，不允许 panic。
    #[test]
    fn dead_handle_is_false_not_panic() {
        assert!(!post_interval_tick(1, 2));
    }
}
