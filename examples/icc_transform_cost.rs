//! m90 探针：量 `platform::color::to_srgb` 每次调用的成本构成。
//!
//! 用法：`cargo run --release --example icc_transform_cost -- <profile.icm> [n]`
//!
//! 三档位图分别回答：
//! - 1×1：几乎只剩"开 profile + 建变换"的固定开销；
//! - 512×512 / 784×441：显示档的转换成本（固定开销 + 逐像素）。
//!
//! 每档 n 次独立调用（含每次重建变换，即当前产品路径），打印中位数与 min/max。

use std::time::Instant;

use rupix_lib::core::pixels::Bitmap;

fn bench(label: &str, w: u32, h: u32, icc: &[u8], n: usize) {
    let template = Bitmap {
        width: w,
        height: h,
        rgba: vec![128u8; w as usize * h as usize * 4],
    };
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let mut b = template.clone();
        let t0 = Instant::now();
        match rupix_lib::platform::color::to_srgb(&mut b, icc) {
            Ok(()) => times.push(t0.elapsed().as_secs_f64() * 1000.0),
            Err(e) => {
                eprintln!("{label}: to_srgb 失败：{e}");
                return;
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = times[n / 2];
    println!(
        "{label} {w}x{h}: 中位 {med:.2} ms（min {:.2} / max {:.2}，n={n}）",
        times[0],
        times[n - 1]
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        r"C:\Windows\System32\spool\drivers\color\sRGB Color Space Profile.icm".into()
    });
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(50);
    let icc = std::fs::read(&path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"));
    println!("profile: {path}（{} 字节）", icc.len());
    bench("tiny", 1, 1, &icc, n);
    bench("display", 512, 512, &icc, n);
    bench("viewport", 784, 441, &icc, n);
}
