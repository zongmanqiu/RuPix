//! 本层的单测：仲裁判据、自述解析、失败分类——都是纯函数，不碰真 DLL。
//!
//! 需要真 DLL 的用例在 `tests/`（集成）与 `.temp/` 探针里，不在这里。

use super::dllmeta::*;
use super::output::*;
use super::select::*;
use super::*;

/// 版本区间判定：v5 起「相等才收」改为「区间即收」——v5 之后的 DLL 不再因
/// 宿主升级而集体作废（这是 ABI 前向兼容的核心承诺，见 [`MIN_ABI_VERSION`]）。
#[test]
fn abi_version_is_range_checked_not_equal() {
    assert!(abi_compatible(5), "冻结点本身必须可载");
    assert!(abi_compatible(IV_ABI_VERSION));
    assert!(!abi_compatible(4), "v4 因改名断点，拒载");
    assert!(!abi_compatible(3), "v3 签名时代，拒载");
    assert!(!abi_compatible(1));
    // 宿主将来升到 v6 时，v5 的老 DLL 依然要能载（模拟未来宿主版本）。
    assert!((MIN_ABI_VERSION..=6).contains(&5));
}

/// ★ **v6 必须兼容 v5**（用户 2026-10-28 拍板）：v6 是**加法**——新增可选导出
/// `iv_render_region`、新结构体 `IvRegionReq`、新能力位 `CAP_REGION`，
/// v5 的每一个导出签名与结构体布局**逐字节未动**。
///
/// 这条断言就是那句承诺的机器凭据：**下界不动**（仍是 5），所以 v5 的老 DLL
/// 不必重编就能被 v6 宿主收载、照常工作；上界跟着契约抬到 6。
#[test]
fn abi_v6_keeps_v5_loadable() {
    assert_eq!(IV_ABI_VERSION, 6);
    assert_eq!(
        MIN_ABI_VERSION, 5,
        "下界不许抬：抬了等于让 v5 的老 DLL 集体作废"
    );
    assert!(abi_compatible(5), "v5 老 DLL 必须照常收载");
    assert!(abi_compatible(6));
    assert!(!abi_compatible(7), "高于宿主的未来导出，拒载");
    // 能力位是新加的，旧位不许挪（挪号等于让老解码器谎报能力）。
    assert_eq!(crate::core::abi::CAP_VECTOR, 1 << 5);
    assert_eq!(crate::core::abi::CAP_REGION, 1 << 6);
}

/// 扩展名表：分号分隔、去点、转小写、丢空段。
#[test]
fn ext_list_parses_and_normalizes() {
    assert_eq!(parse_ext_list("QOI"), vec!["qoi"]);
    assert_eq!(parse_ext_list("jxl; .brn ;"), vec!["jxl", "brn"]);
    assert_eq!(parse_ext_list(""), Vec::<String>::new());
}

/// 错误码文案必须**可区分**（规范 §四 的红线：不许整个吞成"打不开"）。
#[test]
fn error_messages_are_distinguishable() {
    let a = describe_code("qoi", ERR_UNRECOGNIZED, "");
    let b = describe_code("qoi", ERR_CORRUPT, "");
    let c = describe_code("qoi", ERR_TOO_LARGE, "");
    assert_ne!(a, b);
    assert_ne!(b, c);
    assert!(a.contains("未知格式"));
    assert!(c.contains("过大") || c.contains("超出"));
}

/// panic 错误码要给"已隔离"文案，与别的错误区分开（规范 §四）。
#[test]
fn panic_code_says_isolated() {
    let m = describe_code("qoi", ERR_PANIC, "boom");
    assert!(m.contains("已隔离"), "{m}");
}

/// safe-mode 默认关；开启后 `try_decode` 直接不做事（返回 None），
/// `list_decoders` 也不得加载任何 DLL（DllMain 也不跑）。
#[test]
fn safe_mode_disables_decoders() {
    assert!(!safe_mode());
    set_safe_mode(true);
    let mut h = Host::new();
    assert!(
        h.try_decode(Path::new("x.qoi"), "qoi", 100, 100, 0, false)
            .is_none(),
        "safe-mode 下不得进入解码器路径"
    );
    assert!(
        list_decoders().is_empty(),
        "safe-mode 下不得加载 DLL 读自述"
    );
    set_safe_mode(false);
}

/// 没有解码器认领的扩展名返回 None（调用方据此报"无法打开"错误），不误报。
#[test]
fn unknown_ext_returns_none() {
    let mut h = Host::new();
    assert!(h
        .try_decode(Path::new("x.unknownext"), "unknownext", 10, 10, 0, false)
        .is_none());
}

/// 宿主侧 `invoke` 必须把解码器里逸出的 panic 挡在 ABI 边界内（纵深防御那一层）。
///
/// 注意：Rust panic 由 `catch_unwind` 接住；Windows 上真正的访问违规（SEH）
/// 由 C 隔离层 `sv_seh_call` 接住——两者都降级为 `Err`，见 [`seh_run`]。
#[test]
fn invoke_catches_panics() {
    assert!(invoke(|| 42).is_ok());
    assert!(invoke(|| -> i32 { panic!("boom") }).is_err());
}

/// SEH 访问违规也要被 `invoke` 拦下，而不是连进程一起带走（仅 Windows 有意义）。
#[cfg(windows)]
#[test]
fn invoke_catches_access_violation() {
    // 写一个地址空间顶部的 wild 指针（非 0）：触发 EXCEPTION_ACCESS_VIOLATION，
    // 被 sv_seh_call 的 `__except` 拦下。故意避开 `*(0 as *mut T)`——新版 Rust 的
    // `deref_nullptr` lint 会在编译期把空指针解引用判为硬错。write_volatile 保证
    // 编译器不会把这次写优化掉。
    let r = invoke(|| -> i32 {
        unsafe {
            let p = usize::MAX as *mut u8;
            std::ptr::write_volatile(p, 1);
        }
        0
    });
    assert!(r.is_err(), "访问违规必须被降级为 Err（不崩进程）");
}

/// 爆栈同样是**解码器故障**，不是进程终点（审计 V7）：以前 `STATUS_STACK_OVERFLOW` 不在
/// 吞并表里 → `EXCEPTION_CONTINUE_SEARCH` → 整个窗口无声消失，还不算三振。
/// 现在它归 `SehFault`，并且连着爆两次都拦得住（shim 在 `__except` 里调过
/// `_resetstkoflw` 重新武装守护页，否则第二次不再是栈溢出而是踩到栈外的内存）。
///
/// 实跑会打印两行 `thread ... has overflowed its stack`——那是 **Rust std 自己的**
/// 栈溢出处理器（它先于我们的过滤器跑，重画守护页后原样再抛一次），不是我们的失败：
/// 断言照样通过、进程照样活着。
#[cfg(windows)]
#[test]
fn stack_overflow_is_caught_as_decoder_fault_not_an_abort() {
    #[inline(never)]
    fn burn(n: u32) -> u32 {
        // 每帧 16 KiB：递归 100_000 层远超任何线程栈，必然爆。
        let mut pad = [7u8; 16 * 1024];
        pad[0] = n as u8;
        let own = std::hint::black_box(&pad)[0] as u32;
        if n == 0 {
            own
        } else {
            burn(n - 1).wrapping_add(own)
        }
    }
    for _ in 0..2 {
        let r = invoke(|| -> i32 { burn(100_000) as i32 });
        let e = r.expect_err("爆栈必须被降级为 Err，不能让进程没了");
        assert!(
            !crate::platform::seh::is_media_fault(e.as_ref()),
            "爆栈是解码器自己的故障，该计三振"
        );
    }
    // 护栏没被这次爆栈拆掉：之后的正常调用照常工作。
    assert_eq!(invoke(|| 42).unwrap_or(0), 42);
}

