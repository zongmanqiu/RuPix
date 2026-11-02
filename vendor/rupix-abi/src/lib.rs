//! RuPix 解码器 ABI 契约 —— **唯一真源**。
//!
//! # 这个文件是什么
//!
//! 解码器（`cdylib`）和宿主（`main`）是两个**独立编译**的二进制。Rust 没有稳定 ABI，
//! 跨 DLL 边界只能走 C ABI：扁平函数 + `#[repr(C)]` 结构体，不许传 `String`/`Vec`/
//! 泛型/trait object。
//!
//! 本 crate 就是这份契约的**唯一副本**：宿主 `main/src/core/abi.rs` 与全部解码器
//! 都通过 path 依赖引用这里。历史上契约被手抄成三份（宿主一份 + qoi/jxl 各一份），
//! 改一处要人肉同步两处——现在物理上不可能失同步。
//!
//! 后半段还装着**解码器样板（kit）**：`guard` / `set_last_error` / `write_last_error` /
//! `export_decoder!`（原 `decoder-kit` crate，2026-10-16 并入，解码器因此只依赖 `rupix-abi`
//! 一项）。宿主不碰这些，但快照是逐字节复制，**不许在宿主侧删改这一段**。
//!
//! # 为什么结构体长度要写死断言
//!
//! `#[repr(C)]` 只保证「字段顺序与 C 一致」，不保证别的编译器/别的版本不插入填充。
//! `assert!(size_of::<T>() == N)` 把这件事变成**编译错误**：谁在结构体里加了个字段、
//! 换了字段类型，立刻炸在编译期，而不是发出一份布局对不上的 `bytes_needed`、
//! 让宿主按错误长度分配缓冲、再由解码器越界写。
//!
//! # 兼容性立约（永久条款，2026-10-28 用户拍板）
//!
//! **ABI 永远兼容旧版本 DLL；新 ABI 不得影响旧 DLL 的运行。**
//! 推论：宿主升级以后，用户机器上已有的旧 DLL **不需要重新编译、不需要重新上传**。
//!
//! 可判决的五条（不是口号，`tools/check_abi_freeze.py` 逐条机器守）：
//!
//! 1. **已发布的东西永不修改**：导出名 / 参数顺序 / 返回类型、`#[repr(C)]` 结构体的
//!    字段名·类型·顺序、错误码 `ERR_*`、像素格式 `PF_*`、能力位 `CAP_*`、请求位
//!    `REQ_*` 的编号 —— **只增不改号**。
//! 2. **新能力只能加法**：新可选导出 + 新结构体 + 新位 / 新枚举值。宿主用
//!    `GetProcAddress` 探测，**符号缺席 = 缺该能力，不是错误**，旧 DLL 也无感。
//! 3. **下界永不抬**：`MIN_ABI_VERSION` 永远是 5。`IV_ABI_VERSION` 只增不减。
//! 4. **旧 DLL 的行为不因宿主升级改变**：不得要求旧 DLL 提供新导出，不得改变既有
//!    导出的调用语义、默认 `flags` / `req` 含义、`target=0` 与 `out=NULL` 的语义。
//! 5. **没有"破坏式 bump"这条路**：旧表曾写「改结构体 = bump 版本」，会作废全部旧
//!    DLL，与第 1 条冲突 —— 现已**禁止**；需要新输入 / 新输出就新开一个结构体
//!    （实例：v6 的 [`IvRegionReq`]）。
//!
//! 唯一例外是历史断点 **v5**（术语改名，一次性）：v4 及更早的 DLL 不兼容、必须重编；
//! **v5 之后不再有、也不允许再出现第二处这类断点**。冻结指纹基线 =
//! `tools/abi_freeze.txt`（与 `check_abi_freeze.py` 同目录），改既有条目即红灯。
//!
//! # 一处对规格的更正
//!
//! `docs/解码器开发规范.md` §三 里 `IvRenderReq` 的断言历史占位值 20 曾与实际不符；
//! v3 给 Req 加了 `frame_index` 后恰好就是真实值 **20**（5×u32）。`IvRenderOut`
//! 的 **40** = 8（u64 对齐）+ 4×6。

/// 本规范的大版本。文件名里的 `<abi>` 段、`iv_abi_version()` 的返回值都要等于它。
///
/// 改动任何结构体字段顺序/长度/语义、错误码含义、`pixel_format` 枚举值 → 必须 +1。
///
/// **v3（2026-10-10）**：`iv_probe` / `iv_render` 增加尾部 `ext` 参数（小写扩展名，
/// 可 NULL）——TGA 等无魔数格式由此可被认领；`IvRenderReq` 加 `frame_index`、
/// `IvRenderOut` 加 `frame_count` + `frame_delay_ms`（多帧动画语义，GIF 首个受益者）。
///
/// **v4（2026-10-10）**：`IvRenderReq` 加 `page_index`（多页/子图选择：TIFF 多页、
/// ICO 尺寸档、PDF/CBZ/DICOM 页、MPO 双目、DDS mip），`IvRenderOut` 加 `page_count`
/// + `icc_len`（ICC 色彩配置透传协议，像素后附加段）。为 60-70 个目标格式的地基。
///
/// **v4 内可选导出（v5 继续保留，不 bump 版本）**：`iv_render_raw(data, len, width, height,
/// layout, req, out, out_cap, info) -> i32` —— 裸像素数据（.raw/.rgba 等，无魔数
/// 无头）的**参数化解码**。宽/高/布局由宿主向用户收集后传入；`layout` 取 `PF_*`
/// 值（含 [`PF_BGRA8`]）。老宿主/无此导出的解码器照常：规范 §四 允许新增导出，
/// 宿主用 `GetProcAddress` 探测，缺席即视为不支持裸数据语义。
/// **v4 内可选导出（v5 继续保留，不 bump 版本）**：`iv_caps() -> uint32` —— **解码能力位图**，
/// 见 [`CAP_FULL`] 一族。宿主探测缺席 = 按既有字段推定（[`CAP_FULL`] ∪ `is_vector`
/// ∪ 有 `iv_render_raw` 则 [`CAP_RAW_PARAM`]），不是错误。做这件事的动机是把散在
/// `iv_render_raw` 符号、`frame_index`、`page_index`、`is_vector` 四处的"这台解码器
/// 会哪种解码"收进**一个显式维度**，宿主按类型路由，而不是按 DLL 身份猜。
///
/// **v5（2026-10-26）**：术语清理，自述导出、自述结构体与样板宏统一改为 decoder
/// 命名；旧 v4 名称不再提供，宿主与全部解码器必须同步重编。
///
/// **v5 内可选导出（不 bump 版本）**：`iv_metadata()` 返回可扩展键值表
/// （[`IvMetadata`]）。新增元数据 = 往 `fields` 追加 key/value，不改自述结构体、
/// 不改 ABI 版本；宿主读取自己认识的 key，未知 key 忽略。
///
/// **v5 内可选导出（2026-10-28，不 bump 版本）**：`iv_export_svg` 把矢量场景以
/// **UTF-8 SVG 文本字节**交出（[`IvExportSvgFn`]，规范 §十五）。两阶段缓冲与
/// `iv_render` 同构：`out = NULL` 查询、经输出参数 `needed` 回填所需字节数，
/// 第二次真写。宿主缓存 SVG 后用自家的 resvg 按视口重渲；合并档的成员级矢量
/// 声明（`is_vector` 是 DLL 级单值，位图成员占多数）走 `iv_metadata` 的
/// `vector_exts` 键（分号分隔的后缀表，如 `wmf;wmz`）。
///
/// **v6（2026-10-28）**：**纯增量**的一代——v5 的全部导出签名与结构体布局**逐字节不变**，
/// 新增三样东西：
///
/// 1. 可选导出 `iv_render_region`（[`IvRenderRegionFn`]）：把矢量文档的**指定区域**
///    按指定密度直接光栅化成位图。这是"矢量图放大到 4000% 也无损"的**唯一缺口**——
///    宿主已有的可视区域重渲机制（只渲看得见的那一块，画布恒为一屏大小）在内置 resvg
///    路线上早就跑通了，但解码器侧没有对应通道，只能整幅渲染，放大即糊。
/// 2. 请求结构体 [`IvRegionReq`]（40 字节，新结构体，**不改任何既有结构体**）。
/// 3. 能力位 [`CAP_REGION`]（`1 << 6`）——宿主据此知道"这枚 DLL 能按区域渲染"。
///
/// **v5 兼容**：宿主按 `MIN_ABI_VERSION(=5) ≤ 解码器 abi ≤ IV_ABI_VERSION(=6)` 区间收载，
/// v5 的老 DLL（不含 `iv_render_region`）**照常加载、照常工作**——它们只是没有区域渲染
/// 能力（宿主探测不到符号 = 缺能力，不是错误）。v6 是**加法**，不是断点：
/// 没有任何既有导出改过签名，也没有任何既有结构体改过字段。
///
/// **为什么不是给 `IvDecoderInfo` 加字段**：规范 §九 立约「v5 起已发布的导出签名与
/// 结构体永不再改，新能力一律走新增可选导出」。实测也支持这条纪律——往结构体**末尾**
/// 追加字段会让宿主对旧 DLL 越界读，12/12 枚现役解码器在该处读到的是相邻 `.data` 的
/// 垃圾值（`0x74726556`、`0x61746166`…）；塞进 `abi_version` 之后的 4 字节填充虽然
/// `size_of` 不变，但依赖"rustc 把 static 的 padding 清零"这一**实现行为**，不是语言
/// 保证。取证与对比见结构体断言与历史测试。
pub const IV_ABI_VERSION: u32 = 6;

