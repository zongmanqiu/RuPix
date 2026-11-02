//! 每个 FFI 调用的统一闸门：宿主侧再兜一层 panic **与** SEH 访问违规。
//!
//! 解码器自己**也**该在导出函数里 catch_unwind（规范 §六 第 3 条）；这里是纵深防御——
//! 万一解码器的闸门写漏了，宿主这层能把 unwind 挡在 ABI 边界外，不至于直接 abort。
//!
//! # SEH 隔离（Windows）
//!
//! `catch_unwind` 只接得住 Rust panic。解码器里一句 `null.write(1)` 触发的是 **SEH 访问违规**，
//! 不产生 unwind，进程内无法用 Rust 捕获——会连主程序一起带走。x86_64-pc-windows-msvc
//! 没有稳定的 Rust SEH 语法，所以这里把真正的调用包进 C 的 `__try/__except` 隔离层
//! `sv_seh_call`（`seh_shim.c`）：只吃 Access Violation / Illegal Instruction / Privileged
//! Instruction / In Page Error / **Stack Overflow** 这几类"硬故障"，Rust 自己的 panic（另一套
//! SEH 异常码）一律 `EXCEPTION_CONTINUE_SEARCH` 放给下面的 `catch_unwind`。
//!
//! 拦下之后返回的是**分类**而不是一个笼统错误：[`SehFault`]（解码器自己崩了 → 调用方 `penalize`
//! 三振）与 [`MediaFault`]（读页失败 = 介质掉了 → 不冤枉解码器，审计 V8）。栈溢出以前不在这张
//! 表里，递归爆栈等于进程 abort（审计 V7）——现在它按 `SehFault` 计，窗口不再无声消失。
//! 详见 [`SehCtx`]。

use std::panic::{catch_unwind, AssertUnwindSafe};

/// 在 panic/SEH 双重保护下跑一次 `f`。
///
/// ★ 调用期间 `IN_BOUNDARY` 置位：`panic_guard` 的钩子读它来决定弹不弹窗——进到这里
///   的 panic **已经有人处理**（宿主的三振隔离 / 调用方的 `Err`），再弹一个"程序崩了"
///   的框就是把一次"这张图解不出来"说成"程序崩溃"。
pub fn invoke<F, T>(f: F) -> std::thread::Result<T>
where
    F: FnOnce() -> T,
{
    let _marker = enter_boundary();
    #[cfg(windows)]
    {
        let mut ctx = SehCtx {
            f: Some(f),
            result: None,
        };
        // SAFETY: `ctx` 在调用期间存活于本栈帧；`seh_run::<F, T>` 只动它自己的字段，
        // 且 `invoke` 与 `seh_run` 在同一线程、同一调用栈上。
        let rc = unsafe {
            sv_seh_call(
                seh_run::<F, T>,
                (&mut ctx) as *mut _ as *mut std::ffi::c_void,
            )
        };
        if rc != 0 {
            // 硬故障被 SEH 拦下。分类由 shim 给：`SEH_MEDIA_FAULT`（读页失败 = 介质掉了）
            // 不冤枉解码器，其余（访问违规 / 非法指令 / **栈溢出**）照常三振。栈溢出从本轮起
            // 才进得到这里——以前它直接 `EXCEPTION_CONTINUE_SEARCH`，连主窗口一起没了（V7）。
            let fault: Box<dyn std::any::Any + Send> = if rc == SEH_MEDIA {
                Box::new(MediaFault)
            } else {
                Box::new(SehFault)
            };
            return Err(fault);
        }
        // rc==0 但结果槽为空只能是 seh_run 自身逻辑坏了（invoke 一定先填 Some(f)）。
        // 不在这里 panic（这是解码器宿主的热路径）：降级为硬故障，走三振隔离。
        ctx.result.unwrap_or_else(|| Err(Box::new(SehFault)))
    }
    #[cfg(not(windows))]
    {
        catch_unwind(AssertUnwindSafe(f))
    }
}

