# PATCHES.md —— 第三方库补丁登记簿

登记对仓库外第三方源码 `.3rd/wind-ui-rust-0.23.0`（本地 path 依赖）的**修改**。
机制与生命周期见同目录 `README.md`。**只增不减**：本文件章节只追加，历史归档永不删节。

修改后完整文件归档在 [`wind-ui-rust-0.23.0/`](./wind-ui-rust-0.23.0/)。
0.23.0 起不再保留 `before/` 原始文件副本（0.23.0 是在用户下载的上游源码上原地打补丁的，
没有留下原始副本；日后升级时请下载新版本源码自行 diff，或对照上游 release tarball）。
`.3rd/` 工作副本的每个改动处都标了 `★ RuPix 补丁 0NN`。这条承诺**现在是机器核对的**，两只眼睛各看一段：
`main/build/hygiene_scan.py` 第 7 查 `patch_parity` 管**章节 ↔ 标记**——① 本文每个 `## 补丁 0NN` 章节，除非正文写了
`落点状态：0.23.0 无落点`，否则 `.3rd` 里必须至少有一处 `RuPix 补丁 0NN` 标记（缺＝补丁丢了，或标记写法漂了）；
② 反过来 `.3rd` 里出现的号必须有章节（多＝没登记就改了上游）。
**归档 ↔ 工作副本逐字节**那一段由 `main/tests/docs_consistency.rs` 的 `j13_patch_archive_matches_worktree` 守
（`wind-ui-rust-0.23.0/` 那份归档是重放依据，漂了就等于没有；工作树里缺归档中某个文件也算漂）。
★ 这条判据**只在 j13 一处定义**，第 7 查刻意不重复它——同一判据立两个定义点＝改一个漏一个。
★ 目前**无落点**的两个号：`02`（0.23.0 上游已内建 `on_blur`）、`05`（被补丁 `014` 取代，标记归在 014 名下）——
   两节正文各自写明了原因，不是漏更。
0.20.0 时代的补丁与归档见本文各章节及迁移记录（文末）——0.20.0 库与归档已按 2026-10-13
裁决删除，历史章节保留作为重放依据。

---

## 补丁 01 —— 托盘图标运行期显隐（"关闭到托盘"即时生效）

- 编号：01
- 涉及上游文件（均在 `wind-ui-rust-0.20.0/src/platform/`）：
  - `tray.rs`（声明层：`TrayOp` / `TrayHandle` / `Tray`）
  - `win32/tray.rs`（win32 落地）
  - `win32/mod.rs`（应用 op + shell 重启重登记过滤）
  - `macos/tray.rs`（macOS 对齐）
- 应用侧接线：`main/src/ui/tray.rs`、`main/src/runtime/mod.rs`、`main/src/core/config.rs`、`main/src/ui/settings.rs`

### 问题（用户实测）
设置项「关闭到托盘」要么改了要重启才"可能"生效，要么根本不生效——用户诉求是：
**勾上当场出现托盘图标、关窗当场收进托盘而不是退出**；**运行期改 ini 也要即时跟上**，不要重启。

### 根因（实测，非推测）
windui 0.20.0 的托盘是**装配期一次性**设施：
- `App::tray()` 只能随 `App` 装一次，`TrayHandle` 在 `run()` 里一次性装好，之后没有运行期
  显隐 API；
- `App::hide_on_close()` 是装配期布尔，运行期改不了——"关窗是退出还是隐藏"被焊死在启动配置上。

也就是说，上游把「托盘装不装」「关窗退不退出」都当成了启动参数，天然不支持运行期切换。
（实测取证：grep 全仓，`TrayHandle` 只暴露 `set_tooltip`/`set_menu`/`set_icon` 这类，**没有 set_visible**。）

### 修复
1. **声明层** `platform/tray.rs`：
   - `TrayOp` 新增变体 `SetVisible(bool)`；
   - `TrayHandle::set_visible(on)` —— 入队 `SetVisible` 并请求重绘；
   - `Tray` 新增字段 `hidden_at_start` 与构造器 `start_hidden()`（装配期决定"出场露不露面"）。
2. **win32** `platform/win32/tray.rs`：
   - `TrayState` 记 `visible: bool`；
   - `install` 在 `hidden_at_start` 时**连 `NIM_ADD` 都不发**（通知区里根本没它，避免关着时闪一下）；
   - 新增 `remove()`（`NIM_DELETE` + 清 `visible`）、`is_visible()`、`remember_visible(on)`
     （`NIM_ADD` 失败不记账，便于重试）；`Drop` 改调 `remove`。
3. **win32** `platform/win32/mod.rs`：
   - `readd_tray()` 加 `.filter(|ts| ts.is_visible())`——被应用藏起的图标**不许**被 shell 重启（`TaskbarCreated`）复活；
   - `apply_tray_ops()` 改写为逐 op 取素材（借用不跨 OS 调用），`SetVisible` 同值跳过、显示走 `readd`、隐藏走 `remove`，成功才 `remember_visible(on)`。
4. **macOS** `platform/macos/tray.rs`：
   - `apply_tray_ops` 加 `TrayOp::SetVisible(on) => item.setVisible(on)`；
   - `install` 取 `hidden_at_start` 后 `status_item.setVisible(false)`。
5. **应用侧** `main`：
   - `ui/tray.rs::build(shown_at_start)`：关着时 `.start_hidden()`，托盘**恒装配**；
   - `runtime/mod.rs`：**移除 `App::hide_on_close()`**；16ms 轮询里比对 `config.close_to_tray`
     与 `tray_shown`，变化即 `tray_handle.set_visible(want_tray)`；新增纯函数 `close_action()`
     （`lock > tray > exit`）与 `on_close_request` 按 `Hide` 分支 `ctx.hide_window()`；
   - `core/config.rs`：`close_to_tray` 文档改为"运行期即时生效"；
   - `ui/settings.rs`：复选框去掉"重启生效"文案。

### 验收（可复现）
- **上游单测** `windui` `cargo test --lib platform::win32::tray`：
  `托盘图标真的进出通知区` —— install→`present==TRUE`、remove→`FALSE`、readd→`TRUE`、
  `start_hidden`→`FALSE`（连 `NIM_ADD` 都没发）。另含 `visible_ops_and_start_hidden_are_separate_paths`。
- **应用单测** `main` `cargo test --lib -- --test-threads=1`：
  `runtime::tests::close_decision_prioritises_lock_over_tray`（lock > tray > exit）。
- **实机取证** `main/.temp/tray_close_probe.py`：
  - `close_to_tray=1` 启动 → 关窗后**进程存活、主窗消失、托盘图标在通知区**；
  - `close_to_tray=0` 启动 → 通知区**无图标**、关窗直接退出。
- **运行期切换**：启动后改 ini `close_to_tray` 0↔1，下一轮 16ms 轮询图标当场出现/消失。

### 门禁行
layering_scan `violations: 0` · hygiene_scan `violations: 0` · `cargo clippy --all-targets` 0 warning(本项目) ·
`cargo test --lib -- --test-threads=1` **111 passed / 0 failed**（含两新增测试）·
windui `cargo test --lib platform::tray` **5 passed**（含真机测试）。

### 升级 windui 时的重放
从上游新版本 `src/platform/{tray.rs,win32/tray.rs,win32/mod.rs,macos/tray.rs}` 出发，
按本补丁上述 5 步逐一重放；`wind-ui-rust-0.20.0/before/` 提供 0.20.0 原始文件做 diff 基线。

---

## 补丁 02 —— 失焦回调 `on_blur`（移植 CurSeen 补丁 094）

- 编号：02
- **落点状态：0.23.0 无落点**（2026-10-24 核）——这一节是 0.20.0 时代手搭的失焦回调（`BlurFn` / `Widget::take_blur`）。
  0.23.0 上游自带同名设施：`ui/mod.rs:944` 的 `pub fn on_blur(...)` builder、`core.rs` 的 `Widget::on_blur`
  与节点级 `blur_notify`（**都没有 `★ RuPix 补丁` 标记＝上游的**），宿主 `main/src/ui/settings/shortcuts.rs:43`
  的 `.on_blur(on_blur)` 直接打在 upstream 方法上、编译通过。⇒ 补丁不需要再打，也**不该**补一个假标记。
- 涉及上游文件（均在 `wind-ui-rust-0.20.0/src/`）：
  - `core.rs`（`BlurFn` 类型 + `Widget::take_blur` 默认实现）
  - `ui/containers.rs`（`Clickable` 的 `on_blur` / `had_focus` 与 paint 边沿检测）
  - `ui/mod.rs`（`Element::on_blur` builder + build 时注入）
- 应用侧接线：`main/src/ui/settings.rs`（快捷键捕获框 `.on_blur(...)`：点了框又点别处即放弃捕获）

### 问题（用户实测）
设置页「快捷键」捕获框要「点它进捕获、点别处退出捕获」的手感（CurSeen 同款）；windui 没有任何失焦通知，捕获态会黏在框上。

### 根因（实测，非推测）
windui 的焦点变更只翻节点 `focused` 标志位，**不向控件发事件**；`paint(..., focused, ...)` 的 `focused` 参数是控件唯一的失焦信号。上游 stepper 内部有同款「失焦提交」机制，但只写死在 stepper 里，没有暴露给宿主控件。

### 修复
1. `core.rs`：新增 `BlurFn = Rc<dyn Fn()>`（paint 阶段拿不到 `EventCtx`，回调只做「改信号」这类纯状态动作）与 `Widget::take_blur`（默认空）。
2. `ui/containers.rs`：`Clickable` 持 `on_blur: Option<BlurFn>` 与 `had_focus: Cell<bool>`；`paint` 开头做 true→false 边沿检测，命中即触发（回调改信号引发的下次重绘不影响本次绘制）；`take_blur` 落地存回调。
3. `ui/mod.rs`：`Element` 增 `blur` 字段与 `.on_blur(...)` builder；`build` 时注入控件（仅 `Clickable` 接收）。

### 验收（可复现）
- windui `cargo test --lib` 全套通过（stepper 失焦提交等既有回归不破）。
- main `cargo test` 全目标绿（`cargo test --lib` 119 passed，hotkey 单键系列不破）。
- 实机：设置页点捕获框进「按下按键…」，点页面别处即退出捕获（与 `settings.rs` 的 blur 守卫语义一致）。

### 门禁行
layering_scan `violations: 0` · hygiene_scan `violations: 0` · windui `cargo test --lib` **931 passed** · windui clippy 0 error · main `cargo clippy --lib --release` 0 warning · main `cargo test` 全目标绿（lib 119 · config_compat 2 · config_pipeline_e2e 3 · docs_consistency 7）。

### 升级 windui 时的重放
三文件三段管道：`core.rs`（类型与 trait 方法）→ `ui/containers.rs`（字段与边沿检测）→ `ui/mod.rs`（字段、builder、build 注入）。

---

## 补丁 03 —— 逐像素窗口透明度与菜单浮层豁免（`UpdateLayeredWindow` 路线）

- 编号：03
- 涉及上游文件（均在 `wind-ui-rust-0.20.0/src/`）：
  - `platform/mod.rs`（`OpaqueRegion` + `AppHandler::opaque_overlays` 钩子）
  - `app/menu.rs`（`MENU_RADIUS` 单一定义点 + `Tree::menu_open` 开/关两处写入）
  - `app/mod.rs`（`UiHost::opaque_overlays` 实现：把当前菜单各级面板按物理像素推出）
  - `core.rs`（`Tree::menu_open` 字段 + `EventCtx::menu_open()` 只读访问）
  - `platform/win32/mod.rs`（`SkiaBackend` 分层呈现：`set_window_alpha` / `paint_layered` / `copy_premul_rows` / `inside_round_rect` / 分层资源 `Drop`）
- 应用侧接线：无直接触点——`opaque_overlays` 在 windui 宿主内部闭环；透明度数值的调用链属补丁 04。

### 问题
窗口整体不透明度（看图时让桌面透出来）与「菜单必须清晰」互相冲突：`WS_EX_LAYERED` + `SetLayeredWindowAttributes(LWA_ALPHA)` 一个窗口只有一档 alpha，菜单画在同一张画布上必然跟着糊。

### 根因
`LWA_ALPHA` 是**常量整窗 alpha**，没有按区域豁免的表达能力。要「整窗打折、浮层原样」，呈现必须走**逐像素 alpha**（`UpdateLayeredWindow` + 每像素自带 alpha 的位图），并在提交前把浮层区域按原 alpha 重写。

### 修复
1. `platform/mod.rs`：`OpaqueRegion { rect, radius }`（圆角矩形，物理像素）+ `AppHandler::opaque_overlays(&self, out)` 默认空实现——用圆角而非矩形：面板 radius=10，按矩形整块提升会在四角露出方角。
2. `app/menu.rs`：抽出 `MENU_RADIUS`（绘制与逐像素判定共用，两处不一致就露角）；`open_menu` / `close_menu` 维护 `Tree::menu_open`。
3. `core.rs`：`Tree::menu_open` 字段 + `EventCtx::menu_open()`——菜单不属于控件树，宿主没有别的途径知道它开着。
4. `app/mod.rs`：`UiHost::opaque_overlays` 把当前菜单各级面板 `scaled(scale)` 后推出。
5. `platform/win32/mod.rs`：`SkiaBackend` 增逐像素路径——`alpha<255` 时 `paint` 改走 `paint_layered`：pixmap(RGBA 非预乘) → 预乘 BGRA 分层 DIB（`CreateDIBSection` 常驻 + 一次性清零）→ `UpdateLayeredWindow(ULW_ALPHA)`；`copy_premul_rows` 先整行按 alpha 打折、再把 `opaque_overlays` 圆角区段按原 alpha 重写；最大化态按**窗口矩形**提交（客户区已内缩，照客户区传会每帧把窗口向里缩）；`pptDst/psize` 必须显式传（传 NULL 实测返回成功但不上屏，首帧之后卡死——注释内 `paintprobe2` 取证）；D2D 后端 `set_window_alpha` 默认返回 `false` 回退 `LWA_ALPHA`。
6. 同批修复两处 `alpha >= 255`（`u8` 上限比较）为 `== 255`——clippy `absurd_extreme_comparisons` 拒绝级错误。

### 验收（可复现）
- 实机：透明度 <100% 时主窗透出桌面，**展开的菜单面板完全不透明**、圆角外四角不露方角；拖动/最大化不闪不缩水；100% 时摘掉 `WS_EX_LAYERED` 回普通合成。
- windui `cargo test --lib` 全套通过（既有绘制回归不破）。
- GDI/`UpdateLayeredWindow` 路径无头单测不可覆盖，判据以实机 + 全套回归为准。

