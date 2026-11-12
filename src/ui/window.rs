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

use super::canvas::PaneGeom;
use super::companion::{CompanionSource, LinkMode, Pane};
use super::{Chrome, Hub, ImageCanvas};
use crate::core::config::Config;
use crate::core::viewer::Viewer;

pub(crate) fn build_root(
    viewer: &Rc<RefCell<Viewer>>,
    chrome: &Rc<Chrome>,
    config: &Rc<RefCell<Config>>,
    hub: &Hub,
) -> Element {
    // 拖放要判“落在主图还是辅图”：画布每帧把两块 pane 几何写进这里，
    // 根节点的拖放回调用系统光标（拖放结束时光标就在落点）去比。
    let pane_geom = PaneGeom::new();
    let drop_geom = pane_geom.clone();
    Element::stack()
        .fill()
        // ★ m269：**根节点不铺底色**。窗口底色由 `App::bg`（宿主每帧清底）负责，
        //   两份写的是同一个 `empty_bg()`，本来冗余；而无边框形态要把那份转成全透明
        //   （windui 补丁 028），根节点再铺一层不透明底会把透明清底整个盖掉——
        //   实测症状是"图片透明区仍是一片 (15,15,15) 实心色"。画布自己的底色照旧（
        //   `ui::canvas` ①底），所以普通形态看不出任何差别。
        // 整窗接收拖放：把文件从资源管理器拖进来即打开。
        .on_drop_files({
            let v = viewer.clone();
            let config = config.clone();
            let chrome = chrome.clone();
            move |ctx, paths| {
                if let Some(p) = paths.first() {
                    let on_companion = chrome.companion.get()
                        && windui::platform::native_window_handle()
                            .and_then(crate::platform::cursor::client_pos_logical)
                            .is_some_and(|(x, y)| drop_geom.hits_companion(x as i32, y as i32));
                    if on_companion {
                        // 拖到右栏 = 当作“自选辅图”：钉住这一张，不再跟主图换。
                        chrome.companion_source.set(CompanionSource::OwnSequence);
                        *chrome.companion_picked.borrow_mut() = Some(p.clone());
                        chrome.companion_link.set(LinkMode::Pinned);
                        chrome.active_pane.set(Pane::Companion);
                    } else {
                        super::raw_dialog::open_path(&v, &config, p);
                        chrome.active_pane.set(Pane::Primary);
                    }
                    ctx.mark_dirty();
                }
            }
        })
        // 画布铺满整窗：图片、悬浮箭头/工具栏/标题栏都由它绘制。
        .child(
            Element::col()
                .fill()
                .widget(
                    ImageCanvas::new(viewer.clone(), chrome.clone(), config.clone(), hub.clone())
                        .with_pane_geom(pane_geom),
                )
                .autofocus(),
        )
}
