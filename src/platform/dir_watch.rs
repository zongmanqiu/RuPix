//! 打开目录的文件增删 / 改名监控（Windows `ReadDirectoryChangesW`）。
//!
//! 看图器打开一张图 = 打开它所在的整个目录（`core::gallery`）。在资源管理器里
//! 往这个目录增删 / 改名图片时，清单应当自动刷新，否则状态栏的「3/5」会和磁盘
//! 对不上——这是 loupe 用 `gio::FileMonitor` 解决的同一件事。这里用 Win32 原生
//! API，零额外依赖。
//!
//! 去抖策略（比 loupe 的 60ms 更轻）：监控线程只在目录句柄上跑 `ReadDirectoryChangesW`，
//! 醒来就向通道发一个 `DirEvent::Changed`，**不去重、不去抖**——去抖交给上层 16ms
//! 轮询（一次 tick 只重扫一次，天然把一整波拷贝合并成一次 `read_dir`）。
//! `Drop` 先 `CancelIoEx` 再关句柄，阻塞中的线程随即拿到错误返回、自行退出；
//! ★ `Drop` **不等它**（不 `join`）——这条结构体是在 UI 线程上被析构的，等不到醒来就是
//! 整个进程退不掉。理由与安全性写在 [`DirWatcher::drop`]，事故形状见 `docs/踩坑记录.md` K74。

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::CancelIoEx;

/// 目录内容变化信号（通道里只放这一个变体；保留枚举便于将来区分"改名 / 增删"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirEvent {
    /// 本目录下的文件 / 子目录增、删、改名。
    Changed,
}

/// 目录监控器：持有一个后台线程，目录一变就往通道发 `DirEvent::Changed`。
///
/// 重新监控另一个目录 = `drop` 旧的、再 `new` 一个；上层（runtime）按
/// "当前看图目录" 与 "正在监控的目录" 比对，不一致就重臂。
pub struct DirWatcher {
    // 目录句柄正本。线程只持其 `usize` 副本（见 [`watch_loop`] 注释）；正本留着就是为了
    // `Drop` 能打到它——先 `CancelIoEx` 取消那条挂起的读、再关句柄，线程随即拿到错误返回。
    // （把 `HANDLE` 整个移进线程，外面就没有下手的地方了。）
    handle: HANDLE,
    stop: Arc<AtomicBool>,
    /// 只用来**放手**（drop 掉 `JoinHandle` = detach），从不 `join`——见 [`DirWatcher::drop`]。
    thread: Option<JoinHandle<()>>,
}

