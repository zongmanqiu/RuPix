//! 诊断头探针：把会话环境块打到 stdout，并验证它确实落进了 `rupix.log`。
//!
//! 为什么需要它：会话头的全部价值是"**出事时那行字在盘上**"，而生产入口 `run()`
//! 会建窗口——在共享桌面上起 GUI 再关，收尾要按 PID 收拾（本项目已立着"关窗后进程
//! 滞留"的缺陷单）。这个 example 不建窗口：调同一组函数，判据一模一样。
//!
//! 跑法：`cargo run --release --example env_probe`
//! 落点是**本 example 自己的 exe 同级** `rupix.log`（在 `.temp\target\...\examples\`），
//! 不碰 `.build\` 那份真实日志。

use std::io::Write as _;

fn main() {
    let preamble = rupix_lib::platform::env_report::startup_preamble();
    println!("--- 会话头（将写进 rupix.log）---");
    print!("{preamble}");

    let log = rupix_lib::core::paths::run_log_path(&rupix_lib::platform::panic_guard::today())
        .expect("取不到日志路径，探针无意义");
    rupix_lib::platform::crash_evidence::append_run_log(&preamble);
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    println!("--- 落盘核对 ---");
    println!("log            = {}", log.display());
    println!("bytes          = {}", text.len());
    println!(
        "preamble 在盘上 = {}",
        if text.contains("=== RuPix 会话开始 ===") {
            "YES"
        } else {
            "NO —— 写路径没生效"
        }
    );
    // 崩溃报告读的是**冻结**的那份（不在 SEH filter 里现采）。这里模拟崩溃路径的取法。
    match rupix_lib::platform::env_report::frozen() {
        Some(s) => println!(
            "frozen()       = {} 行（崩溃报告用这一份）",
            s.lines().count()
        ),
        None => {
            println!("frozen()       = None —— 采集没发生，崩溃报告那一节会写「未采集」");
            std::process::exit(2);
        }
    }
    // 报告里的分隔线形状（不真崩：把 snapshot 的组装走一遍太重，这里只核前言本身）。
    if !preamble.ends_with("=== 以下才是日志 ===\n") {
        println!("[WARN] 前言结尾不是预期的分隔线");
        let _ = std::io::stdout().flush();
        std::process::exit(3);
    }
}
