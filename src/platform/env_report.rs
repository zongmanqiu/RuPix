//! 诊断头（总纲 §10 / 桌面元原理 E：**你的进程跑在别人的机器上，没有运维、没有 stdout**）。
//!
//! 桌面软件出事时能拿回来的通常只有一个日志文件和一个转储。日志里如果没有
//! "哪一版 / 哪个 Windows / 什么 DPI / 提没提权 / 哪块屏 / 什么命令行"，
//! 同一个现象在四种环境下的四种成因就分不开——本项目已经为此返工过两次：
//! 一次是「装在 Program Files + 非管理员」的写盘失败，一次是「explorer 重启后托盘没了」。
//! 所以这些字段不是装饰，是把"用户说打不开"翻译成"哪条分支"的前提。
//!
//! # 为什么直接写文件而不是 `log::info!`
//!
//! 它是**会话前言**，不是一条日志事件，不该受级别过滤器摆布：用户把 `RUST_LOG` 设成
//! `error`（或干脆 `off`）时，发回来的那份里仍然必须有"哪一版、什么系统、什么命令行"——
//! 那几行是判读崩溃报告的前提，也是一枚 dump 能不能配上 PDB 的唯一线索。
//! （2026-10-20 之前默认档是 `warn`，那时"走 info 就不会落盘"是更直接的理由；档提到 `info`
//! 之后这条要求**不放松**，理由换成上面那句。）
//! 于是这里交给 [`crate::platform::crash_evidence::append_run_log`] 直接写。

use std::fmt::Write as _;

use windows::core::{w, BOOL, PCSTR};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForSystem, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::IsUserAnAdmin;

/// `RtlGetVersion` 的 OSVERSIONINFO 精简版（`um/ntsecapi.h` 的 `OSVERSIONINFOW` 前五个字段）。
///
/// 不用 `GetVersionExW`：没有兼容性清单的进程它一律回 6.2，报出来的"系统版本"是假的。
/// `RtlGetVersion` 在 ntdll 里，不谎报。结构体自己定义——windows crate 的对应类型挂在
/// 没开的 feature 下，为一个版本号开一整个 feature 不值。
#[repr(C)]
struct RtlOsVersionInfo {
    dw_os_version_info_size: u32,
    dw_major_version: u32,
    dw_minor_version: u32,
    dw_build_number: u32,
    dw_platform_id: u32,
    sz_csd_version: [u16; 128],
}

/// 真实系统版本号，如 `10.0.26200`；拿不到时诚实写"（未知）"，不猜一个数。
fn os_version() -> String {
    // SAFETY: 只从 ntdll 取一个纯查询函数；签名（`*mut 结构体 -> NTSTATUS`）由 ntdll 的
    // 公开契约保证，先把尺寸字段填好，函数只会往结构体里写固定长度的值。
    unsafe {
        let ntdll = match GetModuleHandleW(w!("ntdll")) {
            Ok(h) => h,
            Err(_) => return "（未知）".into(),
        };
        // clippy 会建议写成 `c"RtlGetVersion"`，但 windows 0.62 的 `Param<PCSTR>` 不给
        // `&CStr` 实现（只认 `PCSTR` 本身）——手搓 NUL 结尾字节串是这个 crate 版本的正确写法。
        #[allow(clippy::manual_c_str_literals)]
        let addr = match GetProcAddress(ntdll, PCSTR(b"RtlGetVersion\0".as_ptr())) {
            Some(a) => a,
            None => return "（未知）".into(),
        };
        let f: unsafe extern "system" fn(*mut RtlOsVersionInfo) -> i32 = std::mem::transmute(addr);
        let mut vi = RtlOsVersionInfo {
            dw_os_version_info_size: std::mem::size_of::<RtlOsVersionInfo>() as u32,
            dw_major_version: 0,
            dw_minor_version: 0,
            dw_build_number: 0,
            dw_platform_id: 0,
            sz_csd_version: [0; 128],
        };
        if f(&mut vi) != 0 {
            return "（未知）".into();
        }
        format!(
            "{}.{}.{}",
            vi.dw_major_version, vi.dw_minor_version, vi.dw_build_number
        )
    }
}

/// 是否以管理员令牌运行。`IsUserAnAdmin` 被官方标注 deprecated，但它问的
/// （管理员组已启用）正是我们关心的那件事；替代写法要 `OpenProcessToken` +
/// `GetTokenInformation`，为一个布尔位开 `Win32_Security` 不值。
#[allow(deprecated)]
fn elevated() -> bool {
    // SAFETY: 无入参的进程级查询，返回值即结论。
    unsafe { IsUserAnAdmin().as_bool() }
}

