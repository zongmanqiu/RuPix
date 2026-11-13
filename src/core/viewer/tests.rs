//! 状态机单测：67 条，全部离屏跑（不碰真 DLL、不开窗）。
//!
//! 每节顶上的注释写的是**这条用例在防什么回归**——改判据前先读它，
//! 那些数字（视口 74×56、605×605、4000×6000…）都是从真机事故里挑出来的形状。

use super::transform::*;
use super::*;

/// 显示 ↔ 图像的矩形映射在全部 rot/flip 组合下必须互逆。
///
/// 这是 SVG 可视区域渲染的坐标基石：请求用的逆映射和绘制用的正映射只要错开
/// 一档，切片就会画到错误的位置（表现为画面错位/镜像）。
#[test]
fn rect_map_display_and_image_are_inverses() {
    let (x0, y0, x1, y1) = (0.2, 0.1, 0.9, 0.8);
    for rot in 0..4 {
        for flip in [false, true] {
            let (dx0, dy0, dx1, dy1) = rect_map_display(rot, flip, x0, y0, x1, y1);
            let (ix0, iy0, ix1, iy1) = rect_map_image(rot, flip, dx0, dy0, dx1, dy1);
            let eps = 1e-5;
            assert!(
                (ix0 - x0).abs() < eps
                    && (iy0 - y0).abs() < eps
                    && (ix1 - x1).abs() < eps
                    && (iy1 - y1).abs() < eps,
                "rot={rot} flip={flip}: 显示→图像没能还原：({ix0},{iy0},{ix1},{iy1})"
            );
        }
    }
}

/// m332：倍率折算——1× 不改原速，加速档夹 16 ms，慢速档仍受 60 s 上限。
#[test]
fn anim_speed_scales_delay_and_floors_the_fast_end() {
    assert_eq!(anim_wait(100, AnimSpeed::X1).as_millis(), 100);
    assert_eq!(anim_wait(100, AnimSpeed::X2).as_millis(), 50);
    assert_eq!(
        anim_wait(100, AnimSpeed::X8).as_millis(),
        16,
        "8× 夹到 16 ms"
    );
    assert_eq!(anim_wait(100, AnimSpeed::X025).as_millis(), 400);
    assert_eq!(
        anim_wait(1, AnimSpeed::X1).as_millis(),
        1,
        "1× 不改现有手感"
    );
    assert_eq!(
        anim_wait(10, AnimSpeed::X8).as_millis(),
        10,
        "加速档不许把原本更快的帧反向拖慢"
    );
    assert_eq!(
        anim_wait(60_000, AnimSpeed::X025).as_millis(),
        60_000,
        "慢速仍受 60 s 上限"
    );
    assert_eq!(
        anim_wait(Anim::PAGE_DELAY_MS, AnimSpeed::X8).as_millis(),
        125,
        "分页文档切页也走同一条折算"
    );
}

/// m333：阅读状态要能回答"哪本书、哪一项、第几页、视图多大"。
#[test]
fn reading_state_records_book_item_page_and_view() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);
    v.open(&pa);
    let frame = Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index: 1,
            count: 4,
            delay_ms: 100,
            paged: true,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), frame));
    v.set_fit(false);
    v.zoom_at_center(2.0);

    let m = v.reading_state().expect("已上屏的当前项应能读出阅读状态");
    assert_eq!(m.key, crate::core::reading::file_key(&pa));
    assert_eq!(m.item, crate::core::reading::item_path(&pa));
    assert_eq!(m.index, 0);
    assert_eq!(m.page, 1);
    assert!(!m.fit);
    assert!((m.scale - 2.0).abs() < 0.01);
}

/// m333：待恢复的视图/页码只应用一次，并真的发出按号请求。
#[test]
fn pending_reading_restore_applies_view_and_page_once() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);
    v.open(&pa);
    let frame = Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index: 0,
            count: 4,
            delay_ms: 100,
            paged: true,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), frame));

    v.pending_restore = Some(crate::core::reading::Memento {
        key: crate::core::reading::file_key(&pa),
        item: crate::core::reading::item_path(&pa),
        index: 0,
        page: 2,
        fit: false,
        scale: 2.0,
        pan: (10.0, -5.0),
    });
    v.apply_pending_reading_restore(&pa);

    assert!(!v.is_fit(), "记忆是手动档，恢复后不能停在适应窗口");
    assert!((v.scale() - 2.0).abs() < 0.01, "倍率没恢复");
    assert_eq!(v.step_target, Some(2), "页码恢复必须发按号请求");
    assert!(v.pending_restore.is_none(), "同一份记忆只恢复一次");
}

/// m333：目录书按 item 找槽位；文件没了就整条作废，不按旧下标硬套。
#[test]
fn saved_directory_item_is_found_or_dropped() {
    let (dir, [_pa, pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&dir);
    let mut m = crate::core::reading::Memento {
        key: crate::core::reading::dir_key(&dir),
        item: crate::core::reading::item_path(&pb),
        index: 1,
        page: 0,
        fit: true,
        scale: 1.0,
        pan: (0.0, 0.0),
    };
    assert!(v.jump_to_saved_item(&m), "清单里还在的项要能找到");
    assert_eq!(
        v.gallery.as_ref().unwrap().position(),
        2,
        "b.svg 是第 2 项（position 1 起）"
    );

    m.item = crate::core::reading::item_path(&dir.join("gone.png"));
    assert!(!v.jump_to_saved_item(&m), "文件没了整条作废");
}

/// m332：Viewer 开局就按配置档位排表，别让首帧先按 1× 停一次。
#[test]
fn viewer_starts_at_the_configured_anim_speed() {
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let cfg = Config {
        anim_speed: AnimSpeed::X4,
        ..Config::default()
    };
    let v = Viewer::new(Rc::new(Decoder::spawn_with(tx)), Rc::new(RefCell::new(cfg)));
    assert_eq!(v.anim_speed(), AnimSpeed::X4);
}

/// m332：改速只重排当前帧之后的时刻，不重解当前帧；在途帧不许被重复排闸。
#[test]
fn anim_speed_reschedules_without_breaking_the_single_inflight_gate() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);
    let frame = Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index: 0,
            count: 4,
            delay_ms: 200,
            paged: true,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa, frame), "分页帧应装屏成功");
    let lead = |t: Instant| -> i64 {
        let now = Instant::now();
        if t > now {
            t.duration_since(now).as_millis() as i64
        } else {
            -(now.duration_since(t).as_millis() as i64)
        }
    };

    v.set_anim_speed(AnimSpeed::X2);
    let l = lead(v.anim_next_at.expect("无在途帧时应重排闸"));
    assert!(
        (30..=120).contains(&l),
        "2× 后 200 ms 应变 100 ms，实际 {l}"
    );

    v.set_anim_speed(AnimSpeed::X025);
    let l = lead(v.anim_next_at.expect("慢速档重排闸"));
    assert!(
        (700..=900).contains(&l),
        "0.25× 后 200 ms 应变 800 ms，实际 {l}"
    );

    // 模拟"已发请求、回包未到"：闸已清、时间线还在。改速只能作废时间线，
    // 不许重新立闸——否则旧回包落地前会再发一帧，违反单在途约束。
    v.anim_next_at = None;
    v.anim_due_at = Some(Instant::now() + Duration::from_millis(500));
    v.set_anim_speed(AnimSpeed::X8);
    assert!(
        v.anim_next_at.is_none() && v.anim_due_at.is_none(),
        "在途帧期间改速：闸保持空、旧时间线作废，等 install 重排"
    );
}

/// ★ 补丁 011 的承重前提：超过 100% 时上屏位图**封顶在原图档位**，而目标框比它大。
///
/// windui 侧那条"声明了 pixelated、且目标物理尺寸超出源像素才换最近邻"的判据，
/// 全靠这个几何关系成立。反向也要钉：100% 档框与位图相等，那里**不该**判成放大
/// ——正常看图不该出现锯齿。
///
/// 为什么值得单独钉：`derive_level` 的 `shrink` 放开成"允许插值放大"是一行改动，改完
/// 框与位图重新对齐、判据永不触发，画面又回到糊面——而且糊得"有据可依"（位图确实
/// 与框等大）。那还会顺带把 36 MP 的图放大成几百 MB。
#[test]
fn beyond_native_asks_the_renderer_to_upscale() {
    let mut v = viewer_with_image(); // 4×4 原图，无切片
    v.on_viewport(0.0, 0.0, 200.0, 200.0, 1.0);
    let content = RectI::new(0, 0, 200, 200);

    v.zoom_at_center(1.0);
    v.rebuild_image();
    {
        let img = v.image().unwrap();
        let dst = v.image_dst(content).unwrap();
        assert_eq!(
            (dst.w as u32, dst.h as u32),
            (img.width, img.height),
            "100% 档必须 1:1：目标框 {dst:?} 与位图 {}×{} 不等就会被判成放大，正常看图会带锯齿",
            img.width,
            img.height
        );
    }

    v.zoom_at_center(2.0);
    v.rebuild_image();
    {
        let img = v.image().unwrap();
        let dst = v.image_dst(content).unwrap();
        assert_eq!(
            (img.width, img.height),
            (4, 4),
            "位图被本地插值放大了——补丁 011 的判据永不触发，且要多吃几倍内存"
        );
        assert!(
            dst.w as u32 > img.width,
            "200% 档目标框应大于位图，实得框宽 {} vs 位图宽 {}",
            dst.w,
            img.width
        );
    }
}

// ── 切图不闪烁（2026-10-09 用户报障）────────────────────────
//
// 根因：reset_view 曾在翻页瞬间 `loaded = None`，旧图立刻消失，新图解码
// 完成前画布只剩背景——大图几十毫秒，肉眼就是一闪。修法：旧图留屏，
// 由 apply 原子替换。两条用例钉住「翻页不许清图」。

/// 造一个已装上合成位图（4×4）的 Viewer，不碰 WIC。
fn viewer_with_image() -> Viewer {
    use crate::core::decode::{Decoder, Done};
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    let (w, h) = (4u32, 4u32);
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.png"),
        out: Ok(Decoded {
            natural: (w, h),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: w,
                height: h,
                rgba: vec![0x40u8; (w * h * 4) as usize],
            },
        }),
    }));
    v
}

/// 造一个装上**指定尺寸**合成位图的 Viewer（离屏，不碰 WIC）。
///
/// m290 的几何用例要一张真的比视口大的图：4×4 那枚太小，`overscroll` 的退化分支
/// 会把结论吃掉。400×600 = 960 KB 内存，测试里完全可接受。
fn viewer_with_sized_image(w: u32, h: u32) -> Viewer {
    use crate::core::decode::{Decoder, Done};
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.png"),
        out: Ok(Decoded {
            natural: (w, h),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: w,
                height: h,
                rgba: vec![0x40u8; (w * h * 4) as usize],
            },
        }),
    }));
    v
}

/// 造一份**只用于字节账**的解码结果。
///
/// ★ `rgba` 故意不给满：[`Prefetched::bytes`] 的账只按声明尺寸 `w*h*4` 算，
/// 真按 4K 满屏分配就得在单测里吃掉几百 MB——而这里要验的是**淘汰判据**，不是像素。
fn decoded_sized(w: u32, h: u32) -> Decoded {
    use crate::core::pixels::Bitmap;
    Decoded {
        natural: (w, h),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: w,
            height: h,
            rgba: vec![0u8; 16],
        },
    }
}

/// 预读窗口跟着翻页方向走（m34）：刚往前翻就把前面 N 张排在最前，
/// 反方向留一格保「A→B→A」；方向未定时退回对称交错。
/// m281：N 来自 ini `prefetch_depth`（出厂默认 3），这里按默认值写期望；
/// 「手改 ini 会不会真的跟着变」由 `prefetch_depth_comes_from_config_*` 钉住。
#[test]
fn prefetch_window_follows_last_direction() {
    let mut v = viewer_with_image();
    v.last_dir = 1;
    assert_eq!(
        v.prefetch_offsets(),
        vec![1, 2, 3, -1],
        "前进：同方向三格 + 反方向一格"
    );
    v.last_dir = -1;
    assert_eq!(v.prefetch_offsets(), vec![-1, -2, -3, 1], "后退：镜像");
    v.last_dir = 0;
    assert_eq!(
        v.prefetch_offsets(),
        vec![1, -1, 2, -2, 3, -3],
        "未定向：两边都不赌"
    );
}

/// 条数闸：超过 [`PREFETCH_CAP`] 淘汰最旧的，留下最近的那几份。
#[test]
fn prefetch_budget_eviction_stops_automatic_refill() {
    let (dir, [pa, pb, pc]) = reopen_dir("budget-stop");
    let pd = dir.join("d.svg");
    std::fs::write(&pd, b"").unwrap();
    let mut v = viewer_with_image();
    v.gallery = Some(Gallery::from_file(&pa, SortSpec::DEFAULT).unwrap());
    v.view = (0.0, 0.0, 5120.0, 5120.0);
    v.last_dir = 1;
    v.cache_prefetch(pb, decoded_sized(5120, 5120));
    v.cache_prefetch(pc, decoded_sized(5120, 5120));
    v.prefetch_seq = Some(91);
    assert!(!v.apply(Done {
        seq: 91,
        path: pd,
        out: Ok(decoded_sized(5120, 5120)),
    }));
    assert_eq!(v.prefetched.len(), 2);
    assert!(v.prefetch_bytes() < PREFETCH_BYTES);
    assert!(v.prefetch_seq.is_none(), "淘汰后不能立即重解被淘汰的邻图");
    for index in 1..=2 {
        let mut frame = decoded_sized(2, 2);
        frame.anim = Anim {
            index,
            count: 3,
            delay_ms: 40,
            paged: false,
        };
        assert!(v.apply(Done {
            seq: v.want_seq,
            path: pa.clone(),
            out: Ok(frame)
        }));
        assert!(v.prefetch_seq.is_none(), "同图动画回包不能重启已停填的规划");
    }
    v.request_prefetch();
    assert!(v.prefetch_seq.is_none(), "统一入口也必须尊重本轮停填");
    assert!(v.step(1)); // b 已被淘汰，真实导航重启邻图规划，不消费假像素缓存。
    assert!(v.prefetch_seq.is_some(), "后续导航仍能发起新的预读规划");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn prefetch_cache_caps_by_count() {
    let mut v = viewer_with_image();
    let total = PREFETCH_CAP + 2;
    for i in 0..total {
        v.cache_prefetch(PathBuf::from(format!("p{i}.png")), decoded_sized(8, 8));
    }
    assert_eq!(v.prefetched.len(), PREFETCH_CAP);
    assert_eq!(
        v.prefetched.last().map(|e| e.path.as_path()),
        Some(PathBuf::from(format!("p{}.png", total - 1)).as_path()),
        "队尾必须是最新的那份（LRU 顺序）"
    );
}

/// 字节闸：**淘汰最旧的**，但**绝不把缓存清空**——单份就超预算时若也淘汰，
/// 那条闸就成了"关掉预读"（原注释警告过的坑）。
#[test]
fn prefetch_cache_caps_by_bytes_but_never_empties() {
    let mut v = viewer_with_image();
    // 每份声明 16384×8192×4 = 512 MiB，两份即超 256 MiB 预算。
    v.cache_prefetch(PathBuf::from("a.png"), decoded_sized(16384, 8192));
    assert_eq!(
        v.prefetched.len(),
        1,
        "单份超预算不许淘汰：清空缓存等于从此不再预读"
    );
    v.cache_prefetch(PathBuf::from("b.png"), decoded_sized(16384, 8192));
    assert_eq!(v.prefetched.len(), 1, "第二份进来后应淘汰掉最旧的");
    assert_eq!(
        v.prefetched[0].path,
        PathBuf::from("b.png"),
        "被淘汰的必须是先来的 a"
    );
}
/// 造一份"预读回来的那一档"：`natural` 是原图尺寸，位图是按 `box` 装进去的结果。
/// 与 [`decoded_sized`] 的分工：那份只算字节账（natural == 位图），预读缓存里那份
/// **天生比原图小**，档位问题只有这份测得出来。
fn prefetched_tier(path: PathBuf, natural: (u32, u32), box_w: u32, box_h: u32) -> Prefetched {
    use crate::core::pixels::Bitmap;
    let (w, h) = fit_dims(natural.0, natural.1, box_w, box_h);
    Prefetched {
        path,
        img: Decoded {
            natural,
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: w,
                height: h,
                rgba: vec![0u8; (w * h * 4) as usize],
            },
        },
    }
}

/// 建一个"a 已进清单"的 viewer，视口取 74×56（≈800×600 窗口去掉边栏再缩 10 倍，
/// 单测里不想分配 1 MB 位图）。
fn viewer_in_dir(first: &Path) -> Viewer {
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(first);
    v.on_viewport(0.0, 0.0, 74.0, 56.0, 1.0);
    v
}

/// ★ 借还是抄（m106）：`gallery_dir()` 被 runtime 每 16 ms 泵一次，只为和"上一次监控的
/// 那本目录"比一句相等不相等——目录几乎从不真变，旧形状却每拍先抄一份 `PathBuf`。
/// 判据落在**两次读交回同一块字节**：把形状退回"抄一份"，两条指针必然不同
/// （两份都活着，分配器不可能给同一个地址）。
#[test]
fn gallery_dir_lends_the_listing_s_copy() {
    let (dir, [pa, _pb, _pc]) = gallery_dir();
    let mut v = viewer_in_dir(&pa);
    let first = v.gallery_dir();
    let second = v.gallery_dir();
    assert_eq!(first, Some(dir.as_path()), "交出来的要就是清单里那本目录");
    assert!(
        std::ptr::eq(first.unwrap(), second.unwrap()),
        "两次读必须是同一块字节：每 16 ms 抄一份 PathBuf 就是白抄"
    );
    // 没有清单 ⇒ `None`（runtime 据此撤掉 watcher，而不是继续监控上一本的目录）
    v.gallery = None;
    assert_eq!(v.gallery_dir(), None, "清单清空后不许留着旧目录的引用");
}

/// ★ 混尺寸目录里的预读必须真能吃下——旧判据拿**框的两条边**比，而 fit 从不裁切、
/// 结果永远只有一条边贴住框，于是要求"图片长宽比 == 窗口长宽比"。真机实测 6 张混
/// 尺寸翻 12 次：缓存里有、被判据驳回重解 10 次，只有那张 4:3 恰好撞上 4:3 的窗口
/// 才活下来（m38）。这里用方图 605×605 进 74×56 的横框，就是那 10 次里的形状。
#[test]
fn prefetch_hit_survives_aspect_mismatch() {
    let (_dir, [pa, pb, _pc]) = gallery_dir();
    let mut v = viewer_in_dir(&pa);
    v.prefetched
        .push(prefetched_tier(pb.clone(), (605, 605), 74, 56));
    assert_eq!(
        v.prefetched[0].img.bitmap.width, 56,
        "样本得是'短边贴框'那种，否则测不到判据"
    );
    let seq_before = v.want_seq;
    assert!(v.step(1));
    assert_eq!(
        v.want_seq, seq_before,
        "预读那份就是这一张按视口框该有的档，不许驳回重解"
    );
    assert_eq!(v.loaded.as_ref().unwrap().path, pb, "命中要直接上屏");
}