### 门禁行
layering_scan `violations: 0` · hygiene_scan `violations: 0` · windui `cargo test --lib` **931 passed** · windui clippy 0 error（含上述两处 `absurd_extreme_comparisons` 修复）· main clippy/test 全绿。

### 升级 windui 时的重放
`platform/mod.rs`（类型 + trait 钩子）→ `core.rs`（menu_open）→ `app/menu.rs`（开/关维护 + MENU_RADIUS）→ `app/mod.rs`（实现）→ `platform/win32/mod.rs`（分层呈现整段）；`WindowOp::SetOpacity` 的入口在补丁 04。

---

## 补丁 04 —— 窗口级 WindowOp 扩展（全屏 / 置顶 / 拖窗 / 透明度 / 客户区尺寸）

- 编号：04
- 涉及上游文件（均在 `wind-ui-rust-0.20.0/src/`）：
  - `event.rs`（`WindowOp::{ToggleFullscreen, ToggleTopmost, StartDrag, SetOpacity, SetClientSize}` + `WindowState::{fullscreen, topmost}`）
  - `core.rs`（`EventCtx::{toggle_fullscreen, toggle_topmost, start_window_drag, set_window_opacity, set_window_size}` 五个助手）
  - `platform/win32/mod.rs`（五个 op 的执行体 + `WindowState` 三字段 + `push_window_state` 同步 + `run_window_op` 分发 + WM_TIMER 路径补消费）
  - `platform/macos/window.rs`（两变体显式空实现占位，语义收口在 match）
  - `app/mod.rs`（`WindowState` 新增两字段的既有测试字面量同步）
- 应用侧接线：`main/src/runtime/mod.rs`（`StartOp::Topmost` / `Fullscreen`、不透明度轮询、`set_window_size` 开窗档）、`main/src/ui/canvas.rs`（全屏/置顶按钮、双击、拖窗手势、快捷键）

### 问题
看图器需要五个上游没有的窗口动作：无边框全屏（F）、置顶切换、整窗任意位置拖动、窗口不透明度、按图片自然尺寸开窗——上游 `WindowOp` 只有 Show/Hide/Minimize/Maximize/Restore/Close。

### 根因
上游把窗口能力收在一个固定枚举里，自定义标题栏 / 画布铺满的应用无从请求这些状态；且事件分发之外的 `on_interval` 回调里产生的窗口意图没有消费点（宿主常用它做「启动置顶/全屏」首轮补刀，不消费 = 功能静默失效）。

### 修复
1. `event.rs`：五个新变体（各带语义注释）+ `WindowState` 两个字段；`SetOpacity` 用 `u8` 百分比而非 `f32`——`WindowOp` 是 `Eq` 的（与 tagged 事件一起去重/比对），浮点表不了 Eq。
2. `core.rs`：五个 `EventCtx` 助手（`set_window_opacity` 夹到 1..=100；`set_window_size` 拒绝 <1）。
3. `platform/win32/mod.rs`：
   - `toggle_fullscreen`：记 `(GWL_STYLE, 窗口矩形)` 进 `saved_placement`，`WS_POPUP` 铺满就近显示器，退出原样恢复；OS 查询与 OS 修改分段（`SetWindowPos` 同步重入 `wnd_proc`）。
   - `toggle_topmost`：真值读 `WS_EX_TOPMOST` 样式位而非缓存——系统工具也可能改置顶。
   - `start_window_drag`：`ReleaseCapture` + 合成 `WM_NCLBUTTONDOWN/HTCAPTION` 交 DefWindowProc 原生移动循环（必须先释放捕获，否则窗口纹丝不动）。
   - `set_window_opacity`：优先走补丁 03 的逐像素钩子，后端不支持才回退 `LWA_ALPHA`（两者互斥：先设 LWA 会让第一帧之前整窗不可见）；100% 时**摘掉** `WS_EX_LAYERED`；下限 1%（完全透明看起来像崩溃）。
   - `set_client_size`：`AdjustWindowRectExForDpi`（按窗口 DPI，不用系统 DPI）反推外框 → 夹到工作区 → 居中；最大化 / 最小化 / 全屏态不动（那些尺寸不属于常规态）。
   - `push_window_state` 把 `fullscreen/topmost` 一并推给宿主；WM_TIMER 路径补 `apply_window_op`（与事件路径对位）。
4. `platform/macos/window.rs`：新变体显式空实现（match 语义收口，加变体漏平台会编译报错）。

### 验收（可复现）
- main 单测：`ui::canvas` 的 `double_click_jitter_is_not_a_drag`（拖窗阈值不吞双击）、`core::config` 的 `slider_round_trips_opacity` / `opacity_out_of_range_loads_clamped` / `window_size_clamps_in_range`。
- 实机：F 全屏进出（恢复位置样式原样）、置顶即时生效、画布任意位置拖窗跟手、透明度档生效、`启动窗口：自动` 按图片尺寸居中开窗；启动后改 ini 的置顶/全屏首轮补刀当轮生效（WM_TIMER 消费缺口）。

### 门禁行
layering_scan `violations: 0` · hygiene_scan `violations: 0` · windui `cargo test --lib` **931 passed** · windui clippy 0 error · main clippy 0 warning · main `cargo test` 全目标绿（lib 119 · config_compat 2 · config_pipeline_e2e 3 · docs_consistency 7）。

### 升级 windui 时的重放
`event.rs`（变体 + 字段）→ `core.rs`（助手）→ `platform/win32/mod.rs`（执行体五段 + state 三字段 + push 分发 + WM_TIMER）→ `platform/macos/window.rs`（占位）→ `app/mod.rs`（测试字面量）；透明度执行体依赖补丁 03 的后端钩子——**先 03 后 04** 才能编译。

---

## 补丁 05 —— `WM_NCHITTEST` 命中优先级重排（铺满客户区的画布也能四边缩放）

- 编号：05
- **落点状态：0.23.0 无落点**（2026-10-24 核）——被**补丁 014** 取代：014 那一版的注释就写在
  `.3rd/wind-ui-rust-0.23.0/src/platform/win32/mod.rs` 的 `handle_nchittest` 里（"上游把 `interactive_at` 放在最前…
  补丁 05 曾把交互例外降到只留顶缘，但看图器的顶条本身就是画布的一部分…现在八向一律给缩放码"）。
  ⇒ 标记归在 `014` 名下，本节保留作重放依据，**不需要**也**不该**再补一个 `补丁 05` 假标记。
- 涉及上游文件（均在 `wind-ui-rust-0.20.0/src/`）：
  - `platform/win32/mod.rs`（`handle_nchittest`）
- 应用侧接线：无直接触点；`main/src/ui/canvas.rs` 铺满客户区的可聚焦画布是受益方。

### 问题（用户实测）
画布 `focusable()`（要吃键盘事件）铺满客户区后，窗口**只有顶部和左上角能缩放**——左右底三缘的缩放边框被交互控件优先级吃掉。

### 根因（实测，非推测）
上游 `handle_nchittest` 把 `interactive_at`（交互控件判定）放在**全向最前**：先判交互、再判缩放边框、最后才问拖动区。交互控件铺满时四缘全部返回 `HTCLIENT`，缩放边框永远轮不到。

### 修复
`interactive` 优先降级为**仅顶缘**：顶缘让位给贴边的窗口 chrome 按钮（关闭/最大化，hover 要稳定）；其余七向缩放边框无条件优先（与原生窗口一致）；非边缘仍走 `window_drag_at` → `HTCAPTION` / `HTCLIENT`。

### 验收（可复现）
- 实机：画布铺满时左/右/底/四角均可拖拽缩放；顶缘标题按钮 hover 正常、顶缘仍可缩放；非拖动区按下仍是客户区。
- windui `cargo test --lib` 全套通过（既有命中相关回归不破）。

### 门禁行
同补丁 04（layering/hygiene 0 违规 · windui clippy 0 error · main 全绿）。

### 升级 windui 时的重放
`handle_nchittest` 单函数三段：edge 计算 → 顶缘交互例外 → 返回 edge；非边缘问拖动区。

---

## 补丁 06 —— SVG 渲染性能路线（解析缓存 / 全局字体库 / 固有尺寸预量 / 区域渲染）

- 编号：06
- 涉及上游文件（均在 `wind-ui-rust-0.20.0/src/`）：
  - `render/image.rs`（`svg_fontdb` / `svg_warm_fonts` / `svg_tree_cached` / `svg_intrinsic_size` / `Image::from_svg_region` + `rasterize` 改走缓存）
- 应用侧接线：`main/src/ui/svg.rs`（`svg_intrinsic_size` 两段式量尺寸 + `from_svg_region` 可视区 1:1 路线）、`main/src/main.rs`（启动后台线程调 `svg_warm_fonts` 预热）

### 问题
看图器每次缩放/平移都重渲 SVG：实测含文字 SVG `parse ≈ 340ms`、`render ≈ 12ms`——约 95% 时间浪费在重复解析同一份字节；整图光栅化在超大尺寸下必然撞硬顶出马赛克；「只拿宽高」过去也要完整渲染一遍。

### 根因
上游每次解码都 `usvg::Tree::from_data` 现场解析（解析占绝对大头且不可增量）；字体库挂在 `thread_local`（每线程各扫一遍全系统字体 ≈900ms，多线程渲染成倍浪费）；尺寸获取与渲染耦合在同一个函数里。

### 修复
1. `svg_fontdb`：系统字体库 `OnceLock<Arc<Database>>` 全局唯一、只读共享（加载完成后不可变，`Arc` 共享安全）；`svg_warm_fonts` 供启动时后台线程预热。
2. `svg_tree_cached`：按「字节内容哈希 + 长度」缓存 `Arc<usvg::Tree>`（解析完成即不可变），容量 8 满即整清（树很小，粗暴淘汰足够）；`svg-text` 时挂全局字体库。
3. `svg_intrinsic_size`：只解析不渲染取固有尺寸——两段式调用（先量后渲）替代「按固有尺寸完整渲一遍只为拿宽高」。
4. `Image::from_svg_region`：只渲 `origin..origin+size` 矩形、按 `density` 放大（`p' = k·(p − origin)`）——可视区 1:1 路线每次只渲一屏，清晰度与内存都和倍率无关。
5. `rasterize` 改走 `svg_tree_cached`（顺手清理由此不再使用的 `use` 中 `usvg` 项）。

### 验收（可复现）
- windui 单测：`render::image` 的 `from_svg_bytes_uses_intrinsic_size`、`from_svg_bytes_target_width_scales_keeping_aspect`、`renders_svg_text_glyphs`、`invalid_svg_is_decode_error` 等全套通过。
- main 单测：`ui::svg::svg_region_route_renders_requested_area`（区域渲染只出请求区域）。
- 实机：超大 SVG 1:1 缩放平移跟手；同文件二次渲染零解析耗时（debug 日志 `svg region … parse=0ms`）。

### 门禁行
同上（windui clippy 0 error 含 unused import 清理 · main 全绿）。

### 升级 windui 时的重放
`render/image.rs` 单文件五段：字体库/预热 → 解析缓存 → 固有尺寸 → 区域渲染 → `rasterize` 接缓存。

---

## 补丁 07 —— 贴边滚动条内缩按 `frameless` 开关（非无边框窗滑块贴右缘）

- 编号：07
- 涉及上游文件（均在 `wind-ui-rust-0.20.0/src/`）：
  - `core.rs`（`Tree::scrollbar_edge_inset_enabled` 字段 + `scrollbar_edge_inset` 门控 + 单测）
  - `app/mod.rs`（`UiHost::new` 按 `cfg.frameless` 写入）
- 应用侧接线：无——`UiHost` 构造时自动按窗口形态取值（设置窗/属性窗 `frameless=false` → 贴边；主窗 `frameless=true` → 维持避让）。

### 问题（用户实测）
设置窗 / 属性窗（带系统标题栏）的滚动条滑块离右缘太远——上游恒定内缩让出的那圈空白，在没有缩放边框拦截的窗口上是白让的。

### 根因（实测，非推测）
上游 `scrollbar_edge_inset` 为**无边框窗**设计：其 `WM_NCHITTEST` 把客户区右缘 8 逻辑 px 判成 `HTRIGHT`，落在那里的指针事件进不到客户区，滚动条画在那「看得见点不着」；但该内缩没有开关、对带系统标题栏的窗口同样生效，而后者没有那圈拦截——内缩只剩副作用。

### 修复
`Tree` 增 `scrollbar_edge_inset_enabled: bool`（默认 `true`：直接 `Tree::new()` 的测试树与无边框窗行为不变），`scrollbar_edge_inset()` 起手门控；`UiHost::new` 按 `cfg.frameless` 写入——无边框窗 `true`（维持避让），系统标题栏窗 `false`（滚动条贴边）。

### 验收（可复现）
- windui 单测：`core` 的 `disabling_edge_inset_makes_the_scrollbar_hug_the_window_edge`（关内缩后命中区上界 = 窗口右缘、预留宽度不带内缩量）+ 既有 `scroll_tree_of_width` 系列回归（开内缩行为不变）。
- 实机：设置窗滚动条滑块贴右缘；主窗右缘缩放边框内滚动条依旧可点。

### 门禁行
windui `cargo test --lib` **931 passed** · windui clippy 0 error · main `cargo test` 全目标绿（lib 119 · config_compat 2 · config_pipeline_e2e 3 · docs_consistency 7）· main clippy 0 warning。

### 升级 windui 时的重放
`core.rs` 字段 + 门控 + 单测、`app/mod.rs` 一行装配，共两文件。

---

## 补丁 08 —— `list_signal` 保住 `scroll()` 的 ScrollWidget（滚动条画得出却滚不动）

- 编号：08
- 涉及上游文件（均在 `wind-ui-rust-0.23.0/src/ui/`）：
  - `dyn_list.rs`（新增组合 widget `DynListWithScroll`）
  - `mod.rs`（`Element::list_signal` 改挂组合体）
- 应用侧接线：无——`Element::list_signal` 的所有调用点自动受益（本仓库：设置 → 解码器页）。

### 问题（用户实测）
设置 → 解码器页：列表右侧**有**垂直滚动条，但滚轮不滚、拖滑块也不动，无法向下滑动。

