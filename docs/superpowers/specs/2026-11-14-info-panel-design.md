# 设计：右侧信息面板（信息输出坞）

日期：2026-11-14 · 状态：已实现（m342）

## 1. 需求与边界

右侧信息面板是一个**信息输出坞**，不是默认仪表盘：

- 面板默认可以打开，但里面是空的；只有用户主动打开某个信息功能，才往里面加卡片。
- 第一版只接一个功能：图片「属性」。
- 「属性」不再弹窗：右键「文件 ▸ 属性」或属性快捷键只负责“打开面板 + 加卡 + 展开”。
- 卡片可展开/折叠，标题行右侧有独立的关闭按钮；关闭即从面板移除，再点「属性」重新加回。
- 属性卡只读，随切图 / 翻页 / 换层 / 解码完成自动更新。
- 面板开/关、宽度、当前卡片、卡片折叠状态跨重启保留。
- 面板是右侧 dock，挤压图区；高度铺满内容区；宽度可拖，范围 240–4096 px，**实际上限 = 窗口内容宽的一半**。
- 窗口太窄时自动收起；无边框形态隐藏；锁定形态保留面板但整体禁用。
- 原有插件结果窗、设置窗、旋转窗等弹窗保持不变；插件输出如何进面板留到第二阶段。
  ★ 第二阶段已实现（2026-11-14，见 §15）：协议 v2 的 `outputs` 进面板，一次运行一张卡。

第一版明确不做：插件卡片与面板协议、直方图/色阶卡、卡片拖拽排序、卡片内控件、浮动/拆分面板、属性卡里的旋转/缩放/滤镜操作项。

## 2. 方案选型

采用 **Element 树 dock**：

- 复用 windui 的 `Element::row/col/label/label_signal/icon_button/clickable/scroll/window_button`，以及 `SplitHandle` 的分隔条行为。
- 根布局从“画布铺满整窗”改为“画布 + 可隐藏分隔条 + 面板”的横向线性容器。
- 面板的显示/隐藏、卡片显示/隐藏全部由 `Cell`/`Signal` 驱动 `visible_when`，不在运行期重建整棵树。

不直接使用 `Element::split` 的原因：

- 它把分隔条写死在容器内部，面板收起时无法把分隔条一起隐藏。
- `SplitOpts` 的 min/max 是构建期静态比例，无法按“240–4096 px / 窗口半宽”钳制。

因此手动组成 `row + SplitHandle + panel`，并在 `SplitHandle` 外面套一层只做像素钳制的适配器。

不直接使用 `Element::card` / `Element::collapsible` 的原因：

- 这两个组合件的底色、标题色、hover 色来自 windui 的全局主题；windui 主题是浅色，而主窗是深/浅两套跟随 `ui_theme` 的主题。
- 若把 windui 主题切暗，设置页会跟着变暗，违反“设置页始终浅色”的既有口径。
- 卡片外壳改用 `row/col` + 主窗 `tokens` 组合，折叠交互由 `Element::clickable` 承担（hover/press、键盘 Enter/Space、焦点环都由库控件提供）。

## 3. 模块与文件

- `ui/info_panel/mod.rs`：面板状态、可见性、宽度换算、根布局装配、卡片外壳、属性卡。
- `ui/info_panel/properties.rs`：`MetaSignals`、属性刷新、时间/大小/文件名格式化、APNG 判定。
- `ui/window.rs`：`build_root` 改为“画布 + 分隔条 + 面板”的横向容器；保留整窗拖放。
- `ui/chrome.rs`：`Chrome` 增加 `info: InfoPanel`；窗口五颗标题钮始终留在画布顶条（面板头部不再承担窗口三键）。
- `ui/canvas.rs`：右键「属性」与「界面 ▸ 信息面板」入口；标题栏几何/命中/绘制保持五颗一组。
- `ui/mod.rs`：声明信息面板模块。
- `core/config.rs`：四个新键、默认值、解析、写盘、`ALL_KEYS`、首启模板、消费方测试。
- `runtime/mod.rs`：建窗时灌入面板配置；每窗 tick 更新面板宽度/可见性；属性刷新改为按“面板可见 + 卡片 active”门控。
- `ui/settings/mod.rs`：移除属性窗模块的声明与再导出。
- 属性窗模块的数据部分迁入属性数据模块；窗口装配、单例键、`open_metadata` 入口退役。

## 4. 根布局

```text
stack.fill()                                  // 保留 on_drop_files 整窗拖放
└ row.fill().cross(Stretch)
  ├ canvas.weight_when(|| info.ratio.get())
  ├ PanelSplitHandle.width(1).height_match()
  │    .visible_when(|| info.visible.get())
  │    .enabled_when(|| !chrome.locked.get())
  └ panel.weight_when(|| 1.0 - info.ratio.get())
       .visible_when(|| info.visible.get())
       .enabled_when(|| !chrome.locked.get())
```

