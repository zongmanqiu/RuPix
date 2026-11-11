//! 无头命令面的结构性判据（m293）。
//!
//! 为什么用 `include_str!` 读源码：`tests/` 链接不到 bin target，`main.rs` 里的接线
//! 没法靠调用来验证；而「共用解码链」「lib 里零个打印宏」本来就是**结构**性质，
//! 用编译期嵌入的源码文本钉住比运行时探针更稳，改一行就当场红。
//! stdout 那一行由 bin 真入口打印，lib 侧只产字符串（`run` 是纯函数，可直接单测）。

use rupix_lib::cli::{needs_instance_gate, Args, Headless, HeadlessCmd};
use rupix_lib::core::json;
use rupix_lib::headless::{base64, format_response, quote, EXIT_OK};
use std::path::PathBuf;

const HEADLESS_SRC: &str = include_str!("../src/headless.rs");
const MAIN_SRC: &str = include_str!("../src/main.rs");

/// ① 像素必须走与 GUI 同一条解码链，本模块不许自己装后端、不许碰 ABI。
/// ★ 负向臂：若有人在这里复制一套解码（`install_backend`）或另起 `decoders::Host`，判据红。
#[test]
fn pixels_use_the_shared_decode_chain() {
    assert!(
        HEADLESS_SRC.contains("core::decode::probe_decode"),
        "render 必须走共享解码链的 probe_decode"
    );
    assert!(
        HEADLESS_SRC.contains("core::decode::probe_thumb"),
        "thumbnail 必须走共享解码链的 probe_thumb"
    );
    for forbidden in ["install_backend", "decoders::Host", "iv_render", "iv_probe"] {
        assert!(
            !HEADLESS_SRC.contains(forbidden),
            "无头命令面不许另起解码链或碰 ABI：{forbidden}"
        );
    }
}

/// ② 后端装载在 `main` 里只有一处，且闸门例外走的是 lib 里的纯函数。
/// ★ 负向臂：加第二处 `install_decode_stack()` 或把闸门写回 main 里的裸条件，判据红。
#[test]
fn backend_is_installed_once_and_gate_is_delegated() {
    assert_eq!(
        MAIN_SRC.matches("install_decode_stack()").count(),
        1,
        "解码栈全进程只装一次，无头与 GUI 共用"
    );
    assert!(
        MAIN_SRC.contains("cli::needs_instance_gate"),
        "闸门例外判据必须下沉到 lib，否则没有单测盯着"
    );
    assert!(
        MAIN_SRC.contains("headless::run"),
        "无头分支必须接在 main 上"
    );
}

/// ③ lib 里零个打印宏；`run` / `arg_error` 产出的都是一行合法 JSON，
///    且 `code` 字段与退出码同值（打印归 bin 真入口，两条路径各一次）。
/// ★ 负向臂：谁在 lib 里加一条打印宏（把日志打进 stdout），这条当场红。
#[test]
fn response_is_one_line_and_lib_never_prints() {
    for m in ["println!(", "eprintln!(", "print!(", "eprint!(", "dbg!("] {
        assert!(!HEADLESS_SRC.contains(m), "lib 不许打印到 stdout：{m}");
    }
    assert_eq!(
        MAIN_SRC.matches("println!(\"{line}\")").count(),
        2,
        "缺参与正常请求各打印一次，不许有第三条"
    );

    let (code, line) = rupix_lib::headless::arg_error("缺路径");
    assert_eq!(code, rupix_lib::headless::EXIT_ARG);
    assert_eq!(line.lines().count(), 1);
    let v = json::parse(&line).expect("缺参响应必须是一行合法 JSON");
    assert_eq!(v.get("code").and_then(|x| x.as_i64()), Some(code as i64));

    let req = Headless {
        cmd: HeadlessCmd::Metadata,
        path: PathBuf::from("no-such-file-7b2.png"),
        target: None,
    };
    let (code, line) = rupix_lib::headless::run(&req);
    assert_eq!(code, 3, "文件不存在回 code 3");
    assert_eq!(line.lines().count(), 1);
    let v = json::parse(&line).expect("失败响应也必须是一行合法 JSON");
    assert_eq!(v.get("code").and_then(|x| x.as_i64()), Some(code as i64));
    assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(false));
}

