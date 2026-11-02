# 标题栏置顶/锁住按钮 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 自定义标题栏在最小化左侧新增「置顶📌」（=`ws.topmost` 同源开关）与「锁住🔓/🔒」（会话态、锁定后全交互封锁只留解锁）两颗按钮。

**Architecture:** 会话态 + 自有输入入口拦截（方案 A）。`Chrome.locked: Cell<bool>` 一枚真源；`TitleBtn::{Topmost,Lock}` 进 `ALL` 后布局/命中/绘制三处自动跟随；键盘全封靠「消费即吞」（`handle_key`/`on_key` 返回 true，抢在框架 Tab/Escape 兜底之前）；系统关窗靠 `on_close_request` 拒绝。设计全文：`docs/superpowers/specs/2026-10-12-titlebar-topmost-lock-design.md`。

**Tech Stack:** Rust + windui 0.20.0（path 依赖 `.3rd/wind-ui-rust-0.20.0`）；测试 `cargo test --release`。

## Global Constraints

- 项目**非 git 仓库**：所有 "Commit" 步骤一律替换为「跑测试拿绿即过关」；禁止 `git` 命令。
- 所有 cargo 命令 `workdir=E:\tmp\AI.Agent\RuPix\main`（PowerShell）。
- 快循环：`cargo test --release --lib <filter>`；全量回归放任务末尾（编译约 45s，命令会自动转后台，用 bash_watch 等）。
- **第一方 clippy 零警告**（AGENTS.md:227）；`cargo fmt --all` 必须先于 clippy。
- 禁止在 `main/` 根放临时文件；不修改 `.3rd/windui`。
- 颜色常量已有真源：`TEXT`(208)、`TEXT_MUTED`(140)、`TEXT_ACTIVE`(126,201,126 绿)、`PILL_BG`、`BTN_HOVER_BG`、`ICON_FONT="Segoe MDL2 Assets"`（chrome.rs:45-53）——**不新造色**。
- 字面 emoji 一律用 Rust 转义（`"\u{1F4CC}"`），与现有 `"\u{E921}"` 风格一致，避免源码编码坑。
- 用户语义（spec §1）：锁定 = 鼠标只留锁钮 / 键盘全封含 Esc / 右键不弹菜单 / 解锁=再点锁钮；置顶态显示=绿色 `TEXT_ACTIVE`。

---

### Task 1: chrome.rs — 锁态字段、双钮枚举、布局/命中/绘制、单元测试

**Files:**
- Modify: `main/src/ui/chrome.rs`（TitleBtn 73-91、Chrome 结构 173-208、new 219-238、hit 360-384、paint 标题段 442-514、tests 618-654）

**Interfaces:**
- Produces:
  - `pub enum TitleBtn { Minimize, Maximize, Close, Topmost, Lock }`；`ALL = [Close, Maximize, Minimize, Lock, Topmost]`（右→左序，下标越大越靠左）
  - `TitleBtn::glyph(self, locked: bool) -> &'static str`（签名变更！Lock 随 locked 换 🔒/🔓）
  - `TitleBtn::font_family(self) -> Option<&'static str>`（私有即可；Topmost/Lock = Some("Segoe UI Symbol")）
  - `Chrome { locked: Cell<bool>, .. }`，`new()` 初始化 `Cell::new(false)`
  - `Chrome::hit` 锁定门：只返回 `Press::TitleBtn(Lock)` 或 None
- Consumes: 现有 `Press`、`title_btn_rect`（随 ALL 自动跟随，无需改）、颜色常量。

- [ ] **Step 1: 写失败测试**（追加到 chrome.rs `mod tests`；新增变体/字段不存在 → 编译失败即 Rust 的 RED）

