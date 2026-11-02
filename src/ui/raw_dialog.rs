//! 裸数据参数窗：打开无魔数文件时收集 宽/高/像素布局。
//!
//! 裸像素文件没有头，但**首开不逼用户手填**：后缀给出布局，文件长度用常见宽度
//! 反推出整数高度，能推出就直接打开；推不出才弹参数窗（带推断/上次默认值）。
//! 打开后可在右键「RAW 参数…」里改，确认后参数写进会话 store
//! （[`crate::core::decode::set_raw_params`]）+「上次使用值」落盘
//! （[`crate::core::config::Config::raw_last`]），再走正常的 `viewer.open`。
//!
//! # 为什么在 UI 层拦截而不是解码线程
//!
//! 解码线程没有窗口上下文（windui 的 `open_window` 要 `EventCtx`）；且翻页进来的
//! 裸文件不该被反复弹窗打断。所以只在**显式打开**（命令行/拖拽/右键打开）时弹，
//! 翻页遇到没有参数的裸文件给引导性错误文案（见 `core::decode::decode_route`）。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windui::core::EventCtx;
use windui::prelude::*;

use crate::core::config::Config;
use crate::core::decode;
use crate::core::decode::RawParams;
use crate::core::pixels::MAX_EDGE;
use crate::core::viewer::Viewer;

use super::tokens::{to_color, BG, DIALOG_FIT_MIN, DIALOG_PAD};

/// 「宽 / 高」输入框的解析：只认 `1…MAX_EDGE` 的整数，别的（空、负号、字母、超大值）
/// 一律 `None` 交给调用方去说明——不猜、不静默、不悄悄夹到边界值。
fn parse_dim(s: &str) -> Option<u32> {
    let v: u32 = s.trim().parse().ok()?;
    (1..=MAX_EDGE).contains(&v).then_some(v)
}

/// 布局下拉项文案，下标即 [`LAYOUTS`] 的下标（不是 PF 值——落盘/传参前要映射）。
const LAYOUT_LABELS: [&str; 12] = [
    "GRAY8", "RGB8", "RGBA8", "GRAY16", "RGB16", "RGBA16", "BGRA8", "YUY2", "UYVY", "I420", "YV12",
    "NV12",
];
/// 下拉下标 → PF 布局值（与 [`crate::core::abi`] 的 PF_* 对齐，8 起仅输入用）。
const LAYOUTS: [u32; 12] = [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 12, 13];

/// 待弹参数窗的裸数据路径（UI 线程独占；`open_path` 尾插、`poll_pending` 头取——
/// FIFO，按拖入/命令行顺序弹，不搞 LIFO 逆序）。
/// 真正没有文件头的裸像素后缀。相机 RAW（CR2/CR3/NEF/3FR…）不在此列，
/// 它们有完整头，必须先走正常解码；只有这些后缀才允许自动推断/参数窗。
fn layout_index_for_ext(ext: &str) -> Option<usize> {
    decode::layout_index_for_ext(ext)
}

fn infer_params(path: &Path) -> Option<RawParams> {
    decode::infer_raw_params(path)
}

pub fn is_headerless_path(path: &Path) -> bool {
    decode::is_headerless_path(path)
}

/// 右键「RAW 参数…」是否可点：没有当前文件、或有文件头（相机 RAW / 普通图片）都置灰。
///
/// 菜单项本身**常驻**（用户 2026-10-28 口径）；这里只回答"这一格亮不亮"。
pub fn menu_enabled(path: Option<&Path>) -> bool {
    path.is_some_and(is_headerless_path)
}

static PENDING: std::sync::Mutex<VecDeque<PathBuf>> = std::sync::Mutex::new(VecDeque::new());

