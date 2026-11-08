//! 宿主本体自更新的**编排层**：后台线程、下载、解压、进程外覆盖。
//!
//! 纯判据在 [`crate::core::update`]（离屏可测），这里只负责把活挪出 UI 线程并把结果
//! 送回 `global_tick`。一次只允许一条任务在跑（`Slot::phase` 的任务位），不会叠线程。
//!
//! ## 流程
//!
//! 「关于」页点检查 → `start_check_urls` 在后台拉 `update.json`（双源、成功才缓存）→
//! `Event::Available` 弹说明窗（`ui::update_window`）→ 用户点「更新」→
//! `start_download` 后台下 ZIP → 按「文件名 + 版本号」两条判据验收 →
//! 解压到 `<exe>/temp/update-stage/` → 写 `BAT_NAME` → `Event::Ready` →
//! `launch` 起脚本、`ctx.quit_app()` 退出 → 脚本等进程消失后覆盖 4 个白名单文件并重启。
//!
//! ## 界面反馈（2026-11-08 对齐 CurSeen，别再改回 toast）
//!
//! 一切反馈都写进关于页那 4 枚信号（状态行 / 进度 / 忙 / 下载中），见 `apply`：
//! 检查与下载的进展写在卡的**状态行**，下载时「检查更新」原地让位给**进度条**。
//! **不许**再往主窗口弹提示——用户明确说过那位置是错的。
//!
//! **不算哈希、不比文件大小、不比下载字节数**（用户 2026-11-07 口径）。
//! 响应体 / 解包量上限只是防内存爆的护栏，不与任何期望值比对。

use std::cell::RefCell;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use windui::core::EventCtx;

use crate::core::config::Config;
use crate::core::paths;
use crate::core::update::{self, Plan, Version};
use crate::ui::settings::Hub;
use crate::ui::update_state::UpdateState;

/// 进程外覆盖脚本名（放在 exe 同级，成功后自删）。
pub const BAT_NAME: &str = "rupix_update.bat";
/// 下载暂存与解压目录名（都在 `<exe>/temp/` 下，脚本用 `%~dp0` 推导，不把中文路径写进文本）。
const ZIP_NAME: &str = "rupix-update.zip";
const STAGE_NAME: &str = "update-stage";
/// 后台 → UI 回投队列上限（总纲 §6.4 / §6.10）：满了丢中间进度帧，终态永不丢。
const MAX_EVENTS: usize = 8;
/// 下载进度回投节流（总纲 §16 数字表：50~100 ms 一次足够）。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// 正在跑的那条任务。带原因才能给出「正在检查」/「正在下载」的区别：只记一个
/// `busy` 布尔时，用户第二次点只会看到一句「任务没能启动」，读起来像程序坏了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// 拉清单：最坏情况是两个地址各 30 s 超时。
    Check,
    /// 下载 + 解压：空壳包只有 2~3 MiB，给足余量。
    Download,
}

impl Phase {
    /// 抢占失败时**直接给用户看**的一句话。
    fn label(self) -> &'static str {
        match self {
            Phase::Check => "正在检查更新，请稍候",
            Phase::Download => "正在下载更新，请稍候",
        }
    }

    /// 超过这个时长还没回投终态 = 后台线程已经失联（HTTP 四阶段各 30 s，两条地址
    /// 也走不完 90 s），届时把任务位收回，否则「检查更新」在本进程里就永久废了。
    fn budget(self) -> Duration {
        match self {
            Phase::Check => Duration::from_secs(90),
            Phase::Download => Duration::from_secs(600),
        }
    }
}

struct Slot {
    phase: Option<Phase>,
    /// 任务位的到期时刻；`Phase::budget` 用尽即视为线程失联。
    deadline: Option<Instant>,
    events: Vec<Event>,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot {
    phase: None,
    deadline: None,
    events: Vec::new(),
});