/// 上面那条判据不能把"窗口被拉大了"也一并放过：那份确实只够四分之一面积，必须重解。
#[test]
fn prefetch_dropped_when_viewport_grew() {
    let (_dir, [pa, pb, _pc]) = gallery_dir();
    let mut v = viewer_in_dir(&pa);
    // 预读发生在半大窗口（37×28 → 方图解出 28×28），翻过去时窗口已经是 74×56。
    v.prefetched
        .push(prefetched_tier(pb.clone(), (605, 605), 37, 28));
    let seq_before = v.want_seq;
    assert!(v.step(1));
    assert!(
        v.want_seq > seq_before,
        "窗口翻倍后那份不够清晰，必须发重解请求"
    );
    assert!(
        v.prefetched.is_empty(),
        "驳回的那份要扔掉，不许留在缓存里充数"
    );
}

/// ★ 换图首解的请求框必须是**视口**，不能是上一张的显示尺寸。`loaded` 为了防闪
/// 故意还停在上一张，`natural × fit 倍率` 算出来的是**上一张**的框：实测 748×561
/// 窗口里紧跟在 4000×6000 后面的 6400×3600 只解到 374×210（本该 748×421），
/// 首屏白糊一倍，还把预读那份一起带偏成"驳回→重解成更小的档"。
#[test]
fn first_decode_after_flip_asks_viewport_not_previous_image() {
    use crate::core::pixels::Bitmap;
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let mut v = viewer_in_dir(&pa);
    let seq = v.want_seq;
    assert!(v.apply(Done {
        seq,
        path: pa.clone(),
        out: Ok(Decoded {
            natural: (40, 60),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 37,
                height: 56,
                rgba: vec![0u8; (37 * 56 * 4) as usize],
            },
        }),
    }));
    // 先证这个样本能让旧算法和新算法给出不同答案，否则断言是空跑的（K54）。
    let previous_box = fit_dims(40, 60, 74, 56);
    assert_ne!(
        previous_box,
        (74, 56),
        "样本不成立：换一组长宽比差得更大的尺寸"
    );
    assert!(v.step(1), "b 没预读，必须发解码请求");
    assert_eq!(
        v.desired_pixels(),
        (74, 56),
        "此刻 loaded 还是上一张（{previous_box:?} 那一档），首解要按视口框"
    );
}

/// ★ 递到解码器 ABI 上的解码目标**不许是饱和出来的天文数字**。拿真输入的形状测：
/// 1×40 MP 的长条图（过得了 [`MAX_PIXELS`](crate::core::pixels::MAX_PIXELS) 总量闸、
/// 单边今天没有闸）× 64 倍 × dpi 2 = 5.1e9，而 `f32 as u32` 是**饱和**转换 → 目标变成
/// `u32::MAX`，等于对解码器说"给我 42 亿像素宽"。夹到原生后目标就停在原生，行为不变
/// （[`fit_dims`](crate::core::geometry::fit_dims) 从不放大，解码器也变不出比原图更多的像素）。
/// 位图故意给 4×4：这条测的是**算术**，不是解码，别在单测里alloc 160 MB。
#[test]
fn decode_target_is_clamped_before_it_is_cast() {
    use crate::core::decode::{Decoder, Done};
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    let nat = (1u32, 40_000_000u32);
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("long.png"),
        out: Ok(Decoded {
            natural: nat,
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 4,
                height: 4,
                rgba: vec![0u8; 64],
            },
        }),
    }));
    v.fit = false;
    v.zoom = MAX_ZOOM;
    v.dpi = 2.0;
    // 先证这个样本**真的**会溢出，否则下面那条断言是空跑的（K54）。
    let raw = nat.1 as f64 * MAX_ZOOM as f64 * 2.0;
    assert!(raw > u32::MAX as f64, "样本不成立了：换一条真能溢出的");
    assert_eq!(
        v.desired_pixels(),
        nat,
        "目标要停在原生，不是饱和成 u32::MAX"
    );
}

/// ★ 「邻图预读」开关必须真接上（ini `prefetch=0`）：关着就不许再发起任何预读。
///
/// 两条断言成对写：只测"关掉不发"，那条路可能本来就因为别的闸不通（空跑）；
/// 第二半把开关打开、要求它**确实发出去**，才证明第一半测的是开关而不是巧合（K54）。
#[test]
fn prefetch_switch_gates_new_requests() {
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let cfg = Rc::new(RefCell::new(Config::default()));
    let mut v = Viewer::new(Rc::new(Decoder::spawn_with(tx)), cfg.clone());
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 74.0, 56.0, 1.0);
    cfg.borrow_mut().prefetch = false;
    v.request_prefetch();
    assert!(
        v.prefetch_seq.is_none(),
        "开关关着还发预读 = ini 那一条没接上消费方"
    );
    cfg.borrow_mut().prefetch = true;
    v.request_prefetch();
    assert!(v.prefetch_seq.is_some(), "开着就必须发得出去");
}

/// 普通分支：reset_view（step 内部调它）后旧图必须还在——切图不许闪背景。
///
/// 朝向归零只落在**目标**上（m72）：屏上旧图保持它自己转着的样子，直到新图
/// `install` 原子替换。在 reset_view 里同步重建旧图，100% 档要白卡一百多毫秒，
/// 而这份像素马上就会被丢掉。
#[test]
fn switching_keeps_previous_image_on_screen() {
    use crate::core::decode::Done;
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let mut v = viewer_with_image();
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    v.rotate_cw();
    assert_eq!((v.rot, v.flip), (1, false), "4×4 小图应同步转好");
    v.reset_view();
    assert!(
        v.loaded.is_some(),
        "reset_view 清掉了旧图——切图就会闪背景（本次报障的根因）"
    );
    assert_eq!((v.want_rot, v.want_flip), (0, false), "目标朝向必须归零");
    assert_eq!(
        (v.rot, v.flip),
        (1, false),
        "屏上旧图还转着，几何必须按它算，否则旧图被拉伸"
    );

    // 新图落地（install 是权威替换点）：位图按归零后的朝向来。
    assert!(v.apply(Done {
        seq: v.want_seq,
        path: std::path::PathBuf::from("b.png"),
        out: Ok(Decoded {
            natural: (6, 3),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 6,
                height: 3,
                rgba: vec![0x40u8; 6 * 3 * 4],
            },
        }),
    }));
    assert_eq!((v.rot, v.flip), (0, false), "新图必须是未变换朝向");
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(
        (l.image.width, l.image.height),
        l.decoded,
        "新图的上屏位图必须是未变换形状"
    );
}

/// 溢出上限的三条分支（纯函数，四档都钉）：大图、小图、比 `PAN_KEEP_VISIBLE` 还小的图、
/// 以及"没有视口/没有图"。这条线就是"图边能不能越过窗口边、最多越多少"。
#[test]
fn overscroll_keeps_a_sliver_and_never_the_whole_window() {
    use super::zoom::overscroll;
    // 大图 1000 落在 400 的窗口里：旧口径是 (1000-400)/2 = 300（永远不许露黑边），
    // 新口径允许把图一直拖到"只剩 24dp 衣角"⇒ (400+1000)/2 - 24 = 676。
    assert_eq!(overscroll(400.0, 1000.0), 676.0, "大图：图边可以离开窗口边");
    // 小图 100 落在 400 里：(400+100)/2 - 24 = 226。
    assert_eq!(overscroll(400.0, 100.0), 226.0, "小图：同样能拖到只剩衣角");
    // 图比衣角还小（10 < 24）：留不下 24，就整张留在画面里 ⇒ 上限退回"图边贴窗口边"。
    assert_eq!(
        overscroll(400.0, 10.0),
        195.0,
        "10dp 的小图：keep 退化成图宽，整张不许出画面"
    );
    // 窗口比衣角还窄（20 < 24）：同理退化。
    assert_eq!(
        overscroll(20.0, 1000.0),
        490.0,
        "窗口 20dp：keep 退化成窗口宽"
    );
    // 没有窗口或没有图 ⇒ 不动。
    assert_eq!(overscroll(0.0, 100.0), 0.0);
    assert_eq!(overscroll(400.0, 0.0), 0.0);
}

/// ★ m290：标题栏是**浮层**（画在画布顶部之上），而平移上限用的 `view` 含它那一块
/// ⇒ 旧口径把「看图上缘」那一侧的 24dp 衣角推到画布顶 24dp 处，正好落在标题栏底下，
/// 画面看起来整块全空（用户 2026-11-09 拍板要做的那条）。
///
/// 本条钉住修正后的几何：无边框 / 全屏（不画标题栏）时**一个像素都不许变**；
/// 画标题栏时那一侧要多让出标题栏的高度，另一侧（看图下缘）**不许被牵连**。
#[test]
fn pan_limit_keeps_the_corner_below_the_title_bar() {
    let mut v = viewer_with_sized_image(400, 600);
    v.fit = false;
    v.zoom = 1.0;
    v.view = (0.0, 0.0, 200.0, 300.0); // 图 400×600，比视口大一倍

    // ① 没有标题栏：旧口径。span = (300 + 600)/2 - 24 = 426。
    let (_, ry0) = v.pan_limits();
    assert_eq!(ry0.min, -426.0, "无标题栏时下限就是对称口径");
    assert_eq!(ry0.max, 426.0, "无标题栏时上限就是对称口径");
    let edge0 = 300.0 / 2.0 + ry0.min + 600.0 / 2.0;
    assert_eq!(edge0, 24.0, "无标题栏：衣角落在画布顶 24dp（老口径）");

    // ② 有标题栏（40dp）：下限抬高 40，图下缘落到 24 + 40 = 64。
    v.set_chrome_top(40.0);
    let (_, ry1) = v.pan_limits();
    assert_eq!(ry1.min, -386.0, "标题栏 40dp：下限要多让出 40");
    assert_eq!(ry1.max, 426.0, "另一侧（看图下缘）一个像素都不许动");
    let edge1 = 300.0 / 2.0 + ry1.min + 600.0 / 2.0;
    assert_eq!(edge1, 64.0, "衣角必须落在标题栏下沿之下 24dp 处");
    let top1 = 300.0 / 2.0 + ry1.max - 600.0 / 2.0;
    assert_eq!(top1, 276.0, "看图下缘那一侧不受影响");
}

/// ★ m290：夹取必须用**新的非对称区间**——上限改了而夹取还是老的，衣角照样被吃掉。
#[test]
fn clamp_pan_respects_the_title_bar_aware_range() {
    let mut v = viewer_with_sized_image(400, 600);
    v.fit = false;
    v.zoom = 1.0;
    v.view = (0.0, 0.0, 200.0, 300.0);
    v.set_chrome_top(40.0);
    v.pan.1 = -9999.0;
    v.clamp_pan();
    assert_eq!(v.pan.1, -386.0, "夹取必须落在标题栏感知过的下限上");

    // 标题栏收掉（切无边框 / 退出全屏）：区间放宽，但图**不该自己跳**——
    // 旧位置在新区间里仍然合法，跳一下反而是惊吓。
    v.set_chrome_top(0.0);
    assert_eq!(v.pan.1, -386.0, "区间放宽后不该把图拽走");

    // 反过来：标题栏长出来时区间收紧，**必须立刻重夹**，不许停在越界位置上
    // （否则那枚衣角还在标题栏底下，直到用户下一次拖动才"跳"一下）。
    let mut w = viewer_with_sized_image(400, 600);
    w.fit = false;
    w.zoom = 1.0;
    w.view = (0.0, 0.0, 200.0, 300.0);
    w.pan.1 = -9999.0;
    w.clamp_pan();
    assert_eq!(w.pan.1, -426.0, "无标题栏时的下限");
    w.set_chrome_top(40.0);
    assert_eq!(w.pan.1, -386.0, "标题栏长出来时必须立刻重夹");
}

/// 纯函数边界：`head` 大到离谱时不能把区间收成「必须往一边挪」。
#[test]
fn pan_range_head_is_clamped_to_the_travel() {
    use super::zoom::PanRange;
    let r = PanRange::with_head(10.0, 1000.0);
    assert_eq!(r.min, 10.0, "head 超过整段行程时按一半算");
    assert_eq!(r.max, 10.0);
    assert!(r.min <= r.max, "区间不许倒挂");
    // head = 0 ⇒ 与对称口径逐值相同（无边框 / 全屏那两种形态不许有任何行为变化）。
    let z = PanRange::with_head(10.0, 0.0);
    assert_eq!(z, PanRange::symmetric(10.0), "head = 0 必须等于对称口径");
}

/// `can_pan` 必须与 `clamp_pan` 是**同一份几何**——它是左键手势分工的判据之一
/// （`ui::canvas::left_drag` 的输入）。两处各算一遍就会出现"判成可拖、按下去画面
/// 一动不动"，或反过来"没有图时把拖窗口这条路堵死"。
///
/// 2026-11-03 用户第三、第四条口径连着改了两回：先"任何情况下都要能拖动"（小图也算），
/// 再"允许图边离开窗口边"（上限从差值的一半改成并集的一半减衣角）。
#[test]
fn can_pan_tracks_the_room_clamp_pan_uses() {
    let mut v = viewer_with_image(); // 4×4 位图
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    // 适应窗口（4×4 铺满 100×100）：旧口径在这里是 0（无处可挪），新口径 = 100 - 24 = 76。
    assert!(
        v.can_pan(),
        "图与视口一样大也该拖得动（第四条口径露出图边）"
    );
    let _ = v.set_pan((1.0e6, 0.0));
    assert_eq!(v.pan().0, 76.0, "上限 = (视口+图)/2 - 衣角 = 76");

    // 图比视口小（50×50 落在 100×100 里）
    v.zoom_at_center(12.5);
    assert!(v.can_pan(), "图比视口小也要拖得动（第三条口径）");
    let _ = v.set_pan((1.0e6, 0.0));
    assert_eq!(v.pan().0, 51.0, "小图上限 = (100+50)/2 - 24 = 51");

    // 图比视口大（256 落在 100 里）：旧口径 78，新口径 154——多出来的那一段就是"图边离开窗口边"
    v.zoom_at_center(64.0);
    let _ = v.set_pan((1.0e7, 0.0));
    assert_eq!(
        v.pan().0,
        154.0,
        "大图上限 = (100+256)/2 - 24 = 154（旧口径是 78）"
    );

    // 首开空窗（还没有图）：那一按必须还能拖窗口。
    let (tx, _rx) = std::sync::mpsc::channel::<crate::core::decode::Done>();
    let empty = Viewer::new(
        Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(!empty.can_pan(), "没有图就没有可拖的东西");
}

/// `overflows` 判的是"有没有看不见的部分"，与 `can_pan`（拖得动）**不是一把尺子**。
///
/// 唯一消费方是无边框形态的左键分工（m272）：那个形态没有标题栏、整块画面搬窗口，
/// 只有图真溢出视口（按 `1` 到 100% / 放大过）才轮到平移。判错方向的两个后果都很难受——
/// 常态下搬不动窗口（用户原话「没有任何提示」），或者放大后拖不动图。
#[test]
fn overflows_tells_a_fitted_image_apart_from_a_zoomed_one() {
    let mut v = viewer_with_image(); // 4×4 位图
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    assert!(
        !v.overflows(),
        "适应窗口：4×4 铺满 100×100，一个像素都不在画外"
    );
    assert!(
        v.can_pan(),
        "同一刻仍然拖得动——两把尺子各说各的事（overscroll 允许拖到只剩衣角）"
    );

    v.zoom_at_center(12.5);
    assert!(!v.overflows(), "缩到 50×50：整张都在画面里");

    v.zoom_at_center(64.0);
    assert!(
        v.overflows(),
        "放到 256×256：两边都溢出，这时才有画外的东西"
    );

    // 空窗：没有图就没有"溢出"可言。
    let (tx, _rx) = std::sync::mpsc::channel::<crate::core::decode::Done>();
    let empty = Viewer::new(
        Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(!empty.overflows(), "没有图就谈不上溢出");
}

/// m216：WASD 平移的步进。三件事各钉一条：
/// ① 步长有下限（视口小到 100 px 时按 24 px 走，不是 12.5）；
/// ② 平移范围与拖动共用同一份钳位（`set_pan` → `clamp_pan` → `pan_limits`，无第二正本）；
/// ③ 已在边上时步进不许报"变了"（apply_pan 的比较纪律，M6 老账）。
#[test]
fn wasd_pan_nudge_shares_clamp_geometry() {
    let mut v = viewer_with_image(); // 4×4 位图
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    // 适应窗口后 pan 回零（夹具装图即适应）。
    assert!(v.pan_nudge(0.0, 1.0), "第一记 W 该平移（看上方）");
    assert_eq!(v.pan().1, 24.0, "步长 = 视口短边 100/8=12.5，下限 24 兜底");
    assert_eq!(v.pan().0, 0.0, "纵向平移不许带动横向");

    // 推到极限的落点与 set_pan(大数) 的钳位一致（= 76，见 can_pan 用例）：
    // 平移走的就是 set_pan 这一条路，范围天然与拖动一致。
    let _ = v.set_pan((0.0, 1.0e6));
    assert_eq!(v.pan().1, 76.0);

    // 在边上：再步进不许报"变了"（不比较就是每按一下白刷一整帧）。
    assert!(!v.pan_nudge(0.0, 1.0), "已在下极限，向上看无路可走");
    // 反方向仍然能动。
    assert!(v.pan_nudge(0.0, -1.0), "反向平移不受极限影响");
}

/// m216 符号约定第二正本：`sy = +1`（W）必须让 pan.y **增大**（图像下移 = 视口看上方）。
/// 写反的症状：按 W 画面往上飘、看的是图的下缘，与"连按到顶部"的承诺背道而驰。
#[test]
fn wasd_nudge_signs_follow_viewport_direction() {
    let mut v = viewer_with_image();
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let base = v.pan();
    let _ = v.pan_nudge(1.0, 0.0); // A：左看
    assert!(v.pan().0 > base.0, "A（左看）该让 pan.x 增大（图像右移）");
    let _ = v.pan_nudge(-1.0, 0.0);
    let _ = v.pan_nudge(-1.0, 0.0); // D：右看
    assert!(v.pan().0 < base.0, "D（右看）该让 pan.x 减小（图像左移）");
}

/// 平移（拖图 / 鸟瞰导航）必须给 SVG 安排可视区域重渲；位图不该触发解码。
///
/// 2026-10-10 用户报障：放大 SVG 后在鸟瞰图拖视角，拖出旧位图 + 30% 边距的
/// 范围一片空白——根因是 set_pan/drag_to 只改 pan 不调度重渲，没有后续解码。
#[test]
fn panning_schedules_region_rerender_for_vector_only() {
    use crate::core::pixels::{Anim, Bitmap};
    // SVG（is_vector 按扩展名判定）。
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut s = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(s.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.svg"),
        out: Ok(Decoded {
            natural: (4000, 3000),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 4,
                height: 4,
                rgba: vec![0x40u8; 64]
            },
        }),
    }));
    s.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    // 先放大再拖：适应窗口时 4000×3000 正好铺满 100×100 的视口，pan 被钳在
    // (0,0)，那种几何下"平移"根本不存在——M6 之后 `set_pan` 会照实返回 false。
    s.zoom_at_center(8.0);
    s.need_at = None;
    assert!(
        s.set_pan((10.0, 0.0)),
        "放大后 pan 才动得起来，本例的前提是它真的变了"
    );
    assert!(s.need_at.is_some(), "SVG 平移后必须安排可视区域重渲");
    // ★ M6 的另一半：同一个 pan 再来一次（拖到边上、鼠标还在动）不算变化，
    // 既不许标脏也不许调度重渲——那正是"拖鸟瞰时 CPU 5–6%"里的纯浪费段。
    s.need_at = None;
    assert!(!s.set_pan((10.0, 0.0)), "pan 一个没变，返回值必须是 false");
    assert!(
        s.need_at.is_none(),
        "画面没动就不该再安排重渲（整窗重绘与解码都省在这一句上）"
    );
    s.need_at = None;
    assert!(
        s.set_pan((1.0e6, 0.0)),
        "从 (10,0) 钳到边界 (350,0) 是移动，得算变化"
    );
    // 已经在边界上、鼠标继续往外拖：这才是 M6 要省掉的那一段空转。
    s.need_at = None;
    assert!(
        !s.set_pan((1.0e7, 0.0)),
        "钳位后位置没动，返回值必须是 false"
    );
    assert!(s.need_at.is_none(), "钳位空转不许调度重渲");

    // 位图：整图本来就在内存里，平移不许触发解码。
    let mut b = viewer_with_image();
    b.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    b.zoom_at_center(64.0); // 4×4 放大到 256，才谈得上拖
    b.need_at = None;
    assert!(
        b.set_pan((10.0, 0.0)),
        "本例的前提同样是 pan 真的动了，否则测的是钳位不是位图语义"
    );
    assert!(b.need_at.is_none(), "位图平移不该触发解码");
}

