# RuPix —— Windows 10/11 上的小巧极速看图器

**[⬇️ 下载 0.4.3 (Gitee)](https://gitee.com/qiuzongman/RuPix/releases/download/0.4.3/RuPix_0.4.3.zip)**
 ｜ **[⬇️ 下载 0.4.3 (GitHub)](https://github.com/zongmanqiu/RuPix/releases/download/0.4.3/RuPix_0.4.3.zip)**

> 发布包已带 `classic` / `common` / `modern` / `pro` 4 枚基础解码器；`texture`、`doc`（PDF/PS/AI/DXF）、
> `raw`、`doc-legacy` 仍可在 设置 → 解码器 里按需下载。

> 宿主仓库（本仓）: [Gitee](https://gitee.com/qiuzongman/RuPix) | [GitHub](https://github.com/zongmanqiu/RuPix)

> 解码器仓库: [Gitee](https://gitee.com/qiuzongman/RuPix-Decoders) | [GitHub](https://github.com/zongmanqiu/RuPix-Decoders)

> 插件仓库: [Gitee](https://gitee.com/qiuzongman/RuPix-Plugins) | [GitHub](https://github.com/zongmanqiu/RuPix-Plugins)

## 功能速览

- **窗口**：无边框、四缘四角拖缩放、Aero 吸附；双击行为可配（无操作 / 关窗 / 最大化）；窗口尺寸三档（固定 / 全屏 / 图片尺寸）与无边框形态正交组合
- **空格看图**：开启**最小化到托盘**后，关窗只收走窗口、进程常驻托盘——资源管理器里选中文件按空格，直接盖出无边框预览，再按空格或 Esc 收起
- **连续看图**：右键「画面 ▸ 连续看图」，整条序列铺成可滚动长带（纵向 / 横向），Alt+滚轮沿带滚动、按住拖动 1:1 跟手；带内前后各预读 5 张，信息面板跟随滚动
- **多种看图模式**：单页翻页、连续看图随时开关、主辅视图同屏对比、幻灯片放映；目录、PDF、漫画包按「书」记住看到哪一页
- **压缩包直接看图**：zip/cbz、7z/cb7、rar/cbr、tar 系直接打开；文件树一次列全层级，点一层进一层
- **动图 / 文档播放**：GIF/APNG 与分页文档自动播放、0.25×–8× 变速；PDF 由 `doc` 解码器认领（约 26.7 MB，按需安装），一页当一帧
- **格式丰富，联网扩展**：宿主不内置位图解码，解码器与插件都在设置页**联网按需下载**，装多少管多少；解码全在后台线程 UI 不卡，崩溃由看门狗隔离，不带走看图进程

## 快速开始

| 需求 | 构建 | 运行 |
| --- | --- | --- |
| Windows 10/11 x64 | 一键：`main\build\full.bat`（= `build.bat`，六步流水线：工具链体检 → **七道静态门禁**（分层 / 卫生 / `cargo fmt --check` / `cargo clippy -D warnings` / 结构文档新鲜度 / `dump_sym.py --selftest` / 注释断链）→ 解码器 DLL → 宿主 release → 测试门禁 → 部署） | `.build\rupix.exe [要打开的文件]` |
| Rust 1.94+，host 必须 `x86_64-pc-windows-msvc`（windui/windows 需 MSVC） | 单步常用：`main\build\4_build.bat` 只编宿主；`3_build_ext.bat` 只编解码器 | 解码器自动取 **exe 同级 `decoders\`**（部署产物即 `.build\decoders\*.dll`，随 exe 拷走） |
| Python 3 + PowerShell（构建脚本用）；工作区须带 `.3rd\wind-ui-rust-0.35.0` 与 `.3rd\usvg-0.48.1` **两份**本地副本（前者是 `windui` 的 path 依赖，后者经 `Cargo.toml` 的 `[patch.crates-io]` 接入，缺任一 cargo 解析期就失败） | 中间产物：宿主 `.temp\target`、解码器侧 target（源码仓库里不放 target），发布 exe 落 **`.build\rupix.exe`** | 开发期跑 example/探针：设 **`RUPIX_DECODERS_DIR`（绝对路径）指向 `.build\decoders`**，否则 example 旁的旧 DLL 拷贝会让基准数字全错 |
| 无需安装解码器/运行库（位图解码全在解码器 DLL 内，静态 CRT） | 测试门禁：`cargo test --release -- --test-threads=1` 与 `--lib` 同参（杀软会锁新链接的测试二进制，单线程防假死） | 无解码器时 SVG/SVGZ 仍可打开；其余格式不出现在目录清单（能力与清单不脱节） |

> 第①步工具链体检**两份本地副本都查**：`.3rd\wind-ui-rust-0.35.0`（path 依赖）与
> `.3rd\usvg-0.48.1`（`[patch.crates-io]`）。任一份缺失都在那一步报 ERROR 并停下，
> 不会拖到第②步的 cargo 命令才变成一句更难懂的报错。

## 目录树

```
RuPix\                  ← 工作区根（宿主文档只画宿主这一侧）
├─ main\                宿主 crate（rupix.exe + rupix_lib）——本 README 所在
├─ .build\              发布产物：rupix.exe + decoders\*.dll + plugins\<id>\ + docs\（随包清单与插件包）+ LICENSE + THIRD-PARTY-NOTICES.md（可分发目录）
├─ .3rd\                第三方本地副本：wind-ui-rust-0.35.0 + usvg-0.48.1（两枚宿主 path 依赖，缺任一编不过）
└─ .temp\               全部编译中间产物与临时文件（宿主 target 与解码器侧 target 都指到这里）
```

解码器与插件各自是**独立仓库**（各自的规则、契约与构建脚本都在它们内部，不在宿主文档展开）：
宿主只认 `vendor\rupix-abi` 这份 ABI 契约逐字节快照，以及运行期 exe 旁的 `decoders\` 与 `plugins\` 目录。
逐目录的完整说明见 [`目录结构说明.md`](目录结构说明.md)。

## 技术栈

宿主（`main/Cargo.toml`）：

| 组件 | 用途 |
| --- | --- |
| windui 0.35.0（本地 path 依赖 `.3rd/wind-ui-rust-0.35.0`，features：svg / svg-text / d2d） | UI 框架 + 内置 resvg SVG 渲染（唯一内置解码例外） |
| usvg 0.48.1（`.3rd/usvg-0.48.1`，经 `[patch.crates-io]` 接入，不是直接依赖） | 把 windui 的 resvg 换成打了线程级排版缓存补丁的那一份（补丁登记见 `patches/usvg-0.48.1/PATCHES.md`） |
| windows 0.62（Win32 绑定，`[target.'cfg(windows)'.dependencies]` 共 **28 条 feature**，下面已省掉各自的 `Win32_` 前缀：Foundation / Security / Security_Authorization / System_Pipes / Graphics_Gdi / System_DataExchange / System_Ole / System_Memory / System_ProcessStatus / System_Threading / System_RemoteDesktop / System_SystemInformation / UI_WindowsAndMessaging / UI_Input_KeyboardAndMouse / UI_HiDpi / System_LibraryLoader / UI_Shell / UI_Shell_Common / System_Com / System_Variant / System_Diagnostics_Debug / System_Kernel / System_Registry / Storage_FileSystem / System_IO / System_JobObjects / System_Time / Networking_WinHttp） | 剪贴板、崩溃错误框与转储、DPI 边框反算、DLL 搜索路径硬化、会话号（诊断头）、命名管道与共享内存（插件进程）、回收站、**注册表（唯一例外：文件关联，仅 HKCU 且只写不撤）**、WinHTTP 取清单。逐条用途写在 `Cargo.toml` 的行内注释里 |
| libloading 0.8 | LoadLibrary 包装（解码器 DLL 装载） |
| memmap2 0.9 | 解码器文件 mmap（免 12MB 级堆拷贝，失败回退 fs::read） |
| png 0.18.1 | 「复制当前」= RGBA → PNG 编码后进剪贴板 |
| flate2 1 | svgz 的 gzip 解压（windui 的 resvg 没开 svgz feature，而清单与路由都登记了 svgz ⇒ 解压由宿主自己做） |
| zip 8（`default-features = false`，只开 `deflate-flate2`） | 解码器 / 插件下载包的校验与解包；以及看图时打开 `.zip` / `.cbz` |
| sevenz-rust2 0.25 + lzma-rust2 0.25 | 打开 `.7z` / `.cb7`（LZMA / LZMA2 / BCJ / PPMd / bzip2 / Deflate / AES **全算法，不裁剪**）；`lzma-rust2` 同时提供 `.tar.xz` 的**流式**解码 |
| unrar 0.5（vendored UnRAR C++） | 打开 `.rar` / `.cbr`。**只解不压**——许可允许解压、只禁止做 RAR 压缩器，原文见 `THIRD-PARTY-NOTICES.md` §四.6 |
| tar 0.4 + bzip2 0.6 + ruzstd 0.9 | 打开 `.tar` / `.tar.gz` / `.tar.bz2` / `.tar.zst`（tar 自身不含压缩，压缩层在外）；`bzip2` 与 7z 的 bzip2 coder 共用同一份 |
| rupix-abi（path 依赖 `vendor/rupix-abi`） | 解码器 C-ABI 契约的宿主侧逐字节快照（契约正本在解码器仓库内部） |
| log 0.4 + env_logger 0.11（`default-features = false`：regex / humantime / auto-color 全关） | 日志；时刻自己拼、着色器关掉（ANSI 码进日志文件是噪音） |
| cc 1（build 依赖） | 编译 `seh_shim.c`（C SEH 隔离层，`static_crt`） |
| embed-resource 3（build 依赖） | 把 `assets/*.rc`（图标 + VERSIONINFO）编进 exe 的 `.rsrc` |
| tiny-skia 0.12（**dev 依赖**，不进发布的 rupix 二进制） | `examples/paint_cost_probe` 直接驱动生产软光栅入口时要用它命名目标缓冲 |

解码器侧仓库（独立 workspace）与插件侧仓库（独立进程）都由**它们自己**维护规则、目录约定与
档位清单：有哪些档、每个档认领哪些后缀、用的哪个库，宿主 README 不复制——解码器是可替换的
商品，这份清单只该有一个真源（DLL 自述 + 那两个仓库自己的发布清单）。宿主这一侧只认
`vendor/rupix-abi` 那份 ABI 契约快照与 exe 旁的 `decoders\` / `plugins\` 目录。

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
│ extensions(2) 非解码器扩展：manifest 发现 + 进程外编排 + 远程清单  │  禁 windui / windows
├────────────────────────────────────────────────────────────┤
│ platform(1)  Win32 薄封装：剪贴板/SEH/DLL/无边框窗口/崩溃取证/   │  禁 windui
│              管道与共享内存/WinHTTP/日志/回收站/光标             │
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
- `extensions` 与 `decoders` 同层平级、**双向禁互引**（扩展不参与解码路由，解码器也不调扩展）。
  插件调用已接上线：命令型 / 编辑型 / 设置型三种任务都走 `extensions::run`（起进程 → 建管道 →
  发请求 → 收响应，全在后台线程，UI 只 `poll` 取结果），入口是右键「插件 ▸」。
- 性能基线与重构复测数字见 [`HISTORY.md`](HISTORY.md) 与 [`docs/架构总览.md`](docs/架构总览.md) §七。

## 文档地图

| 想了解… | 读这里 |
| --- | --- |
| 项目全貌、快速上手（本文） | [`README.md`](README.md) |
| 可下载档位的格式覆盖（生成物） | [`docs/格式覆盖.md`](docs/格式覆盖.md)；本机实际覆盖跑 `rupix.exe --list-formats` |
| AI / 新人在此仓库工作的规则、红线、陷阱 | [`AGENTS.md`](AGENTS.md) |
| 五层架构与模块职责细节 | [`docs/架构总览.md`](docs/架构总览.md) |
| 写一个解码器 | [`docs/解码器开发规范.md`](docs/解码器开发规范.md)（宿主侧 ABI 对接）；解码器仓库另有独立的开发规范 |
| 写一个插件 | [`docs/扩展体系.md`](docs/扩展体系.md)（宿主侧契约）；插件源码与《插件开发规范》在插件侧仓库自己维护 |
| 格式 ↔ 解码库 ↔ 后缀名对照 | 解码器侧仓库自己的档位 README 与发布清单（清单真源）；宿主只记录"动态并集"这条规则，见 [`docs/图片格式与后缀名.md`](docs/图片格式与后缀名.md) |
| 当前还剩什么要做（唯一清单） | [`docs/待办.md`](docs/待办.md) |
| 让 AI 客户端接入 RuPix、换一个 AI 来验收 | [`docs/MCP接入指南.md`](docs/MCP接入指南.md)（随包同名；适配器是仓外独立组件，不在发行包内） |
| 产品定位、职责边界、轻量定义、明确不做（战略宪章） | [`docs/产品定位与边界.md`](docs/产品定位与边界.md) |
| 每个目录放什么、为什么 | [`目录结构说明.md`](目录结构说明.md) |
| 改代码的流程、改动 → 文档同步对照 | [`开发指南.md`](开发指南.md) |
| 文档登记簿（何时更新 / 谁验证） | [`DOCS_MANIFEST.md`](DOCS_MANIFEST.md) |
| 变更史 | [`HISTORY.md`](HISTORY.md) |
| 宿主更新清单（版本 / 说明 / 两镜像地址） | [`update.json`](update.json)；「关于」页手动检查更新 |
| 第三方依赖与许可清单 | [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) |
| 踩过的坑（改代码前必读） | [`docs/踩坑记录/踩坑记录.md`](docs/踩坑记录/踩坑记录.md) |
| 构建流水线契约与分步脚本 | [`build/build_readme.md`](build/build_readme.md) |
| 测试布局（门禁 vs 探针） | [`tests/README.md`](tests/README.md) |
| 第三方补丁政策与登记（windui 卷 + usvg 卷） | [`patches/README.md`](patches/README.md)、[`patches/PATCHES.md`](patches/PATCHES.md)、[`patches/usvg-0.48.1/PATCHES.md`](patches/usvg-0.48.1/PATCHES.md) |
| 性能基线与变更史 | [`HISTORY.md`](HISTORY.md) |
| 解码器工作区（独立 workspace）规则 | 在解码器仓库自己的文档内，不在宿主文档范围 |

## 命令行

| 参数 | 说明 |
| --- | --- |
| `<文件>`（首个非选项参数） | 启动时打开该文件（资源管理器「打开方式」、拖放等） |
| `--safe-mode` | 一键不加载任何解码器（解码器崩溃/卡死的最后止损） |
| `--probe-decode <文件>` | 隐藏探针：不经 UI 直接解码并打印结果后退出；失败或缺文件以非 0 退出码结束 |
| `--bench-thumb <文件> [次数]` | 隐藏基准：缩略图两条解码腿在生产链路上配对计时，逐次台账打到 stdout；`RUPIX_BENCH_REVERSE=1` 翻转臂序 |
| `--list <目录>` | 无头命令面：一行 JSON 回「这个目录 RuPix 认哪些图」，与翻页清单同源；不建窗 |
| `--metadata <文件>` | 无头命令面：一行 JSON 回大小 / 时间 / 后缀与 EXIF 六行；**不解码** |
| `--thumbnail <文件>` | 无头命令面：一行 JSON 回 240×180 缩略图的 PNG（base64 内联） |
| `--render <文件> [宽 高]` | 无头命令面：一行 JSON 回解码像素的 PNG（base64 内联）、原图尺寸与帧数；缺宽高按原尺寸 |
| `--list-formats` | 无头命令面：一行 JSON 回内置扩展名、各解码器自述、唯一扩展名并集与计数；不建窗 |
| 其他 `--` 开头的参数 | 警告后忽略 |

> 五个无头动词与 GUI 共用同一条解码链，stdout **恰好一行 JSON**（日志只落盘），
> 退出码 `0` 成功 / `2` 参数错 / `3` 失败 / `4` 上限拒绝，与响应里的 `code` 字段同值。
> 发布版是 GUI 子系统，调用方需重定向 stdout 才能取到那一行；它们不建窗、不进单实例闸门、
> 不常驻也不监听端口，供进程外适配器（如 MCP server）转调。

| 环境变量 | 说明 |
| --- | --- |
| `RUPIX_DECODERS_DIR` | 覆盖解码器目录（**须绝对路径**），开发探针/基准指向 `.build\decoders` |
| `RUST_LOG` | 覆盖日志级别，默认 `info,fontdb=error`（一次正常会话的"开了哪张 / 等了多久 / 关窗怎么走"就在 info 这一档）；`debug` 起的是内部判词（解码分派、五段耗时、请求作废） |
| `RUPIX_DECODE_TRACE` | 解码追踪开关；输出走 `log::debug!`，需配合 `RUST_LOG=debug` |
| `RUPIX_DIR_TRACE` | 目录重扫取证开关：每次重扫打一行 `[dir-rescan] … shared=`（`shared=1` = 这一扇窗复用了别的窗口刚扫出来的清单，没读盘） |
| `RUPIX_SKIP_TESTS=1` | 构建脚本用：跳过测试门禁（快速本地循环） |
| `RUPIX_CARGO_ARGS` | 构建脚本用：第 4 步附加 cargo 参数 |

## License

AGPL-3.0，全文见 [`LICENSE`](LICENSE)。
