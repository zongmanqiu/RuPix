//! 对外清单：`list_decoders`（设置页展示）与自述扩展名并集（`core::formats` 的格式来源）。
//!
//! 两份清单同源：都来自一次目录扫描 + 逐个 DLL 读自述，按热加载代次缓存。
//! 「清单与实际能力不可能脱节」这条性质就落在这里。
use super::dllmeta::*;
use super::*;

// ── 供设置页「解码器」展示的列表（UI 线程安全）───────────────────────

/// 一个解码器 DLL 的加载健康度（能否被宿主真正使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// abi 协商通过、符号齐全。
    Ok,
    /// 解码器运行时 abi 与宿主不符。
    AbiMismatch { got: u32 },
    /// 文件/符号缺失、Ffi 调用 panic 等。
    LoadFailed(String),
}

impl Health {
    /// UI 上的一行状态文字。
    pub fn label(&self) -> String {
        match self {
            Health::Ok => "可用".into(),
            Health::AbiMismatch { got } => format!("版本不匹配（abi={got}）"),
            Health::LoadFailed(e) => format!("加载失败（{e}）"),
        }
    }
}

/// 从 DLL 文件名 `<id>_<ver>.dll` 解析出版本号。
///
/// 文件名只带 **id + 版本**（解码器发布时的命名约定）。
/// **ABI 号不在名字里**：宿主接受的是区间（`MIN_ABI_VERSION..=IV_ABI_VERSION`），
/// 写成 `-abi4` 既判断不了"能不能用"，又成了第二份会漂的真源——它读自解码器自述的
/// `iv_abi_version()`，落在 [`DecoderEntry::abi`]。
///
/// 手工丢进来的文件可能不合规——解析不出就返回 `None`，详情窗留空、不硬编假数据。
/// `id` 只含小写字母/数字/连字符，版本以数字开头，所以取最后一个下划线之后的部分。
///
/// ★ 2026-10-26 从 `ui/settings` 搬到这里：**「已下载」的判定要用它**（远程清单里写的是
/// 版本，本机文件只知道文件名），而那套判定属于解码器层的数据问题，不该让 UI 先拼一遍
/// 再交给层里比。
pub fn parse_dll_version(file: &str) -> Option<String> {
    let name = file.strip_suffix(".dll").unwrap_or(file);
    let (id, ver) = name.rsplit_once('_')?;
    if id.is_empty() || ver.is_empty() {
        return None;
    }
    if !ver.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(ver.to_string())
}

/// 设置页上的一行：一个 DLL 的自述 + 文件信息 + 切换状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoderEntry {
    /// 解码器 id（自述得到；读不出来时退化为文件名主干）。
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// 认领的扩展名（自述得到）。
    pub extensions: Vec<String>,
    /// DLL 文件名。
    pub file: String,
    /// DLL **完整路径**（`catalog::dir()/file`）——详情窗展示 + 卸载按钮据此删文件。
    /// 单独一份而不让用户自己拼：safe-mode 与 `RUPIX_DECODERS_DIR` 都改过 `dir()`，
    /// 只有扫描现场知道当时用的是哪个目录。
    pub path: std::path::PathBuf,
    /// 文件字节数。
    pub size: u64,
    /// DLL 修改时间；`metadata()` 拿不到时 `None`（详情窗留空、不硬编假数据）。
    pub mtime: Option<std::time::SystemTime>,
    pub health: Health,
    /// 用户开关：true = 启用。
    pub enabled: bool,
    /// 是否实现了可选导出 `iv_render_raw`（裸数据参数化解码）。
    ///
    /// 只有这类解码器认领的扩展名才会走参数窗（[`declared_raw_exts`]）。
    pub has_raw: bool,
    /// 是否具备**矢量**能力（m152 v2）。矢量后缀的精确清单在 [`Self::vector_exts`]。
    pub has_vector: bool,
    /// 矢量后缀的精确清单：DLL 级矢量（`CAP_VECTOR`/`is_vector`）时为自述全部后缀，
    /// 合并档为 metadata `vector_exts` 键里的成员级声明。空 = 非矢量。
    pub vector_exts: Vec<String>,
    /// 这枚 DLL 的矢量后缀**能按可视区域重渲**（宿主只渲看得见的那一块）。
    ///
    /// 判据：实现了 **ABI v6** 的 `iv_render_region`（或声明了 `CAP_REGION`）。
    /// 矢量格式一律由**解码器自己**渲染（宪章 §5.2），所以这条能力位是
    /// "放大到 4000% 也无损"的唯一开关。
    ///
    /// ★ 不在这一类的矢量后缀**只能整幅渲染**：宿主对它发区域请求 = 拿到
    /// "整篇缩到那一小块"的**内容错误**的图，不是"糊一点"。
    pub vector_region: bool,
    /// 解码器自述的 ABI 号（`iv_abi_version()`）。`None` = 连这个导出都没调到
    /// （架构不符 / 缺依赖 / 不是本项目的 DLL）——那种情况下文件名也给不出答案，
    /// 因为名字里早就没有 abi 段了。
    pub abi: Option<u32>,
    /// Optional `iv_metadata()` table. The installed details window uses
    /// `version` / `author` / `license` / `released` from here; absent values
    /// fall back to filename or unknown. Third-party single DLLs use the same path.
    pub metadata: Vec<(String, String)>,
}

