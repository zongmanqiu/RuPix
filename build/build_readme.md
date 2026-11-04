# RuPix 构建流水线（main/build/）

> `main\build\` 是**源码**（属于项目树）；发布的二进制去 `<项目>\.build\`——上一级、仓库外。
> 一切编译中间产物在 `<项目>\.temp\`（`main\.cargo\config.toml` 与 `RuPix-Decoder/workspace/.cargo/config.toml`
> 分别钉死 `target-dir`，写在 `[package]` 下的 `target-dir` 是无效键）。

## 六步流水线（`full.bat` 一键编排，失败即停）

| 步 | 脚本 | 干什么 | 失败行为 |
|---|---|---|---|
| ① | `1_check_env.bat` | 只读工具链体检：cargo/rustc host 必须 msvc；python/powershell 必须在；`.3rd/windui` 缺失 = ERROR（本地副本，无下载步可修复） | ERROR → exit 1 |
| ② | `2_scan.bat` | **五条静态门禁**：`layering_scan.py`（五层方向 + 禁引 crate）+ `hygiene_scan.py`（硬编路径/调试残留/未完成标记/FFI 边界无解/生产区 panic 源）+ `cargo fmt --check`（宿主格式）+ `cargo clippy --all-targets --release -- -D warnings`（第一方 lint）+ `structure_gen.py --check`（结构文档的数字与符号索引必须等于 `src/` 现状）。违规即红；fmt 步 2026-10-17 补入——§九 早就写明要 exit 0，但此前无任何脚本执行，漂移无人拦；结构文档门禁同日补入，因为它的手工计数已经漂到 46/18,234（磁盘 45/19,108）；clippy 步 2026-10-19 补入，同一个"列而不行"毛病，且**必须带 `-D warnings`**（不带时 clippy 有 warning 也退 0，见 K69） | exit 1 |
| ③ | `3_build_ext.bat` | `RuPix-Decoder/build.ps1`：**fmt 检查** + 编译 + 测试 + 发布 DLL 到仓库外 `../.temp/decoder-dist/`（并同步 `.build/decoders`）（格式或测试失败不发布） | exit 1 |
| ④ | `4_build.bat` | `cd main` + `CARGO_INCREMENTAL=0` + `cargo build --release`；不拷贝任何东西 | exit 1 |
| ⑤ | `5_test.bat` | **真门禁**两层：`cargo test --release`（集成，含 docs_consistency）→ `--lib`（单元）；均 `--test-threads=1`（杀软锁新链接二进制会假死） | 任一层失败 → exit 1 |
| ⑥ | `6_deploy.bat` | `deploy.py`：**两道 PE 硬门禁**（RT_GROUP_ICON 档位表 + RT_VERSION 键与版本值须等于 `Cargo.toml`）→ exe 原子换名部署 + MD5/SIZE 对账 + 符号归档 + 解码器清单核对 + 随包 `docs/decoders.json`（从解码器仓 `decoders.json` 复制）；**解码器或本地清单缺失 = 不可分发 = exit 1**。★ 这两道不可省：两份 `.rc` 都走 `manifest_optional()`，资源编不进只降级成一条 cargo 警告（K70） | exit 1 |

环境开关：`RUPIX_SKIP_TESTS=1`（跳过第⑤步）/ `RUPIX_CARGO_ARGS`（第④步附加参数）。

工具链：`_common.bat` 在 `RUSTUP_TOOLCHAIN` 未定义时钉 `stable-x86_64-pc-windows-msvc`（本机 rustup
**没有默认工具链**，K31；不覆盖调用方已有的值，也不改全局 `rustup default`）。第①步原先裸调 cargo
会抓到空版本串仍打 `[OK]`、再判非 MSVC 而 rc=1——现在空串一律 `[ERROR]`。

`build.bat` 是历史兼容入口，现等价于 `full.bat`。

## 两条静态体检（只读，可单独跑）

```
python build/layering_scan.py [--json]    # 五层依赖方向 + 各层禁引 crate
python build/hygiene_scan.py [--json]     # 五查：绝对路径 / 调试残留 / TODO 真标记 / FFI 边界无解 / 生产区 panic 源（§八.1 红线，豁免要就地写 `红线豁免:`）
```

- **分层规则**（`layering_scan.py` 的 `LAYER_ORDER` / `BARRED` 表是机器判定的唯一来源，
  与 `AGENTS.md` 第三节、`目录结构说明.md` 三方一致）：
  - 方向：`core(0) → platform(1) → decoders(2) → ui(3) → runtime(4)`，低层不得 `use` 高层。
  - 禁引：core 禁 `windui`/`windows`；platform 禁 `windui`；decoders 禁 `windui`/`windows`；ui 禁 `windows`。
  - 禁引检查覆盖 `use X::` 与**裸路径** `X::`（注释行豁免）——裸路径检查是本项目对
    CurSeen 的补强：`main.rs` 曾直调 `windui::…::svg_warm_fonts` 而无 use 语句。
- **异味规则**（cfg(test) 之后按测试桩豁免；生产代码在前、`mod tests` 在文件末尾是**硬性**惯例）：
  - `src/main.rs` 对 stdout 打印有唯一豁免（`--probe-decode` 探针的存在意义就是打印）。

## 体积账（只读，按需跑）

```
python build/size_map.py groups        # 逐 crate × PE 段 + 分类合计 + 闭合检查
python build/size_map.py meta          # .rdata 的成分（字符串/单步状态表/展开元数据）
python build/size_map.py top usvg      # 某 crate 的符号级前 30
```

它读的是**带 `/MAP` 的那一次构建**（独立 target，不动 `.build` 的现行产物）：

```
cd main && set CARGO_TARGET_DIR=..\.temp\target-map
set RUSTFLAGS=-C target-feature=+crt-static -C link-arg=/MAP && cargo build --release --bin rupix
```

★ 两条硬口径（错了就得到一本假账，详见 K71）：`RUSTFLAGS` 是**整条覆盖** `.cargo/config.toml`
的 rustflags、不是追加，所以 `+crt-static` 必须一起写上；MSVC 的 map **不给符号长度**，
只能用"同段下一条地址 − 本条"，故每段都要打"归账 vs PE 段实测"的闭合差自证。
用途：核验 `docs/产品定位与边界.md` §4.1 的体积账。**不进门禁**——目标值 ≤4MB 本身待裁定。

## 按需测试分组

```
python build/gate.py list        # 分组表
python build/gate.py run config  # 只跑 config 组（libtest 子串过滤）
python build/gate.py run all
```

## 真机探针（tests/probes/）

真硬件/真 DLL 探针放 `tests/probes/`：Cargo **不递归发现** `tests/` 子目录，
不显式登记 `[[test]]` 就完全不参与默认编译——省的是编译时间（实测差 26 秒，
一个断言都没跑）。需要时临时登记再跑，跑完还原。

## 翻页取证驱动（`build/flip_probe.ps1`）

```
powershell -File build/flip_probe.ps1 -Interval 1200 -Presses 8 -Tag <臂名> [-Level prod|debug]
                               [-Private] [-Prefetch 0|1] [-CloseWhileBusy] [位置参数见文件头]
