//! 「可下载」清单：从仓库里那份 `decoders.json` 读出"还有哪些解码器可以装"。
//!
//! # 地址与它的意思是
//!
//! 清单有两份只读镜像——Gitee（`raw.giteeusercontent.com` 的只读 CDN，直接 200，不 redirect）
//! 与 GitHub（`raw.githubusercontent.com`）。**实际地址存在 `rupix.ini`
//! （`decoder_index_gitee` / `decoder_index_github`，2026-10-28 用户口径：联网更新的网址
//! 落进配置文件、允许用户改）**，[`crate::core::config::SourceId::index_url`] 只剩
//! "内置默认值"的身份。两份结构不同、**owner 也不是同一个用户名**（Gitee `qiuzongman` /
//! GitHub `zongmanqiu`），不能靠字符串替换得到，所以默认值各自写死。选哪个来源同时决定两件事
//! （用户 2026-10-26 方案）：去哪拉清单，以及每枚 DLL 用清单里哪一列下载地址
//! （`gitee` / `github`）；地址 owner 与来源对不上的那几枚按"该来源没有地址"处理。
//!
//! 选"跟仓库一起走"而不是"跟 release 一起走"，是为了让**上架一枚解码器不必重新发宿主**：
//! 清单由 `scripts/make_index.py` 从 DLL 自述生成、随代码 push，宿主下次读到的就是新的。
//!
//! # 清单的形状（一项一枚 DLL，顶层直接是数组）
//!
//! `id / name / version / abi / size / released / author / license / desc / formats / gitee / github`。
//! `formats` 是**分号分隔、不带点**的扩展名串——和 ABI 的 `IvDecoderInfo.extensions`
//! 同一个形状，宿主在这两处用同一套切法（`split_formats`）。
//!
//! **清单里没有哈希**（2026-11-02 用户口径）：产物全部出自我们自己的构建，不对外接受第三方
//! 改动，"这枚 DLL 是不是我们发的"由 `id` + `version` 认定就够了。这是解码器仓与宿主两侧
//! 同步的约定，改清单结构时两边一起改。
//!
//! # 这里的纪律
//!
//! - **信息不全的项直接不显示**（2026-10-26 用户口径）：清单是手写 `gitee` / `github` 的，半成品条目
//!   完全可能混进来。画一行"名称有、大小没有、按钮点了不知道下什么"比不画更糟，
//!   所以 `parse_catalog` 把认不出的项**丢掉并计数**（[`Catalog::skipped`]，
//!   只进日志，不摊到状态行——2026-10-29 用户口径：状态行只报数字）。
//!   整份清单解析失败才是错误。
//! - **id / version 要能当文件名用**：下载落盘是 `<id>_<ver>.dll`，所以这两段在解析阶段
//!   就按白名单字符校验——清单里写个 `../x` 进来，后面拼路径就成了越界写。
//! - **不认识的东西不猜**：`abi` 不在兼容区间就标出来（[`RemoteEntry::abi_ok`]），
//!   但仍然列出——用户可以看见"有这么一枚，但它要求更新的宿主"，比整份清单作废好。
//!
//! # 本地优先（m148）
//!
//! 发布包在 `<exe>/docs/decoders.json` 自带一份清单，进入「解码器」页只读它——**零网络**。
//! 因此没网、镜像被墙、甚至用户永远不联网，这一页都打得开。点「刷新」才去
//! `SourceId::index_url` 拉一份，**解析成功后再原子覆盖本地文件**；失败就保留旧清单，
//! 状态行如实说原因。切换 Gitee / GitHub **不联网**：同一份清单里两列地址都在，只换列。
//!
//! 刷新走后台线程（`start_refresh` / `poll_refresh`），理由与 DLL 下载相同：
//! WinHTTP 四个阶段的超时加起来足够把设置窗冻成"未响应"。
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use crate::core::config::SourceId;
use crate::core::json::{self, Value};

/// 同层的 ABI 区间判定（与装载时同一口径，见 `dllmeta::abi_compatible`）。
use super::dllmeta::abi_compatible;
use super::{parse_dll_version, DecoderEntry};

/// 响应体上限。真实清单 2 KB 级；1 MiB 是"这明显不是清单"的闸。
const MAX_BYTES: usize = 1 << 20;

/// 清单里的一枚解码器（**一台待下载的 DLL**，不是已安装的那份）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// 解码器标识（停用表、首选表、下载后的 DLL 命名都用它）。
    pub id: String,
    /// 解码器名称（DLL 自述的 `display_name`，**不是 DLL 文件名**）。
    pub name: String,
    pub version: String,
    /// ABI 大版本；宿主按兼容区间判定（见 [`RemoteEntry::abi_ok`]）。
    pub abi: u32,
    /// 字节数。**只用于列表显示**，不参与任何判定：识别一枚解码器靠 `id` + `version`
    /// （见 [`install_state`] 里"为什么不再比字节数"）。
    pub size: u64,
    /// 发布日期 `YYYY-MM-DD`；未发布为空串。
    pub released: String,
    /// 作者；清单没给为空串。
    pub author: String,
    /// 许可证；清单没给为空串。
    pub license: String,
    pub desc: String,
    /// 认领的扩展名（小写、不带点）。
    pub formats: Vec<String>,
    /// 各来源的下载地址：键是来源标识（`"gitee"` / `"github"`，见 [`SourceId::id`]）。
    ///
    /// 两个来源都给，切换来源时**只换一列地址**、不换清单本身（用户 2026-10-26 方案）。
    /// 缺某一来源（镜像还没同步那枚 release）就留空串——那一份在对应来源下会显示成
    /// 灰的"下载"（点不动），详情窗里仍能看到另一来源的地址。
    pub urls: std::collections::HashMap<String, String>,
}

