//! 编辑覆盖（涂鸦等编辑型插件回写，P0.4 / P0.1b-4）。
//!
//! 只存内存（便携铁律：解码/编辑结果不落盘）。一张图的编辑结果登记在
//! [`Viewer::edited`]：装载漏斗（`install`）每装一档就用覆盖换 base——导航离开
//! 再回来、甚至重开同一张图，覆盖都跟着生效；「还原」换回覆盖那一刻抓下的
//! 原始 base 并清掉登记。
//!
//! 几何不重排是整个设计的支点：插件**收到的与回传的都是同一尺寸**（"屏上朝向"那一份，
//! 见 [`Viewer::whole_image_for_plugin`]），宿主在落地前把它转回源朝向——所以 `natural` 恒不变，
//! 缩放 / 平移 / 旋转 / 鸟瞰的全部账目照旧成立。

use super::*;
use std::path::Path;

/// 一张图已应用的编辑覆盖。
///
/// `original` 是覆盖那一刻的原始 base（还原用）：图正好开着时在
/// [`Viewer::install_edited`] 抓下；图没开着时在覆盖首次生效（装载）那刻补抓——
/// 那份刚解码的结果就是原图。
pub(super) struct EditedKeep {
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) rgba: Arc<Vec<u8>>,
    pub(super) original: Option<(u32, u32, Arc<Vec<u8>>)>,
    // 还原可能回到 64MP：复用当初的鸟瞰，不能在 UI 线程再扫描原图。
    pub(super) original_thumb: Option<(Arc<Vec<u8>>, u32, u32)>,
}

impl Viewer {
    /// 把编辑覆盖套到当前 loaded 上（若正是这张图）。装载漏斗每装一档调一次。
    pub(super) fn apply_edited_overlay(&mut self, path: &Path) {
        let Some((w, h, rgba)) = self.edited.get(path).map(|e| (e.w, e.h, e.rgba.clone())) else {
            return;
        };
        // 首次生效时若还没抓过原始 base（应用编辑那会儿图没开着），这份刚解码的
        // loaded 就是原图——补抓，还原才有得换。
        if self.edited.get(path).is_some_and(|e| e.original.is_none()) {
            let orig = self
                .loaded
                .as_ref()
                .filter(|l| l.path == path)
                .map(|l| (l.decoded.0, l.decoded.1, l.base_rgba.clone()));
            if let Some(orig) = orig {
                if let Some(e) = self.edited.get_mut(path) {
                    e.original = Some(orig);
                    e.original_thumb = self.loaded.as_ref().and_then(|l| l.nav_thumb_rgba.clone());
                }
            }
        }
        if !self.loaded.as_ref().is_some_and(|l| l.path == path) {
            return;
        }
        // 编辑档至多 8 MP：鸟瞰按新源重采（旧那份画的是未编辑像素）。
        self.swap_source(w, h, rgba, None);
    }

    /// 换源（覆盖应用 / 还原共用）：先废掉旧派生，再按成本决定当场重算还是投给派生线程。
    ///
    /// ★ 两条硬规矩：① **不许**把未派生的 raw 塞进显示槽——回包前屏上保持旧档，否则
    /// 闪一下未编辑／未旋转的画面（`natural` 一变目标档就变，等着落地即可）；② 鸟瞰标记成
    /// "与新源不符"（`nav_key` 哨兵），落地或当场重算那一次必须重建，不能留旧鸟瞰。
    fn swap_source(
        &mut self,
        w: u32,
        h: u32,
        rgba: Arc<Vec<u8>>,
        thumb: Option<(Arc<Vec<u8>>, u32, u32)>,
    ) {
        self.epoch += 1;
        self.inflight = None;
        {
            let Some(l) = self.loaded.as_mut() else {
                return;
            };
            l.base_rgba = rgba.clone();
            l.decoded = (w, h);
            l.region = None;
            l.anim = Anim::STATIC;
            l.nav_thumb_rgba = Some(thumb.unwrap_or_else(|| Self::nav_thumb_of(&rgba, w, h)));
            l.nav_key = (u8::MAX, false);
        }
        // 换源不能走 `apply_orientation`：目标朝向没变它也直接 return，新像素就永远不算。
        let want = self.desired_pixels();
        let px = match self.loaded.as_ref() {
            Some(l) => l.decoded.0 as u64 * l.decoded.1 as u64 + want.0 as u64 * want.1 as u64,
            None => return,
        };
        if px <= ORIENT_SYNC_PIXELS || !self.post_level(want) {
            self.commit_orientation();
            self.rebuild_image();
            self.clamp_pan();
        }
    }

