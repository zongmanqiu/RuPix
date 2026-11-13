//! 格式认领的**依赖倒置**口（CurSeen 手法二）。
//!
//! "目录里哪些文件算图片"、"哪些扩展名要走裸数据参数窗"是**清单数据**，不是解码器行为；
//! 但清单的产生方是解码器层。若 gallery/viewer/decode 直接 `use decoders`，core 就永远
//! 泄漏到解码器实现——所以这里把**查询接口**放在 core，把**实现**（扫目录 + 读解码器自述）
//! 由解码器层在启动时装入（[`install`]）。core 侧只见 trait，永不见 DLL。
//!
//! 未装入时一律返回空集：启动顺序错位只表现为"清单暂空"（不 panic、不崩），而装入点
//! 有三重保险——`runtime::run()`（GUI 入口）、`crate::decoders::Host::new()`（解码线程）、
//! `crate::decoders::declared_exts()`（示例/探针的直接调用）。

use std::collections::HashSet;
use std::sync::OnceLock;

/// 解码器层对「已认领扩展名」的查询实现（由 `decoders` 层提供并装入）。
pub trait FormatSource: Send + Sync {
    /// 已加载解码器自述认领的扩展名并集（小写、无点）。
    ///
    /// 每次调用允许返回重扫后的最新结果——设置页「重新扫描」后清单随之变化。
    fn declared_exts(&self) -> HashSet<String>;

    /// 上者中需要**外部参数**（宽/高/布局）才能解码的子集。
    ///
    /// rawpx 这类解码器实现了可选导出 `iv_render_raw`，它认领哪些扩展名，哪些就走参数窗。
    /// 宿主不手写第二份后缀表——删掉解码器，裸数据路线随之消失。
    fn declared_raw_exts(&self) -> HashSet<String>;

    /// 声明为**矢量**（按视口重渲染，m152 v2）的扩展名并集。
    ///
    /// 判据来自解码器侧：`CAP_VECTOR` 声明/推定 + `iv_metadata` 的 `vector_exts` 键。
    /// 缺省实现 = 空集（不破坏已有实现）；viewer 的矢量分支与解码路由据此分流。
    ///
    /// ★ 2026-10-28 收口：矢量格式**一律由解码器按目标尺寸自渲染**（宪章 §5.2），
    /// 所以这份清单里的后缀**全部**是"解码器自渲染"那档——不再需要第二份
    /// "自渲染清单"（曾经是"宿主渲染 vs 解码器渲染"两档留下的）。
    fn declared_vector_exts(&self) -> HashSet<String> {
        HashSet::new()
    }

    /// 声明为矢量、且**能按可视区域重渲**的扩展名并集。
    ///
    /// 这一批 = "宿主能只渲看得见的那一块"的那些后缀：
    /// - 内置矢量（svg/svgz）：宿主自带 resvg，永远能；
    /// - 解码器自己渲染、且实现了 **ABI v6** 的 `iv_render_region` 的。
    ///
    /// ★ **不在这一集里的矢量后缀，宿主绝不能对它发区域请求**：没有区域接口的解码器
    /// 只能整幅渲染，宿主若按区域去要，拿到的是"整篇缩到那一小块"的**内容错误**的图。
    fn declared_region_vector_exts(&self) -> HashSet<String> {
        HashSet::new()
    }
}

static SOURCE: OnceLock<Box<dyn FormatSource>> = OnceLock::new();

/// 装入格式来源。重复调用为 no-op（幂等），故各装入点可以无脑前置。
pub fn install(src: Box<dyn FormatSource>) {
    let _ = SOURCE.set(src);
}

/// 声明为矢量的扩展名并集（m152 v2）；未装入时为空集。
pub fn declared_vector_exts() -> HashSet<String> {
    #[cfg(test)]
    if let Some((v, _)) = test_override::current() {
        return v;
    }
    match SOURCE.get() {
        Some(s) => s.declared_vector_exts(),
        None => HashSet::new(),
    }
}

/// 这个后缀走矢量（按视口 1:1 重渲）路线吗？
///
/// 内置矢量（svg/svgz，登记表 `is_vector` 位）∪ 解码器声明的矢量后缀
/// （`CAP_VECTOR` / `vector_exts`）。宿主不再手写第二份矢量后缀表。
pub fn is_vector_ext(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    BUILTIN_DECODERS
        .iter()
        .filter(|b| b.is_vector)
        .any(|b| b.extensions.contains(&e.as_str()))
        || declared_vector_exts().contains(&e)
}

