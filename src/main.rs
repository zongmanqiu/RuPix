//! RuPix 入口。
//!
//! 与 CurSeen 同一范式：`main` 只做"装日志 → 解析命令行 → 交给库"，
//! 全部装配在 `rupix_lib::run` 里，方便将来加测试与探针直接调库。

// 发布版不弹控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 日志：形状、默认档、落盘 tee 全在 `platform::logging` 里定（那里写着为什么时间戳
    // 要自己拼、为什么 fontdb 压到 error、为什么不打 ANSI 颜色）。
    rupix_lib::platform::logging::install_logging();

    // 命令行解析：整段搬到 `rupix_lib::cli`（`tests/` 链接不到 `bin`，留在这里的每一行
    // 都等于没测——这里出过一次「`--probe-decode` 把自己判成未知项」的回归，
    // 2026-10-17 审计 §三b）。未知选项仍然只在这条真入口 warn：命令面的输出
    // 要可信，脚本按「有 warn 就算异常」判读。
    let argv: Vec<String> = std::env::args().collect();
    let args = rupix_lib::cli::parse(&argv);
    for opt in &args.unknown {
        log::warn!("忽略未知命令行选项：{opt}");
    }
    // 带了标志却没给文件：这不是"打开一张默认图"，更不该落到 GUI 静默开一个
    // 空白窗（脚本会以为探针跑了）。用法打到 stderr、退出码 2，与探针失败同档。
    if args.probe_flag && args.probe.is_none() {
        eprintln!("用法：rupix.exe --probe-decode <文件>");
        std::process::exit(2);
    }
    if args.bench_flag && args.bench.is_none() {
        eprintln!("用法：rupix.exe --bench-thumb <文件> [次数]");
        std::process::exit(2);
    }
    // 无头命令面缺路径也回一行 JSON（cmd:null / code:2），不打破「stdout 恰好一行」。
    // 打印只发生在 bin 真入口：lib 里的 `headless` 是纯函数，卫生门禁不许它带打印宏。
    if args.headless_flag && args.headless.is_none() {
        let (code, line) = rupix_lib::headless::arg_error(
            "用法：rupix.exe --list|--metadata|--thumbnail|--render <路径>",
        );
        println!("{line}");
        std::process::exit(code);
    }

    // 单实例闸门：放在一切重活（装解码栈、起线程、建窗）之前。
    // 二次实例（已经有一个进程在跑）会把 argv 转发给首实例、自己直接退出，
    // 绝不新建第二个进程或第二个托盘——这正是「一个进程、一个托盘」的根基。
    // 探针与无头命令例外：它们不建窗，进闸门就会被转发给 GUI、自己反而没跑。
    // 判据在 `cli::needs_instance_gate`（lib 里，有单测盯着），不是这里的裸条件。
    // `app_id` 必须与 `runtime::run` 里 `App::single_instance` 完全一致，否则会把自己
    // 误判成二次实例、转发给自己、窗口永不出现。
    //
    // ★ 多开（2026-11-04 用户裁定）：多开的意思是**窗口可以有多个，进程与托盘都只有一个**，
    //   所以闸门恒生效——多开与否的分岔在首实例收到 argv 之后（`runtime` 的 `second_tx`：
    //   开新窗口 / 在当前窗口换图），不在"要不要起第二个进程"。
    if rupix_lib::cli::needs_instance_gate(&args)
        && windui::claim_instance(rupix_lib::APP_ID) == windui::InstanceRole::Handoff
    {
        return;
    }

    let initial = rupix_lib::initial_path(&argv);

    // 必须在解码线程起来之前置位（Host 每次调用都会读它）。
    rupix_lib::decoders::set_safe_mode(args.safe_mode);
    // 格式清单装入 core::formats（幂等）：目录清单/裸数据路由在任何 GUI 之前就绪。
    rupix_lib::decoders::install_format_source();
    // 解码后端 + SVG 渲染器：GUI、老探针与无头命令面**共用同一条链路**，
    // 全进程只在这里装一次；无头模块不许另起后端。
    rupix_lib::install_decode_stack();

    // 无头命令面（m293）：四个纯读动词，各回一行 JSON 后退出，不建窗、不进托盘。
    // 排在老探针之前是因为它自带退出码；老两条通道一个字没改。
    if let Some(h) = args.headless.as_ref() {
        let (code, line) = rupix_lib::headless::run(h);
        println!("{line}");
        std::process::exit(code);
    }

    // 探针：离屏解码走与打开图片相同的解码链路，打印结果后退出，不建窗口。
    if let Some(p) = args.probe.as_ref() {
        probe_decode(p);
        return;
    }
    if let Some((p, reps)) = args.bench.as_ref() {
        bench_thumb(p, *reps);
        return;
    }

    rupix_lib::run(initial);
}

