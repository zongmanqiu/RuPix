//! 极简 base64 解码（宿主只解、不编）。
//!
//! 用途只有一条：条码插件把码里携带的图片字节按 base64 塞进响应，宿主解出来落 temp、
//! 再交系统默认程序打开。二维码的二进制载荷上限约 3 KB（v40），base64 后 4 KB 量级——
//! 不值得为它引一个依赖，也不值得为它走共享内存。
//!
//! 接受标准字母表、忽略空白与换行；`=` 只在末尾；其余字符一律报错（不猜）。

/// 解一段 base64。空串合法（返回空表）。
pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut pad = 0usize;
    for c in text.chars() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == '=' {
            pad += 1;
            continue;
        }
        if pad > 0 {
            return Err("base64：`=` 之后还有内容".into());
        }
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            other => return Err(format!("base64：非法字符 {other:?}")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    if pad > 2 {
        return Err("base64：填充过长".into());
    }
    if bits >= 6 {
        return Err("base64：末尾位不足一个字节".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_common_shapes() {
        assert_eq!(decode("").expect("empty"), Vec::<u8>::new());
        assert_eq!(decode("aGVsbG8=").expect("hello"), b"hello");
        assert_eq!(decode("aGVsbG8h").expect("hello!"), b"hello!");
        // 空白与换行允许（JSON 里常被折行）。
        assert_eq!(decode("aGVs\nbG8=").expect("wrapped"), b"hello");
    }

    #[test]
    fn rejects_junk_instead_of_guessing() {
        assert!(decode("a$b").is_err());
        assert!(decode("ab=c").is_err(), "`=` 之后还有内容");
        assert!(decode("a").is_err(), "六个位都不够凑一个字节");
        assert!(decode("====").is_err(), "填充过长");
    }

    #[test]
    fn round_trips_a_png_magic() {
        // 手算的 PNG 头 8 字节。
        assert_eq!(
            decode("iVBORw0KGgo=").expect("png magic"),
            b"\x89PNG\r\n\x1a\n"
        );
    }
}