/// **没有区域通道的矢量**（解码器只能整幅渲染）**不许**走"可视区域 1:1 重渲"。
///
/// ★ 这不是"糊一点"的问题，是**内容错误**：没有区域接口的解码器，宿主若按区域去要，
/// 它只能把**整篇**缩进那一小块——画面看着像图，其实内容全错。
/// 判据真源在 `core::formats::is_region_renderable`。
#[test]
fn vector_without_region_channel_does_not_schedule_region_rerender() {
    use crate::core::pixels::{Anim, Bitmap};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut s = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(s.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.emf"),
        out: Ok(Decoded {
            natural: (4000, 3000),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 4,
                height: 4,
                rgba: vec![0x40u8; 64]
            },
        }),
    }));
    s.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    s.zoom_at_center(8.0);

    // ① 解码器**自己按 target 渲染**、**没有** `iv_render_region` → 不许安排区域重渲。
    {
        let _guard = crate::core::formats::test_override::set(&["emf"], &[]);
        s.need_at = None;
        assert!(
            s.set_pan((10.0, 0.0)),
            "本例的前提是 pan 真的动了，否则测的是钳位不是路由"
        );
        assert!(
            s.need_at.is_none(),
            "没有区域接口的矢量：按区域去要会拿到'整篇缩到那一小块'的内容错误图"
        );
    }

    // ② 对照组：同一后缀，但解码器**实现了 ABI v6 的 `iv_render_region`** →
    //    必须走区域重渲（这才是"放大到 4000% 也无损"的宿主侧开关）。
    {
        let _guard = crate::core::formats::test_override::set(&["emf"], &["emf"]);
        s.need_at = None;
        assert!(s.set_pan((20.0, 0.0)));
        assert!(
            s.need_at.is_some(),
            "有区域通道的自渲染矢量必须走可视区域 1:1 重渲（别把这条一起关掉）"
        );
    }

    // ③ 对照组：同后缀但**不是矢量**（压根不进矢量路线）→ 不安排矢量区域重渲。
    {
        let _guard = crate::core::formats::test_override::set(&[], &[]);
        s.need_at = None;
        assert!(s.set_pan((30.0, 0.0)));
        assert!(
            s.need_at.is_none(),
            "非矢量后缀不进矢量区域重渲（位图走缓存裁剪那条路）"
        );
    }
}

/// SVG 区域重渲后，鸟瞰必须还能拿到**整图**（2026-10-10 报障：鸟瞰缩略
/// 图画的当前切片，离视角远的部分空白）。整图来自首帧 whole 解码。
#[test]
fn region_rerender_keeps_whole_image_for_navigator() {
    use crate::core::pixels::{Anim, Bitmap};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    // 首帧 whole（100×75 缩略进 max 框）。
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.svg"),
        out: Ok(Decoded {
            natural: (4000, 3000),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 100,
                height: 75,
                rgba: vec![0x20u8; 100 * 75 * 4]
            },
        }),
    }));
    // 区域重渲接管主位图（seq 需要手动推进——正常由 request_region 发号）。
    v.want_seq = 1;
    assert!(v.apply(Done {
        seq: 1,
        path: std::path::PathBuf::from("a.svg"),
        out: Ok(Decoded {
            natural: (4000, 3000),
            region: Some(Region {
                origin: (0.0, 0.0),
                density: 1.0
            }),
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 50,
                height: 50,
                rgba: vec![0x60u8; 50 * 50 * 4]
            },
        }),
    }));
    let l = v.loaded.as_ref().unwrap();
    assert!(l.region.is_some(), "第二次 apply 应该是切片态");
    assert_eq!(
        l.nav_thumb_rgba.as_ref().map(|(_, w, h)| (*w, *h)),
        Some((100, 75)),
        "切片重渲不得覆盖鸟瞰整图缩略"
    );
    assert!(
        v.nav_image_dst(RectI::new(0, 0, 100, 100)).is_some(),
        "切片态下鸟瞰必须有整图可画"
    );
}

/// ★ **上屏位图必须降边到显卡纹理建得出来的尺寸**（2026-10-28）。
///
/// 解码上限抬到 64 MP 之后，"总量在限内、单边超限"的图出现了（30000×2000 的全景图
/// = 60 MP）。D3D11 建不出 16384 以上的单边 ⇒ 纹理创建失败 ⇒ 宿主当时的处理是
/// "本轮留白"⇒ 用户看到一片空白。所以显示位图要**主动等比降边**。
///
/// 这条钉的是**viewer 这条链**（`apply` → `install` → `rebuild_image` → 派生）真的降了，
/// 不是只测那个纯函数。
#[test]
fn oversized_bitmap_is_displayed_capped_to_texture_side() {
    use crate::core::pixels::{Anim, Bitmap, MAX_TEXTURE_SIDE};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    // 单边 20000（总量很小，测试不占内存）：解码合法，但超显卡纹理单边上限。
    let (w, h) = (20_000u32, 2u32);
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.png"),
        out: Ok(Decoded {
            natural: (w, h),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: w,
                height: h,
                rgba: vec![0x30u8; (w * h * 4) as usize]
            },
        }),
    }));
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(
        (l.decoded.0, l.decoded.1),
        (w, h),
        "解码档原样保留（内存里那份不小）"
    );
    // ★ 走到"真踩上限"那一档：手动 100% ⇒ 显示目标 = 自然尺寸 = 20000×2
    //   （适应窗口那档会缩到视口大小，降边根本没被触发 ⇒ 那样这条用例是假绿）。
    v.zoom_at_center(1.0);
    v.rebuild_image();
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(
        (l.decoded.0, l.decoded.1),
        (w, h),
        "解码档原样保留（内存里那份不小）"
    );
    assert_eq!(
        l.displayed,
        (MAX_TEXTURE_SIDE, 2),
        "100% 档必须正好降边到纹理上限（等比，短边不动）"
    );
    assert!(
        l.image.width <= MAX_TEXTURE_SIDE && l.image.height <= MAX_TEXTURE_SIDE,
        "真正上传的位图同样不许超限：{}x{}",
        l.image.width,
        l.image.height
    );
}

/// ★ 切片重渲之后，鸟瞰那份**来源**也不许把整尺寸缓冲钉在内存里。///
/// 首帧 whole 是"视口档"（4K 屏就是 3840×2160 ≈ 33 MB），旧写法把它原样存进
/// `nav_thumb_rgba`，之后主位图换成切片，那 33 MB 只为一个 ≤512 px 的鸟瞰活着、
/// 永不退场（8K ≈133 MB）。现在来源先缩到 512 档再存，画质一分不损——鸟瞰本来
/// 就只画那一档。
#[test]
fn nav_source_stays_small_after_a_region_rerender() {
    use crate::core::pixels::{Anim, Bitmap};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    let big = 2000u32;
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.svg"),
        out: Ok(Decoded {
            natural: (big, big),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: big,
                height: big,
                rgba: vec![0x20u8; (big * big * 4) as usize],
            },
        }),
    }));
    let (sw, sh) = v
        .loaded
        .as_ref()
        .unwrap()
        .nav_thumb_rgba
        .as_ref()
        .map(|(_, w, h)| (*w, *h))
        .expect("首帧 whole 该收下鸟瞰来源");
    assert_eq!((sw, sh), (512, 512), "来源必须先进 512 框，实际 {sw}×{sh}");
    // 切片接管主位图之后，来源仍是那份小的（旧写法：整图那份被永久钉住）。
    v.want_seq = 1;
    assert!(v.apply(Done {
        seq: 1,
        path: std::path::PathBuf::from("a.svg"),
        out: Ok(Decoded {
            natural: (big, big),
            region: Some(Region {
                origin: (0.0, 0.0),
                density: 1.0,
            }),
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 50,
                height: 50,
                rgba: vec![0x60u8; 50 * 50 * 4],
            },
        }),
    }));
    let l = v.loaded.as_ref().unwrap();
    let (tw, th) = l.nav_thumb_rgba.as_ref().map(|(_, w, h)| (*w, *h)).unwrap();
    assert_eq!((tw, th), (512, 512), "切片态继承的还是那份缩略");
    assert!(
        l.nav_thumb_rgba.as_ref().unwrap().0.len()
            <= (NAV_THUMB_MAX as usize * NAV_THUMB_MAX as usize * 4),
        "来源占的字节必须封顶在缩略档，别再把整图钉住"
    );
}

/// ★ 同一张图的**更高分辨率档**落地时，鸟瞰来源不该跟着重采——那一刀要在 UI 线程
/// 把整张原图读一遍（6055² 实测 64 ms 的停顿），而画面上还是那 ≤512 px。
///
/// 判据是边长：旧那份不小于新一档就留用（同一张图，画质一分不损）；只有新一档
/// **更大**（小窗口首解只有 300 px 那种）才值得再采一次。两条都要钉住——
/// 只钉"不重采"会把画质回退也一起钉成"正确行为"。
#[test]
fn level_upgrade_keeps_the_nav_thumb_unless_it_would_get_bigger() {
    use crate::core::pixels::{Anim, Bitmap};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    let put = |v: &mut Viewer, seq: u64, edge: u32| {
        v.want_seq = seq;
        v.apply(Done {
            seq,
            path: std::path::PathBuf::from("big.tif"),
            out: Ok(Decoded {
                natural: (6055, 6055),
                region: None,
                anim: Anim::STATIC,
                bitmap: Bitmap {
                    width: edge,
                    height: edge,
                    rgba: vec![0x30u8; (edge * edge * 4) as usize],
                },
            }),
        })
    };
    // 视口档 561×561 那一路：缩略采出来是 511（fit_dims 向下取整）。
    assert!(put(&mut v, 0, 561));
    let (first, _, fw) = v
        .loaded
        .as_ref()
        .unwrap()
        .nav_thumb_rgba
        .as_ref()
        .expect("首帧该有鸟瞰来源")
        .clone();
    assert_eq!(fw, 511, "561 px 档应缩到 511");
    // 原图档 6055×6055 落地：同一张图、同一个 512 框 ⇒ 不许重采。
    assert!(put(&mut v, 1, 6055));
    let l = v.loaded.as_ref().unwrap();
    let (kept, kw, kh) = l.nav_thumb_rgba.as_ref().expect("升级后仍要有鸟瞰来源");
    assert_eq!((*kw, *kh), (511, 511), "留用的就是那一档");
    assert!(
        Arc::ptr_eq(kept, &first),
        "边长没变的升级不该在 UI 线程重采整图"
    );
    // 反过来：小窗口那份只有 300 px，原图档能给出 512 ⇒ 这一次必须重采。
    let (tx2, _rx2) = std::sync::mpsc::channel::<Done>();
    let mut small = Viewer::new(
        Rc::new(Decoder::spawn_with(tx2)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(put(&mut small, 0, 300));
    let (_, sw, _) = small
        .loaded
        .as_ref()
        .unwrap()
        .nav_thumb_rgba
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(sw, 300, "300 px 档不必缩");
    assert!(put(&mut small, 1, 6055));
    let (_, nw, _) = small
        .loaded
        .as_ref()
        .unwrap()
        .nav_thumb_rgba
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(nw, 512, "新一档更大时必须重采，否则画质白退一步");
}

/// 整图路线的鸟瞰**不许另建位图**——它退回主位图，而主位图在 ≥100% 时与
/// `base_rgba` 同一块内存。判据用指针而不是"看起来没多占"：只有 `Arc::ptr_eq`
/// 成立才能证明全链路真的只剩一份整图像素。
///
/// 钉的是 M9 那笔账：原来 `rebuild_image` 无条件拿 `nav_thumb_rgba` 造鸟瞰，
/// 而整图态那份就是原图 base，`transformed` 恒等变换照样 `to_vec()`——
/// 6055×6055 上是**第二份** 139.9 MB 白拷，且每次换档/旋转都重来一遍。
/// 鸟瞰那份必须是**缩略**（边长封顶 [`NAV_THUMB_MAX`]），而且缩放一步都不重画它。
///
/// 钉的是 M9 查到的那笔账：原来 `nav_image_of` 直接对整图走 `transformed`，
/// 恒等变换也要 `to_vec()`——6055×6055 上是 base 之外的**第二份** 139.9 MB，
/// 而且每次换档都重来一遍、上屏层还为它单独上传一份预乘 Pixmap。
/// 判据用指针与尺寸：鸟瞰那份既不能与 base 同一块内存（那说明没缩），
/// 也不能有整图尺寸（那说明缩略没生效）；换档前后必须是**同一个对象**。
#[test]
fn navigator_bitmap_is_a_thumbnail_and_survives_zoom() {
    use crate::core::decode::{Decoder, Done};
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (w, h) = (2000u32, 1500u32); // 12 MB 一份，够看出"有没有白拷整图"
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("big.png"),
        out: Ok(Decoded {
            natural: (w, h),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: w,
                height: h,
                rgba: vec![0x40u8; (w * h * 4) as usize],
            },
        }),
    }));
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    v.zoom_at_center(2.0);
    v.rebuild_image();

    // 显示档 == 原图档：主位图与 base 共享（M1），鸟瞰另存一份小的（M9）。
    let (nav, _) = v.nav_image_dst(RectI::new(0, 0, 80, 60)).unwrap();
    let (nw, nh) = (nav.width, nav.height);
    assert!(
        nw <= NAV_THUMB_MAX && nh <= NAV_THUMB_MAX,
        "鸟瞰那份必须是缩略，实际 {nw}×{nh}"
    );
    assert_ne!(
        (nw, nh),
        (w, h),
        "鸟瞰不许拿原图尺寸的位图——那正是要消掉的第二份整图"
    );
    {
        let l = v.loaded.as_ref().unwrap();
        assert_eq!((nw, nh), (512, 384), "2000×1500 等比进 512 框");
        assert_eq!(l.displayed, l.decoded, "200% 应停在原图档");
        assert!(
            Arc::ptr_eq(&l.image.rgba, &l.base_rgba),
            "主位图与 base 必须同一块内存（M1）"
        );
        assert!(
            !Arc::ptr_eq(&l.nav_image.as_ref().unwrap().rgba, &l.base_rgba),
            "鸟瞰那份是独立的小缓冲，不许指着整图"
        );
    }

    // 换档只动主位图：鸟瞰那份一个像素都不必重算（指针必须仍是同一个对象）。
    let before = v
        .loaded
        .as_ref()
        .map(|l| l.nav_image.as_ref().map(|i| i.gen))
        .unwrap_or(None);
    v.zoom_at_center(0.3);
    v.rebuild_image();
    let after = v
        .loaded
        .as_ref()
        .map(|l| l.nav_image.as_ref().map(|i| i.gen))
        .unwrap_or(None);
    assert_eq!(before, after, "缩放不该重画鸟瞰（gen 变了就是又拷了一遍）");
    assert!(v.loaded.as_ref().unwrap().displayed.0 < w, "30% 应落到缩档");

    // 旋转是要重画的（鸟瞰跟着朝向换轴），而且画完仍是缩略档——两头都得钉住。
    // 这一档在 100% 以上属于"大图"（> ORIENT_SYNC_PIXELS），结果由派生线程送回。
    v.rotate_cw();
    assert!(wait_levels(&mut v), "旋转那一档 5 s 没回来");
    let l = v.loaded.as_ref().unwrap();
    let nav = l.nav_image.as_ref().unwrap();
    assert_eq!((nav.width, nav.height), (384, 512), "rot 90° 后轴要换");
    assert_eq!(l.nav_key, (1, false), "重建后 key 必须跟上，否则每帧都重画");
    assert_ne!(nav.gen, before.unwrap(), "朝向变了就该是新的一份");
}

/// 等派生线程送回一档（最多 5 s），返回是否真的落地了一次。
fn wait_levels(v: &mut Viewer) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if v.poll_levels() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// 装好一张 `w×h` 整图（像素全 0x40）的 Viewer——异步换档那两条用例的公共前提。
fn whole_canvas_viewer(w: u32, h: u32) -> Viewer {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("a.png"),
        out: Ok(Decoded {
            natural: (w, h),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: w,
                height: h,
                rgba: vec![0x40u8; (w * h * 4) as usize],
            },
        }),
    }));
    v
}

