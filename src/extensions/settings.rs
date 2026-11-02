//! 插件私有设置区：宿主替每个插件存一小块「它自己的偏好」。
//!
//! # 为什么宿主要管这件事
//!
//! 插件要记事（上次用的语言、选好的模型、要不要自动复制），有三条路：
//! 自己写文件（要 `file.write` 能力，而宿主**没有**这条通道）、写注册表（便携铁律禁止）、
//! 或者让宿主给它一块**自己的**存储。第三条是这里。
//!
//! # 形状与边界
//!
//! - **位置**：`<exe>/plugins/<id>/settings.json`（便携铁律：只在 exe 同级，
//!   不碰 `%APPDATA%`；插件目录本来就是宿主管的，安装/卸载时一起搬家）。
//! - **形状**：扁平的字符串键值对（`core::ext_protocol::Settings`）。
//!   宿主是 **dumb store**：只存只取，不解释结构。插件要复杂结构，自己编码成
//!   一个 JSON 字符串塞进来——这样插件改了设置结构，宿主这边永远不会解析失败。
//! - **权限**：只有声明 `settings.write` 的插件才能让宿主落盘；只有声明
//!   `settings.read` 的插件才会收到宿主发给它的设置。**没声明 = 没有通道。**
//! - **写失败 = 只读介质**：静默降级（记 warn），不让一次设置失败把插件结果打没。
//!
//! # 原子写
//!
//! 先写 `settings.json.tmp` 再 `rename` 覆盖：断电/崩溃时留下的要么是旧文件、
//! 要么是新文件，不会是半截 JSON（半截 JSON = 插件下次读到就当设置没了）。

use std::path::{Path, PathBuf};

use crate::core::ext_protocol::Settings;

/// 设置文件名（插件目录内）。`install_zip` 也要按它判断"新包是否自带设置区"。
pub(crate) const FILE_NAME: &str = "settings.json";
/// 临时文件名（原子写的中转站）。
const TMP_NAME: &str = "settings.json.tmp";

/// 这个插件的设置文件路径。`None` = 拿不到 exe 目录（便携模式下没有 exe 同级目录）。
pub fn settings_path(plugin_id: &str) -> Option<PathBuf> {
    let dir = crate::core::paths::plugin_dir()?;
    // ★ 路径围栏：id 参与拼路径，必须重新验一遍——manifest 的 id 校验在解析期，
    // 但设置区路径是**另一次**使用（跨会话、跨进程），不能默认上游已经验过。
    if !safe_id(plugin_id) {
        return None;
    }
    Some(dir.join(plugin_id).join(FILE_NAME))
}

/// id 是否可安全地参与路径拼接。**判据只有一份**：`core::ext::validate_id`
/// （manifest 解析期用的就是它）——这里再写一遍字面规则就是第二个正本，
/// 两边一旦漂移，"能装进来"与"能拼路径"就不是同一个 id 集合了。
fn safe_id(id: &str) -> bool {
    crate::core::ext::validate_id(id).is_ok()
}

/// 插件**升级**搬家第一步：**在动任何目录之前**把旧设置读进内存。
///
/// ★ 为什么不是"换完目录再复制"：旧写法（`carry_over`）在整目录换掉之后才搬，
/// 复制失败时旧目录已经被挪走、又被无条件删掉 ⇒ 用户偏好静默消失。
/// 把"读"提到前面，读不动就在**还没有任何东西被破坏**的那一刻中止，
/// 才是可逆的（后果能撤，就不必拿"保留一份备份目录"这种脏办法兜底）。
///
/// 返回：
/// - `Ok(None)` = 没有要搬的东西（旧目录没设置区，或新包自带一份——包自带优先）；
/// - `Ok(Some(bytes))` = 读到了，换完目录后交给 [`restore_settings`] 落进新目录；
/// - `Err` = 旧目录里有设置区却读不出来（权限 / 只读介质 / 磁盘错误）：
///   这时**放弃这次升级**比"装完然后发现搬不走"好。
pub fn snapshot_for_upgrade(
    old_dir: &Path,
    new_package_dir: &Path,
) -> Result<Option<Vec<u8>>, String> {
    let src = old_dir.join(FILE_NAME);
    if !src.is_file() || new_package_dir.join(FILE_NAME).exists() {
        return Ok(None);
    }
    std::fs::read(&src)
        .map(Some)
        .map_err(|e| format!("旧版设置区读不出来，已放弃本次升级（偏好不会被丢）：{e}"))
}

/// 搬家第二步：把 [`snapshot_for_upgrade`] 读到的那份写进新目录。
///
/// 新目录已经有设置区（包自带）就不动——包自带优先，与旧口径一致。
/// 失败只 warn：升级本身已经完成，不该为一次写失败回滚整目录；
/// 调用方在这种情况下**留着备份目录**（见 `install_zip`），用户还能手动找回。
pub fn restore_settings(new_dir: &Path, bytes: &[u8]) -> Result<(), String> {
    let dst = new_dir.join(FILE_NAME);
    if dst.exists() {
        return Ok(());
    }
    std::fs::write(&dst, bytes).map_err(|e| format!("旧设置写不进新目录：{e}"))
}

