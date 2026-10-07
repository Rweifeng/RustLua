//! Headless acceptance drive for M66：暂停菜单（`ui/pause_menu.lua`）搬到一份 `.prefab`。
//!
//! 这是"面板五屏逐屏搬"的第三屏（shop 之后）。暂停菜单是**三页共面板**：
//!   主菜单页（`M.render`，id 前缀 `pse`，7 行 ITEMS + 选中底衬 + 音量/形象动态 label）、
//!   设置页（`M.render_settings`，前缀 `pse:s`，四路音量滑轨 + 保存/返回按钮）、
//!   帮助页（`M.render_help`，前缀 `pse:h`，键位/玩法两列行表，行数随绑定与平台浮动）。
//! 三页通过 `switch_page` 在换页瞬间清理旧页，因此同一帧只有一页的面板块在画；门按三态
//! 各采一份账：`A` = 主菜单（index=0）、`B` = 设置页（index=0）、`C` = 帮助页。
//!
//! 门守的是**搬运不改画面**，四段账（与下面四个 banner 同号）：
//!   ① 逐 id 黄金对账（A/B/C 三态，数字取本次 `--capture` 实测）：每块一条 `shown` 行，读数
//!      **与载体无关**：同一块东西由动态层画还是由预设实例画，印出来必须是同一串字节
//!      （矩形/贴图/九宫格/字/字号/颜色）。两块都在画 = 重影，当场红；都没画 = 记一行
//!      `hidden` 交给黄金表管（谁把 `shown` 变成 `hidden` 或反过来，都在这一格上红）。
//!      三页互斥通过 `switch_page` 的清页语义天然成立：非本页的块在 A 态必须全是 `hidden`。
//!   ② 载体记账 + **整屏同层**：一屏里的块只许"全在动态层"或"全在实体层"。半搬是最坏的
//!      形状——实体层整体画在动态层之上（`frame_data.rs:869-883`：`push_ui_*` 之后才
//!      `push_scene_ui`），于是"搬进来的那块"会盖住"没搬的那块"，而①的矩形读数对此全瞎。
//!      三页合计也只许一个载体（三页都迁或都不迁）。
//!   ③ 覆盖账：谁盖住谁。这条是 ① 在层序维度的另一半，而且**跨载体可比**：
//!      迁移前"谁盖谁"由 `ui.*` 的 BTreeMap 键序（同类内 id 字典序）决定，迁移后由
//!      预设的行序（= 实体槽位升序）决定，两套序不同源，所以只能量结果不能量规则。
//!      绘制序在本门里是**测出来的**不是假设的：实体层按 `EntityId::index()` 排，动态层按
//!      (类别, id) 排，而类别序由 [`submission_order`] 拿本帧真实提交序列背书
//!      （白图段必须在 `ui_panel_main_9s.png` 段之前）。
//!      不透明判定用贴图中心的实际 alpha（`--capture` 打印，见 [`OPAQUE_NOTE`]）。
//!      覆盖表也进黄金表：每块一行 `cover`，右边那一列就是"谁整块盖住了我"（没被盖住写 `-`）。
//!   ④ 文字着色：动态层的 UI 文字**写死白色**（`mesh_ui.rs:416,422` 传 `[1.0,1.0,1.0]`），
//!      而 `text_get` 读不到颜色 ⇒ ① 的黄金表管不了这一维。这一条单独钉：搬进预设的每条
//!      `Text` 的 `color` 必须就是白，否则"搬家"顺手改了字色。
//!
//! 本门**没有**的账，写清楚免得被当成有：
//!   * 跨屏层序（暂停面板与 HUD / 引导行的谁盖谁）：①②③ 只看本屏的块，跨屏那一份在
//!     `--capture` 的输出里点名"与本屏面板矩形相交的其他动态块"，今天实测为**空集**才允许搬。
//!   * 版面数据（`authored`）没有迁移前基线：这一屏的版面今天住在 Lua 代码里，不在任何
//!     数据文件里，所以没有"预设里写的就是原来写的"可比。① 的 `shown` 行是唯一的那份账。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m66_pause_prefab [-- <game_jhyt-root>] [--capture]
//!
//! `--capture` 只打印读数表后退出（在**迁移前**的树上跑一次，产出的就是黄金表）。

use engine::{
    core::{
        entity::EntityId,
        input::InputSnapshot,
        ui::{UiImage, UiText},
        ui_layout::{UiRect, UiRectLayout},
    },
    host::{RuntimeScene, SceneDraw, SceneMesh},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::PathBuf,
};

/// 黄金表：`状态 \t shown \t 角色 \t 种类 \t 读数`，逐行与运行期读数比较。
const GOLDEN: &str = include_str!("support/jhyt_m66_pause_golden.tsv");

const VIEW_W: f32 = 1280.0;
const VIEW_H: f32 = 720.0;
const DT: f32 = 1.0 / 60.0;
/// 主菜单行数（`pause_menu.lua:115` 的 ITEMS）。
#[expect(dead_code)]
const ITEM_COUNT: usize = 7;
/// 设置页行数（`pause_menu.lua:200` 的 SETTINGS_ITEMS）。
#[expect(dead_code)]
const SETTINGS_COUNT: usize = 6;
/// 帮助页行数上界（`HELP_ROW_CAP = 20`）；实际行数由键位表长度决定，capture 采实际值，
/// prefab 按本常量开满槽位、多余的隐藏（与 shop 的 9 槽同款固定槽语义）。
#[expect(dead_code)]
const HELP_ROW_CAP: usize = 20;
/// 面板矩形（`layout.PANEL.pause` = 160,40,720,640），⑤ 用它判"与本页相交的其他动态块"。
const PANEL: UiRect = UiRect {
    x: 160.0,
    y: 40.0,
    w: 720.0,
    h: 640.0,
};

/// 一块东西今天可能有的两种载体读数之外的第三种：这一帧它没被画。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Carrier {
    Entity,
    Dynamic,
    Absent,
}

