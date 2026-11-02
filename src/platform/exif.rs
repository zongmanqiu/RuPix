//! 平台层手写最小 EXIF 解析器：属性窗「拍摄信息」段的唯一数据源。
//!
//! # 为什么手写
//!
//! - 镜像源（rsproxy-sparse）无 `kamadak-exif` 可用；
//! - 只需 9 个标签，`exif` 全家桶是 over-engineering；
//! - 项目定位「简单看图」+ 体积优先（`opt-level = "z"`）+ 同 qoi 解码器手写先例。
//!
//! # 范围
//!
//! - **支持**：JPEG APP1 段内嵌 TIFF、PNG `eXIf` chunk、WebP `EXIF` chunk、TIFF/II/MM 直开。
//! - **暂静默**：HEIF / HEIC / AVIF 的 `Exif` ISO-BMFF box（结构较重，留 hook 见 `locate_tiff`）。
//! - **提取字段**：Make · Model · LensModel · DateTimeOriginal · FNumber ·
//!   ExposureTime · PhotographicSensitivity (ISO) · FocalLength · GPS lat/lon/alt+ref。
//!
//! # 架构
//!
//! **入口分派 / TIFF 解析 / 字段格式化 / UI** 四层各自独立：
//! - 入口 `locate_tiff` 只做"找到 TIFF 起点字节"，找到就交给 `parse_tiff`；
//! - `parse_tiff` 只认 TIFF（II/MM + IFD 递归），完全不看容器；
//! - 未来加新格式 = 在 `locate_tiff` 加一条分支，其他三层零改动。
//!
//! # 契约
//!
//! - 字段缺失即空串（不编假数据，同解码器详情窗原则）。
//! - 文件读失败 / 无 EXIF 段 / 解析错 → 整个 [`ExifInfo`] 全空，不 panic。
//! - **工作量对文件长度线性**：条目遍历越界即返回，子 IFD 靠 visited 去重防菱形
//!   重入爆炸。本函数在 **UI 线程**同步调用，不许存在"输入可达的超长循环"。
//! - 属性窗按 signal 消费，`.visible_when(!v.is_empty())` 让无 EXIF 文件整段不显示。

use std::collections::HashSet;
use std::path::Path;

/// 「属性」窗「拍摄信息」段的展示数据，全部预格式化到 String；空 = 无字段。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExifInfo {
    /// `DateTimeOriginal`，格式 `YYYY-MM-DD HH:MM:SS`。
    pub capture_time: String,
    /// Make + Model 合并，如 `Canon EOS R5`。
    pub camera: String,
    /// `LensModel`（可空）。
    pub lens: String,
    /// 光圈 / 快门 / ISO 合并，如 `f/2.8 · 1/250s · ISO 400`。
    pub exposure: String,
    /// `FocalLength`，如 `50mm`。
    pub focal: String,
    /// GPS 度分秒 + 方向，如 `31.2304°N, 121.4737°E`。
    pub gps: String,
    /// `Orientation`（EXIF 274），1..=8；**0 = 文件里没有这个标签**。
    ///
    /// 属性窗不显示它，但解码器解码路要它：解码器自述的 `orientation` 与这里读的是同一个
    /// 274，语义必须一致（0/1 都交宿主处理，见规范），否则竖拍照片会横躺——那是
    /// m48 那族缺陷。（2026-10-25 前这里还挂着"内置 WIC 路要它"的理由，WIC 已删除。）
    pub orientation: u16,
}

/// 从文件路径读 EXIF；任何错误 / 无 EXIF → 全空 [`ExifInfo`]。
///
/// **只读文件头**（K41 那条），上限**按容器分档**：默认 [`EXIF_HEAD_CAP`]，只有 WebP
/// 允许读到 [`EXIF_WEBP_CAP`]。本函数在 **UI 线程**同步调用（属性窗每换一张图走一次，
/// 解码器解码路再走一次只为取 274），所以"读多少"就是"冻多久"。
///
/// 为什么默认档从 12 MiB 收到 256 KiB：那个数从来不是"EXIF 会在那儿"，而是"别把 UI
/// 线程冻死"的兜底，而兜底本身就是代价——m104 探针（跑在测试二进制里，每档 5 遍取最小、
/// 热缓存，8 枚真文件）逐档量过：**4 KiB 15–18 µs / 256 KiB 23–25 µs / 4 MiB 1.4–1.7 ms /
/// 12 MiB 4.2–4.7 ms** ⇒ 属性窗开着时每翻一页，UI 线程白读 12 MiB、白分配 12 MiB。
/// 这条只读"白付多少字节"的量级，不拿来宣称端点变快（端点判据见 [`read_exif`] 下方用例与
/// 本仓历史记录：朝向仍要到位）。
/// 而"EXIF 到底在第几字节"这条不是猜的：本机 94 枚带 EXIF 标记的真文件（`.temp/m104_exif_scan.py`，
/// 只搜前 4 MiB）里，最深的一枚**非 HEIC** 落在 **19,087 B**（PNG 的 `eXIf`），JPEG APP1 全在
/// 头几十 KB；只有两枚 HEIC 在 2,500,433 B——可那容器今天根本不解，读多深都是空。
/// ★ 已知缺口（两条一起看，都不假装没有）：
/// - WebP 的 `EXIF` chunk 常落在文件**尾部** ⇒ 它单独保留深档，这条不收紧。
/// - **本轮新收的一条**：JPEG / PNG / TIFF 里 EXIF 落在 256 KiB 之后的，拍摄信息照实
///   显示为**空**（12 MiB 时代读得到）。这类文件的真实存在率没测过（上面那 94 枚里一枚没有，
///   但那只是本机语料）⇒ 配三条用例把边界钉住：头内的必须解析出来，头外的必须空而不是猜，
///   WebP 尾部的必须还读得到。
pub fn read_exif(path: &Path) -> ExifInfo {
    let bytes = match read_head(path, EXIF_HEAD_CAP) {
        Ok(b) => b,
        Err(_) => return ExifInfo::default(),
    };
    if let Some(tiff) = locate_tiff(&bytes) {
        return parse_tiff(&bytes[tiff..]);
    }
    // 只有 WebP 值得再往深处读一次。头没读满说明整份文件已经在手里了，再读一遍是白付
    // 一次 open——那种情况下第一次定位失败就已经是最终答案。
    if bytes.len() == EXIF_HEAD_CAP && is_webp(&bytes) {
        if let Ok(deep) = read_head(path, EXIF_WEBP_CAP) {
            if let Some(tiff) = locate_tiff(&deep) {
                return parse_tiff(&deep[tiff..]);
            }
        }
    }
    ExifInfo::default()
}

/// 容器是不是 WebP。嗅探只要头 12 字节，长度不够一律判否。
fn is_webp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP"
}

/// 常规容器的文件头上限（JPEG APP1 / PNG `eXIf` / TIFF IFD 都在这一段里）。
const EXIF_HEAD_CAP: usize = 256 * 1024;

/// WebP 专用的深档：它的 `EXIF` chunk 常在尾部，256 KiB 头装不下。12 MiB 是"别冻住
/// UI 线程"的兜底，不是实际需要量（读数见 [`read_exif`]）。
const EXIF_WEBP_CAP: usize = 12 * 1024 * 1024;

