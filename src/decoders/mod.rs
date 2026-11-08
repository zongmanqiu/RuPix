//! 解码器宿主：真正把 `decoders/` 里的 DLL 装进进程、调它解码。
//!
//! 目录约定与「为什么」在 [`crate::decoders::catalog`] 模块头；ABI 契约在
//! [`crate::core::abi`] 与 `docs/解码器开发规范.md`。本模块只做**加载与调用**这一件事。
//!
//! # 一切都是 DLL 自述的
//!
//! 目录里**只有 DLL**：没有清单文件、没有哈希。解码器的 id、显示名、作用说明、认领的
//! 扩展名，全部通过 `iv_decoder_info()` 从 DLL 自己读出来。这样传播时只需拷一个 DLL，
//! 宿主也少一层"清单与文件对不对得上"的维护。
//!
//! # 生命周期：懒加载，不碰冷启动
//!
//! 启动那一截不许碰磁盘扫描与 `LoadLibrary`（双击→首帧的实测账走 `build/startup_probe.ps1`）。
//! 所以这里**不在启动时扫描或加载任何 DLL**：解码线程上第一次
//! 走到 `Host::refresh_catalog`（`catalog_gen` 从 0 起，首访必触发）才扫 `decoders/` 并加载候选 DLL。
//!
//! # 热加载
//!
//! [`request_rescan`] 把代次 +1，宿主下次取目录时重扫一遍：新丢进去的 DLL 无需重启即生效。
//! 设置页的「重新扫描」按钮就是它。
//!
//! # 防护（规范 §六 宿主侧的对应物）
//!
//! | 防护 | 这里怎么落 |
//! |------|-----------|
//! | 防 DLL 劫持 | 加载前 `SetDefaultDllDirectories`，走绝对路径 `LoadLibraryExW` |
//! | 加载前校验 | 读 `iv_abi_version()` 做区间判定（见 `MIN_ABI_VERSION`），出界即拒载 |
//! | 每次 FFI 包 catch_unwind + SEH | 每个导出函数调用都过 [`invoke`]，先用 C 的
//!   `__try/__except` 拦下真正的访问违规（Access Violation），再交 `catch_unwind` 接 Rust panic |
//! | 崩溃三振出局 | 返回 [`ERR_PANIC`] **或** SEH 硬故障累计 3 次 → 会话黑名单，不再加载 |
//! | 用户开关 | 设置页可停用某个 DLL，状态写进 `rupix.ini` |
//! | `--safe-mode` | [`set_safe_mode`] 一开，[`Host::try_decode`] 直接不做事 |
//!
//! # 诚实的边界：in-proc 路线是"止损"，不是"沙箱"
//!
//! 解码器与宿主**同进程**（in-proc 路线）。Rust panic 早已被 `catch_unwind` 隔离；
//! **访问违规（SEH）现在也被宿主侧的 C 隔离层（`seh_shim.c`，见 [`invoke`]）拦下**——
//! 解码器里一句 `std::ptr::null_mut::<u8>().write_volatile(1)` 不再连主程序一起带走，而是
//! 降级成一次失败、计入三振。但被写花的进程内存仍在，所以这是"止损"不是"零污染沙箱"：
//! 真正每字节都干净的隔离要换独立解码进程（宪章 §5.1 判它不划算，见 `docs/产品定位与边界.md`）。
//! 文档与 UI 文案都不许把三振说成沙箱。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{c_char, CStr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use libloading::Library;

pub mod catalog;

use crate::core::abi::*;
use crate::core::decode::RawParams;
use crate::core::geometry::{fit_oriented, fit_to};
use crate::core::pixels::to_rgba8;
use crate::core::pixels::{Bitmap, Decoded, MAX_PIXELS};
use crate::platform::dll::harden_search_path;
use crate::platform::seh::invoke;

mod backend;
mod decode;
mod dll_lock;
mod dllmeta;
mod host;
mod output;
mod registry;
mod select;

#[cfg(test)]
mod tests;

/// 解码器清单（`<exe>/docs/decoders.json` 本地优先，刷新时后台拉远程）：**对外可见**
/// 是因为 UI 要调 `load_local` / `start_refresh` / `poll_refresh`，而不是只读一个类型
/// ——`pub use` 只能带出名字，带不出方法。
pub mod remote;

/// 下载安装（几 MB 的文件走后台线程，界面按 250 ms 轮询 [`install::poll`]）。
pub mod install;

