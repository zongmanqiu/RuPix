r"""

用法：
  python build/hygiene_scan.py            # 分类计数 + 违规清单 + 汇总
  python build/hygiene_scan.py --json     # 机器可读
  python build/hygiene_scan.py --selftest # 规则自检（合成样例命中验证，门禁的牙齿）

十四查（1–6 只扫第一方 Rust 源码 `main/src/**/*.rs`；第 7 查按登记要求跨到 `main/patches/` 与 `.3rd/`，**不碰归档**；
第 8 查扫 `main/**` 的全部第一方文本文件；第 9 查只扫 `main/**/*.ps1`；第 10、11、13、14 查只扫 `main/src/ui/**`；
第 12 查扫 `main/**` 找凭据形状）：

  1. 硬编码绝对路径  `[A-Z]:\`  —— 换台电脑就失效
  2. 调试残留        `println!` / `eprintln!` / `print!` / `eprint!` / `dbg!`
  3. 未完成标记      `TODO` / `FIXME` / `XXX`（只认 `TODO:` / `TODO(` 这类**真标记**）
  4. FFI 边界无解    `unsafe fn` / `extern "C" fn` / `#[no_mangle]` —— 上方没有
                     紧邻的 `//` / `///` 注释即违规。跨边界调用不写"为什么这样是
                     安全的 / 谁负责生命周期"，下一次改动就没人接得住这条条约定。
  5. 生产区 panic 源 `AGENTS.md` §八.1 的红线（禁 `unwrap` / `expect` / `panic!` /
                     `todo!`）**此前只是文档承诺，没有任何机器执行**（审计 V9）。
                     现在由本查落地：命中即违规，除非上方 3 行内有 `红线豁免:` 标注。
  6. 设置键的"最后一公里"（m121）：`ALL_KEYS` 里每个键在 `load_from` 的臂上解到的 `cfg.<字段>`，
                     既要有**用户入口**（`src/ui/**` 的非注释生产行引用它），也要有**行为读者**
                     （`src/ui/settings/**` 之外、且不是 `src/core/config.rs` 自身的非注释生产行引用它）。
                     ★ **配置专用键**（2026-10-29 用户口径）豁免"用户入口"：高级用户手改 ini、
                     界面刻意不出现。名单 = `src/core/config.rs` 的 `CONFIG_ONLY_KEYS`
                     （与 Rust 单测 `ui_touchpoints_cover_all_keys` 读同一张表）；
                     **行为读者仍必须有**——只豁免入口，不豁免"改了得有人读"。
                     既有守卫（`saved_keys_match_all_keys` / `every_key_has_a_load_consumer`）只证到
                     `ini → 字段`；**"字段落在谁手里"此前没有任何机器读过**——解得开、写得了盘、
                     设置页也画得出来却没人看的键，就是 K116"无读者＝假数据源"的用户面同族。
                     一条兜底：`ALL_KEYS` 或解析臂没数出东西时也报（本查不许空跑成"全部通过"）。
  7. 第三方补丁两问（m123）：`main/patches/PATCHES.md` 的每个 `## 补丁 NN` 章节，与 `.3rd/wind-ui-rust-0.23.0`
                     里的 `★ RuPix 补丁 NN` 标记，两两对得上：① 章节正文没写 `落点状态：0.23.0 无落点` 就必须
                     有标记（缺＝补丁丢了，或标记写法漂了）；② `.3rd` 出现的号必须有章节（多＝改了上游没记账）。
                     登记簿原本那句"与本章节一一对应"此前没有任何机器读过；m123 实测 17 个章节里 2 个
                     （02/05）在 0.23.0 真没有落点（02 由上游内建、05 被补丁 014 取代），原因已写进各自正文。
                     ★ **第三处（`patches/wind-ui-rust-0.23.0/` 归档 ↔ 工作副本逐字节）不在本查**：它已由
                     `tests/docs_consistency.rs::j13_patch_archive_matches_worktree` 守着（实测比 16 份、硬门禁）——
                     同一判据立两个定义点＝改一个漏一个（`core/pixels.rs` 头注释记的那次 MAX_PIXELS 事故同形），
                     所以本查只补上面那两问，j13 那边补的是"工作副本缺文件也算漂移"。任何一侧读不到都算违规（防失明）。
  8. 不可见控制字符（m124）：`main/**` 的第一方文本文件（`.md` `.rs` `.py` `.bat` `.ps1` `.toml` `.json`）里
                     不许出现控制字符——放行制表 09、换行 0a、回车 0d（本仓有 CRLF 的 md），其余 00–08/0b/0c/0e–1f
                     一律报。**起因是实测抓到一枚**：`DOCS_MANIFEST.md` 那格本该写反斜杠加 build.ps1，文件里实际
                     是 `extension` + 0x08（退格）+ `uild.ps1`——脚本改文件时那对转义把字母 b 吃掉了，剩下一个
                     谁也看不见的退格，**路径锚点当场静默损坏、而所有"按名字核对"的门禁都读不到它**。
                     这条判据零假警报形状（正常文本不该有控制字符），所以直接做硬门禁；一个文件都没扫到＝本查失明。

豁免：
  - `#[cfg(test)]` 之后的代码是测试桩：允许硬编路径与往 stdout 打印。
  - **纯注释行**里出现的调试宏字样（文档性提及）不算调用；同理 TODO 只在注释里才有意义。
  - `src/main.rs`：`--probe-decode` 隐藏探针的**存在意义就是往 stdout 报结果**，
    它是命令行工具面而非 GUI 日志（CurSeen 同款逻辑给了 logger.rs 唯一豁免，此处同理）。
  - `panic_source` 的**第三类例外**（§八.1）：确实不能降级的 fail-fast（如"解码线程起不
    来 = 这个程序没有第二种活法"）在违规行上方写一行 `// 红线豁免: <为什么>`。标记必须
    就地、可 grep、带理由——不是基线白名单：0 条豁免与 20 条豁免都能一眼看见。
  - `boundary_doc` 只扫 `main/src/**`——`RuPix-Decoder/**` 里的 `#[no_mangle] pub extern "C"`
    导出遵循规范文档里的**统一模板**（`abi` 层 `guard()` 一穿到底），签名本身即契约，
    不属于本仓的"边界"（属于解码器作者与规范文档的对话）；如需扫 RuPix-Decoder，请
    在 `RuPix-Decoder` 目录内单独建同款脚本，不要混进本文件的第一方域。

九查 `ps1_ascii`（2026-10-25，K27 第二次咬人）：`main/**/*.ps1` 必须是**纯 ASCII**。
  这不是风格偏好，是 PowerShell 5.1 的解析规则——无 BOM 的 `.ps1` 被按 **ANSI（活动代码页）**
  读取，于是一段中文注释会被解成与写入时不同的字节；真正要命的是**坏的地方离病灶很远**：
  错解的字节可能吃掉引号或换行，报错落在邻近那条看起来完全无辜的语句上（本轮实测：符号
  归档那块上方写了中文注释，报错是三行之外 `Test-Path` 的 `-Path` 绑到了空值，而消息把它
  记在 `ForEach-Object` 头上）。同期在 `RuPix-Decoder` 侧 `build.ps1` 上**原样复发一次**，
  那边已把同一条判据装进 `scripts/check_docs.py`。中文注释写进 `.rs` / `.md` 没问题，
  `.ps1` 不行。

十查 `literal_color`（2026-10-26，m143；对标 CurSeen 的 `ui/tokens.rs`）：`main/src/ui/**/*.rs`
  里**不许出现字面色值** `Color::rgb(` / `Color::rgba(`，唯一例外是 `src/ui/tokens.rs`——
  它本身就是定义处。起因是实测：设计令牌原本是**设置窗私有的** `mod tokens`，于是画在图片上的
  那几个界面（顶部胶囊 `chrome` / 缩略图条 `filmstrip` / 鸟瞰 `canvas`）只能各自另造一套浮层色，
  同一块深灰 `28,28,30,225` 抄了两遍、按下白 `255,255,255,44` 抄了两遍、窗口底 `15,15,15`
  在 `canvas` 与 `window` 里各写一遍——**改一档配色必然改漏**。收进 `ui/tokens.rs` 后，
  换肤入口只剩一个文件，本查把这条约定钉成硬门禁（放行 `to_color(TOKEN)` 这类引用，
  它们不带 `Color::` 前缀，不会误报）。

十一查 `ui_super_path`（2026-10-26，对标 CurSeen 的"父 mod.rs 做中枢"）：`main/src/ui/**/*.rs`
  的**非注释行**不许出现 `crate::ui::`。ui 是**平铺**的十个模块，兄弟之间一直写全路径，
  依赖就长成一张网：谁用了谁只能 grep、改个模块名要扫四处、删掉一个依赖也看不出还有没有人用。
  一律改经 `super::` 之后，`src/ui/mod.rs` 的「层内中枢」块就是这一层的**内部 API 清单**
  ——删掉那里一行 `use`，编译立刻指出还有谁在用。实测收口前 ui 内 31 处 `crate::ui::`
  （13 条 use + 18 处代码/注释），收口后 0。注释行豁免：文档里的 [`crate::ui::tokens`] 是
  **绝对链接**（跨层可点），改成 `super::` 在 rustdoc 里反而指不进去。

十二查 `secret`（2026-10-26，用户口径「两个目录都要能原样公开」）：`main/**` 里不许出现
  私钥块、访问令牌、URL 内嵌凭据、口令字面量、私人邮箱、含真名的本机路径。判据刻意窄
  （只认高置信写法，`password: self.config.password` 这类赋值不算）——一条天天误报的门禁
  一定被人加白名单加到失效。三方署名里的上游邮箱豁免（`THIRD-PARTY-NOTICES.md` / `LICENSE`，
  法律要求归属）。解码器仓的同源判据是它自己的 `scripts/check_secrets.py`（两仓各自自包含）。

十三查 `single_window_key`（2026-11-04，K196）：`src/ui/**` 里同一个文件若
  `close_window(X)` 与 `.single(Y)` 用了**同一个变量**，即同键既关又开。windui 的关窗是
  投递到应用级旁路队列（下一次定时器/热键收尾才按 key 找窗投 `WM_CLOSE`），开窗是窗口
  回调收尾时立即建窗——那条滞留的关窗请求会找到刚开出来的新窗把它收走，用户看到
  「窗闪一下就没了」。正确写法是 `singleton::recycle` 给出的两个不同键。

十四查 `footer_buttons`（2026-11-06，用户点名）：`main/src/ui/**` 的非测试代码里，
  `Element::button(` 只许出现在 `settings/rows.rs`（`btn` 的实现本身）。其余一律走
  `btn(...)` 或 `dialog_footer::action(...)`，整扇对话框走 `dialog_footer::shell`。
  起因是三扇对话框各写一套四边留白 / 行间距 / 按钮位：插件设置窗 `spacing(0)` 再在按钮行
  加一层 16、裸数据参数窗 `spacing(10)` 且按钮不贴底、恢复提示窗 `spacing(10)` 自己顶一颗
  spacer——用户看到的是「通道调色的快捷键窗按钮和其它插件不一样」。版式正本见
  `docs/界面版式规范.md` §2：四边 16（`DIALOG_PAD`）、行间距与按钮间距 8（`GAP`）、
  行高 40（`ROW_H`）、按钮右对齐且贴底。

分类计数：违规按 `abs_path` / `debug_macro` / `marker` / `boundary_doc` / `panic_source` /
`config_last_mile` / `patch_parity` / `control_char` / `ps1_ascii` / `literal_color` /
`ui_super_path` / `secret` / `single_window_key` / `footer_buttons` 十四档分别报数（见 [`CATEGORIES`]）。**判定门限仍是「总数 = 0」**——分类只为趋势可见（哪一类在动），
不引入基线冻结；RuPix 违规历史 = 0，加基线等于给满分项目开小抄通道。
退出码：0 = 「违规 0 处」；1 = 有违规。

本脚本只做静态字符串扫描；`#[cfg(test)]` 之后的代码统一按测试桩处理——
名为 `tests.rs` 的**文件级测试模块**（父模块里 `#[cfg(test)] mod tests;`）整篇按测试桩处理。
本项目惯例是「生产代码在前、`mod tests` 在文件末尾」（硬性，见 AGENTS.md）。
"""
import json
import pathlib
import re
import sys

