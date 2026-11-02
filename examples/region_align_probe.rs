//! 临时探针：区域渲染的**像素对齐**验证（矢量放大显示不全的定案探针）。
//!
//! 问题现象（用户报）：放大后画布显示的是图像的**一角**，与鸟瞰缩略图里标出的
//! 视口位置对不上——像"显示不全 / 位置错了"。
//!
//! 定案方法（不靠肉眼）：
//! 1. 整幅基准：把文档整幅按 density=1 渲一张 557×561 的**基准图** R；
//! 2. 区域图：对**同一区域**发一次 `iv_render_region`（density=2），拿到 P；
//! 3. 把基准图 R 按 2× 重采样（最近邻，不引入插值误差）后裁出同一块 Q；
//! 4. 逐像素比对 P 与 Q：
//!    - 完全一致 ⇒ 解码器按区域渲染且对齐（问题在宿主贴图/调度）；
//!    - **内容对但位置偏移** ⇒ 解码器的区域坐标与文档坐标系不匹配
//!      （最可能：文档自然尺寸与矢量用户空间不是 1:1）；
//!    - 内容完全不同 ⇒ 解码器根本没按区域渲（把整篇缩进那一小块）。
//!
//! ★ 用最近邻重采样：插值会把边糊掉，掩盖几像素的错位。

use std::path::Path;
use std::sync::mpsc;

use rupix_lib::core::decode::{Decoder, RegionReq, ReqKind};

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    let args: Vec<String> = std::env::args().collect();
    let Some(p) = args.get(1) else {
        println!("用法：region_align_probe <文件>");
        return;
    };
    let path = Path::new(p);

    // ── 1. 整幅基准（density = 1）────────────────────────────────
    let natural = match whole_natural(path) {
        Some(n) => n,
        None => {
            println!("✗ 整幅解码失败");
            return;
        }
    };
    println!("文档自然尺寸 = {natural:?}");

    // 选一块**不在中心附近**的区域：中心区域容易被"整篇缩进"蒙混过去。
    let (nw, nh) = (natural.0 as f32, natural.1 as f32);
    let rx = (nw * 0.45).floor();
    let ry = (nh * 0.55).floor();
    let rw = (nw * 0.40).min(nw - rx).floor().max(8.0);
    let rh = (nh * 0.35).min(nh - ry).floor().max(8.0);
    println!("测试区域（文档坐标）= origin=({rx}, {ry}) size=({rw}, {rh})");

    let density = 2.0f32;
    let (tx, rx_ch) = mpsc::channel();
    let dec = Decoder::spawn_with(tx);
    let req = RegionReq {
        origin: (rx, ry),
        size: (rw, rh),
        density,
        doc: natural,
        index: 0,
    };
    dec.request_region(path, req, ReqKind::Main);
    let got = match rx_ch.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(done) => match done.out {
            Ok(d) => d,
            Err(e) => {
                println!("✗ 区域渲染失败：{e}");
                return;
            }
        },
        Err(e) => {
            println!("✗ 区域渲染超时：{e}");
            return;
        }
    };
    let pw = got.bitmap.width as f32;
    let ph = got.bitmap.height as f32;
    println!(
        "区域渲染返回：画布={pw}x{ph}（期望 ≈ {}x{}）natural={:?} region={:?}",
        rw * density,
        rh * density,
        got.natural,
        got.region.is_some()
    );

    // ── 2. 基准图（整幅 density=1）───────────────────────────────
    // 整幅 density=1 的像素：用普通整图解码（宿主生产路由的目标尺寸 = natural）。
    let base_rgba = match whole_rgba(path, natural) {
        Some(b) => b,
        None => {
            println!("✗ 基准整幅解码失败");
            return;
        }
    };
    let (bw, bh) = (natural.0, natural.1);

    // ── 3. 最近邻 2× 重采样 + 裁出同一块 ─────────────────────────
    // 期望：Q[(x,y)] = base[(rx + x/2) as usize, (ry + y/2) as usize]
    let qw = pw.round() as usize;
    let qh = ph.round() as usize;
    let mut diff = 0usize;
    let mut total = 0usize;
    let mut max_delta = 0i32;
    // 记录一条水平剖面上的像素，便于人工核对。
    let mut profile: Vec<u8> = Vec::new();
    let prof_row = (qh / 2).min(qh.saturating_sub(1));
    for y in 0..qh {
        for x in 0..qw {
            let sx = (rx as usize) + (x as f32 / density) as usize;
            let sy = (ry as usize) + (y as f32 / density) as usize;
            if sx >= bw as usize || sy >= bh as usize {
                continue;
            }
            let si = (sy * bw as usize + sx) * 4;
            let di = (y * qw + x) * 4;
            let mut d = 0i32;
            for c in 0..3 {
                let a = got.bitmap.rgba[di + c] as i32;
                let b = base_rgba[si + c] as i32;
                d += (a - b).abs();
            }
            total += 1;
            if d > 30 {
                diff += 1;
            }
            if d > max_delta {
                max_delta = d;
            }
            if y == prof_row && x < 24 {
                profile.push(got.bitmap.rgba[di]);
            }
        }
    }

    let pct = if total == 0 {
        0.0
    } else {
        diff as f64 * 100.0 / total as f64
    };
    println!("\n══ 比对结果 ══");
    println!("  比对像素 {total} 个，不一致 {diff} 个（{pct:.2}%），最大通道差 {max_delta}");
    println!("  区域图水平剖面前 24 像素 = {profile:?}");
    if diff == 0 {
        println!("  ⇒ ★ 解码器按区域渲染且与文档坐标系对齐");
    } else if pct < 5.0 {
        println!("  ⇒ 内容大体一致但有偏差（可能有 1–2 像素错位或边缘抗锯齿差）");
    } else {
        println!("  ⇒ ⚠ 内容与该区域**不对应**——解码器渲的不是这一块（整篇缩进 / 坐标系不匹配）");
    }
}

/// 整幅自然尺寸。
fn whole_natural(path: &Path) -> Option<(u32, u32)> {
    rupix_lib::core::decode::probe_decode(path, 0, 0)
        .ok()
        .map(|d| d.natural)
}

/// 整幅像素（natural 尺寸，density=1）。
fn whole_rgba(path: &Path, natural: (u32, u32)) -> Option<Vec<u8>> {
    rupix_lib::core::decode::probe_decode(path, 0, 0)
        .ok()
        .map(|d| d.bitmap.rgba)
        .filter(|_| natural.0 > 0 && natural.1 > 0)
}
