//! 崩溃兜底：让 panic 从"窗口无声消失"变成"看得见的线索"。
//!
//! # 为什么 UI 程序必须有这一层
//!
//! 本应用的全部交互都跑在系统回调 `wnd_proc`（`extern "system"`）里面，它不是 unwind ABI，
//! 也没有 `catch_unwind` 兜着（后台解码线程有，UI 线程没有）。意味着：
//!
//! ```text
//! panic 发生在 UI 线程 → 无法从这里展开出去 → 运行时 abort → 进程静默终止
//! ```
//!
//! 表象就是**点一下某个地方，整个窗口瞬间没了**，既没有报错也没有日志——
//! （canvas 里一次 `RefCell already borrowed` 就足以触发，详见 `ui::canvas` 的命中注释）。
//! 这类问题靠"看着代码想一想"很难定位，必须让它留下证据。
//!
//! # 它做什么
//!
//! 1. 把 panic 消息 + 线程 + 源码位置**追加**写到 `temp\crash-<今天>.log`
//!    （[`crate::core::paths`] 的落点规则，按天命名、启动清 7 天前的）；
//! 2. 顺带请 [`crate::platform::crash_evidence`] 写一份 `.dmp` 转储（panic 时进程还完整）；
//! 3. 弹一个错误框——主窗本身就是半透明无边框窗，错误框是独立顶层窗，不受那套影响；
//!    目的是让用户知道"程序崩了"而不是"我点错了某个按钮"。
//!
//! 之所以两者都做：日志适合事后取证，弹窗适合"用户当时就知道"。
//!
//! # 弹窗的三道闸（m128）
//!
//! [`MessageBoxW`] 会**泵本线程的消息队列**。在 UI 线程里弹它 ⇒ 别人的鼠标 / 计时器 /
//! 重绘消息在 panic 尚未收场时就重入 `wnd_proc`，第二枚 panic 落在钩子中间 ⇒ 进程直接
//! 没了、连这次已经落盘的证据都来不及看；而被 `catch_unwind` 已经处理掉的 panic
//! （解码器 FFI 一律走 `platform::seh::invoke`）也照弹不误，"这张图解不出来"被说成
//! "程序崩溃"。于是：
//!
//! 1. **受控边界内的 panic 不弹**（只落盘）：`seh::invoke` 期间的 panic 已由调用方处理；
//! 2. **弹窗挪到专门一条线程**：它的模态循环泵的是那条线程的队列，UI 线程的重入口子
//!    因此关上；
//! 3. **整进程只弹一次、UI 线程的 panic 才等它**：panic 风暴不该刷一屏框；UI 线程的
//!    panic 注定以 abort 收场（钩子返回后 unwind 出 `extern "system"`），那一枚必须
//!    等用户点掉才能让弹窗真的被看见。

//! # 便携落点
//!
//! 日志跟着 exe 走（[`crate::core::paths`]）：程序拷到 U 盘就地可读，删目录即净。
//! 便携目录写不进去（只读介质）时只弹框不写日志——证据留不下来，但至少用户知道崩了。

use std::io::Write;
use std::panic;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND};

/// 崩溃日志文件：`<exe 目录>/temp/crash-<今天>.log`（便携，不在用户配置目录留任何东西）。
pub fn log_path() -> Option<PathBuf> {
    crate::core::paths::crash_log_path(&today())
}

/// 本地日期 `YYYY-MM-DD`：诊断物的**文件名**用它，一天的日志与转储天然落进同一枚名字。
///
/// 进程内按"启动那一刻"定名（`RunLogSink` 建一次就一直写那一枚）——跨午夜的长会话
/// 会继续写昨天那枚，不去中途换文件：换名要重建写入器、把容量闸门也算第二遍，
/// 为几行日志不值当，而且"同一枚文件里最多混两天"不影响取证读法。
pub fn today() -> String {
    now().chars().take(10).collect()
}

/// 本地"今天"距 1970-01-01 的天数——保留闸门算的是"隔了几天"，而 `(年,月,日)` 三元组
/// 减 7 会在月底算错，所以这里直接给天数。与 [`civil_from_secs`] 同一套历法、同一个偏移。
pub(crate) fn today_days() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (secs + local_offset_secs()).div_euclid(86_400)
}