/// 换档交给派生线程（M2）：投递那一刻 UI 侧**画的还是旧档**，采好了由
/// [`Viewer::poll_levels`] 换上去并给出重绘信号。
///
/// 为什么值得单独钉：同步版是在 `tick` 里当场重采样（6055×6055 实测净
/// 0.118–0.131 s 全落在调用线程上），"改完还是旧档"这一步就是那 0.13 s 被挪走
/// 的证据；反过来谁把 `poll_levels` 的返回值吞了，画面会采好了却不刷新——
/// 所以"一定会换"和"换了要说一声"两条都要钉住。
#[test]
fn level_change_is_applied_only_when_the_thread_reports_back() {
    let mut v = whole_canvas_viewer(400, 300);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let before = v.loaded.as_ref().unwrap().displayed;
    let target = fit_dims(400, 300, 200, 150);
    assert_ne!(before, target, "本例的前提是这一档确实要换");

    assert!(
        v.post_level(target),
        "投递必须成功（失败时调用方要退回同步派生）"
    );
    assert_eq!(
        v.loaded.as_ref().unwrap().displayed,
        before,
        "UI 线程不许在投递那一刻自己算——那 0.13 s 正是本轮要挪走的东西"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut applied = false;
    while Instant::now() < deadline {
        if v.poll_levels() {
            applied = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(applied, "派生线程 5 s 没回结果，档位永远上不了屏");
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(l.displayed, target, "换档要落到目标档");
    assert_eq!((l.image.width, l.image.height), target);
    assert_eq!(l.image.rgba.len(), (target.0 * target.1 * 4) as usize);
    assert_eq!(l.image.rgba[0], 0x40, "采的必须是这张图的像素");
    assert!(
        !Arc::ptr_eq(&l.image.rgba, &l.base_rgba),
        "缩档是新缓冲，不许指着 base"
    );
    assert!(
        v.inflight.is_none(),
        "用掉的结果必须清了在途标记，否则下一次换档会被当成重复投递而永不生效"
    );
}

/// ★ 原图档升级落地时，派生必须交给派生线程（m94）：升级这一帧 UI 线程只付投递，
/// 旧上屏档原样顶着，新档由 [`Viewer::poll_levels`] 换上去。
///
/// 旧写法在 `install` 里同步 `rebuild_image`：6055×6055 → 1001 实测 38.1 ms，
/// 正压在"改窗口 / 放大一格"触发的那次升级帧上。两条都要钉住——只钉"投递时不动"
/// 会把"永远停在旧档"写成正确行为，只钉"新档会到"又测不到这一刀挪没挪走。
///
/// 先破后立：把 `install` 末尾改回无条件的 `self.rebuild_image()`，第一条断言立刻红。
#[test]
fn large_native_upgrade_defers_derive_to_the_level_thread() {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 800.0, 600.0, 1.0);
    let path = std::path::PathBuf::from("big.tif");
    let put = |v: &mut Viewer, seq: u64, bm: (u32, u32)| {
        v.want_seq = seq;
        v.apply(Done {
            seq,
            path: path.clone(),
            out: Ok(Decoded {
                natural: (4000, 3000),
                region: None,
                anim: Anim::STATIC,
                bitmap: Bitmap {
                    width: bm.0,
                    height: bm.1,
                    rgba: vec![0x40u8; (bm.0 * bm.1 * 4) as usize],
                },
            }),
        })
    };
    // 视口档 800×600 先上屏（＝"原图 4000×3000 缩到装得下窗口"的首解）。
    assert!(put(&mut v, 0, (800, 600)));
    let (old_rgba, old_displayed) = {
        let l = v.loaded.as_ref().unwrap();
        (Arc::clone(&l.image.rgba), l.displayed)
    };
    assert_ne!(
        old_displayed,
        (4000, 3000),
        "样本不成立：旧档不能已经是原图档"
    );
    // 原图档 4000×3000 落地。
    assert!(put(&mut v, 1, (4000, 3000)));
    {
        let l = v.loaded.as_ref().unwrap();
        assert!(
            Arc::ptr_eq(&l.image.rgba, &old_rgba),
            "升级落地那一帧不许在 UI 线程派生：屏上该还是旧档"
        );
        assert_eq!(l.displayed, old_displayed, "旧档尺寸原样顶着");
        assert_eq!(l.decoded, (4000, 3000), "新源已就位，派生的源必须是它");
        assert!(v.inflight.is_some(), "新档必须已经交给派生线程");
    }
    assert!(wait_levels(&mut v), "升级那一档 5 s 没回来");
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(l.displayed, fit_dims(4000, 3000, 800, 600));
    assert!(
        !Arc::ptr_eq(&l.image.rgba, &l.base_rgba),
        "缩档是新缓冲，落地后不许还顶着旧档"
    );
    assert!(v.inflight.is_none(), "用掉的结果要清在途标记");
}

/// 上面那条升级闸只接"源真的更大"：动画的下一帧是同尺寸重解，必须当场换。
/// 挪去派生线程等于给动画加一帧队尾延迟（并让 `epoch` 每帧作废上一档）。
#[test]
fn same_size_animation_frame_installs_without_waiting_for_a_level() {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 800.0, 600.0, 1.0);
    let path = std::path::PathBuf::from("anim.gif");
    let put = |v: &mut Viewer, seq: u64, index: u32, px: u8| {
        v.want_seq = seq;
        v.apply(Done {
            seq,
            path: path.clone(),
            out: Ok(Decoded {
                natural: (4000, 3000),
                region: None,
                anim: Anim {
                    index,
                    count: 2,
                    delay_ms: 40,
                    paged: false,
                },
                bitmap: Bitmap {
                    width: 800,
                    height: 600,
                    rgba: vec![px; 800 * 600 * 4],
                },
            }),
        })
    };
    assert!(put(&mut v, 0, 0, 0x40));
    assert!(put(&mut v, 1, 1, 0xA5));
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(l.anim.index, 1, "下一帧必须当场装进来");
    assert_eq!(l.image.rgba[0], 0xA5, "屏上像素必须是新那一帧的");
    assert!(v.inflight.is_none(), "动画帧不许排进派生队列");
}

/// ★ 大图旋转：投递那一刻屏上像素与朝向都不动，派生线程送回后才一起换。
///
/// "改完还是旧朝向"这一步就是那 75–200 ms 被挪走的证据；反过来谁在
/// `poll_levels` 落地时不换朝向，屏上与几何就对不上（旧图被拉伸）。
#[test]
fn big_image_rotation_lands_only_when_the_thread_reports_back() {
    let mut v = whole_canvas_viewer(2000, 1500);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let before = v.loaded.as_ref().unwrap().displayed;
    v.rotate_cw();
    assert_eq!((v.rot, v.flip), (0, false), "UI 线程不许当场搬像素");
    assert_eq!(
        (v.want_rot, v.want_flip),
        (1, false),
        "目标朝向必须立刻记下"
    );
    assert_eq!(
        v.loaded.as_ref().unwrap().displayed,
        before,
        "落地前屏上还是旧档"
    );

    assert!(wait_levels(&mut v), "旋转那一档 5 s 没回来");
    assert_eq!((v.rot, v.flip), (1, false), "落地后朝向要跟着换");
    let l = v.loaded.as_ref().unwrap();
    // `displayed` 记的是**缩档目标**（未旋转时的 fit 框），像素本身才换轴。
    assert_eq!(l.displayed, (100, 75));
    assert_eq!(
        (l.image.width, l.image.height),
        (75, 100),
        "90° 后像素宽高互换"
    );
    assert_eq!(l.image.rgba.len(), 75 * 100 * 4);
    assert_eq!(
        (v.rot, v.flip),
        (v.want_rot, v.want_flip),
        "落地即与目标一致"
    );
}

/// 连按 R：每一按都记进目标，最后落地的是**最后一次**的朝向（180°）。
#[test]
fn rapid_rotations_land_on_the_latest_orientation() {
    let mut v = whole_canvas_viewer(2000, 1500);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    v.rotate_cw();
    v.rotate_cw();
    assert_eq!((v.want_rot, v.rot), (2, 0));
    assert!(wait_levels(&mut v), "旋转那一档 5 s 没回来");
    assert_eq!((v.rot, v.flip), (2, false), "连按两下应落到 180°");
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(l.displayed, (100, 75), "180° 不换轴");
}

/// 连按四次回到屏上朝向：在途那一份算完也不许翻上去（它已经过时了）。
#[test]
fn rotating_full_circle_leaves_the_screen_untouched() {
    let mut v = whole_canvas_viewer(2000, 1500);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let before = v.loaded.as_ref().unwrap().displayed;
    for _ in 0..4 {
        v.rotate_cw();
    }
    assert_eq!((v.want_rot, v.want_flip), (0, false));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        assert!(!v.poll_levels(), "过时朝向被接受 = 画面会自己跳一下");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!((v.rot, v.flip), (0, false), "屏上朝向不许被过时结果改掉");
    assert_eq!(
        v.loaded.as_ref().unwrap().displayed,
        before,
        "屏上像素也不许动"
    );
}

/// 换了图之后晚到的旧档必须被丢弃（`epoch` 闸）。
///
/// 场景是真会发生的：旧图的原图档重采样要 0.1 s 量级，这期间用户按了方向键、
/// 新图已经上屏——晚到的那份旧图像素若被接受，表现就是"刚翻页又闪回上一张"。
#[test]
fn stale_level_result_is_dropped_after_installing_another_image() {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let mut v = whole_canvas_viewer(400, 300);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    assert!(v.post_level((300, 225)), "先给旧图投一档");
    let old_epoch = v.epoch;

    // 用户翻页：装进另一张图（尺寸与像素都不同，好分辨谁被画了上去）。
    assert!(v.apply(Done {
        seq: v.want_seq,
        path: std::path::PathBuf::from("b.png"),
        out: Ok(Decoded {
            natural: (60, 40),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 60,
                height: 40,
                rgba: vec![0xA5u8; 60 * 40 * 4],
            },
        }),
    }));
    assert_ne!(v.epoch, old_epoch, "装新图必须换代号");
    assert!(v.inflight.is_none(), "装新图要清掉旧图的在途标记");
    let fresh = v.loaded.as_ref().unwrap().displayed;

    // 把旧档的回包耗干净：它一份像素都不许落到屏上。
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if v.poll_levels() {
            panic!("旧图的档位结果被接受了——会闪回上一张图");
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let l = v.loaded.as_ref().unwrap();
    assert_eq!(l.displayed, fresh, "新图刚装好的档位不许被晚到的旧档盖掉");
    assert_eq!(l.decoded, (60, 40), "画布上必须是新图");
    assert_eq!(l.image.rgba[0], 0xA5, "像素必须来自新图那份");
    // ⑫那颗钉：这份**已经烧掉**的 CPU 必须留下数。这一支正是"算完才发现画面不要"，
    // 探针里"白烧占比"全靠它；钉漏了，读到的永远是 0（看着干净，其实是没记）。
    let c = v.level_counts();
    assert_eq!(
        (c.posted, c.computed, c.discarded),
        (1, 1, 1),
        "丢弃过期回包必须记数：{c:?}"
    );
}

// ── 后台预读相邻图 ─────────────────────────────────────────

/// 造一个 3 张图的临时目录，返回 (目录, [a, b, c] 路径)。
///
/// 扩展名用 `.svg`：扩展名清单 = 内置 ∪ 解码器自述，单测进程没有解码器，
/// 只有内置的 svg/svgz 能进清单（本测试不真解码，扩展名只是清单的门票）。
fn gallery_dir() -> (std::path::PathBuf, [std::path::PathBuf; 3]) {
    let dir = crate::core::paths::test_scratch_root()
        .join(format!("sv-test-prefetch-{}", std::process::id()));
    // 先清掉可能的残留：pid 会被 Windows 回收，同名目录里留着上个进程的文件 ⇒ 用例继承别人的状态（m106）
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|n| {
            let p = dir.join(format!("{n}.svg"));
            std::fs::File::create(&p).unwrap();
            p
        })
        .collect();
    (dir, [paths[0].clone(), paths[1].clone(), paths[2].clone()])
}

fn decoded4(path: std::path::PathBuf) -> Done {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    Done {
        seq: 0,
        path,
        out: Ok(Decoded {
            natural: (4, 4),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 4,
                height: 4,
                rgba: vec![0x80u8; 64],
            },
        }),
    }
}

