//! 崩溃取证的端到端验证：真的制造一次访问冲突（SEH），看 `platform::crash_evidence`
//! 有没有把**报告 + .dmp 转储**落盘。
//!
//! 存在理由：2026-10-17 用户提"只有 log 不够，要 dmp 那种策略"。这条探针就是判据本身——
//! 装完 filter 却写不出东西、或写出的报告缺基址/指纹，都在这里当场暴露，不用等真崩。
//!
//! 用法（release 才有意义：发布版 `strip = true`，正是要验证"无符号也能取证"）：
//!
//! ```text
//! cargo run --release --example crash_probe
//! ```
//!
//! 期望：进程真的带着访问冲突退出（控制台最后一行看不见），而 example 旁的 `temp\`
//! 里留下 `crash-<今天>.log`（一段报告）与 `crash-<今天>-<时分秒>.dmp`。
//!
//! 只装 SEH filter、**不装 panic 钩子**：那条路会 `MessageBoxW`，探针不该在桌面上抢一个窗。

fn main() {
    rupix_lib::platform::crash_evidence::install();
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    println!("产物落点：{}", dir.display());
    // 诊断物一律在 exe 旁的 `temp\` 里、按天命名（`core::paths` 的落点规则）。
    // 探针**不调 `prepare_diagnostics`**（它只装 filter 就崩），所以这一枚目录是
    // 写入点自己建的——正好验证"每个写路径自己保证目录在"这条不是空话。
    println!(
        "崩溃报告应落在：{:?}",
        rupix_lib::platform::panic_guard::log_path()
    );
    let temp = dir.join("temp");
    println!(
        "（崩之前）已有转储：{} 枚于 {}",
        std::fs::read_dir(&temp)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".dmp"))
                    .count()
            })
            .unwrap_or(0),
        temp.display()
    );
    std::io::Write::flush(&mut std::io::stdout()).ok();

    // 0xC0000005：往空指针做一次 volatile 写。`write_volatile` 保证优化不会把它当死代码
    // 删掉——删掉的话这条探针就成了"没测却说测过"。
    unsafe {
        std::ptr::write_volatile(std::ptr::null_mut::<u8>(), 0xFF);
    }
    println!("不该走到这里：访问冲突没发生？");
}
