//! fit 缩放档位探针（无 GUI）：真实解码线程（解码器）+ Viewer 状态机，
//! 验证「适应窗口」与「1:1」在不同图/窗口尺寸下的倍率数学。
//!
//! 起因（2026-10-08 用户反馈）：小图下适应窗口 ≡ 1:1（旧"永不放大"规则），
//! 两个按钮失去区分。本探针钉死改后的行为——
//!
//! - 大图（2000×1200）+ 小窗（800×600）：适应 = 0.4，1:1 = 1.0，切回来也成立；
//! - 小图（300×200）+ 大窗（800×600）：适应 = 2.667（**放大**贴满视口），1:1 = 1.0；
//! - 旋转 90° 后有效宽高互换：适应倍率随之变化（0.4 → 0.3）。
//!
//! 运行：`cargo run --example fit_probe`（无需桌面会话，解码线程自初始化 COM）。

use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

use rupix_lib::core::config::Config;
use rupix_lib::core::decode::{Decoder, Done};
use rupix_lib::core::viewer::Viewer;

/// 写一张纯色 24bpp BMP（解码线程直接可解，无第三方依赖）。
fn write_bmp(path: &PathBuf, w: u32, h: u32) {
    use std::io::Write;
    let row = (w * 3).div_ceil(4) * 4; // 行 4 字节对齐
    let pixels = row * h;
    let mut f = std::fs::File::create(path).expect("创建测试 BMP 失败");
    // BITMAPFILEHEADER（14B）+ BITMAPINFOHEADER（40B）
    f.write_all(b"BM").unwrap();
    f.write_all(&(54 + pixels).to_le_bytes()).unwrap();
    f.write_all(&0u32.to_le_bytes()).unwrap();
    f.write_all(&54u32.to_le_bytes()).unwrap();
    f.write_all(&40u32.to_le_bytes()).unwrap();
    f.write_all(&(w as i32).to_le_bytes()).unwrap();
    f.write_all(&(h as i32).to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&24u16.to_le_bytes()).unwrap();
    f.write_all(&0u32.to_le_bytes()).unwrap(); // BI_RGB
    f.write_all(&pixels.to_le_bytes()).unwrap();
    f.write_all(&0u32.to_le_bytes()).unwrap(); // XPelsPerMeter
    f.write_all(&0u32.to_le_bytes()).unwrap(); // YPelsPerMeter
    f.write_all(&0u32.to_le_bytes()).unwrap(); // biClrUsed
    f.write_all(&0u32.to_le_bytes()).unwrap(); // biClrImportant
    let zeros = vec![0u8; pixels as usize];
    f.write_all(&zeros).unwrap();
}

fn main() {
    rupix_lib::install_decode_stack();
    // 便携原则：探针的临时样本也不写 `%TEMP%`，放在**探针 exe 同级目录**
    // （`cargo clean` 会随 target 一起带走，不会在用户目录里留下 7MB 的 BMP）。
    let dir = rupix_lib::core::paths::exe_dir()
        .map(|d| d.join("fit_probe_samples"))
        .unwrap_or_else(|| rupix_lib::core::paths::test_scratch_root().join("rupix_fit_probe"));
    std::fs::create_dir_all(&dir).unwrap();
    let big = dir.join("big.bmp"); // 2000×1200
    let small = dir.join("small.bmp"); // 300×200
    write_bmp(&big, 2000, 1200);
    write_bmp(&small, 300, 200);

    let (tx, rx) = channel::<Done>();
    let decoder = std::rc::Rc::new(Decoder::spawn_with(tx));
    // 探针不关心配置驱动的行为（循环/保持变换/长图），用默认配置。
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
    let mut v = Viewer::new(decoder, cfg);

    let mut passed = 0;
    let mut failed = 0;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {
            if $cond {
                passed += 1;
                println!("  ✓ {}", format!($($arg)*));
            } else {
                failed += 1;
                println!("  ✗ {}", format!($($arg)*));
            }
        };
    }

    /// 泵 600ms：反复 tick 驱动防抖解码，期间到达的解码结果一律 apply。
    ///
    /// 注意：set_fit 后若现有位图已够清晰（SHARP_ENOUGH 优化），**不会有**新解码
    /// 结果——泵完不报错，由调用方直接断言倍率。
    fn pump(v: &mut Viewer, rx: &std::sync::mpsc::Receiver<Done>) {
        let deadline = Instant::now() + Duration::from_millis(600);
        loop {
            // 本探针只看倍率数学（fit 与 1:1 的换算），不验证画面刷新，重绘信号就地丢弃。
            let _ = v.tick();
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(done) => {
                    v.apply(done);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() > deadline {
                        return;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    // ── 场景 1：大图 + 小窗（用户报障的主场景）──
    println!("大图 2000×1200，窗口 800×600：");
    v.on_viewport(0.0, 0.0, 800.0, 600.0, 1.0);
    v.open(&big);
    pump(&mut v, &rx);
    println!("  [状态] {}", v.status());
    assert!(v.error_text().is_none(), "大图解码失败");
    let s = v.scale();
    check!(
        (s - 0.4).abs() < 1e-3,
        "适应窗口倍率 = {:.3}（期望 0.4）",
        s
    );
    let dst = v
        .dst_rect(rupix_lib::core::geometry::RectI::new(0, 0, 800, 600))
        .unwrap();
    check!(
        dst.w == 800 && dst.h == 480,
        "适应窗口显示 {}×{}（期望 800×480）",
        dst.w,
        dst.h
    );

    v.zoom_actual();
    pump(&mut v, &rx);
    check!(v.scale() == 1.0, "1:1 倍率 = {:.3}（期望 1.0）", v.scale());

    v.set_fit(true);
    pump(&mut v, &rx);
    check!(
        (v.scale() - 0.4).abs() < 1e-3,
        "1:1 → 适应窗口，倍率 = {:.3}（期望回到 0.4）",
        v.scale()
    );

    // ── 场景 2：小图 + 大窗（旧规则下适应 ≡ 1:1 的场景）──
    println!("小图 300×200，窗口 800×600：");
    v.open(&small);
    pump(&mut v, &rx);
    println!("  [状态] {}", v.status());
    assert!(v.error_text().is_none(), "小图解码失败");
    let s = v.scale();
    check!(
        (s - 2.667).abs() < 1e-2,
        "适应窗口倍率 = {:.3}（期望 2.667，放大贴满）",
        s
    );
    check!(v.scale() != 1.0, "适应窗口 ≠ 1:1（重叠问题已消除）");

    // ── 场景 3：旋转 90° 后有效宽高互换，适应倍率随之变化 ──
    println!("旋转 90°（大图，窗口 800×600）：");
    v.open(&big);
    pump(&mut v, &rx);
    v.rotate_cw();
    let s = v.scale();
    check!(
        (s - 0.3).abs() < 1e-3,
        "旋转后适应倍率 = {:.3}（期望 0.3 = min(800/1200, 600/2000)）",
        s
    );
    let (ew, eh) = {
        let dst = v
            .dst_rect(rupix_lib::core::geometry::RectI::new(0, 0, 800, 600))
            .unwrap();
        (dst.w, dst.h)
    };
    check!(ew < eh, "显示矩形竖向化：{}×{}", ew, eh);

    println!("\n结果：{} 通过，{} 失败", passed, failed);
    if failed > 0 {
        std::process::exit(1);
    }
}
