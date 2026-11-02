//! 显示器呈现节奏：这块屏**一秒真能换出几张画面**。
//!
//! # 为什么宿主需要这个数字
//!
//! 拖动时的重绘原先是"每收到一条鼠标消息就画一帧"，实测手速 120 消息/s 时应用画到
//! **90–119 帧/s**，而接这块屏的是 60 Hz——多出来的那三到五成帧一个也没呈现出去，
//! 纯粹是白烧 CPU（M8，`.temp/drag_cpu_rig4.ps1`：同一窗口里每帧 5.1–5.7 ms 的整机
//! 4.2–5.0% 全是这么来的）。要"画得不多于显示得出"，分母只能是刷新率，所以把它放在
//! 这一层取一次，交给 [`crate::ui::chrome`] 的平移限速用。
//!
//! # 为什么取主屏就够
//!
//! `GetDeviceCaps(VREFRESH)` 问的是**当前显示设备**的刷新率。副屏与主屏不同率时这里
//! 读到的是主屏那一档；看图器一次只在一块屏上被人拖，误差的方向也安全——读到偏高的
//! 值就多画几帧（只是没省到），读到偏低的值才可能显慢，而 40 Hz 的下限钳住了后者。
//!
//! # 取不到怎么办
//!
//! 返回 [`FALLBACK_HZ`]。**不许**"取不到就不限速"：那等于把这条优化在无人值守的服务
//! 会话、远程桌面、头less CI 上悄悄关掉，行为随环境漂移比钳错更难查。

use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, VREFRESH};

/// 查询失败 / 读数离谱时采用的刷新率（本机实测 60，也是绝大多数办公屏）。
const FALLBACK_HZ: u32 = 60;
/// 下限：低于这个数就不是"省电"而是"卡顿"了，宁可当读数不可信。
const MIN_HZ: u32 = 40;
/// 上限：再高基本是虚拟显示器与采集卡报的假数，按它限速会把 CPU 放到没必要的高。
const MAX_HZ: u32 = 240;

/// 当前显示设备的刷新率（Hz）。任何一步失败都回 [`FALLBACK_HZ`]。
pub fn refresh_hz() -> u32 {
    // `GetDC(None)` = 整个屏幕的 DC；失败返回**无效 HDC**，不是错误码。
    // `ReleaseDC` 的返回值只表示"这个 DC 是不是你的"，取不到刷新率时它没什么可判的。
    let raw = unsafe {
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            0
        } else {
            let v = GetDeviceCaps(Some(hdc), VREFRESH);
            ReleaseDC(None, hdc);
            v
        }
    };
    clamp_hz(raw)
}

/// 把 GDI 的读数收敛成可用的刷新率。与系统调用拆开是为了让"离谱读数怎么办"这条
/// 判断能被单测钉住——生产里我们没法造一块 0 Hz 或 400 Hz 的屏。
fn clamp_hz(raw: i32) -> u32 {
    // 负数只可能来自没初始化/虚拟驱动；取绝对值后照样过区间判定。
    let hz = raw.unsigned_abs();
    if (MIN_HZ..=MAX_HZ).contains(&hz) {
        hz
    } else {
        FALLBACK_HZ
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 离谱读数一律回落到 60，可信读数原样通过——限速的分母不许被驱动带的节奏走。
    #[test]
    fn absurd_refresh_readings_fall_back_to_sixty() {
        assert_eq!(clamp_hz(0), FALLBACK_HZ, "取不到（0）不许等于不限速");
        assert_eq!(clamp_hz(39), FALLBACK_HZ, "低于 40 当卡顿处理");
        assert_eq!(clamp_hz(40), 40);
        assert_eq!(clamp_hz(60), 60);
        assert_eq!(clamp_hz(144), 144, "高刷屏不许被钳到 60");
        assert_eq!(clamp_hz(240), 240);
        assert_eq!(clamp_hz(241), FALLBACK_HZ);
        assert_eq!(clamp_hz(-59), 59, "负数取绝对值后再判");
    }

    /// 真机这一路只要跑得通就必须在可信区间里（CI / 远程桌面上会是兜底值，同样合格）。
    #[test]
    fn live_query_returns_a_usable_number() {
        let hz = refresh_hz();
        assert!((MIN_HZ..=MAX_HZ).contains(&hz), "读数离谱：{hz}");
    }
}
