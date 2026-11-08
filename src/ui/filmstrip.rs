//! 缩略图条（filmstrip）：几何 / 命中 / 绘制。
//!
//! 开关在 [`crate::ui::chrome::Chrome::filmstrip`]（m53 起随 ini 键 `filmstrip` 落盘），
//! 贴哪一边在 [`crate::ui::chrome::Chrome::filmstrip_left`]（m253，ini 键 `filmstrip_left`）。
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
use windui::spec::Align;
use windui::text::TextStyle;

use super::tokens as tk;

use super::ImageCache;
use crate::core::pixels::RgbaImage;

/// 底条的总高（逻辑 dp，含上下留白）。
pub const STRIP_H: f32 = 96.0;

/// 左侧竖条的总宽（逻辑 dp）：一列格子 + 两端留白。
///
/// 比底条窄一半多：竖排只有一列，112 里 24 是留白、88 给缩略图。
pub const STRIP_W: f32 = 112.0;

/// 底条要求的最小内容区高度（条 + 一点画面）。
///
/// ★ 这个数有两个消费者：`fits`（显示判据）与 `grow_to_fit`（放不下时把窗口放大到哪）。
///   两处必须是同一个常量，否则会出现「放大完仍判放不下」的鬼打墙。
pub const MIN_VIEW_H: f32 = STRIP_H + 140.0;

/// 左条要求的最小内容区宽度（条 + 一点画面）。同上，`fits` 与 `grow_to_fit` 共用。
pub const MIN_VIEW_W: f32 = STRIP_W + 240.0;

/// 条贴哪一边（ini 键 `filmstrip_left`，m253）。
///
/// 底条横排、左条竖排——**同一个模块、同一套格子与命中**，只是主轴换了：
/// 底条按宽度均分格宽，左条按高度均分格高。判据收在这里，调用点不许各算一遍。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Bottom,
    Left,
}

impl Side {
    /// 布尔口径（`config.filmstrip_left` / `chrome.filmstrip_left`）→ 边。
    pub fn from_left(left: bool) -> Self {
        if left {
            Self::Left
        } else {
            Self::Bottom
        }
    }
}

/// 条画在哪：贴哪一边 + 它所属的**完整**内容区（扣条之前）。
///
/// ★ 合成一个参数不是为了省字：`layout` / `panel` / `paint` 必须从**同一份**
///   `view` 与**同一边**推出来（分开传过三次，一次传错就整条错位）。
#[derive(Clone, Copy)]
pub struct Placement {
    pub side: Side,
    pub view: (f32, f32, f32, f32),
}

impl Placement {
    pub fn new(side: Side, view: (f32, f32, f32, f32)) -> Self {
        Self { side, view }
    }
}

/// 格子的基准宽/高。窄窗口下按可用宽度均分，高度不变。
const ITEM_W: f32 = 96.0;
const ITEM_H: f32 = 68.0;
/// 格子间距。
const GAP: f32 = 6.0;
/// 条两端的留白。
const MARGIN: f32 = 12.0;
/// 缩略图在格子里缩进去多少（露出背板当边框）。
const INSET: f32 = 3.0;

/// 滚动指示的厚度（压在条的内边缘上，不占格子）。
///
/// 2dp 是"指示"的尺寸，但**没人能拖住 2dp**（用户实测：鼠标对不准）⇒ m259 起
/// 视觉 6dp、命中热区 [`SCROLL_HIT`] 16dp（居中铺开），另加滑块拖动与点轨道跳转。
const SCROLL_T: f32 = 6.0;
/// 滚动指示的**命中热区**厚度：视觉条的 16dp 版，只用于命中，不参与绘制。
const SCROLL_HIT: f32 = 16.0;
/// 滑块最短长度：目录几千张时纯按比例会缩成一个点，看不见也抓不住。
const SCROLL_MIN: f32 = 24.0;
/// 底条的轨道离条**上缘**多远。
///
/// ★ 用户 2026-11-08 实报「缩略图在下方时，滚动条和底部边框线冲突，鼠标无法拖动」：
///   无边框窗最外 8dp 是缩放命中带（windui 的 `handle_nchittest`），压在条下缘的条
///   正好落进去，按下先被系统拿去缩放。所以底条改压**上缘**（格子之上、离窗口底
///   整整一条高）；左条仍压靠画面的右缘，那边本来就不碰窗口边框。
/// 3dp 让 16dp 热区上下各探出 5dp 时刚好不啃到格子（格子从 `p.y + 14` 起）。
const SCROLL_TOP_GAP: f32 = 3.0;

