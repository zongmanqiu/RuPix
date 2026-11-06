//! 对话框壳与底栏 —— **全项目唯一的按钮行构造点**。
//!
//! 规则写死在 `docs/界面版式规范.md` §2；hygiene 第 14 查（`footer_buttons`）守着：
//! 底栏按钮只要不走本模块，扫描就红。
//!
//! ## 为什么要有这一层（2026-11-06 用户点名）
//!
//! 「通道调色的快捷键窗按钮，和其它插件的不一样」。当时三扇窗各写各的：
//! 插件设置窗 `spacing(0)` + 按钮行内**再加一层** 16、裸数据参数窗 `spacing(10)` 且
//! 按钮紧接内容不贴底、恢复提示窗 `spacing(10)` 靠一颗 `flex_spacer` 自己顶。数字不是
//! 重点，**各自为政**才是——所以这里不做「调数值」，而是把整扇窗收成一个函数，谁都不许再拼。
//!
//! ## 版式（全部取自 [`super::tokens`]，本模块不出现裸数字）
//!
//! | 量 | 值 | 说明 |
//! |---|---|---|
//! | 四边留白 | [`DIALOG_PAD`] = 16 | 左/右/上/下同值 |
//! | 行间距 / 按钮间距 | [`GAP`] = 8 | 内容行之间、相邻按钮之间 |
//! | 行高 | [`ROW_H`] = 40 | 按钮行；内容行由 `option_row_inset` 等给同一高度 |
//! | 对齐 | 按钮一律**右对齐且贴底** | 内容与底栏之间那颗 `flex_spacer` |
//!
//! 于是任何一扇对话框（宽 340 / 480 / 640 都一样）：最右那颗按钮的右缘 = 窗口右缘 − 16，
//! 按钮行下缘 = 窗口底缘 − 16，底栏左缘与内容列同在「窗口左缘 + 16」这条线上。
//!
//! ★ 按钮贴底不是靠"算窗口高度"，是靠那颗弹性空白——`fit_content` 有下限（`DIALOG_FIT_MIN`），
//!   内容比下限矮时窗口不会跟着缩，没有 spacer 就会出现「有的窗按钮悬在半空、有的贴底」。

use windui::core::EventCtx;
use windui::prelude::*;

use super::settings::btn;
use super::tokens::{DIALOG_PAD, GAP, ROW_H};

/// 底栏按钮：外观走 [`btn`]（字号、圆角一处管），点击回调直接挂上。
///
/// 调用点一律写 `action("确定", move |ctx| …)`，别再 `Element::button(...)`——
/// 那条路绕开了本模块，hygiene 第 14 查会红。
pub fn action<F>(label: &str, click: F) -> Element
where
    F: Fn(&mut EventCtx) + 'static,
{
    btn(label).on_click(click)
}

/// 一扇对话框的完整壳：内容在上、按钮行贴底右对齐。
///
/// 参数是 `Vec<Element>` 而不是 `Element`：内容行由各窗自己拼，但**外层容器与底栏
/// 一律由这里给**，窗口四边留白与竖向节奏才不会各写一份。
pub fn shell(content: Vec<Element>, right: Vec<Element>) -> Element {
    shell_split(content, Vec::new(), right)
}

