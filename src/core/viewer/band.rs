//! 连续阅读（m351）的 viewer 接线：模式进出、页带取图窗口、回包落地与锚定补偿。
//!
//! 与单页管线的关系（**最小侵入**的关键）：
//! - `loaded` 在连续模式下继续只装 `band.cur` 那一项（走正常 Main 请求 + `install`），
//!   于是状态格 `(12/340)`、胶片条、信息面板、跳转窗这些依赖单页状态的东西**全部免费工作**；
//! - 其余可见窗口项走 `ReqKind::Prefetch`（m349 的独立车道 + 在飞预算闸 + 去重），
//!   回包进 [`BandCache`] 驻留池，不进单页预读缓存；
//! - 画布绘制时 band 模式整带画（canvas 从驻留池取），单页绘制分支不跑。
//!
//! 帧寻址：带项 = (画廊项, 帧号)，普通图恒 0。页式文件（PDF）第一版先占一项；
//! 已知页数（`meta_cache` / 回包 `anim.count`）时展开成多项的动态拼接在落地确认后做。

use std::path::PathBuf;

use super::Viewer;
use crate::core::decode::{Done, ReqKind};
use crate::core::reading::continuous::{
    Axis, Band, BandCache, BAND_CACHE_BYTES, BAND_KEEP_VISIBLE,
};

/// 可见窗口前后各多取几项（跨屏缓冲，滚动时不至于露白）。
pub(crate) const BAND_CONTEXT: usize = 2;

impl Viewer {
    /// 进入连续阅读模式。以当前画廊序列建带，视口对准当前项；基准横截面取
    /// `cross_px`（画布 work 区的宽/高，调用方从布局拿）。
    pub(crate) fn enter_band(&mut self, axis: Axis, cross_px: f64) {
        let Some(g) = self.gallery.as_ref() else {
            return;
        };
        // 记建带时的排序代次：序列变更检测（band_sync_current）用它兜底——
        // 排序后 len 与首项都可能侥幸不变，代次不会骗人。
        self.band_sort_epoch = self.sort_epoch;
        // 占位纵横比：当前这张的真实比例最有代表性；没有就 1.4（比 A4 略长）。
        let est = self
            .loaded
            .as_ref()
            .and_then(|l| {
                let (w, h) = l.natural;
                (w > 0).then(|| h as f32 / w as f32)
            })
            .filter(|r| *r > 0.05)
            .unwrap_or(1.4);
        let cur = g.position().saturating_sub(1);
        let n = g.len();
        let specs = (0..n)
            .filter_map(|i| g.peek(i).cloned())
            .map(|path| (path, 0u32, None))
            .collect::<Vec<_>>();
        let mut band = Band::build(axis, cross_px.max(64.0), specs, est);
        band.cur = cur.min(band.items.len().saturating_sub(1));
        band.scroll = band.offset_of(band.cur);
        self.band = Some(band);
        self.band_cache = BandCache::new();
        self.band_inflight.clear();
        // 当前项的已解码位图直接进驻留池（省一次重复解码）。
        let seeded = self
            .loaded
            .as_ref()
            .map(|l| ((l.path.clone(), 0u32), l.base_rgba.clone(), l.decoded));
        if let Some(((path, page), rgba, (w, h))) = seeded {
            let cost = (w as u64) * (h as u64) * 4;
            if cost <= BAND_CACHE_BYTES {
                self.band_cache.insert(path, page, rgba, w, h);
            }
        }
    }

    /// 退出连续阅读模式：清带与驻留（在途回包落地时因 inflight 已清而被丢弃）。
    pub(crate) fn leave_band(&mut self) {
        self.band = None;
        self.band_cache = BandCache::new();
        self.band_inflight.clear();
    }

    /// 视口横截面变化（窗口 resize）：重设归一基准并整带重排。
    /// 锚定当前项：`scroll` 对准 `band.cur` 的带首（resize 是显式几何操作，
    /// 不做"像素位置保持"——按项锚定符合直觉且实现简单）。
    ///
    /// ★ m351 六轮：视口宽变了 ⇒ 交叉轴的钳位范围也变了（越界余量按视口宽算），
    ///   旧 `cross_off` 会停在越界的位置上（带歪在一边）。这里重夹一次。
    pub(crate) fn band_resize(&mut self, cross_px: f64) {
        let Some(band) = self.band.as_mut() else {
            return;
        };
        if (band.base - cross_px).abs() < 0.5 {
            return;
        }
        band.base = cross_px.max(64.0);
        band.relayout_from(0);
        band.scroll = band.offset_of(band.cur);
        band.clamp_cross(cross_px.max(64.0), BAND_KEEP_VISIBLE);
    }

