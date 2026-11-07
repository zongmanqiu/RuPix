# 文档登记簿（DOCS_MANIFEST.md）

> 三列登记簿：**文件 | 何时更新 | 谁验证**。写文档前先读它；新增文档必须来此登记。
> 验证方式写命令 / 测试名；「自检」= 改完重读一遍该文件相应章节。
> 本清单本身也在本表内登记（自指）。行数上限以实测为准，不在此处硬编码（避免登记簿秒腐）。

## 一、`main/` 根目录

| 文件 | 何时更新 | 谁验证 |
|---|---|---|
| `README.md` | 功能清单 / 快速开始命令表 / 技术栈版本 / 架构简图 / **文档地图** / 命令行参数表 / License 行 / 致谢变化时 | 自检 + `python build/layering_scan.py`（引用了分层结论时） |
| `AGENTS.md` | 规则本身变化时：§一 五层架构与禁引、§四 布局与术语铁律（含保留项）、§五 解码器 ABI 契约、§六 执行环境坑、§七 源码陷阱、§八 编码纪律、§九 交付清单——增删任一条即更 | `python build/gate.py run config` 等门禁全绿 + 自检 |
| `目录结构说明.md` | 新建 / 删除 / 移动 `src/` 文件或子目录；§二根目录约定变化。**第一、三、四节的数字与第五节符号索引不许手改**——跑 `python build/structure_gen.py --write`；第三节「说明」列与 §二是人写的职责描述，生成器只动数值格 | `python build/structure_gen.py --check`（j10）+ j9 Rust 独立走盘对账 + j11 生成器自检，全在 `cargo test --release -- --test-threads=1`（docs 组）里 |
| `开发指南.md` | §一六步流程、§二改动→文档对照表、§三写文档决策树、§四源码提交与发布资产分工、**§五发布前同步点清单（E1–E12 + 三方对齐）**任一处变化；「明确不做」清单变化。★ §五 的判据是"**这两件事之间没有机器读过**"才许进：做成门禁后该行要删或改成"已下沉"，本节应越用越短 | 自检；与本登记簿交叉核对；每条命令必须能在 `main/` 下照抄跑出数（本轮 12 条逐条跑过） |
| `DOCS_MANIFEST.md`（本文件） | 任何文档新建 / 改名 / 删除 / 废弃时（包括本表任何一行的触发条件变化） | 自检（每行指向的文件必须真实存在，【待建】除外） |
| `HISTORY.md` | **顶部追加**：用户可感知的功能 / 行为 / 兼容 / 性能 / 门禁里程碑；每条含日期（同一天可多节，见卷首第 2、3 条）。卷首约束 §一～§七 | 自检（不改写旧条目；版本号与 `Cargo.toml` 实读一致）。★ 「每条含『取材』行」这句是登记簿自己的旧话——全文只有 3 处出现"取材"，绝大多数条目没有这一行；要么今后补，要么别声称有 |
| `THIRD-PARTY-NOTICES.md` | 新增 / 移除 / 更换**宿主直接依赖**、windui 补丁落地、分发方式变化；[待核] 项逐个补核为 [实测]。**解码器依赖不在本文件**（`解码器仓库许可清单`） | `cargo metadata` / 宿主 `Cargo.lock` 实读比对 + 自检 |
| `LICENSE` | 不更新（AGPL-3.0 官方全文；改许可证须用户拍板并同步 HISTORY + THIRD-PARTY-NOTICES） | — |

## 二、`main/docs/`

