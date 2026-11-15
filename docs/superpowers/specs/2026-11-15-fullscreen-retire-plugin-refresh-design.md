# 设计：全屏退役（改最大化）+ 插件输出卡跨图自动刷新

日期：2026-11-15 · 状态：设计已确认，待实施

## 1. 需求与边界

用户 2026-11-15 三条口径：

1. **取消全屏**：真全屏（铺满显示器、含任务栏那条）整条退役；「启动窗口」档位里的
   全屏改为**最大化**（最大化 = OS 工作区最大化，标题栏最大化钮的既有语义）。
   按钮栏的全屏按钮删掉；右键菜单里的全屏项删掉。
2. **插件输出卡跨图保持**：对着一张图开启插件（如色阶分析）后切图，卡不该消失，
   而应按当前图自动刷新内容。
3. **卡片按头部拖动排序**：本轮搁置，记入 `docs/待办.md`（用户明示允许）。

已拍板的四个口径（2026-11-15 用户确认）：

- 双击图片档位 3（旧「切换全屏」）与出厂 F11 热键 → 都改成**切换最大化**；
- 空格预览「全屏档」预览窗 → 改铺**工作区**（不再铺 `rcMonitor` 整屏）；
- 插件卡跨图 → **自动重跑**：切图后对面板里每张还开着的卡自动重跑该插件、
  原位替换内容；手动关掉卡 = 停止自动刷新；
- 卡片拖动排序 → 搁置进待办。

明确不做：windui 框架的 `WindowOp::ToggleFullscreen` / `WindowState::fullscreen`
**保留不动**（上游能力，宿主不再调用即可）；空格预览本身（它是独立预览窗，
只跟着启动档换尺寸口径）；卡片拖拽排序（本轮）。

## 2. 全屏退役 —— 改动面清单

### 2.1 配置（`core/config.rs`）

- `InitWindow::Fullscreen` → 改名 `Maximized`；ini 存储值**不变**（1）；
  `from_index(1)` / `from_ui_index` 档位序不变（固定 0 / 最大化 1 / 图片尺寸 3）；
  UI 名「全屏」→「最大化」；首启 ini 模板注释「1 全屏（图片适应窗口）」→
  「1 最大化（图片适应窗口）」。
- `DblClick::Fullscreen`（ini 值 3）删除变体；`from_index(3)` → `DblClick::Maximize`
  （旧值兼容读入，写回恒为 2）；下拉只剩 无 / 关窗 / 最大化 三项；
  模板注释「3 切换全屏」删。

### 2.2 热键（`core/hotkey.rs`）

- 删除 `Action::Fullscreen`（`index()`/`name`/`"fullscreen"` 解析/显示名/出厂绑定）。
- 出厂 F11 改绑 `Action::Maximize`（该动作已存在，`exec_action` 里是
  `toggle_maximize()`）；`Action::Maximize` 显示名「最大化切换」。
- 旧 ini `hotkeys` 里存的 `fullscreen` 键读入映射到 `maximize`（兼容），写回恒为
  `maximize`。

### 2.3 入口清理（`ui/chrome.rs` + `ui/canvas.rs`）

- `Btn::Fullscreen` 从 `Btn::ALL` 正本删除 → 按钮栏与「画面 ▸」菜单同源自动少一项；
  `Btn::glyph` / `Btn→Action` 映射同步删。
- m247 的「开关类给勾选列」机制只剩全屏一颗在用（`canvas.rs` 线前菜单循环里
  `b == Btn::Fullscreen` 的 checkable 分支）→ 整支删除；chrome 里 m247 的开关高亮
  注释/分支同步清。
- `exec_action` 的 `Action::Fullscreen` 臂删除；`Action::Close`（Esc）臂去掉
  「全屏里先退全屏」分支 → 恒 `request_close()`。

### 2.4 窗口逻辑（`runtime/mod.rs` + windui 调用面）

- `StartOp::Fullscreen` → `StartOp::Maximize`：启动档 1 发 `ctx.maximize()`
  （框架 `EventCtx::maximize` 已有）。
- `needs_restored_size`（固定档退出最大化后还原固定尺寸）判据里
  `!fullscreen` 项删除；「永远居中」侦测与尺寸账里 `ws.fullscreen` 全部按
  「只剩 maximized」化简（`window_state().fullscreen` 此后恒 false）。
- m238 全屏拖动锁定（`canvas.rs` 顶缘/拖动判据里的全屏分支 + 对应测试）整支删除。

### 2.5 空格预览（`platform/spacehook.rs`）

- `InitWindow::Maximized` 档预览窗尺寸：`rcMonitor` → **工作区**（`work_w/work_h`），
  与最大化语义一致；`PreviewFit::Adapt` 不变。

