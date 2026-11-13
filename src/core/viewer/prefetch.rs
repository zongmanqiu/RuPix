//! 后台「图片预读」：邻图 ±N + 缩略图条可见的那一屏；什么条件算「这份够用」、两条闸（条数 / 字节）。
//!
//! ★ m265：下注集合不再只由当前索引推。条是独立滚动列表（m260），用户点格子能切到的图
//!   全在**条上那一屏**里，这一屏可能离当前图几百张远。所以两种切图手势各有来源：
//!   键盘 / 浮条连翻 → 方向邻居；鼠标点格子远跳 → 条可见范围。两者合并、去重、按份数封顶。
//!
//! 判据要点：混尺寸目录里"框的两条边都比"会永远驳回（fit 从不裁切），必须按
//! 装进框后的**实际覆盖**判——m38 真机实测 12 次翻页有 10 次白解，就是这条写错。
use super::transform::is_target_vector;
use super::*;

impl Viewer {
    // ── 后台预读相邻图 ─────────────────────────────────────────

    /// 预读偏移表的**优先级顺序**（先算的先预读）。
    ///
    /// 同方向给满 `Config::prefetch_depth` 格、反方向只留 1 格：连续翻页的方向是可预测的
    /// （见 [`Self::last_dir`]），而反方向留一格是为了「A→B→A」来回翻仍然命中——
    /// 原来那份在翻到 B 时就被消费掉了。方向未定时退回**对称交错**（`+1,-1,+2,-2`），
    /// 两边都不赌。
    pub(super) fn prefetch_offsets(&self) -> Vec<isize> {
        let d = self.config.borrow().prefetch_depth as isize;
        if self.last_dir > 0 {
            (1..=d).chain([-1]).collect()
        } else if self.last_dir < 0 {
            (-d..=-1).rev().chain([1]).collect()
        } else {
            (1..=d).flat_map(|k| [k, -k]).collect()
        }
    }

    /// 本轮「图片预读」的下注集合：按优先序、去重、有界，返回**清单索引**。
    ///
    /// 两条来源对应两种切图手势：
    /// ① **邻图**：[`Self::prefetch_offsets`] 的方向感知 ±N —— 键盘 / 浮条连续翻页；
    /// ② **缩略图条可见的那一屏** —— 鼠标点格子。m260 之后条是独立滚动列表，
    ///    当前图可以完全不在视野里，这一屏没法由「当前索引 ±k」推出来。
    ///
    /// 条内按「离当前图由近到远」排：一屏里每格都是等价的点击目标，先落近的；
    /// 合并后截到 [`PREFETCH_CAP`]，所以窗口再宽、目录再大，下注集合也不长。
    pub(super) fn prefetch_targets(&self) -> Vec<usize> {
        let Some(g) = self.gallery.as_ref() else {
            return Vec::new();
        };
        let n = g.len();
        if n <= 1 {
            return Vec::new();
        }
        let cur = g.position().saturating_sub(1);
        let mut out: Vec<usize> = Vec::with_capacity(PREFETCH_CAP);
        {
            // 当前图自己不必预读（它已经在屏上 / 正在解）。
            let mut push = |idx: usize| {
                if idx != cur && idx < n && !out.contains(&idx) {
                    out.push(idx);
                }
            };
            for off in self.prefetch_offsets() {
                push((cur as isize + off).rem_euclid(n as isize) as usize);
            }
            if let Some((first, count)) = self.prefetch_strip {
                let mut strip: Vec<usize> = (first..first.saturating_add(count))
                    .filter(|&i| i < n)
                    .collect();
                strip.sort_by_key(|&i| i.abs_diff(cur));
                for i in strip {
                    push(i);
                }
            }
        }
        out.truncate(PREFETCH_CAP);
        out
    }

