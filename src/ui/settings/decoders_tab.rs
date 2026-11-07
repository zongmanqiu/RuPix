//! 「已安装」卡：搜索行、按钮行、计数行、解码器行与解码器详情窗。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
//!
//! ★ **搜索为什么必须能搜后缀**（2026-10-25 用户口径「单纯搜索名字完全没意义」）：
//!   已安装的解码器就那么几枚，名字一眼看完；用户真正要问的是「谁能解 jpg」。
//!   所以这里匹配的是**详情窗里那一整份自述**（名称 / id / DLL 文件 / 说明 / 认领后缀），
//!   而不只是行上那一个名字。
use super::*;

/// 「已安装」卡的正文：搜索行 + 按钮行（含计数）+ 解码器列表。
///
/// ★ 2026-10-25：这一块从「解码器页」降级成「解码器页」里的一张卡（见
/// `decoders.rs`），列表因此从 `list_signal`（自带滚动容器）换成 **`host_signal`**
/// （普通列容器、按内容定高）——卡里再嵌一条滚动条会出现"两条滚动条各滚一半"，
/// 整页由外层 `scroll` 统一滚。回归守卫见 `decoders.rs` 的
/// `decoders_page_cards_survive_layout_and_scroll` 与本文件的
/// `installed_rows_survive_layout`。
///
/// ★ 早先踩过的那个坑仍然值得记着（它决定了这里为什么不能用 `weight`）：
/// windui 的滚动容器按**无限高度**测量子元素，权重子节点在 `measure_linear` 里
/// 两遍都被跳过，永远量不到，arrange 只好按 0 高排版。2026-10-12 实测症状：计数行
/// 显示「发现 7 个解码器」、DynList 也重建出了行，但列表整块高度为 0——解码器行与开关
/// **整体不可见**。
pub fn installed_body(h: &Handles, config: Rc<RefCell<Config>>) -> Element {
    let open_dir = {
        let h = *h;
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            match catalog::ensure_dir() {
                Ok(d) => {
                    if !catalog::open_in_explorer(&d) {
                        log::warn!("打不开资源管理器：{}", d.display());
                    }
                }
                Err(e) => log::warn!("建解码器目录失败：{e}"),
            }
            // 用户可能刚往目录里丢了一枚 DLL：重扫 + **两张卡**一起刷（统一的底层入口）。
            h.refresh_after_dir_change(&config.borrow());
            ctx.mark_dirty();
        }
    };
    let rescan = {
        let h = *h;
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            // 重扫请求在底层函数里（少一处"只刷一半"的机会，见 `refresh_after_dir_change`）。
            h.refresh_after_dir_change(&config.borrow());
            // 成功不提示：按钮下方的计数行当场更新，屏幕自己已经说了（2026-10-16 清理）。
            // **失败必须说**：状态行已撤，这是页面上唯一能承载"读取失败"的地方——
            // 静默失败是最难查的一类 bug，删它等于把报错通道也堵了。
            let state = h.decoder_state.get();
            if state.starts_with("读取失败") {
                ctx.toast_err(state);
            }
            ctx.mark_dirty();
        }
    };

    // ★ 列表走 `host_signal`：普通列容器、按内容定高，整页只有一条滚动条（见函数头）。
    //   行的左右留白由 `decoder_row` 自己的 `padding_xy(16, 0)` 承载。
    //   ★ 绑的是**过滤视图**（`decoder_view`）而不是全量 `decoders`：搜索词一变，派生信号
    //     重算 → 列表当帧重建，与「首选解码器」那张卡同一套机制（无定时器、无差分）。
    let rows = Element::host_signal(h.decoder_view, {
        let config = config.clone();
        let h = *h;
        move |e: DecoderEntry| {
            // 搜着的时候行尾补一行灰的后缀（**只在搜索时**出现）：命中可能来自后缀或说明，
            // 行上光秃秃一个名字会让人不知道它为什么在这儿（2026-10-25 用户口径）。
            // 不搜的时候不加——那条"行里只放名字"的旧口径仍然成立。
            let hint = if h.decoder_filter.get().trim().is_empty() {
                None
            } else {
                Some(e.extensions.join(" · "))
            };
            decoder_row(&h, e, config.clone(), hint)
        }
    });

    Element::col()
        .fill()
        .spacing(0)
        // ① 搜索行：与「首选解码器」那张卡同一只放大镜、同一条 40px 行。
        .child(
            Element::row()
                .fill()
                .height(ROW_H)
                .cross(Align::Center)
                .padding_xy(16, 0)
                .child(installed_search_row(h).width_match().height(CONTROL_H)),
        )
        .child(Element::leaf().height(GAP))
        // ② 头部一行：**文字在左、按钮在右**（2026-10-26 用户口径「学习"可下载"
        // 那一行的布局」）。两张卡因此长得一样：左边一句红字说"现在什么情况"，
        // 右边是动作。文字靠 `weight(1.0)` 吃满余量，窗口再窄也只会截断文字，
        // 不会把按钮挤出去。
        //
        // 计数：**派生自同一份列表**（未搜 = "N 个解码器"，搜着 = "M / N 个解码器"），
        // 只有数字和单位，没有额外的说明文字。
        // **红字（`TEXT_ACCENT`）** 是本设计系统的强调色——快捷键页的段名
        // 同色（CurSeen 传统），与"危险操作红"（主题 `danger()`）是两套语义。
        //
        // ★ 横向 16：**与卡片标题文字、列表行内容同一条左对齐线**（卡片标题条是
        //   `padding_xy(16, 0)`，解码器行自己也是 16，本行不加倍）。
        //   头部与列表之间的 8px 间隔改由下方 `leaf(8)` 提供，避免给首元素加顶距。
        .child(
            Element::row()
                .fill()
                .spacing(8)
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label_signal(h.decoder_count)
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_ACCENT))
                        .weight(1.0)
                        .max_lines(1)
                        .truncate(Truncate::End),
                )
                .child(btn("文件夹").on_click(open_dir))
                .child(btn("刷新").on_click(rescan)),
        )
        // 按钮行与列表之间同样一格 GAP（与页四周、卡↔卡、卡头↔正文同一节奏）。
        .child(Element::leaf().height(GAP))
        .child(rows)
        // ③ 搜不到时的空态：**只在这一刻出现**。不搜的时候不给任何提示——
        // 卡片是空的（真的一枚解码器都没装）另有含义，别用同一句话糊过去。
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
                        !h.decoder_filter.get().trim().is_empty() && h.decoder_view.get().is_empty()
                    }
                }),
        )
}

