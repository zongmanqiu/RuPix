# 右侧信息面板 Implementation Plan

> 状态：已执行（m342）；复选框保留为实施过程记录。
>
> 后续用户反馈（2026-11-14）：窗口三键不再搬进面板头部——五颗窗口钮始终连在画布标题栏，面板头部只留「信息」+ 收起面板 `<`；Task 6 的标题栏过滤与面板头部三键已回退。
>
> 同轮视觉修正：面板头部改用主窗标题栏同档 `pill_bg`，收起按钮改用 MDL2 `ChevronLeft`（14px，同主窗图标）；卡片标题区只包箭头+标题（悬停不铺满整行）；卡片内容区底色改用看图窗口的 `empty_bg`；分隔线视觉 **1px**、命中左右各 **8px**（顶部标题栏那段裁掉）；卡片标题行高 **24px**、外壳 padding **8px**。
>
> 同日追加（用户确认方向后）：新增 **技术信息卡** 与 **直方图卡**；入口为右键 **「本图 ▸ 技术信息 / 直方图」**（信息面板是功能输出坞，不再单开「信息卡」子菜单），三张卡共用 `Card::ALL` 顺序与 `info_cards` 持久化。技术信息接查询阶段 `Meta`（解码器 / 位深 / 像素格式 / ICC / 方向 / 帧页），直方图用 `histogram_rgba` 对当前解码档等距抽样（只挑不混，alpha=0 不计），缓存键 `(path, base_rgba 指针, decoded 宽高)`；图形改为四曲线共用纵轴，高度随面板宽度按 16:9 增长。
>
> 同轮宽度口径：面板上限由 40% / 480 改为 **窗口内容宽的一半**（绝对上限 4096 只防天文 ini 值）。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 RuPix 主窗右侧加入一个可拖宽、可收起、卡片可折叠/关闭的信息输出坞，第一版承载图片「属性」卡，并退役属性弹窗。

**Architecture:** 根布局改为 `row[画布 | 6px SplitHandle | 面板]`；面板用 windui `Element` 树装配，两张主题副本按 `ui_theme` 显隐。面板状态放在 `Chrome::info`，配置落 `info_panel*` 四个 ini 键；属性数据从原属性窗模块迁到 `ui/info_panel/properties.rs`，刷新按“面板可见 + 属性卡 active”门控。

**Tech Stack:** Rust 2021、windui 0.23（本地补丁副本）、现有 `cargo`/`build/*.bat` 流水线。

## Global Constraints

- 只用相对路径书写，不写本机绝对路径。
- 临时文件、日志、草稿一律进 `.temp/`；不要写进源码仓。
- 不提交、不推送、不发布；每个任务末尾只做本地检查点。只有用户明确许可才能 commit。
- 不跑 `main\build\full.bat`。快速循环用 `cargo test --lib <过滤>`，交付前跑 `main\build\5_test.bat`；静态门禁用 `main\build\2_scan.bat` 或 `python build/hygiene_scan.py`。
- `ui/` 下不写字面色值；所有颜色走 `ui/tokens.rs`。新增颜色只能加在 `tokens.rs`。
- 宿主只做“看图”：不新增解码依赖，不把解码库编进 exe，不碰注册表/用户目录。
- 代码注释、文档用中文；注释解释“为什么”，不是复述代码。
- 新增/修改的公开行为必须有测试；先写失败测试，再写实现。
- 文档改动后跑 `python build/structure_gen.py --write`（或 `--check`），并跑 `docs_consistency`。
- 面板卡片第一版只读；不接插件卡片、直方图、旋转/缩放控件。

---

## File Structure

**Create**
- `main/src/ui/info_panel/mod.rs` — 面板状态、几何换算、根布局装配、主题副本、卡片外壳、分隔条适配器。
- `main/src/ui/info_panel/properties.rs` — `MetaSignals`、属性刷新、格式化、APNG 判定、门控。

**Modify**
- `main/src/core/config.rs` — 四个 ini 键、默认值、解析、写盘、`ALL_KEYS`、首启模板、测试。
- `main/src/ui/chrome.rs` — `Chrome::info`；`TitleBtn::shown` 与标题栏几何/命中/绘制的过滤。
- `main/src/ui/window.rs` — 根布局改成画布 + 分隔条 + 面板。
- `main/src/ui/canvas.rs` — 属性入口改走面板；「界面 ▸ 信息面板」；标题注释更新。
- `main/src/ui/mod.rs` — 声明 `info_panel`。
- `main/src/ui/tokens.rs` — `PanelColors` 两套配色。
- `main/src/ui/settings/mod.rs` — 收掉属性窗模块声明与再导出；模块头注释更新。
- `main/src/runtime/mod.rs` — 建窗灌配置、每窗 tick 更新面板、属性刷新门控、关窗清理。
- `main/src/core/hotkey.rs` — `Action::Properties` 注释从“属性窗”改为“属性卡”。
- `main/src/ui/settings/decoders_tab.rs`、`main/src/platform/exif.rs`、`main/src/core/viewer/schedule.rs`、`main/src/core/viewer/prefetch.rs` — 指向属性窗的注释改指向属性卡/信息面板。
- `main/src/ui/singleton.rs` — 文档里的清理函数名更新。
- `main/HISTORY.md`、`main/docs/架构总览.md`、`main/docs/待办.md`、`main/DOCS_MANIFEST.md`、`main/目录结构说明.md`、`.plan/m00-计划编号规范与当前索引.md` — 文档与索引。

**Delete**
- 原属性窗模块 — 内容迁入 `ui/info_panel/properties.rs`，窗口装配退役。

---

## Task 1: 配置键与持久化

**Files:**
- Modify: `main/src/core/config.rs`
- Test: `main/src/core/config.rs`（内联 `#[cfg(test)]`）

**Interfaces:**
- Produces: `INFO_PANEL_W_MIN: u32 = 240`、`INFO_PANEL_W_MAX: u32 = 480`、`INFO_PANEL_W_DEFAULT: u32 = 300`；`Config` 字段 `info_panel: bool`、`info_panel_width: u32`、`info_cards: Vec<String>`、`info_cards_collapsed: Vec<String>`。

- [ ] **Step 1: 写失败测试**

在 `config.rs` 的测试模块里加：

```rust
#[test]
fn info_panel_width_clamps_and_defaults() {
    for (raw, want) in [("10", 240u32), ("999", 480), ("abc", 300), ("", 300)] {
        let p = temp_ini("info-panel-width");
        std::fs::write(&p, format!("info_panel_width={raw}\n")).unwrap();
        assert_eq!(Config::load_from(&p).info_panel_width, want);
        let _ = std::fs::remove_file(&p);
    }
}

#[test]
fn info_panel_keys_round_trip() {
    let p = temp_ini("info-panel");
    let mut c = Config::default();
    c.info_panel = true;
    c.info_panel_width = 420;
    c.info_cards = vec!["properties".into(), "future".into()];
    c.info_cards_collapsed = vec!["properties".into()];
    c.save_to(&p);
    let back = Config::load_from(&p);
    assert!(back.info_panel, "info_panel 没读回");
    assert_eq!(back.info_panel_width, 420, "info_panel_width 没读回");
    assert_eq!(
        back.info_cards,
        vec!["properties".to_string(), "future".to_string()],
        "info_cards 没读回"
    );
    assert_eq!(
        back.info_cards_collapsed,
        vec!["properties".to_string()],
        "info_cards_collapsed 没读回"
    );
    let _ = std::fs::remove_file(&p);
}
```

并在 `every_key_has_a_load_consumer` 的输入 ini 里补：

```text
info_panel=1
info_panel_width=999
info_cards=properties;future
info_cards_collapsed=properties
```

断言：

```rust
assert!(c.info_panel, "info_panel 没有消费方");
assert_eq!(c.info_panel_width, 480, "info_panel_width 没有消费方");
assert_eq!(
    c.info_cards,
    vec!["properties".to_string(), "future".to_string()],
    "info_cards 没有消费方"
);
assert_eq!(
    c.info_cards_collapsed,
    vec!["properties".to_string()],
    "info_cards_collapsed 没有消费方"
);
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib core::config::tests::info_panel`
Expected: FAIL（字段/常量不存在）。

