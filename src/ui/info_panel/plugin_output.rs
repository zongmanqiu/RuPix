//! 插件输出卡：把协议 v2 的 `outputs` 变成信息面板里的动态卡。
//!
//! 一张卡 = 一次插件运行；卡内按插件给的顺序堆叠文本 / 表格 / 图片。
//! 卡是**会话级**的：换图清空，不写 `info_cards`——上一张图的报告不能冒充当前图。

use std::io::Cursor;
use std::rc::Rc;
use std::sync::Arc;

use windui::core::{EventCtx, Widget};
use windui::geometry::{Rect, Size};
use windui::prelude::*;
use windui::render::{Canvas, VisualState};
use windui::style::Style;
use windui::text::TextEngine;
use windui::ui::ImageContent;

use super::{chrome, tokens::PanelColors};
use crate::core::ext_protocol::OutputBlock;

/// 面板里的一张插件输出卡。
#[derive(Clone)]
pub struct PluginCard {
    /// 同一插件重跑替换同一张卡。
    pub key: String,
    pub title: String,
    /// 卡展开还是收起（会话级，不落配置；换图随卡一起清）。
    pub expanded: bool,
    pub blocks: Vec<PluginBlock>,
}

/// 卡内的一块输出。
#[derive(Clone)]
pub enum PluginBlock {
    Text {
        title: Option<String>,
        text: String,
    },
    Table {
        title: Option<String>,
        rows: Vec<(String, String)>,
    },
    Image {
        title: Option<String>,
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
    },
}

/// 协议输出 → 面板卡；图片在这里解码成 RGBA，后续重建卡不再重复解 PNG。
pub fn card_from_outputs(
    plugin_id: &str,
    plugin_name: &str,
    outputs: Vec<OutputBlock>,
) -> Result<PluginCard, String> {
    let mut blocks = Vec::with_capacity(outputs.len());
    for output in outputs {
        match output {
            OutputBlock::Text { title, text } => blocks.push(PluginBlock::Text { title, text }),
            OutputBlock::Table { title, rows } => blocks.push(PluginBlock::Table { title, rows }),
            OutputBlock::Image {
                title,
                format: _,
                base64,
            } => {
                let png = crate::core::b64::decode(&base64)
                    .map_err(|e| format!("插件输出图片 base64 解不开：{e}"))?;
                let (width, height, rgba) = decode_png_rgba(&png)?;
                blocks.push(PluginBlock::Image {
                    title,
                    width,
                    height,
                    rgba: Arc::new(rgba),
                });
            }
        }
    }
    if blocks.is_empty() {
        return Err("插件没有返回可显示的输出".into());
    }
    Ok(PluginCard {
        key: plugin_id.to_string(),
        title: plugin_name.to_string(),
        expanded: true,
        blocks,
    })
}

/// 面板里的动态插件卡列。放在内置卡上方；数据信号变化时由 `host_signal` 重建。
pub fn cards_element(chrome: &Rc<chrome::Chrome>, colors: PanelColors) -> Element {
    let data = chrome.info.plugin_cards;
    let build_chrome = chrome.clone();
    Element::host_signal(data, move |card| {
        card_element(card, build_chrome.clone(), colors)
    })
}

/// 面板不可用（无边框 / 窗太窄）时的文本回退。
pub fn summary_text(card: &PluginCard) -> String {
    let mut out = String::new();
    for block in &card.blocks {
        match block {
            PluginBlock::Text { title, text } => {
                if let Some(t) = title {
                    out.push_str(t);
                    out.push('\n');
                }
                out.push_str(text);
                out.push_str("\n\n");
            }
            PluginBlock::Table { title, rows } => {
                if let Some(t) = title {
                    out.push_str(t);
                    out.push('\n');
                }
                for (k, v) in rows {
                    out.push_str(k);
                    out.push_str(": ");
                    out.push_str(v);
                    out.push('\n');
                }
                out.push('\n');
            }
            PluginBlock::Image { title, .. } => {
                out.push_str("［图片输出：");
                out.push_str(title.as_deref().unwrap_or("未命名"));
                out.push_str("］\n\n");
            }
        }
    }
    out.trim_end().to_string()
}