/// 后台线程送回 UI 的结果（文案映射见 `apply`，逐条对齐 CurSeen）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 任务位刚抢到、线程还没起：状态行立刻换成「正在检查更新…」。
    CheckStarted,
    UpToDate(String),
    Available(Plan),
    CheckFailed(String),
    /// 下载线程已经起来：关于页的按钮立刻让位给进度条。
    DownloadStarted,
    DownloadProgress {
        received: u64,
        total: u64,
    },
    DownloadFailed(String),
    /// staging 与 bat 都已就位：起脚本、退出进程。
    Ready,
}

fn lock() -> std::sync::MutexGuard<'static, Slot> {
    SLOT.lock().unwrap_or_else(|e| e.into_inner())
}

/// 中间帧（开始 / 进度）：不动任务位，队列满了就丢。
fn send(ev: Event) {
    let mut g = lock();
    bounded_push(&mut g.events, ev);
}

/// 终态：收回任务位，并且保证一定进得了队列。
fn finish(ev: Event) {
    let mut g = lock();
    g.phase = None;
    g.deadline = None;
    bounded_push(&mut g.events, ev);
}

/// 线程都没起来：任务位立刻放回空闲，调用方把具体原因交给 `poll` 弹提示。
fn idle() {
    let mut g = lock();
    g.phase = None;
    g.deadline = None;
}

/// 有界追加（纯函数，单测可无头钉住）。进度帧可丢，终态把中间帧全顶掉也要进队。
fn bounded_push(events: &mut Vec<Event>, ev: Event) -> bool {
    let final_ev = matches!(
        ev,
        Event::UpToDate(_)
            | Event::Available(_)
            | Event::CheckFailed(_)
            | Event::DownloadFailed(_)
            | Event::Ready
    );
    if events.len() >= MAX_EVENTS {
        if !final_ev {
            return false;
        }
        events.clear();
    }
    events.push(ev);
    true
}

/// 抢占任务位。`Err` 里那句话是**可以直接 toast 的**（区分「正在检查」/「正在下载」）。
fn begin(phase: Phase) -> Result<(), String> {
    let mut g = lock();
    if let Some(cur) = g.phase {
        let expired = g.deadline.is_none_or(|d| Instant::now() >= d);
        if !expired {
            return Err(cur.label().to_string());
        }
        // 上一次的线程没有回投终态（起不来 / 卡在网络上）：明说一句再放行新任务。
        log::warn!("上一次更新任务超时未回投（{cur:?}），回收任务位");
    }
    g.phase = Some(phase);
    g.deadline = Some(Instant::now() + phase.budget());
    Ok(())
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), String> {
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map(|_| ())
        .map_err(|e| format!("起不来后台更新线程：{e}"))
}

/// 取走全部待处理事件（由 `global_tick` 每帧调）。
///
/// ★ 这里**一个主窗口 toast 都不弹**：更新的一切反馈都写进关于页那 4 枚信号
/// （状态行 / 进度 / 忙 / 下载中），与 CurSeen 同一条通道（用户 2026-11-08 点名）。
pub fn poll(ctx: &mut EventCtx, config: &Rc<RefCell<Config>>, hub: &Hub) {
    // UI 只往 core 投一个空请求；地址一律从 Config（ini 的唯一正本）读，
    // 两个 update_index_* 键因此有真实的行为读者，不会变成第二份正本。
    if let Some(req) = update::take_request() {
        let r = match req {
            update::Request::Check => {
                let c = config.borrow();
                let urls = [c.update_index_gitee.clone(), c.update_index_github.clone()];
                start_check_urls(&urls, c.download_source as usize)
            }
            update::Request::Download(p) => start_download(&p),
        };
        if let Err(e) = r {
            log::warn!("发起更新任务失败：{e}");
            set_status(hub, e);
        }
    }
    let events: Vec<Event> = {
        let mut g = lock();
        std::mem::take(&mut g.events)
    };
    for ev in events {
        apply(ctx, hub, ev);
    }
}

/// 关于页那 4 枚信号；设置页还没打开过时是 `None`（没处显示，只留日志）。
fn signals(hub: &Hub) -> Option<UpdateState> {
    hub.borrow().as_ref().map(|h| h.update)
}

