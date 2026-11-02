//! 绘制侧查询：`on_viewport` 后的目标框、上屏位图、显示↔图像矩形映射。
//!
//! 这里只回答「画在哪、画什么」，不改状态（状态由 `zoom` / `schedule` 推进）。
use super::transform::*;
use super::*;

impl Viewer {
    // ── 绘制 ────────────────────────────────────────────────────

    /// 由 `paint` 调用：记下视口矩形与 DPI，并在尺寸变化时排一次重新解码。
    pub fn on_viewport(&mut self, x: f32, y: f32, w: f32, h: f32, dpi: f32) {
        let changed = (self.view.2 - w).abs() > 0.5 || (self.view.3 - h).abs() > 0.5;
        self.view = (x, y, w, h);
        self.dpi = dpi;
        if changed {
            self.clamp_pan();
            if self.loaded.is_none() {
                // 首帧：视口刚就绪，立刻按真实尺寸发解码（不走 120ms 防抖——
                // 开图的第一感就是"快"，慢 100ms 都看得出来）。
                self.request_decode();
            } else {
                self.mark_need();
            }
        }
    }

    /// 图片在视口中的目标矩形（逻辑坐标）。`None` = 当前没有可绘制的图。
    pub fn dst_rect(&self, content: RectI) -> Option<RectI> {
        let l = self.loaded.as_ref()?;
        let (nw, nh) = self.eff_dims_of(l);
        let s = self.scale();
        let (dw, dh) = (nw * s, nh * s);
        let cx = content.x as f32 + content.w as f32 / 2.0 + self.pan.0;
        let cy = content.y as f32 + content.h as f32 / 2.0 + self.pan.1;
        let x = (cx - dw / 2.0).round();
        let y = (cy - dh / 2.0).round();
        let w = dw.round().max(1.0);
        let h = dh.round().max(1.0);
        Some(RectI::new(x as i32, y as i32, w as i32, h as i32))
    }

    pub fn image(&self) -> Option<&RgbaImage> {
        self.loaded.as_ref().map(|l| &l.image)
    }

    /// 当前位图（整图或 SVG 可视区域切片）在视口中的目标矩形（逻辑坐标）。
    ///
    /// 整图位图与 [`Self::dst_rect`] 相同；切片则把"图像坐标 → 显示坐标"正向映射
    /// 进 dst。绘制必须用这个（而不是 dst_rect），否则 SVG 切片会被错误地拉伸成整图。
    pub fn image_dst(&self, content: RectI) -> Option<RectI> {
        let l = self.loaded.as_ref()?;
        let Some(r) = l.region.as_ref() else {
            return self.dst_rect(content);
        };
        let base = self.dst_rect(content)?;
        let (nw, nh) = (l.natural.0 as f32, l.natural.1 as f32);
        // 切片的图像尺寸 = 位图像素 / 密度（解码端降密度时覆盖范围按比例变大，一致）。
        let w_img = l.decoded.0 as f32 / r.density;
        let h_img = l.decoded.1 as f32 / r.density;
        let (dx0, dy0, dx1, dy1) =
            self.map_rect_to_display(r.origin.0, r.origin.1, w_img, h_img, nw, nh);
        let x = (base.x as f32 + dx0 * base.w as f32).round();
        let y = (base.y as f32 + dy0 * base.h as f32).round();
        let w = ((dx1 - dx0) * base.w as f32).round().max(1.0);
        let h = ((dy1 - dy0) * base.h as f32).round().max(1.0);
        Some(RectI::new(x as i32, y as i32, w as i32, h as i32))
    }

    /// 图像坐标矩形 → 显示坐标归一化矩形（先旋转后镜像，与上屏位图的生成顺序一致）。
    pub(super) fn map_rect_to_display(
        &self,
        ox: f32,
        oy: f32,
        w: f32,
        h: f32,
        nw: f32,
        nh: f32,
    ) -> (f32, f32, f32, f32) {
        let (x0, y0, x1, y1) = (ox / nw, oy / nh, (ox + w) / nw, (oy + h) / nh);
        rect_map_display(self.rot, self.flip, x0, y0, x1, y1)
    }

    /// 当前应渲染的 SVG 可视区域（图像坐标 + 设备密度）。
    ///
    /// 视口 ∩ 整图 dst 归一化后，经"显示 → 图像"逆映射得到图像坐标，四周再各扩 30%
    /// 边距（平移/小幅缩放时旧位图还能盖住视口，不至于露黑底）。
    pub(super) fn region_req(&self, l: &Loaded) -> RegionReq {
        let (nw, nh) = (l.natural.0 as f32, l.natural.1 as f32);
        let density = (self.scale() * self.dpi.max(0.5)).max(0.05);
        let whole = RegionReq {
            origin: (0.0, 0.0),
            size: (nw, nh),
            density,
            doc: l.natural,
            index: l.anim.index,
        };
        // dst 还没得出来（没视口信息）就整图兜底。
        if self.view.2 <= 1.0 || self.view.3 <= 1.0 {
            return whole;
        }
        let content = RectI::new(
            self.view.0 as i32,
            self.view.1 as i32,
            self.view.2 as i32,
            self.view.3 as i32,
        );
        let Some(dst) = self.dst_rect(content) else {
            return whole;
        };
        // 视口在显示坐标里的归一化矩形。
        let (rx, ry) = (
            (self.view.0 - dst.x as f32) / dst.w as f32,
            (self.view.1 - dst.y as f32) / dst.h as f32,
        );
        let (rw, rh) = (self.view.2 / dst.w as f32, self.view.3 / dst.h as f32);
        let (x0, y0) = (rx.clamp(0.0, 1.0), ry.clamp(0.0, 1.0));
        let (x1, y1) = ((rx + rw).clamp(0.0, 1.0), (ry + rh).clamp(0.0, 1.0));
        // 逆映射：先撤镜像（dx_rot = 1 − dx），再撤旋转（角点级）。正向见
        // [`Self::map_rect_to_display`]，这里是它的逆。
        let (ix0, iy0, ix1, iy1) = rect_map_image(self.rot, self.flip, x0, y0, x1, y1);
        // 30% 边距并夹进图内。
        let (mx, my) = ((ix1 - ix0) * 0.3, (iy1 - iy0) * 0.3);
        let (ox0, oy0) = ((ix0 - mx).clamp(0.0, 1.0), (iy0 - my).clamp(0.0, 1.0));
        let (ox1, oy1) = ((ix1 + mx).clamp(0.0, 1.0), (iy1 + my).clamp(0.0, 1.0));
        RegionReq {
            origin: (ox0 * nw, oy0 * nh),
            size: ((ox1 - ox0) * nw, (oy1 - oy0) * nh),
            density,
            doc: l.natural,
            index: l.anim.index,
        }
    }
}
