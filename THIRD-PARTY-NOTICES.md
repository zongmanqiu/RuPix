# 第三方组件许可与说明（THIRD-PARTY-NOTICES.md）

> 本项目自身以 **AGPL-3.0** 授权（实读 `main/Cargo.toml`：`license = "AGPL-3.0"`，全文见 `main/LICENSE`）。
> 本文件登记**宿主自身**的直接依赖与随 `rupix.exe` 分发的第三方组件及其许可证。
> **解码器是独立仓库/独立发布的商品，其第三方依赖不在本文件登记**（见 §二指针）。
>
> **读取日期 2026-10-25；版本号一律实读，不许凭记忆写**：
> 本文件只读 `main/Cargo.toml` + `main/Cargo.lock`。解码器的第三方许可见
> `解码器仓库许可清单`；两边各自以自己的 `Cargo.lock` 为权威全集。
>
> **许可证列的依据分四级**（本文件不猜）：
> **[实读字段]** = 该 crate 自己 `Cargo.toml` 的 `license` 字段；**[实读注释]** = 本仓依赖注释写明；
> **[实测]** = 仓内许可核验留档（下载 `.crate` 解压读真实 `Cargo.toml`，见许可核验记录 / 当日工作日志；
> 2026-10-21 m76 那批改用 `cargo metadata` 读**同一字段**——已发布包自己的 `Cargo.toml`，取法不同、判据不变）；
> **[待核]** = 本机无留档，**分发二进制前必须按 `.crate` 读法补核**，禁止先填一个像样的答案。

---

## 一、windui（UI 框架，本地 path 依赖 —— 唯一的宿主侧第三方源码副本）

| 项 | 实读值 |
|---|---|
| 依赖声明 | `windui = { path = "../.3rd/wind-ui-rust-0.23.0", version = "0.23.0", default-features = false, features = ["svg", "svg-text", "d2d"] }`（`main/Cargo.toml`） |
| 位置 | 仓库根 `.3rd/wind-ui-rust-0.23.0`（**不在 `main/` 内**，禁止把 `main/` 文件复制进去，反向也不允许） |
| name / version | `windui` / `0.23.0`（实读 `.3rd/wind-ui-rust-0.23.0/Cargo.toml`） |
| **许可证** | **`MIT OR Apache-2.0`**（[实读字段] `.3rd/wind-ui-rust-0.23.0/Cargo.toml` 的 `license` 行） |
| 许可文件 | **`LICENSE-MIT` 与 `LICENSE-APACHE` 两份原文均在** `.3rd/wind-ui-rust-0.23.0/` 目录内（实读目录清单）——无需标注「许可证文件未见」 |
| 上游 | `repository = "https://github.com/huanfeng/wind-ui-rust"`（[实读字段]），作者 `huanfeng <huanfengf@gmail.com>` |
| 本地修改 | **已落补丁多枚，份数与逐枚说明以 [`patches/PATCHES.md`](patches/PATCHES.md) 的章节为准**（登记正本在那里，本行不抄第二份；01/03/04/05/06 于 2026-10-13 在 0.23.0 上重放，02/07 已被 0.23.0 上游收编）：修改后完整文件归档于 `main/patches/wind-ui-rust-0.23.0/`，机制与生命周期见 `patches/README.md`；今后任何修改仍须按该生命周期登记 |

> **「OR」的含义**：`MIT OR Apache-2.0` 是 SPDX 许可表达式，作者同时按两个许可证授权，使用者任选其一满足即可；
> 不得自行删改成单一许可证。windui 由 path 依赖编进 `rupix.exe`，其许可义务随二进制分发。

### 1.1 windui 带进 exe 的关键传递依赖（实读 `main/Cargo.lock`）

