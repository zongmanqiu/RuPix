//! 纯像素几何：EXIF 方向补转、宿主侧缩放、视图旋转/翻转。
//!
//! 全部是**纯函数**（`Vec<u8>` 进、`Vec<u8>` 出），不碰文件、不碰 UI、不碰解码器——
//! 所以能从 `decoders`/`viewer` 里整体上收进 core，并被两侧共用：
//!
//! - 解码器侧：[`oriented`]（ABI 契约 `already_oriented=0` 时宿主补转）、[`fit_to`]（解码后
//!   缩到上屏框）、[`fit_oriented`]（两者合做一步，省掉整张那一趟搬运）；
//! - 视图侧：`transformed`（旋转/翻转按钮）。windui `draw_image` 只有目标矩形，
//!   没有旋转/翻转参数；而位图是"缩到上屏"的几 MB 级采样，直接在像素层变换
//!   （毫秒级）比改框架渲染管线或走解码线程都划算。

use crate::core::pixels::Bitmap;

// ── EXIF 方向补转（宿主侧）──────────────────────────────────────

/// 按 EXIF 方向（1–8）重排像素：解码器上报 `already_oriented=0` 时宿主负责补转
/// （ABI 契约见 `RuPix-Decoder/abi`：raw/av 上报未应用状态，jxl 已应用）。
/// 方向 1 或非法值原样返回——不认识的方向宁可躺平也不猜。
pub fn oriented(rgba: Vec<u8>, w: u32, h: u32, o: i32) -> (Vec<u8>, u32, u32) {
    // 每个方向：输出像素 (x, y) 采样自输入的哪个 (sx, sy)。
    // 映射逐个用 2×3 非对称图案手算并在 `tests::orientation_*` 锁定。
    let map: fn(u32, u32, u32, u32) -> (u32, u32) = match o {
        2 => |x, y, w, _| (w - 1 - x, y),         // 左右镜像
        3 => |x, y, w, h| (w - 1 - x, h - 1 - y), // 旋转 180°
        4 => |x, y, _, h| (x, h - 1 - y),         // 上下镜像
        5 => |x, y, _, _| (y, x),                 // 转置（沿主对角线翻）
        6 => |x, y, _, h| (y, h - 1 - x),         // 旋转 90° 顺时针
        7 => |x, y, w, h| (w - 1 - y, h - 1 - x), // 沿副对角线翻
        8 => |x, y, w, _| (w - 1 - y, x),         // 旋转 270° 顺时针
        _ => return (rgba, w, h),
    };
    let (ow, oh) = if matches!(o, 5..=8) { (h, w) } else { (w, h) };
    let mut out = vec![0u8; rgba.len()];
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = map(x, y, w, h);
            let di = ((y * ow + x) * 4) as usize;
            let si = ((sy * w + sx) * 4) as usize;
            out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    (out, ow, oh)
}

// ── 宿主侧缩放 ─────────────────────────────────────────────────────

/// 把解码器返回的位图缩到「上屏需要的框」内（等比、不放大）。面积平均，质量够用。
pub fn fit_to(rgba: Vec<u8>, w: u32, h: u32, max_w: u32, max_h: u32) -> Bitmap {
    let (tw, th) = fit_dims(w, h, max_w, max_h);
    if (tw, th) == (w, h) {
        return Bitmap {
            width: w,
            height: h,
            rgba,
        };
    }
    let out = downscale(&rgba, w, h, tw, th);
    Bitmap {
        width: tw,
        height: th,
        rgba: out,
    }
}

/// 「先转再缩」，但**整图那一趟搬运不做**：先按方向反推出来的块划分降采样，再把那份
/// 小图转正。与 `fit_to(oriented(..))` **逐字节相同**（[`fit_oriented_matches_orient_then_fit`]）。
///
/// ★ 为什么不写成"先缩再转"那么显然：`downscale` 的输出格边界是 `floor(i*n/t)`，从**近端**
///   锚定；而"先转再缩"里被方向反着取样（`w-1-x` / `h-1-y`）的那根轴，其格边界落在**远端**
///   （等价 `ceil(i*n/t)`）。缩放比不是整数时两套格罩住的源像素**不是同一批** ⇒ 朴素的
///   "先缩再转"会改像素：12 MP 竖拍、1280×720 视口实测 6/7/8 **每一个**输出像素都不同
///   （平均差 3.5/255、最大 49，`m49` 的 A/B 探针）。只有方向 5（纯转置、无反向）碰巧相同，
///   而它最罕见——所以"碰巧相同"不能作为接线依据。
///
/// 这里把反向轴的格划分改成远端锚定，转正只作用在几 MB 的小图上：48 MB 的跨步搬运没了，
/// 像素一个不变。
pub fn fit_oriented(rgba: Vec<u8>, w: u32, h: u32, o: i32, max_w: u32, max_h: u32) -> Bitmap {
    if !matches!(o, 2..=8) {
        return fit_to(rgba, w, h, max_w, max_h);
    }
    // 转正后的尺寸：与 `oriented` 同一套换轴规则，保证档位尺寸与参照路径一致。
    let (ow, oh) = if matches!(o, 5..=8) { (h, w) } else { (w, h) };
    let (fw, fh) = fit_dims(ow, oh, max_w, max_h);
    // 降采样在**源坐标系**里做，所以目标尺寸要按同一套换轴规则倒回来。
    let (bw, bh) = if matches!(o, 5..=8) {
        (fh, fw)
    } else {
        (fw, fh)
    };
    let small = if (fw, fh) == (ow, oh) {
        rgba // 装得下 ⇒ 不缩，只转。
    } else {
        // 源轴是否被方向反着取样：`sx = w-1-…` ⇒ x 反向，`sy = h-1-…` ⇒ y 反向。
        // 分支表逐条对照 [`oriented`] 的映射手算，并由参照等值用例整体锁死。
        downscale_anchored(
            &rgba,
            w,
            h,
            bw,
            bh,
            matches!(o, 2 | 3 | 7 | 8),
            matches!(o, 3 | 4 | 6 | 7),
        )
    };
    let (rgba, sw, sh) = oriented(small, bw, bh, o);
    Bitmap {
        width: sw,
        height: sh,
        rgba,
    }
}

