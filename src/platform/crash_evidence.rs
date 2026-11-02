//! 崩溃取证：让"窗口瞬间没了"留下一份**事后能查的东西**。
//!
//! # 为什么光有 panic 日志不够
//!
//! [`crate::platform::panic_guard`] 只覆盖 Rust 的 `panic!` 这一条通道。真正最难查的
//! 崩溃走的是另一条：**未处理的结构化异常（SEH）**——访问冲突 `0xC0000005`、栈溢出
//! `0xC00000FD`、非法指令……它们**不经过** panic 通道，钩子一声不响，进程直接没了。
//! 反过来也一样：本应用刻意不写 `panic = "abort"`（后台线程的 panic 要能被
//! `catch_unwind` 收成"这张图解码失败"），而 abort 走的 `__fastfail` 又**绕过 SEH**。
//! 两条通道不相交，所以都得装：
//!
//! | 通道 | 覆盖 | 手段 |
//! |---|---|---|
//! | Rust `panic!` | 越界、`unwrap`、`RefCell` 重入、断言 | panic 钩子（写 `temp\crash-<今天>.log` + 弹框），**现在也写转储** |
//! | 未处理 SEH | 访问冲突、栈溢出、除零、非法指令 | 本模块：`temp\crash-<今天>.log` 追加报告 + `crash-<今天>-<时分秒>.dmp` |
//!
//! # 一次崩溃留下什么
//!
//! 1. `temp\crash-<今天>.log` 里一段**报告**：异常码（带人话）、异常地址、出错线程（名字，不是裸 ID）、
//!    exe 路径 + 大小 + 构建时间、**exe 基址与 RVA**、栈回溯、运行日志尾部、转储路径。
//! 2. `temp\` 里一枚 `crash-<今天>-<时分秒>.dmp`（`MiniDumpWriteDump`，`MiniDumpNormal`：
//!    带线程栈与异常上下文，几百 KB，不带整堆）。两道闸门都在：最近 [`KEEP_DUMPS`] 份，
//!    外加**文件名日期**过 [`KEEP_DAYS`](crate::core::paths::KEEP_DAYS) 天的一次清理（[`prepare_diagnostics`]）。
//!
//! 报告为什么必须带**基址**：异常地址是含 ASLR 的绝对地址，没有基址就换不出 RVA，
//! 拿着同版本符号也对不上行（只写"崩在 `0x7ff6…`"等于没写）。
//! 报告为什么带**线程名**：本仓所有后台线程都用 `Builder::new().name(..)` 命名，
//! filter 就跑在出错线程上，`thread::current().name()` 免费给出身份——否则只能在
//! `sv-decode` / `sv-level` / 预览线程 / 主线程之间猜。
//!
//! # 事后怎么把它变成"第几行"
//!
//! 发布版 `strip = true`，exe 里没有符号，回溯里自己的帧是 `<unknown>`；**同名 PDB**
//! 才解得开。`build/deploy.py` 每次发布把 `.temp/target/release/rupix.pdb` 按
//! 「exe 大小 + 构建时间」归档到 `.temp/symbols/`，报告里那两行指纹就是用来配对的。
//! md5 不能当"同一份产物"的判据（PE 时间戳与链接器 GUID 每次构建都变），
//! 所以配对用**大小 + mtime**。
//!
//! # 已知限度（不许当成已解决）
//!
//! - **栈溢出**（`0xC00000FD`）时 filter 仍在被打爆的那条栈上跑，报告可能写不全、
//!   转储可能写不出。要彻底覆盖得换备用栈再进 handler，那是另一件事，这里不做。
//! - 被外部 `TerminateProcess` 干掉的场景两条通道都不跑——那条留给会话戳
//!   （[`crate::platform::session`]）去侦测"上次没正常收尾"。
//! - 崩溃路径上允许分配堆（反正要退出），但不允许 `unwrap`/再触发异常：下面每一步
//!   都只做"尽力而为"，失败就降级成报告里的一行说明。

use std::io::Write as _;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

// 诊断物的命名正本在 `core::paths`（`diag_ymd` 按同一套前缀认文件），这里只引用不另写一份。
use crate::core::paths::{DUMP_PREFIX, DUMP_SUFFIX, RUN_LOG_PREFIX};

use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWriteDump, RtlCaptureStackBackTrace, RtlPcToFileHeader, SetUnhandledExceptionFilter,
    EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
// filter 的返回类型在文档里叫 `EXCEPTION_DISPOSITION`，但 windows 0.62 的回调别名用的是
// 裸 `i32`（值 1 = ExecuteHandler）。少开 `Win32_Storage_FileSystem` 时连
// `MiniDumpWriteDump` 都不导出——那三个 feature 的与条件写在 Cargo.toml 的注释里。

/// 转储最多留几份（超出按文件名顺序删最旧的——文件名就是时间戳，天然可排序）。
pub const KEEP_DUMPS: usize = 5;
/// 运行日志的落盘写入器（给 `env_logger` 当 sink 用）。
///
/// 为什么日志要在盘上：发布版是 `windows_subsystem = "windows"`，**stderr 没人看得见**，
/// 而崩溃报告要回答"崩之前在做什么"——那一句只有历史日志能给。GUI 程序把日志只写向
/// 控制台等于没写。
///
/// **写满即停，不截断重写**（闸门是 [`RUN_LOG_MAX_BYTES`]）：到顶就落一条标记然后收手。
/// 为什么不停下来而是丢掉后续：截断重写吃掉的是**当天开头**那几行——会话头（版本 /
/// 构建时间 / exe 路径）与解码器清单，那正是"这份日志是哪一版落下来的、配哪一枚 PDB"的
/// 唯一依据；丢了它，后面写得再多也没法判读。755 字节的日常会话离上限差着三个数量级，
/// 真能写满的只有反复调试的一整天，那种日子"保住开头"比"保住结尾"值钱。
///
/// `len` 从**打开那一刻的文件长度**起算，所以它是"这枚文件的量"的良好近似；只有多个
/// 进程同时往同一枚日志追加时才会低估（各自只数自己写的字节）。为此每行多问一次
/// `metadata()` 不值得——宁可多写几 KB。
///
/// 写不进去（只读介质）就静默丢掉，报告里那一段会诚实写"还没有内容"。
pub struct RunLogSink {
    file: Option<std::fs::File>,
    len: u64,
    /// 本枚日志的容量闸门（`new()` 取 [`RUN_LOG_MAX_BYTES`]；测试注入小值才测得到边界）。
    cap: u64,
    /// 已经写满并落过标记：置真之后不再碰盘（标记因此**只有一条**）。
    stopped: bool,
}

