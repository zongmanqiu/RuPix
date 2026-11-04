//! 缩放与平移：`fit` 两档语义、滚轮缩放的锚点、拖窗与边界夹取。
//!
//! 铁律：`fit = true` 时倍率**由视口算出**，绝不把推导结果写回 `zoom`——
//! 那正是「最大化窗口后图片突然变大」那个经典 bug 的成因。
use super::transform::*;
use super::*;

/// 溢出到图外时，仍然必须留在窗口内的**最少**图像宽度（逻辑 dp，两轴各算一次）。
///
/// 这条常数是"能不能把图拖丢"的界线。qView 的做法是给场景开一个
/// `QGraphicsScene(-1e6, -1e6, 2e6, 2e6)` 的固定 sceneRect（`.ex` 里那份源码的构造函数），
/// 于是它的滚动范围几乎无限——图能被拖到**完全看不见**。我们抄它的手感，
/// 不抄它的下界：留一枚 24dp 的"衣角"在画面里，用户永远看得见东西在哪儿、
/// 也永远能一把拖回来；而按 `0` / `1` 复位只是第二道保险，不该是唯一的出口。
pub(super) const PAN_KEEP_VISIBLE: f32 = 24.0;

/// 单轴的平移上限：`(视口 + 图) / 2 - 至少留在画面里的量`。
///
/// 纯函数抽出来的理由与画布那边的 `beyond_slop` 同一条——这条线就是手感的全部，
/// 要能在无头环境里钉住。三个分支各有实感，都得测：
/// 大图（图边可以越过窗口边，只留衣角）、小图（能拖到只剩衣角）、
/// 图或窗口比 `PAN_KEEP_VISIBLE` 还小（**留不下衣角就整张留在画面里**，上限退回"图边贴窗口边"）。
pub(super) fn overscroll(view: f32, img: f32) -> f32 {
    if view <= 0.0 || img <= 0.0 {
        return 0.0;
    }
    let keep = PAN_KEEP_VISIBLE.min(img).min(view);
    ((view + img) / 2.0 - keep).max(0.0)
}

impl Viewer {
    // ── 缩放 / 平移 ──────────────────────────────────────────────

    /// 当前实际倍率（**屏上**朝向）：fit 档由视口推导，手动档用 `zoom`。
    pub fn scale(&self) -> f32 {
        self.scale_in(self.rot)
    }

    /// 目标朝向下的倍率：投递旋转/镜像时用（fit 档的尺寸随朝向换轴）。
    pub(super) fn scale_in(&self, rot: u8) -> f32 {
        if self.fit {
            self.fit_scale_in(rot)
        } else {
            self.zoom.clamp(MIN_ZOOM, MAX_ZOOM)
        }
    }

    /// 长图判定（菠萝 isLongImage 同阈值）：旋转后宽高比 > 2.5 或 < 0.4。
    pub(super) fn is_long_image_in(&self, rot: u8) -> bool {
        let (nw, nh) = self
            .loaded
            .as_ref()
            .map_or((0.0, 0.0), |l| Self::eff_dims_in(l, rot));
        if nw <= 0.0 || nh <= 0.0 {
            return false;
        }
        let ratio = nw / nh;
        !(0.4..=2.5).contains(&ratio)
    }

    pub(super) fn fit_scale_in(&self, rot: u8) -> f32 {
        let (nw, nh) = match self.loaded.as_ref() {
            Some(l) => Self::eff_dims_in(l, rot),
            None => return 1.0,
        };
        let (vw, vh) = (self.view.2, self.view.3);
        if nw <= 0.0 || nh <= 0.0 || vw <= 0.0 || vh <= 0.0 {
            return 1.0;
        }
        // 长图模式（菠萝 autoLongImageMode）：按短边轴适应（竖图按宽、横图按高），
        // 另一轴远超视口，靠拖动浏览——clamp_pan 天然放行该方向。
        if self.config.borrow().long_image_fit && self.is_long_image_in(rot) {
            return if nw < nh { vw / nw } else { vh / nh };
        }
        // 适应窗口 = 充满视口（等比取小边），**允许放大**。
        // 旧规则"永不放大小图"让小图下适应 ≡ 1:1，两个按钮失去区分（2026-10-08 用户反馈），
        // 故改为与菠萝看图及主流看图器一致：小图放大到贴满视口。
        (vw / nw).min(vh / nh)
    }

