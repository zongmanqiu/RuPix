//! GIF 解码基准：测量真实播放路径（首开 → 连续逐帧 → 循环回绕）的解码耗时。
//! 不改任何生产解码逻辑。磁盘解码缓存已撤（2026-10-19），每一轮量的都是真解码。
//!
//! 用法：`cargo run --release --example gif_bench -- <样本.gif>`
//! 无参默认 `..\.temp\bench.gif`（24 帧 / 640x480 / loop=0）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rupix_lib::{core::decode, decoders, install_decode_stack};

/// 缺省样本帧数（`.temp/bench.gif` 是 24 帧）。GIF 帧数惰性发现（首帧时 `anim.count`
/// 是 0），与生成脚本保持一致，可用第二个命令行参数覆盖。
const DEFAULT_N: u32 = 24;

fn med(v: &[Duration]) -> Duration {
    let mut s = v.to_vec();
    s.sort();
    s[s.len() / 2]
}

fn main() {
    // 归因用：`RUPIX_DECODE_TRACE=1 RUST_LOG=rupix_lib=debug` 时，宿主会给每个请求打一行
    // `[decode-trace] 文件 解码器 read/probe/query/render/post`——看得见"这一帧到底是谁解的"。
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

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "..\\.temp\\bench.gif".into());
    let gif = Path::new(&path);
    assert!(gif.exists(), "样本不存在：{path}（先生成多帧 GIF）");
    let frames: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_N);

    install_decode_stack();
    crate::decoders::request_rescan();
    crate::decoders::set_safe_mode(false);

    println!("=== GIF 解码基准（每轮真解码）===");
    println!("样本：{path}，已知帧数 N={frames}");

    // ① 首开（冷）：建会话 + 解第 0 帧。含 mmap 冷启动 / FFI 首调等一次性噪声，仅作参考。
    let t = Instant::now();
    let d0 = decode::probe_decode(gif, 0, 0).expect("首帧应解出");
    let first_open = t.elapsed();
    let (w, h) = d0.natural;
    println!("画布 {w}x{h}，首帧 RGBA {} 字节", d0.bitmap.rgba.len());
    // ★ 功能读数（不是计时）：第一帧就该知道总帧数，否则宿主信息条只能写 `帧 1/?`。
    println!("解码器报的总帧数：第 0 帧 count={}", d0.anim.count);

    // ② 连续逐帧播放（真实 viewer 路径：request_frame(1..frames-1)，游标紧跟，每帧只解 1 帧）。
    let mut seq_each = Vec::new();
    let mut c1 = 0;
    for i in 1..frames {
        let t = Instant::now();
        let d = decode::probe_decode_frame(gif, 0, 0, i).expect("帧应解出");
        if i == 1 {
            c1 = d.anim.count;
        }
        seq_each.push(t.elapsed());
    }
    println!("                    第 1 帧 count={c1}（两个都该等于 N={frames}）");
    let seq_avg = seq_each.iter().sum::<Duration>() / seq_each.len() as u32;
    let seq_max = *seq_each.iter().max().unwrap();

    // ③ 循环回绕：每次先推游标到尾帧（next_index=frames>0），再 serve(0) 强制命中
    //    `serve` 的重建快进分支（lib.rs:450 `s.next_index > index`）。
    let mut rewind_times = Vec::new();
    for _ in 0..7 {
        let _ = decode::probe_decode_frame(gif, 0, 0, frames - 1).expect("尾帧应解出"); // 推游标到尾
        let t = Instant::now();
        let _ = decode::probe_decode_frame(gif, 0, 0, 0).expect("回绕帧应解出"); // 重建
        rewind_times.push(t.elapsed());
    }

    println!("① 首开（冷，含启动噪声）: {:?}", first_open);
    println!(
        "② 顺序播放每帧 平均 {:?} / 最大 {:?}（{} 帧）",
        seq_avg,
        seq_max,
        seq_each.len()
    );
    println!("③ 循环回绕（中位）      : {:?}", med(&rewind_times));
    let r = med(&rewind_times);
    let ratio = r.as_secs_f64() / seq_avg.as_secs_f64();
    println!("回绕 / 顺序单帧 = {ratio:.2}x");

    // ④ **后退一步**（逐帧浏览真正的代价所在）：先把游标推到第 i 帧之后
    //    （`serve(i)` 让 `next_index = i+1`），再要第 i−1 帧 → 命中 `serve` 的
    //    重建分支 + 快进 i−1 帧。③ 量不到这一刀，因为它回的是第 0 帧（快进 0 步）。
    println!("④ 后退一步（重建 + 快进 i−1 帧）：");
    let mut back = Vec::new();
    for &i in &[4u32, 9, 24, 49, 99] {
        if i + 1 >= frames {
            continue;
        }
        let mut t = Vec::new();
        for _ in 0..5 {
            let _ = decode::probe_decode_frame(gif, 0, 0, i).expect("定位帧应解出");
            let s = Instant::now();
            let _ = decode::probe_decode_frame(gif, 0, 0, i - 1).expect("后退帧应解出");
            t.push(s.elapsed());
        }
        let m = med(&t);
        println!(
            "   第 {} → {} 帧: {:?}（= {:.1}x 顺序单帧）",
            i,
            i - 1,
            m,
            m.as_secs_f64() / seq_avg.as_secs_f64()
        );
        back.push((i, m));
    }
    // ⑤ 前进一步（对照）：游标紧跟，应当与 ② 同量级。
    let mut fwd = Vec::new();
    for i in 1..frames.min(20) {
        let _ = decode::probe_decode_frame(gif, 0, 0, i).expect("前进帧应解出");
        let s = Instant::now();
        let _ = decode::probe_decode_frame(gif, 0, 0, i + 1).expect("前进帧应解出");
        fwd.push(s.elapsed());
    }
    println!("⑤ 前进一步（中位）      : {:?}  ← 对照 ②", med(&fwd));

    // ⑥ **连退 20 步**——逐帧浏览真正的人手动作，也是 O(n²) 的战场：④ 每次只退一步，
    //    环刚好被上一步的重建喂满，量不出"一路向后退"的累积代价。
    if frames > 60 {
        println!("⑥ 从第 60 帧连退 20 步（每轮先定位到 60，定位与首轮不计入）：");
        for r in 0..3u32 {
            let _ = decode::probe_decode_frame(gif, 0, 0, 60).expect("定位帧应解出");
            let t0 = Instant::now();
            let mut worst = Duration::ZERO;
            for k in 1..=20u32 {
                let s = Instant::now();
                let _ = decode::probe_decode_frame(gif, 0, 0, 60 - k).expect("后退帧应解出");
                worst = worst.max(s.elapsed());
            }
            let total = t0.elapsed();
            if r > 0 {
                println!("   第 {r} 遍: 合计 {total:?}，最慢一步 {worst:?}");
            }
        }
    }
    if let Some((i, m)) = back.last() {
        println!(
            "模型：后退到第 k 帧 ≈ 重建 + k×{:.2} ms；本样本最深一次（第 {} 帧）{:?}，\
             一帧展示时长 {:.0} ms —— 超过一帧时长就是看得见的停顿",
            seq_avg.as_secs_f64() * 1000.0,
            i,
            m,
            40
        );
    }
    if ratio >= 2.0 {
        println!("结论：回绕成本显著偏高 → 值得做「合成帧缓存」（缓存第 0 帧 RGBA，回绕直接返回）");
    } else {
        println!("结论：回绕与顺序单帧同量级 → 真实播放无卡顿热点，暂无需优化");
    }
}
