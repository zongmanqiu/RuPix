//! 文件关联（m244）：把 RuPix 写成后缀的默认打开方式。
//!
//! 修宪条款（2026-11-07 用户拍板，2026-11-11 m314/m315 修订）：**注册表的唯一例外 =
//! 文件关联**——只写 HKCU、只删自己写的键。**「永不后台复查」已被推翻**，但改成了
//! **opt-in**：勾选的后缀落进配置（`crate::core::config::Config::assoc_exts`），
//! 只有在 `Config::assoc_heal`（默认**关**）被用户勾上时，运行期才定时核对、被改掉就
//! 重写回来（[`audit`] / [`heal`]）。本模块是这个例外唯一的落点；除它之外任何代码
//! 碰注册表都违宪章。
//!
//! # 哈希直写 + 兜底候选（2026-11-10 翻案后的现行策略）
//!
//! UserChoice 的 Hash 是微软的防篡改锁——但**它是可以按公开算法帧算出来的**
//! （shell32 的 setfta.cs 口径，本机 6/6 盘上样本逐字节命中，见
//! `docs/默认打开方式策略-UserChoice哈希.md`）。2026-11-07 时段的「没人打赢过」
//! 结论经动态取证翻案：算法帧有效，旧探针失败的根因在写入侧时序（复用了旧键的
//! filetime）。
//!
//! 现行行为分三路（写单个后缀时）：
//! - UserChoice 不存在 / 无哈希：legacy 直写 + ProgId 直接改（一次生效）；
//! - UserChoice 带哈希且**已是 RuPix**：行为已正确，一字不动（2026-11-07 实测
//!   90 个「被占」里 49 个是这种）；
//! - UserChoice 带哈希且指向**别人**：按 §四 时序算哈希直写——删旧 UserChoice 子键
//!   → 新建 → 取新键 LastWriteTime → 算 Hash → 写 ProgId + Hash；同时 legacy 候选
//!   照挂（ProgId command + `.ext` 默认值 + `OpenWithProgids`）。**若哈希写入任何
//!   一步失败**（删键被拒、取 SID 失败等），退回候选模式：UserChoice 不碰，
//!   用户右键 → 打开方式 → RuPix → 始终，一次手动完成。
//!
//! legacy 三件套只是兜底候选，不参与 UserChoice 判定——Hash 校验的是
//! UserChoice ProgId + 写入时刻，与 `.ext` 默认值、OpenWithProgids 无关。
//!
//! # 加固（2026-11-11 二轮重审：`.plan/win11-default-app-gimp-case.md` + `默认打开方式-二次重审.md`）
//!
//! 五条经验证的修正，专治「设了但没生效 / 生效了打不开」：
//! 1. **壳必须先完整落地**：`shell\open\command` + `DefaultIcon`。不写图标 → 白纸。
//! 2. **先自检壳、再写默认值**：command 读不回来就不把 `.ext` 默认值指向它——
//!    悬空默认值（C6/C11）比不写更坏；半写残留顺手回滚。
//! 3. **UserChoice 写完读回自检**：ProgId/Hash 不符即退候选，绝不假装成功。
//! 4. **UCPD 保护后缀（`.pdf` 等）跳过哈希直写**：UCPD 拦删也拦写，删成功写失败会把
//!    用户既有默认弄丢且补不回（哈希锁死时间戳）。只挂候选，不动其 UserChoice。
//! 5. **抑制「选择默认应用」气泡**（`ApplicationAssociationToasts`，PS-SFTA 同款）。
//!
//! 另：已指向我们（`AlreadyOurs`）时**不动 UserChoice**，但 legacy 三件套照样补齐——
//! 自愈把 `.ext` 默认值 / command 路径 / 图标 / 候选任一项缺失都算破损，这里不补就
//! 成了「判破 → 零写入 → 下一轮还破」的死循环。
//!
//! # 勾选 + opt-in 自愈（2026-11-11 m314 建、m315 收窄）
//!
//! 勾选不再是"一次性命令"，而是**记住意图**：勾中的后缀记进
//! [`crate::core::config::Config::assoc_exts`]，开窗按它预勾选，「应用」= 保存并立即
//! 写一遍。**是否持续维护是另一个开关**（[`crate::core::config::Config::assoc_heal`]，
//! 默认关）：开着才定时核对（启动满 60 秒首查、之后每 60 分钟），被别的软件或系统
//! 改掉就重写回来；关着就只写那一次。
//!
//! ★ 为什么不能"勾了就自动维护"：自愈分不清"别的程序抢走了默认"和"用户自己在系统
//! 设置里改成了别的程序"——两种都是系统写的合法哈希。抢回必须是用户显式授权。
//!
//! 三条不变式：
//! - **取消勾选 = 退订，不撤销**：撤销得知道还原成哪个程序，我们不知道，删掉只会把
//!   用户推到"双击弹选择器"的更差状态。注册表原样保留。
//! - **执行范围 = 勾选表 ∩ 当前可用后缀**（[`crate::core::formats::declared_exts`] 已
//!   过滤掉被关闭与已卸载的解码器）。解码器临时关掉**不删勾选**——一开一关不能丢。
//! - **只写不删**：永不提供"摘除关联"，悬空由 Windows 回落「选择打开方式」兜底。

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegDeleteValueW, RegOpenKeyExW,
    RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
    KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPEN_CREATE_OPTIONS, REG_SZ,
};
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

use crate::platform::{pipe_dacl, userchoice_hash};

/// 生产环境的两个根： Classes 合并视图的用户层 + 每后缀 FileExts。
const CLASSES_ROOT: &str = r"Software\Classes";
const FILEEXTS_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts";
/// 能力登记（让 RuPix 出现在系统「设置 → 默认应用」列表里）：**只登记、不抢默认**。
/// `RegisteredApplications` 的值数据是 Capabilities 键的路径（相对 HKCU）。
const REGISTERED_ROOT: &str = r"Software\RegisteredApplications";
const CAPS_ROOT: &str = r"Software\RuPix\Capabilities";
const APP_NAME: &str = "RuPix";
/// 抑制「选择默认应用」提示气泡的值仓库（PS-SFTA 同款，值名 `<ProgId>_.<ext>`，
/// REG_DWORD 0）。纯体验优化，写失败不影响关联本身。
const TOASTS_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\ApplicationAssociationToasts";

/// 受 UCPD（User Choice Protection Driver）保护的后缀：内核级驱动同时拦截对
/// UserChoice 的**写入与删除**。对这些后缀若仍走「删旧键→重建→写哈希」，一旦删成功、
/// 写被拦，用户的既有默认就被抹掉且补不回来（哈希把时间戳锁死）。所以一律跳过哈希
/// 直写、只挂候选。名单来自公开逆向（2025-07 v4.3 扩到 Office/HTML），按后缀判。
fn uc_protected(ext: &str) -> bool {
    // 名单只列已公开实证的：http/https（协议，非后缀）+ `.pdf`，2025-07 v4.3
    // 扩到 `.htm/.html/.doc/.docx/.xls/.xlsx/.ppt/.pptx`。多写一个未证实的后缀
    // 只会白丢一条直写能力，故不臆测。
    matches!(
        ext,
        "pdf" | "htm" | "html" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
    )
}

/// 该后缀是否受 UCPD 保护——给 UI 用的只读门面。
///
/// 这些后缀勾了也只能把 RuPix 挂进右键「打开方式」候选，默认写不进去
/// （内核拦 UserChoice 的删与写）。文件关联窗据此把它们标灰并给一句解释。
pub fn is_system_protected(ext: &str) -> bool {
    normalize(ext).is_some_and(|e| uc_protected(&e))
}