### 2.6 测试与文档

- 改写/删除：`canvas.rs`（全屏拖动锁定、Esc 全屏分支、菜单勾选列）、`chrome.rs`
  （顶条全屏收起 m238 测试、开关高亮）、`config.rs`（档位往返断言）、
  `spacehook.rs`（全屏档铺屏断言 → 工作区）、`hotkey.rs`、`viewer/tests.rs`
  （全屏不画标题栏的几何钉子 → 只留无边框形态）。
- 文档：`HISTORY.md` 新条目；`docs/架构总览.md`、`docs/产品定位与边界.md`（体积账
  追加本轮 ±）、`core/config.rs` ini 模板注释、`docs/待办.md`（C 节搁置项）。

## 3. 插件输出卡跨图自动刷新

### 3.1 现状（依据）

- 面板卡存在 `InfoPanel::plugin_cards: Signal<Vec<PluginCard>>`，卡 key = 插件
  run_key（`plugin_output::card_from_outputs` 里 `key: plugin_id`）。
- 换图时 `runtime` tick 调 `sync_plugin_scope(当前路径)`：路径变了 →
  `clear_plugin_cards()`（`info_panel/mod.rs` §sync_plugin_scope）。
- 插件调用入口：右键/热键都汇到 `extensions::start_command(ext, image_path,
  selection, image)`（能力分流 / 8MP 闸 / 抽样判据全在里面）；结果经
  `finish()` → `poll()` → runtime tick 的 poll 臂，`push_plugin_card` 同 key
  原位替换（机制已有）。
- 取消：`extensions::run::cancel(id)` 置停止位 + 杀进程组；被取消的结果走
  `Err("已取消")` 进 poll，runtime 会 toast——**自动重跑引发的取消不能打扰用户**。

### 3.2 改动

1. `extensions::run::JobOutcome` 加 `cancelled: bool`（`finish()` 里
   `cancelled(job)` 记录）；runtime poll 臂对 `cancelled == true` 的 outcome：
   不 toast、不撤卡（静默吞掉）。
2. `InfoPanel::sync_plugin_scope` 不再清空：路径变化时返回/记录「需要刷新的
   卡 key 集」（即当前面板里开着的全部插件卡 key）。
3. runtime tick 在 scope 变化时，对每个 key：
   - 用 `extensions::list()` 按 run_key 找回 `Extension`；找不到 / 不是
     `Kind::Command` / 未声明 `image.read` / 当前无图 → 该卡移除（无法自动刷新）；
   - `run::cancel(key)`（若在跑）→ `start_command(ext, Some(当前路径), None,
     whole_image_for_plugin())`——与右键菜单同一条路，零新口径。
4. 失败（非取消）结果到达：**移除该 key 的卡** + 既有 toast——宁可空不可错，
   旧图的统计数据留着会误导。
5. 边界：手动 ✕ 关卡 = 卡不在面板 = 停止自动刷新；编辑型产编辑覆盖不产卡，
   天然不受影响；同一路径的缩放/旋转不重跑（scope 只认路径，沿用现状）；
   快速翻图 = 每张图一次静默取消 + 一次起进程（levels 实测百毫秒级）。

### 3.3 测试

- `run.rs`：`JobOutcome.cancelled` 记账单测。
- `info_panel`：scope 变化不再清空、返回刷新集。
- `runtime` 级：换图 → 取消旧任务 + 新任务 target=新路径（用假扩展/探针插件，
  沿用 `examples/plugin_smoke` 的路子；真机补验以 levels 为准）。

## 4. 卡片拖动排序（搁置）

`docs/待办.md` 新增一条（顺延编号），要点：卡头拖把手（复用补丁 030 `hit_pad`
扩命中）、拖动半透明跟随 + 插入位指示线、`plugin_cards`/卡片序信号数组重排落点、
与折叠态 / 卡右键 / 面板滚动容器的交互约束、windui 需要补的原语评估
（拖拽重排无现成件）。本轮不实现。

## 5. 交付判据

- 静态门禁 2_scan 全绿；4_build 体积账同步（±字节记入账本）；5_test 全绿
  （宿主测试数会因删除全屏测试而变化，按实际读数记账）；6_deploy 同步。
- 真机判据（`.temp\smoke_step.ps1`）：
  - 启动档「最大化」：窗口最大化落工作区；空格预览铺工作区；
  - 双击图片 = 切换最大化；F11 = 切换最大化；按钮栏 / 右键菜单无全屏项；
  - Esc 直接关窗；
  - 开色阶分析 → → 翻下一张：卡内容自动换新图数据，无「结果未显示」toast；
    关卡后再翻图：无插件进程被拉起；
  - 面板卡片排序不回归（折叠 / 右键 / 富文本复制照旧）。
