//! ICC → sRGB 上屏转换（Windows `mscms.dll`）。
//!
//! 解码器把文件里嵌的 ICC profile 原样交上来（ABI 的 `icc_len` + 像素尾段），本模块
//! 在**缩到显示尺寸之后**把像素转进 sRGB——成本跟视口走，不跟原图字节数走。
//!
//! 为什么走 `mscms.dll` 原始 FFI 而不是加依赖：windows 0.62 没有 ColorManagement
//! feature（`windows` crate 不绑定 mscms），而 lcms2 会把 exe 抬大一截；系统自带的
//! CMM 在每台 Windows 上都有，几十行绑定即可。渲染意图固定**感知**，输出固定 sRGB：
//! 本轮不提供"选意图 / 跟显示器 profile"的开关（见 m53 §六）。
//!
//! 失败语义：任何一步失败都返回 `Err`，**调用方保留原像素**并记 warn——颜色不准
//! 好过整张图打不开。
//!
//! 「保留原像素」有代价边界，这里说清：`TranslateBitmapBits` 分块跑、一块写完才下一块，
//! 失败只可能是两种——①**确定性**失败（坏 profile / 变换句柄失效 / CMM 不可用），它在
//! 任何一块上都会失败；②非确定性失败（同一枚变换前几块成功、某一块忽然不行，只有
//! 瞬时资源耗尽之类能解释）。①由本模块那次**首行干跑**接住 ⇒ 一块未写就已返回 `Err`；
//! ②要回滚必须留一份整图副本，而这条链上的位图可能是**原图档**（`(0, 0)` 那趟升级不
//! 缩，见 `fit_dims` 的「0 = 不限」），翻倍峰值是几百 MB 量级——不为一条没见过的路径
//! 付这个钱。故承诺按两档写：**确定性失败保证原样，非确定性失败留下部分已转换的行**，
//! 调用方的 warn 里按实际语义说清楚。

use std::ffi::c_void;

use windows::core::BOOL;

use crate::core::pixels::Bitmap;

/// `LCS_sRGB`（`'sRGB'`）。mscms 的 `GetStandardColorSpaceProfileW` 用它取系统 sRGB。
const SCS_SRGB: u32 = 0x7352_4742;
/// `PROFILE_MEMBUFFER`：`PROFILE.pProfileData` 指向内存里的 profile 字节。
const PROFILE_MEMBUFFER: u32 = 2;
/// `PROFILE_FILENAME`：`pProfileData` 指向 UTF-16 路径。
const PROFILE_FILENAME: u32 = 1;
const PROFILE_READ: u32 = 1;
const FILE_SHARE_READ: u32 = 1;
const OPEN_EXISTING: u32 = 3;
/// `BEST_MODE`：让 CMM 自己挑最快的合格路径。
const BEST_MODE: u32 = 3;
/// `INTENT_PERCEPTUAL`。
const INTENT_PERCEPTUAL: u32 = 0;
/// `BM_xBGRQUADS`：32bpp，位序 x,B,G,R ⇒ 小端内存字节序 R,G,B,x（与我们的 RGBA 一致；
/// 最高的 x 字节忽略，alpha 另存另还原）。
///
/// 不能用 `BM_xRGBQUADS`（0x0008）：它的内存字节序是 B,G,R,x，会把红蓝对调。
/// 这个错在"同基色只改 gamma"的样本上看不见——那是逐通道对角变换，与对调可交换；
/// 只有异基色 profile（ProPhoto 型）才暴露。
const BM_XBGRQUADS: u32 = 0x0010;
/// 分块转换的暂存上限（1 MiB）。4K 图一整张 RGBA 是 33 MB，分块后峰值只跟块高走。
const CHUNK_BYTES: usize = 1 << 20;

/// `tagPROFILE`（与 `Icm.h` 同布局）。
#[repr(C)]
struct Profile {
    dw_type: u32,
    p_data: *const c_void,
    cb_size: u32,
}

type HProfile = *mut c_void;
type HTransform = *mut c_void;

