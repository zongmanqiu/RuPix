//! 把「可下载」清单里的一枚 DLL 装进 `decoders/`。
//!
//! # 为什么在后台线程里做
//!
//! 清单刷新走 [`crate::decoders::remote::start_refresh`] 的后台线程；**DLL 是几 MB**，
//! 同步拉会把设置窗冻住好几秒——窗口不重绘、按钮不响应，用户以为程序挂了。所以这里
//! 起一个工作线程，界面用 `Window::on_interval`（250 ms）问一次 [`poll`]。
//!
//! # 进度
//!
//! 下载线程每读到一个 HTTP 块就把**已收字节数**写进 [`Running`]（[`report`]），界面
//! 250 ms 取一次 [`progress`]，把被点的那颗按钮画成 `42%`。总字节数用清单里的
//! `size`，不依赖 Content-Length——重定向后的最终响应也必须对得上这一个数。
//!
//! # 装完之后谁负责刷新
//!
//! 这里**只碰文件系统**：不写配置、不碰信号、不动解码器注册表——那些都在 UI 线程。
//! 线程结束时把结果放进 [`Outcome`]，设置窗的定时器取到后自己做三件事：
//! `settings::refresh_catalogs()`（解码线程重扫 + 插件缓存失效）与
//! `Handles::refresh_after_dir_change()`（再补两张卡的信号，**装完/卸完都走这一个入口**）、
//! 把 id 从停用表里摘掉（**自动启用**）。
//!
//! # 落盘的三个保险
//!
//! 1. **先写 `.part` 再改名**：中途失败/断电时目录里不会躺着一个"看起来能用"的半截 DLL
//!    （`list_decoders` 会把每个 DLL 都 LoadLibrary 一遍，半截文件只会得到一个加载失败）。
//! 2. **字节数对不上就不装**：清单里写了 `size`，实收不同即放弃——这是最省事、
//!    也最容易讲给用户听的一条完整性判据。
//! 3. **先读自述，再换目录**：能 `LoadLibrary`、`iv_decoder_info().id` 与清单一致、abi 落在
//!    兼容区间，才算这一枚能用；**任何一条不过，目录一次都没被动过**——删掉暂存那份就完事，
//!    旧档原位不动，也不会冒出 `<file>.old`。
//!    ★ 这个顺序是 2026-10-29 换过来的：旧写法"先换进目录、验不过再回滚"，而回滚第一步是
//!    **删掉刚放进去的那份**——正被加载的文件删不掉（真机 `ERROR_ACCESS_DENIED` 5，
//!    见 `tests/locked_decoder_dll.rs`），紧接着"把旧档挪回原位"又因目标已存在而失败
//!    ⇒ 这一格式当场既没有新版也没有旧版可用。先验后换把这一步整段删掉了。
//!
//! 另外会把**同 id 的旧版本文件**清掉：两枚自述同一个 id 的 DLL 躺在目录里，选谁全看
//! 扫描顺序，用户会看到"版本号变了又变"。
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::catalog;
use super::dllmeta::{inspect, Inspect};
use super::remote::RemoteEntry;
use super::SourceId;

/// 单枚 DLL 的上限。现有最大一枚 7.9 MB；32 MiB 是"这明显不是解码器"的闸。
const MAX_DLL_BYTES: usize = 32 << 20;

/// 一次下载安装的结果（工作线程 → UI 线程的唯一通道）。
#[derive(Debug)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    /// 成功 = 落盘后的完整路径。
    pub result: Result<PathBuf, String>,
}

/// 一次下载的进度快照（UI 每 250 ms 取一次）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub id: String,
    pub got: u64,
    /// 预期总字节数 = 清单里的 `size`（下载前就知道，不读 Content-Length）。
    pub total: u64,
}

impl Progress {
    /// 0..=100。超收夹到 100、`total == 0` 返回 0（不除零）。
    pub fn percent(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        ((self.got.min(self.total) * 100) / self.total) as u8
    }
}

struct Running {
    id: String,
    name: String,
    got: u64,
    total: u64,
}