| 文件 | 何时更新 | 谁验证 |
|---|---|---|
| `docs/踩坑记录/踩坑记录.md`（**第 1 册**，含总索引） | **复现新坑时按编号续写**——★ 2026-10-26 起**分册**：`踩坑记录.md` / `-2` / `-3` / `-4` / `-5` / `-6` / `-7`。新坑一律追加到**最后一册**末尾；K 号 → 册的对照表在第 1 册顶部，别处写「见 K68」照表跳册即可。旧坑根治后标「已根治」不删条目。★ 卷首写的是**目标**上限（≤700 行），**没有任何门禁量它**：册 1/5/6/7 实测 741 / 808 / 814 / 44 行——要当真判据用就得给 j5 加一条按册量，别在登记簿里写做不到的话。★ **2026-11-07 起 7 册统一放 `docs/踩坑记录/`**，册名与 K 号都不变；`vendor/rupix-abi` 是逐字节快照（`开发指南.md` §五 E11 明写不许改），其中 K26 那句仍写 `docs/踩坑记录.md`，按目录名理解，别为它去改快照 | 自检（每条含现象 / 原因 / 解法，可照抄执行）；行数由 `wc -l` 现数，别抄文档里的数 |
| `docs/踩坑记录/踩坑记录-2.md` … `-7.md` | 同上（各册头部写明"新坑追加到最后一册"）。**只有第 1 册收总索引** | 同上；`docs_consistency` 的 j3/j4 扫全部 md，册名不影响判据 |
| `docs/Rust 软件工程设计总纲.md` | **工程方法论宪章**（此前未登记，2026-10-19 补上）。只在"新的编译期约束被采纳 / 某条裁决被推翻"时更新顶部那张《采纳状态》表；正文方法论按需查阅，不追改。**只保留本仓采纳后的方法论**，不复制外部全文 | 自检 + `cargo test --release --test docs_consistency`（j3/j4/j5：点名的 `.rs` 存在、现行文档无考古串、行数上限） |
| `docs/界面版式规范.md` | **版式正本**（2026-11-06 建）：对话框四边留白 / 行间距 / 按钮位 / 按钮文案与顺序变化时；新开一扇自绘对话框、新加一颗底栏按钮的规则也写在这里。**改版式先改它、再改代码**，两边不一致当场二选一 | `ui/dialog_footer.rs` 三条几何判据（340/480/640 三种窗宽量右缘 = 窗宽−16、下缘 = 窗高−16、间距 = 8）+ `python build/hygiene_scan.py`（第十四查 `footer_buttons`）+ 自检 |
| `docs/扩展体系.md` | 扩展 manifest / 能力 / 目录约定 / 分层 / 安全底线变化时；未落地项随 P0.1b 更新 | `cargo test --release --lib core::ext -- --test-threads=1` + `python build/layering_scan.py` + 自检 |
| `docs/架构总览.md` | 五层分层、§三关键类型（`DecodeBackend` / `Fail` / `ImageCache` 等）、§五解码路由与清单规则（**只写动态并集，不登记具体 DLL**）、§七性能红线、§八改动影响面任一变化；「明确不做」清单（§九）变化 | `python build/layering_scan.py` + `hygiene_scan.py` 均 `violations: 0` |
| `docs/解码器开发规范.md`（**宿主侧 ABI 对接说明**，自包含） | 宿主看到的 ABI 版本、导出、结构体、错误码、缓冲所有权、能力路由变化时更新；契约结构体以 `vendor/rupix-abi` 为准 | 宿主 `cargo test --release --lib core::abi -- --test-threads=1` 全绿 + `cargo test --release --test docs_consistency -- --test-threads=1` |
| `docs/图片格式与后缀名.md` | 宿主内置路径（只有 resvg 一枚，管 svg/svgz）与「动态并集」规则变化；**具体解码器清单不写在这里**（清单真源在解码器侧，宿主随包只带 `docs/decoders.json` 这份数据） | `ext` 组测试（`python build/gate.py run ext`）+ 真实文件冒烟 |
| `docs/待办.md` | 候选项增删、状态变更（✅ / 🚫 / 调研结论） | 自检（与 HISTORY 不重复：待办记未来，HISTORY 记已发生） |
| `docs/产品定位与边界.md` | 战略定位 / 职责边界 / "轻量"硬定义 / 明确不做清单任一变化；本文件是战略层，内容长期稳定，改动应稀少 | 自检 + `python build/layering_scan.py`（引用分层结论时） + `cargo test --release --test docs_consistency` 全绿 |
| `docs/待开发内容.md` | 编码相关三条（格式转换 / 涂鸦 / 裁剪）的**共同依赖、保存口径与重开条件**变化：新格式编码器、ABI 增编码面、`image.write` 同尺寸契约、`待办.md` §24 一律另存 PNG、§26④ 硬边界任一松动或出新裁决 | 自检（判定值只在 `docs/待办.md` / `docs/产品定位与边界.md` 写一份，本文件不复述）；`cargo test --release --test docs_consistency -- --test-threads=1` 全绿 |
| `docs/superpowers/plans/2026-10-12-titlebar-topmost-lock.md` | 已执行完毕的计划存档，不更新（同类新工作另开新文件） | 自检（与同名 spec 成对存在） |
| `docs/superpowers/specs/2026-10-12-titlebar-topmost-lock-design.md` | 设计变更时（被 `src/ui/chrome.rs` 注释与 `HISTORY.md` 引用） | 自检（引用路径成立） |

