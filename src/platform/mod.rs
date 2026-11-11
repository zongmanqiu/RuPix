//! 平台层：Windows 专属的薄壳（SEH/DLL 加载/剪贴板/无边框窗口/崩溃钩子）。
//!
//! core 的纯逻辑不碰这里；`ui`/`decoders`/`runtime` 需要平台能力时从本层取。
pub mod association;
pub mod clipboard;
pub mod color;
pub mod crash_evidence;
pub mod cursor;
pub mod dir_watch;
pub mod display;
pub mod dll;
pub mod env_report;
pub mod exif;
/// 最小 HTTPS GET（WinHTTP）：设置页「可下载」卡读清单用，只做"取一份 body"。
pub mod http;
/// 扩展进程 Job Object（宿主退出时内核兑底杀整组子进程）。
pub mod job;
pub mod logging;
pub mod panic_guard;
pub mod pin_glyph;
/// 扩展进程命名管道（长度前缀帧）。
pub mod pipe;
pub mod pipe_dacl;
/// 扩展进程启动/等待/终止。
pub mod process;
pub mod recycle;
pub mod seh;
pub mod session;
/// 扩展图像共享内存（宿主分配、扩展只读打开）。
pub mod shared_mem;
pub mod shell;
pub mod shell_selection;
pub mod spacehook;
pub mod userchoice_hash;
pub mod window_frame;
pub mod working_set;