impl RunLogSink {
    /// 用 `temp\rupix-<今天>.log` 构造。取不到目录时写入器直接丢日志（不影响 stderr 那一路）。
    ///
    /// ★ 名字在**构造那一刻**定下，本进程之后一直写这一枚：跨午夜的长会话会继续写
    ///   昨天那枚，不换名（换名要把长度闸门重算一遍，为几行日志不值当）。
    ///   读侧（[`run_log_tail`]）因此按"最新的一枚"找，而不是按"今天"找。
    pub fn new() -> Option<Self> {
        let _ = ready_diagnostics_dir()?;
        let path = crate::core::paths::run_log_path(&crate::platform::panic_guard::today())?;
        Some(Self::on(path, crate::core::paths::RUN_LOG_MAX_BYTES))
    }

    /// 打开一枚具体日志并装进去（`new()` 与用例共用；`cap` 只由调用方给）。
    fn on(path: PathBuf, cap: u64) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let len = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(0);
        Self {
            file,
            len,
            cap,
            stopped: false,
        }
    }
}

impl std::io::Write for RunLogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // 同时镜像到 stderr：调试版与探针/example 还是照旧能在控制台里看见日志，
        // 发布版没有控制台，那一路写入被系统直接丢掉，无代价。
        let _ = std::io::Write::write_all(&mut std::io::stderr(), buf);
        let Some(f) = self.file.as_mut() else {
            return Ok(buf.len());
        };
        if self.stopped {
            return Ok(buf.len());
        }
        if self.len.saturating_add(buf.len() as u64) > self.cap {
            let mark = format!(
                "=== 运行日志已写到 {} 字节上限，本次会话此后不再落盘 ===\n",
                self.cap
            );
            let _ = f.write_all(mark.as_bytes());
            let _ = f.flush();
            self.stopped = true;
            return Ok(buf.len());
        }
        let n = f.write(buf)?;
        self.len += n as u64;
        // 每条都 flush：崩溃来到的时候，缓冲区里那几行正是我们最想要的那几行。
        let _ = f.flush();
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// 报告里附多少字节的运行日志尾部（"崩之前在做什么"就靠它）。
const TAIL_BYTES: u64 = 8 * 1024;

/// 安装 SEH 兜底。**必须在 `run` 最开头、任何窗口装配之前调**，启动阶段的异常也要留证。
pub fn install() {
    // SAFETY: 进程级只注册一个 filter；handler 自己不再触发异常、必定返回。
    unsafe {
        let _ = SetUnhandledExceptionFilter(Some(seh_filter));
    }
}

/// 未处理异常的最后一次机会。全程不 `unwrap`、不 `panic`、失败只降级为报告里的一行。
unsafe extern "system" fn seh_filter(info: *const EXCEPTION_POINTERS) -> i32 {
    let (code, addr) = if info.is_null() {
        (0u32, 0usize)
    } else {
        let rec = unsafe { (*info).ExceptionRecord };
        if rec.is_null() {
            (0, 0)
        } else {
            // SAFETY: 上一步已排除空指针；异常记录由系统保证在本次调用期内有效。
            let r = unsafe { &*rec };
            (r.ExceptionCode.0 as u32, r.ExceptionAddress as usize)
        }
    };
    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    let dump = write_dump(info, tid);
    let snap = snapshot("未处理异常（SEH）", code, addr, tid, &dump, module_base());
    append_report(&snap.report);
    if let Some(dir) = dump_dir() {
        prune_dumps(&dir, KEEP_DUMPS);
    }
    // EXCEPTION_EXECUTE_HANDLER = 1：交回系统按默认流程收尾（进程随之退出）。
    1
}

/// 本次构建的四件套：字节数、构建时间（本地可读串）、同一时刻的紧凑串、自身路径。
///
/// 「字节数 + 构建时间」就是 `.temp/symbols/` 里 PDB 的配对指纹（md5 不行：源码一字不改
/// 重编它也会变）。诊断头与崩溃报告共用这一份，避免两处各算各的、报出两个数。
///
/// 紧凑串（`YYYYMMDDHHMMSS`，**本地**）是专门为了配对：它与 `deploy.py` 归档 pdb 时
/// 写进文件名的那一段逐字相同（那边的口股本轮已从 UTC 改成本地），读者不必再手算时区。
/// 两个写法由 [`crate::platform::panic_guard::civil_compact`] 与
/// [`crate::platform::panic_guard::civil_from_secs`] 共用一份历法给出。
pub fn exe_fingerprint() -> (u64, String, String, String) {
    let exe = std::env::current_exe().ok();
    let path = exe
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "（取不到自身路径）".to_string());
    let (len, mtime, compact) = exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|md| {
            let t = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    (
                        crate::platform::panic_guard::civil_from_secs(d.as_secs()),
                        crate::platform::panic_guard::civil_compact(d.as_secs()),
                    )
                })
                .unwrap_or_else(|| ("（未知）".to_string(), "（未知）".to_string()));
            (md.len(), t.0, t.1)
        })
        .unwrap_or((0, "（未知）".to_string(), "（未知）".to_string()));
    (len, mtime, compact, path)
}

