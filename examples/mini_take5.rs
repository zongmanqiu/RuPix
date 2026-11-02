//! `mini_take5` —— 对**稳定的 explorer 窗口**做取件双路取证（不依赖前台）。
//!
//! mini_take3/4 的教训：`explorer /select` 的新窗口会丢前台（8s 后前台变回桌面），
//! "对着前台"的探针测到的是假现场。本例改为 EnumWindows 找可见的 CabinetWClass
//! 窗口，对它做 IShellWindows 三重匹配 → QueryActiveShellView →
//! GetItemObject(CF_HDROP) / IFolderView::Items 双路逐环打印。
//! 跑法：先 `explorer /select,<probe_samples 内文件>` 等 6s，再运行本例。
fn main() {
    rupix_lib::platform::logging::install_logging();
    diag_stable_window();
}

fn diag_stable_window() {
    use windows::core::{w, Interface, BOOL};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IDataObject, IServiceProvider, DVASPECT_CONTENT,
        FORMATETC, TYMED_HGLOBAL,
    };
    use windows::Win32::System::Ole::{ReleaseStgMedium, CF_HDROP};
    use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
    use windows::Win32::UI::Shell::{
        DragQueryFileW, IEnumIDList, IFolderView, IShellBrowser, IShellWindows,
        SHGetPathFromIDListW, SID_STopLevelBrowser, HDROP, SVGIO_SELECTION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, GetClassNameW, IsWindowVisible,
    };

    unsafe {
        let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);

        // ── 找一个可见的 CabinetWClass 窗口 ──
        struct Found(HWND);
        unsafe extern "system" fn enum_proc(h: HWND, l: LPARAM) -> BOOL {
            let slot = l.0 as *mut Found;
            let mut buf = [0u16; 64];
            let n = GetClassNameW(h, &mut buf);
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            if class == "CabinetWClass" && IsWindowVisible(h).as_bool() {
                unsafe { *slot = Found(h) };
                return BOOL(0); // 停枚举
            }
            BOOL(1)
        }
        let mut found = Found(HWND::default());
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut found as *mut Found as isize));
        if found.0.is_invalid() {
            println!("diag: 找不到可见的 CabinetWClass 窗口——先开一个资源管理器窗口");
            return;
        }
        let target = found.0;
        let mut buf = [0u16; 64];
        let n = GetClassNameW(target, &mut buf);
        println!(
            "diag target = {:#x} class={:?}",
            target.0 as isize,
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        );
        let fg_tab = FindWindowExW(Some(target), None, w!("ShellTabWindowClass"), None)
            .map(|h| h.0 as isize)
            .unwrap_or(0);
        println!("diag target_tab(ShellTabWindowClass) = {fg_tab:#x}");

        // ── IShellWindows 三重匹配 ──
        let clsid = windows::core::GUID::from_u128(0x9BA05972_F6A8_11CF_A442_00A0C90A8F39);
        let Ok(shell): windows::core::Result<IShellWindows> = CoCreateInstance(
            &clsid,
            None,
            windows::Win32::System::Com::CLSCTX_LOCAL_SERVER,
        ) else {
            println!("diag CoCreate FAILED");
            return;
        };
        let Ok(count) = shell.Count() else {
            println!("diag Count FAILED");
            return;
        };
        println!("diag Count={count}");
        for i in 0..count {
            let mut vi = VARIANT::default();
            vi.Anonymous = VARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_I4,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 { lVal: i },
                }),
            };
            let Ok(disp) = shell.Item(&vi) else { continue };
            // 与 shell_selection 同口径：IWebBrowser2 拿得到就用，拿不到留 0 走 GetWindow 补判。
            let mut h = 0isize;
            if let Ok(browser) = disp.cast::<windows::Win32::UI::Shell::IWebBrowser2>() {
                if let Ok(handle) = browser.HWND() {
                    h = handle.0;
                }
            }
            let Ok(sp) = disp.cast::<IServiceProvider>() else {
                continue;
            };
            let Ok(brows) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else {
                println!("diag item {i}: QueryService FAILED (h={h:#x})");
                continue;
            };
            let bh = if h == 0 {
                brows.GetWindow().map(|x| x.0 as isize).unwrap_or(0)
            } else {
                h
            };
            let matched = bh == target.0 as isize || (fg_tab != 0 && bh == fg_tab);
            println!(
                "diag item {i}: h={h:#x} bh={bh:#x} (target={:#x} tab={fg_tab:#x}) matched={matched}",
                target.0 as isize
            );
            if !matched {
                continue;
            }
            // ── 双路取件 ──
            let Ok(view) = brows.QueryActiveShellView() else {
                println!("  QueryActiveShellView FAILED");
                continue;
            };
            match view.GetItemObject::<IDataObject>(SVGIO_SELECTION) {
                Ok(dao) => {
                    let fe = FORMATETC {
                        cfFormat: CF_HDROP.0,
                        ptd: std::ptr::null_mut(),
                        dwAspect: DVASPECT_CONTENT.0,
                        lindex: -1,
                        tymed: TYMED_HGLOBAL.0 as u32,
                    };
                    match dao.GetData(&fe) {
                        Ok(mut med) => {
                            // 整个 diag_stable_window 已在 unsafe 块内，union 读与
                            // FFI 调用无需再嵌套 unsafe。
                            let hdrop = HDROP(med.u.hGlobal.0);
                            let n = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
                            let mut pb = [0u16; 260];
                            let got = if n >= 1 {
                                DragQueryFileW(hdrop, 0, Some(&mut pb))
                            } else {
                                0
                            };
                            ReleaseStgMedium(&mut med);
                            if got > 0 {
                                let end = pb.iter().position(|&c| c == 0).unwrap_or(pb.len());
                                println!(
                                    "  [主路] GetItemObject+CF_HDROP ok，共 {n} 项，首项={:?}",
                                    String::from_utf16_lossy(&pb[..end])
                                );
                            } else {
                                println!(
                                    "  [主路] GetItemObject ok 但 HDROP 空（n={n}）= 选择集为空"
                                );
                            }
                        }
                        Err(e) => println!("  [主路] GetData(CF_HDROP) FAILED: {e}"),
                    }
                }
                Err(e) => println!("  [主路] GetItemObject FAILED: {e}"),
            }
            match view.cast::<IFolderView>() {
                Ok(fv) => match fv.Items::<IEnumIDList>(SVGIO_SELECTION) {
                    Ok(items) => {
                        let mut slot: [*mut windows::Win32::UI::Shell::Common::ITEMIDLIST; 1] =
                            [std::ptr::null_mut()];
                        let mut got_n = 0u32;
                        let hr = items.Next(&mut slot, Some(&mut got_n));
                        if hr.is_ok() && got_n == 1 {
                            let mut pb = [0u16; 260];
                            let ok = SHGetPathFromIDListW(slot[0], &mut pb).as_bool();
                            let end = pb.iter().position(|&c| c == 0).unwrap_or(pb.len());
                            println!(
                                "  [兜底] Items ok got=1 SHGetPath={ok} path={:?}",
                                String::from_utf16_lossy(&pb[..end])
                            );
                        } else {
                            println!("  [兜底] Items hr_ok={} got={got_n}", hr.is_ok());
                        }
                    }
                    Err(e) => println!("  [兜底] Items FAILED: {e}"),
                },
                Err(e) => println!("  [兜底] cast IFolderView FAILED: {e}"),
            }
        }
    }
}