/// 序号：字号与**描边**厚度（m260）。
///
/// 用户 2026-11-08 的口径是"数字有描边、一黑一白"，**不是**一块有底色的标签：
/// 「数字有线条包裹，不是一整个矩形区域都是其它颜色背景」。所以这里只画字——
/// 黑字 + 八邻域各偏 1px 的白字，读起来就是 1px 白描边。
const BADGE_FONT: f32 = 10.0;
/// 描边厚度（逻辑 px）。1px 用户点名要的。
const BADGE_EDGE: f32 = 1.0;
/// 八邻域偏移：先按这八个位移各画一遍白字，再正中压一遍黑字。
const BADGE_RING: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

// ★ 尺寸关系在**编译期**钉死：2dp 的条没人抓得住（用户实测"鼠标对不准"），
//   而热区必须比视觉条宽出一倍以上，否则边缘那一圈仍然是抓不住的死区。
//   写成 `const` 断言而不是测试——这是常量之间的约束，跑测试才能发现就太晚了。
const _: () = assert!(SCROLL_T >= 6.0);
const _: () = assert!(SCROLL_HIT >= SCROLL_T * 2.0);
// 八邻域的偏移是字面 ±1：改描边厚度时必须连 `BADGE_RING` 一起改，不能只动常量。
const _: () = assert!(BADGE_EDGE == 1.0);

// 配色取自全 ui 共用的令牌（`tk::panel_bg()` 那一组），本文件不自带色值；
// 背板与格子跟着主题走（白条 / 深条），当前格描边与工具栏激活态同一个色。

/// 条放得下吗（`vw` / `vh` 是**扣条之前**的整块内容区）。
///
/// 比"条本身 + 一点画面"再宽松些：**窗口很矮时与其让画面只剩一条缝，不如干脆
/// 不显示条**（此时也不占布局空间）。左条换了一根轴——扣的是宽，所以两个方向都要看。
pub fn fits(side: Side, vw: f32, vh: f32) -> bool {
    match side {
        Side::Bottom => vh >= MIN_VIEW_H,
        // 左条扣的是宽，两个方向都要看：宽够放下条 + 一点画面，高放得下一格 + 留白。
        Side::Left => vw >= MIN_VIEW_W && vh >= ITEM_H + 2.0 * MARGIN + 40.0,
    }
}

/// 让 `side` 放得下所需的**最小内容区**：只补不足的那一维，已经更大的不缩。
///
/// 用于「用户刚打开条 / 换了边，但窗口太小」那一拍：先把窗口放大到刚好放得下，
/// 再让条上屏（m254）。放大由 `EventCtx::set_window_size` 落地——它按物理像素、
/// 超工作区自动夹取、最大化 / 全屏时无操作，所以这里只给逻辑 dp 目标。
pub fn grow_to_fit(side: Side, vw: f32, vh: f32) -> (f32, f32) {
    match side {
        Side::Bottom => (vw, vh.max(MIN_VIEW_H)),
        Side::Left => (vw.max(MIN_VIEW_W), vh),
    }
}

/// 一屏能放几格（至少 1 格）。`avail` 是**条的主轴长度**：底条给宽、左条给高。
///
/// 视口窄到放不下一整格时也返回 1——布局会把格子压窄，压到 0 反而会让
/// "点了没反应"无从排查。
pub fn capacity(side: Side, avail: f32) -> usize {
    let (item, margins) = match side {
        Side::Bottom => (ITEM_W, 2.0 * MARGIN),
        Side::Left => (ITEM_H, 2.0 * MARGIN),
    };
    let avail = avail - margins;
    if avail < item * 0.5 {
        return 1;
    }
    (((avail + GAP) / (item + GAP)).floor() as i32).max(1) as usize
}