fn set_status(hub: &Hub, text: impl Into<String>) {
    if let Some(up) = signals(hub) {
        up.status.set(text.into());
    }
}

/// 把一条后台结果落到信号上。文案逐条对齐 CurSeen 的 `apply_update_event`。
fn apply(ctx: &mut EventCtx, hub: &Hub, ev: Event) {
    let up = signals(hub);
    match ev {
        Event::CheckStarted => {
            if let Some(up) = up {
                up.busy.set(true);
                up.downloading.set(false);
                up.progress.set(-1.0);
                up.status.set("正在检查更新…".to_string());
            }
        }
        Event::UpToDate(latest) => {
            log::info!("检查更新：已是最新 {latest}");
            if let Some(up) = up {
                up.busy.set(false);
                up.downloading.set(false);
                up.status.set(format!("已是最新版本 {latest}"));
            }
        }
        Event::Available(p) => {
            log::info!("检查更新：发现新版本 {}", p.version);
            if let Some(up) = up {
                up.busy.set(false);
                up.downloading.set(false);
                up.status
                    .set(format!("发现新版本 {}，已打开更新说明", p.version));
            }
            crate::ui::update_window::open(ctx, &p);
        }
        Event::CheckFailed(e) => {
            log::warn!("检查更新失败：{e}");
            if let Some(up) = up {
                up.busy.set(false);
                up.downloading.set(false);
                up.status.set(format!("检查失败：{e}"));
            }
        }
        Event::DownloadStarted => {
            // 点下「更新」后的第一帧反馈：按钮立刻换成进度条，不必等网络。
            if let Some(up) = up {
                up.busy.set(true);
                up.downloading.set(true);
                up.progress.set(0.0);
                up.status.set("正在下载… 0%".to_string());
            }
        }
        Event::DownloadProgress { received, total } => {
            // `total == 0`：服务端没给长度，只报已下载的量，进度条走不确定态。
            if let Some(up) = up {
                if total == 0 {
                    up.progress.set(-1.0);
                    up.status
                        .set(format!("正在下载… {:.1} MB", received as f64 / 1_048_576.0));
                } else {
                    let p = (received as f32 / total as f32).clamp(0.0, 1.0);
                    up.progress.set(p);
                    up.status
                        .set(format!("正在下载… {}%", (p * 100.0).round() as i32));
                }
            }
        }
        Event::DownloadFailed(e) => {
            log::warn!("下载更新失败：{e}");
            if let Some(up) = up {
                up.busy.set(false);
                up.downloading.set(false);
                up.progress.set(-1.0);
                up.status.set(format!("下载失败：{e}"));
            }
        }
        Event::Ready => {
            if let Some(up) = up {
                up.progress.set(1.0);
                up.busy.set(true);
                up.status.set("下载完成，正在重启以完成更新…".to_string());
            }
            match launch() {
                Ok(()) => {
                    if let Some(up) = up {
                        up.status.set("更新脚本已启动，程序即将退出…".to_string());
                    }
                    ctx.quit_app();
                }
                Err(e) => {
                    log::warn!("启动更新脚本失败：{e}");
                    if let Some(up) = up {
                        up.busy.set(false);
                        up.downloading.set(false);
                        up.status.set(format!("启动更新脚本失败：{e}"));
                    }
                }
            }
        }
    }
}

/// 内部入口：地址已由 UI 从 ini 读出、经 `core::update::Request` 投递过来。
fn start_check_urls(urls: &[String; 2], source: usize) -> Result<(), String> {
    begin(Phase::Check)?;
    send(Event::CheckStarted);
    let first = source.min(1);
    let ordered = [urls[first].clone(), urls[1 - first].clone()];
    let spawned = spawn("rupix-update-check", move || {
        // 起线程先落一行：这条日志能把「线程没起来」和「卡在 HTTP」分开，
        // 上一轮只有一句「任务没能启动」时，两种原因在日志里长得一模一样。
        log::info!("检查更新：{}（备用 {}）", ordered[0], ordered[1]);
        let result = check_job(&ordered, first);
        finish(match result {
            Ok(update::Outcome::UpToDate { latest }) => Event::UpToDate(latest),
            Ok(update::Outcome::Available(p)) => Event::Available(p),
            Err(e) => Event::CheckFailed(e),
        });
    });
    if let Err(e) = spawned {
        idle();
        log::warn!("起不来检查线程：{e}");
        return Err("更新任务没能启动，请稍后重试".into());
    }
    Ok(())
}

