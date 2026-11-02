//! 临时探针：`Decoded::natural` 是不是**随请求目标尺寸漂移**。
//!
//! 疑点：宿主日志里 WMF 的 `natural=557x561`（= 请求的目标尺寸），
//! 而整幅探针（target=0，即"要原始尺寸"）拿到的是 **3102x3124**。
//!
//! 若两者不一致 ⇒ `natural` 报的不是**文档自然尺寸**，而是"这一次渲出来的尺寸"——
//! 而区域请求的 x/y/w/h 单位正是"文档自然像素"（ABI v6 明写），
//! 坐标系一错，宿主要的那块和真图那块就不是同一块 ⇒ 放大后显示不全 / 位置错。
//!
//! 这一条只读，不改任何东西。

use std::path::Path;

fn main() {
    rupix_lib::platform::logging::install_logging();
    rupix_lib::install_decode_stack();

    let args: Vec<String> = std::env::args().collect();
    let Some(p) = args.get(1) else {
        println!("用法：natural_drift_probe <文件>");
        return;
    };
    let path = Path::new(p);

    println!("══ 不同目标尺寸下的 natural ══");
    for (w, h) in [
        (0u32, 0u32), // 要原始尺寸
        (557, 561),   // 宿主打开时的视口档
        (1127, 897),  // 另一个视口档
        (2000, 2000), // 更大的目标
    ] {
        match rupix_lib::core::decode::probe_decode(path, w, h) {
            Ok(d) => println!(
                "  target={:<5}x{:<5} → bitmap={}x{}  natural={}x{}",
                w, h, d.bitmap.width, d.bitmap.height, d.natural.0, d.natural.1
            ),
            Err(e) => println!("  target={w}x{h} → ERR {e}"),
        }
    }
}