说明：

- `info.ratio` 是**画布占比**，与 `SplitHandle` 的 `ratio_at` 语义一致。
- 面板隐藏时，handle 与 panel 都不可见；线性容器只剩画布一个可见权重子节点，画布自动占满整行。这里的 `panel` 是 §5.5 的两份主题树外壳（`stack`），两份共用同一组信号。
- 面板与 handle 都受 `locked` 门控；锁定后拖宽、滚卡片、点关闭全部失效。画布本身仍由既有锁定逻辑处理，锁钮仍可解锁。
- 根 `stack` 的整窗拖放保持不变；文件拖到面板区域时，事件沿父链冒泡到根，仍按主图打开。

## 5. 面板与卡片

### 5.1 面板骨架

```text
panel: col.fill().bg(colors.panel)
├ header.height(STRIP_H).bg(colors.header)
│  ├ label("信息").weight(1).max_lines(1).fg(colors.text)
│  └ icon_button("\u{E76B}").size(40,40)
│       .font_family("Segoe MDL2 Assets").font_size(14.0)
│       .fg(colors.text).tooltip("收起信息面板")
└ scroll.weight(1).width_match().padding(8)
   └ col.width_match().spacing(8)
      ├ label("右键「属性」或「本图」添加输出").fg(colors.muted)  // 无卡片时
      └ property card                                  // active 时
```

- 面板背景用主窗令牌 `colors.panel`（深/浅两套），与左侧文件树同为“侧 dock 地面”。
- `colors` 是 `tokens` 暴露的 `PanelColors`（`panel/header/card/content/text/muted/edge`），不直接在每个节点里读 `tk::*()`；原因见 §5.5。
- 面板头部**和主窗标题栏同一档**：`header = pill_bg`、13px 标题、40px 关闭按钮；只是没有窗口三键。
- 关闭按钮字形用 MDL2 `ChevronLeft`（`\u{E76B}`，视觉即 `<`），字体族与主窗标题栏同为 `Segoe MDL2 Assets`、字号同为 14px；语义是「收起」而不是「关窗」。
- 面板头部是**面板自己的标题**，不是窗口标题栏：不拖窗、不放最小化/最大化/关闭；窗口五颗钮始终连在画布顶条。
- 面板头部高度与画布标题栏同为 `STRIP_H`（40）。\n- 垂直滚动条：滚动节点向右多占 3px（负 margin）、右 padding 同步 +3，track 右缘 = 面板右缘 − 8px，与左侧文件树 / 缩略图条落在同一条内边线；最外 8px 是窗口缩放手势带，不能压（压上去 `WM_NCHITTEST` 会判成拖窗边，滚动条点不到）。
- 关闭按钮用 `<`（收起），tooltip「收起信息面板」；收起后卡片状态保留，再点「属性」或「界面 ▸ 信息面板」恢复。

### 5.2 卡片外壳

```text
card: col.width_match()
        .bg(colors.card).corner(8).border(colors.edge, 1)
        .padding(8).spacing(6)
        .visible_signal(active)
├ row.cross(Center).width_match().height(24)
│  ├ row.cross(Center).spacing(4)
│  │    .clickable().on_click(toggle)          // 只包箭头+标题，不占满整行
│  │  ├ label("▾").visible_signal(expanded)
│  │  ├ label("▸").visible_when(move || !expanded.get())
│  │  └ label("属性").max_lines(1).fg(colors.text)
│  ├ flex_spacer()
│  └ icon_button("✕").size(22,22).fg(colors.muted).tooltip("关闭卡片")
└ col.visible_signal(expanded)
   .bg(colors.content).corner(6).padding(8).spacing(6)
   ├ label("没有打开图片").fg(colors.muted).visible_when(move || !has_image.get())
   └ col.visible_when(move || has_image.get()) // 信息行
```

- 卡片外壳 `colors.card` 是卡片标题/边缘色；正文面板 `colors.content` = **看图窗口的 `empty_bg`**，标题与内容一眼分得开。
- 标题区**只包箭头+标题文字**；悬停底色不会向右延伸到大片空白。`flex_spacer` 把关闭按钮推到右缘。
- 标题行高 24px、卡片外壳四边 padding 8px、标题↔正文间距 6px：这是 2026-11-14 第二轮回看后的最终档，再高会浪费面板的纵向空间。
- 关闭按钮是标题区的兄弟节点，不是子节点；标题区的 `Clickable` 不会吞掉关闭按钮的点击。
- 折叠时正文不占高度、不参与命中（`visible_when` 语义，不是“隐藏但留位”）。
- 卡片被关闭时 `visible_signal(active)` 为假，整张卡不占高度；面板只剩空态提示。