/// 本地日期 → 距 1970-01-01 的天数（[`civil_from_days`] 的反向，同一份历法不许有两套）。
///
/// 保留闸门要算"今天 − 文件那天 > 7"，而 `(y, m, d)` 三元组减不了 7 天——月底会算错。
pub(crate) fn days_from_civil(y: u32, m: u32, d: u32) -> i64 {
    let (y, m, d) = (y as i64, m as i64, d as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 崩溃弹窗在整个进程里只领一次配额（`show_box`）。panic 风暴时不刷一屏框。
static BOXED: AtomicBool = AtomicBool::new(false);

/// UI 线程 id：**只有它的 panic 会让进程走到终点**（unwind 出 `extern "system"` ⇒ abort），
/// 所以只有它需要"等用户把框点掉"；别的线程崩了进程还活着，框自己立在旁边即可。
static UI_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

/// 安装崩溃钩子。**只在 `run` 入口调用一次**，窗口装配不必关心。
///
/// ★ 必须在 UI 线程上调用：这里记下的线程 id 决定弹窗要不要阻塞等待（见模块头）。
pub fn install() {
    let _ = UI_THREAD.set(std::thread::current().id());
    panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let payload = panic_message(info.payload());
        // ★ 受控边界（解码器 FFI 一律走 `seh::invoke`）里的 panic 已经有人处理：
        //   宿主按一次失败给解码器记账（三振隔离），用户看到的是"这张图打不开"，
        //   不是"程序崩了"。这种场合既不该弹窗，也不该为每次解码失败写一份 MB 级转储。
        let handled = crate::platform::seh::in_boundary();
        let tail = if handled {
            "    （受控边界内：已由调用方接住，不弹窗、不写转储）\n"
        } else {
            ""
        };
        let text = format!(
            "[{}] thread `{name}` panicked at {loc}\n    {payload}\n{tail}",
            now(),
        );

        let body = text;
        if handled {
            let dest = write_log(&body);
            let location = dest
                .as_ref()
                .map(|p| format!("日志 {}", p.display()))
                .unwrap_or_else(|| "（日志写不进便携目录）".to_string());
            log::error!("受控边界内的 panic（{}）：{}", location, body.trim_end());
            return;
        }

        // panic 通道也写一份转储：此刻进程还完整，是写 dump 的最好时机（SEH 那条通道由
        // [`crate::platform::crash_evidence`] 自己在 handler 里写）。两条通道都留转储，
        // 用户报"闪退"时我们才有同一种东西可查。
        let dump = crate::platform::crash_evidence::write_dump_for_panic();
        let body = compose_body(body, dump.as_ref());
        // ★ 取证先落盘，再碰 logger：env_logger 的 pipe sink 持有 std Mutex，若 panic
        //   恰好发生在锁内，`log::error!` 可能先死在这里，dump 与 crash.log 就永远没了。
        let dest = write_log(&body);
        log::error!("panic: {}", body.trim_end());
        show_box(&body, dest.as_deref());
    }));
}

/// 领一张"弹窗券"：只有第一次调用者能弹（见 [`BOXED`]）。
fn claim_dialog() -> bool {
    !BOXED.swap(true, Ordering::SeqCst)
}

/// 把 panic 的头部正文补全成"一份能独立阅读的证据"：转储在哪、调用链、怎么解。
///
/// 抽成函数的唯一理由是**可测**：钩子里那一段真跑起来会弹框（`MessageBoxW` 阻塞），
/// 自动化验证进不去；而正文的组织方式恰恰是最容易改坏、又最没人复查的地方——
/// "忘了写栈帧"这种退化不会报错，只会让下一次崩溃少一半线索。
fn compose_body(head: String, dump: Option<&std::path::PathBuf>) -> String {
    let mut body = head;
    // 这两行（exe 指纹 + 可照抄的 pdb 文件名）是 `dump_sym.py` 找符号的**全部**依据。
    // panic 通道原先没有它们，于是报告里那句"怎么查"其实敲不通——脚本第一句就报
    // "没有崩溃报告块"。两道通道现在共用同一份拼法。
    body.push_str(&crate::platform::crash_evidence::fingerprint_lines());
    match dump {
        Some(p) => body.push_str(&format!("    转储：{}\n", p.display())),
        None => body.push_str("    （本次未能写出转储）\n"),
    }
    // 调用链：panic 自带"哪一行"，但"谁把它调进来的"只有抓栈能给——而那往往才是
    // 要回答的问题（同一行 unwrap 在不同路径上含义完全不同）。
    body.push_str("--- 栈帧（`模块+偏移`，配同指纹 PDB 可解到 文件:行）---\n         ");
    body.push_str(&crate::platform::crash_evidence::frames_block(0));
    body.push('\n');
    body.push_str(crate::platform::crash_evidence::HOW_TO_ANALYZE);
    body
}