## 三、补充登记

| 文件 | 何时更新 | 谁验证 |
|---|---|---|
| `build/build_readme.md` | 构建 / 测试脚本名、流水线步骤、环境变量契约变化 | `build/full.bat` 全绿 |
| `patches/PATCHES.md`（windui 卷，编号只增） | 每落一枚 windui 补丁加一章；撤除时在原章写「豁免」并给原因 | `python build/hygiene_scan.py` 第 7 查 `patch_parity`（章节 ↔ `.3rd` 标记双向对；读不到任一侧判「本查失明」）+ j13 归档逐字节 |
| `patches/usvg-0.48.1/PATCHES.md`（usvg 卷，U 号只增） | 每落一枚 usvg 补丁加一章；**判废的号不复用**（现行 U02，U01 是判废记录） | j13 归档逐字节（`deploy.py` / `docs_consistency` 的 j13 把 usvg 一起比） |
| `tests/locked_decoder_dll.rs` | 真机「加载中的解码器 DLL 删不掉」这条物证判据变化（独占句柄、改名可行 / 删除 code 5） | `cargo test --release --test locked_decoder_dll -- --test-threads=1`（需要 `.build/decoders` 可寻址） |
| `patches/README.md` | windui 补丁的**机制与生命周期**（登记 + 修改后完整文件归档）变化。★ 标题里**不写份数**：份数是第二正本，上一版写「19 份」时磁盘已到 025 | `python build/hygiene_scan.py` 第 7 查 `patch_parity`（章节 ↔ `.3rd` 标记双向对，机器读的是 PATCHES.md 与本目录归档，不读这句散文） |
| `tests/README.md` | 测试布局（门禁组 vs 探针）变化 | `cargo test --release -- --test-threads=1` 全绿 |
| `tests/probes/README.md` | **探针写法与登记方式**变化（本目录当前零枚探针：探针都要按 `[[test]]` 显式登记才参与构建，没登记就不编译） | 登记过的探针用 `cargo test --release --test <名> -- --test-threads=1` 跑；★ 判据不是 `--example`——`tests/` 下的东西是 `[[test]]`，`examples/` 下的才是 example |
| `tests/docs_consistency.rs` | 新增/删除根文档、登记簿条目变化、考古串回流、src 文件增减、结构文档数字或符号索引漂移（j9 独立走盘 / j10 新鲜度 / j11 生成器自检）、发布目录自洽判据有没有钉子（j14 拉起 `deploy.py --selftest`） | `cargo test --release --test docs_consistency -- --test-threads=1` 全绿 |
| `tests/config_compat.rs` + `tests/fixtures/default.ini` | 新增/修改/废弃 ini 键（fixture 与 DEFAULT_INI 同步改） | `cargo test --release --test config_compat -- --test-threads=1` 全绿 |
| `tests/config_pipeline_e2e.rs` | 新增/修改 config→解码接线（decoders_disabled / decoders_preferred / safe-mode / 解码器目录解析） | `cargo test --release --test config_pipeline_e2e -- --test-threads=1` 全绿（需 `.build/decoders` 已发布） |

## 四、明确不入库（不在本登记簿）

`.temp/`（编译中间产物、临时文件、日志）、`.build/`（发布产物，可分发但不是文档）、
工具状态目录、`*.dll` / `*.exp` / `*.lib` / `*.pdb` / `*.obj`、
草稿与一次性实测转储（落 `.temp/`）。
