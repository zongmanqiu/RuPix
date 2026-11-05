//! 「插件」页：已安装 / 可下载两卡，版式与行为对齐「解码器」页。
//!
//! - 已安装：扫描 `<exe>/plugins/*/manifest.json`，开关写 `extensions_disabled`。
//! - 可下载：读 `<exe>/docs/plugins.json`，进页零网络；点「刷新」才按来源后台拉取
//!   远程 `plugins.json`。下载走 release ZIP，整目录替换安装（不校验清单指纹，
//!   2026-11-02 用户口径：官方产物只认名称与版本号）。
//!
//! 与解码器的区别只有一个：插件不参与图片解码，因此没有「首选」卡。
use super::*;
use crate::core::ext::Capability;
use crate::extensions::remote::{self, InstallState};
use crate::extensions::{self, AvailablePlugin, Extension};

/// 插件页的全部信号。作为一个整体挂在 [`Handles`] 上，窗口定时器只碰这一处。
#[derive(Clone, Copy)]
pub struct PluginUi {
    pub installed: Signal<Vec<Extension>>,
    pub available: Signal<Vec<AvailablePlugin>>,
    pub installed_filter: Signal<String>,
    pub available_filter: Signal<String>,
    pub installed_view: Signal<Vec<Extension>>,
    pub available_view: Signal<Vec<AvailablePlugin>>,
    pub installed_count: Signal<String>,
    pub available_count: Signal<String>,
    pub source: Signal<usize>,
    pub state: Signal<String>,
    pub status: Signal<String>,
    pub progress: Signal<Option<(String, u8)>>,
}

impl PluginUi {
    pub fn new(config: &Config) -> Self {
        let installed = signal(Vec::new());
        let available = signal(Vec::new());
        let installed_filter = signal(String::new());
        let available_filter = signal(String::new());
        let installed_view = installed_filter.map(move |q| filter_installed(&installed.get(), q));
        let available_view = available_filter.map(move |q| filter_available(&available.get(), q));
        let installed_count =
            installed_view.map(move |v| count_label(v.len(), installed.get().len(), "插件"));
        let available_count =
            available_view.map(move |v| count_label(v.len(), available.get().len(), "可下载"));
        let state = signal(String::new());
        let status = available_filter.map(move |q| {
            let base = state.get();
            let is_summary = base.chars().next().is_some_and(|c| c.is_ascii_digit());
            if q.trim().is_empty() || !is_summary {
                base
            } else {
                let n = filter_available(&available.get(), q).len();
                format!("{base} · 匹配 {n} 个")
            }
        });
        let ui = Self {
            installed,
            available,
            installed_filter,
            available_filter,
            installed_view,
            available_view,
            installed_count,
            available_count,
            source: signal(config.download_source.ui_index()),
            state,
            status,
            progress: signal(None),
        };
        ui.reload_config(config);
        ui
    }

    /// 重新扫描已安装目录并读本地清单。**不联网**。
    ///
    /// ★ 进门先让缓存失效：`load_installed` 走的是带缓存的 `installed()`，不失效就等于
    /// "重扫"了同一份旧快照——卸载完那一行还挂着、装完看不到新行，都得靠关窗重开。
    pub fn reload(&self, config: &Rc<RefCell<Config>>) {
        extensions::invalidate_installed_cache();
        let installed = load_installed(config);
        self.installed.set(installed.clone());
        match remote::load_local(&installed) {
            Ok(mut list) => {
                mark_installed(&mut list, &installed);
                extensions::enrich_from_local(&mut list, &installed);
                self.available.set(list);
                self.recompute_state();
            }
            Err(e) => {
                self.set_state(e);
                self.available.set(Vec::new());
            }
        }
        self.bump();
    }

    fn reload_config(&self, config: &Config) {
        let disabled = config.extensions_disabled.clone();
        let isolated = config.extensions_isolated.clone();
        let installed = extensions::discover(&disabled, &isolated);
        self.installed.set(installed.clone());
        match remote::load_local(&installed) {
            Ok(mut list) => {
                mark_installed(&mut list, &installed);
                extensions::enrich_from_local(&mut list, &installed);
                self.available.set(list);
                self.recompute_state();
            }
            Err(e) => self.set_state(e),
        }
        self.bump();
    }

