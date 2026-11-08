//! 极简 JSON 解析：只为读解码器清单（仓库的 `decoders.json`）。
//!
//! # 为什么不加 serde_json
//!
//! 宿主是 5 MB 级的便携看图器，为一份 1.8 KB 的清单引入 `serde` + `serde_derive`
//! （编译期负担 + 可观代码量）不成比例；而这份清单的形状由**我们自己的仓库**生成，
//! 叶子只有「字符串 / 非负整数 / 数组包对象」三种。
//!
//! # 但它不是"能跑就行"
//!
//! 清单来自网络，解析器必须能扛住坏输入而不是陪着一起崩：
//! - **深度上限** `MAX_DEPTH`：解析是递归的，一份 10 万个 `[` 的响应能把栈打穿
//!   （这类崩溃在用户侧表现为"看图器打开设置就没了"，比报错难查一百倍）。
//! - **字符串整体校验 UTF-8**：按字节扫、非 ASCII 原样收集，最后一次性 `from_utf8`。
//!   若按字节切片再 `from_str`，多字节字符会被切成非法序列并 panic。
//! - **代理对合并**：`\ud83d\ude00` 要拼成一个字符；落单的代理项 `Err`——Rust 的 `char`
//!   装不下半个代理对，静默丢弃会让说明文字少一半，而这种缺失没有人会发现。
//! - **数字超出 `u64` 报错**，不静默截断：`size` 是要拿去比对下载字节数的。
//!
//! 不做的事（JSON 规范之外）：`NaN` / `Infinity` 字面量、尾随逗号、注释。生成器若写出
//! 这些，说明它坏了——**报错比宽容有用**，宽容只会把问题推到"清单明明更新了却读不到"。
use std::fmt;

/// 嵌套深度上限。清单的形状是 `[ { "formats": ... } ]`，两层就够；给到 32 是留给
/// 将来"依赖关系"这类嵌套的余量，同时把栈深度钉死在安全区（每层约 100 B 栈）。
pub const MAX_DEPTH: usize = 32;

/// 解析出来的一棵 JSON 值。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    /// 对象按**键值对序列**存（不是 `HashMap`）：清单里的字段顺序是契约的一部分，
    /// 保留顺序才能让"清单结构变了"这类 diff 一眼看出来；同名键取第一个（与浏览器一致）。
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// 取对象字段；非对象或没有该键返回 `None`。
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// 整数（可负）。小数 / 非有限值一律 `None`；选区坐标允许负值，故不套 `as_u64`。
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Num(n)
                if n.is_finite()
                    && n.fract() == 0.0
                    && *n >= -9.007_199_254_740_992e15
                    && *n <= 9.007_199_254_740_992e15 =>
            {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    /// 非负整数。**小数或负数一律 `None`**：调用方要的是一个能拿去当字节数的值，
    /// 把 `1e9` 或 `-1` 悄悄转成 `u64` 只会把错误藏起来。
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Num(n)
                if n.is_finite()
                    && *n >= 0.0
                    && n.fract() == 0.0
                    && *n <= 9.007_199_254_740_992e15 =>
            {
                Some(*n as u64)
            }
            _ => None,
        }
    }
}

/// 解析错误：带**字节偏移**的位置信息。
///
/// 网络来的清单不会有人肉读它的机会，报错文案是要贴进日志、贴进界面的，
/// "第 137 字节：期望 ','"比"JSON 解析失败"能省掉一整轮猜测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub at: usize,
    pub what: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "清单第 {} 字节：{}", self.at, self.what)
    }
}

impl std::error::Error for Error {}

