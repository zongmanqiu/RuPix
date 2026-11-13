//! 无头命令面（m293）：不开窗、结构化输出、一次调用一个短命进程。
//!
//! # 它是什么
//!
//! `--probe-decode` 是诊断探针，输出是人读的一行文本；本模块是给它旁边的**正式命令面**：
//! `list` / `metadata` / `thumbnail` / `render` / `list-formats` 五个纯读动词，
//! 各回**一行 JSON**，供进程外的适配器（MCP 等）转调。适配器自己持有会话与生命周期，
//! RuPix 侧不常驻、不监听端口、不建窗、不进单实例闸门。
//!
//! # 三条不许破的规矩
//!
//! 1. **共用解码链**：像素一律走 [`crate::core::decode::probe_decode`] /
//!    [`crate::core::decode::probe_thumb`]，本模块不装第二套后端、不碰解码器 ABI。
//!    后端由 `main` 在一切分支之前调 `install_decode_stack()` 装好，全进程只有那一处。
//! 2. **stdout 恰好一行 JSON**：本模块只**产出**那一行字符串，打印归 `main` 这条
//!    bin 真入口（与老探针同处；lib 里不许有打印宏，卫生门禁盯着）。日志走
//!    `platform::logging` 的落盘 sink，不进 stdout。适配器按一行读，多一行就是坏契约。
//! 3. **失败也回一行 JSON**：不靠 stderr 判成败，`code` 字段与进程退出码同值。
//!
//! # 为什么不落盘出像素
//!
//! v1 把 PNG 以 base64 内联在 JSON 里：不产生临时文件、不必定谁清理、并发调用互不踩踏，
//! 也不碰 `core::paths` 那条「外部目录一个字节都不碰」的纪律。代价是大图的一行 JSON
//! 会变长，用 [`MAX_PIXELS`] 兜住；真要流式交付像素是阶段 3 的事。
//!
//! ★ m305 订正：上面「不产生临时文件」只覆盖**出像素的三个动词 + 目录输入**。
//! `--list` 吃压缩包时会经 `core::archive::resolve` 落一层到 `<exe>/temp/`，归属见
//! [`cmd_list`] 的注释——这是唯一写盘的一条路，其余三动词仍然零临时文件。

use std::collections::BTreeSet;
use std::path::Path;

use crate::cli::{Headless, HeadlessCmd};
use crate::decoders::Health;

/// 退出码：成功。
pub const EXIT_OK: i32 = 0;
/// 退出码：参数错（动词出现过但没拿到路径）。
pub const EXIT_ARG: i32 = 2;
/// 退出码：读写 / 解码 / 编码失败。
pub const EXIT_FAIL: i32 = 3;
/// 退出码：上限拒绝（清单过长、像素过大）。
pub const EXIT_LIMIT: i32 = 4;

/// 一次 `list` 最多回多少条：stdout 是一行，不能让一个几万项的目录把适配器撑住。
pub const MAX_LIST_ENTRIES: usize = 10_000;
/// 一次出像素最多多少像素（含 alpha 的 RGBA8 与 PNG 都在内存里，先兜一道）。
pub const MAX_PIXELS: u64 = 32_000_000;

/// 执行一条无头请求：返回 `(退出码, 那一行 JSON)`，**不打印**。
///
/// 纯函数是刻意的：`stdout 恰好一行` 这条契约要能被单测直接盯住，而不是靠
/// 「跑一遍 exe 看输出」；打印只发生在 bin 真入口，lib 里零个打印宏。
pub fn run(req: &Headless) -> (i32, String) {
    match execute(req) {
        Ok(fields) => (EXIT_OK, format_response(Some(req.cmd), EXIT_OK, &fields)),
        Err((code, msg)) => (
            code,
            format_response(Some(req.cmd), code, &format!(",\"error\":{}", quote(&msg))),
        ),
    }
}

/// 参数缺路径时用：动词还不知道，回 `cmd:null` + 退出码 2，形状与 `run` 一致。
pub fn arg_error(msg: &str) -> (i32, String) {
    (
        EXIT_ARG,
        format_response(None, EXIT_ARG, &format!(",\"error\":{}", quote(msg))),
    )
}

