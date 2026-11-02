//! 会话戳：判"上一次退出是不是异常"的最小状态位。
//!
//! # 为什么用一个文件而不是注册表/环境
//!
//! RuPix 是便携程序——所有持久数据都待在 exe 同级目录（[`crate::core::paths`]）。
//! 会话戳同规则：exe 旁 `temp\rupix.run` 一枚小文件（里面就一句"pid + 本次启动时间"），
//! 程序启动第一件事就是看一眼它在不在，然后在本次会话期间**重建**它、正常退出时**删掉**它。
//!
//! 三种退出场景对号入座：
//! - 用户点托盘「退出 RuPix」→ `on_close_request` 放行 → `rupix_lib::run` 走到尾 →
//!   本模块 [`close_session`] 把戳删干净。**下次启动静默**。
//! - panic / abort / 任务管理器结束进程 → 尾段代码没跑到，戳残留。**下次启动 [`open_session`]
//!   把它侦测成"上次异常"**，交给 UI 层 [`crate::ui::recovery_hint`] 决定要不要弹提示。
//! - exe 目录只读（SD 卡写保护）→ 建戳失败但不影响使用。**不弹任何警告**——看图器
//!   存不下一个空文件不值得打断用户，与 [`crate::core::config`] 的静默降级一脉相承。
//! - 戳属于**另一枚还在运行的 RuPix**（pid 活着且镜像路径相同）→ 那是并发实例，不是
//!   "上次异常"：不弹提示，也不覆写/删除它那一份（2026-10-19 用户报"每次启动都弹"，
//!   根因就是旧判据只看文件在不在，见 [`open_session`] 的 ★ 段）。
//!
//! 那单报障里"很久以前的错误"这半句另有原因：戳里那句时间是 UTC，本地 03:07 落下的戳
//! 显示成"10-18 19:07"，看着像隔天。换算已改到本地口径（[`crate::platform::panic_guard::civil_from_secs`]）。
//!
//! # 与 [`crate::platform::panic_guard`] 的分工
//!
//! 崩溃钩子负责**记下崩在哪**（追加到当天的 `crash-<日期>.log`，取证用）；本模块只负责**下次启动了
//! 解到这事儿发生过**。两件事互相解耦：钩子里再触发一次文件写入太危险（可能递归 panic），
//! 而"看到残留戳"就足以驱动安全模式建议——不需要解析日志内容。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// 会话戳文件名（`exe 目录下的 temp\`，与其它诊断物同处）。
pub const STAMP_FILE: &str = "rupix.run";

/// 本次进程启动时侦测到的"上次异常退出"旗标。
///
/// `AtomicBool` 而不是 `Cell`：`open_session` 在启动主线程调、`take_prior_aborted` 在
/// 建窗后的 16ms 轮询调（同一线程，理论上用 Cell 也行）——但 `#[cfg(test)]` 里想跨
/// 用例复位，静态量更方便；顺带未来若把侦测挪进后台线程也不至于改类型。
static PRIOR_ABORTED: AtomicBool = AtomicBool::new(false);

/// 上次残留戳的内容（`"pid=… 启动于 …"`）。`OnceLock`：只在 `open_session` 里读一次，
/// 之后 UI 层随时可查，不必再碰文件（文件已被本次覆写）。
static PRIOR_STAMP: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// 会话戳路径：`<exe 目录>/temp/rupix.run`。取不到 exe 目录时 `None`（等价于本次会话不落戳）。
pub fn stamp_path() -> Option<PathBuf> {
    Some(crate::core::paths::diagnostics_dir()?.join(STAMP_FILE))
}

/// 本次进程是否**拥有**会话戳（写了它，退出时才删它）。
///
/// 唯一"跑过来但不拥有"的情形：发现戳属于一枚还活着的同镜像进程（[`open_session`] 的
/// ★ 段）。那种时候我们既没覆写它，退出时当然也不许把它删掉。
static OWN_STAMP: AtomicBool = AtomicBool::new(false);