fn card_element(card: PluginCard, chrome: Rc<chrome::Chrome>, colors: PanelColors) -> Element {
    let card_for_copy = card.clone();
    let key = card.key.clone();
    let close = {
        let chrome = chrome.clone();
        let key = key.clone();
        move |ctx: &mut EventCtx| {
            chrome.info.remove_plugin_card(&key);
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };
    let toggle = {
        let chrome = chrome.clone();
        let key = key.clone();
        move |ctx: &mut EventCtx| {
            chrome.info.toggle_plugin_card(&key);
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };

    // 标题行点击折叠：与内置卡同一手势（▾ 展开 / ▸ 收起），状态只在本会话。
    let title_row = Element::row()
        .cross(Align::Center)
        .spacing(4)
        .clickable()
        .on_click(toggle)
        .child(
            Element::label(if card.expanded { "▾" } else { "▸" })
                .font_size(12.0)
                .fg(colors.text),
        )
        .child(
            Element::label(card.title)
                .font_size(13.0)
                .font_weight(600)
                .fg(colors.text)
                .max_lines(1)
                .truncate(Truncate::End),
        );

    let head = Element::row()
        .width_match()
        .height(24)
        .cross(Align::Center)
        .spacing(6)
        .child(title_row)
        .child(Element::flex_spacer())
        .child(
            Element::icon_button("✕")
                .size(22, 22)
                .font_size(11.0)
                .fg(colors.muted)
                .tooltip("关闭插件输出")
                .on_click(close),
        );

    let body = if card.expanded {
        // 同一张卡里的所有表共用一个行头列宽：摘要表与报告表的值列起点对齐。
        let table_label_w = card_table_label_width(&card.blocks);
        let mut body = Element::col()
            .width_match()
            .bg(colors.content)
            .corner(6.0)
            .padding(8)
            .spacing(6);
        for block in card.blocks {
            body = body.child(block_element(block, table_label_w, colors));
        }
        let copy = move || {
            let text = summary_text(&card_for_copy);
            vec![MenuItem::run(
                "复制全部文本",
                move |ctx: &mut EventCtx| match crate::platform::clipboard::copy_text(&text) {
                    Ok(()) => ctx.toast_ok("已复制到剪贴板"),
                    Err(e) => ctx.toast_err(format!("复制失败：{e}")),
                },
                false,
            )]
        };
        body.on_context_menu(copy)
    } else {
        Element::col().width_match()
    };

    Element::col()
        .width_match()
        .bg(colors.card)
        .corner(8.0)
        .border(colors.edge, 1)
        .padding(8)
        .spacing(6)
        .child(head)
        .child(body)
}

/// 文本块高度估算：面板宽不定，按每行约 48 个半角单位折行，钳在 60–320 px。
///
/// 估多了只是底部留白；估少了是文本输入框内部滚动——两者都比在面板里
/// 直接铺一条三千像素的长条好。
fn text_block_height(text: &str) -> i32 {
    let mut lines = 0i32;
    for line in text.lines() {
        let width: i32 = line
            .chars()
            .map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 })
            .sum();
        lines += (width / 48).max(1);
    }
    (lines * 17 + 10).clamp(60, 320)
}

