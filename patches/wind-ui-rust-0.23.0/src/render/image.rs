//! 图片资源与可插拔解码框架。
//!
//! 分两块：
//! - `Image`：解码后的不可变资源（内部 `Rc<Pixmap>`，构建期一次性解码，paint 期零解码）。
//! - 解码框架：`ImageDecoder` trait + thread-local 注册表。核心内置 `PngDecoder`
//!   （零新依赖，走 tiny-skia 原生 PNG 解码）；启用 `svg` feature（默认开）再内置
//!   `SvgDecoder`（resvg 光栅化）；JPEG/WebP 等由使用方 `register_decoder` 注入，
//!   核心代码与公共 API 零破坏。
//!
//! 注册表照搬 `theme` 的 thread-local 模式（UI 单线程，免加锁），PNG 预置，
//! 保证 `from_png_bytes` 永远可用。
//!
//! SVG 是矢量、分辨率无关的：内置解码器按 SVG 的**固有尺寸**一次光栅化（顺着
//! `ImageDecoder` 的定长 RGBA 契约接入，`from_bytes`/`from_file` 自动识别 `.svg`）；
//! 需要 HiDPI 清晰度的调用方用 [`crate::ui::ImageContent::from_svg_bytes`] 并传
//! `target_width=None`——它保留矢量源、按实际 DPI 现场光栅化，任何缩放倍率下都
//! 1:1；本层的 `target_width=Some(w)` 是写死光栅宽的底层出口。resvg 渲染产物经
//! `DecodedImage` 原始 RGBA「统一货币」转入本项目的
//! tiny-skia，与位图解码同一条入库路径。

use std::cell::RefCell;
use std::fmt;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use tiny_skia::{ColorU8, IntSize, Pixmap};

use crate::geometry::{Color, Size};

/// 加载失败时占位框的默认逻辑尺寸（dp），保证布局不塌陷。
pub const PLACEHOLDER_SIZE: i32 = 48;

/// 控件视觉状态。控件把自己的内部状态映射成它，传给图片原语决定调制（透明度/换图）。
/// 与具体控件解耦——原语只认识这套通用词汇。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VisualState {
    #[default]
    Normal,
    Hover,
    Pressed,
    Selected,
    Disabled,
}

impl VisualState {
    /// 该状态下图片的默认不透明度（禁用置灰，其余不变）。可由消费方覆盖。
    pub fn opacity(self) -> f32 {
        match self {
            VisualState::Disabled => 0.38,
            _ => 1.0,
        }
    }
}

/// 图片在控件框内的适配缩放模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// 等比缩放完整显示（可能留白）。
    #[default]
    Contain,
    /// 等比缩放铺满、裁掉溢出。
    Cover,
    /// 非等比拉伸填满。
    Fill,
    /// 原始像素 1:1（仍受 DPI 缩放）。
    None,
}

/// 图片加载/解码错误。
#[derive(Debug)]
pub enum ImageError {
    /// 文件读取失败。
    Io(std::io::Error),
    /// 无任何已注册解码器能识别该字节流。
    UnsupportedFormat,
    /// 解码器内部错误（含底层错误描述）。
    Decode(String),
    /// `from_rgba` 的像素长度与 `width*height*4` 不符。
    InvalidRgba,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Io(e) => write!(f, "图片读取失败: {e}"),
            ImageError::UnsupportedFormat => write!(f, "无可用解码器识别该图片格式"),
            ImageError::Decode(m) => write!(f, "图片解码失败: {m}"),
            ImageError::InvalidRgba => write!(f, "RGBA 数据长度与 width*height*4 不符"),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<std::io::Error> for ImageError {
    fn from(e: std::io::Error) -> Self {
        ImageError::Io(e)
    }
}

/// 解码产物：非预乘 RGBA8 像素 + 尺寸。第三方解码器（如包 image crate）以此为统一货币，
/// `Image` 内部再转 tiny-skia 的预乘格式。
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// 非预乘 RGBA8，长度须等于 `width*height*4`。
    pub rgba: Vec<u8>,
}

/// 可插拔图片解码器。核心内置 PNG；其它格式由使用方注册。
pub trait ImageDecoder {
    /// 是否能解码该字节流（通常按文件头魔数判断）。
    fn probe(&self, bytes: &[u8]) -> bool;
    /// 解码为非预乘 RGBA8。
    fn decode(&self, bytes: &[u8]) -> Result<DecodedImage, ImageError>;
    /// 诊断用名称。
    fn name(&self) -> &'static str;
}

/// 内置 PNG 解码器：走 tiny-skia 原生 `decode_png`，再反预乘为 RGBA8 入 `DecodedImage`。
struct PngDecoder;

/// PNG 文件头魔数。
const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

impl ImageDecoder for PngDecoder {
    fn probe(&self, bytes: &[u8]) -> bool {
        bytes.len() >= PNG_MAGIC.len() && &bytes[..PNG_MAGIC.len()] == PNG_MAGIC
    }
    fn decode(&self, bytes: &[u8]) -> Result<DecodedImage, ImageError> {
        let pm = Pixmap::decode_png(bytes).map_err(|e| ImageError::Decode(format!("{e}")))?;
        let (w, h) = (pm.width(), pm.height());
        let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for p in pm.pixels() {
            let c = p.demultiply();
            rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        Ok(DecodedImage {
            width: w,
            height: h,
            rgba,
        })
    }
    fn name(&self) -> &'static str {
        "png"
    }
}

/// 内置 SVG 解码器（`svg` feature）：用 resvg 解析并按 SVG 固有尺寸光栅化。
///
/// resvg 渲染后端即 tiny-skia（已与本项目对齐到同一版本）。这里仍走 `DecodedImage`
/// 原始 RGBA，与位图解码同一条入库路径，逻辑统一。
// ★ RuPix 补丁 06 —— SVG 性能路线（解析缓存 / 全局字体库 / 固有尺寸 / 区域渲染）。
/// ★ RuPix 补丁 06 —— 系统字体库（svg-text）：全局只加载一次，所有线程共享。
///
/// 曾用 thread_local——每个线程各自扫一遍全系统字体（实测 ~900ms），多线程渲染
/// 下浪费成倍放大。`fontdb::Database` 加载完成后不可变，`Arc` 共享只读是安全的。
/// 启动时可调 [`svg_warm_fonts`] 在后台线程预热带填充；[`svg_fonts_ready`] 给调用方
/// 读初始化状态（★ RuPix 补丁 023 的消费端：宿主首解据此决定单趟还是剥字渐进）。
#[cfg(feature = "svg-text")]
static SVG_FONTDB: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
    std::sync::OnceLock::new();

#[cfg(feature = "svg-text")]
fn svg_fontdb() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    SVG_FONTDB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        // ★ RuPix 补丁 023 —— 优先回放持久索引；指纹不符（装/卸字体、文件变动）
        // 才付整轮扫描，并把新索引写回。
        let cache_path = SVG_FONT_CACHE_PATH.get().cloned().flatten();
        let replayed = cache_path
            .as_deref()
            .map(|p| svg_font_cache_load(&mut db, p).is_some())
            .unwrap_or(false);
        if !replayed {
            db.load_system_fonts();
            if let Some(p) = cache_path.as_deref() {
                svg_font_cache_store(&db, p);
            }
        }
        std::sync::Arc::new(db)
    })
    .clone()
}

/// ★ RuPix 补丁 023 —— 字体库是否已初始化（含回放缓存成功的那次）。
///
/// 消费端语义：`false` = 首次解析可能要付整轮字体扫描，调用方（宿主首解）据此
/// 退回剥字渐进保住图形先上屏；`true` = 完整解析已是几十毫秒级，直接单趟上屏。
#[cfg(feature = "svg-text")]
pub fn svg_fonts_ready() -> bool {
    SVG_FONTDB.get().is_some()
}

/// ★ RuPix 补丁 023 —— 字体索引持久缓存的落点（宿主传 exe 旁 `cache\svg-fonts.bin`）。
/// 必须在**首次** SVG 解析前调用（只认第一次，之后调不动）；`None` 关闭缓存。
#[cfg(feature = "svg-text")]
static SVG_FONT_CACHE_PATH: std::sync::OnceLock<Option<std::path::PathBuf>> =
    std::sync::OnceLock::new();

