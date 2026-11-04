# RuPix


**[⬇️ 发布版 (Gitee)](https://gitee.com/qiuzongman/RuPix/releases)**
 ｜ **[⬇️ 发布版 (GitHub)](https://github.com/zongmanqiu/RuPix/releases)**
 ｜ [Gitee主仓](https://gitee.com/qiuzongman/RuPix)
 ｜ [GitHub镜像](https://github.com/zongmanqiu/RuPix)

> 解码器仓库: [Gitee](https://gitee.com/qiuzongman/RuPix-Decoders) | [GitHub](https://github.com/zongmanqiu/RuPix-Decoders)

> 插件仓库: [Gitee](https://gitee.com/qiuzongman/RuPix-Plugins) | [GitHub](https://github.com/zongmanqiu/RuPix-Plugins)

Windows 10/11 上的小巧极速看图器。**宿主只做「看图」，位图解码全部外置为解码器 DLL**——
`rupix.exe`（体积与逐 crate 构成见
[`docs/产品定位与边界.md`](docs/产品定位与边界.md) §4.1，本页不复制数字）不内置任何第三方解码库，想多支持一种格式就是往 exe 旁边的
`decoders\` 放一个 DLL，宿主一行代码不用改。唯一例外：SVG 走内置 resvg（矢量需按
视口 1:1 重渲，位图解码器语义表达不了，见 [`docs/架构总览.md`](docs/架构总览.md)）。

**便携**：整个程序就是可拷走的一个目录，全部持久化写在 **exe 同级目录**
（配置 `rupix.ini` 与解码器 `decoders/` 在根，其余全部收在 exe 旁的 `temp\` 里：
运行日志 `rupix-<日期>.log`、崩溃报告 `crash-<日期>.log` 与转储 `crash-<日期>-<时分秒>.dmp`、会话戳 `rupix.run`），
不碰注册表、不碰用户配置目录——删掉目录就干净了。诊断物**按天命名**，启动时清掉文件名日期
早于 7 天前的那些，所以 `temp\` 不会越用越大。
`rupix-<日期>.log` 每次启动先写一段**会话头**（版本、Windows 版本、是否管理员运行、显示器与缩放、
启动参数），此后每一行都是 `[02:38:47.547] [ INFO] rupix_lib::core::viewer: 正文`——带本地时刻、
级别定宽、来源模块，并且"打开了哪张图、等了多久、用的是哪一档、关窗怎么裁决"都写在默认档里。
出问题时把 `temp\` 里最新那枚运行日志与崩溃报告发出来，环境就不用来回问。

界面范式模仿 [Pineapple Pictures（菠萝看图）](https://github.com/BLumia/pineapple-pictures)：
无边框深色窗口、画布铺满、无常驻工具栏/状态栏，控件悬浮、悬停才出现。

## 功能速览

- 窗口：无边框，四缘/四角都可拖动缩放（悬停时光标变成双箭头）/ Aero 吸附；顶部拖动区 + 文件名 + 置顶📌/锁住🔒/最小化/最大化/关闭；条上右键弹系统菜单
- **整窗不透明度**：默认 90%，10%~100% 十档可调，`Ctrl+↑/↓` 增减；菜单展开期间临时回 100%（否则深色菜单被糊掉）
- 悬浮层（悬停出现、静止 2s 隐藏）：底部图标工具栏（缩小/放大/适应/1:1/全屏/棋盘格/旋转/镜像）+ 信息小条
- 右键菜单：打开… / 复制当前（PNG 位图）/ 复制原文件（可直接粘贴进资源管理器）/ 复制路径 / 操作 ▸ / 极简模式 / 缩略图条 / 不透明度 ▸ / 设置…
- 设置窗五页：选项（各项**改完当场生效**并落盘）/ 快捷键（可重映射）/ **解码器**（计数行 + 文件夹/刷新 + 独立开关热加载）/ **解码**（按后缀列候选、可选首选；候选含内置 SVG 与已启用 DLL）/ 关于
- 颜色：文件带 ICC profile 时按系统 CMM（mscms）转 sRGB 上屏；无 profile 按 sRGB 原样显示
- **鸟瞰视图**：图超出视口时右下角出现缩略图 + 视口框，点击/拖动直接导航
- **长图模式**：旋转后宽高比超 2.5 自动按短边轴适应
- **SVG 无损放大**：可视区域 1:1 光栅化，平移/缩放防抖调度区域重渲
- 导航：`← →` 翻页（**永远循环**）、`Home/End` 首末张；‹ › 箭头——传统模式常驻，**极简模式靠边浮现**（图形按需显示，热区一寸不小）。所有快捷键都能在设置页改
- 滚轮翻页：一格一张，且**每 100 ms 最多一张**——触控板一次惯性滑行不会滑掉一排；故意慢滚每格都当场生效
- 缩放：滚轮（指针为锚点）、小键盘 `+ -`、`0` 适应窗口、`1` 原尺寸；`R` 旋转 90°、`M` 水平镜像
- 鼠标手势：默认**左键在画布里拖图片**（图比窗口小也拖得动，图边还能越过窗口边——但永远留 24dp 的图在画面里，拖不丢）、移动窗口交给顶部标题栏那一条；`Ctrl+左键` / 中键拖动平移图片（不看开关）、拖鸟瞰框改视角；开关在「选项 → 左键拖图」与右键菜单的开关区都有，取消勾选就回到"任意位置左键拖窗口"（挪够 4dp 才升级成拖动，保住双击关窗）
- 双击行为可配置（无操作/关窗/最大化/全屏），默认关窗；`Esc` 退出全屏
- 解码全在后台线程，UI 不卡；缩放停下 120ms 后才按需重新解码；相邻图预取（还在连续翻页时，
  原图档的补解会给翻页让路，手一停 0.4s 自动补上）
- **明确不做**：PDF/视频/音频；调用系统位图解码器（WIC 已于 2026-10-25 整层下线）；运行时依赖外部 JSON 解码器清单（元数据全在 DLL 内；在线源的 `decoders.json` 只服务下载器，不参与运行时路由）

## 快速开始

| 需求 | 构建 | 运行 |
| --- | --- | --- |
| Windows 10/11 x64 | 一键：`main\build\full.bat`（= `build.bat`，六步流水线：工具链体检 → 双静态门禁 → 解码器 DLL → 宿主 release → 测试门禁 → 部署） | `.build\rupix.exe [要打开的文件]` |
| Rust 1.94+，host 必须 `x86_64-pc-windows-msvc`（windui/windows 需 MSVC） | 单步常用：`main\build\4_build.bat` 只编宿主；`3_build_ext.bat` 只编解码器 | 解码器自动取 **exe 同级 `decoders\`**（部署产物即 `.build\decoders\*.dll`，随 exe 拷走） |
| Python 3 + PowerShell（构建脚本用）；仓库须带 `.3rd\wind-ui-rust-0.23.0` 本地副本 | 中间产物：宿主 `.temp\target`、解码器仓库 `.temp\decoder-target`（源码仓库里不放 target），发布 exe 落 **`.build\rupix.exe`** | 开发期跑 example/探针：设 **`RUPIX_DECODERS_DIR`（绝对路径）指向 `.build\decoders`**，否则 example 旁的旧 DLL 拷贝会让基准数字全错 |
| 无需安装解码器/运行库（位图解码全在解码器 DLL 内，静态 CRT） | 测试门禁：`cargo test --release -- --test-threads=1` 与 `--lib` 同参（杀软会锁新链接的测试二进制，单线程防假死） | 无解码器时 SVG/SVGZ 仍可打开；其余格式不出现在目录清单（能力与清单不脱节） |

## 目录树

```
RuPix\
├─ main\          宿主 crate（rupix.exe + rupix_lib）——本 README 所在
├─ RuPix-Decoder\   解码器独立 workspace（abi/ = 契约真源 + workspace/decoders/* = 发布档）
├─ .build\        发布产物：rupix.exe + decoders\*.dll（可分发目录）
└─ .temp\         全部编译中间产物与临时文件（两个 workspace 的 target 都指到这里）
```

逐目录的完整说明见 [`目录结构说明.md`](目录结构说明.md)。

## 技术栈

宿主（`main/Cargo.toml`）：

| 组件 | 用途 |
| --- | --- |
| windui 0.23.0（本地 path 依赖 `.3rd/wind-ui-rust-0.23.0`，features：svg / svg-text / d2d） | UI 框架 + 内置 resvg SVG 渲染（唯一内置解码例外） |
| windows 0.62（Win32 绑定：Foundation / Gdi / DataExchange / Ole / Memory / WindowsAndMessaging / HiDpi / LibraryLoader / RemoteDesktop） | 剪贴板、崩溃错误框、DPI 边框反算、DLL 搜索路径硬化、会话号（诊断头） |
| libloading 0.8 | LoadLibrary 包装（解码器 DLL 装载） |
| memmap2 0.9 | 解码器文件 mmap（免 12MB 级堆拷贝，失败回退 fs::read） |
| rupix-abi（path 依赖 `vendor/rupix-abi`） | 解码器 C-ABI 契约的宿主侧逐字节快照（真源在解码器仓库内部） |
| log + env_logger（关 regex/hantime 冗余特性） | 日志 |
| cc（build 依赖） | 编译 `seh_shim.c`（C SEH 隔离层） |

解码器仓库（独立 workspace）：

| 目录 | 作用 |
| --- | --- |
| `abi/` | C-ABI 契约真源 + 解码器样板 kit（宿主经 `vendor/rupix-abi` 逐字节快照使用） |
| `testdata/` | 通用图片/夹具 + 生成器（跨语言共用） |
| `tools/` | 通用门禁 / 清单 / 发布工具 |
| `workspace/decoders/<tier>/` | **发布档**：只做装配（依赖表 + 成员路由），不含解码逻辑 |
| `workspace/singles/<id>/` | 单库参考实现（bench / A-B 用，**不发布**） |
| `workspace/shared/{router,pro-support}/` | 档内路由内核与 pro 边界工具（rlib，不是解码器） |

具体有哪些档、每个档认领哪些后缀、用的哪个库：读解码器仓库自己的档位清单与发布清单。
**宿主 README 不复制这份清单**——解码器是可替换的商品，
清单只该有一个真源（DLL 自述 / 独立仓库）。

## 架构总览

五层分层，`extensions` 与 `decoders` 同层平级（依赖只许上层 → 下层，低层禁 `use` 高层）：

```
┌────────────────────────────────────────────────────────────┐
│ runtime(4)   装配层：run() + 16ms 轮询 + 解码线程接线           │  唯一认识所有人的接线点
├────────────────────────────────────────────────────────────┤
│ ui(3)        windui 控件与窗口内容：canvas/window/chrome/…     │  禁 windows
├────────────────────────────────────────────────────────────┤
│ decoders(2)   解码器宿主：DLL 生命周期 + 目录 + FFI 编排           │  禁 windui / windows
├────────────────────────────────────────────────────────────┤
│ extensions(2) 非解码器扩展：manifest 发现 + 进程外编排（P0）      │  禁 windui / windows
├────────────────────────────────────────────────────────────┤
│ platform(1)  Win32 薄封装：剪贴板/SEH/DLL/无边框窗口/崩溃钩子     │  禁 windui
├────────────────────────────────────────────────────────────┤
│ core(0)      纯逻辑：配置/解码调度/状态机/像素类型/几何/缓存      │  禁 windui / windows（单测无头跑）
└────────────────────────────────────────────────────────────┘
      依赖方向：runtime → ui → {decoders, extensions} → platform → core（下层不知道上层）
      机器判定唯一真源：build/layering_scan.py（违规 0 处才许提交/部署）
```

- 入口 `src/main.rs` → `runtime::run()`；解码栈由 `install_decode_stack()` 接线
  （core 定义 `DecodeBackend` trait 与 `SvgDecodeFn` 注入点，实现由高层在运行期装入）。
- 解码路由：内置登记表认领的后缀（今天 svg/svgz）→ 内置实现；其余一切 → 解码器 DLL（按魔数嗅探分派）。
  宿主不调用系统位图解码器；目录清单 = 内置登记表 `auto_claim=true` 的后缀 ∪ 各启用 DLL 自述扩展名的动态并集。
- 性能基线与重构复测数字见 [`HISTORY.md`](HISTORY.md) 与 [`docs/架构总览.md`](docs/架构总览.md) §七。

## 文档地图

| 想了解… | 读这里 |
| --- | --- |
| 项目全貌、快速上手（本文） | [`README.md`](README.md) |
| AI / 新人在此仓库工作的规则、红线、陷阱 | [`AGENTS.md`](AGENTS.md) |
| 五层架构与模块职责细节 | [`docs/架构总览.md`](docs/架构总览.md) |
| 写一个解码器 | [`docs/解码器开发规范.md`](docs/解码器开发规范.md)（宿主侧 ABI 对接）；解码器仓库另有独立的开发规范 |
| 写一个插件 | [`docs/扩展体系.md`](docs/扩展体系.md)（宿主侧契约）；插件源码在独立仓库 `RuPix-Plugin/` |
| 格式 ↔ 解码库 ↔ 后缀名对照 | `RuPix-Decoder/workspace/decoders/README.md` + `RuPix-Decoder/decoders.json`（清单真源；宿主只记录动态并集规则） |
| 当前待办与候选功能 | [`docs/待办.md`](docs/待办.md) |
| 产品定位、职责边界、轻量定义、明确不做（战略宪章） | [`docs/产品定位与边界.md`](docs/产品定位与边界.md) |
| 每个目录放什么、为什么 | [`目录结构说明.md`](目录结构说明.md) |
| 改代码的流程、改动 → 文档同步对照 | [`开发指南.md`](开发指南.md) |
| 文档登记簿（何时更新 / 谁验证） | [`DOCS_MANIFEST.md`](DOCS_MANIFEST.md) |
| 变更史 | [`HISTORY.md`](HISTORY.md) |
| 第三方依赖与许可清单 | [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) |
| 踩过的坑（改代码前必读） | [`docs/踩坑记录.md`](docs/踩坑记录.md) |
| 构建流水线契约与分步脚本 | [`build/build_readme.md`](build/build_readme.md) |
| 测试布局（门禁 vs 探针） | [`tests/README.md`](tests/README.md) |
| windui 补丁政策 | [`patches/README.md`](patches/README.md) |
| 性能基线与变更史 | [`HISTORY.md`](HISTORY.md) |
| 解码器工作区（独立 workspace）规则 | 在解码器仓库自己的文档内，不在宿主文档范围 |

## 命令行

| 参数 | 说明 |
| --- | --- |
| `<文件>`（首个非选项参数） | 启动时打开该文件（资源管理器「打开方式」、拖放等） |
| `--safe-mode` | 一键不加载任何解码器（解码器崩溃/卡死的最后止损） |
| `--probe-decode <文件>` | 隐藏探针：不经 UI 直接解码并打印结果后退出；失败或缺文件以非 0 退出码结束 |
| `--bench-thumb <文件> [次数]` | 隐藏基准：缩略图两条解码腿在生产链路上配对计时，逐次台账打到 stdout；`RUPIX_BENCH_REVERSE=1` 翻转臂序 |
| 其他 `--` 开头的参数 | 警告后忽略 |

| 环境变量 | 说明 |
| --- | --- |
| `RUPIX_DECODERS_DIR` | 覆盖解码器目录（**须绝对路径**），开发探针/基准指向 `.build\decoders` |
| `RUST_LOG` | 覆盖日志级别，默认 `info,fontdb=error`（一次正常会话的"开了哪张 / 等了多久 / 关窗怎么走"就在 info 这一档）；`debug` 起的是内部判词（解码分派、五段耗时、请求作废） |
| `RUPIX_DECODE_TRACE` | 解码追踪开关；输出走 `log::debug!`，需配合 `RUST_LOG=debug` |
| `RUPIX_SKIP_TESTS=1` | 构建脚本用：跳过测试门禁（快速本地循环） |
| `RUPIX_CARGO_ARGS` | 构建脚本用：第 4 步附加 cargo 参数 |

## License

AGPL-3.0，全文见 [`LICENSE`](LICENSE)。

## 致谢

- [Pineapple Pictures（菠萝看图）](https://github.com/BLumia/pineapple-pictures)——UI 范式与交互语义的来源
- CurSeen——工程管理范式（分层门禁、构建流水线、文档体系）的参考项目
- 各解码库作者：image / zune-jpeg / libwebp / jxl-oxide / heif-oxide / zenavif / rawler 等，
  解码器依赖清单在解码器仓库内独立维护；
  宿主自身依赖见 [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md)