impl RemoteEntry {
    /// 某一来源下的下载地址（空串 / 缺键 / **地址不属于该来源** = 没有地址）。
    ///
    /// `accepts_download_url` 拦的是"清单把 Gitee 的 owner 填进 GitHub 地址"这类错账：
    /// 地址看着非空、点下去 404。拦掉之后按钮会走灰色那档，原因在「!」详情窗里。
    pub fn download_url(&self, src: SourceId) -> Option<&str> {
        self.urls
            .get(src.id())
            .filter(|s| !s.is_empty() && src.accepts_download_url(s))
            .map(|s| s.as_str())
    }

    /// 某一来源下有没有可用的下载地址。
    pub fn downloadable_for(&self, src: SourceId) -> bool {
        self.download_url(src).is_some()
    }

    /// 有没有**任一**来源的可下载地址（按各来源的地址归属校验，见 [`RemoteEntry::download_url`]）。
    /// 未上架的条目也留在清单里（作者视角要看见"还差哪几枚"），但界面上不能给一个
    /// 点了没反应的按钮。
    pub fn downloadable(&self) -> bool {
        SourceId::ALL
            .iter()
            .any(|src| self.download_url(*src).is_some())
    }

    /// 本宿主能不能收这一枚（ABI 区间判定，与 [`crate::decoders::Host`] 装载时同一口径）。
    pub fn abi_ok(&self) -> bool {
        abi_compatible(self.abi)
    }

    /// 下载落盘时的文件名：`<id>_<version>.dll`。
    ///
    /// 与解码器发布产物的命名同一套约定——装进来的文件因此和手工拷进去的
    /// 长得一样，`parse_dll_version` 也就认得出它的版本。
    pub fn file_name(&self) -> String {
        format!("{}_{}.dll", self.id, self.version)
    }
}

/// 一份清单的解析结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// 信息完整的那些项。
    pub entries: Vec<RemoteEntry>,
    /// 因信息不全被丢掉的项数。只进日志，不参与列表、不上状态行。
    pub skipped: usize,
}

/// 这一枚在本机眼里是什么状态（「已下载」的判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    /// 本机没有同 id 的 DLL（没装过，或装了另一枚同名前缀的文件）。
    Absent,
    /// 本机有一份**同 id、同版本**的 DLL。
    Installed,
    /// 本机那一份**比清单旧**：界面把「下载」换成「升级」（2026-10-26 用户口径）。
    Upgradable,
    /// 本机那一份**比清单新**（作者回滚过，或本机手工放了一份更新的）。
    /// 不能给出「升级」——那会把正在用的新版本按回去。
    Ahead,
}

/// 比较两串版本号（点分段、逐段比数字）。
///
/// # 为什么要自己比，而不是比字符串
///
/// 版本号是要**排序**的东西：`"0.10.0" < "0.9.0"` 在字符串序里成立，在版本序里正好相反。
/// 文件名里的版本（`parse_dll_version`）与清单里的版本（作者手写的 `version`）来源不同、
/// 写法也可能不同（`1.0` / `1.0.0`），只有逐段数值比较才判得对。
///
/// # 规则
///
/// - 按 `.` 切段，逐段取**数字前缀**比大小，缺的段当 `0`（`1.0` == `1.0.0`）；
/// - 数字后面的尾巴按「有无」比：**没有尾巴的更大**（`1.0.0` > `1.0.0-beta`），
///   两段都有尾巴则按字符串比；
/// - 数字解析不了（超 `u64`、空段、非数字开头）当 0——版本号是给人看的短串，
///   不为它引入错误类型；比不出来的后果最多是"没认出该升级"，不会写坏文件。
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    fn parts(v: &str) -> Vec<(u64, String)> {
        v.trim()
            .trim_start_matches(['v', 'V'])
            .split('.')
            .map(|seg| {
                let n = seg.chars().take_while(char::is_ascii_digit).count();
                let num = seg[..n].parse().unwrap_or(0);
                (num, seg[n..].to_string())
            })
            .collect()
    }

    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let (na, sa) = pa.get(i).cloned().unwrap_or((0, String::new()));
        let (nb, sb) = pb.get(i).cloned().unwrap_or((0, String::new()));
        match na.cmp(&nb) {
            Ordering::Equal => {}
            other => return other,
        }
        let tail = match (sa.is_empty(), sb.is_empty()) {
            (true, true) => Ordering::Equal,
            // 发布版 > 它的预发布（`1.0.0` > `1.0.0-beta`）。
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => sa.cmp(&sb),
        };
        if tail != Ordering::Equal {
            return tail;
        }
    }
    Ordering::Equal
}

/// 「已下载」的判定：**同 id + 同版本**，两条全中就算。
///
/// 判据只认身份，不认字节（2026-11-02 用户口径，与解码器清单"没有哈希"是同一决定）：
/// 清单里的 `size` / `sha256` 在这一版全部退出判定链。理由是它们的维护成本远高于收益——
/// MSVC 每次链接都生成新 PDB GUID，同源码重编出来的 DLL 字节数与哈希都会变，于是"描一手清单
/// 数字"就成了每次发布的必修课，漏一次界面就把正常的一枚报成"大小不符"。
///
/// | 证据 | 防的是什么 |
/// |------|-----------|
/// | `id`（**DLL 自述读出来的**，不是文件名） | 下错了包 / 文件被替换成别的东西 |
/// | `version`（文件名 `<id>_<ver>.dll`） | 装的是旧版却显示"已下载" |
///
/// 真正决定"能不能用"的是装完之后 [`crate::decoders::install`] 的自述校验：DLL 得能
/// `LoadLibrary` 起来、`iv_decoder_info().id` 得和清单对得上。
///
/// # 版本不同分两种（2026-10-26）
///
/// 本机旧 → [`InstallState::Upgradable`]（界面「升级」，橙）；本机新 →
/// [`InstallState::Ahead`]（界面置灰，不给按回去的路）。判据是
/// [`version_cmp`]，不是字符串相等——见那个函数的理由。
pub fn install_state(e: &RemoteEntry, installed: &[DecoderEntry]) -> InstallState {
    use std::cmp::Ordering;

    let Some(cur) = installed.iter().find(|d| d.id == e.id) else {
        return InstallState::Absent;
    };
    // 文件名读不出版本号（不是 `<id>_<ver>.dll` 这个形状）就无从比较：当成"没有可用的
    // 同 id 版本"，让用户下这一份名字规整的——装完目录里的那份形状才是对的。
    let Some(local) = parse_dll_version(&cur.file) else {
        return InstallState::Absent;
    };
    match version_cmp(&local, &e.version) {
        Ordering::Less => InstallState::Upgradable,
        Ordering::Greater => InstallState::Ahead,
        // 数值相等、写法可能不同（`1.0` vs `1.0.0`）：同一版就是同一枚。
        Ordering::Equal => InstallState::Installed,
    }
}

