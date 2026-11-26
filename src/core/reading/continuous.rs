//! 连续阅读（m351，CORE-04）：把当前序列铺成一条可连续滚动的长带。
//!
//! 用户口径（2026-11-25）：等尺寸归一——纵向模式全部页**等宽**、横向全部页**等高**，
//! 各页原生尺寸不齐时按统一横截面等比缩放，高度（宽度）随各自纵横比；一路拖动不切页。
//! 双页（CORE-03）已裁决不做，本模块不引入页键抽象：页带项 = 画廊项 + 帧号，
//! 帧寻址走现有 [`crate::core::pixels::Anim`] / `page_index` 通道。
//!
//! 布局是**渐进式**的：解码回包前项的纵横比未知，按 `est_ratio` 占位；回包落地后
//! [`Band::set_natural`] 重排其后各项的偏移，并返回该项的位移量供视口锚定补偿
//! （锚定项在屏幕上的相对位置不动，用户就感知不到"跳变"）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 滚动轴：纵向从上到下，横向从左到右。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// 相邻两项之间沿滚动轴的间隔（逻辑像素）。给一条细缝便于分辨页界，不做成可配置。
pub const GAP_PX: f64 = 2.0;

/// 交叉轴拖动时**至少留在画面里**的带内容（像素）。
///
/// 与单页模式平移的 `PAN_KEEP_VISIBLE`（24px）同值同义：「图边能越过窗口边，
/// 但永远还剩一枚衣角在画面里」——用户永远看得见东西在哪儿，也永远能一把拖回来。
/// 两处若要改必须一起改（单页那条口径见 `core::viewer::zoom`）。
pub const BAND_KEEP_VISIBLE: f64 = 24.0;

/// 页带里的一项。
#[derive(Debug, Clone)]
pub struct BandItem {
    /// 画廊项路径（与 [`crate::core::gallery::Gallery::peek`] 同一身份）。
    pub path: PathBuf,
    /// 帧号（页式文件的第 n 页 / 动图帧；普通图恒 0）。
    pub page: u32,
    /// 解码回包后的自然尺寸；`None` = 尚未落地，按 `est_ratio` 占位。
    pub natural: Option<(u32, u32)>,
    /// 占位纵横比（高/宽）；自然尺寸落地后不再使用。
    est_ratio: f32,
    /// 沿滚动轴的起点（带坐标，逻辑像素）。**布局由 [`Band`] 统一维护**，外部只读。
    pub offset: f64,
}

impl BandItem {
    /// 该项沿滚动轴的长度（纵=显示高，横=显示宽）。等尺寸归一在这里生效：
    /// 横截面统一为 `cross`，长度 = `cross × 自身纵横比`。
    fn extent(&self, axis: Axis, cross: f64) -> f64 {
        let ratio = match self.natural {
            Some((w, h)) if w > 0 => (h as f64) / (w as f64),
            _ => (self.est_ratio.max(0.05)) as f64,
        };
        let along = match axis {
            Axis::Vertical => cross * ratio,
            Axis::Horizontal => cross / ratio,
        };
        along.max(16.0)
    }
}

/// 页带：布局 + 滚动状态。纯数据与纯逻辑，绘制/取图由 viewer 与 canvas 接线。
#[derive(Debug, Clone)]
pub struct Band {
    pub axis: Axis,
    /// 等尺寸基准：纵 = 显示宽（逻辑 px），横 = 显示高；`zoom = 1.0` 时恰好铺满视口横截面。
    pub base: f64,
    /// 整带缩放系数（1.0 = 归一基准）。
    pub zoom: f32,
    pub items: Vec<BandItem>,
    /// 视口起点在带上的位置（`0` = 带首）。
    pub scroll: f64,
    /// 交叉轴偏移（m351 二轮）：放大后带比视口宽/高，这个值决定看带的哪一段
    /// （0 = 贴左/贴顶）。m351 六轮起：`zoom <= 1` 时**不再恒为 0**——横向拖动
    /// 要能真的把带挪走（否则那是个死手势），范围见 [`Band::clamp_cross`]。
    pub cross_off: f64,
    /// 当前项（视口内首入项的下标）。
    pub cur: usize,
}

