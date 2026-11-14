//! 信息面板属性卡的数据层：属性 / EXIF 信号表、刷新门控与 APNG 判定。
//!
//! ★ 2026-11-14 信息面板落地：原先住在 `ui/settings/metadata.rs` 的"每窗一份属性
//!   信号"随属性窗退役迁到本模块。属性窗时代由开窗动作建立信号集；现在信号集由面板
//!   装配建立，刷新由 [`refresh_metadata_if`] 的门控驱动——面板不可见或属性卡没开时
//!   **一个信号都不写**（不 stat 文件、不读 EXIF，也不惊动整窗重绘）。
//!
//! ★ `Signal` 是 `!Send + !Sync`（句柄含线程本地状态），不能进 `static` 全局——这里用
//!   `thread_local!` 在 UI 主线程持有一份（整个 GUI 都跑在单线程，安全）。
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use windui::signal::{signal, Signal};

use crate::core::decode::Meta;
use crate::core::pixels::{pixel_format_has_alpha, pixel_format_name, Histogram};
use crate::core::util::fmt_bytes;
use crate::core::viewer::{DisplayState, Viewer};

/// 属性卡的实时文本信号。面板内容用 `Element::label_signal` 绑定它们；图片切换
/// （翻页 / 打开新图 / 解码完成上屏）时由 [`refresh_metadata`] 更新。windui 的
/// `Signal::set` 会 App 级标脏所有窗口，故属性卡无需自己轮询即可跟随当前图。
///
/// 字段是 `pub(crate)`：面板装配（`ui::info_panel`）要直接绑定这些信号。
#[derive(Clone)]
pub struct MetaSignals {
    pub(crate) filename: Signal<String>,
    pub(crate) kind: Signal<String>,
    pub(crate) location: Signal<String>,
    pub(crate) size: Signal<String>,
    pub(crate) mtime: Signal<String>,
    pub(crate) dims: Signal<String>,
    pub(crate) anim: Signal<String>,
    // EXIF「拍摄信息」段——由 `platform::exif::read_exif` 填充，缺失即空串。
    pub(crate) capture_time: Signal<String>,
    pub(crate) camera: Signal<String>,
    pub(crate) lens: Signal<String>,
    pub(crate) exposure: Signal<String>,
    pub(crate) focal: Signal<String>,
    pub(crate) gps: Signal<String>,
    // 技术信息卡：解码器 / 位深 / 像素格式 / ICC / 方向 / 显示状态。
    pub(crate) tech_decoder: Signal<String>,
    pub(crate) tech_format: Signal<String>,
    pub(crate) tech_pixels: Signal<String>,
    pub(crate) tech_color: Signal<String>,
    pub(crate) tech_icc: Signal<String>,
    pub(crate) tech_orientation: Signal<String>,
    pub(crate) tech_view: Signal<String>,
    pub(crate) tech_frames: Signal<String>,
    // 直方图卡：数据 + 一行裁切统计。
    pub(crate) histogram: Signal<Histogram>,
    pub(crate) hist_summary: Signal<String>,
    /// 当前这张图是否真的有文件：无图时属性行整块隐藏，只显示"没有打开图片"。
    pub(crate) has_image: Signal<bool>,
    /// 上一拍"面板可见 + 属性卡 active"的取值；false→true 时强制刷新一次。
    ///
    /// ★ **只以 `META` 里那一格为准**，clone 出来的是按值复制的另一个 `Cell`——
    ///   读写 clone 只会读到它构造时的初值，证明不了门控。落点在
    ///   [`refresh_metadata_if`] 的 `map.get_mut(&scope)` 分支，不要从 `meta_signals()`
    ///   的返回值去断言这一位。
    pub(crate) enabled: Cell<bool>,
    /// 上次刷新时的图片世代号；与 `Viewer::image_gen` 不同才重算（每帧调用也几乎零成本）。
    /// 初值是 `u64::MAX`（**从不等于任何真实世代**）：建好信号集后第一次刷新必落数据，
    /// 不必依赖「开图一定把世代推到 1」这种实现细节。
    last_gen: Cell<u64>,
    /// 直方图单独的世代号：只统计像素，不读文件 / EXIF，与属性卡门控分开。
    last_hist_gen: Cell<u64>,
}