#[cfg(feature = "svg-text")]
pub fn svg_set_font_cache_path(path: Option<std::path::PathBuf>) {
    let _ = SVG_FONT_CACHE_PATH.set(path);
}

/// ★ RuPix 补丁 023 —— 字体索引持久缓存：把 `load_system_fonts()` 扫出的 face
/// 索引落盘，下次进程直接回放，免去整轮系统字体扫描（本机 ~1900 面 ≈ 295ms，
/// 每会话第一张带文字的 SVG 都要等它；DirectWrite/GDI 的字体集合由 OS 进程间
/// 共享，所以别的看图器没有这笔账）。
///
/// 安全性：
/// - 缓存只承载"哪些字体文件、哪些 face"这份**索引**；回放仍经
///   `fontdb::Database::push_face_info` 重建——face 的 ID 由 fontdb 现场重发，
///   不进缓存文件，与消费方（usvg）拿到的 ID 语义自洽。
/// - 失效判据 = 指纹块：每个唯一字体文件的（路径, 大小, 修改时刻）全部吻合才
///   采用；任何一项不符 → 整份作废、现场重扫重写。
/// - 家族名的 `Language` 回放一律填 `English_UnitedStates`：fontdb 的 query
///   匹配只比较家族名**字符串**（`face.families.iter().any(|f| f.0 == name)`），
///   语言标签不参与任何决策。
/// - `Style` / `Stretch` 用显式 match 往返（不用 transmute）；枚举值越界 = 缓存
///   损坏，整份作废重扫（不做"回落 Normal"的静默近似）。
/// - 落盘失败（只读介质等）= 静默：缓存是纯提速件，写不进就退回每次现扫。
#[cfg(feature = "svg-text")]
const SVG_FONT_CACHE_MAGIC: &[u8; 7] = b"RUFXDB1";