### 5.3 信息行

- 每行：`label(标签).width(52).fg(colors.muted)` + `label_signal(值).weight(1).fg(colors.text)`。
- 标签 12px、次要色；值 13px、正文色；行间距 **2 px**。
- 长值换行：文件名 / 类型 / 位置 / 动画 / 相机 / 镜头 / 曝光 / GPS 最多 2–3 行（位置 3 行，其余 2 行）；
  大小 / 修改时间 / 图像尺寸 / 拍摄时间 / 焦距这类定长值单行 `max_lines(1) + truncate(End)`。
  超出按高度裁剪，不追求省略号。
- 动画、拍摄时间、相机、镜头、曝光、焦距、GPS 这些值在空串时整行隐藏。
- 属性卡第一版只读：不做复制按钮、不做悬停看全文、不做行内编辑。
- ★ 本节以实施计划 / 任务简报的实现为准；设计稿只记意图，几行、什么间距这类细节不在两处各写一份。

### 5.4 空态

- 面板里没有任何卡片：显示“右键「属性」或「本图」添加输出”，不画卡片外壳。
- 属性卡 active 但没有打开图片：卡片保留，正文显示“没有打开图片”。
- 属性卡被关闭：卡片不占高度；面板回到空态提示。

### 5.5 主题跟随

- `Element` 的 `.bg()` / `.fg()` 接收的是构建期颜色，不是主题角色；只建一份面板，切换 `ui_theme` 后它会停在旧配色。
- windui 的全局主题不能用来解决：它同时服务设置页，设置页必须保持浅色。
- 方案：`tokens` 暴露两套 `PanelColors`（dark/light，字段 `panel/header/card/content/text/muted/edge`），`build_panel` 建两份树，外层用 `stack` 按 `tokens::theme()` 显隐：

```text
stack.fill()
├ panel_dark.visible_when(move || tokens::theme() == Theme::Dark)
└ panel_light.visible_when(move || tokens::theme() == Theme::Light)
```

- 两份树共用同一组 `InfoPanel` / `MetaSignals` 信号；只有一份可见，布局、命中、滚动只走可见那份。
- `window_tick` 的 `seen_theme` 翻转分支除 `mark_dirty_all()` 外再补一次 `ctx.mark_layout_dirty()`，让显隐条件重新求值。
- 代价：面板节点数翻倍；切主题时滚动位置与焦点归零（可见那份被换掉）。面板很小，接受。
- 库控件的 hover 叠层与滚动条仍走 windui 全局浅色主题，不随 `ui_theme`；见 §14。

### 5.6 技术信息卡

```text
card "技术信息"
├ 解码器   jpeg 0.3.4
├ 格式     JPEG 图像
├ 像素     4000×3000 → 2000×1500（面积 25%）
├ 像素格式 RGBA8 · 8 bit · Alpha
├ 色彩配置 内嵌 ICC（2.0 KB）· 输出 sRGB
├ 方向     EXIF 6 · 解码器已校正 / 宿主已校正
├ 显示     37% · 旋转 90° · 水平镜像
└ 帧/页    12 帧 · 80 ms / 3 页（单帧单页隐藏）
```

- 数据源：`Meta`（查询阶段 `iv_render(out=NULL)` 回填的 decoder / bit_depth / pixel_format / orientation / frame_count / page_count / icc_len）+ `Loaded.natural/decoded` + `Viewer::display_state()`。
- `Meta` 同时缓存预读图的条目；`install` 只覆盖 natural，不丢技术字段；Meta 晚于 Frame 到达时 bump `image_gen`，确保卡片从“未报告”刷新一次。
- 技术字段是**解码器报告值**，不是重新解析文件；解码器不报就显示“未报告”。
- 显示档尺寸按面积算百分比（旋转后两轴可能对调，按单边比会读错）。

### 5.7 直方图卡

```text
card "直方图"
└ body
   ├ HistogramView   // 四条平滑曲线叠在同一绘图区：R / G / B / 亮度
   └ label_signal(hist_summary)
```

