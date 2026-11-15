# 实施计划：全屏退役（改最大化）+ 插件输出卡跨图自动刷新

日期：2026-11-15 · 规格：[`../specs/2026-11-15-fullscreen-retire-plugin-refresh-design.md`](../specs/2026-11-15-fullscreen-retire-plugin-refresh-design.md)

## Global Constraints（每个任务开工前先读）

1. **读数一律经 bat**：门禁与构建只跑 `main\build\N_*.bat`（2_scan / 4_build / 5_test / 6_deploy）；直接敲 `cargo` 会丢 `RUSTFLAGS` 里的 `--remap-path-prefix`，链出来的不是出货那枚。执行中随时可用 `python build/structure_gen.py --write`（在 `main/` 下）同步《目录结构说明.md》。
2. **临时文件一律进 `.temp/`**，禁止写进任何源码仓；不写任何本机绝对路径。
3. **账本行是整行长段落**：`docs/产品定位与边界.md` 体积账在行内追加、保持单行；Edit 对该文件报 `non_atomic_windows_write` 警告属正常。
4. **HISTORY 标题格式**：`## <版本> — <日期> — protocol N`，延续既有条目样式与编号。
5. **windui 补丁仓不动**：`ToggleFullscreen` / `WindowState::fullscreen` 是上游能力，宿主只是不再调用；`main/patches/` 本轮零改动。
6. **提交 / 推送前必须取得用户明确许可**；`.bf/` 快照不受此限。
7. 删测试会使 5_test 总数变化——按实际读数记账，不硬凑。
8. clippy 1.97.0 可能对既有代码冒新 lint：顺手修平，不扩大战线。

## 任务 1：配置枚举（`main/src/core/config.rs`）

**InitWindow（368-431）**：

- 变体 `Fullscreen` → 改名 `Maximized`；doc 注释改「最大化：OS 工作区最大化（标题栏最大化钮的既有语义，留任务栏）」。
- `from_index(1)` 注释「1(旧最大化)→全屏」改「1→最大化」；**存储值 1 不变**（旧 ini 无缝读入）。
- `index()` / `ui_index()` / `ALL` / `from_ui_index`：序不变，仅变体改名。
- `label()`：`"全屏"` → `"最大化"`。

**DblClick（220-267）**：

- 删变体 `Fullscreen`（ini 值 3 退役）；`ALL` 收成 3 项。
- `from_index`：`3 => DblClick::Maximize`（旧值兼容读入）；其余不变。
- `index()`：无 Fullscreen 臂（写回恒 0/1/2）。
- `label()`：删「切换全屏」臂；`Maximize` 保持「切换最大化」。

**ini 模板注释（1583 / 1627 附近）**：`init_window` 行「1 全屏（图片适应窗口）」→「1 最大化（图片适应窗口）」；`dbl_click` 行删「3 切换全屏」档。

**测试（2850-2897）**：档位往返断言改写——`InitWindow::from_index(1)==Maximized`、`Maximized.index()==1`；`DblClick::from_index(3)==Maximize`、`from_index(0/1/2)` 不变、`DblClick::ALL.len()==3`。

**验证**：`cargo check` 级编译不过没关系（下游引用还没改），本任务只保证 config.rs 内部自洽；全量编译留给任务 4 之后。

## 任务 2：热键（`main/src/core/hotkey.rs`）

- 删 `Action::Fullscreen`：变体（98）、`index()` 数组项（167）、`"fullscreen"` id（218）、显示名（303 附近）、解析臂。★ 删变体后数组长度与测试里用 `Action::X.index()` 的写法自动适配，但**任何硬编码下标断言都要过一遍**。
- ini 兼容映射：`from_ini`（或等价的解析函数）把旧键 `fullscreen` **读成 `Action::Maximize`**，写回（`to_ini` / 保存侧）不产 `fullscreen` 键。落点即 id 解析处——grep `"fullscreen"` 定位。
- 出厂绑定（685）：`b[Action::Fullscreen.index()] = vec![dhk(Base::F(11), false)]` → 改绑 `Action::Maximize`（同键）。686-688 的注释「全屏里按 Esc 先退全屏」同步改写（Esc 兜底已删，见任务 4）。
- `Action::Maximize` 显示名确认/改为「最大化切换」。
- 测试（925-928 F11、1088-1096 partial_ini）：F11 断言改 `Some(Action::Maximize)`；partial ini 断言改 `from_ini("prev:Right;maximize:F5")`（旧 `fullscreen:F5` 需断言**映射**：`from_ini("fullscreen:F5")` → `Maximize` 绑 F5——两条都要）。

## 任务 3：按钮栏与顶条（`main/src/ui/chrome.rs`）

