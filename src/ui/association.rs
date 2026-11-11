//! 文件关联窗（m244 建窗；m314 勾选态持久；m315 自愈改 opt-in）：全后缀勾选网格。
//!
//! # 勾选 = 记住想要什么，不等于允许自动维护
//!
//! 勾中的后缀记进 `Config::assoc_exts`（ini 键 `assoc_exts`），**开窗时按它预勾选**、
//! 点「应用」后**不复位**——勾选态因此是真的持久，不是一次性命令。
//!
//! 但"持续维护"是**另一件事**，由底栏那颗 `assoc_heal`（ini 同名键，**默认关**）单独
//! 决定：开着才有 `runtime::assoc` 的定时核对（启动满 60 秒首查、之后每 60 分钟），
//! 被别的软件或系统改掉就重写回来；关着就只在点「应用」时写一次。
//!
//! ★ 为什么不能"勾了就自动维护"（用户 2026-11-11 拍板）：自愈分不清"别的程序抢走了
//! 默认"和"用户自己在系统设置里改成了别的程序"——两种都是系统写的合法哈希。所以
//! 抢回这个行为必须由用户显式勾选授权，默认不给。
//!
//! 取消勾选 = **退订**，不撤销已写的关联：撤销要知道还原成哪个程序，我们不知道，
//! 删掉只会把用户推到"双击弹选择器"的更差状态（见 `platform::association` 模块头）。
//!
//! # 版式（2026-11-07 用户两轮实测返修后定稿；m314/m315 两次改底栏）
//!
//! 复用设置页的页面骨架 + `dialog_footer` 的 **16 / 8 / 40** 三个数字：
//! 根列只给 `bottom = DIALOG_PAD` 与 `spacing(GAP)`、**水平留白为 0**；
//! 滚动区 = [`Element::scroll`].fill().weight(1.0).padding_xy(16, 8)——windui 的
//! 垂直滚动条画在滚动节点 `abs.right()` 上，padding 只收内容不收节点边，滚动条因此
//! 与设置页一样贴右缘；底栏自带水平 16、行高 `ROW_H`、按钮间距 `GAP`。按钮一律走
//! [`dialog_footer::action`]（hygiene 第 14 查的门）。
//!
//! 底栏最左那颗「系统默认应用设置」（跳 `ms-settings:defaultapps`）已删——能直写
//! UserChoice 之后它不再必要；位置给了 **`assoc_heal` 勾选框**（`自动维护`）。
//! 原来那句「灰色 = 系统保护，仅能加入候选」挪进搜索框占位符——它解释的是网格里的
//! 灰字，贴着搜索行比挤在底栏更近。
//!
//! 三版废案：① scroll 嵌 `shell_split` + 搜索框抢 fill ⇒ 滚动区无界、底栏被顶出窗；
//! ② 根列四边 16 ⇒ 滚动节点内缩 16、滚动条离右缘一整格（用户 18:34 实测）；
//! ③ 底栏自己不垫 16 ⇒ 按钮与其它对话框不同线。布局钉子见 tests。
//!
//! # 搜索（2026-11-07 用户实测返修）
//!
//! 框架 `Signal` 只有「读时重算」，`build()` 里直接 `search.get()` 只在开窗那一帧求值，
//! 打字不会重建树——首版因此输入不过滤。与解码器页同款：**`search.map()` 派生过滤视图
//! （`Vec<Vec<usize>>`，每行 4 个下标）+ `host_signal` 当帧重建网格**，无定时器、无差分。

use std::cell::RefCell;
use std::rc::Rc;

use windui::core::EventCtx;
use windui::prelude::*;

use super::dialog_footer;
use super::singleton;
use super::text_window;
use super::tokens::{
    to_color, BG, DIALOG_PAD, GAP, PAGE_TOP_MARGIN, ROW_H, TEXT_ACCENT, TEXT_SECONDARY,
};
use crate::core::config::Config;
use crate::core::formats;
use crate::platform::association;

/// 网格一列的格宽：后缀名 ≤5 字符，130 px 富余；620 窗宽 − 16 左衬 ≈ 4 列整。
const CELL_W: i32 = 130;
/// 网格行高：勾选框比按钮行矮，30 够呼吸。
const GRID_ROW_H: i32 = 30;
/// 列数（4 = 一屏 30 行 × 4 ≈ 120 项，余下靠搜索与滚动）。
const COLS: usize = 4;

