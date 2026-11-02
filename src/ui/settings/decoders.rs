//! 「解码器」页：三张可折叠卡片 —— 已安装 / 可下载 / 首选解码器。
//!
//! 2026-10-25 由原「解码器」页与「解码」页合并而来：解码器的**生命周期**（启停 / 卸载 /
//! 详情）与后缀的**归属**（哪个解码器接哪个后缀）是一枚硬币的两面，分成两页只会让人
//! 来回切。合并后一页三段，各段能各自收起，三张**默认都展开**（2026-10-26：可下载卡
//! 接上远程清单后有了内容，再默认收着就等于把功能藏起来）。
//!
//! ★ 页面本体是**外层 scroll + 一张内容列**：卡片按内容长短堆叠，整页一起滚。
//! 卡片里的列表因此走 `host_signal`（普通列容器，按内容定高）而**不是**
//! `list_signal`（它自带滚动容器）：页里再嵌滚动容器会出现"两条滚动条各滚一半"，
//! 而且滚动容器按无限高度测量子元素，里面带 `weight` 的列表会塌成 0 高
//! （2026-10-12 在解码器页踩过，见 `decoders_tab` 的用例注释）。
use std::cell::RefCell;
use std::rc::Rc;

use windui::prelude::*;

use super::*;
use crate::core::config::Config;
// ★ 写全 `crate::decoders::`：本文件自己就叫 `settings::decoders`，裸写会撞名
//   （见 `settings/mod.rs` 的「命名碰撞」注释）。装没装 / 该不该升级的判定都在宿主层，
//   这里只是照着它画按钮。
use crate::decoders::{InstallState, SourceId};

/// 解码器页：三张卡（已安装 / 可下载 / 首选解码器）。
///
/// ★ 2026-10-29 用户口径：「更新地址」卡撤掉——那几个地址只记在 `rupix.ini` 里，
///   给高级用户手改；界面不出现。
pub fn decoders_page(h: &Handles, config: Rc<RefCell<Config>>) -> Element {
    Element::scroll()
        .fill()
        // ★ 左右留白挂滚动节点自己身上：windui 的垂直滚动条画在滚动节点的
        //   `abs.right()` 上，padding 只收内容、不收节点边（2026-10-17 全 UI 统一）。
        .padding_xy(16, PAGE_TOP_MARGIN)
        .child(
            Element::col()
                .fill()
                .spacing(GAP)
                .child(section_card(
                    "已安装",
                    h.card_installed,
                    installed_body(h, config.clone()),
                ))
                .child(section_card(
                    "可下载",
                    h.card_download,
                    download_body(h, config.clone()),
                ))
                .child(section_card(
                    "首选解码器",
                    h.card_preferred,
                    preferred_body(h),
                )),
        )
}

/// 「可下载」卡的正文：搜索行 + 状态行 + 远程清单里的每一枚。
///
/// 数据来自 [`crate::decoders::remote`]：**发布包自带的 `<exe>/docs/decoders.json`**优先、
/// 零网络（`deploy.py` 从解码器仓库 `master` 的 `decoders.json` 复制；镜像 URL 见
/// [`crate::core::config::SourceId::index_url`]）。点「刷新」才后台联网更新它。
/// **刻意不与「已安装」共享数据源**：这张卡回答"还能装什么"，那张卡回答"已经装了什么"，
/// 合成一份就把"装了但仓库下架了"和"仓库有但本机没装"两种情况抹平成一种。
///
/// ★ 版式照「已安装」：搜索行 → 状态行（红字在左、右侧「刷新」）→ 列表 → 空态行。
///   2026-10-26 用户口径：两张卡得长得像同一种东西，只是数据源不同。
///
/// ★ 何时联网：**只有本卡的「刷新」按钮**（后台线程）。进页只读本地、切来源只换地址列，
/// 都不发请求；渲染路径当然更不许发——渲染每帧都会跑，联网就是每帧一次网络往返。
fn download_body(h: &Handles, config: Rc<RefCell<Config>>) -> Element {
    let refresh = {
        let h = *h;
        move |ctx: &mut EventCtx| {
            // 后台刷新：先联网更新 `docs/decoders.json`，结果由设置窗 250 ms 定时器取走。
            match crate::decoders::remote::start_refresh_for(&config.borrow()) {
                Ok(()) => h.set_remote_state("更新中…".to_string()),
                Err(e) => {
                    log::warn!("清单更新未能开始：{e}");
                    h.set_remote_state(e.ui_text().to_string());
                }
            }
            ctx.mark_dirty();
        }
    };
    let rows = Element::host_signal(h.remote_view, {
        let h = *h;
        move |e: RemoteEntry| remote_row(&h, e)
    });

    Element::col()
        .fill()
        .spacing(0)
        // ① 搜索行：与「已安装」「首选解码器」同一只放大镜、同一条 40px 行。
        .child(
            Element::row()
                .fill()
                .height(ROW_H)
                .cross(Align::Center)
                .padding_xy(16, 0)
                .child(
                    search_box(h.remote_filter, "搜索名称或后缀")
                        .width_match()
                        .height(CONTROL_H),
                ),
        )
        .child(Element::leaf().height(GAP))
        // ② 状态行：左红字、右「来源 + 刷新」（与「已安装」的按钮行同一套排法，
        // 2026-10-26 用户口径：来源下拉就在刷新按钮**左侧**）。
        //
        // 切来源只换清单里 `gitee` / `github` 那一列地址，**不联网、不读盘**
        // （见 `Handles::sync`）；「刷新」才按当前来源去拉一份新清单。
        // 文案自己带原因（`读取清单失败：HTTP 404`），不再另配一行说明——那会变成
        // 同一件事说两遍。**红字**用强调色 `TEXT_ACCENT`（与「已安装」的计数同色），
        // 不是危险色：它是这一页的"当前状态"，不是错误专用。
        .child(
            Element::row()
                .fill()
                .spacing(8)
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label_signal(h.remote_status)
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_ACCENT))
                        .weight(1.0)
                        .max_lines(1)
                        .truncate(Truncate::End),
                )
                .child(btn("刷新").on_click(refresh)),
        )
        .child(Element::leaf().height(GAP))
        .child(rows)
        // ③ 搜不到时的空态：只在这一刻出现（不搜时的空白另有含义，别用同一句话糊过去）。
        .child(
            Element::row()
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label("（没有匹配的解码器）")
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_SECONDARY)),
                )
                .visible_when({
                    let h = *h;
                    move || {
                        !h.remote_filter.get().trim().is_empty() && h.remote_view.get().is_empty()
                    }
                }),
        )
}