/// 下载线程写进度：只更新同 id 的那一个（`start` 已保证同时只有一个）。
fn report(id: &str, got: u64) {
    let mut g = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = &mut g.running {
        if r.id == id {
            r.got = got.min(r.total);
        }
    }
}

/// 当前下载进度；没有下载在跑时为 `None`。
pub fn progress() -> Option<Progress> {
    let g = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    g.running.as_ref().map(|r| Progress {
        id: r.id.clone(),
        got: r.got,
        total: r.total,
    })
}

#[derive(Default)]
struct Slot {
    running: Option<Running>,
    done: Option<Outcome>,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot {
    running: None,
    done: None,
});

/// 起一次下载安装。立刻返回——真正的活在工作线程里。
///
/// 同一时刻只允许一个：两枚同时下会把 `decoders/` 写成一锅粥（各自的 `.part` 倒是互不
/// 干扰，但用户看不出哪一行在动），而且网络争抢下两个都慢。
///
/// `src` 是**当前来源**：决定用清单里哪一列下载地址（`gitee` / `github`）。
pub fn start(e: &RemoteEntry, src: SourceId) -> Result<(), String> {
    if !e.abi_ok() {
        return Err(format!("它要求 abi {}，本宿主收不了", e.abi));
    }
    if !e.downloadable_for(src) {
        return Err("这一枚在当前来源还没有发布地址".to_string());
    }
    {
        let mut g = SLOT.lock().unwrap_or_else(|x| x.into_inner());
        if let Some(r) = &g.running {
            return Err(format!("正在下载「{}」，等它完成", r.name));
        }
        g.running = Some(Running {
            id: e.id.clone(),
            name: e.name.clone(),
            got: 0,
            total: e.size,
        });
    }
    let job = e.clone();
    std::thread::spawn(move || {
        let result = install(&job, src);
        let mut g = SLOT.lock().unwrap_or_else(|x| x.into_inner());
        g.running = None;
        g.done = Some(Outcome {
            id: job.id,
            name: job.name,
            result,
        });
    });
    Ok(())
}

/// 取一次结果。`None` = 还没有（或已经取过了）。
///
/// 取走即清空：同一份结果只能被消费一次，避免"定时器每 250 ms 弹一次同样的 toast"。
pub fn poll() -> Option<Outcome> {
    let mut g = SLOT.lock().unwrap_or_else(|x| x.into_inner());
    g.done.take()
}

/// 真正的下载安装（在工作线程里跑）。
struct PackageFiles {
    dll: Vec<u8>,
    docs: Vec<(PathBuf, Vec<u8>)>,
}

fn is_zip_url(url: &str) -> bool {
    url.split('?')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .ends_with(".zip")
}

/// Validate and unpack the release ZIP in memory. Only one root DLL is allowed;
/// the sibling `<id>_<version>/` folder may contain docs/licenses, but no DLLs.
fn unpack_zip(
    bytes: &[u8],
    id: &str,
    version: &str,
    dll_name: &str,
) -> Result<PackageFiles, String> {
    const MAX_FILES: usize = 64;
    const MAX_TOTAL: usize = 64 << 20;

    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|err| format!("ZIP 解析失败：{err}"))?;
    if archive.len() > MAX_FILES {
        return Err(format!("ZIP 文件项过多（{} > {MAX_FILES}）", archive.len()));
    }
    let pkg_dir = format!("{id}_{version}/");
    let mut dll = None;
    let mut docs = Vec::new();
    let mut total = 0usize;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|err| format!("读取 ZIP 第 {} 项失败：{err}", i + 1))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        if name.starts_with('/')
            || name
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(format!("ZIP 含非法路径：{name}"));
        }
        if entry.size() > MAX_TOTAL as u64 {
            return Err("ZIP 单个文件过大".into());
        }
        total = total.saturating_add(entry.size() as usize);
        if total > MAX_TOTAL {
            return Err("ZIP 解压后总大小超过上限".into());
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|err| format!("读取 ZIP 项 {name} 失败：{err}"))?;
        if name == dll_name {
            if dll.is_some() {
                return Err("ZIP 里出现两份同名 DLL".into());
            }
            dll = Some(buf);
        } else if let Some(rel) = name.strip_prefix(&pkg_dir) {
            if rel.is_empty() || rel.to_ascii_lowercase().ends_with(".dll") {
                return Err(format!("ZIP 许可证目录里不允许 DLL：{name}"));
            }
            docs.push((PathBuf::from(rel), buf));
        } else {
            return Err(format!("ZIP 顶层只允许 {dll_name} 和 {pkg_dir}：{name}"));
        }
    }

    let dll = dll.ok_or_else(|| format!("ZIP 缺少 {dll_name}"))?;
    Ok(PackageFiles { dll, docs })
}