impl DirWatcher {
    /// 监控 `dir`。失败（句柄打不开，如目录已被删）返回 `Err`，调用方忽略即可。
    pub fn new(dir: &Path, tx: Sender<DirEvent>) -> std::io::Result<Self> {
        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        let stop = Arc::new(AtomicBool::new(false));
        // `HANDLE` 不保证 `Send`，改传其数值副本；`usize` 一定 `Send`。
        let handle_raw = handle.0 as usize;
        let stop_thread = stop.clone();
        let thread = thread::Builder::new()
            .name("dir-watch".into())
            .spawn(move || watch_loop(handle_raw, tx, stop_thread))
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok(Self {
            handle,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 先取消挂起的通知读，再关句柄。只关句柄**不保证**能叫醒阻塞中的
        // `ReadDirectoryChangesW`（微软给的正规打断方式是 `CancelIoEx(h, NULL)`——
        // 它取消这个句柄上**所有线程**发起的 I/O，正合我们"读在另一条线程上"的形状）。
        unsafe {
            let _ = CancelIoEx(self.handle, None);
            // 正本关掉：线程只持同值的 `usize` 副本，不在此处二次关闭（避免重复关同一内核对象）。
            let _ = CloseHandle(self.handle);
        }
        // ★ **绝不 `join()`**。本结构体被 16 ms 轮询闭包持有，而那个闭包属于框架的宿主状态——
        //   windui 是在 `WM_DESTROY` 里、也就是 **UI 线程上**把它 drop 的。这里一旦等不到后台线程
        //   醒来，UI 线程就停在 `DestroyWindow` 里面，`PostQuitMessage` 已入队却没人去读，
        //   `App::run()` 永不返回 ⇒ 退出收尾（回写 ini、摘钩子、删会话戳）整段不执行，
        //   任务管理器里留下一枚"没有窗口、也没有托盘图标可唤回"的进程
        //   （2026-10-19 用户报，`docs/待办.md` §8 / K74；旧写法就是 `let _ = t.join();`）。
        //
        // 放手（detach）在这里是安全的，不是省事：线程只拿三样**自己拥有**的东西——句柄数值的
        // `usize` 副本、`Sender`（所有权已 move 进去）、`Arc<AtomicBool>`——不借 `DirWatcher`
        // 的任何字段，所以结构体先走它也不会碰到悬垂引用。而它下一步必然拿到错误返回
        // （I/O 已取消 + 句柄已关）或发现 `stop` 已置位，随即 `break`；进程退出时 OS 收掉残余。
        let _ = self.thread.take();
    }
}

/// 后台线程主体：在目录句柄上循环 `ReadDirectoryChangesW`，醒来即广播 `Changed`。
///
/// `raw` 是 `HANDLE.0` 的数值副本——`HANDLE` 不保证 `Send`，故用 `usize` 跨线程；
/// 它与 `DirWatcher::handle` 指向同一内核对象，正本被 `Drop` 关掉后，这里阻塞的
/// 调用会返回错误，循环随即 `break`。
fn watch_loop(raw: usize, tx: Sender<DirEvent>, stop: Arc<AtomicBool>) {
    let h = HANDLE(raw as *mut c_void);
    // 64KB 缓冲：足够吞下一波批量增删，避免 `ERROR_NOTIFY_INFORMATION` 溢出丢文件名
    // （我们本来也不解析文件名，溢出只意味着"有变化"，不影响语义）。
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut bytes = 0u32;
        let r = unsafe {
            ReadDirectoryChangesW(
                h,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                Some(&mut bytes),
                None,
                None,
            )
        };
        if r.is_err() {
            // 句柄被 `Drop` 关掉（目录被删 / 重臂）都会走到这——正常退出。
            break;
        }
        // 物理变化的**唯一**信号点：让清单缓存作废（`core::gallery::note_dir_change`）。
        // 放在 `send` 之前——先作废再通知，避免某扇窗收到事件后按旧代次命中缓存。
        // ★ N 扇窗就有 N 个 watcher，于是这里一次变化会递增 N 次；这是**故意**的：
        //   代次只需要单调，而"多推几次"只会让缓存更早失效（偏保守），不会让它更晚。
        crate::core::gallery::note_dir_change();
        if tx.send(DirEvent::Changed).is_err() {
            break; // 接收端（runtime）已走，自然退出。
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::TryRecvError;
    use std::time::Instant;

    /// 一次性临时目录（名字带 pid + 纳秒，跑多少轮都不撞）。
    fn temp_dir() -> std::path::PathBuf {
        let p = crate::core::paths::test_scratch_root().join(format!(
            "rupix-dirwatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("临时目录要能建");
        p
    }

    /// ★ `Drop` 不许在调用线程上等后台线程醒来——这是 2026-10-19 那枚
    /// 「窗口没了、进程还在任务管理器里」的唯一形状（`docs/待办.md` §8 / K74）：
    /// 结构体在 UI 线程的 `WM_DESTROY` 里析构，旧写法在这里 `join`，而"关句柄能叫醒
    /// `ReadDirectoryChangesW`"并不被保证 ⇒ UI 线程停在 `DestroyWindow` 里面，
    /// `WM_QUIT` 入队没人读，`App::run()` 永不返回，退出收尾整段不执行。
    ///
    /// 两条断言各管一半：
    /// ① drop 的墙钟必须是个位数毫秒量级（给了 100 ms 的宽闸，机器再慢也不该碰线）；
    /// ② 放手之后线程**确实自己走掉了**——判据用通道：线程持有唯一那份 `Sender`，
    ///    它一退出，`rx` 就拿到 `Disconnected`（不数线程、不赌 sleep，看得见的是通道）。
    #[test]
    fn drop_never_waits_for_the_watch_thread_but_the_thread_still_goes_away() {
        let dir = temp_dir();
        let (tx, rx) = std::sync::mpsc::channel::<DirEvent>();
        let w = DirWatcher::new(&dir, tx.clone()).expect("监控要装得上");
        // 让它真的堵在 `ReadDirectoryChangesW` 上（没这一步，join 老写法也不会卡，测不到病灶）。
        std::thread::sleep(std::time::Duration::from_millis(80));

        let t = Instant::now();
        drop(w);
        let dropped = t.elapsed();
        assert!(
            dropped < std::time::Duration::from_millis(100),
            "Drop 在调用线程上等了 {dropped:?}——它不许 join 后台线程"
        );
        drop(tx); // 外面这份也放掉：此后只剩监控线程那份，它退出即 Disconnected

        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match rx.try_recv() {
                Err(TryRecvError::Disconnected) => break,
                // Empty = 那条线程还活着（还在读或还在循环里）。
                Err(TryRecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "放手之后 5 秒内监控线程没退出：它在漏（每换一次目录留一条）"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Ok(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 放手不改变功能：换目录（旧 watcher 析构、新的装上）之后，增删仍然要报变化。
    ///
    /// 这条是"detach 不是把机制改坏"的正面证据：句柄正本关掉之后，新臂用**新句柄**
    /// 重新登记通知，旧线程再怎么收尾都影响不到它。
    ///
    /// ★ **变化要反复造，不能只造一次**（2026-10-19 实测：写一次就等的版本
    /// 20 轮里假红 **10 轮**）。原因是注册与事件之间有先后竞争——后台线程要跑到
    /// `ReadDirectoryChangesW` 那一刻才开始收通知，而测试主线程 `spawn` 完立刻就写文件，
    /// 那次创建整个落进"读还没开始"的窗口里，就永远等不到（没有第二次变化）。
    /// 生产侧同理会漏，但漏了无害：换目录时上层本来就要重扫清单。
    #[test]
    fn rearming_the_same_folder_still_delivers_changes() {
        let dir = temp_dir();
        let (tx, rx) = std::sync::mpsc::channel::<DirEvent>();
        let first = DirWatcher::new(&dir, tx.clone()).expect("第一臂要装得上");
        drop(first);
        let _second = DirWatcher::new(&dir, tx.clone()).expect("第二臂要装得上");

        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let mut n = 0usize;
        loop {
            // 每轮造一个新文件：直到有一轮它落在"读已注册"之后，事件才会出现。
            n += 1;
            std::fs::write(dir.join(format!("newly-added-{n}.png")), b"x")
                .expect("样本文件要写得进");
            match rx.try_recv() {
                Ok(DirEvent::Changed) => break,
                Err(TryRecvError::Disconnected) => panic!("通道断了：新臂的线程没接上手"),
                _ => {
                    assert!(
                        Instant::now() < deadline,
                        "换目录之后连造 {n} 次文件，5 秒内一次都没收到变化事件"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        // 测试结束时 `_second` 析构：不许在这里等后台线程（同一判据，见上一条）。
        let _ = std::fs::remove_dir_all(&dir);
    }
}
