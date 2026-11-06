//! 解码产物的公共类型与像素格式归一（**core 的"货币"定义**）。
//!
//! 这里**没有任何解码实现**——全部解码能力在 `decoders/` 层的 DLL 里（SVG 例外，
//! 走上屏层注入的渲染函数）。本模块只定义宿主与解码器之间传递像素的类型，
//! 以及把解码器输出的任意 `pixel_format` 归一成非预乘 RGBA8 的纯函数。
//!
//! 单一定义点：解码硬顶常量（[`MAX_PIXELS`] / [`MAX_EDGE`]）只在本文件声明一次，
//! 解码器宿主、SVG 路由、裸数据参数窗全部引用这里——历史上 decoders 内部另抄了一份 41MP
//! 常量，两个定义点改一个漏一个，已合并。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::core::abi::*;

/// 解码像素总量上限（64MP ≈ RGBA 缓冲 256MB 的硬顶）。
///
/// ★ 这是**解码前**的闸门，不是解码后才发现 OOM。24 个参考项目里**零个**做了这道闸——
/// 它们要么全解（超大图直接吃满内存），要么靠解码中的水位检查。
/// 在解码前按尺寸算出来再缩，省掉的是整次解码，不是解码完再扔。
///
/// ★ **与解码器侧那枚同名常量现在同值**（2026-10-28 用户拍板"抬到 64 MP"）：
/// 每枚解码 DLL 的 `src/lib.rs` 里各有一份 `const MAX_PIXELS = 64 * 1024 * 1024`，
/// 那是解码器自己的**分配闸**（防畸形头把内存榨干）。此前宿主这道是 41 MP，
/// **比解码器那道更紧** ⇒ 41–64 MP 那一档位图被宿主先拒，而解码器本来能解。
/// 两道闸管两件不同的事（分配闸防畸形头、看图闸防内存与停顿），但**上限取同一个数**：
/// 宿主再往上调就调不动了——解码器那道先拒（与宿主给的 `budget_bytes` 无关）。
/// 同名两值的漂移风险由解码器仓 `tools/check_caps.py` 守着
/// （12 份必须同值，且 `testdata/make_samples.py` 里镜像的本值要等于这里）。
///
/// ★ 代价是清楚的：一张 64 MP 的 RGBA 缓冲 256 MB，解码路径上同时存在
/// 解码器缓冲 + 宿主缓冲 + 派生缓冲 + 纹理 ⇒ 峰值约 1 GB。真要再往上（512 MP 一类）
/// 不是"宿主管不管"的问题，而是"谁来分块"的问题（宿主拿不到"只解这一块"的能力，
/// 那需要解码器侧的新导出 ⇒ ABI v7）。
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// 单边上限，**目前只用于裸数据参数窗的输入校验**（`ui/raw_dialog.rs`：宽/高得落在
/// `1…MAX_EDGE`，那是"用户手填的尺寸"该守的界）。
///
/// ★ 它**不是**位图解码的第二道闸，而且是刻意的：位图路线只看 [`MAX_PIXELS`]。
/// 长截图（1080×20000 这类）与全景图是本产品明确支持的品类（`Viewer::is_long_image_in`
/// 还专门为它做了"长图适应"的交互），拿 16384 去夹单边会把它们挡在门外。
/// 谁要把这条变成硬闸，先回答"长截图还开不开"。（2026-10-18 第四批审计：原注释写的是
/// "与总量闸并行生效"，与代码不符。）
pub const MAX_EDGE: u32 = 16_384;

