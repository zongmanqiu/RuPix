//! 挂死恢复端到端探针（2026-11-04）。
//!
//! 现场问题是"打开一张图卡住，之后所有图都打不开"：解码线程在某个解码器的
//! `iv_render` 里死循环，队列后面的请求全部饿死，而 `catch_unwind` / SEH 只拦崩不拦卡。
//!
//! 这里不需要真的找一个会挂死的 DLL：装一个**第一次必挂 60 秒**的假后端，再提交两笔
//! 请求，看监督者会不会在第一笔超时后熔断它、换 worker、并让第二笔正常回包。
//!
//! 运行：
//! ```text
//! cargo run --release --example hang_recover_probe
//! ```
//! 通过判据（打印在末尾）：第一条 Done 是含「超时」的 Err、耗时 ≈ 超时值；第二条 Done
//! 在超时值之内返回（成功或真后端的错误都行，**不是**再等 60 秒）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use rupix_lib::core::decode::{self, DecodeBackend, Decoder, ReqKind};
use rupix_lib::core::pixels::Decoded;

/// **进程级**调用计数：第一次睡 60 秒（模拟挂死），之后正常返回。
///
/// 必须是静态而不是结构体字段：超时后监督者会起一个**新 worker**，而新 worker 是新的
/// 后端实例（`install_local_backend` 每次新建）——字段计数会从 0 重来，于是"每一笔都挂"，
/// 探针就测不出"挂死之后还能继续解"这件事。
/// 现实里对应的机制是**按解码器 id 熔断**（`is_hung_decoder`）：同一个 DLL 不会再进第二次。
static CALLS: AtomicUsize = AtomicUsize::new(0);

struct HangBackend;

impl DecodeBackend for HangBackend {
    fn decode(
        &mut self,
        path: &std::path::Path,
        _ext: &str,
        _max_w: u32,
        _max_h: u32,
        _frame_index: u32,
        _raw: Option<decode::RawParams>,
        _allow_scaled: bool,
    ) -> Result<Decoded, rupix_lib::core::error::ViewFailure> {
        let n = CALLS.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            println!("[probe] 第 1 次调用：故意挂 60 秒（模拟解码器死循环）");
            std::thread::sleep(Duration::from_secs(60));
        }
        Err(rupix_lib::core::error::ViewFailure::decode(format!(
            "假后端：{}（第 {} 次调用）",
            path.display(),
            n + 1
        )))
    }
}

fn main() {
    // 超时压到 2 秒：判据看"第一笔 2 秒左右回包、第二笔不被拖住"。
    decode::set_decode_timeout(2);
    decode::install_backend_factory(|| Box::new(HangBackend));

    let (tx, rx) = channel();
    let dec = Decoder::spawn_with(tx);

    let t0 = Instant::now();
    // ★ 第一笔必须用 `Thumb`：`Main` 会被后一笔的 floor 判成过期、当场早退（空串 Err），
    //   那样根本进不了 worker，测的就不是超时了（第一版探针就是这么假绿的）。
    let seq1 = dec.request(std::path::Path::new("a.png"), 64, 64);
    let seq2 = dec.request_frame(std::path::Path::new("b.png"), 64, 64, 0, ReqKind::Main);
    println!("[probe] 已提交 seq={seq1}（Thumb，会挂）与 seq={seq2}（Main，应正常回）");

    let mut first_ms = 0u128;
    let mut second_ms = 0u128;
    let mut first_err = String::new();
    let mut second_err = String::new();
    for _ in 0..2 {
        let done = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("两笔都该在 30 秒内回包（否则恢复机制没生效）");
        let ms = t0.elapsed().as_millis();
        match &done.out {
            Ok(_) => println!("[probe] seq={} ms={ms} OK", done.seq),
            Err(e) => println!("[probe] seq={} ms={ms} ERR={e}", done.seq),
        }
        if done.seq == seq1 {
            first_ms = ms;
            first_err = done.out.err().map(|e| e.detail).unwrap_or_default();
        } else {
            second_ms = ms;
            second_err = done.out.err().map(|e| e.detail).unwrap_or_default();
        }
    }

    // 假后端不经过 `decoders::decode_with`，所以"熔断哪个 id"这条链路这里看不到
    //（它由 `decoders` 层上报、`core` 记录、单测 `hung_set_quarantines_for_the_whole_session`
    // 钉住）。本探针只证**超时与换 worker**这一段。
    println!("---- 判据 ----");
    println!("first_ms={first_ms}");
    println!("first_err={first_err}");
    println!("second_ms={second_ms} (挂死值 60_000，恢复后应远小于它)");
    println!("second_err={second_err}");
    // ★ 判据用"两笔各自的结果"，不假设谁先谁后：队列对 `Main` **插队**（`Queue::pop`），
    //   所以先挂的往往是后来的 Main，不是先发的 Thumb。第一版探针假设了顺序，读数是
    //   "两笔都超时/都正常"之外的第三种——其实是机制正确、判据写错。
    let timeouts = [&first_err, &second_err]
        .iter()
        .filter(|e| e.contains("超时"))
        .count();
    let backend_hits = [&first_err, &second_err]
        .iter()
        .filter(|e| e.contains("假后端"))
        .count();
    let slowest = first_ms.max(second_ms);
    println!("timeouts={timeouts} backend_hits={backend_hits} slowest_ms={slowest}");

    // 通过 = 恰好一笔超时（被放弃）+ 恰好一笔真的进到后端（换 worker 后立刻解完），
    // 且全程不给 60 秒的挂死留出任何时间。
    let ok = timeouts == 1 && backend_hits == 1 && slowest < 8000;
    println!("PROBE {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
}
