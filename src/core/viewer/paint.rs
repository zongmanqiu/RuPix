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

    /// 取色器：屏幕坐标 → 当前显示的那枚像素。
    ///
    /// 两套几何各司其职，不许混：
    /// - [`Self::image_dst`] 是**上屏位图**在视口里的目标矩形（SVG 切片时只是整图的一块）——
    ///   采样必须用它，否则区域渲染时取到的是整图错位后的那份颜色。
    /// - [`Self::dst_rect`] 是**整图**在显示朝向下的目标矩形——换算原图坐标必须用它，
    ///   因为 [`rect_map_image`] 收的是相对整图的归一化显示坐标。
    ///
    /// 落在图外（棋盘格 / 空场）返回 `None`：那里没有像素可报。
    pub fn pick_at(&self, content: RectI, pos: (f32, f32)) -> Option<Pick> {
        let l = self.loaded.as_ref()?;
        let img = &l.image;
        let base = self.dst_rect(content)?;
        let idst = self.image_dst(content)?;
        if img.width == 0
            || img.height == 0
            || idst.w <= 0
            || idst.h <= 0
            || base.w <= 0
            || base.h <= 0
        {
            return None;
        }
        // 半开区间：右/下边界那一条属于"图外"，取色器不给读数。
        let u = (pos.0 - idst.x as f32) / idst.w as f32;
        let v = (pos.1 - idst.y as f32) / idst.h as f32;
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        // 所见即所得：上屏位图已按 rot/flip 转正，局部归一化直接就是它的像素坐标。
        let px = ((u * img.width as f32) as u32).min(img.width - 1);
        let py = ((v * img.height as f32) as u32).min(img.height - 1);
        let i = (py as usize * img.width as usize + px as usize) * 4;
        let c = img.rgba.get(i..i + 4)?;
        // 把切片里的局部点折回整图显示坐标，再逆掉 rot/flip 得到原图坐标。
        // 零宽矩形即可：rect_map_image 对点是逐角点变换，退化时仍给出正确单点。
        let dx = (idst.x as f32 - base.x as f32 + u * idst.w as f32) / base.w as f32;
        let dy = (idst.y as f32 - base.y as f32 + v * idst.h as f32) / base.h as f32;
        let (ix, iy, _, _) = rect_map_image(self.rot, self.flip, dx, dy, dx, dy);
        let at = (
            ((ix * l.natural.0 as f32).floor() as u32).min(l.natural.0.saturating_sub(1)),
            ((iy * l.natural.1 as f32).floor() as u32).min(l.natural.1.saturating_sub(1)),
        );
        Some(Pick {
            rgba: [c[0], c[1], c[2], c[3]],
            at,
        })
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
