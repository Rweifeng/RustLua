//! Headless acceptance drive for M65：商店面板（`systems/shop.lua`）搬到一份 `.prefab`。
//!
//! 这是"面板五屏逐屏搬"的第一屏。选它是因为它的形状最全：整块面板 + 半透明色块 +
//! 9 行列表 + 移动高亮 + 双态页签 + 条件出现的滚动箭头，一屏把预设层要覆盖的写法走一遍。
//!
//! 门守的是**搬运不改画面**，四段账（与下面四个 banner 同号）：
//!   ① 逐 id 黄金对账（两个状态，数字取本次 `--capture` 实测）：`A` = 刚打开（购买页、选中
//!      第 1 行；`data/shops.lua` 的赵记摊本来就有 34 条库存 ⇒ 9 个可视槽**全满**，上下箭头
//!      在 A 态就出现），`B` = 追加 8 条后 42 行 + 选中末行 ⇒ 窗口翻到末段（第 1 槽是 34 号、
//!      第 9 槽是 42 号）。每块一条 `shown` 行，读数**与载体无关**：同一块东西由动态层画还是
//!      由预设实例画，印出来必须是同一串字节（矩形/贴图/九宫格/字/字号/颜色）。两块都在画 =
//!      重影，当场红；都没画 = 这一帧不该出现，记一行 `hidden` 交给黄金表管（谁把 `shown`
//!      变成 `hidden` 或反过来，都在这一格上红）。
//!   ② 载体记账 + **整屏同层**：一屏里的块只许"全在动态层"或"全在实体层"。半搬是最坏的
//!      形状——实体层的图整体画在动态层**之下**（`frame_data.rs`：`push_scene_ui_images` 排在
//!      `push_ui_*` 之前），于是"没搬的那块"会盖住"搬进来的那块"，而①的矩形读数对此全瞎。
//!   ③ 覆盖账：谁盖住谁。这条是 ① 在层序维度的另一半，而且**跨载体可比**：
//!      迁移前"谁盖谁"由 `ui.*` 的 BTreeMap 键序（同类内 id 字典序）决定，迁移后由
//!      预设的行序（= 实体槽位升序）决定，两套序不同源，所以只能量结果不能量规则。
//!      绘制序在本门里是**测出来的**不是假设的：实体层按 `EntityId::index()` 排（引擎就是按
//!      槽位升序提交，`world.for_each::<UiImage>`），动态层按 (类别, id) 排；层与层之间的
//!      档序（实体图 < 动态 panel/image/grid/bar < 动态字 < 实体字）是引擎不变式，覆盖表就按它排。
//!      [`submission_order`] 在这一屏只报段序读数、不做背书（整屏同层时"白图在贴图之前"只是
//!      槽位序的产物；跨层的对由 jhyt_m63 的段序表与 jhyt_m70 的层档腿钉）。
//!      不透明判定用贴图中心的实际 alpha（`--capture` 打印，见 [`OPAQUE_NOTE`]）。
//!      本次 `--capture` 实测：A 态有 **8 块**被 `shp:skin_bg` 整块盖住（`box/rule/sel` 三个色块
//!      因"色块类别先提交"而全藏，`close/divider/seal/scroll_up/scroll_down` 因 id 字典序排在
//!      `skin_bg` 之前而全藏），也就是**选中行高亮今天看不见**；B 态另有一处：`shp:sel` 的 y 是
//!      `面板顶 + 100 + (绝对行号-1)×行高`，翻页时不跟着窗口走，42 行时它落在 **y=1534**
//!      （面板只到 680），高亮彻底跑到画面外。这两条都是既有画面事实，不是搬运造成的，
//!      所以本批把它们**照抄进预设**（bug-for-bug），修与不修由用户裁定（账记在 `docs/69`）。
//!      覆盖表也进黄金表：每块一行 `cover`，右边那一列就是"谁整块盖住了我"（没被盖住写 `-`）。
//!      只打印的账改红了不会拦人，而层序恰恰是搬运最容易悄悄改掉的一维（M63 用段序表钉的同款）。
//!   ④ 文字着色：动态层的 UI 文字**写死白色**（`mesh_ui.rs:416,422` 传 `[1.0,1.0,1.0]`），
//!      而 `text_get` 读不到颜色 ⇒ ① 的黄金表管不了这一维。这一条单独钉：搬进预设的每条
//!      `Text` 的 `color` 必须就是白，否则"搬家"顺手改了字色。
//!
//! 本门**没有**的账，写清楚免得被当成有：
//!   * 跨屏层序（shop 面板与 HUD / 引导行的谁盖谁）：①②③ 只看本屏的块，跨屏那一份在
//!     `--capture` 的输出里点名"与本屏面板矩形相交的其他动态块"，今天实测为**空集**才允许搬。
//!   * 版面数据（`authored`）没有迁移前基线：这一屏的版面今天住在 Lua 代码里，不在任何
//!     数据文件里，所以没有"预设里写的就是原来写的"可比。① 的 `shown` 行是唯一的那份账。
//!   * ④（字色）与 `entity_reading` 里那两条误用账（色块的贴图不是内置白图、贴图块带半透明）
//!     **在迁移前造不出红**：今天实体层一块都没有，能凭空造出来的只有"两层各画一遍"，而那先被
//!     ① 拦下（②也拦得住——半搬）。它们的反面对照排在 M65b：整屏搬完之后把一条 `Text` 的
//!     `color` 改成非白、把一个 `Image` 的 alpha 调低，各自必须红一次。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m65_shop_prefab [-- <game_jhyt-root>] [--capture]
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
const GOLDEN: &str = include_str!("support/jhyt_m65_shop_golden.tsv");