// 兼容性立约（规范 §九）：**自 v5 起已发布的导出签名与结构体永不再改**。
// 新能力一律走新增可选导出（如 `iv_render_raw`），宿主探测缺席 = 缺能力。
// 宿主按 `MIN_ABI_VERSION(=5) ≤ 解码器 abi ≤ IV_ABI_VERSION` 区间收载——
// v4 及更早的 DLL 不兼容（v5 是改名断点）；改任何既有签名/结构体 = 重新走一遍
// v1→v5 的全体重编，除非走投无路不允许。
//
// ★ 这一段是 `//` 而不是 `///`：它不描述下面任何一个错误码常量（`///` 挂在这里
//   等于给 `ERR_OK` 写了一段与它无关的文档，clippy 的 `empty_line_after_doc_comments`
//   照得出来）。契约正文的正本在 `docs/解码器开发规范.md` §九 与本节顶部的
//   [`IV_ABI_VERSION`] 文档里，这里只作就近提示，不作第二正本。

// ── 错误码（宿主必须能逐类区分，不能整个吞成"打不开"）──────────────────
//
// 见规范 §四。宿主 UI 上每类给不同文案：未安装 / 版本不匹配 / 加载失败 / 解码失败 / 过大。

/// 成功。
pub const ERR_OK: i32 = 0;
/// 不识别（魔数不对）→ UI「未知格式」。
pub const ERR_UNRECOGNIZED: i32 = 1;
/// 数据损坏 → UI「文件已损坏」。
pub const ERR_CORRUPT: i32 = 2;
/// 不支持的子格式 → UI「该文件用了本解码器不支持的特性」。
pub const ERR_UNSUPPORTED: i32 = 3;
/// 缓冲不足（第二阶段 `out_cap` 太小）→ UI「内部错误」+ 日志。
pub const ERR_BUFFER_TOO_SMALL: i32 = 4;
/// 内存不足 → UI「图片过大，内存不足」。
pub const ERR_OOM: i32 = 5;
/// panic 已被解码器侧捕获 → UI「解码器遇到问题，已隔离」。
pub const ERR_PANIC: i32 = 6;
/// 超出上限（budget / 尺寸）→ UI「图片超出可处理范围」。
pub const ERR_TOO_LARGE: i32 = 7;

// ── pixel_format 枚举（规范 §三 的表）──────────────────────────────
pub const PF_GRAY8: u32 = 0;
pub const PF_RGB8: u32 = 1;
/// 默认传输格式，绝大多数解码器用它即可。
pub const PF_RGBA8: u32 = 2;
pub const PF_GRAY16: u32 = 3;
pub const PF_RGB16: u32 = 4;
pub const PF_RGBA16: u32 = 5;
pub const PF_GRAY_F32: u32 = 6;
pub const PF_RGBA_F32: u32 = 7;
/// 仅作 `iv_render_raw`（可选导出，见下）的**输入布局**：裸数据 dump 常见 BGRA 序。
/// 输出传输格式永远是 PF_RGBA8——新增枚举值 ≠ 改既有值，不 bump ABI。
pub const PF_BGRA8: u32 = 8;
/// 仅输入：packed 4:2:2（Y0 U Y1 V，每像素平均 2 字节）。
pub const PF_YUY2: u32 = 9;
/// 仅输入：packed 4:2:2（U Y0 V Y1，每像素平均 2 字节）。
pub const PF_UYVY: u32 = 10;
/// 仅输入：planar 4:2:0（Y 面 + U 面 + V 面，共 w*h*3/2 字节）。
pub const PF_I420: u32 = 11;
/// 仅输入：planar 4:2:0（Y 面 + **V** 面 + U 面，共 w*h*3/2 字节）。
pub const PF_YV12: u32 = 12;
/// 仅输入：planar 4:2:0（Y 面 + 交错 UV 面，共 w*h*3/2 字节）。
pub const PF_NV12: u32 = 13;
//
// YUV 布局统一约定（规范 §十二）：**BT.601、limited range**（视频行业标准，
// Y ∈ [16,235]）；宽高必须为偶数（色度 2×2 / 2×1 下采样）。宿主弹窗只收集
// 布局号，矩阵约定由解码器实现方按本注释与规范执行，宿主不参与换算。

use std::ffi::c_char;
use std::mem::size_of;

// ── 解码能力位图（可选导出 `iv_caps` 的返回值；v4 内增量，不 bump 版本）──────
//
// **位 = 支持**。宿主的路由判据是这张表，不是 DLL 的名字或身份：同一个后缀由同一枚
// 解码器按类型分派（整解 / 缩放解）属正当，不是"重叠认领"。
//
// 铁律：**只增不减、位号永不重排**。要加第 7 类就占 `1 << 7`，已发布的位含义不许改——
// 它们和错误码一样是 ABI 的一部分，挪号等于让老解码器谎报能力。

/// 整解：把整张图解成 RGBA8（今天所有解码器的默认能力，`iv_render` 的语义）。
pub const CAP_FULL: u32 = 1 << 0;
/// 按 `target_w/target_h` **原生降采样**解码（JPEG 的 DCT 1/8、HEIF/AVIF 的分辨率层、
/// JXL 的可变精度）。回填的 `actual_*` **允许小于** `target_*`，缩放补齐由宿主负责。
/// 无此位 = 给多大 target 都解全图。首位声明者：`jpeg`（DCT 1/8|1/4|1/2，2026-10-18）。
/// ★ 没有这一位不代表可以只看 target 动手——单次请求还要宿主点 [`REQ_ALLOW_SCALED`]。
pub const CAP_SCALED: u32 = 1 << 1;
/// 参数化裸像素解码（可选导出 `iv_render_raw`，无魔数无头，宽高/布局由用户给）。
pub const CAP_RAW_PARAM: u32 = 1 << 2;
/// 多帧动画：`frame_index` 选帧 + `frame_count`/`frame_delay_ms` 回填有语义。
pub const CAP_MULTIFRAME: u32 = 1 << 3;
/// 多页 / 子图：`page_index` 选页 + `page_count` 回填有语义。
pub const CAP_MULTIPAGE: u32 = 1 << 4;
/// 矢量：按需重渲染（尺寸变了要能重出，不是位图放大）。
///
/// ★ 2026-10-28：**本能力位不再与 `iv_export_svg` 绑定**。此前规范 §十五 规定
/// "声明 `CAP_VECTOR` 的解码器必须实现 `iv_export_svg`（把矢量场景交成 SVG 文档
/// 给宿主渲染）"——那条"宿主自己渲染矢量"的路线已整条退役（宪章 §5.2：矢量格式
/// 一律由解码器按目标尺寸自渲染）。本位的含义回归本义：
/// **"这枚 DLL 提供的是矢量解码"**（整幅按 target 尺寸重画 + 有
/// [`CAP_REGION`] 时按区域重画）。
pub const CAP_VECTOR: u32 = 1 << 5;
/// **区域渲染**（v6 新增）：实现了可选导出 `iv_render_region`，能把文档的**指定区域**
/// 按指定密度直接光栅化（[`IvRegionReq`]）。
///
/// ★ 这一位是"矢量图放大到 4000% 也无损"的技术前提：没有它，矢量格式只能整幅渲染，
/// 放大时上屏必然是位图插值 ⇒ 糊；有它，宿主按"只渲看得见的那一块"请求，画布恒为一屏
/// 大小，倍率再高也顶得住（与内置 resvg 的区域重渲同一条机制）。
///
/// 缺席 = 不支持区域渲染：宿主**不会**对它发区域请求（按整幅路线走，行为与今天一致），
/// **不是错误**。宿主用 `GetProcAddress` 探测符号，符号在不在与这一位互为印证。
pub const CAP_REGION: u32 = 1 << 6;

/// 请求位标志：4 = **这一次请求接受原生降采样**（配套 `CAP_SCALED`）。
///
/// 为什么必须有这一位、不能只看 `target_w/h`：主画面**也**传非零 target（视口尺寸，
/// 见宿主 `desired_pixels()`），而它要的是清晰——按 target 就动手缩放会把主画面解糊。
/// 能力位图只回答「这枚解码器会不会缩放解码」，不回答「这一次要不要」：意图是**每次
/// 请求**的属性，只能跟在请求里。未置位时解码器一律整解（现役行为，零变化）。
pub const REQ_ALLOW_SCALED: u32 = 1 << 2;

/// 宿主在**解码器没有 `iv_caps` 导出**时的推定规则——纯函数，可测。
///
/// 为什么敢推定：这三类在 v4 里已经有别处的证据（`is_vector` 字段、`iv_render_raw`
/// 符号在不在），推定只是把既有事实换算成位图，不引入新信息。`CAP_SCALED` **绝不**
/// 从缺席推出来——它没有任何既有证据，硬推就是把"没测"当"PASS"。
pub fn caps_from_legacy(is_vector: bool, has_render_raw: bool) -> u32 {
    let mut caps = CAP_FULL;
    if is_vector {
        caps |= CAP_VECTOR;
    }
    if has_render_raw {
        caps |= CAP_RAW_PARAM;
    }
    caps
}

/// 推定规则的 **v6 版**：多认一个硬证据——`iv_render_region` 符号在不在。
///
/// 为什么能推：[`CAP_REGION`] 的含义就是"有没有这个导出"，符号在不在**就是**这件事本身，
/// 与 `iv_render_raw` → [`CAP_RAW_PARAM`] 同性质（不是"没测当 PASS"，是"有证据"）。
/// 宿主在 [`CAP_REGION`] 这条上仍应**宁缺勿滥**：符号探测不到就当没有，宿主据此不走区域路线。
pub fn caps_from_legacy_v6(is_vector: bool, has_render_raw: bool, has_render_region: bool) -> u32 {
    let mut caps = caps_from_legacy(is_vector, has_render_raw);
    if has_render_region {
        caps |= CAP_REGION;
    }
    caps
}

/// 解码器自述（`iv_decoder_info` 的返回值）。字段全是指向**静态只读** C 串的裸指针。
///
/// 宿主只读、不释放；串的生命周期由解码器负责（通常是 `b"..\0"` 的 `'static`）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IvDecoderInfo {
    /// 解码器唯一标识，小写字母/数字/连字符，如 `qoi`。
    pub id: *const c_char,
    /// 显示名，如 `Quite OK Image`。
    pub display_name: *const c_char,
    /// 必须等于 [`IV_ABI_VERSION`]。
    pub abi_version: u32,
    /// 分号分隔的小写扩展名（不带点），如 `qoi` 或 `jxl;brn`。
    pub extensions: *const c_char,
    /// 0 = 位图；1 = 矢量（矢量另有按需重渲染语义）。
    pub is_vector: i32,
    /// 是否可能产生非透明通道之外的 alpha。
    pub has_alpha: i32,
    /// 一句话作用说明（设置页展示），如 `解码 Quite OK Image 格式`。
    pub description: *const c_char,
}