/// 打开文件关联窗。入口唯一：选项页「系统」卡的管理按钮。
///
/// `config` 用来读写勾选表（`assoc_exts`）：开窗按它预勾选，应用时写回并落盘。
pub(crate) fn show(ctx: &mut EventCtx, config: Rc<RefCell<Config>>) {
    // 后缀全集 = 解码器自述并集（与解码器页同一来源）；排序保证每次开窗顺序稳定。
    // 注意这里**已经**过滤掉被关闭与已卸载的解码器（`registry::declared_cached`），
    // 所以勾选表里那些暂时不可用的后缀在本窗根本不显示——但它们仍留在 ini 里，
    // 解码器重开就自动回来（见 `Config::assoc_exts` 的第 2 条不变式）。
    let mut exts: Vec<String> = formats::declared_exts()
        .into_iter()
        .filter_map(|e| association::normalize(&e))
        .collect();
    exts.sort();
    // 预勾选：勾过哪些就勾哪些（m314）。这是"勾选态持久"的唯一实现——
    // 不复位、不存第二份，勾选的真身就是 ini 里那串后缀。
    let subs: std::collections::HashSet<String> =
        config.borrow().assoc_exts.iter().cloned().collect();
    let sigs: Vec<Signal<bool>> = exts.iter().map(|e| signal(subs.contains(e))).collect();
    let search: Signal<String> = signal(String::new());
    // 派生过滤视图：搜索词一变，行分组当帧重算 → host_signal 当帧重建网格。
    let view: Signal<Vec<Vec<usize>>> = search.map({
        let exts = exts.clone();
        move |q| filtered_rows(&exts, q)
    });

    // 底栏两个会变的文案（m288 返修）：`Window::content` 是 `FnOnce`，树只建一次，
    // 所以它们**必须**是信号——`TextContent::Bound` 每帧取当前值。
    let count: Signal<String> = signal(count_text(0, exts.len()));
    let all_label: Signal<String> = signal("全选".to_string());
    // 初值：预勾过的窗一开就该报「已选中 x/N」、双态按钮该是「取消全选」。
    // 少了这一句，勾是回来的、字还停在"共 N 种"（m288 那个"字不动"的翻版）。
    refresh_footer(&sigs, exts.len(), &count, &all_label);

    // 自愈总开关（m315）：默认关。初值取 ini，随「应用」一起落盘。
    // 它**不**参与底栏计数（那是后缀勾选的数量），也不预勾——用户不主动勾就永远不跑。
    let heal: Signal<bool> = signal(config.borrow().assoc_heal);

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
            .content(move || {
                build(
                    &exts,
                    &sigs,
                    &search,
                    view,
                    &Footer {
                        count: &count,
                        all_label: &all_label,
                        heal: &heal,
                    },
                    &config,
                )
            }),
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

/// 底栏读数与「全选 / 取消全选」的**唯一刷新点**。
///
/// 勾选变化有三条路：单个框（`on_toggle`）、全选框、应用后复位。三条都必须过这里——
/// 少一处就回到"勾了、字不动"的老毛病（m288 用户实测）。
/// 文案本身仍由 [`count_text`] 生成，判据不在本函数里另立一份。
fn refresh_footer(
    sigs: &[Signal<bool>],
    total: usize,
    count: &Signal<String>,
    all_label: &Signal<String>,
) {
    let n = sigs.iter().filter(|s| s.get()).count();
    count.set(count_text(n, total));
    all_label.set(if total > 0 && n == total {
        "取消全选".to_string()
    } else {
        "全选".to_string()
    });
}

/// 窗体内容：滚动页（搜索 + 4 列网格，设置页同款骨架）+ 固定底栏。
///
/// ★ **这棵树只建一次**：`Window::content` 收的是 `FnOnce`（windui `app/mod.rs`），
///   所以任何"跟着勾选变"的文案都不能在这里现算——现算的结果就是开窗那一刻的快照。
///   要变的东西一律**绑信号**：底栏读数与「全选/取消全选」是 `Signal<String>`
///   （`TextContent::Bound` 每帧取当前值），网格的过滤则是 `host_signal` 的派生视图。
///   首版把计数在 `build()` 里现算成一个静态字符串，于是勾了框红字不动、双态按钮也不翻
///   （m288 用户实测报的正是这个）。
/// 底栏要跟着变的三样东西（m315 收成一个结构，免得 `build` 参数继续膨胀）。
///
/// 全是引用：树只建一次，所以它们只在这里"借一下"，真身都在 `show` 的栈上。
struct Footer<'a> {
    /// 红字计数：`共 N 种后缀` / `已选中 x/N`。
    count: &'a Signal<String>,
    /// 「全选 ↔ 取消全选」按钮的文案。
    all_label: &'a Signal<String>,
    /// 自愈总开关（默认关）。它不参与计数：那是后缀勾选的数量，与授权无关。
    heal: &'a Signal<bool>,
}

