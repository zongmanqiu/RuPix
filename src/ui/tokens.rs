//! 设计令牌：**全 `ui/` 层唯一的配色 / 字号 / 间距来源**（对齐 CurSeen `ui/tokens.rs`）。
//!
//! # 为什么要单独一个文件、而不是留在用得最多的那个窗里
//!
//! 这一套原本是**设置窗私有的** `mod tokens`（写在 `ui/settings/mod.rs` 里）。结果是
//! 设置窗之外那几个"画在图片上的"界面——顶部胶囊（`chrome`）、底部缩略图条
//! （`filmstrip`）、鸟瞰与状态浮层（`canvas`）——**各自另造了一套色值**：
//! 光是同一块深灰 `28,28,30,225` 就在 `chrome` 的 `PILL_BG` 与 `canvas` 的鸟瞰底板里
//! 各抄了一遍，悬停白 `255,255,255,44` 也在 `chrome` 里出现两次。
//! 改一档配色要改三四处，且改完没人知道还有没有漏的。
//!
//! 现在的规矩（与 CurSeen 同口径）：
//! - **换肤入口就在这一个文件**；`ui/` 其它文件一律不写字面色值
//!   （`Color::rgba(...)` 那种字面量只许出现在本文件里）。
//! - 忘了哪块是什么颜色，就来这里按名字找；名字即语义（`OVERLAY_*` 是压在图片上的
//!   那套深色，其余是窗口内的浅色）。
//! - 兄弟模块取用一律走 `use super::tokens::…`（`ui/mod.rs` 是本文件唯一声明点），
//!   不许哪个模块自己再开一份。
//!
//! # 三组色，边界只有一条
//!
//! | 组 | 用在哪 | 跟主题走吗 |
//! |---|---|---|
//! 浅色组（[`BG`] / [`TEXT`] / [`BORDER`] …） | 设置窗、属性窗、对话框这类**独立窗口** | **不跟**。永远是白底深字——用户 2026-11-02 明说「不影响我们的设置页面颜色，只是影响看图器主窗口」 |
//! 浮层·跟随主题（[`empty_bg`] / [`pill_bg`] / [`overlay_text`] …） | 压在**我们自己的底**上的东西：空窗底、棋盘格、胶囊、缩略图条与格、浮层文字 | 跟。[`Theme`] 深 / 浅两套表 |
//! 浮层·恒定（[`STROKE_DARK`] / [`NAV_FILL`] / [`SLIDING_BG`] / [`BTN_DANGER_HOVER`]） | 压在**用户图片的像素**上：双层框线、当前视口高亮、放映进度、危险红 | 不跟。它们的职责是"任意图片上都看得见"，翻成反色会在深色照片上失效 |
//!
//! 判断一个新颜色归哪组，别问"它画在哪个文件里"，只问**它压在什么上**。
//!
//! ★ 带 alpha 的走 [`to_color4`]（RGBA 四元组），不透明的走 [`to_color`]（RGB 三元组）；
//!   两者都是 `const fn`，`pub const NAV_FILL: Color = to_color4(OVERLAY_NAV_FILL);` 这种
//!   写法不产生任何运行期开销（windui 的 `Color::rgba` 同为 `const fn`）。
//!
//! ★ 跟随主题那组**不能**是 `const`：主题是运行期切的（[`set_theme`]），所以它们是
//!   **函数**（`tokens::pill_bg()`）。多一次分支与一次 relaxed 原子读，在绘制热点上
//!   可以忽略——换来的是"设置页换主题，看图窗口当场变色"。

use std::sync::atomic::{AtomicU8, Ordering};

use windui::geometry::Color;

// ── 主题：两套浮层配色（2026-11-02 用户口径：「只需要两种颜色方案切换」）──────
//
// 只有**浮层**这一组跟主题走。浅色组（`BG` / `TEXT` / `BORDER` …）恒定不变——
// 用户明说「不影响我们的设置页面颜色，只是影响看图器主窗口」：设置页、属性窗、
// 对话框永远是白底深字那一套，两套主题下长得一模一样。
//
// 切换是**运行期**的：`set_theme` 写一个 `AtomicU8`，各绘制点在下一帧读到的就是
// 新值；它返回"是否真的变了"，runtime 拿这个返回值决定要不要重绘。