### 根因（实测，非推测）
`Element::list_signal` 的容器是 `Self::scroll().fill()`，而 `scroll()` 的本职就是装上
`ScrollWidget`（滚轮 + 滚动条拖动）。紧接着构造器又 `set_widget(Box::new(DynList { .. }))`
挂上重建行的 widget——`set_widget` 是**覆盖**语义（`self.widget = w`），`ScrollWidget` 被
静默丢掉。讽刺的是框架自己在 `Element::widget` 的 `debug_assert!` 里警告过这种行为
（"该节点已有控件，再挂会把它静默替换掉"），而 `set_widget` 作为内部入口恰好绕过那道守卫。

`DynList::on_event` 恒返回 `false`，替换后再无人处理滚轮与拖动。症状极具迷惑性：

- `Layout::Scroll` 仍在 → `measure_scroll` / `arrange_scroll` 照跑，`content_h` / `view_h` /
  `max_scroll` 全算对，**滚动条也照常画出来**（实测 20 行时 `max_scroll = 636`）；
- 但事件侧无人接手 → `scroll_y` 恒为 0。

即"看得见、滚不动"。**布局类的回归测试抓不到**：既有 `decoders_page_rows_survive_layout`
只验"行可见 + 容器有高度"，而那两样恰恰都是好的。

### 修复
组合而非替换：新增 `dyn_list::DynListWithScroll`，内部同时持有 `DynList` 与 `ScrollWidget`，
`on_update` / `measure` / `paint` 走前者、`on_event` 走后者，共用一个 widget 槽位。
`list_signal` 改挂它；`host_signal`（容器是普通 `col`、无可滚容器）仍挂纯 `DynList` 不动——
否则不可滚的列容器会吞掉滚轮、外层反而失灵。

### 验收（可复现）
- main 侧新回归 `ui::settings::tests::decoders_page_list_actually_scrolls`：把真实 `decoders_page`
  搬上无头 `Tree`，**塞 20 行强制溢出**，再真派发事件验偏移量——① 滚轮一格（`-120`，滚轮量
  以 1/120 格为单位）后 `scroll_y` 必须 > 0 且事件被消费；② 反向滚必须能减小；
  ③ 拖滚动条（Down / Move / Up）后 `scroll_y` 必须变化。
- **负向验证**：把 `list_signal` 临时还原成 `set_widget(Box::new(widget))`，该测试立刻变红
  （挂在 `assert!(res.consumed, "滚轮事件无人消费")`），证明测的是真因果而非碰巧。
- 既有 `decoders_page_rows_survive_layout`（行可见 / 行高 = `ROW_H`）仍绿。

### 门禁行
windui `cargo test --lib` **1076 passed / 0 failed** · main `cargo test` 全目标绿
（lib **157** · `src/main.rs` 0 · `config_compat` 2 · `config_pipeline_e2e` 3 ·
`docs_consistency` 8 = **170**，其中 lib +1 即本补丁的新回归测试）· 静态门禁
（`2_scan.bat`：layering 0 / hygiene 0）· `full.bat` 六步 build complete，
exe 重新部署到 `.build\rupix.exe`。

### 升级 windui 时的重放
`ui/dyn_list.rs` 追加 `DynListWithScroll`（约 60 行）、`ui/mod.rs` 的 `list_signal` 改一行
`set_widget(..)` 实参，共两文件。若上游已自行修复（改为组合式挂载，或给 `set_widget` 加守卫），
保留回归测试即可——它不依赖补丁的实现方式，只断言"溢出后滚得动"。

---

## 补丁 09 —— 输入框前置图标镜像绘制（`leading_icon_mirrored` / `draw_text_mirrored`）

- 编号：09
- 涉及上游文件（均在 `wind-ui-rust-0.23.0/src/`）：
  - `render/mod.rs`（`Canvas::draw_text_mirrored`：默认实现 = 退化普通绘制，「默认正确、后端可覆盖」）
  - `render/skia.rs`（软路径覆盖：拷背景 → 离屏画字 → 行翻转 → 带 clip 贴回）
  - `platform/win32/d2d.rs`（D2D 覆盖：本地镜像矩阵叠加既有 scale 变换；`compose_row_vector` / `mirror_about` 两个矩阵辅助函数）
  - `ui/inputs.rs`（`TextConfig.leading_mirrored` + paint 分支 + 逐像素回归测试）
  - `ui/mod.rs`（`Element::leading_icon_mirrored` builder）
- 应用侧接线：`main/src/ui/settings.rs`（解码页搜索框改用镜像图标 + 更正「手柄朝右下」错误注释）；
  顺手清偿存量（非本补丁引入）：`runtime/mod.rs` WINDIR 兜底改写规避 abs_path 违规、
  6 文件 22 条 clippy 存量清零。

### 问题（用户实测）
设置→解码页搜索框的放大镜手柄朝左下，要求朝右下。取证：该图标是字体字形 U+E721
（Segoe MDL2 Search），PIL 渲染探针（`.temp/probe-icon/`）证实其原生就是「圆圈在右、
柄朝左下」，字体里没有镜像变体码位；`settings.rs` 原注释「手柄朝右下的经典样式」与
事实相反。`leading_icon` 只收字符、无镜像能力，上游没有翻转绘制的口子。

### 选型（为什么走补丁通道）
镜像只能发生在**字形绘制环节**，故取 A＝给 windui 加镜像绘制原语（改动小、不动布局占位、走既有补丁通道）。
弃 B＝自绘 SVG 替换字体字形：`text_input` 前置图标槽只吃 `char`，得先给上游加"图片图标位"，改动面更大；
弃 C＝自制单字形镜像字体随 exe 注册：绕开补丁却引入**字体资产与注册时序风险**（私有字体注册失败只退化成方框）。

### 根因
镜像必须发生在字形绘制环节；`Canvas` trait 只有 `draw_text`，两个真实后端都没有
水平翻转原语（软渲染直画物理 pixmap 无变换；D2D 只有 DPI scale 变换）。

### 修复
1. `render/mod.rs`：trait 新增 `draw_text_mirrored(text, rect, color, align, ts)`，
   默认实现调 `draw_text`（不镜像）——未覆盖的后端（macOS/Linux/测试桩）行为不变。
   语义注明：以 rect 水平中轴翻转整块绘制结果（含字序），供图标字形翻向用。
2. `render/skia.rs`：软路径覆盖——「拷背景 → 离屏画字 → 行翻转 → 贴回」：
   - 离屏框 = `text_cull_rect` 外扩 ≥4 物理px 的整数逻辑框；**必须先把目标对应区域
     背景拷进 tmp**（ClearType 亚像素混合依赖不透明真实背景，透明底退化灰阶 AA，
     边缘与直画不一致——首版踩过，逐像素测试抓出）；
   - 翻转 = 预乘 RGBA 行内反序（无色彩语义损失）；裁剪 = tmp 中裁剪区外像素清零
     后免 mask 整块贴回（免整幅窗口 Mask 分配，同 `draw_image` 的那笔账）；
   - 非整数 DPI 下整块落点至多漂 1 物理px（文档注明）；面积 >1MP 的异常 rect
     退化为普通绘制。
3. `platform/win32/d2d.rs`：D2D 覆盖——`mirror_about(cx)`（x' = 2cx − x）与既有
   变换按行向量约定复合（windows-numerics 平移分量名 M31/M32 且无乘法运算符，
   `compose_row_vector` 按分量手写），夹在 Get/SetTransform 之间复用整个
   `draw_text`（半透明层/彩色字形/布局缓存原样生效），退出恢复原变换。
4. `ui/inputs.rs`：`TextConfig.leading_mirrored`（Copy 结构体加 bool）+ paint 分支；
   布局占位 `lead_inset` 不变，文字/光标/命中不受镜像影响。
5. `ui/mod.rs`：`Element::leading_icon_mirrored(glyph)` builder。
6. 应用侧 `settings.rs`：搜索框 `.leading_icon_mirrored('\u{E721}')` + 注释纠错。
7. 2026-10-28：图标区收紧——`lead_inset` 从 `font_size + 8` 改为 `+4`，图标绘制
   x 从 `bounds.x + TEXT_PAD` 改为 `bounds.x + 4`。同时 `\u{E721}` 搜索镜不再走
   字形/镜像绘制，改由 `draw_search_icon` 用圆环+手柄矢量绘制（主题色），消除焦点
   重绘时镜像路径擦出图标区背景块的问题；应用侧统一走 `rows::search_box`。

### 验收（可复现）
- windui 单测（真实 DirectWrite / 真实 D2D-WARP 离屏，均新增）：
  - `render::skia` `draw_text_mirrored_is_exact_horizontal_flip`：scale=1、整数 rect
    下镜像绘制与普通绘制的水平翻转**逐位一致**（拷背景修复前后该测试由红转绿）；
  - `platform::win32::d2d` `mirror_compose_order_applies_mirror_first`（复合顺序）
    + `draw_text_mirrored_mirrors_the_ink_distribution`（总墨量不变 / x 质心镜像 /
    左右半区墨量互换——ClearType 亚像素不对称，比不变量不比逐位）。
- 视觉对照：临时探针 `cargo run --example icon_mirror_probe`（跑完即删）产
  `.temp/probe-icon/mirror_before_after.png`——上行镜像后柄朝右下、下行原样柄朝左下。

### 门禁行
windui `cargo test --lib` **1079 passed / 0 failed**（含 3 新测试）· windui clippy
0 error（存量 4 warning 非本补丁引入）· 主项目 layering 0 / hygiene 0（含顺手修复
runtime/mod.rs 存量 abs_path 违规）· main `cargo clippy --all-targets` **0 warning**
（含清偿 6 文件 22 条存量）· main fmt 0 diff · main 测试全绿（lib **167** ·
config_compat 2 · config_pipeline_e2e 5 · docs_consistency 8）。

### 升级 windui 时的重放
五文件五段：`render/mod.rs`（trait 默认）→ `render/skia.rs`（拷背景+离屏翻转覆盖 +
逐像素测试）→ `platform/win32/d2d.rs`（矩阵覆盖 + 两个辅助函数 + 两个测试）→
`ui/inputs.rs`（配置位 + paint 分支）→ `ui/mod.rs`（builder）。若上游收编了镜像绘制
原语，保留测试即可——它们断言的是语义（镜像 = 水平翻转），不绑实现。

---

## 补丁 10 —— `EventCtx::close_window`（按单例键关窗的主窗回调入口）

- 编号：10
- 涉及上游文件（均在 `wind-ui-rust-0.23.0/src/`）：
  - `core.rs`（`EventCtx::close_window(key)` 一行转发到 `event::push_callback_close`）
- 应用侧接线：`main/src/runtime/mod.rs`（16ms 轮询 → `ui::preview::tick`）

### 问题（空格预览实施中实测）
空格预览需要「预览开着时再按空格 → 收起」：16ms 轮询的 `EventCtx` 能开窗
（`open_window`），却**没有按单例键关窗的对侧**——`close_window` 只挂在常驻模式的
`HotkeyCtx` 上（event.rs，服务 `App::resident` 热键回调），主窗回调上下文拿不到。

### 根因
开/关对侧的管道（`push_callback_window` / `push_callback_close`）都是 `pub(crate)`
且与具体 ctx 类型无关，但 `EventCtx` 只接了开窗那一半。

### 修复
`core.rs` 的 `EventCtx` 增 `close_window(key: impl Into<String>)`——一行转发到
`event::push_callback_close`，与 `HotkeyCtx::close_window` 同管道同语义（无此键时
什么也不做；走完整关闭决策链）。

### 验收（可复现）
- windui `cargo test --lib` **1079 passed / 0 failed**（转发逻辑一行，管道行为由
  既有 `push_callback_close` 系测试与 main 侧集成覆盖）。
- 实机：空格开预览 → 再按空格收起（`window_open(key)` 判存在 → `close_window(key)`），
  `Esc` 同链路。

### 门禁行
windui `cargo test --lib` **1079 passed / 0 failed** · windui clippy 0 error
（存量 4 warning 非本补丁引入）· main 全门禁绿（见补丁 09 同批门禁行）。

### 升级 windui 时的重放
`core.rs` 单方法追加（紧随 `EventCtx::open_window`）。若上游把 `close_window`
提升到 `EventCtx`，删除本补丁方法即可。

---

## 补丁 11 —— 放大保持像素硬边（`Image::with_pixelated`，最近邻上采样）

- 编号：11
- 涉及上游文件（均在 `wind-ui-rust-0.23.0/src/`）：
  - `render/image.rs`（`Image.pixelated` 字段 + `with_pixelated` / `pixelated` + `tinted` 传播 + 1 个用例）
  - `platform/win32/d2d.rs`（`draw_image` **两处** `DrawBitmap` 的插值模式改判 + 3 个离屏像素判据用例）
  - `render/skia.rs`（`draw_image` 的 `FilterQuality` 同一规则改判 + 1 个用例）
- 应用侧接线：`main/src/ui/canvas.rs`（`ImageCache` 增 `pixelated` 声明位，只给主画布置）

### 问题（2026-10-17 用户报障）
打开 `.temp/big-samples/tiff/100M/tiff_100M.tif`（6055×6055，程序生成的纯色方块图），
放大到底"全都是马赛克"；同样的图在别的软件里放大后能看清每一个像素格的棱角。
用户的问题是"放大被限制了，还是不允许看这么清晰"——两者都要回答。

### 根因
**与解码链无关**（`zoom_sharp_probe` 实测：>100% 时上屏位图已是 6055×6055 原图档，
1 源像素 = 1 位图像素；档位与内存都没被阉割）。缺陷在最后一厘米：上游
`D2DCanvas::draw_image` 把 `DrawBitmap` 的 `interpolationMode` **写死成
`D2D1_INTERPOLATION_MODE_LINEAR`**，矩形轴对齐裁剪与圆角 layer 掩膜两个分支各写死一处。
于是"1 个源像素铺满 N×N 个设备像素"这一步由双线性完成，相邻源像素必然混出中间色，
方块边缘在数学上不可能存在。软路径 `SkiaCanvas::draw_image` 同样恒用
`FilterQuality::Bilinear`。放大倍率本身的上限是 `MIN_ZOOM 0.02 / MAX_ZOOM 64`，
与本缺陷无关。

### 修复
1. `Image` 增 `pixelated: bool`（默认关），构造后 `with_pixelated(bool)` 声明，`tinted` 跟随。
2. 两条后端同一规则：**声明者 && 目标尺寸超出源像素（物理比较，容差 1 物理像素）** 才换
   最近邻；降采样方向与未声明者一律保持原来的双线性。
   `dw/dh` 在 D2D 是逻辑值、须 `×scale` 才是物理尺寸（该函数全程逻辑坐标，物理化交给
   ctx 的 `SetTransform`）；软路径的 `dw/dh` 已是物理值。