/// 一个后缀的规范化：去空白、去前导点、小写。非法（空 / 非 [0-9a-z-_]）返回 None。
pub fn normalize(ext: &str) -> Option<String> {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if e.is_empty()
        || !e
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(e)
}

/// 自建 ProgId：`RuPix.<ext>`（短、可预测、按后缀隔离——摘账与判"是不是我们的"都靠它）。
pub fn progid(ext: &str) -> String {
    format!("RuPix.{ext}")
}

/// 「应用」的结果三栏：
/// - `ok`：默认已程序化写成（`.ext` 默认值 + ProgId + UserChoice——无哈希直改 /
///   带哈希但指向别人时按 §四 时序自算哈希直写）；
/// - `already`：带哈希但已是 RuPix（用户此前设过我们），行为已正确，不动（2026-11-07
///   实测纠正：90 个"被占"里 49 个其实是这种）；
/// - `manual`：哈希锁下指向别人、且哈希直写失败（删键被拒 / 取 SID 失败）——
///   UserChoice 一字不动，legacy 候选照挂，用户右键「打开方式」→ RuPix → 始终，
///   一次手动完成。
#[derive(Default, Debug, PartialEq)]
pub struct Outcome {
    pub ok: Vec<String>,
    /// 带哈希但已是 RuPix：跳过写入、计入成功侧的注脚。
    pub already: Vec<String>,
    /// 哈希锁下指向别人且直写失败：UserChoice 不碰，legacy 候选照挂，差一次手动。
    pub manual: Vec<String>,
}

impl Outcome {
    /// 结果窗的正文：成功多少 + 失败清单（用户 2026-11-07 口径：只列名字，不解释机制）。
    pub fn report(&self) -> String {
        let mut s = format!("成功关联 {} 个格式。", self.ok.len());
        if !self.already.is_empty() {
            s.push_str(&format!(
                "\n\n另有 {} 个本就是 RuPix 默认，未改动：\n{}",
                self.already.len(),
                self.already.join("、")
            ));
        }
        if !self.manual.is_empty() {
            s.push_str(&format!(
                "\n\n以下 {} 个未能自动改写（改默认被系统拒绝），已把 RuPix 加入「打开方式」候选——右键任一此类文件 → 打开方式 → RuPix 并选「始终」即完成关联：\n{}",
                self.manual.len(),
                self.manual.join("、")
            ));
        }
        s
    }
}

/// 把 `exts` 全部写成默认打开方式。一次批量、一次外壳通知。
///
/// 本函数只管**写**：复查与重写由 [`audit`] / [`heal`] 承担（m314），调用点仍是
/// 文件关联窗的「应用」与自愈线程。
pub fn apply(exts: &[String]) -> Outcome {
    apply_with(CLASSES_ROOT, FILEEXTS_ROOT, TOASTS_ROOT, exts)
}

/// 能力登记（2026-11-07 用户拍板）：把 RuPix 与本次关联过的后缀**登记进系统
/// 「设置 → 默认应用」的候选列表**。只登记、不抢默认——用户在那边点一下，
/// Windows 自己写合法 UserChoice 哈希（这是 25H2 唯一稳的路径）。
///
/// 写入四处（全部 HKCU、免管理员、修宪例外内）：
/// `Software\RegisteredApplications` 的 `RuPix` 指向 Capabilities 键；
/// Capabilities 的 `ApplicationName` / `FileAssociations`（每个 `.ext` → `RuPix.<ext>`）；
/// `Applications\<exe>\SupportedTypes`（右键打开方式候选枚举）与它的 open command。
/// 每个后缀的 ProgId 也在这里补写一遍，保证 `FileAssociations` 指向的 ProgId 一定存在
/// （「本就是默认」那栏跳过 write_legacy，不能让它们指向空 ProgId）。
pub fn register_capabilities(exts: &[String]) {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) =
                register_capabilities_with(CLASSES_ROOT, REGISTERED_ROOT, CAPS_ROOT, &exe, exts)
            {
                log::warn!("[assoc] 能力登记失败：{e}");
            } else {
                // 让资源管理器/设置页立刻重读候选；apply 只在 ok 非空时发通知，
                // 这里补齐「勾选的全是已默认 / 被占」那种零直写的场景。
                unsafe {
                    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
                }
            }
        }
        Err(e) => log::warn!("[assoc] 取 exe 路径失败，跳过能力登记：{e}"),
    }
}

/// 与实现共用的可测版本：根可替换（测试写 `Software\RuPixAssocTest*` 自留地）。
fn register_capabilities_with(
    classes_root: &str,
    registered_root: &str,
    caps_root: &str,
    exe: &Path,
    exts: &[String],
) -> Result<(), &'static str> {
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return Err("exe 文件名不可用");
    };
    // ① 登记入口：值数据 = Capabilities 键路径（Windows 按这个路径去读候选清单）。
    set_string(registered_root, Some(APP_NAME), caps_root)
        .map_err(|_| "RegisteredApplications 写入失败")?;
    set_string(caps_root, Some("ApplicationName"), APP_NAME)
        .map_err(|_| "ApplicationName 写入失败")?;
    set_string(caps_root, Some("ApplicationDescription"), "RuPix 看图器")
        .map_err(|_| "ApplicationDescription 写入失败")?;
    // ② `Applications\<exe>` 的 open command：系统候选点开的就是这条命令。
    let quoted = format!("\"{}\" \"%1\"", exe.display());
    set_string(
        &format!("{classes_root}\\Applications\\{name}\\shell\\open\\command"),
        None,
        &quoted,
    )
    .map_err(|_| "Applications open command 写入失败")?;

    let mut seen = std::collections::BTreeSet::new();
    for raw in exts {
        let Some(ext) = normalize(raw) else { continue };
        if !seen.insert(ext.clone()) {
            continue;
        }
        let pid = progid(&ext);
        // ProgId 实体（与 write_legacy 同源、幂等）。
        write_progid(classes_root, exe, &ext, &pid)?;
        // ③ FileAssociations：每个 `.ext` → 我们的 ProgId（系统默认应用列表据此列我们）。
        set_string(
            &format!("{caps_root}\\FileAssociations"),
            Some(&format!(".{ext}")),
            &pid,
        )
        .map_err(|_| "FileAssociations 写入失败")?;
        // ④ SupportedTypes：右键「打开方式」枚举用（YeImageViewer 同款，值为空串）。
        set_string(
            &format!("{classes_root}\\Applications\\{name}\\SupportedTypes"),
            Some(&format!(".{ext}")),
            "",
        )
        .map_err(|_| "SupportedTypes 写入失败")?;
    }
    Ok(())
}

/// 测试与实现共用的工作函数：两个根可替换（测试写进 `Software\RuPixAssocTest*`）。
fn apply_with(
    classes_root: &str,
    fileexts_root: &str,
    toasts_root: &str,
    exts: &[String],
) -> Outcome {
    let mut out = Outcome::default();
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[assoc] 取 exe 路径失败：{e}；本次应用整体放弃");
            out.manual = exts.iter().filter_map(|e| normalize(e)).collect();
            return out;
        }
    };
    let mut seen = std::collections::BTreeSet::new();
    for raw in exts {
        let Some(ext) = normalize(raw) else { continue };
        if !seen.insert(ext.clone()) {
            continue;
        }
        match write_one(classes_root, fileexts_root, toasts_root, &exe, &ext) {
            Ok(Written::AlreadyOurs) => out.already.push(ext),
            Ok(Written::Default) => out.ok.push(ext),
            Ok(Written::CandidateOnly) => out.manual.push(ext),
            Err(reason) => {
                log::warn!("[assoc] {ext} 未写入：{reason}");
                out.manual.push(ext);
            }
        }
    }
    if !out.ok.is_empty() {
        // 让资源管理器立刻重读关联；一次批量一次通知。
        unsafe {
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
        }
        log::info!(
            "[assoc] 已关联 {} 个格式（本就是 {} / 候选挂载 {}）",
            out.ok.len(),
            out.already.len(),
            out.manual.len()
        );
    }
    out
}