/// 读插件的设置区。**读不到 = 空设置**（首次运行、没有文件、文件坏了都一样）：
/// 设置区是锦上添花，读不到不该让插件跑不起来。
pub fn load(plugin_id: &str) -> Settings {
    let Some(path) = settings_path(plugin_id) else {
        return Vec::new();
    };
    load_from(&path, plugin_id)
}

/// 从指定路径读设置区（路径与 id 分开：单测要能指向临时目录）。
fn load_from(path: &Path, plugin_id: &str) -> Settings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match crate::core::json::parse(&text) {
        Ok(v) => match parse_store(&v) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("插件 {plugin_id} 的设置区读不出来（当空处理）：{e}");
                Vec::new()
            }
        },
        Err(e) => {
            log::warn!("插件 {plugin_id} 的设置区不是合法 JSON（当空处理）：{e}");
            Vec::new()
        }
    }
}

/// 写插件的设置区（整体覆盖）。失败只返回 Err，由调用方决定降级。
pub fn save(plugin_id: &str, items: &Settings) -> Result<(), String> {
    let Some(path) = settings_path(plugin_id) else {
        return Err("拿不到插件目录（便携模式？）".into());
    };
    save_to(&path, items)
}

/// 写到指定路径（原子写：先临时文件再改名）。
fn save_to(path: &Path, items: &Settings) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("建插件目录失败：{e}"))?;
    let tmp = dir.join(TMP_NAME);
    let body = to_json(items);
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| format!("写设置临时文件失败：{e}"))?;
    // std::fs::rename 支持替换已有文件；不预删，才能在替换失败时保住旧设置。
    std::fs::rename(&tmp, path).map_err(|e| format!("替换设置文件失败：{e}"))
}

/// 把插件回传的新设置**合并**进现有设置（同名键覆盖，新键追加）。
///
/// 为什么要合并而不是整体替换：插件只知道自己改的那几个键，宿主是**它所有
/// 设置的持有者**——整体替换会让插件"没提到的键"被清空（插件重启后偏好全丢）。
pub fn merge_into(plugin_id: &str, incoming: &Settings) -> Result<Settings, String> {
    let Some(path) = settings_path(plugin_id) else {
        return Err("拿不到插件目录（便携模式？）".into());
    };
    merge_into_at(&path, plugin_id, incoming)
}

/// 合并写（路径与 id 分开，单测用）。
fn merge_into_at(path: &Path, plugin_id: &str, incoming: &Settings) -> Result<Settings, String> {
    let mut cur = load_from(path, plugin_id);
    for (k, v) in incoming {
        match cur.iter_mut().find(|(ck, _)| ck == k) {
            Some(slot) => slot.1 = v.clone(),
            None => cur.push((k.clone(), v.clone())),
        }
    }
    save_to(path, &cur)?;
    Ok(cur)
}