/// **显示侧**的单边上限：GPU 纹理建不出来的尺寸（D3D11 的单纹理边长上限 16384）。
///
/// ★ 与 [`MAX_PIXELS`] 是两件事，别混：
/// - [`MAX_PIXELS`] 管**内存与解码工作量**（总量），放开它 = 允许更大的图进 RAM；
/// - 本常量管**能不能画出来**（单边），超了是硬件事实，放不开。
///
/// 为什么需要它：解码上限抬到 64 MP 之后，可能出现"单边就超过 16384"的图
/// （30000×2000 的全景图 = 60 MP，总量在限内、单边超限）。D3D11 建不出那么宽的纹理，
/// windui 的 `Image::from_rgba` 会失败，而宿主的上屏路径当时是"失败 → 本轮留白"
/// ⇒ 用户看到的是**一片空白加一行日志**，比"打不开并说清楚"更难排查。
///
/// ★ 因此显示侧必须**主动降边**而不是撞墙：上屏位图按本常量等比缩到能上传为止
/// （见 `core::derive::derive_level`），画面照常显示，只是 1:1 时略糊。
/// 上传前那道兜底（`ui::canvas`）只在有路径绕过派生时才会响。
pub const MAX_TEXTURE_SIDE: u32 = 16_384;

/// 解码产物：原图尺寸 + 可直接上屏的位图。
///
/// 两个尺寸分开保留是必须的：位图经常被缩到上屏尺寸（省内存、省时间），
/// 而「原图多大」要在状态栏显示、也要参与缩放倍率的计算——拿位图尺寸当原图尺寸
/// 会让状态栏在缩放时跟着变，也永远算不出真正的 100%。
#[derive(Debug, Clone)]
pub struct Decoded {
    /// 原图像素尺寸。
    pub natural: (u32, u32),
    /// 实际解码出的位图（可能小于原图；SVG 局部渲染时只覆盖 [`Decoded::region`]）。
    pub bitmap: Bitmap,
    /// 位图覆盖范围：`None` = 整图；`Some(r)` = 只覆盖图像坐标系里的一块
    /// （SVG 可视区域 1:1 渲染）。位图密度（设备像素/图像像素）随 r 一起给。
    pub region: Option<Region>,
    /// 播放元信息（ABI v3/v4 合流）。静态图恒为 [`Anim::STATIC`]。
    ///
    /// ★ **页与帧在这里统一**（2026-11-02 用户拍板）：对宿主来说"第 3 页"和
    /// "第 3 帧"是同一种东西——都是一次按号请求换出来的另一张位图。所以本结构
    /// 一个 `index/count` 同时承载两者，`paged` 只用来选**人话**（页/帧）和
    /// 决定打开时停不停车。**没有第二套通道、第二套按钮、第二套快捷键。**
    pub anim: Anim,
}

/// 播放元信息：当前第几张（帧或页）、总数、本张展示时长。
///
/// `count = 0` 表示总数未知（GIF 惰性数帧，解码器撞到 EOF 才回填）；
/// `delay_ms = 0` 表示静态图，宿主不挂播放调度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anim {
    /// 本张序号（0 起）。多页文档里就是页号。
    pub index: u32,
    /// 总张数（0 = 未知）。
    pub count: u32,
    /// 本张展示时长（毫秒）。
    pub delay_ms: u32,
    /// 这张是**分页文档**（页数 > 1 且没有帧延时）。只影响一件事：
    /// 状态行写"页"还是"帧"（[`Anim::unit`]）。
    ///
    /// ★ 页与帧已经合流（宿主只维护一个号），`paged` 不再决定"停不停车"
    /// （打开时一律播放，见 `viewer::prefetch`）、也不决定按钮栏有没有那一组
    /// （由 [`Anim::has_index`] 管）。
    pub paged: bool,
}

impl Anim {
    /// 静态图（所有非动画格式的恒定值）。
    pub const STATIC: Anim = Anim {
        index: 0,
        count: 1,
        delay_ms: 0,
        paged: false,
    };

    /// 是否要在浮条上挂播放组（delay > 0 即宿主需要调度下一张）。
    ///
    /// ★ 多页文档同样走这条：页间隔就是"延时"，[`Anim::PAGE_DELAY_MS`]。
    pub fn is_animated(self) -> bool {
        self.delay_ms > 0
    }