/// 单个后缀的写入结果。
enum Written {
    /// 带哈希但已指向 RuPix：行为已正确，不动。
    AlreadyOurs,
    /// 本次把默认写成 RuPix（无哈希直写 / 哈希直写成功都算这条）。
    Default,
    /// 哈希锁下指向别人、且哈希直写失败（删键被拒 / 取 SID 失败）：UserChoice
    /// 不碰，legacy 候选照挂，差一次手动。
    CandidateOnly,
}

/// 写单个后缀。任何一步注册表失败都算这个后缀失败，**不留半写状态重试**（下次应用重来）。
fn write_one(
    classes_root: &str,
    fileexts_root: &str,
    toasts_root: &str,
    exe: &Path,
    ext: &str,
) -> Result<Written, &'static str> {
    let uc = format!("{fileexts_root}\\.{ext}\\UserChoice");
    let pid = progid(ext);
    // UserChoice 带哈希 = 系统上锁（2026-11-10 翻案：锁可按公开算法帧算出来）。
    // 锁下已是 RuPix（`Applications\<exe>` 是系统"始终以此打开"造的通用 ProgId，
    // `RuPix.<ext>` 是我们自己写的）→ 行为已正确，不动；
    // 锁下是别人 → 哈希直写（§四 时序）；写失败才退候选（见下）。
    if get_string(&uc, "Hash").is_some() {
        if get_string(&uc, "ProgId")
            .map(|p| progid_is_ours(&p, ext, exe))
            .unwrap_or(false)
        {
            // 已指向我们：UserChoice 一字不动（它的 ProgId/Hash 已经是对的），但
            // legacy 三件套**必须补齐**。原实现只在 ProgId 是自建形态时补壳，理由
            // 是"已是默认就不用动"；自愈（m314）推翻了这个前提——`audit` 把
            // `.ext` 默认值 / command 路径 / 图标 / 候选任一缺失都判破损，若这里
            // 什么都不写，就变成「判破 → 零写入 → 下一轮还破」的死循环。
            // 补齐不碰 UserChoice，所以哈希锁不受影响。
            write_legacy(classes_root, toasts_root, exe, ext, &pid)?;
            return Ok(Written::AlreadyOurs);
        }
        // legacy 候选先挂（ProgId command + `.ext` 默认值 + OpenWithProgids）；
        // 哈希直写成功与否都不影响这三样——它们是哈希失败时的兜底通道。
        write_legacy(classes_root, toasts_root, exe, ext, &pid)?;
        // UCPD 保护名单：删/写 UserChoice 都会被内核拦，且删了可能写不回。跳过直写、
        // 保持用户既有选择不动，只留候选（与「哈希算得对不关 UCPD 的事」一致）。
        if uc_protected(ext) {
            log::info!("[assoc] {ext} 受 UCPD 保护，跳过 UserChoice 直写，只挂候选");
            return Ok(Written::CandidateOnly);
        }
        let Some(sid) = pipe_dacl::current_user_sid_string() else {
            log::warn!("[assoc] {ext} 取当前用户 SID 失败，退候选模式");
            return Ok(Written::CandidateOnly);
        };
        match write_userchoice_selfhashed(&uc, ext, &pid, &sid) {
            Ok(()) => return Ok(Written::Default),
            Err(reason) => {
                log::warn!("[assoc] {ext} 哈希直写失败（{reason}），退候选模式");
                return Ok(Written::CandidateOnly);
            }
        }
    }
    write_legacy(classes_root, toasts_root, exe, ext, &pid)?;
    // UserChoice 存在但无哈希（老系统 / 未被系统 UI 上锁）：直改 ProgId 才真生效。
    if let Some(old) = get_string(&uc, "ProgId") {
        if old != pid {
            set_string(&uc, Some("ProgId"), &pid).map_err(|_| "UserChoice 直改失败")?;
        }
    }
    Ok(Written::Default)
}

/// UserChoice 哈希直写（§四 时序，2026-11-10 动态取证确证）：
/// **① 删旧 UserChoice 子键 → ② 新建 → ③ 取新键 LastWriteTime → ④ 算 Hash →
/// ⑤ 写 ProgId + Hash**。
///
/// 新键的 filetime 每次新写都是新鲜的，哈希与之自洽——旧键的 filetime 已被它自己的
/// 旧值锁死，复用必被 Explorer 判不符 → 重置。命名值（ProgId/Hash）的写入不改键的
/// LastWriteTime，所以 ③ 到 ⑤ 之间的文件时间戳零漂移。
///
/// 任何一步失败返回 Err：调用方退候选模式（UserChoice 保持原样），不留半写状态。
fn write_userchoice_selfhashed(
    uc: &str,
    ext: &str,
    pid: &str,
    sid: &str,
) -> Result<(), &'static str> {
    // ① 删旧子键——旧 Hash 与旧 ProgId 一起带走。
    let err = unsafe {
        RegDeleteKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(uc).as_ptr()),
            0,
            None,
        )
    };
    if err != ERROR_SUCCESS {
        return Err("删旧 UserChoice 失败（权限或被占用）");
    }
    // ② 新建（KEY_READ 供 ③ 查信息）。
    let mut hk = HKEY::default();
    let err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(uc).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPEN_CREATE_OPTIONS(0),
            KEY_READ | KEY_SET_VALUE,
            None,
            &mut hk,
            None,
        )
    };
    if err != ERROR_SUCCESS {
        return Err("新建 UserChoice 失败");
    }
    // ③ 取新键 LastWriteTime——哈希的盐。只有新建这一刻才新鲜。
    let mut ft = FILETIME::default();
    let err = unsafe {
        RegQueryInfoKeyW(
            hk,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut ft),
        )
    };
    let info_ok = err == ERROR_SUCCESS;
    unsafe {
        let _ = RegCloseKey(hk);
    }
    if !info_ok {
        return Err("取 UserChoice LastWriteTime 失败");
    }
    let filetime = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
    let hash = userchoice_hash::hash_for(ext, pid, filetime, sid);
    // ④⑤ 写 ProgId + Hash。只设两个命值、绝不碰默认值——默认值写入会改键的
    // LastWriteTime，把 ③ 的盐废掉。
    set_string(uc, Some("ProgId"), pid).map_err(|_| "UserChoice ProgId 写入失败")?;
    set_string(uc, Some("Hash"), &hash).map_err(|_| "UserChoice Hash 写入失败")?;
    // 读回自检：两个值必须与刚写的一致（UCPD 拦截 / 权限不足时写会「成功」返回但
    // 不落地）。不符则报错退候选——绝不假装成功，让上层照实分类。
    if get_string(uc, "ProgId").as_deref() != Some(pid)
        || get_string(uc, "Hash").as_deref() != Some(hash.as_str())
    {
        return Err("UserChoice 写入后读回不符");
    }
    Ok(())
}