/// 统一一行 JSON。`fields` 是逗号开头的附加字段串（调用方负责转义与拼装）。
///
/// `code` 恒在：成功 0，失败与退出码同值。把成败判据也放进机器可读的那一行，
/// 而不是让适配器去猜 stderr 里有没有 warn。
pub fn format_response(cmd: Option<HeadlessCmd>, code: i32, fields: &str) -> String {
    let cmd_json = match cmd {
        Some(c) => quote(c.name()),
        None => "null".to_string(),
    };
    format!(
        "{{\"ok\":{},\"cmd\":{cmd_json},\"code\":{code}{fields}}}",
        if code == EXIT_OK { "true" } else { "false" }
    )
}

/// JSON 字符串转义。与 `core::ext_protocol::quote` 同款（那份是私有的，不跨模块复用）。
/// 非 ASCII 原样保留——路径里有中文是常态，转成 `\u` 只会让输出更难读。
pub fn quote(s: &str) -> String {
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

/// 标准 base64（RFC 4648），只在 PNG 内联这一处用，不引新依赖。
pub fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | data[i + 2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(TABLE[(n >> 6) as usize & 63] as char);
        out.push(TABLE[n as usize & 63] as char);
        i += 3;
    }
    match data.len() - i {
        1 => {
            let n = (data[i] as u32) << 16;
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            out.push_str("==");
        }
        2 => {
            let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            out.push(TABLE[(n >> 6) as usize & 63] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// 执行动词，成功回「逗号开头的附加字段串」，失败回「退出码 + 人读原因」。
fn execute(req: &Headless) -> Result<String, (i32, String)> {
    match req.cmd {
        HeadlessCmd::List => cmd_list(&req.path),
        HeadlessCmd::Metadata => cmd_metadata(&req.path),
        HeadlessCmd::Thumbnail => cmd_pixels(&req.path, None, true),
        HeadlessCmd::Render => cmd_pixels(&req.path, req.target, false),
        HeadlessCmd::ListFormats => cmd_list_formats(),
    }
}

/// `list-formats`：这台机器现在认哪些后缀。
///
/// 数据来源与设置页「解码器」三张卡**同源**：
/// - 内置：`core::formats::builtin_exts()`（今天只有 resvg 的 svg/svgz）；
/// - 解码器：`decoders::list_decoders()` 逐个 DLL 读 `iv_decoder_info()`。
///
/// 三条口径：
/// 1. `decoders` **不隐藏坏条目**——`health` / `enabled` 一并回，坏 DLL 是排查线索，
///    不是「这个格式不存在」。
/// 2. `formats` 只并**启用且加载成功**的自述后缀，与 `declared_exts()` 的「实际能开」
///    同一条判据；`count` 是它的长度。
/// 3. 没有解码器目录 / safe-mode 时**仍然成功**，只回内置两条——这不是错误，是新装状态。
///
/// 只读：不写盘、不建窗、不碰单实例闸门。代价与设置页扫一次解码器目录相同（逐个
/// `LoadLibrary` 读自述），只在用户显式调用时发生，不进启动路径。
fn cmd_list_formats() -> Result<String, (i32, String)> {
    let builtin = crate::core::formats::builtin_exts();
    let mut decoders = crate::decoders::list_decoders();
    decoders.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.file.cmp(&b.file)));

    let mut formats: BTreeSet<String> = builtin.iter().cloned().collect();
    for d in &decoders {
        if d.enabled && matches!(d.health, Health::Ok) {
            formats.extend(d.extensions.iter().cloned());
        }
    }
    let formats: Vec<String> = formats.into_iter().collect();
    let active = decoders
        .iter()
        .filter(|d| d.enabled && matches!(d.health, Health::Ok))
        .count();

    let mut fields = String::from(",\"builtin\":[");
    push_json_list(&mut fields, &builtin);
    fields.push_str("],\"decoders\":[");
    for (i, d) in decoders.iter().enumerate() {
        if i > 0 {
            fields.push(',');
        }
        let mut exts = d.extensions.clone();
        exts.sort();
        exts.dedup();
        fields.push_str(&format!(
            "{{\"id\":{},\"name\":{},\"file\":{},\"abi\":{},\"enabled\":{},\"health\":{},\"health_detail\":{},\"extensions\":[",
            quote(&d.id),
            quote(&d.display_name),
            quote(&d.file),
            d.abi
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string()),
            d.enabled,
            quote(health_code(&d.health)),
            quote(&d.health.label()),
        ));
        push_json_list(&mut fields, &exts);
        fields.push_str("]}");
    }
    fields.push_str("],\"formats\":[");
    push_json_list(&mut fields, &formats);
    fields.push_str(&format!(
        "],\"count\":{},\"decoder_count\":{},\"active_decoder_count\":{}",
        formats.len(),
        decoders.len(),
        active
    ));
    Ok(fields)
}

/// 稳定的机器可读健康码（UI 文案会随语言变，JSON 契约不变）。
fn health_code(h: &Health) -> &'static str {
    match h {
        Health::Ok => "ok",
        Health::AbiMismatch { .. } => "abi-mismatch",
        Health::LoadSkipped => "load-skipped",
        Health::LoadFailed(_) => "load-failed",
    }
}

