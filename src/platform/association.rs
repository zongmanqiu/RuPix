//! 文件关联（m244）：把 RuPix 写成后缀的默认打开方式。
//!
//! 修宪条款（2026-11-07 用户拍板）：**注册表的唯一例外 = 文件关联**——只写 HKCU、
//! 完全可逆（只删自己写的键）、永不后台复查（点一次写一次，别的软件随时可以覆盖回来）。
//! 本模块是这个例外唯一的落点；除它之外任何代码碰注册表都违宪章。
//!
//! # 为什么是 legacy 直写 + 哈希退让（同行实证）
//!
//! 三家参考（`.ex/`）：**JPEGView** 直写 `HKCU\Software\Classes\.ext` 与 `FileExts`，
//! 但 `IsProtectedByHash()` 探测到 UserChoice 带哈希就返回 `Reg_ErrorProtectedByHash`
//! 放弃；**PicView** 纯 legacy 108 行；**YeImageViewer** 另做「打开方式登记」
//! （`OpenWithProgids` + `Applications\<exe>\SupportedTypes`，86 行）。
//! 共识：**UserChoice 的 Hash 是微软的防篡改锁，没人打赢过**——直写会触发
//! 「已重置默认」通知甚至被杀软标为劫持（2026-11-07 探针实证：原版 PS-SFTA 算法在
//! 25H2 build 26200 上写入成功但首次使用即被重置，盐串未变、算法整体失效）。
//! 所以这里：带哈希的 UserChoice 一字不动；但 **legacy 候选照挂**（ProgId command +
//! `.ext` 默认值 + `OpenWithProgids`），被占后缀因此进右键「打开方式」列表，
//! 用户右键 → RuPix → 始终，一次手动完成——这是 Windows 留给所有软件的唯一正道。
//! 没被锁的后缀直写立即生效。
//!
//! # 覆盖语义（用户 2026-11-07 拍板）
//!
//! 勾选 = 期望状态，一次「应用」= 按快照写入覆盖一次：勾的写关联，**不提供摘除**
//! ——覆盖式写入没法干净撤销（要撤得备份别人的旧值），悬空关联由 Windows 回落
//! 「选择打开方式」兜底，无害。写的是 `.ext` 默认值 + 自建 ProgId + `OpenWithProgids`
//! 候选（后者让 RuPix 至少出现在右键「打开方式」列表里），外加未保护的 `UserChoice`
//! ProgId 直改。

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPEN_CREATE_OPTIONS, REG_SZ,
};
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

/// 生产环境的两个根： Classes 合并视图的用户层 + 每后缀 FileExts。
const CLASSES_ROOT: &str = r"Software\Classes";
const FILEEXTS_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts";
/// 能力登记（让 RuPix 出现在系统「设置 → 默认应用」列表里）：**只登记、不抢默认**。
/// `RegisteredApplications` 的值数据是 Capabilities 键的路径（相对 HKCU）。
const REGISTERED_ROOT: &str = r"Software\RegisteredApplications";
const CAPS_ROOT: &str = r"Software\RuPix\Capabilities";
const APP_NAME: &str = "RuPix";

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
/// - `ok`：默认已程序化写成（`.ext` 默认值 + ProgId + 未锁的 UserChoice 直改）；
/// - `already`：带哈希但已是 RuPix（用户此前设过我们），行为已正确，不动（2026-11-07
///   实测纠正：90 个"被占"里 49 个其实是这种）；
/// - `manual`：默认被其他程序占（UserChoice 带哈希且指向别人）——微软禁止任何程序
///   改写（探针实证：PS-SFTA 算法在 26200 上使用时被重置），但 **legacy 候选照挂**：
///   RuPix 进右键「打开方式」列表，右键 → 打开方式 → RuPix → 始终，一次手动完成。
#[derive(Default, Debug, PartialEq)]
pub struct Outcome {
    pub ok: Vec<String>,
    /// 带哈希但已是 RuPix：跳过写入、计入成功侧的注脚。
    pub already: Vec<String>,
    /// 带哈希且指向别人：UserChoice 不碰，legacy 候选照挂，默认留给用户一次手动。
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
                "\n\n以下 {} 个的默认被其他程序占用（Windows 禁止程序改写），已把 RuPix 加入「打开方式」候选——右键任一此类文件 → 打开方式 → RuPix 并选「始终」即完成关联：\n{}",
                self.manual.len(),
                self.manual.join("、")
            ));
        }
        s
    }
}