/// 看图窗口的配色方案（ini 键 `ui_theme`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// 深色：压在图片上的浮层用深底白字（默认，2026-10-26 以来的观感）。
    #[default]
    Dark,
    /// 浅色：同一批浮层换白底深字。**不碰设置页**。
    Light,
}

// ★ 这里**只有**变体与"从配置转过来"：`ALL` / `id` / `label` / `from_id` 那一套
//   属于 ini 语义，归 [`crate::core::config::UiTheme`] 管。两处各写一份的下场是
//   "改了落盘写法、界面文案没跟上"——2026-11-02 第一版就复制了一遍，`dead_code`
//   当场把四个方法全报出来（没有任何调用方），于是收掉。
impl Theme {
    /// 配置层的 [`UiTheme`](crate::core::config::UiTheme) → 绘制令牌。
    ///
    /// 桥放在这一侧（ui 认识 core，反过来不行）：`core::config` 只管 ini 怎么写，
    /// `ui::tokens` 只管画成什么色，两边各自演进时只动这一个函数。
    pub fn from_config(t: crate::core::config::UiTheme) -> Theme {
        match t {
            crate::core::config::UiTheme::Dark => Theme::Dark,
            crate::core::config::UiTheme::Light => Theme::Light,
        }
    }
}

/// 当前主题。`0 = Dark`、`1 = Light`。
///
/// 为什么是 `AtomicU8` 而不是信号：读它的是**绘制路径**（每帧、每个浮层元素各一次），
/// 而写它的是 tick 里的配置差分。一次 relaxed load 比走一遍信号系统的成本低得多，
/// 且两档主题只有两个值，不需要任何一致性保证——读到旧值最多让画面晚一帧变色。
static CURRENT_THEME: AtomicU8 = AtomicU8::new(0);

/// 读当前主题。
pub fn theme() -> Theme {
    match CURRENT_THEME.load(Ordering::Relaxed) {
        1 => Theme::Light,
        _ => Theme::Dark,
    }
}

/// 切主题，返回**是否真的变了**（调用方据此决定要不要重绘）。
pub fn set_theme(t: Theme) -> bool {
    let code = match t {
        Theme::Dark => 0u8,
        Theme::Light => 1u8,
    };
    CURRENT_THEME.swap(code, Ordering::Relaxed) != code
}

// ── 排版 ──────────────────────────────────────────────────────────────

pub const FONT_MD: f32 = 14.0;
/// 内容行高（设置页每一行的固定高度）。
pub const ROW_H: i32 = 40;
/// 两列版式的标签列宽（右侧控件列与它同一条左缘）。
pub const COL1_W: i32 = 208;
/// 控件列里"输入类控件"的统一宽（下拉 / 输入框 / 快捷键捕获框同宽，右缘对齐）。
pub const CONTROL_GROUP_W: i32 = 200;
/// 窄数值框（滑块右侧那个只读数字）。
pub const INPUT_W: i32 = 72;
pub const SLIDER_W: i32 = 120;
/// ★ **唯一的竖向节奏**：页四周留白、卡↔卡、卡片标题条↔正文、正文内行↔行，
/// 全部走这一个数（2026-10-25 用户口径「所有行间距、四周间距都一样」）。
///
/// 之前是三种数各管一处（页顶 4 / 卡间 8 / 卡头到正文 0），于是「已安装」里
/// 那颗「文件夹」按钮就贴在标题条下缘上了。
///
/// 横向另有一套（页 16 / 行 16 / 卡头 16）——那是**对齐**而不是节奏：三者同值
/// 才能让按钮、解码器行、卡片标题文字落在同一条左缘上，别拿 GAP 去替它。
pub const GAP: i32 = 8;
/// 页顶/页底留白 = 同一格节奏（各页共用，改一处全页跟着走）。
pub const PAGE_TOP_MARGIN: i32 = GAP;

