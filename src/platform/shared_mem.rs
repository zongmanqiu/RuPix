//! 扩展图像共享内存：宿主分配、扩展只读打开，避免跨进程复制大图。
//!
//! 布局固定 32 字节头 + 像素：
//! ```text
//! 0  magic[8] = b"RPXIMG01"
//! 8  width  u32 LE
//! 12 height u32 LE
//! 16 stride u32 LE（字节，>= width*4）
//! 20 len    u32 LE（= stride*height）
//! 24 reserved[8]（置 0，留给后续格式/颜色空间）
//! 32 pixels（RGBA8，行优先）
//! ```
//! 宿主只创建可读写映射；扩展用 `open` 只读打开。名字由宿主生成并放进请求，
//! 扩展不得自行猜测/枚举其他扩展的映射。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
    FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// 共享内存头长度。改动布局必须同时升 magic。
pub const HEADER_LEN: usize = 32;
const MAGIC: &[u8; 8] = b"RPXIMG01";

/// 一份已映射的 RGBA 图像。
///
/// `view` 的生命周期与 `Self` 绑定；`pixels()` 返回的切片在 drop 前有效。
pub struct SharedImage {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
    width: u32,
    height: u32,
    stride: u32,
    writable: bool,
}

impl SharedImage {
    /// 宿主侧：创建并清零映射，写入头部。
    pub fn create(name: &str, width: u32, height: u32, stride: u32) -> Result<Self, String> {
        validate(name, width, height, stride)?;
        let len = pixel_len(width, height, stride)?;
        let total = HEADER_LEN + len;
        let total_u32 = u32::try_from(total).map_err(|_| "共享内存超过 4 GiB".to_string())?;
        let name_w = wide(name);

        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                total_u32,
                PCWSTR(name_w.as_ptr()),
            )
        }
        .map_err(|e| format!("CreateFileMappingW({name}) 失败：{e}"))?;

        let view = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, total) };
        if view.Value.is_null() {
            let _ = unsafe { CloseHandle(handle) };
            return Err(format!("MapViewOfFile({name}) 失败"));
        }

        let mut me = Self {
            handle,
            view,
            len,
            width,
            height,
            stride,
            writable: true,
        };
        me.write_header();
        unsafe { std::ptr::write_bytes(me.data_ptr(), 0, me.len) };
        Ok(me)
    }

    /// 扩展侧：只读打开宿主创建的映射，并校验头部。
    pub fn open(name: &str) -> Result<Self, String> {
        if !name.starts_with("Local\\rupix-ext-") {
            return Err("共享内存名必须以 Local\\rupix-ext- 开头".into());
        }
        let name_w = wide(name);
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(name_w.as_ptr())) }
            .map_err(|e| format!("OpenFileMappingW({name}) 失败：{e}"))?;

        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, HEADER_LEN) };
        if view.Value.is_null() {
            let _ = unsafe { CloseHandle(handle) };
            return Err(format!("MapViewOfFile({name}) 失败"));
        }

        // 先由 me 托管头视图与句柄，后续校验或完整映射失败均走 Drop。
        let mut me = Self {
            handle,
            view,
            len: 0,
            width: 0,
            height: 0,
            stride: 0,
            writable: false,
        };
        // 仅消费一次本地快照；重映射后不能再读共享头，否则校验与使用会脱节。
        let header = unsafe { std::ptr::read_volatile(me.header_ptr().cast::<[u8; HEADER_LEN]>()) };
        me.read_and_validate_header(&header)?;
        let total = HEADER_LEN
            .checked_add(me.len)
            .filter(|&total| total <= isize::MAX as usize)
            .ok_or("共享内存长度溢出")?;
        // 长度 0 只映射对象实际大小，不能证明头声明的像素可读；显式长度由 OS 拒短。
        let full_view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, total) };
        if full_view.Value.is_null() {
            return Err(format!("MapViewOfFile({name}) 失败"));
        }
        let _ = unsafe { UnmapViewOfFile(me.view) };
        me.view = full_view;
        Ok(me)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    pub fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data_ptr(), self.len) }
    }

    /// 只在宿主侧（`create`）可用；扩展的只读映射调用会返回错误。
    pub fn pixels_mut(&mut self) -> Result<&mut [u8], String> {
        if !self.writable {
            return Err("共享内存是只读映射".into());
        }
        Ok(unsafe { std::slice::from_raw_parts_mut(self.data_ptr(), self.len) })
    }

    /// 把一段 RGBA 拷进映射。长度必须刚好等于 `stride*height`。
    pub fn copy_from(&mut self, rgba: &[u8]) -> Result<(), String> {
        if rgba.len() != self.len {
            return Err(format!("像素长度 {} ≠ 共享内存 {}", rgba.len(), self.len));
        }
        self.pixels_mut()?.copy_from_slice(rgba);
        Ok(())
    }

    fn data_ptr(&self) -> *mut u8 {
        unsafe { self.view.Value.cast::<u8>().add(HEADER_LEN) }
    }

    fn header_ptr(&self) -> *mut u8 {
        self.view.Value.cast::<u8>()
    }

    fn write_header(&mut self) {
        let h = self.header_ptr();
        let mut bytes = [0u8; HEADER_LEN];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&self.width.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.height.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.stride.to_le_bytes());
        bytes[20..24].copy_from_slice(&(self.len as u32).to_le_bytes());
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), h, HEADER_LEN) };
    }

    fn read_and_validate_header(&mut self, h: &[u8; HEADER_LEN]) -> Result<(), String> {
        if &h[..8] != MAGIC {
            return Err("共享内存 magic 不匹配".into());
        }
        let width = u32::from_le_bytes(h[8..12].try_into().unwrap_or([0; 4]));
        let height = u32::from_le_bytes(h[12..16].try_into().unwrap_or([0; 4]));
        let stride = u32::from_le_bytes(h[16..20].try_into().unwrap_or([0; 4]));
        let len = u32::from_le_bytes(h[20..24].try_into().unwrap_or([0; 4])) as usize;
        validate("", width, height, stride)?;
        if len != pixel_len(width, height, stride)? {
            return Err(format!("共享内存长度 {len} 与头不一致"));
        }
        self.width = width;
        self.height = height;
        self.stride = stride;
        self.len = len;
        Ok(())
    }
}

