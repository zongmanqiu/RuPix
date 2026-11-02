//! 非解码器插件宿主：已安装发现、启停、本地可安装目录与安装。
//!
//! 目录约定：
//! - 已安装：`<exe>/plugins/<id>/manifest.json`
//! - 可安装清单：`<exe>/docs/plugins.json`，ZIP 包与清单同目录
//!
//! 用户点「安装」才把 ZIP 解压到 `plugins/`；只有一个插件运行时目录。
//! 「已安装」与「可安装」是两份事实，不能合成一份。
//!
//! 与解码器的边界：插件不认领图片格式，不参与解码路由；它们只消费已上屏的图片/路径。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::core::ext::{Capability, DynamicHotkeys, Kind, Manifest, PluginHotkey};
use crate::core::hotkey::{Hotkeys, KeyInput};

/// 命令型插件的后台调用编排。
pub mod install;
/// 插件清单：本地读取、远程镜像与后台刷新。
pub mod remote;
pub mod run;
/// 插件私有设置区（`plugins/<id>/settings.json`）。
pub mod settings;
pub use run::{
    poll, start_command, start_editor, start_settings, take_isolations, JobOutcome, JobOutput,
};

/// 一枚已发现的扩展。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub enabled: bool,
    /// 宿主版本低于 `manifest.min_host`：照常列出但不开放入口
    /// （菜单置灰并标注原因、启动拒绝）。
    pub blocked: bool,
    /// 三振隔离中（连续 3 次运行期失败，自动写入；设置页可解除）。
    pub isolated: bool,
}

/// 扫描默认已安装目录（exe 同级 `plugins/`），停用表与隔离表取自配置。
///
/// 这是 `extensions_disabled` / `extensions_isolated` 的行为读者：设置页只负责写，
/// 真正决定“用不用/放不放行”的是这里。UI 也得走这条，不允许自己另拼一份过滤。
pub fn discover_configured(config: &crate::core::config::Config) -> Vec<Extension> {
    discover(&config.extensions_disabled, &config.extensions_isolated)
}

/// 扫描默认已安装目录（exe 同级 `plugins/`）。
pub fn discover(disabled: &[String], isolated: &[String]) -> Vec<Extension> {
    match crate::core::paths::plugin_dir() {
        Some(root) => discover_in(&root, disabled, isolated),
        None => Vec::new(),
    }
}

/// 扫描指定根目录。坏 manifest 只记 warn 并跳过，不让一个坏包拖住整页。
pub fn discover_in(root: &Path, disabled: &[String], isolated: &[String]) -> Vec<Extension> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let dir = entry.path();
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        // 安装 staging / 系统垃圾目录不参与发现。
        if dir
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with('.'))
        {
            continue;
        }
        match Manifest::from_dir(&dir) {
            Ok(manifest) => {
                // 目录名 ≠ 自述 id：照常列出（发现环节不许因为一个命名习惯把插件变没），
                // 但要说出来——设置与卸载都按 id 找目录，这一枚的偏好会读不到。
                if dir.file_name().and_then(|s| s.to_str()) != Some(manifest.id.as_str()) {
                    log::warn!(
                        "插件目录名与自述 id 不一致：{} ≠ {}（宿主按 id 找 settings.json 与卸载目标，这一枚的设置可能读不到）",
                        dir.display(),
                        manifest.id
                    );
                }
                let enabled = !disabled.iter().any(|id| id == &manifest.id);
                // `min_host` 门禁：宿主低于要求就拦在入口（发现照常，启动/菜单拒）。
                let blocked =
                    remote::version_cmp(crate::core::ext::HOST_VERSION, &manifest.min_host)
                        == std::cmp::Ordering::Less;
                // 三振隔离：连续失败的插件照列但入口关闭，设置页可解除。
                let is_isolated = isolated.iter().any(|id| id == &manifest.id);
                out.push(Extension {
                    manifest,
                    dir,
                    enabled,
                    blocked,
                    isolated: is_isolated,
                });
            }
            Err(e) => log::warn!("跳过扩展 {}：{e}", dir.display()),
        }
    }
    // 先按 (id, 目录) 排：同 id 内部也就确定了，不跟着 `read_dir` 的顺序漂。
    out.sort_by(|a, b| {
        a.manifest
            .id
            .cmp(&b.manifest.id)
            .then_with(|| a.dir.cmp(&b.dir))
    });
    // **同 id 只留一版**（2026-10-29 m162，插件 id 改成裸短名的配套判据）：
    // 与解码器 `refresh_catalog` 同一句话——版本高者赢，版本相同取目录字典序第一份。
    // 这不是"防撞名保险"：两枚同 id 时旧代码会并排两行、停用表一次命中两枚、热键注册两次，
    // 那是未定义状态；列表必须有唯一答案。落选的那份**不进列表**，日志点名两份路径与版本。
    let mut kept: Vec<Extension> = Vec::with_capacity(out.len());
    for e in out {
        if let Some(prev) = kept.last_mut() {
            if prev.manifest.id == e.manifest.id {
                let take_new = remote::version_cmp(&e.manifest.version, &prev.manifest.version)
                    == std::cmp::Ordering::Greater;
                let (win, lose) = if take_new { (&e, &*prev) } else { (&*prev, &e) };
                log::warn!(
                    "插件 id「{}」有两份：只列 {}（版本 {}），不列 {}（版本 {}）",
                    e.manifest.id,
                    win.dir.display(),
                    win.manifest.version,
                    lose.dir.display(),
                    lose.manifest.version
                );
                if take_new {
                    *prev = e;
                }
                continue;
            }
        }
        kept.push(e);
    }
    kept
}

/// 某个扩展是否在启停表里。
pub fn is_enabled(id: &str, disabled: &[String]) -> bool {
    !disabled.iter().any(|x| x == id)
}

/// 已安装插件列表 + 动态热键表的缓存。
///
/// 为什么要有缓存：动态热键在**每次按键**时都要查一遍"这把键是谁的"，
/// 而插件列表来自扫盘（`<exe>/plugins/*/manifest.json`）。每个按键扫一次盘
/// 是白搬——插件集合几乎不变，变的是"用户改没改内置键位"（那个是纯内存比较）。
struct InstalledCache {
    list: Vec<Extension>,
    /// 装表时用的内置键位快照；用户改了键位就要重装表（不用扫盘）。
    builtins: Hotkeys,
    /// 共享一份：每次按键都要查表，别为此克隆整张表。
    table: std::sync::Arc<DynamicHotkeys>,
}

static INSTALLED: OnceLock<Mutex<Option<InstalledCache>>> = OnceLock::new();

fn installed_cache() -> &'static Mutex<Option<InstalledCache>> {
    INSTALLED.get_or_init(|| Mutex::new(None))
}

/// 锁中毒恢复成原值：插件集合不值得为一次 panic 整条链路报错。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 插件集变了（装/卸/启停）之后调用：下一次查表重新扫盘。
pub fn invalidate_installed_cache() {
    *lock(installed_cache()) = None;
}

