//! ui 层模块声明。
//!
//! 本文件额外承载「windui 按键事件 → core 脱框架按键」的边界翻译（[`to_key_input`]）：
//! core 按分层门禁严禁依赖 windui，所以按键事件的翻译只能落在此处（ui 层允许引 windui）。

use windui::event::{Key as WKey, KeyEvent as WKeyEvent};
use windui::geometry::Rect;
use windui::render::{Canvas, Paint};

use crate::core::hotkey::{KeyInput, VKey};

/// 把 windui 的按键事件翻译成 [`crate::core::hotkey::KeyInput`]。
///
/// 纯修饰键（Alt）/ 控制键（Tab）/ 未知键不构成可绑定主键，翻译为 `None`；
/// Esc 现为可绑定主键（映射进 `VKey::Escape`），真正的主键一律映射进 [`VKey`]。
pub fn to_key_input(ev: &WKeyEvent) -> Option<KeyInput> {
    let key = match ev.key {
        WKey::Left => VKey::Left,
        WKey::Right => VKey::Right,
        WKey::Up => VKey::Up,
        WKey::Down => VKey::Down,
        WKey::Home => VKey::Home,
        WKey::End => VKey::End,
        WKey::PageUp => VKey::PageUp,
        WKey::PageDown => VKey::PageDown,
        WKey::Space => VKey::Space,
        WKey::Enter => VKey::Enter,
        WKey::Backspace => VKey::Backspace,
        WKey::Delete => VKey::Delete,
        WKey::Insert => VKey::Insert,
        WKey::ContextMenu => VKey::ContextMenu,
        WKey::Escape => VKey::Escape,
        WKey::Char(c) => VKey::Char(c),
        WKey::F(n) => VKey::F(n),
        WKey::NumpadAdd => VKey::NumpadAdd,
        WKey::NumpadSubtract => VKey::NumpadSubtract,
        WKey::NumpadMultiply => VKey::NumpadMultiply,
        WKey::NumpadDivide => VKey::NumpadDivide,
        WKey::Alt | WKey::Tab | WKey::Other(_) => return None,
    };
    Some(KeyInput {
        key,
        ctrl: ev.ctrl,
        alt: ev.alt,
        shift: ev.shift,
        win: ev.meta,
    })
}

// ★ [`tokens`] 是本层唯一的配色 / 字号 / 间距来源：**哪个模块都不许自己再开一份色值**
//   （hygiene 第 10 查守着）。它不走下面的「层内中枢」——令牌是**一整族名字**
//   （几十个常量 + 两个转换函数），再发布一遍只是抄名单，`super::tokens::…` 已经够短。
pub(crate) mod tokens;

pub mod association;
pub mod brand;
pub mod canvas;
pub mod chrome;
pub mod companion;
pub mod dialog_footer;
pub mod filmstrip;
pub mod goto_dialog;
/// 抓手光标（松手 / 抓手）的 SVG 光栅化与切换。
pub mod hand_cursor;
pub mod pin;
pub mod plugin_result;
pub mod plugin_settings;
pub mod raw_dialog;
pub mod recovery_hint;
pub mod rotate_dialog;
pub mod settings;
pub mod singleton;
pub mod svg;
pub mod text_window;
pub mod tray;
pub mod tree;
pub mod update_state;
pub mod update_window;
pub mod window;

// ── 层内中枢（2026-10-26，对标 CurSeen 的同款做法）────────────────────
//
// 平铺模块之间**不直接互相 `use`**：一律经 `super::` 从本文件取。理由不是洁癖，是三件
// 具体的事：① 依赖图是星形而不是网——"谁用了谁"读这一个文件就够；② 某个模块改名或搬家，
// 改的是这里的一行，不是散在五处的全路径；③ 反向也好用——想让某模块别再用另一个模块，
// 删掉这里的一行 `use` 就会**编译报错**，而不是靠 grep 全层去找。
//
// 分两档，别混：
// - **类型**（被两个以上模块用）→ 在这里 `pub(crate) use` 出来，消费方写 `use super::{Chrome, Hub}`。
// - **函数**（含只被一个模块用的类型）→ 一律写 `super::<模块>::名字`：函数的**归属模块本身带语义**
//   （`raw_dialog::open_path` 一眼知道是哪个对话框的），摆进中枢读不出属于谁，而且改一次要动两处。
//
// 门禁：hygiene 第 11 查 `ui_super_path`——`src/ui/**` 的非注释行出现 `crate::ui::` 即违规。
// 文档注释里的 [`crate::ui::tokens`] 这类绝对链接不受限（注释行本来就不扫）。
pub(crate) use canvas::{ImageCache, ImageCanvas};
pub(crate) use chrome::Chrome;
pub(crate) use filmstrip::Strip;
pub(crate) use settings::Hub;

// ── 常驻面板的「单侧圆角」（m301）─────────────────────────────────────────
//
// 文件树与缩略图条都贴着内容区的边。★ 圆角只该出现在**贴窗口外轮廓**的角上：凡是
// 与另一块面板、或者与看图区相接的边，一律直角。两个圆角相接处会漏出底下那层底色，
// 视觉上「空出一块」（m301/m302 用户实机反馈）。
//
// 做法：把圆角矩形朝每条「相接边」的方向外扩一个半径，再裁回面板矩形。那条边上的
// 圆角整个落在裁剪区外 ⇒ 边界是直的，其余角照旧是圆的。★ 全程只提交一次填充：半透明
// 背板不会因为「先画圆的、再补一条矩形」在接缝上叠出深色带（两次 alpha 合成会变色）。

