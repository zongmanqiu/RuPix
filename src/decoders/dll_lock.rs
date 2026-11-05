//! 按 DLL 串行：把 ABI 契约「宿主串行调用解码器（可重入、不必 MT-safe）」落成一把锁。
//!
//! # 为什么需要它（2026-11-05 裁决，`docs/待办.md` §28）
//!
//! m212 给每条解码 worker 装了**私有后端**（thread-local），目的是"一个解码器挂死钉不死别人"；
//! m211 又把多开改成单进程多窗口 ⇒ 每条窗口一条解码线程。两件事合起来的后果是：
//! **两条 worker 线程可以同时进入同一枚 DLL**。而 Windows 上同一个模块在一个进程里只映射一次，
//! 静态数据是共享的——契约承诺的"串行"此前**一行代码都没有**。
//!
//! # 粒度：一枚 DLL 的全路径，不是全局
//!
//! 全局锁正是 m212 拆掉的东西（一个挂死拖死所有图）。同一枚 DLL 的两笔解码必须排队；
//! 不同 DLL 的各解各的，互不相干。
//!
//! # 取锁是**先来先得**，不是抢
//!
//! 第一版是 `try_lock` 轮询（150 µs 起翻倍、封顶 4 ms）。真机双窗同格式实测抓到一个必现的
//! 反例：持有者一笔解完（~1.1 s）后，它的 supervisor 立刻取下一笔（drop → 收结果 → pop →
//! enter 全程 <1 ms），而另一条 worker 正睡在 4 ms 的轮询间隔里 ⇒ **赢家通吃**，输家连输
//! 5 笔、`wait=5313.7ms`（`th=sv-decode-w`，不是 UI 线程）。锁本身没错，错在"抢"。
//!
//! 现在每个进入者领一张递增票，排队 FIFO，释放时唤醒。等待**有界**：最多等于前面那笔的
//! 持锁时长。放弃（超预算）会把自己的票从队里摘掉并唤醒下一位——否则一次放弃会把队伍
//! 永久卡在"队首是一张没人认领的票"上。
//!
//! # 取锁是有界等待，不是无限等
//!
//! 挂死的 worker 会被监督者**丢弃**（线程泄漏，守卫永不释放）。后来者必须拿到一条可读裁决，
//! 而不是陪它一起卡死。真正挂死的解码器在上层就被 `is_hung_decoder` 挡住了
//! （`decode_with` 的第一道闸），所以这里通常等的是"另一个窗口正在正常解码这一张"，
//! 预算直接取 [`crate::core::decode::decode_timeout`] 那同一个数——宿主对"一次解码最多等多久"
//! 只该有一个定义。
//!
//! # 可重入
//!
//! 同一线程在同一枚 DLL 上嵌套调用（`decode_with` → `query_natural_only`、`sniff_ranked`
//! 里的逐枚探测）不再抢锁，否则自己把自己锁死。判据是线程本地的持有集合，不是靠调用方自律。
//!
//! # 覆盖范围（诚实的边界）
//!
//! 只有**有状态**的四个出口上锁：`iv_probe` / `iv_render` / `iv_render_raw` / `iv_render_region`。
//! `iv_abi_version` / `iv_decoder_info` / `iv_caps` / `iv_metadata` 是加载期读的一批常量导出
//! （`dllmeta::inspect`），**不上锁**：给它们上锁会让"一次目录重扫"与"别人的解码"互相等待，
//! 而重扫超时会被误记成"这个 DLL 加载失败"，代价比它挡掉的风险大得多。

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// `decode_timeout_s=0`（用户关掉挂死恢复）时的等待上限。
///
/// 即使那条路"允许无限等解码"，**等待一把锁**也不该无限——锁的持有者可能是被丢弃的僵尸线程。
const FALLBACK_WAIT: Duration = Duration::from_secs(30);

