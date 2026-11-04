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
| `开发指南.md` | §一六步流程、§二改动→文档对照表、§三写文档决策树、§四源码提交与发布资产分工任一处变化；「明确不做」清单变化 | 自检；与本登记簿交叉核对 |
| `DOCS_MANIFEST.md`（本文件） | 任何文档新建 / 改名 / 删除 / 废弃时（包括本表任何一行的触发条件变化） | 自检（每行指向的文件必须真实存在，【待建】除外） |
| `HISTORY.md` | **仅顶部追加**：用户可感知的功能 / 行为 / 兼容 / 性能 / 门禁里程碑；每条含日期与「取材」行；卷首约束 §一～§七 | 自检（不改写旧条目；版本号与 `Cargo.toml` 实读一致） |
| `THIRD-PARTY-NOTICES.md` | 新增 / 移除 / 更换**宿主直接依赖**、windui 补丁落地、分发方式变化；[待核] 项逐个补核为 [实测]。**解码器依赖不在本文件**（`解码器仓库许可清单`） | `cargo metadata` / 宿主 `Cargo.lock` 实读比对 + 自检 |
| `LICENSE` | 不更新（AGPL-3.0 官方全文；改许可证须用户拍板并同步 HISTORY + THIRD-PARTY-NOTICES） | — |

## 二、`main/docs/`

| 文件 | 何时更新 | 谁验证 |
|---|---|---|
| `docs/踩坑记录.md`（**第 1 册**，含总索引） | **复现新坑时按编号续写**——★ 2026-10-26 起**分册**：`踩坑记录.md` / `-2` / `-3` / `-4` / `-5` / `-6`，每册 ≤700 行，写满开新册、**不为新条目删旧条目**（对标 CurSeen 同款分册制）。新坑一律追加到**最后一册**末尾；K 号 → 册的对照表在第 1 册顶部，别处写「见 K68」照表跳册即可。旧坑根治后标「已根治」不删条目 | 自检（每条含现象 / 原因 / 解法，可照抄执行） |
| `docs/踩坑记录-2.md` … `-6.md` | 同上（各册头部写明"新坑追加到最后一册"）。**只有第 1 册收总索引** | 同上；`docs_consistency` 的 j3/j4 扫全部 md，册名不影响判据 |
| `docs/Rust 软件工程设计总纲.md` | **工程方法论宪章**（此前未登记，2026-10-19 补上）。只在"新的编译期约束被采纳 / 某条裁决被推翻"时更新顶部那张《采纳状态》表；正文方法论按需查阅，不追改。**只保留本仓采纳后的方法论**，不复制外部全文 | 自检 + `cargo test --release --test docs_consistency`（j3/j4/j5：点名的 `.rs` 存在、现行文档无考古串、行数上限） |
| `docs/扩展体系.md` | 扩展 manifest / 能力 / 目录约定 / 分层 / 安全底线变化时；未落地项随 P0.1b 更新 | `cargo test --release --lib core::ext -- --test-threads=1` + `python build/layering_scan.py` + 自检 |
| `docs/架构总览.md` | 五层分层、§三关键类型（`DecodeBackend` / `Fail` / `ImageCache` 等）、§五解码路由与清单规则（**只写动态并集，不登记具体 DLL**）、§七性能红线、§八改动影响面任一变化；「明确不做」清单（§九）变化 | `python build/layering_scan.py` + `hygiene_scan.py` 均 `violations: 0` |
| `docs/解码器开发规范.md`（**宿主侧 ABI 对接说明**，自包含） | 宿主看到的 ABI 版本、导出、结构体、错误码、缓冲所有权、能力路由变化时更新；契约结构体以 `vendor/rupix-abi` 为准 | 宿主 `cargo test --release --lib core::abi -- --test-threads=1` 全绿 + `cargo test --release --test docs_consistency -- --test-threads=1` |
| `docs/图片格式与后缀名.md` | 宿主内置路径（resvg / 点名 WIC）与「动态并集」规则变化；**具体解码器清单不写在这里**（真源 = `解码器仓库`） | `ext` 组测试（`python build/gate.py run ext`）+ 真实文件冒烟 |
| `docs/待办.md` | 候选项增删、状态变更（✅ / 🚫 / 调研结论） | 自检（与 HISTORY 不重复：待办记未来，HISTORY 记已发生） |
| `docs/产品定位与边界.md` | 战略定位 / 职责边界 / "轻量"硬定义 / 明确不做清单任一变化；本文件是战略层，内容长期稳定，改动应稀少 | 自检 + `python build/layering_scan.py`（引用分层结论时） + `cargo test --release --test docs_consistency` 全绿 |
| `docs/superpowers/plans/2026-10-12-titlebar-topmost-lock.md` | 已执行完毕的计划存档，不更新（同类新工作另开新文件） | 自检（与同名 spec 成对存在） |
| `docs/superpowers/specs/2026-10-12-titlebar-topmost-lock-design.md` | 设计变更时（被 `src/ui/chrome.rs` 注释与 `HISTORY.md` 引用） | 自检（引用路径成立） |

## 三、补充登记

| 文件 | 何时更新 | 谁验证 |
|---|---|---|
| `build/build_readme.md` | 构建 / 测试脚本名、流水线步骤、环境变量契约变化 | `build/full.bat` 全绿 |
| `.3rd/README.md`（★ 宿主第三方源码池，**不在任何仓库内**） | 新增/更新宿主第三方 C/C++ 库、许可变化、溯源缺口 | 宿主构建全绿；GPL/LGPL 库接入前需用户拍板 |
| `patches/README.md` | windui 补丁落地 / 撤除（`PATCHES.md` 登记 + 修改后完整文件归档） | 源码内 `★ RuPix 补丁 0NN` 标记计数与本文件一致 |
| `tests/README.md` | 测试布局（门禁组 vs 探针）变化 | `cargo test --release -- --test-threads=1` 全绿 |
| `tests/probes/README.md` | 探针清单增删 | 逐条 `cargo run --release --example <名>` 可执行 |
| `tests/docs_consistency.rs` | 新增/删除根文档、登记簿条目变化、考古串回流、src 文件增减、结构文档数字或符号索引漂移（j9 独立走盘 / j10 新鲜度 / j11 生成器自检）、发布目录自洽判据有没有钉子（j14 拉起 `deploy.py --selftest`） | `cargo test --release --test docs_consistency -- --test-threads=1` 全绿 |
| `tests/config_compat.rs` + `tests/fixtures/default.ini` | 新增/修改/废弃 ini 键（fixture 与 DEFAULT_INI 同步改） | `cargo test --release --test config_compat -- --test-threads=1` 全绿 |
| `tests/config_pipeline_e2e.rs` | 新增/修改 config→解码接线（decoders_disabled / decoders_preferred / safe-mode / 解码器目录解析） | `cargo test --release --test config_pipeline_e2e -- --test-threads=1` 全绿（需 `.build/decoders` 已发布） |

## 五、明确不入库（不在本登记簿）

`.temp/`（编译中间产物、临时文件、日志）、`.build/`（发布产物，可分发但不是文档）、
工具状态目录、`*.dll` / `*.exp` / `*.lib` / `*.pdb` / `*.obj`、
草稿与一次性实测转储（落 `.temp/`）。