/// legacy 三件套：ProgId 的 open command 与友好名、`.ext` 默认值、「打开方式」候选。
/// 哈希锁只锁 UserChoice，这三样永远可写——被占后缀靠它们进右键列表。
fn write_legacy(
    classes_root: &str,
    toasts_root: &str,
    exe: &Path,
    ext: &str,
    pid: &str,
) -> Result<(), &'static str> {
    write_progid(classes_root, exe, ext, pid)?;
    // 写默认值之前先自检壳：`shell\open\command` 必须读得回来且非空。读不回来说明
    // 壳没落地——此时绝不能把默认值指向它（悬空默认值 = 文档 C6/C11，比不写更坏）。
    let cmd = get_string(&format!("{classes_root}\\{pid}\\shell\\open\\command"), "");
    if cmd.as_deref().is_none_or(|c| c.trim().is_empty()) {
        // 万一上一轮留了 `.ext\(default)=pid` 这个半写状态，顺手清掉，别留悬空。
        if get_string(&format!("{classes_root}\\.{ext}"), "").as_deref() == Some(pid) {
            let _ = delete_value(&format!("{classes_root}\\.{ext}"), None);
        }
        return Err("ProgId command 自检失败，已回滚默认值");
    }
    // 顺序：壳 → 候选 → 默认值（默认值最后写，前面任何一步失败都不碰它）。
    // 右键「打开方式」候选：默认被别人占时至少列表里有我们。
    set_string(
        &format!("{classes_root}\\.{ext}\\OpenWithProgids"),
        Some(pid),
        "",
    )
    .map_err(|_| "OpenWithProgids 写入失败")?;
    // 默认关联本体：覆盖 `.ext` 默认值（覆盖语义正是用户拍板的行为）。
    set_string(&format!("{classes_root}\\.{ext}"), None, pid)
        .map_err(|_| "扩展名默认值写入失败")?;
    // 抑制「选择默认应用」气泡：纯体验，失败只记日志、不拖垮关联。
    write_toast_suppression(toasts_root, pid, ext);
    Ok(())
}

/// 抑制首次设为默认时系统弹的「选择默认应用」提示气泡（PS-SFTA 同款）：
/// `ApplicationAssociationToasts\<ProgId>_.<ext>` = REG_DWORD 0。纯体验优化，
/// 任何失败只记日志、绝不影响关联结果。
fn write_toast_suppression(toasts_root: &str, pid: &str, ext: &str) {
    let name = format!("{pid}_.{ext}");
    if let Err(code) = set_dword(toasts_root, Some(&name), 0) {
        log::warn!("[assoc] {ext} 气泡抑制写入失败（错误码 {code}），不影响关联");
    }
}

/// ProgId 实体：open command + 友好名。写入与能力登记共用（后者要保证候选指向的
/// ProgId 一定存在），幂等。
fn write_progid(classes_root: &str, exe: &Path, ext: &str, pid: &str) -> Result<(), &'static str> {
    let quoted = format!("\"{}\" \"%1\"", exe.display());
    set_string(
        &format!("{classes_root}\\{pid}\\shell\\open\\command"),
        None,
        &quoted,
    )
    .map_err(|_| "ProgId command 写入失败")?;
    // 图标：不写这一条，关联成功后缀在资源管理器里仍是白纸（文档 §5.2 明确点出）。
    set_string(
        &format!("{classes_root}\\{pid}\\DefaultIcon"),
        None,
        &format!("\"{}\",0", exe.display()),
    )
    .map_err(|_| "DefaultIcon 写入失败")?;
    set_string(&format!("{classes_root}\\{pid}"), None, &friendly(ext))
        .map_err(|_| "ProgId 名称写入失败")
}

/// UserChoice 的 ProgId 是不是指向 RuPix：认两种形态——
/// `Applications\<exe 文件名>`（系统「始终以此应用打开」造的通用 ProgId）
/// 与 `RuPix.<ext>`（我们自己写的）。比对一律忽略大小写。
fn progid_is_ours(pid: &str, ext: &str, exe: &Path) -> bool {
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    pid.eq_ignore_ascii_case(&format!("Applications\\{name}"))
        || pid.eq_ignore_ascii_case(&progid(ext))
}

/// 这个后缀现在是不是已经指向 RuPix（读 `.ext` 默认值比对 ProgId）。
///
/// **当前只由测试覆盖（m282 调用图）**：设置页不再显示「已关联」状态。保留它是因为
/// 私有实现 `is_ours_with` 需要一条生产入口——只剩测试用会更糟。
pub fn is_ours(ext: &str) -> bool {
    is_ours_with(CLASSES_ROOT, ext)
}

fn is_ours_with(classes_root: &str, ext: &str) -> bool {
    let Some(ext) = normalize(ext) else {
        return false;
    };
    get_string(&format!("{classes_root}\\.{ext}"), "").as_deref() == Some(progid(&ext)).as_deref()
}

// ── 运行期核对与自愈（m314） ──────────────────────────────────────────
//
// 「应用」写过一次不等于一直是。用户装的别的软件、系统「重置默认应用」、清理工具都会
// 把关联改掉；能直写 UserChoice 之后我们有能力把它修回来。调度在 `runtime`
// （启动满 60 秒首查、之后每 60 分钟），**核对只读、破才写**——正常态零写入、
// 零 `SHChangeNotify`（那一声会让资源管理器重读关联，属于无谓抖动）。

/// 核对 `exts` 里哪些后缀的关联已经不对了。**只读，不碰注册表。**
///
/// 空返回值 = 一切安好。判据只看「双击会不会真的打开我们」：
/// ① `.ext\(default)` 是本后缀的 ProgId；②③ ProgId 的 `command` 与 `DefaultIcon` 都指向
/// **当前** exe（换目录 / 升级后路径会漂）；④ `OpenWithProgids` 里挂着我们（兜底通道）；
/// ⑤ UserChoice 若存在：ProgId 指我们，且 Hash 与「现键 LastWriteTime + 当前 SID」自洽
/// （哈希就是这个键自己的盐，算得回来说明没人动过它）。
///
/// **刻意不算破损的两样**：气泡抑制值（纯体验，缺了只多弹一次提示）与能力登记
/// （那是"系统设置里看不看得到我们"，不影响双击打开，由 [`caps_intact`] 单独核）。
/// 把它们并进来会让「每小时重写一遍关联」成为常态。
pub fn audit(exts: &[String]) -> Vec<String> {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            // 取不到自身路径就别动任何东西——那是"我找不到我"，不是关联坏了。
            log::warn!("[assoc] 核对跳过：取 exe 路径失败（{e}）");
            return Vec::new();
        }
    };
    audit_with(CLASSES_ROOT, FILEEXTS_ROOT, &exe, exts)
}

fn audit_with(classes_root: &str, fileexts_root: &str, exe: &Path, exts: &[String]) -> Vec<String> {
    // SID 每轮只取一次：90 个后缀逐个取就是 90 次令牌查询。
    let sid = pipe_dacl::current_user_sid_string();
    let mut broken = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for raw in exts {
        let Some(ext) = normalize(raw) else { continue };
        if !seen.insert(ext.clone()) {
            continue;
        }
        if !intact(classes_root, fileexts_root, exe, sid.as_deref(), &ext) {
            broken.push(ext);
        }
    }
    broken
}

