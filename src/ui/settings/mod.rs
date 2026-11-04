//! 设置、关于、属性窗口（参考 CurSeen 控制面板风格）。
//!
//! # 设计规范（对齐 CurSeen）
//!
//! - 两列对齐：左侧标签列固定宽度 `COL1_W` (208px)，右侧控件列左对齐
//! - 行高固定 `ROW_H` (40px)，所有控件垂直居中
//! - 字号统一 `FONT_MD` (14px)，标签用次要文字色
//! - 无卡片背景、无分组标题、无注释说明
//! - 滚动条靠右（内容区填满，滚动条紧贴右边框）

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use windui::core::EventCtx;
use windui::prelude::*;
use windui::signal::{signal, Signal};

use crate::core::config::{
    clamp_window_size, opacity_to_slider, slider_to_opacity, Config, CtrlWheel, DblClick,
    InitWindow, SourceId, UiTheme, Wheel,
};
use crate::core::hotkey::{Action, Hotkey};
use crate::core::util::fmt_bytes;
use crate::core::viewer::Viewer;
use crate::decoders::catalog;
use crate::decoders::{install, parse_dll_version, Catalog, DecoderEntry, Health, RemoteEntry};

mod about;
mod decoders;
mod decoders_tab;
mod extensions_tab;
mod exts;
mod metadata;
mod options;
mod rows;
pub mod shortcuts;

// 各页与行版式的名字在这里**再发布一次**：子模块之间不许互相引用（那会织出一张网），
// 它们一律只 `use super::*` 拿父模块的可见名字。`ExtRow` 走 `pub use` 是因为 `Handles`
// 的 `pub` 字段用它当类型——不给公开可达路径，clippy 的 private_interfaces 会判红。
pub(crate) use about::*;
// ★ 这里的 `decoders::` 是**本模块的子模块**（设置页的「解码器」页），不是 `crate::decoders`
//   （解码器宿主层）。两者 2026-10-26 起同名——宿主层从 `decoders` 改叫 `decoders` 之后，
//   裸写 `decoders::` 会先撞上本模块的同名子模块。凡要指**宿主层**一律写全 `crate::decoders::`。
pub(crate) use decoders::*;
pub(crate) use decoders_tab::*;
pub(crate) use extensions_tab::*;
pub use exts::ExtRow;
pub(crate) use exts::*;
pub use metadata::{open_metadata, refresh_open_metadata};
pub(crate) use options::*;
pub(crate) use rows::{
    action_slot, btn, btn_action, btn_danger, check_row, choice_row, divider, info_row, nav_rail,
    option_row, search_box, section_card, slider_row, version_cell, ActionTone, OnSwitch,
};
pub(crate) use shortcuts::*;

// 设计令牌：**全 `ui/` 层共用**的那一份（[`crate::ui::tokens`]，对齐 CurSeen `ui/tokens.rs`）。
//
// ★ 早先它是**本文件的私有** `mod tokens`，于是画在图片上的那几个界面（顶部胶囊
//   `chrome` / 缩略图条 `filmstrip` / 鸟瞰 `canvas`）只能各自另造一套浮层色——
//   同一块深灰 `28,28,30,225` 抄了两遍。2026-10-26 提到 `ui/` 顶层：换肤入口只剩一个
//   文件，本窗与其它界面共用同一套（改一档配色改一处）。
//   子模块仍靠 `use super::*` 取到这些名字，行版式代码一行都不用动。
use super::tokens::{to_color, *};

/// 设置窗的页序。
const TAB_OPTIONS: usize = 0;
const TAB_SHORTCUTS: usize = 1;
/// 「解码器」页 = 原「解码器」+「解码」两页合并（2026-10-25）：三张可折叠卡片
/// （已安装 / 可下载 / 首选解码器）在一页里。
const TAB_DECODERS: usize = 2;
const TAB_EXTENSIONS: usize = 3;
const TAB_ABOUT: usize = 4;

const _: () = assert!(
    TAB_OPTIONS == 0
        && TAB_SHORTCUTS == 1
        && TAB_DECODERS == 2
        && TAB_EXTENSIONS == 3
        && TAB_ABOUT == 4
);

/// 打开中的设置窗信号句柄。
pub type Hub = Rc<RefCell<Option<Handles>>>;

