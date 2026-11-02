//! 拖动鸟瞰时"一帧到底画多久"的分臂实测——M7 的定案依据。
//!
//! 存在理由（2026-10-17 用户真机复测）：内存已经与菠萝同量级（打开那张 6055×6055
//! 稳定在 ~290 MB，与探针算出的 283.6 MB 对上），但**快拖鸟瞰 CPU 仍到 ~5%**。
//! 拖动路径上每来一个鼠标事件就可能整窗重绘一次（本项目静止不重绘，脏了就画，
//! 没有第二条限速），所以 5% 这个数只能拆成两半回答：
//!
//! 1. **画**的成本——本探针量的就是这一半：直接驱动生产入口 `windui::render::SkiaCanvas`
//!    （窗口后端用的就是它），分臂量"整窗铺底 / 主图 6055²→窗口 / 各倍率放大档 /
//!    鸟瞰 512² 与 6055² / 只建整窗 Mask / 绕开 Mask 直连 tiny-skia"各花多少。
//!    2026-10-17 首轮实测的结论写在 `docs/踩坑记录.md` K52：**成本随"目标像素数"走，
//!    与源尺寸无关**（`lto = true` 下 ~20 ns/目标像素）。同一天再往下查一层，
//!    发现这个 20 ns 里有 5 ns 是**构建配置**给的：fat LTO 在链接期按宿主的 `opt-level = "z"`
//!    重编依赖位码，把 `Cargo.toml` 里"tiny-skia / windui 提 3"那条例外冲掉了；
//!    `lto = false` 之后同一批臂全部落到 ~15 ns（生产@200% 一臂中位 9.7 → 7.1 ms，
//!    整窗铺底 0.130 → 0.048 ms），代价 exe +0.55 MB——见 K53。**所以本探针报的每一臂
//!    都必须带上当时那档发布配置**，跨档比较等于换了一支探针。
//! 2. **呈现**的成本——`opacity < 100%` 时窗口是分层窗口，每帧走整窗预乘拷贝 +
//!    `UpdateLayeredWindow`（`.3rd/.../platform/win32/mod.rs:988-1130`）。这一半
//!    需要真窗口，探针够不着，由用户在设置里把不透明度调到 **100%** 复测：
//!    100% 时框架把 `WS_EX_LAYERED` 摘掉（同文件 3477 行起），那条路整个不走。
//!
//! 口径（`docs/踩坑记录.md` K48/K50 的规矩）：**同一进程内分臂交替**跑，报每组的最小值
//! 与中位数而不是单次读数——单值在这台机器上噪声就有 ±20%；"M9 之前/之后"两臂用同一份
//! 源缓冲、同一个目标框，只差源尺寸这一个旋钮。
//!
//! 用法：
//! ```text
//! cd ../.temp/target/release/examples && ./paint_cost_probe.exe [迭代倍数]
//! ```

use std::time::Instant;

use tiny_skia::Pixmap;
use windui::geometry::{Color, Rect};
use windui::render::image::{Fit, Image};
use windui::render::{Canvas, Paint, SkiaCanvas};

/// 窗口内容尺寸（逻辑=物理，本探针 dpi 1.0）：与用户当前配置一致（800×600）。
const WIN_W: u32 = 800;
const WIN_H: u32 = 600;
/// 鸟瞰盒（`canvas.rs` 的 `NAV_W/NAV_H` 口径）。
const NAV_W: i32 = 160;
const NAV_H: i32 = 120;
/// 那张报障图的原图档。
const BIG: u32 = 6055;

/// 造一张**不透明**测试位图（alpha 恒 255）：生产里的照片/截图都是这种形态，
/// 补丁 015 的 1:1 拷贝快路只认它；此前这里把 alpha 也填成 0x40（半透明），
/// 量出来的 1:1 一直是通用 src-over，代表不了真机。
fn make(w: u32, h: u32) -> Image {
    let mut rgba = vec![0x40u8; (w as usize) * (h as usize) * 4];
    for a in rgba.chunks_exact_mut(4) {
        a[3] = 255;
    }
    Image::from_rgba(w, h, &rgba).unwrap_or_else(|e| panic!("造测试位图失败：{e}"))
}

/// 一组计时读数（按毫秒）。报**最小值与中位数**，不报单次。
struct Stats {
    name: &'static str,
    n: usize,
    min: f64,
    mid: f64,
    max: f64,
}

fn measure(name: &'static str, iters: usize, mut run: impl FnMut()) -> Stats {
    let mut times: Vec<f64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        run();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Stats {
        name,
        n: iters,
        min: times[0],
        mid: times[times.len() / 2],
        max: times[times.len() - 1],
    }
}

/// 经生产入口 `SkiaCanvas`（窗口后端用的就是它）跑一臂。
fn bench(name: &'static str, iters: usize, mut draw: impl FnMut(&mut SkiaCanvas<'_>)) -> Stats {
    bench_sized(name, WIN_W, WIN_H, iters, &mut draw)
}