/// 读文件前 `cap` 字节。`File` + `take` 而不是 `fs::read` 后再截断——后者会把整个
/// 大文件先吃进内存，那正是要避免的事。
fn read_head(path: &Path, cap: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len() as usize;
    let take = len.min(cap);
    let mut buf = Vec::with_capacity(take);
    f.by_ref().take(take as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// 找到 TIFF 头起点：按容器格式分派，各入口只负责**定位**、不负责解析。
/// 找到之后一律交给 [`parse_tiff`]——这是"入口可扩展、解析层稳定"的关键。
///
/// **已支持**：JPEG APP1 / PNG `eXIf` chunk / WebP `EXIF` chunk / 裸 TIFF（II / MM）。
/// **暂静默**：HEIF / HEIC / AVIF——它们是 ISO-BMFF box 树（`meta` → `iInf` + `Iloc`
/// 引用 `Exif` box），结构比 chunk 扫描重一些。未来加一个 `locate_iso_bmff_exif`
/// helper 挂在这里即可，`parse_tiff` / `ExifInfo` / UI 一行不改。
fn locate_tiff(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 8 {
        return None;
    }
    if bytes[0] == 0xFF && bytes[1] == 0xD8 {
        return locate_jpeg_app1(bytes);
    }
    // PNG 签名 8B → chunk 流：4B BE len + 4B type + payload + 4B CRC
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return locate_png_exif(bytes);
    }
    // WebP：`RIFF` + LE size + `WEBP` → 子 chunk：4B fourcc + LE size + payload(+pad)
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return locate_webp_exif(bytes);
    }
    // 裸 TIFF
    if &bytes[0..2] == b"II" || &bytes[0..2] == b"MM" {
        return Some(0);
    }
    None
}

/// JPEG：SOI 后逐个 marker 扫，找到 APP1 里带 `Exif\0\0` 前缀的那段。
fn locate_jpeg_app1(bytes: &[u8]) -> Option<usize> {
    let mut p = 2usize;
    while p + 4 <= bytes.len() {
        if bytes[p] != 0xFF {
            return None;
        }
        let marker = bytes[p + 1];
        // 独立 marker（无长度字段）：SOI/EOI/SO/TDPL/RSTn
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            p += 2;
            continue;
        }
        let len = ((bytes[p + 2] as u16) << 8) | bytes[p + 3] as u16;
        let seg = (len as usize).saturating_sub(2);
        let body = p + 4;
        if body + seg > bytes.len() {
            return None;
        }
        if marker == 0xE1 && seg >= 14 && &bytes[body..body + 6] == b"Exif\0\0" {
            return Some(body + 6);
        }
        if marker == 0xDA {
            // SOS：图像数据段开始，EXIF 不会再出现
            return None;
        }
        p = body + seg;
    }
    None
}

/// PNG：8B 签名后按 chunk 流走，找 `eXIf`（PNG 扩展规范 §4.2.7.3）。
/// 遇 `IDAT` 早退——EXIF 必须在 IDAT 之前，避免扫全文件；不校验 CRC（看图器只关心数据）。
fn locate_png_exif(bytes: &[u8]) -> Option<usize> {
    let mut p = 8usize;
    while p + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]]) as usize;
        let typ = &bytes[p + 4..p + 8];
        let data = p + 8;
        // 恶意 / 截断文件：payload 声称超出剩余字节直接放弃
        if bytes.len() - data < len {
            return None;
        }
        if typ == b"eXIf" && len >= 8 {
            return Some(data);
        }
        if typ == b"IDAT" || typ == b"IEND" {
            return None;
        }
        // 跳到下一 chunk：payload + 4B CRC
        p = data + len + 4;
    }
    None
}

/// WebP：`RIFF`(4B) + LE size(4B) + `WEBP`(4B) 头后，子 chunk 是
/// `fourcc`(4B) + LE size(4B) + payload；size 为奇数时后面补一字节 0（RIFF 规范）。
fn locate_webp_exif(bytes: &[u8]) -> Option<usize> {
    let mut p = 12usize;
    while p + 8 <= bytes.len() {
        let fourcc = &bytes[p..p + 4];
        let size =
            u32::from_le_bytes([bytes[p + 4], bytes[p + 5], bytes[p + 6], bytes[p + 7]]) as usize;
        let data = p + 8;
        if bytes.len() - data < size {
            return None;
        }
        if fourcc == b"EXIF" && size >= 8 {
            return Some(data);
        }
        // 奇数 size 补一字节
        p = data + size + (size & 1);
    }
    None
}

/// TIFF 字节序 + 基址。
struct Endian {
    le: bool,
}

impl Endian {
    fn u16(&self, b: &[u8], at: usize) -> Option<u16> {
        let x = at.checked_add(1)?;
        if x >= b.len() {
            return None;
        }
        Some(if self.le {
            u16::from_le_bytes([b[at], b[x]])
        } else {
            u16::from_be_bytes([b[at], b[x]])
        })
    }
    fn u32(&self, b: &[u8], at: usize) -> Option<u32> {
        let e = at.checked_add(3)?;
        if e >= b.len() {
            return None;
        }
        Some(if self.le {
            u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
        } else {
            u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
        })
    }
}

/// 一次 IFD 里我们关心的字段，用 tag 索引收集。
#[derive(Default)]
struct Collected {
    make: String,
    model: String,
    datetime_orig: String,
    lens_model: String,
    fnumber: Option<f64>,
    exposure_time: Option<f64>,
    iso: Option<u32>,
    focal_length: Option<f64>,
    /// EXIF 274 `Orientation`；0 = 没有这个标签。
    orientation: u16,
    // GPS
    gps_lat: Option<(f64, f64, f64)>,
    gps_lat_ref: String,
    gps_lon: Option<(f64, f64, f64)>,
    gps_lon_ref: String,
    gps_alt: Option<f64>,
    gps_alt_below: bool,
}

fn parse_tiff(tiff: &[u8]) -> ExifInfo {
    if tiff.len() < 8 {
        return ExifInfo::default();
    }
    let endian = Endian {
        le: &tiff[0..2] == b"II",
    };
    if endian.u16(tiff, 2) != Some(42) {
        return ExifInfo::default();
    }
    let mut c = Collected::default();
    let Some(ifd0) = endian.u32(tiff, 4) else {
        return ExifInfo::default();
    };
    let mut visited = HashSet::new();
    // 工作量预算：见 [`EXIF_MAX_ENTRIES`]。合法文件的条目总数远够不着，只有畸形
    // 结构会被它截断（截断=停止采集，已采到的字段照常显示）。
    let mut budget = exif_budget(tiff.len());
    walk(
        tiff,
        &endian,
        ifd0 as usize,
        &mut c,
        0,
        false,
        &mut visited,
        &mut budget,
    );
    build_info(&c)
}

/// 递归深度上限（只防"链式成环"；菱形重入靠 [`walk`] 的 visited 去重）。
const MAX_DEPTH: u32 = 3;

/// **条目处理总数上界**（规范§六 第 10 条的"轮数上界取自文件长度、不取自文件内的
/// 计数"落到实现）。2026-10-17 审计补：`visited` 去重消掉了 `N^层数` 爆炸，但每条
/// IFD 的轮数当时仍取自文件里的 `count`（u16，可到 65535），于是总工作量
/// ≈ 访问数 × min(count, 剩余字节/12)——对文件长度**不是线性**，一个 ~1.6MB 的构造
/// 文件即可把 UI 线程冻住分钟级。
///
/// 取 `len/2` 与 65536 的较小值：一条 entry 实占 12 字节，合法文件总条目数必然
/// ≤ `len/12`，所以 `len/2` 永远咬不到正常文件；65536 是给"整读到大文件尾部"那类
/// 段落留的绝对天花板（真实相机 EXIF 的条目数在几十量级，留了 3 个数量级余量）。
const EXIF_MAX_ENTRIES: usize = 65_536;

