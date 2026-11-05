//! 「解码」页：按扩展名列出认领者、首选解码器下拉与搜索过滤。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
use super::*;

/// 「首选解码器」卡的三件基础信号：全量数据、搜索词、**派生过滤视图**。
///
/// 视图是派生信号（挂搜索词版本）：打字即时重算，无需定时器与差分状态——这是
/// 过滤实时性的正主。行集变更（进页/重扫）由 [`Handles::decode_refresh`] 借
/// 「同值写」bump 搜索词版本驱动重算（`Signal::set` 无条件自增版本并通知）。
pub fn decode_signals() -> (Signal<Vec<ExtRow>>, Signal<String>, Signal<Vec<ExtRow>>) {
    let all = signal(Vec::<ExtRow>::new());
    let filter = signal(String::new());
    let view = filter.map(move |q| apply_filter(&all.get(), q));
    (all, filter, view)
}

/// 「解码」页一行的数据：扩展名 + 认领它的候选（DLL 路径字典序 = 运行时自动顺序）。
///
/// `pub` 只为消掉「pub 字段引用私有类型」的 clippy 警告（`Handles::ext_rows_all` 与
/// `Handles::ext_view` 是 pub 字段）；字段全部私有，外部构造不了，仍是 UI 内部类型。
#[derive(Debug, Clone, PartialEq)]
pub struct ExtRow {
    pub(crate) ext: String,
    /// 候选解码器 id（下拉第 i 项对应 `ids[i]`；第 0 项 = 默认接单者，文件名序第一个）。
    pub(crate) ids: Vec<String>,
    /// 下拉候选的 display_name，顺序与 `ids` 一致。
    pub(crate) labels: Vec<String>,
    /// 当前选中项（0 = 默认接单者）。
    pub(crate) current: usize,
}