- `Btn::Fullscreen` 三连删：变体（181）、glyph（256）、`→ Action::Fullscreen` 映射（295）、`ALL` 项（323）。按钮栏与「画面 ▸」菜单同源自动少一项。
- `title_visible`（1668-1674）：删 `fullscreen_on` 参数，函数体收成 `!minimal_mode || hover_title`；doc 注释改写（m238 全屏收条语义退役，保留极简悬停语义）。
- `title_strip_h`（1683-1691）：删 `fullscreen_on` 参数（唯一调用方 canvas.rs，见任务 4）。
- `paint`（1716 起）：签名删 `fullscreen_on` 参数；1739 `title_strip_h(fullscreen_on, …)` 调用同步。m247 开关高亮（2020-2026）：`let color = if b == Btn::Fullscreen && fullscreen_on { … } else { … }` → 恒 `tk::overlay_text()`，注释「m247 后只剩全屏」整句删。
- 1840 注释「同 Checker/Fullscreen/置顶 激活模式」→「同 Checker/置顶 激活模式」。
- 测试（2161-2184）：`title_visible_fullscreen_hides_but_hover_recalls` 整支删，换成两参版回归：`title_visible(false, false)==true`、`title_visible(true, false)==false`、`title_visible(true, true)==true`、`title_visible(false, true)==true`（极简悬停语义一字不变）。

## 任务 4：画布与菜单（`main/src/ui/canvas.rs`）

全文件 grep `fullscreen|Fullscreen` 逐处清，落点：

- 模块 doc（36）与函数注释（440、466）：m238 全屏拖动锁定语义退役，注释改写为「顶条/空窗不响应拖窗，与窗口形态无关」。
- `left_drag`（458-475）：删 `fullscreen` 参数与 `if fullscreen` 分支（470 附近）；非全屏三判据（`drag_image` / `can_pan` / `!in_title`）不动。
- 调用点两处（4033-4040 `press_left`、4093-4100 hover 光标）：删 `window_state().fullscreen` 实参。
- 菜单勾选列（2624-2633）：`let state = if b == Btn::Fullscreen { Some(ws.fullscreen) } …` 整段删，`MenuItem::run` 的 checkable 参数保持 `false`；注释「m247 后只剩全屏这一颗带状态」删。
- `exec_btn`（2845-2851）：删 `if b == Btn::Fullscreen { ctx.toggle_fullscreen(); return; }` 分支及重入注释（Btn::Fullscreen 已不存在，该特判无意义）。
- `chrome::paint` 调用（3644-3656）：删 `ws.fullscreen` 实参（3651）。
- `chrome::title_strip_h` 调用（3202-3206）：删 `window_state().fullscreen` 实参。
- Esc 兜底（3871-3880）：`if k.key == Key::Escape { if window_state().fullscreen { … } return false; }` → 直接 `return false`（可整段收成一句 + 注释「框架默认行为 = 关窗」）。
- 双击（4762-4770）：`DblClick::Fullscreen => ctx.toggle_fullscreen()` 臂删；注释（4756「关窗/最大化/全屏」、4761「关闭 / 最大化 / 全屏」）同步。
- `exec_action`（5280-5283）：`Action::Fullscreen` 臂整支删；`Action::Close` 臂（5298-5307）：删「全屏里先退全屏」分支 → 恒 `c.request_close()`，注释改写；4903-4904、5087 注释里「全屏」措辞更新。
- m238 测试（6432-6546 两支）：`left_drag_never_moves_the_window_in_fullscreen` 与 6506 起那支整支删；非全屏回归断言若混在被删测试体内，**保留其非全屏部分**（搬进现存相邻测试或独立成小测试）。
- 菜单 doc 注释（1312 / 1351 / 1365）：「全屏 /」字样删。

**验证**：本任务完成后 `2_scan` 应能编译（config/hotkey/chrome/canvas 四件套齐改）；跑 `build\2_scan.bat` 确认零 error 再进任务 5。

## 任务 5：窗口逻辑（`main/src/runtime/mod.rs`）

- `StartOp`（82-85）：`Fullscreen` → 改名 `Maximize`，注释「启动置顶 / 全屏」→「启动置顶 / 最大化」。
- `startup_ops`（586-588）：`c.init_window == InitWindow::Fullscreen` → `InitWindow::Maximized`，push `StartOp::Maximize`。
- tick 发放（1076-1079）：`StartOp::Maximize => ctx.maximize()`（windui `EventCtx::maximize` 已有，event.rs 315 现成）。
- `apply_pending_window`（148-166）：`InitWindow::Fullscreen` 臂（149-155）改为 `InitWindow::Maximized` 臂——行为=最大化（按该函数现有 WindowOp 发放路径执行；executor 通读函数后选 `ctx.maximize()` 或等价 WindowOp，保持与 Fixed/Natural 臂同构）。
- `needs_restored_size`（101-109）：删 `fullscreen` 参数与 `!fullscreen` 判据，注释「全屏档自己管还原」→「最大化档由系统还原」；调用点 grep 同步（约 1095 一带传 `ws.fullscreen` 的实参删）。
- 「永远居中」跳变侦测（1288-1310）：`!ws.maximized && !ws.fullscreen`（1297）→ `!ws.maximized`；附近 `ws.fullscreen` 判据（若还有）一并化简。
- `natural_size`（1327-1332）：`|| ws.fullscreen` 删。
- 1095-1142、1200 附近其余 `ws.fullscreen` 引用：grep 逐处按「只剩 maximized」化简。
- 测试（1831-1834）：`needs_restored_size` 断言改两参版；`InitWindow::Fullscreen` 字面量改 `Maximized`。