/// [`installed_signals`] 的四件一组（clippy 的 `type_complexity` 要求拆个名）：
/// 全量 → 搜索词 → 过滤视图（派生）→ 计数（派生）。
pub type InstalledSignals = (
    Signal<Vec<DecoderEntry>>,
    Signal<String>,
    Signal<Vec<DecoderEntry>>,
    Signal<String>,
);

/// 「已安装」卡的四件基础信号：全量、搜索词、**派生过滤视图**，外加一枚派生计数。
///
/// 与 `decode_signals` 同款：视图挂在搜索词版本上，打字即时重算（无定时器、无差分）。
/// 计数也做成派生而不是"刷新时顺手写一次"——它与列表因此不可能各说一套。
pub fn installed_signals() -> InstalledSignals {
    let all = signal(Vec::<DecoderEntry>::new());
    let filter = signal(String::new());
    let view = filter.map(move |q| apply_decoder_filter(&all.get(), q));
    let count = view.map(move |v| {
        let n = all.get().len();
        if v.len() == n {
            format!("{n} 个解码器")
        } else {
            format!("{} / {n} 个解码器", v.len())
        }
    });
    (all, filter, view, count)
}

/// 「已安装」的搜索行：放大镜 + 过滤框。
///
/// 占位文字直接写明**能搜什么**（名称或后缀）——不然用户只会拿它搜名字，而那正是
/// 这枚框唯一没用的那种用法（2026-10-25 用户口径）。
pub fn installed_search_row(h: &Handles) -> Element {
    search_box(h.decoder_filter, "搜索名称或后缀")
}

