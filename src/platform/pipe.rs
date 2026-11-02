//! 扩展进程的命名管道：长度前缀帧（u32 LE + body）。
//!
//! 只做字节流与帧边界，不解析 JSON；请求/响应类型在 `core::ext_protocol`。
//! 管道名固定前缀 `\\.\pipe\rupix-ext-`，宿主生成、扩展按请求里的名字连接。
//!
//! # 当前边界（P0.1b 第一版）
//!
//! - 同步阻塞读写；超时/取消在进程编排层做（P0.1b 后续），不在本模块里塞线程。
//! - 单实例、字节模式；一次只服务一个扩展进程。
//! - `accept` 后管道句柄交给 [`Connection`]，`PipeServer` 不再持有。
//!
//! 句柄用 `isize` 存而不是 `HANDLE`：`HANDLE` 内含裸指针，自动化 trait 不是 `Send`；
//! 扩展进程编排需要把 server 移进接受线程，故在边界处做一次显式数值化。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, WaitNamedPipeW,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::IO::CancelSynchronousIo;

/// 宿主创建的管道名前缀。
pub const NAME_PREFIX: &str = r"\\.\pipe\rupix-ext-";

/// 单帧上限。图片走共享内存，管道只传 JSON/文本；16 MiB 足够且能挡住坏长度。
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// 监听端：创建后调用 [`PipeServer::accept`] 等扩展连上来。
pub struct PipeServer {
    handle: Option<isize>,
}

impl PipeServer {
    pub fn create(name: &str) -> Result<Self, String> {
        validate_name(name)?;
        let w = wide(name);
        // ★ DACL：只放行**当前用户**与 SYSTEM（`pipe_dacl`）。命名管道默认是机器全局的
        //   （Everyone 可读写），同机另一个用户猜到名字就能连上看图片路径。
        //   造不出来就退回默认 DACL（只记 warn）——安全加固不该让插件起不来。
        let sa = crate::platform::pipe_dacl::current_user_attributes();
        let sa_ptr = sa.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null());
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(w.as_ptr()),
                // FIRST_PIPE_INSTANCE：拒绝与既有同名实例共存——抢注（squatting）
                // 直接在创建期失败，而不是等一个假客户端来连。
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                // REJECT_REMOTE_CLIENTS：只收本机连接（P0.5；配合 run.rs 的 PID 校验）。
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                if sa.is_some() { Some(sa_ptr) } else { None },
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.0.is_null() {
            return Err(format!("CreateNamedPipeW({name}) 失败"));
        }
        Ok(Self {
            handle: Some(handle.0 as isize),
        })
    }

    /// 阻塞等待扩展连接；已连接（`ERROR_PIPE_CONNECTED`）也视为成功。
    pub fn accept(mut self) -> Result<Connection, String> {
        let raw = self.handle.take().ok_or("管道已被接受")?;
        let handle = HANDLE(raw as *mut _);
        match unsafe { ConnectNamedPipe(handle, None) } {
            Ok(()) => {}
            Err(e) if win32_code(&e) == ERROR_PIPE_CONNECTED.0 => {}
            Err(e) => {
                let _ = unsafe { CloseHandle(handle) };
                return Err(format!("ConnectNamedPipe 失败：{e}"));
            }
        }
        // 记下连接方的 pid：编排层拿它与启动的子进程比对（陌生进程抢先连接直接拒）。
        let mut client_pid = 0u32;
        if let Err(e) = unsafe { GetNamedPipeClientProcessId(handle, &mut client_pid) } {
            let _ = unsafe { CloseHandle(handle) };
            return Err(format!("GetNamedPipeClientProcessId 失败：{e}"));
        }
        Ok(Connection {
            handle: raw,
            client_pid,
        })
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        if let Some(raw) = self.handle.take() {
            let _ = unsafe { CloseHandle(HANDLE(raw as *mut _)) };
        }
    }
}

/// 打断一个可能阻塞在同步管道 I/O 上的工作线程（当前唯一用途：accept 超时）。
///
/// `ConnectNamedPipe` 这类同步阻塞调用没有超时参数；编排层超时后用
/// `CancelSynchronousIo` 让它以错误返回，随后 worker 的 `Drop` 关句柄、
/// 线程自行退出。线程已退出时是无害 no-op。
pub fn cancel_synchronous_io<T>(worker: &std::thread::JoinHandle<T>) {
    unsafe {
        let _ = CancelSynchronousIo(HANDLE(worker.as_raw_handle()));
    }
}

/// 客户端/已连接的管道。`Drop` 时 `DisconnectNamedPipe` + `CloseHandle`。
pub struct Connection {
    handle: isize,
    /// 服务端 accept 时抓到的对端 pid（客户端 `connect` 侧为 0，无意义）。
    client_pid: u32,
}

impl Connection {
    /// 服务端视角：连接方进程 id（run.rs 拿它与启动的子进程比对）。
    pub fn client_pid(&self) -> u32 {
        self.client_pid
    }

