//! 真实打开路径的解码基准：文件 → `decode::probe_decode`（解码器优先）→ RGBA。
//!
//! 与 `bench_core`（RuPix-Decoder 里，库函数直调）互补：这个量的是用户真正
//! 花掉的时间——目录扩展名认领、DLL 自述路由、两阶段 ABI 调用全算在内。
//!
//! 跑法（从 main 目录）：
//! ```text
//! cargo run --release --example gen_samples -- ../.temp/samples   # 在 RuPix-Decoder 下
//! cargo run --release --example bench_decode -- ..\.temp\samples\bench.*
//! ```
//!
//! 每个文件：1 次预热 + 5 轮计时，报平均与最快。数字只在本机内自比。

use std::path::Path;
use std::time::Instant;

use rupix_lib::core::decode;

const ROUNDS: u32 = 5;

fn main() {
    rupix_lib::install_decode_stack();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: bench_decode <image> [image...]");
        std::process::exit(2);
    }

    println!(
        "{:<12} {:>9} {:>9} {:>10}",
        "file", "avg(ms)", "min(ms)", "MPix/s"
    );
    let mut fail = 0usize;
    for a in &args {
        let p = Path::new(a);
        if !p.exists() {
            println!("{:<12} 文件不存在", a);
            fail += 1;
            continue;
        }
        // 预热一次：页错误、DLL 首次加载、惰性初始化不计入。
        let first = decode::probe_decode(p, 0, 0);
        let d = match first {
            Ok(d) => d,
            Err(e) => {
                println!(
                    "{:<12} 解码失败: {e}",
                    p.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                );
                fail += 1;
                continue;
            }
        };
        let mpix = (d.bitmap.width as f64 * d.bitmap.height as f64) / 1e6;

        let mut total = std::time::Duration::ZERO;
        let mut best = std::time::Duration::MAX;
        for _ in 0..ROUNDS {
            let t0 = Instant::now();
            let r = decode::probe_decode(p, 0, 0).expect("decode");
            total += t0.elapsed();
            best = best.min(t0.elapsed());
            assert!(!r.bitmap.rgba.is_empty());
        }
        let avg = total / ROUNDS;
        println!(
            "{:<12} {:>9.1} {:>9.1} {:>10.1}",
            p.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            avg.as_secs_f64() * 1000.0,
            best.as_secs_f64() * 1000.0,
            mpix / avg.as_secs_f64(),
        );
    }
    if fail > 0 {
        std::process::exit(1);
    }
}
