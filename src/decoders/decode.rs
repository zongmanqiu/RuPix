//! 解码调用：常规解码、裸数据参数化解码、以及「ext 入围但解不动」时的嗅探兜底。
//!
//! 每个 FFI 调用都经 [`crate::platform::seh::invoke`] 包一层（SEH + catch_unwind），
//! 失败按 [`super::select::Fail`] 分类，决定还值不值得换一个解码器再试。
use super::output::*;
use super::select::*;
use super::*;
use crate::core::pixels::{Anim, MAX_EDGE};

/// 把解码器回填的 `IvRenderOut` 归一成宿主的播放元信息：**页与帧在这里合流**。
///
/// ★ 一个文件要么多帧（GIF / WebP / APNG，看 `frame_count` + `frame_delay_ms`），
///   要么多页（PDF / TIFF / MPO，看 `page_count`）——同一枚解码器不会两者都报。
///   所以总数取两者较大者，`paged` 只用来选**人话**（页/帧）与"打开时停不停车"。
///
/// ★ 多页的"延时"是宿主给的常数（[`Anim::PAGE_DELAY_MS`]），不是文件里的：
///   分页文档没有帧时长这个概念，1 秒一张是产品裁决（用户 2026-11-02）。
///   这一条同时保证多页文件 `is_animated()` 为真 ⇒ 浮条自动挂上播放组，
///   与 GIF **走完全同一套 UI、同一条通道、同一组快捷键**。
///
/// ★ 兼容解码器只报帧数、不报页数的老实现：只要有不止一张，就给延时——
///   "有 3 张却翻不动"比"文案写成帧"糟得多。
///
/// ★ 三档延时（用户 2026-11-02 两次拍板之后的终态）：
///   - 文件自己报了帧延时 → 照它播（GIF 的老账，一颗字都不改）；
///   - 分页文档 → [`Anim::PAGE_DELAY_MS`]（1 秒/页，页要留时间给人读）；
///   - 多帧但延时报 0（首帧爱写 `delay=0` 的 GIF/APNG）→ [`Anim::FALLBACK_FRAME_MS`]，
///     否则宿主判它静态、永不推进，用户看到的是"这张 GIF 不动了"。
pub(super) fn anim_of(index: u32, info: &IvRenderOut) -> Anim {
    let paged = info.page_count > 1 && info.frame_count <= 1;
    let count = info.frame_count.max(info.page_count);
    let delay_ms = if info.frame_delay_ms > 0 {
        info.frame_delay_ms
    } else if paged {
        Anim::PAGE_DELAY_MS
    } else if count > 1 {
        Anim::FALLBACK_FRAME_MS
    } else {
        0
    };
    Anim {
        index,
        count,
        delay_ms,
        paged,
    }
}

/// 这一次解码要不要**补查文档自然尺寸**（零目标查询）。
///
/// # 为什么要有这一问
///
/// 位图的 `Decoded::natural` = 原图尺寸：解码器回填的 `actual_*` 就是原图（走原生
/// 降采样的档位由另一次补查拿回来）。
///
/// 矢量不一样——解码器按 `target` 把**文档重画**成目标尺寸，`actual_*` 回报的是
/// "这一次画出来的画布"，**不是文档有多大**。而宿主把 `natural` 当**文档自然像素**
/// 用：画布几何（`eff_dims` → `dst_rect`）与**区域请求的坐标系**（ABI 契约：区域的
/// x/y/w/h 单位 = 文档自然像素）都吃它。拿渲染尺寸冒充文档尺寸 ⇒ 区域请求要的那块
/// 与真图那块不是同一块 ⇒ 放大后"图片显示不全"（真机：WMF 放大后只见到一角）。
///
/// # 判据（三条都成立才补查）
///
/// 1. **解码器自渲染的矢量**（[`crate::core::formats::VectorRender::DllSelf`]）——
///    宿主内置的 svg/svgz 走宿主自己的 resvg，根本不经过本层。
/// 2. **能按可视区域重渲**（[`crate::core::formats::is_region_renderable`]）——
///    只有走区域路线的后缀，`natural` 才会被当文档坐标系用。判据与 viewer 分流
///    **同一份**，两边分叉就等于坐标系与画布几何对不上。
/// 3. **这一次不是"要原始尺寸"**（`max_w/max_h` 不全为 0）——那一次解码器回填的
///    已经是原尺寸，补查是白跑。
///
/// 没有区域通道的矢量后缀保持旧口径：那里 `natural` 只喂"是否已是整图"的闸，
/// 改成文档尺寸会让它突然去整幅渲原始尺寸（大文档直接打不开）。
pub(super) fn needs_doc_natural(ext: Option<&str>, path: &Path, max_w: u32, max_h: u32) -> bool {
    if max_w == 0 && max_h == 0 {
        return false;
    }
    let e = ext.map(|s| s.to_ascii_lowercase()).or_else(|| {
        path.extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase())
    });
    let Some(e) = e else { return false };
    matches!(
        crate::core::formats::vector_render_mode(&e),
        Some(crate::core::formats::VectorRender::DllSelf)
    ) && crate::core::formats::is_region_renderable(&e)
}

impl Host {
    /// 解码器路径的解码入口。`None` = 没有解码器认领这个扩展名（调用方据此报"无法打开"错误）。
    ///
    /// 两阶段：查询（`out=NULL`）拿尺寸与 `bytes_needed`，宿主分配后再渲染。
    /// `index` 是"要第几张"（0 起；静态图解码器可忽略）。**页与帧同值**：
    /// 多帧格式把它当帧号、多页格式把它当页号，见 [`crate::core::pixels::Anim`]。
    /// 宿主只维护这一个号，同一个文件不会既多帧又多页。
    pub fn try_decode(
        &mut self,
        path: &Path,
        ext: &str,
        max_w: u32,
        max_h: u32,
        frame_index: u32,
        allow_scaled: bool,
    ) -> Option<Result<Decoded, crate::core::error::ViewFailure>> {
        if safe_mode() {
            return None;
        }
        // 1) 按扩展名选解码器（最快、最稳）：只有认领该后缀的解码器入围；能力位无要求。
        if let Some(key) = self.candidate_for_ext(ext, 0) {
            let r = self.decode_with(
                &key,
                path,
                Some(ext),
                max_w,
                max_h,
                frame_index,
                allow_scaled,
            );
            match r {
                Ok(d) => return Some(Ok(d)),
                // 硬失败（损坏/过大/panic）：换解码器没意义，原错误直接交出去。
                Err(e) if !e.worth_sniff() => return Some(Err(e.into_failure())),
                // 内容不认 / 本解码器不支持：扩展名只说明"名字像"，内容可能是别的格式
                // （.jxl 改名 .png 是典型），也可能是冠军刻意让位的子集（CMYK / 调色板
                // TIFF）。按嗅探榜单逐个再试，见 [`Host::try_chain`]。
                Err(e) => {
                    let ranked = self.sniff_ranked(path, Some(key.as_path()));
                    // 把冠军的判词当作"第一个让位者"播种进链：若榜上解码器全都主动让位，
                    // 用户挑的是这个后缀，冠军那句"不支持"比跑龙套的那句更有资格出门。
                    // 播了判词就不可能返回 None：这条路径必须有话说。
                    return self.try_chain(
                        ranked,
                        path,
                        max_w,
                        max_h,
                        frame_index,
                        allow_scaled,
                        Some(e.into_failure()),
                    );
                }
            }
        }
        // 2) 兜底：扩展名没人认领（改名 / 无扩展名文件）——对全部已加载解码器做魔数嗅探。
        //    只对"显式打开"路径触发：目录浏览已按扩展名过滤，根本不会走到这里；
        //    嗅探代价极小（每解码器只读文件头几个字节），且若某解码器嗅探本身有 bug，
        //    由 SEH 隔离 + 三振兜底，不会拖垮进程。
        let ranked = self.sniff_ranked(path, None);
        self.try_chain(ranked, path, max_w, max_h, frame_index, allow_scaled, None)
    }

