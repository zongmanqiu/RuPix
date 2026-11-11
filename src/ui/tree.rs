//! 文件树面板（m300）：压缩包打开时贴在画面**最左侧**，缩略图条在它右边。
//!
//! 为什么是树而不是右键「本包 ▸」：右键那套要求用户**记住**自己在哪一层、上一层叫什么，
//! 包里套包时来回跳很容易迷路；树把结构摆在那儿，"看见就能点"。
//!
//! 树只列**层**（目录与子包），不列图片：它回答的是"包里有哪些层"，不是"有哪些文件"。
//!
//! ★ 几何与命中是同一份：`row_rect` 说画在哪，`hit` 就说点到了谁。两处各算一遍是这类
//!   面板最常见的错位来源（见 `chrome::to_canvas` 与 `info_rect` 的教训）。

use windui::geometry::Color;
use windui::geometry::{Point, Rect};
use windui::render::{Canvas, Paint};
use windui::spec::Align;
use windui::text::TextStyle;

use super::{fill_panel_open, tokens as tk, FlatEdges};
use crate::core::archive::TreeNode;

/// 树的目标宽度（逻辑 dp）：够放「两级缩进 + 图标 + 八来个中文字」。
pub const TREE_W: f32 = 184.0;
/// 收窄时的下限：再窄连图标带一个字都放不下，那时宁可让画面受点委屈也不把树删掉
/// ——树是进层的**唯一**入口，隐藏它等于把人锁在当前层。
pub const TREE_MIN: f32 = 112.0;
/// 画面至少要留这么宽，剩下的才给树。
pub const MIN_VIEW_W: f32 = 220.0;

const PAD: i32 = 8;
const ROW_H: i32 = 26;
/// 每一级缩进。
const INDENT: i32 = 13;
/// 图标边长与图标后的空隙。
const ICON: f32 = 11.0;
const ICON_GAP: i32 = 6;
const FONT: f32 = 12.0;

/// 树在 `vw` 宽的内容区里该多宽。
///
/// 默认按 26% 取，夹在 [`TREE_MIN`, `TREE_W`]；但画面至少留 [`MIN_VIEW_W`]——
/// 两者冲突时**让树窄下去**，而不是让它消失。
pub fn width_for(vw: f32) -> f32 {
    let want = (vw * 0.26).clamp(TREE_MIN, TREE_W);
    // 极窄窗口（用户自己拉的）也别把画面挤没：最多占一半。
    want.min((vw * 0.5).max(60.0))
}

/// 树面板的矩形（**视口绝对坐标**，与命中同一套）。贴 `content` 的最左边一整条。
pub fn panel(content: Rect, w: f32) -> Rect {
    Rect::new(content.x, content.y, w as i32, content.h)
}

/// 截断提示占的高度：与一行节点同高，画在面板最下面。
const NOTICE_H: i32 = ROW_H;

/// 滚动指示的视觉厚度 / 最小滑块长 / 命中热区。★ 与缩略图条同一组尺寸
/// （`filmstrip` 的 6 / 24 / 16）：两块地并排站着，条一粗一细会显得是两套东西。
const BAR_T: f32 = 6.0;
const BAR_MIN: f32 = 24.0;
const BAR_HIT: f32 = 16.0;
/// 树没列全时给的一句。短到最窄的树也装得下（112 - 16 = 96dp）。
const NOTICE_TEXT: &str = "还有层没列出";

/// 一屏能放几行（至少 1）。`notice` 为真时给底部提示让出一整行。
pub fn capacity(ph: i32, notice: bool) -> usize {
    let usable = ph - 2 * PAD - if notice { NOTICE_H } else { 0 };
    ((usable / ROW_H).max(1)) as usize
}

/// 截断提示的矩形（**视口绝对坐标**，与 `row_rect` / `hit` 同一套）。
pub fn notice_rect(panel: Rect) -> Rect {
    Rect::new(
        panel.x + PAD,
        panel.y + panel.h - PAD - NOTICE_H,
        (panel.w - 2 * PAD).max(1),
        NOTICE_H,
    )
}

/// 节点占用的那块矩形（扣掉上下内边距与底部提示行）。滚动条只走这一块，
/// 所以它永远不会压住「还有层没列出」那一行。
pub fn rows_area(panel: Rect, notice: bool) -> Rect {
    let h = panel.h - 2 * PAD - if notice { NOTICE_H } else { 0 };
    Rect::new(
        panel.x + PAD,
        panel.y + PAD,
        (panel.w - 2 * PAD).max(1),
        h.max(1),
    )
}

