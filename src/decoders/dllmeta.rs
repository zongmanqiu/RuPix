//! 从 DLL 读出来的自述（`iv_decoder_info`）：ABI 版本判定、`iv_decoder_info` 解析、能力位推定、已加载解码器。
//!
//! 这里只回答「这个 DLL 是什么」，不选谁、也不调谁——那两件在 [`super::select`]
//! 与 [`super::decode`]。加载前的防护（绝对路径 / 硬搜索路径）仍按 [`super`] 模块头的表。
use super::*;

/// 版本判定的**下界**：ABI v5 是导出签名冻结下界（v4 因改名断点已不兼容；
/// 现行上界 v6 只做加法，所以下界锁死 5、不许抬）。
///
/// v1→v4 演进改过 `IvRenderReq/IvRenderOut` 的内存布局和 `iv_render` 的签名；v5 改名后
/// `MIN_ABI_VERSION` 同步抬到 5，旧 DLL 一律拒载
/// （v3 加扩展名尾参），老 DLL 按老布局传参、新宿主按新布局解读，必崩——那
/// 之前只能「相等才收、全部重编」。**v4 起立约：已发布的导出签名与结构体
/// 永不再改，新能力一律走新增可选导出**（如 `iv_render_raw`，缺席 = 缺能力，
/// 不是错误）。因此解码器版本落在这个区间内即可加载：
///
/// ```text
/// MIN_ABI_VERSION ≤ 解码器 abi ≤ IV_ABI_VERSION
/// ```
///
/// ★ **v6 是加法，不是断点**（2026-10-28）：v6 只新增了可选导出 `iv_render_region`、
/// 新结构体 `IvRegionReq` 与新能力位 `CAP_REGION`，**v5 的每一个导出签名与结构体
/// 布局逐字节未动** ⇒ v5 的老 DLL 不必重编就能被 v6 宿主收载、照常工作
/// （它们只是没有区域渲染能力，宿主探测不到符号 = 缺能力，不是错误）。
/// 下界因此**不动**（仍是 5）：把下界抬到 6 会让所有 v5 DLL 集体作废，
/// 而那正是"加法式升级"要避免的事。
pub(super) const MIN_ABI_VERSION: u32 = 5;

/// 解码器自述的 ABI 版本能否被本宿主接受（区间判定，见 `MIN_ABI_VERSION`）。
pub(super) fn abi_compatible(got: u32) -> bool {
    (MIN_ABI_VERSION..=IV_ABI_VERSION).contains(&got)
}

/// 读一个解码器返回的静态 C 串（null → 空串）。
pub(super) unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

// ── 从 DLL 读出来的自述 ────────────────────────────────────────────

/// 一个 DLL 自述的信息（不含函数指针，可自由拷贝展示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub extensions: Vec<String>,
    pub is_vector: bool,
    pub has_alpha: bool,
    /// **解码能力位图**（`CAP_*`，见 `core::abi`）。宿主按它路由请求类型，
    /// 不按解码器身份猜。缺席 `iv_caps` 时由 [`caps_of`] 从既有证据推定。
    pub caps: u32,
    /// 解码器自述的 ABI 号，**就是 `iv_abi_version()` 的返回值**（加载时读的那一个）。
    ///
    /// 它不进文件名：宿主接受的是**区间**（`MIN_ABI_VERSION..=IV_ABI_VERSION`），
    /// 所以文件名叫 `-abi4` 既判断不了"能不能用"，又成了第二份会漂的真源
    /// （2026-10-25 起发布名统一为 `<id>_<ver>.dll`）。
    pub abi: u32,
    /// Optional `iv_metadata` key/value table. Missing export = empty (legacy DLL).
    pub metadata: Vec<(String, String)>,
}

