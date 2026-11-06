//! Manual end-to-end probe for the plugin process/pipe/protocol stack.
//!
//! Usage:
//!   cargo run --release --example plugin_smoke -- ..\.temp\plugin-dist\echo
//!   cargo run --release --example plugin_smoke -- --channels ..\.temp\plugin-dist\channels
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
                    // 命令型探针走不到这里（start_command 只收 Kind::Command）；留着是防御。
                    // 编辑型的端到端验收在 `--editor` 臂，见 run_editor_probe。
                    Ok(format!(
                        "收到编辑回写 {width}x{height}（请用 --editor 臂验收）"
                    ))
                }
                Ok(crate::extensions::run::JobOutput::Settings) => {
                    Err("命令型探针收到设置型结果（请用 --settings 臂验收）".into())
                }
                Err(e) => Err(format!("{}：{e}", job.plugin)),
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("等待插件结果超时".into())
}

// ─────────────────────────── W4：编辑回写契约验收臂 ───────────────────────────
//
// 背景：待办 §26「两条通道账」记着编辑回写通道「留着但没人走」，本探针以前也明写
// 「编辑型不走本探针」。W4 就是把它接上：全部走宿主真实的
// `start_editor` → `poll` → `JobOutput::Edited`，**不在这里重写一份协议/共享内存栈**。
// 量具插件放在 <工作区>/.temp/w4-editor-contract/，不发布、不进清单。

struct EditedInfo {
    width: u32,
    height: u32,
    orient: (u8, bool),
    rgba: Vec<u8>,
}