    /// 当前内存清单的摘要（切来源 / 安装完成后重算）。
    pub fn recompute_state(&self) {
        let src = SourceId::from_ui_index(self.source.get());
        let state = summary(&self.available.get(), &self.installed.get(), src);
        self.set_state(state);
    }

    /// 起后台刷新；结果由设置窗 250 ms 定时器经 [`Self::poll`] 取走。
    pub fn refresh(&self, config: &Config) {
        let src = config.download_source;
        self.source.set(src.ui_index());
        match remote::start_refresh_for(config) {
            Ok(()) => self.set_state("更新中…".to_string()),
            Err(e) => {
                log::warn!("插件清单更新未能开始：{e}");
                self.set_state(e.ui_text().to_string());
            }
        }
    }

    /// 设置窗定时器只看刷新结果和下载进度；安装收尾不依赖窗口寿命。
    pub fn poll(&self) -> bool {
        let mut changed = false;
        if let Some(out) = remote::poll_refresh() {
            changed = true;
            match out.result {
                Ok(mut list) => {
                    let installed = self.installed.get();
                    mark_installed(&mut list, &installed);
                    extensions::enrich_from_local(&mut list, &installed);
                    self.available.set(list);
                    self.bump();
                    let base = summary(
                        &self.available.get(),
                        &self.installed.get(),
                        SourceId::from_ui_index(self.source.get()),
                    );
                    self.set_state(format!("{base} · 已更新"));
                    log::info!("插件清单已从 {} 刷新", out.src.label());
                }
                Err(e) => {
                    log::warn!("插件清单刷新失败（{}）：{e}", out.src.label());
                    self.set_state(remote::catalog_error_text().to_string());
                }
            }
        }
        if let Some(p) = extensions::install::progress() {
            let pct = p.percent();
            let item = (p.id, pct);
            if self.progress.get() != Some(item.clone()) {
                self.progress.set(Some(item));
                changed = true;
            }
        } else if self.progress.get().is_some() {
            self.progress.set(None);
            changed = true;
        }
        changed
    }

    /// 切来源只换地址列，不联网；重画按钮即可。
    pub fn source_changed(&self) {
        self.bump();
        self.recompute_state();
    }

    fn set_state(&self, text: String) {
        self.state.set(text);
        let q = self.available_filter.get();
        self.available_filter.set(q);
    }

    fn bump(&self) {
        let q = self.installed_filter.get();
        self.installed_filter.set(q);
        let q = self.available_filter.get();
        self.available_filter.set(q);
    }
}

pub(super) fn plugin_install_done(
    ctx: &mut EventCtx,
    config: &Rc<RefCell<Config>>,
    ui: Option<PluginUi>,
    out: extensions::install::Outcome,
) {
    if let Some(ui) = ui {
        ui.progress.set(None);
    }
    match out.result {
        Ok(()) => {
            {
                let mut c = config.borrow_mut();
                c.extensions_disabled.retain(|x| x != &out.id);
            }
            crate::core::config::mark_dirty();
            // 统一的底层刷新：插件缓存失效（下面的 reload 才会读盘），顺带让解码线程重扫。
            // 插件不改变"谁能解哪个后缀"，所以不碰两张解码器卡的信号——那是解码器安装的事。
            super::refresh_catalogs();
            if let Some(ui) = ui {
                ui.reload(config);
            }
            log::info!("已安装插件「{}」", out.name);
            ctx.toast(format!("已安装插件：{}", out.name));
        }
        Err(msg) => {
            log::warn!("插件下载失败（{}）：{msg}", out.id);
            if let Some(ui) = ui {
                ui.set_state("下载失败".to_string());
            }
            ctx.toast_err("下载失败".to_string());
        }
    }
    ctx.mark_dirty();
}

fn mark_installed(list: &mut [AvailablePlugin], installed: &[Extension]) {
    for e in list {
        e.mark_installed(&installed_ids(installed));
    }
}

