//! 右侧信息面板（信息坞）的**会话态**与宽度 / 可见性纯函数。
//!
//! ★ 为什么状态住在这里而不是 `Config`：面板开关与目标宽是**持久偏好**（`Config` 是正本），
//! 但"这一拍到底可不可见 / 画布占多少比例 / 卡片收没收起"是**按窗的会话态**——同一份配置
//! 可能开着两扇窗，一扇被拉窄到面板放不下，另一扇仍该显示面板。配置只在建窗时灌一次种子
//! （[`InfoPanel::init_from`]），之后本窗以本结构为真值。
//!
//! ★ 宽度与可见性的判据一律写成**纯函数**（[`panel_max_px`] / [`panel_visible`] /
//! [`clamp_panel_px`] / [`ratio_for_panel`] / [`panel_px`] / [`clamp_ratio`]）：它们同时被
//! `tick`、分隔条适配器与绘制路径消费，抽出来才谈得上无头单测——"拖到 240 停住""窄窗压成
//! 隐藏"这类边界真机上很难精确复现，只能在纯函数上钉死。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windui::core::{EventCtx, Widget};
use windui::event::CursorShape;
use windui::geometry::{Color, Rect, Size};
use windui::prelude::*;
use windui::render::Canvas;
use windui::signal::{signal, Signal};
use windui::style::Style;
use windui::text::TextEngine;
use windui::ui::rich::{Para, RichDoc, SpanStyle};
use windui::ui::split::{SplitHandle, SplitOpts};

use self::properties::MetaSignals;
use super::chrome;
use super::singleton;
use super::tokens::{self, PanelColors};
use crate::core::config::{self, Config, INFO_PANEL_W_DEFAULT, INFO_PANEL_W_MAX, INFO_PANEL_W_MIN};

pub mod plugin_output;
pub mod properties;

/// 面板目标宽的下 / 上限（逻辑 px）。
///
/// 数值正本是 [`crate::core::config`] 的三个 `INFO_PANEL_W_*`（ini 解析与默认值都读那份），
/// 这里只是转成 i32 给几何计算用——两处只许有一份读数。
pub const PANEL_W_MIN: i32 = INFO_PANEL_W_MIN as i32;
pub const PANEL_W_MAX: i32 = INFO_PANEL_W_MAX as i32;

/// 低于这个内容宽就自动收起面板：600 × 50% = 300，仍高于 240 下限。
///
/// 再窄下去"面板 + 抓手"会吃掉画布的大半，看图器的主业（看图）就没了；这里选自动隐藏而
/// 不是继续压窄面板，是因为 240 以下的中文标签已经折得没法读。
pub const PANEL_MIN_WINDOW: i32 = 600;

/// 面板最多占内容宽的多少。
///
/// 用户口径（2026-11-14）：最大宽度 = 窗口内容宽的一半。绝对上限
/// [`PANEL_W_MAX`] 只防 ini 天文数字，4K / 8K 窗口都由这条半宽先约束。
pub const PANEL_MAX_SHARE: f32 = 0.50;

/// 面板左缘那条分隔线的宽度（逻辑 px）。
///
/// ★ 抓手是**一条 1px 线**，不属于面板矩形：比例换算必须把它先扣掉（[`usable_w`]），
///   否则窗口拉宽时面板会比目标宽多出这一条，拖到下限也收不齐。
pub const HANDLE_W: i32 = 1;

/// 分隔线的**可抓扩展**（逻辑 px）：线两侧各 8px 都能拖，视觉仍只有 1px。
///
/// ★ 顶部 `STRIP_H` 那一段由 `window.rs` 用负的上边距裁掉：画布最右那颗关闭钮
///   紧贴分隔线，不裁的话扩进标题栏的 8px 会把「点关闭」抢成「拖宽」。
/// ★ 扩展只作用于命中（windui 补丁 030 的 `hit_pad`），不参与 `usable_w` 的换算。
pub const HANDLE_HIT_PAD: i32 = 8;

/// 「图片信息」卡 id：文件 / 图像 / 拍摄 / 技术四节合一（2026-11-14 合并属性与技术信息）。
pub const CARD_INFO: &str = "info";
/// 旧属性卡 id：仅用于配置兼容（读到即视为 `info`），不再写入。
pub const CARD_PROPERTIES_LEGACY: &str = "properties";
/// 旧技术信息卡 id：仅用于配置兼容。
pub const CARD_TECHNICAL_LEGACY: &str = "technical";

/// 信息面板里的一张卡。
///
/// ★ 2026-11-14 用户裁定：内置直方图退役——`levels`（色阶分析）插件给得更细。
///   这里只留「图片信息」；`ALL` 仍是菜单顺序与持久化顺序的唯一真源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Card {
    Info,
}

impl Card {
    pub const ALL: [Card; 1] = [Card::Info];

    /// ini 里的卡片 id（`info_cards` / `info_cards_collapsed` 共用）。
    pub fn id(self) -> &'static str {
        match self {
            Card::Info => CARD_INFO,
        }
    }

    /// 卡片标题与菜单项文案。
    pub fn label(self) -> &'static str {
        "图片信息"
    }

    /// 这一条 `info_cards` 配置是否打开本卡。
    ///
    /// 兼容旧 `properties` / `technical`；旧 `histogram`（已退役）不在任何卡的
    /// 匹配表里，读到时静默忽略。
    pub fn matches_id(self, id: &str) -> bool {
        match self {
            Card::Info => {
                id == CARD_INFO || id == CARD_PROPERTIES_LEGACY || id == CARD_TECHNICAL_LEGACY
            }
        }
    }

    /// 这张卡在不在面板里。
    pub fn active(self, info: &InfoPanel) -> Signal<bool> {
        match self {
            Card::Info => info.info_active,
        }
    }

    /// 这张卡展不展开。
    pub fn expanded(self, info: &InfoPanel) -> Signal<bool> {
        match self {
            Card::Info => info.info_expanded,
        }
    }
}

/// 卡片标题行高：比设置页 `CARD_HEADER_H`（36）矮一档，面板本来就窄，
/// "属性"两个字的标题条再占 36 + 上下 padding 会显得空。
/// 2026-11-14 用户复看再压一档：28 → 24，配合卡片外壳 padding 8。
const CARD_HEAD_H: i32 = 24;

/// 信息面板滚动条的右移量：抵消 windui 轨道自带的 3px 外边距。
///
/// 目标不是压到窗口最外沿（那 8px 是 `WM_NCHITTEST` 的缩放带，压上去会点不到），
/// 而是让 track 右缘与左侧文件树 / 缩略图条落在**同一条内边线**：面板右缘 − 8px。
/// 做法：滚动节点向右多占 3px（负 margin），右 padding 同步 +3，内容宽度不变。
const SCROLLBAR_SHIFT: i32 = windui::core::scrollbar::MARGIN as i32;

/// 滚动条命中区宽（windui `scrollbar::HIT_W`，16px）：内容右缘必须让开它。
///
/// ★ 真机复现（2026-11-14）：插件卡把面板内容撑出滚动条后，卡头的 ✕ 正好落在
///   滚动条命中区里——一点就翻页，卡片关不掉。这里让出的宽度使卡片右缘停在
///   track 左侧 9px（左 padding 8），视觉上左右几乎对称，✕ 也不再被吞。
const SCROLLBAR_HIT_W: i32 = windui::core::scrollbar::HIT_W;