/// 从解码器扫描结果 + 内置库登记表构建「首选解码器」行（纯函数，[`Handles::decode_refresh`] 的本体）。
///
/// `builtins` 传 `crate::core::formats::available_builtins()`（已接入的那些）：
/// 内置解码库从此**有身份**——出现在候选下拉里、可被 `decoders_preferred` 点名（m56）。
/// 排序规则是硬的：**内置一律排在解码器候选之后**，因为 `auto_claim=false` 的内置库
/// （WIC，m57）不参与默认仲裁——第 0 项必须仍是"不点名时的实际接单者"，
/// 否则"选中第 0 项 = 清除首选"这条语义就会把用户送去走内置路。
///
/// - 只收**健康且启用**的解码器：停用/加载失败的解码器在运行时本就不会接单，把它们
///   列进下拉只会误导（选了也白选，`preferred_candidate` 会静默回退自动）。
/// - **候选按实际可达的路由过滤**：只要有实现了 `iv_render_raw` 的启用解码器认领某
///   后缀，该后缀就走参数窗（`decode()` 里 `is_raw(ext)` 先于常规路径）——常规
///   认领者在参数化期间根本不可达，列出来只会诱导用户设一个不生效的首选（`.raw`
///   上的「Camera RAW」就是前车之鉴）。参数化后缀只列 has_raw 认领者，其余后缀
///   列全部认领者；两份候选都与运行时自动仲裁严格一致。
/// - **下拉直接列候选，不设「自动」项**（2026-10-16 用户口径）：第 0 项就是默认
///   接单者（文件名序第一个可达认领者），选中它等于清除首选、回到自动——行为
///   与稀疏覆盖语义严格一致，界面少一行黑话。
/// - 候选按解码器**路径**字典序：与 `candidate_for_ext` 的自动顺序完全一致。排序在
///   本函数内做——不依赖调用方传入的顺序，乱序输入也得到与运行时一致的下拉
///   （测试用乱序输入钉住这一点）。
/// - 用户首选命中的候选排到下拉当前位置；条目失效（id 不在候选里）回落 0（默认）。
/// - 按扩展名字典序（2026-10-16 用户口径：不做「争用排前」——候选多寡不应打乱
///   字母表，用户按名字找行）。
pub fn ext_rows_from(
    decoders: &[DecoderEntry],
    builtins: &[crate::core::formats::BuiltinDecoder],
    preferred: &BTreeMap<String, String>,
) -> Vec<ExtRow> {
    // 先过滤出可用解码器，并记下「参数窗可达」的后缀集合（has_raw 解码器认领的并集，
    // 与 crate::decoders::declared_raw_exts 的口径一致：同一批启用/健康过滤）。
    let usable: Vec<&DecoderEntry> = decoders
        .iter()
        .filter(|p| p.enabled && matches!(p.health, Health::Ok))
        .collect();
    let mut param_exts: HashSet<String> = HashSet::new();
    for p in &usable {
        if p.has_raw {
            param_exts.extend(p.extensions.iter().cloned());
        }
    }
    // ext → [(path, display_name, id)]；路径用于下面对齐运行时的自动顺序。
    let mut by_ext: BTreeMap<String, Vec<(std::path::PathBuf, String, String)>> = BTreeMap::new();
    for p in &usable {
        for e in &p.extensions {
            // 参数化后缀只收 has_raw 认领者（常规认领者不可达，见函数头）。
            if param_exts.contains(e) && !p.has_raw {
                continue;
            }
            by_ext.entry(e.clone()).or_default().push((
                p.path.clone(),
                p.display_name.clone(),
                p.id.clone(),
            ));
        }
    }
    // 内置库的候选，按后缀归堆（表序即显示序）。参数化后缀不收内置：那条路先于常规
    // 路径进 `decode()`，列出来同样是"选了白选"。
    let mut by_ext_builtin: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for b in builtins {
        for e in b.extensions {
            if param_exts.contains(*e) {
                continue;
            }
            by_ext_builtin
                .entry((*e).to_string())
                .or_default()
                .push((b.display_name.to_string(), b.id.to_string()));
        }
    }
    // 行集 = 解码器认领的后缀 ∪ 内置认领的后缀（svg/svgz 以前在这页**根本不存在**，
    // 因为这一页今天只从 DLL 清单派生——m56 之后内置也有身份）。
    let mut all_exts: Vec<String> = by_ext.keys().cloned().collect();
    for e in by_ext_builtin.keys() {
        if !all_exts.contains(e) {
            all_exts.push(e.clone());
        }
    }
    let mut rows: Vec<ExtRow> = all_exts
        .into_iter()
        .map(|ext| {
            // 内置排在解码器候选**之后**（见函数头的排序规则）：第 0 项必须是不点名时
            // 真正接单的那位，否则"选第 0 项 = 清除首选"就把用户送进内置路了。
            let mut cands = by_ext.remove(&ext).unwrap_or_default();
            cands.sort_by(|a, b| a.0.cmp(&b.0));
            if let Some(bs) = by_ext_builtin.remove(&ext) {
                cands.extend(bs.into_iter().map(|(name, id)| {
                    // 内置没有 DLL 路径可排；排序已完成，这里的路径槽仅作占位，
                    // 用 id 填它，保证后续没有任何地方再按这个键重排。
                    (std::path::PathBuf::from(id.clone()), name, id)
                }));
            }
            // 下拉直接列候选（2026-10-16 用户口径：不设「自动」项）：第 0 项就是
            // 默认接单者（文件名序第一个），选中它 = 清除首选、回到自动。
            let labels: Vec<String> = cands.iter().map(|(_, n, _)| n.clone()).collect();
            let current = preferred
                .get(&ext)
                .and_then(|id| cands.iter().position(|(_, _, i)| i == id))
                .unwrap_or(0);
            ExtRow {
                ext,
                ids: cands.into_iter().map(|(_, _, id)| id).collect(),
                labels,
                current,
            }
        })
        .collect();
    // 2026-10-16 用户口径：按扩展名字典序，不做「争用排前」——.png 有两个候选就
    // 排到 3fr 前面，用户找不到自己要的后缀；字典序才可预期。
    rows.sort_by(|a, b| a.ext.cmp(&b.ext));
    rows
}

/// 「首选解码器」每行下拉的选中信号（进程级单例，UI 线程专用）。
///
/// 信号必须**跨开窗同一只**：`.single("settings")` 已开时树不重建（理由同
/// [`shortcuts_ui`]），每开一次换新信号会让「显示 ↔ 配置」双向失联。`ids` 是
/// 行数据快照，[`Handles::sync`] 差分轮询时用它把「下拉位次」翻译成「解码器 id」。
pub struct DecodeRow {
    pub(crate) sig: Signal<usize>,
    pub(crate) ids: Vec<String>,
    /// 上一次由 `decode_refresh` 写进 `sig` 的位置。`sync` 只处理"用户真的改了"
    /// 的差值；否则刷新把首选候选排到第 0 位时，会被误读成"用户选了自动"而删掉偏好。
    pub(crate) last: std::cell::Cell<usize>,
}