/// 三振口径是"**连续**"：中间成功一次就复位（审计 V10）。不复位的话，一张坏图在一次
/// 动作里烧掉 2 次（嗅一次 + 解一次），会话内累计 3 次就把平时好好的解码器整场拉黑，
/// 而文档、日志、UI 文案说的都是"连续 3 次"。
#[test]
fn strikes_are_consecutive_not_cumulative() {
    let mut h = Host::new();
    h.penalize("qoi");
    h.penalize("qoi");
    assert_eq!(h.strikes.get("qoi"), Some(&2));
    assert!(!h.blacklist.contains("qoi"), "2 次还不该拉黑");
    h.clear_strikes("qoi");
    assert_eq!(h.strikes.get("qoi"), None);
    // 复位后再来 2 次仍不到 3 —— 累计口径会在这里直接拉黑，那就是 V10 的病灶。
    h.penalize("qoi");
    h.penalize("qoi");
    assert_eq!(h.strikes.get("qoi"), Some(&2));
    assert!(!h.blacklist.contains("qoi"));
    h.penalize("qoi");
    assert!(h.blacklist.contains("qoi"), "连续第 3 次才拉黑");
}

/// 介质故障（`EXCEPTION_IN_PAGE_ERROR`）不计三振（审计 V8）：网络盘掉线、U 盘被拔
/// 不该冤枉解码器，更不该让日志把介质问题说成"解码器已隔离"。
#[cfg(windows)]
#[test]
fn media_fault_costs_no_strike() {
    use crate::platform::seh::{MediaFault, SehFault};
    let mut h = Host::new();
    h.charge_fault("qoi", &MediaFault);
    assert_eq!(h.strikes.get("qoi"), None, "介质故障不进三振");
    assert!(!h.blacklist.contains("qoi"));
    h.charge_fault("qoi", &SehFault);
    assert_eq!(h.strikes.get("qoi"), Some(&1), "真故障照计");
}

/// 分配长度按契约来：`bytes_needed + icc_len`（规范 §十一）。过大申报的 ICC 必须在
/// **分配前**被拒——审计 V5 的成因正是"闸门只看了像素段"。
#[test]
fn render_alloc_follows_the_icc_contract() {
    let mut info = IvRenderOut {
        bytes_needed: 4 * 8 * 8, // 8×8 RGBA8
        actual_w: 8,
        actual_h: 8,
        ..Default::default()
    };
    assert_eq!(
        out_segments(&info).unwrap(),
        (256, 0),
        "icc_len=0 与 v3 行为一致"
    );
    info.icc_len = 1234;
    assert_eq!(out_segments(&info).unwrap(), (256, 1234));
    info.icc_len = ICC_CAP + 1;
    assert!(out_segments(&info).is_err(), "荒谬的 icc_len 不许拿去分配");
    // 自洽判定：100×100 最宽也就 16 字节/像素（PF_RGBA_F32），报 160 MB = 契约违约。
    info.icc_len = 0;
    info.actual_w = 100;
    info.actual_h = 100;
    info.bytes_needed = 100 * 100 * 16;
    assert!(out_segments(&info).is_ok(), "上限之内要放行");
    info.bytes_needed = 40_000_000 * 4;
    assert!(
        out_segments(&info).is_err(),
        "小图申报顶格巨图的缓冲要被拒——绝对闸门看不见这种错位"
    );
    // 下界：0 字节会通过“不大于”闸，宿主会拿 dangling 指针进渲染阶段。
    info.actual_w = 8;
    info.actual_h = 8;
    info.bytes_needed = 0;
    assert!(
        out_segments(&info).is_err(),
        "0 长度像素段必须拒绝，不能把空 Vec 的指针递给解码器"
    );
    info.bytes_needed = 8 * 8;
    assert!(out_segments(&info).is_ok(), "每像素 1 字节是最低合法值");
}

/// 被用户关闭的 id 不得被 `candidate_for_ext` 选中。
#[test]
fn disabled_ids_are_skipped() {
    set_disabled(vec!["qoi".to_string()]);
    assert!(is_disabled("qoi"));
    assert!(!is_disabled("jxl"));
    set_disabled(Vec::<String>::new());
    assert!(!is_disabled("qoi"));
}

/// 用户显式首选（`decoders_preferred`）的全局表：灌入/查询/清空往返。
#[test]
fn set_preferred_round_trips() {
    let mut m = BTreeMap::new();
    m.insert("jpg".to_string(), "jpeg".to_string());
    set_preferred(m);
    assert_eq!(preferred_for("jpg"), Some("jpeg".to_string()));
    assert_eq!(preferred_for("png"), None, "没设过的后缀 = 走自动仲裁");
    set_preferred(BTreeMap::new());
    assert_eq!(preferred_for("jpg"), None);
}

/// 测试夹具的"持有版"候选。
///
/// `Cand` 从 m102 起**只借不藏**（生产侧不再为每次选解码器抄一整排 `PathBuf`/`String`），
/// 用例里那些字符串就得有个地方住——这一型就是那个住处。
/// ★ `view()` 逐字段构造 `Cand`：生产体将来加一个字段，这里当场编译不过，
///   不会出现"夹具少一个字段还全绿"的漂移。
#[derive(Debug, Clone)]
struct Own {
    key: PathBuf,
    id: String,
    extensions: Vec<String>,
    disabled: bool,
    blacklisted: bool,
    caps: u32,
}

impl Own {
    fn view(&self) -> Cand<'_> {
        Cand {
            key: &self.key,
            id: &self.id,
            extensions: &self.extensions,
            disabled: self.disabled,
            blacklisted: self.blacklisted,
            caps: self.caps,
        }
    }
}

/// 造一枚候选（默认不停用、不拉黑）。
fn own(key: &str, id: &str, exts: &[&str], caps: u32) -> Own {
    Own {
        key: PathBuf::from(key),
        id: id.to_string(),
        extensions: exts.iter().map(|e| e.to_string()).collect(),
        disabled: false,
        blacklisted: false,
        caps,
    }
}

/// 把一排持有版借成 `select_candidate` 要的切片。**入参必须先落在局部变量上**——
/// 借来的东西不能从临时值里长出来。
fn views(v: &[Own]) -> Vec<Cand<'_>> {
    v.iter().map(Own::view).collect()
}