    /// 这类文件**有没有"第几张"可切**（决定状态行写不写帧/页进度、按钮栏挂不挂播放组）。
    ///
    /// ★ 判据只认解码器回填的**多张语义**（用户 2026-11-06 口径）：静态位图恒为
    ///   [`Anim::STATIC`]（`count=1 / delay_ms=0 / paged=false`），没有第二张，
    ///   不许给它 `(1/1)`，也不许给逐帧按钮。能进来的只有：
    ///   - 真动图：报了帧延时（`delay_ms > 0`）或不止一帧（`count > 1`）；
    ///   - 多页文档：`paged = true`（页间隔由宿主给，见 [`Anim::PAGE_DELAY_MS`]）。
    ///
    /// 总数未知的畸形 GIF（`count = 0`）靠延时那一半进来，状态行仍只写分子。
    ///
    /// ★ 与 [`Anim::is_animated`] 是两件事：后者只管要不要挂播放调度；
    ///   本方法管"有没有号可报"。`is_animated()` 为真时本方法必为真。
    pub fn has_index(self) -> bool {
        self.count > 1 || self.delay_ms > 0 || self.paged
    }

    /// 进度文案的量词（状态行用）：页与帧在宿主里是同一个号，文案只差这一个词。
    pub fn unit(self) -> &'static str {
        if self.paged {
            "页"
        } else {
            "帧"
        }
    }

    /// 分页文档的页间隔：1 秒一张。
    ///
    /// 比 GIF 那种逐帧延时大一个数量级——人读一页要时间，而 GIF 是设计成连续动的。
    /// 用户 2026-11-02 拍板"统一成一套，页就按 1 秒一张自动跳"。
    pub const PAGE_DELAY_MS: u32 = 1000;

    /// 多帧文件**没报延时**时的兜底帧间隔（100 ms ≈ 10 fps）。
    ///
    /// 有些 GIF/APNG 把第一帧写成 `delay=0`（本意"尽快"，不是"停住"）。照抄 0 会让
    /// 宿主把它当静态图、永不推进——表现是"这张 GIF 不动了"。有不止一张就给这个值。
    pub const FALLBACK_FRAME_MS: u32 = 100;
}

/// 局部位图在原图坐标系里的位置与密度（见 [`Decoded::region`]）。
#[derive(Debug, Clone)]
pub struct Region {
    /// 位图左上角在图像坐标系的位置（图像像素）。
    pub origin: (f32, f32),
    /// 位图密度：设备像素 / 图像像素。
    pub density: f32,
}

/// 单张解码产物：**非预乘 RGBA8**（上屏层的统一货币）。
///
/// 所有解码后端（解码器 DLL / SVG 渲染）都归一成这个格式——上层永远不必记
/// 「这份是 BGRA」。
#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// 长度恒为 `width * height * 4`，行优先，非预乘。
    pub rgba: Vec<u8>,
}

// ── 像素格式归一（宿主侧）──────────────────────────────────────────