/// 票号发号器。只用于排队先后，与请求的 `seq` 无关。
fn next_ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 一枚 DLL 的调用权：一把**公平**的锁。
///
/// `waiters` 是票号队列，`held` 表示调用权在谁手里。判据只有一条：**队首 + 没人持有**。
/// 这样"刚放手的持有者立刻回来抢下一笔"不能插队——它得排到队尾，与别人同规矩。
///
/// # 两档：用户的一笔在前，后台预取在后（m237）
///
/// 跨窗共用同一枚 DLL 时，两种请求在这里相遇。此前只有一档先来先得，于是"没人等的下一张"
/// 可以挡在"用户正等的那张"前面。真机实测（26.7 MP × 12 张、双窗各 250 ms 翻页）：
/// 用户那一笔 `wait` 稳定 ~950 ms（= 一整笔解码），首帧 p95 从单窗的 1025 ms 涨到 2871 ms。
/// 现在 `Urgent` 只排在 `Urgent` 后面，`Background` 还要等 `Urgent` 全空——用户最多等
/// **正在跑的那一笔**（不可抢占），不再为别人的预取排队。
///
/// ★ 预取会不会饿死：不会长期。用户翻页有间隔，`Urgent` 一空 `Background` 立刻放行；
/// 而且预取本来就可以不做（[`super::decode`] 的判断只把它当"空闲时消耗 CPU"）。
struct Cell {
    state: Mutex<CellState>,
    cv: Condvar,
}

/// 排队档位。`Urgent` = 用户此刻在等的那一笔（主画面解压、区域重渲、RAW 参数）；
/// `Background` = 邻图预取（`ReqKind::Prefetch`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    Urgent,
    Background,
}

#[derive(Default)]
struct CellState {
    held: bool,
    /// 用户请求的票号，队首先放行。
    urgent: VecDeque<u64>,
    /// 后台预取的票号，**只在 `urgent` 全空时**放行。
    background: VecDeque<u64>,
}

impl Cell {
    fn new() -> Self {
        Self {
            state: Mutex::new(CellState::default()),
            cv: Condvar::new(),
        }
    }

    /// 锁中毒只可能由"在锁内 panic"造成，而这里的临界区只有几行记账；
    /// 真撞上也拿回数据继续用（与 `config`/`with_backend` 同策），不把一次毒化升级成解码器故障。
    fn lock_state(&self) -> MutexGuard<'_, CellState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 排队拿调用权；`false` = 预算耗尽（票已摘掉，队伍不会被卡住）。
    fn acquire(&self, ticket: u64, budget: Duration, class: Class) -> bool {
        let started = Instant::now();
        let mut st = self.lock_state();
        match class {
            Class::Urgent => st.urgent.push_back(ticket),
            Class::Background => st.background.push_back(ticket),
        }
        loop {
            if !st.held && Self::is_next(&st, ticket, class) {
                match class {
                    Class::Urgent => st.urgent.pop_front(),
                    Class::Background => st.background.pop_front(),
                };
                st.held = true;
                return true;
            }
            let left = budget.saturating_sub(started.elapsed());
            if left.is_zero() {
                match class {
                    Class::Urgent => st.urgent.retain(|t| *t != ticket),
                    Class::Background => st.background.retain(|t| *t != ticket),
                }
                drop(st);
                // 我可能就是队首：摘掉之后必须叫醒后面那位，否则队伍停在"队首没人认领"上。
                self.cv.notify_all();
                return false;
            }
            let (g, _) = self
                .cv
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner());
            st = g;
        }
    }

    /// 这一张票现在轮到了吗。后台票必须等**用户队列全空**，这是两档的全部差别。
    fn is_next(st: &CellState, ticket: u64, class: Class) -> bool {
        match class {
            Class::Urgent => st.urgent.front() == Some(&ticket),
            Class::Background => st.urgent.is_empty() && st.background.front() == Some(&ticket),
        }
    }

    fn release(&self) {
        {
            let mut st = self.lock_state();
            st.held = false;
        }
        self.cv.notify_all();
    }
}

/// 进程级锁表：`DLL 全路径 → 那一枚 Cell`。
///
/// `Box::leak` 是有意的：条目数 = 目录里的 DLL 数（十几枚），而 `Guard` 必须活到进程结束——
/// 表重建一次，旧守卫就悬空了。一枚 Cell 只多占一个队列头与条件变量。
fn table() -> &'static Mutex<HashMap<PathBuf, &'static Cell>> {
    static TABLE: OnceLock<Mutex<HashMap<PathBuf, &'static Cell>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cell_for(key: &Path) -> &'static Cell {
    let mut g = table().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cell) = g.get(key) {
        return cell;
    }
    let cell: &'static Cell = Box::leak(Box::new(Cell::new()));
    g.insert(key.to_path_buf(), cell);
    cell
}

thread_local! {
    /// 本线程已持有哪几枚 DLL（`路径 → 深度`）。只用来判重入，不判"谁该等谁"。
    static HELD: RefCell<HashMap<PathBuf, u32>> = RefCell::new(HashMap::new());
}