/// 这次 panic 之后进程是否注定要终点（UI 线程上的 panic 无法从 `extern "system"` 展开出去）。
fn will_terminate() -> bool {
    UI_THREAD
        .get()
        .is_some_and(|ui| *ui == std::thread::current().id())
}

/// 取出 panic 的载荷文本（`&str` 与 `String` 两种都覆盖；其它类型给个占位）。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// 追加一条日志。返回实际写入的文件路径（失败则 `None`）。
fn write_log(text: &str) -> Option<PathBuf> {
    // exe 目录必然存在，不 `create_dir_all`：钩子里的每一步都可能在 panic 之后再次失败，
    // 路径越短越好。
    let path = log_path()?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let _ = f.write_all(text.as_bytes());
    let _ = f.write_all(b"\n");
    Some(path)
}

/// 弹错误框。**这里的每一步都必须容错**——钩子里再 panic 一次就是递归，什么都留不下。
///
/// 实现上有两条硬要求（理由见模块头）：① 框必须立在**专门一条线程**上，让 UI 线程的
/// 消息泵不被重入；② UI 线程的 panic 必须等用户点掉，否则钩子一返回进程就 abort，
/// 那一枚框等于没弹。
fn show_box(text: &str, log_path: Option<&std::path::Path>) {
    if !claim_dialog() {
        return; // 已经弹过一次：panic 风暴不刷一屏框。
    }
    let body = match log_path {
        Some(p) => format!("{text}\n详细日志：{}\n", p.display()),
        None => text.to_string(),
    };
    let title: Vec<u16> = "RuPix 遇到内部错误"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // `move` 进新线程：Vec<u16> 必须在框关闭前活着，而要不要等由下面那句决定。
    let spawned = std::thread::Builder::new()
        .name("panic-dialog".to_string())
        .spawn(move || {
            let wide: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(wide.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
                )
            };
        });
    match spawned {
        // 只有 UI 线程那条支线需要等：别的线程崩了进程还活着，框自己立在那儿让用户
        // 慢慢看。（不等时 `JoinHandle` 丢弃即分离。）
        Ok(handle) if will_terminate() => {
            let _ = handle.join();
        }
        Ok(_) => {}
        Err(_) => {} // 起不起得来证据都已在盘上，钩子里不许再为这事做什么
    }
}

/// 时间戳。不引 chrono（`config` 里同样的取舍），自己换算到秒。
pub(crate) fn now() -> String {
    // Unix 秒 → 日历时间的换算算法见 `ui::settings` 的属性页，这里抄那份精益版。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    civil_from_secs(secs)
}

/// 本地 `HH:MM:SS.mmm`——**日志行首的时刻**（唯一读者是 `platform::logging` 的格式闭包）。
///
/// 不带日期：日志文件按天命名（见 `core::paths::run_log_path`），一枚文件里只需要排序。
/// 毫秒取自 `SystemTime` 的亚秒部分，而时区偏移是**整分钟**（[`local_offset_secs`] 就近
/// 取整过），所以"加偏移"和"取小数"两条腿互不污染。
/// 时钟早于 epoch（`duration_since` 报错）时给 epoch 那一刻的本地读数：宁可为怪，不可没有。
pub fn clock_text() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64 + local_offset_secs();
    let rem = secs.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        now.subsec_millis()
    )
}

/// 从 Unix 秒数算**本地**日历时间。
///
/// 同一个秒数不止进日志（日志只要能排序），它还写进会话戳、再原样显示成
/// 「上次会话：pid=… 启动于 …」。那种场合必须用本地口径：本机取证到的一次误报里，戳是
/// **八分钟前**另一枚实例落下的，按 UTC 却报成"2026-10-18 19:07:43"（本地 03:07:43），
/// 用户据此判定"这错误很久以前就有了"。任务管理器 / 资源管理器的时间全是本地——
/// 让人一眼对得上才是这句话的用途。
pub(crate) fn civil_from_secs(secs: u64) -> String {
    let total = secs as i64 + local_offset_secs();
    // div/rem 取欧几里得版：加上偏移后 total 可能跨到前一天（负偏移）甚至跨负，
    // 普通整除会把 rem 变成负数，格式化出 "-1:-1" 这种时间。
    let days = total.div_euclid(86400);
    let rem = total.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days + 719468);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y,
        m,
        d,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// 本地时区相对 UTC 的偏移（秒，整分钟），进程内首用即实测并缓存。