/// 用户显式首选的纯判定：命中即定；四条失效路径（停用 / 不认领该后缀 /
/// 缺请求所需能力位）任一命中都静默落回自动——首选是**覆盖**不是**冒险**。
#[test]
fn preferred_candidate_overrides_auto_and_falls_back_on_stale() {
    let jpg = |id: &str| own(&format!("decoders/{id}_0.1.0.dll"), id, &["jpg"], CAP_FULL);
    let all = [jpg("aaa"), jpg("mmm"), jpg("zzz")];
    let cands = views(&all);
    // 自动 = 路径序第一个认领者；显式首选可以越过它。
    assert_eq!(
        select_candidate("jpg", &cands, 0).map(|c| c.id),
        Some("aaa")
    );
    assert_eq!(
        preferred_candidate("jpg", 0, Some("zzz"), &cands).map(|c| c.id),
        Some("zzz")
    );
    // 没设过首选 = 自动。
    assert_eq!(preferred_candidate("jpg", 0, None, &cands), None);
    // 停用的首选失效 → 落回自动。
    let off = [
        jpg("aaa"),
        Own {
            disabled: true,
            ..jpg("mmm")
        },
        jpg("zzz"),
    ];
    let with_off = views(&off);
    assert_eq!(
        preferred_candidate("jpg", 0, Some("mmm"), &with_off).map(|c| c.id),
        None
    );
    // 不认领该后缀的首选失效（首选按 id 记，不按后缀记，跨后缀残留要兜住）。
    assert_eq!(preferred_candidate("png", 0, Some("aaa"), &cands), None);
    // 参数化请求（CAP_RAW_PARAM）：首选没声明这个能力 → 失效。
    let no = [jpg("rawpx")];
    let raw_no = views(&no);
    assert_eq!(
        preferred_candidate("jpg", CAP_RAW_PARAM, Some("rawpx"), &raw_no),
        None
    );
    let ok = [own(
        "decoders/rawpx_0.1.0.dll",
        "rawpx",
        &["jpg"],
        CAP_FULL | CAP_RAW_PARAM,
    )];
    let raw_ok = views(&ok);
    assert_eq!(
        preferred_candidate("jpg", CAP_RAW_PARAM, Some("rawpx"), &raw_ok).map(|c| c.id),
        Some("rawpx")
    );
}

/// `.raw` 的仲裁：两个解码器（相机 RAW / 裸像素）都认领时，**请求类型**决定归属。
/// 这是 `select_candidate` 存在的全部理由——常规打开走魔数，参数化走 `CAP_RAW_PARAM`。
#[test]
fn raw_extension_conflict_resolved_by_request_kind() {
    let all = [
        own("decoders/raw_0.3.0.dll", "raw", &["raw", "cr2"], CAP_FULL),
        own(
            "decoders/rawpx_0.1.0.dll",
            "rawpx",
            &["raw", "rgba"],
            CAP_FULL | CAP_RAW_PARAM,
        ),
    ];
    let cands = views(&all);
    let hit =
        |require_caps: u32| select_candidate("raw", &cands, require_caps).map(|c| c.key.clone());
    // 常规打开：字典序在前的相机 RAW 解码器（它靠魔数认，认不出就报未知格式）。
    assert_eq!(
        hit(0).unwrap().file_stem().unwrap(),
        "raw_0.3.0",
        "常规解码不该被无魔数的 rawpx 劫持"
    );
    // 参数化解码：只有声明了 CAP_RAW_PARAM 的 rawpx 入围，哪怕它排在后面。
    assert_eq!(
        hit(CAP_RAW_PARAM).unwrap().file_stem().unwrap(),
        "rawpx_0.1.0",
        "给了宽高布局就该走参数化解码器"
    );
    // 只有 rawpx 认领的扩展名：两种请求都落到它。
    let only = views(&all[1..]);
    assert_eq!(
        select_candidate("rgba", &only, 0).map(|c| c.key.clone()),
        hit(CAP_RAW_PARAM)
    );
}

/// 黑名单与被停用的解码器在任何请求类型下都不入围（拉黑的是它的 id，认了手指就是这样）。
#[test]
fn blacklisted_and_disabled_never_selected() {
    let base = own("p.dll", "raw", &["raw"], CAP_FULL | CAP_RAW_PARAM);
    let dead = [Own {
        blacklisted: true,
        ..base.clone()
    }];
    let dead = views(&dead);
    assert!(select_candidate("raw", &dead, CAP_RAW_PARAM).is_none());
    let off = [Own {
        disabled: true,
        ..base
    }];
    let off = views(&off);
    assert!(select_candidate("raw", &off, 0).is_none());
}

/// 没有解码器声明 `CAP_RAW_PARAM` 时，参数化请求必须老实返回"没人接"，而不是
/// 把请求丢给一个不支持的解码器换来一句"target lacks iv_render_raw"。
#[test]
fn no_render_raw_capability_means_no_candidate() {
    let all = [own("core.dll", "core", &["raw"], CAP_FULL)];
    let cands = views(&all);
    assert!(select_candidate("raw", &cands, CAP_RAW_PARAM).is_none());
    assert!(select_candidate("raw", &cands, 0).is_some());
}

/// 缩放路由（`CAP_SCALED`）的两面：**没人声明时得到"空答案"而不是错误**，
/// 有人声明时它接缩放、整解请求照旧归字典序第一个。
///
/// 第二句是关键：能力位图只**加路**，不抢既有路。加一位 `CAP_SCALED` 之后，
/// 今天所有走整解的图仍然走原来那枚解码器——升级不该改变没升级的人的行为。
#[test]
fn scaled_route_prefers_declared_and_otherwise_falls_back_to_full() {
    let plain = own("decoders/jpeg_0.1.0.dll", "jpeg", &["jpg"], CAP_FULL);
    let scaler = Own {
        key: PathBuf::from("decoders/jpeg-hw_0.1.0.dll"),
        id: "jpeg_hw".to_string(),
        caps: CAP_FULL | CAP_SCALED,
        ..plain.clone()
    };
    // 只有一枚整解解码器（= 现役 12 枚的形状）：缩放请求无人入围。
    let one = [plain.clone()];
    let only = views(&one);
    assert_eq!(select_candidate("jpg", &only, CAP_SCALED), None);
    // 同一张表按"无要求"再问一次，立刻拿回整解解码器——"降级"的实际形状。
    assert_eq!(
        select_candidate("jpg", &only, 0).map(|c| c.id),
        Some("jpeg")
    );
    // 真有人声明了：缩放归它，整解照旧归字典序第一个。
    let both_own = [plain, scaler];
    let both = views(&both_own);
    assert_eq!(
        select_candidate("jpg", &both, CAP_SCALED).map(|c| c.id),
        Some("jpeg_hw")
    );
    assert_eq!(
        select_candidate("jpg", &both, 0).map(|c| c.id),
        Some("jpeg")
    );
}

/// 兜底榜单的排序契约：**置信度降序、0 分不进榜、平手保持入参顺序**。
///
/// 这三条都是"冠军搬家"之后才会咬人的地方：core 报 50 让位给冠军的 100（分层）、
/// 两个冠军同报 100 时必须每次落同一个解码器（确定性由调用方 `sniff_ranked` 先把
/// DLL 按路径排好序来保证——这里只做一次稳定排序，所以平手保留入参先后）、
/// 不认的解码器不能被试一遍（否则一份坏 JPEG 会把每个 DLL 都解一次）。
#[test]
fn ranked_rescue_orders_by_confidence_and_drops_zero_claims() {
    let p = |s: &str| PathBuf::from(s);
    // 入参按路径字典序（真实调用方 sniff_ranked 就是这个前提）。
    let got = rank_candidates(vec![
        (50, p("core.dll")),
        (0, p("qoi.dll")),
        (100, p("raw.dll")),
        (-3, p("thumbdoc.dll")),
        (100, p("tiff.dll")),
    ]);
    assert_eq!(
        got,
        vec![
            (100, p("raw.dll")),
            (100, p("tiff.dll")),
            (50, p("core.dll")),
        ]
    );
    assert!(rank_candidates(vec![(0, p("qoi.dll"))]).is_empty());
}

