//! 非解码器扩展的纯数据模型：manifest、能力、类型。
//!
//! 本模块只做解析与校验，不碰文件系统、Win32、UI，也不启动进程；单测可无头跑。
//! 扩展宿主（`crate::extensions`）负责目录发现与运行期编排。

use std::path::{Component, Path};

use crate::core::hotkey::{Hotkey, KeyInput};
use crate::core::json::{self, Value};

/// manifest 结构版本（安装期）。与运行期协议版本分开。
pub const SCHEMA_VERSION: u32 = 1;

/// 扩展协议**下界**（运行期）。旧插件兼容的地板，**永不抬**。
///
/// ★ 与解码器 ABI 同一条立约：宿主升级后，已安装的旧插件不需要重新编译/重装。
///   新协议只能**加法式**演进（新增可选字段 / 新 action），不能改既有字段语义。
pub const MIN_PROTOCOL_VERSION: u32 = 1;

/// 扩展协议**最新版**（运行期）。宿主收 `MIN..=本值`；只增不减。
pub const PROTOCOL_VERSION: u32 = 1;

/// 版本区间判定：宿主能跑声明的这个协议版本吗？
pub fn protocol_ok(protocol: u32) -> bool {
    (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&protocol)
}

/// 给用户看的支持区间（错误信息 / 信息窗共用，别各写一份）。
pub fn protocol_range_text() -> String {
    if MIN_PROTOCOL_VERSION == PROTOCOL_VERSION {
        format!("v{PROTOCOL_VERSION}")
    } else {
        format!("v{MIN_PROTOCOL_VERSION}–v{PROTOCOL_VERSION}")
    }
}

/// 响应超时缺省值（毫秒）：命令型插件 30s。长任务插件在 manifest 里声明 `timeout_ms` 放宽。
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// 响应超时上限（毫秒，30 分钟）：宿主是桌面软件，不是服务端——
/// 一个插件不能把宿主钉在"等它"上等半天。上限写死，manifest 超过就是坏清单。
pub const MAX_TIMEOUT_MS: u64 = 1_800_000;

/// 宿主自身版本（`min_host` 的比较对象；比较函数在 `extensions::remote::version_cmp`）。
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 扩展类型：命令 / 编辑 / 面板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 无宿主 UI，拿当前图片路径/参数，返回文本或错误。
    Command,
    /// 图片进、图片出（涂鸦等）。
    Editor,
    /// 宿主托管的只读面板（后续）。
    Panel,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "command" => Some(Self::Command),
            "editor" => Some(Self::Editor),
            "panel" => Some(Self::Panel),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Editor => "editor",
            Self::Panel => "panel",
        }
    }
}

/// 能力声明。宿主运行时只开放这里列出的能力；未知能力在解析期拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    ImageRead,
    ImageWrite,
    SelectionRead,
    ClipboardWrite,
    FileRead,
    FileWrite,
    Network,
    UiWindow,
    UiPanel,
    SettingsRead,
    SettingsWrite,
    ProcessSpawn,
    /// 让宿主用系统默认程序打开解码出的网址（只认 http/https）。
    OpenUrl,
    /// 让宿主把解码出的图片写进 `<exe>/temp/` 再用系统默认程序打开。
    OpenFile,
}

impl Capability {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "image.read" => Some(Self::ImageRead),
            "image.write" => Some(Self::ImageWrite),
            "selection.read" => Some(Self::SelectionRead),
            "clipboard.write" => Some(Self::ClipboardWrite),
            "file.read" => Some(Self::FileRead),
            "file.write" => Some(Self::FileWrite),
            "network" => Some(Self::Network),
            "ui.window" => Some(Self::UiWindow),
            "ui.panel" => Some(Self::UiPanel),
            "settings.read" => Some(Self::SettingsRead),
            "settings.write" => Some(Self::SettingsWrite),
            "process.spawn" => Some(Self::ProcessSpawn),
            "open.url" => Some(Self::OpenUrl),
            "open.file" => Some(Self::OpenFile),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImageRead => "image.read",
            Self::ImageWrite => "image.write",
            Self::SelectionRead => "selection.read",
            Self::ClipboardWrite => "clipboard.write",
            Self::FileRead => "file.read",
            Self::FileWrite => "file.write",
            Self::Network => "network",
            Self::UiWindow => "ui.window",
            Self::UiPanel => "ui.panel",
            Self::SettingsRead => "settings.read",
            Self::SettingsWrite => "settings.write",
            Self::ProcessSpawn => "process.spawn",
            Self::OpenUrl => "open.url",
            Self::OpenFile => "open.file",
        }
    }

    /// **宿主是否真的为这项能力开通道**。
    ///
    /// ★ 这条判据是能力表诚实性的锚：能力表分两类，插件作者必须分得清——
    ///
    /// | 类别 | 能力 | 含义 |
    /// |---|---|---|
    /// | **宿主开通道** | `image.read` `image.write` `clipboard.write` `settings.read` `settings.write` `open.url` `open.file` | 宿主在编排层真的把这个能力接给它：发图、回写图、写剪贴板、读写它的设置区 |
    /// | **仅告知** | `network` `process.spawn` `ui.window` `file.read` `file.write` `selection.read` `ui.panel` | 宿主**没有**对应通道；声明它只是让用户在首次同意页上看见"这个插件会自己干这些事"，出了事宿主不背书也不兜底 |
    ///
    /// 为什么必须分清：能力表早先把 12 项平铺在一张表里，读起来像"宿主都支持"，
    /// 实际只有图片两项有入口——插件作者照着声明 `clipboard.write` 却在宿主这头
    /// 等一个永远不会来的剪贴板。**声明不是承诺，宿主提供通道才是承诺。**
    pub fn host_provides(self) -> bool {
        matches!(
            self,
            Self::ImageRead
                | Self::ImageWrite
                | Self::ClipboardWrite
                | Self::SettingsRead
                | Self::SettingsWrite
                | Self::OpenUrl
                | Self::OpenFile
        )
    }
}

