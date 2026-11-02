//! 外挂解码器：`<exe 目录>/decoders/`。
//!
//! # 为什么要有这个目录
//!
//! RuPix 的像素来源是 `decoders/` 里的解码 DLL（SVG 例外，走内置 resvg），
//! 能打开什么格式取决于装了哪些解码器。想要更多格式有两条路：
//!
//! | 路线 | 代价 | 结论 |
//! |------|------|------|
//! | 装 Store「图像解码器」 | 依赖外部商店，换机就丢，违背便携 | ❌ 已否决 |
//! | 把解码器塞进主 exe | 每种格式都涨体积，违背"小巧" | ❌ 已否决 |
//! | **让第三方 DLL 躺到 exe 旁边的 `decoders/`** | 拷贝即用、不联网、不加依赖 | ✅ 采用 |
//!
//! 这个目录就是第三条路的挂载点：主程序不加任何解码依赖，用户（或未来的我们）往
//! `decoders/` 里丢一个 DLL，它声明的格式就变成"可能打得开"。
//!
//! # 目录里只有 DLL
//!
//! 没有清单文件、没有哈希：解码器的 id、显示名、作用说明、认领的扩展名，全部通过
//! `iv_decoder_info()` 从 DLL 自己读出来（见 [`crate::decoders`]）。传播时只需
//! 拷一个 DLL，宿主也少一层"清单与文件对不对得上"的维护。
//!
//! # 诚实的边界：止损，不是沙箱
//!
//! 解码器与宿主同进程（方案 v2 §四 路线 A）。`catch_unwind` 接得住 Rust panic；
//! 访问违规、非法指令、特权指令、读页错误、**栈溢出**这些 SEH 硬故障由
//! [`crate::platform::seh`] 的 C 隔离层（`__try/__except`）拦下，折算成
//! `SehFault` / `MediaFault`，不再把主程序一起带走。但这只是**止损**：
//! 解码器仍与宿主共享地址空间，读得到宿主内存、改得坏宿主状态。真正的隔离要上
//! 路线 B（独立解码进程），本轮不做。见 [`crate::decoders`] 模块头。

use std::path::{Path, PathBuf};

/// 解码器目录名（相对 exe 目录）。改它等于改所有解码器的约定。
pub const DIR_NAME: &str = "decoders";

/// 解码器目录的绝对路径。**不依赖 CWD**（方案 v2 §12.5：从快捷方式启动时 CWD 不是
/// exe 目录，写相对路径必然炸）。
///
/// 环境变量 `RUPIX_DECODERS_DIR` 可整体覆盖（绝对路径）——仅供探针/基准等开发场景
/// 指向 `.build/decoders`，避免 example 旁的 decoders 拷贝过期后测的是陈旧 DLL
/// （2026-10-10 实测踩坑：旧拷贝 z 优化 135ms vs 新版 opt3 30ms，数字全错）。
pub fn dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RUPIX_DECODERS_DIR") {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            return Some(pb);
        }
    }
    Some(crate::core::paths::exe_dir()?.join(DIR_NAME))
}

/// 建目录（幂等）。用户点「打开目录」时才建——不该在 exe 旁边凭空多出一个空文件夹。
pub fn ensure_dir() -> std::io::Result<PathBuf> {
    // 取不到 exe 目录就**不建**。以前这里回落成相对路径 `decoders`，等于在**当前工作目录**
    // 旁边凭空造一个目录（禁 CWD 是硬规矩：运行期落点只认 exe 旁边，见 `core::paths` 模块头），
    // 而调用方是设置页那个「打开目录」按钮——它会打开一个用户从没见过的文件夹。
    let d = dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "取不到 exe 目录，解码器目录无处可建",
        )
    })?;
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// 一次目录扫描的结果。
#[derive(Debug, Clone, Default)]
pub struct Scan {
    /// `None` = 连 exe 目录都取不到（极罕见）。
    pub dir: Option<PathBuf>,
    pub exists: bool,
    /// 扫描本身失败的原因（`exists` 为真但读不到时才有）。
    pub error: Option<String>,
    /// 目录里的解码器（每个 DLL 一行）。见 [`crate::decoders::list_decoders`]。
    pub decoders: Vec<crate::decoders::DecoderEntry>,
}

/// 扫当前进程的解码器目录。
///
/// 只在用户打开「设置 → 解码器」时被调用；**不进启动关键路径**（方案 v2 §12.2，
/// 一切为了冷启动 < 100ms）。
pub fn scan() -> Scan {
    let Some(dir) = dir() else {
        return Scan::default();
    };
    let mut s = Scan {
        dir: Some(dir.clone()),
        ..Default::default()
    };
    s.exists = dir.is_dir();
    if !s.exists {
        return s; // 从没建过解码器目录是常态，不是错误。
    }
    if let Err(e) = std::fs::read_dir(&dir) {
        s.error = Some(e.to_string());
    }
    s.decoders = crate::decoders::list_decoders();
    s
}

/// 一行状态摘要（给设置窗的状态行用）。
pub fn summary(s: &Scan) -> String {
    match (&s.dir, s.exists) {
        (None, _) => "取不到 exe 目录，解码器不可用".to_string(),
        (Some(_), false) => "目录尚未创建".to_string(),
        (Some(_), true) => {
            if let Some(e) = &s.error {
                return format!("读取失败：{e}");
            }
            if s.decoders.is_empty() {
                "目录为空".to_string()
            } else {
                format!("发现 {} 个解码器", s.decoders.len())
            }
        }
    }
}

/// 在资源管理器里打开目录（把 DLL 拖进去很方便）。
///
/// 用 `explorer` 而不是 `ShellExecuteW`：后者要为此多开一个 windows feature，
/// 点到为止。失败只返回 `false`，调用方自己决定怎么告诉用户。
pub fn open_in_explorer(p: &Path) -> bool {
    std::process::Command::new("explorer")
        .arg(p)
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 已知格式必须**恰好落进一类**：内置 / 解码器认领。
    ///
    /// `known_exts = BUILTIN ∪ declared`，分类天然覆盖全部已知格式；
    /// 本测锁住「没有第三类空档」——曾存在的 `awaiting_formats()`
    /// （待支持）随 2026-10-10 WIC 兜底删除后恒为空集，已移除。
    #[test]
    fn every_known_ext_is_builtin_or_claimed() {
        let declared = crate::decoders::declared_exts();
        let known = crate::core::gallery::known_exts();
        assert!(!known.is_empty(), "格式清单不该为空");

        for e in &known {
            let builtin = crate::core::formats::is_builtin_ext(e);
            let claimed = declared.contains(e);
            assert!(
                builtin || claimed,
                "{e} 既不是内置也没有解码器认领——掉进了分类空档"
            );
        }

        // 内置的那几个永远不该被算成解码器认领。
        for e in crate::core::formats::builtin_exts() {
            assert!(crate::core::formats::is_builtin_ext(&e), "{e} 应为内置");
            assert!(!declared.contains(&e), "{e} 不该被解码器重复认领");
        }
    }

    /// 状态摘要：目录不存在 / 空 / 有内容三种口径。
    #[test]
    fn summary_reports_directory_state() {
        let none: Scan = Scan::default();
        assert!(summary(&none).contains("取不到") || summary(&none).contains("未创建"));
        let empty = Scan {
            dir: Some(PathBuf::from("x")),
            exists: true,
            ..Default::default()
        };
        assert_eq!(summary(&empty), "目录为空");
    }
}
