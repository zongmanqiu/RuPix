//! 命名管道的 DACL：**只允许当前用户**（外加 SYSTEM）访问。
//!
//! # 为什么需要
//!
//! Win32 的命名管道是**机器全局**的：默认 DACL 允许「 Everyone」读写，所以同一台
//! 机器上的**另一个用户**（共享机器、远程桌面多账号）只要猜到管道名就能连上，
//! 读到宿主发给插件的请求（含 `image_path` 与选区）。
//!
//! 宿主的管道名是可预测的（`\\.\pipe\rupix-ext-<pid>-<id>-<seq>`，pid 与 seq 都小），
//! 所以"猜得到"是现实假设，不是理论风险。
//!
//! # 这一层防什么、不防什么
//!
//! | 威胁 | 防得住吗 |
//! |---|---|
//! | 另一个用户连上宿主与插件之间的管道 | ✅ 本层（DACL 只放当前用户 + SYSTEM） |
//! | 同用户的恶意进程抢先连上（抢注） | ❌ 但编排层会按 **PID 校验**拒掉它（`run.rs`） |
//! | 同用户的恶意进程在宿主之后连上 | ❌ 同样被 PID 校验拒掉 |
//!
//! 也就是说：**数据路径已经被 PID 校验堵死，本层是把"跨用户"那一条也堵上**——
//! 纵深防御的第二层。同一台机器上的另一个用户不该看到你的图片路径。
//!
//! # 实现口径
//!
//! - DACL 用 **SDDL** 描述：`D:P(A;;GA;;;<当前用户 SID>)(A;;GA;;;SY)`
//!   （受保护 DACL，只放行当前用户与 SYSTEM；`GA` = 全部权限，因为客户端要读写）。
//! - 当前用户 SID 取自**进程令牌**（`OpenProcessToken` → `GetTokenInformation(TokenUser)`），
//!   转成字符串 SID 后拼进 SDDL——不硬编码 `S-1-5-…`（不同机器不同）。
//! - **任何一步失败都退回默认 DACL**（`None`），只记一条 warn：安全加固不该让
//!   "插件跑不起来"成为它的副作用。这条 fallback 必须留着。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::platform::seh::invoke;

/// 造一份「只允许当前用户 + SYSTEM」的 `SECURITY_ATTRIBUTES`。
///
/// 返回值里的安全描述符是**需要调用方在用完后释放**的内存（`LocalFree`）——
/// 它不是 Rust 拥有的内存，所以只能靠这个成对的守卫对象回收。
///
/// 造不出来就返回 `None`（调用方退回默认 DACL，不让加固失败拖垮插件启动）。
pub fn current_user_attributes() -> Option<OwnedSecurityAttributes> {
    let sid = current_user_sid_string()?;
    // D:P = 受保护 DACL（不继承父容器的 ACE）。
    // GA = GENERIC_ALL（客户端要读写管道）；SY = SYSTEM（服务场景与调试仍可用）。
    let sddl = format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)");
    let wide = crate::platform::pipe::wide(&sddl);
    let mut sd: PSECURITY_DESCRIPTOR = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
    };
    if ok.is_err() || sd.0.is_null() {
        log::warn!("管道 DACL 造不出来（退回默认 DACL）");
        return None;
    }
    Some(OwnedSecurityAttributes {
        attributes: SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: windows::core::BOOL(0),
        },
        descriptor: sd,
    })
}

/// 持有的安全属性 + 它指向的安全描述符（两者都要在管道建好后释放）。
pub struct OwnedSecurityAttributes {
    attributes: SECURITY_ATTRIBUTES,
    descriptor: PSECURITY_DESCRIPTOR,
}

impl OwnedSecurityAttributes {
    /// 传给 `CreateNamedPipeW` 的裸指针。
    pub fn as_ptr(&self) -> *const SECURITY_ATTRIBUTES {
        &self.attributes
    }
}

impl Drop for OwnedSecurityAttributes {
    fn drop(&mut self) {
        if !self.descriptor.0.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.descriptor.0)));
            }
        }
    }
}

/// 当前用户的 SID 字符串（形如 `S-1-5-21-…-1001`）。
fn current_user_sid_string() -> Option<String> {
    invoke(|| unsafe { query_current_user_sid() }).ok()?
}