impl Band {
    /// 由序列建带。`specs` = (路径, 帧号, 已知自然尺寸?)；`est_ratio` 供未知项占位。
    pub fn build(
        axis: Axis,
        base: f64,
        specs: impl IntoIterator<Item = (PathBuf, u32, Option<(u32, u32)>)>,
        est_ratio: f32,
    ) -> Band {
        let mut band = Band {
            axis,
            base,
            zoom: 1.0,
            items: Vec::new(),
            scroll: 0.0,
            cross_off: 0.0,
            cur: 0,
        };
        for (path, page, natural) in specs {
            band.items.push(BandItem {
                path,
                page,
                natural,
                est_ratio,
                offset: 0.0,
            });
        }
        band.relayout_from(0);
        band
    }

    /// 横截面尺寸（纵=宽 / 横=高），随整带缩放。
    pub fn cross(&self) -> f64 {
        (self.base * self.zoom as f64).max(16.0)
    }

    /// 带总长（沿滚动轴）。
    pub fn total(&self) -> f64 {
        self.items
            .last()
            .map(|last| {
                let cross = self.cross();
                last.offset + last.extent(self.axis, cross)
            })
            .unwrap_or(0.0)
    }

    /// 从 `from` 项起重排偏移（前缀和 + 间隙）。`set_natural` / `build` 之后调用。
    pub fn relayout_from(&mut self, from: usize) {
        let cross = self.cross();
        // from = 0 时没有前项，带从 0 起（checked_sub，不能用 saturating_sub——
        // 那会把首项自己当成"前项"，整条带双重计数）。
        let mut pos = from
            .checked_sub(1)
            .and_then(|i| self.items.get(i))
            .map(|prev| prev.offset + prev.extent(self.axis, cross) + GAP_PX)
            .unwrap_or(0.0);
        for it in self.items.iter_mut().skip(from) {
            it.offset = pos;
            pos += it.extent(self.axis, cross) + GAP_PX;
        }
    }

    /// 回包落地：填自然尺寸并重排。返回该项**沿轴长度的变化量**（新 − 旧，可负）。
    ///
    /// 调用方用它做视口锚定补偿：锚定项（视口内首入项）在 `idx` **之后**（含
    /// `idx + 1`）时 `scroll += delta`，锚定项在带里的相对位置保持不动；
    /// 锚定项就是 `idx` 或在它之前则无需补偿——它前面的布局一字未动。
    pub fn set_natural(&mut self, idx: usize, natural: (u32, u32)) -> f64 {
        let cross = self.cross();
        let (old_extent, has) = match self.items.get(idx) {
            Some(it) => (it.extent(self.axis, cross), true),
            None => return 0.0,
        };
        if !has {
            return 0.0;
        }
        self.items[idx].natural = Some(natural);
        let new_extent = self.items[idx].extent(self.axis, cross);
        self.relayout_from(idx);
        new_extent - old_extent
    }

