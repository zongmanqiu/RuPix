//! 解码调度：`tick` 的主心跳、请求去抖、原图档升级、动画帧推进、结果落屏。
//!
//! 全程序只有**一条**解码线程，所以这里的每个 `send_decode` 都在抢占它：
//! 「先解用户在看的这张」优先于「补原图档」，后者又要给翻页让路
//! （判据 [`Viewer::upgrade_can_wait`](super::Viewer::upgrade_can_wait)）。
use super::transform::*;
use super::*;

impl Viewer {
    // ── 解码调度 ────────────────────────────────────────────────

    pub(super) fn mark_need(&mut self) {
        if self.need_at.is_none() {
            self.need_at = Some(Instant::now());
        }
    }

    /// 防抖到期后的调度：发起解码，或**本地换档重派生**。由 `App::on_interval` 定期调用。
    ///
    /// 返回**是否要重绘**：C 之后 `tick` 自己就能换掉上屏位图（本地派生），而本项目的
    /// 省电设计是"没人标脏就不合成"——不把这个信号送回 `runtime`，画面会停在旧位图上，
    /// 表现就是"缩放后明明解出来了却不刷新"。发起解码请求的那些分支不换图，返 `false`。
    ///
    /// ★ 返回 `bool` 是**不能吞**的：调用方必须据此标脏。故挂 `#[must_use]`——
    ///   上一版它就是 `()`，本地换档那条路径没人通知重绘，画面会停在旧位图上。
    #[must_use = "tick 的返回值是「画面变了、要重绘」，吞掉就等于解出来却不刷新"]
    pub fn tick(&mut self) -> bool {
        // 卡住自报（m71）放在**最前面**：下面每条提前返回都可能让这一帧什么都不做，
        // 而"等太久"恰恰只会发生在这些提前返回成立的时刻（need_at 为空 = 没别的活儿）。
        self.note_stalled_request();
        // 等待秒数是时间的函数，必须由这里标脏（见 `wait_shown_secs`）。**跳过一帧**
        // 的 tick_anim / 升级判定不要紧：每秒只发生一次，而卡住时本来就没活儿。
        let secs = self.pending_wait().map(|w| w.as_secs());
        if self.wait_shown_secs.get() != secs {
            self.wait_shown_secs.set(secs);
            return true;
        }
        self.tick_anim();
        let Some(need_at) = self.need_at else {
            return false;
        };
        if need_at.elapsed() < REDECODE_DEBOUNCE {
            return false;
        }
        let Some(g) = self.gallery.as_ref() else {
            self.need_at = None;
            return false;
        };
        let Some(path) = g.current() else {
            self.need_at = None;
            return false;
        };
        let (want_w, want_h) = self.desired_pixels();
        // ★ **按目标重画的矢量先于"本地换档"判据**（m189）：doc / doc-legacy 那 33 个
        //   自渲染矢量没有区域通道，解码器只能整幅按 `target` 重画。它们的
        //   `natural` 只是首次画布，手里这一档按倍率放大后本地插值就是位图糊面
        //   （真机：PDF 放大成马赛克，而 PDF 阅读器是清的）。所以倍率一变、手里
        //   那档不够用时，第一件事是请解码器按新目标重画，而不是本地派生。
        let rerender = self
            .loaded
            .as_ref()
            .filter(|l| l.path == *path && l.region.is_none())
            .filter(|l| is_target_vector(&l.path))
            .and_then(|l| self.zoom_rerender_target(l, (want_w, want_h)));
        if let Some(target) = rerender {
            self.need_at = None;
            // 目标档记账（防"解码器不理 target"时每轮重发）：落地不清，换图才清。
            self.vector_zoom_req = Some((path.clone(), target));
            self.want_seq = self.send_decode(path, target.0, target.1, 0, ReqKind::Main, false);
            return false;
        }
        // 先把"当前 loaded 是不是本图的整图、若是则目标档是哪一档"读清楚。
        // 单列一步是为了让不可变借用在这里就结束了：本地重派生要 `&mut self`，
        // 与跨过它的 `cur` 互斥。
        let whole_image = self
            .loaded
            .as_ref()
            .filter(|l| l.path == *path && !is_vector(&l.path))
            .filter(|l| l.decoded.0 >= l.natural.0 && l.decoded.1 >= l.natural.1)
            .map(|l| {
                (
                    l.displayed,
                    fit_dims(l.decoded.0, l.decoded.1, want_w, want_h),
                )
            });
        if let Some((displayed, target)) = whole_image {
            // **手里已是整图** → 缩放 / 改窗口只是换一档本地派生，绝不回头找解码器
            // （模块头「位图的两档」；实测：阶梯 30→50→80% 老规矩要整解 3 次 ≈1.94s/
            // 3.98s（TIFF/JPEG 冷缓存），本地派生是 1 次整解 + 2 次换档 ≈1.60s/2.30s）。
            self.need_at = None;
            let oriented = (self.rot, self.flip) == (self.want_rot, self.want_flip);
            if displayed == target && oriented {
                return false;
            }
            // 换档的成本按**源尺寸**计价（6055×6055 → 1200×900 实测净 0.118–0.131 s，
            // 全落在调用线程上），所以交给派生线程：这一帧继续画旧档，采好了由
            // [`Self::poll_levels`] 换图（M2）。起线程失败才退回同步派生——宁可在 UI
            // 线程上顿一下，也不许画面停在旧档不动。
            if !self.post_level(target) {
                self.commit_orientation();
                self.rebuild_image();
                self.bump_image_gen();
                return true;
            }
            return false;
        }
        // loaded 可能还是旧图（换图防闪不提前清，见 reset_view）：闸与帧号只对
        // 「已是当前路径」的那份成立——陈旧 loaded 直接跳过闸补解，绝不拿旧图的
        // 尺寸/动画帧去裁决新图。
        let cur = self.loaded.as_ref().filter(|l| l.path == *path);
        if let Some(l) = cur {
            if is_vector(&l.path) {
                // 矢量：没有"够清晰/到头"的闸——稳定后一律按可视区域 1:1 重渲。
                // 旧位图在 apply 前继续顶着显示（配合 30% 边距，平移不露底）。
                let region = self.region_req(l);
                self.need_at = None;
                // 区域重渲同样要占住"主画面"这一枚（m71）：不记发出时刻，状态栏就
                // 永远认为没有请求在等，卡死时还是那句"已载入"的假话。
                self.arm_main_wait();
                self.want_seq = self.decoder.request_region(path, region, ReqKind::Main);
                return false;
            }
            // 还不是整图（首解按视口档、或预读命中那一份）：要做的只有"**升级一次**
            // 原图档"，之后上面那条本地派生路就接管了。解码器不理 `target`，同一档
            // 请求两次 = 整解两次，所以这里用 `native_requested` 每图只发一次。
            if self.native_in_flight(path) {
                self.need_at = None;
                return false;
            }
        }
        // ★ **原图档升级让路给翻页**（m58）。走到这里说明手里已是"本图的某一档"，
        // 这次要发的是不限框的原图档整解——一次占住唯一那条解码线程 150–500 ms，
        // 而且**不可抢占**。用户还在连续翻页时，这张图已经不在屏上了：那次整解既救不了
        // 当前这页的首屏，还会把它挤到队尾（实测快翻臂命中率因此从 12/12 掉到 5/24）。
        // 推迟到用户停手；不清 `need_at`，下一次 poll（16 ms）再来判——判据便宜三个数量级。
        if let Some(l) = cur.as_ref() {
            if self.upgrade_can_wait(Instant::now(), l, want_w, want_h) {
                return false;
            }
        }
        self.need_at = None;
        // 升级要原图档（框 0 = 原始尺寸，解码器不再被缩到视口）；换图窗口期的首解没有
        // 本图尺寸可依据，仍按视口档先要一份上屏（`desired_pixels` 的 `loaded == None` 分支）。
        let upgrading = cur.is_some();
        let (box_w, box_h) = if upgrading { (0, 0) } else { (want_w, want_h) };
        // 帧号：同图续动画帧（缩放/窗口变化的重解不许把画面拽回第 0 帧），
        // 换图窗口期恒 0——与 request_decode 的换图首解同义。
        let frame = match cur {
            Some(l) if l.anim.delay_ms > 0 => l.anim.index,
            _ => 0,
        };
        // ◆ 升级要**真原图**——`box_w/h = 0` 已经让解码器不缩，这里再显式关掉降采样，
        //   免得将来看 target 的解码器把升级也缩了（m35）。
        self.want_seq = self.send_decode(path, box_w, box_h, frame, ReqKind::Main, false);
        if upgrading {
            self.native_requested = Some((path.clone(), self.want_seq));
        }
        // 只是发了请求，画面要到回包落地（`apply`）才换——那一头自己会标脏。
        false
    }