/// 自绘对话框（裸数据参数、插件设置、跳转）的四边留白。**三扇窗共用同一格**：
/// 用户 2026-10-31 点名"底部按钮离边框的距离要一样" —— 各写各的数字就会走散。
pub const DIALOG_PAD: i32 = 16;
/// 两列版式对话框的窗宽：**唯一来源**，插件设置窗与跳转窗都吃这一个数。
/// 标签左缘 = `DIALOG_PAD` = 16，控件右缘 = 16 + `COL1_W`(208) +
/// `CONTROL_GROUP_W`(200) = 424，右边再留 40 给行尾的 `flex_spacer`。
/// 不写死 480：前三个令牌谁改了窗宽都跟着走，不会两扇窗各算一遍然后漂开。
pub const DIALOG_W: i32 = DIALOG_PAD * 2 + COL1_W + CONTROL_GROUP_W + 40;
/// `fit_content` 的下限。只防"空窗"，不参与排版：内容自己撑多高就多高，
/// 给大了就会在按钮下面留一条谁也不认领的空白（用户点名的正是这一条）。
pub const DIALOG_FIT_MIN: i32 = 120;
/// 卡片标题条高（CurSeen `HEADER_H` 同值 36）。
pub const CARD_HEADER_H: i32 = 36;
/// 卡片与折叠箭头的容器级圆角（CurSeen `CORNER_RADIUS` 同值 8；比控件级 6 大一号，
/// 因为它是容器而不是输入控件）。
pub const CARD_CORNER: f32 = 8.0;
/// 输入类控件高（CurSeen 快捷键框同高 30）。
pub const CONTROL_H: i32 = 30;
/// 「可下载」行里**版本号**那一格的固定宽（右对齐）。
///
/// 各条目版本串长短不一（`v0.1.0` / `v0.10.2` / `v1.0.0-rc1`），宽度不定死，
/// 后面「!」与动作按钮就会随它左右挪，列表看着像没对齐。
///
/// ★ 行尾那颗动作按钮**没有**这种常量（2026-10-26 撤掉 `ACTION_BTN_W`）：它只有
///   「下载」「升级」两条文案、都是两个汉字，宽度天然一致；钉一个数字反而白留边。
pub const VERSION_COL_W: i32 = 64;
/// 控件级圆角（对齐 windui 主题 corner_md=6，CurSeen `CORNER_CONTROL` 同值）。
pub const CORNER_CONTROL: f32 = 6.0;

/// 行尾**图标小按钮**的盒子尺寸（逻辑 px）：「!」详情与设置齿轮共用一格。
///
/// ★ 为什么必须钉死（2026-11-05 用户报障「齿轮按钮宽度和高度都不对」）：
///   按钮的盒子 = 字形度量 + Medium 内边距（左右 32 / 上下 18，windui 侧写死）。
///   「!」是正文字体里的一个窄字符（实测 5×18），而齿轮是 Segoe MDL2 的图标字形
///   （PUA 码位 U+E713，实测 14×14）——同一个 14 px 字号，两种字体的字形盒差得很远，
///   于是齿轮那颗自然长成 46×32，「!」是 37×36：宽出 9 px、矮了 4 px，并排看着就不是一套。
///
///   钉成「!」那颗的尺寸后，图标在框里居中（windui 按钮的文字/图标都是居中画的），
///   两颗按钮在任何机器上都严格同款。数字来自真实 DirectWrite 量测，
///   回归测试 `info_row_icon_buttons_share_one_box` 盯着它（FONT_MD 或界面字体变了会红）。
pub const ICON_BTN_W: i32 = 37;
pub const ICON_BTN_H: i32 = 36;

// ── 浅色系（独立窗口：设置 / 属性 / 对话框）──────────────────────────

pub const BG: [u8; 3] = [255, 255, 255];
pub const TEXT: [u8; 3] = [45, 52, 54];
pub const TEXT_SECONDARY: [u8; 3] = [99, 110, 114];
pub const ACCENT: [u8; 3] = [76, 139, 245];
/// 卡片标题条底：比卡片略深一档（CurSeen `CARD_HEADER_BG` 同值）。
pub const CARD_HEADER_BG: [u8; 3] = [241, 243, 245];
/// 浅灰边框（CurSeen `BORDER` 同值）。
pub const BORDER: [u8; 3] = [226, 230, 234];
/// 强调色（红）：段名与计数用（CurSeen `TEXT_ACCENT` 同值）。
pub const TEXT_ACCENT: [u8; 3] = [192, 57, 43];

