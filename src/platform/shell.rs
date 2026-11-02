//! 把一小段东西交给**系统默认程序**打开（URL 或宿主自己刚写的临时文件）。
//!
//! ★ 边界都在调用方：URL 只认 `http(s)`（`is_openable_url`）；图片只认宿主自己
//!   按文件头写进 `<exe>/temp/` 的那一枚（`save_temp_image` 会**当场复核 magic**，
//!   插件给什么扩展名都不作数）。这里只负责"把 target 交给 ShellExecute"，
//!   用哪个程序打开是系统关联的事——不是我们的事。
//!
//! ★ 为什么由宿主开而不是插件自己 `ShellExecute`：打开 URL/文件是**用户资产**，
//!   属于"宿主独占"的对外副作用（见 `docs/扩展体系.md` §四）。宿主开能保证：
//!   只认 http(s)、只碰自己的 temp、并且有事可记日志。

use std::path::PathBuf;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// 这个 URL 能不能交给系统打开：只认 http/https，不许任何控制字符或引号。
pub fn is_openable_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    !url.chars().any(|c| c.is_control() || c == '"')
}

/// 让系统用默认关联打开 `target`。
pub fn open_target(target: &str) -> Result<(), String> {
    let verb = HSTRING::from("open");
    let file = HSTRING::from(target);
    // SAFETY: 两个 HSTRING 在调用期间存活；父窗口传 None（不模态、不依赖某个窗口）。
    let rc = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 的返回值 > 32 才算成功（Win32 的老约定）。
    if rc.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "系统没能打开（ShellExecute 返回 {}）",
            rc.0 as isize
        ))
    }
}

/// 按文件头认出光栅图；认不出返回 `None`（那就**不打开**，只记日志）。
pub fn image_ext_of(bytes: &[u8]) -> Option<&'static str> {
    let has =
        |i: usize, sig: &[u8]| bytes.len() >= i + sig.len() && &bytes[i..i + sig.len()] == sig;
    if has(0, b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if has(0, &[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if has(0, b"GIF87a") || has(0, b"GIF89a") {
        return Some("gif");
    }
    if has(0, b"BM") {
        return Some("bmp");
    }
    if has(0, b"RIFF") && has(8, b"WEBP") {
        return Some("webp");
    }
    if has(0, b"II*\0") || has(0, b"MM\0*") {
        return Some("tiff");
    }
    if has(0, &[0, 0, 1, 0]) {
        return Some("ico");
    }
    None
}

/// 把一段图片字节写进 `<exe>/temp/barcode-<毫秒>.<ext>`，返回落点。
///
/// **扩展名以文件头为准**：插件给的写法只当提示，magic 认不出就直接拒——
/// 让系统按错误后缀去关联程序，等于替用户点了一个不该点的东西。
pub fn save_temp_image(claimed_ext: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err("图片载荷是空的".into());
    }
    let Some(ext) = image_ext_of(bytes) else {
        return Err(format!(
            "载荷不是可识别的光栅图（插件说它是 {claimed_ext}）"
        ));
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = crate::core::paths::crash_log_path(&format!("barcode-{stamp}.{ext}"))
        .ok_or("取不到 exe 同级 temp 目录")?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, bytes).map_err(|e| format!("写临时图片失败：{e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_are_openable() {
        assert!(is_openable_url("https://example.invalid/a"));
        assert!(is_openable_url("HTTP://example.invalid"));
        // 其余方案、相对路径、带控制字符的一律不认。
        assert!(!is_openable_url("file:///c:/windows/notepad.exe"));
        assert!(!is_openable_url("javascript:alert(1)"));
        assert!(!is_openable_url("example.invalid"));
        assert!(!is_openable_url("https://a\u{0}b"));
    }

    #[test]
    fn image_ext_follows_the_magic_not_the_claim() {
        assert_eq!(image_ext_of(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(image_ext_of(&[0xFF, 0xD8, 0xFF, 0]), Some("jpg"));
        assert_eq!(image_ext_of(b"not an image"), None);
        // 插件说 png、实际不是：拒，而不是写一个后缀骗系统。
        assert!(save_temp_image("png", b"plain text").is_err());
    }
}
