//! 「快捷键」页：单键捕获框与两组鼠标行为。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
use super::*;

/// 单个快捷键「捕获框」——视觉正本逐项对齐 CurSeen `card::hotkey_box_el`
/// （参考项目）：
///
/// - stack + label 两轴居中（框内文字任何长度都居中，不顶左上角）
/// - 固定高 `CONTROL_H`(30)、宽 `CONTROL_GROUP_W`(200)：放进 [`option_row`] 后
///   左右缘与同列下拉/输入框两条竖线都对齐（2026-10-13 用户口径「足够宽、左右都对齐」）
/// - 底 `CARD_HEADER_BG`、描边 `BORDER` 1px、圆角 `CORNER_CONTROL`(6)
///   ——与下拉/输入框同层级的「输入类控件」外观，不写字面量
/// - 捕获中只换文案「按下按键…」（CurSeen 同款，不另加高亮底色）
///
/// `on_blur` = 失焦即放弃捕获（windui 补丁 094 同款）：点了框又点别处，
/// 框里不再挂着「按下按键…」等人按键；判「目标仍是本框」才清，避免把
/// 新框刚置上的捕获态一并打掉（点另一个捕获框时旧框 blur 可能晚触发）。
pub fn hotkey_box(
    text: Signal<String>,
    on_click: impl FnMut(&mut EventCtx) + 'static,
    on_blur: impl FnMut(&mut EventCtx) + 'static,
) -> Element {
    Element::stack()
        .child(
            Element::label_signal(text)
                .font_size(FONT_MD)
                .fg(to_color(TEXT))
                // 框内文字居中：stack（Layout::Frame）按子元素自己的 align 摆放，
                // 缺省 Start 会顶在左上角——这层与下面 stack 自己那个 align 各管一层。
                .align(Align::Center),
        )
        .width(CONTROL_GROUP_W)
        .height(CONTROL_H)
        .bg(to_color(CARD_HEADER_BG))
        .border(to_color(BORDER), 1)
        .corner(CORNER_CONTROL)
        .align(Align::Center)
        .clickable()
        .on_click(on_click)
        .on_blur(on_blur)
}

/// 把「配置 + 捕获态」推到快捷键页各行的显示文本信号（值没变不 set，避免无谓
/// 重绘）。静态树只建一次，所有改状态的地方——点框、失焦、捕获写回、恢复默认
/// ——都必须调它，否则框里的字停在旧值。
pub fn sync_hotkey_texts(config: &Config, capturing: Option<usize>, texts: &[Signal<String>]) {
    for (i, a) in Action::ALL.iter().enumerate() {
        let ai = a.index();
        let t = if capturing == Some(ai) {
            // CurSeen 同款捕获中文案。
            "按下按键…".to_string()
        } else {
            // 单键：有则显示，无则占位（CurSeen「（空）」同款）。
            config.hotkeys.bindings[ai]
                .first()
                .map(|b| b.pretty())
                .unwrap_or_else(|| "（空）".to_string())
        };
        if texts[i].get() != t {
            texts[i].set(t);
        }
    }
}

/// 快捷键子系统单例的内容：（捕获态，每行捕获框的显示文本）。
pub type ShortcutUi = (Signal<Option<usize>>, Vec<Signal<String>>);

thread_local! {
    /// 设置窗快捷键子系统的进程级单例：捕获态 + 每行文本，建一次终身复用（见
    /// [`shortcuts_ui`]）。
    static SHORTCUT_UI: RefCell<Option<ShortcutUi>> = const { RefCell::new(None) };
}

/// 取单例（首用即建）。信号必须**跨开窗同一只**：`Window::content` 是 FnOnce、
/// `.single("settings")` 已开时不再重建树——已开的窗里只有这对「初代」信号能把
/// on_shortcut 的捕获结果送进 content 那棵树；每开一次窗就换一批新信号，等于
/// 让旧窗的显示与捕获双双失联，还每开一次漏一串槽位。
pub fn shortcuts_ui() -> ShortcutUi {
    SHORTCUT_UI.with(|cell| {
        let mut b = cell.borrow_mut();
        if b.is_none() {
            *b = Some((
                signal(None),
                Action::ALL.iter().map(|_| signal(String::new())).collect(),
            ));
        }
        // 上一句保证这里必是 Some；仍不用 `unwrap`（§八.1 红线，hygiene 的 panic_source 查）。
        // 真取不到只能是别处把 SHORTCUT_UI 清了——宁可现造一对没人监听的信号，
        // 也不在设置页里 panic（那是整个查看器没了）。
        match b.as_ref() {
            Some((cap, texts)) => (*cap, texts.clone()),
            None => (signal(None), Vec::new()),
        }
    })
}

