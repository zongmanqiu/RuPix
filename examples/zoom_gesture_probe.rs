//! A2 缩放手势探针（无 GUI）：真解码线程（解码器 DLL）+ 真 `Viewer` 状态机 + 真派生线程，
//! 量的是**一次连续滚轮手势里画面换了几档、每一档糊不糊、停手后多久清晰**。
//!
//! 起因（`docs/待办.md` §13「A2」）：当年写下"`mark_need` 首次置钟不重置 ⇒ 每跨一个 120 ms
//! 窗口插一次全量重解 ⇒ **糊→清晰来回切换**"，并留了一句候选修法"改成标准 trailing debounce
//! 只动一行（`want_at` 从'下一次触发钟'变成'每次事件推钟'）"，代价是"松手后的清晰度反而延后
//! 120 ms"。当年判"先度量再决定"，**度量从未做**。这一轮就是补那次没做的度量。
//!
//! 跑法（在 `main/` 下；给样本路径当参数，默认那块 100 MB TIFF）：
//!
//! ```text
//! RUPIX_DECODERS_DIR=E:/tmp/AI.Agent/RuPix/.build/decoders \
//!   cargo run --release --example zoom_gesture_probe -- ../.temp/big-samples/tiff/100M/tiff_100M.tif
//! ```
//!
//! ★ 判据不是"我觉得"，是三件事，每条都有明确的"成立/不成立"：
//! 1. **"来回切换"这个说法本身**：把手势期间每一次上屏换档按时间排出来，看清晰度比值
//!    `上屏位图宽 ÷ 当时需要的档宽` 有没有**从清晰侧掉进糊侧**（糊线 `SHARP`，与糊面时长同一条）。
//!    若从不掉下去，那"糊→清晰**来回**"就是文档说谎（与 m45 那批同形），trailing debounce
//!    的收益前提当场塌掉。★ 这条判据在 m67 改过一次：原先写的是"比值单调不降"，而派生快到
//!    46 ms 之后序列里出现了 `1.10 → 1.00`（过采样收回到刚好够用，画面没糊）却被判成回退——
//!    判据必须落在"看得见的糊"上，不能落在比值这个中间量上。
//! 2. **停手后多久清晰**：从最后一个滚轮事件到最后一档落地的墙钟。这是 trailing 唯一会
//!    改动的终点数（它只会把它推后，不会推前）。
//! 3. **每次换档值多少钱**：同一份源、同一个目标档的派生成本就地实测（`fit_to` 直测），
//!    用它把手势时长除以成本 = 这段路最多算了几档（**推算**，不是实测的线程计数）。
//!
//! 中间那条派生线程本身已有"吞旧任务"的合流（`core/derive.rs`：取任务时把队列里更新的都
//! 吞掉，只算最新一条；`poll_levels` 又把"目标档又变了"的回包判过期并重新约一次防抖）——
//! 这两条机制决定了 leading 形态下的浪费上界，本探针量的是它们在真实手势下的实际效果。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, TryRecvError};
use std::time::{Duration, Instant};

use rupix_lib::core::config::Config;
use rupix_lib::core::decode::Done;
use rupix_lib::core::derive::LevelCounts;
use rupix_lib::core::geometry::fit_dims;
use rupix_lib::core::viewer::Viewer;

/// 判"糊"的那条线：`上屏位图宽 ÷ 当时需要的档宽` 低于它，画面就不够清晰。
/// 本探针里**所有**清晰度判据共用这一条（糊面时长、清晰→糊回退），不许两处各写一个数。
const SHARP: f64 = 0.95;

/// 与生产 `Viewer::desired_pixels` + `tick` 同式子的**公开复刻**（那两个是私有的）。
/// `l.decoded == natural`（升级已落地）时二者等价，本探针只在升级落地后起手。
fn needed_w(natural: (u32, u32), scale: f32, dpi: f32) -> u32 {
    let want = (natural.0 as f32 * scale * dpi)
        .ceil()
        .clamp(1.0, natural.0 as f32) as u32;
    let want_h = (natural.1 as f32 * scale * dpi)
        .ceil()
        .clamp(1.0, natural.1 as f32) as u32;
    fit_dims(natural.0, natural.1, want, want_h).0
}