#[link(name = "mscms")]
unsafe extern "system" {
    fn GetStandardColorSpaceProfileW(
        machine: *const u16,
        scs: u32,
        buffer: *mut u16,
        size: *mut u32,
    ) -> BOOL;
    fn OpenColorProfileW(
        profile: *const Profile,
        desired_access: u32,
        share_mode: u32,
        creation: u32,
    ) -> HProfile;
    fn CloseColorProfile(profile: HProfile) -> BOOL;
    fn CreateMultiProfileTransform(
        profiles: *const HProfile,
        count: u32,
        intents: *const u32,
        intent_count: u32,
        flags: u32,
        preferred_cmm: u32,
    ) -> HTransform;
    fn DeleteColorTransform(transform: HTransform) -> BOOL;
    fn TranslateBitmapBits(
        transform: HTransform,
        src: *const c_void,
        input_format: u32,
        width: u32,
        height: u32,
        input_stride: u32,
        dst: *mut c_void,
        output_format: u32,
        output_stride: u32,
        callback: *const c_void,
        callback_data: usize,
    ) -> BOOL;
}

/// 打开的 profile 句柄（Drop 时关闭）。
struct ColorProfile(HProfile);

impl Drop for ColorProfile {
    fn drop(&mut self) {
        // SAFETY: 句柄由 OpenColorProfileW 产生，且只在这里关闭一次。
        unsafe {
            let _ = CloseColorProfile(self.0);
        }
    }
}

/// 颜色变换句柄（Drop 时删除；必须早于 profile 关闭，故声明在 profile 之后）。
struct ColorTransform(HTransform);

impl Drop for ColorTransform {
    fn drop(&mut self) {
        // SAFETY: 句柄由 CreateMultiProfileTransform 产生，且只在这里删除一次。
        unsafe {
            let _ = DeleteColorTransform(self.0);
        }
    }
}

/// 系统 sRGB profile 的路径（UTF-16，带 NUL）。取不到就报错。
fn system_srgb_path() -> Result<Vec<u16>, String> {
    let mut buf = vec![0u16; 512];
    let mut size = buf.len() as u32;
    // SAFETY: 缓冲区长度随 `size` 一并交给 API；失败时不读取。
    let ok = unsafe {
        GetStandardColorSpaceProfileW(std::ptr::null(), SCS_SRGB, buf.as_mut_ptr(), &mut size)
    };
    if !ok.as_bool() || size == 0 || size as usize > buf.len() {
        return Err("取系统 sRGB 色彩配置失败".into());
    }
    // API 回的是字符数（含 NUL）；不同版本可能不含，统一补齐。
    buf.truncate(size as usize);
    while buf.last().is_some_and(|&u| u != 0) {
        buf.push(0);
    }
    Ok(buf)
}

