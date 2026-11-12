//! 「跳转到…帧/页」小窗：多帧动图 / 多页文档的按号定位（m275 / m277）。
//!
//! 为什么要有它：`(12/340)` 这种进度只**能看不能动**——`Home` / `End` 是清单首尾、
//! `,` `.` 一次一帧，一份 340 页的 PDF 要到第 200 页就是三百多下。而按号跳转在
//! 核心层本来就有：ABI 的 `iv_render` 带 `frame_index`，[`Viewer::jump_anim`] 走的是
//! 与逐帧同一套「暂停 + 在途目标 + 按号请求」。这里只补**一个输入入口**。
//!
//! 不做的三件（都是"看着爽、实际更糟"）：帧缩略图条（要解 N 次）、拖动滑块（拖一次解
//! 一帧，GIF 会卡）、给每帧编快捷键。一份 340 页的文档，用户心里本来就有一个目标号，
//! 输入它是最短的路。
//!
//! 版式**不自定任何尺寸**，全部交给共享底层函数（见 `docs/界面版式规范.md` §2）：
//! 外壳 [`super::dialog_footer::shell`] 给四边留白 / 间距 / 贴底右对齐的按钮行，
//! 内容行走 [`super::settings::option_row_inset`] 传 0（标签落在与底栏同一条竖线），
//! 输入控件宽 [`CONTROL_GROUP_W`]，两颗按钮走 [`super::dialog_footer::action`]。
//! 于是按钮行、四边留白、行间距与其余对话框同一套，内容行的两列版式与插件设置窗
//! 共用 [`super::settings::option_row_inset`] 和 [`DIALOG_W`]。
//! 窗口**按看图窗分格**（`singleton::scoped`），多开时每扇窗有自己的一份。

use std::cell::RefCell;
use std::rc::Rc;

use windui::core::EventCtx;
use windui::prelude::*;

use crate::core::viewer::Viewer;

use super::settings::option_row_inset;
use super::tokens::{to_color, BG, CONTROL_GROUP_W, DIALOG_FIT_MIN, DIALOG_W};

/// 窗口标题与内容行标签**同源**：点开它的就是右键「本图 ▸ 跳转到…帧/页」（m277）。
/// 标题栏与左侧名称写成同一份常量，免得一边改名另一边还留着旧话。
const LABEL: &str = "跳转到…帧/页";

/// 内容行：左侧名称 + 右侧输入框。
///
/// 抽成函数是为了让几何判据直接量**生产用的这一行**，而不是在测试里复述一遍。
/// 行版式走 [`option_row_inset`] 传 0（标签左缘与底栏同线）、控件宽走
/// [`CONTROL_GROUP_W`]（与插件设置窗 / 裸数据参数窗的输入控件同宽右缘对齐）。
fn content_row(
    input: Signal<String>,
    placeholder: String,
    on_submit: impl Fn(&mut EventCtx) + 'static,
) -> Element {
    option_row_inset(
        LABEL,
        Element::text_input(input, placeholder)
            .width(CONTROL_GROUP_W)
            .autofocus_select_all()
            // 输入框里按回车 = 点「跳转」：数字框最自然的提交手势。
            // 底层不冒泡，只能挂在这一层（见 windui 的 `on_submit` 文档）。
            .on_submit(on_submit),
        0,
    )
}

/// 界面输入（1 起）→ 核心索引（0 起）。`None` = 填错了。
///
/// **越界一律拒、不夹**：打多一位（`2000` 之于 340 页）静默跳到末页，比报错更难发现；
/// 而"差一页"是这份功能里最不容易被看出来的错，所以换算与判界只写这一处。
fn parse_index(raw: &str, count: u32) -> Option<u32> {
    let n = raw.trim().parse::<u32>().ok()?;
    if n == 0 || n > count {
        return None;
    }
    Some(n - 1)
}

/// 打开（或聚焦）跳转窗。
///
/// 没有"第几张"可跳的文件（静态图 / 还没解出帧数）当场给一句话，不弹空窗——
/// 弹出个框再告诉用户"没有可跳的"是两次打扰。
pub(crate) fn open(ctx: &mut EventCtx, viewer: &Rc<RefCell<Viewer>>) {
    let Some(anim) = viewer.borrow().anim_info() else {
        ctx.toast_err("这张没有多帧/多页可跳");
        return;
    };
    if anim.count == 0 {
        // GIF 惰性数帧：撞到 EOF 才回填总数，这时按号跳无从夹取。
        ctx.toast_err("这个文件的帧数还没数出来，先播一会儿再跳");
        return;
    }
    let scope = super::singleton::window_scope(viewer);
    let key = super::singleton::scoped("goto", scope);
    let jump: Rc<dyn Fn(u32)> = {
        let viewer = viewer.clone();
        Rc::new(move |index| {
            viewer.borrow_mut().jump_anim(index);
        })
    };
    open_dialog(ctx, key, anim, jump);
}

