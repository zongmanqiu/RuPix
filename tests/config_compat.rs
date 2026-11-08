//! 配置兼容门禁（CurSeen Phase 5）：老配置必须被现行解析器完整直读——
//! 现行键生效、淘汰键静默忽略、看不懂的值回退默认、绝不 panic。

use rupix_lib::core::config::{Config, DblClick, InitWindow, SourceId, UiTheme, WheelAction};
use rupix_lib::core::decode::RawParams;
use rupix_lib::core::gallery::SortBy;
use rupix_lib::core::hotkey::Hotkeys;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn temp_ini(name: &str, text: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.temp");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("rupix_cfg_compat_{}_{}", std::process::id(), name));
    fs::write(&p, text).unwrap();
    p
}

/// fixtures/default.ini：全键非默认 + 旧版淘汰键混排，逐键断言。
#[test]
fn legacy_ini_loads_without_loss() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/default.ini");
    let text = fs::read_to_string(&fixture).expect("tests/fixtures/default.ini 缺失");
    let p = temp_ini("legacy.ini", &text);
    let cfg = Config::load_from(&p);
    let _ = fs::remove_file(&p);

    let expected = Config {
        stay_on_top: true,
        dbl_click: DblClick::Fullscreen,
        // 旧版 fixture 不含 decode_timeout_s（2026-11-04 挂死恢复新增）→ 回默认 20 秒。
        decode_timeout_s: 20,
        // 旧版 fixture 不含 slide_interval 键（m167 新增）→ 回默认 3 秒。
        // 这条正是"加了新设置项也不能读坏老配置"的兼容判据：老 ini 缺键必须安静回默认。
        slide_interval: 3,
        // show_title_bar=0 → minimal_mode=true（见 config.rs 的兼容映射）
        minimal_mode: true,
        // 旧版 fixture 不含 checker/filmstrip/locked → 全部回默认（关）。
        checker: false,
        filmstrip: false,
        filmstrip_left: false,
        locked: false,
        long_image_fit: true,
        wheel: WheelAction::Navigate,
        // 旧版 fixture 不含 ctrl_wheel 键 → 回退默认（翻页）。
        ctrl_wheel: WheelAction::Navigate,
        // m278 两个新键在旧 fixture 里同样不存在 → 回默认「无」。
        alt_wheel: WheelAction::None,
        shift_wheel: WheelAction::None,
        init_window: InitWindow::Fullscreen,
        // 旧 fixture 不含 borderless（m269 新增）→ 回默认关。
        borderless: false,
        window_w: 1024,
        window_h: 768,
        opacity: 40,
        // ★ 2026-10-26 键名从旧键统一到 `decoders_disabled`。
        //   **刻意不做读兼容**（用户 2026-10-26 拍板：改键名不兼容），所以这份旧 fixture
        //   里那行 `legacy_disabled=av;raw` 现在是**未知键** → 静默回空。
        //   代价要说清楚：老用户的"停用 / 首选解码器"这两项在升级后需重设一次。
        decoders_disabled: Vec::new(),
        // 旧 fixture 不含 extensions_disabled 键 → 回空表。
        extensions_disabled: Vec::new(),
        // 旧 fixture 不含 extensions_isolated 键 → 回空表。
        extensions_isolated: Vec::new(),
        // 这份 fixture 是**旧版** ini：缺 decoders_preferred 键必须回空表，
        // 这正是"加了新设置项也不能读坏老配置"的兼容判据之一。
        decoders_preferred: BTreeMap::new(),
        prefetch: false,
        // m281 隐藏键：旧 fixture 不含 ⇒ 回默认 3（老配置升级后深度不变）。
        prefetch_depth: 3,
        close_to_tray: true,
        // 旧版 fixture 不含 space_preview 键 → 回默认（关）。
        space_preview: false,
        // 旧版 fixture 不含 drag_image 键 → 回默认（**开**）。这条钉的是"新键默认开，
        // 也不许把老配置读成关"——老 ini 升级后左键拖图片照常，不必重设。
        drag_image: true,
        // 旧版 fixture 同样不含 show_toolbar_buttons → 回默认（**开**）。
        // 老 ini 升级后底部按钮照常显示，不会被静默隐藏。
        show_toolbar_buttons: true,
        raw_last: Some(RawParams {
            width: 1920,
            height: 1080,
            layout: 8,
        }),
        // 这份 fixture 是**旧版** ini：缺 download_source 键必须回默认（Gitee），
        // 这正是"加了新设置项也不能读坏老配置"的兼容判据之一。
        // ★ 2026-10-30 起联网来源只有一个键（旧 fixture 里的 decoder_source / plugin_source
        //   已不再被读——这条用例照样绿，因为缺新键就是回默认）。
        download_source: SourceId::Gitee,
        // 旧 fixture 不含 ui_theme 键 → 回默认（深色）。看图窗口配色是 m200 新增的，
        // 老配置没有它必须安静落回深色——与 `download_source` 上面那条同一个判据。
        ui_theme: UiTheme::Dark,
        // 旧 fixture 不含这四个地址键 → 必须回落内置默认（"加了新键也不能读坏老配置"）。
        decoder_index_gitee: SourceId::Gitee.index_url().to_string(),
        decoder_index_github: SourceId::Github.index_url().to_string(),
        plugin_index_gitee: SourceId::Gitee.plugin_index_url().to_string(),
        plugin_index_github: SourceId::Github.plugin_index_url().to_string(),
        // m250 新增：老 fixture 没有这两键 ⇒ 回内置默认。
        update_index_gitee: SourceId::Gitee.update_index_url().to_string(),
        update_index_github: SourceId::Github.update_index_url().to_string(),
        hotkeys: Hotkeys::default(),
        allow_multiple: false,
        // 旧 fixture 不含 ui_cards 键 → 回空表（= 设置页各卡全部展开）。
        // 与 `drag_image` 那条相反方向的判据：这张表里"缺键"表示**没有收起的卡**，
        // 所以老配置升级后不该突然看到一页折叠起来的卡片。
        ui_cards_collapsed: Vec::new(),
        // 旧 fixture 不含排序两键（m275 新增）→ 回默认「文件名升序」。
        // 老配置升级后清单次序必须和升级前一模一样，不能因为多了个维度就重排。
        sort_by: SortBy::Name,
        sort_desc: false,
    };
    assert_eq!(cfg, expected, "老配置直读结果与预期不符");
}

/// 乱值/越界/未知键：一律回退或夹紧，解析器不得 panic。
#[test]
fn garbage_values_never_panic() {
    let p = temp_ini(
        "garbage.ini",
        "stay_on_top=maybe\nopacity=999\nwindow_w=abc\nunknown_key=1\nbird_eye=1\n",
    );
    let cfg = Config::load_from(&p);
    let _ = fs::remove_file(&p);
    assert!(!cfg.stay_on_top, "看不懂的布尔值应回退默认（0）");
    assert_eq!(cfg.opacity, 90, "越界不透明度回退默认档");
    assert_eq!(cfg.window_w, 800, "非数字宽高回退默认");
}
