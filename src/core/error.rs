//! 统一错误类型与失败分类。
//!
//! 看图器对错误的处理只有两种——**换下一张**或**在画面中央说一句人话**。但"打不开"
//! 在用户那里至少是四件不同的事：缺解码器、文件坏了/解不动、文件根本不在、其余。
//! 混成一句"裂图"既不能告诉用户下一步做什么，也无法在日志里一眼分流。
//!
//! 因此 2026-11-13 起错误出口带 [`FailureKind`]：分类**按来源**在产生错误的那一层
//! 定夺（`ViewError::Io(NotFound)` / 解码器 `Fail` / 超时 / 压缩包解析），调用方
//! **不靠字符串猜**。原始判词仍完整保存在 [`ViewFailure::detail`] 里，进日志；
//! 画面只画「标题 + 一行提示」。
//!
//! ★ 解码链路的失败不走 [`ViewError`]：它用 [`ViewFailure`]（原因已由解码层分类）。
//!   这里的 `ViewError` 仍只覆盖"建清单 / 读目录"这一类。

use std::fmt;
use std::path::PathBuf;

pub type ViewResult<T> = Result<T, ViewError>;

/// 打不开的四类。顺序即设置页/日志里的展示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 没有解码器认领这个格式（或解码器已停用 / 本次会话因超时停用）。
    Unsupported,
    /// 解码器认领了但解不出来：损坏、过大、panic、超时。
    DecodeFailed,
    /// 文件不存在 / 不可访问（判据是 `io::ErrorKind::NotFound`）。
    FileMissing,
    /// 其余：压缩包解析失败、空目录、未归类的宿主错误。
    Other,
}

impl FailureKind {
    /// 画面中央第一行：分类标题。
    pub fn title(self) -> &'static str {
        match self {
            Self::Unsupported => "无法打开：缺少可用的解码器",
            Self::DecodeFailed => "解码失败",
            Self::FileMissing => "文件不存在或无法访问",
            Self::Other => "无法打开",
        }
    }

    /// 兜底提示（`detail` 为空时才用）。
    pub fn hint(self) -> &'static str {
        match self {
            Self::Unsupported => "在「设置 → 解码器」安装支持这个格式的解码器后重试",
            Self::DecodeFailed => "文件可能已损坏，或这个格式当前无法解码",
            Self::FileMissing => "它可能已被移动、删除，或所在磁盘没有连接",
            Self::Other => "详细原因见日志",
        }
    }

    /// I/O 错误 → 分类。只有 `NotFound` 算"文件不在了"；权限/占用/磁盘错误归 Other
    /// （它们不是"路径失效"，不该被最近列表自动摘掉）。
    pub fn from_io(e: &std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound {
            Self::FileMissing
        } else {
            Self::Other
        }
    }
}

/// 一条可显示、可进日志的失败：分类 + 原始判词。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFailure {
    pub kind: FailureKind,
    /// 原始判词（解码器/系统/压缩包给的原文）。**不丢**：日志与悬停/排查都靠它。
    pub detail: String,
}

impl ViewFailure {
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::new(FailureKind::Unsupported, detail)
    }

    pub fn decode(detail: impl Into<String>) -> Self {
        Self::new(FailureKind::DecodeFailed, detail)
    }

    pub fn other(detail: impl Into<String>) -> Self {
        Self::new(FailureKind::Other, detail)
    }

    /// 读文件失败：`NotFound` → FileMissing，其余 → Other；原文带上 OS 判词。
    pub fn from_io(e: std::io::Error) -> Self {
        Self::new(FailureKind::from_io(&e), format!("读不到文件：{e}"))
    }

    /// 画面中央那两行：分类标题 + 一行提示。
    ///
    /// `detail` 只在**有信息量且不是 FileMissing** 时充当第二行：FileMissing 的系统
    /// 判词对用户没有增量（"系统找不到指定的文件"），换成一句可行动的提示更有用；
    /// 其余三类（超大图上限、超时停用解码器、压缩包错误）原文本身就是最好的下一步。
    pub fn display_text(&self) -> String {
        let hint = match self.kind {
            FailureKind::FileMissing => self.kind.hint().to_string(),
            _ if !self.detail.trim().is_empty() => self.detail.clone(),
            _ => self.kind.hint().to_string(),
        };
        format!("{}\n{hint}", self.kind.title())
    }
}

