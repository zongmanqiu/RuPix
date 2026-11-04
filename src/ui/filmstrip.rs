//! 底部缩略图条（filmstrip）：几何 / 命中 / 绘制。
//!
//! 开关在 [`crate::ui::chrome::Chrome::filmstrip`]（m53 起随 ini 键 `filmstrip` 落盘）。
//!
//! **条的高度从画布内容区里扣掉**：`ImageCanvas` 用剩下的矩形摆图（见它的
//! `last_work`），所以图不会被压住半排缩略图——这是与悬浮工具栏最大的区别，
//! 工具栏是盖在画面上的浮层，条是常驻的一块地。
//!
//! 缩略图本身由 [`crate::core::thumbs::Thumbs`] 后台解码，这里只管"摆在哪、
//! 画成什么样、点到了第几格"。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windui::geometry::Rect;
use windui::render::image::Fit;
use windui::render::{Canvas, Paint};

use super::tokens as tk;

use super::ImageCache;
use crate::core::pixels::RgbaImage;

/// 条的总高（逻辑 dp，含上下留白）。
pub const STRIP_H: f32 = 96.0;

/// 格子的基准宽/高。窄窗口下按可用宽度均分，高度不变。
const ITEM_W: f32 = 96.0;
const ITEM_H: f32 = 68.0;
/// 格子间距。
const GAP: f32 = 6.0;
/// 条两端的留白。
const MARGIN: f32 = 12.0;
/// 缩略图在格子里缩进去多少（露出背板当边框）。
const INSET: f32 = 3.0;

// 配色取自全 ui 共用的令牌（`tk::panel_bg()` 那一组），本文件不自带色值；
// 背板与格子跟着主题走（白条 / 深条），当前格描边与工具栏激活态同一个色。

/// 条放得下的最低视口高度。
///
/// 比"条本身 + 一点画面"再宽松些：**窗口很矮时与其让画面只剩一条缝，不如干脆
/// 不显示条**（此时也不占布局空间）。
pub fn fits(vh: f32) -> bool {
    vh >= STRIP_H + 140.0
}

/// 一屏能放几格（至少 1 格）。视口窄到放不下一整格时也返回 1——
/// 布局会把格子压窄，压到 0 反而会让"点了没反应"无从排查。
pub fn capacity(vw: f32) -> usize {
    let avail = vw - 2.0 * MARGIN;
    if avail < ITEM_W * 0.5 {
        return 1;
    }
    (((avail + GAP) / (ITEM_W + GAP)).floor() as i32).max(1) as usize
}

/// `n` 个格子在视口里的矩形（**视口绝对坐标**，与命中测试同一套坐标）。
///
/// 整排居中：目录里只剩两三张时不该偏在左边。
pub fn layout(view: (f32, f32, f32, f32), n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let (vx, vy, vw, vh) = view;
    let avail = vw - 2.0 * MARGIN;
    // 均分可用宽度，但不让单格比基准款还宽（只剩两格时不该撑成两个大方块）。
    let iw = ((avail - (n as f32 - 1.0) * GAP) / n as f32).clamp(8.0, ITEM_W * 1.5);
    let total = n as f32 * iw + (n as f32 - 1.0) * GAP;
    let x0 = vx + (vw - total) / 2.0;
    let y0 = vy + vh - STRIP_H + (STRIP_H - ITEM_H) / 2.0;
    (0..n)
        .map(|i| {
            Rect::new(
                (x0 + i as f32 * (iw + GAP)) as i32,
                y0 as i32,
                iw as i32,
                ITEM_H as i32,
            )
        })
        .collect()
}

/// 命中测试：点到了第几格（返回 `items` 里的下标）。
pub fn hit(items: &[(usize, Rect)], pos: (f32, f32)) -> Option<usize> {
    items.iter().position(|(_, r)| {
        pos.0 >= r.x as f32
            && pos.0 < (r.x + r.w) as f32
            && pos.1 >= r.y as f32
            && pos.1 < (r.y + r.h) as f32
    })
}