/// 本线程此刻是否持有该 DLL 的锁（诊断与单测用）。
pub(super) fn held_here(key: &Path) -> bool {
    HELD.with(|h| h.borrow().contains_key(key))
}

/// `RUPIX_DECODE_TRACE` 只问系统一次：解码热路径不该每次都读环境块。
fn trace_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUPIX_DECODE_TRACE").is_some())
}

/// 一串"这段时间这枚 DLL 归我"的凭证；离开作用域即释放。
pub(super) struct Guard {
    key: PathBuf,
    /// `None` = 本次是**同线程重入**，锁本来就在自己手里。
    cell: Option<&'static Cell>,
    /// 本次是否重入（只影响诊断行里的 `re=` 字段）。
    reentrant: bool,
    /// `(id, 拿到锁的那一刻, 已等待时长, 线程名)`。只有开了 `RUPIX_DECODE_TRACE` 才填，
    /// 生产路径零成本（不分配、不读时钟）。线程名是判据 4 的关键：`sv-decode-w` = 解码
    /// worker，`main` = UI 线程。**UI 线程出现在这里就意味着界面会被别人的解码拖住**。
    trace: Option<(String, Instant, Duration, String)>,
}

impl Guard {
    /// 造一张凭证；`since` = 真正拿到调用权的时刻，`waited` = 为此排队多久。
    fn new(key: PathBuf, cell: Option<&'static Cell>, id: &str, waited: Duration) -> Self {
        let reentrant = cell.is_none();
        let trace = if trace_on() {
            Some((
                id.to_string(),
                Instant::now(),
                waited,
                std::thread::current().name().unwrap_or("?").to_string(),
            ))
        } else {
            None
        };
        Self {
            key,
            cell,
            reentrant,
            trace,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // 先记下"这笔解码到此为止"的时刻，再松锁：日志本身要几十微秒，
        // 记在松锁之后会把持锁区间量短（m229 判据 4 的 `[dll-serial]` 就靠这个数）。
        let ended = self.trace.as_ref().map(|_| Instant::now());
        if let Some(cell) = self.cell {
            cell.release();
        }
        // 先松锁再清持有集：反过来会让"刚释放"那一瞬间的重入判据仍然为真。
        HELD.with(|h| {
            let mut g = h.borrow_mut();
            match g.get_mut(&self.key) {
                Some(n) if *n > 1 => *n -= 1,
                _ => {
                    g.remove(&self.key);
                }
            }
        });
        // 一行一笔。**早退/失败路径也走 Drop**，所以"每次进入 DLL 恰好一行"由类型保证，
        // 不靠调用方记得打点。区间 = [行时刻 - run, 行时刻]。
        if let (Some((id, since, waited, th)), Some(end)) = (self.trace.as_ref(), ended) {
            log::info!(
                "[dll-serial] decoder={id} th={th} wait={:.1}ms run={:.1}ms re={}",
                waited.as_secs_f64() * 1000.0,
                end.duration_since(*since).as_secs_f64() * 1000.0,
                u8::from(self.reentrant)
            );
        }
    }
}

/// 拿这枚 DLL 的调用权（预算 = 宿主的单笔解码超时）。
pub(super) fn enter(key: &Path, id: &str, class: Class) -> Result<Guard, String> {
    let budget = crate::core::decode::decode_timeout().unwrap_or(FALLBACK_WAIT);
    enter_within(key, id, budget, class)
}

/// 同 [`enter`]，但预算由调用方给（单测用它把等待压到毫秒级）。
pub(super) fn enter_within(
    key: &Path,
    id: &str,
    budget: Duration,
    class: Class,
) -> Result<Guard, String> {
    if held_here(key) {
        HELD.with(|h| *h.borrow_mut().entry(key.to_path_buf()).or_insert(0) += 1);
        return Ok(Guard::new(key.to_path_buf(), None, id, Duration::ZERO));
    }

    let cell = cell_for(key);
    let started = Instant::now();
    if !cell.acquire(next_ticket(), budget, class) {
        let ms = started.elapsed().as_millis();
        log::warn!(
            "[dll-lock] decoder={id} path={} 等待 {ms} ms 未拿到调用权，本次放弃",
            key.display()
        );
        return Err(format!(
            "解码器「{id}」正被另一个窗口占用（等待 {ms} ms 未释放）"
        ));
    }
    HELD.with(|h| {
        h.borrow_mut().insert(key.to_path_buf(), 1);
    });
    Ok(Guard::new(
        key.to_path_buf(),
        Some(cell),
        id,
        started.elapsed(),
    ))
}