/// 左侧还挂了一组按钮（对象级动作，例如「还原」）时的壳。
///
/// 两组之间是弹性空白，因此右侧那组**永远贴右**，与左侧有几颗按钮无关。
pub fn shell_split(content: Vec<Element>, left: Vec<Element>, right: Vec<Element>) -> Element {
    let mut bar = Element::row()
        .cross(Align::Center)
        .spacing(GAP)
        .height(ROW_H);
    if !left.is_empty() {
        bar = bar.children(left);
    }
    bar = bar.child(Element::flex_spacer());
    let bar = bar.children(right);

    Element::col()
        .fill()
        .padding(DIALOG_PAD)
        .spacing(GAP)
        .children(content)
        // 内容短于 `fit_content` 下限时，由这颗把按钮顶到底（见模块头 ★）。
        .child(Element::flex_spacer())
        .child(bar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::core::Tree;
    use windui::geometry::Size;
    use windui::text::NullTextEngine;

    fn row(label: &str) -> Element {
        Element::row().height(ROW_H).child(Element::label(label))
    }

    /// 在给定窗宽/窗高下量出底栏行与其中各颗按钮的矩形。
    fn measure(width: i32, height: i32) -> (Vec<windui::geometry::Rect>, windui::geometry::Rect) {
        let mut tree = Tree::new();
        let root = shell(
            vec![row("内容一"), row("内容二")],
            vec![
                action("另存为…", |_| {}),
                action("还原", |_| {}),
                action("确定", |_| {}),
            ],
        )
        .width(width);
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(width, height), &mut NullTextEngine);
        let body = tree.get(id).expect("root");
        let bar_id = *body.children.last().expect("footer bar");
        let bar = tree.get(bar_id).expect("footer bar");
        let rects = bar
            .children
            .iter()
            .map(|c| tree.abs_bounds(*c))
            .collect::<Vec<_>>();
        (rects, tree.abs_bounds(bar_id))
    }

    /// ★ 三扇窗宽不同（340 / 480 / 640）、内容高矮不同：按钮必须落在同一条右边线、
    /// 同一条底线上。这条判据盯的是**函数**，不是某一扇窗。
    #[test]
    fn footer_buttons_hug_the_same_corner_at_every_window_size() {
        for (w, h) in [(340, 200), (480, 400), (640, 620)] {
            let (rects, bar) = measure(w, h);
            // 第一格是 flex_spacer，按钮从第二格起。
            let btns = &rects[1..];
            assert_eq!(btns.len(), 3, "三颗按钮都要在");
            let last = btns.last().expect("last");
            assert_eq!(
                last.x + last.w,
                w - DIALOG_PAD,
                "{w}×{h}：最右那颗的右缘 = 窗口右缘 − DIALOG_PAD"
            );
            for pair in btns.windows(2) {
                assert_eq!(
                    pair[1].x - (pair[0].x + pair[0].w),
                    GAP,
                    "{w}×{h}：相邻按钮间距恒为 GAP"
                );
            }
            assert_eq!(
                bar.y + bar.h,
                h - DIALOG_PAD,
                "{w}×{h}：按钮行下缘 = 窗口底缘 − DIALOG_PAD（贴底）"
            );
            assert_eq!(bar.h, ROW_H, "{w}×{h}：按钮行高 = ROW_H");
            assert_eq!(bar.x, DIALOG_PAD, "{w}×{h}：底栏左缘 = DIALOG_PAD");
        }
    }

    /// 内容行与底栏之间恒为一个 `GAP`，且内容左缘与底栏同一条线。
    #[test]
    fn content_and_footer_share_one_left_edge() {
        let mut tree = Tree::new();
        let root = shell(vec![row("内容一")], vec![action("确定", |_| {})]).width(480);
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(480, 300), &mut NullTextEngine);
        let body = tree.get(id).expect("root");
        let content = tree.abs_bounds(body.children[0]);
        let bar = tree.abs_bounds(*body.children.last().expect("bar"));
        assert_eq!(content.x, DIALOG_PAD, "内容左缘 = DIALOG_PAD");
        assert_eq!(bar.x, DIALOG_PAD, "底栏左缘与内容同一条线");
        assert!(
            bar.y - (content.y + content.h) > GAP,
            "窗比内容高时按钮要贴底，不能紧跟着内容走"
        );
    }

    /// 左组存在时右侧照样贴右：`shell_split` 的弹性空白在两组之间。
    #[test]
    fn left_group_never_pushes_the_right_group_off_the_edge() {
        let mut tree = Tree::new();
        let root = shell_split(
            vec![row("内容一")],
            vec![action("还原", |_| {})],
            vec![action("确定", |_| {})],
        )
        .width(480);
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(480, 300), &mut NullTextEngine);
        let body = tree.get(id).expect("root");
        let bar = tree.get(*body.children.last().expect("bar")).expect("bar");
        let rects = bar
            .children
            .iter()
            .map(|c| tree.abs_bounds(*c))
            .collect::<Vec<_>>();
        assert_eq!(rects[0].x, DIALOG_PAD, "左组贴内容左缘");
        let last = rects.last().expect("right");
        assert_eq!(last.x + last.w, 480 - DIALOG_PAD, "右组贴窗口右缘");
    }
}
