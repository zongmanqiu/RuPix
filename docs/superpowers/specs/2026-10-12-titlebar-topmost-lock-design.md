# 设计：标题栏置顶/锁住按钮（Topmost + Lock）

日期：2026-10-12 · 状态：用户已批准设计，待实施计划
参考：同类项目里的便签实现——原生 Win32 自绘，**只借鉴语义不抄代码**；调研结论已并入本设计。

## 1. 需求（用户拍板）

- 标题栏是最小化左侧增加两颗按钮：**置顶**（= 右键菜单「窗口置顶」同一开关）与**锁住**。
- 置顶 = 当前窗口临时态（`ws.topmost`，不落盘），与右键菜单项同源。
- 锁住 = **窗口临时态（不落盘）**：锁定后除非解锁，无法再与图片窗口进行任何交互。
  - 鼠标：只留锁住按钮可点；最小化/最大化/关闭/置顶全禁，窗口不可拖动。
  - 键盘：**全封，含 Esc**（Esc 关窗兜底也要吞掉）。
  - 右键：**不弹菜单**。
  - 解锁 = 再点一次锁住按钮。无快捷键、无超时。

## 2. 方案选型（已定 A）

A（采用）：会话态 + 自有输入入口拦截。`Chrome` 加 `locked: Cell<bool>`（样板：`checker`/`filmstrip` 会话态）；拦截点全在 `main/src/ui`。
B（否决）：改 `.3rd/windui` 框架做事件过滤——侵入框架，升级即丢。
C（否决）：只在业务动作 `exec_*` 拦——散、漏（拖窗 slop、悬停、Esc 兜底）。

## 3. 按钮与布局

- `TitleBtn` 增变体 `Topmost`、`Lock`；`ALL = [Close, Maximize, Minimize, Lock, Topmost]`（下标越大越靠左，一处真源同时驱动 `title_btn_rect` / `hit` / `paint`）。
- 视觉顺序（左→右）：**📌 🔒 最小化 最大化 ×**。
- 尺寸沿用 `BTN_W=40, BTN_H=STRIP_H=40`，无间距（与现 Windows 风一致；CurSeen 的 2px gap 不引入）。
- 标题文本矩形宽 `vw - 140` → `vw - 220`（3×40→5×40）。

## 4. 字形与状态显示

| 钮 | 字形 | 字体 | 状态表达 |
|---|---|---|---|
| Topmost | `📌` U+1F4CC 恒定 | Segoe UI Symbol | 置顶态 = `TEXT_ACTIVE` 绿（同 Checker/Fullscreen 激活模式）；未置顶 = 常规色 |
| Lock | `🔓` U+1F513 / `🔒` U+1F512 换字形 | Segoe UI Symbol | 锁定态换字形（CurSeen 已验证两字形在 Symbol 字体下为单色、不回退彩色 emoji） |
| Min/Max/Close | 现有 MDL2 码位不变 | Segoe MDL2 Assets | — |

- `TitleBtn::font_family() -> Option<&'static str>`（仿 `Btn::text_glyph()`）：Topmost/Lock 返回 `"Segoe UI Symbol"`，其余 `None`；`paint` 按返回值设 `TextStyle::family`。
- CurSeen 的 📌 45° 旋转（`SetWorldTransform`）不移植——`Canvas::draw_text` 无变换接口，用绿色表达激活态，与本项目视觉语言一致。

## 5. 锁定语义（状态与拦截地图）

状态：`Chrome { locked: Cell<bool>, ... }`。会话态、不落盘、不进 Config（与 CurSeen 同口径）。置顶真值**不新增字段**：读 `window_state().topmost`（`chrome.rs:513` 的 `let _ = topmost_on` 改为真用）。

