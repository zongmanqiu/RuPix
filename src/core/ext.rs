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
    /// 仅 `Int` 且声明了 `min`/`max`：宿主把这一格渲染成滑块 + 实时数值，
    /// 而不是让人手填数字。`None` = 老行为（整数文本框）。
    pub range: Option<(i64, i64)>,
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

/// 声明式显示层算子：拖参数时宿主用它在**看图窗当前显示的位图**上提前演算同一组数值。
/// 不再是 W7c 那张窗内缩略图——效果直接落在看图窗，参数窗只放控件。
/// 宿主解释的是"插件声明的 op"，不是某个具体插件的调色逻辑；正式整图编辑
/// （`image.write`）仍由插件在非交互路径上完成。新增 op 要同时给宿主解释器与端到端字节判据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewOp {
    /// `out = clamp(in * gain/100 + bias)`，R/G/B 各自一组增益与偏置。
    ChannelLinear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSpec {
    pub op: PreviewOp,
    /// 三个增益键在 `settings` 里的 key（顺序 R/G/B）。
    pub gain_keys: [String; 3],
    /// 三个偏置键在 `settings` 里的 key（顺序 R/G/B）。
    pub bias_keys: [String; 3],
}

/// 一枚声明式滤镜预设：下拉里的取值 → 六个整数（R/G/B 增益、R/G/B 偏置）。
///
/// 预设**不是独立特效**，只是一组写进六条滑块的数值；用户改任一条滑块就回落
/// 「自定义」。这样"反相 = gain -100 / bias 255"这类关系只有一处定义（清单），
/// 宿主和插件都不必各自维护一张名字表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSpec {
    pub value: String,
    pub values: [i32; 6],
}

/// 解析 `presets`（加法式字段，缺省 = 没有预设表）。
///
/// 表里的键必须都出现在某个 `choice` 设置的 `choices` 里；至少留一个**不在表中**
/// 的取值当「自定义」哨兵。六个整数按 `preview.gain` + `preview.bias` 的键序，
/// 逐个用那条滑块声明的 `min`/`max` 校验——预设不许把滑块推到界外。
fn parse_presets(
    root: &Value,
    settings: &[SettingSpec],
    preview: Option<&PreviewSpec>,
) -> Result<Vec<PresetSpec>, String> {
    let table = match root.get("presets") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Obj(kv)) => kv,
        Some(_) => return Err("presets 必须是对象".into()),
    };
    let spec = preview.ok_or_else(|| "presets 需要 preview 声明六个滑块键".to_string())?;
    let keys: Vec<&str> = spec
        .gain_keys
        .iter()
        .chain(spec.bias_keys.iter())
        .map(String::as_str)
        .collect();
    let mut ranges = Vec::with_capacity(6);
    for k in &keys {
        let s = settings
            .iter()
            .find(|s| s.key == *k)
            .ok_or_else(|| format!("presets: `{k}` 不在 settings 里"))?;
        let r = s
            .range
            .ok_or_else(|| format!("presets: `{k}` 必须是带 min/max 的滑块"))?;
        ranges.push(r);
    }
    let choices = settings
        .iter()
        .find(|s| s.kind == SettingKind::Choice)
        .map(|s| s.choices.clone())
        .ok_or_else(|| "presets 需要一个 choice 设置项".to_string())?;
    if table.len() >= choices.len() {
        return Err("presets 必须给 choice 留一个「自定义」取值".into());
    }
    let mut out = Vec::with_capacity(table.len());
    for (name, body) in table {
        if !choices.contains(name) {
            return Err(format!("presets: `{name}` 不在 choice.choices 里"));
        }
        if name == "custom" {
            return Err("presets: `custom` 是自定义哨兵，不许声明数值".into());
        }
        let mut values = [0i32; 6];
        for (ci, field) in ["gain", "bias"].iter().enumerate() {
            let arr = body
                .get(field)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("presets.{name}.{field} 必须是数组"))?;
            if arr.len() != 3 {
                return Err(format!("presets.{name}.{field} 必须正好 3 个数"));
            }
            for (i, v) in arr.iter().enumerate() {
                let num = v
                    .as_i64()
                    .ok_or_else(|| format!("presets.{name}.{field}[{i}] 必须是整数"))?;
                let num = i32::try_from(num)
                    .map_err(|_| format!("presets.{name}.{field}[{i}] 超出 i32"))?;
                let (lo, hi) = ranges[ci * 3 + i];
                if !(lo..=hi).contains(&(num as i64)) {
                    return Err(format!(
                        "presets.{name}.{field}[{i}]={num} 越出滑块范围 {lo}..={hi}"
                    ));
                }
                values[ci * 3 + i] = num;
            }
        }
        out.push(PresetSpec {
            value: name.clone(),
            values,
        });
    }
    Ok(out)
}

