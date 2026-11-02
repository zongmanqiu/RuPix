//! 扩展进程启动/等待/终止。
//!
//! 用 `std::process::Command` 而不是手写 `CreateProcessW`：命令行转义、句柄继承、
//! 错误码这些坑 Rust 标准库已经踩平；本层只负责把窗口隐藏、stdin 断开，
//! 并把等待超时包成扩展编排需要的形状。
//!
//! 扩展进程**不继承**宿主的标准输入：IPC 走命名管道。
//! 标准输出/错误有两种去法（见 [`spawn`] 与 [`spawn_logged`]）：
//! - [`spawn`]：丢弃（`Stdio::null()`）——父进程退出后子进程不该继续持有控制台句柄。
//! - [`spawn_logged`]：**抽进宿主日志**——插件崩了、卡了、写了诊断信息，
//!   只有一条线索时，丢掉输出等于把线索扔了（这是插件体系可运维性的地基）。
//!   抽取线程在子进程退出、管道关闭后自行结束，不留悬挂线程。

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child as StdChild, Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::System::Threading::CREATE_NO_WINDOW;

/// 一个已启动的扩展进程。
pub struct Child {
    inner: StdChild,
    pid: u32,
}

impl Child {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 非阻塞查询；`Ok(Some(code))` = 已退出。
    pub fn try_wait(&mut self) -> Result<Option<u32>, String> {
        self.inner
            .try_wait()
            .map(|opt| opt.map(|s| s.code().unwrap_or(-1) as u32))
            .map_err(|e| format!("try_wait 失败：{e}"))
    }

    /// 等退出；超时返回 `Ok(None)`，进程仍在跑。
    pub fn wait_timeout(&mut self, timeout: Duration) -> Result<Option<u32>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(code) = self.try_wait()? {
                return Ok(Some(code));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 强制终止并回收句柄。已退出时是幂等的。
    pub fn terminate(&mut self) -> Result<(), String> {
        match self.inner.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(e) => return Err(format!("try_wait 失败：{e}")),
        }
        self.inner.kill().map_err(|e| format!("kill 失败：{e}"))?;
        let _ = self.inner.wait();
        Ok(())
    }
}

/// 启动扩展。`cwd` 固定为扩展目录，`entry` 由 manifest 校验过必须是相对路径。
///
/// 标准输出与错误**丢弃**（`Stdio::null()`）。要留线索用 [`spawn_logged`]。
pub fn spawn(entry: &Path, args: &[String], cwd: &Path) -> Result<Child, String> {
    if !entry.is_file() {
        return Err(format!("入口不存在：{}", entry.display()));
    }
    let mut cmd = Command::new(entry);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0);
    let child = cmd
        .spawn()
        .map_err(|e| format!("启动 {} 失败：{e}", entry.display()))?;
    let pid = child.id();
    Ok(Child { inner: child, pid })
}

/// 启动扩展并把它的 stdout / stderr **抽进宿主日志**（每行一条，级别区分）。
///
/// `tag` 是插件 id 之类的短标识，会出现在日志行里，排查时能一眼知道是谁说的。
///
/// 为什么值得单独做一套：插件是**别人的进程**，它崩了、它卡了、它打了诊断信息，
/// 宿主这头若把输出丢进 `Stdio::null()`，现场就只剩"插件没回应"四个字——
/// 而插件那句 `eprintln!` 往往正是唯一的原因。
///
/// 两个抽取线程：管道随子进程退出而关闭，`lines()` 读到 EOF 即返回，线程自然结束
/// （不留悬挂线程）。同时因为一直在读，插件输出再多也不会把管道写满而卡住它。
pub fn spawn_logged(entry: &Path, args: &[String], cwd: &Path, tag: &str) -> Result<Child, String> {
    if !entry.is_file() {
        return Err(format!("入口不存在：{}", entry.display()));
    }
    let mut cmd = Command::new(entry);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 {} 失败：{e}", entry.display()))?;
    let pid = child.id();
    if let Some(out) = child.stdout.take() {
        spawn_drain(out, tag.to_string(), false);
    }
    if let Some(err) = child.stderr.take() {
        spawn_drain(err, tag.to_string(), true);
    }
    Ok(Child { inner: child, pid })
}

/// 抽一行行读管道，按行写宿主日志。`is_err` = 走 stderr（warn 级）。
fn spawn_drain<R: std::io::Read + Send + 'static>(reader: R, tag: String, is_err: bool) {
    // 抽取线程本身不该 panic：读管道出错就收工，别把宿主带下水。
    let _ = std::thread::Builder::new()
        .name(format!("plug-out-{}", tag))
        .spawn(move || {
            let reader = BufReader::new(reader);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if is_err {
                    log::warn!("[插件 {tag}] {line}");
                } else {
                    log::info!("[插件 {tag}] {line}");
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd_exe() -> std::path::PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        std::path::PathBuf::from(root).join("System32\\cmd.exe")
    }

    #[test]
    fn observes_exit_code() {
        let cwd = crate::core::paths::test_scratch_root();
        let mut child =
            spawn(&cmd_exe(), &["/c".into(), "exit".into(), "3".into()], &cwd).expect("spawn cmd");
        let code = child
            .wait_timeout(Duration::from_secs(5))
            .expect("wait")
            .expect("cmd 应退出");
        assert_eq!(code, 3);
        assert!(child.pid() > 0);
    }

    #[test]
    fn terminate_running_child() {
        let cwd = crate::core::paths::test_scratch_root();
        let mut child = spawn(
            &cmd_exe(),
            &[
                "/c".into(),
                "ping".into(),
                "-n".into(),
                "20".into(),
                "127.0.0.1".into(),
            ],
            &cwd,
        )
        .expect("spawn ping");
        assert!(child
            .wait_timeout(Duration::from_millis(50))
            .expect("wait")
            .is_none());
        child.terminate().expect("terminate");
        assert!(child.try_wait().expect("try").is_some());
    }
}
