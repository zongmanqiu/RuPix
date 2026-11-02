//! 日志的**唯一**安装入口：形状、级别、落盘三件事只在这里定一次。
//!
//! # 为什么形状要自己写（而不是 `format_timestamp_millis()`）
//!
//! `Cargo.toml` 里 `env_logger` 是 `default-features = false` 且**一个特性都不开**。
//! 而 env_logger 0.11 的 feature 表是 `default = ["auto-color", "humantime", "regex"]`，
//! **时间戳的实现挂在 `humantime` 上**（那枚 feature 的实现又是一整颗 `jiff`）。没开它时
//! `write_timestamp()` 里那句 `let _ = self.format.timestamp; Ok(())` 会让
//! `builder.format_timestamp_millis()` **编译通过、调用成功、一个字节都不输出**——
//! 这就是"日志里没有时刻"的全部原因，也是这条 API 最坑的地方：静默失效。
//!
//! 打开那个 feature 等于为四个数字新引一枚日期时间库，所以自己拼：时刻来自
//! [`crate::platform::panic_guard::clock_text`]（本地口径，与会话戳、崩溃报告同一套历法——
//! 那条腿是为了"很久以前"这种差 8 小时的误报修出来的，见 m39）。
//!
//! # 为什么**不打颜色**
//!
//! 同一份字节既进 stderr 又进 `temp\rupix-<日期>.log`。ANSI 转义码进控制台是帮助，
//! 进日志文件就是取证时要肉眼剔除的噪音，而 `Target::Pipe` 只给我们一个 `Write`，
//! 分不清对面是哪一种。所以 `Cargo.toml` 里连 `auto-color` 都不开——那条腿只服务
//! env_logger 自带的着色 formatter，我们装的是纯文本闭包；关掉它顺带摘掉
//! anstream / anstyle-wincon 一整串终端着色依赖（m95 实测，账见 本仓历史记录）。
//!
//! # 为什么探针也调这一处
//!
//! `main.rs`（GUI 与 `--probe-*` / `--bench-*` 都在它里面）与 `examples/*` 都从这里装日志：
//! **看到的形状必须和生产落盘的形状是同一份**。否则"探针读到的耗时行"与
//! "用户发回来的日志"对不上，每次取证都要重做一遍对账。
//!
//! # 级别口径
//!
//! 默认 [`DEFAULT_FILTER`]（`info`）：一次正常会话必须在日志里留下"打开了哪张、每张等了
//! 多久、关窗怎么裁决的"，只有 warn 的日志等于没有日志。高频/内部的判词（解码分派、
//! 五段耗时、请求作废）留在 `debug`，按需点亮（`RUST_LOG=debug` + `RUPIX_DECODE_TRACE=1`）。

use std::fmt::Display;
use std::io::Write as _;

/// 默认过滤档。`fontdb=error` 是**日志卫生**而不是遮掩：字体库每次启动都 warn 一条
/// 「Failed to load a …」（系统字体里总有读不动的文件），而它是**第三方 crate** 的 warn，
/// 落在我们"日常噪音不进日志"的口径里就成了文件的第一批干扰——自家 warn 一条不少，
/// 只把这一家的降到 error（2026-10-17 审计 §三b 的既定结论，默认档提到 info 之后照旧成立）。
pub const DEFAULT_FILTER: &str = "info,fontdb=error";

/// 装日志：定形状 → 接落盘 tee → 交给 `log` 门面。
///
/// 一个进程**只能调一次**（`env_logger` 的 `init()` 对二次安装是 panic 的）：真入口
/// `main.rs` 的第一件事、每个 `examples/*` 的第一件事——它们分属不同可执行文件，互不重叠。
pub fn install_logging() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(DEFAULT_FILTER));
    // sink 是我们自己的 tee（`platform::crash_evidence::RunLogSink`）：发布版是 GUI 子系统、
    // stderr 没人看得见，而崩溃报告要引用"崩之前在做什么"，所以每条同时落 exe 旁
    // `temp\rupix-<今天>.log`（按天命名，写满即停）。取不到目录时退回纯 stderr。
    if let Some(sink) = crate::platform::crash_evidence::RunLogSink::new() {
        builder.target(env_logger::Target::Pipe(Box::new(sink)));
    }
    builder.format(|out, record| {
        writeln!(
            out,
            "{}",
            format_line(
                &crate::platform::panic_guard::clock_text(),
                record.level(),
                record.target(),
                record.args()
            )
        )
    });
    builder.init();
}