fn intact(
    classes_root: &str,
    fileexts_root: &str,
    exe: &Path,
    sid: Option<&str>,
    ext: &str,
) -> bool {
    let pid = progid(ext);
    // ① 默认值：双击这条路真的走它。
    if get_string(&format!("{classes_root}\\.{ext}"), "").as_deref() != Some(pid.as_str()) {
        return false;
    }
    // ②③ 壳的两条值都得指着当前 exe。
    for sub in ["shell\\open\\command", "DefaultIcon"] {
        let ok = get_string(&format!("{classes_root}\\{pid}\\{sub}"), "")
            .as_deref()
            .and_then(first_token)
            .is_some_and(|p| path_eq(p, exe));
        if !ok {
            return false;
        }
    }
    // ④ 右键「打开方式」候选：缺了列表里就没我们（哈希失败时的兜底通道）。
    if get_string(&format!("{classes_root}\\.{ext}\\OpenWithProgids"), &pid).is_none() {
        return false;
    }
    // ⑤ UserChoice。受 UCPD 保护的后缀我们本来就不写它，只审到 ④ 为止——
    //    否则每轮都会判破、每轮白写一遍 legacy，永远修不好。
    if uc_protected(ext) {
        return true;
    }
    let uc = format!("{fileexts_root}\\.{ext}\\UserChoice");
    let Some(uc_pid) = get_string(&uc, "ProgId") else {
        // 没有 UserChoice = 老系统 / 从未被系统 UI 上锁，默认值说话。
        return true;
    };
    if !progid_is_ours(&uc_pid, ext, exe) {
        return false;
    }
    let Some(hash) = get_string(&uc, "Hash") else {
        return true; // 有 ProgId 没 Hash：未上锁形态，Explorer 不看哈希。
    };
    // 有 Hash 就必须自洽。拿不到 SID 或时间戳时**按良好处理**：
    // 一次取不到就判破，会退化成每小时重写一遍。
    let (Some(sid), Some(ft)) = (sid, key_lastwrite(&uc)) else {
        return true;
    };
    userchoice_hash::hash_for(ext, &uc_pid, ft, sid) == hash
}

/// 能力登记还在不在（系统「设置 → 默认应用」里能不能看到我们）。
///
/// 单独核而不并进 [`audit`]：这组键丢了不影响双击打开，只影响用户去系统设置里挑我们；
/// 但被清理工具 / 系统重置抹掉后不会自己回来，所以自愈要能补上。
fn caps_intact(
    classes_root: &str,
    registered_root: &str,
    caps_root: &str,
    exe: &Path,
    exts: &[String],
) -> bool {
    if get_string(registered_root, APP_NAME).as_deref() != Some(caps_root) {
        return false;
    }
    if get_string(caps_root, "ApplicationName").as_deref() != Some(APP_NAME) {
        return false;
    }
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let mut seen = std::collections::BTreeSet::new();
    for raw in exts {
        let Some(ext) = normalize(raw) else { continue };
        if !seen.insert(ext.clone()) {
            continue;
        }
        let pid = progid(&ext);
        let dotted = format!(".{ext}");
        if get_string(&format!("{caps_root}\\FileAssociations"), &dotted).as_deref()
            != Some(pid.as_str())
        {
            return false;
        }
        if get_string(
            &format!("{classes_root}\\Applications\\{name}\\SupportedTypes"),
            &dotted,
        )
        .is_none()
        {
            return false;
        }
    }
    true
}

/// 自愈：把「核对不过」的东西写回去。返回本次重写的后缀数（0 = 一切安好）。
///
/// 由 `runtime` 定时驱动（启动满 60 秒首查、之后每 60 分钟）。**调用方要放在后台
/// 线程**：`apply` 末尾会发 `SHChangeNotify`，那一声会让资源管理器重读关联，
/// 不该落在 16 ms 的帧里。
pub fn heal(exts: &[String]) -> usize {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return 0,
    };
    let broken = audit(exts);
    let caps_ok = caps_intact(CLASSES_ROOT, REGISTERED_ROOT, CAPS_ROOT, &exe, exts);
    if broken.is_empty() && caps_ok {
        return 0;
    }
    // 先登记能力再补前缀：`write_legacy` 要求 ProgId 壳先落地（前面有自检），
    // 登记那条路正好把壳写全。
    if !caps_ok {
        log::info!("[assoc] 自愈：能力登记缺失，重新登记 {} 个后缀", exts.len());
        register_capabilities(exts);
    }
    if !broken.is_empty() {
        log::info!(
            "[assoc] 自愈：{} 个后缀的关联被改动，重写（{}）",
            broken.len(),
            broken.join("、")
        );
        let _ = apply(&broken);
    }
    broken.len()
}

/// 从 command / 图标串里取第一个路径：`"C:\x\rupix.exe" "%1"` → `C:\x\rupix.exe`。
/// 带引号按引号取；不带引号按第一个空白切（老写法）。
fn first_token(s: &str) -> Option<&str> {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix('"') {
        return rest.find('"').map(|i| &rest[..i]);
    }
    match t.split_whitespace().next() {
        Some(p) if !p.is_empty() => Some(p),
        _ => None,
    }
}

fn path_eq(p: &str, exe: &Path) -> bool {
    p.eq_ignore_ascii_case(&exe.to_string_lossy())
}

fn friendly(ext: &str) -> String {
    format!("RuPix 图片（{ext}）")
}

// ── 注册表薄封装：全部 HKCU，路径拼接手写 ──────────────────────────────

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_key(path: &str, write: bool) -> Result<HKEY, u32> {
    let mut hk = HKEY::default();
    let err = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(path).as_ptr()),
            None,
            if write {
                KEY_SET_VALUE
            } else {
                KEY_QUERY_VALUE
            },
            &mut hk,
        )
    };
    if err != ERROR_SUCCESS {
        Err(err.0)
    } else {
        Ok(hk)
    }
}

/// 读一个键的 `LastWriteTime`（UserChoice 哈希的盐）。
///
/// 必须用 `KEY_READ` 打开：`RegQueryInfoKeyW` 的 LastWriteTime 是读权限下的信息，
/// 只给 `KEY_SET_VALUE` 查不出来（2026-11-11 实测）。
fn key_lastwrite(path: &str) -> Option<u64> {
    let mut hk = HKEY::default();
    let err = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(path).as_ptr()),
            None,
            KEY_READ,
            &mut hk,
        )
    };
    if err != ERROR_SUCCESS {
        return None;
    }
    let mut ft = FILETIME::default();
    let err = unsafe {
        RegQueryInfoKeyW(
            hk,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut ft),
        )
    };
    unsafe {
        let _ = RegCloseKey(hk);
    };
    if err != ERROR_SUCCESS {
        return None;
    }
    Some(((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64))
}

/// 读一个字符串值（`value` 传 `""` = 默认值）。不存在 / 非字符串 → None。
fn get_string(path: &str, value: &str) -> Option<String> {
    let hk = open_key(path, false).ok()?;
    let out = get_string_on(hk, value);
    unsafe {
        let _ = RegCloseKey(hk);
    }
    out
}

fn get_string_on(hk: HKEY, value: &str) -> Option<String> {
    let name = wide(value);
    let mut ty = REG_SZ;
    let mut len: u32 = 0;
    let err = unsafe {
        RegQueryValueExW(
            hk,
            PCWSTR::from_raw(name.as_ptr()),
            None,
            Some(&mut ty),
            None,
            Some(&mut len),
        )
    };
    if err != ERROR_SUCCESS || ty != REG_SZ || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let err = unsafe {
        RegQueryValueExW(
            hk,
            PCWSTR::from_raw(name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut len),
        )
    };
    if err != ERROR_SUCCESS {
        return None;
    }
    // len 含结尾 NUL；按 UTF-16 解到第一个 NUL。
    let u16s: Vec<u16> = buf[..len as usize]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&u16s))
}