// ── 浮层：跟着主题走的那一组 ─────────────────────────────────────────
//
// ★ 判据是"这块颜色压在什么上"，不是"它在哪个文件里画"：
//   * 压在我们自己的底上（空窗底、棋盘格、胶囊、缩略图背板与格、浮层文字）
//     ⇒ 跟主题走，深色一套、浅色一套；
//   * 压在**用户图片的像素**上（双层框线、当前视口高亮、放映进度色、危险红）
//     ⇒ 恒定，不跟主题。理由：这些标记的职责是"在任意图片上都看得见"，
//       浅色主题若把它们翻成深色，遇到深色照片就失效了。
//
// 想加一种颜色：先在下面的 `Overlay` 里起名字、两套表都填上，再写访问函数。
// 别在绘制点直接写字面值——`hygiene` 第 10 查 `literal_color` 会判红。

/// 跟随主题的浮层色（一张表 = 一套完整配色），字段名即语义。
struct Overlay {
    /// 空窗底 / 画布底（棋盘格关掉时铺满整个画面）。
    empty_bg: [u8; 3],
    /// 棋盘格两格（透明背景）：必须一深一浅交替，任何图都能看出哪块是透明的。
    checker_a: [u8; 3],
    checker_b: [u8; 3],
    /// 画布中央的**状态**文字（"载入中…"）。
    hint_text: [u8; 3],
    /// 画布中央的**邀请**文字（"把图片拖进来 · 右键打开"）。
    ///
    /// ★ 单独一档而不是复用 [`Overlay::hint_text`]（2026-11-02 用户口径：这句
    ///   "感觉要设置红色比较好"）：空窗是**等人做事**的状态，灰字容易被当成背景
    ///   装饰略过；"载入中…"是**已在办事**的状态，红字反而像出错了。两者同屏
    ///   不同义，所以两档颜色。
    hint_invite: [u8; 3],
    /// 画布中央的错误文字（打不开时那行）。
    error_text: [u8; 3],
    /// 顶部胶囊 / 鸟瞰面板底。
    pill_bg: [u8; 4],
    /// 胶囊内第一颗按钮前的分隔线。
    pill_sep: [u8; 4],
    /// 按钮悬停底（叠在 [`Overlay::pill_bg`] 之上）。
    btn_hover: [u8; 4],
    /// 按钮按下底（比悬停再重一档）。
    btn_press: [u8; 4],
    /// 翻页箭头悬停底（箭头是圆的、视觉面积更大，所以单独两档）。
    arrow_hover: [u8; 4],
    /// 翻页箭头按下底。
    arrow_press: [u8; 4],
    /// 底部缩略图条的背板（比胶囊更重一档，整条压在画面下缘）。
    panel_bg: [u8; 4],
    /// 缩略图格底。
    slot_bg: [u8; 4],
    /// 缩略图格悬停。
    slot_hover: [u8; 4],
    /// 缩略图格（当前图）。
    slot_cur: [u8; 4],
    /// 缩略图条滚动指示的轨道（只在目录长于一屏时出现）。
    strip_track: [u8; 4],
    /// 缩略图条滚动指示的滑块。
    strip_thumb: [u8; 4],
    /// 浮层上的正文。
    text: [u8; 3],
    /// 浮层上的次要文字（时间 / 尺寸那类）。
    text_muted: [u8; 3],
    /// 浮层上的激活态（工具栏开关点亮、缩略图当前格描边同一个色）。
    ///
    /// ★ 浅色主题下必须换成**深**绿：深色那档 `126,201,126` 压在 236α 的白胶囊上
    ///   几乎看不见（对比不足 1.6:1），"点亮"和"没点亮"就分不出来了。
    text_active: [u8; 3],
}

