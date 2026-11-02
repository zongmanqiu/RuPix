//! 缩略图条（filmstrip）的后台解码状态。
//!
//! 与主画面共用**同一条**解码线程，所以在途数量有硬上限：一次最多
//! [`MAX_INFLIGHT`] 个请求——留着它是为了不让一次翻页把队列灌满陈旧格子，
//! "正在看的那张"排在缩略图后面这件事已由解码线程的**主画面插队**兜住
//! （[`crate::core::decode`] 的 `Queue::pop`，2026-10-17 修放大糊图时加）。
//!
//! 存的是内存里那份 [`RgbaImage`]（本模块的 `map`）；跨会话没有磁盘那一层了——
//! 2026-10-19 撤掉解码缓存，缩略图本来也达不到它的门槛（240×180×4 ≈ 172 KB）。

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::core::decode::{Decoder, Done};
use crate::core::pixels::RgbaImage;

/// 缩略图的目标框（设备像素）。
///
/// **必须是常量**：这一档就是胶片条的格子形状，跟着窗口大小变会让同一目录在一条
/// 条带上出现好几种请求尺寸——内存 map 按路径只留一份，尺寸一换就得重解。
/// （早先这条还多一层理由：磁盘解码缓存的键含 `max_w/max_h`；那层 2026-10-19 已撤。）
pub const THUMB_W: u32 = 240;
pub const THUMB_H: u32 = 180;

/// 同时在途的请求上限（理由见模块头）。
const MAX_INFLIGHT: usize = 3;

/// 内存里最多留多少张（超了按最久未用淘汰）。
///
/// 一张 240×180 的 RGBA 是 172 KB，80 张约 14 MB——够铺满好几屏，又不至于
/// 在几千张的目录里翻一圈就把内存吃掉。
const MAX_KEPT: usize = 80;

/// 一格缩略图的状态。
enum Slot {
    /// 请求已发出，等回包。
    ///
    /// 不带 `seq`：认领回包靠 `inflight` 那张表反查，格子里再存一份就多余了
    /// （两份状态迟早不一致）。
    Pending,
    /// 已就绪。
    Ready(Rc<RgbaImage>),
    /// 解码失败（坏图 / 无人认领的格式）。
    ///
    /// 记下来而不是"当作没有"：否则翻一页回来它又被请求一次，同一张坏图会
    /// 反复进解码队列（而且每次都要真读一遍文件才判得出失败）。
    Failed,
}

/// 缩略图的请求账本与内存缓存。
pub struct Thumbs {
    /// 路径 → 状态。`Cell<u64>` 是**最后一次写入或被读到**的时刻，淘汰时挑最旧的。
    /// 用 `Cell` 而不是裸 `u64`：[`Thumbs::get`] 只借 `&self`，但**读到就该算「最近用过」**——
    /// 不续期的话淘汰的是「最早塞进来的」，来回翻页时刚看过的缩略图会被优先丢掉。
    slots: HashMap<PathBuf, (Slot, Cell<u64>)>,
    /// 在途请求的 `seq → 路径`：回包据此认领，认领不到就不是缩略图的。
    inflight: HashMap<u64, PathBuf>,
    stamp: Cell<u64>,
}