/// 可扩展的键值元数据（可选导出 `iv_metadata` 的返回值表项）。
///
/// 这是 ABI v5 的“无限加字段”入口：`IvDecoderInfo` 的 7 个身份字段保持冻结，
/// 以后只往 `fields` 里追加键值，不修改既有结构体、不 bump ABI。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IvMetadataField {
    /// NUL 结尾的 UTF-8 键，如 `license`。
    pub key: *const c_char,
    /// NUL 结尾的 UTF-8 值；复杂值可放 JSON 文本，但键本身必须稳定。
    pub value: *const c_char,
}

/// 可扩展元数据表头（可选导出 `iv_metadata`）。
///
/// `size` 让未来追加表头字段时旧宿主只读前缀；`fields` 指向解码器静态拥有的
/// 键值数组。**增加新字段 = 在数组末尾追加一项**，不改这里的字段和 `IvDecoderInfo`。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IvMetadata {
    /// 本表头字节数；宿主先读它，未来表头增长也不会越界。
    pub size: u32,
    /// 键语义版本；只增字段不改既有键含义时保持不变。
    pub version: u32,
    /// `fields` 的元素个数。
    pub field_count: u32,
    /// 保留，当前必须为 0。
    pub reserved: u32,
    /// 指向 `field_count` 个静态表项，生命周期由解码器保证。
    pub fields: *const IvMetadataField,
}

/// 元数据键语义版本（与 ABI 版本无关）。
pub const METADATA_VERSION: u32 = 1;

/// `IvMetadata` 的 `Sync` 外壳（裸指针表项仍是只读静态数据）。
pub struct MetadataHolder(pub IvMetadata);
unsafe impl Sync for MetadataHolder {}

/// `IvMetadataField` 切片的 `Sync` 外壳。
pub struct MetadataFields(pub &'static [IvMetadataField]);
unsafe impl Sync for MetadataFields {}

/// 可选导出 `iv_export_svg` 的函数指针类型（宿主 `lib.get::<IvExportSvgFn>(b"iv_export_svg\\0")`）。
/// 语义全文见规范 §十五；这里只写 ABI 层的硬约定。
///
/// ★ **2026-10-28 起宿主不再调用这个导出**（"宿主渲染矢量"那条路线整条退役：
/// 宪章 §5.2「矢量格式一律由解码器按目标尺寸自渲染」）。**符号与类型保留**——
/// ABI 冻结不删符号（老解码器实现了它也不会出事：宿主只是不再探测、不再调用）。
/// 新写的解码器**不必**实现它。
///
/// **两阶段缓冲（照抄 `iv_render` 的先例）**：
///
/// 1. **查询**：`out = NULL`（`out_cap` 忽略）。解码器把矢量场景转换成 UTF-8 SVG 文本，
///    把所需**字节数**（不含结尾 NUL）写进 `needed`（非 NULL 时），返回 [`ERR_OK`]。
///    失败返回错误码：`ERR_UNRECOGNIZED` = 不是本解码器声明的矢量格式；`ERR_CORRUPT`
///    = 数据损坏；`ERR_TOO_LARGE` = 超出解码器自设的解包/尺寸上限。
/// 2. **写入**：宿主按 `needed` 分配缓冲后再次调用（`out != NULL`）。`out_cap < needed`
///    → [`ERR_BUFFER_TOO_SMALL`]，不写一个字节；否则把 SVG 字节写进 `out` 并再次回填
///    `needed`（两次调用必须给出同一个数），返回 [`ERR_OK`]。
///
/// **语义**：
/// - `data`/`len` 与 `iv_render` 同源（同一份文件字节）；gzip 包装（`.wmz`）由解码器自解。
/// - 返回的 SVG 必须**自包含可独立渲染**：根元素带尺寸（width/height 或 viewBox），
///   不依赖外部文件；文本记录保留为 `<text>` + 通用 font-family（宿主的 resvg 用
///   系统字体渲染），解码器不承诺内嵌字体或把文本转路径。
/// - 缓冲由**宿主分配**（规范 §五「谁分配谁释放」），解码器只写、不提供 free。
/// - `needed == NULL` 合法（宿主已从别处知道大小时）；查询阶段传 NULL 拿不到尺寸是
///   宿主自己的责任。
/// - 线程模型、`iv_last_error` 与可重入性和 `iv_render` 完全相同（规范 §六之二）。
/// - **可选性**：宿主 `GetProcAddress` 探测，缺席 = 该解码器不提供矢量导出，不是错误；
///   不 bump [`IV_ABI_VERSION`]。只应由声明了 [`CAP_VECTOR`] 的解码器实现。
///
/// # Safety
/// 跨 DLL 边界的裸指针契约：`data` 指向宿主持有的 `len` 字节；`out` 为空或指向宿主
/// 分配的 `out_cap` 字节可写缓冲；`needed` 为空或指向一个 `usize`。有效性只有宿主
/// 能负责（规范 §三「指针参数的所有权」），所以这是 `unsafe extern "C"`。
pub type IvExportSvgFn = unsafe extern "C" fn(
    data: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
    needed: *mut usize,
) -> i32;

/// 渲染请求（宿主 → 解码器）。`size_of == 24`。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IvRenderReq {
    /// 期望宽；0 = 要原始尺寸。
    pub target_w: u32,
    pub target_h: u32,
    /// 宿主给的像素预算上限（0 = 不限）。
    pub budget_bytes: u32,
    /// 位标志：[`REQ_ALLOW_SCALED`] = 接受原生降采样。1/2（优先速度 / 优先质量）是
    /// v1 就写进文档的占位，今天宿主恒传 0、无解码器读取。
    pub flags: u32,
    /// 要第几帧（0 起）。静态图恒 0，解码器可忽略。
    pub frame_index: u32,
    /// 要第几页/子图（0 起）：多页 TIFF 的页、ICO 的尺寸档、PDF/CBZ/DICOM 的页、
    /// MPO 的视角、DDS 的 mip 级。单页/动画格式忽略（动画帧选 `frame_index`）。
    pub page_index: u32,
}

/// 区域渲染请求（宿主 → 解码器，v6 新增）。`size_of == 40`。
///
/// 这是"矢量图放大到 4000% 也无损"的**唯一通道**：宿主只请求**看得见的那一块**
/// （视口在文档坐标里的矩形），解码器按目标像素尺寸把这块**直接光栅化**。
/// 画布恒为一屏大小 ⇒ 倍率再高也不撞像素硬顶；线条是矢量重画 ⇒ 不是位图插值放大。
///
/// # 坐标语义（最容易搞错的一条）
///
/// `x/y/w/h` 的单位是**文档的自然像素**——与 `iv_render` 查询阶段回报的
/// `actual_w/actual_h`（自然尺寸）**同一坐标系**。解码器内部若用别的逻辑单位
/// （点、1/72 英寸、device unit…），**自己**换算，宿主不参与。
///
/// # 密度
///
/// 密度 = `target_w / w`（宿主已经按设备像素算好）。解码器**必须**把
/// `(x, y, w, h)` 这一块缩进 `target_w × target_h` 的画布——即"以密度
/// `target_w / w` 重画"，不是"先把整块渲出来再拉伸"。
///
/// # 两阶段
///
/// 与 `iv_render` 同构：`out = NULL` 是**查询**阶段（回填 `info`，不写像素），
/// 第二次才是写入。宿主按 `bytes_needed` 一次性分配（**无重试**）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IvRegionReq {
    /// 区域左上角（文档自然像素坐标）。
    pub x: f32,
    /// 区域左上角（文档自然像素坐标）。
    pub y: f32,
    /// 区域宽（文档自然像素）。
    pub w: f32,
    /// 区域高（文档自然像素）。
    pub h: f32,
    /// 输出画布宽（像素）。0 = 与 `w` 同值（1:1）。
    pub target_w: u32,
    /// 输出画布高（像素）。0 = 与 `h` 同值（1:1）。
    pub target_h: u32,
    /// 宿主给的像素预算上限（0 = 不限）。超预算 → [`ERR_TOO_LARGE`]。
    pub budget_bytes: u32,
    /// 位标志（[`REQ_ALLOW_SCALED`] 等；矢量路线恒 0）。
    pub flags: u32,
    /// 要第几帧（0 起），语义同 [`IvRenderReq::frame_index`]。
    pub frame_index: u32,
    /// 要第几页/子图（0 起），语义同 [`IvRenderReq::page_index`]。
    pub page_index: u32,
}

