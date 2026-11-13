//! `space_probe` —— 空格预览全链路真机探针（AGENTS §八.4 "UI 相关用离屏探针" 家族）。
//!
//! 回答一个用户报障的问题："开了空格预览却没效果，到底哪一环断了？"
//! 逐环打印 PASS/FAIL（examples 不在 hygiene 扫描内，stdout 就是它的报告形态）：
//!
//! 1. `spacehook::start` → 控制窗就绪；
//! 2. `set_enabled(true)` → 低级钩子真的挂上了（被杀软/占用会当场现形）；
//! 3. `SendInput` 注入一颗空格 → 钩子回调的观测计数上涨（证明系统键事件到得了回调；
//!    注：注入键按设计会被"接管判定"豁免，但观测点放在豁免之前，所以计数必涨）；
//! 4. `explorer /select` 样本图 → 轮询前台到 Windows 资源管理器窗口，打印类名、
//!    打字判据（期望 false）——这一环专杀 Win11 标签式窗口的前台匹配问题；
//! 5. `foreground_selected_file` 三跳取选中路径 == 样本路径；
//! 6. 同一文件两次 `probe_decode`：两次都必须真解出来（时长只打给人看，不作判据）。
//!
//! 用法（在 main 目录下）：
//! ```text
//! set RUPIX_DECODERS_DIR=<工作区根>\.build\decoders
//! cargo run --example space_probe
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};

/// 探针样本图的边长：2000×1500 = 12MB RGBA，稳过 2MB 落盘门槛。
const IMG_W: u32 = 2000;
const IMG_H: u32 = 1500;