/// 设置项的类型：决定宿主用哪种控件渲染、默认值怎么校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    Choice,
    Int,
    Text,
    /// 一把全局快捷键。★ 与 `manifest.hotkey` 是同一件事的两种写法：
    /// 声明了 `type=hotkey` 的项就**不许**再写 `manifest.hotkey`（两个真源）。
    Hotkey,
}

/// 一条设置项的**声明**（manifest `settings` 数组里的一格）。
///
/// 为什么由清单声明、宿主渲染，而不是插件自己画一个对话框：
/// 命令型插件没有自己的窗口，而让每个插件各画一遍对话框，等于把"校验 / 落盘 /
/// 快捷键捕获"各写五遍。声明式 schema 让宿主统一渲染，插件只管读值
/// （参考实现 CurSeen 的脚本 `schema()` 是同一层想法）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingSpec {
    pub key: String,
    pub label: String,
    pub kind: SettingKind,
    /// 默认值。一律按**字符串**存：设置区是扁平字符串键值对（`Settings`）。
    pub default: String,
    /// 仅 `Choice`：可选值。
    pub choices: Vec<String>,
    /// 仅 `Choice`：给人看的标签（与 `choices` 一一对应；空 = 直接显示取值）。
    pub labels: Vec<String>,
}

impl SettingSpec {
    /// 下拉框要显示的 `(取值, 文案)` 列表。
    pub fn display_choices(&self) -> Vec<(String, String)> {
        self.choices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let label = self.labels.get(i).cloned().unwrap_or_else(|| v.clone());
                (v.clone(), label)
            })
            .collect()
    }
}

/// 解析 `settings` 数组。缺省 = 没有 schema（插件自己画窗，走 `settings_ui` 那条老路）。
fn parse_settings(root: &Value) -> Result<Vec<SettingSpec>, String> {
    let arr = match root.get("settings") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Arr(a)) => a,
        Some(_) => return Err("settings 必须是数组".into()),
    };
    let mut out: Vec<SettingSpec> = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let at = |m: String| format!("settings[{i}]：{m}");
        if !matches!(item, Value::Obj(_)) {
            return Err(at("每一格必须是对象".into()));
        }
        let key = get_str(item, "key").map_err(at)?;
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(at(format!("key=\"{key}\" 只能用字母/数字/._-")));
        }
        if out.iter().any(|s| s.key == key) {
            return Err(at(format!("key=\"{key}\" 重复")));
        }
        let label = get_str(item, "label").map_err(at)?;
        if label.is_empty() {
            return Err(at("label 不能为空".into()));
        }
        let kind_text = get_str(item, "type").map_err(at)?;
        let (kind, choices, labels) = match kind_text.as_str() {
            "bool" => (SettingKind::Bool, Vec::new(), Vec::new()),
            "int" => (SettingKind::Int, Vec::new(), Vec::new()),
            "text" => (SettingKind::Text, Vec::new(), Vec::new()),
            "hotkey" => (SettingKind::Hotkey, Vec::new(), Vec::new()),
            "choice" => {
                let choices = get_str_array(item, "choices").map_err(at)?;
                if choices.len() < 2 {
                    return Err(at("choice 至少要两个可选值".into()));
                }
                let labels = match item.get("labels") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(_) => get_str_array(item, "labels").map_err(at)?,
                };
                if !labels.is_empty() && labels.len() != choices.len() {
                    return Err(at("labels 与 choices 数量必须一致".into()));
                }
                (SettingKind::Choice, choices, labels)
            }
            other => {
                return Err(at(format!(
                    "type=\"{other}\" 不认识（bool / choice / int / text / hotkey）"
                )))
            }
        };
        let default = normalize_default(item, kind, &choices).map_err(at)?;
        out.push(SettingSpec {
            key,
            label,
            kind,
            default,
            choices,
            labels,
        });
    }
    Ok(out)
}