- 统计当前解码档 `base_rgba`（`Viewer::histogram`），用 `plugin_sample_step` 等距抽样、`histogram_rgba` 只挑不混；alpha=0 不计。
- 亮度用 Rec.601 整数近似 `(77R + 150G + 29B) >> 8`，与 `levels` 插件同一份口径。
- 缓存键 `(path, base_rgba 指针, decoded 宽高)`；旋转/镜像不重算，换图/换解码档才重算。
- 直方图只在卡片 active 且展开时刷新（`refresh_histogram_if`），收起/关面板不抽样。
- 图形与 `levels` 插件同级：四条曲线共用纵轴、5 横 5 纵刻度、只描线不上面积色；曲线做 5 抽头高斯平滑，裁切百分比仍按原始箱算。
- 纵轴峰值**去掉 0 / 255 两个端点档**再取最高档：黑边 / 透明垫 / 过曝常把端点顶到 20%+，按全局峰值缩放会把照片主体压成一条平线；端点尖峰截顶画一根贴边竖线，精确比例仍由摘要给出。
- 高度随宽度按 16:9 增长，夹在 160..=480 px：面板拉宽后图会变高，不再是一条窄缝。
- 卡片底部如实交代来路：当前解码画面尺寸、抽样步长、均值 / 中位 / 峰值、R/G/B 均值、阴影裁切（luma 0）、高光裁切（luma 255）；SVG 区域渲染时统计的是当前解码切片，文案写“当前解码画面”。

## 6. 状态与持久化

### 6.1 `InfoPanel`

`Chrome` 增加：

```rust
pub struct InfoPanel {
    pub open: Cell<bool>,             // 用户期望的开/关
    pub width: Cell<i32>,             // 目标宽，240..=4096；实际上限=内容宽 50%
    pub visible: Cell<bool>,          // 当前实际可见（窄窗/无边框会压成 false）
    pub content_w: Cell<i32>,         // 根内容宽，tick 回写
    pub ratio: Signal<f32>,           // 画布占比，SplitHandle 直接写
    pub last_ratio: Cell<f32>,        // 上一拍比例，区分“用户拖动”与“窗口缩放”
    pub was_visible: Cell<bool>,      // 可见性翻转时应用目标宽
    pub layout_dirty: Cell<bool>,     // 无 ctx 的写点请求下一拍重排
    pub props_active: Signal<bool>,   // 属性卡是否在面板里
    pub props_expanded: Signal<bool>, // 属性卡是否展开
    pub tech_active: Signal<bool>,    // 技术信息卡
    pub tech_expanded: Signal<bool>,
    pub hist_active: Signal<bool>,    // 直方图卡
    pub hist_expanded: Signal<bool>,
}
```

- `InfoPanel::new()` 给出厂默认：关闭、300、空卡片、展开。
- `Chrome::new` 创建信号与 Cell；`WinRt::create` 在拿到配置后调用 `info.init_from(&config)`。
- `init_from` 只灌入会话态，之后本窗以 `InfoPanel` 为真值；配置是持久化落点与下次开窗的种子。

### 6.2 配置键

| ini 键 | 默认 | 语义 |
|---|---|---|
| `info_panel` | `0` | 面板开/关（`1` 开） |
| `info_panel_width` | `300` | 目标宽（逻辑 px，夹在 240..=4096；实际再受窗口半宽约束） |
| `info_cards` | 空 | 当前打开的卡片 id，分号分隔；认 `properties` / `technical` / `histogram` |
| `info_cards_collapsed` | 空 | 收起的卡片 id，分号分隔；空 = 全部展开 |

- 解析时宽度越界夹紧到 240..=4096、乱码回落 300；卡片列表按分号切分、去空串，未知 id 解析后忽略（不显示，也不报错）。
- 写回时只写当前已知卡片：按 `Card::ALL` 顺序收集 active / collapsed，未知 id 不写回。
- 面板状态按窗独立：建窗时读配置；用户操作当场改 `shared.config` 并 `mark_dirty()`，走现有 5 秒防抖落盘。已经开着的另一扇窗不实时跟随；新开的窗继承最新内存配置。
- 宽度在拖动过程中每拍写回 `info_panel_width`；防抖保证不会每帧写盘。

### 6.3 卡片 id

- v1 定义 `properties` / `technical` / `histogram` 三个 id，顺序与 `Card::ALL` 一致。
- `info_cards` / `info_cards_collapsed` 用字符串列表而不是布尔，是为了后续加卡片/插件卡片时不用改 ini 形状。
- 未知 id 不参与显示，也不导致解析失败；写回时按“当前已知卡片”重写。

## 7. 宽度与可见性

### 7.1 常量

```text
PANEL_W_MIN      = 240
PANEL_W_MAX      = 4096     // 只防 ini 天文数字；实际由半宽先约束
PANEL_MIN_WINDOW = 600      // 低于此宽自动隐藏（半宽 300 仍高于 240 下限）
PANEL_MAX_SHARE  = 0.50
HANDLE_W         = 1        // 视觉 / 布局只占 1px，不再留 6px 槽
HANDLE_HIT_PAD   = 8        // 命中左右各扩 8px（顶部标题栏那段裁掉）
```

