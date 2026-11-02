//! 展示：状态栏文案、信息条、等待提示（「在等什么」）。
//!
//! 文案纪律：说清楚**在等谁**（解码线程 / 解码器 / 网络盘）与**等了多久**，
//! 不要只说"加载中"——用户没法据此判断该不该等。
use super::*;

impl Viewer {
    // ── 展示 ────────────────────────────────────────────────────

    /// 状态栏文本（浮条左端那一格）。
    ///
    /// **错误不在这里**：它由 [`Self::error_text`] 单独走画面中央那一句，两处都写就是
    /// 把同一句话摆两遍（用户 2026-10-18："错误不要重复，太冗余了，只留中央"）。
    /// 中央那句因此是唯一的错误载体，宽度与让位都由 `ui::canvas` 负责。
    pub fn status(&self) -> String {
        let pos = match self.gallery.as_ref() {
            Some(g) if !g.is_empty() => format!("{}/{}", g.position(), g.len()),
            _ => return "没有打开图片".to_string(),
        };
        // 一次性提示优先（m50-C）：它是"刚刚那一下按键"的答复，比 `3/9 · 100%` 更要紧；
        // 过期就当没有——浮条每帧重算，不需要计时器去擦它。
        if let Some((s, at)) = self.note {
            if at.elapsed() < NOTE_TTL {
                return s.to_string();
            }
        }
        // 换图等太久（m71 / 待办 §18 ⑥）：说清"在等什么、等了多久"，而不是继续
        // 拿旧图的数量/百分比冒充当前状态——卡住时那是谎话。3 s 以下的正常解码
        // 不打扰（见 [`WAIT_NOTICE_AFTER`]）。
        if let Some(waited) = self.pending_wait() {
            return match waited {
                w if w >= STALL_WARN_AFTER => {
                    format!("{} · 载入中… {} 秒 · 可能卡住", pos, w.as_secs())
                }
                w => format!("{} · 载入中… {} 秒", pos, w.as_secs()),
            };
        }
        match self.loaded.as_ref() {
            Some(l) if l.anim.has_index() => {
                // 报张号：**这些格式一律报**，哪怕是 1/1。
                //
                // ★ 2026-11-02 用户拍板后的终态：
                // - 格式 `x/y · a% (c/d)`：x/y=图片进度、a%=缩放百分比、c/d=帧/页进度；
                // - 动图**播放中也报**（计数器在跳就是"在自动播"的证据）；
                // - 分页文档一直报（164 页的 PDF 靠它对齐）；
                // - 静态位图也报，落 `(1/1)`（契约规定解码器回填 `frame_count=1 / page_count=1`）。
                let pct = (self.scale() * 100.0).round() as i32;
                let total = if l.anim.count > 0 {
                    format!("/{}", l.anim.count)
                } else {
                    String::new()
                };
                format!("{} · {}% ({}{})", pos, pct, l.anim.index + 1, total)
            }
            Some(_) => {
                let pct = (self.scale() * 100.0).round() as i32;
                format!("{} · {}%", pos, pct)
            }
            None => {
                // 查询阶段 Meta 已到时（HEIF ispe / RAW 只读头 / PNG IHDR 之类
                // header-only 就能报尺寸的格式），把预览尺寸拼进"载入中"，
                // 让用户立刻看到"这张是 4000×3000，正在解"——比空转的"载入中…"
                //  informative。Frame 一到本分支就切回 `Some(_)` 走百分比。
                //
                // ★ 但**已经失败就不说"载入中"**：错误从状态栏挪走之后（只留中央），
                //   这句会变成"永远载入中"的谎话。留个位置号就够了。
                if self.error.is_some() {
                    pos
                } else {
                    match self.preview_meta {
                        Some(m) => format!("{} · {}×{} · 载入中…", pos, m.natural.0, m.natural.1),
                        None => format!("{} · 载入中…", pos),
                    }
                }
            }
        }
    }