/// 默认值归一：一律落成字符串，且**当场按类型验一遍**——一个装不进控件的默认值
/// 会让设置窗第一次打开就崩在渲染层，而在解析期拒绝只是一条清单错误。
fn normalize_default(
    item: &Value,
    kind: SettingKind,
    choices: &[String],
) -> Result<String, String> {
    fn as_text(v: &Value) -> Option<String> {
        match v {
            Value::Str(s) => Some(s.clone()),
            Value::Bool(b) => Some(if *b { "1" } else { "0" }.to_string()),
            Value::Num(n) if n.fract() == 0.0 => Some(format!("{}", *n as i64)),
            _ => None,
        }
    }
    let fallback = || match kind {
        SettingKind::Choice => choices.first().cloned().unwrap_or_default(),
        // 布尔与整数的"没写默认值"都该落在"关 / 零"，而不是空串——空串在控件里是第三态。
        SettingKind::Bool | SettingKind::Int => "0".to_string(),
        _ => String::new(),
    };
    let Some(raw) = item.get("default") else {
        return Ok(fallback());
    };
    if matches!(raw, Value::Null) {
        return Ok(fallback());
    }
    let text = as_text(raw).ok_or_else(|| "default 的类型不对".to_string())?;
    match kind {
        SettingKind::Bool => match text.to_ascii_lowercase().as_str() {
            "1" | "true" => Ok("1".into()),
            "0" | "false" => Ok("0".into()),
            _ => Err(format!("bool 的 default=\"{text}\" 不可识别")),
        },
        SettingKind::Int => text
            .parse::<i64>()
            .map(|n| n.to_string())
            .map_err(|_| format!("int 的 default=\"{text}\" 不是整数")),
        SettingKind::Choice => {
            if choices.iter().any(|c| c == &text) {
                Ok(text)
            } else {
                Err(format!("choice 的 default=\"{text}\" 不在 choices 里"))
            }
        }
        SettingKind::Text => Ok(text),
        SettingKind::Hotkey => {
            if text.is_empty() || Hotkey::from_spec(&text).is_some() {
                Ok(text)
            } else {
                Err(format!("hotkey 的 default=\"{text}\" 不是宿主认得的键串"))
            }
        }
    }
}

/// 一个扩展的 `manifest.json`。字段名与扩展开发文档一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub min_host: String,
    pub kind: Kind,
    /// 扩展目录内的相对入口；当前只接受 `.exe`。
    pub entry: String,
    pub args: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub platforms: Vec<String>,
    pub description: String,
    pub author: String,
    /// 许可证标识（可选；目录清单里也有一份，用于未安装时的详情窗）。
    pub license: String,
    /// 发行日期 `YYYY-MM-DD`（可选；仅展示）。
    pub released: String,
    /// 响应超时（毫秒，可选）：缺省 [`DEFAULT_TIMEOUT_MS`]，上限 [`MAX_TIMEOUT_MS`]。
    ///
    /// 为什么要有它：OCR/批处理这类插件天生比 30s 慢，而宿主不能替插件猜它要多久——
    /// 猜短了插件被腰斩，猜长了用户干等。**插件自己声明，宿主只负责封顶。**
    pub timeout_ms: Option<u64>,
    /// 动态热键（可选）：插件自己声明的快捷键，按下就跑这个插件。
    ///
    /// 键串用宿主那套规范写法（`Ctrl+Shift+O`、`F8`、`NumAdd`…），解析器与内置
    /// 快捷键**同一个**（[`Hotkey::from_spec`]）——两套写法迟早会漂。
    ///
    /// 冲突裁决（宿主侧定，插件无权）：**内置快捷键优先**——用户自己配的键位不该
    /// 被插件抢走；两个插件抢同一把键，按插件 id 排序先来先得，后来者这条热键不生效。
    pub hotkey: Option<Hotkey>,
    /// 插件是否自带设置窗口（可选，默认 false）。
    ///
    /// 设置区存储早就在（`plugins/<id>/settings.json`），缺的是"用户从哪里改"：
    /// 命令型插件没有自己的窗口，设置便无处可去。声明它之后，宿主在插件卡上
    /// 给一颗齿轮，点下去以 `action=settings` 拉起插件、由插件自己画界面，
    /// 回传的设置照旧合并落盘（见 `docs/扩展体系.md` §五）。
    ///
    /// 为什么必须同时声明 `settings.write`：只有读没有写，改完存不下来——
    /// 那样的 `settings_ui=true` 是坏清单，解析期就拒。
    pub settings_ui: bool,
    /// 声明式设置项（可选）。给了它，宿主就**自己渲染**设置窗（齿轮 → 宿主窗口），
    /// 不再以 `action=settings` 拉起插件；`settings_ui` 那条老路留给没有 schema 的插件。
    ///
    /// 键值存在 `plugins/<id>/settings.json` 里，每次 `run` 随请求下发。
    /// `type=hotkey` 的那一项同时是插件的动态热键（见 `extensions::build_hotkey_table`）。
    pub settings: Vec<SettingSpec>,
}