/// 四份清单：(全部自述扩展名, 裸数据参数化扩展名, 矢量扩展名,
/// 可按区域重渲的矢量扩展名)。
///
/// 用类型别名而不是裸元组：四元组直接写出来会被 clippy 判"类型太复杂"，
/// 而它们**必须**同源同批算出（分开算要加载两遍 DLL）。
pub(super) type DeclaredExts = (
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
);

/// 全部自述扩展名 **∪** 支持裸数据参数化解码的扩展名，按热加载代次缓存。
///
/// 五份清单装在一个缓存里是因为它们同一次目录扫描就能拿到：分开算要加载两遍 DLL。
pub(super) fn declared_cached() -> DeclaredExts {
    /// (热加载代次, 五份清单)
    type ExtCache = (u64, DeclaredExts);
    static CACHE: Mutex<Option<ExtCache>> = Mutex::new(None);
    let gen = rescan_gen();

    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((cached_gen, cached)) = &*g {
        if *cached_gen == gen {
            return cached.clone();
        }
    }

    let mut all: HashSet<String> = HashSet::new();
    let mut raw: HashSet<String> = HashSet::new();
    let mut vector: HashSet<String> = HashSet::new();
    let mut region: HashSet<String> = HashSet::new();
    if !safe_mode() {
        for e in list_decoders() {
            // 只认自述读成功的、且没被用户关掉的解码器。
            if !matches!(e.health, Health::Ok) || is_disabled(&e.id) {
                continue;
            }
            if e.has_raw {
                raw.extend(e.extensions.iter().cloned());
            }
            // 矢量后缀用**精确清单**（DLL 级矢量 = 全部自述后缀；合并档 = metadata
            // `vector_exts` 成员级声明）——不能拿 has_vector 混同全部后缀，否则 pro
            // 档的 jp2/dcm 等位图成员会被误判成矢量。
            vector.extend(e.vector_exts.iter().cloned());
            // 能按区域重渲的那批单列：viewer 据此决定"区域重渲 vs 整图缓存"。
            //
            // 合并档里可能只有一部分矢量成员真能按区域重渲（doc 的 EMF/EMZ 没有
            // 视口 API、XPS 只出空白页），所以 metadata 可以再给一条成员级
            // `region_exts` 精确子集；**缺席时按整枚矢量**（与旧口径一致）。
            if e.vector_region {
                match region_exts_of(&e.metadata) {
                    Some(list) => region.extend(list),
                    None => region.extend(e.vector_exts.iter().cloned()),
                }
            }
            all.extend(e.extensions);
        }
    }
    let cached = (all.clone(), raw.clone(), vector.clone(), region.clone());
    *g = Some((gen, cached.clone()));
    cached
}

/// 所有**解码器自述**的扩展名并集（小写、不带点）。
///
/// # 这是格式清单的主要来源
///
/// `gallery::is_image_ext_in` 用它决定目录里哪些文件算"图片"。装了 `raw` 解码器就出现
/// `.cr2`，把 DLL 删掉它就消失——**清单与实际能力物理上不可能脱节**，不像手写清单
/// 那样会出现"列了 .jxl 却打不开"或"能打开却翻不到"。
///
/// 宿主自己那份内置登记表 [`crate::core::formats::BUILTIN_DECODERS`] 因此只覆盖
/// "不进解码器体系"的东西（按 `auto_claim` 判据，目前是 SVG/SVGZ），
///
/// 结果按热加载代次缓存：目录没变就不重复加载 DLL。
pub fn declared_exts() -> HashSet<String> {
    install_format_source();
    declared_cached().0
}