/// 直接往 `temp\rupix-<今天>.log` 追加一段文本（会话前言用，见 [`crate::platform::env_report`]）。
///
/// 绕开 `log` 过滤器有两个理由：① 它是**会话前言**而不是一条日志事件，落在"哪一级别被放行"
/// 的裁决里没有道理；② 它必须**无条件**在场——用户把 `RUST_LOG` 设成 `warn` 或 `error` 时，
/// 发回来的日志仍然要能答出"哪一版、什么环境"，否则崩溃报告里那一节就空了（默认档 2026-10-20
/// 起是 info，但"不靠级别档"这条要求不因此放松）。
/// 写不进去（只读介质、目录取不到）就静默丢掉——诊断头不许成为新的故障源。
/// 与 [`RunLogSink`] 各持一个 append 句柄：Windows 的 `FILE_APPEND_DATA` 每次写都定位到
/// 当前 EOF，两条通道不会互相覆盖。
pub fn append_run_log(text: &str) {
    if ready_diagnostics_dir().is_none() {
        return;
    }
    let Some(path) = crate::core::paths::run_log_path(&crate::platform::panic_guard::today())
    else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(text.as_bytes());
    }
}

/// 一次崩溃的现场快照，以及由它拼出的报告文本。
pub(crate) struct Snapshot {
    /// 报告正文（已含分隔线，直接追加进当天的 `crash-<日期>.log`）。
    pub report: String,
}

/// 组一份现场快照。`dump` 为 `None` 时报告里诚实写"转储没写成"，不假装成功。
/// 本进程 exe 的加载基址（ASLR 后每次不同）。拿不到时返回 0，报告里诚实写"无法换算 RVA"。
pub(crate) fn module_base() -> usize {
    // SAFETY: 只是问系统"我的主模块句柄是多少"，不持有任何跨调用有效的不变量。
    unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
        .ok()
        .map(|h| h.0 as usize)
        .unwrap_or(0)
}

/// 栈帧上限。32 与 pecia 同档：够看清"用户操作 → UI → 解码 → 解码器"这条链，
/// 又不至于在栈快爆（`0xC00000FD`）时再吃掉一大块。
const MAX_FRAMES: usize = 32;

/// 一个代码地址 → `模块名+0x偏移`（不在任何已加载模块里时写裸地址并注明）。
///
/// **为什么不用 `std::backtrace::Backtrace`**：它的 `Display` 在**没有本地 PDB** 时
/// 只剩 `<unknown>`——名字没了、地址也没留下，事后一个字节都解不出来。而"用户机器上
/// 没有 PDB"正是本模块存在的理由。pecia 走的是另一条等价路（`/DYNAMICBASE:NO` 固定
/// 基址 + 每版归档 `.map`，地址与 map 一一对应）；我们保留 ASLR，那就必须把**每帧的
/// 模块偏移**写下来——偏移与基址无关，换台机器也能落到同一行。
fn frame_text(pc: usize) -> String {
    let mut base: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: 两个调用都只是"问系统这个地址属于哪个模块"，纯只读；文件名缓冲区是本栈
    // 上的定长数组，`GetModuleFileNameW` 按切片长度截断、不写越界。
    let (name, start) = unsafe {
        let hit = RtlPcToFileHeader(pc as *const core::ffi::c_void, &mut base);
        let start = if hit.is_null() {
            base as usize
        } else {
            hit as usize
        };
        if start == 0 || start > pc {
            return format!("0x{pc:016X}（不在任何模块内）");
        }
        let mut file = [0u16; 260];
        let n = GetModuleFileNameW(Some(HMODULE(start as *mut _)), &mut file) as usize;
        let path = String::from_utf16_lossy(&file[..n.min(file.len())]);
        let name = Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("?{start:X}"));
        (name, start)
    };
    format!("{name}+0x{:X}", pc - start)
}

/// 抓当前线程的栈帧，**异常地址单独排成 `#00`**。
///
/// 为什么异常地址要单列：SEH filter 跑在出错线程的栈上，抓到的前几帧是取证代码自己
/// （`capture_frames` → `snapshot` → `seh_filter`），真正的出错点排在
/// `KiUserExceptionDispatcher` 之后；而 `ExceptionAddress` 是**系统给的、不含糊**的那一个。
/// 两者都留着，谁也别替谁作主。
fn capture_frames(addr: usize) -> Vec<String> {
    let mut raw = [std::ptr::null_mut::<core::ffi::c_void>(); MAX_FRAMES];
    // SAFETY: 缓冲区定长且在栈上，`RtlCaptureStackBackTrace` 只会往里写至多 len 个地址；
    // 第三参（哈希）是可选输出，传空即可。
    let n = unsafe { RtlCaptureStackBackTrace(1, &mut raw, None) } as usize;
    let mut out = Vec::with_capacity(n + 1);
    if addr != 0 {
        out.push(format!("#00 {}  ← 异常地址", frame_text(addr)));
    }
    for (i, pc) in raw.iter().take(n).enumerate() {
        out.push(format!("#{:02} {}", i + 1, frame_text(*pc as usize)));
    }
    out
}

/// 帧列表拼成一段（`\n` 分隔 + 缩进由调用方给）。
///
/// panic 通道也用它：panic 自带 `文件:行:列`（比任何符号化都准），但**调用链**只有抓栈
/// 能给——"谁把它调进来的"往往比"它在哪一行 unwrap"更能说明问题。
pub(crate) fn frames_block(addr: usize) -> String {
    let f = capture_frames(addr);
    if f.is_empty() {
        return "（未取到栈帧：栈可能已被打爆）".to_string();
    }
    f.join("\n         ")
}

/// 报告末尾那段"怎么把它变成源码行"的操作指引：**崩溃报告与 panic 兜底共用一份**，
/// 免得两边各写一套、日后只改一处。
///
/// 写在报告里而不是只写在文档里：拿到这份文件的人（可能是几周后的自己，也可能是
/// 用户转发回来的截图）不需要先找到仓库就能照着做。
pub const HOW_TO_ANALYZE: &str = concat!(
    "怎么查      把本文件连同同目录的 .dmp 一起交给开发机，执行：\n",
    "            python main\\build\\dump_sym.py <本文件>\n",
    "            （脚本按上面的指纹自己在 .temp\\symbols\\ 里认 PDB，逐帧解出函数与 文件:行；\n",
    "            认不出会明说，不会拿错版本的 PDB 硬编一个函数名给你。）\n",
);