impl Manifest {
    /// 从扩展目录读 `manifest.json`。
    pub fn from_dir(dir: &Path) -> Result<Self, String> {
        let path = dir.join("manifest.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("读 {} 失败：{e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}：{e}", path.display()))
    }

    /// 解析并校验一份 manifest JSON。
    pub fn parse(text: &str) -> Result<Self, String> {
        // Windows 编辑器可能带 UTF-8 BOM；JSON 解析器不认它，先剥掉再校验。
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let root = json::parse(text).map_err(|e| format!("JSON 解析失败：{e}"))?;
        let schema = get_u32(&root, "schema")?;
        if schema != SCHEMA_VERSION {
            return Err(format!("schema={schema}，宿主只支持 {SCHEMA_VERSION}"));
        }

        let id = get_str(&root, "id")?;
        validate_id(&id)?;

        let name = get_str(&root, "name")?;
        if name.is_empty() {
            return Err("name 不能为空".into());
        }

        let version = get_str(&root, "version")?;
        validate_version(&version, "version")?;

        let protocol = get_u32(&root, "protocol")?;
        if !protocol_ok(protocol) {
            return Err(format!(
                "protocol={protocol}，宿主支持 {}",
                protocol_range_text()
            ));
        }

        let min_host = get_str(&root, "min_host")?;
        validate_version(&min_host, "min_host")?;

        let kind_s = get_str(&root, "kind")?;
        let kind = Kind::parse(&kind_s)
            .ok_or_else(|| format!("kind=\"{kind_s}\" 不认识（command/editor/panel）"))?;

        let entry = get_str(&root, "entry")?;
        validate_entry(&entry, kind)?;

        let args = get_str_array(&root, "args")?;
        let capabilities = get_capabilities(&root)?;
        let platforms = get_str_array(&root, "platforms")?;
        if platforms.is_empty() {
            return Err("platforms 不能为空".into());
        }
        if !platforms.iter().any(|p| p == "windows-x64") {
            return Err("platforms 必须包含 windows-x64".into());
        }

        // 响应超时：可选；超过上限 = 坏清单（而不是"照办"）——
        // 桌面软件没有"等半小时"这种设计，插件也不能替宿主决定它等多久。
        let timeout_ms = match root.get("timeout_ms") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| "timeout_ms 必须是非负整数（毫秒）".to_string())?;
                if n == 0 {
                    return Err("timeout_ms 必须大于 0".into());
                }
                if n > MAX_TIMEOUT_MS {
                    return Err(format!(
                        "timeout_ms={n} 超过上限 {MAX_TIMEOUT_MS}（30 分钟）"
                    ));
                }
                Some(n)
            }
        };

        // 动态热键：可选。给了但写不成宿主认得的键串 = 坏清单（不是"照办"——
        // 一条永远按不出来的热键等于没写，还会让用户以为插件坏了）。
        let hotkey = match root.get("hotkey") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| "hotkey 必须是字符串（键串）".to_string())?;
                let hk = Hotkey::from_spec(s)
                    .ok_or_else(|| format!("hotkey=\"{s}\" 不是宿主认得的键串"))?;
                Some(hk)
            }
        };

        // 设置窗口：可选布尔。只给了 `settings_ui` 却没给写权限 = 改了存不下来，
        // 属于坏清单（不是"降级成只读"——用户改完发现没保存比没有入口更糟）。
        let settings_ui = match root.get("settings_ui") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("settings_ui 必须是布尔值".into()),
        };
        if settings_ui && !capabilities.contains(&Capability::SettingsWrite) {
            return Err("settings_ui=true 必须同时声明 settings.write".into());
        }

        // 声明式设置：可选。给了它就必须能读能写（宿主既要读旧值来填控件，也要存新值）。
        // 与 `settings_ui` 同时出现 = 坏清单：一个说"宿主画"、一个说"插件画"，得挑一个。
        let settings = parse_settings(&root)?;
        if !settings.is_empty() {
            if !capabilities.contains(&Capability::SettingsRead)
                || !capabilities.contains(&Capability::SettingsWrite)
            {
                return Err(
                    "声明了 settings 就必须同时声明 settings.read 与 settings.write".into(),
                );
            }
            if settings_ui {
                return Err("settings（宿主渲染）与 settings_ui（插件自画）只能二选一".into());
            }
        }
        if settings.iter().any(|s| s.kind == SettingKind::Hotkey) && hotkey.is_some() {
            return Err("settings 里已有 type=hotkey 的项，就别再写 manifest.hotkey".into());
        }

        Ok(Self {
            schema,
            id,
            name,
            version,
            protocol,
            min_host,
            kind,
            entry,
            args,
            capabilities,
            platforms,
            settings_ui,
            settings,
            description: optional_str(&root, "description"),
            author: optional_str(&root, "author"),
            license: optional_str(&root, "license"),
            released: optional_str(&root, "released"),
            timeout_ms,
            hotkey,
        })
    }

    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }

    /// 本插件的响应超时（缺省 [`DEFAULT_TIMEOUT_MS`]）。
    ///
    /// 编辑型不用它——编辑是交互任务，用户画多久都行，由「取消」兜底。
    pub fn response_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS))
    }
}