/// 把一张非预乘 RGBA8 位图按文件自带 ICC 转成 sRGB（原地改写 RGB，alpha 原样）。
///
/// `icc` 必须是完整 profile（解码器已按段拼好）。返回 `Err` 时：确定性失败（坏 profile /
/// 变换句柄失效 / CMM 不可用）保证一块未写、`bitmap` 仍是原像素；非确定性失败（跑到中途
/// 忽然不行）会留下部分已转换的行，理由见模块头。两种都只走同一条 `Err` 通道——调用方
/// 无需区分，只要知道"原样"这一承诺只保证到确定性失败那一档。
pub fn to_srgb(bitmap: &mut Bitmap, icc: &[u8]) -> Result<(), String> {
    if icc.len() < 128 {
        return Err(format!("ICC profile 太短（{} 字节）", icc.len()));
    }
    let (w, h) = (bitmap.width, bitmap.height);
    if w == 0 || h == 0 {
        return Ok(());
    }
    let stride = w as usize * 4;
    if bitmap.rgba.len() != stride * h as usize {
        return Err("位图尺寸与像素长度不符".into());
    }

    let srgb_path = system_srgb_path()?;

    // 源 profile 在内存里，目标 profile 是系统文件。
    let src_mem = Profile {
        dw_type: PROFILE_MEMBUFFER,
        p_data: icc.as_ptr().cast(),
        cb_size: icc.len() as u32,
    };
    let dst_mem = Profile {
        dw_type: PROFILE_FILENAME,
        p_data: srgb_path.as_ptr().cast(),
        cb_size: (srgb_path.len() * 2) as u32,
    };
    // SAFETY: 两个 PROFILE 的指针分别指向 `icc` 与 `srgb_path`，生命周期覆盖整个调用；
    // 句柄由 RAII 关闭，transform 比 profile 先析构。
    let src = unsafe { OpenColorProfileW(&src_mem, PROFILE_READ, FILE_SHARE_READ, OPEN_EXISTING) };
    if src.is_null() {
        return Err(format!(
            "打开源色彩配置失败（{}）",
            std::io::Error::last_os_error()
        ));
    }
    let _src = ColorProfile(src);
    let dst = unsafe { OpenColorProfileW(&dst_mem, PROFILE_READ, FILE_SHARE_READ, OPEN_EXISTING) };
    if dst.is_null() {
        return Err(format!(
            "打开 sRGB 色彩配置失败（{}）",
            std::io::Error::last_os_error()
        ));
    }
    let _dst = ColorProfile(dst);

    let handles = [src, dst];
    let intents = [INTENT_PERCEPTUAL];
    // SAFETY: 句柄数组与意图数组长度都由参数显式给出。
    let transform = unsafe {
        CreateMultiProfileTransform(
            handles.as_ptr(),
            handles.len() as u32,
            intents.as_ptr(),
            intents.len() as u32,
            BEST_MODE,
            0,
        )
    };
    if transform.is_null() {
        return Err(format!(
            "创建颜色变换失败（{}）",
            std::io::Error::last_os_error()
        ));
    }
    let _transform = ColorTransform(transform);

    // 分块：暂存一行块（外加原 alpha 备份），峰值与图高无关。
    let rows = (CHUNK_BYTES / stride).clamp(1, h as usize);
    let mut scratch = vec![0u8; stride * rows];
    let mut alpha = vec![0u8; w as usize * rows];

    // ★ 首行干跑（m128）：把**确定性**失败推到"一块都还没写"的时刻。
    //   与真转换用的是同一枚变换、同一份入参，因此任何"换个块也会失败"的原因在这里
    //   同样失败——写完第 k 块才发现 profile 是坏的这种事就不可能发生了。
    //   （`vec![0u8; ..]` 之后紧接这一行，`scratch` 还整块空着，借用与前 1 行兼容。）
    let probe = unsafe {
        TranslateBitmapBits(
            transform,
            bitmap.rgba.as_ptr().cast(),
            BM_XBGRQUADS,
            w,
            1,
            stride as u32,
            scratch.as_mut_ptr().cast(),
            BM_XBGRQUADS,
            stride as u32,
            std::ptr::null(),
            0,
        )
    };
    if !probe.as_bool() {
        return Err(format!(
            "颜色转换失败（{}）",
            std::io::Error::last_os_error()
        ));
    }

    let mut y = 0usize;
    while y < h as usize {
        let n = rows.min(h as usize - y);
        let start = y * stride;
        let block = &bitmap.rgba[start..start + stride * n];

        // mscms 的 x 位不保证写回，先把 alpha 逐行留底。
        for row in 0..n {
            for x in 0..w as usize {
                alpha[row * w as usize + x] = block[row * stride + x * 4 + 3];
            }
        }

        // SAFETY: 源/目标缓冲区都至少有 `stride * n` 字节，宽高与步长一致。
        let ok = unsafe {
            TranslateBitmapBits(
                transform,
                block.as_ptr().cast(),
                BM_XBGRQUADS,
                w,
                n as u32,
                stride as u32,
                scratch.as_mut_ptr().cast(),
                BM_XBGRQUADS,
                stride as u32,
                std::ptr::null(),
                0,
            )
        };
        if !ok.as_bool() {
            return Err(format!(
                "颜色转换失败（{}）",
                std::io::Error::last_os_error()
            ));
        }

        let dst_block = &mut bitmap.rgba[start..start + stride * n];
        dst_block.copy_from_slice(&scratch[..stride * n]);
        for row in 0..n {
            for x in 0..w as usize {
                dst_block[row * stride + x * 4 + 3] = alpha[row * w as usize + x];
            }
        }
        y += n;
    }
    Ok(())
}

