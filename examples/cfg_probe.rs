//! 便携落点探针：**不开窗**，验证"持久化只发生在 exe 同级目录"。
//!
//! 用法：把编译出的 `cfg_probe.exe` 拷到任意目录（模拟 U 盘 / 便携文件夹）再运行。
//! 它会写一份配置并读回来，然后断言文件确实躺在 exe 旁边。
//!
//! ```text
//! cargo build --release --example cfg_probe
//! copy target\release\examples\cfg_probe.exe <某目录>\ & <某目录>\cfg_probe.exe
//! ```
//!
//! 除了配置，它顺手验一遍**外挂解码器目录**（`crate::decoders::catalog`）：在 exe 旁边临时
//! 造一个合规命名的 DLL + 一个错命名的 DLL，看是不是按方案 v2 §12.1 归类，
//! 验完把自己造的东西删干净。
//!
//! exe 旁已存在真实 `rupix.ini` 时：先把配置改名挪开，跑完原样还回去——探针
//! 绝不覆盖、更不删用户配置（静默丢便携配置是最不能接受的失败模式）。

use std::path::PathBuf;

use rupix_lib::core::config::{Config, DblClick, WheelAction};
use rupix_lib::core::paths;
use rupix_lib::core::util::fmt_bytes;
use rupix_lib::decoders::catalog;

fn main() {
    let exe = std::env::current_exe().unwrap_or_default();
    println!("exe        : {}", exe.display());
    println!("config     : {:?}", paths::config_path().map(fmt));
    println!(
        "crash log  : {:?}",
        paths::crash_log_path(&rupix_lib::platform::panic_guard::today()).map(fmt)
    );

    let Some(cfg_path) = paths::config_path() else {
        println!("FAIL: 取不到 exe 目录");
        std::process::exit(1);
    };

    // 判据 1：路径必须就在 exe 同级目录。
    let mut ok = cfg_path.parent() == exe.parent();
    println!("[1] 同级目录 : {}", if ok { "PASS" } else { "FAIL" });

    // 判据 2：真的落盘了。
    // exe 旁已存在真实 rupix.ini 时先改名挪开，跑完原样还回去（见文件头）。
    let pre_existing = cfg_path.exists();
    let bak = {
        let mut s = cfg_path.as_os_str().to_os_string();
        s.push(".probe-bak");
        PathBuf::from(s)
    };
    if pre_existing {
        if let Err(e) = std::fs::rename(&cfg_path, &bak) {
            println!("FAIL: exe 旁已有配置且挪不开，拒绝跑写盘探针：{e}");
            std::process::exit(1);
        }
    }
    let c = Config {
        stay_on_top: true,
        dbl_click: DblClick::Fullscreen,
        wheel: WheelAction::Navigate,
        opacity: 30,
        ..Config::default()
    };
    c.save();
    let written = cfg_path.exists();
    println!("[2] 写盘存在 : {}", if written { "PASS" } else { "FAIL" });
    ok &= written;

    // 判据 3：读回来逐项一致。
    let back = Config::load();
    let same = back.stay_on_top
        && back.dbl_click == DblClick::Fullscreen
        && back.wheel == WheelAction::Navigate
        && back.opacity == 30;
    println!("[3] 往返一致 : {}", if same { "PASS" } else { "FAIL" });
    ok &= same;

    if let Ok(text) = std::fs::read_to_string(&cfg_path) {
        println!("--- {} ---\n{}", cfg_path.display(), text.trim_end());
    }
    // 探针写的文件：本来没有就清掉；原有配置把备份原样还回去。
    if pre_existing {
        let _ = std::fs::remove_file(&cfg_path);
        if let Err(e) = std::fs::rename(&bak, &cfg_path) {
            println!(
                "FAIL: 恢复原配置失败（{e}），你的配置备份仍在 {}",
                bak.display()
            );
            std::process::exit(1);
        }
    } else {
        let _ = std::fs::remove_file(&cfg_path);
    }

    let ext_ok = probe_decoders();

    println!(
        "\n{}",
        if ok && ext_ok {
            "ALL PASS：持久化仅落在 exe 同级目录，解码器目录同样按便携规则工作".to_string()
        } else {
            "FAILED".to_string()
        }
    );
    std::process::exit(if ok && ext_ok { 0 } else { 1 });
}

/// 外挂解码器目录：真在 exe 旁边建一次、放两个文件、扫一遍，最后清干净。
fn probe_decoders() -> bool {
    println!("\n=== 外挂解码器目录 ===");
    let Some(dir) = catalog::dir() else {
        println!("FAIL: 取不到解码器目录");
        return false;
    };
    println!(" 目录      : {}", dir.display());

    let mut ok = dir.parent()
        == std::env::current_exe()
            .ok()
            .as_deref()
            .and_then(|e| e.parent());
    println!(" [1] exe 同级 : {}", if ok { "PASS" } else { "FAIL" });

    // 建目录 → 放一个（假的）DLL → 扫一遍：应被当成一个解码器条目列出（健康度=加载失败）。
    let pre_dir = dir.exists();
    let Ok(dir) = catalog::ensure_dir() else {
        println!("FAIL: 建不出目录");
        return false;
    };
    let fake = dir.join("probe-fake.dll");
    let _ = std::fs::write(&fake, vec![0u8; 2048]);
    let s = catalog::scan();
    println!(" [2] 状态      : {}", catalog::summary(&s));
    for e in &s.decoders {
        println!(
            "     {:<22} {:>8}  {}",
            e.file,
            fmt_bytes(e.size),
            e.health.label()
        );
    }
    let listed = s.decoders.iter().any(|e| e.file == "probe-fake.dll");
    println!(" [3] 列出 DLL  : {}", if listed { "PASS" } else { "FAIL" });
    ok &= listed;

    println!(
        " 解码器认领: {} 个后缀（内置 {} 个）",
        rupix_lib::decoders::declared_exts().len(),
        rupix_lib::core::formats::builtin_exts().len()
    );

    // 把自己造的痕迹清掉：探针验证完不该在 exe 旁边留垃圾（便携清洁原则）。
    // 目录本来就存在（用户真有解码器）时只清自己造的 DLL：remove_dir 对非空目录
    // 会失败，那种情况下「目录还在」是正常态，不判 FAIL。
    let _ = std::fs::remove_file(&fake);
    let _ = std::fs::remove_dir(&dir);
    let clean = if pre_dir {
        !fake.exists()
    } else {
        !dir.exists()
    };
    println!(" [4] 清理自身  : {}", if clean { "PASS" } else { "FAIL" });
    ok &= clean;
    ok
}

fn fmt(p: PathBuf) -> String {
    p.display().to_string()
}
