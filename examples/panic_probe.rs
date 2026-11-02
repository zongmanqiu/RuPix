//! panic 通道的端到端验证：真的 panic 一次，看 `platform::panic_guard` 有没有把
//! **转储 + 调用链 + 怎么查的指路**一起落盘。
//!
//! 与 `crash_probe.rs` 是一对：那枚走未处理异常（SEH），这枚走 Rust 的 `panic!`。
//! 两条通道不相交（见 `crash_evidence` 模块头），所以各要一枚探针——只有 SEH 那枚时，
//! "panic 里少写了栈帧"这种退化不会被任何自动化发现。
//!
//! 用法（release 才有意义，发布版 `strip = true`，正是要验证"用户机上没有 pdb 也能取证"）：
//!
//! ```text
//! cargo run --release --example panic_probe
//! ```
//!
//! 期望：exit code 101；exe 旁的 `temp\` 里 `crash-<今天>.log` 多一段，里面有三样东西——
//! `转储：…dmp`、`--- 栈帧`（每行 `模块+偏移`）、`dump_sym.py` 那一行指路。
//!
//! ★ 故意在**非 UI 线程**上 panic：`panic_guard` 只对 UI 线程的 panic 阻塞等用户点掉弹框
//!   （那条路会让探针挂住、也真的会往桌面上立一个必须手点的窗）。后台线程崩了进程还活着，
//!   框自己立在旁边，探针可以随即退出。

fn main() {
    rupix_lib::platform::panic_guard::install();
    let log = rupix_lib::platform::panic_guard::log_path();
    println!("崩溃日志应落在：{:?}", log);

    let handle = std::thread::Builder::new()
        // 具名线程：报告里那一行"出错线程"读的就是这个名字，别让它变成 <unnamed>。
        .name("panic-probe".to_string())
        .spawn(|| {
            // 这一行在报告里应当能解出 `main/examples/panic_probe.rs:<行>`。
            panic!("panic 探针（这是故意的）");
        })
        .expect("起不了线程");
    // 钩子在 panic 发生的那条线程上同步跑完才继续 unwind，join 就是"等证据写完"。
    let _ = handle.join();

    println!("panic 已被接住（进程还活着，说明这条通道没走到 abort）");
    println!("退出码 101 = Rust 的 panic 约定，脚本可以据此断言");
    std::process::exit(101);
}