- [ ] **Step 3: 加常量、字段、默认值**

在 `WINDOW_MIN`/`WINDOW_MAX` 附近加：

```rust
/// 信息面板目标宽的下限（逻辑 px）。
pub const INFO_PANEL_W_MIN: u32 = 240;
/// 信息面板目标宽的上限（逻辑 px）。
pub const INFO_PANEL_W_MAX: u32 = 480;
/// 信息面板目标宽的出厂默认（逻辑 px）。
pub const INFO_PANEL_W_DEFAULT: u32 = 300;
```

在 `Config` 的 `ui_cards_collapsed` 后面加：

```rust
/// 右侧信息面板是否打开（ini 键 `info_panel`）。
pub info_panel: bool,
/// 信息面板目标宽（逻辑 px，夹在 [INFO_PANEL_W_MIN, INFO_PANEL_W_MAX]）。
pub info_panel_width: u32,
/// 面板里当前打开的卡片 id（分号分隔，ini 键 `info_cards`）。
pub info_cards: Vec<String>,
/// 面板里收起的卡片 id（分号分隔，ini 键 `info_cards_collapsed`）。
pub info_cards_collapsed: Vec<String>,
```

在 `Default` 的 `ui_cards_collapsed: Vec::new(),` 后加：

```rust
info_panel: false,
info_panel_width: INFO_PANEL_W_DEFAULT,
info_cards: Vec::new(),
info_cards_collapsed: Vec::new(),
```

- [ ] **Step 4: 加解析与写盘**

在 `load_from` 里 `ui_cards` 分支后加：

```rust
"info_panel" => set_bool(&mut cfg.info_panel, v, "info_panel"),
"info_panel_width" => {
    cfg.info_panel_width = v
        .parse()
        .unwrap_or(INFO_PANEL_W_DEFAULT)
        .clamp(INFO_PANEL_W_MIN, INFO_PANEL_W_MAX)
}
"info_cards" => {
    cfg.info_cards = v
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
"info_cards_collapsed" => {
    cfg.info_cards_collapsed = v
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
```

在 `save_to` 的 `format!` 里，把 `ui_cards={}\n` 改成：

```text
ui_cards={}\ninfo_panel={}\ninfo_panel_width={}\ninfo_cards={}\ninfo_cards_collapsed={}\n
```

并在参数末尾依次补：

```rust
self.info_panel as i32,
self.info_panel_width,
self.info_cards.join(";"),
self.info_cards_collapsed.join(";"),
```

- [ ] **Step 5: 登记 ALL_KEYS 与首启模板**

`ALL_KEYS` 从 `[&str; 48]` 改成 `[&str; 52]`，末尾加：

```rust
"info_panel",
"info_panel_width",
"info_cards",
"info_cards_collapsed",
```

`default_ini()` 末尾加：

```text
# 右侧信息面板（1 开 / 0 关；面板默认空，点「属性」才加卡）
info_panel=0

# 信息面板目标宽（逻辑 px，240..480；拖面板左缘调整）
info_panel_width=300

# 信息面板里打开的卡片 id，分号分隔（留空 = 空面板）
info_cards=

# 信息面板里收起的卡片 id，分号分隔（留空 = 全部展开）
info_cards_collapsed=
```

- [ ] **Step 6: 跑测试确认绿**

Run: `cargo test --lib core::config`
Expected: PASS（含 `saved_keys_match_all_keys`、`every_key_has_a_load_consumer`、`default_ini` 相关测试）。

---

## Task 2: InfoPanel 状态与几何纯函数

**Files:**
- Create: `main/src/ui/info_panel/mod.rs`
- Modify: `main/src/ui/mod.rs`
- Modify: `main/src/ui/chrome.rs`
- Modify: `main/src/runtime/mod.rs`
- Test: `main/src/ui/info_panel/mod.rs`

**Interfaces:**
- Produces: `InfoPanel { open, width, visible, content_w, ratio, last_ratio, was_visible, layout_dirty, props_active, props_expanded }`；`InfoPanel::new()`、`init_from(&Config)`、`set_open(bool) -> bool`、`open_properties() -> bool`、`close_properties() -> bool`、`toggle_properties() -> bool`、`persist(&Rc<RefCell<Config>>)`、`persist_width(&Rc<RefCell<Config>>)`、`panel_visible() -> bool`、`tick(&mut EventCtx, i32, &Rc<RefCell<Config>>)`。
- Produces pure fns: `panel_max_px`、`panel_visible`、`clamp_panel_px`、`clamp_ratio`、`ratio_for_panel`、`panel_px`、`usable_w`。
- Consumes: `Config` fields from Task 1.

- [ ] **Step 1: 写失败测试**

创建 `main/src/ui/info_panel/mod.rs`，先只放测试与常量骨架：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_width_follows_share_and_caps() {
        assert_eq!(panel_max_px(600), 240);
        assert_eq!(panel_max_px(1000), 400);
        assert_eq!(panel_max_px(1200), 480);
        assert_eq!(panel_max_px(2000), 480);
    }

    #[test]
    fn clamp_keeps_panel_inside_240_480_and_share() {
        assert_eq!(clamp_panel_px(100, 1000), 240);
        assert_eq!(clamp_panel_px(999, 1000), 400);
        assert_eq!(clamp_panel_px(350, 1000), 350);
    }

    #[test]
    fn ratio_and_px_round_trip() {
        for px in [240, 300, 480] {
            let cw = 1200;
            let r = ratio_for_panel(px, cw);
            assert!((panel_px(r, cw) - px).abs() <= 1, "px={px} 往返误差过大");
        }
    }

    #[test]
    fn panel_visible_respects_open_borderless_and_width() {
        assert!(!panel_visible(false, false, 1200));
        assert!(!panel_visible(true, true, 1200));
        assert!(!panel_visible(true, false, 599));
        assert!(panel_visible(true, false, 600));
        assert!(panel_visible(true, false, 0), "首帧宽未知先按够宽");
    }

    #[test]
    fn clamp_ratio_stops_at_both_edges() {
        let cw = 1000;
        assert_eq!(panel_px(clamp_ratio(0.10, cw), cw), 400);
        assert_eq!(panel_px(clamp_ratio(0.90, cw), cw), 240);
        assert_eq!(panel_px(clamp_ratio(0.59, cw), cw), 400);
    }

    #[test]
    fn properties_card_toggles_and_persists() {
        crate::ui::chrome::set_borderless(false);
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(crate::core::config::Config::default()));
        let info = InfoPanel::new();
        assert!(!info.panel_visible());
        assert!(info.open_properties());
        assert!(info.open.get());
        assert!(info.props_active.get());
        assert!(info.props_expanded.get());
        info.persist(&cfg);
        assert!(cfg.borrow().info_panel);
        assert_eq!(cfg.borrow().info_cards, vec!["properties".to_string()]);
        assert!(info.toggle_properties());
        info.persist(&cfg);
        assert_eq!(
            cfg.borrow().info_cards_collapsed,
            vec!["properties".to_string()]
        );
        assert!(info.close_properties());
        info.persist(&cfg);
        assert!(cfg.borrow().info_cards.is_empty());
        assert!(cfg.borrow().info_cards_collapsed.is_empty());
    }

    #[test]
    fn init_from_seeds_session_state() {
        let mut cfg = crate::core::config::Config::default();
        cfg.info_panel = true;
        cfg.info_panel_width = 420;
        cfg.info_cards = vec!["properties".into()];
        cfg.info_cards_collapsed = vec!["properties".into()];
        let info = InfoPanel::new();
        info.init_from(&cfg);
        assert!(info.open.get());
        assert_eq!(info.width.get(), 420);
        assert!(info.props_active.get());
        assert!(!info.props_expanded.get());
    }
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib ui::info_panel`
Expected: FAIL（`InfoPanel`/函数未定义）。

- [ ] **Step 3: 实现状态与纯函数**

在 `info_panel/mod.rs` 顶部加：

```rust
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windui::core::EventCtx;
use windui::signal::{signal, Signal};