/// 逗号分隔的 JSON 字符串数组内容（调用方负责写 `[` / `]`）。
fn push_json_list(out: &mut String, items: &[String]) {
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&quote(s));
    }
}

/// `list`：走与 UI 翻页**同一份** `core::gallery::listing`，所以「RuPix 认哪些图」
/// 在命令面与主窗里逐项一致；非图片不进清单，不另写一套后缀判定。
///
/// ★ m305：五个动词里**只有 `--list` 认压缩包**——它本来就以「目录」为输入，包经
/// [`crate::core::archive::resolve`] 落成真实目录后，这条腿一个字都不用改。像素类动词
/// 仍只吃本机真实路径，不给契约加「包内定位」第二套坐标系；适配器先 `list` 再把
/// `entries[].path` 交给 `--render`。
///
/// ★ **会话目录与 GUI 共用同一份**（`rupix-zip-<包哈希>/L<层哈希>/`），清理仍只有
/// `archive::cleanup_all` 一处（下一次 GUI 退出时收）。这里**绝不在退出时删**：
/// `entries[].path` 要跨进程活到适配器随后那次 `--render`，删了就是死路径；
/// 另起独立命名空间则会让同一个包落两份、并给「清理只有一处」开第二道口子。
fn cmd_list(dir: &Path) -> Result<String, (i32, String)> {
    let layer = crate::core::archive::resolve(dir)
        .map_err(|e| (EXIT_FAIL, format!("打开压缩包失败：{e}")))?;
    let actual = layer.dir.as_path();
    if !actual.is_dir() {
        return Err((EXIT_FAIL, format!("不是目录：{}", dir.display())));
    }
    let files = crate::core::gallery::listing(actual, crate::core::gallery::SortSpec::DEFAULT)
        .map_err(|e| (EXIT_FAIL, format!("读目录失败：{e:?}")))?;
    if files.len() > MAX_LIST_ENTRIES {
        return Err((
            EXIT_LIMIT,
            format!("清单 {} 条超过上限 {MAX_LIST_ENTRIES}", files.len()),
        ));
    }
    let mut s = format!(
        ",\"dir\":{},\"count\":{},\"entries\":[",
        quote(&actual.display().to_string()),
        files.len()
    );
    for (i, p) in files.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let meta = std::fs::metadata(p).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = unix_secs(meta.as_ref().and_then(|m| m.modified().ok()));
        s.push_str(&format!(
            "{{\"path\":{},\"size\":{size},\"mtime\":{mtime}}}",
            quote(&p.display().to_string())
        ));
    }
    s.push(']');
    // 只对包追加：目录响应必须与 m293 逐字节一致。新增字段无条件追加会破坏
    // 「换来源后目录行为不变」这条判据，也让老适配器多做一次无谓的分支。
    if layer.from_archive {
        s.push_str(&format!(
            ",\"archive\":true,\"skipped\":{},\"truncated\":{}",
            layer.skipped,
            crate::core::archive::tree_truncated(actual),
        ));
    }
    Ok(s)
}

