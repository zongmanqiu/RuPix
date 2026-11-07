//! 插件设置窗（**宿主渲染**）：manifest `settings` schema → 控件行 → 写回
//! `plugins/<id>/settings.json`。
//!
//! 为什么由宿主画、而不是插件自己画：命令型插件没有窗口，而让每枚插件各画一遍对话框，
//! 等于把"校验 / 落盘 / 快捷键捕获"各写五遍。插件只声明 schema 与读值
//! （[`crate::core::ext::SettingSpec`]），界面与存储是宿主的事——参考实现 CurSeen 的
//! 脚本 `schema()` 是同一层想法。
//!
//! 与 `settings_ui` 那条老路的分工：声明了 schema 就**不再**以 `action=settings` 拉起插件
//! （解析期已保证两者不同时出现）。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use windui::core::{EventCtx, Widget};
use windui::event::Event;
use windui::geometry::{Rect, Size};
use windui::prelude::*;
use windui::render::Canvas;
use windui::style::Style;
use windui::text::TextEngine;
use windui::ui::Slider;

use super::settings::option_row_inset;
use super::settings::shortcuts::hotkey_box;
use super::tokens::{to_color, BG, CONTROL_GROUP_W, DIALOG_FIT_MIN};
use crate::core::config::Config;
use crate::core::ext::{SettingKind, SettingSpec};
use crate::core::ext_protocol::Settings;
use crate::core::hotkey::Hotkey;
use crate::extensions::Extension;

/// 一行控件的状态（按 schema 的类型分派）。
#[derive(Clone)]
enum Bound {
    Bool(Signal<bool>),
    Choice {
        idx: Signal<usize>,
        values: Vec<String>,
        labels: Vec<String>,
    },
    Text(Signal<String>),
    /// 滑块：框架的 `Signal<f32>` 是 0..1，存进设置区的仍是整数字符串。
    Range {
        norm: Signal<f32>,
        lo: i64,
        hi: i64,
    },
    /// 原始键串（存进设置区的那一份）。
    Hotkey(Signal<String>),
}

impl Bound {
    fn from_spec(spec: &SettingSpec, value: &str) -> Self {
        match spec.kind {
            SettingKind::Bool => Bound::Bool(signal(value == "1")),
            SettingKind::Choice => {
                let pairs = spec.display_choices();
                let values: Vec<String> = pairs.iter().map(|(v, _)| v.clone()).collect();
                let labels: Vec<String> = pairs.iter().map(|(_, l)| l.clone()).collect();
                let idx = values.iter().position(|v| v == value).unwrap_or(0);
                Bound::Choice {
                    idx: signal(idx),
                    values,
                    labels,
                }
            }
            SettingKind::Int => match spec.range {
                Some((lo, hi)) => {
                    let fallback = spec.default.parse::<i64>().unwrap_or(lo);
                    let v = value.parse::<i64>().unwrap_or(fallback);
                    let norm = ((v - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0);
                    Bound::Range {
                        norm: signal(norm),
                        lo,
                        hi,
                    }
                }
                None => Bound::Text(signal(value.to_string())),
            },
            SettingKind::Text => Bound::Text(signal(value.to_string())),
            SettingKind::Hotkey => Bound::Hotkey(signal(value.to_string())),
        }
    }

    /// 当前值 → 存进设置区的字符串。
    fn value(&self) -> String {
        match self {
            Bound::Bool(s) => {
                if s.get() {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Bound::Choice { idx, values, .. } => choice_value(values, idx.get()),
            Bound::Text(s) | Bound::Hotkey(s) => s.get(),
            Bound::Range { norm, lo, hi } => {
                let n = (*lo as f32 + norm.get() * (*hi - *lo) as f32).round() as i64;
                n.clamp(*lo, *hi).to_string()
            }
        }
    }
}

/// 下标 → 取值。越界回落到第一项：调用方是信号驱动的下拉，理论上不会越界，
/// 但这里回落的代价是"显示第一个选项"，而 panic 的代价是整个查看器没了。
fn choice_value(values: &[String], idx: usize) -> String {
    values.get(idx).cloned().unwrap_or_default()
}

/// 滑块右侧的实时数值：派生信号，源变即重算（不依赖 relayout 的那条老坑）。
fn range_value_signal(norm: Signal<f32>, lo: i64, hi: i64) -> Signal<String> {
    norm.map(move |v| {
        let n = (lo as f32 + v * (hi - lo) as f32).round() as i64;
        format!("{n}")
    })
}

/// hotkey 规格在 `specs` 里的下标（按出现顺序）。
///
/// ★ 显示文本向量与捕获路由**必须共用这一份顺序**：按"第几个设置项"建、按"第几个
///   快捷键行"取，就会错位——2026-10-31 实测：`checkup` 的「快捷键」框显示成了上一行
///   「主色板色数」的值（`8`），而且抢占后重开又变回 `8`，根因就是这里错了一格。
fn hotkey_spec_indices(specs: &[SettingSpec]) -> Vec<usize> {
    specs
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == SettingKind::Hotkey)
        .map(|(i, _)| i)
        .collect()
}

/// 窗宽：两列版式吃满所需的宽度 + 一点余量。
///
/// 标签左缘 = 外层 `DIALOG_PAD`(16)，**与底栏按钮同一条竖线**（`dialog_footer::shell`
/// 给的四边留白，见 `docs/界面版式规范.md` §2）；控件右缘 = 16 + `COL1_W`(208) +
/// `CONTROL_GROUP_W`(200) = 424，右边再留
/// 40 给 `flex_spacer`，控件不会被顶到窗框。
const DIALOG_W: i32 = 480;

/// 一次实时调色：绑定当前图片路径，换图后自动失效（不靠定时器清）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTint {
    pub path: PathBuf,
    /// R/G/B 增益、R/G/B 偏置（顺序与 manifest `preview` 的六个键一致）。
    pub values: [i32; 6],
    /// 第七条滑块：Gamma 曲线（100 = 恒等）。老插件没有这一格，恒取 100。
    pub gamma: i32,
}

impl LiveTint {
    pub fn is_neutral(&self) -> bool {
        self.values == crate::core::pixels::CHANNEL_NEUTRAL
            && self.gamma == crate::core::pixels::GAMMA_NEUTRAL
    }

    pub fn gain(&self) -> [i32; 3] {
        [self.values[0], self.values[1], self.values[2]]
    }

