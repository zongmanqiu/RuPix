//! 窗口内容树：无边框窗的根元素。
//!
//! 菠萝看图式极简布局：没有常驻工具栏与状态栏——画布铺满整窗，翻页箭头、工具栏、
//! 信息条与顶部标题区**全部由画布绘制**（见 `ui::chrome`）。拖动也一样走画布：
//! 指针落在顶部标题区里按下时，画布直接调 `start_window_drag`（见 `ui::canvas`），
//! 标题文字是画出来的，不是控件。窗口按钮（最小化 / 最大化 / 关闭）同理是画布命中区。
//!
//! 装配（App 构建、轮询、快捷键）在 `crate::runtime`——本模块只管元素树。

use std::cell::RefCell;
use std::rc::Rc;

use windui::prelude::*;

use super::tokens::{to_color, OVERLAY_EMPTY_BG};
use super::{Chrome, Hub, ImageCanvas};
use crate::core::config::Config;
use crate::core::viewer::Viewer;

/// 窗口与画布共用的深色（恒定深色 UI，不走主题 Role）。
///
/// ★ 数值取自全 ui 共用的令牌：`canvas` 的 `EMPTY_BG` 与它本是同一块深黑，
///   以前各写一遍 `15,15,15`，改色只改一处必然漏另一处。
pub(crate) const DARK: Color = to_color(OVERLAY_EMPTY_BG);

pub(crate) fn build_root(
    viewer: &Rc<RefCell<Viewer>>,
    chrome: &Rc<Chrome>,
    config: &Rc<RefCell<Config>>,
    hub: &Hub,
) -> Element {
    Element::stack()
        .fill()
        .bg(DARK)
        // 整窗接收拖放：把文件从资源管理器拖进来即打开。
        .on_drop_files({
            let v = viewer.clone();
            let config = config.clone();
            move |ctx, paths| {
                if let Some(p) = paths.first() {
                    super::raw_dialog::open_path(&v, &config, p);
                    ctx.mark_dirty();
                }
            }
        })
        // 画布铺满整窗：图片、悬浮箭头/工具栏/标题栏都由它绘制。
        .child(
            Element::col()
                .fill()
                .widget(ImageCanvas::new(
                    viewer.clone(),
                    chrome.clone(),
                    config.clone(),
                    hub.clone(),
                ))
                .autofocus(),
        )
}
