//! 悬浮层与缩略图条要的查询：`ui::chrome` / `ui::filmstrip` 的读数入口。
//!
//! 全是只读查询（状态栏、鸟瞰、缩略图槽位），不推进状态机。
use super::transform::*;
use super::*;

impl Viewer {
    // ── 悬浮层查询（ui::chrome 用）─────────────────────────────

    /// 完整视口矩形 (x, y, w, h)，供悬浮层显隐判定用。
    pub fn view_rect(&self) -> (f32, f32, f32, f32) {
        self.view
    }

    /// 解码错误文本（画布居中显示用）。`None` = 没有错误。
    ///
    /// 返回的是已经排好的两行：分类标题 + 下一步提示（原文判词见 `ViewFailure::detail`，
    /// 已由 `fail` / `report_failure` 写进日志）。
    pub fn error_text(&self) -> Option<String> {
        self.error.as_ref().map(ViewFailure::display_text)
    }

    /// 原图像素尺寸（「文件信息」菜单用）。
    ///
    /// 优先返回**渲染**结果（`loaded.natural`）；未装到位图时退回**查询阶段**的
    /// 预览（`preview_meta.natural`）——让 info bar 与「按原始像素开窗」在整图解码
    /// 完成前先动一步。查询值与渲染值不一致时（罕见，解码器 bug），渲染一到即覆盖。
    pub fn info_dims(&self) -> Option<(u32, u32)> {
        self.loaded
            .as_ref()
            .map(|l| l.natural)
            .or_else(|| self.preview_meta.map(|m| m.natural))
    }

    /// 有上一张可翻（决定左侧箭头画不画）。翻页永远循环，多张即恒有。
    pub fn has_prev(&self) -> bool {
        self.gallery.as_ref().is_some_and(|g| g.len() > 1)
    }

    /// 有下一张可翻（决定右侧箭头画不画）。同 [`Viewer::has_prev`]。
    pub fn has_next(&self) -> bool {
        self.gallery.as_ref().is_some_and(|g| g.len() > 1)
    }

    // ── 底部缩略图条 ────────────────────────────────────────────

    /// 目录里的图片数（缩略图条决定画不画、画几格）。
    pub fn gallery_len(&self) -> usize {
        self.gallery.as_ref().map_or(0, Gallery::len)
    }

    /// 当前在目录里的序号（缩略图条高亮当前格）。
    pub fn gallery_pos(&self) -> usize {
        self.gallery.as_ref().map_or(0, Gallery::position)
    }

    /// 胶片条这一屏该显示哪几格：返回 `(首序号, 格数)`，并顺带为**缺图的格子**发起后台解码。
    ///
    /// `cap` 是要显示的格数（由 UI 按视口宽度算）。窗口以当前图为中心；翻到末尾时不再
    /// 居中（否则右边会空出半排格子）。
    ///
    /// ★ m100：这里不再交出 `Vec<(usize, PathBuf)>`。旧签名每帧克隆 2×格数 个 `PathBuf`
    ///   （一份给返回值、一份给 `ensure`），而格子要的只是"第几格是哪张图"——清单里本来就存着，
    ///   按引用给出去就够了。旧/新两臂留在 `examples/frame_cost_probe.rs` 的 D2 里长期可跑。
    ///   **没有加窗口缓存**：缓存要养一份失效判据（改名、兄弟增删都要对），换来的只有零点几微秒，
    ///   不划算（判据见 本仓历史记录）。
    ///
    /// 路径请用 [`Self::strip_path`] 按索引取（借用的有效区就是这次调用之后、下一次改动之前）。
    pub fn strip_window(&mut self, cap: usize, anchor: Option<usize>) -> (usize, usize) {
        let Some(g) = self.gallery.as_ref() else {
            return (0, 0);
        };
        let len = g.len();
        if len == 0 {
            return (0, 0);
        }
        let n = cap.clamp(1, len);
        // g.position() 是 1-based，strip_window_first 期望 0-based，需减 1
        let cur_0based = g.position().saturating_sub(1);
        // `None` = 没滚过，跟着当前图居中；`Some` = 用户滚过，条就停在他滚到的地方
        // （m260：**不再**把当前图拽回视野，见 `strip_window_anchored`）。
        let first = match anchor {
            None => strip_window_first(len, cur_0based, n),
            Some(a) => strip_window_anchored(len, n, a),
        };
        let refs: Vec<&Path> = (first..first + n)
            .filter_map(|i| g.peek(i).map(PathBuf::as_path))
            .collect();
        let n = refs.len();
        self.thumbs.ensure(&self.decoder, &refs);
        // m265：这一屏同时也是「图片预读」的下注范围。条滚到哪儿，预读跟到哪儿；
        // 点格子能切到的图就全在这一屏里，不需要按当前索引去猜。
        self.set_prefetch_strip(first, n);
        (first, n)
    }

    /// 清单里第 `idx` 张的路径（**按引用**，与 [`Self::strip_window`] 配对使用）。
    pub fn strip_path(&self, idx: usize) -> Option<&Path> {
        self.gallery.as_ref()?.peek(idx).map(PathBuf::as_path)
    }

    /// 已就绪的缩略图（未解码 / 解码失败时为 `None`，那一格留空）。
    pub fn thumb(&self, path: &Path) -> Option<Rc<RgbaImage>> {
        self.thumbs.get(path)
    }

    /// 跳到目录里的第 `idx` 张（缩略图条点击）。越界夹住；位置没变返回 false。
    pub fn goto(&mut self, idx: usize) -> bool {
        let moved = self.gallery.as_mut().is_some_and(|g| g.jump_to(idx));
        if moved {
            self.navigate();
        }
        moved
    }

    /// 动画元信息（供属性窗口展示）：未动画返回 None，否则 (总帧数, 每帧时长ms)。
    /// 帧数可能为 0（GIF 惰性数帧，解码到 EOF 才回填），调用方据此显示"帧数未知"。
    pub fn anim_summary(&self) -> Option<(u32, u32)> {
        let a = self.loaded.as_ref()?.anim;
        if a.delay_ms == 0 {
            None
        } else {
            Some((a.count, a.delay_ms))
        }
    }
}
