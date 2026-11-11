//! 把一小段东西交给**系统默认程序**打开（URL），或替调用方把图片字节落进 temp。
//!
//! ★ 边界都在调用方：URL 只认 `http(s)`（`is_openable_url`，走 `open_target` 交系统
//!   默认浏览器）；图片只认宿主自己按文件头写进 `<exe>/temp/` 的那一枚
//!   （`save_temp_image` 会**当场复核 magic**，插件给什么扩展名都不作数）。
//!   ★ 2026-11-04 起图片**不再**交系统关联打开：本机把 `.png` 关联到 RuPix 自己时，
//!   二次启动被单实例收走、没开多开就"当前窗换图"，用户正在看的图被插件产物顶掉。
//!   现在落盘后由调用方（runtime）开**新看图窗口**显示，与多开开关无关。
//!
//! ★ 为什么落盘由宿主做而不是插件自己写文件：写盘是**用户资产**副作用，属于"宿主
//!   独占"（见 `docs/扩展体系.md` §四）。宿主能保证：只碰自己的 temp、按文件头复核、
//!   有事可记日志。
//!
//! 本模块还管**系统右键菜单**（[`FileMenu`]，m313）：右键「系统菜单」把当前这张图
//! 交还给 explorer，由它弹**这个文件**那一份菜单——打开方式 / 发送到 / 压缩 / 杀毒
//! 扫描 / 属性这些动词全在系统与第三方外壳扩展里注册着，宿主要么复刻一套（违背
//! "只做看图"），要么干脆不做（用户的"针对这个文件"就永远差一块）。第三条路是
//! **转发**：菜单是系统的，宿主只负责把它弹在对的位置、并把选中的动词交回去。

use std::path::PathBuf;

use windows::core::{Interface, HSTRING, PCSTR, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder,
    RemoveWindowSubclass, SHBindToParent, SHParseDisplayName, SetWindowSubclass, ShellExecuteW,
    CMF_EXPLORE, CMF_NORMAL, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, GetMenuItemCount, PostMessageW,
    SetForegroundWindow, TrackPopupMenuEx, HMENU, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL,
};

/// 这个 URL 能不能交给系统打开：只认 http/https，不许任何控制字符或引号。
pub fn is_openable_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    !url.chars().any(|c| c.is_control() || c == '"')
}

/// 让系统用默认关联打开 `target`。
pub fn open_target(target: &str) -> Result<(), String> {
    let verb = HSTRING::from("open");
    let file = HSTRING::from(target);
    // SAFETY: 两个 HSTRING 在调用期间存活；父窗口传 None（不模态、不依赖某个窗口）。
    let rc = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 的返回值 > 32 才算成功（Win32 的老约定）。
    if rc.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "系统没能打开（ShellExecute 返回 {}）",
            rc.0 as isize
        ))
    }
}