const VIEW_W: f32 = 1280.0;
const VIEW_H: f32 = 720.0;
const DT: f32 = 1.0 / 60.0;
/// 商店 id（`data/shops.lua` 的云泽村赵记摊）。
const SHOP_ID: &str = "zhao_stall";
/// `B` 态追加的库存条数：`zhao_stall` 数据里已有 34 条，+8 = 42 行，选中末行才会把窗口推到
/// 末段（第 1..9 槽显示 34..42 号），与 `jhyt_m20_audio` 的 M20-02e 同一条路。
const EXTRA_STOCK: usize = 8;
/// 可视行数与行高（`shop.lua:49,51`）。
const ROW_LIMIT: usize = 9;
/// 面板矩形（`layout.PANEL.shop` = 160,40,720,640），④ 用它判"与本页相交的其他动态块"。
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

macro_rules! row_probes {
    ($tail:expr) => {
        concat!("shp:row", $tail)
    };
}

/// 商店面板的全部可见物，顺序即黄金表顺序。
const PROBES: &[Probe] = &[
    probe("panel/box", "shp:box", &["ShopBox"], Kind::Panel),
    probe("panel/rule", "shp:rule", &["ShopRule"], Kind::Panel),
    probe("panel/sel", "shp:sel", &["ShopSel"], Kind::Panel),
    probe("img/skin_bg", "shp:skin_bg", &["ShopSkinBg"], Kind::Image),
    probe("img/divider", "shp:divider", &["ShopDivider"], Kind::Image),
    probe("img/seal", "shp:seal", &["ShopSeal"], Kind::Image),
    probe("img/close", "shp:close", &["ShopClose"], Kind::Image),
    probe(
        "img/tab_buy",
        "shp:tab_buy",
        &["ShopTabBuyOn", "ShopTabBuyOff"],
        Kind::Image,
    ),
    probe(
        "img/tab_sell",
        "shp:tab_sell",
        &["ShopTabSellOn", "ShopTabSellOff"],
        Kind::Image,
    ),
    probe(
        "img/step_minus",
        "shp:step_minus",
        &["ShopStepMinus"],
        Kind::Image,
    ),
    probe(
        "img/step_plus",
        "shp:step_plus",
        &["ShopStepPlus"],
        Kind::Image,
    ),
    probe(
        "img/scroll_up",
        "shp:scroll_up",
        &["ShopScrollUp"],
        Kind::Image,
    ),
    probe(
        "img/scroll_down",
        "shp:scroll_down",
        &["ShopScrollDown"],
        Kind::Image,
    ),
    probe("txt/title", "shp:title", &["ShopTitle"], Kind::Text),
    probe("txt/money", "shp:money", &["ShopMoney"], Kind::Text),
    probe("txt/tab1", "shp:tab1", &["ShopTabLabel1"], Kind::Text),
    probe("txt/tab2", "shp:tab2", &["ShopTabLabel2"], Kind::Text),
    probe("txt/qty", "shp:qty", &["ShopQty"], Kind::Text),
    probe("txt/row1", row_probes!("1"), &["ShopRow1"], Kind::Text),
    probe("txt/row2", row_probes!("2"), &["ShopRow2"], Kind::Text),
    probe("txt/row3", row_probes!("3"), &["ShopRow3"], Kind::Text),
    probe("txt/row4", row_probes!("4"), &["ShopRow4"], Kind::Text),
    probe("txt/row5", row_probes!("5"), &["ShopRow5"], Kind::Text),
    probe("txt/row6", row_probes!("6"), &["ShopRow6"], Kind::Text),
    probe("txt/row7", row_probes!("7"), &["ShopRow7"], Kind::Text),
    probe("txt/row8", row_probes!("8"), &["ShopRow8"], Kind::Text),
    probe("txt/row9", row_probes!("9"), &["ShopRow9"], Kind::Text),
    probe("txt/msg", "shp:msg", &["ShopMsg"], Kind::Text),
    probe("txt/hint", "shp:hint", &["ShopHint"], Kind::Text),
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
    enter_shop(&mut runtime)?;

    banner("①+② 逐 id 黄金对账（A/B 两态）+ 载体记账");
    let mut dump = String::new();
    let mut carriers = Vec::new();
    // A 态：刚打开。
    let state_a = collect_state(&mut runtime, "A", &mut dump, &mut carriers)?;
    // B 态：12 行库存 + 选中最后一行（翻页 + 箭头）。
    expand_stock(&mut runtime)?;
    let state_b = collect_state(&mut runtime, "B", &mut dump, &mut carriers)?;
    // B 态必须真的是"翻页后的形状"，否则这把钥匙只测了 A 态（12 行却只画 4 行 = 窗口没动）。
    let shown_rows = state_b
        .keys()
        .filter(|role| role.starts_with("txt/row"))
        .count();
    if shown_rows != ROW_LIMIT {
        return Err(format!(
            "B 态只有 {shown_rows} 行文字被画，期望占满 {ROW_LIMIT} 个可视槽（列表窗口没翻起来）"
        )
        .into());
    }
    same_layer_screen(&carriers)?;

    banner("③ 覆盖账：谁盖住谁（跨载体可比，绘制序按实测排；结果进黄金表）");
    covering_account(&state_a, "A", &mut dump)?;
    covering_account(&state_b, "B", &mut dump)?;

    if capture {
        print!("{dump}");
        println!("jhyt_m65_captured lines={}", dump.lines().count());
    } else {
        compare_golden(&dump)?;
        println!("golden_lines={}", dump.lines().count());
    }

    submission_order(&mut runtime)?;

    banner("④ 文字着色：动态层写死白 ⇒ 预设里每条 Text 必须也是白");
    text_colors(&mut runtime)?;

    banner("⑤ 跨屏相交：本屏面板与「别人的动态块」有没有重叠（没有才允许整屏搬）");
    cross_screen(&mut runtime)?;

    println!("jhyt_m65_shop_prefab_ok project={}", project.display());
    Ok(())
}