/// 深色（默认）：2026-10-26 以来压在图片上的那一套，一个字节都没改。
const DARK_OVERLAY: Overlay = Overlay {
    empty_bg: [15, 15, 15],
    checker_a: [34, 34, 34],
    // 浅格不能太暗（2026-11-01 用户实测）：以前 [48,48,48] 与深格只差 14 级，
    // 暗色调的图压在浅格上跟透明区糊成一片；提到 120 后深色内容与透明区一眼可分。
    checker_b: [120, 120, 120],
    hint_text: [128, 128, 128],
    // 与放映进度色同族（珊瑚红），但更实：那是一块会动的底，这是一行字，
    // 字要靠明度站住。
    hint_invite: [224, 86, 78],
    error_text: [224, 120, 120],
    pill_bg: [28, 28, 30, 225],
    pill_sep: [255, 255, 255, 34],
    btn_hover: [255, 255, 255, 28],
    btn_press: [255, 255, 255, 44],
    arrow_hover: [52, 52, 56, 230],
    arrow_press: [70, 70, 74, 235],
    panel_bg: [24, 24, 26, 232],
    slot_bg: [255, 255, 255, 12],
    slot_hover: [255, 255, 255, 34],
    slot_cur: [255, 255, 255, 22],
    strip_track: [255, 255, 255, 20],
    strip_thumb: [255, 255, 255, 78],
    text: [208, 208, 208],
    text_muted: [140, 140, 140],
    text_active: [126, 201, 126],
};

/// 浅色：同一批浮层换白底深字。
///
/// 数值不是"把深色反过来"算出来的，是按各自职责重配的：
/// - 背板走**近白微灰**（`246,246,248` / 白胶囊）而不是纯白：纯白压在浅色照片上
///   边界会糊掉，差 8–10 级刚好把浮层"托"起来，又不至于像一块灰板。
/// - 所有"亮一档表示悬停/按下"的叠加一律改成**压暗**（`0,0,0,a`）：白底上越叠越亮
///   会直接看不见，这是把深色做法反过来之后最容易踩的一脚。
/// - 棋盘格浅格是白、深格是 `221,223,228`（差 34 级），与深色主题的 86 级同一量级，
///   透明区在浅底上照样一眼可辨。
/// - 红系整体加深：`224,86,78` 这类亮红压在 246 的底上对比只有 2.6:1，读不清。
const LIGHT_OVERLAY: Overlay = Overlay {
    empty_bg: [246, 246, 248],
    checker_a: [255, 255, 255],
    checker_b: [221, 223, 228],
    hint_text: [128, 132, 138],
    hint_invite: [200, 56, 48],
    error_text: [198, 52, 46],
    pill_bg: [255, 255, 255, 236],
    pill_sep: [0, 0, 0, 30],
    btn_hover: [0, 0, 0, 18],
    btn_press: [0, 0, 0, 34],
    arrow_hover: [232, 234, 238, 238],
    arrow_press: [214, 217, 222, 242],
    panel_bg: [252, 252, 254, 240],
    slot_bg: [0, 0, 0, 10],
    slot_hover: [0, 0, 0, 28],
    slot_cur: [0, 0, 0, 16],
    strip_track: [0, 0, 0, 14],
    strip_thumb: [0, 0, 0, 62],
    text: [42, 48, 52],
    text_muted: [110, 118, 124],
    text_active: [40, 150, 80],
};

/// 当前主题那张表（一次原子读 + 一次分支，绘制热点上可忽略）。
fn overlay() -> &'static Overlay {
    match theme() {
        Theme::Dark => &DARK_OVERLAY,
        Theme::Light => &LIGHT_OVERLAY,
    }
}

// 访问函数：直接给 `Color`，省得每个绘制点在文件里再写一遍 `to_color(…)`。
// 名字带语义不带 `OVERLAY_` 前缀——调用点已经限定在浮层绘制里，前缀只会加噪声。