pub use backend::install_format_source;
pub use dllmeta::Descriptor;
pub use host::Host;
pub use registry::{
    declared_exts, declared_raw_exts, declared_vector_exts, list_decoders, parse_dll_version,
    DecoderEntry, Health,
};
pub use remote::{install_state, version_cmp, Catalog, InstallState, RemoteEntry, SourceId};

/// 连续失败到几次就把这个 id 拉黑（会话级）。与规范 §六"崩溃三振出局"一致。
const MAX_STRIKES: u32 = 3;

/// 解码器申报的 ICC 段上限。真实 profile（v2/v4，含曲线表）几十 KB 量级，1 MiB 已经宽得离谱；
/// 超过它说明解码器把长度字段写错了或被畸形文件喂坏了——这种值**不能拿去分配**（审计 V5：
/// 分配本身要按 `bytes_needed + icc_len`，闸门必须同在分配前）。
const ICC_CAP: u32 = 1 << 20;

/// `--safe-mode` 开关（进程级）。由入口解析命令行后置位，见 [`set_safe_mode`]。
static SAFE_MODE: AtomicBool = AtomicBool::new(false);

/// 打开 `--safe-mode`：不加载、不调用任何解码器，作为崩溃/卡死的最后止损。
pub fn set_safe_mode(on: bool) {
    SAFE_MODE.store(on, Ordering::Relaxed);
}

/// 当前是否处于 safe-mode。
pub fn safe_mode() -> bool {
    SAFE_MODE.load(Ordering::Relaxed)
}

/// 热加载代次。设置页「重新扫描」把它 +1，宿主下次取目录时重扫。
static RESCAN_GEN: AtomicU64 = AtomicU64::new(1);

/// 请求重扫（热加载）。
///
/// 语义就是"我往 `decoders/` 里动了 DLL"：下次取目录时重扫、格式清单随之更新。
/// （过去它还顺带刷新一枚"解码器身份"喂给磁盘缓存键——那套缓存 2026-10-19 已撤。）
pub fn request_rescan() {
    RESCAN_GEN.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn rescan_gen() -> u64 {
    RESCAN_GEN.load(Ordering::Relaxed)
}

/// 用户关闭的解码器 id（全局：UI 线程写、解码线程读）。
static DISABLED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn disabled_set() -> &'static Mutex<HashSet<String>> {
    DISABLED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 整体覆盖禁用集（启动时按配置灌入，开关切换时改它）。
pub fn set_disabled(ids: impl IntoIterator<Item = String>) {
    let mut g = disabled_set().lock().unwrap_or_else(|e| e.into_inner());
    g.clear();
    g.extend(ids);
}

/// 拿停用集这把锁（调用方自己 `contains`）。
///
/// ★ m102：选解码器那条路要给**每枚**解码器问一次"它停用没有"，逐枚锁就是每次解码十几把锁；
/// 那里改成一次锁读完。单点查询用下面的 [`is_disabled`]，它就是这一句的包装。
fn disabled_guard() -> std::sync::MutexGuard<'static, HashSet<String>> {
    disabled_set().lock().unwrap_or_else(|e| e.into_inner())
}

/// 某个解码器 id 是否被用户关闭。
pub fn is_disabled(id: &str) -> bool {
    disabled_guard().contains(id)
}

/// 追加一个停用 id（2026-11-04 挂死熔断用）。
///
/// 与 [`set_disabled`] 的区别是**只增不覆盖**：熔断发生在运行期，此时那把全局表已经
/// 装着用户勾掉的那些 id，整表覆盖会把它们悄悄放回来。
pub fn add_disabled(id: &str) {
    let mut g = disabled_guard();
    g.insert(id.to_string());
}

/// 各扩展名的**用户显式首选**解码器（`扩展名 → 解码器 id` 稀疏表；全局：UI 线程写、
/// 解码线程读）。设置窗「解码」页是唯一写入方，启动时按配置灌入。
static PREFERRED: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();

fn preferred_map() -> &'static Mutex<BTreeMap<String, String>> {
    PREFERRED.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 整体覆盖首选表（启动时按配置灌入，解码器页「首选解码器」改下拉时更新）。
pub fn set_preferred(map: BTreeMap<String, String>) {
    let mut g = preferred_map().lock().unwrap_or_else(|e| e.into_inner());
    *g = map;
}

/// 某扩展名的用户首选解码器 id；没设过 = `None`（走自动仲裁）。
pub(super) fn preferred_for(ext: &str) -> Option<String> {
    preferred_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(ext)
        .cloned()
}