use super::chrome;
use crate::core::config::{
    self, Config, INFO_PANEL_W_DEFAULT, INFO_PANEL_W_MAX, INFO_PANEL_W_MIN,
};

pub const PANEL_W_MIN: i32 = INFO_PANEL_W_MIN as i32;
pub const PANEL_W_MAX: i32 = INFO_PANEL_W_MAX as i32;
pub const PANEL_MIN_WINDOW: i32 = 600;
pub const PANEL_MAX_SHARE: f32 = 0.40;
pub const HANDLE_W: i32 = 6;
pub const CARD_PROPERTIES: &str = "properties";

pub struct InfoPanel {
    pub open: Cell<bool>,
    pub width: Cell<i32>,
    pub visible: Cell<bool>,
    pub content_w: Cell<i32>,
    pub ratio: Signal<f32>,
    pub last_ratio: Cell<f32>,
    pub was_visible: Cell<bool>,
    pub layout_dirty: Cell<bool>,
    pub props_active: Signal<bool>,
    pub props_expanded: Signal<bool>,
}

impl InfoPanel {
    pub fn new() -> Self {
        let ratio = signal(0.72);
        Self {
            open: Cell::new(false),
            width: Cell::new(INFO_PANEL_W_DEFAULT as i32),
            visible: Cell::new(false),
            content_w: Cell::new(0),
            ratio,
            last_ratio: Cell::new(0.72),
            was_visible: Cell::new(false),
            layout_dirty: Cell::new(false),
            props_active: signal(false),
            props_expanded: signal(true),
        }
    }

    pub fn init_from(&self, cfg: &Config) {
        self.open.set(cfg.info_panel);
        let w = cfg
            .info_panel_width
            .clamp(INFO_PANEL_W_MIN, INFO_PANEL_W_MAX) as i32;
        self.width.set(w);
        let active = cfg.info_cards.iter().any(|id| id == CARD_PROPERTIES);
        let collapsed = cfg
            .info_cards_collapsed
            .iter()
            .any(|id| id == CARD_PROPERTIES);
        self.props_active.set(active);
        self.props_expanded.set(active && !collapsed);
        let approx = (cfg.window_w.max(1) as f32 - 16.0).max(1.0);
        let r = 1.0 - (w as f32 / approx).clamp(0.0, 1.0);
        self.ratio.set(r);
        self.last_ratio.set(r);
        let wide = approx >= PANEL_MIN_WINDOW as f32;
        self.visible
            .set(cfg.info_panel && !cfg.borderless && wide);
    }

    pub fn panel_visible(&self) -> bool {
        self.visible.get()
    }

    pub fn refresh_visible(&self) -> bool {
        let changed = panel_visible(
            self.open.get(),
            chrome::borderless(),
            self.content_w.get(),
        );
        self.visible.replace(changed) != changed
    }

    pub fn set_open(&self, on: bool) -> bool {
        let open_changed = self.open.replace(on) != on;
        if on && open_changed {
            self.apply_width_now();
        }
        let vis_changed = self.refresh_visible();
        if open_changed || vis_changed {
            self.layout_dirty.set(true);
        }
        open_changed || vis_changed
    }

    /// 打开面板时立刻按目标宽反算比例；不等下一拍 tick，避免首帧从旧比例跳一下。
    fn apply_width_now(&self) {
        let cw = self.content_w.get();
        if cw <= 0 {
            return;
        }
        let px = clamp_panel_px(self.width.get(), cw);
        let r = ratio_for_panel(px, cw);
        self.ratio.set(r);
        self.last_ratio.set(r);
    }

    pub fn open_properties(&self) -> bool {
        let mut changed = self.set_open(true);
        if !self.props_active.get() {
            self.props_active.set(true);
            changed = true;
        }
        if !self.props_expanded.get() {
            self.props_expanded.set(true);
            changed = true;
        }
        if changed {
            self.layout_dirty.set(true);
        }
        changed
    }

    pub fn close_properties(&self) -> bool {
        let changed = self.props_active.replace(false);
        if changed {
            self.layout_dirty.set(true);
        }
        changed
    }

    pub fn toggle_properties(&self) -> bool {
        self.props_expanded.set(!self.props_expanded.get());
        self.layout_dirty.set(true);
        true
    }

    pub fn persist(&self, cfg: &Rc<RefCell<Config>>) {
        {
            let mut c = cfg.borrow_mut();
            c.info_panel = self.open.get();
            c.info_panel_width = self
                .width
                .get()
                .clamp(PANEL_W_MIN, PANEL_W_MAX) as u32;
            c.info_cards = if self.props_active.get() {
                vec![CARD_PROPERTIES.to_string()]
            } else {
                Vec::new()
            };
            c.info_cards_collapsed = if self.props_active.get() && !self.props_expanded.get() {
                vec![CARD_PROPERTIES.to_string()]
            } else {
                Vec::new()
            };
        }
        config::mark_dirty();
    }

    pub fn persist_width(&self, cfg: &Rc<RefCell<Config>>) {
        let w = self.width.get().clamp(PANEL_W_MIN, PANEL_W_MAX) as u32;
        if cfg.borrow().info_panel_width == w {
            return;
        }
        cfg.borrow_mut().info_panel_width = w;
        config::mark_dirty();
    }

    pub fn tick(&self, ctx: &mut EventCtx, content_w: i32, cfg: &Rc<RefCell<Config>>) {
        if content_w > 0 {
            self.content_w.set(content_w);
        }
        if self.refresh_visible() {
            ctx.mark_layout_dirty();
        }
        if self.layout_dirty.replace(false) {
            ctx.mark_layout_dirty();
        }
        if !self.visible.get() {
            self.was_visible.set(false);
            return;
        }
        let cw = self.content_w.get();
        if cw <= 0 {
            self.was_visible.set(false);
            return;
        }
        if !self.was_visible.replace(true) {
            let px = clamp_panel_px(self.width.get(), cw);
            let r = ratio_for_panel(px, cw);
            self.ratio.set(r);
            self.last_ratio.set(r);
            ctx.mark_layout_dirty();
            return;
        }
        let r = self.ratio.get();
        if (r - self.last_ratio.get()).abs() > 0.0001 {
            let cur = panel_px(r, cw);
            let px = clamp_panel_px(cur, cw);
            if px != cur {
                self.ratio.set(ratio_for_panel(px, cw));
                ctx.mark_layout_dirty();
            }
            self.last_ratio.set(self.ratio.get());
            if px != self.width.get() {
                self.width.set(px);
                self.persist_width(cfg);
            }
        } else {
            let desired = clamp_panel_px(self.width.get(), cw);
            let cur = panel_px(r, cw);
            if (cur - desired).abs() > 1 {
                self.ratio.set(ratio_for_panel(desired, cw));
                self.last_ratio.set(self.ratio.get());
                ctx.mark_layout_dirty();
            }
        }
    }
}

pub fn panel_max_px(content_w: i32) -> i32 {
    ((content_w as f32 * PANEL_MAX_SHARE).floor() as i32).min(PANEL_W_MAX)
}

pub fn panel_visible(open: bool, borderless: bool, content_w: i32) -> bool {
    open && !borderless && (content_w <= 0 || content_w >= PANEL_MIN_WINDOW)
}

pub fn usable_w(content_w: i32) -> i32 {
    (content_w - HANDLE_W).max(1)
}

pub fn clamp_panel_px(px: i32, content_w: i32) -> i32 {
    let max = panel_max_px(content_w).max(PANEL_W_MIN);
    px.clamp(PANEL_W_MIN, max)
}

pub fn ratio_for_panel(px: i32, content_w: i32) -> f32 {
    1.0 - px as f32 / usable_w(content_w) as f32
}

pub fn panel_px(ratio: f32, content_w: i32) -> i32 {
    ((1.0 - ratio) * usable_w(content_w) as f32).round() as i32
}

