//! 行版式与设计令牌：设置页共用的一套「标签列 + 控件列」行、导航栏。
//!
//! ★ 2026-10-17 审计第二轮的前置拆分：`ui/settings.rs`（2699 行）大到没法逐行精读，
//!   本文件是其中**一页/一窗**的纯搬动切片——不改行为、不改文案、不改版式。
//!   父模块 `settings` 留 `Handles`（信号句柄与差分同步）、窗口装配与页序常量。
use super::*;

/// 统一的「标签 + 控件」行：左侧标签固定宽度，右侧控件左对齐，行高固定，垂直居中。
pub fn option_row(label: &str, control: Element) -> Element {
    Element::row()
        .child(
            Element::label(label)
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY))
                .width(COL1_W),
        )
        .child(control)
        .child(Element::flex_spacer())
        .cross(Align::Center)
        .height(ROW_H)
        .padding_xy(16, 0)
}

/// 统一的「标签 + 下拉」行。
pub fn choice_row(label: &str, options: Vec<String>, sig: Signal<usize>) -> Element {
    option_row(
        label,
        Element::dropdown(options, sig).width(CONTROL_GROUP_W),
    )
}

/// 统一的「标签 + 复选框」行：勾选框进右侧控件列（与下拉/输入框同一条左缘），
/// 行首只留标签——两列对齐与其它页一致（CurSeen `option_row(name,
/// checkbox("", sig))` 同款，2026-10-13 用户口径「勾选框放到右侧」）。
pub fn check_row(label: &str, sig: Signal<bool>) -> Element {
    option_row(label, Element::checkbox("", sig))
}

/// 版本格：与「可下载」列表同一宽度、同一右对齐方式。
pub fn version_cell(version: &str) -> Element {
    Element::label(format!("v{version}"))
        .font_size(FONT_MD)
        .fg(to_color(TEXT_SECONDARY))
        .width(VERSION_COL_W)
        .text_align(Align::End)
        .max_lines(1)
        .truncate(Truncate::End)
}

/// 行尾动作列的固定宽度：与「下载 / 安装」按钮的自然宽度一致（两个汉字 + Medium
/// 内边距 ≈ 60px）。两张卡的行都用它，`!` 才能落在同一条竖线上。
pub const ACTION_COL_W: i32 = 60;

/// 把行尾动作/开关放进固定宽度、右对齐的格子。
///
/// 以「可下载」的动作按钮为基准：按钮宽度不变，槽位宽度等于它；
/// 已安装的 switch 也进同宽槽位，因此两卡的「!」严格对齐。
pub fn action_slot(control: Element) -> Element {
    Element::row()
        .width(ACTION_COL_W)
        .cross(Align::Center)
        .child(Element::flex_spacer())
        .child(control)
}

/// 段间分隔线（1px 浅灰，CurSeen `card::divider` 同款）。
/// 段间分隔线（1px 浅灰，CurSeen `card::divider` 同款）。
pub fn divider() -> Element {
    Element::leaf().height(1).bg(to_color(BORDER))
}

/// 统一的「标签 + 滑块 + 数值」行。
/// windui 的 slider 固定 0..1 范围，数值映射在 sync/push 中完成。
/// 滑块宽度固定为 SLIDER_W (120px)，加上间隔 8px 和数值列 INPUT_W = 200px = CONTROL_GROUP_W。
///
/// ★ 数值列是**只读文本**而不是输入框（2026-10-17 精读时改掉）：它绑的是 `opacity_label`
///   这个显示信号，而 `refresh_labels` 每个 tick 都按配置值重写它——过去做成输入框时，
///   用户敲一个字符就在下一帧被顶回「75%」，框看着能填、实际填不进，还没有任何提交通道。
///   不透明度本来就有三条能用的路：拖滑块、右键菜单十档、快捷键 ±。
pub fn slider_row(label: &str, sig: Signal<f32>, text_sig: Signal<String>) -> Element {
    option_row(
        label,
        Element::row()
            .child(Element::slider(sig).width(SLIDER_W))
            .child(Element::leaf().width(8))
            .child(
                Element::label_signal(text_sig)
                    .font_size(FONT_MD)
                    .fg(to_color(TEXT_SECONDARY))
                    .width(INPUT_W),
            )
            .cross(Align::Center),
    )
}

