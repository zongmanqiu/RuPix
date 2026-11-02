//! 解码挂死看门狗：**检测 + 日志**（P1.1 / C1 期）。
//!
//! # 它解决什么
//!
//! 解码线程是**单条串行**的。它一旦在解码器里卡死（死循环、驱动层的无响应
//! 解码、mmap 打在坏掉的网络盘上），后面所有请求都会饿死——**画面停在旧图上，
//! 日志里一片安静**。`catch_unwind` / SEH 隔离层只拦崩不拦卡，这是本仓明说的边界。
//!
//! C1 不改架构、不碰解码线程、不加依赖，只做一件事：
//! **把"无声挂死"变成"可判读现场"**——哪个文件、哪类请求、卡了多久、队列积压几条。
//! 判读现场的人（我们自己或用户）据此止损：`--safe-mode` 启动、换个解码器。
//!
//! # 为什么不能只有日志就满意
//!
//! 不能——C1 救不活线程。但"看不见"比"看得见但没救"更糟：无声挂死连"是哪个文件
//! 卡住了"都答不出来，用户只能干等并怀疑是软件坏了。
//!
//! # 判定口径
//!
//! - **首报阈值 10 s**：实测最重的整解约 1.98 s（m58 注），10 s 是它的一倍再留足
//!   顶格 64 MP 与慢盘的余量；再低会误报（误报比不报更坏，用户会学会忽略它）。
//! - **之后每 +10 s 升报一次**，直到销账——一次挂死只留一条 warn 会被淹没在日志里，
//!   而"卡了 40 秒"和"卡了 10 秒"是两回事。
//! - **多笔在途时只报最老的那笔**：挂死时队列后面会堆一串，全报等于刷屏；
//!   最老的那笔才是根因。
//! - **销账即停**：请求有回包（含过期早退的空串 Err Done）就销账，不再报。
//!
//! # 边界（写在前面，免得被当成"防卡死"卖）
//!
//! 这是**检测**，不是**防护**：它不打断、不杀、不恢复。挂死期间解码线程仍然占着，
//! 后续请求仍然饿死。真解是独立解码进程（见 `.temp/p11-hang-watchdog.md` §3-A），
//! 那要另立工程。C1 的价值是把现场变成可读的那几行字。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::core::decode::ReqKind;

/// 首报阈值（毫秒）。见模块头「判定口径」。
pub const FIRST_REPORT_MS: u64 = 10_000;

/// 在途请求的登记上限。挂死时用户狂按方向键会堆请求，但登记表不该无限长
/// （它只是给日志用的现场记录，不是队列本身）。
const MAX_PENDING: usize = 64;

/// 一笔在途请求（等回包）。
#[derive(Debug, Clone)]
pub struct Pending {
    pub seq: u64,
    pub kind: ReqKind,
    pub path: PathBuf,
    /// 发出时刻。
    pub at: Instant,
    /// 已经报过几轮（决定下一次什么时候再报）。
    pub reported: u32,
}

/// 到点要报的那一笔。
#[derive(Debug, Clone)]
pub struct WatchdogReport {
    pub seq: u64,
    pub kind: ReqKind,
    pub path: PathBuf,
    /// 已经卡了多久（毫秒）。
    pub age_ms: u64,
    /// 当前在途总数（积压多少条）。
    pub pending: usize,
}

/// 在途登记表（进程级；`send` 与 `note_done` 都在 UI 线程，只有巡检读它时
/// 才可能被别的线程看见——所以用 Mutex，但持有时间极短）。
static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

/// 登记一笔在途请求（`Decoder::send` 调）。
pub fn note_sent(seq: u64, kind: ReqKind, path: &std::path::Path) {
    let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() >= MAX_PENDING {
        // 丢最老的一笔：它多半已经报过了，留着只会让 pending 计数虚高。
        g.remove(0);
    }
    g.push(Pending {
        seq,
        kind,
        path: path.to_path_buf(),
        at: Instant::now(),
        reported: 0,
    });
}

/// 销账（收到回包时调；过期早退的空串 Err Done 也算回包）。
pub fn note_done(seq: u64) {
    let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = g.iter().position(|p| p.seq == seq) {
        g.remove(i);
    }
}