/// 画面上的一块。`entities` 允许两个名字，因为"二选一的贴图"（页签按下/未按下）在预设里
/// 是同一矩形上叠两张图、用 `prefab.set_visible` 互斥——没有运行期换贴图这条 API。
struct Probe {
    role: &'static str,
    /// 今天动态层那个 `ui.*` id。
    dynamic: &'static str,
    /// 迁移后 `.prefab` 里的实体名（一个角色可能两个名字互斥）。
    entities: &'static [&'static str],
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// 纯色块：今天 `ui.panel`，迁移后是 `Image texture=__white__` + RGBA。
    Panel,
    /// 贴图块：今天 `ui.image`（走 `ui/skin.lua`），迁移后同名同图。
    Image,
    /// 文字：今天 `ui_text.draw*` ⇒ `ui.text_at`，迁移后 `Text` + `prefab.set_text`。
    Text,
}

impl Kind {
    const fn tag(self) -> &'static str {
        match self {
            Self::Panel => "panel",
            Self::Image => "image",
            Self::Text => "text",
        }
    }
}

macro_rules! menu_row {
    ($tail:expr) => {
        concat!("pse:row", $tail)
    };
}
macro_rules! set_row {
    ($tail:expr) => {
        concat!("pse:s:row", $tail)
    };
}
macro_rules! set_track {
    ($tail:expr) => {
        concat!("pse:s:track", $tail)
    };
}
macro_rules! set_handle {
    ($tail:expr) => {
        concat!("pse:s:handle", $tail)
    };
}
macro_rules! help_row {
    ($tail:expr) => {
        concat!("pse:h:row", $tail)
    };
}
macro_rules! help_val {
    ($tail:expr) => {
        concat!("pse:h:val", $tail)
    };
}

/// 暂停菜单的全部可见物，顺序即黄金表顺序。三页前缀分列：
/// `menu_*`（主菜单页）、`set_*`（设置页）、`help_*`（帮助页）。
const PROBES: &[Probe] = &[
    // ---- 主菜单页（id 前缀 `pse`） ----
    probe("menu/box", "pse:box", &["PseBox"], Kind::Panel),
    probe("menu/rule", "pse:rule", &["PseRule"], Kind::Panel),
    probe("menu/sel", "pse:sel", &["PseSel"], Kind::Panel),
    probe("menu/skin_bg", "pse:skin_bg", &["PseSkinBg"], Kind::Image),
    probe("menu/divider", "pse:divider", &["PseDivider"], Kind::Image),
    probe("menu/seal", "pse:seal", &["PseSeal"], Kind::Image),
    probe("menu/close", "pse:close", &["PseClose"], Kind::Image),
    probe(
        "menu/key_hint",
        "pse:key_hint",
        &["PseKeyHint"],
        Kind::Image,
    ),
    probe("menu/title", "pse:title", &["PseTitle"], Kind::Text),
    probe("menu/row1", menu_row!("1"), &["PseRow1"], Kind::Text),
    probe("menu/row2", menu_row!("2"), &["PseRow2"], Kind::Text),
    probe("menu/row3", menu_row!("3"), &["PseRow3"], Kind::Text),
    probe("menu/row4", menu_row!("4"), &["PseRow4"], Kind::Text),
    probe("menu/row5", menu_row!("5"), &["PseRow5"], Kind::Text),
    probe("menu/row6", menu_row!("6"), &["PseRow6"], Kind::Text),
    probe("menu/row7", menu_row!("7"), &["PseRow7"], Kind::Text),
    probe("menu/msg", "pse:msg", &["PseMsg"], Kind::Text),
    probe("menu/hint", "pse:hint", &["PseHint"], Kind::Text),
    // ---- 设置页（id 前缀 `pse:s`） ----
    probe("set/box", "pse:s:box", &["PseSBox"], Kind::Panel),
    probe("set/rule", "pse:s:rule", &["PseSRule"], Kind::Panel),
    probe("set/sel", "pse:s:sel", &["PseSSel"], Kind::Panel),
    probe("set/skin_bg", "pse:s:skin_bg", &["PseSSkinBg"], Kind::Image),
    probe(
        "set/divider",
        "pse:s:divider",
        &["PseSDivider"],
        Kind::Image,
    ),
    probe("set/seal", "pse:s:seal", &["PseSSeal"], Kind::Image),
    probe("set/close", "pse:s:close", &["PseSClose"], Kind::Image),
    probe(
        "set/key_hint",
        "pse:s:key_hint",
        &["PseSKeyHint"],
        Kind::Image,
    ),
    probe("set/slot", "pse:s:slot", &["PseSSlot"], Kind::Image),
    probe(
        "set/btn_save",
        "pse:s:btn_save",
        &["PseSBtnSave"],
        Kind::Image,
    ),
    probe(
        "set/btn_back",
        "pse:s:btn_back",
        &["PseSBtnBack"],
        Kind::Image,
    ),
    probe("set/track1", set_track!("1"), &["PseSTrack1"], Kind::Image),
    probe("set/track2", set_track!("2"), &["PseSTrack2"], Kind::Image),
    probe("set/track3", set_track!("3"), &["PseSTrack3"], Kind::Image),
    probe("set/track4", set_track!("4"), &["PseSTrack4"], Kind::Image),
    probe(
        "set/handle1",
        set_handle!("1"),
        &["PseSHandle1"],
        Kind::Image,
    ),
    probe(
        "set/handle2",
        set_handle!("2"),
        &["PseSHandle2"],
        Kind::Image,
    ),
    probe(
        "set/handle3",
        set_handle!("3"),
        &["PseSHandle3"],
        Kind::Image,
    ),
    probe(
        "set/handle4",
        set_handle!("4"),
        &["PseSHandle4"],
        Kind::Image,
    ),
    probe("set/title", "pse:s:title", &["PseSTitle"], Kind::Text),
    probe("set/row1", set_row!("1"), &["PseSRow1"], Kind::Text),
    probe("set/row2", set_row!("2"), &["PseSRow2"], Kind::Text),
    probe("set/row3", set_row!("3"), &["PseSRow3"], Kind::Text),
    probe("set/row4", set_row!("4"), &["PseSRow4"], Kind::Text),
    probe("set/row5", set_row!("5"), &["PseSRow5"], Kind::Text),
    probe("set/row6", set_row!("6"), &["PseSRow6"], Kind::Text),
    probe("set/msg", "pse:s:msg", &["PseSMsg"], Kind::Text),
    probe("set/hint", "pse:s:hint", &["PseSHint"], Kind::Text),
    // ---- 帮助页（id 前缀 `pse:h`；行数随绑定浮动，开满 HELP_ROW_CAP 槽） ----
    probe("help/box", "pse:h:box", &["PseHBox"], Kind::Panel),
    probe("help/rule", "pse:h:rule", &["PseHRule"], Kind::Panel),
    probe(
        "help/skin_bg",
        "pse:h:skin_bg",
        &["PseHSkinBg"],
        Kind::Image,
    ),
    probe(
        "help/divider",
        "pse:h:divider",
        &["PseHDivider"],
        Kind::Image,
    ),
    probe("help/seal", "pse:h:seal", &["PseHSeal"], Kind::Image),
    probe("help/close", "pse:h:close", &["PseHClose"], Kind::Image),
    probe(
        "help/key_hint",
        "pse:h:key_hint",
        &["PseHKeyHint"],
        Kind::Image,
    ),
    probe("help/title", "pse:h:title", &["PseHTitle"], Kind::Text),
    // help 行/值：行数随绑定浮动（键盘/手柄），按 HELP_ROW_CAP=20 开满槽；capture 时
    // 实际行 N 的 row1..N/val1..N 是 shown，N+1..20 由 clear_help_ui 清成 hidden。
    probe("help/row1", help_row!("1"), &["PseHRow1"], Kind::Text),
    probe("help/val1", help_val!("1"), &["PseHVal1"], Kind::Text),
    probe("help/row2", help_row!("2"), &["PseHRow2"], Kind::Text),
    probe("help/val2", help_val!("2"), &["PseHVal2"], Kind::Text),
    probe("help/row3", help_row!("3"), &["PseHRow3"], Kind::Text),
    probe("help/val3", help_val!("3"), &["PseHVal3"], Kind::Text),
    probe("help/row4", help_row!("4"), &["PseHRow4"], Kind::Text),
    probe("help/val4", help_val!("4"), &["PseHVal4"], Kind::Text),
    probe("help/row5", help_row!("5"), &["PseHRow5"], Kind::Text),
    probe("help/val5", help_val!("5"), &["PseHVal5"], Kind::Text),
    probe("help/row6", help_row!("6"), &["PseHRow6"], Kind::Text),
    probe("help/val6", help_val!("6"), &["PseHVal6"], Kind::Text),
    probe("help/row7", help_row!("7"), &["PseHRow7"], Kind::Text),
    probe("help/val7", help_val!("7"), &["PseHVal7"], Kind::Text),
    probe("help/row8", help_row!("8"), &["PseHRow8"], Kind::Text),
    probe("help/val8", help_val!("8"), &["PseHVal8"], Kind::Text),
    probe("help/row9", help_row!("9"), &["PseHRow9"], Kind::Text),
    probe("help/val9", help_val!("9"), &["PseHVal9"], Kind::Text),
    probe("help/row10", help_row!("10"), &["PseHRow10"], Kind::Text),
    probe("help/val10", help_val!("10"), &["PseHVal10"], Kind::Text),
    probe("help/row11", help_row!("11"), &["PseHRow11"], Kind::Text),
    probe("help/val11", help_val!("11"), &["PseHVal11"], Kind::Text),
    probe("help/row12", help_row!("12"), &["PseHRow12"], Kind::Text),
    probe("help/val12", help_val!("12"), &["PseHVal12"], Kind::Text),
    probe("help/row13", help_row!("13"), &["PseHRow13"], Kind::Text),
    probe("help/val13", help_val!("13"), &["PseHVal13"], Kind::Text),
    probe("help/row14", help_row!("14"), &["PseHRow14"], Kind::Text),
    probe("help/val14", help_val!("14"), &["PseHVal14"], Kind::Text),
    probe("help/row15", help_row!("15"), &["PseHRow15"], Kind::Text),
    probe("help/val15", help_val!("15"), &["PseHVal15"], Kind::Text),
    probe("help/row16", help_row!("16"), &["PseHRow16"], Kind::Text),
    probe("help/val16", help_val!("16"), &["PseHVal16"], Kind::Text),
    probe("help/row17", help_row!("17"), &["PseHRow17"], Kind::Text),
    probe("help/val17", help_val!("17"), &["PseHVal17"], Kind::Text),
    probe("help/row18", help_row!("18"), &["PseHRow18"], Kind::Text),
    probe("help/val18", help_val!("18"), &["PseHVal18"], Kind::Text),
    probe("help/row19", help_row!("19"), &["PseHRow19"], Kind::Text),
    probe("help/val19", help_val!("19"), &["PseHVal19"], Kind::Text),
    probe("help/row20", help_row!("20"), &["PseHRow20"], Kind::Text),
    probe("help/val20", help_val!("20"), &["PseHVal20"], Kind::Text),
    probe("help/hint", "pse:h:hint", &["PseHHint"], Kind::Text),
];