/// 把解码器输出的任意 `pixel_format` 归一成非预乘 RGBA8。
///
/// >8bit 的规则照规范 §三 抄：**min-max 归一化**（不是按满量程截断）；非有限值 → 0；
/// > `max - min < ε` 时避免除零；结果 clamp 到 `[0,255]`。
pub fn to_rgba8(buf: Vec<u8>, w: u32, h: u32, pf: u32, _bit_depth: u32) -> Result<Vec<u8>, String> {
    let n = (w as usize) * (h as usize);
    let need = |bps: usize, ch: usize| -> Result<(), String> {
        if buf.len() < n * bps * ch {
            Err(format!(
                "解码器返回的缓冲偏小（{} < {}）",
                buf.len(),
                n * bps * ch
            ))
        } else {
            Ok(())
        }
    };
    match pf {
        PF_RGBA8 => {
            need(1, 4)?;
            let mut buf = buf;
            buf.truncate(n * 4);
            Ok(buf) // 已是目标格式：零拷贝，原缓冲直接就是结果。
        }
        PF_RGB8 => {
            need(1, 3)?;
            let mut out = vec![0u8; n * 4];
            for i in 0..n {
                out[i * 4] = buf[i * 3];
                out[i * 4 + 1] = buf[i * 3 + 1];
                out[i * 4 + 2] = buf[i * 3 + 2];
                out[i * 4 + 3] = 255;
            }
            Ok(out)
        }
        PF_GRAY8 => {
            need(1, 1)?;
            let mut out = vec![0u8; n * 4];
            for i in 0..n {
                let g = buf[i];
                out[i * 4] = g;
                out[i * 4 + 1] = g;
                out[i * 4 + 2] = g;
                out[i * 4 + 3] = 255;
            }
            Ok(out)
        }
        PF_GRAY16 | PF_RGB16 | PF_RGBA16 => {
            let ch = channels_of(pf);
            need(2, ch)?;
            let samples: Vec<f32> = (0..n * ch)
                .map(|i| {
                    let lo = buf[i * 2] as u16;
                    let hi = buf[i * 2 + 1] as u16;
                    u16::from_le_bytes([lo as u8, hi as u8]) as f32
                })
                .collect();
            Ok(normalize_to_rgba(&samples, n, ch))
        }
        PF_GRAY_F32 | PF_RGBA_F32 => {
            let ch = channels_of(pf);
            need(4, ch)?;
            let samples: Vec<f32> = (0..n * ch)
                .map(|i| {
                    let b = &buf[i * 4..i * 4 + 4];
                    f32::from_le_bytes([b[0], b[1], b[2], b[3]])
                })
                .collect();
            Ok(normalize_to_rgba(&samples, n, ch))
        }
        other => Err(format!("不支持的像素格式 {other}")),
    }
}

/// 该像素格式每个样本的通道数（归一化路径专用）。
pub fn channels_of(pf: u32) -> usize {
    match pf {
        PF_GRAY16 | PF_GRAY_F32 => 1,
        PF_RGB16 => 3,
        _ => 4,
    }
}

/// 多通道浮点样本 → RGBA8：逐通道 min-max 归一化。非有限值当 0。
pub fn normalize_to_rgba(samples: &[f32], n: usize, ch: usize) -> Vec<u8> {
    let mut mins = [f32::INFINITY; 4];
    let mut maxs = [f32::NEG_INFINITY; 4];
    for i in 0..n {
        for c in 0..ch {
            let v = samples[i * ch + c];
            let v = if v.is_finite() { v } else { 0.0 };
            if v < mins[c] {
                mins[c] = v;
            }
            if v > maxs[c] {
                maxs[c] = v;
            }
        }
    }
    let mut out = vec![0u8; n * 4];
    for i in 0..n {
        for c in 0..ch {
            let v = samples[i * ch + c];
            let v = if v.is_finite() { v } else { 0.0 };
            let (lo, hi) = (mins[c], maxs[c]);
            let norm = if (hi - lo).abs() < f32::EPSILON {
                0.0
            } else {
                (v - lo) / (hi - lo)
            };
            out[i * 4 + c] = (norm * 255.0).clamp(0.0, 255.0) as u8;
        }
        // 单通道是灰度而不是红色；归一化后复用同一个值，避免通道间漂移。
        if ch == 1 {
            out[i * 4 + 1] = out[i * 4];
            out[i * 4 + 2] = out[i * 4];
        }
        if ch < 4 {
            out[i * 4 + 3] = 255;
        }
    }
    out
}

