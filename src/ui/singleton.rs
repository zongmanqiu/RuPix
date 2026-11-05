//! 「每次打开都要重建内容」的单例窗：给每一次开窗分配一个**新的**单例键。
//!
//! `EventCtx::close_window` 是投递到应用级旁路队列的——下一次 `WM_TIMER` / `WM_HOTKEY`
//! 收尾时才真正找窗投 `WM_CLOSE`；`open_window` 则在窗口回调收尾时就建窗。于是同一个
//! 回调里对**同一个键**既关又开，那条滞留的关窗请求会按 key 找到刚刚开出来的新窗，
//! 把它当旧窗收走。用户看到的是「窗闪一下就没了」（2026-11-04 插件页齿轮实测：
//! 设置页自带 250 ms 定时器，故每次必现）。框架给的出口是「分两次回调，或用不同的键」，
//! 这里选后者：关上一次的键、开这一次的键。
//!
//! 两个消费方都要「内容来自这一次的读盘」：[`super::plugin_settings`] 的控件树按 schema
//! 现搭，[`super::text_window`] 的正文是这一次的文本；`single` 已开时只聚焦、不重建树，
//! 所以不能省掉关旧窗那一步。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

thread_local! {
    /// 每个 base 用掉到第几号。进程内有效，**窗口关掉也不回退**：回退就会复用刚被
    /// `WM_CLOSE` 收走的那个键，重新踩回同一个竞态。
    static SEQ: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// 一扇主窗的**进程内标识**：拿它给「每窗一份」的附属窗与待办分格。
///
/// 用 `Rc<RefCell<Viewer>>` 的堆地址：一扇窗一份 `Rc`，窗口活着期间恒定，而且两扇
/// 活着的窗不可能共用同一个地址。窗口关闭时 `WinRt::close` 会把以它为键的东西一并
/// 清掉（`raw_dialog::forget_window`、`settings::forget_metadata_window`），所以地址
/// 将来被新窗复用时也捡不到上一个窗的遗留。
pub(crate) fn window_scope(viewer: &Rc<RefCell<crate::core::viewer::Viewer>>) -> usize {
    Rc::as_ptr(viewer) as usize
}

/// 把附属窗的单例键按窗口作用域拼出来（`raw-params:140234…`）。
pub(crate) fn scoped(base: &str, scope: usize) -> String {
    format!("{base}:{scope}")
}

/// 返回 `(该关掉的旧键, 本次该开的键)`。
///
/// 首次调用时旧键是 `#0`——那扇窗从未存在，`close_window` 是空操作；之后每次两个键
/// 都不同，且旧键只可能指向上一扇窗，不会误伤本次新开的窗。
pub(crate) fn recycle(base: &str) -> (String, String) {
    SEQ.with(|seq| {
        let mut seq = seq.borrow_mut();
        let n = seq.entry(base.to_string()).or_insert(0);
        *n += 1;
        let cur = *n;
        (format!("{base}#{}", cur - 1), format!("{base}#{cur}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_and_open_keys_never_collide() {
        let (old, new) = recycle("plugin-settings:levels");
        assert_ne!(old, new, "同键既关又开正是那个 bug");
        let (old2, new2) = recycle("plugin-settings:levels");
        assert_eq!(new, old2, "下一轮要关的正是上一轮开的那扇");
        assert_ne!(old2, new2);
        assert_ne!(new, new2, "每次开窗的键都不能复用");
    }

    #[test]
    fn different_bases_have_independent_sequences() {
        assert_eq!(recycle("text:one").1, "text:one#1");
        assert_eq!(recycle("text:two").1, "text:two#1");
    }

    #[test]
    fn first_close_key_points_at_a_window_that_never_existed() {
        let (old, new) = recycle("fresh-base-for-first-open");
        assert_eq!(old, "fresh-base-for-first-open#0");
        assert_eq!(new, "fresh-base-for-first-open#1");
    }
}