/// 能力位图的缺席推定：只用**真有的证据**换算，`CAP_SCALED` 绝不凭空点亮。
/// 本用例钉的是**没有 `iv_caps`** 的那条旧路；导出 `iv_caps` 的解码器走"声明 ∪ 证据"分支。
#[test]
fn caps_without_export_derive_from_legacy_only() {
    use crate::core::abi::{CAP_FULL, CAP_RAW_PARAM, CAP_SCALED, CAP_VECTOR};
    assert_eq!(caps_of(None, false, false, false), CAP_FULL);
    assert_eq!(caps_of(None, true, false, false), CAP_FULL | CAP_VECTOR);
    assert_eq!(caps_of(None, false, true, false), CAP_FULL | CAP_RAW_PARAM);
    for iv in [false, true] {
        for rr in [false, true] {
            assert_eq!(
                caps_of(None, iv, rr, false) & CAP_SCALED,
                0,
                "缩放能力没有既有证据，凭空点亮就是拿未测当 PASS"
            );
        }
    }
}

/// 声明 0 = 笔误（`iv_render` 是必需导出），回落推定而不是把这张图判死。
#[test]
fn caps_declared_zero_falls_back() {
    use crate::core::abi::{CAP_FULL, CAP_RAW_PARAM};
    assert_eq!(caps_of(Some(0), false, false, false), CAP_FULL);
    assert_eq!(
        caps_of(Some(0), false, true, false),
        CAP_FULL | CAP_RAW_PARAM
    );
}

/// 声明与既有证据取并集：漏点不该丢能力，但声明里的新位一律采信。
#[test]
fn caps_union_keeps_both_declared_and_evidenced() {
    use crate::core::abi::{CAP_FULL, CAP_RAW_PARAM, CAP_SCALED, CAP_VECTOR};
    // 只声明缩放、没点整解：整解由推定补齐。
    assert_eq!(
        caps_of(Some(CAP_SCALED), false, false, false),
        CAP_FULL | CAP_SCALED
    );
    // 声明漏了矢量，但 `is_vector` 是硬证据 → 并集补上。
    assert_eq!(
        caps_of(Some(CAP_FULL), true, true, false),
        CAP_FULL | CAP_VECTOR | CAP_RAW_PARAM
    );
}

/// 矢量后缀口径（m152 v2）：**成员级声明优先**，其次才按 DLL 级能力位。
///
/// 这条口径是被一次真实断链逼出来的：合并档 `pro` 实现了 `iv_export_svg` 却先忘了
/// 声明 `CAP_VECTOR`，补上之后若还按 DLL 级优先，pro 自述的 jp2/dcm/exr 等**位图**
/// 成员会被一并当成矢量后缀——那会让这些格式走矢量路线（导出失败→回退），
/// 白白多一次 IPC。
#[test]
fn vector_exts_prefer_member_level_declaration() {
    use crate::core::abi::CAP_FULL;

    let base = Descriptor {
        id: "pro".into(),
        display_name: "pro".into(),
        description: String::new(),
        extensions: vec!["jp2".into(), "dcm".into(), "wmf".into()],
        is_vector: false,
        has_alpha: true,
        caps: CAP_FULL | crate::core::abi::CAP_VECTOR,
        abi: 5,
        metadata: Vec::new(),
    };

    // ① 成员级声明存在 → 只认它列出的那几个。
    let mut d = base.clone();
    d.metadata = vec![("vector_exts".into(), "wmf;wmz".into())];
    assert_eq!(
        super::registry::vector_exts_of(&d),
        vec!["wmf".to_string(), "wmz".to_string()],
        "成员级声明必须压过 DLL 级口径，否则位图成员会被误判成矢量"
    );

    // ② 没有成员级声明、但声明了矢量能力 → 全部自述后缀都是矢量。
    assert_eq!(
        super::registry::vector_exts_of(&base),
        vec!["jp2".to_string(), "dcm".to_string(), "wmf".to_string()]
    );

    // ③ 两者都没有 → 不是矢量解码器。
    let mut d = base.clone();
    d.caps = CAP_FULL;
    assert!(super::registry::vector_exts_of(&d).is_empty());
}

/// ★ 矢量**只有一条渲染路径**（2026-10-28 收口）：解码器认领的矢量后缀**一律**由
/// 它自己按目标尺寸光栅化（宪章 §5.2），宿主不再用自己的 resvg 渲染它们。
///
/// 曾经这里判的是"宿主渲染 vs 解码器自渲染"两档（靠探测 `iv_export_svg` 符号 +
/// metadata `vector_render=target` 覆盖）。那条"宿主渲染"路线已整条退役——
/// 留着它就是**同一个格式挂着两条渲染路径**，换机器/换 DLL 版本/换字体栈时
/// 两边的像素可能不同，那正是"同一文件跨机同结果"这条立宪理由要防的事。
///
/// 现在剩下的唯一一维是"**能不能按区域重渲**"（ABI v6）：它只看硬证据。
#[test]
fn vector_region_capability_needs_hard_evidence() {
    use crate::core::abi::{CAP_FULL, CAP_REGION};

    let exts = vec!["wmf".to_string()];
    // ① 实现了 `iv_render_region` ⇒ 能按区域重渲。
    assert!(
        super::registry::region_capable(&exts, true, CAP_FULL),
        "有区域符号的矢量解码器必须能被发区域请求"
    );
    // ② 没有符号但声明了 `CAP_REGION` ⇒ 也能（能力位与符号互为印证）。
    assert!(super::registry::region_capable(
        &exts,
        false,
        CAP_FULL | CAP_REGION
    ));
    // ③ 两者都缺 ⇒ **不能**：宿主对它只能整幅请求（发区域请求 = 内容错误图）。
    assert!(
        !super::registry::region_capable(&exts, false, CAP_FULL),
        "没有区域通道的矢量解码器绝不能被发区域请求"
    );
    // ④ 不是矢量后缀 ⇒ 不能（压根不进矢量路线）。
    assert!(!super::registry::region_capable(
        &[],
        true,
        CAP_FULL | CAP_REGION
    ));
}

/// metadata `region_exts`：成员级精确子集（合并档里只有一部分矢量成员有视口）。
///
/// 这条与 [`vector_region_capability_needs_hard_evidence`] 合起来才是完整的判据：
/// 能力位/符号回答"这枚 DLL 有没有区域通道"，`region_exts` 回答"它的哪些后缀能走"。
#[test]
fn region_exts_narrows_the_vector_list_per_member() {
    let meta = |k: &str, v: &str| (k.to_string(), v.to_string());

    // 缺席 ⇒ None（宿主按整枚矢量发区域请求）。
    assert_eq!(super::registry::region_exts_of(&[]), None);
    assert_eq!(
        super::registry::region_exts_of(&[meta("vector_exts", "dwg;dxf")]),
        None
    );

    // 有声明 ⇒ 只认列出的那些（小写、去点、去空项）。
    assert_eq!(
        super::registry::region_exts_of(&[meta("region_exts", ".DWG; dxf ;;pdf")]),
        Some(vec![
            "dwg".to_string(),
            "dxf".to_string(),
            "pdf".to_string()
        ])
    );

    // 声明了空表 ⇒ Some(空)：这枚 DLL 有区域符号但没有可用视角的后缀。
    assert_eq!(
        super::registry::region_exts_of(&[meta("region_exts", " ; ")]),
        Some(Vec::new())
    );
}

/// DLL 文件名只解版本：ABI 不在名字里（那是 `iv_abi_version()` 的自述）。
#[test]
fn parse_dll_version_handles_convention_and_garbage() {
    // 标准产物名。
    assert_eq!(
        parse_dll_version("jxl_0.3.0.dll"),
        Some("0.3.0".to_string())
    );
    // id 含连字符也不影响：只认"最后一个下划线之后、以数字开头"的那段。
    assert_eq!(
        parse_dll_version("my-cool_1.2.3.dll"),
        Some("1.2.3".to_string())
    );
    // 无 .dll 后缀也能解。
    assert_eq!(parse_dll_version("core_0.3.0"), Some("0.3.0".to_string()));
    // 不合约定的一律 None，详情窗留空不编假数据。
    assert_eq!(parse_dll_version("random.dll"), None);
    assert_eq!(parse_dll_version("foo_abi.dll"), None);
    assert_eq!(parse_dll_version("foo_.dll"), None);
}