/// 「插件」页。
pub fn extensions_page(config: &Rc<RefCell<Config>>, ui: &PluginUi) -> Element {
    let ui = *ui;
    let card_installed = signal(true);
    let card_available = signal(true);
    Element::scroll()
        .fill()
        .padding_xy(16, PAGE_TOP_MARGIN)
        .child(
            Element::col()
                .fill()
                .spacing(GAP)
                .child(section_card(
                    "已安装",
                    card_installed,
                    installed_body(config, ui),
                ))
                .child(section_card(
                    "可下载",
                    card_available,
                    available_body(config, ui),
                )),
        )
}

fn load_installed(config: &Rc<RefCell<Config>>) -> Vec<Extension> {
    extensions::discover_configured(&config.borrow())
}

fn installed_ids(installed: &[Extension]) -> Vec<String> {
    installed.iter().map(|e| e.manifest.id.clone()).collect()
}

fn installed_body(config: &Rc<RefCell<Config>>, ui: PluginUi) -> Element {
    let refresh = {
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            ui.reload(&config);
            ctx.mark_dirty();
        }
    };
    let open_dir = |ctx: &mut EventCtx| match extensions::open_plugin_dir() {
        Ok(()) => {}
        Err(e) => ctx.toast_err(e),
    };
    let rows = Element::host_signal(ui.installed_view, {
        let config = config.clone();
        move |ext: Extension| installed_row(ext, config.clone(), ui)
    });

    Element::col()
        .fill()
        .spacing(0)
        .child(
            Element::row()
                .fill()
                .height(ROW_H)
                .cross(Align::Center)
                .padding_xy(16, 0)
                .child(
                    search_box(ui.installed_filter, "搜索名称或后缀")
                        .width_match()
                        .height(CONTROL_H),
                ),
        )
        .child(Element::leaf().height(GAP))
        .child(
            Element::row()
                .fill()
                .spacing(8)
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label_signal(ui.installed_count)
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_ACCENT))
                        .weight(1.0)
                        .max_lines(1)
                        .truncate(Truncate::End),
                )
                .child(btn("文件夹").on_click(open_dir))
                .child(btn("刷新").on_click(refresh)),
        )
        .child(Element::leaf().height(GAP))
        .child(rows)
        .child(
            Element::row()
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label("（没有匹配的插件）")
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_SECONDARY)),
                )
                .visible_when(move || {
                    !ui.installed_filter.get().trim().is_empty()
                        && ui.installed_view.get().is_empty()
                        && !ui.installed.get().is_empty()
                }),
        )
        .child(
            Element::row()
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label("（没有已安装插件）")
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_SECONDARY)),
                )
                .visible_when(move || ui.installed.get().is_empty()),
        )
}

fn available_body(config: &Rc<RefCell<Config>>, ui: PluginUi) -> Element {
    let refresh = {
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            ui.refresh(&config.borrow());
            let state = ui.state.get();
            if state.starts_with("清单异常") {
                ctx.toast_err(state);
            }
            ctx.mark_dirty();
        }
    };
    let rows = Element::host_signal(ui.available_view, {
        move |entry: AvailablePlugin| available_row(entry, ui)
    });

    Element::col()
        .fill()
        .spacing(0)
        .child(
            Element::row()
                .fill()
                .height(ROW_H)
                .cross(Align::Center)
                .padding_xy(16, 0)
                .child(
                    search_box(ui.available_filter, "搜索名称或后缀")
                        .width_match()
                        .height(CONTROL_H),
                ),
        )
        .child(Element::leaf().height(GAP))
        .child(
            Element::row()
                .fill()
                .spacing(8)
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label_signal(ui.status)
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
        .child(
            Element::row()
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label("（没有匹配的插件）")
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_SECONDARY)),
                )
                .visible_when(move || {
                    !ui.available_filter.get().trim().is_empty()
                        && ui.available_view.get().is_empty()
                }),
        )
        .child(
            Element::row()
                .cross(Align::Center)
                .height(ROW_H)
                .padding_xy(16, 0)
                .child(
                    Element::label("（没有可下载插件）")
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_SECONDARY)),
                )
                .visible_when(move || ui.available.get().is_empty()),
        )
}