    /// 编辑型插件回写：登记覆盖并把这张图换到新源（当前 loaded 正是它时）。
    ///
    /// 图没开着也登记——覆盖随下一次装载生效。
    /// 返回 = **这张图是不是正开着的那张**（不代表像素已经落地：大图走派生线程，
    /// 晚一拍才换；旧档在落地前继续顶着，不闪空白）。
    pub fn install_edited(&mut self, path: &Path, w: u32, h: u32, rgba: Vec<u8>) -> bool {
        if w == 0 || h == 0 {
            return false;
        }
        if rgba.len() != w as usize * h as usize * 4 {
            return false;
        }
        let original = self
            .loaded
            .as_ref()
            .filter(|l| l.path == path)
            .map(|l| (l.decoded.0, l.decoded.1, l.base_rgba.clone()));
        let original_thumb = self
            .loaded
            .as_ref()
            .filter(|l| l.path == path)
            .and_then(|l| l.nav_thumb_rgba.clone());
        self.edited.insert(
            path.to_path_buf(),
            EditedKeep {
                w,
                h,
                rgba: Arc::new(rgba),
                original,
                original_thumb,
            },
        );
        self.apply_edited_overlay(path);
        self.loaded.as_ref().is_some_and(|l| l.path == path)
    }

    /// 还原某张图的编辑覆盖：换回原始 base（当前正好开着它时）并清掉登记。
    /// 返回 = 是否有覆盖被还原。
    pub fn revert_edited(&mut self, path: &Path) -> bool {
        let Some(ed) = self.edited.remove(path) else {
            return false;
        };
        if !self.loaded.as_ref().is_some_and(|l| l.path == path) {
            return false;
        }
        let Some((ow, oh, px)) = ed.original else {
            return false;
        };
        // ★ 还原可能回到 64 MP 原图档：鸟瞰用覆盖那一刻抓下的那一份，**不在 UI 线程重扫原图**。
        // 抓不到（覆盖登记时图没开着）才退回重采一次——与本轮改动前的行为一致。
        let thumb = ed
            .original_thumb
            .or_else(|| Some(Self::nav_thumb_of(&px, ow, oh)));
        self.swap_source(ow, oh, px, thumb);
        true
    }

    /// 这张图当前有没有未保存的编辑覆盖（插件菜单的另存/还原据此显隐）。
    pub fn is_edited(&self, path: &Path) -> bool {
        self.edited.contains_key(path)
    }

    /// 编辑/分析型插件入口用：当前整图（非切片）**按屏上朝向转正**后的一份 [`ForPlugin`]。
    ///
    /// SVG 切片态返回 `None`——编辑要的是整图语义；放大到原图档后就是整图。
    ///
    /// ★ 交出去的是"用户屏幕上看见的那一张"，不是 `base_rgba` 的源朝向。理由：竖拍照片在
    ///   宿主里是正的，插件要是拿到躺着的，用户就在"他没见过的那个方向"上涂、体检会把
    ///   16:9 报成 9:1、一维条码直接扫不出来。恒等朝向不另拷一份（多数调用走这一支）。
    /// ★ `ForPlugin::orient` 必须随任务一路带到落地，[`Viewer::install_edited_displayed`]
    ///   用它（**不是**此刻的 `self.rot`）换算回源朝向。
    pub fn whole_image_for_plugin(&self) -> Option<ForPlugin> {
        let l = self.loaded.as_ref()?;
        if l.region.is_some() {
            return None;
        }
        let orient = (self.rot, self.flip);
        let (w, h) = (l.decoded.0, l.decoded.1);
        if orient == (0, false) {
            return Some(ForPlugin {
                path: l.path.clone(),
                width: w,
                height: h,
                rgba: l.base_rgba.clone(),
                orient,
            });
        }
        let (dw, dh, px) =
            crate::core::geometry::transformed(&l.base_rgba, w, h, orient.0, orient.1);
        Some(ForPlugin {
            path: l.path.clone(),
            width: dw,
            height: dh,
            rgba: Arc::new(px),
            orient,
        })
    }

