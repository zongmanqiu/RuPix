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
use crate::core::reading::continuous::{Axis, Band, BandCache, BAND_CACHE_BYTES};

/// 可见窗口前后各多取几项（跨屏缓冲，滚动时不至于露白）。
#[allow(dead_code)] // m351 Phase C 接线后移除
pub(super) const BAND_CONTEXT: usize = 2;

impl Viewer {
    // ★ m351 Phase C（canvas 绘制/菜单/键鼠接线）落地前五个方法暂无调用点；
  // 接线后整段标注移除。
    /// 进入连续阅读模式。以当前画廊序列建带，视口对准当前项；基准横截面取
    /// `cross_px`（画布 work 区的宽/高，调用方从布局拿）。
    #[allow(dead_code)]
    pub(super) fn enter_band(&mut self, axis: Axis, cross_px: f64) {
        let Some(g) = self.gallery.as_ref() else {
            return;
        };
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
    #[allow(dead_code)]
    pub(super) fn leave_band(&mut self) {
        self.band = None;
        self.band_cache = BandCache::new();
        self.band_inflight.clear();
    }

    /// 页带窗口刷新：可见项 ± [`BAND_CONTEXT`] 里没有驻留的，按显示尺寸发
    /// Prefetch 请求（独立车道）。`viewport_along` = 沿滚动轴的视口长度。
    /// 便宜调用：窗口没变化时零开销。
    #[allow(dead_code)]
    pub(super) fn band_refresh_window(&mut self, viewport_along: f64) {
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
    #[allow(dead_code)]
    pub(super) fn band_on_done(&mut self, done: &Done) -> bool {
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

    /// 当前项（`band.cur`）变化后的对齐：把单页管线切到新当前项（复用现有
    /// navigate），不复位带——滚动位置就是用户拖到的位置。由 canvas 在滚动
    /// 分派里调用：`cur` 与 loaded 对不上才动。
    #[allow(dead_code)]
    pub(super) fn band_sync_current(&mut self) {
        let cur_path = {
            let Some(band) = self.band.as_ref() else {
                return;
            };
            match band.items.get(band.cur) {
                Some(it) => it.path.clone(),
                None => return,
            }
        };
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