/// 一个解码器 DLL 暴露的解码符号（ABI v3：probe/render 都带小写扩展名尾参）。
/// `render_raw` 是 v4 内的可选导出（裸数据参数化解码），探测不到即 `None`。
/// 能力位图**不在这里**——`iv_caps` 只在 `inspect` 里调一次，结论落在
/// `Descriptor::caps`（要存的是裁定后的位图，不是那个函数指针）。
#[derive(Clone, Copy)]
pub(super) struct Symbols {
    pub(super) probe: unsafe extern "C" fn(*const u8, usize, *const c_char) -> i32,
    pub(super) render: unsafe extern "C" fn(
        *const u8,
        usize,
        *const IvRenderReq,
        *mut u8,
        usize,
        *mut IvRenderOut,
        *const c_char,
    ) -> i32,
    pub(super) last_error: unsafe extern "C" fn(*mut c_char, usize) -> i32,
    /// 可选导出 `iv_render_raw`（v4 裸数据参数化解码）。缺席 = `None`。
    pub(super) render_raw: Option<
        unsafe extern "C" fn(
            *const u8,
            usize,
            u32,
            u32,
            u32,
            *const IvRenderReq,
            *mut u8,
            usize,
            *mut IvRenderOut,
        ) -> i32,
    >,
    /// 可选导出 `iv_render_region`（**ABI v6** 区域渲染）。缺席 = `None`。
    ///
    /// ★ 这是"矢量图放大到 4000% 也无损"的唯一通道：宿主只给"看得见的那一块"，
    /// 解码器按密度**重新光栅化**（不是渲大图再缩）。探测不到 = 这枚 DLL 只能
    /// 整幅渲染，宿主对它走整幅路线（略糊，但不出内容错误图）。
    pub(super) render_region: Option<crate::core::abi::IvRenderRegionFn>,
}

/// 加载一个 DLL 并读取其自述的结果。
pub(super) enum Inspect {
    Ok {
        desc: Descriptor,
        lib: Library,
        sym: Symbols,
    },
    /// 解码器运行时 abi 与宿主不符（连其它符号都不取）。
    AbiMismatch {
        got: u32,
    },
    /// 上次加载时宿主崩溃/硬故障，盘上还留着 `<dll>.loading`：本次不碰它。
    Skipped,
    Error(String),
}