pub fn clamp_ratio(ratio: f32, content_w: i32) -> f32 {
    let cur = panel_px(ratio, content_w);
    let px = clamp_panel_px(cur, content_w);
    if px == cur {
        ratio
    } else {
        ratio_for_panel(px, content_w)
    }
}
```

- [ ] **Step 4: 跑测试确认绿**

Run: `cargo test --lib ui::info_panel`
Expected: PASS。

- [ ] **Step 5: 接进 Chrome 与建窗**

`main/src/ui/mod.rs` 加：

```rust
pub mod info_panel;
```

`main/src/ui/chrome.rs` 顶部加：

```rust
use super::info_panel::InfoPanel;
```

`Chrome` 字段末尾加：

```rust
/// 右侧信息面板的会话态（Task 2 起；面板树在后续任务接入）。
pub info: InfoPanel,
```

`Chrome::new` 里加：

```rust
info: InfoPanel::new(),
```

`main/src/runtime/mod.rs` 的 `WinRt::create` 里，在给 `chrome.checker/filmstrip/...` 灌值的 `let c = config.borrow();` 块内加：

```rust
chrome.info.init_from(&c);
```

- [ ] **Step 6: 跑全量 lib 测试**

Run: `cargo test --lib`
Expected: PASS（新增测试 + 既有测试）。

---

## Task 3: 属性数据模块迁移与刷新门控

**Files:**
- Create: `main/src/ui/info_panel/properties.rs`
- Modify: `main/src/ui/info_panel/mod.rs`
- Modify: `main/src/ui/settings/mod.rs`
- Modify: `main/src/runtime/mod.rs`
- Delete: 原属性窗模块
- Test: `main/src/ui/info_panel/properties.rs`

**Interfaces:**
- Produces: `MetaSignals`（`pub(crate)` 字段，含 `has_image`、`enabled`）、`meta_signals(scope) -> MetaSignals`、`refresh_metadata_if(viewer, wanted)`、`forget_window(scope)`、`format_utc`、`file_name`、`is_apng_file`、`is_apng_bytes`。
- Consumes: `InfoPanel::panel_visible()`、`InfoPanel::props_active`（Task 2）。

- [ ] **Step 1: 写失败测试**

在新建的 `properties.rs` 里先放测试（实现稍后补齐）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::viewer::Viewer;

    #[test]
    fn inactive_panel_does_not_touch_signals() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
        let viewer = std::rc::Rc::new(std::cell::RefCell::new(Viewer::new(
            std::rc::Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
            cfg,
        )));
        let scope = crate::ui::singleton::window_scope(&viewer);
        let sig = meta_signals(scope);
        sig.filename.set("sentinel".into());
        refresh_metadata_if(&viewer, false);
        assert_eq!(sig.filename.get(), "sentinel");
        assert_eq!(sig.last_gen.get(), u64::MAX, "门控为假时不该刷新");
        forget_window(scope);
    }

    #[test]
    fn reactivating_panel_forces_one_refresh() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));
        let viewer = std::rc::Rc::new(std::cell::RefCell::new(Viewer::new(
            std::rc::Rc::new(crate::core::decode::Decoder::spawn_with(tx)),
            cfg,
        )));
        let scope = crate::ui::singleton::window_scope(&viewer);
        let sig = meta_signals(scope);
        refresh_metadata_if(&viewer, true);
        assert_eq!(sig.filename.get(), "（无图片）");
        assert!(!sig.has_image.get());
        // 同 gen 再刷新不重复读盘，但重新激活必须强制刷一次。
        sig.filename.set("stale".into());
        refresh_metadata_if(&viewer, false);
        refresh_metadata_if(&viewer, true);
        assert_eq!(sig.filename.get(), "（无图片）");
        forget_window(scope);
    }

    #[test]
    fn apng_has_actl_before_idat() {
        let b = png(&[(b"acTL", &[0u8; 8]), (b"IDAT", &[0u8; 4])]);
        assert!(is_apng_bytes(&b));
    }

    #[test]
    fn static_png_with_actl_bytes_inside_idat_is_not_animated() {
        let b = png(&[(b"IDAT", b"xxacTLxx")]);
        assert!(!is_apng_bytes(&b), "IDAT 数据里的 acTL 不该算动画控制块");
    }
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib ui::info_panel::properties`
Expected: FAIL（模块不存在/函数未定义）。

- [ ] **Step 3: 迁移数据与刷新逻辑**

把原属性窗模块的内容迁入 `ui/info_panel/properties.rs`，做以下改动：

- 顶部 `use super::*;` 改成显式导入：

```rust
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use windui::prelude::*;
use windui::signal::{signal, Signal};

use crate::core::util::fmt_bytes;
use crate::core::viewer::Viewer;
```

- 删除 `open_metadata` 整个函数与属性窗口装配；
- `MetaSignals` 加字段：

```rust
pub(crate) has_image: Signal<bool>,
/// 上一拍“面板可见 + 属性卡 active”的取值；false→true 时强制刷新一次。
pub(crate) enabled: Cell<bool>,
```

- `meta_signals` 初始化里加：

```rust
has_image: signal(false),
enabled: Cell::new(false),
```

- `refresh_metadata` 的无图分支加：

```rust
set(&st.has_image, false);
```

- 有图分支加：

```rust
set(&st.has_image, true);
```

- 把 `refresh_open_metadata` 改成：

```rust
pub fn refresh_metadata_if(viewer: &Rc<RefCell<Viewer>>, wanted: bool) {
    let scope = super::super::singleton::window_scope(viewer);
    META.with(|m| {
        let mut map = m.borrow_mut();
        let Some(st) = map.get_mut(&scope) else {
            return;
        };
        let was = st.enabled.replace(wanted);
        if wanted && !was {
            st.last_gen.set(u64::MAX);
        }
    });
    if wanted {
        refresh_metadata(viewer);
    }
}
```

- 保留 `forget_window`、`refresh_metadata`、`format_utc`、`file_name`、`is_apng_file`、`is_apng_bytes` 及其测试；测试里的 `Config`/`Viewer`/`Decoder` 用完整路径导入。
- `META` 与 `EXIF_LAST_PATH` 仍是 `thread_local!`；`MetaSignals` 字段改为 `pub(crate)`（`info_panel` 要绑定它们）。

- [ ] **Step 4: 改模块声明与再导出**

`ui/info_panel/mod.rs` 加：

```rust
pub mod properties;
```

`ui/settings/mod.rs`：
- 删 `mod metadata;`
- 删 `pub use metadata::{forget_window as forget_metadata_window, open_metadata, refresh_open_metadata};`
- 模块头第一行“设置、关于、属性窗口”改成“设置、关于窗口”。

同步 `core/config.rs` 的 `ui_touchpoints_cover_all_keys` `concat!` 清单：
- 删原属性窗模块的 `include_str!` 那一行；
- 加 `include_str!("../ui/info_panel/mod.rs")` 与 `include_str!("../ui/info_panel/properties.rs")`；
- 删 `PENDING_UI_KEYS` 那张临时豁免表、它的 `★ 2026-11-14` 注释，以及 skip 条件里的 `|| PENDING_UI_KEYS.contains(&k)`；四个键现在由 `ui/info_panel/mod.rs` 真实引用，豁免表的自失效断言会强制这一步。

否则四个新键在 UI 文件里找不到写点，`ui_touchpoints_cover_all_keys` 会红。

- [ ] **Step 5: 改 runtime 调用**

`runtime/mod.rs` 的 `global_tick` 里，把：

```rust
crate::ui::settings::refresh_open_metadata(&self.viewer);
```

改成：

```rust
let wanted = self.chrome.info.panel_visible() && self.chrome.info.props_active.get();
crate::ui::info_panel::properties::refresh_metadata_if(&self.viewer, wanted);
```

关窗处把：

```rust
crate::ui::settings::forget_metadata_window(scope);
```

改成：

```rust
crate::ui::info_panel::properties::forget_window(scope);
```

- [ ] **Step 6: 跑测试确认绿**

Run: `cargo test --lib ui::info_panel::properties`
Expected: PASS。

Run: `cargo test --lib`
Expected: PASS（旧属性窗测试已随模块迁移；若有 `open_metadata` 引用，编译器会指出，按 Task 5 的入口改造先留到下一步）。

