# RuPix 宿主（main）项目规则（AGENTS.md）

本文件对宿主仓 `main/` 工作文件夹生效（只用相对路径书写，不写任何本机的绝对路径）。任何 AI 工具在此文件夹工作时必须遵守。

> 解码器工作区是独立 cargo workspace，规则与文档都在它自己的仓库内，不在本文件展开。
> 宿主只认 `vendor/rupix-abi` 这份逐字节快照；快照更新由维护者从解码器仓库人工复制，宿主不在代码或文档里链接解码器仓库。

---

## 〇、战略定位（本文件之纲，其他 AI 先读这一节）

> **RuPix 是一个轻量看图器：宿主 exe 只做"看图"，位图解码全部外置为可替换的解码器 DLL。**
> 这条定位决定了一切后续规则——**框架是唯一长期优化的资产，解码器是可随时替换的商品。**

- **宿主只做**：窗口/交互、几何变换、解码调度与路由、解码器生命周期、ABI 契约、缓存、自包含分发、渐进预览、跨解码器资源共享。
- **解码器只做**：把具体格式解成 RGBA8；框架不关心它用什么语言写、多快，只认 `vendor/rupix-abi` 这份 C-ABI 契约快照。
- **"只做轻量宿主"是硬约束**：不堆功能、不引重依赖、不把解码库编进 exe、不碰注册表/用户目录。
- 完整定位、职责边界、"轻量"硬定义、明确不做清单见 [`docs/产品定位与边界.md`](docs/产品定位与边界.md)（战略宪章）。
- 本文件以下是**机器可执行的规则**；架构现行正本见 [`docs/架构总览.md`](docs/架构总览.md)；解码器 ABI 契约见 [`docs/解码器开发规范.md`](docs/解码器开发规范.md)。

---

## 一、五层架构与禁引规则（硬性）

`main` 是单 crate（`rupix` bin + `rupix_lib`），内部按五层组织。**依赖只许高层 → 低层，
低层不得 `use` 高层**；机器判定唯一真源是 `build/layering_scan.py` 的 `LAYER_ORDER` / `BARRED` 表，
本节与它必须保持一致。

```
core(0) → platform(1) → {decoders(2), extensions(2)} → ui(3) → runtime(4)
```

```
┌────────────────────────────────────────────────────────────┐
│ runtime(4)   装配层：run() + 16ms 轮询 + 解码线程接线           │  main.rs → runtime::run()
├────────────────────────────────────────────────────────────┤
│ ui(3)        windui 控件与窗口内容：brand/canvas/chrome/        │
│              settings/tray/raw_dialog/svg/window              │  禁 windows
├────────────────────────────────────────────────────────────┤
│ decoders(2)   解码器宿主：DLL 生命周期 + 目录 + FFI 编排           │  禁 windui / windows
│              （mod.rs + catalog.rs，Win32 一律走 platform）     │
├────────────────────────────────────────────────────────────┤
│ extensions(2) 非解码器扩展：manifest 发现 + 进程外编排 + 远程清单  │  禁 windui / windows
│              （与 decoders 平级且双向禁互引；命令/编辑型已接）      │
├────────────────────────────────────────────────────────────┤
│ platform(1)  Win32 薄封装：clipboard/dll/panic_guard/          │  禁 windui
│              seh/window_frame                                  │
├────────────────────────────────────────────────────────────┤
│ core(0)      纯逻辑：abi/config/decode/error/formats/          │  禁 windui / windows
│              gallery/geometry/hotkey/paths/pixels/thumbs/util/viewer              │  （单测必须能无头跑）
└────────────────────────────────────────────────────────────┘
```

| 层 | 禁引外部 crate | 违反后果 |
|---|---|---|
| core | `windui`、`windows` | 单测无法无头跑；依赖倒置被破坏 |
| platform | `windui` | Win32 薄封装混入 UI 框架 |
| decoders | `windui`、`windows` | Win32 一律经 platform |
| extensions | `windui`、`windows` | Win32 一律经 platform；扩展不进宿主地址空间 |
| ui | `windows` | 平台 API 只能经 platform |

- **core 不 import 解码器 / 扩展宿主**：解码经 `core::decode::DecodeBackend` trait（消费方定义），
  实现（`decoders::Host`）由 `install_decode_stack()`（`src/lib.rs`）在运行期注入；
  SVG 经 `SvgDecodeFn` fn 指针注入（真现在 `ui::svg::decode`）。扩展的纯数据模型在
  `core::ext`，发现/启停在 `extensions`；`decoders` 与 `extensions` 双向禁互引。
- `src/` 根下的 `lib.rs` / `main.rs` 是装配层，不判层，但仍受禁引约束影响的裸路径检查覆盖。
- 禁引检查同时覆盖 `use X::` 与正文裸写的 `X::`（注释豁免）——`main.rs` 曾直调
  `windui::…::svg_warm_fonts` 而无 use，use-only 扫描抓不到。

### 分层体检命令（改代码后必跑，0 违规为过）