    /// 缩略图条这一屏的索引窗口（由 [`Viewer::strip_window`] 每帧喂进来）。
    ///
    /// `n == 0` 表示条关着 / 窗口放不下：`prefetch_targets` 自动退回纯邻图，功能不退步。
    /// 范围变了就是**新一轮下注**：旧一屏的位图不再有人会点，留着只会占住份数闸，
    /// 入缓存时一淘汰就把 `prefetch_stopped` 置上（表现：滚两下连邻图都不预读了）。
    pub fn set_prefetch_strip(&mut self, first: usize, n: usize) {
        let now = (n > 0).then_some((first, n));
        if self.prefetch_strip == now {
            return;
        }
        self.prefetch_strip = now;
        let keep: Vec<PathBuf> = self
            .prefetch_targets()
            .iter()
            .filter_map(|&i| self.gallery.as_ref().and_then(|g| g.peek(i)).cloned())
            .collect();
        self.prefetched.retain(|e| keep.contains(&e.path));
        self.prefetch_stopped = false;
        self.request_prefetch();
    }

    /// 为下注集合里还缺的那一图发起后台预读（同一时刻至多一个在途请求）。
    ///
    /// 解码线程是串行的：预读请求排在当前解码之后，只在空闲时消耗 CPU，
    /// 不与正在看的图抢路。SVG 同样预读（整图路线，作首帧；翻到它后
    /// `tick` 会按可视区域 1:1 重渲接管清晰度）。
    ///
    /// ★ **方向感知**（m34）：偏移表按 [`Self::last_dir`] 排——刚往前翻，就把前面
    /// 两张排在前面两张之前。同方向开 `Config::prefetch_depth` 格、反方向留 1 格：
    /// 连续翻页的方向是可预测的，而反方向留一格是为了「A→B→A」这种来回翻
    /// 仍能命中（原来那份在翻到 B 时就被消费掉了）。
    /// ★ **条范围并入**（m265）：候选不再只有 `cur ± k`。用户快速点击切换时，
    /// 点到的必然是条上那一屏里的某一格；把这一屏并进下注集合，远跳也能零解码。
    pub(super) fn request_prefetch(&mut self) {
        if self.prefetch_stopped || self.prefetch_seq.is_some() {
            return;
        }
        // 用户关掉了「图片预读」（ini `prefetch=0`）：一台安静的机器也是需求。
        // 关的是**发起**这一侧——已在途的那一枚照常回来、已缓存的那些照常命中，
        // 不做任何"顺手清空"，免得改一个开关把用户正看着的东西抖一下。
        if !self.config.borrow().prefetch {
            return;
        }
        let Some(g) = self.gallery.as_ref() else {
            return;
        };
        if g.len() <= 1 || self.view.2 <= 1.0 || self.view.3 <= 1.0 {
            return;
        }
        // 单份已满预算时不再投递；入缓存发生淘汰则由回包侧停止自动续填。
        if !self.prefetched.is_empty() && self.prefetch_bytes() >= PREFETCH_BYTES {
            return;
        }
        // 与 [`Self::use_prefetched`] 同一框：预读要的就是视口档，命中判定才有意义。
        let (bw, bh) = self.viewport_pixels();
        let plan = self.prefetch_targets();
        let cur = g.current().cloned();
        // 「计划内都齐了」按**路径**数，不按缓存长度：条滚走之后缓存里可能还留着
        // 上一屏的位图，拿长度当就绪数会以为已经齐了、从此不再发新一屏的预读。
        let ready = plan
            .iter()
            .filter(|&&i| {
                g.peek(i)
                    .is_some_and(|p| self.prefetched.iter().any(|e| &e.path == p))
            })
            .count();
        if ready >= plan.len() {
            return;
        }
        for idx in plan {
            let Some(p) = g.peek(idx) else { continue };
            if Some(p) == cur.as_ref() || self.prefetched.iter().any(|e| e.path == *p) {
                continue;
            }
            let p = p.clone();
            // ◆ 预读同样点亮降采样：它是"下一次翻页的首屏"，与主画面首解同一口径。
            //   视口那一框本来就够用，省下的解码时间正好让它在用户翻到之前就绪（m35）。
            self.prefetch_seq = Some(self.send_decode(&p, bw, bh, 0, ReqKind::Prefetch, true));
            break;
        }
    }

