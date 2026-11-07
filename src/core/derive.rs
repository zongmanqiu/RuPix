//! 档位本地派生（"C"的另一半）：把**按源尺寸计价**的重采样挪出 UI 线程。
//!
//! 存在理由（2026-10-17 用户报障"放大过程中卡顿更明显"）：主画面已经持有原图档时，
//! 换档不再回头找解码器，只在宿主本地面积采样——但那一次采样的成本是**读完整份源**：
//! 实测 6055×6055 → 1200×900 净 0.118–0.131 s，全花在调用线程上
//! （见 `HISTORY.md` 与 `docs/踩坑记录/踩坑记录.md`）。UI 线程被这一刀按住的时候，窗口就是
//! "糊着不动"。本模块把同一件活儿交给一条专属线程：UI 侧只**投递**（微秒级）并继续
//! 画旧档，新档采好了再通过 [`Viewer::poll_levels`](crate::core::viewer::Viewer::poll_levels)
//! 换上去——换的仍是同一份像素，观感与同步版一致，只是不在 UI 线程上算。
//!
//! 为什么单独一条线程、不并进 `decode` 的队列：那条队列的活儿是**调解码器 DLL**，
//! 一次原图档 TIFF 要 0.9 s；本地派生排在那后面就要陪着等，而它根本不需要解码线程
//! 的任何状态。两条队列各自的取消语义也不同（解码按 `main_floor` 作废，派生按
//! `epoch` 作废），混在一起只会让 K46 那套插队更难读。
//!
//! 线程只保**一份在途**：取任务时把队列里更新的都吞掉、只算最新的一条——用户连滚
//! 五格滚轮，中间那四档没有一帧被看过，采它是白烧 CPU。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::core::geometry::{downscale, fit_dims, transformed};
use crate::core::pixels::{RgbaImage, MAX_TEXTURE_SIDE};

/// 上屏位图的**单边封顶**：GPU 纹理建不出来的尺寸（D3D11 = 16384）。
///
/// 等比缩放（长边顶到上限），所以画面比例不变——只是 1:1 时略糊。
/// ★ 为什么必须有它：解码上限抬到 64 MP 之后，"总量在限内、单边超限"的图出现了
/// （30000×2000 的全景图 = 60 MP）。D3D11 建不出那么宽的纹理 ⇒ windui 上传失败 ⇒
/// 宿主当时的处理是"本轮留白" ⇒ 用户看到一片空白加一行日志，比"打不开"更难排查。
/// 主动降边 = 画面照常显示。
pub fn cap_texture_side(w: u32, h: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= MAX_TEXTURE_SIDE || long == 0 {
        return (w, h);
    }
    let k = MAX_TEXTURE_SIDE as f64 / long as f64;
    (
        ((w as f64 * k).round() as u32).max(1),
        ((h as f64 * k).round() as u32).max(1),
    )
}

/// 从整图 `base`（`bw × bh`）派生一份上屏位图，返回 `(派生档, 位图)`。
///
/// 先按 `shrink` 把 base 面积采样到显示档 `want`，再做 rot/flip——**先缩后转**，
/// 旋转是按像素搬的，先缩能少搬几千万个像素。恒等变换（不旋不翻，绝大多数时候）
/// 直接复用缩放结果，省掉一整份拷贝：36MP 的图上那是 140 MB。
///
/// **显示档 == 原图档时与 `base` 共享同一块缓冲**（`RgbaImage::shared`，M1）：
/// 放大到 ≥100% 后像素一个都不必动，原来这里走 `base.to_vec()` 白拷一份，实测
/// 多占 139.9 MB、白跑 0.028 s（判据见 本仓历史记录）。
///
/// 派生档是**未经旋转**的尺寸（`image` 被 `rot` 交换过轴，脏判定不能拿它直接比）。
pub fn derive_level(
    base: &Arc<Vec<u8>>,
    bw: u32,
    bh: u32,
    want: (u32, u32),
    rot: u8,
    flip: bool,
    shrink: bool,
) -> ((u32, u32), RgbaImage) {
    // 切片态（SVG）不做本地缩放：位图就是那一块的原样像素，密度记在 `Region` 里，
    // 再缩等于把矢量渲染成果抹掉。
    let (fw, fh) = if shrink {
        fit_dims(bw, bh, want.0, want.1)
    } else {
        (bw, bh)
    };
    // ★ **显示侧单边封顶**（GPU 纹理上限，见 [`cap_texture_side`]）：超了就等比降边，
    //   宁可 1:1 时略糊，也不要"建不出纹理 → 本轮留白"那种白屏。
    //   切片态的画布本来就被 `region_target` 按 `MAX_EDGE` 夹过一遍，这里是兜底。
    let (fw, fh) = cap_texture_side(fw, fh);
    let identity = rot == 0 && !flip;
    if (fw, fh) == (bw, bh) {
        if identity {
            return ((bw, bh), RgbaImage::shared(bw, bh, Arc::clone(base)));
        }
        let (tw, th, bytes) = transformed(base, bw, bh, rot, flip);
        return ((fw, fh), RgbaImage::new(tw, th, bytes));
    }
    let scaled = downscale(base, bw, bh, fw, fh);
    if identity {
        return ((fw, fh), RgbaImage::new(fw, fh, scaled));
    }
    let (tw, th, bytes) = transformed(&scaled, fw, fh, rot, flip);
    ((fw, fh), RgbaImage::new(tw, th, bytes))
}

