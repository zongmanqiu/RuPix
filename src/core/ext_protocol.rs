//! 扩展进程的请求/响应 JSON 契约（协议 v1）。
//!
//! 传输层（命名管道）只负责帧边界；本模块负责字段语义与序列化，单测可无头跑。
//! 图像不塞进 JSON：`ImageRef` 只带共享内存名与尺寸，像素在 `platform::shared_mem`。
//!
//! 请求示例：
//! ```json
//! {"protocol":1,"id":"com.example.ocr","action":"run","image_path":"C:\\a.png",
//!  "selection":{"x":0,"y":0,"w":100,"h":50},"args":["--lang=chi_sim"]}
//! ```
//! 响应示例：
//! ```json
//! {"protocol":1,"ok":true,"text":"识别结果"}
//! ```

use crate::core::ext::{protocol_ok, protocol_range_text, PROTOCOL_VERSION};
use crate::core::json::{self, Value};

/// 共享内存图像描述。`name` 由宿主生成，扩展只读打开。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

impl ImageRef {
    pub fn new(
        name: impl Into<String>,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<Self, String> {
        let name = name.into();
        if name.is_empty() {
            return Err("图像共享内存名不能为空".into());
        }
        if width == 0 || height == 0 {
            return Err("图像尺寸不能为 0".into());
        }
        if stride < width.saturating_mul(4) {
            return Err("stride 小于 width*4".into());
        }
        Ok(Self {
            name,
            width,
            height,
            stride,
        })
    }
}

/// 画布选区（原图像素坐标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// 插件私有设置区（一组字符串键值）。
///
/// 为什么是**扁平字符串映射**而不是任意 JSON：设置区是"插件自己的一点点偏好"
/// （语言、模型名、开关），宿主只负责存与取，不该替插件解释结构。插件要复杂结构，
/// 自己编码成一个 JSON 字符串塞进来即可——**宿主当 dumb store**，这样宿主这一侧
/// 永远不会因为插件改了设置结构而解析失败。
pub type Settings = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 协商后的协议版本 = 插件 manifest 声明的那个版本（宿主在 `MIN..=MAX` 内接受）。
    /// 对旧插件必须回它自己的版本，不能硬塞宿主的最新版。
    pub protocol: u32,
    pub id: String,
    pub action: String,
    pub image_path: Option<String>,
    pub image: Option<ImageRef>,
    /// `image` 这份像素的原图尺寸（宽, 高）。
    ///
    /// ★ 为什么必须给：`image.width×height` **不等于**原图——它可能是抽样后的
    /// （`sample_step > 1`），也可能只是当前解码档（视口档 561×561 对一张
    /// 4080×4080 的图）。插件拿不到这一对的后果已经发生过：报告写"全量、未抽稀，
    /// 共 314,721 像素"，而原图有 1,664 万。**只有原图尺寸在请求里，报告才可能诚实。**
    pub source: Option<(u32, u32)>,
    /// 抽样步长：`1` = 原样送出（未抽样）；`N` = 每 N 像素取 1 点。
    ///
    /// 与 `source` 一起就能算出覆盖率，不必让插件知道宿主为什么抽样。
    pub sample_step: u32,
    pub selection: Option<Selection>,
    pub args: Vec<String>,
    /// 宿主要发给插件的设置（插件声明 `settings.read` 时才有）。
    pub settings: Option<Settings>,
}