3. D2D 两个调用点各自改判（共用一个 `filter` 变量），软路径 `PixmapPaint.quality` 同改。
4. 应用侧只给**主画布**的 `ImageCache` 置位；鸟瞰、缩略图条、图库不置。

**为什么是逐图声明而不是全局规则**：全局"放大即用最近邻"会把非整数 DPI 下的矢量图标
一起带进最近邻（18dp 图标在 125% 下物理 22.5px，已算放大），整套 UI 变成锯齿。
做成 opt-in 后影响面精确到"看图主画面"这一处。

**为什么不改在解码/派生侧**（把位图本地插值放大到框的大小再 1:1 上屏）：那要在 64× 档
分配 6055×64 的位图，几百 MB 起、且糊面一点没变——放大这一步只能由渲染滤镜解决。

### 验收（可复现）
- **同一次运行内的两臂对照**（源 2×1 纯红/纯蓝，放大到 8×4）：
  - 矩形分支：32 个像素里，未声明臂 **16** 个是既非红也非蓝的混合色，声明臂 **0** 个
    （`pixelated_upscale_keeps_hard_edges`）。对照臂那个 `> 0` 断言是判据自我校验——
    若哪天读回全空，这条测试就成了空转。
  - 圆角分支（layer 掩膜，另一个调用点）：中间两行 16 个像素，未声明臂 **8** 个混合、
    声明臂 **0** 个（`..._under_rounded_clip`）。分两条是为了抓"只补一处"。
  - 降采样方向：8×8 棋盘画进 2×2，声明臂与未声明臂**逐位相等**
    （`pixelated_does_not_change_downscale`）——本标志只管放大。
- 判据跑在 `offscreen::OffscreenBackend`：与窗口后端**同一个** `D2DTarget`/`D2DCanvas`，
  只把呈现目标从 swapchain 换成 CPU 可读回的离屏位图（设备链先硬件后 WARP）。测的是
  生产代码，不是模拟。
- 应用侧前提由 main 的 `beyond_native_asks_the_renderer_to_upscale` 钉住：200% 档上屏
  位图仍是原图档 4×4 而目标框宽 8（判据会触发）；100% 档两者相等（正常看图不该走放大分支）。
- windui `cargo test --lib` **1084 passed / 0 failed**（原 1079 + 本补丁 5）。
- **未验证项（诚实记账）**：真机窗口里的放大观感。swapchain 呈现的像素读不回来，上面量的是
  离屏目标；像素格是否棱角分明要用户在窗口里放大确认。

### 门禁行
windui `cargo fmt --check` 0 diff · windui clippy 0 error（存量 4 warning：`unused_mut`
在 `svg_tree_cached`、`doc_lazy_continuation` 在 `ui/dyn_list.rs` 等，均非本补丁引入）·
main `cargo fmt --check` / clippy 0 / layering 0 / hygiene 0 ·
main 测试 **186 lib + 17 集成全绿**（`--test-threads=1`）。

### 升级 windui 时的重放
`render/image.rs` 的 `Image` 字段与两个方法（`from_pixmap` 置默认 false、`tinted` 复制）→
`platform/win32/d2d.rs` 的 `filter` 变量与两处 `DrawBitmap` → `render/skia.rs` 的
`quality`。若上游自己加了"按图片声明选插值"的能力，保留测试、删除字段即可——测试断言的是
语义（放大不得造中间色），不绑实现。

---

## 补丁 12 —— `Image::from_rgba` 预乘不再先拷一份源（上传 −30%、瞬时峰值 −139.9 MB）

- 编号：12
- 涉及上游文件：`render/image.rs`（`from_rgba` 改直读调用方切片；新增私有
  `Image::premultiply_rgba`，`from_rgba` 与 `from_decoded` **共用**；新增 1 个逐字节判据用例）
- 应用侧接线：**零改动**。这条纯上游实现细节，宿主仍然调同一个 `Image::from_rgba`
  （唯一上屏入口是 `main/src/ui/canvas.rs` 的 `ImageCache::obtain`）。

### 问题（2026-10-17，M2b 取数时发现）
计划里 M2b 的目标是"把预乘上传挪出 UI 线程"，理由是它是换档/翻页时 UI 被按住的那一截。
动手前先用 `examples/paint_cost_probe.rs` 量了一下这件事本身（上一轮的 0.088–0.140 s 是
**开着 fat LTO** 量的，见 K53，数字已过期）：`Image::from_rgba(6055²)` 中位 **119 ms**，
其中"把源 `to_vec()` 拷一份"单独占 **31.8 ms（26%）**——而这次拷贝对结果一个字节都不影响：
`from_rgba` 把借用的切片拷成 owned 的 `DecodedImage`，只为复用 `from_decoded` 那一条循环。

### 根因
上游把"解码器交来的 owned 缓冲"与"调用方手里的切片"合成同一条路（后者先拷成前者），
于是每一次上屏上传都要多付一轮 **源尺寸** 的 memcpy 和一块同等大小的瞬时可分配内存。
对 40MP 档的看图器，那正好是 139.9 MB。

### 修复
`from_rgba` 自己走"切片 → 预乘目标"，`from_decoded` 与新抽出的
`Image::premultiply_rgba(&[u8]) -> Vec<u8>` 共用同一条循环（**只留一处预乘**，
两条路各自演化迟早出色差）。目标缓冲刻意继续用 `with_capacity` + 逐像素写，
不用 `vec![0; n]` 预置零——140 MB 量级下那一次 memset 就是几毫秒的白账。

### 验收（配对复现，同一支探针、交替顺序）
`examples/paint_cost_probe` 的"预乘上传 6055²"一臂，改前/改后二进制各 4 组：

| | 中位 | 最小 |
|---|---|---|
| 改前 | 115.3 / 119.2 / 122.3 / 119.7 ms | 111.7–114.1 ms |
| 改后 | 85.8 / 82.5 / 84.0 / 83.6 ms | 79.4–82.0 ms |

**4/4 组同向，−30%**（比预估的 26% 还多一点：少掉的不止 memcpy，还有那次 139.9 MB 的
首次触碰页错误）。

**顺带把 B1 整轮没动过的峰值砍掉一份原图**——`mem_chain_probe` 同一张 6055×6055 配对：

| 阶段 | 改前 | 改后 |
|---|---|---|
| ⑤ 上屏上传耗时 | 0.084 s | **0.055 s** |
| ⑤ 之后峰值工作集 | 571.9 MB | **432.1 MB**（不再抬升） |
| 交还前全程峰值 | **711.8 MB** | **571.9 MB**（−139.9 MB） |

常驻不受影响（改前改后都是 427.8 MB 提交那一份纹理 + base）。

判据用例 `from_rgba_matches_premultiply_reference_per_byte`：2×2 四格专挑能暴露差错的
组合（alpha 0 / 255 / 128 / 1），断言"走借用切片的新路"与"走 owned 解码货币的老路"
**逐字节相等**，并钉住 alpha=0 全 0、alpha=255 原样、alpha=128 的 R 被压到 ≈129。
这条是"少掉一次拷贝不许改变任何一个字节"的直接证明，不靠观感。

### 门禁行
windui `cargo test --lib` **1085 passed / 0 failed**（原 1084 + 本补丁 1，`--test-threads=1`）·
windui clippy 无新增 error/warning（`unused_mut` 那条在 `svg_tree_cached`，非本补丁引入）·
main `cargo fmt --check` / clippy 0 / layering 0 / hygiene 0 · main 测试 **193 lib + 17 集成全绿** ·
`deploy OK`。

### 升级 windui 时的重放
`render/image.rs`：`from_rgba` 末尾改为 `Pixmap::from_vec(Self::premultiply_rgba(rgba), size)`、
私有 `premultiply_rgba` 一处、`from_decoded` 收回成一句调用。若上游自己把 `from_rgba`
改成直读切片，保留测试即可——断言的是语义（同一份 RGBA 走哪条入口都得同一块预乘字节），
不绑实现。

### 本补丁没做的那一半（判过、不划算）
"预乘挪出 UI 线程"（原 M2b 的正题）：**不做**。做完 (a) 之后最坏情况（40MP 图升到原生档
那一次）是 55–85 ms、**每张图一次**，而 ≤100% 各档上传只有几毫秒；要挪得动需要上游开一个
"任意线程预乘、回 UI 线程只包一层引用"的 `Send` 句柄，宿主侧再配一套在途状态机
（旧纹理续画、回包标脏、过期丢弃），代价是新图晚一帧出现。**收益一条 55–85 ms 的一次性
停顿，换一整套异步状态 + 一条公开 API，不值。**（若日后 B4 改成"只上传可见区"，
这 139.9 MB 的预乘根本就不再发生，本议题自动消失。）

---

## 补丁 13 —— 差 1 物理像素也要吸附 1:1（`draw_image` 的 1:1 判据补回它自己承诺的那 1 像素）

- 编号：13
- 涉及上游文件（均在 `wind-ui-rust-0.23.0/src/`）：
  - `render/skia.rs`（`draw_image` 的吸附判据 `< 1.0` → `<= 1.0 + 1e-3` + 1 个三臂判据用例）
  - `platform/win32/d2d.rs`（同一判据同一次修 + 1 个三臂离屏用例）
- 应用侧接线：**零改动**。宿主仍然按 `ceil` 请求档位、按 `round` 算上屏矩形——那 1 像素
  的尺寸差是这两条取整天生会造出来的，本补丁把它的代价从"整帧重采样"降回"整数平移"。

### 问题（2026-10-18，`drag_cpu_rig4` 分臂复测抓到）
同一张 6055×6055、同一个窗口（客户区 784×561 @100%），拖动时单帧 `image` 耗时在
**35% 那一档是 7.00 / 6.31 ms**，而 16%、91%、286% 三档都是 3.0–3.3 ms。倍率越深越贵
早就量过（成本轴是视口面积），但这一档是**中间某个倍率单独贵一倍**，说不通。
先排除了两件事：`[decode-trace]` 整轮只有 2 次真解码、`rebuild` 只跑 2 次，
**不是档位来回震荡**；每帧都是同样 3 次 `draw_image`（主图 + 鸟瞰 + 视口框内的那份），
**也不是多画了东西**。（ TEMP-PROBE 抓到的几何：贵的那臂 `src=2131×2131 / dst=2130×2130`，
便宜的臂 `src=5526×5526 / dst=5526×5526`。）

### 根因
`.3rd` 的 1:1 吸附判据是**严格** `< 1.0`，而它自己的注释写着"`scaled()` 四边各自 round
带来的 **±1 误差**也在此吸收"——差恰好 1 像素正落在这句承诺的门外。上游一直没被踩到，
是因为它自己的判据测不出这个：那条 `draw_image_snaps_near_unit_scale` 用的是一张**纯红**
源图，1.125 倍双线性把纯红摊出来还是纯红，断言永远不可能失败。
后果不是"糊一点点"而是**整帧换路**：吸附命中 → `Transform::identity()` + 整数平移，
tiny-skia 走逐行 blit；差 1 像素 → `scale = 0.99953` 的双线性重采样，逐像素 4 抽头。

### 修复
两臂同改（`<= 1.0` 并留 `1e-3` 余量：`dw` 是 `iw × (pw / iw)` 乘回来的 f32，整数边界也会
算出 1.0000001 这类值，2131 处 1 ulp ≈ 1.2e-4，不留余量就在最该命中的那一臂上翻车）。
软路径与 D2D 都得改：**不透明度 100% 时窗口走 D2D 直呈，低于 100% 走软路径离屏 +
`UpdateLayeredWindow`**，只补一边等于让用户拨一下设置滑块就换一种性能曲线（与补丁 011
同一处判断）。

**为什么不在宿主修**（把 `desired_pixels` 的 `ceil` 改成 `round`，让档位正好等于框）：
① 上屏矩形还要过 windui 的 `Rect::scaled`（四边各自 round），非整数 DPI 下"双重取整"与
单次取整本来就会差 1，宿主保证不了逐位相等，能保证的只有 `ceil` 那份"覆盖得住"；
② 差 1 像素该不该重采样**是渲染器的判断**，上游那句注释已经承认要吸收 ±1，
这是它兑现自己的承诺，不是为宿主破例。留着 `ceil`：档位比框大 1 时吸附后多出的那一列
由裁剪 mask 收口，一像素不露底。

### 验收（可复现）
**① 分臂代价（`examples/paint_cost_probe`，同一进程、每臂 n=80、报中位）**

| 臂 | 最小 | 中位 | 最大 |
|---|---|---|---|
| 2131² 源 → 2131² 框（吸附成 1:1） | 2.589 | **3.153** | 4.739 ms |
| 2131² 源 → 2130² 框（差 1 px） | 6.113 | **6.626** | 8.495 ms |
| 5526² 源 → 5526² 框（吸附成 1:1） | 2.589 | 2.791 | 3.965 ms |
| 5526² 源 → 5525² 框（差 1 px） | 6.109 | 6.893 | 10.537 ms |
| 直连 tiny-skia：identity 变换 | 2.205 | 2.334 | 3.648 ms |
| 直连 tiny-skia：scale = 2130/2131 | 5.646 | 6.105 | 8.159 ms |

1 px 失配的代价：**windui 2.10×、直连 2.62×**，两个源尺寸（2131²/5526²）各自复现一遍。
"直连"两臂绕开 windui，只差变换是不是整数 1——把成本钉在变换本身上，与源尺寸、
与双线性/最近邻都无关。

**② 判据用例能真的失败**：把两处阈值改回 `< 1.0` 单跑这两条（`.temp/windui_013_control.log`），
软路径与 D2D **各 9 个混合像素**、双双 FAILED；改回来 1087 passed / 0 failed。
三臂设计：`+1` 与 `−1` 必须 0 个插值中间色，`+3` 是反面对照**必须 >0**——
它为零说明阈值放得太宽或判据根本测不到插值。

**③ 真机同轮对照（`drag_cpu_rig4`，784×561 窗口，本补丁后的二进制，一次启动内四臂）**
35% 那臂 img_med **3.31 ms**，与 16% 3.36 / 91% 3.33 / 286% 3.12 ms 落在同一条线上——
"中间倍率单独贵一倍"消失。跨轮的那次 7.00/6.31 ms 只作旁证，判定以这轮的同进程四臂为准
（K50：跨会话的机器状态不是同一支探针）。