/// 统一的文字按钮：全项目所有普通按钮都走这里，外观集中在一处（字号、圆角），
/// 改样式不必到处搜 `Element::button`。回调由调用方链 `.on_click(...)` 续上。
///
/// 默认走 windui 的 Medium 尺寸（左右 16 / 上下 9 内边距、accent 实心），与下拉、
/// 输入框同层级——这里只显式钉死字号与圆角，避免各调用点写法漂移。
pub fn btn(label: impl Into<TextContent>) -> Element {
    Element::button(label)
        .font_size(FONT_MD)
        .corner(CORNER_CONTROL)
}

/// 危险操作按钮（红底，卸载等不可逆动作），语义同主题 `danger()`，但经 [`btn`]
/// 统一字号/圆角，外观与其余按钮同源。
pub fn btn_danger(label: impl Into<TextContent>) -> Element {
    btn(label).danger()
}

/// 行尾动作按钮的三种外观（2026-10-26 用户口径：只留三种，别的都是多话）。
///
/// | 色调 | 文案 | 意思 |
/// |---|---|---|
/// | [`Primary`](ActionTone::Primary) | 下载（蓝、可点） | 本机能装 |
/// | [`Warning`](ActionTone::Warning) | 升级（橙、可点） | 本机是旧版，点一下换新 |
/// | [`Idle`](ActionTone::Idle) | 下载（灰、不可点） | 这一枚现在没得做 |
///
/// ★ 灰色那一档**文案也是「下载」**：用户口径是"灰的就是不能点"，不必为每种原因
///   各起一个名字（已下载 / 未发布 / 需 abi N …）。原因在「!」详情窗里写着，
///   按钮只回答"能不能点"。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionTone {
    /// 可装：主题主色（蓝）。
    Primary,
    /// 可升级：主题警告色（橙）。
    Warning,
    /// 无事可做：置灰、点不动，但**占的格子分毫不差**（见 [`btn_action`]）。
    Idle,
}

/// 行尾那颗动作按钮。
///
/// ★ **不钉宽度**（2026-10-26 用户口径「文本都控制在 2 个中文，按钮宽度就该是原来的」）：
///   静态态只有「下载」「升级」两条文案，都是两个汉字 ⇒ 按钮自己量出来的宽本来就一致，
///   再钉一个数字等于白留一圈边（原先是按「重新下载」四个字算的 88px）。等宽由"文案等长"
///   保证，不由宽度常量保证；哪天文案变长，这一列会先在这里变形，而不是悄悄错位。
///
/// ★ 下载中传进来的是**动态文案**（`Signal<String>`，`42%`）：`TextContent::Bound`
///   每帧现取、连宽重测，所以下载中的按钮会比「下载」略宽一点；这是可接受的——
///   行尾仍然是一颗按钮，右缘不塌。
///
/// ★ [`ActionTone::Idle`] 走的是 `disabled(true)` 而**不是**换成一句纯灰文字
///   （2026-10-26 用户口径）：纯文字不占按钮那么宽，那一行右端于是凹进去一块，整列
///   对不齐。禁用态由核心层统一管（拦事件 + 置灰底 + `text_disabled` 文字），
///   宽度与启用态分毫不差。也别拿 `visible_when` 去替它——隐藏不占位，一样会塌。
pub fn btn_action(label: impl Into<TextContent>, tone: ActionTone) -> Element {
    let b = btn(label);
    match tone {
        ActionTone::Primary => b,
        // 用主题的 `Intent::Warning` 而不是自造色值：换肤跟着走，也不给 `ui/` 添第二处字面色。
        ActionTone::Warning => b.intent(Intent::Warning),
        ActionTone::Idle => b.disabled(true),
    }
}

