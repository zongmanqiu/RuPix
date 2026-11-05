//! 解码器 ABI 契约的**宿主侧入口**：宿主只认 `main/vendor/rupix-abi/` 这份逐字节快照。
//!
//! # 宿主拿到的是一份快照副本
//!
//! 克隆宿主不要求先克隆解码器仓库，所以 `main/vendor/rupix-abi/` 存着契约的**逐字节快照**。规则只有三条：
//!
//! 1. **不要在宿主这边改 `vendor/rupix-abi/src/lib.rs`**。改了下次同步被覆盖，
//!    而解码器侧仍是旧布局——表现是宿主按错误长度分配像素缓冲、解码器越界写。
//!    下面 `abi_snapshot_matches_upstream_when_configured` 会把这种漂移跑成测试红灯。
//! 2. 要改契约，去解码器仓库改 `abi/src/lib.rs`，然后**把那一份文件整份复制**到这里：
//!    `main/vendor/rupix-abi/src/lib.rs`（覆盖、不编辑），并按 ABI 冻结规则决定是否 bump
//!    `IV_ABI_VERSION`（bump 必须用户拍板）。宿主侧不写同步脚本，解码器仓库的实际位置由维护者掌握。
//! 3. 两份必须逐字节一致（含文档注释）——守卫是内容比对，不做"语义等价"的宽松判断。
//!    快照里除契约结构体，还带着解码器侧样板 `guard` / `set_last_error` / `export_decoder!`
//!    （2026-10-16 起 kit 与契约同一个 crate）。宿主用不到它们，但**不许在宿主侧删掉这一段**——
//!    删了就等于制造快照漂移，守卫测试会把红灯点亮。
//!
//! # 为什么只剩 re-export
//!
//! 本文件只负责把名字以 `crate::core::abi::*` 的形式交给宿主其余模块
//! （`decoders.rs`、`decode.rs` 等），全部调用点零改动。
//!
//! 契约细节见 `vendor/rupix-abi/src/lib.rs` 的模块文档；宿主侧对接说明见
//! `docs/解码器开发规范.md`。

pub use rupix_abi::*;

/// 宿主对解码器 ABI 的**必备导出**（任何 DLL 都必须有，缺席 = 不收）。
pub const ABI_REQUIRED_SYMBOLS: &[&str] = &[
    "iv_abi_version",
    "iv_decoder_info",
    "iv_probe",
    "iv_render",
    "iv_last_error",
];

/// 宿主对解码器 ABI 的**可选导出**（缺席 = 少一项能力，不是错误）。
pub const ABI_OPTIONAL_SYMBOLS: &[&str] = &[
    "iv_caps",
    "iv_metadata",
    "iv_render_raw",
    "iv_render_region",
];

