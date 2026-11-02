//! 「选项」页：窗口 / 托盘 / 空格预览 / 不透明度 / 邻图预读 / 幻灯片间隔 / 下载源。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
use super::*;

/// Options 页：复选框、下拉、滑块、输入框 —— 统一两列对齐（复选框也进右侧
/// 控件列，与下拉/滑块同列起点）。
///
/// 本页所有项都即时生效（m54）：窗口置顶 / 窗口大小 / 固定尺寸由 runtime 差分
/// 推给当前窗口，不再需要「下次启动生效」的尾巴。
pub fn options_page(h: &Handles) -> Element {
    Element::scroll()
        .fill()
        .padding_xy(16, PAGE_TOP_MARGIN)
        .child(
            Element::col()
                .fill()
                .spacing(0)
                // ini 键：stay_on_top（改勾选当场翻转当前窗口的置顶）
                .child(check_row("窗口置顶", h.stay))
                // ini 键：minimal_mode（即时生效：runtime 差分推给当前窗口，m51-D）
                .child(check_row("极简模式", h.minimal_mode))
                // ini 键：checker（即时生效：runtime 每帧把配置推给当前窗口，m53-L）
                .child(check_row("棋盘格", h.checker))
                .child(check_row("适应长图", h.long_fit))
                // ini 键：close_to_tray（勾上即生效：托盘图标当场出现、关窗转为隐藏）
                .child(check_row("最小化到托盘", h.to_tray))
                // ini 键：space_preview（全局空格监听，低级钩子、从不吞键，作用于整个
                // 系统——与快捷键页的看图窗口内键位不同源；16ms 轮询差分装/摘钩，见
                // platform::spacehook）
                .child(check_row("空格预览", h.space_preview))
                // ini 键：prefetch（默认勾上）。开着就是"按下方向键立刻换图"：
                // 后台先解好同文件夹前后各两张的**视口档**，与图片本身多大无关；
                // 最坏驻留由 `viewer` 的份数闸 + 256 MiB 字节闸封顶。关掉它，
                // 每次翻页都要现场解码（实测大图 130–260 ms，慢格式可到 1 s 以上）。
                .child(check_row("邻图预读", h.prefetch))
                // ini 键：init_window（启动时窗口取哪一档；具体宽高在下一行「窗口尺寸」）
                .child(option_row(
                    "启动窗口",
                    Element::dropdown(
                        InitWindow::ALL
                            .iter()
                            .map(|w| w.label().to_string())
                            .collect(),
                        h.init_win,
                    )
                    .width(CONTROL_GROUP_W),
                ))
                // ini 键：window_w / window_h（改下拉/文本框当场改窗；
                // 文本框等输入稳定 ~0.6s 再下发，避免打到一半被 clamp 成中间值）
                .child(option_row(
                    "窗口尺寸 (宽, 高)",
                    Element::text_input(h.win_size, "800, 600").width(CONTROL_GROUP_W),
                ))
                // ini 键：slide_interval（幻灯片每张停留秒数。整数输入框而不是四档下拉——
                // 秒数本来就是一条整数轴，压成档位反而给不出 4 秒、8 秒这种要的值。
                // 越界由 `clamp_slide_interval` 夹进 1..=600，打到一半的中间态不改配置）
                .child(option_row(
                    "幻灯片间隔 (秒)",
                    Element::text_input(h.slide_secs, "3").width(CONTROL_GROUP_W),
                ))
                // ini 键：download_source（联网来源：解码器清单 / 插件清单 / 将来的软件本体更新
                // **共用这一个**）。2026-10-30 用户口径：一个用户机器的网络是固定的，所以只该有
                // 一个开关——两张下载卡左侧的下拉已撤，跟着这里走。
                .child(option_row(
                    "下载源",
                    Element::dropdown(
                        crate::core::config::SourceId::ALL
                            .iter()
                            .map(|s| s.label().to_string())
                            .collect(),
                        h.download_source,
                    )
                    .width(CONTROL_GROUP_W),
                ))
                .child(slider_row("窗口不透明度", h.opacity, h.opacity_label)),
        )
}