    /// 兜底解码榜单：对所有已加载解码器（未被关掉/拉黑）逐一 `iv_probe(data, *, NULL)`，
    /// 按**置信度降序**返回全部分数 >0 的解码器；平手按 DLL 路径字典序。
    ///
    /// `ext = NULL` 让解码器**只按内容（魔数）认领**——这正是"改名 / 无扩展名"文件
    /// 能被打开的关键：例如把 `.jxl` 改成 `.bin`，`jxl` 解码器的 `iv_probe` 仍按 `FF 0A`
    /// 魔数认出它。依赖扩展名才能认领的格式（TGA 无魔数、thumbdoc 等）在此兜底下
    /// 不会误认，符合 ABI 约定（规范 §六 第 3 条 / §新增「ext=NULL 嗅探约定」）。
    ///
    /// # 为什么取"最高分"而不是"第一个 >0"
    ///
    /// 规范把 `iv_probe` 的返回值定义为置信度（0 = 不认，100 = 确定认得），但实现
    /// 一直是"第一个 >0 胜出"——在只有一两个解码器认同一份魔数时够用，一旦同一格式
    /// 有"冠军 DLL + core 通用兜底"两个候选（JPEG 就是第一个），按 DLL 路径字典序
    /// 就变成 `core` 抢先，冠军 DLL 的实测加速对无扩展名/改名文件永久失效。
    /// 取最高分才是规范的原意，也让 core 能安心当低置信度兜底手（见下）。
    ///
    /// # 为什么交"榜单"而不是只交冠军
    ///
    /// 一库一 DLL 的纪律是：**专项冠军只做该格式里最快的那个子集**，子集外（TIFF 的
    /// 调色板 / CMYK / planar / 浮点）必须用 `ERR_UNSUPPORTED` 交回宿主——冠军里塞满
    /// 长尾就又变成 `image` 那种"什么都做、样样不最快"。可若兜底只取第一名，被交回的
    /// 那部分内容就永久没人接，冠军搬家直接变成功能倒退（实测：`.tif` 交给 TIFF 冠军后，
    /// CMYK TIFF 就打不开了）。所以按分值从高到低**逐个试**，见 [`Host::try_chain`]。
    ///
    /// `skip`：刚失败过的解码器不再进榜（ext 重试路径用——core 这类通用解码器靠
    /// `guess_format` 认得很多魔数，却未必编入对应解码器，不跳过它兜底就永远
    /// 落回失败的原解码器）。
    ///
    /// 仅"显式打开"路径调用——目录浏览用 [`crate::core::gallery::is_image_ext_in`] 按扩展名
    /// 过滤，未认领后缀的文件根本不进网格，不会触发逐解码器嗅探的 O(N) 开销。
    pub(super) fn sniff_ranked(&mut self, path: &Path, skip: Option<&Path>) -> Vec<(i32, PathBuf)> {
        if safe_mode() {
            return Vec::new();
        }
        self.refresh_catalog();
        // 读文件（mmap，失败回退整读）。嗅探只读头几个字节，开销极小。
        let mapped = std::fs::File::open(path)
            .ok()
            .and_then(|f| unsafe { memmap2::Mmap::map(&f).ok() });
        let fallback: Vec<u8>;
        let data: &[u8] = match &mapped {
            Some(m) => &m[..],
            None => {
                let Ok(bytes) = std::fs::read(path) else {
                    return Vec::new();
                };
                fallback = bytes;
                &fallback
            }
        };
        // 路径排序后遍历，保证同名多版本选择一致，也让平手分数的先后确定。
        // 已停用的（目录里没了 / 被同 id 更高版本取代）不参与嗅探——它们只是**没卸库**，
        // 不是还能用。
        let mut keys: Vec<PathBuf> = self
            .loaded
            .iter()
            .filter(|(_, l)| l.active)
            .map(|(k, _)| k.clone())
            .collect();
        keys.sort();
        let mut ranked: Vec<(i32, PathBuf)> = Vec::new();
        for k in keys {
            if skip == Some(k.as_path()) {
                continue;
            }
            // 先取解码器引用，确认未禁用/未拉黑，并 clone 出 id（避免持有 self.loaded 的
            // 不可变借用跨越下面的 self.penalize 可变借用）。
            let (probe, id) = {
                let l = match self.loaded.get(&k) {
                    Some(l) => l,
                    None => continue,
                };
                if is_disabled(&l.desc.id) || self.blacklist.contains(&l.desc.id) {
                    continue;
                }
                (l.sym.probe, l.desc.id.clone())
            };
            // 同一枚 DLL 的调用权（`dll_lock` 模块头）：拿不到就跳过这一枚——
            // "别人正在用"不是"这枚坏了"，不许记成故障。
            let Ok(_dll) = super::dll_lock::enter(&k, &id, self.class()) else {
                continue;
            };
            // NULL 扩展名嗅探（解码器按内容认领）。
            match invoke(|| unsafe { probe(data.as_ptr(), data.len(), std::ptr::null()) }) {
                Ok(c) if c > 0 => ranked.push((c, k)),
                Ok(_) => continue,
                Err(e) => {
                    // 该解码器嗅探时崩了（panic / SEH）→ 记账，继续试下一个。
                    self.charge_fault(&id, e.as_ref());
                    continue;
                }
            }
        }
        // 稳定排序：分数降序；同分保留上面的路径字典序。
        rank_candidates(ranked)
    }

