//! 阅读位置记忆：`<exe 目录>/reading.txt`，一本"书"一条，最多 `MAX` 本。
//!
//! 「书」的判据（m333）：
//! - 普通文件 / PDF：规范化后的**文件路径**（`f:`）；
//! - 目录 / 漫画文件夹：规范化后的**目录路径**（`d:`）；
//! - 压缩包层：**原始包路径 + 内部层**（`a:`），绝不记解压临时目录。
//!
//! 每条记下当前清单项、帧/页号与视图（fit / 倍率 / 平移）。记录点由
//! `runtime` 防抖保存；打开时由 `Viewer` 查表恢复。写盘口径与 `recent.txt`
//! 同一条便携铁律：exe 同级、UTF-8、临时文件 + `sync_all` + rename。
//!
//! 行格式（`\t` 分隔，字段内 `\` / `\t` / `\n` / `\r` 转义）：
//!
//! ```text
//! v1
//! key \t item \t index \t page \t fit \t scale \t pan_x \t pan_y
//! ```
//!
//! `item` 对普通目录是完整路径，对压缩包层是**层内文件名**（解压目录每次启动
//! 都会变，路径不可比）。找不到 `item` 就整条作废，不按旧下标硬套——文件删了
//! 还按旧位置翻，用户会落在完全无关的一张上。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 最多记住多少本"书"（LRU：最近打开/改动的在前）。
pub const MAX: usize = 200;

/// 文件名（exe 同级）。路径由 [`crate::core::paths::reading_path`] 给出。
pub const FILE: &str = "reading.txt";

/// 盘上格式版本。改字段含义时递增，老文件整份忽略（不猜、不迁移）。
const SCHEMA: &str = "v1";

/// 一条阅读记忆。
#[derive(Debug, Clone, PartialEq)]
pub struct Memento {
    /// [`dir_key`] / [`file_key`] / [`archive_key`] 之一。
    pub key: String,
    /// 清单项提示：普通目录存完整路径，压缩包层存层内文件名。
    pub item: String,
    /// 清单下标（0 起；只作调试/兜底，恢复时**一律按 `item` 找**）。
    /// `item` 找不到就整条作废，不拿这个下标硬套。
    pub index: usize,
    /// 分页文档的页号（0 起；静态图 / 动图帧恒 0）。
    pub page: u32,
    /// 是否「适应窗口」。
    pub fit: bool,
    /// 手动档倍率（`fit == true` 时忽略）。
    pub scale: f32,
    /// 手动档平移（`fit == true` 时忽略）。
    pub pan: (f32, f32),
}

impl Memento {
    /// 这份记忆属于哪本书（菜单「清除本书阅读位置」按它删）。
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// 已加载的列表（首次访问时从盘上读；进程内唯一缓存，最近在前）。
static CACHE: Mutex<Option<Vec<Memento>>> = Mutex::new(None);

/// 刚被用户清掉、但画面还停在原状态的那一本。
///
/// 没有它，菜单刚清完、下一拍防抖保存又把同一份状态写回去——用户会看到
/// "删了又回来"。只有当这本书真的**换了一项/一页/一个视图**（`Memento`
/// 不再相等）时才解除抑制，新进度照常落盘；清完后原样关窗则保持已清。
static SUPPRESS: Mutex<Option<Memento>> = Mutex::new(None);

/// 书键：普通目录按 Windows 口径归一（大小写不敏感、斜杠统一、去尾斜杠）。
pub fn dir_key(dir: &Path) -> String {
    format!("d:{}", norm_key(dir))
}

/// 书键：单个文件（含 PDF / 动图）。
pub fn file_key(file: &Path) -> String {
    format!("f:{}", norm_key(file))
}

/// 书键：压缩包原始路径 + 内部层（`segs` 为空 = 根层）。
pub fn archive_key(source: &Path, segs: &[String]) -> String {
    format!("a:{}#{}", norm_key(source), segs.join("/"))
}

/// 写进 `item` 的完整路径：保留用户盘上的大小写，只把相对路径补成绝对。
pub fn item_path(path: &Path) -> String {
    absolute(path).to_string_lossy().to_string()
}

/// 两条路径是不是同一个 Windows 文件（大小写 / 斜杠 / 尾斜杠都归一）。
pub fn same_path(a: &Path, b: &Path) -> bool {
    norm_key(a) == norm_key(b)
}

/// 取一条记忆的副本。被用户清掉的那一本在状态变化前一直返回 `None`。
pub fn get(key: &str) -> Option<Memento> {
    if suppressed(key) {
        return None;
    }
    with_cache(|list| list.iter().find(|m| m.key == key).cloned())
}

/// 写入/更新一条：按 key 去重后移到队首，超上限淘汰最旧，并立即落盘。
pub fn put(m: Memento) {
    if suppress_put(&m) {
        return;
    }
    mutate(|list| upsert(list, m));
}

/// 删除一条（不建立抑制；菜单请用 [`remove_with_state`]）。
pub fn remove(key: &str) {
    mutate(|list| list.retain(|m| m.key != key));
}

/// 删除一条，并抑制"画面仍停在原状态"的重复保存（见 [`SUPPRESS`]）。
pub fn remove_with_state(m: &Memento) {
    let key = m.key.clone();
    mutate(|list| list.retain(|x| x.key != key));
    let mut g = SUPPRESS.lock().unwrap_or_else(|e| e.into_inner());
    *g = Some(m.clone());
}

/// 清空全部阅读记忆（连抑制一起清）。
pub fn clear() {
    mutate(|list| list.clear());
    let mut g = SUPPRESS.lock().unwrap_or_else(|e| e.into_inner());
    *g = None;
}

// ── 纯逻辑（单测直接打） ────────────────────────────────────────────

/// 路径比较键：Windows 文件系统大小写不敏感；把 `/` 归一成 `\`，去掉尾部斜杠。
/// 不 canonicalize——`\\?\` 前缀不该出现在用户会看的文件里。
fn norm_key(p: &Path) -> String {
    absolute(p)
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
    }
}

