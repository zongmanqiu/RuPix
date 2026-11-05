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

use std::cell::RefCell;
use std::rc::Rc;

use windui::core::EventCtx;
use windui::prelude::*;

use super::settings::option_row;
use super::settings::shortcuts::hotkey_box;
use super::tokens::{to_color, BG, CONTROL_GROUP_W, DIALOG_FIT_MIN, DIALOG_PAD, ROW_H};
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
            SettingKind::Int | SettingKind::Text => Bound::Text(signal(value.to_string())),
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
        }
    }
}

/// 下标 → 取值。越界回落到第一项：调用方是信号驱动的下拉，理论上不会越界，
/// 但这里回落的代价是"显示第一个选项"，而 panic 的代价是整个查看器没了。
fn choice_value(values: &[String], idx: usize) -> String {
    values.get(idx).cloned().unwrap_or_default()
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
/// 标签左缘 = 外层 `DIALOG_PAD`(16) + 行内 16 = **32**，与设置页（页 16 + 行 16）落在
/// 同一条竖线上；控件右缘 = 32 + `COL1_W`(208) + `CONTROL_GROUP_W`(200) = 440，右边再留
/// 40 给 `flex_spacer`，控件不会被顶到窗框。
const DIALOG_W: i32 = 480;

/// 打开（或聚焦）某插件的设置窗。
pub fn show(ctx: &mut EventCtx, ext: &Extension, config: &Rc<RefCell<Config>>) {
    let id = ext.manifest.id.clone();
    let title = format!("{} 设置", ext.manifest.name);
    let specs = ext.manifest.settings.clone();
    if specs.is_empty() {
        return;
    }
    let stored = crate::extensions::settings::load(&id);
    let value_of = |key: &str, default: &str| {
        stored
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| default.to_string())
    };
    let bounds: Vec<Bound> = specs
        .iter()
        .map(|s| Bound::from_spec(s, &value_of(&s.key, &s.default)))
        .collect();

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
        ctx.request_close();
    };
    let cancel = |ctx: &mut EventCtx| ctx.request_close();

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
    // ★ 先关旧窗再开新窗，但**两个键必须不同**：`single` 已开时只聚焦、不重建内容
    //   （快捷键页改过值之后再点齿轮，留在屏幕上的会是开窗那一刻的旧值）；而关窗是
    //   投递、开窗是即时，同键既关又开会让滞留的 WM_CLOSE 把刚开出来的新窗收走
    //   —— 2026-11-04 用户实测「齿轮点不动，窗闪一下就没了」。见 `singleton::recycle`。
    let (old_key, win_key) = super::singleton::recycle(&format!("plugin-settings:{id}"));
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
                // 行与行不再另加间距：[`option_row`] 自带 `ROW_H`(40) 的固定行高，
                // 与设置页同一套竖向节奏（那边是 `col().spacing(0)` + 40 高的行）。
                let mut col = Element::col().padding(DIALOG_PAD).spacing(0);
                let mut hk_seen = 0usize;
                for (row, (spec, bound)) in specs.iter().zip(bounds_ui.iter()).enumerate() {
                    let control = match bound {
                        Bound::Bool(sig) => Element::switch(*sig),
                        Bound::Choice { idx, labels, .. } => {
                            Element::dropdown(labels.clone(), *idx).width(CONTROL_GROUP_W)
                        }
                        Bound::Text(sig) => Element::text_input(*sig, "").width(CONTROL_GROUP_W),
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
                    col = col.child(option_row(&spec.label, control));
                }
                col = col.child(
                    Element::row()
                        .cross(Align::Center)
                        .spacing(8)
                        .height(ROW_H)
                        // 行内 16 让按钮右缘落回控件列的右缘线上，不再凸出到窗口边。
                        .padding_xy(DIALOG_PAD, 0)
                        .child(Element::flex_spacer())
                        .child(Element::button("确定").on_click(confirm.clone()))
                        .child(Element::button("取消").on_click(cancel)),
                );
                col
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
}
