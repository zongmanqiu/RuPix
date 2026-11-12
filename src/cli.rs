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

/// 无头命令面的五个纯读动词（m293；m317 加 `list-formats`）。
///
/// 全部以 `--<动词>` 选项形式出现，**不做位置参数**：`initial_path` 取首个非 `--`
/// 参数当要打开的文件，位置动词会被它抢走。五个动词都不建窗、不进单实例闸门，
/// 输出由 `headless` 模块统一回一行 JSON。
///
/// `list-formats` 是唯一**不带路径**的动词：它回答「这台机器现在能开哪些后缀」，
/// 输入是解码器目录本身，不是某个文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessCmd {
    List,
    Metadata,
    Thumbnail,
    Render,
    ListFormats,
}

impl HeadlessCmd {
    /// 出现在 JSON `cmd` 字段里的稳定名字；契约的一部分，不许随 UI 文案改名。
    pub const fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Metadata => "metadata",
            Self::Thumbnail => "thumbnail",
            Self::Render => "render",
            Self::ListFormats => "list-formats",
        }
    }

    /// 认选项名；返回 `None` 时交给 `parse` 的未知选项分支。
    pub fn from_option(s: &str) -> Option<Self> {
        match s {
            "--list" => Some(Self::List),
            "--metadata" => Some(Self::Metadata),
            "--thumbnail" => Some(Self::Thumbnail),
            "--render" => Some(Self::Render),
            "--list-formats" => Some(Self::ListFormats),
            _ => None,
        }
    }

    /// 这条动词是否必须带路径。`list-formats` 是唯一例外：它读的是解码器目录，
    /// 不是某个文件；把「没给路径」判成参数错会把一条合法命令变成 GUI。
    pub const fn needs_path(self) -> bool {
        !matches!(self, Self::ListFormats)
    }
}

/// 一条无头请求：动词 + 路径 + `render` 的可选目标框。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headless {
    pub cmd: HeadlessCmd,
    pub path: PathBuf,
    /// `Some((w,h))` 只由 `--render <文件> <宽> <高>` 两数同时给出时产生；
    /// 缺省 `(0,0)` = 按原图尺寸解。其余三个动词恒 `None`。
    pub target: Option<(u32, u32)>,
}

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
    /// 无头命令面（m293）：`--list` / `--metadata` / `--thumbnail` / `--render`。
    pub headless: Option<Headless>,
    /// 无头动词出现过、但没拿到路径。与 `probe_flag` 同理：少了这一位会静默起 GUI。
    pub headless_flag: bool,
    /// 认不出来的 `--*` 选项。解析器**不**自己打日志：warn 属于表现层，
    /// 留在这里，`main` 与测试都能看见同一份事实。
    pub unknown: Vec<String>,
}

/// 是不是一条无头命令。单实例闸门、GUI 装配都由它分流。
pub fn is_headless(args: &Args) -> bool {
    args.headless.is_some()
}

/// 单实例闸门要不要生效：**只有 GUI 才要**。
///
/// 探针与基准绕过闸门是老规矩（进闸门会被转发给首实例、自己反而没跑）；无头命令
/// 同理，而且它连窗口都没有，更不该被「一个进程、一个托盘」的模型收编。把这个判据
/// 放在 lib 里而不是 `main.rs`，是因为 `tests/` 链接不到 bin target——闸门例外必须
/// 有单测盯着，否则哪天少一个分支就是「命令被转发给 GUI、自己没跑」。
pub fn needs_instance_gate(args: &Args) -> bool {
    !is_headless(args) && args.probe.is_none() && args.bench.is_none()
}