    /// 视口起点所在的"当前项"：第一个**末端越过视口起点**的项（含部分可见）。
    pub fn item_at(&self, scroll: f64) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        let cross = self.cross();
        match self
            .items
            .partition_point(|it| it.offset + it.extent(self.axis, cross) <= scroll)
        {
            i if i >= self.items.len() => self.items.len() - 1,
            i => i,
        }
    }

    /// 当前项判据（m351 二轮用户口径）：**最靠近视口中心的项**。滚动/缩放后
    /// 计数跟中心走，而不是跟视口首缘走——首缘判据在放大后一项占几屏时
    /// 计数会长期冻结在顶上那一张。
    pub fn item_at_center(&self, scroll: f64, viewport: f64) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        let cross = self.cross();
        let center = scroll + viewport / 2.0;
        // 找中心点命中的项；中心落在 gap 上时取前一项（阅读语义：刚翻过去的页）。
        match self
            .items
            .partition_point(|it| it.offset + it.extent(self.axis, cross) <= center)
        {
            0 => 0,
            i if i >= self.items.len() => self.items.len() - 1,
            i => i,
        }
    }

    /// 交叉轴偏移钳位（m351 八轮定稿）。
    ///
    /// 范围 `[-越界, 内容余量 + 越界]`：
    /// - **内容可达**——带比视口宽/高（放大过）时，交叉轴能在带内平移看
    ///   被裁掉的那一侧，范围 `0 ..= cross − viewport_cross`；
    /// - **越界余量**——与单页 pan 同款手感（`BAND_KEEP_VISIBLE`，24px 衣角）：
    ///   图边可以越过窗口边，画面里永远剩一枚衣角。zoom=1 满宽/满高时也能
    ///   横向挪（用户裁定「本来可以拖动」，六轮口径保留）。
    ///
    /// ★ 速度手感（m351 九轮定稿）：**全程 1:1 绝对基准拖动**——内容侧与
    ///   越界段都跟手，按住即冻结、松手停原地、再按从当前位置继续（普通看图
    ///   器手感）。八轮的越界阻尼被用户否决（松手再按会「重新校正」弹跳）。
    ///
    /// `keep` = 越界拖到头时至少留在画面里的带内容（像素）。
    pub fn clamp_cross(&mut self, viewport_cross: f64, keep: f64) {
        let cross = self.cross();
        if viewport_cross <= 0.0 || cross <= 0.0 {
            self.cross_off = 0.0;
            return;
        }
        let keep = keep.max(0.0).min(viewport_cross);
        let overs = (viewport_cross - keep).max(0.0);
        let inner = (cross - viewport_cross).max(0.0);
        self.cross_off = self.cross_off.clamp(-overs, inner + overs);
    }

    /// 第 `idx` 项的带首位置（越界时钳到边界）。
    pub fn offset_of(&self, idx: usize) -> f64 {
        match self.items.get(idx) {
            Some(it) => it.offset,
            None => self.total(),
        }
    }

    /// 把 `scroll` 钳进带内（留最后一项可见）。
    pub fn clamp_scroll(&mut self, viewport: f64) {
        let max = (self.total() - viewport).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// 当前项步进：视口跳到相邻项（`←→` / `↑↓` 的连续模式语义）。
    /// m351 二轮：滚动位置对准目标项**中心**——中心判据（`item_at_center`）在
    /// 落定后稳定返回目标项，计数与画面严格同步；对齐带首会在放大时把目标顶
    /// 出视口中心、判据又把它拉回去（用户实测「画面不变计数却变了」的成因）。
    /// 返回是否移动。
    pub fn step_item(&mut self, delta: isize, viewport: f64) -> bool {
        let next = self.cur as isize + delta;
        if next < 0 || next as usize >= self.items.len() {
            return false;
        }
        self.cur = next as usize;
        let cross = self.cross();
        let it = &self.items[self.cur];
        let mid = it.offset + it.extent(self.axis, cross) / 2.0;
        self.scroll = (mid - viewport / 2.0).max(0.0);
        self.clamp_scroll(viewport);
        true
    }
}

/// 驻留池：已解码页带位图，按字节量 LRU。
///
/// 与后台在飞预算（`core::decode::BG_INFLIGHT_BYTES`）是两道独立的闸：这里管
/// **已落地**的位图驻留，那边管**在途**的请求。超限逐出最近最少使用的整项
/// （不做局部裁剪——半张页在带里没有意义）。
pub struct BandCache {
    map: HashMap<(PathBuf, u32), CachedImg>,
    bytes: u64,
    clock: u64,
}

/// 驻留上限：与后台在飞预算同量级。
pub const BAND_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// 池里一张已解码页。
pub struct CachedImg {
    /// 预乘 sRGBA8（与单页 `Loaded::base_rgba` 同一像素口径；Arc 共享避免整幅拷贝）。
    pub rgba: std::sync::Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    last_use: u64,
}

