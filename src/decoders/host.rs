//! 解码器注册表：扫目录、懒加载、会话黑名单与三振出局。
//!
//! 与 [`super::decode`] 同属一个 `impl Host`，只是按「加载」与「调用」分文件；
//! 字段因此是 `pub(super)` 的——跨模块的同一类型，私有字段兄弟看不见。
use super::dllmeta::*;
use super::select::*;
use super::*;

/// 解码器注册表 + 加载缓存 + 会话黑名单。**只在解码线程里用它**（`iv_last_error` 是线程局部的）。
pub struct Host {
    /// 目录里的 DLL（`路径 → 已加载解码器`）。懒加载：第一次需要时才扫。
    pub(super) loaded: HashMap<PathBuf, Loaded>,
    /// 路径 → 加载失败原因（失败过就不在本次扫描周期里重试）。
    pub(super) failed: HashMap<PathBuf, String>,
    /// 该目录对应的热加载代次。
    pub(super) catalog_gen: u64,
    /// id → 连续 panic 次数。
    pub(super) strikes: HashMap<String, u32>,
    /// 会话黑名单：三振出局。
    pub(super) blacklist: HashSet<String>,
    /// 本次请求是不是**后台预取**（由 `core::decode::decode_one` 每次请求置位）。
    ///
    /// 只影响一件事：排队拿 DLL 调用权时站哪一档（见 `dll_lock::Class`）。预取不许挡在
    /// 用户正等的那一笔前面。用状态位而不是加参数，是为了不改
    /// `DecodeBackend::decode` 那条每层都要跟一遍的签名链。
    pub(super) bg: bool,
}

impl Host {
    pub fn new() -> Self {
        // 解码线程是格式清单的天然装入点（幂等）：即使 GUI 入口漏装，
        // 首张图解码前后 gallery 的查询也已就位。
        install_format_source();
        Self {
            loaded: HashMap::new(),
            failed: HashMap::new(),
            // 0 保证第一次一定触发扫描。
            catalog_gen: 0,
            strikes: HashMap::new(),
            blacklist: HashSet::new(),
            bg: false,
        }
    }

    /// 本次请求排队时站哪一档（见 [`Host::bg`] 与 `dll_lock::Class`）。
    pub(super) fn class(&self) -> super::dll_lock::Class {
        if self.bg {
            super::dll_lock::Class::Background
        } else {
            super::dll_lock::Class::Urgent
        }
    }

