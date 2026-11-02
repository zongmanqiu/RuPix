//! 📌 置顶图钉的 ui 侧封装（仅 windui，不碰 Win32）。
//!
//! 离屏 GDI 渲染的真实实现在 [`crate::platform::pin_glyph`]（平台层，允许用 `windows`）。
//! 本模块只做两件事：
//! - 把 [`windui::geometry::Color`] 拆成裸 RGB 喂给平台层；
//! - 把平台层返回的 `SIZE×SIZE` RGBA 字节包成 [`windui::render::image::Image`]，
//!   供 `Chrome::pins` 缓存两张、终身复用（见 `ui::chrome`）。
//!
//! 之所以把 GDI 引擎挪到 platform：ui 层按五层门禁**禁止**引 `windows`，
//! 平台能力一律经 platform 层取用——`render_pins` 是这条边界的 ui 侧入口。

use crate::platform::pin_glyph::{render_pin_rgba as render_pin_gdi, SIZE};
use windui::geometry::Color;
use windui::render::image::Image;

/// 渲染一个姿态的图钉为 RGBA 字节（委托平台层 GDI 引擎；颜色用 windui `Color`）。
pub fn render_pin_rgba(upright: bool, color: Color) -> Option<Vec<u8>> {
    render_pin_gdi(upright, color.r, color.g, color.b)
}

/// 一次渲染「斜躺 `tilted` / 立正 `upright`」两张，供 `Chrome::pins` 缓存。
pub fn render_pins(tilted: Color, upright: Color) -> Option<(Image, Image)> {
    let a = Image::from_rgba(SIZE, SIZE, &render_pin_rgba(false, tilted)?).ok()?;
    let b = Image::from_rgba(SIZE, SIZE, &render_pin_rgba(true, upright)?).ok()?;
    Some((a, b))
}
