//! 轻量配置持久化：手写 INI，零依赖。
//!
//! 设置项与菠萝看图（pineapple-pictures `settings.h`）逐项对齐。鸟瞰图 / 暗色
//! 棋盘格 / 翻图重置变换 / 关窗直关是**恒定行为**，不设开关。未复刻的三项
//! 及理由见 `ui::settings` 模块头：SVG 限制（不支持 SVG）、HiDPI 策略（windui 固定）等。
//!
//! | 键                | 含义                                        | 默认 |
//! |-------------------|---------------------------------------------|------|
//! | `stay_on_top`     | 启动时窗口置顶                              | `0`  |
//! | `double_click`    | 双击：0 无 / 1 关窗 / 2 最大化 / 3 全屏     | `1`  |
//! | `slide_interval`  | 幻灯片每张停留秒数（1..=600）        | `3`  |
//! | `show_title_bar`  | 标题条：1 完整（文件名+三钮）/ 0 无按钮     | `1`  |
//! | `long_image_fit`  | 长图自动按宽适应（竖向可拖）                | `0`  |
//! | `wheel_behavior`  | 滚轮：0 缩放 / 1 翻页                       | `0`  |
//! | `ctrl_wheel`      | Ctrl+滚轮：0 缩放 / 1 翻页                   | `1`  |
//! | `init_window`     | 启动窗口：0 固定 / 1 全屏 / 3 图片尺寸       | `0`  |
//! | `window_w`        | 固定尺寸档窗口宽（100..=8192）               | `800`|
//! | `window_h`        | 固定尺寸档窗口高（100..=8192）               | `600`|
//! | `opacity`         | 窗口整体不透明度（百分制，10% 档）          | `90` |
//! | `prefetch`        | 邻图预读：后台先解好前后各两张（即时生效）  | `1`  |
//! | `close_to_tray`   | 关闭改为收进系统托盘（即时生效）            | `0`  |
//! | `space_preview`   | 空格预览：资源管理器选中图片按空格弹预览（QuickLook 式） | `0`  |
//! | `decoders_disabled`| 被停用的解码器 id（分号分隔）             | 空   |
//! | `decoders_preferred`| 各扩展名的首选解码器 `扩展名=解码器id`（分号分隔）| 空  |
//! | `raw_last`        | 裸数据参数窗的上次使用值 `宽x高:布局`       | 空   |
//! | `download_source` | 联网来源（解码器清单 / 插件清单 / 将来的本体更新共用） | `gitee` |
//! | `decoder_index_gitee` / `decoder_index_github` | 解码器清单地址（可改：镜像/自建） | 内置默认 |
//! | `plugin_index_gitee` / `plugin_index_github`   | 插件清单地址（可改：镜像/自建）   | 内置默认 |
//!
//! 文件位置：**`<exe 目录>/rupix.ini`**（便携，路径唯一来源 [`crate::core::paths`]）。
//! 读写任何一步失败都**静默回退**：看图器丢一份偏好设置不值得报错打断使用。
//! 外表 JSON 起来更好看，但要为此引入一个序列化依赖——手写这份键值对是我的取舍。
//!
//! 落盘机制：UI 改动只 [`mark_dirty`]（置脏 + 5s trailing 计时），主窗 16ms
//! 轮询 [`poll_save`] 统一落盘，退出时 [`flush_save`] 权威保存；写盘走
//! `.tmp` → fsync → rename 的**原子写**——半途断电既不会留下半个 ini，也不会留下
//! "名字是新的、内容是空的"那个更阴的版本（见 [`Config::save_to`] 里那段解释）。
//! 首次启动生成带注释的默认 ini（[`DEFAULT_INI`]，「文件即文档」）。

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 解码器清单的来源：Gitee / GitHub。
///
/// 选哪个来源同时决定两件事（用户 2026-10-26 方案）：**去哪拉清单**，
/// 以及**每枚 DLL 用哪个下载地址**——清单里 `gitee` / `github` 两栏都给了，
/// 切换来源即在同一份清单里换一列地址，不必另存一份"Gitee 版清单"。
///
/// ★ `SourceId` 落在 `core` 而不是 `decoders`：分层上 `core`(0) 不许反向依赖
///   `decoders`(2)，但 `decoders::remote` 反过来引用 `core` 是合法的上层依赖。
///   下载地址常量是纯字符串，放在 `core` 不牵扯任何解码逻辑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceId {
    /// Gitee（`raw.giteeusercontent.com` 只读 CDN；直接返回清单，不 redirect）。
    Gitee,
    /// GitHub（`raw.githubusercontent.com`）。
    ///
    /// ★ owner 是 `zongmanqiu`，与 Gitee 的 `qiuzongman` **不是同一个用户名**：
    ///   两份清单地址必须各自写死，**不能**用"换主机名"推导（推导出来的是 404）。
    Github,
}

impl SourceId {
    /// 下拉框 / 遍历用的有序清单。顺序即下拉里的显示顺序。
    pub const ALL: [SourceId; 2] = [SourceId::Gitee, SourceId::Github];

    /// 清单与本机存储用的稳定标识（`"gitee"` / `"github"`）。
    pub fn id(self) -> &'static str {
        match self {
            SourceId::Gitee => "gitee",
            SourceId::Github => "github",
        }
    }

    /// 下拉框文案。
    pub fn label(self) -> &'static str {
        match self {
            SourceId::Gitee => "Gitee",
            SourceId::Github => "GitHub",
        }
    }

    /// 该来源的清单地址（`decoders.json` 的只读 raw 链接）。
    ///
    /// 两份 URL 结构不同（`giteeusercontent` vs `raw.githubusercontent`），**owner 也不同**
    /// （`qiuzongman` vs `zongmanqiu`），不能靠字符串替换得到，所以这里各自写死——
    /// 改一处只影响一个来源。
    ///
    /// ★ 托管平台上的仓库名是 **`RuPix-Decoders`（复数）**，与本地源码目录 `RuPix-Decoder/`
    /// 不是同一个名字（2026-10-28 实测：单数 URL 两个镜像都 404，复数 200）。
    /// 同一条规矩也适用于插件：`RuPix-Plugins` vs 本地 `RuPix-Plugin/`。
    ///
    /// ★ 这里是**内置默认值**；实际生效的地址存在 `Config::decoder_index_gitee` /
    ///   `decoder_index_github`（2026-10-28 用户口径：地址落进 ini、允许用户改）。
    ///   换镜像只改那两条 ini 键，不必动本文件。
    pub fn index_url(self) -> &'static str {
        match self {
            SourceId::Gitee => {
                "https://raw.giteeusercontent.com/qiuzongman/RuPix-Decoders/raw/master/decoders.json"
            }
            SourceId::Github => {
                "https://raw.githubusercontent.com/zongmanqiu/RuPix-Decoders/refs/heads/master/decoders.json"
            }
        }
    }

    /// 该来源的 release 资产前缀（清单里的下载地址必须落在同一个 repo 下）。
    ///
    /// ★ GitHub 的 owner 是 `zongmanqiu`，不是 Gitee 的 `qiuzongman`：旧清单里把
    ///   owner 带错的地址会被 [`SourceId::accepts_download_url`] 拒收。
    pub fn download_prefix(self) -> &'static str {
        match self {
            SourceId::Gitee => "https://gitee.com/qiuzongman/RuPix-Decoders/releases/download/",
            SourceId::Github => "https://github.com/zongmanqiu/RuPix-Decoders/releases/download/",
        }
    }

    /// 插件清单地址（与解码器同一套 Gitee / GitHub 镜像规则）。
    ///
    /// ★ Gitee 远程仓库名是 **`RuPix-Plugins`（复数）**，本地源码目录仍叫 `RuPix-Plugin/`——
    /// 两者不是同一个名字：目录名是仓库内约定（AGENTS.md 铁律），远程名是托管平台上
    /// 用户建的仓库。**改地址只改这里**，别顺手把目录名也改了。
    ///
    /// ★ 分支是 **`main`**，与解码器仓（`master`）不同：插件仓是新仓，Gitee 建仓当天的
    /// 默认分支就是 `main`，且 raw 只服务默认分支（2026-10-28 实测：同内容推到 `master`
    /// 后 `raw/master/plugins.json` 仍 404，`raw/main/` 200）。改名前先拿 raw 探一下。
    /// GitHub 侧的插件仓库尚未建立（2026-10-28 探两个名字都 404），先按同一命名写好。
    ///
    /// ★ 同样是**内置默认值**；生效地址在 `Config::plugin_index_gitee` /
    ///   `plugin_index_github`（ini 可改，空值回落这里）。
    pub fn plugin_index_url(self) -> &'static str {
        match self {
            SourceId::Gitee => {
                "https://raw.giteeusercontent.com/qiuzongman/RuPix-Plugins/raw/main/plugins.json"
            }
            SourceId::Github => {
                "https://raw.githubusercontent.com/zongmanqiu/RuPix-Plugins/refs/heads/main/plugins.json"
            }
        }
    }

    /// 这个下载地址是不是本来源的 release 资产。
    ///
    /// 只拦**两个已知镜像域名**（`gitee.com` / `github.com`）：别的 owner 或把两个域名
    /// 填反，都视为填错（灰色「下载」），不让用户点了吃 404；其它域名（单测的
    /// `example.com`、未来自建镜像）不拦，交给 `platform::http` 的协议与主机检查。
    pub fn accepts_download_url(self, url: &str) -> bool {
        let known_mirror =
            url.starts_with("https://gitee.com/") || url.starts_with("https://github.com/");
        if known_mirror {
            url.starts_with(self.download_prefix())
        } else {
            true
        }
    }

    /// 下拉位置（0/1，连续）——与 ini 存储值分开（这里只是 UI 序号）。
    pub fn ui_index(self) -> usize {
        match self {
            SourceId::Gitee => 0,
            SourceId::Github => 1,
        }
    }

    /// 下拉位置 → 枚举；越界回落 `Gitee`（与下拉自身夹取行为一致）。
    pub fn from_ui_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(SourceId::Gitee)
    }

    /// 稳定标识 → 枚举；未知值回落 `Gitee`（默认源）。
    pub fn from_id(s: &str) -> Self {
        match s {
            "github" => SourceId::Github,
            _ => SourceId::Gitee,
        }
    }
}

/// 双击图片的行为（与菠萝看图的 DoubleClickBehavior 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DblClick {
    /// 无操作。
    Nothing,
    /// 关闭窗口（菠萝与 RuPix 的默认——双击看图器关窗是它的招牌交互）。
    CloseWindow,
    /// 切换最大化。
    Maximize,
    /// 切换全屏（菠萝 master 新增档位）。
    Fullscreen,
}