/// 上屏位图：非预乘 RGBA8 像素 + **代号 `gen`**。
///
/// `gen` 是一个进程内单调递增的代号（每次构造 +1）：上屏层据此缓存框架纹理
/// （`windui::Image` 内部 `Pixmap`），仅当代号变化才重新上传——与 windui 的
/// `Image::cache_id` 同构，但类型本身不认识任何 UI 框架（core 纯化的关键）。
///
/// 像素用 `Arc<Vec<u8>>` 而不是 `Vec<u8>`：**显示档可以就是解码档本身**。放大到
/// ≥100% 时显示档与原图档同尺寸同像素，用 `Vec` 就必然多占一份整图——6055×6055
/// 那份实测 139.9 MB（本仓历史记录 的 M1）。
/// 顺带这也是 M2（把本地派生挪到解码线程）的前置：跨线程只能走 `Arc`。
#[derive(Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    pub gen: u64,
}

/// `gen` 的分配器。**必须是全模块唯一一个计数器**：`new` 与 `shared` 各自建一个
/// `static` 的话，两条构造路会发出重复代号，而上屏层正是拿 `gen` 判脏——重复即
/// "新图被当成旧图继续画"。
static NEXT_RGBA_GEN: AtomicU64 = AtomicU64::new(1);

fn next_rgba_gen() -> u64 {
    NEXT_RGBA_GEN.fetch_add(1, Ordering::Relaxed)
}

impl RgbaImage {
    /// 造一份新位图并领取新的 `gen` 代号（收下调用方刚产出的像素，不复制）。
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba: Arc::new(rgba),
            gen: next_rgba_gen(),
        }
    }

    /// 用**已存在**的缓冲造一份位图：共享同一块内存，一次拷贝都不发生。
    ///
    /// 给"显示档 == 解码档"那一条路用（见 [`RgbaImage`] 的 `rgba` 字段说明）。
    /// 仍然领取新的 `gen`——共享像素不等于共享身份：上屏层按 `gen` 判脏，而这里代号
    /// 推进正是为了让下一档能换上来；像素没变时框架侧靠 `windui::Image::cache_id`
    /// 自己复用，宿主这边不多画一次。
    pub fn shared(width: u32, height: u32, rgba: Arc<Vec<u8>>) -> Self {
        Self {
            width,
            height,
            rgba,
            gen: next_rgba_gen(),
        }
    }
}

impl fmt::Debug for RgbaImage {
    /// 只报尺寸与字节数：一份整图是三千六百万元素，把像素打出来会把日志撑爆。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .field("gen", &self.gen)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RGB8 → RGBA8 展开要补 alpha=255；缓冲偏小要报错而不是越界读。
    #[test]
    fn rgb8_expands_with_opaque_alpha() {
        let out = to_rgba8(vec![10, 20, 30, 40, 50, 60], 2, 1, PF_RGB8, 8).unwrap();
        assert_eq!(out, vec![10, 20, 30, 255, 40, 50, 60, 255]);
        assert!(
            to_rgba8(vec![1, 2, 3], 2, 1, PF_RGB8, 8).is_err(),
            "偏小要报错"
        );
    }

