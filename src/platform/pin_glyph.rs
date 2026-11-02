//! 置顶图钉的离屏 GDI 渲染引擎（纯 Win32，不依赖 windui）。
//!
//! 与 CurSeen 便签/剪贴板标题栏同款语义：📌（U+1F4CC）字形在 Segoe UI Symbol 里
//! **天生斜躺**（针头朝左下）——未置顶用原生姿态；已置顶绕按钮中心视觉逆时针
//! 转 45° 立正（针头朝下），旋转前 rect 上移 3px 补偿斜躺字形的视觉重心
//! （参数逐条照抄 CurSeen `sticky.rs`，用户实测过）。
//!
//! windui `Canvas` 没有文字旋转 API（`TextStyle` 无 rotation、`draw_image` 不转），
//! CurSeen 的 GDI `SetWorldTransform` 直接画在窗口 DC 上——这边 paint 只有
//! skia canvas、没有窗口 DC 可拿，故改为「离屏 DIB 渲染 → RGBA 字节」。
//!
//! alpha 提取：GDI 往 32bpp DIB 写像素时不写 alpha 字节，故背景预填品红
//! （G=0）、字形画纯白——绿通道恰好是覆盖度（`ANTIALIASED_QUALITY` 灰度 AA），
//! 后处理 `a = G`、RGB 统一刷成目标色（AA 边的 RGB 会被品红污染）。
//!
//! 本模块只产出非预乘 RGBA8 字节（`SIZE×SIZE`），由上层（`ui::pin`）转成 windui `Image`。
//! 之所以放在 `platform` 而非 `ui`：渲染过程全程调用 `windows` 的 GDI API，
//! 而 ui 层按五层门禁**禁止**引 `windows`——平台能力一律经 platform 层取用。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
    SelectObject, SetBkMode, SetGraphicsMode, SetTextColor, SetWorldTransform, ANTIALIASED_QUALITY,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_NORMAL, GM_ADVANCED,
    HGDIOBJ, OUT_TT_PRECIS, TRANSPARENT, XFORM,
};

/// 渲染超采样：windui 贴图时按 DPI 采样，3× 超采保证高分屏不糊。
const SUPER: u32 = 3;
/// 字形字号（逻辑 px）：与 CurSeen `symbol_font(17)` 一致。
const PIN_PX: i32 = 17;
/// DIB 边长（逻辑 px）：字形 em 17 + 旋转包围盒/重心余量 10。
const SIZE_LOGICAL: i32 = PIN_PX + 10;
/// DIB 边长（物理 px）。ui 层据此把字节包成 `SIZE×SIZE` 的 `Image`。
pub const SIZE: u32 = (SIZE_LOGICAL as u32) * SUPER;

/// 渲染一个姿态的图钉为非预乘 RGBA8（`SIZE×SIZE`）。GDI 不可用时返回 `None`。
///
/// `upright=false` = 斜躺（字形原生姿态，未置顶）；`true` = 立正 + 上移补偿（已置顶）。
/// 颜色以裸 RGB 传入——本模块刻意不依赖 windui 的 `Color`，把 ui 层挡在 `windows` 禁区外。
pub fn render_pin_rgba(upright: bool, r: u8, g: u8, b: u8) -> Option<Vec<u8>> {
    // SAFETY: 纯 GDI 离屏渲染：不碰窗口、不触全局状态；所有取得的 GDI 句柄
    // 在任一失败分支与正常出口都成对释放（DC 与位图按 DeleteDC→DeleteObject 顺序）。
    unsafe { render(upright, r, g, b) }
}