/// 条底板的矩形（**视口绝对坐标**）：贴着 `view` 底边的 [`STRIP_H`] 那一条。
///
/// `paint` 的底板与 [`layout`] 的格子必须从同一份 `view` 推出——两者同底，
/// 格子才装在板里；底板一旦推出视口（曾多推出一整条高），条就只剩浮空的格子。
pub fn panel(view: (f32, f32, f32, f32)) -> Rect {
    let (vx, vy, vw, vh) = view;
    Rect::new(
        vx as i32,
        (vy + vh - STRIP_H) as i32,
        vw as i32,
        STRIP_H as i32,
    )
}

/// 上屏纹理缓存上限。超过时按最后使用时间逐张淘汰到 3/4（LRU），
/// 而非整批清空——后者会让大目录连翻几百张时突然全员重上传，一帧闪/顿。
const MAX_KEPT: usize = 128;
/// 每次触发淘汰时的目标剩余数（`MAX_KEPT` 的 3/4，摊销排序成本）。
const KEEP_TARGET: usize = MAX_KEPT - MAX_KEPT / 4;

/// 缩略图上屏纹理的缓存（按 `RgbaImage::gen` 判脏，与画布主位图同一套做法）。
///
/// 为什么按路径存而不是按格子存：目录翻页时窗口会滑动，同一格先后装的是不同
/// 文件，按格子存会让每格频繁重新上传纹理。
pub struct Strip {
    imgs: RefCell<HashMap<PathBuf, (ImageCache, u64)>>,
    stamp: Cell<u64>,
}

impl Strip {
    pub fn new() -> Self {
        Self {
            imgs: RefCell::new(HashMap::new()),
            stamp: Cell::new(0),
        }
    }

    /// 画整条。`slots` 是每一格的 (目录序号, 格子矩形, 清单里的路径)，`thumb` 按路径要图
    /// （未就绪为 `None`，那一格留空）。
    ///
    /// ★ m100：路径是**借来的**，缩略图由调用方现取。旧签名收
    ///   `&[Option<(PathBuf, Rc<RgbaImage>)>]`，画布每帧先克隆整屏的 `PathBuf` 再递进来——
    ///   纹理缓存的键本来就是路径的**值**，克隆那一份除了当场丢掉没有别的用途。
    pub fn paint(
        &self,
        canvas: &mut dyn Canvas,
        view: (f32, f32, f32, f32),
        slots: &[Slot<'_>],
        cur: usize,
        hover: Option<usize>,
        mut thumb: impl FnMut(&Path) -> Option<Rc<RgbaImage>>,
    ) {
        if slots.is_empty() {
            return;
        }
        let (vx, vy, _, _) = view;
        // 视口绝对坐标 → 画布原点相对坐标（与 chrome 同一套换算）。
        let local = |x: f32, y: f32, w: f32, h: f32| (x - vx, y - vy, w, h);
        let p = panel(view);
        let (px, py, pw, ph) = local(p.x as f32, p.y as f32, p.w as f32, p.h as f32);
        canvas.fill_round_rect(px, py, pw, ph, 10.0, &Paint::fill(tk::panel_bg()));

        for (i, slot) in slots.iter().enumerate() {
            let (idx, r, path) = (slot.idx, slot.rect, slot.path);
            let (lx, ly, lw, lh) = local(r.x as f32, r.y as f32, r.w as f32, r.h as f32);
            let is_cur = idx == cur;
            let bg = if is_cur {
                tk::slot_cur()
            } else if hover == Some(i) {
                tk::slot_hover()
            } else {
                tk::slot_bg()
            };
            canvas.fill_round_rect(lx, ly, lw, lh, 4.0, &Paint::fill(bg));

            if let Some(img) = thumb(path) {
                if let Some((dx, dy, dw, dh)) = contain(lx, ly, lw, lh, &img) {
                    // LRU touch + 取纹理：一次借用做完——stamp 递增、必要时剪枝、
                    // 更新当前项 stamp、拿到 windui `Image` 引用后立即 `draw_image`。
                    let next = self.stamp.get().wrapping_add(1);
                    self.stamp.set(next);
                    let mut m = self.imgs.borrow_mut();
                    // ★ 命中就走 `get_mut`：旧写法 `m.entry(path.clone())` 连命中也克隆一份
                    //   键（一帧一格的分配，纯浪费），而且前面那次 `contains_key` 是同一张表的
                    //   第二回查找。剪枝只在**真的要插新键**时才需要判。
                    if let Some(e) = m.get_mut(path) {
                        e.1 = next;
                        if let Some(im) = e.0.obtain(&img) {
                            canvas.draw_image(
                                im,
                                Rect::new(dx as i32, dy as i32, dw as i32, dh as i32),
                                Fit::Fill,
                                0.0,
                                1.0,
                            );
                        }
                    } else {
                        if m.len() >= MAX_KEPT {
                            lru_prune(&mut m, KEEP_TARGET);
                        }
                        let e = m
                            .entry(path.to_path_buf())
                            .or_insert_with(|| (ImageCache::new(), 0));
                        e.1 = next;
                        if let Some(im) = e.0.obtain(&img) {
                            canvas.draw_image(
                                im,
                                Rect::new(dx as i32, dy as i32, dw as i32, dh as i32),
                                Fit::Fill,
                                0.0,
                                1.0,
                            );
                        }
                    }
                }
            }

            if is_cur {
                canvas.stroke_round_rect(
                    lx,
                    ly,
                    lw,
                    lh,
                    4.0,
                    2.0,
                    &Paint::fill(tk::overlay_text_active()),
                );
            }
        }
    }
}

/// 一格的绘制输入（`rect` 是视口绝对坐标，与命中测试同一套）。
pub struct Slot<'a> {
    /// 清单里的序号（1-based 的 `gallery_pos` 语义由调用方换算，本模块内部 0-based）。
    pub idx: usize,
    pub rect: Rect,
    /// 这一格是哪张图——**借自清单**，画完就还。
    pub path: &'a Path,
}