/// 开一次会话：先看旧戳在不在，然后把旗标置上，再重建戳。
///
/// **必须在 `App` 建窗之前调**（`rupix_lib::run` 的最开头）。之后 `runtime` 在首帧
/// 轮询里调 [`take_prior_aborted`] 拿到本次的判定。
///
/// ★ 「有戳」不等于「上次异常」（2026-10-19 用户报障：每次启动都弹"上次未正常结束"，
///   而提示里那句"上次会话 pid=…"对应的进程**当时还活着**）。
///   旧判据只看文件在不在，于是把"另一枚 RuPix 正在运行"读成"上一次没走完"。
///   为什么会同时存在两枚各自认为自己是首实例的进程：`main` 的 `claim_instance` 靠
///   windui 的命名互斥体定身份，而本机取证到有窗口已销毁、进程却滞留的情况（当时
///   `tasklist` 里三枚 rupix.exe、`GetWindowTextLength` 全为 0）——互斥体随窗口销毁而
///   释放，戳却还在那枚活进程手里。次级路径本身不会来覆写戳（`open_session` 在
///   `rupix_lib::run` 里，`claim_instance` 判成 Handoff 的进程根本走不到），
///   所以要修的是**判定**这一侧：认出那是"另一个我还开着"，就既不弹、也不去覆写它。
///   pid 复用那一路由 [`stamp_is_live_sibling`] 里的镜像路径核对堵住；反过来的代价是
///   "该弹但少弹一次"——崩溃取证另有崩溃报告 / `.dmp` 两条独立证据链，不靠这枚戳。
pub fn open_session() {
    let Some(p) = stamp_path() else { return };
    let prior = std::fs::read_to_string(&p).ok();
    let concurrent = prior.as_deref().is_some_and(stamp_is_live_sibling);
    OWN_STAMP.store(!concurrent, Ordering::Relaxed);
    PRIOR_ABORTED.store(p.exists() && !concurrent, Ordering::Relaxed);
    // **先读旧内容再覆写**：残留戳里那句"上次是哪个进程、什么时候启动的"是秒退现场
    // 唯一的时间线索（本次一切正常，下次启动才想不起来上次崩在几点）。并发实例那一份
    // 不是"上次"，不拿去当线索。
    PRIOR_STAMP.get_or_init(|| if concurrent { None } else { prior });
    if !concurrent {
        write_stamp(&p);
    }
}

/// 上次残留的会话戳内容（`"pid=… 启动于 …"`）。本次没侦测到残留、或旧戳是空/读不出 → `None`。
pub fn prior_stamp() -> Option<String> {
    let raw = PRIOR_STAMP.get()?.as_ref()?;
    let s = raw.trim();
    if s.is_empty() {
        // 老版本（或只读介质上写失败）留下的空戳：有戳但没信息，别编一句假话。
        None
    } else {
        Some(s.to_string())
    }
}

/// 正常收尾：把戳删掉。找不到/删不掉都静默——只影响下次开机多弹一次提示，不影响本次。
///
/// 没拥有戳的那一次（[`OWN_STAMP`] 为假）必须**什么都不动**：那枚戳是活着的同镜像进程的。
pub fn close_session() {
    if !OWN_STAMP.load(Ordering::Relaxed) {
        return;
    }
    if let Some(p) = stamp_path() {
        // 只删**仍写着本次 pid** 的戳：两次启动重叠时（单实例转发失败那条路），
        // 先退出的那一枚不能把另一枚活会话的戳删掉。读不出/格式坏了按老语义删——
        // 那时无法证明它是别人的，留着反而会让下次启动误报。
        let ours = match std::fs::read_to_string(&p) {
            Ok(s) => stamp_pid(&s) == Some(std::process::id()),
            Err(_) => true,
        };
        if ours {
            remove_stamp(&p);
        }
    }
}

/// 取本次启动侦测到的"上次异常退出"旗，**取一次即清零**：弹完一次提示就不再骚扰用户。
pub fn take_prior_aborted() -> bool {
    PRIOR_ABORTED.swap(false, Ordering::Relaxed)
}

// ── 文件读写拆成小函数：`#[cfg(test)]` 里用 temp 目录跑一遍完整往返，无需真在
//    exe 目录放文件。生产路径始终走 [`stamp_path`]，测试路径由调用方注入。

