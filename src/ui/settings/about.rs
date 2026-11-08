//! 「关于」页：版本与构建信息、赞助码、手动检查更新。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
use super::*;

/// 关于页：信息卡、赞助码。
pub fn about_page(h: &Handles) -> Element {
    const HOMEPAGE: &str = "https://gitee.com/qiuzongman/RuPix";
    const GITHUB_MIRROR: &str = "https://github.com/zongmanqiu/RuPix";

    const LOGO_SVG: &[u8] = include_bytes!("../../../assets/icon.svg");
    const DONATE_WECHAT_SVG: &[u8] = include_bytes!("../../../assets/WeChatPay.svg");
    const DONATE_ALIPAY_SVG: &[u8] = include_bytes!("../../../assets/ALiPay.svg");

    const LOGO_SIZE: i32 = 56;
    const DONATE_QR_SIZE: i32 = 160;
    const DONATE_QR_GAP: i32 = 28;
    const DONATE_QR_BORDER: i32 = 2;

    const ABOUT_CARD_BG: [u8; 3] = [246, 248, 250];
    const ABOUT_CARD_RADIUS: f32 = 8.0;
    const ABOUT_CARD_PAD: i32 = 16;
    const ABOUT_CARD_GAP: i32 = 12;

    let card_bg = to_color(ABOUT_CARD_BG);
    let card_radius = ABOUT_CARD_RADIUS;
    let card_pad = ABOUT_CARD_PAD;

    let header = Element::row()
        .child(
            Element::image_svg(LOGO_SVG, None)
                .width(LOGO_SIZE)
                .height(LOGO_SIZE)
                .fit(Fit::Contain),
        )
        .child(Element::leaf().width(12))
        .child(
            Element::col()
                .spacing(2)
                .child(
                    Element::row()
                        .spacing(8)
                        .child(Element::label("RuPix").font_size(22.0))
                        .child(
                            Element::label(env!("CARGO_PKG_VERSION"))
                                .font_size(13.0)
                                .fg(to_color(TEXT_SECONDARY)),
                        )
                        .cross(Align::End),
                )
                .child(
                    Element::label("小巧极速的 Windows 看图器")
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT_SECONDARY)),
                ),
        )
        .cross(Align::Center);

    // 两个仓库合成一行（2026-11-05 用户口径）：名字就是链接，省掉两行「×× 仓库：URL」。
    // 位置感不由文案承担——Gitee 在前（主源）、GitHub 在后（镜像），与文档里的口径一致。
    let repo_row = Element::row()
        .spacing(6)
        .child(
            Element::label("仓库：")
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY)),
        )
        .child(Element::link("Gitee").url(HOMEPAGE).font_size(FONT_MD))
        .child(
            Element::label("|")
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY)),
        )
        .child(
            Element::link("GitHub")
                .url(GITHUB_MIRROR)
                .font_size(FONT_MD),
        )
        .cross(Align::Center);

    let info_card = Element::col()
        .spacing(6)
        .child(header)
        .child(
            Element::label("Copyright © 2026 邱宗满")
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY)),
        )
        .child(
            Element::label("许可证：AGPL-3.0")
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY)),
        )
        .child(repo_row)
        .padding(card_pad)
        .bg(card_bg)
        .corner(card_radius);

    let qr = |bytes: &'static [u8], caption: &str| {
        Element::col()
            .child(Element::label(caption).font_size(FONT_MD))
            .child(
                Element::image_svg(bytes, None)
                    .width(DONATE_QR_SIZE)
                    .height(DONATE_QR_SIZE)
                    .fit(Fit::Contain)
                    .padding(DONATE_QR_BORDER)
                    .bg(to_color([255, 255, 255])),
            )
            .cross(Align::Center)
            .spacing(8)
    };

    let donate_card = Element::col()
        .spacing(8)
        .child(Element::label("赞助").font_size(FONT_MD).font_weight(600))
        .child(
            Element::row()
                .child(qr(DONATE_WECHAT_SVG, "微信"))
                .child(Element::leaf().width(DONATE_QR_GAP))
                .child(qr(DONATE_ALIPAY_SVG, "支付宝"))
                .cross(Align::Start),
        )
        .padding(card_pad)
        .bg(card_bg)
        .corner(card_radius);

    // 「版本更新」卡：版式与交互对齐 CurSeen 的关于页（用户 2026-11-08 点名）。
    //
    // ★ 反馈**只**走这张卡：检查 / 下载的进展写在状态行，下载时「检查更新」原地让位
    //   给进度条，不再往主窗口弹 toast。按钮忙时置灰，因此不需要再拦「重复点击」。
    const PROGRESS_W: i32 = 220;

    let up = h.update;
    let check_btn = super::rows::btn("检查更新")
        .enabled_signal(up.busy.map(|b| !*b))
        .visible_when(move || !up.downloading.get())
        .on_click(|_ctx| {
            if let Err(e) = crate::core::update::request(crate::core::update::Request::Check) {
                log::warn!("发起检查更新失败：{e}");
            }
        });
    let progress = Element::progress(up.progress)
        .width(PROGRESS_W)
        .visible_when(move || up.downloading.get() && up.progress.get() >= 0.0);
    let progress_unknown = Element::progress_indeterminate()
        .width(PROGRESS_W)
        .visible_when(move || up.downloading.get() && up.progress.get() < 0.0);
    // 当前更新源：灰字只读。切换入口仍在解码器页的「来源」下拉（同一份 config）。
    let source_hint = Element::label_signal(h.download_source.map(|s| {
        let name = if *s == 0 { "Gitee" } else { "GitHub" };
        format!("更新源：{name}")
    }))
    .font_size(FONT_MD)
    .fg(to_color(TEXT_SECONDARY));

    let update_card = Element::col()
        .spacing(8)
        .child(
            Element::label("版本更新")
                .font_size(FONT_MD)
                .font_weight(600),
        )
        .child(
            Element::row()
                .child(check_btn)
                .child(progress)
                .child(progress_unknown)
                .child(Element::leaf().width(8))
                .child(source_hint)
                .cross(Align::Center),
        )
        .child(
            Element::label_signal(up.status)
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY)),
        )
        .padding(card_pad)
        .bg(card_bg)
        .corner(card_radius);

    Element::scroll()
        .fill()
        .padding_xy(16, PAGE_TOP_MARGIN)
        .child(
            Element::col()
                .fill()
                .spacing(ABOUT_CARD_GAP)
                .child(info_card)
                .child(update_card)
                .child(donate_card),
        )
}