/// 辅图版本的「跳转到…帧/页」：写回 `CompanionPane` 的页游标，主图不动。
pub(crate) fn open_companion(
    ctx: &mut EventCtx,
    viewer: &Rc<RefCell<Viewer>>,
    pane: &Rc<RefCell<super::companion::CompanionPane>>,
) {
    let Some(anim) = pane.borrow().anim_info() else {
        ctx.toast_err("辅图没有多帧/多页可跳");
        return;
    };
    if anim.count == 0 {
        ctx.toast_err("这个文件的帧数还没数出来，先播一会儿再跳");
        return;
    }
    let scope = super::singleton::window_scope(viewer);
    let key = super::singleton::scoped("goto-companion", scope);
    let jump: Rc<dyn Fn(u32)> = {
        let pane = pane.clone();
        Rc::new(move |index| {
            pane.borrow_mut().jump_index(index);
        })
    };
    open_dialog(ctx, key, anim, jump);
}

/// 两扇「跳转」窗共用的版式与提交逻辑。
fn open_dialog(
    ctx: &mut EventCtx,
    key: String,
    anim: crate::core::pixels::Anim,
    jump: Rc<dyn Fn(u32)>,
) {
    if windui::event::window_open(&key) {
        return;
    }

    let count = anim.count;
    let unit = anim.unit();
    // 预填**当前这张**（1 起）：大多数跳转是"就近挪一点"，从原值改两位比从空框重打省事。
    let input = signal((anim.index + 1).to_string());

    // 同一段落地逻辑挂两处（底栏「跳转」+ 输入框回车）：`Rc<dyn Fn>` 而不是两份闭包，
    // 免得"按钮那版夹取、回车那版不夹取"这类两套规则。
    let submit: Rc<dyn Fn(&mut EventCtx)> = {
        let jump = jump.clone();
        Rc::new(move |ctx: &mut EventCtx| {
            // 填错了必须说话且**不关窗**（与裸数据参数窗同一条规矩）：解析失败就闭窗，
            // 用户看到的是"窗没了、图也没换"，等于点了没反应。
            let Some(index) = parse_index(&input.get(), count) else {
                ctx.toast_err(format!("得填 1…{count} 之间的整数"));
                return;
            };
            jump(index);
            ctx.request_close();
        })
    };
    let confirm = submit.clone();
    let enter = submit.clone();
    let cancel = |ctx: &mut EventCtx| ctx.request_close();

    ctx.open_window(
        // 标题与内容行标签共用 [`LABEL`]：入口（右键菜单）、标题栏、左侧名称三处同名。
        Window::new(LABEL, DIALOG_W, 150)
            .bg(to_color(BG))
            .resizable(false)
            .centered(true)
            // 内容高度按实际行数算；下限与裸数据参数窗共用一格。
            .fit_content(DIALOG_FIT_MIN, 360)
            // 归属主窗：主窗置顶时不会被盖住（与其余对话框同款）。
            .owned(true)
            .single(key)
            .content(move || {
                // 版式全交给共享底层函数：内容行走 `content_row`（内部是
                // `option_row_inset(..., 0)` + `CONTROL_GROUP_W`），按钮行走 `shell`。
                // 标签固定写「跳转到…帧/页」，总数与单位挪到占位提示里，信息不丢。
                super::dialog_footer::shell(
                    vec![content_row(
                        input,
                        format!("1…{count} {unit}"),
                        move |ctx| enter(ctx),
                    )],
                    vec![
                        super::dialog_footer::action("跳转", move |ctx| confirm(ctx)),
                        super::dialog_footer::action("取消", cancel),
                    ],
                )
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::core::Tree;
    use windui::geometry::{Rect, Size};
    use windui::text::NullTextEngine;

    use super::super::tokens::{COL1_W, CONTROL_GROUP_W, ROW_H};

    /// 只取生产段：`include_str!` 会把测试自身包进来，而测试里也写着同名常量，
    /// 不过滤就等于"自己证明自己"。
    fn canvas_production() -> &'static str {
        let all = include_str!("canvas.rs");
        let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
        &all[..cut]
    }

    /// 量一行内容行里各格的绝对矩形（与 `dialog_footer` 的几何判据同一套手法）。
    fn measure_row(row: Element, width: i32) -> Vec<Rect> {
        let mut tree = Tree::new();
        let root = row.width(width);
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(width, ROW_H), &mut NullTextEngine);
        let node = tree.get(id).expect("内容行");
        node.children
            .iter()
            .map(|c| tree.abs_bounds(*c))
            .collect::<Vec<_>>()
    }

    /// 判据：内容行的两列与其它对话框同一套数字，且对话框 inset 必须是 0。
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
        let rects = measure_row(
            content_row(signal("12".into()), "1…340 页".into(), |_| {}),
            DIALOG_W,
        );
        assert_shared_geometry(&rects);
    }

    /// 负向臂①：把设置页那套 inset=16 抄进对话框，几何判据必须当场红。
    #[test]
    #[should_panic(expected = "inset 必须是 0")]
    fn shared_geometry_rejects_settings_page_inset() {
        let row = option_row_inset(
            LABEL,
            Element::text_input(signal(String::new()), "").width(CONTROL_GROUP_W),
            16,
        );
        assert_shared_geometry(&measure_row(row, DIALOG_W));
    }

    /// 负向臂②：自定一个控件宽度（m275 旧版的 120），几何判据必须当场红。
    #[test]
    #[should_panic(expected = "CONTROL_GROUP_W")]
    fn shared_geometry_rejects_hand_rolled_input_width() {
        let row = option_row_inset(
            LABEL,
            Element::text_input(signal(String::new()), "").width(120),
            0,
        );
        assert_shared_geometry(&measure_row(row, DIALOG_W));
    }

    /// 文案同源：右键菜单入口、窗口标题、左侧名称三处必须同名。
    fn assert_menu_entry_matches_label(menu_src: &str) {
        assert!(
            menu_src.contains(&format!("\"{}\"", LABEL)),
            "右键菜单里没有与窗口同名的入口"
        );
    }

    #[test]
    fn menu_entry_window_title_and_row_label_share_one_name() {
        assert_eq!(LABEL, "跳转到…帧/页");
        assert_menu_entry_matches_label(canvas_production());
        let src = include_str!("goto_dialog.rs");
        let src = src.split("\n#[cfg(test)]").next().unwrap_or(src);
        assert!(src.contains("Window::new(LABEL"), "标题栏要复用 LABEL");
        assert!(src.contains("content_row("), "内容行要走共享构造函数");
    }

    /// 负向臂③：菜单项改回旧名，同源判据必须红。
    #[test]
    #[should_panic(expected = "右键菜单里没有与窗口同名的入口")]
    fn name_check_rejects_a_renamed_menu_entry() {
        assert_menu_entry_matches_label("MenuItem::run(\"跳转到…\", |_| {}, false)");
    }

    /// 换算与判界都要真绿真红：上一版这里写的是"把规则在测试里复述一遍"，那种用例
    /// 永远不会红（生产代码改成夹取它照样过）。现在直接喂 [`parse_index`]。
    #[test]
    fn typed_number_maps_to_zero_based_index_and_out_of_range_is_rejected() {
        let count = 340u32;
        assert_eq!(parse_index("1", count), Some(0), "第 1 张 = 索引 0");
        assert_eq!(
            parse_index("340", count),
            Some(339),
            "第 count 张 = count-1"
        );
        assert_eq!(parse_index(" 12 ", count), Some(11), "两头空格要吃掉");
        // 越界：拒，不夹。打多一位静默跳末页是最难发现的那种错。
        assert_eq!(parse_index("0", count), None, "0 越界（界面 1 起）");
        assert_eq!(parse_index("341", count), None, "比总数大一位要拒");
        assert_eq!(parse_index("2000", count), None);
        // 非数字 / 负数 / 空串：一律拒，不许 panic。
        assert_eq!(parse_index("", count), None);
        assert_eq!(parse_index("abc", count), None);
        assert_eq!(parse_index("-3", count), None);
        assert_eq!(parse_index("3.5", count), None);
        // 超大数溢出 `u32` 也不许回绕成一个小号（`parse` 失败 ⇒ 拒）。
        assert_eq!(parse_index("99999999999999999999", count), None);
        // 单帧文件：唯一合法输入是 1（调用点另有 `count > 1` 的启用判据）。
        assert_eq!(parse_index("1", 1), Some(0));
        assert_eq!(parse_index("2", 1), None);
    }
}