thread_local! {
    /// **每扇主窗一份**属性信号集（键 = `singleton::window_scope`）。
    ///
    /// ★ 按窗分格（m232）：多开之后两扇窗各绑一张属性卡，共用一份信号集的话后刷新的
    /// 那扇会把前一扇覆盖掉——用户看到的是"我没动那扇窗，它的属性却跟着变了"。
    /// 窗口关闭时 `forget_window` 把这一格摘掉。
    static META: RefCell<HashMap<usize, MetaSignals>> = RefCell::new(HashMap::new());
    /// 上次读取 EXIF 的文件路径（每窗一份）；路径变化才重新读盘（翻页/打开新图时变）。
    /// 与 MetaSignals 分开存放——`Signal` 是共享句柄 clone 语义，而路径需要唯一权威源。
    static EXIF_LAST_PATH: RefCell<HashMap<usize, std::path::PathBuf>> = RefCell::new(HashMap::new());
}

/// 取（首次则初始化）**这一扇窗**的属性信号集。`Signal` 是全局句柄，复制即可跨闭包引用。
pub fn meta_signals(scope: usize) -> MetaSignals {
    META.with(|m| {
        m.borrow_mut()
            .entry(scope)
            .or_insert_with(|| MetaSignals {
                filename: signal(String::new()),
                kind: signal(String::new()),
                location: signal(String::new()),
                size: signal(String::new()),
                mtime: signal(String::new()),
                dims: signal(String::new()),
                anim: signal(String::new()),
                capture_time: signal(String::new()),
                camera: signal(String::new()),
                lens: signal(String::new()),
                exposure: signal(String::new()),
                focal: signal(String::new()),
                gps: signal(String::new()),
                tech_decoder: signal(String::new()),
                tech_format: signal(String::new()),
                tech_pixels: signal(String::new()),
                tech_color: signal(String::new()),
                tech_icc: signal(String::new()),
                tech_orientation: signal(String::new()),
                tech_view: signal(String::new()),
                tech_frames: signal(String::new()),
                histogram: signal(Histogram::default()),
                hist_summary: signal(String::new()),
                has_image: signal(false),
                enabled: Cell::new(false),
                last_gen: Cell::new(u64::MAX),
                last_hist_gen: Cell::new(u64::MAX),
            })
            .clone()
    })
}

/// 属性 / 技术信息卡刷新门控：`wanted` = 面板可见且至少一张需要文件/元信息的卡开着。
///
/// `wanted` 为假时**一个信号都不写**：不 stat、不读 EXIF、不标脏。false→true 的那一拍
/// 把 `last_gen` 打回 `u64::MAX`，保证重新激活后即使图片世代没变也会重算一次——否则
/// 收起期间翻过的图会让卡片停在旧图上。
///
/// 由 `crate::runtime` 的 `global_tick`（16 ms 轮询）每拍调用。
pub fn refresh_metadata_if(viewer: &Rc<RefCell<Viewer>>, wanted: bool) {
    let scope = super::super::singleton::window_scope(viewer);
    META.with(|m| {
        let mut map = m.borrow_mut();
        let Some(st) = map.get_mut(&scope) else {
            return;
        };
        let was = st.enabled.replace(wanted);
        if wanted && !was {
            st.last_gen.set(u64::MAX);
        }
    });
    if wanted {
        refresh_metadata(viewer);
    }
}

