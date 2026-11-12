//! 主辅视图的辅图 pane（m322）：只负责“目标 + 异步解码 + 成品”。
//!
//! 主图仍由 `core::viewer::Viewer` 管；辅图不走第二套 Viewer/调度器，避免与主图
//! 抢同一份解码队列。这里用一次性的 `probe_decode` / `probe_decode_frame` 线程，
//! 成品带世代回投；目标没变就不重复解码，旧世代的结果直接丢弃。
//!
//! 布局、标题、底栏、主图联动都在 `ui::canvas`；本模块不碰窗口几何。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use crate::core::gallery::{Gallery, SortSpec};
use crate::core::pixels::{Anim, RgbaImage};

/// 辅图来源：主序列下一项 / 用户选定的独立序列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompanionSource {
    #[default]
    NextInPrimary,
    OwnSequence,
}

/// 辅图翻页方式：跟随主图 / 固定 / 自由（辅图自己翻）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkMode {
    #[default]
    Linked,
    Pinned,
    /// 不跟主图、也不冻结：辅图在自己的清单里自由前进/后退。
    Free,
}

/// 当前活动窗（决定右栏徽标高亮；翻页键始终作用于主图）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Primary,
    Companion,
}

/// 主图翻页的步长：联动 + 下一张来源 = 2；自选来源 = 1；固定辅图 = 1。
pub fn pair_step(source: CompanionSource, link: LinkMode) -> isize {
    match (source, link) {
        (CompanionSource::NextInPrimary, LinkMode::Linked) => 2,
        _ => 1,
    }
}

/// 自选辅图项的短标签：`父目录\文件`；过长只留尾部，避免菜单被长路径撑爆。
pub fn companion_path_label(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path
        .parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let full = match dir {
        Some(dir) if !dir.is_empty() => format!("{dir}\\{name}"),
        _ => name,
    };
    const MAX: usize = 48;
    if full.chars().count() <= MAX {
        full
    } else {
        let tail: String = full
            .chars()
            .skip(full.chars().count() - (MAX - 1))
            .collect();
        format!("…{tail}")
    }
}

/// 辅图目标：一个文件，或页式文件里的某一页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionTarget {
    pub path: PathBuf,
    /// `None` = 普通图片文件；`Some(page)` = 页式文件的 0 起页号。
    pub page: Option<u32>,
    /// 解码目标框（右栏逻辑像素）；只影响首屏档，不改变原图尺寸语义。
    pub max_w: u32,
    pub max_h: u32,
}

/// 辅图状态：目标、成品、在途请求。
#[derive(Default)]
pub struct CompanionPane {
    target: Option<CompanionTarget>,
    image: Option<RgbaImage>,
    /// 成品的**原图**尺寸。解码器原生降采样可能把位图缩成别的比例；
    /// 上屏按 natural 算 dst，而不是按位图尺寸，否则会顺着坏比例拉伸。
    natural: Option<(u32, u32)>,
    /// 最近一张成品的帧/页元信息（决定页号、页总数、按钮组）。
    anim: Option<Anim>,
    /// 自选辅图自己的页/帧游标（0 起）；换文件时归零。
    index: u32,
    /// 游标所属的文件；换了文件就重算。
    index_path: Option<PathBuf>,
    gen: u64,
    /// 常驻解码线程：**不能每次解码临时起线程**——doc-pdf 的解析缓存是
    /// thread_local，换线程就等于把 30 MB 的 PDF 重新解析一遍。
    worker: Option<DecodeWorker>,
    pending: bool,
    prefetch_pending: bool,
    /// 预读延迟到当前成品显示一会儿之后再发：连按翻页时根本不启动，
    /// 不会出现“当前页排在预读页后面等解码锁”的卡顿。
    prefetch_at: Option<Instant>,
    /// 在途预读的键；当前目标正好是它时不再重复发 Current。
    prefetch_key: Option<PrefetchKey>,
    /// `NextInPrimary` 没有自己的清单，由画布算出“下一项的下一项”传进来。
    prefetch_hint: Option<PrefetchKey>,
    failed: bool,
    source: CompanionSource,
    picked: Option<PathBuf>,
    sort: Option<SortSpec>,
    gallery: Option<Gallery>,
    offset: Option<isize>,
    /// 最近解过的辅图（路径/页 + 成品 + 请求框）；命中且分辨率够才直接出图。
    cache: Vec<(PathBuf, Option<u32>, CompanionShot, u32, u32)>,
}

type PrefetchKey = (PathBuf, Option<u32>);

/// 一张已经解好的辅图：像素 + 原图尺寸 + 解码器回填的帧/页信息。
struct CompanionShot {
    image: RgbaImage,
    natural: (u32, u32),
    anim: Anim,
}

enum WorkerJob {
    Current {
        gen: u64,
        target: CompanionTarget,
    },
    Prefetch {
        key: PrefetchKey,
        target: CompanionTarget,
    },
}

