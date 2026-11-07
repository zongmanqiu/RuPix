//! 宿主本体自更新：**纯逻辑层**（版本 / 清单 / 两条判据）。
//!
//! 本层离屏可跑：不碰窗口、不碰 Win32、不碰文件系统、不联网。线程的活全在
//! [`crate::runtime::update`]，这里只回答「这份清单说了什么」「这个包能不能装」。
//!
//! # 只有两条判据（用户 2026-11-07 拍板）
//!
//! 1. **文件名对得上**：下载地址末段必须是 `RuPix_<version>.zip`；ZIP 里只允许
//!    `RuPix_<version>/` 这一层顶层目录（或干脆没有顶层），其内只允许
//!    [`PACKAGE_FILES`] 那四个精确名字，且必须有 `rupix.exe`。
//! 2. **版本号对得上**：清单 `version` 与下载地址文件名、ZIP 顶层目录名里解析出的
//!    版本逐字一致，且高于当前版本。
//!
//! **不算哈希、不比文件大小、不比下载字节数、不读 PE 里的版本资源**——产物出自
//! 官方渠道，识别一份包只认名字与版本，与 `decoders.json` / `plugins.json`
//! 「清单无哈希、只认 id + version」同一条口径。`platform::http` 的响应体上限与
//! ZIP 解包量上限是防内存爆的资源护栏，不读清单、不与任何期望值比对，不是身份校验。

use std::fmt;

use crate::core::json::{self, Value};

/// 主程序在包里的精确文件名。缺它就不算更新包。
pub const EXE_NAME: &str = "rupix.exe";

/// 包里**只允许**这四种文件名（目录项除外），且必须有 [`EXE_NAME`]。
/// 少一两个非主程序文件不算错：覆盖时同名旧文件照旧留着，不会被删。
pub const PACKAGE_FILES: [&str; 4] = [
    "rupix.exe",
    "LICENSE",
    "THIRD-PARTY-NOTICES.md",
    "README.txt",
];

/// 更新清单的响应体上限。真实清单 1 KB 级；这是「这明显不是清单」的闸，不是判据。
pub const MANIFEST_MAX_BYTES: usize = 1 << 20;

/// 更新包的响应体 / 解包量上限。防 zip bomb 与内存爆的护栏，不与清单比大小。
pub const PACKAGE_MAX_BYTES: usize = 64 << 20;

/// 清单地址只放行两个官方仓的 raw 域；下载资产只放行两个官方仓的 release 域。
/// ini 允许用户改地址，但改到第三方也不会被拿去覆盖自己的 exe。
const OFFICIAL: [&str; 4] = [
    "https://raw.giteeusercontent.com/qiuzongman/RuPix/",
    "https://raw.githubusercontent.com/zongmanqiu/RuPix/",
    "https://gitee.com/qiuzongman/RuPix/releases/download/",
    "https://github.com/zongmanqiu/RuPix/releases/download/",
];

const MANIFEST_PREFIXES: [&str; 2] = [OFFICIAL[0], OFFICIAL[1]];
const ASSET_PREFIXES: [&str; 2] = [OFFICIAL[2], OFFICIAL[3]];

/// 三段式版本号。预发布 / 构建后缀一律丢弃（`0.1.1-beta` == `0.1.1`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const ZERO: Self = Self {
        major: 0,
        minor: 0,
        patch: 0,
    };

    /// 解析 `v1.2` / `1.2.3` / `1.2.3-rc1`；缺段补 0，非数字段整段作废。
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let core = s.split(['-', '+']).next().unwrap_or(s);
        let mut nums = [0u32; 3];
        let mut seen = 0usize;
        for (slot, part) in nums.iter_mut().zip(core.split('.')) {
            let t = part.trim();
            if t.is_empty() {
                break;
            }
            *slot = t.parse().ok()?;
            seen += 1;
        }
        if seen == 0 || core.split('.').count() > 3 {
            return None;
        }
        Some(Self {
            major: nums[0],
            minor: nums[1],
            patch: nums[2],
        })
    }

    /// 编译期植入的当前版本。
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self::ZERO)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 更新清单。字段少而明确，**没有 sha256 / size**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub published: String,
    pub notes: String,
    pub urls: Urls,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Urls {
    pub gitee: String,
    pub github: String,
}