/// 直方图刷新门控：`wanted` = 面板可见 + 直方图卡展开。
///
/// 与属性卡分开是因为这一趟只读像素：不 stat、不读 EXIF；收起卡片时把世代打回
/// `u64::MAX`，下次展开重算一次，不会拿收起期间那张旧图的分布充数。
pub fn refresh_histogram_if(viewer: &Rc<RefCell<Viewer>>, wanted: bool) {
    let scope = super::super::singleton::window_scope(viewer);
    let gen = viewer.borrow().image_gen();
    META.with(|m| {
        let mut map = m.borrow_mut();
        let Some(st) = map.get_mut(&scope) else {
            return;
        };
        if !wanted {
            st.last_hist_gen.set(u64::MAX);
            return;
        }
        if st.last_hist_gen.get() == gen {
            return;
        }
        st.last_hist_gen.set(gen);
        let hist = viewer.borrow().histogram().unwrap_or_default();
        set(&st.histogram, hist);
        let summary = if hist.count == 0 {
            "当前解码画面没有可统计的像素".to_string()
        } else {
            format!(
                "当前解码画面 {}×{} · 每 {} 像素取 1 点 · 均值 {:.0} · 中位 {} · 峰值 {}\n\
                 R {:.0} G {:.0} B {:.0} · 阴影 {:.1}% · 高光 {:.1}%",
                hist.width,
                hist.height,
                hist.step,
                hist.mean_luma(),
                hist.median_luma(),
                hist.peak_bin(),
                hist.mean_channel(&hist.r),
                hist.mean_channel(&hist.g),
                hist.mean_channel(&hist.b),
                hist.clip_low(),
                hist.clip_high()
            )
        };
        set(&st.hist_summary, summary);
    });
}

/// 主窗关掉时摘掉它那一格信号与 EXIF 记账（`WinRt::close` 调）。
///
/// 摘掉之后 [`refresh_metadata_if`] 对这一格直接收工，不再读盘——"窗口没了还每帧
/// stat 一个文件、写十几个信号"是最容易被忽略的一类浪费。
pub fn forget_window(scope: usize) {
    META.with(|m| {
        m.borrow_mut().remove(&scope);
    });
    EXIF_LAST_PATH.with(|e| {
        e.borrow_mut().remove(&scope);
    });
}

