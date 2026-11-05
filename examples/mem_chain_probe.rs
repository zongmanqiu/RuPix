//! 巨图这条链上，**每一份常驻像素缓冲占多少字节、在调用线程上花多久、能不能当场还给系统**
//! 的实测探针。
//!
//! 存在理由（2026-10-17 用户报障）：同一张 `tiff_100M.tif`（6055×6055），参照项目
//! pineapple-pictures 只用约 200 MB，我们峰值 700 MB 以上，缩放还更顿。回答"是不是我们
//! 优化不好"与"交互完像素能不能立刻释放"都不能靠读代码数拷贝——数出来的每一份都得拿
//! 工作集、提交量和分段计时对上账。
//!
//! 三个读数各自的含义（**只看工作集会得出错误结论**）：
//! - 工作集：此刻占着的物理页。drop 之后不一定会掉——归还先发生在分配器层面，留在池子里
//!   的页仍算工作集，要等 `EmptyWorkingSet` 一刀才落回去。
//! - 提交量 `PrivateUsage`：进程真正欠系统的字节。**drop 之后该掉的就是它**，它是"能不能
//!   释放"的判据。
//! - 峰值工作集：对应任务管理器里看到的那一下飙升。
//!
//! 量四件事：
//! 1. **一次解码瞬时要吃掉多少**（解码器不理 `target_w/h`，宿主解完整图再 `fit_to`，所以
//!    "适应窗口"那一份虽然只留 3 MB，瞬时峰值是它的一百倍）；
//! 2. **放大到 ≥100% 之后同时活着几份整图**：`base_rgba` → 派生档位（≥100% 那一份是
//!    `base.to_vec()` 的**白拷**）→ windui `Image`（预乘副本），每存住一份打一次读数；
//! 3. **每一档在调用线程上花多久**：白拷、重采样、上传预乘——这三项今天全跑在 UI 线程，
//!    就是"缩放时那一下顿"的分解；
//! 4. **交还的时机**：全部缓冲 drop 掉读提交量，再跑一次空闲裁剪读工作集——分清"能还"
//!    与"什么时候还得掉"。
//!
//! 记账口径：每份缓冲都由 `Buffers` 按名字持有到该量完为止（**不提前释放、不额外克隆**），
//! 增量才归属得到位；随用随丢的话每次读到的都是同一个数，量不出"第几份"。
//!
//! 用法（release，带解码器目录）：
//!
//! ```text
//! cd ../.temp/target/release/examples
//! RUPIX_DECODERS_DIR=<工作区根>\.build\decoders \
//!   ./mem_chain_probe.exe <big.tif> [邻居大图]
//! ```

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use rupix_lib::core::config::Config;
use rupix_lib::core::decode::{probe_decode, Decoder, Done};
use rupix_lib::core::geometry::{fit_to, RectI};
use rupix_lib::core::pixels::{Anim, Bitmap, Decoded};
use rupix_lib::core::viewer::Viewer;
use windui::render::image::Image;

const MB: f64 = 1024.0 * 1024.0;

/// 进程内存三项读数（MB）。读不到即全 0——报告里照实显示 0，不猜。
#[derive(Copy, Clone, Default)]
struct Mem {
    /// 工作集：此刻占着的物理页。
    ws: f64,
    /// 提交量：进程真正欠系统的字节。
    priv_mb: f64,
    /// 峰值工作集。
    peak: f64,
}

fn mem() -> Mem {
    #[cfg(windows)]
    {
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut pm = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        // 这个 Win32 API 靠 `cb` 分辨结构版本，指针类型仍写的是旧结构。
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                std::ptr::addr_of_mut!(pm).cast::<PROCESS_MEMORY_COUNTERS>(),
                pm.cb,
            )
        };
        if ok.is_ok() {
            return Mem {
                ws: pm.WorkingSetSize as f64 / MB,
                priv_mb: pm.PrivateUsage as f64 / MB,
                peak: pm.PeakWorkingSetSize as f64 / MB,
            };
        }
        Mem::default()
    }
    #[cfg(not(windows))]
    {
        Mem::default()
    }
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / MB
}

fn bytes_of(w: u32, h: u32) -> usize {
    (w as usize) * (h as usize) * 4
}

