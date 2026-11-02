//! 配置 → 解码管线端到端：真实 Config 键、真实解码器 DLL、真实 FFI，不经 UI。
//!
//! 锁住 `runtime::run` 那条接线（`runtime/mod.rs`：`config.decoders_disabled` →
//! `crate::decoders::set_disabled`、`config.decoders_preferred` → `crate::decoders::set_preferred`）
//! 会**真实改变解码结果**——不是只在设置页里存着。另覆盖 `--safe-mode` 进程门
//! （`main.rs` → `set_safe_mode`）。
//!
//! 解码器目录解析与运行时同源：`RUPIX_DECODERS_DIR`（绝对路径）优先，
//! 否则回退 `<repo>/.build/decoders`（流水线顺序 3_build_ext → 5_test 保证其存在）。
//! 样本不依赖 `.temp/samples`：测试自己造 8×8 P6 PPM（由发布档里认领 `.ppm` 的解码器解，无其他解码器按魔数抢注）。
//!
//! 「首选解码器真实命中」的**解码器侧**正向路径不在本文件：第一方有「后缀认领唯一」门禁
//! （`ext_claims_are_unique`），仓内不会出现两个解码器争一个后缀，那条判定由
//! `src/decoders/mod.rs` 的 `preferred_candidate_overrides_auto_and_falls_back_on_stale`
//! 单测覆盖（原先靠 `test-decoders/testred` 夹具 DLL 的端到端已于 2026-10-16 随夹具一起移除）。
//! **内置解码库**那条点名路径已于 2026-10-25 随 `platform::wic` 整层删除，改由
//! `bitmaps_never_route_to_a_builtin_decoder` 锁住它的反面：位图**永不**走内置路。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rupix_lib::core::config::Config;
use rupix_lib::core::decode;
use rupix_lib::decoders;

/// 本文件的测试都操作**进程级全局态**（解码 `Host` 单例目录表、`SAFE_MODE`
/// 原子、`DISABLED` 集合，以及 `RUPIX_DECODERS_DIR` 环境变量）。cargo 默认并行跑
/// 测试会把这些全局态互相踩坏——典型症状：一个测试刚 `set_safe_mode(false)`，
/// 并行邻居 `set_safe_mode(true)` 就让它接下来那次解码撞上 safe-mode 提前返回、
/// 目录表没加载，从而报「ppm 无人认领」。
///
/// 因此这里用一把进程级互斥锁把它们**强制串行**，不再依赖外部
/// `--test-threads=1`（那只是 DOCS_MANIFEST 里记录的推荐跑法，跑 `cargo test
/// --release` 不带它也曾因此翻车）。
static SERIAL: Mutex<()> = Mutex::new(());

/// 每个测试的入口锁：拿住它整个测试期间都不释放，保证它们互斥执行。
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// 每个测试的公共前置：定位解码器目录、装解码栈、清干净进程策略态。
fn setup() -> PathBuf {
    let dir = decoders_dir();
    assert!(
        dir.is_dir(),
        "解码器目录不存在：{}——先跑 build/3_build_ext.bat（流水线顺序保证，或设 RUPIX_DECODERS_DIR）",
        dir.display()
    );
    // edition 2021：set_var 仍是安全函数；本仓门禁 --test-threads=1，无并发竞态。
    std::env::set_var("RUPIX_DECODERS_DIR", &dir);
    rupix_lib::install_decode_stack();
    // 强制目录表在下一次解码时重扫：前一个测试可能改过 safe-mode/停用集，
    // 保证本测试从干净态起步（目录表是进程级单例，不会因为新测试而自动复位）。
    crate::decoders::request_rescan();
    crate::decoders::set_safe_mode(false);
    crate::decoders::set_disabled(Vec::<String>::new());
    crate::decoders::set_preferred(std::collections::BTreeMap::new());
    dir
}

fn decoders_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("RUPIX_DECODERS_DIR") {
        let pb = PathBuf::from(p);
        if pb.is_absolute() && pb.is_dir() {
            return pb;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".build")
        .join("decoders")
}

/// 自造 8×8 渐变 P6 PPM（RGB，每像素 3 字节），返回路径；调用方负责删。
fn write_ppm(dir: &Path) -> PathBuf {
    let path = dir.join("rupix-e2e.ppm");
    let mut buf = b"P6\n8 8\n255\n".to_vec();
    for y in 0..8u32 {
        for x in 0..8u32 {
            buf.push((x * 255 / 7) as u8);
            buf.push((y * 255 / 7) as u8);
            buf.push(128);
        }
    }
    std::fs::write(&path, &buf).expect("写 PPM 样本失败");
    path
}