fn check_job(urls: &[String; 2], source: usize) -> Result<update::Outcome, String> {
    let mut last = "没有可用的更新清单地址".to_string();
    for url in urls.iter().filter(|u| !u.is_empty()) {
        if !update::is_safe_manifest_url(url) {
            last = "更新清单地址不是官方仓库".into();
            log::warn!("拒绝非官方更新清单地址：{url}");
            continue;
        }
        match crate::platform::http::get(url, update::MANIFEST_MAX_BYTES)
            .map_err(|e| e.to_string())
            .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
            .and_then(|t| update::parse_manifest(&t).map(|m| (m, t)))
        {
            Ok((m, text)) => {
                update::save_cached(&text);
                return update::plan(&m, Version::current(), source);
            }
            Err(e) => {
                log::warn!("拉取更新清单失败：{e}");
                last = e;
            }
        }
    }
    // 断网时用上一次成功拉到的原文判「已是最新」，不把离线当故障。
    if let Some(text) = update::load_cached() {
        if let Ok(m) = update::parse_manifest(&text) {
            log::info!("更新清单联网失败，使用本地缓存 {}", m.version);
            return update::plan(&m, Version::current(), source);
        }
    }
    Err(last)
}

/// 用户点说明窗里的「更新」后才下载。首选源失败自动换备用源。
pub fn start_download(p: &Plan) -> Result<(), String> {
    begin(Phase::Download)?;
    send(Event::DownloadStarted);
    let plan = p.clone();
    let spawned = spawn("rupix-update-fetch", move || {
        log::info!("开始下载更新包 {}：{}", plan.version, plan.urls[0]);
        match download_job(&plan) {
            Ok(()) => finish(Event::Ready),
            Err(e) => finish(Event::DownloadFailed(e)),
        }
    });
    if let Err(e) = spawned {
        idle();
        log::warn!("起不来下载线程：{e}");
        return Err("更新任务没能启动，请稍后重试".into());
    }
    Ok(())
}

fn temp_dir() -> Result<PathBuf, String> {
    paths::diagnostics_dir().ok_or_else(|| "拿不到便携目录".to_string())
}

