//! 「复制当前（PNG）」的成本到底是多少：走真实解码，再按 `platform::clipboard`
//! 里同一套 PNG 参数编一遍（不碰剪贴板，免得把探针的图塞进用户机器上的公共剪贴板）。
//!
//! 为什么要量这个：`Action::CopyImage` 在 **UI 线程**上做完整件事——取屏上位图、
//! PNG 编码、进剪贴板。适应窗口时屏上那份只有视口大小（几百 k 像素），放大到 100%
//! 之后它就是整张原图。36.7 MP 的图上这一下到底是 30 ms 还是 3 s，决定要不要改。
//!
//! 跑法（从 main 目录）：
//! ```text
//! RUPIX_DECODERS_DIR=<绝对路径>\.build\decoders cargo run --release --example copy_png_cost -- <图档>
//! ```

use std::path::Path;
use std::time::Instant;

use rupix_lib::core::decode;
use rupix_lib::core::geometry::fit_dims;

const ROUNDS: u32 = 3;

/// 四种参数档位：第 0 档就是 `platform::clipboard::png_block` 现在用的那套（默认），
/// 其余是"编码快多少、字节多花多少"的对照臂。
fn encode_at(rgba: &[u8], w: u32, h: u32, arm: u8) -> usize {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        match arm {
            1 => enc.set_filter(png::Filter::NoFilter),
            2 => enc.set_filter(png::Filter::Up),
            3 => enc.set_compression(png::Compression::Fastest),
            4 => {
                enc.set_filter(png::Filter::NoFilter);
                enc.set_compression(png::Compression::Fastest);
            }
            _ => {}
        }
        if let Ok(mut writer) = enc.write_header() {
            let _ = writer.write_image_data(rgba);
            let _ = writer.finish();
        }
    }
    out.len()
}

const ARMS: [(&str, u8); 5] = [
    ("默认（现用）", 0),
    ("无滤波", 1),
    ("Up 滤波", 2),
    ("只 Fastest 压", 3),
    ("无滤波+Fastest", 4),
];

fn main() {
    rupix_lib::install_decode_stack();
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: copy_png_cost <image>");
        std::process::exit(2);
    };
    let p = Path::new(&arg);

    println!("{arg}");
    for (label, box_w, box_h) in [
        ("视口档 800x600", 800u32, 600u32),
        ("视口档 1920x1080", 1920, 1080),
        ("原图档", 0, 0),
    ] {
        let Ok(d) = decode::probe_decode(p, box_w, box_h) else {
            println!("  {label:<18} 解码失败");
            continue;
        };
        let (w, h) = (d.bitmap.width, d.bitmap.height);
        let mp = (w as f64 * h as f64) / 1e6;
        let rounds = if mp > 10.0 { 1 } else { ROUNDS };
        println!("  {label:<18} {w}x{h}（{mp:.1} MP）");
        for (name, arm) in ARMS {
            let _ = encode_at(&d.bitmap.rgba, w, h, arm);
            let mut best = f64::MAX;
            let mut bytes = 0usize;
            for _ in 0..rounds {
                let t = Instant::now();
                bytes = encode_at(&d.bitmap.rgba, w, h, arm);
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            if bytes == 0 {
                println!("      {name:<16} 编码产出为空，读数不可信");
                continue;
            }
            println!(
                "      {name:<16} 最快 {best:>8.0} ms ｜ PNG {:>7.2} MB",
                bytes as f64 / 1e6
            );
        }
        let (fw, fh) = fit_dims(w, h, 800, 600);
        if (fw, fh) != (w, h) {
            println!("      （同一张图收进 800x600 是 {fw}x{fh}）");
        }
    }
}