const fn probe(
    role: &'static str,
    dynamic: &'static str,
    entities: &'static [&'static str],
    kind: Kind,
) -> Probe {
    Probe {
        role,
        dynamic,
        entities,
        kind,
    }
}

/// ③ 里"这块贴图中心实测不透明"的清单与口径。
///
/// 复现（无 PIL，用 System.Drawing）：
/// ```text
/// powershell -NoProfile -Command "Add-Type -AssemblyName System.Drawing; \
///   $b='examples/game_jhyt/assets/art/ui/common'; foreach($n in \
///   'ui_panel_main_9s','ui_divider','ui_icon_seal','ui_btn_close'){ $p=Join-Path $b ($n+'.png'); \
///   $i=[System.Drawing.Image]::FromFile($p); $d=New-Object System.Drawing.Bitmap $i; \
///   Write-Host ($n+' A='+$d.GetPixel($d.Width/2,$d.Height/2).A) }"
/// ```
/// 实测：`ui_panel_main_9s` 中心 A=255（31,35,40，暗纸不透明），`ui_icon_seal` 中心 A=255，
/// `ui_btn_close` 中心 A=255 但 (2,2) A=0（透明角），`ui_divider` 4×4 中心 A=0。
/// ⇒ 只有"整块矩形都被采样到不透明"的贴图才进这张表；边缘有透明的（close/seal）不进，
/// 因为"盖住别人"要的是它压住的那片像素真的不透明。
const OPAQUE_NOTE: &str = "贴图中心 alpha=255 才算能盖人（见常量注释的复现命令）";
const OPAQUE_TEXTURES: &[&str] = &["ui_panel_main_9s.png", "__white__"];
/// `__white__` 还要再看 RGBA 第四段（半透明色块盖不住人），阈值与注释同 ①。
const OPAQUE_ALPHA: f32 = 0.99;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let capture = args.iter().any(|arg| arg == "--capture");
    let project = resolve_project(&args)?;

    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    enter_pause(&mut runtime)?;

    banner("①+② 逐 id 黄金对账（A/B/C 三态）+ 载体记账");
    let mut dump = String::new();
    let mut carriers = Vec::new();
    // A 态：主菜单（index=0）。
    let state_a = collect_state(&mut runtime, "A", &mut dump, &mut carriers)?;
    // B 态：设置页（open_settings 会经 switch_page 清掉主菜单页）。
    open_settings(&mut runtime)?;
    let state_b = collect_state(&mut runtime, "B", &mut dump, &mut carriers)?;
    // C 态：帮助页（open_help 清设置页）。
    open_help(&mut runtime)?;
    let state_c = collect_state(&mut runtime, "C", &mut dump, &mut carriers)?;
    // C 态必须真的是帮助页：title 是「帮助」，且 help 面板在画（否则这把钥匙只测了 A/B 两态）。
    let help_title = state_c
        .get("help/title")
        .map(|reading| reading.body.clone())
        .unwrap_or_default();
    if !help_title.contains("帮助") {
        return Err(format!("C 态没有帮助页标题（{help_title}）——帮助页没打开").into());
    }
    // 帮助页实际行数（桌面形态行数固定；迁移后 prefab 必须开满这么多槽，多余槽隐藏）。
    let help_rows =
        query_num(&mut runtime, "return #require('ui.pause_menu').help_rows()")? as usize;
    println!("  帮助页实际行数：{help_rows}（help/row 与 help/val 探针按此行数静态化进 PROBES）");
    same_layer_screen(&carriers)?;

    banner("③ 覆盖账：谁盖住谁（跨载体可比，绘制序按实测排；结果进黄金表）");
    covering_account(&state_a, "A", &mut dump)?;
    covering_account(&state_b, "B", &mut dump)?;
    covering_account(&state_c, "C", &mut dump)?;

    if capture {
        print!("{dump}");
        println!("jhyt_m66_captured lines={}", dump.lines().count());
    } else {
        compare_golden(&dump)?;
        println!("golden_lines={}", dump.lines().count());
    }

    submission_order(&mut runtime)?;

    banner("④ 文字着色：动态层写死白 ⇒ 预设里每条 Text 必须也是白");
    text_colors(&mut runtime)?;

    banner("⑤ 跨屏相交：本屏面板与「别人的动态块」有没有重叠（没有才允许整屏搬）");
    cross_screen(&mut runtime)?;

    println!("jhyt_m66_pause_prefab_ok project={}", project.display());
    Ok(())
}