| 组件 | 版本 | 许可证 | 说明 |
|---|---|---|---|
| `tiny-skia` | 0.12.0 | **BSD-3-Clause** [实测（仓内留档勘误：曾被误写为 MIT OR Apache-2.0）] | CPU 光栅化内核（`[profile.release.package.tiny-skia] opt-level = 3`） |
| `resvg` | 0.48.1 | Apache-2.0 OR MIT [实测（仓内留档）] | SVG 光栅化 —— **RuPix 内置解码的唯一例外** |
| `usvg` | 0.48.1 | Apache-2.0 OR MIT [实测（仓内留档）] | SVG 解析 / 文字栈（`svg-text` feature） |
| `fontdb` | 0.24.0 | MIT [实测] | 系统字体加载（首开 SVG 扫一遍后 thread_local 缓存） |
| `png` | 0.18.1 | MIT OR Apache-2.0 [实测（仓内留档）] | 经 `tiny-skia` 传入（反向依赖实读：`tiny-skia → png`） |
| `toml` | 1.1.6+spec-1.1.0 | MIT OR Apache-2.0 [实测] | windui 主题配置读取（**是运行期依赖，不是构建工具**） |
| `wgpu` | 30.0.1 | MIT OR Apache-2.0 [实测] | windui 渲染后端 |
| `rfd` | 0.17.2 | MIT [实测] | 文件对话框 |
| `bytemuck` / `serde` / `pollster` / `raw-window-handle` | 1.25.2 / 1.0.229 / 1.0.1（依赖图里另有 0.4.0 一份）/ 0.6.2 | Zlib OR Apache-2.0 OR MIT / MIT OR Apache-2.0 / Apache-2.0/MIT / MIT OR Apache-2.0 OR Zlib [实测] | windui 依赖 |

---

## 二、解码器（独立仓库，不在此登记）

解码 DLL 及其第三方库、许可证由独立仓库 **`解码器仓库许可清单`** 登记。
宿主只认 C-ABI，**不在本文件复制这份清单**：解码器可独立合并/换库/改版本，宿主文档跟着抄一份
必然当天过期。宿主自己的 `vendor/rupix-abi` 是本仓 AGPL-3.0 契约的逐字节快照，无第三方依赖。

> 本文件只登记随 `rupix.exe` 一起分发的第三方组件（windui/resvg 栈与宿主直接依赖，见 §一、§三）。
> 发布 `.build/` 时，若同时附带解码器 DLL，其许可义务按解码器仓的 notice 文件履行。

---

## 三、宿主 `main/` 的直接依赖关键项（实读 `main/Cargo.toml` + `main/Cargo.lock`）