/// 显式打开入口（命令行 / 拖拽 / 右键打开共用）。
///
/// 非裸数据直接 `viewer.open`；裸数据先自动推断，推得出就直接开。推不出才**压入
/// 待办**——因为命令行入口与文件对话框回调里没有 `EventCtx`（`open_window` 开不了窗），
/// 由主窗 interval 里 [`poll_pending`] 补弹（间隔 16ms，无感）。
pub fn open_path(viewer: &Rc<RefCell<Viewer>>, config: &Rc<RefCell<Config>>, path: &Path) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !decode::is_raw_ext(&ext) || !decode::is_headerless_ext(&ext) {
        viewer.borrow_mut().open(path);
        return;
    }
    // 同一路径已有用户确认过的参数（右键改过）：优先用，别让自动推断覆盖用户选择。
    if decode::get_raw_params(path).is_some() {
        viewer.borrow_mut().open(path);
        return;
    }
    // 首开自动：后缀定布局、长度定宽高，能推出来就直接开，不打断用户。
    // 推不出来才排队弹参数窗（带默认值），右键仍可随时重开参数窗。
    let inferred = infer_params(path);
    if let Some(params) = inferred {
        decode::set_raw_params(path, params);
        viewer.borrow_mut().open(path);
        return;
    }
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(path.to_path_buf());
    let _ = config; // 与 open_params_window 共用签名；预填在弹窗时读。
}

/// 主窗 interval 每帧调用：有待办的裸数据就弹参数窗（一次一个）。
pub fn poll_pending(
    ctx: &mut EventCtx,
    viewer: &Rc<RefCell<Viewer>>,
    config: &Rc<RefCell<Config>>,
) {
    // 参数窗单实例（.single("raw-params")）：窗开着就先不取——取了也开不出第二个
    // 窗，那条路径就永久丢了（旧实现无此守卫，连拖多个裸文件必丢中间的）。等关
    // 窗后下一轮再弹；windui 对已登记的单窗如实报告开合状态。
    if windui::event::window_open("raw-params") {
        return;
    }
    let next = PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front();
    if let Some(path) = next {
        open_params_window(ctx, viewer, config, &path);
    }
}

/// 右键入口：对当前裸像素文件重开参数窗。
///
/// 菜单项常驻，但只有 [`menu_enabled`] 为真时才可点；相机 RAW 有头，不能误触。
pub fn open_current_params(
    ctx: &mut EventCtx,
    viewer: &Rc<RefCell<Viewer>>,
    config: &Rc<RefCell<Config>>,
) {
    let path = viewer.borrow().current_path();
    let Some(path) = path else {
        ctx.toast_err("还没有打开文件");
        return;
    };
    if !is_headerless_path(&path) {
        ctx.toast_err("当前格式有文件头，不需要裸数据参数");
        return;
    }
    if windui::event::window_open("raw-params") {
        return;
    }
    open_params_window(ctx, viewer, config, &path);
}