enum WorkerResult {
    Current {
        gen: u64,
        shot: Option<CompanionShot>,
    },
    Prefetch {
        key: PrefetchKey,
        shot: Option<CompanionShot>,
        max_w: u32,
        max_h: u32,
    },
}

/// 常驻辅图解码线程：保持 thread_local 解码器缓存（PDF 解析结果）不丢。
struct DecodeWorker {
    jobs: Sender<WorkerJob>,
    results: Receiver<WorkerResult>,
}

impl DecodeWorker {
    fn new() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<WorkerJob>();
        let (res_tx, res_rx) = mpsc::channel::<WorkerResult>();
        std::thread::spawn(move || {
            while let Ok(mut job) = job_rx.recv() {
                // ★ 只保留最新请求：辅图滚轮缩放会在几帧内连发多个 Current，
                //   队列里那些旧档位没有一个会被用上，却要按 FIFO 各解一遍，
                //   新档位排在一串慢解码后面 —— 用户看到的就是“缩放卡住”。
                //   新 Current 永远赢；Current 在等时不为 Prefetch 让路。
                while let Ok(next) = job_rx.try_recv() {
                    if matches!(&next, WorkerJob::Current { .. }) {
                        let old = std::mem::replace(&mut job, next);
                        cancel_prefetch(&res_tx, old);
                    } else if matches!(&job, WorkerJob::Current { .. }) {
                        cancel_prefetch(&res_tx, next);
                    } else {
                        let old = std::mem::replace(&mut job, next);
                        cancel_prefetch(&res_tx, old);
                    }
                }
                let result = match job {
                    WorkerJob::Current { gen, target } => WorkerResult::Current {
                        gen,
                        shot: decode_target(&target),
                    },
                    WorkerJob::Prefetch { key, target } => {
                        let (max_w, max_h) = (target.max_w, target.max_h);
                        WorkerResult::Prefetch {
                            key,
                            shot: decode_target(&target),
                            max_w,
                            max_h,
                        }
                    }
                };
                if res_tx.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            jobs: job_tx,
            results: res_rx,
        }
    }

    fn submit_current(&self, gen: u64, target: CompanionTarget) {
        let _ = self.jobs.send(WorkerJob::Current { gen, target });
    }

    fn submit_prefetch(&self, key: PrefetchKey, target: CompanionTarget) {
        let _ = self.jobs.send(WorkerJob::Prefetch { key, target });
    }
}

/// 被丢弃的预读请求要回一个空结果：UI 的 `prefetch_pending` 只在收到回包时清，
/// 不回就再也发不出下一次预读。
fn cancel_prefetch(res_tx: &Sender<WorkerResult>, job: WorkerJob) {
    if let WorkerJob::Prefetch { key, target } = job {
        let _ = res_tx.send(WorkerResult::Prefetch {
            key,
            shot: None,
            max_w: target.max_w,
            max_h: target.max_h,
        });
    }
}

/// 这些矢量格式把 `target_w/h` 当作**画布尺寸**直接拉伸，不按图元比例适配；
/// 交给它们一个 pane 框就会变形。改为先按自然尺寸解，再由宿主 `fit_to` 等比缩。
fn stretches_to_target(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("emf" | "emz" | "wmf" | "wmz")
    )
}

/// 解一张辅图：普通文件走原生降采样；裸像素回退到能读参数的同步探针。
fn decode_target(t: &CompanionTarget) -> Option<CompanionShot> {
    let stretch = stretches_to_target(&t.path);
    let decoded = if crate::core::decode::is_headerless_path(&t.path) {
        crate::core::decode::probe_decode(&t.path, t.max_w, t.max_h)
    } else if stretch {
        match t.page {
            Some(page) => crate::core::decode::probe_decode_frame(&t.path, 0, 0, page),
            None => crate::core::decode::probe_decode(&t.path, 0, 0),
        }
    } else {
        match t.page {
            Some(page) => crate::core::decode::probe_scaled_frame(&t.path, t.max_w, t.max_h, page),
            None => crate::core::decode::probe_scaled(&t.path, t.max_w, t.max_h),
        }
    };
    decoded.ok().map(|d| {
        let natural = d.natural;
        let b = d.bitmap;
        // 拉伸型矢量：自然尺寸解完后由宿主等比缩进 pane 框。
        let b = if stretch {
            crate::core::geometry::fit_to(b.rgba, b.width, b.height, t.max_w, t.max_h)
        } else {
            b
        };
        CompanionShot {
            image: RgbaImage::new(b.width, b.height, b.rgba),
            natural,
            anim: d.anim,
        }
    })
}

impl CompanionPane {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_target(&self) -> Option<&CompanionTarget> {
        self.target.as_ref()
    }

    pub fn image(&self) -> Option<&RgbaImage> {
        self.image.as_ref()
    }