/// **已退役**的导出：ABI 冻结保留符号（不删，避免破坏既有 DLL 的导出表），
/// 但宿主**不再调用**——矢量导出路线（`HostExport`）已整条删除。
///
/// 出现在生产代码里 = 那条路线被复活了 ⇒ 红灯。
///
/// ★ 名字用 `concat!` 拆开写：**这份清单本身就在生产代码里**，
/// 写成完整字面量会被下面那条闸当成"复活"而自己撞自己。
/// （细节看着别扭，但这是"声明不许用"与"不许出现"之间的必然差。）
pub const ABI_RETIRED_SYMBOLS: &[&str] = &[concat!("iv_", "export_svg")];

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// 宿主侧对照钉子：re-export 通了、ABI 版本一致。
    /// 结构体布局与错误码/枚举的逐项冻结测试在契约 crate 里。
    #[test]
    fn reexport_is_wired() {
        use std::mem::size_of;

        assert_eq!(IV_ABI_VERSION, 6);
        assert_eq!(ERR_OK, 0);
        assert_eq!(ERR_TOO_LARGE, 7);
        assert_eq!(size_of::<IvRenderReq>(), 24);
        assert_eq!(size_of::<IvRenderOut>(), 48);
        // v6 新增：区域渲染请求（ABI v6 的"矢量放大到 4000% 也无损"的通道）。
        assert_eq!(size_of::<IvRegionReq>(), 40);
    }

    /// 快照漂移守卫：维护者显式设置 `RUPIX_ABI_UPSTREAM` 时，把该文件与
    /// `vendor/rupix-abi/src/lib.rs` 逐字节比对；未设置时跳过。
    ///
    /// 宿主不在代码里写死解码器仓库路径；需要比对的维护者用环境变量指向
    /// 解码器仓库的 `abi/src/lib.rs`，CI/本地都可选。
    #[test]
    fn abi_snapshot_matches_upstream_when_configured() {
        let main_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(upstream) = std::env::var_os("RUPIX_ABI_UPSTREAM").map(std::path::PathBuf::from)
        else {
            eprintln!("skip: 未设置 RUPIX_ABI_UPSTREAM，无上游可比对");
            return;
        };
        if !upstream.exists() {
            eprintln!("skip: RUPIX_ABI_UPSTREAM 指向的文件不存在");
            return;
        }
        let snapshot = main_dir.join("vendor/rupix-abi/src/lib.rs");
        let a = fs::read(&upstream).expect("读上游契约失败");
        let b = fs::read(&snapshot).expect("读宿主快照失败");
        assert_eq!(
            a.len(),
            b.len(),
            "ABI 快照长度不同：宿主 {snapshot:?} 落后于上游 {upstream:?}；\n\
             请把解码器仓库的 abi/src/lib.rs 整份复制到 vendor/rupix-abi/src/lib.rs"
        );
        assert_eq!(
            a, b,
            "ABI 快照内容与上游不一致；请把解码器仓库的 abi/src/lib.rs 整份复制到 vendor/rupix-abi/src/lib.rs"
        );
    }

    // ── 宿主 ⇄ 解码器的接口边界（机器闸）────────────────────────────

    /// ★ **宿主对解码器 ABI 的依赖面冻结**（2026-10-28）。
    ///
    /// 用户口径："**解码器不再改动**（反复改，解码器那边开发会更麻烦）"。
    /// 这条测试是那句口径的机器凭据：**宿主侧的任何新功能都不得引入新的 ABI 依赖**。
    ///
    /// 扫宿主源码（`src/` + `examples/`）里出现的全部 `iv_*` 符号：
    /// 1. 必须落在**冻结集合**内（必备 5 个 + 可选 4 个）；
    /// 2. **退役符号**（`iv_export_svg`）只允许出现在测试文件里
    ///    ——生产代码里出现它 = 矢量导出那条路被复活了；
    /// 3. 冻结集合里的**必备符号**必须真的被宿主用到
    ///    ——防止有人删了实现却留着白名单（白名单不是许愿池）。
    ///
    /// 红灯 ⇒ 这不是"改一行就行"的事：必须走既有流程
    /// （用户拍板 → 在解码器仓改契约 → 加法式 bump ABI → 整份复制快照到 `vendor/`）。
    #[test]
    fn host_abi_dependency_surface_is_frozen() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut hits: Vec<(String, String, bool)> = Vec::new(); // (符号, 相对路径, 是否测试区)
        for sub in ["src", "examples"] {
            let dir = root.join(sub);
            let mut files = Vec::new();
            walk_rs(&dir, &mut files);
            for f in files {
                let text = fs::read_to_string(&f).unwrap_or_default();
                let rel = f
                    .strip_prefix(root)
                    .unwrap_or(&f)
                    .to_string_lossy()
                    .replace('\\', "/");
                // 测试区的判定：① 整文件就是测试（`tests.rs` / `tests/` 目录）；
                // ② 同文件里 `#[cfg(test)]` 之后的部分（内联测试模块）。
                let file_is_test = rel.ends_with("tests.rs") || rel.contains("/tests/");
                let test_from = text.find("#[cfg(test)]").unwrap_or(usize::MAX);
                for sym in abi_symbols_in(&text) {
                    let at = text.find(&sym).unwrap_or(0);
                    let in_test = file_is_test || at >= test_from;
                    hits.push((sym, rel.clone(), in_test));
                }
            }
        }
        assert!(!hits.is_empty(), "扫描不到任何 iv_* 符号——扫描器本身坏了");

        let frozen: Vec<&str> = ABI_REQUIRED_SYMBOLS
            .iter()
            .chain(ABI_OPTIONAL_SYMBOLS.iter())
            .copied()
            .collect();
        let retired: Vec<&str> = ABI_RETIRED_SYMBOLS.to_vec();

        for (sym, path, in_test) in &hits {
            if retired.contains(&sym.as_str()) {
                assert!(
                    *in_test,
                    "退役符号「{sym}」出现在生产代码 {path}：矢量导出路线（HostExport）已整条删除，\
                     矢量格式一律由解码器按目标尺寸自渲染（宪章 §5.2）"
                );
                continue;
            }
            assert!(
                frozen.contains(&sym.as_str()),
                "宿主引入了冻结集合外的 ABI 符号「{sym}」（{path}）：\
                 宿主侧改动不得触碰解码器接口；要新增能力请走「用户拍板 → 解码器仓改契约 → \
                 加法式 bump ABI → 整份复制快照到 vendor/rupix-abi/src/lib.rs」的既有流程"
            );
        }

        // 必备符号必须真的被生产代码用到（白名单不是许愿池）。
        for sym in ABI_REQUIRED_SYMBOLS {
            let used = hits.iter().any(|(s, _, t)| s == sym && !*t);
            assert!(
                used,
                "必备 ABI 符号「{sym}」在宿主生产代码里没有出现——白名单与实现脱节了"
            );
        }
    }

    /// 递归收集 `.rs` 文件。
    fn walk_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_rs(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }

    /// 取出文本里全部 `iv_*` 符号（词边界：前一个字符不是标识符字符，
    /// 免得 `div_ceil` 这种子串被当成符号）。
    fn abi_symbols_in(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let b = text.as_bytes();
        let mut i = 0usize;
        while i + 3 <= b.len() {
            if &b[i..i + 3] == b"iv_" {
                let start = i;
                let mut j = i + 3;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let prev_ok =
                    start == 0 || !(b[start - 1].is_ascii_alphanumeric() || b[start - 1] == b'_');
                if prev_ok && j > start + 3 {
                    out.push(String::from_utf8_lossy(&b[start..j]).to_string());
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }
}
