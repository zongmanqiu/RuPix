//! 解码器端到端冒烟：从 `.build/decoders` 加载**真 DLL**，把宿主侧 ABI 消费路径走一遍。
//!
//! 与单元测试的区别：单元测试在解码器 crate 内部自测解码；本探针站在**宿主的位置**
//! 用 `libloading` 装载 DLL——`rupix_lib::core::abi`（即 `RuPix-Decoder/abi` 真源）
//! 的结构体与 DLL 里的手写 ABI 是否逐字节对齐、两阶段调用是否自洽，只有这一步
//! 才能证明。跨 DLL 边界的任何布局漂移在这里直接现形。
//!
//! 用法（从 main 目录）：
//! ```text
//! cargo run --release --example decoder_smoke -- \
//!     ../.build/decoders/common_0.1.0.dll \
//!     ../.build/decoders/modern_0.1.0.dll \
//!     --jxl 解码器仓库
//! ```
//! `--jxl <file.jxl>` 对现代格式档做真解码校验。
//!
//! `--raw <raw 档 dll>`：裸数据参数化解码段（v4 可选导出 iv_render_raw）——
//! 生成 3×2 BGRA 临时文件，走宿主 `Host::try_decode_raw` 全链路断言像素。
//! 该段经 `catalog::dir()` 找解码器，须先设 `RUPIX_DECODERS_DIR` 指向 DLL 所在目录。

use std::ffi::{c_char, CStr};

