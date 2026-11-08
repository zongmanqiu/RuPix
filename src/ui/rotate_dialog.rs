//! 「旋转角度」小窗：输入整数度数，主窗的图片**立刻**跟着转（m280）。
//!
//! 为什么不做「拉直」：用户 2026-11-09 定案——他要的是能自己试出一个度数，
//! 不是软件替他算角度。所以窗口只收一个整数，改一次主窗重绘一次；试到满意为止，
//! 不用点「确定」才看到效果。
//!
//! ## 实时是怎么过去的
//!
//! 角度是 [`Chrome::rotate_deg`] 这一份 `Signal<f64>`，stepper 直接绑它、**零回调**：
//! 打字与 `[−]/[+]` 都只改这一格。主窗的 `runtime::WinRt::window_tick` 每拍差分
//! 比对 [`Chrome::rotate_seen`]，变了就 `ctx.mark_dirty_all()` —— 与
//! `raw_dialog::poll_pending` 同一条「窗口段轮询」跨窗范式，不依赖框架响应式相位。
//! 实测一拍 ≈31 ms，肉眼就是"边输边转"。
//!
//! 角度绑定图片路径（[`Chrome::rotate_path`]）：换图后旧值自动失效，不用定时器清，
//! 也不会串到另一扇窗（每窗一份 `Chrome`）。会话临时态，不落盘、不写 ini。
//!
//! ## 版式
//!
//! 与「跳转到…帧/页」窗同一套、不自定任何尺寸（见 `docs/界面版式规范.md` §2）：
//! 外壳 [`super::dialog_footer::shell`]，内容行 [`super::settings::option_row_inset`]
//! 传 0，控件宽 [`CONTROL_GROUP_W`]，窗宽 [`DIALOG_W`]，两颗按钮走
//! [`super::dialog_footer::action`]。增减微调按钮是框架 `Element::stepper` 自带的，
//! 不另拼 `Element::button`——省得和别处数值控件的样式走散。
//!
//! 底栏两颗的语义是「确定 = 保留并关窗」「取消 = 回到打开时那个角度并关窗」，
//! 而不是"确定才生效"：预览已经是实时的，按钮只管收尾与撤销。

use std::cell::RefCell;
use std::rc::Rc;

use windui::core::EventCtx;
use windui::prelude::*;

use crate::core::viewer::Viewer;

use super::chrome::Chrome;
use super::settings::option_row_inset;
use super::tokens::{to_color, BG, CONTROL_GROUP_W, DIALOG_FIT_MIN, DIALOG_W};

/// 窗口标题与内容行左侧名称**同源**（m277 同款规矩）；菜单入口是它加省略号。
const LABEL: &str = "旋转角度";

/// 整数度数：`step=1` 时框架 stepper 的 `NumSpec::decimals` 为 0，显示与钳制都是整数。
/// 顺时针为正，±180 覆盖任意朝向（转 181° 与 −179° 等价，没必要再放大范围）。
const MIN_DEG: f64 = -180.0;
const MAX_DEG: f64 = 180.0;
const STEP_DEG: f64 = 1.0;

/// 内容行：左侧名称 + 右侧带 `[−]/[+]` 的整数输入框。
///
/// 抽成函数是为了让几何判据直接量**生产用的这一行**，而不是在测试里复述一遍。
fn content_row(sig: Signal<f64>) -> Element {
    option_row_inset(
        LABEL,
        Element::stepper(sig, MIN_DEG, MAX_DEG, STEP_DEG).width(CONTROL_GROUP_W),
        0,
    )
}

/// 界面信号（f64，stepper 保证整数）→ 核心角度（i32）。`rotated` 只认整数度数。
///
/// 单独一层是为了让「整数」这条判据有落点：框架给的是 `Signal<f64>`，
/// 显示层与另存路径都走这里换算，不许各自 `as i32`（截断方向会不一致）。
pub(crate) fn angle_of(v: f64) -> i32 {
    v.round().clamp(MIN_DEG, MAX_DEG) as i32
}