/// 参数窗本体（单实例：连拖多个裸文件只留一个窗）。
fn open_params_window(
    ctx: &mut EventCtx,
    viewer: &Rc<RefCell<Viewer>>,
    config: &Rc<RefCell<Config>>,
    path: &Path,
) {
    // 预填优先级：本会话已确认值 > 本轮自动推断 > 上次使用值 > 空。
    // 布局再优先按后缀纠正——`noise.bgra` 不能因为上次开的是 RGBA8 就按 RGBA8 解。
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let suffix_layout = layout_index_for_ext(&ext);
    let current = decode::get_raw_params(path);
    let inferred = infer_params(path);
    let (init_w, init_h, init_layout_idx) = {
        let c = config.borrow();
        let dims = current.or(inferred).or(c.raw_last);
        let layout_idx = current
            .map(|r| r.layout)
            .or(inferred.map(|r| r.layout))
            .or_else(|| suffix_layout.map(|i| LAYOUTS[i]))
            .or(c.raw_last.map(|r| r.layout))
            .and_then(|v| LAYOUTS.iter().position(|&x| x == v))
            .unwrap_or(2);
        let (w, h) = dims
            .map(|r| (r.width.to_string(), r.height.to_string()))
            .unwrap_or_default();
        (w, h, layout_idx)
    };
    let w_sig = signal(init_w);
    let h_sig = signal(init_h);
    let layout_sig = signal(init_layout_idx);

    let viewer = viewer.clone();
    let config = config.clone();
    let path = path.to_path_buf();
    let confirm_path = path.clone();
    let confirm = move |ctx: &mut EventCtx| {
        // 填错了**必须说话，而且不关窗**：过去这里"解析失败就闭窗不动作"，用户点完
        // 「确定」看到的是窗口没了、图也没出来——等于"点了没反应"（审计 O6，撞 §八.7
        // 提示纪律）。留在原地让他改数字才是这条对话框该有的行为。
        let w = parse_dim(&w_sig.get());
        let h = parse_dim(&h_sig.get());
        let (Some(w), Some(h)) = (w, h) else {
            ctx.toast_err(format!("宽和高得是 1…{MAX_EDGE} 之间的整数"));
            return;
        };
        let params = RawParams {
            width: w,
            height: h,
            layout: LAYOUTS[layout_sig.get()],
        };
        decode::set_raw_params(&confirm_path, params);
        // ini 键：raw_last（记住上次裸数据参数，下次预填）
        config.borrow_mut().raw_last = Some(params);
        crate::core::config::mark_dirty();
        viewer.borrow_mut().open(&confirm_path);
        ctx.request_close();
    };
    let cancel = |ctx: &mut EventCtx| ctx.request_close();

    ctx.open_window(
        Window::new("裸数据参数", 340, 210)
            .bg(to_color(BG))
            .resizable(false)
            .centered(true)
            // 内容高度按实际行数算，避免按钮被窗口底边裁掉；下限与插件设置窗共用一格。
            .fit_content(DIALOG_FIT_MIN, 360)
            // 归属主窗：主窗置顶时参数窗不会被盖住（与设置窗同款处理）。
            .owned(true)
            .single("raw-params")
            .content(move || {
                let confirm = confirm.clone();
                Element::col()
                    .fill()
                    .padding(DIALOG_PAD)
                    .spacing(10)
                    .child(Element::field(
                        "宽度",
                        Element::text_input(w_sig, "如 1920"),
                    ))
                    .child(Element::field(
                        "高度",
                        Element::text_input(h_sig, "如 1080"),
                    ))
                    .child(Element::field(
                        "像素布局",
                        Element::dropdown(LAYOUT_LABELS.to_vec(), layout_sig),
                    ))
                    .child(
                        Element::row()
                            .cross(Align::Center)
                            .spacing(8)
                            .child(Element::flex_spacer())
                            .child(Element::button("确定").on_click(confirm))
                            .child(Element::button("取消").on_click(cancel)),
                    )
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_is_enabled_only_for_headerless_files() {
        assert!(!menu_enabled(None));
        assert!(menu_enabled(Some(Path::new("a.bgra"))));
        assert!(menu_enabled(Some(Path::new("A.YUV"))));
        assert!(!menu_enabled(Some(Path::new("a.cr2"))));
        assert!(!menu_enabled(Some(Path::new("a.png"))));
        assert!(!menu_enabled(Some(Path::new("noext"))));
    }

    #[test]
    fn rawpx_defaults_follow_suffix_and_length() {
        let bgra = decode::infer_raw_params_for("bgra", 4096 * 66 * 4).expect("bgra geometry");
        assert_eq!(
            (bgra.width, bgra.height, bgra.layout),
            (4096, 66, LAYOUTS[6])
        );

        let rgb = decode::infer_raw_params_for("rgb", 4096 * 88 * 3).expect("rgb geometry");
        assert_eq!((rgb.width, rgb.height, rgb.layout), (4096, 88, LAYOUTS[1]));

        let i420 = decode::infer_raw_params_for("i420", 4096 * 178 * 3 / 2).expect("i420 geometry");
        assert_eq!(
            (i420.width, i420.height, i420.layout),
            (4096, 178, LAYOUTS[9])
        );

        let yuy2 = decode::infer_raw_params_for("yuy2", 4096 * 134 * 2).expect("yuy2 geometry");
        assert_eq!(
            (yuy2.width, yuy2.height, yuy2.layout),
            (4096, 134, LAYOUTS[7])
        );
    }

    #[test]
    fn rawpx_falls_back_to_a_common_width() {
        let p = decode::infer_raw_params_for("rgba", 1920 * 1080 * 4).expect("1920x1080 rgba");
        assert_eq!((p.width, p.height), (1920, 1080));
    }

    #[test]
    fn rawpx_unknown_length_does_not_guess() {
        assert!(decode::infer_raw_params_for("bgra", 4096 * 66 * 4 + 1).is_none());
        assert!(decode::infer_raw_params_for("3fr", 4096 * 66 * 4).is_none());
    }

    #[test]
    fn camera_raw_never_gets_the_rawpx_dialog() {
        assert!(is_headerless_path(Path::new("noise.bgra")));
        assert!(!is_headerless_path(Path::new("noise.3fr")));
        assert!(!is_headerless_path(Path::new("noise.raw")));
    }
}
