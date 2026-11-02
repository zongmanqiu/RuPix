//! ABI 分段计时：把任意一枚解码 DLL 直接加载，对同一个文件分别量 probe / query / render 耗时。
//!
//! 用途：定位宿主路径（bench_decode）与库直调（bench_core）之间的差价花在哪一段。
//!
//! 跑法：
//! ```text
//! cargo run --release --example bench_abi -- ../.build/decoders/<id>_<ver>.dll ..\.temp\samples\bench.jpg
//! ```

use std::ffi::c_char;
use std::time::Instant;

use rupix_lib::core::abi::*;

const ROUNDS: u32 = 5;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (dll, file) = match args.as_slice() {
        [d, f] => (d.clone(), f.clone()),
        _ => {
            eprintln!("usage: bench_abi <dll> <image>");
            std::process::exit(2);
        }
    };
    let data = std::fs::read(&file).expect("read image");
    println!("file {} ({} bytes), dll {}", file, data.len(), dll);

    unsafe {
        // 实验开关：先加载其余 DLL（复刻 Host 会把 jxl/qoi 一并装进进程的事实）。
        if std::env::var_os("ABI_LOAD_ALL").is_some() {
            let _ = libloading::Library::new("..\\.build\\decoders\\jxl_0.3.0.dll");
            let _ = libloading::Library::new("..\\.build\\decoders\\qoi_0.3.0.dll");
        }
        let lib = libloading::Library::new(&dll).expect("load");
        let probe_fn: libloading::Symbol<
            unsafe extern "C" fn(*const u8, usize, *const c_char) -> i32,
        > = lib.get(b"iv_probe").unwrap();
        let render_fn: libloading::Symbol<
            unsafe extern "C" fn(
                *const u8,
                usize,
                *const IvRenderReq,
                *mut u8,
                usize,
                *mut IvRenderOut,
                *const c_char,
            ) -> i32,
        > = lib.get(b"iv_render").unwrap();

        // v3 尾参：文件扩展名（小写）。NULL 亦可，这里与宿主一致总传。
        let ext = std::path::Path::new(&file)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| std::ffi::CString::new(e.to_ascii_lowercase()).unwrap())
            .unwrap_or_default();
        let ext_ptr = ext.as_ptr();

        // 预热一轮（含查询 + 完整渲染）。
        // budget 复刻宿主实况：MAX_PIXELS*4；ABI_BUDGET=0 时用 0 对照。
        let budget: u32 = if std::env::var_os("ABI_BUDGET_ZERO").is_some() {
            0
        } else {
            (64 * 1024 * 1024) as u32 * 4
        };
        let req = IvRenderReq {
            target_w: 0,
            target_h: 0,
            budget_bytes: budget,
            flags: 0,
            frame_index: 0,
            page_index: 0,
        };
        let mut info = IvRenderOut::default();
        let rc = render_fn(
            data.as_ptr(),
            data.len(),
            &req,
            std::ptr::null_mut(),
            0,
            &mut info,
            ext_ptr,
        );
        assert_eq!(rc, 0, "query 预热失败 rc={rc}");
        let mut buf = vec![0u8; info.bytes_needed as usize];
        let rc = render_fn(
            data.as_ptr(),
            data.len(),
            &req,
            buf.as_mut_ptr(),
            buf.len(),
            &mut info,
            ext_ptr,
        );
        assert_eq!(rc, 0, "render 预热失败 rc={rc}");

        for name in ["probe", "query", "render"] {
            let mut total = std::time::Duration::ZERO;
            let mut best = std::time::Duration::MAX;
            for _ in 0..ROUNDS {
                let t0 = Instant::now();
                match name {
                    "probe" => {
                        let c = probe_fn(data.as_ptr(), data.len(), ext_ptr);
                        assert_eq!(c, 100);
                    }
                    "query" => {
                        let rc = render_fn(
                            data.as_ptr(),
                            data.len(),
                            &req,
                            std::ptr::null_mut(),
                            0,
                            &mut info,
                            ext_ptr,
                        );
                        assert_eq!(rc, 0);
                    }
                    _ => {
                        // ABI_FRESH_BUF=1 时复刻宿主"每次新分配输出缓冲"的实况。
                        let fresh: Vec<u8>;
                        let (p, cap) = if std::env::var_os("ABI_FRESH_BUF").is_some() {
                            fresh = vec![0u8; info.bytes_needed as usize];
                            let ptr = fresh.as_ptr();
                            let len = fresh.len();
                            (ptr as *mut u8, len)
                        } else {
                            (buf.as_mut_ptr(), buf.len())
                        };
                        let rc =
                            render_fn(data.as_ptr(), data.len(), &req, p, cap, &mut info, ext_ptr);
                        assert_eq!(rc, 0);
                    }
                }
                total += t0.elapsed();
                best = best.min(t0.elapsed());
            }
            println!(
                "{:<8} avg {:>8.1} ms   min {:>8.1} ms",
                name,
                (total / ROUNDS).as_secs_f64() * 1000.0,
                best.as_secs_f64() * 1000.0,
            );
        }
    }
}