    /// 翻页后试着用预读缓存直接上屏（零解码延迟）。返回是否命中。
    pub(super) fn use_prefetched(&mut self) -> bool {
        let Some(path) = self.gallery.as_ref().and_then(|g| g.current().cloned()) else {
            return false;
        };
        let Some(i) = self.prefetched.iter().position(|e| e.path == path) else {
            return false;
        };
        let e = self.prefetched.remove(i);
        // 够用才吃：拿它和**这张图此刻按视口框解出来会有多大**比，而不是和框的两条边比。
        // ★ 后者是错的（2026-10-19 实测）：fit 从不裁切，装进框的结果永远只有一条边贴住框，
        //   两条边都要求 ≥90% 等于要求"图片长宽比 == 窗口长宽比"。6 张混尺寸目录翻 12 次，
        //   10 次是缓存里明明有这一张、被判据驳回重解（0.36 s / 1.0 s / 2.5 s 三种间隔读数
        //   一致，说明瓶颈不是预读深度）——预读等于白做，只有那张 4:3 恰好撞上 4:3 的窗口才活下来。
        //   判据继续用 [`Self::viewport_pixels`] 而不是 `desired_pixels`：那一刻 `loaded`
        //   还停在上一张（见其文档），换轴就回到老问题。
        let (want_w, want_h) = self.viewport_pixels();
        let (fit_w, fit_h) = fit_dims(e.img.natural.0, e.img.natural.1, want_w, want_h);
        let enough = e.img.bitmap.width as f32 >= fit_w as f32 * SHARP_ENOUGH
            && e.img.bitmap.height as f32 >= fit_h as f32 * SHARP_ENOUGH;
        let at_native = e.img.bitmap.width >= e.img.natural.0;
        if !(enough || at_native) {
            log::debug!(
                "预读位图过小（视口已变），丢弃重解：{} {}×{} vs {}×{}",
                path.display(),
                e.img.bitmap.width,
                e.img.bitmap.height,
                fit_w,
                fit_h
            );
            return false;
        }
        // 埋点（m34）：命中就是"零解码上屏"，这一行是可数的——翻一轮下来数一数
        // 有几条，就知道预读窗口到底接住了多少张。与解码那条腿**同标签**（见 `apply`）。
        log::info!(
            "[main-latency] file={} src=prefetch bitmap={}x{} natural={}x{}",
            path.display(),
            e.img.bitmap.width,
            e.img.bitmap.height,
            e.img.natural.0,
            e.img.natural.1
        );
        self.install(path, e.img)
    }

    /// 预读结果入缓存（同路径覆盖，超限淘汰最旧的）。
    ///
    /// m265 之后下注集合最多 [`PREFETCH_CAP`] 张，条数闸只在「旧一屏的位图还没
    /// 换完血」这种过渡态才会触发；真正封驻留的仍是字节闸。
    ///
    /// 尺寸账（2026-10-17 复算，见 `docs/踩坑记录/踩坑记录.md` K45）：这里存的是**视口档**，不是原图档——
    /// [`Self::request_prefetch`] 用的框是 `视口逻辑尺寸 × dpi`，解码器出来就经 `fit_to` 收到该框内，
    /// 所以一份预读 ≈ 视口像素 × 4 B（4K 满屏约 31.6 MiB）。
    ///
    /// **两条闸**（m34）：条数 [`PREFETCH_CAP`] + 字节 [`PREFETCH_BYTES`]。
    /// 加字节闸是因为窗口从 ±1 扩到 ±N（ini `prefetch_depth`）之后，"条数封顶"不再等价于
    /// "内存封顶"——4K 与 5K 屏上一份差近一倍，得让驻留有确定的字节上界。
    ///
    /// ★ `len() > 1` 这个前置条件不能省：单份就超预算时若也淘汰，缓存会被清空、
    /// 预读从此再也进不来——那条闸就成了"关掉预读"而不是"限制预读"。
    pub(super) fn cache_prefetch(&mut self, path: PathBuf, img: Decoded) -> bool {
        self.prefetched.retain(|e| e.path != path);
        self.prefetched.push(Prefetched { path, img });
        let mut evicted = false;
        while self.prefetched.len() > PREFETCH_CAP
            || (self.prefetched.len() > 1 && self.prefetch_bytes() > PREFETCH_BYTES)
        {
            self.prefetched.remove(0);
            evicted = true;
        }
        self.prefetch_stopped |= evicted;
        evicted
    }

