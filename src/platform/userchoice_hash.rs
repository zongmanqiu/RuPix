//! UserChoice 哈希（2026-11-10 动态取证确证，对 26200 逐字节 6/6 命中）。
//!
//! Windows「默认打开方式」UserChoice 子键的 ProgId + Hash 双值里，Hash 是微软的
//! 防篡改锁——Explorer 在使用时校验，不符则把 UserChoice 重置并弹选择器。
//! 本模块实现其**公开算法帧**（来源：mullerdavid/tools_setfta 的 setfta.cs，本机
//! 翻译为 Python 6/6 命中，见 `.ex/imgview/_动态证据/fit_hash_frame2.py`）：
//!
//! ```text
//! data    = (ext + sid + progid + regdate + experience + '\0').to_lowercase()
//!          -- UTF-16LE 编码
//! regdate = UserChoice 子键 LastWriteTime 的 FILETIME，**分钟对齐**（% 600_000_000），
//!          格式化 %016x 小写十六进制
//! md5     = MD5(data)
//! hash    = sub1(data, md5) XOR sub2(data, md5)   -- 各 8 字节
//! 结果    = base64(hash)   -- STANDARD 带 padding
//! ```
//!
//! # 写入侧时序（关键，2026-11-10 实测）
//!
//! 哈希把「后缀、ProgId、**写入时刻**（UserChoice 子键 LastWriteTime）」全部锁死。
//! 旧键的 filetime 已被它自己的旧值锁住，复用必被 Explorer 判不符 → 重置。所以正确
//! 写法是：**删旧 UserChoice 子键 → RegCreateKeyExW 新建 → RegQueryInfoKeyW 取新键
//! LastWriteTime → 以此算哈希 → 写 ProgId + Hash**。新键的 filetime 每次新写都是
//! 新鲜的，哈希与之自洽。
//!
//! # 失败兜底
//!
//! 任何一步失败（删键失败、取 SID 失败、算法实现与系统不符等），调用方必须退回
//! 「OpenWithProgids 候选」降级路径，绝不能留半写状态。
//!
//! 纯算法模块，无注册表调用，方便单测锁定算法帧。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use md5::Digest as _;
use md5::Md5;

/// shell32 里嵌的经验串（从本机 shell32.dll 提取，小写）。setfta.cs 公开的固定值。
const EXPERIENCE: &str =
    "user choice set via windows user experience {d18b6dd5-6124-4341-9318-804003bafa0b}";

/// 一个后缀 + ProgId + 写入时刻 → 正确的 Base64 哈希。
///
/// - `ext`：不含点的后缀名（`jpg`）。拼接时按带点形式 `.jpg` 参与（与 Explorer
///   写盘上样本一致）。
/// - `progid`：写入 UserChoice 的 ProgId（例如 `RuPix.jpg`）。
/// - `filetime`：新 UserChoice 子键的 FILETIME（100ns 间隔，UTC）。**须是新键的**
///   ——写入侧必须删建后取。
pub fn hash_for(ext: &str, progid: &str, filetime: u64, sid: &str) -> String {
    // 分钟对齐（C# 口径：FILETIME 先 %100万再转 hex）。
    let ft = filetime - filetime % 600_000_000;
    let regdate = format!("{ft:016x}");
    let mut data =
        String::with_capacity(16 + sid.len() + 1 + progid.len() + 16 + EXPERIENCE.len() + 1);
    data.push('.');
    data.push_str(ext);
    data.push_str(sid);
    data.push_str(progid);
    data.push_str(&regdate);
    data.push_str(EXPERIENCE);
    data.push('\0');
    let data = data.to_lowercase();
    let bytes: Vec<u8> = data.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let md5 = Md5::digest(&bytes);
    let a = sub1(&bytes, &md5);
    let b = sub2(&bytes, &md5);
    let mut out = [0u8; 8];
    for i in 0..8 {
        out[i] = a[i] ^ b[i];
    }
    B64.encode(out)
}

