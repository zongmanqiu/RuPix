//! 「可下载 → 下载」的实机探针：**真联网**装一枚解码器，再读一遍自述确认它真能用。
//!
//! 走的是设置页「下载」按的那条路（`decoders::install`）——后台线程 + `.part` 暂存 +
//! 字节数校验 + 装完读自述。界面那一层只是把结果搬到信号上，所以这里能验的，界面就能做到。
//!
//! 用法（从 `main/` 目录；**把目录指到临时目录**，别污染发布目录）：
//!
//! ```text
//! RUPIX_DECODERS_DIR=<临时目录> cargo run --release --example decoder_fetch -- common [gitee|github]
//! ```
//!
//! 第二个参数是清单/下载来源（默认 gitee）；两个源走同一条安装链，只是清单 URL
//! 与 `gitee` / `github` 下载列不同。
//!
//! 判据三条：
//! 1. 下载返回 `Ok(路径)`，且文件名 = `<id>_<ver>.dll`（宿主"不解析文件名"的前提是它自己写对）；
//! 2. 落盘文件的字节数 = 清单里的 `size`（这一条在 `install` 里也守，这里再对一次账）；
//! 3. `list_decoders()` 里能读到这枚、自述 id 与清单一致、`Health::Ok`——**装完真能用**。
//!
//! 失败即退出码 1（供脚本判定）。

use rupix_lib::decoders;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let want = args
        .first()
        .cloned()
        .unwrap_or_else(|| "common".to_string());
    let src = args
        .get(1)
        .map(|s| decoders::SourceId::from_id(s))
        .unwrap_or(decoders::SourceId::Gitee);
    let dir = decoders::catalog::dir().expect("取不到解码器目录（RUPIX_DECODERS_DIR 要绝对路径）");
    std::fs::create_dir_all(&dir).expect("建目录失败");
    println!("目录  {}", dir.display());
    println!("来源  {}", src.label());

    // 与设置页「刷新」走同一条后台路（m148）：真联网拉清单，再等它落地。
    // 旧线上清单的 GitHub owner 错账会在解析期归一，所以 github 源这里也能直接测。
    // 地址与宿主同源：读 `Config` 的字段（ini 可改，探针不另走一路）。
    let cfg = rupix_lib::core::config::Config::default();
    let url = match src {
        rupix_lib::decoders::SourceId::Gitee => cfg.decoder_index_gitee.clone(),
        rupix_lib::decoders::SourceId::Github => cfg.decoder_index_github.clone(),
    };
    decoders::remote::start_refresh(src, url).expect("起刷新失败");
    let cat = loop {
        if let Some(out) = decoders::remote::poll_refresh() {
            break out.result.unwrap_or_else(|e| {
                eprintln!("读清单失败：{e}");
                std::process::exit(1);
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    println!(
        "清单  {} 枚（跳过 {} 项信息不全）",
        cat.entries.len(),
        cat.skipped
    );
    let Some(entry) = cat.entries.iter().find(|e| e.id == want) else {
        eprintln!("清单里没有 id = {want} 这一枚");
        std::process::exit(1);
    };
    println!(
        "目标  {} v{}  {}  {}",
        entry.name,
        entry.version,
        entry.size,
        if entry.downloadable() {
            "可下载"
        } else {
            "未发布"
        }
    );

    if let Err(e) = decoders::install::start(entry, src) {
        eprintln!("起下载失败：{e}");
        std::process::exit(1);
    }
    let mut last_pct = None;
    let out = loop {
        if let Some(p) = decoders::install::progress() {
            let pct = p.percent();
            if last_pct != Some(pct) {
                println!("进度  {pct:3}%  {}/{} B", p.got, p.total);
                last_pct = Some(pct);
            }
        }
        if let Some(o) = decoders::install::poll() {
            break o;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };

    let path = match out.result {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("下载失败：{msg}");
            std::process::exit(1);
        }
    };
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let expect = format!("{}_{}.dll", entry.id, entry.version);
    println!(
        "落盘  {file}  {} B",
        path.metadata().map(|m| m.len()).unwrap_or(0)
    );
    assert_eq!(file, expect, "落盘文件名应为 <id>_<ver>.dll");
    assert_eq!(
        path.metadata().map(|m| m.len()).unwrap_or(0),
        entry.size,
        "落盘字节数应等于清单 size"
    );

    // 装完真读一遍：这一步同时证明"文件是完整可加载的 DLL"和"自述 id 对得上"。
    decoders::request_rescan();
    let list = decoders::list_decoders();
    let got = list.iter().find(|d| d.id == entry.id);
    match got {
        Some(d) => {
            println!(
                "可用  {} v{}  {} 位图={} 原始={}",
                d.display_name,
                d.file,
                d.health.label(),
                d.extensions.len(),
                d.has_raw
            );
            if !matches!(d.health, decoders::Health::Ok) {
                eprintln!("装完却读不出健康的自述：{}", d.health.label());
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("装完在 list_decoders() 里找不到 id = {} 的这一枚", entry.id);
            std::process::exit(1);
        }
    }
    println!("OK");
}
