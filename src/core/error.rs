//! 统一错误类型。
//!
//! 只有两类，刻意不做细粒度分类：看图器对错误的处理只有两种——**换下一张**或
//! **在状态栏说一句人话**。再细的分类在这两个消费点上没有区别，只会增加 match 分支。
//!
//! ★ 解码链路的失败不走这里（`core::decode` 用 `String` 直传解码器/渲染器给的
//!   人话）；这里的 `Decode` / `TooLarge` / `Empty` 三个变体曾长期无人构造，
//!   2026-10-19（m52）删除。0 字节文件现在在 `decode_one` 入口直接判空。

use std::fmt;
use std::path::PathBuf;

pub type ViewResult<T> = Result<T, ViewError>;

#[derive(Debug)]
pub enum ViewError {
    /// 文件读不到（不存在 / 被占用 / 权限）。
    Io(std::io::Error),
    /// 目录里没有可显示的图片。
    NoImages(PathBuf),
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