fn block_element(block: PluginBlock, table_label_w: i32, colors: PanelColors) -> Element {
    match block {
        PluginBlock::Text { title, text } => {
            let mut col = Element::col().width_match().spacing(2);
            if let Some(t) = title {
                col = col.child(block_title(t, colors));
            }
            // 只读多行输入框：鼠标拖选 / Ctrl+A / Ctrl+C / 右键「复制」都能用。
            let h = text_block_height(&text);
            let sig = signal(text);
            col.child(
                Element::text_input(sig, "")
                    .multiline()
                    .wrap(true)
                    .read_only()
                    .frameless_input()
                    .font_size(12.0)
                    .fg(colors.text)
                    .width_match()
                    .height(h),
            )
        }
        PluginBlock::Table { title, rows } => {
            let mut col = Element::col().width_match().spacing(2);
            if let Some(t) = title {
                col = col.child(block_title(t, colors));
            }
            let label_w = table_label_w;
            for (k, v) in rows {
                col = col.child(table_row(k, v, label_w, colors));
            }
            col
        }
        PluginBlock::Image {
            title,
            width,
            height,
            rgba,
        } => {
            let mut col = Element::col().width_match().spacing(2);
            if let Some(t) = title {
                col = col.child(block_title(t, colors));
            }
            let menu_rgba = rgba.clone();
            let menu = move || {
                let rgba = menu_rgba.clone();
                vec![MenuItem::run(
                    "复制图片",
                    move |ctx: &mut EventCtx| match crate::platform::clipboard::copy_png(
                        width,
                        height,
                        &rgba[..],
                    ) {
                        Ok(()) => ctx.toast_ok("图片已复制到剪贴板"),
                        Err(e) => ctx.toast_err(format!("复制图片失败：{e}")),
                    },
                    false,
                )]
            };
            col.child(
                Element::leaf()
                    .width_match()
                    .widget(PluginImageBlock::new(width, height, rgba))
                    .on_context_menu(menu),
            )
        }
    }
}

fn block_title(text: String, colors: PanelColors) -> Element {
    Element::label(text)
        .font_size(12.0)
        .font_weight(600)
        .fg(colors.text)
        .max_lines(2)
}

/// 整张卡共用的行头列宽（px）：按所有表格里最长的行头算，夹在 48–104。
///
/// 13 px 字号下一个汉字约 13 px；固定 64 放 5 字（如「三通道均值」）会挤，
/// 而摘要表只有 4 字时 64 又浪费。整卡算一次，两张表的值列才会对齐。
fn card_table_label_width(blocks: &[PluginBlock]) -> i32 {
    blocks
        .iter()
        .filter_map(|b| match b {
            PluginBlock::Table { rows, .. } => Some(rows),
            _ => None,
        })
        .flat_map(|rows| rows.iter())
        .map(|(k, _)| k.chars().count())
        .max()
        .map(|n| ((n as i32) * 13 + 6).clamp(48, 104))
        .unwrap_or(70)
}

fn table_row(label: String, value: String, label_w: i32, colors: PanelColors) -> Element {
    Element::row()
        .width_match()
        // 行头与内容顶部对齐：多行内容从第一行起就与行头同高，
        // 不会出现行头悬在整段内容中间的情况。
        .cross(Align::Start)
        .spacing(6)
        .child(
            Element::label(label)
                .width(label_w)
                .font_size(13.0)
                .font_family(super::PANEL_FONT)
                // 行头与值同色同字号：浅灰行头在浅背景上会因对比度显得更小，
                // 放大后像“两列字不一样高”（2026-11-14 用户反馈）。列位本身已能区分。
                .fg(colors.text),
        )
        // ★ 右列不能直接挂 `weight`：windui 的 weight 子节点会走父级的字号解析，
        //   `.font_size(13.0)` 在真实渲染里被面板默认字号盖掉（左右字高差约 1.3 倍，
        //   2026-11-14 真机复现）。用一个 weight 容器包住 label，字号才是 label 自己的。
        .child(
            Element::col()
                .weight(1.0)
                // 字号放容器上：label 直接挂 font_size 在 weight/width_match 路径会被
                // 父级样式盖掉（2026-11-14 真机复现，22 px 实测不生效）。
                .font_size(13.0)
                .child(
                    Element::label(value)
                        .width_match()
                        .font_size(13.0)
                        .font_family(super::PANEL_FONT)
                        .fg(colors.text),
                ),
        )
}

/// 等比缩放的图片块：宽度跟面板，高度按原图比例算，避免图被拉扁。
struct PluginImageBlock {
    content: ImageContent,
    src_w: u32,
    src_h: u32,
}