ROOT_DIR = pathlib.Path(__file__).resolve().parent.parent  # <main>/
SRC = ROOT_DIR / "src"

# `#[cfg(test)]` 之后 = 测试区。
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
# 整个文件都是测试代码的模块：父 `mod.rs` 里声明成 `#[cfg(test)] mod tests;`，
# 文件自己不带 `#[cfg(test)]`（Rust 没法给整个文件模块加"仅测试编译"的内联属性）。
# 2026-10-26 拆 `decoders/` 时首个落地——不认它，`tests.rs` 里的 `.unwrap()` 会被
# 当成生产代码的 panic 源误报。判据只看文件名，不看内容（内容一放开就变成"想绕就绕"）。
TEST_ONLY = re.compile(r"(?:^|/)tests\.rs$")
# 硬编码绝对路径（盘符 + 反斜杠）。只匹配**一个**反斜杠，于是同时覆盖
# `"C:\\x"`（Rust 转义写法）与 `r"C:\x"`（raw string）两种源码形态。
ABS_PATH = re.compile(r"[A-Za-z]:\\")
# 调试宏调用。
DEBUG_MACRO = re.compile(r"\b(?:println|eprintln|print|eprint|dbg)!\s*[\(\[]")
# 真·未完成标记：半角冒号或左括号紧跟（排除「这里以前是 TODO：…」这类全角叙述）。
MARKER = re.compile(r"\b(?:TODO|FIXME|XXX)\s*[:(]")
# 第 10 查：ui 层的**字面色值**。命中形如 `Color::rgb(` / `Color::rgba(` 的直接构造，
# 放行 `to_color(OVERLAY_X)` 这类令牌引用（它们不带 `Color::` 前缀）。
LITERAL_COLOR = re.compile(r"Color::(?:rgba|rgb)\s*\(")
# 第 11 查：ui 层内跨模块引用。平铺模块之间直接写 `crate::ui::<兄弟>::` 会让依赖长成网，
# 一律经父模块（`use super::{Chrome, Hub}` / `super::<模块>::名字`）——中枢在 src/ui/mod.rs。
UI_SIBLING_PATH = re.compile(r"crate::ui::")
# 十三查：关窗与开窗各自吃的那个键。只认裸标识符——`close_window(win_key.clone())`
# 与 `.single(win_key)` 取的正是同一个名字，这正是要拦的写法。
CLOSE_WINDOW_ARG = re.compile(r"\bclose_window\(\s*([A-Za-z_][A-Za-z0-9_]*)")
SINGLE_WINDOW_ARG = re.compile(r"\.single\(\s*([A-Za-z_][A-Za-z0-9_]*)")

# 第 12 查：两仓都要能原样公开（2026-10-26 用户口径）——密钥 / 口令 / 私人联系方式 /
# 含真名的本机路径，一条都不许进源码目录。宁可少报也不误报到没人看：只认高置信写法。
SECRET_PRIVATE_KEY = re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")
# 前缀必须带够长的实体才算：只写 `glpat-` 三个字的文档不该被当成令牌（而且真实令牌
# 至少 20 位）。这条收紧是负向验证过之后改的——先命中过 AGENTS.md 自己的规则正文。
SECRET_TOKEN = re.compile(
    r"\b(?:ghp_[A-Za-z0-9]{20,}|gho_[A-Za-z0-9]{20,}|ghu_[A-Za-z0-9]{20,}"
    r"|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_-]{16,}|xox[baprs]-[A-Za-z0-9-]{10,}"
    r"|AKIA[0-9A-Z]{16}|sk-[A-Za-z0-9]{20,})")
SECRET_URL_CREDS = re.compile(r"\b[a-z][a-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@")
# 值必须是**带引号的字面量**才算：`password: self.config.password` 这种赋值不算，
# 但 `password = "hunter2"` 算。
SECRET_CRED_ASSIGN = re.compile(
    r"(?i)\b(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)\b"
    r"\s*[:=]\s*[\"'][^\"']{8,}[\"']")
