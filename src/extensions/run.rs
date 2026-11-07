//! 插件调用编排：起进程 → 建管道 → 发请求 → 收响应 → 收尾。
//!
//! 三种任务：
//! - **命令型**（`kind=command`）：拿当前图片路径/参数，回传文本，进结果窗。
//! - **编辑型**（`kind=editor`）：宿主把当前解码档写进共享内存，插件开独立窗口
//!   编辑，回传同尺寸 RGBA（共享内存），宿主套成"已修改"覆盖（另存/还原在插件菜单）。
//!   编辑是交互任务：响应**无超时**（用户画多久都行），取消走插件菜单的「取消」。
//! - **设置型**（manifest 声明 `settings_ui` 才有，见 `run_settings`）：插件自己画设置窗，
//!   同样不设响应超时；回传的 `settings` 由宿主合并落盘。
//!
//! 所有阻塞都发生在后台线程，UI 只通过 [`poll`] 取结果并弹提示。
//! 正在运行的任务以 manifest id 为键登记在 [`RUNNING`]，右键菜单据此把条目换成
//! 「取消」；超时与取消都会终止进程并打断阻塞中的 accept，不留悬挂线程或句柄。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use crate::core::ext::{Capability, Kind};
use crate::core::ext_protocol::{ImageRef, Request, Response, Selection};
use crate::core::viewer::ForPlugin;
use crate::extensions::Extension;
use crate::platform::{job, pipe, process, shared_mem};

/// 等插件连上管道的上限。
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);
/// 收到响应后给进程自然退出的时间；到点仍活着就终止。
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// 送插件的整图像素上限（8MP ≈ 3200×2500）：**编辑型与命令型共用同一根闸**。
/// 双向共享内存各 32 MB 封顶，鸟瞰重采也只在这一档做。更大的图先放大到原图档再说
/// （UI 会置灰并说明）。
///
/// ★ m166 第一格起命令型也能收像素，所以这条不再只属于"编辑"。名字留着不改
///   是因为它已出现在两份文档与灰项文案的推导里，改一处漏一处就是第二正本。
pub const EDITOR_MAX_PIXELS: u64 = 8_000_000;

/// 这一档整图能不能送插件（**上限判据只有这一处**：编辑型的拦截、命令型的像素分流、
/// 菜单灰项的文案都读它）。以前那三处各写一遍 `w*h <= EDITOR_MAX_PIXELS`——三份"同一个数"，
/// 改闸时必漏一处，最坏的结果是灰项还写着"≤8MP"而闸已经变了数（菜单撒谎）。
///
/// 零宽 / 零高算**不合法**（不是"0 像素当然在上限内"）：编辑型一直当场拒掉空图，
/// 命令型接像素之后必须一样，否则空图会白建一块 0 字节的映射还发出去。
pub fn pixels_within_limit(w: u32, h: u32) -> bool {
    w != 0 && h != 0 && (w as u64) * (h as u64) <= EDITOR_MAX_PIXELS
}

/// 这一次真正送出去的像素 + 它与原图的关系（请求里的 `source` / `sample_step` 就读它）。
pub struct Outgoing {
    pub px: ForPlugin,
    /// 每 N 像素取 1 点；`1` = 原样送出（未抽样）。
    pub step: u32,
}

/// 这次调用该不该送像素、送**哪一份**。
///
/// 判据分三段，顺序不能换：
/// 1. 没声明 `image.read` ⇒ 一律不发（能力最小化，顺手多发点在这条线上就是越权，
///    而且那是一次最多 32 MB 的映射写入）；
/// 2. 整图在 [`EDITOR_MAX_PIXELS`] 内 ⇒ **原样发**，行为与 m166 那天完全一致；
/// 3. 超上限且声明了 [`crate::core::ext::Capability::ImageSampled`] ⇒ 等距**抽样**后发。
///
/// ★ 第 3 段为什么是声明制、而不是对所有命令型放开：条码那类插件在抽样图上窄条被
///   跳掉，结局从"超限"变成"扫不出来"，**比不发给不出更误导的答案**。抽样只对"要分布、
///   不要每个像素"的统计型插件成立，这件事只有插件自己知道。
///
/// ★ 抽样而不是缩放：面积平均会把 0 混成 3、255 混成 252，暗部/高光溢出的计数随之失真；
///   等距抽样只挑不混，每个样本都是原图上真实存在的原始字节。
fn pixels_to_send(
    declared: bool,
    accepts_sampled: bool,
    image: Option<ForPlugin>,
) -> Option<Outgoing> {
    let px = image.filter(|_| declared)?;
    if pixels_within_limit(px.width, px.height) {
        return Some(Outgoing { px, step: 1 });
    }
    if !accepts_sampled {
        return None;
    }
    let step = crate::core::pixels::plugin_sample_step(px.width, px.height, EDITOR_MAX_PIXELS);
    let (w, h, rgba) = crate::core::pixels::sample_strided(&px.rgba, px.width, px.height, step);
    // 抽完没有变小 / 抽出了空图 ⇒ 退回"不发"，不发一块假图。
    if step <= 1
        || w == 0
        || h == 0
        || (w as u64) * (h as u64) >= (px.width as u64) * (px.height as u64)
    {
        return None;
    }
    Some(Outgoing {
        px: ForPlugin {
            width: w,
            height: h,
            rgba: Arc::new(rgba),
            ..px
        },
        step,
    })
}

/// 一次后台调用的产出。
#[derive(Debug, Clone)]
pub enum JobOutput {
    /// 命令型：文本结果（进结果窗）。
    Text {
        text: String,
        /// 插件声明了 `clipboard.write` → 宿主已把文本写进系统剪贴板。
        /// UI 靠它把"复制"按钮换成"已复制"，不让用户对着空按钮猜。
        copied: bool,
        /// 插件声明了 `open.url` 且勾了"解码后直接打开网址" → 交给系统默认浏览器。
        open_url: Option<String>,
        /// 插件声明了 `open.file` 且勾了"解码后直接打开图片" → 字节（宿主落 temp 再开）。
        open_image: Option<(String, Vec<u8>)>,
    },
    /// 设置型：插件自己的设置窗口关掉了，回传的设置已合并落盘。
    ///
    /// 不进结果窗、也不弹提示：窗口开与关都是用户自己看见的事（提示纪律）。
    Settings,
    /// 编辑型：回传的整档 RGBA（尺寸与送入一致；进 viewer 编辑覆盖）。
    ///
    /// `orient` 是**送入那一刻的屏上朝向** `(rot, flip)`：插件进出两端都是"用户看见的那一张"，
    /// 落地要按这一次冻结值转回源朝向（此刻主窗口可能已经被再转过一次，读 `self.rot` 就错了）。
    Edited {
        path: PathBuf,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        orient: (u8, bool),
    },
}