/// `metadata`：**不解码**。文件大小 / 修改时间 / 后缀 + 属性窗那六行 EXIF。
/// 尺寸与帧数在 `render` / `thumbnail` 的回答里——看元数据不该付一次整解的代价。
fn cmd_metadata(path: &Path) -> Result<String, (i32, String)> {
    let m = std::fs::metadata(path).map_err(|e| (EXIT_FAIL, format!("读文件失败：{e}")))?;
    let size = m.len();
    let mtime = unix_secs(m.modified().ok());
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let x = crate::platform::exif::read_exif(path);
    Ok(format!(
        ",\"path\":{},\"size\":{size},\"mtime\":{mtime},\"ext\":{},\"exif\":{{\
         \"capture_time\":{},\"camera\":{},\"lens\":{},\"exposure\":{},\
         \"focal\":{},\"gps\":{},\"orientation\":{}}}",
        quote(&path.display().to_string()),
        quote(&ext),
        quote(&x.capture_time),
        quote(&x.camera),
        quote(&x.lens),
        quote(&x.exposure),
        quote(&x.focal),
        quote(&x.gps),
        x.orientation,
    ))
}

/// 两个出像素的动词共用一条腿：探针解码 → PNG 编码 → base64 内联。
fn cmd_pixels(
    path: &Path,
    target: Option<(u32, u32)>,
    thumb: bool,
) -> Result<String, (i32, String)> {
    let d = if thumb {
        crate::core::decode::probe_thumb(
            path,
            crate::core::thumbs::THUMB_W,
            crate::core::thumbs::THUMB_H,
        )
    } else {
        let (w, h) = target.unwrap_or((0, 0));
        crate::core::decode::probe_decode(path, w, h)
    }
    .map_err(|e| (EXIT_FAIL, format!("解码失败：{e}")))?;

    let pixels = d.bitmap.width as u64 * d.bitmap.height as u64;
    if pixels > MAX_PIXELS {
        return Err((
            EXIT_LIMIT,
            format!("位图 {pixels} 像素超过上限 {MAX_PIXELS}"),
        ));
    }
    let png =
        crate::platform::clipboard::encode_png(d.bitmap.width, d.bitmap.height, &d.bitmap.rgba)
            .map_err(|e| (EXIT_FAIL, format!("PNG 编码失败：{e}")))?;

    Ok(format!(
        ",\"path\":{},\"natural\":[{},{}],\"width\":{},\"height\":{},\
         \"frames\":{},\"bytes\":{},\"mime\":\"image/png\",\"png_b64\":{}",
        quote(&path.display().to_string()),
        d.natural.0,
        d.natural.1,
        d.bitmap.width,
        d.bitmap.height,
        d.anim.count,
        png.len(),
        quote(&base64(&png)),
    ))
}