/// 等比缩到装得下 `max_w × max_h`，**绝不放大**；`0` 表示该轴不限。
pub fn fit_dims(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (w.max(1), h.max(1));
    }
    if max_w == 0 || max_h == 0 || (w <= max_w && h <= max_h) {
        return (w, h);
    }
    let s = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    (
        ((w as f64 * s).floor() as u32).max(1),
        ((h as f64 * s).floor() as u32).max(1),
    )
}

/// 面积平均降采样。要求 `tw <= w && th <= h`。
///
/// **两遍可分离**：先把这一档覆盖的源行按行**顺序**累加进一份 `w × 4` 的 u32 列累加器
/// （6055 列 = 97 KB，常驻 L2），再从这份小数组里逐输出列归约。与朴素四重循环唯一的
/// 差别是"源怎么被走一遍"——朴素版按输出像素取块，块与块之间在源里跳行跳列。求和顺序
/// 变了但整数加法可交换，**逐字节结果相同**（`downscale_matches_naive_reference` 锁住）。
///
/// ★ 试过更狠的 SWAR（一个 u32 装两条 16 位通道，竖向累加从 4 次加宽变 2 次掩码加）：
///   实测**反而慢 20%**（`bench_downscale` 同进程配对：6055²→1440² 朴素 148.7 / 两遍
///   119.3 / SWAR 179.9 ms）。原因是 `chunks_exact(4).enumerate()` 往两个数组做**跨步
///   写**，LLVM 向量化不出来，白搭进一趟拆包。留这句话是给下一个想"再压一档"的人。
///
/// 为什么值得动它：这是基座侧、与格式无关的一笔钱。三条路都走这一个函数——解码后的
/// `fit_to`（首屏空白期里的 `post` 段，实测各格式 115–200 ms）、缩放换档的本地派生
/// （一档实测 118–131 ms）、鸟瞰。解码器给不给档位都改变不了它。
pub fn downscale(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    downscale_anchored(rgba, w, h, tw, th, false, false)
}

/// 第 `i` 格（共 `t` 格，覆盖 `0..n` 这根轴）罩住的源区间。
/// `ceil = false` 从近端锚定（`floor`，现行降采样的划分）；`ceil = true` 从**远端**锚定
/// （`ceil`），正是"先转再缩"里那根被反着取样的轴应有的划分。两版都保证非空、不越界。
fn band(i: usize, n: usize, t: usize, ceil: bool) -> (usize, usize) {
    let edge = |k: usize| {
        if ceil {
            (k * n).div_ceil(t)
        } else {
            k * n / t
        }
    };
    let a = edge(i).min(n);
    (a, edge(i + 1).max(a + 1).min(n))
}

/// [`downscale`] 的通用版：多两个开关，决定 x / y 轴的格边界从哪一端锚定。
/// 两个都 `false` 时与 [`downscale`] 完全一致（`downscale_anchored_flat_matches_downscale`）。
/// 唯一的消费者是 [`fit_oriented`]——它要在反向的那根轴上用远端锚定，才能和
/// "整图转完再缩"逐字节相同。
pub fn downscale_anchored(
    rgba: &[u8],
    w: u32,
    h: u32,
    tw: u32,
    th: u32,
    ceil_x: bool,
    ceil_y: bool,
) -> Vec<u8> {
    // 竖向累加走不走 AVX2，**每张图判一次**（`is_x86_feature_detected!` 内部有缓存，
    // 但每行再判一次仍是白付）。判不到就是原来那条标量路，一字未变。
    // 实测（`examples/bench_downscale.rs` 候选臂 v3，五臂逐字节断言 + 正反两序）：
    // 6055²→1440² **2.3–2.6×**、→960² 3.0–3.4×、5239²→1440² 1.9×、→180² 5.0×。
    #[cfg(target_arch = "x86_64")]
    let simd = std::arch::is_x86_feature_detected!("avx2");
    #[cfg(not(target_arch = "x86_64"))]
    let simd = false;
    downscale_anchored_impl(rgba, w, h, tw, th, ceil_x, ceil_y, simd)
}

/// 上面那个的实现体，`simd` 由调用方决定（单测要**同一份输入跑两遍**：向量的与标量的，
/// 逐字节比。这是"换写法不许改像素"唯一的证法，见 `downscale_avx2_matches_scalar`）。
// `simd` 平铺成一个 bool 是刻意的：它是测试的强制开关（真机上探测到 AVX2 就永远不会
// 跑标量臂，没有这个口子就证不了等价），为它包一层参数结构只会让生产调用点多一次解包。
#[allow(clippy::too_many_arguments)]
fn downscale_anchored_impl(
    rgba: &[u8],
    w: u32,
    h: u32,
    tw: u32,
    th: u32,
    ceil_x: bool,
    ceil_y: bool,
    simd: bool,
) -> Vec<u8> {
    let (w, h, tw, th) = (w as usize, h as usize, tw as usize, th as usize);
    let mut out = vec![0u8; tw * th * 4];
    let mut col = vec![0u32; w * 4];
    for dy in 0..th {
        let (sy0, sy1) = band(dy, h, th, ceil_y);
        col.fill(0);
        for sy in sy0..sy1 {
            let row = &rgba[sy * w * 4..(sy + 1) * w * 4];
            accumulate_row(&mut col, row, simd);
        }
        let rows = (sy1 - sy0) as u32;
        let o = dy * tw * 4;
        for dx in 0..tw {
            let (sx0, sx1) = band(dx, w, tw, ceil_x);
            let mut acc = [0u32; 4];
            for p in col[sx0 * 4..sx1 * 4].chunks_exact(4) {
                acc[0] += p[0];
                acc[1] += p[1];
                acc[2] += p[2];
                acc[3] += p[3];
            }
            let cnt = rows * (sx1 - sx0) as u32;
            let d = &mut out[o + dx * 4..o + dx * 4 + 4];
            for c in 0..4 {
                d[c] = (acc[c] / cnt) as u8;
            }
        }
    }
    out
}