thread_local! {
    /// 「首选解码器」行信号表：`扩展名 → {选中信号, 候选快照}`。整张表建一次终身复用，
    /// 行的增删由 [`Handles::decode_refresh`] 对齐（retain 掉消失的扩展名）。
    /// `BTreeMap::new()` 是 const fn，const 初始化免掉每线程首次访问的懒构造（clippy）。
    pub(crate) static DECODE_ROWS: RefCell<BTreeMap<String, DecodeRow>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// 取（必要时建）某扩展名行的选中信号。只服务测试与兜底路径——正常流程里
/// [`Handles::decode_refresh`] 会先把表填好，`ext_row` 只是按名领取。
pub fn decode_row_sig(ext: &str, current: usize) -> Signal<usize> {
    DECODE_ROWS.with(|m| {
        let mut g = m.borrow_mut();
        if let Some(row) = g.get(ext) {
            return row.sig;
        }
        let sg = signal(current);
        g.insert(
            ext.to_string(),
            DecodeRow {
                sig: sg,
                ids: Vec::new(),
                last: std::cell::Cell::new(current),
            },
        );
        sg
    })
}

/// 「首选解码器」卡的正文：搜索框 + 全量「扩展名 → 接单解码器」映射（每行一个首选下拉）。
///
/// 与「已安装」卡的分工：那张卡管解码器生命周期（启停/卸载/详情），这张卡管**归属**——
/// 同一扩展名被多个解码器认领时（第一方有唯一认领门禁，争用只可能来自第三方 DLL），
/// 用户可在此指定首选；没指定的后缀按解码器文件名顺序自动选择（`candidate_for_ext`）。
/// 没有争用时这份清单依然成立：它就是一份「谁在解什么」的透明账，用户不必开
/// 详情窗逐个查扩展名。
///
/// ★ 2026-10-25：这一块从「解码页」降级成解码器页里的第三张卡，列表由 `list_signal`
/// 换成 `host_signal`（按内容定高，整页一条滚动条），理由同 `installed_body`。
// ini 键：decoders_preferred（每行下拉经 Handles::sync 差分轮询写回 config 并即时灌给解码线程）
pub fn preferred_body(h: &Handles) -> Element {
    let search = decode_search_row(h);
    // 列表绑定**派生过滤视图**：搜索词版本一动（每个键入都写信号）DynList 当帧
    // 重算重排——实时性由信号机制本身保证，无定时器无差分。
    let rows = Element::host_signal(h.ext_view, |r: ExtRow| ext_row(&r));
    Element::col()
        .fill()
        .spacing(0)
        // 搜索框包进 40px 居中行，与下方 ext_row（亦是 40px 行）同节奏；输入框钉高
        // `CONTROL_H` 后在行内垂直居中，与下方下拉框同一竖线、同一中点（旧版裸输入框
        // 贴顶，与各页 40px 行不齐）。横向 16 = 与卡片标题文字同一条左对齐线。
        .child(
            Element::row()
                .fill()
                .height(ROW_H)
                .cross(Align::Center)
                .padding_xy(16, 0)
                .child(search.width_match().height(CONTROL_H)),
        )
        // 搜索行与列表之间同样一格 GAP（与页四周、卡↔卡、卡头↔正文同一节奏）。
        .child(Element::leaf().height(GAP))
        .child(rows)
}

/// 「首选解码器」的搜索行：放大镜 + 过滤输入框，占位文字报**当前支持的后缀种数**。
///
/// 用户点开这张卡想知道的是"这软件现在认多少种后缀"，不是搜索语法；种数由
/// [`Handles::decode_refresh_from`] 写进 [`Handles::ext_count`]，与下面列表同一来源
/// （`ext_rows_all` 的行集），不另数一遍。
///
/// 重建键就是那个计数：它只在进页 / 重扫时变（`decode_refresh_from` 判过 `changed` 才写），
/// 打字过程只动 `ext_filter` 信号，不重建这一行，焦点与已输入文本都安全。
/// 输入框本体是 [`search_box`]（与「已安装」卡同一只，宽度因此天然一致）。
pub fn decode_search_row(h: &Handles) -> Element {
    let text = h.ext_count.map(|n| vec![preferred_placeholder(*n)]);
    // `Signal` 是句柄语义（`Copy`），搬进闭包不需要 clone。
    let filter = h.ext_filter;
    Element::host_signal(text, move |t| search_box(filter, t).width_match())
}

/// 占位文案（纯函数：措辞与数字口径由用例直接钉住）。
pub fn preferred_placeholder(count: usize) -> String {
    format!("当前支持 {count} 个后缀格式")
}

/// 搜索过滤（纯函数）：按扩展名子串匹配、大小写不敏感；空串 = 全量。
pub fn apply_filter(rows: &[ExtRow], query: &str) -> Vec<ExtRow> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|r| {
            r.ext.to_lowercase().contains(&q)
                || r.labels
                    .iter()
                    .any(|label| label.to_lowercase().contains(&q))
        })
        .cloned()
        .collect()
}

