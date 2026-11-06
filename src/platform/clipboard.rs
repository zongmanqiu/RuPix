//! 位图 / 文件 / 文本复制到剪贴板（PNG + CF_DIBV5 + CF_DIB / CF_HDROP / CF_UNICODETEXT）。
//!
//! 「复制当前」的底层。上屏层自带的文本剪贴板（`ctx.clipboard_set`）**没有返回值**，
//! 失败也报不出所以然，所以文本也走这里：三种格式都用 `windows-rs` 直写
//! `OpenClipboard → EmptyClipboard → SetClipboardData`。
//!
//! 2026-10-19 用户口径：复制只要三种，别搞复杂——当前图像、原文件、文件路径。
//! 当前图像同时写 **PNG + CF_DIBV5 + CF_DIB**：PNG 保留 alpha，DIBV5 给只认
//! GDI 位图但又支持 alpha 的接收方，DIB 给最老式的接收方（透明像素铺白底）。
//! 只写注册格式 "PNG" 时，Photoshop/部分老应用看不到图（raw 用户报障的根因）。
//!
//! 「复制原文件」用 **CF_HDROP**（`DROPFILES` + UTF-16 双 NUL 路径）：接收方拿到的是
//! 磁盘上那个文件本身，不经过重解码，也不受当前显示档位影响。
//!
//! 输入的位图是**非预乘 RGBA8**（`core::pixels::Bitmap` 的货币），PNG 直接按 RGBA 写。

use std::path::Path;

use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BITMAPV5HEADER, BI_BITFIELDS, BI_RGB};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::DROPFILES;

/// 打开剪贴板：可能被别的进程（输入法、剪贴板管理器、正在粘贴的目标）短暂占用，
/// 小睡重试几次；仍然打不开就如实报错，**不**假装复制成功。
fn open_clipboard() -> bool {
    unsafe {
        for _ in 0..5 {
            if OpenClipboard(None).is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }
}

/// 把一段字节装进 `GMEM_MOVEABLE` 全局内存并挂到指定剪贴板格式上。
///
/// 成功后内存归剪贴板所有，**不要** GlobalFree；失败则自己回收。
/// 调用方必须已经打开剪贴板（并负责 `EmptyClipboard`）。
unsafe fn set_block(format: u32, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let hmem =
            GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(|e| format!("分配内存失败：{e}"))?;
        let ptr = GlobalLock(hmem) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hmem));
            return Err("锁定内存失败".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(hmem);
        match SetClipboardData(format, Some(HANDLE(hmem.0))) {
            Ok(_) => Ok(()),
            Err(e) => {
                let _ = GlobalFree(Some(hmem));
                Err(format!("写入剪贴板失败：{e}"))
            }
        }
    }
}

/// 把一段 UTF-8 文本写进剪贴板（CF_UNICODETEXT，带终止 NUL）。
///
/// ★ 为什么自家再写一遍而不用 `ctx.clipboard_set`：后者没有返回值，于是"复制文件路径"
/// 只能无条件 toast「已复制」——占用剪贴板的那一刻我们在报**假成功**（审计 O7，与
/// 「不许假成功」的既有规矩直接冲突）。这里把回执交回调用方，成不成各说各的话。
pub fn copy_text(s: &str) -> Result<(), String> {
    // CF_UNICODETEXT = UTF-16LE + 一个终止 NUL。
    let mut bytes = Vec::with_capacity(s.len() * 2 + 2);
    for u in s.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    unsafe {
        if !open_clipboard() {
            return Err("剪贴板被其他程序占用".into());
        }
        let result = (|| {
            EmptyClipboard().map_err(|e| format!("清空剪贴板失败：{e}"))?;
            set_block(CF_UNICODETEXT.0 as u32, &bytes)
        })();
        let _ = CloseClipboard();
        result
    }
}

