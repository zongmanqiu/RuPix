//! 更新流程的 **UI 可见状态**（版式与文案对齐 CurSeen 的 `update/state.rs`）。
//!
//! `Signal` 是 windui 的类型，`core` 不许引 windui ⇒ 这份状态既进不了
//! `core::update`（纯逻辑层），也不该散在关于页里：更新是**后台发起、可能在设置页
//! 关着的时候完成**的动作，状态跟着页面走就会出现「点过一次检查、结果没人接」。
//! 所以它随 `Handles` 常驻，页面重开时读到的是最新状态而不是初始值。
//!
//! ★ **状态行是唯一反馈通道**（用户 2026-11-08 点名）：点「检查更新」/「更新」之后
//!   不许再往主窗口弹 toast，提示一律落在关于页那张卡的状态行与进度条上。

use windui::prelude::*;

use crate::core::update::{self, Version};

/// 关于页可见的更新状态。
#[derive(Clone, Copy)]
pub struct UpdateState {
    /// 状态行文本：「正在检查更新…」/「已是最新版本 0.1.0」/「检查失败：…」。
    pub status: Signal<String>,
    /// 下载进度 0..=1；`-1` = 未知总量 / 尚未开始 / 失败。
    pub progress: Signal<f32>,
    /// 检查或下载进行中：按钮置灰，避免并发起两次（同 CurSeen）。
    pub busy: Signal<bool>,
    /// 下载已开始（含"还没收到第一块数据"）：按钮让位给进度条。
    pub downloading: Signal<bool>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateState {
    /// 在 UI 线程创建（`Signal` 是 `!Send`，只能在这儿建）。
    pub fn new() -> Self {
        // 本地清单是"离线也知道自己旧不旧"的底账：启动时先给一句人话，
        // 点「检查更新」再联网替换它。
        let cached = update::load_cached()
            .and_then(|t| update::parse_manifest(&t).ok())
            .and_then(|m| Version::parse(&m.version));
        Self {
            status: signal(initial_status(cached)),
            progress: signal(-1.0f32),
            busy: signal(false),
            downloading: signal(false),
        }
    }
}

/// 启动时状态行的文案（纯函数，便于钉住"旧缓存不冒充最新"）。
///
/// - 缓存比当前新 ⇒ 可更新；
/// - 相等 ⇒ 已是最新；
/// - **缓存比当前旧 / 没有缓存 ⇒ 尚未检查**：升级后安装目录里的缓存还是上一版，
///   这时说"已是最新版本"是假话（CurSeen 2026-10-31 真机踩过）。
fn initial_status(cached: Option<Version>) -> String {
    let current = Version::current();
    match cached {
        Some(v) if v > current => format!("本地清单显示 {v} 可更新"),
        Some(v) if v == current => format!("已是最新版本 {v}"),
        _ => "尚未检查更新".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_idle_and_has_no_progress() {
        let s = UpdateState::new();
        assert!(!s.busy.get());
        assert!(!s.downloading.get());
        assert_eq!(s.progress.get(), -1.0);
        assert!(!s.status.get().is_empty(), "启动时也该有一句人话");
    }

    /// ★ 缓存比当前旧时不许说"已是最新"：本地换成 0.0.1 之后，缓存还停在上一版。
    #[test]
    fn stale_cache_is_not_reported_as_up_to_date() {
        let current = Version::current();
        assert_eq!(
            initial_status(Some(current)),
            format!("已是最新版本 {current}")
        );
        assert_eq!(initial_status(None), "尚未检查更新");
        assert_eq!(initial_status(Some(Version::ZERO)), "尚未检查更新");

        let newer = Version::parse("99.0.0").expect("可解析");
        assert!(
            initial_status(Some(newer)).contains("可更新"),
            "缓存比当前新时必须提示可更新"
        );
    }
}
