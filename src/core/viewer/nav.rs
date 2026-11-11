//! 打开与导航：装新图、翻页、跳转、目录重扫、尺寸口径。
//!
//! 「切图不闪烁」的时序前提在这里：`reset_view` **不许**清 `loaded`，旧图要留屏到
//! 新解码结果由 `Viewer::apply` 原子替换（见 `tests.rs` 的用例）。
use super::*;

impl Viewer {
    pub fn new(decoder: Rc<Decoder>, config: Rc<RefCell<Config>>) -> Self {
        Self {
            sort_epoch: 0,
            decoder,
            config,
            gallery: None,
            loaded: None,
            edited: std::collections::HashMap::new(),
            error: None,
            preview_meta: None,
            prefetch_seq: None,
            prefetch_stopped: false,
            meta_cache: std::collections::HashMap::new(),
            prefetched: Vec::new(),
            prefetch_strip: None,
            last_dir: 0,
            main_sent_at: Cell::new(None),
            stall_logged: Cell::new(false),
            wait_shown_secs: Cell::new(None),
            fit: true,
            zoom: 1.0,
            rot: 0,
            flip: false,
            want_rot: 0,
            want_flip: false,
            pan: (0.0, 0.0),
            view: (0.0, 0.0, 1.0, 1.0),
            chrome_top: 0.0,
            dpi: 1.0,
            want_seq: 0,
            need_at: None,
            last_nav_at: Cell::new(None),
            native_requested: None,
            vector_zoom_req: None,
            epoch: 0,
            levels: None,
            inflight: None,
            anim_next_at: None,

            anim_due_at: None,
            anim_paused: false,
            step_target: None,
            archive_skipped: 0,
            note: None,
            thumbs: Thumbs::new(),
            drag: None,
            image_gen: 0,
        }
    }

    // ── 打开 / 导航 ──────────────────────────────────────────────

    /// 打开一张图（同时把它所在目录建成清单）。
    ///
    /// ★ 压缩包在这里分流：`archive::resolve` 把 `.zip` / `.cbz` 换成**这一层**展开后的
    ///   目录，后面所有逻辑仍只看见一个目录路径。命令行 / 拖放 / 文件对话框 / 二次实例
    ///   转发最终都汇到本函数，所以入口不必各写一遍。进包内更深的一层走
    ///   [`Self::open_layer`]——那条路已经拿着 `Layer`，不必再解析一遍。
    pub fn open(&mut self, file: &Path) {
        let layer = match crate::core::archive::resolve(file) {
            Ok(l) => l,
            Err(e) => {
                self.fail(e);
                return;
            }
        };
        self.open_at(
            &layer.dir,
            layer.from_archive && layer.images == 0,
            layer.skipped,
        );
    }

    /// 打开压缩包里**已经展开好**的某一层（左侧文件树点那一行就走这里）。
    ///
    /// 空层合法：`1M.zip/ch1` 这种只有子目录的地方照样能停下，画面空着、树里继续往下，
    /// 不当成"打开失败"——`fail` 会把清单清掉，用户连菜单都没有了。
    pub fn open_layer(&mut self, layer: &crate::core::archive::Layer) {
        self.open_at(&layer.dir, layer.from_archive, layer.skipped);
    }