/// ④ 响应是一行、可被自家解析器读回、字段齐全；失败响应同形。
#[test]
fn response_shape_is_machine_readable() {
    let ok = format_response(Some(HeadlessCmd::Render), EXIT_OK, ",\"bytes\":12");
    assert_eq!(ok.lines().count(), 1);
    let v = json::parse(&ok).expect("成功响应必须能被自家 JSON 解析器读回");
    assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
    assert_eq!(v.get("cmd").and_then(|x| x.as_str()), Some("render"));
    assert_eq!(v.get("code").and_then(|x| x.as_i64()), Some(0));
    assert_eq!(v.get("bytes").and_then(|x| x.as_u64()), Some(12));

    let bad = format_response(Some(HeadlessCmd::Render), 3, ",\"error\":\"解码失败\"");
    assert_eq!(bad.lines().count(), 1);
    let v = json::parse(&bad).expect("失败响应也必须是一行合法 JSON");
    assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(false));
    assert_eq!(v.get("code").and_then(|x| x.as_i64()), Some(3));
}

/// ⑤ 路径中文 / 反斜杠 / 引号转义后仍可解析；base64 补齐规则不变。
#[test]
fn quoting_and_base64_are_stable() {
    let raw = "D:\\照片\\a \"b\".png";
    let line = format!("{{\"p\":{}}}", quote(raw));
    assert_eq!(
        json::parse(&line)
            .unwrap()
            .get("p")
            .and_then(|x| x.as_str()),
        Some(raw)
    );
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
}

/// ⑥ 四个动词一律绕开单实例闸门；GUI 仍要进。
/// ★ 负向臂：旧公式对无头请求返回 true，证明 `is_headless` 这一项不可省。
#[test]
fn every_headless_verb_bypasses_the_gate() {
    for cmd in [
        HeadlessCmd::List,
        HeadlessCmd::Metadata,
        HeadlessCmd::Thumbnail,
        HeadlessCmd::Render,
    ] {
        let a = Args {
            headless: Some(Headless {
                cmd,
                path: PathBuf::from("a.png"),
                target: None,
            }),
            ..Default::default()
        };
        assert!(!needs_instance_gate(&a), "{cmd:?} 不该被转发给 GUI");
        let old = a.probe.is_none() && a.bench.is_none();
        assert!(old, "负向臂：漏掉分支就会把 {cmd:?} 判进闸门");
    }
    assert!(needs_instance_gate(&Args::default()));
}

/// ⑧ 端到端：真 exe 在**不开窗**的前提下回一行 JSON。发布版是 GUI 子系统，
/// stdout 必须由适配器重定向才能拿到——这条判据把那条前提也一起验了。
#[test]
fn real_binary_answers_one_line_without_a_window() {
    let exe = env!("CARGO_BIN_EXE_rupix");
    let dir = rupix_lib::core::paths::test_scratch_root().join("headless-e2e-empty");
    let _ = std::fs::create_dir_all(&dir);

    let ok = std::process::Command::new(exe)
        .arg("--list")
        .arg(&dir)
        .output()
        .expect("无头 exe 必须能独立启动");
    assert_eq!(ok.status.code(), Some(0));
    let text = String::from_utf8_lossy(&ok.stdout);
    assert_eq!(text.lines().count(), 1, "stdout 只许一行 JSON");
    let v = json::parse(text.trim()).expect("一行合法 JSON");
    assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
    assert_eq!(v.get("count").and_then(|x| x.as_u64()), Some(0));

    let bad = std::process::Command::new(exe)
        .arg("--metadata")
        .arg("definitely-absent-e2e.png")
        .output()
        .expect("失败路径也必须能独立启动");
    assert_eq!(bad.status.code(), Some(3), "失败退出码与 code 字段同值");
    let text = String::from_utf8_lossy(&bad.stdout);
    assert_eq!(text.lines().count(), 1);
    assert_eq!(
        json::parse(text.trim())
            .unwrap()
            .get("code")
            .and_then(|x| x.as_i64()),
        Some(3)
    );
}

/// ⑦ 两个上限必须被真正引用、且拒绝时回退出码 4：光定义不设防等于没设。
#[test]
fn limits_are_declared_and_enforced() {
    assert!(
        HEADLESS_SRC.contains("MAX_LIST_ENTRIES"),
        "清单上限必须落到 list 的检查里"
    );
    assert!(
        HEADLESS_SRC.contains("MAX_PIXELS"),
        "像素上限必须落到出像素的检查里"
    );
    assert!(
        HEADLESS_SRC.contains("EXIT_LIMIT"),
        "超出上限必须回退出码 4，而不是继续出像素"
    );
}