/// 矢量格式的**渲染责任方**：谁把它变成像素。
///
/// 两档，调度口径不同，**必须分开判**：
///
/// | 模式 | 谁渲染 | 哪些格式 | 宿主怎么请求 |
/// |---|---|---|---|
/// | [`VectorRender::HostBuiltin`] | 宿主内置 resvg | `svg` `svgz` | 读文件 → resvg，**可视区域 1:1**（区域重渲 + 文档缓存，缩放零 IPC） |
/// | [`VectorRender::DllSelf`] | **解码器自己** | 解码器声明的矢量后缀（`vector_exts`） | 整幅按 target 尺寸；**有 ABI v6 区域通道时**按区域重渲 |
///
/// ★ 为什么**只有两档**（2026-10-28 收口）：宿主不再接受"解码器交出矢量文档、
/// 宿主用自己的 resvg 渲染"那条路（曾经的 `HostExport` 已整条退役）——
/// 宪章 §5.2 写死了"矢量格式一律由解码器按目标尺寸自渲染；宿主内置 resvg 只服务
/// svg/svgz"。留着那档的后果是**同一个格式挂着两条渲染路径**（导出路线 + 自渲染路线），
/// 换机器、换 DLL 版本、换字体栈时两边的像素可能不同——那正是"同一文件跨机同结果"
/// 这条立宪理由要防的事。
///
/// ★ 剩下的一维是"**能不能按区域重渲**"（[`is_region_renderable`]）：它与"谁渲染"
/// 相互独立，由解码器有没有实现 ABI v6 的 `iv_render_region` 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorRender {
    /// 宿主内置渲染器（resvg）——只服务 svg/svgz。
    HostBuiltin,
    /// 解码器自己按目标尺寸渲染（矢量族的主线）。
    DllSelf,
}

/// 这个后缀的矢量渲染责任方；不是矢量格式则 `None`。
pub fn vector_render_mode(ext: &str) -> Option<VectorRender> {
    vector_render_mode_of(ext, &declared_vector_exts())
}

/// [`vector_render_mode`] 的**纯函数**版本：判据只吃两份清单，不碰全局装入状态。
///
/// 抽出来是为了让"三种责任方各走各的调度口径"能被机器验证——全局装入的是真实解码器
/// 目录（测试机上通常一枚都没有），靠它测不出 `DllSelf` 这一档。
pub(crate) fn vector_render_mode_of(
    ext: &str,
    vector_exts: &HashSet<String>,
) -> Option<VectorRender> {
    let e = ext.to_ascii_lowercase();
    if BUILTIN_DECODERS
        .iter()
        .filter(|b| b.is_vector)
        .any(|b| b.extensions.contains(&e.as_str()))
    {
        return Some(VectorRender::HostBuiltin);
    }
    if vector_exts.contains(&e) {
        return Some(VectorRender::DllSelf);
    }
    None
}

/// 声明为矢量、且**能按可视区域重渲**的扩展名并集（解码器侧那部分）。
pub fn declared_region_vector_exts() -> HashSet<String> {
    #[cfg(test)]
    if let Some((_, r)) = test_override::current() {
        return r;
    }
    match SOURCE.get() {
        Some(s) => s.declared_region_vector_exts(),
        None => HashSet::new(),
    }
}

/// 这个后缀**能按可视区域重渲**吗（宿主只渲看得见的那一块）？
///
/// 这是 viewer 决定"区域重渲 vs 整图缓存"的**唯一**判据。两种情况算"能"：
/// - 内置矢量（svg/svgz）：宿主自带 resvg；
/// - 解码器自渲染 **且** 该解码器实现了 **ABI v6** 的 `iv_render_region`。
///
/// ★ 不在这一集合里的矢量后缀（自渲染但没有区域通道）**只能整幅渲染**：
/// 宿主对它发区域请求 = 拿到"整篇缩到那一小块"的内容错误图。
pub fn is_region_renderable(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    if BUILTIN_DECODERS
        .iter()
        .filter(|b| b.is_vector)
        .any(|b| b.extensions.contains(&e.as_str()))
    {
        return true;
    }
    declared_region_vector_exts().contains(&e)
}