/// 一趟竖向累加：`col[i] += row[i] as u32`（`row.len() == col.len()`，即一行 RGBA）。
///
/// `simd == true` 时一次搬 8 个 lane（= 2 像素），尾巴仍走标量——**奇数宽度**（6055 这种
/// `lanes % 8 == 4`）正是生产最常见的那一类，所以尾巴分支不是边角料，`cases` 里必须有它。
#[inline]
fn accumulate_row(col: &mut [u32], row: &[u8], simd: bool) {
    #[cfg(target_arch = "x86_64")]
    if simd {
        // SAFETY：`avx2` 已由 `downscale_anchored` 的调用点探测过；`row` 与 `col` 等长
        //         由这里的断言与上面唯一的调用点（同一行的 RGBA 字节 ↔ u32 lane）保证。
        unsafe { accumulate_row_avx2(col, row) };
        return;
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = simd;
    for (acc, px) in col.iter_mut().zip(row) {
        *acc += *px as u32;
    }
}

/// [`accumulate_row`] 的 AVX2 形态。
///
/// # Safety
/// 调用方必须已确认本机支持 `avx2`（唯一的调用点是 [`accumulate_row`]，它只在
/// `is_x86_feature_detected!("avx2")` 为真时进来）。
///
/// **为什么结果与标量逐字节相同**：lane 分组没变——第 `i` 个 u32 累加的仍然是同一列同一
/// 通道的那批字节；`_mm256_add_epi32` 是模 2³² 的回绕加，与 Rust release 里 `u32 +=`
/// 同语义（连"极端档位溢出"这种既有行为都原样保留，不是新引入的差异）。
/// 边界：`i + 8 <= col.len()` 保证读 8 字节、读写 8 个 u32 都落在两个切片内。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn accumulate_row_avx2(col: &mut [u32], row: &[u8]) {
    use std::arch::x86_64::{
        __m256i, _mm256_add_epi32, _mm256_cvtepu8_epi32, _mm256_loadu_si256, _mm256_storeu_si256,
        _mm_cvtsi64_si128,
    };
    let lanes = col.len();
    debug_assert_eq!(lanes, row.len());
    let mut i = 0usize;
    while i + 8 <= lanes {
        // 8 个字节零扩展成 8 个 u32（正好是 2 像素）。`row` 是 `&[u8]`，起点没有
        // 8 字节对齐保证；先 `read_unaligned` 再送进 xmm，别用要求对齐的 `_mm_loadl_epi64`
        // 直接 cast（那是 UB 的形状，只是 x86 的 `movq` 碰巧不炸）。
        let raw = std::ptr::read_unaligned(row.as_ptr().add(i).cast::<i64>());
        let bytes = _mm_cvtsi64_si128(raw);
        let wide = _mm256_cvtepu8_epi32(bytes);
        let p = col.as_mut_ptr().add(i).cast::<__m256i>();
        _mm256_storeu_si256(p, _mm256_add_epi32(_mm256_loadu_si256(p), wide));
        i += 8;
    }
    // 尾巴：与标量臂逐字相同的写法（`lanes` 不是 8 的倍数时走这里，宽度为奇数就是这种）。
    while i < lanes {
        col[i] += row[i] as u32;
        i += 1;
    }
}

// ── 视图旋转 / 翻转 ────────────────────────────────────────────────

