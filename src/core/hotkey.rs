//! 快捷键的**数据模型与匹配**：动作枚举、单条快捷键、整组绑定。
//!
//! 参考 CurSeen 控制面板的「快捷键捕获框」（[`crate::ui::settings`] 里的
//! `hotkey_box` 视觉正本）：用户点框进入捕获态，下一键被 `Hotkey::from_event`
//! 抓成一条 `Hotkey` 写回配置；同一条键若已被别的功能占用，落盘时强制唯一
//! （vacate 旧主，见 `Hotkeys::set_binding`）。
//! ★ m278：**裸空格是保留键**，独占给「空格看图」（`platform::spacehook`）。那个钩子
//! 恒 `CallNextHookEx`、永不吞键，所以把别的动作绑到裸空格会让一次按键同时触发
//! 预览与该动作——真冲突。绑定 / ini 回读 / 插件热键 / 设置页捕获四处一律避开它。
//!
//! 设计要点：
//! - 一个动作只绑**一条**键（2026-10-13 用户口径「只允许单个」；此前「上一张」
//!   曾默认同时是 `←` 与 `PageUp`）。`Hotkeys` 仍以 `Vec<Hotkey>` 存储以保
//!   INI `hotkeys=动作id:键` 的格式兼容，但单条不变式由三个入口共同守住：
//!   出厂默认只给一把、`Hotkeys::from_ini` 多键只取第一把、
//!   `Hotkeys::set_binding` 整体替换。
//! - 修饰键 Ctrl/Alt/Shift/Win 与主键分开存；匹配时**精确**比对修饰键组合，
//!   所以「按着 Ctrl 的 F」与裸 `F` 是两条不同的快捷键。
//! - 持久化走 RuPix 现有手写 INI：整组序列化成 `hotkeys=` 一个键（见
//!   `config::Config`），既不与「一个设置一项键」的约定冲突，又把 13 个动作
//!   收进单一可解析的值里。

/// 与 windui 解耦的按键表示：字段对齐 `windui::event::Key`，由 ui 层在边界处翻译。
/// 放进 core 是为了让快捷键的「数据模型 + 匹配 + 序列化」完全脱离 UI 框架，
/// core 单测可无头构造（这也是分层门禁要求 core 禁引 windui 的直接原因）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Enter,
    Backspace,
    Delete,
    Insert,
    ContextMenu,
    /// Esc：现可绑定（设置页捕获框按 Esc 即绑到当前动作），见 2026-10-14 快捷键优化。
    Escape,
    Char(char),
    F(u8),
    /// 小键盘 `+`（与主键盘 `+` 区分，后者是 `Char('+')`）。
    NumpadAdd,
    /// 小键盘 `-`。
    NumpadSubtract,
    /// 小键盘 `*`。
    NumpadMultiply,
    /// 小键盘 `/`。
    NumpadDivide,
}

/// 一次按键事件（修饰键 + 主键），与 `windui::event::KeyEvent` 同构但脱框架。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyInput {
    pub key: VKey,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// 可绑定快捷键的功能。**顺序即下标**：`self as usize` 必须落在 `0..ACTION_COUNT`，
/// 且 [`Action::ALL`] 必须与这个顺序一字不差，否则 `bindings[action.index()]` 越界或错位。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// 上一张。
    Prev,
    /// 下一张。
    Next,
    /// 第一张。
    First,
    /// 最后一张。
    Last,
    /// 放大。
    ZoomIn,
    /// 缩小。
    ZoomOut,
    /// 适应窗口。
    Fit,
    /// 1:1 原尺寸。
    ActualSize,
    /// 旋转 90°。
    Rotate,
    /// 水平镜像。
    Flip,
    /// 暂停/继续动画。
    PauseAnim,
    /// 动画后退一帧（按下去自动进暂停态；见 `Viewer::step_anim`）。
    FramePrev,
    /// 动画前进一帧。
    FrameNext,
    /// 缩略图条开关。
    Filmstrip,
    /// 全屏切换。
    Fullscreen,
    // ── 以下 15 个把右键菜单 / 标题栏按钮的全集也纳入可绑动作（2026-10-14 快捷键优化，
    //    m167 又添了幻灯片放映）。
    //    默认**全部不绑**（避免与既有键撞、也不给老用户制造新拦截），需要就到设置页自绑。
    /// 打开文件对话框。
    Open,
    /// 复制当前图像到剪贴板。
    CopyImage,
    /// 复制当前文件路径到剪贴板。
    CopyPath,
    /// 极简模式开关。
    Minimal,
    /// 棋盘格背景开关。
    Checker,
    /// 属性窗。
    Properties,
    /// 窗口不透明度 +1 档（10%）。
    OpacityUp,
    /// 窗口不透明度 −1 档（10%）。
    OpacityDown,
    /// 设置窗。
    Settings,
    /// 窗口置顶开关。
    Topmost,
    /// 锁定（吞掉关窗/键鼠）开关。
    Lock,
    /// 最小化窗口。
    Minimize,
    /// 最大化 / 还原切换。
    Maximize,
    /// 关闭窗口。
    Close,
    /// 幻灯片放映开关（m167）。到点翻下一张，走的是与键盘/滚轮**同一条** `Viewer::step`。
    Slideshow,
    /// 向上平移一步（m216，WASD；一记一步，范围与拖动共用同一份钳位）。
    PanUp,
    /// 向左平移一步（m216）。
    PanLeft,
    /// 向下平移一步（m216）。
    PanDown,
    /// 向右平移一步（m216）。
    PanRight,
    /// 取色器开关（m217）。只进右键「本图 ▸ 取色器」，不进浮条按钮排。
    Picker,
    /// 主辅视图开关（m322）：主图 + 右栏辅图，联动步长 2；不做 PiP/小窗。
    Companion,
}

impl Action {
    /// 动作总数（也是 `bindings` 数组长度）。
    pub const COUNT: usize = 36;

    /// 全部动作，下标顺序与枚举定义严格一致（见类型注释）。
    pub const ALL: [Action; Action::COUNT] = [
        Action::Prev,
        Action::Next,
        Action::First,
        Action::Last,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::Fit,
        Action::ActualSize,
        Action::Rotate,
        Action::Flip,
        Action::PauseAnim,
        Action::FramePrev,
        Action::FrameNext,
        Action::Filmstrip,
        Action::Fullscreen,
        Action::Open,
        Action::CopyImage,
        Action::CopyPath,
        Action::Minimal,
        Action::Checker,
        Action::Properties,
        Action::OpacityUp,
        Action::OpacityDown,
        Action::Settings,
        Action::Topmost,
        Action::Lock,
        Action::Minimize,
        Action::Maximize,
        Action::Close,
        Action::Slideshow,
        Action::PanUp,
        Action::PanLeft,
        Action::PanDown,
        Action::PanRight,
        Action::Picker,
        Action::Companion,
    ];