// ---------- 驱动 ----------

/// 走进商店：门面 → 山庄 → `shop.open_ui`。`scene.load` 只挂起请求，由宿主在步进里应用。
fn enter_shop(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.with_script(|script| script.execute_console("scene.load('manor')"))?;
    run(runtime, 6)?;
    let opened = lua_str(
        runtime,
        &format!("tostring(require('systems.shop').open_ui('{SHOP_ID}'))"),
    )?;
    if opened != "true" {
        return Err(format!("打不开商店 {SHOP_ID}（open_ui 返回 {opened}）").into());
    }
    let mode = lua_str(runtime, "require('core.state').mode")?;
    if mode != "Shop" {
        return Err(format!("open_ui 之后模式是 {mode}，不是 Shop").into());
    }
    run(runtime, 2)?;
    Ok(())
}

/// 追加库存到超过一页，再把选中行推到最后一行并重绘（B 态）。
fn expand_stock(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    let before = query_num(runtime, "return #require('systems.shop').rows()")?;
    runtime.with_script(|script| {
        script.execute_console(&format!(
            "local def = require('data.shops').{SHOP_ID}; \
             for i = 1, {EXTRA_STOCK} do \
               def.stock[#def.stock + 1] = {{ item = 'timber', price = 15 + i, limit = 20 }} \
             end; return true"
        ))
    })?;
    let want = before + EXTRA_STOCK as f64;
    let after = query_num(runtime, "return #require('systems.shop').rows()")?;
    if after != want {
        return Err(format!("追加库存后行数 {after}，期望 {want}").into());
    }
    runtime.with_script(|script| {
        script.execute_console(&format!(
            "local shop = require('systems.shop') shop.select({}) shop.render() return true",
            want as i64
        ))
    })?;
    run(runtime, 2)?;
    let selected = query_num(runtime, "return require('systems.shop').selected()")?;
    if (selected - want).abs() > f64::EPSILON {
        return Err(format!("选中行是 {selected}，期望 {want}（select 或 focus 环没跟上）").into());
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
            "这一屏半搬了：既有实体层的块又有动态层的块（{}）。实体层的图整体画在动态层之下、动态块浮在骨架之上，\
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
    // 提交档：**先分层、层内再分类别**，与 `frame_data.rs` 的 push 次序一一对应
    // （`push_scene_ui_images` → `push_ui_panels` → `push_ui_images` → `push_ui_grids`
    // → `push_ui_bars` → `push_ui_text` → `push_scene_ui_text`）。
    // 旧模型只有"色块 → 贴图 → 文字"一维，那等于假设两层按类别交错提交；实测不是，
    // 而错的方向正好是把"实体卡底埋掉动态图标"这种事故看成合理（实机 2026-10-07）。
    // 档内序才是两套载体不同的地方：动态层 = id 字典序（`ScriptState` 是 BTreeMap，
    // `mesh_ui.rs` 迭代 `.values()`），实体层 = 槽位升序（预设行序）。
    let slot_of = |role: &str, reading: &Reading| -> u8 {
        match reading.carrier {
            Carrier::Entity => {
                if kind_of_role(role) == Kind::Text {
                    4
                } else {
                    0
                }
            }
            Carrier::Dynamic => match kind_of_role(role) {
                Kind::Panel => 1,
                Kind::Image => 2,
                Kind::Text => 3,
            },
            // 这一帧根本没画：排在最后，反正 `rect` 为 None 谁也盖不住它。
            Carrier::Absent => 5,
        }
    };
    let mut order: Vec<(&'static str, Reading)> = state
        .iter()
        .map(|(role, reading)| (*role, reading.clone()))
        .collect();
    order.sort_by(|(role_a, a), (role_b, b)| {
        slot_of(role_a, a)
            .cmp(&slot_of(role_b, b))
            .then(a.order.cmp(&b.order))
            .then_with(|| role_a.cmp(role_b))
    });
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
    kind_of_role(role).tag()
}

/// ③ 的分层判据要看种类（实体层的图与字分属首尾两档），所以这里要的是 `Kind` 本身。
fn kind_of_role(role: &str) -> Kind {
    PROBES
        .iter()
        .find(|probe| probe.role == role)
        .map_or(Kind::Image, |probe| probe.kind)
}

/// ③ 的覆盖表按"层档 → 档内槽位"排，档序是引擎不变式（`frame_data.rs`：
/// `push_scene_ui_images` 在 `push_ui_*` 之前）。这条**只报**本帧的段序读数
/// （`mesh.draws`，下标即画家顺序）做对照，不再充当 ③ 的背书：这一屏是整屏同层（②），
/// 内置白图与 `ui_panel_main_9s.png` 同属"实体图"那一档，档内按预设槽位排，所以
/// "白图在贴图之前"是槽位序的产物而不是规则来源——把它当规则去断言，有人挪一行预设就会假红。
/// 真正钉"骨架在下、动态在上"的是 jhyt_m63 的整段段序表与 jhyt_m70 的层档腿：面板屏按 ⑤
/// 的前提本来就没有与骨架相交的动态块，层序在这一屏没有可判别的对，量不到就照实说。
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
        "  本帧提交 {} 个分段：白图段 {}、{SKIN_TEXTURE} 段 {}（只报读数，不当背书，见函数注释）",
        mesh.draws.len(),
        white.map_or("无".to_owned(), |index| format!("#{index}")),
        skin.map_or("无".to_owned(), |index| format!("#{index}")),
    );
    if white.is_none() || skin.is_none() {
        println!("  ⚠ 这一帧没同时拿到白图段与 {SKIN_TEXTURE} 段 ⇒ 段序读数只有一半");
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
/// `shp:skin_bg` 的贴图末段名（③ 的实测背书用它）。
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
            "  与商店面板 ({},{},{}×{}) 相交的其他动态块：0 个 ⇒ 整屏搬不会把谁推到预设底下",
            PANEL.x, PANEL.y, PANEL.w, PANEL.h
        );
        return Ok(());
    }
    Err(format!(
        "这些动态块与商店面板矩形相交：{}。整屏搬进预设后它们会浮在预设**之上**\
         （预设图趟排在 `push_ui_*` 之前），那是改画面，必须先处理",
        hits.join(" ")
    )
    .into())
}

/// 本页自己的块（`shp:` 前缀）不算"别人的"；其余按 (位置, 尺寸) 与 [`PANEL`] 判相交。
fn push_overlap(hits: &mut Vec<String>, id: &str, position: [f32; 2], size: [f32; 2]) {
    if id.starts_with("shp:") {
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
    for (index, pair) in want.iter().zip(got.iter()).enumerate() {
        if pair.0 != pair.1 {
            return Err(format!(
                "黄金对账第 {} 行不一致：\n  期望 {}\n  实得 {}",
                index + 1,
                pair.0,
                pair.1
            )
            .into());
        }
    }
    if want.len() != got.len() {
        return Err(format!(
            "黄金对账行数不一致：期望 {}、实得 {}（少/多出来的块就是画面少/多出来的块）",
            want.len(),
            got.len()
        )
        .into());
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