/// 顺时针旋转 90°。返回（新缓冲, 新宽, 新高）。
///
/// 映射：`dst(x, y) = src(y, h_src - 1 - x)`（可逆推导见单元测试的角落断言）。
/// ★ 生产路径不再经过它——[`transformed`] 已改成一次成图。留着当**参照实现**：
/// `transformed_matches_stepwise_reference` 拿它对 8 种朝向逐一比对，改错立刻红。
#[cfg(test)]
pub fn rotate_cw_rgba(rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = (h, w);
    let mut out = vec![0u8; rgba.len()];
    for y in 0..nh as usize {
        for x in 0..nw as usize {
            let (sx, sy) = (y, h as usize - 1 - x);
            let di = (y * nw as usize + x) * 4;
            let si = (sy * w as usize + sx) * 4;
            out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    (out, nw, nh)
}

/// 水平镜像：`dst(x, y) = src(w - 1 - x, y)`。同 [`rotate_cw_rgba`]，只当参照实现用。
#[cfg(test)]
pub fn flip_h_rgba(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let di = (y * w as usize + x) * 4;
            let si = (y * w as usize + (w as usize - 1 - x)) * 4;
            out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    out
}

/// 按视图状态变换：`rot` 次顺时针 90°，再（对旋转结果）水平镜像。
///
/// **一次成图**：只分配一份目标缓冲，从源直接搬。旧写法是"先 `to_vec()`，再每转 90°
/// 换一份新缓冲"，于是 40MP 的图按一下 R 的瞬时峰值是 3–4 份 160 MB（base + 上一档
/// 上屏位图 + 中转 + 目标），`rot=2` 还要中转两次（2026-10-18 第四批审计）。
/// 恒等时同样产出一份新缓冲——上屏层要独占它，借用源不行。
///
/// 搬运按**像素（u32）**而不是按字节：转置的写地址仍跨行，但拷的是 4 字节元素，
/// 编译器才有机会合并成一条 32 位读写。`rot=1/3` 再切成 64×64 的块，让每次跨行
/// 写落在同一小片目标行上（m72 实测：36.7MP 185→64 ms，100MP 592→193 ms）。
/// `rot=0/2` 全是整行/整行倒序，直接 `copy_from_slice` 或逐行倒着写。
///
/// 源长度与 `w×h×4` 不符、或任一端没 4 字节对齐时退回逐像素的保守路径，缺的那部分
/// 留透明黑（不 panic：这是 UI 线程，一次 panic 就是窗口消失）。正常调用方
/// （`derive_level`、鸟瞰缩略）长度恒等、两块都来自分配器，走快路。
pub fn transformed(rgba: &[u8], w: u32, h: u32, rot: u8, flip: bool) -> (u32, u32, Vec<u8>) {
    let r = rot % 4;
    let (wu, hu) = (w as usize, h as usize);
    let (dw, dh) = if r % 2 == 1 { (hu, wu) } else { (wu, hu) };
    let mut out = vec![0u8; dw * dh * 4];
    if wu == 0 || hu == 0 {
        return (dw as u32, dh as u32, out);
    }
    // 不能只看 base 指针：源可能是某块大缓冲的子切片。
    let aligned =
        (rgba.as_ptr() as usize).is_multiple_of(4) && (out.as_mut_ptr() as usize).is_multiple_of(4);
    if !aligned || rgba.len() < wu * hu * 4 {
        transformed_bytewise(rgba, w, h, r, flip, dw, &mut out);
        return (dw as u32, dh as u32, out);
    }
    // SAFETY: 两个指针都按 4 字节对齐（上方已判），长度各是 4 的整数倍且切出的元素数
    // 与缓冲一一对应（源 `wu*hu*4`、目标 `dw*dh*4`）；`out` 刚分配、没有别的借用。
    let src = unsafe { std::slice::from_raw_parts(rgba.as_ptr() as *const u32, wu * hu) };
    let dst = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u32, dw * dh) };
    const TILE: usize = 64;
    match (r, flip) {
        (0, false) => dst.copy_from_slice(src),
        // 180° + 镜像 = 行序与行内都反过来，整行对拷即可。
        (2, true) => {
            for sy in 0..hu {
                dst[(hu - 1 - sy) * wu..(hu - sy) * wu]
                    .copy_from_slice(&src[sy * wu..(sy + 1) * wu]);
            }
        }
        // 180° / 水平镜像：行序或行内二者只有一个反，逐行倒着写。
        (2, false) | (0, true) => {
            for sy in 0..hu {
                let dy = if r == 2 { hu - 1 - sy } else { sy };
                let s = &src[sy * wu..(sy + 1) * wu];
                let d = &mut dst[dy * wu..(dy + 1) * wu];
                for sx in 0..wu {
                    d[sx] = s[wu - 1 - sx];
                }
            }
        }
        // 90° / 270°：转置，写地址跨行 ⇒ 分块。
        (1, fl) => {
            for ty in (0..hu).step_by(TILE) {
                for tx in (0..wu).step_by(TILE) {
                    let y1 = (ty + TILE).min(hu);
                    let x1 = (tx + TILE).min(wu);
                    for sy in ty..y1 {
                        let dx = if fl { sy } else { hu - 1 - sy };
                        for sx in tx..x1 {
                            dst[sx * dw + dx] = src[sy * wu + sx];
                        }
                    }
                }
            }
        }
        (3, fl) => {
            for ty in (0..hu).step_by(TILE) {
                for tx in (0..wu).step_by(TILE) {
                    let y1 = (ty + TILE).min(hu);
                    let x1 = (tx + TILE).min(wu);
                    for sy in ty..y1 {
                        let dx = if fl { hu - 1 - sy } else { sy };
                        for sx in tx..x1 {
                            dst[(wu - 1 - sx) * dw + dx] = src[sy * wu + sx];
                        }
                    }
                }
            }
        }
        // r 已由 `rot % 4` 落到 0..=3，这支只是让匹配穷尽。
        _ => {}
    }
    (dw as u32, dh as u32, out)
}

/// [`transformed`] 的**逆**：给同一份像素再转一次，回到转之前的样子。
///
/// 存在的理由只有一条：编辑/分析型插件拿到的是"屏上朝向"那一份（契约见
/// `docs/扩展体系.md`），回传也按那个朝向，宿主落地前必须换算回源朝向——而用的**必须是
/// 发送那一刻冻结的朝向**，不是此刻的 `rot/flip`（用户在插件窗口里待着时，主窗口可能又被转过）。
///
/// D4 里只有 90° 与 270° 互逆，其余六个（恒等、三个 180°/镜像组合、两次对角转置）都是自逆。
/// 这条性质不许靠人推：同文件的用例 [`transformed_then_inverted_is_identity`] 拿**生产内核**
/// 在 3×5（宽高不等，才能抓到维度没换的错）上逐种验一遍。
///
/// [`transformed_then_inverted_is_identity`]: self::transformed_then_inverted_is_identity
pub fn inverted(rot: u8, flip: bool) -> (u8, bool) {
    match (rot % 4, flip) {
        (1, false) => (3, false),
        (3, false) => (1, false),
        (r, f) => (r, f),
    }
}