| 组件 | 声明 | 版本（lock） | 许可证 | 用途 |
|---|---|---|---|---|
| `windui` | path `../.3rd/wind-ui-rust-0.23.0` | 0.23.0 | MIT OR Apache-2.0 [实读字段] | UI 框架（详见第一节） |
| `rupix-abi` | path `vendor/rupix-abi`（上游 `RuPix-Decoder/abi` 的逐字节快照） | 0.5.0 | AGPL-3.0 [实读字段] | 解码器 ABI 契约；真源在解码器仓库，宿主用快照以便独立构建 |
| `log` | `0.4` | 0.4.34 | MIT OR Apache-2.0 [实测] | 日志门面 |
| `env_logger` | `0.11`，`default-features=false`，仅 `auto-color` | 0.11.11 | MIT OR Apache-2.0 [实测] | 日志输出 |
| `libloading` | `0.8` | 0.8.9 | **ISC** [实测]（★ 旧推定「惯例 MIT OR Apache-2.0」**不成立**） | `LoadLibraryExW` 包装（解码器加载） |
| `memmap2` | `0.9` | 0.9.11 | MIT OR Apache-2.0 [实测]（★ 不是单 MIT） | 解码器文件 mmap（免 12MB 级堆拷贝，失败回退 `fs::read`） |
| `windows`（仅 `cfg(windows)`） | `0.62`（feature 全集见 `README.md` 技术栈表，2026-11-10 实读 28 条） | 0.62.2 | MIT OR Apache-2.0 [实测] | Win32 绑定（剪贴板 / DPI / 管道与共享内存 / 注册表·仅文件关联 / WinHTTP 等；逐条用途写在 `Cargo.toml` 的行内注释里） |
| `cc`（build-dependencies） | `1` | 1.6.0（解码器仓依赖图里是 1.7.0） | MIT OR Apache-2.0 [实测] | 编译 `seh_shim.c`（C SEH 隔离层） |
| `embed-resource`（build-dependencies） | `3` | 3.0.12 | **MIT** [实测]（★ 单许可，不是双许可） | 编译 `assets/rupix.rc` → exe `.rsrc` 的 PE 图标（资源管理器/任务管理器显示它） |
| `zip` | `8`，`default-features=false`，仅 `deflate-flate2` | 8.6.0 | **MIT** [实读字段]（★ 单许可，不是双许可） | ZIP / CBZ 容器 |
| `sevenz-rust2` | `0.25`（默认特性 = 全算法，**刻意不裁剪**） | 0.25.0 | Apache-2.0 [实读字段] | 7z 容器 |
| `lzma-rust2` | `0.25` | 0.25.0 | Apache-2.0 [实读字段] | 7z 的 LZMA/LZMA2；`.tar.xz` 的**流式** `XzReader`（不引 `lzma-rs`，那一个只能整条解进内存） |
| `unrar` | `0.5` | 0.5.8 | MIT OR Apache-2.0 [实读字段]（★ 指 wrapper 本身；内含的 UnRAR C++ 源码另有许可，见 §四.6） | RAR / CBR 容器 |
| `unrar_sys` | 传递（`unrar` 依赖） | 0.5.8 | MIT OR Apache-2.0 [实读字段]（同上，见 §四.6） | vendored UnRAR C++ 与 FFI 绑定 |
| `tar` | `0.4` | 0.4.46 | MIT OR Apache-2.0 [实读字段] | tar 归档（**自身不含压缩**，压缩层在其外） |
| `bzip2` | `0.6` | 0.6.1 | MIT OR Apache-2.0 [实读字段] | `.tar.bz2`，以及 7z 里的 bzip2 coder |
| `libbz2-rs-sys` | 传递（`bzip2` 的默认后端） | 0.2.6 | **bzip2-1.0.6** [实读字段]（★ 不是 MIT/Apache，是 bzip2 自己的历史许可） | 纯 Rust bzip2 实现 |
| `ruzstd` | `0.9` | 0.9.1 | **MIT** [实读字段]（★ 单许可） | `.tar.zst` 解码 |
| `ppmd-rust` | 传递（`sevenz-rust2` 的 `ppmd` 特性） | 1.5.2 | CC0-1.0 OR MIT-0 [实读字段] | 7z 的 PPMd coder |
| `aes` / `cbc` / `sha2` | 传递（`sevenz-rust2` 的 `aes256` 特性） | 0.9.4 / 0.2.1 / 0.11.1 | MIT OR Apache-2.0 [实读字段] | 7z 的 AES-256 加密条目 |

> **实读勘误（2026-10-11，2026-10-12 修订）**：`main/Cargo.toml` 中**没有 `winres`、没有 `image`**，
> `main/Cargo.lock` 中也没有 `winres` / `image` / `zune-jpeg` / `gif` / `tiff`。
> 宿主不含任何图片解码 crate（位图解码全部走解码器 DLL，SVG 走 windui 内置 resvg）；
> 版本 / 图标资源也不走 `winres`：PE 图标用 `embed-resource` 3 编译 `assets/rupix.rc`
>（`1 ICON "rupix.ico"`，ico 由 `cargo run --example gen_icon` 生成），`build.rs` 另编译 `seh_shim.c`。
> 历史方案文档里的 `winres`、`fast_image_resize` 属于**参考项目调研结论，不是本项目依赖**，
> 不得写进本文件。

---

## 四、分发义务与待补（当前状态）