    /// 下标（0..COUNT），用于索引 `Hotkeys::bindings`。
    pub fn index(self) -> usize {
        self as usize
    }

    /// 由下标还原；越界返回 `None`（INI 解析防越界）。
    pub fn from_index(i: usize) -> Option<Action> {
        Action::ALL.get(i).copied()
    }

    /// INI 序列化用的稳定 id（与 `from_id` 互为逆）。
    pub fn id(self) -> &'static str {
        match self {
            Action::Prev => "prev",
            Action::Next => "next",
            Action::First => "first",
            Action::Last => "last",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::Fit => "fit",
            Action::ActualSize => "actual",
            Action::Rotate => "rotate",
            Action::Flip => "flip",
            Action::PauseAnim => "pause",
            Action::FramePrev => "frame_prev",
            Action::FrameNext => "frame_next",
            Action::Filmstrip => "filmstrip",
            Action::Fullscreen => "fullscreen",
            Action::Open => "open",
            Action::CopyImage => "copy_img",
            Action::CopyPath => "copy_path",
            Action::Minimal => "minimal",
            Action::Checker => "checker",
            Action::Properties => "props",
            Action::OpacityUp => "op_up",
            Action::OpacityDown => "op_down",
            Action::Settings => "settings",
            Action::Topmost => "topmost",
            Action::Lock => "lock",
            Action::Minimize => "minimize",
            Action::Maximize => "maximize",
            Action::Close => "close",
            Action::Slideshow => "slideshow",
            Action::PanUp => "pan_up",
            Action::PanLeft => "pan_left",
            Action::PanDown => "pan_down",
            Action::PanRight => "pan_right",
            Action::Picker => "picker",
            Action::Companion => "companion",
        }
    }

    /// 由 INI id 还原；未知 id 返回 `None`（旧文件/手改脏值不 panic）。
    pub fn from_id(s: &str) -> Option<Action> {
        match s {
            "prev" => Some(Action::Prev),
            "next" => Some(Action::Next),
            "first" => Some(Action::First),
            "last" => Some(Action::Last),
            "zoom_in" => Some(Action::ZoomIn),
            "zoom_out" => Some(Action::ZoomOut),
            "fit" => Some(Action::Fit),
            "actual" => Some(Action::ActualSize),
            "rotate" => Some(Action::Rotate),
            "flip" => Some(Action::Flip),
            "pause" => Some(Action::PauseAnim),
            "frame_prev" => Some(Action::FramePrev),
            "frame_next" => Some(Action::FrameNext),
            "filmstrip" => Some(Action::Filmstrip),
            "fullscreen" => Some(Action::Fullscreen),
            "open" => Some(Action::Open),
            "copy_img" => Some(Action::CopyImage),
            "copy_path" => Some(Action::CopyPath),
            "minimal" => Some(Action::Minimal),
            "checker" => Some(Action::Checker),
            "props" => Some(Action::Properties),
            "op_up" => Some(Action::OpacityUp),
            "op_down" => Some(Action::OpacityDown),
            "settings" => Some(Action::Settings),
            "topmost" => Some(Action::Topmost),
            "lock" => Some(Action::Lock),
            "minimize" => Some(Action::Minimize),
            "maximize" => Some(Action::Maximize),
            "close" => Some(Action::Close),
            "slideshow" => Some(Action::Slideshow),
            "pan_up" => Some(Action::PanUp),
            "pan_left" => Some(Action::PanLeft),
            "pan_down" => Some(Action::PanDown),
            "pan_right" => Some(Action::PanRight),
            "picker" => Some(Action::Picker),
            "companion" => Some(Action::Companion),
            _ => None,
        }
    }

    /// 设置页/菜单里给人看的功能名。
    pub fn name(self) -> &'static str {
        match self {
            Action::Prev => "上一张",
            Action::Next => "下一张",
            Action::First => "第一张",
            Action::Last => "最后一张",
            Action::ZoomIn => "放大",
            Action::ZoomOut => "缩小",
            Action::Fit => "适应窗口",
            Action::ActualSize => "1:1 原尺寸",
            Action::Rotate => "旋转 90°",
            Action::Flip => "水平镜像",
            Action::PauseAnim => "暂停/继续动画",
            Action::FramePrev => "上一帧",
            Action::FrameNext => "下一帧",
            Action::Filmstrip => "缩略图条",
            Action::Fullscreen => "全屏",
            Action::Open => "打开",
            Action::CopyImage => "复制当前图像（PNG）",
            Action::CopyPath => "复制文件路径",
            Action::Minimal => "极简模式",
            Action::Checker => "棋盘格背景",
            Action::Properties => "属性",
            Action::OpacityUp => "不透明度 +",
            Action::OpacityDown => "不透明度 −",
            Action::Settings => "设置",
            Action::Topmost => "窗口置顶",
            Action::Lock => "锁定",
            Action::Minimize => "最小化",
            Action::Maximize => "最大化 / 还原",
            Action::Close => "关闭窗口",
            Action::Slideshow => "幻灯片放映",
            Action::PanUp => "向上平移",
            Action::PanLeft => "向左平移",
            Action::PanDown => "向下平移",
            Action::PanRight => "向右平移",
            Action::Picker => "取色器",
            Action::Companion => "主辅视图",
        }
    }
}

/// 主键（去掉修饰键后的那一键）。
///
/// 字母/数字/符号统一归到 `Char`，且**一律存大写**——这样 `Shift+F` 与裸 `F`
/// 主键都是 `Char('F')`，区别只在对面的修饰键标志上，匹配才不会因大小写漂移。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Enter,
    Backspace,
    Delete,
    Insert,
    ContextMenu,
    Escape,
    Char(char),
    F(u8),
    /// 小键盘 `+`（与主键盘 `+` 区分，后者是 `Char('+')`）。
    NumAdd,
    /// 小键盘 `-`。
    NumSub,
    /// 小键盘 `*`。
    NumMul,
    /// 小键盘 `/`。
    NumDiv,
}

/// 一条快捷键：修饰键 + 主键。
///
/// `PartialEq` 直接比较「修饰键集合 + 主键」——这正是唯一性判据（见 `set_binding`）：
/// 两条 `Hotkey` 相等当且仅当用户按出的是同一个组合。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub base: Base,
}