/// 设置窗全部控件信号的句柄。
#[derive(Clone, Copy)]
pub struct Handles {
    pub stay: Signal<bool>,
    pub minimal_mode: Signal<bool>,
    /// 棋盘格透明背景（ini 键 checker；m53 起落盘，勾选即时作用于当前窗口）。
    pub checker: Signal<bool>,
    pub long_fit: Signal<bool>,
    pub dcb: Signal<usize>,
    /// 「幻灯片间隔（秒）」文本框（ini 键 slide_interval）。
    ///
    /// 只存**间隔**，不存"是不是在放"——放映是会话态，关窗即停（见 `Chrome::toggle_slideshow`）。
    /// 用文本框而不是下拉（用户 2026-10-31 点名）：秒数是整数，四档下拉是把整数轴压成档位。
    pub slide_secs: Signal<String>,
    pub wheel: Signal<usize>,
    pub ctrl_wheel: Signal<usize>,
    pub init_win: Signal<usize>,
    /// 「固定尺寸 (宽, 高)」组合输入，如 `800, 600`。
    pub win_size: Signal<String>,
    /// 邻图预读开关（ini 键 prefetch 的 UI 写点）。默认开：关掉它是给低配机/省电用的。
    pub prefetch: Signal<bool>,
    pub to_tray: Signal<bool>,
    /// 空格预览总开关（ini 键 space_preview 的 UI 写点）。默认关：抢占空格是强行为。
    pub space_preview: Signal<bool>,
    /// 左键拖动的分工（ini 键 drag_image 的 UI 写点）。默认开 = 画布里拖图片、
    /// 标题栏拖窗口；判据本体在 `ui::canvas::left_drag`。
    pub drag_image: Signal<bool>,
    /// 多开开关（ini 键 allow_multiple 的 UI 写点）。默认关 = 单窗口（二次启动就地换图）；
    /// 开 = 二次启动在同一进程里新开一扇窗口，进程与托盘仍只有一个。
    pub allow_multiple: Signal<bool>,
    pub opacity: Signal<f32>,
    pub opacity_label: Signal<String>,
    pub tab: Signal<usize>,
    /// 「已安装」卡按钮行**右端**那行纯计数文案，形如 `4 个解码器`（红字强调）。
    ///
    /// ★ **派生**自 [`Handles::decoder_view`] 与 [`Handles::decoders`]，不再另算一次：
    /// 未搜索时 `N 个解码器`，搜着时 `M / N 个解码器`。计数与列表行因此必然同源——
    /// 各算一次就会出现「写着 7 个、列表里 6 行」那种说不清的偏差。
    pub decoder_count: Signal<String>,
    pub decoder_state: Signal<String>,
    /// 已安装**全量**（过滤的数据源；进页 / 重扫时由 [`Handles::refresh_decoder_cards`] 写）。
    pub decoders: Signal<Vec<DecoderEntry>>,
    /// 已安装过滤视图（**派生信号**：挂在搜索词版本上，打字即时重算）。
    pub decoder_view: Signal<Vec<DecoderEntry>>,
    /// 已安装搜索框文本：匹配名称 / id / DLL 文件 / 说明 / 认领后缀（见 `decoder_matches`）。
    pub decoder_filter: Signal<String>,
    /// 解码器页「本开窗周期已扫过」哨兵。
    ///
    /// ★ 解码器列表与后缀行**共用一枚**：它们现在是同一页的两张卡，分两枚哨兵会出现
    /// "卡片 A 刷了、卡片 B 没刷"（某侧显示旧数据）这种说不清的偏差。
    pub decoders_seen: Signal<bool>,
    /// 「可下载」清单**全量**（`docs/decoders.json` 的解析结果；进页读本地、刷新后台
    /// 更新时由 [`Handles::apply_remote`] 写）。
    ///
    /// ★ 与 [`Handles::decoders`] 是两份**不同**的事实，别合并：这份来自网络，说的是
    /// "仓库里有哪些可以装"；那份来自本机目录，说的是"已经装了什么"。合并等于把
    /// "装了但仓库下架了"和"仓库有但本机没装"两种状态抹平成一种。
    pub remote: Signal<Vec<RemoteEntry>>,
    /// 「可下载」状态行原文：`N 个可下载` / `更新中…` / `更新失败` / `下载中…` / `下载失败`。
    ///
    /// ★ 界面绑的是**派生**的 [`Handles::remote_status`]（它要把搜索匹配数拼进去），
    ///   写这一枚请走 [`Handles::set_remote_state`]，否则派生信号不会重算。
    pub remote_state: Signal<String>,
    /// 正在下载的 `(id, 百分比)`；`None` = 没有下载在跑。
    ///
    /// 行按钮直接绑它：正在下载的那一枚从「下载」变成 `42%`，装完/失败后回到静态文案。
    pub install_progress: Signal<Option<(String, u8)>>,
    /// 「可下载」搜索框文本：匹配名称 / id / 版本 / 说明 / 认领后缀（见 `remote_matches`）。
    pub remote_filter: Signal<String>,
    /// 「可下载」过滤视图（**派生信号**：挂在搜索词版本上，打字即时重算）。
    pub remote_view: Signal<Vec<RemoteEntry>>,
    /// 当前选中的清单来源（下拉：0 = Gitee / 1 = GitHub）。
    ///
    /// 选哪个同时决定两件事：去哪拉清单（`SourceId::index_url`）、每枚 DLL 用哪一列
    /// 下载地址（`RemoteEntry::download_url`）。改动走 [`Handles::sync`] 的差分轮询落盘；
    /// **切来源不联网**（同一份清单两列地址都在，只重算显示与按钮）。
    pub download_source: Signal<usize>,
    /// 看图窗口配色（下拉：0 = 深色 / 1 = 浅色）。
    ///
    /// 改动走 [`Handles::sync`] 的差分落盘；**生效不需要重建窗口**——`runtime` 每帧
    /// 把 `config.ui_theme` 推给渲染令牌（[`crate::ui::tokens::set_theme`]），变化即重绘。
    pub ui_theme: Signal<usize>,
    /// 「可下载」状态行**显示**文案（派生：清单摘要 + 搜索时的匹配数）。
    pub remote_status: Signal<String>,
    /// 「插件」页的全部信号（已安装 / 可下载 / 来源 / 状态 / 进度）。
    pub plugin: PluginUi,
    /// 「首选解码器」**全量**后缀行（过滤的数据源；切页/重扫时由 [`Handles::decode_refresh`] 写）。
    pub ext_rows_all: Signal<Vec<ExtRow>>,
    /// 「首选解码器」过滤视图（**派生信号**：挂在搜索词版本上，打字即时重算——实时性来源）。
    pub ext_view: Signal<Vec<ExtRow>>,
    /// 全量后缀种数（搜索框占位文字用，进页/重扫时刷新）。
    pub ext_count: Signal<usize>,
    /// 「首选解码器」搜索框文本（按扩展名子串过滤，大小写不敏感）。
    pub ext_filter: Signal<String>,
    /// 「已安装」卡展开态。
    pub card_installed: Signal<bool>,
    /// 「可下载」卡展开态（默认收起：今天它是空的）。
    pub card_download: Signal<bool>,
    /// 「首选解码器」卡展开态。
    pub card_preferred: Signal<bool>,
    /// 快捷键页「键盘快捷键」卡展开态（29 行键位，默认展开——它是这一页的主角）。
    pub card_keys: Signal<bool>,
    /// 快捷键页「鼠标快捷键」卡展开态（三档下拉，默认展开）。
    pub card_mouse: Signal<bool>,
    /// 「插件快捷键」卡（第三张，2026-10-31 用户点名）。
    pub card_plugins: Signal<bool>,
}

/// 解析「固定尺寸 (宽, 高)」组合输入（`,`／`，`／`x`／`×` 皆可作分隔，宽松容错）。
///
/// 解析失败不改配置（输入中途状态原样保留）；成功则两轴各自夹进合法区间。
fn parse_win_size(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once([',', '，', 'x', 'X', '×'])?;
    let w = a.trim().parse::<u32>().ok()?;
    let h = b.trim().parse::<u32>().ok()?;
    Some(clamp_window_size(w, h))
}