impl DblClick {
    pub fn from_index(i: i64) -> Self {
        match i {
            0 => DblClick::Nothing,
            2 => DblClick::Maximize,
            3 => DblClick::Fullscreen,
            _ => DblClick::CloseWindow,
        }
    }

    pub fn index(self) -> i64 {
        match self {
            DblClick::Nothing => 0,
            DblClick::CloseWindow => 1,
            DblClick::Maximize => 2,
            DblClick::Fullscreen => 3,
        }
    }

    /// 下拉框文案（设置对话框用，与菠萝中文语义一致）。
    pub fn label(self) -> &'static str {
        match self {
            DblClick::Nothing => "无操作",
            DblClick::CloseWindow => "关闭窗口",
            DblClick::Maximize => "切换最大化",
            DblClick::Fullscreen => "切换全屏",
        }
    }

    pub const ALL: [DblClick; 4] = [
        DblClick::Nothing,
        DblClick::CloseWindow,
        DblClick::Maximize,
        DblClick::Fullscreen,
    ];
}

/// 幻灯片放映每张停留的秒数边界（ini 键 `slide_interval`，m167）。
///
/// ★ 夹紧只在这一处定义：解析臂、设置页文本框、runtime 三处都调 [`clamp_slide_interval`]，
///   各自写一遍上下界就是三份"同一个数字"，改的时候必漏一处。
pub const SLIDE_SEC_MIN: u32 = 1;
pub const SLIDE_SEC_MAX: u32 = 600;

/// 把用户给的秒数夹进合法区间（下界 1 秒：再快就看不清内容；上界 600 秒：一小时是
/// 放映，不是看图器该负责的事）。
pub fn clamp_slide_interval(v: u32) -> u32 {
    v.clamp(SLIDE_SEC_MIN, SLIDE_SEC_MAX)
}

/// 滚轮行为（菠萝 MouseWheelBehavior：Zoom / Switch）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    /// 缩放（RuPix 默认——看图器滚轮=缩放是主流习惯）。
    Zoom,
    /// 翻页（菠萝备选项）。
    Switch,
}

impl Wheel {
    pub fn from_index(i: i64) -> Self {
        if i == 1 {
            Wheel::Switch
        } else {
            Wheel::Zoom
        }
    }

    pub fn index(self) -> i64 {
        match self {
            Wheel::Zoom => 0,
            Wheel::Switch => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Wheel::Zoom => "缩放图片",
            Wheel::Switch => "切换图片",
        }
    }

    pub const ALL: [Wheel; 2] = [Wheel::Zoom, Wheel::Switch];
}

/// Ctrl+滚轮行为（与 [`Wheel`] 同构，只是按住 Ctrl 时的另一路分派）。
///
/// 2026-10-14 快捷键优化：普通滚轮沿用旧默认（缩放），Ctrl+滚轮补一个简单档，
/// 让用户在不引鼠标捕获子系统的前提下也能用 Ctrl+滚轮翻页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlWheel {
    /// 缩放（与无修饰滚轮一致）。
    Zoom,
    /// 翻页（默认——普通滚轮缩放时，Ctrl+滚轮翻页互补）。
    Switch,
}

impl CtrlWheel {
    pub fn from_index(i: i64) -> Self {
        if i == 0 {
            CtrlWheel::Zoom
        } else {
            CtrlWheel::Switch
        }
    }

    pub fn index(self) -> i64 {
        match self {
            CtrlWheel::Zoom => 0,
            CtrlWheel::Switch => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CtrlWheel::Zoom => "缩放图片",
            CtrlWheel::Switch => "切换图片",
        }
    }

    pub const ALL: [CtrlWheel; 2] = [CtrlWheel::Zoom, CtrlWheel::Switch];
}

/// 启动窗口模式（用户拍板 2026-10-10 三档，取代旧「恒 800×600 永远居中」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitWindow {
    /// 固定尺寸：按 `window_w × window_h` 建窗（默认档，旧版唯一行为）。
    Fixed,
    /// 启动即全屏（图片自动适应窗口——适应档天然跟随视口，无需额外处理）。
    Fullscreen,
    /// 图片尺寸：启动**首图**解码完成后窗口客户区调成图片原始像素（翻页不跟随；
    /// 无图启动回落固定尺寸）。尺寸先夹进 `[WINDOW_MIN, WINDOW_MAX]`，再交给 windui
    /// 的 `set_window_size`——它会把窗口**夹到显示器工作区**（显示器减去任务栏）并居中，
    /// 因此实际生效范围就是 100×100 ~ 当前显示器可用尺寸。
    Natural,
}

impl InitWindow {
    /// 兼容旧 ini：0(旧自动/常规)→固定；1(旧最大化)→全屏；**2 是旧版「常规窗口」
    /// 的退役值，故意跳过落到固定**——新存储值用 3 表示图片尺寸，防旧文件误读。
    pub fn from_index(i: i64) -> Self {
        match i {
            1 => InitWindow::Fullscreen,
            3 => InitWindow::Natural,
            _ => InitWindow::Fixed,
        }
    }

    pub fn index(self) -> i64 {
        match self {
            InitWindow::Fixed => 0,
            InitWindow::Fullscreen => 1,
            InitWindow::Natural => 3,
        }
    }

    /// 设置页下拉里的**位置**（0/1/2，必须连续）。与 ini 存储值 `index()` 分开：
    /// 2 是旧版「常规窗口」的退役值，不能拿来当 `Natural` 的存储号，但下拉只有三项。
    pub fn ui_index(self) -> usize {
        match self {
            InitWindow::Fixed => 0,
            InitWindow::Fullscreen => 1,
            InitWindow::Natural => 2,
        }
    }

    /// 设置页下拉位置 → 枚举；越界回落 `Fixed`（与下拉自身夹取行为一致）。
    pub fn from_ui_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(InitWindow::Fixed)
    }

    pub fn label(self) -> &'static str {
        match self {
            InitWindow::Fixed => "固定尺寸",
            InitWindow::Fullscreen => "全屏（图片适应窗口）",
            InitWindow::Natural => "图片尺寸",
        }
    }

    pub const ALL: [InitWindow; 3] = [
        InitWindow::Fixed,
        InitWindow::Fullscreen,
        InitWindow::Natural,
    ];
}

/// 固定尺寸档的窗口宽/高下限（用户拍板 2026-10-10：最低 100×100）。
pub const WINDOW_MIN: u32 = 100;
/// 固定尺寸档的窗口宽/高上限（防手滑把 ini 写成天文数字）。
pub const WINDOW_MAX: u32 = 8192;

/// 把任意窗口宽高夹进 `[WINDOW_MIN, WINDOW_MAX]`（load 与设置页输入共用）。
pub fn clamp_window_size(w: u32, h: u32) -> (u32, u32) {
    (
        w.clamp(WINDOW_MIN, WINDOW_MAX),
        h.clamp(WINDOW_MIN, WINDOW_MAX),
    )
}

/// 窗口整体不透明度的可选区间（百分制）。
///
/// 下限取 10 而不是 0：低于一成的窗口基本看不见边框，"窗口还在"这件事就无从判断，
/// 和"程序崩了"没区别。上限当然是 100（完全不透明，走普通合成路径）。
pub const OPACITY_MIN: u8 = 10;
/// 见 [`OPACITY_MIN`]。
pub const OPACITY_MAX: u8 = 100;
/// 档位间隔：所有入口（右键菜单、设置滑条）都按 10% 一档对齐。
pub const OPACITY_STEP: u8 = 10;
/// 出厂默认 **90%**（用户拍板 2026-10-17：原来默认 80%，用户实际基本用满档，提一档到 90）。
pub const OPACITY_DEFAULT: u8 = 90;

/// 把任意百分制值**量化到 10% 档**并夹进区间。
///
/// 所有入口共用：右键菜单本身就只发整档，滑条是连续的靠这里对齐
/// （拖到 73% 落盘成 70%，和菜单里的档位永远一致）。
pub fn quantize_opacity(percent: u8) -> u8 {
    let p = percent.clamp(OPACITY_MIN, OPACITY_MAX);
    let q = ((p + OPACITY_STEP / 2) / OPACITY_STEP) * OPACITY_STEP;
    q.clamp(OPACITY_MIN, OPACITY_MAX)
}

/// 百分制不透明度 → 滑条的内部量纲。
///
/// ★ windui 的 [`Slider`](windui::ui::Slider) 内部恒存 `0.0..=1.0`（拖到哪存到哪，
/// 与业务量纲无关），配置里存的必须是真实百分比。**两端必须各做一次换算互相抵消**：
/// 少了 `v2s`，一开设置窗就会把 75% 写成 1%（信号里是 0.0 附近的值直接当百分比用了）；
/// 少了 `s2v`，滑条拖到头也只能得到 1%。
pub fn opacity_to_slider(percent: u8) -> f32 {
    let p = percent.clamp(OPACITY_MIN, OPACITY_MAX) as f32;
    (p - OPACITY_MIN as f32) / (OPACITY_MAX - OPACITY_MIN) as f32
}

/// 滑条的 `0.0..=1.0` → 百分制不透明度（含夹取 + 10% 档量化）。见 [`opacity_to_slider`]。
pub fn slider_to_opacity(v: f32) -> u8 {
    let span = (OPACITY_MAX - OPACITY_MIN) as f32;
    let raw = (OPACITY_MIN as f32 + v.clamp(0.0, 1.0) * span)
        .round()
        .clamp(OPACITY_MIN as f32, OPACITY_MAX as f32) as u8;
    quantize_opacity(raw)
}

/// 布尔 ini 值只认 `1` / `0`。其余值（`true`、空串、乱词）warn 并返回 `None`，
/// 调用方据此**不覆写**——单行笔误等于保持默认（`prefetch` 默认开，旧写法
/// `v == "1"` 会把 `prefetch=true` 静默当成关，m51-H）。
fn parse_bool(v: &str, key: &str) -> Option<bool> {
    match v {
        "1" => Some(true),
        "0" => Some(false),
        _ => {
            log::warn!("配置项 {key} 的值「{v}」不是 0/1，按默认处理");
            None
        }
    }
}

/// [`parse_bool`] 的写入版：值不合法时原样保留槽位。
fn set_bool(slot: &mut bool, v: &str, key: &str) {
    if let Some(b) = parse_bool(v, key) {
        *slot = b;
    }
}

/// 清单地址的空值容错：ini 里把地址手工清空**不许**把刷新弄成一条空 URL
/// （那在用户看来只是"更新失败"，查不出是自己清空了），空即回落内置默认。
fn url_or(v: &str, default: &str) -> String {
    let t = v.trim();
    if t.is_empty() {
        default.to_string()
    } else {
        t.to_string()
    }
}

