//! 放大清晰度探针（无 GUI）：真实解码线程（解码器 DLL）+ `Viewer` 状态机，回答一个问题——
//!
//! **「放大之后，宿主到底有没有去解更多像素？」**
//!
//! 起因（2026-10-17 用户真机）：打开 105 MB 的 6055×6055 TIFF，放大后画面不清晰，怀疑被
//! 「阉割」。宿主的既定设计是**按需解码**：首解只按视口档（省内存、省时间），缩放停止 120ms 后
//! 按 `natural × 倍率` 重解（[`rupix_lib::core::viewer::Viewer::desired_pixels`]）。这条链任何
//! 一环断了，表现都是「放大只会糊」——而它恰好是**看不见的那类 bug**：画面有内容，只是不清。
//!
//! 运行（需要一块真大图；第 2 个参数起是同级大样本，给了才跑阶段二）：
//!
//! ```text
//! RUPIX_DECODERS_DIR=<解码器目录> cargo run --release --example zoom_sharp_probe -- \
//!     <大图.tif> <同级大图2.jpg> <同级大图3.png>
//! ```
//!
//! 判据不是观感：三档各打印**已上屏位图的真实尺寸**，并断言
//! `适应窗口 < 200% < 1:1(=原图)` 严格递增；末档必须等于 `natural`（放大到头）。
//! 阶段二另测解码队列的头阻塞：主画面重解排在缩略图后面要等多久。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

use rupix_lib::core::config::Config;
use rupix_lib::core::decode::{Done, ReqKind};
use rupix_lib::core::thumbs;
use rupix_lib::core::viewer::Viewer;