SECRET_EMAIL = re.compile(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b")
SECRET_USER_PATH = re.compile(r"[A-Za-z]:\\\\?Users\\\\?([A-Za-z0-9._-]+)")
# 三方署名里的上游邮箱是法律要求的归属信息，不是我们的隐私。
SECRET_EMAIL_OK = {"THIRD-PARTY-NOTICES.md", "LICENSE"}
SECRET_USER_OK = {"Someone", "someone", "user", "User", "username", "USERNAME", "Public", "Default"}
SECRET_SKIP_SUFFIX = (".dll", ".pdb", ".exe", ".lib", ".obj", ".png", ".jpg", ".jpeg", ".gif",
                      ".bmp", ".tif", ".tiff", ".webp", ".qoi", ".ico", ".psd", ".hdr", ".exr",
                      ".dcm", ".ktx2", ".jxr", ".jp2", ".rlib", ".rmeta")
# `.git` 必跳（2026-11-02 接入 git 后加的）：`config` / `logs` 里必然有提交者邮箱，
# 而 `.git` 从不参与提交——扫它只会把"git 正常工作"报成泄漏。同一文件的
# `CTRL_SKIP_SEGS` 早就列了 `/.git/`，secret 这一查当时漏了。
SECRET_SKIP_DIRS = {"patches", ".temp", ".build", "target", "__pycache__", ".git"}
# 本文件自身豁免：判据的正则与"应当命中的示例串"都写在它里面（自检用），不跳过就自报违规。
SECRET_SELF = pathlib.Path(__file__).resolve()
# FFI 边界项：`unsafe fn` / `extern "C" fn` / `#[no_mangle]`。
# 匹配行首（可有 pub / pub(crate) / vis 修饰），排除类型定义里的 `unsafe extern "C" fn(...)`
# 函数字段类型——那些是"引用一个外部函数"，不是"定义一个必须解释安全性的边界"。
BOUNDARY_ITEM = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:unsafe\s+)?extern\s+\"C\"\s+fn\s+\w+|unsafe\s+fn\s+\w+)\s*[<(]"
)
BOUNDARY_NO_MANGLE = re.compile(r"^\s*#\[no_mangle\]\s*$")
# 回看时跳过的行：属性（`#[cfg(...)]` / `#[derive(...)]` / `#[repr(...)]` 等）与空行。
ATTRIBUTE_LINE = re.compile(r"^\s*#\[")
# 生产区 panic 源（§八.1 红线）：`.unwrap(` / `::unwrap(` / `.expect(` / `::expect(` 与
# `panic!` / `todo!` / `unimplemented!` / `unreachable!`。
# 刻意**不**用裸 `\bunwrap\b`：`unwrap_or` / `unwrap_or_else` / `unwrap_or_default` 与
# `expect_err` 都是不 panic 的写法，误伤只会把人推向"干脆关掉门禁"。
# 左边要求 `.` 或 `::`，于是 `if expect != None`（变量名）这类不会命中。
PANIC_SOURCE = re.compile(
    r"(?:\.|::)\s*(?:unwrap|expect)\s*[\(:]|\b(?:panic|todo|unimplemented|unreachable)\s*!"
)
# 红线豁免标注：写在必经该行的上方（见模块头「豁免」）。带冒号才算标记，同 MARKER 口径。
EXEMPT_MARK = re.compile(r"^\s*(?://|/\*|///|//!).*红线豁免\s*[:：]")

# 允许直接往 stdout/stderr 写的模块（探针命令面，见模块头）。
STDOUT_EXEMPT = {"src/main.rs"}

# ── 六查（m121）：设置键的"最后一公里" ────────────────────────────────────────────
# 已有守卫只证到 `ini → 字段`（`saved_keys_match_all_keys` 管键集合、
# `every_key_has_a_load_consumer` 管解析臂）。**"字段落在谁手里"没有任何机器读过**：
# 一个键解得开、写得了盘、设置页也画得出来，但全工程没有任何一帧看它 ⇒ 用户改了个寂寞——
# K116"无读者字段＝假数据源"的用户面同族，而且是**面向用户**的那一面，更贵。
ALL_KEYS_DECL = re.compile(r"pub const ALL_KEYS: \[&str;\s*\d+\s*\] = \[(.*?)\];", re.S)
ARM_KEY_NAME = re.compile(r'^\s*"([a-z_0-9]+)"\s*=>')
# 第 6 查的例外：`core/config.rs` 的 `CONFIG_ONLY_KEYS`（高级用户手改 ini，界面刻意不出现）。
# 与 Rust 单测 `ui_touchpoints_cover_all_keys` 读的是同一张表——只定义一处。
CONFIG_ONLY_DECL = re.compile(
    r"pub const CONFIG_ONLY_KEYS:\s*\[&str;\s*\d+\s*\]\s*=\s*\[(.*?)\];", re.S
)
# 第 6 查的第二类例外：`core/config.rs` 的 `UI_STATE_KEYS`（**纯界面状态**的键，m243）。
# 它们的"行为读者"只能是设置页自己：`ui_cards` 描述的就是设置页长什么样（哪几张卡收起），
# "改了不影响画面"在它身上不成立——它本身就是画面。与 CONFIG_ONLY_KEYS 正好相反：
# 那个是"界面刻意不出现"，这个是"界面是它唯一的消费者"。准入写死在 config.rs 那张表里，
# 本查不自己维护名单（第二份名单＝改一处漏一处）。
UI_STATE_DECL = re.compile(
    r"pub const UI_STATE_KEYS:\s*\[&str;\s*\d+\s*\]\s*=\s*\[(.*?)\];", re.S
)
CFG_FIELD = re.compile(r"\bcfg\.([a-z_][a-z0-9_]*)")

# 违规分类的**固定顺序**，用于打印表头与 `--json` 里的 `by_category` 键序。
# 与上面"九查"顺序一一对应；新增分类请**尾追**（头插会打乱旧日志三段布局）。
CATEGORIES = ("abs_path", "debug_macro", "marker", "boundary_doc", "panic_source", "config_last_mile",
              "patch_parity", "control_char", "ps1_ascii", "literal_color",
              "ui_super_path", "secret", "single_window_key", "footer_buttons")


def is_comment(line: str) -> bool:
    s = line.lstrip()
    return s.startswith("//") or s.startswith("/*")


def has_doc_above(lines: list, i: int) -> bool:
    """从第 i 行向上找紧邻的注释。

    跳过空行与属性行（`#[cfg(...)]` / `#[no_mangle]` 之类），遇到第一个"实在"的行：
    若是 `//` 或 `/*` 注释即通过；否则失败。最多回看 8 行——超过这个距离的
    注释已经属于"上一个 item"，不能作为当前边界的解释。
    """
    j = i - 1
    looked_back = 0
    while j >= 0 and looked_back < 8:
        s = lines[j].strip()
        if s == "" or ATTRIBUTE_LINE.match(lines[j]):
            j -= 1
            looked_back += 1
            continue
        return is_comment(lines[j])
    return False


def is_boundary_line(line: str) -> bool:
    return bool(BOUNDARY_ITEM.match(line) or BOUNDARY_NO_MANGLE.match(line))


def has_exempt_above(lines: list, i: int) -> bool:
    """panic 源上方 3 行内有没有 `红线豁免:` 标注（§八.1 第三类例外）。

    只给"必须就地写理由"这一种豁免，不做文件级/函数级白名单——豁免要是能批量声明，
    红线就又回到"文档里有、代码里没"的原点了（审计 V9 的成因）。
    """
    return any(EXEMPT_MARK.match(lines[j]) for j in range(max(0, i - 3), i))


def parse_key_arms(lines):
    """从 `core/config.rs` 的源码行里数出（ALL_KEYS 顺序, {键: 臂上解到的 cfg.<字段>}）。

    臂从 `"key" =>` 起、到下一条臂止（多行臂也收）。**不按声明顺序对齐字段名**：
    `double_click → dbl_click`、`wheel_behavior → wheel` 本来就不同名，靠顺序对齐会错位。
    """
    m = ALL_KEYS_DECL.search("\n".join(lines))
    if not m:
        return [], {}
    keys = re.findall(r'"([a-z_0-9]+)"', m.group(1))
    arms, cur = {}, None
    for line in lines:
        hit = ARM_KEY_NAME.match(line)
        if hit:
            cur = hit.group(1)
            arms.setdefault(cur, set())
        elif cur is not None and line.strip() and not line.startswith((" ", "\t")):
            cur = None  # 顶格行＝函数结束，别再往后累加
        if cur is None:
            continue
        for f in CFG_FIELD.findall(line):
            arms[cur].add(f)
    return keys, arms


def parse_config_only(text):
    """从 `config.rs` 里读 `CONFIG_ONLY_KEYS`（配置专用键，界面刻意不出现）。"""
    m = CONFIG_ONLY_DECL.search(text)
    return set(re.findall(r'"([a-z_0-9]+)"', m.group(1))) if m else set()


def parse_ui_state(text):
    """从 `config.rs` 里读 `UI_STATE_KEYS`（纯界面状态的键，读者只有设置页自己）。"""
    m = UI_STATE_DECL.search(text)
    return set(re.findall(r'"([a-z_0-9]+)"', m.group(1))) if m else set()


