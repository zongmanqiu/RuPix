//! 插件清单的远程镜像与本地缓存。
//!
//! 与解码器 `decoders::remote` 同形：随包 `<exe>/docs/plugins.json` 优先、零网络；
//! 点「刷新」才后台拉取当前来源的 `plugins.json`，解析成功后才原子覆盖本地文件。
//! 失败保留旧清单，状态行只说人话。
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::core::config::SourceId;

use super::{parse_catalog, AvailablePlugin, Extension};

const MAX_BYTES: usize = 1 << 20;
pub const INDEX_FILE: &str = "plugins.json";

pub fn local_path() -> Option<PathBuf> {
    Some(crate::core::paths::docs_dir()?.join(INDEX_FILE))
}

pub fn load_local(installed: &[Extension]) -> Result<Vec<AvailablePlugin>, String> {
    let path = local_path().ok_or_else(|| "清单异常，请「刷新」".to_string())?;
    load_from(&path, installed)
}

fn load_from(path: &Path, installed: &[Extension]) -> Result<Vec<AvailablePlugin>, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        log::warn!("读本地插件清单 {} 失败：{e}", path.display());
        "清单异常，请「刷新」".to_string()
    })?;
    if bytes.len() > MAX_BYTES {
        return Err("清单异常，请「刷新」".to_string());
    }
    let text = String::from_utf8(bytes).map_err(|_| "清单异常，请「刷新」".to_string())?;
    let ids = installed_ids(installed);
    parse_catalog(&text, &ids)
}

pub fn save_local(text: &str) -> Result<(), String> {
    let path = local_path().ok_or_else(|| "清单异常，请「刷新」".to_string())?;
    save_to(&path, text)
}

fn save_to(target: &Path, text: &str) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or_else(|| "清单路径没有父目录".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建 docs 目录失败：{e}"))?;
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "清单路径没有文件名".to_string())?;
    let part = dir.join(format!("{name}.part"));
    std::fs::write(&part, text.as_bytes())
        .map_err(|e| format!("写不进 {}（{e}）——目录是不是只读？", part.display()))?;
    let old = dir.join(format!("{name}.old"));
    let _ = std::fs::remove_file(&old);
    let had_old = target.exists();
    if had_old {
        if let Err(e) = std::fs::rename(target, &old) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("旧本地清单挪不开：{e}"));
        }
    }
    if let Err(e) = std::fs::rename(&part, target) {
        let _ = std::fs::remove_file(&part);
        if had_old {
            let _ = std::fs::rename(&old, target);
        }
        return Err(format!("本地清单落盘失败：{e}"));
    }
    if had_old {
        let _ = std::fs::remove_file(&old);
    }
    Ok(())
}

/// 从某个地址拉清单正文。地址由调用方从配置传进来（`plugin_index_*`，ini 可改）。
fn fetch(url: &str) -> Result<(Vec<AvailablePlugin>, String), String> {
    let bytes = crate::platform::http::get(url, MAX_BYTES)?;
    let text = String::from_utf8(bytes).map_err(|_| "清单不是合法 UTF-8".to_string())?;
    let list = parse_catalog(&text, &[])?;
    Ok((list, text))
}

/// ★ 只有成败两态（2026-10-29 用户口径，与解码器同一套）：取到清单但写不进本地文件
///   也按失败处理，刚取到的那份一并丢掉。
#[derive(Debug)]
pub struct RefreshOutcome {
    pub src: SourceId,
    pub result: Result<Vec<AvailablePlugin>, String>,
}

#[derive(Default)]
struct RefreshSlot {
    running: bool,
    done: Option<RefreshOutcome>,
}

static REFRESH: Mutex<RefreshSlot> = Mutex::new(RefreshSlot {
    running: false,
    done: None,
});

/// 清单刷新失败 → 界面上那一句（与解码器页同一套：细节只进日志）。
pub fn catalog_error_text() -> &'static str {
    "更新失败"
}

/// 发起刷新失败的原因：界面只分两句（仍在跑 = 「更新中…」，其余 = 「更新失败」）。
#[derive(Debug)]
pub enum BeginError {
    Busy,
    Refused(String),
}

impl BeginError {
    pub fn ui_text(&self) -> &'static str {
        match self {
            Self::Busy => "更新中…",
            Self::Refused(_) => catalog_error_text(),
        }
    }
}

impl std::fmt::Display for BeginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("已有一次刷新在跑"),
            Self::Refused(e) => f.write_str(e),
        }
    }
}

/// 起一次后台刷新；`src` 只用于标注"这次拉的是哪个镜像"，去哪拉由 `url`（来自配置）决定。
pub fn start_refresh(src: SourceId, url: String) -> Result<(), BeginError> {
    // ini 里的地址是自由文本：非 http(s) 直接拒在第 0 步（理由同 `decoders::remote`）。
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(BeginError::Refused("更新地址不是 http(s) 链接".to_string()));
    }
    {
        let mut g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
        if g.running {
            return Err(BeginError::Busy);
        }
        g.running = true;
    }
    std::thread::spawn(move || {
        let result = match fetch(&url) {
            Ok((list, text)) => match save_local(&text) {
                Ok(()) => Ok(list),
                Err(e) => {
                    log::warn!("插件清单未能写入本地文件，本次刷新按失败处理：{e}");
                    Err(e)
                }
            },
            Err(e) => Err(e),
        };
        let mut g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
        g.running = false;
        g.done = Some(RefreshOutcome { src, result });
    });
    Ok(())
}