/// 写一个字符串值，**沿途的中间键不存在就创建**（ProgId 三层、OpenWithProgids 一层都靠它）。
fn set_string(path: &str, value: Option<&str>, data: &str) -> Result<(), u32> {
    let mut hk = HKEY::default();
    let err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(path).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPEN_CREATE_OPTIONS(0),
            KEY_SET_VALUE,
            None,
            &mut hk,
            None,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(err.0);
    }
    let name = wide(value.unwrap_or(""));
    let mut bytes = Vec::with_capacity((data.len() + 1) * 2);
    for u in data.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let err = unsafe {
        RegSetValueExW(
            hk,
            PCWSTR::from_raw(name.as_ptr()),
            None,
            REG_SZ,
            Some(&bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(hk);
    }
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(err.0)
    }
}

/// 删一个值（`value` 传 `None`/空 = 默认值）。只给「回滚悬空默认值」这一处用；
/// 键不存在 / 值不存在都当成功（幂等）。
fn delete_value(path: &str, value: Option<&str>) -> Result<(), u32> {
    let mut hk = HKEY::default();
    let err = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(path).as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut hk,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(err.0);
    }
    let name = wide(value.unwrap_or(""));
    let err = unsafe { RegDeleteValueW(hk, PCWSTR::from_raw(name.as_ptr())) };
    unsafe {
        let _ = RegCloseKey(hk);
    }
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(err.0)
    }
}

/// 写一个 REG_DWORD 值（沿途中间键不存在就创建）。目前只给气泡抑制用。
fn set_dword(path: &str, value: Option<&str>, data: u32) -> Result<(), u32> {
    let mut hk = HKEY::default();
    let err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(path).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPEN_CREATE_OPTIONS(0),
            KEY_SET_VALUE,
            None,
            &mut hk,
            None,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(err.0);
    }
    let name = wide(value.unwrap_or(""));
    let bytes = data.to_le_bytes();
    let err = unsafe {
        RegSetValueExW(
            hk,
            PCWSTR::from_raw(name.as_ptr()),
            None,
            REG_DWORD,
            Some(&bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(hk);
    }
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(err.0)
    }
}

/// 删一整棵子树（**只给测试清理用**：生产路径永远只写不删）。
#[cfg(test)]
mod cleaner {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};

    pub fn delete_tree(path: &str) {
        let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR::from_raw(w.as_ptr()));
        }
    }
}
#[cfg(test)]
use cleaner::delete_tree;

#[cfg(test)]
mod tests {
    use super::*;

    // 测试根：真实的 HKCU，但全在自留地 `Software\RuPixAssocTest*` 下，前后各清一次。
    const T_CLASSES: &str = r"Software\RuPixAssocTest";
    const T_FILEEXTS: &str = r"Software\RuPixAssocTestFE";
    const T_REGISTERED: &str = r"Software\RuPixAssocTestReg";
    const T_CAPS: &str = r"Software\RuPixAssocTestCaps";
    const T_TOASTS: &str = r"Software\RuPixAssocTestToasts";

    fn cleanup() {
        delete_tree(T_CLASSES);
        delete_tree(T_FILEEXTS);
        delete_tree(T_REGISTERED);
        delete_tree(T_CAPS);
        delete_tree(T_TOASTS);
    }

    #[test]
    fn capabilities_registration_lists_every_ext_and_open_command() {
        cleanup();
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap();
        register_capabilities_with(
            T_CLASSES,
            T_REGISTERED,
            T_CAPS,
            &exe,
            &["jpg".into(), ".PNG".into(), "jpg".into(), "  ".into()],
        )
        .unwrap();
        // 登记入口指向 Capabilities 键路径；名字在。
        assert_eq!(get_string(T_REGISTERED, APP_NAME).as_deref(), Some(T_CAPS));
        assert_eq!(
            get_string(T_CAPS, "ApplicationName").as_deref(),
            Some("RuPix")
        );
        // 规范化 + 去重：jpg / png 各一条，空串被丢；值指向我们的 ProgId。
        assert_eq!(
            get_string(&format!("{T_CAPS}\\FileAssociations"), ".jpg").as_deref(),
            Some("RuPix.jpg")
        );
        assert_eq!(
            get_string(&format!("{T_CAPS}\\FileAssociations"), ".png").as_deref(),
            Some("RuPix.png")
        );
        // SupportedTypes 枚举 + open command + ProgId 实体（候选不能指向空 ProgId）。
        assert_eq!(
            get_string(
                &format!("{T_CLASSES}\\Applications\\{name}\\SupportedTypes"),
                ".jpg"
            ),
            Some(String::new())
        );
        let cmd = get_string(
            &format!("{T_CLASSES}\\Applications\\{name}\\shell\\open\\command"),
            "",
        )
        .expect("open command 在");
        assert!(cmd.contains("\" \"%1\"") && cmd.contains(name), "{cmd}");
        assert!(get_string(&format!("{T_CLASSES}\\RuPix.jpg\\shell\\open\\command"), "").is_some());
        cleanup();
    }

