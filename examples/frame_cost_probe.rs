//! m63 探针：`待办 §13` 里三条**只有账、没有数**的每帧 / 每次成本，一次量清。
//!
//! 立案原文的诚实声明就是"未实测耗时，不构成性能结论——要动先测"（二-②），
//! 而 A5 / D2 两条更早就卡在"等真机反馈"。这三条都能在同一进程里配对量出来，
//! 所以先把数拿到，再按成本/收益/必要性裁决修哪条。
//!
//! 三条靶子（读的是生产代码，跑的是生产入口）：
//! - **A5 棋盘格**：`ui/canvas.rs` 底图那段——开关打开时整幅视口按 `CHECKER = 12 px`
//!   逐格 `fill_rect`。4K 满窗 = 320 × 180 = **57,600 次**。
//! - **D2 缩略图条**：`ui/canvas.rs` 的 ⑤ 段每帧重算 `capacity` + `layout` +
//!   `items`，还整排 `PathBuf` 克隆（这条比待办记的更重，一并量）。
//!   **m100 起这条是两臂**：旧形状（整排克隆两份）对撞新形状（路径按引用借），
//!   两臂还先断言"摆出来的格子一模一样"——省的那笔只许是克隆，不许是几何。
//! - **二-② 格式清单**：`core::formats::is_previewable()` 每次调用要走
//!   `declared_exts()` + `is_raw()`，而这两句各自**加锁并克隆一份 HashSet**。
//!   调用点在 `platform/spacehook.rs` 的**按键路径**上——每按一次键现算。
//!
//! - **D3 缩略图 LRU 淘汰**（m108 · O14）：`core/thumbs.rs` 里每插一枚超上限的缩略图，
//!   就扫一遍全表挑最旧、还要克隆那枚键才能脱借去 `remove`。三臂对撞：现形 vs
//!   `ui/filmstrip.rs` 那一形（到上限才排一遍、一次剪到 3/4）vs 完全不剪（对照臂）；
//!   计时之前先过两条判据——剪掉的必须正是最旧那批、最旧的三枚 Pending 一枚都不许动。
//!
//! - **D4 中央错误文案**（m109 · O15）：`Viewer::error_text()` 每次交一份 `Option<String>`，
//!   而唯一调用点拿到之后只是把那句话画进画面中央。两对臂（克隆 / `as_deref` 借）× 三种状态
//!   （无错误＝常态、短文案、带文件名）——问的是"每帧克隆"这笔**在哪个状态下才真的存在**。
//!
//! - **D5 装档的缓存写**（m110 · O16）：`Viewer::install` 末尾 `path.clone()` + `meta_cache.insert`，
//!   而装档在动画/换档时每帧都来一次、`natural` 一个字都不变 ⇒ 三臂：现形 / 门闸（值没变不写）/
//!   门闸·值真变了（新图那一档，一次都不省）。
//!
//! 口径（`docs/踩坑记录/踩坑记录.md` K48/K50）：同一进程内分臂交替，报**最小 / 中位 / 最大**，
//! 不报单次读数。棋盘格那几条必须带上当时的发布配置（K53：跨档比较等于换了一支探针）。
//!
//! **2026-10-20 m63 首跑结论**（正本 本仓历史记录，数字是三跑 `mult=1/2/3` 的范围）：
//! - **D2 判废**：整排重建每帧 0.001–0.004 ms，是一帧预算的 0.004–0.024%。
//! - **二-② 判废**：每次按键 8.6–9.1 µs（两次锁 + 两份 HashSet 克隆，清单 82 + 12 项）。
//! - **A5 数字入账、两条修法判废**：逐格比"同面积只铺一次"的下限贵 2.46 ms/帧（4K 满窗）、
//!   0.22 ms/帧（800×600）；"合并同行同色"在 4K 不省（−2.7%～+4.6%，噪声带内），
//!   **"预渲染整幅再 blit" 慢 9–11×**（4K 45–56 ms）——`draw_image` 即便 1:1 吸附也要每像素纳秒级。
//! - **追加收获给 `待办 §7` 候选④**：`draw_image` 的成本跟 **dst 面积**走（框缩到 1/32
//!   面积 ⇒ 快 22–25×），整窗 Mask 在满幅那一臂占 0.7–1.6%、在小图大窗那一臂占 18–36%；
//!   每目标像素 **blit 5.5–9.0 ns**，同面积纯色铺底 **0.09–0.25 ns** ⇒ 今天"贴图"比"铺色"
//!   贵 60–100×（三跑 61 / 82 / 101×）。
//!
//! 用法：
//! ```text
//! cargo run --release --example frame_cost_probe [迭代倍数]
//! # 二-② 那段要读真解码器自述，指一下发布目录（读不到就明确跳过，不拿空跑当读数）：
//! RUPIX_DECODERS_DIR=<绝对路径> cargo run --release --example frame_cost_probe
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tiny_skia::Pixmap;
use windui::geometry::{Color, Rect};
use windui::render::image::{Fit, Image};
use windui::render::{Canvas, Paint, SkiaCanvas};
use windui::spec::Align;
use windui::text::{DWriteEngine, TextEngine, TextStyle};

/// 生产常量：与 `ui/canvas.rs:70-72` 同一套（改了那边要同步这里，否则臂就不是那条路）。
const CHECKER: f32 = 12.0;
/// 同一常量的整数口径（预渲染那臂按像素号算格子，不想到处 `as`）。
const CHECKER_PX: u32 = 12;
const CHECKER_A: Color = Color::rgb(34, 34, 34);
const CHECKER_B: Color = Color::rgb(48, 48, 48);

/// 两组窗口尺寸：用户日常那一档（800×600）与"大窗"的坏情况（4K 满窗）。
const SIZES: [(u32, u32, &str); 2] = [(800, 600, "800×600"), (3840, 2160, "4K 满窗")];

struct Stats {
    name: String,
    n: usize,
    min: f64,
    mid: f64,
    max: f64,
}

