//! 与看图逻辑无关的通用小工具。
//!
//! # 为什么这些函数必须住在 core
//!
//! 它们的消费方跨了不止一层（例如字节数格式化同时被 `core::viewer` 的状态栏和
//! `ui::settings` 的属性窗使用）。分层是 core(0) → platform(1) → decoders(2) → ui(3)，
//! core 不能反向依赖上层，所以**唯一实现只能落在最底层**，由上层各自取用。
//!
//! 各层自己抄一份必然出现口径漂移：历史上 `fmt_bytes` 曾在 core 与 decoders 各有一份，
//! 一份 KB 取整、一份保留一位小数，且只有一份有 GB 档——同一个文件的大小在属性窗
//! 和信息条里显示成两个不同的数字。这类"看着都是对的、合起来就打架"的重复，
//! 靠 review 很难发现，只能靠"只有一份实现"来根除。

/// 人类可读的字节数（B / KB / MB / GB，1024 进位）。
///
/// 小于 1 KB 显示整数（`512 B`），其余一律一位小数（`1.5 KB` / `2.0 GB`）——
/// 展示位宽固定，状态栏不会因进位而左右跳动。
pub fn fmt_bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    if n < 1024 {
        return format!("{n} B");
    }
    let f = n as f64;
    let (v, unit) = if f < K * K {
        (f / K, "KB")
    } else if f < K * K * K {
        (f / (K * K), "MB")
    } else {
        (f / (K * K * K), "GB")
    };
    format!("{v:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单位边界的取整必须正确：状态栏与属性窗直接把它显示给用户，
    /// 而两处共用这一个实现（分层的硬约束，见模块头）。
    #[test]
    fn fmt_bytes_rounds_at_unit_boundaries() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(fmt_bytes(2500 * 1024 * 1024), "2.4 GB");
    }

    /// GB 档必须真实存在：没有它，≥1GB 的文件会显示成 "2048.0 MB" 这类怪值。
    #[test]
    fn fmt_bytes_has_a_gb_step() {
        let s = fmt_bytes(3 * 1024 * 1024 * 1024);
        assert!(s.ends_with("GB"), "实际: {s}");
    }
}