### 7.2 纯函数

```text
panel_max_px(content_w) = min(PANEL_W_MAX, floor(content_w * PANEL_MAX_SHARE))
panel_visible(open, borderless, content_w) =
    open && !borderless && (content_w <= 0 || content_w >= PANEL_MIN_WINDOW)
clamp_panel_px(px, content_w) = clamp(px, PANEL_W_MIN, max(PANEL_W_MIN, panel_max_px(content_w)))
clamp_ratio(ratio, content_w) = ratio_for_panel(clamp_panel_px(panel_px(ratio, content_w), content_w), content_w)
usable_w(content_w) = max(1, content_w - HANDLE_W)
ratio_for_panel(px, content_w) = 1.0 - px / usable_w(content_w)
panel_px(ratio, content_w) = round((1.0 - ratio) * usable_w(content_w))
```

### 7.3 tick 算法

每个窗口的 `window_tick` 调用 `InfoPanel::tick(ctx, content_w, config)`：

1. `content_w = ctx.bounds().w`（窗口根节点宽）；`content_w <= 0` 视为“宽度未知”，可见性先按开/无边框判断。
2. 刷新 `visible`；值翻转时 `ctx.mark_layout_dirty()`。
3. 消费 `layout_dirty` 旗：有请求就 `ctx.mark_layout_dirty()`。
4. 不可见：`was_visible = false`，本拍结束。
5. 可见且上一拍不可见（刚打开 / 窄窗恢复）：按目标宽 `width` 反算比例，夹在 240..=`panel_max_px`；写 `ratio`、`last_ratio`，重排。
6. 可见且比例相对 `last_ratio` 变了：视为用户拖动。把比例换成像素、夹到允许范围；若被夹，回写比例；把夹后的像素写入 `width` 并写回配置。
7. 可见且比例没变：视为窗口缩放或稳定态。按目标宽反算比例；若当前像素与目标差超过 1 px，写比例并重排；**不改 `width`**，所以窗口缩小时目标宽保留，放大后回到原目标。

### 7.4 分隔条适配器

`PanelSplitHandle` 持有 `SplitHandle`、`Signal<f32>` 与 `Rc<Chrome>`，实现 `Widget`：

- `measure` / `paint` / `cursor` 原样委托；视觉仍是 `SplitHandle` 自己那条 1px 线。
- 节点在行里仍是 `HANDLE_W = 1`，但外层加 `hit_pad_edges`（左右 8px、顶部 `-STRIP_H`）与 `raised()`：
  命中扩到 17px，且必须先于面板命中（否则面板侧的 8px 会被面板节点接住）。
  这两项来自 windui 补丁 030；`hit_pad` 不参与 `usable_w` 的换算。
- 顶部标题栏那段裁掉，是因为画布最右那颗窗口关闭钮紧贴分隔线；不裁会把「点关闭」抢成「拖宽」。
- `on_event` 先委托；若事件被消费，再按当前 `content_w` 把 `ratio` 换算成像素并夹到 240..=`panel_max_px`。
- 夹到边界时回写 `ratio` 并 `ctx.mark_layout_dirty()`。
- 这样拖到边界当场停住，不会出现“拖过头、回拖一段才有反应”的死区。
- `SplitOpts` 用宽松比例（如 0.05..=0.95），真正的像素范围由适配器负责。

### 7.5 可见性

- `visible = open && !borderless && (content_w <= 0 || content_w >= 600)`。`content_w <= 0` 表示首帧宽度未知，先按“够宽”处理，下一拍 tick 用真实宽纠正。
- 无边框形态下 `visible` 恒假，但 `open`/卡片状态保留；退出无边框后按 `open` 恢复。
- 窗口窄于 600 时 `visible` 假；拉宽后自动恢复，目标宽沿用 `width`。
- 面板隐藏/可见都不改变画布标题栏的五颗窗口钮；面板头部只留自己的关闭按钮。

## 8. 属性数据流

### 8.1 信号

`MetaSignals` 字段：

```text
has_image, filename, kind, location, size, mtime, dims, anim,
capture_time, camera, lens, exposure, focal, gps
last_gen: Cell<u64>
enabled: Cell<bool>
```

- `Signal<String>` 由属性卡的 `label_signal` 绑定；`Signal::set` 触发重绘。
- `has_image` 由刷新逻辑维护，卡片据此切换“没有打开图片”与信息行。
- `last_gen` 仍是 `Viewer::image_gen` 的差分键。
- `enabled` 记录上一拍“面板可见 + 卡片 active”的取值，用于 false→true 时强制刷新一次。