// 缓存桩不持有 DLL；真实 refresh_catalog 扫描私有目录，损坏文件仍走真实 inspect。
struct AuditCatalogDir {
    path: PathBuf,
    previous: Option<std::ffi::OsString>,
}

impl AuditCatalogDir {
    fn new(name: &str) -> Self {
        let path = crate::core::paths::test_scratch_root()
            .join(format!("host-audit-{name}-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let previous = std::env::var_os("RUPIX_DECODERS_DIR");
        std::env::set_var("RUPIX_DECODERS_DIR", &path);
        Self { path, previous }
    }

    fn cache(&self, host: &mut Host, file: &str, id: &str, version: &str) -> PathBuf {
        let path = self.path.join(file);
        std::fs::write(&path, b"cached stub").unwrap();
        host.loaded.insert(
            path.clone(),
            Loaded {
                desc: Descriptor {
                    id: id.into(),
                    display_name: id.into(),
                    description: String::new(),
                    extensions: vec!["audit-image".into()],
                    is_vector: false,
                    has_alpha: false,
                    caps: CAP_FULL,
                    abi: MIN_ABI_VERSION,
                    metadata: vec![("version".into(), version.into())],
                },
                sym: Symbols {
                    probe: audit_probe,
                    render: audit_render,
                    last_error: fake_last_error,
                    render_raw: None,
                    render_region: None,
                },
                _lib: None,
                active: true,
            },
        );
        path
    }
}

impl Drop for AuditCatalogDir {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var("RUPIX_DECODERS_DIR", previous);
        } else {
            std::env::remove_var("RUPIX_DECODERS_DIR");
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

unsafe extern "C" fn audit_probe(_: *const u8, _: usize, _: *const c_char) -> i32 {
    0
}
unsafe extern "C" fn audit_render(
    _: *const u8,
    _: usize,
    _: *const IvRenderReq,
    _: *mut u8,
    _: usize,
    _: *mut IvRenderOut,
    _: *const c_char,
) -> i32 {
    ERR_UNRECOGNIZED
}

#[test]
fn audit_refresh_selects_self_reported_newest_even_if_old_file_remains() {
    let dir = AuditCatalogDir::new("versions");
    let mut host = Host::new();
    // 文件名刻意撒谎；同 id 的身份与版本以自述为准，0.10 必须大于 0.9。
    let old = dir.cache(&mut host, "a_99.0.dll", "audit-version", "0.9.0");
    let new = dir.cache(&mut host, "z_0.1.dll", "audit-version", "0.10.0");
    host.refresh_catalog();
    assert!(old.exists(), "旧文件删不掉也不能成为活动候选");
    assert_eq!(host.candidate_for_ext("audit-image", 0), Some(new.clone()));
    // 条目**留在表里**只是不再选用：摘掉条目就等于在会话中途 FreeLibrary，那不做。
    assert_eq!(host.loaded.len(), 2, "被淘汰的旧版不摘条目，只置停用");
    assert!(host.loaded[&new].active);
    assert!(!host.loaded[&old].active);
    request_rescan();
    host.refresh_catalog();
    assert_eq!(host.candidate_for_ext("audit-image", 0), Some(new));
}

#[test]
fn audit_refresh_legacy_versions_use_filename_and_equal_versions_are_stable() {
    let dir = AuditCatalogDir::new("legacy-versions");
    let mut host = Host::new();
    dir.cache(&mut host, "a_0.9.0.dll", "audit-legacy", "");
    let winner = dir.cache(&mut host, "b_0.10.0.dll", "audit-legacy", "");
    dir.cache(&mut host, "c_0.10.0.dll", "audit-legacy", "");
    for loaded in host.loaded.values_mut() {
        loaded.desc.metadata.clear();
    }
    host.refresh_catalog();
    assert_eq!(
        host.loaded.values().filter(|l| l.active).count(),
        1,
        "同 id 只能有一份可用"
    );
    assert_eq!(host.candidate_for_ext("audit-image", 0), Some(winner));
}

#[test]
fn audit_refresh_deactivates_missing_paths_but_keeps_them_loaded() {
    let dir = AuditCatalogDir::new("removed");
    let mut host = Host::new();
    let gone = dir.cache(&mut host, "a.dll", "audit-gone", "1.0");
    let keep = dir.cache(&mut host, "b.dll", "audit-keep", "1.0");
    std::fs::remove_file(&gone).unwrap();
    host.refresh_catalog();
    assert!(
        host.loaded.contains_key(&gone),
        "目录里没了也不摘条目（摘了=卸库）"
    );
    assert!(!host.loaded[&gone].active, "目录里没了的必须不再选用");
    assert!(host.loaded[&keep].active, "同目录里还在的那份不受影响");
    assert_eq!(host.candidate_for_ext("audit-image", 0), Some(keep.clone()));
    // 目录整个读不动（网络盘瞬断 / U 盘被拔）= **沿用现有清单**：一次 I/O 失败
    // 不许把全部解码器判成"已不在"，否则所有格式静默变成"无人认领"。
    std::fs::remove_dir_all(&dir.path).unwrap();
    request_rescan();
    host.refresh_catalog();
    assert!(
        host.loaded[&keep].active,
        "读不到目录时不得停用现有解码器（媒体故障不是解码器的错）"
    );
    assert_eq!(host.candidate_for_ext("audit-image", 0), Some(keep));
    // 目录回来了：先前"消失"的那份要能恢复选用（同一路径不重新 LoadLibrary）。
    std::fs::create_dir_all(&dir.path).unwrap();
    std::fs::write(dir.path.join("a.dll"), b"cached stub").unwrap();
    request_rescan();
    host.refresh_catalog();
    assert!(host.loaded[&gone].active, "文件回到目录里应恢复选用");
}

#[test]
fn audit_refresh_broken_upgrade_keeps_old_and_failure_blacklist_state() {
    let dir = AuditCatalogDir::new("broken");
    let mut host = Host::new();
    let old = dir.cache(&mut host, "audit_0.9.0.dll", "audit-fallback", "0.9.0");
    let broken = dir.path.join("audit_0.10.0.dll");
    std::fs::write(&broken, b"not a DLL").unwrap();
    host.strikes.insert("audit-fallback".into(), 2);
    host.blacklist.insert("other-blocked".into());
    host.refresh_catalog();
    assert_eq!(host.candidate_for_ext("audit-image", 0), Some(old));
    assert!(host.failed.contains_key(&broken));
    assert_eq!(host.strikes["audit-fallback"], 2);
    assert!(host.blacklist.contains("other-blocked"));
    host.blacklist.insert("audit-fallback".into());
    request_rescan();
    host.refresh_catalog();
    assert!(host.failed.contains_key(&broken));
    assert_eq!(host.candidate_for_ext("audit-image", 0), None);
    assert!(host.blacklist.contains("audit-fallback"));
}

// ── 区域渲染（ABI v6）────────────────────────────────────────────────

static AUDIT_REGION_WRITES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "C" fn audit_oversized_region(
    _data: *const u8,
    _len: usize,
    req: *const IvRegionReq,
    out: *mut u8,
    _cap: usize,
    info: *mut IvRenderOut,
    _ext: *const c_char,
) -> i32 {
    if !out.is_null() {
        // 红臂最多分配 256 MiB + 1；立即失败，绝不写入或再派生整幅像素。
        AUDIT_REGION_WRITES.fetch_add(1, Ordering::SeqCst);
        return ERR_CORRUPT;
    }
    let bytes_only = unsafe { (*req).frame_index == 1 };
    unsafe {
        (*info).actual_w = if bytes_only {
            4096
        } else {
            (MAX_PIXELS + 1) as u32
        };
        (*info).actual_h = if bytes_only { 4096 } else { 1 };
        (*info).bytes_needed = if bytes_only {
            MAX_PIXELS * 4 + 1
        } else {
            MAX_PIXELS + 1
        };
        (*info).pixel_format = PF_GRAY8;
        (*info).bit_depth = 8;
    }
    ERR_OK
}

fn audit_region_budget_case(frame_index: u32) {
    AUDIT_REGION_WRITES.store(0, Ordering::SeqCst);
    let req = IvRegionReq {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
        target_w: 1,
        target_h: 1,
        budget_bytes: (MAX_PIXELS * 4) as u32,
        flags: 0,
        frame_index,
        page_index: 0,
    };
    let result =
        super::decode::region_roundtrip(audit_oversized_region, fake_last_error, b"x", &req, None);
    assert!(result.is_err());
    assert_eq!(
        AUDIT_REGION_WRITES.load(Ordering::SeqCst),
        0,
        "超预算的查询必须在分配/第二次 render 之前被拒绝"
    );
}

#[test]
fn audit_region_rejects_pixel_budget_before_render() {
    audit_region_budget_case(0);
}

#[test]
fn audit_region_rejects_byte_budget_before_render() {
    audit_region_budget_case(1);
}

/// 假的 `iv_render_region`：不碰真 DLL 也能把"两阶段 + 像素归一"跑一遍。
///
/// 行为按契约：查询阶段（`out == NULL`）只回填 `info`；写入阶段填像素。
/// `MODE` 用一个静态计数器记录这次是第几次调用（1 = 查询，2 = 写入）。
static REGION_CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 假解码器回填的画布宽高（0 = 用请求里的 target）。
static REGION_FAKE_SIZE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "C" fn fake_region(
    _data: *const u8,
    _len: usize,
    req: *const crate::core::abi::IvRegionReq,
    out: *mut u8,
    out_cap: usize,
    info: *mut crate::core::abi::IvRenderOut,
    _ext: *const std::ffi::c_char,
) -> i32 {
    REGION_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let r = unsafe { &*req };
    let (w, h) = {
        let fake = REGION_FAKE_SIZE.load(std::sync::atomic::Ordering::SeqCst);
        if fake > 0 {
            (fake, fake)
        } else {
            (r.target_w, r.target_h)
        }
    };
    let i = unsafe { &mut *info };
    i.actual_w = w;
    i.actual_h = h;
    i.bytes_needed = (w as u64) * (h as u64) * 4;
    i.pixel_format = crate::core::abi::PF_RGBA8;
    i.bit_depth = 8;
    i.orientation = 1;
    i.already_oriented = 0;
    i.frame_count = 1;
    i.page_count = 1;
    i.icc_len = 0;
    if out.is_null() {
        return crate::core::abi::ERR_OK; // 查询阶段：不写像素
    }
    // 写入阶段：把缓冲涂成一个可辨认的颜色（RGBA = 0x11,0x22,0x33,0xff）。
    let need = (w as usize) * (h as usize) * 4;
    if out_cap < need {
        return crate::core::abi::ERR_BUFFER_TOO_SMALL;
    }
    let buf = unsafe { std::slice::from_raw_parts_mut(out, need) };
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&[0x11, 0x22, 0x33, 0xff]);
    }
    crate::core::abi::ERR_OK
}

/// 假的 `iv_last_error`：永远返回空串。
unsafe extern "C" fn fake_last_error(_buf: *mut std::ffi::c_char, _len: usize) -> i32 {
    1
}

/// 区域渲染两阶段：先查询（不写像素）→ 再写入；宿主按 `bytes_needed` 一次性分配。
///
/// 这条钉住 ABI v6 的核心形状——**查询阶段不许写像素**（写了就是越界），
/// 以及"宿主分配、解码器只写"的铁律。
#[test]
fn region_roundtrip_queries_then_writes() {
    REGION_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    REGION_FAKE_SIZE.store(0, std::sync::atomic::Ordering::SeqCst);
    let req = crate::core::abi::IvRegionReq {
        x: 10.0,
        y: 20.0,
        w: 40.0,
        h: 30.0,
        target_w: 80,
        target_h: 60,
        budget_bytes: (crate::core::pixels::MAX_PIXELS * 4) as u32,
        flags: 0,
        frame_index: 0,
        page_index: 0,
    };
    let (bitmap, aw, ah, icc, _info) =
        super::decode::region_roundtrip(fake_region, fake_last_error, b"abc", &req, None)
            .expect("区域往返应当成功");
    assert_eq!(
        REGION_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "必须恰好两阶段：查询一次 + 写入一次"
    );
    assert_eq!((aw, ah), (80, 60), "画布尺寸必须按请求回填");
    assert_eq!((bitmap.width, bitmap.height), (80, 60));
    assert_eq!(bitmap.rgba.len(), 80 * 60 * 4);
    assert_eq!(
        &bitmap.rgba[0..4],
        &[0x11, 0x22, 0x33, 0xff],
        "像素必须来自解码器写入"
    );
    assert!(icc.is_empty(), "没有 ICC 就不该有尾段");
}

/// 解码器报 0 尺寸 ⇒ 宿主必须当失败处理（不能拿 0×0 去分配）。
#[test]
fn region_roundtrip_rejects_zero_sized_output() {
    REGION_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    REGION_FAKE_SIZE.store(0, std::sync::atomic::Ordering::SeqCst);
    let req = crate::core::abi::IvRegionReq {
        x: 0.0,
        y: 0.0,
        w: 40.0,
        h: 30.0,
        target_w: 0,
        target_h: 0,
        budget_bytes: 0,
        flags: 0,
        frame_index: 0,
        page_index: 0,
    };
    // target = 0 时假解码器回填 0×0 —— 正是要拦下的那种"解码器没算尺寸"。
    let e = super::decode::region_roundtrip(fake_region, fake_last_error, b"abc", &req, None)
        .expect_err("0 尺寸必须判失败");
    assert!(e.contains("0 尺寸"), "错误文案要说清是尺寸问题：{e}");
}

/// ★ 画布恒定：**放大到 4000%（密度 40×）时，画布仍是一屏大小**。
///
/// 这是"矢量放大无损"的宿主侧保证：宿主给的区域在图像坐标里随放大**变小**
/// （看得见的那一块），画布 = 区域 × 密度 ≈ 视口像素 ⇒ 像素预算恒定、倍率再高也顶得住。
#[test]
fn region_target_keeps_canvas_viewport_sized_at_extreme_zoom() {
    // 100%：区域 = 视口（图像像素），密度 1 → 画布 = 视口。
    let (w, h, k) = super::decode::region_target((1920.0, 1080.0), 1.0);
    assert_eq!((w, h), (1920, 1080));
    assert!((k - 1.0).abs() < 1e-3);
    // 4000%（密度 40×）：同一块视口在图像坐标里只剩 1/40 → 画布仍是 1920×1080。
    let (w, h, k) = super::decode::region_target((1920.0 / 40.0, 1080.0 / 40.0), 40.0);
    assert_eq!((w, h), (1920, 1080), "放大 4000% 时画布必须仍是一屏大小");
    assert!((k - 40.0).abs() < 1e-2, "密度不许被悄悄压低");
    // 画布不许超过硬顶（极端视口 + 极高密度 ⇒ 降密度而不是失败）。
    let (w, h, k) = super::decode::region_target((1920.0, 1080.0), 40.0);
    assert!(w as u64 <= crate::core::pixels::MAX_EDGE as u64);
    assert!(h as u64 <= crate::core::pixels::MAX_EDGE as u64);
    assert!(k < 40.0, "撞硬顶时降密度（宁可略糊也不失败）");
}

/// ★ 矢量路线的 `natural` 必须是**文档自然尺寸**，不是"这一次渲染出来的画布"。
///
/// 2026-10-28 真机报障（WMF 放大后显示不全）的根因：宿主拿渲染尺寸当文档尺寸，
/// 而区域请求的坐标系（ABI v6：x/y/w/h 单位 = 文档自然像素）与画布几何都吃这个值
/// ⇒ 宿主要的那块与真图那块不是同一块。
///
/// 这条钉住**补查判据**本身（纯函数，不碰 DLL）：
/// - 走"按可视区域重渲"的**解码器自渲染矢量** ⇒ 要补查；
/// - 宿主内置的 svg/svgz ⇒ 不补（那条路由根本不经过解码器层）；
/// - 位图 ⇒ 不补（`natural` 本来就是原图尺寸）；
/// - 这一次已经"要原始尺寸" ⇒ 不补（回填的已经是原尺寸）。
#[test]
fn doc_natural_query_only_for_self_rendered_region_vectors() {
    use crate::core::formats::test_override;
    // 覆盖槽：矢量后缀 = {wmf, wmz}，可按区域重渲的 = {wmf, wmz}（测试机上没有真 DLL）。
    let _guard = test_override::set(&["wmf", "wmz"], &["wmf", "wmz"]);
    let p = std::path::Path::new("x.wmf");
    assert!(
        super::decode::needs_doc_natural(Some("wmf"), p, 557, 561),
        "自渲染矢量必须补查文档自然尺寸"
    );
    assert!(
        super::decode::needs_doc_natural(Some("WMF"), p, 557, 561),
        "后缀大小写不该影响判据"
    );
    assert!(
        super::decode::needs_doc_natural(None, p, 557, 561),
        "嗅探路径（ext = None）要从文件后缀取判据"
    );
    assert!(
        !super::decode::needs_doc_natural(Some("wmf"), p, 0, 0),
        "已经要原始尺寸时补查是白跑"
    );
    assert!(
        !super::decode::needs_doc_natural(Some("png"), std::path::Path::new("x.png"), 557, 561),
        "位图的 natural 本来就是原图尺寸，不补查"
    );
    assert!(
        !super::decode::needs_doc_natural(Some("svg"), std::path::Path::new("x.svg"), 557, 561),
        "宿主内置 svg 走自己的 resvg，根本不经过解码器层"
    );
}

// ── 按 DLL 串行（m229，`dll_lock` 模块头）──────────────────────────────────
//
// 这四条钉的是"契约里那句串行现在有机器把关"：两个线程抢同一枚 DLL 必须排队、
// 同一线程重入不许自锁、占着不放要给可读裁决而不是陪卡、不同 DLL 不许互相牵连。
// 用假的 `Z:\` 路径：锁表只按路径分格，碰不到真 DLL。

/// 两线程抢同一枚 DLL ⇒ 严格排队（B 的"进"排在 A 的"出"之后）。
#[test]
fn dll_lock_serializes_two_threads_on_the_same_dll() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let key = std::path::PathBuf::from(r"Z:\rupix-dll-lock\same\a.dll");
    let log = Arc::new(Mutex::new(Vec::<&'static str>::new()));

    let inner = Arc::clone(&log);
    let k = key.clone();
    let first = std::thread::spawn(move || {
        let _g = super::dll_lock::enter_within(
            &k,
            "bridge",
            Duration::from_secs(2),
            super::dll_lock::Class::Urgent,
        )
        .expect("第一笔应先拿到锁");
        inner.lock().unwrap().push("A-in");
        std::thread::sleep(Duration::from_millis(80));
        inner.lock().unwrap().push("A-out");
    });

    // 让第一笔先跑起来，否则下面那句可能抢在它前面（这条用例要的是"排队"，不是赛跑）。
    std::thread::sleep(Duration::from_millis(15));
    {
        let _g = super::dll_lock::enter_within(
            &key,
            "bridge",
            Duration::from_secs(2),
            super::dll_lock::Class::Urgent,
        )
        .expect("第二笔应等到第一笔松开");
        log.lock().unwrap().push("B-in");
    }
    first.join().expect("第一笔线程不该崩");

    assert_eq!(
        *log.lock().unwrap(),
        vec!["A-in", "A-out", "B-in"],
        "同一枚 DLL 的两笔必须串行，不许交叠"
    );
}

/// 同一线程重入不抢锁（`decode_with` → `query_natural_only` 就是这种嵌套）。
#[test]
fn dll_lock_is_reentrant_on_one_thread() {
    use std::time::Duration;

    let key = std::path::PathBuf::from(r"Z:\rupix-dll-lock\reentrant\b.dll");
    // 预算给到 5 ms：真去抢锁的话必然超时，用例就会在第二层失败而不是吊死。
    let outer = super::dll_lock::enter_within(
        &key,
        "doc",
        Duration::from_millis(5),
        super::dll_lock::Class::Urgent,
    )
    .expect("外层应先拿到");
    assert!(super::dll_lock::held_here(&key));
    let inner = super::dll_lock::enter_within(
        &key,
        "doc",
        Duration::from_millis(5),
        super::dll_lock::Class::Urgent,
    )
    .expect("同线程重入不该抢锁");
    drop(inner);
    assert!(super::dll_lock::held_here(&key), "内层走了，外层还在");
    drop(outer);
    assert!(!super::dll_lock::held_here(&key), "外层走了要清干净");
    // 清干净之后别的线程/下一次调用还能拿到。
    super::dll_lock::enter_within(
        &key,
        "doc",
        Duration::from_millis(50),
        super::dll_lock::Class::Urgent,
    )
    .expect("释放后应可再次获取");
}

/// 占着不放 ⇒ 有界等待后交一条可读裁决，不是无限等。
#[test]
fn dll_lock_gives_up_with_a_readable_verdict() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let key = std::path::PathBuf::from(r"Z:\rupix-dll-lock\busy\c.dll");
    let (tx, rx) = mpsc::channel();
    let k = key.clone();
    let holder = std::thread::spawn(move || {
        let _g = super::dll_lock::enter_within(
            &k,
            "slow",
            Duration::from_secs(2),
            super::dll_lock::Class::Urgent,
        )
        .expect("占锁方应先拿到");
        tx.send(()).expect("通知主线程");
        std::thread::sleep(Duration::from_millis(200));
    });
    rx.recv_timeout(Duration::from_secs(2))
        .expect("等占锁方就位");

    let started = Instant::now();
    let err = match super::dll_lock::enter_within(
        &key,
        "waiter",
        Duration::from_millis(60),
        super::dll_lock::Class::Urgent,
    ) {
        Ok(_) => panic!("锁被占着，不该拿到"),
        Err(e) => e,
    };
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(55),
        "不许立刻放弃（要真等一会儿），实际 {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(1),
        "有界等待必须真的收口，实际 {waited:?}"
    );
    assert!(err.contains("waiter"), "裁决要点名是哪个解码器：{err}");
    assert!(err.contains("占用"), "裁决要说人话：{err}");
    holder.join().expect("占锁方线程不该崩");
}