### 门禁行
windui `cargo test --lib` **1087 passed / 0 failed**（原 1085 + 本补丁 2）· windui
`cargo fmt --check` 0 diff · windui clippy 无新增（lib 4 条存量 warning 在 `svg_tree_cached`
等处，lib test 5 条，其中 3 条是 `unknown_lints` 重复）· main `cargo fmt --check` /
clippy --release --all-targets **0 warning** / layering 0 / hygiene 0 /
structure doc in sync（55 个 .rs / 符号索引 769 项）· main 测试 **227 lib + 17 集成全绿** ·
`deploy OK`。

### 升级 windui 时的重放
`render/skia.rs` 与 `platform/win32/d2d.rs` 各自 `draw_image` 里那条"物理尺寸与源图相差
1 像素以内吸附 1:1"的判据。若上游自己把 `< 1.0` 放宽，保留两条测试即可——断言的是语义
（差 1 像素不得退化成重采样），不绑实现。

---

## 补丁 014 —— 无边框窗四缘都能缩放，且缩放带光标会变（推翻补丁 05 的顶缘例外）

- 编号：014
- 涉及上游文件（均在 `.3rd/wind-ui-rust-0.23.0/src/`）：
  - `platform/win32/mod.rs`（`handle_nchittest` 的顶缘例外、`WM_SETCURSOR`、`resize_cursor`）
- 应用侧接线：无直接触点；看图器画布铺满客户区且 `focusable`，是受益方。

### 问题（用户实测 2026-10-19）
看图窗口四周边框都拖不动、悬停时光标也不变成缩放箭头。

### 根因（实测，非推测）
- 补丁 05 把交互例外降到**只留顶缘**，理由是"窗口按钮贴着顶边，hover 要稳定"。
  但看图器的顶条不是独立按钮控件——整块画布 `focusable()`，`interactive_at`
  在顶缘恒真，于是**顶缘永远返回 HTCLIENT**，缩放入口彻底消失。
- 另三缘的命中码实测是对的（HTLEFT=10 / HTRIGHT=11 / HTBOTTOM=15），但
  `DefWindowProcW(WM_SETCURSOR)` 在本窗口上没有把光标换成缩放形状（实测四缘
  全是箭头），用户没有任何"这里能拖"的反馈。

### 修复
- `handle_nchittest`：八向缩放码**无条件优先**，删除顶缘交互例外。代价是自绘
  窗口按钮最上 8dp 落在缩放带里——与原生窗口的边框行为一致。
- `WM_SETCURSOR`：新增 `resize_cursor(hit)` 映射（左右→SIZEWE、上下→SIZENS、
  两对角→SIZENWSE/SIZENESW），显式 `SetCursor` 并返回 TRUE。

### 验收（可复现）
- `SendMessage(hwnd, WM_NCHITTEST, …)` 探针：顶缘从 HTCLIENT 变为 HTTOP；
  四角与四边均返回对应缩放码。
- 实机：四缘悬停光标变为缩放箭头，按住可改窗口大小（本机共享桌面无法注入
  真实拖拽时以命中码 + 用户签收为准）。

### 门禁行
windui `cargo test --lib` 全绿 · windui clippy 无新增 · main 全绿。

### 升级 windui 时的重放
`handle_nchittest` 里删掉顶缘 `interactive_at` 例外；`WM_SETCURSOR` 的 NC 分支
加 `resize_cursor` 映射；导入补 `IDC_SIZENWSE` / `IDC_SIZENESW`。

---

## 补丁 015 —— 不透明图片的物理 1:1 绘制走逐行拷贝（省掉整片 src-over）

- 编号：015
- 涉及上游文件（均在 `.3rd/wind-ui-rust-0.23.0/src/`）：
  - `render/image.rs`（`Image.opaque` 字段 + `premultiply_rgba` 顺带判定 + `from_pixmap_parts` / `tinted` / `is_opaque`）
  - `render/skia.rs`（`draw_image` 的快路分支 + `blit_opaque_1to1` + 5 条逐字节用例）
- 应用侧接线：无。`SkiaCanvas` 是出货后端（`App::new` 用 `Renderer::Software`），
  所以所有图片绘制自动受益，宿主一行不改。唯一的外围改动是 `main/examples/paint_cost_probe.rs`
  补了快路/强制通用路的对臂与端点臂。

### 问题（实测，非推测）
`draw_image` 即便吸附成物理 1:1（`Transform::identity()`、整数平移）也仍走
`draw_pixmap(..., Some(&mask))` 的**逐像素 src-over 混合**：m63/m69 同进程配对量到
1:1 进 800×600 要 **2.08 ms（5.35 ns/目标像素）**，而"同面积、同裁剪的整数行拷贝"
只要 **0.075 ms（0.16 ns/像素）**——**33×**。看图器的主形态（≤100% 的派生档、
100% 平移）恰好全是物理 1:1，这笔税每帧都在交；整窗 1920×1080 一帧量到 **11.8 ms**。

### 根因
上游把"图片上屏"统一交给 tiny-skia 的 `draw_pixmap`，它没有"源不透明 + 无缩放 +
整数平移 ⇒ 结果恒等于按行搬运"这个特例；mask 由调用方给，矩形 mask 下的 src-over
等价于裁剪，但混合循环本身（读源、乘 alpha、加目标、写回）仍然逐像素跑一遍。

### 修复
1. `image.rs`：`Image` 增 `opaque: bool`（**durable**，构造时定死）。判定放在既有的
   预乘循环里顺带做（`opaque &= px[3] == 255`），**不新增一次全图扫描**——单独扫
   6055² 一次要 13.2 ms，放进去是零成本。`from_pixmap` 走 `chunks_exact(4)` 扫描；
   `tinted` 继承 `self.opaque && color.a == 255`（着色会引入透明）。`pub(crate) fn is_opaque()`。
2. `skia.rs`：`draw_image` 在算出 `tx/ty`（已取整）与 `pr` 之后、建 Mask 之前插入快路，
   条件全部是"语义上拷贝与混合恒等"的必要条件：
   `opacity == 1.0 && radius <= 0.0 && sx == 1.0 && sy == 1.0 && img.is_opaque()
   && tx.fract() == 0.0 && ty.fract() == 0.0`。
3. 裁剪 = **矩形交集**：当前 `Clip` 的不变量就是"mask 恒为 `rect` 的轴对齐矩形"
   （唯一压栈点 `clip_rect`，无 AA），所以带 mask 的混合可安全替换为
   `target ∩ clip ∩ image` 三段求交后逐行 `copy_from_slice`（裁剪矩形按
   `offset/scale` 换算到物理像素）。快路同时省掉每帧一次的整窗 Mask 分配（0.044 ms）。
4. 逐图 **opt-in 语义而非全局规则**：条件里只认"源不透明"，半透明图（圆角卡片、
   PNG 贴图）仍走原路；缩放/旋转/圆角/半透明度一律不改行为。

### 验收（可复现）
- 新用例（均在 `render/skia.rs`，与既有 `draw_image_*` 同批）：
  - `draw_image_opaque_unit_scale_blit_matches_source_over_per_byte`：快路与
    "绕过 windui、直接 `draw_pixmap` + identity + src-over"的对照**逐字节相同**；
    并断言快路命中计数 == 1（"存在 ≠ 会触发"）。
  - `draw_image_transparent_never_takes_blit_fast_path`：半透明源命中数必须为 0。
  - `draw_image_blit_respects_clip_and_bounds`：图片越出裁剪左右两侧 + 越出缓冲，
    逐像素等于"裁剪交集内的源像素、交集外一个像素都不许动"。
  - `draw_image_blit_declines_radius_opacity_and_scale`：圆角 / 半透明度 / 非 1:1
    三种形态各断言命中数为 0，且结果与慢路逐字节相同。
  - `draw_image_blit_handles_offset_scale_and_negative_placement`：2× DPI + 局部帧
    （offset）下落点部分为负，仍逐像素等于源。
- **先破后立（三次变异，逐次变红后从 `.temp/m69/*.orig` 还原并核对 sha256）**：
  ① 去掉裁剪求交 ⇒ `draw_image_blit_respects_clip_and_bounds` 红（裁剪外出现笔迹）；
  ② 源偏移少加一行（`row - y` 项）⇒ 三条逐字节用例**断言级**红；
  ③ 条件里删掉 `img.is_opaque()` ⇒ `draw_image_transparent_never_takes_blit_fast_path` 红。
- 端点（`paint_cost_probe`，同进程 paired 臂，2026-10-21）：
  | 形态 | 强制通用路 | 快路 | 倍数 |
  |---|---|---|---|
  | 显示档 600×600 → 1:1 进窗（≤100% 主形态） | 2.087–2.336 ms | 0.060–0.066 ms | ≈35× |
  | 1920×1080 显示档 → 整窗 1:1 | 11.691–12.796 ms | 0.328–0.431 ms | ≈30–36× |
  | 一帧@fit（铺底 + 显示档 1:1 + 512² 鸟瞰） | 2.441–2.751 ms | 0.376–0.429 ms | ≈6.4× |
  | 2131²/5526² 源 → 同尺寸框（@100% 平移，补丁 013 那条形态） | （改前 3.15/2.33 ms，m69 arm） | 0.079–0.080 ms | — |
  降采样臂（6055² → 600×600，5.0–5.8 ms）不变——快路不碰缩放。

### 门禁行
windui `cargo test --lib` **1092 passed / 0 failed**（+5 新用例）· windui `cargo fmt --check` 干净
（顺手归位 `draw_text` 一处历史换行错位，存档副本本来就正确）· windui clippy 4 条存量、本补丁 0 新增 ·
main `cargo fmt --check` 干净 · main `cargo clippy --all-targets --release -D warnings` 0 warning ·
main lib **311** · `config_compat` 2 · `config_pipeline_e2e` 5 · `docs_consistency` 12 ·
layering/hygiene 0 违规 · `full.bat` 六步 build complete。exe **+1,536 B**（记在 windui，账入宪章 §4.1）。

### 升级 windui 时的重放
`render/image.rs`：字段 + 预乘循环顺带判定 + 三个构造入口传 `opaque` + `is_opaque()`；
`render/skia.rs`：`draw_image` 里"算完 `tx/ty`、建 Mask 之前"插分支，加 `blit_opaque_1to1`
（三段求交 + 逐行 `copy_from_slice`），保留 5 条逐字节用例（它们断言的是语义，不绑实现）。
若上游自己做了"不透明 1:1 快路"，保留用例即可。

---

## 补丁 016 —— 最近邻放大的不透明图片走整数搬运行（>100% 看图）(省掉逐像素 Gather)

- 编号：016
- 涉及上游文件（均在 `.3rd/wind-ui-rust-0.23.0/src/render/skia.rs`）：
  `draw_image` 的快路分支 + `blit_nearest_opaque` + `clip_phys` + 4 条逐字节用例。
  `Image::opaque` / `is_opaque()` 沿用补丁 015，无 `image.rs` 改动。
- 应用侧接线：无。补丁 011（`with_pixelated`）与补丁 015 同在出货后端，宿主不改。

### 问题（实测，非推测）
补丁 015 只覆盖物理 1:1；**>100% 放大**（看图器核心形态）仍走 tiny-skia 的
`draw_pixmap` + `FilterQuality::Nearest`，每个目标像素走一遍 Gather 管线。
m69 收尾量到 **6.29 ns/可见像素**；m70 用真生产几何复测（6055² 源 → 12110² 目标框、
裁进 800×600 窗）：**3.087 ms/帧**，整帧 @200% **3.067 ms**。同几何强制通用路对臂
确认 6.3 这个数没变；而无 mask 直连 `draw_pixmap` 是 2.228 ms、朴素 gather 循环
1.105 ms——说明大头不在 mask，在管线。

### 根因
tiny-skia 的 Gather 是通用路径：每像素构造 point、做 shader 变换求逆、采样、写回。
最近邻放大语义上只是"每个目标像素取源里同一个整像素"——整行目标共享同一源行，
结果可分解成"一次列映射表 + 逐像素 4 字节拷贝"，与混合/着色无关。

### 修复
1. `draw_image` 在补丁 015 分支之后、建 Mask 之前插分支，条件：
   `opacity == 1.0 && radius <= 0.0 && img.is_opaque() && img.pixelated()
   && (sx > 1.0 || sy > 1.0) && tx.fract() == 0.0 && ty.fract() == 0.0`。
   与 tiny-skia 选择 Nearest 的判据同源（吸附后 `sx > 1.0` 等价于补丁 011 的
   `dw > iw + 1 || dh > ih + 1`），因此**不会抢走双线性的形态**。
2. 覆盖范围 = `目标 ∩ clip ∩ [x, x + round(iw*sx)) × [y, y + round(ih*sy))`。
   非吸附缩放时 `round(iw*sx)` 恰等于 `draw_image` 算出的 `pdst`；裁剪按
   `offset/scale` 换算到物理像素（无 AA，mask 恒为轴对齐矩形）。
3. 映射公式必须与 tiny-skia **逐位同序**：`gather_ix` 的种子是像素中心
   `p + 0.5`，pattern 变换取逆后是 `p*inv + (-t*inv)`，再 `trunc` 并夹到
   `[0, w-1]`。所以：
   `inv = 1/sx; inv_t = -x as f32 * inv; f = (dx as f32 + 0.5) * inv + inv_t`。
   写成 `(p - t)/s` 或调整乘加顺序都会在整数边界上差 1 ulp（见验收里的 6×6→13×13）。
4. 列映射表 `cols` 每帧一次；行内用 `chunks_exact_mut(4).zip` 拷贝，消掉逐像素
   边界检查（朴素 gather 2.3 ns/px，本实现 0.52 ns/px）。源不透明 ⇒ 预乘字节
   等于直色字节，直接覆盖即 src-over 的结果。

### 验收（可复现）
- 新用例（`render/skia.rs`）：
  - `draw_image_nearest_upscale_matches_tiny_skia_per_byte`：7 组几何（含非整数倍率、
    负落点、2× DPI）+ **6×6→13×13 边界用例**（`dx=6` 处 `(6+0.5)*6/13 - 6*6/13`
    在 f32 下恰为 2.9999998，除法写法得 3.0），与"直接 `draw_pixmap` + Nearest +
    同变换"逐字节相同；断言命中数 == 1。
  - `draw_image_nearest_declines_other_forms`：非 pixelated / 半透明 / 圆角 / 缩小时
    命中数为 0，且输出与慢路相同。
  - `draw_image_nearest_respects_clip`：裁剪窗口内逐像素等于源，外一个像素不动。
  - `draw_image_nearest_handles_local_offset_and_dpi`：2× DPI + 局部帧负落点。