// ---------- 驱动 ----------

/// 走进暂停菜单：门面 → 山庄 → `pause.open`。`scene.load` 只挂起请求，由宿主在步进里应用。
fn enter_pause(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.with_script(|script| script.execute_console("scene.load('manor')"))?;
    run(runtime, 6)?;
    let opened = lua_str(runtime, "tostring(require('ui.pause_menu').open())")?;
    if opened != "true" {
        return Err(format!("打不开暂停菜单（open 返回 {opened}）").into());
    }
    let mode = lua_str(runtime, "require('core.state').mode")?;
    if mode != "Pause" {
        return Err(format!("open 之后模式是 {mode}，不是 Pause").into());
    }
    run(runtime, 2)?;
    Ok(())
}

/// 切到设置页（B 态）。
fn open_settings(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    let opened = lua_str(
        runtime,
        "tostring(require('ui.pause_menu').open_settings())",
    )?;
    if opened != "true" {
        return Err(format!("打不开设置页（open_settings 返回 {opened}）").into());
    }
    run(runtime, 2)?;
    let active = lua_str(
        runtime,
        "tostring(require('ui.pause_menu').settings_active ~= nil and require('ui.pause_menu').settings_active())",
    )?;
    if active != "true" {
        return Err(format!("设置页未激活（settings_active 返回 {active}）").into());
    }
    Ok(())
}

/// 切到帮助页（C 态）。
fn open_help(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    let opened = lua_str(runtime, "tostring(require('ui.pause_menu').open_help())")?;
    if opened != "true" {
        return Err(format!("打不开帮助页（open_help 返回 {opened}）").into());
    }
    run(runtime, 2)?;
    let active = lua_str(runtime, "tostring(require('ui.pause_menu').help_active())")?;
    if active != "true" {
        return Err(format!("帮助页未激活（help_active 返回 {active}）").into());
    }
    Ok(())
}

fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W as u32, VIEW_H as u32)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
}

fn run(runtime: &mut RuntimeScene, steps: u32) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..steps {
        frame(runtime)?;
    }
    Ok(())
}

// ---------- ①② 读数 ----------

/// 一块东西的本帧读数（`shown` 行的右半边）+ 它是谁画的。
#[derive(Clone, Debug)]
struct Reading {
    carrier: Carrier,
    /// 矩形（实体层由 `ui_rects_by_name` 解算，动态层由 `ui.*_get`）。
    rect: Option<UiRect>,
    /// 类别内排序键：动态层 = id，实体层 = 槽位号。③ 用它排绘制序。
    order: String,
    /// 贴图末段（Panel/Image 才有）。
    texture: String,
    /// RGBA 文本。文字块没有颜色维（动态层写死白），留空。
    rgba: String,
    body: String,
}

impl Reading {
    /// 能不能盖住别人：贴图在 [`OPAQUE_TEXTURES`] 里，且（若是白图）alpha ≥ `OPAQUE_ALPHA`。
    fn opaque(&self) -> bool {
        let tail = self.texture.rsplit('/').next().unwrap_or(&self.texture);
        if !OPAQUE_TEXTURES.contains(&tail) {
            return false;
        }
        if tail != "__white__" {
            return true;
        }
        self.rgba
            .split(',')
            .nth(3)
            .and_then(|value| value.parse::<f32>().ok())
            .is_some_and(|alpha| alpha >= OPAQUE_ALPHA)
    }
}

