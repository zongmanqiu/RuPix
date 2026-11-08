//! RuPix 自有图标：**渲染 `main/assets/icon.svg`**（windui 内置 resvg 光栅化）。
//!
//! # 为什么必须自己有一份
//!
//! 之前 `.icon(windui::icon::brand_icon())` 直接借用了**框架自带**的图标
//! （天蓝底板 + 白色 W 字形）——窗口、任务栏、Alt-Tab 上挂的都是 windui 的牌子。
//! 那是个便利的默认，不是我们的标识；托盘一旦用上它，同一张"别人的图"会出现在
//! 三个地方。
//!
//! # 为什么是 SVG 源而不是 PNG / 程序化
//!
//! 单 exe 便携：686 字节的矢量源 `include_bytes!` 进 exe，没有外部文件
//! （"删目录即干净卸载"不破功）。矢量 + [`Image::from_svg_bytes`] 意味着按系统
//! 要的尺寸**现场光栅化**——150% 缩放要 48、200% 要 64，任意尺寸都是矢量清晰度，
//! 不存在"一张 PNG 缩到 16px 发糊"的问题。
//!
//! 造型：`assets/icon.svg`（米色圆角底 + 橙红 #D14527 与深灰 #272B29 双块叠压），
//! 程序化绘制的第一版已退役。
//!
//! 兜底：SVG 光栅化意外失败时给一块纯色底板（宁可丑，也不要开天窗）。

use windui::icon::{IconSource, WindowIcon};
use windui::render::image::Image;

/// 品牌 logo（矢量源，随 exe 分发）。改 logo = 覆盖这个文件重新编译。
/// ★ 它与 `assets/rupix.ico` 是**同一份 logo 的两端**：PE 图标资源由
///   `examples/gen_icon.rs` 从本文件光栅化而来（m274 把两者放回同一目录）。
///   改了这里没重跑生成器，`pe_icon_asset_matches_freshly_rasterized_logo` 会红。
const LOGO_SVG: &[u8] = include_bytes!("../../assets/icon.svg");

/// 兜底底板色（与主窗深色调一致；只在 SVG 光栅化意外失败时出现）。
const FALLBACK: [u8; 3] = [24, 26, 30];

/// 给 `App::icon` 的图标源：按系统要的尺寸现渲（尺寸由平台层决定），
/// 不缓存单张位图——理由见 windui `icon.rs` 模块头。
pub fn app_icon() -> IconSource {
    IconSource::sized(|size| draw(size).unwrap_or_else(|| flat(size)))
}

/// 托盘图标的像素（非预乘 RGBA8）。固定 32×32：托盘推荐尺寸，
/// 系统再缩到 16 显示时本造型仍可辨识。
pub fn tray_icon() -> (u32, u32, Vec<u8>) {
    let s = 32u32;
    (s, s, raster(s).unwrap_or_else(|| flat_bytes(s)))
}

/// 现场光栅化 logo 成 s×s 非预乘 RGBA（失败返回 None）。
///
/// 公开给 `examples/gen_icon.rs`：生成 PE 图标资源用的 `assets/rupix.ico`。
/// 生成器与运行时窗口/托盘图标必须是同一份真源，
/// 同步性由测试 `pe_icon_asset_matches_freshly_rasterized_logo` 把门。
pub fn logo_rgba(size: u32) -> Option<Vec<u8>> {
    raster(size)
}

/// 一定成功的兜底：一块纯色底板（尺寸算错/溢出时宁可丑，也不要开天窗）。
fn flat(size: u32) -> WindowIcon {
    let s = size.max(1);
    // `flat` 的签名就是"一定成功的兜底"：`flat_bytes` 恒产出 s×s×4 字节，而 `from_rgba`
    // 唯一的失败条件是长度不符 —— 走到这个分支只能是 windui 的校验契约变了。那种时候必须
    // 在开发期炸出来，而不是画一块透明图标蒙过去（`WindowIcon` 没有可降级的空值构造：
    // 两个构造函数都返回 Option）。红线豁免: §八.1 第三类例外，就地标注由 hygiene 检查点名。
    WindowIcon::from_rgba(s, s, flat_bytes(s))
        .unwrap_or_else(|| unreachable!("兜底底板字节长恒为 s×s×4（s={s}）"))
}