> 注意：此步之后画布里的 `settings::open_metadata` 调用会编译失败。为了让每个任务可编译，本任务先在 `ui/info_panel/mod.rs` 加一个临时入口，Task 5 再替换成真正的面板入口：

```rust
/// 临时入口：Task 5 换成面板入口；先保证编译与旧行为不丢。
pub fn open_properties(_ctx: Option<&mut EventCtx>, chrome: &Rc<chrome::Chrome>, cfg: &Rc<RefCell<Config>>) {
    chrome.info.open_properties();
    chrome.info.persist(cfg);
}
```

并把 `canvas.rs` 的两处 `settings::open_metadata(...)` 改成 `info_panel::open_properties(Some(ctx), &self.chrome, &self.config)`（或 `exec_action` 里的 `chrome`/`cfg`）。这一步不追求可见效果，只保证编译；Task 5 补 toast 与菜单入口。

同时把 `canvas.rs` 顶部的 `use super::{filmstrip, settings, tree};` 改成 `use super::{filmstrip, info_panel, settings, tree};`。

- [ ] **Step 7: 跑全量 lib 测试**

Run: `cargo test --lib`
Expected: PASS。

---

## Task 4: 面板 UI、属性卡与主题副本

**Files:**
- Modify: `main/src/ui/info_panel/mod.rs`
- Modify: `main/src/ui/tokens.rs`
- Test: `main/src/ui/info_panel/mod.rs`、`main/src/ui/tokens.rs`

**Interfaces:**
- Consumes: `InfoPanel`、`MetaSignals`、`Config`。
- Produces: `PanelColors { panel, header, card, text, muted, edge }` + `dark()/light()`；`build(viewer, chrome, config) -> Element`；`build_one(chrome, config, sig, colors) -> Element`；`PanelSplitHandle`。

- [ ] **Step 1: 写失败测试（配色 + 布局）**

`tokens.rs` 测试里加：

```rust
#[test]
fn panel_colors_differ_between_themes() {
    let d = PanelColors::dark();
    let l = PanelColors::light();
    assert_ne!(d.panel, l.panel);
    assert_ne!(d.text, l.text);
    assert_ne!(d.card, l.card);
}
```

`info_panel/mod.rs` 测试里加：

```rust
#[test]
fn panel_layout_shrinks_when_card_collapses_or_closes() {
    use windui::core::{NodeId, Tree};
    use windui::text::NullTextEngine;

    fn find_scroll(tree: &Tree, id: NodeId) -> Option<NodeId> {
        let n = tree.get(id)?;
        if matches!(n.layout, windui::core::Layout::Scroll) {
            return Some(id);
        }
        n.children.iter().find_map(|&c| find_scroll(tree, c))
    }

    let chrome = chrome::Chrome::new(false);
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(crate::core::config::Config::default()));
    let sig = properties::meta_signals(9_999);
    let panel = build_one(&chrome, &cfg, &sig, super::tokens::PanelColors::dark());

    let mut tree = Tree::new();
    let root = panel.build(&mut tree);
    tree.root = Some(root);
    let size = windui::geometry::Size::new(300, 600);
    tree.layout_root(size, &mut NullTextEngine);
    let scroll = find_scroll(&tree, root).expect("面板里要有 scroll");
    let h_empty = tree.get(scroll).unwrap().content_h;

    chrome.info.props_active.set(true);
    chrome.info.props_expanded.set(true);
    tree.layout_root(size, &mut NullTextEngine);
    let h_expanded = tree.get(scroll).unwrap().content_h;

    chrome.info.props_expanded.set(false);
    tree.layout_root(size, &mut NullTextEngine);
    let h_collapsed = tree.get(scroll).unwrap().content_h;

    chrome.info.props_active.set(false);
    tree.layout_root(size, &mut NullTextEngine);
    let h_closed = tree.get(scroll).unwrap().content_h;

    assert!(h_expanded > h_collapsed, "展开应比折叠高");
    assert!(h_collapsed > h_empty, "折叠卡仍占标题行");
    assert_eq!(h_closed, h_empty, "关闭卡片应回到空态高度");
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib ui::info_panel ui::tokens::tests::panel_colors`
Expected: FAIL（`PanelColors`/`build_one` 未定义）。

- [ ] **Step 3: 加 PanelColors**

`tokens.rs` 的 `Overlay` 访问函数后加：

```rust
/// 信息面板的一套配色。两套在构建期各取一次，面板树建两份、按主题显隐。
#[derive(Clone, Copy)]
pub struct PanelColors {
    pub panel: Color,
    pub header: Color,
    pub card: Color,
    pub text: Color,
    pub muted: Color,
    pub edge: Color,
}

impl PanelColors {
    pub fn dark() -> Self {
        Self {
            panel: to_color(DARK_OVERLAY.tree_bg),
            header: to_color4(DARK_OVERLAY.panel_bg),
            card: to_color4(DARK_OVERLAY.slot_bg),
            text: to_color(DARK_OVERLAY.text),
            muted: to_color(DARK_OVERLAY.text_muted),
            edge: to_color4(DARK_OVERLAY.panel_edge),
        }
    }

    pub fn light() -> Self {
        Self {
            panel: to_color(LIGHT_OVERLAY.tree_bg),
            header: to_color4(LIGHT_OVERLAY.panel_bg),
            card: to_color4(LIGHT_OVERLAY.slot_bg),
            text: to_color(LIGHT_OVERLAY.text),
            muted: to_color(LIGHT_OVERLAY.text_muted),
            edge: to_color4(LIGHT_OVERLAY.panel_edge),
        }
    }
}
```

- [ ] **Step 4: 实现面板树**

在 `info_panel/mod.rs` 加（完整实现，按现有代码风格补 `Element` 调用）：