/// 终端服务会话号；拿不到写 `u32::MAX`（报告里显示成 4294967295 一看就知道是没取到）。
fn session_id() -> u32 {
    // SAFETY: 传的是本进程 id 与栈上合法的可写 u32。
    let mut id: u32 = u32::MAX;
    let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) };
    id
}

/// 一块屏的自述：设备名、整屏矩形 (x,y,w,h)、工作区、有效 DPI。
pub(crate) struct Monitor {
    pub name: String,
    pub full: (i32, i32, i32, i32),
    pub work: (i32, i32, i32, i32),
    pub dpi: u32,
}

/// 把每块显示器追加进 `data` 指向的 `Vec<Monitor>`（`EnumDisplayMonitors` 的回调）。
unsafe extern "system" fn collect_monitors(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` 由本模块 `monitors()` 构造，指向一个活着的 `Vec<Monitor>`；
    // 回调在枚举期间同步执行，不跨线程、不外存。
    let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
    // windows-rs 把 MONITORINFOEXW 表达成 `{ monitorInfo: MONITORINFO, szDevice }`：
    // cbSize 要填**整个 EXW** 的尺寸（系统据此决定写不写 szDevice），传进去的指针却是
    // 那个嵌套的 MONITORINFO——两者同地址、EXW 是其超集，这正是 MSDN 认可的用法。
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: 回调期内 hmon 有效，info 尺寸已按上面那条规则填好。
    if unsafe { GetMonitorInfoW(hmon, &mut info.monitorInfo) }.as_bool() {
        let mut x = 0u32;
        let mut y = 0u32;
        // SAFETY: hmon 同上，x/y 是栈上可写变量。失败就留 0——报告里 `dpi=0` 比编一个数诚实。
        let _ = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
        let _ = y; // 只报横向 DPI：缩放是等比的，两轴不等的那个现场我们还没遇见过。
        out.push(Monitor {
            name: String::from_utf16_lossy(&info.szDevice)
                .trim_end_matches('\0')
                .to_string(),
            full: rect_tuple(info.monitorInfo.rcMonitor),
            work: rect_tuple(info.monitorInfo.rcWork),
            dpi: x,
        });
    }
    // TRUE = 继续枚举剩下的显示器。
    BOOL(1)
}

fn rect_tuple(r: RECT) -> (i32, i32, i32, i32) {
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// 全部显示器。单块屏取不到不影响其余（回调里每一步都允许失败）。
fn monitors() -> Vec<Monitor> {
    let mut list: Vec<Monitor> = Vec::new();
    let ptr: *mut Vec<Monitor> = &mut list;
    // SAFETY: `list` 的借用活到这条语句结束，而回调是同步的；传出去的指针只是被
    // 回调借用一次，不会被系统保存。
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitors),
            LPARAM(ptr as *mut std::ffi::c_void as isize),
        );
    }
    list
}

/// 会话环境块（多行）。启动前言与崩溃报告共用这一份——同一件事只写一次。
///
/// 每个字段都是"取不到就写（未知）"：诊断头自己也崩了，就本末倒置了。
pub fn session_block() -> String {
    let mut s = String::new();
    let (exe_len, exe_mtime, _stamp, exe_path) = crate::platform::crash_evidence::exe_fingerprint();
    let _ = writeln!(
        s,
        "版本        {}（{exe_len} 字节，构建时间 {exe_mtime}）",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(s, "路径        {exe_path}");
    let _ = writeln!(
        s,
        "系统        Windows {}（提权={}）",
        os_version(),
        if elevated() { "是" } else { "否" }
    );
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "（未知）".into());
    let sess = std::env::var("SESSIONNAME").unwrap_or_else(|_| "（未知）".into());
    let _ = writeln!(
        s,
        "会话        id={} 用户={user} 会话名={sess}",
        session_id()
    );
    // 系统 DPI 在这里只作为**标量**出现：像素换算一律由 windui 的 Per-Monitor V2 负责，
    // 读它只为"截图看着不对"提供判据（总纲 §8.6 禁的是用它拿尺寸）。
    let dpi = unsafe { GetDpiForSystem() };
    let _ = writeln!(
        s,
        "DPI         系统 {dpi}（{}%）",
        (dpi as usize * 100 / 96).max(1)
    );
    for m in monitors() {
        let _ = writeln!(
            s,
            "显示器      {} {}x{}@({},{}) 工作区 {}x{}@({},{}) dpi={}",
            m.name,
            m.full.2,
            m.full.3,
            m.full.0,
            m.full.1,
            m.work.2,
            m.work.3,
            m.work.0,
            m.work.1,
            m.dpi
        );
    }
    let _ = writeln!(
        s,
        "命令行      {}",
        std::env::args().collect::<Vec<_>>().join(" ")
    );
    s
}

/// 冻结下来的会话块：`run()` 在启动早期填一次，崩溃报告只读它。
///
/// ★ 崩溃路径上**不再采集**：会话块要进 `user32`/`shcore` 拿显示器与 DPI，而那正是
/// 崩溃现场可能已经坏掉的东西——在 SEH filter 里二次异常，连崩溃报告都写不出来。
/// 采集只发生一次，代价是"崩在采集之前"时报告里那一节写成空（[`frozen`] 返回 `None`）。
static FROZEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 采集并冻结（幂等）。返回值即 [`startup_preamble`] 正文里的那一段。
pub fn freeze() -> &'static str {
    FROZEN.get_or_init(session_block)
}

/// 崩溃报告用：没采集过就返回 `None`，让报告诚实写"未采集"，不要现场补一次采集。
pub fn frozen() -> Option<&'static str> {
    FROZEN.get().map(|s| s.as_str())
}

/// 写进 `temp\rupix-<今天>.log` 的会话前言（前后各一条分隔线，便于在滚动的日志里定位一次会话的起点）。
pub fn startup_preamble() -> String {
    format!(
        "=== RuPix 会话开始 ===\n{}\n=== 以下才是日志 ===\n",
        freeze()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 会话块必须真的带齐五类字段——缺哪一类，将来就有一类现场分不开。
    #[test]
    fn session_block_has_the_five_context_families() {
        let s = session_block();
        for need in ["版本", "路径", "系统", "会话", "DPI", "命令行"] {
            assert!(s.contains(need), "会话块缺 {need} 一行：\n{s}");
        }
        assert!(s.contains(env!("CARGO_PKG_VERSION")), "版本号没进来");
    }

    /// 版本号不许写成"（未知）"：那是 `CARGO_PKG_VERSION`，编译期就知道。
    #[test]
    fn version_is_never_unknown() {
        assert!(
            !session_block().contains("版本        （"),
            "版本行取空了——env 宏不会空，说明写入逻辑变了"
        );
    }

    /// `RtlGetVersion` 的用途就是戳穿 `GetVersionExW` 的谎报：Win10/11 必须是 10.x。
    #[test]
    fn rtl_reports_ten_not_six() {
        let v = os_version();
        assert_ne!(v, "（未知）", "这台机器上取不到版本号——门禁失去意义");
        assert!(v.starts_with("10."), "{v} 像是 GetVersionExW 的假值（6.2）");
        assert_eq!(v.matches('.').count(), 2, "形状应是 major.minor.build：{v}");
    }

    #[test]
    fn preamble_wraps_the_block_with_markers() {
        let p = startup_preamble();
        assert!(p.starts_with("=== RuPix 会话开始 ==="), "缺起始分隔线");
        assert!(p.contains("=== 以下才是日志 ==="), "缺结束分隔线");
    }

    #[test]
    fn monitor_names_are_printable() {
        let ms = monitors();
        if ms.is_empty() {
            // 无显示器的会话本身合法（服务会话、某些远程场景），但本项目只在交互桌面跑：
            // 真为空时把原因打出来让人看见，而不是让形状断言空跑一遍当通过。
            eprintln!("monitors() 为空：本次会话没有显示器，跳过形状断言");
            return;
        }
        for m in &ms {
            assert!(!m.name.is_empty(), "显示器设备名空了");
            assert!(m.full.2 > 0 && m.full.3 > 0, "显示器尺寸非正：{}", m.name);
        }
    }

    /// 工作区不应大于整屏（虚拟位置例外：副屏在原点的负方向）。
    #[test]
    fn work_area_fits_inside_the_monitor() {
        for m in monitors() {
            assert!(
                m.work.2 <= m.full.2 && m.work.3 <= m.full.3,
                "{} 工作区 {:?} 超出整屏 {:?}",
                m.name,
                m.work,
                m.full
            );
        }
    }
}