/// 一扇窗的信息面板会话态。
///
/// `Cell` / `Signal` 混用与 `Chrome` 其余字段同一定式：需要驱动控件重绘的（卡片展开、
/// 画布占比）走 `Signal`，纯数值判据走 `Cell`——后者由 `tick` 统一转成一次
/// `mark_layout_dirty`，不会每个写点都惊动整棵控件树。
pub struct InfoPanel {
    /// 用户期望的开 / 关。窄窗或无边框时它仍为真，[`Self::visible`] 才反映实际可见。
    pub open: Cell<bool>,
    /// 目标宽（逻辑 px，240..=4096，实际上限 = 内容宽的 50%）。窗口缩小时**不改**它，
    /// 恢复后回到用户选的那一档。
    pub width: Cell<i32>,
    /// 当前实际可见（窄窗 / 无边框会压成 false）。
    pub visible: Cell<bool>,
    /// 根内容宽，由 `tick` 每拍回写；0 = 首帧还不知道。
    pub content_w: Cell<i32>,
    /// 画布列的占比，分隔条适配器直接写它。
    pub ratio: Signal<f32>,
    /// 上一拍的比例：本拍与它不等即视为"用户拖动"，否则是窗口缩放或稳定态。
    pub last_ratio: Cell<f32>,
    /// 上一拍的可见性：不可见 → 可见的那一拍要按目标宽重新反算比例。
    pub was_visible: Cell<bool>,
    /// 没有 `EventCtx` 的写点（菜单、快捷键、持久化）借它请求下一拍重排。
    pub layout_dirty: Cell<bool>,
    /// 「图片信息」卡是否在面板里（对应 `info_cards`；兼容旧 `properties`/`technical`）。
    pub info_active: Signal<bool>,
    /// 「图片信息」卡是否展开（对应 `info_cards_collapsed` 的反面）。
    pub info_expanded: Signal<bool>,
    /// 插件输出卡（会话级；换图清空，不落配置）。
    pub plugin_cards: Signal<Vec<plugin_output::PluginCard>>,
    /// 上一张图的路径：路径变化时清掉插件输出，避免上一张图的报告冒充当前图。
    plugin_scope: RefCell<Option<std::path::PathBuf>>,
}

// 与 `UpdateState` / `CompanionPane` 同一定式：无参 `new()` 必须配一份 `Default`，
// 否则 clippy 的 `new_without_default` 会在 `2_scan.bat` 的 `-D warnings` 上判红。
impl Default for InfoPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl InfoPanel {
    pub fn new() -> Self {
        // 初始比例只为"第一拍之前"存在：真实值由 `init_from` / 首拍 `tick` 按目标宽反算。
        let ratio = signal(0.72);
        Self {
            open: Cell::new(false),
            width: Cell::new(INFO_PANEL_W_DEFAULT as i32),
            visible: Cell::new(false),
            content_w: Cell::new(0),
            ratio,
            last_ratio: Cell::new(0.72),
            was_visible: Cell::new(false),
            layout_dirty: Cell::new(false),
            info_active: signal(false),
            info_expanded: signal(true),
            plugin_cards: signal(Vec::new()),
            plugin_scope: RefCell::new(None),
        }
    }

    /// 建窗时把持久偏好灌成会话态。之后本窗以本结构为真值，配置只作落点。
    pub fn init_from(&self, cfg: &Config) {
        self.open.set(cfg.info_panel);
        let w = cfg
            .info_panel_width
            .clamp(INFO_PANEL_W_MIN, INFO_PANEL_W_MAX) as i32;
        self.width.set(w);
        for card in Card::ALL {
            let active = cfg.info_cards.iter().any(|id| card.matches_id(id));
            let collapsed = cfg
                .info_cards_collapsed
                .iter()
                .any(|id| card.matches_id(id));
            card.active(self).set(active);
            // 收起是"展开"的反面。★ 卡没开时 `expanded` 一律为假（不是"一律展开"）：
            // 没有卡，展开态没有对象；下次入口会把两者一起置真，加回来就是展开的。
            card.expanded(self).set(active && !collapsed);
        }
        // 此时窗口还没建，拿不到真实内容宽：按配置的窗宽估一份，首拍 `tick` 会用真宽纠正。
        // 不估这一下的话，开着面板建窗的首帧会按 `new()` 的初值摆一次，看得见跳一下。
        let approx = (cfg.window_w.max(1) as f32 - 16.0).max(1.0) as i32;
        let wide = approx >= PANEL_MIN_WINDOW;
        // 比例必须和 `tick` 走**同一套**像素换算（含 50% 上限）：旧写法只夹到 [0, 1]，
        // 1000 宽窗里配 480 的目标宽会摆出 48% 的首帧，下一拍再跳回半宽。
        // 窗窄到面板本就不可见时不估——那几拍的比例没有消费方，留 `new()` 的初值即可。
        if wide {
            let px = clamp_panel_px(w, approx);
            let r = ratio_for_panel(px, approx);
            self.ratio.set(r);
            self.last_ratio.set(r);
        }
        self.visible.set(cfg.info_panel && !cfg.borderless && wide);
    }

    pub fn panel_visible(&self) -> bool {
        self.visible.get()
    }

    /// 按当前开 / 无边框 / 内容宽重算可见性，返回"翻没翻"（调用方据此重排）。
    pub fn refresh_visible(&self) -> bool {
        let changed = panel_visible(self.open.get(), chrome::borderless(), self.content_w.get());
        self.visible.replace(changed) != changed
    }

    /// 开 / 关面板。返回"有没有需要重排的变化"。
    pub fn set_open(&self, on: bool) -> bool {
        let open_changed = self.open.replace(on) != on;
        if on && open_changed {
            self.apply_width_now();
        }
        let vis_changed = self.refresh_visible();
        if open_changed || vis_changed {
            self.layout_dirty.set(true);
        }
        open_changed || vis_changed
    }

    /// 打开面板时立刻按目标宽反算比例；不等下一拍 tick，避免首帧从旧比例跳一下。
    fn apply_width_now(&self) {
        let cw = self.content_w.get();
        if cw <= 0 {
            return;
        }
        let px = clamp_panel_px(self.width.get(), cw);
        let r = ratio_for_panel(px, cw);
        self.ratio.set(r);
        self.last_ratio.set(r);
    }

    /// 把一张卡加进面板并展开；面板若关着，一并打开。
    pub fn open_card(&self, card: Card) -> bool {
        let mut changed = self.set_open(true);
        let active = card.active(self);
        let expanded = card.expanded(self);
        if !active.get() {
            active.set(true);
            changed = true;
        }
        if !expanded.get() {
            expanded.set(true);
            changed = true;
        }
        if changed {
            self.layout_dirty.set(true);
        }
        changed
    }

    /// 把一张卡从面板里拿掉（面板本身留不留由用户的开 / 关决定）。
    ///
    /// 返回"原来在里面、这次真拿掉了"。
    pub fn close_card(&self, card: Card) -> bool {
        let active = card.active(self);
        if !active.get() {
            return false;
        }
        active.set(false);
        self.layout_dirty.set(true);
        true
    }

    /// 折叠 / 展开一张卡。恒返回 true：折叠本身就是一次外观变化。
    pub fn toggle_card(&self, card: Card) -> bool {
        let expanded = card.expanded(self);
        expanded.set(!expanded.get());
        self.layout_dirty.set(true);
        true
    }

    /// 面板里还有没有卡（空态提示与真实卡片互斥）。
    pub fn any_active(&self) -> bool {
        Card::ALL.iter().any(|c| c.active(self).get()) || !self.plugin_cards.get().is_empty()
    }

    /// 推入/替换一张插件输出卡。同一插件重跑时替换原卡并置顶。
    pub fn push_plugin_card(&self, card: plugin_output::PluginCard) {
        let mut cards = self.plugin_cards.get();
        cards.retain(|c| c.key != card.key);
        cards.insert(0, card);
        self.plugin_cards.set(cards);
        self.layout_dirty.set(true);
    }

    /// 展开 / 收起一张插件输出卡（会话级，不落配置）。
    pub fn toggle_plugin_card(&self, key: &str) {
        let mut cards = self.plugin_cards.get();
        let Some(card) = cards.iter_mut().find(|c| c.key == key) else {
            return;
        };
        card.expanded = !card.expanded;
        self.plugin_cards.set(cards);
        self.layout_dirty.set(true);
    }

    /// 关闭一张插件输出卡。
    pub fn remove_plugin_card(&self, key: &str) {
        let mut cards = self.plugin_cards.get();
        let before = cards.len();
        cards.retain(|c| c.key != key);
        if cards.len() != before {
            self.plugin_cards.set(cards);
            self.layout_dirty.set(true);
        }
    }

    /// 清空所有插件输出（换图 / 关窗）。
    pub fn clear_plugin_cards(&self) {
        if !self.plugin_cards.get().is_empty() {
            self.plugin_cards.set(Vec::new());
            self.layout_dirty.set(true);
        }
    }

    /// 图片路径变了就清掉插件输出；缩放/旋转同一路径不清。
    pub fn sync_plugin_scope(&self, path: Option<&std::path::Path>) {
        let changed = {
            let mut last = self.plugin_scope.borrow_mut();
            if last.as_deref() == path {
                false
            } else {
                *last = path.map(std::path::Path::to_path_buf);
                true
            }
        };
        if changed {
            self.clear_plugin_cards();
        }
    }

    /// 面板里加入属性卡并展开（「属性」菜单项 / 快捷键的落点）。
    pub fn open_properties(&self) -> bool {
        self.open_card(Card::Info)
    }