```rust
    /// 5 颗标题按钮：各自矩形中央必须命中自己（与工具栏同款口径）。
    #[test]
    fn every_title_button_hits_its_own_rect() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        for tb in TitleBtn::ALL {
            let (x, y, w, h) = Chrome::title_btn_rect(tb, view);
            let hit = chrome.hit((x + w / 2.0, y + h / 2.0), view);
            assert_eq!(hit, Some(Press::TitleBtn(tb)), "{tb:?} 命中不了自己");
        }
    }

    /// 右锚、不重叠：ALL 是右→左序，左边一颗的右缘必须 ≤ 右边一颗的左缘；
    /// 关闭钮贴视口右缘。
    #[test]
    fn title_buttons_are_right_anchored_without_overlap() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let vw = view.2;
        let (cx, _, cw, _) = Chrome::title_btn_rect(TitleBtn::Close, view);
        assert!((cx + cw - vw).abs() < f32::EPSILON, "关闭钮该贴右缘");
        for pair in TitleBtn::ALL.windows(2) {
            let (rx, _, rw, _) = Chrome::title_btn_rect(pair[0], view); // 更靠右
            let (lx, _, lw, _) = Chrome::title_btn_rect(pair[1], view); // 更靠左
            assert!(lx + lw <= rx + f32::EPSILON, "{:?}/{:?} 矩形重叠", pair[0], pair[1]);
        }
    }

    /// 字形与字体：emoji 颗走 Segoe UI Symbol；锁字形随锁定态切换；
    /// 置顶字形恒定（状态靠颜色表达，见 paint）。
    #[test]
    fn title_button_glyphs_and_fonts_reflect_state() {
        for tb in TitleBtn::ALL {
            assert!(!tb.glyph(false).is_empty(), "{tb:?} 缺未锁字形");
            assert!(!tb.glyph(true).is_empty(), "{tb:?} 缺锁定字形");
        }
        assert_eq!(TitleBtn::Lock.glyph(false), "\u{1F513}"); // 🔓
        assert_eq!(TitleBtn::Lock.glyph(true), "\u{1F512}"); // 🔒
        assert_eq!(TitleBtn::Topmost.glyph(false), TitleBtn::Topmost.glyph(true));
        for tb in [TitleBtn::Topmost, TitleBtn::Lock] {
            assert_eq!(tb.font_family(), Some("Segoe UI Symbol"), "{tb:?} 应走 Symbol 字体");
        }
        for tb in [TitleBtn::Minimize, TitleBtn::Maximize, TitleBtn::Close] {
            assert_eq!(tb.font_family(), None, "{tb:?} 应留在 MDL2");
        }
    }

    /// 锁定禁用矩阵（移植 CurSeen `lock_disables_everything_but_add_and_lock`）：
    /// 锁定后整窗只有锁住按钮命中得到——标题栏其余钮、工具栏、箭头一并哑掉。
    #[test]
    fn locked_hit_only_recognizes_the_lock_button() {
        let view = (0.0, 0.0, 800.0, 600.0);
        let chrome = Chrome::new(true);
        chrome.locked.set(true);
        for tb in TitleBtn::ALL {
            let (x, y, w, h) = Chrome::title_btn_rect(tb, view);
            let want = (tb == TitleBtn::Lock)
                .then_some(Press::TitleBtn(TitleBtn::Lock));
            assert_eq!(
                chrome.hit((x + w / 2.0, y + h / 2.0), view),
                want,
                "锁定时 {tb:?} 的命中不对"
            );
        }
        for b in Btn::ALL {
            let (x, y, w, h) = Chrome::btn_rect(b, view);
            assert_eq!(
                chrome.hit((x + w / 2.0, y + h / 2.0), view),
                None,
                "锁定时工具栏 {b:?} 不该命中"
            );
        }
        for a in [Arrow::Prev, Arrow::Next] {
            let (cx, cy) = Chrome::arrow_center(a, view);
            assert_eq!(chrome.hit((cx, cy), view), None, "锁定时箭头 {a:?} 不该命中");
        }
    }
```

- [ ] **Step 2: 跑测试确认 RED**

Run: `cargo test --release --lib chrome`
Expected: 编译失败，报 `Topmost`/`Lock` 不存在、`locked` 字段不存在、`glyph` 参数不符（RED 理由正确：功能未实现）。

- [ ] **Step 3: 实现（按序小步）**

3a. `Chrome` 结构（173 行区域）加字段，`new()`（219-238）加初始化：