/// 插件快捷键子系统：捕获态（正在等按键的插件 id）+ 每行显示文本。
pub struct PluginHkUi {
    pub capturing: Signal<Option<String>>,
    /// 行序：插件 id 与它的 hotkey 设置键。
    pub rows: Vec<(String, String)>,
    /// 与 `rows` 同序的显示文本。
    pub texts: Vec<Signal<String>>,
}

thread_local! {
    static PLUGIN_HK: RefCell<Option<PluginHkUi>> = const { RefCell::new(None) };
}

/// 取插件快捷键子系统；**没有就返回 `None`，绝不现场创建**。
///
/// ★ 为什么不能在窗口内容里创建：windui 给每扇窗口的内容构建开一个 `SignalScope`，
///   窗口关掉整批回收（`app/mod.rs` 的 `scope.collect(...)`）。信号若在内容闭包里
///   首次创建，会归那扇窗所有——关一次设置窗，单例里留的就是**死句柄**，下一次
///   `get()` 直接 panic（2026-10-31 真机秒退就是这一格）。
///   所以创建一律发生在 [`plugin_hk_bootstrap`]，它在 `open_settings` 里、
///   `ctx.open_window` **之前**被调用。
pub fn plugin_hk_ui() -> Option<PluginHkUi> {
    PLUGIN_HK.with(|cell| {
        cell.borrow().as_ref().map(|u| PluginHkUi {
            capturing: u.capturing,
            rows: u.rows.clone(),
            texts: u.texts.clone(),
        })
    })
}

/// 在**任何窗口内容构建之外**把单例建好（并首次对齐行集合）。
///
/// 幂等：已经建过就只刷新行集合与显示文本。
pub fn plugin_hk_bootstrap(config: &Config) {
    let items = crate::extensions::plugin_hotkey_items(config);
    let keys: Vec<(String, String)> = items
        .iter()
        .map(|(id, _, k, _)| (id.clone(), k.clone()))
        .collect();
    let text_count = PLUGIN_HK.with(|cell| {
        let mut b = cell.borrow_mut();
        let ui = match b.as_mut() {
            Some(u) => u,
            None => {
                *b = Some(PluginHkUi {
                    capturing: signal(None),
                    rows: Vec::new(),
                    texts: Vec::new(),
                });
                match b.as_mut() {
                    Some(u) => u,
                    None => return 0,
                }
            }
        };
        if ui.rows != keys || ui.texts.len() != keys.len() {
            ui.rows = keys;
            ui.texts = items.iter().map(|_| signal(String::new())).collect();
        }
        ui.texts.len()
    });
    let _ = text_count;
    sync_plugin_hotkey_texts(config, None);
}

/// 把设置区里的当前值推到各行显示文本（值没变不 set）。捕获中的那一行不动。
pub fn sync_plugin_hotkey_texts(config: &Config, capturing: Option<&str>) {
    let Some(ui) = plugin_hk_ui() else {
        return;
    };
    let items = crate::extensions::plugin_hotkey_items(config);
    for (i, (id, _, _, raw)) in items.iter().enumerate() {
        let Some(text) = ui.texts.get(i) else {
            continue;
        };
        let t = if capturing == Some(id.as_str()) {
            "按下按键…".to_string()
        } else {
            Hotkey::from_spec(raw)
                .map(|h| h.pretty())
                .unwrap_or_else(|| "（空）".to_string())
        };
        if text.get() != t {
            text.set(t);
        }
    }
}

