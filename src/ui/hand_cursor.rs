//! 抓手光标：把 `image/cursor-*.svg` 光栅化成 `HCURSOR`，按指针状态切换。
//!
//! # 为什么由宿主自己管，不走框架的 `Widget::cursor()`
//!
//! 两条都堵死那条路：windui 的 `CursorShape` 只有箭头/手型/文本/左右上下五种，
//! 没有"开手 / 握拳"；更要紧的是它在指针拖动时 `SetCapture`，而**捕获期间系统
//! 不再发 `WM_SETCURSOR`**（MSDN 明写），那条通道拿不到"按住"这一刻。
//!
//! # 分层
//!
//! 光栅化要 windui 的 resvg（`Image::from_svg_bytes`），只能在这一层；
//! 造句柄与 `SetCursor` 要 `windows`，在 [`crate::platform::cursor`]。
//! 所以这里只做"SVG → RGBA"，再交给平台层。
//!
//! # 缓存
//!
//! 两件事都很贵，各缓存一次：光栅化结果按系统光标边长缓存，句柄在平台层缓存。
//! 之后每次移动只是 `SetCursor`，可以放心地每帧调。
//!
//! 缓存**按尺寸失效**（[`refresh_for_dpi`]）：把窗口拖到另一块不同缩放的显示器后，
//! 光标边长会变，旧位图继续用就是一块放大的糊图。两边都是 `Mutex<Option<…>>`
//! 而不是 `OnceLock`，正是为了能丢掉重造。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use windui::render::image::Image;

/// 松手：可拖区域悬停时。
const GRAB_SVG: &[u8] = include_bytes!("../../image/cursor-grab.svg");
/// 抓手：按住左键拖动时。
const GRABBING_SVG: &[u8] = include_bytes!("../../image/cursor-grabbing.svg");

/// 两枚图的坐标系边长（`viewBox="0 0 32 32"`），热点按它换算比例。
const GRID: f32 = 32.0;

/// 热点：落在**手心**——抓取的锚点在手心，不在指尖。
const HOT_GRAB: (f32, f32) = (16.0, 20.0);
const HOT_GRABBING: (f32, f32) = (16.0, 19.0);

/// 光栅化缓存：`(边长, RGBA)`。`Arc` 是为了让 `apply` 每帧都能廉价取用——
/// 拿的是引用计数而不是整块像素的拷贝。
type Rgba = Arc<Vec<u8>>;
static GRAB_PX: Mutex<Option<(u32, Rgba)>> = Mutex::new(None);
static GRABBING_PX: Mutex<Option<(u32, Rgba)>> = Mutex::new(None);

/// 上一次对外请求的状态：0 = 箭头、1 = 松手、2 = 抓手。
///
/// DPI 变化重建之后要按它复原：用户正拖到另一块屏上，光标不该因为"重建"退回箭头、
/// 一直等到下一次鼠标移动才恢复。
static LAST: AtomicU8 = AtomicU8::new(0);

/// 设置手型光标。
///
/// `Some(true)` = 抓手（拖动中）、`Some(false)` = 松手（悬停在可拖区）、
/// `None` = 交回系统箭头。★ 传 `None` 时会**主动**设回箭头：抓手的最后一帧若
/// 不主动收，指针不再移动时光标会停在手上（框架的 `WM_SETCURSOR` 要等下一次移动）。
pub fn apply(hand: Option<bool>) {
    LAST.store(
        match hand {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        },
        Ordering::Relaxed,
    );
    apply_now(hand);
}

/// DPI 变了就按新边长重造两枚光标，并把状态复原。
///
/// 由 runtime 的轮询调用（每轮一次）：`GetDpiForMonitor` 之类没有事件通知可用，
/// 而框架的 `set_scale` 只在窗口自己跨屏时才触发——光标先跑到新屏上时它还没动。
/// 平时只是一个 DPI 读数比较，代价可忽略。
pub fn refresh_for_dpi() {
    let want = crate::platform::cursor::system_size();
    let Some(cached) = cached_size() else {
        // 还没装过任何一枚（这张图不可拖）：等第一次 `apply` 按新尺寸造就行。
        return;
    };
    if cached == want {
        return;
    }
    // 顺序要紧：先把光标从手型上摘下来，再销毁那两枚句柄——`DestroyIcon` 一枚
    // 正被 `SetCursor` 挂着的图标，屏幕上那枚就成了野指针。
    windui::platform::set_cursor_override(None);
    crate::platform::cursor::use_arrow();
    crate::platform::cursor::drop_hands();
    clear_px();
    // 按新边长重造，并恢复用户此刻本该看到的那一态。
    apply_now(match LAST.load(Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    });
}