/// 全部可持久化设置。零值即默认，方便 `Default` 派生与整体覆写。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 窗口置顶的**默认值**：每个新开的看图窗口都按它决定要不要置顶。
    ///
    /// 与运行期那次切换（标题栏置顶钮 📌）是两件事——后者只改当前窗口的
    /// Z 序，不落盘，下次开窗口仍回到这里的值。
    pub stay_on_top: bool,
    /// 双击图片的行为。
    pub dbl_click: DblClick,
    /// 幻灯片放映每张停留的**秒数**（m167；放映本身的开关**不落盘**，只有这个档位落盘）。
    pub slide_interval: u32,
    /// 极简模式：开启后隐藏标题栏、底部工具栏、信息条；鼠标悬停/移动时显示，静止 2 秒后隐藏。
    /// 关闭时始终显示上述 UI（传统模式）。
    pub minimal_mode: bool,
    /// 棋盘格透明背景（m53 起落盘；默认关）。
    pub checker: bool,
    /// 底部缩略图条（m53 起落盘；默认关。开了会占掉画面底部一条）。
    pub filmstrip: bool,
    /// 标题栏锁定（m53 起落盘；默认关）。锁住后整窗交互封锁，只留锁钮解锁。
    pub locked: bool,
    /// 长图自动按宽适应、竖向可拖（菠萝 `autoLongImageMode`）。
    pub long_image_fit: bool,
    /// 滚轮行为。
    pub wheel: Wheel,
    /// Ctrl+滚轮行为（与 `wheel` 同族，独立一档）。
    pub ctrl_wheel: CtrlWheel,
    /// 启动时的窗口模式（三档，见 [`InitWindow`]）。
    pub init_window: InitWindow,
    /// 固定尺寸档的窗口宽/高（clamp 在 [`WINDOW_MIN`]..=[`WINDOW_MAX`]，默认 800×600）。
    pub window_w: u32,
    pub window_h: u32,
    /// 窗口整体不透明度（百分制，默认 [`OPACITY_DEFAULT`]，10% 一档）。
    pub opacity: u8,
    /// 被用户关闭的解码器 id 列表（分号分隔存盘）。空 = 全部启用。
    pub decoders_disabled: Vec<String>,
    /// 被用户关闭的扩展 id 列表（分号分隔存盘）。空 = 全部启用。
    pub extensions_disabled: Vec<String>,
    /// 连续失败被隔离的插件 id（三振自动写入；设置页可解除）。分号分隔。
    ///
    /// P0.5 三振：同一插件连续 3 次运行期失败（崩/超时/拒绝连接）→ 自动隔离，
    /// 菜单置灰、启动拒绝；成功一次即清零计数。用户取消不计振。
    pub extensions_isolated: Vec<String>,
    /// 各扩展名的**用户显式首选**解码器 id（`扩展名 → 解码器id` 稀疏表）。
    ///
    /// 只覆盖用户在设置页「解码」页明确指定过的后缀；没提到的后缀走自动仲裁
    /// （`crate::decoders::select_candidate` 的路径字典序）。条目失效（解码器被卸载/停用/
    /// 拉黑/不认领该后缀）时**静默回退自动**，不报错、不清理——这是一张用户
    /// 显式维护、天然跟解码器同生共死的稀疏覆盖表，不是全量人工优先级清单。
    pub decoders_preferred: BTreeMap<String, String>,
    /// 邻图预读：后台把同文件夹前后各两张先解好（**视口那一档**），翻页即换。
    ///
    /// 默认开。关掉它的真实理由只有资源：预读缓存最坏驻留 `PREFETCH_CAP` × 视口档
    /// ≈ 256 MiB（[`crate::core::viewer`] 的字节闸），低配机 / 电池 / 不想让后台
    /// 抢 CPU 的用户有权要一台安静的机器。判据口径见 `viewer::request_prefetch`。
    pub prefetch: bool,
    /// 关闭（含双击关窗）改为隐藏窗口 + 收进系统托盘，真退出走托盘右键菜单。
    ///
    /// **运行期即时生效**：关窗裁决在 `runtime` 的 `on_close_request` 里当场读本项，
    /// 托盘图标则由 16ms 轮询跟着它显隐（不再要求重启——托盘过去只能随 `App` 装配期
    /// 装一次，故早先标注"（重启生效）"，现由 windui 补丁 01 解除）。
    pub close_to_tray: bool,
    /// 空格预览：资源管理器/桌面选中图片按空格弹预览，再按收起（预览窗内 Esc 同）。
    ///
    /// **默认关**（拍板 2026-10-16，二轮 2026-10-17 维持）：常驻低级键盘钩子虽
    /// 从不吞键（打字场景结构上不介入，见 `platform::spacehook`），仍是对全系统的
    /// 输入链路介入，交给用户显式开启。运行期即时生效：16ms 轮询差分装/摘钩子并
    /// 连带收起预览（平台侧见 `platform::spacehook` 模块文档）。
    pub space_preview: bool,
    /// 裸数据参数窗的「上次使用值」（宽/高/布局），新裸数据文件的预填默认。
    /// `None` = 还没用过（首开空窗）。
    pub raw_last: Option<crate::core::decode::RawParams>,
    /// **联网来源**（Gitee / GitHub）—— 解码器清单、插件清单、将来的软件本体更新**共用这一个**。
    ///
    /// 选哪个同时决定"去哪拉清单"与"每枚 DLL / 插件包用哪个下载地址"——清单里
    /// `gitee` / `github` 两栏都给了，切换来源即在同份清单里换一列地址。
    /// 默认 Gitee（国内可达性更好）。
    ///
    /// ★ 2026-10-30 用户口径：**一个用户机器的网络是固定的**，所以不该有两个来源开关
    /// （旧版 `decoder_source` / `plugin_source` 各存一份，两个页面的下拉能互相打架）。
    /// 现在只有「选项」页那一处「下载源」，两张卡跟着它走；**旧键不再读取**（一次性重设）。
    pub download_source: SourceId,
    /// 解码器清单的 **Gitee / GitHub 地址**（`decoders.json` 的 raw 链接）。
    ///
    /// ★ 2026-10-28 用户口径：**联网更新的网址要记在配置文件里、允许用户改**——
    ///   换镜像、自建转发、内网代理都不必重编宿主。默认值 = [`SourceId::index_url`]
    ///   写死的那两条；ini 里清空则回落默认（见 [`url_or`]）。
    ///   读者 = [`crate::decoders::remote`]（刷新按它取地址，不再取常量）。
    /// ★ 2026-10-29 用户口径：**界面不出现**，只给高级用户手改 ini
    ///   （登记在 [`CONFIG_ONLY_KEYS`]）。
    pub decoder_index_gitee: String,
    pub decoder_index_github: String,
    /// 插件清单的两个地址；口径同 [`Config::decoder_index_gitee`]。
    /// 读者 = [`crate::extensions::remote`]。
    pub plugin_index_gitee: String,
    pub plugin_index_github: String,
    /// 快捷键绑定组（见 [`crate::core::hotkey::Hotkeys`]）。出厂默认值见
    /// `Hotkeys::default`；用户可在设置页改，落盘为单一 `hotkeys=` 键。
    pub hotkeys: crate::core::hotkey::Hotkeys,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            stay_on_top: false,
            dbl_click: DblClick::CloseWindow,
            slide_interval: 3,
            minimal_mode: false,
            checker: false,
            filmstrip: false,
            locked: false,
            long_image_fit: false,
            wheel: Wheel::Zoom,
            ctrl_wheel: CtrlWheel::Switch,
            init_window: InitWindow::Fixed,
            window_w: 800,
            window_h: 600,
            opacity: OPACITY_DEFAULT,
            decoders_disabled: Vec::new(),
            extensions_disabled: Vec::new(),
            extensions_isolated: Vec::new(),
            decoders_preferred: BTreeMap::new(),
            prefetch: true,
            close_to_tray: false,
            space_preview: false,
            raw_last: None,
            download_source: SourceId::Gitee,
            decoder_index_gitee: SourceId::Gitee.index_url().to_string(),
            decoder_index_github: SourceId::Github.index_url().to_string(),
            plugin_index_gitee: SourceId::Gitee.plugin_index_url().to_string(),
            plugin_index_github: SourceId::Github.plugin_index_url().to_string(),
            hotkeys: crate::core::hotkey::Hotkeys::default(),
        }
    }
}

impl Config {
    /// 从磁盘加载；文件缺失或解析失败一律回默认值。
    ///
    /// 路径见 [`crate::core::paths::config_path`]。文件不存在（首次启动）时先写一份
    /// [`DEFAULT_INI`]——用户打开 ini 就知道每个键是干嘛的，文件即文档。
    pub fn load() -> Self {
        match crate::core::paths::config_path() {
            Some(p) => {
                if !p.exists() {
                    // 只读介质下写失败照样降级成内存会话，但**要留一条 warn**——`paths`
                    // 模块文档写的就是"静默降级并留一条 warn"，过去这一处连个痕都没有
                    // （审计 二-① / m64）：便携盘只读时，用户与日志都不知道偏好没落盘。
                    if let Err(e) = std::fs::write(&p, default_ini()) {
                        log::warn!(
                            "首次写入默认配置 {} 失败（本次会话只在内存里生效）：{e}",
                            p.display()
                        );
                    }
                }
                let cfg = Self::load_from(&p);
                // 老 ini 补登记四个清单地址键（新建的那份模板里已经有了，这里自动是空集）。
                cfg.backfill_index_urls(&p);
                cfg
            }
            None => Config::default(),
        }
    }

    /// 老 ini 补登记新键（[`Config::load`] 专用，**不改已有内容**）。
    ///
    /// ★ 为什么需要：2026-10-28 之前发布的 `rupix.ini` 没有四个清单地址键，而"网址要
    /// 记在配置文件里"的意义就是**用户打开 ini 看得见它**。只在 `save_to` 里写的话，
    /// 升上来的老用户若从不改任何设置，那四条永远不会出现在盘上（内存里用着默认值，
    /// 文件里查无此事）。所以启动时补一次：缺哪个补哪个，缺的整段带注释追加在末尾，
    /// **既有行与用户自己的注释一行不动**（不做整份重写——那会抹掉注解）。
    ///
    /// 追加而不是重排：解析器按 `key=value` 逐行认，行序无关（见 `load_from`）。
    /// 读不到 / 写不进就放弃（只读介质下只是"看不见"而已，行为仍用内存默认值）。
    fn backfill_index_urls(&self, p: &Path) {
        let Ok(text) = std::fs::read_to_string(p) else {
            return;
        };
        let present = |key: &str| {
            text.lines()
                .any(|l| l.split_once('=').is_some_and(|(k, _)| k.trim() == key))
        };
        let missing: Vec<&str> = [
            "decoder_index_gitee",
            "decoder_index_github",
            "plugin_index_gitee",
            "plugin_index_github",
        ]
        .into_iter()
        .filter(|k| !present(k))
        .collect();
        if missing.is_empty() {
            return;
        }
        let mut block =
            String::from("\n# 联网更新地址（可改成镜像 / 自建转发；留空 = 内置默认）\n");
        for k in missing {
            let v = match k {
                "decoder_index_gitee" => &self.decoder_index_gitee,
                "decoder_index_github" => &self.decoder_index_github,
                "plugin_index_gitee" => &self.plugin_index_gitee,
                _ => &self.plugin_index_github,
            };
            block.push_str(&format!("{k}={v}\n"));
        }
        // 前面补一行换行，避免贴在原有最后一行上（有些编辑器保存时没有收尾换行）。
        if let Err(e) = std::fs::OpenOptions::new()
            .append(true)
            .open(p)
            .and_then(|mut f| {
                use std::io::Write;
                // 文件末尾若不是换行，先补一个，否则新块的头一行会粘到上一行尾部。
                if !text.is_empty() && !text.ends_with('\n') {
                    f.write_all(b"\n")?;
                }
                f.write_all(block.as_bytes())
            })
        {
            log::warn!("补登记清单地址到 {} 失败：{e}", p.display());
        }
    }