/// 「exe 指纹 + PDB 配对」两行，**两道通道共用一份**。
///
/// 为什么必须共用：`dump_sym.py` 靠这两行（① exe 路径/字节数/构建时间，② 可照抄的 pdb 文件名）
/// 才找得到同次构建的符号。panic 通道原先没有这两行——那样"怎么查"那句就是空头承诺：
/// 照着敲，脚本第一句就报"没有崩溃报告块"。
pub(crate) fn fingerprint_lines() -> String {
    let (len, built, stamp, path) = exe_fingerprint();
    // pdb 名取 exe 的**基名**（产品里就是 `rupix`，与 `deploy.py` 归档时的口径同一个来源）；
    // 示例探针会得到 `panic_probe-…`，虽然没人给它归档，但至少不撒谎。
    let stem = Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rupix".to_string());
    format!(
        "exe         {path}（{len} 字节，构建时间 {built}）\n\
         PDB 配对    {stem}-{len}-{stamp}.pdb（开发机 `.temp\\symbols\\` 下按这两个数找；\
         发给开发者时不必带它，报告自带全部线索）\n"
    )
}

pub(crate) fn snapshot(
    kind: &str,
    code: u32,
    addr: usize,
    thread_id: u32,
    dump: &Option<PathBuf>,
    base: usize,
) -> Snapshot {
    // exe 指纹（字节数 + 构建时间）是与 `.temp/symbols/` 里 PDB 配对的键，见 [`exe_fingerprint`]；
    // 那两行的拼法抽到 [`fingerprint_lines`]，panic 通道用的是同一份。
    let fingerprints = fingerprint_lines();
    // 会话环境只读**启动时冻结的那份**：这里再采集一次就要在 SEH filter 里进
    // user32/shcore，崩溃现场可能正是坏在那些地方——二次异常等于连这份报告都写不出来。
    let mut env = match crate::platform::env_report::frozen() {
        Some(s) => s.to_string(),
        None => "（未采集：崩在会话头之前，这一节留空）".to_string(),
    };
    // 下一节的分隔线必须自成一行：`frozen()` 那份以换行收尾，探针那种"没采到"的短句不会
    // （真机看一眼报告就看见 `（未采集…）--- 现场 ---` 粘在一行）。
    if !env.ends_with('\n') {
        env.push('\n');
    }
    // exe 基址：异常地址 − 基址 = RVA，配同一次构建的 PDB 就能落到行。
    let rva = if base != 0 && addr >= base {
        format!("0x{addr:016X}（RVA {:#X}）", addr - base)
    } else {
        format!("0x{addr:016X}（无法换算 RVA）")
    };
    let thread = std::thread::current();
    let dump_note = match dump {
        Some(p) => format!(
            "{}（配同指纹的 pdb 可精确到行，见下面那行「PDB 配对」）",
            p.display()
        ),
        None => "（本次未能写出转储，只有文本报告）".to_string(),
    };
    let tail = run_log_tail();
    let frames = frames_block(addr);
    let report = format!(
        "=== RuPix 崩溃报告：{kind} ===\n\
         时间        {}\n\
         --- 会话环境（启动时采集：版本/系统/DPI/显示器/命令行）---\n{env}\
         --- 现场 ---\n\
         异常码      0x{code:08X}{}\n\
         异常地址    {rva}\n\
         出错线程    {thread_id}（{}）\n\
         {fingerprints}\
         exe 基址    0x{base:016X}\n\
         minidump    {dump_note}\n\
         --- 栈帧（`模块+偏移`，与基址无关：拿同指纹的 PDB 能逐帧解到 文件:行）---\n\
         （`#00` 是系统报的异常地址；`#01` 起的前几帧是取证代码自己，真正的出错点排在\
         `KiUserExceptionDispatcher` 之后。偏移不是地址——同一份 exe 换台机器也不变。）\n\
         {frames}\n\
         --- 运行日志尾部 ---\n{tail}\n\
         {HOW_TO_ANALYZE}",
        crate::platform::panic_guard::now(),
        code_note(code),
        thread.name().unwrap_or("<unnamed>"),
    );
    Snapshot { report }
}

/// 异常码 → 人话。只覆盖常见几个，其余留空（不猜）。
fn code_note(code: u32) -> &'static str {
    match code {
        0xC000_0005 => "（访问冲突：读写无效地址——野指针 / 空指针 / 已释放对象）",
        0xC000_00FD => "（栈溢出：深递归或超大栈数组）",
        0xC000_0094 => "（整数除零）",
        0xC000_0095 => "（整数溢出）",
        0xC000_001D => "（非法指令）",
        0xC000_0135 => "（缺 DLL）",
        0x8000_0003 => "（断点 / 主动触发）",
        _ => "",
    }
}

/// 报告与转储的落点：**路径**而已（`<exe 目录>/temp`），不顺手建目录——
/// `newest_dump()` 这类只读入口也走它，读一个不存在的目录应当只是返回 `None`。
/// 写之前由调用方 [`ensure_diagnostics_dir`]。
pub(crate) fn dump_dir() -> Option<PathBuf> {
    crate::core::paths::diagnostics_dir()
}

/// 建 `temp\`：只读介质上失败是**预期内**的降级（报告里会诚实写"没写成"），所以不报错。
/// 每个要往这儿写的入口都调一次，`create_dir_all` 幂等、便宜。
fn ensure_diagnostics_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
}

/// **写**路径的统一入口：拿到诊断目录并保证它在盘上存在。
/// 取不到 exe 目录（极罕见）返回 `None`，调用方按"本次不落盘"处理。
fn ready_diagnostics_dir() -> Option<PathBuf> {
    let dir = crate::core::paths::diagnostics_dir()?;
    ensure_diagnostics_dir(&dir);
    Some(dir)
}