impl Handles {
    pub fn sync(&self, c: &mut Config) -> bool {
        let mut dirty = false;
        let set = |dirty: &mut bool, slot: &mut bool, v: bool| {
            if *slot != v {
                *slot = v;
                *dirty = true;
            }
        };
        set(&mut dirty, &mut c.stay_on_top, self.stay.get());
        set(&mut dirty, &mut c.minimal_mode, self.minimal_mode.get());
        set(&mut dirty, &mut c.checker, self.checker.get());
        set(&mut dirty, &mut c.long_image_fit, self.long_fit.get());
        let dcb = DblClick::from_index(self.dcb.get() as i64);
        if c.dbl_click != dcb {
            c.dbl_click = dcb;
            dirty = true;
        }
        // ini 键：slide_interval（「幻灯片间隔（秒）」文本框）。**解不动就不改配置**——
        // 与「窗口尺寸」同一处理方式：打到一半（空串 / 半个数）不该被 clamp 成中间值写盘。
        if let Ok(s) = self.slide_secs.get().trim().parse::<u32>() {
            let v = crate::core::config::clamp_slide_interval(s);
            if c.slide_interval != v {
                c.slide_interval = v;
                dirty = true;
            }
        }
        // ini 键：wheel_behavior（滚轮：0 缩放 / 1 翻页，设置页「滚轮行为」下拉）
        let wh = Wheel::from_index(self.wheel.get() as i64);
        if c.wheel != wh {
            c.wheel = wh;
            dirty = true;
        }
        // ini 键：ctrl_wheel（Ctrl+滚轮：0 缩放 / 1 翻页，与裸滚轮各自独立）
        let cw = CtrlWheel::from_index(self.ctrl_wheel.get() as i64);
        if c.ctrl_wheel != cw {
            c.ctrl_wheel = cw;
            dirty = true;
        }
        let iw = InitWindow::from_ui_index(self.init_win.get());
        if c.init_window != iw {
            c.init_window = iw;
            dirty = true;
        }
        if let Some((w, h)) = parse_win_size(&self.win_size.get()) {
            if c.window_w != w {
                c.window_w = w;
                dirty = true;
            }
            if c.window_h != h {
                c.window_h = h;
                dirty = true;
            }
        }
        set(&mut dirty, &mut c.prefetch, self.prefetch.get());
        set(&mut dirty, &mut c.close_to_tray, self.to_tray.get());
        set(&mut dirty, &mut c.space_preview, self.space_preview.get());
        set(&mut dirty, &mut c.drag_image, self.drag_image.get());
        set(&mut dirty, &mut c.allow_multiple, self.allow_multiple.get());
        let op = slider_to_opacity(self.opacity.get());
        if c.opacity != op {
            c.opacity = op;
            dirty = true;
        }
        // ini 键：decoders_preferred（首选解码器每行下拉：0 = 自动，i>0 = 第 i-1 个候选）。
        // windui 下拉没有 onChange 回调，与页面上其它下拉一样走这里的差分轮询：
        // 读信号位次 → 用行数据快照翻译成解码器 id → 与配置比对。变了就写配置并当场
        // 灌进解码线程的全局首选表（不等落盘，与「已安装」卡开关改 set_disabled 同口径）。
        let mut preferred_dirty = false;
        DECODE_ROWS.with(|m| {
            let g = m.borrow();
            for (ext, row) in g.iter() {
                let idx = row.sig.get();
                // 刷新写进 sig 的值不算"用户改了"：首选候选若恰好排到第 0 位，
                // 不能因此把偏好删掉；只有真的从上次位置变过来才处理。
                if idx == row.last.get() {
                    continue;
                }
                if idx >= row.ids.len() {
                    // 越界（旧 ini / 下拉重建瞬间）：不动配置，把信号拉回上次位置。
                    row.sig.set(row.last.get());
                    continue;
                }
                row.last.set(idx);
                // 下拉无「自动」项（2026-10-16 用户口径）：第 0 项 = 默认接单者
                // （文件名序第一个），选它等于清除首选——保持稀疏，行为与自动一致。
                let want = if idx == 0 {
                    None
                } else {
                    row.ids.get(idx).cloned()
                };
                if c.decoders_preferred.get(ext).cloned() != want {
                    match want {
                        Some(id) => {
                            c.decoders_preferred.insert(ext.clone(), id);
                        }
                        None => {
                            c.decoders_preferred.remove(ext);
                        }
                    }
                    dirty = true;
                    preferred_dirty = true;
                }
            }
        });
        if preferred_dirty {
            crate::decoders::set_preferred(c.decoders_preferred.clone());
        }
        // ini 键：ui_theme（选项页「界面主题」，0 = 深色 / 1 = 浅色）。
        // 只管看图窗口的浮层配色，设置页本身不受影响；写进 config 后由 runtime 推给令牌层。
        let th = UiTheme::from_ui_index(self.ui_theme.get());
        if c.ui_theme != th {
            c.ui_theme = th;
            dirty = true;
        }
        // ini 键：download_source（选项页「下载源」，0 = Gitee / 1 = GitHub）。
        // ★ 2026-10-30：**一个来源，两张卡共用**（解码器清单 / 插件清单 / 将来的本体更新）。
        // 界面上已经没有第二处来源控件，`plugin.source` 是这条配置的投影；差分口径不变：
        // 本帧把 `config` 拉平后，下一帧 `c.download_source` 已等于信号值，不会重复重拉。
        let src = SourceId::from_ui_index(self.download_source.get());
        if c.download_source != src {
            c.download_source = src;
            dirty = true;
            self.plugin.source.set(src.ui_index());
            self.plugin.source_changed();
            // 切来源**不联网、不读盘**：同一份清单里 `gitee` / `github` 两列都在，只换列。
            // 后台正刷新时先别抢状态行（等结果落地会按当前来源重算），只重画行。
            if crate::decoders::remote::is_refreshing() {
                let q = self.remote_filter.get();
                self.remote_filter.set(q);
            } else {
                self.remote_recompute();
            }
        }
        // ★ 落盘标记放在**所有**差分之后：它原先是夹在 `preferred_dirty` 那段前面，
        //   之后才补进来的来源差分只把 `dirty` 置真、走不到那次 `mark_dirty()`。
        //   凡在本函数尾部新增差分，这条 `if` 都必须留在最后一行。
        if dirty {
            crate::core::config::mark_dirty();
        }
        dirty
    }

    pub fn refresh_labels(&self, c: &Config) {
        let text = format!("{}%", c.opacity);
        if self.opacity_label.get() != text {
            self.opacity_label.set(text);
        }
    }