    /// 编辑型回包落地：把**屏上朝向**的回传像素还原成源朝向，再交 [`Viewer::install_edited`]。
    ///
    /// `orient` 是 [`Viewer::whole_image_for_plugin`] 那一刻冻结的朝向。不许图省事改读
    /// `self.rot/self.flip`：用户在插件窗口里待着的时候，主窗口完全可能被再转一次——
    /// 那样还原用的就不是当初发出去的那个变换，回写会带上一次多余旋转（屏幕上歪 90°，
    /// 而像素数量对得上，任何尺寸校验都挡不住）。
    pub fn install_edited_displayed(
        &mut self,
        path: &Path,
        w: u32,
        h: u32,
        rgba: Vec<u8>,
        orient: (u8, bool),
    ) -> bool {
        if orient == (0, false) {
            return self.install_edited(path, w, h, rgba);
        }
        let (ir, iflip) = crate::core::geometry::inverted(orient.0, orient.1);
        let (sw, sh, source) = crate::core::geometry::transformed(&rgba, w, h, ir, iflip);
        self.install_edited(path, sw, sh, source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::decode::{Decoder, Done};
    use crate::core::pixels::Anim;

    fn viewer_with(w: u32, h: u32, path: &Path) -> Viewer {
        let (tx, _rx) = std::sync::mpsc::channel::<Done>();
        let mut v = Viewer::new(
            Rc::new(Decoder::spawn_with(tx)),
            Rc::new(RefCell::new(Config::default())),
        );
        let px = vec![0x40u8; w as usize * h as usize * 4];
        v.loaded = Some(Loaded {
            path: path.to_path_buf(),
            natural: (w, h),
            decoded: (w, h),
            displayed: (w, h),
            base_rgba: Arc::new(px.clone()),
            image: RgbaImage::new(w, h, px),
            region: None,
            anim: Anim::STATIC,
            nav_thumb_rgba: None,
            nav_image: None,
            nav_key: (0, false),
        });
        v
    }

    /// 每像素首字节 = 序号的可区分图（纯色图转了朝向看不出来，验变换必须用这种夹具）。
    fn patterned_viewer(w: u32, h: u32, path: &Path) -> Viewer {
        let mut v = viewer_with(w, h, path);
        let px: Vec<u8> = (0..(w * h) as usize)
            .flat_map(|i| [i as u8, 0, 0, 255])
            .collect();
        let loaded = v.loaded.as_mut().expect("夹具已装图");
        loaded.base_rgba = Arc::new(px.clone());
        loaded.image = RgbaImage::new(w, h, px);
        v
    }

    /// ★ 编辑/分析入口交出去的是**屏上朝向**那一份（宽高随之对调），并冻结这一次朝向。
    /// 恒等时不另拷（多数调用走那一支，`Arc` 同一份）。
    #[test]
    fn edit_entry_hands_out_the_on_screen_orientation() {
        let p = Path::new("orient-out.png");
        let mut v = patterned_viewer(3, 2, p);
        let src = v.loaded.as_ref().expect("夹具").base_rgba.clone();
        let e = v.whole_image_for_plugin().expect("整图态可编辑");
        assert_eq!((e.width, e.height, e.orient), (3, 2, (0, false)));
        assert_eq!(e.rgba.as_slice(), src.as_slice());

        v.rotate_cw();
        let e = v.whole_image_for_plugin().expect("整图态可编辑");
        assert_eq!(
            (e.width, e.height, e.orient),
            (2, 3, (1, false)),
            "宽高必须随朝向对调"
        );
        let (ew, eh, want) = crate::core::geometry::transformed(&src, 3, 2, 1, false);
        assert_eq!((ew, eh), (2, 3));
        assert_eq!(e.rgba.as_slice(), want.as_slice());
    }

    /// ★ 落地还原必须用**送入时冻结**的朝向，不是此刻的 `rot`。
    /// 用例把这条钉死：送出（rot=1）之后主窗口再转一次（此刻 rot=2），插件原样回传，
    /// 源像素仍要逐字节回到送出前——写成读 `self.rot` 就会带上一次多余旋转，
    /// 而像素数量对得上，任何尺寸校验都挡不住。
    #[test]
    fn edited_return_is_restored_with_the_frozen_orientation() {
        let p = Path::new("orient-back.png");
        let mut v = patterned_viewer(3, 2, p);
        let src = v.loaded.as_ref().expect("夹具").base_rgba.clone();
        v.rotate_cw();
        let e = v.whole_image_for_plugin().expect("整图态可编辑");
        assert_eq!(e.orient, (1, false));
        v.rotate_cw();
        assert_eq!(
            (v.rot, v.flip),
            (2, false),
            "用户在插件窗口里时主窗口能再转"
        );

        assert!(v.install_edited_displayed(&e.path, e.width, e.height, e.rgba.to_vec(), e.orient));
        let loaded = v.loaded.as_ref().expect("夹具");
        assert_eq!(loaded.natural, (3, 2), "源尺寸必须回到未转的那一份");
        assert_eq!(
            loaded.base_rgba.as_slice(),
            src.as_slice(),
            "还原应与送出前逐字节相同"
        );
    }

    #[test]
    fn stale_derived_pixels_cannot_replace_edit_or_revert() {
        for revert in [false, true] {
            let p = Path::new("edited-level.png");
            let mut v = viewer_with(4, 4, p);
            v.on_viewport(0.0, 0.0, 20.0, 20.0, 1.0);
            if revert {
                assert!(v.install_edited(p, 4, 4, vec![0xF0; 64]));
            }
            assert!(v.post_level((2, 2)));
            if revert {
                assert!(v.revert_edited(p));
            } else {
                assert!(v.install_edited(p, 4, 4, vec![0xF0; 64]));
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while v.level_counts().discarded == 0 && Instant::now() < deadline {
                assert!(!v.poll_levels(), "旧来源的派生结果不许上屏");
                std::thread::yield_now();
            }
            assert_eq!(v.level_counts().discarded, 1);
            let expected = if revert { 0x40 } else { 0xF0 };
            assert!(v.image().unwrap().rgba.iter().all(|b| *b == expected));
        }
    }

    #[test]
    fn edited_pixels_follow_the_current_orientation() {
        let p = Path::new("rotated-edit.png");
        let mut v = viewer_with(4, 2, p);
        v.on_viewport(0.0, 0.0, 20.0, 20.0, 1.0);
        v.rotate_cw();
        assert_eq!(
            (v.image().unwrap().width, v.image().unwrap().height),
            (2, 4)
        );
        assert!(v.install_edited(p, 4, 2, vec![0xF0; 32]));
        assert_eq!(
            (v.image().unwrap().width, v.image().unwrap().height),
            (2, 4)
        );
        assert!(v.revert_edited(p));
        assert_eq!(
            (v.image().unwrap().width, v.image().unwrap().height),
            (2, 4)
        );
    }

    fn solid_decoded(w: u32, h: u32, value: u8) -> Decoded {
        Decoded {
            natural: (w, h),
            region: None,
            anim: Anim::STATIC,
            bitmap: crate::core::pixels::Bitmap {
                width: w,
                height: h,
                rgba: vec![value; w as usize * h as usize * 4],
            },
        }
    }

    fn wait_for_level(v: &mut Viewer) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !v.poll_levels() {
            assert!(Instant::now() < deadline, "新来源派生未落地");
            std::thread::yield_now();
        }
    }

    #[test]
    fn large_revert_keeps_display_until_one_level_lands() {
        let p = Path::new("large-revert.png");
        let mut v = viewer_with(4, 2, p);
        v.on_viewport(0.0, 0.0, 80.0, 80.0, 1.0);
        assert!(v.install(p.to_path_buf(), solid_decoded(8000, 8000, 0x40)));
        if v.inflight.is_some() {
            wait_for_level(&mut v);
        }
        v.rotate_cw();
        if v.inflight.is_some() {
            wait_for_level(&mut v);
        }
        assert!(v.install_edited(p, 4, 4, vec![0xF0; 64]));
        let old_image = v.image().unwrap().gen;
        let old_displayed = v.loaded.as_ref().unwrap().displayed;
        let old_orientation = (v.rot, v.flip);
        let posted = v.level_counts().posted;
        assert!(v.revert_edited(p));
        assert_eq!(
            v.level_counts().posted - posted,
            1,
            "64MP 还原必须投给原 LevelThread"
        );
        assert_eq!(
            v.image().unwrap().gen,
            old_image,
            "回包前保留编辑画面，不能展示未旋转 raw"
        );
        assert_eq!(v.loaded.as_ref().unwrap().displayed, old_displayed);
        assert_eq!((v.rot, v.flip), old_orientation);
        wait_for_level(&mut v);
        assert!(v.image().unwrap().rgba.iter().all(|b| *b == 0x40));
        assert_eq!(v.loaded.as_ref().unwrap().natural, (8000, 8000));
    }

    #[test]
    fn reinstall_overlay_posts_only_final_source() {
        let p = Path::new("reinstall-overlay.png");
        let mut v = viewer_with(4, 4, p);
        v.on_viewport(0.0, 0.0, 80.0, 80.0, 1.0);
        assert!(v.install(p.to_path_buf(), solid_decoded(400, 400, 0x40)));
        assert!(v.install_edited(p, 2000, 2000, vec![0xF0; 2000 * 2000 * 4]));
        if v.inflight.is_some() {
            wait_for_level(&mut v);
        }
        let posted = v.level_counts().posted;
        assert!(v.install(p.to_path_buf(), solid_decoded(3000, 3000, 0x22)));
        assert_eq!(
            v.level_counts().posted - posted,
            1,
            "重装只投最终编辑源一次"
        );
        wait_for_level(&mut v);
        assert_eq!(v.level_counts().posted - posted, 1);
        assert_eq!(v.level_counts().discarded, 0, "不能先投普通源再作废");
        let e = v.whole_image_for_plugin().unwrap();
        assert_eq!((e.width, e.height), (2000, 2000));
        assert_eq!(e.rgba[0], 0xF0);
        assert!(v.image().unwrap().rgba.iter().all(|b| *b == 0xF0));
        assert_eq!(v.loaded.as_ref().unwrap().natural, (3000, 3000));
        assert!(v.revert_edited(p));
        assert_eq!(v.whole_image_for_plugin().unwrap().rgba[0], 0x40);
    }

    #[test]
    fn large_edit_rejects_completed_old_level_before_new_lands() {
        let p = Path::new("large-edit-race.png");
        let mut v = viewer_with(4, 4, p);
        v.on_viewport(0.0, 0.0, 80.0, 80.0, 1.0);
        v.rotate_cw();
        assert!(v.post_level((2, 2)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while v.level_counts().computed == 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let old_image = v.image().unwrap().gen;
        let posted = v.level_counts().posted;
        assert!(v.install_edited(p, 2000, 2000, vec![0xF0; 2000 * 2000 * 4]));
        assert_eq!(v.level_counts().posted - posted, 1);
        assert_eq!(v.image().unwrap().gen, old_image);
        wait_for_level(&mut v);
        assert_eq!(v.level_counts().discarded, 1);
        assert!(v.image().unwrap().rgba.iter().all(|b| *b == 0xF0));
        assert_eq!((v.rot, v.flip), (1, false));
    }

    #[test]
    fn registering_other_path_preserves_current_level() {
        let p = Path::new("current-edit.png");
        let other = Path::new("other-edit.png");
        let mut v = viewer_with(4, 4, p);
        v.on_viewport(0.0, 0.0, 20.0, 20.0, 1.0);
        assert!(v.post_level((2, 2)));
        let epoch = v.epoch;
        let inflight = v.inflight;
        assert!(!v.install_edited(other, 4, 4, vec![0xF0; 64]));
        assert_eq!(v.epoch, epoch);
        assert_eq!(v.inflight, inflight);
        wait_for_level(&mut v);
        assert_eq!(v.level_counts().discarded, 0);
        assert!(v.install(other.to_path_buf(), solid_decoded(4, 4, 0x22)));
        assert_eq!(v.image().unwrap().rgba[0], 0xF0);
        assert!(v.revert_edited(other));
        assert_eq!(
            v.image().unwrap().rgba[0],
            0x22,
            "首次装载必须补抓 original"
        );
    }

    #[test]
    fn edited_overlay_swaps_and_reverts() {
        let dir =
            crate::core::paths::test_scratch_root().join(format!("vp_edit_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("a.png");
        let mut v = viewer_with(4, 4, &p);
        assert!(!v.is_edited(&p));
        let edited = vec![0xF0u8; 4 * 4 * 4];
        assert!(v.install_edited(&p, 4, 4, edited.clone()));
        assert!(v.is_edited(&p));
        assert_eq!(
            v.loaded.as_ref().expect("loaded").base_rgba.as_slice(),
            &edited
        );
        assert!(v.revert_edited(&p));
        assert!(!v.is_edited(&p));
        assert_eq!(
            v.loaded.as_ref().expect("loaded").base_rgba[0],
            0x40,
            "还原后应回到原始 base"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlay_survives_reinstall_and_rejects_bad_size() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("vp_edit2_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("b.png");
        let mut v = viewer_with(4, 4, &p);
        let edited = vec![0x11u8; 4 * 4 * 4];
        assert!(v.install_edited(&p, 4, 4, edited.clone()));
        // 模拟"导航离开再回来"：装载漏斗重新装一份原始解码（直接换 loaded，
        // 与 install 落点同形状），随后 apply_edited_overlay 应让覆盖重新生效。
        {
            let l = v.loaded.as_mut().expect("loaded");
            let px = vec![0x22u8; 4 * 4 * 4];
            l.base_rgba = Arc::new(px.clone());
            l.image = RgbaImage::new(4, 4, px);
            l.decoded = (4, 4);
            l.displayed = (4, 4);
        }
        v.apply_edited_overlay(&p);
        assert_eq!(
            v.loaded.as_ref().expect("loaded").base_rgba.as_slice(),
            &edited,
            "重装后覆盖应重新生效"
        );
        // 尺寸不匹配的回写一律拒绝。
        assert!(!v.install_edited(&p, 4, 4, vec![0; 3 * 3 * 4]));
        assert!(!v.install_edited(&p, 0, 4, Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