/// 一份"把 `base` 采成 `want` 档"的委托。`epoch` 是宿主的**装图代号**：
/// 换了图（装了新的解码结果）之后回包一律作废，晚到的旧档不许盖掉新图。
pub struct LevelJob {
    pub epoch: u64,
    pub base: Arc<Vec<u8>>,
    pub decoded: (u32, u32),
    pub want: (u32, u32),
    pub rot: u8,
    pub flip: bool,
    /// `false` = 切片态，原样像素不许再缩。
    pub shrink: bool,
}

/// [`LevelJob`] 的结果。带着任务原样回传，宿主据此判断"这还是不是我要的那一档"。
pub struct Derived {
    pub epoch: u64,
    pub want: (u32, u32),
    pub rot: u8,
    pub flip: bool,
    pub shown: (u32, u32),
    pub image: RgbaImage,
}

/// 派生线程的四个计数（`§18` ⑫）。加它们是因为 m66 的结论里有一句**推算不是数**：
/// "一次派生 103–215 ms ⇒ 这段手势最多算了几档"是从成本除出来的，线程实际上算了几档、
/// 其中几档被合流吞掉、几档算完才发现画面已经不要，**当时一个都读不出来**。
///
/// 四条合起来才是那笔账：`posted − collapsed` = 真送去采的档数，`discarded` = 采完却白烧的。
/// 只做计数，不改任何行为；`Ordering::Relaxed`（这些数互相之间没有先后关系，只要最终可见）。
#[derive(Debug, Default)]
struct Counters {
    posted: std::sync::atomic::AtomicU64,
    /// 投递后被"只算最新一档"的合流吞掉的任务数（**没烧 CPU**，是省下的）。
    collapsed: std::sync::atomic::AtomicU64,
    computed: std::sync::atomic::AtomicU64,
    /// 算完了、但回包时 `want` 已经不是画面要的那一档 ⇒ 白烧。
    discarded: std::sync::atomic::AtomicU64,
}

/// 四个计数的快照（读出来就是一张表，探针与单测用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LevelCounts {
    pub posted: u64,
    pub collapsed: u64,
    pub computed: u64,
    pub discarded: u64,
}

/// 派生线程的宿主侧句柄。**只有一条线程**：并发派生会互相抢内存带宽，
/// 而用户一次只看一档——多开只是把同一份源多读几遍。
pub struct LevelThread {
    tx: Sender<LevelJob>,
    rx: Receiver<Derived>,
    counters: std::sync::Arc<Counters>,
}