impl Hotkey {
    /// ★ m278：「空格看图」独占**裸空格**。带修饰键的 `Ctrl+Space` 之类不触发
    /// 预览（spacehook 在 Ctrl/Alt/Shift/Win 按下时放过），所以只锁裸键。
    pub fn is_reserved(&self) -> bool {
        self.base == Base::Space && !self.ctrl && !self.alt && !self.shift && !self.win
    }

    /// 把一次按键事件（带修饰键状态）抓成一条快捷键。
    ///
    /// - 主键是纯修饰键（`VKey::Alt` 等）或无法识别的键 → 返回 `None`：捕获态下
    ///   用户按着 Ctrl 的那一下不该被当成「绑了 Ctrl」这条快捷键（见 `runtime::mod`）。
    /// - 修饰键取自事件当时的 `ctrl/alt/shift/meta`（Win = `meta`）。
    pub fn from_event(ev: &KeyInput) -> Option<Hotkey> {
        let base = match ev.key {
            VKey::Left => Base::Left,
            VKey::Right => Base::Right,
            VKey::Up => Base::Up,
            VKey::Down => Base::Down,
            VKey::Home => Base::Home,
            VKey::End => Base::End,
            VKey::PageUp => Base::PageUp,
            VKey::PageDown => Base::PageDown,
            VKey::Space => Base::Space,
            VKey::Enter => Base::Enter,
            VKey::Backspace => Base::Backspace,
            VKey::Delete => Base::Delete,
            VKey::Insert => Base::Insert,
            VKey::ContextMenu => Base::ContextMenu,
            VKey::Escape => Base::Escape,
            VKey::F(n) => Base::F(n),
            VKey::NumpadAdd => Base::NumAdd,
            VKey::NumpadSubtract => Base::NumSub,
            VKey::NumpadMultiply => Base::NumMul,
            VKey::NumpadDivide => Base::NumDiv,
            VKey::Char(c) => Base::Char(c.to_ascii_uppercase()),
        };
        // 纯修饰键 / Tab / 未知键由 ui 层 `to_key_input` 翻译为 `None`，
        // 此处收到的主键恒为可绑定主键（Esc 现也算可绑定主键）。
        Some(Hotkey {
            ctrl: ev.ctrl,
            alt: ev.alt,
            shift: ev.shift,
            win: ev.win,
            base,
        })
    }

    /// 这次按键是否命中本条快捷键（精确比对修饰键 + 主键）。
    pub fn matches(&self, ev: &KeyInput) -> bool {
        match Hotkey::from_event(ev) {
            Some(o) => *self == o,
            None => false,
        }
    }

    /// 规范化的存储串（修饰键在前、主键在后，如 `Ctrl+F`、`F11`、`Left`）。
    /// 既是 INI 落盘格式，也是 `from_spec` 的输入格式。
    pub fn to_spec(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(self.base_token());
        parts.join("+")
    }

    /// 主键的规范 token（大小写无关解析 `from_spec` 的对端）。
    fn base_token(&self) -> String {
        match self.base {
            Base::Left => "Left".to_string(),
            Base::Right => "Right".to_string(),
            Base::Up => "Up".to_string(),
            Base::Down => "Down".to_string(),
            Base::Home => "Home".to_string(),
            Base::End => "End".to_string(),
            Base::PageUp => "PageUp".to_string(),
            Base::PageDown => "PageDown".to_string(),
            Base::Space => "Space".to_string(),
            Base::Enter => "Enter".to_string(),
            Base::Backspace => "Backspace".to_string(),
            Base::Delete => "Delete".to_string(),
            Base::Insert => "Insert".to_string(),
            Base::ContextMenu => "ContextMenu".to_string(),
            Base::Escape => "Escape".to_string(),
            // 分隔符类主键必须编码成文字 token，否则 INI 落盘回读就碎：'+' 是修饰键
            // 分隔符（from_spec 拆 '+'），',' ';' ':' 是 to_ini/from_ini 的值分隔符
            // （多键列表 / 条目 / id 与键表）——裸写回读后不是解析失败就是键被截断。
            Base::Char(c) => match c {
                '+' => "Plus".to_string(),
                ',' => "Comma".to_string(),
                ';' => "Semicolon".to_string(),
                ':' => "Colon".to_string(),
                c => c.to_string(),
            },
            Base::F(n) => f_token(n),
            Base::NumAdd => "NumAdd".to_string(),
            Base::NumSub => "NumSub".to_string(),
            Base::NumMul => "NumMul".to_string(),
            Base::NumDiv => "NumDiv".to_string(),
        }
    }

    /// 给人看的展示串（主键用更友好的字形，如 `←`/`空格`；修饰键仍是 Ctrl/Alt/Shift/Win）。
    pub fn pretty(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(self.base_pretty());
        parts.join("+")
    }

    fn base_pretty(&self) -> String {
        match self.base {
            Base::Left => "←".to_string(),
            Base::Right => "→".to_string(),
            Base::Up => "↑".to_string(),
            Base::Down => "↓".to_string(),
            Base::Home => "Home".to_string(),
            Base::End => "End".to_string(),
            Base::PageUp => "PageUp".to_string(),
            Base::PageDown => "PageDown".to_string(),
            Base::Space => "空格".to_string(),
            Base::Enter => "Enter".to_string(),
            Base::Backspace => "Backspace".to_string(),
            Base::Delete => "Del".to_string(),
            Base::Insert => "Insert".to_string(),
            Base::ContextMenu => "菜单键".to_string(),
            Base::Escape => "Esc".to_string(),
            Base::Char(c) => c.to_string(),
            Base::F(n) => format!("F{n}"),
            Base::NumAdd => "小键盘+".to_string(),
            Base::NumSub => "小键盘-".to_string(),
            Base::NumMul => "小键盘*".to_string(),
            Base::NumDiv => "小键盘/".to_string(),
        }
    }

