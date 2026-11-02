//! 临时探针：查宿主眼里的"矢量 + 区域能力"到底装了什么。
//!
//! 打印三样：
//! 1. 每枚 DLL 的 `vector_exts` / `vector_region`（宿主判定的硬证据）；
//! 2. `core::formats::declared_region_vector_exts()`（能按区域重渲的后缀并集）；
//! 3. `is_region_renderable("wmf")`（viewer 的唯一判据）。
//!
//! ★ 这三样只要有一处为空，viewer 就**不会发区域请求**，矢量会退回"整幅按目标尺寸
//! 渲染"——那正是"放大后显示不全"的现象来源。

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    println!("══ 1. 每枚 DLL 的区域能力 ══");
    for e in rupix_lib::decoders::list_decoders() {
        println!(
            "  id={:<8} abi={:?} vector_exts={:?} vector_region={} has_vector={}",
            e.id, e.abi, e.vector_exts, e.vector_region, e.has_vector
        );
    }

    println!("\n══ 2. 宿主认定的可按区域重渲后缀并集 ══");
    let mut v: Vec<String> = rupix_lib::core::formats::declared_region_vector_exts()
        .into_iter()
        .collect();
    v.sort();
    println!("  {v:?}");

    println!("\n══ 3. viewer 的判据 ══");
    for ext in ["wmf", "wmz", "svg", "svgz", "png"] {
        println!(
            "  is_region_renderable({ext:?}) = {}",
            rupix_lib::core::formats::is_region_renderable(ext)
        );
    }
}
