//! 纯文本窗：系统标题栏 + 一片多行文本区，高度随内容。
//!
//! 做法对齐 CurSeen 的脚本信息窗（`ui/script_dialog.rs` 的信息档）：
//! - **没有按钮、没有页脚**——整窗就是一片文本区；可以直接点进去改（改动只留在这个
//!   窗里的本地草稿，不回写任何地方），选中后 Ctrl+C / Ctrl+A 复制；
//! - **无框线、白底**：`.frameless_input()` + 白色窗口底，聚焦也不会出现边框变色；
//! - **高度按内容算**（CJK 一字 ≈ 字号宽，ASCII 取半宽），钳在上下限内；
//! - 关窗走标题栏 X 或 Esc。
//!
//! 为什么不用 `Element::label` + `scroll`：label 不可选中，用户想复制其中一段只能
//! 整段复制；多行输入框天然支持拖选、双击选词、Ctrl+C，也允许顺手改两笔。
//!
//! 这是给"展示一段文字"用的公共窗：插件结果、诊断输出、以后脚本返回的文本都走它。

use windui::core::EventCtx;
use windui::prelude::*;

use super::tokens::{to_color, BG, FONT_MD};

/// 窗宽（逻辑像素）。与 CurSeen 的文本窗同宽：一行放得下约 35 个汉字。
pub const W: i32 = 520;
/// 文本区左右留白：文字不贴窗边（记事本也留这一线）。
const PAD_X: i32 = 8;
/// 一行的估算高度（只用于估窗高；真正折行由文本框自己做）。
const LINE_H: i32 = 22;
/// 正文区高度的上下限：太短时别开成一枚小方块，太长时交给区内滚动。
const BODY_MIN_H: i32 = 120;
const BODY_MAX_H: i32 = 520;
/// 窗高下限：低于这个数，文本区连一行都看不全。
const WINDOW_MIN_H: i32 = 160;
/// 可缩放窗的最小尺寸（用户拖小也要能读一行）。
const MIN_W: i32 = 320;
const MIN_H: i32 = 120;

/// 一个可见行按宽度会折成几行：只用于估高，不参与排版。
fn visual_lines(text: &str) -> usize {
    let avail = (W - 2 * PAD_X - 16).max(1) as f32;
    let mut total = 0usize;
    for line in text.split('\n') {
        let px: f32 = line
            .chars()
            .map(|c| {
                if (c as u32) >= 0x3000 {
                    FONT_MD
                } else {
                    FONT_MD * 0.55
                }
            })
            .sum();
        total += ((px / avail).ceil() as usize).max(1);
    }
    total
}

/// 这扇窗该开多高。纯函数，单测可无头钉住上下限。
pub fn height_for(text: &str) -> i32 {
    (visual_lines(text) as i32 * LINE_H + 6)
        .clamp(BODY_MIN_H, BODY_MAX_H)
        .max(WINDOW_MIN_H)
}

/// 打开（或聚焦）一扇纯文本窗。
///
/// `key` 是单例键：同一个键再次打开会先关旧窗，保证内容是最新一次，而不是残影。
/// `title` 走系统标题栏（任务栏 / Alt+Tab 也认得）。
pub fn show(ctx: &mut EventCtx, key: &str, title: &str, text: &str) {
    let body = text.to_string();
    let h = height_for(text);
    let sig = signal(body);
    let key = key.to_string();
    let title = title.to_string();
    // single 窗已存在时只聚焦、不重建内容；先关旧窗，保证看到的是这一次的文本。
    ctx.close_window(key.clone());
    ctx.open_window(
        Window::new(title, W, h)
            // 纯白底：文本区 frameless 后露出的就是这层底（对齐 CurSeen 信息档）。
            .bg(to_color(BG))
            .resizable(true)
            .min_size(MIN_W, MIN_H)
            .centered(true)
            .owned(true)
            .single(key)
            .content(move || {
                Element::text_input(sig, "")
                    .multiline()
                    .wrap(true)
                    // 无框线、聚焦也不变色；可点进去编辑，也可选中复制。
                    .frameless_input()
                    // 视图起点钉在 0：否则 windui 会把光标（默认在末尾）滚进可视区，
                    // 开窗第一眼看到的是最后几行。
                    .select_range(0, 0)
                    .font_size(FONT_MD)
                    .padding_xy(PAD_X, 6)
                    .fill()
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_uses_the_minimum_height() {
        assert_eq!(height_for("OK"), WINDOW_MIN_H);
    }

    #[test]
    fn long_text_is_capped_and_never_exceeds_one_screen() {
        let long = "行\n".repeat(400);
        assert_eq!(height_for(&long), BODY_MAX_H);
    }

    #[test]
    fn more_lines_never_give_a_smaller_window() {
        let h1 = height_for("第一行");
        let h2 = height_for("第一行\n第二行\n第三行\n第四行\n第五行");
        assert!(h2 >= h1, "行数更多不该反而更矮：{h1} -> {h2}");
    }

    #[test]
    fn cjk_wraps_earlier_than_ascii() {
        // 60 个汉字约 840px，必然折行；60 个 ASCII 约 462px，仍在 496px 内。
        assert!(visual_lines(&"汉".repeat(60)) >= 2);
        assert_eq!(visual_lines(&"a".repeat(60)), 1);
    }
}
