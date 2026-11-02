//! 解码结果的整理：像素数与体积校验、分段输出、ICC 附加、错误码转文案。
//!
//! 这里的所有闸门都在**分配之前**判定（`ICC_CAP` / `BPP_MAX`）——审计 V5 的教训是
//! 「先按声明值分配、再校验」等于把分配权交给了畸形文件。
use super::*;

/// `PF_*` 里最宽的一种：`PF_RGBA_F32` = 4 通道 × f32 = 16 字节/像素。自洽判定按它算上限。
pub(super) const BPP_MAX: u64 = 16;

/// 顶到尺寸闸时给用户的**可行动**文案（审计 #8）。
///
/// 过去这里只有一句"图片超出可处理范围"：用户既不知道上限在哪、也不知道该拿这张图怎么办，
/// 而"打开失败"与"这机器解不动"是两种完全不同的结论。带上**这张图的尺寸**与**闸门数值**，
/// 用户至少能做下一个决定（缩小后再来 / 换软件看）。`MAX_PIXELS` 这道闸本身是刻意的取舍：
/// 单帧 41MP 已经要 160 MB RGBA，再往上是"看图器"这个定位不该吃下的内存（见 `core::pixels`）。
pub(super) fn too_large_msg(aw: u32, ah: u32) -> String {
    format!(
        "图片太大，本程序不解：这张 {aw}×{ah}（约 {} 万像素），上限 {} 万像素 / {}。请用图像软件缩小后再打开。",
        (aw as u64 * ah as u64 + 5_000) / 10_000,
        MAX_PIXELS / 10_000,
        crate::core::util::fmt_bytes(MAX_PIXELS * 4),
    )
}

/// 渲染阶段的分配长度 `(像素段, ICC 段)`。契约见规范 §十一：宿主分配 `bytes_needed + icc_len`，
/// 渲染时像素写前段、ICC 紧跟其后。两段长度**只认查询阶段**回填的值（规范明写"以查询值为准，
/// 不重谈缓冲"）。
///
/// 审计 V5：宿主过去只分配 `bytes_needed`，于是任何按规范透传 ICC 的解码器都会往堆缓冲尾后写
/// `icc_len` 字节——那是已映射内存，SEH 拦不住。审计当时 12 个 DLL 全报 0（潜伏）；今天
/// core / jpeg / png / tiff 四枚已照实报 ICC，其余 8 枚仍报 0——契约写在 ABI 上，第三方
/// 解码器第一天就会踩。过大 = 契约违约，拒绝而不是照单分配。
///
/// 审计第三批 (e)：再加一道**自洽**判定——申报的像素段必须与它自己报的宽高相称。绝对上限
/// （`MAX_PIXELS * 4`）只看总量，拦不住"100×100 却报 160 MB"这种小图大要价：那是一张谁也
/// 看不清的图，却要宿主按住一整张顶格巨图的内存。行对齐留 1 MiB 余量。
pub(super) fn out_segments(info: &IvRenderOut) -> Result<(usize, usize), String> {
    if info.icc_len > ICC_CAP {
        return Err(format!(
            "解码器申报的 ICC 段过大（{} 字节 > {}），拒绝渲染",
            info.icc_len, ICC_CAP
        ));
    }
    let px = info.actual_w as u64 * info.actual_h as u64;
    // 下界也要判：`bytes_needed=0` 能通过上面的“不大于”闸，宿主就会拿
    // 0 长度 `Vec::as_mut_ptr()`（dangling）进渲染阶段。每个像素至少 1 字节，
    // 申报小于 `px` 一定是解码器算错/违约。
    if info.bytes_needed < px {
        return Err(format!(
            "解码器申报的缓冲区小于尺寸所需（{}×{} 却只报 {} 字节），拒绝渲染",
            info.actual_w, info.actual_h, info.bytes_needed
        ));
    }
    if info.bytes_needed > px.saturating_mul(BPP_MAX) + (1 << 20) {
        return Err(format!(
            "解码器申报的缓冲区与实际尺寸不符（{}×{} 却报 {} 字节），拒绝渲染",
            info.actual_w, info.actual_h, info.bytes_needed
        ));
    }
    Ok((info.bytes_needed as usize, info.icc_len as usize))
}

/// 从解码器输出缓冲里取出 ICC 尾段，并释放像素段（`to_rgba8` 只该看到像素）。
///
/// 尾段不存在时返回空 `Vec`；长度由查询阶段申报、`out_segments` 已过闸。
pub(super) fn take_icc(buf: &mut Vec<u8>, pix_len: usize, icc_len: usize) -> Vec<u8> {
    let icc = if icc_len == 0 {
        Vec::new()
    } else {
        buf.get(pix_len..pix_len + icc_len)
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    };
    buf.truncate(pix_len);
    icc
}

/// 有 ICC 就把**显示档**位图转成 sRGB；失败就按现有像素显示（颜色不准好过整张打不开）。
///
/// 调用点都在 `fit_to` / `fit_oriented` **之后**：转换成本跟视口像素走，不跟原图大小走
/// ——但 `(0, 0)` 那趟原图档升级不缩，所以它**也可能**是原图尺寸。
pub(super) fn apply_icc(bitmap: &mut Bitmap, icc: &[u8], path: &Path) {
    if icc.is_empty() {
        return;
    }
    match crate::platform::color::to_srgb(bitmap, icc) {
        Ok(()) => log::debug!(
            "ICC→sRGB：{}（{} 字节，色彩空间 {:?}）",
            path.display(),
            icc.len(),
            crate::platform::color::color_space(icc)
                .map(|s| String::from_utf8_lossy(&s).to_string())
        ),
        // m128：`to_srgb` 只保证**确定性**失败那一档的原样；真跑到中途才失败时 picture
        // 里可能混着已转换的行。两条都说得出口，别把承诺写成只有一条。
        Err(e) => log::warn!(
            "ICC 转换失败，按现有像素显示（{}）：{e}（确定性失败时为原图；中途失败时上部可能已转 sRGB）",
            path.display()
        ),
    }
}

/// 拿解码器最近一次失败的可读原因（解码器侧线程局部）。
pub(super) fn last_error_msg(f: unsafe extern "C" fn(*mut c_char, usize) -> i32) -> String {
    let mut buf = vec![0u8; 512];
    let cap = buf.len();
    let need = invoke(|| unsafe { f(buf.as_mut_ptr().cast(), cap) });
    let n = match need {
        Ok(n) if n > 0 => (n as usize).min(cap),
        _ => return String::new(),
    };
    let end = buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// 把错误码翻成**能区分**的中文文案（规范 §四：不许整个吞成"打不开"）。
pub(super) fn describe_code(id: &str, code: i32, detail: &str) -> String {
    let base = match code {
        ERR_UNRECOGNIZED => "未知格式（解码器不识别该文件）".to_string(),
        ERR_CORRUPT => "文件已损坏".to_string(),
        ERR_UNSUPPORTED => "该文件用了此解码器不支持的特性".to_string(),
        ERR_BUFFER_TOO_SMALL => "内部错误（缓冲不足，请反馈）".to_string(),
        ERR_OOM => "图片过大，内存不足".to_string(),
        ERR_PANIC => "解码器遇到问题，已隔离".to_string(),
        ERR_TOO_LARGE => "图片超出可处理范围".to_string(),
        other => format!("解码器返回未知错误码 {other}"),
    };
    if detail.is_empty() {
        format!("{base}（解码器 {id}）")
    } else {
        format!("{base}：{detail}（解码器 {id}）")
    }
}
