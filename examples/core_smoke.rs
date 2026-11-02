//! 宿主完整链路冒烟：真实图片文件 → `decode::probe_decode`（**全部走解码器**解码）。
//!
//! 与 `decoder_smoke` 的区别：那个在 ABI 层直接 `libloading` 调 DLL；
//! 这个站在用户的位置——走的是与"双击打开一张图"完全相同的那条路：
//! 目录过滤 → 扩展名 → 解码器认领 → 两阶段渲染 → 宿主拿到位图。
//!
//! ★ 判据：`.ppm` 与 `.ff` 是 **需解码器解码**的格式（位图解码全部由解码器 DLL 承担）。
//! 它们能打开，只可能是 `core` 解码器解的 —— 这就证明路由真的倒转到解码器了。
//!
//! 用法（从 main 目录）：
//! ```text
//! cargo run --release --example core_smoke -- ../.temp/samples/*
//! ```

use std::path::Path;
use std::time::Instant;

use rupix_lib::core::decode;
use rupix_lib::decoders;

fn main() {
    rupix_lib::install_decode_stack();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: core_smoke <image> [image...]");
        std::process::exit(2);
    }

    // 解码器自述的扩展名 = 格式清单的主要来源。
    let mut exts = crate::decoders::declared_exts()
        .into_iter()
        .collect::<Vec<_>>();
    exts.sort();
    println!("解码器自述的扩展名（{} 个）：", exts.len());
    for chunk in exts.chunks(12) {
        println!("   {}", chunk.join(" "));
    }
    println!();

    let mut fail = 0usize;
    for a in &args {
        let p = Path::new(a);
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let t0 = Instant::now();
        match decode::probe_decode(p, 0, 0) {
            Ok(d) => {
                println!(
                    "OK   {:<10} {}x{}  {:>6.1} ms",
                    name,
                    d.bitmap.width,
                    d.bitmap.height,
                    t0.elapsed().as_secs_f64() * 1000.0
                );
                // 位图必须真的有像素，不能是"成功但空"。
                if d.bitmap.width == 0 || d.bitmap.height == 0 {
                    println!("     FAIL: 尺寸为 0");
                    fail += 1;
                } else if d.bitmap.rgba.len()
                    != d.bitmap.width as usize * d.bitmap.height as usize * 4
                {
                    println!("     FAIL: 像素缓冲长度与尺寸不符");
                    fail += 1;
                }
            }
            Err(e) => {
                println!("FAIL {:<10} {}", name, e);
                fail += 1;
            }
        }
    }

    println!();
    if fail == 0 {
        println!("全部 {} 个文件解码成功", args.len());
    } else {
        println!("{} / {} 个失败", fail, args.len());
        std::process::exit(1);
    }
}