    /// 从规范串（`to_spec` 的输出，或用户手写）解析；空串/无法识别返回 `None`。
    pub fn from_spec(s: &str) -> Option<Hotkey> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut base: Option<Base> = None;
        for raw in s.split('+') {
            let t = raw.trim().to_ascii_uppercase();
            match t.as_str() {
                "CTRL" => ctrl = true,
                "ALT" => alt = true,
                "SHIFT" => shift = true,
                "WIN" => win = true,
                "LEFT" => base = Some(Base::Left),
                "RIGHT" => base = Some(Base::Right),
                "UP" => base = Some(Base::Up),
                "DOWN" => base = Some(Base::Down),
                "HOME" => base = Some(Base::Home),
                "END" => base = Some(Base::End),
                "PAGEUP" => base = Some(Base::PageUp),
                "PAGEDOWN" => base = Some(Base::PageDown),
                "SPACE" => base = Some(Base::Space),
                "ENTER" => base = Some(Base::Enter),
                "BACKSPACE" => base = Some(Base::Backspace),
                "DELETE" => base = Some(Base::Delete),
                "INSERT" => base = Some(Base::Insert),
                "CONTEXTMENU" => base = Some(Base::ContextMenu),
                "ESCAPE" | "ESC" => base = Some(Base::Escape),
                "NUMADD" => base = Some(Base::NumAdd),
                "NUMSUB" => base = Some(Base::NumSub),
                "NUMMUL" => base = Some(Base::NumMul),
                "NUMDIV" => base = Some(Base::NumDiv),
                // `base_token` 把分隔符类主键写成文字 token，这里逐一还原。
                "PLUS" => base = Some(Base::Char('+')),
                "COMMA" => base = Some(Base::Char(',')),
                "SEMICOLON" => base = Some(Base::Char(';')),
                "COLON" => base = Some(Base::Char(':')),
                other => {
                    if let Some(rest) = other.strip_prefix('F') {
                        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                            // `F` 后跟数字 = 功能键，如 `F11`。
                            // windui 只会发出 F1..=F12；越界值虽然能解析，但永远
                            // 匹配不到，按畸形输入拒绝，让 from_ini 保留原绑定。
                            if let Ok(n) = rest.parse::<u8>() {
                                if (1..=12).contains(&n) {
                                    base = Some(Base::F(n));
                                } else {
                                    return None;
                                }
                            } else {
                                return None;
                            }
                        } else if other.chars().count() == 1 {
                            // 单独的 `F` = 字母 F（如 Fit 默认绑定的字母 F）。
                            base = Some(Base::Char('F'));
                        } else {
                            return None;
                        }
                    } else {
                        // 单字符 = 直接绑那把键。用 `(next, next)` 配对判"恰好一个字符"，
                        // 不再 `count() == 1` 后 `unwrap()`：同一件事，一次遍历、零 panic 源
                        // （§八.1 红线，现由 hygiene_scan 的 `panic_source` 查执行）。
                        let mut cs = other.chars();
                        match (cs.next(), cs.next()) {
                            (Some(c), None) => base = Some(Base::Char(c)),
                            _ => return None,
                        }
                    }
                }
            }
        }
        base.map(|b| Hotkey {
            ctrl,
            alt,
            shift,
            win,
            base: b,
        })
    }
}

/// `F(n)` 的规范 token。
fn f_token(n: u8) -> String {
    match n {
        1 => "F1".to_string(),
        2 => "F2".to_string(),
        3 => "F3".to_string(),
        4 => "F4".to_string(),
        5 => "F5".to_string(),
        6 => "F6".to_string(),
        7 => "F7".to_string(),
        8 => "F8".to_string(),
        9 => "F9".to_string(),
        10 => "F10".to_string(),
        11 => "F11".to_string(),
        12 => "F12".to_string(),
        _ => format!("F{n}"),
    }
}

/// 整组快捷键绑定：下标 = `Action::index()`，每个动作**至多一条**绑定
/// （2026-10-13 用户口径「只允许单个」；`Vec` 只是存储载体，保持 INI 格式兼容）。
///
/// 不复用 `#[derive(Default)]`：默认值是**有意义的出厂快捷键**（即旧版写死的那些键），
/// 不是空绑定，故手写 `Default` 把旧行为原样搬进来（见测试 `defaults_keep_one_key_per_action`）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hotkeys {
    /// 下标 = `Action::index()`。
    pub bindings: [Vec<Hotkey>; Action::COUNT],
}

impl Default for Hotkeys {
    fn default() -> Self {
        // 用 dhk 直构，避免走 `from_spec` 在 '+'/'-' 这类「主键盘须按 Shift 才出」的
        // 符号上丢失 shift 修饰（真实按键事件是带 shift 的，不存 shift 就匹配不上）。
        // 单键化（2026-10-13 用户口径「只允许单个」）：多键动作各保留原列表的第一把
        // （← / → / 小键盘+ / 小键盘- / 0）；退役别名（PageUp、PageDown、空格、
        // '+'、'='、'-'、'F' 等）不再随默认生效，需要就到设置页自行绑。
        let mut b = [(); Action::COUNT].map(|_| Vec::new());
        b[Action::Prev.index()] = vec![dhk(Base::Left, false)];
        b[Action::Next.index()] = vec![dhk(Base::Right, false)];
        b[Action::First.index()] = vec![dhk(Base::Home, false)];
        b[Action::Last.index()] = vec![dhk(Base::End, false)];
        b[Action::ZoomIn.index()] = vec![dhk(Base::NumAdd, false)];
        b[Action::ZoomOut.index()] = vec![dhk(Base::NumSub, false)];
        b[Action::Fit.index()] = vec![dhk(Base::Char('0'), false)];
        b[Action::ActualSize.index()] = vec![dhk(Base::Char('1'), false)];
        b[Action::Rotate.index()] = vec![dhk(Base::Char('R'), false)];
        b[Action::Flip.index()] = vec![dhk(Base::Char('M'), false)];
        b[Action::PauseAnim.index()] = vec![dhk(Base::Char('P'), false)];
        // 逐帧：`,` 后退 / `.` 前进（左手区，与 ← → 翻页不撞；一帧一按是刻意
        // 不绑成方向键——那会让「翻图片」和「翻帧」两套语义共用一把键）。
        b[Action::FramePrev.index()] = vec![dhk(Base::Char(','), false)];
        b[Action::FrameNext.index()] = vec![dhk(Base::Char('.'), false)];
        // 翻页：`[` 上一页 / `]` 下一页。与逐帧同区（右手食指两侧）但**不撞**——
        // 逐帧那两个是 `,` `.`，方括号在它们的右上方一格。多页文档与动画是互斥的
        // 两种图，同一只手换一根手指，不需要另置一套方向键语义。
        b[Action::Filmstrip.index()] = vec![dhk(Base::Char('T'), false)];
        // 不透明度 ±1 档（10%）：带 Ctrl——裸 ↑/↓ 曾被预留作纵向平移，m216 落地
        // WASD 后该预留作废（纵向归 W/S），裸 ↑/↓ 维持无绑定。
        // 这两个动作过去在设置页可见却永不命中（孤儿），2026-10-19 补出厂键。
        b[Action::OpacityUp.index()] = vec![Hotkey {
            ctrl: true,
            ..dhk(Base::Up, false)
        }];
        b[Action::OpacityDown.index()] = vec![Hotkey {
            ctrl: true,
            ..dhk(Base::Down, false)
        }];
        b[Action::Fullscreen.index()] = vec![dhk(Base::F(11), false)];
        // 「关闭窗口」出厂绑 Esc（2026-10-14 用户口径）：看图器通用退出键。全屏里
        // 按 Esc 先退全屏再关窗，由 `canvas::exec_action` 的 Close 臂守住。
        b[Action::Close.index()] = vec![dhk(Base::Escape, false)];
        // WASD 平移（m216）：视口往哪边看就按哪个字母（W=上看，A=左看，S=下看，
        // D=右看）。裸 ↑/↓ 已裁决**不做**纵向平移（Ctrl+↑/↓ 是不透明度档），
        // 2026-10-19 那句「裸 ↑/↓ 留给未来可能的纵向平移」的预留就此作废。
        // 「连按两下到边」随做随撤（用户实测不好用，m216 同日）——平移只有一记一步。
        b[Action::PanUp.index()] = vec![dhk(Base::Char('W'), false)];
        b[Action::PanLeft.index()] = vec![dhk(Base::Char('A'), false)];
        b[Action::PanDown.index()] = vec![dhk(Base::Char('S'), false)];
        b[Action::PanRight.index()] = vec![dhk(Base::Char('D'), false)];
        // 取色器（m217）：ImageGlass 同款 K。低频工具放在菜单里也要有快入口——
        // 菜单项显示它，浮条按钮排不新增按钮（用户口径「一排按钮已经很多了」）。
        b[Action::Picker.index()] = vec![dhk(Base::Char('K'), false)];
        Hotkeys { bindings: b }
    }
}