/// 一个状态的一份读数表（③ 复用），键 = role。
type State = BTreeMap<&'static str, Reading>;

fn collect_state(
    runtime: &mut RuntimeScene,
    tag: &str,
    dump: &mut String,
    carriers: &mut Vec<(String, Carrier)>,
) -> Result<State, Box<dyn std::error::Error>> {
    run(runtime, 1)?;
    let rects = runtime.ui_rects_by_name([VIEW_W, VIEW_H]);
    let hidden = runtime.with_script(|script| script.scene_text_hidden());
    let nodes = read_nodes(runtime)?;
    let mut state = BTreeMap::new();
    for probe in PROBES {
        let entity = entity_reading(&rects, &hidden, &nodes, probe);
        let dynamic = dynamic_reading(runtime, probe)?;
        let drawn_entity = entity.is_some();
        let drawn_dynamic = dynamic
            .as_ref()
            .is_some_and(|reading| reading.carrier != Carrier::Absent);
        if drawn_entity && drawn_dynamic {
            let a = entity
                .as_ref()
                .map_or_else(String::new, |reading| reading.body.clone());
            let b = dynamic
                .as_ref()
                .map_or_else(String::new, |reading| reading.body.clone());
            return Err(format!(
                "{} 在实体层与动态层各画了一遍：实体 {a:?} / 动态 {b:?}",
                probe.role
            )
            .into());
        }
        let reading = if let Some(reading) = entity {
            reading
        } else if let Some(reading) = dynamic.filter(|reading| reading.carrier != Carrier::Absent) {
            reading
        } else {
            // 这一帧它本来就不该出现（A 态只有 4 行库存 ⇒ 第 5..9 槽空；页签互斥时另一半不在）。
            // 记一行 `hidden` 而不是判红：黄金表里那一格今天就是 `hidden`，谁让它变成 `shown`
            // 或反过来，都在这条门上红——"少东西"的账由表来记，不靠这里猜。
            // 隐藏节点在实体层**不建布局项**（`prefab.set_visible` 的口径），所以实体层这一侧
            // 读不到矩形，与动态层的 `visible=false` 同形。
            dump.push_str(&format!(
                "{tag}\thidden\t{}\t{}\t-\n",
                probe.role,
                probe.kind.tag()
            ));
            carriers.push((probe.role.to_owned(), Carrier::Absent));
            state.insert(
                probe.role,
                Reading {
                    carrier: Carrier::Absent,
                    rect: None,
                    order: probe.dynamic.to_owned(),
                    texture: String::new(),
                    rgba: String::new(),
                    body: "hidden".to_owned(),
                },
            );
            continue;
        };
        dump.push_str(&format!(
            "{tag}\tshown\t{}\t{}\t{}\n",
            probe.role,
            probe.kind.tag(),
            reading.body
        ));
        carriers.push((probe.role.to_owned(), reading.carrier));
        state.insert(probe.role, reading);
    }
    Ok(state)
}

/// 实体层这一帧画没画这一块：多名字互斥（页签 on/off）时要求**恰好一个**有矩形。
fn entity_reading(
    rects: &BTreeMap<String, UiRect>,
    hidden: &BTreeSet<String>,
    nodes: &BTreeMap<&'static str, Node>,
    probe: &Probe,
) -> Option<Reading> {
    let drawn: Vec<&&'static str> = probe
        .entities
        .iter()
        .filter(|name| {
            let Some(rect) = rects.get(**name) else {
                return false;
            };
            // 0×0 的节点也算"在表里"，但文字被 `scene_text_hidden` 藏起来就是没画。
            !(probe.kind == Kind::Text && hidden.contains(**name)) && rect.w > 0.0 && rect.h > 0.0
        })
        .collect();
    if drawn.is_empty() {
        return None;
    }
    let name = drawn[0];
    let rect = *rects.get(*name)?;
    let node = nodes.get(*name)?;
    let texture = node
        .image
        .as_ref()
        .map(|image| image.texture.clone())
        .unwrap_or_default();
    let rgba = node.image.as_ref().map_or_else(String::new, |image| {
        format!(
            "{:.3},{:.3},{:.3},{:.3}",
            image.color[0], image.color[1], image.color[2], image.color[3]
        )
    });
    let body = match probe.kind {
        Kind::Panel => {
            if texture != "__white__" {
                return Some(mismatch(
                    probe,
                    format!("色块的贴图不是内置白图而是 {texture}"),
                ));
            }
            format!("{},{rgba}", xywh(&rect))
        }
        Kind::Image => {
            if rgba.ends_with(",1.000") || rgba.ends_with(",1.0") {
                format!(
                    "{},{},{}",
                    xywh(&rect),
                    texture,
                    nine_slice(&node.image.as_ref().and_then(|image| image.nine_slice))
                )
            } else {
                return Some(mismatch(
                    probe,
                    format!(
                        "{} 的贴图被写了半透明（{rgba}）：动态层的 ui.image 没有 alpha 可给",
                        tail(&texture)
                    ),
                ));
            }
        }
        Kind::Text => {
            let Some(text) = &node.text else {
                return Some(mismatch(probe, "没有 Text 组件".to_owned()));
            };
            // `trim_end` 是跟动态层那一侧对齐：它的读数过 `lua_str`（整串 trim），文字尾部的
            // 填空空格已经被吃掉；头部空格（行对齐用的那两个）在中段，两边都留得下来。
            format!(
                "{},{:.2},{}",
                xy(&rect),
                text.font_size,
                fitted(&text.text).trim_end()
            )
        }
    };
    Some(Reading {
        carrier: Carrier::Entity,
        rect: Some(rect),
        order: format!("{:08}", node.slot),
        texture: if probe.kind == Kind::Text {
            String::new()
        } else {
            texture
        },
        rgba,
        body,
    })
}

/// 实体层读数与动态层不同形的坏法：不 panic，把它印成一行"必然与黄金表不等"的文本。
fn mismatch(probe: &Probe, why: String) -> Reading {
    Reading {
        carrier: Carrier::Entity,
        rect: None,
        order: String::new(),
        texture: String::new(),
        rgba: String::new(),
        body: format!("实体层读数不合法：{}（{why}）", probe.role),
    }
}