```

它启动**自己那一枚**实例、只往自己的 HWND PostMessage、收尾走 `WM_CLOSE`（不 kill，否则
会话戳会被写成"异常退出"），并只读**本轮追加**的日志字节。三个开关就是三种臂身份：

| 开关 | 现场 |
|---|---|
| `-Level prod` | 清掉 `RUST_LOG` / `RUPIX_DECODE_TRACE` ⇒ 抓下来的就是用户机器上会留下的那份（默认档 `info`，见 `platform::logging`） |
| `-Private` | **私有取证根**：`.temp\probe-<tag>\` 里放一枚 exe **硬链接** + 钉死的 `rupix.ini` + 自己的 `temp\` 日志根，解码器仍指 `.build\decoders`（`RUPIX_DECODERS_DIR`） |
| `-Prefetch 0` | 每次翻页必然现解 ⇒ 才造得出"关窗落在解码中间"这种现场 |
| `-CloseWhileBusy` | 跳过排空循环，最后一按立刻关窗 |

★ **为什么 `-Private` 是默认该用的那一档**（K84）：`.build\rupix.ini` 与
`.build\temp\rupix-<今天>.log` 是**运行期的可变态**，本机还有别的自动化会同时开实例、
改配置、清目录——实测过一次"臂运行期间 ini 被翻、当天日志从 66 KB 变成 7.8 KB"。
读数的臂身份必须包含**它跑在哪份配置上**，共享目录里的读数只能当"现场发生过什么"，不能当对照。
收尾报的 `EXITED=… exit_ms=…` 就是这条链路的产出（关窗 → 进程消失要多久，粒度 20 ms）。

## 崩溃取证驱动（`build/crash_probe.ps1`）

```
powershell -File build/crash_probe.ps1 -Tag <臂名> -DecodersDir <.temp 下的夹具目录> -Image <某张图>
```

让**发布版** `rupix.exe` 真崩一次，然后收它留下的报告与 dump（m68）。它不靠产品里的
"造崩开关"（刻意不提供），而是把一枚测试专用 DLL 经 `RUPIX_DECODERS_DIR` 送进进程——
这与"第三方解码 DLL 乱来"是同一条真实入口。臂身份沿用 `-Private` 那套：私有 exe 硬链接 +
钉死 ini + 私有 `temp\`；超时**只 kill 自己启动的那个 PID**。夹具与证据都只在 `.temp\m68\`，
不进仓。解证据（m129 起**一个参数就够**）：`python build\dump_sym.py <temp\crash-<今天>.log>`
——脚本按报告里「PDB 配对」那行的指纹自己在 `.temp\symbols\` 里认符号，逐帧出 `函数 + 文件:行`；
认不出就说认不出并以退出码 2 收场，绝不拿错版本的 pdb 硬编函数名。旧写法
`<dump> <exe> [pdb 目录]` 仍兼容。`--symbols DIR` 可指定归档目录（默认 `<repo>\.temp\symbols`，
负向验证时指一个只放错指纹 pdb 的目录）。`ThreadNames` 流是紧凑 12 字节（`4 + n×12`），见 K93。

panic 那条通道另有对等探针：`cargo run --release --example panic_probe`（退出码 101），
它走 Rust `panic!`、不经过 SEH，两条通道互不覆盖——改这块时两条都要跑。

**解码器那一帧**（`crashplug.dll+0x1221` 这种）从 `.temp\symbols\decoders\<id>.pdb` 解——
由 `RuPix-Decoder\build.ps1` 在发布时归档（发布名是 `<id>_<ver>`，CodeView 名是编译期 `<id>.pdb`，
脚本自己换算，见 K137）。解码器 dll 不在 exe 旁边时用 `--decoders DIR` 指出它在哪
（夹具目录就是这种情形）。解不出来时会**分两种说**：`(no pdb archived for this decoder)`
= 该去归档，`(archived pdb is another build)` = 归档里那枚是另一版，不给行是对的。

## 浮条显隐取证驱动（`build/pill_probe.ps1`）

回答一个肉眼分不清的问题：底部浮条"在闪"到底是**整条显隐在翻**，还是**宽度随状态文本抖**。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File build/pill_probe.ps1 -Tag <臂名> `
    -Image <图> [-Samples 40] [-EveryMs 30]