/// 与 `bench` 相同，但目标缓冲可指定尺寸（m69：量"大窗"那一档）。
fn bench_sized(
    name: &'static str,
    w: u32,
    h: u32,
    iters: usize,
    mut draw: impl FnMut(&mut SkiaCanvas<'_>),
) -> Stats {
    let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("{w}×{h} 离屏缓冲造不出"));
    measure(name, iters, || {
        let mut cx = SkiaCanvas::new(&mut pm);
        draw(&mut cx);
    })
}

/// 不经 windui、直接对 `tiny_skia::Pixmap` 跑一臂（用来量"少了某一步"的下限）。
fn bench_raw(name: &'static str, iters: usize, mut draw: impl FnMut(&mut Pixmap)) -> Stats {
    let mut pm =
        Pixmap::new(WIN_W, WIN_H).unwrap_or_else(|| panic!("{}×{} 离屏缓冲造不出", WIN_W, WIN_H));
    measure(name, iters, || draw(&mut pm))
}

fn report(s: &Stats) {
    println!(
        "  {:<34} n={:<4} 最小 {:>6.3} ms ｜ 中位 {:>6.3} ms ｜ 最大 {:>6.3} ms",
        s.name, s.n, s.min, s.mid, s.max
    );
}

/// "每帧 × 每秒多少帧"折算成单核占用百分比。
fn cpu_pct(ms: f64, fps: u32) -> f64 {
    ms * fps as f64 / 10.0
}

