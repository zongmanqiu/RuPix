//! ui 层模块声明。
//!
//! 本文件额外承载「windui 按键事件 → core 脱框架按键」的边界翻译（[`to_key_input`]）：
//! core 按分层门禁严禁依赖 windui，所以按键事件的翻译只能落在此处（ui 层允许引 windui）。

use windui::event::{Key as WKey, KeyEvent as WKeyEvent};

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

pub mod brand;
pub mod canvas;
pub mod chrome;
pub mod filmstrip;
pub mod pin;
pub mod plugin_result;
pub mod plugin_settings;
pub mod raw_dialog;
pub mod recovery_hint;
pub mod settings;
pub mod svg;
pub mod text_window;
pub mod tray;
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
