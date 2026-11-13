//! 最近打开的文件：`<exe 目录>/recent.txt`，一行一个路径，最多 `MAX` 条。
//!
//! 只服务右键「最近 ▸」这一处显式入口——不做书签、不做会话恢复、不做启动页。
//! 记录点只有 [`crate::core::viewer::Viewer::open`] 成功之后那一次；压缩包记**原始文件**，
//! 不记展开出来的临时层目录。目录不记（拖文件夹打开不产生最近项）。
//!
//! 存储口径与 `rupix.ini` 同一条便携铁律：只写 exe 同级、UTF-8、一行一个路径、
//! 临时文件 + `sync_all` + rename 的原子写；写失败只 warn，本次会话继续用内存列表。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 最近文件上限。用户 2026-11-13 点名 100 条。
pub const MAX: usize = 100;

/// 文件名（exe 同级）。路径由 [`crate::core::paths::recent_path`] 给出。
pub const FILE: &str = "recent.txt";

/// 已加载的列表（首次访问时从盘上读；进程内唯一缓存）。
static CACHE: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

/// 读最近列表的副本（最新在前）。读盘失败 = 空列表，不打断调用方。
pub fn list() -> Vec<PathBuf> {
    with_cache(|list| list.clone())
}

/// 成功打开一个文件后记一条：去重后移到队首，超过上限淘汰最旧。
///
/// 只接受**文件**：目录（拖文件夹打开）与不存在的路径不进来。写盘失败不影响画面，
/// 下一次成功写盘时整份列表照旧落盘。
pub fn record(path: &Path) {
    if !path.is_file() {
        return;
    }
    let Some(path) = absolute(path) else {
        return;
    };
    mutate(|list| insert_front(list, path));
}

/// 移除一条（失效路径被点到时用）。
pub fn remove(path: &Path) {
    let key = norm_key(path);
    mutate(|list| list.retain(|p| norm_key(p) != key));
}

/// 清空列表（右键「清空最近」）。
pub fn clear() {
    mutate(|list| list.clear());
}

/// 菜单标签：优先「父目录\文件名」，太长时只留尾部。完整路径仍可从日志查。
///
/// 100 条全路径会把菜单撑到屏幕外（windui 能滚动，但横向没有截断），所以这里
/// 只保留能认出文件的最短形式；`父目录` 取最后一级目录名。
pub fn label(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .map(|s| s.to_string_lossy().to_string());
    let full = match parent {
        Some(dir) if !dir.is_empty() => format!("{dir}\\{file}"),
        _ => file,
    };
    truncate_left(&full, 72)
}

/// 从左边截断到最多 `max` 个**字符**（不是字节），前缀用 `…`。
fn truncate_left(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let tail: String = chars[chars.len() - (max.saturating_sub(1))..]
        .iter()
        .collect();
    format!("…{tail}")
}

// ── 纯逻辑（单测直接打） ────────────────────────────────────────────

/// 插到队首：先按 Windows 口径（大小写不敏感、斜杠统一）去重，再截到 `MAX`。
fn insert_front(list: &mut Vec<PathBuf>, path: PathBuf) {
    let key = norm_key(&path);
    list.retain(|p| norm_key(p) != key);
    list.insert(0, path);
    list.truncate(MAX);
}

/// 路径比较键：Windows 文件系统大小写不敏感；把 `/` 归一成 `\`，去掉尾部斜杠。
/// 不 canonicalize——`\\?\` 前缀不该出现在用户会看的文件里。
fn norm_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    s.trim_end_matches('\\').to_ascii_lowercase()
}

fn absolute(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        std::path::absolute(path).ok()
    }
}

fn parse(text: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let p = PathBuf::from(line);
        let key = norm_key(&p);
        if out.iter().any(|q| norm_key(q) == key) {
            continue;
        }
        out.push(p);
        if out.len() == MAX {
            break;
        }
    }
    out
}

fn render(list: &[PathBuf]) -> String {
    let mut out = String::new();
    for p in list {
        out.push_str(&p.to_string_lossy());
        out.push('\n');
    }
    out
}

fn load_from(path: Option<&Path>) -> Vec<PathBuf> {
    let Some(path) = path else {
        return Vec::new();
    };
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            log::warn!("最近文件：读 {} 失败：{e}", path.display());
            Vec::new()
        }
    }
}

fn save_to(path: &Path, list: &[PathBuf]) {
    let tmp = path.with_extension("tmp");
    let body = render(list);
    let staged = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = staged {
        log::warn!(
            "最近文件：写 {} 失败（本次会话仅内存保留）：{e}",
            tmp.display()
        );
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        log::warn!("最近文件：替换 {} 失败：{e}", path.display());
        let _ = std::fs::remove_file(&tmp);
    }
}

fn with_cache<T>(f: impl FnOnce(&Vec<PathBuf>) -> T) -> T {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| load_from(crate::core::paths::recent_path().as_deref()));
    f(list)
}

fn mutate(f: impl FnOnce(&mut Vec<PathBuf>)) {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| load_from(crate::core::paths::recent_path().as_deref()));
    f(list);
    if let Some(path) = crate::core::paths::recent_path() {
        save_to(&path, list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn newest_first_and_duplicate_moves_to_front() {
        let mut list = Vec::new();
        insert_front(&mut list, p(r"C:\a\one.png"));
        insert_front(&mut list, p(r"C:\b\two.png"));
        assert_eq!(list, vec![p(r"C:\b\two.png"), p(r"C:\a\one.png")]);
        insert_front(&mut list, p(r"c:/A/ONE.PNG"));
        assert_eq!(
            list,
            vec![p(r"c:/A/ONE.PNG"), p(r"C:\b\two.png")],
            "同一路径（大小写/斜杠不同）必须去重并移到队首"
        );
    }

    #[test]
    fn caps_at_one_hundred_evicting_the_oldest() {
        let mut list = Vec::new();
        for i in 0..(MAX + 5) {
            insert_front(&mut list, p(&format!(r"C:\d\{i}.png")));
        }
        assert_eq!(list.len(), MAX);
        assert_eq!(list[0], p(&format!(r"C:\d\{}.png", MAX + 4)));
        assert_eq!(list[MAX - 1], p(r"C:\d\5.png"));
    }

    #[test]
    fn parse_skips_blank_and_duplicate_lines() {
        let text = "\r\nC:\\a\\1.png\r\n\r\nc:/A/1.PNG\r\nC:\\b\\2.png\r\n";
        assert_eq!(
            parse(text),
            vec![p(r"C:\a\1.png"), p(r"C:\b\2.png")],
            "空行跳过；大小写/斜杠不同的同一路径只留第一条"
        );
    }

    #[test]
    fn disk_round_trip_keeps_utf8_paths_and_order() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("recent-roundtrip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("recent.txt");
        let list = vec![p(r"C:\图片\第二张.png"), p(r"C:\pics\first.jpg")];
        save_to(&file, &list);
        assert_eq!(load_from(Some(&file)), list);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn label_uses_parent_and_file_and_truncates_from_the_left() {
        assert_eq!(label(Path::new(r"C:\pics\风景\日落.png")), "风景\\日落.png");
        let long = format!(r"C:\pics\{}.png", "很长的名字".repeat(30));
        let got = label(Path::new(&long));
        assert!(got.starts_with('…'), "超长标签必须从左边截断：{got}");
        assert!(got.ends_with(".png"), "尾部（文件名）必须留着：{got}");
        assert!(got.chars().count() <= 72);
    }
}