    /// 写盘（全键重写）。路径不可得或写入失败时静默放弃（见模块头）。
    pub fn save(&self) {
        if let Some(p) = crate::core::paths::config_path() {
            self.save_to(&p);
        }
    }

    /// 从指定文件加载（[`Self::load`] 的实现本体；`pub` 是为了 tests/config_compat.rs
    /// 直接喂 fixture，不再为测试复制一份解析器）。
    pub fn load_from(p: &Path) -> Self {
        let mut cfg = Config::default();
        let Ok(text) = std::fs::read_to_string(p) else {
            return cfg;
        };
        // Windows 编辑器常给 UTF-8 加 BOM；`trim()` 不把 U+FEFF 当空白，
        // 不剥掉就会让第一行的键变成 `\u{feff}stay_on_top`、静默丢失。
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        // show_title_bar（旧键）与 minimal_mode（现行键）互为正反、写的是同一件事：
        // 两个都在时以**现行键**为准，与 ini 行序无关（旧行为是后行无条件覆盖前行，
        // 同一份 ini 换个排序结果就变）；只出现旧键时才采纳旧键。
        let mut legacy_show_title: Option<bool> = None;
        let mut minimal_explicit = false;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "stay_on_top" => set_bool(&mut cfg.stay_on_top, v, "stay_on_top"),
                "double_click" => cfg.dbl_click = DblClick::from_index(v.parse().unwrap_or(1)),
                "slide_interval" => {
                    cfg.slide_interval = clamp_slide_interval(v.parse().unwrap_or(3));
                }
                "minimal_mode" => {
                    // 只有合法值才算"现行键显式出现"：写坏了要允许旧键兜底。
                    if let Some(on) = parse_bool(v, "minimal_mode") {
                        cfg.minimal_mode = on;
                        minimal_explicit = true;
                    }
                }
                // 兼容旧键：show_title_bar=1 -> minimal_mode=false（写坏＝不出现）
                "show_title_bar" => {
                    if let Some(show) = parse_bool(v, "show_title_bar") {
                        legacy_show_title = Some(!show);
                    }
                }
                // `bird_eye`/`light_checker`/`keep_transform`/`close_anim`（旧版设置项）
                // 已废弃：鸟瞰恒开、旧「浅色棋盘」语义已由 `checker`（暗色棋盘开关）
                // 取代、翻图恒重置变换、关窗恒直关，读到只当未知键忽略。
                "checker" => set_bool(&mut cfg.checker, v, "checker"),
                "filmstrip" => set_bool(&mut cfg.filmstrip, v, "filmstrip"),
                "locked" => set_bool(&mut cfg.locked, v, "locked"),
                "long_image_fit" => set_bool(&mut cfg.long_image_fit, v, "long_image_fit"),
                "wheel_behavior" => cfg.wheel = Wheel::from_index(v.parse().unwrap_or(0)),
                "ctrl_wheel" => cfg.ctrl_wheel = CtrlWheel::from_index(v.parse().unwrap_or(1)),
                "init_window" => cfg.init_window = InitWindow::from_index(v.parse().unwrap_or(0)),
                // 固定尺寸档的窗口宽高：越界/乱码一律夹回区间（用户手改 ini 也钳住）。
                "window_w" => cfg.window_w = v.parse().unwrap_or(800).clamp(WINDOW_MIN, WINDOW_MAX),
                "window_h" => cfg.window_h = v.parse().unwrap_or(600).clamp(WINDOW_MIN, WINDOW_MAX),
                "opacity" => cfg.opacity = quantize_opacity(v.parse().unwrap_or(OPACITY_DEFAULT)),
                "decoders_disabled" => {
                    cfg.decoders_disabled = v
                        .split(';')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
                "extensions_disabled" => {
                    cfg.extensions_disabled = v
                        .split(';')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
                "extensions_isolated" => {
                    cfg.extensions_isolated = v
                        .split(';')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
                // 各扩展名首选解码器：`扩展名=解码器id`，分号分隔（如 `jpg=jpeg;png=core`）。
                // 单条缺 `=` 或任一侧为空 → 整条忽略（手工编辑 ini 的容错，不值得报错）。
                "decoders_preferred" => {
                    cfg.decoders_preferred = v
                        .split(';')
                        .filter_map(|s| s.split_once('='))
                        .map(|(k, x)| (k.trim().to_string(), x.trim().to_string()))
                        .filter(|(k, x)| !k.is_empty() && !x.is_empty())
                        .collect();
                }
                "prefetch" => set_bool(&mut cfg.prefetch, v, "prefetch"),
                // `svg_progressive`（矢量渐进首绘）随机制退场：老配置里的这一键落到
                // 下面的 `_ => {}` 被静默忽略，与 `cache_age` 同一种收场。
                // 旧键 `cache_age`（磁盘解码缓存的自动清理档位）与更早的 `cache_limit`
                // 一起随缓存机制退场：落到下面的 `_ => {}` 被忽略，老配置文件照常加载。
                "close_to_tray" => set_bool(&mut cfg.close_to_tray, v, "close_to_tray"),
                // 空格预览（QuickLook 式）：16ms 轮询差分装/摘低级键盘钩子。
                "space_preview" => set_bool(&mut cfg.space_preview, v, "space_preview"),
                // 快捷键组：整段交给 Hotkeys::from_ini 解析（内部先 default 再覆盖）。
                "hotkeys" => cfg.hotkeys = crate::core::hotkey::Hotkeys::from_ini(v),
                // 裸数据「上次使用值」：`宽x高:布局号`（如 `1920x1080:8`）。解析不出
                // 整行放弃——这只影响下次预填，不值得报错。
                "raw_last" => {
                    let parse = |s: &str| -> Option<(u32, u32, u32)> {
                        let (wh, layout) = s.split_once(':')?;
                        let (w, h) = wh.split_once('x')?;
                        Some((
                            w.trim().parse().ok()?,
                            h.trim().parse().ok()?,
                            layout.trim().parse().ok()?,
                        ))
                    };
                    if let Some((w, h, layout)) = parse(v) {
                        cfg.raw_last = Some(crate::core::decode::RawParams {
                            width: w,
                            height: h,
                            layout,
                        });
                    }
                }
                // 联网来源：gitee / github（未知值回落 gitee，见 `SourceId::from_id`）。
                "download_source" => cfg.download_source = SourceId::from_id(v),
                // 清单地址（2026-10-28 落进 ini，用户可改）：空值回落内置默认。
                "decoder_index_gitee" => {
                    cfg.decoder_index_gitee = url_or(v, SourceId::Gitee.index_url())
                }
                "decoder_index_github" => {
                    cfg.decoder_index_github = url_or(v, SourceId::Github.index_url())
                }
                "plugin_index_gitee" => {
                    cfg.plugin_index_gitee = url_or(v, SourceId::Gitee.plugin_index_url())
                }
                "plugin_index_github" => {
                    cfg.plugin_index_github = url_or(v, SourceId::Github.plugin_index_url())
                }
                _ => {}
            }
        }
        // 现行键缺席时才让旧键兑底（显式 minimal_mode 恒赢，见循环前注释）。
        if !minimal_explicit {
            if let Some(show) = legacy_show_title {
                cfg.minimal_mode = show;
            }
        }
        cfg
    }

    /// 写到指定文件（[`Self::save`] 的实现本体）。
    ///
    /// exe 目录不需要建（`parent()` 天然存在），这里也不 `create_dir_all`：
    /// 便携目录写不进去就是**只读介质**，那属于 `paths` 模块文档里说的"静默降级为
    /// 内存会话"，不该在这里大费周章地补mkdir。
    fn save_to(&self, p: &Path) {
        let opacity = quantize_opacity(self.opacity);
        let decoders_disabled = self.decoders_disabled.join(";");
        let extensions_disabled = self.extensions_disabled.join(";");
        let extensions_isolated = self.extensions_isolated.join(";");
        // BTreeMap 迭代序 = 扩展名字典序：同一份配置存两次，字节级一致（幂等测试盯着）。
        let decoders_preferred = self
            .decoders_preferred
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(";");
        let raw_last = self
            .raw_last
            .map(|r| format!("{}x{}:{}", r.width, r.height, r.layout))
            .unwrap_or_default();
        let body = format!(
            "stay_on_top={}\ndouble_click={}\nslide_interval={}\nminimal_mode={}\n\
             checker={}\nfilmstrip={}\nlocked={}\n\
             long_image_fit={}\nwheel_behavior={}\nctrl_wheel={}\ninit_window={}\nwindow_w={}\n\
             window_h={}\nopacity={}\n\
             decoders_disabled={}\nextensions_disabled={}\nextensions_isolated={}\ndecoders_preferred={}\nprefetch={}\nclose_to_tray={}\nspace_preview={}\nraw_last={}\ndownload_source={}\ndecoder_index_gitee={}\ndecoder_index_github={}\nplugin_index_gitee={}\nplugin_index_github={}\nhotkeys={}\n",
            self.stay_on_top as i32,
            self.dbl_click.index(),
            self.slide_interval,
            self.minimal_mode as i32,
            self.checker as i32,
            self.filmstrip as i32,
            self.locked as i32,
            self.long_image_fit as i32,
            self.wheel.index(),
            self.ctrl_wheel.index(),
            self.init_window.index(),
            self.window_w,
            self.window_h,
            opacity,
            decoders_disabled,
            extensions_disabled,
            extensions_isolated,
            decoders_preferred,
            self.prefetch as i32,
            self.close_to_tray as i32,
            self.space_preview as i32,
            raw_last,
            self.download_source.id(),
            self.decoder_index_gitee,
            self.decoder_index_github,
            self.plugin_index_gitee,
            self.plugin_index_github,
            self.hotkeys.to_ini(),
        );
        // 原子写（M2）：写临时文件 → **flush 到盘** → 改名覆盖。
        //
        // 为什么光 rename 不够：NTFS 的元数据日志只保证「改名」这个操作原子，**文件内容
        // 不在日志里**——不 sync 就断电，可能得到"名字是新的、数据还没落盘"的 0 字节 ini，
        // 原子写的全部意义（正式 ini 永远是完整的一份）就此失效。总纲 §7.2 的
        // temp + fsync + replace 三步，中间那一步是内容，不是目录项。
        //
        // 目录句柄的 fsync 不做：Windows 上 `FlushFileBuffers` 对目录只是把该目录项自身
        // 的元数据推一遍，而改名已经在日志里了——多开一次句柄换不到任何额外保证。
        let tmp = p.with_extension("tmp");
        let staged = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(body.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, p)
        })();
        match staged {
            Ok(()) => {}
            Err(e) => {
                // 半成品不留盘：临时文件写成了但 rename 没过，也一并清掉。
                let _ = std::fs::remove_file(&tmp);
                log::warn!(
                    "偏好写入 {} 失败（本次会话只在内存里生效）：{e}",
                    p.display()
                );
            }
        }
    }
}