/// `jpg;jpeg;jpe` → `["jpg", "jpeg", "jpe"]`。空串切成空列表（不是 `[""]`）。
pub fn split_formats(s: &str) -> Vec<String> {
    s.split(';')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}

/// `id` / `version` 能不能直接拼进文件名：只认小写字母、数字、点、连字符、下划线。
///
/// 这两段最终会拼成 `decoders/<id>_<ver>.dll` 落盘，所以这里不是"好看"的问题——
/// 放任一个 `/` 或 `..` 进来，下载就成了往任意路径写文件。
fn is_safe_stem(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !s.starts_with('.')
}

/// 过渡兼容（2026-10-26 起，2026-10-28 补仓库改名）：线上旧清单有两个历史包袱——
/// 把 Gitee 的 owner `qiuzongman` 带进了 GitHub 地址；以及仓库名还是单数 `RuPix-Decoder`。
///
/// 现实名字是 **`RuPix-Decoders`（两个镜像都复数；本地工作区的目录名是单数，两者不是一回事）**。
/// 只在这几个已知前缀上改写，其它地址原样保留；新清单本来就是对的，替换幂等。
///
/// 为什么还留着：用户机器上缓存过的旧清单会一直用到下次刷新，**下载按钮不能因为
/// 仓库改名就整片变灰**——把地址搬正比拒收更接近用户预期。
fn normalize_release_url(url: &str) -> String {
    let mut url = url.to_string();
    // 三条规则按顺序叠加（先修 owner、再修仓库名），单数 + 错 owner 的组合也能一次搬正。
    // 前缀都带仓库名，**不碰别的仓库 / 别的 owner**——猜地址比拒收更危险。
    for (from, to) in [
        (
            "https://github.com/qiuzongman/RuPix-Decoder",
            "https://github.com/zongmanqiu/RuPix-Decoder",
        ),
        (
            "https://github.com/zongmanqiu/RuPix-Decoder/",
            "https://github.com/zongmanqiu/RuPix-Decoders/",
        ),
        (
            "https://gitee.com/qiuzongman/RuPix-Decoder/",
            "https://gitee.com/qiuzongman/RuPix-Decoders/",
        ),
    ] {
        if url.starts_with(from) {
            url = format!("{to}{}", &url[from.len()..]);
        }
    }
    url
}

/// 清单里的一项 → 一枚。**信息不全就 `None`**（宁可不显示，也不显示半行）。
///
/// 必需：`id` `name` `version` `abi` `size` `formats`——这一行要画出来的东西
/// （`size` 是列表上那列"1.2 MB"，只显示），加上识别身份用的 `id` / `version`。
/// 可选：`released` `desc` `url`（缺了就是"未发布 / 无说明"，
/// 那是清单允许表达的真实状态，不是残缺）。
///
/// 清单里**多出来的键一律忽略**：旧清单带 `sha256` 也照样读得动，只是那列不再有人看。
fn entry_from(v: &Value) -> Option<RemoteEntry> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let id = text("id");
    let version = text("version");
    // 身份段要能当文件名用，否则整项作废（见 `is_safe_stem`）。
    if !is_safe_stem(&id) || !is_safe_stem(&version) {
        return None;
    }
    let name = text("name");
    if name.is_empty() {
        return None;
    }
    let abi = u32::try_from(v.get("abi").and_then(Value::as_u64)?).ok()?;
    let size = v.get("size").and_then(Value::as_u64)?;
    if size == 0 {
        return None; // 0 字节 = 没真正构建过，不是一枚能装的 DLL。
    }
    let formats = split_formats(&text("formats"));
    if formats.is_empty() {
        return None; // 认领不了任何格式的解码器，装了也没用。
    }
    Some(RemoteEntry {
        id,
        name,
        version,
        abi,
        size,
        released: text("released"),
        author: text("author"),
        license: text("license"),
        desc: text("desc"),
        formats,
        urls: {
            let mut m = std::collections::HashMap::new();
            m.insert("gitee".to_string(), normalize_release_url(&text("gitee")));
            m.insert("github".to_string(), normalize_release_url(&text("github")));
            m
        },
    })
}

/// 解析清单正文。
///
/// 只有**整份不是数组 / 不是合法 JSON** 才算错误——单项残缺一律跳过并计入
/// [`Catalog::skipped`]（理由见模块头）。错误信息带**第几项、哪个字段**：这份清单的
/// `url` 是手填的，把 `url` 写成 `link` 是完全可能的手误。
pub fn parse_catalog(text: &str) -> Result<Catalog, String> {
    let root = json::parse(text).map_err(|e| e.to_string())?;
    let arr = root
        .as_array()
        .ok_or("清单顶层应当是数组（一项一枚 DLL）")?;
    let mut out = Catalog::default();
    for item in arr {
        match entry_from(item) {
            Some(e) => out.entries.push(e),
            None => out.skipped += 1,
        }
    }
    Ok(out)
}

/// 随包清单的文件名（`<exe>/docs/decoders.json`）。
pub const INDEX_FILE: &str = "decoders.json";

/// 本地清单路径：`<exe 目录>/docs/decoders.json`。
///
/// 发布包自带这一份（`deploy.py` 从解码器发布清单复制），用户点「刷新」
/// 时才联网覆盖。取不到 exe 目录时返回 `None`（调用方按"本次会话没有本地清单"处理）。
pub fn local_path() -> Option<PathBuf> {
    Some(crate::core::paths::docs_dir()?.join(INDEX_FILE))
}

