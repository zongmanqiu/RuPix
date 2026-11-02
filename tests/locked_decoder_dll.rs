//! 真机那一臂：解码器 DLL **正被宿主加载着**（内存映射在册）时，热更新的三种落点各自怎样。
//!
//! 背景（待办 §21.3）：`src/decoders/tests.rs` 的三条 audit 用例用**缓存桩**——手工往
//! `host.loaded` 塞 `Loaded { _lib: None, .. }`——钉住了「同 id 淘汰 = 只置停用、条目留在
//! 表里、旧路径不再进候选」这张表的逻辑。桩里没有真 `Library`，所以 `dllmeta.rs` 里那句
//! 「真正的卸库留到进程结束」在真文件上成不成立，
//! 桩看不见：**被淘汰的那份到底有没有被 `FreeLibrary`**。本文件用真 DLL 补这一臂。
//!
//! 判据（四条，都在同一个进程里互相对照）：
//! 1. 被加载的解码器 DLL **删不掉**（共享冲突 / 拒绝访问）。这条"失败"本身就是映射还在的物证。
//! 2. 同 id 的两份拷贝都进了表：赢的那份在用，**输的那份也仍删不掉** ⇒ 宿主没在会话中途卸库。
//! 3. 对照：同一目录里一枚从没加载成功的坏文件**删得掉** ⇒ 上面两条的"删不掉"来自映射，不是介质。
//! 4. 每一步之后真实解码照常成功 ⇒ 一次失败的删除、一枚坏 DLL 都不许把格式打成"无人认领"。
//!
//! 跑法：随门禁自动跑（`cargo test --release -- --test-threads=1`）。需要发布目录里有真
//! DLL（流水线顺序 `3_build_ext` → `5_test` 保证），或用 `RUPIX_DECODERS_DIR` 指到别处。
//! 夹具目录名固定、**每次开跑先清**：上一轮被锁住的 DLL 在上一进程退出后就可删，
//! 清它的是下一轮的自己，不留 litter（同一口径见 `install.rs` 的 `.old` 前置清理）。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rupix_lib::core::decode;
use rupix_lib::decoders;

/// 本文件的用例都动**进程级全局态**（`RUPIX_DECODERS_DIR`、后端单例的目录表、停用/首选表），
/// 并行跑会互相踩——同一口径见 `config_pipeline_e2e.rs` 的 `SERIAL`。
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// 三种「正被人用着」：`ERROR_SHARING_VIOLATION` / `ERROR_ACCESS_DENIED` / `ERROR_BUSY`。
/// 都算：判据要钉的是"删不掉"，而不是系统用哪一种说法拒绝。
const LOCK_CODES: [i32; 3] = [32, 5, 170];

/// 删一次，返回 `None` = 删掉了，`Some(code)` = 没删掉及它的 Windows 错误码。
fn try_delete(p: &Path) -> Option<i32> {
    match std::fs::remove_file(p) {
        Ok(()) => None,
        Err(e) => Some(e.raw_os_error().unwrap_or(-1)),
    }
}

/// 断言"这一个正被加载着，删不掉"，并把真实错误码留在读数里。
fn assert_locked(p: &Path, what: &str) {
    let code = try_delete(p);
    eprintln!("[dlllock] {what} {} 删除结果 = {code:?}", p.display());
    assert!(
        code.is_some_and(|c| LOCK_CODES.contains(&c)),
        "{what} 已被宿主加载，磁盘上应当删不掉（真机 21.3 那一臂）：{} → {code:?}",
        p.display()
    );
    assert!(p.exists(), "{what} 删不掉就得还在盘上，不能凭空消失");
}