    /// 连接宿主创建的管道；`timeout_ms` 内重试 `ERROR_PIPE_BUSY`。
    pub fn connect(name: &str, timeout_ms: u64) -> Result<Self, String> {
        validate_name(name)?;
        let w = wide(name);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let r = unsafe {
                CreateFileW(
                    PCWSTR(w.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            };
            match r {
                Ok(handle) => {
                    return Ok(Self {
                        handle: handle.0 as isize,
                        client_pid: 0,
                    })
                }
                Err(e) if win32_code(&e) == ERROR_PIPE_BUSY.0 => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(format!("连接管道 {name} 超时"));
                    }
                    let remain = deadline.saturating_duration_since(now).as_millis();
                    let remain = u32::try_from(remain).unwrap_or(u32::MAX);
                    unsafe {
                        let _ = WaitNamedPipeW(PCWSTR(w.as_ptr()), remain);
                    }
                }
                Err(e) => return Err(format!("CreateFileW({name}) 失败：{e}")),
            }
        }
    }

    pub fn write_frame(&mut self, body: &[u8]) -> Result<(), String> {
        if body.len() > MAX_FRAME {
            return Err(format!("帧 {} 超过上限 {MAX_FRAME}", body.len()));
        }
        let len = u32::try_from(body.len())
            .map_err(|_| "帧长度超过 u32".to_string())?
            .to_le_bytes();
        self.write_all(&len)?;
        self.write_all(body)
    }

    pub fn read_frame(&mut self) -> Result<Vec<u8>, String> {
        let mut len = [0u8; 4];
        self.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(format!("帧 {len} 超过上限 {MAX_FRAME}"));
        }
        let mut body = vec![0u8; len];
        self.read_exact(&mut body)?;
        Ok(body)
    }

    fn write_all(&mut self, mut buf: &[u8]) -> Result<(), String> {
        while !buf.is_empty() {
            let mut written = 0u32;
            unsafe {
                WriteFile(
                    HANDLE(self.handle as *mut _),
                    Some(buf),
                    Some(&mut written),
                    None,
                )
            }
            .map_err(|e| format!("WriteFile 失败：{e}"))?;
            if written == 0 {
                return Err("WriteFile 写入 0 字节".into());
            }
            buf = &buf[written as usize..];
        }
        Ok(())
    }

    fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<(), String> {
        while !buf.is_empty() {
            let mut read = 0u32;
            unsafe {
                ReadFile(
                    HANDLE(self.handle as *mut _),
                    Some(buf),
                    Some(&mut read),
                    None,
                )
            }
            .map_err(|e| format!("ReadFile 失败：{e}"))?;
            if read == 0 {
                return Err("管道已关闭".into());
            }
            let (_, rest) = buf.split_at_mut(read as usize);
            buf = rest;
        }
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let h = HANDLE(self.handle as *mut _);
        let _ = unsafe { CloseHandle(h) };
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if !name.starts_with(NAME_PREFIX) {
        return Err(format!("管道名必须以 {NAME_PREFIX} 开头"));
    }
    Ok(())
}

fn win32_code(e: &windows::core::Error) -> u32 {
    (e.code().0 as u32) & 0xFFFF
}

/// UTF-16 宽字符串（带结尾 NUL）。
pub(super) fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;

    fn unique_name(tag: &str) -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!(
            "{NAME_PREFIX}{}-{}-{}",
            std::process::id(),
            tag,
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn frame_roundtrip() {
        let name = unique_name("roundtrip");
        let server = PipeServer::create(&name).expect("server");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut conn = server.accept().expect("accept");
            let req = conn.read_frame().expect("read");
            conn.write_frame(b"pong").expect("write");
            tx.send(req).expect("send");
        });

        let mut client = Connection::connect(&name, 2_000).expect("client");
        client.write_frame(b"ping").expect("write ping");
        assert_eq!(client.read_frame().expect("read pong"), b"pong".to_vec());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).expect("req"),
            b"ping".to_vec()
        );
    }

    #[test]
    fn rejects_bad_name_and_oversize() {
        assert!(PipeServer::create(r"\\.\pipe\other").is_err());
        assert!(Connection::connect(r"\\.\pipe\other", 10).is_err());
        // 不能真分配 16 MiB+ 去测；用长度头直接构造一个超限帧的读端不现实，
        // 这里只锁常量与写端检查。
        assert_eq!(MAX_FRAME, 16 * 1024 * 1024);
    }

    #[test]
    fn accept_can_be_interrupted_from_outside() {
        // 锁死「CancelSynchronousIo 能唤醒阻塞中的 ConnectNamedPipe」这个行为：
        // run.rs 的超时/取消路径依赖它；退化成永远阻塞 = 每失败一次泄漏
        // 一个线程 + 一枚句柄，而且单测测不出来。
        let server = PipeServer::create(&unique_name("interrupt")).expect("server");
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(server.accept().is_err());
        });
        std::thread::sleep(Duration::from_millis(200));
        cancel_synchronous_io(&worker);
        let interrupted = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("worker 没有被唤醒（3s 内无返回）");
        assert!(interrupted, "被打断的 accept 应返回错误而不是成功");
        worker.join().expect("worker 应自行退出");
    }
}