fn download_job(p: &Plan) -> Result<(), String> {
    let temp = temp_dir()?;
    let stage = temp.join(STAGE_NAME);
    let zip_path = temp.join(ZIP_NAME);
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    fs::create_dir_all(&stage).map_err(|e| format!("建解压目录失败：{e}"))?;

    send(Event::DownloadStarted);
    let mut bytes: Option<Vec<u8>> = None;
    let mut errors = Vec::new();
    for url in [&p.urls[0], &p.urls[1]]
        .into_iter()
        .filter(|u| !u.is_empty())
    {
        if let Err(e) = update::check_asset_url(url, &p.version) {
            errors.push(e);
            continue;
        }
        // 进度回投 50 ms 节流：既不把 UI 队列灌满，也不让下载期间界面零反馈。
        let mut last = Instant::now();
        let result = crate::platform::http::get_with_progress_total(
            url,
            update::PACKAGE_MAX_BYTES,
            |got, total| {
                let now = Instant::now();
                if now.duration_since(last) >= PROGRESS_INTERVAL {
                    last = now;
                    send(Event::DownloadProgress {
                        received: got,
                        total,
                    });
                }
            },
        );
        match result {
            Ok(b) => {
                bytes = Some(b);
                break;
            }
            Err(e) => errors.push(e),
        }
    }
    let bytes = bytes.ok_or_else(|| {
        let e = errors.join("；");
        log::warn!("更新包下载失败：{e}");
        "下载更新包失败，请检查网络或稍后重试".to_string()
    })?;

    // 落 .part → 改名，半截下载绝不留下完整名字。
    let part = temp.join(format!("{ZIP_NAME}.part"));
    fs::write(&part, &bytes).map_err(|e| format!("写更新包失败：{e}"))?;
    fs::rename(&part, &zip_path).map_err(|e| format!("更新包改名失败：{e}"))?;

    // 两条判据（文件名 + 版本号）在解压之前跑完，不过就一个现有文件都不动。
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes))
        .map_err(|e| format!("更新包不是有效的 ZIP：{e}"))?;
    let mut names = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let name = archive
            .by_index(i)
            .map_err(|e| format!("读取 ZIP 第 {} 项失败：{e}", i + 1))?
            .name()
            .to_string();
        names.push(name);
    }
    let files = update::check_archive(&names, &p.version)?;

    let mut total = 0usize;
    for (name, target) in files {
        let mut entry = archive
            .by_name(&name)
            .map_err(|e| format!("解压 {target} 失败：{e}"))?;
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("读取 {target} 失败：{e}"))?;
        total = total.saturating_add(buf.len());
        if total > update::PACKAGE_MAX_BYTES {
            return Err("更新包解包量超过上限".into());
        }
        fs::write(stage.join(target), buf).map_err(|e| format!("落盘 {target} 失败：{e}"))?;
    }

    let app = paths::exe_dir().ok_or_else(|| "拿不到程序目录".to_string())?;
    let bat = app.join(BAT_NAME);
    fs::write(&bat, bat_script().as_bytes()).map_err(|e| format!("写更新脚本失败：{e}"))?;
    log::info!("更新包已就位：{}，版本 {}", stage.display(), p.version);
    Ok(())
}

/// 进程外覆盖脚本（纯函数，单测逐行可查）。
///
/// 全 ASCII 注释、UTF-8 无 BOM、CRLF；**所有路径从 `%~dp0` 推导**，中文 / 空格 /
/// 感叹号路径都不写进脚本文本，也就不需要延迟展开。只覆盖四个白名单文件，
/// 旧 exe 先复制成 `.old`；失败时保留 `.old` 与 zip 并启动旧程序。
pub fn bat_script() -> String {
    let raw = r#"@echo off
chcp 65001 >nul
setlocal
set "APP=%~dp0"
set "STAGE=%APP%temp\update-stage"
set "ZIP=%APP%temp\rupix-update.zip"
set "LOG=%APP%temp\rupix-update.log"
call :main >> "%LOG%" 2>&1
exit /b

:main
echo [%DATE% %TIME%] RuPix updater start
REM 1. Wait for the host to exit by itself. Force-kill is the last resort only.
set "N=0"
:wait
tasklist /fi "ImageName eq rupix.exe" 2>nul | find "rupix.exe" >nul
if errorlevel 1 goto replace
set /a N+=1
if %N% GEQ 15 goto force
ping -n 2 127.0.0.1 >nul 2>nul
goto wait
:force
taskkill /f /im rupix.exe >nul 2>nul
ping -n 3 127.0.0.1 >nul 2>nul

:replace
REM 2. The package must contain the main executable, otherwise change nothing.
if not exist "%STAGE%\rupix.exe" goto fail
if not exist "%APP%rupix.exe" goto fail
copy /y "%APP%rupix.exe" "%APP%rupix.exe.old" >nul 2>nul
for %%F in (rupix.exe LICENSE THIRD-PARTY-NOTICES.md README.txt) do (
  if exist "%STAGE%\%%F" copy /y "%STAGE%\%%F" "%APP%\%%F" >nul 2>nul
)
if not exist "%APP%rupix.exe" goto fail
echo [%DATE% %TIME%] files replaced
start "" /d "%APP%" "%APP%rupix.exe"
rmdir /s /q "%STAGE%" 2>nul
del /q "%ZIP%" 2>nul
echo [%DATE% %TIME%] done
ping -n 2 127.0.0.1 >nul 2>nul
del /q "%~f0" >nul 2>nul
exit /b 0

:fail
echo [%DATE% %TIME%] FAILED - kept the old executable and the zip
start "" /d "%APP%" "%APP%rupix.exe"
exit /b 1
"#;
    raw.replace('\n', "\r\n")
}

