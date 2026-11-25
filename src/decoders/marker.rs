//! 解码器加载面包屑：`<dll>.loading`。
//!
//! # 为什么要有它
//!
//! 解码器与宿主同进程，坏 `DllMain` / 静态初始化能把进程直接带走。`invoke` 的
//! SEH 闸门能接住访问违规，但**进程级崩溃**接不住；更糟的是坏 DLL 每次启动都会
//! 在同一个位置再崩一次，形成启动崩溃循环。这里在 `LoadLibrary` **之前**写一枚
//! 标记，正常返回（成功 / 普通错误 / Rust panic）由 [`LoadingGuard`] 的 `Drop`
//! 清除；进程崩溃或 SEH 硬故障跳过析构，标记留在盘上。下次 `inspect` 看到它就
//! 跳过该 DLL，用户可在设置 → 解码器详情里清除标记并重试。
//!
//! # 为什么写在 DLL 旁边
//!
//! 便携程序只认 exe 同级目录；标记与 DLL 同名（`<dll>.loading`）最直观，删 DLL
//! 时残留标记也不会被当成解码器扫描（目录扫描只认 `*.dll`）。写失败（只读介质 /
//! 杀软）只记一条 warn，加载照常进行——不能因为面包屑写不下去就不让用户看图。

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 标记后缀。`common_0.2.6.dll` → `common_0.2.6.dll.loading`。
const SUFFIX: &str = ".loading";

/// 本进程的身份。OS 可能复用 pid，单靠 pid 不够；用启动时刻混进来。
fn process_nonce() -> u64 {
    static NONCE: OnceLock<u64> = OnceLock::new();
    *NONCE.get_or_init(|| {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        t ^ ((std::process::id() as u64) << 32)
    })
}

/// 本进程里**已经出过硬故障**的 DLL：它们留下的标记不能被当成"另一个线程正在加载"。
static FAULTED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// 某个 DLL 的标记路径。
pub(super) fn path_for(dll: &Path) -> PathBuf {
    let mut s = dll.as_os_str().to_os_string();
    s.push(SUFFIX);
    PathBuf::from(s)
}

/// 盘上的标记是不是**上一次进程崩溃**留下的。
///
/// 不能只看"文件在不在"：同一进程里另一个线程正在 `LoadLibrary` 时文件也在，
/// 旧判据会把并发扫描误报成「上次加载崩溃，本次跳过」（2026-11-24 审计 M13）。
/// 标记里写进程 nonce：同 nonce = 本进程正在加载（不算残留）；不同 nonce /
/// 没有 nonce 的老格式 = 残留；本进程已记过硬故障 = 残留。
pub(super) fn stale(dll: &Path) -> bool {
    if faulted(dll) {
        return true;
    }
    let Ok(s) = std::fs::read_to_string(path_for(dll)) else {
        return false;
    };
    match s
        .lines()
        .find_map(|l| l.strip_prefix("nonce=").and_then(|v| v.parse::<u64>().ok()))
    {
        Some(v) => v != process_nonce(),
        None => true,
    }
}

/// 记一笔"本进程在这个 DLL 上出过硬故障"（SEH / panic 穿出加载闸门）。
pub(super) fn note_fault(dll: &Path) {
    let mut g = FAULTED.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashSet::new).insert(dll.to_path_buf());
}

fn faulted(dll: &Path) -> bool {
    FAULTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|s| s.contains(dll))
}

/// 清除标记（用户点「清除标记并重试」）。标记本来就不在也算成功。
pub(super) fn clear(dll: &Path) -> std::io::Result<()> {
    // 硬故障记录也一起清：用户明确要求重试，不能还被同一进程的内存标记挡住。
    if let Some(s) = FAULTED.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.remove(dll);
    }
    match std::fs::remove_file(path_for(dll)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// 一次加载的标记守卫：正常离开作用域即删标记；进程崩溃 / SEH 硬故障跳过析构，
/// 标记留下。
pub(super) struct LoadingGuard {
    path: PathBuf,
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("解码器加载标记清除失败 {}：{e}", self.path.display());
            }
        }
    }
}

