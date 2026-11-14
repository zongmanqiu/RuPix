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
use windui::geometry::{Rect, Size};
use windui::prelude::*;
use windui::render::{Canvas, Paint};
use windui::signal::{signal, Signal};
use windui::style::Style;
use windui::text::{TextEngine, TextStyle};
use windui::ui::split::{SplitHandle, SplitOpts};

use self::properties::MetaSignals;
use super::chrome;
use super::singleton;
use super::tokens::{self, PanelColors};
use crate::core::config::{self, Config, INFO_PANEL_W_DEFAULT, INFO_PANEL_W_MAX, INFO_PANEL_W_MIN};
use crate::core::pixels::Histogram;

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

/// 属性卡 id（文件身份 + EXIF）。
pub const CARD_PROPERTIES: &str = "properties";
/// 技术信息卡 id（解码器 / 位深 / 像素格式 / ICC / 方向 / 显示状态）。
pub const CARD_TECHNICAL: &str = "technical";
/// 直方图卡 id（RGB + 亮度 + 裁切比例）。
pub const CARD_HISTOGRAM: &str = "histogram";

/// 信息面板里的一张卡。
///
/// `ALL` 同时是**菜单顺序**与**持久化顺序**的唯一真源：加一张卡只改这里与
/// `build_one` 的装配，不再到处抄一份字符串列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Card {
    Properties,
    Technical,
    Histogram,
}

impl Card {
    pub const ALL: [Card; 3] = [Card::Properties, Card::Technical, Card::Histogram];

    /// ini 里的卡片 id（`info_cards` / `info_cards_collapsed` 共用）。
    pub fn id(self) -> &'static str {
        match self {
            Card::Properties => CARD_PROPERTIES,
            Card::Technical => CARD_TECHNICAL,
            Card::Histogram => CARD_HISTOGRAM,
        }
    }

    /// 卡片标题与菜单项文案。
    pub fn label(self) -> &'static str {
        match self {
            Card::Properties => "属性",
            Card::Technical => "技术信息",
            Card::Histogram => "直方图",
        }
    }

    /// 这张卡在不在面板里。
    pub fn active(self, info: &InfoPanel) -> Signal<bool> {
        match self {
            Card::Properties => info.props_active,
            Card::Technical => info.tech_active,
            Card::Histogram => info.hist_active,
        }
    }

    /// 这张卡展不展开。
    pub fn expanded(self, info: &InfoPanel) -> Signal<bool> {
        match self {
            Card::Properties => info.props_expanded,
            Card::Technical => info.tech_expanded,
            Card::Histogram => info.hist_expanded,
        }
    }
}