/// 「可下载」卡的三件信号：搜索词、**派生**过滤视图、**派生**状态行文案。
///
/// 与 `installed_signals` 同款（视图挂在搜索词版本上，打字即时重算，无定时器、无差分）。
/// 状态行也做成派生：搜索时把「匹配 N 个」拼在摘要后面，而那正好是过滤视图的长度。
/// 两份数据拼成一句话，就不会出现"写着 4 个、列表里只有 1 行"那种说不清的偏差。
pub type RemoteSignals = (Signal<String>, Signal<Vec<RemoteEntry>>, Signal<String>);

pub fn remote_signals(all: Signal<Vec<RemoteEntry>>, state: Signal<String>) -> RemoteSignals {
    let filter = signal(String::new());
    let view = filter.map(move |q| apply_remote_filter(&all.get(), q));
    let status = filter.map(move |q| {
        let base = state.get();
        // 只有摘要（"N 个可下载…"，以数字开头）才配接"匹配 N 个"。更新中 / 下载中 /
        // 失败本身就是完整的一句话，再拼半句就成了错话。判据用"是否数字开头"
        // 而不是列举关键词——以后再加状态文案不会静默退化。
        let is_summary = base.chars().next().is_some_and(|c| c.is_ascii_digit());
        if q.trim().is_empty() || !is_summary {
            base
        } else {
            let n = apply_remote_filter(&all.get(), q).len();
            format!("{base} · 匹配 {n} 个")
        }
    });
    (filter, view, status)
}

/// 「可下载」的搜索命中判据（纯函数）：名称 / id / 版本 / 说明 / 认领的后缀。
///
/// ★ **不覆盖**大小与 ABI 号——那是给人核对用的数字，拿它当搜索键只会得到
/// "输入 4 跳出四枚"这种噪音（与「已安装」的 `decoder_matches` 同一口径）。
pub fn remote_matches(e: &RemoteEntry, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    e.name.to_lowercase().contains(&q)
        || e.id.to_lowercase().contains(&q)
        || e.version.to_lowercase().contains(&q)
        || e.desc.to_lowercase().contains(&q)
        || e.formats.iter().any(|x| x.contains(&q))
}

/// 全量 → 过滤视图（[`Handles::remote_view`] 的本体）。
pub fn apply_remote_filter(all: &[RemoteEntry], query: &str) -> Vec<RemoteEntry> {
    all.iter()
        .filter(|e| remote_matches(e, query))
        .cloned()
        .collect()
}

/// 「可下载」卡状态行的那句话（纯函数，好钉：它是**同一份数据的另一种说法**）。
///
/// 只有两形（2026-10-29 用户裁定）：
/// `N 个可下载`（恒出现，`0 个可下载` 也一样——"现在没得装"本身就写在数里）、
/// `N 个可下载 · M 个可升级`（M 为 0 就不出现）。
///
/// ★ 2026-10-29 撤掉「跳过 N 项信息不全」：残缺项仍然不显示，但计数只进日志
///   （用户口径：状态行只要数字，别的都不要）。
/// ★ 单位跟「已安装」统一成**个**（那张卡写 `N 个解码器`）。曾经这里写"枚"，
///   两张卡并排一读就是两套单位。
/// ★ 曾经在可下载与可升级都为 0 时补一句「无需下载」/「暂无已发布版本」——用户口径是
///   **多余**：那行数字已经说明白了，再替读者总结一遍只是噪音。
pub fn remote_summary(
    entries: &[RemoteEntry],
    installed: &[DecoderEntry],
    src: SourceId,
) -> String {
    let (mut ready, mut upgradable) = (0usize, 0usize);
    for e in entries {
        // 只看"当前来源真能下"的那些：某一枚只在 Gitee 发了、用户此刻在 GitHub，
        // 那它就不在"可下载"的计数里（按钮也会是灰的）。
        if !e.downloadable_for(src) || !e.abi_ok() {
            continue;
        }
        match crate::decoders::install_state(e, installed) {
            InstallState::Absent => ready += 1,
            InstallState::Upgradable => upgradable += 1,
            InstallState::Installed | InstallState::Ahead => {}
        }
    }
    let mut state = format!("{ready} 个可下载");
    if upgradable > 0 {
        state.push_str(&format!(" · {upgradable} 个可升级"));
    }
    state
}

/// 灰字单元格（版本号）。统一一个函数，省得几处各写一遍字号与颜色。
///
/// ★ 宽度钉死 + 右对齐：各条目的版本串长短不一（`0.1.0` / `0.10.2`），不钉的话
///   后面那两格会随它左右挪；钉死的是**它自己的宽**，所以状态千变万化也不动。
/// 点「下载」：起后台任务，状态行当场改成「下载中…」。
///
/// ★ 2026-10-29 用户裁定：不带包名、不带「后台进行，装好自动启用」——同一时刻只允许
///   一枚下载，百分比就在那一行的按钮上，状态行只说"在干什么"。
///
/// 装完的收尾不在这里——工作线程没法写配置、碰信号。结果由设置窗的 250 ms 定时器取走，
/// 交给 [`decoder_install_done`](super::decoder_install_done)（它负责**自动启用** + 两处刷新 + toast）。
fn start_install(ctx: &mut EventCtx, h: Handles, e: RemoteEntry) {
    let src = SourceId::from_ui_index(h.download_source.get());
    match crate::decoders::install::start(&e, src) {
        Ok(()) => {
            // 按钮当场变 0%，不用等 250 ms 定时器；后续百分比由 `install::progress` 推。
            h.install_progress.set(Some((e.id.clone(), 0)));
            h.set_remote_state("下载中…".to_string());
            ctx.mark_dirty();
        }
        Err(msg) => ctx.toast_err(msg),
    }
}

/// 行尾那颗按钮的（文案, 色调）——**纯函数**，好把"只有三种状态"这条口径钉成表。
///
/// 2026-10-26 用户口径：只要三种，「下载（蓝，可点）/ 升级（橙，可点）/ 下载（灰，不可点）」。
/// 于是判据塌成两问：**能不能点**（ABI 收得了 + 清单有地址 + 本机那份不是同一份/更新的一份）
/// 与**要不要升级**（本机是旧版）。
///
/// | 情形 | 结果 |
/// |---|---|
/// | 清单有地址、ABI 收得了、本机没有（含"同版本但那文件对不上账"） | 下载 · 蓝 |
/// | 同上，且本机那份是旧版 | 升级 · 橙 |
/// | 已下载 / 本机较新 / 未发布 / 需更高 abi | 下载 · 灰（不可点） |
///
/// ★ 灰色那档**不写原因**：能不能装由按钮动作直接表达，按钮只回答"能不能点"。
///   给每种原因各配一个词（已下载 / 已更新 / 未发布 / 需 abi N）就是在界面上摊内部判据。
fn action_of(e: &RemoteEntry, state: InstallState, src: SourceId) -> (&'static str, ActionTone) {
    // 当前来源有没有这枚的下载地址：只有"这一份认这个来源"才给可点的蓝/橙按钮，
    // 否则灰掉（切到另一个镜像也许就有——详情窗里能看到另一来源的地址）。
    if !e.abi_ok() || !e.downloadable_for(src) {
        return ("下载", ActionTone::Idle);
    }
    match state {
        InstallState::Upgradable => ("升级", ActionTone::Warning),
        InstallState::Absent => ("下载", ActionTone::Primary),
        InstallState::Installed | InstallState::Ahead => ("下载", ActionTone::Idle),
    }
}