fn main() {
    let mult: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .filter(|m| *m > 0)
        .unwrap_or(1);
    let big_iters = 40 * mult;
    let small_iters = 200 * mult;

    let big = make(BIG, BIG); // 原图档那份（139.9 MB）
    let nav_small = make(512, 512); // M9 之后鸟瞰的源
    let level600 = make(600, 600); // ≤100% 时我们的显示档（本地派生出来的那一份）
                                   // B4 形态的"可见区"：200% 倍率下 800×600 的窗口只用到源里 400×300 那一条，
                                   // 留一倍余量当平移缓冲，取 900×700。
    let crop = make(900, 700);
    let nav_rect = Rect::new(
        WIN_W as i32 - NAV_W - 8,
        WIN_H as i32 - NAV_H - 8,
        NAV_W,
        NAV_H,
    );
    let win_rect = Rect::new(0, 0, WIN_W as i32, WIN_H as i32);
    let bg = Paint::fill(Color::rgb(0, 0, 0));

    // ★ M2b 的靶子：上屏那一下"**非预乘 RGBA → 预乘 Pixmap**"到底多贵——生产里就是
    // `ImageCache::obtain` 调的 `Image::from_rgba`（`ui/canvas.rs:183`）。这条量的不是
    // "画"，是"把一份档位交给框架"本身，每次换档/翻页各一次。
    // 上一轮的账（0.088–0.140 s）是**开着 fat LTO** 时量的，那段循环当时被按 `z` 重编
    // （见 K53），所以必须重测才准拿来定 M2b 的成本/收益。
    let mut up_src = vec![0x40u8; (BIG as usize) * (BIG as usize) * 4];
    let upload_big = measure(
        "预乘上传 6055²（M2b 靶子：from_rgba）",
        10,
        || {
            let img = Image::from_rgba(BIG, BIG, &up_src)
                .unwrap_or_else(|e| panic!("预乘上传 {BIG}² 失败：{e}"));
            // 不许把优化掉：`Image` 内部是 `Rc<Pixmap>`，只有真被画出来才算造好。
            assert_eq!(img.width(), BIG);
        },
    );
    report(&upload_big);
    // 这条是 (a) 项的靶子：`from_rgba` 现在先把源 `to_vec()` 拷一份再逐像素预乘——
    // 那一次 139.9 MB 的纯拷贝占掉多少，决定"直接从不预乘的切片读进去"值不值一条补丁。
    let copy_only = measure(
        "对照：只把源 to_vec() 拷一份（139.9 MB 纯 memcpy）",
        10,
        || {
            let v = up_src.to_vec();
            assert_eq!(v.len(), up_src.len());
        },
    );
    report(&copy_only);
    println!(
        "  → 预乘上传每像素 {:.2} ns（源一份 139.9 MB；这条是 M2b 要挪走的东西）｜其中纯拷贝 {:.1} ms（{:.0}%）",
        upload_big.mid * 1e6 / ((BIG * BIG) as f64),
        copy_only.mid,
        copy_only.mid * 100.0 / upload_big.mid
    );

    println!(
        "软光栅分臂实测（目标 {}×{}，鸟瞰框 {}×{}，源：大 {}²、小 512²）",
        WIN_W, WIN_H, NAV_W, NAV_H, BIG
    );

    // 单独图元：先看清每一项的量级。
    let clear = bench("整窗铺底 800×600", small_iters, |c| {
        c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
    });
    let main_big = bench("主图 6055² → 800×600", big_iters, |c| {
        c.draw_image(&big, win_rect, Fit::Fill, 0.0, 1.0);
    });
    let nav_s = bench("鸟瞰 512² → 160×120（现在）", small_iters, |c| {
        c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
    });
    let nav_b = bench("鸟瞰 6055² → 160×120（M9 前）", big_iters, |c| {
        c.draw_image(&big, nav_rect, Fit::Fill, 0.0, 1.0);
    });
    for s in [&clear, &main_big, &nav_s, &nav_b] {
        report(s);
    }

    // ★ 关键一组：拖鸟瞰时画面**必然超出视口**（不超出就没有鸟瞰），所以主图那一笔
    // 的真实形态是"目标框比窗口大、被裁进窗口"，不是上面的等比缩小。两臂都要量——
    // 差别在"每帧到底扫了多少源像素"，那决定 M7/B4 该怎么选。
    let up2 = bench(
        "主图 6055² → 1600×1200（200%，裁进窗）",
        big_iters,
        |c| {
            c.draw_image(&big, Rect::new(-400, -300, 1600, 1200), Fit::Fill, 0.0, 1.0);
        },
    );
    let up4 = bench(
        "主图 6055² → 3200×2400（400%，裁进窗）",
        big_iters,
        |c| {
            c.draw_image(
                &big,
                Rect::new(-1200, -900, 3200, 2400),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    let up8 = bench(
        "主图 6055² → 6400×4800（800%，裁进窗）",
        big_iters,
        |c| {
            c.draw_image(
                &big,
                Rect::new(-2800, -2100, 6400, 4800),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    let nav_up = bench(
        "鸟瞰 512² → 160×120 且被裁一半",
        small_iters,
        |c| {
            c.draw_image(
                &nav_small,
                Rect::new(NAV_W * 2 - 8, WIN_H as i32 - 60, NAV_W, NAV_H),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    for s in [&up2, &up4, &up8, &nav_up] {
        report(s);
    }

    // 机制判据：这三臂分辨"9.8 ms 到底随源尺寸还是随目标尺寸走"。
    //   源整幅 → 与源等大的框（被窗口裁掉绝大部分）：若仍旧 ~9 ms，说明每帧过一遍整源。
    //   源整幅 → 1:1 铺满窗口大小（我们的显示档形态）：这是 ≤100% 时生产真实的形态。
    //   **可见区切片**（只把窗口那一条源裁出来 → 放大进框）：这是 B4/补丁 012 的形态，
    //   量它便宜到什么程度，才决定动不动 `.3rd`。
    let full_dst = bench(
        "主图 6055² → 6055² 框（几乎全被裁）",
        big_iters,
        |c| {
            c.draw_image(
                &big,
                Rect::new(-5255, -5455, 6055, 6055),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    let display_level = bench(
        "显示档 600×600 → 1:1 进窗（≤100% 真实形态）",
        small_iters,
        |c| {
            c.draw_image(&level600, Rect::new(100, 0, 600, 600), Fit::Fill, 0.0, 1.0);
        },
    );
    // ★ m69 —— 同一形态强制走通用路：opacity 差一丁点就不满足补丁 015 的 `== 1.0`
    // 判据，落回"整窗 Mask + 逐像素 src-over"。同一进程、同一源、同一目标框，
    // 用来给"快路 vs 旧路"做同轮配对（旧路就是补丁前的唯一路径）。
    let display_level_generic = bench(
        "显示档 600×600 → 1:1（强制通用路对照）",
        small_iters,
        |c| {
            c.draw_image(
                &level600,
                Rect::new(100, 0, 600, 600),
                Fit::Fill,
                0.0,
                0.999_999,
            );
        },
    );
    let sliced = bench(
        "可见区切片 900×700 → 1600×1200（B4 形态）",
        small_iters,
        |c| {
            c.draw_image(
                &crop,
                Rect::new(-400, -300, 1600, 1200),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    let frame_b4 = bench(
        "一帧（B4 之后：铺底+切片+512 鸟瞰）",
        small_iters,
        |c| {
            c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
            c.draw_image(
                &crop,
                Rect::new(-400, -300, 1600, 1200),
                Fit::Fill,
                0.0,
                1.0,
            );
            c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
        },
    );
    for s in [&full_dst, &display_level, &sliced, &frame_b4] {
        report(s);
    }
    report(&display_level_generic);
    // ★ m69 —— "大窗"那一档（1920×1080 显示档 1:1 整窗）：用户报的 7.6–7.8% CPU
    // 就出在这个面积上。快路与强制通用路同轮配对。
    let level1080 = make(1920, 1080);
    let big_win_fast = bench_sized(
        "1920×1080 显示档 → 整窗 1:1（快路）",
        1920,
        1080,
        big_iters,
        |c| {
            c.draw_image(&level1080, Rect::new(0, 0, 1920, 1080), Fit::Fill, 0.0, 1.0);
        },
    );
    let big_win_generic = bench_sized(
        "1920×1080 显示档 → 整窗 1:1（强制通用路）",
        1920,
        1080,
        big_iters,
        |c| {
            c.draw_image(
                &level1080,
                Rect::new(0, 0, 1920, 1080),
                Fit::Fill,
                0.0,
                0.999_999,
            );
        },
    );
    for s in [&big_win_fast, &big_win_generic] {
        report(s);
    }
    // ★ m69 —— 两条生产帧：@fit（派生档 1:1 进视口）与 @100%（原图档 1:1、被窗口裁）。
    // 这是补丁 015 真正瞄准的形态；上面那些"主图 6055² → 800×600"都是降采样，走不到快路。
    let frame_fit = bench(
        "一帧@fit（铺底+显示档 1:1+512 鸟瞰）",
        big_iters,
        |c| {
            c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
            c.draw_image(&level600, Rect::new(100, 0, 600, 600), Fit::Fill, 0.0, 1.0);
            c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
        },
    );
    let frame_fit_generic = bench(
        "一帧@fit（同帧，图片那笔强制通用路）",
        big_iters,
        |c| {
            c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
            c.draw_image(
                &level600,
                Rect::new(100, 0, 600, 600),
                Fit::Fill,
                0.0,
                0.999_999,
            );
            c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
        },
    );
    for s in [&frame_fit, &frame_fit_generic] {
        report(s);
    }

    // ★ 生产形态的对照：**主画布那份 `Image` 是 pixelated 的**（补丁 011），≥100% 时
    // 走最近邻；本探针上面几臂用的都是默认（双线性）图，量出来的不是真机那一条。
    //
    // ★ m70 更正一处口径：这一臂此前标着"生产@200%"，但 6055² 源画进 1600×1200 框
    // 其实是 **26% 降采样**（`upscaling` 为假，pixelated 不生效，走的是双线性）。
    // 真机的 ≥100% 是"源整幅 → 远大于窗口的框，只有一屏可见"：200% 下整图目标框
    // 是 12110²，窗口裁进去只剩 800×600。
    let big_pix = big.clone().with_pixelated(true);
    let prod26 = bench(
        "主图 pixelated 6055² → 1600×1200（26%，降采样）",
        big_iters,
        |c| {
            c.draw_image(
                &big_pix,
                Rect::new(-400, -300, 1600, 1200),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    let prod200 = bench(
        "主图 pixelated 6055² → 12110² 框（生产@200%，裁进窗）",
        big_iters,
        |c| {
            c.draw_image(
                &big_pix,
                Rect::new(-4000, -3000, 12110, 12110),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    // ★ m70 —— 同一 @200% 几何强制走通用路，与本进程的快路配对（旧路＝补丁前唯一路径）。
    let prod200_generic = bench(
        "主图 pixelated 6055² → 12110²（同几何强制通用路）",
        big_iters,
        |c| {
            c.draw_image(
                &big_pix,
                Rect::new(-4000, -3000, 12110, 12110),
                Fit::Fill,
                0.0,
                0.999_999,
            );
        },
    );
    let prod100 = bench(
        "主图 pixelated 6055² → 600×600 框（降采样，非快路）",
        big_iters,
        |c| {
            c.draw_image(&big_pix, Rect::new(100, 0, 600, 600), Fit::Fill, 0.0, 1.0);
        },
    );
    // ★ m69 —— 100% 平移：原图档 1:1、drawn 框与源同尺寸、被窗口裁掉大半。
    // 补丁 015 只搬"交集"那一段，不再把整幅源过一遍。
    let pan100 = bench(
        "主图 pixelated 6055² → 6055² 框（@100% 平移，快路形态）",
        big_iters,
        |c| {
            c.draw_image(
                &big_pix,
                Rect::new(-3000, -2000, 6055, 6055),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    report(&pan100);
    // ★ m70 —— 派生档切片的真放大形态（pixelated 生效）：900×700 源进 1600×1200 是
    // 1.78×，进 1800×1400 是 2×。上面 `sliced` 是同几何的双线性版，可直接配对。
    let crop_pix = crop.clone().with_pixelated(true);
    let sliced_near = bench(
        "可见区切片 pixelated 900×700 → 1600×1200（1.78× 最近邻）",
        small_iters,
        |c| {
            c.draw_image(
                &crop_pix,
                Rect::new(-400, -300, 1600, 1200),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    let sliced_near2 = bench(
        "可见区切片 pixelated 900×700 → 1800×1400（2× 最近邻）",
        small_iters,
        |c| {
            c.draw_image(
                &crop_pix,
                Rect::new(-500, -400, 1800, 1400),
                Fit::Fill,
                0.0,
                1.0,
            );
        },
    );
    // ★ m70 —— 同几何强制走通用路（opacity 差一丁点就不满足补丁 016 的 `== 1.0`），
    // 给"快路 vs 补丁前唯一路径"做同轮配对。
    let sliced_near_generic = bench(
        "可见区切片 pixelated → 1600×1200（同几何强制通用路）",
        small_iters,
        |c| {
            c.draw_image(
                &crop_pix,
                Rect::new(-400, -300, 1600, 1200),
                Fit::Fill,
                0.0,
                0.999_999,
            );
        },
    );
    let prod_frame = bench(
        "一帧（生产@200%：铺底+pixelated 主图+鸟瞰）",
        big_iters,
        |c| {
            c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
            c.draw_image(
                &big_pix,
                Rect::new(-4000, -3000, 12110, 12110),
                Fit::Fill,
                0.0,
                1.0,
            );
            c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
        },
    );
    for s in [
        &prod26,
        &prod200,
        &prod200_generic,
        &prod100,
        &sliced_near,
        &sliced_near_generic,
        &sliced_near2,
        &prod_frame,
    ] {
        report(s);
    }
    println!(
        "  → 补丁 016 同轮配对：@200% {:.3} → {:.3} ms（{:.1}×）｜ 1.78× 切片 {:.3} → {:.3} ms（{:.1}×）",
        prod200_generic.mid,
        prod200.mid,
        prod200_generic.mid / prod200.mid,
        sliced_near_generic.mid,
        sliced_near.mid,
        sliced_near_generic.mid / sliced_near.mid,
    );
    println!(
        "  单位换算：{:.1} ns/目标像素（双线性整窗）｜ 26% 降采样 {:.1} ns ｜ 1.78× 最近邻 {:.2} ns ｜ 2× 最近邻 {:.2} ns（均为每可见像素）",
        main_big.mid * 1e6 / (WIN_W * WIN_H) as f64,
        prod26.mid * 1e6 / (WIN_W * WIN_H) as f64,
        sliced_near.mid * 1e6 / (WIN_W * WIN_H) as f64,
        sliced_near2.mid * 1e6 / (WIN_W * WIN_H) as f64,
    );

    // ★ 决定性一臂：**绕开 windui 的 `draw_image`，直接叫 tiny-skia 画同一张图、
    // 同一个框、同一个变换，但不带 mask**。动机是 `.3rd/.../render/skia.rs:703-715`
    // 那句自认"本函数整体仍是 O(窗口)"——但实测两臂都 ~9 ms：整窗 Mask 单独建+填只花
    // 0.06 ms，省掉 mask 也只从 10.3 降到 8.8 ms。**结论：O(窗口) 那句注释描述的是
    // mask 的尺寸，不是这一帧的成本大头**；大头是缩放混合本身（随目标像素数线性）。
    // 见 `docs/踩坑记录.md` K52——引用别人（包括自己上一轮）写的成本故事不算依据。
    use tiny_skia::{
        FillRule, FilterQuality, Mask, PathBuilder, PixmapPaint, Rect as TsRect,
        Transform as TsTransform,
    };
    let src_pix = Pixmap::new(BIG, BIG).unwrap();
    // 税的本体：只新建并填充那块整窗 mask（windui 每次 `draw_image` 都来一遍）。
    let mask_only = bench_raw(
        "只建整窗 Mask + 扫描线填充（税本体）",
        big_iters,
        |pm| {
            let _ = pm;
            let mut m = Mask::new(WIN_W, WIN_H).unwrap();
            let mut pb = PathBuilder::new();
            pb.push_rect(TsRect::from_xywh(100.0, 0.0, 600.0, 600.0).unwrap());
            let path = pb.finish().unwrap();
            m.fill_path(&path, FillRule::Winding, true, TsTransform::identity());
        },
    );
    report(&mask_only);
    let floor = bench_raw(
        "无 mask 直连 tiny-skia（省掉 Mask 的下限）",
        big_iters,
        |pm| {
            let pp = PixmapPaint {
                opacity: 1.0,
                quality: FilterQuality::Bilinear,
                ..Default::default()
            };
            pm.draw_pixmap(
                -400,
                -300,
                src_pix.as_ref(),
                &pp,
                TsTransform::from_scale(2.0, 2.0),
                None,
            );
        },
    );
    report(&floor);
    // 候选 (ii) 的价值：**1:1 平移 blit**（无缩放变换）该不该和"带变换的采样"一样贵。
    // 若这一臂远小于上面 8.8 ms，就给"最近邻整数倍放大走 sprite 快路"定了价；
    // 若差不多，那条补丁就不值得动 `.3rd`。
    let src_1to1 = Pixmap::new(WIN_W, WIN_H).unwrap();
    let one_to_one = bench_raw(
        "1:1 平移 blit（800×600 源，无变换）",
        small_iters,
        |pm| {
            let pp = PixmapPaint::default();
            pm.draw_pixmap(0, 0, src_1to1.as_ref(), &pp, TsTransform::identity(), None);
        },
    );
    report(&one_to_one);
    // 再价一条：1:1 blit 也要 7.45 ns/像素（铺底只要 0.27），差的是 **SrcOver 混合**——
    // 源是**不透明**的时候这层混合本来就是白算。`derive_level` 逐像素过一遍，
    // 顺手记下"整份 alpha 全 255"是零成本，于是有了这条候选：不透明源走拷贝混合。
    let copy_1to1 = bench_raw(
        "1:1 拷贝混合（不透明源，Src）",
        small_iters,
        |pm| {
            let pp = PixmapPaint {
                blend_mode: tiny_skia::BlendMode::Source,
                ..Default::default()
            };
            pm.draw_pixmap(0, 0, src_1to1.as_ref(), &pp, TsTransform::identity(), None);
        },
    );
    let copy_2x = bench_raw(
        "2× 采样 + 拷贝混合（不透明源，Src）",
        big_iters,
        |pm| {
            let pp = PixmapPaint {
                quality: FilterQuality::Bilinear,
                blend_mode: tiny_skia::BlendMode::Source,
                ..Default::default()
            };
            pm.draw_pixmap(
                -400,
                -300,
                src_pix.as_ref(),
                &pp,
                TsTransform::from_scale(2.0, 2.0),
                None,
            );
        },
    );
    report(&copy_1to1);
    report(&copy_2x);

    // ★ m70 —— 最近邻放大的三条路，同一几何（900×700 → 1600×1200，1.78×）：
    //   (a) 生产 `SkiaCanvas`：补丁 016 之后走行内搬运（上面 `sliced_near`）；
    //   (b) 直连 tiny-skia：Nearest 采样，但不建 mask；
    //   (c) 朴素行内 gather：逐像素切片索引（每步带边界检查），作为"写法不同差多少"的对照。
    // (c) 的映射与 tiny-skia `Gather` 同源（设备像素中心 → 逆变换 → 截断 → 夹紧），
    // 只是写法更笨；补丁 016 用 `chunks_exact_mut(4).zip(...)` 把边界检查拿掉。
    // 表按目标宽预计算（真实实现每帧一次，与窗口宽同阶），不计入搬运耗时。
    let near_w = 900usize;
    let near_h = 700usize;
    let near_src = Pixmap::new(near_w as u32, near_h as u32).unwrap();
    let (ntx, nty, ndw, ndh) = (-400i32, -300i32, 1600i32, 1200i32);
    let (nsx, nsy) = (ndw as f32 / near_w as f32, ndh as f32 / near_h as f32);
    let raw_nearest = bench_raw(
        "直连：Nearest 1.78× 画 900×700（无 mask）",
        small_iters,
        |pm| {
            let pp = PixmapPaint {
                opacity: 1.0,
                quality: FilterQuality::Nearest,
                ..Default::default()
            };
            pm.draw_pixmap(
                0,
                0,
                near_src.as_ref(),
                &pp,
                TsTransform::from_scale(nsx, nsy).post_translate(ntx as f32, nty as f32),
                None,
            );
        },
    );
    let vx0 = ntx.max(0);
    let vx1 = (ntx + ndw).min(WIN_W as i32);
    let col_off: Vec<usize> = (vx0..vx1)
        .map(|dx| {
            let s = ((dx as f32 + 0.5 - ntx as f32) / nsx).floor() as i32;
            s.clamp(0, near_w as i32 - 1) as usize * 4
        })
        .collect();
    let nearest_gather = bench_raw(
        "最近邻 1.78× 自搬（行内 gather 下限）",
        small_iters,
        |pm| {
            let dst = pm.data_mut();
            let src = near_src.data();
            let dst_stride = WIN_W as usize * 4;
            let src_stride = near_w * 4;
            for dy in 0..WIN_H as i32 {
                let sy = ((dy as f32 + 0.5 - nty as f32) / nsy).floor() as i32;
                let srow = sy.clamp(0, near_h as i32 - 1) as usize * src_stride;
                let drow = dy as usize * dst_stride + vx0 as usize * 4;
                for (i, &co) in col_off.iter().enumerate() {
                    let d = drow + i * 4;
                    dst[d..d + 4].copy_from_slice(&src[srow + co..srow + co + 4]);
                }
            }
        },
    );
    report(&raw_nearest);
    report(&nearest_gather);
    println!(
        "  → 最近邻 1.78× 每可见像素：生产（补丁 016）{:.2} ns ｜ 无 mask 直连 {:.2} ns ｜ 朴素 gather {:.2} ns",
        sliced_near.mid * 1e6 / (WIN_W * WIN_H) as f64,
        raw_nearest.mid * 1e6 / (WIN_W * WIN_H) as f64,
        nearest_gather.mid * 1e6 / (WIN_W * WIN_H) as f64,
    );

    // ★ m69 立案：候选 ⑤「框架级真 blit」的下限臂。
    // 生产里 ≤100% 是**派生档 1:1 进视口**（源与目标同像素），这一形态不需要任何
    // 采样/混合：不透明源覆盖目标，逐行 `copy_from_slice` 的字节结果与 SrcOver 恒等。
    // 这两臂量的是"若框架认得出这一形态，一帧的图片那笔最低能到多少"。
    let blit_src = Pixmap::new(WIN_W, WIN_H).unwrap();
    let row_copy = bench_raw(
        "1:1 不透明行拷贝（目标快路下限）",
        small_iters,
        |pm| {
            let dst = pm.data_mut();
            let src = blit_src.data();
            let stride = WIN_W as usize * 4;
            for y in 0..WIN_H as usize {
                let o = y * stride;
                dst[o..o + stride].copy_from_slice(&src[o..o + stride]);
            }
        },
    );
    let row_copy_clipped = bench_raw(
        "1:1 行拷贝：6055² 源裁进 800×600（@100% 形态）",
        small_iters,
        |pm| {
            // 源 (x0,y0) 起贴进目标，等效"大图平移到只剩这一屏"。
            let dst = pm.data_mut();
            let src = src_pix.data();
            let (x0, y0) = (3000usize, 2000usize);
            let dst_stride = WIN_W as usize * 4;
            let src_stride = BIG as usize * 4;
            for y in 0..WIN_H as usize {
                let so = (y0 + y) * src_stride + x0 * 4;
                let do_ = y * dst_stride;
                dst[do_..do_ + dst_stride].copy_from_slice(&src[so..so + dst_stride]);
            }
        },
    );
    report(&row_copy);
    report(&row_copy_clipped);
    // 代价的另一半：框架得先知道"这张图不透明"。检测放到上传时做（每张图一次），
    // 这里量 6055² 的全 alpha 扫描有多贵——它要加到 `Image::from_rgba` 那 85 ms 上。
    // 先把 alpha 字节刷成 255（否则 `all` 在第一个像素就短路、量出来的是零）：
    // 上面那几条上传/拷贝臂早已跑完，改这份缓冲不影响它们。
    for a in up_src.chunks_exact_mut(4) {
        a[3] = 255;
    }
    let opaque_scan = measure("不透明检测 6055²（一次扫描）", 10, || {
        let all = up_src.chunks_exact(4).all(|c| c[3] == 255);
        assert!(all);
    });
    report(&opaque_scan);
    println!(
        "  → 行拷贝 {:.2} ns/px（对照 1:1 blit {:.2} ns/px），不透明检测 6055² {:.1} ms",
        row_copy.mid * 1e6 / (WIN_W * WIN_H) as f64,
        one_to_one.mid * 1e6 / (WIN_W * WIN_H) as f64,
        opaque_scan.mid
    );
    println!(
        "  → 每像素成本：铺底 {:.2} ns ｜ 1:1 blit {:.2} ns ｜ 带 2× 变换采样 {:.1} ns",
        clear.mid * 1e6 / (WIN_W * WIN_H) as f64,
        one_to_one.mid * 1e6 / (WIN_W * WIN_H) as f64,
        floor.mid * 1e6 / (WIN_W * WIN_H) as f64
    );
    println!(
        "  → mask 税 = {:.1} ms（生产一帧 {:.1} ms − 无 mask 直连 {:.1} ms）；省掉的正是那块整窗 Mask",
        prod_frame.mid - floor.mid,
        prod_frame.mid,
        floor.mid
    );

    // ★ 档位尺寸与上屏矩形差**恰好 1 px** 时贵一倍（2026-10-18 真机 `drag_cpu_rig4` 抓到）：
    // 35% 那臂 `src=2131 dst=2130` 要 7.0 ms，91% 那臂 `src=5526 dst=5526` 只要 3.1 ms。
    // 嫌疑是 `.3rd/.../render/skia.rs` 的 1:1 吸附判据 `(dw-iw).abs() < 1.0`——差 1 px 时
    // 它取不到 `Transform::identity()` 的纯 blit，退化成整片双线性重采样。
    // 这四臂只差那 1 px，别的都不变（同一个窗、同一个源、同一个框居中）。
    let l2131 = make(2131, 2131);
    let l5526 = make(5526, 5526);
    let box_at = |s: i32| Rect::new((WIN_W as i32 - s) / 2, (WIN_H as i32 - s) / 2, s, s);
    let id2131 = bench(
        "2131² 源 → 2131² 框（吸附成 1:1）",
        big_iters,
        |c| {
            c.draw_image(&l2131, box_at(2131), Fit::Fill, 0.0, 1.0);
        },
    );
    let m12131 = bench("2131² 源 → 2130² 框（差 1 px）", big_iters, |c| {
        c.draw_image(&l2131, box_at(2130), Fit::Fill, 0.0, 1.0);
    });
    let id5526 = bench(
        "5526² 源 → 5526² 框（吸附成 1:1）",
        big_iters,
        |c| {
            c.draw_image(&l5526, box_at(5526), Fit::Fill, 0.0, 1.0);
        },
    );
    let m15526 = bench("5526² 源 → 5525² 框（差 1 px）", big_iters, |c| {
        c.draw_image(&l5526, box_at(5525), Fit::Fill, 0.0, 1.0);
    });
    for s in [&id2131, &m12131, &id5526, &m15526] {
        report(s);
    }
    // 再剥一层：不经 windui，直接叫 tiny-skia 用「identity」与「scale=2130/2131」两种
    // 变换画同一张源。若前者远小于后者，成本就在"变换不是整数 1"这一件事上，
    // 与源尺寸、与双线性/最近邻都无关。
    let src_2131 = Pixmap::new(2131, 2131).unwrap();
    let raw_id = bench_raw("直连：identity 变换画 2131²", big_iters, |pm| {
        let pp = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..Default::default()
        };
        pm.draw_pixmap(
            -665,
            -765,
            src_2131.as_ref(),
            &pp,
            TsTransform::identity(),
            None,
        );
    });
    let raw_s1 = bench_raw("直连：scale=2130/2131 画 2131²", big_iters, |pm| {
        let pp = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..Default::default()
        };
        pm.draw_pixmap(
            -665,
            -765,
            src_2131.as_ref(),
            &pp,
            TsTransform::from_scale(2130.0 / 2131.0, 2130.0 / 2131.0),
            None,
        );
    });
    report(&raw_id);
    report(&raw_s1);
    println!(
        "  → 1 px 失配的代价：windui {:+.2} ms（{:.2}×）｜ 直连 {:+.2} ms（{:.2}×）",
        m12131.mid - id2131.mid,
        m12131.mid / id2131.mid,
        raw_s1.mid - raw_id.mid,
        raw_s1.mid / raw_id.mid
    );

    // 一帧的组合：拖动时每个重绘周期就是这一整串。
    let frame_now = bench(
        "一帧（现在：铺底+主图+512 鸟瞰）",
        big_iters,
        |c| {
            c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
            c.draw_image(&big, win_rect, Fit::Fill, 0.0, 1.0);
            c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
        },
    );
    let frame_old = bench(
        "一帧（M9 之前：鸟瞰那份是原图）",
        big_iters,
        |c| {
            c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
            c.draw_image(&big, win_rect, Fit::Fill, 0.0, 1.0);
            c.draw_image(&big, nav_rect, Fit::Fill, 0.0, 1.0);
        },
    );
    let frame_drag2 = bench("一帧（拖鸟瞰@200%）", big_iters, |c| {
        c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
        c.draw_image(&big, Rect::new(-400, -300, 1600, 1200), Fit::Fill, 0.0, 1.0);
        c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
    });
    let frame_drag4 = bench("一帧（拖鸟瞰@400%）", big_iters, |c| {
        c.fill_rect(0.0, 0.0, WIN_W as f32, WIN_H as f32, &bg);
        c.draw_image(
            &big,
            Rect::new(-1200, -900, 3200, 2400),
            Fit::Fill,
            0.0,
            1.0,
        );
        c.draw_image(&nav_small, nav_rect, Fit::Fill, 0.0, 1.0);
    });
    for s in [&frame_now, &frame_old, &frame_drag2, &frame_drag4] {
        report(s);
    }

    println!(
        "
──────── 折算（单核占用 = 每帧 ms × 每秒帧数 ÷ 10）────────"
    );
    for fps in [30u32, 60, 125] {
        println!(
            "  每秒 {fps:>3} 帧：生产（pixelated@200% 一帧） ≈ {:>5.1}% ｜ 双线性整源一帧 ≈ {:>5.1}% ｜ 可见区切片一帧 ≈ {:>5.1}%",
            cpu_pct(prod_frame.mid, fps),
            cpu_pct(frame_drag2.mid, fps),
            cpu_pct(frame_b4.mid, fps)
        );
    }
    println!(
        "
读法：任务管理器那个百分比是**整机**口径（多核会摊薄），所以这里算的是单核上限；
两边对不上时先怀疑分母（核数、以及呈现那一半没被本探针覆盖）。
本探针只量『画』。『呈现』那半（分层窗口每帧整窗预乘拷贝 + UpdateLayeredWindow）
要真窗口才碰得到——判据是把不透明度设到 100%（那时 `WS_EX_LAYERED` 被摘掉，
那条路整个不走）后再快拖一次，两边一比就知道 5% 里谁占大头。"
    );
}
