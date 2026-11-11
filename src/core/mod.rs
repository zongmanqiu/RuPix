//! core 层模块声明：**纯逻辑，不许碰窗口、不许碰框架**。
//!
//! 本层的判据只有一条——**离屏能跑**：单测不建窗、不装解码栈也能把这里全部跑完。
//! 因此禁引 `windui` / `windows`（由 `build/layering_scan.py` 机器执行），也**不 import
//! 任何解码器**：解码走本层自己声明的 [`decode::DecodeBackend`] trait（消费方定义契约），
//! 实现由 `crate::decoders::Host` 在运行期注入（见 `src/lib.rs::install_decode_stack`）。
//!
//! 本文件只做**声明**：每个 `pub mod` 后面那行 `///` 是它在分层图里的职责。
//! 想实现什么请进对应文件——这里不放任何函数体（与 CurSeen 的 `core/mod.rs` 同款：
//! 声明 + 一行职责，零实现）。

/// 解码器 ABI 的宿主侧镜像（`#[repr(C)]` 结构 + 错误码 + 编译期断言）。
pub mod abi;
/// 压缩包来源：容器递归展平成一层真实文件，供 `Gallery` 按目录清单消费（离屏可测）。
pub mod archive;
pub mod b64;
pub mod config;
pub mod decode;
pub mod decode_watchdog;
/// 档位本地派生：把"从原图档重采样一档像素"这件按源尺寸计价的活儿交给一条专属线程，
/// 换档时 UI 线程只投递不换图（M2，实测一次 36MP 重采样要在调用线程上花 0.12–0.13 s）。
pub mod derive;
pub mod error;
/// 非解码器扩展的纯数据模型：manifest / 能力 / 类型（不碰文件系统与 UI）。
pub mod ext;
/// 扩展进程的请求/响应 JSON 契约（传输层无关）。
pub mod ext_protocol;
pub mod formats;
pub mod gallery;
pub mod geometry;
/// 快捷键数据模型与匹配：动作枚举、单条快捷键、整组绑定（INI 落盘见 `config`）。
pub mod hotkey;
/// 极简 JSON 解析（只为读解码器清单；无 serde 依赖，带深度上限与 UTF-8 校验）。
pub mod json;
/// 便携模式的唯一落点（exe 同级目录）。config 与 panic_guard 都从这里取路径。
pub mod paths;
pub mod pixels;
/// 缩略图条的后台解码账本（在途请求 + 内存缓存 + LRU 淘汰）。
pub mod thumbs;
/// 宿主自更新的纯逻辑：版本 / 清单 / 「文件名 + 版本号」两条判据（离屏可测）。
pub mod update;
/// 跨层复用的小工具（字节数格式化等）。放 core 是因为 core 不能反向依赖上层。
pub mod util;
pub mod viewer;
