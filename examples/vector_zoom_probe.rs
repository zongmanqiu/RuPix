//! 真机回归探针：矢量放大后显示不全（2026-10-28 用户报障）。
//!
//! 现象：打开 WMF 放大后，画布只显示文档的一角，且与鸟瞰缩略图里标出的视口位置对不上。
//!
//! 根因（2026-10-28 定案）：宿主把**这一次渲染出来的画布尺寸**当成了**文档自然尺寸**
//! （`Decoded::natural`），而两处都吃这个值——画布几何（`eff_dims` → `dst_rect`）与
//! **区域请求的坐标系**（ABI v6 明写：区域的 x/y/w/h 单位 = 文档自然像素）。
//! 坐标系一错，宿主要的那块与真图那块就不是同一块 ⇒ 放大后显示的是文档的**别处**。
//!
//! 本探针走**生产链路**（真实解码器 DLL + 真实 `Viewer`）复现并裁决：
//! 1. 文档自然尺寸的真值 = `target=0` 那次解码（契约：0 = 要原始尺寸）；
//! 2. 打开文件 → 首解回包里的 `natural` 必须等于那个真值（坐标系的锚）；
//! 3. 放大到 4000% → 让 `Viewer` 自己发区域请求 → 拿到切片；
//! 4. 把切片与**真文档同一处**的裁块逐像素比对（最近邻重采样，不引入插值误差）。
//!
//! 判词：
//! - `natural` ≠ 文档自然尺寸 ⇒ 坐标系错位（本次报障的根因还在）；
//! - 切片内容与真文档对不上 ⇒ 显示的是文档的别处（显示不全）；
//! - 两条都对 ⇒ PASS。

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use rupix_lib::core::config::Config;
use rupix_lib::core::decode::{Decoder, Done};
use rupix_lib::core::viewer::Viewer;