    /// 按 [`Host::sniff_ranked`] 的榜单逐个尝试，直到成功或撞上硬失败。
    ///
    /// 硬失败（损坏 / 过大 / panic）立即中止并交出它的判词——坏文件换解码器再全解一遍，
    /// 只是把同一句"已损坏"重复几秒；"不认 / 不支持"则是解码器**主动让位**，接着问下一家。
    ///
    /// 全军覆没时回报**第一条判词**：`first_verdict` 是调用方给的"榜首之上"的判词
    /// （按扩展名入围的冠军那句——用户挑的就是这个后缀，它比跑龙套的解码器更有资格出门），
    /// 没有则用榜单第一名的（置信度最高的解码器）。
    ///
    /// 返回 `None` 只有一种情况：榜单为空**且**调用方没给判词（= 没有解码器认领，
    /// 调用方据此报"无法打开"）。给了 `first_verdict` 就绝不返回 `None`。
    // 参数平铺：调用面与 ABI 请求面一一对应，见 `core::decode::DecodeBackend::decode` 的同款取舍。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_chain(
        &mut self,
        ranked: Vec<(i32, PathBuf)>,
        path: &Path,
        max_w: u32,
        max_h: u32,
        frame_index: u32,
        allow_scaled: bool,
        first_verdict: Option<crate::core::error::ViewFailure>,
    ) -> Option<Result<Decoded, crate::core::error::ViewFailure>> {
        let mut first_soft = first_verdict;
        for (_score, key) in ranked {
            match self.decode_with(&key, path, None, max_w, max_h, frame_index, allow_scaled) {
                Ok(d) => return Some(Ok(d)),
                Err(e) if !e.worth_sniff() => return Some(Err(e.into_failure())),
                Err(e) => {
                    if first_soft.is_none() {
                        first_soft = Some(e.into_failure());
                    }
                }
            }
        }
        first_soft.map(Err)
    }

    /// 裸数据参数化解码入口（`.raw` 等无魔数文件）。`None` = 没有解码器认领。
    ///
    /// 走可选导出 `iv_render_raw`（规范 §十二）：宽/高/布局由用户在参数窗给出。
    /// 解码器没实现该导出时给出可行动的提示，而不是一句含混的"打不开"。
    pub fn try_decode_raw(
        &mut self,
        path: &Path,
        ext: &str,
        params: RawParams,
        max_w: u32,
        max_h: u32,
    ) -> Option<Result<Decoded, crate::core::error::ViewFailure>> {
        if safe_mode() {
            return None;
        }
        // 裸数据：只挑声明了 `CAP_RAW_PARAM` 的解码器（按能力位，不按 id）。
        let key = self.candidate_for_ext(ext, CAP_RAW_PARAM)?;
        Some(self.decode_raw_with(&key, path, params, max_w, max_h))
    }

    pub(super) fn decode_raw_with(
        &mut self,
        key: &Path,
        path: &Path,
        params: RawParams,
        max_w: u32,
        max_h: u32,
    ) -> Result<Decoded, crate::core::error::ViewFailure> {
        let (render_raw, last_error, id) = {
            let Some(l) = self.loaded.get(key) else {
                return Err(crate::core::error::ViewFailure::decode("解码器已卸载"));
            };
            (l.sym.render_raw, l.sym.last_error, l.desc.id.clone())
        };
        let Some(render_raw) = render_raw else {
            return Err(crate::core::error::ViewFailure::unsupported(format!(
                "解码器「{id}」不支持裸数据参数化解码（缺少 iv_render_raw 导出）"
            )));
        };
        // 同一枚 DLL 的调用权（见 `dll_lock` 模块头）。
        let _dll = super::dll_lock::enter(key, &id, self.class())
            .map_err(crate::core::error::ViewFailure::unsupported)?;
        let mapped = std::fs::File::open(path)
            .ok()
            .and_then(|f| unsafe { memmap2::Mmap::map(&f).ok() });
        let fallback: Vec<u8>;
        let data: &[u8] = match &mapped {
            Some(m) => &m[..],
            None => {
                fallback = std::fs::read(path).map_err(crate::core::error::ViewFailure::from_io)?;
                &fallback
            }
        };

        // ★ 这里**不调用 probe**：裸数据没有魔数可嗅探，认领的依据是解码器自述的扩展名
        //   加用户填的宽/高/布局（他已显式声明"这就是 1920×1080 的 RGBA dump"）。
        //   走 probe 反而会让"按扩展名认领"的参数化解码器永远排队.routing 在外层的
        //   `candidate_for_ext(_, CAP_RAW_PARAM)` 已经替我们筛好了解码器。

        let req = IvRenderReq {
            target_w: max_w,
            target_h: max_h,
            budget_bytes: (MAX_PIXELS * 4) as u32,
            flags: 0,
            frame_index: 0,
            page_index: 0,
        };

        // ① 查询阶段：out = NULL（参数化解码器此时已校验过 len == w*h*bpp）。
        let mut info = IvRenderOut::default();
        let q = invoke(|| unsafe {
            render_raw(
                data.as_ptr(),
                data.len(),
                params.width,
                params.height,
                params.layout,
                std::ptr::addr_of!(req),
                std::ptr::null_mut(),
                0,
                std::ptr::addr_of_mut!(info),
            )
        });
        match q {
            Ok(ERR_OK) => {}
            Ok(code) => {
                let detail = last_error_msg(last_error);
                if code == ERR_PANIC {
                    self.penalize(&id);
                }
                let msg = describe_code(&id, code, &detail);
                return Err(if code == ERR_UNSUPPORTED {
                    crate::core::error::ViewFailure::unsupported(msg)
                } else {
                    crate::core::error::ViewFailure::decode(msg)
                });
            }
            Err(e) => {
                self.charge_fault(&id, e.as_ref());
                return Err(crate::core::error::ViewFailure::decode(format!(
                    "解码器「{id}」出错，已隔离"
                )));
            }
        }

        let (aw, ah) = (info.actual_w, info.actual_h);
        if aw == 0 || ah == 0 {
            return Err(crate::core::error::ViewFailure::decode(
                "解码器返回了 0 尺寸",
            ));
        }
        let px = aw as u64 * ah as u64;
        if px > MAX_PIXELS || info.bytes_needed > MAX_PIXELS * 4 {
            return Err(crate::core::error::ViewFailure::decode(too_large_msg(
                aw, ah,
            )));
        }

        // ② 渲染阶段：宿主按契约分配 `bytes_needed + icc_len`（规范 §十一：ICC 紧跟像素之后）。
        // 两段长度**取查询阶段的值**并在分配前过闸——渲染阶段解码器会覆写 info，之后再读就是拿
        // 解码器自己填的数当分配依据了。
        let (pix_len, icc_len) =
            out_segments(&info).map_err(crate::core::error::ViewFailure::decode)?;
        let mut buf = vec![0u8; pix_len + icc_len];
        let r = invoke(|| unsafe {
            render_raw(
                data.as_ptr(),
                data.len(),
                params.width,
                params.height,
                params.layout,
                std::ptr::addr_of!(req),
                buf.as_mut_ptr(),
                buf.len(),
                std::ptr::addr_of_mut!(info),
            )
        });
        match r {
            Ok(ERR_OK) => {}
            Ok(code) => {
                let detail = last_error_msg(last_error);
                if code == ERR_PANIC {
                    self.penalize(&id);
                }
                let msg = describe_code(&id, code, &detail);
                return Err(if code == ERR_UNSUPPORTED {
                    crate::core::error::ViewFailure::unsupported(msg)
                } else {
                    crate::core::error::ViewFailure::decode(msg)
                });
            }
            Err(e) => {
                self.charge_fault(&id, e.as_ref());
                return Err(crate::core::error::ViewFailure::decode(format!(
                    "解码器「{id}」出错，已隔离"
                )));
            }
        }
        // 渲染成功 = "连续出错"这条链断了，三振计数复位（审计 V10）。
        self.clear_strikes(&id);

        // ③ 后处理：与常规路径同一条管线（参数化实现通常恒报 PF_RGBA8，to_rgba8 零拷贝）。
        // ICC 尾段照常取出（相机 RAW 实现实际恒报 0），像素段交给下游。
        let icc = take_icc(&mut buf, pix_len, icc_len);
        let rgba = to_rgba8(buf, aw, ah, info.pixel_format, info.bit_depth)
            .map_err(crate::core::error::ViewFailure::decode)?;
        let alpha_format = pixel_format_has_alpha(info.pixel_format);
        let mut bitmap = fit_to_with_alpha(rgba, aw, ah, max_w, max_h, alpha_format);
        apply_icc(&mut bitmap, &icc, path);
        Ok(Decoded {
            natural: (aw, ah),
            region: None,
            bitmap,
            anim: Anim::STATIC,
        })
    }

    /// 区域渲染（**ABI v6**）：认领该后缀、且实现了可选导出 `iv_render_region` 的解码器，
    /// 按"只渲看得见的那一块"出图。
    ///
    /// `None` = 没有解码器支持这条通道 ⇒ 宿主落回整幅路线（与今天同档，不算错）。
    ///
    /// ★ 这条路是"矢量图放大到 4000% 也无损"的机制：宿主给的区域是**视口那一小块**
    /// （图像坐标），画布 = 区域 × 密度 ≈ 一屏大小 ⇒ 像素预算恒定、倍率再高也顶得住；
    /// 而解码器是**按那个密度重新光栅化**的，不是位图插值放大。
    pub fn try_decode_region(
        &mut self,
        path: &Path,
        ext: &str,
        region: &crate::core::decode::RegionReq,
    ) -> Option<Result<Decoded, crate::core::error::ViewFailure>> {
        if safe_mode() {
            return None;
        }
        crate::core::decode::set_phase(crate::core::decode::PHASE_REGION);
        self.refresh_catalog();
        // 候选：认领该后缀 **且** 实现了 `iv_render_region`。按 id 排序 ⇒ 多枚认领
        // 同一后缀时结果可复现（与矢量导出那条同规）。
        // 候选表带上 DLL 路径：区域重渲同样要拿"这一枚 DLL"的调用权（见 `dll_lock`）。
        let mut cands: Vec<(
            PathBuf,
            String,
            crate::core::abi::IvRenderRegionFn,
            unsafe extern "C" fn(*mut c_char, usize) -> i32,
        )> = Vec::new();
        for (k, l) in self.loaded.iter() {
            if !l.active || is_disabled(&l.desc.id) || self.blacklist.contains(&l.desc.id) {
                continue;
            }
            if !l
                .desc
                .extensions
                .iter()
                .any(|x| x.eq_ignore_ascii_case(ext))
            {
                continue;
            }
            let Some(f) = l.sym.render_region else {
                continue;
            };
            cands.push((k.clone(), l.desc.id.clone(), f, l.sym.last_error));
        }
        if cands.is_empty() {
            return None;
        }
        cands.sort_by(|a, b| a.1.cmp(&b.1));

        // 画布 = 区域 × 密度；超硬顶就降密度（与内置 resvg 的区域路线同一条口径：
        // 宁可略糊，也不要失败或越界）。画布恒 ≈ 一屏大小，所以平时根本碰不到硬顶。
        let (tw, th, density) = region_target(region.size, region.density);

        let mapped = std::fs::File::open(path)
            .ok()
            .and_then(|f| unsafe { memmap2::Mmap::map(&f).ok() });
        let fallback: Vec<u8>;
        let data: &[u8] = match &mapped {
            Some(m) => &m[..],
            None => {
                fallback = std::fs::read(path)
                    .map_err(|e| format!("读不到文件：{e}"))
                    .ok()?;
                &fallback
            }
        };

        let req = crate::core::abi::IvRegionReq {
            x: region.origin.0,
            y: region.origin.1,
            w: region.size.0,
            h: region.size.1,
            target_w: tw,
            target_h: th,
            budget_bytes: (MAX_PIXELS * 4) as u32,
            flags: 0,
            // 页与帧同值（见 `core::decode::RegionReq::index`）：区域重渲必须落在
            // **当前看的那一张**上，否则多页文档一放大就跳回第一页。
            frame_index: region.index,
            page_index: region.index,
        };
        let ext_c = std::ffi::CString::new(ext).ok();

        let mut last_err = String::new();
        for (key, id, f, last_error) in cands {
            let Ok(_dll) = super::dll_lock::enter(&key, &id, self.class()) else {
                continue;
            };
            match region_roundtrip(f, last_error, data, &req, ext_c.as_ref()) {
                Ok((mut bitmap, aw, _ah, icc, info)) => {
                    self.clear_strikes(&id);
                    // ★ 解码器在**成功**的渲染里也可能留一句话（如"文档字体缺失，
                    // 已用兜底字体渲染"）。契约要求字体缺失时**照样渲染**（不许丢字），
                    // 那句话就是唯一的可观测痕迹——宿主把它收进日志，别让它悄悄丢掉。
                    let note = last_error_msg(last_error);
                    if !note.is_empty() {
                        log::info!(
                            "[vector-note] decoder={id} file={} note={note}",
                            path.display()
                        );
                    }
                    apply_icc(&mut bitmap, &icc, path);
                    // 区域信息随结果带回：上层据此把这块切片摆回视口（与内置 SVG 路线同一条）。
                    let eff_density = if region.size.0 > 0.0 {
                        aw as f32 / region.size.0
                    } else {
                        density
                    };
                    return Some(Ok(Decoded {
                        natural: region.doc,
                        region: Some(crate::core::pixels::Region {
                            origin: region.origin,
                            density: eff_density,
                        }),
                        bitmap,
                        // ★ 与整幅解码**同一条**归一（[`anim_of`]）：区域重渲不许
                        //   把"这张文件有几页"抹掉。回到第一页的只有画布，不是元信息。
                        anim: anim_of(region.index, &info),
                    }));
                }
                Err(e) => {
                    last_err = format!("解码器「{id}」{e}");
                    continue;
                }
            }
        }
        Some(Err(if last_err.is_empty() {
            crate::core::error::ViewFailure::unsupported("解码器正忙，本次区域重渲没有执行")
        } else {
            crate::core::error::ViewFailure::decode(last_err)
        }))
    }

    // 参数平铺：调用面与 ABI 请求面一一对应，见 `core::decode::DecodeBackend::decode` 的同款取舍。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn decode_with(
        &mut self,
        key: &Path,
        path: &Path,
        ext: Option<&str>,
        max_w: u32,
        max_h: u32,
        frame_index: u32,
        allow_scaled: bool,
    ) -> Result<Decoded, Fail> {
        // 拿符号与 id（`Symbols` 是 Copy：拷出来即可，不必长期借用 self.loaded）。
        let (probe, render, last_error, id, caps) = {
            let Some(l) = self.loaded.get(key) else {
                return Err(Fail::Hard("解码器已卸载".into()));
            };
            (
                l.sym.probe,
                l.sym.render,
                l.sym.last_error,
                l.desc.id.clone(),
                l.desc.caps,
            )
        };
        // ★ 挂死恢复（2026-11-04）：把"下一步要进哪个解码器"报给 core，超时时由
        //   监督者读它来决定熔断谁（worker 已经卡死，问不了它）。
        crate::core::decode::report_current_decoder(&id);
        // 已熔断的解码器**一次都不许再进**：僵尸线程还在那个 DLL 里转，ABI 明写
        // 「解码器需可重入但不必 MT-safe」，再进去就是违约。
        // 返回 `Unsupported` 而不是 `Hard`：这是"这一家不能用了"，换下一家还有救。
        if crate::core::decode::is_hung_decoder(&id) {
            return Err(Fail::Unsupported(format!(
                "解码器「{id}」本次会话已因超时停用"
            )));
        }
        // ★ 同一枚 DLL 的调用权（2026-11-05 裁决，见 `dll_lock` 模块头与 `docs/待办.md` §28）：
        //   多开之后两条 worker 真的会同时进来，而 ABI 只承诺"可重入、不必 MT-safe"。
        //   拿不到 = "现在忙"，不是"这枚坏了" ⇒ `Unsupported`（换下一家还有救），不是 `Hard`。
        let _dll = super::dll_lock::enter(key, &id, self.class()).map_err(Fail::Unsupported)?;
        // 读文件走 mmap：省掉 12MB 级的堆分配+拷贝（解码器只读，切片直给）。
        // 文件极小/被独占等 mmap 不可用时回退整读。Windows 下文件在解码途中被
        // 外部替换的理论风险由每次 FFI 的 catch_unwind + 三振黑名单兜底。
        crate::core::decode::set_phase(crate::core::decode::PHASE_READ);
        let t0 = std::time::Instant::now();
        let mapped = std::fs::File::open(path)
            .ok()
            .and_then(|f| unsafe { memmap2::Mmap::map(&f).ok() });
        let fallback: Vec<u8>;
        let data: &[u8] = match &mapped {
            Some(m) => &m[..],
            None => {
                fallback = std::fs::read(path).map_err(Fail::Io)?;
                &fallback
            }
        };
        let trace = std::env::var_os("RUPIX_DECODE_TRACE").is_some();
        let t_read = t0.elapsed();

        let tick = || std::time::Instant::now();

        // 置信度：0 = 不认。低于 0 视作不认。ext 供无魔数格式（TGA）认领。
        // `ext = None` 时传 NULL 指针：解码器只按内容（魔数）认领——改名/无扩展名
        // 文件的兜底解码就靠这个语义（见 [`Host::sniff_ranked`]）。
        let ext_c: Option<std::ffi::CString> = ext.and_then(|s| std::ffi::CString::new(s).ok());
        let ext_ptr: *const c_char = ext_c
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());
        crate::core::decode::set_phase(crate::core::decode::PHASE_PROBE);
        let t0 = tick();
        match invoke(|| unsafe { probe(data.as_ptr(), data.len(), ext_ptr) }) {
            Ok(c) if c > 0 => {}
            Ok(_) => return Err(Fail::NotClaimed(format!("解码器「{id}」不认识这个文件"))),
            Err(e) => {
                self.charge_fault(&id, e.as_ref());
                return Err(Fail::Hard(format!("解码器「{id}」探测时出错，已隔离")));
            }
        }
        let t_probe = t0.elapsed();

        let req = IvRenderReq {
            target_w: max_w,
            target_h: max_h,
            budget_bytes: (MAX_PIXELS * 4) as u32,
            // 缩放意图（规范 §十三）：只在**这次请求真接受降采样**（`ReqKind::wants_scaled`）
            // 且**这枚解码器真声明了 `CAP_SCALED`** 时点亮。两个条件缺任何一个都恒 0，
            // 那条路就是今天逐字节相同的路径——主画面传的是视口尺寸，那儿要的是清晰。
            flags: if allow_scaled && caps & CAP_SCALED != 0 {
                REQ_ALLOW_SCALED
            } else {
                0
            },
            frame_index,
            // v4 的多页选择与 v3 的帧号同值：宿主只维护一个号（见 `Anim`），
            // 解码器各取所需——GIF 看 `frame_index`、PDF/TIFF 看 `page_index`，
            // 而同一个文件只会被其中一种认领。
            page_index: frame_index,
        };

        // ①-a 启用降采样时，补一次"原图到底多大"的查询（见 [`Self::query_natural_only`]）。
        //     只有这枚解码器真会缩放时才补：其余格式 `flags` 恒 0，问两次只是白跑。
        let full_natural = if allow_scaled && caps & CAP_SCALED != 0 && max_w > 0 && max_h > 0 {
            let mut plain = req;
            plain.flags = 0;
            crate::core::decode::set_phase(crate::core::decode::PHASE_NATURAL);
            self.query_natural_only(&id, render, data, &plain, ext_ptr)
        } else {
            None
        };

        // ①-a' 矢量（解码器自渲染）的 natural 必须是**文档自然尺寸**，不是这一次的
        //     渲染尺寸（判据见 [`needs_doc_natural`]）。
        //
        //     位图的 natural = 原图尺寸：解码器回填的 `actual_*` 就是原图（走原生降采样
        //     的档位由 `full_natural` 另补一次查询拿回来）。矢量不一样——解码器按
        //     `target` 把**文档重画**成目标尺寸，`actual_*` 回报的是"这一次画出来的
        //     画布"，不是文档有多大。
        //
        //     宿主把 `natural` 当**文档自然像素**用：画布几何（`eff_dims` → `dst_rect`）
        //     与**区域请求的坐标系**（ABI 契约：区域的 x/y/w/h 单位 = 文档自然像素）
        //     都吃它。拿渲染尺寸冒充文档尺寸 ⇒ 区域请求要的是 557×561 坐标系里的
        //     那一小块，而解码器按 3102×3124 的真文档去裁 ⇒ 位置与比例全错，
        //     放大后表现为"图片显示不全"（真机：WMF 放大后只见到右上角一块）。
        //
        //     契约给了正路：`target_w/target_h = 0` 就是"要原始尺寸"（ABI `IvRenderReq`）。
        //     所以矢量路线补一次**零目标查询**（只查不渲，代价是一次解析）。
        let doc_natural = if needs_doc_natural(ext, path, max_w, max_h) {
            crate::core::decode::set_phase(crate::core::decode::PHASE_DOC);
            self.query_doc_natural(&id, render, data, &req, ext_ptr)
        } else {
            None
        };

        // ① 查询阶段：out = NULL。
        crate::core::decode::set_phase(crate::core::decode::PHASE_QUERY);
        let t0 = tick();
        let mut info = IvRenderOut::default();
        let q = invoke(|| unsafe {
            render(
                data.as_ptr(),
                data.len(),
                std::ptr::addr_of!(req),
                std::ptr::null_mut(),
                0,
                std::ptr::addr_of_mut!(info),
                ext_ptr,
            )
        });
        let t_query = t0.elapsed();
        match q {
            Ok(ERR_OK) => {}
            Ok(code) => {
                let detail = last_error_msg(last_error);
                if code == ERR_PANIC {
                    self.penalize(&id);
                }
                let msg = describe_code(&id, code, &detail);
                return Err(if code == ERR_UNSUPPORTED {
                    Fail::Unsupported(msg)
                } else {
                    Fail::Hard(msg)
                });
            }
            Err(e) => {
                self.charge_fault(&id, e.as_ref());
                return Err(Fail::Hard(format!("解码器「{id}」出错，已隔离")));
            }
        }

        // 合理性检查：`bytes_needed` 与尺寸必须自洽，超预算直接判"过大"。
        let (aw, ah) = (info.actual_w, info.actual_h);
        if aw == 0 || ah == 0 {
            return Err(Fail::Hard("解码器返回了 0 尺寸".into()));
        }
        // 矢量路线的一行留痕：文档自然尺寸 vs 这一次渲染出来的画布。
        // 两个数不同才是正常的（渲染按 target 缩过），相同反而说明这条没生效。
        if let Some((dw, dh)) = doc_natural {
            log::info!(
                "[vector-natural] file={} doc={}x{} render={}x{}",
                path.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                dw,
                dh,
                aw,
                ah
            );
        }
        let px = aw as u64 * ah as u64;
        if px > MAX_PIXELS || info.bytes_needed > MAX_PIXELS * 4 {
            return Err(Fail::Hard(too_large_msg(aw, ah)));
        }

        // ①-b 查询阶段元信息上抛：让 UI 提前拿到"朝向后自然尺寸"，
        //     info bar 与「按原始像素开窗」在整图解码之前就能动起来。
        //     header-only 解码够用的格式（HEIF 走 ispe、RAW 只读头、TGA/PNG 等）
        //     收益明显；必须整解才能报尺寸的格式（JXL/AVIF）Meta ≈ Frame，无害。
        //     契约见 `Meta` 上的注释——查询值仅预览，最终以 `Decoded.natural` 为准。
        //     ★ natural 必须是**原图**尺寸，不是这一次的档位尺寸：降采样把 `actual_w/h`
        //     改写成了档位（JPEG DCT 1/2 会把 6055² 报成 3027²），而状态栏尺寸与
        //     `viewer::tick` 的"是否已是整图"升级闸都吃这个值——报小了那次原图档升级
        //     永不发生，放大到 1:1 就永久糊在档位图上（m35）。
        let q_orient = if info.already_oriented != 0 {
            1
        } else {
            info.orientation
        };
        // ★ natural 的权威值只有一处落定：**原图尺寸**。
        //   - 矢量（解码器自渲染）：`doc_natural` = 文档自然尺寸（零目标查询拿来的）。
        //   - 位图：降采样档位时 `full_natural` 补查回来的原图尺寸。
        //   - 都没有：这一次回填的 `actual_*`（它就是原图尺寸）。
        // 三个来源按"谁更权威"取，顺序即优先级：`doc_natural` > `full_natural` > `actual_*`。
        let natural_px = doc_natural.or(full_natural);
        let (n0w, n0h) = natural_px.unwrap_or((aw, ah));
        let q_natural = match q_orient {
            5..=8 => (n0h, n0w),
            _ => (n0w, n0h),
        };
        crate::core::decode::publish_meta_from_current_req(crate::core::decode::Meta {
            natural: q_natural,
        });

        // ② 渲染阶段：宿主按契约分配 `bytes_needed + icc_len`（规范 §十一），两段长度取查询值、
        // 分配前过闸（同上）。
        crate::core::decode::set_phase(crate::core::decode::PHASE_RENDER);
        let t0 = tick();
        let (pix_len, icc_len) = out_segments(&info).map_err(Fail::Hard)?;
        let mut buf = vec![0u8; pix_len + icc_len];
        let r = invoke(|| unsafe {
            render(
                data.as_ptr(),
                data.len(),
                std::ptr::addr_of!(req),
                buf.as_mut_ptr(),
                buf.len(),
                std::ptr::addr_of_mut!(info),
                ext_ptr,
            )
        });
        match r {
            Ok(ERR_OK) => {}
            Ok(code) => {
                let detail = last_error_msg(last_error);
                if code == ERR_PANIC {
                    self.penalize(&id);
                }
                let msg = describe_code(&id, code, &detail);
                return Err(if code == ERR_UNSUPPORTED {
                    Fail::Unsupported(msg)
                } else {
                    Fail::Hard(msg)
                });
            }
            Err(e) => {
                self.charge_fault(&id, e.as_ref());
                return Err(Fail::Hard(format!("解码器「{id}」出错，已隔离")));
            }
        }

        // 渲染成功 = "连续出错"这条链断了，三振计数复位（审计 V10）。
        self.clear_strikes(&id);

        let t_render = t0.elapsed();

        // ③ 后处理：RGBA 归一 + EXIF 方向补转（`already_oriented=1` 的解码器已转好，
        // 宿主不得再转一次）+ 尺寸适配。natural 尺寸取**转正后**的宽高。
        // ICC 尾段在 truncate 前取出，等缩样之后再转 sRGB（成本跟视口走，m53-J）。
        crate::core::decode::set_phase(crate::core::decode::PHASE_POST);
        let t0 = tick();
        let icc = take_icc(&mut buf, pix_len, icc_len);
        let rgba = to_rgba8(buf, aw, ah, info.pixel_format, info.bit_depth).map_err(Fail::Hard)?;
        let orient = if info.already_oriented != 0 {
            1
        } else {
            info.orientation
        };
        // natural 同样取**原图**尺寸并按同一朝向转正（理由见 ①-b 的注释）。
        let natural = match natural_px {
            Some((fw, fh)) => match orient {
                5..=8 => (fh, fw),
                _ => (fw, fh),
            },
            None => match orient {
                5..=8 => (ah, aw),
                _ => (aw, ah),
            },
        };
        // ★ 转正与缩到上屏框**合做一步**（m49）：`fit_oriented` 把方向反着取样那根轴的
        //   块划分改成远端锚定，于是"整张 48 MB 跨步搬运"那一趟可以省掉——结果与
        //   `fit_to(oriented(..))` **逐字节相同**（用例 `fit_oriented_matches_orient_then_fit`）。
        //   12 MP 竖拍首屏因此少 ~80 ms；方向 1（绝大多数图）走的还是原来的 `fit_to`。
        let alpha_format = pixel_format_has_alpha(info.pixel_format);
        let mut bitmap = fit_oriented_with_alpha(rgba, aw, ah, orient, max_w, max_h, alpha_format);
        apply_icc(&mut bitmap, &icc, path);
        let t_post = t0.elapsed();
        if trace {
            // 五段一律换算成毫秒、字段名带 `_ms`（m59）：`{:.1?}` 那种"µs 与 ms 混排"
            // 是人读的写法，脚本要先认两单位；三小数位的毫秒既够分辨率又只有一个单位。
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            log::debug!(
                "[decode-trace] file={} codec={} read_ms={:.3} probe_ms={:.3} query_ms={:.3} render_ms={:.3} post_ms={:.3}",
                path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                id,
                ms(t_read),
                ms(t_probe),
                ms(t_query),
                ms(t_render),
                ms(t_post),
            );
        }
        Ok(Decoded {
            natural,
            region: None,
            bitmap,
            anim: anim_of(frame_index, &info),
        })
    }

    /// 矢量路线的"**文档自然尺寸**"查询：零目标（= 原始尺寸），只查不渲。
    ///
    /// ★ 为什么矢量要单独问一次：解码器按 `target` 把**文档重画**成目标尺寸，
    /// 它回填的 `actual_*` 是"这一次画出来的画布"，不是文档有多大。宿主却把
    /// `natural` 当**文档自然像素**用（画布几何 + 区域请求的坐标系），两者必须分开。
    ///
    /// 契约给了正路：`target_w/target_h = 0` 就是"要原始尺寸"（ABI `IvRenderReq`），
    /// 所以这里发一次**零目标查询**——只查不渲，代价是一次解析（解码器侧通常有
    /// 文档缓存，第二次几乎不要钱）。
    ///
    /// 失败一律降级为 `None`（`natural` 退回这一次的渲染尺寸 = 旧行为），绝不把图
    /// 判死：少一次查询只是"放大后坐标可能对不上"，比"打不开"轻得多。
    ///
    /// **预算闸的例外**：文档比宿主像素预算还大（>64 MP）时，零目标查询会被
    /// `budget_bytes` 挡回来（`ERR_TOO_LARGE`）。这时用"不限预算"再问一次——
    /// 查询阶段**不写像素**（契约：`out = NULL` 只回填 `info`），放宽预算不会让
    /// 解码器多分配像素；真正的渲染仍受宿主自己的 `MAX_PIXELS` 闸与解码器自己的
    /// 分配闸管着。
    pub(super) fn query_doc_natural(
        &mut self,
        id: &str,
        render: unsafe extern "C" fn(
            *const u8,
            usize,
            *const IvRenderReq,
            *mut u8,
            usize,
            *mut IvRenderOut,
            *const c_char,
        ) -> i32,
        data: &[u8],
        req: &IvRenderReq,
        ext_ptr: *const c_char,
    ) -> Option<(u32, u32)> {
        let mut zero = *req;
        zero.target_w = 0;
        zero.target_h = 0;
        zero.flags = 0;
        match self.query_natural_only(id, render, data, &zero, ext_ptr) {
            Some(v) => Some(v),
            None => {
                let mut unlimited = zero;
                unlimited.budget_bytes = u32::MAX;
                self.query_natural_only(id, render, data, &unlimited, ext_ptr)
            }
        }
    }

    /// 只问一次"原图多大"：跑一次 `out = NULL` 的查询，且**不带** `REQ_ALLOW_SCALED`。
    ///
    /// 为什么非得多问一次：启用原生降采样后，解码器回填的 `actual_w/h` 是**档位**尺寸
    /// （JPEG DCT 1/2 会把 6055² 报成 3027²），而 `Decoded::natural` 的契约是**原图**
    /// 尺寸——状态栏尺寸与 `viewer::tick` 的"是否已是整图"升级闸都吃它。报小了的后果
    /// 是 `decoded >= natural` 误判成立，那次原图档升级**永不发生**，放大到 1:1 就永久
    /// 糊在一张档位图上（m35）。
    ///
    /// 代价只是一次头读（JPEG 解析 SOF，微秒级），且只有声明 `CAP_SCALED` 的解码器才走
    /// （今天仅 jpeg 一枚；其余格式这条分支根本不进）。
    ///
    /// 失败一律降级为 `None`，绝不把图判死：少一次查询只是退回"natural = 档位尺寸"的
    /// 旧行为，比"打不开"轻得多。
    pub(super) fn query_natural_only(
        &mut self,
        id: &str,
        render: unsafe extern "C" fn(
            *const u8,
            usize,
            *const IvRenderReq,
            *mut u8,
            usize,
            *mut IvRenderOut,
            *const c_char,
        ) -> i32,
        data: &[u8],
        req: &IvRenderReq,
        ext_ptr: *const c_char,
    ) -> Option<(u32, u32)> {
        let mut info = IvRenderOut::default();
        let r = invoke(|| unsafe {
            render(
                data.as_ptr(),
                data.len(),
                req,
                std::ptr::null_mut(),
                0,
                std::ptr::addr_of_mut!(info),
                ext_ptr,
            )
        });
        match r {
            Ok(ERR_OK) if info.actual_w > 0 && info.actual_h > 0 => {
                Some((info.actual_w, info.actual_h))
            }
            Ok(code) => {
                if code == ERR_PANIC {
                    self.penalize(id);
                }
                log::debug!("补查原图尺寸失败（code={code}）：natural 退回档位尺寸");
                None
            }
            Err(e) => {
                self.charge_fault(id, e.as_ref());
                None
            }
        }
    }
}