```rust
use windui::core::Widget;
use windui::event::CursorShape;
use windui::geometry::{Rect, Size};
use windui::prelude::*;
use windui::render::Canvas;
use windui::style::Style;
use windui::text::TextEngine;
use windui::ui::split::{SplitHandle, SplitOpts};

use super::tokens::{self, PanelColors};
use super::properties::{self, MetaSignals};
use super::singleton;

pub fn build(
    viewer: &Rc<RefCell<crate::core::viewer::Viewer>>,
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
) -> Element {
    let sig = properties::meta_signals(singleton::window_scope(viewer));
    let dark = build_one(chrome, config, &sig, PanelColors::dark())
        .visible_when(move || tokens::theme() == tokens::Theme::Dark);
    let light = build_one(chrome, config, &sig, PanelColors::light())
        .visible_when(move || tokens::theme() == tokens::Theme::Light);
    Element::stack().fill().child(dark).child(light)
}

pub fn build_one(
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    sig: &MetaSignals,
    colors: PanelColors,
) -> Element {
    let close_panel = {
        let chrome = chrome.clone();
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            chrome.info.set_open(false);
            chrome.info.persist(&config);
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };
    let header = Element::row()
        .width_match()
        .height(chrome::STRIP_H)
        .cross(Align::Center)
        .window_drag()
        .bg(colors.header)
        .child(
            Element::label("信息")
                .font_size(13.0)
                .font_weight(600)
                .fg(colors.text)
                .padding_xy(10, 0)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(
            Element::icon_button("›")
                .size(24, 24)
                .fg(colors.muted)
                .tooltip("收起信息面板")
                .on_click(close_panel),
        )
        .child(
            Element::window_button(WindowButtonKind::Minimize)
                .fg(colors.text),
        )
        .child(
            Element::window_button(WindowButtonKind::Maximize)
                .fg(colors.text),
        )
        .child(
            Element::window_button(WindowButtonKind::Close)
                .fg(colors.text),
        );

    let empty = Element::label("右键「属性」添加信息卡")
        .font_size(12.0)
        .fg(colors.muted)
        .padding(12)
        .visible_when({
            let active = chrome.info.props_active;
            move || !active.get()
        });
    let card = property_card(chrome, config, sig, colors);
    let body = Element::scroll()
        .weight(1.0)
        .width_match()
        .padding(8)
        .child(
            Element::col()
                .width_match()
                .spacing(8)
                .child(empty)
                .child(card),
        );

    Element::col()
        .fill()
        .bg(colors.panel)
        .child(header)
        .child(body)
}

fn property_card(
    chrome: &Rc<chrome::Chrome>,
    config: &Rc<RefCell<Config>>,
    sig: &MetaSignals,
    colors: PanelColors,
) -> Element {
    let toggle = {
        let chrome = chrome.clone();
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            chrome.info.toggle_properties();
            chrome.info.persist(&config);
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };
    let close = {
        let chrome = chrome.clone();
        let config = config.clone();
        move |ctx: &mut EventCtx| {
            chrome.info.close_properties();
            chrome.info.persist(&config);
            ctx.mark_layout_dirty();
            ctx.mark_dirty();
        }
    };
    let expanded = chrome.info.props_expanded;
    let active = chrome.info.props_active;
    let has_image = sig.has_image;

    let title_row = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(4)
        .clickable()
        .on_click(toggle)
        .child(Element::label("▾").font_size(12.0).fg(colors.text).visible_signal(expanded))
        .child(
            Element::label("▸")
                .font_size(12.0)
                .fg(colors.text)
                .visible_when(move || !expanded.get()),
        )
        .child(
            Element::label("属性")
                .font_size(13.0)
                .font_weight(600)
                .fg(colors.text)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        );

    let head = Element::row()
        .width_match()
        .cross(Align::Center)
        .child(title_row.weight(1.0))
        .child(
            Element::icon_button("✕")
                .size(22, 22)
                .font_size(11.0)
                .fg(colors.muted)
                .tooltip("关闭卡片")
                .on_click(close),
        );

    let no_image = Element::label("没有打开图片")
        .font_size(13.0)
        .fg(colors.muted)
        .visible_when(move || !has_image.get());

    let rows = Element::col()
        .width_match()
        .spacing(2)
        .child(info_row("文件名", sig.filename, 2, colors))
        .child(info_row("类型", sig.kind, 2, colors))
        .child(info_row("位置", sig.location, 3, colors))
        .child(info_row("大小", sig.size, 1, colors))
        .child(info_row("修改时间", sig.mtime, 1, colors))
        .child(info_row("图像尺寸", sig.dims, 1, colors))
        .child({
            let s = sig.anim;
            info_row("动画", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.capture_time;
            info_row("拍摄时间", s, 1, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.camera;
            info_row("相机", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.lens;
            info_row("镜头", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.exposure;
            info_row("曝光", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.focal;
            info_row("焦距", s, 1, colors).visible_when(move || !s.get().is_empty())
        })
        .child({
            let s = sig.gps;
            info_row("GPS", s, 2, colors).visible_when(move || !s.get().is_empty())
        })
        .visible_when(move || has_image.get());

    let body = Element::col()
        .width_match()
        .spacing(6)
        .visible_signal(expanded)
        .child(no_image)
        .child(rows);

    Element::col()
        .width_match()
        .bg(colors.card)
        .corner(8.0)
        .border(colors.edge, 1)
        .padding(10)
        .spacing(8)
        .visible_signal(active)
        .child(head)
        .child(body)
}

fn info_row(label: &'static str, value: Signal<String>, max_lines: usize, colors: PanelColors) -> Element {
    let text = Element::label_signal(value)
        .font_size(13.0)
        .fg(colors.text)
        .weight(1.0);
    let text = if max_lines == 1 {
        text.max_lines(1).truncate(Truncate::End)
    } else {
        text.max_lines(max_lines)
    };
    Element::row()
        .width_match()
        .cross(Align::Start)
        .spacing(6)
        .child(
            Element::label(label)
                .width(52)
                .font_size(12.0)
                .fg(colors.muted),
        )
        .child(text)
}
```

- [ ] **Step 5: 实现分隔条适配器**

在 `info_panel/mod.rs` 加：

```rust
pub(crate) struct PanelSplitHandle {
    inner: SplitHandle,
    chrome: Rc<chrome::Chrome>,
}

impl PanelSplitHandle {
    pub(crate) fn new(chrome: Rc<chrome::Chrome>) -> Self {
        let opts = SplitOpts {
            thickness: HANDLE_W,
            min: 0.05,
            max: 0.95,
        };
        let inner = SplitHandle::new(Axis::Horizontal, chrome.info.ratio, opts);
        Self { inner, chrome }
    }

    fn clamp(&self, ctx: &mut EventCtx) {
        let cw = self.chrome.info.content_w.get();
        if cw <= 0 {
            return;
        }
        let r = self.chrome.info.ratio.get();
        let next = clamp_ratio(r, cw);
        if (next - r).abs() > f32::EPSILON {
            self.chrome.info.ratio.set(next);
            ctx.mark_layout_dirty();
        }
    }
}

impl Widget for PanelSplitHandle {
    fn measure(&self, avail: Size, style: &Style, text: &mut dyn TextEngine) -> Size {
        self.inner.measure(avail, style, text)
    }

    fn paint(
        &self,
        bounds: Rect,
        content: Rect,
        focused: bool,
        enabled: bool,
        canvas: &mut dyn Canvas,
        style: &Style,
    ) {
        self.inner.paint(bounds, content, focused, enabled, canvas, style);
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &windui::event::Event) -> bool {
        let used = self.inner.on_event(ctx, ev);
        if used {
            self.clamp(ctx);
        }
        used
    }

    fn cursor(&self) -> CursorShape {
        self.inner.cursor()
    }
}
```

- [ ] **Step 6: 跑测试确认绿**

Run: `cargo test --lib ui::info_panel ui::tokens`
Expected: PASS。

---

## Task 5: 根布局、tick 与入口切换

**Files:**
- Modify: `main/src/ui/window.rs`
- Modify: `main/src/runtime/mod.rs`
- Modify: `main/src/ui/canvas.rs`
- Modify: `main/src/ui/info_panel/mod.rs`（把 Task 3 的临时入口换成真正入口）
- Test: `main/src/ui/info_panel/mod.rs`（补入口/宽度测试）

**Interfaces:**
- Consumes: `InfoPanel::build`、`PanelSplitHandle`、`InfoPanel::tick`。
- Produces: 可见的右侧面板、可拖宽、窄窗隐藏、无边框隐藏。

- [ ] **Step 1: 写失败测试（入口与窄窗提示）**

在 `info_panel/mod.rs` 测试里加：

```rust
#[test]
fn open_properties_opens_panel_and_marks_dirty() {
    crate::ui::chrome::set_borderless(false);
    let chrome = chrome::Chrome::new(false);
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(crate::core::config::Config::default()));
    assert!(!chrome.info.open.get());
    open_properties(None, &chrome, &cfg);
    assert!(chrome.info.open.get());
    assert!(chrome.info.props_active.get());
    assert!(chrome.info.props_expanded.get());
    assert!(cfg.borrow().info_panel);
    assert_eq!(cfg.borrow().info_cards, vec!["properties".to_string()]);
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib ui::info_panel::tests::open_properties`
Expected: FAIL（`open_properties` 入口签名/行为不对）。

- [ ] **Step 3: 换根布局**

`ui/window.rs` 的 `build_root` 里，把原来的：

```rust
.child(
    Element::col()
        .fill()
        .widget(ImageCanvas::new(...))
        .autofocus(),
)
```

改成：

```rust
.child(
    Element::row()
        .fill()
        .cross(Align::Stretch)
        .child(
            Element::col()
                .fill()
                .weight_when({
                    let c = chrome.clone();
                    move || c.info.ratio.get()
                })
                .widget(
                    ImageCanvas::new(
                        viewer.clone(),
                        chrome.clone(),
                        config.clone(),
                        hub.clone(),
                    )
                    .with_pane_geom(pane_geom),
                )
                .autofocus(),
        )
        .child(
            Element::leaf()
                .widget(super::info_panel::PanelSplitHandle::new(chrome.clone()))
                .width(super::info_panel::HANDLE_W)
                .height_match()
                .visible_when({
                    let c = chrome.clone();
                    move || c.info.panel_visible()
                })
                .enabled_when({
                    let c = chrome.clone();
                    move || !c.locked.get()
                }),
        )
        .child(
            super::info_panel::build(viewer, chrome, config)
                .weight_when({
                    let c = chrome.clone();
                    move || 1.0 - c.info.ratio.get()
                })
                .visible_when({
                    let c = chrome.clone();
                    move || c.info.panel_visible()
                })
                .enabled_when({
                    let c = chrome.clone();
                    move || !c.locked.get()
                }),
        ),
)
```