fn dynamic_reading(runtime: &mut RuntimeScene, probe: &Probe) -> Result<Option<Reading>, String> {
    let id = probe.dynamic;
    let expr = match probe.kind {
        Kind::Panel => format!(
            "(function() local p = ui.panel_get('{id}') if not p then return 'absent' end \
             if p.visible == false then return 'hidden' end \
             return string.format('%.2f,%.2f,%.2f,%.2f,%.3f,%.3f,%.3f,%.3f', \
             p.x, p.y, p.width, p.height, p.r, p.g, p.b, p.a) end)()"
        ),
        Kind::Image => format!(
            "(function() local i = ui.image_get('{id}') if not i then return 'absent' end \
             return string.format('%.2f,%.2f,%.2f,%.2f,%s,%s', \
             i.x, i.y, i.width, i.height, i.texture, i.nine_slice or '-') end)()"
        ),
        Kind::Text => format!(
            "(function() local t = ui.text_get('{id}') if not t then return 'absent' end \
             if t.visible == false then return 'hidden' end \
             return string.format('%.2f,%.2f,%.2f,%s', t.x, t.y, t.size, t.text) end)()"
        ),
    };
    let text = lua_str(runtime, &expr).map_err(|error| error.to_string())?;
    if text == "absent" {
        return Ok(None);
    }
    let hidden = text == "hidden";
    // 动态层的 body 与实体层同一形状：Image = `x,y,w,h,texture,nine_slice`（`image_get` 没有
    // alpha 可比，所以那边也不印颜色）；Panel = `x,y,w,h,r,g,b,a`。
    let body = match probe.kind {
        Kind::Image if !hidden => {
            // 动态层的 `nine_slice` 是一个**带逗号的字符串**（"16,16,16,16"），所以不能按
            // 逗号取六段——那会把九宫格切成只剩第一段（第一版就这么把"这块板子怎么拉伸"
            // 这份版面数据从黄金表里丢了）。前六段之后再合并一次。
            let mut parts = text.splitn(6, ',');
            let head: Vec<&str> = parts.by_ref().take(5).collect();
            let slice = parts.next().unwrap_or("-");
            format!("{},{}", head.join(","), slice)
        }
        _ if !hidden => text.clone(),
        _ => "hidden".to_owned(),
    };
    let rect = if hidden {
        None
    } else {
        let mut parts = text.split(',');
        match (
            parts.next().and_then(|v| v.parse::<f32>().ok()),
            parts.next().and_then(|v| v.parse::<f32>().ok()),
            parts.next().and_then(|v| v.parse::<f32>().ok()),
            parts.next().and_then(|v| v.parse::<f32>().ok()),
        ) {
            (Some(x), Some(y), Some(w), Some(h)) => Some(UiRect { x, y, w, h }),
            _ if probe.kind == Kind::Text => None,
            _ => return Err(format!("{} 的动态层读数解析不了：{text}", probe.role)),
        }
    };
    let texture = if probe.kind == Kind::Image && !hidden {
        text.split(',').nth(4).unwrap_or("").to_owned()
    } else if probe.kind == Kind::Panel {
        "__white__".to_owned()
    } else {
        String::new()
    };
    let rgba = if probe.kind == Kind::Panel && !hidden {
        text.split(',')
            .skip(4)
            .take(4)
            .collect::<Vec<_>>()
            .join(",")
    } else {
        String::new()
    };
    Ok(Some(Reading {
        carrier: if hidden {
            Carrier::Absent
        } else {
            Carrier::Dynamic
        },
        rect,
        order: id.to_owned(),
        texture,
        rgba,
        body,
    }))
}

/// 整屏同层：一屏里的块只许同属一个载体（`Absent` 允许——那一帧本来就不该出现）。
fn same_layer_screen(carriers: &[(String, Carrier)]) -> Result<(), Box<dyn std::error::Error>> {
    let entity = carriers
        .iter()
        .filter(|(_, carrier)| *carrier == Carrier::Entity)
        .count();
    let dynamic = carriers
        .iter()
        .filter(|(_, carrier)| *carrier == Carrier::Dynamic)
        .count();
    println!(
        "  实体层 {entity} 块、动态层 {dynamic} 块（两态合计 {}/{}）",
        entity + dynamic,
        carriers.len()
    );
    if entity > 0 && dynamic > 0 {
        let mixed: Vec<&str> = carriers
            .iter()
            .filter(|(_, carrier)| *carrier != Carrier::Absent)
            .map(|(role, _)| role.as_str())
            .collect();
        return Err(format!(
            "这一屏半搬了：既有实体层的块又有动态层的块（{}）。实体层整体画在动态层之上，\
             半搬等于改层序（②）",
            mixed.join(" ")
        )
        .into());
    }
    Ok(())
}

// ---------- ③ 覆盖账 ----------

/// ③ 的账要进黄金表才算断言：只打印的话，"谁盖谁"变了门照样绿，而那正是搬层序最容易改坏的一维
/// （M63 用段序表钉的就是这件事）。每个状态每块一行 `cover`，右边是"谁压在我上面并且整块盖住我"。
fn covering_account(
    state: &State,
    tag: &str,
    dump: &mut String,
) -> Result<(), Box<dyn std::error::Error>> {
    // 类别秩：色块（内置白图）→ 贴图块 → 文字。这一维两边**共用**，由 `frame_data.rs:869-883`
    // 的 push 顺序决定（`push_ui_panels` → `push_ui_images` → `push_ui_grids` → `push_ui_bars`
    // → `push_ui_text`），并由 [`submission_order`] 拿真实提交序列对过一次账。
    // 类别内序才是两套载体不同的地方：动态层 = id 字典序（`ScriptState` 是 BTreeMap，
    // `mesh_ui.rs:239` 迭代 `.values()`），实体层 = 槽位升序（预设行序）。
    let rank = |reading: &Reading| match reading.texture.as_str() {
        "__white__" => 0,
        _ if !reading.texture.is_empty() => 1,
        _ => 2,
    };
    let mut order: Vec<(&'static str, Reading)> = state
        .iter()
        .map(|(role, reading)| (*role, reading.clone()))
        .collect();
    order.sort_by(|(_, a), (_, b)| rank(a).cmp(&rank(b)).then(a.order.cmp(&b.order)));
    let mut covered = 0;
    for (index, (role, reading)) in order.iter().enumerate() {
        let covers: Vec<&str> = match reading.rect {
            None => Vec::new(),
            Some(rect) => order[index + 1..]
                .iter()
                .filter(|(_, above)| {
                    above.opaque() && above.rect.is_some_and(|other| contains(&other, &rect))
                })
                .map(|(above, _)| *above)
                .collect(),
        };
        if covers.is_empty() {
            println!("  {role:<16} 看得见");
        } else {
            covered += 1;
            println!("  {role:<16} 被盖住 ← {}", covers.join(" "));
        }
        dump.push_str(&format!(
            "{tag}\tcover\t{}\t{}\t{}\n",
            role,
            kind_of(role),
            if covers.is_empty() {
                "-".to_owned()
            } else {
                covers.join(" ")
            }
        ));
    }
    println!("  被盖住 {covered} 块（不透明判定：{OPAQUE_NOTE}）");
    Ok(())
}

/// 黄金表第二列 `cover` 行的种类列取值（保持四列结构，别让读表的人按种类筛时空一格）。
fn kind_of(role: &str) -> &'static str {
    PROBES
        .iter()
        .find(|probe| probe.role == role)
        .map_or("?", |probe| probe.kind.tag())
}