/// 纵向滚动指示：一屏放得下就**没有**（既没有条也没有热区）。
///
/// 返回 `(轨道, 滑块)`，**视口绝对坐标**。贴右缘**内侧** 2dp：树右缘是「相接边」
/// （m302 把它做成直角），条画在面板内部，不压那条边。
pub fn scroll_bar(
    panel: Rect,
    first: usize,
    cap: usize,
    n: usize,
    notice: bool,
) -> Option<(Rect, Rect)> {
    let cap = cap.max(1);
    if n <= cap || panel.w <= 0 || panel.h <= 0 {
        return None;
    }
    let area = rows_area(panel, notice);
    let (track_h, thumb_h) = bar_lens(area, cap, n);
    let max_first = (n - cap) as f32;
    let traveled = (first.min(n - cap)) as f32 / max_first;
    let slide = ((track_h - thumb_h) as f32 * traveled) as i32;
    let x = panel.x + panel.w - PAD - BAR_T as i32;
    Some((
        Rect::new(x, area.y, BAR_T as i32, track_h),
        Rect::new(x, area.y + slide, BAR_T as i32, thumb_h),
    ))
}

/// 轨道长与滑块长。`scroll_bar` 与点轨道跳转共用这一份 ⇒ 两处算出来的行程一样，
/// 「点一下再拖」不会先跳一截。
fn bar_lens(area: Rect, cap: usize, n: usize) -> (i32, i32) {
    let track_h = area.h.max(1);
    let thumb_h = ((track_h as f32 * cap as f32 / n as f32) as i32)
        .max(BAR_MIN as i32)
        .min(track_h);
    (track_h, thumb_h)
}

/// 滚动指示的**命中**：热区比视觉条厚，居中铺开（6dp 的条没人拖得住）。
/// 判据与 [`scroll_bar`] 同源：`None` = 一屏放得下 = 既没有指示也没有热区。
pub fn scroll_at(
    panel: Rect,
    first: usize,
    cap: usize,
    n: usize,
    notice: bool,
    pos: (f32, f32),
) -> Option<(Rect, Rect)> {
    let (track, thumb) = scroll_bar(panel, first, cap, n, notice)?;
    let pad = ((BAR_HIT - BAR_T) / 2.0) as i32;
    let hot = Rect::new(track.x - pad, track.y, BAR_HIT as i32, track.h);
    let inside = pos.0 >= hot.x as f32
        && pos.0 < (hot.x + hot.w) as f32
        && pos.1 >= hot.y as f32
        && pos.1 < (hot.y + hot.h) as f32;
    inside.then_some((track, thumb))
}

/// 第 `i` 行（可见序号，已扣掉滚动偏移）的矩形。
///
/// 整行可点，所以缩进只影响**画**，不影响命中——缩进放在 `paint` 里加。
pub fn row_rect(panel: Rect, i: usize) -> Rect {
    Rect::new(
        panel.x + PAD,
        panel.y + PAD + i as i32 * ROW_H,
        (panel.w - 2 * PAD).max(1),
        ROW_H,
    )
}

/// 首行下标的上限：树比一屏长时才需要滚。
pub fn max_first(n: usize, cap: usize) -> usize {
    n.saturating_sub(cap.max(1))
}

/// 点到了哪一行（返回节点下标）。点在面板外、或者落在最后半行留白上 ⇒ `None`。
pub fn hit(panel: Rect, n: usize, first: usize, pos: (f32, f32), notice: bool) -> Option<usize> {
    let p = Point {
        x: pos.0 as i32,
        y: pos.1 as i32,
    };
    if !panel.contains(p) {
        return None;
    }
    let dy = p.y - panel.y - PAD;
    if dy < 0 {
        return None;
    }
    // ★ m306：提示那一行不是节点，点它不许进层；顺带把「点到一屏之外」也挡掉
    //   （原来只查 idx < n，尾部留白上的点击会命中一个根本没画出来的节点）。
    if notice && notice_rect(panel).contains(p) {
        return None;
    }
    let idx = first + (dy / ROW_H) as usize;
    let cap = capacity(panel.h, notice);
    (idx < n && idx < first + cap).then_some(idx)
}