/// 预读命中：翻页零解码延迟（want_seq 不变）；未命中才发新请求。
#[test]
fn step_uses_prefetched_neighbor_without_new_decode() {
    let (_dir, [pa, pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    // 模拟"b 已预读完成"（4×4 到原生尺寸，命中判定走 at_native）。
    v.prefetched.push(Prefetched {
        path: pb.clone(),
        img: decoded4(pb.clone()).out.unwrap(),
    });
    let seq_before = v.want_seq;

    assert!(v.step(1));
    assert_eq!(
        v.loaded.as_ref().unwrap().path,
        pb,
        "预读命中必须直接上屏 b"
    );
    assert_eq!(
        v.want_seq, seq_before,
        "命中预读不得再发解码请求（零延迟的意义就在这）"
    );

    // 再翻到未预读的 c：必须走正常解码请求；旧图留屏防闪烁，
    // c 要等解码回包才替换（切图不闪烁铁律）。
    assert!(v.step(1));
    assert!(v.want_seq > seq_before, "未命中必须发新解码请求");
    assert_eq!(
        v.loaded.as_ref().unwrap().path,
        pb,
        "新图解码回来前旧图必须留在屏上"
    );
    // 预读缓存被消费掉，b 不应残留。
    assert!(v.prefetched.iter().all(|e| e.path != pb));
}

/// m101 的夹具：自己造一个三张图的目录（不与 [`gallery_dir`] 共用，
/// 因为下面几条要往里面**再塞第四张**，污染共用夹具会让别的用例改的是时序而不是判据）。
fn reopen_dir(tag: &str) -> (std::path::PathBuf, [std::path::PathBuf; 3]) {
    let dir = crate::core::paths::test_scratch_root()
        .join(format!("sv-m101-{tag}-{}", std::process::id()));
    // 先清掉可能的残留：pid 会被 Windows 回收，同名目录里留着上个进程的文件 ⇒ 用例继承别人的状态（m106）
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|n| {
            let p = dir.join(format!("{n}.svg"));
            std::fs::File::create(&p).unwrap();
            p
        })
        .collect();
    (dir, [paths[0].clone(), paths[1].clone(), paths[2].clone()])
}

/// ★ m101：**同一目录里再开一张不是换目录**。
///
/// 旧写法在 `open` 里无条件 `Gallery::from_file` + `prefetched.clear()` +
/// `thumbs.clear()`，于是资源管理器里连双击每张都重扫整目录、整条胶片条重解、
/// 首屏必走现场解码（端点读数见 本仓历史记录）。这里钉两条可观察的：
/// ① 清单**没重扫**（目录里刚多出来的第四张不该出现）；
/// ② 预读缓存**没被清**（没被点开的 c 要活着），而被点开的 b 是被**消费**掉的
///    （`want_seq` 不动 = 零解码上屏，不是"清空后又解了一遍"）。
/// 第三条（缩略图缓存同样不清）与这两条在同一个分支里，单测不重复钉，
/// 由端点那台的 `thumbs_in_seg` 读数钉（见 本仓历史记录
/// §3.1）——它量的正是"每开一张发几发 Thumb 请求"。
#[test]
fn reopening_in_the_same_folder_reuses_the_listing_and_the_caches() {
    let (dir, [pa, pb, pc]) = reopen_dir("reuse");
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    // ① 目录里现在有四张，但这份清单是开 a 时扫的：复用就不该长到 4。
    std::fs::File::create(dir.join("d.svg")).unwrap();
    // ② b 马上要被点开，c 只是"顺带预读到的邻居"。
    v.prefetched.push(Prefetched {
        path: pb.clone(),
        img: decoded4(pb.clone()).out.unwrap(),
    });
    v.prefetched.push(Prefetched {
        path: pc.clone(),
        img: decoded4(pc.clone()).out.unwrap(),
    });
    let seq_before = v.want_seq;

    v.open(&pb);

    assert_eq!(
        v.gallery.as_ref().unwrap().len(),
        3,
        "同目录开图不该重扫清单"
    );
    assert_eq!(v.gallery.as_ref().unwrap().position(), 2, "当前项要落到 b");
    assert_eq!(
        v.loaded.as_ref().map(|l| l.path.clone()),
        Some(pb.clone()),
        "预读命中就该直接上屏 b（旧写法先清空缓存，这一张必走现场解码 ⇒ `loaded` 还停在 a）"
    );
    assert_eq!(v.want_seq, seq_before, "命中预读不许再发解码请求");
    assert!(
        v.prefetched.iter().any(|e| e.path == pc),
        "没被点开的邻居不该跟着一起被清掉"
    );
    assert!(
        v.prefetched.iter().all(|e| e.path != pb),
        "b 是被消费掉的，不是留在缓存里"
    );
}

/// ★ 反向风险：换了目录**仍然要**重扫并清缓存（复用判据不能顺手把换目录也放过）。
#[test]
fn opening_a_picture_in_another_folder_still_rebuilds_everything() {
    let (_, [pa, _pb, _pc]) = reopen_dir("keepa");
    let (dir_b, [xb, _yb, _zb]) = reopen_dir("keepb");
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    v.prefetched.push(Prefetched {
        path: pa.clone(),
        img: decoded4(pa.clone()).out.unwrap(),
    });

    v.open(&xb);

    assert_eq!(v.gallery.as_ref().unwrap().dir(), dir_b, "换目录必须换清单");
    assert_eq!(
        v.gallery.as_ref().unwrap().len(),
        3,
        "新目录要按它自己扫出来"
    );
    assert!(
        v.prefetched.iter().all(|e| e.path != pa),
        "旧目录那批预读再也看不见，必须清掉（留着就是白占内存）"
    );
}

/// ★ 清单认不得的锚点**不许**走复用：那是"外部刚拷进来的一张新图"或"打开文件夹"。
///
/// 这条判据是复用唯一的失效口——它一失效，屏幕上就会出现"双击了一张图、打开的
/// 却是同目录的另一张"（m45 缺陷 2 的形状）。
#[test]
fn a_file_the_listing_does_not_know_falls_back_to_a_full_scan() {
    let (dir, [pa, _pb, _pc]) = reopen_dir("unknown");
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let fresh = dir.join("newcomer.svg");
    std::fs::File::create(&fresh).unwrap();

    v.open(&fresh);

    assert_eq!(
        v.gallery.as_ref().unwrap().len(),
        4,
        "清单里没有它 ⇒ 必须重扫，把刚拷进来的这张一起收进来"
    );
    assert_eq!(
        v.gallery.as_ref().unwrap().current().unwrap(),
        &fresh,
        "打开的必须是用户点的那一个"
    );

    // 「打开文件夹」也走完整重建：文件夹的 `parent()` 是它的上一级，对不上本清单。
    v.open(&dir);
    assert_eq!(
        v.gallery.as_ref().unwrap().dir(),
        dir,
        "把文件夹本身当锚点 = 打开这个文件夹（清单目录就是它）"
    );
    assert_eq!(v.gallery.as_ref().unwrap().position(), 1);
}

/// 打开一个空目录：旧图、旧标题、旧分母必须一起作废；晚到的旧图回包也不许再上屏。
#[test]
fn opening_an_empty_folder_clears_the_old_view_and_late_reply() {
    let (_, [pa, _pb, _pc]) = reopen_dir("openfail");
    let empty = crate::core::paths::test_scratch_root()
        .join(format!("sv-test-openfail-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let old_seq = v.want_seq;
    let mut shown = decoded4(pa.clone());
    shown.seq = old_seq;
    assert!(v.apply(shown));
    assert!(v.image().is_some());

    v.open(&empty);

    assert!(v.gallery.is_none(), "空目录不能留着旧清单");
    assert!(v.image().is_none(), "旧图必须从屏上撤掉");
    assert_eq!(v.title(), "RuPix");
    assert_eq!(v.status(), "没有打开图片");
    assert!(v.error_text().is_some());
    assert_ne!(v.want_seq, old_seq, "旧请求的 seq 必须被作废");

    let mut late = decoded4(pa);
    late.seq = old_seq;
    assert!(!v.apply(late), "晚到的旧图回包不该再上屏");
    assert!(v.image().is_none());
    let _ = std::fs::remove_dir_all(&empty);
}

/// 目录被清空：不能继续显示已经删掉的那张图，也不能留旧标题/旧分母。
#[test]
fn rescanning_an_emptied_folder_drops_the_deleted_image() {
    let (dir, [pa, pb, pc]) = reopen_dir("rescanempty");
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let mut shown = decoded4(pa.clone());
    shown.seq = v.want_seq;
    assert!(v.apply(shown));
    assert!(v.image().is_some());

    for p in [&pa, &pb, &pc] {
        std::fs::remove_file(p).unwrap();
    }
    assert!(v.rescan_dir(), "目录变空也算清单变了");
    assert!(v.gallery.is_none());
    assert!(v.image().is_none());
    assert_eq!(v.status(), "没有打开图片");
    assert!(
        v.error_text()
            .as_deref()
            .is_some_and(|s| s.contains("没有可显示的图片")),
        "空目录要给出人话，实得 {:?}",
        v.error_text()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 兄弟项改名（长度与当前项都没变）：清单内容已经不同，必须通知 UI 重绘。
#[test]
fn rescanning_a_renamed_sibling_notifies_the_ui() {
    let (dir, [pa, pb, _pc]) = reopen_dir("renamesibling");
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    let renamed = dir.join("d.svg");
    std::fs::rename(&pb, &renamed).unwrap();

    assert!(v.rescan_dir(), "改名不改变长度，但清单已经不同");
    let g = v.gallery.as_ref().unwrap();
    assert_eq!(g.len(), 3);
    assert_eq!(g.current(), Some(&pa), "当前图没动");
    assert_eq!(g.peek(2), Some(&renamed), "胶片条最后一格应换成新名字");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 元信息缓存必须有界：翻几万张不同目录的图不能一直攒路径。
#[test]
fn meta_cache_is_bounded() {
    let mut v = viewer_with_image();
    for i in 0..(META_CACHE_MAX + 10) {
        v.cache_meta(PathBuf::from(format!("{i}.png")), Meta { natural: (1, 1) });
    }
    assert!(
        v.meta_cache.len() <= META_CACHE_MAX,
        "缓存涨到 {} 条，超过上限 {META_CACHE_MAX}",
        v.meta_cache.len()
    );
}

/// ★ 没有真实视口就不发主画面请求——这条闸 m101 之前只有 `open` 有，`navigate` 没有。
///
/// 判据挪进 [`Viewer::request_decode`] 之后，两条路共用一份规则；这里两头都钉：
/// 视口是 (1,1) 时只标需求（`want_seq` 不动），视口一到立刻补发。
#[test]
fn a_decode_request_waits_for_a_real_viewport() {
    let (_, [pa, _pb, _pc]) = reopen_dir("viewport");
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    let seq = v.want_seq;
    assert_eq!(seq, 0, "视口还没尺寸，开图一枚请求都不该发");
    assert!(
        v.need_at.is_some(),
        "不发请求要标需求，否则首帧之后没人补发"
    );

    // 视口就绪：`on_viewport` 自己会补发那一枚。
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    assert!(v.want_seq > seq, "视口一到就要发出去");
    assert!(v.need_at.is_none(), "发出去了，需求就要落地");

    // 回到没有视口的状态（模拟隐藏期换图）：翻页同样不许发请求，只标需求。
    v.view = (0.0, 0.0, 1.0, 1.0);
    v.need_at = None;
    let seq2 = v.want_seq;
    assert!(v.step(1), "翻页要真的动了位置");
    assert_eq!(
        v.want_seq, seq2,
        "navigate 也受同一道闸管（旧写法这里会发一枚 1×1 废请求）"
    );
    assert!(v.need_at.is_some(), "同上：不发就要标需求");
}

/// 造一份"视口档"回包：`natural` 6055² 而手里只有 `tier × tier`（≠ 整图 ⇒ 会走升级腿）。
fn viewport_tier(tier: u32) -> Decoded {
    use crate::core::pixels::Bitmap;
    let n = (tier * tier * 4) as usize;
    Decoded {
        natural: (6055, 6055),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: tier,
            height: tier,
            rgba: vec![0x40u8; n],
        },
    }
}

/// **位图**夹具：`gallery_dir()` 那三枚是 `.svg`，会先撞上矢量分支，量不到升级腿。
fn bitmap_gallery_dir() -> (std::path::PathBuf, [std::path::PathBuf; 3]) {
    let dir = crate::core::paths::test_scratch_root()
        .join(format!("sv-test-upgrade-{}", std::process::id()));
    // 先清掉可能的残留：pid 会被 Windows 回收，同名目录里留着上个进程的文件 ⇒ 用例继承别人的状态（m106）
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|n| {
            let p = dir.join(format!("{n}.png"));
            std::fs::File::create(&p).unwrap();
            p
        })
        .collect();
    (dir, [paths[0].clone(), paths[1].clone(), paths[2].clone()])
}

/// 把 viewer 摆成"刚翻到这一张、视口档已上屏、防抖已到期"。
fn landed_at_viewport_tier(pa: &std::path::Path) -> Viewer {
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    // 6055² 方图 fit 进 100×100 ⇒ 视口档就是 100×100（`desired_pixels` 在 fit 下要的数）。
    assert!(v.install(pa.to_path_buf(), viewport_tier(100)));
    v.need_at = Some(Instant::now() - Duration::from_millis(200));
    v
}

/// **让路成立**（m58）：宽限期内 tick 不发原图档整解，且把 `need_at` 留着下次再判。
///
/// 先破后立：摘掉 `tick` 里那条 `upgrade_can_wait` 分支，本用例立刻红
/// （`want_seq` 会跳号）——它测的不是"函数存在"，是"分支真的接在链上"。
#[test]
fn native_upgrade_yields_to_flipping() {
    let (_dir, [pa, _pb, _pc]) = bitmap_gallery_dir();
    let mut v = landed_at_viewport_tier(&pa);
    // 刚刚才翻页。
    v.last_nav_at.set(Some(Instant::now()));
    let seq_before = v.want_seq;

    assert!(!v.tick(), "让路时不换图，也就不该要重绘");
    assert_eq!(
        v.want_seq, seq_before,
        "翻页宽限期内不得发出 0x0 原图档整解（那一次要占住唯一解码线程 150–500 ms）"
    );
    assert!(
        v.need_at.is_some(),
        "推迟不是取消：`need_at` 必须留着，下一次 poll 再判"
    );
    assert!(
        v.native_requested.is_none(),
        "没发出去就不许把升级闩落下，否则这张图永久拿不到原图档"
    );
}

/// **手一停就补清晰**：距翻页已超过宽限期 ⇒ 升级照发（旧行为不打折）。
#[test]
fn native_upgrade_fires_once_flipping_stops() {
    let (_dir, [pa, _pb, _pc]) = bitmap_gallery_dir();
    let mut v = landed_at_viewport_tier(&pa);
    v.last_nav_at
        .set(Some(Instant::now() - Duration::from_millis(1000)));
    let seq_before = v.want_seq;

    assert!(
        !v.tick(),
        "只是发出请求，画面要等回包落地才换——这一步不许要重绘"
    );
    assert!(
        v.want_seq != seq_before,
        "停下来超过宽限期就该发原图档升级，否则清晰度永远补不上"
    );
    assert!(
        v.need_at.is_none(),
        "发出去了就要清 `need_at`，不许反复重发"
    );
    assert!(
        v.native_requested.as_ref().is_some_and(|(p, _)| *p == pa),
        "升级闩要落下（同一档请求两次 = 整解两次）"
    );
}

/// **不给放大添新账**：放大到视口档撑不住时，即便还在宽限期内也立刻发。
#[test]
fn native_upgrade_not_deferred_when_viewport_tier_is_not_enough() {
    let (_dir, [pa, _pb, _pc]) = bitmap_gallery_dir();
    let mut v = landed_at_viewport_tier(&pa);
    v.last_nav_at.set(Some(Instant::now()));
    // 放大到远过 fit：手里那 100×100 对画面已经不够（判据与预读命中同一条）。
    v.zoom_at_center(30.0);
    v.need_at = Some(Instant::now() - Duration::from_millis(200));
    let seq_before = v.want_seq;

    assert!(
        !v.tick(),
        "只是发出请求，画面要等回包落地才换——这一步不许要重绘"
    );
    assert!(
        v.want_seq != seq_before,
        "视口档不够用时不许让路——翻页后立刻按 `1` 不该再等宽限期"
    );
}

/// **按目标重画**的矢量夹具（m189）：目录里三枚 `.pdf`（空文件足矣——用例手工装
/// 解码结果；走的是"已上屏后缩放"那一段，不读文件内容）。
fn target_gallery_dir() -> (std::path::PathBuf, [std::path::PathBuf; 3]) {
    let dir = crate::core::paths::test_scratch_root()
        .join(format!("sv-test-zvec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|n| {
            let p = dir.join(format!("{n}.pdf"));
            std::fs::File::create(&p).unwrap();
            p
        })
        .collect();
    (dir, [paths[0].clone(), paths[1].clone(), paths[2].clone()])
}

/// ★ m189：**按目标重画的矢量放大后必须请解码器按更大目标整幅重画**，不许拿首次
/// 画布本地插值（真机：PDF 放大成马赛克，而 PDF 阅读器是清的）。
///
/// 先破后立：摘掉 `tick` 里那条 `needs_zoom_rerender` 分支，`want_seq` 不会跳号，
/// 本用例立刻红。夹具 `natural = decoded = 100×100`（无区域通道 ⇒ 没有文档尺寸查询）。
#[test]
fn target_vector_zoom_requests_a_bigger_whole_render() {
    let _guard = crate::core::formats::test_override::set(&["pdf"], &[]);
    let (_dir, [pa, _pb, _pc]) = target_gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    assert!(v.install(pa.clone(), decoded_sized(100, 100)));
    // 放大到 4 倍：目标 = 首次画布 100 × 4 = 400。
    v.zoom_at_center(4.0);
    v.need_at = Some(Instant::now() - Duration::from_millis(200));
    let seq_before = v.want_seq;
    assert!(
        !v.tick(),
        "只是发出请求，画面要等回包落地才换——这一步不许要重绘"
    );
    assert!(
        v.want_seq != seq_before,
        "放大 4 倍必须请解码器按 400×400 把文档重画一遍，否则就是本地插值糊面"
    );
    assert_eq!(
        v.vector_zoom_req.as_ref().map(|(p, t)| (p.as_path(), *t)),
        Some((pa.as_path(), (400, 400))),
        "目标档要记账（防解码器不理 target 时每轮重发）"
    );
}

/// ★ m189：重渲落地**不许改 natural**——它是"首次画布"，画布几何与倍率都锚在它上面。
/// 拿解码器回报的（这一次的画布）当 natural，画面会在每次重渲后按新尺寸重新 fit：
/// 用户看到图忽大忽小，状态栏尺寸也乱跳。
#[test]
fn target_vector_rerender_keeps_the_first_canvas_as_natural() {
    let _guard = crate::core::formats::test_override::set(&["pdf"], &[]);
    let (_dir, [pa, _pb, _pc]) = target_gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    assert!(v.install(pa.clone(), decoded_sized(100, 100)));
    // 重渲回包：画布 400×400，解码器报的自然尺寸也是 400×400。
    use crate::core::pixels::Bitmap;
    let canvas = |w: u32, h: u32| Decoded {
        natural: (w, h),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: w,
            height: h,
            rgba: vec![0x40u8; (w * h * 4) as usize],
        },
    };
    v.want_seq = 7;
    assert!(v.apply(Done {
        seq: 7,
        path: pa.clone(),
        out: Ok(canvas(400, 400)),
    }));
    let l = v.loaded.as_ref().expect("重渲落地后应仍在屏上");
    assert_eq!(
        l.natural,
        (100, 100),
        "natural 是首次画布，跨重渲必须保持不变"
    );
    assert_eq!(l.decoded, (400, 400), "解码档要真的换成新画布");
}

/// ★ m189：解码器**不理 `target`**（画出来仍偏小）时不许每轮重发——同一目标档只发一次，
/// 用户再放大一档才照发。没有这道账，`tick` 会 16 ms 一轮地把整幅重画请求刷下去。
#[test]
fn target_vector_rerender_does_not_loop_when_target_is_ignored() {
    let _guard = crate::core::formats::test_override::set(&["pdf"], &[]);
    let (_dir, [pa, _pb, _pc]) = target_gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    assert!(v.install(pa.clone(), decoded_sized(100, 100)));
    v.zoom_at_center(4.0);
    v.need_at = Some(Instant::now() - Duration::from_millis(200));
    assert!(!v.tick());
    let sent = v.want_seq;
    // 回包只画了 100×100（解码器把 target 当耳边风）。
    assert!(v.apply(Done {
        seq: sent,
        path: pa.clone(),
        out: Ok(decoded_sized(100, 100)),
    }));
    v.need_at = Some(Instant::now() - Duration::from_millis(200));
    assert!(!v.tick());
    assert_eq!(
        v.want_seq, sent,
        "同一目标档不许重发：解码器不理 target 时会变成每轮一次整幅重画"
    );
}

#[test]
fn prefetch_done_is_cached_not_painted() {
    let (_dir, [pa, pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    // 造一个"在途预读"世代号。
    v.prefetch_seq = Some(v.want_seq + 1000);
    let mut done = decoded4(pb);
    let stale_seq = v.want_seq + 1000;
    done.seq = stale_seq;
    assert!(!v.apply(done), "预读回包不能让画布重绘");
    // 在途标记必须**离开那一枚已消费的请求**。m34 起落地一份就续发下一枚
    // （窗口是空闲时一格一格填的），所以落地后它要么清空（窗口已填满/已到预算），
    // 要么挂在一枚**新的**请求上——唯一不许的是还停在刚回来的这枚上，
    // 那样后续预读就再也发不出去了。
    assert!(
        v.prefetch_seq != Some(stale_seq),
        "落地后不得仍挂在同一枚在途请求上"
    );
    assert_eq!(v.prefetched.len(), 1, "预读结果必须入缓存");
    assert!(v.loaded.is_none() || v.loaded.as_ref().unwrap().path == pa);
}

/// GIF 首帧渲染时 `count` 仍是未知 0，但 `delay_ms>0` 即动画。回归：曾用
/// `count<=1` 判静态，导致 GIF 永远不排下一帧、卡在第一帧（实测 sample-gif.gif）。
#[test]
fn gif_animates_even_when_count_unknown() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);

    // GIF 首帧产物：count 未知(0)，但 delay_ms=350 → 必须排动画。
    let gif = Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index: 0,
            count: 0,
            delay_ms: 350,
            paged: false,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), gif), "GIF 首帧应装屏成功");
    assert!(
        v.anim_next_at.is_some(),
        "GIF(count=0, delay_ms=350) 必须排动画时刻表，否则卡在第一帧"
    );

    // 真·静态图：delay_ms=0 → 不排。
    let still = Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), still), "静态图应装屏成功");
    assert!(
        v.anim_next_at.is_none(),
        "静态图(delay_ms=0) 绝不排动画时刻表"
    );
}

/// ★ m75：动画排表的基准是**上一帧应显示的时刻**，不是这一帧落地的时刻。
///
/// 旧写法 `落地 + 延时` 把整段解码成本又加进周期，实测标 25 fps 的 GIF 屏上只有
/// 13.3 fps（640×480）/ 9.1 fps（3 MP @适应窗口）。时间口径与 `wheel_due` 同一定式：
/// 时刻手工挪、不靠 sleep（"这台机器跑得快"从来不是判据）。延时取 200 ms 是为了
/// 把三条判据的读数区间拉开，装档本身那几毫秒落不进任何一条的容差里。
#[test]
fn anim_schedule_chases_the_deadline_not_the_arrival() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);
    let frame = |index: u32| Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index,
            count: 8,
            delay_ms: 200,
            paged: false,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![index as u8; 16],
        },
    };
    // 下一帧还领先多少毫秒（负数 = 已经迟到）。
    let lead = |t: Instant| -> i64 {
        let now = Instant::now();
        if t > now {
            t.duration_since(now).as_millis() as i64
        } else {
            -(now.duration_since(t).as_millis() as i64)
        }
    };
    let at = |ms: i64| {
        let d = Duration::from_millis(ms.unsigned_abs());
        if ms < 0 {
            Instant::now() - d
        } else {
            Instant::now() + d
        }
    };

    v.anim_due_at = None;
    v.install(pa.clone(), frame(0));
    let l = lead(v.anim_next_at.expect("首帧该排上时刻表"));
    assert!(
        (150..=260).contains(&l),
        "全新起表 = 现在 + 延时 200，实际领先 {l} ms"
    );

    // ① 提前落地（成本 < 延时）：时间线照旧一格 200 ms 往下走，不许被"现在"重排。
    //    旧写法这里给的是 200（从落地起算），新写法给 150 + 200 = 350。
    v.anim_due_at = Some(at(150));
    v.install(pa.clone(), frame(1));
    let l = lead(v.anim_next_at.expect("续表"));
    assert!(
        (300..=420).contains(&l),
        "上一帧的截止还在 +150 ms，这一帧该排到 +350 ms 附近，实际领先 {l} ms；\
             读到 200 附近就是退回`落地 + 延时`＝把每一帧的成本都变成慢放"
    );
    assert_eq!(
        v.anim_next_at, v.anim_due_at,
        "闸与时间线必须同刻（闸被 tick 清、线跨帧续）"
    );

    // ② 迟到落地（成本 > 延时）：欠的那一截不补等，下一帧立刻发——仍然只有一个在途请求。
    v.anim_due_at = Some(at(-900));
    v.install(pa.clone(), frame(2));
    let l = lead(v.anim_next_at.expect("续表"));
    assert!(
        l <= 20,
        "已经落后 900 ms 还要再等一整个延时＝越落越慢，实际领先 {l} ms"
    );

    // ③ 同一帧重解（放大到原尺寸那种档位升级）**不是新的一帧**：不许把时间线往前推。
    v.anim_due_at = Some(at(150));
    v.install(pa.clone(), frame(2));
    let l = lead(v.anim_next_at.expect("同帧重解后重排"));
    assert!(
        (150..=260).contains(&l),
        "同帧重解该重新起表（+200），续成 +350 就等于凭空多等一帧，实际领先 {l} ms"
    );

    // ④ 换图 = 新的一轮：哪怕帧号前进了，也不许接着上一张图的时间线走
    //    （旧图欠着的账会让新图第一帧立刻开跑，帧序看着像跳）。
    v.anim_due_at = Some(at(150));
    v.install(_pb.clone(), frame(5));
    let l = lead(v.anim_next_at.expect("换图后重排"));
    assert!(
        (150..=260).contains(&l),
        "换图该重新起表（+200），不该续旧图的账，实际领先 {l} ms"
    );
}