/// 一次后台调用的结果。
#[derive(Debug, Clone)]
pub struct JobOutcome {
    pub plugin: String,
    pub id: String,
    /// 编辑型的源文件路径（命令型 None）。
    pub target: Option<PathBuf>,
    pub result: Result<JobOutput, String>,
}

static JOBS: OnceLock<Mutex<Vec<JobOutcome>>> = OnceLock::new();
static SEQ: AtomicU32 = AtomicU32::new(0);

/// 在跑的一次调用。取消必须能从 UI 线程摸到子进程与 accept worker，
/// 所以它们登记在表里，而不是留在 run_* 的栈上。
struct JobHandle {
    child: Mutex<Option<process::Child>>,
    /// 还阻塞在 `ConnectNamedPipe` 时的线程句柄；取消用它打断。
    accept_worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 进程组兜底：宿主异常退出时内核杀整组子进程（KILL_ON_JOB_CLOSE）。
    job: Mutex<Option<job::Job>>,
    cancelled: AtomicBool,
}

static RUNNING: OnceLock<Mutex<HashMap<String, Arc<JobHandle>>>> = OnceLock::new();

fn running() -> &'static Mutex<HashMap<String, Arc<JobHandle>>> {
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 锁中毒恢复成原值：编排路径不值得为一次 panic 整条链路报错。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 同一插件同时只允许一个实例：管道名带序号本可并存，但「菜单条目变取消」
/// 以 id 为键，并存会把 UI 状态搞乱。
fn register(id: &str) -> Result<Arc<JobHandle>, String> {
    let mut g = lock(running());
    if g.contains_key(id) {
        return Err(format!("插件 {id} 正在运行，请等它结束或先取消"));
    }
    let job = Arc::new(JobHandle {
        child: Mutex::new(None),
        accept_worker: Mutex::new(None),
        job: Mutex::new(None),
        cancelled: AtomicBool::new(false),
    });
    g.insert(id.to_string(), job.clone());
    Ok(job)
}

fn deregister(id: &str, job: &Arc<JobHandle>) {
    let mut g = lock(running());
    // 取消允许立即重启：旧任务迟到的收尾只能摘掉自己的登记。
    if g.get(id).is_some_and(|current| Arc::ptr_eq(current, job)) {
        g.remove(id);
    }
}

/// 某个插件是否正在运行（右键菜单据此把条目换成「取消」）。
pub fn is_running(id: &str) -> bool {
    lock(running()).contains_key(id)
}

fn kill_child(job: &JobHandle) {
    if let Some(c) = lock(&job.child).as_mut() {
        // terminate 幂等；取消过的收尾再走一遍无害。
        let _ = c.terminate();
    }
    // Job 一起终结：孙进程（插件自己 spawn 的）也一并带走。
    if let Some(j) = lock(&job.job).as_ref() {
        j.terminate();
    }
}

fn cancelled(job: &JobHandle) -> bool {
    job.cancelled.load(Ordering::SeqCst)
}

/// 用户取消：置停止位 → 打断阻塞中的 accept（worker 的 Drop 关句柄）→
/// 杀整组进程。返回是否真的取消了任务。
pub fn cancel(id: &str) -> bool {
    let Some(job) = lock(running()).remove(id) else {
        return false;
    };
    job.cancelled.store(true, Ordering::SeqCst);
    if let Some(w) = lock(&job.accept_worker).take() {
        pipe::cancel_synchronous_io(&w);
    }
    kill_child(&job);
    true
}

/// 三振计数（同 id 连续运行期失败）；成功清零，用户取消不动它。
static STRIKES: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
/// 达到三振、等 runtime 拿去写 `extensions_isolated` 的队列。
static ISOLATIONS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// 连续失败几次后隔离。三次 = 每次都能容忍网络抖动/偶发失败，连续三次
/// 则几乎必然是坏包（安装损坏/依赖缺失），继续让用户反复点只会反复看到同样的错。
pub const STRIKE_LIMIT: u32 = 3;

fn strikes() -> &'static Mutex<HashMap<String, u32>> {
    STRIKES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn isolations() -> &'static Mutex<Vec<String>> {
    ISOLATIONS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 记一次结果：成功清零；取消不计；其余累计，达到 [`STRIKE_LIMIT`] 推入隔离队列。
/// 返回 = 本次是否触发隔离。
/// 一次调用的失败，带一条**要不要记账**的位。
///
/// ★ 为什么失败要分类：插件按协议回 `ok:false`（"这图我不处理 / 没有整图"）是**正常答复**，
///   它的设计就是拒绝，宿主却把它和"崩溃、超时、协议坏"一起记三振，于是连点三次就把一枚
///   完全健康的插件推进隔离队列——2026-11-07 真机报障的正是这个。`strike=false` 的那一类
///   只出提示，不动隔离计数。
pub struct Failure {
    pub text: String,
    pub strike: bool,
}

impl Failure {
    /// 插件自己的答复（`ok:false`）：说清楚，但不算它坏。
    fn reported(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            strike: false,
        }
    }
}

impl From<String> for Failure {
    fn from(text: String) -> Self {
        Self { text, strike: true }
    }
}

impl From<&str> for Failure {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_string(),
            strike: true,
        }
    }
}

/// 记一次结果。`counts=false` 的那类失败**不进三振计数**（插件按协议拒绝，不是它坏了）。
fn record_outcome(id: &str, ok: bool, cancelled: bool, counts: bool) -> bool {
    if ok {
        lock(strikes()).insert(id.to_string(), 0);
        return false;
    }
    if cancelled || !counts {
        return false;
    }
    let n = {
        let mut g = lock(strikes());
        let n = g.entry(id.to_string()).and_modify(|c| *c += 1).or_insert(1);
        *n
    };
    if n >= STRIKE_LIMIT {
        lock(isolations()).push(id.to_string());
        lock(strikes()).remove(id);
        return true;
    }
    false
}