///
/// 实测 `GetLocalTime − GetSystemTime` 的差，而不是读时区表再自己判夏令时：系统已经把
/// DST 算进那两只表里，差值天然带上。取不到（转换失败）回 0，等价于退回 UTC 口径——
/// 时间线索宁可差几小时，也不能整个没有。
fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    use windows::Win32::System::Time::SystemTimeToFileTime;

    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let ticks = |st: SYSTEMTIME| -> Option<i64> {
            let mut ft = FILETIME::default();
            unsafe { SystemTimeToFileTime(&st, &mut ft) }
                .ok()
                .map(|_| ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64)
        };
        let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
        match (ticks(local), ticks(utc)) {
            (Some(local), Some(utc)) => {
                // 秒 → 就近取整到分钟：两只表相隔几毫秒，直接除会把 −18000 截成 −17999。
                let secs = (local - utc) / 10_000_000;
                let rounded = (secs + if secs >= 0 { 30 } else { -30 }) / 60;
                rounded * 60
            }
            _ => 0,
        }
    })
}

/// 与 [`civil_from_secs`] **同一套历法、同一个偏移**的紧凑写法 `YYYYMMDDHHMMSS`（本地）。
///
/// 唯一用途：给人的那份线索（崩溃报告 / 会话前言）要能跟机器给的那份文件名
/// （`.temp\symbols\rupix-<字节数>-<这串数字>.pdb`，见 `build/deploy.py`）逐字对上。
/// 以前两边一个是本地 `YYYY-MM-DD HH:MM:SS`、一个是 UTC 紧凑串，读者要当场心算时区，
/// 对不上就以为"符号配错了版本"——部署脚本本轮已改按**本地**落文件名，这里报告侧
/// 补一枚同形状的串，配对变成一次复制。
pub(crate) fn civil_compact(secs: u64) -> String {
    let total = secs as i64 + local_offset_secs();
    let days = total.div_euclid(86400);
    let rem = total.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days + 719468);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Howard Hinnant 的 epoch 换算（`ui::settings` 里同一份算法的精简版）。