/// 用当前 viewer 状态刷新属性 / 技术信息信号。`image_gen` 没变时只补一次显示状态。
///
/// 图片切换 / 解码完成时 `Viewer::image_gen` 自增，这里重算一次并 `set` 信号，
/// 两张卡（若开着）随之刷新；缩放 / 旋转 / 镜像不一定推进世代，故 `tech_view` 每拍比对。
///
/// ★ 这一扇窗**没有信号集**时整段跳过（`META` 里没有这一格即返回）：这一趟要 stat
///   文件、读 64 KiB 判 APNG、必要时解析 EXIF，还要写十几个信号——而 `Signal::set`
///   是 **App 级标脏所有窗口**（windui `signal.rs`），等于每翻一页都为一张没人看的
///   卡全画面重绘一次。首刷由面板装配先建信号集、再经 [`refresh_metadata_if`] 触发。
/// ★ 每个 `set` 都走 [`set`] 助手（值没变不写）：同尺寸同类型的两张图翻页时
///   文案一字不差，没必要为此踢一次全 App 重绘。
pub fn refresh_metadata(viewer: &Rc<RefCell<Viewer>>) {
    let gen = viewer.borrow().image_gen();
    let scope = super::super::singleton::window_scope(viewer);
    META.with(|m| {
        let mut map = m.borrow_mut();
        // 上面那句注释里的「没有信号集」：这一扇窗没有属性卡就没有观看者，直接收工。
        let Some(st) = map.get_mut(&scope) else {
            return;
        };
        let v = viewer.borrow();
        // 缩放 / 旋转 / 镜像不一定推进 image_gen；每次刷新按值比对，变了才写信号。
        set(&st.tech_view, format_tech_view(v.display_state()));
        if st.last_gen.get() == gen {
            return;
        }
        st.last_gen.set(gen);
        let Some(p) = v.current_path() else {
            set(&st.has_image, false);
            set(&st.filename, "（无图片）".into());
            set(&st.kind, String::new());
            set(&st.location, String::new());
            set(&st.size, String::new());
            set(&st.mtime, String::new());
            set(&st.dims, String::new());
            set(&st.anim, String::new());
            set(&st.capture_time, String::new());
            set(&st.camera, String::new());
            set(&st.lens, String::new());
            set(&st.exposure, String::new());
            set(&st.focal, String::new());
            set(&st.gps, String::new());
            set(&st.tech_decoder, "未报告".into());
            set(&st.tech_format, String::new());
            set(&st.tech_pixels, String::new());
            set(&st.tech_color, "未报告".into());
            set(&st.tech_icc, "未报告".into());
            set(&st.tech_orientation, "未报告".into());
            set(&st.tech_frames, String::new());
            EXIF_LAST_PATH.with(|e| {
                e.borrow_mut().remove(&scope);
            });
            return;
        };
        let meta = std::fs::metadata(&p).ok();
        let size = meta
            .as_ref()
            .map(|m| fmt_bytes(m.len()))
            .unwrap_or_else(|| "未知".into());
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(format_utc)
            .unwrap_or_else(|| "未知".into());
        let dims = v
            .info_dims()
            .map(|(w, h)| format!("{w} × {h}"))
            .unwrap_or_else(|| "读取中…".into());
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        let is_apng = ext == "PNG" && is_apng_file(&p);
        let kind = if ext.is_empty() {
            "未知".into()
        } else if is_apng {
            "APNG 动画图像".into()
        } else {
            format!("{ext} 图像")
        };
        let tech_format = kind.clone();
        let anim = v
            .anim_summary()
            .map(|(count, delay_ms)| {
                let frames = if count > 0 {
                    format!("{count} 帧")
                } else {
                    "多帧（帧数未知）".into()
                };
                let dur = if count > 0 {
                    format!(" · 总时长 {:.1}s", count as f64 * delay_ms as f64 / 1000.0)
                } else {
                    String::new()
                };
                format!("{frames}{dur} · 帧延时 {delay_ms}ms")
            })
            .unwrap_or_default();
        set(&st.has_image, true);
        set(&st.filename, file_name(&p));
        set(&st.kind, kind);
        set(&st.location, p.to_string_lossy().to_string());
        set(&st.size, size);
        set(&st.mtime, format!("{mtime} (UTC)"));
        set(&st.dims, dims);
        set(&st.anim, anim);

        // 技术信息卡：解码器/位深/像素格式来自查询阶段的 Meta（渲染后 natural 已由
        // install 合并回缓存）；显示档尺寸与缩放/旋转来自 Viewer 现场值。
        let meta = v.render_meta();
        let meta_ref = meta.as_ref();
        set(
            &st.tech_decoder,
            meta_ref
                .map(|m| {
                    if m.decoder.is_empty() {
                        "未报告".to_string()
                    } else {
                        m.decoder.clone()
                    }
                })
                .unwrap_or_else(|| "未报告".into()),
        );
        set(&st.tech_format, tech_format);
        set(
            &st.tech_pixels,
            format_tech_pixels(v.info_dims(), v.decoded_dims()),
        );
        set(&st.tech_color, format_tech_color(meta_ref));
        set(&st.tech_icc, format_tech_icc(meta_ref));
        set(&st.tech_orientation, format_tech_orientation(meta_ref));
        set(&st.tech_frames, format_tech_frames(meta_ref));

        // EXIF 读取只在**路径变化**时进行——翻页/打开新图触发，缩放/平移等
        // 让 image_gen 变化的操作不重复读盘。
        let need_exif = EXIF_LAST_PATH
            .with(|e| e.borrow().get(&scope).map(|x| x.as_path()) != Some(p.as_path()));
        if need_exif {
            let info = crate::platform::exif::read_exif(&p);
            set(&st.capture_time, info.capture_time);
            set(&st.camera, info.camera);
            set(&st.lens, info.lens);
            set(&st.exposure, info.exposure);
            set(&st.focal, info.focal);
            set(&st.gps, info.gps);
            EXIF_LAST_PATH.with(|e| {
                e.borrow_mut().insert(scope, p.clone());
            });
        }
    });
}

