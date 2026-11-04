//! 把一小段东西交给**系统默认程序**打开（URL），或替调用方把图片字节落进 temp。
//!
//! ★ 边界都在调用方：URL 只认 `http(s)`（`is_openable_url`，走 `open_target` 交系统
//!   默认浏览器）；图片只认宿主自己按文件头写进 `<exe>/temp/` 的那一枚
//!   （`save_temp_image` 会**当场复核 magic**，插件给什么扩展名都不作数）。
//!   ★ 2026-11-04 起图片**不再**交系统关联打开：本机把 `.png` 关联到 RuPix 自己时，
//!   二次启动被单实例收走、没开多开就"当前窗换图"，用户正在看的图被插件产物顶掉。
//!   现在落盘后由调用方（runtime）开**新看图窗口**显示，与多开开关无关。
//!
//! ★ 为什么落盘由宿主做而不是插件自己写文件：写盘是**用户资产**副作用，属于"宿主
//!   独占"（见 `docs/扩展体系.md` §四）。宿主能保证：只碰自己的 temp、按文件头复核、
//!   有事可记日志。

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

/// 临时图片的落点：`<exe>/temp/barcode-<毫秒>.<ext>`。
///
/// ★ **不许借 `crash_log_path` 拼这个名字**：那个函数会加 `crash-` 前缀与 `.log`
///   后缀，系统按 `.log` 的文本关联打开，用户看到的会是一个文本编辑器里的二进制乱码
///   （K195）。图片必须以**真实图片后缀**落地，后续无论谁打开它都走图片路径。
///   打开方式（宿主开新窗）由调用方决定，这里只负责把字节安全落盘。
fn temp_image_path(stamp: u128, ext: &str) -> Option<PathBuf> {
    Some(crate::core::paths::diagnostics_dir()?.join(format!("barcode-{stamp}.{ext}")))
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
    let path = temp_image_path(stamp, ext).ok_or("取不到 exe 同级 temp 目录")?;
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

    #[test]
    fn temp_image_lands_with_a_real_image_suffix() {
        // K195 回归钉：曾借 crash_log_path 拼出 crash-barcode-<ms>.png.log，
        // 系统按 .log 的文本关联打开，图形看不见。
        let p = temp_image_path(1_793_782_074_815, "png").expect("诊断目录路径");
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("png"),
            "落盘后缀必须是图片头认出的那一个：{}",
            p.display()
        );
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name.starts_with("barcode-"), "{name}");
        assert!(!name.contains("crash-"), "{name}");
        assert!(!name.contains(".log"), "{name}");
        // 负向：插件声明的后缀不许参与落盘名，只认 magic。
        let ext = image_ext_of(b"\x89PNG\r\n\x1a\nrest").expect("PNG 头");
        let q = temp_image_path(1, ext).expect("诊断目录路径");
        assert_eq!(q.extension().and_then(|e| e.to_str()), Some("png"));
    }
}
