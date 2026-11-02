//! 删除文件到回收站（Windows 外壳）。
//!
//! 设置页「解码器信息 → 卸载」的唯一落点。
//!
//! 「唯一落点」的另一面是**别的删除不准进这里**：`temp\` 里过期的日志与转储由
//! [`crate::platform::crash_evidence`] 硬删。判据是删的是谁的资产——DLL 是用户装进来的，
//! 删错要能还原；日志是应用自己产的取证中间物，且外壳删除单枚要 46 ms（硬删 0.13 ms）、
//! 在 U 盘 / 网络共享上根本没有回收站可进。理由写在那边，不重复一遍。
//!
//! # 为什么不用 `std::fs::remove_file`
//!
//! 卸载按用户口径（2026-10-16）必须**可后悔**：删错的 DLL 要能从回收站还原。
//! `remove_file` 是硬删，删掉就没了；而回收站是外壳命名空间里的一对元数据
//! （`$I` 描述 + `$R` 实体），自己往 `$Recycle.Bin` 里挪文件会缺元数据、把回收站
//! 弄脏（条目显示不出原名/原位置，还原功能直接失效）。所以只能走外壳 API。
//!
//! # 为什么是 `SHFileOperationW` 而不是 `IFileOperation`
//!
//! 后者要 `CoCreateInstance` + `SHCreateItemFromParsingName` 组 `IShellItem`，三十多行
//! 只为删一个文件。前者一次调用搞定，代价是它已 deprecated——内部走的仍是同一套
//! 外壳操作，Windows 10/11 上工作正常，哪天真被移除再换不迟。
//!
//! # 删不掉是常态，不是异常
//!
//! 解码器 DLL 一旦被 `LoadLibrary` 加载，Windows 就以「只读共享」打开它，
//! 删除与改名都会失败（`ERROR_ACCESS_DENIED`）——文件还在进程里跑着，这正确。
//! 失败原因如实交回 UI（提示重启后再来），**不假装成功、不留半删状态**。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
    SHFILEOPSTRUCTW,
};

/// 把 `path` 移进回收站。全程无弹窗（不确认、不报外壳错误框、不显进度）。
///
/// 成功 = 文件已不在原处、回收站里能还原。失败返回人类可读的原因，
/// 调用方负责呈现（设置页走 `toast_err`）。
pub fn send_to_recycle_bin(path: &Path) -> Result<(), String> {
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    if from.is_empty() || from.contains(&0) {
        return Err(format!("路径不合法：{}", path.display()));
    }
    // 外壳要的是**双 NUL 结尾**的一串路径：末尾补两个 0 = 「一个路径 + 空串收尾」。
    from.push(0);
    from.push(0);

    // 四个标志缺一不可：
    //   FOF_ALLOWUNDO      —— 进回收站而不是硬删（本函数存在的全部意义）
    //   FOF_NOCONFIRMATION —— 不弹「确实要把它放进回收站吗」
    //   FOF_NOERRORUI      —— 失败不弹外壳错误框（错误由调用方呈现）
    //   FOF_SILENT         —— 不弹「正在删除…」进度框
    let flags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16;
    // 其余字段（`pTo` / `fAnyOperationsAborted` / `hNameMappings` / `lpszProgressTitle`）
    // 全零即可：`SHFILEOPSTRUCTW::default()` 就是零初始化。
    let mut op = SHFILEOPSTRUCTW {
        hwnd: HWND::default(),
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: flags,
        ..Default::default()
    };

    // 外壳 API 要 COM。本线程没起过就由我们起 STA，离开时收回；
    // 已经起过（S_FALSE / 起不来）都别动别人的——尤其不能替别人 CoUninitialize。
    let _com = ComGuard::new();
    let rc = unsafe { SHFileOperationW(&mut op) };
    match rc {
        0 => Ok(()),
        _ => Err(describe(rc, op.fAnyOperationsAborted.as_bool())),
    }
}

/// 把外壳的返回值翻成人话。`rc` 是 `SHFileOperationW` 的原始返回（非 Win32 错误码）。
fn describe(rc: i32, aborted: bool) -> String {
    if aborted {
        return "操作被外壳中止".to_string();
    }
    match rc {
        0x402 => "路径不存在".to_string(),   // 文档口径：源/目的路径非法
        0x4C7 => "已被用户取消".to_string(), // ERROR_CANCELLED
        _ => format!("外壳返回 0x{rc:04X}"),
    }
}

/// COM 初始化的作用域守卫：只有**本线程第一次**初始化成功（S_OK）才在 drop 时收回。
pub(crate) struct ComGuard(bool);

impl ComGuard {
    pub(crate) fn new() -> Self {
        // `hr.0 == 0` 即 S_OK：这次是我们起的。S_FALSE（已是 STA）与
        // RPC_E_CHANGED_MODE（已是 MTA）都不归我们管。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(hr.0 == 0)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 含 NUL / 空路径一律在进外壳之前就被挡掉（双 NUL 结尾拼不出来，
    /// 硬塞进去会被外壳当成"空列表"静默返回成功——那才是最坏的结果）。
    #[test]
    fn rejects_unrepresentable_path() {
        assert!(send_to_recycle_bin(Path::new("a\0b.dll")).is_err());
        assert!(send_to_recycle_bin(Path::new("")).is_err());
    }

    /// 真删一次：临时文件进回收站，原地不再有。
    ///
    /// 这是本模块唯一"动真格"的测试——`SHFileOperationW` 的标志组合写错
    /// （漏 `FOF_NOCONFIRMATION`）会当场弹系统确认框把测试挂住，编译期看不出来，
    /// 只有真跑一遍才验得到。代价是每次跑测试往回收站多一个 0 字节文件，可接受。
    #[test]
    fn moves_file_into_recycle_bin() {
        let name = format!(
            "rupix-recycle-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        let p = crate::core::paths::test_scratch_root().join(name);
        std::fs::write(&p, b"rupix recycle test").expect("临时文件要能写");

        assert!(
            send_to_recycle_bin(&p).is_ok(),
            "移进回收站失败：文件仍在原地"
        );
        assert!(!p.exists(), "进了回收站，原路径不该还在");
    }
}