impl Thumbs {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            inflight: HashMap::new(),
            stamp: Cell::new(0),
        }
    }

    /// 已就绪的缩略图（`Rc` 廉价克隆，绘制侧拿去上传纹理）。
    pub fn get(&self, path: &Path) -> Option<Rc<RgbaImage>> {
        let hit = self.slots.get(path);
        match hit {
            Some((Slot::Ready(img), seen)) => {
                // 读到即续期（LRU 的「U」）。失败与在途的格子不续——它们没被用上。
                seen.set(self.bump());
                Some(Rc::clone(img))
            }
            _ => None,
        }
    }

    /// 单调时钟一格。
    fn bump(&self) -> u64 {
        let n = self.stamp.get() + 1;
        self.stamp.set(n);
        n
    }

    /// 为**还没有着落**的路径发起后台解码，受 [`MAX_INFLIGHT`] 约束。
    ///
    /// 已有的（含在途与失败的）一律跳过，所以每帧调用也只会在真正缺图时发请求。
    /// ★ 收的是 `&[&Path]` 而不是 `&[PathBuf]`（m100）：调用方（`Viewer::strip_window`）
    ///   手里只有清单里那些路径的**引用**，旧签名逼它先克隆一份整屏的 `PathBuf`——
    ///   一帧 12 格就是 12 次分配，而这笔克隆在这条路上没有任何下游需要拥有权。
    pub fn ensure(&mut self, decoder: &Decoder, paths: &[&Path]) {
        for p in paths {
            if self.slots.contains_key(*p) {
                continue;
            }
            if self.inflight.len() >= MAX_INFLIGHT {
                break;
            }
            let seq = decoder.request(p, THUMB_W, THUMB_H);
            let owned = p.to_path_buf();
            self.inflight.insert(seq, owned.clone());
            self.put(owned, Slot::Pending);
        }
    }

    /// 这个回包是不是缩略图的（`true` 时调用方应交给 [`Self::take`]）。
    pub fn wants(&self, seq: u64) -> bool {
        self.inflight.contains_key(&seq)
    }

    /// 收一个缩略图回包。**调用前必须先过 [`Self::wants`]**，否则 `done` 白扔。
    pub fn take(&mut self, done: Done) -> bool {
        let Some(path) = self.inflight.remove(&done.seq) else {
            return false;
        };
        let slot = match done.out {
            Ok(d) => Slot::Ready(Rc::new(RgbaImage::new(
                d.bitmap.width,
                d.bitmap.height,
                d.bitmap.rgba,
            ))),
            Err(e) => {
                log::debug!("缩略图解码失败（此格留空）：{}：{e}", path.display());
                Slot::Failed
            }
        };
        self.put(path, slot);
        true
    }

    /// 换目录时清账：旧目录的缩略图不会再被看见，占着内存没意义。
    pub fn clear(&mut self) {
        self.slots.clear();
        self.inflight.clear();
    }

    fn put(&mut self, path: PathBuf, slot: Slot) {
        let stamp = self.bump();
        self.slots.insert(path, (slot, Cell::new(stamp)));
        if self.slots.len() > MAX_KEPT {
            // ★ 这条"每插一枚就扫一遍全表挑最旧、还要克隆那枚键才摘得掉"的形状，m108 立案
            //   也判废了（O14）：满表 80 枚时淘汰这笔＝每张新缩略图 0.4–0.7 µs；换成
            //   "到上限一次剪到 3/4"（`ui::filmstrip` 那一形）或"retain 就地摘、零克隆"，
            //   三臂对撞的差值**符号在轮之间翻** ⇒ 判不出，不是省（读数见 本仓历史记录 与
            //   `examples/frame_cost_probe.rs` 的 D3）。也别拿"两张表形状不一致"当理由重做：
            //   filmstrip 的批量剪枝是 2026-10-14 为消掉「溢出整批 clear 那一下小闪」特意选的。
            // 只淘汰「已有着落」的格（Ready/Failed）：Pending 还有在途回包，淘汰它
            // 会让 ensure 重新发一遍请求、inflight 里的登记也成孤儿。Pending 至多
            // MAX_INFLIGHT 个而 MAX_KEPT ≫ 它，非 Pending 必有候选；极端下宁可暂超
            // 一格，也不制造重复请求。
            let oldest = self
                .slots
                .iter()
                .filter(|(_, (slot, _))| !matches!(slot, Slot::Pending))
                .min_by_key(|(_, (_, s))| s.get())
                .map(|(p, _)| p.clone());
            if let Some(p) = oldest {
                self.slots.remove(&p);
            }
        }
    }
}

