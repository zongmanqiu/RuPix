//! 设置窗像素探针：开一扇**真的**设置窗（走 `ui::settings::open_settings`，与主窗
//! 右键菜单「设置」同一条入口），供 PrintWindow 截图逐页核对版式。
//!
//! 为什么要有这个：设置窗在发布版里只有右键菜单一个入口，而菜单的落点跟着全局
//! 光标走（本机桌面与别家 AI 共用，光标不可控），用消息驱动菜单点不准。这里直接
//! 用同一套装配代码开窗，坐标就落在导航条上，可复现。
//!
//! 跑法（从 `main/`）：`cargo run --release --example settings_preview`
//! 探针窗 240×120 只是宿主，设置窗开在它之后并归它所有；关掉探针窗两个一起走。
//! 不写配置、不落盘（`Config::default()` 是内存里现造的）。
//!
//! 已签收（2026-10-17 逐页截图核对）：选项 / 快捷键 / 解码器 / 解码 / 关于**五页全部正常渲染**
//! （两列对齐、红字段名、导航条选中条、logo 与两张赞助码都出图）。
//! ★ 这个探针**自带**两处与产品无关的差别，看截图前先知道：
//!   ① 它不走 `runtime::run`，所以自己补做了两件事——注册私有字形区字体（否则搜索框
//!      放大镜显示成方框）、开窗后直接调一次 `Handles::refresh_decoders`（否则三张卡
//!      都是「0 个」，因为进页扫描本来挂在主窗 16 ms 轮询上）；
//!   ② 它的当前目录是 `main/`，旁边没有 `decoders/`——要看真实清单就在跑之前把
//!      `RUPIX_DECODERS_DIR`（绝对路径）指过去。
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use rupix_lib::core::config::Config;
use rupix_lib::ui::settings::{open_settings, Hub};
use windui::prelude::*;

fn main() {
    rupix_lib::platform::logging::install_logging();
    // 解码栈照装：解码器页 / 解码页要扫目录、列候选，不装就是两份空清单。
    rupix_lib::install_decode_stack();
    // 搜索框放大镜的私有字形区字体（U+E71E）：真实启动由 `runtime::run` 注册，
    // 探针不走 `run`，不补这一步就只能看到一个方框——那是探针的假象，不是产品的。
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let _ = windui::text::register_private_use_font(
        &format!("{windir}\\Fonts\\segmdl2.ttf"),
        "Segoe MDL2 Assets",
    );
    let config = Rc::new(RefCell::new(Config::default()));
    let hub: Hub = Rc::new(RefCell::new(None));
    let fired = Cell::new(false);

    App::new("settings-probe", 240, 120)
        .content(Element::col().child(Element::label("settings probe")))
        .on_interval(Duration::from_millis(150), move |ctx| {
            if fired.replace(true) {
                return;
            }
            open_settings(ctx, &config, &hub);
            // 直接落在「解码器」页（TAB_DECODERS = 2）：搜索框与放大镜在这页。
            if let Some(h) = hub.borrow().as_ref() {
                h.tab.set(2);
                // 进页扫描今天由**主窗** 16 ms 轮询驱动（`runtime::run` 调
                // `Handles::refresh_decoders`），探针这扇 240×120 的宿主窗没有那条轮询——
                // 不补这一句，三张卡就都是「0 个」（探针自带的假象，不是产品缺数据）。
                h.refresh_decoders(&config.borrow());
            }
        })
        .run();
}
