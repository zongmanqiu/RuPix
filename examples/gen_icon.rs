//! 一次性资源生成器：把 `image/icon.svg` 光栅化成多尺寸 `assets/rupix.ico`。
//!
//! 背景动机：exe 的 PE 图标资源（资源管理器/任务管理器/固定快捷方式显示的那张）
//! 是编译期烙进 `.rsrc` 的静态 ico；窗口/托盘/标题栏走运行时 SVG 光栅化。两条路
//! 必须是同一造型、同一份真源。
//! 判据：ico 每档与 `brand::logo_rgba` 现场光栅逐像素一致——由
//! `brand::tests::pe_icon_asset_matches_freshly_rasterized_logo` 把门，改了
//! icon.svg 忘了重跑本生成器，测试会红。
//! 跑法：`cargo run --example gen_icon`（写 `main/assets/rupix.ico`）
//!
//! 编码：每档用 **PNG 压缩**帧写入 ICO（现代 Windows 资源管理器/任务栏原生支持
//! ICO 内嵌 PNG）。早期版本用未压缩 32bpp BMP，单 256×256 帧就 ~262KB，整 ico
//! 近 300KB 全进 `.rsrc`；改 PNG 后整 ico 降到几十 KB。与 `brand::tests::parse_ico`
//! 互为镜像——解析器能解 PNG 与旧 BMP 两种条目。

use std::fs;
use std::path::Path;

use rupix_lib::ui::brand;

/// ICO 携带的档位：标题栏 16/24、任务栏 32/48、Alt-Tab 64、高 DPI/缩略 256。
const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

fn main() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/rupix.ico");
    let bytes = build_ico();
    fs::write(&out, &bytes).unwrap_or_else(|e| panic!("写 {} 失败：{e}", out.display()));
    println!(
        "wrote {}  ({} bytes, entries: {SIZES:?})",
        out.display(),
        bytes.len()
    );
}

/// ICONDIR + ICONDIRENTRY×6 + 每条 PNG 编码的图像。
fn build_ico() -> Vec<u8> {
    let mut entries: Vec<(u32, Vec<u8>)> = Vec::with_capacity(SIZES.len());
    for &s in SIZES.iter() {
        let rgba = brand::logo_rgba(s).unwrap_or_else(|| panic!("{s}px 光栅化失败"));
        let png = png_entry(s, &rgba);
        entries.push((s, png));
    }

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());

    // 目录项：先把每条图像的对齐后偏移算好写进目录，再按同样对齐落数据，
    // 保证目录里记录的 offset 与文件里 data 的真实位置一致。
    let dir_size = 6 + 16 * entries.len();
    let mut offset = dir_size as u32;
    let mut dir = Vec::with_capacity(16 * entries.len());
    for (s, data) in &entries {
        offset = (offset + 3) & !3; // ICO 图像数据 4 字节对齐
        dir.extend_from_slice(&[*s as u8, *s as u8]); // 宽/高（256 → 0，ICO 规范约定）
        dir.push(0); // 调色板色数：真彩无
        dir.push(0); // 保留
        dir.extend_from_slice(&1u16.to_le_bytes()); // color planes
        dir.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel（PNG 条目按惯例填 32）
        dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
        dir.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    out.extend_from_slice(&dir);

    // 图像数据：按目录里记的（已对齐）偏移落位。
    while out.len() % 4 != 0 {
        out.push(0);
    }
    for (_, data) in &entries {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

/// 一条 PNG 编码的图标图像：把非预乘 RGBA8 直接交给 PNG 编码器
/// （PNG 自带 alpha，不需要 ICO 的 AND 掩码；解码镜像见 `brand::tests::decode_png`）。
fn png_entry(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(rgba.len() + 64);
    {
        let mut enc = png::Encoder::new(&mut buf, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .unwrap_or_else(|e| panic!("{size}px 写 PNG 头失败：{e}"));
        writer
            .write_image_data(rgba)
            .unwrap_or_else(|e| panic!("{size}px 写 PNG 数据失败：{e}"));
    }
    buf
}