/// 一次换档的现场快照：时刻、上屏位图宽、当时需要的档宽、倍率。
struct Landing {
    at_ms: f64,
    painted_w: u32,
    needed_w: u32,
    scale: f32,
}

impl Landing {
    /// 清晰度比值：`< 1` = 上屏这一档比画面需要的少像素（要放大画 = 糊）。
    fn ratio(&self) -> f64 {
        if self.needed_w == 0 {
            return 1.0;
        }
        self.painted_w as f64 / self.needed_w as f64
    }
}

/// 一次手势的读数。
struct Gesture {
    label: &'static str,
    landings: Vec<Landing>,
    /// 手势事件跨度（第一个到最后一个事件）。
    span_ms: f64,
    /// 最后一个事件到最后一档落地（= 用户"停手后等清晰"）。
    settle_ms: Option<f64>,
    /// 期间收到的**解码**回包数（升级已落地后应为 0；不是 0 就说明这条手势又踢了一次解码器）。
    decode_pkts: usize,
    /// 采样期间"画面不够清晰"（上屏像素 < 需要的 95%）的时长，与总采样时长一起算占比。
    blur_ms: f64,
    sampled_ms: f64,
    /// 这一段手势内派生线程的**真实**计数（⑫）。m66 只有"手势时长 ÷ 一次派生成本"
    /// 那条推算，线程实际算了几档、几档被合流吞掉、几档算完才发现画面不要，全读不出来。
    counts: LevelCounts,
}

impl Gesture {
    /// 有没有"清晰 → 糊"的**可见回退**：上一步还在清晰侧（≥ [`SHARP`]）、这一步掉进糊侧。
    ///
    /// ★ 判据从"比值单调不降"改成"跨过糊线"是 m67 逼出来的：派生降到 46 ms 之后，
    ///   序列里第一次出现 `1.10 → 1.00`（过采样收回到刚好够用）。两步都在清晰侧，
    ///   画面一分没糊，但旧判据会给它打 ★。m66 那次量不到这一步——派生 103–215 ms 时
    ///   比值一直挂在 1.1 以上，"单调"这条写在被测量程里根本没被走过。
    fn blur_transitions(&self) -> Vec<(usize, f64, f64)> {
        let mut out = Vec::new();
        for i in 1..self.landings.len() {
            let (a, b) = (self.landings[i - 1].ratio(), self.landings[i].ratio());
            if a >= SHARP && b < SHARP {
                out.push((i, a, b));
            }
        }
        out
    }
    /// 手势期间画面"不够清晰"的时间占比（用户视角的糊面时长，比换档次数更贴近体验）。
    fn blur_pct(&self) -> f64 {
        if self.sampled_ms <= 0.0 {
            return 0.0;
        }
        100.0 * self.blur_ms / self.sampled_ms
    }
}

fn painted_w(v: &Viewer) -> u32 {
    v.image().map(|i| i.width).unwrap_or(0)
}