    /// 缓存里所有位图的字节合计（供 [`PREFETCH_BYTES`] 那条闸比对）。
    pub(super) fn prefetch_bytes(&self) -> usize {
        self.prefetched.iter().map(Prefetched::bytes).sum()
    }

    /// 把一份解码结果装上屏（预读命中与正常解码共用）。
    pub(super) fn install(&mut self, path: PathBuf, d: Decoded) -> bool {
        let (w, h) = (d.bitmap.width, d.bitmap.height);
        // ★ 按目标重画的矢量（m189）：倍率变化后解码器按更大的 `target` 把文档**重画**
        //   一遍，回报的 `natural` 是"这一次的画布"。拿它当 natural，画面会在每次重渲后
        //   按新尺寸重新 fit（用户看到图忽大忽小、状态栏尺寸乱跳），倍率也失去连续
        //   意义。这类格式的 natural 一律**沿用首次画布**——几何与缩放都锚在它上面。
        let natural = self
            .loaded
            .as_ref()
            .filter(|l| l.path == path && l.region.is_none() && d.region.is_none())
            .filter(|_| is_target_vector(&path))
            .map(|l| l.natural)
            .unwrap_or(d.natural);
        // ★ **原图档升级的派生不许压在 UI 线程**（m94）。同一张图更大的源落地时，
        // 旧的上屏档、鸟瞰、朝向全都仍然有效：把它们原样留在屏上，新源交给派生线程
        // 采到同一档，落地由 [`Self::poll_levels`] 换——UI 线程只付一次投递。
        // 实测 6055×6055 → 1001 这一刀同步跑是 38.1 ms（正压在升级落地那一帧）。
        //
        // 只接**真升级**（新源严格更大）。同尺寸重解（动画下一帧）必须立即上屏，
        // 挪去派生线程等于给动画加一帧队尾延迟；切片（region）与换图也不在此列。
        // 判据与 [`Self::apply_orientation`] 同一条：源 + 目标像素过 [`ORIENT_SYNC_PIXELS`]
        // 才值得挪走，小图的线程往返比一刀采样还贵。
        // ★ `Arc<Vec<u8>>` 而不是 `Arc<[u8]>`：后者从 `Vec` 建出来要把像素整份搬一次
        // （6055×6055 实测 32.9 ms，正压在 UI 线程装档那一刻），前者只是把 `Vec` 装进
        // Arc 的箱子里，一个字节都不动。
        let decoded_base: Arc<Vec<u8>> = Arc::new(d.bitmap.rgba);
        // ★ **编辑覆盖就是这张图的最终源**：先定源、再派生。旧写法先按解码档投一次派生，
        // 紧随其后的覆盖又把它作废（`epoch += 1`），等于白烧一次整档重采样，还闪一下
        // 未编辑的画面（m156 B2）。
        let overlay = self.edited.get(&path).map(|e| (e.w, e.h, e.rgba.clone()));
        let (sw, sh) = overlay.as_ref().map_or((w, h), |(ow, oh, _)| (*ow, *oh));
        let base: Arc<Vec<u8>> = match &overlay {
            Some((_, _, px)) => {
                // 覆盖登记时图没开着（抓不到原始 base）→ 这份刚解码的结果就是原图。
                // Arc 克隆，不搬像素；鸟瞰留给还原那一次重采（罕见路径，不为此多扫一遍）。
                if self.edited.get(&path).is_some_and(|e| e.original.is_none()) {
                    if let Some(e) = self.edited.get_mut(&path) {
                        e.original = Some((w, h, Arc::clone(&decoded_base)));
                        e.original_thumb = None;
                    }
                }
                px.clone()
            }
            None => Arc::clone(&decoded_base),
        };
        let want = self.desired_pixels();
        // 屏上这一档还有没有"继续顶着用"的资格：同图、两边都不是切片、有有效上屏档与
        // 鸟瞰、且屏上朝向就是目标朝向（旋转在途时挂着旧朝向等新档会让画面临时换轴）。
        let prev_usable = self
            .loaded
            .as_ref()
            .filter(|l| l.path == path && l.region.is_none() && d.region.is_none())
            .filter(|l| l.displayed != (0, 0) && l.nav_image.is_some())
            .filter(|l| l.nav_key == (self.want_rot, self.want_flip))
            .map(|l| {
                (
                    l.decoded,
                    l.displayed,
                    l.image.clone(),
                    l.nav_image.clone(),
                    l.nav_key,
                )
            });
        let carried = if overlay.is_some() {
            // 换源：小图当场重算（线程往返比一刀采样贵），大图挂旧档等**最终源**落地。
            let cost = sw as u64 * sh as u64 + want.0 as u64 * want.1 as u64;
            if cost > ORIENT_SYNC_PIXELS {
                prev_usable.map(|(_, disp, img, nav, key)| (disp, img, nav, key))
            } else {
                None
            }
        } else {
            prev_usable
                .filter(|(decoded, ..)| {
                    (decoded.0 as u64 * decoded.1 as u64) < (w as u64 * h as u64)
                })
                .filter(|_| {
                    let (fw, fh) = fit_dims(w, h, want.0, want.1);
                    (fw, fh) != (w, h)
                        && w as u64 * h as u64 + fw as u64 * fh as u64 > ORIENT_SYNC_PIXELS
                })
                .map(|(_, disp, img, nav, key)| (disp, img, nav, key))
        };
        // 鸟瞰整图缩略：whole 解码直接收下；SVG 切片重渲只继承**同一张图**的上
        // 一份（首帧必为 whole，同图切片态一定有得继承）。loaded 还停在旧图时宁可
        // 这一帧没有鸟瞰，也不能把旧图的缩略挂到新图名下。
        //
        // ★ 同一张图的更高分辨率档落地（放大到 100% 时原图档正是这样进来的）：旧那份
        // 只要**边长不小于**新采的这一档，重采一分画质也不加——鸟瞰永远只画 ≤512 px，
        // 而这一刀要在 UI 线程把整张原图读一遍（6055×6055 实测 64 ms 的停顿）。
        let nav_thumb_rgba = if overlay.is_some() {
            // 编辑档至多 8 MP（`EDITOR_MAX_PIXELS` 由 UI 把关），这一刀便宜；而旧鸟瞰画的
            // 是未编辑像素，留着就是"缩略图与画面不一致"。
            Some(Self::nav_thumb_of(&base, sw, sh))
        } else if d.region.is_none() {
            // 先只算**目标档位尺寸**（不碰像素），再决定要不要重采：`nav_thumb_of`
            // 那一刀要读完整张原图，判据若靠"先采出来再比"就等于白跑一遍。
            let (fw, fh) = fit_dims(w, h, NAV_THUMB_MAX, NAV_THUMB_MAX);
            let keep = self
                .loaded
                .as_ref()
                .filter(|l| l.path == path)
                .and_then(|l| l.nav_thumb_rgba.clone())
                // 只差一两像素不算"更清晰"：561 档采出来是 511、6055 档是 512，
                // 严格比大小会让每一次原图档落地都在 UI 线程重读整图（实测 26 ms），
                // 换来的画质差别在一个 ≤512 px 的鸟瞰里看不见。差到 3/4 以下才重采。
                .filter(|(_, ow, oh)| (*ow).max(*oh) * 4 >= fw.max(fh) * 3);
            match keep {
                Some(old) => Some(old),
                None => Some(Self::nav_thumb_of(&base, w, h)),
            }
        } else {
            self.loaded
                .as_ref()
                .filter(|l| l.path == path)
                .and_then(|l| l.nav_thumb_rgba.clone())
        };
        // 鸟瞰那份通常不在这里建：紧随其后的 `rebuild_image` 按 `nav_image.is_none()`
        // 判断要重画，来源刚换成新的 `nav_thumb_rgba`，留 `None` 就是"必建一次"。
        // 旧图的那份必须清掉，否则新图的第一帧会画上一张图的鸟瞰。
        //
        // 升级挂起时正相反：旧鸟瞰画的是同一张图、同一朝向，原样留着就是这一帧的鸟瞰；
        // 新来源（同图更高分辨率档）只会在下次换朝向时才被用上。
        let nav_image = carried.as_ref().and_then(|c| c.2.clone());
        // 新像素到了 = 目标朝向成为屏上朝向（换图前 `reset_view` 已把目标归零；
        // SVG 切片重渲时朝向可能非零，同样以目标为准）。
        self.commit_orientation();
        let nav_key = if overlay.is_some() {
            // 鸟瞰留屏但标记为"与新源不符"：落地（或当场重算）那一次必须按编辑结果重建。
            (u8::MAX, false)
        } else {
            carried.as_ref().map_or((self.rot, self.flip), |c| c.3)
        };
        // 按当前 rot/flip 生成上屏位图（翻图后通常已被 reset 归零，此分支是防御）。
        // 装入不再可能失败（GPU 上传推迟到上屏层按 gen 缓存），动画推进闸无条件开：
        // 排在 install 内而不是 tick 里，保证"解码慢于帧延迟"时自然降速而不是排队堆积。
        //
        // ★ 排表基准 = **上一帧应显示的时刻**，不是这一帧落地的时刻（m75）。旧写法
        // `落地 + 延时` 把整段解码成本又白送了一遍，周期成了 `成本 + 延时`：实测标着
        // 25 fps 的 GIF 屏上只有 13.3 fps（640×480）/ 9.1 fps（3 MP @适应窗口）。
        // 续着时间线走之后周期 = `max(延时, 成本)`——文件多快就多快，追不上就一帧不等地放。
        // 落后时 `.max(now)` 夹一下：欠的那一截不补等（补等就是慢放），也不许堆出第二个在途请求。
        //
        // 时间线**只在"同一张图、帧号真的前进了"时续接**：档位升级（同一帧重解一遍）、
        // 换图、逐帧跳转、暂停恢复都重新起表——那些不是"这一帧晚了"，而是"新的一轮开始"。
        //
        // 帧率完全跟随文件自带延时（不设覆盖档）：解码器给每帧多少延时，
        // 我们就按多少播。≥1ms 兕底防 0ms 死循环。
        // 静/动判定以 delay_ms 为准（见 Anim::is_animated）：GIF 首帧渲染时
        // count 仍是未知 0，但 delay_ms>0 即动画——绝不能按 count<=1 把它当
        // 静态图，否则永远不排下一帧、整段 GIF 卡在第一帧。
        //
        // ★ **排表判据用合并后的 `anim`，不是 `d.anim`**（2026-11-02）：
        //   区域重渲时 `d.anim.delay_ms = 0`（`fill_info` 把 `page_count` 写成 1），
        //   但合并后 `anim.delay_ms = PAGE_DELAY_MS`。用 `d.anim` 排表 ⇒ `anim_next_at = None`
        //   ⇒ `tick_anim` 永不触发 ⇒ PDF 放大/平移后自动切页停止。
        //   排表逻辑移到下面 `anim` 变量计算之后执行。
        // ★ 这里**不 stat 文件**（m98）。曾经有一份 `file_size` 在这里每次装档 `fs::metadata`
        //   一次，字段文档还写着"状态栏与属性窗口展示用"——实读：全仓**零读者**，属性窗自己
        //   按路径 stat 它那一份（`ui::settings::metadata`）。而装档不只发生在换图：同一张的
        //   原图档升级、SVG 切片重渲、动画每一帧都进来一次，等于在 UI 线程上白付一串
        //   `GetFileAttributesW`（本地 µs 级）。
        // 缓存路径：`path` 即将被 move 进 Loaded，先 clone 一份留作缓存键。
        let cache_path = path.clone();
        // 装新图 = 世代号自增：在途的旧档位派生回包从此一律作废（晚到的旧像素盖在
        // 新图上就是"闪回上一张"）。`inflight` 一并清掉，免得旧指纹挡住新图的第一次投递。
        self.epoch += 1;
        self.inflight = None;
        // 有一帧上屏了 → 逐帧的在途目标作废，此后以"屏幕上的这一帧"为基准。
        self.step_target = None;
        // ★ 播放元信息的**两条来源约束**（都在页帧合流之后才成立）：
        let prev_anim = self
            .loaded
            .as_ref()
            .filter(|l| l.path == path)
            .map(|l| l.anim);
        let anim = if overlay.is_some() {
            // 编辑覆盖换了像素源，旧文件的动画/分页语义不再成立。
            Anim::STATIC
        } else if d.region.is_some() {
            // 区域切片就是**同一张图的一块**：页数、帧数、页间隔不会因为放大而变。
            // 解码器若没在 region 回包里填 `page_count`（老实现常见），这里兜住——
            // 否则"放大一下，整组播放按钮就没了"（用户 2026-11-02 报的正是这个）。
            match prev_anim {
                Some(p) => Anim {
                    index: p.index,
                    count: p.count.max(d.anim.count),
                    delay_ms: p.delay_ms.max(d.anim.delay_ms),
                    paged: p.paged,
                },
                None => d.anim,
            }
        } else {
            d.anim
        };
        // ★ 播放调度排表：用**合并后的 `anim`**，不是 `d.anim`。
        //   区域重渲时 `d.anim.delay_ms = 0`（`fill_info` 把 `page_count` 写成 1），
        //   但合并后 `anim.delay_ms = PAGE_DELAY_MS`。用 `d.anim` 排表 ⇒ `anim_next_at = None`
        //   ⇒ `tick_anim` 永不触发 ⇒ PDF 放大/平移后自动切页停止（2026-11-02 用户报障）。
        //
        // ★ 时间线**只在"同一张图、帧号真的前进了"时续接**：档位升级（同一帧重解一遍）、
        //   换图、逐帧跳转、暂停恢复都重新起表——那些不是"这一帧晚了"，而是"新的一轮开始"。
        if anim.delay_ms == 0 {
            self.anim_next_at = None;
            self.anim_due_at = None;
        } else {
            let now = Instant::now();
            let delay = anim_wait(anim.delay_ms, self.anim_speed);
            let chain = match (self.anim_due_at, self.loaded.as_ref()) {
                (Some(prev), Some(l)) if l.path == path && l.anim.index != anim.index => {
                    Some(prev + delay)
                }
                _ => None,
            };
            let due = chain.unwrap_or(now + delay).max(now);
            self.anim_due_at = Some(due);
            self.anim_next_at = Some(due);
        }
        self.loaded = Some(Loaded {
            path,
            natural,
            decoded: (sw, sh),
            base_rgba: base,
            // 占位：上屏档要在 `loaded` 就位之后按**这张图**的 natural 与倍率算
            // （`desired_pixels` 读的就是 `self.loaded`），所以位图交给紧随其后的
            // `rebuild_image` 本地派生。这个空位图**不可能被画到**：install 全程同步，
            // 中间没有 paint，而 `clamp_pan` 只碰 `eff_dims`（natural 派生），不读 image。
            //
            // 升级挂起时正相反：旧档原样顶着（见函数开头的 `carried`）。旧像素与新源
            // 是同一张图，画布按 `natural` 缩放绘制，几何一分不差——只是清晰度晚
            // 一次派生（实测 38 ms）到位，而不是让整帧 UI 停在那里等。
            image: carried
                .as_ref()
                .map_or_else(|| RgbaImage::new(0, 0, Vec::new()), |c| c.1.clone()),
            displayed: carried.as_ref().map_or((0, 0), |c| c.0),
            region: overlay.as_ref().map_or(d.region, |_| None),
            anim,
            nav_thumb_rgba,
            nav_image,
            nav_key,
        });
        // ★ **换图 = 播放态归零**（2026-11-02 用户第二次拍板："多帧/多页都自动播放，
        //   否则用户看不出来"）。新图一律从播放态开始：动图自己动、文档按 1 秒/页 自己翻
        //   ——"打开就能看出这张有多张"是这条的唯一目的。
        //
        // ★ 两件事必须同时成立，缺一个就出过 bug：
        //   ① **只在换图时重置**（`prev_anim.is_none()`）。同一张图翻到下一页、放大换档
        //      也走这个函数，那一次若重置，"手动逐帧停下来的那一步"会被下一页的装档
        //      拽回播放。
        //   ② **重置而不是"只删旧的分页默认暂停"**。`anim_paused` 是跨图的单个字段，
        //      在 A 图按过 `,` 之后切到 B 图，B 会带着 A 的暂停态打开——表现就是
        //      "GIF 打开不动了"（用户 2026-11-02 报的正是这个，容易误看成按钮没了）。
        if prev_anim.is_none() {
            // m334：新图按「动图/文档自动播放」决定起点。静态图没有可播的
            // 时间线，不挂假暂停态（与 `toggle_anim_pause` 的静图分支同口径）。
            let autoplay = self.config.borrow().anim_autoplay;
            let has_index = self.loaded.as_ref().is_some_and(|l| l.anim.has_index());
            self.anim_paused = !autoplay && has_index;
        }
        // m333：打开时查到的阅读记忆在这一帧恢复。放在 `rebuild_image` 之前，
        // 让第一次派生就按记忆里的倍率取档；页码跳转也在这里发（不重解当前帧）。
        self.apply_pending_reading_restore(&cache_path);
        // 派生线程接得住就让它算；接不住（线程起不来）退回同步——宁可 UI 线程顿一下，
        // 也不许画面停在旧档不动。
        if !(carried.is_some() && self.post_level(want)) {
            self.rebuild_image();
        }
        // 渲染阶段落地 = 权威尺寸就位。查询阶段的预览作废（契约：查询值仅预览，
        // 与渲染值不一致时以渲染为准；install 之后 info bar 走 loaded.natural）。
        self.preview_meta = None;
        // 编辑覆盖（涂鸦等）已在这里**定源之前**并入（见函数开头的 `overlay`）；
        // 没有覆盖时才需要按 `edited` 账本换一次 base（覆盖登记时图没开着的那条路）。
        if overlay.is_none() {
            self.apply_edited_overlay(&cache_path);
        }
        // 用渲染阶段的权威值覆盖缓存：natural 尺寸以 Decoded 为准。
        self.cache_meta(cache_path, Meta { natural });
        self.error = None;
        self.clamp_pan();
        true
    }
}