    fn open_at(&mut self, path: &Path, allow_empty: bool, skipped: usize) {
        let t0 = Instant::now();
        // ★ m304：压缩包层里「认得出但打不开」的条数。每次打开新层都重设
        //   （普通目录传 0），状态格据此追加后缀；不放进 Gallery，因为它描述的是
        //   **容器**而不是清单本身。
        self.archive_skipped = skipped;
        // 新开一张 = 换目录或换图，翻页方向无从推测。
        self.last_dir = 0;
        // ★ m101：**同一目录里再开一张不是换目录**。清单里认得它，就只换当前项。
        //   旧写法无条件 `Gallery::from_file` + 清 `prefetched` + 清 `thumbs`，于是
        //   资源管理器里连双击每张都要：整条胶片条重解（实测 6 张目录 = 每开一张
        //   6 发 `kind=Thumb`，格子越多发得越多）、首屏必走现场解码（预读命中率恒 0）。
        //   清单的新鲜度本来就不归 `open` 管——目录被外部增删由 `dir_watch` 送到
        //   [`Self::rescan_dir`]，而那条路**同样不清这两个缓存**，本分支与它口径一致。
        //   锚点不在清单里（刚拷进来的新文件、或它其实是个文件夹）⇒ 退回完整重建。
        let slot = self.gallery.as_ref().and_then(|g| g.slot_for(path));
        if let Some(idx) = slot {
            let len = self.gallery.as_ref().map_or(0, Gallery::len);
            log::debug!(
                "[open] 同目录复用清单（第 {}/{} 项，{} ms）→ 只换当前项",
                idx + 1,
                len,
                t0.elapsed().as_millis()
            );
            self.error = None;
            self.note = None;
            if let Some(g) = self.gallery.as_mut() {
                g.jump_to(idx);
            }
            self.navigate();
            return;
        }
        let built = if allow_empty && path.is_dir() {
            Gallery::from_dir(path, self.sort(), true)
        } else {
            Gallery::from_file(path, self.sort())
        };
        match built {
            Ok(g) => {
                log::debug!(
                    "[open] 清单就绪（{} 项，{} ms）→ 请求解码",
                    g.len(),
                    t0.elapsed().as_millis()
                );
                self.gallery = Some(g);
                self.error = None;
                // 换图 = 用户已经在看新东西，旧提示立刻作废（m50-C）。
                self.note = None;
                // 换了目录，旧预读全部作废；条那一屏的索引也换了目录，一并清掉。
                self.prefetch_seq = None;
                self.prefetched.clear();
                self.prefetch_strip = None;
                // 缩略图同理：旧目录那批再也看不见，别占着内存。
                self.thumbs.clear();
                self.anim_next_at = None;
                // m75：时间线随闸一起作废，下一帧重新起表（不续旧账）。
                self.anim_due_at = None;
                self.reset_view();
                // 空层（压缩包里只有子目录 / 子包的那一层）：画面就该是空的，但必须
                // 说清楚为什么、下一步在哪——否则用户以为文件坏了。
                if self.gallery.as_ref().is_some_and(|g| g.is_empty()) {
                    // ★ m312：m300 起「本包 ▸」二级菜单已经撤掉，子层在**左侧文件树**
                    //   里。旧文案还在指一个不存在的入口（用户实报「居然还存在…这种
                    //   旧提示」），而且是**给了一条走不通的指令**——比不给更糟。
                    //   按「还有没有子层」分两句：有子层才指路，没有就只说没有。
                    let subs = crate::core::archive::subs_of(path).len();
                    self.error = Some(if allow_empty && subs > 0 {
                        "这一层没有图片：从左侧文件树进入它的子层".to_string()
                    } else {
                        "这一层没有图片".to_string()
                    });
                    self.bump_image_gen();
                    return;
                }
                // 命中元数据缓存：之前看过这张图，立即填回预览尺寸。
                if let Some(g) = self.gallery.as_ref() {
                    if let Some(p) = g.current() {
                        if let Some(m) = self.meta_cache.get(p) {
                            self.preview_meta = Some(*m);
                        }
                    }
                }
                // 视口还没真实尺寸时不发请求（首帧 paint 之前是 (1,1)，会渲出一张
                // 1×1 废图白等一整轮，实测 1.2s+）——闸在 [`Self::request_decode`] 里，
                // 与翻页那条路共用同一条判据。
                self.request_decode();
                self.bump_image_gen();
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// 相对翻页。**永远循环**（用户拍板 2026-10-09）：3/3 再下一张 → 1/3。
    pub fn step(&mut self, delta: isize) -> bool {
        let moved = match self.gallery.as_mut() {
            Some(g) => g.step_looped(delta),
            None => false,
        };
        if moved {
            // 记方向供预读窗口定向（见 [`Self::last_dir`]）。跳转/新开图不经过这里，
            // 各自把方向清 0——那时"下一张往哪边"没有依据可赌。
            self.last_dir = delta.signum() as i8;
            self.navigate();
        }
        moved
    }

    /// 当前图"版本"自增：属性窗轮询比对这个值，变了才重算刷新。
    pub(super) fn bump_image_gen(&mut self) {
        self.image_gen = self.image_gen.wrapping_add(1);
    }

    pub fn jump(&mut self, to: Jump) {
        if let Some(g) = self.gallery.as_mut() {
            // 跳转后方向无从推测（跳到首/尾再往下翻与前翻一样可能），清零让预读
            // 回到对称窗口。
            self.last_dir = 0;
            g.jump(to);
            self.navigate();
        }
    }

    /// 翻页后的公共动作：复位视图 → 命中预读就零延迟上屏，否则发解码请求
    /// → 顺手为新的相邻图补预读。
    pub(super) fn navigate(&mut self) {
        // 换了当前项 = 用户在看另一张图，上一条提示立刻作废（m50-C）。
        self.note = None;
        // 翻页计时起点（m58：原图档升级在这段宽限期内让位给下一张的首屏）。
        self.last_nav_at.set(Some(Instant::now()));
        self.reset_view();
        // 翻页后旧动画的推进闸立即作废：新图要么是静态（无闸可续），
        // 要么由它自己的首帧 install 重新排。带着旧时刻表推进新图必然乱帧。
        self.anim_next_at = None;
        // m75：时间线随闸一起作废，下一帧重新起表（不续旧账）。
        self.anim_due_at = None;
        // 命中元数据缓存：之前看过这张图，立即填回预览尺寸，不必等 MetaEvent。
        if let Some(g) = self.gallery.as_ref() {
            if let Some(p) = g.current() {
                if let Some(m) = self.meta_cache.get(p) {
                    self.preview_meta = Some(*m);
                }
            }
        }
        if self.use_prefetched() {
            // 预读命中：矢量要按可视区域 1:1 重渲（tick 无闸必发）；位图由
            // tick 的 enough/at_native 闸自己判定，不满足才补解。
            self.mark_need();
        } else {
            self.request_decode();
        }
        self.request_prefetch();
        self.bump_image_gen();
    }

    /// 当前排序口径（真值在 `Config`：ini 键 `sort_by` + `sort_desc`）。
    pub fn sort(&self) -> SortSpec {
        self.config.borrow().sort_spec()
    }

    /// 排序口径的变更代数。界面据此把**按槽位**记的状态（缩略图条的首格）归零。
    pub fn sort_epoch(&self) -> u64 {
        self.sort_epoch
    }

    /// 把配置里的排序口径落到清单上——每帧对齐一次，与 `runtime::apply_session_flags`
    /// 同一套路（配置是唯一真值，菜单与手改 ini 都从这里生效）。
    ///
    /// 返回 `true` = 这次真的重排过（调用方标脏）。**不重解码、不重置视图**：当前这张
    /// 还是同一张（[`Gallery::set_sort`] 拿它当锚点），变的只是它两侧是谁。
    /// 清单还没建（空窗）时什么也不做：口径会在 `open` 那一刻从配置读。
    pub fn sync_sort(&mut self) -> bool {
        let want = self.sort();
        let Some(g) = self.gallery.as_mut() else {
            return false;
        };
        if g.sort() == want {
            return false;
        }
        self.sort_epoch = self.sort_epoch.wrapping_add(1);
        g.set_sort(want);
        log::debug!(
            "[sort] 重排 {:?}（倒序 {}）→ 当前 {}/{}",
            want.by,
            want.desc,
            g.position(),
            g.len()
        );
        true
    }

    /// 目录被外部（资源管理器）增删 / 改名后，重扫当前目录并尽量保留当前位置。
    ///
    /// 返回 `true` 表示清单或当前项变了（调用方据此刷新缩略图条 / 状态栏 / 标题）。
    /// 若当前正在看的图被删 / 改名，自动 [`Viewer::navigate`] 到相邻图并重新发起解码；
    /// 若只是兄弟项变了（分母变、当前图未动），则不动画面、只更新清单。
    ///
    /// 去抖不在这一层做——runtime 的 16ms 轮询负责（一次 tick 只调一次，把一整波
    /// 拷贝合并成一次 `read_dir`）。
    pub fn rescan_dir(&mut self) -> bool {
        let Some(g) = self.gallery.as_ref() else {
            return false;
        };
        let started = Instant::now();
        let res = g.rescan();
        self.apply_rescan(res, false, started)
    }

    /// 事件驱动的重扫：同一次文件变动会被**每扇窗各报一次**，这里只让第一扇窗真读盘。
    ///
    /// 为什么不把复用直接塞进 [`Self::rescan_dir`]：那个函数的语义是"现在去看这个目录"，
    /// 一批既有用例直接调它来验"删掉文件后清单跟着变"。缓存只在**明确知道这是同一批事件**
    /// 的这条路上生效，两条路互不污染（`rescan_never_serves_a_stale_batch` 钉着这一点）。
    pub fn rescan_dir_shared(&mut self) -> bool {
        let Some(g) = self.gallery.as_ref() else {
            return false;
        };
        let started = Instant::now();
        let dir = g.dir().to_path_buf();
        let (res, shared) = match crate::core::gallery::listing_for_event(&dir, g.sort()) {
            Ok((files, scanned)) => (g.rescan_from(files), !scanned),
            Err(e) => (Err(e), false),
        };
        self.apply_rescan(res, shared, started)
    }

    /// 两个入口共用的一段：拿新清单比对旧清单，决定要不要通知 UI 重绘 / 重解码。
    ///
    /// `shared` 只用于取证行（这次是复用了别人的扫描，还是自己读的盘）。
    fn apply_rescan(
        &mut self,
        rescan: crate::core::error::ViewResult<Gallery>,
        shared: bool,
        started: Instant,
    ) -> bool {
        // 旧值必须在 `self.gallery = Some(rebuilt)` 之前取。
        let old_current = self.gallery.as_ref().and_then(|g| g.current().cloned());
        let old_len = self.gallery.as_ref().map_or(0, |g| g.len());
        let rebuilt = match rescan {
            Ok(g) => g,
            // 目录还在、但一张图都没有了：不能继续显示已经删掉的那张（标题/分母/
            // 画面全是旧账）。读不到目录（盘掉线、权限）才保持旧清单，等它回来。
            Err(e @ ViewError::NoImages(_)) => {
                self.fail(e.to_string());
                return true;
            }
            Err(_) => return false,
        };
        let new_current = rebuilt.current().cloned();
        let current_gone = old_current.as_ref() != new_current.as_ref();
        // 兄弟项改名/重排时长度与当前项都没变，但胶片条内容已经不同；
        // `same_listing` 把这一形也算进"要通知 UI 重绘"。
        let same_listing = self
            .gallery
            .as_ref()
            .is_some_and(|old| old.same_listing(&rebuilt));
        let list_changed = rebuilt.len() != old_len || current_gone || !same_listing;
        self.gallery = Some(rebuilt);
        if current_gone {
            // 当前图没了 → 重新解码相邻图（navigate 会复位视图 + 请求解码）。
            self.navigate();
        }
        if list_changed {
            self.prefetch_stopped = false;
            self.request_prefetch();
            self.bump_image_gen();
        }
        // 一行取证（闸见 `viewer::dir_trace`）：多开时**每扇窗各来一条**，条数就是重扫次数，
        // `shared=1` 表示这一条没有读盘。`total` 含本窗自己的比较/预取重排。
        if super::dir_trace() {
            let dir = self
                .gallery
                .as_ref()
                .map(|g| g.dir().display().to_string())
                .unwrap_or_default();
            log::info!(
                "[dir-rescan] dir={} files={} total={:.2}ms changed={} shared={}",
                dir,
                self.gallery.as_ref().map_or(0, |g| g.len()),
                started.elapsed().as_secs_f64() * 1000.0,
                u8::from(list_changed),
                u8::from(shared)
            );
        }
        list_changed
    }

    /// 旋转/镜像后**有效**显示尺寸（rot 为奇数时宽高互换）。
    ///
    /// 以 `natural`（原图）为基准而非位图：dst_rect 画的是 `natural × scale`，
    /// 位图只是"恰好够清晰"的采样，放大绘制是常态（1:1 大图时）。
    pub(super) fn eff_dims_of(&self, l: &Loaded) -> (f32, f32) {
        Self::eff_dims_in(l, self.rot)
    }

    /// 任一朝向下的有效显示尺寸（`rot` 为奇数时宽高互换）。
    pub(super) fn eff_dims_in(l: &Loaded, rot: u8) -> (f32, f32) {
        match rot % 2 {
            0 => (l.natural.0 as f32, l.natural.1 as f32),
            _ => (l.natural.1 as f32, l.natural.0 as f32),
        }
    }

    /// 当前有效显示尺寸（无图时 (0, 0)）。
    pub(super) fn eff_dims(&self) -> (f32, f32) {
        self.loaded
            .as_ref()
            .map_or((0.0, 0.0), |l| self.eff_dims_of(l))
    }

    /// 视口状态复位：旋转/镜像归零、缩放回适应、平移归位——翻图恒重置变换。
    ///
    /// ★ **不清 `loaded`**（用户报障 2026-10-09「切图闪烁」的根因）：旧图必须
    /// 一直留在屏上，直到新图解码完成由 `Viewer::apply` 原子替换。提前清掉，
    /// 从翻页到 Done 之间画布只剩背景——大图几十毫秒，肉眼就是一闪。
    /// rot/flip 的归零只落在**目标**上：屏上那张旧图继续按它自己的朝向画着，
    /// 直到新图的解码结果 [`Self::install`] 原子替换。这里不重建旧图——原图档在
    /// 100% 档下重建一次要一百多毫秒，而这份像素马上就要被丢掉了（m72）。
    pub(super) fn reset_view(&mut self) {
        self.prefetch_stopped = false;
        self.want_rot = 0;
        self.want_flip = false;
        self.fit = true;
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
        self.error = None;
        // 清掉查询阶段预览：换图后旧尺寸不该再显示在 info bar（元数据缓存审计 2026-10-18）。
        self.preview_meta = None;
        // 新图从头计：原图档升级那道闩（`native_requested`）是按路径记的，
        // 复位视图就是"开始看另一张图"的唯一汇合点（open / navigate 都走这里）。
        self.native_requested = None;
        // 缩放重渲的档位账同样只属于当前那张（m189）。
        self.vector_zoom_req = None;
    }

    /// 放弃当前画面与清单，回到**空窗**（不留错误、不留提示）。
    ///
    /// 用户口径（2026-11-04）："关闭看图窗口后，看图窗口里面的图就清空。这样子我们
    /// 最小化后，再通过系统托盘打开窗口，不会出现原来的图。"
    ///
    /// 为什么必须真清而不能只标记"下次打开时再换图"：收进托盘后这份像素会一直钉在
    /// 常驻内存里（原图档顶格 160 MB），而用户已经明确表示"不看了"；更要紧的是
    /// **唤回来时屏幕上那张图会让人以为刚才什么都没发生**——状态与用户的动作用不上。
    ///
    /// 清的范围与 [`Self::fail`] 完全同源（同一份 `drop_current`），差别只有一条：
    /// 这里不留 `error`。清空是**用户主动收走窗口**的结果，不是"这张图坏了"。
    pub fn clear(&mut self) {
        self.drop_current();
        self.error = None;
    }

    /// 放弃当前画面与清单，进入"只有错误"的状态。
    ///
    /// 打开失败 / 目录被清空时走这里。不清 `gallery` 会留下旧标题与旧 `n/n`；
    /// 而晚到的旧图回包在"没有清单"时不做路径裁决（`apply` 那条判据会跳过），
    /// 于是它还能重新上屏。`cancel_pending()` 预留一个不会被后续请求复用的 seq，
    /// 把这些在途回包全部判成过期。
    pub(super) fn fail(&mut self, msg: String) {
        self.drop_current();
        self.error = Some(msg);
    }

    /// UI 侧把一条错误文本送到画面上（进入包内下一层失败时用）。**不动当前画面**：
    /// 进不去只是这一步没成，用户还在原处，清空清单等于把他赶出这个包。
    pub fn report_error(&mut self, msg: String) {
        self.error = Some(msg);
    }

    /// [`Self::fail`] 与 [`Self::clear`] 的共同主体：把"当前这张图"的一切账清干净。
    ///
    /// 两处唯一的差别由调用方补（一个落错误文本、一个不落），所以清账逻辑只写一遍——
    /// 漏掉其中任何一项都会留下"看得见却改不动"的残影：
    /// `gallery`（旧标题与 `n/n`）、`prefetched`（预读缓存几十 MB）、`thumbs`（缩略图条）、
    /// 在途 `want_seq`（晚到的回包会把旧图重新上屏）。
    pub(super) fn drop_current(&mut self) {
        self.gallery = None;
        self.loaded = None;
        self.prefetched.clear();
        self.prefetch_seq = None;
        self.prefetch_strip = None;
        self.thumbs.clear();
        // 复位视图（它会把 error 清成 None）——错误文本由调用方按需补。
        self.reset_view();
        self.anim_next_at = None;
        self.anim_due_at = None;
        self.step_target = None;
        self.anim_paused = false;
        self.drag = None;
        self.note = None;
        self.last_dir = 0;
        self.main_sent_at.set(None);
        self.stall_logged.set(false);
        self.wait_shown_secs.set(None);
        self.want_seq = self.decoder.cancel_pending();
        self.bump_image_gen();
    }
}