    pub fn push(&self, c: &Config) {
        let put_b = |slot: &Signal<bool>, v: bool| {
            if slot.get() != v {
                slot.set(v);
            }
        };
        put_b(&self.stay, c.stay_on_top);
        put_b(&self.minimal_mode, c.minimal_mode);
        put_b(&self.checker, c.checker);
        put_b(&self.long_fit, c.long_image_fit);
        let put_u = |slot: &Signal<usize>, v: usize| {
            if slot.get() != v {
                slot.set(v);
            }
        };
        put_u(&self.dcb, c.dbl_click.index() as usize);
        put_u(&self.wheel, c.wheel.index() as usize);
        put_u(&self.ctrl_wheel, c.ctrl_wheel.index() as usize);
        put_u(&self.init_win, c.init_window.ui_index());
        put_u(&self.ui_theme, c.ui_theme.ui_index());
        put_u(&self.download_source, c.download_source.ui_index());
        // 插件卡也要跟上（它的 source 是 config 的**投影**：界面上已经没有自己的下拉了）。
        if self.plugin.source.get() != c.download_source.ui_index() {
            self.plugin.source.set(c.download_source.ui_index());
            self.plugin.source_changed();
        }
        put_b(&self.prefetch, c.prefetch);
        put_b(&self.to_tray, c.close_to_tray);
        put_b(&self.space_preview, c.space_preview);
        put_b(&self.drag_image, c.drag_image);
        put_b(&self.allow_multiple, c.allow_multiple);
        if self.opacity.get() != opacity_to_slider(c.opacity) {
            self.opacity.set(opacity_to_slider(c.opacity));
        }
    }

    /// 解码器页的刷新门（与旧 `refresh_decoders` 同口径：只在「刚切到本页」时扫一次）。
    ///
    /// ★ 一次扫**三张卡的数据**：解码器列表与后缀行同源（同一次 `catalog::scan()` 的
    /// 读数分两路派生；`list_decoders` 会逐个 LoadLibrary，扫两遍就是白付两遍代价），
    /// 「可下载」读随包清单（零网络）。分开扫会在三张卡上留下不同时刻的快照；
    /// 关窗再开（标签页停在解码器）也靠这里把本地清单重新读一遍。
    pub fn refresh_decoders(&self, c: &Config) {
        if self.tab.get() != TAB_DECODERS {
            if self.decoders_seen.get() {
                self.decoders_seen.set(false);
            }
            return;
        }
        if self.decoders_seen.get() {
            return;
        }
        self.decoders_seen.set(true);
        self.refresh_decoder_cards(c);
        self.remote_load_local();
    }

    /// 切到解码器页的入口（导航回调）：标哨兵 + 立刻扫一轮。
    pub fn enter_decoders(&self, c: &Config) {
        self.decoders_seen.set(true);
        self.refresh_decoder_cards(c);
        // 「可下载」清单**只读随包文件**（零网络）。曾经的"进页顺手拉一次"是每次
        // 切页卡 3 秒的根因：失败不进缓存 ⇒ 没网时每进一次都重打网络（m148 撤）。
        self.remote_load_local();
    }

    /// 写「可下载」状态行，并**顺手 bump 派生信号**。
    ///
    /// ★ 界面绑的是 [`Handles::remote_status`]（派生信号，因为它要把搜索匹配数拼进去，
    /// 见 `decoders::remote_signals`），而派生只在**源版本**变化时重算。直接
    /// `remote_state.set(...)` 的话，状态行会停在旧文案上——「下载中…」写进去、
    /// 屏幕上却还是「2 个可下载」，看着像按钮没反应。所以写状态一律走这里。
    pub fn set_remote_state(&self, text: String) {
        self.remote_state.set(text);
        let q = self.remote_filter.get();
        self.remote_filter.set(q);
    }

    /// 读一次随包清单（`docs/decoders.json`）。进页/开窗时调；**完全不联网**。
    ///
    /// ★ 读失败时**保留上一份内存清单**：网络刷新过但本地只读（写不进 `docs/`）的会话
    ///   里，重新进页不该把眼前那几行抹掉，状态行如实说本地文件不可用即可。
    pub fn remote_load_local(&self) {
        let src = SourceId::from_ui_index(self.download_source.get());
        match crate::decoders::remote::load_local() {
            Ok(cat) => self.apply_remote(cat, src, None),
            Err(e) => {
                // 清单异常已经是一句行动指引（“请「刷新」”），原样显示。
                self.set_remote_state(e);
            }
        }
    }

    /// 起一次后台刷新（只由「刷新」按钮调）：联网 → 解析 → 覆盖本地 `docs/decoders.json`。
    ///
    /// 不进 UI 线程：WinHTTP 四阶段超时叠加起来足够让设置窗"未响应"。结果由设置窗的
    /// 250 ms 定时器经 [`Handles::remote_poll`] 取走。
    pub fn remote_refresh(&self, c: &Config) {
        // 地址取配置里那一条（ini 键 decoder_index_*，用户可改），不是常量。
        match crate::decoders::remote::start_refresh_for(c) {
            Ok(()) => self.set_remote_state("更新中…".to_string()),
            Err(e) => {
                log::warn!("清单更新未能开始：{e}");
                self.set_remote_state(e.ui_text().to_string());
            }
        }
    }

    /// 后台刷新结果落地。返回 true = 信号动过（调用方 `mark_dirty`）。
    ///
    /// ★ **失败保留上一份清单**：网络抖一下就让人眼前那几行消失，比报错更糟。
    /// ★ 成功只在摘要后补一句「已更新」（2026-10-29 用户裁定），不写来源——
    ///   来源就在同一行的下拉里。
    pub fn remote_poll(&self) -> bool {
        let Some(out) = crate::decoders::remote::poll_refresh() else {
            return false;
        };
        let src = SourceId::from_ui_index(self.download_source.get());
        match out.result {
            Ok(cat) => {
                self.apply_remote(cat, src, Some(" · 已更新"));
                log::info!("解码器清单已从 {} 刷新", out.src.label());
            }
            Err(e) => {
                log::warn!("解码器清单刷新失败（{}）：{e}", out.src.label());
                self.set_remote_state(crate::decoders::remote::catalog_error_text().to_string());
            }
        }
        true
    }

    /// 用**当前内存清单**重算状态行并重画行（切来源 / 装完之后）。不读盘、不联网。
    fn remote_recompute(&self) {
        // 没有清单时别把"本地清单缺失 / 读取失败"覆盖成"0 个可下载"：状态行是用户唯一
        // 的排障入口。没有行要重画，只需同值写 bump 让视图重算（代价为零）。
        if self.remote.get().is_empty() {
            let q = self.remote_filter.get();
            self.remote_filter.set(q);
            return;
        }
        let src = SourceId::from_ui_index(self.download_source.get());
        let state = remote_summary(&self.remote.get(), &self.decoders.get(), src);
        // 同值写 bump 搜索词 → 派生视图重算 → 行按新的来源/已安装状态重建。
        self.set_remote_state(state);
    }