fn temp_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.temp")
        .join(format!("rupix-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("建临时目录失败");
    d
}

/// 自造 12×9 RGBA PNG，返回路径；调用方负责删。
///
/// 为什么用 PNG 做这条对账：PNG **没有 EXIF 方向标签、我们也不给它塞 ICC**，
/// 所以"解码器路"与"点名内置"那一路之间原本没有任何可以合理分歧的自由度——
/// 输出应当**逐字节相同**。有分歧就说明其中一条错了（红蓝对调那类，见 m53 收尾）。
/// 今天内置路已经没有位图实现，这条对账判的是"点名内置也不许把位图带走"。
fn write_png(dir: &Path) -> PathBuf {
    let path = dir.join("rupix-e2e.png");
    let (w, h) = (12u32, 9u32);
    let mut px: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            px.push((x * 255 / (w - 1)) as u8);
            px.push((y * 255 / (h - 1)) as u8);
            px.push(64);
            px.push(if (x + y) % 3 == 0 { 120 } else { 255 });
        }
    }
    let f = std::fs::File::create(&path).expect("建 PNG 失败");
    let mut enc = png::Encoder::new(f, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("PNG 头");
    writer.write_image_data(&px).expect("PNG 像素");
    drop(writer);
    path
}

/// 位图**永不**走内置解码库（2026-10-25 用户裁决，`platform::wic` 已整层删除）。
///
/// 锁的是"点名内置"那条分支真的不在了：把 `decoders_preferred` 指向一枚内置库 id，
/// 位图仍必须走解码器路（逐字节相同），不许被送去某个内置实现、也不许因此报错。
/// 这是旧用例 `named_builtin_decoder_really_takes_the_decode`（m57，`png → wic`）的
/// **反面**——那条锁的是"点名内置真的改了路径"，而那正是今天要消灭的行为：
/// 便携看图器不调系统解码器，同一张图在两台机器上必须解出同一个结果（宪章 §5.1）。
#[test]
fn bitmaps_never_route_to_a_builtin_decoder() {
    let _serial = serial();
    let _dir = setup();
    let tmp = temp_dir();
    let png = write_png(&tmp);

    // 臂①：不设首选 → 走解码器（png 认领者）。
    let via_decoder = decode::probe_decode(&png, 0, 0).expect("解码器路应解得开");
    // 臂②：点名一枚内置库（今天只有 resvg）→ **仍走解码器**，像素必须一致。
    let mut pref = std::collections::BTreeMap::new();
    pref.insert("png".to_string(), "resvg".to_string());
    crate::decoders::set_preferred(pref);
    let named = decode::probe_decode(&png, 0, 0).expect("点名内置后位图仍应经解码器解开");
    crate::decoders::set_preferred(std::collections::BTreeMap::new());

    assert_eq!(
        (named.bitmap.width, named.bitmap.height),
        (12, 9),
        "位图尺寸不受点名影响"
    );
    assert_eq!(
        via_decoder.bitmap.rgba, named.bitmap.rgba,
        "点名内置不许把位图带去另一条路：同一张 PNG 必须逐字节相同"
    );

    // ★ 第二条判据是登记表面不是路由面：**内置表里不许再出现解位图的条目**。
    // 矢量（resvg）是唯一例外——它要按视口重渲，位图解码器的语义表达不了。
    for b in rupix_lib::core::formats::BUILTIN_DECODERS {
        assert!(
            b.is_vector,
            "内置解码库「{}」是位图库：位图一律交给解码器（WIC 已于 2026-10-25 删除）",
            b.id
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn default_config_decodes_through_real_decoders() {
    let _serial = serial();
    let _dir = setup();
    let tmp = temp_dir();
    let ppm = write_ppm(&tmp);

    let d = decode::probe_decode(&ppm, 0, 0).expect("默认配置应能经解码器管线解码");
    assert_eq!(d.natural, (8, 8), "natural 尺寸");
    assert_eq!((d.bitmap.width, d.bitmap.height), (8, 8), "位图尺寸");
    assert_eq!(d.bitmap.rgba.len(), 8 * 8 * 4, "RGBA 长度自洽");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// 夹具格式按**运行时自述**挑：返回 (样本路径, 该后缀的认领者 id)。
///
/// 为什么不钉死 `.ppm`：发布档的后缀集合会变（2026-10-29 实测 common 0.2.3 → 0.2.4
/// 就把 `ppm` 摘走了，31 个后缀缩成 24），钉死一种等于让解码器换档弄红宿主的用例。
/// 本文件下面几条要的都是"一个今天真的有人认领、而我们又能自己造出字节"的格式，
/// 这两样都从运行时问，不写进代码里。
fn claimant_fixture(tmp: &Path) -> (PathBuf, &'static str, String) {
    let claimants = crate::decoders::list_decoders();
    for ext in ["png", "ppm"] {
        if let Some(d) = claimants
            .iter()
            .find(|d| d.extensions.iter().any(|e| e == ext))
        {
            let file = if ext == "png" {
                write_png(tmp)
            } else {
                write_ppm(tmp)
            };
            return (file, ext, d.id.clone());
        }
    }
    panic!(
        "发布目录里 {claimants:?}没有任何一条我们能自造样本的后缀（png / ppm）——\
         这是解码器集合的问题，不是宿主的接线问题"
    );
}

#[test]
fn decoders_disabled_key_gates_decode_end_to_end() {
    let _serial = serial();
    let _dir = setup();
    let tmp = temp_dir();
    let (sample, ext, claimant) = claimant_fixture(&tmp);

    // 与 runtime::run 完全相同的接线：ini 键 → Config → set_disabled。
    // id 与后缀都来自上面的运行时自述，本用例不认任何一枚具体的 DLL。
    let ini = tmp.join("disable-claimant.ini");
    std::fs::write(&ini, format!("decoders_disabled={claimant}\n")).expect("写 ini 失败");
    let cfg = Config::load_from(&ini);
    assert_eq!(cfg.decoders_disabled, vec![claimant.clone()], "ini 解析");

    crate::decoders::set_disabled(cfg.decoders_disabled.clone());
    let blocked = decode::probe_decode(&sample, 0, 0);
    assert!(
        blocked.is_err(),
        "停用唯一认领者 {claimant} 后 .{ext} 应无人认领而失败"
    );
    let msg = blocked.unwrap_err();
    eprintln!("停用 {claimant} 后的预期报错：{msg}");

    // 复原后必须恢复——证明这是活开关，不是一次性副作用。
    crate::decoders::set_disabled(Vec::<String>::new());
    let back = decode::probe_decode(&sample, 0, 0).expect("复原 decoders_disabled 后应恢复解码");
    assert!(
        back.bitmap.width > 0 && back.bitmap.height > 0,
        "复原后要真出图"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn decoders_preferred_key_feeds_decode_end_to_end() {
    let _serial = serial();
    let _dir = setup();
    let tmp = temp_dir();
    let ppm = write_ppm(&tmp);

    // 与 runtime::run 完全相同的接线：ini 键 → Config → set_preferred。
    let ini = tmp.join("preferred.ini");
    std::fs::write(&ini, "decoders_preferred=ppm=ghost-id\n").expect("写 ini 失败");
    let cfg = Config::load_from(&ini);
    assert_eq!(
        cfg.decoders_preferred.get("ppm").map(String::as_str),
        Some("ghost-id"),
        "ini 解析"
    );
    crate::decoders::set_preferred(cfg.decoders_preferred.clone());

    // 首选指到不存在的解码器 → 约定是静默回退自动仲裁，解码必须照常成功。
    // （「首选真实生效」的路径由 decoders/mod.rs 的 preferred_candidate 单测覆盖：
    //   第一方有唯一认领门禁，本仓不会出现两个解码器争一个后缀的现场。）
    let d = decode::probe_decode(&ppm, 0, 0).expect("失效首选应回退自动而非让解码失败");
    assert_eq!((d.bitmap.width, d.bitmap.height), (8, 8));

    // 清掉脏首选后必须照常——证明全局表真的被解码路径读着，不是一次性副作用。
    crate::decoders::set_preferred(std::collections::BTreeMap::new());
    let back = decode::probe_decode(&ppm, 0, 0).expect("复原 decoders_preferred 后应照常");
    assert_eq!(back.natural, (8, 8));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn safe_mode_gate_blocks_and_restores_decode() {
    let _serial = serial();
    let _dir = setup();
    let tmp = temp_dir();
    let ppm = write_ppm(&tmp);

    crate::decoders::set_safe_mode(true);
    let blocked = decode::probe_decode(&ppm, 0, 0);
    assert!(blocked.is_err(), "safe-mode 下解码应被进程门拦下");

    crate::decoders::set_safe_mode(false);
    let back = decode::probe_decode(&ppm, 0, 0).expect("退出 safe-mode 后应恢复");
    assert_eq!(back.natural, (8, 8));

    let _ = std::fs::remove_dir_all(&tmp);
}