/// 面板正文统一字体族。
///
/// ★ 中文与数字必须是同一字体：不指定时数字走 Segoe UI、中文回退微软雅黑，
///   两种字体的 ascent/descent 不同，同一个矩形内垂直居中后两列基线会差 1px；
///   放大看就是“右边比左边低一点”（2026-11-14 用户截图实测）。
pub const PANEL_FONT: &str = "Microsoft YaHei";

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
    /// 属性卡是否在面板里（对应 `info_cards`）。
    pub props_active: Signal<bool>,
    /// 属性卡是否展开（对应 `info_cards_collapsed` 的反面）。
    pub props_expanded: Signal<bool>,
    /// 技术信息卡是否在面板里。
    pub tech_active: Signal<bool>,
    pub tech_expanded: Signal<bool>,
    /// 直方图卡是否在面板里。
    pub hist_active: Signal<bool>,
    pub hist_expanded: Signal<bool>,
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
            props_active: signal(false),
            props_expanded: signal(true),
            tech_active: signal(false),
            tech_expanded: signal(true),
            hist_active: signal(false),
            hist_expanded: signal(true),
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
            let active = cfg.info_cards.iter().any(|id| id == card.id());
            let collapsed = cfg.info_cards_collapsed.iter().any(|id| id == card.id());
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
        self.open_card(Card::Properties)
    }

    /// 把属性卡从面板里拿掉。
    pub fn close_properties(&self) -> bool {
        self.close_card(Card::Properties)
    }

    /// 折叠 / 展开属性卡。
    pub fn toggle_properties(&self) -> bool {
        self.toggle_card(Card::Properties)
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
/// `colors` 是**唯一**取色入口：本函数（含 [`property_card`] / [`info_row`]）不读
/// `tk::*()`，否则主题切换那一帧会有一半控件停在旧色上。
///
/// 结构：`col[ 标题条(固定 STRIP_H) , scroll(weight 1)[ 空态提示 / 三张卡 ] ]`。
/// 标题条是**面板自己的标题**，不承担窗口标题栏职责：窗口五颗钮仍在画布顶条，
/// 面板头部只留「信息」和收起面板的 `<`。
///
/// 三张卡共用同一套卡片外壳（[`card_shell`]）：属性 / 技术信息 / 直方图，
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
                .font_size(13.0)
                .fg(colors.text)
                .padding_xy(10, 0)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(
            // 收起用 MDL2 的 ChevronLeft（与主窗标题栏同一套图标字体 / 14px）。
            Element::icon_button("\u{E76B}")
                .size(40, 40)
                .font_family(chrome::ICON_FONT)
                .font_size(14.0)
                .fg(colors.text)
                .tooltip("收起信息面板")
                .on_click(close_panel),
        );

    // 没有卡片时的空态：只写一句提示，不占位。`props_active` 为假时才显示，
    // 与卡片互斥——两者若同时可见，用户会以为卡片下面还挂着一张空卡。
    let empty = Element::label("右键「属性」或「本图」添加输出")
        .font_size(12.0)
        .fg(colors.muted)
        .padding(12)
        .visible_when({
            let chrome = chrome.clone();
            move || !chrome.info.any_active()
        });
    let plugin_cards = plugin_output::cards_element(chrome, colors);
    let properties = property_card(chrome, config, sig, colors);
    let technical = technical_card(chrome, config, sig, colors);
    let histogram = histogram_card(chrome, config, sig, colors);
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
                .child(properties)
                .child(technical)
                .child(histogram),
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

/// 属性卡：文件身份 + 拍摄信息。
fn property_card(
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    sig: &MetaSignals,
    colors: PanelColors,
) -> Element {
    let has_image = sig.has_image;

    // 无图只留一句“没有打开图片”；有图时这行隐藏、属性行整块出现。
    let no_image = Element::label("没有打开图片")
        .font_size(13.0)
        .fg(colors.muted)
        .visible_when(move || !has_image.get());

    let rows = Element::col()
        .width_match()
        .spacing(2)
        .child(info_row("文件名", sig.filename, 2, colors))
        .child(info_row("类型", sig.kind, 2, colors))
        .child(info_row("位置", sig.location, 3, colors))
        .child(info_row("大小", sig.size, 1, colors))
        .child(info_row("修改时间", sig.mtime, 1, colors))
        .child(info_row("图像尺寸", sig.dims, 1, colors))
        .child({
            let s = sig.anim;
            info_row("动画", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.capture_time;
            info_row("拍摄时间", s, 1, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.camera;
            info_row("相机", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.lens;
            info_row("镜头", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.exposure;
            info_row("曝光", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.focal;
            info_row("焦距", s, 1, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.gps;
            info_row("GPS", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .visible_when(move || has_image.get());

    let body = Element::col()
        .width_match()
        .bg(colors.content)
        .corner(6.0)
        .padding(8)
        .spacing(6)
        .child(no_image)
        .child(rows)
        .on_context_menu(copy_info_menu(
            "属性",
            vec![
                ("文件名", sig.filename),
                ("类型", sig.kind),
                ("位置", sig.location),
                ("大小", sig.size),
                ("修改时间", sig.mtime),
                ("图像尺寸", sig.dims),
                ("动画", sig.anim),
                ("拍摄时间", sig.capture_time),
                ("相机", sig.camera),
                ("镜头", sig.lens),
                ("曝光", sig.exposure),
                ("焦距", sig.focal),
                ("GPS", sig.gps),
            ],
        ));

    simple_card(Card::Properties, chrome, config, colors, body)
}

/// 技术信息卡：解码器 / 位深 / 像素格式 / ICC / 方向 / 当前显示状态。
fn technical_card(
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    sig: &MetaSignals,
    colors: PanelColors,
) -> Element {
    let frames = sig.tech_frames;
    let rows = Element::col()
        .width_match()
        .spacing(2)
        .child(info_row("解码器", sig.tech_decoder, 2, colors))
        .child(info_row("格式", sig.tech_format, 1, colors))
        .child(info_row("像素", sig.tech_pixels, 2, colors))
        .child(info_row("像素格式", sig.tech_color, 2, colors))
        .child(info_row("色彩配置", sig.tech_icc, 2, colors))
        .child(info_row("方向", sig.tech_orientation, 1, colors))
        .child(optional_row("显示", sig.tech_view, 2, colors))
        .child(optional_row("帧/页", frames, 1, colors));
    let body = Element::col()
        .width_match()
        .bg(colors.content)
        .corner(6.0)
        .padding(8)
        .spacing(6)
        .child(rows)
        .on_context_menu(copy_info_menu(
            "技术信息",
            vec![
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
    simple_card(Card::Technical, chrome, config, colors, body)
}

/// 直方图卡：RGB + 亮度四条，底部一行裁切统计。
fn histogram_card(
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    sig: &MetaSignals,
    colors: PanelColors,
) -> Element {
    let summary = sig.hist_summary;
    let copy = move || {
        let body = format!("直方图\n{}", summary.get());
        vec![MenuItem::run(
            "复制直方图数据",
            move |ctx: &mut EventCtx| match crate::platform::clipboard::copy_text(&body) {
                Ok(()) => ctx.toast_ok("已复制到剪贴板"),
                Err(e) => ctx.toast_err(format!("复制失败：{e}")),
            },
            false,
        )]
    };
    let body = Element::col()
        .width_match()
        .bg(colors.content)
        .corner(6.0)
        .padding(8)
        .spacing(6)
        .child(
            Element::leaf()
                .widget(HistogramView::new(sig.histogram, colors))
                .width_match(),
        )
        .child(
            Element::label_signal(sig.hist_summary)
                .font_size(11.0)
                .fg(colors.muted)
                .max_lines(4),
        )
        .on_context_menu(copy);
    simple_card(Card::Histogram, chrome, config, colors, body)
}

/// 三张卡共用的外壳：标题行（箭头 + 标题，点击折叠） + 关闭按钮 + 正文。
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
                .font_size(12.0)
                .fg(colors.text)
                .visible_signal(expanded),
        )
        .child(
            Element::label("▸")
                .font_size(12.0)
                .fg(colors.text)
                .visible_when(move || !expanded.get()),
        )
        .child(
            Element::label(card.label())
                .font_size(13.0)
                .font_weight(600)
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
                .font_size(11.0)
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
fn optional_row(
    label: &'static str,
    value: Signal<String>,
    max_lines: usize,
    colors: PanelColors,
) -> Element {
    info_row(label, value, max_lines, colors).visible_when(move || !value.get().is_empty())
}

/// 直方图画布：四条平滑曲线叠在同一绘图区，共用纵轴（满格 = 全局峰值）。
///
/// ★ 高度跟着宽度按 16:9 长（[`hist_height`]）：面板从 240 拉到半屏后，图不是
///   一条窄缝，而是能看清峰值与溢出的曲线图。
struct HistogramView {
    data: Signal<Histogram>,
    colors: PanelColors,
}

impl HistogramView {
    fn new(data: Signal<Histogram>, colors: PanelColors) -> Self {
        Self { data, colors }
    }
}

/// 直方图高度：宽度 × 9/16，夹在 160..=480。
fn hist_height(width: i32) -> i32 {
    (width.max(160) * 9 / 16).clamp(160, 480)
}

/// 绘图区：左 4 / 右 40（纵轴百分比）/ 上 20（图例）/ 下 18（横轴刻度）。
fn hist_plot_rect(bounds: Rect) -> Rect {
    Rect::new(
        bounds.x + 4,
        bounds.y + 20,
        (bounds.w - 44).max(24),
        (bounds.h - 38).max(24),
    )
}

impl Widget for HistogramView {
    fn measure(&self, avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        Size::new(avail.w.max(1), hist_height(avail.w))
    }

    fn paint(
        &self,
        bounds: Rect,
        _content: Rect,
        _focused: bool,
        _enabled: bool,
        canvas: &mut dyn Canvas,
        _style: &Style,
    ) {
        let hist = self.data.get();
        let plot = hist_plot_rect(bounds);
        let ts = TextStyle::new(10.0);
        let muted = self.colors.muted;

        canvas.fill_rect(
            plot.x as f32,
            plot.y as f32,
            plot.w as f32,
            plot.h as f32,
            &Paint::fill(self.colors.hist_track),
        );
        // 5 横 + 5 纵，与 levels 插件同一套刻度：0/64/128/192/255。
        for k in 0..=4 {
            let y = plot.y + plot.h * k / 4;
            canvas.fill_rect(
                plot.x as f32,
                y as f32,
                plot.w as f32,
                1.0,
                &Paint::fill(self.colors.hist_grid),
            );
            let x = plot.x + plot.w * k / 4;
            canvas.fill_rect(
                x as f32,
                plot.y as f32,
                1.0,
                plot.h as f32,
                &Paint::fill(self.colors.hist_grid),
            );
        }

        if hist.count == 0 {
            canvas.draw_text(
                "没有可统计的像素",
                Rect::new(plot.x, plot.y + plot.h / 2 - 7, plot.w, 14),
                muted,
                Align::Center,
                &ts,
            );
            return;
        }

        // 端点（纯黑/纯白）常常是黑边、透明垫或裁切溢出；拿全局峰值当纵轴会被
        // 一根 0 档高柱压扁整张图。曲线按“去掉 0/255 后的最高档”缩放，端点尖峰
        // 允许截顶；精确的阴影/高光比例仍在摘要里照实写。
        let peak = visual_peak(&hist) as f32;
        let step = (plot.w / 140).max(1);
        for (bins, color) in [
            (&hist.r, self.colors.hist_r),
            (&hist.g, self.colors.hist_g),
            (&hist.b, self.colors.hist_b),
            (&hist.luma, self.colors.hist_luma),
        ] {
            // 端点不参与平滑/曲线：否则一根 0 档高柱会糊成一段贴顶的斜坡。
            let mut curve_bins = *bins;
            curve_bins[0] = 0;
            curve_bins[255] = 0;
            let smooth = smooth_bins(&curve_bins);
            let mut pts = Vec::with_capacity((plot.w / step + 2) as usize);
            let mut x = 0;
            while x <= plot.w {
                let t = x as f32 / plot.w.max(1) as f32 * 255.0;
                let v = sample_smooth(&smooth, t);
                let dh = ((v / peak) * plot.h as f32).round().min(plot.h as f32) as i32;
                let y = plot.y + plot.h - dh;
                pts.push((x as f32 + plot.x as f32, y as f32));
                x += step;
            }
            canvas.draw_polyline(&pts, 1.5, &Paint::fill(color));

            // 端点尖峰超过纵轴时画一根贴边竖线（截顶标记），精确比例见摘要。
            for (edge, x) in [(0usize, plot.x + 1), (255, plot.x + plot.w - 2)] {
                if bins[edge] as f32 > peak {
                    canvas.draw_line(
                        x as f32,
                        plot.y as f32,
                        x as f32,
                        (plot.y + plot.h) as f32,
                        1.5,
                        &Paint::fill(color),
                    );
                }
            }
        }

        // 图例：短色条 + 字母，与曲线颜色一一对应。
        let mut lx = bounds.x + 4;
        for (label, color) in [
            ("R", self.colors.hist_r),
            ("G", self.colors.hist_g),
            ("B", self.colors.hist_b),
            ("L", self.colors.hist_luma),
        ] {
            canvas.fill_rect(
                lx as f32,
                (bounds.y + 7) as f32,
                9.0,
                2.0,
                &Paint::fill(color),
            );
            canvas.draw_text(
                label,
                Rect::new(lx + 12, bounds.y, 18, 15),
                muted,
                Align::Start,
                &ts,
            );
            lx += 34;
        }

        // 纵轴：右侧标占全图有效像素的百分比，顶线 = 峰值箱占比。
        let max_pct = peak as f64 / hist.count as f64 * 100.0;
        for k in 0..=4 {
            let y = plot.y + plot.h - plot.h * k / 4;
            canvas.draw_text(
                &format!("{:.1}%", max_pct * k as f64 / 4.0),
                Rect::new(plot.x + plot.w + 3, y - 7, 37, 14),
                muted,
                Align::Start,
                &ts,
            );
        }

        // 横轴：0/64/128/192/255，5 档刻度。
        for (k, s) in [(0, "0"), (1, "64"), (2, "128"), (3, "192"), (4, "255")] {
            let x = plot.x + plot.w * k / 4;
            canvas.draw_text(
                s,
                Rect::new(x - 16, plot.y + plot.h + 2, 32, 14),
                muted,
                Align::Center,
                &ts,
            );
        }
    }
}

/// 曲线纵轴的峰值：忽略 0 / 255 两个端点的最高档。
///
/// 端点是“裁切溢出”的归集档：黑边、透明垫或过曝会把一根柱顶到 20%+，
/// 而照片主体常常不到 1%。按端点峰值缩放等于把其余曲线压成一条平线。
/// 端点本身仍会画（截顶），精确比例由摘要给出。全图只有端点时退回全局峰值。
fn visual_peak(hist: &Histogram) -> u32 {
    let mut best = 0;
    for bins in [&hist.r, &hist.g, &hist.b, &hist.luma] {
        for &c in &bins[1..255] {
            best = best.max(c);
        }
    }
    if best == 0 {
        hist.global_peak().max(1)
    } else {
        best
    }
}

/// 5 抽头高斯平滑：256 档原始箱直接画会全是锯齿；平滑只用于曲线，
/// 裁切百分比仍按原始箱算。
fn smooth_bins(bins: &[u32; 256]) -> [f32; 256] {
    const W: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
    let mut out = [0.0f32; 256];
    for (i, slot) in out.iter_mut().enumerate() {
        let mut acc = 0.0;
        let mut wsum = 0.0;
        for (k, &wk) in W.iter().enumerate() {
            let j = (i as isize + k as isize - 2).clamp(0, 255) as usize;
            acc += bins[j] as f32 * wk;
            wsum += wk;
        }
        *slot = acc / wsum;
    }
    out
}

/// 在平滑后的 256 档上线性取样；两端夹紧，越界不会 panic。
fn sample_smooth(smoothed: &[f32; 256], t: f32) -> f32 {
    let t = t.clamp(0.0, 255.0);
    let i = t.floor() as usize;
    let f = t - i as f32;
    let a = smoothed[i];
    let b = smoothed[(i + 1).min(255)];
    a + (b - a) * f
}

/// 一行「标签 + 值」。`max_lines == 1` 时单行截断（大小 / 时间这类定长值），
/// 文件名、路径、EXIF 文本允许多行——中文路径折行比截断更好认。
fn info_row(
    label: &'static str,
    value: Signal<String>,
    max_lines: usize,
    colors: PanelColors,
) -> Element {
    let text = Element::label_signal(value)
        .font_size(13.0)
        .fg(colors.text)
        .width_match();
    let text = if max_lines == 1 {
        text.max_lines(1).truncate(Truncate::End)
    } else {
        text.max_lines(max_lines)
    };
    Element::row()
        .width_match()
        // 行头与内容顶部对齐（2026-11-14 用户口径）：多行值时行头贴第一行。
        .cross(Align::Start)
        .spacing(6)
        .child(
            Element::label(label)
                .width(60)
                .font_size(13.0)
                .font_family(PANEL_FONT)
                // 同 table_row：行头与值同色，避免浅灰在浅背景上显得字号更小。
                .fg(colors.text),
        )
        // ★ 同 table_row：字号放容器上，否则 label 的 `.font_size` 会被父级样式盖掉。
        .child(
            Element::col()
                .weight(1.0)
                .font_size(13.0)
                .child(text.font_family(PANEL_FONT)),
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
        assert!(info.props_active.get());
        assert!(info.props_expanded.get());
        info.persist(&cfg);
        assert!(cfg.borrow().info_panel);
        assert_eq!(cfg.borrow().info_cards, vec!["properties".to_string()]);
        assert!(info.toggle_properties());
        info.persist(&cfg);
        assert_eq!(
            cfg.borrow().info_cards_collapsed,
            vec!["properties".to_string()]
        );
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
        assert!(chrome.info.props_active.get());
        assert!(chrome.info.props_expanded.get());
        assert!(cfg.borrow().info_panel);
        assert_eq!(cfg.borrow().info_cards, vec!["properties".to_string()]);
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
            info_cards: vec!["properties".into()],
            info_cards_collapsed: vec!["properties".into()],
            ..crate::core::config::Config::default()
        };
        let info = InfoPanel::new();
        info.init_from(&cfg);
        assert!(info.open.get());
        assert_eq!(info.width.get(), 420);
        assert!(info.props_active.get());
        assert!(!info.props_expanded.get());
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

    /// 直方图必须随面板变宽而长高：窄面板保底 160，半屏后到 420 封顶。
    #[test]
    fn histogram_height_grows_with_width() {
        assert_eq!(hist_height(100), 160, "窄面板保底");
        assert_eq!(hist_height(200), 160);
        assert!(
            hist_height(400) > hist_height(200),
            "宽度增加，高度要跟着长"
        );
        assert_eq!(hist_height(2000), 480, "超高面板封顶，免得太矮或太高");
    }

    /// 端点尖峰不参与曲线纵轴：黑边 100、照片内部最高 5，纵轴应取 5。
    #[test]
    fn visual_peak_ignores_endpoint_spikes() {
        let mut h = Histogram {
            count: 195,
            ..Default::default()
        };
        h.r[0] = 100;
        h.r[255] = 90;
        h.r[128] = 5;
        assert_eq!(visual_peak(&h), 5);
        // 全图只有端点时退回全局峰值，不能除零/画成空图。
        let mut only_edges = Histogram {
            count: 10,
            ..Default::default()
        };
        only_edges.r[0] = 10;
        assert_eq!(visual_peak(&only_edges), 10);
    }

    /// 平滑与取样两端夹紧；尖峰不会被平滑抹没。
    #[test]
    fn smooth_sampling_stays_in_bounds_and_keeps_peak() {
        let mut bins = [0u32; 256];
        bins[128] = 1000;
        let s = smooth_bins(&bins);
        assert!(s[128] > 0.0, "尖峰仍在");
        assert!(s[128] > s[0]);
        assert_eq!(sample_smooth(&s, 0.0), s[0]);
        assert_eq!(sample_smooth(&s, 255.0), s[255]);
        assert!(sample_smooth(&s, -10.0).is_finite());
        assert!(sample_smooth(&s, 999.0).is_finite());
    }

    /// 卡片 id / 文案是菜单与 ini 的共同真源，顺序也不能漂。
    #[test]
    fn card_ids_and_labels_are_stable() {
        assert_eq!(
            Card::ALL.map(|c| c.id()),
            [CARD_PROPERTIES, CARD_TECHNICAL, CARD_HISTOGRAM]
        );
        assert_eq!(Card::ALL.map(|c| c.label()), ["属性", "技术信息", "直方图"]);
    }

    /// 三张卡的开关 / 折叠必须能完整落进 `info_cards` 再读回来。
    #[test]
    fn card_activation_and_collapse_round_trip_through_config() {
        let info = InfoPanel::new();
        info.open_card(Card::Technical);
        info.open_card(Card::Histogram);
        info.toggle_card(Card::Histogram);

        let cfg = Rc::new(RefCell::new(Config::default()));
        info.persist(&cfg);
        assert_eq!(
            cfg.borrow().info_cards,
            vec![CARD_TECHNICAL.to_string(), CARD_HISTOGRAM.to_string()]
        );
        assert_eq!(
            cfg.borrow().info_cards_collapsed,
            vec![CARD_HISTOGRAM.to_string()]
        );

        let restored = InfoPanel::new();
        restored.init_from(&cfg.borrow());
        assert!(restored.tech_active.get());
        assert!(restored.hist_active.get());
        assert!(restored.tech_expanded.get());
        assert!(!restored.hist_expanded.get());
    }

    /// 技术信息 / 直方图卡也要真的把面板内容撑高；折叠直方图后要变矮。
    #[test]
    fn technical_and_histogram_cards_add_height() {
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

        chrome.info.open_card(Card::Technical);
        tree.layout_root(size, &mut NullTextEngine);
        let h_tech = tree.get(scroll).unwrap().content_h;
        assert!(h_tech > h_empty, "技术信息卡应撑高内容");

        chrome.info.open_card(Card::Histogram);
        tree.layout_root(size, &mut NullTextEngine);
        let h_hist = tree.get(scroll).unwrap().content_h;
        assert!(h_hist > h_tech, "直方图卡应再撑高一段");

        chrome.info.toggle_card(Card::Histogram);
        tree.layout_root(size, &mut NullTextEngine);
        let h_collapsed = tree.get(scroll).unwrap().content_h;
        assert!(h_collapsed < h_hist, "折叠直方图后内容应变矮");
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

        chrome.info.props_active.set(true);
        chrome.info.props_expanded.set(true);
        tree.layout_root(size, &mut NullTextEngine);
        let h_expanded = tree.get(scroll).unwrap().content_h;

        chrome.info.props_expanded.set(false);
        tree.layout_root(size, &mut NullTextEngine);
        let h_collapsed = tree.get(scroll).unwrap().content_h;

        chrome.info.props_active.set(false);
        tree.layout_root(size, &mut NullTextEngine);
        let h_closed = tree.get(scroll).unwrap().content_h;

        assert!(h_expanded > h_collapsed, "展开应比折叠高");
        assert!(h_collapsed > h_empty, "折叠卡仍占标题行");
        assert_eq!(h_closed, h_empty, "关闭卡片应回到空态高度");
    }
}
