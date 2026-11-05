//! 上次异常退出的恢复提示：本次启动问一句"要不要临时禁用解码器"。
//!
//! # 何时出现
//!
//! [`crate::platform::session::take_prior_aborted`] 返回 true——`temp\rupix.run` 会话戳
//! 残留（上次没走到 `close_session` 就把进程干掉了：panic、任务管理器强杀、断电…）。
//! **只在本次首帧轮询里弹一次**，用户关掉或选完就不再骚扰。已在 `--safe-mode` 下
//! 直接跳过（用户已经用命令行止损过，弹窗反而像重复提问）。
//!
//! # 为什么"当场禁用解码器"而不是"重启到安全模式"
//!
//! 最初设计里，第二条按钮是「以安全模式重启」，落地时发现两个不划算：
//!
//! 1. **单实例 Mutex 释放有竞争**：`Local\rupix_si_mutex` 直到本进程结束才放开，
//!    新起的实例若抢在释放前 `claim_instance` 会被判为二次实例、走转发然后自己退出——
//!    用户看到的是"点了重启没反应"。要绕开得让 `cmd.exe` 兜个延时（`timeout & rupix.exe`），
//!    又引入一个可见/半可见的 shell 进程。
//! 2. **收益与"当场禁解码器"完全一样**：`crate::decoders::Host` 在 `try_decode`/`list_decoders`/
//!    `declared_exts` 三条主入口**每次调用**都读 `safe_mode()` 旗，运行时置位与启动早期
//!    `--safe-mode` 命令行落到的是同一个 `AtomicBool`。既然行为等价，弹窗里那条按钮
//!    只是把用户多绕一圈。
//!
//! 于是本模块只留两个当场动作：「继续使用」什么也不改；「禁用所有解码器」
//! 走 [`crate::decoders::set_safe_mode`] + [`crate::decoders::request_rescan`] 让下一次的
//! 格式清单也剔除解码器认领的扩展名。都不改配置、不落盘，下次启动是干净状态。
//!
//! # 崩溃日志怎么给
//!
//! [`crate::platform::panic_guard`] 已经把 panic 详情写到 `temp\crash-<今天>.log`。
//! 这里只把路径显示出来供用户自己打开——不代跳（`explorer.exe` 打开一个具体文件不是
//! 便携软件该顺手做的事），也不复制路径到剪贴板（用户真要复制会自己框选）。

use std::path::PathBuf;

use windui::core::EventCtx;
use windui::prelude::*;

use super::tokens::{to_color, BG};

/// 会话戳残留时的启动提示。`log_path` 允许为 `None`（只读介质下崩溃钩子写不进日志，
/// 但会话戳本身也写不进——真到那一步 `take_prior_aborted` 也返回 false，这里进不来）。
pub fn show(ctx: &mut EventCtx, log_path: Option<PathBuf>) {
    let keep = |ctx: &mut EventCtx| ctx.request_close();
    let disable = |ctx: &mut EventCtx| {
        // 与 `--safe-mode` 命令行落到的是同一枚旗标（见模块头「为什么'当场禁解码器'」）。
        crate::decoders::set_safe_mode(true);
        // 让下一次 `Host` 取目录时重扫（重扫里 `!safe_mode()` 分支跳过解码器枚举，
        // 结果就是"解码器认领的扩展名从画廊清单里也消失"）。
        crate::decoders::request_rescan();
        ctx.toast_ok("本次会话已禁用全部解码器");
        ctx.request_close();
    };

    let notes = evidence_lines(
        log_path.as_deref(),
        crate::platform::crash_evidence::newest_dump().as_deref(),
        crate::platform::session::prior_stamp().as_deref(),
    );

    // 高度交给 `fit_content`（windui 补丁 07）而不是写死 200：行数会随"有没有转储/
    // 上次会话那两行"变化，写死的高度要么截掉线索要么留一大片空白。
    ctx.open_window(
        Window::new("RuPix 上次异常退出", 460, 200)
            // 信息窗客户区恒白（标题栏由系统绘制，不在其内）。
            .bg(to_color(BG))
            .resizable(false)
            .centered(true)
            .owned(true)
            .single("recovery-hint")
            .fit_content(180, 420)
            .content(move || {
                let mut col = Element::col()
                    .fill()
                    .padding(16)
                    .spacing(10)
                    .child(
                        Element::label("RuPix 检测到上次运行未正常结束。")
                            .font_size(13.5)
                            .fg_role(Role::Text),
                    )
                    .child(
                        Element::label(
                            "如果反复闪退，可以先临时禁用所有解码器继续看图（下次启动会自动恢复）。",
                        )
                        .font_size(12.5)
                        .fg_role(Role::TextMuted),
                    );
                for n in &notes {
                    col = col.child(Element::label(n.clone()).font_size(12.0).fg_role(Role::TextMuted));
                }
                col.child(Element::flex_spacer()).child(
                    Element::row()
                        .spacing(8)
                        .cross(Align::Center)
                        .child(Element::flex_spacer())
                        .child(Element::button("继续使用").on_click(keep))
                        .child(Element::button("禁用所有解码器").on_click(disable)),
                )
            }),
    );
}

/// 组装"取证线索"那几行。三条各自独立可缺：**缺哪条就不写哪条**——没有 `.dmp` 时不许
/// 出现"崩溃转储：…"，取不到上次会话时不许编一句时间。日志那一行总有（写不进去时
/// 那句实话本身就是内容）。
fn evidence_lines(
    log: Option<&std::path::Path>,
    dump: Option<&std::path::Path>,
    prior: Option<&str>,
) -> Vec<String> {
    let mut v = Vec::new();
    v.push(match log {
        Some(p) => format!("详细日志：{}", p.display()),
        None => "本次未能写入日志（便携目录只读？）。".to_string(),
    });
    if let Some(d) = dump {
        v.push(format!("崩溃转储：{}", d.display()));
    }
    if let Some(s) = prior {
        v.push(format!("上次会话：{s}"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 三条线索都在 → 三行，每行前缀认得出来（用户照着就能把文件发给我们）。
    #[test]
    fn all_three_clues_get_a_line() {
        let v = evidence_lines(
            Some(Path::new(r"C:\rupix\temp\crash-2026-10-19.log")),
            Some(Path::new(r"C:\rupix\temp\crash-2026-10-17-203311.dmp")),
            Some("pid=4242 启动于 2026-10-17 20:33:11"),
        );
        assert_eq!(v.len(), 3);
        assert!(v[0].starts_with("详细日志："));
        assert!(v[1].starts_with("崩溃转储："), "实得：{v:?}");
        assert!(v[2].contains("pid=4242"), "实得：{v:?}");
    }

    /// 只写了日志、没有转储也没有旧戳 → **一行都不许多**。这是"不许假装成功"的门面。
    #[test]
    fn absent_clues_are_not_invented() {
        let v = evidence_lines(
            Some(Path::new(r"C:\rupix\temp\crash-2026-10-19.log")),
            None,
            None,
        );
        assert_eq!(v.len(), 1, "实得：{v:?}");
        assert!(!v[0].contains("转储"));
        let v2 = evidence_lines(None, None, None);
        assert_eq!(v2, vec!["本次未能写入日志（便携目录只读？）。".to_string()]);
    }
}