/// 泵到"这一档不再变"为止（首解/升级用）：每 20 ms 收一次，静默窗内无新包且无换档就算落地。
fn settle(v: &mut Viewer, rx: &std::sync::mpsc::Receiver<Done>, quiet: Duration) {
    let start = Instant::now();
    let mut last = Instant::now();
    loop {
        let mut busy = false;
        if v.tick() {
            busy = true;
        }
        if v.poll_levels() {
            busy = true;
        }
        if let Ok(d) = rx.try_recv() {
            if v.apply(d) {
                busy = true;
            }
            last = Instant::now();
        }
        if busy {
            last = Instant::now();
        }
        if last.elapsed() >= quiet || start.elapsed() > Duration::from_secs(240) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 跑一次手势：按 `plan` 给的（相对第一个事件的毫秒偏移, delta）序列发滚轮事件，
/// 其余时间以 16 ms 一轮泵 `tick`/`poll_levels`/`apply`（与 `runtime` 的轮询同形）。
///
/// ★ 事件与泵都在**真实时间**里跑：`REDECODE_DEBOUNCE` 用的是 `Instant::now()`，
///   不可注入（那条只做了 `Chrome` 侧的滚轮闸注入），所以这里只能用真时钟。
fn run_gesture(
    v: &mut Viewer,
    rx: &std::sync::mpsc::Receiver<Done>,
    natural: (u32, u32),
    label: &'static str,
    plan: &[(u64, i32)],
) -> Gesture {
    let start = Instant::now();
    let c0 = v.level_counts();
    let mut landings = Vec::new();
    let mut pkts = 0usize;
    let mut ei = 0usize;
    let mut painted = painted_w(v);
    let mut last_event = start;
    let mut prev = start;
    let mut blur_ms = 0f64;
    let mut sampled_ms = 0f64;
    let anchor = (600.0f32, 450.0f32);
    let center = (600.0f32, 450.0f32);
    loop {
        let now = Instant::now();
        // 到点的滚轮事件（可能一次泵里到好几个，按序发）。
        while ei < plan.len() && plan[ei].0 <= now.duration_since(start).as_millis() as u64 {
            v.zoom_by_wheel(plan[ei].1, anchor, center);
            last_event = now;
            ei += 1;
        }
        // 换没换档由下面的"上屏位图宽度变了"判定，这里只负责把生产的那三步泵一遍。
        let _ = v.tick();
        let _ = v.poll_levels();
        match rx.try_recv() {
            Ok(d) => {
                pkts += 1;
                let _ = v.apply(d);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break,
        }
        let pw = painted_w(v);
        if pw != painted {
            painted = pw;
            landings.push(Landing {
                at_ms: now.duration_since(start).as_secs_f64() * 1000.0,
                painted_w: pw,
                needed_w: needed_w(natural, v.scale(), 1.0),
                scale: v.scale(),
            });
        }
        // 只在**事件还在进来**的那段采样"此刻画面够不够清晰"（比值低于 `SHARP` 记进糊面时长）。
        // 停手之后的尾巴不算进来，否则那段必然清晰的等待会把占比稀释掉。
        if ei < plan.len() {
            let need = needed_w(natural, v.scale(), 1.0);
            let r = if need == 0 {
                1.0
            } else {
                pw as f64 / need as f64
            };
            let dt = now.duration_since(prev).as_secs_f64() * 1000.0;
            sampled_ms += dt;
            if r < SHARP {
                blur_ms += dt;
            }
            prev = now;
        } else {
            prev = now;
        }
        if ei >= plan.len() && last_event.elapsed() > Duration::from_millis(2500) {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let span_ms = plan.last().map(|p| p.0 as f64).unwrap_or(0.0);
    let settle_ms = landings.last().map(|l| l.at_ms - span_ms);
    let c1 = v.level_counts();
    Gesture {
        label,
        landings,
        span_ms,
        settle_ms,
        decode_pkts: pkts,
        blur_ms,
        sampled_ms,
        counts: LevelCounts {
            posted: c1.posted - c0.posted,
            collapsed: c1.collapsed - c0.collapsed,
            computed: c1.computed - c0.computed,
            discarded: c1.discarded - c0.discarded,
        },
    }
}

/// 直测一次派生的成本：同一份源缓冲、同一目标档，跑 `fit_to`（`derive_level` 的重活就是它）。
fn derive_cost_ms(path: &Path, target_w: u32) -> Vec<f64> {
    let Ok(d) = rupix_lib::core::decode::probe_decode(path, 0, 0) else {
        println!("  （跳过：取不到原图档）");
        return Vec::new();
    };
    let (w, h) = (d.bitmap.width, d.bitmap.height);
    let (tw, th) = fit_dims(
        w,
        h,
        target_w,
        (target_w as f64 * h as f64 / w as f64) as u32,
    );
    let mut out = Vec::new();
    for _ in 0..3 {
        let buf = d.bitmap.rgba.to_vec();
        let t = Instant::now();
        let img = rupix_lib::core::geometry::fit_to(buf, w, h, tw, th);
        out.push(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&img);
    }
    println!(
        "  派生成本直测：{}×{} → {}×{}，三次 {:.0} / {:.0} / {:.0} ms",
        w, h, tw, th, out[0], out[1], out[2]
    );
    out
}

fn report(g: &Gesture, cost_ms: &[f64]) {
    println!(
        "\n手势 {}：事件跨度 {:.0} ms，换档 {} 次，解码回包 {} 包",
        g.label,
        g.span_ms,
        g.landings.len(),
        g.decode_pkts
    );
    for (i, l) in g.landings.iter().enumerate() {
        println!(
            "   {:>2}  t={:>7.0} ms  倍率 {:.2}  上屏 {:>5}  需要 {:>5}  清晰比 {:>5.2}",
            i + 1,
            l.at_ms,
            l.scale,
            l.painted_w,
            l.needed_w,
            l.ratio()
        );
    }
    let regs = g.blur_transitions();
    println!(
        "   清晰→糊回退：{}（判据①，糊线 = 比值 <{SHARP:.2}）",
        if regs.is_empty() {
            "无 —— 没有一次从上屏够用掉到不够用"
        } else {
            "有，见下"
        }
    );
    for (i, a, b) in &regs {
        println!("   ★ 第 {i} 次换档掉进糊侧：{a:.2} → {b:.2}");
    }
    // 顺带把"过采样收回"那几步如实报一行（它们不是缺陷，但读表的人需要知道比值为什么在降）。
    let withdrawn = g
        .landings
        .windows(2)
        .filter(|w| w[1].ratio() < w[0].ratio() - 0.02 && w[1].ratio() >= SHARP)
        .count();
    if withdrawn > 0 {
        println!("   （另有 {withdrawn} 步比值在降但两步都仍在清晰侧 = 过采样收回，不算糊）");
    }
    match g.settle_ms {
        Some(ms) => println!("   停手到最后一档落地：{ms:.0} ms（判据②，trailing 只会把它推后）"),
        None => println!("   停手到落地：本次没有换档（画面一档到底）"),
    }
    println!(
        "   实测计数（⑫）：投 {} 档，合流吞掉 {}，算完 {}，算完又被画面丢掉 {} ⇒ 白烧占比 {:.0}%",
        g.counts.posted,
        g.counts.collapsed,
        g.counts.computed,
        g.counts.discarded,
        if g.counts.computed == 0 {
            0.0
        } else {
            100.0 * g.counts.discarded as f64 / g.counts.computed as f64
        }
    );
    if let Some(c) = cost_ms.first() {
        let n = (g.span_ms / c).ceil();
        println!(
            "   推算（对照，非线程计数）：按一次派生 {:.0} ms，这 {:.0} ms 手势最多算 {} 档；实测换档 {} 次",
            c,
            g.span_ms,
            n.max(1.0),
            g.landings.len()
        );
    }
    println!(
        "   手势期间糊面占比：{:.1}%（{:.0} / {:.0} ms，判据=上屏像素<需要的 {:.0}%）",
        g.blur_pct(),
        g.blur_ms,
        g.sampled_ms,
        SHARP * 100.0
    );
}

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();
    let path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../.temp/big-samples/tiff/100M/tiff_100M.tif"));
    if !path.is_file() {
        eprintln!("样本不存在：{}", path.display());
        std::process::exit(2);
    }
    println!("样本：{}", path.display());

    let (tx, rx) = channel::<Done>();
    let decoder = std::rc::Rc::new(rupix_lib::core::decode::Decoder::spawn_with(tx));
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
    let mut v = Viewer::new(decoder, cfg);
    v.on_viewport(0.0, 0.0, 1200.0, 900.0, 1.0);
    v.open(&path);
    // 首解 + 原图档升级都落地，之后才谈"稳态缩放手势"（`derive` 那条本地换档路）。
    settle(&mut v, &rx, Duration::from_millis(600));
    let Some(natural) = v.info_dims() else {
        println!("✗ 首解没落地，测不了");
        std::process::exit(1);
    };
    println!(
        "原图 {}×{}，升级后上屏 {}（宽），倍率 {:.3}",
        natural.0,
        natural.1,
        painted_w(&v),
        v.scale()
    );

    // 三档派生成本：手势里大概会出现的目标档（1200 / 2400 / 3100 宽）。
    println!("\n判据③：派生成本（同进程直测，取第一次）");
    let c1200 = derive_cost_ms(&path, 1200);
    let c2400 = derive_cost_ms(&path, 2400);

    // 回到适应窗口再起手，三段手势的起点一致。
    v.set_fit(true);
    settle(&mut v, &rx, Duration::from_millis(600));
    println!(
        "\n起点复位：上屏 {}（宽），倍率 {:.3}",
        painted_w(&v),
        v.scale()
    );

    // ① 鼠标连滚 10 格，每 110 ms 一格（≈每秒 9 格，故意的最快速度）→ 约 2.6×。
    let g1 = run_gesture(
        &mut v,
        &rx,
        natural,
        "① 鼠标 10 格 @110 ms（单向放大到 ≈2.6×）",
        &(0..10).map(|i| (i * 110, 120)).collect::<Vec<_>>(),
    );
    report(&g1, &c2400);

    // ② 来回滚：放大 10 格后立刻反向缩 10 格（"来回切换"最容易出现的形状）。
    v.set_fit(true);
    settle(&mut v, &rx, Duration::from_millis(600));
    let mut plan2: Vec<(u64, i32)> = (0..10).map(|i| (i * 110, 120)).collect();
    plan2.extend((0..10).map(|i| ((10 + i) * 110, -120)));
    let g2 = run_gesture(
        &mut v,
        &rx,
        natural,
        "② 来回滚 10 格放大 + 10 格缩回",
        &plan2,
    );
    report(&g2, &c1200);

    // ③ 触控板快滚：25 ms 一次碎增量（delta 40 = 1/3 格），20 次 ≈ 1.9×。
    v.set_fit(true);
    settle(&mut v, &rx, Duration::from_millis(600));
    let g3 = run_gesture(
        &mut v,
        &rx,
        natural,
        "③ 触控板 20 次 @25 ms（碎增量）",
        &(0..20).map(|i| (i * 25, 40)).collect::<Vec<_>>(),
    );
    report(&g3, &c1200);

    // ④ 纯缩小（从 3.0 往下 10 格，每 110 ms 一格）：②的后半程"一次都不落地"到底是
    //    方向性的还是间隔性的，这一臂单独隔离出来。起点先滚到 3.0 并等它落清。
    v.set_fit(true);
    settle(&mut v, &rx, Duration::from_millis(600));
    for _ in 0..11 {
        v.zoom_by_wheel(120, (600.0, 450.0), (600.0, 450.0));
    }
    settle(&mut v, &rx, Duration::from_millis(600));
    println!(
        "\n④⑤ 起点：倍率 {:.3}，上屏 {}（宽）",
        v.scale(),
        painted_w(&v)
    );
    let g4 = run_gesture(
        &mut v,
        &rx,
        natural,
        "④ 纯缩小 10 格 @110 ms（目标档一路变小）",
        &(0..10).map(|i| (i * 110, -120)).collect::<Vec<_>>(),
    );
    report(&g4, &c1200);

    // ⑤ 纯缩小、但每格隔 400 ms（长到派生一定来得及）：若这一臂每次都落地，
    //    ④的"不落地"就是**投档间隔 ≈ 派生成本**时的过期死循环，而不是缩小的语义问题。
    v.set_fit(true);
    settle(&mut v, &rx, Duration::from_millis(600));
    for _ in 0..11 {
        v.zoom_by_wheel(120, (600.0, 450.0), (600.0, 450.0));
    }
    settle(&mut v, &rx, Duration::from_millis(600));
    let g5 = run_gesture(
        &mut v,
        &rx,
        natural,
        "⑤ 纯缩小 10 格 @400 ms（间隔长到派生必然来得及）",
        &(0..10).map(|i| (i * 400, -120)).collect::<Vec<_>>(),
    );
    report(&g5, &c1200);

    // 汇总：判据①的总线。
    let gestures = [&g1, &g2, &g3, &g4, &g5];
    let regs: usize = gestures.iter().map(|g| g.blur_transitions().len()).sum();
    println!(
        "\n汇总：五段手势的'清晰→糊'回退合计 {regs} 次（判据①，糊线 = 比值 <{SHARP:.2}）；解码回包合计 {} 包（升级已落地后应为 0）",
        gestures.iter().map(|g| g.decode_pkts).sum::<usize>()
    );
    for g in &gestures {
        let short: String = g.label.chars().take(1).collect();
        println!(
            "   {short}：换档 {:>2} 次，事件跨度 {:.0} ms，糊面占比 {:>5.1}%，停手到清晰 {:?}",
            g.landings.len(),
            g.span_ms,
            g.blur_pct(),
            g.settle_ms.map(|x| x as i32)
        );
    }
}