```rust
    /// 锁定（🔒）：会话态、不落盘（与 `checker`/`filmstrip` 同口径）。
    /// 锁定后整窗交互封锁，只留锁住按钮解锁——语义与拦截地图见
    /// `docs/superpowers/specs/2026-10-12-titlebar-topmost-lock-design.md` §5。
    pub locked: Cell<bool>,
```

```rust
            locked: Cell::new(false),
```

3b. `TitleBtn` 枚举与方法（73-91）：

```rust
/// 标题栏窗口按钮。`ALL` 是**右→左序**（下标越大越靠左）：布局、命中、绘制
/// 三处都以它为唯一真源，加按钮只改这一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBtn {
    Minimize,
    Maximize,
    Close,
    /// 置顶（= 右键菜单「窗口置顶」同一开关，读写 `ws.topmost`）。
    Topmost,
    /// 锁住：锁定后整窗交互封锁，再点解锁（会话态 `Chrome::locked`）。
    Lock,
}

impl TitleBtn {
    /// Windows 标准顺序贴右缘，新按钮向左排：× ▢ _ 📌 🔒（视觉左→右）。
    pub const ALL: [TitleBtn; 5] = [
        TitleBtn::Close,
        TitleBtn::Maximize,
        TitleBtn::Minimize,
        TitleBtn::Lock,
        TitleBtn::Topmost,
    ];

    fn glyph(self, locked: bool) -> &'static str {
        match self {
            TitleBtn::Minimize => "\u{E921}", // ChromeMinimize
            TitleBtn::Maximize => "\u{E922}", // ChromeMaximize
            TitleBtn::Close => "\u{E8BB}",    // ChromeClose
            TitleBtn::Topmost => "\u{1F4CC}", // 📌 状态用颜色表达（paint 读 topmost_on）
            TitleBtn::Lock if locked => "\u{1F512}", // 🔒
            TitleBtn::Lock => "\u{1F513}",           // 🔓
        }
    }

    /// 标题按钮字形所在字体：置顶/锁住是 emoji 区码位，MDL2 没有，走
    /// Segoe UI Symbol（CurSeen 已验证该字体下这两个字形单色、不回退彩色 emoji）。
    fn font_family(self) -> Option<&'static str> {
        match self {
            TitleBtn::Topmost | TitleBtn::Lock => Some("Segoe UI Symbol"),
            _ => None,
        }
    }
}
```

3c. `hit()`（360-384）头部加锁定门（放在 `self.last_move`/`last_pos` 设置之后、按钮循环之前）：

```rust
        // 锁定：整窗只认锁住按钮（解锁唯一入口）——其余命中一律 None，
        // 拖窗/工具栏/箭头自然全断（用户拍板 2026-10-12，spec §5）。
        if self.locked.get() {
            let (x, y, w, h) = Self::title_btn_rect(TitleBtn::Lock, view);
            return if pos.0 >= x && pos.0 < x + w && pos.1 >= y && pos.1 < y + h {
                Some(Press::TitleBtn(TitleBtn::Lock))
            } else {
                None
            };
        }
```

3d. `paint` 标题段（442-486）：标题宽 140→220；按钮循环体改为——active 门控 hover 底、前景按「锁定禁用=TEXT_MUTED / 置顶激活=TEXT_ACTIVE / 其余=TEXT」、字体按 `font_family()`；并删除 513 行的 `let _ = topmost_on;`（该参数从此在标题段使用）。

