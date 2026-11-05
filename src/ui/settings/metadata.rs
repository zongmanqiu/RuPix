//! 「文件信息」窗：属性/EXIF 信号表与 APNG 判定。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
use super::*;
use std::collections::HashMap;

/// 属性窗的实时文本信号。窗口内容用 `Element::label_signal` 绑定它们；图片切换
/// （翻页 / 打开新图 / 解码完成上屏）时由 [`refresh_metadata`] 更新。windui 的
/// `Signal::set` 会 App 级标脏所有窗口，故属性窗无需自己轮询即可跟随当前图。
///
/// `Signal` 是 `!Send + !Sync`（句柄含线程本地状态），不能进 `static` 全局——这里用
/// `thread_local!` 在 UI 主线程持有一份（整个 GUI 都跑在单线程，安全）。
#[derive(Clone)]
pub struct MetaSignals {
    filename: Signal<String>,
    kind: Signal<String>,
    location: Signal<String>,
    size: Signal<String>,
    mtime: Signal<String>,
    dims: Signal<String>,
    anim: Signal<String>,
    // EXIF「拍摄信息」段——由 `platform::exif::read_exif` 填充，缺失即空串。
    capture_time: Signal<String>,
    camera: Signal<String>,
    lens: Signal<String>,
    exposure: Signal<String>,
    focal: Signal<String>,
    gps: Signal<String>,
    /// 上次刷新时的图片世代号；与 `Viewer::image_gen` 不同才重算（每帧调用也几乎零成本）。
    /// 初值是 `u64::MAX`（**从不等于任何真实世代**）：建好信号集后第一次刷新必落数据，
    /// 不必依赖「开图一定把世代推到 1」这种实现细节。
    last_gen: Cell<u64>,
}

thread_local! {
    /// **每扇主窗一份**属性窗信号集（键 = `singleton::window_scope`）。
    ///
    /// ★ 从 `Option<MetaSignals>` 改成按窗分格（m232）：多开之后两扇窗可以各开一扇
    /// 属性窗，共用一份信号集的话后刷新的那扇会把前一扇覆盖掉——用户看到的是"我没动
    /// 那扇窗，它的属性却跟着变了"。窗口关闭时 `forget_window` 把这一格摘掉。
    static META: RefCell<HashMap<usize, MetaSignals>> = RefCell::new(HashMap::new());
    /// 上次读取 EXIF 的文件路径（每窗一份）；路径变化才重新读盘（翻页/打开新图时变）。
    /// 与 MetaSignals 分开存放——`Signal` 是共享句柄 clone 语义，而路径需要唯一权威源。
    static EXIF_LAST_PATH: RefCell<HashMap<usize, std::path::PathBuf>> = RefCell::new(HashMap::new());
}

/// 取（首次则初始化）**这一扇窗**的属性窗信号集。`Signal` 是全局句柄，复制即可跨闭包引用。
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
                last_gen: Cell::new(u64::MAX),
            })
            .clone()
    })
}

/// 用当前 viewer 状态刷新属性窗信号。`image_gen` 没变就直接返回。
///
/// 由 `crate::runtime::run` 的 `on_interval(POLL)`（16 ms）每轮调用：图片切换 /
/// 解码完成时 `Viewer::image_gen` 自增，这里重算一次并 `set` 信号，
/// 属性窗（若开着）随之刷新。
///
/// ★ 属性窗**从未开过**时整段跳过（`META` 为空即返回）：这一趟要 stat 文件、读
///   64 KiB 判 APNG、必要时解析 EXIF，还要写十几个信号——而 `Signal::set` 是
///   **App 级标脏所有窗口**（windui `signal.rs`），等于每翻一页都为一张没人看的
///   窗全画面重绘一次。首刷由 [`open_metadata`] 负责（它先建信号集再刷）。
/// ★ 每个 `set` 都走 [`set`] 助手（值没变不写）：同尺寸同类型的两张图翻页时
///   文案一字不差，没必要为此踢一次全 App 重绘。
pub fn refresh_open_metadata(viewer: &Rc<RefCell<Viewer>>) {
    // 观看者以框架的单例窗口登记表为准，**且要认是哪一扇窗的属性窗**（键带 scope）。
    let scope = super::super::singleton::window_scope(viewer);
    if windui::event::window_open(&super::super::singleton::scoped("metadata", scope)) {
        refresh_metadata(viewer);
    }
}

/// 主窗关掉时摘掉它那一格信号与 EXIF 记账（`WinRt::close` 调）。
///
/// 摘掉之后 [`refresh_metadata`] 对这一格直接收工，不再读盘——"窗口没了还每帧 stat
/// 一个文件、写十几个信号"是最容易被忽略的一类浪费。
pub fn forget_window(scope: usize) {
    META.with(|m| {
        m.borrow_mut().remove(&scope);
    });
    EXIF_LAST_PATH.with(|e| {
        e.borrow_mut().remove(&scope);
    });
}