### 8.2 刷新门控

`refresh_metadata_if(viewer, wanted)`：

1. 取本窗 `MetaSignals`；没有就返回。
2. `was = enabled.replace(wanted)`。
3. `wanted && !was` ⇒ `last_gen = u64::MAX`（保证重新打开卡片/面板立刻刷一次）。
4. `wanted` 为假 ⇒ 直接返回，不 stat、不读 APNG、不读 EXIF。
5. `wanted` 为真 ⇒ 走既有 `refresh_metadata`：`image_gen` 没变就返回；变了才重算。

`wanted` 由 `runtime` 计算：

```text
chrome.info.visible.get() && chrome.info.props_active.get()
```

- 刷新调用仍在活动窗口的 `global_tick` 里，和现有属性窗轮询同一位置。
- 非活动窗口的面板不实时刷新；窗口被激活后的第一拍按 `image_gen` 补上。
- 切图 / 翻页 / 换层 / 解码完成都会推 `image_gen`，因此卡片自动更新。
- EXIF 仍只在路径变化时读；缩放/平移只推 `image_gen` 但不重复读盘。

### 8.3 无图与空值

- 没有打开图片：`has_image=false`；文件名置“（无图片）”，其余字段置空；卡片显示“没有打开图片”。
- 文件 stat 失败：大小/时间显示“未知”。
- 尺寸未解出：显示“读取中…”，解码完成后再刷新。
- 动画信息为空：动画行隐藏。
- EXIF 字段为空：对应行隐藏。
- 技术信息卡：解码器未回报的字段显示“未报告”；无图时“显示”行隐藏。
- 直方图卡：无图 / 无有效像素时四条曲线为空，摘要写“当前解码画面没有可统计的像素”。

## 9. 标题栏与窗口按钮

- `TitleBtn::ALL` 保持现有右→左序；窗口五颗钮（置顶 / 锁定 / 最小化 / 最大化 / 关闭）始终连在画布标题栏。
- 面板头部只留「信息」+ 收起面板 `<`，不放窗口三键，也不作为窗口拖动区。
- `title_btn_rect`、`chrome_hit`、`paint` 都从 `TitleBtn::ALL` 这一份走；面板开合不影响标题栏几何。
- 标题文本的右侧让宽固定为五颗按钮（5×40 + 8 = 208）。
- 锁定形态：面板整体 `enabled_when(!locked)`，面板自身控件不可交互；画布的锁钮仍可用。
- 锁定/全屏时面板头部不提供窗口拖动；窗口拖动只走画布标题栏。

## 10. 入口与文案

| 入口 | 行为 |
|---|---|
| 右键「文件 ▸ 属性」 | 打开面板 + 加属性卡 + 展开；菜单项去掉省略号 |
| 属性快捷键（`Action::Properties`） | 同上；没有 `EventCtx` 的兜底路径也能执行，只是不弹 toast |
| 右键「界面 ▸ 信息面板」 | 只切面板开/关；勾选态跟本窗 `InfoPanel.open`；不动卡片 |
| 右键「本图 ▸ 技术信息 / 直方图」 | 勾选即把该输出加进面板并展开；取消即移除；勾上会顺手打开面板 |
| 面板头部 `<` | 收起面板；卡片状态保留 |
| 卡片标题区 | 展开/折叠；写回折叠状态 |
| 卡片 `✕` | 关闭卡片；从 `info_cards` 移除；面板保留 |
| 无边框下点「属性」 | toast「无边框形态不显示信息面板」；不改面板/卡片状态 |
| 窄窗下点「属性」 | 卡片加上，面板因宽度自动收起；toast「窗口太窄，信息面板暂时收起；拉宽窗口后显示」 |

- 面板空态文案：`右键「属性」或「本图」添加输出`。
- 卡片无图文案：`没有打开图片`。
- 收起面板按钮 tooltip：`收起信息面板`；关闭卡片按钮 tooltip：`关闭卡片`。
- 插件结果窗保持现状；面板不接管插件输出。

## 11. 测试与验收

### 11.1 配置

- 四个键的保存→读取往返；宽度 10→240、999→999、9999→4096、-1→240、u32 溢出→4096、乱码→300。
- `info_cards` / `info_cards_collapsed` 分号切分、去空串、空串读成空列表。
- `ALL_KEYS` 集合、`default_ini()` 注释模板、`ui_touchpoints_cover_all_keys`、`config_last_mile` 全部通过。

### 11.2 纯函数