/// 等一次插件结果，把 JobOutcome 原样交回（命令型那条 wait_result 只要文本）。
fn wait_job() -> Result<extensions::JobOutcome, String> {
    let deadline = Instant::now() + Duration::from_secs(35);
    while Instant::now() < deadline {
        if let Some(job) = extensions::poll().into_iter().next() {
            return Ok(job);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("等待插件结果超时".into())
}

/// 同一枚量具插件换一个 `--mode`（宿主把 manifest.args 放在 --rupix-pipe 之前）。
fn with_mode(base: &Extension, mode: &str) -> Extension {
    let mut ext = base.clone();
    ext.manifest.args = vec!["--mode".into(), mode.into()];
    ext
}

fn editor_pixels(
    width: u32,
    height: u32,
    orient: (u8, bool),
    rgba: Vec<u8>,
) -> rupix_lib::core::viewer::ForPlugin {
    rupix_lib::core::viewer::ForPlugin {
        path: PathBuf::from("w4-editor-probe.png"),
        width,
        height,
        rgba: std::sync::Arc::new(rgba),
        orient,
    }
}

fn one_edit(ext: Extension, px: rupix_lib::core::viewer::ForPlugin) -> Result<EditedInfo, String> {
    extensions::start_editor(ext, px)?;
    let job = wait_job()?;
    match job.result {
        Ok(extensions::JobOutput::Edited {
            width,
            height,
            rgba,
            orient,
            ..
        }) => Ok(EditedInfo {
            width,
            height,
            orient,
            rgba,
        }),
        Ok(_) => Err("命令型结果混进了编辑型验收".into()),
        Err(e) => Err(e),
    }
}

fn record(label: &str, ok: bool, detail: &str, fails: &mut Vec<String>) {
    println!(
        "[w4] arm={:<10} verdict={} detail={}",
        label,
        if ok { "PASS" } else { "FAIL" },
        detail
    );
    if !ok {
        fails.push(label.to_string());
    }
}

/// 六条臂：正常回写 / orient 冻结 / 同尺寸拒绝 / 无 image 拒绝 / 显式失败 / 8 MP 闸。
fn run_editor_probe(dir: &Path) -> Result<(), String> {
    let manifest = Manifest::from_dir(dir)?;
    if manifest.kind != rupix_lib::core::ext::Kind::Editor {
        return Err(format!("{} 不是 kind=editor 插件", manifest.name));
    }
    let base = Extension {
        manifest,
        dir: dir.to_path_buf(),
        enabled: true,
        blocked: false,
        isolated: false,
    };

    // 4×2 探针图：每像素首字节 = 序号；反相结果可逐字节断言，不靠肉眼。
    let probe: Vec<u8> = (0..8u8).flat_map(|i| [i, 0, 200, 255]).collect();
    let inverted: Vec<u8> = probe
        .chunks_exact(4)
        .flat_map(|p| [255 - p[0], 255 - p[1], 255 - p[2], p[3]])
        .collect();
    let mut fails: Vec<String> = Vec::new();

    // A1 正常回写：尺寸、像素、orient 三样都必须是送进去的那一档。
    match one_edit(
        with_mode(&base, "invert"),
        editor_pixels(4, 2, (0, false), probe.clone()),
    ) {
        Ok(r) => {
            let ok = r.width == 4 && r.height == 2 && r.orient == (0, false) && r.rgba == inverted;
            record(
                "invert",
                ok,
                &format!(
                    "{}x{} orient=({},{}) rgba_match={} len={}",
                    r.width,
                    r.height,
                    r.orient.0,
                    r.orient.1,
                    r.rgba == inverted,
                    r.rgba.len()
                ),
                &mut fails,
            );
        }
        Err(e) => record("invert", false, &format!("err={e}"), &mut fails),
    }

    // A2 orient 冻结：送进去的非零朝向必须原样带到落地（转回源朝向是
    // install_edited_displayed 的事，已有 edited.rs 单测，不在这里重复）。
    match one_edit(
        with_mode(&base, "invert"),
        editor_pixels(4, 2, (1, false), probe.clone()),
    ) {
        Ok(r) => record(
            "orient",
            r.orient == (1, false) && r.rgba == inverted,
            &format!(
                "frozen=({},{}) rgba_match={}",
                r.orient.0,
                r.orient.1,
                r.rgba == inverted
            ),
            &mut fails,
        ),
        Err(e) => record("orient", false, &format!("err={e}"), &mut fails),
    }

    // A3 同尺寸约束：插件回 4×3，宿主必须在拷出像素前拒绝。
    let a3 = one_edit(
        with_mode(&base, "bad-size"),
        editor_pixels(4, 2, (0, false), probe.clone()),
    );
    let a3_ok = matches!(&a3, Err(e) if e.contains("尺寸"));
    record(
        "bad-size",
        a3_ok,
        &format!("got={:?}", a3.as_ref().err()),
        &mut fails,
    );

    // A4 无 image：ok=true 却不回图，宿主必须报错而不是当成"编辑成功"。
    let a4 = one_edit(
        with_mode(&base, "no-image"),
        editor_pixels(4, 2, (0, false), probe.clone()),
    );
    let a4_ok = matches!(&a4, Err(e) if e.contains("未回传图像"));
    record(
        "no-image",
        a4_ok,
        &format!("got={:?}", a4.as_ref().err()),
        &mut fails,
    );

    // A5 显式失败：插件 ok=false 的 error 要传到结果里。
    let a5 = one_edit(
        with_mode(&base, "fail"),
        editor_pixels(4, 2, (0, false), probe.clone()),
    );
    let a5_ok = matches!(&a5, Err(e) if e.contains("failure"));
    record(
        "fail",
        a5_ok,
        &format!("got={:?}", a5.as_ref().err()),
        &mut fails,
    );

    // A6 8 MP 闸：2829² = 8,003,241 > 8,000,000。闸在 publish_image 之前，
    // 所以这里不给像素缓冲（闸只看 w*h，不会读到它）。
    let a6 = extensions::start_editor(
        with_mode(&base, "invert"),
        editor_pixels(2829, 2829, (0, false), Vec::new()),
    );
    let a6_ok = matches!(&a6, Err(e) if e.contains("8 MP"));
    record(
        "over-8mp",
        a6_ok,
        &format!("got={:?}", a6.as_ref().err()),
        &mut fails,
    );

    println!(
        "[w4] arms=6 failed={} verdict={}",
        fails.len(),
        if fails.is_empty() { "PASS" } else { "FAIL" }
    );
    if fails.is_empty() {
        Ok(())
    } else {
        Err(format!("失败臂：{}", fails.join(",")))
    }
}

// ─────────────────────────── W5：设置通道验收臂 ───────────────────────────
//
// 待办 §26 的「两条通道账」：第 1 条（编辑回写）已由 W4 销账，第 2 条设置通道
// 仍只有存储层单测，没有真插件进程走过管道。本臂走宿主真实的
// `start_settings` / `start_command` → `attach_settings` / `persist_settings`，
// 不重写一份协议栈。量具在 <工作区>/.temp/w5-settings-contract/，不发布。

fn settings_store(id: &str) -> Result<PathBuf, String> {
    rupix_lib::core::paths::plugin_dir()
        .map(|d| d.join(id).join("settings.json"))
        .ok_or_else(|| "拿不到插件目录".to_string())
}

fn read_store(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// 同一枚量具换能力与 `settings_ui`（能力最小化门控的对照臂）。
fn variant(base: &Extension, mode: &str, read: bool, write: bool, ui: bool) -> Extension {
    use rupix_lib::core::ext::Capability::{SettingsRead, SettingsWrite};
    let mut e = with_mode(base, mode);
    e.manifest.capabilities.retain(|c| {
        let is_read = *c == SettingsRead;
        let is_write = *c == SettingsWrite;
        (!is_read || read) && (!is_write || write)
    });
    e.manifest.settings_ui = ui;
    e
}

fn start_settings_wait(ext: Extension) -> Result<extensions::JobOutcome, String> {
    extensions::start_settings(ext)?;
    wait_job()
}

fn start_command_wait(ext: Extension) -> Result<extensions::JobOutcome, String> {
    extensions::start_command(ext, None, None, None)?;
    wait_job()
}

fn outcome_text(j: &extensions::JobOutcome) -> Option<&str> {
    match &j.result {
        Ok(extensions::JobOutput::Text { text, .. }) => Some(text.as_str()),
        _ => None,
    }
}

fn record5(label: &str, ok: bool, detail: &str, fails: &mut Vec<String>) {
    println!(
        "[w5] arm={:<10} verdict={} detail={}",
        label,
        if ok { "PASS" } else { "FAIL" },
        detail
    );
    if !ok {
        fails.push(label.to_string());
    }
}

/// 七条臂：齿轮落盘 / run 下发 / 无 write 不落盘 / 无 read 不下发 /
/// 合并语义 / settings_ui 门控 / 声明式 schema 可解析。
fn run_settings_probe(dir: &Path) -> Result<(), String> {
    let manifest = Manifest::from_dir(dir)?;
    let base = Extension {
        manifest,
        dir: dir.to_path_buf(),
        enabled: true,
        blocked: false,
        isolated: false,
    };
    let store = settings_store(&base.manifest.id)?;
    let _ = std::fs::remove_file(&store);
    let mut fails: Vec<String> = Vec::new();

    // A：`action=settings` → 插件回 settings → 宿主合并落盘。
    let a = start_settings_wait(variant(&base, "gear", true, true, true));
    let a_res = matches!(&a, Ok(j) if matches!(&j.result, Ok(extensions::JobOutput::Settings)));
    let disk_a = read_store(&store);
    let a_disk = disk_a.contains("w5.gear") && disk_a.contains("gear-ok");
    record5(
        "gear",
        a_res && a_disk,
        &format!(
            "result=Settings:{a_res} disk_has_gear:{a_disk} bytes={}",
            disk_a.len()
        ),
        &mut fails,
    );

    // C：掉 `settings.write` 后回传 settings，落盘必须一字不变。
    let c = start_settings_wait(variant(&base, "gear", true, false, true));
    let disk_c = read_store(&store);
    let c_ok = matches!(&c, Ok(j) if matches!(&j.result, Ok(extensions::JobOutput::Settings)))
        && disk_c == disk_a;
    record5(
        "no-write",
        c_ok,
        &format!("disk_unchanged={}", disk_c == disk_a),
        &mut fails,
    );

    // B：有 `settings.read` 时，run 的请求里必须带上刚存的键值。
    let b = start_command_wait(variant(&base, "noop", true, true, true));
    let b_ok = b
        .as_ref()
        .ok()
        .and_then(outcome_text)
        .is_some_and(|t| t.contains("w5.gear") && t.contains("gear-ok"));
    record5("read-down", b_ok, &format!("seen_key={b_ok}"), &mut fails);

    // D：掉 `settings.read` 后，请求里不许出现 settings。
    let d = start_command_wait(variant(&base, "noop", false, true, true));
    let d_ok = d
        .as_ref()
        .ok()
        .and_then(outcome_text)
        .is_some_and(|t| t.contains("seen=none"));
    record5("no-read", d_ok, &format!("seen_none={d_ok}"), &mut fails);

    // E：齿轮写 A、run 回 B ⇒ 落盘是 A+B（dumb store 合并，不整体替换）。
    let _ = start_command_wait(variant(&base, "run", true, true, true));
    let disk_e = read_store(&store);
    let e_ok = disk_e.contains("w5.gear") && disk_e.contains("w5.run") && disk_e.contains("run-ok");
    record5(
        "merge",
        e_ok,
        &format!(
            "has_A={} has_B={}",
            disk_e.contains("w5.gear"),
            disk_e.contains("w5.run")
        ),
        &mut fails,
    );

    // F：没声明 settings_ui 的插件不该被拉起设置窗口。
    let f = extensions::start_settings(variant(&base, "gear", true, true, false));
    let f_ok = matches!(&f, Err(e) if e.contains("设置界面"));
    record5(
        "ui-gate",
        f_ok,
        &format!("got={:?}", f.as_ref().err()),
        &mut fails,
    );

    // G：声明式 settings 数组能被宿主解析（设置窗本身由宿主渲染，属实机）。
    let schema_path = dir.join("manifest-schema.json");
    let (g_ok, g_detail) = if schema_path.is_file() {
        let txt = std::fs::read_to_string(&schema_path).unwrap_or_default();
        match Manifest::parse(&txt) {
            Ok(m) => {
                let keys: Vec<&str> = m.settings.iter().map(|x| x.key.as_str()).collect();
                (
                    m.settings.len() == 2
                        && !m.settings_ui
                        && keys.contains(&"effect")
                        && keys.contains(&"strength"),
                    format!(
                        "items={} ui={} keys={:?}",
                        m.settings.len(),
                        m.settings_ui,
                        keys
                    ),
                )
            }
            Err(e) => (false, format!("parse_err={e}")),
        }
    } else {
        (false, "manifest-schema.json 缺失".into())
    };
    record5("schema", g_ok, &g_detail, &mut fails);

    println!(
        "[w5] arms=7 failed={} verdict={}",
        fails.len(),
        if fails.is_empty() { "PASS" } else { "FAIL" }
    );
    // 探针写在 exe 同级 plugins/ 下；跑完清场，不留构建垃圾。
    let _ = std::fs::remove_file(&store);
    if fails.is_empty() {
        Ok(())
    } else {
        Err(format!("失败臂：{}", fails.join(",")))
    }
}

// ─────────────────────── W7：可调通道颜色端到端验收臂 ───────────────────────
//
// 正式插件 `channels`（kind=editor）走宿主真实的 start_editor → poll →
// JobOutput::Edited；按住宿 settings.json，让「参数 -> 像素」这条链真的跑一遍。
// 期望字节是硬编码的，不复用插件的公式，避免两份实现互相印证的假绿。

fn channels_settings(store: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = store.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("建 {} 失败：{e}", parent.display()))?;
    }
    std::fs::write(store, body).map_err(|e| format!("写设置失败：{e}"))
}

/// 4x2，每像素 [i, 10, 200, 255]（i=0..8）：R 递进、G/B/A 恒定。
fn channels_pixels() -> rupix_lib::core::viewer::ForPlugin {
    let px: Vec<u8> = (0..8u8).flat_map(|i| [i, 10, 200, 255]).collect();
    editor_pixels(4, 2, (0, false), px)
}

/// 硬编码的期望像素，不从插件公式推。
fn channels_expected(mode: &str) -> Vec<u8> {
    (0..8u8)
        .flat_map(|i| match mode {
            "red" => [255 - i, 10, 200, 255],
            "invert" => [255 - i, 245, 55, 255],
            _ => [i, 10, 200, 255],
        })
        .collect()
}

fn record7(label: &str, ok: bool, detail: &str, fails: &mut Vec<String>) {
    println!(
        "[w7] arm={:<15} verdict={} detail={}",
        label,
        if ok { "PASS" } else { "FAIL" },
        detail
    );
    if !ok {
        fails.push(label.to_string());
    }
}

fn run_channels_probe(dir: &Path) -> Result<(), String> {
    let manifest = Manifest::from_dir(dir)?;
    if manifest.kind != rupix_lib::core::ext::Kind::Editor {
        return Err("--channels 只接 editor 型插件".into());
    }
    let store = settings_store(&manifest.id)?;
    let base = Extension {
        manifest,
        dir: dir.to_path_buf(),
        enabled: true,
        blocked: false,
        isolated: false,
    };
    let mut fails: Vec<String> = Vec::new();

    // ① 自定义：只反相红通道，G/B 不动。
    channels_settings(
        &store,
        r#"{"preset":"custom","r_gain":"-100","r_bias":"255","g_gain":"100","b_gain":"100","g_bias":"0","b_bias":"0"}"#,
    )?;
    let a = one_edit(base.clone(), channels_pixels());
    let a_ok = matches!(&a, Ok(got) if got.rgba == channels_expected("red"));
    record7(
        "custom-red",
        a_ok,
        &format!("bytes={:?}", a.as_ref().map(|g| g.rgba.len())),
        &mut fails,
    );

    // ② 预设「反相」= 三通道 gain -100 / bias 255 这组数值，并把数值合并落盘。
    channels_settings(&store, r#"{"preset":"invert"}"#)?;
    let b = one_edit(base.clone(), channels_pixels());
    let b_px = matches!(&b, Ok(got) if got.rgba == channels_expected("invert"));
    let disk = read_store(&store);
    let b_disk = disk.contains("\"r_gain\":\"-100\"") && disk.contains("\"b_bias\":\"255\"");
    record7(
        "preset-invert",
        b_px && b_disk,
        &format!("pixels={b_px} numbers_saved={b_disk}"),
        &mut fails,
    );

    // ③ 预设「原图」：逐字节不变。
    channels_settings(&store, r#"{"preset":"original"}"#)?;
    let c = one_edit(base.clone(), channels_pixels());
    let c_ok = matches!(&c, Ok(got) if got.rgba == channels_expected("original"));
    record7(
        "preset-original",
        c_ok,
        &format!("pixels={c_ok}"),
        &mut fails,
    );

    // ④ 没有设置文件：默认自定义 = 原图，不许崩。
    let _ = std::fs::remove_file(&store);
    let d = one_edit(base.clone(), channels_pixels());
    let d_ok = matches!(&d, Ok(got) if got.rgba == channels_expected("original"));
    record7("no-settings", d_ok, &format!("pixels={d_ok}"), &mut fails);

    // ⑤ 坏数值：夹到边界后仍给出合法图像（不崩、不空、尺寸不变）。
    channels_settings(
        &store,
        r#"{"preset":"custom","r_gain":"99999","g_gain":"nope"}"#,
    )?;
    let e = one_edit(base.clone(), channels_pixels());
    let e_ok = matches!(&e, Ok(got) if got.rgba.len() == 32 && got.width == 4 && got.height == 2);
    record7("bad-numbers", e_ok, &format!("pixels={e_ok}"), &mut fails);

    // ⑥ 声明式显示层 == 插件正式回写：宿主的 op 解释器与插件公式必须逐字节一致，
    //    否则「窗里看到的」和「另存出去的」会是两张不同的图。
    channels_settings(&store, r#"{"preset":"invert"}"#)?;
    let f = one_edit(base.clone(), channels_pixels());
    let src = channels_pixels();
    let preview =
        rupix_lib::core::pixels::channel_linear(&src.rgba[..], [-100, -100, -100], [255, 255, 255]);
    let f_ok = matches!(&f, Ok(got) if got.rgba == preview);
    record7(
        "preview-parity",
        f_ok,
        &format!("bytes={}", preview.len()),
        &mut fails,
    );

    // ⑦ 声明式预设表 == 插件内建的「反相」：预设只是一组数值，宿主写进滑块后
    //    插件按同一组数算，必须逐字节一致；表里不许再留「原图」这种恒等预设。
    let inv = base
        .manifest
        .presets
        .iter()
        .find(|p| p.value == "invert")
        .map(|p| p.values);
    let g_ok = match inv {
        Some(v) => {
            let want = rupix_lib::core::pixels::channel_linear_lut(
                &channels_pixels().rgba[..],
                [v[0], v[1], v[2]],
                [v[3], v[4], v[5]],
            );
            matches!(&f, Ok(got) if got.rgba == want)
        }
        None => false,
    };
    let no_original = !base.manifest.presets.iter().any(|p| p.value == "original");
    record7(
        "preset-table",
        g_ok && no_original && base.manifest.presets.len() >= 8,
        &format!(
            "parity={g_ok} presets={} no-original={no_original}",
            base.manifest.presets.len()
        ),
        &mut fails,
    );

    println!(
        "[w7] arms=7 failed={} verdict={}",
        fails.len(),
        if fails.is_empty() { "PASS" } else { "FAIL" }
    );
    let _ = std::fs::remove_file(&store);
    if fails.is_empty() {
        Ok(())
    } else {
        Err(format!("失败臂：{}", fails.join(",")))
    }
}

fn main() -> Result<(), String> {
    // 每个 example 装一次 logger（AGENTS §日志纪律）：插件的 stdout/stderr 会被
    // 抽进宿主日志，不装 logger 就只能看到"插件没回应"，看不到它说了什么。
    rupix_lib::platform::logging::install_logging();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // W4：编辑型回写契约的端到端验收臂。用 `--editor <插件目录>` 进入，
    // 走宿主真实的 start_editor → poll → JobOutput::Edited，不复制一份协议栈。
    if argv.first().map(|s| s.as_str()) == Some("--editor") {
        let dir = PathBuf::from(
            argv.get(1)
                .ok_or("用法：plugin_smoke --editor <插件目录>")?,
        );
        return run_editor_probe(&dir);
    }
    // W5：设置通道的端到端验收臂，走真实 start_settings / start_command。
    if argv.first().map(|s| s.as_str()) == Some("--settings") {
        let dir = PathBuf::from(
            argv.get(1)
                .ok_or("用法：plugin_smoke --settings <插件目录>")?,
        );
        return run_settings_probe(&dir);
    }
    // W7：可调通道颜色的端到端验收臂，走真实 start_editor + settings 下发/回写。
    if argv.first().map(|s| s.as_str()) == Some("--channels") {
        let dir = PathBuf::from(
            argv.get(1)
                .ok_or("用法：plugin_smoke --channels <插件目录>")?,
        );
        return run_channels_probe(&dir);
    }
    let mut args = argv.into_iter();
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