def last_mile(keys, arms, sources, config_only=(), ui_state=()):
    """每个键：要有**用户入口**（`src/ui/**` 引用它的字段），也要有**行为读者**
    （除 `src/core/config.rs` 与 `src/ui/settings/**` 之外还有地方引用它）。

    入口口径刻意放宽到整个 `src/ui/**`：紧口径（只认设置页那几个文件）会把
    `raw_last` 判成"没有入口"——它的入口是裸数据参数窗 `src/ui/raw_dialog.rs`，
    这是实测出来的，不是猜的。**宁可少判一格，也不造一条会把人推向关掉门禁的假警报。**

    `config_only`：`CONFIG_ONLY_KEYS` 里的键（高级用户手改 ini，界面刻意不出现）。
    只豁免"用户入口"，**行为读者照旧必须有**——否则就是"改了没人读"的假数据源。

    `ui_state`：`UI_STATE_KEYS` 里的键（纯界面状态，读者天然只在设置页内）。
    只豁免"行为读者"这一格，**入口照旧必须有**（`ui_touchpoints_cover_all_keys` 另管）。
    """
    config_only = set(config_only)
    ui_state = set(ui_state)
    bad = []
    for key in keys:
        fields = arms.get(key) or set()
        if not fields:
            bad.append(f"config_last_mile: 键 `{key}` 的解析臂没解到任何 cfg.<字段>（本查失明）")
            continue
        patterns = [re.compile(rf"\b{re.escape(f)}\b") for f in sorted(fields)]
        entry = behavior = False
        for rel, lines in sources.items():
            if rel == "src/core/config.rs":
                continue  # 声明与写盘自身不算读者
            in_settings = rel.startswith("src/ui/settings/")
            for line in lines:
                if not any(p.search(line) for p in patterns):
                    continue
                if rel.startswith("src/ui/"):
                    entry = True
                if not in_settings:
                    behavior = True
        if not entry and key not in config_only:
            bad.append(
                f"config_last_mile: 键 `{key}`（字段 {sorted(fields)}）没有用户入口——"
                "`src/ui/**` 里没有任何一处非注释引用它"
            )
        if not behavior and key not in ui_state:
            bad.append(
                f"config_last_mile: 键 `{key}`（字段 {sorted(fields)}）没有行为读者——"
                "只有设置页自己碰它，改了不影响任何画面或交互（K116 的用户面同族）"
            )
    return bad


def detect_last_mile(src_dir, root_dir):
    cfg = src_dir / "core" / "config.rs"
    if not cfg.is_file():
        return ["config_last_mile: 找不到 src/core/config.rs（本查失明）"]
    text = cfg.read_text(encoding="utf-8", errors="replace")
    keys, arms = parse_key_arms(text.splitlines())
    if not keys:
        return ["config_last_mile: ALL_KEYS 没解析出任何键（词式变了，本查失明）"]
    config_only = parse_config_only(text)
    ui_state = parse_ui_state(text)
    sources = {}
    for p in sorted(src_dir.rglob("*.rs")):
        rel = p.relative_to(root_dir).as_posix()
        prod, in_test = [], bool(TEST_ONLY.search(rel))
        for line in p.read_text(encoding="utf-8", errors="replace").splitlines():
            if CFG_TEST.match(line):
                in_test = True
            if in_test or is_comment(line):
                continue  # 测试桩与注释都不算引用
            prod.append(line)
        sources[rel] = prod
    return last_mile(keys, arms, sources, config_only, ui_state)


# ── 七查（m123）：第三方补丁的"登记簿章节 ↔ `.3rd` 标记"两问 ──────────────────
# `.3rd/wind-ui-rust-0.23.0` 是本地 path 依赖（cargo 直接编它），补丁打在哪、有没有漏，
# 全靠 `main/patches/PATCHES.md` 那份登记簿；而登记簿自己写"每个改动处都标了 `★ RuPix 补丁 0NN`，
# 与本章节一一对应"——这句话此前**没有任何机器读过**（m123 实测：17 个章节里 2 个在 0.23.0 真没有落点，
# 02 由上游内建、05 被补丁 014 取代 ⇒ 毛病不是"补丁丢了"，是**那句承诺的范围过宽**）。
# ★ 第三处"`patches/wind-ui-rust-0.23.0/` 归档 ↔ 工作副本逐字节"**不在本查**：它已由
#   `tests/docs_consistency.rs::j13_patch_archive_matches_worktree` 逐字节守着（实测比 16 份、硬门禁）。
#   同一判据立两个定义点＝改一个漏一个（`core/pixels.rs` 头注释记的那次 MAX_PIXELS 两定义点事故同形）
#   ⇒ 本查只补两问；要补的口子补在 j13 里（工作副本缺文件也算漂移）。
PATCH_MARK = re.compile(r"RuPix 补丁\s*(\d+)")
PATCH_EXEMPT = "0.23.0 无落点"          # 章节正文写了这句＝承认没有落点，不算缺标记
UPSTREAM_DIR = ".3rd/wind-ui-rust-0.23.0"


def parse_patch_sections(text):
    """把 PATCHES.md 切成 {补丁号: 该节正文}（前言不算节）。"""
    parts = re.split(r"(?m)^## 补丁\s*0*(\d+)\b", text)
    return {int(parts[i]): parts[i + 1] for i in range(1, len(parts) - 1, 2)}


def patch_parity(secs, marks):
    """两问，全是**存在性**判断、不猜语义（K133：只判等值/存在才没有假警报形状）。"""
    bad = []
    for n in sorted(secs):
        if PATCH_EXEMPT in secs[n]:
            continue
        if n not in marks:
            bad.append(
                f"patch_parity: 补丁 {n:02d} 登记了章节，`.3rd` 里却没有任何 `RuPix 补丁 {n:02d}` 标记"
                f"（补丁丢了？标记写法漂了？还是该写明「{PATCH_EXEMPT}」+ 原因）"
            )
    for n in sorted(set(marks) - set(secs)):
        bad.append(f"patch_parity: `.3rd` 里有 `RuPix 补丁 {n:02d}` 标记，PATCHES.md 没有该章节（改了上游没记账）")
    return bad


def detect_patch_parity(root_dir):
    """root_dir = `<main>/`。返回违规清单；任何一侧读不到都算违规（本查不许空跑）。"""
    reg = root_dir / "patches" / "PATCHES.md"
    third = root_dir.parent / UPSTREAM_DIR
    if not reg.is_file():
        return [f"patch_parity: 找不到 {reg}（本查失明）"]
    if not third.is_dir():
        return [f"patch_parity: 找不到上游工作副本 {third}（本查失明）"]
    secs = parse_patch_sections(reg.read_text(encoding="utf-8", errors="replace"))
    if not secs:
        return ["patch_parity: PATCHES.md 没解析出任何 `## 补丁 NN` 章节（词式变了，本查失明）"]
    marks = set()
    for p in sorted(third.rglob("*.rs")):
        marks.update(int(x) for x in PATCH_MARK.findall(p.read_text(encoding="utf-8", errors="replace")))
    if not marks:
        return ["patch_parity: `.3rd` 里一处 `RuPix 补丁 NN` 都没找到（标记写法变了，本查失明）"]
    return patch_parity(secs, marks)


# ── 八查（m124）：文本文件里的不可见控制字符 ────────────────────────────────────
# 起因是实测抓到一枚：`DOCS_MANIFEST.md` 那格本该写 `RuPix-Decoder\build.ps1`，文件里实际是
# `extension` + **0x08（退格）** + `uild.ps1`——脚本改文件时那对反斜杠加 b 被当成转义吃掉，
# 于是"b"没了、剩下一个谁也看不见的退格。读起来像 `extensionuild.ps1`，锚点已经静默损坏。
# 这条判据天生零假警报：正常源文件里不该有任何控制字符，除了制表、换行、回车三枚（本仓有 CRLF 的 md）。
CTRL_BYTES = re.compile(rb"[\x00-\x08\x0b\x0c\x0e-\x1f]")   # 放行 09 制表、0a 换行、0d 回车
TEXT_SUFFIXES = {".md", ".rs", ".py", ".bat", ".ps1", ".toml", ".json"}
# 跳过这些段：构建中间物与快照（里面是别人的产物，不是本仓手写的字）。
CTRL_SKIP_SEGS = ("/target/", "/.git/", "/.temp/", "/node_modules/")


def control_chars(data):
    """字节串里逐枚报出控制字符的 (行号, 码位)。纯函数，自检直接喂合成样本。"""
    out = []
    for m in CTRL_BYTES.finditer(data):
        out.append((data[: m.start()].count(b"\n") + 1, m.group(0)[0]))
    return out


def detect_control_chars(root_dir):
    """root_dir = `<main>/`。扫第一方文本文件的控制字符；一个文件都没扫到＝本查失明。"""
    bad, scanned = [], 0
    for p in sorted(root_dir.rglob("*")):
        if not p.is_file() or p.suffix.lower() not in TEXT_SUFFIXES:
            continue
        rel = p.relative_to(root_dir).as_posix()
        if any(seg in "/" + rel for seg in CTRL_SKIP_SEGS):
            continue
        scanned += 1
        data = p.read_bytes()
        for line, code in control_chars(data):
            bad.append(
                f"control_char: {rel}:{line} 有不可见控制字符 0x{code:02x}"
                "（反斜杠转义被脚本吃掉一个字母时就是这么坏的——把这枚字符删掉、锚点重写一遍）"
            )
    if scanned == 0:
        return ["control_char: 域内一个文本文件都没扫到（目录形状或扩展名清单变了，本查失明）"]
    return bad