- `panel_max_px`：600→300、1000→500、1200→600、2000→1000、10000→4096。
- `panel_visible`：关闭/无边框/窄窗分别为假；`content_w <= 0`（首帧未知）与 600 及以上为真。
- `clamp_panel_px`：低于 240 抬到 240；高于窗口半宽压到上限；上限低于 240 时按下限处理。
- `ratio_for_panel` / `panel_px` 往返误差 ≤1 px。

### 11.3 布局

- 面板空态：只有提示，没有卡片外壳。
- 属性卡 active + expanded：信息行可见，`scroll.content_h` 为展开高度。
- 折叠后：正文不占高度，`content_h` 变小；再展开恢复。
- 技术信息 / 直方图卡 active 也撑高 `scroll.content_h`；折叠直方图后变矮。
- 关闭卡片后：卡片不占高度，空态提示回来。
- 面板隐藏时：画布占满整行；面板可见时：画布宽度 = 内容宽 − 1 − 面板宽。

### 11.4 数据

- 面板不可见 / 卡片 inactive：`refresh_metadata_if(false)` 不改信号、不推 `last_gen`。
- 重新可见 / 重新加卡：`refresh_metadata_if(true)` 在 `image_gen` 未变时也能强制刷一次。
- 有图切到无图：`has_image` 翻转，卡片显示“没有打开图片”。
- 切图后 `image_gen` 变化：文件名/大小/时间/尺寸随之更新。
- EXIF 路径变化才重读；同路径缩放不重读（沿用既有 `EXIF_LAST_PATH` 记账）。
- 预读阶段的 Meta 只入缓存、不进 `preview_meta`；`install` 只覆盖 natural，解码器/位深/ICC 保留；Meta 晚于 Frame 到达时 bump `image_gen`，技术卡从“未报告”刷新。
- 直方图：RGB/亮度分档、全透明样本排除、8×8 一半黑一半白抽样后阴影/高光各 50%。

### 11.5 标题栏与交互

- 面板可见/隐藏都不改变画布标题栏的五颗窗口钮与命中；面板头部只有「信息」+ `<`，不命中窗口三键。
- 「本图 ▸」里的技术信息 / 直方图两项按 `Card` 取文案，勾选态读 active，切换走 `open_card` / `close_card` + persist + 重排；不再有独立的「信息卡」子菜单。
- 分隔条视觉 1px，但左右各 8px 都能命中；顶部标题栏那段不归分隔条，关闭钮点击不受影响。
- 面板隐藏/窄窗/无边框时，画布标题栏保持原样。
- 锁定后面板整体不可交互；解锁后恢复。
- 拖分隔条：宽度停在 240 / 4096（或窗口半宽上限），不抖动、不出现回拖死区。
- 窗口缩放：面板保持目标像素宽（受窗口半宽约束）；低于 600 隐藏，拉宽后恢复。

### 11.6 门禁

- `cargo test`（宿主）全绿；`cargo check` 无新增警告（按现有 `RUSTFLAGS` 入口跑）。
- `python build/structure_gen.py --check` 绿。
- `docs_consistency` 全绿（含文档行数、结构文档覆盖、j3 悬空引用、j4 考古串）。
- 静态门禁（`hygiene_scan`）0 违规。
- 不跑 `full.bat` 全量构建；用目标构建/测试脚本或 `cargo` 分步验证，交付前再按需跑完整流水线。

## 12. 文档与门禁更新

- `目录结构说明.md`：跑 `python build/structure_gen.py --write` 同步新文件、行数与符号索引。
- `HISTORY.md`：本功能落地后按既有惯例补一条里程碑记录。
- `docs/架构总览.md`：UI 层补“右侧信息面板 / 信息输出坞”的职责与数据流。
- `docs/待办.md`：第 6 项（右侧信息面板）完成后销账或改为后续插件卡片条目。
- `.plan/m00-计划编号规范与当前索引.md`：登记本计划的编号与状态。
- `DOCS_MANIFEST.md`：登记本 spec 与后续实施计划。
- 代码注释里指向属性窗的链接改为属性卡/信息面板；不再引用已退役的窗口入口。

## 13. 非目标与后续

- 插件卡片与 `kind=panel` 协议：原定第二阶段。★ 2026-11-14：**命令型插件的输出卡已落地**
  （协议 v2 `outputs`，见 §15）；`kind=panel` 插件形态仍不存在。
- 直方图 / 色阶分析卡：原定第二阶段。★ 直方图卡已做（§5.7）；`levels` 的“图 + 表 + 文”
  走插件输出卡（§15），不是内置卡。
