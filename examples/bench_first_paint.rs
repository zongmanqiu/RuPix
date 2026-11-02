//! 首屏解码基准（m35）：同一张图，"整解再缩到视口" vs "原生降采样直接出档位"。
//!
//! 量的是**用户真正等的那一段**——打开/翻页后第一帧出来的时间。与 `bench_decode`
//! （整解口径）互补：这里的两条路线都是生产真实路径，只是缩放意图不同。
//!
//! ★ 顺带断言画质红线：降采样那一档的 `Decoded::natural` 必须仍是**原图**尺寸。
//!   报小了 `viewer::tick` 的"是否已是整图"闸会误判成立，那次原图档升级永不发生，
//!   放大到 1:1 就永久糊在档位图上——这是 m35 唯一的新风险，必须每次跑都验。
//!
//! 用法（从 `main` 目录）：
//! ```text
//! cargo run --release --example bench_first_paint -- <基准语料>\jpeg_photo_XL_q100.jpg
//! ```
//! 视口框用 `RUPIX_BENCH_VIEW=2560x1440` 覆盖（默认 2560×1440，约 2K 屏满窗）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rupix_lib::{core::decode, decoders, install_decode_stack};

/// 计时轮数。取**中位数**：首轮带页错误/DLL 惰性初始化的毛刺不该进结论。
/// `RUPIX_BENCH_ROUNDS` 可覆盖（配合 `RUPIX_DECODE_TRACE=1` 取 1 轮看分段）。
const ROUNDS: u32 = 7;

fn med(v: &[Duration]) -> Duration {
    let mut s = v.to_vec();
    s.sort();
    s[s.len() / 2]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// 跑一轮并计时。`f` 返回位图像素数，防止优化器把结果整段消掉。
fn time_it(rounds: u32, mut f: impl FnMut() -> usize) -> Duration {
    let mut v = Vec::with_capacity(rounds as usize);
    for _ in 0..rounds {
        let t0 = Instant::now();
        let px = f();
        v.push(t0.elapsed());
        assert!(px > 0, "解出空位图");
    }
    med(&v)
}

fn main() {
    // 归因用：`RUPIX_DECODE_TRACE=1 RUST_LOG=rupix_lib=debug` 时宿主给每个请求打一行
    // `[decode-trace] 文件 解码器 read/probe/query/render/post`——看得见时间花在哪一段。
    rupix_lib::platform::logging::install_logging();
    let dir = if let Some(p) = std::env::var_os("RUPIX_DECODERS_DIR") {
        PathBuf::from(p)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(".build")
            .join("decoders")
    };
    assert!(dir.is_dir(), "解码器目录不存在：{}", dir.display());
    std::env::set_var("RUPIX_DECODERS_DIR", &dir);

    let vwvh = std::env::var("RUPIX_BENCH_VIEW")
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

    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(!args.is_empty(), "用法：bench_first_paint <图> [图...]");

    install_decode_stack();
    crate::decoders::request_rescan();

    println!("视口框 {vwvh:?}，轮数 {rounds}（中位数）；每一轮都是真解码");
    println!(
        "{:<26} {:>10} {:>10} {:>8} {:>10}",
        "文件", "整解(ms)", "降采样(ms)", "加速", "natural"
    );

    for a in &args {
        let p = Path::new(a);
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        // 原图尺寸：框 0 = 要原始尺寸，这一档不带缩放意图。
        let full = decode::probe_decode(p, 0, 0).unwrap_or_else(|e| panic!("{name}: {e}"));
        let natural = full.natural;

        // ① 现状：按视口框请求，但**不点亮**降采样 → 解码器整解后再 `fit_to` 到视口。
        let t_full = time_it(rounds, || {
            let d = decode::probe_decode(p, vwvh.0, vwvh.1).expect("整解失败");
            d.bitmap.width as usize * d.bitmap.height as usize
        });
        // ② m35：同一框 + 点亮降采样 → 解码器按档位直接解出够用的那一份。
        let t_scaled = time_it(rounds, || {
            let d = decode::probe_scaled(p, vwvh.0, vwvh.1).expect("降采样失败");
            // ★ 画质红线：natural 必须还是原图尺寸，否则升级闸永不响（见模块头）。
            assert_eq!(
                d.natural, natural,
                "降采样把 natural 报成了档位尺寸（{}×{}），原图档升级将永不发生",
                d.natural.0, d.natural.1
            );
            d.bitmap.width as usize * d.bitmap.height as usize
        });

        println!(
            "{:<26} {:>10.1} {:>10.1} {:>7.2}× {:>10}",
            name,
            ms(t_full),
            ms(t_scaled),
            ms(t_full) / ms(t_scaled).max(0.0001),
            format!("{}×{}", natural.0, natural.1),
        );
    }
}