impl LevelThread {
    /// 起线程。失败（系统线程数耗尽）由调用方**退回同步派生**并记日志——
    /// 这里不 panic，也不静默吞掉：画面停在旧档比程序崩好，但必须留痕。
    pub fn spawn() -> Result<Self, std::io::Error> {
        let (job_tx, job_rx) = channel::<LevelJob>();
        let (done_tx, done_rx) = channel::<Derived>();
        let counters = std::sync::Arc::new(Counters::default());
        let c_thread = std::sync::Arc::clone(&counters);
        std::thread::Builder::new()
            .name("sv-level".into())
            .spawn(move || loop {
                let Ok(first) = job_rx.recv() else {
                    return;
                };
                // 只算最新的那一档：中间档没被看过，采它是白烧 CPU。
                let mut job = first;
                while let Ok(newer) = job_rx.try_recv() {
                    job = newer;
                    c_thread
                        .collapsed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                let (shown, image) = derive_level(
                    &job.base,
                    job.decoded.0,
                    job.decoded.1,
                    job.want,
                    job.rot,
                    job.flip,
                    job.shrink,
                );
                c_thread
                    .computed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let out = Derived {
                    epoch: job.epoch,
                    want: job.want,
                    rot: job.rot,
                    flip: job.flip,
                    shown,
                    image,
                };
                if done_tx.send(out).is_err() {
                    return;
                }
            })
            .map(|_| Self {
                tx: job_tx,
                rx: done_rx,
                counters,
            })
    }

    /// 投递一档。`false` = 句柄已失效（不该发生，调用方据此退回同步路径）。
    pub fn post(&self, job: LevelJob) -> bool {
        if self.tx.send(job).is_ok() {
            self.counters
                .posted
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// 取一份**已经到了**的结果；没有就立刻返回 `None`（绝不阻塞 UI 线程）。
    pub fn take(&mut self) -> Option<Derived> {
        self.rx.try_recv().ok()
    }

    /// 宿主判回包过期、把它丢掉时补这一颗钉（唯一的调用点是 `Viewer::poll_levels`）。
    /// 为什么钉在宿主侧而不是线程里：线程算完时就**还不知道**画面要不要这一档，
    /// "过期"只有在 `poll_levels` 拿当前 `inflight` 比的那一瞬间才判得出来。
    pub fn note_discarded(&self) {
        self.counters
            .discarded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 四个计数的快照。
    pub fn counts(&self) -> LevelCounts {
        let g = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
        LevelCounts {
            posted: g(&self.counters.posted),
            collapsed: g(&self.counters.collapsed),
            computed: g(&self.counters.computed),
            discarded: g(&self.counters.discarded),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 本地派生档位（C 的核心动作）：只面积采样、**不放大**、恒等变换不多拷一份、
    /// 切片态不再缩第二刀；而返回的"派生档"始终是**未旋转**的轴（脏判定要用它）。
    ///
    /// M1 的另一半判据在这里：**显示档等于原图档且不变换时必须与 `base` 同一块内存**
    /// （`Arc::ptr_eq`），而真需要新像素的两条路（重采样、旋转）必须**不是**同一块——
    /// 后者若错共享，旋转后的像素会写穿原图档，base 就再也回不去了。
    #[test]
    fn derive_fits_to_display_box_without_upscaling() {
        let base: Arc<Vec<u8>> = Arc::new(vec![0x40u8; 400 * 300 * 4]);

        // 400×300 装进 100×100 的框 → 等比 100×75（fit_dims 口径）。
        let (shown, img) = derive_level(&base, 400, 300, (100, 100), 0, false, true);
        assert_eq!(shown, (100, 75), "派生档按等比缩");
        assert_eq!((img.width, img.height), (100, 75), "位图与派生档同尺寸");
        assert!(
            !Arc::ptr_eq(&base, &img.rgba),
            "重采样出的新像素必须独立成一块，不许与 base 共享"
        );

        // 要得比原图还大：绝不放大（放大是上屏层的活儿，解码侧给不出更多细节）。
        let (shown2, img2) = derive_level(&base, 400, 300, (4000, 3000), 0, false, true);
        assert_eq!(shown2, (400, 300), "不许把位图放大到超过原图");
        assert_eq!((img2.width, img2.height), (400, 300));
        assert!(
            Arc::ptr_eq(&base, &img2.rgba),
            "≥100% 档像素与 base 完全相同，必须共享同一块（M1：这里曾经整份 to_vec，实测白占 139.9 MB）"
        );

        // SVG 切片态：位图就是那一块的原样像素，密度记在 Region 里，再缩等于抹掉矢量成果。
        let (shown3, img3) = derive_level(&base, 400, 300, (100, 100), 0, false, false);
        assert_eq!(shown3, (400, 300), "切片态不做本地缩放");
        assert_eq!(img3.rgba.len(), base.len());
        assert!(
            Arc::ptr_eq(&base, &img3.rgba),
            "切片态原样收下，同样不许多拷一份"
        );

        // 旋转 90°：位图被换轴，而记账的派生档仍是未旋转尺寸——
        // 脏判定拿 `image.width` 比就会在 90° 下永远对不上，每帧重派生一次。
        let (shown4, img4) = derive_level(&base, 400, 300, (400, 400), 1, false, true);
        assert_eq!(shown4, (400, 300), "派生档记未旋转轴");
        assert_eq!((img4.width, img4.height), (300, 400), "位图按 rot 换轴");
        assert!(
            !Arc::ptr_eq(&base, &img4.rgba),
            "变换产出的是新像素，共享 base 会让旋转写穿原图档"
        );
    }

    /// 线程那条路：投一档要能拿到结果，且结果把任务参数原样带回（宿主靠它判新旧）。
    #[test]
    fn level_thread_roundtrip_carries_job_identity() {
        let mut t = match LevelThread::spawn() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  起不了派生线程（环境不允许），跳过：{e}");
                return;
            }
        };
        let base: Arc<Vec<u8>> = Arc::new(vec![0x7Fu8; 200 * 100 * 4]);
        assert!(
            t.post(LevelJob {
                epoch: 7,
                base: Arc::clone(&base),
                decoded: (200, 100),
                want: (50, 25),
                rot: 0,
                flip: false,
                shrink: true,
            }),
            "投递必须成功"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let got = loop {
            if let Some(d) = t.take() {
                break d;
            }
            if Instant::now() > deadline {
                panic!("派生线程 5 s 没回结果");
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(
            (got.epoch, got.want, got.rot, got.flip),
            (7, (50, 25), 0, false)
        );
        assert_eq!(got.shown, (50, 25), "回包的派生档要能直接对账");
        assert_eq!((got.image.width, got.image.height), (50, 25));
        // 像素真是采出来的：本例源全 0x7F，采样后仍是它（面积采样不造新值）。
        assert_eq!(got.image.rgba[0], 0x7F);
    }

    /// 连投多档：只算**最新**那一档，中间档不许被当成结果换到屏幕上。
    ///
    /// 为什么值得钉：吞旧任务是那句 `while try_recv`，改成一件事一次循环就等于
    /// 用户滚五格滚轮时多烧四倍 CPU，而且画面最后停在中间某一档（不是最新那格）。
    ///
    /// ★ 前面那颗"堵路"大活（epoch 99）不是装饰。m67 之前这里就是四档连投，线程若在
    ///   第一档之后就醒来，`(200,200)` 会被**单独算一遍**——做计数突变时当场量到这条抖
    ///   （回包 `[200, 60]`，判据①红）。堵一条它一定要忙的活，四档才必然排在队列里，
    ///   合流分支才是真的被执行到（K90：夹具必须落在被测分支里面）。
    #[test]
    fn level_thread_collapses_to_the_newest_job() {
        let mut t = match LevelThread::spawn() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  起不了派生线程（环境不允许），跳过：{e}");
                return;
            }
        };
        let base: Arc<Vec<u8>> = Arc::new(vec![0x11u8; 400 * 300 * 4]);
        let big: Arc<Vec<u8>> = Arc::new(vec![0x22u8; 2400 * 1600 * 4]);
        assert!(
            t.post(LevelJob {
                epoch: 99,
                base: Arc::clone(&big),
                decoded: (2400, 1600),
                want: (1200, 800),
                rot: 0,
                flip: false,
                shrink: true,
            }),
            "堵路档投递必须成功"
        );
        // 四档一路缩小，全在这条线程忙于上一档时进队列。
        for w in [200u32, 150, 100, 60] {
            assert!(t.post(LevelJob {
                epoch: 1,
                base: Arc::clone(&base),
                decoded: (400, 300),
                want: (w, w),
                rot: 0,
                flip: false,
                shrink: true,
            }));
        }
        // 等到"每一档要么被吞、要么被算"为止（计数在另一条线程里加，得给它时间收敛）。
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        let counts = loop {
            while let Some(d) = t.take() {
                if d.epoch == 1 {
                    seen.push(d.want);
                }
            }
            let c = t.counts();
            if c.collapsed + c.computed == c.posted {
                break c;
            }
            if Instant::now() > deadline {
                panic!("五档投进去，吞＋算对不上账（等满 5 s）：{c:?}，回包 {seen:?}");
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(
            seen,
            vec![(60, 60)],
            "中间档被单独算了一遍，吞旧任务没生效（计数 {counts:?}）"
        );
        // ⑫：合流吞掉的档数必须如实记账。上面那步已经把账等平了，这里可以要精确值——
        // 线程要么"先接大活再吞三档"（3），要么"五档全排队、一次吞四档"（4）。
        assert_eq!(counts.posted, 5, "五档全投成功");
        assert_eq!(counts.discarded, 0, "没人丢回包 ⇒ 白烧必须是 0");
        assert!(
            counts.collapsed >= 3,
            "只算最新一档 ⇒ 至少吞掉三档，实测吞 {}（{counts:?}）",
            counts.collapsed
        );
        assert_eq!(
            counts.computed,
            5 - counts.collapsed,
            "算的档数 = 投的 − 吞的（{counts:?}）"
        );
    }

    /// 结果的**接收端**被丢弃 = 宿主不再要结果，线程要能自己收（不留僵尸线程）。
    ///
    /// 钉的是那句 `if done_tx.send(..).is_err() { return }`：宿主退出时丢的是
    /// `LevelThread`（连着 `rx`），若线程只盯着任务队列，它会一直算下去、发不出去，
    /// 变成一条烧 CPU 的孤儿。
    #[test]
    fn level_thread_exits_when_result_receiver_dropped() {
        let (job_tx, job_rx) = channel::<LevelJob>();
        let (done_tx, done_rx) = channel::<Derived>();
        let h = std::thread::Builder::new()
            .name("sv-level-test".into())
            .spawn(move || loop {
                let Ok(job) = job_rx.recv() else { return };
                let (shown, image) = derive_level(
                    &job.base,
                    job.decoded.0,
                    job.decoded.1,
                    job.want,
                    job.rot,
                    job.flip,
                    job.shrink,
                );
                let out = Derived {
                    epoch: job.epoch,
                    want: job.want,
                    rot: job.rot,
                    flip: job.flip,
                    shown,
                    image,
                };
                if done_tx.send(out).is_err() {
                    return;
                }
            })
            .expect("测试线程应能起");
        drop(done_rx); // 模拟宿主把结果接收端丢了
        let base: Arc<Vec<u8>> = Arc::new(vec![0u8; 4 * 4 * 4]);
        assert!(job_tx
            .send(LevelJob {
                epoch: 0,
                base,
                decoded: (4, 4),
                want: (2, 2),
                rot: 0,
                flip: false,
                shrink: true,
            })
            .is_ok());
        // 线程应在有限时间内自己退出。
        let started = Instant::now();
        while !h.is_finished() && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(h.is_finished(), "结果没人要时线程必须自收，不留僵尸");
    }

    /// ★ **显示侧单边封顶**：上屏位图不许超过 GPU 纹理建得出来的尺寸。
    ///
    /// 背景（2026-10-28）：解码上限抬到 64 MP 之后，"总量在限内、单边超限"的图出现了
    /// （30000×2000 的全景图 = 60 MP）。D3D11 建不出 16384 以上的单边 ⇒ 纹理创建失败 ⇒
    /// 宿主当时是"本轮留白"⇒ 用户看到一片空白。所以显示位图要**主动降边**。
    ///
    /// 判据三条：① 长边顶到上限；② 等比（比例不变，画面不歪）；③ 短边跟着缩。
    #[test]
    fn display_bitmap_is_capped_to_texture_side() {
        // ① 长边超限 ⇒ 等比降边。
        assert_eq!(cap_texture_side(30_000, 2_000), (16_384, 1_092));
        // ② 比例不变（宽高比守得住，差在取整）。
        let (w, h) = cap_texture_side(30_000, 2_000);
        let got = w as f64 / h as f64;
        let want = 30_000.0 / 2_000.0;
        assert!((got - want).abs() < 0.01, "比例必须守住：{got} vs {want}");
        // ③ 在限内的尺寸**原样不动**（绝大多数图走这一条，不能被顺手降一刀）。
        assert_eq!(cap_texture_side(800, 600), (800, 600));
        assert_eq!(cap_texture_side(16_384, 16_384), (16_384, 16_384));
        // ④ 短边超限也一样降（竖长图）。
        assert_eq!(cap_texture_side(2_000, 30_000), (1_092, 16_384));
    }

    /// 派生路径必须真的用上那道封顶（`cap_texture_side` 单独测不够：
    /// 派生档是上屏位图的唯一来源，绕过去就等于没封）。
    #[test]
    fn derive_level_applies_texture_side_cap() {
        // 造一张"单边超限"的底：宽 30000、高 2（只要 240 KB 像素，不是真图）。
        let base: Arc<Vec<u8>> = Arc::new(vec![0x40u8; 30_000 * 2 * 4]);
        let (shown, img) = derive_level(&base, 30_000, 2, (30_000, 2), 0, false, true);
        assert!(
            shown.0 <= MAX_TEXTURE_SIDE && shown.1 <= MAX_TEXTURE_SIDE,
            "上屏位图的单边不许超过显卡纹理上限：{shown:?}"
        );
        assert_eq!((img.width, img.height), shown);
    }
}
