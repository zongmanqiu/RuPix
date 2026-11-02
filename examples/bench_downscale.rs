//! 宿主面积平均降采样的**同进程配对 A/B**（m37）。
//!
//! 为什么量这个函数：它是基座侧、与格式无关的一笔钱，三条路都走它——
//! ① 解码后的 `fit_to`（首屏空白期里的 `post` 段，实测各格式 115–200 ms）；
//! ② 缩放换档的本地派生（`core::derive` / `viewer` 的派生线程，实测一档 118–131 ms）；
//! ③ 鸟瞰小图。解码器能不能原生降采样都改变不了它——解码器不给档位，宿主就得自己缩。
//!
//! 两臂只差实现，不差环境：A 臂是**朴素四重循环**（改动前逐字照抄的参照实现），
//! B 臂是生产 `core::geometry::downscale`。同一 rep 内相邻执行 → 环境漂移对两臂同向；
//! 并且**逐字节比对两臂输出**，加速不许改像素。
//!
//! 用法（从 `main` 目录）：
//! ```text
//! cargo run --release --example bench_downscale
//! RUPIX_BENCH_ROUNDS=21 …                                               # 要更稳的尾部就加轮数
//! RUPIX_BENCH_REVERSE=1 …                                               # 翻臂序复跑，判 tails
//! ```
//!
//! ★ m65 追加**候选臂 C/D**（扁平索引 u32 / 扁平索引 u16）：`post_leg_probe` 量出生产
//!   `fit_to` 是"把同一份缓冲摸一遍"的 8–12 倍 ⇒ 它不是带宽受限，是那段逐像素累加没被
//!   向量化。两条候选臂与生产**逐字节相同**才继续谈加速（不一致当场 panic）。

use std::time::Instant;

use rupix_lib::core::geometry::{downscale, fit_dims};

/// A 臂：朴素实现（改动前的生产代码，逐字照抄当参照）。
///
/// ★ 这份拷贝**只服务于本探针**：判"加速有没有改像素"必须拿旧行为比，而旧行为在
/// 新实现里没有别的落脚点。生产码里那份参照实现见 `geometry.rs` 的同名测试。
fn naive(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    for dy in 0..th as usize {
        let sy0 = dy * h as usize / th as usize;
        let sy1 = (((dy + 1) * h as usize / th as usize).max(sy0 + 1)).min(h as usize);
        for dx in 0..tw as usize {
            let sx0 = dx * w as usize / tw as usize;
            let sx1 = (((dx + 1) * w as usize / tw as usize).max(sx0 + 1)).min(w as usize);
            let mut acc = [0u32; 4];
            let mut cnt = 0u32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let si = (sy * w as usize + sx) * 4;
                    for c in 0..4 {
                        acc[c] += rgba[si + c] as u32;
                    }
                    cnt += 1;
                }
            }
            let oi = (dy * tw as usize + dx) * 4;
            for c in 0..4 {
                out[oi + c] = (acc[c] / cnt.max(1)) as u8;
            }
        }
    }
    out
}

/// 确定性伪随机 RGBA（LCG）：两臂吃的是同一份字节，且不占 145 MB 的构造时间。
fn make(w: u32, h: u32) -> Vec<u8> {
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let mut v = vec![0u8; (w as usize) * (h as usize) * 4];
    for px in v.chunks_exact_mut(4) {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (s >> 33) as u32;
        px[0] = r as u8;
        px[1] = (r >> 8) as u8;
        px[2] = (r >> 16) as u8;
        // alpha 也铺不同值：面积平均对 alpha 与对颜色走的是同一条累加路。
        px[3] = ((r >> 24) as u8) | 1;
    }
    v
}

fn med(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[s.len() / 2]
}

/// 与 `core::geometry::band` 逐字一致的复刻（该函数是私有的，example 拿不到）。
/// 改 `geometry.rs::band` 时**必须同步**——否则候选臂测的是另一种划分。
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