- **先破后立（三次变异，逐次变红后从 `.temp/m70/skia.rs.orig` 还原并核对
  sha256 `7bdde448…`）**：
  ① 映射改回 `(p - t)/s` ⇒ 首轮**存活**（既有用例没落在分歧点），补上 6×6→13×13
     边界用例后断言级红；② 去掉裁剪求交 ⇒ `draw_image_nearest_respects_clip` 红；
  ③ 条件删掉 `img.pixelated()` ⇒ `draw_image_nearest_declines_other_forms` 红。
- 端点（`paint_cost_probe`，同进程 paired 臂，WIN 800×600，2026-10-21）：

  | 形态 | 强制通用路 | 快路 | 倍数 |
  |---|---|---|---|
  | 6055² 源 @200%（12110² 框，裁进窗） | 2.897 ms | 0.242 ms | **12.0×** |
  | 900×700 源 1.78×（真机切片形态） | 2.795 ms | 0.251 ms | **11.1×** |
  | 6055² 源 2.0× | — | 0.258 ms | — |
  | 整帧 @200%（铺底 + 主图 + 鸟瞰） | 3.067 ms（改前） | 0.538 ms | ≈5.7× |
  | 每可见像素 | 6.29 ns（改前） | 0.52 ns | 12× |
  | 无 mask 直连 `draw_pixmap` Nearest | 2.228 ms | — | 对照下限 |
  | 朴素 gather（预计算列表 + 边界检查） | 1.105 ms | — | 对照下限 |

  26% 降采样臂（6.7–7.1 ms）、1:1 臂（0.06–0.08 ms）不变——本补丁只碰放大。

### 门禁行
windui `cargo test --lib` **1096 passed / 0 failed**（+4 新用例）· windui `cargo fmt --check` 干净 ·
windui clippy 本补丁 0 新增 · main `cargo fmt --check` 干净 ·
main `cargo clippy --all-targets --release -D warnings` 0 warning · main lib **311** ·
`config_compat` 2 · `config_pipeline_e2e` 5 · `docs_consistency` 12 ·
layering/hygiene 0 违规 · `full.bat` 六步 build complete。exe **+3,072 B**（记在 windui，账入宪章 §4.1）。

### 升级 windui 时的重放
`render/skia.rs`：`draw_image` 里补丁 015 分支之后插本分支，加 `blit_nearest_opaque`
与 `clip_phys`（若 015 已带 `clip_phys` 则复用），保留 4 条逐字节用例——它们断言的是
"与 tiny-skia 最近邻逐字节一致"，不绑实现。若上游给 Nearest 做了整像素快路，
保留用例即可；**映射的 f32 运算顺序是与上游对齐的契约，不是实现细节**。

---

## 补丁 017 —— 无文字的 SVG 不再取系统字体库（启动关键路径 −约 300 ms）

- 编号：017
- 涉及上游文件：`src/render/image.rs` —— 新增 `svg_may_have_text(bytes)`，
  `svg_tree_cached` 里那句**无条件**的 `opt.fontdb = svg_fontdb();` 改成按判据取；
  另加 2 条用例（`svg_may_have_text_predicate_forms` / `text_free_svg_renders_without_fontdb`）。
- 应用侧接线：无。宿主源码一字未动（`main.rs` 的 `svg_warm_fonts` 预热线程照旧留着，
  它服务的"首张带文字的 SVG"今天才真的被提前到后台做掉）。
  ★ **2026-10-22 m87 补记**：宿主**已不再调用** `svg_warm_fonts`（`main.rs` 的预热线程删除）。
  补丁 017 之后正常启动没有任何取库者，那枚线程成了"为一张可能永远打不开的带文字 SVG
  每次开机扫一遍字体"；m87 端点读数（正反序、n=6）：删掉它省 **−21.4 ~ −36.4 MB 启动期峰值**、
  RAF 首开 `cost=` **−29 ms**，代价是进程内第一次打开带文字 SVG **+270 ms（一次）**。
  `svg_warm_fonts` 作为 windui 的库 API 保留，只是宿主不调用。见 `HISTORY.md`。

### 问题（实测，非推测）
RuPix 每次启动都有一段与格式、与图片尺寸都无关的固定开销。m86 新立的
`main/build/startup_probe.ps1`（秒表起在 `Start-Process` 之前，只认 Win32 窗口判据与
产品自己的日志行）量到：**空启动 437~452 ms 才见窗，120×80 的小图首帧 376 ms 而它自己
的 `[main-latency] cost=` 只有 15.6 ms**。宿主插桩（17 个 `[startup]` 点）进一步定位：
我们自己从 `anchor` 到 `root_built`（即将调用 `App::run()`）只用 **0~12 ms**，
而 `root_built → windui 第一次 interval tick` 是 **348~391 ms**。

### 根因
`App::run()` → `create_window` → `apply_window_icon`（大小图标各一次）→ `ImageSource::at(px)`
→ 宿主 `ui/brand.rs` 的闭包 → `Image::from_svg_bytes` → `svg_tree_cached`；托盘 `tray::install`
同一条路。而 `svg_tree_cached`（补丁 06 的成果）里 `opt.fontdb = svg_fontdb();` 是**无条件**的，
`svg_fontdb()` 第一次取值就是整轮 `fontdb::Database::load_system_fonts()`（补丁 06 自己的注释
写着「扫一遍全系统字体（实测 ~900ms）」）。**本仓 `main/image/` 三枚 SVG 实测零 `<text`**——
一张文字都没有的矢量图标，把约 300 ms 压在了每次开机的关键路径上。
★ 补丁 06 把库收成全局 `OnceLock` 是对的（它消灭的是"每个线程各扫一遍"），漏的是**该不该扫**
这一维：全局化让"只付一次"成立，但那次付款落在了启动上。后台预热也救不了——主线程几十毫秒
就撞进同一个 `OnceLock`（消融那条线程只移动 20~40 ms，在噪声内）。

### 修复
1. `svg_may_have_text(bytes)`：字节流里出现 `<text`（覆盖 `<textPath`）或 `:text`
   （带命名空间的写法）才算需要字体库；两者大小写无关。
2. ★ **gzip 魔数（`.svgz`）一律当真**：resvg 内部透明解压，这里看不见内容。
   两个误判方向不对等——判成"有"只多付一次早已全局化的扫描，判成"没有"会**悄悄把字形丢掉**，
   所以宁可错付。
3. 判据为假时用 `Options::default()` 里那份空库解析纯图形 SVG；判据为真的照旧共享全局那一份。
   解析缓存的键不变（同一份字节的判定结果恒定）。

### 验收（可复现）
- 新用例（`render/image.rs`，与宿主同档 `--no-default-features --features svg,svg-text,d2d`）：
  - `svg_may_have_text_predicate_forms`：纯图形假 / `<text`、`sv:text`、gzip 魔数真，
    并反向钉「`<svg:text>` 不含 `<text` 子串」⇒ 第二条 needle 是承重的。
  - `text_free_svg_renders_without_fontdb`：判据为假那张图（`RED_SVG`）中心仍是同一个红像素。
- 既有用例 `renders_svg_text_glyphs`（带文字 SVG 出字形）**照旧绿**＝本轮回归的守门人。
- 宿主侧 `ui::brand::tests::pe_icon_asset_matches_freshly_rasterized_logo` 与
  `tray_icon_matches_app_icon` 全绿——那枚 `.ico` 是**旧路径**生成的字节资产，
  新路径现场光栅化与它逐像素相同 ⇒ 图标画面一字未变，变的只有时间。
- **先破后立（`.temp/m86_mutate.py`，台账 `.temp/m86_mutations.txt`，跑完按 sha256 `45b5ddc1…` 还原）**：
  ① 判据恒假（任何 SVG 都不给字体库）⇒ `renders_svg_text_glyphs` **＋** 判据用例双双红；
  ② 摘掉 gzip 那一支 ⇒ `svg_may_have_text_predicate_forms` 红；
  ③ 摘掉 `:text` 那一支 ⇒ `svg_may_have_text_predicate_forms` 红。
- ★ 有一条"变异"故意不红：把判据写回恒真（＝退回补丁 06 的形状）**画面一字不变**，
  因为本轮的主张本来就是"纯图形两种写法出同一张图"。**"有没有省"由端点读数判，不由用例判。**

---

## 补丁 018 —— 接收窗提前到「取得互斥体」的同一刻，二次实例的 argv 不再丢在空档里

- 编号：018
- 涉及上游文件：
  - `src/single_instance/win.rs`：新增 `SI_HWND` / `SI_HWND_TID` / `MAX_PENDING` / `PENDING`
    与 `ensure_listener_window` / `drop_early_window` / `listener_ready` / `push_pending` /
    `drain_pending` / `deliver_argv`；`install_listener` 改为「复用已提前建好的窗口 + 回放
    积压的 argv」，`si_wnd_proc` 里那段回调逻辑抽成 `deliver_argv` 供两处共用；
    另加 2 条用例（`pending_argv_round_trips_in_arrival_order` / `pending_queue_is_bounded`）。
  - `src/single_instance/mod.rs`：`claim_instance` 与 `arbitrate` 判出首实例的同一刻调用
    `ensure_listener_window`。
- 应用侧接线：无（`main.rs` 的 `claim_instance` 调用形状一字未改）。新增的是**一条约束**：
  它与随后的 `App::run()` 必须在同一线程——本来就如此（`App::run` 在调用它的线程上建窗跑泵），
  不一致时 `install_listener` 会先把那枚没人泵的窗口销毁再按原样重建，行为退回补丁之前。

### 问题（声称与成立的差额，不是用户报障）
`forward` 靠 `FindWindowW(class_name(app_id))` 找首实例的接收窗，而那枚窗原先只在
`App::run()` 里的 `install_listener` 时才建。互斥体从 `claim_instance` 那一步就归本进程
（进程活着期间一直持有），两者之间隔着应用自己的整段启动。**这段空档里进来的二次实例找不到窗**
⇒ `forward` 判"送不到" ⇒ 回落 `InstanceRole::First` ⇒ 它把命令行解码栈、解码器目录、配置、
画廊全部再跑一遍，然后走到 `App::run` 里的第二次仲裁（`arbitrate`）——那次能不能救回来，
取决于它自己跑到那一步时首实例的窗是否已经建好，是个**纯竞态**。

用户可见的场景是资源管理器里选中多张图回车（Explorer 一次性拉起 N 个进程），以及旧机器上
二次启动比首次快（后者冷读 DLL、前者已经 warm）的情形。

### 根因
`install_listener` 把两件事捆在了一起：**窗口的存在**（二次实例找不找得到）与
**回调的就位**（收到 argv 谁来处理）。前者只需要早到，后者必须等应用把 `on_second` 交出来。

### 修复
1. `ensure_listener_window(app_id)`：幂等地把那枚 message-only 窗建出来，`claim_instance`
   （以及没走过它的 `arbitrate`）拿到互斥体的同一刻就调一次 ⇒ 此后到达的二次实例**立刻**
   找得到窗，"空档"在存在性这一侧被抹掉。
2. 回调就位之前的 argv 不再静默丢弃：`si_wnd_proc` 见监听器未就位就进 `PENDING`（有上限），
   `install_listener` 装完回调后按到达顺序回放一次。
3. 线程守卫：窗口的消息由建它的线程泵；`install_listener` 发现不在同一线程时销毁那枚窗再
   重建，补丁因此不会引入新的失败模式。

### 验收（可复现）
- 新用例（`single_instance/win.rs`）：积压 argv 按序回放且取出即清空；队列不超过
  `MAX_PENDING`（外进程不该能无限往本进程堆东西）。
- 真机（`.temp/m128_gap.txt`，**临时探针，量完已从 `.3rd` 摘掉**，读数见 `HISTORY.md`）：
  `ensure → install` 这段空档 **32.3 ~ 82.9 ms（n=12，小图热启动）**；过去这段时间里
  接收窗压根不存在。
- 真机双实例（`.temp/m128_si_race.txt`，8 组）：先后拉起两枚进程（间隔 0/20/150/400 ms）
  ⇒ **每次都只剩 1 个进程**，且运行日志里两张图都被同一枚进程解过（`[main-latency]` 两条），
  说明第二枚的 argv 真的被接收下来而不是"转发失败后自己再开一个窗"。
- 真机模拟 `forward()`（`.temp/m128_si_forward.txt`，不靠第二个进程、直接用 WM_COPYDATA）：
  接收窗首见于 t=119.5 ms 且**此刻 `install_listener` 还没跑到**（被 m128 探针证实），投递
  阻塞 71.5 ms 后成功，`m128_b.png` 被打开，进程数 1。
- ★ 回放分支在本机上**没有被真机走到**：本仓的消息泵在 `install_listener` **之后**才起，
  所以外部投递总是落在回调就位之后。它是保险（任何"先泵后装回调"的宿主都要靠它），
  覆盖面由上面 2 条单元用例守着，不在"已用真机复现"的口径里。
- 回归：宿主 `cargo test --lib` 全绿（最终数见 `HISTORY.md`）；windui
  `cargo test --lib` **1101 passed**（默认集与 `--no-default-features --features svg,svg-text,d2d`
  两档各跑一遍，含本机 2 条新用例）。

---

## 补丁 019 —— 文本输入的无边框构器（纯文本窗）

- 编号：019
- 涉及上游文件：`wind-ui-rust-0.23.0/src/ui/mod.rs`（`impl Element` 的文本输入构器区）
- 应用侧接线：`main/src/ui/text_window.rs`

### 问题（用户实测）
插件的纯文本结果窗要求对齐 CurSeen 的"信息档"：**没有框线、没有聚焦框线变色、背景纯白**。
windui 的 `TextInput` 在非 frameless 时会画圆角底与边框，聚焦时边框还会变色，
整片文本区看起来像一只输入框。

### 根因
0.23.0 的 `TextConfig` 已有 `frameless` 字段（复合控件内嵌用），但**没有公开的
构建器**把它打开；`Element::text_input(..)` 的公开链式方法里没有对应入口。

### 修复
`Element` 新增：
```rust
pub fn frameless_input(self) -> Self { self.config_text_input(|c| c.frameless = true) }
```
只打开已有的 `frameless` 绘制分支（`inputs.rs` 里 `if !self.config.frameless` 那一段），
不改布局、不改主题、不改其他控件。

### 验收
- `text_window.rs` 用 `.frameless_input()` + 白色窗口底；无框线、无聚焦框线变色。
- `hygiene_scan` 第 7 查：本章节 ↔ `.3rd` 的 `★ RuPix 补丁 019` 标记对得上。
- `docs_consistency::j13`：`patches/wind-ui-rust-0.23.0/src/ui/mod.rs` 与 `.3rd` 工作副本逐字节相同。