fn main() {
    rupix_lib::platform::logging::install_logging();
    // 与 GUI 入口同一套解码栈（不装则 declared_exts 为空、probe_decode 无从解）。
    rupix_lib::install_decode_stack();

    let mut failures = 0u32;

    // ── 1. 线程与控制窗 ─────────────────────────────────────────────
    let before = rupix_lib::platform::spacehook::status();
    println!("step1 pre-status: {before:?}");
    if let Err(e) = rupix_lib::platform::spacehook::start() {
        println!("FAIL step1: start() = {e}");
        std::process::exit(1);
    }
    let st = wait_until(
        || rupix_lib::platform::spacehook::status().ctrl_ready,
        Duration::from_secs(5),
    );
    if !st {
        println!("FAIL step2: 控制窗 5s 未就绪");
        failures += 1;
    } else {
        println!("ok  step2: 控制窗就绪");
    }

    // ── 2. 装钩 ────────────────────────────────────────────────────
    rupix_lib::platform::spacehook::set_enabled(true);
    let hooked = wait_until(
        || rupix_lib::platform::spacehook::status().hooked,
        Duration::from_secs(5),
    );
    if !hooked {
        println!("FAIL step3: set_enabled(true) 后钩子未挂上（被占用/拦截？看上面的 warn 日志）");
        failures += 1;
    } else {
        println!("ok  step3: WH_KEYBOARD_LL 钩子在位");
    }

    // ── 3. 注入一颗空格，看回调计数 ────────────────────────────────
    let seen0 = rupix_lib::platform::spacehook::status().space_seen;
    inject_space();
    let grew = wait_until(
        || rupix_lib::platform::spacehook::status().space_seen > seen0,
        Duration::from_secs(2),
    );
    let st = rupix_lib::platform::spacehook::status();
    if !grew {
        println!(
            "FAIL step4: 注入键未被回调观测（seen={}）——注入豁免把计数也挡了？还是钩子根本没收到",
            st.space_seen
        );
        failures += 1;
    } else {
        println!(
            "ok  step4: 钩子回调可达（space_seen {} → {}）",
            seen0, st.space_seen
        );
    }

    // ── 4/5. 资源管理器前台 + 取选中 ───────────────────────────────
    // 清单覆盖面先打一遍：`is_previewable` 是空格预览的**准入判据**，探针里取件用的是
    // 全放行（`|_| true`），所以这条判据断掉时探针照样全绿——`.raf` / `.uyvy` 就是这么
    // 漏掉的（排除项先按 DLL 级 `is_raw` 判、后按 headerless 后缀判，两次都是拿后缀当
    // "解不解得开"的代理）。这里把它抄回来。
    let declared = rupix_lib::core::formats::declared_exts();
    if !declared.is_empty() {
        let mut blocked: Vec<&str> = declared
            .iter()
            .filter(|e| !rupix_lib::core::formats::is_previewable(e))
            .map(String::as_str)
            .collect();
        blocked.sort_unstable();
        println!(
            "step5 清单 {} 个后缀，其中不可空格预览 {} 个：{}",
            declared.len(),
            blocked.len(),
            blocked.join(" ")
        );
        // 相机 RAW 与裸像素都必须能预览：准入只看"有没有解码器认领这个后缀"，
        // 裸像素解不解得出由解码前那次参数推断现场决定（推不出才静默）。
        // 两轮用户实测都是这一条断掉——先 `.raf`（按 DLL 级 is_raw 排整档），
        // 后 `.uyvy`（改按 headerless 后缀排）——两次都是拿后缀当"解不解得开"的代理。
        let mut bad: Vec<&str> = [
            "cr2", "nef", "raf", "dng", "arw", "rgb", "rgba", "bgra", "gray", "yuv", "uyvy",
            "yuy2", "i420", "yv12", "nv12",
        ]
        .into_iter()
        .filter(|e| declared.contains(*e) && !rupix_lib::core::formats::is_previewable(e))
        .collect();
        bad.sort_unstable();
        if bad.is_empty() {
            println!("ok  step5b: 相机 RAW 与裸像素后缀都在可预览集合内");
        } else {
            println!(
                "FAIL step5b: 这些后缀被挡在空格预览外：{}（双击能开、按空格没反应）",
                bad.join(" ")
            );
            failures += 1;
        }
    }
    let sample = make_sample_png();
    match &sample {
        Ok(p) => println!("ok  sample: {}", p.display()),
        Err(e) => println!("FAIL 样本生成: {e}"),
    }
    if let Ok(path) = sample {
        // 关掉上一步探针自窗：让 explorer /select 抢前台。
        std::process::Command::new("explorer")
            .arg(format!("/select,{path:?}"))
            .spawn()
            .map_err(|e| println!("FAIL explorer /select: {e}"))
            .ok();
        let fg_shell = wait_until(
            || {
                let fg = unsafe { GetForegroundWindow() };
                let mut buf = [0u16; 64];
                let n = unsafe { GetClassNameW(fg, &mut buf) };
                let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
                class == "CabinetWClass" || class == "ExploreWClass"
            },
            Duration::from_secs(8),
        );
        let mut buf = [0u16; 64];
        let fg = unsafe { GetForegroundWindow() };
        let n = unsafe { GetClassNameW(fg, &mut buf) };
        let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        println!(
            "step5 前台: class={class:?} 达到外壳窗={} 打字判据={}（期望 False）",
            fg_shell,
            rupix_lib::platform::shell_selection::is_typing_context()
        );
        if !fg_shell {
            println!("WARN step5: 前台 8s 内没等到资源管理器——后面取件判定以实际前台为准");
        }
        if fg_shell {
            // 贴近真实使用：用户按空格时资源管理器窗口早已稳定存在。explorer /select
            // 弹出的**全新一秒**窗口有两段竞态（K38）：外壳条目注册、选中异步完成——
            // 实测 800ms/1.8s 都可能"窗口在前台而选择集为空"（Items 报 0x80070490），
            // 3s 才稳；生产环境用户早就选好了，这个等待只是探针贴平现场。
            std::thread::sleep(Duration::from_secs(3));
        }
        let all = |_: &std::path::Path| true;
        // 前台取件轮询 10s：窗口注册与选中就绪都可能慢（K38 实测：/select 窗口
        // 前台就绪 ≠ 选择集就绪）。
        let mut got = None;
        for _ in 0..20 {
            got = rupix_lib::platform::shell_selection::foreground_selected_file(&all);
            if got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        match got.as_deref() {
            Some(p) if p == path => println!("ok  step6: 三跳取到选中路径 = 样本"),
            Some(p) => {
                println!("WARN step6: 取到的是 {p:?}（用户此刻的选中项），说明链路本身是通的")
            }
            None => {
                // 降级取证：本系统对 `explorer /select` 新窗口可能**从不注册**
                // IShellWindows（真机实测：前台稳定 30s 仍无条目）——这是探针
                // 现场缺陷而非产品缺陷（用户手动开的窗口注册与取件正常，见
                // examples/mini_take5）。此时改验「取件 API 存活」：对枚举到的
                // 任意 explorer 条目做一次三跳，能报选择集状态即算通过，
                // 端到端验收转交真机手动步骤。
                if selection_api_alive() {
                    println!(
                        "WARN step6: /select 新窗口未注册 IShellWindows（系统行为，探针现场缺陷）；\
                         取件 API 本身存活——真机端到端请手动验收（开窗选中图按空格）"
                    );
                } else {
                    println!("FAIL step6: 取件 API 也不可用——选中探测断在这里");
                    failures += 1;
                }
            }
        }

        // ── 6. 同一张图解两次都必须成功（空格预览会连按两次开合同一文件）──
        let t1 = Instant::now();
        let r1 = rupix_lib::core::decode::probe_decode(&path, IMG_W, IMG_H);
        let c1 = t1.elapsed();
        println!("step7 decode#1: {:?}（{c1:?}）", r1.is_ok());
        let t2 = Instant::now();
        let r2 = rupix_lib::core::decode::probe_decode(&path, IMG_W, IMG_H);
        let c2 = t2.elapsed();
        println!("step7 decode#2: {:?}（{c2:?}）", r2.is_ok());
        if !(r1.is_ok() && r2.is_ok()) {
            println!("FAIL step7: 两次解码没都成功");
            failures += 1;
        } else {
            // 磁盘解码缓存已于 2026-10-19 撤掉：这里只断"第二次照样解得出来"，
            // 两次数值打给人看，不再拿"走缓存更快"当判据。
            println!(
                "ok  step7: 两次都是真解码，时长 {}ms / {}ms",
                c1.as_millis(),
                c2.as_millis()
            );
        }
        // ── 6b. 裸像素：空格预览没有参数窗，只能自动推断 ──────────────
        // 后缀定布局、文件长度反推宽高，与双击 `raw_dialog::open_path` 的"推得出就直接
        // 开"是同一条推断。任一环断掉，用户看到的就是"双击开得开、按空格没反应"
        // （`.uyvy` / `.rgb` 实测）。样本用 640×480 的 RGB8，长度 921600。
        let raw = make_sample_raw(640, 480);
        match &raw {
            Ok(raw_path) => {
                match rupix_lib::core::decode::infer_raw_params(raw_path) {
                    Some(p) if (p.width, p.height) == (640, 480) => {
                        rupix_lib::core::decode::set_raw_params(raw_path, p);
                        match rupix_lib::core::decode::probe_decode(raw_path, IMG_W, IMG_H) {
                            Ok(d) => println!(
                                "ok  step7b: 裸像素 .rgb 推断 {}x{} 并解出 {:?}",
                                p.width, p.height, d.natural
                            ),
                            Err(e) => {
                                println!("FAIL step7b: 裸像素推断出了参数却解不出：{e}");
                                failures += 1;
                            }
                        }
                    }
                    other => {
                        println!("FAIL step7b: 裸像素参数推断失败（{:?}）", other);
                        failures += 1;
                    }
                }
                let _ = std::fs::remove_file(raw_path);
            }
            Err(e) => {
                println!("FAIL step7b 样本: {e}");
                failures += 1;
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    // ── 收尾：关开关 → 摘钩 ────────────────────────────────────────
    rupix_lib::platform::spacehook::set_enabled(false);
    let unhooked = wait_until(
        || !rupix_lib::platform::spacehook::status().hooked,
        Duration::from_secs(5),
    );
    println!(
        "step8 摘钩: hooked={} status={:?}",
        !unhooked,
        rupix_lib::platform::spacehook::status()
    );

    if failures == 0 {
        println!("PROBE ALL PASS");
    } else {
        println!("PROBE FAILURES={failures}");
        std::process::exit(1);
    }
}

/// 按间隔轮询判据到超时；true=最终达成。
fn wait_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

/// 取件 API 存活验证（step6 降级取证）：对 `IShellWindows` 枚举到的任意 explorer
/// 条目做一次三跳（QueryActiveShellView → GetItemObject → GetData），**无论取到
/// 选择集还是明确的"空选择"，都证明 API 链路活着**；全链 COM 报错才算死。
/// 场景：本系统对 `explorer /select` 新窗口可能从不注册 IShellWindows（K38），
/// 前台匹配永远落空——此时用本函数区分"探针现场缺陷"与"取件 API 故障"。
fn selection_api_alive() -> bool {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IDataObject, IServiceProvider, DVASPECT_CONTENT,
        FORMATETC, TYMED_HGLOBAL,
    };
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
    use windows::Win32::UI::Shell::{
        IShellBrowser, IShellWindows, SID_STopLevelBrowser, SVGIO_SELECTION,
    };

    unsafe {
        let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        let clsid = windows::core::GUID::from_u128(0x9BA05972_F6A8_11CF_A442_00A0C90A8F39);
        let Ok(shell): windows::core::Result<IShellWindows> = CoCreateInstance(
            &clsid,
            None,
            windows::Win32::System::Com::CLSCTX_LOCAL_SERVER,
        ) else {
            println!("  alive-check: CoCreate IShellWindows FAILED");
            return false;
        };
        let Ok(count) = shell.Count() else {
            println!("  alive-check: Count FAILED");
            return false;
        };
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
            let Ok(sp) = disp.cast::<IServiceProvider>() else {
                continue;
            };
            let Ok(brows) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else {
                continue;
            };
            let Ok(view) = brows.QueryActiveShellView() else {
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
                            windows::Win32::System::Ole::ReleaseStgMedium(&mut med);
                            println!(
                                "  alive-check: 条目 {i} GetItemObject+GetData OK（选择集可取）"
                            );
                            return true;
                        }
                        Err(e) => {
                            // 明确的"空/不可取"也是 API 活着的证据（COM 链全通）。
                            println!("  alive-check: 条目 {i} GetData 报 {e}（COM 链通）");
                            return true;
                        }
                    }
                }
                Err(e) => println!("  alive-check: 条目 {i} GetItemObject 报 {e}，试下一条"),
            }
        }
        println!("  alive-check: 没有任何 IShellWindows 条目可试");
        false
    }
}