    /// 清单落地（本地读 / 后台刷新共用）：entries 与摘要行一次写齐。
    ///
    /// 信息不全被丢掉的项数只进日志（2026-10-29：状态行只要数字）。
    fn apply_remote(&self, cat: Catalog, src: SourceId, note: Option<&str>) {
        if cat.skipped > 0 {
            log::info!("清单有 {} 项信息不全，已跳过", cat.skipped);
        }
        let mut state = remote_summary(&cat.entries, &self.decoders.get(), src);
        if let Some(note) = note {
            state.push_str(note);
        }
        self.remote.set(cat.entries);
        self.set_remote_state(state);
    }

    /// **目录变了之后唯一的入口**（装完 / 卸掉 / 点「重新扫描」/ 打开目录）：一次推到底。
    ///
    /// 四件事少做一件就留一半"幽灵"，而且四条都不是理论——每一条都真出现过：
    /// - `request_rescan`：让**解码线程**重扫目录，新 DLL 当场认领后缀
    ///   （不刷 = "列表里有它、打开图还是说不支持"）；
    /// - `refresh_decoder_cards`：**已安装卡 + 首选解码器卡一起换**。★ 首选卡以前只由
    ///   `refresh_decoder_cards` 在**进页**时刷，装完只刷了已安装卡 ⇒ 2026-10-29 用户报障：
    ///   "下载装好、图也能解了，可首选页的后缀集合是旧的，必须关掉设置窗再开"；
    /// - `invalidate_installed_cache`：插件页的已安装列表下次读盘（插件装完不失效会少一行）。
    ///
    /// 所以**旧的半刷函数 `decoder_refresh` 已删除**：这类"忘了刷另一半"不该靠后人记得，
    /// 从 API 上只留这一个入口，想漏也漏不掉。
    pub fn refresh_after_dir_change(&self, c: &Config) {
        refresh_catalogs();
        self.refresh_decoder_cards(c);
    }

    /// 一次目录扫描喂两张卡（已安装 + 首选解码器）。
    ///
    /// `list_decoders()` 会把目录里的每个 DLL 都 LoadLibrary 一遍；分头调两个 refresh 就是
    /// 把这份代价付两遍（切页卡顿的次要来源）。**两张卡永远一起刷**——它们是同一份扫描的
    /// 两个视图，只刷一张就是 2026-10-29 那个"装完能解、首选页还是旧的"报障。
    pub(crate) fn refresh_decoder_cards(&self, c: &Config) {
        let s = catalog::scan();
        self.decoder_refresh_from(&s);
        self.decode_refresh_from(&s, c);
    }

    fn decoder_refresh_from(&self, s: &catalog::Scan) {
        // ★ 计数不再在这里写：它是 `decoder_view` 的派生信号（未搜 = N 个、搜着 = M / N 个），
        //   这里只灌全量列表，计数随之重算——少一处"两边各算一次"的漂移面。
        let base = catalog::summary(s);
        let off = s.decoders.iter().filter(|p| !p.enabled).count();
        set(
            &self.decoder_state,
            if off > 0 {
                format!("{base}，{off} 个已停用")
            } else {
                base
            },
        );
        if self.decoders.get() != s.decoders {
            self.decoders.set(s.decoders.clone());
            // ★ 行集变了必须借**同值写** bump 搜索词版本：过滤视图是挂在搜索词上的
            //   派生信号（`Signal::set` 无条件自增版本并通知），不 bump 的话重扫/卸载
            //   之后视图仍停在上一次的行集上——表现为"文件删了、列表还挂着那一行"。
            let q = self.decoder_filter.get();
            self.decoder_filter.set(q);
        }
    }

    /// 扫解码器目录 → 重建「首选解码器」行 → 行信号单例对齐（新增建、消失删、现存回灌配置值）。
    ///
    /// `s` 由 [`Handles::refresh_decoder_cards`] 一次扫描提供；本函数自己不再扫目录。
    fn decode_refresh_from(&self, s: &catalog::Scan, c: &Config) {
        let rows = ext_rows_from(
            &s.decoders,
            &crate::core::formats::available_builtins()
                .copied()
                .collect::<Vec<_>>(),
            &c.decoders_preferred,
        );
        DECODE_ROWS.with(|m| {
            let mut g = m.borrow_mut();
            g.retain(|ext, _| rows.iter().any(|r| &r.ext == ext));
            for r in &rows {
                match g.get_mut(&r.ext) {
                    // 已有信号：快照换成新候选，显示值回灌配置（条目失效回落自动）。
                    Some(row) => {
                        row.ids = r.ids.clone();
                        if row.sig.get() != r.current {
                            row.sig.set(r.current);
                        }
                        row.last.set(r.current);
                    }
                    None => {
                        g.insert(
                            r.ext.clone(),
                            DecodeRow {
                                sig: signal(r.current),
                                ids: r.ids.clone(),
                                last: std::cell::Cell::new(r.current),
                            },
                        );
                    }
                }
            }
        });
        // 全量数据落位；派生过滤视图挂在搜索词版本上，行集变了要借**同值写**
        // bump 它（Signal::set 无条件自增版本并通知），派生重算逼 DynList 重排。
        let changed = self.ext_rows_all.get() != rows;
        if changed {
            self.ext_rows_all.set(rows.clone());
            let q = self.ext_filter.get();
            self.ext_filter.set(q);
        }
        if self.ext_count.get() != rows.len() {
            self.ext_count.set(rows.len());
        }
    }
}

/// 目录变了之后的**底层刷新**：解码线程重扫 + 插件列表缓存失效。
///
/// 不依赖设置窗是否开着——`poll_installs` 在窗关掉之后仍然要收尾（宿主的既有承诺），
/// 那条路径上只有这里能调。设置页还开着时，[`Handles::refresh_after_dir_change`] 在这个
/// 之上再补**两张卡的信号**（扫描只做一次：它把同一份 `catalog::scan()` 结果喂给两张卡）。
///
/// ★ 放这里而不是 `decoders`/`extensions`：那两个层**双向禁互引**（AGENTS §一），
/// 只有 ui 这一层同时看得到它们。
pub fn refresh_catalogs() {
    crate::decoders::request_rescan();
    crate::extensions::invalidate_installed_cache();
}