use rupix_lib::core::abi::*;

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return "(null)".into();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// 2×2 纯红 QOI（与 RuPix-Decoder/workspace/singles/qoi 单元测试同款字节串）。
const QOI_2X2_RED: &[u8] = &[
    0x71, 0x6f, 0x69, 0x66, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x04, 0x00, 0xfe, 0xff,
    0x00, 0x00, 0xc2, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: decoder_smoke <dll> [dll...] [--jxl <file.jxl>]");
        std::process::exit(2);
    }

    let mut jxl_file: Option<String> = None;
    let mut raw_dll: Option<String> = None;
    let mut dlls: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--jxl" {
            jxl_file = it.next().cloned();
        } else if a == "--raw" {
            raw_dll = it.next().cloned();
        } else {
            dlls.push(a);
        }
    }

    let mut failures = 0usize;
    for path in &dlls {
        println!("== {}", path);
        unsafe {
            let lib = match libloading::Library::new(path) {
                Ok(l) => l,
                Err(e) => {
                    println!("   LOAD-FAIL: {e}");
                    failures += 1;
                    continue;
                }
            };

            let abi_v: libloading::Symbol<unsafe extern "C" fn() -> u32> =
                lib.get(b"iv_abi_version").expect("no iv_abi_version");
            let v = abi_v();
            println!("   iv_abi_version = {v}");
            if !(5..=IV_ABI_VERSION).contains(&v) {
                println!("   FAIL: ABI 版本出区间（宿主 = {IV_ABI_VERSION}，下界 = 5）");
                failures += 1;
                continue;
            }

            let info_fn: libloading::Symbol<unsafe extern "C" fn() -> *const IvDecoderInfo> =
                lib.get(b"iv_decoder_info").expect("no iv_decoder_info");
            let info = info_fn();
            if info.is_null() {
                println!("   FAIL: iv_decoder_info 返回 null");
                failures += 1;
                continue;
            }
            let (id, exts, is_vector) = ((*info).id, (*info).extensions, (*info).is_vector);
            println!(
                "   id={} name={} exts={} vector={} alpha={}",
                cstr(id),
                cstr((*info).display_name),
                cstr(exts),
                is_vector,
                (*info).has_alpha,
            );

            let probe_fn: libloading::Symbol<
                unsafe extern "C" fn(*const u8, usize, *const c_char) -> i32,
            > = lib.get(b"iv_probe").expect("no iv_probe");
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
            > = lib.get(b"iv_render").expect("no iv_render");
            let last_err: libloading::Symbol<unsafe extern "C" fn(*mut c_char, usize) -> i32> =
                lib.get(b"iv_last_error").expect("no iv_last_error");

            let id_s = cstr(id);
            // 选测试数据：qoi 用内嵌字节；自述里认领 jxl 的解码器用 --jxl 文件；其余跳过解码段。
            let has_jxl = cstr(exts).split(';').any(|e| e == "jxl");
            let data: Vec<u8> = if id_s == "qoi" {
                QOI_2X2_RED.to_vec()
            } else if has_jxl {
                if let Some(f) = &jxl_file {
                    std::fs::read(f).unwrap_or_default()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            if data.is_empty() {
                println!("   SKIP decode（无测试数据）");
                continue;
            }

            // v3：扩展名尾参（小写，可 NULL——宿主总是传，探针也传）。
            let ext_s = std::ffi::CString::new(id_s.as_str()).unwrap();
            let ext_ptr = ext_s.as_ptr();

            let pr = probe_fn(data.as_ptr(), data.len(), ext_ptr);
            println!("   iv_probe = {pr}");
            if pr != 100 {
                println!("   FAIL: probe 应为 100");
                failures += 1;
            }

            let req = IvRenderReq {
                target_w: 0,
                target_h: 0,
                budget_bytes: 0,
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
            println!(
                "   query: rc={rc} {}x{} bytes={} pf={} oriented={} frames={} delay={} pages={} icc={}",
                info.actual_w, info.actual_h, info.bytes_needed, info.pixel_format,
                info.already_oriented, info.frame_count, info.frame_delay_ms,
                info.page_count, info.icc_len
            );
            if rc != 0 {
                println!("   FAIL: 查询阶段 rc={rc}");
                failures += 1;
                continue;
            }

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
            if rc != 0 {
                println!("   FAIL: 渲染阶段 rc={rc}");
                failures += 1;
                continue;
            }
            // 像素校验：两者都是纯红测试图。
            let ok_px =
                buf.len() >= 4 && buf[0] == 255 && buf[1] == 0 && buf[2] == 0 && buf[3] == 255;
            println!(
                "   render: rc={rc} px0={:?} {}",
                &buf[..4.min(buf.len())],
                if ok_px { "OK" } else { "FAIL" }
            );
            if !ok_px {
                failures += 1;
            }

            let mut ebuf = [0i8; 256];
            let need = last_err(ebuf.as_mut_ptr(), ebuf.len());
            let msg = if need > 1 {
                cstr(ebuf.as_ptr())
            } else {
                "(empty)".into()
            };
            println!("   last_error = {msg}");

            // 4 字节坏数据：probe 必须干净地拒绝，而不是 panic。
            let bad = [0xdeu8, 0xad, 0xbe, 0xef];
            println!("   iv_probe(bad) = {}", probe_fn(bad.as_ptr(), 4, ext_ptr));
        }
    }

    // ── 裸数据参数化解码段（v4 可选导出 iv_render_raw）──
    if let Some(dll) = &raw_dll {
        println!("== raw segment ({dll})");
        use rupix_lib::core::decode::RawParams;
        use rupix_lib::decoders::Host;
        if !dll.contains("raw") {
            println!("   SKIP（--raw 指向的 DLL 不是 raw 档）");
        } else {
            // 3×2 BGRA：第 0 像素纯红（B,G,R,A = 0,0,255,255），经交换后应为 R,G,B,A = 255,0,0,255。
            let mut px = vec![0u8; 3 * 2 * 4];
            px[0..4].copy_from_slice(&[0, 0, 255, 255]);
            px[4..8].copy_from_slice(&[0, 255, 0, 255]); // 第 1 像素纯绿
            let dir = rupix_lib::core::paths::test_scratch_root();
            let f = dir.join("decoder_smoke_3x2.bgra");
            std::fs::write(&f, &px).expect("写临时裸文件");
            let mut host = Host::new();
            let params = RawParams {
                width: 3,
                height: 2,
                layout: PF_BGRA8,
            };
            match host.try_decode_raw(&f, "bgra", params, 0, 0) {
                Some(Ok(d)) => {
                    let ok_dim = d.natural == (3, 2) && d.bitmap.width == 3 && d.bitmap.height == 2;
                    let ok_px = d.bitmap.rgba[0..8] == [255, 0, 0, 255, 0, 255, 0, 255];
                    println!(
                        "   natural={:?} px0..1={:?} {}",
                        d.natural,
                        &d.bitmap.rgba[0..8],
                        if ok_dim && ok_px { "OK" } else { "FAIL" }
                    );
                    if !ok_dim || !ok_px {
                        failures += 1;
                    }
                }
                other => {
                    println!("   FAIL: try_decode_raw = {other:?}");
                    failures += 1;
                }
            }
            // YUV 段：I420 2×2 全白（Y=235/UV=128，BT.601 limited）全链路。
            let f2 = dir.join("decoder_smoke_2x2.i420");
            std::fs::write(&f2, [235u8, 235, 235, 235, 128, 128]).expect("写临时 I420");
            let yuv = RawParams {
                width: 2,
                height: 2,
                layout: PF_I420,
            };
            match host.try_decode_raw(&f2, "i420", yuv, 0, 0) {
                Some(Ok(d)) => {
                    let ok_px = d.bitmap.rgba[0..4] == [255, 255, 255, 255];
                    println!(
                        "   I420 px0={:?} {}",
                        &d.bitmap.rgba[0..4],
                        if ok_px { "OK" } else { "FAIL" }
                    );
                    if !ok_px {
                        failures += 1;
                    }
                }
                other => {
                    println!("   FAIL: I420 try_decode_raw = {other:?}");
                    failures += 1;
                }
            }
            let _ = std::fs::remove_file(&f2);
            // 负例：长度与 宽×高×bpp 不符必须报错而不是 panic。
            let bad = RawParams {
                width: 9,
                height: 9,
                layout: PF_BGRA8,
            };
            match host.try_decode_raw(&f, "bgra", bad, 0, 0) {
                Some(Err(_)) => println!("   size-mismatch correctly rejected: OK"),
                other => {
                    println!("   FAIL: 长度不符未被拒绝 = {other:?}");
                    failures += 1;
                }
            }
            let _ = std::fs::remove_file(&f);
        }
    }

    if failures == 0 {
        println!("\nALL OK");
    } else {
        println!("\n{failures} FAILURE(S)");
        std::process::exit(1);
    }
}