impl Request {
    /// `protocol` = **这次调用的协商版本**（调用方传插件 manifest 声明的版本）。
    ///
    /// ★ 故意不默认成宿主最新版：那会让"忘了覆盖"的调用点把新版本塞给旧插件。
    ///   参数必填，编译器替我们守这条。
    pub fn new(id: impl Into<String>, action: impl Into<String>, protocol: u32) -> Self {
        Self {
            protocol,
            id: id.into(),
            action: action.into(),
            image_path: None,
            image: None,
            source: None,
            sample_step: 1,
            selection: None,
            args: Vec::new(),
            settings: None,
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = format!(
            "{{\"protocol\":{},\"id\":{},\"action\":{}",
            self.protocol,
            quote(&self.id),
            quote(&self.action)
        );
        if let Some(path) = &self.image_path {
            s.push_str(&format!(",\"image_path\":{}", quote(path)));
        }
        if let Some(img) = &self.image {
            s.push_str(&format!(",\"image\":{}", img.to_json()));
        }
        // 这两个字段只在真的下发了像素时才有意义：没有 `image` 却说"原图多大"，
        // 是在暗示插件"你可以自己按路径去读"，而那正是能力最小化要挡的。
        if self.image.is_some() {
            if let Some((w, h)) = self.source {
                s.push_str(&format!(",\"source\":{{\"width\":{w},\"height\":{h}}}"));
            }
            if self.sample_step > 1 {
                s.push_str(&format!(",\"sample_step\":{}", self.sample_step));
            }
        }
        if let Some(sel) = &self.selection {
            s.push_str(&format!(
                ",\"selection\":{{\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}",
                sel.x, sel.y, sel.w, sel.h
            ));
        }
        if !self.args.is_empty() {
            let args = self
                .args
                .iter()
                .map(|a| quote(a))
                .collect::<Vec<_>>()
                .join(",");
            s.push_str(&format!(",\"args\":[{args}]"));
        }
        if let Some(st) = &self.settings {
            s.push_str(&format!(",\"settings\":{}", settings_to_json(st)));
        }
        s.push('}');
        s
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let root = json::parse(text).map_err(|e| format!("JSON 解析失败：{e}"))?;
        let protocol = get_u32(&root, "protocol")?;
        if !protocol_ok(protocol) {
            return Err(format!(
                "protocol={protocol}，宿主支持 {}",
                protocol_range_text()
            ));
        }
        let id = get_str(&root, "id")?;
        let action = get_str(&root, "action")?;
        let image_path = optional_string(&root, "image_path");
        let image = match root.get("image") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_image(v)?),
        };
        let selection = match root.get("selection") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_selection(v)?),
        };
        let source = match root.get("source") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let w = get_u32(v, "width")?;
                let h = get_u32(v, "height")?;
                Some((w, h))
            }
        };
        let sample_step = match root.get("sample_step") {
            None | Some(Value::Null) => 1,
            Some(v) => v.as_i64().filter(|n| *n >= 1).unwrap_or(1) as u32,
        };
        let args = match root.get("args") {
            None | Some(Value::Null) => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or("args 必须是字符串数组")?
                .iter()
                .map(|x| x.as_str().map(str::to_string).ok_or("args 只能放字符串"))
                .collect::<Result<Vec<_>, _>>()?,
        };
        let settings = match root.get("settings") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_settings(v)?),
        };
        Ok(Self {
            protocol,
            id,
            action,
            image_path,
            image,
            source,
            sample_step,
            selection,
            args,
            settings,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub protocol: u32,
    pub ok: bool,
    pub text: Option<String>,
    pub image: Option<ImageRef>,
    pub error: Option<String>,
    /// 插件要写回宿主的设置（插件声明 `settings.write` 时才会被落盘）。
    pub settings: Option<Settings>,
    /// 要交给系统打开的网址（插件声明 `open.url` 时才被采纳）。
    pub open_url: Option<String>,
    /// 要交给系统打开的图片：`(声明扩展名, base64 载荷)`（声明 `open.file` 时才被采纳）。
    pub open_image: Option<(String, String)>,
}

impl Response {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            ok: true,
            text: Some(text.into()),
            image: None,
            error: None,
            settings: None,
            open_url: None,
            open_image: None,
        }
    }

    pub fn edited(image: ImageRef) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            ok: true,
            text: None,
            image: Some(image),
            error: None,
            settings: None,
            open_url: None,
            open_image: None,
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            ok: false,
            text: None,
            image: None,
            error: Some(message.into()),
            settings: None,
            open_url: None,
            open_image: None,
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = format!(
            "{{\"protocol\":{},\"ok\":{}",
            self.protocol,
            if self.ok { "true" } else { "false" }
        );
        if let Some(text) = &self.text {
            s.push_str(&format!(",\"text\":{}", quote(text)));
        }
        if let Some(img) = &self.image {
            s.push_str(&format!(",\"image\":{}", img.to_json()));
        }
        if let Some(err) = &self.error {
            s.push_str(&format!(",\"error\":{}", quote(err)));
        }
        if let Some(st) = &self.settings {
            s.push_str(&format!(",\"settings\":{}", settings_to_json(st)));
        }
        s.push('}');
        s
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let root = json::parse(text).map_err(|e| format!("JSON 解析失败：{e}"))?;
        let protocol = get_u32(&root, "protocol")?;
        if !protocol_ok(protocol) {
            return Err(format!(
                "protocol={protocol}，宿主支持 {}",
                protocol_range_text()
            ));
        }
        let ok = root
            .get("ok")
            .and_then(Value::as_bool)
            .ok_or("缺少布尔字段 `ok`")?;
        let text = optional_string(&root, "text");
        let error = optional_string(&root, "error");
        let image = match root.get("image") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_image(v)?),
        };
        let settings = match root.get("settings") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_settings(v)?),
        };
        let open_url = optional_string(&root, "open_url");
        // `open_image`: {"ext":"PNG","base64":"..."}。**只收不验内容**——字节的合法性
        // 由 `platform::shell::save_temp_image` 按文件头复核（这里验也没用，那边还要再验）。
        let open_image = match root.get("open_image") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let ext = get_str(v, "ext")?;
                let data = get_str(v, "base64")?;
                Some((ext, data))
            }
        };
        if !ok && error.is_none() {
            return Err("ok=false 但缺少 error".into());
        }
        Ok(Self {
            protocol,
            ok,
            text,
            image,
            error,
            settings,
            open_url,
            open_image,
        })
    }
}