/// 由 Unix 秒推出**既可读又可排序**的文件名中段：`2026-10-19-112233`。
/// 前 10 位是日期（`core::paths::diag_ymd` 就认这一段），后面接紧凑时分秒。
pub(crate) fn name_stamp(secs: u64) -> String {
    let civil = crate::platform::panic_guard::civil_from_secs(secs);
    let mut halves = civil.split(' ');
    let day = halves.next().unwrap_or("");
    let time: String = halves
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    format!("{day}-{time}")
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 把当前进程写成 minidump。**崩溃路径与 panic 路径共用这一条**（panic 时进程还完整，
/// 正是写转储的好时机）。失败返回 `None`，由调用方在报告里如实写明。
///
/// SAFETY: `info` 要么为空、要么指向系统在本次调用期内保证有效的异常记录；
/// 文件句柄由 `File` 的 RAII 持有，`MiniDumpWriteDump` 是同步调用，返回后即可关。
pub fn write_dump(info: *const EXCEPTION_POINTERS, thread_id: u32) -> Option<PathBuf> {
    let dir = ready_diagnostics_dir()?;
    let base = format!("{DUMP_PREFIX}{}{DUMP_SUFFIX}", name_stamp(unix_secs()));
    // 同一秒内两枚转储（两个线程 panic / 钩子重入）不能互相截断：`create_new` 撞名
    // 就加序号后缀，旧的那枚保住。
    let mut path = dir.join(&base);
    let file = {
        let mut n = 2u32;
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(f) => break f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stem = base.strip_suffix(DUMP_SUFFIX).unwrap_or(&base);
                    path = dir.join(format!("{stem}-{n}{DUMP_SUFFIX}"));
                    n = n.saturating_add(1);
                }
                Err(_) => return None,
            }
        }
    };
    let exc = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: thread_id,
        ExceptionPointers: info as *mut EXCEPTION_POINTERS,
        // 异常上下文就在本进程地址空间里，不需要 ClientPointers 翻译。
        ClientPointers: windows::core::BOOL(0),
    };
    let exc_param = if info.is_null() {
        None
    } else {
        Some(std::ptr::from_ref(&exc))
    };
    let ok = unsafe {
        MiniDumpWriteDump(
            windows::Win32::System::Threading::GetCurrentProcess(),
            std::process::id(),
            HANDLE(file.as_raw_handle()),
            // MiniDumpNormal：线程栈 + 模块表 + 异常上下文，够符号化用，体积小。
            // 整堆（WithHeap）对看图器是几百 MB 量级，便携目录里不该随手就写。
            MINIDUMP_TYPE(0),
            exc_param,
            None,
            None,
        )
    };
    // 句柄必须在 `file` drop 前用完：上面是同步调用，这里 drop 正合适。
    drop(file);
    if ok.is_ok() {
        // 崩溃与 panic 两条路共用这里；panic 路径以前不剪枝，解码器每崩一次就多一枚
        // 转储，直到 7 天启动清理。成功写完后统一按 KEEP_DUMPS 剪一次。
        if let Some(dir) = path.parent() {
            prune_dumps(dir, KEEP_DUMPS);
        }
        Some(path)
    } else {
        // 失败留下的是 0 B / 半截文件，会占一个 KEEP_DUMPS 名额，还可能被
        // `newest_dump()` 当成"最近一次崩溃"交给恢复提示。
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// panic 通道的转储入口（没有异常记录，只带当前线程）。
pub fn write_dump_for_panic() -> Option<PathBuf> {
    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    write_dump(std::ptr::null(), tid)
}

/// 追加报告到 `temp\crash-<今天>.log`（沿用 [`crate::platform::panic_guard`] 那一份，
/// 取证只有一条时间线；同一天崩两次就落在同一枚里，行首的本地时间戳负责区分）。
pub(crate) fn append_report(text: &str) {
    if ready_diagnostics_dir().is_none() {
        return;
    }
    let Some(path) = crate::platform::panic_guard::log_path() else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = f.write_all(text.as_bytes());
        let _ = f.write_all(b"\n\n");
    }
}

/// 只留最近 `keep` 份转储。文件名就是时间戳，按名字排序=按时间排序。
///
/// 与 [`purge_old_diagnostics`] 同一个口径：硬删，不进回收站。
pub(crate) fn prune_dumps(dir: &Path, keep: usize) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // 按 **mtime** 排，不按名字：旧式紧凑名 `crash-20261017131340.dmp` 与新式
    // `crash-2026-10-21-005045.dmp` 逐字节比较时新式恒小，按名字排会把刚写的
    // 那份排到最旧、刚崩完就被删掉（`purge_old_diagnostics` 又不认旧名，旧名
    // 反而永生）。mtime 对两种名字都成立；读不到 mtime 的跳过不删。
    let mut dumps: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(DUMP_PREFIX) && n.ends_with(DUMP_SUFFIX))
        })
        .filter_map(|p| {
            let t = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((t, p))
        })
        .collect();
    if dumps.len() <= keep {
        return Vec::new();
    }
    dumps.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let excess = dumps.len() - keep;
    let doomed: Vec<PathBuf> = dumps.into_iter().take(excess).map(|(_, p)| p).collect();
    for p in &doomed {
        let _ = std::fs::remove_file(p);
    }
    doomed
}

/// 最近一份转储（恢复提示窗用它告诉用户"这次崩在哪能看到细节"）。
pub fn newest_dump() -> Option<PathBuf> {
    let dir = dump_dir()?;
    let rd = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in rd.flatten() {
        let p = e.path();
        let hit = p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(DUMP_PREFIX) && n.ends_with(DUMP_SUFFIX));
        if !hit {
            continue;
        }
        let Some(t) = std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, p));
        }
    }
    best.map(|(_, p)| p)
}

