//! 便携模式唯一的磁盘落点：**exe 所在目录**。
//!
//! # 为什么不再往 `%APPDATA%` 写
//!
//! RuPix 是便携程序——整个应用就是拷走的一个目录（或一枚 U 盘）。把持久化
//! 产物丢进用户配置目录有三个实际问题：
//!
//! 1. **换个环境设置就丢了**（或反过来：换了机器却带着旧偏好）；
//! 2. **多份拷贝互相打架**：U 盘一份、桌面一份，`exe` 不同但写的却是同一个 ini，
//!    改一边另一边跟着变，且看不出来为什么；
//! 3. **卸载残留**：删掉目录还留下一份文件在看不见的地方，与"拷走即用、删掉即净"
//!    的便携前提冲突。
//!
//! 因此**任何**持久化只往一个地方写：`exe` 的同级目录——其中**用户要看的**（配置）
//! 直接放在根，**只用来取证的**（日志、转储、会话戳）收进根下的 `temp\` 一枚子目录，
//! 按天命名、启动时清掉 7 天之前的（`KEEP_DAYS`）；**纯提速件**（丢了你只会觉得
//! 慢一点，如 SVG 字体索引缓存）收进根下的 `cache\`，不参与按天清理。根目录里
//! 因此只剩 `rupix.ini` 与 `decoders\`，一眼看得完；删目录仍然即净卸载（`temp\`
//! 与 `cache\` 都在目录里）。外部目录一个字节都不碰。
//!
//! | 产物           | 路径                          |
//! |----------------|-------------------------------|
//! | 配置文件       | `<exe 目录>/rupix.ini`    |
//! | 解码器清单     | `<exe 目录>/docs/decoders.json`（发布随包提供；「刷新」时覆盖） |
//! | 运行日志       | `<exe 目录>/temp/rupix-<YYYY-MM-DD>.log`（按天，写满即停见 `RUN_LOG_MAX_BYTES`） |
//! | 崩溃报告       | `<exe 目录>/temp/crash-<YYYY-MM-DD>.log`（同一天的几次崩追加进同一枚） |
//! | 崩溃转储       | `<exe 目录>/temp/crash-<YYYY-MM-DD-HHMMSS>.dmp`（只留最近几份，见 `platform::crash_evidence`） |
//! | 会话戳         | `<exe 目录>/temp/rupix.run`（正常退出即删，**不日期化**——它是状态不是历史） |
//!
//! 落点分两层是刻意的：诊断物一律 `temp\` + 名字里带日期，"7 天之前"这条判据才有得可解析；
//! 而 `rupix.ini` 留在根，是因为它是这个目录里唯一一份**用户会主动打开看**的文件。
//!
//! # 写不进去怎么办
//!
//! exe 落在只读介质（写保护的 SD 卡、光盘、受限目录）上时，写入会失败。此时
//! 一律**静默降级为纯内存会话**（不影响看图），只留一条 `warn` 日志——看图器存不下
//! 一份偏好不值得弹窗打断，这是 [`crate::core::config`] 既定策略的延续。

use std::path::{Path, PathBuf};

/// 配置文件名（exe 同级目录）。
pub const CONFIG_FILE: &str = "rupix.ini";

/// 诊断物子目录名（exe 同级之下）。
pub const DIAGNOSTICS_DIR: &str = "temp";

/// 运行期缓存子目录名（exe 同级之下）。
///
/// 与 `temp\`（取证物，按天清理）分工：这里是**纯提速件**——丢了只损失一点速度，
/// 不损失任何证据。典型住客：SVG 字体索引缓存（m187 起，`svg-fonts.bin`）。
/// 没有自动清理：单件千字节量级，且"下次启动要不要重扫字体"不值得按天腾挪。
pub const CACHE_DIR: &str = "cache";

/// 已安装插件目录名（exe 同级之下）：每个子目录一个插件，内含 `manifest.json` + 入口。
/// 用复数 `plugins`，与 `decoders` 保持同一命名习惯。
pub const PLUGIN_DIR: &str = "plugins";

/// 随包数据子目录名（exe 同级之下）：目前放解码器清单 `decoders.json`。
///
/// 它**不是** `main/docs/`（那是源码仓的长期文档）：这是发布包里的数据，由
/// `deploy.py` 从解码器发布清单复制过来，「刷新」按钮就地覆盖。
pub const DOCS_DIR: &str = "docs";

/// 崩溃报告文件名前缀（`crash-<YYYY-MM-DD>.log`）。
pub const CRASH_LOG_PREFIX: &str = "crash-";