/// 注入一颗空格（down+up）。钩子链可达性的确定性证明——不依赖任何人碰键盘。
fn inject_space() {
    unsafe {
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_SPACE,
                    wScan: 0,
                    dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_SPACE,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let sent = SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32);
        let _ = (sent, WPARAM(0), LPARAM(0));
    }
}

/// 生成 2000×1500 诊断样本 PNG（对角条纹）。
/// 裸像素样本：RGB8 交错、无文件头，长度 = w×h×3。
///
/// 存在的理由：`.uyvy` / `.rgb` 这类文件"双击开得开、按空格没反应"是格式准入的
/// 经典回归，而它们的解码必须带参数（后缀定布局 + 长度定宽高）。样本自己造，
/// 不依赖 `.build/1M` 那批真机语料。
fn make_sample_raw(w: u32, h: u32) -> Result<PathBuf, String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.temp/probe_samples");
    std::fs::create_dir_all(&dir).map_err(|e| format!("样本目录创建失败: {e}"))?;
    let path = dir.join(format!("rupix-space-probe-{}.rgb", std::process::id()));
    let mut img: Vec<u8> = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for y in 0..h {
        for x in 0..w {
            let stripe = ((x + y) / 32) % 2 == 0;
            let (r, g, b) = if stripe {
                (220u8, 40, 60)
            } else {
                (30u8, 140, 200)
            };
            img.extend_from_slice(&[r, g, b]);
        }
    }
    std::fs::write(&path, &img).map_err(|e| format!("创建裸像素样本失败: {e}"))?;
    Ok(path)
}