### 门禁
宿主单测 **516 passed / 1 ignored** · 集成 **13 passed**（含 j13 归档逐字节）·
clippy `--all-targets --release -- -D warnings` 0 · layering 0 · hygiene 0 · fmt 0。

---

## 补丁 020 —— 撤掉搜索镜的矢量自绘分支（放大镜改回普通字形直绘）

- 编号：020
- 涉及上游文件：`wind-ui-rust-0.23.0/src/ui/inputs.rs`（`TextInput::paint` 的前置图标分支）
- 应用侧接线：`main/src/ui/settings/rows.rs::search_box`（改用 `U+E71E` + `.leading_icon`）

### 问题（用户实测）
设置页三张卡的搜索框放大镜是**宿主侧自绘**的（圆环 + 手柄两段矢量），而参考项目
CurSeen 的官方资源页搜索框只是普通地画一个字形。用户口径：自绘没必要，改成和它一样。

### 根因（这条自绘是给"另一个码位"打的补丁）
链路本来是这样：搜索镜原先用 `U+E721`，而该字形原生「圆圈在右、手柄朝左下」，
观感反了 → **补丁 09** 给 `Canvas` 加了镜像绘制；镜像路径又在焦点重绘时擦出图标区的
背景块（2026-10-28 用户截图）→ 于是改成绕开字形的矢量自绘。**两层补丁叠在一个
本来朝向就错的码位上。**

### 修复
换用朝向本来就正确的 `U+E71E`（与 CurSeen 同一个码位），走 `draw_text` 普通绘制：

- `inputs.rs` 删掉 `if g == '\u{E721}' { draw_search_icon(..) }` 那一臂与 `draw_search_icon`
  整个函数（本补丁撤的就是补丁 09 追加的这一段）。
- **不动**补丁 09 的镜像原语（`draw_text_mirrored` / `leading_icon_mirrored`）：那是通用
  能力，别的图标可能要用，撤掉会让 09 的归档对账整体返工。
- 应用侧 `search_box` 从 `.leading_icon_mirrored('\u{E721}')` 改为 `.leading_icon('\u{E71E}')`。

### 验收
- 码位覆盖**直读字体 cmap 核对**：`%WINDIR%\Fonts\segmdl2.ttf` 里 `U+E71E` → glyph 8248
  （`U+E721` → 8152），不是"看着应该有"。
- 像素：`cargo run --release --example settings_preview`（探针已补注册私有字形区字体，
  否则只会看到一个方框）→ 三只搜索框的放大镜朝向正常、焦点切换不擦背景。
- `hygiene_scan` 第 7 查：本章节 ↔ `.3rd` 的 `★ RuPix 补丁 020` 标记对得上。
- `docs_consistency::j13`：`patches/wind-ui-rust-0.23.0/src/ui/inputs.rs` 与 `.3rd` 工作副本逐字节相同。

### 门禁
升级 windui 版本时：先按 09 的原语重放，再看上游有没有把搜索镜图标做成可配置；
若上游自带朝向正确的搜索字形，本补丁与 09 的镜像臂可一并撤掉。

---

## 补丁 021 —— 多行文本框的中日韩断行（`wrap_paragraph` 认宽字符，不再只认空格）

- 编号：021
- 涉及上游文件（`wind-ui-rust-0.23.0/src/`）：
  - `ui/inputs.rs`（`wrap_paragraph` 的断点回退判据；新增四个私有纯函数
    `can_break_before` / `is_wide_cjk` / `is_closing_punct` / `is_opening_punct` + 六条单测）
- 应用侧接线：无（`text_input.multiline().wrap(true)` 是既有用法；宿主 `ui/text_window.rs`
  的插件结果窗与属性类纯文本窗直接受益）。

### 问题（用户实测，2026-10-31）
「图片体检」结果窗里**长中文行整句被搬到第二行开头**，第一行只剩几个字符。
取证：`- ★ 这一节把背景当前景，相对亮度比…` 这一行里唯一的 U+0020 在开头（`- ` 与 `★ ` 之后），
旧判据"回退到最后一个空格之后"于是把首行截成 4 个字符——实测读数
`assertion left == right failed: 首行只放了 4`（宽 50 本可放 5 个字符）。

### 根因
`wrap_paragraph` 的断点表只由 `chars[k] == ' '` 构成。这套判据是从西文词换行抄来的：
西文没空格就等于没断点，回退是**对的**；中文逐字可断，回退到唯一的空格就把整行清空了。
上游自己另一条路径（`text/linux/layout.rs::can_break_before`，`#[cfg(target_os = "linux")]`）
**已经有**正确判据（空白后可断 + CJK 逐字可断 + 闭合标点不落行首 + 开启标点不落行尾），
Windows 的 TextInput 路径没有它。

### 修复
1. `ui/inputs.rs` 加四个私有纯函数，判据与 Linux 那份同族：空格之后可断、紧邻宽字符
   （CJK/假名/谚文/全角形式/CJK 标点）可断、闭合标点（，。、）」… 等）不许开行首、
   开启标点（（「《【 等）不许吊行尾；一个断点都找不到时仍硬断在宽度边界（超宽单字符兜底不变）。
2. `wrap_paragraph` 的回退从"最后一个空格之后"换成"`(ls, e]` 里最后一个允许的断点"。
   不变量照旧：断点 `k ≥ ls+1` ⇒ 每行至少推进一格，不会死循环。
3. ★ **刻意不与 Linux 那份共享模块**：那块整片 `#[cfg(target_os = "linux")]`，本机编译不到，
   接成共享真源等于把验不了的代码挂进主路径；两份从不参与同一次构建，走散的代价是"某一端
   排版稍不同"，接错的代价是"主端编译失败"。两份的**判据文字**在各自注释里互相指认。
4. 左/右引号用 `'\u{201C}'` / `'\u{2018}'` 写死码位：手打智能引号会被代码页吞成
   `'"'` 与 `'''`（本轮实际踩过，编译期才抓住）。

### 验收（可复现）
- windui 单测（`cargo test --lib -- --test-threads=1`，全库 **1104 passed / 0 failed**）：
  - `cjk_line_fills_the_width_instead_of_backing_off_to_the_first_space`——钉"首行吃满宽度"；
  - `latin_word_wrap_is_unchanged_by_the_cjk_rule`——反面对手：西文仍只在空格后断，
    且逐行断言"软换行的最后一格必须是空格"（不许出现 `alph`/`a` 这种劈词）；
  - `closing_punctuation_is_not_left_at_the_head_of_a_line`、
    `opening_punctuation_is_not_left_at_the_tail_of_a_line`——避头尾两半各一枚；
  - `mixed_paragraph_covers_every_char_once_and_always_advances`——拿报告真实一行，
    四档宽度下断言区间首尾相接、不重不漏、每行至少一个字符；
  - `a_single_oversized_character_still_gets_its_own_line`——超宽单字符兜底不变。
- **红证**：把回退判据临时换回旧写法（只认 U+0020），三枚中文用例当场翻红
  （`首行只放了 4` / 逗号落在行首 / 括号吊在行尾），而西文与旧三枚用例
  （`wrap_paragraph_char_wrap` / `_word_break` / `_nowrap_and_empty`）**照绿**——
  证明钉住的是中文那一格，也没有把既有行为改成第二套判据。
- `cargo clippy --lib` 对本补丁零新增告警（库内存量 4 条不在此列）。
- `hygiene_scan` 第 7 查：本章节 ↔ `.3rd` 的 `★ RuPix 补丁 021` 标记对得上。
- `docs_consistency::j13`：`patches/wind-ui-rust-0.23.0/src/ui/inputs.rs` 与 `.3rd` 工作副本逐字节相同。

### 门禁
升级 windui 版本时：先看上游有没有把 TextInput 的折行换成 Pango/UAX #14 一类带 script
stack 的实现（若有，本补丁整段撤掉）；若仍是"只认空格"，按上面第 1–2 步重放。
★ 宿主 `ui/text_window.rs` 的 `visual_lines` 只是**估窗高**的近似（CJK 记整号宽、其余 0.55 倍），
不参与排版；本补丁让实际折行更贴近它那根"按宽度切满"的假设，估高因此只会更准，不需要跟着改。

---

## 补丁 022 —— SVG 渐进首绘（`from_svg_bytes_no_text`：剥 `<text>` 先出图形，完整版随后补）

- 编号：022
- 涉及上游文件：`wind-ui-rust-0.23.0/src/render/image.rs`
  （私有 `svg_strip_text_nodes` + 公开 `Image::from_svg_bytes_no_text` + 三条单测）
- 应用侧接线（**2026-11-02 起已撤**，见下方状态）：`main/src/core/decode.rs`
  （`SvgDecodeFn` 加 `no_text` 参）→ `main/src/ui/svg.rs::decode` →
  `main/src/core/viewer/schedule.rs`（矢量首绘走剥字趟、落地后补一次完整档）。
- **状态（2026-11-02，m197）**：宿主侧不再有任何调用方——用户裁定"文字晚到像程序有
  缺陷"，渐进首绘整体删除（`ReqKind::VectorFirst` / `no_text` 参数 / `svg_progressive`
  键 / 选项页一行）。补丁本体与三条单测**留在 `.3rd`**：`svg_strip_text_nodes` 是库内
  自洽能力，撤掉要重打补丁并重新登记，收益为零；宿主不调它，它就不在任何路径上。

### 问题（用户实测，2026-11-01）
带 CJK 文字的 SVG 双击打开要卡 0.4–2.3 s，菠萝看图（QtSvg）同图秒开。

### 根因（m183/m184 探针实测归因）
usvg 在**解析期**就把 text 转成轮廓（ shaping ），每份带文字的文档固定 ~390 ms +
每段 ~3–20 ms；这与字体库大小无关（换小库、族命中的文档照样 407 ms，指纹一致），
QtSvg 没这一步是因为它把 shaping 推迟到 paint 期、且 RuPix 用 resvg 架构上拿不到那个位置。
唯一能把首屏做快的路：**第一趟不解析文字只画图形**。实测剥字趟 图片1.svg
948 ms → 13 ms，两趟指纹分离。

### 修复
1. 私有 `svg_strip_text_nodes(bytes) -> Option<Vec<u8>>`：删掉 `<text …>…</text>` 与
   `<text …/>`（大小写不敏感）。返回 `None` 三种情况：无可识别 text（剥与不剥同键）、
   gzip 包装（.svgz 内容看不见不猜）、找到 text 但收尾丢了（形态超出识别能力，保守走全趟）。
   只认不带命名空间前缀的 `<text`——usvg 只把 SVG 命名空间里的 text 当文字。
2. 公开 `Image::from_svg_bytes_no_text(bytes, target_width)`：剥得动就按剥后的字节
   光栅化，剥不动（None）结果与 `from_svg_bytes` **完全一致**——调用方无需预判。
   剥后的字节无 text ⇒ `svg_may_have_text`（补丁 017）判假 ⇒ 还顺带免掉首次字体库
   索引扫描（OnceLock 冷启 ~250 ms）。
3. **不动**解析缓存（`svg_tree_cached`，CAP 8 整体清空）：两趟至多占两键，够用；
   树很小，补丁 06 的"粗暴淘汰足够"结论照旧成立。
4. 应用侧语义：剥字趟只是**首屏**，随后必有一次完整档跟进补文字（宿主 `schedule.rs`
   的 refine-once 守卫），缩放/平移用的区域渲染始终走完整树。

### 验收（可复现）
- windui 单测（`cargo test --lib -- --test-threads=1`，全库 **1105 passed / 0 failed**）：
  - `strip_text_node_forms`——自闭合/配对/大小写/textPath 不算/命名空间前缀不算/
    无文字 None/收尾丢失 None/gzip None；
  - `no_text_pass_matches_full_when_text_free_and_differs_with_text`——无文字两趟
    **逐字节相同**（调用方无需预判的契约），带文字两趟像素分离且剥字趟无字形残留。
- 宿主端到端读数见 `.plan/m184`（原型量具 `examples/svg_progressive_probe` 已随 m197 删除，此处不写后缀，免得被 j3 当成现存文件）。

### 门禁
升级 windui 版本时：先看上游有没有把 text 的 shaping 挪出解析期（挪了则本补丁
整段撤掉）；没有就按第 1–2 步重放。`hygiene_scan` 第 7 查：本章节 ↔ `.3rd` 的
`★ RuPix 补丁 022` 标记对得上；`docs_consistency::j13`：
`patches/wind-ui-rust-0.23.0/src/render/image.rs` 与 `.3rd` 工作副本逐字节相同。

---

## 补丁 023 —— SVG 字体索引持久缓存（`svg-fonts.bin`：省掉每会话一次的系统字体扫描）

- 编号：023
- 涉及上游文件：`wind-ui-rust-0.23.0/src/render/image.rs`
  （`SVG_FONT_CACHE_PATH` + 公开 `svg_set_font_cache_path` / `svg_fonts_ready` +
  私有 `svg_font_cache_load` / `svg_font_cache_store` + `svg_fontdb` 接线 + 一条单测；
  m187 增补：全局 `SVG_FONTDB` 提到模块级，`svg_fonts_ready()` 读初始化状态供宿主
  首解分岔）
- **状态（2026-11-02，m197）**：`svg_fonts_ready()` 的宿主消费方（首解单趟/剥字分岔）
  已随渐进首绘一起删除；`svg_set_font_cache_path` 与启动后台预热**照旧**——字体索引
  缓存本身省的是每会话一次的系统字体扫描（~295 ms → ~11 ms），与本轮删除无关。