/// `a;b;c` → `["a","b","c"]`：去空白、去前导点、转小写、丢空段。
/// Read the optional `iv_metadata` table. Size/count are checked before touching
/// the array; unknown keys are kept for future consumers.
fn read_metadata(ptr: *const IvMetadata) -> Vec<(String, String)> {
    const MAX_FIELDS: u32 = 256;
    if ptr.is_null() {
        return Vec::new();
    }
    let meta = unsafe { &*ptr };
    if meta.size < std::mem::size_of::<IvMetadata>() as u32
        || meta.field_count > MAX_FIELDS
        || (meta.field_count > 0 && meta.fields.is_null())
    {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(meta.field_count as usize);
    for i in 0..meta.field_count as usize {
        let field = unsafe { &*meta.fields.add(i) };
        let key = unsafe { cstr(field.key) };
        if key.is_empty() {
            continue;
        }
        out.push((key, unsafe { cstr(field.value) }));
    }
    out
}

pub(super) fn parse_ext_list(s: &str) -> Vec<String> {
    s.split(';')
        .map(|x| x.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|x| !x.is_empty())
        .collect()
}

/// 能力位图的最终裁定：**解码器声明了就用声明值，没声明就按既有证据推定**。
///
/// 推定只用真存在的证据（`is_vector` 字段、`iv_render_raw` / `iv_render_region` 符号在不在），
/// 所以 [`CAP_SCALED`] 永远推不出来——没实现 `iv_caps` 的解码器一律按"只会整解"对待，
/// 与今天的实际行为一致；要给它们点亮缩放能力，必须真去实现并声明。
///
/// ★ `CAP_REGION`（**v6**）是**可以**推出来的：`iv_render_region` 符号在不在**就是**
/// "会不会区域渲染"这件事本身，与 `iv_render_raw` → [`CAP_RAW_PARAM`] 同性质
/// （有证据，不是"没测当 PASS"）。
///
/// 两点刻意的设计：
/// - **声明 0 不当"什么都不支持"处理**。`iv_render` 是必需导出，能加载就说明整解能力
///   在，声明 0 只可能是解码器写错了；此时回落推定，别让一个笔误把图永久变成打不开。
/// - **声明值与既有证据取并集**。漏点是笔误，不该因此丢能力（`is_vector` 与
///   `iv_render_raw` 都在结构体/符号表里有硬证据）。反过来声明里的**新位一律采信**：
///   宿主没法从外部验证解码器真会缩放，谎报属实现错误，由规范 §八自查清单逐条对账，
///   不在这里加一层"猜你想干什么"的兜底。
pub(super) fn caps_of(
    declared: Option<u32>,
    is_vector: bool,
    has_render_raw: bool,
    has_render_region: bool,
) -> u32 {
    use crate::core::abi::caps_from_legacy_v6;
    let derived = caps_from_legacy_v6(is_vector, has_render_raw, has_render_region);
    match declared {
        None | Some(0) => derived,
        Some(bits) => bits | derived,
    }
}

/// 加载 DLL、协商 abi、读自述与符号。宿主解码与设置页共用。
///
/// 外层再包一层 [`invoke`]：`Library::new` 会执行解码器 `DllMain`/静态初始化，
/// 提前返回时又会 `FreeLibrary`（`DllMain(DLL_PROCESS_DETACH)`）——这两处以前
/// 不在任何 SEH/panic 闸门里，一个坏 `DllMain` 能直接带走宿主。
pub(super) fn inspect(path: &Path) -> Inspect {
    // 上次加载没有正常走完（进程崩溃 / SEH 硬故障，Drop 没跑）：本次直接跳过。
    // 这是防"坏 DLL 每次启动都在同一处把宿主带走"的唯一持久化闸门。
    if marker::stale(path) {
        log::warn!(
            "解码器 {} 上次加载时崩溃，本次跳过（清除标记后可重试）",
            path.display()
        );
        return Inspect::Skipped;
    }
    match invoke(|| inspect_inner(path)) {
        Ok(v) => v,
        Err(_) => {
            // SEH 硬故障不会跑 Rust 析构，标记会留下；本次也按"已跳过"收口，
            // 让设置页当场能显示/清除，而不是只报一句泛化错误。
            if marker::stale(path) {
                log::warn!(
                    "解码器 {} 加载时发生硬故障，本次跳过（标记已留下）",
                    path.display()
                );
                Inspect::Skipped
            } else {
                Inspect::Error("解码器加载/卸载时崩溃，已隔离".into())
            }
        }
    }
}

pub(super) fn inspect_inner(path: &Path) -> Inspect {
    // 必须在 `Library::new` 之前落盘：`DllMain` / 静态初始化就崩在这里面。
    // 正常返回（含普通错误 / panic）由 guard 的 Drop 清掉；崩溃才留下。
    let _loading = marker::begin(path);
    harden_search_path();

    // SAFETY: 绝对路径加载；`lib` 在 `Inspect::Ok` 里被持有到函数指针失效为止。
    let lib = match unsafe { Library::new(path) } {
        Ok(l) => l,
        Err(e) => return Inspect::Error(format!("DLL 加载失败（{}）：{e}", path.display())),
    };

    // 版本协商：必须在取其它符号之前先问，版本不符就别再调用别的导出。
    let abi: unsafe extern "C" fn() -> u32 = match unsafe { lib.get(b"iv_abi_version\0") } {
        Ok(s) => *s,
        Err(e) => return Inspect::Error(format!("缺少导出 iv_abi_version：{e}")),
    };
    let got = match invoke(|| unsafe { abi() }) {
        Ok(v) => v,
        Err(_) => return Inspect::Error("iv_abi_version 调用时 panic".into()),
    };
    if !abi_compatible(got) {
        return Inspect::AbiMismatch { got };
    }

    let decoder_info: unsafe extern "C" fn() -> *const IvDecoderInfo =
        match unsafe { lib.get(b"iv_decoder_info\0") } {
            Ok(s) => *s,
            Err(e) => return Inspect::Error(format!("缺少导出 iv_decoder_info：{e}")),
        };
    let probe: unsafe extern "C" fn(*const u8, usize, *const c_char) -> i32 =
        match unsafe { lib.get(b"iv_probe\0") } {
            Ok(s) => *s,
            Err(e) => return Inspect::Error(format!("缺少导出 iv_probe：{e}")),
        };
    let render: unsafe extern "C" fn(
        *const u8,
        usize,
        *const IvRenderReq,
        *mut u8,
        usize,
        *mut IvRenderOut,
        *const c_char,
    ) -> i32 = match unsafe { lib.get(b"iv_render\0") } {
        Ok(s) => *s,
        Err(e) => return Inspect::Error(format!("缺少导出 iv_render：{e}")),
    };
    let last_error: unsafe extern "C" fn(*mut c_char, usize) -> i32 =
        match unsafe { lib.get(b"iv_last_error\0") } {
            Ok(s) => *s,
            Err(e) => return Inspect::Error(format!("缺少导出 iv_last_error：{e}")),
        };
    // 可选导出：iv_render_raw（裸数据参数化解码）。缺席 = 该解码器不支持裸数据语义，
    // 照常作为普通解码器使用（规范 §四：新增导出不 bump ABI）。
    let render_raw: Option<
        unsafe extern "C" fn(
            *const u8,
            usize,
            u32,
            u32,
            u32,
            *const IvRenderReq,
            *mut u8,
            usize,
            *mut IvRenderOut,
        ) -> i32,
    > = unsafe { lib.get(b"iv_render_raw\0") }.ok().map(|s| *s);

    // 可选导出：iv_render_region（**ABI v6** 区域渲染）。缺席 = 这枚 DLL 不会"只渲
    // 看得见的那一块"——宿主对它走整幅路线（今天的行为，不算错）。
    let render_region: Option<crate::core::abi::IvRenderRegionFn> =
        unsafe { lib.get(b"iv_render_region\0") }.ok().map(|s| *s);
    // 可选导出：iv_caps（解码能力位图）。缺席就下面按既有字段推定。
    let caps_sym: Option<unsafe extern "C" fn() -> u32> =
        unsafe { lib.get(b"iv_caps\0") }.ok().map(|s| *s);
    // Optional export: iv_metadata (extensible key/value table). Missing = legacy DLL.
    let metadata_sym: Option<unsafe extern "C" fn() -> *const IvMetadata> =
        unsafe { lib.get(b"iv_metadata\0") }.ok().map(|s| *s);
    let metadata = match metadata_sym {
        Some(f) => invoke(|| unsafe { read_metadata(f()) }).unwrap_or_default(),
        None => Vec::new(),
    };
    // 调它拿声明值；panic 就当没声明（推定路径），不把能力位图的读取变成故障源。
    let declared_caps = match caps_sym {
        Some(f) => invoke(|| unsafe { f() }).ok(),
        None => None,
    };

    let desc = match invoke(|| unsafe {
        let p = decoder_info();
        if p.is_null() {
            return None;
        }
        let i = &*p;
        Some(Descriptor {
            id: cstr(i.id),
            display_name: cstr(i.display_name),
            description: cstr(i.description),
            extensions: parse_ext_list(&cstr(i.extensions)),
            is_vector: i.is_vector != 0,
            has_alpha: i.has_alpha != 0,
            caps: caps_of(
                declared_caps,
                i.is_vector != 0,
                render_raw.is_some(),
                render_region.is_some(),
            ),
            abi: got,
            metadata,
        })
    }) {
        Ok(Some(d)) => d,
        Ok(None) => return Inspect::Error("iv_decoder_info 返回空指针".into()),
        Err(_) => return Inspect::Error("iv_decoder_info 调用时 panic".into()),
    };
    if desc.id.is_empty() {
        return Inspect::Error("解码器自述 id 为空".into());
    }

    Inspect::Ok {
        desc,
        lib,
        sym: Symbols {
            probe,
            render,
            last_error,
            render_raw,
            render_region,
        },
    }
}

// ── 宿主 ───────────────────────────────────────────────────────────

/// 一个已加载并校验过版本的解码器。持有 `Library` 到 drop，保证函数指针有效。
pub(super) struct Loaded {
    pub(super) desc: Descriptor,
    pub(super) sym: Symbols,
    /// 这一枚还在不在**当前目录清单**里（升级/卸载/换目录后置 `false`）。
    ///
    /// ★ 为什么不直接把条目摘掉：摘掉 = `drop` = `FreeLibrary` = 跑解码器的
    /// `DllMain(DLL_PROCESS_DETACH)`——那是宿主第一次在**会话中途**卸库，坏解码器的
    /// detach 路径没人验过，而收益只是省一点内存。置停用就够了：旧文件哪怕被别的
    /// 句柄锁着删不掉，也不会再被任何一次路由选中；真正的卸库留到进程结束。
    pub(super) active: bool,
    /// 按声明序**最后** drop：库留到全结构最后才卸。`sym` 是裸函数指针（无
    /// Drop，卸库与否都只是一段地址），但把「库最后卸」立成字段序，后人给
    /// 本型加会碰库的析构时也不会踩悬垂。
    pub(super) _lib: Option<Library>,
}

impl Drop for Loaded {
    fn drop(&mut self) {
        if let Some(lib) = self._lib.take() {
            // `FreeLibrary` 会跑解码器的 `DllMain(DLL_PROCESS_DETACH)`；坏解码器同样能在这里
            // 访问违规。拦不住就泄漏模块，不让一次卸载把宿主带走。
            let _ = invoke(|| drop(lib));
        }
    }
}