fn set(slot: &Signal<String>, v: String) {
    if slot.get() != v {
        slot.set(v);
    }
}

pub fn push_to_hub(hub: &Hub, c: &Config) {
    // 同 `runtime` 那条轮询的口径：`Handles` 是 `Copy`，拷出来再调，别让 `Ref` 跨过 `push`。
    let handles = hub.borrow().as_ref().copied();
    if let Some(h) = handles {
        h.push(c);
    }
}

fn sig(v: bool) -> Signal<bool> {
    signal(v)
}

/// 构建「设置」窗的请求（不依赖具体 ctx 类型）。
///
/// 抽出来是为了让**两条入口共用同一扇窗**：主窗热键/菜单走
/// [`open_settings`]（`EventCtx`），系统托盘右键「进入设置」走 `TrayCtx`。
/// 两者都只有 `open_window(WindowRequest)` 这一环是 ctx 相关的，其余（Handles
/// 单例的建立/回灌、捕获态复位、`Window` 构造）与此无关，故收进这里返回
/// `WindowRequest` 交给调用方 `open_window`。
pub(crate) fn settings_window(config: &Rc<RefCell<Config>>, hub: &Hub) -> WindowRequest {
    let hub: Hub = hub.clone();
    // 单例窗（.single("settings")）已开时 windui 只聚焦、content 不重建（windui
    // app/mod.rs 单例测试 builds==0）。每次开都另造一套信号会让「窗口握旧的、
    // runtime 同步读新的」——设置改动静默丢失。故只在首次开窗建 Handles；之后
    // 复用同一套信号并用最新配置回灌显示值（关窗期间窗外改过的配置随之跟上）。
    if hub.borrow().is_none() {
        let c = config.borrow();
        let (ext_rows_all, ext_filter, ext_view) = decode_signals();
        let (decoders, decoder_filter, decoder_view, decoder_count) = installed_signals();
        // 「可下载」的三件套：全量（网络来的）→ 搜索词 → 派生视图 + 派生状态行。
        let remote = signal(Vec::new());
        let remote_state: Signal<String> = signal(String::new());
        let (remote_filter, remote_view, remote_status) = remote_signals(remote, remote_state);
        let plugin = PluginUi::new(&c);
        let h = Handles {
            stay: sig(c.stay_on_top),
            minimal_mode: sig(c.minimal_mode),
            checker: sig(c.checker),
            long_fit: sig(c.long_image_fit),
            dcb: signal(c.dbl_click.index() as usize),
            slide_secs: signal(c.slide_interval.to_string()),
            wheel: signal(c.wheel.index() as usize),
            ctrl_wheel: signal(c.ctrl_wheel.index() as usize),
            init_win: signal(c.init_window.ui_index()),
            win_size: signal(format!("{}, {}", c.window_w, c.window_h)),
            prefetch: sig(c.prefetch),
            to_tray: sig(c.close_to_tray),
            space_preview: sig(c.space_preview),
            drag_image: sig(c.drag_image),
            allow_multiple: sig(c.allow_multiple),
            opacity: signal(opacity_to_slider(c.opacity)),
            opacity_label: signal(format!("{}%", c.opacity)),
            tab: signal(TAB_OPTIONS),
            decoder_count,
            decoder_state: signal(String::new()),
            decoders,
            decoder_view,
            decoder_filter,
            decoders_seen: sig(false),
            remote,
            remote_state,
            install_progress: signal(None),
            remote_filter,
            remote_view,
            remote_status,
            plugin,
            ext_rows_all,
            ext_view,
            ext_count: signal(0),
            ext_filter,
            // 三张卡的初始展开态：有内容的两张开、空的那张收起（不摊开一页空白）。
            card_installed: sig(true),
            card_download: sig(true),
            card_preferred: sig(true),
            card_keys: sig(true),
            card_mouse: sig(true),
            card_plugins: sig(true),

            ui_theme: signal(c.ui_theme.ui_index()),
            download_source: signal(c.download_source.ui_index()),
        };
        drop(c);
        *hub.borrow_mut() = Some(h);
    } else {
        let c = config.borrow();
        let handles = hub.borrow().as_ref().copied();
        if let Some(h) = handles {
            h.push(&c);
            h.refresh_labels(&c);
            // 关窗期间解码器目录可能变过：让 tick 的 refresh_decoders 重扫一轮
            // （解码器列表与后缀行同扫，两处口径不会各停在不同时刻）。
            h.decoders_seen.set(false);
        }
    }

    // 快捷键捕获态（窗口级 on_shortcut 读它拦截下一键）+ 每行显示文本：取进程级
    // 单例（见 shortcuts_ui）。每次开窗先复位捕获、再把文本对回当前配置——单例
    // 窗已开时 content 不重建，只有现成这对信号能把状态拉平到旧树上。
    let (capturing, texts) = shortcuts_ui();
    capturing.set(None);
    sync_hotkey_texts(&config.borrow(), None, &texts);
    // ★ 插件快捷键的单例必须在这里建好：下面 `open_window` 的内容闭包运行在 windui 的
    //   `SignalScope` 里，在那儿首次创建的信号会归这扇窗所有、关窗即回收 —— 单例里
    //   留下死句柄，下次一读就 panic（2026-10-31 秒退的根因）。
    shortcuts::plugin_hk_bootstrap(&config.borrow());

    let config = config.clone();
    let cap_sc = capturing;
    let cfg_sc = config.clone();
    let texts_sc = texts.clone();
    // 设置窗只展示下载进度、消费清单刷新；安装结果由主 runtime 唯一消费。
    let hub_tick: Hub = hub.clone();
    Window::new("设置", 640, 480)
        .resizable(false)
        .centered(true)
        .owned(true)
        .single("settings")
        .on_interval(Duration::from_millis(250), move |ctx| {
            // 先把句柄拷出来：`borrow()` 的临时量会活到整个 if-let 体，
            // 而下面 `decoder_install_done` / `remote_poll` 都会动信号（见 runtime 的同款注释）。
            let Some(h) = hub_tick.borrow().as_ref().copied() else {
                return;
            };
            // 进度先于结果落地：下载刚结束时 `progress()` 已经是 None，先把按钮从
            // 「100%」翻回静态文案，再让 `decoder_install_done` 按新装状态重画。
            let p = crate::decoders::install::progress().map(|p| {
                let pct = p.percent();
                (p.id, pct)
            });
            if h.install_progress.get() != p {
                h.install_progress.set(p);
                ctx.mark_dirty();
            }
            if h.remote_poll() {
                ctx.mark_dirty();
            }
            if h.plugin.poll() {
                ctx.mark_dirty();
            }
        })
        // 捕获态：焦点在本窗口时，下一键被抓成一条 Hotkey 写回配置。**取消捕获**=点击别处
        // 使本框失焦（见 hotkey_box 的 on_blur），故这里不再把 Esc 当取消——Esc 是一个正常
        // 可绑定主键。纯修饰键等不构成主键的键被忽略（继续等下一键）。撞键由 set_binding 强制唯一。
        .on_shortcut(move |_ctx, ev| {
            // 插件快捷键：捕获目标记在另一个信号里（按插件 id），与主机快捷键互不干扰。
            // 写的是同一份 `plugins/<id>/settings.json` —— 这就是"两处同步"的实体：
            // 不是两份数据互相同步，而是**只有一个真源**。
            let plugin_cap = shortcuts::plugin_hk_ui();
            let capturing_plugin = plugin_cap.as_ref().and_then(|u| u.capturing.get());
            if let Some(id) = capturing_plugin {
                if !ev.pressed {
                    return true;
                }
                let Some(ki) = super::to_key_input(&ev) else {
                    return true;
                };
                let Some(hk) = Hotkey::from_event(&ki) else {
                    return true;
                };
                let key = crate::extensions::plugin_hotkey_items(&cfg_sc.borrow())
                    .into_iter()
                    .find(|(pid, _, _, _)| pid == &id)
                    .map(|(_, _, k, _)| k);
                if let Some(key) = key {
                    if let Err(e) = crate::extensions::set_plugin_hotkey(&id, &key, &hk.to_spec()) {
                        log::warn!("插件「{id}」快捷键保存失败：{e}");
                    } else {
                        // 同一个键在宿主/别的插件里出现 ⇒ 旧的清空（最后绑定的人赢）。
                        crate::extensions::vacate_hotkey(&mut cfg_sc.borrow_mut(), &hk, Some(&id));
                    }
                }
                if let Some(u) = plugin_cap {
                    u.capturing.set(None);
                }
                // 两边文本都刷：宿主那一栏可能刚刚被清空。
                let cfg = cfg_sc.borrow();
                shortcuts::sync_plugin_hotkey_texts(&cfg, None);
                let (cap_host, texts_host) = shortcuts::shortcuts_ui();
                shortcuts::sync_hotkey_texts(&cfg, cap_host.get(), &texts_host);
                return true;
            }
            let Some(a_idx) = cap_sc.get() else {
                return false;
            };
            if !ev.pressed {
                return true;
            }
            let Some(ki) = super::to_key_input(&ev) else {
                return true;
            };
            let Some(hk) = Hotkey::from_event(&ki) else {
                return true;
            };
            let Some(a) = Action::from_index(a_idx) else {
                cap_sc.set(None);
                sync_hotkey_texts(&cfg_sc.borrow(), cap_sc.get(), &texts_sc);
                return true;
            };
            // 先清同键的旧持有者（宿主动作 + 插件），再绑 —— 顺序反了会把自己清掉，
            // 症状就是"很多键绑不上"（2026-10-31 实测）。见 `bind_host_action`。
            crate::extensions::bind_host_action(&mut cfg_sc.borrow_mut(), a, hk);
            cap_sc.set(None);
            sync_hotkey_texts(&cfg_sc.borrow(), cap_sc.get(), &texts_sc);
            shortcuts::sync_plugin_hotkey_texts(&cfg_sc.borrow(), None);
            true
        })
        .content(move || {
            let Some(h) = hub.borrow().as_ref().copied() else {
                return Element::label("设置已关闭");
            };
            let pages = [
                options_page(&h),
                shortcuts_page(&h, config.clone(), capturing, texts),
                decoders_page(&h, config.clone()),
                extensions_page(&config, &h.plugin),
                about_page(),
            ];
            let mut stack = Element::stack().fill().weight(1.0);
            for (i, page) in pages.into_iter().enumerate() {
                let sel = h.tab;
                stack = stack.child(page.fill().visible_when(move || sel.get() == i));
            }
            let cfg_switch = config.clone();
            let on_switch: OnSwitch = {
                Rc::new(move |ctx, idx| {
                    if idx == TAB_DECODERS {
                        h.enter_decoders(&cfg_switch.borrow());
                        ctx.mark_dirty();
                    }
                })
            };
            Element::row()
                .fill()
                .child(nav_rail(h.tab, on_switch))
                .child(stack)
                .bg(to_color(BG))
                .cross(Align::Stretch)
        })
}