    pub fn bias(&self) -> [i32; 3] {
        [self.values[3], self.values[4], self.values[5]]
    }

    pub fn gamma(&self) -> i32 {
        self.gamma
    }
}

/// 画布与参数窗共享的实时状态（**每窗一份**，窗口创建时造，见 `ImageCanvas::new`）。
#[derive(Clone)]
pub struct LiveState {
    /// 当前这张图的显示层变换；`None` / 恒等参数 = 原样显示。
    pub tint: Signal<Option<LiveTint>>,
    /// 「还原」按一下 +1：参数窗把六条滑块归位用。
    pub reset: Signal<u64>,
}

impl LiveState {
    pub fn new() -> Self {
        Self {
            tint: signal(None),
            reset: signal(0),
        }
    }
}

impl Default for LiveState {
    fn default() -> Self {
        Self::new()
    }
}

/// 六条滑块 ↔ 一个 `LiveTint` 的双向同步。
///
/// 为什么需要 `poll`：框架的 `Signal` 只有"读时重算"，没有 on_change 回调；
/// 下拉预设和「还原」都不是滑块事件。`LiveSlider::paint` 每次整窗重绘都会
/// 跑到这里，用两组 `Cell` 判"这次是谁改的"，避免来回写打乒乓。
struct SyncState {
    /// 前六格是 gain/bias，第七格是 Gamma。老清单没有第七格：
    /// `ranges[6] = (100, 100)` 让它恒等，不参与任何插值。
    norms: [Signal<f32>; 7],
    ranges: [(i64, i64); 7],
    preset_idx: Signal<usize>,
    custom_idx: usize,
    /// (choice 下标, 六个整数, Gamma)。`custom` 不在表里 = 保留用户自己拖出来的值。
    presets: Vec<(usize, [i32; 6], i32)>,
    state: LiveState,
    path: PathBuf,
    reset_seen: Cell<u64>,
    last_preset: Cell<usize>,
}

impl SyncState {
    fn slot_value(&self, i: usize) -> i32 {
        let (lo, hi) = self.ranges[i];
        let n = (lo as f32 + self.norms[i].get() * (hi - lo) as f32).round() as i64;
        n.clamp(lo, hi) as i32
    }

    fn values(&self) -> ([i32; 6], i32) {
        let mut out = [0i32; 6];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.slot_value(i);
        }
        (out, self.slot_value(6))
    }

    fn set_values(&self, v: [i32; 6], gamma: i32) {
        let all = [v[0], v[1], v[2], v[3], v[4], v[5], gamma];
        for (i, want) in all.iter().enumerate() {
            let (lo, hi) = self.ranges[i];
            // 无 Gamma 的那条老清单：零宽范围，别做 0/0 除法喂出 NaN。
            if hi == lo {
                self.norms[i].set(0.0);
                continue;
            }
            self.norms[i].set(((*want as i64 - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0));
        }
    }

    /// 当前滑块 → 看图窗。恒等参数直接清成 `None`，一帧都不多算。
    fn publish(&self) {
        let (v, gamma) = self.values();
        let neutral = v == crate::core::pixels::CHANNEL_NEUTRAL
            && gamma == crate::core::pixels::GAMMA_NEUTRAL;
        self.state.tint.set((!neutral).then(|| LiveTint {
            path: self.path.clone(),
            values: v,
            gamma,
        }));
    }

    /// 用户亲手拖了滑块：预设回落「自定义」，立即上屏。
    fn on_slider(&self) {
        self.preset_idx.set(self.custom_idx);
        self.last_preset.set(self.custom_idx);
        self.publish();
    }

    /// paint 里跑：收「还原」与下拉选预设这两个外部来源。
    fn poll(&self) {
        let r = self.state.reset.get();
        if r != self.reset_seen.get() {
            self.reset_seen.set(r);
            self.set_values(
                crate::core::pixels::CHANNEL_NEUTRAL,
                crate::core::pixels::GAMMA_NEUTRAL,
            );
            self.preset_idx.set(self.custom_idx);
            self.last_preset.set(self.custom_idx);
            self.publish();
            return;
        }
        let p = self.preset_idx.get();
        if p != self.last_preset.get() {
            self.last_preset.set(p);
            if let Some((_, v, g)) = self.presets.iter().find(|(i, _, _)| *i == p) {
                self.set_values(*v, *g);
                self.publish();
            }
        }
    }
}

/// 滑块包装：**只加两件事**，绘制与拖拽仍全权交给框架 `Slider`。
///
/// ① 值一变就 `publish` + `mark_dirty_all()`。框架在 Move 事件里把 `Signal::set`
///    降级成"本控件局部脏区"，派生数值标签和另一个窗口都不在那一格 ⇒ 用户看到
///    "拖着不动、松手才变"。显式升整窗 + 跨窗脏标记才是真·逐帧。
/// ② `paint` 里 `poll()`，把下拉预设 / 「还原」接进同一条同步链。
struct LiveSlider {
    inner: Slider,
    value: Signal<f32>,
    sync: Rc<SyncState>,
}

impl Widget for LiveSlider {
    fn measure(&self, avail: Size, style: &Style, text: &mut dyn TextEngine) -> Size {
        self.inner.measure(avail, style, text)
    }

    fn paint(
        &self,
        bounds: Rect,
        content: Rect,
        focused: bool,
        enabled: bool,
        canvas: &mut dyn Canvas,
        style: &Style,
    ) {
        self.sync.poll();
        self.inner
            .paint(bounds, content, focused, enabled, canvas, style);
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        let before = self.value.get();
        let used = self.inner.on_event(ctx, ev);
        if used && (self.value.get() - before).abs() > f32::EPSILON {
            self.sync.on_slider();
            ctx.mark_dirty_all();
        }
        used
    }