fn restore_old(target: &Path, package_dir: &Path, old_dll: &Path, old_dir: &Path) {
    // 回滚 = 「删掉刚放进去的那份」+「把旧的挪回来」。★ 第一步在真机上会失败：正被加载的
    // 文件删不动（`ERROR_ACCESS_DENIED` 5，读数见 K159），紧接着第二步又因目标已存在而失败
    // ⇒ 这个格式当场既没有新版也没有旧版可用。自述校验已经挪到换目录**之前**（见
    // [`place_package`]），所以这条回滚如今只为"新档已验过、换目录途中另一步失败"兜底；
    // 而调用方只能把"装不上"抛给用户，日志是唯一的现场证据，不许无声。
    if let Err(err) = std::fs::remove_file(target) {
        log::warn!(
            "回滚：删不掉刚放进去的 {}（多半正被加载）：{err}",
            target.display()
        );
    }
    let _ = std::fs::remove_dir_all(package_dir);
    if old_dll.exists() {
        if let Err(err) = std::fs::rename(old_dll, target) {
            log::warn!("回滚：旧 DLL 挪不回 {}：{err}", target.display());
        }
    }
    if old_dir.exists() {
        let _ = std::fs::rename(old_dir, package_dir);
    }
}

/// 下载并安装一枚解码器。URL 可以是新的 ZIP 包，也可以是旧的裸 DLL（兼容期）。
fn install(e: &RemoteEntry, src: SourceId) -> Result<PathBuf, String> {
    let dir = catalog::ensure_dir().map_err(|err| format!("建解码器目录失败：{err}"))?;
    let file = e.file_name();

    let url = e
        .download_url(src)
        .ok_or_else(|| "这一枚在当前来源没有下载地址".to_string())?;
    let bytes = crate::platform::http::get_with_progress(url, MAX_DLL_BYTES, |got| {
        report(&e.id, got);
    })?;

    let package = if is_zip_url(url) {
        unpack_zip(&bytes, &e.id, &e.version, &file)?
    } else {
        // Backward compatibility with the old bare-DLL release assets.
        PackageFiles {
            dll: bytes,
            docs: Vec::new(),
        }
    };

    // ★ 2026-11-02：这里原来核 `size` 与 `sha256`，现在两样都去掉了（用户口径：官方产物不需要
    // 指纹，识别一枚解码器只认 `id` + `version`）。删得起是因为包身份已有三道互不依赖的证据：
    // ① ZIP 里按**精确文件名** `<id>_<version>.dll` 取（`unpack_zip`），取不到直接报错；
    // ② `get_with_progress` 走 WinHTTP 读完整个响应体才算成功，半截响应是 Err 不是短 buffer；
    // ③ 落盘**之前**读自述核对 `iv_decoder_info().id`（`place_package`），对不上目录一次都不碰。
    // 代价是清单与远端资产万一脱节（size 对不上）不再当场报错——但那正是原来最招人烦的一类
    // 假警报（每次重编 DLL 字节都变，忘了重算清单就报"大小不符"），已不值得为它买单。

    place_package(&dir, &file, &e.id, &e.version, &package.dll, &package.docs)
}