    /// 视口中心（绝对逻辑坐标）。缩放锚点在没有指针位置时就用它。
    pub fn center(&self) -> (f32, f32) {
        (
            self.view.0 + self.view.2 / 2.0,
            self.view.1 + self.view.3 / 2.0,
        )
    }

    /// 以视口中心为锚点缩放（按钮 / 键盘触发时用：画面中心保持不动）。
    pub fn zoom_at_center(&mut self, z: f32) {
        let c = self.center();
        self.zoom_to_at(z, c, c);
    }

    /// 把倍率设到 `z`（并切到手动档）。`center` 是视口中心，`anchor` 是要保持不动的点。
    pub fn zoom_to_at(&mut self, z: f32, anchor: (f32, f32), center: (f32, f32)) {
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        // 旋转后宽高互换：锚点映射发生在**显示空间**，必须用有效尺寸。
        let (nw, nh) = self.eff_dims_of(l);
        let old = self.scale();
        let new = z.clamp(MIN_ZOOM, MAX_ZOOM);

        // 锚点不动：先求锚点在图中的相对位置 u/v，再反解新的平移量。
        let (dw, dh) = (nw * old, nh * old);
        let img_x = center.0 + self.pan.0 - dw / 2.0;
        let img_y = center.1 + self.pan.1 - dh / 2.0;
        let u = if dw > 0.0 {
            (anchor.0 - img_x) / dw
        } else {
            0.5
        };
        let v = if dh > 0.0 {
            (anchor.1 - img_y) / dh
        } else {
            0.5
        };
        let (dw2, dh2) = (nw * new, nh * new);

        self.zoom = new;
        self.fit = false;
        self.pan.0 = anchor.0 - u * dw2 + dw2 / 2.0 - center.0;
        self.pan.1 = anchor.1 - v * dh2 + dh2 / 2.0 - center.1;
        self.clamp_pan();
        self.mark_need();
    }

    /// 滚轮缩放：以指针位置为锚点，**每格（WHEEL_DELTA=120）±10%**。
    ///
    /// `delta` 是 Windows 原始滚轮增量：普通鼠标一格送 ±120，高分辨率滚轮与触摸板
    /// 会送更碎的增量（±10、±40 …）。这里按 120 折算成格数再取 `1.10^格数`，
    /// 碎增量就得到相应比例的小步缩放（1.10^0.1 ≈ +0.95%），普通滚轮每格恰好 ±10%。
    ///
    /// 坑：不能把 `delta` 当格数直接做 `powi`——一格就是 1.15¹²⁰，倍率瞬间撞上
    /// 上限钳制，表现是"滚一下直接拉满/缩没"。
    pub fn zoom_by_wheel(&mut self, delta: i32, anchor: (f32, f32), center: (f32, f32)) {
        const WHEEL_DELTA: f32 = 120.0;
        const STEP: f32 = 1.10;
        let notches = delta as f32 / WHEEL_DELTA;
        let factor = STEP.powf(notches);
        let z = self.scale() * factor;
        self.zoom_to_at(z, anchor, center);
    }

    /// 切到/退出"适应窗口"。
    pub fn set_fit(&mut self, on: bool) {
        if on {
            self.fit = true;
            self.pan = (0.0, 0.0);
            self.mark_need();
        } else {
            // 退出 fit 时把当前推导出的倍率固化下来，画面才不会跳。
            self.zoom = self.scale();
            self.fit = false;
        }
    }

    pub fn is_fit(&self) -> bool {
        self.fit
    }

    /// 顺时针旋转 90°。
    ///
    /// 位图是"缩到上屏"的尺寸时旋转是毫秒级，当场算；**手里是原图档**（≥100% 档）
    /// 时同一份搬运要 75–200 ms，交给派生线程，屏上旧朝向一直画着，落地再换
    /// （见 [`Self::apply_orientation`]）。
    ///
    /// ★ 不返回 bool：上一版返回的是 `loaded.is_some()`（＝成功），文档却写"是否需要
    /// 重绘"——与 [`Self::step_anim`] 同一处坑（K60：两个语义共用一个 bool，跨层一读就错）。
    /// 生产侧本来就没有读者，直接把那个值删掉。
    pub fn rotate_cw(&mut self) {
        if self.loaded.is_none() {
            return;
        }
        self.want_rot = (self.want_rot + 1) % 4;
        self.apply_orientation();
    }