/// 当前已安装插件列表（带缓存）。
///
/// 与 [`discover_configured`] 的区别只有一处：**这个不保证是磁盘上的最新事实**
/// （装了/卸了插件但没调 [`invalidate_installed_cache`] 时会读到旧表）。
/// 菜单与设置页走 `discover_configured`（要真事实），热键这条路走这里（要快）。
pub fn installed_cached(config: &crate::core::config::Config) -> Vec<Extension> {
    ensure_cache(config);
    lock(installed_cache())
        .as_ref()
        .map(|c| c.list.clone())
        .unwrap_or_default()
}

/// 当前生效的动态热键表。
///
/// 裁决口径（宿主定，插件无权）：
/// - 只认**命令型**插件：编辑型要整图像素、面板型没有入口，热键没有意义；
/// - 停用 / `min_host` 不满足 / 三振隔离的插件，热键不生效；
/// - **内置快捷键优先**：用户自己配的键位不被插件抢走（插件这条热键静默失效 + 记一条 warn）；
/// - 两个插件抢同一把键：按插件 id 排序先来先得，后来者失效 + warn。
pub fn hotkey_table(config: &crate::core::config::Config) -> std::sync::Arc<DynamicHotkeys> {
    ensure_cache(config);
    lock(installed_cache())
        .as_ref()
        .map(|c| c.table.clone())
        .unwrap_or_else(|| std::sync::Arc::new(DynamicHotkeys::default()))
}

/// 保证缓存里有一份与当前配置一致的表；内置键位变了就重装表（不扫盘）。
fn ensure_cache(config: &crate::core::config::Config) {
    {
        let g = lock(installed_cache());
        let stale = match g.as_ref() {
            Some(c) => c.builtins != config.hotkeys,
            None => true,
        };
        if !stale {
            return;
        }
    }
    let list = discover_configured(config);
    let table = build_hotkey_table(&list, &config.hotkeys);
    let mut g = lock(installed_cache());
    *g = Some(InstalledCache {
        list,
        builtins: config.hotkeys.clone(),
        table: std::sync::Arc::new(table),
    });
}

/// 声明了 `type=hotkey` 的已安装插件：`(插件 id, 插件名, 设置键, 当前值)`。
///
/// ★ 判据只看**清单 schema**：没有 schema 的老插件（`manifest.hotkey` 那条路）
/// 不进这份清单——它们的键位在发布物里，用户改不了，列出来只会让人点了没反应。
///
/// 放在 `extensions` 而不是 UI：它是纯数据（清单 + 设置区），而 `hotkey` 冲突清空
/// （[`vacate_hotkey`]）也要读同一份清单——UI 层不许被底层反向依赖。
pub fn plugin_hotkey_items(
    config: &crate::core::config::Config,
) -> Vec<(String, String, String, String)> {
    let list = installed_cached(config);
    let mut out = Vec::new();
    for ext in list {
        let Some(spec) = ext
            .manifest
            .settings
            .iter()
            .find(|s| s.kind == crate::core::ext::SettingKind::Hotkey)
        else {
            continue;
        };
        let raw = settings::load(&ext.manifest.id)
            .into_iter()
            .find(|(k, _)| k == &spec.key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| spec.default.clone());
        out.push((
            ext.manifest.id.clone(),
            ext.manifest.name.clone(),
            spec.key.clone(),
            raw,
        ));
    }
    out
}

/// 给**宿主动作**绑键：先清同键的旧持有者，再绑。
///
/// ★ 顺序不能反：`vacate_hotkey` 会清掉绑定表里所有含这把键的动作——
///   若先 `set_binding(a, hk)` 再 vacate，刚绑上的 `a` 会被自己清掉，
///   症状是"很多键绑不上"（2026-10-31 实测：F4 绑完立刻变空）。
pub fn bind_host_action(
    config: &mut crate::core::config::Config,
    a: crate::core::hotkey::Action,
    hk: crate::core::hotkey::Hotkey,
) {
    vacate_hotkey(config, &hk, None);
    config.hotkeys.set_binding(a, hk);
    crate::core::config::mark_dirty();
}

/// 新绑定的键 = **唯一持有者**：宿主动作与其它插件里同一把键一律清空。
///
/// 用户 2026-10-31 口径："a 选了 F4，若软件里（无论宿主还是插件）已有第二个用 F4，
/// 则自动把第一个 F4 变为空。"——**最后一个绑定的人赢**，不留"按 id 序先来先得"
/// 这种看不见的裁决（那会让用户以为自己绑上了，按下去却跑了别人）。
///
/// `keep_plugin`：本次绑定属于哪个插件（清别人时跳过它自己）。
pub fn vacate_hotkey(
    config: &mut crate::core::config::Config,
    hk: &crate::core::hotkey::Hotkey,
    keep_plugin: Option<&str>,
) {
    // 1) 宿主动作：绑定表里同一把键的，清掉。
    let mut host_changed = false;
    for a in crate::core::hotkey::Action::ALL {
        let ai = a.index();
        if config.hotkeys.bindings[ai].iter().any(|b| b == hk) {
            config.hotkeys.clear_binding(a);
            host_changed = true;
        }
    }
    if host_changed {
        crate::core::config::mark_dirty();
        log::info!("[hotkey] 新键抢占，已清空宿主里同键的动作");
    }

    // 2) 其它插件：设置区里那把键同样清空（写的是同一个 settings.json，走原子写）。
    let mut plugins_changed = false;
    for (id, _, key, raw) in plugin_hotkey_items(config) {
        if Some(id.as_str()) == keep_plugin {
            continue;
        }
        if crate::core::hotkey::Hotkey::from_spec(&raw).as_ref() == Some(hk) {
            let mut items = settings::load(&id);
            match items.iter_mut().find(|(k, _)| k == &key) {
                Some(slot) => slot.1 = String::new(),
                None => items.push((key.clone(), String::new())),
            }
            if let Err(e) = settings::save(&id, &items) {
                log::warn!("插件「{id}」同键热键清空失败：{e}");
            } else {
                plugins_changed = true;
                log::info!("[hotkey] 新键抢占，已清空插件「{id}」的同键");
            }
        }
    }
    if plugins_changed {
        invalidate_installed_cache();
    }
}

/// 把某插件的动态热键写进它的设置区（快捷键页与插件设置窗共用这一条写路径）。
///
/// 两个入口改的是**同一个文件**（`plugins/<id>/settings.json`），谁写都一样——
/// 这就是"两处同步"的实现方式：不是互相同步，而是**只有一个真源**。
pub fn set_plugin_hotkey(plugin_id: &str, spec_key: &str, spec: &str) -> Result<(), String> {
    let mut items = settings::load(plugin_id);
    match items.iter_mut().find(|(k, _)| k == spec_key) {
        Some(slot) => slot.1 = spec.to_string(),
        None => items.push((spec_key.to_string(), spec.to_string())),
    }
    settings::save(plugin_id, &items)?;
    // 热键表读的就是这份文件：写完立刻失效缓存，下一次按键就按新键判。
    invalidate_installed_cache();
    Ok(())
}