/// 信息条格式自检：「第几张/共几张 · 缩放百分比」——2026-10-12 用户拍板条上
/// 只留这两样（尺寸、动画帧数这类细节交给「文件信息」）。
fn is_count_and_percent(s: &str) -> bool {
    // 格式：{pos} · {pct}% [({index}{/count})]（没有帧/页号时只有前两段）。
    let Some((first, rest)) = s.split_once(" · ") else {
        return false;
    };
    let Some((a, b)) = first.split_once('/') else {
        return false;
    };
    let pos_ok = a.parse::<usize>().is_ok() && b.parse::<usize>().is_ok();
    // rest 里要能找到 "N%"（百分比段），后面可能还有 " · 帧/页 M/K"。
    let pct_ok = rest
        .split('%')
        .next()
        .and_then(|p| p.trim().parse::<f32>().ok())
        .is_some();
    pos_ok && pct_ok
}

/// 信息条只报「数量 · 百分比 · 帧/页 N/M」（动图播放中也报张号——计数器在跳
/// 就是自动播的证据；静态位图不报第三段，见下一条）。
#[test]
fn status_is_count_and_zoom_percent() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    let anim = Decoded {
        natural: (80, 60),
        region: None,
        anim: Anim {
            index: 0,
            count: 4,
            delay_ms: 200,
            paged: false,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), anim));
    let s = v.status();
    assert!(
        is_count_and_percent(&s),
        "信息条应是「x/x · N%」，实际：{s}"
    );
}

/// 静态图状态栏绝不该带"动画"字样。
#[test]
fn status_static_has_no_animation_tag() {
    use crate::core::pixels::Bitmap;
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    let still = Decoded {
        natural: (80, 60),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), still));
    let s = v.status();
    assert!(!s.contains("动画"), "静态图状态栏不应标动画，实际：{s}");
    assert!(!s.contains("(1/1)"), "静态位图没有帧/页号，实际：{s}");
    assert!(is_count_and_percent(&s), "基础信息必须在，实际：{s}");
    assert!(!v.has_index(), "静态位图不该挂播放组");
}

/// ★ 用户 2026-11-06 口径：帧/页进度与逐帧按钮只给「真有第二张」的格式。
/// 判据吃解码器回填的多张语义，不按扩展名维护第二份白名单——那十几二十种格式
/// 的解码器才会报 `count>1` / `delay_ms>0` / `paged`。
#[test]
fn frame_progress_reserved_for_real_multi_image_formats() {
    use crate::core::pixels::Anim;
    // 静态位图 / 单张 SVG / 单页 PDF：只有一张，不给进度也不给按钮。
    assert!(!Anim::STATIC.has_index(), "静态图恒为一张，不该报进度");
    assert!(
        !Anim {
            count: 1,
            ..Anim::STATIC
        }
        .has_index(),
        "单张就是静态"
    );
    // 真动图：帧数或延时任一成立；总数未知的畸形 GIF 靠延时进来。
    assert!(Anim {
        count: 4,
        delay_ms: 0,
        ..Anim::STATIC
    }
    .has_index());
    assert!(Anim {
        count: 0,
        delay_ms: 100,
        ..Anim::STATIC
    }
    .has_index());
    // 多页文档：`paged` 单独也是充分条件（页间隔由宿主给）。
    assert!(Anim {
        count: 3,
        paged: true,
        ..Anim::STATIC
    }
    .has_index());
}

/// ★ m50-C 的承重判据：静图上按 `P` / `,` / `.` **必须有话说**，而且不许挂出
/// 假暂停态。★ 用户 2026-11-06：这句话走顶部消息窗（取走 `note`），状态格的
/// 「数量 · 百分比」一个字都不许被替换。把两条守卫还原，本条立刻红。
#[test]
fn still_image_anim_keys_say_why() {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    assert!(v.install(
        pa.clone(),
        Decoded {
            natural: (80, 60),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 2,
                height: 2,
                rgba: vec![0u8; 16],
            },
        }
    ));
    // ① 逐帧键：说得出"为什么没反应"，且不动任何帧状态。
    v.step_anim(1);
    assert_eq!(v.take_note(), Some(NOTE_STILL), "按 `,` `.` 要给出那句话");
    assert_eq!(v.take_note(), None, "同一句只许弹一次");
    assert!(
        is_count_and_percent(&v.status()),
        "提示不许顶替基础信息，实际：{}",
        v.status()
    );
    assert_eq!(
        v.loaded.as_ref().map(|l| l.anim.index),
        Some(0),
        "静图不许响应逐帧——帧号原地不动"
    );
    // ② 暂停键：既要说，也不许把静图翻成暂停态。
    v.toggle_anim_pause();
    assert!(
        !v.anim_paused(),
        "静图不该有暂停态——那是个屏幕上看不见的假状态"
    );
    assert_eq!(v.take_note(), Some(NOTE_STILL));
    assert!(is_count_and_percent(&v.status()), "状态格仍是基础信息");
    // ③ 翻一页就作废：提示说的是上一张图，别赖在新图上。
    v.step(1);
    assert_eq!(v.take_note(), None, "换图后旧提示必须作废");
    assert!(
        is_count_and_percent(&v.status()),
        "应回到「x/x · N%」，实际：{}",
        v.status()
    );
}

/// 过期的一句不许再弹；状态格始终只有基础信息。
#[test]
fn expired_note_is_never_shown() {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    assert!(v.install(
        pa.clone(),
        Decoded {
            natural: (80, 60),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 2,
                height: 2,
                rgba: vec![0u8; 16],
            },
        }
    ));
    // 挂一条"很久以前"的：直接把时刻往前挪，不靠 sleep（那会让单测看时钟脸色）。
    v.note = Some((
        NOTE_STILL,
        Instant::now() - NOTE_TTL - Duration::from_millis(1),
    ));
    assert_eq!(v.take_note(), None, "过期的一句不许弹出来");
    assert!(
        is_count_and_percent(&v.status()),
        "状态格始终是「x/x · N%」，实际：{}",
        v.status()
    );
}

/// 反向护栏：真有动画时 `P` 的行为一个字都不许被这条守卫改掉。
#[test]
fn animated_image_still_pauses_and_shows_frame() {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    assert!(v.install(
        pa.clone(),
        Decoded {
            natural: (80, 60),
            region: None,
            anim: Anim {
                index: 0,
                count: 4,
                delay_ms: 200,
                paged: false,
            },
            bitmap: Bitmap {
                width: 2,
                height: 2,
                rgba: vec![0u8; 16],
            },
        }
    ));
    v.toggle_anim_pause();
    assert!(v.anim_paused(), "动画图按 P 该停下来");
    let s = v.status();
    assert!(s.contains("(1/4)"), "停下来要看得见第几帧，实际：{s}");
    assert_ne!(s, NOTE_STILL);
}

/// 缩略图条窗口：开头不"居中到负数"、中间真的居中、末尾不空出半排。
#[test]
fn strip_window_centering_and_clamping() {
    let (len, n) = (12, 5);
    // 开头：当前图在第 1 格（没有"前面两张"可留）。
    assert_eq!(strip_window_first(len, 0, n), 0);
    assert_eq!(strip_window_first(len, 1, n), 0);
    // 中间：当前图居正中（5 格 → 前面留 2）。
    assert_eq!(strip_window_first(len, 5, n), 3);
    assert_eq!(strip_window_first(len, 6, n), 4);
    // 末尾：不能再居中，否则右边空出两格。
    assert_eq!(strip_window_first(len, 11, n), len - n);
    // 目录比一屏还少：从 0 开始，格数收缩到目录长度。
    assert_eq!(strip_window_first(3, 2, n), 0);
    // 单格窗口永远跟着当前图走。
    assert_eq!(strip_window_first(len, 7, 1), 7);
    // 空目录不该 panic（除零 / 下溢）。
    assert_eq!(strip_window_first(0, 0, n), 0);
}

/// ★ m260：用户滚过之后（`anchor = Some`）条**就停在滚到的地方**，哪怕当前图完全在
/// 视野之外——这是用户 2026-11-08 要的「拖动滚动条移动，直接切换到很远的图」。
///
/// m258 第一版在这里写了"当前图跑出视野就把它带回来"。看着合理，实际等于**拖不动**：
/// 拖条本身不换图，一拖到当前格看不见，下一帧就被拽回原处。现在这条函数**签名里
/// 根本没有 `cur`**——结构上就不存在把当前图拽回来的可能，比任何注释都硬。
#[test]
fn strip_window_anchored_never_pulls_the_current_picture_back() {
    // 锚点在中间：原样返回。
    assert_eq!(strip_window_anchored(40, 8, 8), 8);
    // 锚点远离当前图（当前图无论在第几张都查不到、也不需要查）：依旧原样。
    assert_eq!(strip_window_anchored(40, 8, 30), 30);
    assert_eq!(strip_window_anchored(40, 8, 0), 0);
    // 锚点越界（目录变短）⇒ 先夹进合法区间（最后一屏的起点）。
    assert_eq!(strip_window_anchored(40, 8, 999), 32);
    // 目录比一屏还少 ⇒ 恒 0；空目录不许 panic。
    assert_eq!(strip_window_anchored(5, 8, 3), 0);
    assert_eq!(strip_window_anchored(0, 8, 0), 0);
    // 单格窗口：锚点就是那一张。
    assert_eq!(strip_window_anchored(40, 1, 20), 20);
    assert_eq!(strip_window_anchored(40, 1, 999), 39);
}

/// ★ m100：`strip_window` 交出的**序号**与 `strip_path` 交出的**路径**必须是同一张图。
///
/// 本轮把"整排 `PathBuf` 交给画布"换成"画布按序号来借"，代价是这两句之间多了一条
/// 约定：`(first, n)` 里的 `first` 一旦和 `strip_path` 的起点差一格，屏幕上就是
/// **整排缩略图错位一格**——而 `strip_window_first` 那条纯算术用例照旧全绿。
/// 所以这里跑真状态机：窗口起点、格数、当前图三者对齐。
#[test]
fn the_strip_window_indices_and_the_borrowed_paths_are_the_same_pictures() {
    let dir = crate::core::paths::test_scratch_root()
        .join(format!("sv-m100-pair-{}", std::process::id()));
    // 先清掉可能的残留：pid 会被 Windows 回收，同名目录里留着上个进程的文件 ⇒ 用例继承别人的状态（m106）
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a", "b", "c", "d", "e", "f", "g"] {
        std::fs::File::create(dir.join(format!("{n}.svg"))).unwrap();
    }
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    // 停在第 4 张（`d`）：7 张目录、5 格窗口 → 窗口既不是从 0 开始，也没贴到末尾。
    v.open(&dir.join("d.svg"));
    let (first, n) = v.strip_window(5, None);
    assert_eq!((first, n), (1, 5), "当前图居中：前面留 2 ⇒ 起点是第 2 张");
    let g = v.gallery.as_ref().unwrap();
    let want: Vec<PathBuf> = (first..first + n)
        .map(|i| g.peek(i).unwrap().clone())
        .collect();
    let got: Vec<PathBuf> = (0..n)
        .map(|i| v.strip_path(first + i).unwrap().to_path_buf())
        .collect();
    assert_eq!(got, want, "按序号借来的路径必须就是清单里那几份");
    assert!(
        got.contains(&v.current_path().unwrap()),
        "当前那张必须在窗口里，否则高亮格根本不在屏上"
    );
}

// ── S3 · 查询阶段 Meta ─────────────────────────────────────

use crate::core::decode::Meta;

fn meta_event(seq: u64, path: &str, natural: (u32, u32)) -> MetaEvent {
    MetaEvent {
        seq,
        path: std::path::PathBuf::from(path),
        meta: Meta { natural },
    }
}

/// `apply_meta` 只认同 `want_seq`、且 `loaded` 为空——预览的窗口期只在
/// 主画面"Frame 还没到"那一段。过期/预读/已有位图一律丢弃。
#[test]
fn apply_meta_accepts_only_current_seq_when_no_bitmap() {
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.want_seq = 7;
    // 不匹配 seq：过期/预读，忽略。
    assert!(!v.apply_meta(meta_event(8, "x.svg", (640, 480))));
    assert!(v.preview_meta.is_none());
    // 匹配 seq：接受、info bar 立即可见预览尺寸。
    assert!(v.apply_meta(meta_event(7, "x.svg", (1024, 768))));
    assert_eq!(v.info_dims(), Some((1024, 768)));
}

/// 渲染结果一到就"以渲染为准"：install 里 `loaded` 就位、`preview_meta` 清空，
/// `info_dims` 走 loaded.natural——即便查询/渲染尺寸不一致（解码器 bug）也不留残余。
#[test]
fn install_supersedes_preview() {
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.want_seq = 3;
    // 预览先报 4000×3000（假设 header-only 查询解码器把 ispe 报了个大的）。
    assert!(v.apply_meta(meta_event(3, "x.svg", (4000, 3000))));
    assert_eq!(v.info_dims(), Some((4000, 3000)));
    // 渲染回来的实际位图 2000×1500（不同！）——install 必须把预览抹掉。
    use crate::core::pixels::{Anim, Bitmap};
    let d = Decoded {
        natural: (2000, 1500),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: 8,
            height: 6,
            rgba: vec![0u8; 8 * 6 * 4],
        },
    };
    assert!(v.install(std::path::PathBuf::from("x.svg"), d));
    assert!(v.preview_meta.is_none(), "install 后预览必须清空");
    assert_eq!(v.info_dims(), Some((2000, 1500)), "以渲染值为准");
}

/// 已有位图（loaded 非空）时 Meta 一律忽略——即便 seq 匹配。翻页过渡期间
/// 屏幕还在显示旧图，用新图的预览尺寸刷 info bar 反而误导。
#[test]
fn apply_meta_ignored_while_bitmap_present() {
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.want_seq = 5;
    use crate::core::pixels::{Anim, Bitmap};
    let d = Decoded {
        natural: (800, 600),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: 8,
            height: 6,
            rgba: vec![0u8; 8 * 6 * 4],
        },
    };
    assert!(v.install(std::path::PathBuf::from("cur.svg"), d));
    // 下一张的 Meta 提前到（假设宿主为动画帧发了新 seq=5？—— 其实 want_seq 未变，
    // 但为演示"有 loaded 就不接受"，seq 故意对上）：应被 apply_meta 忽略。
    assert!(!v.apply_meta(meta_event(5, "next.svg", (1600, 900))));
    assert_eq!(v.info_dims(), Some((800, 600)));
}

/// Meta 抵达时底部状态栏把预览尺寸拼进"载入中"，让 header-only 就能报
/// 尺寸的格式（HEIF ispe / RAW 只读头 / PNG IHDR）在整图解码之前就有
/// 尺寸反馈。装到位图后（install）回到"N/M · P%"，不带预览。
#[test]
fn status_shows_preview_dims_while_loading() {
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    // 打开之后 loaded 尚未落地，want_seq 是刚发出的解码请求；
    // 模拟该 seq 的 Meta 先回。
    v.want_seq = 42;
    assert!(v.apply_meta(meta_event(42, "a.svg", (4032, 3024))));
    let s = v.status();
    assert!(
        s.contains("4032×3024") && s.contains("载入中"),
        "载入中应带预览尺寸，实际：{s}"
    );
}

// ── 逐帧浏览（环形缓存 / step_anim / 状态栏）──────────────────

/// 开好图、拿到视口的 Viewer（逐帧那组测试共用）。
fn anim_viewer() -> (Viewer, std::path::PathBuf) {
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    v.on_viewport(0.0, 0.0, 400.0, 400.0, 1.0);
    (v, pa)
}

/// 造一帧动画回包：100×100 RGBA（40_000 字节），帧号/总数可控。
fn anim_done(path: std::path::PathBuf, seq: u64, index: u32, count: u32) -> Done {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    Done {
        seq,
        path,
        out: Ok(Decoded {
            natural: (100, 100),
            region: None,
            anim: Anim {
                index,
                count,
                delay_ms: 40,
                paged: false,
            },
            bitmap: Bitmap {
                width: 100,
                height: 100,
                rgba: vec![index as u8; 100 * 100 * 4],
            },
        }),
    }
}

/// 未命中才发按号请求；没暂停的图按逐帧要先自动暂停。
#[test]
fn step_anim_miss_asks_the_decoder_by_index() {
    let (mut v, pa) = anim_viewer();
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 8));
    assert!(!v.anim_paused());
    let seq = v.want_seq;
    v.step_anim(-1);
    assert!(v.anim_paused(), "按逐帧即进暂停态");
    assert_ne!(v.want_seq, seq, "未命中必须发一次按号请求");
}

/// 跳到第 N 张：一次按号请求（不是连按 N 下），越界夹到末张，且先自动暂停。
#[test]
fn jump_anim_asks_by_index_and_clamps() {
    let (mut v, pa) = anim_viewer();
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 340));
    let seq = v.want_seq;
    assert!(v.jump_anim(199), "第 200 页要能跳");
    assert!(
        v.anim_paused(),
        "跳转即进暂停态（否则下一帧调度立刻把画面拽走）"
    );
    assert_ne!(v.want_seq, seq, "必须发一次按号请求");
    assert_eq!(
        v.step_target,
        Some(199),
        "界面 1 起、核心 0 起：第 200 = 199"
    );
    // 越界：夹到末张，不拒、不 panic（输入框那一层已经拒过一次，这里是第二道）。
    assert!(v.jump_anim(9999));
    assert_eq!(v.step_target, Some(339));
}

/// 总数未知（`count == 0`）与静图都不受理：按号跳无从夹取，宁可不动。
#[test]
fn jump_anim_is_ignored_without_a_known_last_frame() {
    let (mut v, pa) = anim_viewer();
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 0));
    let seq = v.want_seq;
    assert!(!v.jump_anim(5), "总数未知不该猜");
    assert_eq!(v.want_seq, seq, "不该发请求");

    // 静图：`has_index()` 为假（count=1 / delay=0），同样不受理。
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let still = Done {
        seq: v.want_seq,
        path: pa,
        out: Ok(Decoded {
            natural: (10, 10),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: 10,
                height: 10,
                rgba: vec![0; 10 * 10 * 4],
            },
        }),
    };
    v.apply(still);
    assert!(!v.jump_anim(1));
}

