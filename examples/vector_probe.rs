//! 临时探针：矢量路线（ABI v6 区域渲染）端到端验证。
//!
//! 四段：
//! 1. 注册表：哪枚解码器声明了哪些矢量后缀；
//! 2. 整图：`probe_decode` 走生产路由（矢量路线 → 解码器按 target 自渲染）；
//! 3. 区域重渲：整幅 @4×（对照）与 **4000% 场景**（视口只占文档 1/40、密度 40×）；
//! 4. ★ **无损判据（对照式）**：同一个区域，拿两版像素比：
//!    - **A = 解码器按密度重渲**（生产路线）；
//!    - **B = 位图双线性插值**（对照组，用同一张 1× 整图裁剪放大）。
//!
//!    位图插值会把 1 px 硬边摊成 4 px 灰坡 ⇒ **硬边（相邻像素差 ≥128）几乎消失**；
//!    重新光栅化的边仍然落在 1–2 px 内 ⇒ **硬边数保得住**。
//!    所以判据 = A 的硬边数显著高于 B ⇒ A 是重画出来的，不是插值出来的。
//!
//! 判据**不依赖具体格式**（不写死夹具的棋盘格距）：任何矢量文档都适用。

use std::path::Path;
use std::sync::mpsc;

use rupix_lib::core::decode::{Decoder, RegionReq, ReqKind};

/// 4000% 场景的倍率：视口只占文档的 1/40，密度 = 40 设备像素/文档像素。
const ZOOM_4000: f32 = 40.0;