    /// 把属性卡从面板里拿掉。
    pub fn close_properties(&self) -> bool {
        self.close_card(Card::Info)
    }

    /// 折叠 / 展开属性卡。
    pub fn toggle_properties(&self) -> bool {
        self.toggle_card(Card::Info)
    }

    /// 把会话态写回配置（**不写盘**：交给 `config::mark_dirty` 的 5 秒防抖）。
    pub fn persist(&self, cfg: &Rc<RefCell<Config>>) {
        {
            let mut c = cfg.borrow_mut();
            c.info_panel = self.open.get();
            c.info_panel_width = self.width.get().clamp(PANEL_W_MIN, PANEL_W_MAX) as u32;
            // 只写"当前已知卡片"：未知 id（未来版本 / 已删卡片）不在这里续命。
            let mut active = Vec::new();
            let mut collapsed = Vec::new();
            for card in Card::ALL {
                if card.active(self).get() {
                    active.push(card.id().to_string());
                    if !card.expanded(self).get() {
                        collapsed.push(card.id().to_string());
                    }
                }
            }
            c.info_cards = active;
            c.info_cards_collapsed = collapsed;
        }
        config::mark_dirty();
    }

    /// 拖动过程中每拍写回目标宽。值没变就**不**碰配置——否则每帧都把防抖计时推后，
    /// 用户松手后要再等满 5 秒才落盘。
    pub fn persist_width(&self, cfg: &Rc<RefCell<Config>>) {
        let w = self.width.get().clamp(PANEL_W_MIN, PANEL_W_MAX) as u32;
        // ★ 先把读数取出来再判：`if cfg.borrow().…` 的临时 `Ref` 在该语句结束时才释放，
        // 下面紧跟的 `borrow_mut` 稍有不慎就是 `RefCell already borrowed`（§七那条禁写）。
        let current = cfg.borrow().info_panel_width;
        if current == w {
            return;
        }
        cfg.borrow_mut().info_panel_width = w;
        config::mark_dirty();
    }

    /// 每窗一拍：回写内容宽、刷新可见性、消费重排请求，并在"用户拖动"与"窗口缩放"之间
    /// 分流（判据见下）。
    pub fn tick(&self, ctx: &mut EventCtx, content_w: i32, cfg: &Rc<RefCell<Config>>) {
        if content_w > 0 {
            self.content_w.set(content_w);
        }
        if self.refresh_visible() {
            ctx.mark_layout_dirty();
        }
        if self.layout_dirty.replace(false) {
            ctx.mark_layout_dirty();
        }
        if !self.visible.get() {
            self.was_visible.set(false);
            return;
        }
        let cw = self.content_w.get();
        if cw <= 0 {
            self.was_visible.set(false);
            return;
        }
        // 刚变可见（打开 / 窄窗拉宽）：按目标宽重摆一次，别沿用旧比例。
        if !self.was_visible.replace(true) {
            let px = clamp_panel_px(self.width.get(), cw);
            let r = ratio_for_panel(px, cw);
            self.ratio.set(r);
            self.last_ratio.set(r);
            ctx.mark_layout_dirty();
            return;
        }
        let r = self.ratio.get();
        if (r - self.last_ratio.get()).abs() > 0.0001 {
            // 比例变了 = 用户拖了抓手：夹进允许范围，并把夹后的像素**记成新的目标宽**。
            let cur = panel_px(r, cw);
            let px = clamp_panel_px(cur, cw);
            if px != cur {
                self.ratio.set(ratio_for_panel(px, cw));
                ctx.mark_layout_dirty();
            }
            self.last_ratio.set(self.ratio.get());
            if px != self.width.get() {
                self.width.set(px);
                self.persist_width(cfg);
            }
        } else {
            // 比例没变 = 窗口缩放或稳定态：按目标宽反算。★ 这里**不改 `width`**，
            // 于是窗口缩小时目标宽保留，拉回去仍回到用户选的那一档。
            let desired = clamp_panel_px(self.width.get(), cw);
            let cur = panel_px(r, cw);
            if (cur - desired).abs() > 1 {
                self.ratio.set(ratio_for_panel(desired, cw));
                self.last_ratio.set(self.ratio.get());
                ctx.mark_layout_dirty();
            }
        }
    }
}

/// 「属性」的统一入口：右键菜单 / 快捷键 / 浮条共用。
///
/// 无边框形态不显示面板（没有标题条可拖，开了只能进不能出），只回一句提示；
/// 普通形态把面板打开、属性卡展开，并当场落进配置。窗口太窄时面板会先收起，
/// 这里补一句说明——否则用户点了菜单什么也看不见，会以为入口坏了。
pub fn open_properties(
    ctx: Option<&mut EventCtx>,
    chrome: &Rc<chrome::Chrome>,
    cfg: &Rc<RefCell<Config>>,
) {
    if chrome::borderless() {
        if let Some(ctx) = ctx {
            ctx.toast_err("无边框形态不显示信息面板");
        }
        return;
    }
    // 只有真变了才落配置：重复按「属性」不该把 5 秒防抖计时一直推后。
    if chrome.info.open_properties() {
        chrome.info.persist(cfg);
    }
    if let Some(ctx) = ctx {
        // 判据用**实际可见性**而不是"内容宽 < 600"：无边框已在上面提前返回，
        // 剩下能压成不可见的只有窄窗这一条，但可见性是唯一正本，别再抄一遍阈值。
        if !chrome.info.panel_visible() {
            ctx.toast_err("窗口太窄，信息面板暂时收起；拉宽窗口后显示");
        }
        ctx.mark_layout_dirty();
        ctx.mark_dirty();
    }
}

/// 面板这一拍最多能占多宽：内容宽的 [`PANEL_MAX_SHARE`]，再夹到 [`PANEL_W_MAX`]。
pub fn panel_max_px(content_w: i32) -> i32 {
    ((content_w as f32 * PANEL_MAX_SHARE).floor() as i32).min(PANEL_W_MAX)
}

/// 面板该不该显示。`content_w <= 0` = 首帧宽度未知，先按"够宽"处理，下一拍用真宽纠正。
pub fn panel_visible(open: bool, borderless: bool, content_w: i32) -> bool {
    open && !borderless && (content_w <= 0 || content_w >= PANEL_MIN_WINDOW)
}

/// 扣掉抓手之后真正分给"画布 + 面板"的宽（下限 1 防除零）。
pub fn usable_w(content_w: i32) -> i32 {
    (content_w - HANDLE_W).max(1)
}

/// 把像素目标宽夹进 `[PANEL_W_MIN, max(PANEL_W_MIN, panel_max_px)]`。
///
/// 上界的 `max(PANEL_W_MIN)` 是给窄窗兜底的：600 以下面板本来就隐藏，但拖动中/刚打开那几拍
/// 仍会算像素，若上界掉到 240 以下，`clamp` 的 lo > hi 会 panic。
pub fn clamp_panel_px(px: i32, content_w: i32) -> i32 {
    let max = panel_max_px(content_w).max(PANEL_W_MIN);
    px.clamp(PANEL_W_MIN, max)
}

/// 像素宽 → 画布占比（`Signal<f32>` 用的那一侧换算）。
pub fn ratio_for_panel(px: i32, content_w: i32) -> f32 {
    1.0 - px as f32 / usable_w(content_w) as f32
}

/// 画布占比 → 像素宽（`ratio_for_panel` 的逆）。
pub fn panel_px(ratio: f32, content_w: i32) -> i32 {
    ((1.0 - ratio) * usable_w(content_w) as f32).round() as i32
}

/// 把外部写进来的比例夹进允许范围；没越界时**原样返回**，免得拖动时被来回取整磨出抖动。
pub fn clamp_ratio(ratio: f32, content_w: i32) -> f32 {
    let cur = panel_px(ratio, content_w);
    let px = clamp_panel_px(cur, content_w);
    if px == cur {
        ratio
    } else {
        ratio_for_panel(px, content_w)
    }
}

// ── 面板树（`ui::window::build_root` 已接进根布局）────────────────────

