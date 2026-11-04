//! 系统托盘：**整个进程唯一**的那一个（多窗口时代仍然只有一个）。
//!
//! # 为什么要它
//!
//! 进程是常驻形态（`App::run_resident`）：没有主窗，窗口全关也能活着。所以托盘必须
//! 恒在场——它是零窗口时唯一的入口（开窗 / 进设置）与唯一出口（退出）。图标是否显示
//! 由「关闭到托盘」决定（`Tray::start_hidden` + `TrayHandle::set_visible`，见 [`build`]）。
//!
//! # 菜单为什么必须自己建
//!
//! 托盘菜单是**原生**菜单（`TrackPopupMenu`），不是画在窗口上的浮层：零窗口时浮层
//! 无处可画。因此菜单项用 `TrayMenuItem` 逐个列出，而不是复用画布右键菜单那套。
//!
//! ★ 2026-11-04 多窗口改造后，「显示窗口」不再指主窗（没有主窗了），而是交给宿主
//! 注入的回调：有窗口就激活最近那扇，一扇都没有就开一扇新的。托盘自己不碰窗口——
//! 它拿不到 `Viewer`，也不该知道窗口长什么样。
//!
//! 图标复用窗口那张 [`brand`](crate::ui::brand)：窗口、任务栏、托盘三处同一张脸。

use std::cell::RefCell;
use std::rc::Rc;

use windui::prelude::*;

use super::Hub;
use crate::core::config::Config;

/// 托盘图标像素边长（系统再缩到 16 显示；32 是推荐源尺寸）。
const TRAY_ICON_SIZE: u32 = 32;

/// 托盘上的一个动作（由 `runtime` 注入：托盘不认识看图窗口）。
pub type TrayActionFn = Rc<dyn Fn(&mut TrayCtx)>;

/// 组装托盘：工具提示 + 图标 + 左键唤出 + 右键菜单。
///
/// `shown_at_start` = 装配那一刻「关闭到托盘」开关的值：关着就用 `start_hidden`
/// **连登记都不做**（不是"装上再摘掉"——那样通知区会闪一下）。此后由 `runtime` 的
/// 应用级轮询在开关翻转时调 `TrayHandle::set_visible` 跟随。
///
/// `wake` = 显示/唤出看图窗口（已有就激活，没有就新建）；`new_win` = 明确新建一扇。
///
/// `config` / `hub` 供「进入设置」菜单项复用主窗那条设置单例窗（见
/// [`crate::ui::settings::settings_window`]）：托盘回调拿的是 `TrayCtx` 而非
/// `EventCtx`，故这里走 ctx 无关的 `settings_window`，再由 `TrayCtx::open_window` 开窗。
pub fn build(
    shown_at_start: bool,
    config: Rc<RefCell<Config>>,
    hub: Hub,
    wake: TrayActionFn,
    new_win: TrayActionFn,
) -> Tray {
    let (w, h, rgba) = super::brand::tray_icon();
    let settings_cfg = config.clone();
    let settings_hub = hub.clone();
    let left_click = wake.clone();
    let show_item = wake.clone();
    let tray = Tray::new()
        .tooltip("RuPix")
        // 图标像素按值传进构造器（`Tray` 要 'static，借用会活不够长）。
        .icon_rgba(w.max(TRAY_ICON_SIZE), h.max(TRAY_ICON_SIZE), &rgba)
        // 左键单击：把用户正在看的那扇窗叫回来（一扇都没有就开一扇）。
        // 多窗口之后没有"唯一主窗"可谈，这件事只有宿主知道。
        .on_left_click(move |ctx| (left_click)(ctx))
        .menu(vec![
            TrayMenuItem::item("显示窗口", move |ctx| (show_item)(ctx)),
            TrayMenuItem::item("新建窗口", move |ctx| (new_win)(ctx)),
            TrayMenuItem::separator(),
            // 设置窗是普通顶层窗（多窗口时代不再挂 owner）：从托盘进来时没有任何窗口
            // 在场，`owned` 会指向一个不存在的发起者。
            TrayMenuItem::item("进入设置", move |ctx| {
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
