# RuPix 构建流水线（main/build/）

> `main\build\` 是**源码**（属于项目树）；发布的二进制去 `<项目>\.build\`——上一级、仓库外。
> 一切编译中间产物在 `<项目>\.temp\`：宿主这一侧由 `main\.cargo\config.toml` 的
> `[build] target-dir` 钉死（写在 `[package]` 下的 `target-dir` 是无效键，cargo 会警告并回落到
> `./target`）；解码器那一侧由它自己仓库里的 `.cargo/config.toml` 钉到同一个 `.temp\`，
> 宿主这边的脚本不替它做这件事。

## 六步流水线（`full.bat` 一键编排，失败即停）

| 步 | 脚本 | 干什么 | 失败行为 |
|---|---|---|---|
| ① | `1_check_env.bat` | 只读工具链体检：cargo/rustc host 必须 msvc；python/powershell 必须在；`.3rd\wind-ui-rust-0.23.0` 缺失 = ERROR（本地 path 依赖，无下载步可修复）；解码器 workspace 缺失 = ERROR。`.3rd\usvg-0.48.1` 缺失 = ERROR（`Cargo.toml` 的 `[patch.crates-io]` 那一份，同样没有下载步可修复） | ERROR → exit 1 |
| ② | `2_scan.bat` | **七道静态门禁**，按 `[1/7]`…`[7/7]` 逐道打点：`layering_scan.py`（五层方向 + 各层禁引 crate + decoders/extensions 禁互引）→ `hygiene_scan.py`（15 查，见下）→ `cargo fmt --check` → `cargo clippy --all-targets --release -- -D warnings` → `structure_gen.py --check`（结构文档的数字与符号索引必须等于 `src/` 现状）→ `dump_sym.py --selftest`（崩溃取证解析器自己的断言：发布名 → CodeView pdb 名、报告解析）→ **注释断链**（`cargo doc -D rustdoc::broken_intra_doc_links`，m282 补——前六道都只读代码、不读 doc comment，54 处断链因此长期无人发现）。违规即红。★ 三条判据的牙齿各在何处：fmt 与 clippy 要的是 exit 0，而 **clippy 不带 `-D warnings` 时有 warning 照样退 0**（那就是假绿，见 K69）；结构文档这一步只读，漂移的正解是 `--write` 重生成、**不是手改数字**（手工计数漂过一截，所以这七道里它是唯一由生成器写文档的门）；`dump_sym.py --selftest` 不需要符号归档与崩溃夹具，一秒内跑完，为的是"最后一环的工具不能只靠信任" | exit 1 |
| ③ | `3_build_ext.bat` | 调解码器仓库自己的 `build.ps1`：**fmt 检查** + 编译 + 测试 + 发布 DLL 到 `<项目>\.temp\decoder-dist\`（并同步 `.build\decoders`）（格式或测试失败不发布） | exit 1 |
| ④ | `4_build.bat` | `cd main` + `CARGO_INCREMENTAL=0` + `cargo build --release`；不拷贝任何东西 | exit 1 |
| ⑤ | `5_test.bat` | **真门禁**两层：`cargo test --release`（集成，含 docs_consistency）→ `--lib`（单元）；均 `--test-threads=1`（杀软锁新链接二进制会假死） | 任一层失败 → exit 1 |
| ⑥ | `6_deploy.bat` | `deploy.py`：**两道 PE 硬门禁**（RT_GROUP_ICON 档位表 + RT_VERSION 键与版本值须等于 `Cargo.toml`）+ **许可门禁** `license_gate`（`LICENSE` 全文与 `THIRD-PARTY-NOTICES.md` 必须随包落到 `.build\`，三问都查：在不在、够不够长（`LICENSE` ≥ 30,000 B、清单 ≥ 1,000 B）、标志串对不对（`LICENSE` 要见 `GNU AFFERO GENERAL PUBLIC LICENSE`，清单要写明 `AGPL-3.0`）——空壳与误拷别人的许可证都会拦下）→ exe 原子换名部署 + MD5/SIZE 对账 + 符号归档 + 解码器清单核对 + 随包 `docs/decoders.json`（从解码器仓库的发布清单复制）；**解码器或本地清单缺失 = 不可分发 = exit 1**。★ 这两道 PE 门禁不可省：两份 `.rc` 都走 `manifest_optional()`，资源编不进只降级成一条 cargo 警告（K70）——deploy 这两道是唯一的硬拦截点；m317 起 deploy 还跑一次 `rupix.exe --list-formats` 生成 `docs/formats.md` / `formats.json`，并用 `formats_gate` 校验版本 / 计数 / 至少一枚可用解码器 | exit 1 |

环境开关：`RUPIX_SKIP_TESTS=1`（跳过第⑤步）/ `RUPIX_CARGO_ARGS`（第④步附加参数）。

工具链：`_common.bat` 在 `RUSTUP_TOOLCHAIN` 未定义时钉 `stable-x86_64-pc-windows-msvc`（本机 rustup
**没有默认工具链**，K31；不覆盖调用方已有的值，也不改全局 `rustup default`）。第①步遇到
"cargo 只给出空版本串"一律打 `[ERROR]`——空串 = 工具链根本选不出来，不能让它带着 `[OK]` 走进后面五步。

`build.bat` 是留在原地的旧入口名，跑的就是 `full.bat` 这一条六步流水线。

## 发布包（六步之后，人工/发布脚本）

`full.bat` 只把产物落到 `.build\`；release 资产由 `release_pack.py` 从 `.build\` 生成：

```powershell
cd main
python build\release_pack.py --version 0.4.0
```

产出在 `../.temp/release-out/`，只有一个包：

| 产物 | 内容 |
|---|---|
| `RuPix_<ver>.zip` | `rupix.exe` / `LICENSE` / `THIRD-PARTY-NOTICES.md` / `README.txt` + `classic` / `common` / `modern` / `pro` / `raw` 5 枚 DLL，**每枚都带同版本 `<id>_<version>/` 许可文件夹** + `docs/decoders.json` / `plugins.json` / `MCP接入指南.md` |

- `doc`（PDF/PS/AI/DXF，约 26.7 MB）**不进包**；`texture` / `doc-legacy` 也不进。
  三者都在设置页「解码器 → 可下载」按需安装，包里的 `docs/decoders.json` 已经列全。
- 不带 `rupix.ini` / `recent.txt` / `reading.txt` / `cache/` / `temp/` / `plugins/` /
  样本图；也不带 `formats.md|json`——格式覆盖需要时跑 `rupix.exe --list-formats`。
- 解码器许可文件夹从 `.temp/decoder-packages/<id>_<ver>.stage/<id>_<ver>/` 取；缺任何
  一枚，`release_pack.py` 直接中止，不会生成"少一个文件夹"的包。
- 打包固定 7-Zip 标准 ZIP/Deflate（`-mx=9 -mtm=off -mta=off -mtc=off`）。
- **0.4.0 自动更新暂停**：0.3.x 更新器只接受四文件包，遇到 `decoders/` 会拒绝；本版不
  发布新的 `update.json`，0.3.x 用户手动下载。下一版先把更新器改成接受这个单包，再恢复
  自动更新；不再另造"更新信封"。

## 两条静态体检（只读，可单独跑）

```
python build/layering_scan.py [--json|--selftest]   # 五层依赖方向 + 各层禁引 crate + decoders/extensions 禁互引
python build/hygiene_scan.py  [--json|--selftest]   # 十三查，逐类见下表
```

两条都由第②步执行，也各自带 `--selftest`（合成样例命中验证——门禁的牙齿，`tests/docs_consistency.rs`
的 j7 / j8 跑的就是这两支）。判据都是**总数 = 0** 才算过，退出码 0 = 无违规、1 = 有违规。

- **分层规则**（`layering_scan.py` 的 `LAYER_ORDER` / `BARRED` 表是机器判定的唯一来源，
  与 `AGENTS.md` §一、`目录结构说明.md` §一 三方一致）：
  - 层序：`core(0) → platform(1) → {decoders(2), extensions(2)} → ui(3) → runtime(4)`，低层不得 `use` 高层。
  - 禁引（`BARRED`）：core 禁 `windui`/`windows`；platform 禁 `windui`；decoders 禁 `windui`/`windows`；
    **extensions 禁 `windui`/`windows`**；ui 禁 `windows`。
  - 同层禁互引（`FORBIDDEN_PEERS`）：`decoders ↔ extensions` 双向禁——扩展不参与解码路由，
    解码器也不调用扩展。
  - 禁引检查覆盖 `use X::` 与**裸路径** `X::`（注释行豁免）——裸路径那一路是对 CurSeen 的补强：
    正文里直接写 `windui::…::svg_warm_fonts` 而不带 use 语句的形态，use-only 扫描抓不到。
- **卫生十四查**（`hygiene_scan.py` 的 `CATEGORIES`，按它的固定顺序；打印与 `--json` 都按这十四档分别报数）：

| # | 分类名 | 判据 | 扫描域 |
|---|---|---|---|
| 1 | `abs_path` | 硬编码绝对路径 `[A-Z]:\`（换台电脑就失效） | `src/**/*.rs`（`#[cfg(test)]` 之后豁免） |
| 2 | `debug_macro` | 调试残留 `println!` / `eprintln!` / `print!` / `eprint!` / `dbg!` | 同上 |
| 3 | `marker` | 未完成标记 `TODO:` / `TODO(` / `FIXME` / `XXX`（只认真标记） | 同上 |
| 4 | `boundary_doc` | FFI 边界（`unsafe fn` / `extern "C" fn` / `#[no_mangle]`）上方没有紧邻注释 | 同上 |
| 5 | `panic_source` | 生产区 `unwrap` / `expect` / `panic!` / `todo!`（`AGENTS.md` §八.1 红线；就地豁免要写 `红线豁免:`） | 同上 |
| 6 | `config_last_mile` | 每个 ini 键既要有 **UI 入口**、也要有**行为读者**；`CONFIG_ONLY_KEYS` 登记者只豁免"入口" | `src/**` |
| 7 | `patch_parity` | 第三方补丁两问：`patches/PATCHES.md`（windui 卷）的每个 `## 补丁 NN` 章节 ↔ `.3rd/wind-ui-rust-0.23.0` 里的 `★ RuPix 补丁 NN` 标记对得上。★ 覆盖面：本查只认 windui 这一卷，`patches/usvg-0.48.1/PATCHES.md` 不在它的扫描范围里；"归档 ↔ 工作副本逐字节"那一格由集成门禁 j13 守 | `main/patches/PATCHES.md` + `.3rd/wind-ui-rust-0.23.0` |
| 8 | `control_char` | 文本里的不可见控制字符（只放行 09/0a/0d）——脚本改文件时反斜杠转义会吃掉一个字母，锚点静默损坏 | `main/**` 的第一方文本 |
| 9 | `ps1_ascii` | `.ps1` 必须纯 ASCII（PowerShell 5.1 按 ANSI 读无 BOM 文件，中文注释会坏到邻近语句上，K27） | `main/**/*.ps1` |
| 10 | `literal_color` | 字面色值 `Color::rgb(` / `Color::rgba(` 只许出现在 `ui/tokens.rs` | `src/ui/**` |
| 11 | `ui_super_path` | ui 层内跨模块引用一律经 `super::`，禁 `crate::ui::`（中枢在 `ui/mod.rs`） | `src/ui/**` |
| 12 | `secret` | 无密钥 / 无隐私：私钥块、访问令牌、URL 内嵌凭据、口令字面量、私人邮箱、含真名的本机路径 | `main/**` |
| 13 | `single_window_key` | 单例窗不许同一个键既 `close_window` 又 `single`（关窗是投递、开窗是即时，K196） | `src/ui/**` |
| 14 | `footer_buttons` | 自绘对话框的按钮只许走 `settings::rows::btn` / `dialog_footer::action`，`Element::button(` 不许散落（版式正本 `docs/界面版式规范.md`，2026-11-06） | `src/ui/**` |
| 15 | `settings_include` | `config.rs` 的 M4 用例（`ui_touchpoints_cover_all_keys`）里 `include_str!` 的 `../ui/settings/` 条目集 = 磁盘 `src/ui/settings/*.rs` 全集，双向判等（`include_str!` 不支持通配，漏列的那一页若恰好没有 ini 键写点注释，M4 用例永远查不到——`decoders.rs` 曾静默漏过；E3 下沉，2026-11-10） | `src/core/config.rs` + `src/ui/settings/` |

  豁免（第十三查以外的通用形状）：`#[cfg(test)]` 之后按测试桩处理（整篇是测试模块的 `tests.rs` 文件
  也算），**生产代码在前、`mod tests` 在文件末尾**是硬性惯例；`src/main.rs` 对 stdout 打印有唯一豁免
  （`--probe-decode` 探针的存在意义就是打印）。

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