/// 可折叠卡片：**标题色带 + 折叠箭头 + 正文**，点标题行整行可点（CurSeen
/// `FeatureCard` 的同款简化版）。
///
/// 与 CurSeen 的差别：那边的标题行还挂着「功能开关 / 标题下拉」，设置页这三张卡都没有——
/// 不留假开关（CurSeen 自己也是这个口径）。差别之二是**整行可点**：curseen 只让名称与
/// 箭头可点，因为它的标题行里真的有复选框与下拉，而 windui 的 `Clickable` 会吃掉
/// `Down`/`Up`（冒泡在它那里断掉）——整行可点会让那些控件全部点不动。
/// 本卡的标题行里没有任何控件，整行可点是更大更好点的命中区；
/// ★ 哪天要给标题行加控件，就得把 `clickable` 从 `row` 挪回「名称 + 箭头」两处。
///
/// 正文用 [`Element::visible_when`] 直接挂在展开信号上：折叠时不占高度（不是"隐藏但仍占位"），
/// 三张卡因此能各按自己的内容长短堆在一起，整页由外层 `scroll` 统一滚。
/// 标题条与正文之间留 `GAP` 一格（与页四周、卡↔卡同一节奏——不留的话，正文首行
/// 那颗按钮就贴在色带下缘上了）。
///
/// ★ 正文**不额外缩进**：卡片标题文字从 `padding_xy(16, 0)` 起算，正文里的行
/// （`option_row` / 解码器行）自己也带 16，两者天然同一条左对齐线，再加缩进就错位了
/// （CurSeen 那 24px 缩进是给它标题行里的开关留的，我们没有开关）。
pub fn section_card(name: &str, expanded: Signal<bool>, body: Element) -> Element {
    let toggle = move |_ctx: &mut EventCtx| expanded.set(!expanded.get());
    // 折叠箭头：展开 ▼ / 收起 ▶。对齐写在**箭头自己**身上（stack 上的 align 对子元素无效，
    // 写错地方箭头会落在命中区左上角，看着"偏高"）。
    let chevron = Element::label_signal(expanded.map(|e| {
        if *e {
            "\u{25BC}".to_string()
        } else {
            "\u{25B6}".to_string()
        }
    }))
    .font_size(FONT_MD)
    .fg(to_color(TEXT_SECONDARY))
    .align(Align::Center);
    Element::col()
        .width_match()
        .child(
            Element::row()
                .child(Element::label(name).font_size(FONT_MD).fg(to_color(TEXT)))
                .child(Element::flex_spacer())
                .child(Element::stack().child(chevron).width(28).height(28))
                .cross(Align::Center)
                .height(CARD_HEADER_H)
                .padding_xy(16, 0)
                .bg(to_color(CARD_HEADER_BG))
                .corner(CARD_CORNER)
                .clickable()
                .on_click(toggle),
        )
        .child(
            body.visible_when(move || expanded.get())
                .padding_edges(Insets {
                    left: 0,
                    top: GAP,
                    right: 0,
                    bottom: 0,
                }),
        )
}

/// 详情窗的一行：**窄标签列 + 自然折行的值**。
///
/// 标签列只 92px（远窄于设置页 `COL1_W` = 208），值列吃满剩余宽度并且不设
/// `max_lines/truncate`——长路径、多扩展名、长说明都要能整段读完。
/// `.cross(Align::Start)` 让值折成多行时标签仍贴在首行左侧。
///
/// 两个详情窗（已安装的「!」与可下载的「!」）共用这一份，省得各写一遍版式再走散。
pub fn info_row(label: &'static str, value: String) -> Element {
    Element::row()
        .fill()
        .cross(Align::Start)
        .spacing(12)
        .padding_xy(0, 6)
        .child(
            Element::label(label)
                .font_size(FONT_MD)
                .fg(to_color(TEXT_SECONDARY))
                .width(92),
        )
        .child(
            Element::label(value)
                .font_size(FONT_MD)
                .fg(to_color(TEXT))
                .weight(1.0),
        )
}