/// 不同 DLL 各解各的（粒度是"这一枚"，不是全局）。
#[test]
fn dll_lock_does_not_couple_two_different_dlls() {
    use std::time::Duration;

    let a = std::path::PathBuf::from(r"Z:\rupix-dll-lock\apart\a.dll");
    let b = std::path::PathBuf::from(r"Z:\rupix-dll-lock\apart\b.dll");
    let _ga = super::dll_lock::enter_within(
        &a,
        "one",
        Duration::from_secs(1),
        super::dll_lock::Class::Urgent,
    )
    .expect("拿 A");
    let gb = super::dll_lock::enter_within(
        &b,
        "two",
        Duration::from_millis(20),
        super::dll_lock::Class::Urgent,
    )
    .expect("拿 B 不该等 A");
    drop(gb);
    assert!(super::dll_lock::held_here(&a));
    assert!(!super::dll_lock::held_here(&b));
}

/// 先来先得：后到的 C 不许插队到已排队 40 ms 的 B 前面。
///
/// 这条钉的是真机抓到的反例（`wait=5313.7ms`）：旧实现用 `try_lock` 轮询，持有者一笔解完
/// 立刻回来抢下一笔（周转 <1 ms），而等待者睡在 4 ms 的间隔里 ⇒ 输家连输 5 笔。
#[test]
fn dll_lock_serves_waiters_in_arrival_order() {
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    let key = std::path::PathBuf::from(r"Z:\rupix-dll-lock\fair\a.dll");
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let holder = super::dll_lock::enter_within(
        &key,
        "holder",
        Duration::from_secs(2),
        super::dll_lock::Class::Urgent,
    )
    .expect("持锁方先拿");

    let (tx, rx) = mpsc::channel();
    let b = {
        let key = key.clone();
        let order = Arc::clone(&order);
        std::thread::spawn(move || {
            tx.send(()).expect("报告即将入队");
            let _g = super::dll_lock::enter_within(
                &key,
                "B",
                Duration::from_secs(5),
                super::dll_lock::Class::Urgent,
            )
            .expect("B 应拿到");
            order.lock().unwrap().push("B");
        })
    };
    rx.recv_timeout(Duration::from_secs(2)).expect("等 B 就位");
    // 60 ms 足够 B 走完"领票 + 入队"；留得太短会让这条用例在慢机器上偶发假红。
    std::thread::sleep(Duration::from_millis(60));
    let c = {
        let key = key.clone();
        let order = Arc::clone(&order);
        std::thread::spawn(move || {
            let _g = super::dll_lock::enter_within(
                &key,
                "C",
                Duration::from_secs(5),
                super::dll_lock::Class::Urgent,
            )
            .expect("C 应拿到");
            order.lock().unwrap().push("C");
        })
    };
    std::thread::sleep(Duration::from_millis(40));
    drop(holder);
    b.join().expect("B 线程不该崩");
    c.join().expect("C 线程不该崩");

    assert_eq!(
        *order.lock().unwrap(),
        vec!["B", "C"],
        "先入队者先拿锁（旧实现会退化成 B、C 赛跑）"
    );
}