/// 面板哪几条边**不是**贴窗口外缘 ⇒ 那几条边的圆角要抹平。
///
/// 例：贴底条左边紧挨文件树、上边紧挨看图区 ⇒ `FlatEdges::new(true, false, true)`，
/// 四个角里只剩右下角（真正贴窗口外角）还留着圆。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FlatEdges {
    pub top: bool,
    pub right: bool,
    pub left: bool,
}

impl FlatEdges {
    pub(crate) const fn new(top: bool, right: bool, left: bool) -> Self {
        Self { top, right, left }
    }
}

/// 外扩后的圆角矩形 `(x, y, w, h)`。纯几何，绘制与判据共用这一份推导。
fn open_rect(r: Rect, radius: f32, flat: FlatEdges) -> (f32, f32, f32, f32) {
    let (mut x, mut y, mut w, mut h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
    if flat.left {
        x -= radius;
        w += radius;
    }
    if flat.right {
        w += radius;
    }
    if flat.top {
        y -= radius;
        h += radius;
    }
    (x, y, w, h)
}

/// 画一块「贴窗口外角圆、相接边直」的面板。
///
/// `rect` 是**视口绝对坐标**（与 `filmstrip::panel` / `tree::panel` 的命中同一套），
/// 裁剪也用它——不许在绘制点再换算一次坐标系。
pub(crate) fn fill_panel_open(
    canvas: &mut dyn Canvas,
    rect: Rect,
    radius: f32,
    flat: FlatEdges,
    paint: &Paint,
    edge: &Paint,
) {
    let (x, y, w, h) = open_rect(rect, radius, flat);
    canvas.save();
    canvas.clip_rect(rect);
    canvas.fill_round_rect(x, y, w, h, radius, paint);
    canvas.restore();
    // 线画在**面板内缘**：越出去就压到看图区、或者压到隔壁那块地上了。
    for line in edge_lines(rect, flat).into_iter().flatten() {
        canvas.fill_rect(line.0, line.1, line.2, line.3, edge);
    }
}

/// 相接边那条 1dp 线要占的矩形（`[上, 右]`；`None` = 这条边不画）。
///
/// ★ 只认 `top` / `right` —— **朝内容区的那两个方向**。两块相邻的面板各画一条
///   就成了 2dp 双线（树的右边与贴底条的左边是同一条缝），所以另一半一律不画：
///   `flat.left` 仍然表示「这条边该直角」，但不表示「要画线」。
fn edge_lines(rect: Rect, flat: FlatEdges) -> [Option<(f32, f32, f32, f32)>; 2] {
    let (x, y, w, h) = (rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32);
    [
        flat.top.then_some((x, y, w, 1.0)),
        flat.right.then_some((x + w - 1.0, y, 1.0, h)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个方向的外扩只动自己那根轴，多条相接边可以叠加；没点名的边不许顺手动。
    #[test]
    fn open_rect_grows_only_on_the_joined_edges() {
        let r = Rect::new(20, 30, 184, 600);
        // 树 / 贴左条：只有右缘相接。
        let right = open_rect(r, 10.0, FlatEdges::new(false, true, false));
        assert_eq!(right, (20.0, 30.0, 194.0, 600.0));
        assert_eq!((right.0, right.1, right.3), (20.0, 30.0, 600.0));

        // 贴底条（左边有树）：上缘 + 左缘相接，只剩右下角是圆。
        let both = open_rect(r, 10.0, FlatEdges::new(true, false, true));
        assert_eq!(both, (10.0, 20.0, 194.0, 610.0));

        // 负向臂：没有相接边时就该是原矩形，贴窗口外角的圆角一个都不能少。
        let none = open_rect(r, 10.0, FlatEdges::default());
        assert_eq!(none, (20.0, 30.0, 184.0, 600.0));
    }

    /// m311：分隔线只画在**朝内容区**的那两条边（上、右），而且必须落在面板**内缘**。
    ///
    /// 两条相邻的面板各画一条 = 2dp 双线（树的右边与贴底条的左边是同一条缝），
    /// 所以 `left` 虽然有 flat 标记，也只表示"这条边该直角"，不表示"要画线"。
    #[test]
    fn panel_edge_goes_inside_the_panel_and_only_faces_the_view() {
        let r = Rect::new(20, 30, 184, 600);

        // 树：只有右缘相接 ⇒ 只有一条竖线，压在 x = 20+184-1 = 203（内缘最后一列）。
        let tree = edge_lines(r, FlatEdges::new(false, true, false));
        assert_eq!(tree[0], None, "树的上缘不接内容区，不该有线");
        assert_eq!(tree[1], Some((203.0, 30.0, 1.0, 600.0)));

        // 贴底条（左边有树）：上缘有线、左缘**没有**（那一半由树的右缘负责）。
        let strip = edge_lines(r, FlatEdges::new(true, false, true));
        assert_eq!(strip[0], Some((20.0, 30.0, 184.0, 1.0)));
        assert_eq!(strip[1], None, "左边画了就与树的右缘叠成 2dp 双线");

        // 负向臂：没有相接边时一条线都不许有（面板贴窗口外角时是圆的，线会露在圆角外）。
        let none = edge_lines(r, FlatEdges::default());
        assert_eq!((none[0], none[1]), (None, None));

        // 负向臂：线不许越出面板——越出去就压到看图区或隔壁那块地上了。
        for line in edge_lines(r, FlatEdges::new(true, true, true))
            .into_iter()
            .flatten()
        {
            assert!(line.0 >= 20.0 && line.0 + line.2 <= 204.0);
            assert!(line.1 >= 30.0 && line.1 + line.3 <= 630.0);
        }
    }
}