/// ICC profile 的原始色彩空间签名（profile header 偏移 16 的 4 字节，如 `RGB `）。
///
/// 只用于日志与调试；转换本身交给系统 CMM，不需要我们解释 profile。
pub fn color_space(icc: &[u8]) -> Option<[u8; 4]> {
    icc.get(16..20).and_then(|s| s.try_into().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 给恒等变换用例用：系统 sRGB profile 是否存在（不参与转换路径）。
    fn system_profile_path_lossy() -> Option<String> {
        let mut buf = system_srgb_path().ok()?;
        while buf.last() == Some(&0) {
            buf.pop();
        }
        Some(String::from_utf16_lossy(&buf))
    }

    fn sample() -> Bitmap {
        // 2×2：红、绿、蓝、半透明白。
        Bitmap {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
            ],
        }
    }

    /// 真机 CMM 端到端：系统 sRGB profile 作为源跑一遍，颜色应几乎不变、alpha 原样。
    /// 这条同时证明 `mscms` 绑定能开 profile、建变换、分块转换。
    #[test]
    fn identity_transform_preserves_pixels_and_alpha() {
        let Some(path) = system_profile_path_lossy() else {
            return; // 系统没有 sRGB profile：不把环境缺失算成回归
        };
        let icc = std::fs::read(&path).expect("读系统 sRGB profile");
        let mut bmp = sample();
        let before = bmp.rgba.clone();
        to_srgb(&mut bmp, &icc).expect("sRGB→sRGB 变换应成功");
        for (i, (a, b)) in before.iter().zip(bmp.rgba.iter()).enumerate() {
            if i % 4 == 3 {
                assert_eq!(a, b, "alpha 必须原样保留（第 {i} 字节）");
            } else {
                let d = (*a as i16 - *b as i16).abs();
                assert!(d <= 3, "通道 {i} 偏差 {d} 过大：{a} → {b}");
            }
        }
    }

    /// 非对角 profile（不同基色）必须按 RGBA 字节序读像素。
    ///
    /// 用 ProPhoto 型 D50 矩阵 + gamma 1.8 的最小 v2 profile：把 (200,100,50) 转成
    /// sRGB 约 (255,80,47)。若误用 `BM_xRGBQUADS`（内存 B,G,R,x），红蓝被对调，
    /// 结果约 (221,128,0)——蓝通道几乎归零，正是这条用例的判据。
    ///
    /// 为什么不能只靠系统 sRGB 的恒等样本：同基色变换是逐通道对角阵，与通道对调
    /// 可交换（红蓝交换两次又换回来），对角样本对这个错完全免疫。
    #[test]
    fn non_diagonal_profile_keeps_channel_order() {
        let mut bmp = Bitmap {
            width: 1,
            height: 1,
            rgba: vec![200, 100, 50, 255],
        };
        to_srgb(&mut bmp, &wide_profile()).expect("合成 profile 应被系统 CMM 接受");
        let (r, g, b, a) = (bmp.rgba[0], bmp.rgba[1], bmp.rgba[2], bmp.rgba[3]);
        assert_eq!(a, 255, "alpha 必须原样保留");
        assert!(r >= 245, "红通道 {r} 过低（期望 ~255）");
        assert!((20..=110).contains(&g), "绿通道 {g} 越界（期望 ~80）");
        assert!(b >= 30, "蓝通道 {b} 过低（期望 ~47；≈0 说明红蓝被对调）");
    }

    fn s15(x: f64) -> i32 {
        (x * 65536.0).round() as i32
    }

    fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut v = b"XYZ \0\0\0\0".to_vec();
        for c in [x, y, z] {
            v.extend_from_slice(&s15(c).to_be_bytes());
        }
        v
    }

    fn curv_gamma(gamma: f64) -> Vec<u8> {
        let mut v = b"curv\0\0\0\0".to_vec();
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&((gamma * 256.0).round() as u16).to_be_bytes());
        v
    }

    fn desc_tag(text: &str) -> Vec<u8> {
        let mut v = b"desc\0\0\0\0".to_vec();
        let t = text.as_bytes();
        v.extend_from_slice(&((t.len() + 1) as u32).to_be_bytes());
        v.extend_from_slice(t);
        v.push(0);
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(&0u16.to_be_bytes());
        v.push(0);
        v.extend_from_slice(&[0u8; 67]);
        v
    }

    fn text_tag(text: &str) -> Vec<u8> {
        let mut v = b"text\0\0\0\0".to_vec();
        v.extend_from_slice(text.as_bytes());
        v.push(0);
        v
    }

    /// 最小可用的 v2 RGB matrix/TRC profile（mscms 要求 desc + cprt 才认）。
    fn wide_profile() -> Vec<u8> {
        // RGB→XYZ 矩阵的**列**（纯 R/G/B 的 XYZ），列和 = D50 白点。
        let white = (0.96429568, 1.0, 0.8251046);
        let mut tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"rXYZ", xyz_tag(0.7977605, 0.2880711, 0.0)),
            (b"gXYZ", xyz_tag(0.1351858, 0.7118432, 0.0)),
            (b"bXYZ", xyz_tag(0.0313493, 0.0000857, 0.8251046)),
            (b"wtpt", xyz_tag(white.0, white.1, white.2)),
            (b"rTRC", curv_gamma(1.8)),
            (b"gTRC", curv_gamma(1.8)),
            (b"bTRC", curv_gamma(1.8)),
            (b"desc", desc_tag("RuPix test profile")),
            (b"cprt", text_tag("Public domain test profile")),
        ];
        tags.sort_by_key(|t| *t.0);

        let table = 4 + tags.len() * 12;
        let mut off = 128 + table;
        let mut entries = Vec::new();
        let mut bodies = Vec::new();
        for (sig, body) in &tags {
            while !off.is_multiple_of(4) {
                bodies.push(0u8);
                off += 1;
            }
            entries.push((*sig, off as u32, body.len() as u32));
            bodies.extend_from_slice(body);
            off += body.len();
        }

        let mut hdr = [0u8; 128];
        hdr[0..4].copy_from_slice(&(off as u32).to_be_bytes());
        hdr[4..8].copy_from_slice(b"lcms");
        hdr[8..12].copy_from_slice(&0x0210_0000u32.to_be_bytes());
        hdr[12..16].copy_from_slice(b"mntr");
        hdr[16..20].copy_from_slice(b"RGB ");
        hdr[20..24].copy_from_slice(b"XYZ ");
        for (i, v) in [2026u16, 10, 19, 12, 0, 0].iter().enumerate() {
            hdr[24 + i * 2..26 + i * 2].copy_from_slice(&v.to_be_bytes());
        }
        hdr[36..40].copy_from_slice(b"acsp");
        hdr[40..44].copy_from_slice(b"    ");
        hdr[68..72].copy_from_slice(&s15(white.0).to_be_bytes());
        hdr[72..76].copy_from_slice(&s15(white.1).to_be_bytes());
        hdr[76..80].copy_from_slice(&s15(white.2).to_be_bytes());

        let mut out = hdr.to_vec();
        out.extend_from_slice(&(tags.len() as u32).to_be_bytes());
        for (sig, o, sz) in entries {
            out.extend_from_slice(sig);
            out.extend_from_slice(&o.to_be_bytes());
            out.extend_from_slice(&sz.to_be_bytes());
        }
        out.extend_from_slice(&bodies);
        assert_eq!(out.len(), off);
        out
    }

    /// 坏 profile 必须报错而不是 panic；报错时调用方可以保留原像素。
    #[test]
    fn garbage_profile_is_an_error_not_a_panic() {
        let mut bmp = sample();
        let before = bmp.rgba.clone();
        assert!(to_srgb(&mut bmp, b"not a profile").is_err());
        assert_eq!(bmp.rgba, before, "失败不得留下半张改过的图");
    }
}
