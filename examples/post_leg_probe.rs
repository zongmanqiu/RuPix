//! `post` 段的分账探针（m65）：把「先转 RGBA8 再缩到视口」这一趟拆成两条腿各测一次。
//!
//! 为什么要拆：`[decode-trace]` 的 `post_ms` 是一个数（`decoders/mod.rs` 的 ③ 后处理段
//! ＝ `take_icc` + `to_rgba8` + `fit_oriented` + `apply_icc`），而 m65 要判的那条候选
//! （"把转格式与降采样融成一趟，省掉自然尺寸的 RGBA8 中转缓冲"）只吃得到其中两条腿。
//! 本探针把 **`fit_to` 那一腿单独量出来**，`to_rgba8 ≈ post − fit` 就是这么来的
//! （样本都不带 ICC：`take_icc` 只是 truncate、`apply_icc` 空转，所以剩下两条腿）。
//!
//! ★ 只量生产函数本身，不改生产代码：`fit_to` 是 `core::geometry` 的公开入口，
//!   方向 1（绝大多数图）走的正是它（`fit_oriented` 对 o∉2..=8 直接委托 `fit_to`）。
//!   输入是自然尺寸的 RGBA8 缓冲，与宿主在 `post` 段拿到的那份**同形同尺寸**。
//!
//! ★ 同时量一条**下限臂**：同一份缓冲"只是摸一遍字节"（标量求和，编译器自动向量化）。
//!   按 K87 的定式，`fit_to` 若已贴近这条下限，就说明它受的是内存带宽不是算法，
//!   "把平均做得更聪明"拿不到钱；差得多才谈得上有肉。
//!
//! 用法（从 `main` 目录）：
//! ```text
//! cargo run --release --example post_leg_probe
//! RUPIX_BENCH_VIEW=2560x1440 RUPIX_BENCH_ROUNDS=7 cargo run --release --example post_leg_probe
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};

use rupix_lib::core::geometry::{fit_dims, fit_to};

const ROUNDS: u32 = 5;

/// 与 `bench_first_paint` 同口径：取中位数，首轮页错误毛刺不进结论。
fn med(v: &[Duration]) -> Duration {
    let mut s = v.to_vec();
    s.sort();
    s[s.len() / 2]
}

fn main() {
    let view = std::env::var("RUPIX_BENCH_VIEW")
        .ok()
        .and_then(|s| {
            let (a, b) = s.split_once(['x', 'X'])?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((2560u32, 1440u32));
    let rounds = std::env::var("RUPIX_BENCH_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(ROUNDS);

    // 自然尺寸清单＝m65 首屏那批样本的尺寸（png/jpeg/tiff/bmp/gif 各自的原生宽高）。
    let sizes: [(u32, u32); 6] = [
        (6324, 6324), // jpeg_100M.jpg
        (6055, 6055), // tiff_100M.tif
        (5239, 5239), // png_100M.png
        (5120, 3840), // bmp/gif_gradient
        (3000, 2000), // bmp/gif_photo
        (2560, 1920), // 典型 corpus L 档
    ];

    println!("视口框 {view:?}，轮数 {rounds}（中位数）；`fit_to` 单独一趟");
    println!(
        "{:>12} {:>12} {:>12} {:>10} {:>12} {:>10} {:>9} {:>9} {:>7} {:>7}",
        "自然尺寸",
        "源 MB",
        "档位",
        "fit ms",
        "ns/源像素",
        "首轮 ms",
        "sum ms",
        "copy ms",
        "vs读",
        "vs写"
    );
    for (w, h) in sizes {
        let (fw, fh) = fit_dims(w, h, view.0, view.1);
        // 源缓冲在计时外造好并克隆：克隆的是"宿主已经拿到手的那一份"，
        // 计时里只留 `fit_to` 自己（含它读完整份源、写出档位那趟）。
        let src: Vec<u8> = {
            let mut v = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, px) in v.chunks_exact_mut(4).enumerate() {
                let g = (i % 251) as u8;
                px.copy_from_slice(&[g, g >> 1, g >> 2, 255]);
            }
            v
        };
        let mut ts = Vec::with_capacity(rounds as usize);
        // ★ 先跑两轮**不计时**的暖机：`fit_to` 每轮都要新造一份档位输出（典型 11–20 MB），
        //   第一轮的页错误/堆扩容会把"这台机器第一次给你这块内存"算进腿里。m65 判的是
        //   **这条腿本身值多少钱**，所以暖机后再测；暖机与不暖机的差写在下面单独报。
        let cold = {
            let buf = src.clone();
            let t0 = Instant::now();
            let out = fit_to(black_box(buf), w, h, view.0, view.1);
            let d = t0.elapsed();
            black_box(out.rgba.len());
            d
        };
        for _ in 0..2 {
            let buf = src.clone();
            black_box(fit_to(black_box(buf), w, h, view.0, view.1).rgba.len());
        }
        for _ in 0..rounds {
            let buf = src.clone();
            black_box(&buf);
            let t0 = Instant::now();
            let out = fit_to(black_box(buf), w, h, view.0, view.1);
            ts.push(t0.elapsed());
            black_box(out.rgba.len());
        }
        let t = med(&ts);
        // ★ 下限臂（K87 的定式：先问"再便宜能便宜到哪去"）：`fit_to` 本质是**把整份源读一遍**
        //   + 写一份小的档位。这里量两条"只摸内存不做平均"的代价当**下限**：
        //   ①`sum`＝按 8 字节字读一遍（读带宽）；②`copy`＝读一遍并等量写出（读+写带宽，
        //   比 `fit_to` 写的还多，因为它写的是全尺寸）。平均那一趟怎么写也不可能比这两条快。
        //   下限打不平 `fit_to` 才谈得上"算法还有肉"；下限就是 `fit_to`，判废。
        //   ★ 第一版这里写的是"逐字节求和"，测出来 152 MB 要 46 ms＝只有 3.3 GB/s，
        //     那把我自己的下限抬得太高（编译器没向量化到字宽），会把"还有肉"判成假象——
        //     换成字宽求和 + 加一条 memcpy 才是能拿来比的下限。
        let mut sums = Vec::with_capacity(rounds as usize);
        for _ in 0..rounds {
            let t0 = Instant::now();
            let mut acc: u64 = 0;
            for word in src.chunks_exact(8) {
                acc = acc.wrapping_add(u64::from_ne_bytes(word.try_into().unwrap()));
            }
            sums.push(t0.elapsed());
            black_box(acc);
        }
        let mut dst: Vec<u8> = vec![0u8; src.len()];
        let mut cps = Vec::with_capacity(rounds as usize);
        for _ in 0..rounds {
            let t0 = Instant::now();
            dst.copy_from_slice(&src);
            cps.push(t0.elapsed());
            black_box(&dst);
        }
        let s = med(&sums);
        let c = med(&cps);
        println!(
            "{:>6}×{:<5} {:>12.1} {:>12} {:>10.1} {:>12.2} {:>10.1} {:>9.1} {:>9.1} {:>7.1} {:>7.1}",
            w,
            h,
            src.len() as f64 / 1048576.0,
            format!("{fw}×{fh}"),
            t.as_secs_f64() * 1000.0,
            t.as_nanos() as f64 / (w as f64 * h as f64),
            cold.as_secs_f64() * 1000.0,
            s.as_secs_f64() * 1000.0,
            c.as_secs_f64() * 1000.0,
            t.as_secs_f64() / s.as_secs_f64(),
            t.as_secs_f64() / c.as_secs_f64(),
        );
    }
}