    /// 页带窗口刷新：可见项 ± [`BAND_CONTEXT`] 里没有驻留的，按显示尺寸发
    /// Prefetch 请求（独立车道）。`viewport_along` = 沿滚动轴的视口长度。
    /// 便宜调用：窗口没变化时零开销。
    pub(crate) fn band_refresh_window(&mut self, viewport_along: f64) {
        // 第一段借用：只算窗口与裁剪，不碰 send_decode（它要整个 &self）。
        let (_cur, cross, axis, wanted) = {
            let Some(band) = self.band.as_mut() else {
                return;
            };
            band.clamp_scroll(viewport_along);
            band.cur = band.item_at(band.scroll);
            let cur = band.cur;
            let cross = band.cross();
            let lo = cur.saturating_sub(BAND_CONTEXT);
            let hi = (cur + BAND_CONTEXT + 1).min(band.items.len());
            let wanted: Vec<(PathBuf, u32)> = (lo..hi)
                .map(|idx| (band.items[idx].path.clone(), band.items[idx].page))
                .collect();
            (cur, cross, band.axis, wanted)
        };
        // 窗口外的驻留裁掉；窗口内已驻留的跳过，缺的发请求。
        let keys: Vec<(PathBuf, u32)> = wanted.clone();
        self.band_cache.evict_outside(&keys);
        for (path, page) in wanted {
            if self.band_cache.contains(&path, page) {
                continue;
            }
            if self
                .band_inflight
                .values()
                .any(|(p, pg)| p == &path && pg == &page)
            {
                continue;
            }
            // 按"等尺寸归一后的显示尺寸"要图（REQ_ALLOW_SCALED 让解码器侧降采样，
            // 驻留池与带宽一起省）；长度未知按 1.4 占位估，自然尺寸由回包回填布局。
            let (w, h) = match axis {
                Axis::Vertical => (cross as u32, (cross * 1.4) as u32),
                Axis::Horizontal => ((cross * 1.4) as u32, cross as u32),
            };
            let seq = self.send_decode(&path, w, h, page, ReqKind::Prefetch, true);
            self.band_inflight.insert(seq, (path, page));
        }
    }

    /// 回包落地：band 在途请求按 seq 认领，驻留 + 布局修正 + 锚定补偿。
    /// 返回 `true` = 这笔回包已被 band 消费（apply 不再走单页过期过滤）。
    pub(crate) fn band_on_done(&mut self, done: &Done) -> bool {
        if self.band.is_none() {
            return false;
        }
        let Some((path, page)) = self.band_inflight.remove(&done.seq) else {
            return false;
        };
        let Ok(d) = &done.out else {
            // 解码失败：占位仍留着（est 高度）；inflight 已摘，下一轮窗口刷新会
            // 重试——后台去重键已随 note_done 销账，天然幂等。
            return true;
        };
        let (w, h) = (d.bitmap.width, d.bitmap.height);
        let rgba = std::sync::Arc::new(d.bitmap.rgba.clone());
        // 先在带里定位（借 path），再 move 进驻留池。
        let idx = self.band.as_ref().and_then(|band| {
            band.items
                .iter()
                .position(|it| it.path == path && it.page == page)
        });
        self.band_cache.insert(path, page, rgba, w, h);
        let Some(band) = self.band.as_mut() else {
            return true;
        };
        if let Some(idx) = idx {
            let delta = band.set_natural(idx, d.natural);
            // 锚定补偿：修正项在当前项之前 → 当前项及其后整体平移 delta；
            // 修正项就是当前项或在它之后 → 当前项前面的布局一字未动，不补。
            if idx < band.cur {
                band.scroll += delta;
            }
        }
        true
    }

    /// 沿轴滚动 `delta_px`（滚轮/拖动的统一入口），钳位后重算当前项。
    /// `viewport_along` = 沿滚动轴的视口长度（canvas 缓存的上次 paint 值）。
    pub(crate) fn band_scroll_by(&mut self, delta_px: f64, viewport_along: f64) {
        let Some(band) = self.band.as_mut() else {
            return;
        };
        band.scroll += delta_px;
        band.clamp_scroll(viewport_along);
        band.cur = band.item_at_center(band.scroll, viewport_along);
    }

    /// 拖带跟手（m351 二轮双轴）：`along_abs` = 起点 scroll − 指针沿轴位移；
    /// `d_cross` = 指针**交叉轴**位移（纵向带用 dx、横向带用 dy——放大后带
    /// 比视口宽/高，两个方向都要能拖）。双轴钳位后按中心判据重算当前项。
    ///
    /// ★ m351 六轮：交叉轴的钳位范围改了（见 [`Band::clamp_cross`]）——旧口径
    ///   在 zoom=1 时把 `cross_off` 钉死 0，横向拖动整个是死手势（用户实报
    ///   「纵向模式左右拖不动」）。现在横向拖动照样把带挪走。
    pub(crate) fn band_drag_by(
        &mut self,
        along_abs: f64,
        d_cross: f64,
        viewport_along: f64,
        viewport_cross: f64,
    ) {
        let Some(band) = self.band.as_mut() else {
            return;
        };
        band.scroll = along_abs;
        // 内容跟手：指针拖右（d_cross > 0）⇒ 内容右移 ⇒ 带相对视口左移量减小。
        let target = band.cross_off - d_cross;
        // 越界段橡皮筋阻尼（m351 八轮）：内容侧（0..=inner）1:1 跟手；
        // 拖出内容的越界部分 ×0.3——满宽时轻拖不再把图片大半滑出屏幕
        // （六轮实测「拖动速度太快」的真身）。BAND_CROSS_DAMPING = 0.3。
        let inner = (band.cross() - viewport_cross).max(0.0);
        let clamped = target.clamp(0.0, inner);
        let over = target - clamped;
        let damped = over * crate::core::reading::continuous::BAND_CROSS_DAMPING;
        band.cross_off = clamped + damped;
        band.clamp_scroll(viewport_along);
        band.clamp_cross(viewport_cross, BAND_KEEP_VISIBLE);
        band.cur = band.item_at_center(band.scroll, viewport_along);
    }