/// 排序口径从配置来、落到清单上：**代数要涨**（界面据此归零按槽位记的状态），
/// 当前这张不许换（它是锚点）。
#[test]
fn sync_sort_reorders_the_listing_and_bumps_the_epoch() {
    let (mut v, pa) = anim_viewer();
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 2));
    let before = v.sort_epoch();
    assert_eq!(v.sort(), SortSpec::DEFAULT, "出厂口径是文件名升序");
    assert!(!v.sync_sort(), "配置没变就不该重排");

    v.config.borrow_mut().sort_by = crate::core::gallery::SortBy::Size;
    assert!(v.sync_sort(), "配置变了要重排");
    assert_eq!(
        v.sort_epoch(),
        before + 1,
        "代数要涨（界面据此清缩略图条锚点）"
    );
    assert_eq!(
        v.gallery.as_ref().unwrap().current().cloned(),
        Some(pa),
        "重排不许换掉当前这张"
    );
    assert!(!v.sync_sort(), "同一口径第二次不该再重排");
}

/// ★ 连按不许丢步：后退一步可能要付一次「重建 + 快进」，这期间屏幕上还是旧帧。
/// 目标帧号必须以"在途目标"为基准往下累加，否则按 N 下只退 1 帧（真机实测：
/// 从第 92 帧连按 91 下 `,`，只退到 91）。
#[test]
fn rapid_steps_accumulate_targets_while_decode_is_in_flight() {
    let (mut v, pa) = anim_viewer();
    v.set_anim_paused(true);
    v.apply(anim_done(pa.clone(), v.want_seq, 5, 8));
    assert_eq!(v.step_target, None, "刚上屏不该有在途目标");
    for want in [4u32, 3, 2] {
        v.step_anim(-1);
        assert_eq!(v.step_target, Some(want), "连按时要退到第 {want} 帧");
    }
    // 最后那枚目标落地，其余回包按 seq 丢弃 → 屏幕停在用户按到的地方。
    v.apply(anim_done(pa.clone(), v.want_seq, 2, 8));
    assert_eq!(v.step_target, None, "上屏后在途标记要让位给屏幕上的那一帧");
    assert_eq!(v.loaded.as_ref().unwrap().anim.index, 2);
    // 再按一下从 2 继续 → 1，不是从"最初显示的 5"重来。
    v.step_anim(-1);
    assert_eq!(v.step_target, Some(1));
}

/// 连按按回到"屏幕上这一帧"：必须发一次请求顶掉在途的旧目标，否则那枚旧回包
/// 稍后落地，画面自己走到用户已经取消的帧上。
#[test]
fn stepping_back_to_the_displayed_frame_cancels_the_pending_target() {
    let (mut v, pa) = anim_viewer();
    v.set_anim_paused(true);
    v.apply(anim_done(pa.clone(), v.want_seq, 5, 8));
    v.step_anim(-1);
    let seq = v.want_seq;
    v.step_anim(1);
    assert_ne!(v.want_seq, seq, "必须发一次请求把在途的第 4 帧顶掉");
    assert_eq!(v.step_target, Some(5));
    // 没有在途目标时，"原地不动"那一步仍然安静地什么都不做（单帧图不空转）。
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 1));
    let settled = v.want_seq;
    v.step_anim(1);
    assert_eq!(v.want_seq, settled, "总数 1 → 取模回自己，不该发请求");
    assert_eq!(v.step_target, None);
}

/// 帧数未知（GIF 第一圈）：前进 +1 交给解码器回绕，后退夹在 0，不编假总数。
#[test]
fn step_anim_with_unknown_count_never_invents_a_target() {
    let (mut v, pa) = anim_viewer();
    v.set_anim_paused(true);
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 0));
    let seq = v.want_seq;
    v.step_anim(-1);
    assert_eq!(v.want_seq, seq, "没有目标就不该发请求");
    // 前进 +1 是合法目标；宿主不再自存帧，所以只能发请求。
    v.step_anim(1);
    assert_ne!(v.want_seq, seq, "未命中必须发一次按号请求");
}

/// 状态栏：动画暂停时给「帧 i/count」；总数未知就只写「帧 i」，不写 `?`、不写「已暂停」。
#[test]
fn status_shows_frame_without_pause_label() {
    let (mut v, pa) = anim_viewer();
    // 先落一帧：**首帧落地 = 换图**，会把播放态归零（见 `install`），
    // 所以"用户按暂停"要排在那之后。
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 8));
    v.set_anim_paused(true);
    v.apply(anim_done(pa.clone(), v.want_seq, 5, 8));
    let s = v.status();
    assert!(s.contains("(6/8)") && !s.contains("已暂停"), "实际：{s}");
    // 未知总数（畸形文件）：宁可不写分母，也不写一个看着像 bug 的 `?`。
    let (mut v2, pa2) = anim_viewer();
    v2.apply(anim_done(pa2.clone(), v2.want_seq, 0, 0));
    v2.set_anim_paused(true);
    v2.apply(anim_done(pa2, v2.want_seq, 2, 0));
    let s2 = v2.status();
    assert!(s2.contains("(3)"), "总数未知不该有分母，实际：{s2}");
    assert!(!s2.contains('?'), "不许出现 ?，实际：{s2}");
    // 播放态也报张号（2026-11-02 用户拍板"计数器在跳就是证据"）。
    let (mut v3, pa3) = anim_viewer();
    v3.apply(anim_done(pa3, v3.want_seq, 2, 8));
    let s3 = v3.status();
    assert!(
        s3.contains("(3/8)"),
        "播放态也报张号（自动播的证据），实际：{s3}"
    );
}

/// 错误**不进状态栏**：它只走画面中央那一句（`error_text`），两处都写同一句话
/// 就是冗余（用户 2026-10-18 拍板"只留中央"）。
#[test]
fn error_does_not_enter_the_status_line() {
    let (mut v, _pa) = anim_viewer();
    v.error = Some(crate::core::error::ViewFailure::other(
        "文件已损坏： PNG 库报错 \"Unknown filter method 68.\"",
    ));
    let s = v.status();
    assert!(!s.contains("损坏"), "状态栏不许重复错误，实际：{s}");
    assert!(!s.contains("载入中"), "失败态不许谎称还在载入，实际：{s}");
    assert_eq!(s, "1/3", "失败时状态栏只留位置号");
    assert!(v.error_text().is_some(), "中央那句的取数口不能被顺手关掉");
}

/// ★ 升级/动画帧失败不许把已经上屏的好图清掉（m92，m91 审计 D1）。
///
/// 失败回包分两种：首解失败（手里没图）→ 空白 + 中央错误；升级失败（好图在屏上）
/// → 像素仍然有效，只停动画、松闩、记日志。旧代码无条件清 `loaded`，这条会红。
#[test]
fn failed_upgrade_keeps_the_image_on_screen() {
    let (mut v, pa) = anim_viewer();
    assert!(v.apply(anim_done(pa.clone(), v.want_seq, 0, 4)));
    let seq = v.want_seq;
    // 模拟一枚原图档升级在途：闩按「路径 + 当前 seq」记。
    v.native_requested = Some((pa.clone(), seq));
    v.anim_next_at = Some(Instant::now());
    v.anim_due_at = Some(Instant::now());

    assert!(
        !v.apply(Done {
            seq,
            path: pa.clone(),
            out: Err(crate::core::error::ViewFailure::decode(
                "解码器「x」解到一半崩了",
            )),
        }),
        "当前帧还在屏上，没有可见变化，不该标脏"
    );

    let img = v.image().expect("好图被一次失败的升级清掉了");
    assert_eq!(
        (img.width, img.height),
        (100, 100),
        "保留的必须是原来那份像素"
    );
    assert!(v.error_text().is_none(), "屏上还有好图，中央不该摆错误句");
    assert!(
        v.native_requested.is_none(),
        "失败的升级必须松闩，否则这张图永远停在低清档"
    );
    assert!(
        v.anim_next_at.is_none() && v.anim_due_at.is_none(),
        "失败的动画帧要把时间线停掉，不许继续排下一帧"
    );
}

/// 反向护栏：换图窗口期失败时，屏上的旧图**不许**冒充新图留下。
///
/// `keep` 的判据必须同时看路径，不能只看"loaded 非空"——否则 A→B 的首解失败
/// 会让 A 的像素挂在 B 的文件名下，比空白更糟。
#[test]
fn failed_first_decode_does_not_keep_the_previous_image() {
    let (mut v, pa) = anim_viewer();
    assert!(v.apply(anim_done(pa.clone(), v.want_seq, 0, 4)));
    v.step(1); // 翻到 B；防闪设计让 A 还留在 loaded 里
    let cur = v.current_path().expect("清单该有当前项");
    assert_ne!(cur, pa, "这一步要真的翻到另一张");
    let seq = v.want_seq;

    assert!(
        v.apply(Done {
            seq,
            path: cur.clone(),
            out: Err(crate::core::error::ViewFailure::decode("B 解不开")),
        }),
        "清掉旧图是可见变化，必须标脏"
    );
    assert!(v.image().is_none(), "旧图 A 的像素不许挂在 B 的文件名下");
    assert!(v.error_text().is_some(), "首解失败要报中央错误");
}

// ── 第四批审计（2026-10-18）：回包身份 / 换图窗口期 / 升级闩 ──────

/// ★ 上一张在途的回包不许盖掉刚翻到的新图，**seq 相符也不行**。
/// 旧裁决只比 seq，而 `install` 不推进 `want_seq`（预读命中那条路压根没发请求），
/// 于是"升级 A 的那一枚晚到一步"就把 B 换回 A：画面是上一张、状态栏是新文件名。
#[test]
fn a_late_reply_cannot_overwrite_the_new_image_even_when_the_seq_matches() {
    let (mut v, pa) = anim_viewer();
    let (_dir, [_pa, pb, _pc]) = gallery_dir();
    v.apply(anim_done(pa.clone(), v.want_seq, 0, 0));
    // A 的原图档升级还在途，用户先翻到了 B。
    v.step(1);
    let seq_b = v.want_seq;
    v.apply(anim_done(pb.clone(), seq_b, 0, 0));
    assert_eq!(v.loaded.as_ref().unwrap().path, pb, "B 该已上屏");
    // A 那枚回包现在才到，且 seq 正好等于"当前期待值"。
    v.apply(anim_done(pa.clone(), v.want_seq, 7, 0));
    assert_eq!(
        v.loaded.as_ref().unwrap().path,
        pb,
        "上一张的晚到回包把新图盖掉了"
    );
}

/// 屏幕上还顶着旧图时按 `,`：那是**旧图**的属性，替它发帧请求会顶掉新图的首解、
/// 再把新图拽进暂停态，画面冻在上一张。
#[test]
fn stepping_before_the_new_image_lands_does_not_request_for_the_old_one() {
    let (mut v, pa) = anim_viewer();
    let (_dir, [_pa, pb, _pc]) = gallery_dir();
    v.apply(anim_done(pa.clone(), v.want_seq, 3, 8));
    v.step(1);
    assert_eq!(
        v.gallery.as_ref().unwrap().current().unwrap(),
        &pb,
        "清单该已翻到 B"
    );
    assert_eq!(v.loaded.as_ref().unwrap().path, pa, "翻页不清旧图（防闪）");
    let seq_b = v.want_seq;
    v.step_anim(-1);
    assert_eq!(v.want_seq, seq_b, "旧图还在屏上时逐帧不该发任何请求");
    assert!(!v.anim_paused(), "更不该把新图拽进暂停态");
    assert_eq!(v.step_target, None, "也不该记下在途目标");
}

/// ★ 畸形文件报"这一帧停 49 天"（APNG 的 delay 是 u32，真给得出来）时不许把动画
/// 永久冻住——排表夹在 [`ANIM_MAX_DELAY_MS`] 之内，用户看到的仍是"它在动"。
#[test]
fn an_absurd_frame_delay_is_clamped_not_obeyed() {
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (mut v, pa) = anim_viewer();
    let want = v.want_seq;
    assert!(v.apply(Done {
        seq: want,
        path: pa,
        out: Ok(Decoded {
            natural: (100, 100),
            region: None,
            anim: Anim {
                index: 0,
                count: 8,
                delay_ms: u32::MAX,
                paged: false,
            },
            bitmap: Bitmap {
                width: 100,
                height: 100,
                rgba: vec![0u8; 100 * 100 * 4],
            },
        }),
    }));
    let at = v
        .anim_next_at
        .expect("delay>0 就该排动画时刻表，哪怕是畸形值");
    let wait = at.saturating_duration_since(Instant::now());
    assert!(
        wait <= Duration::from_millis(ANIM_MAX_DELAY_MS as u64 + 50),
        "排表该被夹住，实际还要等 {wait:?}"
    );
}

/// 升级闩只在"那一枚请求仍然在途"时成立。
///
/// 为什么直接测这道判据而不是驱动 `tick`：单测环境里没装 `FormatSource`，
/// 清单只看得见内置的 `.svg`，而矢量走的是切片重渲那条分支，压根到不了这道闩。
#[test]
fn the_native_latch_only_holds_while_that_request_is_still_in_flight() {
    let (mut v, pa) = anim_viewer();
    let (_dir, [_pa, pb, _pc]) = gallery_dir();
    assert!(!v.native_in_flight(&pa), "没发过请求就不该闩着");
    v.native_requested = Some((pa.clone(), 7));
    v.want_seq = 7;
    assert!(
        v.native_in_flight(&pa),
        "那一枚还在途 → 闩住，同一档不许整解两次"
    );
    // 后来的主画面请求顶掉了它：回包永远不会落地，闩必须随之失效。
    v.want_seq = 8;
    assert!(
        !v.native_in_flight(&pa),
        "被顶掉的升级必须能重发，否则这张图永久停在视口档糊面"
    );
    // 别的图不共享这道闩。
    v.want_seq = 7;
    assert!(!v.native_in_flight(&pb), "闩是逐图的，别把邻居也闩上");
}

// ── 解码等太久：说清楚"在等什么"（m71 / 待办 §18 ⑥）──────────────
//
// 现场取证：解码器 `iv_render` 睡 30 s 时，13 s 后画面中央还写着「把图片拖进来」，
// 状态栏还是「1/1 · 载入中…」——用户分不清"在解"和"卡死了"。这一组用例钉住
// 三档时机（<3 s 不打扰、≥3 s 报时长、≥10 s 报可能卡住）和四个"不该报"。

fn waiting_viewer() -> (Viewer, std::path::PathBuf) {
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&pa);
    (v, pa)
}

/// 把"发出时刻"拨回 `d` 之前，模拟这一枚请求已经等了这么久。
fn sent_ago(v: &Viewer, d: Duration) {
    v.main_sent_at
        .set(Some(Instant::now().checked_sub(d).expect("时刻可回拨")));
}

#[test]
fn wait_notice_appears_after_three_seconds_and_escalates_at_ten() {
    let (v, _pa) = waiting_viewer();
    assert_eq!(v.status(), "1/3 · 载入中…", "刚发出：只说在载入");
    assert!(v.awaiting_first_bitmap());

    sent_ago(&v, Duration::from_millis(2900));
    assert!(
        !v.status().contains("已等"),
        "3 s 以内的正常解码不许打扰：{}",
        v.status()
    );

    sent_ago(&v, Duration::from_secs(4));
    let s = v.status();
    assert!(
        s.contains("载入中… 4 秒") && !s.contains("卡住"),
        "过 3 s 要报等了多久，但还不到下判词的时候：{s}"
    );

    sent_ago(&v, Duration::from_secs(11));
    let s = v.status();
    assert!(
        s.contains("11 秒") && s.contains("可能卡住"),
        "过 10 s 要给「可能卡住」这句判词：{s}"
    );
}

/// 屏上那张**就是**当前图时，解码再慢也不改"你在看什么"——不报等待。
#[test]
fn wait_is_silent_while_the_current_image_is_on_screen() {
    let (mut v, pa) = waiting_viewer();
    assert!(v.apply(decoded4(pa)));
    sent_ago(&v, Duration::from_secs(11));
    let s = v.status();
    assert!(
        !s.contains("载入中"),
        "当前图已上屏，状态栏不该谎称在等：{s}"
    );
    assert!(!v.awaiting_first_bitmap(), "有位图就不是首图空白期");
}

/// 失败已经落地：中央那句是唯一载体，状态栏只留位置号——不许继续说"载入中"。
#[test]
fn wait_is_silent_after_an_error() {
    let (mut v, _pa) = waiting_viewer();
    v.error = Some(crate::core::error::ViewFailure::decode(
        "解码器「x」解码失败",
    ));
    sent_ago(&v, Duration::from_secs(11));
    assert_eq!(v.status(), "1/3", "出错后状态栏只留位置号");
    assert!(!v.awaiting_first_bitmap(), "出错不算首图空白期");
}

/// 等待秒数每秒主动标脏一次——否则状态栏停在请求发出时那句话（真机取证）。
#[test]
fn wait_seconds_repaint_once_per_second() {
    let (mut v, _pa) = waiting_viewer();
    sent_ago(&v, Duration::from_millis(2900));
    assert!(!v.tick(), "不到 3 s：不报也不重绘");
    sent_ago(&v, Duration::from_secs(3));
    assert!(v.tick(), "满 3 s 要重绘一次，否则新文案上不了屏");
    assert!(!v.tick(), "同一秒内不许每帧重绘（省电）");
    sent_ago(&v, Duration::from_secs(4));
    assert!(v.tick(), "过整秒就要再标一次脏");
}

/// 卡住 warn 每枚请求最多一条；发下一枚请求时双双重置。
#[test]
fn stall_log_is_once_per_request_and_reset_by_the_next_one() {
    let (mut v, _pa) = waiting_viewer();
    sent_ago(&v, Duration::from_millis(5000));
    let _ = v.tick();
    assert!(!v.stall_logged.get(), "5 s 还不到写 warn 的时候");

    sent_ago(&v, Duration::from_secs(11));
    let _ = v.tick();
    assert!(v.stall_logged.get(), "过 10 s 就该写一条 warn");
    let _ = v.tick();
    assert!(v.stall_logged.get(), "同一枚请求不重复写");

    v.arm_main_wait();
    assert!(!v.stall_logged.get(), "新请求必须复位，否则下一张卡住不响");
    assert!(v.pending_wait().is_none(), "刚发出的请求不该立刻被当成卡住");
}