/// 视口尺寸（逻辑像素）与放大倍率（1.0 = 100%）。
const VIEW_W: f32 = 557.0;
const VIEW_H: f32 = 561.0;
const DPI: f32 = 1.0;
/// 4000%。
const ZOOM: f32 = 40.0;
/// 像素比对的容差：矢量重渲与基准整幅之间只该差抗锯齿。
const TOLERANCE: f64 = 0.05;

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    let args: Vec<String> = std::env::args().collect();
    let Some(p) = args.get(1) else {
        println!("用法：vector_zoom_probe <矢量文件>");
        return;
    };
    let path = Path::new(p);

    // ── 0. 文档自然尺寸（真值）：target = 0 = 要原始尺寸 ─────────────────
    let truth = match rupix_lib::core::decode::probe_decode(path, 0, 0) {
        Ok(d) => d,
        Err(e) => {
            println!("✗ 整幅解码失败：{e}");
            return;
        }
    };
    let doc = truth.natural;
    println!("══ 文档自然尺寸（真值）= {doc:?} ══");

    // ── 1. 走真实 Viewer：打开 → 首解 → 放大 → 区域请求 ──────────────────
    let (tx, rx) = mpsc::channel::<Done>();
    let cfg = Rc::new(RefCell::new(Config::default()));
    // 别让邻图预读插进来搅浑水（它会往同一个通道里塞回包）。
    cfg.borrow_mut().prefetch = false;
    let mut v = Viewer::new(Rc::new(Decoder::spawn_with(tx)), cfg.clone());
    v.open(path);
    v.on_viewport(0.0, 0.0, VIEW_W, VIEW_H, DPI);

    let Some(first) = recv(&rx) else {
        println!("✗ 首解没有回包");
        return;
    };
    let (believed, bw, bh) = match &first.out {
        Ok(d) => (d.natural, d.bitmap.width, d.bitmap.height),
        Err(e) => {
            println!("✗ 首解失败：{e}");
            return;
        }
    };
    println!("宿主首解：bitmap={bw}x{bh}  natural={believed:?}");
    v.apply(first);

    // ── 2. 放大到 4000%，让 Viewer 自己发区域请求 ────────────────────────
    let c = v.center();
    v.zoom_to_at(ZOOM, c, c);
    // 调度有 120ms 防抖（滚轮停了才动画面），这里等它过去。
    std::thread::sleep(Duration::from_millis(220));
    let _ = v.tick();

    let Some(second) = recv(&rx) else {
        println!("✗ 区域请求没有回包");
        return;
    };
    let d2 = match &second.out {
        Ok(d) => d,
        Err(e) => {
            println!("✗ 区域渲染失败：{e}");
            return;
        }
    };
    let Some(ref region) = d2.region else {
        println!("✗ 回包不是区域切片（region = None）");
        return;
    };
    println!(
        "区域回包：画布={}x{}  origin=({:.1}, {:.1})  密度={:.2}",
        d2.bitmap.width, d2.bitmap.height, region.origin.0, region.origin.1, region.density
    );

    // ── 3. 对账：切片的内容必须是真文档里它被摆放的那一块 ────────────────
    // 宿主把区域坐标当成**它以为的**文档像素；真文档里的位置要按两者的比例换算。
    // 比例 = 1 说明宿主的 natural 就是文档自然尺寸（正确）；比例明显 ≠ 1 说明
    // 坐标系错位，宿主摆的位置与解码器裁的位置压根不是同一处。
    let kx = doc.0 as f32 / believed.0.max(1) as f32;
    let ky = doc.1 as f32 / believed.1.max(1) as f32;
    println!("坐标系比例：真文档/宿主认知 = {kx:.3} × {ky:.3}（= 1 才是对的）");

    let base = &truth.bitmap.rgba;
    let slice = &d2.bitmap.rgba;
    let sw = d2.bitmap.width as usize;
    let sh = d2.bitmap.height as usize;
    let mut diff = 0usize;
    let mut total = 0usize;
    let mut max_delta = 0i32;
    for j in 0..sh {
        for i in 0..sw {
            let bx = ((region.origin.0 + i as f32 / region.density) * kx).floor() as isize;
            let by = ((region.origin.1 + j as f32 / region.density) * ky).floor() as isize;
            if bx < 0 || by < 0 || bx >= doc.0 as isize || by >= doc.1 as isize {
                continue;
            }
            let si = (by as usize * doc.0 as usize + bx as usize) * 4;
            let di = (j * sw + i) * 4;
            let mut d = 0i32;
            for c in 0..3 {
                d += (slice[di + c] as i32 - base[si + c] as i32).abs();
            }
            total += 1;
            if d > 30 {
                diff += 1;
            }
            if d > max_delta {
                max_delta = d;
            }
        }
    }
    let pct = if total == 0 {
        0.0
    } else {
        diff as f64 * 100.0 / total as f64
    };

    println!("\n══ 裁决 ══");
    println!("  比对像素 {total} 个，不一致 {diff} 个（{pct:.2}%），最大通道差 {max_delta}");
    let natural_ok = (kx - 1.0).abs() < 0.01 && (ky - 1.0).abs() < 0.01;
    let content_ok = total > 0 && pct <= TOLERANCE * 100.0;
    if !natural_ok {
        println!("  ✗ 宿主认知的文档尺寸 ≠ 真文档 ⇒ 区域坐标系错位（本次报障的根因）");
    }
    if !content_ok {
        println!("  ✗ 切片内容与真文档对不上 ⇒ 放大后显示的是文档的别处（显示不全）");
    }
    if natural_ok && content_ok {
        println!("  ⇒ PASS：坐标系是文档自然像素，切片内容与真文档一致");
    }
    v.apply(second);
}

/// 从通道里取一份回包（带超时，别把探针挂死）。
fn recv(rx: &mpsc::Receiver<Done>) -> Option<Done> {
    rx.recv_timeout(Duration::from_secs(60)).ok()
}
