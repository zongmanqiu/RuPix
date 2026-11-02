//! 扩展进程的 Job Object：宿主退出（含崩溃）时由内核兜底杀掉整组子进程。
//!
//! 每次插件调用一个专用 Job（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`）：启动子进程后
//! `AssignProcessToJobObject`，正常收尾时主动 terminate；宿主异常退出时句柄随进程
//! 关闭 → Job 的最后一个句柄由内核关闭 → 整组进程被杀。子进程自己 spawn 的孙进程
//! 默认也在同一 Job 里，想逃逸必须显式 BREAKAWAY——那本身就是一个值得记录的信号。
//!
//! 这是 P0.5 的第一块：进程级"跟宿主共存亡"，与管道的 PID 校验、三振隔离互补。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

/// 一个已创建的 Job。Drop = terminate + 关句柄（幂等）。
pub struct Job {
    handle: isize,
}

impl Job {
    /// 创建带 KILL_ON_JOB_CLOSE 限位的 Job。
    pub fn create() -> Result<Self, String> {
        unsafe {
            let handle = CreateJobObjectW(None, PCWSTR::null())
                .map_err(|e| format!("CreateJobObjectW 失败：{e}"))?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if let Err(e) = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) {
                let _ = CloseHandle(handle);
                return Err(format!("SetInformationJobObject 失败：{e}"));
            }
            Ok(Self {
                handle: handle.0 as isize,
            })
        }
    }

    /// 把进程（按 pid）加入 Job。失败只降级（少一层兜底），调用方记 warn 即可。
    pub fn assign(&self, pid: u32) -> Result<(), String> {
        unsafe {
            let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)
                .map_err(|e| format!("OpenProcess({pid}) 失败：{e}"))?;
            let r = AssignProcessToJobObject(HANDLE(self.handle as *mut _), proc);
            let _ = CloseHandle(proc);
            r.map_err(|e| format!("AssignProcessToJobObject({pid}) 失败：{e}"))
        }
    }

    /// 立刻终止整组进程（幂等）。
    pub fn terminate(&self) {
        unsafe {
            let _ = TerminateJobObject(HANDLE(self.handle as *mut _), 0);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.terminate();
        unsafe {
            let _ = CloseHandle(HANDLE(self.handle as *mut _));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用真子进程验证 assign/terminate：先起的 sleep 子进程被 Job 终止后应立即退出。
    /// ★ 不能拿自身进程测 terminate——Job 会连调用者一起杀，测试进程直接被自己干掉。
    #[test]
    fn job_create_assign_terminate_roundtrip() {
        use std::os::windows::process::CommandExt;
        let job = Job::create().expect("创建 Job");
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping", "-n", "5", "127.0.0.1"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .expect("spawn sleep 子进程");
        job.assign(child.id()).expect("assign");
        let t0 = std::time::Instant::now();
        job.terminate();
        child.wait().expect("wait");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(3),
            "terminate 应立即生效，而不是等子进程自然睡完（未生效 = 没真杀）"
        );
        // 退出码不断言：TerminateJobObject(0) 的退出码就是 0，与 cmd 正常退出同值；
        // 时序断言已经足以证明 Job 终止生效。
    }
}