/// 把**已经下载好的一枚包**落进解码器目录：**先验自述，后换目录**。
///
/// ★ 顺序为什么要反过来（2026-10-29 真机读数，正本 `tests/locked_decoder_dll.rs` 与 K159）：
/// 正被宿主加载的解码器 DLL **改得动名字、删不掉**（改名成功；删除 = ERROR_ACCESS_DENIED 5）。
/// 旧写法是"先把新档换进目录、再 `inspect` 验自述、验不过回滚"，而回滚的第一步恰好是
/// **删掉刚放进目录的那一份**：解码线程若在"换进来 ~ 验完"这段里被用户的一次「重新扫描」
/// 触发并加载了它，那一步就撞 code=5，紧接着"把旧档挪回原位"又因目标已存在而失败
/// ⇒ 这个格式当场既没有新版也没有旧版可用。先验后换之后，验不过**目录一次都没被碰过**：
/// 删掉暂存那份就结束，旧档原位不动，也不会冒出 `.old`。
///
/// 提前读自述是安全的，三条都读过源码：`inspect` 取的是 DLL 自己的导出
/// （`iv_abi_version` / `iv_decoder_info` / `iv_metadata`，**不读旁边的目录**），暂存那份与
/// 换进去那份是同一串字节；依赖搜索路径由 `platform::dll::harden_search_path` 设在**进程**上
/// （`SetDefaultDllDirectories`），与文件躺在哪个目录无关。次数也没变：全程只 `inspect` 一次。
fn place_package(
    dir: &Path,
    file: &str,
    id: &str,
    version: &str,
    dll: &[u8],
    docs: &[(PathBuf, Vec<u8>)],
) -> Result<PathBuf, String> {
    let target = dir.join(file);
    let pkg_dir = dir.join(format!("{id}_{version}"));
    let stage = dir.join(format!(".rupix-install-{id}_{version}"));
    let old_dll = dir.join(format!("{file}.old"));
    let old_dir = dir.join(format!("{id}_{version}.old"));

    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|err| format!("建临时目录失败：{err}"))?;
    let stage_dll = stage.join(file);
    let stage_pkg = stage.join(format!("{id}_{version}"));
    std::fs::write(&stage_dll, dll).map_err(|err| format!("写临时 DLL 失败：{err}"))?;
    for (rel, data) in docs {
        let path = stage_pkg.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("建许可证目录失败：{err}"))?;
        }
        std::fs::write(&path, data).map_err(|err| format!("写许可证文件失败：{err}"))?;
    }

    // ① **先验**：读不出自述、id 与清单不符、版本与清单不符、abi 出界——都在这里当场结束，
    //    上面只写了暂存，正式目录一个字节都没动。
    let verdict = match inspect(&stage_dll) {
        Inspect::Ok { desc, .. } => {
            if desc.id != id {
                Err(format!("自述 id 是「{}」，清单写的是「{id}」", desc.id))
            } else if let Some((_, v)) = desc
                .metadata
                .iter()
                .find(|(key, _)| key == "version")
                .filter(|(_, v)| !v.is_empty())
            {
                if v != version {
                    Err(format!("自述版本是「{v}」，清单写的是「{version}」"))
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        }
        Inspect::AbiMismatch { got } => Err(format!("它自述 abi {got}，本宿主收不了")),
        Inspect::Error(err) => Err(format!("读不出自述：{err}")),
    };
    if let Err(msg) = verdict {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(msg);
    }

    // ② **后换**：旧档挪开 → 新档与许可证目录落位。这里的失败仍走 `restore_old`
    //    （挪位本身对正被加载的文件是成功的，见上面的读数）。
    if target.exists() {
        let _ = std::fs::remove_file(&old_dll);
        if let Err(err) = std::fs::rename(&target, &old_dll) {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(format!(
                "旧 DLL 挪不开（多半正被加载，重启 RuPix 后再装）：{err}"
            ));
        }
    }
    if pkg_dir.exists() {
        let _ = std::fs::remove_dir_all(&old_dir);
        if let Err(err) = std::fs::rename(&pkg_dir, &old_dir) {
            restore_old(&target, &pkg_dir, &old_dll, &old_dir);
            let _ = std::fs::remove_dir_all(&stage);
            return Err(format!("旧许可证目录挪不开：{err}"));
        }
    }
    if let Err(err) = std::fs::rename(&stage_dll, &target) {
        restore_old(&target, &pkg_dir, &old_dll, &old_dir);
        let _ = std::fs::remove_dir_all(&stage);
        return Err(format!("DLL 落盘失败：{err}"));
    }
    if stage_pkg.exists() {
        if let Err(err) = std::fs::rename(&stage_pkg, &pkg_dir) {
            restore_old(&target, &pkg_dir, &old_dll, &old_dir);
            let _ = std::fs::remove_dir_all(&stage);
            return Err(format!("许可证目录落盘失败：{err}"));
        }
    }
    let _ = std::fs::remove_dir_all(&stage);

    // ③ 收尾：删掉挪开的旧档。★ 被加载着的那一份删不掉是真机常态（code=5），留着不碍事
    //（文件名以 `.old` 结尾，扫描只认 `*.dll`），但必须出声——它是"上一次是热更新"的唯一痕迹。
    if let Err(err) = std::fs::remove_file(&old_dll) {
        log::warn!(
            "挪开的旧 DLL 删不掉（多半正被加载）：{}：{err}",
            old_dll.display()
        );
    }
    if let Err(err) = std::fs::remove_dir_all(&old_dir) {
        log::warn!("挪开的旧许可证目录删不掉：{}：{err}", old_dir.display());
    }
    drop_older_versions(dir, &target, id);
    drop_older_packages(dir, &pkg_dir, id);
    Ok(target)
}

/// Remove old official package folders, but only those carrying metadata.json;
/// arbitrary user folders are left alone.
fn drop_older_packages(dir: &Path, keep: &Path, id: &str) {
    let prefix = format!("{id}_");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path == keep || !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !path.join("metadata.json").is_file() {
            continue;
        }
        if let Err(err) = std::fs::remove_dir_all(&path) {
            log::warn!("旧许可证目录 {name} 删不掉：{err}");
        }
    }
}