fn installed_row(ext: Extension, config: Rc<RefCell<Config>>, ui: PluginUi) -> Element {
    let enabled = signal(ext.enabled);
    let show_info = {
        let ext = ext.clone();
        let config = config.clone();
        move |ctx: &mut EventCtx| open_installed_info(ctx, ext.clone(), ui, config.clone())
    };
    // 2026-10-29 用户口径：齿轮放在「!」**左侧**——「!」才能和「可下载」行同一条竖线。
    let mut info = Element::row().spacing(6).cross(Align::Center);
    if let Some(gear) = settings_gear(&ext, config.clone()) {
        info = info.child(gear);
    }
    info = info.child(info_btn().on_click(show_info));
    let (name, version) = (ext.manifest.name.clone(), ext.manifest.version.clone());
    row_shell(
        &name,
        &version,
        info,
        action_slot(enable_switch(&ext, config, enabled)),
    )
}

/// 行尾那颗「!」按钮（详情窗）。盒子钉死成 `ICON_BTN_W/H`——它是两颗小按钮的**标尺**，
/// 齿轮按它来（见 [`tokens::ICON_BTN_W`]）。两张卡（「已安装」/「可下载」）共用这一颗。
fn info_btn() -> Element {
    btn("!").width(ICON_BTN_W).height(ICON_BTN_H)
}

/// 设置齿轮按钮：Segoe MDL2 的设置字形（PUA 码位 U+E713）。
///
/// 盒子与 [`info_btn`] 同款——图标字形天然比正文字形宽一圈，不钉死就会宽出 9 px、
/// 矮 4 px（2026-11-05 用户报障）。图标在框里居中（windui 按钮居中画）。
fn gear_btn() -> Element {
    btn("\u{E713}")
        .font_family("Segoe MDL2 Assets")
        .width(ICON_BTN_W)
        .height(ICON_BTN_H)
}

/// 设置齿轮（仅 `settings_ui` 插件）。判据只看"现在能不能开"：停用 / 隔离 /
/// 宿主版本不足都置灰；悬停提示由该行「!」详情窗回答。
fn settings_gear(ext: &Extension, config: Rc<RefCell<Config>>) -> Option<Element> {
    // 两条路：声明了 `settings` schema ⇒ 宿主自己渲染一扇设置窗（[`crate::ui::plugin_settings`]）；
    // 只有 `settings_ui` 的老插件 ⇒ 仍以 `action=settings` 拉起插件、由插件画自己的界面。
    let has_schema = !ext.manifest.settings.is_empty();
    if !ext.manifest.settings_ui && !has_schema {
        return None;
    }
    let usable = ext.enabled && !ext.isolated && !ext.blocked;
    let ext = ext.clone();
    let b = gear_btn().disabled(!usable);
    let cfg_gear = config;
    Some(if usable {
        b.on_click(move |ctx: &mut EventCtx| {
            if !ext.manifest.settings.is_empty() {
                super::super::plugin_settings::show(ctx, &ext, &cfg_gear);
                return;
            }
            match extensions::start_settings(ext.clone()) {
                // 窗口是插件自己开的，屏幕上看得见，不弹提示。
                Ok(()) => {}
                Err(e) => ctx.toast_err(e),
            }
        })
    } else {
        b
    })
}

/// 启停开关（行尾动作槽里的那一颗）。
fn enable_switch(ext: &Extension, config: Rc<RefCell<Config>>, enabled: Signal<bool>) -> Element {
    let id = ext.manifest.id.clone();
    let change = move |on: bool| {
        let mut c = config.borrow_mut();
        if on {
            c.extensions_disabled.retain(|x| x != &id);
        } else if !c.extensions_disabled.iter().any(|x| x == &id) {
            c.extensions_disabled.push(id.clone());
        }
        crate::core::config::mark_dirty();
        // 启停变了：动态热键表（停用的插件热键不生效）要重新算。
        extensions::invalidate_installed_cache();
        log::info!("插件「{id}」已{}", if on { "启用" } else { "停用" });
    };
    Element::switch(enabled).on_switch_change(move |_ctx, on| change(on))
}