    /// 整带缩放：`zoom ×= factor`，双轴锚定——沿轴按**视口中心**（长带比例
    /// 近似），交叉轴按**指针位置**（m351 二轮：修「缩放后靠边、无法拖动位置」
    /// ——放大后带比视口宽/高，指针下的内容保持不动，`cross_off` 让两侧都可达）。
    /// `pointer_cross` = 指针在视口横截面上的坐标；None = 按视口中心。
    pub(crate) fn band_zoom_by(
        &mut self,
        factor: f32,
        viewport_along: f64,
        viewport_cross: f64,
        pointer_cross: Option<f64>,
    ) {
        let Some(band) = self.band.as_mut() else {
            return;
        };
        let old_zoom = band.zoom;
        band.zoom = (band.zoom * factor).clamp(0.25, 4.0);
        if (band.zoom - old_zoom).abs() < 1e-6 {
            return;
        }
        let scale = band.zoom / old_zoom;
        // 沿轴：中心锚定。
        let center = band.scroll + viewport_along / 2.0;
        band.relayout_from(0);
        band.scroll = (center * scale as f64 - viewport_along / 2.0).max(0.0);
        band.clamp_scroll(viewport_along);
        // 交叉轴：指针锚定（zoom 回 1 以下时带完全可见，偏移自然归到 0 附近）。
        let px = pointer_cross.unwrap_or(viewport_cross / 2.0);
        let cross_content = band.cross_off + px;
        band.cross_off = cross_content * scale as f64 - px;
        band.clamp_cross(viewport_cross, BAND_KEEP_VISIBLE);
        band.cur = band.item_at_center(band.scroll, viewport_along);
    }

    /// 当前项步进（`←→` / `↑↓` 的连续模式语义）：视口跳到相邻项带首。
    /// 返回是否移动（没动就让调用方回落到单页翻页）。
    pub(crate) fn band_step_item(&mut self, delta: isize, viewport_along: f64) -> bool {
        let Some(band) = self.band.as_mut() else {
            return false;
        };
        band.step_item(delta, viewport_along)
    }

    /// 当前项（`band.cur`）变化后的对齐：**先把清单下标推到 `band.cur`**（m351
    /// 二轮修：原先只看 loaded 路径，`navigate()` 永远拿旧下标——计数冻结、
    /// 每帧重发同一张的解码），再把单页管线切过去。同时检测序列变更：
    /// 排序 / 目录增删后带还是旧序列 ⇒ 自动退出连续模式（用户重开即新序列）。
    pub(crate) fn band_sync_current(&mut self) {
        let (cur, cur_path, stale) = {
            let Some(band) = self.band.as_ref() else {
                return;
            };
            let cur_path = match band.items.get(band.cur) {
                Some(it) => it.path.clone(),
                None => return,
            };
            let stale = match self.gallery.as_ref() {
                Some(g) => {
                    // 排序代次（m275）变了 ⇒ 清单顺序全变，带必过期——比"首项
                    // 相同"可靠（排序后第一项可能侥幸不变）。代次在 Viewer 自身
                    // （`set_sort` 时 +1），与 Gallery 无关。
                    self.sort_epoch != self.band_sort_epoch
                        || g.len() != band.items.len()
                        || band
                            .items
                            .first()
                            .zip(g.peek(0))
                            .map(|(it, p0)| !crate::core::reading::same_path(&it.path, p0))
                            .unwrap_or(true)
                }
                None => false,
            };
            (band.cur, cur_path, stale)
        };
        if stale {
            // 排序变了 / 目录内容变了：带序列已过期，自动关（第一版不支持热刷新）。
            self.leave_band();
            return;
        }
        // 清单下标先对齐（band.cur 是唯一真相），否则 navigate 用的还是旧下标。
        if let Some(g) = self.gallery.as_mut() {
            if g.position().saturating_sub(1) != cur {
                g.jump_to(cur);
            }
        }
        let moved = match self.loaded.as_ref() {
            Some(l) => !crate::core::reading::same_path(&l.path, &cur_path),
            None => true,
        };
        if moved {
            self.navigate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 窗口缓冲约定：前后各 2 项。跨屏滚动时不露白的下限。
    #[test]
    fn band_context_window_is_symmetric() {
        assert_eq!(BAND_CONTEXT, 2);
        let w: HashMap<u64, (PathBuf, u32)> = HashMap::new();
        assert!(w.is_empty());
    }
}