/// 设置区落盘形状：`{"键":"值",…}`（顺序保留，人眼可比对）。
fn to_json(items: &Settings) -> String {
    let body = items
        .iter()
        .map(|(k, v)| format!("{}:{}", quote(k), quote(v)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}\n")
}

/// 读回设置区：只接受扁平的字符串键值。
fn parse_store(v: &crate::core::json::Value) -> Result<Settings, String> {
    use crate::core::json::Value;
    let Value::Obj(kv) = v else {
        return Err("设置区根必须是对象".into());
    };
    let mut out = Settings::new();
    for (k, val) in kv {
        let s = match val {
            Value::Str(s) => s.clone(),
            Value::Num(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return Err(format!("`{k}` 的值只能是字符串/数字/布尔")),
        };
        out.push((k.clone(), s));
    }
    Ok(out)
}

/// 最小 JSON 字符串转义（与协议层同一套口径）。
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// id 参与路径拼接，必须挡掉穿越。
    #[test]
    fn unsafe_ids_are_refused() {
        assert!(!safe_id(".."));
        assert!(!safe_id("../evil"));
        assert!(!safe_id("a/b"));
        assert!(!safe_id("C:evil"));
        assert!(!safe_id("ab"));
        assert!(safe_id("com.example.ocr"));
    }

    /// 设置区的 JSON 形状与解析必须往返一致（顺序保留）。
    #[test]
    fn settings_store_roundtrips() {
        let items: Settings = vec![
            ("lang".to_string(), "chi_sim".to_string()),
            ("copy".to_string(), "true".to_string()),
        ];
        let text = to_json(&items);
        let back = parse_store(&crate::core::json::parse(&text).expect("合法 JSON"))
            .expect("设置区应能读回");
        assert_eq!(back, items);
    }

    /// 嵌套结构一律拒绝：宿主是 dumb store，不替插件解释结构。
    #[test]
    fn nested_settings_are_refused() {
        let v = crate::core::json::parse(r#"{"a":{"b":1}}"#).expect("合法 JSON");
        assert!(parse_store(&v).is_err());
    }

    /// 落盘 → 读回 → 合并，三件事必须自洽（这是插件"记住偏好"的全部机制）。
    #[test]
    fn store_writes_reads_and_merges() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix_settings_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");

        // 没有文件 = 空设置（首次运行）。
        assert!(load_from(&path, "t").is_empty());

        let items: Settings = vec![
            ("lang".to_string(), "chi_sim".to_string()),
            ("copy".to_string(), "true".to_string()),
        ];
        save_to(&path, &items).expect("写设置");
        assert_eq!(load_from(&path, "t"), items);

        // 合并：改一个 + 加一个，另一个必须保住（插件没提到的键不该被清空）。
        let incoming: Settings = vec![
            ("lang".to_string(), "en-US".to_string()),
            ("model".to_string(), "m1".to_string()),
        ];
        let merged = merge_into_at(&path, "t", &incoming).expect("合并");
        assert_eq!(merged.len(), 3);
        assert!(merged.iter().any(|(k, v)| k == "lang" && v == "en-US"));
        assert!(merged.iter().any(|(k, v)| k == "copy" && v == "true"));
        assert!(merged.iter().any(|(k, v)| k == "model" && v == "m1"));

        // 坏文件当空处理（不能让一次损坏毁掉整个插件的启动）。
        std::fs::write(&path, b"{ not json").expect("写坏文件");
        assert!(load_from(&path, "t").is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn audit_failed_replace_preserves_old_settings() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix-audit-save-{}", std::process::id()));
        std::fs::create_dir(&dir).expect("create own sandbox");
        let path = dir.join(FILE_NAME);
        let old: Settings = vec![("lang".into(), "old".into())];
        save_to(&path, &old).expect("initial settings");
        // Allow the temporary write but deny DELETE sharing: rename must fail after writing.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0x1 | 0x2)
            .open(dir.join(TMP_NAME))
            .expect("hold temporary file");
        let result = save_to(&path, &vec![("lang".into(), "new".into())]);
        let preserved = load_from(&path, "test.audit.save");
        drop(held);
        std::fs::remove_dir_all(&dir).expect("cleanup own sandbox");
        let err = result.expect_err("rename must fail with temporary file held");
        // 只要求"报了错"：错误**文案**不是判据（改字不该弄红用例），真正的判据是下面那条。
        assert!(!err.is_empty());
        assert_eq!(
            preserved, old,
            "failed replacement must retain old settings"
        );
    }

    /// 升级搬家的两步：旧有新无 → 读到并写进新目录；新包自带 → 谁都不许覆盖。
    #[test]
    fn snapshot_then_restore_keeps_settings_across_upgrade() {
        let root = crate::core::paths::test_scratch_root()
            .join(format!("rupix_carry_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let old_dir = root.join("old");
        let new_dir = root.join("new");
        std::fs::create_dir_all(&old_dir).expect("建旧目录");
        std::fs::create_dir_all(&new_dir).expect("建新目录");
        let items: Settings = vec![("lang".to_string(), "chi_sim".to_string())];
        save_to(&old_dir.join(FILE_NAME), &items).expect("写旧设置");

        // 旧有新无 → 读出来，再落进新目录。
        let carried = snapshot_for_upgrade(&old_dir, &new_dir)
            .expect("读旧设置")
            .expect("旧目录里有设置区");
        restore_settings(&new_dir, &carried).expect("写新目录");
        assert_eq!(
            load_from(&new_dir.join(FILE_NAME), "t"),
            items,
            "升级后用户的偏好必须还在"
        );

        // 新包自带设置 → 快照阶段就返回 None（包作者的设置优先），restore 也不覆盖。
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).expect("建 staging");
        std::fs::write(staging.join(FILE_NAME), b"{\"lang\":\"packaged\"}").expect("写包内设置");
        assert_eq!(snapshot_for_upgrade(&old_dir, &staging).expect("读"), None);
        std::fs::write(new_dir.join(FILE_NAME), b"{\"lang\":\"packaged\"}").expect("写新包设置");
        restore_settings(&new_dir, &carried).expect("restore 不该报错");
        assert_eq!(
            load_from(&new_dir.join(FILE_NAME), "t"),
            vec![("lang".to_string(), "packaged".to_string())],
            "包自带的设置区必须赢"
        );

        // 旧目录根本没有设置区 → 也没必要搬。
        let bare = root.join("bare");
        std::fs::create_dir_all(&bare).expect("建 bare");
        assert_eq!(snapshot_for_upgrade(&bare, &new_dir).expect("读"), None);

        let _ = std::fs::remove_dir_all(&root);
    }
}