/// 搜索命中判据（纯函数）：匹配**详情窗里那一整份自述**，而不只是行上那个名字。
///
/// 覆盖：显示名 / id / DLL 文件名（含版本那一段）/ 说明 / 认领的后缀。
/// 每一项都按子串、大小写不敏感——搜 `jpg` 命中认领 `.jpg` 的解码器，正是本卡的用处。
///
/// ★ **不覆盖**：大小、修改时间、ABI 号。那些是给人核对用的数字，拿它当搜索键
///   只会得到"输入 4 跳出四枚解码器"这种噪音。
pub fn decoder_matches(e: &DecoderEntry, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    e.display_name.to_lowercase().contains(&q)
        || e.id.to_lowercase().contains(&q)
        || e.file.to_lowercase().contains(&q)
        || e.description.to_lowercase().contains(&q)
        || e.extensions.iter().any(|x| x.to_lowercase().contains(&q))
}

/// 全量 → 过滤视图（[`Handles::decoder_view`] 的本体）。
pub fn apply_decoder_filter(all: &[DecoderEntry], query: &str) -> Vec<DecoderEntry> {
    all.iter()
        .filter(|e| decoder_matches(e, query))
        .cloned()
        .collect()
}

/// 解码器列表行：**名称 + 「!」详情钮 + 开关**，干净的一行一枚解码器。
///
/// 大小/说明/状态这些细节不再挤在行里（2026-10-14 用户口径「页面只需要 dll 的名字，
/// 点感叹号再看详情」），全部收进 [`open_decoder_info`] 弹出的详情窗。
/// ★ 唯一例外是 `hint`（**只在搜索时**给）：不给的话，靠后缀命中的那一行看不出
///   它为什么在结果里——搜 `jpg` 出来四枚解码器却全长得一样，等于没搜。
///
/// `h` 是设置窗的信号句柄：详情窗里的「卸载」要拿它当场刷新列表与计数行
/// （删完不刷，页面会留着一行"文件已经没了"的幽灵）。
pub fn decoder_row(
    h: &Handles,
    e: DecoderEntry,
    config: Rc<RefCell<Config>>,
    hint: Option<String>,
) -> Element {
    let sig = signal(e.enabled);
    let id = e.id.clone();
    let detail_entry = e.clone();

    // 「!」钮：按当前行解码器的快照弹详情窗（每点一次传一份克隆，窗口内容自包含、
    // 不再依赖列表信号，翻列表/重扫也不影响已开的详情窗）。
    let show_info = {
        let h = *h;
        let config = config.clone();
        move |ctx: &mut EventCtx| open_decoder_info(ctx, &h, detail_entry.clone(), config.clone())
    };

    let version = decoder_version(&e);
    let mut row = Element::row()
        .fill()
        .cross(Align::Center)
        .spacing(8)
        .height(ROW_H)
        .padding_xy(16, 0)
        .child(
            Element::label(e.display_name)
                .font_size(FONT_MD)
                .weight(1.0)
                // 单行省略：名字过长截断，别把版本/「!」/开关挤出右缘。
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(version_cell(&version));
    if let Some(text) = hint {
        // 灰的后缀串：最多一行、尾部截断，别把「!」与开关挤出右缘。
        row = row.child(
            Element::label(text)
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY))
                .width(160)
                .max_lines(1)
                .truncate(Truncate::End),
        );
    }
    let change = {
        let config = config.clone();
        let id = id.clone();
        move |on: bool| {
            let mut c = config.borrow_mut();
            if on {
                c.decoders_disabled.retain(|x| x != &id);
            } else if !c.decoders_disabled.iter().any(|x| x == &id) {
                c.decoders_disabled.push(id.clone());
            }
            crate::decoders::set_disabled(c.decoders_disabled.clone());
            // 「清单与能力不脱节」的另一半：declared_exts 按热加载代次缓存，开关不
            // bump 代次的话，目录网格会继续显示已经打不开的后缀（直到手动刷新/重启）。
            // 开关当场 +1，让格式清单与解码行为同拍生效（与「刷新」按钮同一入口）。
            crate::decoders::request_rescan();
            crate::core::config::mark_dirty();
            log::info!("解码器「{id}」已{}", if on { "启用" } else { "停用" });
        }
    };
    row.child(btn("!").on_click(show_info)).child(action_slot(
        Element::switch(sig).on_switch_change(move |_ctx, on| change(on)),
    ))
}

fn decoder_version(e: &DecoderEntry) -> String {
    e.metadata
        .iter()
        .find(|(k, _)| k == "version")
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
        .or_else(|| parse_dll_version(&e.file))
        .unwrap_or_default()
}