/// 解析 `preview`（加法式字段，缺省 = 不做显示层变换）。键名必须指向**带范围的 int** 项：
/// 实时变换只从滑块信号取数，文本框那一格没有可监听的 0..1 归一化值。
fn parse_preview(root: &Value, settings: &[SettingSpec]) -> Result<Option<PreviewSpec>, String> {
    let Some(p) = root.get("preview") else {
        return Ok(None);
    };
    if matches!(p, Value::Null) {
        return Ok(None);
    }
    if !matches!(p, Value::Obj(_)) {
        return Err("preview 必须是对象".into());
    }
    let op = get_str(p, "op").map_err(|e| format!("preview: {e}"))?;
    if op != "channel-linear" {
        return Err(format!("preview.op=\"{op}\" 不认识（channel-linear）"));
    }
    let keys = |name: &str| -> Result<[String; 3], String> {
        let arr = get_str_array(p, name).map_err(|e| format!("preview.{name}: {e}"))?;
        if arr.len() != 3 {
            return Err(format!("preview.{name} 必须正好 3 个键名"));
        }
        let mut out: [String; 3] = Default::default();
        for (i, k) in arr.iter().enumerate() {
            let spec = settings
                .iter()
                .find(|s| &s.key == k)
                .ok_or_else(|| format!("preview.{name}[{i}]=\"{k}\" 不在 settings 里"))?;
            if spec.kind != SettingKind::Int || spec.range.is_none() {
                return Err(format!(
                    "preview.{name}[{i}]=\"{k}\" 必须是带 min/max 的 int"
                ));
            }
            out[i] = k.clone();
        }
        Ok(out)
    };
    Ok(Some(PreviewSpec {
        op: PreviewOp::ChannelLinear,
        gain_keys: keys("gain")?,
        bias_keys: keys("bias")?,
    }))
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
        // `type=int` 可额外给 `min`/`max`：两个键必须成对、min < max；
        // 没给就是老的文本框。旧清单一个字段都不用改。
        let range = if kind == SettingKind::Int {
            parse_range(item).map_err(at)?
        } else {
            None
        };
        let default = normalize_default(item, kind, &choices, range).map_err(at)?;
        out.push(SettingSpec {
            key,
            label,
            kind,
            default,
            choices,
            labels,
            range,
        });
    }
    Ok(out)
}

/// 滑块范围：两个键必须成对出现且 `min < max`；没给 = 没有范围（文本框）。
/// 解析期拒绝坏范围，免得渲染层拿一个 0 宽轨道去画滑块。
fn parse_range(item: &Value) -> Result<Option<(i64, i64)>, String> {
    let num = |k: &str| -> Result<Option<i64>, String> {
        match item.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Num(n)) if n.fract() == 0.0 => Ok(Some(*n as i64)),
            Some(Value::Str(s)) => s
                .parse::<i64>()
                .map(Some)
                .map_err(|_| format!("{k} 不是整数")),
            Some(_) => Err(format!("{k} 不是整数")),
        }
    };
    match (num("min")?, num("max")?) {
        (None, None) => Ok(None),
        (Some(a), Some(b)) if a < b => Ok(Some((a, b))),
        (Some(_), None) | (None, Some(_)) => Err("min / max 必须成对出现".into()),
        _ => Err("min 必须小于 max".into()),
    }
}

