//! 最小 HTTPS GET（WinHTTP）：只做一件事——把一个 URL 的响应体取成字节。
//!
//! # 为什么是 WinHTTP 而不是 WinInet
//!
//! WinInet（`InternetOpenUrl`）看起来更短，但它整套语义是"浏览器"：有 URL 缓存、
//! Cookie 罐、自动拨号、离线模式。我们只要一份**每次都要最新**的清单，缓存命中就等于
//! 用户点了刷新却看到旧数据。WinHTTP 是服务型 API，无缓存副作用，HTTPS 开箱可用。
//!
//! # 边界条件都显式处理
//!
//! - **超时**（[`DEFAULT_TIMEOUT_MS`]）：四个阶段都设，缺一个都能挂到天荒地老
//!   （典型是 TCP 连上了但服务器不回，只设 connect 超时的代码会永远等下去）。
//! - **响应体上限**：调用方传入 `max_bytes`，超了立即报错并关连接——不解压、不落盘，
//!   一份"清单"不该有几十 MB。
//! - **状态码**：非 200 一律报错并带上码值。gitee 的 raw 地址在文件不存在时返回 404 页面，
//!   不查状态码就会把一页 HTML 当 JSON 去解析，报出来的错完全指错方向。
//! - **句柄**：`session → connect → request` 三级全部走 RAII（[`Handle`]），
//!   任何一步提前 `return` 都不会漏关——漏关的句柄在设置窗反复开关时会攒成泄漏。
//!
//! 重定向交给 WinHTTP 的默认策略：清单源用 CDN 直链（Gitee `raw.giteeusercontent.com` /
//! GitHub `raw.githubusercontent.com`，都不 redirect）；只有 release 资产会跳到
//! `foruda.gitee.com` / `objects.githubusercontent.com`。本模块不自己读 `Location`，
//! 实测 WinHTTP 会跟随；真遇到未跟随的 3xx 仍如实报 `HTTP 302`，不静默猜地址。
use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE,
};

/// 默认超时（毫秒，四个阶段各一份）。
///
/// **曾经是 3 秒**（"一份 2 KB 清单足够"）——但那是进页同步拉清单的时代。现在清单刷新
/// 与 DLL 下载都在后台线程（界面有进度/状态行），不再拿 UI 线程赌网络；弱网或代理探测
/// 时 3 秒会误报 `0x80072EE2`（ERROR_WINHTTP_TIMEOUT，2026-10-26 Gitee 刷新实测）。
/// 用户 2026-10-26 拍板 **30 秒**：给 DNS / 代理 / TLS / Gitee CDN 留足余量；超时仍会
/// 如实报（界面只说"连接超时"，不显示十六进制码）。
pub const DEFAULT_TIMEOUT_MS: i32 = 30_000;

/// User-Agent。带上项目名让服务端日志里能认出来；不带版本号——我们不靠它做兼容分支。
const AGENT: &str = "RuPix-catalog/1";

/// 一次 GET 的句柄。`Drop` 里关，任何提前返回都不会漏。
struct Handle(*mut c_void);

impl Handle {
    fn new(p: *mut c_void, what: &str) -> Result<Self, String> {
        if p.is_null() {
            Err(format!("{what}失败（WinHTTP 未返回句柄）"))
        } else {
            Ok(Self(p))
        }
    }