fn build(
    exts: &[String],
    sigs: &[Signal<bool>],
    search: &Signal<String>,
    view: Signal<Vec<Vec<usize>>>,
    footer: &Footer,
    config: &Rc<RefCell<Config>>,
) -> Element {
    let (count, all_label, heal) = (footer.count, footer.all_label, footer.heal);
    let mut inner = Element::col().fill().spacing(0);
    // 搜索行（在滚动页里，随页滚走——设置页的卡片也住同一个滚动区）。
    // ★ 占位符兼职图例（m315）：原来底栏那句「灰色 = 系统保护，仅能加入候选」挪到这里，
    //   与网格只隔一行，比挤在底栏更近；底栏空出来的位置给了自愈开关。
    inner = inner.child(
        Element::row()
            .height(ROW_H)
            .child(Element::col().fill().child(super::settings::search_box(
                *search,
                "搜索后缀…（灰色 = 系统保护，仅能加入候选）",
            ))),
    );
    // 网格：绑派生视图而不是一次性算好的可见列表——搜索词变即当帧重建。
    inner = inner.child(Element::host_signal(view, {
        let exts = exts.to_vec();
        let sigs = sigs.to_vec();
        let count = *count;
        let all_label = *all_label;
        let total = sigs.len();
        move |chunk: Vec<usize>| {
            let mut row = Element::row().height(GRID_ROW_H).spacing(0);
            for i in chunk {
                // 受控勾选（m288）：挂 `on_toggle` 就接管了默认翻转 ⇒ 必须自己 `set` 回去，
                // 再刷底栏。不挂的话读数只能等下一次重建，而重建根本不会发生。
                let sig = sigs[i];
                let all = sigs.clone();
                let count = count;
                let all_label = all_label;
                let checkbox = Element::checkbox("", sigs[i]).on_toggle(move |_ctx| {
                    sig.set(!sig.get());
                    refresh_footer(&all, total, &count, &all_label);
                });
                let cell = Element::row()
                    .width(CELL_W)
                    .height(GRID_ROW_H)
                    .spacing(6)
                    .cross(Align::Center)
                    .child(checkbox)
                    // 受 UCPD 保护的后缀（`.pdf` / Office / HTML）上灰：勾了也只能把
                    // RuPix 挂进「打开方式」候选，UserChoice 写不进去。底栏那句说明
                    // 解释的就是这些灰字。
                    .child(if association::is_system_protected(&exts[i]) {
                        Element::label(exts[i].clone()).fg(to_color(TEXT_SECONDARY))
                    } else {
                        Element::label(exts[i].clone())
                    });
                row = row.child(cell);
            }
            row
        }
    }));
    // 底部留白：滚到最底时最后一行不贴着窗底/底栏。
    inner = inner.child(Element::row().height(GAP));

    let sigs_all = sigs.to_vec();
    let count_all = *count;
    let label_all = *all_label;
    let total = sigs.len();
    let select_all = dialog_footer::action(*all_label, move |_ctx| {
        // 有一个没勾就算"没全选"⇒ 全选；否则全不勾。
        let target = sigs_all.iter().any(|s| !s.get());
        for s in &sigs_all {
            s.set(target);
        }
        refresh_footer(&sigs_all, total, &count_all, &label_all);
    });

    let sigs_apply = sigs.to_vec();
    let exts_apply = exts.to_vec();
    let heal_apply = *heal;
    let config_apply = config.clone();
    let apply = dialog_footer::action("应用", move |ctx| {
        let checked: Vec<String> = exts_apply
            .iter()
            .zip(sigs_apply.iter())
            .filter(|(_, s)| s.get())
            .map(|(e, _)| e.clone())
            .collect();
        let heal_now = heal_apply.get();
        // ini 键：assoc_exts —— 勾选了哪些后缀（"我想要什么"）。
        // ini 键：assoc_heal —— 自愈总开关（"要不要一直盯着"）。两个一起落盘：
        // 写注册表可能部分失败，但意图必须如实记下来。
        {
            let mut cfg = config_apply.borrow_mut();
            cfg.assoc_exts = checked.clone();
            cfg.assoc_heal = heal_now;
        }
        crate::core::config::mark_dirty();
        if checked.is_empty() {
            // 全退订也是有效操作：已写的关联**保持原样**（不撤销），但要说清楚——
            // 否则用户看着一屏空框点「应用」，什么反应都没有，会以为按钮坏了。
            // ★ 开关状态也要如实回报：开着但没有可维护的后缀 = 这轮什么都不会发生。
            let heal_note = if heal_now {
                "「自动维护」已开启，但当前没有勾选任何后缀——没有可维护的对象。"
            } else {
                "「自动维护」关闭：只在点「应用」时写一次，之后不再核对。"
            };
            text_window::show(
                ctx,
                "file-assoc-result",
                "文件关联结果",
                &format!(
                    "已取消全部勾选。\n\n\
                     已经写入的关联保持原样——要换成别的程序，请在系统设置里改。\n\n{heal_note}"
                ),
            );
            return;
        }
        let out = association::apply(&checked);
        // 能力登记：让这些后缀在系统「设置 → 默认应用」里能看到 RuPix（只登记、不抢默认）。
        association::register_capabilities(&checked);
        // 回报里带上开关状态：用户得知道自己刚才授权了什么（自愈会抢回默认）。
        let heal_note = if heal_now {
            "\n\n「自动维护」已开启：运行满一分钟后首查，之后每 60 分钟核对一次。\
             被别的程序或系统改掉的关联会被写回 RuPix（也包括你在系统设置里改的那些）。"
        } else {
            "\n\n「自动维护」关闭：本次只写入一次，之后不再核对。\
             想让它一直保持，请勾上底栏的「自动维护」。"
        };
        text_window::show(
            ctx,
            "file-assoc-result",
            "文件关联结果",
            &(out.report() + heal_note),
        );
        // ★ 勾选**不复位**（m314）：勾选态就是"我想要什么"，复位等于用户点完「应用」当场清空。
        //   底栏读数同样不用刷——一个勾都没变。
    });

    // 底栏：三组按钮靠一颗弹性空白右对齐；**左右 16 挂在自己身上**（根列不给
    // 水平留白，否则滚动节点被推进 16、垂直滚动条离右缘一整格）。
    let bar = Element::row()
        .height(ROW_H)
        .cross(Align::Center)
        .padding_xy(DIALOG_PAD, 0)
        .spacing(GAP)
        // ★ m315：自愈总开关，**默认关**。勾后缀 = 点应用时写一次；要不要定时抢回
        //   是另一件事——自愈分不清"别人抢走了默认"和"用户自己改成了别的程序"，
        //   所以授权必须显式。
        .child(Element::checkbox("自动维护（被改掉就自动改回）", *heal))
        .child(Element::flex_spacer())
        // 计数（m288，用户口径）：全选左侧、**红字**——没勾读「共 N 种后缀」，
        // 勾了就报「已选中 x/N」。红字用浅色系的 `TEXT_ACCENT`（与解码器「已安装 N」
        // 的计数同色，色值只在 tokens 一处）。
        // ★ **必须绑信号**：树只建一次（`FnOnce`），现算就是开窗快照——首版正是这么错的。
        .child(Element::label(*count).fg(to_color(TEXT_ACCENT)))
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

    /// m288 返修：底栏读数与双态按钮**必须是活的**。
    ///
    /// 背景：`Window::content` 收 `FnOnce`，树只建一次 ⇒ 任何现算文案都是开窗快照
    /// （首版正是这么错的：勾了框红字不动、「全选 ↔ 取消全选」也永远不翻）。这条用例
    /// 同时钉三件事：① 刷新函数把两条路都写对；② 计数元素绑的是信号不是字符串；
    /// ③ 每个勾选框都挂了 `on_toggle`（不挂就永远收不到变化）。
    #[test]
    fn footer_readout_is_bound_and_refreshed() {
        let sigs: Vec<Signal<bool>> = (0..5).map(|_| signal(false)).collect();
        let count = signal(String::new());
        let all = signal(String::new());

        refresh_footer(&sigs, 5, &count, &all);
        assert_eq!(count.get(), "共 5 种后缀");
        assert_eq!(all.get(), "全选");

        sigs[0].set(true);
        sigs[3].set(true);
        refresh_footer(&sigs, 5, &count, &all);
        assert_eq!(count.get(), "已选中 2/5");
        assert_eq!(all.get(), "全选", "只选了一部分，按钮不能翻成取消全选");

        for s in &sigs {
            s.set(true);
        }
        refresh_footer(&sigs, 5, &count, &all);
        assert_eq!(count.get(), "已选中 5/5");
        assert_eq!(all.get(), "取消全选");

        for s in &sigs {
            s.set(false);
        }
        refresh_footer(&sigs, 5, &count, &all);
        assert_eq!(count.get(), "共 5 种后缀", "复位后回到总数态");
        assert_eq!(all.get(), "全选");

        // 空集（理论上不会出现，但别让 0/0 长出「取消全选」）
        let none: Vec<Signal<bool>> = Vec::new();
        refresh_footer(&none, 0, &count, &all);
        assert_eq!(all.get(), "全选");
    }

    /// m288：接线层面的两条负向判据——写成静态文案就红，去掉 `on_toggle` 也红。
    #[test]
    fn footer_wiring_must_stay_reactive() {
        let src = include_str!("association.rs");
        // ★ 反向判据的针不能直接写在本文件里：`include_str!` 连**这条断言自己**一起读，
        //   字面量写全就永远命中（与 K188「grep 命中登记它缺号那一句」同一个坑）。
        //   `concat!` 在编译期拼，源码里不会出现连着的整串。
        let bad = concat!("Element::label(", "count_text(");
        assert!(
            !src.contains(bad),
            "计数又被现算成静态串了（树只建一次，它是开窗快照）"
        );
        assert!(
            src.contains("Element::label(*count)"),
            "计数元素没有绑到 `Signal<String>` 上"
        );
        assert!(
            src.contains("dialog_footer::action(*all_label"),
            "「全选」按钮的文字没绑信号 ⇒ 永远停在开窗那一刻"
        );
        assert!(
            src.contains(".on_toggle(move |_ctx|"),
            "勾选框没挂 on_toggle ⇒ 勾了不会刷新底栏"
        );
    }

    /// m314：勾选 = 订阅。三条钉子，改回"一次性命令"就红。
    #[test]
    fn selection_persists_as_a_subscription() {
        let src = include_str!("association.rs");
        // ① 开窗必须按订阅表预勾选（不是清一色 `signal(false)`）。
        assert!(
            src.contains("subs.contains(e)"),
            "开窗没按 assoc_exts 预勾选 ⇒ 勾选态又变回一次性的"
        );
        // ② 应用后**不许**复位。反向针用 `sigs_apply` 认这段循环——直接写
        //   `s.set(false)` 会命中本文件别处的测试代码（自证陷阱，见 K188）。
        let reset = concat!("for s in &sigs_apply", " ");
        assert!(!src.contains(reset), "应用后又把勾选清空了 ⇒ 等于当场退订");
        // ③ 订阅表必须落 ini（写 Config + 标脏），否则下一轮自愈没有依据。
        assert!(
            src.contains("assoc_exts = checked.clone()"),
            "应用没把订阅写进 Config"
        );
        assert!(
            src.contains("crate::core::config::mark_dirty()"),
            "写完订阅没标脏 ⇒ 这条改动永远不落盘"
        );
    }

    /// m315：自愈必须 **opt-in**。写成"勾了后缀就自动维护"这条就红。
    ///
    /// 三件事一次钉死：① 开关默认关（`Config` 的默认构造）；② 开关随「应用」落 ini；
    /// ③ 调度端真的看这个开关。任一处漏掉，用户就会被"没授权也动注册表"。
    #[test]
    fn self_heal_must_be_opt_in() {
        // 自证陷阱（K188）：本文件里的字面量会被 `include_str!` 自己读到，
        // 所以这三条针一律用 `concat!` 在编译期拼开——源码里不出现连着的整串。
        let src = include_str!("association.rs");
        assert!(
            src.contains(concat!("signal(config.borrow()", ".assoc_heal)")),
            "自愈开关初值没跟着 ini 走（写死 true 就等于默认开启）"
        );
        assert!(
            src.contains(concat!("cfg.assoc_heal", " = heal_now")),
            "应用没把自愈开关写进 Config ⇒ 用户勾了也开不起来"
        );

        // 默认值必须关：这是"用户没勾就绝不动注册表"的最后一道保险。
        let cfg = include_str!("../core/config.rs");
        assert!(
            cfg.contains("assoc_heal: false"),
            "assoc_heal 默认值不是 false ⇒ 用户没勾也会跑自愈"
        );
        // 调度端必须读这个键，不能拿到订阅表就直接 kick。
        let rt = include_str!("../runtime/mod.rs");
        assert!(
            rt.contains("config.borrow().assoc_heal"),
            "runtime 没读 assoc_heal ⇒ 关不掉定时核对"
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

        let heal: Signal<bool> = signal(false);
        let bar = Element::row()
            .height(ROW_H)
            .cross(Align::Center)
            .padding_xy(DIALOG_PAD, 0)
            .spacing(GAP)
            // m315：最左是自愈开关（默认关），然后弹性空白、红字计数、全选、应用。
            // 原来那句「灰色 = 系统保护」已挪进搜索框占位符。
            .child(Element::checkbox("自动维护（被改掉就自动改回）", heal))
            .child(Element::flex_spacer())
            // m288：计数红字在全选左侧（缺了它这条布局钉子就不再描述现状）。
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
