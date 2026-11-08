//! 认领冲突的仲裁：谁该接这个扩展名 / 这个内容。
//!
//! 判据只用解码器自己带的三样证据（认领的扩展名、能力位图、用户开关），**不维护
//! 任何 `ext → 解码器 id` 的手写优先级表**——那份表删掉一个解码器就悄悄失效。
use super::*;

// ── 解码失败的两分类：换解码器还有没有救 ────────────────────────────

/// [`Host::decode_with`] 的失败分类（宿主内部用，不出 ABI）。
///
/// 区分它只有一个目的：ext 命中的解码器解不动时，**要不要再花一次嗅探兜底**。
/// 「内容不认 / 本解码器不支持」值得（.jxl 改名 .png 的典型场景：core 按 ext
/// 入围但不认内容，或认得魔数却没编译进对应解码器，真正能解的解码器还没被问过）；
/// 硬失败不值得（损坏的 50MP RAW 换个解码器再全解一遍，只是把几秒钟的失败重复一次）。
#[derive(Debug)]
pub(super) enum Fail {
    /// 解码器的 probe 明确拒绝了内容（魔数不认识）——换解码器还有救。
    NotClaimed(String),
    /// 内容认了但本解码器缺能力（`ERR_UNSUPPORTED`，如 feature 未编入）——换解码器还有救。
    Unsupported(String),
    /// 损坏 / 过大 / panic——换解码器多半也没用。
    Hard(String),
}

impl Fail {
    pub(super) fn into_msg(self) -> String {
        match self {
            Fail::NotClaimed(m) | Fail::Unsupported(m) | Fail::Hard(m) => m,
        }
    }

    /// 失败后是否值得走嗅探兜底（换一个解码器再试）。
    pub(super) fn worth_sniff(&self) -> bool {
        matches!(self, Fail::NotClaimed(_) | Fail::Unsupported(_))
    }
}

// ── 选解码器：认领冲突的仲裁 ────────────────────────────────────────

/// 把 `(置信度, DLL 路径)` 排成兜底榜单：**分数降序，平手保留入参顺序**，分数 ≤0
/// （解码器不认）不进榜。
///
/// 平手必须确定（同一个目录、同一份文件，每次落同一个解码器），否则"改名文件由谁解"
/// 会随加载顺序漂移——确定性由**调用方先把路径按字典序排好**提供（`sniff_ranked` 已排），
/// 这里只做一次稳定排序，不再重复约定。
pub(super) fn rank_candidates(scores: Vec<(i32, PathBuf)>) -> Vec<(i32, PathBuf)> {
    let mut ranked: Vec<(i32, PathBuf)> = scores.into_iter().filter(|(c, _)| *c > 0).collect();
    ranked.sort_by_key(|item| std::cmp::Reverse(item.0));
    ranked
}

/// 候选筛选的一项（**只借不藏**，不持 `Library`，便于测试）。
///
/// 宿主选解码器的判据只有这几样：认领哪些扩展名、是否被用户关掉、是否进了黑名单、
/// **会哪种解码类型**（`caps`，契约可选导出 `iv_caps` 的位图，缺席时由 `caps_of` 推定）。
/// `id` **只**服务用户显式首选的按名反查（[`preferred_candidate`]）——自动仲裁
/// [`select_candidate`] 依旧不读它，理由见其注释里「为什么不该手写优先级表」的说明。
///
/// ★ m102：字段全是**借来的**。这结构以前是 owned（`PathBuf` + `String` + `Vec<String>`），
/// 而它每枚解码器造一份、每次解码都要造一整排：12 枚解码器（合计声明 83 条扩展名）一次选解码器
/// = **133 次堆分配 + 12 把停用锁**。同进程配对探针（测试档 opt-level=1，n=5000×3 交替取
/// 最小，三轮 16.9/16.9/26.9 µs → 9.9/10.1/13.1 µs，见 本仓历史记录）⇒ 省 40–51%。
/// 判据一个字没改，只是不再抄一份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Cand<'a> {
    pub(super) key: &'a PathBuf,
    pub(super) id: &'a str,
    pub(super) extensions: &'a [String],
    pub(super) disabled: bool,
    pub(super) blacklisted: bool,
    pub(super) caps: u32,
}

/// 从候选里挑一个能接这个请求的解码器；**按给定顺序取第一个**（调用方已排好序）。
///
/// # `require_caps`：宿主对「同一扩展名多人认领」的仲裁手段
///
/// `.raw` 既是相机 RAW 的一种后缀（Minolta/Panasonic…），也是最常见的裸像素 dump
/// 后缀——**光看后缀与内容都可能分不出来**。两条路因此按**解码类型**分开，判据是
/// 解码器自己声明的能力位图（`abi::CAP_*`），不是解码器名字或目录序：
///
/// | 请求 | `require_caps` | `.raw` 的实际走向 |
/// |------|:---:|---|
/// | 常规打开（魔数嗅探） | `0`（无要求） | 相机 RAW 解码器（path 字典序在前） |
/// | 参数窗给了宽/高/布局 | `CAP_RAW_PARAM` | 只有声明了裸数据参数化解码的实现入围 |
/// | 只要一张 240×180 缩略图 | `CAP_SCALED` | 只会整解的解码器全部出局；声明名单由各 DLL 的 `iv_caps()` 自报，宿主不维护；没人入围时调用方按 `0` 再来一次 |
///
/// 这样**两个解码器谁都不用改自述**去抢 `.raw`：用户在参数窗里明确报了宽高布局，
/// 就只可能是裸数据；常规双击打开则交给魔数说话（相机 RAW 认不出来就报未知格式，
/// 而不是把二进制当像素解成一屏雪花——后者比"打不开"更糟）。
///
/// 为什么不写一张 `ext → 解码器 id` 的优先级表：那是一份必须人工维护的第二清单，
/// 删掉/改名一个解码器就悄悄失效；判据「会不会这种解码」是解码器自己带的能力，
/// 天然跟解码器同生共死。
pub(super) fn select_candidate<'a, 'b>(
    ext: &str,
    cands: &'a [Cand<'b>],
    require_caps: u32,
) -> Option<&'a Cand<'b>> {
    cands.iter().find(|c| {
        if c.disabled || c.blacklisted {
            return false;
        }
        if c.caps & require_caps != require_caps {
            return false;
        }
        c.extensions.iter().any(|e| e == ext)
    })
}

/// 用户显式首选（`decoders_preferred`）的纯判定：`pref` 指到的候选若**仍可用**
/// （认领 `ext`、未停用/拉黑、满足 `require_caps`）就返回它；否则 `None` 落回
/// [`select_candidate`] 的自动仲裁。
///
/// 独立成纯函数只为一件事——**可测**：不碰目录表与全局态，四条失效路径
/// （停用 / 拉黑 / 不认领该后缀 / 缺请求所需能力位）都能用 `Vec<Cand>` 直接构造。
pub(super) fn preferred_candidate<'a, 'b>(
    ext: &str,
    require_caps: u32,
    pref: Option<&str>,
    cands: &'a [Cand<'b>],
) -> Option<&'a Cand<'b>> {
    let pref = pref?;
    cands.iter().find(|c| {
        c.id == pref
            && !c.disabled
            && !c.blacklisted
            && c.extensions.iter().any(|e| e == ext)
            && c.caps & require_caps == require_caps
    })
}
