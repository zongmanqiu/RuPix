//! Manual end-to-end probe for the plugin process/pipe/protocol stack.
//!
//! Usage:
//!   cargo run --release --example plugin_smoke -- ..\.temp\plugin-dist\echo
//!
//! It starts the plugin, sends one request, waits for the result, and prints it.
//! Then it exercises the **dynamic hotkey** path: the plugin is copied into the
//! exe-adjacent `plugins/` directory (that is where the host discovers plugins),
//! and the same plugin is started again through its manifest-declared shortcut.
//! This is a probe, not a hosted extension; plugin sources live in another repo.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rupix_lib::core::config::Config;
use rupix_lib::core::ext::Manifest;
use rupix_lib::core::hotkey::{KeyInput, VKey};
use rupix_lib::extensions::{self, Extension};

/// 递归复制一个目录（探针要把自己放到 exe 同级 `plugins/` 下）。
/// 探针图（m166 第一格）：4×2 RGBA，每像素首字节 = 序号，尺寸与像素数严格对得上
/// （不然是撞映射校验的假失败）。带的是"通道通不通"，不是"解得对不对"——后者归解码侧门禁。
fn probe_pixels(path: PathBuf) -> rupix_lib::core::viewer::ForPlugin {
    let px: Vec<u8> = (0..8u8).flat_map(|i| [i, 0, 200, 255]).collect();
    rupix_lib::core::viewer::ForPlugin {
        path,
        width: 4,
        height: 2,
        rgba: std::sync::Arc::new(px),
        orient: (0, false),
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("建 {} 失败：{e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读 {} 失败：{e}", src.display()))?
    {
        let entry = entry.map_err(|e| format!("读目录项失败：{e}"))?;
        let to = dst.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to).map_err(|e| format!("复制失败：{e}"))?;
        }
    }
    Ok(())
}

/// 等一次插件结果（最多 35s）。
fn wait_result() -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(35);
    while Instant::now() < deadline {
        if let Some(job) = extensions::poll().into_iter().next() {
            return match job.result {
                Ok(crate::extensions::run::JobOutput::Text { text, copied, .. }) => {
                    Ok(format!("{text}（已自动复制到剪贴板：{copied}）"))
                }
                Ok(crate::extensions::run::JobOutput::Edited { width, height, .. }) => {
                    Ok(format!("编辑回写 {width}x{height}（编辑型不走本探针）"))
                }
                Ok(crate::extensions::run::JobOutput::Settings) => {
                    Err("设置型结果不走本探针（用 --settings 臂验证）".into())
                }
                Err(e) => Err(format!("{}：{e}", job.plugin)),
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("等待插件结果超时".into())
}

fn main() -> Result<(), String> {
    // 每个 example 装一次 logger（AGENTS §日志纪律）：插件的 stdout/stderr 会被
    // 抽进宿主日志，不装 logger 就只能看到"插件没回应"，看不到它说了什么。
    rupix_lib::platform::logging::install_logging();
    let mut args = std::env::args().skip(1);
    let dir = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "用法：plugin_smoke <插件目录> [图片路径]".to_string())?;
    let image = args.next().map(PathBuf::from);
    let manifest = Manifest::from_dir(&dir)?;
    let ext = Extension {
        manifest,
        dir: dir.clone(),
        enabled: true,
        blocked: false,
        isolated: false,
    };
    let name = ext.manifest.name.clone();
    let id = ext.manifest.id.clone();
    // m166 第一格：命令型也能收到像素，所以探针把这一路也走一遍。传的是**探针图**
    // （4×2，每像素一个可辨认的字节），不是真实解码结果——本探针验的是"通道通不通"，
    // 不是"解得对不对"（那是解码侧的门禁）。声明了 `image.read` 的插件会看到请求里
    // 多出 `image{...}`；没声明的（如 echo）由 `pixels_to_send` 挡掉，行为与以前一致。
    let pixels = image.as_ref().map(|p| probe_pixels(p.clone()));
    extensions::start_command(ext.clone(), image.clone(), None, pixels.clone())?;
    println!("started: {name}");
    println!("result: {}", wait_result()?);

    // ── 真实像素这一腿（m166 第一格插件半的验收台）──
    // 探针图只证明"通道通、字节不错位"；分析型插件的读数在**真解出来的像素**上长什么样，
    // 得让宿主真的解一次。解码走宿主自己的栈（探针不自备解码，见 AGENTS §七 DecodeBackend）。
    // 目标档 2000×2000：刻意落在宿主 8 MP 下发闸之内，这一腿测的才是"给了像素"那条支路。
    if let Some(p) = image.as_ref() {
        rupix_lib::install_decode_stack();
        let d = rupix_lib::core::decode::probe_decode(p, 2000, 2000)
            .map_err(|e| format!("解码 {} 失败，真实像素这一腿没跑成：{e}", p.display()))?;
        let (w, h) = (d.bitmap.width, d.bitmap.height);
        let real = rupix_lib::core::viewer::ForPlugin {
            path: p.clone(),
            width: w,
            height: h,
            rgba: std::sync::Arc::new(d.bitmap.rgba),
            orient: (0, false),
        };
        extensions::start_command(ext.clone(), Some(p.clone()), None, Some(real))?;
        println!("[real-pixels] natural={:?} sent={w}x{h}", d.natural);
        println!("[real-report] {}", wait_result()?);
    }

    // ── 动态热键路径 ──
    // 宿主只在 exe 同级 `plugins/<id>/` 里发现插件，所以先把它放进去。
    let plugins_dir = rupix_lib::core::paths::plugin_dir().ok_or("取不到 exe 同级 plugins 目录")?;
    let dest = plugins_dir.join(&id);
    copy_dir(&dir, &dest)?;
    extensions::invalidate_installed_cache();

    let cfg = Config::load();
    let Some(hk) = ext.manifest.hotkey else {
        // 热键是 manifest 的**可选**字段，四枚第一方插件都没声明 ⇒ 这一腿只能如实跳过。
        // 不许把"没声明"报成失败：探针一退 1，前面那两腿的真读数反而没人看。
        println!("hotkey: manifest 未声明热键，跳过这一腿（前两腿已跑）");
        return Ok(());
    };
    println!("hotkey: manifest 声明 {}", hk.pretty());

    // 用 manifest 里那把键构造一次按键事件（探针不模拟整条 UI 事件链）。
    let (vkey, ctrl, alt, shift, win) = match hk.base {
        rupix_lib::core::hotkey::Base::F(n) => (VKey::F(n), hk.ctrl, hk.alt, hk.shift, hk.win),
        rupix_lib::core::hotkey::Base::Char(c) => {
            (VKey::Char(c), hk.ctrl, hk.alt, hk.shift, hk.win)
        }
        _ => return Err("探针只支持字母键与功能键".into()),
    };
    let ev = KeyInput {
        key: vkey,
        ctrl,
        alt,
        shift,
        win,
    };
    let msg = extensions::run_by_hotkey(&cfg, &ev, image.clone(), pixels)?;
    println!("hotkey: {msg}");
    println!("hotkey result: {}", wait_result()?);
    Ok(())
}