/// 解码器详情窗：点列表行「!」弹出（仿 [`open_metadata`]——owned、不可缩放、居中、
/// 单例键按解码器 id，重复点同一解码器只回到已开的那个）。内容取自点击时的条目快照，
/// 纯静态标签，无需信号；底部一颗「卸载」按钮**一击即走**，把 DLL 移进回收站，
/// 失败（DLL 被 `LoadLibrary` 占用）如实 toast 提示重启后再来——不假装成功、不留半删状态。
///
/// # 为什么不二次确认
///
/// 删除的代价已经可逆了：① 走的是回收站（[`crate::platform::recycle`]）而不是硬删，
/// 删错从回收站还原即可；② 解码器以后全部可联网重下。既然后果可撤，再让按钮先变成
/// 「确认卸载？」、多要一次点击，只是把简单操作变麻烦（2026-10-16 用户口径）。
pub fn open_decoder_info(
    ctx: &mut EventCtx,
    h: &Handles,
    e: DecoderEntry,
    config: Rc<RefCell<Config>>,
) {
    // Metadata first (works for a single third-party DLL); filename only as fallback.
    let meta = |key: &str| {
        e.metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let ver = {
        let v = meta("version");
        if v.is_empty() {
            parse_dll_version(&e.file).unwrap_or_default()
        } else {
            v
        }
    };
    let author = {
        let v = meta("author");
        if v.is_empty() {
            "（未知）".to_string()
        } else {
            v
        }
    };
    let license = {
        let v = meta("license");
        if v.is_empty() {
            "（未知）".to_string()
        } else {
            v
        }
    };
    let released = {
        let v = meta("released");
        if v.is_empty() {
            "—".to_string()
        } else {
            v
        }
    };
    let abi = e
        .abi
        .map_or_else(|| "（未知）".to_string(), |n| n.to_string());
    let exts = if e.extensions.is_empty() {
        "（无）".to_string()
    } else {
        e.extensions.join("; ")
    };
    let size = fmt_bytes(e.size);
    let desc = if e.description.is_empty() {
        "（无）".to_string()
    } else {
        e.description.clone()
    };
    let title = e.display_name.clone();
    let file_for_uninstall = e.file.clone();
    let path = e.path.clone();
    let single_key = format!("decoder-info-{}", e.id);

    // 行版式走 `rows::info_row`（标签列 92px + 值列自然折行），与「可下载」那张卡的
    // 详情窗是同一份——两个窗的字段不一样，版式不该长出两套。
    let row = info_row;

    // 卸载：一击即走。DLL 被 LoadLibrary 占用时外壳会拒绝（文件还在进程里跑着），
    // 此时如实提示重启后再来，不做重命名兜底、不假装成功。
    let uninstall = {
        let h = *h;
        let config = config.clone();
        let file = file_for_uninstall;
        move |ctx: &mut EventCtx| {
            match crate::platform::recycle::send_to_recycle_bin(&path) {
                Ok(()) => {
                    // 统一的底层入口一次推到底（重扫 + 已安装卡 + **首选卡** + 插件缓存）：
                    // 少刷一半就留一半幽灵——文件没了那行还挂着，或者首选页还列着这个后缀。
                    h.refresh_after_dir_change(&config.borrow());
                    // 不提示：详情窗当场关闭、列表少一行、计数减一，屏幕自己已经说了。
                    ctx.request_close();
                }
                Err(err) => ctx.toast_err(format!(
                    "移不进回收站 {file}：{err}\n（多半是 DLL 已加载，重启 RuPix 后再点）"
                )),
            }
            ctx.mark_dirty();
        }
    };

    // 字段按用户口径「够用+重要」定，值一律取自 DLL 自述（除文件名派生的 DLL 版本、
    let content = move || {
        Element::scroll()
            .fill()
            .padding_xy(16, PAGE_TOP_MARGIN)
            .child(
                Element::col()
                    .fill()
                    .spacing(0)
                    .child(row("解码器名", title.clone()))
                    .child(row("标识", e.id.clone()))
                    .child(row("版本", ver.clone()))
                    .child(row("ABI 版本", abi.clone()))
                    .child(row("作者", author.clone()))
                    .child(row("许可证", license.clone()))
                    .child(row("发行时间", released.clone()))
                    .child(divider())
                    .child(row("说明", desc.clone()))
                    .child(row("支持格式", exts.clone()))
                    .child(divider())
                    .child(row("大小", size.clone()))
                    .child(row("DLL 文件", e.file.clone()))
                    .child(Element::leaf().height(10))
                    // 卸载按钮：走主题 `danger` 语义（红底 + 圆角，与"窗口/工具栏
                    // 里的危险操作按钮"同源），不硬编 bg/fg 破坏主题。右对齐。
                    // 按钮下方不再挂任何说明行——删完就走，可逆的事不必先讲一通。
                    .child(
                        Element::row()
                            .fill()
                            .cross(Align::Center)
                            .padding_xy(0, 8)
                            .child(Element::flex_spacer())
                            .child(btn_danger("卸载").on_click(uninstall)),
                    ),
            )
    };

    // 起窗高度走 windui 补丁 07 的 `fit_content`：建 OS 窗口前用真实布局引擎按固定宽
    // 量一次内容自然高（文本换行、滚动容器完整测），夹进 [260, 屏高上限]。既不截断长
    // 内容，也不因固定高留大片空白。仍 `.resizable(true)`，用户可手动改。
    ctx.open_window(
        Window::new("解码器信息", 520, 400)
            // 信息窗客户区恒白（标题栏由系统绘制，不在其内）：与插件信息窗同款。
            .bg(to_color(BG))
            .fit_content(260, 1100)
            .resizable(true)
            .centered(true)
            .owned(true)
            .single(&single_key)
            .content(content),
    );
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// 「已安装」卡回归：扫描结果（信号 set）之后，列表行必须**真的可见**。
    ///
    /// 2026-10-12 踩坑（当时的形状是 `list_signal(...).weight(1.0)` 嵌在外层 `scroll` 里）：
    /// windui 滚动容器按**无限高度**测量子元素，权重子节点在 `measure_linear` 里
    /// 两遍都被跳过，永远量不到，arrange 只好按 0 高排版。症状：计数行显示「发现 7 个解码器」、
    /// DynList 也都重建出了行，但列表整块高度为 0——解码器行与开关整体不可见。
    /// 本测试把真实 `installed_body` 搬上无头 Tree，钉住「行数对 + 容器有高度」。
    ///
    /// ★ 滚动那条判据不在这里了：2026-10-25 起整页只有一条滚动条（三张卡按内容堆叠，
    /// 由外层 `scroll` 统一滚），"溢出时真的能滚"由 `decoders.rs` 的
    /// `decoders_page_cards_survive_layout_and_scroll` 钉住。
    #[test]
    fn installed_rows_survive_layout() {
        use windui::core::Tree;
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
            drag_image: sig(false),
            allow_multiple: sig(false),
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
            card_appearance: sig(true),
            card_window: sig(true),
            card_browse: sig(true),
            card_system: sig(true),
            card_ext_installed: sig(true),
            card_ext_available: sig(true),

            ui_theme: signal(0),
            download_source: signal(0),
            plugin: PluginUi::new(&Config::default()),
        };

        // 复刻解码器页的外形：导航条 + stack + scroll（卡按内容堆叠在里面）。
        let ui = Element::row()
            .fill()
            .child(Element::leaf().width(88).height_match())
            .child(
                Element::stack().fill().weight(1.0).child(
                    Element::scroll()
                        .fill()
                        .padding_xy(16, PAGE_TOP_MARGIN)
                        .child(section_card(
                            "已安装",
                            h.card_installed,
                            installed_body(&h, config),
                        )),
                ),
            );

        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 480);
        // 开窗首轮：decoders 信号还是空 Vec（与 open_settings 一致）。
        tree.layout_root(size, &mut NullTextEngine);

        // 进页扫一轮：一份 scan() 同时喂已安装卡与首选卡（`refresh_decoder_cards`）。
        // 计数行与列表同源（同一次 scan），两者的 set 必须成对出现——只刷列表不刷计数，
        // 计数标签会是空串（`NullTextEngine` 按字数算宽 → 宽 0），测出来的是假象。
        h.decoders
            .set(vec![entry("qoi"), entry("jxl"), entry("avif")]);
        // 视图挂在搜索词版本上：行集变了要同值写 bump 一下才重算（与两张卡重刷那处同款）。
        h.decoder_filter.set(h.decoder_filter.get());
        tree.layout_root(size, &mut NullTextEngine);

        // root(row) → stack → scroll → card(col) → [标题条, 正文]
        // 正文 = [搜索行, GAP, 头部行(计数+两按钮), GAP, 列表, 空态行]
        // （2026-10-16 用户口径：外挂目录/状态两行撤掉，个数并进按钮行、靠右、红字；
        //  2026-10-25 再加一行搜索——只搜名字没意义，要能搜后缀）
        let stack = tree.get(root).expect("row 根").children[1];
        let scroll_id = tree.get(stack).expect("stack").children[0];
        let card_id = tree.get(scroll_id).expect("scroll").children[0];
        let card = tree.get(card_id).expect("卡片").children.clone();
        assert_eq!(card.len(), 2, "卡片应为「标题条 + 正文」两段");
        let body_id = card[1];
        let kids = &tree.get(body_id).expect("正文 col").children;
        assert_eq!(
            kids.len(),
            6,
            "已安装卡正文应为「搜索行 + GAP + 按钮行 + GAP + 列表 + 空态行」六段"
        );
        // 竖向节奏：两处间隔都必须是同一格 GAP。
        for i in [1usize, 3] {
            assert_eq!(
                tree.get(kids[i]).expect("间隔").bounds.h,
                GAP,
                "第 {i} 段的间隔应为 GAP（全页同一节奏）"
            );
        }
        // 空态行只在"搜了但没命中"时出现：现在没搜 → 零高度。
        assert_eq!(
            tree.get(kids[5]).expect("空态").bounds.h,
            0,
            "没搜索时不该出现「没有匹配的解码器」"
        );

        // 头部行 = 计数 + 文件夹 + 刷新（**文字在左、按钮在右**，2026-10-26 用户口径
        // 「学习可下载那一行的布局」）。三件事：计数占尺寸、贴在左端、两个按钮在它右边。
        let head_id = kids[2];
        let head = tree.get(head_id).expect("头部行");
        assert_eq!(
            head.children.len(),
            3,
            "头部行应为「计数 + 文件夹 + 刷新」三个子节点"
        );
        let rows_id = kids[4];

        // 计数必须是**第一个**子节点（顺序写反 = 又回到"按钮在左、文字在右"的旧版式）。
        let count_id = head.children[0];
        let count_bounds = tree.get(count_id).expect("计数").bounds;
        assert!(
            count_bounds.h > 0,
            "计数（头部行第一个子节点）高度塌成 0：「xx 个解码器」看不见"
        );
        assert!(count_bounds.w > 0, "计数宽度塌成 0：数字一个都画不出来");
        // 比边必须用**绝对坐标**：`bounds` 是父相对的，行的边在页面坐标系里。
        let count_abs = tree.abs_bounds(count_id);
        let head_abs = tree.abs_bounds(head_id);
        // 头部行带 `padding_xy(16, 0)`（与卡片标题文字、解码器行同一条左对齐线）：
        // 计数左缘应内缩行左缘恰 16px（写成「≤17」只防它没排在行首）。
        assert!(
            count_abs.x <= head_abs.x + 17,
            "计数应贴住头部行内容左缘：计数左缘 {} vs 行左缘 {}",
            count_abs.x,
            head_abs.x
        );
        // 两个按钮必须整颗排在计数的右边（文字吃 weight、按钮不吃）。
        let btn_left = tree.abs_bounds(head.children[1]).x;
        assert!(
            btn_left > count_abs.x + count_abs.w,
            "「文件夹」按钮应排在计数右侧：按钮左缘 {btn_left} vs 计数右缘 {}",
            count_abs.x + count_abs.w
        );
        let rows = tree.get(rows_id).expect("行容器");
        assert_eq!(rows.children.len(), 3, "DynList 必须按新数据重建出 3 行");
        assert!(
            rows.bounds.h > 0,
            "行容器高度塌成 0：解码器行与开关会整体不可见（0 高那族旧病复发）"
        );
        // 每一行也要拿到确定高度（ROW_H），否则行内开关同样画不出来。
        for (i, &c) in rows.children.iter().enumerate() {
            assert_eq!(
                tree.get(c).expect("行").bounds.h,
                ROW_H,
                "第 {i} 行高度应为 ROW_H"
            );
        }
    }

    /// 搜索命中判据：★ 关键是**能按后缀搜**（用户原话「单纯搜索名字完全没意义」）——
    /// 搜 `jpg` 要命中那枚认领 `.jpg` 的解码器，尽管"支持格式"只写在详情窗里。
    /// 另钉：名称 / id / DLL 文件（含版本）/ 说明同样可搜；大小写不敏感；空串 = 全量；
    /// 而**大小 / 时间 / ABI 号不参与**（那是给人核对的数字，当搜索键只会出噪音）。
    #[test]
    fn decoder_search_matches_name_id_and_exts_but_not_numbers() {
        let entry = |id: &str, name: &str, exts: &[&str], desc: &str| DecoderEntry {
            id: id.to_string(),
            display_name: name.to_string(),
            description: desc.to_string(),
            extensions: exts.iter().map(|s| s.to_string()).collect(),
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
        let all = vec![
            entry("common", "常用格式", &["jpg", "png", "webp"], "日常位图"),
            entry("raw", "相机原始", &["cr2", "nef"], "RAW 参数窗"),
        ];

        // 空串 = 全量（未搜索时列表不该被悄悄过滤）。
        assert_eq!(apply_decoder_filter(&all, "").len(), 2);
        assert_eq!(apply_decoder_filter(&all, "   ").len(), 2);

        // ★ 后缀命中：这正是搜索框存在的理由。
        let hit = apply_decoder_filter(&all, "jpg");
        assert_eq!(hit.len(), 1, "搜 jpg 应命中认领 .jpg 的那枚");
        assert_eq!(hit[0].id, "common");
        assert_eq!(apply_decoder_filter(&all, "PNG").len(), 1, "大小写不敏感");
        assert_eq!(apply_decoder_filter(&all, "ne").len(), 1, "ne 命中 nef");
        // 名称 / id / 说明 / DLL 文件（版本那一段）也都能搜。
        assert_eq!(apply_decoder_filter(&all, "常用").len(), 1, "名称");
        assert_eq!(apply_decoder_filter(&all, "common").len(), 1, "id");
        assert_eq!(apply_decoder_filter(&all, "参数窗").len(), 1, "说明");
        assert_eq!(apply_decoder_filter(&all, "0.1.0").len(), 2, "DLL 版本段");
        // 数字字段不参与：ABI 4、大小 1234 都不该命中。
        assert_eq!(
            apply_decoder_filter(&all, "1234").len(),
            0,
            "大小不该当搜索键"
        );
        assert_eq!(
            apply_decoder_filter(&all, "4").len(),
            0,
            "ABI 号不该当搜索键"
        );
        // 无命中 = 空清单（页面上有空态行兜着）。
        assert_eq!(apply_decoder_filter(&all, "tiff").len(), 0);
    }

    /// 计数是**派生**于同一份列表的：未搜 = `N 个解码器`，搜着 = `M / N 个解码器`。
    /// 这条钉的是"计数与列表不会各说一套"（曾经是刷新时顺手 set 一次，两个来源）。
    #[test]
    fn installed_count_derives_from_the_same_list() {
        let (all, filter, view, count) = installed_signals();
        let entry = |id: &str, exts: &[&str]| DecoderEntry {
            id: id.to_string(),
            display_name: format!("{id} 解码器"),
            description: String::new(),
            extensions: exts.iter().map(|s| s.to_string()).collect(),
            file: format!("{id}_0.1.0.dll"),
            path: std::path::PathBuf::from(format!("decoders/{id}_0.1.0.dll")),
            size: 0,
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
        assert_eq!(count.get(), "0 个解码器", "空表时也不该写出怪句子");
        all.set(vec![entry("a", &["jpg"]), entry("b", &["png"])]);
        filter.set(filter.get()); // 同值写 bump：行集变了要让派生重算
        assert_eq!(view.get().len(), 2);
        assert_eq!(count.get(), "2 个解码器");
        // 搜 jpg → 视图 1 条，计数跟着变成 1 / 2。
        filter.set("jpg".to_string());
        assert_eq!(view.get().len(), 1);
        assert_eq!(count.get(), "1 / 2 个解码器");
        // 清空 → 回全量。
        filter.set(String::new());
        assert_eq!(count.get(), "2 个解码器");
    }
}