/// 把 `exts` 全部写成默认打开方式。一次批量、一次外壳通知；**永不复查**。
///
/// 调用点只有文件关联窗的「应用」（`ui::association`）。`exts` 先规范化去重。
pub fn apply(exts: &[String]) -> Outcome {
    apply_with(CLASSES_ROOT, FILEEXTS_ROOT, exts)
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
fn apply_with(classes_root: &str, fileexts_root: &str, exts: &[String]) -> Outcome {
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
        match write_one(classes_root, fileexts_root, &exe, &ext) {
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
    /// 本次把默认写成 RuPix。
    Default,
    /// 默认被别人占（哈希锁）：UserChoice 不碰，legacy 候选照挂，差一次手动。
    CandidateOnly,
}

/// 写单个后缀。任何一步注册表失败都算这个后缀失败，**不留半写状态重试**（下次应用重来）。
fn write_one(
    classes_root: &str,
    fileexts_root: &str,
    exe: &Path,
    ext: &str,
) -> Result<Written, &'static str> {
    let uc = format!("{fileexts_root}\\.{ext}\\UserChoice");
    let pid = progid(ext);
    // UserChoice 带哈希 = 系统上锁（探针实证：26200 对逆向哈希使用时重置，直改是哈希战）。
    // 锁下已是 RuPix（`Applications\<exe>` 是系统"始终以此打开"造的通用 ProgId，
    // `RuPix.<ext>` 是我们自己写的）→ 行为已正确，不动；
    // 锁下是别人 → UserChoice 不碰，但 legacy 候选照挂：右键「打开方式」里有我们，
    // 用户右键 → 始终，一次手动完成。**不再把这类当失败。**
    if get_string(&uc, "Hash").is_some() {
        if get_string(&uc, "ProgId")
            .map(|p| progid_is_ours(&p, ext, exe))
            .unwrap_or(false)
        {
            return Ok(Written::AlreadyOurs);
        }
        write_legacy(classes_root, exe, ext, &pid)?;
        return Ok(Written::CandidateOnly);
    }
    write_legacy(classes_root, exe, ext, &pid)?;
    // UserChoice 存在但无哈希（老系统 / 未被系统 UI 上锁）：直改 ProgId 才真生效。
    if let Some(old) = get_string(&uc, "ProgId") {
        if old != pid {
            set_string(&uc, Some("ProgId"), &pid).map_err(|_| "UserChoice 直改失败")?;
        }
    }
    Ok(Written::Default)
}

/// legacy 三件套：ProgId 的 open command 与友好名、`.ext` 默认值、「打开方式」候选。
/// 哈希锁只锁 UserChoice，这三样永远可写——被占后缀靠它们进右键列表。
fn write_legacy(classes_root: &str, exe: &Path, ext: &str, pid: &str) -> Result<(), &'static str> {
    write_progid(classes_root, exe, ext, pid)?;
    // 默认关联本体：覆盖 `.ext` 默认值（覆盖语义正是用户拍板的行为）。
    set_string(&format!("{classes_root}\\.{ext}"), None, pid)
        .map_err(|_| "扩展名默认值写入失败")?;
    // 右键「打开方式」候选（YeImageViewer 那半）：默认被别人占时至少列表里有我们。
    set_string(
        &format!("{classes_root}\\.{ext}\\OpenWithProgids"),
        Some(pid),
        "",
    )
    .map_err(|_| "OpenWithProgids 写入失败")
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
pub fn is_ours(ext: &str) -> bool {
    is_ours_with(CLASSES_ROOT, ext)
}

fn is_ours_with(classes_root: &str, ext: &str) -> bool {
    let Some(ext) = normalize(ext) else {
        return false;
    };
    get_string(&format!("{classes_root}\\.{ext}"), "").as_deref() == Some(progid(&ext)).as_deref()
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

    fn cleanup() {
        delete_tree(T_CLASSES);
        delete_tree(T_FILEEXTS);
        delete_tree(T_REGISTERED);
        delete_tree(T_CAPS);
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
        let out = apply_with(T_CLASSES, T_FILEEXTS, &["jpg".into(), ".PNG".into()]);
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
        // is_ours 双向。
        assert!(is_ours_with(T_CLASSES, "jpg"));
        assert!(!is_ours_with(T_CLASSES, "tif"));
        cleanup();
    }

    #[test]
    fn hash_protected_userchoice_keeps_lock_but_hangs_candidate() {
        cleanup();
        // 造一个带 Hash 的受保护 UserChoice（等价系统里用户手动设过别的默认）：
        // UserChoice 一字不动，但 legacy 候选照挂——右键「打开方式」里要有我们。
        set_string(
            &format!("{T_FILEEXTS}\\.jpg\\UserChoice"),
            Some("Hash"),
            "abc",
        )
        .unwrap();
        let out = apply_with(T_CLASSES, T_FILEEXTS, &["jpg".into(), "png".into()]);
        assert_eq!(out.manual, vec!["jpg".to_string()]);
        assert_eq!(out.ok, vec!["png".to_string()]);
        // 锁本身不动：Hash 原值还在、ProgId 没被塞进 UserChoice。
        assert_eq!(
            get_string(&format!("{T_FILEEXTS}\\.jpg\\UserChoice"), "Hash").as_deref(),
            Some("abc")
        );
        assert!(get_string(&format!("{T_FILEEXTS}\\.jpg\\UserChoice"), "ProgId").is_none());
        // legacy 候选照挂：.jpg 默认值、ProgId command、打开方式列表全在。
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.jpg"), "").as_deref(),
            Some("RuPix.jpg")
        );
        assert!(get_string(&format!("{T_CLASSES}\\RuPix.jpg\\shell\\open\\command"), "").is_some());
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.jpg\\OpenWithProgids"), "RuPix.jpg"),
            Some(String::new())
        );
        assert_eq!(
            get_string(&format!("{T_CLASSES}\\.png"), "").as_deref(),
            Some("RuPix.png")
        );
        cleanup();
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
        let out = apply_with(T_CLASSES, T_FILEEXTS, &["jpg".into(), "png".into()]);
        assert_eq!(out.already, vec!["jpg".to_string(), "png".to_string()]);
        assert!(out.ok.is_empty() && out.manual.is_empty(), "{out:?}");
        // 一个键都不该写（行为已正确，不折腾）。
        assert!(get_string(&format!("{T_CLASSES}\\.jpg"), "").is_none());
        assert!(get_string(&format!("{T_CLASSES}\\.png"), "").is_none());
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
        let out = apply_with(T_CLASSES, T_FILEEXTS, &["jpg".into()]);
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
        assert!(r.contains("被其他程序占用"), "{r}");
        assert!(r.contains("已把 RuPix 加入「打开方式」候选"), "{r}");
        assert!(r.contains("cr3、heif"), "{r}");
    }
}