/// [`transformed`] 的保守路径：逐像素、允许源比 `w×h×4` 短（缺的留零）。
fn transformed_bytewise(rgba: &[u8], w: u32, h: u32, r: u8, flip: bool, dw: usize, out: &mut [u8]) {
    for sy in 0..h as usize {
        for sx in 0..w as usize {
            let Some(px) = rgba.get((sy * w as usize + sx) * 4..(sy * w as usize + sx) * 4 + 4)
            else {
                continue;
            };
            // 顺时针 r×90° 的**正向**映射（源→目标），所以一遍写完、不需要中转。
            let (dx, dy) = match r {
                0 => (sx, sy),
                1 => (h as usize - 1 - sy, sx),
                2 => (w as usize - 1 - sx, h as usize - 1 - sy),
                _ => (sy, w as usize - 1 - sx),
            };
            // 镜像在旋转**之后**，故作用在目标坐标系上。
            let dx = if flip { dw - 1 - dx } else { dx };
            let di = (dy * dw + dx) * 4;
            out[di..di + 4].copy_from_slice(px);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×3 非对称图案（R 通道 1..6）：方向矩阵逐个手算锁定。输入
    /// ```text
    /// 1 2
    /// 3 4
    /// 5 6
    /// ```
    fn pattern() -> (Vec<u8>, u32, u32) {
        let px: Vec<u8> = [1u8, 2, 3, 4, 5, 6]
            .iter()
            .flat_map(|&v| [v, 0, 0, 255])
            .collect();
        (px, 2, 3)
    }

    /// 把输出按 R 通道还原成数字网格断言。
    fn grid(rgba: &[u8], w: u32, h: u32) -> Vec<Vec<u8>> {
        (0..h)
            .map(|y| (0..w).map(|x| rgba[((y * w + x) * 4) as usize]).collect())
            .collect()
    }

    /// EXIF 方向 2/3/4（不换轴）：镜像与 180°。
    #[test]
    fn orientation_flip_and_180() {
        let (px, w, h) = pattern();
        let (o2, w2, h2) = oriented(px.clone(), w, h, 2);
        assert_eq!((w2, h2), (2, 3));
        assert_eq!(grid(&o2, w2, h2), vec![vec![2, 1], vec![4, 3], vec![6, 5]]);

        let (o3, _, _) = oriented(px.clone(), w, h, 3);
        assert_eq!(grid(&o3, 2, 3), vec![vec![6, 5], vec![4, 3], vec![2, 1]]);

        let (o4, _, _) = oriented(px, w, h, 4);
        assert_eq!(grid(&o4, 2, 3), vec![vec![5, 6], vec![3, 4], vec![1, 2]]);
    }

    /// EXIF 方向 5–8（换轴）：输出宽高必须互换。
    #[test]
    fn orientation_transpose_and_rotations() {
        let (px, w, h) = pattern();
        let (o5, w5, h5) = oriented(px.clone(), w, h, 5);
        assert_eq!((w5, h5), (3, 2), "5–8 必须交换宽高");
        assert_eq!(grid(&o5, w5, h5), vec![vec![1, 3, 5], vec![2, 4, 6]]);

        // 6 = 顺时针 90°：原左上 (1) 到输出右上。
        let (o6, _, _) = oriented(px.clone(), w, h, 6);
        assert_eq!(grid(&o6, 3, 2), vec![vec![5, 3, 1], vec![6, 4, 2]]);

        let (o7, _, _) = oriented(px.clone(), w, h, 7);
        assert_eq!(grid(&o7, 3, 2), vec![vec![6, 4, 2], vec![5, 3, 1]]);

        // 8 = 顺时针 270°（逆时针 90°）。
        let (o8, _, _) = oriented(px, w, h, 8);
        assert_eq!(grid(&o8, 3, 2), vec![vec![2, 4, 6], vec![1, 3, 5]]);
    }

    /// 方向 1、0、9 都原样返回：不猜，宁可躺平。
    #[test]
    fn orientation_passthrough_for_trivial_or_unknown() {
        let (px, w, h) = pattern();
        for o in [0, 1, 9, -1] {
            let (out, ow, oh) = oriented(px.clone(), w, h, o);
            assert_eq!((ow, oh), (w, h), "方向 {o} 不得换轴");
            assert_eq!(out, px, "方向 {o} 不得改像素");
        }
    }

    /// 方向矩阵必须自洽：任何方向连做四次恒等（5/7 两次即恒等）。
    #[test]
    fn orientation_is_involution_consistent() {
        let (px, w, h) = pattern();
        for o in [2, 3, 4, 5, 6, 7, 8] {
            let mut cur = (px.clone(), w, h);
            let times = if matches!(o, 5 | 7) { 2 } else { 4 };
            for _ in 0..times {
                cur = oriented(cur.0, cur.1, cur.2, o);
            }
            assert_eq!((cur.1, cur.2), (w, h), "方向 {o} ×{times} 应回原尺寸");
            assert_eq!(cur.0, px, "方向 {o} ×{times} 应回原像素");
        }
    }

    /// 缩放：只缩不放；16×16 缩到 8×8 时每格是 2×2 的平均。
    #[test]
    fn fit_dims_never_upscales() {
        assert_eq!(fit_dims(100, 50, 200, 200), (100, 50), "不放大");
        assert_eq!(fit_dims(400, 200, 100, 100), (100, 50), "等比缩到装得下");
        assert_eq!(fit_dims(400, 200, 0, 0), (400, 200), "0 = 不限");
        assert_eq!(fit_dims(1, 1000, 100, 100), (1, 100), "窄条也不炸成 0");
    }

    /// 面积平均：把 2×2 四个不同灰度缩成 1×1，应是四点均值。
    #[test]
    fn downscale_averages_area() {
        let rgba = vec![
            0, 0, 0, 255, //
            100, 100, 100, 255, //
            200, 200, 200, 255, //
            40, 40, 40, 255,
        ];
        let out = downscale(&rgba, 2, 2, 1, 1);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], ((100 + 200 + 40) / 4) as u8);
        assert_eq!(out[3], 255);
    }

    /// 参照实现（改动前的朴素四重循环，逐字照抄）：**加速不许改像素**。
    ///
    /// 两版求和顺序不同（新版竖向先合、再横向归约），但整数加法可交换，所以必须逐字节
    /// 相同。覆盖除不尽的比例——块边界与尾巴才是这种函数会错的地方。
    #[test]
    fn downscale_matches_naive_reference() {
        fn naive(rgba: &[u8], w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
            let mut out = vec![0u8; tw * th * 4];
            for dy in 0..th {
                let sy0 = dy * h / th;
                let sy1 = ((dy + 1) * h / th).max(sy0 + 1).min(h);
                for dx in 0..tw {
                    let sx0 = dx * w / tw;
                    let sx1 = ((dx + 1) * w / tw).max(sx0 + 1).min(w);
                    let mut acc = [0u32; 4];
                    let mut cnt = 0u32;
                    for sy in sy0..sy1 {
                        for sx in sx0..sx1 {
                            let si = (sy * w + sx) * 4;
                            for c in 0..4 {
                                acc[c] += rgba[si + c] as u32;
                            }
                            cnt += 1;
                        }
                    }
                    let oi = (dy * tw + dx) * 4;
                    for c in 0..4 {
                        out[oi + c] = (acc[c] / cnt.max(1)) as u8;
                    }
                }
            }
            out
        }
        // 确定性伪随机（与 `bench_downscale` 同一支 LCG，方便两边对表）。
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        for (w, h, tw, th) in [
            (8usize, 8usize, 3usize, 5usize),
            (7, 11, 2, 3),
            (13, 5, 4, 4),
            (16, 16, 1, 1),
            (5, 5, 5, 5),
            (9, 4, 2, 4),
            // 退路：一个输出行覆盖 300 行源 > SWAR_MAX_ROWS，必须走逐通道那版且仍同值。
            (300, 300, 7, 1),
            (257, 257, 257, 1),
            (256, 256, 1, 1),
        ] {
            let mut src = vec![0u8; w * h * 4];
            for px in src.chunks_exact_mut(4) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = (s >> 33) as u32;
                px[0] = r as u8;
                px[1] = (r >> 8) as u8;
                px[2] = (r >> 16) as u8;
                px[3] = ((r >> 24) as u8) | 1;
            }
            assert_eq!(
                downscale(&src, w as u32, h as u32, tw as u32, th as u32),
                naive(&src, w, h, tw, th),
                "{w}×{h}→{tw}×{th} 两版输出不一致"
            );
        }
    }

    /// 两个开关都为 false 时，通用版必须与 [`downscale`] 逐字节相同——这条只护"重构没改
    /// 现行降采样"，方向等值另开一条（下面）。
    #[test]
    fn downscale_anchored_flat_matches_downscale() {
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        for (w, h, tw, th) in [
            (13usize, 7usize, 5usize, 5usize),
            (17, 13, 7, 4),
            (31, 17, 13, 13),
            (9, 4, 2, 4),
            (5, 5, 5, 5),
            (256, 128, 1, 1),
        ] {
            let mut src = vec![0u8; w * h * 4];
            for px in src.chunks_exact_mut(4) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = (s >> 32) as u32;
                for (c, b) in px.iter_mut().enumerate() {
                    *b = ((r >> (8 * c as u32)) & 0xFF) as u8;
                }
            }
            assert_eq!(
                downscale_anchored(&src, w as u32, h as u32, tw as u32, th as u32, false, false),
                downscale(&src, w as u32, h as u32, tw as u32, th as u32),
                "{w}×{h}→{tw}×{th}：flat 版与现行版不一致"
            );
        }
    }

    /// ★ m67 的承重判据：**AVX2 竖向累加与标量累加必须逐字节相同**。
    ///
    /// 为什么这条能"证明"而不只是"看起来一样"：lane 分组没变——第 `i` 个 u32 累加的仍然
    /// 是同一列同一通道的那批字节；`_mm256_add_epi32` 与 Rust 的 `u32 +=` 同为模 2³² 回绕加。
    /// 所以这条用例断言的是"换写法不许改像素"，不是"两个近似实现碰巧一样"。
    ///
    /// 夹具特意选了**宽度为奇数**的档（`w=13` ⇒ `lanes=52`，整块 8 lane 走 6 次剩 4 lane 尾巴）
    /// 和**装不满一个向量**的档（`w=1` ⇒ `lanes=4` 全走尾巴）：尾巴分支不是边角料，
    /// 生产里最常见的大图宽度就是 6055 这种奇数——去掉尾巴循环，本条必须当场红（先破后立做过）。
    ///
    /// 本机探不到 AVX2 时两臂走的是同一条标量路，这条只能算哨兵 ⇒ 照实打一行说明。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn downscale_avx2_matches_scalar() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            eprintln!("本机没有 AVX2：两臂同路，这条只是哨兵，不构成等值证明");
        }
        let mut s = 0x6A09_E667_F3BC_C908u64;
        let cases: [(u32, u32, u32, u32); 9] = [
            (1, 4, 1, 2),     // lanes=4：一个向量都装不下，全走尾巴
            (2, 5, 1, 3),     // lanes=8：恰好一个向量
            (3, 9, 2, 3),     // lanes=12：一个向量 + 4 lane 尾巴
            (13, 7, 5, 5),    // lanes=52：六个向量 + 尾巴（奇数宽度）
            (16, 16, 4, 4),   // lanes=64：整除
            (17, 13, 7, 4),   // lanes=68：八个向量 + 尾巴
            (64, 48, 7, 9),   // 整除且多行同档
            (255, 3, 1, 2),   // 奇数宽度、单列输出
            (33, 33, 32, 32), // 几乎不缩（rows 小、tw 大）
        ];
        for (w, h, tw, th) in cases {
            let mut src = vec![0u8; (w * h * 4) as usize];
            for px in src.chunks_exact_mut(4) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = (s >> 32) as u32;
                for (c, b) in px.iter_mut().enumerate() {
                    *b = ((r >> (8 * c as u32)) & 0xFF) as u8;
                }
            }
            // 两个锚定方向都查：`fit_oriented` 用的正是远端锚定那一支。
            for (cx, cy) in [(false, false), (true, true), (false, true), (true, false)] {
                let scalar = downscale_anchored_impl(&src, w, h, tw, th, cx, cy, false);
                let vector = downscale_anchored_impl(&src, w, h, tw, th, cx, cy, true);
                assert_eq!(
                    scalar, vector,
                    "{w}×{h}→{tw}×{th} ceil=({cx},{cy})：AVX2 臂与标量臂不一致"
                );
                // 生产入口（自己判 CPU）必须与标量参照也一致——它才是"用户看到的那一版"。
                assert_eq!(
                    scalar,
                    downscale_anchored(&src, w, h, tw, th, cx, cy),
                    "{w}×{h}→{tw}×{th} ceil=({cx},{cy})：生产入口与标量参照不一致"
                );
            }
        }
    }

    /// ★ m49 的承重判据：**先按远端锚定的块划分降采样、再把小图转正**，必须与
    /// 「整张转完再缩」**逐字节相同**，8 个方向 × 非整数比例 × 换轴/不换轴全覆盖。
    ///
    /// 夹具特意选了缩放比除不尽的尺寸（`13×7→5×5` 这类）：整数比时两套块划分重合，
    /// 这条用例会在"相位其实不一样"的实现上照样绿——那正是 m49 第一版踩的坑。
    #[test]
    fn fit_oriented_matches_orient_then_fit() {
        let mut s = 0x1234_5678_9ABC_DEF0u64;
        let cases: [(u32, u32, u32, u32); 8] = [
            (13, 7, 5, 5),
            (17, 13, 7, 4),
            (31, 17, 13, 13),
            (23, 5, 9, 9),
            (11, 29, 4, 6),
            (37, 31, 12, 12),
            (64, 60, 7, 9),
            (100, 60, 0, 0), // 0 = 不限 ⇒ 走"装得下，只转不缩"那支
        ];
        for (w, h, mw, mh) in cases {
            let mut src = vec![0u8; (w * h * 4) as usize];
            for px in src.chunks_exact_mut(4) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = (s >> 29) as u32;
                for (c, b) in px.iter_mut().enumerate() {
                    *b = ((r >> (8 * c as u32)) & 0xFF) as u8;
                }
            }
            for o in 1..=8i32 {
                // 参照：生产现状（整图转正 → 缩到框）。
                let (r, rw, rh) = oriented(src.clone(), w, h, o);
                let reference = fit_to(r, rw, rh, mw, mh);
                let got = fit_oriented(src.clone(), w, h, o, mw, mh);
                assert_eq!(
                    (got.width, got.height),
                    (reference.width, reference.height),
                    "{w}×{h} 框 {mw}×{mh} 方向 {o}：尺寸就不一致"
                );
                assert_eq!(
                    got.rgba, reference.rgba,
                    "{w}×{h} 框 {mw}×{mh} 方向 {o}：像素与「先转再缩」不同 ⇒ 会改画面"
                );
            }
        }
    }

    /// 反向用例：朴素的「先缩再转」（两轴都从近端锚定）**不等价**——把它接上去会改像素。
    /// 这条存在的意义是证明上面那条夹具是**活的**：如果哪天有人觉得"两种顺序反正一样"，
    /// 这里会指出哪些方向、哪些比例真的不一样（`fit_oriented` 的注释引的就是这些数字）。
    #[test]
    fn naive_orient_after_fit_is_not_equivalent() {
        let src = {
            let mut s = 0x0BADC0DE_0BADC0DEu64;
            let mut v = vec![0u8; 31 * 17 * 4];
            for px in v.chunks_exact_mut(4) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = (s >> 30) as u32;
                for (c, b) in px.iter_mut().enumerate() {
                    *b = ((r >> (8 * c as u32)) & 0xFF) as u8;
                }
            }
            v
        };
        let (w, h, mw, mh) = (31u32, 17u32, 13u32, 13u32);
        let mut differ: Vec<i32> = Vec::new();
        for o in 2..=8i32 {
            let (r, rw, rh) = oriented(src.clone(), w, h, o);
            let reference = fit_to(r, rw, rh, mw, mh);
            let (tw, th) = if matches!(o, 5..=8) {
                (mh, mw)
            } else {
                (mw, mh)
            };
            let fitted = fit_to(src.clone(), w, h, tw, th);
            let (rgba, fw, fh) = oriented(fitted.rgba, fitted.width, fitted.height, o);
            if (fw, fh) != (reference.width, reference.height) || rgba != reference.rgba {
                differ.push(o);
            }
        }
        assert_eq!(
            differ,
            vec![2, 3, 4, 6, 7, 8],
            "只有方向 5（纯转置、无反向）碰巧相同；若这套夹具让别的方向也相同，说明夹具退化成整数比了"
        );
    }

    /// 每像素一个标记值（RGBA 四字节同值），断言只看 R 通道。
    fn make(pixels: &[u8]) -> Vec<u8> {
        pixels.iter().flat_map(|&v| [v, 0, 0, 255]).collect()
    }

    fn at(buf: &[u8], w: u32, x: u32, y: u32) -> u8 {
        buf[((y * w + x) * 4) as usize]
    }

    #[test]
    fn rotate_90cw_places_corners_right() {
        // 2×3 源图：      顺时针 90° 后应为 3×2：
        //   A B              E C A
        //   C D              F D B
        //   E F
        let (b, w, h) = rotate_cw_rgba(&make(&[1, 2, 3, 4, 5, 6]), 2, 3);
        assert_eq!((w, h), (3, 2));
        assert_eq!(at(&b, w, 0, 0), 5); // E
        assert_eq!(at(&b, w, 1, 0), 3); // C
        assert_eq!(at(&b, w, 2, 0), 1); // A
        assert_eq!(at(&b, w, 0, 1), 6); // F
        assert_eq!(at(&b, w, 1, 1), 4); // D
        assert_eq!(at(&b, w, 2, 1), 2); // B
    }

    #[test]
    fn flip_h_mirrors_each_row() {
        let b = flip_h_rgba(&make(&[1, 2, 3, 4]), 2, 2);
        assert_eq!(at(&b, 2, 0, 0), 2);
        assert_eq!(at(&b, 2, 1, 0), 1);
        assert_eq!(at(&b, 2, 0, 1), 4);
        assert_eq!(at(&b, 2, 1, 1), 3);
    }

    #[test]
    fn rotate_four_times_returns_to_start() {
        let src = make(&[1, 2, 3, 4, 5, 6]);
        let (mut w, mut h, mut b) = transformed(&src, 2, 3, 0, false);
        for _ in 0..4 {
            let (nb, nw, nh) = rotate_cw_rgba(&b, w, h);
            b = nb;
            w = nw;
            h = nh;
        }
        assert_eq!((w, h), (2, 3));
        assert_eq!(b, src);
    }

    #[test]
    fn transformed_identity_and_swap() {
        let src = make(&[1, 2, 3, 4, 5, 6]);
        let (w, h, b) = transformed(&src, 2, 3, 0, false);
        assert_eq!((w, h), (2, 3));
        assert_eq!(b, src);
        // 旋转 180°：中心对称。
        let (w, h, b) = transformed(&src, 2, 3, 2, false);
        assert_eq!((w, h), (2, 3));
        assert_eq!(at(&b, w, 0, 0), 6);
        assert_eq!(at(&b, w, 1, 2), 1);
    }

    /// ★ 一次成图的 `transformed` 必须与"逐步中转"的参照实现**逐字节相同**。
    /// 正向映射的四个分支手写在 `match` 里，写反一个方向不会崩、也不会报错，
    /// 只会把画面转成另一个朝向——所以这里把 8 种朝向 × 两种长宽比全比一遍。
    #[test]
    fn transformed_matches_stepwise_reference() {
        for (src, w, h) in [
            (make(&[1, 2, 3, 4, 5, 6]), 2u32, 3u32),
            (make(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]), 4, 3),
        ] {
            for rot in 0..4u8 {
                for flip in [false, true] {
                    // 参照：先按 rot 次顺时针中转，再在旋转后的坐标系里镜像。
                    let (mut rw, mut rh, mut rb) = (w, h, src.clone());
                    for _ in 0..rot {
                        let (b, nw, nh) = rotate_cw_rgba(&rb, rw, rh);
                        (rw, rh, rb) = (nw, nh, b);
                    }
                    if flip {
                        rb = flip_h_rgba(&rb, rw, rh);
                    }
                    let (tw, th, tb) = transformed(&src, w, h, rot, flip);
                    assert_eq!((tw, th), (rw, rh), "rot={rot} flip={flip} 尺寸就不对");
                    assert_eq!(tb, rb, "rot={rot} flip={flip} 像素与参照不一致");
                }
            }
        }
    }

    /// ★ `inverted` 必须与**生产内核**互逆，且夹具要**宽高不等**（3×5）——只有非方阵
    /// 才抓得出"逆元写对像素顺序、却忘了换宽高"这一支错（方图上它一路绿，把用户看到的
    /// 图存成躺着的还不报错）。八种朝向全比，外加"逆的逆 = 原"。
    #[test]
    fn transformed_then_inverted_is_identity() {
        let (w, h) = (3u32, 5u32);
        let src: Vec<u8> = (0..(w * h) as usize)
            .flat_map(|i| [i as u8, (i / 4) as u8, 200, 255])
            .collect();
        for rot in 0..4u8 {
            for flip in [false, true] {
                let inv = inverted(rot, flip);
                assert_eq!(
                    inverted(inv.0, inv.1),
                    (rot, flip),
                    "({rot},{flip}) 的逆不自洽"
                );
                let (tw, th, turned) = transformed(&src, w, h, rot, flip);
                let (iw, ih, back) = transformed(&turned, tw, th, inv.0, inv.1);
                assert_eq!((iw, ih), (w, h), "rot={rot} flip={flip} 尺寸没还原");
                assert_eq!(back, src, "rot={rot} flip={flip} 像素没还原");
            }
        }
    }

    /// ★ 分块（64×64）的边界与对齐判据：65×67 跨块、未对齐的源必须与参照一字不差。
    /// 块写漏一行尾、`dx` 少算一块这类错在小图上照不出来。
    #[test]
    fn transformed_tiles_and_unaligned_source_match() {
        let (w, h) = (65u32, 67u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 37 % 251) as u8).collect();
        for rot in 0..4u8 {
            for flip in [false, true] {
                let (mut rw, mut rh, mut rb) = (w, h, px.clone());
                for _ in 0..rot {
                    let (b, nw, nh) = rotate_cw_rgba(&rb, rw, rh);
                    (rw, rh, rb) = (nw, nh, b);
                }
                if flip {
                    rb = flip_h_rgba(&rb, rw, rh);
                }
                let (tw, th, tb) = transformed(&px, w, h, rot, flip);
                assert_eq!((tw, th), (rw, rh), "rot={rot} flip={flip} 尺寸不对");
                assert_eq!(tb, rb, "rot={rot} flip={flip} 跨块像素不一致");

                // 同一批字节挪到奇数地址：必须走保守路径，且结果不变。
                let mut shifted = vec![0u8; px.len() + 1];
                shifted[1..].copy_from_slice(&px);
                let unaligned = &shifted[1..];
                assert!(
                    !(unaligned.as_ptr() as usize).is_multiple_of(4),
                    "夹具没落在未对齐地址上，这条断言等于没测"
                );
                let (uw, uh, ub) = transformed(unaligned, w, h, rot, flip);
                assert_eq!((uw, uh, ub), (tw, th, tb), "未对齐源 rot={rot} flip={flip}");
            }
        }
    }

    /// 源长度与 `w×h×4` 不符（内部账算错时的最坏情形）：不许 panic，缺的留透明黑。
    #[test]
    fn transformed_with_short_source_degrades_without_panicking() {
        let short = make(&[1, 2]);
        let (w, h, b) = transformed(&short, 2, 2, 0, false);
        assert_eq!((w, h), (2, 2));
        assert_eq!(b.len(), 16);
        assert_eq!(at(&b, w, 0, 0), 1);
        assert_eq!(at(&b, w, 1, 1), 0, "没数据的那格该是透明黑，而不是越界");
    }
}

// ── 整数矩形（框架无关）─────────────────────────────────────────

/// 整数像素矩形（逻辑坐标/目标矩形）。与 windui 的 `Rect` 同构（同为 x/y/w/h: i32），
/// 上屏层做 1:1 转换——core 不认识任何 UI 框架类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
}