    /// 水平镜像（对旋转后的画面生效）。返回值同 [`Self::rotate_cw`]。
    pub fn toggle_flip(&mut self) {
        if self.loaded.is_none() {
            return;
        }
        self.want_flip = !self.want_flip;
        self.apply_orientation();
    }

    /// 把目标朝向落到屏上：小图当场重算，大图投给派生线程。
    ///
    /// 异步期间 `rot/flip`（＝屏上朝向）不动，所有几何继续按旧朝向算，画面不会
    /// 被拉伸；落地由 [`Self::poll_levels`] 一次性换像素、换朝向、换鸟瞰。
    /// 投递失败（线程起不来）退回同步——宁可顿一下，不许按键没反应。
    pub(super) fn apply_orientation(&mut self) {
        if (self.want_rot, self.want_flip) == (self.rot, self.flip) {
            // 目标又转回屏上这一朝向（例如连按四次）：屏上已经是对的，不必再算。
            return;
        }
        let want = self.desired_pixels();
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        let px = l.decoded.0 as u64 * l.decoded.1 as u64 + want.0 as u64 * want.1 as u64;
        if px <= ORIENT_SYNC_PIXELS || !self.post_level(want) {
            self.commit_orientation();
            self.rebuild_image();
            self.clamp_pan();
        }
    }

    /// 目标朝向 = 屏上朝向。只在"像素已经按这个朝向算好"时调用。
    pub(super) fn commit_orientation(&mut self) {
        self.rot = self.want_rot;
        self.flip = self.want_flip;
    }

    /// 重建上屏位图：按当前显示档从 `base_rgba` **本地派生**（含 rot/flip）。
    ///
    /// 缩放、改窗口、旋转、镜像全走这一条——它不碰解码线程（模块头「位图的两档」）。
    pub(super) fn rebuild_image(&mut self) {
        let (rot, flip, want) = (self.rot, self.flip, self.desired_pixels());
        let Some(l) = self.loaded.as_mut() else {
            return;
        };
        // 切片态（SVG）不缩：位图就是那一块的原样像素，密度记在 `Region` 里，
        // 再缩一刀等于把矢量渲染成果抹掉。
        let shrink = l.region.is_none();
        let (displayed, image) = crate::core::derive::derive_level(
            &l.base_rgba,
            l.decoded.0,
            l.decoded.1,
            want,
            rot,
            flip,
            shrink,
        );
        l.displayed = displayed;
        l.image = image;
        // 鸟瞰那份只跟**朝向**有关，跟缩放/换档无关——所以按 `nav_key` 判断，
        // 缩放一步都不重画它。（原来这里每次换档都从整图重造一份，见字段文档。）
        if l.nav_image.is_none() || l.nav_key != (rot, flip) {
            let nav = l
                .nav_thumb_rgba
                .clone()
                .map(|(rgba, nw, nh)| Self::nav_image_of(&rgba, nw, nh, rot, flip));
            l.nav_image = nav;
            l.nav_key = (rot, flip);
        }
    }

    /// 把目标档交给派生线程去采。返回 `false` = 线程起不来（调用方必须退回同步派生，
    /// 不许让画面停在旧档不动）。同一档已在途时返回 `true` 但不重复投递。
    ///
    /// 任务带的是**目标朝向**（`want_rot/want_flip`）：换档与旋转可以拼进同一次派生，
    /// 落地时由 [`Self::poll_levels`] 一并提交。
    pub(super) fn post_level(&mut self, target: (u32, u32)) -> bool {
        let key = (self.epoch, target, self.want_rot, self.want_flip);
        if self.inflight == Some(key) {
            return true;
        }
        let (base, decoded, shrink) = match self.loaded.as_ref() {
            Some(l) => (
                Arc::clone(&l.base_rgba),
                l.decoded,
                // 切片态（SVG）不许再缩第二刀，与 `rebuild_image` 同一条判据。
                l.region.is_none(),
            ),
            None => return false,
        };
        if self.levels.is_none() {
            self.levels = Some(match crate::core::derive::LevelThread::spawn() {
                Ok(t) => t,
                Err(e) => {
                    log::error!("派生线程起不来（退回同步换档，UI 线程会顿一下）：{e}");
                    return false;
                }
            });
        }
        let posted = self.levels.as_ref().is_some_and(|t| {
            t.post(crate::core::derive::LevelJob {
                epoch: key.0,
                base,
                decoded,
                want: target,
                rot: self.want_rot,
                flip: self.want_flip,
                shrink,
            })
        });
        if posted {
            self.inflight = Some(key);
        }
        posted
    }