/// `--probe-decode <文件>`：走与打开图片完全相同的解码链路（SVG → 内置 resvg；
/// 其余 → 解码器 DLL），把结果打到 stdout 后退出。用于在没有 GUI 交互的环境里
/// 验证解码器是否真的接通。
///
/// 失败以非 0 退出码结束，方便脚本断言。
fn probe_decode(path: &std::path::Path) {
    match rupix_lib::core::decode::probe_decode(path, 0, 0) {
        Ok(d) => {
            println!(
                "OK natural={}x{} bitmap={}x{} bytes={}",
                d.natural.0,
                d.natural.1,
                d.bitmap.width,
                d.bitmap.height,
                d.bitmap.rgba.len()
            );
        }
        Err(e) => {
            eprintln!("FAIL {e}");
            std::process::exit(2);
        }
    }
}

/// `--bench-thumb <文件> [reps]`：缩略图两条解码腿的**生产链路**配对 A/B（默认 200 次）。
///
/// 两臂只差一个缩放意图：`full` = 主画面口径（`ReqKind::Main`，整解 + 宿主补齐），
/// `scaled` = 缩略图口径（`ReqKind::Thumb`，声明了 `CAP_SCALED` 的解码器走原生降采样）。
/// 同一 rep 内两臂相邻执行，环境漂移对两臂同向；`RUPIX_BENCH_REVERSE=1` 翻转臂序，
/// 正反两序各跑一轮、方向一致才算数（本仓历史记录 的判据口径）。
///
/// 每一轮都是真解码：磁盘缓存那层 2026-10-19 已撤，第二条起也不会"量到读缓存"。
/// 逐次台账打到 stdout（`rep,arm,ms,natural,bitmap`），末行给
/// median 与倍率——读数的人自己核对，不做黑箱汇总。
fn bench_thumb(path: &std::path::Path, reps: usize) {
    use std::time::Instant;

    let w = rupix_lib::core::thumbs::THUMB_W;
    let h = rupix_lib::core::thumbs::THUMB_H;
    let reverse = std::env::var_os("RUPIX_BENCH_REVERSE").is_some();
    println!(
        "file={} reps={reps} box={w}x{h} reverse={reverse}",
        path.display()
    );
    println!("rep,arm,ms,natural,bitmap");

    let mut ledger: std::collections::BTreeMap<&'static str, Vec<f64>> =
        std::collections::BTreeMap::new();
    for rep in 0..reps {
        let arms: [&str; 2] = if reverse {
            ["scaled", "full"]
        } else {
            ["full", "scaled"]
        };
        for arm in arms {
            let t = Instant::now();
            let r = match arm {
                "full" => rupix_lib::core::decode::probe_decode(path, w, h),
                _ => rupix_lib::core::decode::probe_thumb(path, w, h),
            };
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(d) => {
                    println!(
                        "{rep},{arm},{ms:.3},{}x{},{}x{}",
                        d.natural.0, d.natural.1, d.bitmap.width, d.bitmap.height
                    );
                    ledger.entry(arm).or_default().push(ms);
                }
                Err(e) => {
                    eprintln!("FAIL arm={arm}: {e}");
                    std::process::exit(2);
                }
            }
        }
    }

    let median = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    for (arm, v) in &ledger {
        println!(
            "SUMMARY arm={arm} n={} median_ms={:.3}",
            v.len(),
            median(v.clone())
        );
    }
    if let (Some(f), Some(s)) = (ledger.get("full"), ledger.get("scaled")) {
        let (mf, ms) = (median(f.clone()), median(s.clone()));
        if ms > 0.0 {
            println!("SUMMARY ratio_full_over_scaled={:.3}", mf / ms);
        }
    }
}