/// 「首选解码器」一行：扩展名 + 该后缀的解码库下拉。
///
/// 下拉直接列候选（2026-10-16 用户口径：无「自动」项、不解释路由机制）：第 0 项 =
/// 默认接单者（文件名序第一个可达认领者），选中它 = 清除首选、回到自动。
/// 回写走 [`Handles::sync`] 的差分轮询（windui 下拉无 onChange 回调），信号从行
/// 信号单例领取，跨树重建稳定。
pub fn ext_row(r: &ExtRow) -> Element {
    let sig = decode_row_sig(&r.ext, r.current);
    // 不用 option_row：它末尾的 flex_spacer 会把下拉固定在 200px，右侧留一大片空。
    // 这里让下拉吃满标签列之后的全部宽度，右缘与其它卡片的动作槽同线。
    Element::row()
        .child(
            Element::label(&r.ext)
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY))
                .width(COL1_W),
        )
        .child(Element::dropdown(r.labels.clone(), sig).weight(1.0))
        .cross(Align::Center)
        .height(ROW_H)
        .padding_xy(16, 0)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// 无头用例共用的 `Handles` 夹具（只造信号，不碰窗口与解码线程）。
    ///
    /// 两条版式用例都要**独立**的一份（各自 `set` 不同的行数/计数），所以这里是函数
    /// 而不是静态单例；共用它是为了不再抄第二份 40 行的字段清单。
    fn test_handles() -> Handles {
        let (ext_rows_all, ext_filter, ext_view) = decode_signals();
        Handles {
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
            decoder_count: signal(String::new()),
            decoder_state: signal(String::new()),
            decoders: signal(Vec::<DecoderEntry>::new()),
            decoder_view: signal(Vec::<DecoderEntry>::new()),
            decoder_filter: signal(String::new()),
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

            ui_theme: signal(0),
            download_source: signal(0),
            plugin: PluginUi::new(&Config::default()),
        }
    }

    /// 占位文案的口径：报的是**后缀种数**，不是搜索语法说明。
    #[test]
    fn preferred_placeholder_reports_suffix_count() {
        assert_eq!(preferred_placeholder(109), "当前支持 109 个后缀格式");
        // 一枚解码器都没装时也不能写"0 个"之外的怪话：这个数字就是列表的行数。
        assert_eq!(preferred_placeholder(0), "当前支持 0 个后缀格式");
    }

    /// 搜索行的版式回归：包进 `host_signal` 后**不许塌宽**（K156 那一族：水平容器主轴上的
    /// `Match` 会被降级成按内容宽），并且换计数时真的重建（占位文字是这一行的输入）。
    #[test]
    fn preferred_search_row_keeps_width_and_follows_count() {
        use windui::core::Tree;
        use windui::text::NullTextEngine;

        let h = test_handles();
        let ui = Element::row()
            .fill()
            .child(Element::leaf().width(88).height_match())
            .child(
                Element::stack().fill().weight(1.0).child(
                    Element::scroll()
                        .fill()
                        .padding_xy(16, PAGE_TOP_MARGIN)
                        .child(section_card(
                            "首选解码器",
                            h.card_preferred,
                            preferred_body(&h),
                        )),
                ),
            );
        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 480);
        tree.layout_root(size, &mut NullTextEngine);

        let deepest = |tree: &Tree, start: windui::core::NodeId| {
            let mut id = start;
            loop {
                let node = tree.get(id).expect("搜索行子树");
                match node.children.first() {
                    Some(c) => id = *c,
                    None => return id,
                }
            }
        };
        let body_of_card = |tree: &Tree| {
            let stack = tree.get(root).expect("row 根").children[1];
            let scroll_id = tree.get(stack).expect("stack").children[0];
            let card_id = tree.get(scroll_id).expect("scroll").children[0];
            tree.get(card_id).expect("卡片").children[1]
        };

        let row = tree.get(body_of_card(&tree)).expect("正文 col").children[0];
        let input = deepest(&tree, row);
        // 搜索行自己带 16px 左内边距（与卡片标题同一条对齐线），所以输入框右缘应落在
        // **该行右缘减 16** 上——塌宽时它只会占占位文字的固有宽（K156 那一族）。
        let line = tree.abs_bounds(row);
        let bounds = tree.abs_bounds(input);
        assert_eq!(
            bounds.right(),
            line.right() - 16,
            "搜索框没吃到本行的可用宽（塌宽 = 与下方下拉不同一条右线）"
        );
        assert!(
            bounds.w >= line.w - 32,
            "搜索框宽度只占内容宽：input={bounds:?} line={line:?}"
        );
        assert!(bounds.h >= CONTROL_H - 2, "搜索框高度塌了：{bounds:?}");

        // 换计数 → 这一行必须重建（新节点），宽度仍然不塌。
        h.ext_count.set(7);
        tree.layout_root(size, &mut NullTextEngine);
        let row2 = tree.get(body_of_card(&tree)).expect("正文 col").children[0];
        let input2 = deepest(&tree, row2);
        assert_ne!(
            input, input2,
            "后缀种数变了却没重建这一行，占位文字会停在旧数字"
        );
        assert_eq!(
            tree.abs_bounds(input2).right(),
            tree.abs_bounds(row2).right() - 16,
            "重建后搜索框塌宽"
        );
    }

    /// 「首选解码器」卡回归：数据落位之后行必须**真的可见**。
    ///
    /// 判据与 `installed_rows_survive_layout` 同款（防"行容器高度塌成 0"那族旧病），
    /// 差别只在载体：2026-10-25 起这张卡的列表走 `host_signal`（按内容定高），
    /// 整页由外层 `scroll` 统一滚——"溢出能滚"由 `decoders.rs` 那条用例钉住。
    #[test]
    fn preferred_rows_survive_layout() {
        use windui::core::Tree;
        use windui::text::NullTextEngine;

        let h = test_handles();

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
                            "首选解码器",
                            h.card_preferred,
                            preferred_body(&h),
                        )),
                ),
            );

        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 480);
        tree.layout_root(size, &mut NullTextEngine);

        // 进页：数据落位 + 同值写 bump（模拟 decode_refresh 的行集变更触发，
        // 派生过滤视图随版本重算）。
        let row = |ext: &str, ids: &[&str]| ExtRow {
            ext: ext.to_string(),
            ids: ids.iter().map(|s| s.to_string()).collect(),
            labels: ids.iter().map(|id| format!("{id} 名")).collect(),
            current: 0,
        };
        h.ext_rows_all
            .set(vec![row("jpg", &["aaa", "mmm"]), row("png", &["zzz"])]);
        h.ext_filter.set(h.ext_filter.get());
        tree.layout_root(size, &mut NullTextEngine);

        // root(row) → stack → scroll → card(col) → [标题条, 正文]；正文 = [搜索行, 列表]
        let stack = tree.get(root).expect("row 根").children[1];
        let scroll_id = tree.get(stack).expect("stack").children[0];
        let card_id = tree.get(scroll_id).expect("scroll").children[0];
        let card = tree.get(card_id).expect("卡片").children.clone();
        assert_eq!(card.len(), 2, "卡片应为「标题条 + 正文」两段");
        let kids = &tree.get(card[1]).expect("正文 col").children;
        assert_eq!(
            kids.len(),
            3,
            "首选解码器卡正文应为「搜索行 + GAP 间隔 + 列表」三段"
        );
        assert_eq!(
            tree.get(kids[1]).expect("间隔").bounds.h,
            GAP,
            "搜索行与列表之间应是一格 GAP（与页四周、卡↔卡同一节奏）"
        );
        let rows = tree.get(kids[2]).expect("行容器");
        assert_eq!(rows.children.len(), 2, "DynList 必须按新数据重建出 2 行");
        assert!(
            rows.bounds.h > 0,
            "行容器高度塌成 0：每种格式的首选下拉会整体不可见"
        );
        for (i, &c) in rows.children.iter().enumerate() {
            let node = tree.get(c).expect("行");
            assert_eq!(node.bounds.h, ROW_H, "第 {i} 行高度应为 ROW_H");
            // 下拉必须吃到右侧留白前：右缘与其它卡片的动作按钮同一条线。
            let control = node.children[1];
            assert_eq!(
                tree.abs_bounds(control).right(),
                tree.abs_bounds(c).right() - 16,
                "第 {i} 行的下拉没有延伸到行右缘（差 16px 内边距）"
            );
        }
    }

    /// 搜索过滤（纯函数）：后缀子串、大小写不敏感、空串全量、trim 后为空同空串。
    #[test]
    fn apply_filter_matches_ext_substring_case_insensitive() {
        let row = |ext: &str| ExtRow {
            ext: ext.to_string(),
            ids: vec!["x".to_string()],
            labels: vec!["x".to_string()],
            current: 0,
        };
        let mut rows = vec![row("jpg"), row("png"), row("webp")];
        rows[0].labels = vec!["JPEG 解码器".to_string()];
        assert_eq!(apply_filter(&rows, "").len(), 3, "空串 = 全量");
        assert_eq!(apply_filter(&rows, "   ").len(), 3, "纯空白 = 全量");
        let hit = apply_filter(&rows, "PN");
        let got: Vec<&str> = hit.iter().map(|r| r.ext.as_str()).collect();
        assert_eq!(got, vec!["png"], "大小写不敏感的子串命中");
        let hit = apply_filter(&rows, "W");
        let got: Vec<&str> = hit.iter().map(|r| r.ext.as_str()).collect();
        assert_eq!(got, vec!["webp"]);
        assert_eq!(apply_filter(&rows, "xyz").len(), 0, "无命中 = 空清单");
        let hit = apply_filter(&rows, "jpeg");
        let got: Vec<&str> = hit.iter().map(|r| r.ext.as_str()).collect();
        assert_eq!(got, vec!["jpg"], "名称也要能命中");
    }

    /// 派生过滤视图回归：数据落位 + 同值写 bump（decode_refresh 的做法）必须出
    /// 全量视图（首进页差分门回归锚）；打字即时重算；清空回全量；重扫换数据跟算。
    #[test]
    fn derived_view_tracks_filter_and_rows() {
        let all = signal(Vec::<ExtRow>::new());
        let filter = signal(String::new());
        let view = filter.map(move |q| apply_filter(&all.get(), q));
        let row = |ext: &str| ExtRow {
            ext: ext.to_string(),
            ids: vec!["x".to_string()],
            labels: vec!["x".to_string()],
            current: 0,
        };
        // 首进页：数据落位 + 同值写 bump（query 与 applied 的旧差分门已随重构消亡）。
        all.set(vec![row("png"), row("jpg")]);
        filter.set(filter.get());
        assert_eq!(view.get().len(), 2, "首进页必须出全量视图");
        // 打字：过滤即时生效。
        filter.set("pn".to_string());
        let hit = view.get();
        assert_eq!(hit.len(), 1, "打字过滤仍生效");
        assert_eq!(hit[0].ext, "png");
        // 重扫：行集变了 + 搜索词跟进（"jp"）→ 视图重算（jpg 命中）。
        all.set(vec![row("jpg")]);
        filter.set("jp".to_string());
        let hit = view.get();
        assert_eq!(hit.len(), 1, "行集变更后视图重算");
        assert_eq!(hit[0].ext, "jpg");
        // 清空搜索 → 回全量。
        filter.set(String::new());
        let hit = view.get();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].ext, "jpg");
    }

    /// 装完解码器之后**「首选解码器」那一页也必须跟着变**（2026-10-29 用户报障：
    /// 下载装好、图当场也能解了，但首选页的后缀集合停在旧值——必须关掉设置窗再开一下）。
    ///
    /// 夹具刻意用**空目录**：一个 DLL 都没有时内置矢量（svg/svgz）仍在 ⇒ 首选页应有行；
    /// 而"只刷已安装卡"的那条旧路根本不碰 `ext_count`，于是它停在 0。这条因此是对
    /// **入口**的判据，不是对某个信号赋值顺序的判据。
    #[test]
    fn install_completion_refreshes_the_preferred_card_too() {
        let dir = crate::core::paths::test_scratch_root().join("settings-refresh");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var_os("RUPIX_DECODERS_DIR");
        std::env::set_var("RUPIX_DECODERS_DIR", &dir);

        let h = test_handles();
        let before = h.ext_count.get();
        h.refresh_after_dir_change(&crate::core::config::Config::default());
        let after = h.ext_count.get();

        match prev {
            Some(v) => std::env::set_var("RUPIX_DECODERS_DIR", v),
            None => std::env::remove_var("RUPIX_DECODERS_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(before, 0, "起点：首选页还没有任何行");
        assert!(
            after > 0,
            "装完之后首选页的后缀行没刷新（内置矢量也算行）：{before} → {after}"
        );
    }

    /// **一处设置，两张卡都跟着走**（2026-10-30 统一「下载源」）。
    ///
    /// 旧形状下解码器卡与插件卡各有一个下拉、各存一个 ini 键——能配成"解码器走 Gitee、
    /// 插件走 GitHub"，而一个用户机器的网络是固定的，那种不一致只会让人以为下载坏了。
    /// 这条钉：改信号 → `sync` 写进配置（落盘）→ 插件卡的门面信号跟着变 → `push` 回灌
    /// **不会把选择打回去**（旧的两个下拉曾经每帧被 `push` 覆盖）。
    #[test]
    fn one_download_source_drives_both_cards() {
        let h = test_handles();
        let mut c = crate::core::config::Config::default();
        h.download_source.set(1); // 选项页选 GitHub
        assert!(h.sync(&mut c), "改了下载源要标脏（下一次落盘）");
        assert_eq!(c.download_source, crate::core::config::SourceId::Github);
        assert_eq!(
            h.plugin.source.get(),
            1,
            "插件卡是同一份配置的投影，必须跟着变"
        );
        h.push(&c);
        assert_eq!(h.download_source.get(), 1, "push 不许把选择打回去");
        assert_eq!(h.plugin.source.get(), 1);
    }

    /// 行构建纯函数：健康/启用过滤、候选路径序、字典序、首选位次映射与失效回落。
    #[test]
    fn ext_rows_from_filters_sorts_and_maps_preferred() {
        let entry = |id: &str, file: &str, exts: &[&str], enabled: bool| DecoderEntry {
            id: id.to_string(),
            display_name: format!("{id} 名"),
            description: String::new(),
            extensions: exts.iter().map(|s| s.to_string()).collect(),
            file: file.to_string(),
            path: std::path::PathBuf::from(file),
            size: 0,
            mtime: None,
            health: Health::Ok,
            enabled,
            has_raw: false,
            has_vector: false,
            vector_exts: Vec::new(),
            vector_region: false,
            abi: Some(5),
            metadata: Vec::new(),
        };
        // 加载失败的解码器（health ≠ Ok）不得进候选，哪怕它启用着。
        let mut bad = entry("bad", "decoders/bad.dll", &["jpg"], true);
        bad.health = Health::LoadFailed("x".to_string());
        let decoders = vec![
            entry("zzz", "decoders/zzz.dll", &["jpg", "png"], true),
            entry("aaa", "decoders/aaa.dll", &["jpg"], true),
            entry("off", "decoders/off.dll", &["jpg"], false),
            bad,
        ];
        let rows = ext_rows_from(&decoders, &[], &BTreeMap::new());
        // jpg 三家自述争用，但停用/加载失败的不得入候选 → 2 家；按扩展名字典序。
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ext, "jpg");
        assert_eq!(
            rows[0].ids,
            vec!["aaa", "zzz"],
            "候选按路径字典序，停用/坏解码器不入列"
        );
        assert_eq!(rows[0].labels[0], "aaa 名", "第 0 项就是默认接单者本身");
        assert_eq!(rows[1].ext, "png");
        assert_eq!(rows[1].labels[0], "zzz 名");
        // 首选命中 → 位次 = 候选下标；失效条目 → 0（默认接单者）。
        let mut pref = BTreeMap::new();
        pref.insert("jpg".to_string(), "zzz".to_string());
        pref.insert("png".to_string(), "ghost".to_string());
        let rows = ext_rows_from(&decoders, &[], &pref);
        assert_eq!(rows[0].current, 1, "zzz 是第 1 个候选 → 下拉位次 1");
        assert_eq!(rows[1].current, 0, "指向不存在解码器的首选回落默认");
    }

    /// 内置解码库在「首选解码器」里**有身份**（m56）：它认领的后缀进列表、进下拉，
    /// 并且**排在解码器候选之后**——第 0 项必须仍是"不点名时真正接单的那位"。
    ///
    /// ★ 两条都要钉：
    /// - `auto_claim=false` 的内置库（m57 的 WIC）能被点名，就**不能**同时是第 0 项，
    ///   否则"选第 0 项 = 清除首选"这条既有语义会把用户悄悄送进 OS 解码器；
    /// - 内置库不靠 DLL 文件存在，`decoders/` 目录空着它也必须在页上——
    ///   否则内置就是隐身路，"清单与能力不脱节"这句话对它不成立。
    #[test]
    fn builtin_decoders_join_the_candidate_list_after_decoders() {
        use crate::core::formats::BuiltinDecoder;
        let builtins = crate::core::formats::available_builtins()
            .copied()
            .collect::<Vec<BuiltinDecoder>>();
        assert!(
            builtins.iter().any(|b| b.id == "resvg"),
            "夹具前提：resvg 得是已接入的内置库"
        );
        // 臂①：没有任何解码器时，内置认领的后缀仍然成行（今天 = svg / svgz）。
        let rows = ext_rows_from(&[], &builtins, &BTreeMap::new());
        let exts: Vec<&str> = rows.iter().map(|r| r.ext.as_str()).collect();
        assert!(
            exts.contains(&"svg") && exts.contains(&"svgz"),
            "内置后缀在空解码器目录里也该出现：{exts:?}"
        );
        let svg = rows.iter().find(|r| r.ext == "svg").expect("svg 行");
        assert_eq!(svg.ids, vec!["resvg"], "svg 的候选就是内置库本身");
        // 出货字面量（2026-10-20 用户口径：`WIC (内置)` / `resvg (内置)`，别再拼长串）。
        assert_eq!(svg.labels[0], "resvg (内置)");
        assert_eq!(svg.current, 0, "唯一候选 = 默认接单者 = 不设首选");

        // 臂②：同一后缀被解码器与内置同时认领时，内置排**最后**。
        let fake = BuiltinDecoder {
            id: "fake",
            display_name: "假内置",
            description: "只为测排序",
            extensions: &["jpg"],
            available: true,
            auto_claim: false,
            is_vector: false,
            has_alpha: false,
        };
        let entry = |id: &str, file: &str| DecoderEntry {
            id: id.to_string(),
            display_name: format!("{id} 名"),
            description: String::new(),
            extensions: vec!["jpg".to_string()],
            file: file.to_string(),
            path: std::path::PathBuf::from(file),
            size: 0,
            mtime: None,
            health: Health::Ok,
            enabled: true,
            has_raw: false,
            has_vector: false,
            vector_exts: Vec::new(),
            vector_region: false,
            abi: Some(5),
            metadata: Vec::new(),
        };
        let rows = ext_rows_from(
            &[
                entry("aaa", "decoders/aaa.dll"),
                entry("bbb", "decoders/bbb.dll"),
            ],
            &[fake],
            &BTreeMap::new(),
        );
        let jpg = rows.iter().find(|r| r.ext == "jpg").expect("jpg 行");
        assert_eq!(
            jpg.ids,
            vec!["aaa", "bbb", "fake"],
            "内置必须排在解码器之后：第 0 项才是默认接单者"
        );
        // 点名内置 → 位次落在最后那项；首选失效（没这条内置）→ 回落 0。
        let mut pref = BTreeMap::new();
        pref.insert("jpg".to_string(), "fake".to_string());
        let rows = ext_rows_from(&[entry("aaa", "decoders/aaa.dll")], &[], &pref);
        assert_eq!(
            rows[0].current, 0,
            "内置没接入这张表时，指向它的首选要回落默认（与解码器失效同规）"
        );
    }

    /// 参数化路由：has_raw 解码器认领的后缀只列 has_raw 认领者（常规认领者经参数窗
    /// 路由不可达，列出来会诱导用户设不生效的首选——.raw 上的「Camera RAW」教训），
    /// 自动标签标明「参数窗」；该解码器停用后，后缀失去参数化身份、落回常规认领者。
    /// 这组断言就是用户报告的「关掉 Camera RAW 后 .raw 行去向」问题的回归锚。
    #[test]
    fn ext_rows_from_routes_parameterized_exts_to_has_raw_claimants() {
        let entry =
            |id: &str, file: &str, exts: &[&str], has_raw: bool, enabled: bool| DecoderEntry {
                id: id.to_string(),
                display_name: format!("{id} 名"),
                description: String::new(),
                extensions: exts.iter().map(|s| s.to_string()).collect(),
                file: file.to_string(),
                path: std::path::PathBuf::from(file),
                size: 0,
                mtime: None,
                health: Health::Ok,
                enabled,
                has_raw,
                has_vector: false,
                vector_exts: Vec::new(),
                vector_region: false,
                abi: Some(5),
                metadata: Vec::new(),
            };
        let decoders = vec![
            entry("raw", "decoders/raw.dll", &["cr2", "raw"], false, true),
            entry("rawpx", "decoders/rawpx.dll", &["raw"], true, true),
        ];
        let rows = ext_rows_from(&decoders, &[], &BTreeMap::new());
        // raw：rawpx 的参数窗地盘，常规认领者（raw 解码器）不可达，不得入列。
        let raw_row = rows.iter().find(|r| r.ext == "raw").expect("raw 行应在");
        assert_eq!(raw_row.ids, vec!["rawpx"], "参数化后缀只列 has_raw 认领者");
        assert_eq!(raw_row.labels[0], "rawpx 名");
        // cr2：没有 has_raw 认领 → 常规路由，raw 解码器入列。
        let cr2_row = rows.iter().find(|r| r.ext == "cr2").expect("cr2 行应在");
        assert_eq!(cr2_row.ids, vec!["raw"]);
        assert_eq!(cr2_row.labels[0], "raw 名");
        // rawpx 停用 → .raw 失去参数化身份，落回常规认领者（正是运行时的路由真相）。
        let decoders_off = vec![
            entry("raw", "decoders/raw.dll", &["cr2", "raw"], false, true),
            entry("rawpx", "decoders/rawpx.dll", &["raw"], true, false),
        ];
        let rows = ext_rows_from(&decoders_off, &[], &BTreeMap::new());
        let raw_row = rows.iter().find(|r| r.ext == "raw").expect("raw 行应在");
        assert_eq!(raw_row.ids, vec!["raw"], "rawpx 停用后 .raw 应回到常规路由");
        assert_eq!(raw_row.labels[0], "raw 名");
    }
}