/// 读随包清单（**不联网**）。
///
/// 失败原因直接给用户看，所以三种情况分开说：文件不在 / 读不出 / 内容不是清单。
pub fn load_local() -> Result<Catalog, String> {
    let path = local_path().ok_or_else(|| "清单异常，请「刷新」".to_string())?;
    load_from(&path)
}

/// [`load_local`] 的可测形态：路径由调用方给（单测用临时目录）。
fn load_from(path: &Path) -> Result<Catalog, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        log::warn!("读本地清单 {} 失败：{e}", path.display());
        "清单异常，请「刷新」".to_string()
    })?;
    if bytes.len() > MAX_BYTES {
        log::warn!("本地清单 {} 超过 {MAX_BYTES} 字节", path.display());
        return Err("清单异常，请「刷新」".to_string());
    }
    let text = String::from_utf8(bytes).map_err(|_| "清单异常，请「刷新」".to_string())?;
    parse_catalog(&text)
}

/// 原子覆盖本地清单：`.part` 暂存 → 旧文件挪 `.old` → 新文件顶上 → 删 `.old`。
///
/// Windows 的 `rename` **不能覆盖已存在的目标**，直接 `rename(part, target)` 会失败；
/// 与 `install` 同一套换名理由。任何一步失败都把旧文件放回去——刷新失败之后，
/// 本地目录里应该还是"刷新之前那份能用清单"。
///
/// ★ 写失败 = 这次刷新失败（2026-10-29 用户口径：「要么更新失败，要么更新成功」）。
///   状态行只有这两态，所以调用方拿到 `Err` 后**丢掉刚取到的那份清单**、保留旧的，
///   原因只进日志——不另立「未能写入本地清单」这种第三态。
pub fn save_local(text: &str) -> Result<(), String> {
    let path = local_path().ok_or_else(|| "清单异常，请「刷新」".to_string())?;
    save_to(&path, text)
}

/// [`save_local`] 的可测形态：换名与回滚全在这里，路径由调用方给。
fn save_to(target: &Path, text: &str) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or_else(|| "清单路径没有父目录".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建 docs 目录失败：{e}"))?;
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "清单路径没有文件名".to_string())?;
    let part = dir.join(format!("{name}.part"));
    std::fs::write(&part, text.as_bytes())
        .map_err(|e| format!("写不进 {}（{e}）——目录是不是只读？", part.display()))?;
    let old = dir.join(format!("{name}.old"));
    // 上一次中断可能留下 `.old`：Windows 的 rename 目标已存在就失败，先清掉它。
    // 此刻我们手里有一份刚解析通过的新正文，清掉残缺的旧副本不会丢"能用的数据"。
    let _ = std::fs::remove_file(&old);
    let had_old = target.exists();
    if had_old {
        if let Err(e) = std::fs::rename(target, &old) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("旧本地清单挪不开：{e}"));
        }
    }
    if let Err(e) = std::fs::rename(&part, target) {
        let _ = std::fs::remove_file(&part);
        if had_old {
            let _ = std::fs::rename(&old, target);
        }
        return Err(format!("本地清单落盘失败：{e}"));
    }
    if had_old {
        let _ = std::fs::remove_file(&old);
    }
    Ok(())
}

/// 从某个地址拉清单正文（不落盘、不缓存）。只给后台线程与联网用例用。
///
/// ★ 地址由调用方从配置传进来（2026-10-28 用户口径：**联网更新的网址记在 ini 里、
///   允许用户改**），这里不再取 `SourceId::index_url` 常量——那只剩"内置默认值"的身份。
fn fetch(url: &str) -> Result<(Catalog, String), String> {
    let bytes = crate::platform::http::get(url, MAX_BYTES)?;
    let text = String::from_utf8(bytes).map_err(|_| "清单不是合法 UTF-8".to_string())?;
    let catalog = parse_catalog(&text)?;
    Ok((catalog, text))
}

/// 一次后台刷新的结果（工作线程 → UI 线程的唯一通道）。
///
/// ★ 只有成败两态（2026-10-29 用户口径）：取到清单但写不进本地文件 = 失败，
///   刚取到的那份一并丢掉——界面不立"取到了但没落盘"的第三态。
#[derive(Debug)]
pub struct RefreshOutcome {
    /// 这次拉的哪个来源（内容两列都带，日志要说是谁）。
    pub src: SourceId,
    pub result: Result<Catalog, String>,
}

#[derive(Default)]
struct RefreshSlot {
    running: bool,
    done: Option<RefreshOutcome>,
}

static REFRESH: Mutex<RefreshSlot> = Mutex::new(RefreshSlot {
    running: false,
    done: None,
});

/// 清单刷新失败 → 界面上那一句。原始错误只进日志。
///
/// ★ 2026-10-29 用户口径：刷新只有「更新中… / 更新失败 / 已更新」三态，
///   网络错误、地址不合法、落盘失败一律报同一句，细节进日志。
pub fn catalog_error_text() -> &'static str {
    "更新失败"
}

/// 发起刷新失败的原因：界面只分两句（仍在跑 = 「更新中…」，其余 = 「更新失败」）。
#[derive(Debug)]
pub enum BeginError {
    /// 上一下还没跑完（用户连点「刷新」）。
    Busy,
    /// 地址不合法等，根本没发出去。
    Refused(String),
}

impl BeginError {
    /// 状态行该显示的那一句。
    pub fn ui_text(&self) -> &'static str {
        match self {
            Self::Busy => "更新中…",
            Self::Refused(_) => catalog_error_text(),
        }
    }
}

impl std::fmt::Display for BeginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("已有一次刷新在跑"),
            Self::Refused(e) => f.write_str(e),
        }
    }
}