/// 一次确认可用的更新计划；`urls` 顺序 = 先首选源、再备用源（备用可为空串）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub version: String,
    pub notes: String,
    pub urls: [String; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate { latest: String },
    Available(Plan),
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("update.json 缺字段 {key}"))
}

/// 解析清单文本（纯逻辑，测试直接喂字符串）。
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let root = json::parse(text).map_err(|e| format!("update.json：{e}"))?;
    if root.as_array().is_some() || root.as_str().is_some() {
        return Err("update.json 根必须是对象".into());
    }
    let version = field(&root, "version")?.trim().to_string();
    if Version::parse(&version).is_none() {
        return Err(format!("update.json 的 version 不合法：{version}"));
    }
    let urls = root
        .get("urls")
        .ok_or_else(|| "update.json 缺字段 urls".to_string())?;
    let m = Manifest {
        version,
        published: root
            .get("published")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        notes: root
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        urls: Urls {
            gitee: field(urls, "gitee").unwrap_or("").to_string(),
            github: field(urls, "github").unwrap_or("").to_string(),
        },
    };
    if m.urls.gitee.is_empty() && m.urls.github.is_empty() {
        return Err("update.json 没有任何 zip 地址".into());
    }
    Ok(m)
}

/// 清单地址白名单。
pub fn is_safe_manifest_url(url: &str) -> bool {
    MANIFEST_PREFIXES.iter().any(|p| url.starts_with(p))
}

/// 下载地址白名单。
pub fn is_safe_asset_url(url: &str) -> bool {
    ASSET_PREFIXES.iter().any(|p| url.starts_with(p))
}

/// 版本对应的包名：`0.1.1` → `RuPix_0.1.1.zip`。
pub fn package_name(version: &str) -> String {
    format!("RuPix_{version}.zip")
}

/// 从 `RuPix_<version>.zip` / `RuPix_<version>` 里取出版本串。
pub fn version_from_package_name(name: &str) -> Option<&str> {
    let s = name.strip_prefix("RuPix_")?;
    let s = s.strip_suffix(".zip").unwrap_or(s);
    let s = s.strip_suffix('/').unwrap_or(s);
    if s.is_empty() || s.contains('/') || s.contains('\\') {
        return None;
    }
    Some(s)
}

/// **判据 1（下载地址那一半）+ 判据 2**：URL 必须来自官方域，末段文件名必须是
/// `RuPix_<version>.zip`，且文件名里的版本与清单逐字一致。
pub fn check_asset_url(url: &str, version: &str) -> Result<(), String> {
    if !is_safe_asset_url(url) {
        return Err("拒绝非官方更新包地址".into());
    }
    let tail = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "更新包地址没有文件名".to_string())?;
    let want = package_name(version);
    if tail != want {
        return Err(format!("更新包文件名对不上（应为 {want}）"));
    }
    let got = version_from_package_name(tail).unwrap_or("");
    if got != version {
        return Err("更新包文件名里的版本号与清单不一致".into());
    }
    Ok(())
}

/// 一个 ZIP 条目校验后的落点：`(存档内名字, 目标文件名)`。
pub type ArchiveFile = (String, &'static str);

/// **判据 1（包内那一半）+ 判据 2**：核对 ZIP 的全部条目名，返回解压映射。
///
/// 顶层目录只允许 `RuPix_<version>/`（也允许压根没有顶层、四个裸文件），其余目录、
/// 额外文件、重复文件、路径穿越一律拒绝。目录项本身跳过。
pub fn check_archive(names: &[String], version: &str) -> Result<Vec<ArchiveFile>, String> {
    let top = format!("RuPix_{version}/");
    let mut out: Vec<ArchiveFile> = Vec::new();
    for raw in names {
        let name = raw.replace('\\', "/");
        // 目录项只允许顶层那一个（名字里的版本也必须对得上）；包内不许有子目录。
        if name.ends_with('/') {
            if name != top {
                return Err(format!("更新包里不允许这个目录：{name}"));
            }
            continue;
        }
        if name.starts_with('/')
            || name
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(format!("更新包含非法路径：{name}"));
        }
        let rel = if let Some(r) = name.strip_prefix(&top) {
            r
        } else {
            // 没有顶层时，顶层名本身也不能是别的目录：下面这个判断会把它当文件名拒掉。
            name.as_str()
        };
        if let Some(dir_name) = name.split('/').next() {
            if name.contains('/') && dir_name != &top[..top.len() - 1] {
                let got = version_from_package_name(dir_name).unwrap_or("");
                if got != version {
                    return Err(format!("更新包顶层目录名或版本号对不上：{dir_name}"));
                }
            }
        }
        let target = match PACKAGE_FILES.iter().find(|f| **f == rel) {
            Some(f) => *f,
            None => return Err(format!("更新包里不允许这个文件：{rel}")),
        };
        if out.iter().any(|(_, t)| *t == target) {
            return Err(format!("更新包里出现两份 {target}"));
        }
        out.push((raw.clone(), target));
    }
    if !out.iter().any(|(_, t)| *t == EXE_NAME) {
        return Err("更新包里没有 rupix.exe，已中止".into());
    }
    Ok(out)
}