// ── 保存防抖（M1）───────────────────────────────────────────
//
// UI 侧改配置（设置窗 / 右键菜单 / 裸数据参数窗）一律只 [`mark_dirty`]，
// 由主窗 16ms 轮询 [`poll_save`] 在防抖到期后统一落盘，退出时 [`flush_save`]
// 权威保存。这样连续拖滑条/连点菜单不会每个信号抖动都写一次盘。
//
// 三个入口都是静态的（`Mutex<Option<Instant>>`），省得把计时器 Rc 穿过
// canvas/settings/raw_dialog 三层——防抖状态本来就全局唯一。

/// trailing 计时：距**最后一次**改动 5 秒才落盘。
const SAVE_DEBOUNCE: Duration = Duration::from_secs(5);

/// 防抖截止时刻；`None` = 不脏。见模块头「保存防抖」。
static SAVE_DEADLINE: Mutex<Option<Instant>> = Mutex::new(None);

/// UI 侧改动配置后调用：只置脏 + 重置 5s 计时，**不落盘**。
pub fn mark_dirty() {
    *SAVE_DEADLINE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now() + SAVE_DEBOUNCE);
}

/// 主窗 16ms 轮询调用：防抖到期才真正写盘。
pub fn poll_save(c: &Config) {
    if let Some(p) = crate::core::paths::config_path() {
        poll_at(c, &p, Instant::now());
    }
}

/// 退出时的权威保存：无论计时到没到期，只要脏过就立即落盘并清脏。
///
/// 返回 `bool` = 本次真的写了盘。K44 那条"从托盘退出时兜底没跑到"的排查全靠这个数，
/// 光看文件 mtime 分不清"没脏"与"没跑"。
pub fn flush_save(c: &Config) -> bool {
    crate::core::paths::config_path().is_some_and(|p| flush_at(c, &p))
}

/// 防抖到期判据（**时钟作参数**，总纲试金石 D′：同一输入永远同一输出）。
///
/// 三行代码，却决定"拖一次滑条写几回盘"和"退出到底落没落盘"。写成读
/// `Instant::now()` 的版本只能 sleep 五秒来验——或者更常见：不验。时间进来之后，
/// "还差一秒"与"到点了"就是两条普通断言。落点 `p` 同样由调用方给：测试写临时文件，
/// 不许把一份像真配置 ini 的东西留在测试二进制旁边迷惑下一次的取证。
/// 返回 `bool` = 本次写了盘。
pub(crate) fn poll_at(c: &Config, p: &Path, now: Instant) -> bool {
    let due = SAVE_DEADLINE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|d| now >= d);
    if due {
        *SAVE_DEADLINE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        c.save_to(p);
    }
    due
}

/// [`poll_at`] 的不计时代：只要脏过就写。
pub(crate) fn flush_at(c: &Config, p: &Path) -> bool {
    if SAVE_DEADLINE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .is_some()
    {
        c.save_to(p);
        true
    } else {
        false
    }
}

/// 首次启动生成的带注释默认 ini（「文件即文档」）。
///
/// 解析端把**无 `=` 的行**当注释忽略，所以说明必须独占一行——不能写行内
/// 注释（`stay_on_top=0 # …` 会把值解析脏）。内容必须与 [`Config::default`]
/// 一致，`default_ini_parses_to_defaults` 测试盯着。
///
/// 用函数而非 `const`：快捷键默认值是动态生成的（见
/// [`crate::core::hotkey::Hotkeys::default`]），手写串易与模型漂移。
fn default_ini() -> String {
    let hotkeys = crate::core::hotkey::Hotkeys::default().to_ini();
    // 清单地址直接写进模板（用户打开 ini 就能看见"联网去哪"）：值取内置默认，
    // 与 `Config::default()` 同源，`default_ini_parses_to_defaults` 盯着不漂移。
    let decoder_gitee = SourceId::Gitee.index_url();
    let decoder_github = SourceId::Github.index_url();
    let plugin_gitee = SourceId::Gitee.plugin_index_url();
    let plugin_github = SourceId::Github.plugin_index_url();
    format!(
        "\
# RuPix 偏好设置（与 rupix.exe 同目录，便携——删掉本文件即恢复出厂默认）
# 布尔值用 1/0；看不懂或写错的值一律回退默认，不会报错。

# 启动时窗口置顶
stay_on_top=0

# 双击图片：0 无操作 / 1 关闭窗口 / 2 切换最大化 / 3 切换全屏
double_click=1

# 幻灯片放映每张停留的秒数（1..=600，越界夹紧；放映的开与关本身不落盘，关窗即停）
slide_interval=3

# 极简模式：1 隐藏标题栏/工具栏/信息条，悬停显示 / 0 传统模式始终显示
minimal_mode=0

# 棋盘格透明背景（1 开 / 0 关）
checker=0

# 底部缩略图条（1 开 / 0 关；开着会占掉画面底部一条）
filmstrip=0

# 标题栏锁定（1 开 / 0 关；锁住后整窗交互封锁，只留锁钮解锁）
locked=0

# 长图自动按宽适应（竖向可拖）
long_image_fit=0

# 滚轮：0 缩放 / 1 翻页
wheel_behavior=0

# Ctrl+滚轮：0 缩放 / 1 翻页
ctrl_wheel=1

# 启动窗口：0 固定尺寸 / 1 全屏（图片适应窗口）/ 3 图片尺寸（按启动首图原始像素开窗）
init_window=0

# 固定尺寸档的窗口宽高（最低 100，最高 8192）
window_w=800
window_h=600

# 窗口整体不透明度（百分制，10% 一档，最低 10）
opacity=90

# 停用的解码器 id，分号分隔（留空 = 全部启用）
decoders_disabled=

# 停用的扩展 id，分号分隔（留空 = 全部启用）
extensions_disabled=

# 连续失败被隔离的插件 id（三振自动写入；设置→插件里可解除）
extensions_isolated=

# 各扩展名的首选解码器（格式：扩展名=解码器id，分号分隔，如 jpg=jpeg;png=core；
# 留空 = 自动按解码器文件名顺序选择；指向不存在/已停用解码器的条目自动忽略）
decoders_preferred=

# 邻图预读：开着一张图时，后台把同文件夹前后各两张先解好（存的是视口那一档，
# 与图片本身多大无关，预算 ≤256 MiB）。翻页就是按下即换；关掉它则每次翻页现场解码
prefetch=1

# 关闭窗口改为收进系统托盘（真退出走托盘右键菜单；改动即时生效）
close_to_tray=0

# 空格预览：资源管理器/桌面选中图片按空格弹预览，再按收起（预览窗内 Esc 同）。默认关——
# 常驻低级键盘钩子监听全局空格（从不吞键，打字场景自动放行），仍属对全系统的输入介入，
# 须显式开启；改动即时生效
space_preview=0

# 裸数据参数窗的上次使用值（宽x高:布局，如 1920x1080:8；留空 = 还没用过）
raw_last=

# 联网来源：gitee / github —— 解码器清单、插件清单与将来的软件本体更新**共用这一个**
# （选哪个同时决定去哪拉清单、每一枚下载物用哪个地址）。一个用户机器的网络是固定的，
# 所以只该有一个来源开关：选项页「下载源」。
download_source=gitee

# 解码器清单地址：点「刷新」时按 `download_source` 选其中一条去拉 decoders.json。
# 可改成镜像 / 自建转发；留空 = 内置默认（下面就是内置默认值）
decoder_index_gitee={decoder_gitee}
decoder_index_github={decoder_github}

# 插件清单地址（同样可改；留空 = 内置默认）
plugin_index_gitee={plugin_gitee}
plugin_index_github={plugin_github}

# 快捷键绑定（动作id:键;动作id2:键2；每个动作至多一把）。整项留空 = 全部出厂默认；
# 单个动作写成 `fit:` = 明确解绑；键名写坏 = 该动作保持出厂默认（不会静默解绑）。动作 id：
# prev/next/first/last/zoom_in/zoom_out/fit/actual/rotate/flip/pause/filmstrip/fullscreen
hotkeys={hotkeys}
"
    )
}

/// 全部 ini 键（M4 消费方测试的枚举源）：新增持久化键必须登记在这里。
///
/// 机器读这三段的是三条不同的判据，别说混了：
/// - `saved_keys_match_all_keys`——盘上键集合 ≡ 本表（漏登记 / 漏写盘 ⇒ 红）；
/// - `every_key_has_a_load_consumer`——每个键都有解析臂且真的落到 `cfg.<字段>`（只写盘不解 ⇒ 红）；
/// - `build/hygiene_scan.py` 第 6 查 `config_last_mile`——本表每个键解到的字段，既要有**用户入口**
///   （`src/ui/**` 里引用它），也要有**行为读者**（除设置页与 config.rs 之外还有地方读它）。
///   前两条只证到 `ini → 字段`，**"字段落在谁手里"以前没有任何机器读过**（m121 补上）。
///   ★ 写在 UI 写点处的 `ini 键：<名>` 注释是**给人导航的约定，没有测试读它**（2026-10-24 实测：
///   20 个键里 13 个标了，没标的是 `double_click`、`filmstrip`、`locked`、`long_image_fit`、
///   `decoders_disabled`、`window_w`、`window_h`）——别把它当成门禁。
pub const ALL_KEYS: [&str; 28] = [
    "stay_on_top",
    "double_click",
    "slide_interval",
    "minimal_mode",
    "checker",
    "filmstrip",
    "locked",
    "long_image_fit",
    "wheel_behavior",
    "ctrl_wheel",
    "init_window",
    "window_w",
    "window_h",
    "opacity",
    "decoders_disabled",
    "extensions_disabled",
    "extensions_isolated",
    "decoders_preferred",
    "prefetch",
    "close_to_tray",
    "space_preview",
    "raw_last",
    "download_source",
    "decoder_index_gitee",
    "decoder_index_github",
    "plugin_index_gitee",
    "plugin_index_github",
    "hotkeys",
];