/// runtime 每轮取走刚触发隔离的插件 id（写 `extensions_isolated` + 提示）。
pub fn take_isolations() -> Vec<String> {
    std::mem::take(&mut *lock(isolations()))
}

/// 启动前的公共门禁：启停/隔离/版本/入口 + 能力最小化的图片入口检查。
///
/// 2026-10-29 用户口径：**安装即视为同意**，不再有首用授权检查。
fn gate(ext: &Extension, wants_image: bool) -> Result<(), String> {
    if !ext.enabled {
        return Err(format!("插件 {} 已停用", ext.manifest.id));
    }
    if ext.isolated {
        return Err(format!(
            "插件 {} 连续失败已隔离，请到设置→插件里解除",
            ext.manifest.name
        ));
    }
    if ext.blocked {
        return Err(format!(
            "插件 {} 需要宿主 {} 及以上",
            ext.manifest.name, ext.manifest.min_host
        ));
    }
    if !ext.dir.join(&ext.manifest.entry).is_file() {
        return Err(format!(
            "插件入口不存在：{}",
            ext.dir.join(&ext.manifest.entry).display()
        ));
    }
    // 能力最小化（docs/扩展体系.md §四）：宿主只为声明过的能力开入口，
    // 未声明 image.read 的插件拿不到图片路径/共享内存。
    if wants_image && !ext.manifest.has_capability(Capability::ImageRead) {
        return Err(format!(
            "插件 {} 未声明 image.read，不提供图片入口",
            ext.manifest.name
        ));
    }
    Ok(())
}

/// 一行定论：这次到底发没发像素、发了多少、为什么没发。
///
/// ★ 这是上一轮查案的直接产物：日志里只剩一句「插件连续失败 3 次，已推入隔离队列」，
///   而"像素根本没发"这件事**没有任何一行记录**——最后只能靠时间线反推。有了这一行，
///   下一次同类问题在日志的第一眼就能分清「没声明 / 没图 / 超闸没接抽样」。
fn log_pixels(
    id: &str,
    declared: bool,
    accepts_sampled: bool,
    had_image: bool,
    out: Option<&Outgoing>,
) {
    match out {
        Some(o) => log::info!(
            "[plugin-pixels] id={id} sent={}x{} step={} source={}x{}",
            o.px.width,
            o.px.height,
            o.step,
            o.px.natural.0,
            o.px.natural.1
        ),
        None => {
            let reason = if !declared {
                "no-read"
            } else if !had_image {
                "no-image"
            } else {
                "over-cap"
            };
            log::info!(
                "[plugin-pixels] id={id} sent=none reason={reason} \
                 declared={declared} accepts_sampled={accepts_sampled}"
            );
        }
    }
}

/// 在后台启动一次命令插件调用。立即返回，结果用 [`poll`] 取。
///
/// `image` 是**屏上朝向**的当前整图（[`Viewer::whole_image_for_plugin`] 那一家的返回值，
/// 类型见 [`ForPlugin`]）：声明了 `image.read` 的**命令型**插件从此也能收到像素，不必自己
/// 再去解文件——这正对应用户那句"只要能解开的都能体检 / 都能扫码"。拿不到整图（切片态）
/// 或超过 [`EDITOR_MAX_PIXELS`] 时**照旧只发路径**：老插件的行为因此是子集，不会突然变坏。
pub fn start_command(
    ext: Extension,
    image_path: Option<PathBuf>,
    selection: Option<Selection>,
    image: Option<ForPlugin>,
) -> Result<(), String> {
    if ext.manifest.kind != Kind::Command {
        return Err("该插件不是命令型".into());
    }
    gate(&ext, image_path.is_some())?;
    // 能力最小化 + 上限 + 抽样：判据全在 `pixels_to_send`，不在 UI 再抄一遍。
    let declared = ext.manifest.has_capability(Capability::ImageRead);
    let accepts_sampled = ext.manifest.has_capability(Capability::ImageSampled);
    let had_image = image.is_some();
    let image = pixels_to_send(declared, accepts_sampled, image);
    log_pixels(
        &ext.manifest.id,
        declared,
        accepts_sampled,
        had_image,
        image.as_ref(),
    );
    let job = register(&ext.manifest.id)?;
    let name = ext.manifest.name.clone();
    let id = ext.manifest.id.clone();
    let cleanup_job = job.clone();
    let cleanup_id = id.clone();
    std::thread::Builder::new()
        .spawn(move || {
            let result = run_command(&ext, image_path, selection, image, &job);
            finish(&id, name, &job, None, result);
        })
        .map_err(|e| {
            deregister(&cleanup_id, &cleanup_job);
            format!("启动插件任务线程失败：{e}")
        })?;
    Ok(())
}

/// 在后台启动一次编辑型插件调用。立即返回，结果用 [`poll`] 取。
///
/// 送的是 [`ForPlugin`]——**屏上朝向**的整图（`Viewer::whole_image_for_plugin` 那一份）：
/// 回传必须同尺寸，落地按里面冻结的 `orient` 转回源朝向再套成编辑覆盖（理由见
/// [`JobOutput::Edited`]）。命令型走的是**同一个类型、同一根闸**（[`pixels_within_limit`]），
/// 两型不该各有一套"什么算能送"的判据。
pub fn start_editor(ext: Extension, image: ForPlugin) -> Result<(), String> {
    if ext.manifest.kind != Kind::Editor {
        return Err("该插件不是编辑型".into());
    }
    gate(&ext, true)?;
    if !ext.manifest.has_capability(Capability::ImageWrite) {
        return Err(format!(
            "编辑型插件 {} 未声明 image.write，宿主不接收回写",
            ext.manifest.name
        ));
    }
    if !pixels_within_limit(image.width, image.height) {
        return Err(format!(
            "图片超出编辑档上限（{} MP）",
            EDITOR_MAX_PIXELS / 1_000_000
        ));
    }
    let job = register(&ext.manifest.id)?;
    let name = ext.manifest.name.clone();
    let id = ext.manifest.id.clone();
    let cleanup_job = job.clone();
    let cleanup_id = id.clone();
    std::thread::Builder::new()
        .spawn(move || {
            // `finish` 要报的是"哪张图的编辑落地了"，路径先摘出来（`image` 整个进 `run_editor`）。
            let target = image.path.clone();
            let result = run_editor(&ext, image, &job);
            finish(&id, name, &job, Some(target), result);
        })
        .map_err(|e| {
            deregister(&cleanup_id, &cleanup_job);
            format!("启动插件任务线程失败：{e}")
        })?;
    Ok(())
}