fn exif_budget(tiff_len: usize) -> usize {
    EXIF_MAX_ENTRIES.min(tiff_len / 2)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    tiff: &[u8],
    e: &Endian,
    ifd_off: usize,
    c: &mut Collected,
    depth: u32,
    in_gps: bool,
    visited: &mut HashSet<(usize, bool)>,
    budget: &mut usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    // 同一 (IFD, 上下文) 只走一次。深度上限防不了**菱形重入**：一个 IFD 的 N 个
    // 条目可以全指向同一个子 IFD，递归次数退化成 N^层数——3 层、N=65535 就是
    // 2.8e14 次，约 3MB 构造文件即可把属性窗（在 UI 线程上调本函数）永久假死。
    // key 带 `in_gps` 是因为同一偏移可能被 0x8769 与 0x8825 各引一次，两类上下文
    // 采集的字段不同，不能互相顶掉。
    if !visited.insert((ifd_off, in_gps)) {
        return;
    }
    let Some(count) = e.u16(tiff, ifd_off) else {
        return;
    };
    let mut p = ifd_off + 2;
    for _ in 0..count {
        // 每处理一条扣一格预算，扣光即停——**轮数上界来自预算（段落长度驱动），
        // 不来自 `count`**（规范§六 第 10 条；见 [`EXIF_MAX_ENTRIES`] 的推导）。
        match budget.checked_sub(1) {
            Some(rest) => *budget = rest,
            None => return,
        }
        let Some(tag) = e.u16(tiff, p) else {
            return;
        };
        let Some(ty) = e.u16(tiff, p + 2) else {
            return;
        };
        let Some(n) = e.u32(tiff, p + 4) else {
            return;
        };
        // count = 0 的条目没有值可读；不跳过的话 size = 0 会被判成 inline，
        // RATIONAL/SHORT 读取器就会把本条 value 字段和**下一条目/next-IFD** 的字节
        // 拼成一个值（与未知类型那条同族）。
        if n == 0 {
            p += 12;
            continue;
        }
        // value/offset 字段（4 字节）：size ≤ 4 直接内联；否则按 offset 读。
        //
        // 类型不认识 = 这一条不作数，跳过。以前写的是 `type_size(ty).unwrap_or(0)`：
        // size 算成 0 → `inline` 为真 → 从本条自己的 4 字节 value 字段按 `n` 个字节读出去。
        // `read_ascii` 只保证不越**文件**界，所以它会一路读进邻近条目的字节，属性窗于是
        // 拿出一句从别处拼出来的"相机型号"（审计第三批：畸形结构要的不是一份猜测）。
        let Some(size) = type_size(ty).and_then(|s| s.checked_mul(n as usize)) else {
            p += 12;
            continue;
        };
        let inline = size <= 4;
        let vbase = if inline {
            p + 8
        } else {
            match e.u32(tiff, p + 8) {
                Some(o) => o as usize,
                None => {
                    p += 12;
                    continue;
                }
            }
        };
        // GPS 子 IFD 用低位 tag 号（0x0001..0x0006），与 TIFF/EXIF 主 IFD 的
        // tag 号可能重号——靠 `in_gps` 上下文标志分派，不看 depth。
        if in_gps {
            match tag {
                0x0001 => c.gps_lat_ref = read_ascii(tiff, e, vbase, n as usize, inline),
                0x0002 => c.gps_lat = read_dms(tiff, e, vbase, inline, n as usize),
                0x0003 => c.gps_lon_ref = read_ascii(tiff, e, vbase, n as usize, inline),
                0x0004 => c.gps_lon = read_dms(tiff, e, vbase, inline, n as usize),
                0x0005 => {
                    // AltitudeRef：BYTE 单值，TIFF 左对齐 → 首字节即值。
                    // 0=海平面以上，1=以下。不看字节序（BYTE 是单字节）。
                    c.gps_alt_below = tiff.get(p + 8).copied().unwrap_or(0) == 1;
                }
                0x0006 => c.gps_alt = read_rational(tiff, e, vbase, inline),
                _ => {}
            }
            p += 12;
            continue;
        }
        match tag {
            0x010F => c.make = read_ascii(tiff, e, vbase, n as usize, inline),
            0x0110 => c.model = read_ascii(tiff, e, vbase, n as usize, inline),
            // Orientation（274）：SHORT 单值，存在 value/offset 字段的**低半部**（m48 那条
            // 同族坑）。0 保持"没有标签"，与解码器侧照实回报的口径一致。
            0x0112 => {
                c.orientation = read_short_or_long(tiff, e, vbase, ty, inline).unwrap_or(0) as u16;
            }
            0x0132 => { /* DateTime (改过时间)，暂不采集 */ }
            0x829A => c.exposure_time = read_rational(tiff, e, vbase, inline),
            0x829D => c.fnumber = read_rational(tiff, e, vbase, inline),
            0x8769 => {
                // ExifIfd 子 IFD 指针（LONG 单值，存于 value/offset 位置）
                if let Some(sub) = e.u32(tiff, p + 8) {
                    if sub > 0 {
                        walk(tiff, e, sub as usize, c, depth + 1, false, visited, budget);
                    }
                }
            }
            0x8825 => {
                // GPSInfo 子 IFD 指针
                if let Some(sub) = e.u32(tiff, p + 8) {
                    if sub > 0 {
                        walk(tiff, e, sub as usize, c, depth + 1, true, visited, budget);
                    }
                }
            }
            0x8827 => c.iso = read_short_or_long(tiff, e, vbase, ty, inline),
            0x9003 => c.datetime_orig = read_ascii(tiff, e, vbase, n as usize, inline),
            0x920A => c.focal_length = read_rational(tiff, e, vbase, inline),
            0xA434 => c.lens_model = read_ascii(tiff, e, vbase, n as usize, inline),
            _ => {}
        }
        p += 12;
    }
}

/// EXIF 类型 → 单元素字节数。
fn type_size(ty: u16) -> Option<usize> {
    Some(match ty {
        1 => 1,  // BYTE
        2 => 1,  // ASCII
        3 => 2,  // SHORT
        4 => 4,  // LONG
        5 => 8,  // RATIONAL
        6 => 1,  // SBYTE
        7 => 1,  // UNDEFINED
        8 => 2,  // SSHORT
        9 => 4,  // SLONG
        10 => 8, // SRATIONAL
        11 => 4, // FLOAT
        12 => 8, // DOUBLE
        _ => return None,
    })
}

fn read_ascii(tiff: &[u8], e: &Endian, base: usize, n: usize, inline: bool) -> String {
    let _ = e;
    let _ = inline;
    let end = match base.checked_add(n) {
        Some(x) => x,
        None => return String::new(),
    };
    if end > tiff.len() {
        return String::new();
    }
    let s = String::from_utf8_lossy(&tiff[base..end]);
    s.trim_end_matches('\0').trim().to_string()
}