/// 画整棵树。`cur` 是当前层在 `nodes` 里的下标（`None` = 找不到，不高亮）。
pub fn paint(
    canvas: &mut dyn Canvas,
    panel: Rect,
    nodes: &[TreeNode],
    cur: Option<usize>,
    first: usize,
    hover: Option<usize>,
    notice: bool,
) {
    if nodes.is_empty() || panel.w <= 0 || panel.h <= 0 {
        return;
    }
    // ★ m302：树是**第三块地**，既不是看图区底色也不是缩略图条底色。上一版取了
    //   看图区的 `empty_bg`，真机截图里树与画面同为 #F6F6F8，整块面板等于没画。
    // ★ 右缘与缩略图条 / 看图区相接 ⇒ 直角；只有贴窗口外角的左侧两角留圆。
    fill_panel_open(
        canvas,
        panel,
        10.0,
        FlatEdges::new(false, true, false),
        &Paint::fill(tk::tree_bg()),
        &Paint::fill(tk::panel_edge()),
    );

    let cap = capacity(panel.h, notice);
    let ts = TextStyle::new(FONT);
    let text = tk::overlay_text();

    for (i, node) in nodes.iter().enumerate().skip(first).take(cap) {
        let r = row_rect(panel, i - first);
        let is_cur = cur == Some(i);
        if is_cur {
            // ★ m309：当前层靠**两个**信号标识——不透明底 + 左缘竖条。
            //   旧写法用 `slot_cur`（6% 黑叠加，本来是给缩略图格压图用的），
            //   压在实心的 `tree_bg` 上只差 14 级灰，真机上等于没标。
            canvas.fill_round_rect(
                r.x as f32,
                r.y as f32,
                r.w as f32,
                r.h as f32,
                4.0,
                &Paint::fill(tk::tree_cur()),
            );
            // 竖条贴**面板内缘**，与缩进无关：depth 再深也不会跟图标挤在一起。
            // 它同时是区分手段——hover 只有淡底、没有竖条，两者靠形状分开，
            // 不靠灰度差（6 级灰的差别人眼本来就分不出来）。
            canvas.fill_round_rect(
                panel.x as f32 + 3.0,
                r.y as f32 + 3.0,
                3.0,
                (ROW_H - 6) as f32,
                1.5,
                &Paint::fill(text),
            );
        } else if hover == Some(i) {
            canvas.fill_round_rect(
                r.x as f32,
                r.y as f32,
                r.w as f32,
                r.h as f32,
                4.0,
                &Paint::fill(tk::slot_hover()),
            );
        }

        // 缩进 + 图标。根节点（depth 0）缩进为 0，所以包名贴着面板左缘。
        let ix = (r.x + node.depth as i32 * INDENT) as f32;
        let iy = r.y as f32 + (ROW_H as f32 - ICON) / 2.0;
        if node.dir {
            icon_dir(canvas, ix, iy, tk::overlay_text_muted());
        } else {
            icon_pack(canvas, ix, iy, tk::overlay_text_active());
        }

        // 尾部的图数：只在**有图**时画。空层画个 "0" 只是噪音，而且空层本来就该靠
        // 色彩（次要色）区分——进得去、但里面没图。
        let mut right_pad = PAD;
        let count = node.images.to_string();
        let count_w = if node.images > 0 {
            let ts_small = TextStyle::new(FONT - 1.0);
            let m = canvas.measure_text(&count, &ts_small);
            let w = m.w;
            canvas.draw_text(
                &count,
                Rect::new(r.right() - PAD - w, r.y + 4, w, ROW_H - 8),
                tk::overlay_text_muted(),
                Align::Center,
                &ts_small,
            );
            right_pad += w + 6;
            w
        } else {
            0
        };
        let _ = count_w;

        let tx = ix + ICON + ICON_GAP as f32;
        let avail = (r.right() - right_pad) as f32 - tx;
        if avail <= 4.0 {
            continue;
        }
        let label = clip(&node.label, avail, FONT);
        let color = if node.images > 0 || node.depth == 0 {
            text
        } else {
            tk::overlay_text_muted()
        };
        canvas.draw_text(
            &label,
            Rect::new(tx as i32, r.y, avail.max(1.0) as i32, ROW_H),
            color,
            Align::Start,
            &ts,
        );
    }
    // ★ m306：树被层级预算截断时，被截掉的那几支「看起来就是没有子层」。
    //   只在 `notice` 为真时多这一行；普通包一个字都不多，节点数量也不变。
    if notice {
        paint_notice(canvas, panel);
    }
    // ★ m307：树比一屏长才有滚动指示。轨道只覆盖 `rows_area`，不压提示行。
    if let Some((track, thumb)) = scroll_bar(panel, first, cap, nodes.len(), notice) {
        canvas.fill_round_rect(
            track.x as f32,
            track.y as f32,
            track.w as f32,
            track.h as f32,
            1.0,
            &Paint::fill(tk::strip_track()),
        );
        canvas.fill_round_rect(
            thumb.x as f32,
            thumb.y as f32,
            thumb.w as f32,
            thumb.h as f32,
            1.0,
            &Paint::fill(tk::strip_thumb()),
        );
    }
}