/// 已安装 / 可下载两张卡共用的插件信息字段（2026-10-29 用户裁定）：
/// 名称 / 标识 / 版本 / 协议版本 / 需要宿主 / 作者 / 许可证 / 发行时间 / 说明 / 大小。
///
/// ★ 两窗**同源同序**：以前各写一份，结果同一枚插件在两处显示的字段都不一样。
///   字段只在本结构 + [`plugin_info_rows`] 定义一次。
#[derive(Clone)]
struct PluginFacts {
    name: String,
    id: String,
    version: String,
    protocol: u32,
    min_host: String,
    author: String,
    license: String,
    released: String,
    description: String,
    /// 大小值（已安装 = 目录占用；可下载 = ZIP 包）。
    ///
    /// ★ 只写数字，不加括号注释（2026-11-05 用户裁定）：口径两窗本来就不同，
    ///   括号反而把两种写法摆到台面上；解码器信息窗的「大小」同样只有数字，
    ///   两套系统外观必须一致。
    size: String,
}

fn or_unknown(s: String) -> String {
    if s.is_empty() {
        "（未知）".to_string()
    } else {
        s
    }
}

fn or_none(s: String) -> String {
    if s.is_empty() {
        "（无）".to_string()
    } else {
        s
    }
}

fn host_ok(min_host: &str) -> bool {
    crate::extensions::host_supports(min_host)
}

fn plugin_info_rows(f: PluginFacts) -> Vec<Element> {
    let row = info_row;
    // 不只给一个数字（用户报障：只显示「1」等于没说）：写清插件说的版本
    // 与宿主支持的区间。
    let range = crate::core::ext::protocol_range_text();
    let protocol = if crate::core::ext::protocol_ok(f.protocol) {
        format!("v{}（宿主支持 {range}）", f.protocol)
    } else {
        format!("v{}（宿主支持 {range}，这一枚不能用）", f.protocol)
    };
    let min_host = if f.min_host.is_empty() {
        "（未标注）".to_string()
    } else {
        let current = crate::core::ext::HOST_VERSION;
        let verdict = if host_ok(&f.min_host) {
            ""
        } else {
            "，不满足"
        };
        format!("≥ {}（当前 {current}{verdict}）", f.min_host)
    };
    vec![
        row("名称", f.name),
        row("标识", f.id),
        row("版本", f.version),
        row("协议版本", protocol),
        row("需要宿主", min_host),
        row("作者", or_unknown(f.author)),
        row("许可证", or_unknown(f.license)),
        row("发行时间", or_unknown(f.released)),
        divider(),
        row("说明", or_none(f.description)),
        divider(),
        row("大小", f.size),
    ]
}

fn open_installed_info(
    ctx: &mut EventCtx,
    ext: Extension,
    ui: PluginUi,
    config: Rc<RefCell<Config>>,
) {
    let id = ext.manifest.id.clone();
    let title = format!("插件信息 · {}", ext.manifest.name);
    let facts = PluginFacts {
        name: ext.manifest.name.clone(),
        id: ext.manifest.id.clone(),
        version: ext.manifest.version.clone(),
        protocol: ext.manifest.protocol,
        min_host: ext.manifest.min_host.clone(),
        author: ext.manifest.author.clone(),
        license: ext.manifest.license.clone(),
        released: ext.manifest.released.clone(),
        description: ext.manifest.description.clone(),
        size: crate::extensions::dir_size(&ext.dir)
            .map(fmt_bytes)
            .unwrap_or_else(|| "（未知）".to_string()),
    };
    let iconfig = ext.clone();
    let isolated_now = ext.isolated;
    let cfg_unisolate = config.clone();
    let un_id = id.clone();
    let un_name = ext.manifest.name.clone();
    let unisolate = move |ctx: &mut EventCtx| {
        cfg_unisolate
            .borrow_mut()
            .extensions_isolated
            .retain(|x| x != &un_id);
        crate::core::config::mark_dirty();
        // 隔离状态变了：动态热键表要重算（被隔离的插件热键不生效）。
        extensions::invalidate_installed_cache();
        ui.reload(&cfg_unisolate);
        ctx.toast_ok(format!("已解除隔离：{un_name}"));
        ctx.request_close();
        ctx.mark_dirty();
    };
    let uninstall = move |ctx: &mut EventCtx| {
        match extensions::uninstall(&iconfig.manifest.id) {
            Ok(()) => {
                ui.reload(&config);
                ctx.request_close();
            }
            Err(e) => ctx.toast_err(e),
        }
        ctx.mark_dirty();
    };
    let key = format!("plugin-info-{id}");
    ctx.open_window(
        Window::new(title, 520, 360)
            // 信息窗客户区恒白（标题栏由系统绘制，不在其内）：与解码器信息窗同款。
            .bg(to_color(BG))
            .fit_content(240, 900)
            .resizable(true)
            .centered(true)
            .owned(true)
            .single(key)
            .content(move || {
                let mut items = plugin_info_rows(facts.clone());
                items.push(Element::leaf().height(10));
                items.push({
                    let mut parts = Vec::new();
                    if isolated_now {
                        parts.push(Element::button("解除隔离").on_click(unisolate));
                    }
                    parts.push(Element::flex_spacer());
                    parts.push(btn_danger("卸载").on_click(uninstall));
                    let mut r = Element::row().fill().cross(Align::Center).padding_xy(0, 8);
                    for p in parts {
                        r = r.child(p);
                    }
                    r
                });
                Element::scroll()
                    .fill()
                    .padding_xy(16, PAGE_TOP_MARGIN)
                    .child(Element::col().fill().spacing(0).children(items))
            }),
    );
}

