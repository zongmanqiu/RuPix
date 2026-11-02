//! SVG 渲染后端（windui `svg` feature 的 resvg 栈，svgz 由 usvg 透明解压）。
//!
//! `core::decode` 只认 [`decode::SvgDecodeFn`] 这个函数指针（启动时经
//! [`crate::install_decode_stack`] 注入）——core 因此不 import windui，
//! 矢量渲染的具体实现停在上屏层。
//!
//! 两条路线：
//! - **整图**（`region=None`，首帧 / probe）：装进 max 框（允许放大），套单边/总量
//!   硬顶。首帧先用它，屏幕上立刻有图。
//! - **可视区域**（`region=Some`，缩放/平移稳定后）：只渲染看到的矩形，按设备像素
//!   1:1 输出——这是**无损放大的真正机制**：位图路线的密度被"整图尺寸×倍率"绑架，
//!   超过硬顶必然糊；区域路线的位图恒为一屏大小，倍率多高都顶得住。

use std::path::Path;

use crate::core::decode::RegionReq;
use crate::core::pixels::{Anim, Bitmap, Decoded, Region, MAX_EDGE, MAX_PIXELS};

/// svgz 解压上限：正常 SVG 文本档撑死几十 MB，256 MiB 只可能是恶意构造
/// （gzip 炸弹）。超限报错，不 OOM。
const SVGZ_MAX_DECOMPRESSED: u64 = 256 * 1024 * 1024;

/// [`crate::core::decode::SvgDecodeFn`] 的实现（签名必须逐字对上）。
pub fn decode(
    path: &Path,
    region: Option<&RegionReq>,
    max_w: u32,
    max_h: u32,
) -> Result<Decoded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败：{e}"))?;
    decode_bytes(&bytes, region, max_w, max_h)
}

/// 后台预热字体库：回放缓存（~11 ms）或首扫（仅本机首次，数百毫秒）都发生在
/// 启动线程上，不占解码线程——这样用户打开第一张 SVG 时字体库多半已热，
/// 首解直接走完整解析单趟（文字与图形同帧）。
pub fn warm_fonts() {
    windui::render::image::svg_warm_fonts();
}