- [ ] **Step 4: 接 tick 与主题重排**

`runtime/mod.rs` 的 `window_tick` 顶部主题翻转分支：

```rust
if self.seen_theme.replace(theme) != theme {
    self.chrome.invalidate_theme_caches();
    ctx.mark_dirty_all();
    ctx.mark_layout_dirty();
}
```

在 `apply_session_flags` 之后加：

```rust
self.chrome.info.tick(ctx, ctx.bounds().w, &config);
```

- [ ] **Step 5: 真正入口**

把 `info_panel/mod.rs` 里的临时 `open_properties` 换成：

```rust
pub fn open_properties(
    ctx: Option<&mut EventCtx>,
    chrome: &Rc<chrome::Chrome>,
    cfg: &Rc<RefCell<Config>>,
) {
    if chrome::borderless() {
        if let Some(ctx) = ctx {
            ctx.toast_err("无边框形态不显示信息面板");
        }
        return;
    }
    chrome.info.open_properties();
    chrome.info.persist(cfg);
    if let Some(ctx) = ctx {
        let cw = chrome.info.content_w.get();
        if cw > 0 && cw < PANEL_MIN_WINDOW {
            ctx.toast_err("窗口太窄，信息面板暂时收起；拉宽窗口后显示");
        }
        ctx.mark_layout_dirty();
        ctx.mark_dirty();
    }
}
```

把 `canvas.rs` 两处属性入口改成这个函数：
- 右键菜单项：用 `self.chrome` / `self.config` 克隆，`move |ctx| info_panel::open_properties(Some(ctx), &chrome, &cfg)`。
- `Action::Properties`：`info_panel::open_properties(ctx, chrome, cfg); true`（`ctx` 已经是 `Option<&mut EventCtx>`）。

- [ ] **Step 6: 跑测试与编译**

Run: `cargo test --lib`
Expected: PASS。

Run: `cargo check`
Expected: 无新增错误。

---

## Task 6: 标题栏过滤与窗口三键

**Files:**
- Modify: `main/src/ui/chrome.rs`
- Test: `main/src/ui/chrome.rs`

**Interfaces:**
- Produces: `TitleBtn::shown(panel_visible) -> &'static [TitleBtn]`；`title_btn_rect(tb, view, panel_visible)`。
- Consumes: `Chrome::info.panel_visible()`（Task 2/5）。

- [ ] **Step 1: 写失败测试**

在 `chrome.rs` 测试里加：

```rust
#[test]
fn panel_visible_keeps_only_lock_and_topmost_in_canvas() {
    let shown = TitleBtn::shown(true);
    assert_eq!(shown, &[TitleBtn::Lock, TitleBtn::Topmost]);
    let all = TitleBtn::shown(false);
    assert_eq!(all.len(), 5);
    assert!(all.contains(&TitleBtn::Close));
}

#[test]
fn panel_visible_reflows_lock_to_canvas_right_edge() {
    let view = (0.0, 0.0, 800.0, 600.0);
    let (x_lock, _, w_lock, _) = Chrome::title_btn_rect(TitleBtn::Lock, view, true);
    assert_eq!(x_lock + w_lock, 800.0, "Lock 应贴画布右缘");
    let (x_top, _, w_top, _) = Chrome::title_btn_rect(TitleBtn::Topmost, view, true);
    assert_eq!(x_top + w_top, x_lock, "Topmost 应在 Lock 左侧，不重叠");
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib ui::chrome::tests::panel_visible`
Expected: FAIL（`shown`/新参数不存在）。

- [ ] **Step 3: 实现过滤**

`chrome.rs` 的 `impl TitleBtn` 里加：

```rust
const PANEL_VISIBLE_BTNS: [TitleBtn; 2] = [TitleBtn::Lock, TitleBtn::Topmost];

/// 面板可见时画布只留置顶/锁定；三颗窗口键移到面板头部。
pub fn shown(panel_visible: bool) -> &'static [TitleBtn] {
    if panel_visible {
        &Self::PANEL_VISIBLE_BTNS
    } else {
        &Self::ALL
    }
}
```

把 `title_btn_rect` 改成：

```rust
fn title_btn_rect(
    tb: TitleBtn,
    view: (f32, f32, f32, f32),
    panel_visible: bool,
) -> (f32, f32, f32, f32) {
    let (_, _, vw, _) = view;
    const BTN_W: f32 = 40.0;
    const BTN_H: f32 = STRIP_H as f32;
    let shown = TitleBtn::shown(panel_visible);
    let i = shown.iter().position(|&x| x == tb).unwrap_or(0) as f32;
    let x = vw - (i + 1.0) * BTN_W;
    (x, 0.0, BTN_W, BTN_H)
}
```

把 `hit`、`paint`、`title_room` 的所有 `title_btn_rect(...)` 调用补上 `panel_visible` 参数；`title_room` 改成：

```rust
let shown = TitleBtn::shown(chrome.info.panel_visible());
let title_room = vw - (shown.len() as f32 * 40.0 + 8.0) - TITLE_PAD;
```

`hit` 的锁定分支与末尾循环都用：

```rust
let shown = TitleBtn::shown(self.info.panel_visible());
for &tb in shown { ... }
```

`paint` 的循环用：

```rust
let shown = TitleBtn::shown(chrome.info.panel_visible());
for &tb in shown.iter().rev() { ... }
```

- [ ] **Step 4: 更新既有测试调用**

`chrome.rs` 内所有 `Chrome::title_btn_rect(...)` 的测试调用补 `false`（面板隐藏）；新增测试用 `true`。若测试断言 5 颗按钮全部贴右缘，保持 `false`。

删除不再使用的 `TITLE_BTN_RESERVE` 常量（若没有别的读者），让 `cargo check` 不出现 dead_code 警告。

- [ ] **Step 5: 跑测试确认绿**

Run: `cargo test --lib ui::chrome`
Expected: PASS。

Run: `cargo test --lib`
Expected: PASS。

---

## Task 7: 「信息面板」菜单、文案与旧入口清理

**Files:**
- Modify: `main/src/ui/canvas.rs`
- Modify: `main/src/core/hotkey.rs`
- Modify: `main/src/ui/settings/decoders_tab.rs`
- Modify: `main/src/platform/exif.rs`
- Modify: `main/src/core/viewer/schedule.rs`
- Modify: `main/src/core/viewer/prefetch.rs`
- Modify: `main/src/ui/singleton.rs`
- Test: `main/src/ui/canvas.rs`（菜单构建的可测部分）

**Interfaces:**
- Consumes: `info_panel::open_properties`、`InfoPanel::set_open/persist`。
- Produces: 「界面 ▸ 信息面板」勾选项；「属性」菜单文案；旧窗口注释清理。

- [ ] **Step 1: 写失败测试**

在 `canvas.rs` 测试里加一个源码级接线钉子（与现有 `view_and_ops_groups_keep_one_separator_each` 同一手法）：

```rust
#[test]
fn appearance_menu_has_info_panel_toggle() {
    let src = production_src();
    let body = src
        .split("fn appearance_items")
        .nth(1)
        .expect("右键外观子菜单");
    let end = body.find("\n    fn ").unwrap_or(body.len());
    assert!(
        body[..end].contains("\"信息面板\""),
        "界面菜单必须有信息面板开关"
    );
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib ui::canvas::tests::appearance_menu_has_info_panel_toggle`
Expected: FAIL。

- [ ] **Step 3: 加菜单项**

`canvas.rs` 的 `appearance_items` 里，缩略图条项之后加：