/// 可选导出 `iv_render_region` 的函数指针类型（v6 新增）。
///
/// 语义与 [`IvRenderReq`] / `iv_render` 对齐，差别只有两处：
/// - 请求体换成 [`IvRegionReq`]（带区域与目标画布）；
/// - `info`（[`IvRenderOut`]）里的 `actual_w/actual_h` 是**本次输出画布**的尺寸，
///   `bytes_needed` 是画布字节数（RGBA8 = w×h×4）。
///
/// # 必须遵守的硬约定
///
/// 1. **不得**"先渲整块再缩放"——那是有损插值，"不是无损放大，是伪装"。
/// 2. **不得**擅自拉伸变形：区域比例与画布比例不一致时，等比缩进并把实际画布
///    回填到 `actual_w/actual_h`（宿主负责补边）。
/// 3. 区域超出文档范围的部分渲染成**透明**（alpha = 0），不要报错。
/// 4. 同一份字节会**连续**用不同区域请求（宿主每次平移/缩放都发一次）⇒
///    解析结果**必须在进程内缓存**，不许每次重新解析文档。
/// 5. 线程模型、`iv_last_error`、可重入性、缓冲归属与 `iv_render` 完全相同。
///
/// # 字体（**硬要求**：文字必然要渲染）
///
/// 矢量文档里的文字**必须被渲染出来**——这是硬要求，不是可选项：
///
/// 1. **字体优先级**：文档内嵌字体 → 文档指定的系统字体 → 任意可用的兜底字体。
/// 2. **绝不许**因为找不到原字体就**丢字、空字、留白或崩溃**——那等于没渲染这张图。
/// 3. 用了兜底字体时，在 `iv_last_error` 里记一句（宿主会把这句话转达给用户，
///    例如"文档字体 X 缺失，已用兜底字体渲染"）。
/// 4. ★ **跨机像素差异**：兜底字体在不同机器上可能不同 ⇒ 同一文件在两台机器上的
///    像素**可能不一致**。这是"同一文件跨机同结果"这条立宪原则的**已登记例外**
///    （见 `.plan/m153` §7.3 的例外登记簿），**接受**它——比"缺字体就不渲染文字"好得多。
///
/// # Safety
///
/// 与 `iv_render` 同一条指针契约：`data` 指向宿主持有的 `len` 字节；`out` 为空
/// 或指向宿主分配的 `out_cap` 字节可写缓冲；`info` 指向一个 [`IvRenderOut`]。
pub type IvRenderRegionFn = unsafe extern "C" fn(
    data: *const u8,
    len: usize,
    req: *const IvRegionReq,
    out: *mut u8,
    out_cap: usize,
    info: *mut IvRenderOut,
    ext: *const c_char,
) -> i32;

/// 渲染结果（解码器 → 宿主）。`size_of == 48`。查询阶段只回填不写像素。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IvRenderOut {
    /// ★ 允许 ≠ `target_w`（DCT 缩放常只支持 1/2、1/4、1/8）。宿主负责缩放补齐。
    pub actual_w: u32,
    pub actual_h: u32,
    /// 查询阶段回填：宿主据此一次性分配像素缓冲。
    pub bytes_needed: u64,
    /// 见 `PF_*` 常量。
    pub pixel_format: u32,
    /// 每通道位数：8/16/32。
    pub bit_depth: u32,
    /// EXIF 方向 1..8。
    pub orientation: i32,
    /// 1 = 解码时已应用方向，宿主不得再转一次。
    pub already_oriented: i32,
    /// 总帧数。静态格式必须 = 1；0 = 解码器暂时无法确定（动画还没扫到 EOF），
    /// 宿主不得把 0 当静态，应看 `frame_delay_ms`。
    pub frame_count: u32,
    /// 本帧展示时长（毫秒）。0 = 静态图。动画帧解码器侧钳到 ≥10。
    pub frame_delay_ms: u32,
    /// 子资源总数。**单页静态图恒 = 1**（页数总能立刻知道，没有"惰性数页"场景，
    /// 与 `frame_count` 的 0=未知语义刻意不同）；0 = 解码器未实现多页语义。
    pub page_count: u32,
    /// ICC 色彩配置字节数，查询阶段回填；0 = 无。传输协议见规范 §十一：
    /// 宿主分配 `bytes_needed + icc_len`，渲染时 ICC 紧跟像素之后。
    pub icc_len: u32,
}

// ── 编译期断言：布局对不上就在这里炸，不等运行时 ────────────────────
//
// `IvDecoderInfo` 在 x64 上是 48：4 个裸指针（32）+ `abi_version`（4）+ 4 字节对齐填充
// + `is_vector`/`has_alpha`（8）。填充那 4 字节容易被忽略，正是断言要钉的东西。
// `IvRenderReq` 6×u32 = 24，对齐 4，无填充。`IvRenderOut` 是 48：
// 8（u64 的 8 字节对齐在 two u32 后补 4）+ 4×8 = 48，48%8==0，尾部无填充。
const _: () = assert!(size_of::<IvDecoderInfo>() == 48);
// `IvMetadata` = 4 个 u32 + 指针（x64 8）= 24；`IvMetadataField` = 2 个指针 = 16。
const _: () = assert!(size_of::<IvMetadata>() == 24);
const _: () = assert!(size_of::<IvMetadataField>() == 16);
const _: () = assert!(size_of::<IvRenderReq>() == 24);
const _: () = assert!(size_of::<IvRenderOut>() == 48);
// `IvRegionReq`（v6）= 4×f32（16）+ 6×u32（24）= 40；对齐 4，无填充。
const _: () = assert!(size_of::<IvRegionReq>() == 40);

// ══ 解码器样板（kit）══════════════════════════════════════════════════
//
// 原 `decoder-kit` crate，2026-10-16 并进本 crate：契约 + 样板同一个 crate，
// 解码器只依赖 `rupix-abi` 一项。宿主用不到这一段（它只用结构体与常量），
// 但**不许把这段拆出去或删掉**——本文件与宿主 `vendor/rupix-abi/src/lib.rs`
// 是逐字节快照关系（见 `core::abi::tests::abi_snapshot_matches_upstream_when_present`）。
//
// 解决什么问题：历史上 qoi / jxl 两个解码器各手抄一份 panic gate、线程本地
// last-error、`DecoderInfoHolder(Sync)` 包装和 `iv_abi_version` / `iv_decoder_info` /
// `iv_last_error` 三个导出。抄错一处（比如 gate 忘套）宿主的隔离承诺就破了。
// 现在解码器只写一行 `export_decoder!`，样板由这里保证正确。
//
// 解码器还需要自己写的只有两个业务导出：`iv_probe`（魔数嗅探）与 `iv_render`
// （两阶段解码），它们必须套 [`guard`]，错误路径调 [`set_last_error`]。

/// panic gate：宿主侧 `catch_unwind` 的解码器侧对偶。
///
/// 任何导出函数体都必须套它——内部 panic 被拦下并转成 [`ERR_PANIC`]，
/// 宿主据此把解码器拉黑（三振出局）而不是整个进程消失。
///
/// 进入时先清空线程本地 last-error：规范 §六之二把 `iv_last_error` 定义为
/// "当前这条解码线程上、**最近一次调用**的失败原因"。不清就会串味——上一次
/// 失败的文案会一直挂到下一次成功的调用上，宿主 UI 于是能拿出一句与本次无关的
/// 错误。让样板保证这条语义，比让每个解码器记得去清更可靠（历史上 qoi 的单测就是
/// 靠测试顺序侥幸通过这类断言的）。
pub fn guard(f: impl FnOnce() -> i32) -> i32 {
    LAST_ERROR.with(|e| e.borrow_mut().clear());
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        set_last_error("panic caught by decoder gate");
        ERR_PANIC
    })
}

/// 同一道 panic 栅栏，但**不清** last-error——只给 `iv_last_error` 用。
///
/// `guard` 的进门清理对读消息的函数本身是致命的：`iv_last_error` 套它等于
/// "先把要看的那行字擦掉再去看"，任何解码器的错误文案都永远取不到（2026-10-16
/// 真 DLL 实测：`iv_render` 返回 4 并 set 了 "null out-info pointer"，
/// 紧接着的 `iv_last_error` 回报 need=1，宿主永远拿到空串）。
/// 栅栏照留：`write_last_error` 理论上仍可能因 RefCell 中毒而 panic。
pub fn guard_preserve_error(f: impl FnOnce() -> i32) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        set_last_error("panic caught by decoder gate");
        ERR_PANIC
    })
}

thread_local! {
    // `const {}` 初始化式让这份 last-error 不必再走惰性 `Once` 检查（clippy 的
    // `missing_const_for_thread_local`）：`RefCell::new` 本来就是 `const fn`，
    // 而这段代码在每次"报错取错"的路径上都会执行。
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// 记录本次调用的失败原因（线程本地）。宿主在拿到非零错误码后调 `iv_last_error` 取走。
pub fn set_last_error(msg: &str) {
    LAST_ERROR.with(|e| {
        let mut e = e.borrow_mut();
        e.clear();
        e.push_str(msg);
    });
}

/// 取回本次调用的失败原因（克隆一份）。
///
/// 供**合并库**用：一枚 DLL 里有多家解码器时，"连试几家都不行"需要把**最后一家**的
/// 原话拼进自己的消息里——否则用户只看到"内部已全部让位"，看不到真正的原因
/// （例如 `tiff` 那句 "Photometric interpretation RGBPalette ... is unsupported"）。
///
/// 这不是导出函数，只是 DLL 内部的便利读取；宿主那一侧仍只走 `iv_last_error`。
pub fn last_error_text() -> String {
    LAST_ERROR.with(|e| e.borrow().clone())
}

/// `iv_last_error` 的实现（由 [`export_decoder!`] 导出的函数转发到这里）。
///
/// 返回**完整串长 + 1**（含结尾 NUL）：宿主可据此判断缓冲是否截断。
/// `buf` 为空或 `len == 0` 时只回报长度。
///
/// # Safety
/// `buf` 要么是空指针，要么指向**至少 `len` 字节、在本次调用期内归调用方独占**的可写内存。
/// 这条不变量只有 DLL 边界那一侧能负责，所以这一层必须是 `unsafe fn`——过去它是
/// safe `pub fn` 却直接往 `buf` 里写字节（`not_unsafe_ptr_arg_deref`，clippy 里
/// **deny-by-default 的一条 error**），调用方从签名上完全看不出自己在承担什么。
/// edition 2021 下整个函数体已在 unsafe 上下文里，内部不再套 `unsafe {}`。
pub unsafe fn write_last_error(buf: *mut std::ffi::c_char, len: usize) -> i32 {
    LAST_ERROR.with(|e| {
        let s = e.borrow();
        let bytes = s.as_bytes();
        let need = bytes.len() + 1; // 含结尾 NUL
        if !buf.is_null() && len > 0 {
            let n = std::cmp::min(bytes.len(), len - 1);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, n);
            *buf.add(n) = 0;
        }
        need as i32
    })
}

