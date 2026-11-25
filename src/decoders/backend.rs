//! 依赖倒置与调度接口：把本层的格式清单装进 `core::formats`，并实现 `DecodeBackend`。
//!
//! 这一层是 `core`（不认识 DLL）与 `decoders`（只知道 DLL）之间的唯一接缝，
//! 装配入口是 [`install_format_source`] 与 [`crate::decoders::Host::new`]。
use super::registry::*;
use super::*;

// ── 依赖倒置：把上述两份清单装进 core::formats ────────────────────

/// [`crate::core::formats::FormatSource`] 的解码器层实现：查询直接落到本层的
/// [`declared_cached`]（同一次目录扫描、按热加载代次缓存）。
pub(super) struct Catalog;

impl crate::core::formats::FormatSource for Catalog {
    fn declared_exts(&self) -> HashSet<String> {
        declared_cached().0
    }
    fn declared_raw_exts(&self) -> HashSet<String> {
        declared_cached().1
    }
    fn declared_vector_exts(&self) -> HashSet<String> {
        crate::decoders::registry::declared_vector_exts()
    }
    fn declared_region_vector_exts(&self) -> HashSet<String> {
        crate::decoders::registry::declared_region_vector_exts()
    }
}

/// 把本层的格式清单装进 core（幂等）。三个装入点：`runtime::run()`（GUI 入口）、
/// [`Host::new`]（解码线程）、本模块的 [`declared_exts`]（示例/探针直调）——
/// 任一条路走通，gallery/viewer/decode 的查询就不再返回空集。
pub fn install_format_source() {
    crate::core::formats::install(Box::new(Catalog));
}

// ── DecodeBackend：core 调度接口的解码器层实现 ─────────────────────
//
// 能力位图（契约可选导出 `iv_caps`，位含义见 `core::abi::CAP_*`）已经随
// `Descriptor.caps` 落到目录表，缺席时由 [`caps_of`] 按既有证据推定。
// **缩放路由已接通（2026-10-18）**：`allow_scaled` 来自请求语义（只有缩略图那一类），
// 与候选解码器的 `CAP_SCALED` 求与后才落成 `iv_render` 的 `REQ_ALLOW_SCALED` 位。
// **缩略图快路径（2026-11-13）**：`thumbnail` 只在 `ReqKind::Thumb` 为真，与
// `CAP_THUMB` 求与后落成 `REQ_THUMBNAIL`；`common` 0.2.7 的 JPEG 成员用它读 EXIF IFD1。
// 两条位互不替代：主画面首屏不会带 `REQ_THUMBNAIL`，缩略图也不靠 `CAP_SCALED` 冒充。
// **并发豁免（m349 P2，2026-11-25）**：`CAP_PARALLEL` 声明者（common 0.2.8 /
// classic 0.1.1 / texture 0.1.1 / modern 0.3.5 / pro 0.4.4 / raw 0.5.5——m349 复审
// 后六档全声明，doc/doc-legacy 留锁）在 `decode_with` 里跳过同一枚 DLL 的串行锁；
// 未声明者照旧持锁，行为与今天逐字节一致。

impl crate::core::decode::DecodeBackend for Host {
    /// 后台预取标志：只改排队档位，不动解码行为（见 `Host::bg`）。
    fn set_background(&mut self, on: bool) {
        self.bg = on;
    }

    /// 区域渲染（**ABI v6**）：认领该后缀、且实现了 `iv_render_region` 的解码器，
    /// 按"只渲看得见的那一块"出图。
    ///
    /// `None` = 没有解码器支持这条通道（宿主据此落回整幅路线，行为与今天一致）。
    fn try_decode_region(
        &mut self,
        path: &Path,
        ext: &str,
        region: &crate::core::decode::RegionReq,
    ) -> Option<Result<Decoded, crate::core::error::ViewFailure>> {
        Host::try_decode_region(self, path, ext, region)
    }

    fn decode(
        &mut self,
        path: &Path,
        ext: &str,
        max_w: u32,
        max_h: u32,
        frame_index: u32,
        raw: Option<RawParams>,
        allow_scaled: bool,
        thumbnail: bool,
    ) -> Result<Decoded, crate::core::error::ViewFailure> {
        use crate::core::error::ViewFailure;
        if safe_mode() {
            return Err(ViewFailure::unsupported(
                "安全模式：解码器已全部禁用（--safe-mode）",
            ));
        }
        // 裸数据参数：显式走 iv_render_raw。没有参数时先走普通解码——相机 RAW
        // （CR2/CR3/NEF/ARW…）有完整文件头，不能因为同档还支持裸像素就一律拦成
        // "需要宽高"；只有普通解码确实失败时，才给裸数据的引导。
        if let Some(p) = raw {
            return self
                .try_decode_raw(path, ext, p, max_w, max_h)
                .unwrap_or_else(|| {
                    Err(ViewFailure::unsupported(format!(
                        "没有解码器支持 .{ext}（解码器缺失或已停用）"
                    )))
                });
        }
        if crate::core::formats::is_raw(ext) {
            return self
                .try_decode(
                    path,
                    ext,
                    max_w,
                    max_h,
                    frame_index,
                    allow_scaled,
                    thumbnail,
                )
                .unwrap_or_else(|| {
                    // `None` = 没有任何解码器认领，**不是**「普通解码失败」。相机 RAW
                    // 有完整文件头，没有对应解码器时让用户去填宽高只会白忙；只有真正的
                    // 无头格式才给参数窗引导（2026-11-24 审计 L53）。
                    if crate::core::decode::is_headerless_ext(ext) {
                        Err(ViewFailure::other(
                            "裸数据文件没有尺寸信息：请从「打开…」重新选择该文件以指定宽/高/布局",
                        ))
                    } else {
                        Err(ViewFailure::unsupported(format!(
                            "没有解码器支持 .{ext}（解码器缺失或已停用）"
                        )))
                    }
                });
        }
        // 位图全部交给解码器。没有解码器认领时给出可行动的提示，而不是一句含混的"打不开"。
        // 无扩展名的文件不再提前报错——`try_decode` 会退一步做魔数嗅探（改名/无后缀图）。
        //
        // ★ 这里**没有**「点名内置库」那条分支了（2026-10-25 随 `platform::wic` 一并删除）：
        //   位图一律由解码器解，宿主自带的位图解码能力为零——便携看图器不调系统解码器，
        //   同一张图在两台机器上必须解出同一个结果（宪章 §5.1）。设置页「首选解码器」
        //   因此只在**多个解码器认领同一后缀**时才有意义。
        self.try_decode(
            path,
            ext,
            max_w,
            max_h,
            frame_index,
            allow_scaled,
            thumbnail,
        )
        .unwrap_or_else(|| {
            Err(ViewFailure::unsupported(format!(
                "没有解码器支持此文件（扩展名「{ext}」无人认领，且魔数嗅探也无匹配）"
            )))
        })
    }
}