/// 空窗底 / 画布底。
pub fn empty_bg() -> Color {
    to_color(overlay().empty_bg)
}
/// 棋盘格深格。
pub fn checker_a() -> Color {
    to_color(overlay().checker_a)
}
/// 棋盘格浅格。
pub fn checker_b() -> Color {
    to_color(overlay().checker_b)
}
/// 画布中央的状态文字（"载入中…"）。
pub fn hint_text() -> Color {
    to_color(overlay().hint_text)
}
/// 画布中央的邀请文字（空窗那句，红）。
pub fn hint_invite() -> Color {
    to_color(overlay().hint_invite)
}
/// 画布中央的错误文字。
pub fn error_text() -> Color {
    to_color(overlay().error_text)
}
/// 胶囊 / 鸟瞰面板底。
pub fn pill_bg() -> Color {
    to_color4(overlay().pill_bg)
}
/// 胶囊内分隔线。
pub fn pill_sep() -> Color {
    to_color4(overlay().pill_sep)
}
/// 按钮悬停底。
pub fn btn_hover() -> Color {
    to_color4(overlay().btn_hover)
}
/// 按钮按下底。
pub fn btn_press() -> Color {
    to_color4(overlay().btn_press)
}
/// 翻页箭头悬停底。
pub fn arrow_hover() -> Color {
    to_color4(overlay().arrow_hover)
}
/// 翻页箭头按下底。
pub fn arrow_press() -> Color {
    to_color4(overlay().arrow_press)
}
/// 缩略图条背板。
pub fn panel_bg() -> Color {
    to_color4(overlay().panel_bg)
}
/// 缩略图格底。
pub fn slot_bg() -> Color {
    to_color4(overlay().slot_bg)
}
/// 缩略图格悬停。
pub fn slot_hover() -> Color {
    to_color4(overlay().slot_hover)
}
/// 缩略图格（当前图）。
pub fn slot_cur() -> Color {
    to_color4(overlay().slot_cur)
}
/// 缩略图条滚动指示的轨道。
pub fn strip_track() -> Color {
    to_color4(overlay().strip_track)
}
/// 缩略图条滚动指示的滑块。
pub fn strip_thumb() -> Color {
    to_color4(overlay().strip_thumb)
}
/// 浮层正文。
pub fn overlay_text() -> Color {
    to_color(overlay().text)
}
/// 浮层次要文字。
pub fn overlay_text_muted() -> Color {
    to_color(overlay().text_muted)
}
/// 浮层激活态。
pub fn overlay_text_active() -> Color {
    to_color(overlay().text_active)
}

// ── 浮层：恒定不跟主题的那一组 ───────────────────────────────────────
//
// 全部压在**用户图片的像素**上。它们的职责是"在任意图片上都看得见"，
// 所以两套主题共用同一份值——跟着翻会让浅色主题在深色照片上失效。

/// 双层高对比描边的**外黑**（鸟瞰框 / 视口框：先黑后白，黑图上白线可见）。
const OVERLAY_STROKE_DARK: [u8; 3] = [0, 0, 0];
/// 双层高对比描边的**内白**（白图上黑线可见——单色框在极端底色下会隐身）。
const OVERLAY_STROKE_LIGHT: [u8; 3] = [255, 255, 255];
/// 鸟瞰框里"当前视口"那块高亮（叠在缩略图上，缩略图内容任意）。
const OVERLAY_NAV_FILL: [u8; 4] = [255, 255, 255, 30];
/// **幻灯片放映的进度**色：右侧那颗翻页箭头的底色**自左向右**被这一档推满，推满即翻页。
///
/// ★ 用**色相**而不是灰阶：悬停 / 按下本来就是同一条灰阶上的亮度差，在深底上再叠
///   一档窗口整体不透明度，那点差根本比不出来（用户 2026-10-31 原话"本来就黑乎乎的，
///   感觉不明显"）。色相不吃明度，所以它在两套主题下都成立，不跟着翻。
/// ★ 选珊瑚红而不是琥珀（同日再判："黄色其实不适合我们当前的 UI 色系"）。
///   也**不是** [`BTN_DANGER_HOVER`] 那枚危险红（`232,17,35` 太饱和，语义是
///   "关闭/删除"）——持续几分钟的运行状态借用它会把"在放"读成"出事了"。
const OVERLAY_SLIDING_BG: [u8; 4] = [224, 86, 78, 215];
/// 关闭按钮悬停：红底（唯一一枚"危险"底，与浅色系 `TEXT_ACCENT` 是两套语义）。
/// 两套主题共用——危险色不该因为换了主题就换个红法。
const OVERLAY_BTN_DANGER_HOVER: [u8; 4] = [232, 17, 35, 220];
/// 缩略图**序号**的字：纯黑。
///
/// 压在缩略图的像素上（内容任意），所以归"恒定"这一组。用户 2026-11-08 两次口径合起来
/// 才完整：「里面是黑色，外面要包裹 1px 的白色」「不是一整个矩形区域都是其它颜色背景」
/// —— 所以要的是**描边文字**，不是色块标签。
const OVERLAY_BADGE_TEXT: [u8; 3] = [0, 0, 0];
/// 序号文字四周那 1px **白描边**（同一个字画八遍再压一遍黑字）。
///
/// 一黑一白互为退路：浅色照片上黑字自己就够，深色照片上黑字会融进背景、靠这圈白边
/// 勾出字形轮廓。没有底色块，缩略图不会被遮掉一块。
const OVERLAY_BADGE_HALO: [u8; 3] = [255, 255, 255];