/// 按文件头认出光栅图；认不出返回 `None`（那就**不打开**，只记日志）。
pub fn image_ext_of(bytes: &[u8]) -> Option<&'static str> {
    let has =
        |i: usize, sig: &[u8]| bytes.len() >= i + sig.len() && &bytes[i..i + sig.len()] == sig;
    if has(0, b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if has(0, &[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if has(0, b"GIF87a") || has(0, b"GIF89a") {
        return Some("gif");
    }
    if has(0, b"BM") {
        return Some("bmp");
    }
    if has(0, b"RIFF") && has(8, b"WEBP") {
        return Some("webp");
    }
    if has(0, b"II*\0") || has(0, b"MM\0*") {
        return Some("tiff");
    }
    if has(0, &[0, 0, 1, 0]) {
        return Some("ico");
    }
    None
}

/// 临时图片的落点：`<exe>/temp/barcode-<毫秒>.<ext>`。
///
/// ★ **不许借 `crash_log_path` 拼这个名字**：那个函数会加 `crash-` 前缀与 `.log`
///   后缀，系统按 `.log` 的文本关联打开，用户看到的会是一个文本编辑器里的二进制乱码
///   （K195）。图片必须以**真实图片后缀**落地，后续无论谁打开它都走图片路径。
///   打开方式（宿主开新窗）由调用方决定，这里只负责把字节安全落盘。
fn temp_image_path(stamp: u128, ext: &str) -> Option<PathBuf> {
    Some(crate::core::paths::diagnostics_dir()?.join(format!("barcode-{stamp}.{ext}")))
}

/// 把一段图片字节写进 `<exe>/temp/barcode-<毫秒>.<ext>`，返回落点。
///
/// **扩展名以文件头为准**：插件给的写法只当提示，magic 认不出就直接拒——
/// 让系统按错误后缀去关联程序，等于替用户点了一个不该点的东西。
pub fn save_temp_image(claimed_ext: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err("图片载荷是空的".into());
    }
    let Some(ext) = image_ext_of(bytes) else {
        return Err(format!(
            "载荷不是可识别的光栅图（插件说它是 {claimed_ext}）"
        ));
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = temp_image_path(stamp, ext).ok_or("取不到 exe 同级 temp 目录")?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, bytes).map_err(|e| format!("写临时图片失败：{e}"))?;
    Ok(path)
}

// ── 系统右键菜单（m313）────────────────────────────────────────────────────
//
// 链路照 MSDN 的标准三段走：**路径 → 绝对 PIDL → `IShellFolder` →
// `IContextMenu`**，然后 `QueryContextMenu` 装项、`TrackPopupMenuEx` 弹出、
// `InvokeCommand` 执行选中项。动词本身一行都不由宿主实现——这正是这条路的唯一
// 价值：装了 7-Zip 就有「添加到压缩包」，装了杀软就有「扫描」，改成默认看图器之后
// 系统该给的「打开方式」也在这里。
//
// ★ 为什么不自己拼一份菜单：菜单的内容随机器而变（装了谁、注册了什么动词），
//   复刻出来的一定是"我这台机器上长这样"。宿主只做转发，才谈得上"跟系统一致"。
//
// ★ 为什么转发三条消息（`WM_INITMENUPOPUP` / `WM_DRAWITEM` / `WM_MEASUREITEM`）：
//   外壳菜单里的项**可以**是 owner-draw——系统只画一个空白格，让菜单属主调
//   `IContextMenu2::HandleMenuMsg` 自己画。契约要求属主接这三条并转给外壳，所以接。
//   `WM_MENUCHAR`（键盘首字母定位）归 `IContextMenu3::HandleMenuMsg2`，拿不到 3 就退回 2。
//
//   ★ 实测（Win11 26200 + 7-Zip，`build/shell_menu_probe.ps1`）：这一份菜单里
//   **没有 owner-draw 项**（`WM_DRAWITEM`/`WM_MEASUREITEM` 一条都没来），图标是
//   外壳自己下的位图、子菜单项在 `QueryContextMenu` 那一趟就装好了，所以
//   `HandleMenuMsg(WM_INITMENUPOPUP)` 回 `E_FAIL`（2 / 3 都回）也不影响观感——菜单
//   18 项齐全、图标文字都在、7-Zip 子菜单 7 项、点「复制文件地址」后剪贴板等于原路径。
//   但**不能因为这个就删掉转发**：真 ownerdraw 的扩展（版本控制外壳扩展一类）只有
//   这条路能画出字来，而"本机没有这种扩展"不是证据。
//
// ★ 子类化的是**调用方的窗口**（windui 主窗），不是另建一枚隐藏窗：菜单属主必须
//   是"前台那个窗"，否则点屏幕别处它不收（`TrackPopupMenu` 的前台归属是 Win32 的
//   老规矩，见 `SetForegroundWindow` 那一行）。

/// 宿主给菜单命令留的 id 区间下界。`1` 起是约定（`0` 留给"没选中"）。
const MENU_CMD_FIRST: u32 = 1;
/// id 区间上界：`0x7FFF` 是资源脚本给自定义命令留的分界，再往上会与系统 id 撞。
/// `TrackPopupMenuEx` 在 `TPM_RETURNCMD` 下原样把 id 报回来，所以上界也是"这一定是
/// 我们的项"的判据。
const MENU_CMD_LAST: u32 = 0x7FFF;
/// 子类 id（同一扇窗只装一枚，值随便取但要避开别的子类）。
const SUBCLASS_ID: usize = 0x5250;

/// 一个**已经装好项的**「这个文件的系统右键菜单」。
///
/// 分成 [`FileMenu::for_path`] 与 [`FileMenu::show`] 两段是刻意的：前半段只读盘与
/// 查注册表（快，失败能当场报给用户），后半段会**跑一圈模态消息循环**（`show`）。
/// 调用方必须在"事件分发已经完全返回"之后才调 `show`——见 `ui::canvas` 里
/// `defer_blocking` 那段注释。
pub struct FileMenu {
    cm: IContextMenu,
    cm2: Option<IContextMenu2>,
    cm3: Option<IContextMenu3>,
    hmenu: OwnedMenu,
    /// 绝对 PIDL：外壳在上面挂了引用，`InvokeCommand` 之前必须一直有效。
    _pidl: OwnedPidl,
    /// `IShellFolder` 与 `_pidl` 都只是"让 `cm` 活着"的锚点（外壳的引用计数自己
    /// 也保得住，但多留一手不比少留便宜）。
    _folder: IShellFolder,
    /// 弹出位置（屏幕像素）。`ptInvoke` 也用它——有些动词按点击点定位。
    at: POINT,
    /// **最后**一个字段：COM 的收回排在全部接口释放之后。
    _com: crate::platform::recycle::ComGuard,
}

/// 绝对 PIDL 的归属守卫（`SHParseDisplayName` 交给我们，由我们 `CoTaskMemFree`）。
struct OwnedPidl(*mut ITEMIDLIST);

impl Drop for OwnedPidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 指针来自 `SHParseDisplayName`，只在这里释放一次。
            unsafe { CoTaskMemFree(Some(self.0 as *const std::ffi::c_void)) };
        }
    }
}