impl PluginImageBlock {
    fn new(width: u32, height: u32, rgba: Arc<Vec<u8>>) -> Self {
        let content = ImageContent::from_rgba(width, height, &rgba[..]);
        Self {
            content,
            src_w: width.max(1),
            src_h: height.max(1),
        }
    }
}

impl Widget for PluginImageBlock {
    fn measure(&self, avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        let w = avail.w.max(1);
        let h = ((w as f32 * self.src_h as f32 / self.src_w as f32).round() as i32).clamp(80, 480);
        Size::new(w, h)
    }

    fn paint(
        &self,
        bounds: Rect,
        _content: Rect,
        _focused: bool,
        _enabled: bool,
        canvas: &mut dyn Canvas,
        style: &Style,
    ) {
        self.content
            .paint_into(bounds, canvas, style, VisualState::Normal);
    }
}

/// PNG → 非预乘 RGBA8。面板输出图只接受 8 bit；16 bit 图让插件先降位。
fn decode_png_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("PNG 解头失败：{e}"))?;
    let (w, h) = {
        let info = reader.info();
        if info.bit_depth != png::BitDepth::Eight {
            return Err(format!(
                "面板输出图只接受 8 bit PNG（收到 {:?}）",
                info.bit_depth
            ));
        }
        (info.width, info.height)
    };
    if w == 0 || h == 0 || w as u64 * h as u64 > 16_000_000 {
        return Err(format!("面板输出图尺寸不合法：{w}×{h}"));
    }
    let buf_len = reader
        .output_buffer_size()
        .ok_or_else(|| "PNG 输出缓冲大小未知".to_string())?;
    let mut buf = vec![0u8; buf_len];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG 解码失败：{e}"))?;
    let src = &buf[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => src.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(w as usize * h as usize * 4);
            for px in src.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(w as usize * h as usize * 4);
            for g in src {
                out.extend_from_slice(&[*g, *g, *g, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(w as usize * h as usize * 4);
            for px in src.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            out
        }
        other => return Err(format!("PNG 颜色类型不支持：{other:?}")),
    };
    Ok((w, h, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_table_cards_do_not_need_an_image() {
        let card = card_from_outputs(
            "levels",
            "色阶分析",
            vec![
                OutputBlock::Text {
                    title: Some("报告".into()),
                    text: "hello".into(),
                },
                OutputBlock::Table {
                    title: Some("摘要".into()),
                    rows: vec![("均值".into(), "128".into())],
                },
            ],
        )
        .expect("card");
        assert_eq!(card.key, "levels");
        assert_eq!(card.blocks.len(), 2);
        assert!(summary_text(&card).contains("均值: 128"));
    }

    #[test]
    fn table_label_width_is_shared_by_all_blocks_in_a_card() {
        let summary = PluginBlock::Table {
            title: None,
            rows: vec![
                ("均值".into(), "140".into()),
                ("高光溢出".into(), "22.7%".into()),
            ],
        };
        assert_eq!(card_table_label_width(&[summary]), 58, "4 字行头 → 58px");

        let report = PluginBlock::Table {
            title: None,
            rows: vec![("三通道均值".into(), "R/G/B".into())],
        };
        assert_eq!(card_table_label_width(&[report]), 71, "5 字行头 → 71px");

        let both = [
            PluginBlock::Table {
                title: None,
                rows: vec![("均值".into(), "140".into())],
            },
            PluginBlock::Table {
                title: None,
                rows: vec![("三通道均值".into(), "R/G/B".into())],
            },
        ];
        assert_eq!(
            card_table_label_width(&both),
            71,
            "同一张卡里两张表取最长行头，值列才对得齐"
        );
        assert_eq!(card_table_label_width(&[]), 70, "无表时回退到旧列宽");
    }

    #[test]
    fn a_one_pixel_png_decodes_to_rgba() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("header");
            writer.write_image_data(&[0, 0, 0, 0]).expect("pixels");
        }
        let (w, h, rgba) = decode_png_rgba(&png).expect("decode");
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, vec![0, 0, 0, 0]);
    }
}
