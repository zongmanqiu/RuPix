//! 大目录排序耗时基准：模拟 `Gallery::from_file` 的自然序排序段。
//!
//! 用途：B1.1（大目录自然序排序键预计算）的前后对比——比较器每次现算 `natural_key`
//! 的代价 vs 预计算键（Schwartzian transform）。条目出自已随 m47 删除的「手感断点清单」。
//!
//! 跑法：`cargo run --release --example bench_gallery_sort`
//! 打印 3 档目录规模 × 5 轮取中位。

use std::ffi::OsStr;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// 与 `gallery.rs::natural_key` 字节级一致的复刻实现（example 不能访问 crate 私有函数）。
/// 修改 `gallery.rs::natural_key` 时**必须同步**——否则基准测的是错的键。
fn natural_key(s: &OsStr) -> String {
    use std::fmt::Write as _;
    let s = s.to_string_lossy().to_lowercase();
    let bytes = s.as_bytes();
    let mut key = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < bytes.len() {
        let is_digit = bytes[i].is_ascii_digit();
        let end = bytes[i..]
            .iter()
            .position(|c| c.is_ascii_digit() != is_digit)
            .unwrap_or(bytes.len() - i)
            + i;
        if is_digit {
            let num: u64 = s[i..end].parse().unwrap_or(u64::MAX);
            key.push('\u{1}');
            let _ = write!(key, "{num:020}");
            key.push('\u{1}');
            let _ = write!(key, "{}", end - i);
        } else {
            key.push_str(&s[i..end]);
        }
        i = end;
    }
    key
}

/// 生成 n 个合成文件名（混合前缀 + 变宽数字 + 后缀，模拟真实相册目录）。
fn synth(n: usize) -> Vec<PathBuf> {
    let prefixes = ["IMG_", "photo", "DSC", "p", "2024-06-", "截图_"];
    let exts = [".jpg", ".png", ".heic", ".webp"];
    (0..n)
        .map(|i| {
            let pre = prefixes[i % prefixes.len()];
            let ext = exts[(i / prefixes.len()) % exts.len()];
            // 数字宽度打散：0-4 位前导零混合，逼比较器多走分支。
            let pad = (i % 5) * (i / 100).min(4);
            let num = format!("{:0>1$}", i % 100000, 1 + pad);
            PathBuf::from(format!("{pre}{num}{ext}"))
        })
        .collect()
}

/// 现状：比较器每次现算 natural_key（对应 gallery.rs:55-59）。
fn sort_naive(files: &mut [PathBuf]) {
    files.sort_by(|a, b| {
        let ka = a.file_name().map(natural_key);
        let kb = b.file_name().map(natural_key);
        ka.cmp(&kb)
    });
}

/// 预计算键版：一次算完 O(n) 键，排序只做 String 比较。
/// 与 gallery.rs 落地的形态一致：sort_by_cached_key 一次算键（等价于旧「元组排序
/// 再剥键」写法），排序全程无 clone。
fn sort_decorated(files: &mut [PathBuf]) {
    files.sort_by_cached_key(|p| p.file_name().map(natural_key).unwrap_or_default());
}

fn bench(label: &str, n: usize, f: fn(&mut [PathBuf])) -> Duration {
    let base = synth(n);
    let mut times = Vec::new();
    for _ in 0..5 {
        let mut v = base.clone();
        let t = Instant::now();
        f(&mut v);
        black_box(&v);
        times.push(t.elapsed());
    }
    times.sort();
    let median = times[times.len() / 2];
    println!("{label:>4} n={n:<6} {median:>10.3?}");
    median
}

fn main() {
    println!("gallery 排序基准（release，5 轮取中位）");
    for n in [300usize, 1000, 3000] {
        let a = bench("naive", n, sort_naive);
        let b = bench("decor", n, sort_decorated);
        let gain = if a > Duration::ZERO {
            100.0 * (1.0 - b.as_secs_f64() / a.as_secs_f64())
        } else {
            0.0
        };
        println!("  → n={n} 收益 {gain:+.1}%\n");
    }
}
