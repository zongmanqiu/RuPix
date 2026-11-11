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
    /// 左侧文件树面板底（m302）。
    ///
    /// ★ 它是**第三块地**，既不能等于 [`Overlay::empty_bg`]（看图区），也不能等于
    ///   [`Overlay::panel_bg`]（缩略图条）：相等的那两块在屏上会连成一片，圆角处
    ///   漏出的底色又跟面板同色，面板等于没画（2026-11-10 用户截图实测）。
    ///   两套主题都取「离看图区最远」的一档：深色最亮、浅色最暗。
    tree_bg: [u8; 3],
    /// 文件树里**当前层**那一行的底（m309）。
    ///
    /// ★ 不复用 [`Overlay::slot_cur`]：那是给缩略图格用的半透明叠加（6% 黑），
    ///   压在**实心**的 `tree_bg` 上只差 14 级灰，真机上等于没标（用户原话
    ///   「标出当前层倒是有必要」——那时候其实已经有标记，只是看不出来）。
    ///   这里取不透明值：深浅两套都精确可控，不受底层色影响。
    tree_cur: [u8; 3],
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
    /// 面板朝内容区那侧的 **1dp 分隔线**（m311，见 `ui::fill_panel_open` 的 `edge`）。
    ///
    /// ★ 浅色主题**必须**有线：看图区已经 L\* 99，再亮就到 100，三块地最多挤出
    ///   ΔE≈4 的差；不补这条线就只能靠「把树压暗」来分开——那正是用户说的「太重」。
    /// ★ 深色主题是 α=0（**不画**）：那两档 ΔE 已经 11，再加一道线纯属多余。
    ///   两套给不同的 α 不是偷懒，是这两套主题的可用明度空间根本不一样。
    panel_edge: [u8; 4],
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
    // ★ m310：三档按 **CIELAB 的 L\*** 等距（每步 ΔL*≈11），不是按 sRGB 数值等距。
    //   暗部要**更大**的数值差才有同样的感知落差（用户 2026-11-10 口径），
    //   所以这儿的 sRGB 跨度 48 明显大于浅色那套的 24——两套用同一组步长的话，
    //   深色在观感上永远比浅色弱一档。看图区永远在**最暗**那端。
    //   L*: 4.0 / 15.3 / 26.4。
    empty_bg: [14, 14, 16],
    tree_bg: [62, 62, 68],
    tree_cur: [106, 106, 114],
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
    panel_bg: [38, 38, 42, 232],
    // α=0：深色两档之间差了 11 个 ΔE，分得开靠的是填充色本身，不需要线。
    panel_edge: [0, 0, 0, 0],
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
/// - 三块地走**等距阶梯**（m309：看图区 251 → 缩略图条 239 → 文件树 227，每级 12）：
///   看图区取近白微灰而不是纯白——纯白压在浅色照片上边界会糊掉；阶梯把浮层一级一级
///   "托"起来，又不至于像一块灰板。
/// - 所有"亮一档表示悬停/按下"的叠加一律改成**压暗**（`0,0,0,a`）：白底上越叠越亮
///   会直接看不见，这是把深色做法反过来之后最容易踩的一脚。
/// - 棋盘格浅格是白、深格是 `221,223,228`（差 34 级），与深色主题的 86 级同一量级，
///   透明区在浅底上照样一眼可辨。
/// - 红系整体加深：`224,86,78` 这类亮红压在浅底上对比只有 2.6:1，读不清。
const LIGHT_OVERLAY: Overlay = Overlay {
    // ★ m311：三块地整体**抬亮**（看区 249→252、条 232→239、树 215→228）。
    //   m310 只按 L* 等距排，数被压到灰那一端：树 #D7D9DD 压在近白的画面上就是一整块
    //   灰，而它与条只差 5.7 L*，五五开地糊成一片（用户 2026-11-10：「太重，而且难以
    //   区分」）。现在每步 ΔE≈4.2，树再带一点冷调（b* −3.7）——但这点差**不够**，
    //   剩下的分离交给 `panel_edge` 那条 1dp 线（浅色端没有明度余量，见该字段注释）。
    //   L*: 99.0 / 94.8 / 91.6，sRGB 跨度 24（深色那套是 48，见其注释）。
    empty_bg: [252, 252, 253],
    tree_bg: [228, 231, 238],
    tree_cur: [189, 194, 204],
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
    panel_bg: [239, 240, 242, 240],
    // 8% 黑：压在条上比底色深 6.7 个 L*，一条锐利的细缝（VS Code 的边框就是这个量级）。
    panel_edge: [0, 0, 0, 20],
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
/// 左侧文件树面板底（第三块地，别与 `empty_bg` / `panel_bg` 混用）。
pub fn tree_bg() -> Color {
    to_color(overlay().tree_bg)
}
/// 文件树里当前层那一行的底（m309）。不透明，别拿 `slot_cur` 顶替。
pub fn tree_cur() -> Color {
    to_color(overlay().tree_cur)
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
/// 面板朝内容区那侧的 1dp 分隔线（见 `ui::fill_panel_open`）。
pub fn panel_edge() -> Color {
    to_color4(overlay().panel_edge)
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

    /// ★ m302：树、条、看图区是三块地，必须两两可分。截图实测过一次「树 = 看图区」
    ///   （浅色主题下都是 #F6F6F8）的后果：整块面板融进背景，圆角漏出来的也是同色，
    ///   看上去就是没画面板。
    #[test]
    fn tree_strip_and_view_are_three_distinct_surfaces() {
        // 两套表一个是 RGB、一个是 RGBA：都按切片前三个通道比，别为长度写两份。
        let dist = |a: &[u8], b: &[u8]| {
            a[0].abs_diff(b[0]) as u32 + a[1].abs_diff(b[1]) as u32 + a[2].abs_diff(b[2]) as u32
        };
        for (name, view, panel, tree) in [
            (
                "dark",
                DARK_OVERLAY.empty_bg,
                DARK_OVERLAY.panel_bg,
                DARK_OVERLAY.tree_bg,
            ),
            (
                "light",
                LIGHT_OVERLAY.empty_bg,
                LIGHT_OVERLAY.panel_bg,
                LIGHT_OVERLAY.tree_bg,
            ),
        ] {
            assert!(dist(&view, &tree) >= 18, "{name}: 树与看图区糊成一片");
            assert!(dist(&tree, &panel) >= 18, "{name}: 树与缩略图条糊成一片");
            // 负向臂：树不许落在「看图区与条之间」——那样必定有一套主题下它贴近
            // 半透明条压出来的观感色，条贴左时又分不出两块。
            assert!(
                dist(&view, &tree) > dist(&view, &panel),
                "{name}: 树该比条离看图区更远"
            );
        }
    }

    /// sRGB 三通道 → CIELAB 的 L*（D65，只取明度轴）。
    ///
    /// ★ 判据为什么必须用它，而不是 `R+G+B`：sRGB 的数值差**不是**感知差。用户
    ///   2026-11-10 的原话是「浅色部分的一点点数值就会有很大区别，深色部分却需要
    ///   比较大的数值」。m309 拿算术和当尺子，把两套都判成"等距"（12/12 与 14/14），
    ///   看着仍然是"没层次"——因为同一套 sRGB 步长，在暗部换来的观感落差和亮部不等。
    /// sRGB → CIELAB（D65）。
    ///
    /// ★ 只比 L\* 不够：m311 起树底带一点冷调（b\* ≈ −3.7），明度差相同时色相也在
    ///   帮忙分开。判「看起来差多少」必须走 ΔE，否则那点冷调被算成零。
    fn lab(rgb: &[u8]) -> (f32, f32, f32) {
        let lin = |v: u8| {
            let c = v as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
        let x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b;
        let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
        let z = 0.0193339 * r + 0.119_192 * g + 0.9503041 * b;
        let f = |t: f32| {
            if t > 0.008856 {
                t.powf(1.0 / 3.0)
            } else {
                7.787 * t + 16.0 / 116.0
            }
        };
        let (fx, fy, fz) = (f(x / 0.95047), f(y), f(z / 1.08883));
        (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
    }

    /// 两色的 CIE76 色差（ΔE\*ab）。经验刻度：≈2.3 是「刚好能看出」的门槛。
    fn delta_e(a: &[u8], b: &[u8]) -> f32 {
        let (p, q) = (lab(a), lab(b));
        ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt()
    }

    /// 把一枚（可能带 α 的）ink 压到 bg 上，得到「看见的那个颜色」。
    fn blend(ink: &[u8], bg: &[u8]) -> [u8; 3] {
        let k = ink[3] as f32 / 255.0;
        [0, 1, 2].map(|i| (ink[i] as f32 * k + bg[i] as f32 * (1.0 - k)).round() as u8)
    }

    fn lstar(rgb: &[u8]) -> f32 {
        lab(rgb).0
    }

    /// `lstar` 自己的标定：黑 0、白 100，中点大致在 sRGB 119。
    /// 这条先站稳，后面两条判据才有意义。
    #[test]
    fn lstar_hits_its_two_anchors() {
        assert!(lstar(&[0, 0, 0]).abs() < 0.5, "纯黑该是 L*≈0");
        assert!(
            (lstar(&[255, 255, 255]) - 100.0).abs() < 0.5,
            "纯白该是 L*≈100"
        );
        // 负向臂：算术和会把 119 和 255 判成差 136 级，L* 上它们差 50。
        assert!((lstar(&[119, 119, 119]) - 50.0).abs() < 1.0);
    }

    /// ★ m311：三块地要**亮**、还得**分得开**——浅色端这两件事会打架。
    ///
    /// 四件事一起判：
    ///   ① 顺序一致——看图区永远在最极端那端（它是内容区，最不该被背景抢）；
    ///   ② 浅色不许沉——树底 L\* ≥90。这条是用户「太重」那句的机器版：m310 把树
    ///      压到 86.6（#D7D9DD），压在近白的画面上就是一整块灰；
    ///   ③ 每步 ΔE ≥3.5，且**分不够就必须有线**——浅色看图区已经 L\* 99，再往上没
    ///      空间，三块地最多挤出 ΔE≈4；这时光靠填充色必然「难以区分」，得由 1dp 线
    ///      补刀。深色反过来：ΔE 已经 11，画线纯属多一道；
    ///   ④ **深色的 sRGB 跨度必须大于浅色**——「暗部需要更大的数值差」的机器判据，
    ///      挡住「两套共用一组步长」这种偷懒写法。
    #[test]
    fn three_surfaces_stay_light_and_apart() {
        let l = [
            &LIGHT_OVERLAY.empty_bg[..],
            &LIGHT_OVERLAY.panel_bg[..3],
            &LIGHT_OVERLAY.tree_bg[..],
        ];
        let d = [
            &DARK_OVERLAY.empty_bg[..],
            &DARK_OVERLAY.panel_bg[..3],
            &DARK_OVERLAY.tree_bg[..],
        ];

        // ① 先后位置。
        assert!(
            lstar(l[0]) > lstar(l[1]) && lstar(l[1]) > lstar(l[2]),
            "light: 三块地的先后位置不对（看图区必须最亮）"
        );
        assert!(
            lstar(d[0]) < lstar(d[1]) && lstar(d[1]) < lstar(d[2]),
            "dark: 三块地的先后位置不对（看图区必须最暗）"
        );

        // ② 浅色不许沉。
        assert!(
            lstar(l[2]) >= 90.0,
            "light: 树底 L* 只有 {:.1}（该 ≥90）——压在近白的画面上就是一整块灰",
            lstar(l[2])
        );

        // ③ 每步跟得上；分不够时，空缺必须由那条线补上。
        let (l1, l2) = (delta_e(l[0], l[1]), delta_e(l[1], l[2]));
        assert!(
            l1 >= 3.5 && l2 >= 3.5,
            "light: 有一步 ΔE 只 {l1:.1} / {l2:.1}（每步至少 3.5）"
        );
        if l1.min(l2) < 5.5 {
            let a = LIGHT_OVERLAY.panel_edge[3];
            let on = blend(&LIGHT_OVERLAY.panel_edge, l[1]);
            assert!(
                a >= 12 && delta_e(&on, l[1]) >= 3.0,
                "light: 每步 ΔE 只有 {:.1}，分隔线却看不见（α={a}，压在条上差 {:.1} ΔE）——两块地会糊在一起",
                l1.min(l2),
                delta_e(&on, l[1])
            );
        }
        assert_eq!(
            DARK_OVERLAY.panel_edge[3],
            0,
            "dark: 两档 ΔE 已经 {:.1}，不该再画线（多一道反而脏）",
            delta_e(d[0], d[1])
        );

        // ④ 暗部靠更大的数值差。
        let (ls, ds) = (
            l[0][0] as i32 - l[2][0] as i32,
            d[2][0] as i32 - d[0][0] as i32,
        );
        assert!(
            ds > ls,
            "深色的 sRGB 跨度 {ds} 不大于浅色的 {ls}——暗部要靠更大的数值差才看得出层次"
        );
    }

    /// 当前层那一行必须**看得出来**。m309 之前它用 `slot_cur`（6% 黑）压在实心的
    /// 树底上，只差 14 级灰——标记在，效果等于没有。
    #[test]
    fn tree_current_row_stands_out_from_the_panel() {
        // hover 是半透明的：先合成到树底上，再一起折算成 L*，否则比的是两个世界的东西。
        let hover_over = |hover: &[u8], bg: &[u8]| -> [u8; 3] {
            let k = hover[3] as f32 / 255.0;
            let f = |i: usize| (hover[i] as f32 * k + bg[i] as f32 * (1.0 - k)).round() as u8;
            [f(0), f(1), f(2)]
        };
        for (name, bg, cur, hover) in [
            (
                "dark",
                DARK_OVERLAY.tree_bg,
                DARK_OVERLAY.tree_cur,
                DARK_OVERLAY.slot_hover,
            ),
            (
                "light",
                LIGHT_OVERLAY.tree_bg,
                LIGHT_OVERLAY.tree_cur,
                LIGHT_OVERLAY.slot_hover,
            ),
        ] {
            assert_ne!(bg, cur, "{name}: 当前层与树底同值");
            let lb = lstar(&bg);
            let cur_off = (lstar(&cur) - lb).abs();
            let hov_off = (lstar(&hover_over(&hover, &bg)) - lb).abs();
            assert!(
                cur_off >= 8.0,
                "{name}: 当前层与树底的 L* 只差 {cur_off:.1}（至少 8）——标记等于没标"
            );
            // ★ 当前层要比 hover **更重**：鼠标划过是瞬时提示，"你在这儿"是持久状态。
            //   反过来就成了「划一下比停在那儿还显眼」——m309 第一版正是这个毛病。
            assert!(
                cur_off >= hov_off + 3.0,
                "{name}: 当前层 L* 偏移 {cur_off:.1} 没比 hover 的 {hov_off:.1} 高出 3"
            );
        }
    }
}