/// 运行日志文件名前缀（`rupix-<YYYY-MM-DD>.log`）。
pub const RUN_LOG_PREFIX: &str = "rupix-";

/// 两类日志共用的后缀。
pub const LOG_SUFFIX: &str = ".log";

/// 崩溃转储的文件名前缀与后缀（前缀与崩溃报告同族，`crash-` 一眼配成一对）。
pub const DUMP_PREFIX: &str = "crash-";
pub const DUMP_SUFFIX: &str = ".dmp";

/// 诊断物保留天数：**按文件名里的日期**判，启动时删掉早于「今天 − 本值」的。
/// 判据用名字而不是 mtime——拷贝出来的日志 mtime 会变，名字里那串日期不会。
pub const KEEP_DAYS: i64 = 7;

/// 运行日志的容量闸门：**不是**"日志最多留多少历史"，而是**一枚日志写到多少就收手**——
/// 到顶之后本进程不再追加（`platform::crash_evidence::RunLogSink` 会先落一条标记）。
/// 为什么不是"截断重写"：那样吃掉的是当天开头的会话头与解码器清单，那是判版本、配 PDB
/// 的唯一依据。2 MiB 是"一次正常会话（千字节量级）× 一整天反复调试"都装得下的余量，
/// 同时守住便携目录的底线：一份日志不许吃满 U 盘。
pub const RUN_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// exe 所在目录的进程级缓存。
///
/// `current_exe` 是一次系统调用（QueryFullProcessImageName），而配置/崩溃日志都要用它，
/// 没必要每次派生都问一遍；进程生命周期内 exe 目录也不会变。
static EXE_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// exe 所在目录。取不到（极罕见）时为 `None`，调用方按"本次会话不落盘"处理。
pub fn exe_dir() -> Option<&'static Path> {
    EXE_DIR.get_or_init(current_exe_dir).as_deref()
}

fn current_exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // `current_exe` 返回的是**文件**路径，`parent()` 必不为 `None`（不会是根本身）。
    let dir = exe.parent()?.to_path_buf();
    log::debug!("便携根目录：{}", dir.display());
    Some(dir)
}

/// 配置文件路径：`<exe 目录>/rupix.ini`。
pub fn config_path() -> Option<PathBuf> {
    Some(exe_dir()?.join(CONFIG_FILE))
}

/// 已安装插件根目录：`<exe 目录>/plugins`。这里只是**路径**，建目录由安装器/用户负责。
pub fn plugin_dir() -> Option<PathBuf> {
    Some(exe_dir()?.join(PLUGIN_DIR))
}

/// 本地插件目录清单：`<exe 目录>/docs/plugins.json`（与解码器 `docs/decoders.json` 同模型）。
/// 清单里的 `file` 指向同目录下的 ZIP 包。
pub fn plugin_catalog_path() -> Option<PathBuf> {
    Some(docs_dir()?.join("plugins.json"))
}

/// 随包数据目录：`<exe 目录>/docs`。这里只是**路径**，建目录由写它的人负责
/// （`decoders::remote::save_local` 在覆盖清单前保证一次）。
pub fn docs_dir() -> Option<PathBuf> {
    Some(exe_dir()?.join(DOCS_DIR))
}

/// 诊断物目录：`<exe 目录>/temp`。这里只是**路径**，建目录由写它的人负责
/// （`platform::crash_evidence` 在开日志与写转储前各保证一次，静默失败）。
pub fn diagnostics_dir() -> Option<PathBuf> {
    Some(exe_dir()?.join(DIAGNOSTICS_DIR))
}

/// 运行期缓存目录：`<exe 目录>/cache`。这里只是**路径**，建目录由写它的人负责
/// （windui 的字体索引缓存在落盘前 `create_dir_all`，静默失败）。
pub fn cache_dir() -> Option<PathBuf> {
    Some(exe_dir()?.join(CACHE_DIR))
}

/// 崩溃报告路径：`<exe 目录>/temp/crash-<day>.log`。`day` 用本地 `YYYY-MM-DD`
/// （由 `platform::panic_guard` 那一刻的本地日历给出，core 不认识时钟）。
pub fn crash_log_path(day: &str) -> Option<PathBuf> {
    Some(diagnostics_dir()?.join(format!("{CRASH_LOG_PREFIX}{day}{LOG_SUFFIX}")))
}

/// 运行日志路径：`<exe 目录>/temp/rupix-<day>.log`（写满即停，见 `RUN_LOG_MAX_BYTES`）。
pub fn run_log_path(day: &str) -> Option<PathBuf> {
    Some(diagnostics_dir()?.join(format!("{RUN_LOG_PREFIX}{day}{LOG_SUFFIX}")))
}