/// RGBA8 → PNG 字节流。
/// 把 RGBA8 编成 PNG 字节。
///
/// ★ 压缩档写死 `Compression::Fastest`（默认档是"自适应滤波 + 常规 deflate"）：
/// 同一份真实像素、同一台机器（`examples/copy_png_cost.rs`）——
/// 1620×1080 那档 **828 ms → 25 ms**，1800×1200 **979 ms → 36 ms**，
/// 6055×6055（36.7 MP）**8 051 ms → 612 ms**；代价是 PNG 大 12–15%
/// （3.31 → 3.70 MB、125.9 → 146.7 MB）。剪贴板的消费者在乎的是像素，
/// 不是那 0.4 MB，而这一档省下的是**界面停不停**的差别。
///
/// 同一对夹具当日复测一遍：861 → 27、991 → 37、8 768 → 637 ms（前两条是
/// `exif_padded_20M.jpg` 的视口档与原图档，最后一条是 `tiff_100M.tif` 原图档）——
/// 臂间差在几个百分点内，取哪一遍都不改变结论。
/// 复现：`cargo run --release --example copy_png_cost -- <图档>`。
fn checked_len(w: u32, h: u32, rgba: &[u8]) -> Result<usize, String> {
    let (wu, hu) = (w as usize, h as usize);
    let expect = wu.checked_mul(hu).and_then(|n| n.checked_mul(4));
    if expect != Some(rgba.len()) || wu == 0 || hu == 0 {
        return Err("位图尺寸与像素长度不符".into());
    }
    Ok(wu * hu * 4)
}

/// CF_DIBV5：BITMAPV5HEADER + 自底向上的 BGRA8，保留 alpha。
fn dibv5_block(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let n = checked_len(w, h, rgba)?;
    let header = BITMAPV5HEADER {
        bV5Size: std::mem::size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: w as i32,
        bV5Height: h as i32,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: n as u32,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        bV5CSType: 0x7352_4742, // 'sRGB'
        ..Default::default()
    };
    let hs = std::mem::size_of::<BITMAPV5HEADER>();
    let mut out = vec![0u8; hs + n];
    unsafe {
        std::ptr::copy_nonoverlapping(
            &header as *const BITMAPV5HEADER as *const u8,
            out.as_mut_ptr(),
            hs,
        );
    }
    let stride = w as usize * 4;
    for y in 0..h as usize {
        let src = (h as usize - 1 - y) * stride;
        let dst = hs + y * stride;
        for x in 0..w as usize {
            let s = src + x * 4;
            let d = dst + x * 4;
            out[d] = rgba[s + 2];
            out[d + 1] = rgba[s + 1];
            out[d + 2] = rgba[s];
            out[d + 3] = rgba[s + 3];
        }
    }
    Ok(out)
}

/// CF_DIB：BITMAPINFOHEADER + 自底向上 BGRA8；透明像素铺白底（老应用没有 alpha）。
fn dib_block(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let n = checked_len(w, h, rgba)?;
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: h as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: n as u32,
        ..Default::default()
    };
    let hs = std::mem::size_of::<BITMAPINFOHEADER>();
    let mut out = vec![0u8; hs + n];
    unsafe {
        std::ptr::copy_nonoverlapping(
            &header as *const BITMAPINFOHEADER as *const u8,
            out.as_mut_ptr(),
            hs,
        );
    }
    let stride = w as usize * 4;
    for y in 0..h as usize {
        let src = (h as usize - 1 - y) * stride;
        let dst = hs + y * stride;
        for x in 0..w as usize {
            let s = src + x * 4;
            let d = dst + x * 4;
            let a = rgba[s + 3] as u32;
            let inv = 255 - a;
            out[d] = ((rgba[s + 2] as u32 * a + 255 * inv) / 255) as u8;
            out[d + 1] = ((rgba[s + 1] as u32 * a + 255 * inv) / 255) as u8;
            out[d + 2] = ((rgba[s] as u32 * a + 255 * inv) / 255) as u8;
            out[d + 3] = 255;
        }
    }
    Ok(out)
}

