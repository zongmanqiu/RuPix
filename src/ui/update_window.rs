//! 「发现新版本」说明窗：版式对齐 CurSeen 的 `ui/update_window.rs`。
//!
//! - 560×480、可缩放（下限 360×220）、居中、属主窗、同一时刻只有一扇；
//! - 第一行「发现新版本 <版本>」；正文是**记事本式多行文本区**——可拖选、Ctrl+C，
//!   不渲染 Markdown，用户拿到的是能原地复制进 HISTORY 的原文；
//! - 底栏右对齐两颗：「更新」「取消」。点「更新」只投一个请求就关窗，**下载进度与结果
//!   回到关于页的状态行 / 进度条**，不在主窗口弹提示（用户 2026-11-08 点名）。
//!
//! ★ 说明区**自己就是滚动容器**（多行文本框内部滚），外面不许再套 `Element::scroll()`：
//!   外层滚动按无界高度测量子节点，文本框拿不到确定高度就缩成一小块，外层还多出一条
//!   滚动条（CurSeen 2026-10-31 真机踩过，判据见本文件末尾的用例）。
//!
//! 底栏走 `dialog_footer::shell`（本项目铁律：四边 16 / 间距 8 / 行高 40 / 右对齐贴底），
//! 与 CurSeen 那颗「同一处拼底栏」的纪律等价，只是数值取自本项目的版式正本。

use windui::core::EventCtx;
use windui::prelude::*;

use super::dialog_footer::{action, shell};
use super::tokens::{to_color, BG, FONT_MD, TEXT};
use crate::core::update::{self, Plan};

/// 窗宽 / 窗高（逻辑像素），与 CurSeen 说明窗同尺寸。
const W: i32 = 560;
const H: i32 = 480;
/// 可缩放窗的下限：再小就连标题都放不下。
const MIN_W: i32 = 360;
const MIN_H: i32 = 220;
/// 单例键：同一时刻只留一扇说明窗（再检查一次只把内容换掉，不叠窗）。
const KEY: &str = "update-notes";
/// 正文左右留白：文字不贴窗边（记事本也留这一线）。
const PAD_X: i32 = 8;

/// 打开（或聚焦）说明窗。
///
/// 传的是 [`Plan`] 而不是三个字段：按钮要把**同一份**计划原样投回
/// `core::update::Request::Download`，中间不再拼一次地址。
pub fn open(ctx: &mut EventCtx, plan: &Plan) {
    let latest = plan.version.clone();
    let notes = plan.notes.clone();
    let plan = plan.clone();
    ctx.open_window(
        Window::new("发现新版本", W, H)
            .owned(true)
            .resizable(true)
            .min_size(MIN_W, MIN_H)
            .centered(true)
            .bg(to_color(BG))
            .single(KEY)
            .content(move || content(&latest, &notes, plan.clone())),
    );
}

/// 窗内容：标题行 + 文本区 + 底栏。工厂形态（`Window::content` 会被重复调用）。
fn content(latest: &str, notes: &str, plan: Plan) -> Element {
    let title = Element::label(format!("发现新版本 {latest}"))
        .font_size(FONT_MD)
        .fg(to_color(TEXT));

    let body = Element::text_input(signal(notes.to_string()), "")
        .multiline()
        .wrap(true)
        // 无框线、聚焦也不变色；可拖选复制，也可以顺手改两笔（只留在本窗）。
        .frameless_input()
        // 视图起点钉在 0：否则光标（默认在末尾）会把可视区滚到最后几行。
        .select_range(0, 0)
        .font_size(FONT_MD)
        .padding_xy(PAD_X, 6)
        .weight(1.0)
        .width_match();

    let update_btn = action("更新", move |ctx| {
        // 先关窗再投请求（同 CurSeen）：下载进度由关于页那张卡接。
        ctx.request_close();
        if let Err(e) = update::request(update::Request::Download(plan.clone())) {
            log::warn!("发起更新下载失败：{e}");
        }
    });

    shell(
        vec![title, body],
        vec![update_btn, action("取消", |ctx| ctx.request_close())],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::core::Tree;
    use windui::geometry::Size;
    use windui::text::NullTextEngine;

    /// 本文件的生产代码文本（`include_str!` 会把测试自身也包进来）。
    fn production_src() -> &'static str {
        let all = include_str!("update_window.rs");
        let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
        &all[..cut]
    }

    fn plan() -> Plan {
        Plan {
            version: "9.9.9".to_string(),
            notes: "### 修复\n\n- 很长的说明。\n".repeat(60),
            urls: [
                "https://gitee.com/x/RuPix_9.9.9.zip".to_string(),
                "https://github.com/x/RuPix_9.9.9.zip".to_string(),
            ],
        }
    }

    /// 在 560×480 下量出根节点的每一格。
    fn measure() -> (Vec<windui::geometry::Rect>, usize) {
        let p = plan();
        let notes = p.notes.clone();
        let root = content("9.9.9", &notes, p);
        let mut tree = Tree::new();
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(W, H), &mut NullTextEngine);
        let node = tree.get(id).expect("根节点");
        let rects = node
            .children
            .iter()
            .map(|k| tree.abs_bounds(*k))
            .collect::<Vec<_>>();
        let kids = node.children.len();
        (rects, kids)
    }

    /// ★ 回归：正文只占一小块、余下大片空白，还多出一条垂直滚动条——都是外层多套
    /// 了一层滚动容器造成的。判据两条：源码里不许有外层滚动；真实排版下文本区必须
    /// 吃满剩余高度与宽度。
    #[test]
    fn notes_area_fills_the_window_and_owns_its_scroll() {
        let src = production_src();
        assert!(
            !src.contains(".child(Element::scroll()"),
            "说明窗不该再套外层滚动容器（文本框自己滚）"
        );

        let (rects, kids) = measure();
        // 壳的格子：标题 / 文本区 / 弹性空白 / 底栏。
        assert!(rects.len() >= 4, "壳应有「标题 + 文本 + 空白 + 底栏」四格");
        let area = rects[1];
        assert_eq!(
            area.w,
            W - 2 * crate::ui::tokens::DIALOG_PAD,
            "文本区必须铺满内容宽度"
        );
        assert!(
            area.h > H / 3,
            "文本区必须吃掉剩余高度，不能缩成一小块：{} / {}",
            area.h,
            H
        );
        assert!(kids == rects.len(), "格子数与子节点数必须一一对应");
        let foot = *rects.last().expect("底栏");
        assert!(
            foot.y + foot.h <= H,
            "底栏越界：{} > {}",
            foot.y + foot.h,
            H
        );
    }
}