    /// 当前辅图的**原图**尺寸（不是解码位图尺寸）。上屏比例以它为准。
    pub fn natural(&self) -> Option<(u32, u32)> {
        self.natural
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    /// 当前辅图有没有可切的页/帧（解码器回填的 `Anim::has_index()`）。
    pub fn has_index(&self) -> bool {
        self.anim.is_some_and(|a| a.has_index())
    }

    pub fn anim_info(&self) -> Option<Anim> {
        self.anim
    }

    /// 辅图自己清单里的文件数（自由翻页时决定左右箭头是否出现）。
    pub fn gallery_len(&self) -> usize {
        self.gallery.as_ref().map_or(0, |g| g.len())
    }

    /// 底栏状态：活动窗是辅图时用。格式与主图一致：
    /// `文件进度 · 缩放% (页/帧进度)`；不再带「辅 ·」前缀。
    /// `fallback_file` 给 `NextInPrimary` 这种没有自己清单的来源，用主图清单
    /// 推算辅图所在位置（下一张 / 同文件下一页）。
    pub fn status(&self, scale: f32, fallback_file: Option<(usize, usize)>) -> String {
        let Some(_t) = self.target.as_ref() else {
            return "没有下一张".to_string();
        };
        let (pos, len) = match &self.gallery {
            Some(g) if !g.is_empty() => (g.position(), g.len()),
            _ => fallback_file.unwrap_or((0, 0)),
        };
        let mut out = if len > 0 {
            format!("{pos}/{len}")
        } else {
            String::new()
        };
        if self.image.is_none() {
            if self.failed {
                return if out.is_empty() {
                    "无法解码".to_string()
                } else {
                    format!("{out} · 无法解码")
                };
            }
            return if out.is_empty() {
                "载入中…".to_string()
            } else {
                format!("{out} · 载入中…")
            };
        }
        let pct = (scale * 100.0).round() as i32;
        if out.is_empty() {
            out = format!("{pct}%");
        } else {
            out.push_str(&format!(" · {pct}%"));
        }
        if let Some(a) = self.anim.filter(|a| a.has_index()) {
            let total = if a.count > 0 {
                format!("/{}", a.count)
            } else {
                String::new()
            };
            out.push_str(&format!(" ({}{})", a.index + 1, total));
        }
        out
    }

    /// 翻辅图自己的页/帧。两种来源都允许：主图翻页不再带着辅图走，
    /// 辅图的页码只能由用户在这里显式改。
    pub fn step_index(&mut self, delta: i32) -> bool {
        let Some(anim) = self.anim else {
            return false;
        };
        if !anim.has_index() || anim.count == 0 {
            return false;
        }
        let Some(target) = self.target.as_mut() else {
            return false;
        };
        let next = (anim.index as i32 + delta).rem_euclid(anim.count as i32) as u32;
        target.page = Some(next);
        self.index = next;
        self.index_path = Some(target.path.clone());
        self.anim = Some(Anim {
            index: next,
            ..anim
        });
        self.gen = self.gen.wrapping_add(1);
        self.image = None;
        self.natural = None;
        self.failed = false;
        self.pending = false;
        self.prefetch_at = None;
        true
    }

    /// 自由翻页：在辅图自己的清单里前进/后退一格（会回绕）。
    /// 调用方负责只在 `LinkMode::Free` 时调用。
    pub fn step_file(&mut self, delta: i32) -> bool {
        let Some(g) = self.gallery.as_mut() else {
            return false;
        };
        if g.is_empty() {
            return false;
        }
        let n = g.len() as isize;
        let cur = g.position() as isize - 1;
        let next = (cur + delta as isize).rem_euclid(n) as usize;
        if !g.jump_to(next) {
            return false;
        }
        self.index = 0;
        self.index_path = None;
        self.anim = None;
        self.image = None;
        self.natural = None;
        self.failed = false;
        self.pending = false;
        self.prefetch_at = None;
        self.gen = self.gen.wrapping_add(1);
        true
    }

    /// 按号跳辅图自己的页/帧（0 起）；「跳转到…帧/页」对话框用。
    pub fn jump_index(&mut self, index: u32) -> bool {
        let Some(anim) = self.anim else {
            return false;
        };
        if !anim.has_index() || anim.count == 0 || index >= anim.count {
            return false;
        }
        let Some(target) = self.target.as_mut() else {
            return false;
        };
        if anim.index == index {
            return true;
        }
        target.page = Some(index);
        self.index = index;
        self.index_path = Some(target.path.clone());
        self.anim = Some(Anim { index, ..anim });
        self.gen = self.gen.wrapping_add(1);
        self.image = None;
        self.natural = None;
        self.failed = false;
        self.pending = false;
        self.prefetch_at = None;
        true
    }

    /// 切换来源：主序列下一项 / 用户选定的独立序列。
    /// 排序口径变了也重建（与主窗同一套排序）。
    pub fn set_source(&mut self, source: CompanionSource, picked: Option<&Path>, sort: SortSpec) {
        if self.source == source && self.picked.as_deref() == picked && self.sort == Some(sort) {
            return;
        }
        self.source = source;
        self.picked = picked.map(Path::to_path_buf);
        self.sort = Some(sort);
        self.offset = None;
        self.gallery = if source == CompanionSource::OwnSequence {
            picked.and_then(|p| Gallery::from_file(p, sort).ok())
        } else {
            None
        };
        self.target = None;
        self.image = None;
        self.natural = None;
        self.anim = None;
        self.index = 0;
        self.index_path = None;
        self.failed = false;
        self.gen = self.gen.wrapping_add(1);
        self.pending = false;
        self.prefetch_pending = false;
        self.prefetch_at = None;
        self.prefetch_key = None;
        self.prefetch_hint = None;
        self.cache.clear();
    }

    /// 交换主辅后把旧主图钉成辅图：强制按 `path` 重建清单，并把页游标落到
    /// 旧主图当时那一页。`set_source` 在同源同文件时会早退，不能承担这次重建。
    pub fn pin_to(&mut self, path: &Path, sort: SortSpec, page: u32) {
        self.source = CompanionSource::OwnSequence;
        self.picked = Some(path.to_path_buf());
        self.sort = Some(sort);
        self.gallery = Gallery::from_file(path, sort).ok();
        self.offset = None;
        self.target = None;
        self.image = None;
        self.natural = None;
        self.anim = None;
        self.index = page;
        self.index_path = Some(path.to_path_buf());
        self.failed = false;
        self.pending = false;
        self.prefetch_pending = false;
        self.prefetch_at = None;
        self.prefetch_key = None;
        self.prefetch_hint = None;
        self.gen = self.gen.wrapping_add(1);
        self.cache.clear();
    }

    /// 按当前来源算目标。`NextInPrimary` 用主图给的下一项/下一页；
    /// `OwnSequence` 在联动时按“主图位置 + 初始偏移”取自己的清单，
    /// 固定时停在用户选的那一张。
    pub fn resolve_target(
        &mut self,
        link: LinkMode,
        primary_pos: usize,
        next_path: Option<PathBuf>,
        next_page: Option<u32>,
    ) -> Option<(PathBuf, Option<u32>)> {
        match self.source {
            CompanionSource::NextInPrimary => {
                // ★ 自由翻页：辅图要有自己的清单才能“自己翻”。第一次解析时
                //   以主图给的下一项为锚点建一份，之后不再看主图换到哪。
                if link == LinkMode::Free {
                    let path = if self.gallery.is_none() {
                        let seed = next_path?;
                        let sort = self.sort?;
                        self.gallery = Gallery::from_file(&seed, sort).ok();
                        self.index = 0;
                        self.index_path = None;
                        self.anim = None;
                        seed
                    } else {
                        self.gallery.as_mut()?.current().cloned()?
                    };
                    if self.index_path.as_deref() != Some(path.as_path()) {
                        self.index = 0;
                        self.index_path = Some(path.clone());
                        self.anim = None;
                    }
                    return Some((path, Some(self.index)));
                }
                // ★ 固定辅图：一旦有了目标就原地不动，主图换文件也不跟。
                //   旧写法只看来源不看 `link`，于是「固定辅图」在默认来源下
                //   仍会跟着主图换图——用户拿 zip 实测到的就是这一条。
                if link == LinkMode::Pinned {
                    if let Some(t) = self.target.as_ref() {
                        return Some((t.path.clone(), t.page));
                    }
                }
                let path = next_path?;
                // ★ 同一文件内翻页时辅图不跟：页游标冻结在首次解析出的那一页，
                //   只有主图换了**文件**才重新取“下一页”。否则主图按一次 `,`，
                //   右栏那页也跟着走，两边就没法对照看了。
                let page = match self.target.as_ref() {
                    Some(t) if t.path == path => t.page,
                    _ => next_page,
                };
                Some((path, page))
            }
            CompanionSource::OwnSequence => {
                let path = {
                    let g = self.gallery.as_mut()?;
                    if g.is_empty() {
                        return None;
                    }
                    if link != LinkMode::Linked {
                        g.current().cloned()
                    } else {
                        if self.offset.is_none() {
                            self.offset = Some(g.position() as isize - 1 - primary_pos as isize);
                        }
                        let n = g.len() as isize;
                        let idx = (primary_pos as isize + self.offset.unwrap_or(0)).rem_euclid(n)
                            as usize;
                        let path = g.peek(idx).cloned();
                        let _ = g.jump_to(idx);
                        path
                    }
                }?;
                // 换文件才归零；同一文件内翻页保留游标和缩放。
                if self.index_path.as_deref() != Some(path.as_path()) {
                    self.index = 0;
                    self.index_path = Some(path.clone());
                    self.anim = None;
                }
                Some((path, Some(self.index)))
            }
        }
    }

    /// 切换目标；同一路径/页且已有成品/在途请求时零动作。
    /// 但窗口或辅图倍率变大到需要更多像素时，会按新框重解一次（只升不降）。
    /// 先查预读缓存；命中即出图，再去预读下一组。
    pub fn set_target(&mut self, target: Option<CompanionTarget>) {
        let same_content = match (&target, &self.target) {
            (Some(a), Some(b)) => a.path == b.path && a.page == b.page,
            (None, None) => true,
            _ => false,
        };
        let has_state = self.image.is_some() || self.pending || self.failed;
        // 目标框长到原来的 1.5 倍以上才重解：普通窗口拖动不触发，缩放一档也不触发。
        let needs_detail = match (&target, &self.target) {
            (Some(a), Some(b)) if a.path == b.path && a.page == b.page => {
                a.max_w > b.max_w.saturating_add(b.max_w / 2)
                    || a.max_h > b.max_h.saturating_add(b.max_h / 2)
            }
            _ => false,
        };
        if same_content && has_state && !needs_detail {
            return;
        }
        // ★ 只是“框变大了要重解”（缩放辅图）时**保留旧成品**：先把旧图拉伸上屏，
        //   新成品到了再无感替换。旧写法在这里 `image = None`，每跨一档缩放
        //   右栏就白一下、等解码，用户感知就是“缩放卡顿”。
        let detail_only = same_content && has_state && needs_detail;
        self.gen = self.gen.wrapping_add(1);
        let gen = self.gen;
        self.target = target.clone();
        if !detail_only {
            self.image = None;
            self.natural = None;
        }
        self.failed = false;
        self.pending = false;
        // 目标变了：作废尚未到点的预读；已在途的那张留给队列自然结束。
        self.prefetch_at = None;
        let Some(target) = target else {
            return;
        };
        if let Some(shot) = self.take_cache(&target) {
            self.anim = Some(shot.anim);
            self.natural = Some(shot.natural);
            if self.index_path.as_deref() == Some(target.path.as_path()) {
                self.index = shot.anim.index;
            }
            self.image = Some(shot.image);
            self.schedule_prefetch();
            return;
        }
        // 当前要的正好是正在预读的那一张：不再发 Current，等预读结果直接上屏。
        if self.prefetch_key.as_ref() == Some(&(target.path.clone(), target.page)) {
            self.pending = true;
            return;
        }
        self.pending = true;
        self.worker
            .get_or_insert_with(DecodeWorker::new)
            .submit_current(gen, target);
    }

    /// 每帧调用：收当前成品与预读成品；旧世代（目标已换）丢弃。
    pub fn poll(&mut self) {
        let mut got_current = false;
        let mut disconnected = false;
        let mut results = Vec::new();
        if let Some(worker) = &self.worker {
            loop {
                match worker.results.try_recv() {
                    Ok(result) => results.push(result),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for result in results {
            match result {
                WorkerResult::Current { gen, shot } => {
                    if gen != self.gen {
                        continue;
                    }
                    self.pending = false;
                    got_current = true;
                    match shot {
                        Some(shot) => {
                            let natural = shot.natural;
                            if let Some(t) = self.target.as_ref() {
                                self.push_cache(
                                    t.path.clone(),
                                    t.page,
                                    CompanionShot {
                                        image: shot.image.clone(),
                                        natural,
                                        anim: shot.anim,
                                    },
                                    t.max_w,
                                    t.max_h,
                                );
                            }
                            self.anim = Some(shot.anim);
                            self.natural = Some(natural);
                            if self.index_path.as_deref()
                                == self.target.as_ref().map(|t| t.path.as_path())
                            {
                                self.index = shot.anim.index;
                            }
                            self.failed = false;
                            self.image = Some(shot.image);
                        }
                        None => {
                            self.anim = None;
                            self.natural = None;
                            self.failed = true;
                            self.image = None;
                        }
                    }
                }
                WorkerResult::Prefetch {
                    key,
                    shot,
                    max_w,
                    max_h,
                } => {
                    self.prefetch_pending = false;
                    self.prefetch_key = None;
                    let applied = self
                        .target
                        .as_ref()
                        .is_some_and(|t| t.path == key.0 && t.page == key.1)
                        && self.image.is_none();
                    if let Some(shot) = shot {
                        let natural = shot.natural;
                        if applied {
                            self.anim = Some(shot.anim);
                            self.natural = Some(natural);
                            self.image = Some(shot.image.clone());
                            self.failed = false;
                            self.pending = false;
                        }
                        self.push_cache(key.0, key.1, shot, max_w, max_h);
                    } else if applied {
                        self.anim = None;
                        self.natural = None;
                        self.failed = true;
                        self.pending = false;
                    }
                }
            }
        }
        if disconnected {
            self.pending = false;
            self.prefetch_pending = false;
            self.prefetch_key = None;
            self.failed = true;
            self.natural = None;
        }
        if got_current {
            self.schedule_prefetch();
        }
        self.run_due_prefetch();
    }

    fn take_cache(&mut self, target: &CompanionTarget) -> Option<CompanionShot> {
        let pos = self.cache.iter().position(|(p, page, _, w, h)| {
            *p == target.path
                && *page == target.page
                && *w + 32 >= target.max_w
                && *h + 32 >= target.max_h
        })?;
        Some(self.cache.swap_remove(pos).2)
    }

    fn push_cache(
        &mut self,
        path: PathBuf,
        page: Option<u32>,
        shot: CompanionShot,
        max_w: u32,
        max_h: u32,
    ) {
        if let Some(i) = self
            .cache
            .iter()
            .position(|(p, pg, _, _, _)| *p == path && *pg == page)
        {
            // 同目标已有缓存：只有新成品分辨率更高才替换。
            let (_, _, _, old_w, old_h) = &self.cache[i];
            if max_w.saturating_mul(max_h) <= old_w.saturating_mul(*old_h) {
                return;
            }
            self.cache.remove(i);
        }
        self.cache.push((path, page, shot, max_w, max_h));
        if self.cache.len() > 4 {
            self.cache.remove(0);
        }
    }

    /// 画布算好的 `NextInPrimary` 预读目标（下一项的下一项/下一页）。
    pub fn set_prefetch_hint(&mut self, hint: Option<PrefetchKey>) {
        self.prefetch_hint = hint;
    }

    /// 当前成品显示一会儿之后再预读，避免连按翻页时预读抢解码锁。
    fn schedule_prefetch(&mut self) {
        if self.prefetch_pending || self.prefetch_key.is_some() || self.prefetch_at.is_some() {
            return;
        }
        self.prefetch_at = Some(Instant::now() + Duration::from_millis(250));
    }

    fn run_due_prefetch(&mut self) {
        let due = self.prefetch_at.is_some_and(|t| Instant::now() >= t);
        if !due {
            return;
        }
        self.prefetch_at = None;
        if !self.pending && !self.prefetch_pending && self.prefetch_key.is_none() {
            self.prefetch_next();
        }
    }

    /// 预读“下一组”的辅图：普通文件读下一张；页式/动图读下一页/帧。
    /// 页式只预读**一张**，且延迟到成品稳定后再发；固定不预读。
    fn prefetch_next(&mut self) {
        if self.prefetch_pending {
            return;
        }
        let Some(cur) = self.target.as_ref().cloned() else {
            return;
        };
        let next = match self.source {
            CompanionSource::NextInPrimary => {
                // 自由翻页已有自己的清单：预读清单下一张。
                if let Some(g) = self.gallery.as_mut() {
                    if g.is_empty() {
                        return;
                    }
                    let idx = g.position() % g.len();
                    g.peek(idx).cloned().map(|p| (p, Some(0)))
                } else {
                    // 跟随/固定：画布给了“下一项的下一项/下一页”的现成目标。
                    self.prefetch_hint.take()
                }
            }
            CompanionSource::OwnSequence => {
                if let Some(a) = self.anim.filter(|a| a.has_index() && a.count > 0) {
                    let idx = (a.index as i32 + 1).rem_euclid(a.count as i32) as u32;
                    Some((cur.path.clone(), Some(idx)))
                } else {
                    let Some(g) = self.gallery.as_mut() else {
                        return;
                    };
                    if g.is_empty() {
                        return;
                    }
                    let idx = g.position() % g.len();
                    g.peek(idx).cloned().map(|p| (p, Some(0)))
                }
            }
        };
        let Some(key) = next else {
            return;
        };
        if self
            .cache
            .iter()
            .any(|(p, pg, _, _, _)| *p == key.0 && *pg == key.1)
            || self
                .target
                .as_ref()
                .is_some_and(|t| t.path == key.0 && t.page == key.1)
        {
            return;
        }
        let target = CompanionTarget {
            path: key.0.clone(),
            page: key.1,
            max_w: cur.max_w,
            max_h: cur.max_h,
        };
        self.prefetch_pending = true;
        self.prefetch_key = Some(key.clone());
        self.worker
            .get_or_insert_with(DecodeWorker::new)
            .submit_prefetch(key, target);
    }

    /// 顶栏标题：文件名 + 页/帧号（活动窗是辅图时用，替代主图文件名）。
    pub fn file_title(&self) -> Option<String> {
        let t = self.target.as_ref()?;
        let name = t.path.file_name()?.to_string_lossy().to_string();
        match self.anim.filter(|a| a.has_index()) {
            Some(a) => {
                let total = if a.count > 0 {
                    format!("/{}", a.count)
                } else {
                    String::new()
                };
                Some(format!("{name} · {}{}", a.index + 1, total))
            }
            None => Some(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = crate::core::paths::test_scratch_root()
            .join(format!("companion-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("建辅图测试目录");
        for name in ["01.svg", "02.svg", "03.svg", "04.svg"] {
            std::fs::write(p.join(name), b"<svg/>").expect("写样本");
        }
        p
    }

    fn name(t: Option<(PathBuf, Option<u32>)>) -> Option<String> {
        t.and_then(|(p, _)| p.file_name().map(|n| n.to_string_lossy().to_string()))
    }

    #[test]
    fn own_sequence_links_by_offset_and_pinned_stays_put() {
        let dir = scratch("offset");
        let mut pane = CompanionPane::new();
        pane.set_source(
            CompanionSource::OwnSequence,
            Some(&dir.join("02.svg")),
            SortSpec::DEFAULT,
        );
        // 主图 0（01）时辅图 02；主图 2（03）时辅图 04——偏移 +1。
        assert_eq!(
            name(pane.resolve_target(LinkMode::Linked, 0, None, None)),
            Some("02.svg".to_string())
        );
        assert_eq!(
            name(pane.resolve_target(LinkMode::Linked, 2, None, None)),
            Some("04.svg".to_string())
        );
        // 固定：钉住当前辅图（此时已跟到 04）。
        assert_eq!(
            name(pane.resolve_target(LinkMode::Pinned, 2, None, None)),
            Some("04.svg".to_string())
        );
        // 新开一个 pane 选 02 后固定，钉住 02。
        let mut fixed = CompanionPane::new();
        fixed.set_source(
            CompanionSource::OwnSequence,
            Some(&dir.join("02.svg")),
            SortSpec::DEFAULT,
        );
        assert_eq!(
            name(fixed.resolve_target(LinkMode::Pinned, 2, None, None)),
            Some("02.svg".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pair_step_matches_source_and_link() {
        assert_eq!(
            pair_step(CompanionSource::NextInPrimary, LinkMode::Linked),
            2
        );
        assert_eq!(pair_step(CompanionSource::OwnSequence, LinkMode::Linked), 1);
        assert_eq!(
            pair_step(CompanionSource::NextInPrimary, LinkMode::Pinned),
            1
        );
    }

    #[test]
    fn own_picked_file_keeps_its_own_page_cursor() {
        let dir = scratch("cursor");
        let mut pane = CompanionPane::new();
        pane.set_source(
            CompanionSource::OwnSequence,
            Some(&dir.join("02.svg")),
            SortSpec::DEFAULT,
        );
        assert_eq!(
            pane.resolve_target(LinkMode::Linked, 0, None, None),
            Some((dir.join("02.svg"), Some(0)))
        );
        // 模拟已解出的多页文件：当前页 0/3，再显式翻一页。
        pane.target = Some(CompanionTarget {
            path: dir.join("02.svg"),
            page: Some(0),
            max_w: 100,
            max_h: 100,
        });
        pane.anim = Some(Anim {
            index: 0,
            count: 3,
            delay_ms: 0,
            paged: true,
        });
        assert!(pane.has_index(), "多页文件应挂底栏页号与按钮");
        assert!(pane.step_index(1));
        assert_eq!(pane.current_target().and_then(|t| t.page), Some(1));
        assert_eq!(pane.anim_info().map(|a| a.index), Some(1));
        assert_eq!(
            pane.resolve_target(LinkMode::Linked, 0, None, None),
            Some((dir.join("02.svg"), Some(1)))
        );
        // 末页回绕。
        assert!(pane.step_index(-1));
        assert_eq!(pane.current_target().and_then(|t| t.page), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_in_primary_uses_the_supplied_next_item() {
        let mut pane = CompanionPane::new();
        pane.set_source(CompanionSource::NextInPrimary, None, SortSpec::DEFAULT);
        assert_eq!(
            name(pane.resolve_target(LinkMode::Linked, 0, Some(PathBuf::from("14.svg")), None,)),
            Some("14.svg".to_string())
        );
    }

    /// EMF/WMF 把 target 当画布直接拉伸，必须走“自然尺寸 + 宿主等比缩”的特殊路。
    #[test]
    fn vector_stretch_formats_are_detected() {
        for name in ["a.emf", "a.EMZ", "a.wmf", "a.wmz"] {
            assert!(
                stretches_to_target(Path::new(name)),
                "{name} 应按自然尺寸解"
            );
        }
        for name in ["a.pdf", "a.jpg", "a.svg"] {
            assert!(!stretches_to_target(Path::new(name)));
        }
    }

    /// 自由翻页：辅图在自己的清单里前进/后退，主图换什么都不再带走它。
    #[test]
    fn free_mode_steps_its_own_files() {
        let dir = scratch("free");
        let mut pane = CompanionPane::new();
        pane.set_source(
            CompanionSource::OwnSequence,
            Some(&dir.join("02.svg")),
            SortSpec::DEFAULT,
        );
        assert_eq!(
            name(pane.resolve_target(LinkMode::Free, 0, None, None)),
            Some("02.svg".to_string())
        );
        assert!(pane.step_file(1));
        assert_eq!(
            name(pane.resolve_target(LinkMode::Free, 3, Some(dir.join("99.svg")), None)),
            Some("03.svg".to_string()),
            "自由翻页后不再看主图给的 next"
        );
        assert!(pane.step_file(-1));
        assert_eq!(
            name(pane.resolve_target(LinkMode::Free, 0, None, None)),
            Some("02.svg".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 底栏信息与主图同构：文件进度 · 缩放% (页/帧进度)。
    #[test]
    fn status_reports_file_progress_zoom_and_page() {
        let dir = scratch("status");
        let mut pane = CompanionPane::new();
        pane.set_source(
            CompanionSource::OwnSequence,
            Some(&dir.join("02.svg")),
            SortSpec::DEFAULT,
        );
        pane.target = Some(CompanionTarget {
            path: dir.join("02.svg"),
            page: Some(0),
            max_w: 10,
            max_h: 10,
        });
        pane.anim = Some(Anim {
            index: 1,
            count: 5,
            delay_ms: 0,
            paged: true,
        });
        pane.image = Some(RgbaImage::new(10, 10, vec![0; 400]));
        assert_eq!(pane.status(0.5, None), "2/4 · 50% (2/5)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 交换主辅后旧主图要成为辅图，并保留旧主图当时那一页。
    #[test]
    fn pin_to_rebuilds_gallery_and_keeps_the_old_page() {
        let dir = scratch("pin-to");
        let mut pane = CompanionPane::new();
        let path = dir.join("02.svg");
        pane.pin_to(&path, SortSpec::DEFAULT, 3);
        assert_eq!(
            pane.resolve_target(LinkMode::Pinned, 0, None, None),
            Some((path, Some(3)))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「固定辅图」在主序列来源下也必须真固定：主图换文件不能把辅图带走。
    #[test]
    fn pinned_next_in_primary_stays_put_when_primary_changes_file() {
        let mut pane = CompanionPane::new();
        pane.set_source(CompanionSource::NextInPrimary, None, SortSpec::DEFAULT);
        let a = PathBuf::from("a.svg");
        assert_eq!(
            pane.resolve_target(LinkMode::Linked, 0, Some(a.clone()), None),
            Some((a.clone(), None))
        );
        pane.target = Some(CompanionTarget {
            path: a.clone(),
            page: None,
            max_w: 100,
            max_h: 100,
        });
        // 切到固定后，主图换到 b / c 都不动。
        assert_eq!(
            pane.resolve_target(LinkMode::Pinned, 0, Some(PathBuf::from("b.svg")), None),
            Some((a.clone(), None))
        );
        assert_eq!(
            pane.resolve_target(LinkMode::Pinned, 1, Some(PathBuf::from("c.svg")), None),
            Some((a, None))
        );
    }

    /// 主图在同一页式文件里翻页时，辅图页游标必须冻结：只换了文件才重取“下一页”。
    #[test]
    fn next_in_primary_freezes_page_within_the_same_file() {
        let mut pane = CompanionPane::new();
        pane.set_source(CompanionSource::NextInPrimary, None, SortSpec::DEFAULT);
        let doc = PathBuf::from("doc.pdf");
        // 首次解析：主图第 1 页 → 辅图第 2 页。
        assert_eq!(
            pane.resolve_target(LinkMode::Linked, 0, Some(doc.clone()), Some(1)),
            Some((doc.clone(), Some(1)))
        );
        // 模拟首帧成品已落地（生产里由 `set_target` 写入）。
        pane.target = Some(CompanionTarget {
            path: doc.clone(),
            page: Some(1),
            max_w: 100,
            max_h: 100,
        });
        pane.anim = Some(Anim {
            index: 1,
            count: 5,
            delay_ms: 0,
            paged: true,
        });
        // 主图翻到第 3 页：辅图仍停在第 2 页。
        assert_eq!(
            pane.resolve_target(LinkMode::Linked, 0, Some(doc.clone()), Some(3)),
            Some((doc.clone(), Some(1)))
        );
        // 辅图自己的页码仍可手动翻；主图再翻页也不会把它拉回去。
        assert!(pane.step_index(1));
        assert_eq!(
            pane.resolve_target(LinkMode::Linked, 0, Some(doc.clone()), Some(4)),
            Some((doc, Some(2)))
        );
        // 主图换文件：跟到新文件的下一页。
        assert_eq!(
            pane.resolve_target(
                LinkMode::Linked,
                0,
                Some(PathBuf::from("next.pdf")),
                Some(0)
            ),
            Some((PathBuf::from("next.pdf"), Some(0)))
        );
    }

    #[test]
    fn path_label_keeps_parent_and_name_within_width() {
        assert_eq!(
            companion_path_label(Path::new(r"C:\pics\sub\a.jpg")),
            r"sub\a.jpg"
        );
        let deep = format!(r"C:\{}\very-long-name.jpg", "d".repeat(80));
        let label = companion_path_label(Path::new(&deep));
        assert!(label.starts_with('…'));
        assert!(label.ends_with("very-long-name.jpg"));
        assert!(label.chars().count() <= 48);
    }
}