```rust
        // 标题文本：左对齐，显示传入的 title（文件名或 RuPix）。
        // 右侧让出 5 颗按钮：5 × 40 = 200 + 20 留白。
        let title_ts = TextStyle::new(13.0);
        canvas.draw_text(
            title,
            Rect::new(vx as i32 + 12, vy as i32, vw as i32 - 220, STRIP_H),
            TEXT,
            Align::Start,
            &title_ts,
        );
        // 窗口按钮（📌 🔒 最小化 最大化 ×），按 Windows 标准顺序从右向左绘制
        let locked = chrome.locked.get();
        for &tb in TitleBtn::ALL.iter().rev() {
            let (bx, by, bw, bh) = Chrome::title_btn_rect(tb, view);
            let hovered = chrome.hover_title_btn.get() == Some(tb);
            let pressed = chrome.pressed_title_btn.get() == Some(tb);
            // 锁定时除锁钮外全是"禁用"：不画 hover/pressed 底。
            let active = !locked || tb == TitleBtn::Lock;
            if active && (hovered || pressed) {
                let c = if pressed {
                    Color::rgba(255, 255, 255, 44)
                } else if tb == TitleBtn::Close {
                    // 关闭按钮悬停时红色背景
                    Color::rgba(232, 17, 35, 220)
                } else {
                    BTN_HOVER_BG
                };
                let (lx, ly, lw, lh) = local(bx, by, bw, bh);
                canvas.fill_rect(lx, ly, lw, lh, &Paint::fill(c));
            }
            let fg = match tb {
                // 置顶态：绿色高亮（同 Checker/Fullscreen 激活模式）。
                TitleBtn::Topmost if topmost_on => TEXT_ACTIVE,
                _ if !active => TEXT_MUTED,
                _ => TEXT,
            };
            let family = tb.font_family().unwrap_or(ICON_FONT);
            let (tx, ty, tw, th) = local(bx, by, bw, bh);
            canvas.draw_text(
                tb.glyph(locked),
                Rect::new(tx as i32, ty as i32, tw as i32, th as i32),
                fg,
                Align::Center,
                &TextStyle {
                    family: Some(family),
                    ..TextStyle::new(14.0)
                },
            );
        }
```

3e. 确认 `title_btn_rect`（350-358）无需改动（`ALL.position` 自动跟随）；确认 `arrow_center` 可见性为当前文件私有（测试同文件可调用）。

- [ ] **Step 4: 跑 chrome 测试拿绿**

Run: `cargo test --release --lib chrome`
Expected: 全 passed（新增 4 条 + 原有）。

- [ ] **Step 5: 全量 lib 回归（本任务关门）**

Run: `cargo test --release --lib`
Expected: 全 passed。若 `canvas.rs` 因 `glyph()` 签名变更编译失败属预期耦合——**立即做 Task 2 Step 3 的 exec/调用点最小修补**（只改 `tb.glyph(locked)` 调用与 `exec_title_btn` 签名占位），不展开 Task 2 其余内容；或把 Task 1+2 合并执行（两任务同文件耦合紧，允许合并）。

---

### Task 2: canvas.rs — 点击动作、输入拦截、键盘消费

**Files:**
- Modify: `main/src/ui/canvas.rs`（exec_title_btn 709-715、on_key 937-955、on_pointer 995-1205、Up 分支调用点 1161、tests 1276+）

**Interfaces:**
- Consumes: Task 1 的 `TitleBtn::{Topmost,Lock}`、`Chrome::locked`；已有 `ctx.toggle_topmost()`（右键菜单 canvas.rs:470 同 API）。
- Produces: `fn exec_title_btn(ctx: &mut EventCtx, chrome: &Rc<Chrome>, tb: TitleBtn)`（签名变更）；`handle_key` 锁定消费语义（runtime 复用，无签名变化）。

- [ ] **Step 1: 写失败测试**（canvas.rs `mod tests` 追加；Viewer 构造照抄 `zoomed_canvas` 的 `Decoder::spawn_with` 通道法）

```rust
    /// 锁定时键盘全封：任何键都返回 true（=已消费），且不许有业务副作用。
    /// 消费是关键——`App::on_shortcut` 排在框架 Tab/Escape 兜底之前，
    /// 返回 true 才能吞掉 Esc 关窗（windui core.rs 的 `Key::Escape if resolve_close()`）。
    #[test]
    fn locked_handle_key_consumes_everything() {
        use crate::core::decode::Decoder;
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = Rc::new(RefCell::new(Config::default()));
        let v = Rc::new(RefCell::new(Viewer::new(Rc::new(Decoder::spawn_with(tx)), cfg)));
        let chrome = Chrome::new(false);

        // 未锁：T 正常切换（对照组，证明测试不是空转）。
        assert!(handle_key(&v, &chrome, Key::Char('t')));
        assert!(chrome.filmstrip.get());

        chrome.locked.set(true);
        for key in [Key::Left, Key::Char('f'), Key::Char('t'), Key::Escape, Key::Char('+')] {
            assert!(handle_key(&v, &chrome, key), "锁定时 {key:?} 应被消费");
        }
        // 副作用检查：翻页没走（索引仍 0）、缩放没走、T 没把条切回去。
        assert!(chrome.filmstrip.get(), "锁定时 T 不该切换缩略图条");
    }
```