/// 解析一份完整文档。允许首尾空白，**不允许**尾随垃圾（多出来的字节一定是拼接坏了）。
pub fn parse(text: &str) -> Result<Value, Error> {
    let mut p = Parser {
        b: text.as_bytes(),
        i: 0,
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("文档结束后还有多余内容"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, what: &str) -> Error {
        Error {
            at: self.i,
            what: what.to_string(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(self.err(&format!("嵌套超过 {MAX_DEPTH} 层")));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::Str),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => {
                Err(self
                    .err("这里应该是一个值（对象 / 数组 / 字符串 / 数字 / true / false / null）"))
            }
            None => Err(self.err("文档意外结束")),
        }
    }

    fn literal(&mut self, lit: &str, v: Value) -> Result<Value, Error> {
        if self.b[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(v)
        } else {
            Err(self.err("不是合法的字面量"))
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Error> {
        self.i += 1; // '{'
        let mut kv = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Obj(kv));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("键必须是双引号字符串"));
            }
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("键之后缺少 ':'"));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            kv.push((k, v));
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(kv));
                }
                _ => return Err(self.err("对象里缺少 ',' 或 '}'（尾随逗号也不接受）")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Error> {
        self.i += 1; // '['
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err(self.err("数组里缺少 ',' 或 ']'（尾随逗号也不接受）")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c == b'-' || c == b'+' || c == b'.' || c == b'e' || c == b'E' || c.is_ascii_digit())
        {
            self.i += 1;
        }
        let s = std::str::from_utf8(&self.b[start..self.i])
            .map_err(|_| self.err("数字里混进了非 ASCII 字节"))?;
        // `f64::from_str` 会接受 `inf` / `NaN`：这两个词在这里等于"生成器写坏了"。
        let n: f64 = s.parse().map_err(|_| {
            self.i = start;
            self.err("不是合法的数字")
        })?;
        if !n.is_finite() {
            self.i = start;
            return Err(self.err("数字不是有限值"));
        }
        Ok(Value::Num(n))
    }

    /// 字符串：按字节扫，非 ASCII 原样收集，最后整体校验 UTF-8。
    fn string(&mut self) -> Result<String, Error> {
        let open = self.i;
        self.i += 1; // '"'
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                None => {
                    self.i = open;
                    return Err(self.err("字符串没有收尾的双引号"));
                }
                Some(b'"') => {
                    self.i += 1;
                    return String::from_utf8(out).map_err(|_| {
                        self.i = open;
                        self.err("字符串不是合法的 UTF-8")
                    });
                }
                Some(b'\\') => {
                    self.i += 1;
                    let Some(e) = self.peek() else {
                        self.i = open;
                        return Err(self.err("转义符后面没有内容"));
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            // 高代理项必须紧跟一个低代理项，否则这个字符根本装不进 char。
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if self.b.get(self.i..self.i + 2) != Some(&b"\\u"[..]) {
                                    return Err(self.err("落单的 UTF-16 高代理项"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("高代理项后面不是低代理项"));
                                }
                                0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("落单的 UTF-16 低代理项"));
                            } else {
                                hi
                            };
                            let Some(ch) = char::from_u32(code) else {
                                return Err(self.err("\\u 转义不是合法字符"));
                            };
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => {
                            self.i -= 1;
                            return Err(self.err("不认识的转义字符"));
                        }
                    }
                }
                Some(c) if c < 0x20 => {
                    return Err(self.err("字符串里有未转义的控制字符"));
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        if self.i + 4 > self.b.len() {
            return Err(self.err("\\u 后面不足 4 位十六进制"));
        }
        let s = std::str::from_utf8(&self.b[self.i..self.i + 4])
            .map_err(|_| self.err("\\u 后面不是 ASCII"))?;
        let v = u32::from_str_radix(s, 16).map_err(|_| self.err("\\u 后面不是 4 位十六进制"))?;
        self.i += 4;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_real_catalogue_shape() {
        // 与仓库里 decoders.json 的真实形状一致：数组包对象、中文、带 `;` 的格式串。
        let src = r#"[{"id":"common","name":"Common Raster Set","abi":5,"size":1967104,
            "released":"2026-10-26","desc":"常见位图合集","formats":"jpg;jpeg","url":""}]"#;
        let v = parse(src).expect("应当解析成功");
        let arr = v.as_array().expect("顶层是数组");
        assert_eq!(arr.len(), 1);
        let e = &arr[0];
        assert_eq!(e.get("id").and_then(Value::as_str), Some("common"));
        assert_eq!(e.get("abi").and_then(Value::as_u64), Some(5));
        assert_eq!(e.get("size").and_then(Value::as_u64), Some(1_967_104));
        // 非 ASCII 原样进出：这条挂了就说明按字节切片切坏了 UTF-8。
        assert_eq!(e.get("desc").and_then(Value::as_str), Some("常见位图合集"));
        assert_eq!(e.get("url").and_then(Value::as_str), Some(""));
    }

    #[test]
    fn escapes_and_surrogate_pairs() {
        let v = parse(r#""a\"b\\c\/d\n\u4e2d\ud83d\ude00""#).expect("转义");
        assert_eq!(v.as_str(), Some("a\"b\\c/d\n中😀"));
    }

    #[test]
    fn rejects_what_a_broken_generator_would_emit() {
        // 尾随逗号
        assert!(parse("[1,]").is_err());
        assert!(parse(r#"{"a":1,}"#).is_err());
        // 文档后有垃圾
        assert!(parse("[] x").is_err());
        // 落单代理对
        assert!(parse(r#""\ud83d""#).is_err());
        assert!(parse(r#""\ude00""#).is_err());
        // 截断
        assert!(parse("[1,2").is_err());
        assert!(parse(r#"{"a""#).is_err());
        // 无限值
        assert!(parse("1e999").is_err());
    }

    #[test]
    fn depth_is_bounded() {
        let deep = "[".repeat(MAX_DEPTH + 2) + &"]".repeat(MAX_DEPTH + 2);
        let e = parse(&deep).expect_err("超深必须报错，不能把栈打穿");
        assert!(e.what.contains("嵌套"), "{e}");
    }

    #[test]
    fn u64_conversion_is_strict() {
        assert_eq!(Value::Num(4.0).as_u64(), Some(4));
        assert_eq!(Value::Num(1.5).as_u64(), None, "小数不能当字节数");
        assert_eq!(Value::Num(-1.0).as_u64(), None, "负数不能当字节数");
        assert_eq!(Value::Num(1e20).as_u64(), None, "超出精度区间不能硬转");
    }
}