// 本线程当前跑在几层受控边界里（`invoke` 可重入；用 doc 注释会被 `thread_local!` 判为
// 悬空文档，故用普通注释）。
thread_local! {
    static IN_BOUNDARY: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 当前是否在受控边界内（有没有人在接 panic）。panic 钩子据此决定弹不弹窗。
///
/// 用计数而不是 bool 是给"边界里再进边界"留路——`invoke` 嵌套时内层还没退，flag 不能
/// 被内层复位掉。
pub fn in_boundary() -> bool {
    IN_BOUNDARY.with(|c| c.get() > 0)
}

/// 进入边界，离开包（含 panic 展开）自动退出。
fn enter_boundary() -> BoundaryGuard {
    IN_BOUNDARY.with(|c| c.set(c.get() + 1));
    BoundaryGuard
}

/// [`enter_boundary`] 的退包凭证（见 [`Drop`]）。
struct BoundaryGuard;

impl Drop for BoundaryGuard {
    fn drop(&mut self) {
        IN_BOUNDARY.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// `sv_seh_call` 的返回码：与 `seh_shim.c` 的 `SEH_MEDIA_FAULT` 是**同一份契约的两个书写处**，
/// 改一边必须改另一边（C 侧无法 include 这个常量，所以靠注释钉住）。
#[cfg(windows)]
const SEH_MEDIA: i32 = -2;

/// SEH 硬故障的标记：被 C 隔离层拦下后作为 `Err` 返回，与 `Err(Box<dyn Any+Send>)` 兼容。
///
/// 单元结构体自动是 `Send + 'static`，可塞进 `Box<dyn Any + Send>`。
#[cfg(windows)]
pub struct SehFault;

/// 读页失败的标记（`EXCEPTION_IN_PAGE_ERROR`）：网络盘掉线、U 盘被拔、文件被截断。
///
/// ★ 它**不是**解码器的错，所以不进三振（审计 V8）：冤枉一个好解码器会让那个格式整场会话
/// "无人认领"，而日志写着"已隔离"更是把介质故障说成解码故障——两条错线索一起。
/// 但**仍然要吞**：不吞就是整个进程没了。
#[cfg(windows)]
pub struct MediaFault;

/// 这次 FFI 失败的载荷是不是"介质掉了"。非 Windows 分支恒 `false`（那里没有 SEH）。
pub fn is_media_fault(err: &(dyn std::any::Any + Send)) -> bool {
    #[cfg(windows)]
    {
        err.is::<MediaFault>()
    }
    #[cfg(not(windows))]
    {
        let _ = err;
        false
    }
}

#[cfg(windows)]
#[link(name = "seh_shim")]
extern "C" {
    /// C 隔离层（`seh_shim.c`）：用 `__try/__except` 包住一次调用，硬故障返回 -1。
    fn sv_seh_call(
        thunk: extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_int,
        ctx: *mut std::ffi::c_void,
    ) -> std::ffi::c_int;
}

/// 把待调用的闭包与结果槽打包，跨 C 调用边界传递（泛型，不要求 `'static`——
/// 闭包捕获的局部借用在 `invoke` 栈帧内有效）。
#[cfg(windows)]
struct SehCtx<F, T> {
    f: Option<F>,
    result: Option<std::thread::Result<T>>,
}

/// `sv_seh_call` 实际跑的 thunk：在 SEH 保护内执行闭包，把结果（含 panic）回填。
#[cfg(windows)]
extern "C" fn seh_run<F, T>(ctx: *mut std::ffi::c_void) -> std::ffi::c_int
where
    F: FnOnce() -> T,
{
    // SAFETY: 调用方传入的是合法的 `SehCtx<F, T>` 指针，且 `invoke` 在生命周期内持有它。
    let ctx = unsafe { &mut *(ctx as *mut SehCtx<F, T>) };
    // 闭包缺失 = 调用方违约。extern "C" 里绝不能 panic（展开出边界是 abort）：
    // 返回 -1 让 invoke 侧按硬故障处理。
    let Some(f) = ctx.f.take() else {
        return -1;
    };
    ctx.result = Some(catch_unwind(AssertUnwindSafe(f)));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ m128:受控边界必须能被 panic 钩子读到(`panic_guard` 靠它决定弹不弹窗)。
    ///
    /// 这里钉的是"标记本身":边界内 true、出来复位、嵌套不丢层。真实效果是"解码器 FFI
    /// 里的 panic 不再弹'程序崩溃'那个框",那一半由 `panic_guard` 自己的结构保证,
    /// 造不出来:单元测里装不了生产钩子(装了会污染整个测试进程)。
    #[test]
    fn boundary_flag_is_visible_inside_and_reset_outside() {
        assert!(!in_boundary(), "边界外不该标记");
        let seen = std::rc::Rc::new(std::cell::Cell::new(false));
        let probe = seen.clone();
        // 这次 panic 会走默认钩子打印一行:受控标记能不能被读到正是要用真 panic 量。
        let r = invoke(|| {
            probe.set(in_boundary());
            panic!("边界标记探针");
        });
        assert!(r.is_err(), "panic 必须被边界接住");
        assert!(seen.get(), "边境内必须标记为 true");
        assert!(!in_boundary(), "离开 invoke 必须复位");
    }

    /// 嵌套:内层还没退,信号不能被内层复位掉。
    #[test]
    fn nested_boundary_keeps_the_flag_until_the_outer_one_returns() {
        let inner = std::rc::Rc::new(std::cell::Cell::new(false));
        let probe = inner.clone();
        let _ = invoke(|| {
            let _ = invoke(|| {
                probe.set(in_boundary());
            });
            assert!(in_boundary(), "内层退出后外层仍需在边界内");
        });
        assert!(inner.get(), "内层必须仍在边界内");
        assert!(!in_boundary());
    }
}