/// 当前在写的那一枚运行日志：取 `temp\` 里**名字最大**（= 日期最新）的 `rupix-<日期>.log`。
///
/// 为什么不是"今天那枚"：[`RunLogSink`] 的名字在进程启动那一刻就定了，跨午夜的长会话
/// 一直写昨天那枚；崩溃发生在 00:05 时按"今天"去找会拿到一枚空文件，
/// "崩之前在做什么"这句就丢了。按天命名之后，"最新"恰好等于"按名字排序的最大值"。
pub(crate) fn current_run_log() -> Option<PathBuf> {
    let dir = crate::core::paths::diagnostics_dir()?;
    // 目录或日志还没有时的兜底名：报告里那句"（… 还没有内容）"要指得出具体文件。
    let fallback = crate::core::paths::run_log_path(&crate::platform::panic_guard::today())?;
    Some(newest_run_log_in(&dir).unwrap_or(fallback))
}

/// [`current_run_log`] 的可测部分：给定目录找最新一枚运行日志（目录不存在返回 `None`）。
pub(crate) fn newest_run_log_in(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut best: Option<PathBuf> = None;
    for e in rd.flatten() {
        let p = e.path();
        let fresh = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.starts_with(RUN_LOG_PREFIX) && crate::core::paths::diag_ymd(n).is_some()
        });
        if fresh && best.as_ref().is_none_or(|b| p > *b) {
            best = Some(p);
        }
    }
    best
}

/// 启动时的一次保留清理：删掉 `temp\` 里**文件名日期**早于「今天 − [`KEEP_DAYS`](crate::core::paths::KEEP_DAYS)」的日志与转储。
///
/// `today_days` 由调用方给：判据不许自己读时钟，否则这条规则要等 8 天之后才测得了
/// （与 `core::config` 防抖同一个定式）。**认不出日期的名字一律不动**——`rupix.run`
/// 是状态不是历史，而用户手放进 `temp\` 的任何东西都不该被看图器删掉。
///
/// # 硬删，不进回收站（与 [`crate::platform::recycle`] 的分工）
///
/// 判据是**删的是谁的资产**，不是删的是什么文件。卸载走回收站，因为 DLL 是用户装进来的
/// 东西；这里的日志与转储是应用自己产的取证中间物，7 天窗口就是它的保质期。
/// 三条实测与机制决定的取舍（2026-10-19，`.temp/m42_recycle_cost.py` 同一份外壳调用配对两序）：
/// 硬删 0.13–0.15 ms/枚，`SHFileOperationW + FOF_ALLOWUNDO` **46–47 ms/枚、首枚含外壳
/// 冷启动 182–185 ms**（差 300 倍以上），而这条路径在窗口出现之前；进回收站并不释放空间，
/// 用户不清空就一直占着盘，等于把 `temp\` 的堆积搬到另一个没人打扫的地方；便携应用
/// 常跑在 U 盘 / 网络共享上，那些介质没有 `$Recycle.Bin`，`FOF_ALLOWUNDO` 直接失败，
/// "可恢复"这个收益恰好在便携场景落空，而硬删照旧工作。
/// 误删风险已由上面那条"认不出日期一个不删"挡掉，回收站兜不了也不需要兜。
pub(crate) fn purge_old_diagnostics(dir: &Path, today_days: i64, keep_days: i64) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut doomed: Vec<PathBuf> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let (y, m, d) = crate::core::paths::diag_ymd(p.file_name()?.to_str()?)?;
            (today_days - crate::platform::panic_guard::days_from_civil(y, m, d) > keep_days)
                .then_some(p)
        })
        .collect();
    doomed.sort();
    for p in &doomed {
        let _ = std::fs::remove_file(p);
    }
    doomed
}

/// 启动序列里的"诊断物准备"一步：建 `temp\` + 清掉过保留期的旧日志与转储。
///
/// **必须在 [`install`] 之后、任何写日志之前**调（会话头、崩溃报告、转储都指向这个目录）。
/// 失败一律静默：只读介质 = 本次不落盘，这一程不许变成新的故障源。
pub fn prepare_diagnostics() {
    let Some(dir) = ready_diagnostics_dir() else {
        return;
    };
    let gone = purge_old_diagnostics(
        &dir,
        crate::platform::panic_guard::today_days(),
        crate::core::paths::KEEP_DAYS,
    );
    for p in &gone {
        log::debug!("诊断物过保留期已删除：{}", p.display());
    }
}