```

做法：私有的 exe 根（hardlink 到 `.build\rupix.exe`，写一份 `minimal_mode=1` 的 ini，不动共享配置），
启动后等 3 秒，再按 `-EveryMs` 用 `PrintWindow(hwnd, memDC, 2)` 抓客户区，取浮条那一行上
`w/2±200、±100、0` 五个点的 ARGB，落 `.temp\pill_<臂名>.txt`。

**怎么读**：

| 观察 | 结论 |
|---|---|
| 五个点（至少中心 + 两侧）**同步**在"胶囊底色"与"图底色"之间跳 | 整条显隐在翻 → 查 `show_toolbar` 的三条来源谁在抢 |
| 只有边缘点跳、中心点恒定 | 宽度随文本变（浮条居中，两边一起移）→ 查 `info_cell` |
| 某点偶尔变成按钮/文字色 | 正常：采样点压在字形上 |

2026-11-02 的实测：修前 `FF101010`（无浮条）与 `FF1B1B1C`（胶囊底）交替、四同步；修后 40 帧
全是 `FF1B1B1C` 系，`FF101010` 出现 **0** 次。

## 常见坑（本机实测）

- **杀软锁新链接二进制**：多线程 `cargo test` 像假死——第⑤步强制 `--test-threads=1`。
- **cargo `os error 5` 拒绝访问**：重跑；`CARGO_INCREMENTAL=0` 可少一类噪声。
- **`.bat` 的 echo 半角括号必须 `^(`**：cmd 会提前闭块、静默改变块语义——
  输出像正常结束但流水线停在半路。写新 echo 行时留意。
- **PowerShell 前台 stdout 取不到 / `>` 写成 UTF-16**：要读输出就落文件再读。