/// 原图尺寸 → 解码档尺寸（面积百分比）。旋转过的图两轴可能对调，按面积算才准。
fn format_tech_pixels(natural: Option<(u32, u32)>, decoded: Option<(u32, u32)>) -> String {
    let Some((nw, nh)) = natural else {
        return "读取中…".into();
    };
    match decoded {
        Some((dw, dh)) if dw > 0 && dh > 0 && (dw, dh) != (nw, nh) => {
            let area = if nw > 0 && nh > 0 {
                (dw as f64 * dh as f64) / (nw as f64 * nh as f64) * 100.0
            } else {
                0.0
            };
            format!("{nw}×{nh} → {dw}×{dh}（面积 {area:.0}%）")
        }
        _ => format!("{nw}×{nh}"),
    }
}

fn format_tech_color(meta: Option<&Meta>) -> String {
    let Some(m) = meta else {
        return "未报告".into();
    };
    let pf = pixel_format_name(m.pixel_format);
    let bits = if m.bit_depth == 0 {
        "位深未报告".to_string()
    } else {
        format!("{} bit", m.bit_depth)
    };
    let alpha = if pixel_format_has_alpha(m.pixel_format) {
        "Alpha"
    } else {
        "无 Alpha"
    };
    format!("{pf} · {bits} · {alpha}")
}

fn format_tech_icc(meta: Option<&Meta>) -> String {
    let Some(m) = meta else {
        return "未报告".into();
    };
    if m.icc_len == 0 {
        "无内嵌 ICC · 按 sRGB 显示".into()
    } else {
        format!("内嵌 ICC（{}）· 输出 sRGB", fmt_bytes(m.icc_len as u64))
    }
}

fn format_tech_orientation(meta: Option<&Meta>) -> String {
    let Some(m) = meta else {
        return "未报告".into();
    };
    if m.orientation <= 1 {
        "正常（1）".into()
    } else if m.already_oriented {
        format!("EXIF {} · 解码器已校正", m.orientation)
    } else {
        format!("EXIF {} · 宿主已校正", m.orientation)
    }
}

fn format_tech_frames(meta: Option<&Meta>) -> String {
    let Some(m) = meta else {
        return String::new();
    };
    if m.page_count > 1 {
        format!("{} 页", m.page_count)
    } else if m.frame_count > 1 {
        format!("{} 帧 · {} ms", m.frame_count, m.frame_delay_ms)
    } else {
        String::new()
    }
}

fn format_tech_view(state: Option<DisplayState>) -> String {
    let Some(s) = state else {
        return String::new();
    };
    let mut out = format!("{:.0}% · 旋转 {}°", s.scale * 100.0, s.rotation as u32 * 90);
    if s.mirrored {
        out.push_str(" · 水平镜像");
    }
    out
}