fn measure(name: String, iters: usize, mut run: impl FnMut()) -> Stats {
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

fn report(s: &Stats) {
    println!(
        "  {:<44} n={:<4} 最小 {:>8.3} ms ｜ 中位 {:>8.3} ms ｜ 最大 {:>8.3} ms",
        s.name, s.n, s.min, s.mid, s.max
    );
}

/// 现状：**逐字照 `ui/canvas.rs:828-846`**（含每格现造一个 `Paint`）。返回画了多少格。
fn checker_cells(c: &mut SkiaCanvas<'_>, vx: f32, vy: f32, vw: f32, vh: f32) -> u32 {
    let mut cells = 0u32;
    let mut y = vy - (vy % CHECKER);
    while y < vy + vh {
        let mut x = vx - (vx % CHECKER);
        let row = ((y - vy) / CHECKER) as i32;
        while x < vx + vw {
            let col = ((x - vx) / CHECKER) as i32;
            let paint = if (row + col) % 2 == 0 {
                Paint::fill(CHECKER_A)
            } else {
                Paint::fill(CHECKER_B)
            };
            let w = CHECKER.min(vx + vw - x);
            let h = CHECKER.min(vy + vh - y);
            c.fill_rect(x, y, w, h, &paint);
            cells += 1;
            x += CHECKER;
        }
        y += CHECKER;
    }
    cells
}

/// 候选修法：**整幅先铺一种色，再只补另一种的那些格**。棋盘格正好是两色，
/// 所以输出像素与逐格版**完全相同**，而 `fill_rect` 次数减半（代价：多一遍底铺）。
fn checker_base_plus_half(c: &mut SkiaCanvas<'_>, vx: f32, vy: f32, vw: f32, vh: f32) -> u32 {
    let mut rects = 0u32;
    let x0 = vx - (vx % CHECKER);
    let y0 = vy - (vy % CHECKER);
    c.fill_rect(
        x0,
        y0,
        (vx + vw - x0).max(0.0),
        (vy + vh - y0).max(0.0),
        &Paint::fill(CHECKER_B),
    );
    rects += 1;
    let mut y = y0;
    while y < vy + vh {
        let row = ((y - vy) / CHECKER) as i32;
        let h = CHECKER.min(vy + vh - y);
        // row 偶数时 (row+col) 偶 ⇒ col 偶是 A；row 奇数时 col 奇是 A。
        let start = vx - (vx % CHECKER) + if row % 2 == 0 { 0.0 } else { CHECKER };
        let mut x = start;
        while x < vx + vw {
            let w = CHECKER.min(vx + vw - x);
            c.fill_rect(x, y, w, h, &Paint::fill(CHECKER_A));
            rects += 1;
            x += 2.0 * CHECKER;
        }
        y += CHECKER;
    }
    rects
}

/// D2（**旧形状**，m100 之前）：`ui/canvas.rs:1021-1043` 里"每帧整排重建"那部分
/// （几何 + 两份 Vec + 整排 `PathBuf` 克隆）。
/// ★ 不含 `viewer.thumb_window()` 与 `viewer.thumb()`（那要活的 Gallery 状态机，探针够不着），
/// 所以这条读数是**下界**，不是这段代码的全部代价。
fn strip_rebuild(vw: f32, paths: &[PathBuf]) -> (Vec<(usize, Rect)>, usize) {
    let cap = rupix_lib::ui::filmstrip::capacity(vw);
    let picked: Vec<(usize, PathBuf)> = paths
        .iter()
        .take(cap)
        .enumerate()
        .map(|(i, p)| (i, p.clone()))
        .collect();
    // 旧 `thumb_window` 交出返回值后，还要再克隆一整排喂给 `Thumbs::ensure`——
    // 那第二份克隆也是每帧的账，臂要照着旧形状就做出来，否则两臂之差偏小。
    let owned: Vec<PathBuf> = picked.iter().map(|(_, p)| p.clone()).collect();
    // 这份克隆在后面确实没人读——不钉住它，优化器把整段循环当死码删掉，臂就空跑了。
    std::hint::black_box(&owned);
    let rects = rupix_lib::ui::filmstrip::layout((0.0, 0.0, vw, 600.0), picked.len());
    let items: Vec<(usize, Rect)> = picked
        .iter()
        .zip(rects)
        .map(|((idx, _), r)| (*idx, r))
        .collect();
    (items, picked.len())
}

/// D2（**新形状**，m100 之后）：格子向清单**借**路径，一帧一个 `PathBuf` 都不克隆。
///
/// 留着旧形状不是怀旧——两臂之差才是本轮删掉的那笔；而且 `strip_items`（命中测试的
/// 账本）在两臂里都照建，这条臂没有靠"少做别的事"充数。
fn strip_rebuild_by_ref(vw: f32, paths: &[PathBuf]) -> (Vec<(usize, Rect)>, usize) {
    let cap = rupix_lib::ui::filmstrip::capacity(vw);
    let n = cap.min(paths.len());
    let rects = rupix_lib::ui::filmstrip::layout((0.0, 0.0, vw, 600.0), n);
    let slots: Vec<rupix_lib::ui::filmstrip::Slot<'_>> = (0..n)
        .map(|i| rupix_lib::ui::filmstrip::Slot {
            idx: i,
            rect: rects[i],
            path: &paths[i],
        })
        .collect();
    let items: Vec<(usize, Rect)> = slots.iter().map(|s| (s.idx, s.rect)).collect();
    (items, slots.len())
}

// ───────── D3 缩略图 LRU 淘汰（m108 · O14） ─────────
// 三臂都照**生产里真存在的形状**写，不造理想化结构：
//   · `put_now`   ＝逐字照 `core::thumbs::Thumbs::put` 的淘汰段（每插一枚就扫一遍全表找最旧 + 克隆那枚键）
//   · `put_batch` ＝逐字照 `ui::filmstrip` 的剪枝调用点 + `lru_prune`（到上限才排一遍全表、一次剪到 3/4）
//   · `put_none`  ＝只插不剪（对照臂：把"同样的 20 次插入"那笔公共成本减掉，剩下的才是淘汰那笔）
// 表里刻意躺着**三枚最旧档的 Pending**（`Thumbs::put` 里的 Pending 豁免：Pending 有在途回包，
// 剪掉它就等于重复请求 + 留下 inflight 孤儿）——两臂都不许动它们，动了就是"少做了一步"（K81）。

/// 与 `core::thumbs::MAX_KEPT` 同值（改了那边要同步这里，否则臂就不是那条路）。
const THUMB_CAP: usize = 80;
/// 摊销档的目标剩余：与 `ui::filmstrip::KEEP_TARGET` 同一个 3/4 口径。
const THUMB_TARGET: usize = THUMB_CAP - THUMB_CAP / 4;

/// 生产 `Slot` 只有"有没有着落"参与淘汰判据，像素本体不参与 ⇒ 臂里只留判别式。
#[derive(Clone, Copy, PartialEq)]
enum ThumbKind {
    Ready,
    Pending,
}

fn thumb_key(seq: u64) -> PathBuf {
    PathBuf::from(format!(
        r"\\nas\photos\2024\DSC-{seq:06}-婚礼全家福-最终版.jpg"
    ))
}

/// 起手账本：`n` 枚已有着落的格（stamp 1..n），其中**最旧的三枚**是 Pending——
/// 这是淘汰判据最难的那种分布（最旧的不是候选，得跳过它继续找）。
fn bench_fill(n: usize) -> HashMap<PathBuf, (ThumbKind, u64)> {
    let mut m = HashMap::with_capacity(n + 24);
    for st in 1..=n as u64 {
        let kind = if st <= 3 {
            ThumbKind::Pending
        } else {
            ThumbKind::Ready
        };
        m.insert(thumb_key(st), (kind, st));
    }
    m
}

/// 现形：`core::thumbs::Thumbs::put`。插完超上限就**扫一遍全表**挑最旧的剪掉，
/// 挑中之后还要**克隆一枚键**才能脱借remove（`HashMap` 的键借用不能同时喂 `remove`）。
fn put_now(m: &mut HashMap<PathBuf, (ThumbKind, u64)>, p: PathBuf, k: ThumbKind, st: u64) {
    m.insert(p, (k, st));
    if m.len() > THUMB_CAP {
        let oldest = m
            .iter()
            .filter(|(_, (kind, _))| *kind != ThumbKind::Pending)
            .min_by_key(|(_, (_, s))| *s)
            .map(|(path, _)| path.clone());
        if let Some(gone) = oldest {
            m.remove(&gone);
        }
    }
}

/// 改法：m96 给 thumbs 提的那条"一次剪到目标位"，形状与 `ui/filmstrip.rs` 的
/// `lru_prune` 一致——剪枝排在插入**之前**，一次剪到 3/4，后面几十次插入都白拿。
/// 唯一多出来的约束是 thumbs 的 Pending 豁免（filmstrip 没有在途概念），照加。
fn put_batch(m: &mut HashMap<PathBuf, (ThumbKind, u64)>, p: PathBuf, k: ThumbKind, st: u64) {
    if m.len() >= THUMB_CAP {
        let drop_n = m.len() - THUMB_TARGET;
        let mut keyed: Vec<(u64, PathBuf)> = m
            .iter()
            .filter(|(_, (kind, _))| *kind != ThumbKind::Pending)
            .map(|(path, (_, s))| (*s, path.clone()))
            .collect();
        keyed.sort_unstable_by_key(|(s, _)| *s);
        for (_, gone) in keyed.into_iter().take(drop_n) {
            m.remove(&gone);
        }
    }
    m.insert(p, (k, st));
}

/// 对照臂：同样的插入、没有淘汰分支（表一路涨）。两臂之差才是"淘汰这笔"的成本。
fn put_none(m: &mut HashMap<PathBuf, (ThumbKind, u64)>, p: PathBuf, k: ThumbKind, st: u64) {
    m.insert(p, (k, st));
}

/// 第三条路（顺手查"有没有白捡的钱"）：先扫一遍挑出最旧的 **stamp**，再用 `retain`
/// 就地摘掉它——**一次克隆都不要**。生产里 stamp 每次 `bump` 严格 +1，全表唯一，
/// 所以"按 stamp 剪"与"按键剪"落进同一枚条目；唯一要多付的是第二遍扫表。
fn put_retain(m: &mut HashMap<PathBuf, (ThumbKind, u64)>, p: PathBuf, k: ThumbKind, st: u64) {
    m.insert(p, (k, st));
    if m.len() > THUMB_CAP {
        let oldest = m
            .iter()
            .filter(|(_, (kind, _))| *kind != ThumbKind::Pending)
            .min_by_key(|(_, (_, s))| *s)
            .map(|(_, (_, s))| *s);
        if let Some(gone) = oldest {
            m.retain(|_, (_, s)| *s != gone);
        }
    }
}

/// 一轮跑多少枚新缩略图落地（＝翻页时"请求 + 回包"里真正进表的那几次 put）。
const THUMB_RUN: u64 = 20;

/// 跑 `THUMB_RUN` 枚新缩略图落地的整段（起手满表 80 枚，新来的 stamp 从 101 起）。
/// 返回表本身供判据核对；计时与核对用**同一个闭包形状**，臂之间只差淘汰那一步。
fn thumb_run<F>(put: F) -> HashMap<PathBuf, (ThumbKind, u64)>
where
    F: Fn(&mut HashMap<PathBuf, (ThumbKind, u64)>, PathBuf, ThumbKind, u64),
{
    let mut m = bench_fill(THUMB_CAP);
    for st in 101..101 + THUMB_RUN {
        put(&mut m, thumb_key(st), ThumbKind::Ready, st);
    }
    m
}

/// 判据核对（K81/K119）：淘汰策略不许变成"随便剪"。两件事——
/// ① 三枚最旧的 Pending 一枚都不许被剪；② 活下来的 Ready 必须正好是历史上**最新的那批**
/// （不是最新那批＝剪错了对象，成本再低也不是同一条路）。
fn thumb_invariants(m: &HashMap<PathBuf, (ThumbKind, u64)>) {
    for st in 1..=3u64 {
        assert!(
            m.contains_key(&thumb_key(st)),
            "最旧的 Pending（stamp {st}）被剪了：等于重复请求 + inflight 孤儿"
        );
    }
    let mut history: Vec<u64> = (4..=THUMB_CAP as u64).chain(101..101 + THUMB_RUN).collect();
    history.sort_unstable();
    let mut ready: Vec<u64> = m
        .iter()
        .filter(|(_, (k, _))| *k == ThumbKind::Ready)
        .map(|(_, (_, s))| *s)
        .collect();
    ready.sort_unstable();
    assert!(
        ready.len() <= history.len(),
        "存活的 Ready 比历史上出现过的还多＝臂没在淘汰"
    );
    assert_eq!(
        ready,
        history[history.len() - ready.len()..],
        "剪掉的不是最旧那批（淘汰 {} 枚）",
        history.len() - ready.len()
    );
}

// ───────── D4 中央错误文案的读法（m109 · O15） ─────────
// 靶子＝`core::thumbs` 隔壁那条：`Viewer::error_text()` 返回 `Option<String>`，
// 而唯一的生产调用点（`ui::canvas` 的 ② 段）拿到之后只做一件事——`draw_text` 读一嘴。
// m96 的账写的是"每帧克隆一份"。两对臂把这条拆成两个状态问：**平时（没有错误）**付多少，
// **画面上真摆着错误句那一档**付多少。两种形状（克隆 / 借）在同状态下必须交出**同一串字**，
// 否则比的不是同一条路（K81）。

/// 现形：逐字照 `Viewer::error_text`（`self.error.clone()`）。
/// `#[inline(never)]` 不是摆样子：生产里这一嘴是**跨模块的一次真调用**（`core::viewer` →
/// `ui::canvas`），内联进来反而会让臂量到一条生产里不存在的路（还可能整个被优化没了）。
#[inline(never)]
fn error_now(slot: &Option<String>) -> Option<String> {
    slot.clone()
}

/// 借形：m96 提的改法（`self.error.as_deref()`，交 `Option<&str>`）。同上，不内联。
#[inline(never)]
fn error_borrow(slot: &Option<String>) -> Option<&str> {
    slot.as_deref()
}

/// 短文案那一档：解码器分类消息本体就这么长（`decoders` 里"该文件用了此解码器不支持的特性"级）。
const ERR_SHORT: &str = "该文件用了此解码器不支持的特性";
/// 带文件名那一档：解码错误里会把 `path.file_name()` 拼进去，UTF-8 下这是现实的长度。
const ERR_LONG: &str =
    "内置解码库「png」出错，已隔离：IMG-20240131-婚礼全家福-最终版.png 不是合法的 PNG 流";

/// 纳秒级的臂（D4）：一批装 `inner` 次操作再摊平。逐次计时在这一臂上不可用——
/// `Instant::now()` + `elapsed()` 自己就几十 ns，而这里要分的是 ~20 ns 的活；
/// 第一版就是逐次跑的，三臂全读 0.000 ms，量了个空（读数是 0 先怀疑量具，与 K98 同形）。
/// 这次 `Stats` 的三个读数装的是 **ns/次**。
fn measure_each(name: String, iters: usize, inner: usize, mut run: impl FnMut()) -> Stats {
    let mut times: Vec<f64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        for _ in 0..inner {
            run();
        }
        times.push(t.elapsed().as_secs_f64() * 1_000_000_000.0 / inner as f64);
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

/// `report` 的纳秒版（同进程分臂交替的口径不变，K48/K50）。
fn report_ns(s: &Stats) {
    println!(
        "  {:<34} n={:<5} 最小 {:>7.2} ns ｜ 中位 {:>7.2} ns ｜ 最大 {:>8.2} ns",
        s.name, s.n, s.min, s.mid, s.max
    );
}

/// D4 的等价控制：两种形状必须交出**同一串字**，否则比的不是"读法"而是文案（K81）。
fn error_same_text(slot: &Option<String>, tag: &str) {
    assert_eq!(
        error_now(slot).as_deref(),
        error_borrow(slot),
        "{tag}：两种形状交出的字不同，臂不同形"
    );
}

/// D4 的一档两臂：现形 clone / 借形 as_deref，各自一批 `inner` 次、摊成 ns/帧。
fn error_arms(mult: usize, slot: &Option<String>, tag: &str) -> (Stats, Stats) {
    let iters = 300 * mult;
    let inner = 3_000;
    let clone = measure_each(format!("现形 clone｜{tag}"), iters, inner, || {
        std::hint::black_box(error_now(slot).map(|s| s.len()));
    });
    let borrow = measure_each(format!("借形 as_deref｜{tag}"), iters, inner, || {
        std::hint::black_box(error_borrow(slot).map(str::len));
    });
    (clone, borrow)
}

// ───────── D5 装档时往元数据缓存盖权威尺寸那一步（m110 · O16） ─────────
// 靶子＝`core::viewer::Viewer::install` 末尾那两行：`path.clone()` 留一份当缓存键，
// 再 `meta_cache.insert(...)`。K125 的三问在这条上答案是"**真的在付**"：装档不只发生在换图，
// 同一张的原图档升级、SVG 切片重渲、**动画每一帧**都进来一次（m91 记的 21.7 fps 就是每帧一次）。
// 而缓存存的就是 `natural`——同一张图每帧那个值**一个字都不变**，于是绝大多数次是"写同一个值"。
//
// 三臂：现形（克隆 + 插入，逐字照生产）／门闸（先比值，没变就不写）／门闸·值变了（新图那一档，
// 仍然克隆 + 插入）。表在计时**之前**就建好并含键——两臂同阶段、同起手（缓存环境就是臂的身份，
// 记忆 `evidence-driven` 那条），差只许是"要不要写"。

use rupix_lib::core::decode::Meta;

/// 现形：逐字照生产那两行（克隆一枚键 + 一次插入）。
#[inline(never)]
fn install_now(m: &mut HashMap<PathBuf, Meta>, path: &Path, natural: (u32, u32)) {
    let cache_path = path.to_path_buf();
    m.insert(cache_path, Meta { natural });
}

/// 门闸：值没变就不写（房规同 `ui::settings` 的 `put_b` / `Handles::refresh_labels`）。
#[inline(never)]
fn install_gated(m: &mut HashMap<PathBuf, Meta>, path: &Path, natural: (u32, u32)) {
    if m.get(path).map(|x| x.natural) != Some(natural) {
        m.insert(path.to_path_buf(), Meta { natural });
    }
}

/// 一臂的起手账本：缓存里已经有这张图（动画跑到第 N 帧时的常态）。
fn cache_seeded(path: &Path, natural: (u32, u32)) -> HashMap<PathBuf, Meta> {
    let mut m = HashMap::new();
    m.insert(path.to_path_buf(), Meta { natural });
    m
}

/// D5 的一档三臂。`alt` 决定"值变了"那一臂怎么喂（交替两个 natural ⇒ 每次都判成要写）。
fn install_arms(
    mult: usize,
    path: &Path,
    natural: (u32, u32),
    alt: (u32, u32),
) -> (Stats, Stats, Stats) {
    let iters = 300 * mult;
    let inner = 2_000;
    let mut seeded_now = cache_seeded(path, natural);
    let mut seeded_gate = cache_seeded(path, natural);
    let now = measure_each("现形 克隆+插入".to_string(), iters, inner, || {
        install_now(&mut seeded_now, path, natural);
        std::hint::black_box(seeded_now.len());
    });
    let gate = measure_each("门闸 值没变·不写".to_string(), iters, inner, || {
        install_gated(&mut seeded_gate, path, natural);
        std::hint::black_box(seeded_gate.len());
    });
    // 门闸·值变了：交替两个尺寸，逼它每轮都走写的那条路（新图 / 首次装档那一档）。
    let mut flip = natural;
    let mut seeded_gate2 = cache_seeded(path, natural);
    let gate_write = measure_each("门闸 值变了·照写".to_string(), iters, inner, || {
        flip = if flip == natural { alt } else { natural };
        install_gated(&mut seeded_gate2, path, flip);
        std::hint::black_box(seeded_gate2.len());
    });
    (now, gate, gate_write)
}

/// D5 的等价控制：三臂跑完，缓存里都必须是**最后一次喂进去的那个权威值**，
/// 而且不许有多余的键——门闸"省"的前提是同一条路，不是少写。
fn install_equivalence(path: &Path, natural: (u32, u32), alt: (u32, u32)) {
    let mut a = cache_seeded(path, natural);
    let mut b = cache_seeded(path, natural);
    let mut c = cache_seeded(path, natural);
    for i in 0..17u64 {
        // 交替喂：门闸必须每次都跟着改，一次都不许冻住旧值。
        let n = if i % 2 == 0 { natural } else { alt };
        install_now(&mut a, path, n);
        install_gated(&mut b, path, n);
        install_gated(&mut c, path, n);
    }
    assert_eq!(
        a.get(path).unwrap().natural,
        b.get(path).unwrap().natural,
        "门闸冻住了旧值"
    );
    assert_eq!(a.len(), 1, "现形臂自己都不唯一＝夹具不对");
    assert_eq!(b.len(), 1, "门闸多长了键＝比的不是同一条路");
    assert_eq!(c.len(), 1);
}

// ───────── D6 自然序排序键的写法（m111 · O17） ─────────
// 靶子＝`core::gallery::natural_key` 里数字段那四行：零填充数值与段长度各造一枚**中间 String**
// 再 `push_str`（`&format!("{num:020}")` 与 `&(end-i).to_string()`）。Schwartzian 那一轮（HISTORY
// 2026-10-14）把"每次比较现算一遍键"收成了"每项一次"，**没碰键里的这两次分配**（K120 查过：无结案）。
// ★ 两处臂都**逐字复刻生产那段切分**——探针碰不到 crate 私有函数，这是从 B1.1 那轮就背着的副本债
//   （`examples/bench_gallery_sort.rs` 头顶写着同一句话）：改了生产必须同步这里，否则臂在量另一条路。
// K125 第④问在这条上有好答案：直写**不引入任何新判断**，所以只有"省"的一档、没有反档——
// 但正因如此更要防"省的是不是真的那两枚分配"：两臂的键必须**逐字节相同**，整本排序的**顺序也必须相同**。
// ★★ m111 落地状态：**直写臂＝今天的生产**，现形臂＝落地前的旧形，留着只为"这条确实省过"可复核。
//   判废/落地的读数都写在这段的输出里，别把旧臂当成现网在跑的那条。

/// 旧形（m111 之前的生产）：每枚数字段两次中间 String 分配。
#[inline(never)]
fn natural_key_now(s: &str) -> String {
    let s = s.to_lowercase();
    let bytes = s.as_bytes();
    let mut key = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < bytes.len() {
        let is_digit = bytes[i].is_ascii_digit();
        let end = bytes[i..]
            .iter()
            .position(|c| c.is_ascii_digit() != is_digit)
            .unwrap_or(bytes.len() - i)
            + i;
        if is_digit {
            let num: u64 = s[i..end].parse().unwrap_or(u64::MAX);
            key.push('\u{1}');
            key.push_str(&format!("{num:020}"));
            key.push('\u{1}');
            key.push_str(&(end - i).to_string());
        } else {
            key.push_str(&s[i..end]);
        }
        i = end;
    }
    key
}

/// 直写：`write!` 把两位数字**直接排进目标串**，中间那两枚 String 根本不存在。＝ m111 之后的生产。
/// `String` 的 `fmt::Write` 只在分配失败时出错（那时进程早已 abort），所以 `let _ =` 是有意的。
#[inline(never)]
fn natural_key_written(s: &str) -> String {
    use std::fmt::Write as _;
    let s = s.to_lowercase();
    let bytes = s.as_bytes();
    let mut key = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < bytes.len() {
        let is_digit = bytes[i].is_ascii_digit();
        let end = bytes[i..]
            .iter()
            .position(|c| c.is_ascii_digit() != is_digit)
            .unwrap_or(bytes.len() - i)
            + i;
        if is_digit {
            let num: u64 = s[i..end].parse().unwrap_or(u64::MAX);
            key.push('\u{1}');
            let _ = write!(key, "{num:020}");
            key.push('\u{1}');
            let _ = write!(key, "{}", end - i);
        } else {
            key.push_str(&s[i..end]);
        }
        i = end;
    }
    key
}

/// D6 的等价控制：最难看的那些形状上，两臂必须交出**逐字节相同**的键。
/// 覆盖：无数字段 / 一段 / 前导零（长度那一档必须跟着变）/ 超 20 位（parse 失败兜底 u64::MAX）/
/// 非 ASCII（`to_lowercase` 的 Unicode 路径）/ 空串。
fn natural_key_equivalence() {
    let cases = [
        "photo.jpg",
        "img2.png",
        "img02.png",
        "img10.png",
        "IMG-20240131-婚礼全家福-最终版-0312.jpg",
        "99999999999999999999999.png",
        "İnceleme-1.jpeg",
        "",
        "0",
        "12-34-56-78-90.tiff",
    ];
    for c in cases {
        assert_eq!(
            natural_key_now(c),
            natural_key_written(c),
            "键逐字节不同＝直写改了语义，臂不同形：{c}"
        );
    }
}

/// 造 N 项真实感文件名（数字段**数量**从 0 到 4 都覆盖，长度与位置各异）。
/// 第 4 形刻意一枚数字段都没有——那是"直写在这条形上根本不该省钱"的对照，
/// 防的是把只在数字段上发生的事算成整本清单的收益（K125 第③问的反面）。
fn natural_key_corpus(n: usize) -> Vec<String> {
    const WORDS: [&str; 16] = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
    ];
    (0..n)
        .map(|i| match i % 4 {
            // 4 枚数字段（含中文段）
            0 => format!("IMG-{i:04}-婚礼全家福-第{i:03}张.jpg"),
            // 1 枚数字段，位数随 i 自然变（1→4 位）
            1 => format!("photo{i}.png"),
            // 3 枚数字段，含前导零档
            2 => format!("{i:06}_DSC{}.tif", i % 97),
            // 0 枚数字段
            _ => format!("{}-final-cut.bmp", WORDS[i % WORDS.len()]),
        })
        .collect()
}

/// D6 的整本排序臂：`sort_natural` 的 Schwartzian 形（每项一次键 + 比较器只比键）。
/// 比较器**逐字照生产**：只比键、不加 `then(name)`——生产用的是稳定 `sort_by`，
/// 键相同的两项按输入序排；臂若自己补一层 tie-break，就和跑的那条不是一条路了。
fn natural_sorted_names(names: &[String], key: fn(&str) -> String) -> Vec<String> {
    let mut decorated: Vec<(String, &str)> = names.iter().map(|n| (key(n), n.as_str())).collect();
    decorated.sort_by(|a, b| a.0.cmp(&b.0));
    decorated.into_iter().map(|(_, n)| n.to_string()).collect()
}

/// ★ 探针是不是在量生产那条路：真造一册刁钻名字的**目录**，读生产 `Gallery::from_file`
/// 给出的顺序，必须与臂的顺序逐项相同。副本对不上生产＝臂作废（K119 同形：量具先过自己的判据）。
/// 覆盖形状：大小写同段 / 前导零 / 无数字段 / 超 20 位（parse 兜底）/ 中文段 / 纯数字名。
/// 后缀用 `.png`——它由**解码器**认领（wic 是 `auto_claim=false`，进不了清单），
/// 所以跑探针要先 `RUPIX_DECODERS_DIR` 指向发布解码器目录，否则这里如实炸出来而不是静默空册。
fn natural_key_pin_to_production(names: &[String], key: fn(&str) -> String) {
    let dir = rupix_lib::core::paths::test_scratch_root()
        .join(format!("rupix-d6-pin-{}", std::process::id()));
    // K121：按 pid 命名的夹具目录**必须先清**，上一轮残留会混进这一轮的册子。
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("夹具目录建不出 {dir:?}：{e}"));
    for n in names {
        std::fs::write(dir.join(n), b"x").unwrap_or_else(|e| panic!("夹具文件写不出 {n}：{e}"));
    }
    let g = rupix_lib::core::gallery::Gallery::from_file(&dir)
        .unwrap_or_else(|e| panic!("生产清单读不出（解码器目录没挂上？）：{e:?}"));
    let got: Vec<String> = (0..g.len())
        .map(|i| {
            g.peek(i)
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let want = natural_sorted_names(names, key);
    assert_eq!(got, want, "生产给出的顺序 ≠ 臂给出的顺序＝探针在量另一条路");
    std::fs::remove_dir_all(&dir).unwrap_or_else(|e| panic!("夹具目录清不掉 {dir:?}：{e}"));
    println!(
        "  对生产钉子：真目录 {} 项，生产顺序与臂逐项相同 ✅（副本没漂）",
        got.len()
    );
}

/// 钉子专用的刁钻小册（名字互不相同，键也互不相同，避开 tie 的形状）。
fn natural_key_pin_corpus() -> Vec<String> {
    [
        "img1.png",
        "IMG2.png",
        "img02.png",
        "img10.png",
        "img.png",
        "99999999999999999999999.png",
        "12-34-56.png",
        "婚礼第3张.png",
        "婚礼第03张.png",
        "a1b2c3.png",
        "photo-000007.png",
        "7.png",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

fn main() {
    let mult: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .filter(|m| *m > 0)
        .unwrap_or(1);
    println!("frame_cost_probe｜迭代倍数 {mult}｜同一进程内分臂交替（K48/K50 口径）");

    // ───────── A5 棋盘格 ─────────
    println!("\n【A5】棋盘格底图（生产常量 CHECKER = 12 px，ui/canvas.rs:828-846）");
    for (w, h, tag) in SIZES {
        let iters = if w > 1000 { 30 * mult } else { 300 * mult };
        let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("{w}×{h} 离屏缓冲造不出"));
        let (vw, vh) = (w as f32, h as f32);
        let cells = {
            let mut c = SkiaCanvas::new(&mut pm);
            checker_cells(&mut c, 0.0, 0.0, vw, vh)
        };
        let rects = {
            let mut c = SkiaCanvas::new(&mut pm);
            checker_base_plus_half(&mut c, 0.0, 0.0, vw, vh)
        };
        let a = measure(
            format!("棋盘格 {tag} 逐格（{cells} 次 fill_rect）"),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm);
                assert_eq!(checker_cells(&mut c, 0.0, 0.0, vw, vh), cells);
            },
        );
        let b = measure(
            format!("棋盘格 {tag} 铺底+补半格（{rects} 次）"),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm);
                assert_eq!(checker_base_plus_half(&mut c, 0.0, 0.0, vw, vh), rects);
            },
        );
        let f = measure(
            format!("对照 {tag} 整幅一次铺底（像素下限）"),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm);
                c.fill_rect(0.0, 0.0, vw, vh, &Paint::fill(CHECKER_A));
            },
        );
        // 待办 §13 A5 自己给的候选修法：**预渲染一张视口尺寸的棋盘位图**，尺寸不变就
        // 每帧只 blit 一次。这条臂量的是"那个修法到底值多少"，不是猜。
        let cached = {
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            for (i, chunk) in rgba.chunks_mut(4).enumerate() {
                let px = ((i % w as usize) / CHECKER_PX as usize) as i32;
                let py = ((i / w as usize) / CHECKER_PX as usize) as i32;
                let col = if (px + py) % 2 == 0 {
                    CHECKER_A
                } else {
                    CHECKER_B
                };
                chunk.copy_from_slice(&[col.r, col.g, col.b, 255]);
            }
            Image::from_rgba(w, h, &rgba).unwrap_or_else(|e| panic!("预渲染棋盘失败：{e}"))
        };
        let d = measure(
            format!("棋盘格 {tag} 预渲染整幅 + 每帧 blit（修法上限）"),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm);
                c.draw_image(
                    &cached,
                    Rect::new(0, 0, w as i32, h as i32),
                    Fit::Fill,
                    0.0,
                    1.0,
                );
            },
        );
        for s in [&a, &b, &d, &f] {
            report(s);
        }
        println!(
            "  → 减半那条省 {:.1}%（{:.3} → {:.3} ms）｜预渲染 blit 省 {:.1}%（{:.3} ms）｜下限 {:.3} ms；每格开销 ≈ {:.2} µs",
            (1.0 - b.mid / a.mid) * 100.0,
            a.mid,
            b.mid,
            (1.0 - d.mid / a.mid) * 100.0,
            d.mid,
            f.mid,
            a.mid * 1000.0 / cells as f64
        );
    }

    // ───────── 追加：draw_image 的成本构成（给 `待办 §7` 候选④ 拿数） ─────────
    //
    // A5 那条"预渲染整幅再 blit"的候选被上面的读数判废之后，紧接着要回答的问题是：
    // **今天框架的位图贴图到底贵在哪**。windui 自己在 `render/skia.rs:702-708` 写了
    // "本函数整体仍是 O(窗口)"（每帧造一块**整窗尺寸**的 `Mask` 再 `fill_path`），
    // 并把"真要根治"的两条路点名了。这三臂就是去分账：贵的是窗口面积（mask），
    // 还是图本身（采样/搬运）。
    println!("\n【追加】draw_image 成本构成：O(窗口) 还是 O(图)？（候选④ 的靶子）");
    {
        let (w, h) = (3840u32, 2160u32);
        let iters = 30 * mult;
        let mut pm = Pixmap::new(w, h).unwrap_or_else(|| panic!("{w}×{h} 离屏缓冲造不出"));
        let img_big = {
            let rgba = vec![0x40u8; (w as usize) * (h as usize) * 4];
            Image::from_rgba(w, h, &rgba).unwrap_or_else(|e| panic!("造 4K 位图失败：{e}"))
        };
        let img_small = {
            let rgba = vec![0x40u8; 512 * 512 * 4];
            Image::from_rgba(512, 512, &rgba).unwrap_or_else(|e| panic!("造 512² 位图失败：{e}"))
        };
        let full = Rect::new(0, 0, w as i32, h as i32);
        let small = Rect::new(40, 40, 512, 512);
        let a = measure(
            "blit 4K 图 → 4K 框（4K 窗）".to_string(),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm);
                c.draw_image(&img_big, full, Fit::Fill, 0.0, 1.0);
            },
        );
        let b = measure(
            "blit 512² 图 → 512² 框（4K 窗）".to_string(),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm);
                c.draw_image(&img_small, small, Fit::Fill, 0.0, 1.0);
            },
        );
        let c = measure(
            "对照 fill_rect 512² 框（4K 窗，不走 mask）".to_string(),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm);
                c.fill_rect(40.0, 40.0, 512.0, 512.0, &Paint::fill(CHECKER_A));
            },
        );
        let mut pm2 = Pixmap::new(800, 600).unwrap_or_else(|| panic!("800×600 离屏缓冲造不出"));
        let d = measure(
            "对照 blit 512² 图 → 512² 框（800×600 窗）".to_string(),
            iters,
            || {
                let mut c = SkiaCanvas::new(&mut pm2);
                c.draw_image(&img_small, small, Fit::Fill, 0.0, 1.0);
            },
        );
        for s in [&a, &b, &c, &d] {
            report(s);
        }
        // 三条推导各自独立，别把"缩图"和"缩窗"混成一句话：
        // ① 同一块窗里把**目标框**从满幅缩到 512²（面积 1/32）；
        // ② 同一个目标框把**窗口**从 4K 缩到 800×600——剩下的差就是整幅 Mask 那一项；
        // ③ 每目标像素 ns，与同面积纯色 `fill_rect` 对照。
        let px_full = (w * h) as f64;
        let px_small = 512.0 * 512.0;
        println!(
            "  ① 目标框缩到 1/{:.0} 面积 ⇒ 成本 {:.2} → {:.2} ms（{:.0}×）：**跟的是 dst 面积**",
            px_full / px_small,
            a.mid,
            b.mid,
            a.mid / b.mid
        );
        println!(
            "  ② 同一个框把窗缩到 800×600 ⇒ {:.2} → {:.2} ms，整幅 Mask 那一项 ≈ {:.2} ms\
             （占 4K 满幅那一臂 {:.1}%）——框架注释说的是这件，但它不是主项",
            b.mid,
            d.mid,
            b.mid - d.mid,
            (b.mid - d.mid) / a.mid * 100.0
        );
        println!(
            "  ③ 每目标像素：4K blit {:.2} ns ｜ 512² blit {:.2} ns ｜ 同面积纯色 fill_rect {:.2} ns \
             ⇒ 贴图比铺色贵约 {:.0}×（1:1 吸附已生效，见补丁 13）",
            a.mid * 1e6 / px_full,
            b.mid * 1e6 / px_small,
            c.mid * 1e6 / px_small,
            (b.mid / px_small) / (c.mid / px_small)
        );
    }

    // ───────── D2 缩略图条 ─────────
    println!("\n【D2】缩略图条每帧整排重建（几何 + Vec + PathBuf 克隆；下界口径）");
    println!("  m100 两臂：旧形状（整排克隆两份）vs 新形状（路径按引用借）");
    let paths: Vec<PathBuf> = (0..64)
        .map(|i| {
            PathBuf::from(format!(
                r"\\nas\photos\2024\IMG-{i:05}-婚礼全家福-最终版.jpg"
            ))
        })
        .collect();
    for (w, _, tag) in SIZES {
        let iters = 2000 * mult;
        let vw = w as f32;
        let (_, n) = strip_rebuild(vw, &paths);
        // ★ 两臂必须交出**一模一样**的命中测试账本：省的那笔只许是克隆，不许是几何。
        //   这条断言放在计时之前跑，两臂任何一处格子序号或矩形错位都当场炸。
        assert_eq!(
            strip_rebuild(vw, &paths).0,
            strip_rebuild_by_ref(vw, &paths).0,
            "{tag}：新旧形状摆出的格子不同"
        );
        let full = measure(
            format!("重建整排 {tag}·旧形状（容量 {n} 格）"),
            iters,
            || {
                let (items, got) = strip_rebuild(vw, &paths);
                assert_eq!(items.len(), got);
            },
        );
        let byref = measure(
            format!("重建整排 {tag}·新形状（容量 {n} 格）"),
            iters,
            || {
                let (items, got) = strip_rebuild_by_ref(vw, &paths);
                assert_eq!(items.len(), got);
            },
        );
        report(&full);
        report(&byref);
        for s in [&full, &byref] {
            println!(
                "  → {}：每帧中位 {:.4} ms ＝ 每格 {:.2} µs（16 ms 一帧里占 {:.3}%）｜n={}",
                if s.name.contains("旧") { "旧" } else { "新" },
                s.mid,
                s.mid * 1000.0 / n as f64,
                s.mid / 16.0 * 100.0,
                s.n
            );
        }
        println!(
            "  → 两臂之差（本轮删掉的那笔）：中位 {:.3} µs/帧、最小 {:.3} µs/帧｜参照：每次迭代自己那次秒表读数约 0.02–0.05 µs",
            (full.mid - byref.mid) * 1000.0,
            (full.min - byref.min) * 1000.0
        );
    }

    // ───────── D3 缩略图 LRU 淘汰（m108 · O14） ─────────
    println!("\n【D3】缩略图 LRU 淘汰：每插一枚扫全表（现形＝`Thumbs::put`）vs 一次剪到 3/4（`ui::filmstrip` 那形）vs 不剪（对照）");
    println!(
        "  一臂＝满表 {} 枚起手 + {} 枚新缩略图落地；三臂只差淘汰那一步",
        THUMB_CAP, THUMB_RUN
    );
    // ★ 判据先过（K81/K119）：两臂都必须"最旧的 Pending 一枚不动 + 剪掉的正是最旧那批"，
    //   而且要剪得**一模一样**——否则"快了"其实是"少做了一步"。
    let gone_now = thumb_run(put_now);
    let gone_batch = thumb_run(put_batch);
    let gone_retain = thumb_run(put_retain);
    thumb_invariants(&gone_now);
    thumb_invariants(&gone_batch);
    thumb_invariants(&gone_retain);
    let mut keys_now: Vec<PathBuf> = gone_now.keys().cloned().collect();
    let mut keys_batch: Vec<PathBuf> = gone_batch.keys().cloned().collect();
    let mut keys_retain: Vec<PathBuf> = gone_retain.keys().cloned().collect();
    keys_now.sort();
    keys_batch.sort();
    keys_retain.sort();
    assert_eq!(
        keys_now, keys_batch,
        "现形与改法留下的表不同形，比的就不是同一条淘汰路"
    );
    assert_eq!(
        keys_now, keys_retain,
        "按 stamp 剪与按键剪剪到了不同条目（stamp 唯一这条前提不成立）"
    );
    let iters = 4000 * mult;
    let base = measure(
        format!("对照臂：同样 {THUMB_RUN} 次插入，不剪"),
        iters,
        || {
            std::hint::black_box(thumb_run(put_none).len());
        },
    );
    let now = measure(
        "现形：每插一枚扫全表 + 克隆一枚键".to_string(),
        iters,
        || {
            std::hint::black_box(thumb_run(put_now).len());
        },
    );
    let batch = measure(
        "改法：到上限排一遍全表、一次剪到 3/4".to_string(),
        iters,
        || {
            std::hint::black_box(thumb_run(put_batch).len());
        },
    );
    let retain = measure(
        "零克隆：扫一遍挑 stamp、retain 就地摘".to_string(),
        iters,
        || {
            std::hint::black_box(thumb_run(put_retain).len());
        },
    );
    for s in [&base, &now, &batch, &retain] {
        report(s);
    }
    for (tag, s) in [("现形", &now), ("改法", &batch), ("零克隆", &retain)] {
        let per_run = (s.mid - base.mid) * 1000.0; // µs / 一臂（20 枚）
        println!(
            "  → {}：淘汰那笔 = 每臂 {:.2} µs ＝ 每张新缩略图 {:.2} µs ＝ {} 枚 put 摊平；对照臂中位 {:.2} µs 里公共成本占 {:.1}%",
            tag,
            per_run,
            per_run / THUMB_RUN as f64,
            THUMB_RUN,
            base.mid * 1000.0,
            base.mid / s.mid * 100.0
        );
    }
    for (tag, s) in [("改法", &batch), ("零克隆", &retain)] {
        println!(
            "  → {} 相对现形：中位 {:+.2} µs/臂、最小 {:+.2} µs/臂、最大 {:+.2} µs/臂（正数＝比现形省）｜n={}/{}",
            tag,
            (now.mid - s.mid) * 1000.0,
            (now.min - s.min) * 1000.0,
            (now.max - s.max) * 1000.0,
            now.n,
            s.n
        );
    }
    println!(
        "  → 触发频次这笔不在探针里，在读代码：`put` 只在**新图进表**那两刻走（请求一枚 Pending + 回包一枚 Ready），\n     命中走 `get_mut` 根本不碰淘汰；`ensure` 每帧最多发 3 枚请求（`core::thumbs::MAX_INFLIGHT`）⇒ 每帧上限 3 次扫表、\n     稳态翻页约每张图 2 次。这是常数推出来的上界，不是量出来的次数——按 K123，这种量级的收益本就过不了 15.6 ms 那把尺。"
    );

    // ───────── D4 中央错误文案的读法（m109 · O15） ─────────
    println!("\n【D4】`error_text()` 那一嘴：现形 clone `Option<String>` vs 借形 `Option<&str>`，各跑三种状态");
    let none: Option<String> = None;
    let short: Option<String> = Some(ERR_SHORT.to_string());
    let long: Option<String> = Some(ERR_LONG.to_string());
    // ★ 同状态下两种形状必须交出**同一串字**，否则比的不是"读法"而是文案（K81）。
    error_same_text(&none, "无错误");
    error_same_text(&short, "短文案");
    error_same_text(&long, "带文件名");
    println!(
        "  文案字节数（UTF-8）：短 {} B ｜ 长 {} B；无错误那一档就是画面正常的常态",
        ERR_SHORT.len(),
        ERR_LONG.len()
    );
    let mut delta_long = 0.0_f64;
    for (tag, slot) in [
        ("无错误(常态)", &none),
        ("短文案", &short),
        ("带文件名", &long),
    ] {
        let (clone, borrow) = error_arms(mult, slot, tag);
        if tag == "带文件名" {
            delta_long = clone.mid - borrow.mid;
        }
        report_ns(&clone);
        report_ns(&borrow);
        println!(
            "  → {}：现形 {:.2} ns/帧 ｜ 借形 {:.2} ns/帧 ｜ 差 {:+.2} ns（最小列 {:+.2} ns）；按 41 帧/s 的上界摊＝{:+.3} µs/s",
            tag, clone.mid, borrow.mid, clone.mid - borrow.mid, clone.min - borrow.min,
            (clone.mid - borrow.mid) * 41.0 / 1000.0
        );
    }
    println!(
        "  → 每格＝3,000 次一批再摊平（n=300/轮 ×{}），粒度到纳秒——**不是** 15.6 ms 那把 CPU 尺（K123）。",
        mult
    );

    // ★ 参照臂：省下的那几十纳秒是**在同一帧里为它服务的这一句话**付的，所以分母必须是
    //   draw_text 本身（K118：落不进帧预算的账要挂在看得见它的分母上，否则只是一个孤零零的 ns）。
    //   字号与矩形照生产那一格：`hint_ts = 14.0`、中央槽两头让开 `ARROW_ZONE`。
    //   这里没走 `runtime::run` 的 `register_private_use_font`——字形来源不同，成本口径不受影响。
    let mut pm = Pixmap::new(900, 700).unwrap_or_else(|| panic!("900×700 离屏缓冲造不出"));
    let mut eng = DWriteEngine::new();
    eng.set_scale(1.0);
    let ts = TextStyle::new(14.0);
    let z = rupix_lib::ui::chrome::ARROW_ZONE as i32;
    let slot = Rect::new(z, 0, (900 - 2 * z).max(60), 700);
    let draw = measure(
        "参照：把这句错误文案真画出来（draw_text）".to_string(),
        300 * mult,
        || {
            let mut c = SkiaCanvas::with_text(&mut pm, &mut eng, 1.0);
            c.draw_text(
                ERR_LONG,
                slot,
                Color::rgb(224, 120, 120),
                Align::Center,
                &ts,
            );
        },
    );
    report(&draw);
    let draw_ns = draw.mid * 1_000_000.0;
    println!(
        "  → 占比：这一档省 {:.1} ns ÷ 同一帧那句排版+绘制 {:.0} ns ＝ {:.3}%｜借形换来的那点，连它服务的那句话的零头都不到。",
        delta_long,
        draw_ns,
        delta_long / draw_ns * 100.0,
    );

    // ───────── D5 装档那一步的缓存写（m110 · O16） ─────────
    println!(
        "\n【D5】`install` 末尾那两行：`path.clone()` + `meta_cache.insert` vs 先比值再决定写不写"
    );
    println!("  装档不只发生在换图：同一张的原图档升级、SVG 切片重渲、动画**每一帧**都进来一次");
    let paths = [
        (
            "本地短路径",
            PathBuf::from(r"E:\Photos\2024\DSC-000123.jpg"),
        ),
        (
            "本地长中文名",
            PathBuf::from(
                r"E:\tmp\AI.Agent\RuPix\.temp\m110-samples\IMG-20240131-婚礼全家福-最终版-精选-第0312张.jpeg",
            ),
        ),
        (
            "UNC 病态",
            PathBuf::from(
                r"\\nas01\photo-library\2024\wedding\final-cuts\IMG-20240131-wedding-family-full-portrait-final-selection-no-0312.jpeg",
            ),
        ),
    ];
    for (tag, p) in &paths {
        println!(
            "  键长 {tag}：{} B（克隆＝一次堆分配 +  memcpy）",
            p.as_os_str().len()
        );
        install_equivalence(p, (3000, 2000), (4000, 3000));
        let (now, gate, gate_write) = install_arms(mult, p, (3000, 2000), (4000, 3000));
        report_ns(&now);
        report_ns(&gate);
        report_ns(&gate_write);
        println!(
            "  → 同值重复装档（动画/换档那一档）：现形 {:.1} ns ⇒ 门闸 {:.1} ns，省 {:+.1} ns/帧；按 m91 记的 21.7 帧/s 摊＝{:+.2} µs/s",
            now.mid,
            gate.mid,
            now.mid - gate.mid,
            (now.mid - gate.mid) * 21.7 / 1000.0
        );
        println!(
            "  → 值变了那一档（新图/首装）：门闸 {:.1} ns vs 现形 {:.1} ns＝多付 {:+.1} ns（多出来的那次比值），\n     门闸省下的只有\"同值重复\"这一档，换图路径一次都不省——别把两档混着报。",
            gate_write.mid,
            now.mid,
            gate_write.mid - now.mid
        );
    }
    println!(
        "  → 每格＝2,000 次一批摊平；三臂起手相同（表里已有这张图），差的只许是\"要不要写\"。"
    );

    // ───────── D6 自然序排序键里那两枚中间 String（m111 · O17） ─────────
    println!("\n【D6】`natural_key` 数字段：`&format!(\"{{num:020}}\")` + `&(end-i).to_string()` vs `write!` 直写");
    let names = natural_key_corpus(3000);
    // 先数清"这一册里到底有多少条形在付钱"：直写只在**数字段**上少两次分配，
    // 非数字段那两条臂走的是同一句 `push_str(&s[i..end])`。分母不说清，后面那句"省几 %"就是空话。
    let (mut hit_items, mut hit_runs) = (0usize, 0usize);
    for n in &names {
        let mut runs = 0usize;
        let b = n.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i].is_ascii_digit() {
                runs += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        if runs > 0 {
            hit_items += 1;
            hit_runs += runs;
        }
    }
    println!(
        "  册子构成：{} 项，其中 {} 项含数字段、数字段合计 {} 枚 ⇒ 直写的上限是每枚少 {} 次堆分配",
        names.len(),
        hit_items,
        hit_runs,
        2
    );
    // 等价控制①：每项的键逐字节相同；②：整册排序后的**名字顺序**相同。两样都过才有资格比快慢。
    natural_key_equivalence();
    assert_eq!(
        natural_sorted_names(&names, natural_key_now),
        natural_sorted_names(&names, natural_key_written),
        "整册顺序不同＝直写改了排序语义，不是等价改写"
    );
    // 钉子跑在两臂上：落地后**直写臂＝生产**，现形臂是落地前的旧形；两条都必须逐项等于生产
    // （它们逐字节同键，所以这条钉子同时防两件事：副本漂了、以及"旧形臂其实和今天跑的不是一条路"）。
    // 格式来源要先挂上：`.png` 由解码器认领（wic `auto_claim=false` 进不了清单），
    // 不挂就得到一张空册——空册会让钉子"什么都不比"地绿过去，正是量具骗人。
    rupix_lib::decoders::install_format_source();
    let pin = natural_key_pin_corpus();
    natural_key_pin_to_production(&pin, natural_key_now);
    natural_key_pin_to_production(&pin, natural_key_written);
    let iters = 60 * mult;
    let now_keys = measure("旧形 整册建键".to_string(), iters, || {
        for n in &names {
            std::hint::black_box(natural_key_now(n));
        }
    });
    let written_keys = measure("直写＝生产 整册建键".to_string(), iters, || {
        for n in &names {
            std::hint::black_box(natural_key_written(n));
        }
    });
    report(&now_keys);
    report(&written_keys);
    let saved_ms = now_keys.mid - written_keys.mid;
    println!(
        "  → 整册（{} 项、{} 枚数字段）：旧形 {:.3} ms vs 直写 {:.3} ms ⇒ 落地省下 {:+.3} ms/册 ＝ 每枚数字段 {:+.1} ns",
        names.len(),
        hit_runs,
        now_keys.mid,
        written_keys.mid,
        saved_ms,
        if hit_runs > 0 {
            saved_ms * 1_000_000.0 / hit_runs as f64
        } else {
            0.0
        }
    );
    // ★ 对照臂：只有"零枚数字段"那一形。直写在这条形上一个分配都没少，所以两臂之差**必须**落在噪声里。
    //   这条存在的意义：若它也"省"了同样多，说明量的根本不是那两枚 String，而是两臂之间的别的东西。
    let no_digit: Vec<String> = names
        .iter()
        .filter(|s| !s.as_bytes().iter().any(u8::is_ascii_digit))
        .cloned()
        .collect();
    let nd_now = measure("对照·无数字段 旧形".to_string(), iters, || {
        for n in &no_digit {
            std::hint::black_box(natural_key_now(n));
        }
    });
    let nd_written = measure("对照·无数字段 直写".to_string(), iters, || {
        for n in &no_digit {
            std::hint::black_box(natural_key_written(n));
        }
    });
    report(&nd_now);
    report(&nd_written);
    println!(
        "  → 对照（{} 项、0 枚数字段）：{:+.3} ms｜两臂同形处应当读不出差别，读得出＝量具在骗人",
        no_digit.len(),
        nd_now.mid - nd_written.mid
    );
    // 端点：生产真正付的那一步＝扫目录后整本自然序排序（建键 + 比较）。
    let now_sort = measure("端点：旧形整本排序".to_string(), iters, || {
        std::hint::black_box(natural_sorted_names(&names, natural_key_now));
    });
    let written_sort = measure("端点：直写整本排序".to_string(), iters, || {
        std::hint::black_box(natural_sorted_names(&names, natural_key_written));
    });
    report(&now_sort);
    report(&written_sort);
    println!(
        "  → 端点占比：整本排序旧形 {:.3} ms，直写（今天的生产）{:.3} ms ⇒ 省 {:.1}%（分母＝用户翻页/开册等到的那一步）",
        now_sort.mid,
        written_sort.mid,
        (now_sort.mid - written_sort.mid) / now_sort.mid * 100.0
    );

    // ───────── D7 draw_image 的"每次固定开销 vs 每像素"（m115 · O26） ─────────
    // 由头（m114）：标题栏那颗**已预渲染成位图、目标框只有 26×26** 的一次 `draw_image` 要 0.17–0.25 ms，
    // 而按 512² 那一档拟合出的面积系数（~8 ns/像素）它只该值 ~0.005 ms ⇒ 单价不是一条过原点的直线。
    // 这一档把两件事分开量：**每像素**（面积项）与**每次调用**（固定项），并分三形：
    //   ① 不透明（a=255，m69 那条"1:1 不透明快路"够得着）
    //   ② 半透明（a=64，走 mask——生产那颗旧 4K 臂就是这一形）
    //   ③ 半透明 + `Fit::Contain`（**置顶按钮今天用的就是这一形**）
    // 外加一臂"同一颗 26×26 换缓冲尺寸"⇒ 把**窗口项**（整幅 Mask 按窗口造）从固定项里摘出来。
    println!("\n【D7】draw_image 单价拆开：固定项 vs 面积项 vs 窗口项（同进程，阶梯正反两序）");
    {
        let mk = |w: u32, h: u32, a: u8| -> Image {
            let mut rgba = vec![0x40u8; (w as usize) * (h as usize) * 4];
            for px in rgba.chunks_exact_mut(4) {
                px[3] = a;
            }
            Image::from_rgba(w, h, &rgba).unwrap_or_else(|e| panic!("造 {w}×{h} 位图失败：{e}"))
        };
        let sizes: [(u32, u32); 5] = [(1, 1), (26, 26), (128, 128), (512, 512), (1920, 1080)];
        let mut pm = Pixmap::new(1920, 1080).expect("1920×1080 离屏缓冲造不出");
        let mut pm_small = Pixmap::new(800, 600).expect("800×600 离屏缓冲造不出");
        let opaque: Vec<(u32, u32, Image)> =
            sizes.iter().map(|&(w, h)| (w, h, mk(w, h, 255))).collect();
        let trans: Vec<(u32, u32, Image)> =
            sizes.iter().map(|&(w, h)| (w, h, mk(w, h, 64))).collect();
        // dst 尺寸**与源尺寸分开传**：1:1 那一形吃的是 windui 补丁 015 的整行拷贝快路，
        // 而它的门槛写死了「不透明 且 物理 1:1 且 整数平移」⇒ 缩放形（生产的常态）走不进那条路，
        // 单价必须单独量，不许拿 1:1 的价去套主画面与胶片条。
        let bench =
            |buf: &mut Pixmap, img: &Image, dw: u32, dh: u32, fit: Fit, name: String| -> Stats {
                let area = (dw as usize) * (dh as usize);
                let inner = if area > 1_000_000 {
                    1
                } else if area > 10_000 {
                    20
                } else {
                    200
                };
                measure_each(name, 200 * mult, inner, || {
                    let mut c = SkiaCanvas::new(buf);
                    c.draw_image(img, Rect::new(10, 10, dw as i32, dh as i32), fit, 0.0, 1.0);
                })
            };
        let mut rows: Vec<(String, usize, f64)> = Vec::new();
        for (tag, set, fit) in [
            ("不透明 Fill", &opaque, Fit::Fill),
            ("半透明 Fill", &trans, Fit::Fill),
            ("半透明 Contain", &trans, Fit::Contain),
        ] {
            let mut v: Vec<&(u32, u32, Image)> = set.iter().collect();
            // 阶梯**正反各跑一遍**（K：尾部噪声要靠反序复跑才看得住），每档取两次中位的最小值。
            let mut twice: Vec<(String, usize, f64)> = Vec::new();
            for rev in [false, true] {
                if rev {
                    v.reverse();
                }
                for (w, h, img) in &v {
                    let area = (*w as usize) * (*h as usize);
                    let s = bench(
                        &mut pm,
                        img,
                        *w,
                        *h,
                        fit,
                        format!("{tag} {w}×{h}｜{}", if rev { "反" } else { "正" }),
                    );
                    let label = format!("{tag} {w}×{h}");
                    match twice.iter_mut().find(|r| r.0 == label) {
                        Some(slot) => slot.2 = slot.2.min(s.mid),
                        None => twice.push((label, area, s.mid)),
                    }
                }
            }
            rows.extend(twice);
        }
        for (label, area, ns) in &rows {
            println!(
                "  {label:<20} 面积 {:>9} px ｜ 每次 {:>9.1} ns ｜ 每像素 {:>7.3} ns",
                area,
                ns,
                ns / *area as f64
            );
        }
        // 两系数从**同一形**的端点解出来：固定项取最小档实测（不是外推），面积项取 (t大-t小)/(面积差)。
        // ★ 两点拟合只在"直线真的过原点附近"时有意义：不透明那一形读数是**非线性**的
        //   （小图快到大图 0.1→0.4 ns/px，缓存效应），硬解会算出负的固定项 ⇒ 那种情形如实打"不成立"。
        for tag in ["不透明 Fill", "半透明 Fill", "半透明 Contain"] {
            let small = rows.iter().find(|r| r.0 == format!("{tag} 1×1"));
            let pin = rows.iter().find(|r| r.0 == format!("{tag} 26×26"));
            let big = rows.iter().find(|r| r.0 == format!("{tag} 1920×1080"));
            if let (Some(s), Some(p), Some(b)) = (small, pin, big) {
                let slope = (b.2 - s.2) / (b.1 - s.1) as f64;
                let explained = slope * p.1 as f64;
                if explained >= p.2 {
                    println!(
                        "  → {tag}：两点拟合**不成立**（面积项 {slope:.3} ns/px 已经解释不掉 26×26 那档的 {:.1} ns）\n     ⇒ 这一形的成本不是直线，读上表逐档，别用单系数外推（K129）",
                        p.2,
                    );
                } else {
                    println!(
                        "  → {tag}：面积项 ≈ {slope:.3} ns/px（由 1 px 与 2,073,600 px 两端解出）｜\n     1×1 实测 {:.1} ns ＝ 每次调用的固定项下限｜26×26 实测 {:.1} ns，\n     其中面积只解释 {explained:.1} ns ⇒ **固定项 ≈ {:.1} ns，占 {:.0}%**",
                        s.2,
                        p.2,
                        p.2 - explained,
                        (p.2 - explained) / p.2 * 100.0,
                    );
                }
            }
        }
        // ── 缩放档（源 ≠ 目标）：生产的两处 blit 全是这一形 ──
        // ① 胶片条一格＝缩略图 240×180 经 `filmstrip::contain` 落进 96×68 的格子 ⇒ 90×68；
        //    一格的目标面积 6,120 px，`filmstrip::capacity(800)`＝7 格、`capacity(1920)`＝18 格。
        // ② 主画面＝解码出来的大图进视口框（≤100% 时是降采样）。
        // 不透明那档是关键对照：**同为目标像素**，1:1 与缩放差多少 ⇒ 补丁 015 那条快路的门槛
        // 是不是就是那笔钱。半透明那档给的是通用路径的真实价。
        let scaled_arms: [(&str, u32, u32, u32, u32); 3] = [
            ("胶片条一格", 240, 180, 90, 68),
            ("降采样 512²→128²", 512, 512, 128, 128),
            ("降采样 1080p→800×450", 1920, 1080, 800, 450),
        ];
        let mut sarms: Vec<(String, Image, u32, u32)> = Vec::new();
        for (name, sw, sh, dw, dh) in scaled_arms {
            for (tag, a) in [("不透明", 255u8), ("半透明", 64u8)] {
                sarms.push((format!("{tag} 缩放 {name}"), mk(sw, sh, a), dw, dh));
            }
        }
        let mut srows: Vec<(String, usize, f64, usize, usize)> = Vec::new();
        for rev in [false, true] {
            let order: Vec<&(String, Image, u32, u32)> = if rev {
                sarms.iter().rev().collect()
            } else {
                sarms.iter().collect()
            };
            for (label, img, dw, dh) in order {
                let s = bench(
                    &mut pm,
                    img,
                    *dw,
                    *dh,
                    Fit::Fill,
                    format!("{label}｜{}", if rev { "反" } else { "正" }),
                );
                let area = (*dw as usize) * (*dh as usize);
                match srows.iter_mut().find(|r| r.0 == *label) {
                    Some(slot) => slot.2 = slot.2.min(s.mid),
                    None => srows.push((label.clone(), area, s.mid, *dw as usize, *dh as usize)),
                }
            }
        }
        for (label, area, ns, dw, dh) in &srows {
            println!(
                "  {label:<26}→{dw}×{dh} 目标 {:>9} px ｜ 每次 {:>10.1} ns ｜ 每目标像素 {:>7.3} ns",
                area,
                ns,
                ns / *area as f64
            );
        }
        // 同目标面积的 1:1 与缩放对照：只有 128² 那一档两边都有。
        if let (Some(s), Some(o)) = (
            srows
                .iter()
                .find(|r| r.0 == "不透明 缩放 降采样 512²→128²")
                .map(|r| r.2),
            rows.iter()
                .find(|r| r.0 == "不透明 Fill 128×128")
                .map(|r| r.2),
        ) {
            println!(
                "  → 不透明、同 16,384 个目标像素：缩放 {s:.1} ns ｜ 1:1 {o:.1} ns ⇒ **缩放形贵 {s:.0}/{o:.0} ＝ {:.1}×**\n     ⇒ 补丁 015 的整行拷贝门槛（物理 1:1）就是这一笔；生产的缩放 blit 一分钱都吃不到",
                s / o
            );
        }
        if let (Some(s), Some(o)) = (
            srows
                .iter()
                .find(|r| r.0 == "半透明 缩放 降采样 512²→128²")
                .map(|r| r.2),
            rows.iter()
                .find(|r| r.0 == "半透明 Fill 128×128")
                .map(|r| r.2),
        ) {
            println!(
                "  → 半透明、同 16,384 个目标像素：缩放 {s:.1} ns ｜ 1:1 {o:.1} ns ⇒ {:.2}×（通用路径里缩放本身不算主项）",
                s / o
            );
        }
        // 摊到胶片条那一格：capacity(800)=7 格 / capacity(1920)=18 格，与 m113 的 0.77–1.69 ms/帧 对账。
        if let Some(tile) = srows
            .iter()
            .find(|r| r.0 == "不透明 缩放 胶片条一格")
            .map(|r| r.2)
        {
            let alpha = srows
                .iter()
                .find(|r| r.0 == "半透明 缩放 胶片条一格")
                .map(|r| r.2)
                .unwrap_or(0.0);
            println!(
                "  → 胶片条整排（一格 {tile:.1} ns 不透明 / {alpha:.1} ns 半透明）：\n     7 格 ＝ {:.2} ms / {:.2} ms ｜ 18 格 ＝ {:.2} ms / {:.2} ms（不透明/半透明）\n     对账 m113 实测的胶片条段 0.77–1.69 ms/帧（还含每格一次 `fill_round_rect`）",
                tile * 7.0 / 1e6,
                alpha * 7.0 / 1e6,
                tile * 18.0 / 1e6,
                alpha * 18.0 / 1e6
            );
        }
        // 窗口项：同一颗 26×26，只把**缓冲**从 1920×1080 换成 800×600。
        let pin_t = trans.iter().find(|(w, _, _)| *w == 26).unwrap();
        let win_big = bench(
            &mut pm,
            &pin_t.2,
            26,
            26,
            Fit::Contain,
            "窗口项 26×26 @1920×1080".to_string(),
        );
        let win_small = bench(
            &mut pm_small,
            &pin_t.2,
            26,
            26,
            Fit::Contain,
            "窗口项 26×26 @800×600  ".to_string(),
        );
        report_ns(&win_big);
        report_ns(&win_small);
        let (dmin, dmid) = (win_big.min - win_small.min, win_big.mid - win_small.mid);
        if dmin.signum() == dmid.signum() && dmin.abs() > 2_000.0 {
            println!(
                "  → 窗口项：缓冲 1920×1080 → 800×600，最小列 {dmin:+.1} ns、中位列 {dmid:+.1} ns（两列同号才立案）\n     ⇒ 除这一笔之外与缓冲面积无关的，才是**每次调用**的固定开销",
            );
        } else {
            println!(
                "  → 窗口项：**读不出来**（最小列 {dmin:+.1} ns ｜ 中位列 {dmid:+.1} ns，两列不同号或小于 2 µs）\n     ⇒ 本轮没证据说单价里有缓冲面积那一项（框架注释里的『整幅 mask』在这条路上没留下读数）",
            );
        }
    }

    // ───────── 二-② 每次按键的格式清单 ─────────
    println!("\n【二-②】is_previewable()：加锁 + 克隆 HashSet（按键路径，每次调用）");
    rupix_lib::decoders::install_format_source();
    let declared = rupix_lib::core::formats::declared_exts();
    if declared.is_empty() {
        println!(
            "  跳过：解码器目录里读不到任何自述清单（设 RUPIX_DECODERS_DIR 指向发布目录再跑）。"
        );
        println!("  ★ 这一条没有读数就不构成结论，别拿空跑当已量。");
        return;
    }
    println!(
        "  清单规模：全部自述 {} 项、裸数据 {} 项",
        declared.len(),
        rupix_lib::core::formats::declared_raw_exts().len()
    );
    let iters = 5000 * mult;
    let per = measure("is_previewable(\"jpg\") 整句".to_string(), iters, || {
        assert!(rupix_lib::core::formats::is_previewable("jpg"))
    });
    let one = measure(
        "只取 declared_exts()（一次锁 + 一份克隆）".to_string(),
        iters,
        || assert!(!rupix_lib::core::formats::declared_exts().is_empty()),
    );
    let hold = {
        let set = declared;
        measure(
            "下限：拿住一份再 contains（修法的形状）".to_string(),
            iters,
            || assert!(set.contains("jpg")),
        )
    };
    for s in [&per, &one, &hold] {
        report(s);
    }
    println!(
        "  → 每次按键 {:.2} µs｜克隆那部分 {:.2} µs｜纯查表 {:.3} µs ⇒ 修法上限省 {:.0}%",
        per.mid * 1000.0,
        (per.mid - hold.mid) * 1000.0,
        hold.mid * 1000.0,
        (1.0 - hold.mid / per.mid) * 100.0
    );
}