- [ ] **Step 2: 跑测试确认 RED**

Run: `cargo test --release --lib locked_handle_key`
Expected: FAILED——未锁定语义已过的部分之外，`locked` 后 `handle_key(Key::Left))` 返回 false（当前实现会真翻页返回 true？——注意：当前 `Key::Left` 分支 `v.step(-1)` 返回值是 step 的返回……`v.step` 返回 bool，空目录下可能 false）。**若因空 Viewer 行为导致断言含糊，改用纯副作用断言**：锁定后 `handle_key(Key::Char('t'))` 必须 true 且 filmstrip 不翻转（此条一定 RED：现在会翻转）。

- [ ] **Step 3: 实现**

3a. `exec_title_btn`（709-715）整体替换：

```rust
fn exec_title_btn(ctx: &mut EventCtx, chrome: &Rc<Chrome>, tb: TitleBtn) {
    // 锁定纵深防御：hit 已只认锁钮，这里再拦一道——自绘 × 走
    // `ctx.request_close()` 会绕过 `on_close_request`，必须自拦。
    if chrome.locked.get() && tb != TitleBtn::Lock {
        return;
    }
    match tb {
        TitleBtn::Minimize => ctx.minimize(),
        TitleBtn::Maximize => ctx.toggle_maximize(),
        TitleBtn::Close => ctx.request_close(),
        // 与右键菜单「窗口置顶」同一 API、同一真值（ws.topmost）。
        TitleBtn::Topmost => ctx.toggle_topmost(),
        TitleBtn::Lock => {
            chrome.locked.set(!chrome.locked.get());
            // 锁定瞬间清掉按下残留；hover 留着（指针还在锁钮上才有反馈）。
            chrome.pressed_title_btn.set(None);
            ctx.mark_dirty();
        }
    }
}
```

3b. 调用点（1159-1166 Up 分支）：`exec_title_btn(ctx, tb);` → `exec_title_btn(ctx, &self.chrome, tb);`

3c. `on_key`（937-955）：在 `if !k.pressed` 之后、Esc 特判之前插入：

```rust
                // 锁定：键盘全封。返回 true = 已消费，Esc 也就轮不到框架的关窗兜底。
                if self.chrome.locked.get() {
                    return true;
                }
```

3d. `handle_key`（1214 起）函数体第一行插入（覆盖 `on_shortcut` 兜底路径，含 Esc）：

```rust
    // 锁定：键盘全封。返回 true = 已消费——`App::on_shortcut` 这条兜底路
    // 排在框架 Tab/Escape 之前，消费掉框架就收不到 Esc 关窗键。
    if chrome.locked.get() {
        return true;
    }
```

3e. `on_pointer` 四处守卫：

- Wheel 分支（998）第一行：`if self.chrome.locked.get() { return true; }`
- Down-Left 分支（1087）`self.chrome_activity(pos);` 之后：
  - `nav_grab` 行（1094）改 `if !self.chrome.locked.get() && self.nav_grab(pos) {`
  - `filmstrip::hit` 段（1117）外包 `if !self.chrome.locked.get() { ... }`（整段：记 pressed 那几行）
  - 双击段（1124）`if p.click_count >= 2 {` 内最前面加 `if self.chrome.locked.get() { return true; }`
  - 结尾记账段（1139-1143）改：

```rust
                if self.chrome.locked.get() {
                    return true; // 锁定：吞掉这一击，不记拖窗账、不开平移
                }
                if p.mods.ctrl {
                    self.viewer.borrow_mut().begin_drag(pos);
                } else {
                    self.lmb_pending.set(Some(pos));
                }
```