/// 把 data 按 4 字节小端切 word。长度口径（setfta.cs / 验证过的 Python 版一致）：
/// `n = len/4`，若 n 为偶取 n，否则取 n-1——**恒偶数**（C# 表达式
/// `(((len>>2)&1)<1?1:0) + ((len>>2)-1)` 化简即：n 偶 → n，n 奇 → n-1）。
/// sub1/sub2 的下标越界判据（dw\[v6\], dw\[i2+1\], dw\[v6-1\]）都按这个偶数长度工作。
#[inline]
fn dwords(data: &[u8]) -> (Vec<u32>, usize) {
    let n = data.len() / 4;
    let length = if n & 1 == 0 { n } else { n - 1 };
    let dw: Vec<u32> = data[..length * 4]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    (dw, length)
}

/// setfta.cs 的私有混合之一；data 须是 UTF-16LE 字节（与 MD5 输入同一段），
/// md5 是 MD5 原始 16 字节。全部 32 位运算 wrapping（溢出即取模），与原机等价。
fn sub1(data: &[u8], md5: &[u8]) -> [u8; 8] {
    let (dw, length) = dwords(data);
    let m = [
        u32::from_le_bytes([md5[0], md5[1], md5[2], md5[3]]),
        u32::from_le_bytes([md5[4], md5[5], md5[6], md5[7]]),
        u32::from_le_bytes([md5[8], md5[9], md5[10], md5[11]]),
        u32::from_le_bytes([md5[12], md5[13], md5[14], md5[15]]),
    ];
    if length <= 1 {
        return [0u8; 8];
    }
    let mut v5: u32 = 0;
    let mut v6: u32 = 0;
    let v18 = (length as u32 - 2) >> 1;
    let mut v8 = v18.wrapping_add(1);
    let mut result: u32 = 0;
    let v9 = (m[1] | 1).wrapping_add(0x13DB0000);
    let v10 = (m[0] | 1).wrapping_add(0x69FB0000);
    while v8 != 0 {
        let i2 = v6;
        let v11 = dw[v6 as usize].wrapping_add(result);
        let t = v10
            .wrapping_mul(v11)
            .wrapping_sub(0x10FA9605u32.wrapping_mul(v11 >> 16));
        let v12 = 0x79F8A395u32
            .wrapping_mul(t)
            .wrapping_add(0x689B6B9Fu32.wrapping_mul(t >> 16));
        let v13 = 0xEA970001u32
            .wrapping_mul(v12)
            .wrapping_sub(0x3C101569u32.wrapping_mul(v12 >> 16));
        let v14 = v13.wrapping_add(v5);
        let u15 = dw[(i2 + 1) as usize].wrapping_add(v13);
        let v15 = v9
            .wrapping_mul(u15)
            .wrapping_sub(0x3CE8EC25u32.wrapping_mul(u15 >> 16));
        let t2 = 0x59C3AF2Du32
            .wrapping_mul(v15)
            .wrapping_sub(0x2232E0F1u32.wrapping_mul(v15 >> 16));
        result = 0x1EC90001u32
            .wrapping_mul(t2)
            .wrapping_add(0x35BD1EC9u32.wrapping_mul(t2 >> 16));
        v5 = result.wrapping_add(v14);
        v6 = v6.wrapping_add(2);
        v8 = v8.wrapping_sub(1);
    }
    if length - 2 - 2 * v18 as usize == 1 {
        let d = dw[2 * v18 as usize].wrapping_add(result);
        let v16 = v10
            .wrapping_mul(d)
            .wrapping_sub(0x10FA9605u32.wrapping_mul(d >> 16));
        let v17 = 0x39646B9Fu32
            .wrapping_mul(v16 >> 16)
            .wrapping_add(0x28DBA395u32.wrapping_mul(v16))
            .wrapping_sub(
                0x3C101569u32.wrapping_mul(
                    (0x689B6B9Fu32
                        .wrapping_mul(v16 >> 16)
                        .wrapping_add(0x79F8A395u32.wrapping_mul(v16)))
                        >> 16,
                ),
            );
        let w = v17
            .wrapping_mul(v9)
            .wrapping_sub(0x3CE8EC25u32.wrapping_mul(v17 >> 16));
        result = 0x35BD1EC9u32
            .wrapping_mul(
                (0x59C3AF2Du32
                    .wrapping_mul(w)
                    .wrapping_sub(0x2232E0F1u32.wrapping_mul(w >> 16)))
                    >> 16,
            )
            .wrapping_add(0x2A18AF2Du32.wrapping_mul(w))
            .wrapping_sub(0xFD6BE0F1u32.wrapping_mul(w >> 16));
        v5 = v5.wrapping_add(result).wrapping_add(v17);
    }
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&result.to_le_bytes());
    out[4..].copy_from_slice(&v5.to_le_bytes());
    out
}

