//! 文件关联窗（m244）：全后缀勾选网格 + 一次性写入。
//!
//! 设计（用户 2026-11-07 定稿）：独立窗口从「选项 → 系统」进；4 列网格一屏
//! 扫完 150+ 后缀；**打开时全不勾**（一次性快照）；「全选/取消全选」双态按钮 +
//! 「应用」——点一次 = 按当前勾选写入覆盖一次，完成后弹文本结果窗（成功 N /
//! 本就是默认 M / 候选已挂 K+名单），勾选复位全不勾。写入本体在
//! [`crate::platform::association`]（同行方案：HKCU legacy 直写 + UserChoice 哈希退让），
//! 本文件只管取数、布局与回调。
//!
//! # 版式（2026-11-07 用户两轮实测返修后定稿）
//!
//! 复用设置页的页面骨架 + `dialog_footer` 的 **16 / 8 / 40** 三个数字：
//! 根列只给 `bottom = DIALOG_PAD` 与 `spacing(GAP)`、**水平留白为 0**；
//! 滚动区 = [`Element::scroll`].fill().weight(1.0).padding_xy(16, 8)——windui 的
//! 垂直滚动条画在滚动节点 `abs.right()` 上，padding 只收内容不收节点边，滚动条因此
//! 与设置页一样贴右缘；底栏自带水平 16、行高 `ROW_H`、按钮间距 `GAP`。按钮一律走
//! [`dialog_footer::action`]（hygiene 第 14 查的门）。
//! 三版废案：① scroll 嵌 `shell_split` + 搜索框抢 fill ⇒ 滚动区无界、底栏被顶出窗；
//! ② 根列四边 16 ⇒ 滚动节点内缩 16、滚动条离右缘一整格（用户 18:34 实测）；
//! ③ 底栏自己不垫 16 ⇒ 按钮与其它对话框不同线。布局钉子见 tests。
//!
//! # 搜索（2026-11-07 用户实测返修）
//!
//! 框架 `Signal` 只有「读时重算」，`build()` 里直接 `search.get()` 只在开窗那一帧求值，
//! 打字不会重建树——首版因此输入不过滤。与解码器页同款：**`search.map()` 派生过滤视图
//! （`Vec<Vec<usize>>`，每行 4 个下标）+ `host_signal` 当帧重建网格**，无定时器、无差分。

use windui::core::EventCtx;
use windui::prelude::*;

use super::dialog_footer;
use super::singleton;
use super::text_window;
use super::tokens::{to_color, BG, DIALOG_PAD, GAP, PAGE_TOP_MARGIN, ROW_H, TEXT_ACCENT};
use crate::core::formats;
use crate::platform::association;

/// 网格一列的格宽：后缀名 ≤5 字符，130 px 富余；620 窗宽 − 16 左衬 ≈ 4 列整。
const CELL_W: i32 = 130;
/// 网格行高：勾选框比按钮行矮，30 够呼吸。
const GRID_ROW_H: i32 = 30;
/// 列数（4 = 一屏 30 行 × 4 ≈ 120 项，余下靠搜索与滚动）。
const COLS: usize = 4;

/// 打开文件关联窗。入口唯一：选项页「系统」卡的管理按钮。
pub(crate) fn show(ctx: &mut EventCtx) {
    // 后缀全集 = 解码器自述并集（与解码器页同一来源）；排序保证每次开窗顺序稳定。
    let mut exts: Vec<String> = formats::declared_exts()
        .into_iter()
        .filter_map(|e| association::normalize(&e))
        .collect();
    exts.sort();
    let sigs: Vec<Signal<bool>> = exts.iter().map(|_| signal(false)).collect();
    let search: Signal<String> = signal(String::new());
    // 派生过滤视图：搜索词一变，行分组当帧重算 → host_signal 当帧重建网格。
    let view: Signal<Vec<Vec<usize>>> = search.map({
        let exts = exts.clone();
        move |q| filtered_rows(&exts, q)
    });

    // ★ 先关旧窗再开新窗、两个键必须不同——插件设置窗踩过的坑（同键 WM_CLOSE 收走新窗）。
    let (old_key, win_key) = singleton::recycle("file-assoc");
    ctx.close_window(old_key);
    ctx.open_window(
        Window::new("文件关联", 620, 520)
            .bg(to_color(BG))
            .resizable(false)
            .centered(true)
            .owned(true)
            .single(win_key)
            .content(move || build(&exts, &sigs, &search, view)),
    );
}