/// 回放成功返回 `Some(())`；文件缺失 / 损坏 / 指纹不符一律 `None`（调用方重扫）。
#[cfg(feature = "svg-text")]
fn svg_font_cache_load(db: &mut resvg::usvg::fontdb::Database, path: &Path) -> Option<()> {
    use std::time::UNIX_EPOCH;

    struct Rd<'a> {
        b: &'a [u8],
    }
    impl Rd<'_> {
        fn take(&mut self, n: usize) -> Option<&[u8]> {
            if self.b.len() < n {
                None
            } else {
                let (h, t) = self.b.split_at(n);
                self.b = t;
                Some(h)
            }
        }
        fn u8(&mut self) -> Option<u8> {
            self.take(1).map(|b| b[0])
        }
        fn u16(&mut self) -> Option<u16> {
            self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
        }
        fn u32(&mut self) -> Option<u32> {
            self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        }
        fn u64(&mut self) -> Option<u64> {
            self.take(8).map(|b| {
                u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            })
        }
        fn s(&mut self) -> Option<String> {
            let n = usize::from(self.u16()?);
            std::str::from_utf8(self.take(n)?).ok().map(str::to_owned)
        }
    }

    let bytes = std::fs::read(path).ok()?;
    let mut r = Rd { b: &bytes[..] };
    if r.take(SVG_FONT_CACHE_MAGIC.len())? != &SVG_FONT_CACHE_MAGIC[..] {
        return None;
    }

    // 文件布局：magic + face 区（在前）+ 指纹区（在后），顺序读写。
    let face_count = r.u32()?;
    // 计数字段来自文件、可能是垃圾：预分配只是提示，上限夹紧防 OOM；
    // 真实长度由随后的越界读取兜底（读不到就是 None）。
    let mut faces = Vec::with_capacity((face_count as usize).min(65536));
    for _ in 0..face_count {
        let file = std::path::PathBuf::from(r.s()?);
        let index = r.u32()?;
        let family_count = r.u16()?;
        let mut families = Vec::with_capacity(usize::from(family_count).min(1024));
        for _ in 0..family_count {
            families.push((r.s()?, resvg::usvg::fontdb::Language::English_UnitedStates));
        }
        let post_script_name = r.s()?;
        let style = match r.u16()? {
            0 => resvg::usvg::fontdb::Style::Normal,
            1 => resvg::usvg::fontdb::Style::Italic,
            2 => resvg::usvg::fontdb::Style::Oblique,
            _ => return None,
        };
        let weight = resvg::usvg::fontdb::Weight(r.u16()?);
        let stretch = match r.u16()? {
            1 => resvg::usvg::fontdb::Stretch::UltraCondensed,
            2 => resvg::usvg::fontdb::Stretch::ExtraCondensed,
            3 => resvg::usvg::fontdb::Stretch::Condensed,
            4 => resvg::usvg::fontdb::Stretch::SemiCondensed,
            5 => resvg::usvg::fontdb::Stretch::Normal,
            6 => resvg::usvg::fontdb::Stretch::SemiExpanded,
            7 => resvg::usvg::fontdb::Stretch::Expanded,
            8 => resvg::usvg::fontdb::Stretch::ExtraExpanded,
            9 => resvg::usvg::fontdb::Stretch::UltraExpanded,
            _ => return None,
        };
        let monospaced = match r.u8()? {
            0 => false,
            1 => true,
            _ => return None,
        };
        faces.push((file, index, families, post_script_name, style, weight, stretch, monospaced));
    }

    // 指纹区：每个唯一字体文件逐一核对（大小 + 修改时刻），一项不符整份作废。
    let file_count = r.u32()?;
    let mut files = Vec::with_capacity((file_count as usize).min(65536));
    for _ in 0..file_count {
        let file = std::path::PathBuf::from(r.s()?);
        let size = r.u64()?;
        let (secs, nanos) = (r.u64()?, r.u32()?);
        files.push((file, size, secs, nanos));
    }
    if !r.b.is_empty() {
        return None;
    }
    for (file, size, secs, nanos) in &files {
        let m = std::fs::metadata(file).ok()?;
        let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        if m.len() != *size || t.as_secs() != *secs || t.subsec_nanos() != *nanos {
            return None;
        }
    }

    for (file, index, families, post_script_name, style, weight, stretch, monospaced) in faces {
        db.push_face_info(resvg::usvg::fontdb::FaceInfo {
            id: resvg::usvg::fontdb::ID::dummy(),
            source: resvg::usvg::fontdb::Source::File(file),
            index,
            families,
            post_script_name,
            style,
            weight,
            stretch,
            monospaced,
        });
    }
    Some(())
}
#[cfg(feature = "svg-text")]
fn svg_font_cache_store(db: &resvg::usvg::fontdb::Database, path: &Path) {
    use std::time::UNIX_EPOCH;

    let mut out = Vec::new();
    out.extend_from_slice(SVG_FONT_CACHE_MAGIC);
    fn put_u16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u64(out: &mut Vec<u8>, v: u64) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u8(out: &mut Vec<u8>, v: u8) {
        out.push(v);
    }
    fn put_str(out: &mut Vec<u8>, s: &str) {
        put_u16(out, s.len() as u16);
        out.extend_from_slice(s.as_bytes());
    }

    // 同一 TTC 的多个 face 引用同一文件；指纹按唯一文件去重（BTreeMap 还顺带定序，
    // 缓存字节只取决于文件集合本身）。
    let mut seen_files: std::collections::BTreeMap<std::path::PathBuf, ()> =
        std::collections::BTreeMap::new();
    let faces: Vec<_> = db
        .faces()
        .filter(|f| matches!(f.source, resvg::usvg::fontdb::Source::File(_)))
        .collect();

    put_u32(&mut out, faces.len() as u32);
    for f in &faces {
        let file = match &f.source {
            resvg::usvg::fontdb::Source::File(p) => p,
            _ => unreachable!(),
        };
        seen_files.insert(file.clone(), ());
        put_str(&mut out, &file.to_string_lossy());
        put_u32(&mut out, f.index);
        put_u16(&mut out, f.families.len() as u16);
        for (name, _) in &f.families {
            put_str(&mut out, name);
        }
        put_str(&mut out, &f.post_script_name);
        put_u16(
            &mut out,
            match f.style {
                resvg::usvg::fontdb::Style::Normal => 0,
                resvg::usvg::fontdb::Style::Italic => 1,
                resvg::usvg::fontdb::Style::Oblique => 2,
            },
        );
        put_u16(&mut out, f.weight.0);
        put_u16(
            &mut out,
            match f.stretch {
                resvg::usvg::fontdb::Stretch::UltraCondensed => 1,
                resvg::usvg::fontdb::Stretch::ExtraCondensed => 2,
                resvg::usvg::fontdb::Stretch::Condensed => 3,
                resvg::usvg::fontdb::Stretch::SemiCondensed => 4,
                resvg::usvg::fontdb::Stretch::Normal => 5,
                resvg::usvg::fontdb::Stretch::SemiExpanded => 6,
                resvg::usvg::fontdb::Stretch::Expanded => 7,
                resvg::usvg::fontdb::Stretch::ExtraExpanded => 8,
                resvg::usvg::fontdb::Stretch::UltraExpanded => 9,
            },
        );
        put_u8(&mut out, u8::from(f.monospaced));
    }

    put_u32(&mut out, seen_files.len() as u32);
    for (file, _) in &seen_files {
        let m = match std::fs::metadata(file) {
            Ok(m) => m,
            Err(_) => return, // 扫描与落盘之间文件被移走：宁可不用缓存
        };
        let (secs, nanos) = match m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        {
            Some(d) => (d.as_secs(), d.subsec_nanos()),
            None => (0, 0),
        };
        put_str(&mut out, &file.to_string_lossy());
        put_u64(&mut out, m.len());
        put_u64(&mut out, secs);
        put_u32(&mut out, nanos);
    }

    // 写临时 + 原子改名：断电也不留半份缓存（坏了的旧缓存下次读失败 → 重扫）。
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("bin.tmp");
    if std::fs::write(&tmp, &out).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// ★ RuPix 补丁 017 —— 这份 SVG 到底要不要系统字体库。
///
/// 补丁 06 把字体库收成全局 `OnceLock`，是为了消灭"每个线程各扫一遍全系统字体
/// （实测 ~900ms）"；可它挂在解析入口上**无条件**取值，于是任何一次 SVG 解析都
/// 要先等整轮字体扫描——**包括窗口图标、托盘图标这种一个文字都没有的矢量图**。
/// RuPix 实测这一段把约 300ms 压在"双击→窗口出现"的关键路径上（正本
/// 本仓历史记录 §三，端点读数：空启动 437→133ms、小图首帧 433→133ms）。
///
/// 判据只看字节流里有没有 text 元素（`<text` / 带命名空间的 `:text`）。**gzip 包装
/// （.svgz）看不见内容，一律当"有文字"**：判成"有"只是多付一次早已全局化的扫描，
/// 判成"没有"会悄悄把字形丢掉——两个方向的代价不对等，所以宁可错付。
#[cfg(all(feature = "svg", feature = "svg-text"))]
fn svg_may_have_text(bytes: &[u8]) -> bool {
    fn has(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
    }
    // gzip 魔数：resvg 内部透明解压，这里无从判断内容。
    bytes.starts_with(&[0x1f, 0x8b]) || has(bytes, b"<text") || has(bytes, b":text")
}

/// ★ RuPix 补丁 022 —— 渐进首绘的第一趟：把 `<text>` 元素从字节流里剥掉。
///
/// 起因（RuPix m183/m184 实测）：usvg 在**解析期**就把 text 转成轮廓，每份带文字的
/// 文档固定 ~390 ms + 每段 ~3–20 ms，首开卡顿都压在这一步；换小字体库不省（族命中
/// 的文档照样 407 ms）。剥掉 text 后解析只花十几毫秒——首屏先画"没有字的图形"，
/// 完整版随后补上。
///
/// 返回 `None` 表示"没有可剥的"或"不敢剥"，调用方原样走完整解析：
/// - 字节里根本没有可识别的 `<text`（剥与不剥同键，省一次重复入缓存）；
/// - gzip 包装（.svgz），内容看不见，不猜；
/// - 找到 text 但收尾 `</text>` 丢了——文档形态超出本函数的识别能力，保守交给全趟。
///
/// 只认不带命名空间前缀的 `<text`（后跟空白或 `>`，`<textPath` 不算）：usvg 只把
/// SVG 命名空间里的 text 当文字，带前缀的（`<sv:text>`）它也不渲染，剥了是白剥。
/// 注释/CDATA 里恰好出现 `<text>…</text>` 会被误剥——只影响首屏那一帧的图形内容，
/// 且不产生非法 XML（删的是完整元素），随后会被完整版覆盖。
///
/// 公开给两段式调用方：先剥再量固有尺寸再渲染（量尺寸那趟也要吃到剥字的便宜）。
#[cfg(feature = "svg")]
pub fn svg_strip_text_nodes(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return None;
    }
    let needle = b"<text";
    let close = b"</text>";
    let mut out = Vec::with_capacity(bytes.len());
    let mut stripped = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].len() >= needle.len()
            && bytes[i..i + needle.len()].eq_ignore_ascii_case(needle)
            && matches!(
                bytes.get(i + needle.len()),
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'>')
            )
        {
            // 找到这个元素的结束：先看它是不是自闭合。
            let mut j = i + needle.len();
            let mut self_closing = false;
            while j < bytes.len() {
                if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'>') {
                    self_closing = true;
                    break;
                }
                if bytes[j] == b'>' {
                    break;
                }
                j += 1;
            }
            if self_closing {
                i = (j + 2).min(bytes.len());
                stripped = true;
                continue;
            }
            // 非自闭合：找配对的收尾标签。
            let mut k = j;
            let mut found = false;
            while k + close.len() <= bytes.len() {
                if bytes[k..k + close.len()].eq_ignore_ascii_case(close) {
                    found = true;
                    break;
                }
                k += 1;
            }
            if found {
                i = k + close.len();
                stripped = true;
                continue;
            }
            // 收尾丢了：整段后面不剥，保守交给全趟。
            return None;
        }
        out.push(bytes[i]);
        i += 1;
    }
    stripped.then_some(out)
}

/// ★ RuPix 补丁 06 —— 预热系统字体库（`svg-text` feature）：启动时在后台线程调用一次，
/// 首张带文字的 SVG 就不必现场等整轮字体扫描。
#[cfg(feature = "svg-text")]
pub fn svg_warm_fonts() {
    let _ = svg_fontdb();
}

/// ★ RuPix 补丁 06 —— 解析 SVG（带缓存）：`usvg::Tree` 解析完成即不可变，`Arc` 共享只读渲染安全。
///
/// 实测（含文字、字体缺失回退的 SVG）：parse ≈ 340ms，render ≈ 12ms——看图器
/// 每次缩放/平移都重渲，不缓存的话 95% 时间浪费在重复解析同一份字节上。
/// 键为字节内容哈希 + 长度（同一文件的重复解析必然命中）；容量满即整体清空
/// （树很小，粗暴淘汰足够）。字体库全局唯一，不参与键。
#[cfg(feature = "svg")]
fn svg_tree_cached(bytes: &[u8]) -> Result<std::sync::Arc<resvg::usvg::Tree>, ImageError> {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Arc, Mutex};

    static CACHE: Mutex<Option<HashMap<(u64, usize), Arc<resvg::usvg::Tree>>>> = Mutex::new(None);
    static CAP: usize = 8;

    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let key = (h.finish(), bytes.len());
    if let Ok(map) = CACHE.lock() {
        if let Some(t) = map.as_ref().and_then(|m| m.get(&key)) {
            return Ok(t.clone());
        }
    }
    let mut opt = resvg::usvg::Options::default();
    // ★ RuPix 补丁 017 —— 无文字的文档不碰字体库（`Options::default()` 里那份空库
    // 足够解析纯图形 SVG）；带文字的照旧共享全局那一份。
    #[cfg(feature = "svg-text")]
    if svg_may_have_text(bytes) {
        opt.fontdb = svg_fontdb();
    }
    let tree = Arc::new(
        resvg::usvg::Tree::from_data(bytes, &opt)
            .map_err(|e| ImageError::Decode(format!("SVG 解析失败: {e}")))?,
    );
    if let Ok(mut slot) = CACHE.lock() {
        let m = slot.get_or_insert_with(HashMap::new);
        if m.len() >= CAP {
            m.clear();
        }
        m.insert(key, tree.clone());
    }
    Ok(tree)
}