impl ImageRef {
    fn to_json(&self) -> String {
        format!(
            "{{\"name\":{},\"width\":{},\"height\":{},\"stride\":{}}}",
            quote(&self.name),
            self.width,
            self.height,
            self.stride
        )
    }
}

fn parse_image(v: &Value) -> Result<ImageRef, String> {
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .ok_or("image.name 缺失")?;
    let width = field_u32(v, "width")?;
    let height = field_u32(v, "height")?;
    let stride = field_u32(v, "stride")?;
    ImageRef::new(name, width, height, stride)
}

fn parse_selection(v: &Value) -> Result<Selection, String> {
    Ok(Selection {
        x: field_i32(v, "x")?,
        y: field_i32(v, "y")?,
        w: field_u32(v, "w")?,
        h: field_u32(v, "h")?,
    })
}

/// 设置区序列化成 JSON 对象（顺序保留，便于人眼比对与测试断言）。
fn settings_to_json(st: &Settings) -> String {
    let body = st
        .iter()
        .map(|(k, v)| format!("{}:{}", quote(k), quote(v)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

/// 解析设置区对象。值只接受**标量**（字符串/数字/布尔），统一转成字符串；
/// 对象与数组一律拒绝——宿主当 dumb store，不替插件解释嵌套结构。
fn parse_settings(v: &Value) -> Result<Settings, String> {
    let Value::Obj(kv) = v else {
        return Err("settings 必须是对象（键 → 字符串值）".into());
    };
    let mut out = Settings::new();
    for (k, val) in kv {
        let s = match val {
            Value::Str(s) => s.clone(),
            Value::Num(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return Err(format!("settings.{k} 的值只能是字符串/数字/布尔")),
        };
        out.push((k.clone(), s));
    }
    Ok(out)
}

fn get_str(root: &Value, key: &str) -> Result<String, String> {
    root.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("缺少字符串字段 `{key}`"))
}

fn optional_string(root: &Value, key: &str) -> Option<String> {
    root.get(key).and_then(Value::as_str).map(str::to_string)
}

fn get_u32(root: &Value, key: &str) -> Result<u32, String> {
    field_u32(root, key)
}

fn field_u32(root: &Value, key: &str) -> Result<u32, String> {
    let n = root
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("缺少非负整数字段 `{key}`"))?;
    u32::try_from(n).map_err(|_| format!("`{key}` 超出 u32"))
}

fn field_i32(root: &Value, key: &str) -> Result<i32, String> {
    let n = root
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("缺少整数字段 `{key}`"))?;
    i32::try_from(n).map_err(|_| format!("`{key}` 超出 i32"))
}

/// 最小 JSON 字符串转义：引号、反斜杠、控制字符。UTF-8 中文原样保留。
fn quote(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_with_path_and_selection() {
        let mut req = Request::new("com.example.ocr", "run", PROTOCOL_VERSION);
        req.image_path = Some(r"C:\图片\a b.png".into());
        req.selection = Some(Selection {
            x: -2,
            y: 3,
            w: 100,
            h: 50,
        });
        req.args = vec!["--lang=chi_sim".into(), r#"a"b"#.into()];
        let text = req.to_json();
        assert!(text.contains("\\\"b"));
        assert_eq!(Request::parse(&text).expect("parse"), req);
    }

    #[test]
    fn response_roundtrip_text_and_edit() {
        let r = Response::text("第一行\n第二行");
        assert_eq!(Response::parse(&r.to_json()).expect("parse"), r);

        let img = ImageRef::new("Local\\rupix-ext-test", 4, 3, 16).expect("img");
        let r = Response::edited(img);
        assert_eq!(Response::parse(&r.to_json()).expect("parse"), r);
    }

    #[test]
    fn rejects_bad_protocol_and_missing_error() {
        let bad = r#"{"protocol":2,"id":"x","action":"run"}"#;
        assert!(Request::parse(bad).is_err());

        let bad = r#"{"protocol":1,"ok":false}"#;
        assert!(Response::parse(bad).is_err());
    }

    #[test]
    fn image_ref_rejects_short_stride() {
        assert!(ImageRef::new("x", 4, 2, 15).is_err());
        assert!(ImageRef::new("", 4, 2, 16).is_err());
        assert!(ImageRef::new("x", 4, 2, 16).is_ok());
    }

    /// 设置区往返：请求带它、响应带它，解析回来必须一字不差。
    #[test]
    fn settings_roundtrip_in_request_and_response() {
        let mut req = Request::new("com.example.ocr", "run", PROTOCOL_VERSION);
        req.settings = Some(vec![
            ("lang".to_string(), "chi_sim".to_string()),
            ("scale".to_string(), "2".to_string()),
        ]);
        let text = req.to_json();
        assert!(text.contains("\"settings\":{"));
        assert_eq!(Request::parse(&text).expect("parse"), req);

        let mut r = Response::text("ok");
        r.settings = Some(vec![("lang".to_string(), "en-US".to_string())]);
        let text = r.to_json();
        assert_eq!(Response::parse(&text).expect("parse"), r);
    }

    /// 老插件不认识 settings 字段 —— 不带这个字段必须照旧能解析（协议向后兼容）。
    #[test]
    fn missing_settings_field_is_fine() {
        let req = Request::parse(r#"{"protocol":1,"id":"x","action":"run"}"#).expect("parse");
        assert!(req.settings.is_none());
        let r = Response::parse(r#"{"protocol":1,"ok":true,"text":"x"}"#).expect("parse");
        assert!(r.settings.is_none());
    }

    /// 设置区的值只接受标量：嵌套结构一律拒绝（宿主是 dumb store）。
    #[test]
    fn settings_values_must_be_scalar() {
        let bad = r#"{"protocol":1,"id":"x","action":"run","settings":{"a":{"b":1}}}"#;
        assert!(Request::parse(bad).is_err());
        // 数字/布尔按字面量转成字符串（插件写起来省事）。
        let ok = r#"{"protocol":1,"id":"x","action":"run","settings":{"n":3,"b":true}}"#;
        let req = Request::parse(ok).expect("parse");
        let st = req.settings.expect("settings");
        assert_eq!(st.len(), 2);
        assert!(st.iter().any(|(k, v)| k == "n" && v == "3"));
        assert!(st.iter().any(|(k, v)| k == "b" && v == "true"));
    }
}