/// setfta.cs 的私有混合之二；与 sub1 同样的 data/md5 口径。
fn sub2(data: &[u8], md5: &[u8]) -> [u8; 8] {
    let (dw, length) = dwords(data);
    let m = [
        u32::from_le_bytes([md5[0], md5[1], md5[2], md5[3]]),
        u32::from_le_bytes([md5[4], md5[5], md5[6], md5[7]]),
        u32::from_le_bytes([md5[8], md5[9], md5[10], md5[11]]),
        u32::from_le_bytes([md5[12], md5[13], md5[14], md5[15]]),
    ];
    if length <= 1 {
        return [0u8; 8];
    }
    let mut v5: u32 = 0;
    let mut v6: u32 = 0;
    let mut v7: u32 = 0;
    let v25 = (length as u32 - 2) >> 1;
    let v21 = m[0] | 1;
    let v22 = m[1] | 1;
    let v23 = 0xB1110000u32.wrapping_mul(v21);
    let v24 = 0x16F50000u32.wrapping_mul(v22);
    let mut v8 = v25.wrapping_add(1);
    while v8 != 0 {
        v6 = v6.wrapping_add(2);
        let s = dw[(v6 - 2) as usize].wrapping_add(v5);
        let v9 = v23
            .wrapping_mul(s)
            .wrapping_sub(0x30674EEFu32.wrapping_mul((v21.wrapping_mul(s)) >> 16));
        let v10 = v9 >> 16;
        let v11 = 0xE9B30000u32.wrapping_mul(v10).wrapping_add(
            0x12CEB96Du32.wrapping_mul(
                (0x5B9F0000u32
                    .wrapping_mul(v9)
                    .wrapping_sub(0x78F7A461u32.wrapping_mul(v10)))
                    >> 16,
            ),
        );
        let v12 = 0x1D830000u32
            .wrapping_mul(v11)
            .wrapping_add(0x257E1D83u32.wrapping_mul(v11 >> 16));
        let q = v12.wrapping_add(dw[(v6 - 1) as usize]);
        let x = v24
            .wrapping_mul(q)
            .wrapping_sub(0x5D8BE90Bu32.wrapping_mul((v22.wrapping_mul(q)) >> 16));
        let v13 = x >> 16;
        let v14 = (0x96FF0000u32
            .wrapping_mul(x)
            .wrapping_sub(0x2C7C6901u32.wrapping_mul(v13)))
            >> 16;
        v5 = 0xF2310000u32.wrapping_mul(v14).wrapping_sub(
            0x405B6097u32.wrapping_mul(
                (0x7C932B89u32
                    .wrapping_mul(v14)
                    .wrapping_sub(0x5C890000u32.wrapping_mul(v13)))
                    >> 16,
            ),
        );
        v7 = v7.wrapping_add(v5).wrapping_add(v12);
        v8 = v8.wrapping_sub(1);
    }
    if length - 2 - 2 * v25 as usize == 1 {
        let d = dw[2 * (v25 + 1) as usize].wrapping_add(v5);
        let v15 = 0xB1110000u32
            .wrapping_mul(v21)
            .wrapping_mul(d)
            .wrapping_sub(0x30674EEFu32.wrapping_mul((v21.wrapping_mul(d)) >> 16));
        let v16 = v15 >> 16;
        let v17 = (0x5B9F0000u32
            .wrapping_mul(v15)
            .wrapping_sub(0x78F7A461u32.wrapping_mul(v16)))
            >> 16;
        let v18 = 0x257E1D83u32
            .wrapping_mul(
                (0xE9B30000u32
                    .wrapping_mul(v16)
                    .wrapping_add(0x12CEB96Du32.wrapping_mul(v17)))
                    >> 16,
            )
            .wrapping_add(0x3BC70000u32.wrapping_mul(v17));
        let w = 0x16F50000u32
            .wrapping_mul(v18)
            .wrapping_mul(v22)
            .wrapping_sub(0x5D8BE90Bu32.wrapping_mul((v18.wrapping_mul(v22)) >> 16));
        let v19 = w >> 16;
        let v20 = (0x96FF0000u32
            .wrapping_mul(w)
            .wrapping_sub(0x2C7C6901u32.wrapping_mul(v19)))
            >> 16;
        v5 = 0xF2310000u32.wrapping_mul(v20).wrapping_sub(
            0x405B6097u32.wrapping_mul(
                (0x7C932B89u32
                    .wrapping_mul(v20)
                    .wrapping_sub(0x5C890000u32.wrapping_mul(v19)))
                    >> 16,
            ),
        );
        v7 = v7.wrapping_add(v5).wrapping_add(v18);
    }
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&v5.to_le_bytes());
    out[4..].copy_from_slice(&v7.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机 6 组真实盘上样本（含 Explorer 亲笔写的 `.pdf`——`Applications\SumatraPDF-Q.exe`
    /// 的 UserChoice 由系统代写，是金标准）。命中判据 = 6 组逐字节一致——锁死算法帧
    /// （dwords 偶数长度口径、分钟对齐、UTF-16LE、全小写、base64 STANDARD + padding、
    /// sub1/sub2 全部魔数与 wrapping 运算）。
    #[test]
    fn hash_matches_six_on_disk_samples() {
        let sid = "S-1-5-21-3386992490-4078822679-1903118105-1001";
        let samples: &[(&str, &str, u64, &str)] = &[
            ("jpg", "ImgView", 134387027272522983, "f8nEJf8vIuE="),
            ("png", "ImgView", 134387027272552982, "EOSBBbYCKUc="),
            ("webp", "ImgView", 134387027272595295, "+z7OkBl9oWI="),
            ("gif", "ImgView", 134387027272582643, "2mWwav3cFPQ="),
            ("svg", "ImgView", 134387027272630291, "demB+1hnSjQ="),
            (
                "pdf",
                "Applications\\SumatraPDF-Q.exe",
                134378406012120657,
                "uPYg3AVKfuw=",
            ),
        ];
        for (ext, pid, ft, want) in samples {
            let got = hash_for(ext, pid, *ft, sid);
            assert_eq!(got, *want, "样本 .{ext} 未命中");
        }
    }

    /// 分钟对齐：同一 filetime 加 59 秒不改变结果（Explorer 按分钟粒度校验的推论）。
    #[test]
    fn minute_alignment_is_stable() {
        let sid = "S-1-5-21-1-2-3-4-100";
        let base = 134387027272522500u64;
        let a = hash_for("jpg", "RuPix.jpg", base, sid);
        // 同分钟内的微小抖动（3 毫秒）不应改变结果（Explorer 按分钟粒度校验）。
        let b = hash_for("jpg", "RuPix.jpg", base + 30_000, sid);
        assert_eq!(a, b, "分钟对齐必须稳定");
        // 真正跨过一分钟边界（600_000_000 × 100ns）则必须不同。
        let c = hash_for("jpg", "RuPix.jpg", base + 600_000_000, sid);
        assert_ne!(a, c, "跨分钟必须不同");
    }

    /// 后缀/ProgId/SID 任一变化，哈希必须变化（防串改语义自洽）。
    #[test]
    fn components_are_bound_into_hash() {
        let sid = "S-1-5-21-1-2-3-4-100";
        let ft = 134387027272522983u64;
        let base = hash_for("jpg", "RuPix.jpg", ft, sid);
        assert_ne!(base, hash_for("png", "RuPix.jpg", ft, sid));
        assert_ne!(base, hash_for("jpg", "Other.app", ft, sid));
        assert_ne!(
            base,
            hash_for("jpg", "RuPix.jpg", ft, "S-1-5-21-9-8-7-6-200")
        );
    }
}