/// 行尾按钮在这一刻该显示什么：正在下载的那一枚显示百分比，其余显示静态动作。
///
/// 纯函数，方便钉住"下载中按钮直接变 42%"这条口径；真正的实时刷新由
/// `Signal::map` 绑定 `install_progress` 完成。
fn row_caption(progress: &Option<(String, u8)>, id: &str, label: &str) -> String {
    match progress {
        Some((pid, pct)) if pid == id => format!("{pct}%"),
        _ => label.to_string(),
    }
}

/// 清单里的一行：**名称 + 版本 + 「!」+ 行尾那颗动作按钮**。
///
/// 与「已安装」那张卡的行同一套骨架（名字吃满左边、右侧一串固定信息的格子，最后是动作）；
/// 「!」弹详情窗，字段与已安装那份同源（大小、ABI、格式、说明），只是数据来自清单而不是
/// DLL 自述。
///
/// ★ 2026-10-26 用户口径「名称右边别堆版本/大小/日期，最多留版本号」：大小与发布时间
///   挪进「!」详情窗。理由不只是省地方——那三格把名称挤成一小截，而**同一屏里读起来
///   最重要的信息是"这枚叫什么"与"我能不能装/该不该升"**，大小和日期都是要核对时才看的。
///
/// ★ 行尾**永远是一颗按钮**（[`btn_action`]），只有三种外观（见 [`action_of`]）；
///   没有"换成一句灰字"这条路，因为那会让整列按钮的右缘凹一块。
/// ★ 下载中那一颗按钮临时换成百分比（`Signal<String>` 绑 `install_progress`），
///   仍占同一个动作格；其它行不动。
fn remote_row(h: &Handles, e: RemoteEntry) -> Element {
    let src = SourceId::from_ui_index(h.download_source.get());
    let state = crate::decoders::install_state(&e, &h.decoders.get());
    let (label, tone) = action_of(&e, state, src);
    // 下载中：按钮文案绑 `install_progress`（`42%`），并且自己不能再点；
    // 其余行仍可点（点了会收到"正在下载另一枚"的提示）。
    let caption = h.install_progress.map({
        let id = e.id.clone();
        move |p| row_caption(p, &id, label)
    });
    let not_downloading = {
        let h = *h;
        let id = e.id.clone();
        move || !matches!(h.install_progress.get(), Some((pid, _)) if pid == id)
    };
    let open = {
        let e = e.clone();
        move |ctx: &mut EventCtx| open_remote_info(ctx, &e)
    };
    let install = {
        let e = e.clone();
        let h = *h;
        move |ctx: &mut EventCtx| start_install(ctx, h, e.clone())
    };

    let row = Element::row()
        .fill()
        .cross(Align::Center)
        .spacing(8)
        .height(ROW_H)
        .padding_xy(16, 0)
        .child(
            Element::label(e.name.clone())
                .font_size(FONT_MD)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(version_cell(&e.version))
        // 「!」：详情窗。装完之后想核对说明/格式/大小/日期的人，不该被逼着去「已安装」
        // 那张卡里翻——何况还能装的那几枚根本不在那张卡里。
        .child(btn("!").on_click(open));

    let action = btn_action(caption, tone);
    // 灰的那档不挂回调（按钮自己也拦事件，这里只是别留一个永远走不到的回调）。
    // ★ `enabled_when` 只挂在可点的两档上：灰档已经 `disabled(true)`，再叠一个
    //   动态谓词会把"无事可做"重新算成可点。
    if tone == ActionTone::Idle {
        row.child(action_slot(action))
    } else {
        row.child(action_slot(
            action.enabled_when(not_downloading).on_click(install),
        ))
    }
}

/// 可下载条目的详情窗：点行尾「!」弹出。
///
/// 内容全部来自清单（不是 DLL 自述——这枚可能还没下载）。字段顺序与已安装那份
/// 基本统一；不显示下载地址/安装状态（行上的按钮已经回答“能不能装”）。窗口语义与
/// [`open_decoder_info`] 一致：owned、居中、可缩放、高度按内容量（`fit_content`）、
/// 单例键按 id。
fn open_remote_info(ctx: &mut EventCtx, e: &RemoteEntry) {
    let formats = if e.formats.is_empty() {
        "（无）".to_string()
    } else {
        e.formats.join("; ")
    };
    let released = if e.released.is_empty() {
        "（未发布）".to_string()
    } else {
        e.released.clone()
    };
    let desc = if e.desc.is_empty() {
        "（无）".to_string()
    } else {
        e.desc.clone()
    };
    let author = if e.author.is_empty() {
        "（未知）".to_string()
    } else {
        e.author.clone()
    };
    let license = if e.license.is_empty() {
        "（未知）".to_string()
    } else {
        e.license.clone()
    };
    let key = format!("remote-info-{}", e.id);
    let (id, name, version, abi) = (
        e.id.clone(),
        e.name.clone(),
        e.version.clone(),
        e.abi.to_string(),
    );
    let size = fmt_bytes(e.size);
    let file = e.file_name();

    let content = move || {
        Element::scroll()
            .fill()
            .padding_xy(16, PAGE_TOP_MARGIN)
            .child(
                Element::col()
                    .fill()
                    .spacing(0)
                    .child(info_row("解码器名", name.clone()))
                    .child(info_row("标识", id.clone()))
                    .child(info_row("版本", version.clone()))
                    .child(info_row("ABI 版本", abi.clone()))
                    .child(info_row("作者", author.clone()))
                    .child(info_row("许可证", license.clone()))
                    .child(info_row("发行时间", released.clone()))
                    .child(divider())
                    .child(info_row("说明", desc.clone()))
                    .child(info_row("支持格式", formats.clone()))
                    .child(divider())
                    .child(info_row("大小", size.clone()))
                    .child(info_row("DLL 文件", file.clone())),
            )
    };

    ctx.open_window(
        Window::new("解码器信息", 520, 400)
            .fit_content(260, 1100)
            .resizable(true)
            .centered(true)
            .owned(true)
            .single(&key)
            .content(content),
    );
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// 解码器页回归：**三张卡都在、正文高度不塌、页能真的滚动**。
    ///
    /// 三条判据各自盯一类旧病：
    /// ① 卡片与行的高度都 > 0（防 "scroll 里放 weight 子节点 → 0 高" 复发，
    ///    也防 `host_signal` 在无限高度下量不出内容）；
    /// ② 折叠信号真能收起正文（点了没反应是最容易被漏掉的一类 UI bug）；
    /// ③ 塞够行数强制溢出后**真的派发滚轮**验偏移量（布局对但事件没人处理的老病）。
    #[test]
    fn decoders_page_cards_survive_layout_and_scroll() {
        use windui::core::Tree;
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        use windui::text::NullTextEngine;

        let entry = |id: &str| DecoderEntry {
            id: id.to_string(),
            display_name: format!("{id} 解码器"),
            description: "测试用".to_string(),
            extensions: vec!["qoi".into()],
            file: format!("{id}_0.1.0.dll"),
            path: std::path::PathBuf::from(format!("decoders/{id}_0.1.0.dll")),
            size: 1234,
            mtime: None,
            health: crate::decoders::Health::Ok,
            enabled: true,
            has_raw: false,
            has_vector: false,
            vector_exts: Vec::new(),
            vector_region: false,
            abi: Some(5),
            metadata: Vec::new(),
        };

        let config = Rc::new(RefCell::new(Config::default()));
        let (ext_rows_all, ext_filter, ext_view) = decode_signals();
        let (decoders, decoder_filter, decoder_view, decoder_count) = installed_signals();
        let h = Handles {
            stay: sig(false),
            minimal_mode: sig(false),
            checker: sig(false),
            long_fit: sig(false),
            dcb: signal(0),
            slide_secs: signal("3".into()),
            wheel: signal(0),
            ctrl_wheel: signal(0),
            init_win: signal(0),
            win_size: signal(String::new()),
            prefetch: signal(false),
            to_tray: sig(false),
            space_preview: sig(false),
            opacity: signal(0.75),
            opacity_label: signal(String::new()),
            tab: signal(TAB_DECODERS),
            decoder_count,
            decoder_state: signal(String::new()),
            decoders,
            decoder_view,
            decoder_filter,
            decoders_seen: sig(false),
            remote: signal(Vec::new()),
            remote_state: signal(String::new()),
            install_progress: signal(None),
            remote_filter: signal(String::new()),
            remote_view: signal(Vec::new()),
            remote_status: signal(String::new()),
            ext_rows_all,
            ext_view,
            ext_count: signal(0),
            ext_filter,
            card_installed: sig(true),
            card_download: sig(true),
            card_preferred: sig(true),
            card_keys: sig(true),
            card_mouse: sig(true),
            card_plugins: sig(true),

            download_source: signal(0),
            plugin: PluginUi::new(&Config::default()),
        };

        let page = decoders_page(&h, config);
        let ui = Element::row()
            .fill()
            .child(Element::leaf().width(88).height_match())
            .child(Element::stack().fill().weight(1.0).child(page.fill()));

        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 300);
        tree.layout_root(size, &mut NullTextEngine);

        // 20 枚解码器 + 20 行后缀 → 必定溢出（300px 视口装不下）。
        let many: Vec<DecoderEntry> = (0..20).map(|i| entry(&format!("p{i:02}"))).collect();
        h.decoders.set(many);
        // 行集变了要同值写 bump 搜索词版本，派生视图才重算（与两张卡重刷那处同款）。
        h.decoder_filter.set(h.decoder_filter.get());
        let rows: Vec<ExtRow> = (0..20)
            .map(|i| ExtRow {
                ext: format!("e{i:02}"),
                ids: vec!["x".to_string()],
                labels: vec!["x".to_string()],
                current: 0,
            })
            .collect();
        h.ext_rows_all.set(rows);
        h.ext_filter.set(h.ext_filter.get());
        tree.layout_root(size, &mut NullTextEngine);

        let stack = tree.get(root).expect("row 根").children[1];
        let page_id = tree.get(stack).expect("stack").children[0];
        let kids = &tree.get(page_id).expect("页面 scroll").children;
        let col_id = kids[0];
        let cards = tree.get(col_id).expect("内容列").children.clone();
        assert_eq!(cards.len(), 3, "解码器页应为三张卡（更新地址只在 ini）");

        // ① 三张卡的标题条都在，且都有高度（卡片本体塌 0 = 整页看不见）。
        for (i, &c) in cards.iter().enumerate() {
            let card = tree.get(c).expect("卡片");
            assert!(card.bounds.h > 0, "第 {i} 张卡高度塌成 0");
            let header = card.children[0];
            assert_eq!(
                tree.get(header).expect("标题条").bounds.h,
                CARD_HEADER_H,
                "第 {i} 张卡的标题条高度应为 CARD_HEADER_H"
            );
        }

        // ★ 竖向节奏统一：卡↔卡、卡头↔正文、页四周，全部同一格 GAP。
        // 早期就是这三处各用各的数（页顶 4 / 卡间 8 / 卡头到正文 0），结果「已安装」
        // 那颗「文件夹」按钮贴在色带下缘上——这条断言就是那个观感的回归锚。
        let gap_between_cards = tree.abs_bounds(cards[1]).y - tree.abs_bounds(cards[0]).bottom();
        assert_eq!(
            gap_between_cards, GAP,
            "卡↔卡应为 GAP：实测 {gap_between_cards}"
        );
        // 间距量的是**正文首行**的顶缘，不是正文节点的顶缘：GAP 垫在正文节点自己的
        // padding 上，节点框从色带下缘就开始了，量节点等于量了个 0（本测试第一版就
        // 栽在这里——断言红着说实测 0，其实是量错了对象）。
        let header_bottom = tree
            .abs_bounds(tree.get(cards[0]).expect("卡").children[0])
            .bottom();
        let body_id = tree.get(cards[0]).expect("卡").children[1];
        let first_row = tree.get(body_id).expect("正文").children[0];
        let gap_header_body = tree.abs_bounds(first_row).y - header_bottom;
        assert_eq!(
            gap_header_body, GAP,
            "卡片标题条↔正文首行应为 GAP：实测 {gap_header_body}（0 = 按钮行贴在色带下缘上）"
        );
        // 已安装卡：正文（按钮行 + 20 行解码器）必须真的有高度。
        let installed_body = tree.get(cards[0]).expect("卡").children[1];
        assert!(
            tree.get(installed_body).expect("正文").bounds.h > ROW_H,
            "已安装卡正文高度只剩一行：host_signal 在无限高度下没量出内容"
        );
        // 可下载卡同样默认展开：正文（状态行 + 清单行）至少一整行高。
        let download_body = tree.get(cards[1]).expect("卡").children[1];
        assert!(
            tree.get(download_body).expect("正文").bounds.h >= ROW_H,
            "可下载卡默认展开，正文至少该有一行状态行的高度"
        );

        // ② 折叠是活开关。**量卡片自己**而不是被隐藏的正文：windui 的
        //   `visible_when(false)` 只把节点从 `visible_children` 里摘掉（不占位），
        //   但它自己的 `bounds` 还挂着上一帧的值（见 K141）。
        let expanded_card_h = tree.get(cards[1]).expect("卡").bounds.h;
        h.card_download.set(false);
        tree.layout_root(size, &mut NullTextEngine);
        assert_eq!(
            tree.get(cards[1]).expect("卡").bounds.h,
            CARD_HEADER_H,
            "收起「可下载」后卡片应只剩标题条那么高"
        );
        h.card_download.set(true);
        tree.layout_root(size, &mut NullTextEngine);
        assert_eq!(
            tree.get(cards[1]).expect("卡").bounds.h,
            expanded_card_h,
            "再展开「可下载」应回到原高：折叠是单向的？"
        );

        // ③ 整页滚动：滚轮必须被消费、且 scroll_y 真的动。
        let content_h = tree.get(page_id).expect("页面").content_h;
        assert!(
            content_h > size.h,
            "内容必须溢出视口，否则测不到滚动（content_h={content_h} view={}）",
            size.h
        );
        let abs = tree.abs_bounds(page_id);
        let p = Point::new(abs.x + 20, abs.y + 20);
        let mut hover = None;
        let mut capture = None;
        let ev = PointerEvent::single(PointerKind::Wheel(-120), p, MouseButton::Left);
        let res = tree.dispatch_pointer(ev, &mut hover, &mut capture);
        let after = tree.get(page_id).unwrap().scroll_y;
        assert!(res.consumed, "滚轮事件无人消费：页面滚动条又悬空了？");
        assert!(
            after > 0,
            "滚一格后 scroll_y 应 >0，实为 {after}——「滚动条画得出却滚不动」的旧病复发"
        );
    }

    /// 全页搜索框**同宽且撑满**：两只框必须都是「可用宽」，一宽一窄就是 UI 不统一。
    ///
    /// 这条判据钉的是一个真实的坑（2026-10-26 用户口径「宽度与首选解码器一致」）：
    /// windui 的**水平容器主轴上的 `Match` 会降级成「按内容宽」**
    /// （`measure_linear` 避免单个子独占整条主轴）。所以把 `text_input(...).width_match()`
    /// **直接挂进 `row`**，量到的是占位文字的固有宽——「已安装」实测只有 160px，
    /// 而「首选」那侧输入框外层还有一层 `host_signal` 返回的 `col().fill()`，
    /// 横向是交叉轴、Match 正常撑满 → 488px。修法是把「包一层 col」收进
    /// [`rows::search_box`]，两只框共用，宽度逻辑只有一份。
    #[test]
    fn both_search_boxes_are_the_same_width() {
        use windui::core::Tree;
        use windui::text::NullTextEngine;

        let config = Rc::new(RefCell::new(Config::default()));
        let (ext_rows_all, ext_filter, ext_view) = decode_signals();
        let (decoders, decoder_filter, decoder_view, decoder_count) = installed_signals();
        let h = Handles {
            stay: sig(false),
            minimal_mode: sig(false),
            checker: sig(false),
            long_fit: sig(false),
            dcb: signal(0),
            slide_secs: signal("3".into()),
            wheel: signal(0),
            ctrl_wheel: signal(0),
            init_win: signal(0),
            win_size: signal(String::new()),
            prefetch: sig(false),
            to_tray: sig(false),
            space_preview: sig(false),
            opacity: signal(0.75),
            opacity_label: signal(String::new()),
            tab: signal(TAB_DECODERS),
            decoder_count,
            decoder_state: signal(String::new()),
            decoders,
            decoder_view,
            decoder_filter,
            decoders_seen: sig(false),
            remote: signal(Vec::new()),
            remote_state: signal(String::new()),
            install_progress: signal(None),
            remote_filter: signal(String::new()),
            remote_view: signal(Vec::new()),
            remote_status: signal(String::new()),
            ext_rows_all,
            ext_view,
            ext_count: signal(0),
            ext_filter,
            card_installed: sig(true),
            card_download: sig(true),
            card_preferred: sig(true),
            card_keys: sig(true),
            card_mouse: sig(true),
            card_plugins: sig(true),

            download_source: signal(0),
            plugin: PluginUi::new(&Config::default()),
        };

        let page = decoders_page(&h, config);
        // 与真窗同形：88px 导航条 + stack + 页（卡在页的 scroll 里）。
        let ui = Element::row()
            .fill()
            .child(Element::leaf().width(88).height_match())
            .child(Element::stack().fill().weight(1.0).child(page.fill()));
        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 300);
        tree.layout_root(size, &mut NullTextEngine);

        // 卡0 = 已安装、卡2 = 首选解码器；各自正文的第一个子节点是搜索行。
        let stack = tree.get(root).expect("row 根").children[1];
        let page_id = tree.get(stack).expect("stack").children[0];
        let col_id = tree.get(page_id).expect("scroll").children[0];
        let cards = tree.get(col_id).expect("内容列").children.clone();
        assert_eq!(cards.len(), 3, "解码器页应为三张卡（更新地址只在 ini）");

        /// 搜索行的第一个子节点就是输入框本体（`search_box` 的 col）。
        fn input_of(card: usize, cards: &[windui::core::NodeId], tree: &Tree) -> (i32, i32) {
            let body = tree
                .get(tree.get(cards[card]).expect("卡").children[1])
                .expect("正文")
                .children[0];
            let b = tree.get(body).expect("搜索行").bounds;
            let input = tree
                .get(tree.get(body).expect("搜索行").children[0])
                .expect("输入框")
                .bounds;
            (b.x, input.w)
        }

        let (row_x_a, w_installed) = input_of(0, &cards, &tree);
        let (row_x_b, w_download) = input_of(1, &cards, &tree);
        let (row_x_c, w_preferred) = input_of(2, &cards, &tree);
        assert_eq!(
            row_x_a, row_x_b,
            "「已安装」与「可下载」两个搜索行的左缘必须同一条竖线"
        );
        assert_eq!(row_x_a, row_x_c, "「首选解码器」那一行也要同一条竖线");
        assert_eq!(
            w_installed, w_download,
            "「可下载」的搜索框必须与「已安装」同宽（2026-10-26 三张卡统一版式）"
        );
        assert_eq!(
            w_installed, w_preferred,
            "两只搜索框必须同宽（曾一宽 488 / 一宽 160，见用例注释）"
        );
        // 且必须是「撑满」而不是「恰好相等地窄」：
        // 可用宽 = 窗宽 640 − 导航条 88 − scroll 左右 pad(16×2) − 搜索行左右 pad(16×2)。
        let avail = 640 - 88 - 16 - 16 - 16 - 16;
        assert_eq!(
            w_installed, avail,
            "搜索框应撑满可用宽 {avail}，实为 {w_installed}"
        );
    }

    /// 「可下载」的搜索判据：名称 / id / 版本 / 说明 / 后缀都能搜，**大小与 ABI 不参与**。
    #[test]
    fn remote_search_matches_name_version_desc_and_exts() {
        let e = |id: &str, name: &str, ver: &str, exts: &[&str], desc: &str| RemoteEntry {
            id: id.to_string(),
            name: name.to_string(),
            version: ver.to_string(),
            abi: 5,
            size: 1234,
            released: "2026-10-26".to_string(),
            author: String::new(),
            license: String::new(),
            desc: desc.to_string(),
            formats: exts.iter().map(|s| s.to_string()).collect(),
            urls: {
                let mut m = std::collections::HashMap::new();
                m.insert("gitee".to_string(), "https://example.com/x.dll".to_string());
                m
            },
        };
        let all = vec![
            e(
                "common",
                "Common Raster Set",
                "0.1.0",
                &["jpg", "png"],
                "常见位图合集",
            ),
            e(
                "raw",
                "Camera RAW",
                "0.4.0",
                &["cr2", "nef"],
                "相机原始数据",
            ),
        ];

        assert_eq!(apply_remote_filter(&all, "").len(), 2, "空串 = 全量");
        assert_eq!(apply_remote_filter(&all, "   ").len(), 2);
        // 后缀：这是搜索框存在的主要理由（"谁能解 nef"）。
        let hit = apply_remote_filter(&all, "nef");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].id, "raw");
        assert_eq!(apply_remote_filter(&all, "JPG").len(), 1, "大小写不敏感");
        // 名称 / id / 版本 / 说明。
        assert_eq!(apply_remote_filter(&all, "camera").len(), 1, "名称");
        assert_eq!(apply_remote_filter(&all, "common").len(), 1, "id");
        assert_eq!(apply_remote_filter(&all, "0.4.0").len(), 1, "版本");
        assert_eq!(apply_remote_filter(&all, "位图").len(), 1, "说明");
        // 大小不当搜索键（与「已安装」同一口径）。
        // ABI 同理不参与——这里不再单钉一条，因为版本号里本来就有数字（`0.4.0`），
        // 搜 "4" 必然命中它，"命中 ABI 还是命中版本"根本分不清，正好说明数字不该当判据。
        assert_eq!(apply_remote_filter(&all, "1234").len(), 0, "大小不参与");
    }

    /// 状态行：摘要 + 搜索匹配数；失败/下载中的整句话**不许被拼上"匹配 0 个"**。
    ///
    /// 这条钉的是「展示文案与列表同源」：写着 `匹配 1 个` 时列表里就得恰好一行。
    #[test]
    fn remote_status_appends_match_count_but_keeps_error_text() {
        let all: Signal<Vec<RemoteEntry>> = signal(vec![
            RemoteEntry {
                id: "common".into(),
                name: "Common".into(),
                version: "0.1.0".into(),
                abi: 5,
                size: 1,
                released: String::new(),
                author: String::new(),
                license: String::new(),
                desc: String::new(),
                formats: vec!["jpg".into()],
                urls: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("gitee".to_string(), "u".into());
                    m
                },
            },
            RemoteEntry {
                id: "raw".into(),
                name: "RAW".into(),
                version: "0.4.0".into(),
                abi: 5,
                size: 1,
                released: String::new(),
                author: String::new(),
                license: String::new(),
                desc: String::new(),
                formats: vec!["cr2".into()],
                urls: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("gitee".to_string(), String::new());
                    m
                },
            },
        ]);
        let state: Signal<String> = signal("1 个可下载".to_string());
        let (filter, view, status) = remote_signals(all, state);
        assert_eq!(view.get().len(), 2);
        assert_eq!(status.get(), "1 个可下载", "不搜时原样显示摘要");

        filter.set("cr2".to_string());
        assert_eq!(view.get().len(), 1, "搜 cr2 命中 RAW 那一枚");
        assert_eq!(status.get(), "1 个可下载 · 匹配 1 个");

        // 失败文案是完整一句话：拼上"匹配 N 个"就变成了半句错话。
        state.set("更新失败".to_string());
        filter.set(filter.get()); // 同值写 bump（`Handles::set_remote_state` 的同一套做法）
        assert_eq!(status.get(), "更新失败");
    }

    /// 状态行的两本账：**可下载 / 可升级**各自独立，且与列表同源。
    ///
    /// 钉的是 2026-10-26 那条新口径：清单版本更高 = 「可升级」，它不能被算成"可下载"
    /// （本机已经有了），更不能被算成"已安装"（那一枚点一下就能变新）。
    /// 2026-10-29 起「跳过 N 项」不再上状态行（只进日志），本测同时钉住这一点。
    #[test]
    fn remote_summary_separates_installable_from_upgradable() {
        let e = |id: &str, ver: &str, url: &str, abi: u32, size: u64| RemoteEntry {
            id: id.to_string(),
            name: id.to_string(),
            version: ver.to_string(),
            abi,
            size,
            released: String::new(),
            author: String::new(),
            license: String::new(),
            desc: String::new(),
            formats: vec!["qoi".into()],
            urls: {
                let mut m = std::collections::HashMap::new();
                m.insert("gitee".to_string(), url.to_string());
                m
            },
        };
        let local = |id: &str, ver: &str, size: u64| DecoderEntry {
            id: id.to_string(),
            display_name: id.to_string(),
            description: String::new(),
            extensions: vec!["qoi".into()],
            file: format!("{id}_{ver}.dll"),
            path: std::path::PathBuf::from(format!("decoders/{id}_{ver}.dll")),
            size,
            mtime: None,
            health: crate::decoders::Health::Ok,
            enabled: true,
            has_raw: false,
            has_vector: false,
            vector_exts: Vec::new(),
            vector_region: false,
            abi: Some(5),
            metadata: Vec::new(),
        };

        // 一枚新装、一枚该升级、一枚已是最新、一枚未发布、一枚 ABI 收不了。
        let entries = vec![
            e("fresh", "0.1.0", "u", 5, 10),
            e("up", "0.2.0", "u", 5, 10),
            e("done", "0.1.0", "u", 5, 10),
            e("draft", "0.1.0", "", 5, 10),
            e("future", "0.1.0", "u", 99, 10),
        ];
        let installed = vec![
            local("up", "0.1.0", 10),
            local("done", "0.1.0", 10),
            local("future", "0.1.0", 10),
        ];
        assert_eq!(
            remote_summary(&entries, &installed, SourceId::Gitee),
            "1 个可下载 · 1 个可升级"
        );

        // 全都装好了 → 只有数字，不替读者总结（用户口径：「无需下载」这种判词是多余的）。
        let all_done = vec![e("done", "0.1.0", "u", 5, 10)];
        assert_eq!(
            remote_summary(&all_done, &[local("done", "0.1.0", 10)], SourceId::Gitee),
            "0 个可下载"
        );
        // 清单里一枚能装的都没有（全未发布）→ 同样是 0，没有第二种说法。
        let draft = vec![e("draft", "0.1.0", "", 5, 10)];
        assert_eq!(remote_summary(&draft, &[], SourceId::Gitee), "0 个可下载");
        // 清单里一枚都没有时也一样。
        assert_eq!(remote_summary(&[], &[], SourceId::Gitee), "0 个可下载");
    }

    /// 下载中只有**那一枚**的按钮变成百分比；其它行保持静态文案。
    #[test]
    fn row_caption_shows_percent_only_for_the_downloading_entry() {
        assert_eq!(row_caption(&None, "common", "下载"), "下载");
        let p = Some(("common".to_string(), 42));
        assert_eq!(row_caption(&p, "common", "下载"), "42%");
        assert_eq!(row_caption(&p, "raw", "下载"), "下载", "别的行不跟着变");
        let done = Some(("common".to_string(), 100));
        assert_eq!(row_caption(&done, "common", "下载"), "100%");
    }

    /// 动作格的**全部状态**：只有两种文案、三种外观（2026-10-26 用户口径）。
    ///
    /// 这条是"别再把内部判据摊到按钮上"的锚：谁哪天又想加「已下载 / 未发布 / 需 abi N」
    /// 之类的文案，先来这里看列——加词就得先改这张表，改表就得先回答"为什么灰「下载」
    /// 不够用"。
    #[test]
    fn action_slot_has_exactly_three_looks() {
        let e = |url: &str, abi: u32| RemoteEntry {
            id: "x".into(),
            name: "X".into(),
            version: "0.2.0".into(),
            abi,
            size: 10,
            released: String::new(),
            author: String::new(),
            license: String::new(),
            desc: String::new(),
            formats: vec!["qoi".into()],
            urls: {
                let mut m = std::collections::HashMap::new();
                m.insert("gitee".to_string(), url.to_string());
                m
            },
        };

        // 能装的那一档：本机没有这一枚（点得动）。
        let ready = e("u", 5);
        assert_eq!(
            action_of(&ready, InstallState::Absent, SourceId::Gitee),
            ("下载", ActionTone::Primary)
        );
        // 本机旧版：橙「升级」。
        assert_eq!(
            action_of(&ready, InstallState::Upgradable, SourceId::Gitee),
            ("升级", ActionTone::Warning)
        );
        // 其余一律灰「下载」：已下载 / 本机较新 / 未发布 / abi 收不了。
        for state in [InstallState::Installed, InstallState::Ahead] {
            assert_eq!(
                action_of(&ready, state, SourceId::Gitee),
                ("下载", ActionTone::Idle),
                "{state:?}"
            );
        }
        let draft = e("", 5);
        assert_eq!(
            action_of(&draft, InstallState::Absent, SourceId::Gitee),
            ("下载", ActionTone::Idle),
            "未发布 = 灰"
        );
        let future = e("u", 99);
        assert_eq!(
            action_of(&future, InstallState::Absent, SourceId::Gitee),
            ("下载", ActionTone::Idle),
            "abi 收不了 = 灰，且不再写「需 abi N」"
        );
        // 文案只有两种：没有「已下载 / 已更新 / 未发布 / 重新下载」这些词。
        let words: Vec<&str> = [
            InstallState::Absent,
            InstallState::Upgradable,
            InstallState::Installed,
            InstallState::Ahead,
        ]
        .iter()
        .map(|s| action_of(&ready, *s, SourceId::Gitee).0)
        .collect();
        assert!(
            words.iter().all(|w| *w == "下载" || *w == "升级"),
            "动作格只许两种文案，实际 {words:?}"
        );
    }

    /// 「可下载」的行版式回归：**列表必须真的建出行、每行满高、骨架四段**
    /// （名称 + 版本 + 「!」+ 动作），且**各种状态的动作用同一尺寸的格子**。
    ///
    /// 与「已安装」那条同源 —— 卡里嵌的是 `host_signal`（普通列容器），
    /// 它一旦在无限高度下量不出内容，症状就是"清单读到了、页面上什么都没有"。
    /// 尺寸那条是 2026-10-26 用户口径的回归锚：非动作态**不许**退回灰字。
    #[test]
    fn download_rows_keep_one_action_slot_per_state() {
        use windui::core::Tree;
        use windui::text::NullTextEngine;

        let config = Rc::new(RefCell::new(Config::default()));
        let (ext_rows_all, ext_filter, ext_view) = decode_signals();
        let (decoders, decoder_filter, decoder_view, decoder_count) = installed_signals();
        let h = Handles {
            stay: sig(false),
            minimal_mode: sig(false),
            checker: sig(false),
            long_fit: sig(false),
            dcb: signal(0),
            slide_secs: signal("3".into()),
            wheel: signal(0),
            ctrl_wheel: signal(0),
            init_win: signal(0),
            win_size: signal(String::new()),
            prefetch: signal(false),
            to_tray: sig(false),
            space_preview: sig(false),
            opacity: signal(0.75),
            opacity_label: signal(String::new()),
            tab: signal(TAB_DECODERS),
            decoder_count,
            decoder_state: signal(String::new()),
            decoders,
            decoder_view,
            decoder_filter,
            decoders_seen: sig(false),
            remote: signal(Vec::new()),
            remote_state: signal(String::new()),
            install_progress: signal(None),
            remote_filter: signal(String::new()),
            remote_view: signal(Vec::new()),
            remote_status: signal(String::new()),
            ext_rows_all,
            ext_view,
            ext_count: signal(0),
            ext_filter,
            card_installed: sig(true),
            card_download: sig(true),
            card_preferred: sig(true),
            card_keys: sig(true),
            card_mouse: sig(true),
            card_plugins: sig(true),

            download_source: signal(0),
            plugin: PluginUi::new(&Config::default()),
        };

        let page = decoders_page(&h, config);
        let ui = Element::row()
            .fill()
            .child(Element::leaf().width(88).height_match())
            .child(Element::stack().fill().weight(1.0).child(page.fill()));
        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 300);
        tree.layout_root(size, &mut NullTextEngine);

        // ★ 六行覆盖六条路（2026-10-26 用户口径：行尾永远是一颗**同尺寸**的按钮）：
        //   已下载 / 可升级 / 本机更新 / 新装 / 未发布 / ABI 收不了。
        let remote = |id: &str, ver: &str, url: &str, abi: u32, size: u64| RemoteEntry {
            id: id.to_string(),
            name: format!("{id} 解码器"),
            version: ver.to_string(),
            abi,
            size,
            released: "2026-10-26".to_string(),
            author: String::new(),
            license: String::new(),
            desc: "测试用".to_string(),
            formats: vec!["qoi".into()],
            urls: {
                let mut m = std::collections::HashMap::new();
                m.insert("gitee".to_string(), url.to_string());
                m
            },
        };
        let u = |id: &str, ver: &str| format!("https://example.com/{id}_{ver}.dll");
        h.remote_view.set(vec![
            remote("common", "0.1.0", &u("common", "0.1.0"), 5, 1000),
            remote("upgradable", "0.2.0", &u("upgradable", "0.2.0"), 5, 1000),
            remote("ahead", "0.1.0", &u("ahead", "0.1.0"), 5, 1000),
            remote("fresh", "0.9.0", &u("fresh", "0.9.0"), 5, 1000),
            remote("bad", "0.5.0", &u("bad", "0.5.0"), 5, 1000),
            remote("unpub", "0.3.0", "", 5, 1000),
            remote("future", "0.1.0", &u("future", "0.1.0"), 99, 1000),
        ]);
        // 本机已装的那四枚：同版本同字节 / 旧版 / 新版 / 同版不同字节。
        let local = |id: &str, file: &str, size: u64| DecoderEntry {
            id: id.to_string(),
            display_name: id.to_string(),
            description: String::new(),
            extensions: vec!["qoi".into()],
            file: file.to_string(),
            path: std::path::PathBuf::from(format!("decoders/{file}")),
            size,
            mtime: None,
            health: crate::decoders::Health::Ok,
            enabled: true,
            has_raw: false,
            has_vector: false,
            vector_exts: Vec::new(),
            vector_region: false,
            abi: Some(5),
            metadata: Vec::new(),
        };
        h.decoders.set(vec![
            local("common", "common_0.1.0.dll", 1000),
            local("upgradable", "upgradable_0.1.0.dll", 1000),
            local("ahead", "ahead_0.2.0.dll", 1000),
            local("bad", "bad_0.5.0.dll", 999),
        ]);
        tree.layout_root(size, &mut NullTextEngine);

        let stack = tree.get(root).expect("row 根").children[1];
        let page_id = tree.get(stack).expect("stack").children[0];
        let col_id = tree.get(page_id).expect("scroll").children[0];
        let cards = tree.get(col_id).expect("内容列").children.clone();
        let body_id = tree.get(cards[1]).expect("可下载卡").children[1];
        let body = tree.get(body_id).expect("正文");
        assert_eq!(
            body.children.len(),
            6,
            "「可下载」正文应为「搜索行 + GAP + 状态行 + GAP + 列表 + 空态行」六段"
        );
        // ★ 2026-10-30：来源下拉**撤了**——「下载源」统一到 选项 页一处（两张卡共用）。
        //   状态行因此只剩「状态文本 + 刷新」两格；这条用例同时守着"别再长回第三格"。
        let status = tree.get(body.children[2]).expect("状态行");
        assert_eq!(
            status.children.len(),
            2,
            "状态行 = 状态文本 + 刷新（来源已统一到选项页）"
        );
        let refresh = tree.abs_bounds(status.children[1]);
        assert!(refresh.w > 0, "刷新按钮宽度塌成 0");
        let rows_id = body.children[4];
        let rows = tree.get(rows_id).expect("行容器");
        assert_eq!(rows.children.len(), 7, "七枚清单条目应建出七行");
        assert!(
            rows.bounds.h > 0,
            "行容器高度塌成 0（host_signal 没量出内容）"
        );

        // 一行 = 名称 + 版本 + 「!」+ 动作（2026-10-26 起大小/日期挪进详情窗）。
        for (i, &c) in rows.children.iter().enumerate() {
            let row = tree.get(c).expect("行");
            assert_eq!(row.bounds.h, ROW_H, "第 {i} 行高度应为 ROW_H");
            assert_eq!(
                row.children.len(),
                4,
                "第 {i} 行应为「名称 + 版本 + ! + 动作」四格"
            );
            let ver = tree.get(row.children[1]).expect("版本格").bounds;
            assert_eq!(ver.w, VERSION_COL_W, "第 {i} 行的版本格应是钉死的宽");
        }

        // ★ 本用例的核心判据：**各种状态，动作那一格一模一样**。
        //   过去「已下载 / 未发布」渲染成一行灰字，宽度只有按钮的一半，整列右缘于是
        //   参差不齐（2026-10-26 用户口径「完全破坏了布局」）。现在它们是
        //   `disabled(true)` 的按钮——同宽、同高、同一条右缘，只是灰的、点不动。
        //
        // ★ 宽度**不靠常量**（同日第三轮：按钮文本只有「下载」「升级」两种、都是两个汉字，
        //   所以量出来天然等宽，`ACTION_BTN_W` 已撤）。这里断言的是"各行彼此相等"，
        //   一旦哪天冒出一个更长/更短的文案，这条立刻红——比钉死一个数字更能反映意图。
        let slot = |i: usize| -> Rect {
            let row = tree.get(rows.children[i]).expect("行");
            tree.abs_bounds(*row.children.last().expect("动作格"))
        };
        let first = slot(0);
        assert!(first.w > 0, "动作格宽度塌成 0");
        for i in 1..rows.children.len() {
            let s = slot(i);
            assert_eq!(
                (s.x, s.w, s.h),
                (first.x, first.w, first.h),
                "第 {i} 行的动作格与第 0 行不同尺寸/不同右缘：\
                 灰态又变回纯文字了？还是有人给按钮加了长度不同的文案？"
            );
        }
    }
}