/// 后台预取不许挡在用户请求前面（m237）。
///
/// 同一条队里：预取先到（`Background`），用户请求后到（`Urgent`）。放手后**用户先拿到**，
/// 预取仍等着——这就是"用户那一笔不该为没人等的下一张排队"。
#[test]
fn dll_lock_urgent_overtakes_a_waiting_background_request() {
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    let key = std::path::PathBuf::from(r"Z:\rupix-dll-lock\overtake\a.dll");
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let holder = super::dll_lock::enter_within(
        &key,
        "holder",
        Duration::from_secs(2),
        super::dll_lock::Class::Urgent,
    )
    .expect("持锁方先拿");

    // 预取先入队，但它不该因此占住队首。
    let (bg_tx, bg_rx) = mpsc::channel();
    let bg = {
        let key = key.clone();
        let order = Arc::clone(&order);
        std::thread::spawn(move || {
            bg_tx.send(()).expect("报告即将入队");
            let _g = super::dll_lock::enter_within(
                &key,
                "prefetch",
                Duration::from_secs(5),
                super::dll_lock::Class::Background,
            )
            .expect("预取最终也应拿到（用户队列空了之后）");
            order.lock().unwrap().push("prefetch");
        })
    };
    bg_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("等预取就位");
    // 60 ms 足够它走完"领票 + 入队"；太短会让这条用例在慢机器上偶发假红。
    std::thread::sleep(Duration::from_millis(60));

    let urgent = {
        let key = key.clone();
        let order = Arc::clone(&order);
        std::thread::spawn(move || {
            let _g = super::dll_lock::enter_within(
                &key,
                "user",
                Duration::from_secs(5),
                super::dll_lock::Class::Urgent,
            )
            .expect("用户请求应拿到");
            order.lock().unwrap().push("user");
        })
    };
    std::thread::sleep(Duration::from_millis(40));
    drop(holder);
    urgent.join().expect("用户线程不该崩");
    bg.join().expect("预取线程不该崩");

    assert_eq!(
        *order.lock().unwrap(),
        vec!["user", "prefetch"],
        "后到的用户请求必须先拿到锁（只有一档的旧实现会先给预取）"
    );
}

