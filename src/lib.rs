//! RuPix —— Windows 上小巧极速的看图器。
//!
//! 结构照 CurSeen 的套路分层，但按"看图器"这个更小的目标做了裁剪：
//!
//! ```text
//! core::gallery   目录清单与导航（打开一张 = 打开它所在的目录）
//! core::decode    后台解码线程 + 世代令牌（过期结果丢弃）
//! core::viewer    状态机：缩放 / 平移 / 该不该重新解码
//! core::pixels   解码产物类型（Decoded/Bitmap/Region，解码实现在 decoders/ 的 DLL）
//! ui::canvas      自定义控件：把图按当前倍率画到视口
//! runtime::run    运行时装配（App + 轮询，最顶层）
//! ```
//!
//! # 明确不做
//!
//! PDF、视频、音频一律不在范围内——它们是另外的文档模型与播放管线，
//! 塞进"看图器"只会让体积和复杂度失控。

pub mod cli;
pub mod core;
pub mod decoders;
pub mod extensions;
pub mod platform;
pub mod runtime;
pub mod ui;

/// 单实例标识：首实例持 `Local\{APP_ID}_si_mutex` 命名 Mutex，二次实例把 argv 转发给首实例后退出。
///
/// 必须同时出现在两处且完全一致：
/// - `main` 的 `windui::claim_instance(APP_ID)`（启动早期闸门，二次实例直接 return）；
/// - `runtime` 的 `App::single_instance(APP_ID, …)`（首实例在此收另一进程的 argv）。
///   两处不一致会导致「自己把自己判成二次实例、转发给自己、窗口永不出现」。
pub const APP_ID: &str = "rupix";

/// 从命令行 argv 抽取「要打开的文件」路径。
///
/// 规则与主入口 `main` 的解析对齐：**跳过 `argv[0]`（程序名）**，取首个不以 `--` 开头的参数。
///
/// 单实例模式下，二次运行的进程把整段 argv 转发给首实例，首实例用同一份判定决定开哪张图——
/// 这正是「双击图片永远落在同一个运行中的窗口/托盘、绝不冒出第二个实例」的关键：首实例
/// 关闭到托盘后进程仍在跑（Mutex 仍被持有），后续再打开图片只会被转发、不会新建窗口或托盘。
pub fn initial_path(argv: &[String]) -> Option<std::path::PathBuf> {
    argv.iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(std::path::PathBuf::from)
}

/// 装配解码栈：解码器后端 + SVG 渲染器（均幂等，重复调用无害）。
///
/// GUI 入口（`runtime::run`）、命令行探针（`main.rs --probe-decode`）与示例
/// 统一调这一处——core 的调度（`core::decode`）不 import 解码器与 windui，
/// 两者的实现由这里在运行期装入。
///
/// ★ SVG 渲染器**只服务内置 svg/svgz**：解码器认领的矢量格式一律由解码器自己
/// 按目标尺寸光栅化（宪章 §5.2），宿主不参与它们的像素。
pub fn install_decode_stack() {
    core::decode::install_backend(Box::new(crate::decoders::Host::new()));
    // ★ 挂死恢复（2026-11-04）：每个解码 worker 用**自己那份** Host。
    // 进程级单例那把锁的 guard 覆盖 probe + 查询 + 渲染整段 FFI，一旦某个解码器在
    // `iv_render` 里死循环，锁就再也放不回来——那正是"打开一张图卡住，之后所有图
    // 都打不开"的机制。工厂只在 worker 重建时调用（正常启动一次、每次挂死恢复一次）。
    core::decode::install_backend_factory(|| Box::new(crate::decoders::Host::new()));
    core::decode::set_svg_decoder(ui::svg::decode);
    // ★ m187 —— 字体索引持久缓存落点：exe 旁 `cache\svg-fonts.bin`（缓存目录，
    // 不是 `temp\`——那是按天清理的取证区，纯提速件不住那）。必须在首次 SVG 解析
    // 前调（windui 只认第一次）；首次冷启动整扫并落盘（本机实测扫描 260–950 ms，
    // 随机器状态浮动），之后每次启动回放（实测 11 ms）。`cache\` 建不了（只读
    // 介质）→ 缓存静默失效，退回现扫。
    if let Some(dir) = core::paths::cache_dir() {
        windui::render::image::svg_set_font_cache_path(Some(dir.join("svg-fonts.bin")));
    }
}

/// 启动应用（阻塞到窗口关闭）。`initial` 为命令行传入的首个路径（资源管理器"打开方式"）。
///
/// 第一件事是装崩溃钩子：UI 线程跑在 `extern "system"` 的系统回调里，panic 从这里
/// 展开不出去（直接 abort），没有钩子的话表现就是"点一下窗口没了"，毫无线索。
pub fn run(initial: Option<std::path::PathBuf>) {
    // 顺序有讲究：SEH filter 先于 panic 钩子之外独立装好，启动阶段的任何一条通道都留证；
    // 会话戳再后（它靠"戳有没有被正常删掉"判断上次是不是异常）。
    crate::platform::crash_evidence::install();
    crate::platform::panic_guard::install();
    // 诊断物落点：建 `temp\`，并删掉**文件名日期**过保留期（7 天）的旧日志与转储。
    // 排在两条 install 之后、任何写入之前：崩溃报告也要落进这个目录，而"先清后写"
    // 才不会出现"本次刚写的那枚被自己删掉"。失败静默，见 `prepare_diagnostics`。
    crate::platform::crash_evidence::prepare_diagnostics();
    // 会话头（版本/系统/DPI/显示器/提权/命令行）：桌面软件出事能拿回来的通常只有一个
    // 日志文件，没有这几行就分不清"哪一版、什么环境"——同一现象四种成因挤在一条日志里。
    // 它绕开 log 的 warn 过滤器直接落盘，理由见 `platform::env_report` 模块头。
    crate::platform::crash_evidence::append_run_log(
        &crate::platform::env_report::startup_preamble(),
    );
    // 打开的是哪一张，紧跟在会话头后面说一句。
    // ★ 为什么不能在 `main.rs` 里说：那早于上面这段前言，日志里就会变成"正文第一行 →
    //   会话头 → 其余正文"，前言不再是前言（2026-10-20 默认档提到 info 之后才现形——
    //   这条行过去是 info、被 warn 档挡在门外，所以从没人看见过它的顺序）。
    if let Some(p) = initial.as_ref() {
        log::info!("命令行打开：{}", p.display());
    }
    crate::platform::session::open_session();
    runtime::run(initial);
    crate::platform::session::close_session();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `initial_path` 必须与 `main` 的解析完全一致：跳过程序名、取首个非 `--` 参数。
    #[test]
    fn initial_path_skips_exe_and_picks_first_file() {
        // argv[0] 是程序名（不是路径选项），不应被当成要打开的文件。
        assert_eq!(
            initial_path(&["rupix.exe".into(), "C:\\a.png".into()]),
            Some(std::path::PathBuf::from("C:\\a.png")),
            "首个真参数应是文件路径"
        );
        // 选项在前、文件在后：文件仍应被识别。
        assert_eq!(
            initial_path(&["rupix.exe".into(), "--safe-mode".into(), "D:\\b.jpg".into()]),
            Some(std::path::PathBuf::from("D:\\b.jpg")),
        );
        // 纯选项、无文件：None。
        assert_eq!(
            initial_path(&["rupix.exe".into(), "--safe-mode".into()]),
            None
        );
        // 仅程序名：None。
        assert_eq!(initial_path(&["rupix.exe".into()]), None);
    }
}