/// 导航 tab 切换回调：（事件上下文, 目标 tab 序号）。
pub type OnSwitch = Rc<dyn Fn(&mut EventCtx, usize)>;

/// 左侧导航栏。
pub fn nav_rail(tab: Signal<usize>, on_switch: OnSwitch) -> Element {
    const NAV_W: i32 = 88;
    const NAV_BG: [u8; 3] = [246, 248, 250];
    let accent = to_color(ACCENT);

    let mut col = Element::col()
        .width(NAV_W)
        .height_match()
        .bg(to_color(NAV_BG));

    // ★「解码器」= 原「解码器」与「解码」两页合并（2026-10-25）：解码器的生命周期与
    //   后缀归属本来就是一枚硬币的两面，分页只会让人来回切。
    const NAV_LABELS: [&str; 5] = ["选项", "快捷键", "解码器", "插件", "关于"];
    for (idx, label) in NAV_LABELS.iter().enumerate() {
        let selected = tab.map(move |t| *t == idx);
        col = col.child(
            Element::row()
                .cross(Align::Center)
                .height(40)
                .child(
                    Element::leaf()
                        .width(3)
                        .height(18)
                        .bg(accent)
                        .visible_signal(selected),
                )
                .child(
                    Element::label(*label)
                        .font_size(FONT_MD)
                        .fg(to_color(TEXT))
                        .padding_xy(14, 0),
                )
                .child(Element::flex_spacer())
                .clickable()
                .on_click({
                    let on_switch = on_switch.clone();
                    move |ctx| {
                        tab.set(idx);
                        on_switch(ctx, idx);
                    }
                }),
        );
    }
    col.child(Element::flex_spacer())
}

/// 设置页的**标准搜索框**：放大镜 + 过滤输入框，横向撑满所在行。
///
/// ★ 为什么外面要包一层 `col().fill()`：windui 的**水平容器主轴上的 `Match` 会被降级
/// 成「按内容宽」**（`measure_linear`：单个子独占整条主轴不是想要的效果，故降级）。
/// 直接把 `text_input(...).width_match()` 挂进 `row`，量到的是占位文字的固有宽
/// （2026-10-26 实测「搜索名称或后缀」= 160px），两行搜索框于是**一宽一窄**。
/// 包进竖形容器后，横向是交叉轴，`Match` 正常撑满；外层 `.fill()` 让这层 col 在
/// `row` 里吃掉全部剩余宽，输入框再 `width_match()` 跟上——全页搜索框同一只宽。
///
/// 放大镜直接用 Segoe MDL2 的 Search 字形 **U+E71E**，普通绘制、不镜像、不自绘——
/// 与参考项目 CurSeen 的官方资源页搜索框同一写法（`.leading_icon('\u{E71E}')`）。
///
/// 为什么不再是"矢量圆环 + 手柄"自绘（2026-10-29 用户口径撤掉）：那条路是给
/// `U+E721` 打的补丁——该字形原生「圆圈在右、手柄朝左下」，先经镜像绘制（补丁 09），
/// 又因"镜像字形在焦点重绘时擦出图标区背景块"改成宿主侧矢量绘制。换成朝向本来就对的
/// `U+E71E` 后，这两个补丁都不需要了：**少一处第三方改动、少一段自绘代码**，观感与参考一致。
/// 码位覆盖已直读字体 cmap 核对（`segmdl2.ttf` glyph 8248），仍靠启动时
/// `register_private_use_font` 注册的那份兜底（见 `runtime::run`）。
pub fn search_box(filter: Signal<String>, placeholder: impl Into<String>) -> Element {
    Element::col().fill().child(
        Element::text_input(filter, placeholder.into())
            .leading_icon('\u{E71E}')
            .width_match(),
    )
}