fn flat_bytes(size: u32) -> Vec<u8> {
    let s = size.max(1) as usize;
    let mut rgba = Vec::with_capacity(s * s * 4);
    for _ in 0..s * s {
        rgba.extend_from_slice(&[FALLBACK[0], FALLBACK[1], FALLBACK[2], 255]);
    }
    rgba
}

fn draw(size: u32) -> Option<WindowIcon> {
    let s = size.max(1);
    WindowIcon::from_rgba(s, s, raster(s)?)
}

/// 按请求边长把 logo 光栅化成正方形非预乘 RGBA。
///
/// `target_width=Some(s)`：resvg 按宽等比缩放。本 logo 固有尺寸 1299×1299
/// （正方形），常规图标尺寸下恰好落在 s×s；即便换了一个宽高比更偏的源，
/// [`to_square`] 也保证输出恒为正方形（图标契约）。
fn raster(size: u32) -> Option<Vec<u8>> {
    let s = size.max(1);
    let img = Image::from_svg_bytes(LOGO_SVG, Some(s)).ok()?;
    let (w, h) = (img.width(), img.height());
    Some(to_square(s as usize, w as usize, h as usize, img.to_rgba()))
}

/// 光栅产物居中裁/补成 s×s：短边方向补透明、长边方向居中裁。
/// 输入宽高比接近 1:1 时几乎不触发，纯属防御。
fn to_square(s: usize, w: usize, h: usize, rgba: Vec<u8>) -> Vec<u8> {
    let mut out = vec![0u8; s * s * 4];
    let cw = w.min(s);
    let ch = h.min(s);
    let sx = (w - cw) / 2;
    let sy = (h - ch) / 2;
    let dx = (s - cw) / 2;
    let dy = (s - ch) / 2;
    for row in 0..ch {
        let src = ((sy + row) * w + sx) * 4;
        let dst = ((dy + row) * s + dx) * 4;
        out[dst..dst + cw * 4].copy_from_slice(&rgba[src..src + cw * 4]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    /// 任意尺寸都要能出图（系统会要 16/24/32/48/64…，早年的偶必须是 2 的幂）。
    #[test]
    fn any_size_produces_full_icon() {
        for s in [16u32, 24, 32, 48, 64, 256] {
            let icon = draw(s).expect("logo SVG 光栅化不应失败");
            assert_eq!((icon.width(), icon.height()), (s, s));
            let rgba = icon.rgba();
            assert_eq!(rgba.len(), (s * s * 4) as usize);
        }
        // 尺寸 0 也要夹住（平台层保证 ≥1，这里是防御）。
        assert!(draw(0).is_some());
    }

    /// 四角必须是**透明**的（logo 底板是圆角矩形）：否则托盘/任务栏上是一块方板。
    #[test]
    fn corners_are_transparent_center_is_opaque() {
        let rgba = raster(32).expect("32 可光栅化");
        let alpha_at = |x: usize, y: usize| rgba[(y * 32 + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 0, "左上角必须透");
        assert!(alpha_at(16, 16) > 200, "正中必须实");
    }

    /// 内容测试：品牌橙红（#D14527）与品牌深灰（#272B29）都要出现在产物里——
    /// 证明渲的是真 logo，而不是兜底纯色或半途坏掉的图。
    /// 容差 ±10（不是 ±30）：新旧两版相邻色（#D14545↔#D14527、#454545↔#272B29）
    /// 每通道差恰好 ≤30，宽容会互相误过——色断言必须能区分真假源。
    #[test]
    fn brand_colors_are_present() {
        let rgba = raster(64).expect("64 可光栅化");
        let near = |px: &[u8], c: [u8; 3]| {
            px[3] > 200 && (0..3).all(|i| (px[i] as i32 - c[i] as i32).abs() <= 10)
        };
        let has = |c: [u8; 3]| rgba.chunks_exact(4).any(|px| near(px, c));
        assert!(has([209, 69, 39]), "品牌橙红 #D14527 缺失");
        assert!(has([39, 43, 41]), "品牌深灰 #272B29 缺失");
    }

    /// 托盘源也是同一次光栅化（不另做第二套造型：窗口与托盘该是同一张脸）。
    #[test]
    fn tray_icon_matches_app_icon() {
        let (w, h, a) = tray_icon();
        assert_eq!((w, h), (32, 32));
        assert_eq!(a, draw(32).expect("32 合法").rgba().to_vec());
    }

    /// `IconSource::sized` 要能按不同尺寸给出**不同像素数**（而不是反复给同一张）。
    #[test]
    fn icon_source_is_sized() {
        let src = app_icon();
        assert!(src.is_sized());
        assert_eq!(src.at(32).width(), 32);
        assert_eq!(src.at(48).width(), 48);
    }

    /// `assets/rupix.ico`（exe 的 PE 图标资源，资源管理器/任务管理器显示它）的每个
    /// 尺寸条目都要与现场光栅化逐像素一致：icon.svg 改了而 ico 忘了重新生成时这条会红。
    /// 生成器：`cargo run --example gen_icon`。
    #[test]
    fn pe_icon_asset_matches_freshly_rasterized_logo() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/rupix.ico");
        let bytes = fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "读 {} 失败：{e}（改过 icon.svg 后请跑 cargo run --example gen_icon）",
                path.display()
            )
        });
        let entries = parse_ico(&bytes);
        assert_eq!(entries.len(), 6, "ico 应含 6 档尺寸（16/24/32/48/64/256）");

        for size in [16u32, 24, 32, 48, 64, 256] {
            let (bgra, mask) = entries
                .get(&size)
                .unwrap_or_else(|| panic!("ico 缺少 {size}px 条目"));
            let rgba = raster(size).unwrap_or_else(|| panic!("{size}px 光栅化失败"));

            // ico 的 BMP 条目存 BGRA + AND 掩码；此处按同一份 raster 构造期望值，
            // 生成器写错通道序/掩码阈值/行方向都会在这里逐字节暴露。
            let mut want_bgra = Vec::with_capacity(rgba.len());
            let mut want_mask = Vec::with_capacity((size * size) as usize);
            for px in rgba.chunks_exact(4) {
                want_bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                want_mask.push(px[3] < 128);
            }
            assert_eq!(
                bgra, &want_bgra,
                "{size}px 的 BGRA 像素与现场光栅化不一致（icon.svg 改了没重生成 ico？）"
            );
            assert_eq!(mask, &want_mask, "{size}px 的 AND 掩码不一致");
        }
    }

    /// 解析 ICO 容器：返回 size → (顶行优先 BGRA, 顶行优先 AND 掩码位)。
    /// 同时支持本项目生成器产出的两种条目：
    /// - PNG 内嵌条目（2026-10 起，省 .rsrc 体积）：整条 data 是一份完整 PNG 文件，
    ///   自带 alpha，无需 AND 掩码；
    /// - 旧 32bpp BMP 条目（兼容历史资源）。
    ///   解析器与 `examples/gen_icon.rs` 互为镜像，任一侧改了编码约定，同步测试立刻红。
    fn parse_ico(bytes: &[u8]) -> HashMap<u32, (Vec<u8>, Vec<bool>)> {
        let u16le = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let u32le =
            |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        assert_eq!(u16le(0), 0, "ICO reserved 应为 0");
        assert_eq!(u16le(2), 1, "ICO type 应为 1（图标）");
        let count = u16le(4) as usize;

        let mut out = HashMap::new();
        for i in 0..count {
            let e = 6 + i * 16;
            let w = if bytes[e] == 0 { 256 } else { bytes[e] as u32 };
            let h = if bytes[e + 1] == 0 {
                256
            } else {
                bytes[e + 1] as u32
            };
            assert_eq!(w, h, "{w}×{h} 条目应为正方形");
            assert_eq!(u16le(e + 6), 32, "{w}px 条目应为 32bpp");
            let len = u32le(e + 8) as usize;
            let off = u32le(e + 12) as usize;
            let data = &bytes[off..off + len];

            // PNG 内嵌条目：data 以 PNG 签名开头，整条即一份完整 PNG 文件。
            if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
                let (w2, h2, rgba) = decode_png(data);
                assert_eq!(w2, w, "{w}px PNG 解码宽不符");
                assert_eq!(h2, h, "{w}px PNG 解码高不符");
                let mut bgra = Vec::with_capacity(rgba.len());
                let mut mask = Vec::with_capacity(rgba.len() / 4);
                // 现场光栅与期望值同为顶行优先 RGBA；这里转 BGRA 并据 alpha 推掩码。
                for px in rgba.chunks_exact(4) {
                    bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    mask.push(px[3] < 128);
                }
                out.insert(w, (bgra, mask));
                continue;
            }

            // 旧 BMP 条目：BITMAPINFOHEADER + XOR(BGRA 自底向上) + AND 掩码。
            let u32d =
                |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
            assert_eq!(u32d(0), 40, "{w}px 条目应为 BITMAPINFOHEADER");
            assert_eq!(u32d(4), w, "{w}px biWidth 不符");
            assert_eq!(u32d(8), h * 2, "{w}px biHeight 应为 XOR+AND 双倍");
            assert_eq!(u16d(data, 12), 1, "{w}px biPlanes 应为 1");
            assert_eq!(u16d(data, 14), 32, "{w}px biBitCount 应为 32");

            // BMP 为自底向上行序：文件第 0 行 = 图像最后一行。
            let wus = w as usize;
            let row = wus * 4;
            let mut bgra = Vec::with_capacity(wus * wus * 4);
            let mut mask = Vec::with_capacity(wus * wus);
            let mask_stride = wus.div_ceil(32) * 4;
            let mask_base = 40 + wus * wus * 4;
            for y in 0..wus {
                let stored = wus - 1 - y;
                let at = 40 + stored * row;
                bgra.extend_from_slice(&data[at..at + row]);
                let mrow = mask_base + stored * mask_stride;
                for x in 0..wus {
                    mask.push(data[mrow + x / 8] & (0x80 >> (x % 8)) != 0);
                }
            }
            out.insert(w, (bgra, mask));
        }
        out
    }

    /// 解码 ICO 内嵌的 PNG 条目，返回 (宽, 高, 顶行优先 RGBA8)。
    /// 与 `examples/gen_icon.rs::png_entry` 互为镜像。
    fn decode_png(data: &[u8]) -> (u32, u32, Vec<u8>) {
        use std::io::Cursor;
        let dec = png::Decoder::new(Cursor::new(data));
        let mut reader = dec
            .read_info()
            .unwrap_or_else(|e| panic!("ICO PNG 解码头失败：{e}"));
        // 先取宽高并把 info 的不可变借用限制在局部，避免与下方 next_frame 的可变借用冲突。
        let (w, h) = {
            let info = reader.info();
            assert_eq!(
                info.color_type,
                png::ColorType::Rgba,
                "ICO PNG 应为 RGBA8（生成器保证）"
            );
            (info.width, info.height)
        };
        let mut buf = vec![0u8; w as usize * h as usize * 4]; // RGBA8
        reader
            .next_frame(&mut buf)
            .unwrap_or_else(|e| panic!("ICO PNG 解码帧失败：{e}"));
        (w, h, buf)
    }

    fn u16d(data: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([data[o], data[o + 1]])
    }
}