/// 按 `source`（0 = Gitee 先，1 = GitHub 先）排出两条下载地址，并跑一遍判据 2。
pub fn plan(m: &Manifest, current: Version, source: usize) -> Result<Outcome, String> {
    let latest = Version::parse(&m.version)
        .ok_or_else(|| format!("update.json 的 version 不合法：{}", m.version))?;
    if latest <= current {
        return Ok(Outcome::UpToDate {
            latest: m.version.clone(),
        });
    }
    let first = source.min(1);
    let cols = [&m.urls.gitee, &m.urls.github];
    let good: Vec<String> = [cols[first].as_str(), cols[1 - first].as_str()]
        .into_iter()
        .filter(|u| !u.is_empty())
        .filter(|u| check_asset_url(u, &m.version).is_ok())
        .map(str::to_string)
        .collect();
    if good.is_empty() {
        return Err("update.json 没有文件名与版本都对得上的官方地址".into());
    }
    Ok(Outcome::Available(Plan {
        version: m.version.clone(),
        notes: m.notes.clone(),
        urls: [good[0].clone(), good.get(1).cloned().unwrap_or_default()],
    }))
}

/// UI → runtime 的**单向投递槽**。
///
/// `ui` 层不许引 `runtime`（分层门禁 machine-checked），所以「关于」页的按钮只把
/// 请求放在这里，由 `runtime::update::poll` 在下一拍取走并起后台线程。槽本身不含
/// 任何 IO 或窗口依赖，`core` 层保持离屏可测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Check,
    Download(Plan),
}

static PENDING: std::sync::Mutex<Option<Request>> = std::sync::Mutex::new(None);

/// 投递一次请求；同时只允许一个待办，重复点击直接被挡住。
pub fn request(r: Request) -> Result<(), String> {
    let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_some() {
        return Err("已有一个更新请求在排队".into());
    }
    *g = Some(r);
    Ok(())
}