    /// 这一次原图档升级**能不能再等等**（m58）。两条都得成立：
    /// ① 距最近一次翻页不足 [`NAV_UPGRADE_GRACE`]——用户的手还在翻页，这一张已经不
    ///   在屏上了，而那次整解不可抢占、要占住唯一那条解码线程 150–500 ms；
    /// ② 手里这一档对**当前画面**已经够用（与预读命中同一条 `fit_dims × SHARP_ENOUGH`
    ///   判据）。②是刻意的：放大到视口档撑不住时它先失效，升级当场照发——不给
    ///   "翻页后立刻按 `1` 还得等宽限期"添新账。
    ///
    /// `now` 作参数进来（与 `tick_hide` 的光标锚点、`config` 的防抖时钟同一定式）：
    /// 时刻不可注入的判据没法写单测，而这条的两臂差别正好在毫秒级。
    pub(super) fn upgrade_can_wait(
        &self,
        now: Instant,
        l: &Loaded,
        want_w: u32,
        want_h: u32,
    ) -> bool {
        let still_flipping = self
            .last_nav_at
            .get()
            .is_some_and(|t| now.saturating_duration_since(t) < NAV_UPGRADE_GRACE);
        if !still_flipping {
            return false;
        }
        let (fit_w, fit_h) = fit_dims(l.natural.0, l.natural.1, want_w, want_h);
        l.decoded.0 as f32 >= fit_w as f32 * SHARP_ENOUGH
            && l.decoded.1 as f32 >= fit_h as f32 * SHARP_ENOUGH
    }