/// ③ 的排序规则不能只是"我读了 push 顺序得出的推断"：这里拿宿主**真正提交的序列**
/// （`mesh.draws`，下标即画家顺序）证一次——内置白图那一段必须排在
/// `ui_panel_main_9s.png`（三页 `skin_bg` 用的整屏不透明九宫格）之前。
/// 这条一旦反过来，③ 那张覆盖表就是拿错的规则算出来的，整段失去意义。
fn submission_order(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W as u32, VIEW_H as u32)?;
    let first = |role: &str| {
        mesh.draws.iter().position(|draw| {
            if role == WHITE_ROLE {
                draw.texture == 0 || draw_path(&mesh, draw).is_some_and(|path| path == "__white__")
            } else {
                draw_path(&mesh, draw).is_some_and(|path| tail(path) == role)
            }
        })
    };
    let white = first(WHITE_ROLE);
    let skin = first(SKIN_TEXTURE);
    println!(
        "  本帧提交 {} 个分段：白图段 {}、{SKIN_TEXTURE} 段 {}",
        mesh.draws.len(),
        white.map_or("无".to_owned(), |index| format!("#{index}")),
        skin.map_or("无".to_owned(), |index| format!("#{index}")),
    );
    match (white, skin) {
        (Some(white), Some(skin)) if white < skin => {
            println!("  ⇒ 实测类别序与 ③ 用的规则一致（色块 #{white} 在贴图 #{skin} 之前提交）")
        }
        (Some(white), Some(skin)) => {
            return Err(format!(
                "③ 的排序规则被实测否了：白图段 #{white} 在 {SKIN_TEXTURE} 段 #{skin} **之后**提交，\
                 而 ③ 按`色块 → 贴图`算覆盖表（frame_data.rs 的 push 顺序与提交顺序不一致，先查合并批的那一步）"
            )
            .into());
        }
        _ => {
            println!(
                "  ⚠ 这一帧没同时拿到白图段与 {SKIN_TEXTURE} 段 ⇒ 覆盖表只按规则排，这次没有实测背书"
            );
        }
    }
    Ok(())
}

/// `SceneDraw.texture` 走的是**约定下标**（`frame_data.rs` 写入时 `+1`，`texture == 0` 是内置
/// 白图），直接索引 `paths` 会整体错开一格（m35 翻过车）。
fn draw_path<'a>(mesh: &'a SceneMesh, draw: &SceneDraw) -> Option<&'a str> {
    if draw.texture == 0 {
        return None;
    }
    mesh.paths
        .get((draw.texture - 1) as usize)
        .map(String::as_str)
}

/// 纯色块（`ui.panel`）在提交序列里的角色名：它没有文件名，以 `texture == 0` 出现。
const WHITE_ROLE: &str = "<内置白图>";
/// `pse:skin_bg` 的贴图末段名（③ 的实测背书用它）。
const SKIN_TEXTURE: &str = "ui_panel_main_9s.png";

/// `outer` 是否整块盖住 `inner`（含边界相等：同矩形就是全盖）。
fn contains(outer: &UiRect, inner: &UiRect) -> bool {
    outer.x <= inner.x + 0.5
        && outer.y <= inner.y + 0.5
        && outer.right() >= inner.right() - 0.5
        && outer.bottom() >= inner.bottom() - 0.5
}

// ---------- ④ 文字着色 ----------

fn text_colors(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    let nodes = read_nodes(runtime)?;
    let mut checked = 0;
    for probe in PROBES {
        if probe.kind != Kind::Text {
            continue;
        }
        for name in probe.entities {
            let Some(node) = nodes.get(*name) else {
                continue; // 还没搬进预设（迁移前）／这一块这一态不在
            };
            let Some(text) = &node.text else { continue };
            checked += 1;
            if text.color != [1.0, 1.0, 1.0] {
                return Err(format!(
                    "{name} 的文字色是 {:?}，而动态层的 UI 文字写死白（mesh_ui.rs:416,422）\
                     ⇒ 搬进来顺手改了字色",
                    text.color
                )
                .into());
            }
        }
    }
    println!("  预设里的 Text：{checked} 条，逐条与动态层的白色对齐（0 条 = 还没搬）");
    Ok(())
}

// ---------- ⑤ 跨屏相交 ----------