1. **许可全文随副本**：MIT / Apache-2.0 / BSD-3-Clause 类组件要求许可文本随副本分发。
   当前 `main/` **没有 `licenses/` 目录**（总计划 Phase 1 结论：不建，因为不再分发第三方**源码**）——
   但 `rupix.exe` 内编译进了 windui 栈，**开始对外分发二进制前必须补**：
   至少附 `windui` 的 `LICENSE-MIT`、`LICENSE-APACHE` 两份原文 + 本文件 + `main/LICENSE`。
2. **LGPL-2.1 的 `rawler`** 以独立 DLL 形式动态链接，满足其条款的前提是该 DLL 的可获取性与再分发声明；
    分发 `raw_<ver>.dll` 时须在本文件保留本行并提供对应源码获取方式。
3. **`AGPL-3.0` 的 `zenavif`（OR 商业许可）**：本项目整体为 AGPL-3.0，按其 AGPL 分支使用；
   若未来改变项目许可证，须先复核该组件（届时许可证列必须同步更新本文件）。
4. ✅ **[待核] 已清零（2026-10-21 m76）**：表格里的 **20 个 [待核] 单元格、覆盖 32 个 crate**
   全部按仓内既有方法补核完——取的是**已发布包自己的 `Cargo.toml` 的 `license` 字段**
   （`cargo metadata --format-version 1`，解在 `registry/src/` 的那份原文，不信 README、不信 crates.io 页面）。
   三处旧推定被实读值推翻（`libloading` = ISC、`embed-resource` = 单 MIT、`rgb` = 单 MIT），
   已在行内就地写明。两份 workspace 的完整依赖图共 **469 条包记录**（main 231 / RuPix-Decoder 238，
   按逐条口径而非按包名聚合），**`license` 字段为空的 = 0 条**，所以本表没有「只能空着」的行。
   ★ 复跑：`python .temp/m76/license_audit.py`（一次性区，不在仓里；改依赖后照 §四 第 5 条重跑并更新本文件）。
5. **触发本文件更新的时机**（见 `DOCS_MANIFEST.md` 第一节末行）：新增 / 移除 / 更换依赖、
   依赖版本变化、windui 补丁落地、分发方式变化。
6. **★ UnRAR 许可（RAR / CBR 支持，m297）**：`unrar` crate 把 RARLAB 的 UnRAR C++ 源码
   vendored 编进二进制。源码自身 `license` 字段写的是 `MIT OR Apache-2.0`（那是 Rust 外壳），
   **真正约束来自 UnRAR 源码的许可**，其原文要求**随副本保留下面这段**（"or in documentation
   if license is not available"——本文件就是那个 documentation，所以这段必须原样留在这里）：

   > UnRAR source code may be used in any software to handle RAR archives without limitations
   > free of charge, but cannot be used to develop RAR (WinRAR) compatible archiver and to
   > re-create RAR compression algorithm, which is proprietary. Distribution of modified UnRAR
   > source code in separate form or as a part of other software is permitted, provided that
   > full text of this paragraph, starting from "UnRAR source code" words, is included in
   > license, or in documentation if license is not available, and in source code comments of
   > resulting package.
   >
   > —— Copyright (c) Alexander Roshal

   **本项目只解包、不打包**：RAR 解压被明确允许（免费、无限制，含商业软件）；被禁止的是
   拿它做 RAR 兼容的压缩器或重造压缩算法。若哪天有人想加 RAR **压缩**功能，先读这一条。

> 版本读取日期：**2026-10-12**（新增 `embed-resource` 行时重读 `Cargo.lock`：3.0.12）。下次改动依赖后重读两份 `Cargo.lock` 再更新本文件，禁止沿用旧数字。
> **2026-10-21 m76 只重读了许可证列**（§四 第 4 条那 32 个 crate）——顺带证明这些行的版本号自 2026-10-12 起未漂移；版本号本身没有重新全量实读，别把这句当成"全表版本都是 10-21 的"。