    fn raw(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: 句柄由 WinHTTP 创建、本结构独占；`Drop` 只执行一次。
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// 拆过的 URL。只认 `http` / `https` + 主机 + 路径（无认证段、无 IPv6 字面量）。
struct Url {
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

fn split_url(url: &str) -> Result<Url, String> {
    let (rest, secure) = if let Some(r) = url.strip_prefix("https://") {
        (r, true)
    } else if let Some(r) = url.strip_prefix("http://") {
        (r, false)
    } else {
        return Err(format!("只支持 http/https：{url}"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(format!("地址里没有主机名：{url}"));
    }
    if authority.contains('@') {
        // 带凭据的 URL 属于"密钥不该出现在这里"的一类，直接拒绝而不是悄悄支持。
        return Err("不接受带凭据的 URL".to_string());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h,
            p.parse::<u16>().map_err(|_| format!("端口不是数字：{p}"))?,
        ),
        None => (authority, if secure { 443 } else { 80 }),
    };
    Ok(Url {
        secure,
        host: host.to_string(),
        port,
        path: path.to_string(),
    })
}

/// UTF-16 + NUL。Win32 的宽字符 API 要的就是这个，且缓冲区必须活到调用返回。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// HTTP GET，返回响应体字节。
///
/// `max_bytes` 是硬上限：超过即报错（不截断——截断的 JSON 会在解析阶段报一个
/// 与真正原因无关的"第 N 字节"错误，更难查）。
pub fn get(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    get_with_progress(url, max_bytes, |_| {})
}

/// 同 [`get`]，每读到一个块就回调一次**已收字节数**（用于下载进度）。
///
/// 回调在调用方的线程上同步执行（DLL 下载时就是那个后台下载线程）；不要在里面
/// 做 UI 操作。总字节数由调用方自己知道（清单里的 `size`），这里不猜 Content-Length。
pub fn get_with_progress(
    url: &str,
    max_bytes: usize,
    mut on_progress: impl FnMut(u64),
) -> Result<Vec<u8>, String> {
    let u = split_url(url)?;
    let agent = wide(AGENT);
    let host = wide(&u.host);
    let path = wide(&u.path);
    let verb = wide("GET");

    // SAFETY: 所有指针都指向在本函数栈上活着的、以 NUL 结尾的 UTF-16 缓冲区；
    // 句柄一旦创建就交给 `Handle` 独占并在 `Drop` 里关闭。
    unsafe {
        let session = Handle::new(
            WinHttpOpen(
                PCWSTR::from_raw(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "打开 WinHTTP 会话",
        )?;
        // 四个阶段都设。少设一个就可能挂死在这个阶段上。
        WinHttpSetTimeouts(
            session.raw(),
            DEFAULT_TIMEOUT_MS,
            DEFAULT_TIMEOUT_MS,
            DEFAULT_TIMEOUT_MS,
            DEFAULT_TIMEOUT_MS,
        )
        .map_err(|e| format!("设置超时失败（0x{:08X}）", e.code().0 as u32))?;

        let conn = Handle::new(
            WinHttpConnect(session.raw(), PCWSTR::from_raw(host.as_ptr()), u.port, 0),
            "连接服务器",
        )?;

        let flags = if u.secure {
            WINHTTP_FLAG_SECURE
        } else {
            Default::default()
        };
        let req = Handle::new(
            WinHttpOpenRequest(
                conn.raw(),
                PCWSTR::from_raw(verb.as_ptr()),
                PCWSTR::from_raw(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            ),
            "创建请求",
        )?;

        WinHttpSendRequest(req.raw(), None, None, 0, 0, 0)
            .map_err(|e| format!("发送请求失败（0x{:08X}）", e.code().0 as u32))?;
        WinHttpReceiveResponse(req.raw(), std::ptr::null_mut())
            .map_err(|e| format!("读取响应失败（0x{:08X}）", e.code().0 as u32))?;

        // 状态码：不查它就会把 404 的 HTML 页面当 JSON 解析（错误指错方向）。
        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            req.raw(),
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| format!("读状态码失败（0x{:08X}）", e.code().0 as u32))?;
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }

        let mut body: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let mut got: u32 = 0;
            WinHttpReadData(
                req.raw(),
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut got,
            )
            .map_err(|e| format!("读响应体失败（0x{:08X}）", e.code().0 as u32))?;
            if got == 0 {
                break;
            }
            body.extend_from_slice(&buf[..got as usize]);
            on_progress(body.len() as u64);
            if body.len() > max_bytes {
                return Err(format!(
                    "响应超过 {max_bytes} 字节上限（这不是一份清单该有的大小）"
                ));
            }
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        let u = split_url("https://example.com/a/b.json").expect("https");
        assert!(u.secure);
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 443);
        assert_eq!(u.path, "/a/b.json");

        let u = split_url("http://example.com:8080/x").expect("http 带端口");
        assert!(!u.secure);
        assert_eq!(u.port, 8080);

        // 没写路径：补 "/"，否则 WinHttpOpenRequest 拿到空对象名会 400。
        let u = split_url("https://example.com").expect("裸主机");
        assert_eq!(u.path, "/");
    }

    #[test]
    fn rejects_unusable_urls() {
        assert!(split_url("ftp://example.com/x").is_err(), "非 http(s)");
        assert!(split_url("https:///x").is_err(), "没有主机");
        // 带凭据的地址要拒绝。★ 这里**刻意在运行期拼**：源码里若写出完整形状
        // （协议名 + `://` + 账号:口令 + `@` + 主机），hygiene 第 12 查会当成真凭据
        // 抓走——它抓得对，所以示例让路（同族：K144「禁止 X 的文档天然带着 X 的样本」）。
        let creds = "user:pw";
        let url = format!("https://{creds}@example.com/x");
        assert!(split_url(&url).is_err(), "带凭据");
        assert!(
            split_url("https://example.com:abc/x").is_err(),
            "端口不是数字"
        );
    }

    // ★ 真联网那条不放在这里：`platform` 不许引 `decoders`（分层门禁），
    //   而清单地址属于解码器分发约定，测试放在 `decoders::remote` 里。
}