| 入口 | 位置 | 锁定时行为 |
|---|---|---|
| 标题栏命中 | `Chrome::hit`（chrome.rs） | 只返回 `Press::TitleBtn(Lock)`，其余全 `None`（含 Min/Max/Close/Topmost 与拖动区） |
| 悬停 | `Chrome::activity` | 照常计算（锁钮 hover/pressed 才有反馈），但 `paint` 对禁用钮不画 hover 底 |
| 指针总入口 | `canvas.rs on_pointer` | 右键分支早退（不弹菜单）；中键平移、拖窗升级（`lmb_pending`/`start_window_drag`）自然被 `hit=None` 断掉；双击分支早退 |
| 滚轮 | `canvas.rs on_pointer` 滚轮段 | 早退 |
| 键盘（焦点路径） | `canvas.rs on_key` → `handle_key` | `handle_key` 见锁早退并 **return true（消费）** |
| 键盘（兜底路径） | `runtime/mod.rs App::on_shortcut` → `handle_key` | 同上；**消费是关键**——`on_shortcut` 排在框架 Tab/Escape 兜底之前，return true 才能吞掉 Esc 关窗（windui `core.rs:3339` `Key::Escape if resolve_close()`） |
| 自绘 × / min / max / 置顶 | `canvas.rs exec_title_btn` | 守卫：锁定时**只放行 `Lock`**，其余（含 Topmost）直接 return（纵深防御；自绘 × 走 `ctx.request_close()` 会绕过 `on_close_request`，必须自拦） |
| 系统关窗（Alt+F4 / 系统 ×） | `runtime/mod.rs on_close_request`（已有接线，runtime/mod.rs:259） | 锁定时 deny（CurSeen `WM_CLOSE` 同口径：锁定吞关闭） |
| 置顶钮点击 | `exec_title_btn(Topmost)` | `ctx.toggle_topmost()`（与右键菜单 `canvas.rs:470` 同一 API） |
| 锁钮点击 | `exec_title_btn(Lock)` | `chrome.locked.set(!locked)`；锁定瞬间清掉 pressed/hover 残留 |

关闭路径完整性核对：Esc（on_shortcut 消费）、Alt+F4/系统×（on_close_request deny）、自绘×（exec_title_btn 守卫）三条全覆盖；应用主动退出（`ctx.request_close` / 程序逻辑）不受影响。

## 6. 绘制

- 锁定时禁用钮：前景用暗色（现有 `TEXT` 与底色的 dim 混合，参照 CurSeen `mix_color(bg, ink, 0.35)` 的意图，具体色值对齐本项目调色板）；不画 hover/pressed 底。
- hover/pressed 底色条件加 `&& btn_active`（active = 未锁定 或 该钮是 Lock）。
- `paint` 已有 `topmost_on: bool` 形参——置顶绿随它。

## 7. 测试（TDD，先红后绿）

1. 扩展 `every_button_hits_its_own_rect` / `every_button_is_labelled_and_drawable` 至 5 颗。
2. 新增：锁定时 `hit()` 只认锁钮（锁态矩阵——信息密度最高，CurSeen `lock_disables_everything_but_add_and_lock` 的移植）。
3. 新增：字形随状态（🔒/🔓）与 `font_family()` 分流。
4. 新增：`title_btn_rect` 五钮不重叠且贴右锚点（CurSeen `hit_btn_geometry_right_aligned` 移植）。
5. 新增：`handle_key` 锁定返回 true（消费）——键盘全封的可测面。
6. 全量回归：121 现有测试 + 新增；`docs_consistency` 七判据。

## 8. 文档

- `目录结构说明.md`：chrome.rs / canvas.rs 行数与描述如变动则同步。
- HISTORY：本功能落地后按既有惯例补一条。
- 规划文件（task_plan/findings/progress，项目根）：新功能单独记录。

## 9. 实现时验证点

- windui `TextStyle::family` 切换对 U+1F4CC/U+1F512/U+1F513 的实际渲染（Segoe UI Symbol 单色性）——跑起来肉眼验一次。
- `on_close_request` 现有实现（runtime/mod.rs:259 附近）的 deny 语义（返回值形态）。
- 最小模式下标题条 hover 显隐与新按钮的交互（按钮跟随现有 title_visible 逻辑，不特判）。

## 10. 非目标

- 不持久化置顶/锁住（不进 Config/ini）。
- 不给标题栏按钮加 tooltip（CurSeen 无；后续有需求另开）。
- 不做 📌 旋转动画/图标字体改造。