/// 字节渲染管线（**私有**）：整图框 / 可视区域 1:1。
///
/// ★ 2026-10-28：这条管线过去还有一个**公开**入口（`core::decode::VectorDecodeFn`），
/// 供"解码器导出矢量文档 → 宿主用自己的 resvg 渲染"那条路线用。那条路线已随
/// `HostExport` 整条退役（宪章 §5.2：矢量格式一律由解码器按目标尺寸自渲染），
/// 于是它退回**私有**——宿主只对内置 svg/svgz 走这条路。
fn decode_bytes(
    bytes: &[u8],
    region: Option<&RegionReq>,
    max_w: u32,
    max_h: u32,
) -> Result<Decoded, String> {
    use std::borrow::Cow;
    use windui::render::image::Image;

    let t0 = std::time::Instant::now();
    // svgz：gzip 包装（魔数 1f 8b）。m188 实锤：清单与路由都登记了 svgz，但 windui
    // 的 resvg 没开 svgz feature，解析直接报 "enable svgz cargo feature"——解压由
    // 宿主自己完成。解压上限挡 zip 炸弹；
    // 解出来没人要的 .svg 不受影响（不是 gzip 原样走）。
    let bytes: Cow<[u8]> = if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .take(SVGZ_MAX_DECOMPRESSED)
            .read_to_end(&mut out)
            .map_err(|e| format!("svgz 解压失败：{e}"))?;
        if out.len() as u64 >= SVGZ_MAX_DECOMPRESSED {
            return Err(format!(
                "svgz 解压结果达到上限（{SVGZ_MAX_DECOMPRESSED} B），疑似恶意构造"
            ));
        }
        if out.is_empty() {
            return Err("svgz 解压结果为空".into());
        }
        Cow::Owned(out)
    } else {
        Cow::Borrowed(bytes)
    };
    // 只解析拿固有尺寸（不渲染）。早先"先按固有尺寸完整渲一遍只为拿宽高"是
    // 双倍渲染浪费，大 SVG 上直接表现为打开慢。
    let natural = Image::svg_intrinsic_size(&bytes).map_err(|e| e.to_string())?;
    log::debug!(
        "svg_decode：解析固有尺寸 {}×{} 用时 {} ms（route={}）",
        natural.0,
        natural.1,
        t0.elapsed().as_millis(),
        if region.is_some() { "region" } else { "whole" }
    );
    let (img, out_region) = match region {
        None => {
            let target = if max_w == 0 || max_h == 0 {
                natural
            } else {
                // 装进 (max_w, max_h) 框（允许放大）。首帧不搞超采样——它只需要
                // "立刻有图"，清晰度由随后的可视区域重渲接管（那才是常态路线）。
                let s = (max_w as f64 / natural.0 as f64).min(max_h as f64 / natural.1 as f64);
                let mut tw = ((natural.0 as f64 * s).round() as u32).max(1);
                let mut th = ((natural.1 as f64 * s).round() as u32).max(1);
                if tw > MAX_EDGE || th > MAX_EDGE {
                    let k = (MAX_EDGE as f64 / tw.max(th) as f64).min(1.0);
                    tw = ((tw as f64 * k).floor() as u32).max(1);
                    th = ((th as f64 * k).floor() as u32).max(1);
                }
                let px = tw as u64 * th as u64;
                if px > MAX_PIXELS {
                    let k = (MAX_PIXELS as f64 / px as f64).sqrt();
                    tw = ((tw as f64 * k).floor() as u32).max(1);
                    th = ((th as f64 * k).floor() as u32).max(1);
                }
                (tw, th)
            };
            let img = Image::from_svg_bytes(
                &bytes,
                if target == natural {
                    None
                } else {
                    Some(target.0)
                },
            )
            .map_err(|e| e.to_string())?;
            (img, None)
        }
        Some(r) => {
            // 区域渲染：密度先行，超硬顶时降密度而不是失败（极端倍率下宁可略糊）。
            let mut k = r.density.max(f32::EPSILON);
            let mut w = r.size.0 * k;
            let mut h = r.size.1 * k;
            let mx = w.max(h) / MAX_EDGE as f32;
            if mx > 1.0 {
                k /= mx;
                w /= mx;
                h /= mx;
            }
            let px = (w * h) as u64;
            if px > MAX_PIXELS {
                let f = (MAX_PIXELS as f64 / px as f64).sqrt() as f32;
                k *= f;
            }
            let img =
                Image::from_svg_region(&bytes, r.origin, r.size, k).map_err(|e| e.to_string())?;
            (
                img,
                Some(Region {
                    origin: r.origin,
                    density: k,
                }),
            )
        }
    };
    Ok(Decoded {
        natural,
        region: out_region,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: img.width(),
            height: img.height(),
            rgba: img.to_rgba(),
        },
    })
    .inspect(|d| {
        log::debug!(
            "svg_decode：完成 {}×{}，总用时 {} ms",
            d.bitmap.width,
            d.bitmap.height,
            t0.elapsed().as_millis()
        );
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 区域渲染路线：按请求的 origin/size/density 出图，region 随结果带回。
    ///
    /// 这是"SVG 无损放大"的直接凭据：100×50 的图取 (20,10) 起 40×20 的区域、
    /// 密度 2 → 位图必须恰为 80×40，且区域信息原样返回供定位绘制。
    #[test]
    fn svg_region_route_renders_requested_area() {
        let t0 = std::time::Instant::now();
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("sv_svg_region_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.svg");
        std::fs::write(
            &p,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="#123456"/></svg>"##,
        )
        .unwrap();
        let region = RegionReq {
            origin: (20.0, 10.0),
            size: (40.0, 20.0),
            density: 2.0,
            doc: (100, 50),
            index: 0,
        };
        let d = decode(&p, Some(&region), 0, 0).unwrap();
        let t_cold = t0.elapsed();
        let t0 = std::time::Instant::now();
        let _warm = decode(&p, Some(&region), 0, 0).unwrap();
        println!(
            "svg region 首渲（含字体库扫描）：{:?}；热渲：{:?}",
            t_cold,
            t0.elapsed()
        );
        assert_eq!((d.bitmap.width, d.bitmap.height), (80, 40));
        assert_eq!(d.natural, (100, 50));
        let r = d.region.expect("区域路线必须带回 region");
        assert_eq!(r.origin, (20.0, 10.0));
        assert!((r.density - 2.0).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ svgz 的宿主解压（m188）：gzip 包装的 SVG 必须与裸 SVG 解出**逐字节相同**
    /// 的像素（windui 的 resvg 没开 svgz feature，解压是宿主的事）；坏 gzip 与
    /// 空结果要报错，不许 panic。
    #[test]
    fn svgz_decodes_identical_to_plain_and_rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("sv_svgz_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="40"><rect width="60" height="40" fill="#00ff00"/><circle cx="30" cy="20" r="15" fill="#ff0000"/></svg>"##;
        let gz = {
            use std::io::Write;
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(svg).unwrap();
            enc.finish().unwrap()
        };
        let p_plain = dir.join("a.svg");
        let p_gz = dir.join("a.svgz");
        std::fs::write(&p_plain, svg).unwrap();
        std::fs::write(&p_gz, &gz).unwrap();
        let plain = decode(&p_plain, None, 0, 0).unwrap();
        let gzres = decode(&p_gz, None, 0, 0).unwrap();
        assert_eq!((gzres.bitmap.width, gzres.bitmap.height), (60, 40));
        assert_eq!(
            fp(&plain.bitmap.rgba),
            fp(&gzres.bitmap.rgba),
            "svgz 与裸 SVG 必须同像素"
        );

        // 坏 gzip：报错，不 panic。
        let p_bad = dir.join("bad.svgz");
        std::fs::write(&p_bad, [0x1f, 0x8b, 0x08, 0x00, 1, 2, 3]).unwrap();
        assert!(decode(&p_bad, None, 0, 0).is_err(), "坏 gzip 必须报错");

        // 空内容 gzip：报错。
        let p_empty = dir.join("empty.svgz");
        let empty_gz = {
            use std::io::Write;
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            enc.flush().unwrap();
            enc.finish().unwrap()
        };
        std::fs::write(&p_empty, &empty_gz).unwrap();
        assert!(decode(&p_empty, None, 0, 0).is_err(), "解压为空必须报错");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 像素指纹（FNV-1a）：判"两趟是不是同一张图"用。
    fn fp(buf: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in buf {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }
}