/// 在 `LoadLibrary` 之前写标记。写失败返回 `None`（加载照常，只是没有面包屑）。
///
/// ★ **原子落盘（m349）**：先写完整内容到临时名，再 `rename` 成正式标记名。
///   旧实现 `File::create` → `write_all` 两步之间有一段"文件已在、内容还是空"的
///   窗口——多车道（m349 T1）或多窗口（m211）并发起 worker 时，另一台 worker 的
///   `stale()` 恰好在这段窗口里读到**没有 nonce 行的空文件**，按"无 nonce = 残留"
///   判真，把这枚 DLL 从**那台 worker 的目录里永久跳过**（本次会话它解不了任何
///   该 DLL 认领的格式，全部落进"无人认领"兜底）。真机证据：T3 复测一次 5 worker
///   并发启动，一台 worker 缺 common，一半预读解码全报嗅探兜底，`[dll-serial]`
///   留下 7 枚 DLL × 每笔的嗅探风暴。rename 在 NTFS 上原子：读者要么看不到文件、
///   要么看到完整内容，空窗结构性消失。
///
///   崩溃保护语义不变：临时文件阶段（`LoadLibrary` **之前**）崩溃不留正式标记——
///   那时还没进 DLL，无需保护；`rename` 成功后才进 `LoadLibrary`，此后崩溃照旧
///   留下完整标记。临时文件不叫 `*.dll`，目录扫描不会把它当解码器；进程在写入
///   中被硬杀只会留一个无害的临时残留。
pub(super) fn begin(dll: &Path) -> Option<LoadingGuard> {
    let path = path_for(dll);
    // 临时名带进程 nonce：同进程并发 begin 互不覆盖；跨进程 nonce 必不同。
    let mut tmp = path.clone().into_os_string();
    tmp.push(format!(".tmp{}", process_nonce()));
    let tmp = PathBuf::from(tmp);
    let body = format!(
        "nonce={}\npid={}\nstarted={:?}\n",
        process_nonce(),
        std::process::id(),
        std::time::SystemTime::now()
    );
    let staged = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        // 不 sync 的话，进程在 `LoadLibrary` 里崩掉时这枚文件可能还留在页缓存里。
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = staged {
        log::warn!(
            "解码器加载标记写入失败 {}：{e}（本次无面包屑保护）",
            path.display()
        );
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!(
            "解码器加载标记落盘失败 {}：{e}（本次无面包屑保护）",
            path.display()
        );
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(LoadingGuard { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("decoder-marker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dll = dir.join("common_9.9.9.dll");
        (dir, dll)
    }

    #[test]
    fn guard_writes_marker_and_clears_it_on_normal_drop() {
        let (dir, dll) = scratch("guard");
        {
            let guard = begin(&dll).expect("临时目录可写，标记必须建得出来");
            assert!(
                !stale(&dll),
                "本进程正在加载的标记不算崩溃残留（并发扫描不能误报）"
            );
            drop(guard);
        }
        assert!(!stale(&dll), "正常返回后标记必须清掉");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn leftover_marker_is_detected_and_can_be_cleared() {
        let (dir, dll) = scratch("leftover");
        // 模拟上次崩溃留下的标记：不走 guard，直接落盘。
        std::fs::write(path_for(&dll), b"pid=1\n").unwrap();
        assert!(stale(&dll), "残留标记必须能看见");
        clear(&dll).unwrap();
        assert!(!stale(&dll), "清除后必须放行重试");
        // 再清一次也成功（幂等）。
        clear(&dll).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn marker_path_keeps_the_dll_extension() {
        let p = path_for(Path::new(r"C:\x\common.dll"));
        assert_eq!(p.file_name().unwrap(), "common.dll.loading");
    }

    /// ★ m349：标记必须**原子可见**——并发的 `stale()` 读侧在任何时刻都只能看到
    /// "无文件"或"完整内容"，绝不许看到半截/空文件（空文件无 nonce 行会被判成
    /// 崩溃残留，那台 worker 的目录就永久缺这枚 DLL——T3 真机复测抓到的启动竞态）。
    ///
    /// 写侧循环 begin→持住→drop，读侧同时高频 hammer `stale()`；守卫存活期间
    /// `stale()` 必须恒为 false。修复前的 create→write 空窗被 hammer 抓到过才会
    /// 有这条钉子（窗口极窄，这条测试主要是**回归防倒退**——把 begin 改回两步写
    /// 不一定每次都红，所以另有一条结构断言：`begin` 存活期间盘上若有标记，内容
    /// 必须带 nonce 行）。
    #[test]
    fn marker_is_never_seen_half_written_by_concurrent_stale() {
        let (dir, dll) = scratch("atomic");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let dll2 = dll.clone();
        let reader = std::thread::spawn(move || {
            let mut seen = 0;
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                if std::fs::exists(path_for(&dll2)).unwrap_or(false) {
                    seen += 1;
                    // 标记一旦可见就必须是完整的（nonce 行在）——空窗即红。
                    // 读到 NotFound = 写侧恰在此刻 drop 清掉，不算数（读侧自己的
                    // TOCTOU，不是被测窗口）。
                    match std::fs::read_to_string(path_for(&dll2)) {
                        Ok(body) => assert!(
                            body.contains("nonce="),
                            "读到了半截标记（原子性被破坏）：{body:?}"
                        ),
                        // NotFound = 写侧恰在此刻 drop 清掉；AccessDenied = Windows
                        // 的 delete-pending 瞬态（删除句柄还没关，读打开被拒）——两者
                        // 都是"标记正在消失"，不是被测的半截窗口。生产路径 `stale()`
                        // 读失败同样按"无标记"放行，无害。
                        Err(e)
                            if e.kind() == std::io::ErrorKind::NotFound
                                || e.raw_os_error() == Some(5) => {}
                        Err(e) => panic!("读标记出现非预期错误：{e}"),
                    }
                }
            }
            seen
        });
        for _ in 0..50 {
            let guard = begin(&dll).expect("标记必须建得出来");
            assert!(!stale(&dll), "守卫存活期间不算残留");
            drop(guard);
            assert!(!stale(&dll), "drop 之后标记必须清掉");
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let seen = reader.join().expect("读线程不该崩");
        assert!(
            seen > 0,
            "读侧至少要真实观察到一次标记存在，否则测试没量到东西"
        );
        // 没有任何临时残留：begin 成功即 rename 走人。
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件不许残留：{leftovers:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