## 任务 6：空格预览（`main/src/platform/spacehook.rs`）

- `preview_box`（97-129）：`InitWindow::Fullscreen` 臂（108）`（mon_w, mon_h）` → `InitWindow::Maximized` 臂 `（work_w, work_h）`；居中/定位分支（123-127）`matches!(init, Fullscreen) => (mon_x, mon_y)` 删——三档全部居中于工作区。
- `preview_fit`（140-145）不变（`Maximized` 落 `_` → `Adapt`）。
- doc 注释（92 附近「`Natural`：框只当夹取上限」同段里全屏档说明）改写。
- 测试（1421-1476）：「全屏档必须按 rcMonitor」断言改为 `preview_box(InitWindow::Maximized, …) == (0, 0, 1920, 1040)`（工作区）；`preview_fit(InitWindow::Maximized) == Adapt`。

## 任务 7：core 注释（`main/src/core/viewer/tests.rs`）

仅措辞：765「无边框 / 全屏（不画标题栏）」、804「切无边框 / 退出全屏」、830「无边框 / 全屏那两种形态」→ 全部改「无边框形态」等价说法。**几何与断言零改动**（全屏收条语义退役后 chrome_top 由 `set_chrome_top` 驱动，测试本来就是纯几何钉子）。

## 任务 8：取消记账（`main/src/extensions/run.rs`）

- `JobOutcome`（139-145）加 `pub cancelled: bool`（doc：本次结果是否来自被取消的调用；runtime 对它静默）。
- `finish`（506-541）：构造入队 `JobOutcome` 处（函数尾 push 进 `JOBS`）记 `cancelled: cancelled(job)`；`record_outcome` 的第三个实参维持 `cancelled(job)` 不动（三振记账口径不变）。
- 测试：沿用现有 cancel 测试的手法（1216 附近手工构造 `JobHandle`）补一条——置 `cancelled` 位后走 `finish`，断言 `poll()` 出的 `JobOutcome.cancelled == true`；非取消路径断言 `== false`。

## 任务 9：面板刷新集（`main/src/ui/info_panel/mod.rs`）

- `sync_plugin_scope`（359-373）改签名：`pub fn sync_plugin_scope(&self, path: Option<&Path>) -> Vec<String>`——路径没变返回 `Vec::new()`；变了则**不清空**，返回当前 `plugin_cards` 全部 key（即需要重跑的插件 run_key 集）。doc 注释改写（换图不清卡，交给 runtime 重跑）。
- `clear_plugin_cards`（352-357）：唯一调用方是旧 sync，整支删。
- 测试（1459 起 `plugin_card_adds_height_and_clears_on_image_scope_change`）：改写为「跨图保持」——push 一张卡 → `sync_plugin_scope(同路径)` 返回空、卡还在 → `sync_plugin_scope(新路径)` 卡还在、返回该 key；内容仍由 push 替换路径覆盖。

## 任务 10：runtime 自动重跑（`main/src/runtime/mod.rs` tick poll 段）

落点 763-768 与 769-918：

1. **换图重跑**：`sync_plugin_scope` 返回值接住：
   ```rust
   let refresh_keys = self.chrome.info.sync_plugin_scope(current_plugin_path.as_deref());
   if !refresh_keys.is_empty() {
       let pixels = self.viewer.borrow().whole_image_for_plugin(); // 借用出循环前算好
       let path = current_plugin_path.clone();
       let list = crate::extensions::discover_configured(&config.borrow());
       for key in refresh_keys {
           let Some(ext) = list.iter().find(|e| e.run_key == key) else {
               log::warn!("[plugin-panel] id={key} verdict=dropped（插件已不在，卡移除）");
               self.chrome.info.remove_plugin_card(&key);
               continue;
           };
           if ext.manifest.kind != crate::core::ext::Kind::Command
               || !ext.manifest.has_capability(crate::core::ext::Capability::ImageRead)
               || path.is_none()
           {
               self.chrome.info.remove_plugin_card(&key);
               continue;
           }
           let _ = crate::extensions::run::cancel(&key); // 在跑就静默停（JobOutcome.cancelled 静默）
           let path = path.clone();
           if let Err(e) = crate::extensions::run::start_command(
               ext.clone(), path, None, pixels.clone(),
           ) {
               log::warn!("[plugin-panel] id={key} verdict=restart-failed：{e}");
               self.chrome.info.remove_plugin_card(&key);
           }
       }
   }
   ```
   ★ 与右键菜单同一条路：能力分流 / 8MP 闸 / 抽样判据全在 `start_command` 内部，UI 不另抄（`pixels_to_send` 是唯一正本）。★ `pixels` 的 `ForPlugin` 是 `Clone`（rgba 是 `Arc`），克隆廉价。★ 注意 769 行 poll 循环在 sync 之后照旧跑，取消的旧结果会被 `cancelled` 位静默吞掉。