/// Shortcuts 页：两组快捷键，**各占一张可折叠卡片**——「键盘快捷键」（每个功能
/// **一个捕获框、至多一把键**）与「鼠标快捷键」（双击 / 滚轮 / Ctrl+滚轮，自选项页
/// 迁入）。卡片与「解码器」页那三张同款（[`section_card`]）。
///
/// ★ 2026-10-26 由「横线 + 红字段名」分段改成卡片（用户口径「快捷键部分也采用折叠
///   卡片的形式，刚好两组」）：两组本来就是两件事，卡片让任一组能整组收起，页也不会
///   一打开就摊出 29 行键位。段名不再另写一行红字——卡片标题条就是段名，`group_label`
///   因此退场（它在全项目只剩这一处调用点）。
///
/// 单键化（2026-10-13 用户口径「只允许单个」）：不再有 ±绑定位杂件，行版式走
/// [`option_row`]——框左缘 = 控件列左缘、宽 `CONTROL_GROUP_W` 右缘也对齐，
/// 与其它页两列排版完全一致。点框即清空现值再进捕获（CurSeen `hotkey_row` 同款），
/// 失焦/Esc 留「（空）」，这也是唯一的解绑通道。
///
/// 捕获逻辑在窗口级 `Window::on_shortcut`（见 `open_settings`）：焦点在本窗口时，
/// 下一键被 [`Hotkey::from_event`] 抓取写回配置；与别的功能撞键时由
/// [`crate::core::hotkey::Hotkeys::set_binding`] 强制唯一（vacate 旧主）。
///
/// `capturing`：当前正在捕获的动作下标；`texts`：每行框里的显示文本信号——
/// windui 的 `content` 是 FnOnce、静态树只建一次（app/mod.rs `content` 文档），
/// 改键/捕获/恢复默认只有写信号 + `label_signal` 才到得了 UI，状态变化处
/// 统一调 [`sync_hotkey_texts`] 把「配置 + 捕获态」推平到各行文本。
// ini 键：hotkeys（快捷键绑定，设置页「快捷键」可改、可重映射、唯一性由 Hotkeys::set_binding 保证）
pub fn shortcuts_page(
    h: &Handles,
    config: Rc<RefCell<Config>>,
    capturing: Signal<Option<usize>>,
    texts: Vec<Signal<String>>,
) -> Element {
    // 建树时从配置回灌一遍文本（开窗外配置被改过也能从正确值起显）。
    sync_hotkey_texts(&config.borrow(), capturing.get(), &texts);

    let mut rows: Vec<Element> = Vec::with_capacity(Action::COUNT);
    for (i, a) in Action::ALL.into_iter().enumerate() {
        let ai = a.index();
        let cfg_click = config.clone();
        let texts_click = texts.clone();
        let cfg_blur = config.clone();
        let texts_blur = texts.clone();
        let box_el = hotkey_box(
            texts[i],
            move |_ctx| {
                // 点击即清空现值 + 进入捕获（CurSeen hotkey_row 同款）：捕获中途
                // 失焦（点别处）留下「（空）」——单键化后没有 ✕，这就是解绑通道。
                // Esc 不再当取消用，它是一个正常可绑定主键。
                cfg_click.borrow_mut().hotkeys.clear_binding(a);
                crate::core::config::mark_dirty();
                capturing.set(Some(ai));
                sync_hotkey_texts(&cfg_click.borrow(), capturing.get(), &texts_click);
            },
            move |_ctx| {
                // 失焦 = 放弃捕获（仅当捕获目标仍是本框）。
                if capturing.get() == Some(ai) {
                    capturing.set(None);
                    sync_hotkey_texts(&cfg_blur.borrow(), capturing.get(), &texts_blur);
                }
            },
        );
        // 与其它页同一条两列版式：标签 COL1_W + 控件列，左右缘同竖线。
        rows.push(option_row(a.name(), box_el));
    }

    // 鼠标行为两行（自「选项」页迁来，与键盘快捷键同一标签列对齐）。
    let mouse_rows = [
        choice_row(
            "鼠标左键双击",
            DblClick::ALL
                .iter()
                .map(|d| d.label().to_string())
                .collect(),
            h.dcb,
        ),
        choice_row(
            "鼠标滚轮",
            Wheel::ALL.iter().map(|w| w.label().to_string()).collect(),
            h.wheel,
        ),
        choice_row(
            "Ctrl+鼠标滚轮",
            CtrlWheel::ALL
                .iter()
                .map(|w| w.label().to_string())
                .collect(),
            h.ctrl_wheel,
        ),
    ];

    // 恢复默认（**整页**）：一键把本页全部复位——键盘快捷键那一批 + 鼠标三档下拉
    // （双击 / 滚轮 / Ctrl+滚轮）一起回到出厂值。范围与「页面顶部」这个位置相称。
    // 出厂值单一来源取 `Config::default()`，将来默认值调整这里自动跟随，不写死数字。
    // 必须用 Element::button：label 上叠 clickable() 违反 windui 契约（clickable 仅限
    // 容器），release 下会把 Label 静默替换成空壳、debug 下断言 panic。
    let h = *h;
    let cfg_def = config.clone();
    let cap_def = capturing;
    let texts_def = texts.clone();
    let reset = btn("恢复默认").on_click(move |_ctx| {
        let def = Config::default();
        {
            let mut c = cfg_def.borrow_mut();
            c.hotkeys.reset_defaults();
            c.dbl_click = def.dbl_click;
            c.wheel = def.wheel;
            c.ctrl_wheel = def.ctrl_wheel;
        }
        crate::core::config::mark_dirty();
        // 下拉框选中项由信号驱动：写信号即刷 UI；config 也同步改掉，sync 时二者相等、
        // 不会重复标脏。
        h.dcb.set(def.dbl_click.index() as usize);
        h.wheel.set(def.wheel.index() as usize);
        h.ctrl_wheel.set(def.ctrl_wheel.index() as usize);
        sync_hotkey_texts(&cfg_def.borrow(), cap_def.get(), &texts_def);
    });

    // 「恢复默认」行：与页面其余内容行同高（ROW_H）、垂直居中、**左对齐**——左缘
    // 与卡片标题文字同一条竖线（行自己的 16 + 卡片标题条的 16），不垫标签列占位。
    let reset_row = Element::row()
        .height(ROW_H)
        .cross(Align::Center)
        .padding_xy(16, 0)
        .child(reset);

    // 「插件快捷键」：行名 `插件名 > 设置项名`（2026-10-31 用户口径，参考 CurSeen 的
    // 分模块快捷键入口）。值只存在各插件自己的 settings.json —— 两处显示的都是它。
    let plugin_items = crate::extensions::plugin_hotkey_items(&config.borrow());
    // 信号由 `plugin_hk_bootstrap`（开窗前）建好；这里只取，不在窗口内容里造。
    let plugin_texts = plugin_hk_ui().map(|u| u.texts).unwrap_or_default();
    let mut plugin_rows: Vec<Element> = Vec::new();
    for (i, (id, name, _key, _)) in plugin_items.iter().enumerate() {
        let label = format!("{name} > 快捷键");
        let text = match plugin_texts.get(i) {
            Some(t) => *t,
            None => continue,
        };
        let id_click = id.clone();
        let text_click = text;
        let id_blur = id.clone();
        let cfg_blur = config.clone();
        let box_el = hotkey_box(
            text,
            move |_ctx| {
                if let Some(ui) = plugin_hk_ui() {
                    ui.capturing.set(Some(id_click.clone()));
                }
                text_click.set("按下按键…".to_string());
            },
            move |_ctx| {
                if let Some(ui) = plugin_hk_ui() {
                    if ui.capturing.get().as_deref() == Some(id_blur.as_str()) {
                        ui.capturing.set(None);
                        sync_plugin_hotkey_texts(&cfg_blur.borrow(), None);
                    }
                }
            },
        );
        plugin_rows.push(option_row(&label, box_el));
    }
    let cap_now = plugin_hk_ui().and_then(|u| u.capturing.get());
    sync_plugin_hotkey_texts(&config.borrow(), cap_now.as_deref());

    Element::scroll()
        .fill()
        .padding_xy(16, PAGE_TOP_MARGIN)
        .child(
            Element::col()
                .fill()
                .spacing(GAP)
                .child(section_card(
                    "键盘快捷键",
                    h.card_keys,
                    Element::col().fill().spacing(0).children(rows),
                ))
                .child(section_card(
                    "鼠标快捷键",
                    h.card_mouse,
                    Element::col().fill().spacing(0).children(mouse_rows),
                ))
                .child(section_card(
                    "插件快捷键",
                    h.card_plugins,
                    Element::col().fill().spacing(0).children(plugin_rows),
                ))
                // 「恢复默认」**压在页末**（2026-10-26 用户口径：不要放在顶部）。
                // 它复位的范围仍是**整页**（键盘绑定 + 鼠标三档一起回出厂值），
                // 摆在两张卡之后——页首整块留给卡片本身，不让人还没看到内容就先撞上
                // 一颗会清掉全部改动的按钮；而"整页复位"摆在一页的末尾，范围与位置
                // 同样是相称的（它下面是这一页的尽头，不会看着像只管最后一张卡）。
                // 卡↔卡、末卡↔本行的间隔都走 `spacing(GAP)`：全页同一节奏。
                .child(reset_row),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单例信号必须活在**窗口作用域之外**。
    ///
    /// 2026-10-31 真机秒退：`plugin_hk_ui()` 第一次调用发生在设置页的内容闭包里，
    /// 而 windui 给每扇窗的内容构建开一个 `SignalScope`、关窗整批回收 —— 单例里于是
    /// 留下死句柄，下一次 `get()` 直接 panic（`signal.rs` 那句"槽位已被回收"）。
    /// 修法是"开窗之前 bootstrap"；这条用例钉的就是"scope 建了又毁，句柄仍能读写"。
    #[test]
    fn plugin_hotkey_singleton_survives_a_disposed_window_scope() {
        let cfg = Config::default();
        plugin_hk_bootstrap(&cfg);
        let ui = plugin_hk_ui().expect("bootstrap 之后单例必须在");
        let mut scope = windui::signal::SignalScope::new();
        scope.collect(|| {
            let _ = signal(1u8);
        });
        scope.dispose();
        // 句柄仍是活的：写进去还得读得出来（死句柄会在这里 panic）。
        ui.capturing.set(Some("checkup".to_string()));
        assert_eq!(ui.capturing.get().as_deref(), Some("checkup"));
        ui.capturing.set(None);
    }

    /// 快捷键页回归：**恰好三张卡、正文高度不塌、折叠是活开关、整页真能滚**。
    ///
    /// 四条判据各自盯一类旧病（与 `decoders.rs` 那三张卡同一套写法）：
    /// ① 三张卡的标题条都在且都是 `CARD_HEADER_H`（卡片本体塌 0 = 整页看不见）；
    /// ② 竖向节奏只有一格 `GAP`（页四周 / 行↔卡 / 卡↔卡 / 卡头↔正文）；
    /// ③ 展开信号真能收起正文（点了没反应是最容易漏的一类 UI bug）；
    /// ④ 29 行键位必定溢出视口，**真的派发滚轮**验偏移量（布局对但事件没人处理的老病）。
    #[test]
    fn shortcuts_page_three_cards_survive_layout_and_collapse() {
        use windui::core::Tree;
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
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
            drag_image: sig(false),
            show_toolbar_buttons: sig(true),
            filmstrip: sig(false),
            allow_multiple: sig(false),
            opacity: signal(0.75),
            opacity_label: signal(String::new()),
            tab: signal(TAB_SHORTCUTS),
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
        let texts: Vec<Signal<String>> =
            Action::ALL.iter().map(|_| signal(String::new())).collect();

        let page = shortcuts_page(&h, config, signal(None), texts);
        // 与真窗同形：88px 导航条 + stack + 页（卡在页的 scroll 里）。
        let ui = Element::row()
            .fill()
            .child(Element::leaf().width(88).height_match())
            .child(Element::stack().fill().weight(1.0).child(page.fill()));
        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 480);
        tree.layout_root(size, &mut NullTextEngine);

        let stack = tree.get(root).expect("row 根").children[1];
        let page_id = tree.get(stack).expect("stack").children[0];
        let col_id = tree.get(page_id).expect("页面 scroll").children[0];
        let kids = tree.get(col_id).expect("内容列").children.clone();
        assert_eq!(
            kids.len(),
            4,
            "快捷键页应为「键盘卡 + 鼠标卡 + 插件卡 + 恢复默认行」四段（恢复默认在页末）"
        );
        // 「恢复默认」必须是**最后一段**：它在页末，不在页首（2026-10-26 用户口径）。
        // 顺带对齐：它那一行与卡片同一条左缘（两者各自的 padding 都是 16）。
        assert_eq!(
            tree.abs_bounds(kids[3]).x,
            tree.abs_bounds(kids[0]).x,
            "「恢复默认」行应与卡片同一条左缘"
        );

        // ① 三张卡的标题条都在、都有高度。
        for i in [0usize, 1, 2] {
            let card = tree.get(kids[i]).expect("卡片");
            assert!(card.bounds.h > 0, "第 {i} 张卡高度塌成 0");
            let header = card.children[0];
            assert_eq!(
                tree.get(header).expect("标题条").bounds.h,
                CARD_HEADER_H,
                "第 {i} 张卡的标题条高度应为 CARD_HEADER_H"
            );
        }

        // ② 竖向节奏统一：卡↔卡、末卡↔恢复默认行、卡头↔正文首行，全部同一格 GAP。
        let gap_card_card = tree.abs_bounds(kids[1]).y - tree.abs_bounds(kids[0]).bottom();
        assert_eq!(gap_card_card, GAP, "卡↔卡应为 GAP：实测 {gap_card_card}");
        let gap_card_card2 = tree.abs_bounds(kids[2]).y - tree.abs_bounds(kids[1]).bottom();
        assert_eq!(gap_card_card2, GAP, "卡↔卡应为 GAP：实测 {gap_card_card2}");
        let gap_card_reset = tree.abs_bounds(kids[3]).y - tree.abs_bounds(kids[2]).bottom();
        assert_eq!(
            gap_card_reset, GAP,
            "末张卡↔「恢复默认」行应为 GAP：实测 {gap_card_reset}"
        );
        // 量的是**正文首行**的顶缘，不是正文节点的顶缘：GAP 垫在正文节点自己的 padding 上，
        // 量节点恒得 0（decoders 那条用例第一版就栽在这里）。
        let header_bottom = tree
            .abs_bounds(tree.get(kids[0]).expect("卡").children[0])
            .bottom();
        let keys_body = tree.get(kids[0]).expect("卡").children[1];
        let keys_first = tree.get(keys_body).expect("正文").children[0];
        let gap_header_body = tree.abs_bounds(keys_first).y - header_bottom;
        assert_eq!(
            gap_header_body, GAP,
            "卡片标题条↔正文首行应为 GAP：实测 {gap_header_body}"
        );

        // ③ 正文行数 = 功能数，且真的有高度（host/col 在无限高度下量不出内容就塌了）。
        let keys_kids = tree.get(keys_body).expect("键盘正文").children.clone();
        assert_eq!(keys_kids.len(), Action::COUNT, "键盘卡应为每个功能一行");
        let mouse_body = tree.get(kids[1]).expect("卡").children[1];
        assert_eq!(
            tree.get(mouse_body).expect("鼠标正文").children.len(),
            3,
            "鼠标卡应为「双击 / 滚轮 / Ctrl+滚轮」三行"
        );
        assert!(
            tree.get(keys_body).expect("键盘正文").bounds.h > ROW_H,
            "键盘卡正文只剩一行高：列容器在无限高度下没量出内容"
        );

        // ④ 折叠是活开关：收起 → **卡片只剩标题条那么高**（正文不再占布局），展开 → 回到原高。
        //
        // ★ 判据落在**卡片**而不是正文节点上：windui 的 `visible_when(false)` 只是把该节点
        //   从 `visible_children` 里摘掉（父测量不再为它留位置），**并不把它的 `bounds`
        //   归零**——`arrange` 对不可见节点直接 return，上一帧量到的高度原样留着
        //   （`core.rs` 的 `arrange`）。所以"收起后正文 bounds.h == 0"只在**从没展开过**
        //   的节点上成立（decoders 那条用例的「可下载」卡正是这种），这里是假判据。
        let expanded_card_h = tree.get(kids[0]).expect("卡").bounds.h;
        let expanded_content_h = tree.get(page_id).expect("页面").content_h;
        h.card_keys.set(false);
        tree.layout_root(size, &mut NullTextEngine);
        assert_eq!(
            tree.get(kids[0]).expect("卡").bounds.h,
            CARD_HEADER_H,
            "收起「键盘快捷键」后卡片应只剩标题条那么高（正文仍在占布局 = 折叠没生效）"
        );
        let collapsed_content_h = tree.get(page_id).expect("页面").content_h;
        assert!(
            collapsed_content_h < expanded_content_h,
            "收起后整页内容高度应下降：{collapsed_content_h} → {expanded_content_h}"
        );
        h.card_keys.set(true);
        tree.layout_root(size, &mut NullTextEngine);
        assert_eq!(
            tree.get(kids[0]).expect("卡").bounds.h,
            expanded_card_h,
            "再点「键盘快捷键」应展开回原高：折叠是单向的？"
        );
        assert!(
            tree.get(keys_body).expect("键盘正文").bounds.h > ROW_H,
            "正文高度只剩一行：列容器在无限高度下没量出内容"
        );

        // ⑤ 整页滚动：29 行键位必定溢出，滚轮必须被消费且 scroll_y 真的动。
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
}
