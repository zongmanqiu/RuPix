//! `select_diag` —— Shell 三跳选中探测的逐跳诊断（临时排查用，同例即修复依据）。
//!
//! 对着**当前前台**的 explorer/桌面把每一步打出来：每枚 IShellWindows 条目的
//! HWND、与 frame/tab 子窗的匹配结果、IServiceProvider/QueryService/
//! QueryActiveShellView/Items 各级成败。配合 `explorer /select` 使用。
//!
//! 用法：先 `explorer /select,<某文件>`，再把本例跑起来（同一前台存续内）。

use windows::core::{w, Interface, GUID};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IServiceProvider, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
use windows::Win32::UI::Shell::{
    IEnumIDList, IFolderView, IShellWindows, IWebBrowser2, SHGetPathFromIDListW,
    SID_STopLevelBrowser, SVGIO_SELECTION,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetClassNameW, GetForegroundWindow};

const CLSID_SHELL_WINDOWS: GUID = GUID::from_u128(0x9BA05972_F6A8_11CF_A442_00A0C90A8F39);

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let fg = GetForegroundWindow();
        let mut buf = [0u16; 64];
        let n = GetClassNameW(fg, &mut buf);
        println!(
            "fg = {:p}, class = {:?}",
            fg.0,
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        );
        let fg_tab = FindWindowExW(Some(fg), None, w!("ShellTabWindowClass"), None)
            .map(|h| h.0 as isize)
            .unwrap_or(0);
        println!("fg_tab(ShellTabWindowClass) = {fg_tab:#x}");

        let shell: IShellWindows =
            match CoCreateInstance(&CLSID_SHELL_WINDOWS, None, CLSCTX_LOCAL_SERVER) {
                Ok(s) => s,
                Err(e) => {
                    println!("CoCreate IShellWindows FAILED: {e}");
                    return;
                }
            };
        let count = match shell.Count() {
            Ok(c) => c,
            Err(e) => {
                println!("Count FAILED {e}");
                return;
            }
        };
        println!("IShellWindows.Count = {count}");
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
            let Ok(dispatch) = shell.Item(&vi) else {
                println!("item {i}: Item() err");
                continue;
            };
            let browser = match dispatch.cast::<IWebBrowser2>() {
                Ok(b) => b,
                Err(e) => {
                    println!("item {i}: cast IWebBrowser2 err {e}");
                    continue;
                }
            };
            let Some(handle) = browser.HWND().ok() else {
                println!("item {i}: HWND err");
                continue;
            };
            let h = handle.0;
            let match_frame = h == fg.0 as isize;
            let match_tab = fg_tab != 0 && h == fg_tab;
            println!(
                "item {i}: hwnd={h:#x} (fg={:#x} tab={fg_tab:#x}) match_frame={match_frame} match_tab={match_tab}",
                fg.0 as isize
            );
            if match_frame || match_tab {
                let sp = match dispatch.cast::<IServiceProvider>() {
                    Ok(sp) => sp,
                    Err(e) => {
                        println!("  item {i}: cast IServiceProvider err {e}");
                        continue;
                    }
                };
                let brows = match sp
                    .QueryService::<windows::Win32::UI::Shell::IShellBrowser>(&SID_STopLevelBrowser)
                {
                    Ok(b) => b,
                    Err(e) => {
                        println!("  item {i}: QueryService err {e}");
                        continue;
                    }
                };
                let view = match brows.QueryActiveShellView() {
                    Ok(v) => v,
                    Err(e) => {
                        println!("  item {i}: QueryActiveShellView err {e}");
                        continue;
                    }
                };
                let fv = match view.cast::<IFolderView>() {
                    Ok(f) => f,
                    Err(e) => {
                        println!("  item {i}: cast IFolderView err {e}");
                        continue;
                    }
                };
                match fv.Items::<IEnumIDList>(SVGIO_SELECTION) {
                    Ok(items) => {
                        let mut slot: [*mut windows::Win32::UI::Shell::Common::ITEMIDLIST; 1] =
                            [std::ptr::null_mut()];
                        let mut got = 0u32;
                        let r = items.Next(&mut slot, Some(&mut got));
                        println!("  item {i}: Items.Next ok={} got={got}", r.is_ok());
                        if r.is_ok() && got == 1 && !slot[0].is_null() {
                            let mut path = [0u16; 260];
                            let ok = SHGetPathFromIDListW(slot[0], &mut path).as_bool();
                            let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
                            println!(
                                "  item {i}: SHGetPath ok={ok} path={:?}",
                                String::from_utf16_lossy(&path[..end])
                            );
                        }
                    }
                    Err(e) => println!("  item {i}: Items err {e}"),
                }
            }
        }
        let class_probe = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        println!("class_probe = {class_probe:?}");
        // 无论前台是否桌面都尝试一次兜底跳，让"桌面取件"与
        // "IShellWindows 缺条目时的实际行为"一并现形。
        {
            let mut phwnd = 0i32;
            let pv = VARIANT::default();
            match shell.FindWindowSW(
                &pv,
                &pv,
                windows::Win32::UI::Shell::SWC_DESKTOP,
                &mut phwnd,
                windows::Win32::UI::Shell::SWFO_NEEDDISPATCH,
            ) {
                Ok(disp) => {
                    println!("desktop: FindWindowSW ok phwnd={phwnd:#x}");
                    match disp.cast::<IServiceProvider>() {
                        Ok(sp) => {
                            match sp.QueryService::<windows::Win32::UI::Shell::IShellBrowser>(
                                &SID_STopLevelBrowser,
                            ) {
                                Ok(brows) => match brows.QueryActiveShellView() {
                                    Ok(view) => match view.cast::<IFolderView>() {
                                        Ok(fv) => match fv.Items::<IEnumIDList>(SVGIO_SELECTION) {
                                            Ok(items) => {
                                                let mut slot: [*mut windows::Win32::UI::Shell::Common::ITEMIDLIST; 1] = [std::ptr::null_mut()];
                                                let mut got = 0u32;
                                                let r = items.Next(&mut slot, Some(&mut got));
                                                println!(
                                                    "desktop: Items.Next ok={} got={got}",
                                                    r.is_ok()
                                                );
                                                if r.is_ok() && got == 1 && !slot[0].is_null() {
                                                    let mut path = [0u16; 260];
                                                    let ok =
                                                        SHGetPathFromIDListW(slot[0], &mut path)
                                                            .as_bool();
                                                    let end = path
                                                        .iter()
                                                        .position(|&c| c == 0)
                                                        .unwrap_or(path.len());
                                                    println!(
                                                        "desktop: SHGetPath ok={ok} path={:?}",
                                                        String::from_utf16_lossy(&path[..end])
                                                    );
                                                }
                                            }
                                            Err(e) => println!("desktop: Items err {e}"),
                                        },
                                        Err(e) => println!("desktop: cast IFolderView err {e}"),
                                    },
                                    Err(e) => println!("desktop: QueryActiveShellView err {e}"),
                                },
                                Err(e) => println!("desktop: QueryService err {e}"),
                            }
                        }
                        Err(e) => println!("desktop: cast IServiceProvider err {e}"),
                    }
                }
                Err(e) => println!("desktop: FindWindowSW FAILED {e}"),
            }
        }
    }
}