/// 新图默认从**播放态**开始（`anim_autoplay` 默认开；2026-11-02 用户拍板
/// "多帧/多页都自动播放，否则看不出来"）。
///
/// ★ 防三条回归：
/// 1. 打开分页文档必须**正在播**（1 秒/页），不许停在第 1 页等人按 ▶。
/// 2. 同一张图翻到下一页**不许**把用户按下的暂停态吃掉（装档与换图走同一个函数）。
/// 3. 换到别的图必须**归零**——`anim_paused` 是跨图的单字段，不归零就是"在 PDF 上
///    停过之后，切到的 GIF 也不动"（用户 2026-11-02 报的"gif 按钮/动画没了"）。
#[test]
fn anim_autoplay_off_starts_paused_and_setting_toggles_the_current_picture() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let cfg = Rc::new(RefCell::new(Config {
        anim_autoplay: false,
        ..Config::default()
    }));
    let mut v = Viewer::new(Rc::new(Decoder::spawn_with(tx)), cfg);
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);
    let gif = |index: u32| Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index,
            count: 3,
            delay_ms: 100,
            paged: false,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    let still = Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };

    assert!(v.install(pa.clone(), gif(0)));
    assert!(v.anim_paused(), "自动播放关：GIF 首帧应停住");
    assert!(v.install(pa.clone(), gif(1)));
    assert!(v.anim_paused(), "同一张图翻页不许吃掉暂停态");

    assert!(v.install(pb.clone(), still));
    assert!(!v.anim_paused(), "静态图没有可播时间线，不该挂假暂停态");

    assert!(v.install(pa.clone(), gif(0)));
    assert!(v.anim_paused(), "换回 GIF 仍按配置停在第 0 帧");

    v.set_anim_autoplay(true);
    assert!(!v.anim_paused(), "设置页打开自动播放，当前这张应立即恢复");
    v.set_anim_autoplay(false);
    assert!(v.anim_paused(), "设置页关闭自动播放，当前这张应立即暂停");
}

#[test]
fn every_new_picture_starts_playing_and_paging_keeps_the_users_state() {
    use crate::core::pixels::{Anim, Bitmap};
    let (_dir, [pa, pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);

    let page = |index: u32| Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index,
            count: 5,
            delay_ms: Anim::PAGE_DELAY_MS,
            paged: true,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    // 动图（非分页）：换到它身上时暂停态必须被清掉。
    let gif = |index: u32| Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index,
            count: 3,
            delay_ms: 100,
            paged: false,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };

    assert!(v.install(pa.clone(), page(0)));
    assert!(!v.anim_paused, "分页文档打开就该播，不许停在第 1 页");
    assert!(v.is_animated(), "分页文档要挂上播放组（与 GIF 同一套 UI）");

    // 用户手动逐帧（step_anim 会进暂停态）之后，下一页的装档不许把他拽回播放。
    v.set_anim_paused(true);
    assert!(v.install(pa.clone(), page(1)));
    assert!(v.anim_paused, "同一张图翻页必须保住用户的暂停");

    // 换图 = 播放态归零。这条正是"PDF 停过之后 GIF 不动"的钉子。
    assert!(v.install(pb.clone(), gif(0)));
    assert!(!v.anim_paused, "换到 GIF 必须回到播放态");
}

/// 区域切片（放大后重渲的那一块）**不许**改动播放元信息。
///
/// 用户 2026-11-02 报的现象：PDF 能翻页，放大/拖动几下（走 `iv_render_region`）
/// 再回来，浮条上的播放组就没了。根因是区域回包被当成"一张新的静态图"装档，
/// 页数清零。这里是那条路径的钉子：`region: Some(..)` 的装档一律继承上一份。
#[test]
fn region_slice_keeps_the_play_metadata_of_the_whole_document() {
    use crate::core::pixels::{Anim, Bitmap, Region};
    let (_dir, [pa, _pb, _pc]) = gallery_dir();
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.on_viewport(0.0, 0.0, 2000.0, 2000.0, 1.0);

    let whole = Decoded {
        natural: (100, 100),
        region: None,
        anim: Anim {
            index: 3,
            count: 9,
            delay_ms: Anim::PAGE_DELAY_MS,
            paged: true,
        },
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), whole));

    // 区域重渲：解码器回包不带播放元信息（老实现常见）⇒ 装档处必须兜住。
    let slice = Decoded {
        natural: (100, 100),
        region: Some(Region {
            origin: (10.0, 10.0),
            density: 2.0,
        }),
        anim: Anim::STATIC,
        bitmap: Bitmap {
            width: 2,
            height: 2,
            rgba: vec![0u8; 16],
        },
    };
    assert!(v.install(pa.clone(), slice));
    let a = v.loaded.as_ref().unwrap().anim;
    assert_eq!((a.index, a.count), (3, 9), "区域切片不许把页数抹掉");
    assert!(a.paged, "分页标记必须跟着走");
    assert!(
        v.anim_next_at.is_some(),
        "区域重渲后必须重新排播放闸（否则自动切页停止）"
    );
}

// ── 取色器采样（m217）────────────────────────────────────────────
//
// 两条各钉一半：颜色取自"上屏位图"（所见即所得），坐标取自"原图"（逆掉
// 旋转/镜像/降采样）。两套几何混用是这个功能最容易写错、又最难肉眼发现的地方。

/// 4×4 彩色坐标网格：`R = x*60, G = y*60, B = 100`——取到哪一格能直接读出来。
fn viewer_with_color_grid() -> Viewer {
    use crate::core::decode::{Decoder, Done};
    use crate::core::pixels::{Anim, Bitmap, Decoded};
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    let (w, h) = (4u32, 4u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&[(x * 60) as u8, (y * 60) as u8, 100, 255]);
        }
    }
    assert!(v.apply(Done {
        seq: 0,
        path: std::path::PathBuf::from("grid.png"),
        out: Ok(Decoded {
            natural: (w, h),
            region: None,
            anim: Anim::STATIC,
            bitmap: Bitmap {
                width: w,
                height: h,
                rgba
            },
        }),
    }));
    v
}

#[test]
fn pick_at_reads_the_displayed_pixel_and_reports_native_coords() {
    let mut v = viewer_with_color_grid();
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let content = RectI::new(0, 0, 100, 100);
    // 钉在 1:1 档：fit 档会把 4×4 本地重采样成视口大小（100×100），那时取色器
    // 报的是**屏上插值后**的像素而不是文件里的字节——那是"所见即所得"的正确
    // 口径，但拿它验"原图坐标映射"会被插值噪声糊住。1:1 档位图 == 原图。
    v.zoom_at_center(1.0);
    v.rebuild_image();
    assert_eq!(v.image().unwrap().width, 4, "前提：1:1 档位图与原图同尺寸");
    let d = v.image_dst(content).expect("有图就该有目标框");
    // 第 (1,1) 格的中心：局部归一化 u=v=0.375，落在第 1 格。
    let pos = (
        d.x as f32 + d.w as f32 * 0.375,
        d.y as f32 + d.h as f32 * 0.375,
    );
    let p = v.pick_at(content, pos).expect("图内必须有读数");
    assert_eq!(p.at, (1, 1), "原图坐标必须来自整图 dst 的归一化坐标");
    assert_eq!(p.rgba, [60, 60, 100, 255], "颜色必须是屏上那一枚像素");

    // 图外（左、右下边界外）一律无读数：不许把钳到边缘的颜色冒充光标下的像素。
    assert_eq!(v.pick_at(content, (d.x as f32 - 1.0, pos.1)), None);
    assert_eq!(
        v.pick_at(content, (d.x as f32 + d.w as f32, d.y as f32 + d.h as f32)),
        None,
        "半开区间：右/下边界那条属于图外"
    );
}

#[test]
fn pick_at_inverse_maps_rotation_and_matches_the_pixel_it_shows() {
    let mut v = viewer_with_color_grid();
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    let content = RectI::new(0, 0, 100, 100);
    v.rotate_cw(); // 4×4 小图走同步落地，rot 当场 = 1
    assert_eq!(v.rot, 1);
    let d = v.image_dst(content).expect("旋转后有目标框");

    // 顺时针 90° 的逆映射：显示 (dx,dy) ← 原图 (dy, 1−dx)。
    // 取 dx=dy=0.125（第 0 格中心）⇒ 原图 (0, 3)。
    let pos = (
        d.x as f32 + d.w as f32 * 0.125,
        d.y as f32 + d.h as f32 * 0.125,
    );
    let p = v.pick_at(content, pos).expect("旋转后仍要有读数");
    assert_eq!(p.at, (0, 3), "旋转后的坐标必须逆变换回原图朝向");

    // 强判据：报出来的颜色必须就是报出来的那个原图坐标的颜色。
    // 像素变换（rebuild_image）与坐标逆映射（rect_map_image）只要方向不一致，
    // 这一条必红——那正是"取色器指着红色报蓝色"的现场。
    assert_eq!(
        p.rgba,
        [(p.at.0 * 60) as u8, (p.at.1 * 60) as u8, 100, 255],
        "颜色与坐标必须指向同一枚原图像素"
    );
}

// ── m265/m281：图片预读 = 邻图 ±N（ini 可配） + 缩略图条可见一屏 ──────

/// 造一个有 `n` 张图的目录（文件名补零，清单顺序 == 数组顺序）。
fn gallery_many(tag: &str, n: usize) -> (PathBuf, Vec<PathBuf>) {
    let dir = crate::core::paths::test_scratch_root()
        .join(format!("sv-m265-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<_> = (0..n)
        .map(|i| {
            let p = dir.join(format!("{i:03}.svg"));
            std::fs::File::create(&p).unwrap();
            p
        })
        .collect();
    (dir, paths)
}

fn viewer_for(paths: &[PathBuf]) -> Viewer {
    let (tx, _rx) = std::sync::mpsc::channel::<Done>();
    let mut v = Viewer::new(
        Rc::new(Decoder::spawn_with(tx)),
        Rc::new(RefCell::new(Config::default())),
    );
    v.open(&paths[0]);
    v.on_viewport(0.0, 0.0, 100.0, 100.0, 1.0);
    v
}

/// 判据共用：条上那一屏的**每一格**都得在下注集合里。
fn assert_covers_strip(plan: &[usize], first: usize, n: usize, what: &str) {
    for i in first..first + n {
        assert!(
            plan.contains(&i),
            "{what}：条上第 {} 格（索引 {i}）没被预读：{plan:?}",
            i + 1
        );
    }
}

/// 当前图第 1 张，条滚到 8–12（当前图完全不在视野里）：两种切图手势都得接住。
#[test]
fn picture_prefetch_bets_on_neighbours_and_the_visible_strip() {
    let (dir, paths) = gallery_many("strip-scope", 14);
    let mut v = viewer_for(&paths);
    v.last_dir = 1;
    v.prefetch_strip = Some((7, 5));
    let plan = v.prefetch_targets();
    for i in [1usize, 2, 13] {
        assert!(plan.contains(&i), "方向邻居 {i} 必须在下注集合里：{plan:?}");
    }
    assert_covers_strip(&plan, 7, 5, "图片预读");
    assert!(plan.len() <= PREFETCH_CAP, "下注集合必须有界：{plan:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

/// 负向臂：把「只写邻图」的旧下注集合喂同一个断言，必须报红。
#[test]
#[should_panic(expected = "条上第")]
fn picture_prefetch_rejects_neighbours_only_plan() {
    let only_neighbours = vec![1usize, 2, 13];
    assert_covers_strip(&only_neighbours, 7, 5, "旧「邻图预读」");
}

/// 目录几十张、条一屏几十格 ⇒ 下注集合仍恰好封顶，且留近不留远。
#[test]
fn picture_prefetch_plan_is_bounded_and_keeps_the_nearest() {
    let (dir, paths) = gallery_many("bounded", 40);
    let mut v = viewer_for(&paths);
    v.prefetch_strip = Some((0, 40));
    let plan = v.prefetch_targets();
    assert_eq!(plan.len(), PREFETCH_CAP, "下注集合必须按份数封顶：{plan:?}");
    assert!(plan.contains(&1), "离当前图最近的要先下注：{plan:?}");
    assert!(!plan.contains(&20), "最远的必须被截掉：{plan:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

/// 条画出来的那一屏真的喂进了预读；关条时自动退回纯邻图。
#[test]
fn strip_window_feeds_the_prefetch_scope_and_closing_it_falls_back() {
    let (dir, paths) = gallery_many("feed", 14);
    let mut v = viewer_for(&paths);
    let (first, n) = v.strip_window(5, Some(7));
    assert_eq!((first, n), (7, 5));
    assert_eq!(v.prefetch_strip, Some((first, n)));
    assert_covers_strip(&v.prefetch_targets(), first, n, "条开着");
    v.set_prefetch_strip(0, 0);
    assert_eq!(v.prefetch_strip, None, "关条 = 没有可见的那一屏");
    // 关条后只剩邻图：直接和偏移表比，别把「3..=11 之外」写死——m281 深度可配到 3
    // 之后，±3 本来就会落进那段范围，写死区间是假红而不是判据。
    let expected: Vec<usize> = v
        .prefetch_offsets()
        .iter()
        .map(|&o| o.rem_euclid(paths.len() as isize) as usize)
        .collect();
    assert_eq!(v.prefetch_targets(), expected, "关条后只剩邻图");
    std::fs::remove_dir_all(dir).unwrap();
}

/// m281：深度来自配置——默认 3（对称交错 6 格）、改成 4 是 8 格、改成 1 只剩 2 格。
/// 行为读者就在这里：配置里的值必须真的改变偏移表，不是「解得开但没人读」。
#[test]
fn prefetch_depth_comes_from_config_not_a_hardcoded_constant() {
    let (dir, paths) = gallery_many("depth", 30);
    let mut v = viewer_for(&paths);
    assert_eq!(
        v.prefetch_offsets(),
        vec![1, -1, 2, -2, 3, -3],
        "出厂默认 3 ⇒ 未定向时对称交错 6 格"
    );
    v.config.borrow_mut().prefetch_depth = 4;
    assert_eq!(
        v.prefetch_offsets(),
        vec![1, -1, 2, -2, 3, -3, 4, -4],
        "ini 改成 4 当场生效"
    );
    v.last_dir = 1;
    assert_eq!(
        v.prefetch_offsets(),
        vec![1, 2, 3, 4, -1],
        "方向已定：同方向给满 4 格、反方向留 1 格"
    );
    v.config.borrow_mut().prefetch_depth = 1;
    assert_eq!(
        v.prefetch_offsets(),
        vec![1, -1],
        "ini 改成 1 只剩最少的一格"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// m281：深度拉到上限 4 时，常规可见一屏（7 格）仍**全部**在下注集合里。
/// CAP=16 的算式 `2×4 + 7 + 1` 就是为这一条存在的。
#[test]
fn max_depth_never_crowds_out_the_visible_strip() {
    let (dir, paths) = gallery_many("cap", 30);
    let mut v = viewer_for(&paths);
    v.config.borrow_mut().prefetch_depth = 4;
    v.prefetch_strip = Some((10, 7));
    let plan = v.prefetch_targets();
    assert_eq!(plan.len(), 15, "8 格邻图 + 7 格可见屏，去重后 15：{plan:?}");
    assert_covers_strip(&plan, 10, 7, "深度 4");
    assert!(plan.len() <= PREFETCH_CAP, "16 的硬闸要留得住这一屏");
    std::fs::remove_dir_all(dir).unwrap();
}

/// 负向臂：拿生产下注集合按**旧 CAP=12** 截断，喂同一个「每格都得在」判据，必须红。
/// 这正是 m281 把 CAP 提到 16 的理由——不提，深度调大就会挤掉可点的格子。
#[test]
#[should_panic(expected = "条上第")]
fn old_cap_of_twelve_crowds_out_visible_cells_at_max_depth() {
    let (dir, paths) = gallery_many("old-cap", 30);
    let mut v = viewer_for(&paths);
    v.config.borrow_mut().prefetch_depth = 4;
    v.prefetch_strip = Some((10, 7));
    let mut old = v.prefetch_targets();
    old.truncate(12);
    assert_covers_strip(&old, 10, 7, "旧 CAP=12");
    std::fs::remove_dir_all(dir).unwrap();
}

/// 滚条 = 新一轮下注：旧一屏的位图不许占着份数闸，停填闸也要松开。
#[test]
fn moving_the_strip_replans_and_drops_stale_cached_pages() {
    let (dir, paths) = gallery_many("replan", 20);
    let mut v = viewer_for(&paths);
    v.prefetch_strip = Some((10, 5));
    v.prefetched.push(Prefetched {
        path: paths[4].clone(),
        img: decoded4(paths[4].clone()).out.unwrap(),
    });
    v.prefetched.push(Prefetched {
        path: paths[6].clone(),
        img: decoded4(paths[6].clone()).out.unwrap(),
    });
    v.prefetch_stopped = true;
    v.set_prefetch_strip(5, 5);
    assert_eq!(v.prefetch_strip, Some((5, 5)));
    assert!(!v.prefetch_stopped, "滚动条之后必须重新开闸");
    assert!(
        v.prefetched.iter().all(|e| e.path != paths[4]),
        "上一屏的旧位图不该留着占份数闸"
    );
    assert!(
        v.prefetched.iter().any(|e| e.path == paths[6]),
        "新一屏里已解好的要留着"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// m304：压缩包层里有跳过项时状态格必须说出来；为 0 时不许多一个字，
/// 基础信息也不许被这句替换掉。
#[test]
fn status_reports_skipped_archive_entries_without_replacing_basics() {
    let mut v = viewer_with_image();
    v.archive_skipped = 0;
    let plain = v.status();
    assert!(!plain.contains("不支持"), "没有跳过项时不许多一段文案");
    v.archive_skipped = 3;
    let s = v.status();
    assert!(s.contains("另有 3 项不支持"), "{s}");
    // 负向臂：基础串必须原样保留在前面（后缀只许追加，不许顶替）。
    assert!(s.starts_with(&plain), "{s}");
}

#[test]
fn empty_layer_hint_points_at_the_tree_not_the_removed_menu() {
    // ★ m312：旧文案写「右键「本包」进入子层」，可那个二级菜单 m300 就撤了、换成
    //   左侧文件树——用户看到的是一句**根本走不通的指令**（实报「居然还存在…这种
    //   旧提示」）。这里既钉文案，也钉"只有真有子层时才指路"。
    let dir = std::env::temp_dir().join(format!("rupix_m312_empty_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut v = viewer_with_image();

    // 有子层的空层：指路。
    std::fs::write(dir.join("subs.tsv"), "#images 0\n#skipped 0\nd\tsub\tch1\n").unwrap();
    let layer = crate::core::archive::Layer {
        dir: dir.clone(),
        images: 0,
        from_archive: true,
        skipped: 0,
    };
    v.open_layer(&layer);
    let msg = v
        .error
        .as_ref()
        .map(|f| f.detail.clone())
        .unwrap_or_default();
    assert!(msg.contains("左侧文件树"), "空层提示没指路：{msg}");
    // 负向臂：撤掉的入口名一个字都不许回来。
    assert!(
        !msg.contains("本包"),
        "又指回 m300 撤掉的「本包 ▸」了：{msg}"
    );
    assert!(!msg.contains("右键"), "空层还让人右键找入口：{msg}");

    // 负向臂二：**没有**子层时只报事实。给一句走不通的指令比不给更糟。
    std::fs::write(dir.join("subs.tsv"), "#images 0\n#skipped 0\n").unwrap();
    v.open_layer(&layer);
    assert_eq!(
        v.error.as_ref().map(|f| f.detail.as_str()),
        Some("这一层没有图片")
    );

    std::fs::remove_dir_all(&dir).ok();
}