pub fn format_utc(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

pub fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub fn is_apng_file(p: &std::path::Path) -> bool {
    use std::io::Read;

    const HEAD: usize = 64 * 1024;

    let mut f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut head = vec![0u8; HEAD];
    let head = match f.read(&mut head) {
        Ok(n) => &head[..n],
        Err(_) => return false,
    };
    is_apng_bytes(head)
}

pub fn is_apng_bytes(head: &[u8]) -> bool {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    if head.len() < SIG.len() || head[..8] != SIG {
        return false;
    }
    let mut pos = 8usize;
    while pos + 8 <= head.len() {
        let len = u32::from_be_bytes([head[pos], head[pos + 1], head[pos + 2], head[pos + 3]]);
        match &head[pos + 4..pos + 8] {
            b"acTL" => return true,
            b"IDAT" => return false,
            _ => {}
        }
        pos = match pos
            .checked_add(12)
            .and_then(|v| v.checked_add(len as usize))
        {
            Some(v) => v,
            None => return false,
        };
    }
    false
}

/// 值没变就不写信号：`Signal::set` 无条件标脏整 App，同值重绘纯属浪费。
fn set<T: PartialEq + Clone + 'static>(slot: &Signal<T>, v: T) {
    if slot.get() != v {
        slot.set(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::viewer::Viewer;

    #[test]
    fn inactive_panel_does_not_touch_signals() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
        let viewer = std::rc::Rc::new(std::cell::RefCell::new(Viewer::new(
            std::rc::Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
            cfg,
        )));
        let scope = crate::ui::singleton::window_scope(&viewer);
        let sig = meta_signals(scope);
        sig.filename.set("sentinel".into());
        // ★ `MetaSignals` 的 `Clone` 会把 `Cell` 按值复制，`sig.last_gen` 与 META 里那一格
        //   是两个独立 Cell——读 clone 只能读到构造时的初值，证明不了门控。这里直接读
        //   META 记账，并故意放一个非初值：被重置或被刷新都会当场露馅。
        let before = META.with(|m| {
            let mut map = m.borrow_mut();
            let st = map.get_mut(&scope).unwrap();
            st.last_gen.set(42);
            (st.enabled.get(), st.last_gen.get())
        });
        refresh_metadata_if(&viewer, false);
        let after = META.with(|m| {
            let map = m.borrow();
            let st = map.get(&scope).unwrap();
            (st.enabled.get(), st.last_gen.get())
        });
        assert_eq!(after, before, "门控为假时不该动 enabled / last_gen");
        assert_eq!(sig.filename.get(), "sentinel");
        forget_window(scope);
    }

    #[test]
    fn reactivating_panel_forces_one_refresh() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
        let viewer = std::rc::Rc::new(std::cell::RefCell::new(Viewer::new(
            std::rc::Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
            cfg,
        )));
        let scope = crate::ui::singleton::window_scope(&viewer);
        let sig = meta_signals(scope);
        refresh_metadata_if(&viewer, true);
        assert_eq!(sig.filename.get(), "（无图片）");
        assert!(!sig.has_image.get());
        // 同 gen 再刷新不重复读盘，但重新激活必须强制刷一次。
        sig.filename.set("stale".into());
        refresh_metadata_if(&viewer, false);
        refresh_metadata_if(&viewer, true);
        assert_eq!(sig.filename.get(), "（无图片）");
        forget_window(scope);
    }

    /// 两扇窗各有一份属性信号集：刷新这一扇不许动那一扇（m232）。
    #[test]
    fn each_window_refreshes_only_its_own_metadata() {
        let (tx1, _r1) = std::sync::mpsc::channel();
        let (tx2, _r2) = std::sync::mpsc::channel();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let a = Rc::new(RefCell::new(Viewer::new(
            Rc::new(crate::core::decode::Decoder::spawn_with(tx1)),
            cfg.clone(),
        )));
        let b = Rc::new(RefCell::new(Viewer::new(
            Rc::new(crate::core::decode::Decoder::spawn_with(tx2)),
            cfg,
        )));
        let sa = crate::ui::singleton::window_scope(&a);
        let sb = crate::ui::singleton::window_scope(&b);
        assert_ne!(sa, sb, "两扇活着的窗不可能共用一个作用域");
        let sig_a = meta_signals(sa);
        let sig_b = meta_signals(sb);

        refresh_metadata(&a);
        assert_eq!(sig_a.filename.get(), "（无图片）");
        assert_eq!(sig_b.filename.get(), "", "另一扇窗的信号一个字都不许被写");

        forget_window(sa);
        assert!(
            META.with(|m| m.borrow().get(&sa).is_none()),
            "关窗后这一格要摘掉（否则每帧还在为一个没人看的窗口读盘）"
        );
        assert!(
            META.with(|m| m.borrow().get(&sb).is_some()),
            "别人的格子不许动"
        );
        forget_window(sb);
    }

    /// `format_utc` 的边界：纪元当天 + 一个跨越闰日的已知刻度。
    ///
    /// 属性卡的时间列直接展示这串字，算错一天在真机上很难在当场看出来（用户要自己去
    /// 对不同文件的时间）；纯函数是唯一能钉死它的地方。
    #[test]
    fn format_utc_starts_at_the_epoch_and_crosses_a_leap_day() {
        use std::time::{Duration, UNIX_EPOCH};
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01 00:00:00");
        assert_eq!(
            format_utc(UNIX_EPOCH + Duration::from_secs(1_000_000_000)),
            "2001-09-09 01:46:40"
        );
        assert_eq!(
            format_utc(UNIX_EPOCH + Duration::from_secs(951_827_696)),
            "2000-02-29 12:34:56"
        );
    }

    /// 属性卡只显示叶子名：完整路径另占「位置」一行。
    #[test]
    fn file_name_returns_only_the_leaf() {
        assert_eq!(file_name(std::path::Path::new("a/b.png")), "b.png");
        assert_eq!(
            file_name(std::path::Path::new(r"C:\pics\风景.png")),
            "风景.png"
        );
        assert_eq!(file_name(std::path::Path::new("")), "");
    }

    /// `has_image = true` 这一支：临时真文件 + 现有测试解码器。
    ///
    /// 只验属性卡的**数据面**（has_image / 文件名 / 大小 / 修改时间 / 类型 / 位置），
    /// 不验解码上屏：`Viewer` 的解码请求投给 `spawn_with` 的空通道，没人回包也不影响
    /// 这组信号。`recent::record` 会因 `Viewer::open` 记一条，用 `recent::remove` 撤掉。
    #[test]
    fn refresh_metadata_fills_the_card_for_a_real_file() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let viewer = Rc::new(RefCell::new(Viewer::new(
            Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
            cfg,
        )));
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("info-panel-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("唯一一张.png");
        let bytes = png(&[(b"IDAT", &[0u8; 4])]);
        std::fs::write(&file, &bytes).unwrap();

        viewer.borrow_mut().open(&file);
        let scope = crate::ui::singleton::window_scope(&viewer);
        let sig = meta_signals(scope);
        refresh_metadata(&viewer);

        assert!(sig.has_image.get(), "有真文件时属性行整块要出现");
        assert_eq!(sig.filename.get(), "唯一一张.png");
        assert_eq!(sig.size.get(), fmt_bytes(bytes.len() as u64));
        assert_ne!(sig.mtime.get(), "未知 (UTC)", "真文件要读得到修改时间");
        assert_eq!(sig.kind.get(), "PNG 图像");
        assert!(
            sig.location.get().ends_with("唯一一张.png"),
            "位置行要带完整路径，实际：{}",
            sig.location.get()
        );

        // 查询阶段的 Meta 缓存后，技术信息卡要能读到解码器 / 位深 / ICC。
        // seq=0 走“非当前请求只入缓存”的分支；不需要真解码器。
        use crate::core::abi::PF_RGBA8;
        use crate::core::decode::MetaEvent;
        viewer.borrow_mut().apply_meta(MetaEvent {
            seq: 0,
            path: file.clone(),
            meta: Meta {
                natural: (4000, 3000),
                decoder: "jpeg 0.3.4".into(),
                bit_depth: 8,
                pixel_format: PF_RGBA8,
                orientation: 6,
                already_oriented: true,
                frame_count: 1,
                page_count: 1,
                frame_delay_ms: 0,
                icc_len: 2048,
            },
        });
        // 强制一次重算：同 gen 的 refresh_metadata 会早退，先把门控关再开。
        refresh_metadata_if(&viewer, false);
        refresh_metadata_if(&viewer, true);
        assert_eq!(sig.tech_decoder.get(), "jpeg 0.3.4");
        assert_eq!(sig.tech_color.get(), "RGBA8 · 8 bit · Alpha");
        assert_eq!(sig.tech_icc.get(), "内嵌 ICC（2.0 KB）· 输出 sRGB");
        assert_eq!(sig.tech_orientation.get(), "EXIF 6 · 解码器已校正");

        forget_window(scope);
        crate::core::recent::remove(&file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tech_info_formats_are_human_readable() {
        use crate::core::abi::PF_RGBA8;

        let meta = Meta {
            natural: (4000, 3000),
            decoder: "jpeg 0.3.4".into(),
            bit_depth: 8,
            pixel_format: PF_RGBA8,
            orientation: 6,
            already_oriented: false,
            frame_count: 1,
            page_count: 1,
            frame_delay_ms: 0,
            icc_len: 2048,
        };
        assert_eq!(
            format_tech_pixels(Some((4000, 3000)), Some((2000, 1500))),
            "4000×3000 → 2000×1500（面积 25%）"
        );
        assert_eq!(
            format_tech_pixels(Some((4000, 3000)), Some((4000, 3000))),
            "4000×3000"
        );
        assert_eq!(format_tech_color(Some(&meta)), "RGBA8 · 8 bit · Alpha");
        assert_eq!(
            format_tech_icc(Some(&meta)),
            "内嵌 ICC（2.0 KB）· 输出 sRGB"
        );
        assert_eq!(format_tech_orientation(Some(&meta)), "EXIF 6 · 宿主已校正");
        assert_eq!(format_tech_frames(Some(&meta)), "");
        assert_eq!(
            format_tech_view(Some(DisplayState {
                scale: 0.5,
                rotation: 1,
                mirrored: true,
            })),
            "50% · 旋转 90° · 水平镜像"
        );
        let mut animated = meta.clone();
        animated.frame_count = 12;
        animated.frame_delay_ms = 80;
        assert_eq!(format_tech_frames(Some(&animated)), "12 帧 · 80 ms");
        let mut paged = meta;
        paged.page_count = 3;
        paged.frame_count = 1;
        assert_eq!(format_tech_frames(Some(&paged)), "3 页");
        assert_eq!(format_tech_icc(None), "未报告");
        assert_eq!(format_tech_color(None), "未报告");
    }

    #[test]
    fn apng_has_actl_before_idat() {
        let b = png(&[(b"acTL", &[0u8; 8]), (b"IDAT", &[0u8; 4])]);
        assert!(is_apng_bytes(&b));
    }

    #[test]
    fn static_png_with_actl_bytes_inside_idat_is_not_animated() {
        let b = png(&[(b"IDAT", b"xxacTLxx")]);
        assert!(!is_apng_bytes(&b), "IDAT 数据里的 acTL 不该算动画控制块");
    }

    #[test]
    fn non_png_bytes_are_not_apng() {
        assert!(!is_apng_bytes(b"\xFF\xD8\xFF\xE0 JPEG really"));
        assert!(!is_apng_bytes(b""));
        assert!(!is_apng_bytes(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn truncated_or_bogus_length_does_not_panic() {
        let mut b = png(&[(b"IDAT", &[0u8; 4])]);
        b[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(!is_apng_bytes(&b));
        assert!(!is_apng_bytes(&png(&[])[..20]));
    }

    fn chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(ty);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0u8; 4]);
        v
    }

    fn png(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&chunk(b"IHDR", &[0u8; 13]));
        for (ty, data) in chunks {
            v.extend_from_slice(&chunk(ty, data));
        }
        v
    }
}