/// 取走待办（由 `runtime::update::poll` 每帧调一次）。
pub fn take_request() -> Option<Request> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "version": "0.1.1",
        "published": "2026-11-08",
        "notes": "新增一些东西",
        "urls": {
            "gitee": "https://gitee.com/qiuzongman/RuPix/releases/download/0.1.1/RuPix_0.1.1.zip",
            "github": "https://github.com/zongmanqiu/RuPix/releases/download/0.1.1/RuPix_0.1.1.zip"
        }
    }"#;

    #[test]
    fn version_parse_is_lenient_but_not_sloppy() {
        assert_eq!(
            Version::parse("1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            Version::parse("v1.2"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("0.1.1-beta.2"),
            Some(Version {
                major: 0,
                minor: 1,
                patch: 1
            })
        );
        assert_eq!(Version::parse("1.x.3"), None);
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert!(Version::parse("0.1.1").unwrap() > Version::parse("0.1.0").unwrap());
    }

    #[test]
    fn manifest_parses_without_any_hash_or_size() {
        let m = parse_manifest(GOOD).expect("合法清单");
        assert_eq!(m.version, "0.1.1");
        assert_eq!(m.notes, "新增一些东西");
        assert!(m.urls.gitee.contains("RuPix_0.1.1.zip"));
        assert!(!GOOD.contains("sha256"), "清单不许出现哈希字段");
    }

    #[test]
    fn manifest_rejects_bad_shape() {
        assert!(parse_manifest("[]").is_err());
        assert!(parse_manifest(r#"{"version":"x","urls":{}}"#).is_err());
        assert!(parse_manifest(r#"{"version":"0.1.1","urls":{}}"#).is_err());
        assert!(parse_manifest(r#"{"version":"0.1.1"}"#).is_err());
    }

    #[test]
    fn only_official_urls_pass() {
        assert!(is_safe_manifest_url(
            "https://raw.giteeusercontent.com/qiuzongman/RuPix/raw/master/update.json"
        ));
        assert!(is_safe_manifest_url(
            "https://raw.githubusercontent.com/zongmanqiu/RuPix/refs/heads/master/update.json"
        ));
        assert!(!is_safe_manifest_url("https://evil.example/update.json"));
        assert!(!is_safe_asset_url("https://evil.example/RuPix_0.1.1.zip"));
    }

    #[test]
    fn asset_url_must_carry_the_exact_version_in_its_name() {
        let ok = "https://gitee.com/qiuzongman/RuPix/releases/download/0.1.1/RuPix_0.1.1.zip";
        assert!(check_asset_url(ok, "0.1.1").is_ok());
        let mismatch = "https://gitee.com/qiuzongman/RuPix/releases/download/0.1.1/RuPix_0.1.0.zip";
        assert!(check_asset_url(mismatch, "0.1.1").is_err());
        assert!(check_asset_url("https://evil.example/RuPix_0.1.1.zip", "0.1.1").is_err());
        assert_eq!(version_from_package_name("RuPix_0.1.1.zip"), Some("0.1.1"));
    }

    #[test]
    fn archive_accepts_only_the_four_named_files() {
        let names: Vec<String> = [
            "RuPix_0.1.1/",
            "RuPix_0.1.1/rupix.exe",
            "RuPix_0.1.1/LICENSE",
            "RuPix_0.1.1/THIRD-PARTY-NOTICES.md",
            "RuPix_0.1.1/README.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let files = check_archive(&names, "0.1.1").expect("四文件齐备");
        assert_eq!(files.len(), 4);
        assert!(files.iter().any(|(_, t)| *t == EXE_NAME));
    }

    #[test]
    fn archive_rejects_extra_missing_or_wrong_version() {
        let base = |extra: &str| {
            let mut v = vec!["RuPix_0.1.1/rupix.exe".to_string()];
            if !extra.is_empty() {
                v.push(extra.to_string());
            }
            v
        };
        assert!(check_archive(&base("RuPix_0.1.1/decoders/x.dll"), "0.1.1").is_err());
        assert!(check_archive(&base("RuPix_0.1.0/README.txt"), "0.1.1").is_err());
        assert!(check_archive(&base("../evil.exe"), "0.1.1").is_err());
        assert!(
            check_archive(&base(""), "0.1.1").is_ok(),
            "只给主程序也算过关"
        );
        let no_exe = vec!["RuPix_0.1.1/README.txt".to_string()];
        assert!(check_archive(&no_exe, "0.1.1").is_err());
        let dup = vec!["RuPix_0.1.1/rupix.exe".to_string(), "rupix.exe".to_string()];
        assert!(check_archive(&dup, "0.1.1").is_err());
    }

    #[test]
    fn request_slot_is_single_and_drains() {
        assert!(take_request().is_none());
        let r = Request::Check;
        assert!(request(r.clone()).is_ok());
        assert!(request(r.clone()).is_err(), "同时只许一个待办");
        assert_eq!(take_request(), Some(r));
        assert!(take_request().is_none(), "取走即清空");
    }

    #[test]
    fn plan_only_offers_a_newer_version() {
        let m = parse_manifest(GOOD).expect("合法清单");
        let cur = Version::parse("0.1.0").unwrap();
        let p = match plan(&m, cur, 0).expect("有新版") {
            Outcome::Available(p) => p,
            _ => panic!("应当报有新版"),
        };
        assert_eq!(p.version, "0.1.1");
        assert!(p.urls[0].contains("gitee.com"));
        assert!(p.urls[1].contains("github.com"));
        match plan(&m, Version::parse("0.1.1").unwrap(), 0).expect("已是最新") {
            Outcome::UpToDate { latest } => assert_eq!(latest, "0.1.1"),
            _ => panic!("不该报有新版"),
        }
    }
}