#[cfg(feature = "svg")]
struct SvgDecoder;

#[cfg(feature = "svg")]
impl SvgDecoder {
    /// 解析 + 光栅化为非预乘 RGBA8。`target_width=None` 用 SVG 固有尺寸；
    /// `Some(w)` 按该宽度等比缩放光栅（高度随宽高比，至少 1px）。
    fn rasterize(bytes: &[u8], target_width: Option<u32>) -> Result<DecodedImage, ImageError> {
        use resvg::tiny_skia;

        let tree = svg_tree_cached(bytes)?;
        let size = tree.size();
        let (iw, ih) = (size.width(), size.height());
        if iw <= 0.0 || ih <= 0.0 {
            return Err(ImageError::Decode("SVG 固有尺寸非法".into()));
        }
        // 目标像素尺寸：指定宽度则等比缩放，否则取固有尺寸（向上取整保证不丢边）。
        let (tw, th) = match target_width {
            Some(w) if w > 0 => {
                let scale = w as f32 / iw;
                (w, (ih * scale).round().max(1.0) as u32)
            }
            _ => (iw.ceil() as u32, ih.ceil() as u32),
        };
        let mut pm = tiny_skia::Pixmap::new(tw, th)
            .ok_or_else(|| ImageError::Decode("SVG 目标尺寸过大或非法".into()))?;
        let transform = tiny_skia::Transform::from_scale(tw as f32 / iw, th as f32 / ih);
        resvg::render(&tree, transform, &mut pm.as_mut());

        // resvg 产物为预乘 RGBA8，反预乘成 DecodedImage 货币。
        let (w, h) = (pm.width(), pm.height());
        let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for p in pm.pixels() {
            let c = p.demultiply();
            rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        Ok(DecodedImage {
            width: w,
            height: h,
            rgba,
        })
    }
}

#[cfg(feature = "svg")]
impl ImageDecoder for SvgDecoder {
    fn probe(&self, bytes: &[u8]) -> bool {
        // svgz：gzip 包装的 SVG（usvg::Tree::from_data 透明解压）。
        if bytes.starts_with(&[0x1f, 0x8b]) {
            return true;
        }
        // 纯文本 SVG：前 1KB 内出现 `<svg`（容忍 BOM/XML 声明/注释/大小写）。
        let n = bytes.len().min(1024);
        bytes[..n]
            .windows(4)
            .any(|w| w.eq_ignore_ascii_case(b"<svg"))
    }
    fn decode(&self, bytes: &[u8]) -> Result<DecodedImage, ImageError> {
        Self::rasterize(bytes, None)
    }
    fn name(&self) -> &'static str {
        "svg"
    }
}

thread_local! {
    /// 解码器注册表，PNG 预置（启用 `svg` feature 时追加 SVG）。后注册者优先级更高（覆盖在前）。
    static DECODERS: RefCell<Vec<Box<dyn ImageDecoder>>> = RefCell::new({
        #[allow(unused_mut)]
        let mut v: Vec<Box<dyn ImageDecoder>> = vec![Box::new(PngDecoder)];
        #[cfg(feature = "svg")]
        v.push(Box::new(SvgDecoder));
        v
    });
}

/// 注册一个额外解码器（如包 `image` crate 的 JPEG/WebP 解码器）。
///
/// 后注册者在 `from_bytes` 嗅探时优先匹配，可覆盖内置实现。
pub fn register_decoder(decoder: Box<dyn ImageDecoder>) {
    DECODERS.with(|r| r.borrow_mut().push(decoder));
}

/// 用注册表嗅探并解码：从后往前找首个 `probe` 命中的解码器。
fn decode_with_registry(bytes: &[u8]) -> Result<DecodedImage, ImageError> {
    DECODERS.with(|r| {
        let decoders = r.borrow();
        for d in decoders.iter().rev() {
            if d.probe(bytes) {
                return d.decode(bytes);
            }
        }
        Err(ImageError::UnsupportedFormat)
    })
}

/// 图片身份 id 的全局分配器。单调递增、**永不复用**——这一点是正确性前提，见
/// [`Image::cache_id`]。从 1 开始，0 留作"未分配"的哨兵值（当前无人用，留给将来）。
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

/// 解码后的图片资源。`Rc` 共享，克隆廉价；paint 期只做 blit，不再解码。
#[derive(Clone)]
pub struct Image {
    pixmap: Rc<Pixmap>,
    /// 身份 id：见 [`Image::cache_id`]。随 `Clone` 复制，故同一张图的所有克隆共享它。
    id: u64,
    w: u32,
    h: u32,
    /// ★ RuPix 补丁 011 —— 放大时是否禁用插值，见 [`Image::with_pixelated`]。
    pixelated: bool,
    /// ★ RuPix 补丁 015 —— 全部像素 alpha == 255。真时软路径的 1:1 绘制可走
    /// 逐行拷贝而不必逐像素 src-over（源不透明时两者的字节结果恒等）。
    /// 由构造处一次算出（预乘循环里顺手记，见 [`Image::premultiply_rgba`]）。
    opaque: bool,
}