impl BandCache {
    pub fn new() -> BandCache {
        BandCache {
            map: HashMap::new(),
            bytes: 0,
            clock: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// 是否已驻留某项（**不续命**——探测与续命分开，窗口刷新每帧跑）。
    pub fn contains(&self, path: &Path, page: u32) -> bool {
        self.map.contains_key(&(path.to_path_buf(), page))
    }

    /// 取一份拷贝视图（命中即续命）。借用冲突的折衷：调用方拿到的是快照引用计数外的
    /// 克隆——位图克隆一次的代价远小于重复解码。
    pub fn get(&mut self, path: &Path, page: u32) -> Option<&CachedImg> {
        let clock = self.tick();
        let img = self.map.get_mut(&(path.to_path_buf(), page))?;
        img.last_use = clock;
        Some(img)
    }

    /// 插入；超限时按最旧逐出（可能包含刚插入项——调用方无须特判）。
    pub fn insert(
        &mut self,
        path: PathBuf,
        page: u32,
        rgba: std::sync::Arc<Vec<u8>>,
        w: u32,
        h: u32,
    ) {
        let cost = (w as u64) * (h as u64) * 4;
        let key = (path, page);
        if let Some(old) = self.map.remove(&key) {
            self.bytes -= (old.w as u64) * (old.h as u64) * 4;
        }
        let clock = self.tick();
        self.map.insert(
            key,
            CachedImg {
                rgba,
                w,
                h,
                last_use: clock,
            },
        );
        self.bytes += cost;
        while self.bytes > BAND_CACHE_BYTES && self.map.len() > 1 {
            let Some(victim) = self
                .map
                .iter()
                .min_by_key(|(_, img)| img.last_use)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(old) = self.map.remove(&victim) {
                self.bytes -= (old.w as u64) * (old.h as u64) * 4;
            }
        }
    }

    /// 视口窗口之外的项按距离裁剪（比纯 LRU 更贴合页带语义）。
    pub fn evict_outside(&mut self, keep: &[(PathBuf, u32)]) {
        let clock = self.tick();
        let keep_keys: Vec<(PathBuf, u32)> = keep.to_vec();
        self.map.retain(|k, img| {
            if keep_keys.iter().any(|x| x == k) {
                img.last_use = clock;
                true
            } else {
                self.bytes -= (img.w as u64) * (img.h as u64) * 4;
                false
            }
        });
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

impl Default for BandCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: &str) -> PathBuf {
        PathBuf::from(n)
    }

    #[test]
    fn uniform_cross_widths_match() {
        // 等宽归一：不同纵横比的项显示宽一致。
        let mut b = Band::build(
            Axis::Vertical,
            800.0,
            [
                (p("a"), 0, Some((400, 300))),
                (p("b"), 0, Some((2000, 1000))),
                (p("c"), 0, None),
            ],
            1.4,
        );
        let cross = b.cross();
        let hs: Vec<f64> = b.items.iter().map(|it| it.extent(b.axis, cross)).collect();
        assert!((hs[0] - 600.0).abs() < 1e-3); // 800×300/400
        assert!((hs[1] - 400.0).abs() < 1e-3); // 800×1000/2000
        assert!((hs[2] - 1120.0).abs() < 1e-3); // est 1.4
        assert_eq!(b.items[1].offset, hs[0] + GAP_PX);
        assert_eq!(b.items[2].offset, hs[0] + GAP_PX + hs[1] + GAP_PX);
        assert_eq!(b.item_at(0.0), 0);
        assert_eq!(b.item_at(hs[0] + GAP_PX + 1.0), 1);
        b.step_item(1, 800.0);
        assert_eq!(b.cur, 1);
        // 步进后 scroll 对准目标项**中心**（中心判据稳定返回该页——二轮口径）。
        let mid = b.items[1].offset + b.items[1].extent(b.axis, b.cross()) / 2.0;
        assert!((b.scroll - (mid - 400.0)).abs() < 1e-3, "scroll 对准项中心");
        b.clamp_scroll(800.0);
    }

    #[test]
    fn horizontal_uses_uniform_height() {
        // 等高归一：横轴 display_h 一致，长度 = cross × nw/nh。
        let b = Band::build(
            Axis::Horizontal,
            600.0,
            [
                (p("a"), 0, Some((300, 600))),
                (p("b"), 0, Some((1000, 2500))),
            ],
            1.4,
        );
        let cross = b.cross();
        assert!((b.items[0].extent(b.axis, cross) - 300.0).abs() < 1e-3);
        assert!((b.items[1].extent(b.axis, cross) - 240.0).abs() < 1e-3);
    }

    #[test]
    fn natural_late_arrival_anchors_scroll() {
        // 渐进布局：est 占位 → 回包修正，锚定位移 = 该项起点位移。
        let mut b = Band::build(
            Axis::Vertical,
            800.0,
            [(p("a"), 0, None), (p("b"), 0, Some((1000, 4000)))],
            1.4,
        );
        let est_a = b.items[0].extent(b.axis, b.cross());
        assert_eq!(b.items[1].offset, est_a + GAP_PX);
        // 锚定 items[1]：视口正对它的带首（此时布局还是 est 占位）。
        b.scroll = b.items[1].offset;
        let delta = b.set_natural(0, (800, 3200)); // 真实比 4.0 > est 1.4 → 变长
        assert!(
            (delta - (3200.0 - est_a)).abs() < 1e-3,
            "delta = 长度变化量"
        );
        assert!(delta > 0.0);
        assert!((b.items[1].offset - (b.items[0].extent(b.axis, b.cross()) + GAP_PX)).abs() < 1e-3);
        // 锚定项在 idx 之后 → scroll += delta 补偿，锚定项仍正对视口首。
        b.scroll += delta;
        assert!((b.items[1].offset - b.scroll).abs() < 1e-3);
    }

    #[test]
    fn clamp_and_item_at_edges() {
        let mut b = Band::build(
            Axis::Vertical,
            800.0,
            [(p("a"), 0, Some((800, 600))), (p("b"), 0, Some((800, 600)))],
            1.4,
        );
        b.scroll = 1e9;
        b.clamp_scroll(400.0);
        assert!(b.scroll <= (b.total() - 400.0).max(0.0) + 1e-9);
        assert_eq!(b.item_at(1e9), b.items.len() - 1);
        let mut empty = Band::build(Axis::Vertical, 800.0, [], 1.4);
        assert_eq!(empty.item_at(0.0), 0);
        assert!(!empty.step_item(1, 800.0));
    }

    /// m351 八轮→九轮定稿：交叉轴钳位——可自由拖（zoom=1 满宽也能拖出
    /// `[-越界, +越界]`；放大后 `[-24, 内容余量+越界]`）。拖动全程 1:1
    /// 绝对基准（阻尼已删，见 `Viewer::band_drag_by` 九轮注释）。
    #[test]
    fn cross_axis_clamp_allows_free_pan() {
        let mut b = Band::build(Axis::Vertical, 800.0, [(p("a"), 0, Some((800, 600)))], 1.4);
        let keep = BAND_KEEP_VISIBLE;

        // ① zoom=1：带恰满视口宽——钳位范围 [-776, +776]（满宽也能横向挪）。
        b.cross_off = 5000.0;
        b.clamp_cross(800.0, keep);
        assert!(
            (b.cross_off - 776.0).abs() < 1e-9,
            "zoom=1 钳位上限 = 越界 776（实际 {}）",
            b.cross_off
        );
        b.cross_off = -5000.0;
        b.clamp_cross(800.0, keep);
        assert!(
            (b.cross_off + 776.0).abs() < 1e-9,
            "zoom=1 钳位下限 = −776（实际 {}）",
            b.cross_off
        );

        // ② 放大 2×：cross=1600，内容平移范围 800，钳位 = [−24, 1576]。
        b.zoom = 2.0;
        b.relayout_from(0);
        b.cross_off = 5000.0;
        b.clamp_cross(800.0, keep);
        assert!(
            (b.cross_off - (800.0 + 776.0)).abs() < 1e-9,
            "放大后钳位上限 = 内容余量(800) + 越界(776，剩24衣角) = 1576（实际 {}）",
            b.cross_off
        );
    }

    #[test]
    fn cache_evicts_by_bytes() {
        let mut c = BandCache::new();
        // 8 MiB/张，40 张必然超 256 MiB。
        for i in 0..40 {
            let n = (8 * 1024 * 1024) as usize; // 8 MiB
            c.insert(
                p(&format!("f{i}")),
                0,
                std::sync::Arc::new(vec![0u8; n]),
                1024,
                2048,
            );
        }
        assert!(c.bytes() <= BAND_CACHE_BYTES);
        assert!(c.len() < 40);
        // 命中续命后，旧项先被逐出。
        assert!(c.get(&p("f8"), 0).is_some());
        c.insert(
            p("x"),
            0,
            std::sync::Arc::new(vec![0u8; 8 * 1024 * 1024]),
            1024,
            2048,
        );
        assert!(c.get(&p("f8"), 0).is_some(), "刚续命的项不应被逐出");
    }

    #[test]
    fn evict_outside_keeps_window() {
        let mut c = BandCache::new();
        for i in 0..4 {
            c.insert(
                p(&format!("f{i}")),
                0,
                std::sync::Arc::new(vec![0u8; 16]),
                2,
                2,
            );
        }
        c.evict_outside(&[(p("f1"), 0), (p("f2"), 0)]);
        assert!(c.get(&p("f1"), 0).is_some());
        assert!(c.get(&p("f2"), 0).is_some());
        assert!(c.get(&p("f0"), 0).is_none());
        assert!(c.get(&p("f3"), 0).is_none());
    }
}