/// 当前光栅化缓存的边长（两枚取任一，它们永远同尺寸）。没缓存返回 `None`。
fn cached_size() -> Option<u32> {
    for cell in [&GRAB_PX, &GRABBING_PX] {
        if let Ok(guard) = cell.lock() {
            if let Some((size, _)) = guard.as_ref() {
                return Some(*size);
            }
        }
    }
    None
}

/// 丢掉光栅化缓存（DPI 变了要按新边长重画）。
fn clear_px() {
    for cell in [&GRAB_PX, &GRABBING_PX] {
        if let Ok(mut guard) = cell.lock() {
            *guard = None;
        }
    }
}

fn apply_now(hand: Option<bool>) {
    let Some(dragging) = hand else {
        // 撤覆盖必须**先**于设箭头：留着的话，紧接到达的 `WM_SETCURSOR` 会把
        // 抓手句柄又设回去，指针就停在手上撤不下来。
        windui::platform::set_cursor_override(None);
        crate::platform::cursor::use_arrow();
        return;
    };
    let size = crate::platform::cursor::system_size();
    let (cell, svg, hot) = if dragging {
        (&GRABBING_PX, GRABBING_SVG, HOT_GRABBING)
    } else {
        (&GRAB_PX, GRAB_SVG, HOT_GRAB)
    };
    let Some(rgba) = raster(cell, svg, size) else {
        return;
    };
    let k = size as f32 / GRID;
    let hotspot = ((hot.0 * k).round() as i32, (hot.1 * k).round() as i32);
    if crate::platform::cursor::install_hand(dragging, &rgba, size, hotspot) {
        crate::platform::cursor::use_hand(dragging);
        // 登记成框架的"自绘光标覆盖"（windui 补丁 024）：框架应答 `WM_SETCURSOR`
        // 时先认它。少了这一步，鼠标每移动一次系统都把光标设回箭头，我们紧接着
        // 在 MOUSEMOVE 里设回手型 —— 两次 `SetCursor` 前后脚，肉眼就是手型↔箭头闪。
        windui::platform::set_cursor_override(crate::platform::cursor::handle_of(dragging));
    }
}