/// `pub(crate)` 是为了让保留清理能拿"天数 ↔ 日期"互转后写夹具（见 `crash_evidence` 的
/// `purge_…` 用例）——同一套历法只许有这一份，不许测试再抄第二份。
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ m129:panic 证据必须自成一份——"转储在哪"、"调用链"、"怎么解"三样缺一样，
    /// 拿到文件的人都得回头问一次。这条不测弹框（会阻塞），只测正文的组织。
    ///
    /// 存在的理由：这种退化**不会报错**。少了栈帧那一节，程序照跑、测试照绿，
    /// 只是下一次真崩溃时线索少一半——正是最该用断言钉住的类型。
    #[test]
    fn panic_body_carries_dump_frames_and_how_to_read_them() {
        // 有转储
        let body = compose_body(
            "[t] thread `x` panicked at a.rs:1:1\n    boom\n".to_string(),
            Some(&std::path::PathBuf::from(r"E:\tmp\temp\crash.dmp")),
        );
        assert!(
            body.contains("转储：E:\\tmp\\temp\\crash.dmp"),
            "要写清转储在哪"
        );
        // 没有这两行，下面那句"怎么查"就是空头承诺：脚本全靠它找同次构建的符号
        assert!(
            body.contains("PDB 配对"),
            "要带能照抄的 pdb 文件名（解析器靠它配对）"
        );
        assert!(body.contains("字节，构建时间"), "要带 exe 指纹");
        assert!(body.contains("--- 栈帧"), "要带调用链那一节");
        assert!(body.contains(".exe+0x"), "帧必须写成 模块+偏移（离线可解）");
        assert!(body.contains("dump_sym.py"), "要自带解析入口");
        // 没写出转储：诚实说，不装作有
        let bare = compose_body("x".to_string(), None);
        assert!(bare.contains("未能写出转储"));
        assert!(!bare.contains("转储："), "没转储就不许出现转储路径");
        assert!(bare.contains("--- 栈帧"), "没转储也必须有调用链");
    }

    /// 实测到的本地偏移：整分钟、且不可能超过 ±14 小时。
    /// 这条是给换算兜底的——100 ns 单位搞错就会偏出十万八千里，而提示里的时间一旦
    /// 对不上任务管理器，用户读到的就是"很久以前的错误"（正是 2026-10-19 那单）。
    #[test]
    fn local_offset_is_whole_minutes_in_range() {
        let off = local_offset_secs();
        assert_eq!(off % 60, 0, "偏移必须取整到分钟，实得 {off} 秒");
        assert!(
            off.abs() <= 14 * 3600,
            "偏移不该超出 ±14 小时，实得 {off} 秒"
        );
    }

    /// 保留闸门算的是"隔了几天"，靠的是 `days_from_civil` ↔ `civil_from_days` **互为逆**，
    /// 而"相邻两天差正好 1"必须在月底、年底、闰日次日都成立——那正是"拿 y/m/d 直接减 7"
    /// 会算错的地方。日期一律写成字面量，不靠注释换算（换算本身就是第二个正本）。
    #[test]
    fn days_and_civil_are_inverse_and_consecutive() {
        let dates = [
            (1970u32, 1u32, 1u32),
            (1969, 12, 31),
            (2024, 2, 28),
            (2024, 2, 29),
            (2024, 3, 1),
            (2026, 10, 19),
            (2026, 10, 31),
            (2026, 11, 1),
            (2024, 12, 31),
            (2025, 1, 1),
            (2100, 2, 28),
        ];
        for (y, m, d) in dates {
            let days = days_from_civil(y, m, d);
            let (yy, mm, dd) = civil_from_days(days + 719_468);
            assert_eq!((yy, mm, dd), (y as i64, m, d), "{y}-{m}-{d} 往返变形");
        }
        // 相邻两天差正好 1：月底、年底、闰日次日各一对。
        for [a, b] in [
            [(2024, 2, 28), (2024, 2, 29)],
            [(2024, 2, 29), (2024, 3, 1)],
            [(2026, 10, 30), (2026, 10, 31)],
            [(2026, 10, 31), (2026, 11, 1)],
            [(2024, 12, 31), (2025, 1, 1)],
        ] {
            let da = days_from_civil(a.0, a.1, a.2);
            let db = days_from_civil(b.0, b.1, b.2);
            assert_eq!(
                db - da,
                1,
                "{a:?} → {b:?} 必须只差一天，实差 {db} − {da} = {}",
                db - da
            );
        }
        // 今天：`today()` 的字符串与 `today_days()` 的天数必须是同一天（两处共用一份偏移）。
        let t = today();
        let y = t.get(0..4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let m = t.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(1);
        let d = t.get(8..10).and_then(|s| s.parse().ok()).unwrap_or(1);
        assert_eq!(
            today_days(),
            days_from_civil(y, m, d),
            "`today()` 与 `today_days()` 用的是两套历法"
        );
    }

    /// 两个写法必须是**同一个时刻**：给人的那一串去掉分隔符，就是符号文件名中间那段。
    /// 这不是格式美观问题——任一边单独改口径（例如把其中一边改回 UTC）配对立刻失效。
    #[test]
    fn civil_compact_is_the_same_instant_without_separators() {
        for secs in [0u64, 1, 1_700_000_000, 4_000_000_000, u64::from(u32::MAX)] {
            let readable = civil_from_secs(secs);
            let compact = civil_compact(secs);
            assert_eq!(compact.len(), 14, "紧凑串必须是 14 位：{compact}");
            let stripped: String = readable.chars().filter(|c| c.is_ascii_digit()).collect();
            assert_eq!(stripped, compact, "{secs} 秒两个写法必须指同一时刻");
        }
    }

    /// m128：弹窗配额这层机制本身要能被验（panic 风暴时不该刷一屏框）。
    /// 真弹窗进不了单元测（`MessageBoxW` 会阻塞），这里钉的是它上面那一层闸门。
    #[test]
    fn dialog_quota_is_consumed_only_once() {
        assert!(claim_dialog(), "第一次要领到配额");
        assert!(!claim_dialog(), "第二次必须领不到：panic 风暴不该刷一屏框");
    }

    /// 负偏移时 epoch 那秒会落到**前一天**（如 UTC−5 → 1969-12-31 19:00:00）。
    /// 这里守的是"绝不格式化出负时分"：普通整除在 total<0 时把 rem 也变成负数，
    /// 输出就成了 `1969-12-31 -5:-1:-1`。换成 div_euclid/rem_euclid 之后形状恒合法。
    #[test]
    fn wall_clock_shape_survives_negative_total() {
        for secs in [0u64, 1, 4_000_000_000, u64::from(u32::MAX)] {
            let s = civil_from_secs(secs);
            let mut parts = s.split([' ', ':']);
            let (date, hh, mm, ss) = (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
            );
            assert!(
                parts.next().is_none()
                    && date.len() == 10
                    && date.bytes().all(|b| b.is_ascii_digit() || b == b'-')
                    && [hh, mm, ss]
                        .iter()
                        .all(|t| t.len() == 2 && t.bytes().all(|b| b.is_ascii_digit())),
                "时间戳形状不合法：{s}"
            );
        }
    }
}