/// 候选臂 C：与生产 `downscale_anchored`（(false,false)）**同一套划分与同一条算术**，
/// 只把竖向累加从"两份 `chunks_exact(4)` zip 起来逐像素加四个 lane"
/// 改成**扁平索引循环** `for i in 0..w*4 { col[i] += row[i] as u32 }`。
/// 为什么要试：u32 整数加法满足结合律与交换律（档位面积远不到溢出），
/// 所以"换写法不改结果"是能**证明**的，不只是"看起来一样"。
fn cand_u32(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let (w, h, tw, th) = (w as usize, h as usize, tw as usize, th as usize);
    let mut out = vec![0u8; tw * th * 4];
    let mut col = vec![0u32; w * 4];
    let lanes = w * 4;
    for dy in 0..th {
        let (sy0, sy1) = band(dy, h, th, false);
        col.fill(0);
        for sy in sy0..sy1 {
            let row = &rgba[sy * lanes..(sy + 1) * lanes];
            for i in 0..lanes {
                col[i] += row[i] as u32;
            }
        }
        let rows = (sy1 - sy0) as u32;
        let o = dy * tw * 4;
        for dx in 0..tw {
            let (sx0, sx1) = band(dx, w, tw, false);
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

/// 候选臂 D：竖向累加换成 **u16** lane（`col` 一半大 ⇒ 每指令处理的 lane 翻倍、
/// 缓存行减半）。前提是这一档的 `rows × 255 ≤ 65535`，即 `rows ≤ 257`；
/// 超了必须退回 u32 那条路（这里直接 assert——生产接线要带这条闸）。
fn cand_u16(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let (w, h, tw, th) = (w as usize, h as usize, tw as usize, th as usize);
    let mut out = vec![0u8; tw * th * 4];
    let mut col = vec![0u16; w * 4];
    let lanes = w * 4;
    for dy in 0..th {
        let (sy0, sy1) = band(dy, h, th, false);
        let rows = (sy1 - sy0) as u32;
        assert!(rows <= 257, "u16 臂撑不住 {rows} 行×255，会溢出");
        col.fill(0);
        for sy in sy0..sy1 {
            let row = &rgba[sy * lanes..(sy + 1) * lanes];
            for i in 0..lanes {
                col[i] += row[i] as u16;
            }
        }
        let o = dy * tw * 4;
        for dx in 0..tw {
            let (sx0, sx1) = band(dx, w, tw, false);
            let mut acc = [0u32; 4];
            for p in col[sx0 * 4..sx1 * 4].chunks_exact(4) {
                acc[0] += p[0] as u32;
                acc[1] += p[1] as u32;
                acc[2] += p[2] as u32;
                acc[3] += p[3] as u32;
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

/// 候选臂 v3（m67）：**AVX2 竖向累加**。m66 把两条"纯安全写法"臂判废之后，
/// `§18` ⑪ 剩下的唯一杠杆就是向量化，而"5–6 倍"当时只是推算 ⇒ 这一轮把它**量出来**，
/// 量不到就判废，绝不把没数的 `unsafe` 放进 layer 0。
///
/// ★ 为什么这两条臂可以**断言逐字节相同**（不是"看起来一样"）：
/// - lane 分组不变：每个 u32/u16 lane 累加的仍然是**同一批**源字节（同一列同一通道），
///   SIMD 只是把 8/16 个 lane 的合成一次做；
/// - `_mm256_add_epi32` 是模 2³² 的回绕加，与 Rust release 里 `u32 +=` 完全同语义，
///   所以连"极端档位会不会溢出"这个既有行为都被原样保留（不是新引入的差异）；
/// - 除法、`band` 划分、`cnt` 一律照抄生产。
///
/// 不一致就当场 panic——这是这条线唯一的安全网。
///
/// G 臂：u32 lane（与生产同宽，**无前提**）。
/// H 臂：u16 lane（`col` 减半 ⇒ 每指令 16 lane、缓存流量每 lane 从 9 B 降到 5 B），
/// 前提是这一档 `rows × 255 ≤ 65535`（`rows ≤ 257`）；超了必须退回 G 那条路。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn vertical_avx2(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32, narrow: bool) -> Vec<u8> {
    use std::arch::x86_64::{
        __m128i, __m256i, _mm256_add_epi16, _mm256_add_epi32, _mm256_cvtepu8_epi16,
        _mm256_cvtepu8_epi32, _mm256_loadu_si256, _mm256_storeu_si256, _mm_loadl_epi64,
        _mm_loadu_si128,
    };
    let (w, h, tw, th) = (w as usize, h as usize, tw as usize, th as usize);
    let lanes = w * 4;
    let mut out = vec![0u8; tw * th * 4];
    if narrow {
        let mut col = vec![0u16; lanes];
        for dy in 0..th {
            let (sy0, sy1) = band(dy, h, th, false);
            let rows = (sy1 - sy0) as u32;
            assert!(
                rows <= 257,
                "H 臂撑不住 {rows} 行×255，会溢出（该退回 G 臂）"
            );
            col.fill(0);
            for sy in sy0..sy1 {
                let row = &rgba[sy * lanes..(sy + 1) * lanes];
                let mut i = 0usize;
                // 一次 16 个 lane：16 字节零扩展成 16 个 u16，与 col 对应 16 lane 相加。
                while i + 16 <= lanes {
                    let bytes = _mm_loadu_si128(row.as_ptr().add(i) as *const __m128i);
                    let wide = _mm256_cvtepu8_epi16(bytes);
                    let accp = col.as_mut_ptr().add(i) as *mut __m256i;
                    let sum = _mm256_add_epi16(_mm256_loadu_si256(accp), wide);
                    _mm256_storeu_si256(accp, sum);
                    i += 16;
                }
                // 尾巴（`lanes % 16 != 0` 的那些宽度）：与标量臂逐字相同的写法。
                while i < lanes {
                    col[i] += row[i] as u16;
                    i += 1;
                }
            }
            horizontal_into(&mut out, dy, tw, th, rows, &col);
        }
    } else {
        let mut col = vec![0u32; lanes];
        for dy in 0..th {
            let (sy0, sy1) = band(dy, h, th, false);
            let rows = (sy1 - sy0) as u32;
            col.fill(0);
            for sy in sy0..sy1 {
                let row = &rgba[sy * lanes..(sy + 1) * lanes];
                let mut i = 0usize;
                // 一次 8 个 lane（= 2 像素）：8 字节零扩展成 8 个 u32。
                while i + 8 <= lanes {
                    let bytes = _mm_loadl_epi64(row.as_ptr().add(i) as *const __m128i);
                    let wide = _mm256_cvtepu8_epi32(bytes);
                    let accp = col.as_mut_ptr().add(i) as *mut __m256i;
                    let sum = _mm256_add_epi32(_mm256_loadu_si256(accp), wide);
                    _mm256_storeu_si256(accp, sum);
                    i += 8;
                }
                while i < lanes {
                    col[i] += row[i] as u32;
                    i += 1;
                }
            }
            horizontal_into(&mut out, dy, tw, th, rows, &col);
        }
    }
    out
}

/// 横向归并 + 除法那一段，**逐字照抄生产**（`downscale_anchored` 的后半）。
/// 泛型在 lane 宽度上：`col` 是 u32 时 `p[c]` 就是 u32，是 u16 时先加宽——
/// 与生产 / `cand_u16` 完全同式。
fn horizontal_into<T: Copy + Into<u32>>(
    out: &mut [u8],
    dy: usize,
    tw: usize,
    _th: usize,
    rows: u32,
    col: &[T],
) {
    let o = dy * tw * 4;
    for dx in 0..tw {
        let (sx0, sx1) = band(dx, col.len() / 4, tw, false);
        let mut acc = [0u32; 4];
        for p in col[sx0 * 4..sx1 * 4].chunks_exact(4) {
            acc[0] += p[0].into();
            acc[1] += p[1].into();
            acc[2] += p[2].into();
            acc[3] += p[3].into();
        }
        let cnt = rows * (sx1 - sx0) as u32;
        let d = &mut out[o + dx * 4..o + dx * 4 + 4];
        for c in 0..4 {
            d[c] = (acc[c] / cnt) as u8;
        }
    }
}

/// G/H 两臂的安全外壳：本机没有 AVX2 时**照实回退到 C 臂的标量写法**，
/// 并在 main 里打一行"G 臂退回标量，读数不算数"——不许拿回退臂当加速证据。
fn cand_avx2_u32(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        // 调用点负责运行期判据：`vertical_avx2` 只在探测到 AVX2 后从这里进。
        return unsafe { vertical_avx2(rgba, w, h, tw, th, false) };
    }
    cand_u32(rgba, w, h, tw, th)
}

fn cand_avx2_u16(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        return unsafe { vertical_avx2(rgba, w, h, tw, th, true) };
    }
    cand_u16(rgba, w, h, tw, th)
}

/// B0 臂：**m67 落地之前的生产写法**（竖向逐 lane 标量累加 + 同一套横向归并/除法）。
/// 留着它只为一件事：让"AVX2 到底值多少"在**同一进程、同一份源、正反两序**里比出来。
/// 跨轮比历史读数不算证据（本机循环探针噪声带 ±20%，而 B 自己在两轮之间能漂 119→152 ms）。
/// 同时它也是"落地真的开火了吗"的判据：若 B 贴在 B0 而不是贴在 G，说明生产没走到 SIMD。
fn old_prod_scalar(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let (w, h, tw, th) = (w as usize, h as usize, tw as usize, th as usize);
    let lanes = w * 4;
    let mut out = vec![0u8; tw * th * 4];
    let mut col = vec![0u32; lanes];
    for dy in 0..th {
        let (sy0, sy1) = band(dy, h, th, false);
        col.fill(0);
        for sy in sy0..sy1 {
            let row = &rgba[sy * lanes..(sy + 1) * lanes];
            for (acc, px) in col.iter_mut().zip(row) {
                *acc += *px as u32;
            }
        }
        horizontal_into(&mut out, dy, tw, th, (sy1 - sy0) as u32, &col);
    }
    out
}

/// 诊断臂 E/F：**拆开看钱花在哪一段**，不产出可用图（E 只累加不平均，F 只横向不纵向）。
/// 为什么要拆：C（扁平写法）与 D（u16，内存流量减半）都只落在 0.9–1.1× 的噪声带里，
/// 说明瓶颈既不是"编译器没向量化写法"也不是 col 缓冲的流量，得看两段各自多少。
/// 计时口径与 A/B/C/D 完全同（同一份源、同一套 band 划分、同一趟 rep）。
fn vertical_only(rgba: &[u8], w: u32, h: u32, _tw: u32, th: u32) -> u64 {
    let (w, h, th) = (w as usize, h as usize, th as usize);
    let lanes = w * 4;
    let mut col = vec![0u32; lanes];
    let mut sink = 0u64;
    for dy in 0..th {
        let (sy0, sy1) = band(dy, h, th, false);
        col.fill(0);
        for sy in sy0..sy1 {
            let row = &rgba[sy * lanes..(sy + 1) * lanes];
            for i in 0..lanes {
                col[i] += row[i] as u32;
            }
        }
        sink = sink.wrapping_add(col[0] as u64 + col[lanes - 1] as u64);
    }
    sink
}

/// F 臂：给定一份已经竖向累加好的 `col`，只做横向归并 + 除法那一段。
/// 这是"横向段单独值多少钱"的量法（生产里每档位行做一次，这里同样做 `th` 次）。
fn horizontal_only(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let (w, h, tw, th) = (w as usize, h as usize, tw as usize, th as usize);
    let lanes = w * 4;
    let mut out = vec![0u8; tw * th * 4];
    // 先把一份"真的"col 造出来（这一段不计进 F，计进 E 的差值里）。
    let mut col = vec![0u32; lanes];
    for sy in 0..h.min(4) {
        let row = &rgba[sy * lanes..(sy + 1) * lanes];
        for i in 0..lanes {
            col[i] += row[i] as u32;
        }
    }
    let rows = 4u32;
    for dy in 0..th {
        let o = dy * tw * 4;
        for dx in 0..tw {
            let (sx0, sx1) = band(dx, w, tw, false);
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

fn main() {
    let rounds: usize = std::env::var("RUPIX_BENCH_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7)
        .max(3);
    let reverse = std::env::var_os("RUPIX_BENCH_REVERSE").is_some();

    // 三档真实需求：首屏 fit（视口 2560×1440 与缩略图档 240×180）+ 一次换档派生。
    // 目标尺寸一律由 `fit_dims` 现算，探针不许自己发明档位。
    let cases: [(u32, u32, u32, u32); 4] = [
        (6055, 6055, 2560, 1440),
        (6055, 6055, 1280, 960),
        (5239, 5239, 2560, 1440),
        (6055, 6055, 240, 180),
    ];
    println!("轮数 {rounds}（中位数）reverse={reverse}；A=朴素四重循环  B=生产 downscale");
    println!(
        "{:<22} {:>13} {:>11} {:>11} {:>8}",
        "源 → 目标", "A 朴素(ms)", "B 生产(ms)", "加速", "B MP源/s"
    );
    for &(w, h, bw, bh) in &cases {
        let (tw, th) = fit_dims(w, h, bw, bh);
        let src = make(w, h);
        let a = naive(&src, w, h, tw, th);
        let b = downscale(&src, w, h, tw, th);
        assert_eq!(a, b, "{w}×{h}→{tw}×{th}：两臂输出不一致，加速改了像素");
        let (mut va, mut vb) = (Vec::new(), Vec::new());
        for r in 0..rounds {
            let order = if reverse || r % 2 == 1 {
                ["b", "a"]
            } else {
                ["a", "b"]
            };
            for arm in order {
                let t = Instant::now();
                let out = if arm == "a" {
                    naive(&src, w, h, tw, th)
                } else {
                    downscale(&src, w, h, tw, th)
                };
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(&out);
                if arm == "a" {
                    va.push(ms);
                } else {
                    vb.push(ms);
                }
                println!(
                    "  rep {r:>2} arm {arm}  {ms:>8.1} ms   源 {:.1} MP → {tw}×{th}",
                    (w as f64 * h as f64) / 1e6
                );
            }
        }
        let (ma, mb) = (med(&va), med(&vb));
        let tag = format!("{w}×{h}→{tw}×{th}");
        let mps = (w as f64 * h as f64 / 1e6) / (mb / 1000.0);
        println!(
            "{tag:<22} {ma:>13.1} {mb:>11.1} {:>10.2}× {mps:>11.0}",
            ma / mb,
        );
    }

    // ── 候选臂 v2（m66 重做）：上一轮把三条臂**交替插在同一 rep 里**跑，B 自己在两轮之间
    //    能从 119 ms 漂到 152 ms ⇒ 比值 0.58–1.10× 里分不清"臂差"与"环境漂"。
    //    **那是证据不足，不是"没有收益"**，这一版把比臂做干净：
    //      ① 每条臂跑**自己的循环**（不与别的臂共用缓存/频率状态）；
    //      ② 报 **min 与 median**——这是确定性 CPU 循环，min 是最干净的读数，median 只作对照；
    //      ③ 臂序整体**反序再跑一遍**，两遍同向才算数（K 系老规矩：尾统计要靠反序复跑）。
    //    输出仍先**逐字节断言与生产一致**：整数加法可交换，换写法不许改像素，不一致当场 panic。
    //    m67 之后基准换成 **B0（落地前的生产写法）**：AVX2 已经进了 `downscale`，
    //    "B 生产"这一臂本身就是被测对象，再拿它当基准等于跟自己比。
    println!();
    println!("候选臂 v3：分节跑 + min/median + 反序复跑（先逐字节相同，再谈加速）");
    println!(
        "  AVX2 探测：is_x86_feature_detected!(\"avx2\") = {}",
        std::is_x86_feature_detected!("avx2")
    );
    type Arm = fn(&[u8], u32, u32, u32, u32) -> Vec<u8>;
    let arm_fns: [(&str, Arm); 6] = [
        ("B0 标量(旧)", old_prod_scalar),
        ("B 生产", downscale),
        ("C 扁平u32", cand_u32),
        ("D 扁平u16", cand_u16),
        ("G avx2 u32", cand_avx2_u32),
        ("H avx2 u16", cand_avx2_u16),
    ];
    let n = arm_fns.len();
    // 两遍（正序 / 反序）各取每臂每档的 min，**两遍同向才算数**。
    for pass in 0..2 {
        let ord: Vec<usize> = if pass == 0 {
            (0..n).collect()
        } else {
            (0..n).rev().collect()
        };
        println!(
            "── 第 {pass} 遍（臂序 {}）",
            ord.iter()
                .map(|&i| arm_fns[i].0)
                .collect::<Vec<_>>()
                .join(" → ")
        );
        println!(
            "{:<22} {:>11} {:>10} {:>11}",
            "源 → 目标", "臂", "min/med", "相对 B0(min)"
        );
        for &(w, h, bw, bh) in &cases {
            let (tw, th) = fit_dims(w, h, bw, bh);
            let src = make(w, h);
            let b_ref = downscale(&src, w, h, tw, th);
            // 先逐字节断言（C/D/G/H 全部），不一致当场 panic——这是这条线唯一的安全网。
            for (name, f) in arm_fns.iter().skip(1) {
                assert_eq!(b_ref, f(&src, w, h, tw, th), "{name} 臂改了像素");
            }
            let mut mn = vec![f64::INFINITY; n];
            let mut md = vec![0f64; n];
            for &ai in &ord {
                let f = arm_fns[ai].1;
                let mut v = Vec::new();
                for _ in 0..rounds {
                    let t = Instant::now();
                    let out = f(&src, w, h, tw, th);
                    v.push(t.elapsed().as_secs_f64() * 1000.0);
                    std::hint::black_box(&out);
                }
                mn[ai] = v.iter().cloned().fold(f64::INFINITY, f64::min);
                md[ai] = med(&v);
            }
            for ai in 0..n {
                println!(
                    "{:<22} {:>11} {:>7.1}/{:<5.1} {:>9}",
                    if ai == 0 {
                        format!("{w}×{h}→{tw}×{th}")
                    } else {
                        String::new()
                    },
                    arm_fns[ai].0,
                    mn[ai],
                    md[ai],
                    if ai == 0 {
                        "基准".to_string()
                    } else {
                        format!("{:.2}×", mn[0] / mn[ai])
                    }
                );
            }
        }
    }

    // 诊断臂：只纵向 ≈ 全部开销？只横向值多少？（定位钱在哪一段，不参与加速裁决）
    println!();
    println!("诊断臂（只纵向 / 只横向）");
    println!(
        "{:<22} {:>14} {:>14}",
        "源 → 目标", "E 只纵向 min", "F 只横向 min"
    );
    for &(w, h, bw, bh) in &cases {
        let (tw, th) = fit_dims(w, h, bw, bh);
        let src = make(w, h);
        let mut ve = Vec::new();
        let mut vf = Vec::new();
        for _ in 0..rounds {
            let t = Instant::now();
            std::hint::black_box(vertical_only(&src, w, h, tw, th));
            ve.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        for _ in 0..rounds {
            let t = Instant::now();
            std::hint::black_box(horizontal_only(&src, w, h, tw, th));
            vf.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "{:<22} {:>14.1} {:>14.1}",
            format!("{w}×{h}→{tw}×{th}"),
            ve.iter().cloned().fold(f64::INFINITY, f64::min),
            vf.iter().cloned().fold(f64::INFINITY, f64::min),
        );
    }
}