fn open_available_info(ctx: &mut EventCtx, e: AvailablePlugin) {
    let facts = PluginFacts {
        name: e.name.clone(),
        id: e.id.clone(),
        version: e.version.clone(),
        protocol: e.protocol,
        min_host: e.min_host.clone(),
        author: e.author.clone(),
        license: e.license.clone(),
        released: e.released.clone(),
        description: e.description.clone(),
        size: fmt_bytes(e.size),
    };
    let key = format!("plugin-info-available-{}", e.id);
    ctx.open_window(
        Window::new(format!("插件信息 · {}", e.name), 520, 400)
            // 信息窗客户区恒白（标题栏由系统绘制，不在其内）：与解码器信息窗同款。
            .bg(to_color(BG))
            .fit_content(240, 900)
            .resizable(true)
            .centered(true)
            .owned(true)
            .single(key)
            .content(move || {
                Element::scroll()
                    .fill()
                    .padding_xy(16, PAGE_TOP_MARGIN)
                    .child(
                        Element::col()
                            .fill()
                            .spacing(0)
                            .children(plugin_info_rows(facts.clone())),
                    )
            }),
    );
}

fn available_row(entry: AvailablePlugin, ui: PluginUi) -> Element {
    let src = SourceId::from_ui_index(ui.source.get());
    let state = remote::install_state(&entry, &ui.installed.get());
    let local_ok = extensions::local_package_exists(&entry);
    let (label, tone) = action_of(&entry, state, src, local_ok);
    let info = {
        let entry = entry.clone();
        // 与「已安装」那张卡的「!」同一颗（`info_btn`）：两张卡的行尾按钮必须同款，
        // 否则同一列上下对不齐。
        info_btn().on_click(move |ctx| open_available_info(ctx, entry.clone()))
    };
    let caption = ui.progress.map({
        let id = entry.id.clone();
        move |p| row_caption(p, &id, label)
    });
    let not_downloading = {
        let id = entry.id.clone();
        move || !matches!(ui.progress.get(), Some((pid, _)) if pid == id)
    };
    let install = {
        let entry = entry.clone();
        move |ctx: &mut EventCtx| {
            let source = SourceId::from_ui_index(ui.source.get());
            match extensions::install::start(&entry, source) {
                Ok(()) => {
                    ui.progress.set(Some((entry.id.clone(), 0)));
                    ui.set_state("下载中…".to_string());
                    ctx.mark_dirty();
                }
                Err(e) => ctx.toast_err(e),
            }
        }
    };
    let action = btn_action(caption, tone);
    let row = Element::row()
        .fill()
        .cross(Align::Center)
        .spacing(8)
        .height(ROW_H)
        .padding_xy(16, 0)
        .child(
            Element::label(entry.name.clone())
                .font_size(FONT_MD)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(version_cell(&entry.version))
        .child(info);
    if tone == ActionTone::Idle {
        row.child(action_slot(action))
    } else {
        row.child(action_slot(
            action.enabled_when(not_downloading).on_click(install),
        ))
    }
}

fn action_of(
    e: &AvailablePlugin,
    state: InstallState,
    src: SourceId,
    local_ok: bool,
) -> (&'static str, ActionTone) {
    // 协议或宿主版本不满足：装了也跑不起来，按钮灰掉（原因在「!」信息窗）。
    if !e.compatible() {
        return ("下载", ActionTone::Idle);
    }
    if !e.downloadable_for(src) && !local_ok {
        return ("下载", ActionTone::Idle);
    }
    match state {
        InstallState::Upgradable => ("升级", ActionTone::Warning),
        InstallState::Absent => ("下载", ActionTone::Primary),
        InstallState::Installed | InstallState::Ahead => ("下载", ActionTone::Idle),
    }
}

fn row_caption(progress: &Option<(String, u8)>, id: &str, label: &str) -> String {
    match progress {
        Some((pid, pct)) if pid == id => format!("{pct}%"),
        _ => label.to_string(),
    }
}

fn row_shell(name: &str, version: &str, info: Element, control: Element) -> Element {
    Element::row()
        .fill()
        .cross(Align::Center)
        .spacing(8)
        .height(ROW_H)
        .padding_xy(16, 0)
        .child(
            Element::label(name)
                .font_size(FONT_MD)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(version_cell(version))
        .child(info)
        .child(control)
}

fn filter_installed(list: &[Extension], query: &str) -> Vec<Extension> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return list.to_vec();
    }
    list.iter()
        .filter(|e| {
            let m = &e.manifest;
            matches_text(
                &format!(
                    "{} {} {} {} {}",
                    m.id,
                    m.name,
                    m.version,
                    m.description,
                    caps_label(&m.capabilities)
                ),
                &q,
            )
        })
        .cloned()
        .collect()
}