fn read_rational(tiff: &[u8], e: &Endian, base: usize, inline: bool) -> Option<f64> {
    // 若 inline（size ≤ 4）：不是 RATIONAL（RATIONAL size=8）；
    // RATIONAL 一定 offset 存储。因此忽略 inline 分支。
    let _ = inline;
    let num = e.u32(tiff, base)?;
    let den = e.u32(tiff, base + 4)?;
    if den == 0 {
        None
    } else {
        Some(num as f64 / den as f64)
    }
}

fn read_dms(
    tiff: &[u8],
    e: &Endian,
    base: usize,
    _inline: bool,
    n: usize,
) -> Option<(f64, f64, f64)> {
    if n < 3 {
        return None;
    }
    let d = read_rational(tiff, e, base, false)?;
    let m = read_rational(tiff, e, base + 8, false)?;
    let s = read_rational(tiff, e, base + 16, false)?;
    Some((d, m, s))
}

/// SHORT / LONG 单值内联取值。TIFF 6.0 §"Value"：值左对齐，
/// SHORT 占 value/offset 字段首 2 字节，LONG 占整 4 字节——两种字节序都从 base 读。
fn read_short_or_long(tiff: &[u8], e: &Endian, base: usize, ty: u16, inline: bool) -> Option<u32> {
    let _ = inline;
    match ty {
        3 => e.u16(tiff, base).map(|v| v as u32),
        4 => e.u32(tiff, base),
        _ => None,
    }
}

fn build_info(c: &Collected) -> ExifInfo {
    let camera = join_nonblank(&[&c.make, &c.model]);
    let exposure = format_exposure(c);
    let focal = c
        .focal_length
        .map(|f| format!("{:.0}mm", f))
        .unwrap_or_default();
    let gps = format_gps(c);
    ExifInfo {
        capture_time: c.datetime_orig.clone(),
        camera,
        lens: c.lens_model.clone(),
        exposure,
        focal,
        gps,
        orientation: c.orientation,
    }
}

fn join_nonblank(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_exposure(c: &Collected) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(3);
    if let Some(ap) = c.fnumber {
        parts.push(format!("f/{:.1}", ap));
    }
    if let Some(sh) = c.exposure_time {
        parts.push(if sh <= 0.0 {
            // 0/1 是合法 RATIONAL；1.0 / 0.0 会饱和成 i64::MAX，显示成
            // 「1/9223372036854775807s」。零曝光就说零。
            "0s".to_string()
        } else if sh >= 1.0 {
            format!("{:.1}s", sh)
        } else {
            format!("1/{}s", (1.0 / sh).round() as i64)
        });
    }
    if let Some(iso) = c.iso {
        parts.push(format!("ISO {}", iso));
    }
    parts.join(" · ")
}