/// RGBA8 → PNG，**体积优先**：给「另存为」用（后台线程，用户不在等）。
///
/// 与 [`encode_png`] 两处不同，都是有意的：
/// ① `Compression::Balanced` 而不是 `Fastest`——那一档的面积代价是编码慢若干倍，
///    换来约 12–15% 的体积（同一对夹具实测：3.31 → 3.70 MB 的反向读数见
///    [`encode_png`] 的注释）。剪贴板等不起，另存等得起。
/// ② 全不透明的图存 **RGB** 而不是 RGBA：PNG 里 alpha 是实打实每像素 1 字节，
///    8 MP 图就白扔 8 MB，而照片、截图、解码器回写的位图绝大多数一个透明像素都没有。
///    逐像素扫一遍 alpha 只要几毫秒，且在后台线程里。
fn png_block_compact(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let opaque = rgba.chunks_exact(4).all(|p| p[3] == 255);
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        if opaque {
            enc.set_color(png::ColorType::Rgb);
            let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
            for px in rgba.chunks_exact(4) {
                rgb.extend_from_slice(&px[..3]);
            }
            let mut writer = enc
                .write_header()
                .map_err(|e| format!("PNG 编码失败：{e}"))?;
            writer
                .write_image_data(&rgb)
                .map_err(|e| format!("PNG 编码失败：{e}"))?;
            writer.finish().map_err(|e| format!("PNG 编码失败：{e}"))?;
        } else {
            enc.set_color(png::ColorType::Rgba);
            let mut writer = enc
                .write_header()
                .map_err(|e| format!("PNG 编码失败：{e}"))?;
            writer
                .write_image_data(rgba)
                .map_err(|e| format!("PNG 编码失败：{e}"))?;
            writer.finish().map_err(|e| format!("PNG 编码失败：{e}"))?;
        }
    }
    Ok(out)
}

/// 「另存为 PNG」：体积优先的那一档（见 [`png_block_compact`]）。
pub fn encode_png_compact(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (wu, hu) = (w as usize, h as usize);
    let expect = wu.checked_mul(hu).and_then(|n| n.checked_mul(4));
    if expect != Some(rgba.len()) || wu == 0 || hu == 0 {
        return Err("位图尺寸与像素长度不符".into());
    }
    png_block_compact(w, h, rgba)
}

fn png_block(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fastest);
        let mut writer = enc
            .write_header()
            .map_err(|e| format!("PNG 编码失败：{e}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| format!("PNG 编码失败：{e}"))?;
        writer.finish().map_err(|e| format!("PNG 编码失败：{e}"))?;
    }
    Ok(out)
}

/// 组装 CF_HDROP（`DROPFILES` + UTF-16 路径 + 双 NUL）。
fn hdrop_block(path: &Path) -> Option<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() {
        return None;
    }
    let header = std::mem::size_of::<DROPFILES>();
    // 路径 + 双 NUL 终止（CF_HDROP 要求列表以空串结尾）。
    let mut out = vec![0u8; header + (wide.len() + 2) * 2];
    let df = DROPFILES {
        pFiles: header as u32,
        pt: Default::default(),
        fNC: windows::core::BOOL(0),
        fWide: windows::core::BOOL(1),
    };
    // SAFETY: `out` 长度已含完整头部与 UTF-16 区。
    unsafe {
        std::ptr::copy_nonoverlapping(
            &df as *const DROPFILES as *const u8,
            out.as_mut_ptr(),
            header,
        );
        std::ptr::copy_nonoverlapping(
            wide.as_ptr() as *const u8,
            out.as_mut_ptr().add(header),
            wide.len() * 2,
        );
    }
    Some(out)
}

/// 把一张非预乘 RGBA8 位图编码成 PNG 并复制进剪贴板（一步做完，小图那条路）。
///
/// `rgba` 长度须为 `w * h * 4`。PNG 自带 alpha，透明区域原样保留。
///
/// 大图不许走这一句：编码那一段要在调用线程上跑（36.7 MP 实测 0.6 秒），
/// 那条路改成分两步 [`encode_png`] → [`put_png`]，把编码交给后台线程。
pub fn copy_png(w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    put_png(&encode_png(w, h, rgba)?, w, h, rgba)
}

/// 只做 PNG 编码，**不碰剪贴板**。
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (wu, hu) = (w as usize, h as usize);
    let expect = wu.checked_mul(hu).and_then(|n| n.checked_mul(4));
    if expect != Some(rgba.len()) || wu == 0 || hu == 0 {
        return Err("位图尺寸与像素长度不符".into());
    }
    png_block(w, h, rgba)
}