/// `n` 个格子在视口里的矩形（**视口绝对坐标**，与命中测试同一套坐标）。
///
/// 整排居中：目录里只剩两三张时不该偏在左边（左条则是偏上）。
pub fn layout(at: Placement, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let (vx, vy, vw, vh) = at.view;
    match at.side {
        Side::Bottom => {
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
        Side::Left => {
            // 竖排：格宽固定（条就那么宽），格高按可用高度均分。
            let avail = vh - 2.0 * MARGIN;
            let ih = ((avail - (n as f32 - 1.0) * GAP) / n as f32).clamp(8.0, ITEM_H * 1.5);
            let total = n as f32 * ih + (n as f32 - 1.0) * GAP;
            let y0 = vy + (vh - total) / 2.0;
            let iw = (STRIP_W - 2.0 * MARGIN).min(ITEM_W);
            let x0 = vx + (STRIP_W - iw) / 2.0;
            (0..n)
                .map(|i| {
                    Rect::new(
                        x0 as i32,
                        (y0 + i as f32 * (ih + GAP)) as i32,
                        iw as i32,
                        ih as i32,
                    )
                })
                .collect()
        }
    }
}

/// 相邻一格的跨距（含间隙，**方向性**）：底条给横向、左条给纵向。
///
/// 拖动换算与命中都要它。格子宽度是按 `n` 均分的，所以不能写死 `ITEM_W`——
/// 目录只剩三张时一格会撑到 `ITEM_W * 1.5`，写死就会"拖了半天不动一格"。
pub fn pitch(side: Side, items: &[(usize, Rect)]) -> f32 {
    if items.len() >= 2 {
        let step = match side {
            Side::Bottom => (items[1].1.x - items[0].1.x) as f32,
            Side::Left => (items[1].1.y - items[0].1.y) as f32,
        };
        if step > 0.0 {
            return step;
        }
    }
    match side {
        Side::Bottom => ITEM_W + GAP,
        Side::Left => ITEM_H + GAP,
    }
}

/// 滚动指示：`(轨道, 滑块)`，底条压**上缘**（格子之上）、左条压**右缘**（靠画面那侧）。
///
/// `None` = 目录一屏就放得下（此时不该出现"能滚"的暗示）。
/// 滑块长度按 `n / len` 缩，最短 [`SCROLL_MIN`]，位置按 `first / (len - n)` 线性映射——
/// 与 `layout` 用同一份 `first` / `n`，所以指示永远和屏上看的窗口一致。
pub fn scroll_bar(at: Placement, first: usize, n: usize, len: usize) -> Option<(Rect, Rect)> {
    let n = n.clamp(1, len.max(1));
    if len <= n {
        return None;
    }
    let p = panel(at);
    let max_first = (len - n) as f32;
    let traveled = (first.min(len - n)) as f32 / max_first;
    // ★ 滑块与轨道都在**整数域**里算：f32 各自取整会让"滚到最后一屏"差 1 像素
    //   （实测 787 vs 788），而末屏贴不齐正是最容易被人一眼看出来的那种错。
    match at.side {
        Side::Bottom => {
            let track_x = (p.x as f32 + MARGIN) as i32;
            let track_w = ((p.w as f32 - 2.0 * MARGIN).max(1.0)) as i32;
            let thumb_w = ((track_w as f32 * n as f32 / len as f32) as i32)
                .max(SCROLL_MIN as i32)
                .min(track_w);
            // 上缘，不是下缘：下缘那 8dp 归窗口缩放边框，指针一进去就被 NC 命中吃掉。
            let y = (p.y as f32 + SCROLL_TOP_GAP) as i32;
            let slide = ((track_w - thumb_w) as f32 * traveled) as i32;
            Some((
                Rect::new(track_x, y, track_w, SCROLL_T as i32),
                Rect::new(track_x + slide, y, thumb_w, SCROLL_T as i32),
            ))
        }
        Side::Left => {
            let track_y = (p.y as f32 + MARGIN) as i32;
            let track_h = ((p.h as f32 - 2.0 * MARGIN).max(1.0)) as i32;
            let thumb_h = ((track_h as f32 * n as f32 / len as f32) as i32)
                .max(SCROLL_MIN as i32)
                .min(track_h);
            let x = (p.x as f32 + p.w as f32 - SCROLL_T - 2.0) as i32;
            let slide = ((track_h - thumb_h) as f32 * traveled) as i32;
            Some((
                Rect::new(x, track_y, SCROLL_T as i32, track_h),
                Rect::new(x, track_y + slide, SCROLL_T as i32, thumb_h),
            ))
        }
    }
}

/// 滚动指示的**命中**：指针落在热区里就返回 `(轨道, 滑块)`（两条仍是绘制用的矩形）。
///
/// 热区比视觉条厚（[`SCROLL_HIT`] vs [`SCROLL_T`]），居中铺开——2dp/6dp 的条没人拖得住，
/// 但画粗了会难看。判据与 [`scroll_bar`] 同源：`None` = 一屏放得下 = 既没有指示也没有热区。
pub fn scroll_at(
    at: Placement,
    first: usize,
    n: usize,
    len: usize,
    pos: (f32, f32),
) -> Option<(Rect, Rect)> {
    let (track, thumb) = scroll_bar(at, first, n, len)?;
    let pad = ((SCROLL_HIT - SCROLL_T) / 2.0) as i32;
    let hot = match at.side {
        Side::Bottom => Rect::new(track.x, track.y - pad, track.w, SCROLL_HIT as i32),
        Side::Left => Rect::new(track.x - pad, track.y, SCROLL_HIT as i32, track.h),
    };
    let inside = pos.0 >= hot.x as f32
        && pos.0 < (hot.x + hot.w) as f32
        && pos.1 >= hot.y as f32
        && pos.1 < (hot.y + hot.h) as f32;
    inside.then_some((track, thumb))
}

/// 序号**文字盒**的矩形（**视口绝对坐标**）：贴在格子右下角，内缩 `INSET + 描边厚度`
/// （给那圈白边留位，否则描边会被格子边裁掉一半）。
///
/// `tw` / `th` 是量出来的文字盒；装不下（窄窗口把格子压得很小，或目录到了五位数）
/// 就返回 `None`——宁可这一格不标，也不让字糊到格子外面去。
fn badge_rect(cell: Rect, tw: f32, th: f32) -> Option<Rect> {
    let pad = INSET + BADGE_EDGE;
    if tw > cell.w as f32 - 2.0 * pad || th > cell.h as f32 - 2.0 * pad {
        return None;
    }
    Some(Rect::new(
        (cell.x as f32 + cell.w as f32 - pad - tw).round() as i32,
        (cell.y as f32 + cell.h as f32 - pad - th).round() as i32,
        tw.round() as i32,
        th.round() as i32,
    ))
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

/// 条底板的矩形（**视口绝对坐标**）：底条贴 `view` 底边、左条贴 `view` 左边。
///
/// `paint` 的底板与 [`layout`] 的格子必须从同一份 `view` 推出——两者同边，
/// 格子才装在板里；底板一旦推出视口（曾多推出一整条高），条就只剩浮空的格子。
pub fn panel(at: Placement) -> Rect {
    let (vx, vy, vw, vh) = at.view;
    match at.side {
        Side::Bottom => Rect::new(
            vx as i32,
            (vy + vh - STRIP_H) as i32,
            vw as i32,
            STRIP_H as i32,
        ),
        Side::Left => Rect::new(vx as i32, vy as i32, STRIP_W as i32, vh as i32),
    }
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
        at: Placement,
        slots: &[Slot<'_>],
        cur: usize,
        hover: Option<usize>,
        mut thumb: impl FnMut(&Path) -> Option<Rc<RgbaImage>>,
    ) {
        if slots.is_empty() {
            return;
        }
        // ★ `layout` / `panel` 给的是**视口绝对坐标**，而画布（`core::paint_node` 以 abs
        //   递归、不做 translate）要的也正是绝对坐标 ⇒ 这里一个像素都不许挪。
        //   m255 之前这里减过 `vx/vy`：底条时恒为 0 侥幸无害，贴左时整条左移 112。
        let local = |x: f32, y: f32, w: f32, h: f32| (x, y, w, h);
        let p = panel(at);
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

            // 序号（m260 提出、m261 改成**描边文字**）：只画数字，不画底色块。
            //
            // 用户两次口径合起来才完整：「数字有线条包裹，不是一整个矩形区域都是
            // 其它颜色背景」「可以理解为数字有描边，一黑一白，数字一定能看清」。
            // 所以先按八邻域各偏 1px 画白字勾出字形轮廓，正中再压一遍黑字：
            // 浅色缩略图上黑字自己就够，深色缩略图上靠这圈白边把字形勾出来，
            // 而且一格像素都不被底衬吃掉。
            let label = (idx + 1).to_string();
            let ts = TextStyle::new(BADGE_FONT);
            let m = canvas.measure_text(&label, &ts);
            if let Some(b) = badge_rect(r, m.w as f32, m.h as f32) {
                for (dx, dy) in BADGE_RING {
                    canvas.draw_text(
                        &label,
                        Rect::new(b.x + dx, b.y + dy, b.w, b.h),
                        tk::BADGE_HALO,
                        Align::Center,
                        &ts,
                    );
                }
                canvas.draw_text(&label, b, tk::BADGE_TEXT, Align::Center, &ts);
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
        let rects = layout(Placement::new(Side::Bottom, (0.0, 0.0, vw, 400.0)), 5);
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
            let n = capacity(Side::Bottom, vw);
            let rects = layout(Placement::new(Side::Bottom, (0.0, 0.0, vw, 400.0)), n);
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
        let rects = layout(Placement::new(Side::Bottom, (0.0, 0.0, 800.0, vh)), 3);
        let bottom = (rects[0].y + rects[0].h) as f32;
        assert!(bottom <= vh, "格子底边不该超出视口：{bottom} > {vh}");
        assert!(vh - bottom < STRIP_H, "格子应落在条的那一条里");
    }

    /// 左条是**竖排**：同一列、纵坐标递增，整列纵向居中且不出视口。
    #[test]
    fn left_strip_stacks_vertically() {
        let vh = 600.0;
        let rects = layout(Placement::new(Side::Left, (0.0, 0.0, 1200.0, vh)), 4);
        assert_eq!(rects.len(), 4);
        for w in rects.windows(2) {
            assert_eq!(w[0].x, w[1].x, "竖排必须同一列");
            assert!(w[0].y + w[0].h <= w[1].y, "相邻两格不许重叠");
        }
        let top = rects[0].y as f32;
        let bottom = vh - (rects[3].y + rects[3].h) as f32;
        assert!(
            (top - bottom).abs() < 1.5,
            "整列应纵向居中：{top} vs {bottom}"
        );
        for r in &rects {
            assert!(
                r.x >= 0 && (r.x + r.w) as f32 <= STRIP_W + 0.5,
                "格子不许越出左条：{r:?}"
            );
        }
    }

    /// 左条底板贴视口左边、通高，并把所有格子整个装进去。
    #[test]
    fn left_panel_hugs_the_view_left_and_swallows_the_slots() {
        let view = (0.0, 0.0, 1200.0, 700.0);
        let p = panel(Placement::new(Side::Left, view));
        assert_eq!(p.x, 0, "板左边必须贴视口左");
        assert_eq!(p.w, STRIP_W as i32, "板宽就是条宽");
        assert_eq!(p.h, 700, "左条通高");
        for (i, r) in layout(Placement::new(Side::Left, view), 6)
            .iter()
            .enumerate()
        {
            assert!(
                r.x >= p.x && (r.x + r.w) <= p.x + p.w && r.y >= p.y && r.y + r.h <= p.y + p.h,
                "第 {} 格 {r:?} 没装进板 {p:?} 里",
                i + 1
            );
        }
    }

    /// 左条按**高度**算格数（底条按宽度）：同一个 600，两条主轴给出的数不同。
    #[test]
    fn left_capacity_counts_by_height() {
        assert_eq!(capacity(Side::Left, 600.0), 7, "582/74 ⇒ 7 格");
        assert_eq!(capacity(Side::Bottom, 600.0), 5, "600/102 ⇒ 5 格");
        // 极窄也至少留一格，绝不返回 0（否则点了没反应无从排查）。
        assert_eq!(capacity(Side::Left, 10.0), 1);
    }

    /// ★ m258：拖动换算用的格距必须量**真实的**布局，不能写死 `ITEM_W + GAP`。
    ///
    /// 均分布局下只剩两三张时一格会被撑到 `ITEM_W * 1.5`，写死常量就会
    /// "拖了半天不动一格"；只有一格时没有相邻间距可量，才回落基准款。
    #[test]
    fn pitch_matches_the_laid_out_step() {
        let at = Placement::new(Side::Bottom, (0.0, 0.0, 800.0, 600.0));
        let rects = layout(at, 6);
        let items: Vec<(usize, Rect)> = rects.iter().enumerate().map(|(i, r)| (i, *r)).collect();
        assert_eq!(
            pitch(Side::Bottom, &items),
            (rects[1].x - rects[0].x) as f32
        );
        assert_eq!(pitch(Side::Bottom, &items[..1]), ITEM_W + GAP);
        assert_eq!(pitch(Side::Left, &items[..1]), ITEM_H + GAP);
        // 竖排量的是纵向间距。
        let left = Placement::new(Side::Left, (0.0, 0.0, 800.0, 600.0));
        let lr = layout(left, 6);
        let li: Vec<(usize, Rect)> = lr.iter().enumerate().map(|(i, r)| (i, *r)).collect();
        assert_eq!(pitch(Side::Left, &li), (lr[1].y - lr[0].y) as f32);
    }

    /// ★ m258：滚动指示只在"一屏放不下"时出现；滑块长度随可见比例、位置随首格。
    #[test]
    fn scroll_bar_appears_only_when_the_strip_cannot_show_everything() {
        let at = Placement::new(Side::Bottom, (0.0, 0.0, 800.0, 600.0));
        assert!(
            scroll_bar(at, 0, 8, 8).is_none(),
            "一屏放得下就不该暗示能滚"
        );
        assert!(scroll_bar(at, 0, 8, 7).is_none(), "目录比一屏还少");
        let (track, thumb) = scroll_bar(at, 0, 8, 40).expect("放不下就要有指示");
        let p = panel(at);
        assert!(
            track.y >= p.y && track.y + track.h <= p.y + p.h,
            "轨道压在条里，不许探出条外"
        );
        assert_eq!(thumb.x, track.x, "首格在起点时滑块贴左");
        assert!(
            thumb.w >= SCROLL_MIN as i32,
            "目录几千张时滑块不能缩成一个点"
        );
        assert!(thumb.w <= track.w, "滑块不许比轨道还长");
    }

    /// ★ 用户 2026-11-08：底条的滚动条要在**缩略图上方**，不许压在窗口底边。
    ///
    /// 负向臂同时钉住左条：贴左时滚动条仍压靠画面的右缘，别把"上缘"照抄到竖排上
    /// （那会占掉最上面一格的头部，而且右缘本来就不碰窗口边框）。
    #[test]
    fn bottom_scroll_bar_sits_above_the_cells_far_from_the_window_edge() {
        let at = Placement::new(Side::Bottom, (0.0, 0.0, 800.0, 600.0));
        let p = panel(at);
        let cells = layout(at, 8);
        let (track, thumb) = scroll_bar(at, 0, 8, 40).expect("放不下就该有指示");
        assert!(track.y >= p.y, "不许画到条外面去");
        assert!(
            (track.y + track.h) as f32 <= cells[0].y as f32,
            "整条滚动指示都该在格子上方：bar 底={} 格顶={}",
            track.y + track.h,
            cells[0].y
        );
        assert!(
            (p.y + p.h) - (track.y + track.h) > 8,
            "距窗口底边要留足一整条，不能落进 8dp 缩放带"
        );
        assert_eq!(thumb.y, track.y, "滑块与轨道同一条线");
        // 热区仍盖得住视觉条：条上缘那一拖必须能被 `scroll_at` 接住。
        assert!(scroll_at(
            at,
            0,
            8,
            40,
            (track.x as f32 + 4.0, track.y as f32 + SCROLL_T / 2.0)
        )
        .is_some());

        let left = Placement::new(Side::Left, (0.0, 0.0, 1200.0, 700.0));
        let lp = panel(left);
        let (lt, _) = scroll_bar(left, 0, 8, 40).expect("左条也要有指示");
        assert!(
            lt.x > lp.x + lp.w / 2,
            "左条的指示仍压在右缘（靠画面那侧），不是上缘"
        );
    }

    /// ★ m258：滚到最后一屏，滑块必须贴到轨道末端（右 / 下两头都要对）。
    #[test]
    fn scroll_bar_thumb_reaches_the_far_end_at_the_last_window() {
        let at = Placement::new(Side::Bottom, (0.0, 0.0, 800.0, 600.0));
        let (track, thumb) = scroll_bar(at, 32, 8, 40).unwrap();
        assert_eq!(thumb.x + thumb.w, track.x + track.w, "末屏滑块贴右");
        let left = Placement::new(Side::Left, (0.0, 0.0, 800.0, 600.0));
        let (lt, lb) = scroll_bar(left, 32, 8, 40).unwrap();
        assert_eq!(lb.y + lb.h, lt.y + lt.h, "末屏滑块贴底");
        assert!(lb.x >= panel(left).x && lb.x + lb.w <= panel(left).x + panel(left).w);
    }

    /// ★ m259：滚动指示的**热区**比视觉条厚，且与 `scroll_bar` 同一条存在性判据。
    ///
    /// 用户实报"2dp 太细，鼠标对不准"⇒ 视觉 6dp、热区 16dp；一屏放得下时两者都不存在
    /// （否则会给出"能滚"的假暗示）。
    #[test]
    fn scroll_hit_zone_is_fatter_than_the_painted_bar() {
        let at = Placement::new(Side::Bottom, (0.0, 0.0, 800.0, 600.0));
        let (track, _) = scroll_bar(at, 0, 8, 40).unwrap();
        let mid_y = track.y as f32 + track.h as f32 / 2.0;
        let mid_x = track.x as f32 + track.w as f32 / 2.0;
        // 横轴中线附近、纵向偏离 6dp：视觉条（6dp）已经出界，热区（16dp）仍在。
        assert!(scroll_at(at, 0, 8, 40, (mid_x, mid_y + 6.0)).is_some());
        assert!(scroll_at(at, 0, 8, 40, (mid_x, mid_y + 10.0)).is_none());
        // 放得下就没有热区。
        assert!(scroll_at(at, 0, 8, 8, (mid_x, mid_y)).is_none());
        // 左条：热区沿横向铺开。
        let left = Placement::new(Side::Left, (0.0, 0.0, 800.0, 600.0));
        let (lt, _) = scroll_bar(left, 0, 8, 40).unwrap();
        let lx = lt.x as f32 + lt.w as f32 / 2.0;
        let ly = lt.y as f32 + lt.h as f32 / 2.0;
        assert!(scroll_at(left, 0, 8, 40, (lx + 6.0, ly)).is_some());
        assert!(scroll_at(left, 0, 8, 40, (lx + 12.0, ly)).is_none());
    }

    /// ★ m255：当前格描边必须画在 `layout` 给的绝对坐标上（不许再减视口原点）。
    ///
    /// 底条时 `vx = 0`，减不减看不出来；贴左时 `vx = 112`，减一次整条就左移 112，
    /// 而命中仍在原位（2026-11-08 用户报障：贴左后标题栏与浮条错位）。
    #[test]
    fn painted_cells_land_on_the_absolute_rects() {
        let view = (112.0, 0.0, 900.0, 600.0);
        let rects = layout(Placement::new(Side::Left, view), 3);
        let paths: Vec<PathBuf> = vec![PathBuf::from("a")];
        let slots: Vec<Slot<'_>> = vec![Slot {
            idx: 0,
            rect: rects[0],
            path: paths[0].as_path(),
        }];
        let mut spy = SpyCells::default();
        Strip::new().paint(
            &mut spy,
            Placement::new(Side::Left, view),
            &slots,
            0,
            None,
            |_| None::<Rc<RgbaImage>>,
        );
        assert_eq!(
            spy.framed,
            vec![(rects[0].x, rects[0].y)],
            "当前格描边必须落在 layout 的绝对坐标上（差一条宽 = 少减了一次原点）"
        );
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

    /// ★ m261：序号位置贴在格子**右下角**、给 1px 描边留位；装不下就不画。
    /// 盒子尺寸就是量出来的文字盒本身——**不额外加一圈底衬**（用户明确不要色块）。
    #[test]
    fn badge_sits_in_the_bottom_right_corner() {
        let cell = Rect::new(100, 200, 96, 68);
        let inset = (INSET + BADGE_EDGE) as i32;
        let b = badge_rect(cell, 16.0, 10.0).expect("正常格子该有文字盒");
        assert_eq!(b.x + b.w, cell.x + cell.w - inset, "右边沿内缩到描边外");
        assert_eq!(b.y + b.h, cell.y + cell.h - inset, "底边沿内缩到描边外");
        assert_eq!((b.w, b.h), (16, 10), "文字盒不许再膨胀成底色块");
        // 位数多一档就宽一档：宽度跟着量出来的文字走，不写死。
        let wide = badge_rect(cell, 24.0, 10.0).expect("四位数也放得下");
        assert!(wide.w > b.w, "位数多了标签要更宽");
        assert_eq!(
            wide.x + wide.w,
            cell.x + cell.w - inset,
            "变宽之后仍然贴右下角（往左长，不往右溢出）"
        );
        // 格子被窄窗口压到放不下 ⇒ 不画（画出来白描边会糊出格子）。
        assert!(badge_rect(Rect::new(0, 0, 20, 18), 16.0, 10.0).is_none());
    }

    /// ★ m261 负向验证：描边文字**不许**退回「黑底圆角矩形 + 1px 白边」。
    /// `SpyCells` 数圆角填充与 1dp 描边，一整排格子只该有「1 块底板 + 每格 1 块背板」。
    #[test]
    fn outline_digits_never_paint_a_filled_plate() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let rects = layout(Placement::new(Side::Bottom, view), 3);
        let paths: Vec<PathBuf> = ["a", "b", "c"].iter().map(PathBuf::from).collect();
        let slots: Vec<Slot<'_>> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| Slot {
                idx: i,
                rect: rects[i],
                path: p.as_path(),
            })
            .collect();
        let mut spy = SpyCells::default();
        Strip::new().paint(
            &mut spy,
            Placement::new(Side::Bottom, view),
            &slots,
            0,
            None,
            |_| None::<Rc<RgbaImage>>,
        );
        assert_eq!(
            spy.round_fills,
            1 + slots.len(),
            "多出来的圆角填充就是底色块"
        );
        assert_eq!(spy.badge_strokes, 0, "序号不该再画 1dp 的矩形白边");
        assert_eq!(spy.halos, slots.len() * BADGE_RING.len(), "每格八邻域白字");
        assert_eq!(spy.digits, vec!["1", "2", "3"], "正中的黑字每格一遍");
    }

    /// 放大目标只补不足的那一维：够大就原样返回（不许把用户调大的窗口缩回去）。
    #[test]
    fn grow_to_fit_only_extends_the_short_dimension() {
        // 已经放得下 ⇒ 一个数都不动。
        assert_eq!(
            grow_to_fit(Side::Bottom, 900.0, 800.0),
            (900.0, 800.0),
            "放得下就不该改尺寸"
        );
        assert_eq!(grow_to_fit(Side::Left, 1200.0, 900.0), (1200.0, 900.0));

        // 底条：只长高，宽原样。
        let (w, h) = grow_to_fit(Side::Bottom, 900.0, 100.0);
        assert_eq!(w, 900.0);
        assert_eq!(h, MIN_VIEW_H);
        assert!(fits(Side::Bottom, w, h), "放大后必须判放得下");

        // 左条：只长宽，高原样。
        let (w, h) = grow_to_fit(Side::Left, 300.0, 800.0);
        assert_eq!(w, MIN_VIEW_W);
        assert_eq!(h, 800.0);
        assert!(fits(Side::Left, w, h), "放大后必须判放得下");
    }

    /// 太矮的窗口不放条（`fits` 守着，免得画面只剩一条缝）。
    #[test]
    fn does_not_fit_in_a_short_window() {
        assert!(fits(Side::Bottom, 800.0, 400.0));
        assert!(!fits(Side::Bottom, 800.0, 120.0));
        // 左条扣的是宽：够高但太窄照样不放。
        assert!(fits(Side::Left, 900.0, 700.0));
        assert!(!fits(Side::Left, 300.0, 700.0));
        assert!(!fits(Side::Left, 900.0, 100.0));
    }

    /// 底板必须贴着视口底边、并把所有格子整个装进去——底板一旦画到视口下沿
    /// 之外（把 `view` 底边又往下多推了整条高），条就只剩浮空的格子没有背板。
    #[test]
    fn panel_sits_on_the_view_bottom_and_swallows_the_slots() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let p = panel(Placement::new(Side::Bottom, view));
        assert_eq!(p.y + p.h, 600, "板底边必须贴视口底");
        assert_eq!(p.h, STRIP_H as i32, "板高就是条高");
        for (i, r) in layout(Placement::new(Side::Bottom, view), 5)
            .iter()
            .enumerate()
        {
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

    /// 只记接线关系的画布：当前框描在**哪一格**、每格标了**什么数字**、
    /// 白描边画了几遍、有没有偷偷画回底色块。其余画全丢弃——这些判据问的不是
    /// 「长什么样」，是「有没有走对那条绘制路径」。
    #[derive(Default)]
    struct SpyCells {
        framed: Vec<(i32, i32)>,
        /// 正中的黑字（每个序号一遍）。
        digits: Vec<String>,
        /// 八邻域白字的遍数。
        halos: usize,
        /// 圆角填充次数：1 块底板 + 每格 1 块背板，多一枚就是底色块回来了。
        round_fills: usize,
        /// 1dp 的矩形描边次数（m260 那版标签的白边）。
        badge_strokes: usize,
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
        fn fill_round_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _r: f32, _p: &Paint) {
            self.round_fills += 1;
        }
        fn stroke_round_rect(
            &mut self,
            x: f32,
            y: f32,
            _w: f32,
            _h: f32,
            _r: f32,
            line: f32,
            _p: &Paint,
        ) {
            if (line - 1.0).abs() < 0.01 {
                self.badge_strokes += 1;
            }
            // 只记**当前格**那圈 2dp 描边。m261 起序号是描边文字，不再走这条
            // 绘制调用；留着这条线宽判据是为了让"底色块回来"当场判红。
            if (line - 2.0).abs() < 0.01 {
                self.framed.push((x as i32, y as i32));
            }
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
        fn draw_text(&mut self, t: &str, _r: Rect, c: Color, _a: Align, _ts: &TextStyle) {
            if c == super::tk::BADGE_TEXT {
                self.digits.push(t.to_string());
            } else if c == super::tk::BADGE_HALO {
                self.halos += 1;
            }
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
        let rects = layout(Placement::new(Side::Bottom, view), paths.len());
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
        let mut spy = SpyCells::default();
        Strip::new().paint(
            &mut spy,
            Placement::new(Side::Bottom, view),
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
        // 序号标签（m260）写的是**清单里的序号**（1-based），不是屏上的位置号：
        // 这一屏的序号是 3..7，位置号是 1..5，两者对不上正好把接错线照出来。
        assert_eq!(
            spy.digits,
            vec!["3", "4", "5", "6", "7"],
            "标签要按 idx+1 写；写成屏上位置号，滚动到远处时数字就全错了"
        );
    }
}