/// 当前在途条数（诊断用）。
pub fn pending_count() -> usize {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// 判定：**纯函数**——给定在途表、阈值与当前时刻，返回"到点该报的那一笔"。
///
/// 判定规则（模块头口径）：
/// - 条件：`age >= 阈值 × (已报次数 + 1)`（首报 = 阈值，之后每 +1 个阈值再报一次）；
/// - 多笔到点 → 只取**最老**的一笔（挂死时后面堆的请求不是根因）；
/// - 判定后把该笔的 `reported` 加一（下一次要再等一个阈值）。
///
/// 返回 `None` = 现在不该报。
pub fn watchdog_due(
    pending: &mut [Pending],
    threshold: Duration,
    now: Instant,
) -> Option<WatchdogReport> {
    let total = pending.len();
    // 找最老的"到点该报"的那一笔。
    let mut idx: Option<usize> = None;
    for (i, p) in pending.iter().enumerate() {
        let age = now.saturating_duration_since(p.at);
        let due = threshold * (p.reported + 1);
        if age < due {
            continue;
        }
        // 到点了：候选里取更老的（at 更早）。
        match idx {
            None => idx = Some(i),
            Some(j) => {
                if p.at < pending[j].at {
                    idx = Some(i);
                }
            }
        }
    }
    let i = idx?;
    let p = &mut pending[i];
    let age_ms = now.saturating_duration_since(p.at).as_millis() as u64;
    p.reported += 1;
    Some(WatchdogReport {
        seq: p.seq,
        kind: p.kind,
        path: p.path.clone(),
        age_ms,
        pending: total,
    })
}

/// 巡检（UI 线程的 16 ms 轮询调一次）：到点就记一条 warn。
///
/// 返回本次是否报了（供单测与调用方判断；UI 侧 C1 不弹提示，见模块头）。
pub fn watchdog_check() -> bool {
    let threshold = Duration::from_millis(FIRST_REPORT_MS);
    let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    match watchdog_due(&mut g, threshold, Instant::now()) {
        Some(r) => {
            log::warn!(
                "[decode-watchdog] file={} kind={:?} age_ms={} pending={} hint=用 --safe-mode 启动可临时绕开解码器",
                r.path.display(),
                r.kind,
                r.age_ms,
                r.pending
            );
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一笔"相对 `now` 已经卡了 `age_ms`"的在途请求。
    ///
    /// ★ 必须相对同一个 `now` 造样本：若样本用 `Instant::now()` 造、判定用更早的
    /// `now`，年龄会差出几毫秒，"刚好到点"的断言就成了掷硬币。
    fn pend_at(now: Instant, seq: u64, age_ms: u64, reported: u32) -> Pending {
        Pending {
            seq,
            kind: ReqKind::Main,
            path: PathBuf::from(format!("C:\\{seq}.png")),
            at: now - Duration::from_millis(age_ms),
            reported,
        }
    }

    /// 阈值之前不报；到点报一次；之后每 +1 个阈值再报一次。
    #[test]
    fn reports_at_threshold_then_every_interval() {
        let threshold = Duration::from_millis(10_000);
        let now = Instant::now();

        // 未到点。
        let mut v = vec![pend_at(now, 1, 9_999, 0)];
        assert!(watchdog_due(&mut v, threshold, now).is_none());

        // 到点 → 报一次。
        let mut v = vec![pend_at(now, 1, 10_000, 0)];
        let r = watchdog_due(&mut v, threshold, now).expect("到点应报");
        assert_eq!(r.age_ms, 10_000);
        assert_eq!(r.pending, 1);
        // 报完立刻再查 → 不该再报（要再等一个阈值）。
        assert!(watchdog_due(&mut v, threshold, now).is_none());

        // 再过一个阈值 → 又报。
        let later = now + Duration::from_millis(10_001);
        let r = watchdog_due(&mut v, threshold, later).expect("第二个阈值应再报");
        assert!(r.age_ms >= 20_000);
    }

    /// 多笔在途时只报**最老**的一笔（挂死时后面堆的请求不是根因）。
    #[test]
    fn multiple_pending_reports_the_oldest() {
        let threshold = Duration::from_millis(10_000);
        let now = Instant::now();
        // seq=1 卡了 30 s（最老）· seq=2 卡了 12 s · seq=3 卡了 20 s。
        let mut v = vec![
            pend_at(now, 1, 30_000, 0),
            pend_at(now, 2, 12_000, 0),
            pend_at(now, 3, 20_000, 0),
        ];
        let r = watchdog_due(&mut v, threshold, now).expect("应报");
        assert_eq!(r.seq, 1, "最老的那笔（seq=1）才是根因");
        assert_eq!(r.pending, 3, "积压条数要如实报出来");
    }

    /// 销账即停：登记 → 销账 → 不再报。
    #[test]
    fn done_clears_the_entry() {
        note_sent(9001, ReqKind::Main, std::path::Path::new("C:\\x.png"));
        assert!(pending_count() >= 1);
        note_done(9001);
        // 销账后这张图不该再被报出来。
        let threshold = Duration::from_millis(10_000);
        let mut v: Vec<Pending> = Vec::new();
        assert!(watchdog_due(&mut v, threshold, Instant::now()).is_none());
    }
}
