//! m183/m184 量具：把"打开一张带文字的 SVG 要等一两秒"这笔账拆到具体哪一段。
//!
//! 臂（**冷臂必须分进程跑**：字体库与解析缓存都是进程级的）：
//! - `cold`：直接解——第一份文档会连带付掉扫字体库索引那一次；
//! - `warm`：先显式叫 `windui::render::image::svg_warm_fonts()`（单独计时），再按序解；
//! - `thumb-first`：每份先按 256 缩略图档解一次（浏览文件夹时就是它先跑），再解主画面档。
//!
//! ★ 用法是把**同一批文档按同一顺序**在两臂各跑一次：`warm` 或第二份起的文档把索引那一笔
//! 剥掉之后，剩下的才是"每份文档自己要付的钱"。每份都打印像素指纹，换字体库时靠它判断
//! 内容有没有变——**别拿尺寸相同当渲染相同**。
//!
//! ```text
//! cargo run --release --example svg_font_cost_probe -- warm a.svg b.svg c.svg
//! RUPIX_SVG_DB=single cargo run --release --example svg_font_cost_probe -- warm a.svg b.svg c.svg
//! ```

use std::path::Path;
use std::time::Instant;

/// 像素指纹（FNV-1a，按 4 KiB 采样）：A/B 换字体库时判断内容是否改变。
fn fingerprint(buf: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < buf.len() {
        let end = (i + 64).min(buf.len());
        for &b in &buf[i..end] {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        i += 4096;
    }
    h
}

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(arm), files) = (args.first(), &args[1..]) else {
        println!("用法：svg_font_cost_probe cold|warm|thumb-first <svg…>");
        std::process::exit(2);
    };
    if files.is_empty() {
        println!("用法：svg_font_cost_probe cold|warm|thumb-first <svg…>");
        std::process::exit(2);
    }
    let db_env = std::env::var("RUPIX_SVG_DB").unwrap_or_else(|_| "(默认)".into());
    println!(
        "臂={arm} 字体库环境 RUPIX_SVG_DB={db_env} 文档 {} 份",
        files.len()
    );

    if arm == "warm" {
        let t = Instant::now();
        windui::render::image::svg_warm_fonts();
        println!("  svg_warm_fonts()：{} ms", t.elapsed().as_millis());
    }

    for f in files {
        let path = Path::new(f);
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| f.clone());
        if arm == "thumb-first" {
            let t = Instant::now();
            let th = rupix_lib::core::decode::probe_thumb(path, 256, 256);
            match th {
                Ok(d) => println!(
                    "  {:<24} 缩略图 {:>6} ms {}x{} fp={:016x}",
                    name,
                    t.elapsed().as_millis(),
                    d.bitmap.width,
                    d.bitmap.height,
                    fingerprint(&d.bitmap.rgba)
                ),
                Err(e) => println!("  {:<24} 缩略图失败 {e}", name),
            }
        }
        let t = Instant::now();
        match rupix_lib::core::decode::probe_decode(path, 0, 0) {
            Ok(d) => println!(
                "  {:<24} 主画面 {:>6} ms {}x{} fp={:016x}",
                name,
                t.elapsed().as_millis(),
                d.bitmap.width,
                d.bitmap.height,
                fingerprint(&d.bitmap.rgba)
            ),
            Err(e) => println!("  {:<24} 失败 {e}", name),
        }
    }
}