/// 直构一条无修饰（或显式 shift）的快捷键。
fn dhk(base: Base, shift: bool) -> Hotkey {
    Hotkey {
        ctrl: false,
        alt: false,
        shift,
        win: false,
        base,
    }
}

impl Hotkeys {
    /// 这次按键命中哪个动作；无命中返回 `None`。动作按 [`Action::ALL`] 顺序查，
    /// 其唯一一条绑定命中即返回（单键不变式下没有多绑先后问题）。
    pub fn match_action(&self, ev: &KeyInput) -> Option<Action> {
        for a in Action::ALL {
            for h in &self.bindings[a.index()] {
                if h.matches(ev) {
                    return Some(a);
                }
            }
        }
        None
    }

    /// 这把键当前是否已被某个内置动作占用。
    ///
    /// 用途：动态（插件）热键的冲突裁决——**内置优先**，用户自己配的键位
    /// 不该被插件抢走。见 `extensions::hotkey_table`。
    pub fn uses(&self, h: &Hotkey) -> bool {
        Action::ALL
            .iter()
            .any(|a| self.bindings[a.index()].iter().any(|b| b == h))
    }

    /// 序列化成 INI 值：每个动作写成 `id:spec1,spec2`（无绑定写 `id:`），动作间 `;` 分隔。
    /// 空段照写是**必须**的：解绑过的动作不写，回读时 from_ini 只给写到的动作清默认，
    /// 那次解绑就无声复活了（落盘 ≠ 意图）；空段则被 from_ini 正确清成无绑定。
    pub fn to_ini(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for a in Action::ALL {
            let hs = &self.bindings[a.index()];
            let specs: Vec<String> = hs.iter().map(|h| h.to_spec()).collect();
            out.push(format!("{}:{}", a.id(), specs.join(",")));
        }
        out.join(";")
    }

    /// 从 INI 值解析：
    ///
    /// - 整项为空 / 缺段 / 未知 id：静默跳过（未提到的动作保持默认）。
    /// - 动作的空段（`fit:`）：**明确解绑**——这是 [`Self::to_ini`] 对无绑定动作的
    ///   序列化形态，round-trip 依赖它。
    /// - 非空但没有任何 spec 能解析：warn 并**保留该动作当前绑定**（首次出现即出厂
    ///   默认），不再静默解绑（m51-H）。
    pub fn from_ini(s: &str) -> Hotkeys {
        let mut hk = Hotkeys::default();
        for entry in s.split(';') {
            let Some((id, specs)) = entry.split_once(':') else {
                continue;
            };
            let Some(a) = Action::from_id(id.trim()) else {
                continue;
            };
            let specs = specs.trim();
            if specs.is_empty() {
                hk.bindings[a.index()].clear();
                continue;
            }
            // 单键不变式：旧 ini 形如 `next:Right,PageDown,Space` 的多键段只取
            // 第一把，其余别名随之退役（2026-10-13 单键化）。
            let Some(h) = specs.split(',').find_map(|x| Hotkey::from_spec(x.trim())) else {
                log::warn!("快捷键 {id} 的绑定「{specs}」无法解析，保留该动作默认绑定");
                continue;
            };
            // m278：老 ini 可能把裸空格绑给了某动作 —— 保留键优先，清掉该动作。
            if h.is_reserved() {
                log::warn!("快捷键 {id} 占用了保留键空格（空格看图），已清空");
                hk.bindings[a.index()].clear();
                continue;
            }
            // 该动作以 ini 为准：清空（出厂默认或先前同 id 段）再填入解析到的键。
            // 不清空会叠加出重复绑定，破坏 `ini_round_trip_is_stable`。
            hk.bindings[a.index()].clear();
            // 唯一性与 set_binding 同规：同一条键只属于一个动作，ini 里后出现
            // 的段把它从别的动作手里夺走（写入即夺键），否则两个功能共享一
            // 键，match_action 撞上谁算谁。按行序先来后到，与 set_binding 的
            // 「后来者居上」一致，且与 ini 行序无关（先 clear 了本动作默认）。
            for other in Action::ALL {
                if other.index() != a.index() {
                    hk.bindings[other.index()].retain(|x| *x != h);
                }
            }
            hk.bindings[a.index()].push(h);
        }
        hk
    }

    /// 给右键菜单尾随列 / 工具栏按钮悬停用的展示串：所有绑定用 " / " 连接。
    /// 无任何绑定返回 `None`（不画快捷键列）。
    pub fn display(&self, a: Action) -> Option<String> {
        let bs = &self.bindings[a.index()];
        if bs.is_empty() {
            return None;
        }
        Some(
            bs.iter()
                .map(|h| h.pretty())
                .collect::<Vec<_>>()
                .join(" / "),
        )
    }

    /// 写入某动作的绑定——**整体替换成这一条**（单键不变式，不留旧位、不追加）。
    ///
    /// 写入前先从**所有其它动作**清掉同一条键，保证「每个组合键同一时刻只属于一个
    /// 功能」（唯一性原则，对应需求"每种功能的快捷键基本是唯一的"）。
    pub fn set_binding(&mut self, a: Action, hk: Hotkey) {
        // m278：保留键不许落到任何动作上。捕获路径已先拦一道，这里是最后一道
        // （插件 / 程序化调用都走得进来），两处合起来才叫「锁死」。
        if hk.is_reserved() {
            log::warn!("快捷键：裸空格已保留给「空格看图」，忽略本次绑定");
            return;
        }
        let ai = a.index();
        for other in Action::ALL {
            if other.index() == ai {
                continue;
            }
            self.bindings[other.index()].retain(|x| *x != hk);
        }
        self.bindings[ai] = vec![hk];
    }

