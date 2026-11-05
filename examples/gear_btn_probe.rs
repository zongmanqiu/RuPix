//! 量行尾两颗小按钮的盒子尺寸：「!」详情 vs 设置齿轮。
//!
//! 跑法：`cargo run --release --example gear_btn_probe`
//!
//! 为什么需要它：windui 的按钮盒子 = **字形度量 + Medium 内边距**（左右 32 / 上下 18，
//! 框架侧写死）。「!」是正文字体里的一个窄字符，齿轮是 Segoe MDL2 的图标字形
//! （PUA 码位 U+E713）——同一个 14 px 字号，两种字体的字形盒差得很远，于是齿轮那颗
//! 天然长成 46×32、「!」是 37×36：宽出 9 px、矮了 4 px，并排看着不是一套
//! （2026-11-05 用户报障「齿轮按钮宽度和高度都不对」）。
//!
//! 修法是**把盒子钉死**成「!」那颗的尺寸（`ui::tokens::ICON_BTN_W/H`），
//! 本探针就是复核那两个数字的量具：改了 `FONT_MD` 或界面字体就重跑一次。
//! 同一组数字还有一条常驻回归测试盯着：
//! `ui::settings::extensions_tab::tests::info_row_icon_buttons_share_one_box`。

use tiny_skia::Pixmap;
use windui::render::{Canvas, SkiaCanvas};
use windui::text::{DWriteEngine, TextEngine, TextStyle};

/// windui Medium 尺寸按钮的内边距（左右 / 上下），框架侧写死，这里照抄一份。
const PAD_W: i32 = 32;
const PAD_H: i32 = 18;

fn main() {
    let mut pm = Pixmap::new(8, 8).expect("画布");
    let mut eng = DWriteEngine::new();
    eng.set_scale(1.0);
    let mut cx = SkiaCanvas::with_text(&mut pm, &mut eng, 1.0);

    for size in [14.0f32, 13.5, 13.0] {
        let bang = cx.measure_text("!", &TextStyle::new(size));
        let gear = {
            let ts = TextStyle {
                family: Some("Segoe MDL2 Assets"),
                ..TextStyle::new(size)
            };
            cx.measure_text("\u{E713}", &ts)
        };
        println!(
            "字号 {size}: 「!」 {}×{} → 按钮 {}×{}  ｜ 齿轮 {}×{} → 按钮 {}×{}",
            bang.w,
            bang.h,
            bang.w + PAD_W,
            bang.h + PAD_H,
            gear.w,
            gear.h,
            gear.w + PAD_W,
            gear.h + PAD_H
        );
    }
    println!("（`tokens::ICON_BTN_W/H` 取的是字号 14 那行的「!」按钮盒）");
}