fn unix_secs(t: Option<std::time::SystemTime>) -> u64 {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一行 JSON 必须能被自家解析器读回去；`code` 与 `ok` 是同一件事的两种读法。
    #[test]
    fn response_is_parseable_one_line_json() {
        let line = format_response(Some(HeadlessCmd::List), EXIT_OK, ",\"count\":0");
        assert_eq!(line.lines().count(), 1, "stdout 只许一行");
        assert!(!line.contains('\n'));
        let v = crate::core::json::parse(&line).expect("自家 JSON 必须能解析");
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
        assert_eq!(v.get("cmd").and_then(|x| x.as_str()), Some("list"));
        assert_eq!(v.get("code").and_then(|x| x.as_i64()), Some(0));
        let bad = format_response(None, EXIT_ARG, ",\"error\":\"缺路径\"");
        let v = crate::core::json::parse(&bad).expect("失败响应也必须是一行合法 JSON");
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(false));
        assert_eq!(v.get("code").and_then(|x| x.as_i64()), Some(2));
    }

    /// 负向臂：若有人把日志拼进这行（多一个换行或前缀），判据当场红。
    #[test]
    fn response_has_no_log_prefix_or_trailing_text() {
        let line = format_response(Some(HeadlessCmd::Render), EXIT_OK, "");
        assert!(line.starts_with("{\"ok\":"), "日志不许排在 JSON 前面");
        assert!(line.ends_with('}'), "JSON 后面不许再挂东西");
    }

    /// `list-formats` 不带路径也必须成功；没有解码器时只回内置 svg/svgz。
    /// 这条同时钉住三件事：JSON 可解析、`count` 与 `formats` 同长、内置恒在。
    #[test]
    fn list_formats_reports_builtin_even_without_decoders() {
        let req = Headless {
            cmd: HeadlessCmd::ListFormats,
            path: std::path::PathBuf::new(),
            target: None,
        };
        let (code, line) = run(&req);
        assert_eq!(code, EXIT_OK, "没有解码器目录不是错误：{line}");
        assert_eq!(line.lines().count(), 1, "stdout 只许一行");
        let v = crate::core::json::parse(&line).expect("自家 JSON 必须能解析");
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
        assert_eq!(
            v.get("cmd").and_then(|x| x.as_str()),
            Some("list-formats"),
            "cmd 是稳定契约，不许随 UI 文案改名"
        );
        let builtin: Vec<&str> = v
            .get("builtin")
            .and_then(|x| x.as_array())
            .expect("builtin 必须是数组")
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        assert_eq!(builtin, vec!["svg", "svgz"], "内置只有 resvg 两条");
        let formats = v
            .get("formats")
            .and_then(|x| x.as_array())
            .expect("formats 必须是数组");
        assert!(
            formats.iter().any(|x| x.as_str() == Some("svg")),
            "内置格式必须进并集"
        );
        assert_eq!(
            v.get("count").and_then(|x| x.as_u64()),
            Some(formats.len() as u64),
            "count 与 formats 必须同长"
        );
        assert!(
            v.get("decoders").and_then(|x| x.as_array()).is_some(),
            "decoders 字段必须在（空目录也要有空数组）"
        );
    }

    /// `quote` 要能扛中文路径、反斜杠、引号和控制字符，且转义后仍可解析。
    #[test]
    fn quote_roundtrips_through_own_parser() {
        let raw = "D:\\照片\\a \"b\".png\n";
        let line = format!("{{\"path\":{}}}", quote(raw));
        let v = crate::core::json::parse(&line).expect("转义后必须合法");
        assert_eq!(v.get("path").and_then(|x| x.as_str()), Some(raw));
    }

    /// base64 三条已知向量（长度 3 的倍数 / 余 1 / 余 2）都要对得上补齐规则。
    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// 负向臂：换掉字母表或漏掉补齐，上面那条向量必须当场红（这里钉住补齐位）。
    #[test]
    fn base64_padding_is_not_optional() {
        assert!(base64(b"f").ends_with("=="));
        assert!(base64(b"fo").ends_with('='));
        assert!(!base64(b"foo").ends_with('='));
    }

    /// `list` 对空目录回 `count:0` 而不是报错；不依赖解码器 DLL。
    #[test]
    fn list_empty_dir_is_ok_not_error() {
        let dir = crate::core::paths::test_scratch_root().join("headless-empty");
        let _ = std::fs::create_dir_all(&dir);
        let req = Headless {
            cmd: HeadlessCmd::List,
            path: dir,
            target: None,
        };
        let fields = cmd_list(&req.path).expect("空目录不该报错");
        let line = format_response(Some(HeadlessCmd::List), EXIT_OK, &fields);
        let v = crate::core::json::parse(&line).expect("合法 JSON");
        assert_eq!(v.get("count").and_then(|x| x.as_u64()), Some(0));
    }

    /// `metadata` 对不存在的文件回 code 3，且原因里不夹换行（否则 stdout 会多一行）。
    #[test]
    fn metadata_missing_file_is_code_3() {
        let req = Headless {
            cmd: HeadlessCmd::Metadata,
            path: std::path::PathBuf::from("definitely-not-here-9f3a.png"),
            target: None,
        };
        let (code, msg) = execute(&req).unwrap_err();
        assert_eq!(code, EXIT_FAIL);
        let line = format_response(Some(req.cmd), code, &format!(",\"error\":{}", quote(&msg)));
        assert_eq!(line.lines().count(), 1);
        assert_eq!(
            crate::core::json::parse(&line)
                .unwrap()
                .get("code")
                .and_then(|x| x.as_i64()),
            Some(3)
        );
    }

    /// m305 负向臂：目录输入的响应不许因为「接了包」多出任何一个字段，
    /// `dir` 也必须是输入目录本身。把新增字段无条件追加、或把 dir 换成层目录，当场红。
    #[test]
    fn list_directory_response_is_unchanged_by_archive_support() {
        let dir = crate::core::paths::test_scratch_root().join("headless-m305-dir");
        let _ = std::fs::create_dir_all(&dir);
        let fields = cmd_list(&dir).expect("目录不该报错");
        assert!(!fields.contains("\"archive\""), "目录响应不许带 archive");
        assert!(!fields.contains("\"skipped\""), "目录响应不许带 skipped");
        assert!(
            !fields.contains("\"truncated\""),
            "目录响应不许带 truncated"
        );
        assert!(
            fields.contains(&format!("\"dir\":{}", quote(&dir.display().to_string()))),
            "dir 必须还是输入目录本身"
        );
        let line = format_response(Some(HeadlessCmd::List), EXIT_OK, &fields);
        assert_eq!(line.lines().count(), 1, "stdout 仍只许一行");
    }

    /// m305 正向臂：包输入经 resolve 落盘，响应带三个溯源字段，`entries[].path`
    /// 是落盘后的真实文件（适配器可直接拿去 `--render`）。
    #[test]
    fn list_archive_reports_provenance_and_real_paths() {
        use std::io::Write;
        let root = crate::core::paths::test_scratch_root();
        let zip_path = root.join("headless-m305.zip");
        let f = std::fs::File::create(&zip_path).expect("造样本包");
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        // 内置认 svg（不依赖运行时装载的解码器 DLL），.png 在 lib 单测里没人认领。
        for name in ["a.svg", "b.svg"] {
            w.start_file(name, opts).expect("条目");
            w.write_all(b"<svg/>").expect("写条目");
        }
        w.finish().expect("收尾");

        let fields = cmd_list(&zip_path).expect("包该被认出来");
        let line = format_response(Some(HeadlessCmd::List), EXIT_OK, &fields);
        let v = crate::core::json::parse(&line).expect("合法 JSON");
        assert_eq!(v.get("archive").and_then(|x| x.as_bool()), Some(true));
        assert_eq!(v.get("count").and_then(|x| x.as_u64()), Some(2));
        assert!(v.get("skipped").is_some(), "跳过数必须有位置");
        assert!(v.get("truncated").is_some(), "树截断标记必须有位置");
        let entries = v
            .get("entries")
            .and_then(|x| x.as_array())
            .expect("entries 数组");
        assert_eq!(entries.len(), 2);
        assert!(
            entries.iter().all(|e| {
                e.get("path")
                    .and_then(|x| x.as_str())
                    .map(Path::new)
                    .is_some_and(|p| p.is_file())
            }),
            "entries[].path 必须是落盘后的真实文件"
        );
    }

    /// m305：无头进程**不许**在退出时删会话目录 —— `entries[].path` 要跨进程
    /// 活到适配器随后那次 `--render`。谁在这里加清理，先看到这条红，
    /// 而不是等适配器报「文件不存在」。
    #[test]
    fn headless_never_cleans_archive_sessions() {
        let all = include_str!("headless.rs");
        let prod = &all[..all.find("\n#[cfg(test)]").unwrap_or(all.len())];
        assert!(!prod.contains("cleanup_all("), "无头不许调全局清理");
        assert!(!prod.contains("remove_dir_all("), "无头不许删包会话目录");
    }
}