/// 从一枚诊断物的**文件名**里取出它的日期，返回 `(年, 月, 日)`；认不出来返回 `None`。
///
/// 三族名字一律以 `<前缀>YYYY-MM-DD` 开头（后面可以再接时分秒，也可以没有）：
/// `rupix-2026-10-19.log` / `crash-2026-10-19.log` / `crash-2026-10-19-112233.dmp`。
/// **认不出来就一律不删**——这是清理逻辑的底线：`temp\` 里可能躺着用户自己放的任何东西，
/// 而一次误删比留下几枚旧日志严重得多。
pub fn diag_ymd(name: &str) -> Option<(u32, u32, u32)> {
    for prefix in [RUN_LOG_PREFIX, DUMP_PREFIX] {
        let Some(rest) = name.strip_prefix(prefix) else {
            continue;
        };
        // `get(..10)` 而不是 `[..10]`：文件名可能以多字节字符开头，裸切片会踩到
        // 非字符边界直接 panic——清理逻辑不许有把进程弄崩的那条路。
        if let Some(d) = rest.get(..10).and_then(ymd) {
            return Some(d);
        }
    }
    None
}

/// 测试与离屏探针的**统一落点**：`<仓库>/.temp/rupix-test/`。
///
/// ★ 为什么生产代码不该调它：便携铁律只承认"exe 同级"这一个持久化落点，而这里返回的
/// 是**源码树旁边**的构建期临时区（`AGENTS.md` §二）——它存在于开发机上，不存在于用户机器上。
/// 把它做成一个函数正是为了让"谁在写临时文件"这件事可 grep、可数。
///
/// ★ 为什么不用 `std::env::temp_dir()`：那会把每一轮用例的夹具写进**用户自己的**系统 TEMP。
/// 本机 2026-10-29 清掉过一次，光 `sv-test-prefetch-*` 就积了 369 个目录；而
/// `build/5_test.bat` 与 `build/gate.py` 虽然都把 `TEMP`/`TMP` 指到仓内，**IDE 或手工裸跑
/// `cargo test` 不经过它们**（`.cargo/config.toml` 那条 `[env]` 便宜路已试过并撤回：带 `..` 的
/// 路径交给 `link.exe` 会按它自己的 CWD 再解一次 ⇒ `LNK1104`，正本见那个文件的注释）。
///
/// 返回的是**已创建**的目录。路径按**词法**绝对化（消掉 `..`）——★ 刻意不用 `canonicalize()`：
/// 它会加 `\\?\` verbatim 前缀，而 Win32 的一批路径接口不认那个前缀（实测本仓的
/// `platform::recycle::send_to_recycle_bin` → `SHFileOperationW` 直接失败，用例当场红）。
/// 名字里带不带 `-{pid}` 由调用方自己拼（与旧写法逐字同形），所以这一格**只改落点，
/// 不改任何用例的清理逻辑**。整个 `rupix-test/` 子树是可以随手删的：`.temp` 按定义就是 disposable。
pub fn test_scratch_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let raw = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.temp")
            .join("rupix-test");
        std::fs::create_dir_all(&raw).ok();
        // 词法绝对化失败（极少数环境不给读目录）就退回原样：用例照样能跑，
        // 只是别拿这条路径去做字符串比较。
        std::path::absolute(&raw).unwrap_or(raw)
    })
    .clone()
}