/// 插件实际生效的那把键：**声明式设置里的 hotkey 项优先**（值存在
/// `plugins/<id>/settings.json`，用户可在插件设置窗里改），没有 schema 的老插件
/// 退回 `manifest.hotkey`。
///
/// 为什么值放设置区而不是清单：清单是**发布物**，用户改不了；而"哪把键"本来就是
/// 用户自己的偏好（快捷键页里所有键位都归用户）。
fn effective_hotkey(ext: &Extension) -> Option<crate::core::hotkey::Hotkey> {
    if let Some(spec) = ext
        .manifest
        .settings
        .iter()
        .find(|s| s.kind == crate::core::ext::SettingKind::Hotkey)
    {
        let raw = settings::load(&ext.manifest.id)
            .into_iter()
            .find(|(k, _)| k == &spec.key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| spec.default.clone());
        return crate::core::hotkey::Hotkey::from_spec(&raw);
    }
    ext.manifest.hotkey
}

/// 按裁决口径组装动态热键表（纯函数，测试直接喂列表）。
fn build_hotkey_table(list: &[Extension], builtins: &Hotkeys) -> DynamicHotkeys {
    let mut table = DynamicHotkeys::default();
    for ext in list {
        let Some(key) = effective_hotkey(ext) else {
            continue;
        };
        if !ext.enabled {
            continue;
        }
        if ext.blocked {
            log::warn!(
                "插件 {} 声明的热键 {} 不生效：宿主版本低于它要求的 {}",
                ext.manifest.id,
                key.pretty(),
                ext.manifest.min_host
            );
            continue;
        }
        if ext.isolated {
            log::warn!(
                "插件 {} 声明的热键 {} 不生效：插件已被隔离",
                ext.manifest.id,
                key.pretty()
            );
            continue;
        }
        if ext.manifest.kind != Kind::Command {
            log::warn!(
                "插件 {} 声明的热键 {} 不生效：只有命令型插件能绑热键",
                ext.manifest.id,
                key.pretty()
            );
            continue;
        }
        if builtins.uses(&key) {
            log::warn!(
                "插件 {} 声明的热键 {} 与内置快捷键冲突，内置优先，该热键不生效",
                ext.manifest.id,
                key.pretty()
            );
            continue;
        }
        if table.contains_key(&key) {
            log::warn!(
                "插件 {} 声明的热键 {} 已被别的插件占用，该热键不生效",
                ext.manifest.id,
                key.pretty()
            );
            continue;
        }
        table.push(PluginHotkey {
            plugin_id: ext.manifest.id.clone(),
            plugin_name: ext.manifest.name.clone(),
            key,
        });
    }
    table
}

/// 按动态热键启动插件。返回 `Ok(提示语)` 或 `Err(原因)`。
///
/// 安装即视为同意（2026-10-29 用户口径）：不再有首用授权窗/授权表。
pub fn run_by_hotkey(
    config: &crate::core::config::Config,
    ev: &KeyInput,
    image_path: Option<PathBuf>,
    pixels: Option<crate::core::viewer::ForPlugin>,
) -> Result<String, String> {
    let table = hotkey_table(config);
    let Some(entry) = table.match_plugin(ev) else {
        return Err("没有插件认领这把键".into());
    };
    let list = installed_cached(config);
    let Some(ext) = list.iter().find(|e| e.manifest.id == entry.plugin_id) else {
        return Err(format!("插件「{}」已不在安装目录里", entry.plugin_name));
    };
    // 能力最小化：没声明 image.read 的插件连路径都不发。
    let needs_image = ext.manifest.has_capability(Capability::ImageRead);
    let img = if needs_image { image_path } else { None };
    if needs_image && img.is_none() {
        return Err(format!(
            "插件「{}」需要当前图片，但屏上没有图",
            entry.plugin_name
        ));
    }
    let name = entry.plugin_name.clone();
    start_command(ext.clone(), img, None, pixels)?;
    Ok(format!("已启动插件：{name}"))
}

/// 插件目录的**解压后占用**（递归求和，跳过链接项；读不到的项忽略）。
///
/// 信息窗「大小」在已安装这一侧显示的是真实磁盘占用（解压后），
/// 可下载那一侧显示的是 ZIP 包大小——两个数含义不同，标签写明。
pub fn dir_size(dir: &Path) -> Option<u64> {
    /// 返回 false = 这一层读不了。子目录读失败只跳过；**根目录**失败才让整次返回 None
    /// （否则权限不足会显示成「0 B」，比「未知」更误导）。
    fn walk(p: &Path, total: &mut u64) -> bool {
        let Ok(rd) = std::fs::read_dir(p) else {
            return false;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                let _ = walk(&entry.path(), total);
            } else if let Ok(m) = entry.metadata() {
                *total = total.saturating_add(m.len());
            }
        }
        true
    }
    let mut total = 0;
    walk(dir, &mut total).then_some(total)
}

/// 把已安装插件目录移进回收站。失败（例如插件 exe 正被运行）原样交回 UI。
pub fn uninstall(id: &str) -> Result<(), String> {
    // 与 `install_zip` 同一条口径：**路径使用点不能只信解析期**——这个 id 来自
    // 磁盘上的目录名/调用方，越界的名字会在 plugins/ 之外指到一个目录并送进回收站。
    crate::core::ext::validate_id(id)?;
    let root = crate::core::paths::plugin_dir().ok_or("取不到 exe 目录")?;
    let dir = root.join(id);
    if !dir.is_dir() {
        return Err(format!("插件未安装：{id}"));
    }
    invalidate_installed_cache();
    crate::platform::recycle::send_to_recycle_bin(&dir)
}

/// 打开已安装插件目录（不存在就先建）。
pub fn open_plugin_dir() -> Result<(), String> {
    let dir = crate::core::paths::plugin_dir().ok_or("取不到 exe 目录")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("打开 {} 失败：{e}", dir.display()))?;
    Ok(())
}

/// 可安装清单里的一枚插件（来自 `<exe>/docs/plugins.json` 或远程镜像）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailablePlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: u32,
    /// 插件要求的最低宿主版本；空 = 旧清单没标注（按不限制处理）。
    pub min_host: String,
    pub kind: Kind,
    pub description: String,
    /// 同目录下的 ZIP 包文件名；不含路径分隔符。
    pub file: String,
    pub size: u64,
    pub capabilities: Vec<Capability>,
    pub released: String,
    pub author: String,
    pub license: String,
    /// 各来源的 release 资产地址：键是 `"gitee"` / `"github"`。
    pub urls: std::collections::HashMap<String, String>,
    pub installed: bool,
}

impl AvailablePlugin {
    /// 当前来源下可用的下载地址（缺键 / 空串算没有；地址由清单生成，不再做主机名校验）。
    pub fn download_url(&self, src: crate::core::config::SourceId) -> Option<&str> {
        self.urls
            .get(src.id())
            .filter(|s| !s.is_empty())
            .map(String::as_str)
    }