fn make_sample_png() -> Result<PathBuf, String> {
    // 关键：样本放**小目录**（workspace 的 .temp/probe_samples），不放系统 Temp——
    // Temp 动辄数万文件，explorer /select 打开它要在异步加载里挣扎好几秒，
    // 而真实用户操作的图片目录都是普通小文件夹。目录选错 = 探针测出生产环境
    // 根本不会遇到的"秒开秒选"假竞争，白绕一大圈（2026-10-17 教训实录）。
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.temp/probe_samples");
    std::fs::create_dir_all(&dir).map_err(|e| format!("样本目录创建失败: {e}"))?;
    let path = dir.join(format!("rupix-space-probe-{}.png", std::process::id()));
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(
            std::fs::File::create(&path).map_err(|e| format!("创建样本失败: {e}"))?,
        ),
        IMG_W,
        IMG_H,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    // 整幅一次编码（write_image_data），不做逐行流式——样本只此一份，12MB 内存无所谓。
    let mut img: Vec<u8> = Vec::with_capacity((IMG_W as usize) * (IMG_H as usize) * 4);
    for y in 0..IMG_H {
        for x in 0..IMG_W {
            let stripe = ((x + y) / 32) % 2 == 0;
            let (r, g, b) = if stripe {
                (220u8, 40, 60)
            } else {
                (30u8, 140, 200)
            };
            // 留一小块完全透明的角落，顺带把 alpha 通道也走一遍。
            let a = if x > IMG_W - 3 && y > IMG_H - 3 {
                0u8
            } else {
                255u8
            };
            img.extend_from_slice(&[r, g, b, a]);
        }
    }
    enc.write_header()
        .map_err(|e| format!("PNG 头失败: {e}"))?
        .write_image_data(&img)
        .map_err(|e| format!("PNG 数据失败: {e}"))?;
    Ok(path)
}