/// 那一段 `YYYY-MM-DD` → 日期。分隔符不对、月份/日期越界都算不认识。
fn ymd(day: &str) -> Option<(u32, u32, u32)> {
    let b = day.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y = day.get(0..4)?.parse().ok()?;
    let m = day.get(5..7)?.parse().ok()?;
    let d = day.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 配置文件必须**就在** exe 同级目录里：不能多套一层子目录，也不能跑到别处。
    #[test]
    fn config_sits_beside_exe() {
        let dir = exe_dir().expect("拿不到 exe 目录就无从谈便携");
        let p = config_path().expect("配置路径");
        assert_eq!(p.parent(), Some(dir), "配置文件不在 exe 同级目录");
        assert_eq!(p.file_name().unwrap(), CONFIG_FILE);
        assert!(
            !p.to_string_lossy().contains("AppData"),
            "不许写到用户配置目录"
        );
    }

    /// 插件目录必须在 exe 同级，不能跑到用户目录。
    #[test]
    fn plugin_dirs_sit_beside_exe() {
        let dir = exe_dir().expect("拿不到 exe 目录");
        let plugins = plugin_dir().expect("插件目录");
        assert_eq!(plugins.parent(), Some(dir), "插件目录不是 exe 的直接子目录");
        assert_eq!(plugins.file_name().unwrap(), PLUGIN_DIR);
        assert!(!plugins.to_string_lossy().contains("AppData"));

        let catalog = plugin_catalog_path().expect("插件清单路径");
        assert_eq!(
            catalog.parent(),
            docs_dir().as_deref(),
            "清单必须和 docs 在一起"
        );
        assert_eq!(catalog.file_name().unwrap(), "plugins.json");
    }

    /// 随包清单就在 exe 同级的 `docs\` 里：发布目录搬走时清单跟着走。
    #[test]
    fn decoder_catalogue_sits_beside_exe_in_docs() {
        let dir = exe_dir().expect("拿不到 exe 目录");
        let docs = docs_dir().expect("清单目录路径");
        assert_eq!(docs.parent(), Some(dir), "docs 不是 exe 的直接子目录");
        assert_eq!(docs.file_name().unwrap(), DOCS_DIR);
        assert!(!docs.to_string_lossy().contains("AppData"));
    }

    /// 诊断物一律在 `<exe 目录>/temp\` 之下，且**只有** temp 这一层——
    /// 它们不再平摊在根目录（2026-10-19 用户裁定：根目录只留配置与解码器）。
    #[test]
    fn diagnostics_sit_under_temp() {
        let dir = exe_dir().expect("拿不到 exe 目录");
        let temp = diagnostics_dir().expect("诊断目录路径");
        assert_eq!(temp.parent(), Some(dir), "temp 不是 exe 的直接子目录");
        assert_eq!(temp.file_name().unwrap(), DIAGNOSTICS_DIR);
        for p in [crash_log_path("2026-10-19"), run_log_path("2026-10-19")]
            .into_iter()
            .flatten()
        {
            assert_eq!(
                p.parent(),
                Some(temp.as_path()),
                "诊断物没落在 temp 里：{p:?}"
            );
            assert!(!p.to_string_lossy().contains("AppData"));
        }
    }

    /// 运行期缓存目录在 `<exe 目录>/cache\`：与 temp（取证，按天清理）分工，
    /// 也不许落根目录平摊或外部目录（便携铁律同款判据）。
    #[test]
    fn cache_sits_under_cache_dir() {
        let dir = exe_dir().expect("拿不到 exe 目录");
        let cache = cache_dir().expect("缓存目录路径");
        assert_eq!(cache.parent(), Some(dir), "cache 不是 exe 的直接子目录");
        assert_eq!(cache.file_name().unwrap(), CACHE_DIR);
        assert!(!cache.to_string_lossy().contains("AppData"));
    }

    /// 清理判据读的是名字，所以名字解析必须**只认我们自己写得出的那三族**。
    /// `rupix.run`（状态）与用户手放进 temp 的文件解析成 `None` = 永不删除。
    #[test]
    fn diag_ymd_reads_only_our_own_names() {
        assert_eq!(diag_ymd("rupix-2026-10-19.log"), Some((2026, 10, 19)));
        assert_eq!(diag_ymd("crash-2026-10-19.log"), Some((2026, 10, 19)));
        assert_eq!(
            diag_ymd("crash-2026-10-19-112233.dmp"),
            Some((2026, 10, 19))
        );
        // 不删的五种：状态文件、别人的文件、坏日期、假前缀、旧式紧凑名。
        assert_eq!(diag_ymd("rupix.run"), None);
        assert_eq!(diag_ymd("notes.txt"), None);
        assert_eq!(diag_ymd("crash-2026-13-45.log"), None);
        assert_eq!(diag_ymd("rupix-backup.log"), None);
        assert_eq!(diag_ymd("crash-20261019112233.dmp"), None);
        // ★ 非字符边界的前缀：裸切片会 panic，`get` 版本只会"认不出来"。
        assert_eq!(diag_ymd("crash-中文中文中文.dmp"), None);
        assert_eq!(diag_ymd("rupix-é2026-10-19.log"), None);
    }

    /// 缓存必须生效且稳定：多次调用是同一路径（`OnceLock` 用对了才成立）。
    #[test]
    fn exe_dir_is_cached_and_stable() {
        let a = exe_dir().map(|p| p.to_path_buf());
        let b = exe_dir().map(|p| p.to_path_buf());
        assert_eq!(a, b, "两次调用返回不同目录，缓存写错了");
        // 顺带它必须等于真实的 exe 目录。
        let truth = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()));
        assert_eq!(a, truth, "缓存的目录与 current_exe 不一致");
    }
}