/// 过滤（纯函数，供测试钉住）：搜索词去点、不区分大小写、空词 = 全显；
/// 然后把命中下标按 `COLS` 切成一行一组，交给 `host_signal` 逐行渲染。
fn filtered_rows(exts: &[String], query: &str) -> Vec<Vec<usize>> {
    let q = query.trim().trim_start_matches('.').to_ascii_lowercase();
    let visible: Vec<usize> = (0..exts.len())
        .filter(|&i| q.is_empty() || exts[i].contains(&q))
        .collect();
    visible.chunks(COLS).map(|c| c.to_vec()).collect()
}

/// 底栏计数文案（纯函数，供测试钉住）：没勾 = 「共 N 种后缀」；勾了 = 「已选中 x/N」。
///
/// 两个数都指**全集**（解码器自述并集），不是搜索框过滤后的可见集——搜索只让人找得快，
/// 不改变"一共有多少种 / 你选中了几种"这件事。每帧现算一遍 150 次布尔读，成本可忽略；
/// 不另存一份计数状态（第二份状态迟早与勾选框漂）。
fn count_text(selected: usize, total: usize) -> String {
    if selected == 0 {
        format!("共 {total} 种后缀")
    } else {
        format!("已选中 {selected}/{total}")
    }
}

/// 窗体内容：滚动页（搜索 + 4 列网格，设置页同款骨架）+ 固定底栏。
/// 每帧重建：双态按钮文案、勾选状态全部即时读信号；**网格的过滤走派生信号**。
fn build(
    exts: &[String],
    sigs: &[Signal<bool>],
    search: &Signal<String>,
    view: Signal<Vec<Vec<usize>>>,
) -> Element {
    let mut inner = Element::col().fill().spacing(0);
    // 搜索行（在滚动页里，随页滚走——设置页的卡片也住同一个滚动区）。
    inner = inner.child(
        Element::row().height(ROW_H).child(
            Element::col()
                .fill()
                .child(super::settings::search_box(*search, "搜索后缀…")),
        ),
    );
    // 网格：绑派生视图而不是一次性算好的可见列表——搜索词变即当帧重建。
    inner = inner.child(Element::host_signal(view, {
        let exts = exts.to_vec();
        let sigs = sigs.to_vec();
        move |chunk: Vec<usize>| {
            let mut row = Element::row().height(GRID_ROW_H).spacing(0);
            for i in chunk {
                let cell = Element::row()
                    .width(CELL_W)
                    .height(GRID_ROW_H)
                    .spacing(6)
                    .cross(Align::Center)
                    .child(Element::checkbox("", sigs[i]))
                    .child(Element::label(exts[i].clone()));
                row = row.child(cell);
            }
            row
        }
    }));
    // 底部留白：滚到最底时最后一行不贴着窗底/底栏。
    inner = inner.child(Element::row().height(GAP));

    // 双态按钮文案按当前全量勾选状态即时算（150 次布尔读，每帧可忽略）。
    let all_selected = !sigs.is_empty() && sigs.iter().all(|s| s.get());
    let select_all_label = if all_selected {
        "取消全选"
    } else {
        "全选"
    };

    let sigs_all = sigs.to_vec();
    let select_all = dialog_footer::action(select_all_label, move |_ctx| {
        // 有一个没勾就算"没全选"⇒ 全选；否则全不勾。
        let target = sigs_all.iter().any(|s| !s.get());
        for s in &sigs_all {
            s.set(target);
        }
    });

    let sigs_apply = sigs.to_vec();
    let exts_apply = exts.to_vec();
    let apply = dialog_footer::action("应用", move |ctx| {
        let checked: Vec<String> = exts_apply
            .iter()
            .zip(sigs_apply.iter())
            .filter(|(_, s)| s.get())
            .map(|(e, _)| e.clone())
            .collect();
        if checked.is_empty() {
            return; // 什么都没勾：不写、不弹窗（没有可报告的东西）。
        }
        let out = association::apply(&checked);
        // 能力登记：让这些后缀在系统「设置 → 默认应用」里能看到 RuPix（只登记、不抢默认）。
        association::register_capabilities(&checked);
        // 先弹结果窗再复位：复位只影响本窗，结果窗是一次性文本。
        text_window::show(ctx, "file-assoc-result", "文件关联结果", &out.report());
        for s in &sigs_apply {
            s.set(false);
        }
    });

    // 底栏：三组按钮靠一颗弹性空白右对齐；**左右 16 挂在自己身上**（根列不给
    // 水平留白，否则滚动节点被推进 16、垂直滚动条离右缘一整格）。
    let bar = Element::row()
        .height(ROW_H)
        .cross(Align::Center)
        .padding_xy(DIALOG_PAD, 0)
        .spacing(GAP)
        .child(dialog_footer::action("系统默认应用设置", |_ctx| {
            // 失败路径基本不可达（ShellExecute 拉起系统页），记日志即可。
            if let Err(e) = crate::platform::shell::open_target("ms-settings:defaultapps") {
                log::warn!("[assoc] 打开系统默认应用设置失败：{e}");
            }
        }))
        .child(Element::flex_spacer())
        // 计数（m288，用户口径）：全选左侧、**红字**——没勾读「共 N 种后缀」，
        // 勾了就报「已选中 x/N」。红字用浅色系的 `TEXT_ACCENT`（与解码器页「已安装 N」
        // 的计数同色，色值只在 tokens 一处）；每帧现算，不存第二份状态。
        .child(
            Element::label(count_text(
                sigs.iter().filter(|s| s.get()).count(),
                sigs.len(),
            ))
            .fg(to_color(TEXT_ACCENT)),
        )
        .child(select_all)
        .child(apply);

    // 根列：只给**下缘 16**（底栏与 dialog_footer::shell 同线）、间距 8、水平 0。
    // ★ 与设置页同款：滚动节点自己 `.padding_xy(16, 8)`——windui 的垂直滚动条画在
    //   滚动节点的 abs.right() 上，padding 只收内容、不收节点边（2026-10-17 全 UI
    //   统一，settings/decoders.rs:32 有原文）。上一版把 16 挪到根列 = 滚动条内缩 16，
    //   这正是用户实测的「距离右侧间隔太大」；此版钉在布局用例里。
    Element::col()
        .fill()
        .padding_edges(Insets {
            left: 0,
            top: 0,
            right: 0,
            bottom: DIALOG_PAD,
        })
        .spacing(GAP)
        .child(
            Element::scroll()
                .fill()
                .weight(1.0)
                .padding_xy(DIALOG_PAD, PAGE_TOP_MARGIN)
                .child(inner),
        )
        .child(bar)
}