/// 弹出菜单句柄的归属守卫（`CreatePopupMenu` 建、`DestroyMenu` 收）。
struct OwnedMenu(HMENU);

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        // SAFETY: 句柄由 `CreatePopupMenu` 建，只在这里销毁一次。
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

/// 子类过程能看到的那一份：两个接口的裸指针（空 = 这一档不可用）。
///
/// 为什么不直接存 `&IContextMenu2`：它要进 thread-local，而 `'static` 的生命周期
/// 是假的——真正的存活期是 [`FileMenu::show`] 的栈帧，裸指针 + "只在 show 期间
/// 可见"这条纪律比伪造生命周期诚实。
#[derive(Clone, Copy)]
struct Hook {
    cm2: *const IContextMenu2,
    cm3: *const IContextMenu3,
}

thread_local! {
    /// 当前正在显示的系统菜单（空 = 没在显示）。只有 UI 线程用到：弹菜单的是它，
    /// 子类过程也只在那条线程的消息循环里跑。
    static HOOK: std::cell::Cell<*const Hook> = const { std::cell::Cell::new(std::ptr::null()) };
}

/// 转发 `WM_DRAWITEM` / `WM_MEASUREITEM` / `WM_INITMENUPOPUP` / `WM_MENUCHAR`。
///
/// 其余消息一律 `DefSubclassProc` 交回 windui 自己的窗口过程——**这条子类只活到
/// `show` 返回**，装与拆都在同一个函数里。
unsafe extern "system" fn menu_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let hook = HOOK.with(|c| c.get());
    if !hook.is_null() {
        // SAFETY: 指针由 `show` 在 `TrackPopupMenuEx` 之前写下、返回之后清空，
        // 期间栈帧一直活着（`Hook` 是 `Copy`，这里只读）。
        let hook = unsafe { *hook };
        if msg == WM_MENUCHAR {
            if !hook.cm3.is_null() {
                // SAFETY: 同上，`cm3` 指向 `FileMenu` 里活着的那个接口。
                let cm3 = unsafe { &*hook.cm3 };
                let mut out = LRESULT(0);
                if unsafe { cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut out)) }.is_ok() && out.0 != 0
                {
                    return out;
                }
            }
        } else if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM) {
            // 优先 **2**：`HandleMenuMsg` 本来就是 2 的方法（3 只在 `WM_MENUCHAR`
            // 上用它的 `HandleMenuMsg2`，那里要出参）。
            // SAFETY: 两个指针都指向 `FileMenu` 里活着的接口。
            let res = unsafe {
                if !hook.cm2.is_null() {
                    (&*hook.cm2).HandleMenuMsg(msg, wp, lp)
                } else if !hook.cm3.is_null() {
                    (&*hook.cm3).HandleMenuMsg(msg, wp, lp)
                } else {
                    Ok(())
                }
            };
            // debug 档：这三条就是"外壳画菜单"的全部证据（`WM_DRAWITEM` = 外壳真的
            // 落了笔）。弹一次菜单会来十几到几十条，所以不能进 info。
            log::debug!(
                "系统菜单：转发消息 {msg} → {}",
                match &res {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("0x{:08X}", e.code().0),
                }
            );
        }
    }
    // SAFETY: 参数原样交回链上原来的窗口过程。
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