    #[test]
    fn normalize_strips_dots_case_and_junk() {
        assert_eq!(normalize(" JPG ").as_deref(), Some("jpg"));
        assert_eq!(normalize(".Cr3").as_deref(), Some("cr3"));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("a b"), None);
        assert_eq!(normalize("jp2-k").as_deref(), Some("jp2-k"));
    }

    #[test]
    fn apply_writes_progid_command_and_default() {
        cleanup();
        let out = apply_with(
            T_CLASSES,
            T_FILEEXTS,
            T_TOASTS,
            &["jpg".into(), ".PNG".into()],
        );
        assert_eq!(out.ok.len(), 2, "两个都成功：{:?}", out);
        assert!(out.manual.is_empty());
        // 默认值指向我们的 ProgId。
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.jpg"), "").as_deref(),
            Some("RuPix.jpg")
        );
        // ProgId 的 open command 带 exe 路径与 "%1"。
        let cmd = get_string(&format!("{T_CLASSES}\\RuPix.jpg\\shell\\open\\command"), "")
            .expect("command 在");
        assert!(
            cmd.contains("\" \"%1\"") && cmd.starts_with('"'),
            "cmd={cmd}"
        );
        // 打开方式候选已挂。
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.jpg\\OpenWithProgids"), "RuPix.jpg"),
            Some(String::new())
        );
        // 图标：`DefaultIcon` 指向本 exe（否则关联成功图标仍是白纸）。
        let icon = get_string(&format!("{T_CLASSES}\\RuPix.jpg\\DefaultIcon"), "")
            .expect("DefaultIcon 应在");
        assert!(icon.contains(&exe_name()) && icon.ends_with(",0"), "{icon}");
        // 气泡抑制：`<ProgId>_.<ext>` = DWORD 0（值名格式与真实系统一致）。
        assert_eq!(get_dword(T_TOASTS, "RuPix.jpg_.jpg"), Some(0));
        // is_ours 双向。
        assert!(is_ours_with(T_CLASSES, "jpg"));
        assert!(!is_ours_with(T_CLASSES, "tif"));
        cleanup();
    }

    fn exe_name() -> String {
        std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn hash_protected_userchoice_recomputed_and_written() {
        cleanup();
        // 造一个带哈希的受保护 UserChoice 指向别人：2026-11-10 翻案后，
        // 这次应被 §四 时序直写——删旧键、建新键、取新 filetime、算哈希、写回。
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("Hash"),
            "abc",
        )
        .unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("ProgId"),
            "Other.app",
        )
        .unwrap();
        let out = apply_with(
            T_CLASSES,
            T_FILEEXTS,
            T_TOASTS,
            &["jpg".into(), "png".into()],
        );
        // 被占的 jpg 也应算成功（哈希战打赢了），不是 manual。
        assert_eq!(
            out.ok,
            vec!["jpg".to_string(), "png".to_string()],
            "{out:?}"
        );
        assert!(out.manual.is_empty(), "{out:?}");
        // UserChoice 已改写：ProgId 是我们的；Hash 不再是 "abc"。
        let got_pid = get_string(&format!("{T_FILEEXTS}\\.jpg\\UserChoice"), "ProgId")
            .expect("UserChoice ProgId 应在");
        assert_eq!(got_pid, "RuPix.jpg");
        // 强判据：盘上 Hash 必须与「现键 LastWriteTime + 真实当前 SID」自洽——
        // 锁死 §四 时序（删建后取 ft 再算，不是拿旧 ft 凑合）。
        let got_hash = get_string(&format!("{T_FILEEXTS}\\.jpg\\UserChoice"), "Hash")
            .expect("UserChoice Hash 应在");
        assert_ne!(got_hash, "abc");
        let ft = read_unsigned_lastwritetime(&format!("{T_FILEEXTS}\\.jpg\\UserChoice"));
        let sid =
            crate::platform::pipe_dacl::current_user_sid_string().expect("测试进程应能取 SID");
        assert_eq!(
            got_hash,
            userchoice_hash::hash_for("jpg", "RuPix.jpg", ft, &sid),
            "盘上 Hash 必须与现键 LastWriteTime 自洽（§四 时序）"
        );
        // legacy 三件套照挂（ProgId command、`.ext` 默认值、打开方式候选）。
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.jpg"), "").as_deref(),
            Some("RuPix.jpg")
        );
        assert!(get_string(&format!("{T_CLASSES}\\RuPix.jpg\\shell\\open\\command"), "").is_some());
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.jpg\\OpenWithProgids"), "RuPix.jpg"),
            Some(String::new())
        );
        cleanup();
    }

    /// 与上个测试同源的直接单元版：写死的 SID 可以与真实用户无关地交叉验证
    /// 「现键 ft 重算哈希」这条自洽性。
    #[test]
    fn selfhashed_userchoice_is_self_consistent_with_given_sid() {
        cleanup();
        set_string(
            &format!("{T_FILEEXTS}\\.svg\\UserChoice"),
            Some("Hash"),
            "zzz",
        )
        .unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.svg\\UserChoice"),
            Some("ProgId"),
            "Other.app",
        )
        .unwrap();
        let fake_sid = "S-1-5-21-1111111111-2222222222-3333333333-999";
        // 直接跑 §四 时序（生产路径会传真实 SID；这里传固定假 SID 给函数，
        // 交叉验证「盘上 Hash == 现键 ft 重算值」）。
        write_userchoice_selfhashed(
            &format!("{T_FILEEXTS}\\.svg\\UserChoice"),
            "svg",
            "RuPix.svg",
            fake_sid,
        )
        .expect("词典整条 §四 路径应成功");
        let got =
            get_string(&format!("{T_FILEEXTS}\\.svg\\UserChoice"), "Hash").expect("Hash 应在");
        let ft = read_unsigned_lastwritetime(&format!("{T_FILEEXTS}\\.svg\\UserChoice"));
        assert_eq!(
            got,
            crate::platform::userchoice_hash::hash_for("svg", "RuPix.svg", ft, fake_sid),
            "给定 SID 下盘上 Hash 与现键 ft 必须自洽"
        );
        assert_eq!(
            get_string(&format!("{T_FILEEXTS}\\.svg\\UserChoice"), "ProgId").as_deref(),
            Some("RuPix.svg")
        );
        cleanup();
    }

    /// 读一个键的 LastWriteTime 为 u64（100ns 区间）。测试用，与 production 同 API。
    /// 显式用 KEY_READ 打开：RegQueryInfoKeyW 不含 Get/Set/Key 权限集时取不到 LastWriteTime。
    /// 测试侧的「取现键 LastWriteTime」——**直接复用生产实现**（`key_lastwrite`），
    /// 不另写一份：两份就读权限/参数顺序这类细节各自漂，等于测试验的是另一条路。
    fn read_unsigned_lastwritetime(path: &str) -> u64 {
        super::key_lastwrite(path).unwrap_or_else(|| panic!("测试内键应能开 {path}"))
    }

    #[test]
    fn hash_protected_but_already_ours_is_not_a_failure() {
        cleanup();
        // 带哈希 + ProgId 是 `Applications\<本 exe>`（用户此前已把默认设成我们）：
        // 不写入任何键、计入 already、不算失败——2026-11-07 实测 90 个"被占"里 49 个是这种。
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("Hash"),
            "abc",
        )
        .unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("ProgId"),
            &format!("Applications\\{name}"),
        )
        .unwrap();
        // 自建 ProgId 形态（RuPix.<ext>）同样认。
        set_string(
            &format!("{T_FILEEXTS}\\.png\\UserChoice"),
            Some("Hash"),
            "abc",
        )
        .unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.png\\UserChoice"),
            Some("ProgId"),
            "rupix.png",
        )
        .unwrap();
        let out = apply_with(
            T_CLASSES,
            T_FILEEXTS,
            T_TOASTS,
            &["jpg".into(), "png".into()],
        );
        assert_eq!(out.already, vec!["jpg".to_string(), "png".to_string()]);
        assert!(out.ok.is_empty() && out.manual.is_empty(), "{out:?}");
        // m314 起口径变了：**legacy 三件套要补齐**。原来的「一个键都不写」建立在
        // "已是默认就不用动"上，而自愈会把这些 legacy 项当破损判据——不补就是
        // 「判破 → 零写入 → 下一轮还破」的死循环。UserChoice 仍然一字不动。
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.jpg"), "").as_deref(),
            Some("RuPix.jpg")
        );
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.png"), "").as_deref(),
            Some("RuPix.png")
        );
        assert_eq!(
            get_string(&format!("{T_FILEEXTS}\\.jpg\\UserChoice"), "Hash").as_deref(),
            Some("abc"),
            "已有哈希不许被重算（重算会换掉盐，把用户的默认弄失效）"
        );
        cleanup();
    }

    #[test]
    fn unprotected_userchoice_is_overwritten() {
        cleanup();
        // 无 Hash、别人占着：直改 ProgId（JPEGView 同款）。
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("ProgId"),
            "Other.app",
        )
        .unwrap();
        let out = apply_with(T_CLASSES, T_FILEEXTS, T_TOASTS, &["jpg".into()]);
        assert_eq!(out.ok, vec!["jpg".to_string()]);
        assert_eq!(
            get_string(&format!("{T_FILEEXTS}\\.jpg\\UserChoice"), "ProgId").as_deref(),
            Some("RuPix.jpg")
        );
        cleanup();
    }

    #[test]
    fn report_lists_failures_by_name_only() {
        let out = Outcome {
            ok: vec!["jpg".into(), "png".into()],
            already: vec!["webp".into()],
            manual: vec!["cr3".into(), "heif".into()],
        };
        let r = out.report();
        assert!(r.starts_with("成功关联 2 个格式。"), "{r}");
        assert!(r.contains("另有 1 个本就是 RuPix 默认"), "{r}");
        assert!(r.contains("webp"), "{r}");
        assert!(r.contains("未能自动改写"), "{r}");
        assert!(r.contains("已把 RuPix 加入「打开方式」候选"), "{r}");
        assert!(r.contains("cr3、heif"), "{r}");
    }

    /// 读一个 REG_DWORD 值（测试用，只读 HKCU）。
    fn get_dword(path: &str, value: &str) -> Option<u32> {
        let mut h = HKEY::default();
        let err = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR::from_raw(wide(path).as_ptr()),
                None,
                KEY_QUERY_VALUE,
                &mut h,
            )
        };
        if err != ERROR_SUCCESS {
            return None;
        }
        let name = wide(value);
        let mut ty = REG_DWORD;
        let mut data: u32 = 0;
        let mut len: u32 = 4;
        let err = unsafe {
            RegQueryValueExW(
                h,
                PCWSTR::from_raw(name.as_ptr()),
                None,
                Some(&mut ty),
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut len),
            )
        };
        unsafe {
            let _ = RegCloseKey(h);
        }
        (err == ERROR_SUCCESS && ty == REG_DWORD).then_some(data)
    }

    #[test]
    fn uc_protected_covers_ucpd_guardeds_only() {
        // UCPD 保护名单（http/https/.pdf，2025-07 扩 Office/HTML）+ PDF 的兄弟 XPS。
        for e in [
            "pdf", "htm", "html", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
        ] {
            assert!(uc_protected(e), ".{e} 应受 UCPD 保护");
        }
        // 图片后缀一律不受保护——这是 RuPix 的主战场。
        for e in ["jpg", "png", "webp", "svg", "dcm", "j2c", "psd", "tif"] {
            assert!(!uc_protected(e), ".{e} 不应受 UCPD 保护");
        }
    }

    #[test]
    fn protected_extension_keeps_userchoice_untouched() {
        cleanup();
        // `.pdf` 有哈希、指向别人：UCPD 会拦删/写，所以绝不动它的 UserChoice。
        set_string(
            &format!("{T_FILEEXTS}\\.pdf\\UserChoice"),
            Some("Hash"),
            "abc",
        )
        .unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.pdf\\UserChoice"),
            Some("ProgId"),
            "Other.app",
        )
        .unwrap();
        let out = apply_with(T_CLASSES, T_FILEEXTS, T_TOASTS, &["pdf".into()]);
        // 计入 manual（候选已挂、差一次手动），不是 ok。
        assert_eq!(out.manual, vec!["pdf".to_string()], "{out:?}");
        assert!(out.ok.is_empty(), "{out:?}");
        // UserChoice 一字未动（ProgId / Hash 原样）。
        assert_eq!(
            get_string(&format!("{T_FILEEXTS}\\.pdf\\UserChoice"), "ProgId").as_deref(),
            Some("Other.app")
        );
        assert_eq!(
            get_string(&format!("{T_FILEEXTS}\\.pdf\\UserChoice"), "Hash").as_deref(),
            Some("abc")
        );
        // 但候选与壳照挂（图片分享的兜底通道）。
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.pdf\\OpenWithProgids"), "RuPix.pdf"),
            Some(String::new())
        );
        assert!(get_string(&format!("{T_CLASSES}\\RuPix.pdf\\shell\\open\\command"), "").is_some());
        cleanup();
    }

    /// m314 自愈的核心判据：**五项**里任一被拆掉都要被 `audit` 抓到，写回去后归零。
    ///
    /// 逐项破坏而不是"一锅端"：只有逐项才能证明 audit 不是只看 UserChoice 了事——
    /// 前四项是纯 legacy（与哈希无关），恰恰是最常被清理工具抹掉的那批。
    #[test]
    fn audit_flags_each_broken_link_and_a_rewrite_clears_it() {
        cleanup();
        let exe = std::env::current_exe().unwrap();
        let exts = vec!["jpg".to_string()];
        let broken = |e: &[String]| audit_with(T_CLASSES, T_FILEEXTS, &exe, e);
        let rewrite = || write_one(T_CLASSES, T_FILEEXTS, T_TOASTS, &exe, "jpg").unwrap();

        rewrite();
        assert!(broken(&exts).is_empty(), "刚写完就该是完整的");

        // ① `.ext` 默认值被别的程序抢走。
        set_string(&format!("{T_CLASSES}\\.jpg"), Some(""), "Other.app").unwrap();
        assert_eq!(broken(&exts), exts, "默认值被抢没抓到");
        rewrite();
        assert!(broken(&exts).is_empty());

        // ② 壳被指到别处（换目录 / 升级后的典型形态）。
        set_string(
            &format!("{T_CLASSES}\\RuPix.jpg\\shell\\open\\command"),
            Some(""),
            "\"D:\\old\\rupix.exe\" \"%1\"",
        )
        .unwrap();
        assert_eq!(broken(&exts), exts, "command 路径漂了没抓到");
        rewrite();
        assert!(broken(&exts).is_empty());

        // ③ 图标被清（关联成功但白纸）。
        delete_value(&format!("{T_CLASSES}\\RuPix.jpg\\DefaultIcon"), None).unwrap();
        assert_eq!(broken(&exts), exts, "DefaultIcon 缺了没抓到");
        rewrite();
        assert!(broken(&exts).is_empty());

        // ④ 右键「打开方式」候选被清。
        delete_value(
            &format!("{T_CLASSES}\\.jpg\\OpenWithProgids"),
            Some("RuPix.jpg"),
        )
        .unwrap();
        assert_eq!(broken(&exts), exts, "候选被清没抓到");
        rewrite();
        assert!(broken(&exts).is_empty());

        // ⑤ UserChoice 被别的程序上锁（有 Hash、指向别人）。
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("ProgId"),
            "Other.app",
        )
        .unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("Hash"),
            "abc",
        )
        .unwrap();
        assert_eq!(broken(&exts), exts, "UserChoice 指向别人没抓到");
        cleanup();
    }

    /// 受 UCPD 保护的后缀只审 legacy：UserChoice 指向别人也**不许**判破。
    ///
    /// 自愈最容易写错的一处：判破就会每轮重写一遍 legacy，而 UserChoice 永远写不进去
    /// ⇒ 日志刷屏 + 每小时给资源管理器发一次关联刷新。
    #[test]
    fn audit_stops_at_legacy_for_ucpd_protected_exts() {
        cleanup();
        let exe = std::env::current_exe().unwrap();
        let exts = vec!["pdf".to_string()];
        set_string(
            &format!("{T_FILEEXTS}\\.pdf\\UserChoice"),
            Some("ProgId"),
            "Other.app",
        )
        .unwrap();
        set_string(
            &format!("{T_FILEEXTS}\\.pdf\\UserChoice"),
            Some("Hash"),
            "abc",
        )
        .unwrap();
        // 生产路径：受保护 ⇒ 只挂 legacy，UserChoice 一字不动。
        let out = write_one(T_CLASSES, T_FILEEXTS, T_TOASTS, &exe, "pdf").unwrap();
        assert!(
            matches!(out, Written::CandidateOnly),
            "受保护的后缀只能挂候选"
        );
        assert!(
            audit_with(T_CLASSES, T_FILEEXTS, &exe, &exts).is_empty(),
            "受 UCPD 保护的后缀不该被判破（判破就会每小时白写一遍）"
        );
        cleanup();
    }

    /// 能力登记单独核：它不影响双击打开，只影响系统「设置 → 默认应用」里看得到我们，
    /// 但被抹掉后不会自己回来，所以自愈要能发现。
    #[test]
    fn caps_intact_detects_a_wiped_registration() {
        cleanup();
        let exe = std::env::current_exe().unwrap();
        let exts = vec!["jpg".to_string()];
        let ok = |e: &[String]| caps_intact(T_CLASSES, T_REGISTERED, T_CAPS, &exe, e);
        assert!(!ok(&exts), "没登记过就必须是 false");
        register_capabilities_with(T_CLASSES, T_REGISTERED, T_CAPS, &exe, &exts).unwrap();
        assert!(ok(&exts));
        // 抹掉 FileAssociations 的一条 = 系统设置里少一个后缀。
        delete_value(&format!("{T_CAPS}\\FileAssociations"), Some(".jpg")).unwrap();
        assert!(!ok(&exts), "FileAssociations 少了没抓到");
        cleanup();
    }
}
