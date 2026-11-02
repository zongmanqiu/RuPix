//! 系统托盘：关闭到托盘时的**唯一出口**。
//!
//! # 为什么要它
//!
//! 窗口一旦被隐藏（关闭到托盘），任务栏与 Alt-Tab 里都找不到它——没有托盘图标
//! 就等于「关不掉了，也退不出了」。所以只要"关闭转隐藏"这件事**有可能**发生，托盘
//! 就必须在场；而设置里那个开关运行期可翻，于是托盘**恒装配**、图标按开关显隐
//! （`Tray::start_hidden` + `TrayHandle::set_visible`，见 [`build`]）。
//!
//! # 菜单为什么必须自己建
//!
//! 托盘菜单是**原生**菜单（`TrackPopupMenu`），不是画在窗口上的浮层：窗口已经藏了，
//! 浮层无处可画。因此菜单项用 `TrayMenuItem` 逐个列出，而不是复用画布右键菜单那套。
//!
//! 图标复用窗口那张 [`brand`](crate::ui::brand)：窗口、任务栏、托盘三处同一张脸。

use std::cell::RefCell;
use std::rc::Rc;

use windui::prelude::*;

use super::Hub;
use crate::core::config::Config;

/// 托盘图标像素边长（系统再缩到 16 显示；32 是推荐源尺寸）。
const TRAY_ICON_SIZE: u32 = 32;

/// 组装托盘：工具提示 + 图标 + 左键切换显隐 + 右键菜单。
///
/// `shown_at_start` = 装配那一刻「关闭到托盘」开关的值：关着就用 `start_hidden`
/// **连登记都不做**（不是"装上再摘掉"——那样通知区会闪一下）。此后由
/// `runtime` 的 16ms 轮询在开关翻转时调 `TrayHandle::set_visible` 跟随。
///
/// `config` / `hub` 供「进入设置」菜单项复用主窗那条设置单例窗（见
/// [`crate::ui::settings::settings_window`]）：托盘回调拿的是 `TrayCtx` 而非
/// `EventCtx`，故这里走 ctx 无关的 `settings_window`，再由 `TrayCtx::open_window` 开窗。
pub fn build(shown_at_start: bool, config: Rc<RefCell<Config>>, hub: Hub) -> Tray {
    let (w, h, rgba) = super::brand::tray_icon();
    let settings_cfg = config.clone();
    let settings_hub = hub.clone();
    let tray = Tray::new()
        .tooltip("RuPix")
        // 图标像素按值传进构造器（`Tray` 要 'static，借用会活不够长）。
        .icon_rgba(w.max(TRAY_ICON_SIZE), h.max(TRAY_ICON_SIZE), &rgba)
        // 左键单击：**切换**而不是"总是显示"——用户点托盘多半是想收/放。
        // 用 `toggle_window` 而不是自己维护可见标志：隐藏也可能由框架内部发生
        // （ESC 关窗、`hide_on_close`），自建布尔标志迟早与真实状态对不上。
        .on_left_click(|ctx| ctx.toggle_window())
        .menu(vec![
            TrayMenuItem::item("显示窗口", |ctx| ctx.show_window()),
            // 设置窗是主窗的 owned 子窗（随主窗显隐）。从托盘进来时主窗正藏着，
            // 所以先 `show_window` 唤回主窗（owner 可见），owned 设置窗才会跟着现身。
            TrayMenuItem::item("进入设置", move |ctx| {
                ctx.show_window();
                ctx.open_window(super::settings::settings_window(
                    &settings_cfg,
                    &settings_hub,
                ));
            }),
            TrayMenuItem::separator(),
            TrayMenuItem::item("退出", |ctx| ctx.quit()),
        ]);
    if shown_at_start {
        tray
    } else {
        tray.start_hidden()
    }
}
