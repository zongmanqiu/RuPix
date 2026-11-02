//! 量文本宽：给浮条里那一格状态定宽用。
//!
//! 存在理由：状态并进浮条之后，**格子必须是定宽**——否则暂停动图时多出来的
//! "· 帧 57/100" 会把整排按钮推开几个像素，那正是本轮要消掉的"跳"。定多宽不能拍：
//! 要装得下**永不许截断**的那一条（帧号形态，AGENTS §八.7），又要认得最坏输入
//! （四位数张数 + 四位数帧号）。跑法：`cargo run --release --example text_measure`。

use tiny_skia::Pixmap;
use windui::render::{Canvas, SkiaCanvas};
use windui::text::{DWriteEngine, TextEngine, TextStyle};

/// 候选串：从"日常"到"病态"。★ **判据取 4 位数那两条**（帧号 207 / 载入中 209）——
/// 5 位数那两条是"一个目录上万张、一张图上万帧"的病态输入，故意不为它们多付 25 px
/// 浮条宽，那种串在浮条里会被收成省略号（`chrome::INFO_TEXT_W` 的注释记着这条取舍）。
const CASES: &[&str] = &[
    "8/184 · 117%",
    "8/184 · 117% · 帧 47/100",
    "1234/1234 · 100% · 帧 1234/1234",
    "1234/1234 · 4000×3000 · 载入中…",
    "12345/12345 · 12345×3216 · 载入中…",
    "12345/12345 · 100% · 帧 12345/12345",
    "1234/1234 · 载入中…",
    "没有打开图片",
];

fn main() {
    let mut pm = Pixmap::new(8, 8).unwrap();
    let mut eng = DWriteEngine::new();
    eng.set_scale(1.0);
    let mut cx = SkiaCanvas::with_text(&mut pm, &mut eng, 1.0);
    let ts = TextStyle::new(13.0);
    println!("字号 13（逻辑 px）：");
    let mut max = 0i32;
    for s in CASES {
        let w = cx.measure_text(s, &ts).w;
        max = max.max(w);
        println!(
            "  文本 {w:>4} px ｜ +16 内边距 = 格子 {:>4} px   {s}",
            w + 16
        );
    }
    println!("最坏文本宽 {max} px");
    // 帮助判读：格子能不能挤进浮条（9 颗=346 / 12 颗=457），最小窗 360。
    for pad in [8.0f32, 12.0, 16.0] {
        let cell = max as f32 + pad;
        println!(
            "内边距 {pad} → 格子 {cell:.0} px ｜ 9 颗浮条总宽 {:.0}（最小窗 360）｜ 12 颗 {:.0}",
            cell + 346.0,
            cell + 457.0
        );
    }
}