/// 配置专用键：只给高级用户手改 ini，**界面刻意不出现**（2026-10-29 用户口径）。
///
/// 两个门禁都读这张表，别在别处再抄第二份：
/// - `hygiene_scan.py` 第 6 查：这类键只豁免"用户入口"，**行为读者照旧必须有**；
/// - `ui_touchpoints_cover_all_keys`（本文件测试）：跳过"UI 源码里必须出现这个键名"。
pub const CONFIG_ONLY_KEYS: [&str; 4] = [
    "decoder_index_gitee",
    "decoder_index_github",
    "plugin_index_gitee",
    "plugin_index_github",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 百分制 ↔ 滑条量纲必须**互相抵消**：少换算一端，设置窗会把 75% 显示成极低值
    /// （曾经的项目踩过：`Slider` 内部恒存 `0..=1`，直接当百分比用就成 1%）。
    #[test]
    fn slider_round_trips_opacity() {
        for p in [OPACITY_MIN, 30, 50, 70, 90, OPACITY_MAX] {
            let back = slider_to_opacity(opacity_to_slider(p));
            assert_eq!(back, p, "{p}% 走一趟滑条变了样");
        }
        assert_eq!(opacity_to_slider(OPACITY_MIN), 0.0, "最左=下限");
        assert_eq!(opacity_to_slider(OPACITY_MAX), 1.0, "最右=上限");
    }

    /// 两个镜像的清单地址是**逐个写死的**，不能用"换主机名"推导：
    /// Gitee 的 owner 是 `qiuzongman`，GitHub 是 `zongmanqiu`。2026-10-26 实测：
    /// 换主机名得到 `raw.githubusercontent.com/qiuzongman/...`，GitHub 直接 404。
    #[test]
    fn index_urls_are_pinned_per_mirror() {
        assert_eq!(
            SourceId::Gitee.index_url(),
            "https://raw.giteeusercontent.com/qiuzongman/RuPix-Decoders/raw/master/decoders.json"
        );
        assert_eq!(
            SourceId::Github.index_url(),
            "https://raw.githubusercontent.com/zongmanqiu/RuPix-Decoders/refs/heads/master/decoders.json"
        );
        assert_eq!(
            SourceId::Gitee.plugin_index_url(),
            "https://raw.giteeusercontent.com/qiuzongman/RuPix-Plugins/raw/main/plugins.json"
        );
        assert_eq!(
            SourceId::Github.plugin_index_url(),
            "https://raw.githubusercontent.com/zongmanqiu/RuPix-Plugins/refs/heads/main/plugins.json"
        );
        for s in SourceId::ALL {
            assert_eq!(SourceId::from_id(s.id()), s, "id 回读不自洽：{}", s.id());
            assert_eq!(
                SourceId::from_ui_index(s.ui_index()),
                s,
                "下拉位次回读不自洽：{}",
                s.label()
            );
        }
    }

    /// 下载地址必须落在**本来源**的 repo 下：旧清单把 Gitee 的 owner 带进 GitHub
    /// 地址（`github.com/qiuzongman/...`），点下载只会 404；这类地址要判成"来源不可用"，
    /// 让界面灰掉，而不是给一个点了报错的按钮。
    #[test]
    fn download_urls_must_match_their_source() {
        let gitee_ok = "https://gitee.com/qiuzongman/RuPix-Decoders/releases/download/x/x.dll";
        let github_ok = "https://github.com/zongmanqiu/RuPix-Decoders/releases/download/x/x.dll";
        let github_wrong_owner =
            "https://github.com/qiuzongman/RuPix-Decoders/releases/download/x/x.dll";

        assert!(SourceId::Gitee.accepts_download_url(gitee_ok));
        assert!(!SourceId::Gitee.accepts_download_url(github_ok));
        assert!(SourceId::Github.accepts_download_url(github_ok));
        assert!(!SourceId::Github.accepts_download_url(github_wrong_owner));
        // 非本仓域名不拦（单测夹具 / 未来自建镜像）。
        assert!(SourceId::Gitee.accepts_download_url("https://example.com/x.dll"));
        assert!(SourceId::Github.accepts_download_url("https://example.com/x.dll"));
    }

    /// 清单地址**落进 ini 且可改**（2026-10-28 用户口径：解码器与插件的联网更新网址
    /// 都要记在宿主配置文件里，允许用户换成镜像）。空值必须回落内置默认——
    /// 手工清空一行绝不该把「刷新」变成一条空 URL。
    #[test]
    fn index_urls_are_configurable_and_empty_falls_back() {
        let p = temp_ini("index_url");
        std::fs::write(
            &p,
            "decoder_index_gitee=https://mirror.example/d.json\n\
             decoder_index_github=\n\
             plugin_index_gitee=https://mirror.example/p.json\n\
             plugin_index_github=\n",
        )
        .unwrap();
        let c = Config::load_from(&p);
        assert_eq!(c.decoder_index_gitee, "https://mirror.example/d.json");
        assert_eq!(
            c.decoder_index_github,
            SourceId::Github.index_url(),
            "空值必须回落内置默认"
        );
        assert_eq!(c.plugin_index_gitee, "https://mirror.example/p.json");
        assert_eq!(
            c.plugin_index_github,
            SourceId::Github.plugin_index_url(),
            "空值必须回落内置默认"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// 老 ini 补登记：缺的补上、已有的不动、用户自己的注释一行不丢；
    /// 补完之后再补一次应当是**幂等**的（不重复追加）。
    #[test]
    fn backfill_appends_missing_index_keys_once() {
        let p = temp_ini("backfill");
        // 老版本 ini：没有四个地址键，但有自己的注释与一个已自定义的值。
        std::fs::write(
            &p,
            "# 我自己的笔记\nstay_on_top=1\nplugin_index_gitee=https://mine.example/p.json",
        )
        .unwrap();
        let c = Config::load_from(&p);
        assert_eq!(c.decoder_index_gitee, SourceId::Gitee.index_url());
        c.backfill_index_urls(&p);

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("# 我自己的笔记\nstay_on_top=1\n"));
        assert_eq!(
            text.matches("plugin_index_gitee=").count(),
            1,
            "已有的键不该被重复追加"
        );
        for k in [
            "decoder_index_gitee",
            "decoder_index_github",
            "plugin_index_github",
        ] {
            assert_eq!(text.matches(&format!("{k}=")).count(), 1, "{k} 应补一次");
        }
        // 补齐后能原样读回，且值就是内置默认。
        let c2 = Config::load_from(&p);
        assert_eq!(c2.decoder_index_github, SourceId::Github.index_url());
        assert_eq!(c2.plugin_index_gitee, "https://mine.example/p.json");

        // 幂等：再补一次不再追加。
        let before = text.clone();
        c2.backfill_index_urls(&p);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
        let _ = std::fs::remove_file(&p);
    }

    /// 滑条是连续浮点的，越界必须夹住：面板宽度算错或信号被外部写脏时，
    /// 值掉出 `[0,1]` 也不能把配置写成 0%（窗口整个消失）。
    #[test]
    fn slider_extremes_stay_visible() {
        assert_eq!(slider_to_opacity(-3.0), OPACITY_MIN);
        assert_eq!(slider_to_opacity(9.9), OPACITY_MAX);
        assert!(slider_to_opacity(0.0) >= OPACITY_MIN);
    }

    /// 出厂默认必须是 80%（用户拍板 2026-10-09：步进 10% 后取整档）。
    #[test]
    fn default_is_eighty_percent() {
        assert_eq!(Config::default().opacity, OPACITY_DEFAULT);
        assert_eq!(OPACITY_DEFAULT, 90);
    }

    /// 所有档位都是 10% 的整数倍：右键菜单与设置滑条看到的是同一套档。
    #[test]
    fn quantize_snaps_to_ten_percent_steps() {
        // 菜单档位原样通过。
        for p in (1..=10).map(|i| i * 10) {
            assert_eq!(quantize_opacity(p), p, "{p}% 本身就是整档");
        }
        // 旧默认 75 这类非整档值：四舍五入到最近的档。
        assert_eq!(quantize_opacity(75), 80);
        assert_eq!(quantize_opacity(74), 70);
        assert_eq!(quantize_opacity(88), 90);
        // 越界夹回 [MIN, MAX] 且仍是整档。
        assert_eq!(quantize_opacity(0), OPACITY_MIN);
        assert_eq!(quantize_opacity(255), OPACITY_MAX);
    }

    /// 磁盘上被手工改坏的值要被夹回区间，而不是让窗口变得不可见。
    #[test]
    fn opacity_out_of_range_loads_clamped() {
        // 直接构造函数槽位，绕开实际文件读写（"0%" 的窗口看不见，属于必须挡住的失败）。
        let c = Config {
            opacity: 0,
            ..Config::default()
        };
        let clamped = c.opacity.clamp(OPACITY_MIN, OPACITY_MAX);
        assert_eq!(clamped, OPACITY_MIN);
    }

    /// 临时文件路径（按 pid 区分，多个测试并行跑不会互相踩）。
    fn temp_ini(tag: &str) -> std::path::PathBuf {
        crate::core::paths::test_scratch_root().join(format!(
            "rupix_{}_{}.ini",
            std::process::id(),
            tag
        ))
    }

    /// 真·落盘往返：把**全部**设置项翻转成非默认值，存完读回来必须逐字节一致。
    ///
    /// 这条覆盖了 `save_to` 的键名与 `load_from` 的解析必须一一对应的要求——
    /// 加了新设置项却只在 `save` 里写了、忘了 `load`（或反过来），这里是唯一能抓到的地方。
    #[test]
    fn full_round_trip_survives_disk() {
        let p = temp_ini("roundtrip");
        let c = Config {
            stay_on_top: true,
            dbl_click: DblClick::Fullscreen,
            minimal_mode: true,
            checker: true,
            filmstrip: true,
            locked: true,
            long_image_fit: true,
            wheel: Wheel::Switch,
            init_window: InitWindow::Fullscreen,
            window_w: 1024,
            window_h: 768,
            opacity: 30,
            decoders_disabled: vec!["abc".to_string(), "def".to_string()],
            decoders_preferred: BTreeMap::from([
                ("jpg".to_string(), "jpeg".to_string()),
                ("png".to_string(), "png".to_string()),
            ]),
            prefetch: false,
            close_to_tray: true,
            space_preview: true,
            raw_last: Some(crate::core::decode::RawParams {
                width: 1920,
                height: 1080,
                layout: 8,
            }),
            ..Config::default()
        };
        c.save_to(&p);

        let back = Config::load_from(&p);
        assert!(back.stay_on_top, "stay_on_top 没存住");
        assert_eq!(back.dbl_click, DblClick::Fullscreen);
        assert!(back.minimal_mode, "minimal_mode 没存住");
        assert!(back.checker, "checker 没存住");
        assert!(back.filmstrip, "filmstrip 没存住");
        assert!(back.locked, "locked 没存住");
        assert!(back.long_image_fit);
        assert_eq!(back.wheel, Wheel::Switch);
        assert_eq!(back.init_window, InitWindow::Fullscreen);
        assert_eq!(
            (back.window_w, back.window_h),
            (1024, 768),
            "窗口尺寸没存住"
        );
        assert_eq!(back.opacity, 30, "opacity 没存住");
        assert!(!back.prefetch, "prefetch 没存住");
        assert!(back.close_to_tray, "close_to_tray 没存住");
        assert!(back.space_preview, "space_preview 没存住");
        assert_eq!(
            back.decoders_disabled,
            vec!["abc".to_string(), "def".to_string()],
            "decoders_disabled 没存住"
        );
        assert_eq!(
            back.decoders_preferred,
            BTreeMap::from([
                ("jpg".to_string(), "jpeg".to_string()),
                ("png".to_string(), "png".to_string()),
            ]),
            "decoders_preferred 没存住"
        );
        assert_eq!(
            back.raw_last,
            Some(crate::core::decode::RawParams {
                width: 1920,
                height: 1080,
                layout: 8
            }),
            "raw_last 没存住"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// 值奇形怪状的文件也得扛住：缺键、乱码、越界，一律回默认而不是 panic
    /// （用户会手改 ini，这是便携配置最常见的现场形态）。
    #[test]
    fn garbage_file_falls_back_to_defaults() {
        let p = temp_ini("garbage");
        let _ = std::fs::write(&p, "opacity=abc\ndouble_click=999\n这不是键值对\n=空键\n");
        let c = Config::load_from(&p);
        assert_eq!(c.opacity, OPACITY_DEFAULT, "乱码退回默认");
        assert_eq!(c.dbl_click, DblClick::CloseWindow, "越界档位退回默认");
        assert!(!c.long_image_fit, "没提到的键保持默认");
        let _ = std::fs::remove_file(&p);
    }

    /// 布尔键只认 `1`/`0`：`true` / 空值 / 别的词都回落**该键自己的默认**。
    /// 判据必须挑默认是 `true` 的键（`prefetch`）——默认是 `false` 的键上，
    /// "写坏当 false" 与 "写坏回落 false" 读数相同，测不出方向性错误。
    #[test]
    fn illegal_bool_values_fall_back_to_their_own_defaults() {
        let p = temp_ini("bool-defaults");
        std::fs::write(
            &p,
            "prefetch=true\nspace_preview=\nclose_to_tray=yes\nstay_on_top=on\nlong_image_fit=2\n",
        )
        .unwrap();
        let c = Config::load_from(&p);
        assert!(
            c.prefetch,
            "prefetch 默认开：写坏必须回落 true，不能把笔误当成 0"
        );
        assert!(!c.space_preview, "空值回落默认 false");
        assert!(!c.close_to_tray, "乱词回落默认 false");
        assert!(!c.stay_on_top, "乱词回落默认 false");
        assert!(!c.long_image_fit, "越界档位回落默认 false");
        let _ = std::fs::remove_file(&p);
    }

    /// 写坏的 `minimal_mode` 不算"现行键显式出现"，不能让旧键 `show_title_bar`
    /// 永远被它压住；合法值时现行键照旧优先。
    #[test]
    fn invalid_minimal_mode_does_not_shadow_the_legacy_key() {
        let p = temp_ini("minimal-legacy");
        std::fs::write(&p, "show_title_bar=0\nminimal_mode=maybe\n").unwrap();
        let c = Config::load_from(&p);
        assert!(
            c.minimal_mode,
            "写坏的现行键应视作缺席，让合法的旧键 show_title_bar=0 说话"
        );

        std::fs::write(&p, "show_title_bar=0\nminimal_mode=0\n").unwrap();
        let c = Config::load_from(&p);
        assert!(!c.minimal_mode, "合法的现行键仍优先于旧键");
        let _ = std::fs::remove_file(&p);
    }

    /// 文件不存在时必须是**默认配置**而不是炸掉（首次启动就走这条）。
    #[test]
    fn missing_file_loads_defaults() {
        let c = Config::load_from(&std::path::PathBuf::from(
            "C:/不存在/也不可能/有这个文件.ini",
        ));
        assert_eq!(c.opacity, OPACITY_DEFAULT);
        assert_eq!(c.dbl_click, DblClick::CloseWindow);
    }

    /// decoders_preferred 的解析容错：缺 `=`、任一侧为空、多余分号的条目一律整条
    /// 忽略，好条目原样保留——用户手改 ini 写坏一条，不能把整张首选表带崩。
    #[test]
    fn decoders_preferred_tolerates_bad_entries() {
        let p = temp_ini("pref-bad");
        std::fs::write(&p, "decoders_preferred=jpg=jpeg;;=x;png=;=png;qoi=qoi\n").unwrap();
        let c = Config::load_from(&p);
        assert_eq!(
            c.decoders_preferred,
            BTreeMap::from([
                ("jpg".to_string(), "jpeg".to_string()),
                ("qoi".to_string(), "qoi".to_string()),
            ]),
            "坏条目应整条忽略，好条目原样保留"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// 默认配置写出来的值一定等于再读一遍读到的值（幂等：反复保存不会把配置漂咸）。
    #[test]
    fn save_then_load_is_idempotent() {
        let p = temp_ini("idem");
        Config::default().save_to(&p);
        let once = Config::load_from(&p);
        once.save_to(&p);
        let twice = Config::load_from(&p);
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            {
                let cfg = Config {
                    opacity: twice.opacity,
                    ..Config::default()
                };
                cfg.save_to(&p);
                std::fs::read_to_string(&p).unwrap()
            },
            "存两次文件内容就变了"
        );
        assert_eq!(once.opacity, twice.opacity);
        let _ = std::fs::remove_file(&p);
    }

    /// `slide_interval`（m167 幻灯片每张停留**秒数**，用户 2026-10-31 点名要整数输入）：
    /// 解得动就用、越界就夹、解不动回默认——与 `double_click` 同一条判据：写错的值不报错。
    /// 夹紧只有一处定义（[`clamp_slide_interval`]），解析臂与设置页文本框都调它；
    /// 上下界各写一遍就是三份"同一个数字"，改的时候必漏一处。
    #[test]
    fn slide_interval_clamps_and_survives_junk() {
        assert_eq!(
            clamp_slide_interval(0),
            SLIDE_SEC_MIN,
            "0 秒不该被当成合法值"
        );
        assert_eq!(clamp_slide_interval(99999), SLIDE_SEC_MAX);
        assert_eq!(clamp_slide_interval(7), 7);

        let p = temp_ini("interval");
        std::fs::write(&p, "slide_interval=12\n").unwrap();
        assert_eq!(Config::load_from(&p).slide_interval, 12);
        std::fs::write(&p, "slide_interval=0\n").unwrap();
        assert_eq!(Config::load_from(&p).slide_interval, SLIDE_SEC_MIN);
        std::fs::write(&p, "slide_interval=喵\n").unwrap();
        assert_eq!(
            Config::load_from(&p).slide_interval,
            3,
            "解不动的值不该报错，只该回默认"
        );
        // 夹过的值写出去再读回来必须还在（不然设置页会"弹回去"）。
        let c = Config {
            slide_interval: clamp_slide_interval(5000),
            ..Config::default()
        };
        c.save_to(&p);
        assert_eq!(Config::load_from(&p).slide_interval, SLIDE_SEC_MAX);
        let _ = std::fs::remove_file(&p);
    }

    /// 联网来源只有**一个**键（2026-10-30 用户口径：一个用户机器的网络是固定的）。
    ///
    /// 旧形状是 `decoder_source` + `plugin_source` 各存一份，两个页面的下拉能互相打架——
    /// 那正是要消灭的不一致。这条钉三件事：登记表里只有一个 source 键、两个取值都认、
    /// 旧键**不再读取**（一次性重设，不背兼容垫片）。

    #[test]
    fn download_source_is_a_single_key_and_clamps_unknown() {
        let srcs: Vec<&str> = ALL_KEYS
            .iter()
            .copied()
            .filter(|k| k.ends_with("_source"))
            .collect();
        assert_eq!(srcs, vec!["download_source"], "联网来源只能有一个开关");

        let p = temp_ini("src-key");
        std::fs::write(
            &p,
            "download_source=github
",
        )
        .unwrap();
        assert_eq!(
            Config::load_from(&p).download_source,
            SourceId::Github,
            "github 要认"
        );
        std::fs::write(
            &p,
            "download_source=gitee
",
        )
        .unwrap();
        assert_eq!(Config::load_from(&p).download_source, SourceId::Gitee);
        std::fs::write(
            &p,
            "download_source=gitlab
",
        )
        .unwrap();
        assert_eq!(
            Config::load_from(&p).download_source,
            SourceId::Gitee,
            "未知值夹紧回默认（越界夹紧，同其它选项）"
        );
        std::fs::write(
            &p,
            "decoder_source=github
plugin_source=github
",
        )
        .unwrap();
        assert_eq!(
            Config::load_from(&p).download_source,
            SourceId::Gitee,
            "旧的两个键不再读取：老用户重设一次，不背兼容垫片"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// M4 消费方测试之一：默认配置写出的键集合必须与 [`ALL_KEYS`] **完全一致**——
    /// 加了键忘登记、登记了却没写盘，都在这里红。
    #[test]
    fn saved_keys_match_all_keys() {
        let p = temp_ini("keys");
        Config::default().save_to(&p);
        let text = std::fs::read_to_string(&p).unwrap();
        let mut got: Vec<&str> = text
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, _)| k))
            .collect();
        got.sort_unstable();
        let mut want: Vec<&str> = ALL_KEYS.to_vec();
        want.sort_unstable();
        assert_eq!(got, want, "盘上键集合 ≠ ALL_KEYS（新键漏登记或漏写盘）");
        let _ = std::fs::remove_file(&p);
    }

    /// M4 消费方测试之二：每个键都必须有**解析消费方**——喂一遍与默认不同的
    /// 哨兵值，逐字段断言真的生效。加键只写了 `save_to` 忘了 `load_from` 的
    /// 分支，这里必红。
    #[test]
    fn every_key_has_a_load_consumer() {
        let p = temp_ini("consume");
        std::fs::write(
            &p,
            "stay_on_top=1\ndouble_click=3\nminimal_mode=1\n\
             long_image_fit=1\nwheel_behavior=1\nctrl_wheel=0\ninit_window=3\nwindow_w=1024\n\
             window_h=768\nopacity=30\n\
             decoders_disabled=abc;def\nextensions_disabled=com.example.ocr\ndecoders_preferred=jpg=jpeg;png=png\nprefetch=0\nclose_to_tray=1\nspace_preview=1\nraw_last=1x2:3\ndownload_source=github\n\
             decoder_index_gitee=https://a.example/d.json\ndecoder_index_github=https://b.example/d.json\n\
             plugin_index_gitee=https://a.example/p.json\nplugin_index_github=https://b.example/p.json\n\
             hotkeys=prev:Right\n",
        )
        .unwrap();
        let c = Config::load_from(&p);
        assert!(c.stay_on_top, "stay_on_top 没有消费方");
        assert_eq!(c.dbl_click, DblClick::Fullscreen, "double_click 没有消费方");
        assert!(c.minimal_mode, "minimal_mode 没有消费方");
        assert!(c.long_image_fit, "long_image_fit 没有消费方");
        assert_eq!(c.wheel, Wheel::Switch, "wheel_behavior 没有消费方");
        assert_eq!(c.ctrl_wheel, CtrlWheel::Zoom, "ctrl_wheel 没有消费方");
        assert_eq!(c.init_window, InitWindow::Natural, "init_window 没有消费方");
        assert_eq!(
            (c.window_w, c.window_h),
            (1024, 768),
            "window_w/window_h 没有消费方"
        );
        assert_eq!(c.opacity, 30, "opacity 没有消费方");
        assert_eq!(
            c.download_source,
            SourceId::Github,
            "download_source 没有消费方（两张卡与将来的本体更新都读它）"
        );
        assert_eq!(
            c.decoders_disabled,
            vec!["abc".to_string(), "def".to_string()],
            "decoders_disabled 没有消费方"
        );
        assert_eq!(
            c.extensions_disabled,
            vec!["com.example.ocr".to_string()],
            "extensions_disabled 没有消费方"
        );
        assert_eq!(
            c.decoders_preferred,
            BTreeMap::from([
                ("jpg".to_string(), "jpeg".to_string()),
                ("png".to_string(), "png".to_string()),
            ]),
            "decoders_preferred 没有消费方"
        );
        // 默认是"开"，所以这里喂 `0` 才验得出这条键真有人消费。
        assert!(!c.prefetch, "prefetch 没有消费方");
        assert!(c.close_to_tray, "close_to_tray 没有消费方");
        assert!(c.space_preview, "space_preview 没有消费方");
        assert_eq!(
            c.raw_last,
            Some(crate::core::decode::RawParams {
                width: 1,
                height: 2,
                layout: 3
            }),
            "raw_last 没有消费方"
        );
        assert_eq!(
            c.hotkeys.bindings[crate::core::hotkey::Action::Prev.index()],
            vec![crate::core::hotkey::Hotkey::from_spec("Right").unwrap()],
            "hotkeys 没有消费方"
        );
        assert_eq!(c.decoder_index_gitee, "https://a.example/d.json");
        assert_eq!(c.decoder_index_github, "https://b.example/d.json");
        assert_eq!(c.plugin_index_gitee, "https://a.example/p.json");
        assert_eq!(c.plugin_index_github, "https://b.example/p.json");
        let _ = std::fs::remove_file(&p);
    }

    /// M4 消费方测试之三：每个键都必须有 **UI/菜单写点**（写点源码里留有
    /// `ini 键：<名>` 注释，`include_str!` 编译期取进来核对）。键只存在于
    /// 配置层、界面改不到它，就是死键。
    ///
    /// ★ 2026-10-29：登记在 [`CONFIG_ONLY_KEYS`] 里的键例外——它们是给高级用户
    ///   手改 ini 的，界面刻意不出现；但必须先证明登记表没写错（每个都真在
    ///   [`ALL_KEYS`] 里）。
    ///
    /// ★ 2026-10-17 `ui/settings.rs` 拆成 `ui/settings/` 各页后，这里逐文件列出：
    ///   `include_str!` 不支持通配，编译期取源码是这个测试的立身之本（不靠运行时
    ///   去找目录）。漏列一页的后果不是静默通过，而是"那一页上的键报找不到写点"——
    ///   报错是响的。
    #[test]
    fn ui_touchpoints_cover_all_keys() {
        let ui = concat!(
            include_str!("../ui/settings/mod.rs"),
            include_str!("../ui/settings/about.rs"),
            include_str!("../ui/settings/exts.rs"),
            include_str!("../ui/settings/metadata.rs"),
            include_str!("../ui/settings/options.rs"),
            include_str!("../ui/settings/decoders_tab.rs"),
            include_str!("../ui/settings/extensions_tab.rs"),
            include_str!("../ui/settings/rows.rs"),
            include_str!("../ui/settings/shortcuts.rs"),
            include_str!("../ui/canvas.rs"),
            include_str!("../ui/raw_dialog.rs"),
        );
        for k in CONFIG_ONLY_KEYS {
            assert!(
                ALL_KEYS.contains(&k),
                "配置专用键 {k} 不在 ALL_KEYS 里：登记表写错了（本测试会跳过它，等于开小差）"
            );
        }
        for k in ALL_KEYS {
            if CONFIG_ONLY_KEYS.contains(&k) {
                continue;
            }
            assert!(
                ui.contains(k),
                "键 {k} 找不到 UI/菜单写点：在写出处加 `ini 键：{k}` 注释"
            );
        }
    }

    /// M3：首启生成的注释模板解析回来必须是**纯默认值**（模板漂移了默认值，
    /// 用户首启就会拿到一份和自己想的不一样的配置）。
    #[test]
    fn default_ini_parses_to_defaults() {
        let p = temp_ini("first");
        std::fs::write(&p, default_ini()).unwrap();
        assert_eq!(
            Config::load_from(&p),
            Config::default(),
            "注释模板≠出厂默认"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// 固定尺寸档的宽高必须被夹进 [100, 8192]：ini 手改成 50 抬到 100，
    /// 天文数字压回 8192，正常值原样通过。
    #[test]
    fn window_size_clamps_in_range() {
        assert_eq!(clamp_window_size(50, 50), (WINDOW_MIN, WINDOW_MIN));
        assert_eq!(clamp_window_size(99999, 0), (WINDOW_MAX, WINDOW_MIN));
        assert_eq!(clamp_window_size(800, 600), (800, 600));
        let p = temp_ini("winclamp");
        std::fs::write(&p, "window_w=50\nwindow_h=99999\n").unwrap();
        let c = Config::load_from(&p);
        assert_eq!((c.window_w, c.window_h), (WINDOW_MIN, WINDOW_MAX));
        let _ = std::fs::remove_file(&p);
    }

    /// 旧 ini 的 init_window 值必须按既定映射升级，不能误读：
    /// 1(旧"最大化")→全屏；2(旧"常规窗口"，退役值)→固定；3(新)→图片尺寸。
    #[test]
    fn init_window_legacy_values_map_correctly() {
        assert_eq!(InitWindow::from_index(0), InitWindow::Fixed);
        assert_eq!(InitWindow::from_index(1), InitWindow::Fullscreen);
        assert_eq!(
            InitWindow::from_index(2),
            InitWindow::Fixed,
            "旧「常规」不得误读成图片尺寸"
        );
        assert_eq!(InitWindow::from_index(3), InitWindow::Natural);
        assert_eq!(InitWindow::from_index(99), InitWindow::Fixed);
    }

    /// UTF-8 BOM 不许把第一行键名吃掉（Windows 编辑器常见）。
    #[test]
    fn utf8_bom_does_not_eat_the_first_key() {
        let p = temp_ini("bom");
        std::fs::write(&p, "\u{feff}stay_on_top=1\nopacity=50\n").unwrap();
        let c = Config::load_from(&p);
        assert!(c.stay_on_top, "BOM 后的第一个键没被读到");
        assert_eq!(c.opacity, 50);
        let _ = std::fs::remove_file(&p);
    }

    /// 设置页下拉的位置与 ini 存储值是两套编号：`Natural` 存 3、显示在第 2 项。
    #[test]
    fn init_window_ui_positions_are_dense_and_round_trip() {
        for (i, w) in InitWindow::ALL.iter().enumerate() {
            assert_eq!(w.ui_index(), i, "下拉位置必须连续");
            assert_eq!(InitWindow::from_ui_index(i), *w);
        }
        assert_eq!(InitWindow::from_ui_index(3), InitWindow::Fixed, "越界回落");
        assert_eq!(InitWindow::Natural.index(), 3, "ini 存储值不变");
    }

    /// 注释模板也必须包含全部键（注释里解释了 A 键、盘上却写出 B 键 = 文档失真）。
    #[test]
    fn default_ini_mentions_every_key() {
        for k in ALL_KEYS {
            assert!(default_ini().contains(k), "注释模板没写 {k}");
        }
    }

    /// 防抖的三条判据：没脏不写、未到点不写、到点只写一次；`flush` 不计时刻但只要脏过就写。
    ///
    /// 时间是传进来的（[`poll_at`]），所以这里不需要 sleep 五秒；落点是临时文件，
    /// 不会在测试二进制旁边留一份像真配置的 ini 迷惑下次取证。
    #[test]
    fn debounce_writes_only_when_due() {
        let p = temp_ini("debounce");
        let _ = std::fs::remove_file(&p);
        let c = Config::default();
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);

        assert!(!poll_at(&c, &p, at(99)), "没脏却写了盘");
        assert!(!p.exists(), "没脏不该有文件");

        mark_dirty();
        assert!(
            !poll_at(&c, &p, at(1)),
            "未到点就落盘 = 连点滑条每一下都写一遍盘"
        );
        assert!(!p.exists(), "未到点不该有文件");

        assert!(poll_at(&c, &p, at(10)), "到点没写");
        assert!(p.exists(), "到点要落盘");
        assert!(!poll_at(&c, &p, at(20)), "同一份脏不许写第二次");
        assert!(!flush_at(&c, &p), "清完脏之后的 flush 不该写");

        // 退出兜底（K44）：脏了就当场落盘，不等防抖。
        mark_dirty();
        assert!(flush_at(&c, &p), "退出兜底必须落盘");
        assert!(!flush_at(&c, &p), "兜底只此一次");

        let _ = std::fs::remove_file(&p);
    }
}