impl FileMenu {
    /// 为 `path` 装一份系统右键菜单（不弹）。`hwnd` 是调用方的窗口句柄。
    ///
    /// 失败一定说得清原因：路径系统认不出 / 所在的文件夹打不开 / 这个文件没有注册
    /// 任何动词。给用户的提示直接用这些句子。
    pub fn for_path(hwnd: isize, path: &std::path::Path) -> Result<Self, String> {
        let com = crate::platform::recycle::ComGuard::new();
        let window = HWND(hwnd as *mut std::ffi::c_void);
        if window.is_invalid() {
            return Err("取不到窗口句柄".into());
        }
        // 路径按 UTF-16 直接传（`to_string_lossy` 会把非 Unicode 文件名换成 `?`，
        // 那样弹出来的是**别的文件**的菜单）。`encode_wide` 不补结尾零，手动补。
        let mut wide: Vec<u16> = {
            use std::os::windows::ffi::OsStrExt;
            path.as_os_str().encode_wide().collect()
        };
        wide.push(0);

        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        // SAFETY: `wide` 以 NUL 结尾且在调用期间存活；`pbc` 传空（不需要绑定上下文）。
        unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>, &mut pidl, 0, None) }
            .map_err(|e| format!("系统认不出这个路径（{e}）"))?;
        if pidl.is_null() {
            return Err("系统认不出这个路径".into());
        }
        let pidl = OwnedPidl(pidl);

        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        // SAFETY: `pidl` 有效；`child` 指向 `pidl` 内部，不单独释放。
        let folder: IShellFolder = unsafe { SHBindToParent(pidl.0, Some(&mut child)) }
            .map_err(|e| format!("打不开这个文件所在的文件夹（{e}）"))?;

        // SAFETY: 同一枚子 PIDL 只读传给外壳，取回它的上下文菜单对象。
        let cm: IContextMenu = unsafe { folder.GetUIObjectOf(window, &[child], None) }
            .map_err(|e| format!("系统没有为这个文件注册菜单（{e}）"))?;

        // SAFETY: 无参建一枚空弹出菜单，归属交给守卫。
        let hmenu =
            OwnedMenu(unsafe { CreatePopupMenu() }.map_err(|e| format!("建不出菜单（{e}）"))?);
        // `CMF_EXPLORE`：按"在资源管理器里右键"的语境给出动词（少了它会缺一批
        // 只在浏览场景出现的项）。`CMF_NORMAL` = 不是 Shift+右键的扩展集。
        let hr = unsafe {
            cm.QueryContextMenu(
                hmenu.0,
                0,
                MENU_CMD_FIRST,
                MENU_CMD_LAST,
                CMF_NORMAL | CMF_EXPLORE,
            )
        };
        if hr.is_err() {
            return Err(format!("系统没能列出这个文件的菜单项（0x{:08X}）", hr.0));
        }
        // 数项而不是看 `QueryContextMenu` 的返回值：契约说返回"用掉的 id 数"，可
        // 实测有扩展返回 S_OK(0) 却装满了项，按返回值判会把好菜单当空菜单拒掉。
        // SAFETY: 句柄有效。
        if unsafe { GetMenuItemCount(Some(hmenu.0)) } <= 0 {
            return Err("这个文件在系统里没有可用的菜单项".into());
        }

        let mut at = POINT::default();
        // SAFETY: 出参指向本栈帧的 `POINT`。
        unsafe { GetCursorPos(&mut at) }.map_err(|e| format!("取不到鼠标位置（{e}）"))?;

        Ok(Self {
            cm2: cm.cast().ok(),
            cm3: cm.cast().ok(),
            cm,
            hmenu,
            _pidl: pidl,
            _folder: folder,
            at,
            _com: com,
        })
    }

    /// 装了几项。给判据用（真的弹一次是模态的，单测里弹不起来）。
    pub fn items(&self) -> i32 {
        // SAFETY: 句柄由 `self` 持有。
        unsafe { GetMenuItemCount(Some(self.hmenu.0)) }
    }

    /// 在鼠标位置弹出菜单，用户选中的那一项**当场执行**（外壳自己跑动词）。
    ///
    /// 返回 `Ok` 里没有任何"用户选了什么"——动词的产物在 explorer 一侧，宿主不需要
    /// 也不该知道。用户按 Esc / 点别处（`TrackPopupMenu` 回 0）同样是 `Ok`。
    pub fn show(self, hwnd: isize) -> Result<(), String> {
        let window = HWND(hwnd as *mut std::ffi::c_void);
        if window.is_invalid() {
            return Err("取不到窗口句柄".into());
        }
        let hook = Hook {
            cm2: self
                .cm2
                .as_ref()
                .map_or(std::ptr::null(), |c| c as *const _),
            cm3: self
                .cm3
                .as_ref()
                .map_or(std::ptr::null(), |c| c as *const _),
        };

        // 前台归属：不先把窗设成前台，点屏幕别处菜单不收（Win32 老规矩）。
        // 用户刚在这个窗里点完右键，正常情况这里是无操作。
        // SAFETY: 窗口句柄有效。
        unsafe {
            let _ = SetForegroundWindow(window);
        }
        // SAFETY: 子类过程是 `extern "system"` 且只在这条线程上跑。
        let subclassed =
            unsafe { SetWindowSubclass(window, Some(menu_subclass), SUBCLASS_ID, 0).as_bool() };
        HOOK.with(|c| c.set(&hook));
        // SAFETY: 菜单句柄、窗口句柄都由调用方保证存活；`TPM_RETURNCMD` 让返回值
        // 直接是命令 id 而不是 BOOL。`TPM_LEFTALIGN | TPM_TOPALIGN` 都是 0，省掉。
        let cmd = unsafe {
            TrackPopupMenuEx(
                self.hmenu.0,
                TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0,
                self.at.x,
                self.at.y,
                window,
                None,
            )
        };
        HOOK.with(|c| c.set(std::ptr::null()));
        log::debug!("系统菜单：TrackPopupMenuEx → id={}", cmd.0);
        if subclassed {
            // SAFETY: 与上面的 `SetWindowSubclass` 成对。
            unsafe {
                let _ = RemoveWindowSubclass(window, Some(menu_subclass), SUBCLASS_ID);
            }
        } else {
            // 装不上子类意味着图标与文字都会是空白格——那比"没有这一项"更让人
            // 困惑，所以宁可什么都不弹。实测只有"同一 hwnd 同一 id 重复装"会走到
            // 这里（正常路径不可能），留一条 warn 供取证。
            log::warn!("系统菜单：owner-draw 转发没装上，这次不弹（避免弹出一片空格）");
            return Ok(());
        }
        // 菜单关掉之后按 MSDN 的收尾补一条空消息，免得焦点卡在已经消失的菜单上。
        // SAFETY: 窗口句柄有效。
        unsafe {
            let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        }

        let id = cmd.0 as u32;
        if id == 0 {
            return Ok(()); // Esc / 点别处
        }
        if !(MENU_CMD_FIRST..=MENU_CMD_LAST).contains(&id) {
            return Ok(()); // 系统 id（不该出现，出现也不越俎代庖）
        }
        // `makeintresource`：id 区间从 `MENU_CMD_FIRST` 起，动词下标 = id - 下界。
        // 走 ANSI 那一格（`lpVerb`）而不是 `lpVerbW`：这里传的是整数而不是字符串，
        // 编码问题根本不存在，也就不用 `CMIC_MASK_UNICODE` 那面旗。
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            hwnd: window,
            lpVerb: PCSTR((id - MENU_CMD_FIRST) as usize as *const u8),
            nShow: SW_SHOWNORMAL.0,
            ptInvoke: self.at,
            ..Default::default()
        };
        // SAFETY: `info` 是 `CMINVOKECOMMANDINFOEX`，按外壳的约定以基类指针传入
        // （它读 `cbSize` 分辨是哪一版）。
        unsafe {
            self.cm
                .InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
        }
        .map_err(|e| format!("系统菜单执行失败（{e}）"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_are_openable() {
        assert!(is_openable_url("https://example.invalid/a"));
        assert!(is_openable_url("HTTP://example.invalid"));
        // 其余方案、相对路径、带控制字符的一律不认。
        assert!(!is_openable_url("file:///c:/windows/notepad.exe"));
        assert!(!is_openable_url("javascript:alert(1)"));
        assert!(!is_openable_url("example.invalid"));
        assert!(!is_openable_url("https://a\u{0}b"));
    }

    #[test]
    fn image_ext_follows_the_magic_not_the_claim() {
        assert_eq!(image_ext_of(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(image_ext_of(&[0xFF, 0xD8, 0xFF, 0]), Some("jpg"));
        assert_eq!(image_ext_of(b"not an image"), None);
        // 插件说 png、实际不是：拒，而不是写一个后缀骗系统。
        assert!(save_temp_image("png", b"plain text").is_err());
    }

    #[test]
    fn temp_image_lands_with_a_real_image_suffix() {
        // K195 回归钉：曾借 crash_log_path 拼出 crash-barcode-<ms>.png.log，
        // 系统按 .log 的文本关联打开，图形看不见。
        let p = temp_image_path(1_793_782_074_815, "png").expect("诊断目录路径");
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("png"),
            "落盘后缀必须是图片头认出的那一个：{}",
            p.display()
        );
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name.starts_with("barcode-"), "{name}");
        assert!(!name.contains("crash-"), "{name}");
        assert!(!name.contains(".log"), "{name}");
        // 负向：插件声明的后缀不许参与落盘名，只认 magic。
        let ext = image_ext_of(b"\x89PNG\r\n\x1a\nrest").expect("PNG 头");
        let q = temp_image_path(1, ext).expect("诊断目录路径");
        assert_eq!(q.extension().and_then(|e| e.to_str()), Some("png"));
    }

    // ── 系统菜单（m313）──
    //
    // 弹一次是真模态的（要人点），单测里做不了；能自动验的是**装菜单**这一段：
    // 路径 → PIDL → IShellFolder → IContextMenu → QueryContextMenu 全链路走通、
    // 项数不为零。链路一旦断在任何一环，`for_path` 就回 `Err`，用户看到的会是
    // "系统菜单打不开"的红字——所以这几条要把三处失败分开钉住。

    /// 桌面的 HWND：外壳 API 只拿它当"属主窗"用，弹菜单的路径不经过它。
    fn desktop_hwnd() -> isize {
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        // SAFETY: 无参、无副作用，任何会话下都有桌面窗。
        unsafe { GetDesktopWindow() }.0 as isize
    }

    /// 写一枚真实的临时文件（外壳只对**存在**的路径给菜单）。
    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("rupix-shell-menu-test");
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let f = dir.join(name);
        std::fs::write(&f, b"rupix").expect("写临时文件");
        f
    }

    #[test]
    fn shell_menu_has_items_for_a_real_file() {
        let f = temp_file("a.txt");
        let menu = FileMenu::for_path(desktop_hwnd(), &f).expect("装得上一份系统菜单");
        // 一枚普通文本文件在任何 Windows 上都至少有「打开 / 打开方式 / 属性」这一批
        // 动词；一项都没有说明 PIDL 或 QueryContextMenu 那一段断了。
        assert!(
            menu.items() > 0,
            "系统菜单一项都没有（items = {}）",
            menu.items()
        );
    }

    #[test]
    fn shell_menu_rejects_a_path_that_is_not_there() {
        let missing = std::env::temp_dir().join("rupix-shell-menu-test/no-such-file-9d1f.png");
        match FileMenu::for_path(desktop_hwnd(), &missing) {
            Ok(_) => panic!("不存在的路径不该装出菜单"),
            Err(err) => assert!(err.contains("路径"), "错误要说清是路径的问题：{err}"),
        }
    }

    #[test]
    fn shell_menu_needs_a_window() {
        let f = temp_file("b.txt");
        // 0 是"没有窗口"：外壳拿它当 owner 会把菜单挂到桌面上去，所以入口就挡掉。
        assert!(FileMenu::for_path(0, &f).is_err());
    }
}
