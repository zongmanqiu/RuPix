//! 命令行解析（`main` 的全部决策，搬到库里来）。
//!
//! # 为什么不在 `main.rs` 里解析
//!
//! `tests/` 只能链接 **lib target**，`bin` 里的代码对集成测试**不可见**（总纲 §2.4）。
//! 这段解析曾经踩过一次真坑：`--probe-decode` 自己被判成"未知选项"打了一条 warn，
//! 而脚本按「有 warn 就算异常」判读探针输出（2026-10-17 审计 §三b）。修的时候是
//! 手工改两遍循环、手工跑一遍确认——正因为它没法被单测盯着。现在它是 [`parse`]，
//! 那条回归有自己的用例。

use std::path::PathBuf;

/// `--bench-thumb` 缺省重复次数：够中位数稳（本项目配对基准的口径是 n≥200）。
pub const BENCH_DEFAULT_REPS: usize = 200;

/// 解析结果。字段彼此独立，调用方按用途各取所需。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// `--probe-decode <文件>`：离屏解码探针（不建窗口）。路径缺省 = 探针无参，
    /// 由 `main` 打用法并以非 0 退出——标志出现过要看 [`Self::probe_flag`]。
    pub probe: Option<PathBuf>,
    /// `--bench-thumb <文件> [reps]`：缩略图两腿配对基准。
    pub bench: Option<(PathBuf, usize)>,
    /// `--probe-decode` 是否出现过（含不带值）。少了这一位就分不清
    /// "没要探针"和"要了探针但没给文件"——后者旧行为会静默起一个空白 GUI。
    pub probe_flag: bool,
    /// `--bench-thumb` 是否出现过（含不带值）。语义同 [`Self::probe_flag`]。
    pub bench_flag: bool,
    /// `--safe-mode`：停用全部第三方解码器（`crate::decoders::set_safe_mode`）。
    pub safe_mode: bool,
    /// 认不出来的 `--*` 选项。解析器**不**自己打日志：warn 属于表现层，
    /// 留在这里，`main` 与测试都能看见同一份事实。
    pub unknown: Vec<String>,
}

/// 一次遍历分辨「离屏探针」「safe-mode」「未知选项」。
///
/// 分段的旧写法会让第二段从头再看一遍 argv，于是探针标志自己被当成未知项——
/// 模块头说的那次事故。三个带参标志会把**紧跟其后的路径参数**一起吃掉，避免它
/// 落到未知项判定里；而"看起来像次数却不是数字"的那一颗**不吞**，留给调用方
/// 判断（吞掉一个真选项会让脚本误判"参数被接受了"）。
///
/// 要打开的文件路径不在这里取：它由 [`crate::initial_path`] 单独负责——
/// 单实例转发给首实例的也是那份 argv，两处必须共用同一判据。
pub fn parse(argv: &[String]) -> Args {
    let mut out = Args::default();
    let mut it = argv.iter().skip(1).peekable();
    while let Some(a) = it.next() {
        if a == "--probe-decode" {
            out.probe_flag = true;
            out.probe = it.next().map(PathBuf::from);
        } else if a == "--bench-thumb" {
            out.bench_flag = true;
            let path = it.next().map(PathBuf::from);
            let reps = it.peek().and_then(|s| s.parse::<usize>().ok());
            if reps.is_some() {
                it.next();
            }
            if let Some(p) = path {
                out.bench = Some((p, reps.unwrap_or(BENCH_DEFAULT_REPS)));
            }
        } else if a == "--safe-mode" {
            out.safe_mode = true;
        } else if a.starts_with("--") {
            out.unknown.push(a.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// 探针标志不许把自己报成未知项（2026-10-17 审计 §三b 的那次事故）。
    #[test]
    fn probe_flag_is_not_unknown() {
        let a = parse(&argv(&["rupix.exe", "--probe-decode", "D:\\a.tif"]));
        assert_eq!(a.probe.as_deref(), Some(std::path::Path::new("D:\\a.tif")));
        assert!(
            a.unknown.is_empty(),
            "探针命令面的 warn 会骗过脚本判读：{:?}",
            a.unknown
        );
    }

    /// 选项值彼此不许串台：`--bench-thumb` 的次数跟着走，后面的文件仍归 `--probe-decode`。
    #[test]
    fn bench_takes_optional_reps_only_when_numeric() {
        let a = parse(&argv(&["rupix.exe", "--bench-thumb", "D:\\b.png", "50"]));
        assert_eq!(
            a.bench,
            Some((PathBuf::from("D:\\b.png"), 50)),
            "显式次数要生效"
        );
        let b = parse(&argv(&["rupix.exe", "--bench-thumb", "D:\\b.png"]));
        assert_eq!(
            b.bench,
            Some((PathBuf::from("D:\\b.png"), BENCH_DEFAULT_REPS)),
            "缺省次数 = 配对基准的中位数口径"
        );
        // 不是数字的那一颗留着：吞掉它等于让脚本以为"参数被接受了"。
        let c = parse(&argv(&[
            "rupix.exe",
            "--bench-thumb",
            "D:\\b.png",
            "--safe-mode",
        ]));
        assert_eq!(
            c.bench,
            Some((PathBuf::from("D:\\b.png"), BENCH_DEFAULT_REPS))
        );
        assert!(c.safe_mode, "--safe-mode 不该被当成次数吃掉");
    }

    /// 标志后面空了：`--probe-decode` / `--bench-thumb` 不带路径 = 探针无参，
    /// 不许 panic，也不许凭空造出一个空路径；但**标志出现过**这件事必须留下，
    /// 否则调用方分不清"没要探针"和"要了探针却没给文件"（后者会静默开 GUI）。
    #[test]
    fn flags_without_value_degrade_to_none() {
        let a = parse(&argv(&["rupix.exe", "--probe-decode"]));
        assert_eq!(a.probe, None);
        assert!(a.probe_flag, "标志出现过要能被调用方看见");
        assert!(a.unknown.is_empty());
        let b = parse(&argv(&["rupix.exe", "--bench-thumb"]));
        assert_eq!(b.bench, None, "没有文件路径就不该有基准");
        assert!(b.bench_flag);
    }

    /// 没有任何探针标志时两个"出现位"必须是假——否则正常启动会被当成缺参。
    #[test]
    fn absent_flags_stay_false() {
        let a = parse(&argv(&["rupix.exe", "D:\\c.jpg"]));
        assert!(!a.probe_flag && !a.bench_flag);
    }

    /// 真未知项要逐条列出来（调用方负责 warn），且不影响其余选项。
    #[test]
    fn unknown_options_are_collected_not_swallowed() {
        let a = parse(&argv(&[
            "rupix.exe",
            "--nope",
            "--safe-mode",
            "--also-bad",
            "D:\\c.jpg",
        ]));
        assert_eq!(
            a.unknown,
            vec!["--nope".to_string(), "--also-bad".to_string()]
        );
        assert!(a.safe_mode);
    }

    /// 文件名（要打开的那张）由 `initial_path` 负责，与解析器共用同一份 argv：
    /// 选项在前、文件在后也认得。
    #[test]
    fn initial_path_agrees_with_parser() {
        let v = argv(&["rupix.exe", "--safe-mode", "D:\\c.jpg"]);
        assert_eq!(
            crate::initial_path(&v).as_deref(),
            Some(std::path::Path::new("D:\\c.jpg"))
        );
        assert!(parse(&v).safe_mode, "两处判据必须同时成立");
    }
}
