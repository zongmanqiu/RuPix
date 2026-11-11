//! 离屏探针：把一枚 `.zip` / `.cbz` 的某一层交给 `core::archive::enter`，打印落盘结果。
//!
//! 用法：`cargo run --example archive_probe -- <压缩包路径> [层段...]`
//!
//! 例如
//! - `archive_probe demo.cbz`             → 根层
//! - `archive_probe demo.cbz inner.zip`   → 内层包那一层
//! - `archive_probe demo.cbz inner.zip ch1` → 再往里一层
//!
//! 不建窗、不装解码栈；展开出的目录留在 `<exe 目录>/temp/`，可直接喂 `--list` / `--render`
//! 复查「宿主自己认不认得包里的图」。`SUB` 行是这一层可以直接进下去的项。

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first() else {
        eprintln!("用法：archive_probe <压缩包路径> [层段...]");
        std::process::exit(2);
    };
    let source = PathBuf::from(first);
    let segs: Vec<String> = args[1..].to_vec();

    match rupix_lib::core::archive::enter(&source, &segs) {
        Ok(layer) => {
            println!("DIR={}", layer.dir.display());
            println!("IMAGES={}", layer.images);
            let mut names: Vec<String> = match std::fs::read_dir(&layer.dir) {
                Ok(rd) => rd
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| !n.starts_with('.'))
                    .collect(),
                Err(_) => Vec::new(),
            };
            names.sort();
            for n in &names {
                println!("FILE={n}");
            }
            for s in rupix_lib::core::archive::subs_of(&layer.dir) {
                println!("SUB={}\t{}", if s.dir { "d" } else { "a" }, s.seg);
            }
        }
        Err(e) => {
            println!("ERROR={e}");
            std::process::exit(3);
        }
    }
}