    /// 那道升级闩的读法：**"那一枚请求仍然是在途的那一枚"** 才算被闩住。
    ///
    /// ★ 只按路径记闩是不够的（旧写法就是这样）：升级请求会被后来的主画面请求
    /// （动画的下一帧、再按一次缩放）顶掉而永不落地，那时闩必须跟着失效，否则
    /// 这张图就永久停在视口档糊面上，翻页再翻回来才复位（2026-10-18 第四批审计）。
    pub(super) fn native_in_flight(&self, path: &Path) -> bool {
        self.native_requested
            .as_ref()
            .is_some_and(|(p, s)| p.as_path() == path && *s == self.want_seq)
    }

    /// 手里这一档够不够当前画面；不够就给出下一次要请的**整幅目标**（m189）。
    ///
    /// "够"沿用预读命中那条判据（[`SHARP_ENOUGH`]）：短 10% 以内不再整幅重画一次。
    /// `None` 的另外两种来源都是**防重发**：上一发要的档不亚于这次（解码器不理
    /// `target` 时画出来仍偏小，不靠这道账就会 16 ms 一轮地重发）；或者这一发此刻
    /// 就在途。用户真放大一档（目标超过上次）立刻照发。
    fn zoom_rerender_target(&self, l: &Loaded, want: (u32, u32)) -> Option<(u32, u32)> {
        let enough = l.decoded.0 as f32 >= want.0 as f32 * SHARP_ENOUGH
            && l.decoded.1 as f32 >= want.1 as f32 * SHARP_ENOUGH;
        if enough {
            return None;
        }
        if let Some((p, last)) = self.vector_zoom_req.as_ref() {
            if *p == l.path && last.0.max(last.1) >= want.0.max(want.1) {
                return None;
            }
        }
        Some(want)
    }

    pub(super) fn request_decode(&mut self) {
        let Some(g) = self.gallery.as_ref() else {
            return;
        };
        let Some(path) = g.current() else { return };
        // ★ 视口还没有真实尺寸（首帧 `paint` 之前是 (1,1)）就不发这一枚请求：按它解出来
        //   是一张 1×1 废图，白等一整轮（实测 1.2s+）。只标需求，`on_viewport` 拿到尺寸
        //   立刻补发。闸放在这一层而不是各调用点——`open` 与 `navigate` 都要这条判据，
        //   一份规则写两处迟早分叉（m101 之前 `navigate` 就没有这道闸）。
        if self.view.2 <= 1.0 || self.view.3 <= 1.0 {
            self.mark_need();
            return;
        }
        let (w, h) = self.desired_pixels();
        self.need_at = None;
        // 换图首解恒第 0 帧：此刻 loaded 还是旧图（防闪不提前清，见 reset_view），
        // 拿旧图的动画帧号去解新图——播放中翻 GIF 页可能以中帧起解甚至越界。
        // 同图重解（缩放/窗口变化）走 tick，帧号在那儿按「已是当前图才续帧」取。
        // ◆ 首屏要**快**：点亮原生降采样，让解码器按视口那一框直接解出档位，
        //   省掉"整解一遍再缩到视口"里那次完整整解（JPEG 实测 1.5× 上下，m35）。
        //   清晰度不受损：`natural` 仍是原图尺寸，`tick` 照旧会补一次原图档升级。
        self.want_seq = self.send_decode(path, w, h, 0, ReqKind::Main, true);
    }

