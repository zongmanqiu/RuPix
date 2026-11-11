//! 关联自愈的调度（m314）：把「勾选表 ∩ 当前可用后缀」交给后台线程核对与重写。
//!
//! # 为什么单独一层
//!
//! [`crate::platform::association`] 只懂"怎么写、怎么核对"，不懂"什么时候写"；
//! 把线程与时间门塞进 `runtime::global_tick` 会让那一大段调度逻辑更浑。这一层只做三件事：
//! 过滤（勾选 ∩ 可用后缀）、起线程、记日志。
//!
//! # 为什么必须后台
//!
//! `heal` 判破时会走 `apply`，末尾一次 `SHChangeNotify(SHCNE_ASSOCCHANGED)` 会让
//! 资源管理器重读关联与图标缓存——那一声落在 16 ms 的帧里就是一次可感知的卡顿。
//! 核对本身只是毫秒级，但既然写入必须离开主线程，索性整趟都放进去。
//!
//! # 时间门（用户 2026-11-11 拍板）
//!
//! 启动**不**立刻查：头十秒里字体扫描（~0.9 s）、DLL 首扫、首屏解码、两轮工作集裁剪
//! 都在抢，自愈再插一脚只会让启动变糙。启动满 `FIRST_DELAY` 后首查，之后每
//! `INTERVAL` 一次。破坏多发生在 RuPix **没**运行时（装别的软件、系统重置默认应用），
//! 所以启动那一次价值最大，常驻期只是兜底。
//!
//! # 总开关（m315）
//!
//! 这一层只在 `Config::assoc_heal` 打开时被调用——勾了后缀只代表"点应用时写一次"，
//! 不代表授权我们反复动注册表。开关的 UI 在文件关联窗底栏。

use std::time::Duration;

/// 首查延迟：避开启动期（字体 / DLL / 首屏 / 工作集裁剪）。
pub const FIRST_DELAY: Duration = Duration::from_secs(60);

/// 之后每轮间隔（用户拍板 60 分钟：比 30 分钟省，又不至于一整晚不纠）。
pub const INTERVAL: Duration = Duration::from_secs(3600);

/// 起一趟自愈。`subs` = 配置里勾选的后缀表；空表直接返回（没勾后缀就没有要维护的东西）。
///
/// ★ 本函数**不**判断总开关：`assoc_heal`（默认关）由调用方 `global_tick` 先看。
///   自愈会抢回"用户自己在系统设置里改成的别的程序"，所以必须是用户显式选择。
///
/// 线程 fire-and-forget：起不来只是这轮跳过，关联坏着不影响本次会话看图。
pub fn kick(subs: Vec<String>) {
    if subs.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("assoc-heal".into())
        .spawn(move || run(subs));
    if let Err(e) = spawned {
        log::warn!("[assoc] 自愈线程启动失败：{e}");
    }
}

/// 线程体：勾选表 ∩ 当前可用后缀 → 核对 + 重写。
///
/// `formats::declared_exts` 已经过滤掉「被用户关掉」与「已卸载」的解码器，所以这里
/// 求交就等于实现了「解码器关了就不要再维护它的后缀」——**勾选本身不删**（一开一关
/// 不能丢用户的勾选，见 `config::Config::assoc_exts` 的说明）。
fn run(subs: Vec<String>) {
    let live = crate::core::formats::declared_exts();
    let exts: Vec<String> = subs.into_iter().filter(|e| live.contains(e)).collect();
    if exts.is_empty() {
        return;
    }
    let n = crate::platform::association::heal(&exts);
    if n > 0 {
        log::info!("[assoc] 自愈完成：{n} 个后缀被改回 RuPix");
    }
}