def non_ascii_lines(data):
    """字节串里逐行报出"含非 ASCII 字节"的行号。纯函数，自检直接喂合成样本。

    只针对 `.ps1`：`.md` / `.rs` / 注释里的中文是本项目日常，本查不许碰它们。
    """
    return [i + 1 for i, line in enumerate(data.splitlines()) if any(b > 127 for b in line)]


def detect_non_ascii_ps1(root_dir):
    """`<main>/**/*.ps1` 必须纯 ASCII（见模块头九查）。一个 ps1 都没扫到＝本查失明。"""
    bad, scanned = [], 0
    for p in sorted(root_dir.rglob("*.ps1")):
        if not p.is_file():
            continue
        rel = p.relative_to(root_dir).as_posix()
        if any(seg in "/" + rel for seg in CTRL_SKIP_SEGS):
            continue
        scanned += 1
        data = p.read_bytes()
        if data.startswith(b"\xef\xbb\xbf"):
            bad.append(f"ps1_ascii: {rel}: 带 UTF-8 BOM（PS 5.1 要 ASCII 或 UTF-16）")
        for lineno in non_ascii_lines(data)[:5]:
            bad.append(
                f"ps1_ascii: {rel}:{lineno} 含非 ASCII 字节 —— PS 5.1 按 ANSI 读它，"
                "错解会坏到邻近语句上（K27）；注释改成英文，字符串改用 [char] 码点拼"
            )
    if scanned == 0:
        return ["ps1_ascii: 域内一个 .ps1 都没扫到（目录形状变了，本查失明）"]
    return bad


def detect_literal_colors(src_dir):
    """第 10 查：`src/ui/**/*.rs` 不许自带字面色值，唯一例外是 `ui/tokens.rs`（定义处本身）。

    零文件扫到＝本查失明（与其余各查同款：静默通过比报错更危险）。
    """
    bad, scanned = [], 0
    for p in sorted((src_dir / "ui").rglob("*.rs")):
        if not p.is_file():
            continue
        rel = p.relative_to(src_dir.parent).as_posix()
        if rel.endswith("ui/tokens.rs"):
            continue  # 令牌文件自身就是字面色值的唯一合法落点
        scanned += 1
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
        in_test = bool(TEST_ONLY.search(rel))
        for i, line in enumerate(lines, 1):
            if CFG_TEST.match(line):
                in_test = True
            if in_test or is_comment(line):
                continue
            if LITERAL_COLOR.search(line):
                bad.append(
                    f"literal_color: {rel}:{i} ui 层出现字面色值 -- {line.strip()[:80]}"
                    "（色值统一在 ui/tokens.rs 定义：换肤只改一处、改一档灰不会漏掉某个窗；"
                    "确有新色就先在 tokens.rs 里起个名字，再回来引用）"
                )
    if scanned == 0:
        return ["literal_color: 域内一个 ui/*.rs 都没扫到（目录形状变了，本查失明）"]
    return bad