/// 起一次后台刷新：联网 → 解析 → 覆盖本地 `docs/decoders.json`。
///
/// 同一时刻只允许一次：两个刷新线程同时换同一个文件，用户看不出谁赢了。
/// 拒绝时把原因交回去（调用方只显示 [`catalog_error_text`]，原因进日志）。
///
/// 超时由 [`crate::platform::http::DEFAULT_TIMEOUT_MS`]（30 s）守。
///
/// `src` 只用来标注"这次拉的是哪个镜像"（日志）；真正决定去哪拉的是 `url`
/// ——它来自配置（用户可改），见 [`start_refresh_for`]。
pub fn start_refresh(src: SourceId, url: String) -> Result<(), BeginError> {
    // 地址是 ini 里的自由文本（用户可改）：非 http(s) 的直接拒在第 0 步，
    // 否则 WinHTTP 会把它当相对路径，错误会拐到网络层去。
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(BeginError::Refused("更新地址不是 http(s) 链接".to_string()));
    }
    {
        let mut g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
        if g.running {
            return Err(BeginError::Busy);
        }
        g.running = true;
    }
    std::thread::spawn(move || {
        // 先解析成功再落盘：拉到半截 / 格式变了，本地那份必须原样留着。
        // 落盘失败 = 整次刷新失败：刚取到的清单一并丢掉，不交付"只在内存里新"的态。
        let result = match fetch(&url) {
            Ok((cat, text)) => match save_local(&text) {
                Ok(()) => Ok(cat),
                Err(e) => {
                    log::warn!("新清单未能写入本地文件，本次刷新按失败处理：{e}");
                    Err(e)
                }
            },
            Err(e) => Err(e),
        };
        let mut g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
        g.running = false;
        g.done = Some(RefreshOutcome { src, result });
    });
    Ok(())
}

/// 按配置里当前来源**及它对应的地址**刷新（`download_source` / `decoder_index_*`
/// 两族键的直接行为读者，设置页「刷新」按钮走这里）。
///
/// 地址就是 ini 里那一条（用户可改）；解析期与设置页都已保证它非空，
/// 所以这里不做空值兜底——**空值回落内置默认只发生在两处**：解析（手改 ini 清空）
/// 与设置页（清空输入框），都不在这条刷新路径上。
pub fn start_refresh_for(c: &crate::core::config::Config) -> Result<(), BeginError> {
    let url = match c.download_source {
        SourceId::Gitee => c.decoder_index_gitee.clone(),
        SourceId::Github => c.decoder_index_github.clone(),
    };
    start_refresh(c.download_source, url)
}

/// 后台是否正在刷新（UI 用来决定"切来源时别抢状态行"）。
pub fn is_refreshing() -> bool {
    REFRESH.lock().unwrap_or_else(|e| e.into_inner()).running
}