    fn focusable(&self) -> bool {
        true
    }
}

/// 从 manifest 的声明式 `preview` + `presets` 造同步器；任一环缺失就没有实时能力
/// （老插件仍然走「应用 → 起进程」那条路，一点不受影响）。
fn build_sync(
    ext: &Extension,
    specs: &[SettingSpec],
    bounds: &[Bound],
    state: &LiveState,
    path: PathBuf,
) -> Option<Rc<SyncState>> {
    let spec = ext.manifest.preview.as_ref()?;
    // 六个 gain/bias 键 + 可选的第七个 Gamma 键，顺序即 `LiveTint.values` 的槽位。
    let keys = spec.value_keys();
    let mut norms: [Signal<f32>; 7] = std::array::from_fn(|_| signal(0.0f32));
    let mut ranges = [(0i64, 1i64); 7];
    // 老清单没有 Gamma：零宽范围让第七格恒等于 100，不参与插值。
    let neutral = crate::core::pixels::GAMMA_NEUTRAL as i64;
    ranges[6] = (neutral, neutral);
    for (i, key) in keys.iter().enumerate() {
        let idx = specs.iter().position(|s| s.key == *key)?;
        let Bound::Range { norm, lo, hi } = &bounds[idx] else {
            return None;
        };
        norms[i] = *norm;
        ranges[i] = (*lo, *hi);
    }
    // 预设下拉：找到第一个 choice 设置项的取值表，把声明式数值按名字对上下标。
    let ci = specs.iter().position(|s| s.kind == SettingKind::Choice)?;
    let Bound::Choice { idx, values, .. } = &bounds[ci] else {
        return None;
    };
    let custom_idx = values.iter().position(|v| v == "custom").unwrap_or(0);
    let presets: Vec<(usize, [i32; 6], i32)> = ext
        .manifest
        .presets
        .iter()
        .filter_map(|p| {
            values
                .iter()
                .position(|v| v == &p.value)
                .map(|i| (i, p.values, p.gamma))
        })
        .collect();
    let last = idx.get();
    Some(Rc::new(SyncState {
        norms,
        ranges,
        preset_idx: *idx,
        custom_idx,
        presets,
        state: state.clone(),
        path,
        reset_seen: Cell::new(state.reset.get()),
        last_preset: Cell::new(last),
    }))
}

/// 运行参数窗里的两颗宿主动作按钮（另存 / 还原），由画布构造、这里只负责摆位置。
/// 这样参数窗不必认识 `Viewer` / `Chrome`，两个入口也共用同一份实现。
/// 两颗**恒亮**：实时调色是在开窗之后才产生的，按开窗那刻的覆盖状态置灰会点不动。
#[derive(Clone)]
pub struct RunActions {
    pub save: Rc<dyn Fn(&mut EventCtx)>,
    pub revert: Rc<dyn Fn(&mut EventCtx)>,
}

/// 「按应用才起进程」那条老路的回调。
pub type LaunchFn = Rc<dyn Fn(&mut EventCtx) -> bool>;

/// 运行参数窗需要的全部外部依赖。
#[derive(Clone)]
pub struct RunPrompt {
    /// 画布那一份实时状态（每窗一份）。
    pub state: LiveState,
    /// 当前图片路径；换图后这层变换自动失效。
    pub path: PathBuf,
    /// 老路（命令型 run_ui）：按「应用」才起进程。声明式实时变换的编辑型不填。
    pub on_apply: Option<LaunchFn>,
    /// 另存 / 还原两颗按钮（只给编辑型）。
    pub actions: Option<RunActions>,
}

/// 参数窗的两种来路：设置页齿轮（只存值）／插件菜单（实时或起进程）。
#[derive(Clone)]
enum Mode {
    Settings,
    Run(RunPrompt),
}

/// 打开（或聚焦）某插件的设置窗（设置页齿轮）。
pub fn show(ctx: &mut EventCtx, ext: &Extension, config: &Rc<RefCell<Config>>) {
    open(ctx, ext, config, Mode::Settings);
}

/// 「运行时参数窗」：点插件菜单先由宿主渲染这扇窗。
/// 声明了 `preview` 的编辑型：拖滑块看图窗立刻变，「完成」只保存参数；
/// 没声明的命令型：仍是按「应用」保存后起进程（旧行为一字不动）。
pub fn show_run_prompt(
    ctx: &mut EventCtx,
    ext: &Extension,
    config: &Rc<RefCell<Config>>,
    opts: RunPrompt,
) {
    open(ctx, ext, config, Mode::Run(opts));
}

/// 底栏按钮的回调（确定 / 取消各一份，交给渲染闭包）。
type ClickFn = Rc<dyn Fn(&mut EventCtx)>;

/// 窗口底栏的三种形态：设置（确定/取消）／实时参数（只有动作按钮）／老路（应用/取消）。
///
/// 一张表决定底栏，是为了让「有没有那个按钮」不散在渲染代码里——用户对「完成/取消」
/// 的裁决（2026-11-06：调色不是保存，用不到）落在这一个枚举上。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Footer {
    Settings,
    Live,
    Legacy,
}

impl Footer {
    fn label(self) -> &'static str {
        match self {
            Footer::Settings => "设置",
            Footer::Live => "实时",
            Footer::Legacy => "应用",
        }
    }

    fn confirm_text(self) -> Option<&'static str> {
        match self {
            Footer::Settings => Some("确定"),
            Footer::Live => None,
            Footer::Legacy => Some("应用"),
        }
    }
}

/// 参数窗的初值：**从当前显示状态读**，不碰 `settings.json`。
///
/// 这样每张图第一次打开都是原图（恒等参数），只有真调用过插件的那张才带着自己的值；
/// 换图后 `LiveState` 里那条 `tint` 路径不再匹配，又回到恒等——「全部默认为原图」
/// 是这条链的自然结果，而不是另写一份按文件的记录。
fn live_seed(ext: &Extension, tint: Option<&LiveTint>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(preview) = ext.manifest.preview.as_ref() else {
        return out;
    };
    let (values, gamma) = tint.map(|t| (t.values, t.gamma)).unwrap_or((
        crate::core::pixels::CHANNEL_NEUTRAL,
        crate::core::pixels::GAMMA_NEUTRAL,
    ));
    // 预设回填：六个数值 + Gamma 一起等于某组预设才报那一组，否则「自定义」。
    // 「暗部补偿」那组的六个数是恒等的，只看六个数会和「原图」撞车。
    // 重开窗时下拉要说出"现在是哪个滤镜"，一律写自定义等于把用户刚选的东西忘掉。
    if let Some(choice) = ext
        .manifest
        .settings
        .iter()
        .find(|s| s.kind == SettingKind::Choice)
    {
        let custom = choice
            .choices
            .iter()
            .find(|c| c.as_str() == "custom")
            .or_else(|| choice.choices.first())
            .cloned()
            .unwrap_or_default();
        let hit = ext
            .manifest
            .presets
            .iter()
            .find(|p| p.values == values && p.gamma == gamma)
            .map(|p| p.value.clone())
            .unwrap_or(custom);
        out.push((choice.key.clone(), hit));
    }
    for (i, key) in preview.gain_keys.iter().enumerate() {
        out.push((key.clone(), values[i].to_string()));
    }
    for (i, key) in preview.bias_keys.iter().enumerate() {
        out.push((key.clone(), values[3 + i].to_string()));
    }
    if let Some(key) = preview.gamma_key.as_deref() {
        out.push((key.to_string(), gamma.to_string()));
    }
    out
}