/// 一条**动态热键**：插件在 manifest 里声明的快捷键。
///
/// 与内置快捷键（[`crate::core::hotkey::Action`] 那一套固定枚举）是两回事：
/// 内置键写死在宿主里、用户可在设置页改；动态键由插件自报、宿主运行时收集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHotkey {
    /// 插件 id（宿主内部用它找插件）。
    pub plugin_id: String,
    /// 插件名（给人看的提示语）。
    pub plugin_name: String,
    /// 按下就触发的那把键。
    pub key: Hotkey,
}

/// 动态热键表：当前生效的插件热键集合。
///
/// **只装数据，不做裁决**：谁生效、谁被内置键压住，是宿主组装这张表时定的
/// （见 `extensions::hotkey_table`），这张表只负责"按下这把键该找谁"。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynamicHotkeys {
    pub entries: Vec<PluginHotkey>,
}

impl DynamicHotkeys {
    /// 记一条热键。
    pub fn push(&mut self, entry: PluginHotkey) {
        self.entries.push(entry);
    }

    /// 这把键是否已经被某条动态热键占用。
    pub fn contains_key(&self, h: &Hotkey) -> bool {
        self.entries.iter().any(|e| &e.key == h)
    }

    /// 这次按键命中哪条动态热键。
    pub fn match_plugin(&self, ev: &KeyInput) -> Option<&PluginHotkey> {
        self.entries.iter().find(|e| e.key.matches(ev))
    }

    /// 某个插件的动态热键（展示用）。
    pub fn of_plugin(&self, id: &str) -> Option<&PluginHotkey> {
        self.entries.iter().find(|e| e.plugin_id == id)
    }
}

fn get_str(root: &Value, key: &str) -> Result<String, String> {
    root.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("缺少字符串字段 `{key}`"))
}

fn optional_str(root: &Value, key: &str) -> String {
    root.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn get_u32(root: &Value, key: &str) -> Result<u32, String> {
    let n = root
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("缺少非负整数字段 `{key}`"))?;
    u32::try_from(n).map_err(|_| format!("`{key}` 超出 u32"))
}

fn get_str_array(root: &Value, key: &str) -> Result<Vec<String>, String> {
    match root.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| format!("`{key}` 必须是字符串数组"))?;
            arr.iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("`{key}` 里只能放字符串"))
                })
                .collect()
        }
    }
}

fn get_capabilities(root: &Value) -> Result<Vec<Capability>, String> {
    let names = get_str_array(root, "capabilities")?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let cap = Capability::parse(&name).ok_or_else(|| format!("未知能力 `{name}`"))?;
        if !out.contains(&cap) {
            out.push(cap);
        }
    }
    Ok(out)
}

pub(crate) fn validate_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if !(3..=64).contains(&bytes.len()) {
        return Err("id 长度必须是 3..=64".into());
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return Err("id 必须以字母或数字开头".into());
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'.' | b'_' | b'-'))
    {
        return Err("id 只允许 ASCII 字母/数字/点/下划线/短横线".into());
    }
    Ok(())
}

fn validate_version(s: &str, field: &str) -> Result<(), String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(format!("`{field}` 必须是数字点分版本"));
    }
    if s.starts_with('.') || s.ends_with('.') || s.contains("..") {
        return Err(format!("`{field}` 版本点分不合法"));
    }
    Ok(())
}

