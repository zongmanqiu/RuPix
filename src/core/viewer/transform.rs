//! 像素层视图变换：旋转 / 镜像的坐标映射与切片截取。
//!
//! 为什么在像素层做：windui 的 `draw_image` 没有旋转/翻转参数，而放大到 ≥100% 时
//! 上屏的可能是 SVG 可视切片——切片与朝向必须一起算，否则画面错位。
//!
//! 正映射 [`rect_map_display`] 与逆映射 [`rect_map_image`] 必须**互逆**
//! （`tests.rs` 用四组 rot × 两组 flip 全排列钉住）。

use super::*;

/// 缩略图条可见窗口的首序号：`n` 格以当前图为中心，翻到末尾时不再居中。
///
/// 抽成纯函数只为能单测——这三个分支（开头 / 中间 / 末尾）是最容易算错的地方，
/// 而算错的表现在 UI 上是"右边空半排"或"当前格自己跑出去了"。
pub(super) fn strip_window_first(len: usize, cur: usize, n: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let n = n.clamp(1, len);
    cur.saturating_sub((n - 1) / 2).min(len - n)
}

/// 用户自己滚过之后（`strip_first = Some`）的窗口首序号：**只把锚点夹进合法区间**，
/// 不跟随当前图（m260）。
///
/// ★ m258 第一版在这里写了"当前图跑出视野就把它带回来"。看着合理，实际等于**拖不动**：
///   拖条本身不换图，所以只要一拖到当前格看不见，下一帧就被拽回原处——用户实测
///   2026-11-08 报的就是这句「我们当前看的图好像必须出现在缩略图里面？」
///   现在口径：条的位置只由用户的手决定（拖动 / 滚轮 / 拖滑块），换图（点格子、
///   键盘、浮条、自动播放）一律不动它；想回到"跟着当前图"就换边或关掉条（锚点归零）。
///
/// `anchor` 越界（目录变短）时夹进合法区间，所以调用方不必预夹。
pub(super) fn strip_window_anchored(len: usize, n: usize, anchor: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let n = n.clamp(1, len);
    anchor.min(len - n)
}

// ── 像素层视图变换（旋转 / 镜像）────────────────────────────────

/// 归一化显示矩形 → 图像矩形（区域请求用；[`rect_map_display`] 的逆）。
pub(super) fn rect_map_image(
    rot: u8,
    flip: bool,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) -> (f32, f32, f32, f32) {
    let (dx0, dy0, dx1, dy1) = if flip {
        (1.0 - x1, y0, 1.0 - x0, y1)
    } else {
        (x0, y0, x1, y1)
    };
    match rot % 4 {
        0 => (dx0, dy0, dx1, dy1),
        1 => (dy0, 1.0 - dx1, dy1, 1.0 - dx0),
        2 => (1.0 - dx1, 1.0 - dy1, 1.0 - dx0, 1.0 - dy0),
        _ => (1.0 - dy1, dx0, 1.0 - dy0, dx1),
    }
}

/// 归一化图像矩形 → 显示矩形（先旋转后镜像；与上屏位图生成顺序一致）。
pub(super) fn rect_map_display(
    rot: u8,
    flip: bool,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) -> (f32, f32, f32, f32) {
    let (mut dx0, dy0, mut dx1, dy1) = match rot % 4 {
        0 => (x0, y0, x1, y1),
        // 顺时针 90°：dx = 1−y, dy = x。
        1 => (1.0 - y1, x0, 1.0 - y0, x1),
        2 => (1.0 - x1, 1.0 - y1, 1.0 - x0, 1.0 - y0),
        // 270°：dx = y, dy = 1−x。
        _ => (y0, 1.0 - x1, y1, 1.0 - x0),
    };
    if flip {
        (dx0, dx1) = (1.0 - dx1, 1.0 - dx0);
    }
    (dx0.min(dx1), dy0.min(dy1), dx0.max(dx1), dy0.max(dy1))
}

/// **能按可视区域重渲**的格式（走"只渲看得见的那一块"路线）。
///
/// 判据真源在 `core::formats::is_region_renderable`：内置矢量（svg/svgz，宿主 resvg）
/// ∪ 解码器交出矢量文档由宿主渲染的那批 ∪ **ABI v6** 自渲染且实现了 `iv_render_region`
/// 的那批。
///
/// ★ 为什么不是"所有矢量"：**没有区域通道**的自渲染（解码器只能整幅渲染）绝不能走
/// 这条路——宿主若按可视区域去要，解码器只能把**整篇**缩进那一小块，那是**内容错误**
/// 的图，不是"糊一点"。判据集中在 `core::formats`，不散在各处 `if`。
pub(super) fn is_vector(p: &Path) -> bool {
    let Some(e) = p.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    crate::core::formats::is_region_renderable(e)
}

/// **按目标重画、但没有区域通道**的矢量（m189）：放大后要请解码器整幅重画一档。
///
/// 与 [`is_vector`] 互斥：那个是"能按可视区域 1:1 重渲"的（svg 族 + 有 `iv_render_region`
/// 的解码器），这个恰好是剩下那批（doc / doc-legacy 的 pdf/emf/wmf/dxf/dwg/xps/ps/hpgl/cmx…）。
/// 判据同样集中在 `core::formats`。
pub(super) fn is_target_vector(p: &Path) -> bool {
    let Some(e) = p.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    crate::core::formats::needs_zoom_rerender(e)
}
