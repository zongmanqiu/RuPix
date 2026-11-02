//! 工作集裁剪：把进程当前工作集（Working Set）里「可归还」的页——已释放但仍留在 WS 的
//! 堆页、以及文件映射页——交还系统，让任务管理器看到的数字落回个位数。
//!
//! ★ 与 CurSeen 同款手段（`platform/working_set.rs`）。这些页下次被触碰会自动按需调回，
//! 所以对「常驻后台、间歇交互」的看图器几乎零代价：启动期字体扫描 / DLL 加载留下的残留页
//! 不会凭空消失，只是退回按需调度的状态。
//!
//! 实证（本机 `rupix.exe`，启动后开设置窗）：WS 约 27MB，`EmptyWorkingSet` 一刀 → 0.45MB，
//! 私有内存基本不变——那 20MB+ 大头都是可归还页，不是真占用。故本优化针对的是
//! 「任务管理器数字」，而非"改不了"的已提交内存；这与用户"超过 20MB"的体感完全对应。

#[cfg(windows)]
pub fn trim() -> bool {
    use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: `GetCurrentProcess` 返回进程伪句柄（无需关闭）；`EmptyWorkingSet` 仅读取 /
    // 归还页，不破坏进程状态、不改返回后行为。
    let ok = unsafe { EmptyWorkingSet(GetCurrentProcess()) }.is_ok();
    if !ok {
        log::warn!("EmptyWorkingSet 裁剪工作集失败（仍按原内存运行，不影响功能）");
    }
    ok
}

#[cfg(not(windows))]
pub fn trim() -> bool {
    // 非 Windows 平台上工作集裁剪无意义，静默 no-op。
    true
}

/// 距用户最近一次「会话级」键鼠输入已过去的毫秒数。用于空闲判定：用户停下交互一段
/// 时间后再裁工作集，既不打扰活跃使用、又能回收「启动后才开的窗口（如设置）」的分配。
#[cfg(windows)]
pub fn idle_millis() -> u64 {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetLastInputInfo;
    use windows::Win32::UI::Input::KeyboardAndMouse::LASTINPUTINFO;
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` 已初始化 cbSize；`GetLastInputInfo` 只写入 dwTime。取不到（极少）
    // 就保守返回 MAX，让调用方不裁。
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        let now = unsafe { GetTickCount() };
        // 两者都是 32 位 tick（约 49.7 天回绕），用 wrapping 减避免回绕误判。
        now.wrapping_sub(info.dwTime) as u64
    } else {
        u64::MAX
    }
}

#[cfg(not(windows))]
pub fn idle_millis() -> u64 {
    // 非 Windows 不裁剪，直接返回「永不空闲」以免误触。
    u64::MAX
}

#[cfg(all(windows, test))]
mod tests {
    use super::trim;

    #[test]
    fn trim_is_safe_and_idempotent() {
        // 裁剪本身无害：连调两次也不应 panic / 影响后续运行。
        let _ = trim();
        let _ = trim();
    }

    #[test]
    fn idle_millis_is_safe() {
        // ★ 断言口径：**同基代数的恒等式**，不是「现在没空闲超过 60 秒」，也不是
        // 「idle 不可能大于开机时长」（后者不成立：`GetTickCount` 是 32 位 tick，
        // 开机跨过一整轮约 49.7 天后，uptime 会显示成回绕后的余数，反而小于真实 idle；
        // 本机实测踩到——idle=139609 而 uptime=630021312，按那条不变量直接误红）。
        //
        // 旧断言 `m < 60_000 || m == u64::MAX` 读的是整机真实键鼠空闲时长：人离开键盘超过
        // 一分钟必红（无人值守 / CI / AI 跑门禁时必定发生，实测同一二进制依次报
        // 87s → 109s → 123s → 167s，只递增不收敛）。更糟的是它在 5_test.bat 的第一层，
        // 一红就中断，把真正的文档一致性与集成门禁全挡成「没跑」。
        // 另：`m == u64::MAX` 那个哨兵分支数学上不可达——两个 tick 同纪元，`wrapping_sub`
        // 只会得到真实空闲时长，所以旧断言实际在验死码。
        use windows::Win32::System::SystemInformation::GetTickCount;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

        let t0 = unsafe { GetTickCount() };
        let probe = super::idle_millis(); // 主目的：探测本身不得 panic
        let t1 = unsafe { GetTickCount() };
        if probe == u64::MAX {
            return; // GetLastInputInfo 失败的本实现约定值，同样算「安全」
        }

        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        let got = unsafe { GetLastInputInfo(&mut info) }.as_bool();
        // 独立取证：生产路径取到过值，这里也必须取到（拿不到就说明探测不可信）。
        assert!(got, "idle_millis 取到值，同基复取却失败");

        let m = probe as u32;
        // 恒等式：idle = last_tick - t_now（回绕域）。因此它必然落在
        // [t1-last, t0-last] 之间——同基 wrapping 减天然消掉回绕，与是否有人在用无关。
        let upper = t0.wrapping_sub(info.dwTime);
        let lower = t1.wrapping_sub(info.dwTime);
        assert!(
            lower <= m && m <= upper,
            "idle 读数与时间基不自洽：idle={m} 区间=[{lower},{upper}]"
        );
    }
}