/// 运行日志的尾部（"崩之前在做什么"）。没有日志文件时诚实说一句。
fn run_log_tail() -> String {
    let Some(path) = current_run_log() else {
        return "（取不到 exe 目录，无运行日志）".to_string();
    };
    let Ok(mut f) = std::fs::File::open(&path) else {
        return format!("（{} 还没有内容）", path.display());
    };
    use std::io::{Read, Seek, SeekFrom};
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = size.saturating_sub(TAIL_BYTES);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    // 半行开头（从字节中间切来的）直接丢掉，读起来才不迷惑。
    // `from == 0` 时文件本来就是从头读的，第一行是完整行，不能丢。
    if from == 0 {
        return text;
    }
    match text.find('\n') {
        Some(0) | None => text,
        Some(i) => text[i + 1..].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ m129:帧必须带**偏移**，不是地址——偏移与 ASLR 无关，换台机器照样能对上行。
    /// 这里钉四件事：非空、每行都是 `#NN 模块+0x十六进制`、异常地址排在 `#00`、
    /// 且**绝不 panic**（它跑在崩溃处理路径上，再炸一次连报告都没了）。
    #[test]
    fn frames_carry_module_offsets_and_never_panic() {
        let block = frames_block(0);
        assert!(!block.is_empty(), "栈帧不能是空的");
        for line in block.lines() {
            let line = line.trim();
            assert!(
                line.starts_with('#'),
                "每帧都要带序号（否则读者无法按序引用）：{line}"
            );
            assert!(line.contains("+0x"), "每帧都要写成 模块+偏移：{line}");
        }
        // 单独给一个异常地址时它必须排在最前并被标出来
        let with_addr = frames_block(0x0000_7FF6_0000_1234);
        assert!(
            with_addr.trim_start().starts_with("#00") && with_addr.contains("异常地址"),
            "异常地址要单独排成 #00 并标注，实得：{with_addr}"
        );
        // 野地址：不能崩，只能诚实地说"不在任何模块内"
        assert!(frame_text(0xDEAD_BEEF).contains("不在任何模块内"));
        assert!(frame_text(0).contains("不在任何模块内"));
    }

    /// 文件名时间戳必须**定长、可排序，且前 10 位就是一个能解析的日期**
    /// （保留清理 `purge_old_diagnostics` 读的就是那一段，它不读 mtime）。
    #[test]
    fn name_stamp_is_fixed_width_sortable_and_dated() {
        let a = name_stamp(0);
        let b = name_stamp(1_700_000_000);
        assert_eq!(a.len(), 17, "应是 `YYYY-MM-DD-HHMMSS`，实得 {a}");
        assert_eq!(b.len(), 17, "同上，实得 {b}");
        assert!(a < b, "越早的时间戳必须排得越前：{a} vs {b}");
        let (y, _m, _d) =
            crate::core::paths::diag_ymd(&format!("crash-{a}.dmp")).expect("日期段要认得出");
        // epoch 那天换个时区会差一天，所以只钉"年份在 1969/1970 之间"这条稳的。
        assert!(y == 1969 || y == 1970, "epoch 的日期段算错了，实得 {y}");
    }

    /// 保留闸门只删**认得出的日期**且已过 7 天的：边界（第 7 天留 / 第 8 天删）、
    /// 状态文件 `rupix.run`、别人的文件、旧式紧凑名，四种都不许碰。
    #[test]
    fn purge_deletes_only_dated_files_past_the_boundary() {
        let dir = crate::core::paths::test_scratch_root().join("rupix_purge_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建 temp 目录");
        let today = crate::platform::panic_guard::today_days();
        let day = |back: i64| {
            let (y, m, d) = crate::platform::panic_guard::civil_from_days(today - back + 719_468);
            format!("{y:04}-{m:02}-{d:02}")
        };
        let doomed_expect = [
            format!("rupix-{}.log", day(8)),
            format!("crash-{}.dmp", day(30)),
        ];
        let keep = [
            format!("rupix-{}.log", day(0)),
            format!("rupix-{}.log", day(7)), // 边界：第 7 天整还留
            "rupix.run".to_string(),
            "notes.txt".to_string(),
            "crash-20260101000000.dmp".to_string(), // 旧式紧凑名：认不出 = 不删
        ];
        for n in doomed_expect.iter().chain(keep.iter()) {
            std::fs::write(dir.join(n), b"x").expect("写夹具");
        }

        let gone = purge_old_diagnostics(&dir, today, crate::core::paths::KEEP_DAYS);
        let mut got: Vec<String> = gone
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        got.sort();
        let mut want = doomed_expect.to_vec();
        want.sort();
        assert_eq!(got, want, "删的必须正好是过界那两枚");
        for n in &keep {
            assert!(dir.join(n).exists(), "{n} 不该被删");
        }
        // 幂等：再跑一次一份都删不着（上一步已经清干净了）。
        assert!(purge_old_diagnostics(&dir, today, crate::core::paths::KEEP_DAYS).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 崩溃报告取的是**名字最新**的那枚日志，而不是"今天"那枚：跨午夜的长会话里
    /// 写入器还在写昨天那枚，按今天去找会读到空文件、丢掉"崩之前在做什么"。
    #[test]
    fn newest_run_log_picks_the_latest_dated_file() {
        let dir = crate::core::paths::test_scratch_root().join("rupix_newest_log_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建 temp 目录");
        for n in [
            "rupix-2026-10-18.log",
            "rupix-2026-10-19.log",
            "rupix.run",
            "crash-2026-10-19.log", // 崩溃报告不是运行日志，不许被选中
        ] {
            std::fs::write(dir.join(n), b"x").expect("写夹具");
        }
        let got = newest_run_log_in(&dir).expect("该挑出一枚");
        assert_eq!(got.file_name().unwrap(), "rupix-2026-10-19.log");
        // 目录不存在时安静返回 None（读路径不建目录）。
        assert!(newest_run_log_in(&dir.join("没有这层")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 剪枝：只留最近 N 份，且删的是最旧的；不足数时一份都不动。
    #[test]
    fn prune_keeps_the_newest_by_name() {
        let dir = crate::core::paths::test_scratch_root().join("rupix_prune_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建 temp 目录");
        for s in [
            "20260101000000",
            "20260102000000",
            "20260103000000",
            "20260104000000",
        ] {
            std::fs::write(dir.join(format!("crash-{s}.dmp")), b"x").expect("写假转储");
        }
        // 与转储同目录的别的文件不许被误删。
        std::fs::write(dir.join("crash.log"), b"keep me").expect("写日志");

        let doomed = prune_dumps(&dir, 2);
        assert_eq!(doomed.len(), 2, "应剪掉两份最旧的");
        assert!(doomed[0].to_string_lossy().contains("20260101000000"));
        assert!(doomed[1].to_string_lossy().contains("20260102000000"));
        assert!(
            dir.join("crash-20260104000000.dmp").exists(),
            "最新的必须还在"
        );
        assert!(dir.join("crash.log").exists(), "同名的日志不是删除目标");

        // 不足 keep 时一份都不动。
        assert!(prune_dumps(&dir, 5).is_empty());
        assert!(dir.join("crash-20260103000000.dmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧式紧凑名与新式带横线名混在一起：按名字排会把刚写的删掉，按 mtime 排才对。
    #[test]
    fn prune_keeps_fresh_dump_even_with_legacy_names() {
        let dir = crate::core::paths::test_scratch_root().join("rupix_prune_mixed_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建 temp 目录");
        for s in [
            "20260101000000",
            "20260102000000",
            "20260103000000",
            "20260104000000",
            "20260105000000",
        ] {
            std::fs::write(dir.join(format!("crash-{s}.dmp")), b"old").expect("写旧名");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        let fresh = dir.join("crash-2026-10-24-120000.dmp");
        std::fs::write(&fresh, b"fresh").expect("写新名");

        let doomed = prune_dumps(&dir, 2);
        assert_eq!(doomed.len(), 4, "六份只留两份");
        assert!(fresh.exists(), "刚写的转储不许被旧名排序坑掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 报告必须自带**配对线索**：异常码人话、RVA、线程名、exe 指纹、转储路径。
    /// 少任何一项，事后拿到 dump 也配不上符号（见模块头"为什么必须带基址"）。
    #[test]
    fn report_carries_every_clue_needed_afterwards() {
        // 基址由参数注入（不是读测试进程自己的模块）——不然这条测试钉的是运气，不是算术。
        let base = 0x7ff6_0000_0000usize;
        let addr = base + 0x1234;
        let snap = snapshot(
            "未处理异常（SEH）",
            0xC000_0005,
            addr,
            4242,
            &Some(PathBuf::from(r"C:\rupix\temp\crash-2026-10-17-203311.dmp")),
            base,
        );
        let r = &snap.report;
        assert!(r.contains("访问冲突"), "异常码要翻成人话");
        assert!(r.contains("0xC0000005"), "原始码也要留着：{r}");
        assert!(
            r.contains("RVA 0x1234"),
            "RVA 必须等于 异常地址 − 基址，实得：{r}"
        );
        assert!(
            r.contains("exe 基址    0x00007FF600000000"),
            "基址那一行也必须在"
        );
        assert!(r.contains("4242"), "要带出错线程 ID");
        assert!(
            r.contains("\n--- 现场 ---"),
            "会话环境那一节（本例没采到）不许把下一节分隔线粘在同一行：{r}"
        );
        assert!(r.contains("crash-2026-10-17-203311.dmp"), "要带转储路径");
        assert!(r.contains("字节，构建时间"), "要带 exe 指纹用于配对 PDB");
        // ★ m128:光有"字节 + 可读时间"还不够——`.temp\symbols\` 里那枚 pdb 的文件名是
        //   `<基名>-<字节数>-<紧凑本地时间>.pdb`,报障的人得把两种写法在脑子里换算一次,
        //   换算错就以为符号配不上。报告里这一行必须是能直接拿去比对文件名的那串。
        // ★ m129:基名跟着 exe 走（不再写死 `rupix`）——测试进程叫 `rupix_lib-<hash>`，
        //   示例探针叫 `panic_probe`，写死 `rupix` 的话这两处报告都是假文件名。
        let (len, _readable, stamp, path) = exe_fingerprint();
        let stem = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let expected = format!("PDB 配对    {stem}-{len}-{stamp}.pdb");
        assert!(
            r.contains(&expected),
            "PDB 配对那一行必须是真文件名形状（期望包含 {expected}），实得：{r}"
        );
        assert!(
            stamp.len() == 14 && stamp.bytes().all(|b| b.is_ascii_digit()),
            "紧凑指纹必须是 14 位数字（拿去对 pdb 文件名），实得 {stamp}"
        );
        // ★ m129:栈必须写成「模块+偏移」。`std::backtrace` 的 Display 在没有本地 PDB 时
        //   只有 `<unknown>`——名字和地址一起消失，发回来也解不出任何东西。这里钉两件事：
        //   ① 自己这一帧带着可离线解析的 `rupix.exe+0x…`；② 报告自带"怎么查"的指路。
        assert!(r.contains("--- 栈帧"), "要带栈帧那一节");
        assert!(
            r.contains(".exe+0x"),
            "自己的帧必须写成 模块+偏移（离线可解），实得：{r}"
        );
        assert!(
            r.contains("dump_sym.py"),
            "报告要自带解析工具的指路，否则拿到文件的人不知道下一步做什么"
        );
    }

    /// 基址拿不到（返回 0）时不许把 RVA 编成一个数，转储写不出来时也不许假装成功。
    #[test]
    fn report_says_honestly_when_no_dump() {
        let snap = snapshot("panic", 0, 0, 1, &None, 0);
        assert!(snap.report.contains("未能写出转储"));
        assert!(!snap.report.contains("minidump    \n"));
    }

    /// `code_note` 只覆盖认识的码，认不出的一律留空——不许编一个解释糊弄后来人。
    #[test]
    fn unknown_codes_get_no_made_up_explanation() {
        assert!(code_note(0xDEAD_BEEE).is_empty());
        assert!(code_note(0xC000_00FD).contains("栈溢出"));
    }

    /// 写满即停的三条同时成立：**开头那行还在**（这是这条设计的存在理由）、标记**只有一条**、
    /// 此后不再落盘。上限由夹具注入（真值是 MiB 级，拿它做单测等于在 temp 里写一兆）。
    ///
    /// ★ 先破后立的靶子：把 `write()` 到顶那支改回"截断重写"（`File::create` + `len=0`），
    ///   第一条断言当场红——开头那行正是被截掉的那一行。
    #[test]
    fn run_log_stops_at_the_cap_instead_of_eating_the_head() {
        let dir = crate::core::paths::test_scratch_root().join("rupix_sink_cap_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建 temp 目录");
        let path = dir.join("rupix-test.log");
        {
            let mut sink = RunLogSink::on(path.clone(), 120);
            for i in 0..10 {
                // 每行 31 字节 ⇒ 前三行（93 B）进得去，第四行撞上 120 B 闸门。
                sink.write_all(format!("line-{i:02} 0123456789 0123456789\n").as_bytes())
                    .expect("写夹具");
            }
            let _ = sink.flush();
        }
        let text = std::fs::read_to_string(&path).expect("读回夹具日志");
        let head = text.lines().next().unwrap_or("<空文件>");
        assert!(
            head.starts_with("line-00"),
            "开头那行没了——闸门又变成截断重写了：{head}"
        );
        assert_eq!(
            text.matches("不再落盘").count(),
            1,
            "停止标记必须恰好一条：\n{text}"
        );
        let kept = text.lines().filter(|l| l.starts_with("line-")).count();
        assert!(kept < 10, "写满之后还在落盘（{kept} 行都进了文件）");
        assert!(kept >= 1, "连第一行都没写进去");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