impl Default for Thumbs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> RgbaImage {
        RgbaImage::new(w, h, vec![0u8; (w * h * 4) as usize])
    }

    fn done(seq: u64, path: &Path, ok: bool) -> Done {
        Done {
            seq,
            path: path.to_path_buf(),
            out: if ok {
                Ok(crate::core::pixels::Decoded {
                    natural: (4, 3),
                    bitmap: crate::core::pixels::Bitmap {
                        width: 4,
                        height: 3,
                        rgba: vec![7u8; 4 * 3 * 4],
                    },
                    region: None,
                    anim: crate::core::pixels::Anim::STATIC,
                })
            } else {
                Err("坏图".into())
            },
        }
    }

    /// 非缩略图的回包必须被拒收——否则主画面的结果会被这里吞掉。
    #[test]
    fn ignores_unknown_seq() {
        let mut t = Thumbs::new();
        let p = PathBuf::from("a.png");
        assert!(!t.wants(1));
        assert!(!t.take(done(1, &p, true)));
    }

    /// 回包按 seq 认领进对应的格子。
    #[test]
    fn take_stores_ready_image() {
        let mut t = Thumbs::new();
        let p = PathBuf::from("a.png");
        t.slots.insert(p.clone(), (Slot::Pending, Cell::new(1)));
        t.inflight.insert(9, p.clone());
        assert!(t.wants(9));
        assert!(t.take(done(9, &p, true)));
        assert_eq!(t.get(&p).map(|i| (i.width, i.height)), Some((4, 3)));
        // 认领过一次就没了：重复回包不再写。
        assert!(!t.wants(9));
    }

    /// 失败也要占一格：否则同一张坏图会被反复请求。
    #[test]
    fn failed_slot_is_remembered() {
        let mut t = Thumbs::new();
        let p = PathBuf::from("bad.png");
        t.slots.insert(p.clone(), (Slot::Pending, Cell::new(1)));
        t.inflight.insert(3, p.clone());
        assert!(t.take(done(3, &p, false)));
        assert!(t.get(&p).is_none());
        // 有格子在 → 不会再发请求（ensure 靠 contains_key 判缺）。
        assert!(t.slots.contains_key(&p));
    }

    /// `ensure` 只对缺图的发起请求，且在途数到顶就停。
    ///
    /// 用**真的** `Decoder`（它只是个发送端，背后那条线程解不解得出来无所谓）：
    /// 这里验的是账本侧的"跳过已知 + 到顶即停"，换成假解码器反而要复制一遍
    /// `ensure` 的逻辑，那就不是在测 `ensure` 了。
    #[test]
    fn ensure_skips_known_and_caps_inflight() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let decoder = Decoder::spawn_with(tx);
        let mut t = Thumbs::new();
        let paths: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("{i}.png"))).collect();
        // 手工给三张"已有着落"（一张就绪、一张失败、一张在途），其余待发。
        t.put(paths[0].clone(), Slot::Ready(Rc::new(img(2, 2))));
        t.put(paths[1].clone(), Slot::Failed);
        t.put(paths[2].clone(), Slot::Pending);
        t.inflight.insert(0, paths[2].clone());

        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        t.ensure(&decoder, &refs);
        // 前三张被跳过；在途原本 1 个，故再发 2 个就到顶（不是把剩下 7 张全发掉）。
        assert_eq!(t.inflight.len(), MAX_INFLIGHT);
        assert_eq!(t.slots.len(), 5, "3 张手工 + 2 张新请求");
        assert!(t.get(&paths[0]).is_some(), "就绪那张不受影响");
    }

    /// 超过上限时淘汰**最久未用**的那一格——写入和读到都算「用过」。
    ///
    /// 这条测试原本是反过来钉的（「淘汰的是最久未写入的，不是最近刚用到的」），
    /// 那等于把 FIFO 判成正确。2026-10-17 审计指出模块头的口径是 LRU 而实现是 FIFO，
    /// 现按 LRU 落地并把断言掉转：**刚被读过的格子不得先被淘汰**。
    #[test]
    fn evicts_least_recently_used() {
        let mut t = Thumbs::new();
        for i in 0..MAX_KEPT {
            t.put(PathBuf::from(format!("{i}.png")), Slot::Failed);
        }
        assert_eq!(t.slots.len(), MAX_KEPT);
        // 先把最旧的 0.png「用一下」——此后它就不该是淘汰候选。
        t.put(PathBuf::from("0.png"), Slot::Ready(Rc::new(img(4, 3))));
        assert!(t.get(&PathBuf::from("0.png")).is_some());
        t.put(PathBuf::from("new.png"), Slot::Failed);
        assert_eq!(t.slots.len(), MAX_KEPT, "超上限后应稳定在上限");
        assert!(
            t.slots.contains_key(&PathBuf::from("0.png")),
            "刚被读到的 0.png 不该先被淘汰（LRU 的 U）"
        );
        assert!(
            !t.slots.contains_key(&PathBuf::from("1.png")),
            "该淘汰的是次旧且没用过的 1.png"
        );
        assert!(t.slots.contains_key(&PathBuf::from("new.png")));
    }

    /// 换目录清空：旧目录的缩略图不该继续占内存。
    #[test]
    fn clear_drops_everything() {
        let mut t = Thumbs::new();
        t.put(PathBuf::from("a.png"), Slot::Ready(Rc::new(img(2, 2))));
        t.inflight.insert(5, PathBuf::from("b.png"));
        t.clear();
        assert!(t.slots.is_empty());
        assert!(t.inflight.is_empty());
        assert!(!t.wants(5));
    }
}