    /// 一发主画面请求出门：重置"等了多久"的时钟与"已报卡住"位（m71）。
    ///
    /// 这两件事必须**成对**做：`stall_logged` 是"这一枚请求已经报过"，跟着时钟走，
    /// 否则下一张图一卡住就再也不响（或者同一枚请求每帧写一条）。
    pub(super) fn arm_main_wait(&self) {
        self.main_sent_at.set(Some(Instant::now()));
        self.stall_logged.set(false);
    }

    /// 按扩展名选择解码入口：裸数据走 `request_raw`（参数窗已把参数写进会话
    /// store），其余走 `request_frame`。`kind` 由调用方决定主画面还是预读；
    /// 预读允许被新主画面作废（见 [`ReqKind`]）。
    ///
    /// `allow_scaled` = 这一次要**原生降采样**（JPEG 走 DCT 1/2\|1/4\|1/8）。只有首屏
    /// 那一次（换图首解、预读）点它——清晰度由随后那次 `(0, 0)` 原图档升级补上，
    /// [`Decoded::natural`] 仍是原图尺寸，所以升级闸照旧会响（m35）。
    pub(super) fn send_decode(
        &self,
        path: &Path,
        w: u32,
        h: u32,
        frame: u32,
        kind: ReqKind,
        allow_scaled: bool,
    ) -> u64 {
        // 埋点（m34）：记下主画面这一枚的发出时刻，`apply` 落地时算端到端耗时；
        // m71 起它还兼作"等太久"的时钟（见 [`Self::pending_wait`]）。
        if kind == ReqKind::Main {
            self.arm_main_wait();
        }
        // 裸像素（.rgb/.yuv 等）没有头：首次请求前先按后缀 + 文件长度自动推断，
        // 这样从命令行、文件对话框、目录首图、翻页/预读进来的路径都走同一条自动参数路。
        // 相机 RAW 的推断返回 None，仍走普通解码。
        if crate::core::decode::get_raw_params(path).is_none() {
            if let Some(params) = crate::core::decode::infer_raw_params(path) {
                crate::core::decode::set_raw_params(path, params);
            }
        }
        // 参数化裸数据只能走 `request_raw`；以前这里漏了这一支，参数窗写进 store
        // 却永远没人读——表现就是"参数填了仍然不识别"。
        if crate::core::decode::get_raw_params(path).is_some() {
            return self.decoder.request_raw(path, w, h, kind);
        }
        // 相机 RAW 有文件头，走普通解码；裸像素的普通解码只作为未推断出参数时
        // 的兜底（例如未知布局）。
        // ★ 页与帧合流（2026-11-02）：只有这一个号，多页文档的页号就是帧号，
        //   解码器各取所需（GIF 看 frame_index、PDF/TIFF 看 page_index，见 `Anim`）。
        if allow_scaled {
            self.decoder.request_frame_scaled(path, w, h, frame, kind)
        } else {
            self.decoder.request_frame(path, w, h, frame, kind)
        }
    }

    /// 动画推进：到期就请求下一帧。**只消费** [`Self::install`] 排下的时刻。
    pub(super) fn tick_anim(&mut self) {
        if self.anim_paused {
            return;
        }
        let Some(at) = self.anim_next_at else { return };
        if Instant::now() < at {
            return;
        }
        self.anim_next_at = None;
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        if l.anim.delay_ms == 0 {
            return;
        }
        // count 未知（GIF 惰性数帧）时只 +1，解码器侧 EOF 数清后自会回填并回绕；
        // 已知则直接取模回绕，保证循环播放。
        let next = if l.anim.count > 0 {
            (l.anim.index + 1) % l.anim.count
        } else {
            l.anim.index + 1
        };
        let Some(g) = self.gallery.as_ref() else {
            return;
        };
        let Some(path) = g.current() else { return };
        let (want_w, want_h) = self.desired_pixels();
        self.want_seq = self.send_decode(path, want_w, want_h, next, ReqKind::Main, false);
    }

    /// P 键：暂停/继续动画。
    ///
    /// ★ 静图上按 P **不许**把 `anim_paused` 翻起来：那会挂出一个假暂停态，还会开
    ///   "暂停才收帧"的环形缓存（[`Self::set_anim_paused`]），而屏幕上什么都不会变——
    ///   用户只能反复按。改成给一句提示（m50-C）。
    pub fn toggle_anim_pause(&mut self) {
        if !self.is_animated() {
            self.set_note(NOTE_STILL);
            return;
        }
        let on = !self.anim_paused;
        self.set_anim_paused(on);
    }

    /// 挂一句一次性提示：窗口段取走后走顶部消息窗，状态格不受影响（见 `note` 字段）。
    pub(super) fn set_note(&mut self, s: &'static str) {
        self.note = Some((s, Instant::now()));
    }