/// 把已经编好的 PNG 字节放进剪贴板，同时挂 CF_DIBV5 / CF_DIB 兼容老接收方。
pub fn put_png(png: &[u8], w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    // "PNG" 是事实标准名（Chrome/Office/截图工具都按它取图）；注册失败说明系统
    // 资源耗尽，如实报错而不是假装复制成功。
    let format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    if format == 0 {
        return Err("注册 PNG 剪贴板格式失败".into());
    }
    // DIB 是可选兼容层：PNG 成功就算复制成功；DIB 写不进去只记 warn，不让用户
    // 因为一个老格式的分配失败而看不到“已经复制好了”的回执。
    let dibv5 = dibv5_block(w, h, rgba);
    let dib = dib_block(w, h, rgba);
    unsafe {
        if !open_clipboard() {
            return Err("剪贴板被其他程序占用".into());
        }
        let result = (|| {
            EmptyClipboard().map_err(|e| format!("清空剪贴板失败：{e}"))?;
            set_block(format, png)?;
            match dibv5 {
                Ok(bytes) => {
                    if let Err(e) = set_block(CF_DIBV5.0 as u32, &bytes) {
                        log::warn!("写 CF_DIBV5 失败（PNG 已在剪贴板）：{e}");
                    }
                }
                Err(e) => log::warn!("编码 CF_DIBV5 失败（PNG 已在剪贴板）：{e}"),
            }
            match dib {
                Ok(bytes) => {
                    if let Err(e) = set_block(CF_DIB.0 as u32, &bytes) {
                        log::warn!("写 CF_DIB 失败（PNG 已在剪贴板）：{e}");
                    }
                }
                Err(e) => log::warn!("编码 CF_DIB 失败（PNG 已在剪贴板）：{e}"),
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// 把磁盘上的文件本身复制进剪贴板（CF_HDROP）——粘贴到资源管理器/邮件里就是那个文件。
///
/// 与 [`copy_png`] 的分工：这里不重解码、不受当前显示尺寸影响；
/// 只对真实存在的文件调用（调用方按 `current_path` 判断）。
pub fn copy_file(path: &Path) -> Result<(), String> {
    let bytes = hdrop_block(path).ok_or_else(|| "文件路径为空".to_string())?;
    unsafe {
        if !open_clipboard() {
            return Err("剪贴板被其他程序占用".into());
        }
        let result = (|| {
            EmptyClipboard().map_err(|e| format!("清空剪贴板失败：{e}"))?;
            set_block(CF_HDROP.0 as u32, &bytes)
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「另存为」的体积账：全不透明的图不该白带 alpha 通道。
    ///
    /// PNG 的 IHDR 第 9 个数据字节就是 color type（偏移 25），2 = RGB、6 = RGBA。
    /// 这条不看"文件小了多少"（那依赖图像内容），只看**通道数真的少了**。
    #[test]
    fn compact_save_drops_the_alpha_channel_when_nothing_is_transparent() {
        let opaque = [1u8, 2, 3, 255, 4, 5, 6, 255];
        let bytes = encode_png_compact(2, 1, &opaque).expect("编码成功");
        assert_eq!(bytes[25], 2, "全不透明 ⇒ color type 2（RGB）");

        let with_alpha = [1u8, 2, 3, 128, 4, 5, 6, 255];
        let bytes = encode_png_compact(2, 1, &with_alpha).expect("编码成功");
        assert_eq!(bytes[25], 6, "有透明像素 ⇒ color type 6（RGBA）");
    }

    /// PNG 输出带标准签名、IHDR 宽高与像素一致，且解码回来的像素就是输入的 RGBA。
    #[test]
    fn png_round_trips_rgba() {
        let rgba = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let bytes = png_block(2, 2, &rgba).expect("编码成功");
        assert_eq!(&bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10], "PNG 签名");
        assert_eq!(&bytes[12..16], b"IHDR");
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        assert_eq!((w, h), (2, 2));

        let mut decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().expect("解头");
        let mut out = vec![0u8; reader.output_buffer_size().expect("解头后有缓冲区大小")];
        let info = reader.next_frame(&mut out).expect("解码");
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&out[..info.buffer_size()], &rgba);
    }

    /// ★ 那行 `Compression::Fastest` 到底有没有落到字节上——**不许只用"跑起来快了"当证据**。
    ///
    /// zlib 流头的第二个字节里 FLEVEL（bit 6–7）就是 deflate 自己声明的档位；把它和
    /// 同一份像素按默认档（`Balanced`）编出来的那一位比：**我们这一档必须更低**。
    /// 删掉 `set_compression` 那行，两边就相等，本用例立刻红——这条判据不是装饰，
    /// 因为剪贴板的输出**内容仍然逐字节合法**（上一条用例照样过），只有档位能抓住。
    #[test]
    fn png_carries_the_fast_deflate_level() {
        fn flevel(png: &[u8]) -> u8 {
            let mut i = 8usize; // 8 字节签名
            while i + 12 <= png.len() {
                let len = u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as usize;
                let typ = &png[i + 4..i + 8];
                let data = &png[i + 8..i + 8 + len];
                if typ == b"IDAT" {
                    assert!(len >= 2, "IDAT 至少要有 zlib 头两字节");
                    return (data[1] & 0xC0) >> 6;
                }
                i += 12 + len;
            }
            panic!("没有 IDAT 块");
        }
        // 一幅会真压缩的图（渐变 + 斜线），别用纯色——纯色两边都可能走最短路径。
        let (w, h) = (64u32, 64u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 4;
                let v = ((x * 4 + y * 7) % 251) as u8;
                rgba[i] = v;
                rgba[i + 1] = 255 - v;
                rgba[i + 2] = if (x + y) % 3 == 0 { 200 } else { 40 };
                rgba[i + 3] = 255;
            }
        }
        let ours = png_block(w, h, &rgba).expect("编码成功");
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Balanced);
            let mut writer = enc.write_header().expect("写头");
            writer.write_image_data(&rgba).expect("写数据");
            writer.finish().expect("收尾");
        }
        let balanced = flevel(&out);
        assert!(
            flevel(&ours) < balanced,
            "剪贴板那档必须真的比默认档快：ours={} balanced={}",
            flevel(&ours),
            balanced
        );
    }

    /// DIBV5：头字段正确，像素自底向上（正高度 = bottom-up），alpha 原样保留。
    #[test]
    fn dibv5_header_and_pixels_are_bottom_up() {
        let rgba = [
            255, 0, 0, 255, 255, 0, 0, 255, // 顶行红
            0, 0, 255, 255, 0, 0, 255, 255, // 底行蓝
        ];
        let bytes = dibv5_block(2, 2, &rgba).expect("DIBV5");
        let hs = std::mem::size_of::<BITMAPV5HEADER>();
        assert_eq!(&bytes[..4], &(hs as u32).to_ne_bytes());
        let h = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const BITMAPV5HEADER) };
        assert_eq!((h.bV5Width, h.bV5Height), (2, 2));
        assert_eq!(h.bV5BitCount, 32);
        assert_eq!(h.bV5Compression, BI_BITFIELDS);
        assert_eq!(&bytes[hs..hs + 4], &[255, 0, 0, 255], "底行是蓝（BGR）");
        assert_eq!(&bytes[hs + 8..hs + 12], &[0, 0, 255, 255], "顶行是红");
    }

    /// 老式 CF_DIB 没有 alpha：透明像素铺白底，别让接收方看到黑块。
    #[test]
    fn dib_composites_transparency_on_white() {
        let bytes = dib_block(1, 1, &[10, 20, 30, 0]).expect("DIB");
        let hs = std::mem::size_of::<BITMAPINFOHEADER>();
        assert_eq!(&bytes[hs..hs + 4], &[255, 255, 255, 255]);
    }

    /// CF_HDROP 的 DROPFILES 头：pFiles 指向路径区、fWide=1、路径以双 NUL 结尾。
    #[test]
    fn hdrop_points_at_wide_path_with_double_nul() {
        let bytes = hdrop_block(Path::new("C:\\tmp\\a.png")).expect("非空路径");
        // DROPFILES 是 packed(1)：按值读，不建字段引用（unaligned reference 是 UB）。
        let df = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const DROPFILES) };
        let p_files = df.pFiles;
        let f_wide = df.fWide;
        assert_eq!(p_files as usize, std::mem::size_of::<DROPFILES>());
        assert_eq!(f_wide, windows::core::BOOL(1));
        let tail = &bytes[p_files as usize..];
        assert_eq!(tail.len(), ("C:\\tmp\\a.png".len() + 2) * 2);
        assert_eq!(&tail[tail.len() - 4..], &[0, 0, 0, 0], "双 NUL 结尾");
    }
}