fn upsert(list: &mut Vec<Memento>, m: Memento) {
    list.retain(|x| x.key != m.key);
    list.insert(0, m);
    list.truncate(MAX);
}

fn suppressed(key: &str) -> bool {
    let g = SUPPRESS.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().is_some_and(|m| m.key == key)
}

/// `put` 之前过一遍抑制：同一本、同一份状态 → 不写；一旦状态真的变了，
/// 解除抑制并让新进度落盘。
fn suppress_put(m: &Memento) -> bool {
    let mut g = SUPPRESS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sup) = g.as_ref() else {
        return false;
    };
    if sup.key == m.key {
        if sup == m {
            return true;
        }
        *g = None;
        return false;
    }
    // 换了一本书：上一本的抑制到此为止。
    *g = None;
    false
}

/// 字段转义：`\` / `\t` / `\n` / `\r`。路径里出现这些字符极罕见，但手改坏了
/// 也不该把后一行吃进同一字段。
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => {
                // 不认识的反斜杠原样保留（手改坏一行不至于吞字符）。
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn render_memento(m: &Memento) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        escape(&m.key),
        escape(&m.item),
        m.index,
        m.page,
        u8::from(m.fit),
        m.scale,
        m.pan.0,
        m.pan.1
    )
}

fn parse_memento(line: &str) -> Option<Memento> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != 8 {
        return None;
    }
    let key = unescape(f[0]);
    if key.is_empty() {
        return None;
    }
    let scale: f32 = f[5].parse().ok()?;
    let pan = (f[6].parse::<f32>().ok()?, f[7].parse::<f32>().ok()?);
    // NaN / inf 会让 `clamp` 之后的几何一路带毒；写坏的一行直接丢。
    if !scale.is_finite() || !pan.0.is_finite() || !pan.1.is_finite() {
        return None;
    }
    Some(Memento {
        key,
        item: unescape(f[1]),
        index: f[2].parse().ok()?,
        page: f[3].parse().ok()?,
        fit: match f[4] {
            "0" => false,
            "1" => true,
            _ => return None,
        },
        scale,
        pan,
    })
}

fn parse(text: &str) -> Vec<Memento> {
    let mut lines = text.lines();
    // 版本头必须先对上：老/未来格式整份忽略，不猜字段。
    let Some(version) = lines.next() else {
        return Vec::new();
    };
    // 手改 / 某些编辑器会带 UTF-8 BOM：版本头不能被它顶掉。
    if version.trim_start_matches('\u{feff}').trim() != SCHEMA {
        return Vec::new();
    }
    let mut out: Vec<Memento> = Vec::new();
    for line in lines {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some(m) = parse_memento(line) else {
            continue;
        };
        if out.iter().any(|x| x.key == m.key) {
            continue;
        }
        out.push(m);
        if out.len() == MAX {
            break;
        }
    }
    out
}

fn render(list: &[Memento]) -> String {
    let mut out = String::from(SCHEMA);
    out.push('\n');
    for m in list {
        out.push_str(&render_memento(m));
        out.push('\n');
    }
    out
}

fn load_from(path: Option<&Path>) -> Vec<Memento> {
    let Some(path) = path else {
        return Vec::new();
    };
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            log::warn!("阅读位置：读 {} 失败：{e}", path.display());
            Vec::new()
        }
    }
}

fn save_to(path: &Path, list: &[Memento]) {
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
            "阅读位置：写 {} 失败（本次会话仅内存保留）：{e}",
            tmp.display()
        );
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        log::warn!("阅读位置：替换 {} 失败：{e}", path.display());
        let _ = std::fs::remove_file(&tmp);
    }
}

