//! 解码器加载面包屑：`<dll>.loading`。
//!
//! # 为什么要有它
//!
//! 解码器与宿主同进程，坏 `DllMain` / 静态初始化能把进程直接带走。`invoke` 的
//! SEH 闸门能接住访问违规，但**进程级崩溃**接不住；更糟的是坏 DLL 每次启动都会
//! 在同一个位置再崩一次，形成启动崩溃循环。这里在 `LoadLibrary` **之前**写一枚
//! 标记，正常返回（成功 / 普通错误 / Rust panic）由 [`LoadingGuard`] 的 `Drop`
//! 清除；进程崩溃或 SEH 硬故障跳过析构，标记留在盘上。下次 `inspect` 看到它就
//! 跳过该 DLL，用户可在设置 → 解码器详情里清除标记并重试。
//!
//! # 为什么写在 DLL 旁边
//!
//! 便携程序只认 exe 同级目录；标记与 DLL 同名（`<dll>.loading`）最直观，删 DLL
//! 时残留标记也不会被当成解码器扫描（目录扫描只认 `*.dll`）。写失败（只读介质 /
//! 杀软）只记一条 warn，加载照常进行——不能因为面包屑写不下去就不让用户看图。

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// 标记后缀。`common_0.2.6.dll` → `common_0.2.6.dll.loading`。
const SUFFIX: &str = ".loading";

/// 某个 DLL 的标记路径。
pub(super) fn path_for(dll: &Path) -> PathBuf {
    let mut s = dll.as_os_str().to_os_string();
    s.push(SUFFIX);
    PathBuf::from(s)
}

/// 盘上是否有一枚上次留下的标记。
pub(super) fn stale(dll: &Path) -> bool {
    path_for(dll).is_file()
}

/// 清除标记（用户点「清除标记并重试」）。标记本来就不在也算成功。
pub(super) fn clear(dll: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path_for(dll)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// 一次加载的标记守卫：正常离开作用域即删标记；进程崩溃 / SEH 硬故障跳过析构，
/// 标记留下。
pub(super) struct LoadingGuard {
    path: PathBuf,
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("解码器加载标记清除失败 {}：{e}", self.path.display());
            }
        }
    }
}

/// 在 `LoadLibrary` 之前写标记。写失败返回 `None`（加载照常，只是没有面包屑）。
pub(super) fn begin(dll: &Path) -> Option<LoadingGuard> {
    let path = path_for(dll);
    let body = format!(
        "pid={}\nstarted={:?}\n",
        std::process::id(),
        std::time::SystemTime::now()
    );
    let staged = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&path)?;
        f.write_all(body.as_bytes())?;
        // 不 sync 的话，进程在 `LoadLibrary` 里崩掉时这枚文件可能还留在页缓存里。
        f.sync_all()?;
        Ok(())
    })();
    match staged {
        Ok(()) => Some(LoadingGuard { path }),
        Err(e) => {
            log::warn!(
                "解码器加载标记写入失败 {}：{e}（本次无面包屑保护）",
                path.display()
            );
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("decoder-marker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dll = dir.join("common_9.9.9.dll");
        (dir, dll)
    }

    #[test]
    fn guard_writes_marker_and_clears_it_on_normal_drop() {
        let (dir, dll) = scratch("guard");
        {
            let guard = begin(&dll).expect("临时目录可写，标记必须建得出来");
            assert!(stale(&dll), "加载期间标记必须在盘上");
            drop(guard);
        }
        assert!(!stale(&dll), "正常返回后标记必须清掉");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn leftover_marker_is_detected_and_can_be_cleared() {
        let (dir, dll) = scratch("leftover");
        // 模拟上次崩溃留下的标记：不走 guard，直接落盘。
        std::fs::write(path_for(&dll), b"pid=1\n").unwrap();
        assert!(stale(&dll), "残留标记必须能看见");
        clear(&dll).unwrap();
        assert!(!stale(&dll), "清除后必须放行重试");
        // 再清一次也成功（幂等）。
        clear(&dll).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn marker_path_keeps_the_dll_extension() {
        let p = path_for(Path::new(r"C:\x\common.dll"));
        assert_eq!(p.file_name().unwrap(), "common.dll.loading");
    }
}