fn validate_entry(entry: &str, kind: Kind) -> Result<(), String> {
    if entry.is_empty() {
        return Err("entry 不能为空".into());
    }
    let p = Path::new(entry);
    if p.is_absolute() {
        return Err("entry 必须是扩展目录内的相对路径".into());
    }
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("entry 不许含 .. / 根 / 盘符".into());
    }
    if kind != Kind::Panel && !entry.to_ascii_lowercase().ends_with(".exe") {
        return Err("entry 必须是 .exe".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> &'static str {
        r#"{
            "schema": 1,
            "id": "com.example.ocr",
            "name": "OCR",
            "version": "1.0.0",
            "protocol": 1,
            "min_host": "0.2.0",
            "kind": "command",
            "entry": "ocr.exe",
            "capabilities": ["image.read", "clipboard.write"],
            "platforms": ["windows-x64"],
            "description": "local OCR",
            "author": "example"
        }"#
    }

    #[test]
    fn parses_minimal_manifest() {
        let m = Manifest::parse(minimal()).expect("valid manifest");
        assert_eq!(m.id, "com.example.ocr");
        assert_eq!(m.kind, Kind::Command);
        assert!(m.has_capability(Capability::ImageRead));
        assert!(!m.has_capability(Capability::Network));
        assert_eq!(m.args, Vec::<String>::new());
    }

    #[test]
    fn bom_prefixed_manifest_parses() {
        let text = format!("\u{feff}{}", minimal());
        let m = Manifest::parse(&text).expect("BOM 不应让 manifest 失效");
        assert_eq!(m.id, "com.example.ocr");
    }

    #[test]
    fn rejects_bad_id_and_entry() {
        let mut bad = minimal().replace("com.example.ocr", "../evil");
        assert!(Manifest::parse(&bad).is_err());

        bad = minimal().replace("\"ocr.exe\"", "\"../ocr.exe\"");
        assert!(Manifest::parse(&bad).is_err());

        bad = minimal().replace("\"ocr.exe\"", "\"ocr.dll\"");
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn rejects_unknown_capability_and_protocol() {
        let bad = minimal().replace("\"clipboard.write\"", "\"clipboard.write\", \"telepathy\"");
        assert!(Manifest::parse(&bad).is_err());

        let bad = minimal().replace("\"protocol\": 1", "\"protocol\": 2");
        assert!(Manifest::parse(&bad).is_err());
    }

    /// 协议是**区间**兼容（2026-10-29）：下界永不抬，旧插件继续可跑；
    /// 只有高于宿主最新版的插件才拒。与解码器 ABI 的同一条立约。
    #[test]
    fn protocol_is_a_backward_compatible_range() {
        assert_eq!(MIN_PROTOCOL_VERSION, 1);
        assert!(protocol_ok(MIN_PROTOCOL_VERSION));
        assert!(protocol_ok(PROTOCOL_VERSION));
        assert!(!protocol_ok(MIN_PROTOCOL_VERSION - 1));
        assert!(!protocol_ok(PROTOCOL_VERSION + 1));
        assert_eq!(protocol_range_text(), format!("v{PROTOCOL_VERSION}"));
    }

    #[test]
    fn accepts_editor_kind_and_args() {
        let text = minimal()
            .replace("\"kind\": \"command\"", "\"kind\": \"editor\"")
            .replace("\"ocr.exe\"", "\"doodle.exe\"")
            .replace(
                "\"capabilities\"",
                "\"args\": [\"--pipe\"], \"capabilities\"",
            );
        let m = Manifest::parse(&text).expect("valid editor manifest");
        assert_eq!(m.kind, Kind::Editor);
        assert_eq!(m.args, vec!["--pipe"]);
    }

    #[test]
    fn entry_components_reject_parent_and_root() {
        assert!(validate_entry("a/../b.exe", Kind::Command).is_err());
        assert!(validate_entry("/a.exe", Kind::Command).is_err());
        assert!(validate_entry("C:/a.exe", Kind::Command).is_err());
        assert!(validate_entry("tools/a.exe", Kind::Command).is_ok());
    }

    /// 响应超时：缺省 30s，manifest 可放宽，但不许超过 30 分钟。
    ///
    /// 上限不是"建议"——插件不能替桌面软件决定"让它等多久"。
    #[test]
    fn timeout_is_optional_and_capped() {
        let m = Manifest::parse(minimal()).expect("valid manifest");
        assert_eq!(m.timeout_ms, None);
        assert_eq!(
            m.response_timeout(),
            std::time::Duration::from_millis(DEFAULT_TIMEOUT_MS)
        );

        let long = minimal().replace(
            "\"capabilities\"",
            "\"timeout_ms\": 120000, \"capabilities\"",
        );
        let m = Manifest::parse(&long).expect("valid manifest");
        assert_eq!(m.timeout_ms, Some(120_000));
        assert_eq!(
            m.response_timeout(),
            std::time::Duration::from_millis(120_000),
            "manifest 声明的超时必须被采纳（长任务插件靠它活命）"
        );

        let over = minimal().replace(
            "\"capabilities\"",
            &format!("\"timeout_ms\": {}, \"capabilities\"", MAX_TIMEOUT_MS + 1),
        );
        assert!(
            Manifest::parse(&over).is_err(),
            "超过上限的 timeout_ms 必须被拒（坏清单不是坏超时）"
        );

        let zero = minimal().replace("\"capabilities\"", "\"timeout_ms\": 0, \"capabilities\"");
        assert!(
            Manifest::parse(&zero).is_err(),
            "timeout_ms=0 无意义，必须被拒"
        );
    }

    /// 设置窗口声明：缺省 = 没有；`true` 必须配 `settings.write`。
    ///
    /// 判据是"改完能不能存下来"：只有读权限的插件给了齿轮，用户改一通却落不了盘，
    /// 那比没有入口更糟——所以这种 manifest 在解析期就拒，而不是运行时降级。
    #[test]
    fn settings_ui_requires_write_capability() {
        let plain = Manifest::parse(minimal()).expect("valid manifest");
        assert!(!plain.settings_ui, "缺省没有设置窗口");

        let with_write = minimal().replace(
            "\"capabilities\"",
            "\"settings_ui\": true, \"capabilities\"",
        );
        let with_write = with_write.replace(
            "\"image.read\"",
            "\"image.read\", \"settings.read\", \"settings.write\"",
        );
        let m = Manifest::parse(&with_write).expect("带写权限的设置窗口合法");
        assert!(m.settings_ui);

        // 只声明 settings_ui、没有 settings.write = 坏清单。
        let no_write = minimal().replace(
            "\"capabilities\"",
            "\"settings_ui\": true, \"capabilities\"",
        );
        assert!(
            Manifest::parse(&no_write).is_err(),
            "改完存不下来的设置窗口必须被拒"
        );

        // 非布尔值同样是坏清单。
        let bad = minimal().replace(
            "\"capabilities\"",
            "\"settings_ui\": \"yes\", \"capabilities\"",
        );
        assert!(Manifest::parse(&bad).is_err());
    }

    /// 能力表分层：宿主**真的**提供通道的能力只有五项，其余只是"告知用户"。
    ///
    /// 这条是能力表的诚实性锚：插件作者必须分得清"声明了就有"与"声明了只是告知"。
    #[test]
    fn capability_channels_are_classified() {
        use Capability::*;
        for c in [
            ImageRead,
            ImageWrite,
            ClipboardWrite,
            SettingsRead,
            SettingsWrite,
        ] {
            assert!(c.host_provides(), "{} 应该是宿主真开通道的能力", c.as_str());
        }
        for c in [
            SelectionRead,
            FileRead,
            FileWrite,
            Network,
            UiWindow,
            UiPanel,
            ProcessSpawn,
        ] {
            assert!(
                !c.host_provides(),
                "{} 宿主并没有通道，只能是告知类",
                c.as_str()
            );
        }
    }

    /// manifest 的可选热键：缺省 = 没有；给了就必须是宿主认得的键串。
    ///
    /// 给了却写不成键串 = **坏清单**，不是"照办"：一条永远按不出来的热键
    /// 等于没写，还会让用户以为插件坏了。
    #[test]
    fn optional_hotkey_is_parsed_or_rejected() {
        let m = Manifest::parse(minimal()).expect("valid manifest");
        assert_eq!(m.hotkey, None, "没写 hotkey 就不该有动态热键");

        let with = minimal().replace(
            "\"capabilities\"",
            "\"hotkey\": \"Ctrl+Shift+O\", \"capabilities\"",
        );
        let m = Manifest::parse(&with).expect("valid manifest with hotkey");
        let hk = m.hotkey.expect("热键应被解析");
        assert!(hk.ctrl && hk.shift, "修饰键必须被解析出来");
        assert_eq!(hk.base, crate::core::hotkey::Base::Char('O'));

        let bad = minimal().replace(
            "\"capabilities\"",
            "\"hotkey\": \"Ctrl+NotAKey\", \"capabilities\"",
        );
        assert!(
            Manifest::parse(&bad).is_err(),
            "认不出的键串必须被拒（坏清单不是坏热键）"
        );

        let wrong_type = minimal().replace("\"capabilities\"", "\"hotkey\": 42, \"capabilities\"");
        assert!(Manifest::parse(&wrong_type).is_err(), "hotkey 必须是字符串");
    }

    /// 动态热键表：按下这把键该找谁。
    #[test]
    fn dynamic_hotkeys_match_the_declared_key() {
        let key = Hotkey::from_spec("Ctrl+Shift+O").expect("键串");
        let mut table = DynamicHotkeys::default();
        table.push(PluginHotkey {
            plugin_id: "com.example.ocr".to_string(),
            plugin_name: "OCR".to_string(),
            key,
        });
        let hit = table
            .match_plugin(&KeyInput {
                key: crate::core::hotkey::VKey::Char('O'),
                ctrl: true,
                alt: false,
                shift: true,
                win: false,
            })
            .expect("应命中");
        assert_eq!(hit.plugin_id, "com.example.ocr");
        assert!(table.contains_key(&key));

        // 少一个修饰键就不是同一把键（精确比对）。
        assert!(table
            .match_plugin(&KeyInput {
                key: crate::core::hotkey::VKey::Char('O'),
                ctrl: true,
                alt: false,
                shift: false,
                win: false,
            })
            .is_none());
    }

    // ── 声明式设置（settings schema）────────────────────────────────────

    /// 在最小清单上挂一段 `settings`（连带给读写能力）。
    fn with_settings(settings_json: &str) -> String {
        minimal()
            .replace(
                "\"capabilities\": [\"image.read\", \"clipboard.write\"]",
                "\"capabilities\": [\"image.read\", \"settings.read\", \"settings.write\"]",
            )
            .replace(
                "\"author\": \"example\"",
                &format!("\"author\": \"example\", \"settings\": {settings_json}"),
            )
    }

    #[test]
    fn settings_schema_parses_every_kind_and_normalizes_defaults() {
        let m = Manifest::parse(&with_settings(
            r#"[
                {"key": "open_url", "label": "解码后直接打开网址", "type": "bool"},
                {"key": "open_image", "label": "解码后直接打开图片", "type": "bool", "default": true},
                {"key": "palette", "label": "主色板色数", "type": "choice",
                 "choices": ["4", "8", "16"], "labels": ["4 色", "8 色", "16 色"], "default": "16"},
                {"key": "retries", "label": "重试次数", "type": "int", "default": 3},
                {"key": "note", "label": "备注", "type": "text"},
                {"key": "hotkey", "label": "快捷键", "type": "hotkey", "default": "Ctrl+Shift+S"}
            ]"#,
        ))
        .expect("schema must parse");

        assert_eq!(m.settings.len(), 6);
        let by = |k: &str| m.settings.iter().find(|s| s.key == k).expect("key");
        assert_eq!(by("open_url").default, "0", "bool 缺省 = 关");
        assert_eq!(by("open_image").default, "1", "true 归一成 1");
        assert_eq!(by("palette").default, "16");
        assert_eq!(
            by("palette").display_choices(),
            vec![
                ("4".to_string(), "4 色".to_string()),
                ("8".to_string(), "8 色".to_string()),
                ("16".to_string(), "16 色".to_string()),
            ]
        );
        assert_eq!(by("retries").default, "3", "数字默认值也落成字符串");
        assert_eq!(by("note").default, "");
        assert_eq!(by("hotkey").kind, SettingKind::Hotkey);
        assert_eq!(by("hotkey").default, "Ctrl+Shift+S");
    }

    #[test]
    fn settings_schema_rejects_manifests_that_would_break_the_renderer() {
        // 没有读写能力：宿主既读不到旧值也存不下新值。
        let no_caps = minimal().replace(
            "\"author\": \"example\"",
            "\"author\": \"example\", \"settings\": [{\"key\": \"a\", \"label\": \"A\", \"type\": \"bool\"}]",
        );
        assert!(Manifest::parse(&no_caps).is_err(), "缺 settings 能力要拒");

        // 与 settings_ui 同时出现：一个说宿主画、一个说插件画。
        let both = with_settings(r#"[{"key":"a","label":"A","type":"bool"}]"#)
            .replace("\"author\"", "\"settings_ui\": true, \"author\"");
        assert!(Manifest::parse(&both).is_err(), "两条路只能选一条");

        // 与 manifest.hotkey 同时出现：同一件事两个真源。
        let dual = with_settings(r#"[{"key":"hotkey","label":"键","type":"hotkey"}]"#)
            .replace("\"author\"", "\"hotkey\": \"F8\", \"author\"");
        assert!(Manifest::parse(&dual).is_err(), "热键不许两处声明");
    }

    #[test]
    fn settings_schema_rejects_bad_entries_with_the_index_in_the_message() {
        for (json, needle) in [
            (r#"[{"key":"a","label":"A","type":"nope"}]"#, "type="),
            (r#"[{"key":"","label":"A","type":"bool"}]"#, "key="),
            (r#"[{"key":"a b","label":"A","type":"bool"}]"#, "key="),
            (
                r#"[{"key":"a","label":"A","type":"bool"},{"key":"a","label":"B","type":"bool"}]"#,
                "重复",
            ),
            (
                r#"[{"key":"a","label":"A","type":"choice","choices":["x"]}]"#,
                "至少要两个",
            ),
            (
                r#"[{"key":"a","label":"A","type":"choice","choices":["x","y"],"labels":["X"]}]"#,
                "数量必须一致",
            ),
            (
                r#"[{"key":"a","label":"A","type":"choice","choices":["x","y"],"default":"z"}]"#,
                "不在 choices",
            ),
            (
                r#"[{"key":"a","label":"A","type":"int","default":"x"}]"#,
                "不是整数",
            ),
            (
                r#"[{"key":"a","label":"A","type":"hotkey","default":"Ctrl+Nope"}]"#,
                "键串",
            ),
            (r#""not-an-array""#, "数组"),
        ] {
            let err = Manifest::parse(&with_settings(json)).expect_err("must reject");
            assert!(
                err.contains("settings[") || err.contains("settings 必须"),
                "{err}"
            );
            assert!(err.contains(needle), "错因没说到点子上：{err}");
        }
    }
}