fn open(ctx: &mut EventCtx, ext: &Extension, config: &Rc<RefCell<Config>>, mode: Mode) {
    let id = ext.manifest.id.clone();
    let specs_all = ext.manifest.settings.clone();
    if specs_all.is_empty() {
        return;
    }
    // 这一扇窗该渲染哪些项：`preview` 消费掉的归运行时参数窗，其余归设置窗。
    // 两扇窗各拿一半——同一条清单不许被它们重复渲染（齿轮点开是六条滑块，正是
    // 2026-11-06 用户判「这不对」的那一次）。
    let live_keys = ext.manifest.live_param_keys();
    let is_live_key = |spec: &SettingSpec| {
        live_keys
            .as_ref()
            .is_some_and(|keys| keys.iter().any(|k| k == &spec.key))
    };
    let specs: Vec<SettingSpec> = match (&mode, &live_keys) {
        (Mode::Run(_), Some(_)) => specs_all
            .iter()
            .filter(|s| is_live_key(s))
            .cloned()
            .collect(),
        (Mode::Settings, Some(_)) => specs_all
            .iter()
            .filter(|s| !is_live_key(s))
            .cloned()
            .collect(),
        _ => specs_all.clone(),
    };
    // 设置窗没有非参数项：齿轮那边已经拦过，这里再兜一层，绝不开一扇空窗。
    if specs.is_empty() {
        return;
    }
    // 初值：参数窗读当前显示状态（每张图都从原图起步），设置窗才读落盘那一份。
    let stored = crate::extensions::settings::load(&id);
    let seeds: Vec<(String, String)> = match (&mode, &live_keys) {
        (Mode::Run(o), Some(_)) => {
            let cur = o.state.tint.get().filter(|t| t.path == o.path);
            live_seed(ext, cur.as_ref())
        }
        _ => stored.clone(),
    };
    let value_of = |key: &str, default: &str| {
        seeds
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| default.to_string())
    };
    let bounds: Vec<Bound> = specs
        .iter()
        .map(|s| Bound::from_spec(s, &value_of(&s.key, &s.default)))
        .collect();
    let sync = match (&mode, &live_keys) {
        // 只有"声明式 op 且不起进程"的编辑型才走实时；命令型 run_ui 仍是老路。
        (Mode::Run(o), Some(_)) if o.on_apply.is_none() => {
            build_sync(ext, &specs, &bounds, &o.state, o.path.clone())
        }
        _ => None,
    };
    let footer = match (&mode, &sync) {
        (Mode::Settings, _) => Footer::Settings,
        (_, Some(_)) => Footer::Live,
        _ => Footer::Legacy,
    };
    let actions = match &mode {
        Mode::Run(o) => o.actions.clone(),
        Mode::Settings => None,
    };
    log::info!("[plugin-ui] id={} mode={} verdict=open", id, footer.label());
    if let Some(sync) = &sync {
        // 开窗即让看图窗与这六条滑块对齐（恒等参数不会写 tint，一帧都不多算）。
        sync.publish();
    }

    // 捕获态：某一行正在等按键（下标）。窗口级 `on_shortcut` 读它。
    let capturing = signal(None::<usize>);
    // 每个 hotkey 行一枚"框里显示什么"的信号：`hotkey_box` 只认文本信号。
    let hotkey_texts: Vec<Signal<String>> = hotkey_spec_indices(&specs)
        .into_iter()
        .filter_map(|i| specs.get(i))
        .map(|s| {
            let raw = value_of(&s.key, &s.default);
            signal(
                Hotkey::from_spec(&raw)
                    .map(|h| h.pretty())
                    .unwrap_or_else(|| {
                        if raw.is_empty() {
                            "（空）".to_string()
                        } else {
                            raw.clone()
                        }
                    }),
            )
        })
        .collect();

    let spec_save = specs.clone();
    let bound_save = bounds.clone();
    let stored_save = stored.clone();
    let id_save = id.clone();
    let cfg_save = config.clone();
    let launch = match &mode {
        Mode::Run(o) => o.on_apply.clone(),
        Mode::Settings => None,
    };
    let confirm = move |ctx: &mut EventCtx| {
        let mut items: Settings = stored_save.clone();
        for (spec, bound) in spec_save.iter().zip(bound_save.iter()) {
            let v = bound.value();
            // 整数在解析期已验过默认值，但用户可以在框里改成任意文本。
            if spec.kind == SettingKind::Int && v.parse::<i64>().is_err() {
                ctx.toast_err(format!("「{}」得是整数", spec.label));
                return;
            }
            if spec.kind == SettingKind::Hotkey && !v.is_empty() && Hotkey::from_spec(&v).is_none()
            {
                ctx.toast_err(format!("「{}」不是宿主认得的键串", spec.label));
                return;
            }
            match items.iter_mut().find(|(k, _)| k == &spec.key) {
                Some(slot) => slot.1 = v,
                None => items.push((spec.key.clone(), v)),
            }
        }
        if let Err(e) = crate::extensions::settings::save(&id_save, &items) {
            ctx.toast_err(format!("保存设置失败：{e}"));
            return;
        }
        // 热键/启停都可能变了：动态热键表要重算（它读的就是这份 settings.json）。
        crate::extensions::invalidate_installed_cache();
        // 新绑的键 = 唯一持有者：宿主或别的插件里同一把键一律清空
        //（用户口径：最后绑定的人赢，旧的不留）。
        if let Some(spec) = spec_save
            .iter()
            .zip(bound_save.iter())
            .find(|(s, _)| s.kind == SettingKind::Hotkey)
            .map(|(_, b)| b.value())
        {
            if let Some(hk) = Hotkey::from_spec(&spec) {
                crate::extensions::vacate_hotkey(&mut cfg_save.borrow_mut(), &hk, Some(&id_save));
            }
        }
        // 快捷键页那张「插件快捷键」卡显示的是同一个文件 —— 当场把它拉到新值，
        // 用户不用关窗重开才看见（两处同步＝同一真源 + 显示层跟着刷新）。
        super::settings::shortcuts::sync_plugin_hotkey_texts(
            &crate::core::config::Config::load(),
            None,
        );
        log::info!("插件「{id_save}」设置已保存");
        // 命令型 run_ui：按「应用」= 值落盘后起进程。声明式实时变换的编辑型
        // 已经在拖动时上屏，参数窗不再有"应用"这一颗。
        // ★ 失败时不关窗、也不在将关的窗口上弹成功 toast——2026-11-06 用户报「秒退」，
        //   最可疑的就是成功提示与 request_close 抢同一帧。成功由图像本身回答。
        let close = match launch.as_ref() {
            Some(apply) => {
                if apply(ctx) {
                    log::info!("[plugin-ui] id={} verdict=launch-ok", id_save);
                    true
                } else {
                    log::warn!("[plugin-ui] id={} verdict=launch-failed", id_save);
                    false
                }
            }
            None => true,
        };
        if close {
            ctx.request_close();
        }
    };
    let id_cancel = id.clone();
    let cancel = move |ctx: &mut EventCtx| {
        // 参数窗没有「取消」：实时调色是"看着图改"，要反悔点「还原」。
        // 这条分支只剩设置窗与老路在用，行为与以前一致。
        log::info!("[plugin-ui] id={} verdict=cancel", id_cancel);
        ctx.request_close();
    };

    // 捕获：窗口里按下键 → 写进那一行的原始值信号 + 框里的显示文本。
    let cap_sig = capturing;
    let hotkey_bounds: Vec<(usize, Signal<String>)> = bounds
        .iter()
        .enumerate()
        .filter_map(|(i, b)| match b {
            Bound::Hotkey(s) => Some((i, *s)),
            _ => None,
        })
        .collect();
    let hotkey_texts_sc = hotkey_texts.clone();
    let on_shortcut =
        move |_ctx: &mut windui::event::ShortcutCtx, ev: windui::event::KeyEvent| -> bool {
            let Some(row) = cap_sig.get() else {
                return false;
            };
            if !ev.pressed {
                return true;
            }
            let Some(ki) = super::to_key_input(&ev) else {
                return true;
            };
            let Some(hk) = Hotkey::from_event(&ki) else {
                return true;
            };
            if let Some((_, raw)) = hotkey_bounds.iter().find(|(i, _)| *i == row) {
                raw.set(hk.to_spec());
            }
            // 显示文本按"这一行是第几个 hotkey 行"折算。
            if let Some(pos) = hotkey_bounds.iter().position(|(i, _)| *i == row) {
                if let Some(t) = hotkey_texts_sc.get(pos) {
                    t.set(hk.pretty());
                }
            }
            cap_sig.set(None);
            true
        };

    let hk_texts = hotkey_texts.clone();
    let cap = capturing;
    let bounds_ui = bounds.clone();
    let sync_ui = sync.clone();
    let confirm_text = footer.confirm_text();
    let confirm_ui: Option<ClickFn> = confirm_text.map(|_| {
        let f = confirm.clone();
        Rc::new(move |ctx: &mut EventCtx| f(ctx)) as Rc<dyn Fn(&mut EventCtx)>
    });
    let cancel_ui: Option<ClickFn> = match footer {
        // 实时参数窗没有取消：关窗就是结束（参数已在 LiveState 里，图已经变了）。
        Footer::Live => None,
        _ => Some(Rc::new(move |ctx: &mut EventCtx| cancel(ctx))),
    };
    // ★ 先关旧窗再开新窗，但**两个键必须不同**：`single` 已开时只聚焦、不重建内容
    //   （快捷键页改过值之后再点齿轮，留在屏幕上的会是开窗那一刻的旧值）；而关窗是
    //   投递、开窗是即时，同键既关又开会让滞留的 WM_CLOSE 把刚开出来的新窗收走
    //   —— 2026-11-04 用户实测「齿轮点不动，窗闪一下就没了」。见 `singleton::recycle`。
    let (old_key, win_key) = super::singleton::recycle(&format!("plugin-settings:{id}"));
    let title = match &mode {
        // 设置窗与参数窗同名会让人以为是同一扇：加个尾巴区分（参数窗就是插件名）。
        Mode::Settings => format!("{} 设置", ext.manifest.name),
        Mode::Run(_) => ext.manifest.name.clone(),
    };
    ctx.close_window(old_key);
    ctx.open_window(
        Window::new(title, DIALOG_W, 300)
            .bg(to_color(BG))
            .resizable(false)
            .centered(true)
            .owned(true)
            .single(win_key)
            // 下限与裸数据参数窗共用同一格：内容多高窗就多高，按钮下面不留空条。
            .fit_content(DIALOG_FIT_MIN, 640)
            .on_shortcut(on_shortcut)
            .content(move || {
                // 行版式与间距全交给 `dialog_footer::shell`：这里只按 schema 攒行。
                // 行内缩传 0 —— 对话框容器自己就是 `DIALOG_PAD`，再缩就与底栏按钮错开。
                let mut rows: Vec<Element> = Vec::new();
                let mut hk_seen = 0usize;
                for (row, (spec, bound)) in specs.iter().zip(bounds_ui.iter()).enumerate() {
                    let control = match bound {
                        Bound::Bool(sig) => Element::switch(*sig),
                        Bound::Choice { idx, labels, .. } => {
                            Element::dropdown(labels.clone(), *idx).width(CONTROL_GROUP_W)
                        }
                        Bound::Text(sig) => Element::text_input(*sig, "").width(CONTROL_GROUP_W),
                        Bound::Range { norm, lo, hi } => {
                            let (lo, hi) = (*lo, *hi);
                            let text = range_value_signal(*norm, lo, hi);
                            let slider = match &sync_ui {
                                Some(sync) => Element::leaf().widget(LiveSlider {
                                    inner: Slider::new(*norm),
                                    value: *norm,
                                    sync: sync.clone(),
                                }),
                                None => Element::slider(*norm),
                            };
                            Element::row()
                                .cross(Align::Center)
                                .child(slider.width(128))
                                .child(Element::label_signal(text).width(60))
                                .width(CONTROL_GROUP_W)
                        }
                        Bound::Hotkey(_) => {
                            let text = hk_texts
                                .get(hk_seen)
                                .copied()
                                .unwrap_or_else(|| signal("（空）".to_string()));
                            let cap_click = cap;
                            let text_click = text;
                            let cap_blur = cap;
                            let text_blur = text;
                            hk_seen += 1;
                            hotkey_box(
                                text,
                                move |_ctx| {
                                    // 点框即清空再进捕获（快捷键页同款）：捕获中失焦留空 =
                                    // 解绑，这是唯一的解绑通道。
                                    cap_click.set(Some(row));
                                    text_click.set("按下按键…".to_string());
                                },
                                move |_ctx| {
                                    if cap_blur.get() == Some(row) {
                                        cap_blur.set(None);
                                        if text_blur.get() == "按下按键…" {
                                            text_blur.set("（空）".to_string());
                                        }
                                    }
                                },
                            )
                        }
                    };
                    // 两列版式与设置页共用同一份（标签列 `COL1_W`、控件列左缘同一条线），
                    // `hotkey_box` 本来就吃 `CONTROL_GROUP_W`，另外两控件这里对齐它。
                    rows.push(option_row_inset(&spec.label, control, 0));
                }
                // 底栏只走 `dialog_footer`（见 `docs/界面版式规范.md` §2）：
                // 另存 / 还原是这次编辑的两个出口，右键菜单不再重复挂；确定 / 取消按模式给。
                let mut right: Vec<Element> = Vec::new();
                if let Some(a) = &actions {
                    let save = a.save.clone();
                    let revert = a.revert.clone();
                    right.push(super::dialog_footer::action("另存为…", move |ctx| {
                        save(ctx)
                    }));
                    right.push(super::dialog_footer::action("还原", move |ctx| {
                        revert(ctx)
                    }));
                }
                if let (Some(text), Some(confirm_fn)) = (confirm_text, &confirm_ui) {
                    let c = confirm_fn.clone();
                    right.push(super::dialog_footer::action(text, move |ctx| c(ctx)));
                }
                if let Some(cancel_fn) = &cancel_ui {
                    let c = cancel_fn.clone();
                    right.push(super::dialog_footer::action("取消", move |ctx| c(ctx)));
                }
                super::dialog_footer::shell(rows, right)
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: SettingKind, default: &str) -> SettingSpec {
        SettingSpec {
            key: "k".into(),
            label: "L".into(),
            kind,
            default: default.into(),
            choices: Vec::new(),
            labels: Vec::new(),
            range: None,
        }
    }

    #[test]
    fn bool_values_round_trip_through_the_stored_form() {
        let s = spec(SettingKind::Bool, "1");
        let b = Bound::from_spec(&s, "1");
        assert_eq!(b.value(), "1");
        let b = Bound::from_spec(&s, "0");
        assert_eq!(b.value(), "0");
        // 脏值（旧版本留下）按关处理，不许把控件建成第三态。
        let b = Bound::from_spec(&s, "yes");
        assert_eq!(b.value(), "0");
    }

    #[test]
    fn hotkey_rows_are_indexed_by_their_own_kind_not_by_spec_position() {
        // 夹具＝`checkup` 的真实形状：[choice, hotkey]。若按"第几个设置项"取，
        // 快捷键框会读到 choice 那一行（默认 "8"）——2026-10-31 实机就是显示成 8。
        let mut palette = spec(SettingKind::Choice, "8");
        palette.choices = vec!["4".into(), "8".into(), "16".into()];
        let hotkey = spec(SettingKind::Hotkey, "");
        let specs = vec![palette, hotkey];
        assert_eq!(hotkey_spec_indices(&specs), vec![1], "hotkey 在第二格");

        // 只有 hotkey 的清单（scan）：下标是 0，且总数必须等于 hotkey 行数。
        let only_hotkey = vec![spec(SettingKind::Hotkey, "")];
        assert_eq!(hotkey_spec_indices(&only_hotkey), vec![0]);
        assert_eq!(hotkey_spec_indices(&specs).len(), 1);
    }

    #[test]
    fn choice_maps_index_back_to_the_declared_value() {
        let mut s = spec(SettingKind::Choice, "8");
        s.choices = vec!["4".into(), "8".into(), "16".into()];
        s.labels = vec!["4 色".into(), "8 色".into(), "16 色".into()];
        let b = Bound::from_spec(&s, "16");
        assert_eq!(b.value(), "16", "存的是取值不是下标");
        // 取值不在表里 ⇒ 回落第一项（不是原样回写一个渲染器不认识的串）。
        let b = Bound::from_spec(&s, "12");
        assert_eq!(b.value(), "4");
        assert_eq!(choice_value(&s.choices, 99), "", "越界不 panic");
    }

    #[test]
    fn slider_range_round_trips_through_the_normalised_signal() {
        let mut s = spec(SettingKind::Int, "100");
        s.range = Some((-400, 400));
        assert_eq!(Bound::from_spec(&s, "100").value(), "100");
        assert_eq!(Bound::from_spec(&s, "-100").value(), "-100");
        assert_eq!(Bound::from_spec(&s, "999").value(), "400", "越界钳到上界");
        assert_eq!(
            Bound::from_spec(&s, "nope").value(),
            "100",
            "坏文本回落 default"
        );
        // 没声明范围的整数项仍是文本框，存什么读什么。
        let plain = spec(SettingKind::Int, "7");
        assert_eq!(Bound::from_spec(&plain, "42").value(), "42");
    }

    fn test_sync() -> (Rc<SyncState>, LiveState) {
        let state = LiveState::new();
        let s = Rc::new(SyncState {
            norms: std::array::from_fn(|_| signal(0.0f32)),
            ranges: [
                (-400, 400),
                (-400, 400),
                (-400, 400),
                (-255, 255),
                (-255, 255),
                (-255, 255),
                (100, 100), // 老清单：第七格恒等
            ],
            preset_idx: signal(0usize),
            custom_idx: 0,
            presets: vec![(1, [-100, -100, -100, 255, 255, 255], 100)],
            state: state.clone(),
            path: PathBuf::from("fixture.png"),
            reset_seen: Cell::new(0),
            last_preset: Cell::new(0),
        });
        (s, state)
    }

    #[test]
    fn choosing_a_preset_writes_all_six_sliders_and_publishes() {
        let (sync, state) = test_sync();
        sync.preset_idx.set(1);
        sync.poll();
        assert_eq!(sync.values().0, [-100, -100, -100, 255, 255, 255]);
        let tint = state.tint.get().expect("非恒等参数要上屏");
        assert_eq!(tint.values, [-100, -100, -100, 255, 255, 255]);
        assert_eq!(tint.gamma(), 100);
        assert_eq!(tint.path, PathBuf::from("fixture.png"));
        assert_eq!(tint.gain(), [-100, -100, -100]);
        assert_eq!(tint.bias(), [255, 255, 255]);
        // 同一帧被七条 LiveSlider 各跑一次 poll 也不能重复写。
        sync.poll();
        assert_eq!(sync.values().0, [-100, -100, -100, 255, 255, 255]);
    }

    #[test]
    fn dragging_a_slider_falls_back_to_custom_and_publishes_immediately() {
        let (sync, state) = test_sync();
        sync.preset_idx.set(1);
        sync.poll();
        // 拖红通道增益到 200（0..1 归一化 = (200+400)/800 = 0.75）。
        sync.norms[0].set(0.75);
        sync.on_slider();
        assert_eq!(sync.preset_idx.get(), 0, "必须回落「自定义」");
        assert_eq!(state.tint.get().expect("已上屏").values[0], 200);
        // 数值标签读的是同一个源信号，set 完当帧就能拿到。
        assert_eq!(range_value_signal(sync.norms[0], -400, 400).get(), "200");
    }

    #[test]
    fn revert_resets_sliders_and_clears_the_live_tint() {
        let (sync, state) = test_sync();
        sync.set_values([-100, -100, -100, 255, 255, 255], 100);
        sync.publish();
        assert!(state.tint.get().is_some());
        state.reset.set(state.reset.get() + 1);
        sync.poll();
        assert_eq!(sync.values().0, crate::core::pixels::CHANNEL_NEUTRAL);
        assert_eq!(sync.values().1, crate::core::pixels::GAMMA_NEUTRAL);
        assert!(state.tint.get().is_none(), "还原后一帧都不该多算");
        assert_eq!(sync.preset_idx.get(), 0);
    }

    #[test]
    fn neutral_parameters_never_publish_a_tint() {
        let (sync, state) = test_sync();
        sync.set_values(
            crate::core::pixels::CHANNEL_NEUTRAL,
            crate::core::pixels::GAMMA_NEUTRAL,
        );
        sync.publish();
        assert!(state.tint.get().is_none());
        let t = LiveTint {
            path: PathBuf::from("fixture.png"),
            values: crate::core::pixels::CHANNEL_NEUTRAL,
            gamma: crate::core::pixels::GAMMA_NEUTRAL,
        };
        assert!(t.is_neutral(), "恒等参数不该被当成一次调色");
    }

    /// 第七格 Gamma：单独拖动它也要上屏、也要参与预设回填；六数恒等不等于中性。
    #[test]
    fn gamma_only_tint_is_not_neutral_and_preset_writes_it_back() {
        let state = LiveState::new();
        let sync = Rc::new(SyncState {
            norms: std::array::from_fn(|_| signal(0.0f32)),
            ranges: [
                (-400, 400),
                (-400, 400),
                (-400, 400),
                (-255, 255),
                (-255, 255),
                (-255, 255),
                (30, 300),
            ],
            preset_idx: signal(0usize),
            custom_idx: 0,
            // 六数恒等 + Gamma 180 = 「暗部补偿」预设；只看六数会误判成「原图」。
            presets: vec![(1, crate::core::pixels::CHANNEL_NEUTRAL, 180)],
            state: state.clone(),
            path: PathBuf::from("fixture.png"),
            reset_seen: Cell::new(0),
            last_preset: Cell::new(0),
        });
        sync.preset_idx.set(1);
        sync.poll();
        let (v, g) = sync.values();
        assert_eq!(v, crate::core::pixels::CHANNEL_NEUTRAL);
        assert_eq!(g, 180, "预设必须把第七格也写回滑块");
        let tint = state.tint.get().expect("Gamma 单独生效也要上屏");
        assert!(!tint.is_neutral());
        assert_eq!(tint.gamma(), 180);
        assert_eq!(tint.values, crate::core::pixels::CHANNEL_NEUTRAL);

        // 还原：六数与 Gamma 一起归位，显示层不留残影。
        state.reset.set(state.reset.get() + 1);
        sync.poll();
        assert_eq!(sync.values().1, crate::core::pixels::GAMMA_NEUTRAL);
        assert!(state.tint.get().is_none());
    }

    /// ★ 用户报「拖动时不实时变、松手才变」的根因判据。
    ///
    /// 框架 Slider 在 Move 里**确实**写了值（所以不是控件坏了），但 `call_on_event`
    /// 把 Move 期间的 Signal::set 降级成「本控件局部脏区」⇒ 派生数值标签、另一个
    /// 窗口都不在那一格里，只有 Down/Up 的 Layout 级失效才让它们一起刷。
    /// 这里用真 Tree + 真指针事件把「值」和「失效强度」分别量出来。
    #[test]
    fn live_slider_changes_on_move_and_escalates_to_a_full_repaint() {
        use windui::core::{DamageReq, NodeId, Tree};
        use windui::event::{MouseButton, PointerEvent, PointerKind};
        use windui::geometry::{Point, Size};
        use windui::text::NullTextEngine;

        fn drive(root: windui::ui::Element, value: Signal<f32>) -> (f32, DamageReq) {
            let mut tree = Tree::new();
            let id = root.build(&mut tree);
            tree.root = Some(id);
            tree.layout_root(Size::new(200, 40), &mut NullTextEngine);
            let slider: NodeId = tree.get(id).unwrap().children[0];
            let b = tree.abs_bounds(slider);
            let y = b.y + b.h / 2;
            let (mut hover, mut capture) = (None, None);
            tree.dispatch_pointer(
                PointerEvent::single(
                    PointerKind::Down,
                    Point::new(b.x + b.w - 10, y),
                    MouseButton::Left,
                ),
                &mut hover,
                &mut capture,
            );
            let down = value.get();
            assert!(down > 0.5, "右端按下应把值拉到上半段，实际 {down}");
            let res = tree.dispatch_pointer(
                PointerEvent::single(
                    PointerKind::Move,
                    Point::new(b.x + 10, y),
                    MouseButton::Left,
                ),
                &mut hover,
                &mut capture,
            );
            (value.get(), res.damage)
        }

        // 对照组：裸框架滑块。值跟着 Move 变了，但脏区只有自己那一格。
        let plain = signal(0.0f32);
        let tree_plain = windui::ui::Element::col()
            .width(200)
            .height(40)
            .child(windui::ui::Element::slider(plain).width(100));
        let (plain_value, plain_damage) = drive(tree_plain, plain);
        assert!(plain_value < 0.3, "裸滑块的 Move 也应该改值：{plain_value}");
        assert!(
            matches!(plain_damage, DamageReq::Rect(_)),
            "裸滑块 Move 只有局部脏区，正是「松手才变」的根因：{plain_damage:?}"
        );

        // 包装组：值一样实时，脏区升整窗，派生数字与看图窗同帧刷新。
        let value = signal(0.0f32);
        let state = LiveState::new();
        let mut norms: [Signal<f32>; 7] = std::array::from_fn(|_| signal(0.0f32));
        norms[0] = value;
        let sync = Rc::new(SyncState {
            norms,
            ranges: [
                (-400, 400),
                (-400, 400),
                (-400, 400),
                (-255, 255),
                (-255, 255),
                (-255, 255),
                (100, 100), // 老清单：第七格恒等
            ],
            preset_idx: signal(0usize),
            custom_idx: 0,
            presets: Vec::new(),
            state: state.clone(),
            path: PathBuf::from("fixture.png"),
            reset_seen: Cell::new(0),
            last_preset: Cell::new(0),
        });
        let live = windui::ui::Element::col().width(200).height(40).child(
            windui::ui::Element::leaf()
                .widget(LiveSlider {
                    inner: Slider::new(value),
                    value,
                    sync,
                })
                .width(100),
        );
        let (live_value, live_damage) = drive(live, value);
        assert!(live_value < 0.3, "包装后 Move 仍要实时改值：{live_value}");
        assert_eq!(
            live_damage,
            DamageReq::Full,
            "必须升整窗，派生标签才会同帧变"
        );
        assert!(
            state.tint.get().is_some(),
            "Move 这一帧就应把参数发布到看图窗，不等松手"
        );
    }

    /// 真实清单形状的夹具：六个数值 + 预设 + preview + 一组反相预设。
    fn live_ext() -> Extension {
        let json = r#"{
            "schema":1,"id":"t.ch","name":"通道调色","version":"1.0.0","protocol":1,
            "min_host":"0.1.0","kind":"editor","entry":"t.exe",
            "capabilities":["image.read","image.write","settings.read","settings.write"],
            "platforms":["windows-x64"],
            "settings":[
              {"key":"preset","label":"预设","type":"choice",
               "choices":["custom","invert"],"labels":["自定义","反相"]},
              {"key":"g0","label":"G0","type":"int","min":-400,"max":400,"default":"100"},
              {"key":"g1","label":"G1","type":"int","min":-400,"max":400,"default":"100"},
              {"key":"g2","label":"G2","type":"int","min":-400,"max":400,"default":"100"},
              {"key":"b0","label":"B0","type":"int","min":-255,"max":255,"default":"0"},
              {"key":"b1","label":"B1","type":"int","min":-255,"max":255,"default":"0"},
              {"key":"b2","label":"B2","type":"int","min":-255,"max":255,"default":"0"}
            ],
            "preview":{"op":"channel-linear","gain":["g0","g1","g2"],"bias":["b0","b1","b2"]},
            "presets":{"invert":{"gain":[-100,-100,-100],"bias":[255,255,255]}}
        }"#;
        Extension {
            manifest: crate::core::ext::Manifest::parse(json).expect("valid manifest"),
            dir: PathBuf::new(),
            enabled: true,
            blocked: false,
            isolated: false,
        }
    }

    /// ★ 用户 2026-11-06：每张图的初始状态都该一样，全都默认原图。
    ///
    /// 这条判据钉的是"参数窗的初值从哪来"——**不读 settings.json**，所以没调过的图
    /// 拿到的是恒等参数；调过的图按它自己的那条 `tint` 回填。
    #[test]
    fn live_seed_starts_every_image_from_the_original() {
        let ext = live_ext();
        let seed = live_seed(&ext, None);
        assert_eq!(seed[0], ("preset".to_string(), "custom".to_string()));
        assert_eq!(seed[1], ("g0".to_string(), "100".to_string()));
        assert_eq!(seed[4], ("b0".to_string(), "0".to_string()));
        assert_eq!(seed.len(), 7, "六个数 + 预设，不多不少");

        // 已经调成反相：重开窗要说「反相」，不能一律写自定义（那等于把刚选的滤镜忘掉）。
        let tint = LiveTint {
            path: PathBuf::from("a.png"),
            values: [-100, -100, -100, 255, 255, 255],
            gamma: crate::core::pixels::GAMMA_NEUTRAL,
        };
        let seed = live_seed(&ext, Some(&tint));
        assert_eq!(seed[0], ("preset".to_string(), "invert".to_string()));
        assert_eq!(seed[1], ("g0".to_string(), "-100".to_string()));
        assert_eq!(seed[4], ("b0".to_string(), "255".to_string()));

        // 不是预设的那种组合 ⇒ 回落自定义，数值照实回填。
        let tint = LiveTint {
            path: PathBuf::from("a.png"),
            values: [130, 100, 100, 7, 0, 0],
            gamma: crate::core::pixels::GAMMA_NEUTRAL,
        };
        let seed = live_seed(&ext, Some(&tint));
        assert_eq!(seed[0], ("preset".to_string(), "custom".to_string()));
        assert_eq!(seed[1], ("g0".to_string(), "130".to_string()));
        assert_eq!(seed[4], ("b0".to_string(), "7".to_string()));
    }

    /// 调色不是保存：实时参数窗的底栏没有「完成」也没有「取消」。
    /// 老路（无 preview）与设置窗的按钮一字不变——加法式改造不许翻旧账。
    #[test]
    fn the_live_footer_has_no_confirm_or_cancel() {
        assert!(
            Footer::Live.confirm_text().is_none(),
            "调色是看着图改，不该有提交动作"
        );
        assert_eq!(Footer::Settings.confirm_text(), Some("确定"));
        assert_eq!(Footer::Legacy.confirm_text(), Some("应用"));
    }

    #[test]
    fn slider_number_label_follows_the_signal_immediately() {
        let s = signal(0.0f32);
        let label = range_value_signal(s, -400, 400);
        assert_eq!(label.get(), "-400", "最左端 = 下界");
        s.set(0.625);
        assert_eq!(label.get(), "100", "拖到 62.5% 就该显示 100，不等松手");
        s.set(1.0);
        assert_eq!(label.get(), "400", "最右端 = 上界");
    }
}