## 探针脚本清单（`build/*.ps1`，**不参与 `full.bat`**）

`build\` 下现在有 **12 枚** PowerShell 探针，六步流水线一枚都不调用它们（被流水线调用的
`.ps1` 只有第③步里解码器仓库自己的 `build.ps1`，那是构建不是取证），
所以它们是**人工按需跑**的那一层：真机读数、红绿对照、验证一个肉眼分不清的现象。
共同的规矩写在各文件头里：`.ps1` 纯 ASCII（`ps1_ascii` 那查会拦）、私有 exe 根 + 钉死的 ini +
私有 `temp\`（`-Private` 那一族）、只关**自己启动的那一枚**子进程（先 `WM_CLOSE`，
万不得已才 `Stop-Process` 自己的 PID）。

| 脚本 | 它验什么（一句话） |
|---|---|
| `flip_probe.ps1` | 固定节奏翻页并只读**本轮追加**的日志字节：数 `[main-latency]` 的行数 = 上屏次数、数 `src=prefetch` = 预读命中（详见下一节） |
| `crash_probe.ps1` | 让**发布版** `rupix.exe` 真崩一次（经 `RUPIX_DECODERS_DIR` 送一枚测试专用 DLL），收它留下的崩溃报告与 `.dmp`，再交 `dump_sym.py` 解（详见再下一节） |
| `hang_probe.ps1` | 换一枚 `iv_render` 里睡 30 秒的解码 DLL：解码线程卡住时宿主**有没有话对用户说**（隔离栈只拦崩、不拦卡，这条只能实测） |
| `drag_probe.ps1` | `drag_image` 开关的效果：左键在画布里拖 → 像素变了而**窗口矩形不动**；再加一臂"按 `1` 原尺寸"当对照，防止把"没变化"误读成"抓帧死了" |
| `pan_probe.ps1` | WASD 平移端到端：竖向渐变图 + 亮度 oracle（W 看顶最暗、S 看底最亮、中间居中），一条序不对就是平移坏了 |
| `picker_probe.ps1` | 取色器：纯色图 + **系统剪贴板文本**当 oracle——`K` 进模式、左键复制该点 HEX、第二击不再复制、`K` 后 `Esc` 只退模式不关窗 |
| `rotate_probe.ps1` | `R` / `M` 在大图上让 UI 线程停多久（oracle = `SendMessageTimeout(WM_NULL)`：忙着的线程答不了 sent message） |
| `stall_probe.ps1` | 不按键也采样：开图、原图档落地、翻页各自把 UI 线程压住多久——整条时间线都量，不盯某一个键 |
| `startup_probe.ps1` | "双击 → 窗口出现 → 第一帧"这段的读数：秒表在 `Start-Process` 之前起，同时盯 Win32 窗口 oracle 与产品自己的日志行 |
| `pill_probe.ps1` | 底部浮条"在闪"到底是**整条显隐**还是**宽度随状态文本抖**：逐帧取那一行五个点的 ARGB |
| `key_dispatch_probe.ps1` | 一次按下的字符键触发几个动作：黑白交替、帧延迟 300 s 的 GIF 当夹具，配合 `RUST_LOG=trace` 分辨 WM_KEYDOWN 与 WM_CHAR 双派发 |
| `probe_tray_clear.ps1` | "关进托盘再唤醒 → 旧图必须已经清空"：唤醒不带文件应当是空窗（截图小）、带文件应当有图（截图大），且工作集与线程数不随轮次上涨 |
| `shell_menu_probe.ps1` | 右键「系统菜单」是不是**真转到 explorer 了**：按 `#32768` 找到原生菜单窗、按它自己的矩形抓屏看项名，再用真鼠标点「复制文件地址」，**剪贴板等于原路径**才算 `InvokeCommand` 的 id→verb 映射没歪（`-RowY` / `-SubY` / `-VerbY` 三段） |
| `multi_window_probe.ps1` | 多开是"每次启动一扇新窗"还是"就地换图"：私有根连开 N 次，数本进程顶层窗 + 读日志里上屏的文件 |
| `dll_serial_probe.ps1` | 两（三）扇窗迎面翻同一批图，`[dll-serial]` 记每笔的 `wait`/`run`；`-CloseMidway` 在解码在飞时关掉一扇，`-HoldSec` 让进程在驱动之后再活 N 秒（跨过看门狗 10 s 首报阈值，才验得了"没有假告警"）。`-Arm idle` = 第二扇窗只开不驱动（**用户真实场景**：一窗翻页、另一窗只是开着并在后台预取）；`-Prefetch 0/1` 开关邻图预读（默认 0 保持 m229/m230 的臂可复跑，要测真实体验必须给 1）；`-Bin` 指定被测二进制做 A/B（旧名 `-Exe` 会被脚本里的 `$exe` 顶掉，PS 变量名不分大小写） |
| `dll_serial_analyze.py` | 上一条的判据：持锁区间零重叠、等待上界随窗口数缩放、跨解码器重叠是**并行**的正面证据；退出码 0/1/2（2 = 没测到争用 ⇒ 结论不成立） |
| `raw_dialog_probe.ps1` | 多开时**每扇窗各弹一个**裸数据参数窗（两个无整数几何的 `.bgra` 造夹具，数顶层窗：主窗 + 各自的对话框） |
| `metadata_probe.ps1` | 同上，验属性窗：ini 里 `hotkeys=props:F2`，`PostMessage` 往每扇窗投 F2，数标题不是主窗标题的窗 |
| `decoder_inuse_probe.ps1` | 卸载一个**本会话已加载**的解码器会怎样：私有解码器目录只放 `common`，开着 `.jpg` 时把 DLL 送回收站（外壳真实路径，不是原地改名）⇒ 期望失败；关进程后再试 ⇒ 期望成功。附带一条方法论：原地改名**不是**回收站的替身 |
| `dir_rescan_probe.ps1` | 多窗盯同一个大目录时**一次文件变动的重扫成本**：造 N 项夹具、开 N 扇窗、加/删一个文件，读 `[dir-rescan]` 数 `scanned`（真读盘）与 `reused`（复用别人的扫描）。判定 `scanned` 恒为 1 才算同批只扫一次 |
| `tray_wake_probe.ps1` | 点托盘「显示窗口」（等价于再次启动不带参数的 `rupix.exe`）时，多开的窗口是不是**全部**回到前面：开 N 扇，拿一个无关窗口把它们盖住，读 z 序（同一次 `EnumWindows` 里算 index）。`-Bin` 指被测 exe（新旧对照要传两个不同的二进制，**不能**叫 `-Exe`：PS 变量名不分大小写，会被脚本里的 `$exe` 顶掉）；`-Cover 0` 改走"最小化 + 读 IsIconic"的老判据，`-SkipWake 1` 是不唤回的对照臂（证明恢复确实由唤回触发，而不是系统自己还原） |
| `multi_window_mem_probe.ps1` | 多开一扇窗要多少内存：同一张大图造 N 个硬链接（磁盘只占一份）、逐窗打开、读进程级 `PrivateMemorySize64` / `WorkingSet64` / `PeakWorkingSet64`，给出每窗边际增量。`-Burst 1` 让第 2..N 扇**不留间隔地连发**，解码才会真的撞在一起——峰值只有在这一臂才有意义（逐窗开时解码是串行的，峰值必然不涨）。★ 判据用私有提交与峰值，别用瞬时工作集：它随时可能被系统裁到 1 MB 级 |
| `fullscreen_corner_probe.ps1` | 全屏是不是**真全屏**的 DWM 属性判据：私有根起一窗（`init_window=0`），`DwmGetWindowAttribute(hwnd, 33)` 读回 `DWMWA_WINDOW_CORNER_PREFERENCE`（`2=ROUND` / `1=DONOTROUND`），投 F11 进全屏、再投一次退出，期望 `2 → 1 → 2`。`-Bin` 传旧 exe 做对照臂（补丁 026 之前全屏仍读回 `2` ⇒ FAIL）。★ 判据取**属性读回**而不是像素：圆角是 DWM 合成层裁切，截图会被壁纸 / 图片内容干扰；窗口按 PID 枚举（`FindWindowW` 按标题不可靠）；开头 `taskkill` 清场、结尾连 `temp/rupix.run` 一起删，全 ASCII。★ 探针另发一次**模拟顶缘拖动**并量窗口矩形，但新旧 exe 都读回 `0,0` —— 模拟输入没能触发系统移动循环，**该读数只作诊断、不参与 PASS/FAIL**；「全屏禁拖窗」由 `ui/canvas.rs::left_drag_never_moves_the_window_in_fullscreen` 单测钉住，视觉判据留实机 |

> `_common.bat` **不是探针**：它是六个编号步骤各自 `call` 的公共前置——算出
> `MAIN_DIR` / `PROJECT_ROOT` / `TEMP_DIR` / `TARGET_DIR` / `OUTPUT_DIR` / `DECODER_DIR` /
> `EXT_DIR` 这些位置，找 `python` 与 `powershell`，并在调用方没给时钉
> `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc`（本机 rustup 没有默认工具链，K31）
> 与 `RUSTFLAGS`（`+crt-static` 加 `--remap-path-prefix`，产物字节里不留本机路径）。
> ★ `RUSTFLAGS` 是**整条覆盖** `.cargo/config.toml` 的 rustflags 而不是追加，所以手工跑
> cargo 时这两条得自己带上（与上面《体积账》那条硬口径同一条道理）。

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
由解码器仓库自己的 `build.ps1` 在发布时归档（发布名是 `<id>_<ver>`，CodeView 名是编译期 `<id>.pdb`，
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