/// # Safety
///
/// 见 [`render_pin_rgba`] 的 SAFETY 说明。
unsafe fn render(upright: bool, r: u8, g: u8, b: u8) -> Option<Vec<u8>> {
    let hdc = CreateCompatibleDC(None);
    if hdc.is_invalid() {
        return None;
    }

    // top-down 32bpp DIB；bits 先预填品红 BGRA（B=FF G=00 R=FF A=00）。
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: SIZE as i32,
        biHeight: -(SIZE as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let Ok(hbmp) = CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
        let _ = DeleteDC(hdc);
        return None;
    };
    if bits.is_null() {
        let _ = DeleteDC(hdc);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        return None;
    }
    let old = SelectObject(hdc, HGDIOBJ(hbmp.0));
    let size = (SIZE * SIZE * 4) as usize;
    // 预填品红：只借一个**短命**的可变引用写初始化图案，让它在本块结束就死掉。
    //★ 绝不能把 `&mut` 横跨下面的 GDI 绘制：字形像素是 GDI 经 `hdc` 写进**同一块**
    //  DIB 内存的，而 `&mut` 在 LLVM 眼里是 noalias——优化器有权认定没人动过它，
    //  于是把这里的预填值直接折进读回阶段的读，表现是图钉整幅全透明（置顶图标消失）。
    //  读回改用裸指针另建的只读切片。debug 回归测试抓不到这一类（只在 release 成立）。
    {
        let prefill = std::slice::from_raw_parts_mut(bits as *mut u8, size);
        for px in prefill.chunks_exact_mut(4) {
            px.copy_from_slice(&[0xFF, 0x00, 0xFF, 0x00]);
        }
    }

    // Segoe UI Symbol：📌 在此字体是单色轮廓字形，颜色跟 SetTextColor 走。
    let face: Vec<u16> = "Segoe UI Symbol\0".encode_utf16().collect();
    let font = CreateFontW(
        -(PIN_PX * SUPER as i32),
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_TT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        (DEFAULT_PITCH.0 as u32) | (FF_DONTCARE.0 as u32),
        PCWSTR(face.as_ptr()),
    );
    if font.is_invalid() {
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteDC(hdc);
        return None;
    }
    let old_font = SelectObject(hdc, HGDIOBJ(font.0));
    let _ = SetBkMode(hdc, TRANSPARENT);
    // 字形画纯白：后处理把绿通道提成 alpha（白 = 满覆盖）。
    let _ = SetTextColor(hdc, COLORREF(0x00FF_FFFF));

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: SIZE as i32,
        bottom: SIZE as i32,
    };
    if upright {
        // 立正姿态的视觉重心比排版中心偏下 3 逻辑 px（CurSeen 用户实测），
        // 旋转前先把排版 rect 上移。
        let shift = 3 * SUPER as i32;
        rect.top -= shift;
        rect.bottom -= shift;
        // 绕 rect 中心做视觉逆时针 45°：M = T·R·T⁻¹
        //（y 向下坐标系：eM12 取负才是视觉逆时针，CurSeen 同式）。
        let f = std::f32::consts::FRAC_1_SQRT_2;
        let (fx, fy) = (rect.left as f32, rect.top as f32);
        let (fw, fh) = (
            (rect.right - rect.left) as f32,
            (rect.bottom - rect.top) as f32,
        );
        let (mx, my) = (fx + fw / 2.0, fy + fh / 2.0);
        let xf = XFORM {
            eM11: f,
            eM12: -f,
            eM21: f,
            eM22: f,
            eDx: mx - f * mx - f * my,
            eDy: my + f * mx - f * my,
        };
        let _ = SetGraphicsMode(hdc, GM_ADVANCED);
        let _ = SetWorldTransform(hdc, &xf);
    }
    let mut glyph = [0u16; 2];
    let n = '\u{1F4CC}'.encode_utf16(&mut glyph).len();
    let _ = DrawTextW(
        hdc,
        &mut glyph[..n],
        &mut rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    if upright {
        let identity = XFORM {
            eM11: 1.0,
            eM12: 0.0,
            eM21: 0.0,
            eM22: 1.0,
            eDx: 0.0,
            eDy: 0.0,
        };
        let _ = SetWorldTransform(hdc, &identity);
    }

    let _ = SelectObject(hdc, old_font);
    let _ = DeleteObject(HGDIOBJ(font.0));
    let _ = SelectObject(hdc, old);

    // a = 绿通道（覆盖度），RGB = 目标色（抹掉品红污染）。
    //★ 必须在 DeleteObject(hbmp) **之前**读 bits——位图一删，DIB 内存随之释放。
    //  切片在这里**重新构造**（只读），不复用绘制前的任何借用（见上面的 noalias 说明）。
    let drawn = std::slice::from_raw_parts(bits as *const u8, size);
    let mut out = Vec::with_capacity(size);
    for px in drawn.chunks_exact(4) {
        let a = px[1];
        out.extend_from_slice(&[r, g, b, if a < 8 { 0 } else { a }]);
    }

    let _ = DeleteDC(hdc);
    let _ = DeleteObject(HGDIOBJ(hbmp.0));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两姿态位图：有可见字形、姿态确实不同（旋转真的生效）、绿色版字形像素是绿系。
    #[test]
    fn pin_bitmaps_render_both_postures() {
        let tilted = render_pin_rgba(false, 208, 208, 208).expect("GDI 斜躺渲染失败");
        let upright = render_pin_rgba(true, 208, 208, 208).expect("GDI 立正渲染失败");
        assert_eq!(tilted.len(), (SIZE * SIZE * 4) as usize);
        let count = |v: &[u8]| v.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(
            count(&tilted) > 100,
            "斜躺位图应有可见字形，实际 {} 像素",
            count(&tilted)
        );
        assert!(count(&upright) > 100);
        assert_ne!(tilted, upright, "两姿态位图应不同（旋转真的生效）");
        let green = render_pin_rgba(true, 126, 201, 126).expect("GDI 绿色渲染失败");
        let green_px = green
            .chunks_exact(4)
            .filter(|p| p[3] > 200 && p[1] > p[0] && p[1] > p[2])
            .count();
        assert!(green_px > 30, "立正绿位图应含目标绿色像素，实际 {green_px}");
    }
}