    pub fn downloadable_for(&self, src: crate::core::config::SourceId) -> bool {
        self.download_url(src).is_some()
    }

    /// 宿主能跑这个协议版本吗（旧插件兼容区间 `MIN..=MAX`）。
    pub fn protocol_ok(&self) -> bool {
        crate::core::ext::protocol_ok(self.protocol)
    }

    /// 宿主版本够不够（旧清单没有 `min_host` = 不限制）。
    pub fn host_ok(&self) -> bool {
        host_supports(&self.min_host)
    }

    /// 这台宿主能不能装/用它（协议与宿主版本都满足）。
    pub fn compatible(&self) -> bool {
        self.protocol_ok() && self.host_ok()
    }

    /// 用当前已安装 id 重算 `installed`。
    pub fn mark_installed(&mut self, installed_ids: &[String]) {
        self.installed = installed_ids.iter().any(|id| id == &self.id);
    }
}

/// 宿主版本是否满足 `min_host`（空 = 不限制；旧清单 / 旧 manifest 都按不限制处理）。
///
/// 判定只有这一处：可下载条目的 `host_ok()` 与设置页信息窗都调它，
/// 别在 UI 里再抄一遍 `version_cmp`。
pub fn host_supports(min_host: &str) -> bool {
    min_host.is_empty()
        || remote::version_cmp(crate::core::ext::HOST_VERSION, min_host) != std::cmp::Ordering::Less
}

/// 读取默认清单 `<exe>/docs/plugins.json`。
pub fn discover_available(installed_ids: &[String]) -> Vec<AvailablePlugin> {
    let Some(path) = crate::core::paths::plugin_catalog_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match parse_catalog(&text, installed_ids) {
        Ok(list) => list,
        Err(e) => {
            log::warn!("跳过插件清单 {}：{e}", path.display());
            Vec::new()
        }
    }
}

