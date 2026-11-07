//! 自更新链路探针：直接走 `platform::http`（就是宿主用的 WinHTTP 那条路），
//! 打印两个清单地址的真实状态码 / 耗时 / 错误，判断「检查更新」卡在哪一层。
//!
//! 用法：`cargo run --release --example update_probe`

use std::time::Instant;

use rupix_lib::platform::http;

fn main() {
    let urls = [
        "https://gitee.com/qiuzongman/RuPix/raw/master/update.json",
        "https://raw.githubusercontent.com/zongmanqiu/RuPix/master/update.json",
    ];
    for url in urls {
        let t0 = Instant::now();
        let r = http::get(url, 256 * 1024);
        let ms = t0.elapsed().as_millis();
        match r {
            Ok(b) => println!("OK   {ms:>6} ms  {} bytes  {url}", b.len()),
            Err(e) => println!("ERR  {ms:>6} ms  {e}  {url}"),
        }
    }
}