/// 打开（或聚焦）旋转窗。没有图时一句话，不弹空窗。
pub(crate) fn open(ctx: &mut EventCtx, chrome: &Rc<Chrome>, viewer: &Rc<RefCell<Viewer>>) {
    let Some(path) = viewer.borrow().current_path() else {
        ctx.toast_err("还没有打开文件");
        return;
    };
    let scope = super::singleton::window_scope(viewer);
    let key = super::singleton::scoped("rotate", scope);
    if windui::event::window_open(&key) {
        return;
    }

    // 换图后旧角度失效：登记新路径并归零（与 `LiveTint.path` 同一条判据）。
    if chrome.rotate_path.get().as_deref() != Some(path.as_path()) {
        chrome.rotate_deg.set(0.0);
        chrome.rotate_path.set(Some(path));
    }
    let opening = chrome.rotate_deg.get();
    // `Signal` 是 Copy：这里拿的是同一格信号的句柄，不是拷贝一份值。
    let sig = chrome.rotate_deg;

    // 「取消」把角度还原到打开这一刻，下一拍 tick 差分感知后主窗转回去。
    let ch_cancel = chrome.clone();
    let cancel = move |ctx: &mut EventCtx| {
        ch_cancel.rotate_deg.set(opening);
        ctx.request_close();
    };
    let confirm = |ctx: &mut EventCtx| ctx.request_close();

    let ch_window = chrome.clone();
    ctx.open_window(
        Window::new(LABEL, DIALOG_W, 150)
            .bg(to_color(BG))
            .resizable(false)
            .centered(true)
            .fit_content(DIALOG_FIT_MIN, 360)
            .owned(true)
            .single(key)
            .content(move || {
                super::dialog_footer::shell(
                    vec![content_row(sig)],
                    vec![
                        super::dialog_footer::action("确定", confirm),
                        super::dialog_footer::action("取消", cancel),
                    ],
                )
            }),
    );
    // 每窗一份：窗口关掉时不用撤角度（角度属于看图窗，不属于这扇小窗）。
    let _ = ch_window;
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::core::Tree;
    use windui::geometry::{Rect, Size};
    use windui::text::NullTextEngine;

    use super::super::tokens::{COL1_W, ROW_H};

    /// 只取生产段：`include_str!` 会把测试自身包进来，而测试里也写着同名常量，
    /// 不过滤就等于"自己证明自己"。
    fn canvas_production() -> &'static str {
        let all = include_str!("canvas.rs");
        let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
        &all[..cut]
    }

    fn measure_row(row: Element, width: i32) -> Vec<Rect> {
        let mut tree = Tree::new();
        let id = row.width(width).build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(width, ROW_H), &mut NullTextEngine);
        let node = tree.get(id).expect("内容行");
        node.children
            .iter()
            .map(|c| tree.abs_bounds(*c))
            .collect::<Vec<_>>()
    }

    fn assert_shared_geometry(rects: &[Rect]) {
        assert!(rects.len() >= 2, "内容行要有标签格与控件格");
        assert_eq!(
            rects[0].x, 0,
            "对话框 inset 必须是 0（标签左缘与底栏同一条竖线）"
        );
        assert_eq!(rects[0].w, COL1_W, "标签列宽要吃令牌 COL1_W");
        assert_eq!(rects[1].x, COL1_W, "控件左缘紧贴标签列");
        assert_eq!(
            rects[1].w, CONTROL_GROUP_W,
            "输入控件宽要吃令牌 CONTROL_GROUP_W"
        );
    }

    #[test]
    fn content_row_matches_the_shared_dialog_two_column_geometry() {
        let rects = measure_row(content_row(signal(0.0)), DIALOG_W);
        assert_shared_geometry(&rects);
    }

    /// 负向臂①：把设置页那套 inset=16 抄进对话框，几何判据必须当场红。
    #[test]
    #[should_panic(expected = "inset 必须是 0")]
    fn shared_geometry_rejects_settings_page_inset() {
        let row = option_row_inset(LABEL, Element::stepper(signal(0.0), 0.0, 10.0, 1.0), 16);
        assert_shared_geometry(&measure_row(row, DIALOG_W));
    }

    /// 负向臂②：自定控件宽度，几何判据必须当场红。
    #[test]
    #[should_panic(expected = "CONTROL_GROUP_W")]
    fn shared_geometry_rejects_hand_rolled_input_width() {
        let row = option_row_inset(
            LABEL,
            Element::stepper(signal(0.0), 0.0, 10.0, 1.0).width(120),
            0,
        );
        assert_shared_geometry(&measure_row(row, DIALOG_W));
    }

    /// 整数步长的唯一判据：正向喂生产常量，负向臂喂 0.5。
    fn assert_integer_step(step: f64) {
        assert_eq!(step, 1.0, "微调按钮一次只能走 1°");
    }

    /// 度数必须是整数：步长 1、范围两端整数、换算走 `round` 而不是截断。
    #[test]
    fn degrees_are_integers_and_rounding_is_one_place() {
        assert_integer_step(STEP_DEG);
        assert_eq!(MIN_DEG.fract(), 0.0);
        assert_eq!(MAX_DEG.fract(), 0.0);
        assert_eq!(angle_of(2.4), 2);
        assert_eq!(angle_of(2.6), 3);
        assert_eq!(angle_of(-2.5), -3, "f64::round 远离零");
        assert_eq!(angle_of(999.0), 180, "越界夹到端点，不留野值");
        let src = include_str!("rotate_dialog.rs");
        let prod = &src[..src.find("\n#[cfg(test)]").unwrap_or(src.len())];
        assert!(
            prod.contains("Element::stepper(sig, MIN_DEG, MAX_DEG, STEP_DEG)"),
            "必须直接复用框架 stepper，不许自己拼 −/+ 按钮"
        );
        assert!(
            prod.contains("dialog_footer::shell") && prod.contains("DIALOG_W"),
            "窗口壳与窗宽必须走共享函数/令牌"
        );
    }

    /// 负向臂③：把步长改成 0.5（会出现小数度数），整数判据必须当场红。
    #[test]
    #[should_panic(expected = "一次只能走 1°")]
    fn integer_degrees_rejects_fractional_step() {
        assert_integer_step(0.5);
    }

    /// 实时联动的跨窗链路钉子：主窗 tick 必须差分角度并整窗标脏。
    /// 少任何一环，用户看到的就是"输完数字没反应 / 关掉窗口才变"。
    #[test]
    fn live_preview_bridge_is_wired_in_window_tick() {
        let src = include_str!("../runtime/mod.rs");
        let prod = &src[..src.find("\n#[cfg(test)]").unwrap_or(src.len())];
        assert!(
            prod.contains("self.chrome.rotate_deg.get()"),
            "主窗 tick 没在读角度信号"
        );
        assert!(
            prod.contains("self.chrome.rotate_seen.set(deg)"),
            "差分记账没了，空闲主窗会每拍空转重绘"
        );
        assert!(
            prod.contains("ctx.mark_dirty_all()"),
            "角度变了没标脏主窗，实时预览会停在上一帧"
        );
    }

    /// 文案同源：菜单入口 = 标题 + 省略号，标题与左侧名称是同一份常量。
    #[test]
    fn menu_entry_window_title_and_row_label_share_one_name() {
        assert_eq!(LABEL, "旋转角度");
        assert!(
            canvas_production().contains("\"旋转角度…\""),
            "右键「本图 ▸」里没有与窗口同名的入口"
        );
    }
}