fn format_gps(c: &Collected) -> String {
    let (d1, m1, s1) = match c.gps_lat {
        Some(v) => v,
        None => return String::new(),
    };
    let (d2, m2, s2) = match c.gps_lon {
        Some(v) => v,
        None => return String::new(),
    };
    let lat = d1 + m1 / 60.0 + s1 / 3600.0;
    let lon = d2 + m2 / 60.0 + s2 / 3600.0;
    let lat_ref = if c.gps_lat_ref.is_empty() {
        ""
    } else {
        &c.gps_lat_ref
    };
    let lon_ref = if c.gps_lon_ref.is_empty() {
        ""
    } else {
        &c.gps_lon_ref
    };
    let mut out = format!("{:.4}°{}, {:.4}°{}", lat, lat_ref, lon, lon_ref);
    if let Some(alt) = c.gps_alt {
        out.push_str(&format!(
            ", {:.0}m{}",
            alt,
            if c.gps_alt_below { " below" } else { "" }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_rejects_short_and_non_image() {
        assert!(locate_tiff(&[]).is_none());
        assert!(locate_tiff(&[0x00; 4]).is_none());
    }

    #[test]
    fn locate_accepts_bare_tiff_little_endian() {
        let mut bytes = Vec::from(b"II".as_slice());
        bytes.extend_from_slice(&42u16.to_le_bytes());
        bytes.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset = 8
        assert_eq!(locate_tiff(&bytes), Some(0));
    }

    #[test]
    fn read_exif_missing_file_is_empty() {
        let info = read_exif(Path::new("Z:\\definitely\\does\\not\\exist.jpg"));
        assert!(info.camera.is_empty());
        assert!(info.capture_time.is_empty());
        assert!(info.gps.is_empty());
    }

    #[test]
    fn ascii_and_u16_and_rational_parsing() {
        // 手工构造最小 TIFF：II 42 IFD0=8 → 1 条 Make 记录（"CANON\0"）+ 空 next_ifd
        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        // 8 起：IFD0
        // count = 1
        t.extend_from_slice(&1u16.to_le_bytes());
        // entry: tag=0x010F, type=2 (ASCII), n=6, offset = 26 (值在 8+2+12+4=26)
        t.extend_from_slice(&0x010Fu16.to_le_bytes());
        t.extend_from_slice(&2u16.to_le_bytes());
        t.extend_from_slice(&6u32.to_le_bytes());
        t.extend_from_slice(&26u32.to_le_bytes());
        // next IFD = 0
        t.extend_from_slice(&0u32.to_le_bytes());
        // ASCII 值
        t.extend_from_slice(b"CANON\0");
        let info = parse_tiff(&t);
        assert_eq!(info.camera, "CANON");
    }

    #[test]
    fn rational_shutter_and_iso_format() {
        // 手工构造：IFD0 一条 → 0x829A ExposureTime = 1/250，
        // 0x8827 ISO = 400。
        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        // IFD0 count = 2
        t.extend_from_slice(&2u16.to_le_bytes());
        // entry 1: tag=0x829A, type=5 Rational, n=1, offset=8+2+24+4=38
        t.extend_from_slice(&0x829Au16.to_le_bytes());
        t.extend_from_slice(&5u16.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&38u32.to_le_bytes());
        // entry 2: tag=0x8827, type=3 SHORT, n=1, value=400 (inline)
        t.extend_from_slice(&0x8827u16.to_le_bytes());
        t.extend_from_slice(&3u16.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&400u16.to_le_bytes());
        t.extend_from_slice(&0u16.to_le_bytes()); // 内联 4B 高 2B 填零
                                                  // next IFD = 0
        t.extend_from_slice(&0u32.to_le_bytes());
        // rational 值：num=1 den=250
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&250u32.to_le_bytes());
        let c = {
            let e = Endian { le: true };
            let mut c = Collected::default();
            let mut free = usize::MAX;
            walk(&t, &e, 8, &mut c, 0, false, &mut HashSet::new(), &mut free);
            c
        };
        assert_eq!(c.exposure_time, Some(1.0 / 250.0));
        assert_eq!(c.iso, Some(400));
        assert_eq!(format_exposure(&c), "1/250s · ISO 400");
    }

    /// 未知类型（不在 TIFF 6.0 / EXIF 类型表里的 `ty`）必须**整条跳过**，不是猜着读
    /// （审计第三批）。旧实现写的是 `type_size(ty).unwrap_or(0)`：size 变成 0 → `inline`
    /// 成立 → 从本条 value 字段起按 `n` 个字节往外读，一路读进紧邻的 next-IFD 字节，
    /// 属性窗于是拿到一句"从别处拼出来的相机型号"。畸形结构的正确答复是"没有这一条"。
    #[test]
    fn unknown_type_entry_is_skipped_not_guessed() {
        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes()); // count = 1
        t.extend_from_slice(&0x010Fu16.to_le_bytes()); // Make
        t.extend_from_slice(&99u16.to_le_bytes()); // 不存在的类型
        t.extend_from_slice(&6u32.to_le_bytes()); // n = 6，比 4 字节的 value 字段还长
        t.extend_from_slice(b"NIK0"); // value/offset 内联那 4 字节
        t.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0（旧实现会把它读进来）
        let e = Endian { le: true };
        let mut c = Collected::default();
        let mut free = usize::MAX;
        walk(&t, &e, 8, &mut c, 0, false, &mut HashSet::new(), &mut free);
        assert!(
            c.make.is_empty(),
            "未知类型不许从邻近字节拼出值：{:?}",
            c.make
        );
    }

    /// count=0 的 RATIONAL 条目必须整条跳过，不能把 next-IFD 的字节当成分母。
    #[test]
    fn zero_count_entry_is_skipped_not_read_from_neighbour() {
        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes()); // count = 1 entry
        t.extend_from_slice(&0x829Au16.to_le_bytes()); // ExposureTime
        t.extend_from_slice(&5u16.to_le_bytes()); // RATIONAL
        t.extend_from_slice(&0u32.to_le_bytes()); // n = 0（畸形）
        t.extend_from_slice(&1u32.to_le_bytes()); // value 字段 = 1
        t.extend_from_slice(&1u32.to_le_bytes()); // next IFD = 1（旧实现会当分母）
        let e = Endian { le: true };
        let mut c = Collected::default();
        let mut free = usize::MAX;
        walk(&t, &e, 8, &mut c, 0, false, &mut HashSet::new(), &mut free);
        assert_eq!(
            c.exposure_time, None,
            "count=0 不许从下一条目/next-IFD 拼出一个值"
        );
    }

    /// 0/1 曝光时间显示成 0s，不是 i64::MAX 分之一。
    #[test]
    fn zero_exposure_formats_as_zero_not_huge_denominator() {
        let c = Collected {
            exposure_time: Some(0.0),
            ..Default::default()
        };
        assert_eq!(format_exposure(&c), "0s");
    }

    // ── 合成 JPEG + ExifIfd + GPSInfo 大端字节序 ─────────────────
    //
    // 上一批单测走"裸 II + 单层 IFD"路径。真实相机 JPEG 全部走 APP1 段 +
    // ExifIfd (0x8769) / GPSInfo (0x8825) 递归，Nikon/Canon 常见 Motorola
    // 大端（`MM`）。以下合成样本一次性覆盖：
    //   ① `locate_tiff` 扫描 APP1
    //   ② 大端 `MM` 字节序 reader 分派
    //   ③ IFD0 → ExifIfd 递归读 6 个字段
    //   ④ IFD0 → GPSInfo 递归读度分秒 Rational 三元组 + ref + 高程
    //
    // 布局（相对 tiff 起点；IFD size = 2B count + N×12B entries + 4B next）：
    //   [0..8]    TIFF 头 `MM` 42 + ifd0_off=8
    //   [8..62]   IFD0 (4 条)：Make / Model(inline) / ExifIfd→70 / GPSInfo→148 / next=0
    //   [62..70]  零填充：对齐 ExifSub 起点
    //   [70..148] ExifIfd (6 条)：0x9003 / 0x829D / 0x829A / 0x8827 / 0x920A / 0xA434
    //   [148..226] GPSInfo (6 条)：0x1..0x6
    //   [226..328] 零填充（不引用）
    //   [328..444] ASCII + RATIONAL 值堆
    //
    // tiff 总长 = 444（Altitude 分母末尾）
    fn synthetic_exif_tiff() -> Vec<u8> {
        const TIFF_LEN: usize = 444;
        let mut t: Vec<u8> = Vec::with_capacity(TIFF_LEN);
        fn be16(t: &mut Vec<u8>, v: u16) {
            t.extend_from_slice(&v.to_be_bytes());
        }
        fn be32(t: &mut Vec<u8>, v: u32) {
            t.extend_from_slice(&v.to_be_bytes());
        }
        /// SHORT 单值内联：高 2 字节放值 + 低 2 字节填 0（BE 语义）
        fn entry_short(t: &mut Vec<u8>, tag: u16, v: u16) {
            be16(t, tag);
            be16(t, 3);
            be32(t, 1);
            be16(t, v);
            be16(t, 0);
        }
        /// LONG 单值内联（子 IFD 指针）
        fn entry_long(t: &mut Vec<u8>, tag: u16, v: u32) {
            be16(t, tag);
            be16(t, 4);
            be32(t, 1);
            be32(t, v);
        }
        /// ASCII 溢出：value/offset 字段放偏移
        fn entry_ascii(t: &mut Vec<u8>, tag: u16, n: u32, off: u32) {
            be16(t, tag);
            be16(t, 2);
            be32(t, n);
            be32(t, off);
        }
        /// ASCII 内联（≤ 4 字节）：值直接写在 value/offset 字段
        fn entry_ascii_inline(t: &mut Vec<u8>, tag: u16, s: &[u8]) {
            be16(t, tag);
            be16(t, 2);
            be32(t, s.len() as u32);
            let mut buf = [0u8; 4];
            buf[..s.len()].copy_from_slice(s);
            t.extend_from_slice(&buf);
        }
        /// 单值 RATIONAL 溢出
        fn entry_rational(t: &mut Vec<u8>, tag: u16, off: u32) {
            be16(t, tag);
            be16(t, 5);
            be32(t, 1);
            be32(t, off);
        }
        /// 三值 RATIONAL（GPS DMS）溢出
        fn entry_dms(t: &mut Vec<u8>, tag: u16, off: u32) {
            be16(t, tag);
            be16(t, 5);
            be32(t, 3);
            be32(t, off);
        }
        /// BYTE 单值内联（AltitudeRef）：值放首字节
        fn entry_byte(t: &mut Vec<u8>, tag: u16, v: u8) {
            be16(t, tag);
            be16(t, 1);
            be32(t, 1);
            t.push(v);
            t.extend_from_slice(&[0, 0, 0]);
        }

        // [0..8] TIFF 头
        t.extend_from_slice(b"MM");
        be16(&mut t, 42);
        be32(&mut t, 8); // IFD0 offset

        // [8..62] IFD0（4 条：Make / Model / 0x8769 / 0x8825 + next）
        be16(&mut t, 4);
        entry_ascii(&mut t, 0x010F, 6, 328); // Make "CANON\0" @ 328
        entry_ascii_inline(&mut t, 0x0110, b"R5\0"); // Model 内联
        entry_long(&mut t, 0x8769, 70); // ExifIfd → 70（下面 padding 到 70 起写 ExifSub）
        entry_long(&mut t, 0x8825, 148); // GPSInfo → 148（ExifSub 结束位置）
        be32(&mut t, 0); // next IFD
        debug_assert_eq!(t.len(), 62, "IFD0 结束（54B）");
        // 62..70 padding 对齐 ExifSub
        while t.len() < 70 {
            t.push(0);
        }

        // [70..148] ExifIfd（6 条）
        be16(&mut t, 6);
        entry_ascii(&mut t, 0x9003, 20, 334); // DateTimeOriginal @ 334
        entry_rational(&mut t, 0x829D, 354); // FNumber 28/10 @ 354
        entry_rational(&mut t, 0x829A, 362); // ExposureTime 1/250 @ 362
        entry_short(&mut t, 0x8827, 400); // ISO inline
        entry_rational(&mut t, 0x920A, 370); // FocalLength 50/1 @ 370
        entry_ascii(&mut t, 0xA434, 7, 378); // LensModel "EF50mm\0" @ 378
        be32(&mut t, 0);

        // [148..226] GPSInfo（6 条）
        be16(&mut t, 6);
        entry_ascii_inline(&mut t, 0x0001, b"N\0"); // LatRef inline
        entry_dms(&mut t, 0x0002, 388); // Latitude  @ 388
        entry_ascii_inline(&mut t, 0x0003, b"E\0"); // LonRef inline
        entry_dms(&mut t, 0x0004, 412); // Longitude @ 412
        entry_byte(&mut t, 0x0005, 0); // AltitudeRef inline = 0 (above)
        entry_rational(&mut t, 0x0006, 436); // Altitude 100/1 @ 436
        be32(&mut t, 0);

        // 230..328 空洞（不引用，零填充）
        while t.len() < 328 {
            t.push(0);
        }
        // [328..] overflow 段
        t.extend_from_slice(b"CANON\0"); // 328..334
        t.extend_from_slice(b"2024:05:15 12:30:00\0"); // 334..354
        t.extend_from_slice(&28u32.to_be_bytes());
        t.extend_from_slice(&10u32.to_be_bytes()); // 354..362 F/2.8
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&250u32.to_be_bytes()); // 362..370 1/250s
        t.extend_from_slice(&50u32.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes()); // 370..378 50mm
        t.extend_from_slice(b"EF50mm\0"); // 378..385
        while t.len() < 388 {
            t.push(0);
        }
        // [388..412] Lat 31/1, 23/1, 4464/100 = 31.395666
        for (n, d) in [(31u32, 1u32), (23, 1), (4464, 100)] {
            t.extend_from_slice(&n.to_be_bytes());
            t.extend_from_slice(&d.to_be_bytes());
        }
        // [412..436] Lon 121/1, 28/1, 2520/100 = 121.473667
        for (n, d) in [(121u32, 1u32), (28, 1), (2520, 100)] {
            t.extend_from_slice(&n.to_be_bytes());
            t.extend_from_slice(&d.to_be_bytes());
        }
        // [436..444] Alt 100/1 = 100m
        t.extend_from_slice(&100u32.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());

        // 用零填齐到 TIFF_LEN
        while t.len() < TIFF_LEN {
            t.push(0);
        }
        debug_assert_eq!(t.len(), TIFF_LEN);
        debug_assert_eq!(&t[328..334], b"CANON\0");
        debug_assert_eq!(&t[334..354], b"2024:05:15 12:30:00\0");
        debug_assert_eq!(&t[378..385], b"EF50mm\0");

        t
    }

    fn synthetic_exif_jpeg() -> Vec<u8> {
        let t = synthetic_exif_tiff();
        let tiff_len = t.len();
        // 包成 JPEG：SOI + APP1 + SOS + EOI
        let mut j: Vec<u8> = Vec::new();
        j.extend_from_slice(&[0xFF, 0xD8]); // SOI
                                            // APP1：marker len = 2 + 6 + TIFF_LEN
        let app1_len = (2 + 6 + tiff_len) as u16;
        j.extend_from_slice(&[0xFF, 0xE1]);
        j.extend_from_slice(&app1_len.to_be_bytes());
        j.extend_from_slice(b"Exif\0\0");
        j.extend_from_slice(&t);
        j.extend_from_slice(&[0xFF, 0xDA]); // SOS（终止扫描）
        j.extend_from_slice(&[0x00, 0x04, 0x01, 0x00]); // 假 SOS payload
        j.extend_from_slice(&[0xFF, 0xD9]); // EOI
        j
    }

    /// PNG 容器：8B 签名 + IHDR（占位）+ `eXIf` chunk + IEND。CRC 全零——我们的
    /// `locate_png_exif` 只走 chunk 流不校验，测试不依赖真实 PNG 解码器。
    fn synthetic_exif_png() -> Vec<u8> {
        let t = synthetic_exif_tiff();
        let mut p: Vec<u8> = Vec::new();
        p.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        // IHDR：13B 假数据（width/height/bit_depth/…）
        let ihdr: [u8; 13] = [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0];
        p.extend_from_slice(&13u32.to_be_bytes());
        p.extend_from_slice(b"IHDR");
        p.extend_from_slice(&ihdr);
        p.extend_from_slice(&[0, 0, 0, 0]); // CRC 占位
                                            // eXIf chunk
        p.extend_from_slice(&(t.len() as u32).to_be_bytes());
        p.extend_from_slice(b"eXIf");
        p.extend_from_slice(&t);
        p.extend_from_slice(&[0, 0, 0, 0]); // CRC 占位
                                            // IDAT 占位（若 EXIF chunk 定位失败，走到这里应被 locate_png_exif 早退）
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(b"IDAT");
        p.extend_from_slice(&[0, 0, 0, 0]);
        // IEND
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(b"IEND");
        p.extend_from_slice(&[0, 0, 0, 0]);
        p
    }

    /// WebP：`RIFF` + LE size + `WEBP` 头，之后 `VP8 ` 占位子 chunk 让解析器**必须**
    /// 遍历才能到 `EXIF`——这样这个测试同时验证了「跳过非 EXIF chunk」和「奇偶 pad 逻辑」
    /// 两条独立代码路径。
    fn synthetic_exif_webp() -> Vec<u8> {
        let t = synthetic_exif_tiff();
        let mut w: Vec<u8> = Vec::new();
        w.extend_from_slice(b"RIFF");
        // 总 size = 4 (WEBP) + 8+3 (VP8 payload 奇数补 1) + 8+t.len() = 23 + t.len()
        let riff_size = (4 + 8 + 3 + 1 + 8 + t.len()) as u32;
        w.extend_from_slice(&riff_size.to_le_bytes());
        w.extend_from_slice(b"WEBP");
        // VP8 占位：size=3（奇数 → 后面补一字节）
        w.extend_from_slice(b"VP8 ");
        w.extend_from_slice(&3u32.to_le_bytes());
        w.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        w.extend_from_slice(&[0x00]); // pad
                                      // EXIF chunk
        w.extend_from_slice(b"EXIF");
        w.extend_from_slice(&(t.len() as u32).to_le_bytes());
        w.extend_from_slice(&t);
        w
    }

    #[test]
    fn locate_finds_app1_in_jpeg() {
        let j = synthetic_exif_jpeg();
        // Exif\0\0 前缀 6 字节，之后就是 TIFF 头
        let off = locate_tiff(&j).expect("APP1 应被扫描到");
        assert!(off > 0, "JPEG 内 TIFF 起点必须非 0");
        assert_eq!(&j[off..off + 2], b"MM");
    }

    #[test]
    fn synthetic_jpeg_end_to_end_big_endian_full_tree() {
        let j = synthetic_exif_jpeg();
        let off = locate_tiff(&j).unwrap();
        let info = parse_tiff(&j[off..]);
        assert_eq!(
            info.camera, "CANON R5",
            "Make + Model 合并 (BE + inline Model)"
        );
        assert_eq!(
            info.capture_time, "2024:05:15 12:30:00",
            "ExifIfd 递归 + DateTimeOriginal"
        );
        assert_eq!(info.lens, "EF50mm", "ExifIfd 递归 + LensModel");
        assert_eq!(
            info.exposure, "f/2.8 · 1/250s · ISO 400",
            "ExifIfd 三 rational/short"
        );
        assert_eq!(info.focal, "50mm", "ExifIfd 递归 + FocalLength");
        assert_eq!(
            info.gps, "31.3957°N, 121.4737°E, 100m",
            "GPSInfo 递归 + DMS 三元组 + ref + altitude"
        );
    }

    #[test]
    fn big_endian_is_correctly_dispatched() {
        // 同一份 tiff 数据用 MM（BE）解析成功；若 reader 走 LE 分支所有字段
        // 都会读歪、camera 为空。作为回归哨兵。
        let j = synthetic_exif_jpeg();
        let off = locate_tiff(&j).unwrap();
        let info = parse_tiff(&j[off..]);
        assert!(!info.camera.is_empty(), "MM 字节序必须正确分派");
    }

    #[test]
    fn exif_ifd_recursion_reaches_sub_fields() {
        // 哨兵：DateTimeOriginal (0x9003) 只可能通过 0x8769 → ExifIfd 递归
        // 抵达；IFD0 层不认这个 tag（tag 值 0x9003 在 depth=0 是未定义）。
        // 断言 capture_time 非空 = 递归链路真的跑通。
        let j = synthetic_exif_jpeg();
        let off = locate_tiff(&j).unwrap();
        let info = parse_tiff(&j[off..]);
        assert!(!info.capture_time.is_empty());
        assert!(!info.exposure.is_empty());
        assert!(!info.focal.is_empty());
    }

    #[test]
    fn gps_ifd_recursion_reaches_dms_and_refs() {
        let j = synthetic_exif_jpeg();
        let off = locate_tiff(&j).unwrap();
        let info = parse_tiff(&j[off..]);
        // GPS 字段完整合成，非空即证明递归成功。
        assert!(!info.gps.is_empty());
        // 精确度到 4 位小数：31.395666 → 31.3957
        assert!(info.gps.starts_with("31.3957°N"), "lat 值错: {}", info.gps);
        assert!(info.gps.contains("121.4737°E"), "lon 值错: {}", info.gps);
        assert!(info.gps.ends_with("100m"), "altitude 后缀错: {}", info.gps);
    }

    /// 菱形子 IFD 只走一次（**挂死盲点回归**）。
    ///
    /// `MAX_DEPTH` 防的是链式成环，防不了"一个 IFD 的 N 个条目全指向同一个子 IFD"
    /// 的菱形：没有 visited 去重时递归次数是 N^层数。真实构造用 N=65535、三层
    /// （约 3MB 文件）就是 2.8e14 次，而 `read_exif` 在 **UI 线程**同步调用
    /// → 整个属性窗永久假死，且 catch_unwind / SEH / 三振一层都不沾边。
    #[test]
    fn diamond_sub_ifd_is_walked_once() {
        const N: u32 = 200;
        // IFD 字节数 = 2B count + N×12B entries + 4B next
        let ifd_sz = |n: u32| 2 + 12 * n as usize + 4;
        let a = (8 + ifd_sz(N)) as u32; // IFD0 → A
        let b = (a as usize + ifd_sz(N)) as u32; // A → B
        let c = (b as usize + ifd_sz(N)) as u32; // B → C（C 放 Model）

        fn be16(t: &mut Vec<u8>, v: u16) {
            t.extend_from_slice(&v.to_be_bytes());
        }
        fn be32(t: &mut Vec<u8>, v: u32) {
            t.extend_from_slice(&v.to_be_bytes());
        }
        /// N 条目全指向同一个子 IFD 的扇出层
        fn fan(t: &mut Vec<u8>, n: u32, target: u32) {
            be16(t, n as u16);
            for _ in 0..n {
                be16(t, 0x8769); // ExifIfd 指针
                be16(t, 4); // LONG
                be32(t, 1);
                be32(t, target);
            }
            be32(t, 0); // next IFD
        }

        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(b"MM");
        be16(&mut t, 42);
        be32(&mut t, 8); // ifd0 offset
        fan(&mut t, N, a);
        fan(&mut t, N, b);
        fan(&mut t, N, c);
        be16(&mut t, 1); // C：1 条 Model 内联
        be16(&mut t, 0x0110);
        be16(&mut t, 2); // ASCII
        be32(&mut t, 3);
        t.extend_from_slice(b"R5\0\0");
        be32(&mut t, 0);

        let e = Endian { le: false };
        let mut col = Collected::default();
        let mut visited = HashSet::new();
        let mut free = usize::MAX;
        walk(&t, &e, 8, &mut col, 0, false, &mut visited, &mut free);
        assert_eq!(
            visited.len(),
            4,
            "四个 IFD 应各走一次，实际 {}",
            visited.len()
        );
        assert!(
            col.model.contains("R5"),
            "去重后仍须走到最深层: {}",
            col.model
        );

        // 端到端：同一份菱形数据过完整入口，不得挂死、不得丢字段。
        let info = parse_tiff(&t);
        assert!(info.camera.contains("R5"), "camera 丢字段: {}", info.camera);
    }

    // ── 容器分派：PNG eXIf / WebP EXIF ─────────────────────────
    //
    // `parse_tiff` 与 `synthetic_exif_tiff` 是上一批已经钉稳的（同一份 444B
    // MM 大端 + ExifIfd + GPSInfo）。这两组只测「入口分派」——同一份 TIFF 塞进
    // PNG / WebP 容器后 `locate_tiff` 能找到、字段完整解出。测的是新增代码，
    // 不重跑递归。

    #[test]
    fn locate_finds_png_exif_chunk() {
        let p = synthetic_exif_png();
        let off = locate_tiff(&p).expect("eXIf chunk 应被扫描到");
        assert_eq!(&p[off..off + 2], b"MM", "PNG 内 TIFF 起点应指向大端头");
        // 精确偏移：sig(8) + IHDR chunk(4+4+13+4) + len(4) + type(4) = 41
        assert_eq!(off, 41);
    }

    #[test]
    fn locate_finds_webp_exif_chunk() {
        let w = synthetic_exif_webp();
        let off = locate_tiff(&w).expect("EXIF chunk 应被扫描到");
        assert_eq!(&w[off..off + 2], b"MM", "WebP 内 TIFF 起点应指向大端头");
        // 精确偏移：RIFF hdr(12) + VP8 chunk(8+3+1 pad) + fourcc(4) + size(4) = 32
        assert_eq!(off, 32);
    }

    #[test]
    fn synthetic_png_end_to_end_uses_same_tiff_parser() {
        // 同一份 TIFF，换容器不改解析：断言完全等价于 JPEG 版。
        let p = synthetic_exif_png();
        let off = locate_tiff(&p).unwrap();
        let info = parse_tiff(&p[off..]);
        assert_eq!(info.camera, "CANON R5");
        assert_eq!(info.capture_time, "2024:05:15 12:30:00");
        assert_eq!(info.lens, "EF50mm");
        assert_eq!(info.exposure, "f/2.8 · 1/250s · ISO 400");
        assert_eq!(info.focal, "50mm");
        assert_eq!(info.gps, "31.3957°N, 121.4737°E, 100m");
    }

    #[test]
    fn synthetic_webp_end_to_end_odd_size_padding() {
        // 关键副验证：VP8 子 chunk size=3 奇数、补一字节 pad；若 pad 逻辑错，
        // 遍历 EXIF 起点会偏一字节、读到非 `MM` 头，locate_tiff 返回 None。
        let w = synthetic_exif_webp();
        let off = locate_tiff(&w).unwrap();
        let info = parse_tiff(&w[off..]);
        assert_eq!(info.camera, "CANON R5");
        assert_eq!(info.exposure, "f/2.8 · 1/250s · ISO 400");
        assert_eq!(info.gps, "31.3957°N, 121.4737°E, 100m");
    }

    /// 预算本身的上界**取自段落长度**（规范§六 第 10 条的完整判据），并另有一个
    /// 绝对天花板兜住大段落。
    #[test]
    fn exif_budget_is_length_derived_and_capped() {
        assert_eq!(exif_budget(2_000), 1_000);
        assert_eq!(exif_budget(0), 0, "空段落不该留出任何轮数");
        assert_eq!(
            exif_budget(600_000),
            EXIF_MAX_ENTRIES,
            "大段落应由绝对天花板兜住，而不是随长度线性放大成百万轮"
        );
    }

    /// **挂死盲点回归（K41）**：轮数不得取自文件内的 `count`。
    ///
    /// 这里给 7 格预算，而段落里的 `count` 是 0xFFFF=65535——上一版的实现里
    /// `visited` 去重只消掉了 `N^层数` 的递归爆炸，每条 IFD 的轮数仍然由 `count`
    /// 决定，于是总工作量 ≈ 访问数 × 段落长度/12，对文件长度**不是线性**。
    /// 断言只认一件事：预算扣光即停。
    #[test]
    fn rounds_are_bounded_by_budget_not_by_count() {
        let mut t = vec![0xFFu8; 200];
        t[..2].copy_from_slice(b"MM");
        t[2..4].copy_from_slice(&42u16.to_be_bytes());
        t[4..8].copy_from_slice(&8u32.to_be_bytes());
        let e = Endian { le: false };
        let mut col = Collected::default();
        let mut budget = 7usize;
        walk(
            &t,
            &e,
            8,
            &mut col,
            0,
            false,
            &mut HashSet::new(),
            &mut budget,
        );
        assert_eq!(
            budget, 0,
            "预算应被扣光：说明轮数是预算说了算，不是 count 说了算"
        );
    }

    /// 大文件的读取上限是**读多少字节**这一层挡住的，不是解析层：2GB 的文件只进
    /// 内存 12MiB（`read_exif` 在 UI 线程同步跑，整读就是冻结）。
    #[test]
    fn read_head_never_exceeds_cap() {
        let dir = crate::core::paths::test_scratch_root();
        let f = dir.join("rupix_exif_cap_probe.bin");
        let big = 1024 * 1024 + 123;
        std::fs::write(&f, vec![0u8; big]).ok();
        let got = read_head(&f, 4096).unwrap_or_default();
        std::fs::remove_file(&f).ok();
        assert_eq!(got.len(), 4096, "超过上限时只应取文件头那么多字节");
    }

    /// 把 `synthetic_exif_jpeg` 的 APP1 推到文件里第 `pad` 字节之后（COM 段填充，
    /// 单段最长 65533 ⇒ 循环补），其余结构与真 JPEG 同形。
    fn jpeg_with_app1_behind(pad: usize) -> Vec<u8> {
        let j = synthetic_exif_jpeg();
        let mut out: Vec<u8> = Vec::with_capacity(j.len() + pad + 8);
        out.extend_from_slice(&j[..2]); // SOI
        let mut left = pad;
        while left > 0 {
            let seg = left.min(65533);
            out.extend_from_slice(&[0xFF, 0xFE]); // COM：合法的"非 EXIF 前置段"
            out.extend_from_slice(&((seg + 2) as u16).to_be_bytes());
            out.extend_from_slice(&vec![0x20; seg]);
            left -= seg;
        }
        out.extend_from_slice(&j[2..]); // APP1 起（含其后的 SOS / EOI）
        out
    }

    /// 写进临时目录、跑 `read_exif`、删掉。属性窗那条路就是"给路径读一次"，测试同形。
    fn read_exif_of(bytes: &[u8], name: &str) -> ExifInfo {
        let dir = crate::core::paths::test_scratch_root();
        let f = dir.join(name);
        std::fs::write(&f, bytes).expect("临时语料写得动");
        let info = read_exif(&f);
        std::fs::remove_file(&f).ok();
        info
    }

    /// ★ m104 判据的正半条：EXIF 落在**默认头档以内**（这里 200 KiB）必须照常解析出来。
    /// 收档不是"少读点字节"，它的前提就是这一条成立——没有这条，改法就是拿功能换性能。
    #[test]
    fn exif_behind_a_fat_jpeg_header_still_reads() {
        let info = read_exif_of(&jpeg_with_app1_behind(200 * 1024), "rupix_m104_in_head.jpg");
        assert!(
            !info.camera.is_empty(),
            "APP1 在 200 KiB 处（默认档 256 KiB 以内）⇒ 拍摄信息必须读得到，实读 {info:?}"
        );
    }

    /// ★ m104 判据的反半条：EXIF 落在默认头档**之外**（300 KiB）照实为空——不许猜、
    /// 不许拿上一张的缓存顶着。这条用例存在的意义就是把"本轮新收的缺口"钉成一个断言，
    /// 而不是只写在文档里等人发现。
    #[test]
    fn exif_beyond_the_head_cap_reads_empty_not_guessed() {
        let info = read_exif_of(
            &jpeg_with_app1_behind(300 * 1024),
            "rupix_m104_past_head.jpg",
        );
        assert_eq!(
            info,
            ExifInfo::default(),
            "APP1 在 300 KiB 处（超出默认档）⇒ 照实全空，这是 m104 登记的已知缺口"
        );
    }

    /// WebP 的 `EXIF` chunk 常在尾部：它单独保留深档，所以"EXIF 在 300 KiB 之后"这一格
    /// **仍然要读得到**。这条用例钉的是"收紧默认档不许顺手把 WebP 一起砍掉"。
    #[test]
    fn webp_tail_exif_keeps_its_deep_read() {
        let t = synthetic_exif_tiff();
        let mut w: Vec<u8> = Vec::new();
        w.extend_from_slice(b"RIFF");
        // VP8 占位段撑到 300 KiB（> 默认头档），EXIF chunk 排在它后面 = 真实 WebP 的常见摆位
        let vp8 = 300 * 1024;
        let riff_size = (4 + 8 + vp8 + 8 + t.len()) as u32;
        w.extend_from_slice(&riff_size.to_le_bytes());
        w.extend_from_slice(b"WEBP");
        w.extend_from_slice(b"VP8 ");
        w.extend_from_slice(&(vp8 as u32).to_le_bytes());
        w.extend_from_slice(&vec![0xAA; vp8]);
        w.extend_from_slice(b"EXIF");
        w.extend_from_slice(&(t.len() as u32).to_le_bytes());
        w.extend_from_slice(&t);
        let info = read_exif_of(&w, "rupix_m104_webp_tail.webp");
        assert!(
            !info.camera.is_empty(),
            "WebP 尾部 EXIF 在 300 KiB 之后 ⇒ 走深档，拍摄信息不许因为默认档收紧而丢"
        );
    }
}