/// 建信息面板的整棵树：深 / 浅两套配色各建一份，按当前主题显隐。
///
/// ★ 两份共用同一套 [`MetaSignals`]（全是 `Signal` 句柄，复制即可）：图片切换时只写一遍
///   信号，两棵树同时跟着刷新。构建期就把颜色定死，是因为 [`Element`] 的颜色不是信号：
///   在绘制路径上临时读主题，换肤就得重建整棵树；建两份只用一次 `visible_when` 切换。
pub fn build(
    viewer: &Rc<RefCell<crate::core::viewer::Viewer>>,
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
) -> Element {
    let sig = properties::meta_signals(singleton::window_scope(viewer));
    let dark = build_one(chrome, config, &sig, PanelColors::dark())
        .visible_when(move || tokens::theme() == tokens::Theme::Dark);
    let light = build_one(chrome, config, &sig, PanelColors::light())
        .visible_when(move || tokens::theme() == tokens::Theme::Light);
    Element::stack().fill().child(dark).child(light)
}

/// 用一套配色建一扇窗的面板树。`build` 只负责建两份主题副本，布局全在这里。
///
/// `colors` 是**唯一**取色入口：本函数（含 [`info_card`] / [`info_row`]）不读
/// `tk::*()`，否则主题切换那一帧会有一半控件停在旧色上。
///
/// 结构：`col[ 标题条(固定 STRIP_H) , scroll(weight 1)[ 空态提示 / 三张卡 ] ]`。
/// 标题条是**面板自己的标题**，不承担窗口标题栏职责：窗口五颗钮仍在画布顶条，
/// 面板头部只留「信息」和收起面板的 `<`。
///
/// 内置卡共用同一套卡片外壳（[`card_shell`]）：目前只有「图片信息」一张，
/// 各自可折叠、可关闭、状态进 `info_cards`。
pub fn build_one(
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    sig: &MetaSignals,
    colors: PanelColors,
) -> Element {
    // 收起面板：只有真变了才落配置（重复点不该把 5 秒防抖计时推后）。
    let close_panel = {
        let chrome = chrome.clone();
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            if chrome.info.set_open(false) {
                chrome.info.persist(&config);
            }
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };
    let header = Element::row()
        .width_match()
        .height(chrome::STRIP_H)
        .cross(Align::Center)
        .bg(colors.header)
        .child(
            Element::label("信息")
                .font_size(super::tokens::SIZE_BODY)
                .fg(colors.text)
                .padding_xy(10, 0)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(
            // 收起用 MDL2 的 ChevronLeft（与主窗标题栏同一套图标字体 / 统一图标尺寸）。
            Element::icon_button("\u{E76B}")
                .size(40, 40)
                .font_family(super::tokens::FAMILY_ICON)
                .font_size(super::tokens::SIZE_ICON)
                .fg(colors.text)
                .tooltip("收起信息面板")
                .on_click(close_panel),
        );

    // 没有卡片时的空态：只写一句提示，不占位。`info_active` 为假时才显示，
    // 与卡片互斥——两者若同时可见，用户会以为卡片下面还挂着一张空卡。
    let empty = Element::label("右键「本图 ▸ 图片信息」或插件输出")
        .font_size(super::tokens::SIZE_BODY)
        .fg(colors.muted)
        .padding(12)
        .visible_when({
            let chrome = chrome.clone();
            move || !chrome.info.any_active()
        });
    let plugin_cards = plugin_output::cards_element(chrome, colors);
    let info = info_card(chrome, config, sig, colors);
    // ★ `weight(1.0)` 而不是 `fill()`：`fill()` 在列里会把主轴也撑成“内容高”，
    //   面板就跟着卡片长高 / 缩矮；这里要的是**吃掉标题条剩下的高度**，_内部_再滚。
    let body = Element::scroll()
        .weight(1.0)
        .width_match()
        // 右移 3px 后 track 右缘贴面板内边线；右 padding 同步补回，卡片宽度不变。
        .margin_edges(Insets {
            right: -SCROLLBAR_SHIFT,
            ..Default::default()
        })
        .padding_edges(Insets {
            left: 8,
            top: 8,
            right: 8 + SCROLLBAR_SHIFT + SCROLLBAR_HIT_W,
            bottom: 8,
        })
        .child(
            Element::col()
                .width_match()
                .spacing(8)
                .child(plugin_cards)
                .child(empty)
                .child(info),
        );

    Element::col()
        .fill()
        .bg(colors.panel)
        .child(header)
        .child(body)
}

/// 内置卡右键「复制全部信息」：把行头与当前文本信号拼成纯文本。
///
/// 行是 `行头<TAB>值`，空值跳过；插件卡有自己的复制菜单，不走这里。
fn copy_info_menu(
    title: &'static str,
    rows: Vec<(&'static str, Signal<String>)>,
) -> impl Fn() -> Vec<MenuItem> {
    move || {
        let mut text = String::from(title);
        text.push('\n');
        for (label, sig) in &rows {
            let v = sig.get();
            if !v.trim().is_empty() {
                text.push_str(label);
                text.push('\t');
                text.push_str(&v);
                text.push('\n');
            }
        }
        let body = text.trim_end().to_string();
        vec![MenuItem::run(
            "复制全部信息",
            move |ctx: &mut EventCtx| match crate::platform::clipboard::copy_text(&body) {
                Ok(()) => ctx.toast_ok("已复制到剪贴板"),
                Err(e) => ctx.toast_err(format!("复制失败：{e}")),
            },
            false,
        )]
    }
}

/// 「图片信息」卡：文件 / 图像 / 拍摄 / 技术四节合一。
///
/// 2026-11-14 用户口径：图片**本来就有的信息**（文件属性、EXIF、解码器/格式/像素）
/// 用原生入口，不再拆「属性」「技术信息」两张卡；需要额外分析的归插件。
fn info_card(
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    sig: &MetaSignals,
    colors: PanelColors,
) -> Element {
    let has_image = sig.has_image;

    // 无图只留一句“没有打开图片”；有图时这行隐藏、四节整块出现。
    let no_image = Element::label("没有打开图片")
        .font_size(super::tokens::SIZE_BODY)
        .fg(colors.muted)
        .visible_when(move || !has_image.get());

    let section = |title: &'static str, rows: Element| {
        Element::col()
            .width_match()
            .spacing(2)
            .child(
                Element::label(title)
                    .font_size(super::tokens::SIZE_BODY)
                    .font_weight(super::tokens::WEIGHT_EMPHASIS)
                    .fg(colors.text)
                    .padding_edges(Insets {
                        top: 4,
                        ..Default::default()
                    }),
            )
            .child(rows)
    };

    let file_rows = Element::col()
        .width_match()
        .spacing(2)
        .child(info_row("文件名", sig.filename, colors))
        .child(info_row("类型", sig.kind, colors))
        .child(info_row("位置", sig.location, colors))
        .child(info_row("大小", sig.size, colors))
        .child(info_row("修改时间", sig.mtime, colors));

    let shot_rows = Element::col()
        .width_match()
        .spacing(2)
        .child({
            let s = sig.capture_time;
            info_row("拍摄时间", s, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.camera;
            info_row("相机", s, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.lens;
            info_row("镜头", s, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.exposure;
            info_row("曝光", s, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.focal;
            info_row("焦距", s, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.gps;
            info_row("GPS", s, colors).visible_when(move || !s.get().is_empty())
        });

    let tech_rows = Element::col()
        .width_match()
        .spacing(2)
        .child(info_row("解码器", sig.tech_decoder, colors))
        .child(info_row("格式", sig.tech_format, colors))
        .child(info_row("像素", sig.tech_pixels, colors))
        .child(info_row("像素格式", sig.tech_color, colors))
        .child(info_row("色彩配置", sig.tech_icc, colors))
        .child(info_row("方向", sig.tech_orientation, colors))
        .child(optional_row("显示", sig.tech_view, colors))
        .child(optional_row("帧/页", sig.tech_frames, colors));

    // 拍摄节：六个 EXIF 信号全空时整节（含标题）隐藏，不留一个空标题。
    let shot_any = {
        let s = sig.clone();
        move || {
            [s.capture_time, s.camera, s.lens, s.exposure, s.focal, s.gps]
                .iter()
                .any(|sig| !sig.get().is_empty())
        }
    };

    let sections = Element::col()
        .width_match()
        .spacing(6)
        .child(section("文件", file_rows))
        .child(section("拍摄", shot_rows).visible_when(shot_any))
        .child(section("技术", tech_rows))
        .visible_when(move || has_image.get());

    let body = Element::col()
        .width_match()
        .bg(colors.content)
        .corner(6.0)
        .padding(8)
        .spacing(6)
        .child(no_image)
        .child(sections)
        .on_context_menu(copy_info_menu(
            "图片信息",
            vec![
                ("文件名", sig.filename),
                ("类型", sig.kind),
                ("位置", sig.location),
                ("大小", sig.size),
                ("修改时间", sig.mtime),
                ("拍摄时间", sig.capture_time),
                ("相机", sig.camera),
                ("镜头", sig.lens),
                ("曝光", sig.exposure),
                ("焦距", sig.focal),
                ("GPS", sig.gps),
                ("解码器", sig.tech_decoder),
                ("格式", sig.tech_format),
                ("像素", sig.tech_pixels),
                ("像素格式", sig.tech_color),
                ("色彩配置", sig.tech_icc),
                ("方向", sig.tech_orientation),
                ("显示", sig.tech_view),
                ("帧/页", sig.tech_frames),
            ],
        ));

    simple_card(Card::Info, chrome, config, colors, body)
}

/// 内置卡外壳：标题行（箭头 + 标题，点击折叠） + 关闭按钮 + 正文。
fn simple_card(
    card: Card,
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    colors: PanelColors,
    body: Element,
) -> Element {
    let toggle = {
        let chrome = chrome.clone();
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            if chrome.info.toggle_card(card) {
                chrome.info.persist(&config);
            }
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };
    let close = {
        let chrome = chrome.clone();
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            if chrome.info.close_card(card) {
                chrome.info.persist(&config);
            }
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };
    card_shell(card, chrome, colors, toggle, close, body)
}

/// 卡片外壳。`on_toggle` / `on_close` 由调用方注入，外壳只管视觉与命中。
fn card_shell(
    card: Card,
    chrome: &Rc<chrome::Chrome>,
    colors: PanelColors,
    on_toggle: impl FnMut(&mut EventCtx) + 'static,
    on_close: impl FnMut(&mut EventCtx) + 'static,
    body: Element,
) -> Element {
    let expanded = card.expanded(&chrome.info);
    let active = card.active(&chrome.info);

    let title_row = Element::row()
        .cross(Align::Center)
        .spacing(4)
        .clickable()
        .on_click(on_toggle)
        .child(
            Element::label("▾")
                .font_size(super::tokens::SIZE_BODY)
                .fg(colors.text)
                .visible_signal(expanded),
        )
        .child(
            Element::label("▸")
                .font_size(super::tokens::SIZE_BODY)
                .fg(colors.text)
                .visible_when(move || !expanded.get()),
        )
        .child(
            Element::label(card.label())
                .font_size(super::tokens::SIZE_BODY)
                .font_weight(super::tokens::WEIGHT_EMPHASIS)
                .fg(colors.text)
                .max_lines(1),
        );

    let head = Element::row()
        .width_match()
        .height(CARD_HEAD_H)
        .cross(Align::Center)
        .child(title_row)
        .child(Element::flex_spacer())
        .child(
            Element::icon_button("✕")
                .size(22, 22)
                .font_size(super::tokens::SIZE_BODY)
                .fg(colors.muted)
                .tooltip(format!("关闭{}卡", card.label()))
                .on_click(on_close),
        );

    Element::col()
        .width_match()
        .bg(colors.card)
        .corner(8.0)
        .border(colors.edge, 1)
        .padding(8)
        .spacing(6)
        .visible_signal(active)
        .child(head)
        .child(body.visible_signal(expanded))
}

/// 空值整行隐藏（技术卡里“帧/页 / 显示”这类可选行）。
fn optional_row(label: &'static str, value: Signal<String>, colors: PanelColors) -> Element {
    let shown = value;
    info_row(label, value, colors).visible_when(move || !shown.get().is_empty())
}

/// 富文本文档：一段可划选、可复制的纯文本。
///
/// ★ 面板里所有**值**都走这里，而不是 `Element::label`：label 只能看，富文本控件
///   自带拖选 / 双击选词 / Ctrl+C / 右键「复制 / 复制全部」——2026-11-14 用户
///   报「文字没办法鼠标选择后再复制」，根因是值都画成了 label。
fn value_doc(text: &str, color: Color) -> RichDoc {
    // weight 不写：富文本默认就是常规 400，显式写会逼着 tokens.rs 多开一个令牌口子。
    let mut doc = RichDoc::new().style(
        "v",
        SpanStyle::new().size(super::tokens::SIZE_BODY).fg(color),
    );
    let mut any = false;
    for line in text.lines() {
        doc = doc.para(Para::new().styled("v", line.to_string()));
        any = true;
    }
    if !any {
        doc = doc.para(Para::new().styled("v", ""));
    }
    doc
}

/// 静态值的可选中版本（插件表格用）。
pub(super) fn value_text(text: &str, color: Color) -> Element {
    Element::rich(value_doc(text, color)).width_match()
}

/// 信号值的可选中版本（图片信息卡用）。
fn value_text_signal(value: Signal<String>, color: Color) -> Element {
    let doc = value.map(move |s| value_doc(s, color));
    Element::rich_signal(doc).width_match()
}

/// 一行「标签 + 值」。值可划选复制；长路径自动折行（富文本自己量高度）。
fn info_row(label: &'static str, value: Signal<String>, colors: PanelColors) -> Element {
    Element::row()
        .width_match()
        // 行头与内容顶部对齐（2026-11-14 用户口径）：多行值时行头贴第一行。
        .cross(Align::Start)
        .spacing(6)
        .child(
            Element::label(label)
                .width(60)
                .font_size(super::tokens::SIZE_BODY)
                // 同 table_row：行头与值同色，避免浅灰在浅背景上显得字号更小。
                .fg(colors.text),
        )
        .child(
            Element::col()
                .weight(1.0)
                .font_size(super::tokens::SIZE_BODY)
                .child(value_text_signal(value, colors.text)),
        )
}

/// 画布与面板之间的分隔条适配器：windui 的 [`SplitHandle`] 只管比例，本适配器补上
/// **像素范围**（[`clamp_ratio`]）：窄窗里往左拖到端头时，windui 只钳到 `max = 0.95`
/// （画布最多 95%，面板剩 5%），面板会缩成一条放不下字；这里每拖一下就按
/// 240..=`panel_max_px` 夹回去。
pub struct PanelSplitHandle {
    inner: SplitHandle,
    chrome: Rc<chrome::Chrome>,
    /// 重建 `inner` 用的同一份参数（见 `reset_interaction`）。
    opts: SplitOpts,
}

impl PanelSplitHandle {
    pub fn new(chrome: Rc<chrome::Chrome>) -> Self {
        let opts = SplitOpts {
            thickness: HANDLE_W,
            min: 0.05,
            max: 0.95,
        };
        let inner = SplitHandle::new(Axis::Horizontal, chrome.info.ratio, opts);
        Self {
            inner,
            chrome,
            opts,
        }
    }

    fn clamp(&self, ctx: &mut EventCtx) {
        let cw = self.chrome.info.content_w.get();
        if cw <= 0 {
            return;
        }
        let r = self.chrome.info.ratio.get();
        let next = clamp_ratio(r, cw);
        if (next - r).abs() > f32::EPSILON {
            self.chrome.info.ratio.set(next);
            ctx.mark_layout_dirty();
        }
    }
}

impl Widget for PanelSplitHandle {
    fn measure(&self, avail: Size, style: &Style, text: &mut dyn TextEngine) -> Size {
        self.inner.measure(avail, style, text)
    }

    fn paint(
        &self,
        bounds: Rect,
        content: Rect,
        focused: bool,
        enabled: bool,
        canvas: &mut dyn Canvas,
        style: &Style,
    ) {
        // 1px 槽只由 SplitHandle 自己那条线填满；不再铺底色（铺了就是一条宽色带）。
        self.inner
            .paint(bounds, content, focused, enabled, canvas, style);
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &windui::event::Event) -> bool {
        let used = self.inner.on_event(ctx, ev);
        if used {
            self.clamp(ctx);
        }
        used
    }

    fn cursor(&self) -> CursorShape {
        self.inner.cursor()
    }

    /// 面板被禁用（锁定）或隐藏（窄窗 / 无边框）时，框架调这里清交互态。
    ///
    /// `SplitHandle` 没有公开的重置口子：`dragging` / `hover` 是私有字段，禁用后
    /// `Up` 被核心层丢掉，这两个位会永远冻在 true——复显时描边亮着，下次按下还会
    /// 先进拖动分支。重建一枚新控件是唯一能同时清掉两者的办法（比例信号照旧共享）。
    fn reset_interaction(&mut self) {
        self.inner = SplitHandle::new(Axis::Horizontal, self.chrome.info.ratio, self.opts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_width_follows_share_and_caps() {
        assert_eq!(panel_max_px(600), 300);
        assert_eq!(panel_max_px(1000), 500);
        assert_eq!(panel_max_px(1200), 600);
        assert_eq!(panel_max_px(2000), 1000);
        assert_eq!(panel_max_px(10_000), 4096, "绝对上限只防天文数字");
    }

    #[test]
    fn clamp_keeps_panel_inside_240_4096_and_share() {
        assert_eq!(clamp_panel_px(100, 1000), 240);
        assert_eq!(clamp_panel_px(999, 1000), 500);
        assert_eq!(clamp_panel_px(350, 1000), 350);
    }

    #[test]
    fn ratio_and_px_round_trip() {
        for px in [240, 300, 480, 900] {
            let cw = 2000;
            let r = ratio_for_panel(px, cw);
            assert!((panel_px(r, cw) - px).abs() <= 1, "px={px} 往返误差过大");
        }
    }

    #[test]
    fn panel_visible_respects_open_borderless_and_width() {
        assert!(!panel_visible(false, false, 1200));
        assert!(!panel_visible(true, true, 1200));
        assert!(!panel_visible(true, false, 599));
        assert!(panel_visible(true, false, 600));
        assert!(panel_visible(true, false, 0), "首帧宽未知先按够宽");
    }

    #[test]
    fn clamp_ratio_stops_at_both_edges() {
        let cw = 1000;
        assert_eq!(panel_px(clamp_ratio(0.10, cw), cw), 500);
        assert_eq!(panel_px(clamp_ratio(0.90, cw), cw), 240);
        assert_eq!(panel_px(clamp_ratio(0.30, cw), cw), 500);
    }

    #[test]
    fn properties_card_toggles_and_persists() {
        crate::ui::chrome::set_borderless(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(
            crate::core::config::Config::default(),
        ));
        let info = InfoPanel::new();
        assert!(!info.panel_visible());
        assert!(info.open_properties());
        assert!(info.open.get());
        assert!(info.info_active.get());
        assert!(info.info_expanded.get());
        info.persist(&cfg);
        assert!(cfg.borrow().info_panel);
        assert_eq!(cfg.borrow().info_cards, vec!["info".to_string()]);
        assert!(info.toggle_properties());
        info.persist(&cfg);
        assert_eq!(cfg.borrow().info_cards_collapsed, vec!["info".to_string()]);
        assert!(info.close_properties());
        info.persist(&cfg);
        assert!(cfg.borrow().info_cards.is_empty());
        assert!(cfg.borrow().info_cards_collapsed.is_empty());
    }

    /// 入口打开面板并把偏好写回配置：菜单 / 快捷键 / 右键三处共用这一条路径。
    #[test]
    fn open_properties_opens_panel_and_marks_dirty() {
        crate::ui::chrome::set_borderless(false);
        let chrome = chrome::Chrome::new(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(
            crate::core::config::Config::default(),
        ));
        assert!(!chrome.info.open.get());
        open_properties(None, &chrome, &cfg);
        assert!(chrome.info.open.get());
        assert!(chrome.info.info_active.get());
        assert!(chrome.info.info_expanded.get());
        assert!(cfg.borrow().info_panel);
        assert_eq!(cfg.borrow().info_cards, vec!["info".to_string()]);
    }

    /// 无边框形态不打开面板、也不改配置：那形态下没有标题条可拖，面板开了只能进不能出。
    #[test]
    fn open_properties_ignored_in_borderless() {
        crate::ui::chrome::set_borderless(true);
        let chrome = chrome::Chrome::new(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(
            crate::core::config::Config::default(),
        ));
        open_properties(None, &chrome, &cfg);
        assert!(!chrome.info.open.get());
        assert!(!cfg.borrow().info_panel);
        // 还原全局镜像，免得影响同进程里的其他用例。
        crate::ui::chrome::set_borderless(false);
    }

    /// 建窗种子比例也要守半宽上限（与 `tick` 同一套像素换算）。
    ///
    /// 旧写法只把比例夹到 [0, 1]：1000 宽窗里配 480 的目标宽，首帧会摆出 48% 的
    /// 面板，下一拍再跳回半宽——用户看得见那一跳。这里按 `clamp_panel_px` 现算，
    /// 断言种子比例反算回来就是**夹后的**像素宽。
    #[test]
    fn init_from_caps_the_seed_ratio_at_the_share_limit() {
        let cfg = crate::core::config::Config {
            window_w: 1000,
            info_panel: true,
            info_panel_width: 480,
            ..crate::core::config::Config::default()
        };
        let info = InfoPanel::new();
        info.init_from(&cfg);
        let approx = 1000 - 16;
        assert_eq!(panel_max_px(approx), 492, "1000 宽窗的半宽上限");
        assert_eq!(
            panel_px(info.ratio.get(), approx),
            clamp_panel_px(480, approx),
            "种子比例必须已经夹进 240..=窄窗半宽"
        );
    }

    #[test]
    fn init_from_seeds_session_state() {
        // ★ 整块构造而不是 `default()` 后逐字段赋值：后者会被
        //   `clippy::field_reassign_with_default` 在 `2_scan.bat` 的 `-D warnings` 上判红。
        let cfg = crate::core::config::Config {
            info_panel: true,
            info_panel_width: 420,
            info_cards: vec!["info".into()],
            info_cards_collapsed: vec!["info".into()],
            ..crate::core::config::Config::default()
        };
        let info = InfoPanel::new();
        info.init_from(&cfg);
        assert!(info.open.get());
        assert_eq!(info.width.get(), 420);
        assert!(info.info_active.get());
        assert!(!info.info_expanded.get());
    }

    /// 借一棵一次性控件树把 `InfoPanel::tick` 跑一拍：`tick` 要 `&mut EventCtx`，
    /// 而 `EventCtx` 外部不可构造，只能走真实指针事件（按下 + 抬起 = 一次点击）。
    fn run_tick(info: &std::rc::Rc<InfoPanel>, cfg: &Rc<RefCell<Config>>, content_w: i32) {
        use windui::core::Tree;
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::Point;
        use windui::text::NullTextEngine;

        let info = info.clone();
        let cfg = cfg.clone();
        let root = Element::col()
            .width(200)
            .height(100)
            .child(Element::button("tick").on_click(move |ctx| info.tick(ctx, content_w, &cfg)));
        let mut tree = Tree::new();
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(200, 100), &mut NullTextEngine);
        let btn = tree.get(id).unwrap().children[0];
        let b = tree.abs_bounds(btn);
        let center = Point::new(b.x + b.w / 2, b.y + b.h / 2);
        let (mut hover, mut capture) = (None, None);
        for kind in [PointerKind::Down, PointerKind::Up] {
            tree.dispatch_pointer(
                PointerEvent::single(kind, center, MouseButton::Left),
                &mut hover,
                &mut capture,
            );
        }
    }

    /// 视觉 1px 的线也要好抓：左右各 8px 命中扩展，且顶部标题栏那段必须裁掉。
    ///
    /// 这个测试同时钉住 windui 补丁 030 的两半：`hit_pad` 扩命中、`raised` 让
    /// 扩展区赢过相邻面板（否则面板侧的 8px 会被面板节点先接住）。
    #[test]
    fn split_handle_hit_pad_reaches_both_sides_of_the_line() {
        use windui::core::Tree;
        use windui::geometry::{Point, Size};
        use windui::text::NullTextEngine;

        let ch = chrome::Chrome::new(false);
        let root = Element::row()
            .width(300)
            .height(100)
            .cross(Align::Stretch)
            .child(Element::col().weight(1.0).bg(tokens::empty_bg()))
            .child(
                Element::leaf()
                    .widget(PanelSplitHandle::new(ch))
                    .width(HANDLE_W)
                    .height_match()
                    .hit_pad_edges(Insets {
                        left: HANDLE_HIT_PAD,
                        top: -chrome::STRIP_H,
                        right: HANDLE_HIT_PAD,
                        bottom: 0,
                    })
                    .raised(),
            )
            .child(Element::col().weight(1.0).bg(tokens::tree_bg()));
        let mut tree = Tree::new();
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(300, 100), &mut NullTextEngine);
        let handle = tree.get(id).unwrap().children[1];
        let b = tree.abs_bounds(handle);
        assert_eq!(b.w, HANDLE_W, "视觉 / 布局仍只占 1px");
        assert_eq!(tree.hit_test(Point::new(b.x, 60)), Some(handle));
        assert_eq!(
            tree.hit_test(Point::new(b.x - HANDLE_HIT_PAD, 60)),
            Some(handle),
            "画布侧 8px 也要能抓"
        );
        assert_eq!(
            tree.hit_test(Point::new(b.x + HANDLE_W + HANDLE_HIT_PAD - 1, 60)),
            Some(handle),
            "面板侧 8px 也要能抓（raised 必须先于面板命中）"
        );
        assert_ne!(
            tree.hit_test(Point::new(b.x - HANDLE_HIT_PAD - 1, 60)),
            Some(handle),
            "扩展区外不再归分隔条"
        );
        assert_ne!(
            tree.hit_test(Point::new(b.x, chrome::STRIP_H - 1)),
            Some(handle),
            "标题栏那段必须裁掉，关闭钮仍留在画布命中"
        );
    }

    /// 滚动条 track 右缘贴面板内边线（与文件树同一条），不是 windui 默认的 -11px。
    #[test]
    fn panel_scrollbar_track_aligns_with_the_tree_inner_edge() {
        use windui::core::{scrollbar, NodeId, Tree};
        use windui::text::NullTextEngine;

        fn find_scroll(tree: &Tree, id: NodeId) -> Option<NodeId> {
            let n = tree.get(id)?;
            if matches!(n.layout, windui::core::Layout::Scroll) {
                return Some(id);
            }
            n.children.iter().find_map(|&c| find_scroll(tree, c))
        }

        let chrome = chrome::Chrome::new(false);
        let cfg = Rc::new(RefCell::new(Config::default()));
        let sig = properties::meta_signals(9_997);
        let panel = build_one(&chrome, &cfg, &sig, PanelColors::dark());
        let mut tree = Tree::new();
        let root = panel.build(&mut tree);
        tree.root = Some(root);
        tree.layout_root(Size::new(300, 600), &mut NullTextEngine);
        let scroll = find_scroll(&tree, root).expect("面板里要有 scroll");
        let b = tree.abs_bounds(scroll);
        let rb = tree.abs_bounds(root);
        assert_eq!(
            b.x + b.w,
            rb.x + rb.w + SCROLLBAR_SHIFT,
            "滚动节点向右多占 3px，抵消 windui 轨道外边距"
        );
        let track_x = b.x + b.w
            - scrollbar::MARGIN as i32
            - scrollbar::TRACK_W as i32
            - scrollbar::WINDOW_EDGE_INSET;
        let track_right = track_x + scrollbar::TRACK_W as i32;
        assert_eq!(
            track_right,
            rb.x + rb.w - 8,
            "track 右缘要和文件树/缩略图条落在同一条内边线"
        );
        drop(cfg);
        drop(chrome);
        properties::forget_window(9_997);
    }

    /// 卡片 id / 文案是菜单与 ini 的共同真源，顺序也不能漂。
    #[test]
    fn card_ids_and_labels_are_stable() {
        assert_eq!(Card::ALL.map(|c| c.id()), [CARD_INFO]);
        assert_eq!(Card::ALL.map(|c| c.label()), ["图片信息"]);
    }

    /// 卡片的开关 / 折叠必须能完整落进 `info_cards` 再读回来。
    #[test]
    fn card_activation_and_collapse_round_trip_through_config() {
        let info = InfoPanel::new();
        info.open_card(Card::Info);
        info.toggle_card(Card::Info);

        let cfg = Rc::new(RefCell::new(Config::default()));
        info.persist(&cfg);
        assert_eq!(cfg.borrow().info_cards, vec![CARD_INFO.to_string()]);
        assert_eq!(
            cfg.borrow().info_cards_collapsed,
            vec![CARD_INFO.to_string()]
        );

        let restored = InfoPanel::new();
        restored.init_from(&cfg.borrow());
        assert!(restored.info_active.get());
        assert!(!restored.info_expanded.get());
    }

    /// 旧 `histogram` 卡 id 已退役：读到它既不能让任何卡亮起，也不能把配置写回去。
    #[test]
    fn retired_histogram_id_is_ignored() {
        let cfg = Config {
            info_cards: vec!["histogram".into(), "info".into()],
            info_cards_collapsed: vec!["histogram".into()],
            ..Config::default()
        };

        let cfg = Rc::new(RefCell::new(cfg));
        let info = InfoPanel::new();
        info.init_from(&cfg.borrow());
        assert!(info.info_active.get(), "同一条配置里的 info 仍要生效");
        assert!(info.info_expanded.get(), "histogram 的折叠态不许串到 info");

        info.persist(&cfg);
        assert_eq!(cfg.borrow().info_cards, vec![CARD_INFO.to_string()]);
        assert!(
            cfg.borrow().info_cards_collapsed.is_empty(),
            "退役 id 不许再写回配置"
        );
    }

    /// 「图片信息」卡要真的把面板内容撑高；折叠后要变矮。
    #[test]
    fn info_card_adds_height_and_collapses() {
        use windui::core::{NodeId, Tree};
        use windui::text::NullTextEngine;

        fn find_scroll(tree: &Tree, id: NodeId) -> Option<NodeId> {
            let n = tree.get(id)?;
            if matches!(n.layout, windui::core::Layout::Scroll) {
                return Some(id);
            }
            n.children.iter().find_map(|&c| find_scroll(tree, c))
        }

        let chrome = chrome::Chrome::new(false);
        let cfg = Rc::new(RefCell::new(Config::default()));
        let sig = properties::meta_signals(9_998);
        let panel = build_one(&chrome, &cfg, &sig, PanelColors::dark());
        let mut tree = Tree::new();
        let root = panel.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(300, 600);
        tree.layout_root(size, &mut NullTextEngine);
        let scroll = find_scroll(&tree, root).expect("面板里要有 scroll");
        let h_empty = tree.get(scroll).unwrap().content_h;

        chrome.info.open_card(Card::Info);
        tree.layout_root(size, &mut NullTextEngine);
        let h_open = tree.get(scroll).unwrap().content_h;
        assert!(h_open > h_empty, "图片信息卡应撑高内容");

        chrome.info.toggle_card(Card::Info);
        tree.layout_root(size, &mut NullTextEngine);
        let h_collapsed = tree.get(scroll).unwrap().content_h;
        assert!(h_collapsed < h_open, "折叠图片信息卡后内容应变矮");
        properties::forget_window(9_998);
    }

    /// 插件输出卡要真的进面板，并在换图时清空。
    #[test]
    fn plugin_card_adds_height_and_clears_on_image_scope_change() {
        use windui::core::{NodeId, Tree};
        use windui::text::NullTextEngine;

        fn find_scroll(tree: &Tree, id: NodeId) -> Option<NodeId> {
            let n = tree.get(id)?;
            if matches!(n.layout, windui::core::Layout::Scroll) {
                return Some(id);
            }
            n.children.iter().find_map(|&c| find_scroll(tree, c))
        }

        let chrome = chrome::Chrome::new(false);
        let cfg = Rc::new(RefCell::new(Config::default()));
        let sig = properties::meta_signals(9_996);
        let panel = build_one(&chrome, &cfg, &sig, PanelColors::dark());
        let mut tree = Tree::new();
        let root = panel.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(300, 600);
        tree.layout_root(size, &mut NullTextEngine);
        let scroll = find_scroll(&tree, root).expect("面板里要有 scroll");
        let h_empty = tree.get(scroll).unwrap().content_h;

        chrome.info.push_plugin_card(plugin_output::PluginCard {
            key: "levels".into(),
            title: "色阶分析".into(),
            expanded: true,
            blocks: vec![plugin_output::PluginBlock::Text {
                title: Some("摘要".into()),
                text: "均值 128\n中位 130".into(),
            }],
        });
        tree.layout_root(size, &mut NullTextEngine);
        let h_card = tree.get(scroll).unwrap().content_h;
        assert!(h_card > h_empty, "插件输出卡要把面板内容撑高");

        chrome.info.toggle_plugin_card("levels");
        tree.layout_root(size, &mut NullTextEngine);
        let h_collapsed = tree.get(scroll).unwrap().content_h;
        assert!(h_collapsed < h_card, "折叠插件卡后内容应变矮");
        assert!(
            h_collapsed > h_empty,
            "收起后仍留卡头（标题行），不是整张卡消失"
        );

        chrome.info.toggle_plugin_card("levels");
        tree.layout_root(size, &mut NullTextEngine);
        assert_eq!(
            tree.get(scroll).unwrap().content_h,
            h_card,
            "再点一次展开回原高度"
        );

        chrome
            .info
            .sync_plugin_scope(Some(std::path::Path::new("another.png")));
        tree.layout_root(size, &mut NullTextEngine);
        let h_cleared = tree.get(scroll).unwrap().content_h;
        assert_eq!(h_cleared, h_empty, "换图后插件输出要清空");
        properties::forget_window(9_996);
    }

    /// 卡头的 ✕ 不得落进滚动条命中区：内容溢出时，滚动条会先吞掉那次点击。
    ///
    /// 真机复现（2026-11-14）：插件卡把面板撑出滚动条后，✕ 一点就变成翻页。
    #[test]
    fn card_close_button_clears_the_scrollbar_hit_zone() {
        use windui::core::{NodeId, Tree};
        use windui::text::NullTextEngine;

        fn find_scroll(tree: &Tree, id: NodeId) -> Option<NodeId> {
            let n = tree.get(id)?;
            if matches!(n.layout, windui::core::Layout::Scroll) {
                return Some(id);
            }
            n.children.iter().find_map(|&c| find_scroll(tree, c))
        }
        fn collect(tree: &Tree, id: NodeId, out: &mut Vec<NodeId>) {
            out.push(id);
            if let Some(n) = tree.get(id) {
                for &c in &n.children {
                    collect(tree, c, out);
                }
            }
        }

        let chrome = chrome::Chrome::new(false);
        let cfg = Rc::new(RefCell::new(Config::default()));
        let sig = properties::meta_signals(9_995);
        let panel = build_one(&chrome, &cfg, &sig, PanelColors::dark());
        let mut tree = Tree::new();
        let root = panel.build(&mut tree);
        tree.root = Some(root);
        tree.layout_root(Size::new(300, 600), &mut NullTextEngine);

        chrome.info.push_plugin_card(plugin_output::PluginCard {
            key: "levels".into(),
            title: "色阶分析".into(),
            expanded: true,
            // 图片块有确定高度（80..480），用来把内容撑出滚动条；
            // NullTextEngine 对多行长文本的测量不可靠，不能拿文本当“撑高器”。
            blocks: (0..8)
                .map(|_| plugin_output::PluginBlock::Image {
                    title: None,
                    width: 1,
                    height: 1,
                    rgba: std::sync::Arc::new(vec![255, 255, 255, 255]),
                })
                .collect(),
        });
        tree.layout_root(Size::new(300, 600), &mut NullTextEngine);

        let scroll = find_scroll(&tree, root).expect("面板里要有 scroll");
        let scroll_bounds = tree.abs_bounds(scroll);
        let (hit_x, _) = tree.scrollbar_hit_zone(scroll_bounds);
        assert!(
            tree.get(scroll).unwrap().content_h > scroll_bounds.h,
            "前置：内容要溢出，滚动条命中区才生效（content_h={} h={}）",
            tree.get(scroll).unwrap().content_h,
            scroll_bounds.h
        );

        let mut ids = Vec::new();
        collect(&tree, root, &mut ids);
        let mut checked = 0;
        for id in ids {
            let b = tree.abs_bounds(id);
            if b.w == 22 && b.h == 22 {
                checked += 1;
                assert!(
                    b.x + b.w <= hit_x,
                    "22×22 按钮右缘 {} 落进滚动条命中区（左缘 {hit_x}）",
                    b.x + b.w
                );
            }
        }
        assert!(checked >= 1, "至少要找到卡头的 ✕");
        properties::forget_window(9_995);
    }
    /// 首拍可见：按目标宽反算比例，不用 `init_from` 拿旧窗宽估的那一份。
    #[test]
    fn tick_first_visible_applies_target_width() {
        crate::ui::chrome::set_borderless(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(crate::core::config::Config {
            info_panel: true,
            info_panel_width: 300,
            ..crate::core::config::Config::default()
        }));
        let info = std::rc::Rc::new(InfoPanel::new());
        info.init_from(&cfg.borrow());
        assert!(info.visible.get());
        run_tick(&info, &cfg, 1200);
        assert_eq!(info.content_w.get(), 1200);
        assert_eq!(info.width.get(), 300, "首拍不改变目标宽");
        assert!((info.ratio.get() - ratio_for_panel(300, 1200)).abs() < 1e-6);
    }

    /// 用户拖过左端：比例夹回半宽上限，并把夹后的像素记成新的目标宽、当场落配置。
    #[test]
    fn tick_drag_clamps_width_and_persists() {
        crate::ui::chrome::set_borderless(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(crate::core::config::Config {
            info_panel: true,
            info_panel_width: 300,
            ..crate::core::config::Config::default()
        }));
        let info = std::rc::Rc::new(InfoPanel::new());
        info.init_from(&cfg.borrow());
        info.visible.set(true);
        info.was_visible.set(true);
        info.content_w.set(1200);
        info.ratio.set(0.1);
        info.last_ratio.set(0.9);
        run_tick(&info, &cfg, 1200);
        assert_eq!(info.width.get(), 600, "1200 宽下上限 = 半宽 600");
        assert_eq!(cfg.borrow().info_panel_width, 600, "拖动后当场写回配置");
        assert!((info.ratio.get() - ratio_for_panel(600, 1200)).abs() < 1e-6);
    }

    /// 窗口缩窄：比例跟着重算，但目标宽保留用户选的那一档（拉宽后回来）。
    #[test]
    fn tick_resize_keeps_target_width() {
        crate::ui::chrome::set_borderless(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(crate::core::config::Config {
            info_panel: true,
            info_panel_width: 300,
            ..crate::core::config::Config::default()
        }));
        let info = std::rc::Rc::new(InfoPanel::new());
        info.init_from(&cfg.borrow());
        info.visible.set(true);
        info.was_visible.set(true);
        info.content_w.set(1200);
        info.ratio.set(ratio_for_panel(300, 1200));
        info.last_ratio.set(info.ratio.get());
        run_tick(&info, &cfg, 800);
        assert_eq!(info.content_w.get(), 800);
        assert_eq!(info.width.get(), 300, "缩放不改目标宽");
        assert_eq!(cfg.borrow().info_panel_width, 300, "缩放不写配置");
        assert!((info.ratio.get() - ratio_for_panel(300, 800)).abs() < 1e-6);
    }

    /// 属性卡收起 / 关闭都要真的把面板变矮：卡片折叠只留标题行，关闭回到空态。
    ///
    /// 判据量的是**滚动内容高**而不是外框：外框由 `weight(1.0)` 吃掉剩余高度，
    /// 永远不变；用户看到的"面板变矮"其实是内容高度在变。
    #[test]
    fn panel_layout_shrinks_when_card_collapses_or_closes() {
        use windui::core::{NodeId, Tree};
        use windui::text::NullTextEngine;

        fn find_scroll(tree: &Tree, id: NodeId) -> Option<NodeId> {
            let n = tree.get(id)?;
            if matches!(n.layout, windui::core::Layout::Scroll) {
                return Some(id);
            }
            n.children.iter().find_map(|&c| find_scroll(tree, c))
        }

        let chrome = chrome::Chrome::new(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(
            crate::core::config::Config::default(),
        ));
        let sig = properties::meta_signals(9_999);
        let panel = build_one(&chrome, &cfg, &sig, super::tokens::PanelColors::dark());

        let mut tree = Tree::new();
        let root = panel.build(&mut tree);
        tree.root = Some(root);
        let size = windui::geometry::Size::new(300, 600);
        tree.layout_root(size, &mut NullTextEngine);
        let scroll = find_scroll(&tree, root).expect("面板里要有 scroll");
        let h_empty = tree.get(scroll).unwrap().content_h;

        chrome.info.info_active.set(true);
        chrome.info.info_expanded.set(true);
        tree.layout_root(size, &mut NullTextEngine);
        let h_expanded = tree.get(scroll).unwrap().content_h;

        chrome.info.info_expanded.set(false);
        tree.layout_root(size, &mut NullTextEngine);
        let h_collapsed = tree.get(scroll).unwrap().content_h;

        chrome.info.info_active.set(false);
        tree.layout_root(size, &mut NullTextEngine);
        let h_closed = tree.get(scroll).unwrap().content_h;

        assert!(h_expanded > h_collapsed, "展开应比折叠高");
        assert!(h_collapsed > h_empty, "折叠卡仍占标题行");
        assert_eq!(h_closed, h_empty, "关闭卡片应回到空态高度");
    }
}