/// 这个后缀是**按目标重画、但没有区域通道**的矢量吗？
///
/// 这一档（今天 = doc / doc-legacy 的 33 个后缀：pdf/emf/wmf/dxf/dwg/xps/ps/hpgl/cmx…）
/// 解码器只能**整幅**按 `target_w/h` 把文档重画一遍。宿主要放大到不失真，就得在倍率
/// 变化后**再要一档更大的整幅**——它们的 `natural` 只是"首次渲染的画布"，不是文档尺寸，
/// 夹在 `natural` 上本地重采样放大就退化成位图糊面（真机：PDF 放大成马赛克）。
///
/// 判据 = 解码器自渲染（[`VectorRender::DllSelf`]）**且** 不能区域重渲
/// （[`is_region_renderable`]）。能区域重渲的那些走 SVG 同一条 1:1 切片路，不归这里。
pub fn needs_zoom_rerender(ext: &str) -> bool {
    vector_render_mode(ext) == Some(VectorRender::DllSelf) && !is_region_renderable(ext)
}

/// 测试期覆盖槽：临时替换"矢量后缀 / 可按区域重渲的后缀"两份清单。
///
/// 为什么需要：判据的输入真源是**真实解码器目录**，测试机上通常一枚 DLL 都没有，
/// 于是 `DllSelf` 这一档在测试里永远走不到——只能靠"改真源"才测得动。
/// 这里给一个带还原语义的槽（守卫析构即还原），只在 `cfg(test)` 下编译。
#[cfg(test)]
pub(crate) mod test_override {
    use super::HashSet;
    use std::sync::Mutex;

    /// (矢量后缀, 可按区域重渲的后缀)
    pub(crate) type Pair = (HashSet<String>, HashSet<String>);

    static SLOT: Mutex<Option<Pair>> = Mutex::new(None);

    /// 还原守卫：析构时把槽还原成设置前的值（嵌套设置也安全）。
    pub(crate) struct Guard(Option<Pair>);

    impl Drop for Guard {
        fn drop(&mut self) {
            *SLOT.lock().unwrap_or_else(|e| e.into_inner()) = self.0.take();
        }
    }

    fn set_of(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// 设置两份清单；返回守卫，**必须**让它活到用例结束（否则还原提前发生）。
    pub(crate) fn set(vector: &[&str], region: &[&str]) -> Guard {
        let prev = SLOT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let pair = (set_of(vector), set_of(region));
        *SLOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(pair);
        Guard(prev)
    }

    /// 当前覆盖值（没设置则 `None`）。
    pub(crate) fn current() -> Option<Pair> {
        SLOT.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// 解码器认领的扩展名并集；未装入时为空集。
pub fn declared_exts() -> HashSet<String> {
    SOURCE.get().map(|s| s.declared_exts()).unwrap_or_default()
}

/// 需要裸数据参数的扩展名子集；未装入时为空集。
pub fn declared_raw_exts() -> HashSet<String> {
    SOURCE
        .get()
        .map(|s| s.declared_raw_exts())
        .unwrap_or_default()
}

/// 该扩展名是否需要**外部参数**（宽/高/布局）才能解码。
///
/// ★ 清单来自解码器自述（[`FormatSource::declared_raw_exts`]）：rawpx 这类解码器实现了
/// 可选导出 `iv_render_raw`，它认领哪些扩展名，哪些就走参数窗。宿主不手写第二份后缀表——
/// 删掉解码器，裸数据路线随之消失，不会出现"弹了参数窗却没人接"的错位。
pub fn is_raw(ext: &str) -> bool {
    SOURCE
        .get()
        .is_some_and(|s| s.declared_raw_exts().contains(ext))
}

/// 内置解码库的登记项（m56：内置路从此**有身份**，不再只是路由里一行硬编码后缀）。
///
/// 与解码器自述同构：解码器那份是 DLL 的 `iv_decoder_info()`，这一份是宿主的。
/// ★ **这张表就是唯一真源**——`gallery::is_image_ext_in` / 本文件的 `is_builtin_ext` /
/// `decode::decode_route` / 设置页「解码」页的候选，全部从它派生。历史上这三处
/// 各写一份内置后缀清单（`decode_route` 里硬编码 `"svg" | "svgz"`、本文件的
/// `is_builtin_ext`、`gallery::BUILTIN_EXTS`），加一条内置路要记得改三遍，
/// 漏一遍就是"清单说支持、路由不接"或反之（m45 抓的那族病）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinDecoder {
    /// 稳定 id：与解码器 id 同一 namespace，可被 `decoders_preferred` 点名。
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// 认领的扩展名（小写、无点）。
    pub extensions: &'static [&'static str],
    /// 已接入生产解码路。**没接的库不进任何清单**——列出来却解不了就是说谎。
    pub available: bool,
    /// 是否参与**默认自动仲裁**。
    ///
    /// ★ 这条是硬的：`auto_claim=false` 的内置库**只在用户点名时**出场。
    /// 理由见 `docs/产品定位与边界.md` §5.1——解码行为一旦随机器上装了什么而变，
    /// 同一个文件在两台机器上就可能解出不同结果，而没人知道。
    /// （2026-10-25：唯一一枚 `false` 的库是 WIC，它已随"不调系统解码器"这条裁决
    /// 整层删除；今天表里只剩 `resvg`，但这条闸门是机制、不是历史，留着。）
    pub auto_claim: bool,
    /// 矢量（按需重渲染）/ 位图。
    pub is_vector: bool,
    pub has_alpha: bool,
}

/// 宿主内置的解码库登记表（今天只有一枚：`resvg`）。
///
/// ★ 2026-10-25 用户裁决：**位图一律不内置**——表里不许再出现解位图的条目。
/// 曾经的第二枚是 `wic`（系统 Windows Imaging Component，2026-10-20 接进来、只走点名），
/// 它已整层删除：便携看图器不调系统解码器，同一张图在两台机器上必须解出同一个结果
/// （宪章 §5.1）。矢量（SVG）例外——它要按视口重渲，位图解码器的语义表达不了。
pub const BUILTIN_DECODERS: &[BuiltinDecoder] = &[BuiltinDecoder {
    id: "resvg",
    display_name: "resvg (内置)",
    description:
        "宿主内置的矢量渲染器，按目标尺寸无损光栅化；不做成解码器是刻意的（重复链接一整份 resvg 栈）",
    extensions: &["svg", "svgz"],
    available: true,
    auto_claim: true,
    is_vector: true,
    has_alpha: true,
}];

/// **已接入**的内置库（进清单的那批）。
pub fn available_builtins() -> impl Iterator<Item = &'static BuiltinDecoder> {
    BUILTIN_DECODERS.iter().filter(|b| b.available)
}