pub fn refresh_metadata(viewer: &Rc<RefCell<Viewer>>) {
    let gen = viewer.borrow().image_gen();
    let scope = super::super::singleton::window_scope(viewer);
    META.with(|m| {
        let mut map = m.borrow_mut();
        // 上面那句注释里的「从未开过」：这一扇窗没有信号集就没有观看者，直接收工。
        let Some(st) = map.get_mut(&scope) else {
            return;
        };
        if st.last_gen.get() == gen {
            return;
        }
        st.last_gen.set(gen);

        let v = viewer.borrow();
        let Some(p) = v.current_path() else {
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
        set(&st.filename, file_name(&p));
        set(&st.kind, kind);
        set(&st.location, p.to_string_lossy().to_string());
        set(&st.size, size);
        set(&st.mtime, format!("{mtime} (UTC)"));
        set(&st.dims, dims);
        set(&st.anim, anim);

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

pub fn open_metadata(ctx: &mut EventCtx, viewer: &Rc<RefCell<Viewer>>) {
    let scope = super::super::singleton::window_scope(viewer);
    // 没有打开图片：弹一个静态提示窗（没数据可跟随）。
    if viewer.borrow().current_path().is_none() {
        ctx.open_window(
            Window::new("属性", 360, 120)
                // 信息窗客户区恒白（标题栏由系统绘制，不在其内）。
                .bg(to_color(BG))
                .fit_content(120, 400)
                .resizable(false)
                .centered(true)
                .owned(true)
                // ★ 不能与真正的属性窗共用 single key：windui 对已开的单例窗只聚焦、
                //   不重建 content，于是空态窗会永久挡住之后的真实属性窗。
                .single(super::super::singleton::scoped("metadata-empty", scope))
                .content(|| {
                    Element::col()
                        .padding(20)
                        .child(Element::label("没有打开图片"))
                }),
        );
        return;
    }
    // 有图：文本用信号绑定，属性窗跟随当前图（翻页 / 打开新图 / 解码完成都会刷新）。
    // 顺序是**先建信号集、再刷**：`refresh_metadata` 只刷已经存在的信号集（没人看
    // 时不读盘），首刷就得由这里把信号集立起来。
    let sig = meta_signals(scope);
    refresh_metadata(viewer);
    let anim = sig.anim;
    let capture_time = sig.capture_time;
    let camera = sig.camera;
    let lens = sig.lens;
    let exposure = sig.exposure;
    let focal = sig.focal;
    let gps = sig.gps;
    let exif_row = |label: &'static str, s: windui::signal::Signal<String>| {
        let sc = s;
        option_row(
            label,
            Element::label_signal(s)
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY)),
        )
        .visible_when(move || !sc.get().is_empty())
    };
    // 起窗高度走 windui 补丁 07 `fit_content`——与解码器详情窗同一机制。属性窗字段
    // 会随功能扩展增加（EXIF 8 行已经被上一版固定 280 高截断过），让布局引擎按
    // 固定宽量一次自然高再夹进 [200, 1100]，未来加字段无需重算窗口尺寸。
    ctx.open_window(
        Window::new("属性", 480, 280)
            // 信息窗客户区恒白（标题栏由系统绘制，不在其内）。
            .bg(to_color(BG))
            .fit_content(200, 1100)
            .resizable(true)
            .centered(true)
            .owned(true)
            .single(super::super::singleton::scoped("metadata", scope))
            .content(move || {
                Element::scroll()
                    .fill()
                    .padding_xy(16, PAGE_TOP_MARGIN)
                    .child(Element::col().fill().spacing(0).children(vec![
                                option_row(
                                    "文件名",
                                    Element::label_signal(sig.filename)
                                        .font_size(FONT_MD)
                                        .fg(to_color(TEXT_SECONDARY)),
                                ),
                                option_row(
                                    "类型",
                                    Element::label_signal(sig.kind)
                                        .font_size(FONT_MD)
                                        .fg(to_color(TEXT_SECONDARY)),
                                ),
                                option_row(
                                    "位置",
                                    Element::label_signal(sig.location)
                                        .font_size(FONT_MD)
                                        .fg(to_color(TEXT_SECONDARY)),
                                ),
                                option_row(
                                    "大小",
                                    Element::label_signal(sig.size)
                                        .font_size(FONT_MD)
                                        .fg(to_color(TEXT_SECONDARY)),
                                ),
                                option_row(
                                    "修改时间",
                                    Element::label_signal(sig.mtime)
                                        .font_size(FONT_MD)
                                        .fg(to_color(TEXT_SECONDARY)),
                                ),
                                option_row(
                                    "图像尺寸",
                                    Element::label_signal(sig.dims)
                                        .font_size(FONT_MD)
                                        .fg(to_color(TEXT_SECONDARY)),
                                ),
                                option_row(
                                    "动画",
                                    Element::label_signal(sig.anim)
                                        .font_size(FONT_MD)
                                        .fg(to_color(TEXT_SECONDARY)),
                                )
                                .visible_when(move || !anim.get().is_empty()),
                                exif_row("拍摄时间", capture_time),
                                exif_row("相机", camera),
                                exif_row("镜头", lens),
                                exif_row("曝光", exposure),
                                exif_row("焦距", focal),
                                exif_row("GPS", gps),
                            ]))
            }),
    );
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

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn host_audit_closed_metadata_does_not_refresh_retained_signals() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let viewer = Rc::new(RefCell::new(Viewer::new(
            Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
            cfg,
        )));
        let scope = super::super::super::singleton::window_scope(&viewer);
        let key = super::super::super::singleton::scoped("metadata", scope);
        assert!(!windui::event::window_open(&key));
        let sig = meta_signals(scope);
        sig.filename.set("retained".into());
        META.with(|m| m.borrow().get(&scope).unwrap().last_gen.set(u64::MAX));
        refresh_open_metadata(&viewer);
        assert_eq!(sig.filename.get(), "retained");
        // 首刷仍可在窗口尚未登记时执行。
        refresh_metadata(&viewer);
        assert_eq!(sig.filename.get(), "（无图片）");
        META.with(|m| {
            m.borrow_mut().remove(&scope);
        });
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
        let sa = super::super::super::singleton::window_scope(&a);
        let sb = super::super::super::singleton::window_scope(&b);
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