/// 按配置里当前来源**及它对应的地址**刷新（`download_source` / `plugin_index_*`
/// 两族键的直接行为读者，设置页「刷新」按钮走这里）。
pub fn start_refresh_for(c: &crate::core::config::Config) -> Result<(), BeginError> {
    let url = match c.download_source {
        SourceId::Gitee => c.plugin_index_gitee.clone(),
        SourceId::Github => c.plugin_index_github.clone(),
    };
    start_refresh(c.download_source, url)
}

pub fn is_refreshing() -> bool {
    REFRESH.lock().unwrap_or_else(|e| e.into_inner()).running
}

pub fn poll_refresh() -> Option<RefreshOutcome> {
    let mut g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    g.done.take()
}

pub fn installed_ids(installed: &[Extension]) -> Vec<String> {
    installed.iter().map(|e| e.manifest.id.clone()).collect()
}

/// 版本比较：逐段数字，缺段当 0；发布版大于同号预发布版。
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn parts(v: &str) -> Vec<(u64, String)> {
        v.trim()
            .trim_start_matches(['v', 'V'])
            .split('.')
            .map(|seg| {
                let n = seg.chars().take_while(char::is_ascii_digit).count();
                let num = seg[..n].parse().unwrap_or(0);
                (num, seg[n..].to_string())
            })
            .collect()
    }
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let (na, sa) = pa.get(i).cloned().unwrap_or((0, String::new()));
        let (nb, sb) = pb.get(i).cloned().unwrap_or((0, String::new()));
        match na.cmp(&nb) {
            Ordering::Equal => {}
            other => return other,
        }
        let tail = match (sa.is_empty(), sb.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => sa.cmp(&sb),
        };
        if tail != Ordering::Equal {
            return tail;
        }
    }
    Ordering::Equal
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    Absent,
    Installed,
    Upgradable,
    Ahead,
}

pub fn install_state(e: &AvailablePlugin, installed: &[Extension]) -> InstallState {
    use std::cmp::Ordering;
    let Some(cur) = installed.iter().find(|x| x.manifest.id == e.id) else {
        return InstallState::Absent;
    };
    match version_cmp(&cur.manifest.version, &e.version) {
        Ordering::Less => InstallState::Upgradable,
        Ordering::Greater => InstallState::Ahead,
        Ordering::Equal => InstallState::Installed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(id: &str, version: &str) -> Extension {
        Extension {
            manifest: crate::core::ext::Manifest {
                schema: 1,
                id: id.into(),
                name: id.into(),
                version: version.into(),
                protocol: 1,
                min_host: "0.1.0".into(),
                kind: crate::core::ext::Kind::Command,
                entry: "x.exe".into(),
                args: Vec::new(),
                description: String::new(),
                capabilities: Vec::new(),
                platforms: vec!["windows-x64".into()],
                author: String::new(),
                license: String::new(),
                released: String::new(),
                timeout_ms: None,
                hotkey: None,
                settings_ui: false,
                settings: Vec::new(),
            },
            dir: PathBuf::new(),
            enabled: true,
            blocked: false,
            isolated: false,
        }
    }

    #[test]
    fn version_order_is_numeric() {
        use std::cmp::Ordering;
        assert_eq!(version_cmp("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(version_cmp("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(version_cmp("1.0.0", "1.0.0-beta"), Ordering::Greater);
    }

    #[test]
    fn states_follow_versions() {
        let list = vec![ext("a", "1.0.0"), ext("b", "2.0.0")];
        let entry = |id: &str, version: &str| AvailablePlugin {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            protocol: 1,
            min_host: "0.1.0".into(),
            kind: crate::core::ext::Kind::Command,
            description: String::new(),
            file: "x.zip".into(),
            size: 1,
            capabilities: Vec::new(),
            released: String::new(),
            author: String::new(),
            license: String::new(),
            urls: std::collections::HashMap::new(),
            installed: false,
        };
        assert_eq!(
            install_state(&entry("a", "1.0.0"), &list),
            InstallState::Installed
        );
        assert_eq!(
            install_state(&entry("a", "1.1.0"), &list),
            InstallState::Upgradable
        );
        assert_eq!(
            install_state(&entry("b", "1.9.0"), &list),
            InstallState::Ahead
        );
        assert_eq!(
            install_state(&entry("c", "1.0.0"), &list),
            InstallState::Absent
        );
    }

    #[test]
    fn local_catalog_round_trips() {
        let dir = crate::core::paths::test_scratch_root()
            .join(format!("rupix-plugin-cat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(INDEX_FILE);
        assert!(load_from(&path, &[]).is_err());
        let text = r#"[{"id":"com.example.test","name":"A","version":"1.0.0","protocol":1,"kind":"command","file":"a.zip","size":3}]"#;
        save_to(&path, text).expect("save");
        assert_eq!(load_from(&path, &[]).expect("load").len(), 1);
        save_to(&path, text).expect("replace");
        assert!(!dir.join(format!("{INDEX_FILE}.part")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_failure_is_one_short_sentence() {
        assert_eq!(catalog_error_text(), "更新失败");
        assert!(!catalog_error_text().contains("0x"));
        assert!(!catalog_error_text().contains("HTTP"));
    }

    #[test]
    fn busy_refresh_keeps_the_in_progress_sentence() {
        assert_eq!(BeginError::Busy.ui_text(), "更新中…");
        assert_eq!(BeginError::Refused("坏地址".into()).ui_text(), "更新失败");
    }
}