impl Drop for SharedImage {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

fn validate(name: &str, width: u32, height: u32, stride: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("共享内存尺寸不能为 0".into());
    }
    let min_stride = width.checked_mul(4).ok_or("宽度溢出")?;
    if stride < min_stride {
        return Err(format!("stride {stride} < width*4 {min_stride}"));
    }
    if !name.is_empty() && !name.starts_with("Local\\rupix-ext-") {
        return Err("共享内存名必须以 Local\\\\rupix-ext- 开头".into());
    }
    Ok(())
}

fn pixel_len(width: u32, height: u32, stride: u32) -> Result<usize, String> {
    let stride = stride as usize;
    let height = height as usize;
    let len = stride.checked_mul(height).ok_or("像素长度溢出")?;
    if len > u32::MAX as usize {
        return Err("像素长度超过 4 GiB".into());
    }
    let _ = width;
    Ok(len)
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_name(tag: &str) -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!(
            "Local\\rupix-ext-test-{}-{}-{}",
            std::process::id(),
            tag,
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn create_open_read_pixels() {
        let name = unique_name("read");
        let mut host = SharedImage::create(&name, 2, 2, 8).expect("create");
        assert!(host.is_writable());
        assert_eq!((host.width(), host.height(), host.stride()), (2, 2, 8));
        host.copy_from(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
            .expect("copy");

        let mut ext = SharedImage::open(&name).expect("open");
        assert!(!ext.is_writable());
        assert_eq!(
            (ext.width(), ext.height(), ext.stride(), ext.len()),
            (2, 2, 8, 16)
        );
        assert_eq!(
            ext.pixels(),
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        assert!(ext.pixels_mut().is_err());
    }

    #[test]
    fn open_rejects_short_mapping_with_valid_header() {
        let name = unique_name("short");
        let host = SharedImage::create(&name, 1, 1, 4).expect("short mapping");
        let mut header = [0u8; HEADER_LEN];
        header[..8].copy_from_slice(b"RPXIMG01");
        header[8..12].copy_from_slice(&1024u32.to_le_bytes());
        header[12..16].copy_from_slice(&1024u32.to_le_bytes());
        header[16..20].copy_from_slice(&4096u32.to_le_bytes());
        header[20..24].copy_from_slice(&4_194_304u32.to_le_bytes());
        // 声明长度跨过 OS 页取整的余量；红臂只检查 open，不读越界像素。
        unsafe {
            std::ptr::copy_nonoverlapping(header.as_ptr(), host.header_ptr(), HEADER_LEN);
        }
        assert!(
            SharedImage::open(&name).is_err(),
            "合法头不能为只有 36 字节的映射担保 4 MiB 像素"
        );
        drop(host);
        let name_w = wide(&name);
        let leaked = unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(name_w.as_ptr())) };
        if let Ok(handle) = leaked {
            let _ = unsafe { CloseHandle(handle) };
            panic!("open 失败后泄漏了映射句柄或视图");
        }
    }

    #[test]
    fn rejects_bad_name_and_size() {
        assert!(SharedImage::create("Global\\x", 2, 2, 8).is_err());
        assert!(SharedImage::create(&unique_name("bad"), 2, 2, 4).is_err());
        assert!(SharedImage::create(&unique_name("zero"), 0, 2, 8).is_err());
    }

    #[test]
    fn open_missing_name_fails() {
        assert!(SharedImage::open(&unique_name("missing")).is_err());
    }
}