/// 泵到"这一档该来的回包都来了"为止：每 20ms 收一次，收到回包后连续 `quiet` 时间
/// 再无新包就算落地；最多等 `max_wait`。
///
/// `expect` 是这一档**至少该有几个回包**（0 = 本来就不该有，1:1 已到头就是这么档）。
/// 不能省：静默判据在"一个包都还没来过"时也算成立，而 JPEG 首解要 1.5s，于是 0.5s
/// 就误判成"没解出来"（本探针初版栽在这里，实测把 jpeg_100M 报成首解失败）。
fn pump(
    v: &mut Viewer,
    rx: &std::sync::mpsc::Receiver<Done>,
    max_wait: Duration,
    quiet: Duration,
    expect: usize,
) -> Pump {
    let start = Instant::now();
    let mut last = Instant::now();
    let mut out = Pump::default();
    loop {
        if v.tick() {
            out.redraw += 1;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(done) => {
                out.got += 1;
                if v.apply(done) {
                    out.painted += 1;
                }
                last = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {
                let settled = last.elapsed() >= quiet && (out.got > 0 || expect == 0);
                if settled || start.elapsed() >= max_wait {
                    out.secs = start.elapsed().as_secs_f64();
                    return out;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                out.secs = start.elapsed().as_secs_f64();
                return out;
            }
        }
    }
}

/// 一轮泵包的结果：收到几个回包、上屏几次、花了多少秒。
///
/// `secs` 是**这一档从发起缩放到底不再变清晰**的墙钟时间——用户嘴里的"放大很糊"
/// 到底是永久状态还是"还没解完"，就看这个数。
#[derive(Default)]
struct Pump {
    got: usize,
    painted: usize,
    secs: f64,
    /// `Viewer::tick()` 回报"画面变了"的次数。C 之后本地换档也换图，而这个项目
    /// 静止不重绘——**这个数漏了就是"明明派生出来却不刷新"**（见 runtime 的 `arrived`）。
    redraw: usize,
}

/// 位图里的像素数（判据用它比大小，别用 `w*h` 现场乘——三处口径要一致）。
fn px(dims: (u32, u32)) -> u64 {
    dims.0 as u64 * dims.1 as u64
}

/// 在当前上屏位图尺寸（`(0, 0)` = 还没图）。
fn painted(v: &Viewer) -> (u32, u32) {
    v.image().map(|i| (i.width, i.height)).unwrap_or((0, 0))
}

/// 直接向解码线程发一个请求并等它的回包，返回（耗时, 位图宽）。
///
/// 不经 `Viewer`：阶段二要量的是"这次解码本身多少钱"和"它在队列里排了多久"，
/// 掺进防抖/闸门就读不到真数了。
fn one(
    d: &rupix_lib::core::decode::Decoder,
    rx: &std::sync::mpsc::Receiver<Done>,
    path: &Path,
    w: u32,
    h: u32,
    kind: ReqKind,
) -> (f64, u32) {
    let t = Instant::now();
    let seq = d.request_frame(path, w, h, 0, kind);
    loop {
        match rx.recv_timeout(Duration::from_secs(600)) {
            Ok(done) if done.seq == seq => {
                let secs = t.elapsed().as_secs_f64();
                return match done.out {
                    Ok(dd) => (secs, dd.bitmap.width),
                    Err(e) => {
                        println!("   ✗ 解码失败：{e}");
                        (secs, 0)
                    }
                };
            }
            // 队列里更早的请求的回包：与本请求无关，丢掉继续等。
            Ok(_) => {}
            Err(_) => return (t.elapsed().as_secs_f64(), 0),
        }
    }
}

/// 阶段二：解码队列的**头阻塞**——主画面重解排在缩略图后面要等多久。
///
/// 出队自 2026-10-17 起是**主画面插队**（`decode::Queue::pop`）；这一阶段量的是
/// 插队前后的差别，也是"为什么必须有插队"的证据：修前 2 张缩略图在前时主图重解
/// 0.37s → 1.98s，修后同场景 0.25s（本仓历史记录 §七）。
///
/// 顺带钉一条实测事实：**本解码器集不理 `target_w/target_h`**——同一张 TIFF 的
/// 240×180 缩略图与全尺寸主图耗时几乎相同，都是"整图解一遍再由宿主 `fit_to` 缩小"。
/// 所以按需解码省的是内存，不是 CPU，缩放必然带来一次全量重解。
fn queue_blocking(tiff: &Path, siblings: &[PathBuf]) {
    println!("\n阶段二：解码队列头阻塞");
    let (tx, rx) = channel::<Done>();
    let d = std::rc::Rc::new(rupix_lib::core::decode::Decoder::spawn_with(tx));

    let (t_base, w_base) = one(&d, &rx, tiff, 12110, 12110, ReqKind::Main);
    println!("  ① 光板上解一次全尺寸主图：{t_base:.2}s（位图宽 {w_base}）");

    let (t_thumb, w_thumb) = one(
        &d,
        &rx,
        tiff,
        thumbs::THUMB_W,
        thumbs::THUMB_H,
        ReqKind::Thumb,
    );
    println!(
        "  ② 同一张的缩略图（{}×{}）：{t_thumb:.2}s（位图宽 {w_thumb}）",
        thumbs::THUMB_W,
        thumbs::THUMB_H
    );

    if siblings.is_empty() {
        println!("  （缺同级大样本，跳过排队对照）");
        return;
    }
    // 先把同级大图的缩略图塞进队列（`thumbs::MAX_INFLIGHT` = 3，是私有常量，这里同值），
    // 再发主画面全尺寸请求，量它从**提交**到**回包**的墙钟——这就是用户缩放后要盯着
    // 糊图的时间。
    const INFLIGHT: usize = 3;
    let n = siblings.len().min(INFLIGHT);
    for p in &siblings[..n] {
        d.request_frame(p, thumbs::THUMB_W, thumbs::THUMB_H, 0, ReqKind::Thumb);
    }
    let t0 = Instant::now();
    let seq = d.request_frame(tiff, 12110, 12110, 0, ReqKind::Main);
    let mut got = None;
    while let Ok(done) = rx.recv_timeout(Duration::from_secs(600)) {
        if done.seq == seq {
            got = Some(done.out.is_ok());
            break;
        }
    }
    let blocked = t0.elapsed().as_secs_f64();
    println!(
        "  ③ {n} 张缩略图在前 + 主图全尺寸：主画面等到回包 {blocked:.2}s（{:?}，是①的 {:.1} 倍）",
        got,
        if t_base > 0.0 { blocked / t_base } else { 0.0 }
    );
}

/// 阶段三：**阶梯缩放不重复整解**（C 想消掉的就是这个）。
///
/// 从"适应窗口"逐级放大到 30% / 50% / 80%（都还在原图档以内）。按老规矩，每一级都要
/// 回头整解一次：`desired = natural × 倍率`，解码器又不理 `target`，于是**每级都是整图**
/// 解码——四步就是四步整解，谁在中间停住谁就白等。按 C 的规矩，第一步升级完原图档
/// 之后，后面几级只是本地换档，一个回包都不该有。
///
/// 这条比 `zoom_sharp_probe` 主链的 200% 跳档更能说明问题：200% 直接过 100%，
/// 两种规矩下都只解两次，区分不出来。
/// 返回"本阶段的判据是否全过"（失败条由 main 计入退出码）。
fn ladder(path: &Path) -> bool {
    let mut ok_all = true;
    println!(
        "
阶段三：阶梯缩放（30% → 50% → 80%，都在原图档以内）"
    );
    let (tx, rx) = channel::<Done>();
    let decoder = std::rc::Rc::new(rupix_lib::core::decode::Decoder::spawn_with(tx));
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
    let mut v = Viewer::new(decoder, cfg);
    v.on_viewport(0.0, 0.0, 1200.0, 900.0, 1.0);
    v.open(path);
    // 先把"适应窗口"那一档落地（首解按视口档），后面的计时才干净。
    pump(
        &mut v,
        &rx,
        Duration::from_secs(180),
        Duration::from_millis(400),
        1,
    );
    let Ok(nat) = rupix_lib::core::decode::probe_decode(path, 0, 0) else {
        println!("  （跳过：取不到原图尺寸）");
        return true;
    };
    let natural = nat.natural;

    let mut packets = 0usize;
    let mut new_total = 0f64;
    for z in [0.3f32, 0.5, 0.8] {
        v.zoom_at_center(z);
        // 期望的上屏档：`natural × 倍率 × dpi` 向上取整（dpi=1），且绝不超过原图。
        let want_w = ((natural.0 as f32 * z).ceil() as u32).min(natural.0);
        let p = pump_until_width(&mut v, &rx, want_w, Duration::from_secs(180));
        packets += p.got;
        new_total += p.secs;
        println!(
            "  {}%：等到上屏位图宽 {want_w} 用了 {:.2}s（收到 {} 包，重绘信号 {} 次）",
            (z * 100.0) as u32,
            p.secs,
            p.got,
            p.redraw
        );
        if z > 0.3 {
            // 第二级起是纯本地换档：必须有重绘信号，否则屏幕停在旧位图上
            // （C 把这活儿从解码线程挪到了 UI 线程，重绘的账也得跟着挪）。
            let ok = p.redraw >= 1;
            ok_all &= ok;
            println!(
                "  {} {}% 这一档要给出重绘信号（实测 {} 次）",
                if ok { "✓" } else { "✗" },
                (z * 100.0) as u32,
                p.redraw
            );
        }
    }
    println!("  阶梯内解码回包合计：{packets}（C 的期望是 1：第一次升级原图档，之后全本地）");

    // 配对：老规矩每一级的等待 = 120 ms 防抖 + **该档一次整解**（解码器不理 target，
    // 1816 的框和 6055 的框解的是同一张整图）。整解时间就地实测，不靠推理。
    let (tx2, rx2) = channel::<Done>();
    let d2 = std::rc::Rc::new(rupix_lib::core::decode::Decoder::spawn_with(tx2));
    let debounce = 0.12f64;
    let mut old_total = 0f64;
    let mut old_pk = 0usize;
    for z in [0.3f32, 0.5, 0.8] {
        let (secs, _) = one(
            &d2,
            &rx2,
            path,
            ((natural.0 as f32 * z).ceil() as u32).min(natural.0),
            ((natural.1 as f32 * z).ceil() as u32).min(natural.1),
            ReqKind::Main,
        );
        old_total += debounce + secs;
        old_pk += 1;
    }
    println!(
        "  配对：老规矩 {old_pk} 次整解 ≈ {old_total:.2}s；C 实测 {new_total:.2}s（1 次整解 + 2 次本地换档）"
    );
    ok_all
}

/// 泵到"上屏位图宽 = `want_w`"为止，返回 `(耗时, 期间收到的回包数)`。
///
/// 量的是**用户在等的那件事**（画面到这一档），不是"静默了多久"：后者会把探测窗
/// 算进耗时，跟老规矩的直接解码计时不对等（本探针前一版就因此得出反过来的结论）。
fn pump_until_width(
    v: &mut Viewer,
    rx: &std::sync::mpsc::Receiver<Done>,
    want_w: u32,
    max_wait: Duration,
) -> Pump {
    let start = Instant::now();
    let mut out = Pump::default();
    loop {
        if v.tick() {
            out.redraw += 1;
        }
        if painted(v).0 == want_w {
            out.secs = start.elapsed().as_secs_f64();
            return out;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(done) => {
                out.got += 1;
                v.apply(done);
            }
            Err(RecvTimeoutError::Disconnected) => {
                out.secs = start.elapsed().as_secs_f64();
                return out;
            }
            Err(RecvTimeoutError::Timeout) => {
                if start.elapsed() >= max_wait {
                    out.secs = start.elapsed().as_secs_f64();
                    return out;
                }
            }
        }
    }
}

/// 阶段四：**本地降采样**要多少钱。
///
/// 这是"解码一次原图档、缩放档位由宿主本地派生"这条路线的可行性判据：拿
/// `queue_blocking` 的 ① 当对照（一次整图解码），若 `fit_to` 只要几十毫秒，缩放
/// 就根本不该再回头找解码器。`fit_to` 按值吃缓冲，所以连那份拷贝一起计时——本地派生
/// 路线同样要为它付钱。
/// 阶段三：本地降采样成本（对照：一次整图解码）。
fn resample_cost(path: &Path) {
    println!("\n阶段三：本地降采样成本（对照：一次整图解码）");
    let Ok(d) = rupix_lib::core::decode::probe_decode(path, 0, 0) else {
        println!("  （跳过：取不到原图档）");
        return;
    };
    let (w, h) = (d.bitmap.width, d.bitmap.height);
    println!(
        "  原图档 {w}×{h} = {:.0} MB RGBA",
        d.bitmap.rgba.len() as f64 / 1048576.0
    );
    for (tw, th) in [(1280u32, 960u32), (2400, 1800), (4096, 3072)] {
        let t = Instant::now();
        let out = rupix_lib::core::geometry::fit_to(d.bitmap.rgba.clone(), w, h, tw, th);
        println!(
            "  → {tw}×{th}：{:.0}ms（出图 {}×{}）",
            t.elapsed().as_secs_f64() * 1000.0,
            out.width,
            out.height
        );
    }
}

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r".temp\big-samples\tiff\100M\tiff_100M.tif"));
    if !path.is_file() {
        eprintln!(
            "样本不存在: {}（用法：cargo run --release --example zoom_sharp_probe -- <大图路径>）",
            path.display()
        );
        std::process::exit(2);
    }
    println!("样本：{}", path.display());

    let (tx, rx) = channel::<Done>();
    let decoder = std::rc::Rc::new(rupix_lib::core::decode::Decoder::spawn_with(tx));
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
    let mut v = Viewer::new(decoder, cfg);

    // 视口取 1200×900（dpi 1.0）：与常见笔记本窗口同量级，足以让三档尺寸拉开差距。
    v.on_viewport(0.0, 0.0, 1200.0, 900.0, 1.0);
    v.open(&path);
    let p0 = pump(
        &mut v,
        &rx,
        Duration::from_secs(180),
        Duration::from_millis(500),
        1,
    );
    println!(
        "   首解：收到 {} 包、上屏 {} 次、位图宽 {}，耗时 {:.1}s",
        p0.got,
        p0.painted,
        painted(&v).0,
        p0.secs
    );

    // 原图尺寸单独问一次（`probe_decode` 的 box 传 0 = 不缩，返回权威 natural）。
    let t = Instant::now();
    let natural = match rupix_lib::core::decode::probe_decode(&path, 0, 0) {
        Ok(d) => d.natural,
        Err(e) => {
            println!("✗ 取不到原图尺寸：{e}");
            std::process::exit(1);
        }
    };
    println!("   全尺寸直解耗时 {:.1}s", t.elapsed().as_secs_f64());
    if painted(&v).0 == 0 {
        println!(
            "✗ 首解没落地（status：{}，error：{:?}）",
            v.status(),
            v.error_text()
        );
        std::process::exit(1);
    }
    let fit = painted(&v);
    println!(
        "① 适应窗口：倍率 {:.3} → 上屏位图 {}×{}（原图 {}×{}）",
        v.scale(),
        fit.0,
        fit.1,
        natural.0,
        natural.1
    );
    // 宿主认定的"原图尺寸"单独打一行：它来自 `Decoded.natural`，而那个字段是解码器
    // 渲染回包的 `actual_w/h`。若这里比真实原图小，说明解码器按 target 缩了解，宿主
    // 把缩过的尺寸当成了原图 —— `at_native` 闸会当场把重解判成"到头"。
    println!("   宿主认知的原图：{:?}", v.info_dims());

    v.zoom_at_center(2.0);
    let p2 = pump(
        &mut v,
        &rx,
        Duration::from_secs(180),
        Duration::from_millis(500),
        1,
    );
    let z2 = painted(&v);
    println!(
        "② 200%：倍率 {:.3} → 上屏位图 {}×{}（{} 包、上屏 {} 次、耗时 {:.1}s，error {:?}）",
        v.scale(),
        z2.0,
        z2.1,
        p2.got,
        p2.painted,
        p2.secs,
        v.error_text()
    );

    v.zoom_actual();
    // 1:1 这一档通常**不该**有新回包（200% 已解到原图，`at_native` 闸收下）；静默窗
    // 放到 2s，真有迟到的重解回包也来得及收进同一轮。
    let p1 = pump(
        &mut v,
        &rx,
        Duration::from_secs(180),
        Duration::from_millis(2000),
        0,
    );
    let one = painted(&v);
    println!(
        "③ 1:1：倍率 {:.3} → 上屏位图 {}×{}（{} 包、上屏 {} 次、耗时 {:.1}s，error {:?}）",
        v.scale(),
        one.0,
        one.1,
        p1.got,
        p1.painted,
        p1.secs,
        v.error_text()
    );

    // ④⑤ C 的**签名特征**：整图档到位之后，缩放一档不再有任何解码回包，
    //    而上屏位图确实跟着档位走（先缩到 50%，再回到 1:1）。
    //    判据是"收包数 = 0 且尺寸变了"——只断言尺寸变不变，区分不出"本地重采样"与
    //    "又整解一次"，那正是本轮要消掉的东西。
    v.zoom_at_center(0.5);
    let p4 = pump(
        &mut v,
        &rx,
        Duration::from_secs(30),
        Duration::from_millis(500),
        0,
    );
    let half = painted(&v);
    println!(
        "④ 缩回 50%：上屏位图 {}×{}（收到 {} 包）",
        half.0, half.1, p4.got
    );

    v.zoom_actual();
    let p5 = pump(
        &mut v,
        &rx,
        Duration::from_secs(30),
        Duration::from_millis(500),
        0,
    );
    let back = painted(&v);
    println!(
        "⑤ 回到 1:1：上屏位图 {}×{}（收到 {} 包）",
        back.0, back.1, p5.got
    );

    let mut failed = 0;
    let mut check = |ok: bool, label: String| {
        println!("  {}", if ok { "✓ " } else { "✗ " }.to_string() + &label);
        if !ok {
            failed += 1;
        }
    };

    // 判据零：宿主认知的原图尺寸必须是真的（`at_native` 闸与 `desired_pixels` 都靠它）。
    check(
        v.info_dims() == Some(natural),
        format!(
            "宿主认定的原图必须等于真实原图 {}×{}，实测 {:?}",
            natural.0,
            natural.1,
            v.info_dims()
        ),
    );
    // 判据一：放大必须真的要到**更多像素**（这是"有没有被阉割"的直接答案）。
    check(
        px(z2) > px(fit),
        format!(
            "200% 的位图像素数必须大于适应窗口档（{} > {}）",
            px(z2),
            px(fit)
        ),
    );
    // 判据二：1:1 必须到原图尺寸（放大到头 = 逐像素）。
    check(
        one.0 == natural.0 && one.1 == natural.1,
        format!(
            "1:1 必须等于原图 {}×{}，实测 {}×{}",
            natural.0, natural.1, one.0, one.1
        ),
    );
    // 判据三：倍率与位图同步（`desired_pixels` 用 natural×scale，位图不得反向缩水）。
    check(
        px(one) >= px(z2),
        "1:1 的位图不得比 200% 更小（档位单调）".to_string(),
    );

    // 判据四：整图到位后缩放**不再发解码**（C 的签名），但位图确实换了档。
    check(
        p4.got == 0,
        format!("50% 那一档不该有解码回包，实测 {} 包", p4.got),
    );
    check(
        half.0 > 0 && px(half) < px(one),
        format!(
            "位图要本地缩到 50% 档（{} < {}），否则派生没跑起来",
            px(half),
            px(one)
        ),
    );
    check(
        p5.got == 0 && back.0 == natural.0 && back.1 == natural.1,
        format!(
            "回 1:1 应零解码直接恢复原图档，实测 {} 包 / {}×{}",
            p5.got, back.0, back.1
        ),
    );
    println!(
        "   整程解码回包：首解 {} + 升级 {} + 缩 {} + 回原 {} = {}（大图理想值 2）",
        p0.got,
        p2.got,
        p4.got,
        p5.got,
        p0.got + p2.got + p4.got + p5.got
    );

    if failed == 0 {
        println!("zoom_sharp_probe: PASS（按需重解链路成立）");
    } else {
        println!(
            "zoom_sharp_probe: FAIL {} 条（放大没拿到更多像素 = 阉割级缺陷）",
            failed
        );
        std::process::exit(1);
    }

    // 阶段二只在给了同级大样本时跑（第 2 个及以后的命令行参数）；阶段三随时能跑。
    let siblings: Vec<PathBuf> = std::env::args().skip(2).map(PathBuf::from).collect();
    queue_blocking(&path, &siblings);
    if !ladder(&path) {
        println!("zoom_sharp_probe: FAIL（阶梯阶段判据没过）");
        std::process::exit(1);
    }
    resample_cost(&path);
}