/// 主 runtime 每 250ms 消费一次；设置窗关闭不能中断安装的启用/重扫收尾。
pub fn poll_installs(ctx: &mut EventCtx, config: &Rc<RefCell<Config>>, hub: &Hub) {
    // 回调会写信号/配置，不能把 Hub 的 Ref 带进去。
    let h = hub.borrow().as_ref().copied();
    if let Some(out) = install::poll() {
        decoder_install_done(ctx, config, h, out);
    }
    if let Some(out) = crate::extensions::install::poll() {
        extensions_tab::plugin_install_done(ctx, config, h.map(|h| h.plugin), out);
    }
}

fn decoder_install_done(
    ctx: &mut EventCtx,
    config: &Rc<RefCell<Config>>,
    h: Option<Handles>,
    out: install::Outcome,
) {
    if let Some(h) = h {
        h.install_progress.set(None);
    }
    match out.result {
        Ok(path) => {
            let disabled = {
                let mut c = config.borrow_mut();
                c.decoders_disabled.retain(|x| x != &out.id);
                c.decoders_disabled.clone()
            };
            crate::decoders::set_disabled(disabled);
            crate::core::config::mark_dirty();
            // ★ 底层刷新无条件跑：这个收尾函数存在的理由就是"设置窗关着也要收尾"。
            refresh_catalogs();
            if let Some(h) = h {
                // 两卡一起刷（2026-10-29 报障：只刷已安装卡 ⇒ 首选页的后缀集要关窗重开才更新）。
                h.refresh_decoder_cards(&config.borrow());
                h.remote_recompute();
            }
            log::info!("已安装解码器「{}」（{}）并启用", out.name, path.display());
            ctx.toast(format!("已安装「{}」并启用", out.name));
        }
        Err(msg) => {
            log::warn!("解码器下载失败（{}）：{msg}", out.id);
            if let Some(h) = h {
                h.set_remote_state("下载失败".to_string());
            }
            ctx.toast_err("下载失败".to_string());
        }
    }
    ctx.mark_dirty();
}