- 卡片拖拽排序、浮动/停靠/拆分、卡片内旋转/缩放/滤镜控件：后续版本。
- 属性弹窗兼容：不保留第二个属性入口。
- 面板快捷键：第一版只给菜单入口。

## 14. 风险与已知限制

- **windui 全局浅色主题**：面板底色/文字用主窗令牌，但 `icon_button` 的 hover 叠层、`scroll` 的滚动条仍取 windui 主题色。深色主窗下这些细节可能偏浅；第一版接受，后续若需要再给 windui 增加按节点配色或自绘滚动条。
- **最大化字形**：`Element::window_button` 的最大化键不随窗口状态切换“还原”字形；第一版接受。
- **分隔条拖动中被隐藏**：`SplitHandle` 自身没有 `reset_interaction`，若拖宽时窗口突然窄于 600，`dragging` 状态可能残留。实现时验证一次；若复现，在适配器里记录并清理拖动态。
- **多窗状态**：面板按窗独立，配置是种子；两扇窗同时改面板时，以最后写入 `shared.config` 的值为准。开/关与卡片不实时同步到已开窗口。
- **非活动窗口刷新**：属性刷新仍在活动窗口的全局段；非活动窗口的面板在激活后的第一拍补刷，避免后台窗口无谓推信号重绘。
- **卡片内容高度**：长路径/长 EXIF 会让卡片很高，靠外层 `scroll` 滚动；v1 不做“展开全部/收起全部”。

## 15. 第二阶段：插件输出卡（已实现，2026-11-14）

### 15.1 协议

- 宿主 `PROTOCOL_VERSION = 2`，`MIN_PROTOCOL_VERSION = 1`；区间 `1..=2`，旧插件照旧按 v1 说话。
- 响应新增 `outputs` 数组，块类型：
  - `{"type":"text","title?","text"}`；
  - `{"type":"table","title?","rows":[["标签","值"], …]}`；
  - `{"type":"image","title?","format":"png","base64":"…"}`（只收 png）。
- 解析期闸：base64 可解、原始字节 ≤ 4 MB、PNG 8 bit、解码后 ≤ 16 MP；不满足 = 坏响应。
- `outputs` 缺席或为空 = 旧插件，仍走 `text` + `open_image` 老路。

### 15.2 卡片

- 一次运行 = 一张卡；`key = 插件 id`，同 id 重跑替换并置顶；卡头 = 插件名 + ✕。
- **可折叠**：点标题行 ▾ / ▸ 展开收起（会话级，不写配置）；换图随卡一起清。
- 数据在 `InfoPanel::plugin_cards: Signal<Vec<PluginCard>>`；`plugin_output::cards_element`
  用 `host_signal` 按当前列表重建子树，放在三张内置卡上方。
- 会话级：换图（当前路径变化）清空；同一路径缩放/旋转不清；不写 ini。
- 面板关着 → 自动打开；无边框或面板不可见 → 文本结果窗回退（图只留
  「［图片输出：标题］」一行，表与文本照旧）。
- 图片块宽跟面板、高按比例夹 80–480 px；PNG 在拿到响应时解一次，滚动不重复解。
- **表格**：行头列宽按整张卡最长行头自适应（48–104 px），摘要表与报告表值列对齐；
  行头与内容同字号 13 px、同颜色、同字体族（微软雅黑）、顶部对齐——数字与中文
  分走两套字体时 ascent 不同，两列基线会差 1px；右列内容在自己的列宽内换行，
  卡片高度随内容结束。
- **复制**：文本块用只读多行输入框，支持鼠标拖选 / Ctrl+A / Ctrl+C / 右键复制；
  图片块右键「复制图片」（PNG 写剪贴板）；卡内空白 / 表格右键「复制全部文本」；
  内置属性 / 技术信息卡右键「复制全部信息」、直方图卡右键「复制直方图数据」。

### 15.3 滚动条让位

- windui 滚动条命中区 16 px（`scrollbar::HIT_W`），会把卡头 ✕ 的点击抢成翻页
  （真机 2026-11-14 复现）。面板 `scroll` 的右 padding 增加 `SCROLLBAR_HIT_W`，
  卡片右缘停在 track 左侧 9 px，视觉左右接近对称。
- 回归用例 `card_close_button_clears_the_scrollbar_hit_zone` 钉住：内容溢出时，
  所有 22×22 按钮的右缘不得进入滚动条命中区。

### 15.4 验收

- 真机：`levels` 0.2.0 一次输出图 + 摘要表 + 报告；面板自动打开；✕ 可关；换图清空；
  无边框回退文本窗。
- 单测：协议 v2 解析 / 序列化往返、坏格式拒绝；PNG → RGBA；插件卡撑高面板、
  换图清空、关闭按钮让位。