# ── 十一查（2026-10-26）：ui 层内跨模块引用必须经父模块 ──────────────────────
def detect_ui_super_path(src_dir: pathlib.Path) -> list[str]:
    """`src/ui/**` 的非注释行不许出现 `crate::ui::`。

    为什么值得一条硬门禁：ui 是**平铺**的（10 个模块同层），兄弟之间一写全路径，
    依赖就成了网——"谁用了谁"只能靠 grep，改一个模块名要扫四处，删一个依赖也看不出
    还有没有人在用。全部经 `super::` 之后，`src/ui/mod.rs` 的「层内中枢」块就是
    这一层的内部 API 清单；删掉那里一行 `use`，编译立刻指出还有谁在用。

    注释行豁免：文档里的 [`crate::ui::tokens`] 是**绝对链接**，跨层可点，改成 `super::`
    反而在 rustdoc 里指不进去。
    """
    ui_dir = src_dir / "ui"
    bad: list[str] = []
    scanned = 0
    for p in sorted(ui_dir.rglob("*.rs")):
        if not p.is_file():
            continue
        scanned += 1
        rel = p.relative_to(src_dir.parent).as_posix()
        in_test = bool(TEST_ONLY.search(rel))
        for i, line in enumerate(p.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
            if CFG_TEST.match(line):
                in_test = True
            if in_test or is_comment(line):
                continue
            if UI_SIBLING_PATH.search(line):
                bad.append(
                    f"ui_super_path: {rel}:{i} ui 层内跨模块引用走了全路径 -- {line.strip()[:80]}"
                    "（平铺模块之间一律经 `super::`：类型从中枢 `use super::{…}` 取，"
                    "函数写 `super::<模块>::名字`；中枢与理由见 src/ui/mod.rs 的「层内中枢」）"
                )
    if scanned == 0:
        return ["ui_super_path: ui/ 下一个 .rs 都没扫到（目录形状变了，本查失明）"]
    return bad


# ── 十四查（2026-11-06）：按钮创建点只留两处 ────────────────────────────
# 底栏按钮 = `dialog_footer::action`，普通按钮 = `settings::rows::btn`。两者内部都落到
# `Element::button`，而那**只能**出现在 `btn` 的实现里——否则「统一版式」就只是自觉。
BUTTON_CTOR = re.compile(r"\bElement::button\s*\(")
BUTTON_CTOR_ALLOWED = {"src/ui/settings/rows.rs"}


def detect_footer_buttons(src_dir: pathlib.Path) -> list[str]:
    """`src/ui/**` 里除 `settings/rows.rs` 之外不许出现 `Element::button(`。

    为什么值得一条硬门禁：2026-11-06 之前三扇对话框各写一套四边留白/行间距/按钮位，
    用户一眼看出「通道调色的快捷键窗按钮和其它插件不一样」。事后靠人盯是盯不住的——
    新开一扇窗的人只会照抄眼前那扇。把创建点收到两处，`dialog_footer` 的几何判据
    （340/480/640 三种窗宽量右缘、底缘、间距）才有意义：它量的就是**所有人**的按钮行。
    """
    ui_dir = src_dir / "ui"
    bad: list[str] = []
    scanned = 0
    for p in sorted(ui_dir.rglob("*.rs")):
        if not p.is_file():
            continue
        scanned += 1
        rel = p.relative_to(src_dir.parent).as_posix()
        if rel in BUTTON_CTOR_ALLOWED:
            continue
        in_test = bool(TEST_ONLY.search(rel))
        for i, line in enumerate(
            p.read_text(encoding="utf-8", errors="replace").splitlines(), 1
        ):
            if CFG_TEST.match(line):
                in_test = True
            if in_test or is_comment(line):
                continue
            if BUTTON_CTOR.search(line):
                bad.append(
                    f"footer_buttons: {rel}:{i} 自己造按钮 -- {line.strip()[:80]}"
                    "（普通按钮走 `btn(..)`、底栏按钮走 `dialog_footer::action(..)`，"
                    "整扇窗走 `dialog_footer::shell`；版式正本 docs/界面版式规范.md §2）"
                )
    if scanned == 0:
        return ["footer_buttons: ui/ 下一个 .rs 都没扫到（目录形状变了，本查失明）"]
    return bad


# ── 十三查（2026-11-04）：单例窗「同键既关又开」 ────────────────────────
def single_window_clash(rel: str, lines: list[str]) -> list[str]:
    """判一个文件里 `close_window(X)` 与 `.single(X)` 是否共用同一个变量。

    扫描与 selftest 共用这一份判据（不许"自检验正则、实际跑另一套代码"）。
    """
    in_test = bool(TEST_ONLY.search(rel))
    close_vars: dict[str, int] = {}
    single_vars: dict[str, int] = {}
    for i, line in enumerate(lines, 1):
        if CFG_TEST.match(line):
            in_test = True
        if in_test or is_comment(line):
            continue
        m = CLOSE_WINDOW_ARG.search(line)
        if m:
            close_vars.setdefault(m.group(1), i)
        m = SINGLE_WINDOW_ARG.search(line)
        if m:
            single_vars.setdefault(m.group(1), i)
    bad = []
    for name, at in sorted(close_vars.items(), key=lambda kv: kv[1]):
        if name in single_vars:
            bad.append(
                f"single_window_key: {rel}:{at} close_window({name}) 与 "
                f".single({name})（{rel}:{single_vars[name]}）共用同一个键 -- "
                "关窗是投递、开窗是即时，滞留的 WM_CLOSE 会把刚开出的新窗收走；"
                "改用 singleton::recycle 拿两个不同的键"
            )
    return bad


def detect_single_window_key(src_dir: pathlib.Path) -> list[str]:
    """`src/ui/**` 里 `close_window(X)` 与 `.single(X)` 不许共用同一个变量。

    为什么值得一条硬门禁：K196 现场是「插件页齿轮点不动，窗闪一下就没了」——看起来
    完全无害的「先关旧窗再开新窗」写成了同一个单例键，而框架的关窗是**延迟投递**、
    开窗是**即时建窗**，滞留的 WM_CLOSE 会认错对象。静态扫同名变量即可挡住这一类；
    换成 `super::singleton::recycle` 给出的两个不同键就合法。
    """
    ui_dir = src_dir / "ui"
    bad: list[str] = []
    scanned = 0
    for p in sorted(ui_dir.rglob("*.rs")):
        if not p.is_file():
            continue
        scanned += 1
        rel = p.relative_to(src_dir.parent).as_posix()
        bad.extend(single_window_clash(rel, p.read_text(encoding="utf-8", errors="replace").splitlines()))
    if scanned == 0:
        return ["single_window_key: ui/ 下一个 .rs 都没扫到（目录形状变了，本查失明）"]
    return bad


# ── 十二查（2026-10-26）：无密钥 / 无隐私 ──────────────────────────────
def detect_secrets(root_dir: pathlib.Path) -> list[str]:
    """`main/**` 里不许出现密钥、令牌、口令字面量、私人邮箱、含真名的本机路径。

    为什么这条要在**push 之前**拦：泄了之后删文件不管用，历史还在。判据刻意窄
    （只认高置信写法）——一条天天误报的门禁，最后一定被人加白名单加到失效。
    """
    bad: list[str] = []
    scanned = 0
    for p in sorted(root_dir.rglob("*")):
        if not p.is_file():
            continue
        if any(part in SECRET_SKIP_DIRS for part in p.relative_to(root_dir).parts):
            continue
        if p.suffix.lower() in SECRET_SKIP_SUFFIX or p.name == "Cargo.lock":
            continue
        if p.resolve() == SECRET_SELF:
            continue
        scanned += 1
        rel = p.relative_to(root_dir.parent).as_posix()
        try:
            text = p.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for i, line in enumerate(text.splitlines(), 1):
            if SECRET_PRIVATE_KEY.search(line):
                bad.append(f"secret: {rel}:{i} private key block")
            if SECRET_TOKEN.search(line):
                bad.append(f"secret: {rel}:{i} access token prefix -- {line.strip()[:60]}")
            if SECRET_URL_CREDS.search(line):
                bad.append(f"secret: {rel}:{i} credentials inside a url")
            if SECRET_CRED_ASSIGN.search(line):
                bad.append(f"secret: {rel}:{i} literal credential assignment -- {line.strip()[:60]}")
            if p.name not in SECRET_EMAIL_OK:
                m = SECRET_EMAIL.search(line)
                if m:
                    bad.append(f"secret: {rel}:{i} email -- {m.group(0)}")
            m = SECRET_USER_PATH.search(line)
            if m and m.group(1) not in SECRET_USER_OK:
                bad.append(f"secret: {rel}:{i} absolute user path -- {m.group(0)}")
    if scanned == 0:
        return ["secret: 一个文件都没扫到（目录形状变了，本查失明）"]
    return bad


def selftest() -> int:
    """自检：合成样例必须被十查规则正确命中（tests/docs_consistency.rs 判据⑧）。

    门禁的牙齿——扫不出违规的门禁等于没门禁；扫出违规但样例其实合规的门禁
    会拖慢每次流水线。两侧都要合成样本证一遍。
    """
    fails = []

    def expect(cond, msg):
        if not cond:
            fails.append(msg)

    # secret —— 四条高置信判据必须命中，而"赋值变量"这种写法不许误伤
    expect(bool(SECRET_PRIVATE_KEY.search("-----BEGIN RSA PRIVATE KEY-----")), "私钥块应命中")
    expect(bool(SECRET_TOKEN.search("token: glpat-abcdefgh12345678")), "glpat- 应命中")
    expect(not SECRET_TOKEN.search("文档里只说 glpat- 这个前缀"), "光写前缀不该命中")
    expect(bool(SECRET_URL_CREDS.search("https://bob:pw123@example.com/x")), "URL 内嵌凭据应命中")
    expect(bool(SECRET_CRED_ASSIGN.search('password = "hunter2hunter2"')), "口令字面量应命中")
    expect(not SECRET_CRED_ASSIGN.search("password: self.config.password"), "字段赋值不该误伤")
    expect(not SECRET_TOKEN.search("fn attach_activation_token(token: Option<String>)"), "普通标识符不该误伤")
    # 正则本身**会**命中 `C:\Users\Someone`（它只做形状匹配）；真正的判定在白名单那一步，
    # 所以这里验的是"命中的用户名落进豁免名单"，而不是"没命中"。
    _m = SECRET_USER_PATH.search(r"C:\Users\Someone\AppData")
    expect(_m is not None and _m.group(1) in SECRET_USER_OK, "占位符用户名该落进豁免名单")

    # ui_super_path —— 层内全路径必须命中，`super::` 写法不许碰
    expect(bool(UI_SIBLING_PATH.search("use crate::ui::chrome::Chrome;")), "ui 内全路径应命中")
    expect(UI_SIBLING_PATH.search("use super::{Chrome, Hub};") is None, "super:: 写法不该命中")
    expect(UI_SIBLING_PATH.search("super::raw_dialog::open_path(p);") is None, "super:: 函数调用不该命中")

    # footer_buttons（十四查：按钮创建点）—— 裸 `Element::button` 必须命中，两条正路不许碰
    expect(
        bool(BUTTON_CTOR.search('Element::button("确定").on_click(f)')),
        "裸 Element::button 应命中",
    )
    expect(BUTTON_CTOR.search('btn("确定").on_click(f)') is None, "btn 不该命中")
    expect(
        BUTTON_CTOR.search('super::dialog_footer::action("确定", f)') is None,
        "dialog_footer::action 不该命中",
    )
    expect(
        detect_footer_buttons(SRC) == [],
        "本仓 ui 层不该有第二处按钮创建点（有就说明有人绕开了 dialog_footer）",
    )

    # single_window_key（十三查：同键既关又开的单例窗，K196）
    expect(
        CLOSE_WINDOW_ARG.search("ctx.close_window(win_key.clone());").group(1) == "win_key",
        "close_window 取裸变量名（带 .clone() 也还是同一个键）",
    )
    expect(
        SINGLE_WINDOW_ARG.search(".single(win_key)").group(1) == "win_key",
        "single 取裸变量名",
    )
    expect(
        len(single_window_clash("src/ui/plugin_settings.rs", [
            'let win_key = format!("plugin-settings:{id}");',
            "ctx.close_window(win_key.clone());",
            "Window::new(title, 480, 300)",
            ".single(win_key)",
        ])) == 1,
        "同键既关又开必须命中（K196 现场写法）",
    )
    expect(
        single_window_clash("src/ui/plugin_settings.rs", [
            'let (old_key, win_key) = super::singleton::recycle("x");',
            "ctx.close_window(old_key);",
            ".single(win_key)",
        ]) == [],
        "两个不同的键不该误报",
    )
    expect(
        single_window_clash("src/ui/plugin_settings.rs", [
            "// ctx.close_window(win_key); 说明性注释",
            ".single(win_key)",
        ]) == [],
        "注释里的旧写法不算调用",
    )
    expect(
        single_window_clash("src/ui/plugin_settings.rs", [
            "#[cfg(test)]",
            "ctx.close_window(win_key);",
            ".single(win_key)",
        ]) == [],
        "测试区不算生产代码",
    )

    # TEST_ONLY —— 文件级测试模块（`#[cfg(test)] mod tests;`）整篇算测试区；
    # 判据只看文件名，所以生产文件名必须一条都不命中（否则 panic 红线会被悄悄绕开）。
    expect(bool(TEST_ONLY.search("src/decoders/tests.rs")), "tests.rs 应被认成整篇测试区")
    expect(not TEST_ONLY.search("src/decoders/host.rs"), "生产文件不该被认成测试区")
    expect(not TEST_ONLY.search("src/decoders/mytests.rs"), "只认精确文件名 tests.rs，不认后缀")

    # abs_path
    expect(ABS_PATH.search('let p = "C:\\\\x";') is not None, "abs_path 应命中 C:\\\\x 转义")
    expect(ABS_PATH.search('let p = r"D:\\y";') is not None, "abs_path 应命中 raw string")
    expect(ABS_PATH.search("let n = 1;") is None, "abs_path 不该误报普通代码")

    # literal_color（第十查：ui 层色值单一来源）
    expect(
        LITERAL_COLOR.search("const A: Color = Color::rgb(15, 15, 15);") is not None,
        "literal_color 应命中 Color::rgb(",
    )
    expect(
        LITERAL_COLOR.search("const A: Color = Color::rgba(28, 28, 30, 225);") is not None,
        "literal_color 应命中 Color::rgba(",
    )
    expect(
        LITERAL_COLOR.search("const A: Color = to_color4(OVERLAY_PILL_BG);") is None,
        "literal_color 不该误报令牌引用（改色只改一处正是靠这层间接）",
    )

    # debug_macro
    expect(DEBUG_MACRO.search('println!("x");') is not None, "debug_macro 应命中 println!")
    expect(DEBUG_MACRO.search("// println! 是调试宏") is None or True, "注释行由上层 comment 判定豁免，不测这里")
    expect(DEBUG_MACRO.search('log::debug!("ok");') is None, "log::debug 不是 println，不该命中")

    # marker
    expect(MARKER.search("// TODO: 补超时") is not None, "marker 应命中 TODO:")
    expect(MARKER.search("// FIXME(x): y") is not None, "marker 应命中 FIXME(")
    expect(MARKER.search("// 这里以前是 TODO：叙述") is None, "全角冒号叙述不算真标记")

    # boundary_doc — 三类项都必须紧邻注释。
    lines_bad = [
        "struct S;",
        "",
        "unsafe fn poke(p: *const u8) {}",
    ]
    expect(is_boundary_line(lines_bad[2]), "unsafe fn 应识别为边界项")
    expect(not has_doc_above(lines_bad, 2), "无注释的 unsafe fn 必须判违规")

    lines_ok = [
        "struct S;",
        "",
        "// SAFETY: 调用方保证 p 指向 len 字节可读。",
        "unsafe fn poke(p: *const u8) {}",
    ]
    expect(has_doc_above(lines_ok, 3), "紧邻注释的 unsafe fn 通过")

    lines_attr = [
        "/// 说明。",
        "#[cfg(windows)]",
        'extern "C" fn thunk(ctx: *mut u8) -> i32 { 0 }',
    ]
    expect(
        has_doc_above(lines_attr, 2),
        "属性行 `#[cfg(windows)]` 应被跳过、上方 `///` 视为解释",
    )

    lines_mangle = [
        "#[no_mangle]",
        'pub extern "C" fn iv_probe(d: *const u8) -> i32 { 0 }',
    ]
    expect(is_boundary_line(lines_mangle[0]), "#[no_mangle] 应识别为边界项")
    expect(not has_doc_above(lines_mangle, 0), "#[no_mangle] 上方是空/文件头 → 违规")
    expect(
        not has_doc_above(lines_mangle, 1),
        "`extern \"C\" fn` 上方只有属性，仍判违规",
    )

    lines_mangle_ok = [
        "/// ABI 导出：iv_probe 只做魔数嗅探。",
        "#[no_mangle]",
        'pub extern "C" fn iv_probe(d: *const u8) -> i32 { 0 }',
    ]
    expect(
        has_doc_above(lines_mangle_ok, 1),
        "#[no_mangle] 有上方 `///` doc 应通过（跳过空行/属性回溯）",
    )
    expect(
        has_doc_above(lines_mangle_ok, 2),
        "`extern \"C\" fn` 跳过紧邻的 `#[no_mangle]` 属性后仍能找到 doc",
    )

    # 函数字段类型 `unsafe extern "C" fn(...)`（引用外部函数签名）不算定义。
    lines_field = ["    probe: unsafe extern \"C\" fn(*const u8, usize) -> i32,"]
    expect(
        not is_boundary_line(lines_field[0]),
        "结构体字段里的 `unsafe extern \"C\" fn(...)` 类型不该被判为边界项",
    )

    # panic_source —— §八.1 红线的四类都要命中，而**不 panic 的兄弟写法不能误伤**
    # （误伤一次，下一次就有人直接把门禁关掉）。
    expect(PANIC_SOURCE.search("let v = opt.unwrap();") is not None, "panic_source 应命中 .unwrap()")
    expect(PANIC_SOURCE.search('let v = res.expect("boom");') is not None, "应命中 .expect(")
    expect(PANIC_SOURCE.search('panic!("no");') is not None, "应命中 panic!")
    expect(PANIC_SOURCE.search('Btn::Info => unreachable!("x"),') is not None, "应命中 unreachable!")
    expect(PANIC_SOURCE.search("todo!();") is not None, "应命中 todo!")
    expect(
        PANIC_SOURCE.search('x.unwrap_or_else(|e| panic!("{e}"));') is not None,
        "unwrap_or_else 里套 panic! 也是 panic 源",
    )
    expect(PANIC_SOURCE.search("let n = opt.unwrap_or(0);") is None, "unwrap_or 不 panic，不该误伤")
    expect(PANIC_SOURCE.search("let n = opt.unwrap_or_default();") is None, "unwrap_or_default 不该误伤")
    expect(PANIC_SOURCE.search('let e = r.expect_err("x");') is None, "expect_err 不该误伤")
    expect(PANIC_SOURCE.search("if expect != Some(4) {") is None, "名为 expect 的变量不该误伤")
    expect(PANIC_SOURCE.search("// 这里不要 unwrap 好不好") is None or True, "注释行由上层 comment 判定豁免")
    expect(
        has_exempt_above(["// 红线豁免: 起不来就没有第二种活法。", "", "spawn(f).unwrap();"], 2),
        "上方 3 行内的 `红线豁免:` 标注应放行",
    )
    expect(
        not has_exempt_above(["fn f() {", "    spawn(f).unwrap();"], 1),
        "没有标注的 panic 源必须判违规",
    )
    expect(
        not has_exempt_above(["// 红线豁免要隔 4 行才到。", "", "", "", "spawn(f).unwrap();"], 4),
        "标注离得太远（>3 行）不放行",
    )

    # config_last_mile —— 六查的两侧样本：入口缺、行为缺、都齐，三臂各证一次。
    cfg = 'pub const ALL_KEYS: [&str; 3] = [\n    "alpha",\n    "beta",\n    "gamma",\n];\n' \
          'fn load_from() {\n    let mut cfg = C::default();\n    match k {\n' \
          '        "alpha" => cfg.a_flag = x,\n' \
          '        "beta" => cfg.b_mode = y,\n' \
          '        "gamma" => {\n            cfg.c_list.push(z);\n        }\n    }\n}\n'
    keys, arms = parse_key_arms(cfg.splitlines())
    expect(keys == ["alpha", "beta", "gamma"], "ALL_KEYS 顺序没解析出来（本查会失明）")
    expect(arms.get("gamma") == {"c_list"}, "多行臂里的 cfg.<字段> 也该收到")
    both = lambda lines: {"src/core/x.rs": lines, "src/ui/settings/p.rs": lines}
    expect(
        last_mile(["alpha"], {"alpha": {"a_flag"}}, {"src/runtime/r.rs": ["cfg.a_flag = x;"]}) != [],
        "只有行为、没有 UI 入口 ⇒ 必须报",
    )
    expect(
        last_mile(["beta"], {"beta": {"b_mode"}}, {"src/ui/settings/p.rs": ["c.b_mode = y;"]}) != [],
        "只有设置页自己碰、没有任何行为读 ⇒ 必须报（改了个寂寞）",
    )
    expect(
        last_mile(["gamma"], {"gamma": {"c_list"}}, both(["cfg.c_list.push(z);"])) == [],
        "入口与行为都在 ⇒ 不该报",
    )
    expect(
        last_mile(["beta"], {"beta": {"b_mode"}}, {"src/core/config.rs": ["cfg.b_mode = y;"],
                                                   "src/ui/settings/p.rs": ["cfg.b_mode = y;"]}) != [],
        "config.rs 自身的声明/写盘不算行为读者",
    )
    # 配置专用键（2026-10-29）：豁免 UI 入口，但行为读者照旧必须有。
    expect(
        parse_config_only('pub const CONFIG_ONLY_KEYS: [&str; 2] = [\n    "alpha",\n    "beta",\n];')
        == {"alpha", "beta"},
        "CONFIG_ONLY_KEYS 没解析出来（本查会失明）",
    )
    expect(
        last_mile(["alpha"], {"alpha": {"a_flag"}}, {"src/runtime/r.rs": ["cfg.a_flag = x;"]},
                  {"alpha"}) == [],
        "配置专用键：有行为读者即可，不该再要求 UI 入口",
    )
    expect(
        last_mile(["alpha"], {"alpha": {"a_flag"}}, {"src/ui/settings/p.rs": ["c.a_flag = y;"]},
                  {"alpha"}) != [],
        "配置专用键仍必须有行为读者（只豁免入口，不豁免'改了得有人读'）",
    )
    # 纯界面状态键（m243）：`ui_cards` 这类"设置页自身的显示状态"，行为读者天然只在
    # 设置页里。豁免的**只有**"行为读者"这一格——入口照旧必须有，名单外的键照旧要报。
    expect(
        parse_ui_state('pub const UI_STATE_KEYS: [&str; 1] = [\n    "ui_cards",\n];')
        == {"ui_cards"},
        "UI_STATE_KEYS 没解析出来（本查会失明）",
    )
    expect(
        last_mile(["alpha"], {"alpha": {"a_flag"}},
                  {"src/ui/settings/p.rs": ["h.card(c.a_flag).set(x);"]},
                  (), {"alpha"}) == [],
        "纯界面状态键：读者只在设置页里，不该报'没有行为读者'",
    )
    expect(
        last_mile(["beta"], {"beta": {"b_mode"}}, {"src/ui/settings/p.rs": ["c.b_mode = y;"]},
                  (), {"alpha"}) != [],
        "不在 UI_STATE_KEYS 里的键，照样必须有行为读者（名单不是全域豁免）",
    )
    expect(
        last_mile(["alpha"], {"alpha": {"a_flag"}}, {}, (), {"alpha"}) != [],
        "纯界面状态键仍然必须有用户入口（豁免的只有行为读者那一格）",
    )

    # patch_parity —— 七查两问：缺标记 / 无落点豁免 / 孤儿标记，各证一次。
    # （归档 ↔ 工作副本那问**不在这里**：由 `docs_consistency.rs::j13` 守，同一判据不立两个定义点。）
    secs = parse_patch_sections("前言\n## 补丁 02 —— x\n正文\n## 补丁 14 —— y\n落点状态：0.23.0 无落点\n")
    expect(set(secs) == {2, 14}, f"章节解析少了号：{sorted(secs)}")
    expect(PATCH_EXEMPT in secs[14], "豁免句没被认出来")
    expect(patch_parity({3: "正文"}, {4}) != [], "登记了 03 而 .3rd 只有 04 ⇒ 必须报缺标记")
    expect(patch_parity({14: "落点状态：0.23.0 无落点", 4: "正文"}, {4}) == [],
           "写明无落点的章节不该被当成缺标记")
    expect(patch_parity({4: "正文"}, {4, 9}) != [], ".3rd 出现没登记的号 09 ⇒ 必须报孤儿标记")
    expect(patch_parity({4: "正文"}, {4}) == [], "章节与标记两两对上 ⇒ 不该报")

    # control_chars —— 八查：控制字符逐枚报行号与码位；制表/换行/回车必须放行（本仓有 CRLF 的 md）。
    expect(control_chars(b"x = 1\x08y") == [(1, 8)], "退格 0x08 必须命中并给出行号")
    expect(control_chars(b"a\nb\x00c\nd\x1be") == [(2, 0), (3, 27)], "跨行要报第 2 行与第 3 行，NUL 与 ESC 的码位要对")
    expect(control_chars(b"tab\tlf\ncr\r ok") == [], "制表/换行/回车是合法字符，不许报")
    expect(control_chars("中文与 ASCII 混排\n".encode("utf-8")) == [], "中文与正常 ASCII 不该命中")

    # ps1_ascii —— 九查：非 ASCII 逐行报号；纯 ASCII 与 BOM 各证一次。
    expect(non_ascii_lines(b"a = 1\n# ok\n") == [], "纯 ASCII 不许报")
    expect(non_ascii_lines("# 中文注释\n".encode("utf-8")) == [1], "中文注释必须命中并给行号")
    expect(
        non_ascii_lines(b"# a\n# \xe4\xb8\xad\n# b\n") == [2],
        "只报真正含非 ASCII 的那一行",
    )
    expect(
        non_ascii_lines("★".encode("utf-8")) == [1],
        "星标这类装饰字符也算：ANSI 下同样会被错解",
    )

    if fails:
        for f in fails:
            print(" [x] selftest: " + f)
        print(f"\nselftest failures: {len(fails)}")
        return 1
    print("selftest: ok")
    return 0


def main():
    if "--selftest" in sys.argv:
        return selftest()
    if "--json" in sys.argv:
        # 机器可读通道必须锁 UTF-8：Windows 默认按控制台代码页（本机 GBK）编码，
        # 重定向到文件时中文会被写成非法字节——m124 实测 `--json > x.json` 再读就
        # `UnicodeDecodeError: invalid continuation byte 0xd3`。**rc 与文件都会"看着成功"，
        # 半份读数却已经坏了**（m119 那把尺子同一课）。控制台人读那一侧保持本地码页不动。
        sys.stdout.reconfigure(encoding="utf-8")
    # 违规按分类装桶：`(category, message)`。同 `layering_scan`，用列表而非 dict-of-list
    # 保输出顺序稳定（文件遍历已 sorted），分类计数从这一条列表聚合。
    violations = []
    for p in sorted(SRC.rglob("*.rs")):
        rel = p.relative_to(ROOT_DIR).as_posix()
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
        in_test = bool(TEST_ONLY.search(rel))
        for i, line in enumerate(lines, 1):
            if CFG_TEST.match(line):
                in_test = True
            if in_test:
                continue  # 测试桩豁免
            comment = is_comment(line)
            snippet = line.strip()[:80]
            if ABS_PATH.search(line) and not comment:
                violations.append(("abs_path", f"{rel}:{i}: hardcoded absolute path -- {snippet}"))
            if DEBUG_MACRO.search(line) and not comment and rel not in STDOUT_EXEMPT:
                violations.append(("debug_macro", f"{rel}:{i}: debug leftover -- {snippet}"))
            if comment and MARKER.search(line):
                violations.append(("marker", f"{rel}:{i}: unfinished marker -- {snippet}"))
            # boundary_doc：FFI 边界项必须紧邻注释。0-index 传给 has_doc_above。
            is_boundary = bool(BOUNDARY_ITEM.match(line) or BOUNDARY_NO_MANGLE.match(line))
            if is_boundary and not has_doc_above(lines, i - 1):
                violations.append(
                    (
                        "boundary_doc",
                        f"{rel}:{i}: FFI/unsafe item without a 'why' comment above -- {snippet}",
                    )
                )
            # panic_source：§八.1 红线的机器执行（0-index 传给 has_exempt_above）。
            if PANIC_SOURCE.search(line) and not comment and not has_exempt_above(lines, i - 1):
                violations.append(
                    (
                        "panic_source",
                        f"{rel}:{i}: production panic source without a 红线豁免 note -- {snippet}",
                    )
                )

    # patch_parity：第三方补丁的登记簿章节↔`.3rd` 标记两问（见模块头七查与 detect_patch_parity）。
    for msg in detect_patch_parity(ROOT_DIR):
        violations.append(("patch_parity", msg))

    # config_last_mile：设置键的"最后一公里"（见模块头六查与 detect_last_mile）。
    for msg in detect_last_mile(SRC, ROOT_DIR):
        violations.append(("config_last_mile", msg))

    # control_char：第一方文本文件里的不可见控制字符（见模块头八查与 detect_control_chars）。
    for msg in detect_control_chars(ROOT_DIR):
        violations.append(("control_char", msg))

    # ps1_ascii：`.ps1` 必须纯 ASCII（见模块头九查与 detect_non_ascii_ps1）。
    for msg in detect_non_ascii_ps1(ROOT_DIR):
        violations.append(("ps1_ascii", msg))

    # literal_color：ui 层色值单一来源（见模块头十查与 detect_literal_colors）。
    for msg in detect_literal_colors(SRC):
        violations.append(("literal_color", msg))

    # ui_super_path：ui 层内跨模块引用经父模块（见模块头十一查与 detect_ui_super_path）。
    for msg in detect_ui_super_path(SRC):
        violations.append(("ui_super_path", msg))

    # secret：无密钥 / 无隐私（见模块头十二查与 detect_secrets）。
    for msg in detect_secrets(ROOT_DIR):
        violations.append(("secret", msg))

    # single_window_key：单例窗同键既关又开（见模块头十三查与 detect_single_window_key）。
    for msg in detect_single_window_key(SRC):
        violations.append(("single_window_key", msg))

    # footer_buttons：按钮创建点只留两处（见模块头十四查与 detect_footer_buttons）。
    for msg in detect_footer_buttons(SRC):
        violations.append(("footer_buttons", msg))

    by_cat = {c: 0 for c in CATEGORIES}
    for cat, _ in violations:
        by_cat[cat] += 1
    total = len(violations)

    if "--json" in sys.argv:
        print(
            json.dumps(
                {
                    "by_category": by_cat,
                    "count": total,
                    "violations": [msg for _, msg in violations],
                },
                ensure_ascii=False,
                indent=2,
            )
        )
        return 0 if total == 0 else 1

    print("hygiene violations by category:")
    for c in CATEGORIES:
        print(f"  {c:<12}: {by_cat[c]}")
    if violations:
        print()
        for _, msg in violations:
            print(" [x] " + msg)
        print(f"\nviolations: {total}")
        return 1
    print("violations: 0")
    return 0


if __name__ == "__main__":
    sys.exit(main())