/// 取一次刷新结果。`None` = 还没有（或已经取过了）。
///
/// 取走即清空：同一份结果只能被消费一次，避免定时器每 250 ms 重放同一句状态。
pub fn poll_refresh() -> Option<RefreshOutcome> {
    let mut g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    g.done.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实形状：一项已发布、一项未发布（`url` / `released` 空串）。
    const SAMPLE: &str = r#"[
      {
        "id": "common",
        "name": "Common Raster Set",
        "version": "0.1.0",
        "abi": 5,
        "size": 1967104,
        "released": "2026-10-26",
        "desc": "常见位图合集",
        "formats": "jpg;jpeg;jpe",
        "gitee": "https://example.com/common_0.1.0.dll",
        "github": ""
      },
      {
        "id": "pro",
        "name": "Professional Formats",
        "version": "0.1.0",
        "abi": 5,
        "size": 8266240,
        "released": "",
        "desc": "专业档",
        "formats": "jp2;j2k",
        "gitee": "",
        "github": ""
      }
    ]"#;

    #[test]
    fn parses_a_catalogue() {
        let v = parse_catalog(SAMPLE).expect("应当解析成功");
        assert_eq!(v.skipped, 0);
        assert_eq!(v.entries.len(), 2);
        let c = &v.entries[0];
        assert_eq!(c.id, "common");
        assert_eq!(c.name, "Common Raster Set");
        assert_eq!(c.size, 1_967_104);
        assert_eq!(c.formats, vec!["jpg", "jpeg", "jpe"], "分号串要切成列表");
        assert_eq!(c.file_name(), "common_0.1.0.dll", "落盘名 = <id>_<ver>.dll");
        assert!(c.downloadable());
        assert!(c.abi_ok());
        // 未发布那枚：如实列出，但不算"可下载"。
        assert_eq!(v.entries[1].id, "pro");
        assert_eq!(v.entries[1].released, "");
        assert!(!v.entries[1].downloadable());
    }

    /// 旧清单里残留的 `sha256` 键**必须被无声忽略**：线上发布出去的清单改不了，
    /// 新宿主读到老格式（或老宿主读到新格式）都不能因为多一个键就把整行丢掉。
    #[test]
    fn legacy_sha256_key_is_ignored_not_rejected() {
        let text = r#"[
          {
            "id": "common",
            "name": "Common",
            "version": "0.1.0",
            "abi": 5,
            "size": 100,
            "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "released": "2026-10-26",
            "formats": "jpg",
            "gitee": "https://example.com/common_0.1.0.dll"
          }
        ]"#;
        let v = parse_catalog(text).expect("多一个键不该让整份清单作废");
        assert_eq!(v.skipped, 0, "带 sha256 的旧项仍要正常显示");
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].id, "common");
    }

    /// 两个来源的下载地址分别读：发布项在 gitee 有地址、github 空；切到 github 就不算可下载。
    #[test]
    fn per_source_urls_are_read_independently() {
        let url = "https://gitee.com/qiuzongman/RuPix-Decoders/releases/download/c/c.dll";
        let v = parse_catalog(&format!(
            r#"[{{"id":"c","name":"C","version":"1.0.0","abi":5,"size":9,"formats":"qoi",
                 "gitee":"{url}","github":""}}]"#
        ))
        .expect("应当解析成功");
        let e = &v.entries[0];
        assert!(e.downloadable_for(SourceId::Gitee), "gitee 有地址");
        assert!(
            !e.downloadable_for(SourceId::Github),
            "github 空 = 该来源不可下"
        );
        assert_eq!(e.download_url(SourceId::Gitee), Some(url));
    }

    /// 来源下拉必须**同时换清单与下载列**：两个来源各有各的地址，选谁就用谁那一列。
    ///
    /// 这条是用户 2026-10-26 口径的锚："有的用户更方便 Gitee，有的更方便 GitHub"——
    /// 只换清单不换下载列等于把人送到另一个域名，两个都要跟着下拉走。
    #[test]
    fn both_columns_follow_the_selected_source() {
        let gitee = "https://gitee.com/qiuzongman/RuPix-Decoders/releases/download/c/c.dll";
        let github = "https://github.com/zongmanqiu/RuPix-Decoders/releases/download/c/c.dll";
        let v = parse_catalog(&format!(
            r#"[{{"id":"c","name":"C","version":"1.0.0","abi":5,"size":9,"formats":"qoi",
                 "gitee":"{gitee}","github":"{github}"}}]"#
        ))
        .expect("应当解析成功");
        let e = &v.entries[0];
        assert_eq!(e.download_url(SourceId::Gitee), Some(gitee));
        assert_eq!(e.download_url(SourceId::Github), Some(github));
        assert!(e.downloadable_for(SourceId::Gitee));
        assert!(e.downloadable_for(SourceId::Github));
    }

    /// 线上旧清单的 GitHub owner 错账：解析期修成 `zongmanqiu`，下载列直接可用。
    ///
    /// 这是过渡兼容（2026-10-26）：两份 raw 清单推送前仍是 `qiuzongman`，不修的话
    /// 用户一刷新就把本机正确的 GitHub 地址覆盖成灰的。
    #[test]
    fn stale_wrong_owner_github_url_is_repaired() {
        let v = parse_catalog(
            r#"[{"id":"c","name":"C","version":"1.0.0","abi":5,"size":9,"formats":"qoi",
                 "gitee":"https://gitee.com/qiuzongman/RuPix-Decoders/releases/download/c/c.dll",
                 "github":"https://github.com/qiuzongman/RuPix-Decoders/releases/download/c/c.dll"}]"#,
        )
        .expect("应当解析成功");
        assert_eq!(
            v.entries[0].download_url(SourceId::Github),
            Some("https://github.com/zongmanqiu/RuPix-Decoders/releases/download/c/c.dll"),
            "已知的 owner 错账应在解析期修正"
        );
    }

    /// 线上旧清单的**仓库改名**：单数 `RuPix-Decoder` 在解析期搬成复数，两个来源都能下。
    ///
    /// 用户缓存过的旧清单会一直用到下次刷新；仓库改名不能把已装用户手里的
    /// 下载按钮整片变灰（2026-10-28 实测：单数 raw 清单两个镜像都 404）。
    #[test]
    fn stale_singular_repo_name_is_repaired() {
        let v = parse_catalog(
            r#"[{"id":"c","name":"C","version":"1.0.0","abi":6,"size":9,"formats":"qoi",
                 "gitee":"https://gitee.com/qiuzongman/RuPix-Decoder/releases/download/c/c.dll",
                 "github":"https://github.com/qiuzongman/RuPix-Decoder/releases/download/c/c.dll"}]"#,
        )
        .expect("应当解析成功");
        let e = &v.entries[0];
        assert_eq!(
            e.download_url(SourceId::Gitee),
            Some("https://gitee.com/qiuzongman/RuPix-Decoders/releases/download/c/c.dll")
        );
        assert_eq!(
            e.download_url(SourceId::Github),
            Some("https://github.com/zongmanqiu/RuPix-Decoders/releases/download/c/c.dll"),
            "单数仓库名 + 错 owner 的组合要一次搬正"
        );
        assert!(e.downloadable_for(SourceId::Gitee));
        assert!(e.downloadable_for(SourceId::Github));
    }

    /// 不属于本仓的地址原样保留：修复逻辑只认几条已知前缀，**不猜**别家仓库。
    #[test]
    fn foreign_urls_are_left_untouched() {
        let v = parse_catalog(
            r#"[{"id":"c","name":"C","version":"1.0.0","abi":6,"size":9,"formats":"qoi",
                 "gitee":"https://example.com/c.dll","github":""}]"#,
        )
        .expect("应当解析成功");
        assert_eq!(
            v.entries[0].download_url(SourceId::Gitee),
            Some("https://example.com/c.dll")
        );
    }

    /// 地址 owner 与来源对不上：**不算可下载**（灰按钮），而不是让用户点了吃 404。
    ///
    /// 2026-10-26 的线上清单就把 Gitee 的 `qiuzongman` 写进了 GitHub 地址；
    /// 这条判据让它切到 GitHub 来源时安静地变灰。
    #[test]
    fn wrong_owner_url_is_not_downloadable_for_that_source() {
        let mut e = RemoteEntry {
            id: "c".into(),
            name: "C".into(),
            version: "1.0.0".into(),
            abi: 5,
            size: 9,
            released: String::new(),
            author: String::new(),
            license: String::new(),
            desc: String::new(),
            formats: vec!["qoi".into()],
            urls: std::collections::HashMap::new(),
        };
        e.urls.insert(
            "gitee".into(),
            "https://gitee.com/qiuzongman/RuPix-Decoders/releases/download/c/c.dll".into(),
        );
        e.urls.insert(
            "github".into(),
            "https://github.com/qiuzongman/RuPix-Decoders/releases/download/c/c.dll".into(),
        );
        assert!(e.downloadable_for(SourceId::Gitee), "gitee 地址是对的");
        assert!(
            !e.downloadable_for(SourceId::Github),
            "github 地址带着 gitee 的 owner，必须判成该来源没有地址"
        );
        assert_eq!(e.download_url(SourceId::Github), None);

        // 换成正确的 GitHub owner 就恢复可下载。
        e.urls.insert(
            "github".into(),
            "https://github.com/zongmanqiu/RuPix-Decoders/releases/download/c/c.dll".into(),
        );
        assert!(e.downloadable_for(SourceId::Github));
    }

    /// 信息不全的项**不进列表**，只计数（用户口径：没有完整信息就不展示）。
    #[test]
    fn incomplete_entries_are_skipped_not_shown() {
        let text = r#"[
          {"id":"a","name":"A","version":"1.0.0","abi":5,"size":100,"formats":"jpg","url":"u"},
          {"name":"缺 id","version":"1.0.0","abi":5,"size":100,"formats":"jpg","url":"u"},
          {"id":"c","version":"1.0.0","abi":5,"size":100,"formats":"jpg","url":"u"},
          {"id":"d","name":"D","version":"1.0.0","abi":5,"formats":"jpg","url":"u"},
          {"id":"e","name":"E","version":"1.0.0","abi":5,"size":0,"formats":"jpg","url":"u"},
          {"id":"f","name":"F","version":"1.0.0","abi":5,"size":100,"formats":"","url":"u"},
          {"id":"../g","name":"G","version":"1.0.0","abi":5,"size":100,"formats":"jpg","url":"u"},
          {"id":"h","name":"H","version":"../1.0.0","abi":5,"size":100,"formats":"jpg","url":"u"}
        ]"#;
        let v = parse_catalog(text).expect("整份是数组就该成功");
        assert_eq!(v.entries.len(), 1, "只有第一项信息完整");
        assert_eq!(v.entries[0].id, "a");
        assert_eq!(v.skipped, 7, "其余七项各缺一样或多一样致命字段");
    }

    /// 可选字段缺了不算残缺：`released` / `desc` / `url` 缺席 = 未发布、无说明。
    #[test]
    fn optional_fields_may_be_absent() {
        let v = parse_catalog(
            r#"[{"id":"x","name":"X","version":"1.0.0","abi":5,"size":9,"formats":"qoi"}]"#,
        )
        .expect("可选字段缺席不该判错");
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].released, "");
        assert_eq!(v.entries[0].desc, "");
        assert!(!v.entries[0].downloadable(), "没有 gitee = 未发布");
    }

    #[test]
    fn empty_format_string_is_an_empty_list() {
        assert!(split_formats("").is_empty());
        assert!(split_formats(";;;").is_empty(), "空段不该变成空字符串项");
        assert_eq!(split_formats(" jpg ; png "), vec!["jpg", "png"]);
    }

    /// 整份形状不对才是错误（单项残缺已改成跳过）。
    #[test]
    fn errors_only_when_the_whole_document_is_wrong() {
        let e = parse_catalog("{\"decoders\":[]}").expect_err("顶层必须是数组");
        assert!(e.contains("数组"), "{e}");
        assert!(parse_catalog("[{\"id\":").is_err(), "截断的 JSON 必须报错");
    }

    /// 「已下载」判定：**同 id + 同版本**就够，字节数不参与（2026-11-02 用户口径）。
    /// 版本不同**不再一律算「没装」**：本机旧 = 可升级，本机新 = 已超前（2026-10-26）。
    #[test]
    fn install_state_needs_only_id_and_version() {
        let remote = RemoteEntry {
            id: "common".into(),
            name: "Common".into(),
            version: "0.1.0".into(),
            abi: 5,
            size: 1000,
            released: "2026-10-26".into(),
            author: String::new(),
            license: String::new(),
            desc: String::new(),
            formats: vec!["jpg".into()],
            urls: std::collections::HashMap::new(),
        };
        let installed = |id: &str, file: &str, size: u64| DecoderEntry {
            id: id.into(),
            display_name: id.into(),
            description: String::new(),
            extensions: vec!["jpg".into()],
            file: file.into(),
            path: std::path::PathBuf::from(file),
            size,
            mtime: None,
            health: super::super::Health::Ok,
            enabled: true,
            has_raw: false,
            has_vector: false,
            vector_exts: Vec::new(),
            vector_region: false,
            abi: Some(5),
            metadata: Vec::new(),
        };

        assert_eq!(install_state(&remote, &[]), InstallState::Absent);
        assert_eq!(
            install_state(&remote, &[installed("other", "other_0.1.0.dll", 1000)]),
            InstallState::Absent,
            "id 不同不当成同一枚"
        );
        assert_eq!(
            install_state(&remote, &[installed("common", "common_0.0.9.dll", 1000)]),
            InstallState::Upgradable,
            "本机是 0.0.9、清单是 0.1.0 ⇒ 可升级（不是「没装」）"
        );
        assert_eq!(
            install_state(&remote, &[installed("common", "common_0.2.0.dll", 1000)]),
            InstallState::Ahead,
            "本机比清单新 ⇒ 不能给「升级」把人按回旧版"
        );
        assert_eq!(
            install_state(&remote, &[installed("common", "common_0.1.0.dll", 1000)]),
            InstallState::Installed
        );
        assert_eq!(
            install_state(&remote, &[installed("common", "common_0.1.0.dll", 999)]),
            InstallState::Installed,
            "同名同版本就是同一枚：不因字节数不同把正常的一份报成「大小不符」"
        );
        assert_eq!(
            install_state(&remote, &[installed("common", "common.dll", 1000)]),
            InstallState::Absent,
            "文件名读不出版本号 ⇒ 退回「没装」，不猜"
        );
    }

    /// 版本比较：**逐段比数字**，不是比字符串。
    ///
    /// 这条钉的是「0.10.0 比 0.9.0 新」——字符串序会给出相反答案，而那正好是界面判
    /// 「升级 / 已下载」的唯一依据。
    #[test]
    fn version_cmp_compares_numbers_not_text() {
        use std::cmp::Ordering;

        assert_eq!(version_cmp("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(version_cmp("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(version_cmp("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(version_cmp("1.0.0", "1.0.1"), Ordering::Less);
        assert_eq!(version_cmp("2.0.0", "0.9.9"), Ordering::Greater);
        // 缺段当 0：`1.0` 与 `1.0.0` 是同一版（写法不同而已，不该判升级）。
        assert_eq!(version_cmp("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(version_cmp("1", "1.0.0"), Ordering::Equal);
        // 预发布尾巴：发布版更大。
        assert_eq!(version_cmp("1.0.0", "1.0.0-beta"), Ordering::Greater);
        assert_eq!(version_cmp("1.0.0-beta", "1.0.0"), Ordering::Less);
        // 前缀 `v` 不算数值，两串都带也能比。
        assert_eq!(version_cmp("v0.2.0", "0.1.0"), Ordering::Greater);
        // 解析不出数字的段：数字按 0 算、尾巴非空 ⇒ 排在同数字的发布版之前，且**不会崩**。
        assert_eq!(version_cmp("x.y", "0"), Ordering::Less);
    }

    /// 随包清单必须在 `<exe>/docs/decoders.json`：发布目录整体搬走时它跟着走。
    #[test]
    fn local_catalogue_lives_in_docs_beside_exe() {
        let dir = crate::core::paths::exe_dir().expect("拿不到 exe 目录");
        let p = local_path().expect("本地清单路径");
        assert_eq!(p.parent(), Some(dir.join("docs").as_path()));
        assert_eq!(p.file_name().unwrap(), INDEX_FILE);
    }

    /// 本地清单的读写与覆盖：首次创建、读回、二次覆盖、不留 `.part` / `.old`。
    ///
    /// 这条钉的是 Windows 那个「`rename` 不能覆盖已存在目标」的换名链：谁图省事把它改回
    /// `rename(part, target)`，第二次刷新就会失败，而单靠"首次写"的用例测不到。
    #[test]
    fn local_catalogue_round_trips_and_replaces_cleanly() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(INDEX_FILE);
        assert!(load_from(&path).is_err(), "没有文件时必须是错误");
        save_to(&path, SAMPLE).expect("首次写");
        assert_eq!(load_from(&path).expect("读回").entries.len(), 2);

        // 上一次中断留下的 `.old` 不该让下一次覆盖卡死。
        std::fs::write(dir.join(format!("{INDEX_FILE}.old")), b"stale").expect("造一个残留 .old");
        let one = r#"[{"id":"x","name":"X","version":"1.0.0","abi":5,"size":9,"formats":"qoi"}]"#;
        save_to(&path, one).expect("覆盖");
        let cat = load_from(&path).expect("读回新");
        assert_eq!(cat.entries.len(), 1);
        assert_eq!(cat.entries[0].id, "x");
        assert!(!dir.join(format!("{INDEX_FILE}.part")).exists());
        assert!(!dir.join(format!("{INDEX_FILE}.old")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 刷新失败只有一句话（2026-10-29 用户口径）：不区分网络 / 地址 / 落盘，
    /// 界面绝不出现 `0x...`、裸 HTTP 码或路径。
    #[test]
    fn refresh_failure_is_one_short_sentence() {
        assert_eq!(catalog_error_text(), "更新失败");
        assert!(!catalog_error_text().contains("0x"));
        assert!(!catalog_error_text().contains("HTTP"));
    }

    /// 连点「刷新」时页面继续说「更新中…」，不谎报「更新失败」（2026-10-29 裁定 #11）。
    #[test]
    fn busy_refresh_keeps_the_in_progress_sentence() {
        assert_eq!(BeginError::Busy.ui_text(), "更新中…");
        assert_eq!(BeginError::Refused("坏地址".into()).ui_text(), "更新失败");
    }

    /// 真联网一条：**两个镜像都拉一遍**，本机没网就跳过（不能因为它把门禁判红）。
    ///
    /// 用的是**线上那两份清单**——这条用例同时守着"清单结构与解析器没走散"，以及
    /// "两个源都真的可达"：2026-10-26 把 GitHub 地址写成了 Gitee 的 owner
    /// （`qiuzongman`），只测 Gitee 的用例全绿，用户切到 GitHub 却拿到 404。
    /// 所以每个源都要有自己的联网用例；URL 本身另有纯测试钉死（见 `core::config`）。
    #[test]
    fn fetches_both_live_catalogues_or_skips_without_network() {
        let mut fetched: Vec<(SourceId, Catalog)> = Vec::new();
        for src in SourceId::ALL {
            // 地址本身来自配置的默认值：这里验的正是"默认地址真好用"。
            let cfg = crate::core::config::Config::default();
            let url = match src {
                SourceId::Gitee => cfg.decoder_index_gitee.clone(),
                SourceId::Github => cfg.decoder_index_github.clone(),
            };
            match fetch(&url) {
                Ok((v, text)) => {
                    assert!(!text.is_empty(), "{} 响应正文不该是空的", src.label());
                    assert!(!v.entries.is_empty(), "{} 线上清单不该是空的", src.label());
                    assert_eq!(v.skipped, 0, "{} 线上清单出现了信息不全的项", src.label());
                    for e in &v.entries {
                        assert!(!e.id.is_empty());
                        // 线上清单可能还停在旧 ABI；联网冒烟只要求结构可读，
                        // 不把“远端尚未发布 v5”判成本地失败。
                        if !e.abi_ok() && e.abi != 0 {
                            eprintln!(
                                "skip: {} 的线上 abi {} 本宿主收不了（等清单刷新）",
                                e.id, e.abi
                            );
                            continue;
                        }
                        assert!(e.size > 0, "{} 的 size 是 0", e.id);
                    }
                    fetched.push((src, v));
                }
                Err(e) => {
                    // 只允许"网络不可用"这一类跳过；HTTP 4xx / 解析错必须判红。
                    let network_down = e.contains("连接服务器")
                        || e.contains("发送请求")
                        || e.contains("读取响应");
                    assert!(
                        network_down,
                        "{} 线上清单取不到，且不是网络原因：{e}",
                        src.label()
                    );
                    eprintln!("跳过 {}：无网络（{e}）", src.label());
                }
            }
        }
        if fetched.len() == SourceId::ALL.len() {
            let ids =
                |v: &Catalog| -> Vec<String> { v.entries.iter().map(|e| e.id.clone()).collect() };
            assert_eq!(
                ids(&fetched[0].1),
                ids(&fetched[1].1),
                "Gitee 与 GitHub 两份镜像的条目 id 不一致"
            );
        }
    }
}