fn cross_screen(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    // Lua 侧没有"枚举全部动态块"的 API，所以这一维在 Rust 侧读宿主那份真源
    // （`with_dynamic_ui` = 面板/贴图/网格，`with_ui` = 文字/条），只留**与本页面板矩形相交**的。
    let hits = runtime.with_script(|script| {
        let mut hits: Vec<String> = Vec::new();
        script.with_dynamic_ui(|panels, images, grids| {
            for (id, panel) in panels {
                if panel.visible {
                    push_overlap(&mut hits, id, panel.position, panel.size);
                }
            }
            for (id, image) in images {
                push_overlap(&mut hits, id, image.position, image.size);
            }
            for (id, grid) in grids {
                push_overlap(&mut hits, id, grid.position, grid.total_size());
            }
        });
        script.with_ui(|texts, bars| {
            // 文字没有尺寸：按它的落点做点包含判定（一个字都探不到就是不在本页里）。
            for (id, text) in texts {
                if text.visible {
                    push_overlap(&mut hits, id, text.position, [0.0, 0.0]);
                }
            }
            for (id, bar) in bars {
                push_overlap(&mut hits, id, bar.position, bar.size);
            }
        });
        hits.sort();
        hits
    });
    if hits.is_empty() {
        println!(
            "  与暂停面板 ({},{},{}×{}) 相交的其他动态块：0 个 ⇒ 整屏搬不会把谁推到预设底下",
            PANEL.x, PANEL.y, PANEL.w, PANEL.h
        );
        return Ok(());
    }
    Err(format!(
        "这些动态块与暂停面板矩形相交：{}。整屏搬进预设后它们会跑到预设**底下**\
         （`push_scene_ui` 在最后），那是改画面，必须先处理",
        hits.join(" ")
    )
    .into())
}

/// 本页自己的块（`pse:` 前缀，三页共用）不算"别人的"；其余按 (位置, 尺寸) 与 [`PANEL`] 判相交。
fn push_overlap(hits: &mut Vec<String>, id: &str, position: [f32; 2], size: [f32; 2]) {
    if id.starts_with("pse:") {
        return;
    }
    let other = UiRect {
        x: position[0],
        y: position[1],
        w: size[0],
        h: size[1],
    };
    if PANEL.x < other.right()
        && other.x < PANEL.right()
        && PANEL.y < other.bottom()
        && other.y < PANEL.bottom()
    {
        hits.push(id.to_owned());
    }
}

// ---------- 公共读数 ----------

/// 活世界里一个节点的版面数据。
#[derive(Default, Clone)]
struct Node {
    slot: u32,
    layout: Option<UiRectLayout>,
    text: Option<UiText>,
    image: Option<UiImage>,
}

fn read_nodes(
    runtime: &mut RuntimeScene,
) -> Result<BTreeMap<&'static str, Node>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    for probe in PROBES {
        for name in probe.entities {
            names.push(*name);
        }
    }
    let found: BTreeMap<&'static str, EntityId> = names
        .into_iter()
        .filter_map(|name| entity_id(runtime, name).ok().flatten().map(|id| (name, id)))
        .collect();
    let ids: Vec<(&'static str, EntityId)> = found.into_iter().collect();
    let read = runtime.with_script(move |script| {
        script.with_world(|world| {
            ids.iter()
                .map(|(name, id)| {
                    let mut node = Node {
                        slot: id.index(),
                        ..Default::default()
                    };
                    if let Ok(Some(layout)) = world.get::<UiRectLayout>(*id) {
                        node.layout = Some(layout.clone());
                    }
                    if let Ok(Some(text)) = world.get::<UiText>(*id) {
                        node.text = Some(text.clone());
                    }
                    if let Ok(Some(image)) = world.get::<UiImage>(*id) {
                        node.image = Some(image.clone());
                    }
                    (*name, node)
                })
                .collect::<BTreeMap<_, _>>()
        })
    });
    Ok(read)
}

fn entity_id(
    runtime: &mut RuntimeScene,
    name: &str,
) -> Result<Option<EntityId>, Box<dyn std::error::Error>> {
    let text = lua_str(runtime, &format!("entity.find('{name}')"))?;
    let Ok(raw) = text.parse::<f64>() else {
        return Ok(None);
    };
    let raw = raw as u64;
    Ok(Some(EntityId::from_raw((raw >> 32) as u32, raw as u32)))
}

fn compare_golden(dump: &str) -> Result<(), Box<dyn std::error::Error>> {
    let want: Vec<&str> = GOLDEN.lines().filter(|line| !line.is_empty()).collect();
    let got: Vec<&str> = dump.lines().filter(|line| !line.is_empty()).collect();
    if want.len() != got.len() {
        return Err(format!(
            "黄金对账行数不一致：期望 {}、实得 {}（少/多出来的块就是画面少/多出来的块）",
            want.len(),
            got.len()
        )
        .into());
    }
    // 逐行按"排序键"比对：shown/hidden 段两载体行序同（PROBES 定义序）；cover 段行序
    // 载体相关（动态层 = id 字典序、实体层 = 槽位号；行序是排序产物不是画面属性）——
    // 排序后每行内容唯一（每态每 role 一行），按内容对齐即可比出"谁变了"。
    let mut want = want;
    let mut got = got;
    want.sort_unstable();
    got.sort_unstable();
    for (index, pair) in want.iter().zip(got.iter()).enumerate() {
        if pair.0 != pair.1 {
            return Err(format!(
                "黄金对账第 {} 行（排序键）不一致：\n  期望 {}\n  实得 {}",
                index + 1,
                pair.0,
                pair.1
            )
            .into());
        }
    }
    Ok(())
}

fn tail(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn xy(rect: &UiRect) -> String {
    format!("{:.2},{:.2}", rect.x, rect.y)
}

fn xywh(rect: &UiRect) -> String {
    format!("{:.2},{:.2},{:.2},{:.2}", rect.x, rect.y, rect.w, rect.h)
}

/// 九宫格边：与 `UiImage::to_component_str` 同口径（无九宫格印 `-`）。
fn nine_slice(slice: &Option<[f32; 4]>) -> String {
    slice.map_or("-".to_owned(), |[left, right, top, bottom]| {
        format!("{left},{right},{top},{bottom}")
    })
}

/// 行文本里的制表符会毁掉 TSV；这一屏的字里本来就没有，真出现就是数据坏了，替换后让它红。
fn fitted(text: &str) -> String {
    text.replace('\t', " ")
}

fn resolve_project(args: &[String]) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let project = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    Ok(project)
}

fn lua_str(runtime: &mut RuntimeScene, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let text = runtime
        .with_script(|script| script.execute_console(&format!("return tostring({expr})")))?;
    Ok(text.trim().to_owned())
}

fn query_num(runtime: &mut RuntimeScene, line: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let text = runtime.with_script(|script| script.execute_console(line))?;
    text.trim()
        .parse::<f64>()
        .map_err(|error| format!("{line} ⇒ {:?} 不是数：{error}", text.trim()).into())
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
