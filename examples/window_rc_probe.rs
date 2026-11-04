//! 最小复现：常驻模式下关掉一扇窗，窗口状态（`Rc`）到底有没有被放掉。
//!
//! 背景（2026-11-04）：RuPix 里关窗到托盘再唤回，**旧图还在**。查证发现关窗后
//! `Rc<WinRt>` 仍有一个强引用没释放，于是托盘唤回命中的是"已经关掉的那扇"。
//! 这个探针把同一个问法缩到最小：只开一扇窗、窗口状态里放一枚会打日志的 `Rc`，
//! 让它自己关掉，然后看那枚 `Rc` 的 `Drop` 有没有跑。
//!
//! 跑法：`cargo run --release --example window_rc_probe`
//! 判据：日志里出现 `STATE DROPPED` = 框架没有多扣一份；不出现 = 框架持有。

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use windui::app::{App, Window};
use windui::core::EventCtx;
use windui::prelude::*;

/// 窗口状态：本该随窗口一起消失的东西。
struct State;

impl Drop for State {
    fn drop(&mut self) {
        println!("STATE DROPPED");
    }
}

fn main() {
    println!("[probe] 常驻模式单窗：3 秒后自动关窗，然后看 State 是否释放");

    let state = Rc::new(State);
    let tick_state = state.clone();
    let close_state = state.clone();
    let ticks = Rc::new(Cell::new(0u32));

    // 主线程只留一枚弱引用：它若在关窗后仍能升级成功，说明有人扣着强引用。
    let weak = Rc::downgrade(&state);
    drop(state);

    let win = Window::new("rc-probe", 320, 200)
        .single("rc-probe")
        .on_interval(Duration::from_millis(100), move |ctx: &mut EventCtx| {
            let _ = &tick_state;
            let n = ticks.get() + 1;
            ticks.set(n);
            if n == 30 {
                println!("[probe] 发关窗请求");
                ctx.request_close();
            }
        })
        .on_close_request(move |_ctx| {
            let _ = &close_state;
            println!("[probe] close 决策：放行");
            true
        });

    let win = win.content(|| Element::row().fill());

    // 看门的那枚弱引用放 thread_local：`Weak` 不是 `Send`，而应用级轮询本来就在 UI 线程。
    thread_local! { static WEAK: std::cell::RefCell<Option<std::rc::Weak<State>>> = const { std::cell::RefCell::new(None) }; }
    WEAK.with(|w| *w.borrow_mut() = Some(weak));

    let n = Rc::new(Cell::new(0u32));
    App::resident("rc-probe")
        .tray(windui::platform::Tray::new().tooltip("rc-probe"))
        .start_window(win)
        .on_interval(Duration::from_millis(200), move |_ctx| {
            let k = n.get() + 1;
            n.set(k);
            if k.is_multiple_of(5) {
                let alive = WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.upgrade()).is_some());
                println!("[probe] t={}ms state_alive={}", k * 200, alive);
            }
            if k == 40 {
                let alive = WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.upgrade()).is_some());
                println!(
                    "[probe] VERDICT {}",
                    if alive {
                        "LEAK（框架多扣了一份强引用）"
                    } else {
                        "OK（关窗后状态被释放）"
                    }
                );
                windui::platform::post_quit();
            }
        })
        .run_resident();
}