2. **poll `Err` 臂（917）**：
   ```rust
   Err(e) => {
       if job.cancelled {
           log::info!("[plugin-cancel] id={} verdict=silent", job.id);
       } else {
           // 失败（非取消）：宁可空不可错——旧图的统计数据留着会误导。
           self.chrome.info.remove_plugin_card(&job.id);
           ctx.toast_err(format!("{}：{}", job.plugin, e));
       }
   }
   ```
   `Ok` 各臂不动（stale toast 782-787 保留；`push_plugin_card` 同 key 原位替换机制已有）。
3. 764 行注释「换图时清掉上一张图的插件输出」改写为新语义。

**验证**：run.rs 与 info_panel 单测覆盖记账与刷新集；端到端（换图→重跑）靠真机 levels 判据（任务 12）。

## 任务 11：文档

- `main/docs/待办.md`：`### 17` 之后新增 `### 18. 信息面板插件卡拖动排序（搁置）`，要点照规格 §4（卡头拖把手复用补丁 030 `hit_pad`、半透明跟随 + 插入位指示线、`plugin_cards` 信号数组重排落点、与折叠态/卡右键/滚动容器交互约束、windui 拖拽原语评估）。文首行 16 的「16 条」顺手订正为实际条数。并在「一」节引言（21 行附近第 6 条句）补一句「卡片拖动排序见 18」。
- `main/src/ui/settings/options.rs`：116 / 136 / 153 注释里「全屏」措辞改「最大化」（设置页下拉由 `InitWindow::ALL`/`label()` 驱动，代码零改动）。
- `main/HISTORY.md`：新条目（标题格式见 Global Constraints #4），记全屏退役（档位/双击/F11/按钮/菜单/Esc/预览）与插件卡跨图自动刷新，含旧 ini 兼容口径。
- 《目录结构说明.md》由 `structure_gen --write` 代出；《产品定位与边界.md》账本在 4_build 读数后追加（任务 12）。

## 任务 12：门禁、体积账与真机判据

顺序执行（每步确认再下一步）：

1. `main/` 下 `python build/structure_gen.py --write`（结构变更入册）。
2. `build\2_scan.bat`：静态门禁全绿（fmt + clippy；新冒 lint 顺手修平）。
3. `build\4_build.bat`：release 产物 + 体积读数 → `docs/产品定位与边界.md` 账本行内追加本轮 ±（保持单行）。
4. `build\5_test.bat`：全绿；总数变化如实记账。
5. `build\6_deploy.bat`：`.build/` 同步。
6. 真机判据（`.temp\smoke_step.ps1`）：
   - 启动档「最大化」：窗最大化落工作区、留任务栏；空格预览铺工作区；
   - 双击图片 = 切换最大化；F11 = 切换最大化；
   - 按钮栏与右键「画面 ▸」无全屏项；Esc 直接关窗；
   - 旧 ini 兼容：把 ini 手写 `init_window=1`、`dbl_click=3`、hotkeys 含 `fullscreen:…`，重启读入不炸、语义落到最大化；
   - 开「色阶分析」→ 翻下一张：卡自动换新图数据，无「结果未显示」toast；手动 ✕ 关卡后再翻图：无插件进程被拉起（任务管理器或日志 `[plugin-run]` 判）；
   - 面板既有行为回归：属性卡 / 折叠 / 卡右键 / 富文本复制照旧。
7. 全绿后 `.bf/` 快照（`MMdd-HHmm 名称`），**请求用户提交许可**。

## 自查

- 规格覆盖：A 节 → 任务 1-7；B 节 → 任务 8-10；C 节 → 任务 11；判据 §5 → 任务 12。✅
- 类型一致：`JobOutcome.cancelled` 在 finish 记、poll 读；`sync_plugin_scope` 返回 `Vec<String>` 与 runtime 消费一致；`InitWindow::Maximized` / `StartOp::Maximize` 命名对齐。✅
- 无占位符、无本机绝对路径、行号锚点均来自当前工作区实读。✅