/// SID 指针归 TokenUser 缓冲所有，必须在缓冲释放前转成 owned String。
unsafe fn query_current_user_sid() -> Option<String> {
    let mut token = HANDLE(std::ptr::null_mut());
    OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
    // 先问长度，再取内容（TOKEN_USER 是变长结构）。
    let mut need: u32 = 0;
    let _ = GetTokenInformation(
        token,
        windows::Win32::Security::TokenUser,
        None,
        0,
        &mut need,
    );
    if (need as usize) < std::mem::size_of::<TOKEN_USER>() {
        let _ = windows::Win32::Foundation::CloseHandle(token);
        return None;
    }
    // Vec<u8> 不承诺 TOKEN_USER 的对齐；按结构体粒度留出变长 SID 的尾部空间。
    let count = (need as usize).div_ceil(std::mem::size_of::<TOKEN_USER>());
    let mut buf = vec![std::mem::MaybeUninit::<TOKEN_USER>::uninit(); count];
    let got = GetTokenInformation(
        token,
        windows::Win32::Security::TokenUser,
        Some(buf.as_mut_ptr() as *mut std::ffi::c_void),
        need,
        &mut need,
    );
    let _ = windows::Win32::Foundation::CloseHandle(token);
    got.ok()?;
    let user = &*(buf.as_ptr() as *const TOKEN_USER);
    let mut raw = windows::core::PWSTR(std::ptr::null_mut());
    let ok = ConvertSidToStringSidW(user.User.Sid, &mut raw);
    let s = if ok.is_ok() && !raw.is_null() {
        raw.to_string().ok()
    } else {
        None
    };
    if !raw.is_null() {
        let _ = LocalFree(Some(HLOCAL(raw.0 as *mut std::ffi::c_void)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
    use windows::Win32::Security::DACL_SECURITY_INFORMATION;

    /// 所有权 + 真跑一次：owned String 才能在令牌缓冲离开作用域后仍然有效。
    /// 这不是动态 UAF 复现（旧写法也可能"看着对"），红证只到返回类型那一层。
    #[test]
    fn sid_query_returns_owned_string() {
        fn check<T>(_: unsafe fn() -> Option<T>) {
            assert_eq!(
                std::any::type_name::<T>(),
                std::any::type_name::<String>(),
                "令牌缓冲释放前必须把 SID 转成 owned String"
            );
        }
        check(query_current_user_sid);
        // 光断类型是半条判据：真跑一次，返回值必须在令牌缓冲**已经离开作用域之后**
        // 仍然有效（owned String 才可能），且形状是一个 SID。
        let sid = unsafe { query_current_user_sid() }.expect("当前进程令牌里应有 TokenUser");
        assert!(sid.starts_with("S-1-"), "拿到的不像 SID：{sid}");
        assert!(sid.len() > 10, "SID 太短：{sid}");
    }

    /// 把安全描述符转回 SDDL 文本（测试用）。
    fn sddl_of(sd: PSECURITY_DESCRIPTOR) -> String {
        let mut out: windows::core::PWSTR = windows::core::PWSTR(std::ptr::null_mut());
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut out,
                None,
            )
        };
        assert!(ok.is_ok(), "安全描述符转 SDDL 失败");
        let s = unsafe { out.to_string() }.expect("SDDL 文本");
        unsafe {
            let _ = LocalFree(Some(HLOCAL(out.0 as *mut std::ffi::c_void)));
        }
        s
    }

    /// 当前用户的 SID 必须拿得到，且形如 `S-1-…`。
    ///
    /// 这条锁住的是"SDDL 里填的 SID 必须是真 SID"——填错会造出一份谁也连不上的
    /// DACL（插件直接起不来），而失败时本层会静默退回默认 DACL。
    #[test]
    fn current_user_sid_is_readable() {
        let sid = current_user_sid_string().expect("应能读到当前用户 SID");
        assert!(sid.starts_with("S-1-"), "SID 形状不对：{sid}");
    }

    /// 造得出 `SECURITY_ATTRIBUTES`，且长度字段对得上结构体大小。
    #[test]
    fn attributes_are_well_formed() {
        let sa = current_user_attributes().expect("应能造出安全属性");
        assert_eq!(
            sa.attributes.nLength as usize,
            std::mem::size_of::<SECURITY_ATTRIBUTES>()
        );
        assert!(!sa.descriptor.0.is_null());
    }

    /// ★ DACL 的内容必须是"只放当前用户 + SYSTEM"，**没有 Everyone**。
    ///
    /// 这是本层的全部意义：命名管道的默认 DACL 是 Everyone 可读写，同机另一个用户
    /// 猜到管道名就能连上看图片路径。测试把造出来的安全描述符转回 SDDL 文本，
    /// 断言里面能看到自己的 SID 与 SYSTEM，且看不到 Everyone（`WD`）。
    #[test]
    fn dacl_grants_current_user_and_system_only() {
        let sid = current_user_sid_string().expect("应能读到当前用户 SID");
        let sa = current_user_attributes().expect("应能造出安全属性");
        let sddl = sddl_of(sa.descriptor);
        assert!(
            sddl.contains(&sid),
            "DACL 里应含当前用户 SID {sid}，实际：{sddl}"
        );
        assert!(sddl.contains(";;;SY)"), "DACL 里应含 SYSTEM，实际：{sddl}");
        assert!(
            !sddl.contains(";;;WD)") && !sddl.contains(";;;AU)"),
            "DACL 里不该出现 Everyone / 匿名用户，实际：{sddl}"
        );
    }
}
