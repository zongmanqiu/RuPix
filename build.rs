// 宿主构建脚本：编译 C SEH 隔离层（仅 Windows）+ 生成 exe 版本资源。
//
// `seh_shim.c` 提供一个 `sv_seh_call`，用 C 的 `__try/__except` 包住一次解码器 FFI
// 调用，把真正的访问违规降级成错误码（详见 `seh_shim.c` 头部注释）。
// 静态 CRT（`/MT`）与宿主的 `+crt-static` 对齐，避免混用动态/静态 CRT 的链接冲突。

use std::path::PathBuf;

/// `0.2.0` → `0,2,0,0`：VERSIONINFO 要四段数值，第四段留给"构建号"（我们没有，填 0）。
fn version_quad(v: &str) -> String {
    let mut parts: Vec<String> = v
        .split('.')
        .map(|p| {
            let d: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            if d.is_empty() {
                "0".into()
            } else {
                d
            }
        })
        .collect();
    parts.resize(4, "0".into());
    parts.truncate(4);
    parts.join(",")
}

/// 把 `assets/version.rc.in` 的版本占位符换成 Cargo.toml 里的真值，写进 `OUT_DIR`。
fn render_version_rc(template: &str, out_dir: &str) -> PathBuf {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let quad = version_quad(&version);
    // `Debug` 由 cargo 传（"true"/"false"）：调试版在属性页上带 VS_FF_DEBUG 位，
    // 用户截图报障时一眼能看出这不是发布版。
    let debug = std::env::var("DEBUG").map(|d| d == "true").unwrap_or(false);
    let body = template
        .replace("@FILEVERSION_NUM@", &quad)
        .replace("@PRODUCTVERSION_NUM@", &quad)
        .replace("@VERSION_STR@", &version)
        .replace("@FILEFLAGS_NUM@", if debug { "0x1L" } else { "0x0L" });
    let path = PathBuf::from(out_dir).join("rupix_version.rc");
    // 写失败不硬失败：缺版本资源由 deploy.py 的 RT_VERSION 门禁拦住（与图标同一套路）。
    if let Err(e) = std::fs::write(&path, body) {
        println!("cargo:warning=版本资源生成失败：{e}");
    }
    path
}

fn main() {
    // 让 cargo 跟踪 seh_shim.c 的改动：它变了就重跑本构建脚本，cc 才会按 mtime
    // 重新编译出新的 seh_shim.lib（否则会链接到旧的、仍引用 GetExceptionCode 的目标文件）。
    println!("cargo:rerun-if-changed=seh_shim.c");
    println!("cargo:rerun-if-changed=build.rs");
    // PE 图标链路：ico 或 rc 变了就重嵌（ico 由 `cargo run --example gen_icon` 生成）。
    println!("cargo:rerun-if-changed=assets/rupix.rc");
    println!("cargo:rerun-if-changed=assets/rupix.ico");
    // 版本资源的两处输入：模板本身 + 版本号的唯一来源 Cargo.toml。
    println!("cargo:rerun-if-changed=assets/version.rc.in");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // cc/rc 都按**目标**平台走，不看 build.rs 自己的 cfg(windows)（那是 host 的）：
    // host≠target 的交叉编译会错位——shim 是链给目标机的，图标是烙进目标 exe 的。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cc::Build::new()
            .file("seh_shim.c")
            .static_crt(true)
            .compile("seh_shim");

        // 把 `assets/rupix.rc`（1 ICON "rupix.ico"）烙进 exe 的 .rsrc：资源管理器、
        // 任务管理器、固定快捷方式显示的图标来自这里；窗口/托盘/标题栏的图标走
        // 运行时 SVG 光栅化（brand::logo_rgba），两条路同一份 assets/icon.svg。
        // rc.exe 发现链：`RC_<target>`/`RC` env → Windows Kits 注册表 → PATH；
        // manifest_optional：无资源编译器时降级为警告而非硬失败（与 windui build.rs 同款）
        // ——真失败会被 brand 的 pe_icon_asset 测试与 deploy.py 的 PE 门禁拦住。
        if let Err(e) =
            embed_resource::compile("assets/rupix.rc", embed_resource::NONE).manifest_optional()
        {
            println!("cargo:warning=exe 图标资源嵌入跳过：{e}");
        }

        // VERSIONINFO：资源管理器"详细信息"页、Alt-Tab 提示、崩溃上报要看的版本/公司/描述。
        // 缺它的具体代价不是"不好看"而是**误报与对不上号**：无版本资源的 exe 是打包器/
        // 恶意样本的典型特征，杀软启发式对它更凶；而用户报障时我们连"哪一版"都问不出来
        // （`.symbols/` 靠 exe 大小 + 构建时间配对 PDB，那正是版本缺位的替代方案）。
        // 模板里不 #include 任何东西，理由与符号字面值的出处见 `assets/version.rc.in` 头部。
        if let (Ok(out_dir), Ok(template)) = (
            std::env::var("OUT_DIR"),
            std::fs::read_to_string("assets/version.rc.in"),
        ) {
            let rc = render_version_rc(&template, &out_dir);
            if let Err(e) = embed_resource::compile(&rc, embed_resource::NONE).manifest_optional() {
                println!("cargo:warning=exe 版本资源嵌入跳过：{e}");
            }
        } else {
            println!("cargo:warning=版本资源模板 assets/version.rc.in 或 OUT_DIR 取不到，本次不嵌 VERSIONINFO");
        }
    }
}