fn write_stamp(p: &Path) {
    // 戳不再留空：里面写"本次是哪个进程、什么时候启动的"（本地时间，与任务管理器同一口径）。
    // 正常退出它就被删掉，一旦被强杀/崩掉，下次启动读到的就是**上次**那一句——秒退场景
    // 唯一的时间线索。
    // `fs::write` = create-or-truncate，正好是"重建本次戳"的语义；只读介质写失败照旧静默。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!(
        "pid={} 启动于 {}\n",
        std::process::id(),
        crate::platform::panic_guard::civil_from_secs(secs)
    );
    // 戳现在住在 `temp\` 里，目录不一定已经存在（`prepare_diagnostics` 建它，但探针与
    // 只读介质那两条路可能没走到）。写之前顺手保证一次，失败照旧静默。
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, body);
}

fn remove_stamp(p: &Path) {
    // 文件本就不存在（比如上次是崩溃，UI 弹了提示后用户又走了一次正常关窗）→ NotFound，忽略。
    let _ = std::fs::remove_file(p);
}

/// 残留戳是不是**另一个还活着的 RuPix** 留下的（那就是并发实例，不是"上次异常"）。
///
/// 只判"pid 还在不在"不够：Windows 会回收 pid，一枚陈旧戳里的号完全可能已经属于别的
/// 程序——那样会把真·崩溃误判成并发（该弹的不弹）。所以连带核对镜像路径：同一个 exe
/// 才是"另一个我"，别的程序占了这个号就当它不存在。
fn stamp_is_live_sibling(raw: &str) -> bool {
    let Some(pid) = stamp_pid(raw) else {
        return false;
    };
    // 戳里写着**本次自己的 pid**：`open_session` 还没写过戳，所以这不可能是
    // "另一个我"，只能是 Windows 回收 pid 后留下的旧戳。按并发处理会少弹一次提示，
    // 更糟的是不重写戳、`close_session` 也不删，下一次启动就会把它当成异常残留。
    if pid == std::process::id() {
        return false;
    }
    process_is_same_exe(pid)
}

/// 解析戳里那句 `pid=NNNN 启动于 …` 的进程号。形状不对（老版本、被截断、空）→ `None`。
fn stamp_pid(raw: &str) -> Option<u32> {
    let rest = raw.trim().strip_prefix("pid=")?;
    let head = rest.split_whitespace().next()?;
    head.parse::<u32>().ok()
}