/// 在后台拉起某插件的**设置窗口**（manifest 声明 `settings_ui` 才有）。
///
/// 为什么复用管道而不是另开协议形态：设置窗口就是插件的另一条 `action`，
/// 启动参数、PID 校验、能力下发、设置合并落盘全走同一条路——旧插件永远收不到
/// 这条 action（宿主只对声明者发），因此不必 bump `protocol`。
pub fn start_settings(ext: Extension) -> Result<(), String> {
    if !ext.manifest.settings_ui {
        return Err(format!("插件 {} 没有提供设置界面", ext.manifest.name));
    }
    gate(&ext, false)?;
    let job = register(&ext.manifest.id)?;
    let name = ext.manifest.name.clone();
    let id = ext.manifest.id.clone();
    let cleanup_job = job.clone();
    let cleanup_id = id.clone();
    std::thread::Builder::new()
        .spawn(move || {
            let result = run_settings(&ext, &job);
            finish(&id, name, &job, None, result);
        })
        .map_err(|e| {
            deregister(&cleanup_id, &cleanup_job);
            format!("启动插件任务线程失败：{e}")
        })?;
    Ok(())
}

/// 统一收尾：摘运行标记 → 记三振 → 交结果（顺序不能乱：错误提示出现时，
/// 菜单应已回到「运行」；隔离提示出现时，运行标记必须已摘）。
fn finish(
    id: &str,
    name: String,
    job: &Arc<JobHandle>,
    target: Option<PathBuf>,
    result: Result<JobOutput, Failure>,
) {
    deregister(id, job);
    let counts = result.as_ref().err().map(|f| f.strike).unwrap_or(false);
    let isolated_now = record_outcome(id, result.is_ok(), cancelled(job), counts);
    if isolated_now {
        log::warn!("插件 {id} 连续失败 {} 次，已推入隔离队列", STRIKE_LIMIT);
    }
    if let Err(f) = &result {
        log::warn!(
            "插件 {id} 调用失败（{}）：{}",
            if f.strike {
                "计入隔离"
            } else {
                "设计内拒绝，不记账"
            },
            f.text
        );
    }
    lock(JOBS.get_or_init(|| Mutex::new(Vec::new()))).push(JobOutcome {
        plugin: name,
        id: id.to_string(),
        target,
        result: result.map_err(|f| f.text),
    });
}

/// UI 侧每帧/每 tick 调用：取走已完成的结果（不阻塞）。
pub fn poll() -> Vec<JobOutcome> {
    let Some(q) = JOBS.get() else {
        return Vec::new();
    };
    let Ok(mut q) = q.lock() else {
        return Vec::new();
    };
    std::mem::take(&mut *q)
}

/// 起插件进程并纳入 Job Object。返回 pid（accept 的 PID 校验用）。
fn spawn_plugin(ext: &Extension, pipe_name: &str, job: &JobHandle) -> Result<u32, String> {
    let mut args = ext.manifest.args.clone();
    args.push("--rupix-pipe".into());
    args.push(pipe_name.to_string());
    args.push("--rupix-protocol".into());
    // 回插件自己声明的版本（宿主在 MIN..=MAX 内接受了它），不硬塞宿主最新版。
    args.push(ext.manifest.protocol.to_string());
    let entry = ext.dir.join(&ext.manifest.entry);
    let child = process::spawn_logged(&entry, &args, &ext.dir, &ext.manifest.id)
        .map_err(|e| format!("启动插件失败：{e}"))?;
    let pid = child.pid();
    // 进程组兜底：宿主异常退出时内核杀整组子进程。失败只降级（记 warn 继续），
    // 不让一层额外保护挡住正常调用。
    match job::Job::create().and_then(|j| j.assign(pid).map(move |()| j)) {
        Ok(j) => *lock(&job.job) = Some(j),
        Err(e) => log::warn!("插件 {pid} 未能加入 Job Object（少一层兜底）：{e}"),
    }
    *lock(&job.child) = Some(child);
    Ok(pid)
}

/// 建管道等插件连上（10s 超时；超时/取消打断 accept + 杀进程）+ PID 校验。
///
/// 请求体还没发出去就做校验：陌生进程抢连（管道名可预测）拿不到 image_path/选区。
fn accept_plugin(
    pipe_name: &str,
    expected_pid: u32,
    job: &JobHandle,
) -> Result<pipe::Connection, String> {
    let server =
        pipe::PipeServer::create(pipe_name).map_err(|e| format!("创建插件管道失败：{e}"))?;
    if cancelled(job) {
        return Err("已取消".into());
    }
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tx.send(server.accept());
    });
    *lock(&job.accept_worker) = Some(worker);
    let conn = match rx.recv_timeout(ACCEPT_TIMEOUT) {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            kill_child(job);
            return Err(if cancelled(job) {
                "已取消".into()
            } else {
                format!("插件未连接管道：{e}")
            });
        }
        Err(_) => {
            // 超时/取消：打断 worker 的阻塞 ConnectNamedPipe——它的 Drop 会关
            // 句柄，线程随后退出。不打断 = 每失败一次泄漏一个阻塞线程 +
            // 一枚打开的管道句柄（§七 退出路径阻塞的同族问题）。
            if let Some(w) = lock(&job.accept_worker).take() {
                pipe::cancel_synchronous_io(&w);
            }
            kill_child(job);
            return Err(if cancelled(job) {
                "已取消".into()
            } else {
                "插件连接管道超时".into()
            });
        }
    };
    if conn.client_pid() != expected_pid {
        kill_child(job);
        return Err("插件管道被陌生进程连接，已拒绝".into());
    }
    Ok(conn)
}