/// 起脚本。调用方随后立刻 `ctx.quit_app()`；脚本第一步就是等我们消失。
pub fn launch() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let app: &Path = paths::exe_dir().ok_or_else(|| "拿不到程序目录".to_string())?;
    let bat = app.join(BAT_NAME);
    if !bat.is_file() {
        return Err("更新脚本不存在".into());
    }
    std::process::Command::new("cmd")
        .arg("/C")
        .arg(&bat)
        .current_dir(app)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("启动更新脚本失败：{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_queue_drops_progress_but_never_a_terminal_event() {
        let mut q: Vec<Event> = Vec::new();
        for i in 0..(MAX_EVENTS as u64 * 4) {
            bounded_push(
                &mut q,
                Event::DownloadProgress {
                    received: i,
                    total: 0,
                },
            );
        }
        assert_eq!(q.len(), MAX_EVENTS, "中间帧必须被有界丢弃");
        assert!(q.iter().all(|e| matches!(
            e,
            Event::DownloadProgress {
                received: _,
                total: _
            }
        )));
        assert!(bounded_push(&mut q, Event::Ready), "终态不许被丢");
        assert_eq!(q, vec![Event::Ready], "终态顶掉全部中间帧");
    }

    #[test]
    fn progress_throttle_stays_in_the_50_to_100ms_band() {
        // black_box 挡住编译期折叠：这条是真的在测常量取值，不是 const 断言。
        let ms = std::hint::black_box(PROGRESS_INTERVAL).as_millis();
        assert!((50..=100).contains(&ms), "节流区间 {ms} ms 越界");
    }

    #[test]
    fn phase_budget_covers_two_http_timeouts_and_labels_differ() {
        // black_box 挡住编译期折叠：这里测的是取值，不是常量表达式。
        let check = std::hint::black_box(Phase::Check.budget()).as_secs();
        let download = std::hint::black_box(Phase::Download.budget()).as_secs();
        // 清单最坏要走两个 30 s 超时：任务位必须活得比它久，否则会把还在跑线程判成失联。
        assert!(check >= 90, "检查任务位 {check} s 盖不住两个 30 s 超时");
        assert!(download > check, "下载任务位不能比检查还短");
        assert_ne!(
            Phase::Check.label(),
            Phase::Download.label(),
            "两种任务必须给出可区分的一句话，否则用户还是看不懂"
        );
    }

    #[test]
    fn bat_is_ascii_crlf_and_never_embeds_a_machine_path() {
        let s = bat_script();
        assert!(s.is_ascii(), "脚本正文必须全 ASCII");
        assert!(s.contains("\r\n"), "cmd 对 LF-only 的 goto 标签容错很差");
        assert!(!s.contains("\n\n"), "CRLF 转换不能留下裸 LF");
        assert!(s.contains("%~dp0"), "路径一律从脚本自身目录推导");
        assert!(!s.contains(":\\"), "不许把盘符绝对路径写进脚本");
        assert!(s.contains("rupix.exe.old"), "覆盖前要备份旧 exe");
        assert!(s.contains("goto fail"), "失败分支必须存在");
    }

    #[test]
    fn bat_only_touches_the_four_whitelisted_names() {
        let s = bat_script();
        for name in update::PACKAGE_FILES {
            assert!(s.contains(name), "白名单文件 {name} 没进覆盖清单");
        }
        assert!(!s.contains("robocopy"), "不许整目录同步");
        assert!(!s.contains("/e"), "不许 robocopy 的 /e 语义");
    }
}