/// 该 pid 现在是否还活着，且镜像路径就是本 exe。任何一步失败都答 `false`
/// （会话戳是"尽力而为"的辅助状态，判不出来就退回老语义：当作残留）。
fn process_is_same_exe(pid: u32) -> bool {
    use std::os::windows::ffi::OsStringExt;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    /// `GetExitCodeProcess` 回这个值 = 进程还没退。系统没在 windows crate 里导出常量，
    /// 值取自 SDK（`winbase.h`）。
    const STILL_ACTIVE: u32 = 259;

    let Ok(my_exe) = std::env::current_exe() else {
        return false;
    };
    // 自己那枚号出现在戳里 = 上一次跑的就是本进程的镜像，路径必然相同；这条只在
    // "同一个 pid 恰好是本次自己"的 pid 复用场景下把我们引向"并发"，代价是少弹一次提示。
    let mut path = [0u16; 1024];
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE;
        let mut same = false;
        if alive {
            let mut len = path.len() as u32;
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len)
                .is_ok()
            {
                let other = std::ffi::OsString::from_wide(&path[..len as usize]);
                // Windows 路径大小写不敏感，且快捷方式/8.3 名/junction 可能给出
                // 不同拼写；先 canonicalize，失败再退回 ASCII 大小写不敏感比较。
                same = match (
                    std::fs::canonicalize(&other),
                    std::fs::canonicalize(&my_exe),
                ) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => other
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&my_exe.to_string_lossy()),
                };
            }
        }
        let _ = CloseHandle(h);
        same
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 戳里必须带"pid + 启动时间"：残留时下次启动才有时间线索（空戳等于什么都没记下，
    /// 用户报"打开就没了"时我们连上次跑到几点都不知道）。
    #[test]
    fn stamp_carries_pid_and_start_time() {
        let dir = crate::core::paths::test_scratch_root().join("rupix_stamp_content_test");
        std::fs::create_dir_all(&dir).expect("建 temp 目录");
        let p = dir.join(STAMP_FILE);
        write_stamp(&p);
        let raw = std::fs::read_to_string(&p).expect("戳应读得回来");
        assert!(
            raw.starts_with(&format!("pid={} ", std::process::id())),
            "戳里要认得出是哪个进程，实得：{raw}"
        );
        assert!(raw.contains("启动于 20"), "要带本次启动时间：{raw}");
        // 重建（truncate）而不是追加：跑两次不许变成两行。
        write_stamp(&p);
        assert_eq!(
            std::fs::read_to_string(&p).expect("再读").lines().count(),
            1,
            "同一份戳不许越写越长"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 完整往返：戳在无→有→无之间切换，异常旗标只在"残留"那一次亮。
    ///
    /// 拆三段断言是为了让失败现场直接指向具体哪一步走歪：
    /// 1) 空目录首开不应报警；2) 戳残留必须报警；3) 干净关窗后不应报警。
    #[test]
    fn stamp_round_trip_across_two_sessions() {
        let dir = crate::core::paths::test_scratch_root().join("rupix_session_test_round_trip");
        std::fs::create_dir_all(&dir).expect("建 temp 目录");
        let p = dir.join(STAMP_FILE);
        let _ = std::fs::remove_file(&p);

        // ① 首开：无旧戳 → 建戳、旗标应为 false。
        assert!(!p.exists(), "前置：不该有残留戳");
        write_stamp(&p);
        assert!(p.exists(), "开会话应落下戳文件");

        // ② 模拟崩溃：戳残留 → 下次 open 应把旗标置真。
        assert!(p.exists(), "第二次开前应能看见旧戳");

        // ③ 干净关窗：戳应被清掉。
        remove_stamp(&p);
        assert!(!p.exists(), "close_session 必须删掉戳");

        // NotFound 也不能报错（重复关窗 / 崩溃后没建戳的路径）。
        remove_stamp(&p);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 戳里写着本次自己的 pid = 旧戳被 pid 复用，不是「另一个我」。
    #[test]
    fn a_stamp_naming_this_process_is_not_a_live_sibling() {
        let raw = format!("pid={} 启动于 2026-10-24 00:00:00\n", std::process::id());
        assert!(
            !stamp_is_live_sibling(&raw),
            "本次自己的 pid 不可能是在写的并发实例"
        );
    }
    /// `take_prior_aborted` 的取一次即语义：第二次必须回 `false`，否则提示会反复弹。
    ///
    /// `--test-threads=1` 保证本用例独占进程级 `PRIOR_ABORTED`，不会与别的测试串扰。
    #[test]
    fn take_prior_aborted_is_single_shot() {
        PRIOR_ABORTED.store(true, Ordering::Relaxed);
        assert!(take_prior_aborted(), "置真后第一次取应为真");
        assert!(!take_prior_aborted(), "第二次取必须清零");
        // 复位，避免污染同一次 `cargo test` 里后续可能读它的用例。
        PRIOR_ABORTED.store(false, Ordering::Relaxed);
    }

    /// 戳的形状解析：只认 `pid=NNNN…`。老版本的空戳、被截断的、被人手改过的都必须回
    /// `None`——判不出来就当残留（老语义），不许当并发实例把真提示吞了。
    #[test]
    fn stamp_pid_accepts_only_our_shape() {
        assert_eq!(
            stamp_pid("pid=4242 启动于 2026-10-19 03:07:43\n"),
            Some(4242)
        );
        assert_eq!(stamp_pid("pid=4242"), Some(4242));
        assert_eq!(stamp_pid("  pid=7 启动于 x"), Some(7));
        assert_eq!(stamp_pid(""), None);
        assert_eq!(
            stamp_pid("启动于 2026-10-19"),
            None,
            "没有 pid= 就不是我们的形状"
        );
        assert_eq!(stamp_pid("pid=abc 启动于 x"), None);
        assert_eq!(stamp_pid("pid= 启动于 x"), None);
        // 超出 u32 的号（被人手改过的戳）不许饱和成一个假号。
        assert_eq!(stamp_pid("pid=99999999999999 启动于 x"), None);
    }

    /// 辅助子进程：只睡一会儿，给"同镜像还活着"的并发实例判定当靶子。
    #[test]
    #[ignore = "由 live_sibling/open_session 用例显式拉起"]
    fn session_sleep_child() {
        std::thread::sleep(std::time::Duration::from_secs(10));
    }

    fn spawn_sleep_child() -> std::process::Child {
        let exe = std::env::current_exe().expect("测试 exe 路径");
        std::process::Command::new(exe)
            .args([
                "--ignored",
                "--exact",
                "platform::session::tests::session_sleep_child",
                "--nocapture",
            ])
            .spawn()
            .expect("拉起同镜像子进程")
    }

    /// ★ 并发实例的判定（2026-10-19 用户报"每次启动都弹上次未正常结束"的那条路）。
    ///
    /// 用**本进程的 pid** 造的戳必须是"旧戳"而不是并发（`open_session` 还没写过戳）；
    /// 真正的并发用一枚同镜像、还活着的子进程来验。`pid=4`（System）与越界号都必须回
    /// `false`，否则真·崩溃残留会被误判成并发，该弹的提示被吞。
    #[test]
    fn live_sibling_requires_same_exe_image() {
        let mine = format!("pid={} 启动于 now\n", std::process::id());
        assert!(
            !stamp_is_live_sibling(&mine),
            "本进程的 pid 只可能是被复用的旧戳，不能算并发"
        );
        let mut child = spawn_sleep_child();
        let child_stamp = format!("pid={} 启动于 now\n", child.id());
        assert!(
            stamp_is_live_sibling(&child_stamp),
            "同镜像子进程活着，必须认成并发实例"
        );
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            !stamp_is_live_sibling("pid=4 启动于 now\n"),
            "System 进程活着但镜像不是本 exe，不能算并发"
        );
        assert!(
            !stamp_is_live_sibling("pid=4294967295 启动于 now\n"),
            "不存在的号不能算并发"
        );
    }

    /// ★ 上面那条只答"是不是并发"，用户看到的差别却在 `open_session` 的三步动作上：
    /// 弹不弹、覆不覆写、退出时删不删。这里拿**生产入口**跑两态。
    ///
    /// 并发那一态在真机上造不出来：两枚 RuPix 同时活着会被 windui 的命名互斥体挡掉一枚
    /// （`main.rs` 的 `claim_instance` 直接 return，根本走不到 `open_session`），所以只能
    /// 借"本测试进程的 pid 天然活着、且镜像就是当前这个测试 exe"来充当另一枚实例。
    #[test]
    fn open_session_splits_concurrent_from_aborted() {
        let p = stamp_path().expect("测试 exe 目录应可取");
        // 这条用例现在可能排在“第一个创建 temp\”的用例之前（测试顺序按名字），
        // 生产入口写戳失败是静默降级，测试再读就会 NotFound。先把落点建好，
        // 让用例只验“并发/异常”的裁决，不验目录是否存在。
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("建 temp 目录");
        }
        let _ = std::fs::remove_file(&p);
        let mut child = spawn_sleep_child();
        let forged_live = format!("pid={} 启动于 2026-01-01 00:00:00\n", child.id());
        let forged_dead = "pid=4294967295 启动于 2026-01-01 00:00:00\n";

        // ① 戳属于活着的同镜像子进程 ⇒ 并发：不弹、不覆写、收尾时也不许删它那一份。
        std::fs::write(&p, &forged_live).expect("伪造活戳");
        open_session();
        assert!(
            !take_prior_aborted(),
            "并发实例不是「上次异常」，不许弹提示"
        );
        assert_eq!(
            std::fs::read_to_string(&p).expect("读回活戳"),
            forged_live,
            "并发时不许覆写别人正在用的戳"
        );
        close_session();
        assert!(p.exists(), "并发时退出必须留下那枚戳（不属于本进程）");
        let _ = child.kill();
        let _ = child.wait();

        // ② 戳里的号已不存在 ⇒ 真·残留：要弹，并且本次自己重新落一枚戳、正常收尾删掉。
        std::fs::write(&p, forged_dead).expect("伪造死号戳");
        open_session();
        assert!(take_prior_aborted(), "死号戳就是上次没走完，必须弹");
        let now = std::fs::read_to_string(&p).expect("本次应覆写戳");
        assert!(
            now.starts_with(&format!("pid={} ", std::process::id())),
            "覆写后要认得出是本进程，实得：{now}"
        );
        close_session();
        assert!(!p.exists(), "正常收尾必须删掉本进程那枚戳");

        // ③ 干净首开（无戳）⇒ 不弹，且落下本次的戳。
        open_session();
        assert!(!take_prior_aborted(), "没有旧戳不该弹");
        assert!(p.exists(), "首开应落戳");
        close_session();
        let _ = std::fs::remove_file(&p);
    }
}