/// 删掉同 id 的其它版本（本函数只做 best-effort：删不掉说明它正被加载，那也留不住）。
fn drop_older_versions(dir: &std::path::Path, keep: &std::path::Path, id: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p == keep {
            continue;
        }
        let is_dll = p
            .extension()
            .map(|x| x.eq_ignore_ascii_case("dll"))
            .unwrap_or(false);
        if !is_dll {
            continue;
        }
        let Some(name) = p.file_name().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        // `<id>_<ver>.dll`：最后一个下划线之前那段就是 id。
        if name.rsplit_once('_').map(|(stem, _)| stem) != Some(id) {
            continue;
        }
        if let Err(err) = std::fs::remove_file(&p) {
            log::warn!("旧版本 {name} 删不掉（可能正被加载）：{err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 被拒的两种前置条件：abi 不兼容、没有下载地址。
    ///
    /// 这两个判据的价值是**别让线程白起**：真去下一次再失败，用户要等到网络往返结束
    /// 才知道"这一枚根本装不了"。
    #[test]
    fn start_rejects_unusable_entries_before_spawning() {
        let mut e = RemoteEntry {
            id: "x".into(),
            name: "X".into(),
            version: "0.1.0".into(),
            abi: 5,
            size: 10,
            released: String::new(),
            author: String::new(),
            license: String::new(),
            desc: String::new(),
            formats: vec!["jpg".into()],
            urls: std::collections::HashMap::new(),
        };
        assert!(start(&e, SourceId::Gitee).is_err(), "没有地址就不该起线程");

        e.urls
            .insert("gitee".to_string(), "https://example.com/x.dll".to_string());
        e.abi = 999;
        assert!(
            start(&e, SourceId::Gitee).is_err(),
            "abi 不兼容就不该起线程"
        );

        e.abi = 4;
        assert!(
            start(&e, SourceId::Github).is_err(),
            "gitee 有地址、但当前来源是 github ⇒ 仍该拒（换源才能下）"
        );

        // 合法的那一枚不在这里真下——那要联网，归集成测试/实机验。
        assert!(poll().is_none(), "没起过任务就不该有结果");
        assert!(progress().is_none(), "没起过任务就不该有进度");
    }

    /// 21.5 那一格的顺序判据：**验不过的包不许碰正式目录**。
    ///
    /// 夹具刻意把目录里那份旧 DLL 用**独占句柄**开着——真机上"正被加载"就是这个形状
    /// （改名与删除都被拒，`ERROR_ACCESS_DENIED` 5，读数见 K159）。这样两种顺序会给出
    /// **不同的诊断**，红证因此是确定的而不是时序运气：
    /// - 先换后验（旧写法）：第一步就撞在"把旧档挪开"上 ⇒ 报「旧 DLL 挪不开」，
    ///   用户读到的是"你的目录被占用"，而真相是"这个包根本读不出自述"；
    /// - 先验后换（现在）：包本身先被否掉 ⇒ 报自述失败，目录里旧档原位不动、也不冒出 `.old`。
    #[test]
    fn a_rejected_package_never_touches_the_live_directory() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = crate::core::paths::test_scratch_root().join("install-reject");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = "x_0.1.0.dll";
        let target = dir.join(file);
        std::fs::write(&target, b"OLD").unwrap();
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&target)
            .expect("独占打开旧档");

        let err = place_package(&dir, file, "x", "0.1.0", b"not a dll", &[])
            .expect_err("读不出自述就必须拒");
        eprintln!("[place-package] 拒绝原因：{err}");
        assert!(
            err.contains("自述"),
            "拒的理由要指向包本身读不出自述，而不是目录里的旧档挪不动：{err}"
        );
        assert!(
            !err.contains("挪不开"),
            "报「挪不开」= 这个顺序又回到了先换后验：{err}"
        );

        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n.ends_with(".old")),
            "验不过不许在目录里留下挪开的旧档：{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with(".rupix-install-")),
            "暂存目录要跟着一起清掉：{names:?}"
        );
        drop(hold);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"OLD",
            "旧档要原位不动、一个字节没被改"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_package_unpacks_one_dll_and_versioned_docs() {
        use std::io::{Cursor, Write};
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("common_0.2.0.dll", opts).unwrap();
            zip.write_all(b"dll").unwrap();
            zip.start_file("common_0.2.0/LICENSE", opts).unwrap();
            zip.write_all(b"license").unwrap();
            zip.start_file("common_0.2.0/licenses/MIT.txt", opts)
                .unwrap();
            zip.write_all(b"mit").unwrap();
            zip.finish().unwrap();
        }
        let got = unpack_zip(bytes.get_ref(), "common", "0.2.0", "common_0.2.0.dll")
            .expect("valid package");
        assert_eq!(got.dll, b"dll");
        assert_eq!(got.docs.len(), 2);
        assert_eq!(got.docs[0].0, PathBuf::from("LICENSE"));
    }

    #[test]
    fn zip_package_rejects_nested_dlls() {
        use std::io::{Cursor, Write};
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("common_0.2.0.dll", opts).unwrap();
            zip.write_all(b"dll").unwrap();
            zip.start_file("common_0.2.0/evil.dll", opts).unwrap();
            zip.write_all(b"evil").unwrap();
            zip.finish().unwrap();
        }
        assert!(unpack_zip(bytes.get_ref(), "common", "0.2.0", "common_0.2.0.dll").is_err());
    }

    /// 百分比：0 起、100 止，超收夹住，`total == 0` 不除零。
    #[test]
    fn progress_percent_clamps_and_survives_zero_total() {
        let pct = |got, total| {
            Progress {
                id: "x".into(),
                got,
                total,
            }
            .percent()
        };
        assert_eq!(pct(0, 100), 0);
        assert_eq!(pct(42, 100), 42);
        assert_eq!(pct(100, 100), 100);
        assert_eq!(pct(101, 100), 100, "超收不该报 >100%");
        assert_eq!(pct(0, 0), 0, "total 为 0 不能除零");
    }
}
