//! 复现探针：把**真解码器 DLL** 装进真宿主，按给定顺序解一串文件，超时就报「卡死」。
//!
//! 用法（从 main 目录）：
//! ```text
//! RUPIX_DECODERS_DIR=../.build/decoders RUPIX_DECODE_TRACE=1 \
//!   cargo run --release --example decode_repro -- <文件> [文件...]
//!      [--spec=native|WxH] [--rounds=N] [--timeout=S]
//! ```
//!
//! 为什么要有它：`[decode-watchdog]` 只能告诉你"某个文件卡了"，说不出**卡在哪一段**
//! （读文件 / probe / 查询 / 渲染 / 回填），也说不出**要不要前面的解码垫底**。本探针
//! 把整串解码放在一条独立线程里、**共用同一个 `Host`**（于是共用同一批已加载的 DLL 与
//! 同一份解码器内部状态），主线程盯着它——这正是真机那条线程的形状。
//!
//! ★ 第一版探针每次新建 `Host` 只解一个文件，读数是"都能解"；可现场日志里卡住的那次
//! 往往**前面刚解过别的图**（邻图预读）。所以这里默认按传入顺序解完整个串。

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rupix_lib::decoders::Host;

fn main() {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut spec = String::from("native");
    let mut rounds: u32 = 1;
    let mut timeout_s: u64 = 10;
    for a in std::env::args().skip(1) {
        if let Some(v) = a.strip_prefix("--spec=") {
            spec = v.to_string();
        } else if let Some(v) = a.strip_prefix("--rounds=") {
            rounds = v.parse().unwrap_or(1);
        } else if let Some(v) = a.strip_prefix("--timeout=") {
            timeout_s = v.parse().unwrap_or(10);
        } else {
            files.push(PathBuf::from(a));
        }
    }
    if files.is_empty() {
        eprintln!("usage: decode_repro <file> [file...] [--spec=native|WxH] [--rounds=N]");
        std::process::exit(2);
    }
    // "native" = 不限框（原图档）；"WxH" = 带框 + 允许原生降采样（首屏那一次的形状）。
    let (max_w, max_h, allow_scaled) = if spec == "native" {
        (0, 0, false)
    } else {
        let (w, h) = spec
            .split_once('x')
            .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            .unwrap_or((1920, 1017));
        (w, h, true)
    };
    println!(
        "mode={spec} max={max_w}x{max_h} scaled={allow_scaled} rounds={rounds} timeout={timeout_s}s"
    );

    // 整个串在一条线程里跑、共用一个 Host：模拟真机那条解码线程。
    let (tx, rx) = mpsc::channel::<(usize, u32, String)>();
    let files_for_thread = files.clone();
    std::thread::Builder::new()
        .name("repro-decode".into())
        .spawn(move || {
            let mut h = Host::new();
            for round in 1..=rounds {
                for (i, p) in files_for_thread.iter().enumerate() {
                    let ext = p
                        .extension()
                        .map(|e| e.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default();
                    let r = h
                        .try_decode(p, &ext, max_w, max_h, 0, allow_scaled)
                        .map(|r| {
                            r.map(|d| {
                                format!(
                                    "ok {}x{} natural={:?}",
                                    d.bitmap.width, d.bitmap.height, d.natural
                                )
                            })
                            .unwrap_or_else(|e| format!("err {e}"))
                        })
                        .unwrap_or_else(|| "none（没有解码器认领）".into());
                    if tx.send((i, round, r)).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("起线程");

    let mut done = 0usize;
    let total = files.len() as u32 * rounds;
    while done < total as usize {
        match rx.recv_timeout(Duration::from_secs(timeout_s)) {
            Ok((i, round, msg)) => {
                done += 1;
                let name = files[i]
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                println!("{done}/{total} r{round} {name} → {msg}");
            }
            Err(_) => {
                let name = files
                    .get(done % files.len().max(1))
                    .map(|p| {
                        p.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                println!(
                    "HANG 第 {done} 笔之后 {timeout_s}s 没回包（下一笔约 {name}；阶段见 [decode-trace]）"
                );
                std::process::exit(3);
            }
        }
    }
    println!("PROBE PASS（{total} 笔都没卡）");
}
