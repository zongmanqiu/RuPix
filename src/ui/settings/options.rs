//! 「选项」页：四张可折叠卡片 —— 外观 / 窗口 / 看图与操作 / 系统与网络。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
//!
//! ★ 2026-11-04（m224）：原先 15 行平铺，是设置五页里唯一没有卡片的一页。现按
//!   「用户想调这件事的时候会去哪找」分成四组，卡片与快捷键 / 解码器 / 插件三页同款
//!   （[`section_card`]：标题色带 + 折叠箭头 + 正文）。**只重排，不增删任何一项**，
//!   ini 键、读写与即时生效时机一个都没动；四张卡默认全展开（展开态是会话态、不落盘）。
//!   组序从「看得见」到「看不见」：外观 → 窗口 → 看图与操作 → 系统与网络。
use super::*;

/// Options 页：四张卡按内容长短堆叠，整页只有最外层这一条滚动条。
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
                .spacing(GAP)
                .child(section_card("外观", h.card_appearance, appearance_body(h)))
                .child(section_card("窗口", h.card_window, window_body(h)))
                .child(section_card("看图与操作", h.card_browse, browse_body(h)))
                .child(section_card("系统与网络", h.card_system, system_body(h))),
        )
}

/// 「外观」：颜色与可见元素长什么样。四项全是看图窗口的观感，不改图片像素。
fn appearance_body(h: &Handles) -> Element {
    Element::col()
        .fill()
        .spacing(0)
        // ini 键：ui_theme（0 深色 / 1 浅色）。**只影响看图窗口**——浮层胶囊、
        // 缩略图条、棋盘格、空窗底与中央提示文字；设置页、属性窗、对话框
        // 永远是浅底深字那一套（2026-11-02 用户口径）。标签 m223 起只写「主题色」，
        // 作用域没跟着变。换下拉当场生效：config 写回后 runtime 下一帧换令牌并重绘。
        .child(option_row(
            "主题色",
            Element::dropdown(
                UiTheme::ALL.iter().map(|t| t.label().to_string()).collect(),
                h.ui_theme,
            )
            .width(CONTROL_GROUP_W),
        ))
        // ini 键：opacity（0.2 ~ 1.0）。滑块 + 只读数值列，另有右键菜单十档与快捷键 ±
        // 两条入口；数值列不做输入框（刷新时会把用户敲到一半的值顶回去）。
        .child(slider_row("窗口不透明度", h.opacity, h.opacity_label))
        // ini 键：minimal_mode（即时生效：runtime 差分推给当前窗口，m51-D）
        .child(check_row("极简模式", h.minimal_mode))
        // ini 键：checker（即时生效：runtime 每帧把配置推给当前窗口，m53-L）
        .child(check_row("棋盘格", h.checker))
}

/// 「窗口」：窗口／进程以什么方式存在——z 序、初始尺寸、关窗去向、能否开第二扇。
///
/// 「启动窗口」与「窗口尺寸」必须同组：一个选档、一个填具体宽高，拆开过一次就分不清
/// 「大小 / 尺寸」（m168 那轮同改两处标签正是为这个）。
fn window_body(h: &Handles) -> Element {
    Element::col()
        .fill()
        .spacing(0)
        // ini 键：stay_on_top（改勾选当场翻转当前窗口的置顶）
        .child(check_row("窗口置顶", h.stay))
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
        // ini 键：close_to_tray（勾上即生效：托盘图标当场出现、关窗转为隐藏）
        .child(check_row("最小化到托盘", h.to_tray))
        // ini 键：allow_multiple（多开）。勾上后再双击图片，在同一进程里新开
        // 一扇窗口——进程与托盘始终只有一个。
        // 关闭时维持单窗口：双击另一张图会把 argv 转发到已有窗口、就地换图。
        .child(check_row("允许多开", h.allow_multiple))
}

/// 「看图与操作」：图怎么被看、手怎么动。
fn browse_body(h: &Handles) -> Element {
    Element::col()
        .fill()
        .spacing(0)
        .child(check_row("适应长图", h.long_fit))
        // ini 键：drag_image（默认勾上）。勾着 = 画布里按住左键只平移图片、窗口不动，
        // 移动窗口交给**顶部标题栏**那一条；图比窗口小也拖得动，图边还能越过窗口边
        // （永远留 24dp 在画面里，图拖不丢）。
        // 取消勾选 = 回到旧分工：任意位置左键拖都拖窗口，平移改用 Ctrl+左键 / 中键 / 鸟瞰图。
        // 名字只说"左键"：中键与 Ctrl+左键一直能拖图，这个开关管的确实是左键那一路。
        // 同一个开关也在右键菜单的开关区（「极简模式 / 缩略图条」那一组）。
        .child(check_row("左键拖图", h.drag_image))
        // ini 键：space_preview（全局空格监听，低级钩子、从不吞键，作用于整个
        // 系统——与快捷键页的看图窗口内键位不同源；16ms 轮询差分装/摘钩，见
        // platform::spacehook）。★ 这四项里唯一越界的：它不只管看图窗口，
        // 归本组是取「想找它的时候会去哪」的最大公约数。
        .child(check_row("空格预览", h.space_preview))
        // ini 键：slide_interval（幻灯片每张停留秒数。整数输入框而不是四档下拉——
        // 秒数本来就是一条整数轴，压成档位反而给不出 4 秒、8 秒这种要的值。
        // 越界由 `clamp_slide_interval` 夹进 1..=600，打到一半的中间态不改配置）
        .child(option_row(
            "幻灯片间隔 (秒)",
            Element::text_input(h.slide_secs, "3").width(CONTROL_GROUP_W),
        ))
}

/// 「系统与网络」：都不改画面，改的是后台怎么跑、从哪拉东西。只有两项，
/// 不另开第五张卡——一张卡只装一行，标题比内容还抢眼，折叠反倒多一次点击。
fn system_body(h: &Handles) -> Element {
    Element::col()
        .fill()
        .spacing(0)
        // ini 键：prefetch（默认勾上）。开着就是"按下方向键立刻换图"：
        // 后台先解好同文件夹前后各两张的**视口档**，与图片本身多大无关；
        // 最坏驻留由 `viewer` 的份数闸 + 256 MiB 字节闸封顶。关掉它，
        // 每次翻页都要现场解码（实测大图 130–260 ms，慢格式可到 1 s 以上）。
        .child(check_row("邻图预读", h.prefetch))
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
}