```
cd main
python build\layering_scan.py     # 五层方向 + 各层禁引 crate + decoders/extensions 禁互引；violations: 0 才算过
python build\hygiene_scan.py      # 硬编路径 / 调试残留 / TODO 真标记 / FFI 边界无解 / 生产区 panic 源 /
                                  # 设置键的"最后一公里"（每个 ini 键既要有 UI 入口、也要有行为读者；
                                  #   `CONFIG_ONLY_KEYS` 登记者只要求行为读者；
                                  #   `UI_STATE_KEYS` 登记者（纯界面状态，如 `ui_cards`）只豁免行为读者、入口照旧要）/
                                  # 第三方补丁登记对账（PATCHES.md 章节 ↔ `.3rd` 标记两问；归档逐字节那一格由集成门禁 j13 守）/
                                  # 文本里的不可见控制字符（脚本改文件时反斜杠转义会吃掉一个字母，锚点会静默损坏）/
                                  # `.ps1` 必须纯 ASCII（PS 5.1 按 ANSI 读无 BOM 的 .ps1，中文注释会坏到邻近语句上，K27）/
                                  # ui 层色值只认 `ui/tokens.rs`（禁字面色值 `Color::rgb(`/`rgba(`）/
                                  # ui 层内跨模块引用一律经 `super::`（禁 `crate::ui::`，中枢在 `ui/mod.rs`）/
                                  # 单例窗不许同键既关又开（close_window 是投递、open_window 是即时，K196）/
                                  # 无密钥/无隐私（私钥块 / 访问令牌 / URL 内嵌凭据 / 口令字面量 / 私人邮箱 / 含真名的本机路径）；
                                  # violations: 0 才算过。★ 写 `.ps1` 注释一律英文；解码器仓同款判据在
                                  # 解码器仓库的 `check_docs.py`
```

两条也由 `build\2_scan.bat`（`full.bat` 第 2 步）强制执行；**违规即流水线红灯**。

---

## 二、临时产物与构建中间文件规则（硬性）

1. **所有编译中间产物、临时文件一律只放仓库根的 `.temp/` 目录**，包括：
   - 编译产物 target 目录：**宿主 → `.temp/target`**（`.cargo/config.toml` 已钉死，见下）。
     解码器仓库 是**另一个独立仓库**，它的编译中间产物落**本工作区** `.temp/decoder-target/`
     （由它自己的 `.cargo/config.toml` 钉死——源码仓库里不留 target）。
   - 调试 / 验证脚本（`.py` / `.ps1` / 临时探针）、构建日志、命令输出（**禁止重定向到 `main/` 或 `解码器仓库`**）。
   - 计划 / 进度 / 发现类文件由工作区计划区管理，不在宿主文档内展开；一次性草稿 / 实测转储 / 截图 / 样本图片 → `.temp`。
   - 退役源码备份、探针工具目录（`.temp/*probe*/`）等。
2. **禁止向 `main/` / `解码器仓库` 内写入任何日志、临时脚本或草稿文件**。`main/` 只放：源码（`src/`）、离屏探针（`examples/`）、构建脚本（`build/`）、文档（`*.md` / `docs/`）、资源（`image/` 品牌图标）、ABI 契约快照（`vendor/rupix-abi/`）。
   - `main/docs/` 的文档必须有长期价值（架构总览 / 解码器开发规范指针 / 图片格式与后缀名 / 待办 / 踩坑记录）。一次性草稿 / 实测转储放 `.temp`，不入库。「未受控文档进仓库」是明令禁止的。
3. **target 重定向是写死的**：
   - `main/.cargo/config.toml`：`[build] target-dir = "../.temp/target"`（宿主：不得改回 `./target`）
   - 解码器仓库的 `workspace/.cargo/config.toml`：`[build] target-dir = "../../.temp/decoder-target"`（相对**当前工作目录**解析，
     所以 cargo 必须从解码器仓库根跑——`build.ps1` 里有 `Push-Location $root` 保证这点；用户口径
     「源码文件夹只放自己写的代码」。换机器不用改文件，临时挪窝设 `CARGO_BUILD_TARGET_DIR`）
   - 把 `target-dir` 写进 `[package]` 是**无效键**（cargo 会警告并回落到 `./target`），只能写在 `[build]`。
4. 删除临时文件时只动 `.temp/` 内自己创建的文件。

---

## 三、`main/` 目录纯净性（硬性）

| 允许 | 禁止 |
|---|---|
| `src/` `examples/` `tests/` `build/` `docs/` `image/` `patches/` | 任何 `.log` / `.txt` 草稿 / `__pycache__` |
| `Cargo.toml` `Cargo.lock` `build.rs` `seh_shim.c` | 编译中间产物（`.rs.bk`、备份、`.target`） |
| 随源码的文档（`*.md` / `docs/`） | 一次性诊断脚本（→ `.temp/`） |
| 构建脚本（`build/full.bat` 及编号步骤脚本） | 未在文档体系登记的临时文档（→ `.temp/`） |

> 第三方依赖 `windui` 位于仓库根 `.3rd/wind-ui-rust-0.23.0`（本地 path 依赖）；
> `usvg` 0.48.1 位于 `.3rd/usvg-0.48.1`（经 `Cargo.toml` 的 `[patch.crates-io]` 接入，
> 补丁登记在 `patches/usvg-0.48.1/PATCHES.md`，归档逐字节由 j13 守）。
> **禁止**把 `main/` 文件复制到 `.3rd/`；反向也不允许（第三方库的修改要先登记补丁再重放：
> windui 见 `patches/PATCHES.md`，usvg 见 `patches/usvg-0.48.1/PATCHES.md`，编号只增）。
> ★ 解码器侧也有一个补丁副本：`.3rd/emfsdk-0.2.0`（只服务 `doc` 解码器的 `doc-emf` 成员，
> 与插件通道无关）。它的公开登记在解码器侧仓库自己的 `patches/emfsdk-0.2.0/`（那份登记不在本仓），
> 不归本文件的 `patches/` 管——宿主仓只记"它在、谁在用"；上游核对日期由解码器仓
> `tools/check_patches.py` 每 90 天逼一次（只核补丁结构与路径，不记哈希）。

### 无密钥 / 无隐私（两仓通用，2026-10-26，硬性）

`main/`、`解码器仓库`、`插件仓库` 都要能**原样公开**（三仓都已在 gitee 公开）。下列东西一条都不许进：

| 禁止 | 样子 | 正确做法 |
|---|---|---|
| 私钥 / 证书 | `-----BEGIN … PRIVATE KEY-----`、`.pfx` / `.pem` / `.p12` / `id_rsa*` | 放仓库外；运行时从环境变量读 |
| 访问令牌 | gitee / github 的 token（`glpat-` / `ghp_` / `AKIA` / `sk-` 开头的长串） | 只进本机 git 凭据管理器或环境变量 |
| URL 内嵌凭据 | 地址里写成「账号:口令@主机」的那种 | 远端只写 `https://host/owner/repo` |
| 口令字面量 | `password = "…"`、`api_key: "…"`（键名像凭据 + **带引号的值**） | 本机配置文件，且写进 `.gitignore` |
| 个人联系方式 | 手机号、QQ、微信号、私人邮箱 | 要署名就用仓库主页 |
| 含真名的本机路径 | `C:\Users\<真名>\…`（**机器判**）；文档里尽量别写 `E:\…` 这类本机构建路径（**人眼判**：它不泄密，但换台机器就指不到东西） | 相对路径 / `%LOCALAPPDATA%` / 环境变量 |

- 例外只有一处：**三方署名里的上游作者邮箱**（`THIRD-PARTY-NOTICES.md` / `LICENSE`）必须保留，
  那是法律要求的归属信息，不是我们的隐私。
- 机器把关：`python build\hygiene_scan.py` 的**第十二查 `secret`**（`2_scan` 自带，`violations: 0` 才算过）。
  判据刻意窄——只认高置信写法（`password: self.config.password` 这种赋值不算），
  因为一条天天误报的门禁，最后一定被人加白名单加到失效。加了新判据必须配**负向验证**。
- 解码器仓的对应判据是它自己的 `python tools/check_secrets.py`（两仓各自自包含，判据同源）。
- **万一已经泄了**：第一动作是**把那把凭据作废/换新**——删文件不管用，它已经在 git 历史里；
  再清历史、再补门禁。


---

## 四、整体布局与术语铁律

```
RuPix/
├─ main/              ← 宿主（本 AGENTS.md 管辖）：rupix.exe + rupix_lib
│  ├─ src/{core,platform,decoders,ui,runtime}/   五层（见 §一）
│  ├─ examples/       离屏探针 / 验证
│  ├─ tests/          集成门禁（tests/*.rs）+ 真机探针（tests/probes/，默认不编译）
│  ├─ build/          六步流水线 bat + layering_scan.py / hygiene_scan.py / gate.py
│  │                  / structure_gen.py（结构文档数字与符号索引的唯一写入者） / deploy.py
│  │                  / dump_sym.py（读自家报告/.dmp → 异常码 / 寄存器 / RVA / 线程名 / 逐帧 源码行）
│  │                  / size_map.py（读 /MAP 链接图 → exe 体积逐 crate 账本 + 闭合检查）
│  ├─ docs/           长期文档
│  ├─ image/          品牌图标（svg，资源）
│  ├─ patches/        第三方补丁登记（windui 卷 `PATCHES.md` + usvg 卷 `usvg-0.48.1/PATCHES.md`，编号只增）
│  ├─ seh_shim.c      C SEH 隔离层（build.rs 编译）
│  └─ build.rs        编译 seh_shim.c（cc，static_crt）
├─ 解码器仓库      解码器独立仓库（规则、契约与文档都在它内部，不在本文件展开）
├─ .build/             发布目录：rupix.exe + decoders/*.dll + plugins/<id>/ + docs/（两份随包清单
│                      与插件包）+ **`LICENSE` + `THIRD-PARTY-NOTICES.md`**（m182：许可全文与第三方
│                      署名随作品到达，`deploy.py` 硬拦——缺料或放错内容就不给部署）
├─ .3rd/               第三方本地副本：windui + usvg 0.48.1（宿主）+ emfsdk 0.2.0（解码器 doc 的 Rust 补丁副本）+ stb/libde265/libheif/jxrlib（解码器仓库的 C 源）
└─ .temp/              ★ **构建期**临时与编译中间产物（宿主 `target/`、解码器 `decoder-target/`、日志 / 探针 / 样本）；运行时缓存不落这里（见下便携铁律）
```

**便携铁律**：不往外部（`%APPDATA%` 等）写任何文件；持久化唯一落点 = exe 同级（`core::paths` 的 `OnceLock`）——**用户会打开看的**留根目录（`rupix.ini`、`decoders/`、`plugins/`、随包的 `docs/decoders.json` 与 `docs/plugins.json`），**只用来取证的**收进 exe 旁的 `temp\`（按天命名、7 天清理）：运行日志 `rupix-<YYYY-MM-DD>.log`、崩溃报告 `crash-<YYYY-MM-DD>.log`、转储 `crash-<YYYY-MM-DD-HHMMSS>.dmp`、会话戳 `rupix.run`；诊断物一律**按天命名**，启动时删掉**文件名日期**早于 7 天前的（`core::paths::KEEP_DAYS`，认不出日期的名字一律不删；**硬删、不进回收站**——回收站只兜用户自己的资产，见 `platform::recycle`）；**纯提速件**（丢了只损失速度、不损失证据，如 SVG 字体索引缓存 `cache\svg-fonts.bin`）收进 exe 旁的 `cache\`（`core::paths::cache_dir`；2026-11-01 用户裁定不得混进 temp；不参与按天清理）；**解码结果只在内存，不落盘**（磁盘解码缓存 2026-10-19 撤除，理由见 `HISTORY.md` 与 K68）；写失败 = 只读介质 → 静默降级为纯内存会话（只 warn）。仓库根 `.temp/` 是**构建期**临时区，与这里的运行时落点是两回事。

**桌面诊断铁律**（总纲元原理 E：进程跑在别人机器上，没有运维也没有 stdout）：
`temp\rupix-<今天>.log` 每次启动必须由 `platform::env_report::startup_preamble()` 先写一段会话头
（版本 + exe 字节数/构建时间、Windows 真实版本、是否提权、会话号与用户、系统 DPI、
每块显示器、完整命令行），崩溃报告复用**同一份**且只读启动时冻结的那份——
不许在 SEH filter 里现场采集。新增这类字段时同步改 `env_report` 的
`session_block_has_the_five_context_families` 用例。exe 必须有 VERSIONINFO
（`build.rs` + `assets/version.rc.in`），`deploy.py` 的 RT_VERSION 门禁负责硬拦。

**日志形状与级别铁律**（正本 = `platform::logging`，别处只许引用不许另写一份）：一行是
`[HH:MM:SS.mmm] [ LEVEL] target: 正文`，**默认档 `info,fontdb=error`**——一次正常会话必须在
日志里留下"打开了哪张 / 每张等了多久 / 关窗怎么裁决"，只有 warn 的日志等于没有日志。
装 logger 只能经 `platform::logging::install_logging()`（`main.rs` 与每个 `examples/*` 各调一次）：
`env_logger` 的 `default-features = false` 连带关掉了时间戳实现，**`format_timestamp_millis()`
在这种配置下静默失效**（见 K82）。判词行一律 `key=value`（`[main-latency] file=… src=decode|prefetch
cost=…ms bitmap=…`、`[decode-req] kind=… tier=… verdict=…`、`[close] locked=… action=…`），
"数标签行数 = 上屏次数、数 `src=prefetch` = 预读命中"这类读数必须能在默认档日志上直接数出来。
落盘上限 `core::paths::RUN_LOG_MAX_BYTES`：**写满即停 + 一条标记**，不许改回截断重写（见 K83）。

### 联网只有设置页两张下载卡（2026-10-26 立；m148 改成本地优先）

宿主的网络行为只有设置页**两张下载卡**（解码器 / 插件），动作都只有两个：**更新清单**
（只在用户点「刷新」时）与**拉对应资产**（只在用户点「下载 / 升级」时）：

- **进页只读随包清单**：`<exe>/docs/decoders.json`（`core::paths::docs_dir`；`deploy.py` 从
  解码器仓库的发布清单复制）。没网、镜像被墙、用户永不联网，这一页都打得开；文件
  缺失就如实说"点刷新联网获取"，**不自动联网**。
- 地址：**两份只读镜像**——Gitee（`https://raw.giteeusercontent.com/qiuzongman/RuPix-Decoders/raw/master/decoders.json`，
  CDN 直接返回清单、不 redirect）
  与 GitHub（`https://raw.githubusercontent.com/zongmanqiu/RuPix-Decoders/refs/heads/master/decoders.json`），
  由 ini 的 `decoder_index_gitee` / `decoder_index_github` 决定（默认值见
  `crate::core::config::SourceId::index_url`，**别在别处再写一遍**）。★ **两个 owner 不同**
  （Gitee `qiuzongman` / GitHub `zongmanqiu`），清单 URL 各自写死；★ **仓库名是复数
  `RuPix-Decoders`**（本地工作区的目录名是单数，两者不是一回事；2026-10-28 实测单数地址两个镜像都 404）。
  下载列由清单的 `gitee` / `github` 两列提供（`make_index.py` 推导时主机名与 owner **一起换**）。
  GitHub release 资产要单独上传；上传前 GitHub 下载列会 404，`check_urls.py` 会如实报错；
- 选哪个来源**同时决定两件事**（用户 2026-10-26 方案）：去哪拉清单，以及每枚 DLL 用清单里哪一列
下载地址（`gitee` / `github`）。清单顶层就是数组，一项一枚 DLL，十一个字段
`id/name/version/abi/size/released/desc/formats/gitee/github`——**没有哈希**（2026-11-02 用户口径：
官方产物不需要指纹，识别一枚只认 `id` + `version`；`size` 只作列表显示，不参与判定）。
`gitee` 是主源（作者手填）、
  `github` 默认由 `gitee` 推导（**主机名 + owner 一起换**，见 `make_index.py::derive_github_url`），
  也可用 `--set-github-url` 显式覆盖。清单里的地址还要与来源对得上：
  `SourceId::accepts_download_url` 把两个镜像域名上**别的 owner** 判成"该来源没有地址"
  （按钮灰掉），避免旧清单把用户送到 404。线上旧清单的 `github.com/qiuzongman/...` 在
  解析期由 `normalize_github_url` 归一成 `zongmanqiu`（只认本仓前缀，幂等；推送更新后是兜底）。
  切来源**不联网**：两列地址都在同一份清单里，只换列 + 换下一次刷新的 URL。
- 实现：`platform::http` 的最小 WinHTTP GET（四阶段超时 + 响应体上限 + 非 200 报错）；
  刷新走后台线程（`decoders::remote::start_refresh` / `poll_refresh`，设置窗 250 ms 取结果），
  **成功解析后才原子覆盖**本地 `docs/decoders.json`；失败保留旧清单并如实报原因。
  ★ 四阶段超时 **30 秒**（用户 2026-10-26 拍板；刷新在后台，等得起）。失败文案只给人话：
  「连接超时，请检查网络或稍后重试」/「连接失败，请检查网络」/「更新失败，请稍后重试」；
  `0x...` 与裸 HTTP 状态码**只进日志**，不显示在界面上（`catalog_error_text` /
  `download_error_text`）。渲染路径与进页路径里不许发请求、也不许在 UI 线程上同步 GET。
- 下载落点：解码器 `<exe>/decoders/<id>_<version>.dll`（`decoders::install`；后台线程 + `.part` 暂存 +
  字节数核对 + 装完读自述核对 id + 失败把旧文件原样放回），装好自动启用。
- 插件走同一套逻辑：本地 `<exe>/docs/plugins.json` + `<exe>/docs/*.zip` 优先，点「刷新」按
  `download_source`（选项页「下载源」，与解码器清单共用）拉 ini 里对应的 `plugin_index_gitee` / `plugin_index_github` 地址，点「下载 / 升级」由 `extensions::install` 后台拉
  release ZIP；**不校验清单指纹**（2026-11-02 同日口径，只认 id + 版本），解压 staging 后整目录替换，装好自动启用；远程取不到时回退本地 ZIP。
  插件清单里的 `gitee` / `github` 两列与解码器一样只换列、不换清单。
- ★ **联网更新的网址一律记在 `rupix.ini`、允许用户改**（2026-10-28 用户口径）：解码器
  `decoder_index_gitee` / `decoder_index_github`、插件 `plugin_index_gitee` / `plugin_index_github`；
  `SourceId::index_url` / `plugin_index_url` 只剩"内置默认值"的身份，`start_refresh_for` 读配置字段。
  ★ 2026-10-29 用户口径：**这四个地址不进界面**，只给高级用户手改 ini；设置页原先那张
  「更新地址」卡已撤。ini 里清空/删掉该项 = 回内置默认。新增任何取清单的路径都必须走配置，
  不许再直接引常量。
- 不做：上传任何数据、上报统计、自动更新、后台轮询。离线可用是底线：本地清单 + 已装 DLL
  永远可用；取不到就如实报错，其余功能一律照常。

新增任何联网行为都要先回答两个问题：**这份数据非联网不可吗？能不能随包分发？**
（例：解码器清单随包分发，联网只是为了"上架新档不必重发宿主"——这是选联网的唯一理由。）

### UI 版式铁律：对话框底栏只有一个入口（2026-11-06，硬性）

- **版式正本 = [`docs/界面版式规范.md`](docs/界面版式规范.md)**。改任何对话框的留白 / 间距 / 按钮位，
  先改那份文档，再改 `src/ui/tokens.rs`；代码与文档不一致时当场二选一，不许两边都放着。
- 自绘对话框（插件设置 / 调色参数 / 裸数据参数 / 恢复提示）一律
  `dialog_footer::shell(内容行, 右组按钮)`：四边 `DIALOG_PAD`(16)、行间距与按钮间距 `GAP`(8)、
  行高 `ROW_H`(40)、按钮**右对齐且贴底**（贴底靠壳里的弹性空白，不是算窗口高度——
  `fit_content` 有下限，内容矮时窗口不跟着缩）。
- 按钮只有两条路：普通按钮 `settings::rows::btn`、底栏按钮 `dialog_footer::action`。
  `Element::button(` 只许出现在 `btn` 的实现里——**hygiene 第 14 查 `footer_buttons` 守着**。
- 对话框内容行用 `option_row_inset(..., 0)`：设置页的「页 16 + 行 16 = 32」**不适用**于对话框，
  照抄会让标签与底栏按钮错开 16（2026-11-06 用户点名「按钮布局不一样」的实际形状）。

### 术语铁律：解码器 vs 解码库 vs 扩展名（2026-10-29 收口，三词各归其位）

- **解码器（decoder）**——指宿主这套 **DLL 外挂解码系统**。涵括：`src/decoders/` 这一层、
  exe 旁的 `decoders/` 发布目录、`解码器仓库` 工作区、`decoders::catalog`、
  设置窗「解码器」页、`LoadLibraryExW` 加载、ini 键 `decoders_disabled` / `decoders_preferred`。
  **凡机制义一律用「解码器 / decoder」，禁写「扩展」指代它。**
- **解码库**——指**一枚发布出去的 DLL**（`<id>_<ver>.dll`，如"pro 这枚解码库认领 wmf"）。
  机制义用「解码器」，单个产物用「解码库」，别把两词混着用。
- **扩展名（ext / suffix）**——指文件后缀（`.jpg` 等）。**凡后缀义一律用「扩展名」。**
- ★ **2026-10-26 那次「decoder 这个名字留给将来的真解码器系统、今天这套一律改叫别的」的方案
  作废**：它要求的方向与代码事实相反（今天这套在层目录、类型、ini 键、环境变量、发布目录里
  **就叫 decoder**），且当时的改名叙述是 `src/decoders/ → src/decoders/` 这种同名改同名。
  将来若真另起一套解码器系统、要占这个名字，**另立一次拍板**并把
  `src/decoders/`、`DecoderEntry`、`Handles::decoder_*`、`RUPIX_DECODERS_DIR`、
  ini 键 `decoders_*`、发布目录 `.build/decoders/` 一起改，不许像上次那样只写不改。
  **仍然有效的那条事实**：2026-10-26 起的 ini 键名**刻意不读旧键**，老用户的
  「停用 / 首选解码器」需重设一次（判据钉在 `tests/config_compat.rs` 的断言注释里）。
- **保留项（不得改名，改了两仓就分叉）**：ABI C 契约里的导出符号 `iv_decoder_info()`、
  结构体 `IvDecoderInfo`、宏 `export_decoder!`——它们以 `vendor/rupix-abi` 的逐字节快照为宿主侧真源，
  改名必须两仓同步发版，**不随术语调整**。另有 ABI 字段 `extensions`、std `with_extension()`、
  Rust 字段 `extensions: Vec<String>`、构建变量 `EXT_DIR`。
  ★ 所以看到 `iv_decoder_info` 别当成漏改——那是**契约**，不是命名遗留。
- **扩展名（ext / suffix）**——指文件后缀（`.jpg` 等）。**凡后缀义一律用「扩展名」。**
- **解码器仓库目录名**：由解码器仓库自己的规则维护；宿主文档统一称“解码器仓库”，不复制其目录名与改名史。
- **文档名已定**：《解码器开发规范.md》（曾名《扩展解码器规范.md》），所有引用改之。

---

## 五、解码器 ABI 契约（硬性，勿擅改）

- **ABI 现行 v6，v5 是签名冻结下界**（2026-10-26 拍板术语清理 = v5 冻结点，v4 及更早拒载；2026-10-28 拍板 v6 区域渲染 = 加法式：只新增可选导出 `iv_render_region` + 新结构体 `IvRegionReq` + 能力位 `CAP_REGION`，v5 的导出签名与结构体布局逐字节未动）。宿主只认 `vendor/rupix-abi` 这份逐字节快照；快照更新由维护者从解码器仓库人工复制覆盖（两边都不放同步脚本）。结构体布局靠编译期 `size_of` 断言钉死。
- 导出：必需 `iv_abi_version` / `iv_decoder_info` / `iv_probe` / `iv_render` / `iv_last_error`；可选（`GetProcAddress` 探测，缺席 = 缺能力，不是错误）`iv_render_raw` / `iv_caps` / `iv_metadata` / `iv_render_region`。★ `iv_export_svg` **已退役**（矢量导出的那条路线整条撤了）：符号按冻结规矩留在契约里，但宿主不再探测、不再调用，它归 `core::abi::ABI_RETIRED_SYMBOLS`，生产代码里再出现这个名字 = 依赖面冻结测试判红。C-ABI、cdylib、`#[repr(C)]`。
- **版本区间判定**：`MIN_ABI_VERSION(=5) ≤ 解码器 ≤ IV_ABI_VERSION(=6)`。低于下界和高出宿主一律拒载。**兼容是单向的**：v5 老 DLL 在 v6 宿主上照常收载（下界锁死不许抬），v6 DLL 在只认到 v5 的旧宿主上被拒。
- ★ **兼容性立约（永久条款，2026-10-28 用户拍板）**：**ABI 永远兼容旧版本 DLL，新 ABI 不得影响旧 DLL 的运行** ⇒ 宿主升级后，用户机器上的旧 DLL **不需要重新编译、不需要重新上传**。可判决的五条：(a) 已发布导出名 / 参数顺序 / 返回类型、`#[repr(C)]` 结构体字段名·类型·顺序、`ERR_*` / `PF_*` / `CAP_*` / `REQ_*` 编号**只增不改号**；(b) 新能力只走「新可选导出 + 新结构体 + 新位 / 新枚举值」，宿主 `GetProcAddress` 探测、缺席 = 缺能力不是错误；(c) `MIN_ABI_VERSION` **永不抬**（现 5），`IV_ABI_VERSION` 只增不减；(d) 宿主不得因升级改变既有导出的调用语义（默认 `flags` / `req`、`target=0`、`out=NULL` 冻结）；(e) **"改结构体再 bump"这条路禁止**——v5 改名是唯一历史断点，之后不许再有。契约仓侧的冻结指纹门禁在它自己仓里（不在本仓、宿主不参与）；**本仓的机器凭据是本地两条**：`core::abi` 的依赖面冻结测试（`iv_*` 只能减不能加）+ 快照比对用例。
- **宿主分配像素缓冲、解码器只写入**：彻底规避跨 DLL 释放崩溃。
- **ext=NULL 嗅探约定**：`ext == NULL` 时解码器**只能按内容（魔数）认领**；无魔数格式（TGA）应返回 0。
- **解码类型路由（v4 引入、v5/v6 继续保留的可选导出 `iv_caps`，2026-10-18）**：宿主选解码器的判据是
  `(后缀, 能力位图 abi::CAP_*)`，不是解码器身份。裁定在 `decoders::caps_of`（无声明/声明 0
  → 按 `is_vector` 与 `iv_render_raw` 在不在推定；有声明 → **声明 ∪ 推定**），仲裁在
  `select_candidate(ext, cands, require_caps)`。`CAP_SCALED`（原生降采样）**永不从缺席推出**。
  **单次请求的意图位 `REQ_ALLOW_SCALED`（flags 位 4，同日）**：点亮判据是
  `Req::allow_scaled || kind.wants_scaled()`——`Thumb` 自带，其余（`Main` / `Prefetch`）看**这一次请求**
  （`decoders::decode_with` 再与候选的能力位求与）。m35 起**换图首解与预读都点它**
  （那一框本来就是"够用的首屏"），清晰度由随后那次 `(0,0)` 原图档升级补上，`Decoded::natural`
  仍是原图尺寸 ⇒ 升级闸照旧会响。**同图重解与原图档升级显式关掉它**（那两档要的是真原图）。
  ★ 那次 `(0,0)` 升级在**连续翻页时让路**（m58：距上次翻页 <400 ms **且** 当前档对画面已够用
  ⇒ 推迟，手一停或一放大就照发）——它服务的是"已经不显示的那一张"，而一次原图档整解要独占
  唯一那条解码线程 150–500 ms 且不可抢占（实测快翻臂预读命中 5/24 → 14/24）。
  哪些实现声明 `CAP_SCALED` 由各 DLL 的 `iv_caps()` 自报，宿主**不维护名单**（`CAP_SCALED`
  永不从缺席推定）；换发布档、增删解码库都不需要改宿主。实测与选型账记在宿主 `HISTORY.md` 与
  解码器仓库自己的 README/历史里，两边各自留档。
- **产物命名**：`<id>_<ver>.dll`；**目录里只有 DLL，没有任何清单/指针文件**（自述全在 DLL 内的 `iv_decoder_info()`）；**同 id 只留一份 DLL**。
- **线程模型（契约）**：宿主单解码线程串行调用；解码器需**可重入**但不必 MT-safe；`iv_last_error` 线程局部；禁止解码器自建线程池。
- **对输入字节"推进且线性"（防挂死，正本 `docs/解码器开发规范.md` §六 第 10 条）**：由文件长度字段驱动的循环读到 0 长度必须推进或报错；递归 / 交叉引用用 **visited 去重**（只设深度上限不够，菱形重入是 `N^层数`）；轮数上界取自文件长度而非文件内的计数。**挂死风险源是「输入字节」，不是「解码器谁写的」**——自研解码器、第三方 DLL、宿主自己的元数据解析（`platform::exif` 走 UI 线程）适用同一条。隔离栈（`catch_unwind` / SEH / 三振 / `--safe-mode`）**只拦崩、不拦卡**。
- 新增 / 修改解码库：在解码器仓库内部完成；宿主 `main/` 不接收任何解码实现或解码库源码。

---

## 六、这台机器的执行环境（踩过的坑，硬性）

> 扩展版正本是 [`docs/踩坑记录/踩坑记录.md`](docs/踩坑记录/踩坑记录.md)（每条含现象 / 原因 / 解法，编号只增不改；
> **末号不写在这里**——写死计数就是第二正本，它上次停在 K66 而磁盘已到 K70）；下表为浓缩速查。

| 坑 | 对策 |
|---|---|
| ★ **`seh_shim.c` 禁用 `#include <windows.h>` 和 `GetExceptionCode`** | 构建脚本子进程 INCLUDE 无 SDK 路径 → `C1034`；`GetExceptionCode` 是宏，不 include 就 LNK2019。改用编译器内建 `_exception_code()`，常量 `#define` 硬编码，注释改英文避 GBK 代码页 C4819 |
| ★ **写空指针 `*(0 as *mut u8) = 1` 撞 `#[deny(deref_nullptr)]`** | 测试里触发访问违规用**非 0 wild 指针**（`usize::MAX`）+ `std::ptr::write_volatile` |
| ★ **`build.rs` 必须 `println!("cargo:rerun-if-changed=seh_shim.c")`** | 否则改 `.c` 后 cc 缓存不重编；仍 LNK2019 就手动清 `.temp/target/debug/build/rupix-*/out/*` |
| target 由 `.cargo/config.toml` 指定：宿主 `../.temp/target`，解码器仓库 `../../.temp/decoder-target`（相对 `workspace/`） | 两边产物都不在源码目录里，别去找 `main/target` 或 `解码器仓库 target` |
| cargo 偶发 `os error 5 拒绝访问`（杀软 / 索引持锁） | 重跑；设 `CARGO_INCREMENTAL=0` 可少一类噪声 |
| ★ **`cargo test` 多线程 runner 会被杀软锁二进制假死** | 一律 `cargo test --lib -- --test-threads=1`（或 `--release` 同参）；`5_test.bat` 两层测试都带此参。症状是「像挂了」，单线程几秒跑完 |
| 静态链接 CRT（`+crt-static`）：宿主与解码器 DLL 都必须开 | 混用动态 / 静态 CRT 会链接冲突；`seh_shim.c` 用 `cc` 的 `static_crt(true)` 对齐 |
| 解码库 / 光栅内核必须 `opt-level = 3`（`z` 让 JPEG 3MP 慢 4.6 倍、软光栅一帧慢 26%） | `[profile.release.package.*]` 对 tiny-skia / windui 提 3，**但这条例外只在 `lto = false` 时成立**：fat LTO 在链接期按宿主档位重编依赖位码，把例外冲掉（见 K53，宿主 profile 因此是 `lto = false`，exe +0.55 MB）；解码器侧同理 |
| 冒烟 / 实测时 decoders 目录必须可寻址 | example exe 在 `.temp/target` 下，跑前设 `RUPIX_DECODERS_DIR`（**绝对路径**）指向 `.build/decoders`，避免旧 DLL 拷贝测出错误数字 |
| ★ **PowerShell 没有 `&&`** | `a && b` 在 PowerShell 报错；顺序执行写两行，或用 `;`；bat 脚本里用 `if !ERRORLEVEL!` 串联 |
| ★ 2026-11-02 之前本仓无 `.git`，`cargo clippy --fix` 要加 `--allow-no-vcs` 才肯改文件 | 接入 git 后已不需要；若见到别人加这个参数，那是旧习惯 |
| 一条几十个文件的批处理命令会静默失败 | 复杂操作落成脚本再执行 |
| 本工具的终端**实际执行 PowerShell 5.1**（不是 git bash） | 别按 bash 语法写：`$(...)` / `Export` 一律换 PowerShell；**多行或复杂逻辑落成 `.ps1`**（纯 ASCII，见 K27）再 `powershell -NoProfile -ExecutionPolicy Bypass -File <path>`；前台 stdout 取不到就写文件再读，`>` 出 UTF-16（要读就用 `-Encoding Unicode` 转码，或走 `build\*.py`）。备注：本机另有 `git.exe`（`D:\ruanjian\Git\Git\cmd`）；本仓 2026-11-02 起有 `.git`，但与终端选择无关 |

---

## 七、源码已知陷阱（改代码前必读）

### ★ windui：scroll 里放 weight 子节点会塌成 0 高
- `list_signal(...).weight(1.0)` 一旦身处外层 `Element::scroll()`：滚动容器按无限高度
  测量子元素，权重子在 `measure_linear` 里两遍都被跳过 → 永远量不到 → arrange 0 高。
  症状是「数据到了、行也重建了，就是看不见」（2026-10-12 解码器页实测）。
- 正解：页面本体 `col().fill() … .child(list.weight(1.0))`，让 `list_signal` 自带的
  内部滚动去滚；重建子树含 weight 正文（表格）时换 `host_signal`。
- 回归测试：`ui::settings::decoders::tests::decoders_page_cards_survive_layout_and_scroll`（无头 Tree 验高度）。

### ★ windui：水平轴上的 `fill()` 会算进前面定宽的兄弟（K156）
- `row` 里放 `[标签 width(208), 控件 fill()]`：控件量到的是**整条主轴**可用宽（不减标签那一格），
  右缘会溢到行外。改用 `weight(1.0)` 才是"剩余宽"——但同一行里**只能有一颗**权重子
  （`option_row` 自带 `flex_spacer`，两颗各 1.0 就平分，控件只剩一半）。
- 需要"控件吃满余量"的行自己排：`[标签 width(COL1_W), 控件 weight(1.0)]`，不复用 `option_row`；
  无头布局用例的根必须是**定宽**容器（`col().width(640)`），否则 `width_match` 在无界宽下
  量出自然宽，用例永不失败（也会掩盖真 bug）。

### ★ UI 线程 panic = 窗口消失
- 交互全跑在 `extern "system"` 的 `wnd_proc` 里，**panic 展开不出去 = abort = 窗口无声没了**。
- `if let Some(x) = cell.borrow().f() { … cell.borrow_mut() }` 是禁写（`Ref` 活到整个语句 → `RefCell already borrowed`）。先 `let opt = cell.borrow().f();` 再 `if let`。
- 崩溃取证：`platform::panic_guard` 把消息 + 线程 + 源码位置写到 **`temp\crash-<今天>.log`** 并弹错误框；
  原生崩溃由 `platform::crash_evidence` 的 SEH 过滤器同处再落一枚 **`crash-<今天>-<时分秒>.dmp`**（两道闸门：最近 5 枚 + 7 天）。
  ★ 发布配置里 **`debug = 1` 不许删**：MSVC 下行表只进 PDB、不进 exe（体积不变），删了
  崩溃报告里就只剩 `<unknown>`、`.dmp` 也解不到源码行。**别升 `debug = 2`**：实测 21.9 MB → 74.2 MB
  而结果完全一样（"要名字必须开 2"是 `SYMBOL_INFO` 布局 bug 造成的误判，见 K135）。
- ★ 报告里的栈帧**必须写成 `模块+偏移`**（`RtlCaptureStackBackTrace` + `RtlPcToFileHeader`），
  不许用 `std::backtrace::Backtrace` 的 Display：后者在没有本地 PDB 时只有 `<unknown>`，
  名字和地址一起消失，发回来也解不出任何东西。偏移与 ASLR 无关，换台机器仍落到同一行。
- 读证据一条命令：`python build\dump_sym.py <temp\crash-<今天>.log>`（吃报告或 `.dmp` 都行，
  自动按指纹在 `.temp\symbols\` 里认 pdb，逐帧给 `函数 + 文件:行`；认不出会明说并退非 0）。
  **解码器那一侧**从 `.temp\symbols\decoders\<id>.pdb` 取（`build.ps1` 发布时归档；发布名
  `jpeg_0.1.0.dll` 与 CodeView 名 `jpeg.pdb` **不是一个名字**，脚本自己做换算，见 K137）；
  解码器 dll 在别处时用 `--decoders DIR` 指。本机无 WinDbg/cdb，走系统 dbghelp。
  ★ 解码器发布档必须带 `debug = 1`（`解码器仓库 Rust 实现的 Cargo.toml`）：否则 pdb 只有公开符号、
  解码器里崩了只能停在 `模块+偏移`（与宿主 `debug = 1` 同一条理由）。

### ★ canvas 的 ImageCache 按 gen 判脏，只在变化时重传
- `core::pixels::RgbaImage` 带单调递增的 `gen`（对齐 windui `Image::cache_id` 语义）。
  `ui/canvas.rs` 的 `ImageCache` **仅在 gen 变化时**做一次 `Image::from_rgba` 纹理重传；
  每帧路径仍是 `Rc` 克隆 + `draw_image`，零新增分配。转换只发生在「解码结果落地 / 旋转 / 翻转」。
- **不要**在每帧路径上重建 `windui::Image`——那正是 gen 判脏要消灭的浪费。上传失败保留旧纹理并记日志，不置错。

### ★ DecodeBackend trait 注入：core 禁直接调解码器
- `core::decode` 定义 `pub trait DecodeBackend`（`try_decode` / `try_decode_raw`）与
  `install_backend()`；`decoders::Host` 实现之；`install_decode_stack()`（`src/lib.rs`）接线，
  GUI 入口、`--probe-decode` 与 examples 统一走这一处。
- **core 层任何代码不得 `use crate::decoders`**（方向违规，layering_scan 会红）；探针要解码就传入 Backend 或调 `install_decode_stack()`。

### ★ SvgDecodeFn 注入
- SVG 解码是 fn 指针 `core::decode::SvgDecodeFn`（零虚表、单态化），真现 = `ui::svg::decode`
  （windui resvg），由 `set_svg_decoder()` 注入。core 不 import windui。
- 探针 / 示例要走真实 SVG 链路就调 `install_decode_stack()`；自测可传桩。

### ★ WindowOp 单槽
- `EventOutcome::window_op` 是 `Option<WindowOp>` 单槽：同一次分发里第二个 op 会**顶掉**第一个。多个窗口动作排成 `Vec<StartOp>` 一帧一个地发（`runtime/mod.rs` 的 `StartOp`）。

### ★ 透明度逐像素
- `WS_EX_LAYERED` + `UpdateLayeredWindow` 每像素 alpha；菜单展开期间整窗临时回 100%（`ctx.menu_open()`）；`pptDst` / `psize` 必须显式传（否则 ULW 返回成功但不上屏）。
- 100% 不透明时**摘掉 `WS_EX_LAYERED`**：`GetLayeredWindowAttributes` 查不到 `LWA_ALPHA` 是预期行为，不是 bug。

### ★ 便携：不碰外部
- 新增任何持久化必须走 `core::paths`（exe 同级 `OnceLock`），禁 CWD、禁 `%APPDATA%`；写失败 = 只读介质 → 静默降级为纯内存会话（只 warn）。

### ★ 解码路由
- 解码器优先、SVG 内置、**无 WIC 兜底**。格式清单正源是解码器 `iv_decoder_info()` 的动态并集 ∪ 内置 svg/svgz，不要手写第二份清单（已有测试锁住「清单与能力不脱节」）。
- **2026-10-25 用户裁决（现行，正本 `docs/产品定位与边界.md` §5.1）**：WIC **整层下线**——`platform::wic`、内置登记项、`Win32_Graphics_Imaging` 绑定与相关探针一并删除；宿主不调用任何系统位图解码器。resvg 仍登记为内置解码库（svg/svgz），只处理矢量，不参与位图后缀路由。

### ★ 退出路径上的 `Drop` 不许 `join` 后台线程
- 16 ms 轮询闭包属于框架宿主状态，而 **windui 在 `WM_DESTROY` 里（= UI 线程上）析构它**：闭包里任何
  `join()` / 等锁 / 等通道关闭的收尾，都可能把 UI 线程停在 `DestroyWindow` 里面——那时 `WM_QUIT` 已入队
  却没人读，`App::run()` 永不返回，退出收尾（回写 ini、摘钩子、`session::close_session()` 删会话戳）
  **整段不执行**。用户侧就是"窗口没了、任务管理器里还有一枚点不动的进程"，侧影是"每次启动都提示上次
  未正常结束"与 `deploy.py` 删不掉 `.exe.old`（2026-10-19 真踩，`DirWatcher`，见 K74 三臂实测）。
- 正确形状：**置停止位 → `CancelIoEx` 打断挂起 I/O → 关句柄 → 放手（drop 掉 `JoinHandle`）**。
  放手的内存安全判据 = 线程只持自己拥有的东西（数值副本 / move 进来的 `Sender` / `Arc`），不借结构体字段。
- ★ 这类缺陷**单测测不到**（join 与不 join 两版在裸测试里都绿）：哨兵用例只钉"不许无限等"与"线程会自己走掉"，
  定案要靠**真机配对复现**（红臂 8 s 不退 / 绿臂 0.1 s code=0 且会话戳被删）。

---

## 八、编码纪律

1. **生产代码禁止 `unwrap` / `expect` / `panic!` / `todo!`**（红线）。2026-10-17 起由
   `hygiene_scan.py` 第 5 查 `panic_source` **机器执行**——此前这条红线只写在文档里，
   一上去就扫出 6 处（见 `docs/踩坑记录/踩坑记录.md` K57）。例外仅三类：
   `#[cfg(test)]` 测试断言、`src/main.rs` 的 stdout 输出（`--probe-decode` 探针命令面，
   `hygiene_scan.py` 唯一豁免模块）、以及**刻意保留且不能降级的 fail-fast**：在违规行
   上方 3 行内写 `// 红线豁免: <为什么>`。不做文件级白名单、不做基线冻结——豁免要可 grep、
   可数、并且必须付出理由。UI 线程尤其不得 panic（见 §七）。
2. 注释写**为什么**而不是**做了什么**；每条非常规取舍留「踩过才知道」的理由。
3. 对第三方 `windui` 的任何修改都必须登记补丁（`patches/`）并在被改文件用 `★` 标注；升级依赖时从头重放。
4. 新增多于 60 行的逻辑必须带对应单测；纯逻辑优先、UI 相关用离屏渲染探针（`examples/`）。
   **真机探针的臂身份包含它跑在哪份配置上**：`.build\rupix.ini` 与 `.build\temp\rupix-<今天>.log`
   是运行期可变态，本机上还有别的自动化会同时开实例、改配置、清目录（实测一次），所以
   读数一律用 `build/flip_probe.ps1 -Private`（私有 exe 根 + 钉死的 ini + 私有日志根），
   共享目录里的读数只能当"现场发生过什么"，不能当对照（K84）。
5. **生产代码在前、`mod tests` 在文件末尾**是硬性布局——`hygiene_scan.py` 的 cfg(test) 豁免依赖它。
6. 改了源码 / ABI / 目录结构，同步 `docs/` 与 `DOCS_MANIFEST.md` 登记的对应文档（对照表见 `开发指南.md`）。
7. **提示（toast）纪律**（2026-10-16 清理后定）：RuPix 是极简软件，提示要克制——
   **只在「沉默会造成实际损失」时开口**。判据是「这条结果屏幕上已经看得见吗」：
   - **看得见 → 不提示**：元素出现/消失（极简模式、缩略图条）、窗口变透（不透明度）、
     计数行刷新（解码器扫描）、占用数字更新（缓存刷新）——屏幕自己会说话。
     历史教训：曾提示「极简模式已开启：悬停元素显示」，后半句是在**给没写清楚的菜单项补课**；
     正确的修法是改菜单项，不是加提示。
   - **看不见 → 必须提示**：复制到剪贴板这类**无痕副作用**（屏幕零变化，提示是唯一证据），
     以及**一切错误**——静默失败是最难查的一类 bug。错误分支是提示系统的底线，
     清理提示时**先确认删的不是报错通道**。
   - **★ 需要被记住的信息不许放进会自动消失的浮层**（toast 默认 3 秒、错误 5 秒），
     要放常驻位置——但**优先级更高的一条是：先把后果做成可逆的**。
     卸载解码器原本靠「按钮变『确认卸载？』+ 下方常驻说明」两件套兜底；改成
     **移进回收站**（可还原、且解码器以后可联网重下）之后，确认态与说明行整段删掉。
     二次确认是"后果不可逆"的补丁；后果能撤，就别拿它烦用户。
   - 删提示前必查：删掉后该操作会不会变成"点了完全没反应"？若是，先把信息挪到常驻位置。
8. **状态行纪律**（2026-10-29 用户裁定，解码器页与插件页共用同一套）：设置页两张下载卡的
   红字状态行**只许下面这些句子**，其余一切细节（包名、来源名、错误码、错误原文、
   "后台进行 / 装好自动启用"、跳过项计数）**只进日志**：
   `N 个可下载` / `N 个可下载 · M 个可升级` / `匹配 K 个` / `已更新` / `更新中…` /
   `更新失败` / `下载中…` / `下载失败` / `清单异常，请「刷新」`。
   - 刷新只有成败两态：取到清单但写不进本地文件 = **失败**（不立"取到了没落盘"的第三态）。
   - 默认 640px 设置窗里这一格只有 ≈392px（≈28 个汉字宽），长句会被 `truncate(End)` 吃掉；
     `正在下载「包名」…（后台进行，装好自动启用）` 这类句子禁止再出现（2026-10-29 撤）。
9. **对话框按钮纪律**（2026-10-29 用户裁定）：窗口底部按钮行必须 `flex_spacer` 右对齐、
   **取消在最右**、按钮之间留 8px 间距（正本 `ui/raw_dialog.rs`：`[确定] [取消]`）。
   禁止无间距并排、禁止把「取消」放在主按钮左边。

---

## 九、交付前核查清单（硬性）

| # | 项 | 命令 / 做法 | 通过判据 |
|---|---|---|---|
| 1 | 分层体检 | `cd main` → `python build\layering_scan.py` | `violations: 0` |
| 2 | 卫生体检 | `python build\hygiene_scan.py` | `violations: 0` |
| 3 | 格式化 | `cargo fmt --check` | exit 0（**已被 `build\2_scan.bat` 第 [3/6] 步强制执行**；此前列而不行，2026-10-17 补。★ 那一步一共 6 道：[1/6] 分层、[2/6] 卫生、[3/6] fmt、[4/6] clippy、[5/6] 结构文档新鲜度、[6/6] `dump_sym.py --selftest`） |
| 4 | 静态检查 | `cargo clippy --all-targets --release -- -D warnings` | exit 0。★ **必须带 `-D warnings`**：不带的话 clippy 有 warning 也退 0，"exit 0" 就是假绿。此列曾长期"列而不行"（与第 3 行 fmt 同一个毛病），2026-10-19 起由 `build\2_scan.bat` 第 [4/6] 步强制执行 |
| 5 | 宿主单测 | `cargo test --lib -- --test-threads=1` | 全部 passed |
| 6 | 集成门禁 | `cargo test --release -- --test-threads=1` | 全部 passed（含 docs_consistency） |
| 7 | 解码器构建 | 在解码器仓库根运行它自己的 `build.ps1` | 编译 + 测试 + 发布 DLL 全过 |
| 8 | 端到端冒烟 | `cargo run --release --example decoder_smoke -- <dll>...` / `--probe-decode` | 各解码器解码 OK |
| 9 | 发布 | `main\build\full.bat`（或 `6_deploy.bat`） | `rupix.exe` 落 `.build\`，解码器清单核对过；**许可全文与第三方署名清单也落 `.build\`**（`deploy.py` 的 `license_gate` 硬拦：缺文件、空壳、或全文不是 AGPL 都不给部署） |
| 10 | 文档同步 | 核 `docs/` 与 `DOCS_MANIFEST.md` | 清单里的文件描述成立 |
| 11 | 结构文档新鲜度 | `python build\structure_gen.py --check`（漂移则 `--write` 重生成） | exit 0；第一/三/四节数字与第五节符号索引 = `src/` 现状。**这些数字与索引不许手改**，改 `src/` 后跑 `--write` |

> 红线：本清单只检查、不提交。本仓 2026-11-02 起是 git 仓（origin = `gitee.com/qiuzongman/RuPix`，分支 `master`）；`commit` / `push` 仍须先取得用户明确许可。`.bf/` 快照继续打——它存的是 `main/` + `.3rd/` + 解码器仓三处合集，git 只管 `main/` 这一层。
> ★ 源码只推 Gitee、GitHub 是自动镜像（不手推），但 release 资产与说明两边都要传——规则见 `开发指南.md` §四（本节不复述）。