/// 需要**外部参数**（宽/高/布局）才能解码的扩展名——只有实现了可选导出
/// `iv_render_raw` 的解码器自述的那些。
///
/// # 为什么这份清单必须是动态的
///
/// 它与 [`declared_exts`] 同源同批算出：把某个实现 `iv_render_raw` 的解码器删掉或关掉，
/// 它认领的后缀就不再走参数窗而是回到常规解码路径，不存在"清单说要参数、实际没人接"的错位。
/// 这里的并集是**路由用的**；`core::decode::HEADERLESS_EXTS` 是另一回事——它标"哪些后缀
/// **真的没有文件头、必须由用户给宽/高/布局**"（`is_headerless_ext`，11 个），读者是参数窗
/// （`ui/raw_dialog.rs`）与空格预览的推断分支（`platform/spacehook.rs`）。两张表各判各的。
pub fn declared_raw_exts() -> HashSet<String> {
    install_format_source();
    declared_cached().1
}

/// 声明为**矢量**的扩展名并集（m152 v2）：`CAP_VECTOR` 声明/推定者的自述后缀
/// ∪ metadata `vector_exts` 键里的成员级声明。与 [`declared_raw_exts`] 同源同批。
pub fn declared_vector_exts() -> HashSet<String> {
    install_format_source();
    declared_cached().2
}

/// 声明为**矢量**、且**能按可视区域重渲**的扩展名并集。
///
/// 这一批是 viewer 敢发区域请求的全部依据：
/// - 有导出通道的解码器（宿主拿矢量文档用 resvg 渲）；
/// - 自渲染但实现了 **ABI v6** `iv_render_region` 的解码器（解码器自己按区域渲）。
///
/// ★ 不在这一集的矢量后缀**只能整幅渲染**——对它们发区域请求会拿到
/// "整篇缩到那一小块"的内容错误图（判据见 [`DecoderEntry::vector_region`]）。
pub fn declared_region_vector_exts() -> HashSet<String> {
    install_format_source();
    declared_cached().3
}

/// 矢量后缀精确清单（m152 v2）：**成员级声明优先**，其次才是 DLL 级口径。
///
/// 两种声明方式各管各的：
/// - **成员级**（metadata 键 `vector_exts`）：合并档（如 `pro`）用的就是这种——它自述的
///   后缀里位图占多数（jp2/dcm/exr…），只有 wmf/wmz 能矢量重渲，所以必须显式枚举。
/// - **DLL 级**（`CAP_VECTOR` 能力位 / `is_vector` 自述）："这枚 DLL 提供的都是矢量"，
///   此时全部自述后缀都是矢量后缀。
///
/// ★ 为什么成员级**优先**：合并档是"位图成员占多数、只有 wmf/wmz 是矢量"的那种，
/// `CAP_VECTOR` 说的是**能力**不是**范围**。若按 DLL 级优先，pro 声明了 `CAP_VECTOR`
/// 就会被当成"整枚都是矢量"，jp2/dcm 等位图成员会被误判成矢量后缀。
/// （2026-10-28：矢量导出路线退役后，合并档不再声明 `CAP_VECTOR`；这条优先级
/// 仍然保留——将来出现"整枚都是矢量"的专用 DLL（如通用矢量栈那枚）时，
/// DLL 级口径照样用得上。）
pub(super) fn vector_exts_of(desc: &Descriptor) -> Vec<String> {
    if let Some((_, list)) = desc.metadata.iter().find(|(k, _)| k == "vector_exts") {
        return split_ext_list(list);
    }
    if desc.caps & crate::core::abi::CAP_VECTOR != 0 || desc.is_vector {
        return desc.extensions.clone();
    }
    Vec::new()
}

/// metadata 的成员级 `region_exts` 声明：**只有列出的矢量后缀**会收到区域请求。
///
/// 缺席（`None`）= 这枚 DLL 声明的全部矢量后缀都走区域通道（旧口径）。声明了却
/// 列空表 = 一个都不走（合法：这枚 DLL 有 `iv_render_region` 但没有可用的视口）。
///
/// ★ 为什么需要这条：合并档里"能不能按区域重渲"是**成员级**事实——doc 的 cad/
/// wmf/hpgl/pdf/print 都能，EMF/EMZ 的 `emfsdk` 只有目标宽高、XPS 还只出空白页。
/// 宿主若按整枚矢量发区域请求，那两类会拿到"整篇缩到那一小块"的**内容错误**图。
pub(super) fn region_exts_of(metadata: &[(String, String)]) -> Option<Vec<String>> {
    metadata
        .iter()
        .find(|(k, _)| k == "region_exts")
        .map(|(_, list)| split_ext_list(list))
}