```rust
if !chrome::borderless() {
    out.push({
        let on = self.chrome.info.open.get();
        let chrome = self.chrome.clone();
        let cfg = self.config.clone();
        let mut it = MenuItem::run(
            "信息面板",
            move |ctx| {
                let on = !chrome.info.open.get();
                chrome.info.set_open(on);
                chrome.info.persist(&cfg);
                ctx.mark_layout_dirty();
                ctx.mark_dirty();
            },
            on,
        );
        it.checkable = true;
        it.checked = on;
        it
    });
}
```

- [ ] **Step 4: 改文案与注释**

- 右键「属性…」→「属性」（`canvas.rs`）。
- 同步真实构造函数测试 `file_group_flattens_copy_and_keeps_properties_last` 的期望标签，以及源码级 `FILE_GROUP_MENU_KEYS` 里的 `"属性…"` → `"属性"`；负向臂里模拟旧布局的字符串保持原样。
- `core/hotkey.rs` 的 `Properties` 注释“属性窗” → “属性卡（右侧信息面板）”。
- `ui/settings/decoders_tab.rs` 里“仿 [`open_metadata`]”改成“仿属性卡的信息行”。
- `platform/exif.rs` 里“`ui::settings::metadata` 的属性窗”改成“属性卡”。
- `core/viewer/schedule.rs` 里“`refresh_metadata`（每帧比对…属性窗）”改成“属性卡”。
- `core/viewer/prefetch.rs` 里“`ui::settings::metadata`”改成“属性卡”。
- `runtime/mod.rs` 模块文档里“它们不像属性窗那样每帧刷新”改成“属性卡”。
- `canvas.rs` 里 `exec_action` 文档的“属性窗”改成“属性卡”。
- `ui/singleton.rs` 里“`settings::forget_metadata_window`”改成“`info_panel::properties::forget_window`”。

- [ ] **Step 5: 跑测试**

Run: `cargo test --lib`
Expected: PASS。

Run: `cargo check`
Expected: 无新增错误。

---

## Task 8: 文档、结构索引与门禁

**Files:**
- Modify: `main/HISTORY.md`
- Modify: `main/docs/架构总览.md`
- Modify: `main/docs/待办.md`
- Modify: `main/DOCS_MANIFEST.md`
- Modify: `main/目录结构说明.md`（由 `structure_gen.py --write` 生成）
- Modify: `.plan/m00-计划编号规范与当前索引.md`
- Modify: `main/docs/superpowers/specs/2026-11-14-info-panel-design.md`（去掉新文件的旧标记）
- Test: `docs_consistency`、`structure_gen --check`、`hygiene_scan`

- [ ] **Step 1: 更新结构文档**

Run: `python build/structure_gen.py --write`
Expected: `目录结构说明.md` 增加 `ui/info_panel/mod.rs`、`ui/info_panel/properties.rs`，行数/符号索引同步。

- [ ] **Step 2: 更新文档**

- `HISTORY.md`：按现有格式补一条 m342（右侧信息面板骨架 + 属性卡）。
- `docs/架构总览.md`：UI 层补“信息面板：右侧 dock，卡片可折叠/关闭，属性数据按需刷新”。
- `docs/待办.md`：第 6 项右侧信息面板标记完成；插件卡片留一条后续。
- `DOCS_MANIFEST.md`：登记 spec 与 plan。
- `.plan/m00-计划编号规范与当前索引.md`：登记本计划编号/状态。
- `docs/踩坑记录/踩坑记录.md`、`踩坑记录-4.md`、`踩坑记录-5.md`：把旧属性窗模块路径的指路改成 `ui/info_panel/properties.rs`（代码搬过去后指路要跟着走）；否则 `docs_consistency` j3 会把旧路径当悬空 `.rs` 引用。
- `docs/产品定位与边界.md`、`docs/建议筛选-值得开发清单.md`、`docs/待办.md` 里把“属性窗”改成“属性卡/信息面板”的现行表述；`踩坑记录` 里的历史叙述保留“属性窗”原话，只改 `.rs` 路径。
- spec 里两个新文件前缀的旧标记去掉（文件已存在）。
- 本 plan 里对原属性窗模块的 Delete 记录（File Structure 与 Task 3 两处）已改成“原属性窗模块”；文件删掉后，j3 不再把旧 `.rs` 路径当成悬空引用。

- [ ] **Step 3: 跑结构/文档门禁**

Run: `python build/structure_gen.py --check`
Expected: PASS。

Run: `cargo test --test docs_consistency`
Expected: PASS（j1–j19）。

- [ ] **Step 4: 跑静态门禁**

Run: `main\build\2_scan.bat`
Expected: 0 违规。

若 `2_scan.bat` 需要交互，改跑 `python build/hygiene_scan.py` 与 `python build/gate.py`（按现有脚本用法）。

- [ ] **Step 5: 跑宿主测试**

Run: `main\build\5_test.bat`
Expected: 全部通过（含新增测试）。

- [ ] **Step 6: 交付检查点**

- `main\build\4_build.bat` 只生成 `.temp/target/release/rupix.exe`，**不写 `.build`**（不跑 `full.bat`）。
- `main\build\6_deploy.bat` 才是把 exe / docs 复制进 `.build` 的步骤（同样不跑 `full.bat`）。
- 读 `.temp/target/release/rupix.exe`（j12 优先读它）或部署后的 `.build/rupix.exe` 实际字节数，更新 `main/docs/产品定位与边界.md` 里 `**当前 exe：**` 那一行（唯一机器锚）；否则 `docs_consistency` j12 会红。
- 更新体积后重跑 `cargo test --test docs_consistency`（或 `5_test.bat`），确认 j12 对**新 exe** 也绿。
- 人工验收：
  1. 右键「属性」→ 面板从右侧出现，属性卡展开，窗口三键在面板头部。
  2. 切图/翻页/换层/解码完成 → 卡片自动更新。
  3. 折叠卡片 → 正文收起；关闭卡片 → 卡片消失、空态提示；再点「属性」→ 卡片回来并展开。
  4. 拖面板左缘 → 宽度停在 240/480（或 40% 上限），不抖。
  5. 窗口缩到 <600 → 面板消失；拉宽 → 按原目标宽回来。
  6. 无边框 → 面板消失；点「属性」→ toast 提示。
  7. 锁定 → 面板不可交互，画布锁钮可解锁。
  8. 关闭程序再开 → 面板开/关、宽度、卡片、折叠状态恢复。
  9. 切换 `ui_theme` → 面板配色跟随；设置页仍是浅色。
  10. 插件结果窗仍按原样弹出；属性不再弹旧窗。

---

## Self-Review

**Spec coverage:**
- 面板行为、空态、卡片折叠/关闭/重加 → Task 2/4/5。
- 属性不再弹窗、旧窗退役 → Task 3/5/7。
- 宽度 240–480、40% 上限、窄窗隐藏、无边框隐藏、锁定禁用 → Task 2/4/5。
- 持久化四键 → Task 1/2/5。
- 属性自动更新、无图空态、EXIF 路径变化才读 → Task 3/4。
- 标题栏三键迁移 → Task 6。
- 入口/文案/toast → Task 5/7。
- 主题跟随 → Task 4（两份树）+ Task 5（主题翻转重排）。
- 文档/门禁 → Task 8。

**Placeholder scan:** 计划里没有 TBD/TODO；Task 7 的菜单接线用源码级钉子（与现有 `view_and_ops_groups_keep_one_separator_each` 同一手法）加人工验收覆盖。

**Type consistency:**
- `InfoPanel::width` 是 `i32`；配置 `info_panel_width` 是 `u32`，写回时 clamp 再 cast。
- `panel_visible(open: bool, borderless: bool, content_w: i32)` 与 `InfoPanel::refresh_visible` 的参数顺序一致。
- `build` / `build_one` / `PanelSplitHandle::new` 的 `Rc<Chrome>` / `Rc<RefCell<Config>>` 与 `build_root`、`window_tick` 的现有类型一致。
- `refresh_metadata_if(viewer, wanted)` 与 runtime 的 `wanted` 计算一致。