/// 解析清单文本（纯逻辑，测试直接喂字符串）。
pub fn parse_catalog(text: &str, installed_ids: &[String]) -> Result<Vec<AvailablePlugin>, String> {
    let root = crate::core::json::parse(text).map_err(|e| format!("JSON：{e}"))?;
    let arr = root.as_array().ok_or("插件清单根必须是数组")?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(crate::core::json::Value::as_str)
            .ok_or_else(|| format!("第 {} 项缺 id", i + 1))?;
        crate::core::ext::validate_id(id)?;
        let name = item
            .get("name")
            .and_then(crate::core::json::Value::as_str)
            .ok_or_else(|| format!("{id} 缺 name"))?;
        let version = item
            .get("version")
            .and_then(crate::core::json::Value::as_str)
            .ok_or_else(|| format!("{id} 缺 version"))?;
        let protocol = item
            .get("protocol")
            .and_then(crate::core::json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("{id} 缺 protocol"))?;
        let kind_s = item
            .get("kind")
            .and_then(crate::core::json::Value::as_str)
            .ok_or_else(|| format!("{id} 缺 kind"))?;
        let kind = Kind::parse(kind_s).ok_or_else(|| format!("{id} kind 不认识：{kind_s}"))?;
        let file = item
            .get("file")
            .and_then(crate::core::json::Value::as_str)
            .ok_or_else(|| format!("{id} 缺 file"))?;
        if file.contains('/') || file.contains('\\') || file.contains("..") {
            return Err(format!("{id} 的 file 必须是同目录文件名"));
        }
        let description = item
            .get("description")
            .and_then(crate::core::json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let size = item
            .get("size")
            .and_then(crate::core::json::Value::as_u64)
            .unwrap_or(0);
        let capabilities = match item.get("capabilities") {
            None | Some(crate::core::json::Value::Null) => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| format!("{id} 的 capabilities 必须是数组"))?
                .iter()
                .map(|x| {
                    x.as_str()
                        .and_then(Capability::parse)
                        .ok_or_else(|| format!("{id} 含未知能力"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        out.push(AvailablePlugin {
            id: id.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            protocol,
            min_host: item
                .get("min_host")
                .and_then(crate::core::json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            kind,
            description,
            file: file.to_string(),
            size,
            capabilities,
            released: item
                .get("released")
                .and_then(crate::core::json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            author: item
                .get("author")
                .and_then(crate::core::json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            license: item
                .get("license")
                .and_then(crate::core::json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            urls: {
                let mut m = std::collections::HashMap::new();
                for key in ["gitee", "github"] {
                    m.insert(
                        key.to_string(),
                        item.get(key)
                            .and_then(crate::core::json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    );
                }
                m
            },
            installed: installed_ids.iter().any(|x| x == id),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// 清单项对应的本地 ZIP（`docs/<file>`）；路径由清单解析阶段白名单校验。
pub fn local_package_path(entry: &AvailablePlugin) -> Option<PathBuf> {
    Some(crate::core::paths::docs_dir()?.join(&entry.file))
}

pub fn local_package_exists(entry: &AvailablePlugin) -> bool {
    local_package_path(entry).is_some_and(|p| p.is_file())
}

/// 用本地信息补齐可下载条目的 `min_host`（2026-10-29 用户报障）。
///
/// 线上旧 `plugins.json` 没有这个字段，界面于是显示「（未标注）」；同一枚装完后
/// manifest 又有值——用户看到的是两套信息。补齐顺序：
/// 条目自带的 → 已安装 manifest → 本地 ZIP 里的 manifest。
///
/// 只在清单落地时跑一次（进页 / 刷新结果），不进渲染或按钮判据的热路径。
pub fn enrich_from_local(list: &mut [AvailablePlugin], installed: &[Extension]) {
    for e in list {
        if !e.min_host.is_empty() {
            continue;
        }
        if let Some(ext) = installed.iter().find(|x| x.manifest.id == e.id) {
            e.min_host = ext.manifest.min_host.clone();
            continue;
        }
        if let Some(m) = local_package_manifest(e) {
            e.min_host = m.min_host;
        }
    }
}

/// 读本地 ZIP 里的 `manifest.json`（没有包 / 读不出 = `None`）。
fn local_package_manifest(entry: &AvailablePlugin) -> Option<Manifest> {
    let path = local_package_path(entry)?;
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut item = archive.by_name("manifest.json").ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut item, &mut text).ok()?;
    Manifest::parse(&text).ok()
}

/// 从 `docs/<file>` 安装一个 ZIP 包到 `<exe>/plugins/<id>/`。
///
/// 解压到隐藏 staging，校验 manifest / 入口后再整目录 rename；失败不留半装状态。
pub fn install_local(entry: &AvailablePlugin) -> Result<(), String> {
    let docs = crate::core::paths::docs_dir().ok_or("取不到 exe 目录，插件无处安装")?;
    let root = crate::core::paths::plugin_dir().ok_or("取不到 exe 目录，插件无处安装")?;
    let zip_path = docs.join(&entry.file);
    let bytes = std::fs::read(&zip_path)
        .map_err(|e| format!("读插件包 {} 失败：{e}", zip_path.display()))?;
    install_zip(&bytes, &root, &entry.id, false)
}

/// 安装已经取到内存里的包（远程下载线程用）。
pub(super) fn install_bytes(
    bytes: &[u8],
    entry: &AvailablePlugin,
    replace: bool,
) -> Result<(), String> {
    let root = crate::core::paths::plugin_dir().ok_or("取不到 exe 目录，插件无处安装")?;
    install_zip(bytes, &root, &entry.id, replace)
}

// ★ 2026-11-02：原先这里有个 `verify_package`，按清单的 `size` / `sha256` 校验整枚 ZIP。
// 两样都去掉了（用户口径：官方产物不需要指纹，识别一枚插件只认 `id` + `version`）——
// 插件包每改一个可读文件都要重算重传哈希，是发布链上最容易忘、忘了就报"大小不符"的一环。
// 包身份由 ZIP 结构本身兜住：必须有 `manifest.json`（读出来核 `id` / `protocol`），
// 目录名与 `id` 对不上就整包拒绝（见 `install_zip`）。

fn install_zip(bytes: &[u8], root: &Path, id: &str, replace: bool) -> Result<(), String> {
    const MAX_FILES: usize = 10_000;
    const MAX_TOTAL: u64 = 512 * 1024 * 1024;

    // AvailablePlugin 可由调用方直接构造，路径使用点不能只信清单解析期。
    crate::core::ext::validate_id(id)?;
    let dest = root.join(id);
    if dest.exists() && !replace {
        return Err(format!("插件已安装：{id}"));
    }
    std::fs::create_dir_all(root).map_err(|e| format!("创建 {} 失败：{e}", root.display()))?;
    let staging = root.join(format!(".installing-{id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);

    let result = (|| {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!("ZIP 解析失败：{e}"))?;
        if archive.len() > MAX_FILES {
            return Err(format!("ZIP 文件项过多（{} > {MAX_FILES}）", archive.len()));
        }
        std::fs::create_dir_all(&staging)
            .map_err(|e| format!("创建 {} 失败：{e}", staging.display()))?;
        let mut total = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("读取 ZIP 第 {} 项失败：{e}", i + 1))?;
            let Some(rel) = entry.enclosed_name() else {
                return Err(format!("ZIP 第 {} 项路径越界", i + 1));
            };
            if entry.is_dir() {
                std::fs::create_dir_all(staging.join(rel))
                    .map_err(|e| format!("建目录失败：{e}"))?;
                continue;
            }
            total = total.saturating_add(entry.size());
            if total > MAX_TOTAL {
                return Err("插件包解压后超过 512 MiB".into());
            }
            let out = staging.join(rel);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
            }
            let mut f = std::fs::File::create(&out)
                .map_err(|e| format!("创建 {} 失败：{e}", out.display()))?;
            std::io::copy(&mut entry, &mut f).map_err(|e| format!("解压失败：{e}"))?;
        }
        let manifest = Manifest::from_dir(&staging)?;
        if manifest.id != id {
            return Err(format!("包内 id {} ≠ 清单 id {id}", manifest.id));
        }
        if !staging.join(&manifest.entry).is_file() {
            return Err(format!("包内缺少入口 {}", manifest.entry));
        }
        if dest.exists() {
            // 先读旧设置：**读不动就在这行中止**，此刻 dest / staging 都还没被碰过，
            // 用户装的是旧版还是新版都完好（旧写法是换完目录才发现搬不走，偏好直接没了）。
            let carried = settings::snapshot_for_upgrade(&dest, &staging)?;
            let backup = root.join(format!(".old-{id}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&backup);
            std::fs::rename(&dest, &backup).map_err(|e| format!("旧版插件占着目录：{e}"))?;
            if let Err(e) = std::fs::rename(&staging, &dest) {
                let _ = std::fs::rename(&backup, &dest);
                return Err(format!("更新 {id} 失败：{e}"));
            }
            match carried {
                Some(bytes) => {
                    if let Err(e) = settings::restore_settings(&dest, &bytes) {
                        // 升级已经成功、设置写不进去：留着备份目录（旧版 + 旧设置都在里面），
                        // 并把路径写进日志——这是"宁可留一份脏东西也不静默丢数据"的唯一一处。
                        log::warn!("{e}；旧版目录保留在 {}，可手动取回设置", backup.display());
                        return Ok(());
                    }
                    log::info!("插件 {id} 升级：旧设置区已保留");
                }
                None => {
                    if staging.join(settings::FILE_NAME).is_file() {
                        log::info!("插件 {id} 升级：包自带设置区，沿用新包的那份");
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&backup);
        } else {
            std::fs::rename(&staging, &dest).map_err(|e| format!("安装 {id} 失败：{e}"))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    } else {
        // 插件集变了：动态热键表要重新扫盘才能看见新插件。
        invalidate_installed_cache();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ext::{MIN_PROTOCOL_VERSION, PROTOCOL_VERSION};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = crate::core::paths::test_scratch_root().join(format!("rupix-ext-{tag}-{n}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("建临时目录");
        p
    }

    /// 一枚最小可安装的插件包（manifest + 入口）。三条安装用例共用一份，
    /// 免得"改 manifest 字段"时改了两处漏一处。
    fn ocr_package() -> Vec<u8> {
        use std::io::{Cursor, Write};
        use zip::write::SimpleFileOptions;
        let manifest = r#"{
            "schema":1,"id":"com.example.ocr","name":"OCR","version":"1.0.0",
            "protocol":1,"min_host":"0.1.0","kind":"command","entry":"x.exe",
            "capabilities":[],"platforms":["windows-x64"]
        }"#;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut buf);
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("manifest.json", opts).expect("zip manifest");
            zw.write_all(manifest.as_bytes()).expect("write manifest");
            zw.start_file("x.exe", opts).expect("zip exe");
            zw.write_all(b"dummy").expect("write exe");
            zw.finish().expect("finish zip");
        }
        buf.into_inner()
    }

    fn write_ext(root: &Path, id: &str) {
        write_ext_at(root, id, id, "1.0.0");
    }

    /// 夹具的完整形状：目录名、自述 id、自述版本三者**可以不一致**——
    /// 裸短名 id（m162）之后"两枚同 id"变成随手可得的现场，这些组合必须被判定过。
    fn write_ext_at(root: &Path, dir_name: &str, id: &str, version: &str) {
        let dir = root.join(dir_name);
        fs::create_dir_all(&dir).expect("建扩展目录");
        let text = format!(
            r#"{{
                "schema":1,"id":"{id}","name":"{id}","version":"{version}","protocol":1,
                "min_host":"0.1.0","kind":"command","entry":"x.exe",
                "capabilities":["image.read"],"platforms":["windows-x64"]
            }}"#
        );
        fs::write(dir.join("manifest.json"), text).expect("写 manifest");
    }

    /// 同 id 只列一版：版本更高的赢，另一份不进列表。
    ///
    /// ★ 这条不是"防撞名保险"，而是**列表必须有确定答案**：两枚同 id 落进 `plugins/` 时，
    /// 旧代码会并排给出两行、停用表一次命中两枚、热键注册两次（行为未定义）。
    #[test]
    fn same_id_plugins_are_arbitrated_to_the_higher_version() {
        let root = crate::core::paths::test_scratch_root().join("ext-dup-id");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_ext_at(&root, "aaa_old", "doodle", "1.0.0");
        write_ext_at(&root, "zzz_new", "doodle", "1.2.0");

        let found = discover_in(&root, &[], &[]);
        let listed: Vec<_> = found
            .iter()
            .map(|e| (e.manifest.id.clone(), e.manifest.version.clone()))
            .collect();
        assert_eq!(found.len(), 1, "同 id 必须只列一枚，实际列出 {listed:?}");
        assert_eq!(found[0].manifest.version, "1.2.0", "留下的要版本更高那一份");
        assert!(
            found[0].dir.ends_with("zzz_new"),
            "留下的必须是那份高版本的目录"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// 版本也相同 ⇒ 取目录名字典序第一份：同一个目录必须永远给同一个答案，
    /// 不许跟着 `read_dir` 的顺序漂（否则用户每次看到的"是哪一枚"都可能不同）。
    #[test]
    fn duplicate_ids_with_equal_versions_are_picked_deterministically() {
        let root = crate::core::paths::test_scratch_root().join("ext-dup-equal");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_ext_at(&root, "aaa_first", "echo", "1.0.0");
        write_ext_at(&root, "bbb_second", "echo", "1.0.0");

        for round in 0..3 {
            let found = discover_in(&root, &[], &[]);
            assert_eq!(found.len(), 1, "第 {round} 轮：同 id 同版本也只列一枚");
            assert!(
                found[0].dir.ends_with("aaa_first"),
                "第 {round} 轮：取字典序第一份，实际 {}",
                found[0].dir.display()
            );
        }

        let _ = fs::remove_dir_all(&root);
    }

    /// 目录名与自述 id 不一致的那一枚**照常列出**（发现环节不因它丢插件），
    /// 只在日志里点名——代价说清楚：宿主按 id 找 `settings.json` 与卸载目标。
    /// 这条约定在插件仓是硬门禁（`tools/check_manifests.py` 断言目录名 == id）。
    #[test]
    fn dir_name_that_disagrees_with_the_id_is_still_listed() {
        let root = crate::core::paths::test_scratch_root().join("ext-dirname");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_ext_at(&root, "renamed_by_hand", "ocr", "1.0.0");

        let found = discover_in(&root, &[], &[]);
        assert_eq!(found.len(), 1, "目录名不合规不许把插件变没");
        assert_eq!(found[0].manifest.id, "ocr");

        let _ = fs::remove_dir_all(&root);
    }

    /// 可下载条目的兼容判据：协议走 `MIN..=MAX`，宿主走 `min_host`；
    /// 旧清单没有 `min_host` = 不限制（不能因为字段缺席就把老清单全灰掉）。
    #[test]
    fn available_compatibility_covers_protocol_and_host() {
        let mut p = AvailablePlugin {
            id: "a".into(),
            name: "A".into(),
            version: "1.0.0".into(),
            protocol: PROTOCOL_VERSION,
            min_host: String::new(),
            kind: Kind::Command,
            description: String::new(),
            file: "a.zip".into(),
            size: 1,
            capabilities: Vec::new(),
            released: String::new(),
            author: String::new(),
            license: String::new(),
            urls: std::collections::HashMap::new(),
            installed: false,
        };
        assert!(p.compatible(), "协议/宿主都满足");
        p.protocol = PROTOCOL_VERSION + 1;
        assert!(!p.compatible(), "高于宿主最新协议必须拒");
        p.protocol = MIN_PROTOCOL_VERSION;
        p.min_host = "99.0.0".into();
        assert!(!p.compatible(), "宿主版本不够必须拒");
        p.min_host.clear();
        assert!(p.compatible(), "旧清单没 min_host 不该被判死");
    }

    /// 旧索引没 `min_host` 时，用已安装 manifest 补齐——可下载与已安装两处才一致。
    #[test]
    fn enrich_min_host_from_installed_manifest() {
        let manifest = Manifest::parse(
            r#"{"schema":1,"id":"com.example.d","name":"D","version":"1.0.0",
                "protocol":1,"min_host":"0.2.0","kind":"editor","entry":"x.exe",
                "capabilities":[],"platforms":["windows-x64"]}"#,
        )
        .expect("manifest");
        let installed = vec![Extension {
            manifest,
            dir: std::path::PathBuf::new(),
            enabled: true,
            blocked: false,
            isolated: false,
        }];
        let mut p = AvailablePlugin {
            id: "com.example.d".into(),
            name: "D".into(),
            version: "1.0.0".into(),
            protocol: 1,
            min_host: String::new(),
            kind: Kind::Editor,
            description: String::new(),
            file: "d.zip".into(),
            size: 1,
            capabilities: Vec::new(),
            released: String::new(),
            author: String::new(),
            license: String::new(),
            urls: std::collections::HashMap::new(),
            installed: true,
        };
        enrich_from_local(std::slice::from_mut(&mut p), &installed);
        assert_eq!(p.min_host, "0.2.0");
        // 已有值不覆盖（索引是新的时候以索引为准）。
        p.min_host = "0.3.0".into();
        enrich_from_local(std::slice::from_mut(&mut p), &installed);
        assert_eq!(p.min_host, "0.3.0");
    }

    /// 「占用空间（解压后）」：目录递归求和；目录不存在返回 None。
    #[test]
    fn dir_size_sums_files_recursively() {
        let root = temp_root("dir-size");
        fs::write(root.join("a.bin"), vec![0u8; 10]).expect("写 a");
        fs::create_dir_all(root.join("sub")).expect("建子目录");
        fs::write(root.join("sub").join("b.bin"), vec![0u8; 20]).expect("写 b");
        assert_eq!(dir_size(&root), Some(30), "子目录里的文件也要算");
        assert_eq!(dir_size(&root.join("missing")), None);
        let _ = fs::remove_dir_all(&root);
    }

    /// 造一枚带热键声明的扩展（纯内存，不落盘）。
    fn ext_with(id: &str, kind: &str, hotkey: &str) -> Extension {
        let text = format!(
            r#"{{"schema":1,"id":"{id}","name":"{id}","version":"1.0.0","protocol":1,
                "min_host":"0.1.0","kind":"{kind}","entry":"x.exe",
                "capabilities":["image.read"],"platforms":["windows-x64"],
                "hotkey":"{hotkey}"}}"#
        );
        let manifest = Manifest::parse(&text).expect("manifest 应能解析");
        Extension {
            manifest,
            dir: PathBuf::new(),
            enabled: true,
            blocked: false,
            isolated: false,
        }
    }

    /// 动态热键的裁决：内置优先、插件间先来先得、不可用的插件不进表。
    #[test]
    fn hotkey_table_applies_the_conflict_rules() {
        let builtins = Hotkeys::default();
        // `R` 是 Rotate 的出厂键位 → 插件抢不走。
        let clash = ext_with("com.example.a", "command", "R");
        // 没人占的键 → 生效。
        let ok = ext_with("com.example.b", "command", "Ctrl+Shift+O");
        let list = vec![clash, ok];
        let table = build_hotkey_table(&list, &builtins);
        assert_eq!(table.entries.len(), 1, "只有不冲突的那条该进表");
        assert_eq!(table.entries[0].plugin_id, "com.example.b");

        // 两个插件抢同一把键：先来先得（列表序 = 插件 id 序）。
        let first = ext_with("com.example.a", "command", "Ctrl+Shift+I");
        let second = ext_with("com.example.b", "command", "Ctrl+Shift+I");
        let table = build_hotkey_table(&[first, second], &builtins);
        assert_eq!(table.entries.len(), 1);
        assert_eq!(table.entries[0].plugin_id, "com.example.a");

        // 停用 / 隔离 / min_host 不满足 / 非命令型：热键都不生效。
        let mut disabled = ext_with("com.example.c", "command", "Ctrl+Shift+P");
        disabled.enabled = false;
        let mut isolated = ext_with("com.example.d", "command", "Ctrl+Shift+Q");
        isolated.isolated = true;
        let mut blocked = ext_with("com.example.e", "command", "Ctrl+Shift+R");
        blocked.blocked = true;
        let editor = ext_with("com.example.f", "editor", "Ctrl+Shift+S");
        let table = build_hotkey_table(&[disabled, isolated, blocked, editor], &builtins);
        assert!(
            table.entries.is_empty(),
            "不可用的插件不该带热键进表：{:?}",
            table.entries
        );
    }

    /// 动态热键表：按下声明的那把键要能找回插件。
    #[test]
    fn hotkey_table_matches_the_declared_key() {
        let builtins = Hotkeys::default();
        let list = vec![ext_with("com.example.ocr", "command", "Ctrl+Shift+O")];
        let table = build_hotkey_table(&list, &builtins);
        let hit = table
            .match_plugin(&KeyInput {
                key: crate::core::hotkey::VKey::Char('O'),
                ctrl: true,
                alt: false,
                shift: true,
                win: false,
            })
            .expect("应命中动态热键");
        assert_eq!(hit.plugin_id, "com.example.ocr");
    }

    #[test]
    fn discovers_valid_and_skips_bad() {
        let root = temp_root("discover");
        write_ext(&root, "com.example.ocr");
        write_ext(&root, "com.example.doodle");
        let bad = root.join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("manifest.json"), "{not json").unwrap();

        let list = discover_in(&root, &[], &[]);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].manifest.id, "com.example.doodle");
        assert!(list[0].enabled);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn audit_catalog_rejects_bad_ids_before_other_fields() {
        for id in [
            "",
            "ab",
            "..",
            "../sentinel",
            "a/b",
            "a\\b",
            "C:bad",
            " bad",
            "插件",
        ] {
            let quoted = id.replace('\\', "\\\\");
            let text = format!(
                r#"[{{"id":"{quoted}","name":"P","version":"1.0.0","protocol":1,"kind":"command","file":"p.zip"}}]"#
            );
            assert!(
                parse_catalog(&text, &[]).is_err(),
                "bad id accepted: {id:?}"
            );
            let incomplete = format!(r#"[{{"id":"{quoted}"}}]"#);
            let err = parse_catalog(&incomplete, &[]).expect_err("id must be checked first");
            // 判据是**顺序**而不是文案：只给 id 的项如果先查了别的字段，错误里
            // 必然提到 name / version（那说明坏 id 被放过了）。
            assert!(
                !err.contains("name") && !err.contains("version"),
                "id 校验必须排在其他字段之前：{err}"
            );
        }
        let long = "a".repeat(65);
        let err = parse_catalog(&format!(r#"[{{"id":"{long}"}}]"#), &[]).expect_err("long id");
        assert!(
            !err.contains("name") && !err.contains("version"),
            "超长 id 也要在字段校验之前被挡下：{err}"
        );
    }

    #[test]
    fn audit_install_rejects_bad_ids_before_filesystem() {
        let sandbox = temp_root("audit-id");
        let root = sandbox.join("plugins");
        fs::create_dir(&root).expect("plugins");
        let sentinel = sandbox.join("sentinel");
        fs::create_dir(&sentinel).expect("sentinel");
        fs::write(sentinel.join("keep"), b"untouched").expect("sentinel contents");
        // Only fabricated paths under this sandbox: even the unfixed code cannot touch user data.
        let absolute = sentinel.to_string_lossy().into_owned();
        let mut results = Vec::new();
        for id in ["../sentinel", "..\\sentinel", absolute.as_str(), "ab", ""] {
            for replace in [false, true] {
                results.push(install_zip(b"not a zip", &root, id, replace));
            }
        }
        let missing = sandbox.join("not-created");
        results.push(install_zip(b"not a zip", &missing, "../sentinel", true));
        let untouched = fs::read(sentinel.join("keep"));
        let created = missing.exists();
        fs::remove_dir_all(&sandbox).expect("cleanup own sandbox");
        for result in results {
            let err = result.expect_err("bad id must fail");
            // 判据是"没走到 ZIP / 目录那一步"（文案措辞不算判据）：
            // 真往下走只会报解析或"已安装"，那说明坏 id 被放过了。
            assert!(
                !err.contains("ZIP") && !err.contains("已安装"),
                "must reject id before path/ZIP operations: {err}"
            );
        }
        assert!(!created, "invalid id must not create the installation root");
        assert_eq!(untouched.expect("sentinel remains"), b"untouched");
    }

    #[test]
    fn catalog_marks_already_installed_ids() {
        let text = r#"[
            {"id":"com.example.ocr","name":"OCR","version":"1.0.0","protocol":1,
             "kind":"command","file":"com.example.ocr_1.0.0.zip",
             "capabilities":["image.read"]},
            {"id":"com.example.doodle","name":"Doodle","version":"1.0.0","protocol":1,
             "kind":"editor","file":"com.example.doodle_1.0.0.zip"}
        ]"#;
        let list = parse_catalog(text, &["com.example.ocr".to_string()]).expect("catalog");
        assert_eq!(list.len(), 2);
        let ocr = list
            .iter()
            .find(|p| p.id == "com.example.ocr")
            .expect("ocr");
        let doodle = list
            .iter()
            .find(|p| p.id == "com.example.doodle")
            .expect("doodle");
        assert!(ocr.installed);
        assert!(!doodle.installed);
    }

    #[test]
    fn catalog_reads_remote_metadata_and_per_source_urls() {
        let gitee =
            "https://gitee.com/qiuzongman/RuPix-Plugins/releases/download/a_1.0.0/a_1.0.0.zip";
        let github =
            "https://github.com/zongmanqiu/RuPix-Plugins/releases/download/a_1.0.0/a_1.0.0.zip";
        let text = format!(
            r#"[{{"id":"com.example.a","name":"A","version":"1.0.0","protocol":1,"kind":"command",
                 "file":"a_1.0.0.zip","size":3,"released":"2026-10-28","author":"A",
                 "license":"MIT","gitee":"{gitee}","github":"{github}"}}]"#
        );
        let list = parse_catalog(&text, &[]).expect("catalog");
        let e = &list[0];
        assert_eq!(e.released, "2026-10-28");
        assert_eq!(e.author, "A");
        assert_eq!(e.license, "MIT");
        assert_eq!(
            e.download_url(crate::core::config::SourceId::Gitee),
            Some(gitee)
        );
        assert_eq!(
            e.download_url(crate::core::config::SourceId::Github),
            Some(github)
        );
    }

    /// 旧清单里残留的 `sha256` / `size` **必须被无声忽略**（2026-11-02 用户口径：插件也不校验
    /// 哈希，只认 `id` + `version`）。线上清单改不了，老格式必须读得动。
    #[test]
    fn legacy_sha256_key_in_plugin_manifest_is_ignored() {
        let text = r#"[
          {
            "id": "com.example.ocr",
            "name": "OCR",
            "version": "1.0.0",
            "protocol": 1,
            "kind": "command",
            "file": "ocr-1.0.0.zip",
            "size": 4096,
            "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "capabilities": [],
            "gitee": "https://example.com/ocr-1.0.0.zip"
          }
        ]"#;
        let list = parse_catalog(text, &[]).expect("多一个 sha256 键不该让整份清单作废");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "com.example.ocr");
    }

    #[test]
    fn install_zip_extracts_and_rejects_duplicate() {
        let pkg = ocr_package();
        let plugins = temp_root("plugins");
        install_zip(&pkg, &plugins, "com.example.ocr", false).expect("install");
        assert!(plugins.join("com.example.ocr/manifest.json").is_file());
        assert!(install_zip(&pkg, &plugins, "com.example.ocr", false).is_err());
        install_zip(&pkg, &plugins, "com.example.ocr", true).expect("update");
        let _ = fs::remove_dir_all(&plugins);
    }

    /// 升级不能把插件的设置区冲掉——升级是常规操作，不是事故。
    #[test]
    fn upgrade_keeps_plugin_settings() {
        let pkg = ocr_package();
        let plugins = temp_root("plugins_upgrade");
        install_zip(&pkg, &plugins, "com.example.ocr", false).expect("install");
        // 用户用了一阵，攒下偏好。
        let settings = plugins.join("com.example.ocr/settings.json");
        fs::write(&settings, b"{\"lang\":\"chi_sim\"}").expect("写设置");
        // 升级。
        install_zip(&pkg, &plugins, "com.example.ocr", true).expect("update");
        let kept = fs::read_to_string(&settings).expect("升级后设置区还在");
        assert!(kept.contains("chi_sim"), "升级把用户的偏好冲掉了：{kept}");
        // 备份 / staging 目录必须清干净：留着会被当成第二枚已安装插件扫出来。
        let leftovers: Vec<String> = fs::read_dir(&plugins)
            .expect("读 plugins")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".old-") || n.starts_with(".installing-"))
            .collect();
        assert!(leftovers.is_empty(), "升级留下脏目录：{leftovers:?}");
        let _ = fs::remove_dir_all(&plugins);
    }

    /// 旧设置**读不动**时，升级必须在动任何目录之前就放弃：旧版与旧偏好都得原封不动。
    ///
    /// 旧写法是换完目录才复制设置，复制失败只 warn、随后把备份目录删掉
    /// ⇒ 用户升一次插件偏好全丢，且没有任何地方能找回。这条用例钉的是"先读后动"的顺序。
    #[cfg(windows)]
    #[test]
    fn unreadable_old_settings_aborts_upgrade_without_touching_anything() {
        use std::os::windows::fs::OpenOptionsExt;
        let pkg = ocr_package();
        let plugins = temp_root("plugins_locked");
        install_zip(&pkg, &plugins, "com.example.ocr", false).expect("install");
        let dir = plugins.join("com.example.ocr");
        let settings = dir.join("settings.json");
        fs::write(&settings, b"{\"lang\":\"chi_sim\"}").expect("写设置");
        // share_mode(0)：之后任何一次 open 这个文件都会撞 ERROR_SHARING_VIOLATION。
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&settings)
            .expect("独占句柄");
        let err = install_zip(&pkg, &plugins, "com.example.ocr", true)
            .expect_err("旧设置读不动时必须放弃升级");
        drop(held);
        assert!(err.contains("设置区"), "要说清是哪一步读不动：{err}");
        // 中止发生在 rename 之前：旧版目录与旧偏好原封不动。
        assert!(dir.join("manifest.json").is_file(), "旧版目录被动过");
        assert_eq!(
            fs::read_to_string(&settings).expect("旧设置必须还在"),
            "{\"lang\":\"chi_sim\"}"
        );
        let _ = fs::remove_dir_all(&plugins);
    }

    #[test]
    fn disabled_list_is_respected() {
        let root = temp_root("disabled");
        write_ext(&root, "com.example.ocr");
        let list = discover_in(&root, &["com.example.ocr".to_string()], &[]);
        assert_eq!(list.len(), 1);
        assert!(!list[0].enabled);
        assert!(!is_enabled(
            "com.example.ocr",
            &["com.example.ocr".to_string()]
        ));
        let _ = fs::remove_dir_all(&root);
    }
    #[test]
    fn vacate_clears_the_previous_host_action_with_the_same_key() {
        // 用户 2026-10-31 口径：新绑的键 = 唯一持有者，旧的自动变空。
        use crate::core::hotkey::{Action, Hotkey};
        let mut cfg = crate::core::config::Config::default();
        let f4 = Hotkey::from_spec("F4").expect("F4 是合法键串");
        cfg.hotkeys.set_binding(Action::Next, f4);
        assert!(
            cfg.hotkeys.bindings[Action::Next.index()].contains(&f4),
            "前置条件：Next 已绑 F4"
        );
        super::vacate_hotkey(&mut cfg, &f4, None);
        assert!(
            !cfg.hotkeys.bindings[Action::Next.index()].contains(&f4),
            "同键的动作必须被清空"
        );
    }
    #[test]
    fn binding_a_host_action_keeps_the_key_it_just_bound() {
        // 顺序钉：先清后绑。反了的话这里会读到空表（"F4 绑不上"）。
        use crate::core::hotkey::{Action, Hotkey};
        let mut cfg = crate::core::config::Config::default();
        let f4 = Hotkey::from_spec("F4").expect("F4 是合法键串");
        super::bind_host_action(&mut cfg, Action::Next, f4);
        assert!(
            cfg.hotkeys.bindings[Action::Next.index()].contains(&f4),
            "刚绑上的键不许被自己的清场逻辑抹掉"
        );
    }
}