/// 光栅化并按边长缓存。非正方形（改了 svg 的形状）时返回 `None`——那时再造光标
/// 只会得到一个被拉伸的手，不如不换。
fn raster(cell: &'static Mutex<Option<(u32, Rgba)>>, svg: &[u8], size: u32) -> Option<Rgba> {
    let mut guard = match cell.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((cached, px)) = guard.as_ref() {
        if *cached == size {
            return Some(Arc::clone(px));
        }
    }
    let img = Image::from_svg_bytes(svg, Some(size)).ok()?;
    let (w, h) = (img.width(), img.height());
    let px = img.to_rgba();
    if w != size || h != size || px.len() != (size as usize * size as usize * 4) {
        return None;
    }
    let px = Arc::new(px);
    *guard = Some((size, Arc::clone(&px)));
    Some(px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两枚 svg 都要能被 resvg 光栅化成**正方形**的 RGBA。
    /// 这条是接线的地基：图坏了或者 viewBox 被改成非正方形，抓手就永远不出现，
    /// 而失败是静默的（`apply` 只在内部 return）。
    #[test]
    fn both_hands_rasterize_to_square_rgba() {
        for (name, svg) in [("cursor-grab", GRAB_SVG), ("cursor-grabbing", GRABBING_SVG)] {
            let img = Image::from_svg_bytes(svg, Some(32))
                .unwrap_or_else(|e| panic!("{name}.svg 光栅化失败：{e}"));
            assert_eq!(
                (img.width(), img.height()),
                (32, 32),
                "{name}.svg 光栅化后不是 32×32"
            );
            let px = img.to_rgba();
            assert_eq!(px.len(), 32 * 32 * 4, "{name}.svg 字节数不对");
            // 不能整幅空白：有像素是实心的，才说明图真画出来了。
            assert!(
                px.chunks_exact(4).any(|p| p[3] > 200),
                "{name}.svg 全透明——图是空的"
            );
        }
    }

    /// 两枚都要真的装进平台层（造句柄失败时 `apply` 只 return，不留痕迹）。
    /// 顺带盖住"两枚用了同一个缓存槽"这种会让抓手永远出不来（或反之）的错。
    #[test]
    fn apply_installs_both_hands() {
        apply(Some(false));
        apply(Some(true));
        let grab = crate::platform::cursor::use_hand(false);
        let grabbing = crate::platform::cursor::use_hand(true);
        apply(None);
        assert!(grab, "松手光标没装进平台层");
        assert!(grabbing, "抓手光标没装进平台层");
    }

    /// 手型要被登记成框架的"自绘光标覆盖"，且撤手时要清干净。
    ///
    /// 这一条守的是 **windui 补丁 024 的接线**：覆盖位不登记，`WM_SETCURSOR` 就会把
    /// 光标设回箭头，表现是拖图时光标一闪一闪（用户在 2026-11-03 报的那个）。
    /// 撤得不干净的后果相反——指针离开图区还赖在手上。
    #[test]
    fn hands_are_registered_as_cursor_override() {
        apply(Some(true));
        assert_eq!(
            windui::platform::cursor_override(),
            crate::platform::cursor::handle_of(true),
            "抓手没登记成覆盖，框架会把光标设回箭头（拖图闪烁会回来）"
        );
        apply(Some(false));
        assert_eq!(
            windui::platform::cursor_override(),
            crate::platform::cursor::handle_of(false),
            "开手没登记成覆盖"
        );
        apply(None);
        assert_eq!(
            windui::platform::cursor_override(),
            None,
            "放手后覆盖没撤，指针会停在手型上"
        );
    }

    /// 尺寸没变时 `refresh_for_dpi` 必须是空操作。
    ///
    /// 它挂在 16 ms 轮询上：若每轮都销毁重造，光标会成一具不断换句柄的空壳
    /// （屏幕上看不出来，但每次都白建两枚图标）。
    #[test]
    fn dpi_refresh_is_a_noop_when_the_size_matches() {
        apply(Some(true));
        let before = crate::platform::cursor::handle_of(true);
        refresh_for_dpi();
        assert_eq!(
            crate::platform::cursor::handle_of(true),
            before,
            "系统光标边长没变，却把句柄重建了"
        );
        apply(None);
    }

    /// DPI 变了要能真的按新边长重造：缓存与句柄都换成新的，状态复原到重建前那一态。
    ///
    /// 这条盖住的是"用 `OnceLock` 就再也换不掉"——那时光标跨屏后会一直用旧块头。
    #[test]
    fn dpi_refresh_rebuilds_and_restores_the_hand() {
        apply(Some(false));
        let old = crate::platform::cursor::handle_of(false).expect("开手应已装好");
        // 假装 DPI 变了：把缓存里的边长改掉，`refresh_for_dpi` 才会认为需要重造。
        for cell in [&GRAB_PX, &GRABBING_PX] {
            if let Ok(mut g) = cell.lock() {
                if let Some((size, px)) = g.as_ref() {
                    let (size, px) = (*size, Arc::clone(px));
                    *g = Some((size + 1, px));
                }
            }
        }
        refresh_for_dpi();
        let new = crate::platform::cursor::handle_of(false).expect("重造后开手应还在");
        assert_ne!(old, new, "边长变了却没换句柄");
        assert_eq!(
            windui::platform::cursor_override(),
            Some(new),
            "重造后覆盖位仍应指向开手"
        );
        apply(None);
    }

    /// 热点必须落在图内：越界的热点会让 Windows 把光标放歪（抓取点不在手心）。
    #[test]
    fn hotspots_are_inside_the_grid() {
        for (hot, tag) in [(HOT_GRAB, "grab"), (HOT_GRABBING, "grabbing")] {
            assert!(
                hot.0 > 0.0 && hot.0 < GRID && hot.1 > 0.0 && hot.1 < GRID,
                "{tag} 的热点 {hot:?} 不在 32 网格内"
            );
        }
    }
}