    /// 取走一次性提示（取走即清空，同一句只弹一次）。
    ///
    /// ★ 窗口段每 16 ms 跑一次，不清空就会每拍弹一条消息窗；过期的一句直接作废，
    ///   它说的是很久以前那一下按键。由 `runtime::window_tick` 交给 `ctx.toast`。
    pub fn take_note(&mut self) -> Option<&'static str> {
        let (s, at) = self.note.take()?;
        (at.elapsed() < NOTE_TTL).then_some(s)
    }

    /// 设暂停态。**它同时是逐帧环形缓存的开关**：进暂停才开始收帧，出暂停立刻整份
    /// 释放——播放中的 GIF 不该为"也许要逐帧"多占内存。
    pub fn set_anim_paused(&mut self, on: bool) {
        if self.anim_paused == on {
            return;
        }
        self.anim_paused = on;
        if on {
            self.anim_next_at = None;
            // m75：时间线随闸一起作废，下一帧重新起表（不续旧账）。
            self.anim_due_at = None;
        } else {
            // 恢复时重新排一次延时。不重排的话：暂停期间 `anim_next_at` 早已过期，
            // 再按 P 的那一帧 `tick_anim` 立刻判定"到期"并跳帧——"继续"看着像
            // "先闪一帧再继续"。夹延时的那道上限在 `anim_wait` 里，两条排表路共用。
            let wait = self
                .loaded
                .as_ref()
                .filter(|l| l.anim.delay_ms > 0)
                .map(|l| anim_wait(l.anim.delay_ms));
            if let Some(wait) = wait {
                let due = Instant::now() + wait;
                self.anim_next_at = Some(due);
                // 恢复时也要把时间线立起来：只排闸不排线的话，下一帧 install 找不到
                // 基准，整段播放就退回"落地 + 延时"那一套慢放。
                self.anim_due_at = Some(due);
            }
        }
    }

    /// 逐帧浏览：`dir` = ±1。每步都发一次按号请求，画面在回包里换，所以本函数不返回
    /// 「要不要重绘」——过去它返回一个恒为 `false` 的 bool，被快捷键层当成「这个键
    /// 消费了没」透传给框架，于是画布报"没消费"、同一次按键接着走 `App::on_shortcut`
    /// 第二遍（真机实测：按一下退两帧，2026-10-18）。
    ///
    /// 按下即自动进暂停态（不暂停的话，手动退一步会被播放调度立刻拽回去）。后退
    /// 优先吃环形缓存；未命中只能付一次「重建 + 快进」——那是 GIF 格式的性质
    /// （disposal 合成串行），实测数字与取舍见 本仓历史记录。
    pub fn step_anim(&mut self, dir: i32) {
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        if !l.anim.is_animated() {
            // ★ 静图按 `,` `.`：给一句人话，不要无声返回（m50-C——"点了没反应"最容易
            //   被当成程序卡了）。这里直接赋值而不走 `set_note`：`l` 还借着
            //   `self.loaded`，方法调用要 `&mut self` 会撞借用，而写另一个字段是允许的。
            self.note = Some((NOTE_STILL, Instant::now()));
            return;
        }
        // 屏幕上这份必须就是清单当前项。换图窗口期 `loaded` 故意还停在旧图（防闪，
        // 见 [`Self::reset_view`]），而旧图的动画按钮还画着、`,` `.` 也还绑着——这一刻
        // 按下去就是替**上一张**要帧：seq 比新图的首解更大，把新图顶掉后冻在旧图上。
        if !self.screen_is_current() {
            return;
        }
        let (index, count) = (l.anim.index, l.anim.count);
        let path = l.path.clone();
        // ★ 基准是"在途目标"而不是"屏幕上这一帧"：后退一步可能要付一次重建 + 快进，
        // 那段时间屏幕上还是旧帧，用旧帧算就连按无效（真机实测按 91 下只退 1 帧）。
        let base = self.step_target.unwrap_or(index);
        // 帧数未知（畸形文件数不出总数）：前进交给解码器 EOF 回填并回绕，后退夹在 0——
        // 不编一个假总数。
        let target = if count > 0 {
            (base as i64 + dir as i64).rem_euclid(count as i64) as u32
        } else if dir > 0 {
            base + 1
        } else {
            base.saturating_sub(1)
        };
        if target == index && self.step_target.is_none() {
            // 没地方可去（第 0 帧再退、单帧动图）。但**有在途目标时不能走这条**：
            // 用户按回来了当前位置，得发一次请求把在途那枚旧目标顶掉，否则它稍后落地，
            // 画面自己走到用户已经取消的地方。
            return;
        }
        self.set_anim_paused(true);
        self.step_target = Some(target);
        // 后退不再由宿主缓存帧：快进途中被跳过的帧只有解码器见过（它合成完就丢了），
        // 所以环放在 `core` 解码器的动画会话里——实测连退 20 步 3.8 s → 0.22 s。
        // 这里每步都规规矩矩发一次解码请求。
        let (w, h) = self.desired_pixels();
        self.want_seq = self.send_decode(&path, w, h, target, ReqKind::Main, false);
    }

    /// 屏幕上那份 `loaded` 是不是清单当前项。换图窗口期（旧图还顶着显示、新图在解）
    /// 为 `false`——凡是"拿旧图的属性替新图做决定"的路径都得先问这一句。
    ///
    /// 没有清单（`gallery` 为空 / 打开失败）时算 `true`：那时不存在"该显示谁"这回事，
    /// 不该由这条判据再吞掉一次操作（合成回包的单测走的正是这条路）。
    pub(super) fn screen_is_current(&self) -> bool {
        match (
            self.loaded.as_ref(),
            self.gallery.as_ref().and_then(|g| g.current()),
        ) {
            (Some(l), Some(p)) => l.path == *p,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// 当前上屏的是不是动画。浮条上那三颗动画按钮的显隐就看它（与 `anim_paused`
    /// 一起构成按钮的两级状态：出现 / 字形）。
    pub fn is_animated(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.anim.is_animated())
    }

    /// 当前文件**有没有"第几张"可切**（按钮栏要不要挂那一组 + 状态行写不写进度）。
    ///
    /// ★ 与 [`Viewer::is_animated`] 分开：静态位图 / 单张 SVG 返回假（不给 `1/1`、
    ///   不给逐帧按钮），只有真动图与多页文档返回真；判据见
    ///   `core::pixels::Anim::has_index`。
    pub fn has_index(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.anim.has_index())
    }

    /// 动画是否处于暂停态（状态栏显示用）。
    pub fn anim_paused(&self) -> bool {
        self.anim_paused
    }

    /// 视口那一框的**物理**像素尺寸：预读请求要的就是这一档（解码器把图缩到装得下它）。
    ///
    /// ★ 与 [`Self::desired_pixels`] 不是一回事，别混用：后者按"当前这张图的 natural ×
    /// 倍率"算，而换图窗口期 `loaded` 故意还停在**上一张**（防闪，见 [`Self::reset_view`]），
    /// 拿它当预读的"够不够清晰"判据，等于用上一张的长宽比裁决这一张——目录里横幅紧挨
    /// 竖图时，刚解好的预读会被整份丢掉重解（2026-10-18 第四批审计）。
    pub(super) fn viewport_pixels(&self) -> (u32, u32) {
        let dpi = self.dpi.max(0.5);
        (
            (self.view.2 * dpi).ceil().max(1.0) as u32,
            (self.view.3 * dpi).ceil().max(1.0) as u32,
        )
    }

    /// 上屏需要的像素尺寸（物理像素）。
    ///
    /// 原图尺寸未知时（首次解码）直接按视口物理尺寸要：解码器解码器会把图等比缩到装得下这个框，
    /// 结果恰好就是"适应窗口"该有的尺寸，无需先知道原图多大。
    ///
    /// ★ `loaded` 只在**它就是当前这一张**时才能参与计算。换图窗口期 `loaded` 故意还停在
    ///   上一张（防闪，见 [`Self::reset_view`]），那时 `natural × 倍率` 是**上一张**的显示
    ///   尺寸，拿来当这一张的框等于让上一张的长宽比裁决这一张：实测 748×561 的窗口里，
    ///   紧跟在 4000×6000 后面的 6400×3600 只解到 374×210（本该 748×421），首屏白糊一倍，
    ///   还把已经预读好的那份按比例不同的对比判据一起带偏（与 [`Self::viewport_pixels`]
    ///   文档里 2026-10-18 那条同源，那条只修了判据，这条修请求尺寸）。
    pub(super) fn desired_pixels(&self) -> (u32, u32) {
        let (vw, vh) = (self.view.2, self.view.3);
        let dpi = self.dpi.max(0.5);
        let current = self.gallery.as_ref().and_then(|g| g.current());
        let live = self.loaded.as_ref().filter(|l| match current {
            // 清单里没有当前项 = 合成上下文（单测直接 `apply` 就是这么进的），维持老算法。
            Some(p) => *p == l.path,
            None => true,
        });
        // ★ 按目标重画的矢量（m189）：`natural` 只是**首次渲染的画布**，上限不能夹在
        //   它上面——目标 = natural × 倍率 × dpi（总量按 [`MAX_PIXELS`] 夹）。夹在
        //   natural 上等于"放大只做本地重采样"，与位图同一条糊路。
        if let Some(l) = live {
            if l.region.is_none() && is_target_vector(&l.path) {
                let s = self.scale_in(self.want_rot) * dpi;
                return vector_target_pixels(l.natural, s);
            }
        }
        let (dw, dh, cap_w, cap_h) = match live {
            None => (vw, vh, vw, vh),
            Some(l) => {
                // **目标**朝向：旋转在途时这一档要按换轴后的 fit 倍率采，
                // 否则落地那一刻还要再换一档（多烧一次几十毫秒）。
                let s = self.scale_in(self.want_rot);
                (
                    l.natural.0 as f32 * s,
                    l.natural.1 as f32 * s,
                    l.natural.0 as f32,
                    l.natural.1 as f32,
                )
            }
        };
        // ★ **先夹到原生、再转整数**。`natural × 64 倍 × dpi` 在长条图上（1×40 MP 那种过了
        // [`MAX_PIXELS`] 总量闸、但单边没有闸）算得到 5.1e9，而 `f32 as u32` 是**饱和**转换——
        // 于是递到解码器 ABI 上的那句话变成"给我 42 亿像素宽"。实测本解码器集不理 `target_w/h`，
        // 所以今天无感；但一个**照契约办事**的第三方解码器按它分配就是 OOM，罚错了人。
        // 夹住不改任何行为：解码要不出比原图更多的像素，[`fit_dims`](crate::core::geometry::fit_dims)
        // 也从不放大（那里有现成的单测）。
        (
            (dw * dpi).ceil().clamp(1.0, cap_w.max(1.0)) as u32,
            (dh * dpi).ceil().clamp(1.0, cap_h.max(1.0)) as u32,
        )
    }

    /// 收解码结果。**过期结果（`seq` 不等于当前期待的）一律丢弃。**
    pub fn apply(&mut self, done: Done) -> bool {
        // 预读回包：入缓存即可，与上屏无关。
        if Some(done.seq) == self.prefetch_seq {
            self.prefetch_seq = None;
            if let Ok(d) = done.out {
                let evicted = self.cache_prefetch(done.path, d);
                // 淘汰后字节数可能又低于预算，立即续填会反复重解被淘汰的邻图。
                if !evicted {
                    self.request_prefetch();
                }
            }
            return false;
        }
        // 缩略图回包：与当前画面无关，入缓存即可。返回 true 让 UI 重绘（那一格
        // 该填上了）。**必须在下面的"过期丢弃"判据之前**——缩略图的 seq 天然
        // 不等于 `want_seq`，放后面就永远被当成过期结果扔了。
        if self.thumbs.wants(done.seq) {
            return self.thumbs.take(done);
        }
        if done.seq != self.want_seq {
            log::debug!(
                "丢弃过期解码结果：seq={} want={} path={}",
                done.seq,
                self.want_seq,
                done.path.display()
            );
            return false;
        }
        // ★ **光看 seq 不够，还得看是不是当前这一张。** `install` 不推进 `want_seq`
        // （预读命中那条路压根没发请求），于是"上一张的原图档升级回包"在翻页之后
        // 依然 seq 相符——晚到一步就把刚翻到的新图盖掉，画面停在上一张、状态栏却是
        // 新文件名，要等 120 ms 防抖 + 一次重解才自己爬回来（2026-10-18 第四批审计）。
        // 只在**清单有当前项**时裁决：没有清单就没有"该显示谁"这回事（合成回包的
        // 单测走的正是那条路）。
        if let Some(cur) = self.gallery.as_ref().and_then(|g| g.current()) {
            if cur != &done.path {
                log::debug!(
                    "丢弃非当前图的回包：path={} 当前={}",
                    done.path.display(),
                    cur.display()
                );
                return false;
            }
        }
        match done.out {
            Ok(d) => {
                // 埋点（m34）：端到端 = 发出主画面请求 → 结果落地。
                // 与 `decoders` 侧的 `[decode-trace]`（read/probe/query/render/post 五段）
                // 互补：那段只量解码内部，这一段还含"在串行解码线程里排队"的等待——
                // 用户感知的延迟恰恰是两者之和。
                //
                // ★ 这一行是 **info**（m59）：它是"用户到底看了哪张、等了多久、哪一档"
                // 的唯一记录，默认档必须留得下来。两条腿共用一个 `[main-latency]` 标签，
                // 用 `src=` 分家 ⇒ "数一数标签行数 = 上屏次数，数 `src=prefetch` = 命中次数"，
                // 判命中率不再需要把日志点亮到 debug（m38/m58 那两轮都为此重跑过取证）。
                if let Some(t0) = self.main_sent_at.take() {
                    log::info!(
                        "[main-latency] file={} src=decode cost={:.1}ms bitmap={}x{} natural={}x{}",
                        done.path.display(),
                        t0.elapsed().as_secs_f64() * 1000.0,
                        d.bitmap.width,
                        d.bitmap.height,
                        d.natural.0,
                        d.natural.1
                    );
                }
                self.install(done.path, d);
                // 当前图就位，正好把解码线程空出来，预读相邻图。
                self.request_prefetch();
                self.bump_image_gen();
                true
            }
            Err(msg) => {
                // ★ 失败回包分两种，判据是"手里有没有这张图"：
                //   首解失败 → 空白 + 中央错误（现状，诚实：这张图确实没得看）；
                //   升级/动画帧失败 → 好图已经在屏上，像素本身仍然有效，不能为了
                //   一次没拿到更清晰档就把用户正在看的画面整屏夺走。
                //   旧代码无条件清 `loaded`，升级失败时画面会突然变空白 + 报错（m91 审计 D1）。
                let keep = self.loaded.as_ref().is_some_and(|l| l.path == done.path);
                // 失败的升级必须松开闩：`native_in_flight` 只认"仍在途的那一枚"，
                // 不清它这张图就永远停在低清档，用户再缩放也不会重试。
                self.native_requested = None;
                // 缩放重渲失败同理：不清账，同一倍率下再也不会有第二次请求（m189）。
                self.vector_zoom_req = None;
                // 渲染失败 = 同 seq 的预览不再可信，一并清掉。
                self.preview_meta = None;
                self.anim_next_at = None;
                // m75：时间线随闸一起作废，下一帧重新起表（不续旧账）。
                self.anim_due_at = None;
                // 请求已经落地（失败也是落地）：等待时钟与"已报卡住"位一起复位，
                // 免得下一次成功上屏还带着上一枚的 warn 记录（m71）。
                self.main_sent_at.set(None);
                self.stall_logged.set(false);
                if keep {
                    // 没有可见变化（动画停下也不改当前帧），所以不标脏、不 bump 世代。
                    log::warn!(
                        "升级/动画帧解码失败，保留已上屏画面：{}（{msg}）",
                        done.path.display()
                    );
                    return false;
                }
                self.loaded = None;
                self.error = Some(msg);
                self.bump_image_gen();
                true
            }
        }
    }

    /// 把查询/渲染阶段的尺寸记进有界缓存。
    pub(super) fn cache_meta(&mut self, path: PathBuf, meta: Meta) {
        if self.meta_cache.len() >= META_CACHE_MAX && !self.meta_cache.contains_key(&path) {
            self.meta_cache.clear();
        }
        self.meta_cache.insert(path, meta);
    }

    /// 收查询阶段的元信息（预览）。**过期/预读/缩略图的 Meta 一律丢弃。**
    ///
    /// 与 [`Self::apply`] 的分工：Frame 决定画布与权威尺寸，Meta 只在 Frame 还没到
    /// 时给 info bar 和「按原始像素开窗」提前一个可用值。返回值 = 是否要重绘。
    pub fn apply_meta(&mut self, ev: MetaEvent) -> bool {
        // 只接受当前主画面的 seq。预读、缩略图、翻页前的过期请求都无关。
        if ev.seq != self.want_seq {
            return false;
        }
        // 已经有位图 = 权威尺寸就位，预览不再有意义。
        if self.loaded.is_some() {
            return false;
        }
        self.preview_meta = Some(ev.meta);
        // 缓存查询阶段元信息：翻回这张图时不必等 MetaEvent。
        self.cache_meta(ev.path, ev.meta);
        // 让 `refresh_metadata`（每帧比对 `image_gen`）感知"文件信息"窗的尺寸
        // 也可以刷新了；canvas 上的 `v.status()` 本来就走每帧重绘，不依赖这个 bump。
        self.bump_image_gen();
        true
    }
}

/// "按目标重画"的矢量在这一倍率下的目标像素（m189）。
///
/// 用 f64 算再夹：`natural × 64 倍 × dpi` 在长条图上能到 5e9，而 `f32 as u32` 是
/// **饱和**转换（与 [`Viewer::desired_pixels`] 老注释同一条坑）；总量按
/// [`MAX_PIXELS`] 等比缩，一次重渲的缓冲不超过全局上限。
fn vector_target_pixels(natural: (u32, u32), scale: f32) -> (u32, u32) {
    let (w, h) = (
        (natural.0 as f64 * scale as f64).ceil().max(1.0),
        (natural.1 as f64 * scale as f64).ceil().max(1.0),
    );
    let px = w * h;
    let cap = MAX_PIXELS as f64;
    if px > cap {
        let k = (cap / px).sqrt();
        return ((w * k).max(1.0) as u32, (h * k).max(1.0) as u32);
    }
    (w as u32, h as u32)
}