/// 相邻像素差 ≥ 该值算"硬边"（灰坡/抗锯齿的差值都在 128 以下）。
const HARD_DELTA: i32 = 128;

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    for e in rupix_lib::decoders::list_decoders() {
        println!(
            "id={} health={:?} has_vector={} vector_exts={:?}",
            e.id, e.health, e.has_vector, e.vector_exts
        );
    }
    println!(
        "declared_vector_exts={:?}",
        rupix_lib::decoders::declared_vector_exts()
    );

    let args: Vec<String> = std::env::args().collect();
    let Some(p) = args.get(1) else {
        println!("用法：vector_probe <文件>");
        return;
    };
    let path = Path::new(p);

    // 2. 整图（生产路由）
    let whole = match rupix_lib::core::decode::probe_decode(path, 0, 0) {
        Ok(d) => {
            println!(
                "整图 OK：natural={:?} bitmap={}x{} region={:?}",
                d.natural,
                d.bitmap.width,
                d.bitmap.height,
                d.region.is_some()
            );
            Some(d)
        }
        Err(e) => {
            println!("整图 ERR：{e}");
            None
        }
    };

    let Some(whole) = whole else { return };
    let natural = whole.natural;
    let base = whole.bitmap.rgba.clone();
    let bw = whole.bitmap.width;
    let bh = whole.bitmap.height;

    // 3. 区域重渲（放大机制）
    let (tx, rx) = mpsc::channel();
    let dec = Decoder::spawn_with(tx);

    // ① 对照组：整幅 @4×。
    let region = RegionReq {
        origin: (0.0, 0.0),
        size: (natural.0 as f32, natural.1 as f32),
        density: 4.0,
        doc: natural,
        index: 0,
    };
    dec.request_region(path, region, ReqKind::Main);
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(done) => match done.out {
            Ok(d) => {
                println!(
                    "区域重渲（整幅 @4×）OK：natural={:?} bitmap={}x{} region={:?}（应为 {}x{}）",
                    d.natural,
                    d.bitmap.width,
                    d.bitmap.height,
                    d.region.is_some(),
                    (natural.0 as f32 * 4.0) as u32,
                    (natural.1 as f32 * 4.0) as u32
                );
                report(
                    &d.bitmap.rgba,
                    d.bitmap.width,
                    d.bitmap.height,
                    &CropCtx {
                        base: &base,
                        bw,
                        bh,
                        ox: 0.0,
                        oy: 0.0,
                        rw: natural.0 as f32,
                        rh: natural.1 as f32,
                        density: 4.0,
                    },
                );
            }
            Err(e) => println!("区域重渲（整幅 @4×）ERR：{e}"),
        },
        Err(e) => println!("区域重渲（整幅 @4×）超时/无回包：{e}"),
    }

    // ② ★ 4000% 场景：视口 = 文档的 1/40，密度 = 40×。
    //    画布 = 区域 × 密度 ≈ **一屏大小** ⇒ 倍率再高也顶得住像素硬顶。
    //    视口要落在**有内容的地方**，否则整屏纯色、没有可比边界。
    let (vx, vy) = first_edge(&base, bw, bh);
    let vw = (natural.0 as f32 / ZOOM_4000).max(1.0);
    let vh = (natural.1 as f32 / ZOOM_4000).max(1.0);
    let ox = (vx as f32 - vw / 2.0).clamp(0.0, (natural.0 as f32 - vw).max(0.0));
    let oy = (vy as f32 - vh / 2.0).clamp(0.0, (natural.1 as f32 - vh).max(0.0));

    let region = RegionReq {
        origin: (ox, oy),
        size: (vw, vh),
        density: ZOOM_4000,
        doc: natural,
        index: 0,
    };
    let expect_w = (vw * ZOOM_4000) as u32;
    let expect_h = (vh * ZOOM_4000) as u32;
    dec.request_region(path, region, ReqKind::Main);
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(done) => match done.out {
            Ok(d) => {
                println!(
                    "区域重渲（4000% 视口）OK：bitmap={}x{} region={:?}（画布应 ≈ {}x{}，即一屏大小）",
                    d.bitmap.width,
                    d.bitmap.height,
                    d.region.is_some(),
                    expect_w,
                    expect_h
                );
                report(
                    &d.bitmap.rgba,
                    d.bitmap.width,
                    d.bitmap.height,
                    &CropCtx {
                        base: &base,
                        bw,
                        bh,
                        ox,
                        oy,
                        rw: vw,
                        rh: vh,
                        density: ZOOM_4000,
                    },
                );
            }
            Err(e) => println!("区域重渲（4000% 视口）ERR：{e}"),
        },
        Err(e) => println!("区域重渲（4000% 视口）超时/无回包：{e}"),
    }
}

/// 无损判据的输入上下文：1× 整图 + 待裁的文档区域 + 密度。
struct CropCtx<'a> {
    base: &'a [u8],
    bw: u32,
    bh: u32,
    ox: f32,
    oy: f32,
    rw: f32,
    rh: f32,
    density: f32,
}

/// 无损判据（对照式）。
///
/// A = 区域重渲结果；B = 同一区域从 1× 整图双线性插值出来的对照组。
/// 硬边数 A ≫ B ⇒ A 是**按密度重新光栅化**的（无损放大成立）；
/// 两者接近 ⇒ A 其实只是把位图插值放大（不成立）。
fn report(a: &[u8], aw: u32, ah: u32, ctx: &CropCtx<'_>) {
    let hard_a = hard_edges(a, aw, ah);
    let (crop, cw0, ch0) = crop_base(ctx.base, ctx.bw, ctx.bh, ctx.ox, ctx.oy, ctx.rw, ctx.rh);
    let cw = ((ctx.rw * ctx.density) as u32).max(1);
    let ch = ((ctx.rh * ctx.density) as u32).max(1);
    let interp = bilinear_upscale(&crop, cw0, ch0, cw, ch);
    let hard_b = hard_edges(&interp, cw, ch);
    let hard_base = hard_edges(ctx.base, ctx.bw, ctx.bh);

    println!("无损判据：硬边数 重渲={hard_a} / 插值对照={hard_b} / 1× 整图={hard_base}");
    if hard_a == 0 {
        println!("  ⇒ 该区域没有可比的边界（视口落在纯色区），判据不适用");
    } else if hard_b == 0 || hard_a >= hard_b * 2 {
        println!("  ⇒ ★ 重渲保住了硬边、插值把边抹平 ⇒ **按目标密度重新光栅化**（无损放大成立）");
    } else {
        println!("  ⇒ ⚠ 重渲与插值对照的硬边数接近 ⇒ 疑似位图插值放大（不是无损放大）");
    }
}

