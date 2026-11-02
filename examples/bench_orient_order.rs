//! 「先转再缩」的两条实现配对 A/B（m49）。
//!
//! 用法（从 `main/` 目录）：
//! ```text
//! .temp/target/release/examples/bench_orient_order.exe 4000 3000 1280 720 30
//! RUPIX_BENCH_REVERSE=1 .temp/target/release/examples/bench_orient_order.exe 4000 3000 1280 720 30
//! # 第 6 个参数给**真照片**的裸 RGBA 转储（w*h*4 字节）时，用它替掉合成图：
//! .temp/target/release/examples/bench_orient_order.exe 3000 4000 1280 720 30 ../.temp/m49/orient6_big.rgba
//! ```
//! A 臂 = 参照实现（`oriented` 转整张 → `fit_to` 缩）；B 臂 = 生产实现（`fit_oriented`：
//! 反向轴按远端锚定分块降采样 → 只转那份小图）。两臂只差顺序与块划分，同一个 rep 内相邻
//! 执行 ⇒ 环境漂移同向；正反两序各跑一轮，方向一致才算数。
//!
//! ★ 判据先于时间：**差异像素必须为 0**。m49 的第一版把 B 臂写成朴素"先缩再转"，在
//!   1280×720 这种非整数比例下 6/7/8 每个像素都不同（平均差 3.5/255）——那版已作废。
//!   ★ 夹具别选整数比（4000×3000→800×600 是 5:1，两套块划分恰好重合，会把这条判据照成假绿）。

use std::hint::black_box;
use std::time::Instant;

use rupix_lib::core::geometry::{fit_oriented, fit_to, oriented};
use rupix_lib::core::pixels::Bitmap;

fn reference(src: &[u8], w: u32, h: u32, o: i32, mw: u32, mh: u32) -> Bitmap {
    let (r, rw, rh) = oriented(src.to_vec(), w, h, o);
    fit_to(r, rw, rh, mw, mh)
}

/// 一臂 = 名字 + 一次完整调用（闭包只吃环境，无参数）。`'a` 是环境本身的寿命。
type Arm<'a> = (&'static str, Box<dyn Fn() + 'a>);

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let g = |i: usize, d: u32| a.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (w, h, mw, mh) = (g(0, 4000), g(1, 3000), g(2, 1280), g(3, 720));
    let reps: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(30);
    let reverse = std::env::var_os("RUPIX_BENCH_REVERSE").is_some();

    // 源像素：给了转储路径就用真照片（PIL 解出来的是**未转正**的原始行序，正是解码器
    // 交给宿主的那份）；否则用合成图——渐变 + 硬边条纹，硬边最容易暴露取样相位差。
    let src: Vec<u8> = match a.get(5) {
        Some(p) => {
            let v = std::fs::read(p).unwrap_or_else(|e| panic!("读转储 {p} 失败：{e}"));
            assert_eq!(v.len(), (w * h * 4) as usize, "转储长度与 w×h×4 不符");
            v
        }
        None => (0..w * h)
            .flat_map(|i| {
                let x = i % w;
                let y = i / w;
                let edge = if (x / 37 + y / 53) % 2 == 0 {
                    250u8
                } else {
                    5u8
                };
                [
                    (x as u8).wrapping_mul(3),
                    (y as u8).wrapping_mul(5),
                    edge,
                    255,
                ]
            })
            .collect(),
    };

    println!(
        "size={w}x{h} 视口框={mw}x{mh} reps={reps} reverse={reverse} 源={} MB 内容={}",
        src.len() / 1048576,
        if a.get(5).is_some() {
            "真照片"
        } else {
            "合成硬边"
        }
    );
    println!("orient,rep,arm,ms");

    for o in [2i32, 3, 4, 5, 6, 7, 8] {
        // —— 像素对账：各算一次，不进计时环 ——
        let arm_a = reference(&src, w, h, o, mw, mh);
        let arm_b = fit_oriented(src.clone(), w, h, o, mw, mh);
        let same =
            (arm_a.width, arm_a.height) == (arm_b.width, arm_b.height) && arm_a.rgba == arm_b.rgba;
        println!(
            "CHECK orient={o} A={}x{} B={}x{} 逐字节相同={same}",
            arm_a.width, arm_a.height, arm_b.width, arm_b.height
        );

        let mut led: Vec<(&'static str, Vec<f64>)> = Vec::new();
        for rep in 0..reps {
            let mut arms: Vec<Arm<'_>> = vec![
                (
                    "A_转整张再缩",
                    Box::new(|| {
                        let b = reference(black_box(src.as_slice()), w, h, o, mw, mh);
                        black_box(b.width ^ b.height);
                    }),
                ),
                (
                    "B_fit_oriented",
                    Box::new(|| {
                        let b = fit_oriented(black_box(src.as_slice()).to_vec(), w, h, o, mw, mh);
                        black_box(b.width ^ b.height);
                    }),
                ),
            ];
            if reverse {
                arms.reverse();
            }
            for (name, f) in arms {
                let t0 = Instant::now();
                f();
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                println!("{o},{rep},{name},{ms:.3}");
                if let Some(e) = led.iter_mut().find(|(n, _)| *n == name) {
                    e.1.push(ms);
                } else {
                    led.push((name, vec![ms]));
                }
            }
        }
        let median = |name: &str| -> f64 {
            let mut v = led
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        };
        let am = median("A_转整张再缩");
        let bm = median("B_fit_oriented");
        let saved = (am - bm).max(0.0);
        println!(
            "SUMMARY orient={o} 相同={same} A median={am:.3} ms  B median={bm:.3} ms  省={saved:.3} ms  A/B={:.2}x",
            if bm > 0.0 { am / bm } else { 0.0 }
        );
    }
}
