//! DLL 加载的平台硬化。

use std::sync::Once;

/// 一次性硬化 DLL 搜索路径（防劫持）。重复调用无害。
///
/// `LOAD_LIBRARY_SEARCH_DEFAULT_DIRS` 让后续 `LoadLibraryExW` 只在「应用目录 + System32 +
/// 用户显式 `AddDllDirectory` 的目录」里找依赖，把当前工作目录从搜索路径里摘出去——
/// 否则用户从快捷方式启动、CWD 指向别处时，一个同名 DLL 就能被顶替加载。
pub fn harden_search_path() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = unsafe {
            windows::Win32::System::LibraryLoader::SetDefaultDllDirectories(
                windows::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        };
    });
}