/// 树底部的一行提示。用次要色，看着是注脚而不是又一个可点的层。
fn paint_notice(canvas: &mut dyn Canvas, panel: Rect) {
    let r = notice_rect(panel);
    canvas.draw_text(
        &clip(NOTICE_TEXT, r.w as f32, FONT),
        r,
        tk::overlay_text_muted(),
        Align::Start,
        &TextStyle::new(FONT),
    );
}

/// 文件夹图标：一个方块 + 顶部左边一小截"标签"。
fn icon_dir(canvas: &mut dyn Canvas, x: f32, y: f32, c: Color) {
    canvas.fill_rect(x, y + 3.0, ICON, ICON - 3.0, &Paint::fill(c));
    canvas.fill_rect(x, y, ICON * 0.45, 3.0, &Paint::fill(c));
}

/// 子包图标：圆角方块 + 中间一条横线（"扎起来的箱子"）。
fn icon_pack(canvas: &mut dyn Canvas, x: f32, y: f32, c: Color) {
    canvas.fill_round_rect(x, y, ICON, ICON, 2.0, &Paint::fill(c));
    canvas.fill_rect(
        x,
        y + ICON / 2.0 - 0.5,
        ICON,
        1.0,
        &Paint::fill(tk::tree_bg()),
    );
}