/// 默认值归一：一律落成字符串，且**当场按类型验一遍**——一个装不进控件的默认值
/// 会让设置窗第一次打开就崩在渲染层，而在解析期拒绝只是一条清单错误。
fn normalize_default(
    item: &Value,
    kind: SettingKind,
    choices: &[String],
    range: Option<(i64, i64)>,
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
        // 有范围的整数没写默认值时落在下界：0 未必在滑块范围内。
        SettingKind::Int => range
            .map(|(lo, _)| lo.to_string())
            .unwrap_or_else(|| "0".to_string()),
        // 布尔没写默认值该落在"关"，而不是空串——空串在控件里是第三态。
        SettingKind::Bool => "0".to_string(),
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
        SettingKind::Int => {
            let n = text
                .parse::<i64>()
                .map_err(|_| format!("int 的 default=\"{text}\" 不是整数"))?;
            if let Some((lo, hi)) = range {
                if !(lo..=hi).contains(&n) {
                    return Err(format!("int 的 default={n} 不在 [{lo},{hi}] 里"));
                }
            }
            Ok(n.to_string())
        }
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
    /// 运行时参数窗（可选，默认 false）。声明它之后，点插件菜单打开的是一扇
    /// **实时**参数窗：拖滑块看图窗立刻变，「完成」只保存参数并关窗，不再起进程。
    /// 快捷键仍是用上次保存的值直接跑（快捷键不该弹窗）。
    ///
    /// 加法式字段：旧清单没有它 = 老行为，一点即跑。必须有 `settings`
    /// 才有窗可弹，解析期就拒空 schema 的 `run_ui=true`。
    pub run_ui: bool,
    /// 声明式显示层变换（可选）：给了它，运行参数窗拖动时**看图窗本尊**实时变色。
    /// 缺省 = 老清单行为不变（只有插件正式回写会改图）。
    pub preview: Option<PreviewSpec>,
    /// 声明式滤镜预设（可选，加法式）：取值 → 六条滑块的数值组。
    pub presets: Vec<PresetSpec>,
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

        // 运行时参数窗：可选布尔。声明了却没有 settings = 没有窗可弹，
        // 属于坏清单（不是静默降级成一点即跑）。
        let run_ui = match root.get("run_ui") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("run_ui 必须是布尔值".into()),
        };
        if run_ui && settings.is_empty() {
            return Err("run_ui=true 必须同时声明 settings（否则没有参数窗可弹）".into());
        }
        let preview = parse_preview(&root, &settings)?;
        let presets = parse_presets(&root, &settings, preview.as_ref())?;

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
            run_ui,
            preview,
            presets,
            description: optional_str(&root, "description"),
            author: optional_str(&root, "author"),
            license: optional_str(&root, "license"),
            released: optional_str(&root, "released"),
            timeout_ms,
            hotkey,
        })
    }

    /// 声明式实时参数占用的设置项 key：`preview` 的六个数值项 + 预设那个下拉。
    ///
    /// `None` = 这枚插件没有"运行时实时参数"这一层（纯命令型 / 老式编辑型）。
    /// 这份清单是**一处分界**，两扇窗都读它：运行时参数窗只渲染这些项，设置页齿轮
    /// 只渲染**剩下**的项——齿轮点开就是调色滑块，正是把两者混成一扇窗的后果
    /// （2026-11-06 用户裁决「数值调整窗口该在应用时激活，不是底层设置」）。
    pub fn live_param_keys(&self) -> Option<Vec<String>> {
        let preview = self.preview.as_ref()?;
        let mut keys: Vec<String> = preview
            .gain_keys
            .iter()
            .chain(preview.bias_keys.iter())
            .cloned()
            .collect();
        // 预设下拉与六个数值是同一次编辑的入口，参数窗要一起显示。判据与 UI 建同步器
        // 那一处相同（第一个 choice 项）：两处若各挑各的，预设会掉进设置窗。
        if let Some(choice) = self.settings.iter().find(|s| s.kind == SettingKind::Choice) {
            keys.push(choice.key.clone());
        }
        Some(keys)
    }

    /// 除实时参数之外还有没有别的设置项。没有就不给齿轮——别用一个"设置"窗
    /// 冒充同一份参数的第二个人口。
    pub fn has_settings_beyond_live(&self) -> bool {
        match self.live_param_keys() {
            None => !self.settings.is_empty(),
            Some(keys) => self
                .settings
                .iter()
                .any(|s| !keys.iter().any(|k| k == &s.key)),
        }
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

#[cfg(test)]
mod range_and_run_ui_tests {
    use super::*;

    fn manifest_with(extra: &str) -> String {
        format!(
            r#"{{
                "schema":1,"id":"test.channels","name":"C","version":"1.0.0",
                "protocol":1,"min_host":"0.1.0","kind":"editor","entry":"c.exe",
                "capabilities":["image.read","image.write","settings.read","settings.write"],
                "platforms":["windows-x64"]{extra}
            }}"#
        )
    }

    #[test]
    fn int_range_is_parsed_and_plain_int_stays_a_text_box() {
        let src = manifest_with(
            r#","settings":[
                {"key":"gain","label":"增益","type":"int","default":"100","min":-400,"max":400},
                {"key":"plain","label":"普通","type":"int","default":"7"}
            ]"#,
        );
        let m = Manifest::parse(&src).expect("valid manifest");
        assert_eq!(m.settings.len(), 2);
        assert_eq!(m.settings[0].range, Some((-400, 400)));
        assert_eq!(m.settings[1].range, None, "没给范围 = 老文本框");
        assert!(!m.run_ui, "旧清单不弹运行参数窗");
    }

    #[test]
    fn a_range_without_an_explicit_default_lands_on_the_lower_bound() {
        let src =
            manifest_with(r#","settings":[{"key":"k","label":"K","type":"int","min":1,"max":9}]"#);
        let m = Manifest::parse(&src).expect("valid manifest");
        assert_eq!(m.settings[0].default, "1");
    }

    #[test]
    fn bad_ranges_are_rejected_at_parse_time() {
        // 负向验证：每一条都必须被解析期挡住，不能拖到渲染层画一条 0 宽轨道。
        for (tail, needle) in [
            (
                r#"[{"key":"k","label":"K","type":"int","min":5}]"#,
                "必须成对",
            ),
            (
                r#"[{"key":"k","label":"K","type":"int","min":5,"max":5}]"#,
                "min 必须小于 max",
            ),
            (
                r#"[{"key":"k","label":"K","type":"int","min":9,"max":1}]"#,
                "min 必须小于 max",
            ),
            (
                r#"[{"key":"k","label":"K","type":"int","default":"99","min":1,"max":9}]"#,
                "不在 [1,9]",
            ),
            (
                r#"[{"key":"k","label":"K","type":"int","min":"oops","max":9}]"#,
                "min 不是整数",
            ),
        ] {
            let src = manifest_with(&format!(r#","settings":{tail}"#));
            let err = Manifest::parse(&src).expect_err("bad range must be rejected");
            assert!(err.contains(needle), "错因没说到点子上：{err}");
        }
    }

    #[test]
    fn run_ui_requires_a_non_empty_schema() {
        let ok = manifest_with(
            r#","run_ui":true,"settings":[{"key":"k","label":"K","type":"int","min":0,"max":10}]"#,
        );
        assert!(Manifest::parse(&ok).expect("valid manifest").run_ui);

        let no_schema = manifest_with(r#","run_ui":true"#);
        let err = Manifest::parse(&no_schema).expect_err("run_ui without settings");
        assert!(err.contains("没有参数窗可弹"), "{err}");

        let not_bool = manifest_with(r#","run_ui":"yes""#);
        let err = Manifest::parse(&not_bool).expect_err("run_ui must be a bool");
        assert!(err.contains("必须是布尔值"), "{err}");
    }
}

#[cfg(test)]
mod preview_spec_tests {
    use super::*;

    fn base(extra: &str) -> String {
        format!(
            r#"{{
                "schema":1,"id":"test.preview","name":"P","version":"1.0.0",
                "protocol":1,"min_host":"0.1.0","kind":"editor","entry":"p.exe",
                "capabilities":["image.read","image.write","settings.read","settings.write"],
                "platforms":["windows-x64"]{extra}
            }}"#
        )
    }

    const SETTINGS: &str = r#","settings":[
        {"key":"g0","label":"G0","type":"int","min":-400,"max":400,"default":"100"},
        {"key":"g1","label":"G1","type":"int","min":-400,"max":400,"default":"100"},
        {"key":"g2","label":"G2","type":"int","min":-400,"max":400,"default":"100"},
        {"key":"b0","label":"B0","type":"int","min":-255,"max":255,"default":"0"},
        {"key":"b1","label":"B1","type":"int","min":-255,"max":255,"default":"0"},
        {"key":"b2","label":"B2","type":"int","min":-255,"max":255,"default":"0"},
        {"key":"txt","label":"T","type":"int","default":"7"}
    ]"#;

    #[test]
    fn preview_is_optional_and_old_manifests_are_untouched() {
        let m = Manifest::parse(&base(SETTINGS)).expect("valid");
        assert!(m.preview.is_none());
        assert!(!m.run_ui);
    }

    #[test]
    fn a_valid_preview_binds_six_slider_keys() {
        let extra = format!(
            r#"{SETTINGS},"run_ui":true,"preview":{{"op":"channel-linear","gain":["g0","g1","g2"],"bias":["b0","b1","b2"]}}"#
        );
        let m = Manifest::parse(&base(&extra)).expect("valid");
        let p = m.preview.expect("preview parsed");
        assert_eq!(p.op, PreviewOp::ChannelLinear);
        assert_eq!(p.gain_keys, ["g0", "g1", "g2"]);
        assert_eq!(p.bias_keys, ["b0", "b1", "b2"]);
    }

    fn preset_base(extra: &str) -> String {
        format!(
            r#"{{
                "schema":1,"id":"test.presets","name":"P","version":"1.0.0",
                "protocol":1,"min_host":"0.1.0","kind":"editor","entry":"p.exe",
                "capabilities":["image.read","image.write","settings.read","settings.write"],
                "platforms":["windows-x64"],
                "settings":[
                  {{"key":"preset","label":"预设","type":"choice",
                    "choices":["custom","invert","bright"],"labels":["自定义","反相","提亮"]}},
                  {{"key":"g0","label":"G0","type":"int","min":-400,"max":400,"default":"100"}},
                  {{"key":"g1","label":"G1","type":"int","min":-400,"max":400,"default":"100"}},
                  {{"key":"g2","label":"G2","type":"int","min":-400,"max":400,"default":"100"}},
                  {{"key":"b0","label":"B0","type":"int","min":-255,"max":255,"default":"0"}},
                  {{"key":"b1","label":"B1","type":"int","min":-255,"max":255,"default":"0"}},
                  {{"key":"b2","label":"B2","type":"int","min":-255,"max":255,"default":"0"}}
                ],
                "preview":{{"op":"channel-linear","gain":["g0","g1","g2"],"bias":["b0","b1","b2"]}}{extra}
            }}"#
        )
    }

    /// 参数窗 / 设置窗的分界：`preview` 消费掉六个数值 + 预设下拉，剩下的才归齿轮。
    ///
    /// 两扇窗重复渲染同一份清单，就是用户 2026-11-06 报的「点齿轮出来的居然是
    /// 数值调整窗口」；而清单里一项都不剩时，齿轮本身也不该出现。
    #[test]
    fn live_params_are_carved_out_of_the_settings_window() {
        let m = Manifest::parse(&preset_base("")).expect("valid");
        assert_eq!(
            m.live_param_keys().expect("声明了 preview 就有实时参数"),
            vec!["g0", "g1", "g2", "b0", "b1", "b2", "preset"],
            "六个数值 + 预设下拉，顺序与 preview 的键序一致"
        );
        assert!(
            !m.has_settings_beyond_live(),
            "清单里只剩实时参数 ⇒ 不给齿轮（别拿参数窗冒充设置）"
        );

        // 补一行快捷键（真实插件的形状）：齿轮该有了，参数窗仍然只拿七项。
        let with_hotkey = preset_base("").replace(
            r#"{"key":"b2","label":"B2","type":"int","min":-255,"max":255,"default":"0"}"#,
            r#"{"key":"b2","label":"B2","type":"int","min":-255,"max":255,"default":"0"},
                  {"key":"hotkey","label":"快捷键","type":"hotkey","default":""}"#,
        );
        let m = Manifest::parse(&with_hotkey).expect("valid");
        assert!(m.has_settings_beyond_live(), "有快捷键行 ⇒ 齿轮该出现");
        let keys = m.live_param_keys().expect("仍是实时型");
        assert_eq!(keys.len(), 7, "快捷键不进参数窗");
        assert!(keys.iter().all(|k| k != "hotkey"));
    }

    /// 没有 `preview` 的清单不是实时型：它们两扇窗的分界退回老口径
    /// （有 schema 就给齿轮，参数窗就是设置窗）。
    #[test]
    fn manifests_without_a_preview_keep_the_old_settings_boundary() {
        let plain = Manifest::parse(
            r#"{"schema":1,"id":"t.plain","name":"P","version":"1.0.0","protocol":1,
                "min_host":"0.1.0","kind":"command","entry":"p.exe",
                "platforms":["windows-x64"]}"#,
        )
        .expect("valid");
        assert!(
            plain.live_param_keys().is_none(),
            "没有 preview 就没有实时参数"
        );
        assert!(
            !plain.has_settings_beyond_live(),
            "没有 schema 就没有设置窗"
        );
    }

    #[test]
    fn declarative_presets_are_parsed_as_six_numbers() {
        let m = Manifest::parse(&preset_base(
            r#","presets":{
                "invert":{"gain":[-100,-100,-100],"bias":[255,255,255]},
                "bright":{"gain":[100,100,100],"bias":[40,40,40]}
            }"#,
        ))
        .expect("valid");
        assert_eq!(m.presets.len(), 2);
        assert_eq!(m.presets[0].value, "invert");
        assert_eq!(m.presets[0].values, [-100, -100, -100, 255, 255, 255]);
        assert_eq!(m.presets[1].values, [100, 100, 100, 40, 40, 40]);
        // 没有 presets 的旧清单不受影响。
        assert!(Manifest::parse(&preset_base(""))
            .unwrap()
            .presets
            .is_empty());
    }

    #[test]
    fn bad_preset_tables_are_rejected_at_parse_time() {
        for (extra, needle) in [
            (r#","presets":42"#, "必须是对象"),
            (
                r#","presets":{"nope":{"gain":[1,2,3],"bias":[1,2,3]}}"#,
                "不在 choice.choices",
            ),
            (
                r#","presets":{"invert":{"gain":[-100,-100],"bias":[1,2,3]}}"#,
                "正好 3 个数",
            ),
            (
                r#","presets":{"invert":{"gain":[-100,-100,-100],"bias":[999,2,3]}}"#,
                "越出滑块范围",
            ),
            (
                r#","presets":{"invert":{"gain":[-100,-100,-100],"bias":["x",2,3]}}"#,
                "必须是整数",
            ),
            (
                r#","presets":{"custom":{"gain":[100,100,100],"bias":[0,0,0]}}"#,
                "自定义哨兵",
            ),
            (
                r#","presets":{"invert":{"gain":[1,2,3],"bias":[1,2,3]},"bright":{"gain":[1,2,3],"bias":[1,2,3]},"third":{"gain":[1,2,3],"bias":[1,2,3]}}"#,
                "留一个",
            ),
        ] {
            let err = Manifest::parse(&preset_base(extra)).expect_err("坏预设表必须拒");
            assert!(err.contains(needle), "错因没说到点子上：{err}");
        }
    }

    #[test]
    fn presets_without_a_preview_declaration_are_rejected() {
        let src = preset_base(
            r#","presets":{"invert":{"gain":[-100,-100,-100],"bias":[255,255,255]}}"#,
        )
        .replace(
            r#""preview":{"op":"channel-linear","gain":["g0","g1","g2"],"bias":["b0","b1","b2"]}"#,
            r#""preview":null"#,
        );
        let err = Manifest::parse(&src).expect_err("没有 op 就没有键序可校验");
        assert!(err.contains("需要 preview"), "错因没说到点子上：{err}");
    }

    #[test]
    fn bad_preview_declarations_are_rejected_at_parse_time() {
        // 负向验证：坏公式、坏键数、不存在的键、无滑块的 int，一条都不能拖到渲染层。
        for (prev, needle) in [
            (
                r#""preview":{"op":"gamma","gain":["g0","g1","g2"],"bias":["b0","b1","b2"]}"#,
                "不认识",
            ),
            (
                r#""preview":{"op":"channel-linear","gain":["g0","g1"],"bias":["b0","b1","b2"]}"#,
                "正好 3 个",
            ),
            (
                r#""preview":{"op":"channel-linear","gain":["g0","g1","nope"],"bias":["b0","b1","b2"]}"#,
                "不在 settings",
            ),
            (
                r#""preview":{"op":"channel-linear","gain":["g0","g1","txt"],"bias":["b0","b1","b2"]}"#,
                "带 min/max 的 int",
            ),
            (r#""preview":42"#, "必须是对象"),
        ] {
            let extra = format!("{SETTINGS},{prev}");
            let err = Manifest::parse(&base(&extra)).expect_err("bad preview must be rejected");
            assert!(err.contains(needle), "错因没说到点子上：{err}");
        }
    }
}