fn with_cache<T>(f: impl FnOnce(&Vec<Memento>) -> T) -> T {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| load_from(crate::core::paths::reading_path().as_deref()));
    f(list)
}

fn mutate(f: impl FnOnce(&mut Vec<Memento>)) {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| load_from(crate::core::paths::reading_path().as_deref()));
    f(list);
    if let Some(path) = crate::core::paths::reading_path() {
        save_to(&path, list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(key: &str, item: &str, index: usize, page: u32) -> Memento {
        Memento {
            key: key.to_string(),
            item: item.to_string(),
            index,
            page,
            fit: true,
            scale: 1.0,
            pan: (0.0, 0.0),
        }
    }

    #[test]
    fn keys_normalize_case_slashes_and_trailing_separators() {
        let a = dir_key(Path::new(r"C:\Pics\Trip\"));
        let b = dir_key(Path::new(r"c:/pics/trip"));
        assert_eq!(a, b, "大小写、斜杠、尾部斜杠都必须归一");
        assert!(a.starts_with("d:"));

        assert_eq!(
            file_key(Path::new(r"C:\Pics\A.PDF")),
            file_key(Path::new(r"c:/pics/a.pdf"))
        );
        assert!(file_key(Path::new(r"C:\x.pdf")).starts_with("f:"));

        let segs = vec!["ch1".to_string(), "ch2".to_string()];
        assert_eq!(
            archive_key(Path::new(r"C:\Books\Comic.ZIP"), &segs),
            "a:c:\\books\\comic.zip#ch1/ch2"
        );
    }

    #[test]
    fn disk_round_trip_keeps_utf8_and_all_fields() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("reading-roundtrip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(FILE);
        let mut a = m("f:c:\\图片\\第一张.pdf", r"C:\图片\第一张.pdf", 3, 42);
        a.fit = false;
        a.scale = 2.25;
        a.pan = (-120.5, 66.0);
        let list = vec![
            a.clone(),
            m("d:c:\\图片", r"C:\图片\第二张.png", 0, 0),
            m("a:c:\\books\\x.zip#ch1", "003.png", 2, 0),
        ];
        save_to(&file, &list);
        assert_eq!(load_from(Some(&file)), list);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_dedupes_moves_to_front_and_caps() {
        let mut list = Vec::new();
        upsert(&mut list, m("f:c:\\a.png", "a", 0, 0));
        upsert(&mut list, m("f:c:\\b.png", "b", 0, 0));
        upsert(&mut list, m("f:c:\\a.png", "a2", 1, 0));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].item, "a2", "同 key 更新后移到队首");
        assert_eq!(list[1].item, "b");

        for i in 0..(MAX + 5) {
            upsert(&mut list, m(&format!("f:c:\\{i}.png"), "x", 0, 0));
        }
        assert_eq!(list.len(), MAX);
        assert_eq!(list[0].key, format!("f:c:\\{}.png", MAX + 4));
    }

    #[test]
    fn escapes_keep_tabs_newlines_and_backslashes_in_one_field() {
        let m = m("f:c:\\a\\b.png", "a\tb\nc\\d", 0, 0);
        let line = render_memento(&m);
        assert_eq!(line.matches('\t').count(), 7, "字段分隔符不能被内容污染");
        assert_eq!(parse_memento(&line), Some(m));
        assert_eq!(escape(r"a\b"), r"a\\b");
        assert_eq!(unescape(r"a\\b"), r"a\b");
        assert_eq!(unescape(r"\q"), r"\q", "不认识的反斜杠原样保留");
    }

    #[test]
    fn parse_rejects_wrong_version_and_junk_lines() {
        assert!(
            parse("v2\nf:c:\\a.png\tx\t0\t0\t1\t1\t0\t0\n").is_empty(),
            "未知版本整份忽略"
        );
        let text = "v1\n\nf:c:\\a.png\tx\t0\t0\t1\t1\t0\t0\nbad line\n\
                    f:c:\\a.png\ty\t0\t0\t1\t1\t0\t0\n";
        let got = parse(text);
        assert_eq!(got.len(), 1, "坏行跳过、重复 key 只留第一条");
        assert_eq!(got[0].item, "x");
    }

    #[test]
    fn suppression_ignores_the_same_state_but_accepts_new_progress() {
        let first = m("f:c:\\a.pdf", "a.pdf", 0, 0);
        {
            let mut g = SUPPRESS.lock().unwrap();
            *g = Some(first.clone());
        }
        assert!(suppressed(&first.key), "清除后先进入抑制态");
        assert!(suppress_put(&first), "同一份状态不许写回");

        let mut next = first.clone();
        next.page = 7;
        assert!(!suppress_put(&next), "真的翻页了，新进度要放行");
        assert!(!suppressed(&first.key), "新进度放行后解除抑制");
    }
}