/// 双层框线的外层（黑）。绘制顺序恒为「先 `STROKE_DARK` 后 `STROKE_LIGHT`」。
pub const STROKE_DARK: Color = to_color(OVERLAY_STROKE_DARK);
/// 双层框线的内层（白）。
pub const STROKE_LIGHT: Color = to_color(OVERLAY_STROKE_LIGHT);
/// 鸟瞰框里的当前视口高亮。
pub const NAV_FILL: Color = to_color4(OVERLAY_NAV_FILL);
/// 放映进度色。
pub const SLIDING_BG: Color = to_color4(OVERLAY_SLIDING_BG);
/// 危险（关闭）按钮悬停底色。
pub const BTN_DANGER_HOVER: Color = to_color4(OVERLAY_BTN_DANGER_HOVER);
/// 缩略图序号的字（黑）。
pub const BADGE_TEXT: Color = to_color(OVERLAY_BADGE_TEXT);
/// 缩略图序号的白描边。
pub const BADGE_HALO: Color = to_color(OVERLAY_BADGE_HALO);

// ── 转换 ──────────────────────────────────────────────────────────────

/// RGB → 不透明 `Color`（`const fn`：可直接用在 `const` 上，无运行期开销）。
pub const fn to_color(rgb: [u8; 3]) -> Color {
    Color::rgba(rgb[0], rgb[1], rgb[2], 255)
}

/// RGBA → `Color`（浮层那一组带 alpha 的走这里）。
pub const fn to_color4(rgba: [u8; 4]) -> Color {
    Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 切换是幂等的、且"没变"要如实报 false——runtime 靠这个返回值决定要不要重绘，
    /// 恒报 true 会让静止画面每帧重画一次（省电设计的老规矩）。
    #[test]
    fn theme_switch_is_idempotent_and_defaults_to_dark() {
        assert_eq!(
            theme(),
            Theme::Dark,
            "未调用 set 时必须是深色（AtomicU8 初值 0）"
        );

        assert!(set_theme(Theme::Light), "深色 → 浅色应当报变化");
        assert_eq!(theme(), Theme::Light);
        assert!(!set_theme(Theme::Light), "同值不该报变化");

        assert!(set_theme(Theme::Dark), "浅色 → 深色应当报变化");
        assert_eq!(theme(), Theme::Dark);
        assert!(!set_theme(Theme::Dark));
    }

    /// 两套表的**形状必须一致**：字段全填、且两个主题下至少底色与文字真的有区别。
    ///
    /// 这条防的是"加了新字段只填了深色表"——`LIGHT` 那边跟着默认值走，
    /// 浅色主题下某个浮层会用深色值，肉眼才看得出来，门禁看不出来。
    #[test]
    fn both_palettes_differ_where_they_should() {
        assert_ne!(
            DARK_OVERLAY.empty_bg, LIGHT_OVERLAY.empty_bg,
            "空窗底必须不同"
        );
        assert_ne!(
            DARK_OVERLAY.pill_bg, LIGHT_OVERLAY.pill_bg,
            "胶囊底必须不同"
        );
        assert_ne!(DARK_OVERLAY.text, LIGHT_OVERLAY.text, "浮层正文必须不同");
        assert_ne!(
            DARK_OVERLAY.text_active, LIGHT_OVERLAY.text_active,
            "激活绿必须换（浅底上用深绿，否则看不出来）"
        );
    }
}
