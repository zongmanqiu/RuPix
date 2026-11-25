//! m349 P2 并发压力验收：`CAP_PARALLEL` 声明者的多线程并发解码必须与顺序基线
//! **像素逐字节一致**，无 panic / 挂死 / 错误结果漂移。
//!
//! 用法：`cargo run --release --example parallel_stress -- <图片目录> [线程数=4] [每线程迭代=200]`
//!
//! 方法：
//! 1. 先用单条解码线程顺序解一遍目录内全部文件 → 基线表（路径 → Ok(像素指纹) / Err）。
//! 2. 再开 N 条线程，各自一条独立的 `Decoder`（独立回程、独立私有后端），按迭代数
//!    循环解同一批文件；每一笔回包与基线表比对。
//! 3. 结论 = 并发下"谁解的都一样"。同时天然压到宿主的跳锁分支（声明者）与
//!    DLL 锁分支（未声明者）两条路。
//!
//! 基线里的 Err 也允许出现（打不开的文件），但并发下必须**同文件同结果**——
//! 结果漂移本身就是并发缺陷。

use rupix_lib::core::decode::{Decoder, Done, ReqKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Instant;

/// 像素指纹：宽高 + FNV-1a 64（同进程内比对，稳定性足够）。
fn fingerprint(d: &rupix_lib::core::pixels::Decoded) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    };
    for v in d
        .natural
        .0
        .to_be_bytes()
        .into_iter()
        .chain(d.natural.1.to_be_bytes())
    {
        mix(v);
    }
    for b in &d.bitmap.rgba {
        mix(*b);
    }
    h
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args
        .next()
        .expect("用法：parallel_stress <目录> [线程=4] [迭代=200]");
    let threads: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);

    rupix_lib::install_decode_stack();
    let dir = PathBuf::from(dir);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("目录可读")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    assert!(!files.is_empty(), "目录里没有样本：{}", dir.display());

    // ── 顺序基线（同一条解码线程，串行）────────────────────────────
    let (tx, rx) = channel::<Done>();
    let decoder = Decoder::spawn_with(tx);
    let mut baseline: HashMap<PathBuf, Result<u64, ()>> = HashMap::new();
    for p in &files {
        decoder.request_frame(p, 0, 0, 0, ReqKind::Main);
        let done = rx.recv().expect("基线回包");
        baseline.insert(
            done.path.clone(),
            done.out.as_ref().map(fingerprint).map_err(|_| ()),
        );
    }
    drop(decoder);
    let ok_count = baseline.values().filter(|r| r.is_ok()).count();
    println!("基线：{} 个文件，{} 个可解", files.len(), ok_count);
    assert!(
        ok_count > 0,
        "基线一个都解不出：解码器目录没装配？（设 RUPIX_DECODERS_DIR）"
    );

    // ── 并发腿：N 线程各自一条 Decoder，循环解同一批 ────────────────
    let baseline = std::sync::Arc::new(baseline);
    let files = std::sync::Arc::new(files);
    let t0 = Instant::now();
    let mut handles = Vec::new();
    for tid in 0..threads {
        let baseline = baseline.clone();
        let files = files.clone();
        handles.push(std::thread::spawn(move || {
            let (tx, rx) = channel::<Done>();
            let decoder = Decoder::spawn_with(tx);
            let mut mismatch = 0usize;
            let mut decoded = 0usize;
            for i in 0..iters {
                let p = &files[(tid + i) % files.len()];
                decoder.request_frame(p, 0, 0, 0, ReqKind::Main);
                let done = rx.recv().expect("并发回包");
                let got = done.out.as_ref().map(fingerprint).map_err(|_| ());
                let want = baseline.get(&done.path).expect("基线里必有此路径");
                if got != *want {
                    mismatch += 1;
                    println!(
                        "线程 {tid} 第 {i} 轮结果漂移：{} want={want:?} got={got:?}",
                        done.path.display()
                    );
                }
                if got.is_ok() {
                    decoded += 1;
                }
            }
            (mismatch, decoded)
        }));
    }
    let total_mismatch: usize = handles
        .into_iter()
        .map(|h| h.join().expect("线程不崩"))
        .map(|(m, _)| m)
        .sum();
    let elapsed = t0.elapsed();

    println!(
        "并发：{threads} 线程 × {iters} 迭代 × {} 文件，成功解码与基线全比对，耗时 {:.1}s",
        files.len(),
        elapsed.as_secs_f64()
    );
    if total_mismatch > 0 {
        println!("FAIL：{total_mismatch} 笔结果漂移");
        std::process::exit(1);
    }
    println!("PASS：0 漂移");
}