/// 一次遍历分辨「离屏探针」「safe-mode」「无头命令」「未知选项」。
///
/// 分段的旧写法会让第二段从头再看一遍 argv，于是探针标志自己被当成未知项——
/// 模块头说的那次事故。带参标志会把**紧跟其后的路径参数**一起吃掉，避免它
/// 落到未知项判定里；而"看起来像次数却不是数字"的那一颗**不吞**，留给调用方
/// 判断（吞掉一个真选项会让脚本误判"参数被接受了"）。
///
/// 要打开的文件路径不在这里取：它由 [`crate::initial_path`] 单独负责——
/// 单实例转发给首实例的也是那份 argv，两处必须共用同一判据。这也是无头动词
/// 一律做成 `--选项` 而不是位置参数的原因。
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
        } else if let Some(cmd) = HeadlessCmd::from_option(a) {
            out.headless_flag = true;
            if !cmd.needs_path() {
                // `--list-formats` 不吃路径：紧跟其后的参数留给 `initial_path`，
                // 但它本来就不建窗，不会被当成「要打开的文件」消费。
                out.headless = Some(Headless {
                    cmd,
                    path: PathBuf::new(),
                    target: None,
                });
                continue;
            }
            let path = it.next().map(PathBuf::from);
            // 只有 `--render` 吃可选目标框；两数必须同时给且都 > 0，缺一按原尺寸。
            let target = if cmd == HeadlessCmd::Render {
                let w = it.peek().and_then(|s| s.parse::<u32>().ok());
                if w.is_some() {
                    it.next();
                }
                let h = if w.is_some() {
                    it.peek().and_then(|s| s.parse::<u32>().ok())
                } else {
                    None
                };
                if h.is_some() {
                    it.next();
                }
                match (w, h) {
                    (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(p) = path {
                out.headless = Some(Headless {
                    cmd,
                    path: p,
                    target,
                });
            }
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

    /// 五个无头动词都认得、都不进 `unknown`；`--render` 的可选目标框要两数齐备。
    #[test]
    fn headless_verbs_parse_with_optional_box() {
        let check = |opt: &str, cmd: HeadlessCmd| {
            let a = parse(&argv(&["rupix.exe", opt, "D:\\dir"]));
            assert_eq!(a.unknown, Vec::<String>::new(), "{opt} 不许自报未知项");
            assert_eq!(a.headless.as_ref().map(|h| h.cmd), Some(cmd));
            assert_eq!(a.headless.as_ref().unwrap().target, None);
            assert!(a.headless_flag);
        };
        check("--list", HeadlessCmd::List);
        check("--metadata", HeadlessCmd::Metadata);
        check("--thumbnail", HeadlessCmd::Thumbnail);
        check("--render", HeadlessCmd::Render);

        // `--list-formats` 是唯一不带路径的动词：没有路径也必须成为一条请求，
        // 不能落进 `main` 的「标志出现过但没给路径」参数错分支。
        let f = parse(&argv(&["rupix.exe", "--list-formats"]));
        assert_eq!(f.unknown, Vec::<String>::new());
        assert_eq!(
            f.headless.as_ref().map(|h| h.cmd),
            Some(HeadlessCmd::ListFormats)
        );
        assert!(f.headless.as_ref().unwrap().path.as_os_str().is_empty());
        assert!(f.headless_flag);

        let r = parse(&argv(&["rupix.exe", "--render", "D:\\a.png", "800", "600"]));
        assert_eq!(r.headless.as_ref().unwrap().target, Some((800, 600)));
        // 只给一个数 / 给了 0：不吃成目标框，按原尺寸解；真选项仍留在循环里。
        let one = parse(&argv(&[
            "rupix.exe",
            "--render",
            "D:\\a.png",
            "800",
            "--safe-mode",
        ]));
        assert_eq!(one.headless.as_ref().unwrap().target, None);
        assert!(one.safe_mode, "--safe-mode 不该被当成高度吃掉");
        let zero = parse(&argv(&["rupix.exe", "--render", "D:\\a.png", "0", "0"]));
        assert_eq!(zero.headless.as_ref().unwrap().target, None);
    }

    /// 无头动词缺路径：标志留下了、请求为 None，`main` 据此回退出码 2。
    #[test]
    fn headless_flag_without_path_degrades_to_none() {
        let a = parse(&argv(&["rupix.exe", "--list"]));
        assert!(a.headless_flag);
        assert_eq!(a.headless, None);
        assert!(a.unknown.is_empty());
        // 负向臂：`--list-formats` 不带路径是**合法**的，不许跟着降级成 None。
        let f = parse(&argv(&["rupix.exe", "--list-formats"]));
        assert!(f.headless_flag);
        assert!(f.headless.is_some(), "无路径动词不许被判成缺路径");
    }

    /// 闸门例外：五个动词一律绕开，GUI 一律进，老探针 / 基准照旧绕开。
    /// ★ 负向臂：把 `is_headless` 这一项从判据里拿掉（即旧公式），五个动词就会
    /// 被判成"要进闸门"——那正是「命令被转发给 GUI、自己没跑」的老故障。
    #[test]
    fn headless_bypasses_instance_gate_with_negative_arm() {
        for cmd in [
            HeadlessCmd::List,
            HeadlessCmd::Metadata,
            HeadlessCmd::Thumbnail,
            HeadlessCmd::Render,
            HeadlessCmd::ListFormats,
        ] {
            let a = Args {
                headless: Some(Headless {
                    cmd,
                    path: PathBuf::from("a.png"),
                    target: None,
                }),
                ..Default::default()
            };
            assert!(!needs_instance_gate(&a), "{cmd:?} 不该进闸门");
            let old_formula = a.probe.is_none() && a.bench.is_none();
            assert!(
                old_formula,
                "负向臂：漏掉 is_headless 时 {cmd:?} 会被误判进闸门"
            );
        }
        let gui = Args::default();
        assert!(needs_instance_gate(&gui), "普通 GUI 仍要进闸门");
        let probe = Args {
            probe: Some(PathBuf::from("a.png")),
            ..Default::default()
        };
        assert!(!needs_instance_gate(&probe), "老探针通道不许被改坏");
        let bench = Args {
            bench: Some((PathBuf::from("a.png"), 3)),
            ..Default::default()
        };
        assert!(!needs_instance_gate(&bench), "老基准通道不许被改坏");
    }

    /// 新增五个动词后，老的 `--probe-decode` / `--bench-thumb` 解析逐值不变。
    #[test]
    fn legacy_probe_channels_unchanged() {
        let a = parse(&argv(&["rupix.exe", "--probe-decode", "D:\\a.tif"]));
        assert_eq!(a.probe.as_deref(), Some(std::path::Path::new("D:\\a.tif")));
        assert!(!a.headless_flag && a.headless.is_none());
        let b = parse(&argv(&["rupix.exe", "--bench-thumb", "D:\\b.png", "50"]));
        assert_eq!(b.bench, Some((PathBuf::from("D:\\b.png"), 50)));
        assert!(!b.headless_flag);
    }
}