/// 真实链路的"停稳"：泵解码回包与**派生线程回包**直到安静，再报上屏尺寸、两项内存读数，
/// 以及 **UI 线程到底被占用了多久**。
///
/// 为什么要这一段：①–⑧ 量的是**单份缓冲的单价**（可以加总），而用户抱怨的常驻是
/// **真实链路**同时留下了哪几份——只有走 `Viewer` + 解码线程才说得清"打开 / 放大 / 缩回
/// 之后各停在哪里、缩回去会不会降"。
///
/// 停稳要三条判据一起满足（K48 的教训：不许把"探测器安静了"读成"画面到位了"）：回包数够、
/// 重绘信号数够、上屏位图到达**期望尺寸**（`(0,0)` = 本步不检查尺寸）。M2 之后换档在另一条
/// 线程上采，这里必须**同时泵** `poll_levels`——漏泵它就是"实现完全到位、画面却永远不到"。
///
/// `ui_busy` 才是用户感知的"卡顿"：墙钟里等线程的那段时间 UI 是**空闲**的（照样能响应
/// 下一次输入），不该算进停顿，所以两项都报。
#[allow(clippy::too_many_arguments)]
fn settle(
    v: &mut Viewer,
    rx: &Receiver<Done>,
    label: &str,
    expect: usize,
    expect_repaints: usize,
    want_dims: (u32, u32),
) {
    let start = Instant::now();
    let mut last = start;
    let mut got = 0usize;
    let mut painted = 0usize;
    let mut ui_busy = Duration::ZERO;
    loop {
        let t = Instant::now();
        let hit = v.tick() || v.poll_levels();
        ui_busy += t.elapsed();
        if hit {
            painted += 1;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(done) => {
                got += 1;
                let t2 = Instant::now();
                let hit = v.apply(done);
                ui_busy += t2.elapsed();
                if hit {
                    painted += 1;
                }
                last = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {
                let dims = v.image().map(|i| (i.width, i.height)).unwrap_or((0, 0));
                let size_ok = want_dims == (0, 0) || dims == want_dims;
                let settled = got >= expect
                    && painted >= expect_repaints
                    && size_ok
                    && last.elapsed() >= Duration::from_millis(400);
                if settled || start.elapsed() >= Duration::from_secs(60) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let m = mem();
    let dims = v.image().map(|i| (i.width, i.height)).unwrap_or((0, 0));
    println!(
        "  {label:<16} 上屏 {:>5}×{:<5}（回包 {got}，换档 {painted}）｜ 墙钟 {:>5.2}s ｜ UI 忙 {:>5.3}s ｜ 工作集 {:>7.1} MB ｜ 提交 {:>7.1} MB",
        dims.0,
        dims.1,
        start.elapsed().as_secs_f64(),
        ui_busy.as_secs_f64(),
        m.ws,
        m.priv_mb,
    );
    let size_ok = want_dims == (0, 0) || dims == want_dims;
    if got < expect || painted < expect_repaints || !size_ok {
        println!("  ⚠ 判据未满足（回包 {got}/{expect}、换档 {painted}/{expect_repaints}、尺寸 {dims:?} 要 {want_dims:?}）——上面那行是**中间态**，不能当终态读");
    }
}

/// 读数账本：基线 + 每次"又存住一份"后的三项读数与用时。
struct Ledger {
    base: Mem,
    /// 已计账的份数与合计字节（末段汇总用）。
    copies: usize,
    bytes: usize,
}

impl Ledger {
    fn start() -> Self {
        Self {
            base: mem(),
            copies: 0,
            bytes: 0,
        }
    }
    /// 记下一份**已由调用方按名字持有**的缓冲（本方法不接管、不克隆）。
    fn note(&mut self, what: &str, bytes: usize, secs: f64) {
        self.copies += 1;
        self.bytes += bytes;
        let now = mem();
        println!(
            "  {:<24}{:>7.1} MB ｜ 工作集 {:>6.1}（+{:5.1}）｜ 提交 {:>6.1}（+{:5.1}）｜ 峰值 {:>6.1} ｜ {:>6.3}s",
            what,
            mb(bytes),
            now.ws,
            now.ws - self.base.ws,
            now.priv_mb,
            now.priv_mb - self.base.priv_mb,
            now.peak,
            secs
        );
    }
}

/// 探针对各份像素的**所有权**集中放在这里：末段一次交还，才说得清"到底还了几份"。
#[derive(Default)]
struct Buffers {
    /// ① 适应窗口的显示档（宿主 fit_to 之后的那张小图）。
    fit_displayed: Vec<u8>,
    /// ② 原图档 = `Loaded::base_rgba`。
    base: Vec<u8>,
    /// ③ ≥100% 档 = `Loaded::image`（base 的白拷）。
    native_copy: Vec<u8>,
    /// ④ 另一次换档的 `Loaded::image`（缩档）。
    level_copy: Vec<u8>,
    /// ⑤⑥ 上屏纹理 = windui 的预乘 Pixmap。
    textures: Vec<Image>,
    /// ⑦ 预读邻居（真程序里它被宿主缩到视口档，故这里也按视口档发）。
    prefetch: Vec<u8>,
}

impl Buffers {
    /// 全部交还：逐块拿走所有权并 drop。
    fn release_all(&mut self) {
        drop(std::mem::take(&mut self.fit_displayed));
        drop(std::mem::take(&mut self.base));
        drop(std::mem::take(&mut self.native_copy));
        drop(std::mem::take(&mut self.level_copy));
        drop(std::mem::take(&mut self.prefetch));
        self.textures.clear();
    }
}

fn main() {
    // 与真实启动同一套装配：位图后端（解码器目录）+ SVG 渲染器，装一次全局复用。
    rupix_lib::install_decode_stack();
    let argv: Vec<String> = std::env::args().collect();
    let Some(arg1) = argv.get(1) else {
        eprintln!("用法：mem_chain_probe.exe <大图路径> [邻居大图路径]");
        std::process::exit(2);
    };
    let path = PathBuf::from(arg1);
    if !path.exists() {
        eprintln!("文件不存在：{}", path.display());
        std::process::exit(2);
    }
    let file_bytes = path.metadata().map(|s| s.len() as usize).unwrap_or(0);
    let b = mem();
    println!(
        "探针启动：工作集 {:.1} MB ｜ 提交 {:.1} MB ｜ 峰值 {:.1} MB｜ 目标 {}",
        b.ws,
        b.priv_mb,
        b.peak,
        path.display()
    );
    let mut led = Ledger::start();
    let mut buf = Buffers::default();

    // ── ① 适应窗口档：宿主发 1200×900，解码器到底认不认这个目标 ──────────────
    println!(
        "
① 适应窗口档解码（宿主请求 1200×900）"
    );
    let t = Instant::now();
    let fit = match probe_decode(&path, 1200, 900) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  解码失败：{e}");
            std::process::exit(1);
        }
    };
    let (natural, t_fit) = (fit.natural, t.elapsed().as_secs_f64());
    println!(
        "  原图 {}×{} → 回包 {}×{}（{} MB），用时 {t_fit:.2}s",
        natural.0,
        natural.1,
        fit.bitmap.width,
        fit.bitmap.height,
        mb(fit.bitmap.rgba.len())
    );
    let m = mem();
    println!(
        "  解码器不理 target（回包小是宿主解完整图后自己 fit_to 的结果）；这一次解码瞬时峰值 {:.1} MB ≈ 原图档 {:.1} + 文件字节 {:.1}",
        m.peak,
        mb(bytes_of(natural.0, natural.1)),
        mb(file_bytes)
    );
    buf.fit_displayed = fit.bitmap.rgba;
    led.note("显示档（适应窗口）", buf.fit_displayed.len(), 0.0);

    // ── ② 原图档解码：C 升级那一次，产出常驻的 `base_rgba` ─────────────────
    println!(
        "
② 原图档解码（box=0×0）→ Loaded::base_rgba 常驻"
    );
    let t = Instant::now();
    let whole = match probe_decode(&path, 0, 0) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  解码失败：{e}");
            std::process::exit(1);
        }
    };
    let t_native = t.elapsed().as_secs_f64();
    let (bw, bh) = (whole.bitmap.width, whole.bitmap.height);
    buf.base = whole.bitmap.rgba;
    led.note("base_rgba（原图档）", buf.base.len(), t_native);

    // ── ③ ≥100% 档的白拷：derive_of 的 (fw,fh)==(bw,bh) 分支就是 base.to_vec() ──
    println!(
        "
③ ≥100% 档（派生档 == 原图档）：derive_of 走 base.to_vec()"
    );
    let t = Instant::now();
    buf.native_copy = buf.base.to_vec();
    let t_copy = t.elapsed().as_secs_f64();
    led.note("image（原图档白拷）", buf.native_copy.len(), t_copy);
    println!(
        "  吞吐 ≈ {:.0} MB/s——这一次拷贝**没有改变任何一个像素**，纯粹是多活了一份原图",
        mb(buf.base.len()) / t_copy.max(0.0005)
    );

    // ── ④ ≤100% 换档的重采样（今天跑在 UI 线程上）─────────────────────────
    println!(
        "
④ ≤100% 换档：把 {bw}×{bh} 重采样进 1200×900 的框"
    );
    let t = Instant::now();
    let feed = buf.base.to_vec();
    let t_feed = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let small = fit_to(feed, bw, bh, 1200, 900);
    let t_all = t.elapsed().as_secs_f64();
    let net = (t_all - t_feed).max(0.0);
    buf.level_copy = small.rgba;
    led.note("image（缩档）", buf.level_copy.len(), net);
    println!("  重采样净用时 ≈ {net:.3}s（喂进去的那次 to_vec 单独 {t_feed:.3}s，本次总墙钟 {t_all:.3}s）");

    // ── ⑤ 上屏上传：windui 逐像素预乘，另存一份 Pixmap ─────────────────────
    println!(
        "
⑤ 上屏上传：windui Image::from_rgba（逐像素预乘 → Pixmap）"
    );
    let t = Instant::now();
    let first = match Image::from_rgba(bw, bh, &buf.base) {
        Ok(i) => i,
        Err(e) => {
            println!("  上传失败（不影响前面各项）：{e}");
            std::process::exit(1);
        }
    };
    let sz = bytes_of(first.width(), first.height());
    let secs_up = t.elapsed().as_secs_f64();
    buf.textures.push(first);
    led.note("windui Image（预乘副本）", sz, secs_up);

    // ── ⑥ 换档再上传一次，旧的当场 drop：提交量到底掉不掉 ──────────────────
    println!(
        "
⑥ 第二次上传后立即 drop 第一份（换档后旧纹理的归还时机）"
    );
    match Image::from_rgba(bw, bh, &buf.base) {
        Ok(second) => {
            let before = mem();
            buf.textures.push(second);
            // 换档语义：旧的那份被替换掉（这里丢第一份，只留第二份）。
            let old = buf.textures.remove(0);
            let old_bytes = bytes_of(old.width(), old.height());
            drop(old);
            let after = mem();
            println!(
                "  drop 一份（{:.1} MB）之后：提交 {:.1} → {:.1}（{:+.1}）｜ 工作集 {:.1} → {:.1}（{:+.1}）",
                mb(old_bytes),
                before.priv_mb,
                after.priv_mb,
                after.priv_mb - before.priv_mb,
                before.ws,
                after.ws,
                after.ws - before.ws
            );
            println!(
                "  判据：**提交量**掉了才是真还给系统；工作集不掉只是分配器留用，不等于没释放。"
            );
        }
        Err(e) => println!("  第二次上传失败，跳过本阶段：{e}"),
    }

    // ── ⑦ 预读邻居：真程序发的是视口档，看它常驻多少 ───────────────────────
    if let Some(arg2) = argv.get(2) {
        let sib = Path::new(arg2);
        println!(
            "
⑦ 预读邻居（发 1200×900 视口档，与 `request_prefetch` 同框）：{}",
            sib.display()
        );
        let t = Instant::now();
        if let Ok(s) = probe_decode(sib, 1200, 900) {
            let secs = t.elapsed().as_secs_f64();
            buf.prefetch = s.bitmap.rgba;
            led.note("预读 Decoded", buf.prefetch.len(), secs);
        } else {
            println!("  邻居解码失败，跳过");
        }
    }

    // ── ⑧ 交还：drop 全部 → 读提交量 → 空闲裁剪 → 读工作集 ─────────────────
    println!(
        "
⑧ 交还全部 {} 份（合计 {:.1} MB），看什么时候真的还给系统",
        led.copies,
        mb(led.bytes)
    );
    let m0 = mem();
    println!(
        "  交还前：工作集 {:>6.1} ｜ 提交 {:>6.1} ｜ 峰值 {:>6.1}",
        m0.ws, m0.priv_mb, m0.peak
    );
    buf.release_all();
    let m1 = mem();
    println!(
        "  drop 后：工作集 {:>6.1}（{:+5.1}）｜ 提交 {:>6.1}（{:+5.1}）← 提交量掉才算真还",
        m1.ws,
        m1.ws - m0.ws,
        m1.priv_mb,
        m1.priv_mb - m0.priv_mb
    );
    let ok = rupix_lib::platform::working_set::trim();
    let m2 = mem();
    println!(
        "  EmptyWorkingSet（空闲裁剪，真程序由 runtime 定时触发）可用={ok}：工作集 {:>6.1}（{:+5.1}）｜ 提交 {:>6.1}（{:+5.1}）",
        m2.ws,
        m2.ws - m1.ws,
        m2.priv_mb,
        m2.priv_mb - m1.priv_mb
    );

    // ── ⑨ 真实链路：打开 / 放大 / 缩回，各停下来多少 ────────────────────────
    println!(
        "
⑨ 真实链路（core::viewer + 解码线程，不建窗口）——常驻到底是哪几份、缩回降不降"
    );
    let (tx, rx) = channel::<Done>();
    let decoder = Rc::new(Decoder::spawn_with(tx));
    let mut v = Viewer::new(decoder, Rc::new(RefCell::new(Config::default())));
    // 视口取用户当前配置（`window_w=800 / window_h=600`，dpi 1.0）：档位 = 视口 × 倍率，
    // 量错尺寸档等于量另一台机器。
    v.on_viewport(0.0, 0.0, 800.0, 600.0, 1.0);
    v.open(&path);
    settle(&mut v, &rx, "打开(适应窗口)", 1, 1, (600, 600));
    v.zoom_at_center(2.0);
    settle(&mut v, &rx, "放大 200%", 1, 1, (6055, 6055));
    // 上屏纹理：真实程序里由 UI 层 `ImageCache::obtain` 做，探针走同一个构造函数。
    let tex = v
        .image()
        .and_then(|i| Image::from_rgba(i.width, i.height, &i.rgba).ok());
    let tex_mb = tex
        .as_ref()
        .map(|t| mb(bytes_of(t.width(), t.height())))
        .unwrap_or(0.0);
    // 鸟瞰那份也按 canvas 的 `nav_img` 同样方式上传一次：它与主位图是**两个**
    // 缓存、两份 Pixmap。这一步量的是"鸟瞰到底要框架侧再存多大一张"——原来是
    // 整图尺寸（M9），现在是 ≤512 的缩略。
    let nav = v
        .nav_image_dst(RectI::new(0, 0, 800, 600))
        .and_then(|(src, _)| {
            let (sw, sh) = (src.width, src.height);
            Image::from_rgba(sw, sh, &src.rgba).ok()
        });
    let nav_mb = nav
        .as_ref()
        .map(|t| mb(bytes_of(t.width(), t.height())))
        .unwrap_or(0.0);
    println!(
        "  鸟瞰那份：源位图 {}×{}，框架侧纹理 {:.1} MB（主纹理 {tex_mb:.1} MB 是另一份）",
        nav.as_ref().map(|t| t.width()).unwrap_or(0),
        nav.as_ref().map(|t| t.height()).unwrap_or(0),
        nav_mb
    );
    settle(&mut v, &rx, "＋上屏纹理(主+鸟瞰)", 0, 0, (6055, 6055));
    v.zoom_at_center(0.3);
    // 期望 0 个回包是 C 的签名特征：缩回只是本地换档，不该再回头找解码器。
    // 换档那一下现在由派生线程采，所以判据是"换档信号 ≥1 且尺寸到位"。
    settle(&mut v, &rx, "缩回 30%", 0, 1, (1817, 1817));
    println!(
        "  ↑ 缩回这一步回包应为 0（本地派生）；提交量若不回到「打开」那一档，差额就是按设计常驻的原图档 base（纹理那 {:.1} MB 是另一回事）",
        tex_mb
    );
    drop(nav);
    drop(tex);
    drop(v);
    let m = mem();
    println!(
        "  关掉这张图（viewer 与两份纹理都交还）：工作集 {:>6.1} ｜ 提交 {:>6.1}",
        m.ws, m.priv_mb
    );

    // ── ⑩ 只喂一份自造的 Decoded：把"解码线程那一份"从账上摘掉 ──────────────
    //
    // 这一步不经解码器、不经解码线程：直接喂一份我们自己分配好的整图像素给 `apply`，
    // 读数即知 `install` 在"交出去的那份"之外**自己又留下了几份**。
    //
    // 历史读数（同一张 6055×6055，一份整图 139.9 MB）：
    //   · M1 之前：+141.5 —— 当时算在 `derive_of` 的 `base.to_vec()` 白拷头上，
    //     但 M1 让它共享之后**这项没动**，说明那个归因是错的；
    //   · M1 之后 / M9 之前：仍 +141.5 —— 真凶是鸟瞰那份：`nav_image_of` 拿整图
    //     走 `transformed`，恒等变换也要整份 `to_vec()`；
    //   · M9 之后：应 ≈ +1（鸟瞰封顶 512²，1 MB）。
    println!(
        "
⑩ 只喂一份自造的 Decoded（不经解码器）→ install 自己留几份"
    );
    let n = bytes_of(bw, bh);
    let (tx3, rx3) = channel::<Done>();
    let dec3 = Rc::new(Decoder::spawn_with(tx3));
    let mut v3 = Viewer::new(dec3, Rc::new(RefCell::new(Config::default())));
    v3.on_viewport(0.0, 0.0, 800.0, 600.0, 1.0);
    let before = mem();
    // 只是"造好一份交给宿主的解码结果"，还没喂——这一步的增量就是探针自己那份。
    let d = Decoded {
        natural: (bw, bh),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: bw,
            height: bh,
            rgba: vec![0x10u8; n],
        },
    };
    let mid = mem();
    v3.apply(Done {
        seq: 0,
        path: path.clone(),
        out: Ok(d),
    });
    let after = mem();
    println!(
        "  造好 {n} 字节（{} MB）的 Decoded：提交 +{:.1}（这一份本来就是交出去、由宿主持有的）",
        mb(n),
        mid.priv_mb - before.priv_mb
    );
    println!(
        "  喂给 `apply`（→ install）之后：提交再 +{:.1} ← 这就是 install 在交出去那份之外**又自己留下的量**",
        after.priv_mb - mid.priv_mb
    );
    println!(
        "  判据（一份整图 = {:.1} MB）：≈0~1 → base 与显示档共享、鸟瞰已是缩略；\
         ≈140 → 又物化了一份整图尺寸的位图（白拷或鸟瞰，看 ⑨ 那行「鸟瞰那份」的源尺寸分辨）",
        mb(n)
    );
    drop(v3);
    drop(rx3);
    let m = mem();
    println!("  交还后：提交 {:>6.1}", m.priv_mb);

    // ── 汇总 ───────────────────────────────────────────────────────────────
    let one_mb = mb(bytes_of(bw, bh));
    println!(
        "
──────── 汇总（{bw}×{bh}，一份整图 = {one_mb:.1} MB）────────
  全程峰值工作集 {:.1} MB；交还并裁剪后停在工作集 {:.1}（提交 {:.1}）。
  真程序打开同一张并放大到 ≥100% 时，常驻是这几份：
    ① base_rgba（原图档，C 换来的「缩放不回头找解码器」）      一份整图
    ② image = 与 ① **同一块内存**（M1 共享，0 字节）          ← 原来是白拷一份
    ③ 主画布纹理：windui Image 的预乘 Pixmap                 一份整图（框架表示要求）
    ④ 鸟瞰纹理：≤512 缩略的一份                              ≈1 MB（M9 之前是**第二份整图**）
  另有解码瞬时（②③④ 之外的峰值）与预读邻居（各 ≈视口档，按张数封顶）。
  **更正上一轮的说法**：没有第四份「D2D 设备位图」。生产后端是 tiny-skia **软光栅**——
  windui 的 `Renderer::default()` 就是 Software，宿主从未调 `.renderer()`；所以补丁 011
  真机生效的是 skia 那条臂（d2d 那条臂是给将来切 GPU 后端备用的）。
  DLL 边界本身**不多花一份**：输出缓冲由宿主按 bytes_needed 分配、解码器直接写进来，
  to_rgba8 对 PF_RGBA8 走零拷贝——所以「用了 DLL 就要两份」这个前提并不成立。
  对照 pineapple-pictures：也只持有 1 份，而且它同样是 CPU 光栅——差别不在 GPU，
  在「缩放是改矩阵还是物化成新位图」：它的旋转/缩放是 QTransform 矩阵、>100% 由光栅器
  只采样可见区，因此既不预读也不留第二副本。",
        m0.peak, m2.ws, m2.priv_mb
    );
}