/// 一行的形状：`[HH:MM:SS.mmm] [ LEVEL] target: message`。
///
/// 级别右对齐到 5（`INFO` 前补一格，`DEBUG`/`ERROR`/`TRACE` 正好）——定宽的级别列让一屏
/// 日志能竖着读；时刻不带日期（文件名里就有）；`target` 用 `log` 给的模块路径原样，
/// 不在这里截短，否则同一个模块在 grep 里会分成两家。
///
/// `at` 作参数进来（本仓"时钟作参数注入"的定式，`viewer::upgrade_can_wait`、`tick_hide`
/// 的光标锚点同理）：形状能被逐字节断言，而不是只能靠肉眼看出一次窗。
fn format_line(at: &str, level: log::Level, target: &str, msg: &dyn Display) -> String {
    format!("[{at}] [{level:>5}] {target}: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 级别列必须**定宽右对齐**：四种级别的 `]` 落在同一列。
    /// ★ 这条断的是 `format_line` 本体——把 `{level:>5}` 改成 `{level}` 立刻红。
    #[test]
    fn level_column_is_five_wide_right_aligned() {
        let at = "23:59:59.780";
        for (lvl, want) in [
            (log::Level::Info, "[ INFO]"),
            (log::Level::Warn, "[ WARN]"),
            (log::Level::Error, "[ERROR]"),
            (log::Level::Debug, "[DEBUG]"),
            (log::Level::Trace, "[TRACE]"),
        ] {
            assert_eq!(
                format_line(at, lvl, "t", &"m"),
                format!("[{at}] {want} t: m"),
                "{lvl} 那一档的形状不对"
            );
        }
    }

    /// 整条线（时刻 + 级别 + target + 正文）逐字节钉住——**出货的就是这一串**。
    #[test]
    fn line_shape_is_pinned_byte_for_byte() {
        assert_eq!(
            format_line(
                "08:00:00.000",
                log::Level::Info,
                "rupix_lib::decoders",
                &"已加载解码器：TIFF (tiff crate)（tiff）"
            ),
            "[08:00:00.000] [ INFO] rupix_lib::decoders: 已加载解码器：TIFF (tiff crate)（tiff）"
        );
    }

    /// 默认档必须是 `info`，且带着 `fontdb=error` 那条卫生。
    /// ★ 这条是"正文不为空"的门禁：改回 `warn` 就等于把一次正常会话重新写成 0 行。
    #[test]
    fn default_filter_reports_a_normal_session() {
        assert!(
            DEFAULT_FILTER.starts_with("info"),
            "默认档不是 info：{DEFAULT_FILTER}"
        );
        assert!(
            DEFAULT_FILTER.contains("fontdb=error"),
            "fontdb 的第三方噪音没压住：{DEFAULT_FILTER}"
        );
    }

    /// `clock_text` 的**形状**（真时刻只能在这台机器上取，所以钉形状不钉值）：
    /// 12 个字符、`HH:MM:SS.mmm`，每个位置上的字符类型各就各位。
    #[test]
    fn clock_text_has_the_logged_shape() {
        let s = crate::platform::panic_guard::clock_text();
        assert_eq!(s.len(), 12, "时刻应是 HH:MM:SS.mmm：{s}");
        for (i, c) in s.char_indices() {
            let want_digit = !matches!(i, 2 | 5 | 8);
            assert_eq!(
                c.is_ascii_digit(),
                want_digit,
                "第 {i} 位应是{}，实际 {c:?}（整串 {s}）",
                if want_digit { "数字" } else { "冒号或点" }
            );
        }
    }
}