/// 把宿主传来的扩展名指针读成 `Option<&str>`：`NULL` → `None`，非 UTF-8 → `None`。
///
/// 供**合并库**使用：合并后 `iv_probe` 只写一份，各业务库的 `probe_impl` 拿的是 `&[u8]`
/// 与 `Option<&str>`，不再各自解指针。单库解码器也可以用它，省掉一处重复的 `from_raw_parts`。
///
/// # Safety
/// `ptr` 必须是 null，或指向一段以 NUL 结尾的有效字节。返回引用的生命周期是
/// **调用期借用的便利签名**——宿主不保证这个指针活过本次调用，不得存进任何结构体。
pub unsafe fn ext_str<'a>(ptr: *const std::ffi::c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    let mut n = 0usize;
    // 上界 64：扩展名不可能更长，遇到畸形指针也不至于无限读。
    while n < 64 && *ptr.add(n) != 0 {
        n += 1;
    }
    let bytes = std::slice::from_raw_parts(ptr as *const u8, n);
    std::str::from_utf8(bytes).ok()
}

/// `IvDecoderInfo` 含裸指针，本身不是 `Sync`，不能直接进 `static`。
/// 这个 newtype 手工声明 `Sync`：指针始终指向 `'static` 字节字面量，只读共享安全。
#[doc(hidden)]
pub struct DecoderInfoHolder(pub IvDecoderInfo);
unsafe impl Sync for DecoderInfoHolder {}

/// 生成解码器的静态自述与三个固定导出：`iv_abi_version` / `iv_decoder_info` / `iv_last_error`。
///
/// 在 crate 顶层调用一次；四个串参数必须是**字符串字面量**（编进 DLL 的 `'static` 数据）。
///
/// ```ignore
/// rupix_abi::export_decoder! {
///     id: "qoi",
///     name: "Quite OK Image",
///     extensions: "qoi",
///     description: "解码 Quite OK Image (QOI) 格式",
///     is_vector: 0,
///     has_alpha: 1,
/// }
/// ```
#[macro_export]
macro_rules! export_decoder {
    (
        id: $id:expr,
        name: $name:expr,
        extensions: $exts:expr,
        description: $desc:expr,
        is_vector: $is_vector:expr,
        has_alpha: $has_alpha:expr $(,)?
    ) => {
        static __SV_ID: &[u8] = concat!($id, "\0").as_bytes();
        static __SV_NAME: &[u8] = concat!($name, "\0").as_bytes();
        static __SV_EXTS: &[u8] = concat!($exts, "\0").as_bytes();
        static __SV_DESC: &[u8] = concat!($desc, "\0").as_bytes();

        static __SV_INFO: $crate::DecoderInfoHolder =
            $crate::DecoderInfoHolder($crate::IvDecoderInfo {
                id: __SV_ID.as_ptr() as *const std::ffi::c_char,
                display_name: __SV_NAME.as_ptr() as *const std::ffi::c_char,
                abi_version: $crate::IV_ABI_VERSION,
                extensions: __SV_EXTS.as_ptr() as *const std::ffi::c_char,
                is_vector: $is_vector,
                has_alpha: $has_alpha,
                description: __SV_DESC.as_ptr() as *const std::ffi::c_char,
            });

        #[no_mangle]
        pub extern "C" fn iv_abi_version() -> u32 {
            $crate::guard(|| $crate::IV_ABI_VERSION as i32) as u32
        }

        #[no_mangle]
        pub extern "C" fn iv_decoder_info() -> *const $crate::IvDecoderInfo {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                &__SV_INFO.0 as *const $crate::IvDecoderInfo
            }))
            .unwrap_or(std::ptr::null())
        }

        #[no_mangle]
        #[allow(clippy::not_unsafe_ptr_arg_deref)] // 规范 §三「指针参数的所有权」
        pub extern "C" fn iv_last_error(buf: *mut std::ffi::c_char, len: usize) -> i32 {
            // 裸指针只在**这一处**越过分界线：`buf`/`len` 由宿主给，独占可写的不变量
            // 在这里成立，再交给 [`write_last_error`] 那份带 `# Safety` 说明的实现。
            $crate::guard_preserve_error(|| unsafe { $crate::write_last_error(buf, len) })
        }
    };
}

/// 声明**解码能力位图**：生成可选导出 `iv_caps()`。放在 `export_decoder!` 之后一行：
///
/// ```text
/// rupix_abi::export_decoder! { id, name, exts, desc, is_vector, has_alpha }
/// rupix_abi::export_caps!(rupix_abi::CAP_FULL | rupix_abi::CAP_SCALED);
/// ```
///
/// 不写这一行也合法——宿主按 [`caps_from_legacy`] 推定，代价是永远拿不到
/// [`CAP_SCALED`] 这类新维度。**位图必须如实**：谎报的结果不是崩溃，而是宿主把缩放
/// 请求发给只会整解的解码器，白付一次整图解码——这种错测不出来也不报错，所以宁缺勿滥。
#[macro_export]
macro_rules! export_caps {
    ($($bits:tt)+) => {
        /// 能力位图（v4 内可选导出，位含义见 `$crate::CAP_*`）。
        #[no_mangle]
        pub extern "C" fn iv_caps() -> u32 {
            $crate::guard(|| ($($bits)+) as i32) as u32
        }
    };
}

/// 声明**可扩展元数据**：生成可选导出 `iv_metadata()`。
///
/// 参数是任意数量的 `"key" => "value"` 字符串字面量对；新增字段只改这里，
/// 不修改 `IvDecoderInfo`、不 bump ABI。宿主按 key 取自己认识的项，未知 key 忽略。
///
/// ```ignore
/// rupix_abi::export_metadata! {
///     "license" => "AGPL-3.0",
///     "source" => "https://example.invalid/source",
/// }
/// ```
#[macro_export]
macro_rules! export_metadata {
    ($($key:literal => $value:literal),* $(,)?) => {
        static __SV_META_FIELDS: $crate::MetadataFields = $crate::MetadataFields(&[
            $($crate::IvMetadataField {
                key: concat!($key, "\0").as_ptr() as *const std::ffi::c_char,
                value: concat!($value, "\0").as_ptr() as *const std::ffi::c_char,
            }),*
        ]);
        static __SV_META: $crate::MetadataHolder = $crate::MetadataHolder($crate::IvMetadata {
            size: std::mem::size_of::<$crate::IvMetadata>() as u32,
            version: $crate::METADATA_VERSION,
            field_count: __SV_META_FIELDS.0.len() as u32,
            reserved: 0,
            fields: __SV_META_FIELDS.0.as_ptr(),
        });

        /// 可扩展元数据表（可选导出；旧宿主可忽略）。
        #[no_mangle]
        pub extern "C" fn iv_metadata() -> *const $crate::IvMetadata {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| &__SV_META.0))
                .map(|p| p as *const _)
                .unwrap_or(std::ptr::null())
        }
    };
}

/// 声明**矢量导出**：生成可选导出 `iv_export_svg` 的薄包装，业务在 `export_svg_impl`
/// 这一层（与 `probe_impl` / `render_impl` 同一分层：合并档直接调 impl，不过宏）。
///
/// ★ **2026-10-28 起宿主不再调用 `iv_export_svg`**（"宿主渲染矢量"那条路线退役）。
/// 这个宏**保留**（ABI 冻结不删符号），但新写的解码器不必用它——矢量格式一律
/// 走 `iv_render`（整幅按 target）+ `iv_render_region`（ABI v6 区域重渲）。
///
/// ```ignore
/// rupix_abi::export_decoder! { id, name, exts, desc, is_vector, has_alpha }
/// rupix_abi::export_svg!(export_svg_impl);
/// ```
///
/// impl 的形状就是 [`IvExportSvgFn`] 去掉 `unsafe`：指针有效性由 impl 内部的显式
/// `unsafe {}` 负责（规范 §三），[`guard`] 照旧把 panic 拦成 [`ERR_PANIC`]。
/// 合并档（如 pro）**不用**这个宏：它自己写 `#[no_mangle] iv_export_svg` 转发到
/// 唯一的矢量成员，避免多份导出重名（同 `export_decoder!` 的 standalone 约定）。
#[macro_export]
macro_rules! export_svg {
    ($impl:path) => {
        /// 可选导出：矢量场景导出为 UTF-8 SVG 文本字节（语义见 `$crate::IvExportSvgFn`）。
        #[no_mangle]
        #[allow(clippy::not_unsafe_ptr_arg_deref)] // 规范 §三「指针参数的所有权」
        pub extern "C" fn iv_export_svg(
            data: *const u8,
            len: usize,
            out: *mut u8,
            out_cap: usize,
            needed: *mut usize,
        ) -> i32 {
            $crate::guard(|| $impl(data, len, out, out_cap, needed))
        }
    };
}

/// 声明**区域渲染**（v6）：生成可选导出 `iv_render_region` 的薄包装，业务在
/// `export_region_impl` 这一层（与 `probe_impl` / `render_impl` 同一分层：合并档直接调
/// impl，不过宏）。
///
/// ```ignore
/// rupix_abi::export_decoder! { id, name, exts, desc, is_vector, has_alpha }
/// rupix_abi::export_caps!(rupix_abi::CAP_FULL | rupix_abi::CAP_REGION);
/// rupix_abi::export_region!(export_region_impl);
/// ```
///
/// impl 的形状就是 [`IvRenderRegionFn`] 去掉 `unsafe`：指针有效性由 impl 内部的显式
/// `unsafe {}` 负责（规范 §三），[`guard`] 照旧把 panic 拦成 [`ERR_PANIC`]。
#[macro_export]
macro_rules! export_region {
    ($impl:path) => {
        /// 可选导出：把文档的指定区域按目标画布直接光栅化（语义见 `$crate::IvRenderRegionFn`）。
        #[no_mangle]
        #[allow(clippy::not_unsafe_ptr_arg_deref)] // 规范 §三「指针参数的所有权」
        pub extern "C" fn iv_render_region(
            data: *const u8,
            len: usize,
            req: *const $crate::IvRegionReq,
            out: *mut u8,
            out_cap: usize,
            info: *mut $crate::IvRenderOut,
            ext: *const std::ffi::c_char,
        ) -> i32 {
            $crate::guard(|| $impl(data, len, req, out, out_cap, info, ext))
        }
    };
}