/// 按估算宽度截断，末尾补省略号。
///
/// 不用 `measure_text` 逐行量：一屏二三十行、每行一次全量测量，纯属白烧。
/// 估算按「ASCII 约 0.55 个字号、其余约 1 个字号」——宁可早一点截断，也不要让
/// 文字压到右上角的图数上。
fn clip(text: &str, max_w: f32, size: f32) -> String {
    if max_w <= size * 0.6 {
        return String::new();
    }
    let mut w = 0.0f32;
    let mut out = String::new();
    for c in text.chars() {
        let cw = if c.is_ascii() { size * 0.55 } else { size };
        // 留一个省略号的余量。
        if w + cw > max_w - size * 0.8 {
            out.push('…');
            return out;
        }
        w += cw;
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(segs: &[&str], dir: bool) -> TreeNode {
        TreeNode {
            segs: segs.iter().map(|s| s.to_string()).collect(),
            label: segs.last().unwrap_or(&"pkg").to_string(),
            dir,
            depth: segs.len(),
            images: 1,
        }
    }

    #[test]
    fn width_shrinks_before_it_disappears() {
        // 宽窗口：取到上限。
        assert_eq!(width_for(1400.0), TREE_W);
        // 常规 800：仍是上限（0.26 × 800 = 208 > 184）。
        assert_eq!(width_for(800.0), TREE_W);
        // 收窄：按 26% 走（500 × 0.26 = 130），不低于下限。
        assert_eq!(width_for(500.0), 130.0);
        assert!(width_for(440.0) >= TREE_MIN.min(440.0 * 0.5));
        // 极窄：最多占一半，画面必须还剩下东西。
        assert!(width_for(200.0) <= 100.0);
        assert!(width_for(120.0) < 120.0);
    }

    #[test]
    fn rows_and_hits_are_the_same_geometry() {
        let content = Rect::new(0, 0, 800, 600);
        let p = panel(content, TREE_W);
        let nodes: Vec<TreeNode> = (0..10).map(|i| node(&[&format!("n{i}")], true)).collect();
        let cap = capacity(p.h, false);
        assert_eq!(cap, ((600 - 16) / 26) as usize);
        // 每一行的正中都该命中它自己（行数比节点多时，多出来的行无主）。
        for i in 0..cap.min(nodes.len()) {
            let r = row_rect(p, i);
            let mid = ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
            assert_eq!(hit(p, nodes.len(), 0, mid, false), Some(i));
        }
        // 滚起来之后，同一位置对应下一批节点。
        let r = row_rect(p, 0);
        let mid = ((r.x + 2) as f32, (r.y + 2) as f32);
        assert_eq!(hit(p, nodes.len(), 3, mid, false), Some(3));
    }

    #[test]
    fn hit_rejects_outside_and_tail_padding() {
        let p = panel(Rect::new(0, 0, 800, 600), TREE_W);
        let nodes = [node(&["a"], true)];
        // 面板右边外。
        assert_eq!(hit(p, nodes.len(), 0, (TREE_W + 5.0, 20.0), false), None);
        // 顶部留白。
        assert_eq!(hit(p, nodes.len(), 0, (10.0, 2.0), false), None);
        // 最后一行之后的空白。
        assert_eq!(hit(p, nodes.len(), 0, (10.0, 500.0), false), None);
        // 下标越过节点数。
        assert_eq!(hit(p, nodes.len(), 5, (10.0, 20.0), false), None);
    }

    #[test]
    fn max_first_never_underflows() {
        assert_eq!(max_first(3, 30), 0);
        assert_eq!(max_first(40, 30), 10);
        // 容量为 0 时按 1 算，避免除零式下溢。
        assert_eq!(max_first(3, 0), 2);
    }

    #[test]
    fn clip_keeps_short_names_and_trims_long_ones() {
        assert_eq!(clip("abc", 200.0, 12.0), "abc");
        let long = "一个非常非常非常长的压缩包名字.zip";
        let got = clip(long, 90.0, 12.0);
        assert!(got.ends_with('…'));
        assert!(got.chars().count() < long.chars().count());
        // 一点空间都没有时给空串，调用方据此跳过绘制。
        assert_eq!(clip("abc", 2.0, 12.0), "");
    }

    /// m302 两条判据：树必须用自己的第三档底色、右缘相接处直角。颜色令牌与裁剪
    /// 路径没有运行时可读的把手 ⇒ 用源码钉子守接线（同 `canvas::production_src`）。
    #[test]
    fn tree_panel_has_its_own_background_and_a_flat_right_edge() {
        let all = include_str!("tree.rs");
        let prod = &all[..all.find("\n#[cfg(test)]").unwrap_or(all.len())];
        assert!(prod.contains("fill_panel_open("), "树背板没走相接边直角");
        assert!(
            prod.contains("FlatEdges::new(false, true, false)"),
            "贴左的树该让右缘平直"
        );
        assert!(
            prod.contains("&Paint::fill(tk::tree_bg())"),
            "树背板必须用自己的那一档"
        );
        // m311：浅色端填充色分不到 5 个 ΔE，右缘那条 1dp 线是唯一的硬边界。
        assert!(
            prod.contains("&Paint::fill(tk::panel_edge())"),
            "树背板没画相接边那条分隔线：浅色下它和缩略图条只差 4 个 ΔE，会糊在一起"
        );
        // 负向臂：退回看图区或缩略图条的底色，都会让两块地糊成一整片。
        assert!(
            !prod.contains("tk::empty_bg()") && !prod.contains("tk::panel_bg()"),
            "树不许复用看图区或缩略图条底色"
        );
    }
    /// m306：提示必须让出整整一行，且这一行不许被点中。
    #[test]
    fn truncation_notice_takes_one_row_and_is_not_clickable() {
        let p = panel(Rect::new(0, 0, 800, 600), TREE_W);
        let plain = capacity(p.h, false);
        assert_eq!(capacity(p.h, true), plain - 1, "提示正好占一行");
        let r = notice_rect(p);
        let last = row_rect(p, plain - 2);
        assert!(r.y >= last.y + last.h, "提示不许压住最后一个节点");
        let mid = ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
        assert_eq!(hit(p, 100, 0, mid, true), None, "点提示行不许选中任何层");
        // 负向臂：同一位置、没提示时必须命中一个节点——否则上面那个 None
        // 可能只是「点在了留白上」，这条就测不到提示。
        assert!(hit(p, 100, 0, mid, false).is_some());
    }

    /// m306 负向臂：普通包（notice=false）的容量与命中一个字都不许变。
    #[test]
    fn notice_off_keeps_the_original_geometry() {
        let p = panel(Rect::new(0, 0, 800, 600), TREE_W);
        assert_eq!(capacity(p.h, false), ((600 - 16) / 26) as usize);
        let r = row_rect(p, 0);
        let mid = ((r.x + 2) as f32, (r.y + 2) as f32);
        assert_eq!(hit(p, 10, 0, mid, false), Some(0));
        assert_eq!(NOTICE_TEXT, "还有层没列出");
    }

    /// 源码钉子：提示的绘制必须走 `paint` 里那一个分支，不许挂到别处每帧画。
    #[test]
    fn notice_is_drawn_from_the_same_paint_path() {
        let all = include_str!("tree.rs");
        let prod = &all[..all.find("\n#[cfg(test)]").unwrap_or(all.len())];
        assert!(prod.contains("if notice {"), "paint 里没有提示分支");
        assert!(
            prod.contains("paint_notice(canvas, panel)"),
            "提示没走同一份几何"
        );
    }
    /// m307：一屏放得下就没有条；放不下才有，且滑块到末屏必须贴齐轨道底。
    #[test]
    fn scroll_bar_appears_only_when_the_tree_cannot_fit() {
        let p = panel(Rect::new(0, 0, 800, 600), TREE_W);
        let cap = capacity(p.h, false);
        assert!(
            scroll_bar(p, 0, cap, cap, false).is_none(),
            "正好一屏不该有条"
        );
        let (track, thumb) = scroll_bar(p, 0, cap, 40, false).expect("放不下就该有条");
        assert_eq!(track.x + track.w, p.x + p.w - 8, "条贴右缘内侧，不压相接边");
        assert!(thumb.h >= 24, "滑块不许细到抓不住");
        // 末屏贴齐：差 1 像素都是「滚到底却还差一点」那种一眼能看出来的错。
        let last = max_first(40, cap);
        let (t2, th2) = scroll_bar(p, last, cap, 40, false).unwrap();
        assert_eq!(th2.y + th2.h, t2.y + t2.h, "末屏滑块没贴到底");
        assert!(th2.y > thumb.y, "滚到底滑块该往下走");
    }

    /// m307：轨道只走节点区，**不许压住**「还有层没列出」那一行。
    #[test]
    fn scroll_bar_never_covers_the_notice_row() {
        let p = panel(Rect::new(0, 0, 800, 600), TREE_W);
        let cap = capacity(p.h, true);
        let (track, _) = scroll_bar(p, 0, cap, 60, true).expect("放不下就该有条");
        let n = notice_rect(p);
        assert!(
            track.y + track.h <= n.y,
            "轨道伸进了提示行：轨底 {} vs 提示顶 {}",
            track.y + track.h,
            n.y
        );
    }

    /// m307：热区比视觉条厚且居中铺开；条外一指就不算命中。
    #[test]
    fn scroll_at_uses_a_thicker_hot_zone() {
        let p = panel(Rect::new(0, 0, 800, 600), TREE_W);
        let cap = capacity(p.h, false);
        let (track, _) = scroll_bar(p, 0, cap, 40, false).unwrap();
        let mid_y = track.y as f32 + 30.0;
        assert!(scroll_at(p, 0, cap, 40, false, (track.x as f32 + 3.0, mid_y)).is_some());
        // 视觉条左右各多出 5dp（16 vs 6）⇒ 条外 3dp 仍算命中。
        assert!(scroll_at(p, 0, cap, 40, false, (track.x as f32 - 3.0, mid_y)).is_some());
        // 再远就不算。
        assert!(scroll_at(p, 0, cap, 40, false, (track.x as f32 - 12.0, mid_y)).is_none());
        // 负向臂：一屏放得下时既没有条也没有热区。
        assert!(scroll_at(p, 0, cap, cap, false, (track.x as f32 + 3.0, mid_y)).is_none());
    }

    /// m309：当前层要「不透明底 + 左缘竖条」两个信号。
    ///
    /// 颜色令牌与绘制路径没有运行时可读的把手 ⇒ 用源码钉子守接线（同 `canvas`）。
    /// ★ 钉的是**不许退回 `slot_cur`**：那是给缩略图格压图用的 6% 叠加，压在实心
    ///   树底上只差 14 级灰，用户看不到自己在哪一层。
    #[test]
    fn current_layer_row_uses_a_solid_fill_and_a_side_bar() {
        let all = include_str!("tree.rs");
        let prod = &all[..all.find("\n#[cfg(test)]").unwrap_or(all.len())];
        assert!(prod.contains("tk::tree_cur()"), "当前层没用专用底令牌");
        // 竖条贴面板内缘：`panel.x + 3.0` 出现即证明画了一条与缩进无关的标记线。
        assert!(
            prod.contains("panel.x as f32 + 3.0"),
            "当前层少了左缘竖条：只剩底色时与 hover 分不开"
        );
        assert!(
            !prod.contains("Paint::fill(tk::slot_cur())"),
            "当前层又退回缩略图格的 6% 叠加了——压在实心树底上等于没标"
        );
    }
}