- 应用侧接线：`main/src/lib.rs::install_decode_stack`（传 exe 旁
  `cache\svg-fonts.bin`——m187 起，不再放 `temp\`：那是按天清理的取证区，纯提速件
  不住那；必须在首次 SVG 解析前调）+ `runtime::run` 启动后台预热 + 调度侧
  `core::decode::svg_fonts_ready()` 分岔（热了单趟完整解析，冷了才剥字渐进）。

### 问题（用户实测，2026-11-01）
m185（usvg 补丁 U02）之后真机复测：首绘 9.8 ms，但完整跟进仍要 **294.6 ms**——用户
口径"文字显示依然有延迟，别的看图软件都是秒开的"。

### 根因（实测归因）
这 295 ms 是 fontdb `load_system_fonts()` 的**全系统字体扫描**（本机 ~1900 面），
每进程付一次、挂在第一张带文字 SVG 的完整解析上（m184 的两趟调度把它挪出了首绘，
但没消掉）。DirectWrite/GDI 的字体集合由 OS 进程间共享，别的看图器没有这笔账；
usvg 架构上必须吃一份 `fontdb::Database`，所以**索引必须自己保存**。

### 修复
1. `svg_fontdb()` 的 `get_or_init`：有落点就先回放持久索引；指纹不符（装/卸字体、
   文件变动）才 `load_system_fonts()` 并把新索引写回。
2. 缓存内容 = face 索引（路径/index/家族名/PostScript 名/style/weight/stretch/
   monospaced）+ 指纹区（每个唯一字体文件的路径/大小/修改时刻，全部吻合才采用）。
   face 的 ID 不进缓存——回放经 `fontdb::push_face_info` 现场重发，消费方（usvg）
   拿到的 ID 语义自洽。
3. 失效安全：任何计数字段越界、文件缺失、mtime/大小不符、字节损坏 → 整份作废
   重扫，**不做半信半疑的局部采用**；家族名的 `Language` 回放一律填
   `English_UnitedStates`（fontdb 的 query 只比较家族名字符串，语言标签不参与
   决策——`fontdb-0.24.0/src/lib.rs` 的 `query` 内 `face.families.iter().any(|f| f.0 == name)`）。
4. 落盘 = 写临时 + 原子改名，失败静默（只读介质 → 每次现扫，缓存是纯提速件）。

### 验收（可复现）
- windui 单测（全库 **1110 passed / 0 failed**）：
  `font_index_cache_roundtrip_matches_scan_and_rejects_mismatch`——回放库与现扫库
  面数/家族名集合/PostScript 名逐一相等；回放库 query 命中；改写字体副本（mtime 变）
  → 拒绝且零面进库；损坏字节 → 拒绝。
- 宿主真机读数见 `.plan/m186`（跟进档 294.6 ms → 预期 <50 ms）。

### 门禁
升级 windui 版本时：若上游 fontdb 自带索引持久化则整段撤掉；否则按第 1–4 步重放。
`hygiene_scan` 第 7 查：本章节 ↔ `.3rd` 的 `★ RuPix 补丁 023` 标记对得上；
`docs_consistency::j13`：`patches/wind-ui-rust-0.23.0/src/render/image.rs` 与
`.3rd` 工作副本逐字节相同。

---

## 补丁 024 —— 宿主自绘光标的覆盖通道（拖图时手型不再闪回箭头）

- 编号：024
- 涉及上游文件：`wind-ui-rust-0.23.0/src/platform/mod.rs`
  （`CURSOR_OVERRIDE` 原子 + 公开 `set_cursor_override` / `cursor_override`）、
  `wind-ui-rust-0.23.0/src/platform/win32/mod.rs`
  （`WM_SETCURSOR` 分支最前插一段、导入 `HCURSOR`）
- 应用侧接线：`main/src/platform/cursor.rs`（`use_hand` 登记覆盖、`use_arrow` 撤覆盖）

### 问题（用户实测，2026-11-03）
抓手光标接上之后，按住左键拖图时**光标一闪一闪地回到箭头**（"观感很不好"）。

### 根因（实测，非推测）
鼠标每次移动，系统都发 `WM_SETCURSOR`；框架应答时按 `AppHandler::cursor()` 的形状
`SetCursor`（画布没覆写 ⇒ `Arrow`）。宿主随后在自己的 `WM_MOUSEMOVE` 里 `SetCursor`
回自绘手型，两次调用前后脚，屏幕上就是手型↔箭头反复闪。

取证方式（`.temp/cursor_probe9.py`）：先让光标变成手型，再手动投一枚
`WM_SETCURSOR(HTCLIENT)` 给窗口，紧接读 `GetCursorInfo` —— 句柄从自绘句柄
（如 `0x4170C41`）变回 `0x10003`（`IDC_ARROW`）。同一次拖动里逐帧采样也能抓到
序列中偶现的 `0x10003`。

`CursorShape` 只有箭头/手型/文本/左右/上下五档，表达不了 `CreateIconIndirect` 造出来的
自绘光标，所以"让 `cursor()` 返回抓手"这条路不存在。

### 修复
1. `platform/mod.rs`：加模块级 `CURSOR_OVERRIDE: AtomicIsize`（0 = 无覆盖）与
   `set_cursor_override(Option<isize>)` / `cursor_override() -> Option<isize>`。
2. `platform/win32/mod.rs`：`WM_SETCURSOR` **最前**判覆盖——有值就
   `SetCursor(HCURSOR(h))` 并 `return LRESULT(1)`，不再问形状、也不再走缩放边框那支。
3. 宿主 `platform/cursor.rs`：`use_hand` 里 `set_cursor_override(Some(h))` 再 `SetCursor`；
   `use_arrow` 里**先**撤覆盖再设箭头（顺序反了，紧随其后的 `WM_SETCURSOR` 会把手型
   设回来，指针停在手上）。

### 验收（可复现）
- `.temp/cursor_probe9.py`：悬停读到自绘句柄；投 `WM_SETCURSOR` 之后**仍**是自绘句柄
  （修复前会变回 `0x10003`）。
- `.temp/cursor_probe8.py`：拖动中逐帧采样只出现两枚自绘句柄（开手/握拳），不再夹杂
  `0x10003`。
- 真机：按住拖图，光标保持握拳，松手回开手，不闪。

### 门禁
升级 windui 时：若上游给 `CursorShape` 加了自定义句柄变体（或提供等价的"宿主接管光标"
钩子），本章整段撤掉、宿主改走上游 API；否则按第 1–3 步重放。
`hygiene_scan` 第 7 查：本章节 ↔ `.3rd` 的 `★ RuPix 补丁 024` 标记对得上；
`docs_consistency::j13`：`patches/wind-ui-rust-0.23.0/src/platform/mod.rs`、
`.../src/platform/win32/mod.rs` 与 `.3rd` 工作副本逐字节相同。
宿主侧回归：`ui::hand_cursor::hands_are_registered_as_cursor_override`（覆盖随手型登记、
`apply(None)` 撤净）。门禁行：layering 0 / hygiene 0 / lib **610 passed** /
docs_consistency **15 passed**。

---

## 补丁 025 —— 应用级退出的公开入口（`EventCtx::quit_app`）

- 编号：025
- 涉及上游文件：`wind-ui-rust-0.23.0/src/core.rs`（`EventCtx::quit_app`）、
  `wind-ui-rust-0.23.0/src/platform/mod.rs`（`pub use win32::post_quit` + 两平台桩）、
  `wind-ui-rust-0.23.0/src/platform/win32/mod.rs`（`pub fn post_quit`）
- 应用侧接线：`main/src/runtime/mod.rs`（`CloseAction::Quit` → `ctx.quit_app()`）、
  `main/src/runtime/mod.rs` 的应用级轮询兜底（零窗口时 `windui::platform::post_quit()`）

### 问题（用户裁定，2026-11-04）
多开改造要求「窗口可以有多个，但进程与托盘只有一个」⇒ 界面必须全部由 `Window` 建出、
进程跑常驻模式（`App::run_resident`，零主窗）。常驻模式**刻意**不在最后一个窗口销毁时
退消息循环（`should_quit_on_last_window(resident=true, was_last=true) == false`），
于是"用户关掉最后一扇窗、且没开关闭到托盘 ⇒ 进程该走了"这件事表达不出来。

### 根因（源码定位，非推测）
应用级退出此前**只有托盘那一个入口**：`TrayCtx::quit()` → `TrayAction::Quit` →
平台 `quit_app()`。`EventCtx` 上只有 `request_close()`（"这个窗口能不能关"）与
`force_close()`（跳过决策链关本窗），两者都作用在**窗口**上；非常驻模式还能靠
"主窗销毁 → 消息循环退出"间接达成，常驻模式没有主窗可关。

### 修复
1. `core.rs`：`EventCtx::quit_app()` —— 直接调 `crate::platform::post_quit()`。
2. `platform/win32/mod.rs`：`pub fn post_quit()` = `PostQuitMessage(0)`。**不复用**
   `quit_app()`：后者同步 `DestroyWindow` 全部窗口，而本方法要在 `wnd_proc` 内部调用
   （正持有窗口状态借用），那正是铁律 6 的 `&mut` 别名 UB。窗口资源随消息循环收尾
   由平台统一销毁。
3. `platform/mod.rs`：Windows 导出真实现；macOS/Linux 给空桩（与 `run_resident`
   在那两个平台"未实现"的口径一致——RuPix 只发布 Windows）。

### 验收（可复现）
真机：开「允许多开」→ 开两扇窗 → 关掉一扇，另一扇仍在、进程还在（任务管理器一条）；
关掉最后一扇（「关闭到托盘」不勾）→ 进程退出、托盘图标消失。
勾「关闭到托盘」再关最后一扇 → 窗口关掉、进程留在托盘（图标常显），点托盘「显示窗口」
重新开出一扇。
无头面：`runtime::tests::close_decision_keeps_the_process_alive_while_other_windows_open`
钉住三条去路（锁定取消 / 还有别的窗口只关这一扇 / 最后一扇按托盘开关决定关还是退）。

### 门禁
升级 windui 时：若上游给 `EventCtx` 加了应用级退出（或 `App::request_quit` 一类公开入口），
本章整段撤掉、宿主改走上游 API；否则按第 1–3 步重放。
`hygiene_scan` 第 7 查：本章节 ↔ `.3rd` 的 `★ RuPix 补丁 025` 标记对得上；
`docs_consistency::j13`：`patches/wind-ui-rust-0.23.0/src/core.rs`、
`.../src/platform/mod.rs`、`.../src/platform/win32/mod.rs` 与 `.3rd` 工作副本逐字节相同。

---

## 迁移记录 —— 0.20.0 → 0.23.0（2026-10-13，用户裁决"全部迁移为 0.23.0"）

windui 0.23.0 是破坏性重构（托盘子系统重写、跨平台 win32/macos/linux），不是平替。
迁移完成后 **`.3rd/wind-ui-rust-0.20.0` 与 `main/patches/wind-ui-rust-0.20.0/` 已删除**，
`Cargo.toml` path 依赖改指 `.3rd/wind-ui-rust-0.23.0`。各补丁去向：

| 补丁 | 去向 | 说明 |
| --- | --- | --- |
| 01 托盘运行期显隐 | **重放** | 声明层 `platform/tray.rs`（`TrayOp::SetVisible` + `TrayHandle::set_visible` + `Tray::start_hidden`/`hidden_at_start`）；win32 `tray.rs`（`TrayState::visible` + `install` 跳过 `NIM_ADD` + `remove`/`is_visible`/`remember_visible`）；win32 `mod.rs`（`apply_tray_ops` 加 `SetVisible` 臂：同值跳过、显示走 `readd`、隐藏走 `remove`、成功才记账；`readd_tray` 起手过滤 `!is_visible`，被藏图标不许被 shell 重启复活）。**macOS 侧未重放**（本仓库只跑 Windows；Linux 后端无托盘，`TrayOp` match 的非穷尽由 `#[allow]` 收口）。 |
| 02 失焦回调 `on_blur` | **上游已收编** | 0.23.0 内建 `Element::on_blur`，但签名从 `Fn()` 升为 `FnMut(&mut EventCtx)`——应用侧 `ui/settings.rs::hotkey_box` 的 `on_blur` 参数与调用闭包已随之改签名。 |
| 03 逐像素窗口透明度 | **重放** | `platform/mod.rs`（`OpaqueRegion` + `AppHandler::opaque_overlays` 默认空实现）→ `core.rs`（`Tree::menu_open` + `EventCtx::menu_open()`）→ `app/menu.rs`（`MENU_RADIUS` 单一定义点 + 开/关维护）→ `app/mod.rs`（`UiHost::opaque_overlays`）→ win32 `mod.rs`（`SkiaBackend` 分层呈现整段：分层字段/`Drop`/`ensure_layer_dib`/`paint_layered`/`set_window_alpha`/`copy_premul_rows`/`inside_round_rect`；`BlendOp` 改用 `AC_SRC_OVER as u8` 常量）。**此补丁无编译症状，漏了只会在运行期丢透明度行为**。 |
| 04 WindowOp 扩展 | **重放** | `event.rs`（5 变体 + `WindowState::{fullscreen, topmost}` 字段；全仓 8 处 `WindowState` 字面量同步：event.rs×2、app/mod.rs×4、wayland/mod.rs×1、win32 `push_window_state`×1）→ `core.rs`（5 个 `EventCtx` 助手）→ win32 `mod.rs`（`run_window_op` 5 臂 + 5 个执行体 + `saved_placement` 第三字段）。macOS/wayland 的 match 穷尽性在 Windows 上被 cfg 掩盖，迁移时已顺手核对。 |
| 05 NCHITTEST 顶缘优先 | **重放** | 0.23.0 上游把 `interactive_at` 放在**全向最前**——正是本补丁文档里点名的"全向优先会让铺满客户区的可聚焦画布四缘全失"反模式。已按 0.20.0 语义改回：edge 计算后**仅顶缘**让位交互控件，其余七向缩放边框无条件优先。 |
| 06 SVG 渲染性能 | **重放** | `render/image.rs`（`svg_fontdb`/`svg_warm_fonts`/`svg_tree_cached`/`Image::svg_intrinsic_size`/`Image::from_svg_region` + `rasterize` 走缓存）。API 变化：`svg_intrinsic_size` 上游化为 `Image` 关联函数——应用侧 `ui/svg.rs` 调用点从模块函数改为 `Image::svg_intrinsic_size`。 |
| 07 滚动条内缩按 frameless | **上游已收编** | 0.23.0 重构为 `Tree::scroll_edge_inset: i32`（0=不内缩），`UiHost` 按 `frameless && resizable` 置 0，语义与 0.20.0 补丁一致，无需重放。 |

应用侧适配（非补丁本体）：`ui/svg.rs` 调用点、`ui/settings.rs::hotkey_box` blur 签名。
托盘用法（`Tray::start_hidden` + `TrayHandle::set_visible` + `ctx.toggle_window`）与 0.23.0
新托盘 API（`TrayCtx`/`TrayAction::Toggle`/`hide_on_close`）完全兼容，`runtime/mod.rs`
的 16ms 轮询接线不变。

### 门禁行（迁移完成后）
`cargo check --all-targets` 0 error 0 warning · `cargo clippy --all-targets` 0 warning ·
`cargo fmt` 无差异 · layering_scan `violations: 0` · hygiene_scan `violations: 0` ·
main `cargo test --lib` **119 passed** · `cargo test --test docs_consistency` **7 passed** ·
windui `cargo test --lib` **1075 passed**。