/// 把插件自己的设置**发**给它（能力最小化：只有声明 `settings.read` 的插件拿得到）。
fn attach_settings(ext: &Extension, req: &mut Request) {
    if !ext.manifest.has_capability(Capability::SettingsRead) {
        return;
    }
    let stored = super::settings::load(&ext.manifest.id);
    if !stored.is_empty() {
        req.settings = Some(stored);
    }
}

/// 把插件回传的设置**存**回去（能力最小化：只有声明 `settings.write` 的插件能落盘）。
///
/// 写失败只降级为 warn：设置没存上不该把这次插件结果打没。
fn persist_settings(ext: &Extension, resp: &Response) {
    if !ext.manifest.has_capability(Capability::SettingsWrite) {
        return;
    }
    let Some(incoming) = &resp.settings else {
        return;
    };
    if incoming.is_empty() {
        return;
    }
    match super::settings::merge_into(&ext.manifest.id, incoming) {
        Ok(_) => log::info!("插件 {} 写回设置 {} 项", ext.manifest.id, incoming.len()),
        Err(e) => log::warn!("插件 {} 的设置没能存下：{e}", ext.manifest.id),
    }
}

fn run_command(
    ext: &Extension,
    image_path: Option<PathBuf>,
    selection: Option<Selection>,
    image: Option<Outgoing>,
    job: &JobHandle,
) -> Result<JobOutput, Failure> {
    let mut req = Request::new(ext.manifest.id.clone(), "run", ext.manifest.protocol);
    // WinRT StorageFile 只接受绝对路径；相对路径按**宿主**当前目录解析（不是插件目录）。
    req.image_path = image_path.map(|p| {
        std::path::absolute(&p)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    });
    req.selection = selection;
    req.args = ext.manifest.args.clone();
    // ★ m166 第一格：命令型也能收到像素了。`_shm` 握到 `run_action` 返回为止
    //   （命名映射随最后一个句柄消失），与编辑型同一条规矩、同一个 `publish_image`。
    //   拿不到整图（切片态 / 超上限 / 屏上没图）就**照旧只发路径**——不是报错，
    //   老插件（只要路径的那种）行为因此完全是子集。
    let _shm = match image {
        Some(out) => {
            let (reference, shm) = publish_image(out.px.width, out.px.height, &out.px.rgba)?;
            req.image = Some(reference);
            // 原图尺寸 + 步长：插件据此算"这份像素覆盖原图多少"，报告才可能诚实。
            req.source = Some(out.px.natural);
            req.sample_step = out.step;
            Some(shm)
        }
        None => None,
    };
    // 设置区（能力最小化）：只有声明 `settings.read` 的插件才拿到宿主存的设置。
    attach_settings(ext, &mut req);
    let resp = run_action(ext, req, false, job)?;

    // 设置写回（能力最小化）：只有声明 `settings.write` 的插件能让宿主落盘。
    persist_settings(ext, &resp);

    let text = resp.text.unwrap_or_else(|| "完成".into());
    // 剪贴板（能力最小化）：只有声明 `clipboard.write` 的插件，宿主才把结果
    // 自动写进系统剪贴板（OCR 类插件的常见诉求）。写失败只 warn——
    // 结果已经在结果窗里了，剪贴板写不进去不是这次调用的失败。
    let copied = if ext.manifest.has_capability(Capability::ClipboardWrite) {
        match crate::platform::clipboard::copy_text(&text) {
            Ok(()) => {
                log::info!("插件 {} 的结果已写入剪贴板", ext.manifest.id);
                true
            }
            Err(e) => {
                log::warn!("插件 {} 的结果没能写入剪贴板：{e}", ext.manifest.id);
                false
            }
        }
    } else {
        false
    };
    // 打开动作（能力最小化）：插件没声明对应能力就**根本不看**这两个字段。
    // URL 只认 http/https；图片的 base64 解不出就只记 warn——结果窗里的文本照样给。
    let open_url = if ext.manifest.has_capability(Capability::OpenUrl) {
        resp.open_url
            .filter(|u| crate::platform::shell::is_openable_url(u))
    } else {
        None
    };
    let open_image = if ext.manifest.has_capability(Capability::OpenFile) {
        match resp.open_image {
            Some((ext_name, data)) => match crate::core::b64::decode(&data) {
                Ok(bytes) => Some((ext_name, bytes)),
                Err(e) => {
                    log::warn!("插件 {} 的图片载荷解不开：{e}", ext.manifest.id);
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };
    Ok(JobOutput::Text {
        text,
        copied,
        open_url,
        open_image,
    })
}

/// 设置窗口调用（manifest 声明 `settings_ui` 才有）。
///
/// 交互形态与编辑型一致：**不设响应超时**——用户在插件的设置窗口里待多久都行，
/// 取消走插件菜单。回传的设置照旧合并落盘（`persist_settings`）。
fn run_settings(ext: &Extension, job: &JobHandle) -> Result<JobOutput, Failure> {
    let mut req = Request::new(ext.manifest.id.clone(), "settings", ext.manifest.protocol);
    attach_settings(ext, &mut req);
    let resp = run_action(ext, req, true, job)?;
    if resp.settings.is_some() {
        persist_settings(ext, &resp);
    }
    Ok(JobOutput::Settings)
}

/// 一次「起进程 → 连管道 → 发一帧 → 收一帧」的公共骨架。
///
/// `interactive=true` = 交互任务（设置窗口）：不设响应超时，等插件自己回；
/// `false` = 命令型，等 `manifest.timeout_ms`（缺省 30 s）就腰斩。
fn run_action(
    ext: &Extension,
    req: Request,
    interactive: bool,
    job: &JobHandle,
) -> Result<Response, Failure> {
    let pname = pipe_name(&ext.manifest.id);
    let pid = spawn_plugin(ext, &pname, job)?;
    let mut conn = accept_plugin(&pname, pid, job)?;
    if cancelled(job) {
        kill_child(job);
        return Err("已取消".into());
    }
    let body = req.to_json();

    let (tx_resp, rx_resp) = mpsc::channel();
    let _io_worker = std::thread::spawn(move || {
        let result = conn
            .write_frame(body.as_bytes())
            .and_then(|()| conn.read_frame())
            .and_then(|b| String::from_utf8(b).map_err(|e| format!("响应不是 UTF-8：{e}")))
            .and_then(|s| Response::parse(&s));
        let _ = tx_resp.send(result);
    });

    // 响应超时由插件自己声明（manifest.timeout_ms，缺省 30s，上限 30 分钟）：
    // 宿主不替插件猜它要跑多久——猜短了腰斩，猜长了干等。
    let timeout = ext.manifest.response_timeout();
    let resp = if interactive {
        // 交互任务：取消走插件菜单（杀进程断管道，这条 recv 随即返回错误）。
        match rx_resp.recv() {
            Ok(r) => r,
            Err(_) => {
                kill_child(job);
                return Err("插件响应通道已断".into());
            }
        }
    } else {
        match rx_resp.recv_timeout(timeout) {
            Ok(r) => r,
            Err(_) => {
                // 终止子进程会断开管道，io_worker 的 read 随之返回并退出线程。
                kill_child(job);
                return Err(if cancelled(job) {
                    "已取消".into()
                } else {
                    format!("插件响应超时（{} 秒）", timeout.as_secs()).into()
                });
            }
        }
    };
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            kill_child(job);
            return Err(if cancelled(job) {
                "已取消".into()
            } else {
                format!("插件响应无效：{e}").into()
            });
        }
    };

    // 收尾：给 2s 自然退出，仍活着就终止；取消过的这步是幂等的。
    if let Some(c) = lock(&job.child).as_mut() {
        if c.wait_timeout(EXIT_GRACE).ok().flatten().is_none() {
            let _ = c.terminate();
        }
    }

    if cancelled(job) {
        return Err("已取消".into());
    }
    if !resp.ok {
        // ★ 这是**插件按协议回的答复**，不是它坏了：只出提示，不记三振。
        return Err(Failure::reported(
            resp.error.unwrap_or_else(|| "插件返回失败".into()),
        ));
    }
    Ok(resp)
}

/// 建一块送插件的命名映射并写入整图像素，交回「引用 + 句柄」。
///
/// ★ 句柄必须被**调用方**握到插件打开映射之后：命名映射对象随最后一个句柄消失，
///   提前 drop = 插件按名字打开时对象已经没了。所以这里把它返回出去留在调用栈上，
///   而不是在函数里丢掉（那正是这类 bug 的样子：本地测不出、插件端偶发打不开）。
///
/// 编辑型与命令型共用这一份：两型送出去的必须是同一种东西（同一布局、同一朝向语义，
/// 见插件仓《插件开发规范》§6.2），不该有两套建法。
fn publish_image(
    w: u32,
    h: u32,
    rgba: &[u8],
) -> Result<(ImageRef, shared_mem::SharedImage), String> {
    let stride = w.checked_mul(4).ok_or_else(|| "图片宽度过大".to_string())?;
    let name = format!(
        "Local\\rupix-ext-host-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let mut shm = shared_mem::SharedImage::create(&name, w, h, stride)
        .map_err(|e| format!("创建共享内存失败：{e}"))?;
    shm.copy_from(rgba)
        .map_err(|e| format!("写入共享内存失败：{e}"))?;
    let reference =
        ImageRef::new(name, w, h, stride).map_err(|e| format!("图像引用不合法：{e}"))?;
    Ok((reference, shm))
}

fn run_editor(ext: &Extension, image: ForPlugin, job: &JobHandle) -> Result<JobOutput, Failure> {
    let ForPlugin {
        path: image_path,
        width: w,
        height: h,
        rgba,
        orient,
        // 编辑型**不看** natural：回写必须同尺寸，抽样在那条路上是错的。
        natural: _,
    } = image;
    // 宿主建可读写映射（P0.1b-1 底座），写进当前解码档的整图像素（屏上朝向那一份）。
    let (reference, _shm) = publish_image(w, h, &rgba)?;
    if cancelled(job) {
        return Err("已取消".into());
    }

    let pname = pipe_name(&ext.manifest.id);
    let pid = spawn_plugin(ext, &pname, job)?;
    let mut conn = accept_plugin(&pname, pid, job)?;
    if cancelled(job) {
        kill_child(job);
        return Err("已取消".into());
    }

    let mut req = Request::new(ext.manifest.id.clone(), "run", ext.manifest.protocol);
    // 只给共享内存引用，不给文件路径：编辑插件没声明 file.read（能力最小化），
    // 也不需要——它编辑的就是送进来的这一档像素。
    req.image = Some(reference);
    // 编辑型同样能读自己的设置区（画笔颜色、上次选的工具……）。
    attach_settings(ext, &mut req);

    let (tx_resp, rx_resp) = mpsc::channel();
    let _io_worker = std::thread::spawn(move || {
        let result = conn
            .write_frame(req.to_json().as_bytes())
            .and_then(|()| conn.read_frame())
            .and_then(|b| String::from_utf8(b).map_err(|e| format!("响应不是 UTF-8：{e}")))
            .and_then(|s| Response::parse(&s))
            // 在 worker 里打开插件回传的映射并**当场拷出像素**：SharedImage 内含
            // 裸指针不能跨线程，映射对象随 worker 里的句柄存活到拷贝完成。
            .and_then(|r| match &r.image {
                Some(img) => {
                    let shm = shared_mem::SharedImage::open(&img.name)
                        .map_err(|e| format!("读取编辑结果失败：{e}"))?;
                    if shm.width() != w || shm.height() != h {
                        return Err(format!(
                            "编辑结果尺寸 {}×{} 与送入 {}×{} 不一致，已拒绝",
                            shm.width(),
                            shm.height(),
                            w,
                            h
                        ));
                    }
                    Ok((r, Some(shm.pixels().to_vec())))
                }
                None => Ok((r, None)),
            });
        let _ = tx_resp.send(result);
    });

    // 编辑是交互任务：不设响应超时（用户画多久都行）；取消走插件菜单的
    // 「取消」（杀进程断管道，下面的 recv 随即返回错误）。recv 双层：外层是
    // 通道，内层是 worker 的 Result。
    let payload = match rx_resp.recv() {
        Ok(p) => p,
        Err(_) => return Err("插件响应通道已断".into()),
    };
    let (resp, edited) = match payload {
        Ok(pair) => pair,
        Err(e) => {
            kill_child(job);
            return Err(if cancelled(job) {
                "已取消".into()
            } else {
                format!("插件响应无效：{e}").into()
            });
        }
    };
    let edited = match (&resp.image, edited) {
        (Some(_), Some(px)) if !px.is_empty() => px,
        (Some(_), _) => {
            kill_child(job);
            return Err("编辑型插件回传了空图像".into());
        }
        (None, _) => {
            kill_child(job);
            return Err(if cancelled(job) {
                "已取消".into()
            } else {
                resp.error
                    .unwrap_or_else(|| "编辑型插件未回传图像".into())
                    .into()
            });
        }
    };

    // 收尾：宿主已拷走像素，插件（按契约读到 EOF）应自退；2s 不退就终止。
    if let Some(c) = lock(&job.child).as_mut() {
        if c.wait_timeout(EXIT_GRACE).ok().flatten().is_none() {
            let _ = c.terminate();
        }
    }

    if cancelled(job) {
        return Err("已取消".into());
    }
    if !resp.ok {
        return Err(Failure::reported(
            resp.error.unwrap_or_else(|| "插件返回失败".into()),
        ));
    }
    persist_settings(ext, &resp);
    Ok(JobOutput::Edited {
        path: image_path,
        width: w,
        height: h,
        rgba: edited,
        orient,
    })
}

fn pipe_name(id: &str) -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{}{}-{}-{}",
        pipe::NAME_PREFIX,
        std::process::id(),
        safe,
        seq
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names_are_distinct_and_prefixed() {
        let a = pipe_name("com.example.ocr");
        let b = pipe_name("com.example.ocr");
        assert!(a.starts_with(pipe::NAME_PREFIX));
        assert!(a.contains("com.example.ocr"));
        assert_ne!(a, b);
    }

    #[test]
    fn poll_without_jobs_is_empty() {
        assert!(poll().is_empty());
    }

    /// 设置入口只对声明了 `settings_ui` 的插件开放（旧插件永远收不到 `action=settings`）。
    ///
    /// 判据放在起进程之前：没声明就当场拒，不 spawn、不建管道——这是"协议只加不改"
    /// 能成立的前提（旧插件收到未知 action 只会一脸茫然）。
    #[test]
    fn settings_entry_refused_without_settings_ui() {
        use crate::core::ext::Manifest;
        let plain = Manifest::parse(
            r#"{"schema":1,"id":"test.settings.plain","name":"P","version":"1.0.0",
                "protocol":1,"min_host":"0.1.0","kind":"command","entry":"x.exe",
                "capabilities":[],"platforms":["windows-x64"]}"#,
        )
        .expect("manifest");
        let ext = Extension {
            manifest: plain,
            dir: std::path::PathBuf::new(),
            enabled: true,
            blocked: false,
            isolated: false,
        };
        let err = start_settings(ext).expect_err("没有设置窗口的插件必须被拒");
        assert!(err.contains("没有提供设置界面"), "错误应说清原因：{err}");
        assert!(!is_running("test.settings.plain"), "拒在起进程之前");
    }

    /// 入口按 `kind` 分派：编辑型不能走命令入口，命令型也不能走编辑入口。
    ///
    /// 这条钉的是 2026-10-29 用户报障的根：涂鸦是 `editor`，却被首用同意窗
    /// 统一送进了 `start_command`，于是同意后报「该插件不是命令型」。
    #[test]
    fn editor_and_command_entries_refuse_the_wrong_kind() {
        use crate::core::ext::Manifest;
        let mk = |kind: &str| {
            let manifest = Manifest::parse(&format!(
                r#"{{"schema":1,"id":"test.kind.{kind}","name":"P","version":"1.0.0",
                    "protocol":1,"min_host":"0.1.0","kind":"{kind}","entry":"x.exe",
                    "capabilities":[],"platforms":["windows-x64"]}}"#
            ))
            .expect("manifest");
            Extension {
                manifest,
                dir: std::path::PathBuf::new(),
                enabled: true,
                blocked: false,
                isolated: false,
            }
        };
        let err = start_command(mk("editor"), None, None, None).expect_err("编辑型不能走命令入口");
        assert!(err.contains("不是命令型"), "错误应说清类型：{err}");
        let err =
            start_editor(mk("command"), fixture_pixels(1, 1)).expect_err("命令型不能走编辑入口");
        assert!(err.contains("不是编辑型"), "错误应说清类型：{err}");
    }

    /// 一份只带尺寸的用例整图（`rgba` 留空：分流判据读尺寸不读像素，
    /// 按尺寸真分配就等于为每条用例拷 32 MB）。
    fn fixture_pixels(w: u32, h: u32) -> ForPlugin {
        ForPlugin {
            path: PathBuf::from("x.png"),
            width: w,
            height: h,
            rgba: Arc::new(Vec::new()),
            natural: (w, h),
            orient: (0, false),
        }
    }

    /// ★ m166 第一格：命令型也能收像素了，但**送不送只有一条判据**——声明了 `image.read`
    /// 且整图在 [`EDITOR_MAX_PIXELS`] 内。没声明的一律不发（能力最小化：顺手多发点在这条线上
    /// 就是越权，而且那是一次最多 32 MB 的映射写入）；超上限且没接抽样也不发（宿主不替插件猜）。
    /// 边界取"正好等于上限"与"多一像素"这一对，只有它们能证明判据用的是 `<=` 不是 `<`。
    #[test]
    fn pixels_go_only_to_declaring_plugins_within_the_cap() {
        let ok = Some(fixture_pixels(2000, 1000));
        assert!(
            pixels_to_send(true, false, ok.clone()).is_some(),
            "声明 + 在上限内该送"
        );
        assert!(
            pixels_to_send(false, true, ok).is_none(),
            "没声明 image.read 一律不送（接了抽样也一样）"
        );
        assert!(
            pixels_to_send(true, false, Some(fixture_pixels(4000, 2000))).is_some(),
            "正好 8 MP 该在界内"
        );
        assert!(
            pixels_to_send(true, false, Some(fixture_pixels(4000, 2001))).is_none(),
            "多一像素且没接抽样 ⇒ 出界不发"
        );
        assert!(
            pixels_to_send(true, false, Some(fixture_pixels(0, 4))).is_none(),
            "0 高不是「当然在上限内」，与编辑型一样当场拒"
        );
        assert!(
            pixels_to_send(true, true, Some(fixture_pixels(0, 4))).is_none(),
            "接了抽样也不是空图的通行证"
        );
        assert!(
            pixels_to_send(true, false, None).is_none(),
            "屏上没有整图就不发"
        );
        // 空像素缓冲（尺寸对、字节没带）抽不出东西 ⇒ 安全降级为不发，而不是发一张空图。
        assert!(
            pixels_to_send(true, true, Some(fixture_pixels(4000, 2001))).is_none(),
            "没有像素字节时抽样救不了"
        );
    }

    /// ★ 超上限 + 声明接受抽样 ⇒ 送出去的是**抽样后**的一千万级以下的图，步长 > 1。
    ///
    /// 这条用真像素（32 MB）走一遍，因为"抽样是否真的只挑不混"在尺寸判据里看不出来——
    /// 那是 `core::pixels::sampling_keeps_the_distribution_while_averaging_would_not` 的活。
    #[test]
    fn over_cap_pixels_are_sampled_when_the_plugin_accepts_it() {
        let (w, h) = (4000u32, 2001u32);
        let px = ForPlugin {
            path: PathBuf::from("big.png"),
            width: w,
            height: h,
            rgba: Arc::new(vec![9u8; (w * h * 4) as usize]),
            natural: (w, h),
            orient: (0, false),
        };
        let out = pixels_to_send(true, true, Some(px)).expect("接了抽样就该有图");
        assert!(out.step > 1, "超 8 MP 必须走抽样");
        let got = (out.px.width as u64) * (out.px.height as u64);
        assert!(
            got <= crate::core::pixels::PLUGIN_SAMPLE_TARGET * 2,
            "样本量必须落回预算：{got}"
        );
        assert_eq!(out.px.natural, (w, h), "原图尺寸要原样带出去");
        assert_eq!(
            out.px.rgba.len(),
            (out.px.width as usize) * (out.px.height as usize) * 4,
            "像素长度必须与新尺寸一致"
        );
    }

    #[test]
    fn register_is_exclusive_and_deregisters() {
        let id = "test.register.exclusive";
        let _job = register(id).expect("首次注册应成功");
        assert!(is_running(id));
        assert!(register(id).is_err(), "同 id 二次注册必须被拒");
        assert!(cancel(id), "取消在跑的任务应返回 true");
        assert!(!is_running(id), "取消即摘除运行标记");
        assert!(!cancel(id), "重复取消是无害 no-op");
    }

    #[test]
    fn audit_old_finish_keeps_restarted_job_registered() {
        let id = "test.audit.restart";
        let old = register(id).expect("old task");
        assert!(cancel(id));
        let new = register(id).expect("cancel permits immediate restart");
        finish(id, "old".into(), &old, None, Err("cancelled".into()));
        let remains = lock(running())
            .get(id)
            .is_some_and(|job| Arc::ptr_eq(job, &new));
        let exclusive = register(id).is_err();
        let outcomes = poll();
        let can_cancel_new = cancel(id);
        assert!(remains, "old finish must not remove the replacement task");
        assert!(
            exclusive,
            "replacement must still prevent duplicate registration"
        );
        assert!(can_cancel_new);
        assert!(cancelled(&new));
        assert!(outcomes.iter().any(|outcome| outcome.id == id));
    }

    #[test]
    fn audit_old_deregister_keeps_restarted_job_registered() {
        let id = "test.audit.deregister";
        let old = register(id).expect("old task");
        assert!(cancel(id));
        let new = register(id).expect("replacement task");
        deregister(id, &old);
        let remains = lock(running())
            .get(id)
            .is_some_and(|job| Arc::ptr_eq(job, &new));
        deregister(id, &new);
        assert!(
            remains,
            "late cleanup must not deregister another task instance"
        );
        assert!(!is_running(id), "own cleanup must remove its registration");
    }

    #[test]
    fn cancel_unknown_id_is_noop() {
        assert!(!cancel("test.cancel.nope"));
    }

    #[test]
    fn three_strikes_isolate_and_success_resets() {
        let id = "test.strikes.a";
        assert!(!record_outcome(id, false, false, true));
        assert!(!record_outcome(id, false, false, true));
        // 用户取消不打断连败计数，但也不计振。
        assert!(!record_outcome(id, false, true, true));
        assert!(!record_outcome(id, true, false, true), "成功清零");
        assert!(!record_outcome(id, false, false, true));
        assert!(!record_outcome(id, false, false, true));
        assert!(record_outcome(id, false, false, true), "第三次实败触发隔离");
        assert_eq!(take_isolations(), vec![id.to_string()]);
        assert!(!record_outcome(id, false, false, true), "隔离后计数已重置");
        let _ = take_isolations();
    }

    #[test]
    fn cancellation_does_not_count_as_strike() {
        let id = "test.strikes.cancel";
        assert!(!record_outcome(id, false, true, true));
        assert!(!record_outcome(id, false, true, true));
        assert!(!record_outcome(id, false, true, true));
        assert!(take_isolations().is_empty(), "取消永远不触发隔离");
    }

    /// ★ 2026-11-07 报障的正：插件按协议回 `ok:false`（"这图我不处理"）连续三次，
    /// 旧逻辑把它当三次崩溃、推进隔离队列。**拒绝是它的设计，不是它坏了。**
    #[test]
    fn reported_refusal_never_isolates() {
        let id = "test.strikes.reported";
        for _ in 0..6 {
            assert!(
                !record_outcome(id, false, false, false),
                "counts=false 的失败永远不该触发隔离"
            );
        }
        assert!(
            take_isolations().is_empty(),
            "六次设计内拒绝也不许隔离一枚健康的插件"
        );
        // 但它也**不该**把已有的连败计数清零——那会让"真崩三次"更容易被放过。
        assert!(!record_outcome(id, false, false, true));
        assert!(!record_outcome(id, false, false, true));
        assert!(record_outcome(id, false, false, true), "真崩三次照样隔离");
        let _ = take_isolations();
    }
}