// ── 区域渲染（ABI v6）────────────────────────────────────────────────

/// 区域请求的**画布尺寸**：区域（图像像素）× 密度（设备像素/图像像素）。
///
/// 返回 `(画布宽, 画布高, 实际密度)`；实际密度可能被硬顶压低（降密度而不是失败）。
///
/// ★ 这条算式就是"矢量图放大到 4000% 也无损"的**宿主侧保证**：
/// 宿主给的区域是**视口那一小块**（放大时区域在图像坐标里越来越小），
/// 画布 = 区域 × 密度 ≈ **一屏大小** ⇒ 像素预算恒定，倍率再高也顶得住；
/// 而解码器是按那个密度**重新光栅化**的，不是位图插值放大。
pub(super) fn region_target(size: (f32, f32), density: f32) -> (u32, u32, f32) {
    let mut density = density.max(f32::EPSILON);
    let (mut tw, mut th) = (
        (size.0 * density).round().max(1.0) as u32,
        (size.1 * density).round().max(1.0) as u32,
    );
    let mx = tw.max(th) as f64 / MAX_EDGE as f64;
    if mx > 1.0 {
        let k = (1.0 / mx) as f32;
        density *= k;
        tw = (tw as f32 * k).round().max(1.0) as u32;
        th = (th as f32 * k).round().max(1.0) as u32;
    }
    let px = tw as u64 * th as u64;
    if px > MAX_PIXELS {
        let k = (MAX_PIXELS as f64 / px as f64).sqrt() as f32;
        density *= k;
        tw = (tw as f32 * k).floor().max(1.0) as u32;
        th = (th as f32 * k).floor().max(1.0) as u32;
    }
    (tw, th, density)
}