    /// 清空某动作的绑定。设置页点捕获框先清空再捕获（CurSeen `hotkey_row` 同款），
    /// 这也是单键化后**唯一的解绑通道**：清空后失焦/Esc 不回填，该动作即停用。
    pub fn clear_binding(&mut self, a: Action) {
        self.bindings[a.index()].clear();
    }

    /// 重设出厂默认（设置页「恢复默认快捷键」按钮用）。
    pub fn reset_defaults(&mut self) {
        *self = Hotkeys::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(key: VKey, shift: bool, ctrl: bool, alt: bool, win: bool) -> KeyInput {
        KeyInput {
            key,
            ctrl,
            alt,
            shift,
            win,
        }
    }

    /// 出厂默认：每个动作**恰好一条**主键（2026-10-13 单键化，各保留原多键列表的
    /// 第一把）；退役的别名键（PageUp/PageDown/空格/+/=/F）一律不再命中。
    #[test]
    fn defaults_keep_one_key_per_action() {
        let hk = Hotkeys::default();
        for a in Action::ALL {
            assert!(
                hk.bindings[a.index()].len() <= 1,
                "{:?} 的默认绑定不止一条（单键不变式）",
                a
            );
        }
        // 保留的主键（逐一对照 `canvas::handle_key` 旧表的第一把）
        assert_eq!(
            hk.match_action(&ev(VKey::Left, false, false, false, false)),
            Some(Action::Prev)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Right, false, false, false, false)),
            Some(Action::Next)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Home, false, false, false, false)),
            Some(Action::First)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::End, false, false, false, false)),
            Some(Action::Last)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::NumpadAdd, false, false, false, false)),
            Some(Action::ZoomIn)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::NumpadSubtract, false, false, false, false)),
            Some(Action::ZoomOut)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('0'), false, false, false, false)),
            Some(Action::Fit)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('1'), false, false, false, false)),
            Some(Action::ActualSize)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('r'), false, false, false, false)),
            Some(Action::Rotate)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('m'), false, false, false, false)),
            Some(Action::Flip)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('p'), false, false, false, false)),
            Some(Action::PauseAnim)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('t'), false, false, false, false)),
            Some(Action::Filmstrip)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::F(11), false, false, false, false)),
            Some(Action::Fullscreen)
        );
        // 逐帧那两把：页与帧合流之后，多页文档也走它们（没有第二套键）。
        assert_eq!(
            hk.match_action(&ev(VKey::Char(','), false, false, false, false)),
            Some(Action::FramePrev)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('.'), false, false, false, false)),
            Some(Action::FrameNext)
        );
        // 退役的别名键：不再命中（单键化的直接后果）
        for retired in [
            VKey::PageUp,
            VKey::PageDown,
            VKey::Space,
            VKey::Char('='),
            VKey::Char('f'),
        ] {
            assert_eq!(
                hk.match_action(&ev(retired, false, false, false, false)),
                None,
                "退役别名键 {retired:?} 不应再命中"
            );
        }
        // 未绑定的键不命中
        assert_eq!(
            hk.match_action(&ev(VKey::Char('z'), false, false, false, false)),
            None
        );
    }

    /// 修饰键敏感：Ctrl+→ 不该命中裸 →（Next 的默认键）。
    #[test]
    fn modifier_sensitivity() {
        let hk = Hotkeys::default();
        let ctrl_right = ev(VKey::Right, false, true, false, false);
        assert_eq!(
            hk.match_action(&ctrl_right),
            None,
            "Ctrl+→ 未绑定，不应命中 Next 的 →"
        );
    }

    /// 大小写无关：'r' 与 'R' 主键都是 Char('R')，命中同一条。
    #[test]
    fn letter_case_is_normalized() {
        let hk = Hotkeys::default();
        assert_eq!(
            hk.match_action(&ev(VKey::Char('r'), false, false, false, false)),
            hk.match_action(&ev(VKey::Char('R'), false, false, false, false))
        );
    }

    /// `uses`：这把键是不是已经被某个内置动作占着。
    ///
    /// 这是动态（插件）热键的裁决依据——内置优先，用户配的键位不被插件抢走。
    #[test]
    fn uses_reports_occupied_keys() {
        let hk = Hotkeys::default();
        let rotate = Hotkey::from_spec("R").expect("键串");
        assert!(hk.uses(&rotate), "R 是 Rotate 的出厂键位，应算被占用");
        let free = Hotkey::from_spec("Ctrl+Shift+O").expect("键串");
        assert!(!hk.uses(&free), "没绑过的键不该算被占用");
        // 解绑之后就不再占用（用户改键位后插件热键应当恢复生效）。
        let mut hk2 = Hotkeys::default();
        hk2.set_binding(Action::Rotate, free);
        assert!(hk2.uses(&free));
        assert!(!hk2.uses(&rotate), "旧键位被夺走后不该再算占用");
    }

    /// ';' ',' ':' '+' 主键经 to_ini→from_ini 不丢：这些字符正是 INI 值的分隔符，
    /// 裸写会在落盘回读时被拆碎，必须编码成 Semicolon/Comma/Colon/Plus 文字 token。
    #[test]
    fn separator_char_keys_survive_ini_round_trip() {
        let mut hk = Hotkeys::default();
        hk.set_binding(Action::Next, Hotkey::from_spec(";").unwrap());
        hk.set_binding(Action::Prev, Hotkey::from_spec(",").unwrap());
        hk.set_binding(Action::ZoomIn, Hotkey::from_spec(":").unwrap());
        let back = Hotkeys::from_ini(&hk.to_ini());
        for a in [Action::Next, Action::Prev, Action::ZoomIn] {
            assert_eq!(
                back.bindings[a.index()],
                hk.bindings[a.index()],
                "{} 的键经落盘回读不该变",
                a.id()
            );
        }
    }

    /// 解绑能落盘：空段（`id:`）回读成无绑定，不让默认值复活。
    #[test]
    fn cleared_binding_survives_ini_round_trip() {
        let mut hk = Hotkeys::default();
        hk.clear_binding(Action::Rotate);
        let back = Hotkeys::from_ini(&hk.to_ini());
        assert!(
            back.bindings[Action::Rotate.index()].is_empty(),
            "解绑过的动作不许在回读后复活出厂默认"
        );
        assert_eq!(back, hk, "解绑后的整组序列化往返不能漂变");
    }

    /// 跨动作重复键：ini 里后出现的段夺走它（唯一性与 set_binding 一致），
    /// 解析完不允许两个动作共享同一把键。
    #[test]
    fn from_ini_enforces_cross_action_uniqueness() {
        let hk = Hotkeys::from_ini("prev:Left;next:Left");
        assert!(
            hk.bindings[Action::Prev.index()].is_empty(),
            "先出现的 prev:Left 应被后出现的 next:Left 夺走"
        );
        assert_eq!(hk.bindings[Action::Next.index()].len(), 1);
    }

    /// 规范化规范串 round-trip 自洽。
    #[test]
    fn spec_round_trip() {
        for spec in [
            "Left",
            "PageUp",
            "Ctrl+F",
            "Alt+Shift+G",
            "F11",
            "NumAdd",
            "0",
            "Plus",
            "Win+Space",
        ] {
            let h = Hotkey::from_spec(spec).expect("应能解析");
            assert_eq!(
                h.to_spec(),
                spec,
                "to_spec 必须能还原原始规范串（大小写规范后）"
            );
        }
    }

    /// 空格分隔的规范串应被拒（分隔符只能是 '+'）。
    #[test]
    fn malformed_spec_is_rejected() {
        assert!(Hotkey::from_spec("").is_none());
        assert!(Hotkey::from_spec("Ctrl F").is_none());
        assert!(Hotkey::from_spec("??").is_none());
        assert!(Hotkey::from_spec("F0").is_none(), "F0 永远收不到");
        assert!(Hotkey::from_spec("F13").is_none(), "windui 只发 F1..=F12");
        assert!(Hotkey::from_spec("F12").is_some());
    }

    /// 整组 INI 序列化 round-trip：默认值写出再读回必须逐字节一致。
    #[test]
    fn ini_round_trip_is_stable() {
        let hk = Hotkeys::default();
        let s = hk.to_ini();
        let back = Hotkeys::from_ini(&s);
        assert_eq!(hk, back, "默认快捷键序列化往返不能漂变");
    }

    /// 部分解析：只给了 prev 的 INI 段，其余动作必须保持默认，prev 被覆盖。
    #[test]
    fn partial_ini_keeps_other_defaults() {
        let back = Hotkeys::from_ini("prev:Right;fullscreen:F5");
        assert_eq!(
            back.bindings[Action::Prev.index()],
            vec![dhk(Base::Right, false)]
        );
        assert_eq!(
            back.bindings[Action::Fullscreen.index()],
            vec![dhk(Base::F(5), false)]
        );
        // 未提及的动作沿默认——**除非**其默认键被本 ini 显式划给了别的动作：
        // 唯一性（写入即夺键，与 set_binding 同规）下 prev:Right 夺走 Next 的默认
        // Right，Next 随之变空，不允许两个动作共享同一把键。
        assert!(back.bindings[Action::Next.index()].is_empty());
        // 其余既未提及也没被夺键的动作仍沿默认（如 first）。
        assert_eq!(
            back.bindings[Action::First.index()],
            Hotkeys::default().bindings[Action::First.index()]
        );
    }

    /// 单个动作的 spec 写坏＝**回落该动作出厂默认**，不是静默解绑。
    /// 这是手改 ini 的现场：多敲一个字符，功能不该从键盘上消失。
    #[test]
    fn a_malformed_entry_keeps_that_actions_default() {
        let back = Hotkeys::from_ini("fit:??");
        assert_eq!(
            back.bindings[Action::Fit.index()],
            Hotkeys::default().bindings[Action::Fit.index()],
            "写坏的 spec 应保留 `fit` 的出厂键，而不是把它清成无绑定"
        );
    }

    /// 空段是**解绑的序列化形态**（`to_ini` 对无绑定动作写 `id:`），不能被
    /// "写坏回落默认"顺手复活——否则设置页解绑一把键，重启就被悄悄绑回去。
    #[test]
    fn an_empty_entry_still_means_unbound() {
        let back = Hotkeys::from_ini("fit:");
        assert!(
            back.bindings[Action::Fit.index()].is_empty(),
            "空段必须保持解绑（round-trip 依赖它）"
        );
    }

    /// 唯一性判据：两条同组合 Hotkey 必须判等（set_binding 靠它 vacate 旧主）。
    #[test]
    fn same_combo_is_equal() {
        let a = Hotkey::from_spec("Ctrl+Shift+D").unwrap();
        let b = Hotkey::from_spec("Shift+Ctrl+D").unwrap();
        assert_eq!(a, b, "修饰键顺序不应影响相等性");
    }

    /// 动作集扩到 35（2026-10-14 扩到 27 + 2026-10-18 逐帧两把 + 2026-10-31 m167 幻灯片
    /// 一把 + m216 WASD 平移四把 + m217 取色器一把），id 双向映射自洽；
    /// 那批新动作默认不绑，仅「关闭窗口」出厂绑 Esc（看图器通用退出键，见
    /// `Hotkeys::default`），逐帧的 `,` / `.` 属出厂就绑的那一组；
    /// 2026-10-19 起不透明度 `Ctrl+↑/↓` 也出厂就绑（m53-K，消除两个孤儿动作）。
    #[test]
    fn expanded_action_set_ids_round_trip_and_new_unbound() {
        assert_eq!(Action::COUNT, 36);
        for a in Action::ALL {
            assert_eq!(
                Action::from_id(a.id()),
                Some(a),
                "{} 的 id 回读不自洽",
                a.id()
            );
        }
        let hk = Hotkeys::default();
        for a in [
            Action::Open,
            Action::CopyImage,
            Action::CopyPath,
            Action::Minimal,
            Action::Checker,
            Action::Properties,
            Action::Settings,
            Action::Topmost,
            Action::Lock,
            Action::Minimize,
            Action::Maximize,
            // 放映也一样出厂不绑键（与上面那一组同一条规矩：不给老用户制造新拦截）。
            // 入口是右键「操作」子菜单那一项，想要键的人在快捷键页自绑——那一页
            // 遍历 `Action::ALL`，这一行是自动多出来的。
            Action::Slideshow,
            Action::Companion,
        ] {
            assert!(
                hk.bindings[a.index()].is_empty(),
                "新动作 {} 默认应不绑（不给老用户制造新拦截）",
                a.id()
            );
        }
        // 不透明度两只在设置页可见，出厂必须真能按到（m53-K）：Ctrl+↑/↓。
        assert_eq!(
            hk.match_action(&ev(VKey::Up, false, true, false, false)),
            Some(Action::OpacityUp)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Down, false, true, false, false)),
            Some(Action::OpacityDown)
        );
        // 裸 ↑/↓ 不拦截：m216 落地 WASD 后纵向平移归 W/S（"留给未来纵向平移"的
        // 预留作废），裸 ↑/↓ 维持无绑定（Ctrl+↑/↓ 是不透明度档）。
        assert_eq!(
            hk.match_action(&ev(VKey::Up, false, false, false, false)),
            None
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Down, false, false, false, false)),
            None
        );
        // WASD 平移（m216）出厂即绑，四向各自命中。
        assert_eq!(
            hk.match_action(&ev(VKey::Char('W'), false, false, false, false)),
            Some(Action::PanUp)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('A'), false, false, false, false)),
            Some(Action::PanLeft)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('S'), false, false, false, false)),
            Some(Action::PanDown)
        );
        assert_eq!(
            hk.match_action(&ev(VKey::Char('D'), false, false, false, false)),
            Some(Action::PanRight)
        );
        // 取色器（m217）出厂即绑 K。
        assert_eq!(
            hk.match_action(&ev(VKey::Char('K'), false, false, false, false)),
            Some(Action::Picker)
        );
        // 「关闭窗口」出厂绑 Esc，单键唯一。
        assert_eq!(
            hk.bindings[Action::Close.index()],
            vec![dhk(Base::Escape, false)],
            "关闭窗口默认应绑 Esc"
        );
        // 整组 27 段写出→读回逐字节一致（新增空段不许漂变）。
        assert_eq!(Hotkeys::from_ini(&hk.to_ini()), hk);
    }

    /// 旧 ini 的多键段只取第一把（单键不变式：其余别名退役）。
    #[test]
    fn from_ini_keeps_only_the_first_key() {
        let hk = Hotkeys::from_ini("next:Right,PageDown,Space;zoom_in:NumAdd,Plus,=");
        assert_eq!(
            hk.bindings[Action::Next.index()],
            vec![dhk(Base::Right, false)],
            "多键段应只取第一把"
        );
        assert_eq!(
            hk.bindings[Action::ZoomIn.index()],
            vec![dhk(Base::NumAdd, false)]
        );
    }

    /// set_binding 整体替换单条，且把同键从别的动作手里夺走（唯一性）；
    /// clear_binding 是唯一的解绑通道。
    #[test]
    fn set_binding_replaces_and_vacates_others() {
        let mut hk = Hotkeys::default();
        // 夺键：Right 原属 Next
        hk.set_binding(Action::Rotate, Hotkey::from_spec("Right").unwrap());
        assert_eq!(
            hk.bindings[Action::Rotate.index()],
            vec![Hotkey::from_spec("Right").unwrap()]
        );
        assert!(
            hk.bindings[Action::Next.index()].is_empty(),
            "旧主应被 vacate"
        );
        // 替换：同一动作再绑 → 仍是单条，不留旧位
        hk.set_binding(Action::Rotate, Hotkey::from_spec("0").unwrap());
        assert_eq!(
            hk.bindings[Action::Rotate.index()],
            vec![Hotkey::from_spec("0").unwrap()],
            "整体替换后仍应恰好一条"
        );
        assert!(
            hk.bindings[Action::Fit.index()].is_empty(),
            "0 的旧主 Fit 被 vacate"
        );
        // 解绑
        hk.clear_binding(Action::Rotate);
        assert!(hk.bindings[Action::Rotate.index()].is_empty());
    }

    /// m278 判据本体：任何动作都不许持有裸空格。正向用例与负向臂共用这一处，
    /// 免得"把规则在测试里复述一遍"那种永远不红的假绿。
    fn assert_no_action_owns_space(hk: &Hotkeys, what: &str) {
        assert!(
            !Action::ALL
                .iter()
                .any(|a| hk.bindings[a.index()].iter().any(|b| b.is_reserved())),
            "{what}：裸空格被动作占用了"
        );
    }

    #[test]
    fn only_bare_space_is_reserved() {
        assert!(
            Hotkey::from_spec("Space").unwrap().is_reserved(),
            "裸空格要锁"
        );
        for spec in ["Ctrl+Space", "Alt+Space", "Shift+Space", "Escape", "F11"] {
            let h = Hotkey::from_spec(spec).unwrap();
            assert!(!h.is_reserved(), "{spec} 不该被锁：预览钩子不放过修饰键");
        }
    }

    #[test]
    fn reserved_space_gate_blocks_binding_and_legacy_ini() {
        let mut bound = Hotkeys::default();
        bound.set_binding(Action::Next, Hotkey::from_spec("Space").unwrap());
        assert_no_action_owns_space(&bound, "set_binding 之后");
        let from_ini = Hotkeys::from_ini("next:Space;prev:Left");
        assert_no_action_owns_space(&from_ini, "from_ini 之后");
        assert_eq!(
            from_ini.bindings[Action::Prev.index()]
                .first()
                .map(|h| h.to_spec()),
            Some("Left".to_string()),
            "锁空格不该把同串里的其它键一起吃掉"
        );
        assert_no_action_owns_space(&Hotkeys::default(), "出厂默认");
    }

    /// 负向臂：手工绕过闸门把空格塞进绑定表，判据必须当场红。
    /// 拿掉这道闸门（或判据自己写成恒真）这条就绿了 —— 说明上面的正向用例有牙齿。
    #[test]
    #[should_panic(expected = "裸空格被动作占用了")]
    fn reserved_space_gate_rejects_a_binding_that_bypassed_it() {
        let mut hk = Hotkeys::default();
        hk.bindings[Action::Next.index()] = vec![Hotkey::from_spec("Space").unwrap()];
        assert_no_action_owns_space(&hk, "绕过闸门的旧实现");
    }

    /// Esc 现为可绑定主键：捕获→序列化→回读三端一致，展示用 "Esc"。
    #[test]
    fn escape_is_bindable_and_round_trips() {
        let hk = Hotkey::from_event(&ev(VKey::Escape, false, false, false, false))
            .expect("Esc 应能被抓成一条快捷键");
        assert_eq!(hk.base, Base::Escape);
        assert_eq!(hk.to_spec(), "Escape");
        assert_eq!(hk.pretty(), "Esc");
        assert_eq!(Hotkey::from_spec("Escape"), Some(hk));
        assert_eq!(Hotkey::from_spec("Esc"), Some(hk));
        // 带修饰的组合也回读一致（唯一性命名的正是这个组合）。
        let esc_ctrl = Hotkey::from_spec("Ctrl+Escape").unwrap();
        assert_eq!(esc_ctrl.base, Base::Escape);
        assert!(esc_ctrl.ctrl);
    }
}