    /// 按需（懒加载 / 代次变化时）扫 `decoders/` 下的 `*.dll` 并加载。
    ///
    /// 目录内容变了（`request_rescan` 或首次）才重扫；仍在目录里的 DLL 沿用缓存。
    /// **本函数不卸库**：目录里没了的、以及被同 id 更高版本取代的，一律置 `active=false`
    /// 留在表里（理由见 [`Loaded::active`]）——宿主不在会话中途 `FreeLibrary`。
    /// 失败缓存清空以便先前失败的 DLL 有机会重试；会话黑名单不复位。
    pub(super) fn refresh_catalog(&mut self) {
        let gen = rescan_gen();
        if self.catalog_gen == gen {
            return;
        }
        self.catalog_gen = gen;
        self.failed.clear();

        let Some(dir) = catalog::dir() else {
            return;
        };
        // 读不动目录（网络盘瞬断、U 盘被拔、权限、正被枚举）= **沿用现有清单**：
        // 一次 I/O 失败就把全部解码器判成"已不在"，所有格式会静默变成"无人认领"，
        // 比继续用旧清单坏得多（同一条口径见 `charge_fault` 区分媒体故障）。
        let Ok(rd) = std::fs::read_dir(&dir) else {
            log::warn!("读解码器目录失败，本轮沿用现有清单：{}", dir.display());
            return;
        };
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .map(|x| x.eq_ignore_ascii_case("dll"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        let present: HashSet<PathBuf> = paths.iter().cloned().collect();
        // 换目录（`RUPIX_DECODERS_DIR` 指走）也算"不在了"；又回来的恢复可用。
        for (p, l) in self.loaded.iter_mut() {
            let here = p.parent() == Some(dir.as_path()) && present.contains(p);
            if l.active != here {
                log::info!(
                    "解码器 {} {}",
                    p.display(),
                    if here {
                        "重新出现在目录里，恢复选用"
                    } else {
                        "已不在目录里，本次会话不再选用"
                    }
                );
                l.active = here;
            }
        }

        for p in paths {
            if self.loaded.contains_key(&p) {
                continue;
            }
            match inspect(&p) {
                Inspect::Ok { desc, lib, sym } => {
                    log::info!("已加载解码器：{}（{}）", desc.display_name, desc.id);
                    self.loaded.insert(
                        p,
                        Loaded {
                            desc,
                            active: true,
                            _lib: Some(lib),
                            sym,
                        },
                    );
                }
                Inspect::AbiMismatch { got } => {
                    log::warn!(
                        "解码器 {} 版本不匹配：解码器 abi={got}，宿主接受区间 [{MIN_ABI_VERSION}, {IV_ABI_VERSION}]",
                        p.display()
                    );
                    self.failed.insert(
                        p,
                        format!(
                            "版本不匹配（解码器 abi={got}，宿主接受 {MIN_ABI_VERSION}..{IV_ABI_VERSION}）"
                        ),
                    );
                }
                Inspect::Skipped => {
                    log::warn!(
                        "解码器 {} 上次加载崩溃，本次跳过（在设置 → 解码器中清除标记后可重试）",
                        p.display()
                    );
                    self.failed.insert(
                        p,
                        "上次加载崩溃，本次已跳过（清除标记后可重试）".to_string(),
                    );
                }
                Inspect::Error(e) => {
                    log::warn!("加载解码器 {} 失败：{e}", p.display());
                    self.failed.insert(p, e);
                }
            }
        }

        // 只在**当前可用**的集合里选版本：损坏的新包不能挤掉可用旧包。
        // metadata 缺席的旧 DLL 仍用既有文件名版本规则；同版本按路径确定性取一份。
        let mut versions: Vec<_> = self
            .loaded
            .iter()
            .filter(|(_, loaded)| loaded.active)
            .map(|(path, loaded)| {
                let version = loaded
                    .desc
                    .metadata
                    .iter()
                    .find(|(key, value)| key == "version" && !value.trim().is_empty())
                    .map(|(_, value)| value.clone())
                    .or_else(|| {
                        path.file_name()
                            .and_then(|s| s.to_str())
                            .and_then(parse_dll_version)
                    })
                    .unwrap_or_default();
                (path.clone(), loaded.desc.id.clone(), version)
            })
            .collect();
        versions.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| version_cmp(&b.2, &a.2))
                .then_with(|| a.0.cmp(&b.0))
        });
        let mut ids = HashSet::new();
        let winners: HashSet<_> = versions
            .into_iter()
            .filter_map(|(path, id, _)| ids.insert(id).then_some(path))
            .collect();
        // 不依赖磁盘删除成功：旧文件即使仍被别的句柄锁住，也不再进入任何候选路径——
        // 置停用而不是摘条目（摘了就等于在会话中途卸库）。
        for (path, loaded) in self.loaded.iter_mut() {
            if loaded.active && !winners.contains(path) {
                loaded.active = false;
                log::info!(
                    "解码器 {} 已被同 id 的更高版本取代，本次会话不再选用",
                    path.display()
                );
            }
        }
    }

    /// 找一个认领该扩展名、且未被用户关闭/未拉黑的解码器。
    ///
    /// `require_caps` = **只要声明了这些解码类型的解码器**（`abi::CAP_*` 位图，全部满足
    /// 才入围）。裸数据参数化请求传 `CAP_RAW_PARAM`；这是宿主解决"一个扩展名多个解码器
    /// 认领"的手段，见 [`select_candidate`] 的说明。传 `0` = 无要求（今天的常规路径）。
    ///
    /// # 用户显式首选先行（稀疏覆盖，非全量清单）
    ///
    /// 设置页「解码」页按后缀指定的首选（`decoders_preferred`）优先于自动仲裁：命中
    /// 且仍可用（认领该后缀、未停用、未拉黑、满足 `require_caps`）就定它；条目失效
    /// （解码器卸载/停用/拉黑）**静默回退自动**。这与 [`select_candidate`] 注释里反对的
    /// "全量 ext→id 优先级表"不是一回事：那张表要求覆盖每个后缀、取代自动路由，
    /// 删掉/改名一个解码器就悄悄失效；这张表只覆盖用户点过的后缀，失效条目落回的路
    /// （路径字典序）正是它想覆盖的那条默认路——覆盖不掉就当没设过，不会错。
    pub(super) fn candidate_for_ext(&mut self, ext: &str, require_caps: u32) -> Option<PathBuf> {
        self.refresh_catalog();
        // 路径排序后遍历，保证同一个目录总是同一种选择（字典序，跨平台一致）。
        // ★ m102：排的是**引用的序**，不是路径的副本；`Cand` 也不再抄走 `id` 与
        //   `extensions`。判据一字未动，只是不再为每次选解码器抄一整排：一次路由的堆
        //   分配 133 → 3、停用锁 12 把 → 1 把（配对读数见 [`Cand`] 的 m102 注）。
        let mut keys: Vec<&PathBuf> = self
            .loaded
            .iter()
            .filter(|(_, l)| l.active)
            .map(|(k, _)| k)
            .collect();
        keys.sort();
        let cands: Vec<Cand<'_>> = {
            // 停用集**一次锁**读完（旧写法每枚解码器锁一次，而这条路每张图、每枚缩略图、
            // 每帧动画都要走一遍）。作用域收在这里：下面 `preferred_for` 要拿另一把锁。
            let disabled = disabled_guard();
            keys.iter()
                .map(|k| {
                    let l = &self.loaded[k.as_path()];
                    Cand {
                        key: k,
                        id: &l.desc.id,
                        extensions: &l.desc.extensions,
                        disabled: disabled.contains(&l.desc.id),
                        blacklisted: self.blacklist.contains(&l.desc.id),
                        caps: l.desc.caps,
                    }
                })
                .collect()
        };
        // 用户显式首选先行（见上）；没设过或已失效 → 自动仲裁。
        let pref = preferred_for(ext);
        if let Some(c) = preferred_candidate(ext, require_caps, pref.as_deref(), &cands) {
            return Some(c.key.clone());
        }
        select_candidate(ext, &cands, require_caps).map(|c| c.key.clone())
    }

    /// 记一次解码器故障。**三振是"连续"**：中间任何一次成功解码都会把计数清掉
    /// （[`Host::clear_strikes`]），口径与文档、与日志里那句"连续 N 次"一致（审计 V10）。
    pub(super) fn penalize(&mut self, id: &str) {
        let n = self.strikes.entry(id.to_string()).or_insert(0);
        *n += 1;
        if *n >= MAX_STRIKES {
            log::error!("解码器「{id}」连续 {n} 次出错，本次会话拉黑");
            self.blacklist.insert(id.to_string());
        }
    }

    /// 一次成功解码 = "连续"链断开。不复位的话，坏图一次动作烧掉 2 次（`sniff_ranked`
    /// 嗅一次、`decode_with` 解一次），累计到 3 就把一个平时好好的解码器整场会话拉黑，
    /// 那个格式从此静默"无人认领"，而且**不重启不解黑**（审计 V10）。
    pub(super) fn clear_strikes(&mut self, id: &str) {
        self.strikes.remove(id);
    }

    /// FFI 边界抛回的错误该怎么记账：**只有解码器自己的故障才三振**。
    ///
    /// 分类来自 `platform::seh`：`EXCEPTION_IN_PAGE_ERROR`（网络盘掉线、U 盘被拔、文件
    /// 被截断）不是解码器的错，只留一条 warn（审计 V8）。
    pub(super) fn charge_fault(&mut self, id: &str, err: &(dyn std::any::Any + Send)) {
        if crate::platform::seh::is_media_fault(err) {
            log::warn!(
                "解码器「{id}」读到半途介质出错（网络盘掉线 / U 盘被拔 / 文件被截断），不计解码器故障"
            );
        } else {
            self.penalize(id);
        }
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}