/// 区域渲染的一次完整往返：**查询**（`out = NULL`）→ 按 `bytes_needed` 分配 → **写入**。
///
/// 抽成自由函数是为了能**不用真 DLL** 就测：`IvRenderRegionFn` 只是一个函数指针，
/// 测试可以塞一个假的进去，把"两阶段顺序 / 尺寸回填 / 像素归一"这几条钉住
/// （见本文件末尾的 `region_roundtrip_*` 用例）。
///
/// 失败一律返回人话（`Err`），由调用方决定是换下一枚解码器还是交给上层。
pub(super) fn region_roundtrip(
    f: crate::core::abi::IvRenderRegionFn,
    last_error: unsafe extern "C" fn(*mut std::ffi::c_char, usize) -> i32,
    data: &[u8],
    req: &crate::core::abi::IvRegionReq,
    ext: Option<&std::ffi::CString>,
) -> Result<(Bitmap, u32, u32, Vec<u8>, IvRenderOut), String> {
    // ① 查询阶段：out = NULL（不写像素），拿画布尺寸与所需字节数。
    let mut info = IvRenderOut::default();
    let q = invoke(|| unsafe {
        f(
            data.as_ptr(),
            data.len(),
            std::ptr::from_ref(req),
            std::ptr::null_mut(),
            0,
            std::ptr::addr_of_mut!(info),
            ext.map_or(std::ptr::null(), |c| c.as_ptr()),
        )
    });
    match q {
        Ok(ERR_OK) => {}
        Ok(code) => {
            return Err(format!(
                "区域渲染查询失败（code={code}）：{}",
                last_error_msg(last_error)
            ))
        }
        Err(_) => return Err("区域渲染时出错（解码器崩溃或访问违规）".into()),
    }
    let (aw, ah) = (info.actual_w, info.actual_h);
    if aw == 0 || ah == 0 {
        return Err("区域渲染返回了 0 尺寸".into());
    }
    // 查询回报不可信：与常规解码同一道总预算闸，必须早于分段与分配。
    let px = aw as u64 * ah as u64;
    if px > MAX_PIXELS || info.bytes_needed > MAX_PIXELS * 4 {
        return Err(too_large_msg(aw, ah));
    }
    let (pix_len, icc_len) = out_segments(&info)?;
    // ② 渲染阶段：宿主按契约分配 `bytes_needed + icc_len`（缓冲由宿主分配，解码器只写）。
    let mut buf = vec![0u8; pix_len + icc_len];
    let r = invoke(|| unsafe {
        f(
            data.as_ptr(),
            data.len(),
            std::ptr::from_ref(req),
            buf.as_mut_ptr(),
            buf.len(),
            std::ptr::addr_of_mut!(info),
            ext.map_or(std::ptr::null(), |c| c.as_ptr()),
        )
    });
    match r {
        Ok(ERR_OK) => {}
        Ok(code) => {
            return Err(format!(
                "区域渲染失败（code={code}）：{}",
                last_error_msg(last_error)
            ))
        }
        Err(_) => return Err("区域渲染时出错（解码器崩溃或访问违规）".into()),
    }
    let icc = take_icc(&mut buf, pix_len, icc_len);
    let rgba = to_rgba8(buf, aw, ah, info.pixel_format, info.bit_depth)?;
    Ok((
        Bitmap {
            width: aw,
            height: ah,
            rgba,
        },
        aw,
        ah,
        icc,
        // ★ 播放元信息一并带出去：区域重渲也是同一次 `iv_render_region`，
        //   解码器照样回填 `page_count` / `frame_count`。丢掉它，多页文档
        //   一放大就退化成单页——那正是"翻页按钮翻两下就没了"的来历。
        info,
    ))
}