#[cfg(test)]
mod tests {
    use windui::geometry::Rect;

    use super::*;

    /// 过滤取数口径：去点、不区分大小写、空词 = 全显；命中下标按 4 个一组切行。
    #[test]
    fn search_filter_is_dot_tolerant_and_chunks_by_four() {
        let exts: Vec<String> = vec![
            "jpg".into(),
            "png".into(),
            "jp2".into(),
            "svg".into(),
            "webp".into(),
        ];
        let flat = |q: &str| -> Vec<usize> { filtered_rows(&exts, q).concat() };
        assert_eq!(flat(""), vec![0, 1, 2, 3, 4]);
        assert_eq!(flat("JP"), vec![0, 2]); // contains 不是前缀匹配
        assert_eq!(flat(".png"), vec![1]);
        assert!(flat("xyz").is_empty());
        assert_eq!(filtered_rows(&exts, "").len(), 2); // 5 项 → 2 行（4+1）
    }

    /// m288：底栏计数文案，两态四例。两个数都指**全集**，与搜索框无关。
    #[test]
    fn count_text_reports_total_or_selected() {
        assert_eq!(count_text(0, 150), "共 150 种后缀");
        assert_eq!(count_text(3, 150), "已选中 3/150");
        // 全选也要报 x/y（读数一致），不要退化回"共 N"。
        assert_eq!(count_text(150, 150), "已选中 150/150");
        assert_eq!(count_text(1, 1), "已选中 1/1");
        // 红字：计数那一格必须走 tokens 的强调色，UI 里不许出现字面色值。
        assert!(
            include_str!("association.rs").contains(".fg(to_color(TEXT_ACCENT))"),
            "计数没上红字（或改用字面色值了）"
        );
    }