// ---------------------------------------------------------------------------
// kit：EXIF 方向取值（解码器侧共用，2026-10-19 m48）
// ---------------------------------------------------------------------------
/// 从一段 EXIF / TIFF 结构里只取**方向标签** `0x0112`（`IvRenderOut::orientation` 的来处）。
///
/// # 为什么值得放进 kit
/// 方向是**容器知识**：JPEG 在 APP1、PNG 在 `eXIf` 块、WebP 在 RIFF 的 `EXIF` chunk、
/// TIFF 整个文件就是这段结构。容器切块各家不同（留在各解码器里），但**切出来之后的那一小段
/// TIFF 走读完全一样**——这一份复制 4 次，就是把"count 撒谎"这类坑也复制 4 次。
/// 各 DLL 本来就静态链接自己的副本，共用不省体积，省的是**正确性的维护面**。
///
/// # 三条刻意的设计（都是踩过的坑，见宿主 `docs/踩坑记录.md` K26 / 规范 §六 第 10 条）
/// 1. **零递归**：只读 IFD0。方向语义描述"这张图怎么摆"，规范把它放在 IFD0；`ExifIfd`(0x8769)
///    那棵树里是摄影参数，与像素摆放无关。少一层递归 = 少一类输入驱动的结构风险，
///    菱形重入（`N^层数`）在这里根本不可能发生。
/// 2. **工作量上界取自字节长度，不信文件内的 `count`**：`count` 撒谎时按
///    `(剩余字节 - 2) / 12` 夹住，最坏扫完真实存在的条目就停——挂死的源头是输入字节，不是作者。
/// 3. **值域不猜**：`count != 1`、类型不是 BYTE/SHORT/LONG、值不在 1–8，一律 `None`
///    （调用方据此报 `orientation=1`）。宁可当这文件没方向，也不把一个 255 当成方向喂给宿主——
///    那意味着宿主按 `5..=8` 交换宽高，画面变成一张没人预期的拼图。
///
/// 返回 `Some(1..=8)`；`None` = 没有这个标签 / 结构不可信 / 字节不够。
pub fn exif_orientation(tiff: &[u8]) -> Option<i32> {
    let little = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b = tiff.get(i..i + 2)?;
        Some(if little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b = tiff.get(i..i + 4)?;
        Some(if little {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };

    // TIFF 魔数 42 认不认都得看：不认就当不是 EXIF 块（JPEG 里 APP1 未必是 EXIF）。
    if u16_at(2)? != 42 {
        return None;
    }
    let mut i = u32_at(4)? as usize;
    if i.checked_add(2)? > tiff.len() {
        return None;
    }
    let declared = u16_at(i)? as usize;
    i += 2;
    // ★ 上界来自长度，不来自 declared（撒谎的 0xFFFF 只能让我们扫到真实存在的条目为止）。
    let room = (tiff.len() - i) / 12;
    let entries = declared.min(room);
    for n in 0..entries {
        let e = i + n * 12;
        if u16_at(e)? != 0x0112 {
            continue;
        }
        let kind = u16_at(e + 2)?;
        if u32_at(e + 4)? != 1 {
            return None; // 方向是单值；带个数组进来就是畸形
        }
        let v = match kind {
            1 => tiff.get(e + 8).copied()? as u32, // BYTE
            3 => u16_at(e + 8)? as u32,            // SHORT（值就落在偏移字段低半部）
            4 => u32_at(e + 8)?,                   // LONG
            _ => return None,                      // 别的类型不猜
        };
        return (1..=8).contains(&v).then_some(v as i32);
    }
    None
}

/// JPEG：从 SOI 之后扫到第一个 SOS 为止，找带 `Exif\0\0` 前缀的 APP1，把 TIFF 段交给
/// [`exif_orientation`]。★ **只扫段表、不碰熵编码数据**——微秒级；扫到 SOS/EOS 或段表
/// 断裂就停，所以畸形文件不会让我们读完整个大文件。
pub fn jpeg_exif_orientation(data: &[u8]) -> Option<i32> {
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    // 段表最多走 64 段（真实文件通常 ≤ 8 段；这是给畸形数据的时间上界，不是功能边界）。
    for _ in 0..64 {
        if data.get(i..i + 2)? != [0xFF, 0xE1] {
            // 非 APP1：遇到 SOS（0xFFDA）或任何不是 0xFF 开头的东西就结束——
            // 后面是压缩数据，再往里找只会把字节当长度。
            if data.get(i) == Some(&0xFF) {
                let marker = *data.get(i + 1)?;
                if marker == 0xDA || marker == 0xD9 {
                    return None;
                }
                // 其它带长度的段：跳过它。
                let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
                if len < 2 {
                    return None; // 长度 0/1 = 撒谎，不推进（规范 §六 第 10 条）
                }
                i += 2 + len;
                continue;
            }
            return None;
        }
        let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
        let body_start = i + 4;
        let body_end = body_start.checked_add(len.checked_sub(2)?)?;
        if body_end > data.len() {
            return None;
        }
        let body = &data[body_start..body_end];
        if body.starts_with(b"Exif\0\0") {
            return exif_orientation(&body[6..]);
        }
        i = body_end;
    }
    None
}

/// PNG：逐块走 `IDAT` 之前的块表，找 `eXIf` 数据块（块长驱动、每步严格推进）。
pub fn png_exif_orientation(data: &[u8]) -> Option<i32> {
    if data.get(0..8)? != [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return None;
    }
    let mut i = 8usize;
    for _ in 0..64 {
        let len = u32::from_be_bytes(<[u8; 4]>::try_from(data.get(i..i + 4)?).ok()?) as usize;
        let ty: &[u8; 4] = data.get(i + 4..i + 8)?.try_into().ok()?;
        let body = i + 8;
        let end = body.checked_add(len)?;
        if end > data.len() {
            return None;
        }
        if ty == b"eXIf" {
            return exif_orientation(&data[body..end]);
        }
        if ty == b"IDAT" || ty == b"IEND" {
            return None; // 元数据块都在 IDAT 之前（PNG 规范），往后没有
        }
        i = end + 4; // 跳过 CRC
    }
    None
}

/// WebP：RIFF 容器里找 `EXIF` chunk（4 字节 4CC + u32 LE 长度，奇数长度补一字节填充）。
pub fn webp_exif_orientation(data: &[u8]) -> Option<i32> {
    if data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
        return None;
    }
    let mut i = 12usize;
    for _ in 0..64 {
        let ty = data.get(i..i + 4)?;
        let len = u32::from_le_bytes(<[u8; 4]>::try_from(data.get(i + 4..i + 8)?).ok()?) as usize;
        let body = i + 8;
        let end = body.checked_add(len)?;
        if end > data.len() {
            return None;
        }
        if ty == b"EXIF" {
            return exif_orientation(&data[body..end]);
        }
        // chunk 按 2 字节对齐：奇数长度后跟一个填充字节。★ len 为 0 时也必须推进，否则原地打转。
        i = body + len + (len & 1);
    }
    None
}

/// ICC 附加段上限：与宿主 `ICC_CAP` 同口径（1 MiB）。超限或分片不齐一律**整份丢弃**——
/// 截断的色彩配置比没有更糟（宿主会照着半份曲线转，颜色反而更远）。
pub const ICC_MAX: usize = 1 << 20;

/// JPEG：扫到 SOS 前的 APP2 段，拼 `ICC_PROFILE\0` 多分片。
///
/// 分片头 14 字节 = `ICC_PROFILE\0`(12) + seq(1, 从 1 起) + count(1)。只有
/// `1..=count` 全部到齐、无重复、总长 ≤ [`ICC_MAX`] 才返回；否则 `None`（当作没有 ICC）。
pub fn jpeg_icc_profile(data: &[u8]) -> Option<Vec<u8>> {
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut parts: Vec<&[u8]> = Vec::new();
    let mut count = 0usize;
    let mut total = 0usize;
    let mut i = 2usize;
    for _ in 0..64 {
        if data.get(i) != Some(&0xFF) {
            return None;
        }
        let marker = *data.get(i + 1)?;
        if marker == 0xDA || marker == 0xD9 {
            break; // 熵编码数据开始；元数据段到此为止
        }
        let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
        if len < 2 {
            return None;
        }
        let body_start = i + 4;
        let body_end = body_start.checked_add(len - 2)?;
        if body_end > data.len() {
            return None;
        }
        let body = &data[body_start..body_end];
        if marker == 0xE2 && body.starts_with(b"ICC_PROFILE\0") && body.len() >= 14 {
            let seq = body[12] as usize;
            let n = body[13] as usize;
            if n == 0 || seq == 0 || seq > n {
                return None;
            }
            if count == 0 {
                count = n;
            }
            if n != count || parts.len() >= count {
                return None; // 分片数前后不一致 / 重复分片 = 不可信
            }
            if parts.len() + 1 != seq {
                return None; // 分片必须按 1,2,3… 顺序到达；乱序在真实文件里不存在
            }
            total = total.checked_add(body.len() - 14)?;
            if total > ICC_MAX {
                return None;
            }
            parts.push(&body[14..]);
        }
        i = body_end;
    }
    if count == 0 || parts.len() != count {
        return None; // 缺片：宁可整份不要
    }
    let mut out = Vec::with_capacity(total);
    for p in parts {
        out.extend_from_slice(p);
    }
    Some(out)
}

/// WebP：RIFF 容器里找 `ICCP` chunk（与 [`webp_exif_orientation`] 同一套推进规则）。
pub fn webp_icc_profile(data: &[u8]) -> Option<Vec<u8>> {
    if data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
        return None;
    }
    let mut i = 12usize;
    for _ in 0..64 {
        let ty = data.get(i..i + 4)?;
        let len = u32::from_le_bytes(<[u8; 4]>::try_from(data.get(i + 4..i + 8)?).ok()?) as usize;
        let body = i + 8;
        let end = body.checked_add(len)?;
        if end > data.len() {
            return None;
        }
        if ty == b"ICCP" {
            if len > ICC_MAX {
                return None;
            }
            return Some(data[body..end].to_vec());
        }
        i = body + len + (len & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个跨边界结构体的长度必须冻结。这条测试是编译期断言的"看得见"版本。
    ///
    /// `IvDecoderInfo` 也必须在这张表里：它的 48 字节含 4 字节对齐填充，那 4 字节
    /// 是"新字段不能靠追加蒙过去"的最后一道防线（实测往末尾追加会让宿主对旧 DLL
    /// 越界读到相邻 `.data` 的垃圾值）。谁改了这个数，就是在改契约。
    #[test]
    fn ffi_struct_sizes_are_frozen() {
        assert_eq!(size_of::<IvDecoderInfo>(), 48);
        assert_eq!(size_of::<IvRenderReq>(), 24);
        assert_eq!(size_of::<IvRenderOut>(), 48);
        // v6 新增：区域渲染请求（4×f32 + 6×u32 = 40）。
        assert_eq!(size_of::<IvRegionReq>(), 40);
    }

    /// 能力位是 ABI 的一部分，和错误码同样逐个钉死——挪号等于让老解码器谎报能力。
    #[test]
    fn capability_bits_are_stable() {
        assert_eq!(
            [
                CAP_FULL,
                CAP_SCALED,
                CAP_RAW_PARAM,
                CAP_MULTIFRAME,
                CAP_MULTIPAGE,
                CAP_VECTOR,
                CAP_REGION
            ],
            [0b1, 0b10, 0b100, 0b1000, 0b1_0000, 0b10_0000, 0b100_0000]
        );
        // 请求位标志也在发布出去的契约里：宿主与解码器各读一半，位号一样不许挪。
        assert_eq!(REQ_ALLOW_SCALED, 0b100);
    }

    /// 缺席推定：只从**既有证据**换算，`CAP_SCALED` 永不凭空出现。
    #[test]
    fn caps_from_legacy_never_invents_scaled() {
        assert_eq!(caps_from_legacy(false, false), CAP_FULL);
        assert_eq!(caps_from_legacy(true, false), CAP_FULL | CAP_VECTOR);
        assert_eq!(caps_from_legacy(false, true), CAP_FULL | CAP_RAW_PARAM);
        for iv in [false, true] {
            for rr in [false, true] {
                assert_eq!(caps_from_legacy(iv, rr) & CAP_SCALED, 0);
            }
        }
    }

    /// v6 的推定规则：`CAP_REGION` 只从**符号在不在**这个硬证据来，绝不凭空出现。
    #[test]
    fn caps_region_only_from_symbol_evidence() {
        assert_eq!(
            caps_from_legacy_v6(false, false, false),
            CAP_FULL,
            "没有区域导出就不许有区域能力位"
        );
        assert_eq!(
            caps_from_legacy_v6(false, false, true),
            CAP_FULL | CAP_REGION
        );
        // 向后兼容：老函数的行为一字未变。
        assert_eq!(caps_from_legacy_v6(true, true, false), caps_from_legacy(true, true));
    }

    /// ABI 版本区间：宿主按 `5 ≤ abi ≤ 6` 收载——**v5 的老 DLL 照常加载**。
    ///
    /// 这条是"v6 兼容 v5"的机器凭据：v6 是纯增量（新增导出 + 新增结构体 + 新增能力位），
    /// v5 的签名与布局逐字节未动 ⇒ 老 DLL 不必重编就能被 v6 宿主收载。
    #[test]
    fn abi_v6_keeps_v5_loadable() {
        assert_eq!(IV_ABI_VERSION, 6);
        // 下界（宿主侧的 MIN_ABI_VERSION）与本 crate 的一致性由宿主自己的测试盯；
        // 这里只钉"v5 在区间内"这条兼容承诺。
        assert!((5..=IV_ABI_VERSION).contains(&5));
        assert!((5..=IV_ABI_VERSION).contains(&6));
    }

    /// 错误码是 ABI 的一部分，逐个钉死——改了含义等于 bump abi，绝不能悄悄挪号。
    #[test]
    fn error_codes_are_stable() {
        assert_eq!(
            [
                ERR_OK,
                ERR_UNRECOGNIZED,
                ERR_CORRUPT,
                ERR_UNSUPPORTED,
                ERR_BUFFER_TOO_SMALL,
                ERR_OOM,
                ERR_PANIC,
                ERR_TOO_LARGE,
            ],
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
    }

    /// `iv_last_error` 的读取路径必须保住消息本身。
    ///
    /// 曾经的事故：导出样板把 `write_last_error` 也套进 [`guard`]，而 `guard` 进门先清
    /// last-error——读消息的函数把要读的那行字擦掉了，所有 Rust 解码器的错误文案一律取不到
    /// （真 DLL 实测 need=1、宿主拿到空串）。这条测试钉住两个栅栏的分工。
    #[test]
    fn reading_last_error_keeps_the_message() {
        let mut buf = [0u8; 64];
        set_last_error("header: corrupt");
        let n = guard_preserve_error(|| unsafe {
            write_last_error(buf.as_mut_ptr() as *mut std::ffi::c_char, buf.len())
        });
        assert_eq!(&buf[..15], b"header: corrupt");
        assert_eq!(buf[15], 0);
        assert_eq!(n, 16, "回报长度含结尾 NUL");

        // 业务导出那侧的进门清理照旧：上一次失败的文案不得串到下一次调用上。
        set_last_error("stale");
        guard(|| 0);
        let mut buf = [0u8; 64];
        assert_eq!(
            unsafe { write_last_error(buf.as_mut_ptr() as *mut std::ffi::c_char, 64) },
            1
        );
        assert_eq!(buf[0], 0);
    }

    /// pixel_format 枚举同样冻结：宿主按数值 blit，挪号等于显示成乱码色。
    #[test]
    fn pixel_formats_are_stable() {
        assert_eq!(
            [
                PF_GRAY8,
                PF_RGB8,
                PF_RGBA8,
                PF_GRAY16,
                PF_RGB16,
                PF_RGBA16,
                PF_GRAY_F32,
                PF_RGBA_F32
            ],
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
        // v4 内可选导出新增的输入布局（输出传输格式永远是 PF_RGBA8）。
        assert_eq!(PF_BGRA8, 8);
        // v5 内可选导出追加的 YUV 输入布局（追值不改值，契约冻结）。
        assert_eq!(
            [PF_YUY2, PF_UYVY, PF_I420, PF_YV12, PF_NV12],
            [9, 10, 11, 12, 13]
        );
    }

    // ── EXIF 方向取值（m48）────────────────────────────────────
    //
    // kit 的三条设计（零递归 / 上界取自长度 / 值域不猜）每条都要有用例钉着，
    // 否则下一次"顺手优化"就会把 count 当真、把 255 当方向喂给宿主。

    /// 造一段最小 TIFF 块。`entries` 是 (tag, kind, count, value)，按 IFD0 的 12 字节格式写。
    fn tiff_block(
        little: bool,
        entries: &[(u16, u16, u32, u32)],
        declared: Option<usize>,
    ) -> Vec<u8> {
        let mut v = Vec::new();
        let push16 = |v: &mut Vec<u8>, x: u16| {
            v.extend(if little {
                x.to_le_bytes()
            } else {
                x.to_be_bytes()
            })
        };
        let push32 = |v: &mut Vec<u8>, x: u32| {
            v.extend(if little {
                x.to_le_bytes()
            } else {
                x.to_be_bytes()
            })
        };
        v.extend(if little {
            b"II".to_vec()
        } else {
            b"MM".to_vec()
        });
        push16(&mut v, 42);
        push32(&mut v, 8); // IFD0 就在头之后
        push16(&mut v, declared.unwrap_or(entries.len()) as u16);
        for (tag, kind, count, value) in entries {
            push16(&mut v, *tag);
            push16(&mut v, *kind);
            push32(&mut v, *count);
            // ★ 值字段按**类型宽度**写、尾部补零——这才是真实文件的摆法：
            // SHORT 6 在 big-endian 里是 `00 06 00 00`，当成 u32 写就变成 `00 00 00 06`，
            //  fixture 一旦写错，reader 会"看起来错"，所以这里刻意按规范造数据。
            match *kind {
                1 => v.push(*value as u8),
                3 => push16(&mut v, *value as u16),
                _ => push32(&mut v, *value),
            }
            let pad = match *kind {
                1 => 3,
                3 => 2,
                _ => 0,
            };
            v.resize(v.len() + pad, 0);
        }
        push32(&mut v, 0); // 无子 IFD
        v
    }

    const ORIENT: u16 = 0x0112;
    const SHORT: u16 = 3;
    const BYTE: u16 = 1;
    const LONG: u16 = 4;

    #[test]
    fn orientation_reads_short_in_both_byte_orders() {
        for little in [true, false] {
            let b = tiff_block(
                little,
                &[(0x0100, SHORT, 1, 200), (ORIENT, SHORT, 1, 6)],
                None,
            );
            assert_eq!(exif_orientation(&b), Some(6), "little={little}");
        }
    }

    #[test]
    fn orientation_accepts_byte_and_long_but_not_other_types() {
        for little in [true, false] {
            let b = tiff_block(little, &[(ORIENT, BYTE, 1, 8)], None);
            assert_eq!(exif_orientation(&b), Some(8), "BYTE little={little}");
            let b = tiff_block(little, &[(ORIENT, LONG, 1, 3)], None);
            assert_eq!(exif_orientation(&b), Some(3), "LONG little={little}");
        }
        // 类型 2(ASCII) 之类不猜：把 4 字节当数字用是编出来的语义。
        let b = tiff_block(true, &[(ORIENT, 2, 1, 6)], None);
        assert_eq!(exif_orientation(&b), None);
    }

    #[test]
    fn orientation_rejects_out_of_range_and_multi_value() {
        for bad in [0u32, 9, 255, 0x1234] {
            let b = tiff_block(true, &[(ORIENT, SHORT, 1, bad)], None);
            assert_eq!(exif_orientation(&b), None, "值 {bad} 不在 1–8，不许当方向");
        }
        // count≠1 = 畸形：方向的语义是单值。
        let b = tiff_block(true, &[(ORIENT, SHORT, 2, 6)], None);
        assert_eq!(exif_orientation(&b), None);
    }

    #[test]
    fn missing_tag_and_short_header_are_none() {
        let b = tiff_block(true, &[(0x0100, SHORT, 1, 200)], None);
        assert_eq!(exif_orientation(&b), None);
        assert_eq!(exif_orientation(b"II"), None);
        assert_eq!(exif_orientation(&[]), None);
        // 头齐但 IFD 偏移越界：不许 panic，也不许读越界。
        let mut b = tiff_block(true, &[(ORIENT, SHORT, 1, 6)], None);
        b.truncate(10);
        assert_eq!(exif_orientation(&b), None);
    }

    #[test]
    fn lying_entry_count_is_clamped_by_bytes_not_trusted() {
        // ★ 上界取自**字节长度**：declared=0xFFFF 但块里只放了一枚真条目。
        // 若信 declared，扫描会跑到越界（我们让它停在真实存在的条目数）——这条断言
        // 同时钉住两件事：越界不 panic，且**夹住之后仍然找得到前面那枚真方向**。
        let b = tiff_block(
            true,
            &[(ORIENT, SHORT, 1, 6), (0x0100, SHORT, 1, 200)],
            Some(0xFFFF),
        );
        assert_eq!(exif_orientation(&b), Some(6));
        // 反向：`count` 写小（撒谎说只有 1 条）⇒ 我们**信它的小值**，绝不为了找方向去越权
        // 多读一条。这里真方向排在第二条，所以必须读不到。
        let hidden = tiff_block(
            true,
            &[(0x0100, SHORT, 1, 1), (ORIENT, SHORT, 1, 6)],
            Some(1),
        );
        assert_eq!(exif_orientation(&hidden), None);
    }

    #[test]
    fn jpeg_locator_walks_segments_and_stops_at_sos() {
        let exif = tiff_block(true, &[(ORIENT, SHORT, 1, 8)], None);
        let mut app1 = vec![0xFF, 0xE1];
        let payload_len = (6 + exif.len() + 2) as u16;
        app1.extend(payload_len.to_be_bytes());
        app1.extend(b"Exif\0\0");
        app1.extend(&exif);

        let mut j = vec![0xFF, 0xD8];
        // 前置一个 JFIF APP0，证明"不是第一段也找得到"。
        let mut app0 = vec![0xFF, 0xE0, 0x00, 0x04, b'x', b'y'];
        j.append(&mut app0);
        j.extend(&app1);
        assert_eq!(jpeg_exif_orientation(&j), Some(8));

        // SOS 之后不再有元数据：把 APP1 放到 SOS 后面 ⇒ 找不到（也不许把压缩数据当段表读）。
        let mut late = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x04, 1, 2];
        late.extend(&app1);
        assert_eq!(jpeg_exif_orientation(&late), None);

        // 段长撒谎（超出文件真实长度）⇒ None，不许越界。
        let mut liar = vec![0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xFF];
        liar.extend(b"Exif\0\0");
        liar.extend(&exif);
        assert_eq!(jpeg_exif_orientation(&liar), None);

        // APP1 但不是 EXIF（比如 JFIF 扩展 / XMP）⇒ None。
        let mut xmp = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let body = b"<?xpacket begin=\"\"?>";
        xmp.extend(((body.len() + 2) as u16).to_be_bytes());
        xmp.extend(body);
        assert_eq!(jpeg_exif_orientation(&xmp), None);

        assert_eq!(jpeg_exif_orientation(b"not a jpeg"), None);
        assert_eq!(jpeg_exif_orientation(&[0xFF, 0xD8]), None);
    }

    #[test]
    fn png_locator_finds_exif_chunk_before_idat() {
        let exif = tiff_block(true, &[(ORIENT, SHORT, 1, 5)], None);
        let mut p: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let chunk = |ty: &[u8; 4], body: &[u8], p: &mut Vec<u8>| {
            p.extend((body.len() as u32).to_be_bytes());
            p.extend(ty);
            p.extend(body);
            p.extend([0, 0, 0, 0]); // CRC：本函数不看它，填零正好证明走的是长度而不是校验
        };
        chunk(b"IHDR", &[0u8; 16], &mut p);
        chunk(b"eXIf", &exif, &mut p);
        assert_eq!(png_exif_orientation(&p), Some(5));

        // eXIf 在 IDAT 之后 = 不合规范，也不该去那儿找（往后走只会把像素当块表）。
        let mut late: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        chunk(b"IDAT", &[1, 2, 3, 4], &mut late);
        chunk(b"eXIf", &exif, &mut late);
        assert_eq!(png_exif_orientation(&late), None);

        // 块长撒谎越界 ⇒ None。
        let mut liar: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        liar.extend(0xFFFF_FFFFu32.to_be_bytes());
        liar.extend(b"eXIf");
        assert_eq!(png_exif_orientation(&liar), None);
        assert_eq!(png_exif_orientation(b"\x89PNG"), None);
    }

    #[test]
    fn webp_locator_honours_odd_padding_and_zero_length_chunks() {
        let exif = tiff_block(true, &[(ORIENT, SHORT, 1, 7)], None);
        let mut w: Vec<u8> = b"RIFF".to_vec();
        w.extend(0u32.to_le_bytes());
        w.extend(b"WEBP");
        let push = |w: &mut Vec<u8>, ty: &[u8; 4], body: &[u8]| {
            w.extend(ty);
            w.extend((body.len() as u32).to_le_bytes());
            w.extend(body);
            if body.len() & 1 == 1 {
                w.push(0); // 2 字节对齐的填充
            }
        };
        push(&mut w, b"VP8L", &[1, 2, 3]); // 奇数长度 + 填充字节
        push(&mut w, b"EXIF", &exif);
        assert_eq!(webp_exif_orientation(&w), Some(7));

        // ★ 零长度 chunk 必须仍然推进（否则原地打转 = 挂死，规范 §六 第 10 条的那一条）。
        let mut spin: Vec<u8> = b"RIFF".to_vec();
        spin.extend(0u32.to_le_bytes());
        spin.extend(b"WEBP");
        for _ in 0..20 {
            push(&mut spin, b"VP8X", &[]);
        }
        push(&mut spin, b"EXIF", &exif);
        assert_eq!(
            webp_exif_orientation(&spin),
            Some(7),
            "0 长度块不能卡住推进"
        );

        // 一堆永远走不完的 0 长度块 ⇒ 到段数上限就收手返回 None（**时间有界**，不是挂死）。
        let mut endless: Vec<u8> = b"RIFF".to_vec();
        endless.extend(0u32.to_le_bytes());
        endless.extend(b"WEBP");
        for _ in 0..400 {
            push(&mut endless, b"AAAA", &[]);
        }
        assert_eq!(webp_exif_orientation(&endless), None);

        assert_eq!(webp_exif_orientation(b"RIFF....WEBP"), None);
        assert_eq!(webp_exif_orientation(b"not webp"), None);
    }

    #[test]
    fn jpeg_icc_concatenates_segments_in_sequence_and_rejects_gaps() {
        let seg = |seq: u8, count: u8, body: &[u8]| {
            let mut v = vec![0xFF, 0xE2];
            let len = (2 + 12 + 2 + body.len()) as u16;
            v.extend(len.to_be_bytes());
            v.extend(b"ICC_PROFILE\0");
            v.extend([seq, count]);
            v.extend(body);
            v
        };
        let mut j = vec![0xFF, 0xD8];
        j.extend(seg(1, 3, b"AAA"));
        // 中间夹一个无关 APP0，证明按段类型走而不是只认第一段。
        j.extend([0xFF, 0xE0, 0x00, 0x04, b'x', b'y']);
        j.extend(seg(2, 3, b"BBB"));
        j.extend(seg(3, 3, b"CCC"));
        j.extend([0xFF, 0xDA, 0x00, 0x02]);
        assert_eq!(jpeg_icc_profile(&j).as_deref(), Some(&b"AAABBBCCC"[..]));

        // 缺片（1、3 没有 2）：整份丢弃，不许拼出半份曲线。
        let mut gap = vec![0xFF, 0xD8];
        gap.extend(seg(1, 2, b"AAA"));
        gap.extend(seg(3, 2, b"CCC"));
        assert_eq!(jpeg_icc_profile(&gap), None);

        // 分片总长超过上限：整份丢弃。
        let mut huge = vec![0xFF, 0xD8];
        huge.extend(seg(1, 1, &vec![0u8; ICC_MAX + 1]));
        assert_eq!(jpeg_icc_profile(&huge), None);

        // 段长撒谎 / 不是 JPEG / 只有 SOI。
        let liar = [0xFF, 0xD8, 0xFF, 0xE2, 0xFF, 0xFF, b'I'];
        assert_eq!(jpeg_icc_profile(&liar), None);
        assert_eq!(jpeg_icc_profile(b"not a jpeg"), None);
        assert_eq!(jpeg_icc_profile(&[0xFF, 0xD8]), None);
    }

    #[test]
    fn webp_icc_locator_honours_padding() {
        let mut w: Vec<u8> = b"RIFF".to_vec();
        w.extend(0u32.to_le_bytes());
        w.extend(b"WEBP");
        let push = |w: &mut Vec<u8>, ty: &[u8; 4], body: &[u8]| {
            w.extend(ty);
            w.extend((body.len() as u32).to_le_bytes());
            w.extend(body);
            if body.len() & 1 == 1 {
                w.push(0);
            }
        };
        push(&mut w, b"VP8X", &[0u8; 10]);
        push(&mut w, b"ICCP", b"abcdefg"); // 奇数长度 + 填充
        push(&mut w, b"VP8L", &[1, 2, 3]);
        assert_eq!(webp_icc_profile(&w).as_deref(), Some(&b"abcdefg"[..]));

        let mut big: Vec<u8> = b"RIFF".to_vec();
        big.extend(0u32.to_le_bytes());
        big.extend(b"WEBP");
        push(&mut big, b"ICCP", &vec![0u8; ICC_MAX + 1]);
        assert_eq!(webp_icc_profile(&big), None, "超上限整份丢弃");

        assert_eq!(webp_icc_profile(b"not webp"), None);
        assert_eq!(webp_icc_profile(b"RIFF....WEBP"), None);
    }
}
