//! 插件文本结果窗：复用 [`super::text_window`] 的纯文本窗。
//!
//! 2026-10-29 用户口径：结果窗太大、不随内容自适应。改成 CurSeen 的"信息档"——
//! 系统标题栏 + 无框线白底的多行文本区 + 无按钮，高度按行数算；可点进去改，
//! 选中后 Ctrl+C 直接复制。
//!
//! 自动复制（插件声明 `clipboard.write`）的可见证据改成开窗时的一条 toast：
//! 无痕副作用必须留提示（AGENTS §八.7），但不再占正文一行，也不再有按钮。

use windui::core::EventCtx;

/// 打开（或聚焦）某插件的文本结果窗。
///
/// `copied`：宿主是否已把文本写进剪贴板（插件声明了 `clipboard.write` 时为真）。
pub fn show(ctx: &mut EventCtx, plugin_id: &str, plugin_name: &str, text: &str, copied: bool) {
    if copied {
        ctx.toast_ok("已复制到剪贴板");
    }
    let key = format!("plugin-result:{plugin_id}");
    let title = format!("{plugin_name} 结果");
    super::text_window::show(ctx, &key, &title, text);
}