fn filter_available(list: &[AvailablePlugin], query: &str) -> Vec<AvailablePlugin> {
    let q = query.trim().to_lowercase();
    list.iter()
        .filter(|p| {
            if q.is_empty() {
                return true;
            }
            matches_text(
                &format!(
                    "{} {} {} {} {}",
                    p.id,
                    p.name,
                    p.version,
                    p.description,
                    caps_label(&p.capabilities)
                ),
                &q,
            )
        })
        .cloned()
        .collect()
}

fn matches_text(haystack: &str, query: &str) -> bool {
    let h = haystack.to_lowercase();
    query.split_whitespace().all(|t| h.contains(t))
}

fn count_label(shown: usize, total: usize, unit: &str) -> String {
    if shown == total {
        format!("{total} 个{unit}")
    } else {
        format!("{shown} / {total} 个{unit}")
    }
}

fn summary(list: &[AvailablePlugin], installed: &[Extension], src: SourceId) -> String {
    let (mut ready, mut upgradable) = (0usize, 0usize);
    for e in list {
        if !e.compatible() {
            continue; // 协议/宿主不满足：列出来但不算"可下载"（按钮也是灰的）
        }
        if !e.downloadable_for(src) && !extensions::local_package_exists(e) {
            continue;
        }
        match remote::install_state(e, installed) {
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

fn caps_label(caps: &[Capability]) -> String {
    if caps.is_empty() {
        return "无能力声明".to_string();
    }
    caps.iter()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ext::Kind;

    #[test]
    fn labels_are_human_readable() {
        assert_eq!(caps_label(&[]), "无能力声明");
        assert_eq!(caps_label(&[Capability::ImageRead]), "image.read");
    }

    #[test]
    fn counts_describe_filtered_and_total() {
        assert_eq!(count_label(3, 3, "插件"), "3 个插件");
        assert_eq!(count_label(1, 3, "插件"), "1 / 3 个插件");
    }

    fn entry(id: &str, version: &str, url: &str) -> AvailablePlugin {
        let mut urls = std::collections::HashMap::new();
        urls.insert("gitee".to_string(), url.to_string());
        AvailablePlugin {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            protocol: 1,
            min_host: "0.1.0".into(),
            kind: Kind::Command,
            description: String::new(),
            file: format!("{id}_{version}.zip"),
            size: 1,
            capabilities: Vec::new(),
            released: String::new(),
            author: String::new(),
            license: String::new(),
            urls,
            installed: false,
        }
    }

    #[test]
    fn action_is_grey_without_a_usable_source() {
        let e = entry("a", "1.0.0", "");
        assert_eq!(
            action_of(&e, InstallState::Absent, SourceId::Gitee, false),
            ("下载", ActionTone::Idle)
        );
        let local = entry("a", "1.0.0", "");
        assert_eq!(
            action_of(&local, InstallState::Absent, SourceId::Gitee, true),
            ("下载", ActionTone::Primary)
        );
    }

    #[test]
    fn upgrade_is_orange_and_installed_is_grey() {
        let e = entry(
            "a",
            "1.0.0",
            "https://gitee.com/qiuzongman/RuPix-Plugins/releases/download/a_1.0.0/a_1.0.0.zip",
        );
        assert_eq!(
            action_of(&e, InstallState::Upgradable, SourceId::Gitee, false),
            ("升级", ActionTone::Warning)
        );
        assert_eq!(
            action_of(&e, InstallState::Installed, SourceId::Gitee, false),
            ("下载", ActionTone::Idle)
        );
        assert_eq!(
            action_of(&e, InstallState::Ahead, SourceId::Gitee, false),
            ("下载", ActionTone::Idle)
        );
    }

    #[test]
    fn percent_only_marks_the_downloading_row() {
        let p = Some(("a".to_string(), 42));
        assert_eq!(row_caption(&p, "a", "下载"), "42%");
        assert_eq!(row_caption(&p, "b", "下载"), "下载");
    }

    #[test]
    fn page_builds_without_panicking() {
        let cfg = Rc::new(RefCell::new(Config::default()));
        let ui = PluginUi::new(&cfg.borrow());
        let _ = extensions_page(&cfg, &ui);
    }

    /// 行尾两颗小按钮（「!」详情 / 设置齿轮）必须是**同一个盒子**。
    ///
    /// 2026-11-05 用户报障「齿轮按钮宽度和高度都不对」：按钮盒子 = 字形度量 +
    /// Medium 内边距（左右 32 / 上下 18，windui 侧写死），而齿轮是 MDL2 图标字形
    /// （14×14）、「!」是正文字形（5×18）——不钉死就长成 46×32 vs 37×36。
    ///
    /// 这里用**真实 DirectWrite** 量：①「!」的自然盒子必须等于钉死的 `ICON_BTN_W/H`
    /// （FONT_MD 或界面字体变了就红，提示重订）；②两颗按钮排出来的实际盒子必须一致。
    #[test]
    fn info_row_icon_buttons_share_one_box() {
        use tiny_skia::Pixmap;
        use windui::core::Tree;
        use windui::geometry::Size;
        use windui::render::{Canvas, SkiaCanvas};
        use windui::text::{DWriteEngine, TextEngine, TextStyle};

        // ① 「!」的自然盒子（字形 + Medium 内边距 32/18）。
        let (bw, bh) = {
            let mut pm = Pixmap::new(8, 8).expect("画布");
            let mut eng = DWriteEngine::new();
            eng.set_scale(1.0);
            let mut cx = SkiaCanvas::with_text(&mut pm, &mut eng, 1.0);
            let s = cx.measure_text("!", &TextStyle::new(FONT_MD));
            (s.w, s.h)
        };
        assert_eq!(
            (bw + 32, bh + 18),
            (ICON_BTN_W, ICON_BTN_H),
            "「!」的自然盒子变了（{bw}×{bh}）：FONT_MD 或界面字体改了，ICON_BTN_W/H 要跟着重订"
        );

        // ② 两颗按钮排一次，比实际盒子。
        let row = Element::row()
            .spacing(6)
            .child(gear_btn())
            .child(info_btn());
        let mut tree = Tree::new();
        let root = row.build(&mut tree);
        tree.root = Some(root);
        let mut eng = DWriteEngine::new();
        eng.set_scale(1.0);
        tree.layout_root(Size::new(200, 60), &mut eng);
        let kids = &tree.get(root).expect("行").children;
        let gear = tree.get(kids[0]).expect("齿轮").bounds;
        let bang = tree.get(kids[1]).expect("感叹号").bounds;
        assert_eq!(
            (gear.w, gear.h),
            (bang.w, bang.h),
            "齿轮与「!」不同款：齿轮 {gear:?} / 「!」 {bang:?}"
        );
        assert_eq!((gear.w, gear.h), (ICON_BTN_W, ICON_BTN_H));
    }
}