/// 按 stamp 升序剪到 `target` 项（LRU 淘汰）。泛型 V 让单测能塞 `()` 免造 ImageCache。
fn lru_prune<V>(m: &mut HashMap<PathBuf, (V, u64)>, target: usize) {
    if m.len() <= target {
        return;
    }
    let mut keyed: Vec<(u64, PathBuf)> = m.iter().map(|(p, (_, s))| (*s, p.clone())).collect();
    keyed.sort_unstable_by_key(|(s, _)| *s);
    let drop_n = m.len() - target;
    for (_, p) in keyed.into_iter().take(drop_n) {
        m.remove(&p);
    }
}

impl Default for Strip {
    fn default() -> Self {
        Self::new()
    }
}

/// 缩略图等比 contain 进格子后的矩形（相对画布原点）。尺寸为零的图直接不画。
fn contain(x: f32, y: f32, w: f32, h: f32, img: &RgbaImage) -> Option<(f32, f32, f32, f32)> {
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let avail_w = (w - 2.0 * INSET).max(1.0);
    let avail_h = (h - 2.0 * INSET).max(1.0);
    let s = (avail_w / img.width as f32).min(avail_h / img.height as f32);
    let dw = img.width as f32 * s;
    let dh = img.height as f32 * s;
    Some((x + (w - dw) / 2.0, y + (h - dh) / 2.0, dw, dh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use windui::geometry::Color;

    /// 布局必须整排居中：左右两侧留白相等。
    #[test]
    fn row_is_centered() {
        let vw = 800.0;
        let rects = layout((0.0, 0.0, vw, 400.0), 5);
        assert_eq!(rects.len(), 5);
        let left = rects[0].x as f32;
        let right = vw - (rects[4].x + rects[4].w) as f32;
        assert!(
            (left - right).abs() < 1.5,
            "左右留白应相等：{left} vs {right}"
        );
    }

    /// 格子不许越出视口（含两端留白），也不许互相重叠。
    #[test]
    fn items_stay_inside_and_never_overlap() {
        for vw in [120.0, 300.0, 640.0, 1024.0, 2560.0] {
            let n = capacity(vw);
            let rects = layout((0.0, 0.0, vw, 400.0), n);
            assert!(!rects.is_empty());
            for (i, r) in rects.iter().enumerate() {
                assert!(
                    r.x >= 0 && (r.x + r.w) as f32 <= vw + 0.5,
                    "vw={vw} 第 {i} 格越界：{r:?}"
                );
                if i + 1 < rects.len() {
                    assert!(
                        (r.x + r.w) as f32 <= rects[i + 1].x as f32,
                        "vw={vw} 第 {i} 与后一格重叠"
                    );
                }
            }
        }
    }

    /// 条落在视口底部、且不高于视口（窗口再矮也不能画到视口外面去）。
    #[test]
    fn strip_sits_at_the_bottom() {
        let (_, _, vh) = (0.0, 0.0, 400.0);
        let rects = layout((0.0, 0.0, 800.0, vh), 3);
        let bottom = (rects[0].y + rects[0].h) as f32;
        assert!(bottom <= vh, "格子底边不该超出视口：{bottom} > {vh}");
        assert!(vh - bottom < STRIP_H, "格子应落在条的那一条里");
    }

    /// 命中测试：格内命中、格外不命中。
    #[test]
    fn hit_only_inside_a_slot() {
        let items: Vec<(usize, Rect)> = vec![
            (0, Rect::new(0, 0, 100, 60)),
            (1, Rect::new(106, 0, 100, 60)),
        ];
        assert_eq!(hit(&items, (50.0, 30.0)), Some(0));
        assert_eq!(hit(&items, (150.0, 30.0)), Some(1));
        // 格间缝隙与格子外。
        assert_eq!(hit(&items, (103.0, 30.0)), None);
        assert_eq!(hit(&items, (50.0, 61.0)), None);
        assert_eq!(hit(&[], (0.0, 0.0)), None);
    }

    /// contain 保持比例且居中；零尺寸的图不画（除零会产生 NaN 坐标）。
    #[test]
    fn contain_keeps_aspect_and_skips_empty() {
        let img = RgbaImage::new(200, 100, vec![0u8; 200 * 100 * 4]);
        let (dx, dy, dw, dh) = contain(0.0, 0.0, 90.0, 60.0, &img).expect("正常图该有矩形");
        assert!((dw / dh - 2.0).abs() < 0.01, "比例应保住：{dw}x{dh}");
        assert!((dx + dw / 2.0 - 45.0).abs() < 0.6, "横向应居中");
        assert!((dy + dh / 2.0 - 30.0).abs() < 0.6, "纵向应居中");

        let zero = RgbaImage::new(0, 0, Vec::new());
        assert!(contain(0.0, 0.0, 90.0, 60.0, &zero).is_none());
    }

    /// 太矮的窗口不放条（`fits` 守着，免得画面只剩一条缝）。
    #[test]
    fn does_not_fit_in_a_short_window() {
        assert!(fits(400.0));
        assert!(!fits(120.0));
    }

    /// 底板必须贴着视口底边、并把所有格子整个装进去——底板一旦画到视口下沿
    /// 之外（把 `view` 底边又往下多推了整条高），条就只剩浮空的格子没有背板。
    #[test]
    fn panel_sits_on_the_view_bottom_and_swallows_the_slots() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let p = panel(view);
        assert_eq!(p.y + p.h, 600, "板底边必须贴视口底");
        assert_eq!(p.h, STRIP_H as i32, "板高就是条高");
        for (i, r) in layout(view, 5).iter().enumerate() {
            assert!(
                r.y >= p.y && r.y + r.h <= p.y + p.h,
                "第 {} 格 {r:?} 没装进板 {p:?} 里",
                i + 1
            );
        }
    }

    /// LRU 剪枝：按 stamp 升序丢，保留最新 `target` 项，不动 stamp 相同或更大的。
    #[test]
    fn lru_prune_keeps_recent() {
        let mut m: HashMap<PathBuf, ((), u64)> = HashMap::new();
        for i in 0..10u64 {
            m.insert(PathBuf::from(format!("p{i}")), ((), i));
        }
        lru_prune(&mut m, 4);
        assert_eq!(m.len(), 4);
        // 应保留 stamp 最大的 4 项：p6/p7/p8/p9。
        for i in 0..6 {
            assert!(
                !m.contains_key(Path::new(&format!("p{i}"))),
                "p{i} stamp={i} 属最旧应被淘汰"
            );
        }
        for i in 6..10 {
            assert!(
                m.contains_key(Path::new(&format!("p{i}"))),
                "p{i} stamp={i} 属最新应保留"
            );
        }
    }

    /// LRU 未超上限时不剪：`target ≥ len` 走早退分支。
    #[test]
    fn lru_prune_is_noop_below_target() {
        let mut m: HashMap<PathBuf, ((), u64)> = HashMap::new();
        for i in 0..3u64 {
            m.insert(PathBuf::from(format!("p{i}")), ((), i));
        }
        lru_prune(&mut m, 4);
        assert_eq!(m.len(), 3, "未超上限不该淘汰任何项");
    }

    /// 只记两件事的画布：每一格向调用方要的是**哪张图**、当前框描在**哪一格**。
    /// 其余画全丢弃——这条判据问的不是长什么样，是接线对不对。
    struct SpyCells {
        framed: Vec<(i32, i32)>,
    }

    impl Canvas for SpyCells {
        fn dpi_scale(&self) -> f32 {
            1.0
        }
        fn save(&mut self) {}
        fn restore(&mut self) {}
        fn clip_rect(&mut self, _r: Rect) {}
        fn push_layer(&mut self, _opacity: f32) {}
        fn pop_layer(&mut self) {}
        fn fill_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _paint: &Paint) {}
        fn fill_round_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _r: f32, _p: &Paint) {}
        fn stroke_round_rect(
            &mut self,
            x: f32,
            y: f32,
            _w: f32,
            _h: f32,
            _r: f32,
            _line: f32,
            _p: &Paint,
        ) {
            self.framed.push((x as i32, y as i32));
        }
        fn draw_line(&mut self, _x0: f32, _y0: f32, _x1: f32, _y1: f32, _w: f32, _p: &Paint) {}
        fn fill_circle(&mut self, _cx: f32, _cy: f32, _r: f32, _paint: &Paint) {}
        fn draw_shadow(
            &mut self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _r: f32,
            _blur: f32,
            _c: Color,
        ) {
        }
        fn draw_image(
            &mut self,
            _img: &windui::render::image::Image,
            _dst: Rect,
            _fit: Fit,
            _r: f32,
            _o: f32,
        ) {
        }
        fn draw_text(
            &mut self,
            _t: &str,
            _r: Rect,
            _c: Color,
            _a: windui::spec::Align,
            _ts: &windui::text::TextStyle,
        ) {
        }
        fn measure_text(
            &mut self,
            text: &str,
            ts: &windui::text::TextStyle,
        ) -> windui::geometry::Size {
            windui::geometry::Size::new(text.len() as i32, ts.size as i32)
        }
    }

    /// ★ m100：格子改成向调用方**借**路径之后，"第几格要哪张图"这条线得自己钉住。
    ///
    /// 本轮把"画布整排克隆路径 + 一份图 Vec 递进来"换成"格子里存借用、要图时现取"，
    /// 接线错了屏幕上就是**整排缩略图错位一格**，而 `layout` / `capacity` / `hit` 那几条
    /// 纯几何用例照旧全绿。所以窗口起点故意不从 0 开始（`idx` 从 2 数起）：起点为 0 时
    /// "序号当位置用"和"位置当序号用"看不出差别。
    #[test]
    fn each_cell_asks_for_its_own_path_and_the_frame_follows_the_index() {
        let paths: Vec<PathBuf> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|n| PathBuf::from(*n))
            .collect();
        let view = (0.0, 0.0, 800.0, 600.0);
        let rects = layout(view, paths.len());
        let slots: Vec<Slot<'_>> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| Slot {
                idx: 2 + i,
                rect: rects[i],
                path: p.as_path(),
            })
            .collect();
        let asked = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&asked);
        let mut spy = SpyCells { framed: Vec::new() };
        Strip::new().paint(
            &mut spy,
            view,
            &slots,
            4, // 当前那张的**序号**（= 第三格），不是位置号
            None,
            |p| {
                sink.borrow_mut().push(p.to_path_buf());
                None::<Rc<RgbaImage>>
            },
        );
        assert_eq!(
            *asked.borrow(),
            paths,
            "每格要的路径必须就是它自己带的那一份，且按格子顺序各要一次"
        );
        assert_eq!(
            spy.framed,
            vec![(rects[2].x, rects[2].y)],
            "当前框要落在序号 4 那一格：既不能一个都不描，也不能描到位置 4 那一格"
        );
    }
}