impl Image {
    /// 从文件读取并解码（按字节嗅探格式，自适配已注册的解码器）。
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ImageError> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// 从字节流解码：嗅探格式后分发给匹配的解码器。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ImageError> {
        let decoded = decode_with_registry(bytes)?;
        Self::from_decoded(decoded)
    }

    /// 便捷直通 PNG：跳过注册表嗅探，直接走 tiny-skia 原生解码（无反预乘往返）。
    pub fn from_png_bytes(bytes: &[u8]) -> Result<Self, ImageError> {
        let pm = Pixmap::decode_png(bytes).map_err(|e| ImageError::Decode(format!("{e}")))?;
        Ok(Self::from_pixmap(pm))
    }

    /// 从 SVG 字节光栅化（`svg` feature）。`target_width=None` 用 SVG 固有尺寸；
    /// `Some(w)` 按该宽度等比光栅——HiDPI 求清晰可传 2× 逻辑宽度。
    ///
    /// `from_bytes`/`from_file` 已能自动识别 `.svg` 并按固有尺寸光栅化；本方法是想
    /// 显式控制光栅分辨率时的出口。含文字的 SVG 因未启用 resvg 文字特性而不渲染文字。
    #[cfg(feature = "svg")]
    pub fn from_svg_bytes(bytes: &[u8], target_width: Option<u32>) -> Result<Self, ImageError> {
        Self::from_decoded(SvgDecoder::rasterize(bytes, target_width)?)
    }

    /// ★ RuPix 补丁 022 —— 渐进首绘：剥掉 `<text>` 后光栅化（`svg` feature）。
    ///
    /// 带 CJK 文字的 SVG 解析要 ~400 ms（usvg 在解析期把 text 转成轮廓），剥掉后只
    /// 十几毫秒；看图器用它先出"只有图形"的第一帧，随后再用
    /// [`Image::from_svg_bytes`] 补完整版。没有文字 / gzip 包装 / 形态异常时结果与
    /// `from_svg_bytes` **完全一致**——调用方无需预先判断文档里有没有文字。
    #[cfg(feature = "svg")]
    pub fn from_svg_bytes_no_text(
        bytes: &[u8],
        target_width: Option<u32>,
    ) -> Result<Self, ImageError> {
        match svg_strip_text_nodes(bytes) {
            Some(stripped) => Self::from_decoded(SvgDecoder::rasterize(&stripped, target_width)?),
            None => Self::from_svg_bytes(bytes, target_width),
        }
    }

    /// ★ RuPix 补丁 06 —— 只解析不渲染：取 SVG 的固有尺寸（viewBox/width/height 声明）。
    ///
    /// 供「先量尺寸、再按目标尺寸渲染一次」的两段式调用方用——解析远比渲染便宜，
    /// 替代「先按固有尺寸完整渲染一遍只为拿宽高」的浪费。
    #[cfg(feature = "svg")]
    pub fn svg_intrinsic_size(bytes: &[u8]) -> Result<(u32, u32), ImageError> {
        let tree = svg_tree_cached(bytes)?;
        let size = tree.size();
        if size.width() <= 0.0 || size.height() <= 0.0 {
            return Err(ImageError::Decode("SVG 固有尺寸非法".into()));
        }
        Ok((size.width().ceil() as u32, size.height().ceil() as u32))
    }

    /// ★ RuPix 补丁 06 —— SVG 局部光栅化（`svg` feature）：只渲染图像坐标系里 `origin..origin+size`
    /// 的矩形，输出密度 `density`（设备像素/图像像素）。
    ///
    /// 看图器的「可视区域 1:1」路线专用：无论缩放多大，每次都只渲一屏大小的位图，
    /// 清晰度与内存都和倍率无关（整图光栅化在超大头寸下必然撞硬顶出马赛克）。
    #[cfg(feature = "svg")]
    pub fn from_svg_region(
        bytes: &[u8],
        origin: (f32, f32),
        size: (f32, f32),
        density: f32,
    ) -> Result<Self, ImageError> {
        use resvg::tiny_skia;

        let t0 = std::time::Instant::now();
        let tree = svg_tree_cached(bytes)?;
        let t_parse = t0.elapsed();
        let k = density.max(f32::EPSILON);
        let pw = ((size.0 * k).ceil() as u32).max(1);
        let ph = ((size.1 * k).ceil() as u32).max(1);
        let mut pm = tiny_skia::Pixmap::new(pw, ph)
            .ok_or_else(|| ImageError::Decode("SVG 目标尺寸过大或非法".into()))?;
        // p' = k·(p − origin)：把可视区域平移到原点再按密度放大。
        let t = tiny_skia::Transform::from_row(k, 0.0, 0.0, k, -origin.0 * k, -origin.1 * k);
        resvg::render(&tree, t, &mut pm.as_mut());
        log::debug!(
            "svg region：{}×{} parse={}ms render={}ms",
            pw,
            ph,
            t_parse.as_millis(),
            (t0.elapsed() - t_parse).as_millis()
        );
        let mut rgba = Vec::with_capacity((pw as usize) * (ph as usize) * 4);
        for p in pm.pixels() {
            let c = p.demultiply();
            rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        Self::from_rgba(pw, ph, &rgba)
    }

    /// 从原始非预乘 RGBA8 构造。`rgba.len()` 须等于 `width*height*4`。
    ///
    /// ★ RuPix 补丁 012 —— 预乘直接读调用方的切片，不再先把源 `to_vec()` 拷一份。
    /// 改前这一段里同时存在**源 + 等长拷贝 + 预乘目标**三份字节；拷贝对结果一个字节
    /// 都不影响，纯属过路。6055²（139.9 MB）实测整次上传 124.4 ms → 92.6 ms（−26%），
    /// 并少一次 139.9 MB 的瞬时分配。**输出字节与改前逐位相同**（判据见
    /// `from_rgba_matches_premultiply_reference_per_byte`）。
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::InvalidRgba);
        }
        let expect = (width as usize) * (height as usize) * 4;
        if rgba.len() != expect {
            return Err(ImageError::InvalidRgba);
        }
        let size = IntSize::from_wh(width, height).ok_or(ImageError::InvalidRgba)?;
        let (bytes, opaque) = Self::premultiply_rgba(rgba);
        let pm = Pixmap::from_vec(bytes, size).ok_or(ImageError::InvalidRgba)?;
        Ok(Self::from_pixmap_parts(pm, opaque))
    }

    /// 把非预乘 RGBA8 转为 tiny-skia 预乘位图的字节。
    ///
    /// ★ RuPix 补丁 012 —— 这条循环是 `from_rgba`（借用的切片）与 `from_decoded`
    /// （解码器交来的owned 缓冲）**唯一**的预乘入口：两处各自写一遍迟早分叉出色差。
    /// 刻意保留 `with_capacity` + 逐像素写，不用 `vec![0; n]` 预置零——后者要先把
    /// 整块目标 memset 一遍，140 MB 量级下那一次清零就是几毫秒的白账。
    ///
    /// ★ RuPix 补丁 015 —— 顺手记下"是否全不透明"（返回值第二项）。单独再扫一遍
    /// 6055² 实测 13.7 ms，而这里本来就要读每个像素的 alpha；判据与
    /// [`Image::is_opaque`] 的消费者（1:1 拷贝快路）严格同源。
    fn premultiply_rgba(rgba: &[u8]) -> (Vec<u8>, bool) {
        let mut data = Vec::with_capacity(rgba.len());
        let mut opaque = true;
        for px in rgba.as_chunks::<4>().0 {
            opaque &= px[3] == 255;
            let c = ColorU8::from_rgba(px[0], px[1], px[2], px[3]).premultiply();
            data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        (data, opaque)
    }

    /// 把非预乘 RGBA8 转为预乘 Pixmap。
    fn from_decoded(d: DecodedImage) -> Result<Self, ImageError> {
        let size = IntSize::from_wh(d.width, d.height).ok_or(ImageError::InvalidRgba)?;
        let (bytes, opaque) = Self::premultiply_rgba(&d.rgba);
        let pm = Pixmap::from_vec(bytes, size).ok_or(ImageError::InvalidRgba)?;
        Ok(Self::from_pixmap_parts(pm, opaque))
    }

    fn from_pixmap(pm: Pixmap) -> Self {
        let opaque = pm.data().chunks_exact(4).all(|c| c[3] == 255);
        Self::from_pixmap_parts(pm, opaque)
    }

    fn from_pixmap_parts(pm: Pixmap, opaque: bool) -> Self {
        let (w, h) = (pm.width(), pm.height());
        Self {
            pixmap: Rc::new(pm),
            id: NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
            w,
            h,
            pixelated: false,
            opaque,
        }
    }

    /// ★ RuPix 补丁 011 —— 声明「放大时保持像素硬边」：目标尺寸超出源像素时改用最近邻，
    /// 而不是被双线性抹成一团糊。
    ///
    /// 存在的理由是**看图器的高倍放大**：解码侧已经交付原图档位（1 源像素 = 1 位图
    /// 像素），上屏框却比它大若干倍，这段放大原本恒用双线性插值——于是像素格永远
    /// 看不到棱角，无论怎么放大都只是糊面。这是渲染滤镜的限制，与解码质量无关。
    ///
    /// 做成**逐图 opt-in 而不是全局规则**：全局「放大即用最近邻」会把非整数 DPI 下的
    /// 矢量图标一起带进最近邻（18dp 图标在 125% 下物理 22.5px 即算放大），UI 全线变锯齿。
    /// 降采样方向不受本标志影响（仍用双线性），所以缩略图即便误置也不会变成锯齿——
    /// 语义严格限定为「不得凭空造出中间色」。
    #[must_use]
    pub fn with_pixelated(mut self, on: bool) -> Self {
        self.pixelated = on;
        self
    }

    /// 是否声明了放大禁用插值（见 [`Image::with_pixelated`]）。
    #[must_use]
    pub fn pixelated(&self) -> bool {
        self.pixelated
    }

    /// 固有逻辑尺寸（1 图片像素 = 1 逻辑 dp，再由 DPI 缩放）。
    pub fn size(&self) -> Size {
        Size::new(self.w as i32, self.h as i32)
    }

    /// 模板着色副本：rgb 替换为 `color`，alpha 乘以 `color.a`（保形）。
    /// 用于单色图标随主题/状态变色（彩色图请勿用，会丢失原色）。
    pub fn tinted(&self, color: Color) -> Image {
        let size = IntSize::from_wh(self.w, self.h).expect("尺寸已在构造时校验");
        let mut data = Vec::with_capacity((self.w as usize) * (self.h as usize) * 4);
        for p in self.pixmap.pixels() {
            // p 为预乘像素，alpha 即覆盖度；按模板着色重建直色后再预乘。
            let a = ((p.alpha() as u16 * color.a as u16) / 255) as u8;
            let c = ColorU8::from_rgba(color.r, color.g, color.b, a).premultiply();
            data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        let pm = Pixmap::from_vec(data, size).expect("着色 Pixmap 尺寸匹配");
        // 像素内容与 `self` 不同，必须是**新身份**：沿用 `self.id` 会让渲染后端把
        // 着色前的位图当成着色后的用。
        Self {
            pixmap: Rc::new(pm),
            id: NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
            w: self.w,
            h: self.h,
            // ★ RuPix 补丁 011 —— 着色只换颜色不改「放大不得插值」的声明。
            pixelated: self.pixelated,
            // ★ RuPix 补丁 015 —— 着色把 alpha 乘上 `color.a`：只有原图与着色都
            // 不透明时结果才全 255，否则 1:1 拷贝快路必须让位给 src-over。
            opaque: self.opaque && color.a == 255,
        }
    }

    /// ★ RuPix 补丁 015 —— 全部像素 alpha 恒为 255（软路径 1:1 拷贝快路的判据）。
    pub(crate) fn is_opaque(&self) -> bool {
        self.opaque
    }

    /// 像素宽。
    pub fn width(&self) -> u32 {
        self.w
    }
    /// 像素高。
    pub fn height(&self) -> u32 {
        self.h
    }

    /// 导出**非预乘** RGBA8（行优先，长度 `width * height * 4`）。
    ///
    /// 后端 `Pixmap` 存的是预乘像素，这里逐像素反预乘还原。窗口图标
    /// （[`WindowIcon::from_image`](crate::icon::WindowIcon::from_image)）与平台
    /// 位图 API 要的都是非预乘，这是它们的入口。
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity((self.w as usize) * (self.h as usize) * 4);
        for p in self.pixmap.pixels() {
            let c = p.demultiply();
            out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        out
    }

    /// 后端 Pixmap 引用（供渲染层 blit）。
    pub(crate) fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }

    /// 稳定缓存键：构造时分配的**单调递增 id**。同一张图的 `Clone` 共享它，两张不同的
    /// 图永不共享（供渲染后端按图片身份缓存 device-dependent 位图/纹理，免每帧重建）。
    ///
    /// # 为什么不是 `Rc::as_ptr`
    ///
    /// 这里曾经用底层 `Rc<Pixmap>` 的指针值。指针只在对象活着时唯一：图片一旦释放，
    /// 下一次分配可能**复用同一地址**，于是新图命中一条本属于旧图的缓存条目，画出的是
    /// 上一张图——ABA。当时的取舍写着"UI 图片随窗口常驻，不会释放"，这个前提被 DPI
    /// 感知的 SVG 打破了：[`ImageContent`](crate::ui::ImageContent) 在 DPI 变化时会
    /// 重新光栅每一个图标，旧 `Pixmap` 依次释放、新 `Pixmap` 依次分配，而同一批图标
    /// 尺寸相同（同一分配器 size class），复用地址是常态而非偶然。表现为跨不同 DPI 的
    /// 屏幕拖动窗口后，某个图标画成了另一个图标；下一次重绘又可能恢复。
    ///
    /// 单调递增的 id 从根上消除这一类，代价只是每张图多 8 字节。
    ///
    /// 消费者是 d2d / gpu 后端；其余平台+feature 组合下无人使用，显式放行 dead_code。
    #[cfg_attr(not(any(all(windows, feature = "d2d"), gpu_backend)), allow(dead_code))]
    pub(crate) fn cache_id(&self) -> u64 {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张 w×h 纯色 PNG 的字节（红色不透明）。
    fn red_png(w: u32, h: u32) -> Vec<u8> {
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        pm.encode_png().unwrap()
    }

    /// 身份 id 必须**单调递增**：一张图释放后新建的图不得拿到旧 id。
    ///
    /// 这条曾经不成立——`cache_id` 取的是 `Rc<Pixmap>` 指针，`a` 释放后 `b` 复用同一
    /// 地址就会相等，渲染后端于是把 `a` 的位图当成 `b` 画出来。
    ///
    /// 注意它是**契约护栏而非捕获判据**：实测把实现改回指针时单次 drop 未必复用地址，
    /// 本测试仍会绿。真正能抓住那个缺陷的是下面的 `cache_id_unique_across_realloc_churn`
    /// 与 `ui::image` 的 `svg_rerasterize_never_reuses_cache_id`（两者在指针实现下确认变红）。
    #[test]
    fn cache_id_never_reused_after_drop() {
        let a = Image::from_png_bytes(&red_png(18, 18)).unwrap();
        let old = a.cache_id();
        drop(a);
        let b = Image::from_png_bytes(&red_png(18, 18)).unwrap();
        assert!(
            b.cache_id() > old,
            "新图的 cache_id({}) 应严格大于已释放图的 {old}",
            b.cache_id()
        );
    }

    /// 复刻现场：DPI 变化时每个图标依次「释放旧光栅、分配新光栅」，尺寸全都相同。
    /// 指针作键时分配器几乎必然复用地址，这里会收到重复 id。
    #[test]
    fn cache_id_unique_across_realloc_churn() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let img = Image::from_png_bytes(&red_png(18, 18)).unwrap();
            assert!(
                seen.insert(img.cache_id()),
                "cache_id {} 重复——释放后的身份被复用了",
                img.cache_id()
            );
            // 立刻释放，把这块内存还给分配器供下一轮复用。
            drop(img);
        }
    }

    /// 同一张图的克隆共享身份：否则每次 `clone` 都会让后端重传一遍纹理。
    #[test]
    fn cache_id_shared_by_clone() {
        let a = Image::from_png_bytes(&red_png(4, 4)).unwrap();
        assert_eq!(a.clone().cache_id(), a.cache_id());
    }

    /// 着色产生新像素，必须是新身份——沿用原 id 会让后端画出**未着色**的那张。
    #[test]
    fn tinted_gets_new_cache_id() {
        let a = Image::from_png_bytes(&red_png(4, 4)).unwrap();
        let b = a.tinted(Color::rgb(0, 0, 255));
        assert_ne!(a.cache_id(), b.cache_id());
    }

    /// ★ RuPix 补丁 012 的判据：`from_rgba` 不再先把源 `to_vec()` 拷一份再预乘，所以
    /// 它必须与"解码器交来 owned 缓冲"那条路**逐字节相同**——少掉的那次拷贝一个字节
    /// 都不许影响。四格专挑能暴露差错的：全透明、不透明（RGB 应原样）、半透明（应被压小）、
    /// 极小 alpha（最容易在舍入上分叉）。
    #[test]
    fn from_rgba_matches_premultiply_reference_per_byte() {
        let rgba: Vec<u8> = vec![
            10, 20, 30, 0, // 全透明 → 预乘后四字节全 0
            255, 128, 64, 255, // 不透明 → RGB 原样
            255, 128, 64, 128, // 半透明 → R 必须被压小
            1, 2, 3, 1, // 极小 alpha → 不许舍成 255 那档
        ];
        let borrowed = Image::from_rgba(2, 2, &rgba).unwrap();
        let owned = Image::from_decoded(DecodedImage {
            width: 2,
            height: 2,
            rgba: rgba.clone(),
        })
        .unwrap();
        assert_eq!(
            borrowed.pixmap.data(),
            owned.pixmap.data(),
            "补丁 012 少掉的 to_vec() 不许改变任何一个字节"
        );
        let d = borrowed.pixmap.data();
        assert_eq!(&d[0..4], &[0, 0, 0, 0], "alpha=0 预乘后必须全 0");
        assert_eq!(&d[4..8], &[255, 128, 64, 255], "alpha=255 时 RGB 不许动");
        assert!(
            (127..=131).contains(&d[8]),
            "alpha=128 的 R 应被预乘成 ≈129，实得 {}",
            d[8]
        );
        assert_eq!(d[11], 128, "预乘不许改 alpha 本身");
        assert!(d[12] <= 1, "alpha=1 时 R 只能≈0，实得 {}", d[12]);
    }

    #[test]
    fn from_png_bytes_reports_size() {
        let png = red_png(4, 3);
        let img = Image::from_png_bytes(&png).unwrap();
        assert_eq!(img.size(), Size::new(4, 3));
    }

    #[test]
    fn from_bytes_sniffs_png() {
        let png = red_png(2, 2);
        let img = Image::from_bytes(&png).unwrap();
        assert_eq!(img.size(), Size::new(2, 2));
    }

    #[test]
    fn from_bytes_rejects_unknown_format() {
        let junk = [0u8, 1, 2, 3, 4, 5, 6, 7];
        assert!(matches!(
            Image::from_bytes(&junk),
            Err(ImageError::UnsupportedFormat)
        ));
    }

    #[test]
    fn from_rgba_validates_length() {
        // 2×2×4 = 16 字节，正确。
        let ok = Image::from_rgba(2, 2, &[0u8; 16]);
        assert!(ok.is_ok());
        // 长度不符。
        assert!(matches!(
            Image::from_rgba(2, 2, &[0u8; 15]),
            Err(ImageError::InvalidRgba)
        ));
        // 零尺寸。
        assert!(matches!(
            Image::from_rgba(0, 2, &[]),
            Err(ImageError::InvalidRgba)
        ));
    }

    #[test]
    fn from_rgba_preserves_size() {
        let img = Image::from_rgba(5, 7, &[128u8; 5 * 7 * 4]).unwrap();
        assert_eq!(img.size(), Size::new(5, 7));
    }

    #[test]
    fn tinted_recolors_keeping_alpha() {
        // 白色不透明 + 半透明像素混合源。
        let src = Image::from_rgba(2, 1, &[255, 255, 255, 255, 255, 255, 255, 128]).unwrap();
        let red = src.tinted(Color::rgb(255, 0, 0));
        assert_eq!(red.size(), Size::new(2, 1));
        // 取回非预乘验证：第 1 像素红不透明，第 2 像素红半透明。
        let pm = red.pixmap();
        let p0 = pm.pixel(0, 0).unwrap().demultiply();
        assert_eq!(
            (p0.red(), p0.green(), p0.blue(), p0.alpha()),
            (255, 0, 0, 255)
        );
        let p1 = pm.pixel(1, 0).unwrap().demultiply();
        assert_eq!((p1.red(), p1.green(), p1.blue()), (255, 0, 0));
        assert!(p1.alpha() < 200, "半透明 alpha 应保留，实得 {}", p1.alpha());
    }

    /// ★ RuPix 补丁 011 —— `pixelated` 默认关闭，且随 `clone`/`tinted` 正确传播。
    ///
    /// 默认关闭是**整个 UI 的护栏**：这条规则若默认开，非整数 DPI 下的矢量图标
    /// （18dp 图标在 125% 下物理 22.5px，已算放大）会全线变成锯齿。
    /// `tinted` 只换颜色不改声明，所以标志必须跟着走。
    #[test]
    fn pixelated_defaults_off_and_propagates() {
        let a = Image::from_rgba(2, 2, &[255u8, 0, 0, 255].repeat(4)).unwrap();
        assert!(!a.pixelated(), "默认必须是双线性");
        let b = a.clone().with_pixelated(true);
        assert!(b.pixelated() && !a.pixelated(), "标志须随值拷贝走");
        assert!(b.tinted(Color::rgb(0, 255, 0)).pixelated());
    }

    #[test]
    fn visual_state_opacity() {
        assert_eq!(VisualState::Disabled.opacity(), 0.38);
        assert_eq!(VisualState::Normal.opacity(), 1.0);
        assert_eq!(VisualState::Hover.opacity(), 1.0);
    }

    #[test]
    fn png_decoder_probe() {
        assert!(PngDecoder.probe(PNG_MAGIC));
        assert!(!PngDecoder.probe(&[0, 1, 2]));
        assert!(!PngDecoder.probe(&[0xff, 0xd8])); // JPEG 头不应命中
    }

    #[cfg(feature = "svg")]
    mod svg {
        use super::*;

        /// 一张 10×6 的红色矩形 SVG。
        const RED_SVG: &[u8] =
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="6"><rect width="10" height="6" fill="#ff0000"/></svg>"##;

        #[test]
        fn svg_decoder_probe() {
            assert!(SvgDecoder.probe(RED_SVG));
            // 带 XML 声明前缀仍命中。
            assert!(SvgDecoder.probe(br#"<?xml version="1.0"?><svg></svg>"#));
            // 大小写不敏感。
            assert!(SvgDecoder.probe(b"<SVG></SVG>"));
            // svgz（gzip 魔数）命中。
            assert!(SvgDecoder.probe(&[0x1f, 0x8b, 0x08, 0x00]));
            // PNG / 垃圾不命中。
            assert!(!SvgDecoder.probe(PNG_MAGIC));
            assert!(!SvgDecoder.probe(&[0, 1, 2, 3]));
        }

        #[test]
        fn from_svg_bytes_uses_intrinsic_size() {
            let img = Image::from_svg_bytes(RED_SVG, None).unwrap();
            assert_eq!(img.size(), Size::new(10, 6));
        }

        #[test]
        fn from_svg_bytes_target_width_scales_keeping_aspect() {
            // 指定宽度 20 → 高度按 10:6 等比 → 12。
            let img = Image::from_svg_bytes(RED_SVG, Some(20)).unwrap();
            assert_eq!(img.size(), Size::new(20, 12));
        }

        #[test]
        fn from_bytes_sniffs_svg_via_registry() {
            let img = Image::from_bytes(RED_SVG).unwrap();
            assert_eq!(img.size(), Size::new(10, 6));
        }

        #[test]
        fn rasterized_svg_has_red_pixel() {
            // 光栅化后中心像素应为红色（验证确实渲染了内容，而非空白）。
            let img = Image::from_svg_bytes(RED_SVG, Some(20)).unwrap();
            let p = img.pixmap().pixel(10, 6).unwrap().demultiply();
            assert!(
                p.red() > 200 && p.green() < 60 && p.blue() < 60,
                "中心应为红色，实得 ({},{},{})",
                p.red(),
                p.green(),
                p.blue()
            );
        }

        #[test]
        fn invalid_svg_is_decode_error() {
            assert!(matches!(
                Image::from_svg_bytes(b"<svg not valid", None),
                Err(ImageError::Decode(_))
            ));
        }

        /// ★ 补丁 017：判据本身——纯图形为假、任何 text 形态为真、gzip 一律当真。
        #[cfg(feature = "svg-text")]
        #[test]
        fn svg_may_have_text_predicate_forms() {
            assert!(!svg_may_have_text(
                br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#f00"/></svg>"##
            ));
            assert!(svg_may_have_text(
                br##"<svg xmlns="http://www.w3.org/2000/svg"><text x="4" y="30">Hi</text></svg>"##
            ));
            assert!(svg_may_have_text(
                br##"<svg xmlns="http://www.w3.org/2000/svg"><sv:text>Hi</sv:text></svg>"##
            ));
            // 带命名空间的写法里 `<svg:text` 同时命中 `<text`？不会——它命中的是 `:text`。
            // 反向钉一次：确认 `<svg:text` 里不含 `<text` 子串，判据靠的是第二条。
            assert!(!&br##"<svg:text>"##
                .windows(b"<text".len())
                .any(|w| w.eq_ignore_ascii_case(b"<text")));
            // gzip 包装：内容看不见，必须当真。
            assert!(svg_may_have_text(&[0x1f, 0x8b, 0x08, 0x00, 0x00]));
        }

        /// ★ 补丁 017 承重：判据为假的图（RED_SVG 无 text）仍要渲出与旧路径同一
        /// 个中心像素——这条与 [`rasterized_svg_has_red_pixel`] 判的是同一枚像素，
        /// 只是走的分支不同。判据写反（把纯图形也塞进字体库、或反过来把带字的排除）
        /// 时，这条与 [`renders_svg_text_glyphs`] 里必有一条先红。
        #[cfg(feature = "svg-text")]
        #[test]
        fn text_free_svg_renders_without_fontdb() {
            let img = Image::from_svg_bytes(RED_SVG, Some(20)).unwrap();
            let p = img.pixmap().pixel(10, 6).unwrap().demultiply();
            assert!(
                p.red() > 200 && p.green() < 60 && p.blue() < 60,
                "纯图形 SVG 走免字体库分支后中心仍应为红，实得 ({},{},{})",
                p.red(),
                p.green(),
                p.blue()
            );
        }

        /// 文字 SVG 应渲染出字形（依赖系统字体；svg-text feature 才编译）。
        #[cfg(feature = "svg-text")]
        #[test]
        fn renders_svg_text_glyphs() {
            const TEXT_SVG: &[u8] =
                br##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"><text x="4" y="30" font-size="30" font-family="Arial" fill="#000000">Hi</text></svg>"##;
            let img = Image::from_svg_bytes(TEXT_SVG, None).unwrap();
            let opaque = img
                .pixmap()
                .pixels()
                .iter()
                .filter(|p| p.alpha() > 0)
                .count();
            assert!(opaque > 0, "文字 SVG 应光栅出字形像素，实得 {opaque}");
        }

        /// ★ 补丁 022：剥字函数的形态判据——该剥的剥干净、不该剥的一个字节不动、
        /// 不敢剥的返回 None 让调用方走完整解析。
        #[test]
        fn strip_text_node_forms() {
            // 自闭合与配对都删的是完整元素。
            assert_eq!(
                svg_strip_text_nodes(b"<svg><text x='1'/>rest</svg>").as_deref(),
                Some(&b"<svg>rest</svg>"[..])
            );
            assert_eq!(
                svg_strip_text_nodes(b"<svg><text>Hi</text>rest</svg>").as_deref(),
                Some(&b"<svg>rest</svg>"[..])
            );
            // 大小写不敏感（开标签与收尾各钉一次）。
            assert_eq!(
                svg_strip_text_nodes(b"<TEXT>a</TEXT>b").as_deref(),
                Some(&b"b"[..])
            );
            // `<text` 后跟非空白非 `>` 不是 text 元素（textPath / textArea）。
            assert_eq!(svg_strip_text_nodes(b"<svg><textPath/></svg>"), None);
            // 命名空间前缀不认（usvg 也不渲染它）。
            assert_eq!(svg_strip_text_nodes(b"<svg><sv:text>a</sv:text></svg>"), None);
            // 没有文字 → None。
            assert_eq!(svg_strip_text_nodes(b"<svg><rect/></svg>"), None);
            // 收尾丢了 → None（保守）。
            assert_eq!(svg_strip_text_nodes(b"<svg><text>x</svg>"), None);
            // gzip 包装 → None。
            assert_eq!(svg_strip_text_nodes(&[0x1f, 0x8b, 0x08, 0x00]), None);
        }

        /// ★ 补丁 022 承重：无文字文档走 no_text 入口必须与全趟**逐字节相同**——
        /// 这是"调用方无需预判有没有文字"的契约；带文字文档的两趟必须不同
        /// （第一趟真的少了字），否则渐进首绘画的就是假渐进。
        #[cfg(feature = "svg-text")]
        #[test]
        fn no_text_pass_matches_full_when_text_free_and_differs_with_text() {
            // 无文字：两趟逐字节相同。
            let a = Image::from_svg_bytes(RED_SVG, Some(20)).unwrap();
            let b = Image::from_svg_bytes_no_text(RED_SVG, Some(20)).unwrap();
            assert_eq!(a.pixmap().data(), b.pixmap().data(), "无文字两趟必须同像素");

            // 带文字：全趟渲出字形、剥字趟一个字都没有 ⇒ 像素必然分离。
            const TEXT_SVG: &[u8] =
                br##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"><rect width="80" height="40" fill="#ff0000"/><text x="4" y="30" font-size="30" font-family="Arial" fill="#000000">Hi</text></svg>"##;
            let full = Image::from_svg_bytes(TEXT_SVG, None).unwrap();
            let fast = Image::from_svg_bytes_no_text(TEXT_SVG, None).unwrap();
            assert_ne!(
                full.pixmap().data(),
                fast.pixmap().data(),
                "带文字的两趟像素必须不同，否则剥字没生效"
            );
            assert!(
                fast.pixmap().pixels().iter().all(|p| p.red() > 200),
                "剥字趟应只剩红色底，不得残留字形"
            );
        }

        /// ★ 补丁 023 承重：索引落盘→回放必须重建出与现扫**同清单**的字体库
        /// （面数 / 家族名集合 / PostScript 名逐一相等），回放库还要真能被
        /// query 命中（face ID 重发后消费方无感）；指纹不符（文件被改写）与
        /// 损坏字节必须整份作废——宁可重扫，不许半信半疑。
        #[cfg(feature = "svg-text")]
        #[test]
        fn font_index_cache_roundtrip_matches_scan_and_rejects_mismatch() {
            use std::collections::BTreeSet;

            let mut scanned = resvg::usvg::fontdb::Database::new();
            scanned.load_system_fonts();

            let dir = std::env::temp_dir()
                .join(format!("windui-fontcache-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("fonts.bin");

            svg_font_cache_store(&scanned, &path);
            assert!(path.exists(), "缓存必须真实落盘");

            let mut replayed = resvg::usvg::fontdb::Database::new();
            assert!(svg_font_cache_load(&mut replayed, &path).is_some(), "指纹吻合必须回放成功");

            let names = |db: &resvg::usvg::fontdb::Database| -> BTreeSet<String> {
                db.faces()
                    .flat_map(|f| f.families.iter().map(|(n, _)| n.clone()))
                    .collect()
            };
            let ps = |db: &resvg::usvg::fontdb::Database| -> BTreeSet<String> {
                db.faces().map(|f| f.post_script_name.clone()).collect()
            };
            assert_eq!(scanned.faces().count(), replayed.faces().count(), "面数必须一致");
            assert_eq!(names(&scanned), names(&replayed), "家族名集合必须一致");
            assert_eq!(ps(&scanned), ps(&replayed), "PostScript 名集合必须一致");

            // 回放库可被消费：按现扫库里任一家族 query，必须命中。
            let family = scanned.faces().next().unwrap().families[0].0.clone();
            let q = resvg::usvg::fontdb::Query {
                families: &[resvg::usvg::fontdb::Family::Name(family.as_str())],
                weight: resvg::usvg::fontdb::Weight::NORMAL,
                style: resvg::usvg::fontdb::Style::Normal,
                stretch: resvg::usvg::fontdb::Stretch::Normal,
            };
            assert!(replayed.query(&q).is_some(), "回放库必须能 query 命中");

            // 指纹判据的真红：拷一枚系统字体建库落盘，回放成功；改写副本（mtime
            // 必变）后再回放——必须拒绝，一个面都不许进库。
            let src = scanned
                .faces()
                .find_map(|f| match &f.source {
                    resvg::usvg::fontdb::Source::File(p) => Some(p.clone()),
                    _ => None,
                })
                .unwrap();
            let copy = dir.join("probe.ttf");
            std::fs::copy(&src, &copy).unwrap();
            let mut probe = resvg::usvg::fontdb::Database::new();
            probe.load_font_file(&copy).unwrap();
            let probe_path = dir.join("probe.bin");
            svg_font_cache_store(&probe, &probe_path);
            let mut ok = resvg::usvg::fontdb::Database::new();
            assert!(svg_font_cache_load(&mut ok, &probe_path).is_some());
            std::fs::write(&copy, b"tampered").unwrap();
            let mut stale = resvg::usvg::fontdb::Database::new();
            assert!(svg_font_cache_load(&mut stale, &probe_path).is_none(), "指纹不符必须拒绝");
            assert_eq!(stale.faces().count(), 0, "拒绝时不得放进任何面");

            // 损坏字节：解析失败 = 拒绝，不是半信半疑。
            let broken = dir.join("broken.bin");
            std::fs::write(&broken, b"RUFXDB1-broken").unwrap();
            let mut bad = resvg::usvg::fontdb::Database::new();
            assert!(svg_font_cache_load(&mut bad, &broken).is_none());

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