/// 分号分隔的后缀清单（小写、去点、去空项）。
fn split_ext_list(list: &str) -> Vec<String> {
    list.split(';')
        .map(|s| s.trim().trim_start_matches('.').to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 这枚 DLL 的矢量后缀**能不能按可视区域重渲**（ABI v6 区域通道）。
///
/// 判据只看**硬证据**：实现了可选导出 `iv_render_region`（符号在不在），或声明了
/// `CAP_REGION` 能力位。两者都缺 = 这枚 DLL 只能整幅渲染。
///
/// ★ 抽成独立函数是为了让这条能被机器验证：宿主对"不能按区域重渲"的矢量后缀
/// 发区域请求，拿到的是"整篇缩到那一小块"的**内容错误**图，不是"糊一点"。
pub(super) fn region_capable(vector_exts: &[String], has_region_sym: bool, caps: u32) -> bool {
    !vector_exts.is_empty() && (has_region_sym || caps & crate::core::abi::CAP_REGION != 0)
}

/// 扫 `decoders/` 下所有 `*.dll`，逐个读自述，返回给设置页展示。
///
/// 会加载每个 DLL 以读元数据（读自述后即释放库句柄）。这是"只靠单个 DLL 自述"的代价。
pub fn list_decoders() -> Vec<DecoderEntry> {
    let mut out = Vec::new();
    // safe-mode 与 Host::decode 同规（见上）：本函数要逐个 LoadLibrary 读自述，
    // 一开就会跑每个 DLL 的 DllMain——安全模式下连读都不读，直接空列表。
    if safe_mode() {
        return out;
    }
    let Some(dir) = catalog::dir() else {
        return out;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|x| x.eq_ignore_ascii_case("dll"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();

    for p in paths {
        let file = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let meta = std::fs::metadata(&p).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = meta.as_ref().and_then(|m| m.modified().ok());
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let (
            id,
            display_name,
            description,
            extensions,
            health,
            has_raw,
            vector_exts,
            vector_region,
            abi,
            metadata,
        ) = match inspect(&p) {
            Inspect::Ok { desc, sym, .. } => (
                desc.id.clone(),
                desc.display_name.clone(),
                desc.description.clone(),
                desc.extensions.clone(),
                Health::Ok,
                // 可选导出 `iv_render_raw`：认领的扩展名要走参数窗。
                sym.render_raw.is_some(),
                {
                    // 矢量后缀精确清单（m152 v2）：成员级 metadata 声明优先，
                    // 否则按 DLL 级口径（全部自述后缀）。口径见 [`vector_exts_of`]。
                    vector_exts_of(&desc)
                },
                {
                    // 能不能按可视区域重渲：实现了 ABI v6 的 iv_render_region（或声明了
                    // CAP_REGION）的矢量解码器——宿主据此才敢发区域请求。
                    let exts = vector_exts_of(&desc);
                    region_capable(&exts, sym.render_region.is_some(), desc.caps)
                },
                Some(desc.abi),
                desc.metadata.clone(),
            ),
            // 版本不符时**仍然知道**它自述的是哪一版：这个数字是用户排查
            // "为什么它被拒"的唯一线索，别丢。
            Inspect::AbiMismatch { got } => (
                stem.clone(),
                stem.clone(),
                String::new(),
                Vec::new(),
                Health::AbiMismatch { got },
                false,
                Vec::new(),
                false,
                Some(got),
                Vec::new(),
            ),
            Inspect::Error(e) => (
                stem.clone(),
                stem.clone(),
                String::new(),
                Vec::new(),
                Health::LoadFailed(e),
                false,
                Vec::new(),
                false,
                None,
                Vec::new(),
            ),
        };
        let enabled = !is_disabled(&id);
        out.push(DecoderEntry {
            id,
            display_name,
            description,
            extensions,
            file,
            path: p.clone(),
            size,
            mtime,
            health,
            enabled,
            has_raw,
            has_vector: !vector_exts.is_empty(),
            vector_exts,
            vector_region,
            abi,
            metadata,
        });
    }
    out
}