- Down-Middle 分支（1147）第一行：`if self.chrome.locked.get() { return true; }`
- Down-Right 分支（1198）第一行：`if self.chrome.locked.get() { return true; } // 锁定：不弹菜单`

（Move/Up/Leave 分支不加守卫：`lmb_pending`/`dragging`/`nav_drag` 在锁定下无源头，已死路径。）

- [ ] **Step 4: 跑测试拿绿**

Run: `cargo test --release --lib canvas`
Run: `cargo test --release --lib handle_key`
Expected: 全 passed。

- [ ] **Step 5: 全量 lib 回归**

Run: `cargo test --release --lib`
Expected: 全 passed（chrome + canvas + 其余）。无 git：以测试绿为过关。

---

### Task 3: runtime/mod.rs — on_close_request 锁定拒关

**Files:**
- Modify: `main/src/runtime/mod.rs`（on_close_request 259-265）

**Interfaces:**
- Consumes: `Chrome::locked`；已有 `let chrome` 变量（244-246 的 on_shortcut 闭包已 clone 过一份，照抄手法）。
- Produces: 锁定时 `on_close_request` 返回 false（吞 Alt+F4 / 系统 ×；不影响 `ctx.request_close()` 应用主动关）。

- [ ] **Step 1: 确认 windui 语义（读码，不猜）**

Run: 读 `.3rd/wind-ui-rust-0.20.0/src/core.rs` `on_close_request` 文档（约 2186-2200、2444 附近）与 `resolve_close`（3339 附近）。
Expected: 确认「返回 false = 拒绝关闭」。若语义相反（true=拒绝），Step 2 的返回值取反并在注释里记明。

- [ ] **Step 2: 实现**

```rust
    // 退出权威保存（M1）：防抖没等到的改动在这里兜底落盘。
    .on_close_request({
        let config = config.clone();
        let chrome = chrome.clone();
        move |_| {
            crate::core::config::flush_save(&config.borrow());
            // 锁定（🔒）吞掉用户侧关窗（Alt+F4 / 系统 ×），CurSeen WM_CLOSE 同口径；
            // 应用主动关（ctx.request_close / 程序退出）不走本决策链，不受影响。
            !chrome.locked.get()
        }
    })
```

- [ ] **Step 3: 编译 + 回归**

Run: `cargo test --release --lib`
Expected: 全 passed。

---

### Task 4: 文档、全量门禁、部署、人工验收

**Files:**
- Modify: `main/HISTORY.md`（追加一条）、项目根 `progress.md`（追加本功能记录）
- Check: `main/目录结构说明.md`（chrome.rs/canvas.rs 行数与描述若漂移则同步；`docs_consistency` 七判据会先报）

**Interfaces:** 无代码接口。

- [ ] **Step 1: fmt + clippy**

Run: `cargo fmt --all`
Run: `cmd /c "cargo clippy --release --all-targets 2>&1"` 后过滤第一方路径
Expected: 第一方 0 warning（仅 `.3rd/windui` 预存 1 条可留）。

- [ ] **Step 2: 全量测试**

Run: `cargo test --release`
Expected: lib + config_compat + config_pipeline_e2e + docs_consistency 全绿（数量 = 121 + 本计划新增）。

- [ ] **Step 3: 构建 + 部署**

Run: `cargo build --release`
Run: `python build\deploy.py`
Expected: PE 图标门禁过、md5 MATCH、deploy OK。

- [ ] **Step 4: 文档落笔**

- HISTORY.md 追加：标题栏新增置顶/锁住按钮（会话态、锁定全封、📌绿=置顶态）。
- 项目根 progress.md 追加本功能各任务完成记录。

- [ ] **Step 5: 人工验收（请用户实机）**

- 标题栏左起：📌 🔒 最小化 最大化 ×；📌 点击置顶/取消（右键菜单勾选态同步）；置顶时 📌 变绿。
- 🔒 点击锁定：min/max/×/📌 变暗且点不动、拖不动窗、滚轮/键盘/右键全无反应、🔒 变 🔒 形态；再点解锁恢复。
- 锁定按 Alt+F4 窗口不关；Esc 不关。
- 极简模式悬停顶部条按钮照常出现。