/// 从主窗（`EventCtx`）打开设置单例窗——真正建窗逻辑在 [`settings_window`]，
/// 这里只把请求交给当前 ctx。托盘那条入口见 [`crate::ui::tray`]。
pub fn open_settings(ctx: &mut EventCtx, config: &Rc<RefCell<Config>>, hub: &Hub) {
    let req = settings_window(config, hub);
    ctx.open_window(req);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_audit_plugin_install_without_hub_enables_and_reports() {
        let cfg = Rc::new(RefCell::new(Config::default()));
        cfg.borrow_mut().extensions_disabled = vec!["test-plugin".into()];
        let result = windui::testing::run_with_ctx(|ctx| {
            extensions_tab::plugin_install_done(
                ctx,
                &cfg,
                None,
                crate::extensions::install::Outcome {
                    id: "test-plugin".into(),
                    name: "test".into(),
                    result: Ok(()),
                },
            )
        });
        assert!(cfg.borrow().extensions_disabled.is_empty());
        assert!(result.toast.is_some());
        let result = windui::testing::run_with_ctx(|ctx| {
            extensions_tab::plugin_install_done(
                ctx,
                &cfg,
                None,
                crate::extensions::install::Outcome {
                    id: "test-plugin".into(),
                    name: "test".into(),
                    result: Err("private detail".into()),
                },
            )
        });
        assert_eq!(result.toast.unwrap().text, "下载失败");
    }

    #[test]
    fn host_audit_decoder_install_without_hub_enables_and_reports() {
        let cfg = Rc::new(RefCell::new(Config::default()));
        cfg.borrow_mut().decoders_disabled = vec!["test-decoder".into()];
        let result = windui::testing::run_with_ctx(|ctx| {
            decoder_install_done(
                ctx,
                &cfg,
                None,
                install::Outcome {
                    id: "test-decoder".into(),
                    name: "test".into(),
                    result: Ok("test_1.dll".into()),
                },
            )
        });
        assert!(cfg.borrow().decoders_disabled.is_empty());
        assert!(result.toast.is_some());
        let result = windui::testing::run_with_ctx(|ctx| {
            decoder_install_done(
                ctx,
                &cfg,
                None,
                install::Outcome {
                    id: "test-decoder".into(),
                    name: "test".into(),
                    result: Err("private detail".into()),
                },
            )
        });
        assert_eq!(result.toast.unwrap().text, "下载失败");
    }

    /// 五个标签页的首元素顶距必须统一：每个页的首内容元素（scroll 内层 col 的首子，
    /// 或 col 的首子）顶缘 = 页面顶缘 + `PAGE_TOP_MARGIN`。任何页给首元素额外加顶距
    /// （旧版解码器头部 `.padding_xy(0,8)` 把按钮下压 8px、各页顶距不齐的根因）都会判红。
    ///
    /// ★ 同轮加验「垂直滚动条靠右」全 UI 不变量（2026-10-17 用户口径）：windui 把滚动条
    /// 钉在**滚动节点自身**的 `abs.right()` 上（padding 只收内容、不收节点边），所以滚动
    /// 条贴右窗缘 ⇔ 滚动节点贴右窗缘。options/shortcuts/about 的滚动节点就是页根
    /// （`Element::scroll().fill()`）；decoders/decode 用 `list_signal` 自带滚动，滚动
    /// 节点 = 页 col 的最后一个子。任何页的滚动节点右缘比窗口右缘（640）早收线都判红——
    /// 这正是解码器/解码两页曾把左右 pad 挂在外层 col 上时滚动条悬空 16px 的病灶形状。
    #[test]
    fn settings_tab_first_rows_top_align() {
        use windui::core::Tree;
        use windui::text::NullTextEngine;

        let config = Rc::new(RefCell::new(Config::default()));
        let (ext_rows_all, ext_filter, ext_view) = decode_signals();
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
            tab: signal(0),
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

            ui_theme: signal(0),
            download_source: signal(0),
            plugin: PluginUi::new(&Config::default()),
        };
        let (capturing, texts) = shortcuts_ui();
        let pages: Vec<Element> = vec![
            options_page(&h),
            shortcuts_page(&h, config.clone(), capturing, texts),
            decoders_page(&h, config.clone()),
            extensions_page(&config, &h.plugin),
            about_page(),
        ];
        let mut stack = Element::stack().fill().weight(1.0);
        for p in pages {
            stack = stack.child(p.fill());
        }
        let ui = Element::row()
            .fill()
            .child(Element::leaf().width(88).height_match())
            .child(stack);

        let mut tree = Tree::new();
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let size = Size::new(640, 480);
        tree.layout_root(size, &mut NullTextEngine);

        let stack_id = tree.get(root).expect("row 根").children[1];
        let names = ["options", "shortcuts", "decoders", "about"];
        for (i, name) in names.iter().enumerate() {
            let page_id = tree.get(stack_id).expect("stack").children[i];
            let page_abs = tree.abs_bounds(page_id);
            // 首内容元素：scroll 页面 children[0] 是内层 col（其首子才是内容）；
            // col 页面 children[0] 即内容。统一取「带 padding 容器的直接首子」。
            let kids = &tree.get(page_id).expect("page").children;
            let first = if kids.len() == 1 && tree.get(kids[0]).expect("inner").children.len() > 1 {
                tree.get(kids[0]).expect("inner").children[0]
            } else {
                kids[0]
            };
            let top = tree.abs_bounds(first).y;
            assert_eq!(
                top,
                page_abs.y + PAGE_TOP_MARGIN,
                "{name} 首行顶距应为 PAGE_TOP_MARGIN({})，实为 {}（各页顶距必须统一）",
                PAGE_TOP_MARGIN,
                top - page_abs.y
            );
            // 滚动条靠右不变量：本页「承载滚动的那个节点」的绝对右缘必须等于窗口右缘。
            // 四页现在都是 `Element::scroll()` 当页根（解码器页里的三张卡按内容堆叠，
            // 不再自带滚动容器），所以滚动节点一律就是页根。
            let scroll_id = page_id;
            assert_eq!(
                tree.abs_bounds(scroll_id).right(),
                size.w,
                "{name} 的滚动节点右缘应贴窗口右缘（滚动条就画在这里）；\
                 差值 {} = 该页左右留白又被挂回外层容器了",
                size.w - tree.abs_bounds(scroll_id).right()
            );
        }
    }
}