impl fmt::Display for ViewFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl From<String> for ViewFailure {
    /// 兜底：还没有分类的上游错误按 Other 收下，原文不丢。
    fn from(s: String) -> Self {
        Self::other(s)
    }
}

impl From<&str> for ViewFailure {
    fn from(s: &str) -> Self {
        Self::other(s)
    }
}

#[derive(Debug)]
pub enum ViewError {
    /// 文件读不到（不存在 / 被占用 / 权限）。
    Io(std::io::Error),
    /// 目录里没有可显示的图片。
    NoImages(PathBuf),
}

impl ViewError {
    /// 分类 + 原文。`NoImages` 不是"文件坏了"，也不是"缺解码器"，归 Other。
    pub fn into_failure(self) -> ViewFailure {
        match self {
            Self::Io(e) => ViewFailure::from_io(e),
            Self::NoImages(p) => {
                ViewFailure::other(format!("目录里没有可显示的图片：{}", p.display()))
            }
        }
    }
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViewError::Io(e) => write!(f, "读不到文件：{e}"),
            ViewError::NoImages(p) => write!(f, "目录里没有可显示的图片：{}", p.display()),
        }
    }
}

impl std::error::Error for ViewError {}

impl From<std::io::Error> for ViewError {
    fn from(e: std::io::Error) -> Self {
        ViewError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn io_classification_only_treats_not_found_as_file_missing() {
        let missing = Error::from(ErrorKind::NotFound);
        assert_eq!(FailureKind::from_io(&missing), FailureKind::FileMissing);
        let denied = Error::from(ErrorKind::PermissionDenied);
        assert_eq!(FailureKind::from_io(&denied), FailureKind::Other);
        assert_eq!(
            ViewError::Io(missing).into_failure().kind,
            FailureKind::FileMissing
        );
        assert_eq!(
            ViewError::Io(denied).into_failure().kind,
            FailureKind::Other
        );
    }

    #[test]
    fn four_kinds_have_distinct_titles_and_file_missing_keeps_detail_in_log_only() {
        let all = [
            FailureKind::Unsupported,
            FailureKind::DecodeFailed,
            FailureKind::FileMissing,
            FailureKind::Other,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.title(), b.title(), "四类标题必须各不相同");
            }
        }
        let missing = ViewFailure::new(FailureKind::FileMissing, "读不到文件：os error 2");
        let text = missing.display_text();
        assert!(text.contains("文件不存在"), "{text}");
        assert!(
            !text.contains("os error 2"),
            "FileMissing 的 OS 原文只进日志"
        );
        assert_eq!(missing.detail, "读不到文件：os error 2", "原文不许丢");
    }

    #[test]
    fn other_kinds_show_the_original_detail_under_the_title() {
        let too_big = ViewFailure::decode("这张图太大（9000×9000）");
        let text = too_big.display_text();
        assert!(text.starts_with("解码失败\n"), "{text}");
        assert!(text.contains("这张图太大"), "现有好文案必须保留：{text}");

        let unsupported = ViewFailure::unsupported("解码器「x」本次会话已因超时停用");
        let text = unsupported.display_text();
        assert!(text.starts_with("无法打开：缺少可用的解码器\n"), "{text}");
        assert!(text.contains("已因超时停用"), "{text}");

        let other = ViewFailure::other("这一层没有图片：从左侧文件树进入它的子层");
        assert!(other.display_text().contains("左侧文件树"));
    }

    #[test]
    fn empty_detail_falls_back_to_the_kind_hint() {
        let f = ViewFailure::new(FailureKind::DecodeFailed, "");
        assert_eq!(
            f.display_text(),
            "解码失败\n文件可能已损坏，或这个格式当前无法解码"
        );
    }

    #[test]
    fn string_fallback_is_other_and_keeps_the_text() {
        let f: ViewFailure = "压缩包解析失败".to_string().into();
        assert_eq!(f.kind, FailureKind::Other);
        assert_eq!(f.detail, "压缩包解析失败");
    }
}