/// 放弃的等待者必须把票摘走，否则队伍会卡在一张没人认领的票上。
#[test]
fn dll_lock_abandoned_waiter_does_not_block_the_queue() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let key = std::path::PathBuf::from(r"Z:\rupix-dll-lock\abandon\a.dll");
    // 持锁方必须在**另一条线程**上：同一线程再进一次是"重入"，压根不会去抢锁，
    // 这条用例就变成了自证重入（第一版就是这么写错的，被自己抓到）。
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = {
        let key = key.clone();
        std::thread::spawn(move || {
            let _g = super::dll_lock::enter_within(
                &key,
                "holder",
                Duration::from_secs(2),
                super::dll_lock::Class::Urgent,
            )
            .expect("持锁方先拿");
            ready_tx.send(()).expect("报告已持锁");
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        })
    };
    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("等持锁方就位");

    // B 预算 50 ms，注定超时；它的票此时排在队首。
    let err = match super::dll_lock::enter_within(
        &key,
        "B",
        Duration::from_millis(50),
        super::dll_lock::Class::Urgent,
    ) {
        Ok(_) => panic!("锁被占着，B 不该拿到"),
        Err(e) => e,
    };
    assert!(err.contains("占用"), "裁决要说人话：{err}");

    // C 排在 B 后面（B 已放弃）。若 B 的票没摘掉，C 会一路等到自己 2 s 预算耗尽。
    let started = Instant::now();
    let c = {
        let key = key.clone();
        std::thread::spawn(move || {
            let _g = super::dll_lock::enter_within(
                &key,
                "C",
                Duration::from_secs(2),
                super::dll_lock::Class::Urgent,
            )
            .expect("C 应拿到：队首那张票已经被摘掉");
        })
    };
    std::thread::sleep(Duration::from_millis(30));
    release_tx.send(()).expect("通知持锁方放手");
    c.join().expect("C 线程不该崩");
    holder.join().expect("持锁方线程不该崩");
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(1),
        "C 只该等持锁方放手（~30 ms），实际 {waited:?}"
    );
}