    #[test]
    fn audit_gray16_to_rgba8_replicates_all_color_channels() {
        let buf = [1000u16, 1500, 2000]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            to_rgba8(buf, 3, 1, PF_GRAY16, 16).unwrap(),
            [0, 0, 0, 255, 127, 127, 127, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn audit_gray_f32_to_rgba8_replicates_all_color_channels() {
        let buf = [f32::NAN, 0.5, 1.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        assert_eq!(
            to_rgba8(buf, 3, 1, PF_GRAY_F32, 32).unwrap(),
            [0, 0, 0, 255, 127, 127, 127, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn audit_high_depth_rgb_and_rgba_keep_independent_channels() {
        for (pf, samples, expected) in [
            (
                PF_RGB16,
                vec![0u16, 20, 40, 10, 10, 80],
                vec![0, 255, 0, 255, 255, 0, 255, 255],
            ),
            (
                PF_RGBA16,
                vec![0, 20, 40, 10, 10, 10, 80, 0],
                vec![0, 255, 0, 255, 255, 0, 255, 0],
            ),
        ] {
            let buf = samples.into_iter().flat_map(u16::to_le_bytes).collect();
            assert_eq!(to_rgba8(buf, 2, 1, pf, 16).unwrap(), expected);
        }
        let buf = [0.0f32, 20.0, 40.0, 10.0, 10.0, 10.0, 80.0, 0.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        assert_eq!(
            to_rgba8(buf, 2, 1, PF_RGBA_F32, 32).unwrap(),
            [0, 255, 0, 255, 255, 0, 255, 0]
        );
    }

    /// >8bit：min-max 归一，且非有限值当 0、常量场不除零。
    #[test]
    fn high_bitdepth_normalizes_min_max() {
        let mut buf = Vec::new();
        for v in [1000u16, 2000] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        let out = to_rgba8(buf, 2, 1, PF_GRAY16, 16).unwrap();
        assert_eq!(out[0], 0);
        assert_eq!(out[4], 255);
        assert_eq!(out[3], 255, "补不透明 alpha");

        let mut flat = Vec::new();
        for _ in 0..2 {
            flat.extend_from_slice(&777u16.to_le_bytes());
        }
        let out = to_rgba8(flat, 2, 1, PF_GRAY16, 16).unwrap();
        assert_eq!(out[0], 0);
    }
}

/// 通道线性变换的恒等参数：三通道增益 100%、偏置 0。
pub const CHANNEL_NEUTRAL: [i32; 6] = [100, 100, 100, 0, 0, 0];

/// 声明式 op `channel-linear`：R/G/B 各自 `out = clamp(in * gain/100 + bias)`。
///
/// ★ 与 `channels` 插件的正式回写**逐字节同源**：中间值走 i64、整除向零截断、
/// alpha 与每像素第 4 字节原样复制。改任何一边都要同步改另一边，端到端判据
/// `channel_linear_matches_the_plugin_formula` 钉着这一点。
pub fn channel_linear(rgba: &[u8], gain: [i32; 3], bias: [i32; 3]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        for c in 0..3 {
            let n = px[c] as i64 * gain[c] as i64 / 100 + bias[c] as i64;
            px[c] = n.clamp(0, 255) as u8;
        }
    }
    out
}

/// `channel_linear` 的查表版：主窗整图每帧都要跟着滑块变，逐像素做三次 i64 除法
/// 太贵。输入只有 0..=255 共 256 种，先按同一公式把三张 256 项表算出来，之后
/// 每像素三次查表 + 一次 alpha 复制。**输出与 [`channel_linear`] 逐字节相同**
/// （`lut_matches_the_reference_formula` 钉着），不是"看起来差不多"的近似。
pub fn channel_linear_lut(rgba: &[u8], gain: [i32; 3], bias: [i32; 3]) -> Vec<u8> {
    let mut tables = [[0u8; 256]; 3];
    for c in 0..3 {
        for (v, slot) in tables[c].iter_mut().enumerate() {
            let n = v as i64 * gain[c] as i64 / 100 + bias[c] as i64;
            *slot = n.clamp(0, 255) as u8;
        }
    }
    let [r, g, b] = tables;
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        px[0] = r[px[0] as usize];
        px[1] = g[px[1] as usize];
        px[2] = b[px[2] as usize];
        // px[3]（alpha / 行尾填充）原样不动，与参考实现一致。
    }
    out
}

/// 最近邻缩略图（保持比例，不放大）：参数窗预览只算十万级像素，拖滑块才不会卡。
pub fn nearest_thumb(rgba: &[u8], w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32, Vec<u8>) {
    if w == 0 || h == 0 || rgba.len() < w as usize * h as usize * 4 {
        return (0, 0, Vec::new());
    }
    let scale = (max_w as f64 / w as f64)
        .min(max_h as f64 / h as f64)
        .min(1.0);
    let tw = (w as f64 * scale).round().max(1.0) as u32;
    let th = (h as f64 * scale).round().max(1.0) as u32;
    let mut out = vec![0u8; tw as usize * th as usize * 4];
    for y in 0..th as usize {
        let sy = (y as u64 * h as u64 / th as u64) as usize;
        for x in 0..tw as usize {
            let sx = (x as u64 * w as u64 / tw as u64) as usize;
            let si = (sy * w as usize + sx) * 4;
            let di = (y * tw as usize + x) * 4;
            out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    (tw, th, out)
}

#[cfg(test)]
mod preview_pixel_tests {
    use super::*;

    #[test]
    fn channel_linear_matches_the_plugin_formula() {
        // 插件 W7 的硬编码期望：0,127,255,77 经三通道 -100/255 => 255,128,0,77。
        let src = [0u8, 127, 255, 77];
        assert_eq!(
            channel_linear(&src, [-100, -100, -100], [255, 255, 255]),
            vec![255, 128, 0, 77]
        );
        // 恒等变换逐字节不变（含 alpha 与尾部字节）。
        let src2: Vec<u8> = (0..32).map(|i| (i * 7) as u8).collect();
        assert_eq!(channel_linear(&src2, [100, 100, 100], [0, 0, 0]), src2);
        // 每通道独立 + 越界夹取。
        assert_eq!(
            channel_linear(&[100, 50, 200, 128], [200, 100, 0], [10, 0, 0]),
            vec![210, 50, 0, 128]
        );
    }

    #[test]
    fn lut_matches_the_reference_formula() {
        // 含负增益、大偏置、每通道不同：一张表必须对得上逐像素公式。
        let src: Vec<u8> = (0..256u32)
            .flat_map(|i| [i as u8, (255 - i) as u8, (i / 2) as u8, 0xA5])
            .collect();
        for (gain, bias) in [
            ([-100, 100, 200], [255, 0, 40]),
            ([100, 100, 100], [0, 0, 0]),
            ([-400, 0, 400], [-255, 128, 255]),
            ([37, -63, 141], [7, -9, 11]),
        ] {
            assert_eq!(
                channel_linear_lut(&src, gain, bias),
                channel_linear(&src, gain, bias),
                "LUT 与参考公式在 {gain:?}/{bias:?} 上分叉"
            );
        }
        let alpha: Vec<u8> = channel_linear_lut(&src, [-100, -100, -100], [255, 255, 255]);
        assert!(
            alpha.chunks_exact(4).all(|p| p[3] == 0xA5),
            "第 4 字节不许改"
        );
    }

    #[test]
    fn neutral_parameters_are_a_no_op() {
        let src: Vec<u8> = (0..64).map(|i| (i * 13) as u8).collect();
        assert_eq!(channel_linear_lut(&src, [100, 100, 100], [0, 0, 0]), src);
        assert_eq!(CHANNEL_NEUTRAL, [100, 100, 100, 0, 0, 0]);
    }

    #[test]
    fn nearest_thumb_keeps_aspect_and_samples_the_source() {
        let src = [
            10u8, 20, 30, 255, 40, 50, 60, 255, //
            70, 80, 90, 255, 100, 110, 120, 255,
        ];
        let (w, h, px) = nearest_thumb(&src, 2, 2, 1, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(px, vec![10, 20, 30, 255]);
        // 不放大：小图仍是原尺寸。
        let (w2, h2, px2) = nearest_thumb(&src, 2, 2, 8, 8);
        assert_eq!((w2, h2), (2, 2));
        assert_eq!(px2, src.to_vec());
        // 源数据长度不够时明确返回空，别让 UI 拿一段残缺像素去建 Image。
        assert!(nearest_thumb(&[0u8; 4], 2, 2, 8, 8).2.is_empty());
    }
}