/// 源目录：优先 `RUPIX_DECODERS_DIR`（绝对且存在），否则回退仓旁 `.build/decoders`。
fn source_dir() -> PathBuf {
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

/// 挑一枚**能解我们自造样本**的真解码器：后缀在 `png` / `ppm` 里挑（这两种我们能就地造），
/// 同可用的里面挑最小的（本用例只借它占住映射，越小拷得越快）。
/// 不认任何一枚具体的 DLL——发布档换名、增减后缀都不用改这里（同一口径见 K158）。
fn pick_claimant() -> (&'static str, PathBuf) {
    let src = source_dir();
    assert!(
        src.is_dir(),
        "解码器目录不存在：{}——先跑 build/3_build_ext.bat（流水线顺序保证，或设 RUPIX_DECODERS_DIR）",
        src.display()
    );
    std::env::set_var("RUPIX_DECODERS_DIR", src);
    let found = decoders::list_decoders();
    for ext in ["png", "ppm"] {
        let mut hit: Vec<&rupix_lib::decoders::DecoderEntry> = found
            .iter()
            .filter(|d| d.extensions.iter().any(|e| e == ext))
            .collect();
        hit.sort_by_key(|d| d.size);
        if let Some(d) = hit.first() {
            return (ext, d.path.clone());
        }
    }
    panic!(
        "发布目录 {found:?} 里没有任何一枚认领 png/ppm 的解码器——\
         这是解码器集合的问题，不是宿主的接线问题"
    );
}

/// 一条用例的夹具根目录：`<仓>/.temp/rupix-dlllock-<name>/`，其 `decoders/` 子目录当解码器目录。
/// **先清再建**：上一进程留下的、当时删不掉的 DLL，这一轮已经可删了。
fn fixture(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".temp")
        .join(format!("rupix-dlllock-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("decoders");
    std::fs::create_dir_all(&dir).expect("建夹具解码器目录失败");
    dir
}

/// 把解码器目录指到夹具并让下一次解码重扫；同时把策略全局态清干净（同 `setup()`）。
fn point_at(dir: &Path) {
    rupix_lib::install_decode_stack();
    std::env::set_var("RUPIX_DECODERS_DIR", dir);
    decoders::request_rescan();
    decoders::set_safe_mode(false);
    decoders::set_disabled(Vec::<String>::new());
    decoders::set_preferred(std::collections::BTreeMap::new());
}

/// 在 `parent` 下造一张该后缀的小图，返回路径。
fn write_sample(parent: &Path, ext: &str) -> PathBuf {
    let path = parent.join(format!("rupix-dlllock-sample.{ext}"));
    match ext {
        "ppm" => {
            let mut buf = b"P6\n8 8\n255\n".to_vec();
            for y in 0..8u32 {
                for x in 0..8u32 {
                    buf.push((x * 255 / 7) as u8);
                    buf.push((y * 255 / 7) as u8);
                    buf.push(128);
                }
            }
            std::fs::write(&path, buf).expect("写 PPM 样本失败");
        }
        _ => {
            let (w, h) = (12u32, 9u32);
            let f = std::fs::File::create(&path).expect("建 PNG 失败");
            let mut enc = png::Encoder::new(f, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("PNG 头");
            let mut px: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h {
                for x in 0..w {
                    px.push((x * 255 / (w - 1)) as u8);
                    px.push((y * 255 / (h - 1)) as u8);
                    px.push(64);
                    px.push(255);
                }
            }
            writer.write_image_data(&px).expect("写 PNG 数据失败");
        }
    }
    path
}

/// 解一次并确认**真出了图**（不是只没报错）。
fn decode_ok(sample: &Path, what: &str) {
    let d = decode::probe_decode(sample, 0, 0).unwrap_or_else(|e| {
        panic!("{what}：应当照常解码成功，实际失败：{e}");
    });
    assert!(
        d.bitmap.width > 0 && d.bitmap.height > 0,
        "{what}：要真有像素"
    );
}

/// 判据 1 + 3 + 4：加载中的 DLL 删不掉、同目录里没加载成功的坏文件删得掉，中间解码照常。
#[test]
fn loaded_decoder_dll_stays_locked_while_a_never_loaded_one_is_deletable() {
    let _serial = serial();
    let (ext, src_dll) = pick_claimant();
    let dir = fixture("locked");
    let dll = dir.join("locked.dll");
    std::fs::copy(&src_dll, &dll).expect("拷真 DLL 进夹具失败");
    let broken = dir.join("broken.dll");
    std::fs::write(&broken, b"MZ\x00\x00 not-a-decoder-bytes").expect("写坏 DLL 失败");
    let sample = write_sample(dir.parent().unwrap(), ext);

    point_at(&dir);
    decode_ok(&sample, "首次解码");

    // ★ 物证：宿主正拿着这枚 DLL 的映射。
    assert_locked(&dll, "已加载的解码器");
    // 对照：同一目录、同一进程，从没加载成功过的那一枚**删得掉**。
    // 没有这一格，上面那条"删不掉"可能只是介质只读或夹具目录有鬼。
    let broken_code = try_delete(&broken);
    eprintln!("[dlllock] 没加载成功的坏文件删除结果 = {broken_code:?}");
    assert_eq!(broken_code, None, "对照物必须删得掉，否则上面的判据失明");
    assert!(!broken.exists());

    // 一次失败的删除不许污染清单：`failed` 表只记加载失败，不该记删除失败。
    decode_ok(&sample, "删不动之后重解");

    // 读数臂（不改判据）：热更新靠的是"把旧 DLL 挪开"而不是"删掉"。
    // 正被映射的镜像文件能否改名，Windows 与多数安装器的答案是能——但这是我们
    // 抄来的先验，不是这台的读数，所以只量不钉（`install.rs` 两条分支都写全了）。
    let moved = dir.join("moved-aside.dll.old");
    match std::fs::rename(&dll, &moved) {
        Ok(()) => {
            eprintln!("[dlllock] 改名正被加载的 DLL = 成功（挪位这条路在 Windows 上可行）");
            // 改名之后映射跟着文件对象走：这条"删不掉"证的不是名字，是句柄。
            assert_locked(&moved, "已挪位的解码器");
            std::fs::rename(&moved, &dll).expect("把夹具改回来失败");
        }
        Err(e) => eprintln!(
            "[dlllock] 改名正被加载的 DLL = 失败（code={:?}）——正是 install.rs 那句「重启后再装」",
            e.raw_os_error()
        ),
    }
    let _ = std::fs::remove_file(&sample);
}

/// 判据 2：同 id 两份拷贝，赢的那份在用，**输的那份也没被卸库**。
///
/// 这一条是 21.3 的正身：桩用例只看得见 `active=false`，看不见 `_lib` 还在不在。
/// 谁哪天把"淘汰"改成"摘条目"（= 会话中途 `FreeLibrary`，正是 `Loaded` 注释里禁止的那件事），
/// 输的那份立刻变得可删，本用例当场翻红。
#[test]
fn losing_duplicate_of_the_same_decoder_id_is_not_unloaded() {
    let _serial = serial();
    let (ext, src_dll) = pick_claimant();
    let dir = fixture("dup");
    // 两份**同一枚** DLL 的不同路径：id 与自述版本必然相同 ⇒ 版本仲裁只能按路径定一份为赢家。
    let first = dir.join("aaa_dup.dll");
    let second = dir.join("bbb_dup.dll");
    std::fs::copy(&src_dll, &first).expect("拷第一份失败");
    std::fs::copy(&src_dll, &second).expect("拷第二份失败");
    let sample = write_sample(dir.parent().unwrap(), ext);

    point_at(&dir);
    decode_ok(&sample, "同 id 两份并存时");

    assert_locked(&first, "同 id 拷贝（赢家）");
    assert_locked(&second, "同 id 拷贝（被淘汰的那份）");

    // 再扫一轮：淘汰关系不变，路由仍必须给出这一枚。
    decoders::request_rescan();
    decode_ok(&sample, "重扫之后");

    // 夹带一条真机口径：目录里同 id 有两份时，**列表**按文件给行（磁盘真相），
    // 与"只有一份进候选"（选择真相）不冲突。这里只钉行数是 2，防止后来人把
    // 两者混成同一个数——那条 UI 判据的正本在 `ui::settings` 的解码器页用例里。
    let rows = decoders::list_decoders();
    assert_eq!(
        rows.iter()
            .filter(|r| r.path.parent() == Some(dir.as_path()))
            .count(),
        2,
        "清单读的是磁盘，不是候选表"
    );
    let _ = std::fs::remove_file(&sample);
}

/// 判据 4 的反向臂：文件名版本号更高、但根本加载不起来的那一枚，不许挤掉正在用的旧版。
#[test]
fn corrupt_higher_version_file_does_not_disable_the_locked_decoder() {
    let _serial = serial();
    let (ext, src_dll) = pick_claimant();
    let dir = fixture("corrupt");
    let old = dir.join("aaa_0.1.0.dll");
    std::fs::copy(&src_dll, &old).expect("拷真 DLL 失败");
    // 名字里的版本号更高（0.9.0 > 0.1.0），内容却是垃圾：`inspect()` 只会得到加载失败。
    let bogus = dir.join("zzz_0.9.0.dll");
    std::fs::write(&bogus, b"MZ\x00\x00 not-a-decoder-bytes").expect("写坏 DLL 失败");
    let sample = write_sample(dir.parent().unwrap(), ext);

    point_at(&dir);
    decode_ok(&sample, "目录里混进一枚坏新版时");

    // 坏的删得掉（从没被加载），旧的删不掉（在用）——一对照就把"是谁在挡"说清楚了。
    assert_eq!(try_delete(&bogus), None, "坏文件没被加载，应当删得掉");
    assert_locked(&old, "仍在用的旧版");

    let _ = std::fs::remove_file(&sample);
}