    /// 当前这枚主画面请求已等了多久；**不该报**的时候返 `None`（m71 / 待办 §18 ⑥）。
    ///
    /// 四个"不该报"：出错（中央已有一句，状态栏不再谎称载入中）、没打开图片、
    /// 屏上那张就是当前图（正常看图中，解码再慢也不改变"你在看什么"）。
    /// 剩下的就是换图后旧图还顶着显示（防闪），或者首图还没落地——此时状态栏里
    /// 那个百分比/帧号是**上一张**的，不说"载入中"就是在骗人。
    pub(super) fn pending_wait(&self) -> Option<Duration> {
        if self.error.is_some() {
            return None;
        }
        let cur = self.gallery.as_ref()?.current()?;
        if self.loaded.as_ref().is_some_and(|l| l.path == *cur) {
            return None;
        }
        let waited = self.main_sent_at.get()?.elapsed();
        (waited >= WAIT_NOTICE_AFTER).then_some(waited)
    }

    /// 同一枚主画面请求等到 [`STALL_WARN_AFTER`] 时写**一条** warn（m71）。
    ///
    /// 只报不治：这里不取消、不改发、不换解码路——超时取消/熔断是 §18 ⑥ 里
    /// **明确不做**的那一半（解码线程串行且解码器调用不可抢占，掐掉只会让宿主
    /// 与解码器状态失配）。`stall_logged` 由下一枚 Main 请求复位，所以"每枚请求
    /// 最多一条"，不会把日志刷满。
    pub(super) fn note_stalled_request(&self) {
        if self.stall_logged.get() {
            return;
        }
        let Some(waited) = self.pending_wait() else {
            return;
        };
        if waited < STALL_WARN_AFTER {
            return;
        }
        self.stall_logged.set(true);
        let path = self
            .gallery
            .as_ref()
            .and_then(|g| g.current())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        log::warn!(
            "[stall] file={} waited={:.1}s 主画面仍未回包（不自动取消，继续等）",
            path,
            waited.as_secs_f64()
        );
    }

    /// 首图还在路上（没有位图、没有错误、但清单已有当前项）——画布中央该说
    /// "载入中…"而不是"把图片拖进来"（m71：那张卡住 13 s 的截图中央就是后者）。
    pub fn awaiting_first_bitmap(&self) -> bool {
        self.error.is_none()
            && self.loaded.is_none()
            && self.gallery.as_ref().and_then(|g| g.current()).is_some()
    }

    /// 窗口标题：当前文件名；无图片时显示 RuPix。
    pub fn title(&self) -> String {
        self.gallery
            .as_ref()
            .and_then(|g| g.current())
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "RuPix".to_string())
    }

    /// 当前目录里的图片路径（供"另存/复制路径"之类后续功能用）。
    pub fn current_path(&self) -> Option<PathBuf> {
        self.gallery.as_ref().and_then(|g| g.current()).cloned()
    }

    /// 当前正在看的目录（供 runtime 挂目录监控：看图目录变了就重臂 watcher）。
    ///
    /// **借出来，不抄一份**（m106）：这个口子以前每 16 ms 被泵一次，只为和"上一次监控
    /// 的是哪个目录"比一句相等不相等——目录绝大多数时候根本没变，那份 `PathBuf` 是白抄的。
    /// 判据要的只是一次比较，所以这里交引用；**真要留下那份路径的调用点自己 `.to_path_buf()`**。
    pub fn gallery_dir(&self) -> Option<&std::path::Path> {
        self.gallery.as_ref().map(|g| g.dir())
    }

    /// 当前图的世代号（每次打开新图 / 翻页 / 当前图解码完成上屏 自增）。
    /// 属性窗轮询比对它：变了就重算并刷新那扇窗的内容，否则保持不动（不空耗帧）。
    pub fn image_gen(&self) -> u64 {
        self.image_gen
    }
}