    /// 收派生线程采好的档位。**主循环每轮 `tick` 之后调一次**；返回值是"画面换了档、
    /// 要重绘"，吞掉就等于采好了却不刷新（与 [`Self::tick`] 同一条铁律，故 `#[must_use]`）。
    ///
    /// 过期回包（换了图 / 转了向 / 目标档又变了）一律丢弃：画面继续停在当前档，
    /// 不换、不闪。丢弃时重新约一次防抖，让最新那一档随后补上。
    ///
    /// 旋转/镜像的那一档也走这里：投递时带的是目标朝向，落地时一次性把像素、朝向、
    /// 鸟瞰一起换掉——所以"换朝向"发生在**这份像素真到了**之后，不是按键那一刻。
    #[must_use = "返回值是「画面换了档、要重绘」，吞掉就等于采好了却不刷新"]
    pub fn poll_levels(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(t) = self.levels.as_mut() else {
                return changed;
            };
            let Some(d) = t.take() else { return changed };
            let mine = self.inflight == Some((d.epoch, d.want, d.rot, d.flip));
            // 朝向也必须还是目标朝向：连按四次转回原朝向时，在途那一份(rot=1)算完
            // 就不该上屏（屏上本来就是 rot=0）。
            if !mine || (d.rot, d.flip) != (self.want_rot, self.want_flip) {
                // 过期。`d.epoch` 不等于当前代号 = 已经不是这张图，连重约都不必。
                // 两条都记同一颗钉：CPU 已经烧下去了，只是画面不要它（⑫ 要分的正是这一笔）。
                t.note_discarded();
                if mine {
                    self.inflight = None;
                }
                // 只有屏上这张就是当前图时才值得重约：翻页之后屏上还是旧图，
                // 重约只会让 tick 再发一枚重复的解码请求。
                if d.epoch == self.epoch && self.loaded_is_current() {
                    self.mark_need();
                }
                continue;
            }
            self.inflight = None;
            let Some(l) = self.loaded.as_mut() else {
                continue;
            };
            l.displayed = d.shown;
            l.image = d.image;
            // 鸟瞰只跟朝向有关：朝向真的换了才重画一份。
            if l.nav_image.is_none() || l.nav_key != (d.rot, d.flip) {
                l.nav_image = l
                    .nav_thumb_rgba
                    .clone()
                    .map(|(rgba, nw, nh)| Self::nav_image_of(&rgba, nw, nh, d.rot, d.flip));
                l.nav_key = (d.rot, d.flip);
            }
            self.rot = d.rot;
            self.flip = d.flip;
            self.clamp_pan();
            // 落地这一份可能已经过时（按 R 之后用户又按了 R）：目标还没到就接着追。
            if (self.rot, self.flip) != (self.want_rot, self.want_flip) {
                self.mark_need();
            }
            self.bump_image_gen();
            changed = true;
        }
    }

    /// 屏上这份是不是清单当前项（没有清单 = 合成上下文，算当前）。
    pub(super) fn loaded_is_current(&self) -> bool {
        let Some(l) = self.loaded.as_ref() else {
            return false;
        };
        match self.gallery.as_ref().and_then(|g| g.current()) {
            Some(p) => *p == l.path,
            None => true,
        }
    }

    /// 派生线程的四条计数快照（`§18` ⑫：投了多少 / 合流吞了多少 / 算完多少 / 算完被丢多少）。
    /// 没起过线程就全 0。探针与单测读它，**生产行为一分不依赖它**。
    pub fn level_counts(&self) -> crate::core::derive::LevelCounts {
        match self.levels.as_ref() {
            Some(t) => t.counts(),
            None => crate::core::derive::LevelCounts::default(),
        }
    }

    /// 鸟瞰的**来源**：先把整图收进 [`NAV_THUMB_MAX`] 再存。本来就不比缩略档大的，
    /// 直接共享 base 那份 `Arc`（不另占字节）。
    pub(super) fn nav_thumb_of(base: &Arc<Vec<u8>>, w: u32, h: u32) -> (Arc<Vec<u8>>, u32, u32) {
        let (fw, fh) = fit_dims(w, h, NAV_THUMB_MAX, NAV_THUMB_MAX);
        if (fw, fh) == (w, h) {
            return (Arc::clone(base), w, h);
        }
        (Arc::new(downscale(base, w, h, fw, fh)), fw, fh)
    }

    /// 由鸟瞰来源按给定 rot/flip 造实际画的那份位图。
    ///
    /// 来源在 [`Viewer::nav_thumb_of`] 里已经缩到 ≤[`NAV_THUMB_MAX`]，这里只剩按像素
    /// 搬一次。**先缩后转**与 [`crate::core::derive::derive_level`] 同一条顺序——旋转是
    /// 按像素搬的，先缩能少搬几千万个像素（6055×6055 直接转就是 146 MB 搬运 + 146 MB 常驻）。
    pub(super) fn nav_image_of(
        rgba: &Arc<Vec<u8>>,
        w: u32,
        h: u32,
        rot: u8,
        flip: bool,
    ) -> RgbaImage {
        let (tw, th, bytes) = transformed(rgba, w, h, rot, flip);
        RgbaImage::new(tw, th, bytes)
    }

    /// 鸟瞰缩略图应显示的位图与它在视口中的目标矩形。
    ///
    /// 鸟瞰永远显示**完整图片**（菠萝 NavigatorView 语义），画的是 [`Loaded::nav_image`]
    /// 那份 ≤[`NAV_THUMB_MAX`] 的缩略——切片态与整图态都是它，两条路线的几何都锚在
    /// 整图 `dst_rect` 上。只有连缩略来源都还没有（首帧之前）才退回主位图。
    pub fn nav_image_dst(&self, content: RectI) -> Option<(&RgbaImage, RectI)> {
        let l = self.loaded.as_ref()?;
        if let Some(img) = &l.nav_image {
            return Some((img, self.dst_rect(content)?));
        }
        Some((self.image()?, self.image_dst(content)?))
    }

    /// 1:1（原图像素）。
    pub fn zoom_actual(&mut self) {
        self.zoom = 1.0;
        self.fit = false;
        self.pan = (0.0, 0.0);
        self.mark_need();
    }

    pub fn begin_drag(&mut self, pos: (f32, f32)) {
        self.drag = Some(Drag {
            start: pos,
            origin: self.pan,
        });
    }

    /// 拖拽中：`true` 表示平移量真的变了、需要重绘；`false` = 没在拖，或拖到边界外
    /// 被钳回原地（M6：后者原来每个 move 都整窗重绘一遍，画面上一个像素都没动）。
    #[must_use = "返回值是「画面变了、要重绘」，吞掉就等于拖了没反应"]
    pub fn drag_to(&mut self, pos: (f32, f32)) -> bool {
        let Some(d) = self.drag.as_ref() else {
            return false;
        };
        let (ox, oy) = d.origin;
        let p = (ox + (pos.0 - d.start.0), oy + (pos.1 - d.start.1));
        self.apply_pan(p)
    }

    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    /// 当前平移量（鸟瞰导航读）。
    pub fn pan(&self) -> (f32, f32) {
        self.pan
    }

    /// 直接设置平移量（鸟瞰导航写：把缩略图上的点映射回视口中心）。
    /// 内部仍走夹取，不会把图拖出边界。**返回值 = 画面真的变了**，见 [`Self::apply_pan`]。
    #[must_use = "返回值是「画面变了、要重绘」，吞掉就等于拖了没反应"]
    pub fn set_pan(&mut self, p: (f32, f32)) -> bool {
        self.apply_pan(p)
    }

    /// WASD 平移一步（m216）：`sx`/`sy` ∈ {-1, 0, +1}，是**视口该往哪边看**的符号——
    /// `sy = +1` 是"看上方"（W），图像整体下移，pan.y 增大。步长 = 视口短边的 1/8
    /// （下限 24 逻辑像素），内部常量不进配置（待办 §59 已拒「平移步长参数暴露」）。
    #[must_use = "返回值是「画面变了、要重绘」，吞掉就等于按了没反应"]
    pub fn pan_nudge(&mut self, sx: f32, sy: f32) -> bool {
        let step = self.pan_step_len();
        self.apply_pan((self.pan.0 + sx * step, self.pan.1 + sy * step))
    }

    /// 平移步长：视口短边的 1/8，下限 24 逻辑像素（再小按一下就看不出生移动）。
    fn pan_step_len(&self) -> f32 {
        let step = self.view.2.min(self.view.3) / 8.0;
        if step < 24.0 {
            24.0
        } else {
            step
        }
    }

    /// 平移的唯一入口：写入 → 夹取 → **比较**。返回 `true` 才算"画面变了、要重绘"。
    ///
    /// 为什么要比较：拖到图片边界外时 `clamp_pan` 把新值钳回同一个位置，而拖动仍在
    /// 继续、每个 move 事件都照样进来——不比较就是每移一下都整窗重绘一遍，画面上
    /// 一个像素都没变（用户报"拖鸟瞰时 CPU 稳定 5–6%"里纯浪费的那一段，M6）。
    /// 同一条理由也管 SVG：pan 没动就不该调度区域重渲。
    pub(super) fn apply_pan(&mut self, p: (f32, f32)) -> bool {
        let before = self.pan;
        self.pan = p;
        self.clamp_pan();
        if self.pan == before {
            return false;
        }
        self.mark_region_need();
        true
    }

    /// 平移变了：SVG 可视区域切片必须跟着重渲，否则拖出旧位图 + 边距的范围
    /// 就是空白。位图不需要（整图本来就在内存里），别白白触发解码。
    pub(super) fn mark_region_need(&mut self) {
        if self.loaded.as_ref().is_some_and(|l| is_vector(&l.path)) {
            self.mark_need();
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// 平移的可达范围（半宽 / 半高，逻辑像素）——**允许图边越过窗口边**，
    /// 但永远还剩 [`PAN_KEEP_VISIBLE`] 在画面里（见 [`overscroll`]）。
    ///
    /// 2026-11-03 用户第三条与第四条口径的落点：
    /// · 第三条："任何情况下，开启这个功能就都要能拖动"（图比窗口小也拖得动）；
    /// · 第四条："拖图范围相当于被鸟瞰图限制死了……允许图片的边缘离开我们的窗口边缘，
    ///   并让整个图片继续向上移动"。旧口径 `|图 - 视口| / 2` 的含义是"永远不许露出黑边"，
    ///   那正是把图钉死在窗口里的那道界。
    ///
    /// 仍然返回 `(0, 0)` 的只有：还没有图，或视口还没量出来（首帧之前）。
    ///
    /// ★ 夹取与"能不能拖"共用这一份算法：两处各写一遍就会出现"夹出来其实拖不动、
    /// 但判据说可拖"的手感（按下、拖动、画面一动不动）。
    pub(super) fn pan_limits(&self) -> (f32, f32) {
        let (nw, nh) = self.eff_dims();
        if nw <= 0.0 {
            return (0.0, 0.0);
        }
        let s = self.scale();
        (
            overscroll(self.view.2, nw * s),
            overscroll(self.view.3, nh * s),
        )
    }

    /// 这张图**此刻拖得动吗**——左键手势分工的判据之一（见 `ui::canvas::left_drag`）。
    ///
    /// 溢出上限改成 [`overscroll`] 之后，这条几乎总是真：只剩"没有图"与"视口还没量出来"
    /// 两种拖不动的情况，那一按留着拖窗口（**不许把拖窗口这条路堵死**）。
    pub fn can_pan(&self) -> bool {
        let (mx, my) = self.pan_limits();
        mx > 0.0 || my > 0.0
    }

    /// 平移夹取：上限来自 [`Self::pan_limits`]——图边可以越过窗口边，
    /// 但画面里永远还剩 [`PAN_KEEP_VISIBLE`]（图比衣角还小就整张留在画面里）。
    pub(super) fn clamp_pan(&mut self) {
        let (max_x, max_y) = self.pan_limits();
        self.pan.0 = self.pan.0.clamp(-max_x, max_x);
        self.pan.1 = self.pan.1.clamp(-max_y, max_y);
    }
}