    /// ★ 两件事一次量死：① 底栏与所有对话框同线（右缘 −16、下缘 −16、行高 40、
    /// 间距 8）；② **滚动节点右缘 = 窗右缘**（垂直滚动条贴右，与设置页一致；
    /// 2026-10-17 全 UI 统一，用户 18:34 实测「距离右侧间隔太大」的那颗钉子）。
    #[test]
    fn footer_and_scrollbar_match_the_settings_page() {
        use windui::core::Tree;
        use windui::geometry::Size;
        use windui::text::NullTextEngine;

        let bar = Element::row()
            .height(ROW_H)
            .cross(Align::Center)
            .padding_xy(DIALOG_PAD, 0)
            .spacing(GAP)
            .child(dialog_footer::action("系统默认应用设置", |_| {}))
            .child(Element::flex_spacer())
            // m288：与生产底栏同构——计数红字在全选左侧（缺了它这条布局钉子就不再描述现状）。
            .child(Element::label("共 150 种后缀").fg(to_color(TEXT_ACCENT)))
            .child(dialog_footer::action("全选", |_| {}))
            .child(dialog_footer::action("应用", |_| {}));
        let root = Element::col()
            .fill()
            .padding_edges(Insets {
                left: 0,
                top: 0,
                right: 0,
                bottom: DIALOG_PAD,
            })
            .spacing(GAP)
            .child(
                Element::scroll()
                    .fill()
                    .weight(1.0)
                    .padding_xy(DIALOG_PAD, PAGE_TOP_MARGIN)
                    .child(Element::row()),
            )
            .child(bar);

        let (w, h) = (620, 520);
        let mut tree = Tree::new();
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(w, h), &mut NullTextEngine);
        let body = tree.get(id).expect("root");
        let scroll_id = body.children[0];
        let bar_id = *body.children.last().expect("footer bar");
        let scroll_rect: Rect = tree.abs_bounds(scroll_id);
        let bar_rect: Rect = tree.abs_bounds(bar_id);
        let rects: Vec<Rect> = tree
            .get(bar_id)
            .expect("bar")
            .children
            .iter()
            .map(|c| tree.abs_bounds(*c))
            .collect();

        // ① 滚动节点横跨整窗：右缘即窗右缘 ⇒ 滚动条贴右（16 只收内容，不收节点边）。
        assert_eq!(scroll_rect.x, 0, "滚动节点左缘 = 0");
        assert_eq!(
            scroll_rect.x + scroll_rect.w,
            w,
            "滚动节点右缘 = 窗右缘（滚动条不内缩）"
        );
        // ② 底栏同线：行本身横跨整窗（水平 16 是它的 padding，不收节点边），
        //    所以量的是左右两颗按钮的落点，不是行的 x。
        assert_eq!(rects[0].x, DIALOG_PAD, "最左按钮左缘 = 16");
        assert_eq!(bar_rect.h, ROW_H, "底栏行高 = 40");
        assert_eq!(
            bar_rect.y + bar_rect.h,
            h - DIALOG_PAD,
            "底栏下缘 = 窗底 − 16"
        );
        let last = rects.last().expect("last button");
        assert_eq!(last.x + last.w, w - DIALOG_PAD, "最右按钮右缘 = 窗右 − 16");
        // rects[0]=左按钮、[1]=弹性空白、[2..]=右侧两颗。
        for pair in rects[2..].windows(2) {
            assert_eq!(pair[1].x - (pair[0].x + pair[0].w), GAP, "相邻按钮间距 = 8");
        }
    }
}
