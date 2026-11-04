//! 回包审计：真解码线程 + 真 DLL，按 viewer 的节奏连发 Main / Prefetch，
//! **逐个 seq 对账**——每一个发出去的请求都必须恰好收到一条 Done。
//!
//! 用法（从 main 目录）：
//! ```text
//! RUPIX_DECODERS_DIR=../.build/decoders \
//!   cargo run --release --example done_audit -- <目录或文件...>
//! ```
//!
//! 为什么要有它：真机日志里出现过"只有一个在途请求、20 分钟不回包"的现场。那有两种
//! 解释——(a) 解码器卡住了，(b) **请求根本没被派发 / 回包丢了**。两者在 UI 上长得一模一样
//! （旧图不动、pending 不减），但修法完全不同。本探针把 (b) 单独拎出来判：
//! 只要有一笔发了却没回，账就对不上。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use rupix_lib::core::decode::{Decoder, Done, ReqKind};

fn collect(args: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in args {
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .map(|rd| rd.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            v.sort();
            out.extend(v.into_iter().filter(|p| p.is_file()));
        } else {
            out.push(p);
        }
    }
    out
}

fn main() {
    rupix_lib::install_decode_stack();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let files = collect(&args);
    if files.is_empty() {
        eprintln!("usage: done_audit <dir|file>...");
        std::process::exit(2);
    }
    println!("样本 {} 个", files.len());

    let (tx, rx) = channel::<Done>();
    let dec = Decoder::spawn_with(tx);
    let mut sent: Vec<u64> = Vec::new();
    let (w, h) = (900u32, 700u32);

    // viewer 的节奏：翻一页（Main 带框 + 允许缩放）＋ 预读邻图（Prefetch）。
    let t0 = Instant::now();
    for (i, p) in files.iter().cycle().take(160).enumerate() {
        sent.push(dec.request_frame_scaled(p, w, h, 0, ReqKind::Main));
        let next = &files[(i + 1) % files.len()];
        sent.push(dec.request_frame(next, w, h, 0, ReqKind::Prefetch));
        std::thread::sleep(Duration::from_millis(45));
    }
    let sent_n = sent.len();
    println!("已发 {} 笔（{:.1}s）", sent_n, t0.elapsed().as_secs_f64());

    let mut got: HashSet<u64> = HashSet::new();
    let mut dup = 0usize;
    let mut errs = 0usize;
    let deadline = Instant::now() + Duration::from_secs(45);
    while got.len() < sent_n && Instant::now() < deadline {
        if let Ok(d) = rx.recv_timeout(Duration::from_millis(200)) {
            if !got.insert(d.seq) {
                dup += 1;
            }
            if d.out.is_err() {
                errs += 1;
            }
        }
    }
    let want: HashSet<u64> = sent.iter().copied().collect();
    let missing: Vec<u64> = want.difference(&got).copied().collect();
    println!(
        "收到 {} / 发出 {}；dup={dup} err={errs} 缺失={} 在途登记={}",
        got.len(),
        sent_n,
        missing.len(),
        rupix_lib::core::decode_watchdog::pending_count()
    );
    if !missing.is_empty() {
        println!("缺失 seq（前 20）：{:?}", &missing[..missing.len().min(20)]);
        println!("AUDIT FAIL");
        std::process::exit(3);
    }
    println!("AUDIT PASS");
}
