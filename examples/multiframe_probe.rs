//! 多帧格式逐格实测（m183 · 用户报"旋转的星星 AVIF 不动"后立案）。
//!
//! 问两件事，且都只有站在宿主的位置才答得出：
//! 1. **解码器回没回多帧**：`Anim.count` / `Anim.delay_ms`；
//! 2. **宿主拿第 N 帧拿不拿得到、帧与帧是不是真不同**：逐帧 `probe_decode_frame` 取像素做指纹。
//!
//! ★ 走的是生产同步入口 `core::decode::probe_decode_frame`（真实 UI 走 `Decoder::request_frame`
//! 异步链路，但那条链路最终也是把 `frame_index` 写进 `IvRenderReq`，同一扇门）。
//! ★ 判动画的那道闸是 `delay_ms > 0`（`core::pixels::Anim::is_animated`）——所以
//! **count>1 而 delay=0** 这种形状要单独点名：解码器说"我有 5 帧"，宿主仍然不会挂调度。
//!
//! 用法（从 `main` 目录，DLL 目录要指到发布目录，否则测的是旧拷贝）：
//! ```text
//! RUPIX_DECODERS_DIR=E:/…/.build/decoders cargo run --release --example multiframe_probe -- <文件…>
//! ```
//! ★ **不要**把 `emf` / `wmf` / `pdf` 一类文档档丢进来：本轮已实测 `图片2.emf` 在
//! `doc_0.1.0.dll` 里 90 秒不返回，而这个探针是单线程顺序取帧，会把整轮读数拖死。

use std::path::Path;
use std::time::Instant;

/// 帧像素指纹：FNV-1a 64。够用且确定性——比"看尺寸一样就说没动"严谨，
/// 又不至于为了取个指纹把 3704² 的样张做两次 SHA。
fn fingerprint(buf: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    // 全量扫字节对 3704²(≈55 MB) 太慢；按 4 KiB 采样一次，步长固定 ⇒ 可复现。
    let step = 4096usize;
    let mut i = 0;
    while i < buf.len() {
        let end = (i + 64).min(buf.len());
        for &b in &buf[i..end] {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        i += step;
    }
    h
}

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        println!("用法：multiframe_probe <文件> [文件…]");
        std::process::exit(2);
    }

    println!(
        "{:<34} {:>5} {:>6} {:>6} {:>7}  {:<10}  判定",
        "文件", "宽", "高", "帧数", "delay", "distinct"
    );
    println!("{}", "-".repeat(112));

    for f in files {
        let p = Path::new(&f);
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or(f.clone());
        let t0 = Instant::now();
        let d0 = match rupix_lib::core::decode::probe_decode_frame(p, 0, 0, 0) {
            Ok(d) => d,
            Err(e) => {
                println!(
                    "{:<34} {:>5} {:>6} {:>6} {:>7}  {:<10}  打不开：{}",
                    name, "-", "-", "-", "-", "-", e
                );
                continue;
            }
        };
        let first_ms = t0.elapsed().as_millis();
        let (w, h) = (d0.bitmap.width, d0.bitmap.height);
        let count = d0.anim.count;
        let delay = d0.anim.delay_ms;

        // 帧数未知（GIF 惰性数帧：撞 EOF 才回填）⇒ 自己往上试，最多 16 帧或撞到错。
        let probe_to = if count == 0 { 16 } else { count.clamp(1, 8) };
        let mut hashes = vec![fingerprint(&d0.bitmap.rgba)];
        let mut err = String::new();
        let mut frame_ms = Vec::new();
        for i in 1..probe_to {
            let t = Instant::now();
            match rupix_lib::core::decode::probe_decode_frame(p, 0, 0, i) {
                Ok(d) => {
                    frame_ms.push(t.elapsed().as_millis());
                    let hv = fingerprint(&d.bitmap.rgba);
                    if hv != *hashes.last().unwrap() {
                        hashes.push(hv);
                    }
                }
                Err(e) => {
                    err = format!("第 {i} 帧失败：{e}");
                    break;
                }
            }
        }
        let distinct = hashes
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        let verdict = if delay == 0 && distinct == 1 {
            "静态：只有一帧，宿主不挂调度（正确）".to_string()
        } else if delay == 0 && count > 1 {
            format!("★ 报多帧（count={count}）但 delay=0 ⇒ 宿主判静态、不会动")
        } else if delay == 0 {
            "★ 报多帧但 delay=0 ⇒ 宿主判静态、不会动".to_string()
        } else if distinct > 1 {
            format!("真多帧：{distinct} 帧像素各不相同，可播")
        } else if count > 1 {
            "★ 报多帧但每帧像素相同".to_string()
        } else {
            "delay>0 却只有一帧".to_string()
        };
        let ms_note = if frame_ms.is_empty() {
            format!("首帧 {first_ms}ms")
        } else {
            format!("首帧 {first_ms}ms / 后续 {:?}ms", frame_ms)
        };
        println!(
            "{:<34} {:>5} {:>6} {:>6} {:>7}  {:<10}  {verdict}  [{ms_note}]{}",
            name,
            w,
            h,
            if count == 0 {
                "未知".to_string()
            } else {
                count.to_string()
            },
            delay,
            format!("{distinct}/{probe_to}"),
            if err.is_empty() {
                String::new()
            } else {
                format!("  ⚠ {err}")
            },
        );
    }
}