/// 统计整幅图的硬边数（相邻像素 R 通道差 ≥ HARD_DELTA）。
fn hard_edges(rgba: &[u8], w: u32, h: u32) -> usize {
    if w < 2 || h == 0 {
        return 0;
    }
    let stride = w as usize;
    let mut n = 0usize;
    for y in 0..h as usize {
        for x in 1..stride {
            let a = rgba[(y * stride + x - 1) * 4] as i32;
            let b = rgba[(y * stride + x) * 4] as i32;
            if (a - b).abs() >= HARD_DELTA {
                n += 1;
            }
        }
    }
    n
}

/// 找第一处硬边的坐标（用于把 4000% 视口落在有内容的地方）。
fn first_edge(rgba: &[u8], w: u32, h: u32) -> (u32, u32) {
    if w < 2 || h == 0 {
        return (0, 0);
    }
    let stride = w as usize;
    for y in 0..h as usize {
        for x in 1..stride {
            let a = rgba[(y * stride + x - 1) * 4] as i32;
            let b = rgba[(y * stride + x) * 4] as i32;
            if (a - b).abs() >= HARD_DELTA {
                return (x as u32, y as u32);
            }
        }
    }
    (0, 0)
}

/// 从 1× 整图裁出文档区域（越界处填白），返回 `(像素, 宽, 高)`。
fn crop_base(
    base: &[u8],
    bw: u32,
    bh: u32,
    ox: f32,
    oy: f32,
    rw: f32,
    rh: f32,
) -> (Vec<u8>, u32, u32) {
    let cw = rw.ceil().max(1.0) as u32;
    let ch = rh.ceil().max(1.0) as u32;
    let mut out = vec![255u8; (cw as usize) * (ch as usize) * 4];
    for y in 0..ch {
        let sy = oy as i64 + y as i64;
        for x in 0..cw {
            let sx = ox as i64 + x as i64;
            if sx < 0 || sy < 0 || sx >= bw as i64 || sy >= bh as i64 {
                continue;
            }
            let si = ((sy as usize) * bw as usize + sx as usize) * 4;
            let di = (y as usize * cw as usize + x as usize) * 4;
            out[di..di + 4].copy_from_slice(&base[si..si + 4]);
        }
    }
    (out, cw, ch)
}

/// 双线性放大（对照组：模拟"渲大图再缩/位图插值放大"的结果）。
fn bilinear_upscale(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![255u8; (dw as usize) * (dh as usize) * 4];
    if sw == 0 || sh == 0 {
        return out;
    }
    for y in 0..dh as usize {
        let sy = ((y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(sh as usize - 1);
        let fy = sy - y0 as f32;
        for x in 0..dw as usize {
            let sx = ((x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(sw as usize - 1);
            let fx = sx - x0 as f32;
            for c in 0..4 {
                let p00 = src[(y0 * sw as usize + x0) * 4 + c] as f32;
                let p10 = src[(y0 * sw as usize + x1) * 4 + c] as f32;
                let p01 = src[(y1 * sw as usize + x0) * 4 + c] as f32;
                let p11 = src[(y1 * sw as usize + x1) * 4 + c] as f32;
                let top = p00 + (p10 - p00) * fx;
                let bot = p01 + (p11 - p01) * fx;
                let v = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
                out[(y * dw as usize + x) * 4 + c] = v;
            }
        }
    }
    out
}