/// 按 id 查内置库（`decoders_preferred` 存的就是这个 id）。
///
/// ★ 与 [`builtin_owner`] 是两件事：后者问"这个后缀默认谁接"，前者问"用户点名的那位
/// 存不存在、认不认这个后缀"。今天点名分支已经随 WIC 一并删除（`crate::decoders::Host::decode`
/// 不再有内置位图那一臂），这个函数留着供设置页与清单侧按 id 反查。
pub fn builtin_by_id(id: &str) -> Option<&'static BuiltinDecoder> {
    available_builtins().find(|b| b.id == id)
}

/// 内置认领、且**不点名也解得开**的全部扩展名（小写、无点、去重后按字典序）。
///
/// ★ 判据是 `auto_claim`，不是"在表里"。这张表喂给目录清单与"算不算图片"的判定
/// （`gallery::known_exts` / `is_image_ext_in`），那里的问题必须是"**没装解码器时这台机器
/// 到底开不开得了这个文件**"。`auto_claim=false` 的内置库只有被点名才解得动——把它
/// 算进清单就会出现"文件进了网格、打开却说没有解码器"，正是宪章"能力与清单不脱节"
/// 要防的那种脱节（WIC 就是这么被挡在清单外的，2026-10-25 它已整层删除）。
/// 它在设置页「解码器 > 首选解码器」里照样可见（那里遍历整张表，见
/// `ui::settings::ext_rows_from`）。
pub fn builtin_exts() -> Vec<String> {
    let mut v: Vec<String> = available_builtins()
        .filter(|b| b.auto_claim)
        .flat_map(|b| b.extensions.iter().map(|s| (*s).to_string()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// 该扩展名的内置认领者（已接入的才算）。同一后缀被两枚内置认领是登记表写错，
/// 取表序第一个，并由单测钉住"不许发生"。
pub fn builtin_owner(ext: &str) -> Option<&'static BuiltinDecoder> {
    available_builtins().find(|b| b.extensions.contains(&ext))
}

/// 该扩展名在**默认自动仲裁**下的内置认领者（`auto_claim=false` 的内置库在这里隐身）。
pub fn builtin_auto_owner(ext: &str) -> Option<&'static BuiltinDecoder> {
    builtin_owner(ext).filter(|b| b.auto_claim)
}

/// 内置格式（不进解码器体系、宿主自认领）：判定与清单**同源**，见 [`BUILTIN_DECODERS`]。
///
/// 口径同 [`builtin_exts`]：**不点名也解得开**才算内置。
pub fn is_builtin_ext(ext: &str) -> bool {
    builtin_auto_owner(ext).is_some()
}

/// 空格预览可用的扩展名：内置 ∪ 解码器自述（**只看后缀归属，不看"这个文件解不解得开"**）。
///
/// ★ 判据唯一真源放格式清单数据层（本函数）：调用方不再各拼一份。历史缺口——
/// spacehook 的取件过滤曾只查 `declared_exts()`（解码器自述并集），内置 SVG 不在内，
/// 选中 SVG 时 `allowed=false` → 空格无效（K39）；主窗「算不算图片」判定
///（`gallery::is_image_ext_in` = 内置 ∪ 取好的那份 `declared` 清单）一直是对齐的。
///
/// ★ **不再在这里预判"裸像素解不开"**：曾经这里减掉
/// [`crate::core::decode::is_headerless_ext`] 那 11 个后缀，理由是"没有宽高/布局就解不开"。
/// 但那是**文件级**的事实，不是**后缀级**的：裸像素的首开参数由后缀定布局、由文件长度
/// 反推宽高（[`crate::core::decode::infer_raw_params`]），双击路径本来就"推得出就直接开"。
/// 拿后缀当代理，代价是 `.uyvy` 双击开得开、按空格毫无反应（用户实测，与 m209 的 `.raf`
/// 同一族）。现在准入只看清单，**能不能真出图由解码前那次参数推断决定**——推不出就静默
/// 不出窗（预览没有参数窗可弹），与双击"推不出才弹窗"是同一条分岔。
pub fn is_previewable(ext: &str) -> bool {
    previewable_of(ext, &declared_exts())
}

/// [`is_previewable`] 的**纯函数**版本：判据只吃一份清单，不碰全局装入状态。
///
/// 抽出来是为了让"准入只看后缀归属、不按 headerless 预判"这件事能被机器验证——
/// 单测里没有解码器目录（`declared_exts()` 恒空），拿它测不出 `.uyvy` 那条回归。
/// 大小写在这里统一（清单本身是小写、无点）。
pub(crate) fn previewable_of(ext: &str, declared: &HashSet<String>) -> bool {
    let e = ext.to_ascii_lowercase();
    is_builtin_ext(&e) || declared.contains(&e)
}

/// 空格预览的**魔数准入**：只认五种最常见、且解码器一定接得住的格式。
///
/// 调用方给前 ~64 字节即可；本函数纯字节判断，不碰盘。
/// BMP 的 `"BM"` 太短、纯文本也会命中，所以额外要求 6..10 的 reserved 全零、
/// 且数据偏移 ≥ 14（正常 BMP 头至少 14 字节）。
pub(crate) fn sniff_preview_magic(head: &[u8]) -> bool {
    let has = |p: &[u8]| head.len() >= p.len() && &head[..p.len()] == p;
    if has(b"\x89PNG\r\n\x1a\n") || has(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    if has(b"GIF87a") || has(b"GIF89a") {
        return true;
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return true;
    }
    if head.len() >= 14 && &head[..2] == b"BM" && head[6..10] == [0, 0, 0, 0] {
        let offset = u32::from_le_bytes([head[10], head[11], head[12], head[13]]);
        return offset >= 14;
    }
    false
}

/// 空格预览准入（带路径）：**已知可预览后缀不读盘**；空后缀 / 未认领后缀读前 64 字节，
/// 按 [`sniff_preview_magic`] 认五种常见图片。
///
/// 为什么只在这条闸上做、不动主窗清单：目录浏览按扩展名过滤，给每个未知文件读头
/// 会把大目录首屏拖慢；空格预览是“用户指了这一个文件”，读 64 字节可以忽略。
pub fn is_previewable_path(path: &std::path::Path) -> bool {
    is_previewable_path_of(path, &declared_exts())
}

pub(crate) fn is_previewable_path_of(path: &std::path::Path, declared: &HashSet<String>) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ext.is_empty() && previewable_of(&ext, declared) {
        return true;
    }
    let mut head = [0u8; 64];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read as _;
    let Ok(n) = f.read(&mut head) else {
        return false;
    };
    sniff_preview_magic(&head[..n])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_ext_is_svg_family_only() {
        assert!(is_builtin_ext("svg"));
        assert!(is_builtin_ext("svgz"));
        assert!(!is_builtin_ext("png"));
        assert!(!is_builtin_ext("jpg"));
        assert!(!is_builtin_ext(""));
    }

    /// 登记表的四条不变量——**这张表现在是内置清单的唯一真源**，它自己先得自洽。
    ///
    /// ★ 反向用例（把 `resvg` 的 `available` 改 `false`）会同时让
    /// `builtin_exts`/`is_builtin_ext`/`decode_route` 三处一起变，正是"同源"的证明：
    /// 再也没有"清单说支持、路由不接"那种半拉子状态。
    #[test]
    fn builtin_registry_is_self_consistent() {
        let mut seen_ext: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut seen_id: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for b in BUILTIN_DECODERS {
            assert!(
                !b.id.is_empty(),
                "内置库必须有 id（它要能被 decoders_preferred 点名）"
            );
            assert!(seen_id.insert(b.id), "内置库 id 重复：{}", b.id);
            assert!(
                !b.extensions.is_empty(),
                "内置库 {} 不认领任何后缀 = 隐身项",
                b.id
            );
            // 未接入的库**不许**参与仲裁，也**不该**认领后缀（否则就是列了个解不了的）。
            if !b.available {
                assert!(!b.auto_claim, "{}：没接入却参与自动仲裁", b.id);
            }
            for e in b.extensions {
                assert_eq!(
                    *e,
                    e.to_ascii_lowercase(),
                    "{} 认领的后缀必须小写无点",
                    b.id
                );
                assert!(
                    seen_ext.insert(*e),
                    "后缀 {e} 被两枚内置库认领——自动仲裁会随机取一个"
                );
            }
        }
        // 派生函数与表必须同结果（这条是"三处同源"收成一处之后的守门人）。
        assert_eq!(builtin_exts(), vec!["svg".to_string(), "svgz".to_string()]);
        assert_eq!(builtin_owner("svg").map(|b| b.id), Some("resvg"));
        assert_eq!(builtin_auto_owner("svg").map(|b| b.id), Some("resvg"));
        assert_eq!(builtin_by_id("nope"), None);
        // ★ 清单口径的那一刀：**位图一律不内置**。png 在登记表里没有认领者
        //   （2026-10-25 删掉 wic 之后），默认路只交给解码器——把位图算进内置清单
        //   就会出现"文件进了网格、打开却说没有解码器"那种脱节。
        assert_eq!(
            builtin_owner("png"),
            None,
            "png 不该有内置认领者：位图一律交给解码器"
        );
        assert_eq!(builtin_auto_owner("png"), None);
        assert!(!is_builtin_ext("png"));
        assert!(!builtin_exts().contains(&"png".to_string()));
    }

    #[test]
    fn builtin_exts_always_previewable_even_without_source() {
        // 未装入格式来源（SOURCE 空）时解码器清单为空，内置 SVG 必须仍然可预览——
        // 这是 K39（空格预览对 SVG 无效）的回归锁：不允许退回「只查解码器自述」。
        assert!(is_previewable("svg"));
        assert!(is_previewable("svgz"));
    }

    #[test]
    fn previewable_without_source_is_builtin_only() {
        // 未装入格式来源：解码器认领的格式一律不算（清单本就该空，读到的是真空）。
        assert!(!is_previewable("png"));
        assert!(!is_previewable("jpg"));
        assert!(!is_previewable("avif"));
    }

    /// 同一枚 raw 解码器认领的两类后缀，空格预览**都**要接。
    ///
    /// ★ 回归锁（用户实测两轮）：`.raf` 与 `.uyvy` 都是"双击开得开、按空格没反应"。
    /// 第一次的病灶是按 `is_raw`（DLL 级口径）排整档，第二次的病灶是改成按
    /// `is_headerless_ext` 排裸像素——两次都是**拿后缀当"解不解得开"的代理**。
    /// 裸像素的真实分岔在解码前那次参数推断（后缀定布局 + 文件长度定宽高），
    /// 准入这一层只回答"这个后缀有没有解码器认领"，不回答"这个文件解不解得出"。
    #[test]
    fn camera_raw_and_headerless_pixels_are_both_admitted() {
        let declared: HashSet<String> = [
            "raf", "nef", "cr2", "dng", "rgb", "yuv", "uyvy", "bgra", "nv12", "png",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for ext in ["raf", "nef", "cr2", "dng"] {
            assert!(
                previewable_of(ext, &declared),
                "相机 RAW（.{ext}）自带完整文件头，空格预览必须能开"
            );
        }
        for ext in ["rgb", "yuv", "uyvy", "bgra", "nv12"] {
            assert!(
                previewable_of(ext, &declared),
                "裸像素（.{ext}）的参数可推断，空格预览不该按后缀预判挡掉"
            );
        }
        assert!(previewable_of("png", &declared), "普通位图照旧可预览");
        assert!(!previewable_of("xyz", &declared), "清单没认领的后缀不接");
        assert!(
            previewable_of("SVG", &declared),
            "大小写不该影响内置格式判定"
        );
    }

    /// 矢量**渲染责任方**的两档分流（m153 A 方案，2026-10-28 收口）。
    ///
    /// ★ 这条是"矢量无损放大"能不能落地的守门人：两档的**调度口径完全不同**——
    /// 内置那档走"可视区域 1:1 重渲"，解码器自渲染那档走"整幅按 target"，
    /// 有 ABI v6 区域通道时走"按区域重渲"。
    /// 判错一档，后果不是"糊一点"，而是**内容错误**（整篇缩到那一小块）。
    #[test]
    fn vector_render_mode_splits_two_responsibilities() {
        let empty = HashSet::new();
        // ① 内置矢量（svg/svgz）→ 宿主内置 resvg。
        assert_eq!(
            vector_render_mode_of("svg", &empty),
            Some(VectorRender::HostBuiltin)
        );
        assert_eq!(
            vector_render_mode_of("SVGZ", &empty),
            Some(VectorRender::HostBuiltin),
            "后缀必须大小写不敏感"
        );

        // ② 解码器声明的矢量后缀 → **解码器自渲染**（宿主不碰它的像素）。
        let vec: HashSet<String> = ["wmf".to_string()].into_iter().collect();
        assert_eq!(
            vector_render_mode_of("wmf", &vec),
            Some(VectorRender::DllSelf)
        );

        // ③ 不是矢量 → None。
        assert_eq!(vector_render_mode_of("png", &vec), None);
    }

    /// ★ **矢量只有一条渲染路径**：解码器声明的矢量后缀**一律**是"解码器自渲染"，
    /// 不存在"解码器交出矢量文档、宿主用自己的 resvg 渲染"的第二档。
    ///
    /// 这条锁的是宪章 §5.2（"矢量格式一律由解码器按目标尺寸自渲染；宿主内置 resvg
    /// 只服务 svg/svgz"）：一旦宿主重新长出"宿主渲染矢量"的分支，同一个格式就会挂着
    /// 两条渲染路径，换机器/换 DLL 版本时两边的像素可能不同。
    #[test]
    fn decoder_declared_vectors_are_always_self_rendered() {
        let guard = test_override::set(&["wmf", "emf"], &["wmf"]);
        for ext in ["wmf", "emf"] {
            assert_eq!(
                vector_render_mode(ext),
                Some(VectorRender::DllSelf),
                "{ext} 必须由解码器自己渲染"
            );
        }
        // 内置那批仍然是宿主渲（且只有内置那批）。
        assert_eq!(vector_render_mode("svg"), Some(VectorRender::HostBuiltin));
        assert_eq!(vector_render_mode("png"), None);
        drop(guard);
    }

    /// ★「能不能按区域重渲」是**独立于"谁渲染"**的一维（ABI v6 起）。
    ///
    /// - `HostBuiltin`：宿主自己渲 ⇒ 天然能按区域重渲；
    /// - `DllSelf` **且**实现了 `iv_render_region` ⇒ 解码器按区域渲 ⇒ 也能；
    /// - `DllSelf` **没有**区域通道 ⇒ **只能整幅渲染**，宿主对它发区域请求 = 内容错误。
    ///
    /// 这条是"矢量放大到 4000% 也无损"的**宿主侧开关**：有它才发区域请求。
    #[test]
    fn region_renderable_is_an_axis_of_its_own() {
        // 内置矢量（svg/svgz）：宿主自带 resvg，永远能。
        assert!(is_region_renderable("svg"));
        assert!(is_region_renderable("svgz"));

        // 自渲染 + 有区域通道（ABI v6）⇒ 能。
        let guard = test_override::set(&["emf", "wmf"], &["emf"]);
        assert_eq!(vector_render_mode("emf"), Some(VectorRender::DllSelf));
        assert!(
            is_region_renderable("emf"),
            "实现了 iv_render_region 的自渲染解码器必须能接区域请求"
        );
        // 自渲染 + 没有区域通道 ⇒ 不能（宿主对它只能整幅请求）。
        assert!(
            !is_region_renderable("wmf"),
            "没有区域接口的解码器绝不能被发区域请求：那是内容错误，不是糊"
        );
        // 非矢量 ⇒ 不能。
        assert!(!is_region_renderable("png"));
        drop(guard);
    }

    /// ★ **按目标重画**的那一档 = 自渲染 **且** 没有区域通道（m189）。
    ///
    /// 宿主靠这条判据决定"放大后要不要请解码器整幅重画一档"：漏判 ⇒ 本地插值放大，
    /// PDF 之类放大成马赛克（用户实测）；误判到有区域通道的那些 ⇒ 两条渲染路打架，
    /// 切片请求的坐标系也没人维护。三档各取一枚验，外加内置与位图必须为假。
    #[test]
    fn zoom_rerender_is_dll_self_without_region_channel() {
        let guard = test_override::set(&["pdf", "wmf"], &["wmf"]);
        assert!(
            needs_zoom_rerender("pdf"),
            "自渲染 + 无区域通道 ⇒ 放大只能整幅重画"
        );
        assert!(
            !needs_zoom_rerender("wmf"),
            "有区域通道的走 1:1 切片路，不该再整幅重画"
        );
        assert!(!needs_zoom_rerender("svg"), "内置矢量由宿主按区域重渲");
        assert!(!needs_zoom_rerender("png"), "位图没有重画这回事");
        drop(guard);
    }

    /// m331：空格预览的魔数准入——五种签名、截断、非图片、BMP 假阳性。
    #[test]
    fn preview_magic_covers_five_signatures_and_rejects_junk() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let jpeg = b"\xFF\xD8\xFF\xE0\0\x10JFIF";
        let gif87 = b"GIF87a\x01\0\x01\0";
        let gif89 = b"GIF89a\x01\0\x01\0";
        let bmp = b"BM\x36\0\0\0\0\0\0\0\x36\0\0\0";
        let webp = b"RIFF\x24\0\0\0WEBPVP8 ";
        for (name, head) in [
            ("png", &png[..]),
            ("jpeg", &jpeg[..]),
            ("gif87", &gif87[..]),
            ("gif89", &gif89[..]),
            ("bmp", &bmp[..]),
            ("webp", &webp[..]),
        ] {
            assert!(sniff_preview_magic(head), "{name} 应命中");
        }
        assert!(!sniff_preview_magic(b"hello world"), "文本不该命中");
        assert!(!sniff_preview_magic(b"\x89PNG"), "截断的 PNG 不该命中");
        assert!(
            !sniff_preview_magic(b"BMW is a car"),
            "BM 开头不够，reserved 必须全零"
        );
        assert!(
            !sniff_preview_magic(b"RIFF\0\0\0\0WAVE"),
            "RIFF 但不是 WEBP"
        );
    }

    /// m331：已知后缀直接放行（不读盘）；无后缀 / 错后缀读魔数；非图片拒绝。
    #[test]
    fn preview_path_admits_magic_only_when_the_suffix_is_unclaimed() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("m331-preview-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let declared: HashSet<String> = ["png".to_string()].into_iter().collect();

        // 后缀在清单里：内容根本不是 PNG 也放行——不读盘是这条闸的性能承诺。
        let known = dir.join("known.png");
        std::fs::write(&known, b"not a png at all").unwrap();
        assert!(
            is_previewable_path_of(&known, &declared),
            "已知图片后缀不读盘就该放行"
        );

        let no_ext = dir.join("noext");
        std::fs::write(&no_ext, png).unwrap();
        assert!(
            is_previewable_path_of(&no_ext, &declared),
            "无后缀 PNG 要放行"
        );

        let wrong = dir.join("photo.bin");
        std::fs::write(&wrong, png).unwrap();
        assert!(
            is_previewable_path_of(&wrong, &declared),
            "错后缀 PNG 要放行"
        );

        let text = dir.join("notes.txt");
        std::fs::write(&text, b"hello world").unwrap();
        assert!(!is_previewable_path_of(&text, &declared), "非图片不许预览");

        let missing = dir.join("gone");
        assert!(
            !is_previewable_path_of(&missing, &declared),
            "读不到文件静默拒绝"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
