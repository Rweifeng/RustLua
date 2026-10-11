//! Headless acceptance drive for M63：title_menu 门面搬到一份 `.prefab`。
//!
//! 门面是这一串迁移的试点屏（用户点的名），而它今天的画面是**两层合成**的：
//!   * `main.scene` 里 17 个带 `UiLayout` 的实体（背景/水墨/灯笼/三块木牌/三行标签/贴士/页脚），
//!     由 `tools/gen_main_menu_scene.py` 生成；
//!   * `scripts/ui/title_menu.lua` 每帧画的动态层：三行**标签文字**（场景那三份 `Text` 被
//!     `ui.scene_text_visible(name,false)` 全数隐藏）、选中高亮框、选中箭头、hover 贴图、
//!     四角/竹分隔/印章（走 `ui/skin.lua`）。
//!
//! 于是"门面长什么样"没有单一真源：挪一块木牌要同时改 `.scene` 与 Lua 两处，编辑器里也没有
//! 一份能双击预览的载体。M63 把它搬进 `assets/prefabs/title_menu.prefab`。
//!
//! 这道门守的是**搬运不许改画面**，而且刻意与"载体是谁"无关（三段账与下面三个 banner 同号）：
//!   ① 逐 id 黄金对账，一张表两层层：`shown` 行 = 玩家此刻看得见的每一块（矩形/贴图/文字/字号），
//!      读数来自宿主本帧真正用于绘制的矩形（`RuntimeScene::ui_rects_by_name`，名字换自那张全局
//!      实体名表 ⇒ 场景与 spawn 实例同源）与 `ui.*_get` 的动态层读数；同一块东西在实体层与动态层
//!      **各画一遍**就是重影（M22 修过的老毛病），当场红。迁移前后这张表逐字节相等，才谈得上
//!      "一像素没动"。`authored` 行 = 每个节点的版面数据（`UiLayout` 的 anchors/offsets 串、
//!      `Text` 的字与字号与色、`Image` 的贴图与九宫格、`UiEvents` 绑的回调名），它钉的是"预设里写的
//!      就是 `.scene` 里原来写的"——矩形对上了而九宫格边距丢了，也会在这里红。
//!   ② 标签两层的位置账：场景那份今天**不可见**、动态那份才是玩家看到的。实测两者差 2px
//!      （见 `LABEL_ALIGN_EPS` 那段），所以预设里那三行 `Text` 的 x 抄的是**画面**那一份；
//!      差值超过 4px 就红，因为那种量级说明两边不是同一坐标口径（M22 的重影差了 17px）。
//!   ③ 载体记账：逐块报"这一格今天由实体层还是动态层画"。迁移要改的就是这份账——标签与
//!      静态装饰从动态层搬进实体层，而①的两张表一个字都不许变。
//!   ④ 层序账：本帧 `mesh.draws` 的提交先后就是画家顺序（UI 层不写深度，后提交的盖住先提交
//!      的）。①的矩形读数对"画在谁底下"完全瞎：一块东西搬进/搬出预设时矩形可以一个像素都不变
//!      而从此被别人的贴图盖住。所以搬家之前先把"动态层与实体层谁盖谁"量成一张表，④ 钉住它，
//!      搬完之后同一张表必须还是同一个形状（见 `docs/66` §10）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m63_title_prefab [-- <game_jhyt-root>] [--capture]
//!
//! `--capture` 只打印读数表后退出（在**迁移前**的树上跑一次，产出的就是黄金表）：
//!   cargo run -p game --example jhyt_m63_title_prefab --capture

use engine::host::{RuntimeScene, SceneDraw, SceneMesh};
use rustlua_ecs::entity::EntityId;
use rustlua_input::input::InputSnapshot;
use rustlua_ui::ui::{UiImage, UiText};
use rustlua_ui::ui_events::UiEvents;
use rustlua_ui::ui_layout::{UiRect, UiRectLayout};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::PathBuf,
};

/// 黄金表：`层 \t 角色 \t 种类 \t 读数`，逐行与运行期读数比较。
const GOLDEN: &str = include_str!("support/jhyt_m63_title_menu_golden.tsv");

const VIEW_W: f32 = 1280.0;
const VIEW_H: f32 = 720.0;
const DT: f32 = 1.0 / 60.0;
/// 标签两层允许的坐标差。实测差 **2px**（`x = 578/590/590` 是脚本按真实字宽居中算的，
/// `main.scene` 里 authored 的是 `580/592/592`——生成器用的是"每字 25px"的估宽）：
/// 迁移时以**画面那一份**为准，所以预设里写 578/590/590。阈值留 4px 是钉另一种坏法——
/// M22 那次重影的两层差了 17px（`y=397` vs `y=380`），那种量级说明两边根本不是同一口径，
/// 照抄任何一份都会挪字。
const LABEL_ALIGN_EPS: f32 = 4.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// 贴图块（`Image` 组件 / `ui.image_get`）。
    Image,
    /// 文字块（`Text` 组件 / `ui.text_get`）。
    Text,
    /// 纯色面板（只有动态层有：选中高亮框）。
    Panel,
}

/// 一个"画面上的东西"可能在的两个载体。两边都在画 = 重影。
struct Probe {
    /// 角色名：与载体无关的稳定标识（迁移前后同一行对得上）。
    role: &'static str,
    /// 场景/预设里那个实体的名字。
    entity: Option<&'static str>,
    /// 动态层那个 `ui.*` id。
    dynamic: Option<&'static str>,
    kind: Kind,
}

const fn entity_probe(role: &'static str, entity: &'static str, kind: Kind) -> Probe {
    Probe {
        role,
        entity: Some(entity),
        dynamic: None,
        kind,
    }
}

const fn dynamic_probe(role: &'static str, dynamic: &'static str, kind: Kind) -> Probe {
    Probe {
        role,
        entity: None,
        dynamic: Some(dynamic),
        kind,
    }
}

const fn paired_probe(role: &'static str, entity: &'static str, dynamic: &'static str) -> Probe {
    Probe {
        role,
        entity: Some(entity),
        dynamic: Some(dynamic),
        kind: Kind::Text,
    }
}

/// 门面的全部可见物，顺序即黄金表的顺序。
const PROBES: &[Probe] = &[
    entity_probe("bg/night", "BgNight", Kind::Image),
    entity_probe("bg/vignette", "BgVignette", Kind::Image),
    entity_probe("ink/band", "InkBand", Kind::Image),
    entity_probe("logo", "Logo", Kind::Image),
    entity_probe("lantern/left", "LanternL", Kind::Image),
    entity_probe("lantern/right", "LanternR", Kind::Image),
    entity_probe("btn/BtnStart", "BtnStart", Kind::Image),
    entity_probe("btn/BtnContinue", "BtnContinue", Kind::Image),
    entity_probe("btn/BtnExit", "BtnExit", Kind::Image),
    paired_probe("label/BtnStart", "BtnStartLabel", "title:label:BtnStart"),
    paired_probe(
        "label/BtnContinue",
        "BtnContinueLabel",
        "title:label:BtnContinue",
    ),
    paired_probe("label/BtnExit", "BtnExitLabel", "title:label:BtnExit"),
    entity_probe("tip/scroll", "TipScroll", Kind::Image),
    entity_probe("tip/title", "TipTitle", Kind::Text),
    entity_probe("tip/body", "TipBody", Kind::Text),
    entity_probe("tip/body2", "TipBody2", Kind::Text),
    entity_probe("footer", "FooterText", Kind::Text),
    dynamic_probe("skin/btn_hover", "title:btn_hover", Kind::Image),
    dynamic_probe("skin/orn_tl", "title:orn_tl", Kind::Image),
    dynamic_probe("skin/orn_tr", "title:orn_tr", Kind::Image),
    dynamic_probe("skin/orn_bl", "title:orn_bl", Kind::Image),
    dynamic_probe("skin/orn_br", "title:orn_br", Kind::Image),
    dynamic_probe("skin/divider", "title:divider", Kind::Image),
    dynamic_probe("skin/seal", "title:seal", Kind::Image),
    dynamic_probe("hl/BtnStart", "title:hl:BtnStart", Kind::Panel),
    dynamic_probe("hl/BtnContinue", "title:hl:BtnContinue", Kind::Panel),
    dynamic_probe("hl/BtnExit", "title:hl:BtnExit", Kind::Panel),
    dynamic_probe("arrow/BtnStart", "title:arrow:BtnStart", Kind::Text),
    dynamic_probe("arrow/BtnContinue", "title:arrow:BtnContinue", Kind::Text),
    dynamic_probe("arrow/BtnExit", "title:arrow:BtnExit", Kind::Text),
    dynamic_probe("hint", "title:hint", Kind::Text),
];

/// ② 的 `authored` 行顺序 = 这 17 个实体名（门面上带版面数据的全部节点）。
const NODES: &[&str] = &[
    "BgNight",
    "BgVignette",
    "InkBand",
    "Logo",
    "LanternL",
    "LanternR",
    "BtnStart",
    "BtnStartLabel",
    "BtnContinue",
    "BtnContinueLabel",
    "BtnExit",
    "BtnExitLabel",
    "TipScroll",
    "TipTitle",
    "TipBody",
    "TipBody2",
    "FooterText",
];

/// 三行按钮标签：动态层与实体层的坐标口径由这一组对齐（`title_menu.lua` 的 `label_left`）。
const LABELS: &[(&str, &str)] = &[
    ("BtnStartLabel", "BtnStart"),
    ("BtnContinueLabel", "BtnContinue"),
    ("BtnExitLabel", "BtnExit"),
];

/// 活世界里一个节点的版面数据（② 的行由它生成）。
#[derive(Default)]
struct Node {
    layout: Option<UiRectLayout>,
    text: Option<UiText>,
    image: Option<UiImage>,
    events: Vec<(String, String)>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let capture = args.iter().any(|arg| arg == "--capture");
    let project = resolve_project(&args)?;

    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    // 门面的动态层是 `main.lua` 每帧画出来的；步数固定 ⇒ 读数也固定。
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    println!("  门面场景 = {:?}", lua_str(&mut runtime, "map.current()")?);

    let rects = runtime.ui_rects_by_name([VIEW_W, VIEW_H]);
    let hidden = runtime.with_script(|script| script.scene_text_hidden());
    let nodes = read_nodes(&mut runtime)?;
    let (dump, carrier) = collect_dump(&mut runtime, &rects, &hidden, &nodes)?;

    banner("① 逐 id 黄金对账：画面（shown）+ 版面数据（authored）");
    if capture {
        print!("{dump}");
        println!("jhyt_m63_captured lines={}", dump.lines().count());
    } else {
        compare_golden(&dump)?;
        println!("golden_lines={}", dump.lines().count());
    }

    banner("② 标签两层的位置账：谁在画、差多少（决定预设里那三行 Text 写哪个 x）");
    label_pairing(&mut runtime, &rects, &hidden)?;

    banner("③ 载体记账：每一块由谁画（实体层 = .scene 或 .prefab；动态层 = 脚本每帧画的）");
    let mut from_entity = 0;
    let mut from_dynamic = 0;
    let mut not_drawn = 0;
    for (role, carrier) in &carrier {
        match carrier {
            Carrier::Entity => from_entity += 1,
            Carrier::Dynamic => {
                from_dynamic += 1;
                println!("  {role} ← 动态层");
            }
            Carrier::Absent => {
                // 只有"这一刻不该出现"的动态块允许走到这里（未选中的高亮框与箭头）；
                // 有实体来源的块在 ① 里就已经判红了。
                not_drawn += 1;
                println!("  {role} ← 此刻没画");
            }
        }
    }
    println!("  实体层 {from_entity} 块、动态层 {from_dynamic} 块、未画 {not_drawn} 块");

    banner("④ 层序账：本帧提交的先后（谁盖谁）——搬进/搬出预设前必须先有这张表");
    layer_order(&mut runtime, capture)?;

    println!("jhyt_m63_title_prefab_ok project={}", project.display());
    Ok(())
}

// ---------- ① 读数 ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Carrier {
    Entity,
    Dynamic,
    Absent,
}

/// 一份读数表 + 每块的载体（①/③ 同源，避免跑两遍宿主）。
type Dump = (String, Vec<(String, Carrier)>);

/// 按 `PROBES` 出 `shown` 行、按 `NODES` 出 `authored` 行；同时回报每块的载体。
fn collect_dump(
    runtime: &mut RuntimeScene,
    rects: &BTreeMap<String, UiRect>,
    hidden: &BTreeSet<String>,
    nodes: &BTreeMap<&'static str, Node>,
) -> Result<Dump, Box<dyn std::error::Error>> {
    let mut out = String::new();
    let mut carriers = Vec::new();
    for probe in PROBES {
        let entity = probe
            .entity
            .and_then(|name| entity_reading(rects, hidden, nodes, name, probe.kind));
        let dynamic = match probe.dynamic {
            Some(id) => Some(dynamic_reading(runtime, id, probe.kind)?),
            None => None,
        };
        let drawn_entity = entity.is_some();
        let drawn_dynamic = matches!(dynamic.as_deref(), Some(reading) if !is_blank(reading));
        // 两边都在画 = 同一块东西叠两层。M22 的老毛病就是这个（一明一暗两遍字）。
        if drawn_entity && drawn_dynamic {
            return Err(format!(
                "{} 在实体层与动态层各画了一遍：实体 {:?} / 动态 {:?}",
                probe.role, entity, dynamic
            )
            .into());
        }
        let (carrier, reading) = match (entity, dynamic) {
            (Some(reading), _) => (Carrier::Entity, reading),
            (_, Some(reading)) if drawn_dynamic => (Carrier::Dynamic, reading),
            (_, Some(reading)) => (Carrier::Absent, reading),
            (None, None) => (Carrier::Absent, "missing".to_owned()),
        };
        // 有实体来源的块必须真的在画（哪一层画的记在 ③）：否则 `--capture` 会把一桌子
        // `missing` 钉成黄金表。迁移**前**三行标签正是"实体有、但由动态层画"，那合法。
        if probe.entity.is_some() && carrier == Carrier::Absent {
            return Err(format!(
                "{} 两层都没画（读数 {reading}）：门面少一块板子",
                probe.role
            )
            .into());
        }
        out.push_str(&format!(
            "shown\t{}\t{}\t{reading}\n",
            probe.role,
            kind_tag(probe.kind)
        ));
        carriers.push((probe.role.to_owned(), carrier));
    }
    for name in NODES {
        let node = nodes
            .get(name)
            .ok_or_else(|| format!("{name} 不在活世界里（② 的 authored 行没有来源）"))?;
        out.push_str(&format!(
            "authored\t{name}\tlayout\t{}\n",
            node.layout
                .as_ref()
                .map_or("none".to_owned(), UiRectLayout::to_component_str)
        ));
        if let Some(text) = &node.text {
            out.push_str(&format!(
                "authored\t{name}\ttext\t{},{},{}\n",
                g(text.font_size),
                text.text,
                rgb(&text.color)
            ));
        }
        if let Some(image) = &node.image {
            out.push_str(&format!(
                "authored\t{name}\timage\t{},{},{}\n",
                image.texture,
                nine_slice(&image.nine_slice),
                tint(&image.color)
            ));
        }
        if !node.events.is_empty() {
            let bound = node
                .events
                .iter()
                .map(|(event, function)| format!("{event}={function}"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("authored\t{name}\tevents\t{bound}\n"));
        }
    }
    Ok((out, carriers))
}

/// 实体层这一帧画没画：没有 `UiLayout` 就没有矩形；被 `ui.scene_text_visible(false)` 藏起来的
/// 文字也算没画（那条接口动的就是绘制，布局矩形照旧在）。
fn entity_reading(
    rects: &BTreeMap<String, UiRect>,
    hidden: &BTreeSet<String>,
    nodes: &BTreeMap<&'static str, Node>,
    name: &str,
    kind: Kind,
) -> Option<String> {
    let rect = rects.get(name)?;
    let node = nodes.get(name)?;
    match kind {
        Kind::Text => {
            if hidden.contains(name) {
                return None;
            }
            let text = node.text.as_ref()?;
            Some(format!("{},{},{}", xy(rect), g(text.font_size), text.text))
        }
        Kind::Image => {
            let image = node.image.as_ref()?;
            Some(format!(
                "{},{},{}",
                xywh(rect),
                image.texture,
                nine_slice(&image.nine_slice)
            ))
        }
        // 纯色面板只有动态层有（选中高亮框是脚本算出来的动画矩形）。
        Kind::Panel => None,
    }
}

fn is_blank(reading: &str) -> bool {
    reading == "missing" || reading == "hidden"
}

/// 动态层读数（`ui.*_get` 对不存在的 id 返回 nil ⇒ `missing`，不可见 ⇒ `hidden`）。
fn dynamic_reading(
    runtime: &mut RuntimeScene,
    id: &str,
    kind: Kind,
) -> Result<String, Box<dyn std::error::Error>> {
    let (getter, shape) = match kind {
        Kind::Text => (
            "ui.text_get",
            "string.format('%.2f,%.2f,%g,%s', t.x, t.y, t.size, tostring(t.text))",
        ),
        Kind::Image => (
            "ui.image_get",
            "string.format('%.2f,%.2f,%.2f,%.2f,%s,-', t.x, t.y, t.width, t.height, tostring(t.texture))",
        ),
        // 高亮框的 alpha 含呼吸项（`title_menu.M.anim()`），进黄金表就成了"每帧都在动"，
        // 所以只取几何。
        Kind::Panel => (
            "ui.panel_get",
            "string.format('%.2f,%.2f,%.2f,%.2f', t.x, t.y, t.width, t.height)",
        ),
    };
    lua_str(
        runtime,
        &format!(
            "(function() local t = {getter}('{id}') \
             if not t then return 'missing' end \
             if t.visible == false then return 'hidden' end \
             return {shape} end)()"
        ),
    )
}

fn compare_golden(dump: &str) -> Result<(), Box<dyn std::error::Error>> {
    let golden: Vec<&str> = GOLDEN.lines().filter(|line| !line.is_empty()).collect();
    let fresh: Vec<&str> = dump.lines().filter(|line| !line.is_empty()).collect();
    if golden.len() != fresh.len() {
        return Err(format!(
            "黄金表 {} 行，实得 {} 行（门面上的东西数量变了）",
            golden.len(),
            fresh.len()
        )
        .into());
    }
    for (index, (want, got)) in golden.iter().zip(fresh.iter()).enumerate() {
        if want != got {
            return Err(format!(
                "黄金对账第 {} 行不一致：\n  期望 {want}\n  实得 {got}",
                index + 1
            )
            .into());
        }
    }
    Ok(())
}

// ---------- ② authored 组件读数 ----------

/// 一次借用把 17 个节点的组件读全（`entity.find` 走的是全局名表，场景与 spawn 实例同源）。
fn read_nodes(
    runtime: &mut RuntimeScene,
) -> Result<BTreeMap<&'static str, Node>, Box<dyn std::error::Error>> {
    let mut ids = Vec::new();
    for name in NODES {
        ids.push((*name, entity_id(runtime, name)?));
    }
    let read = runtime.with_script(move |script| {
        script.with_world(|world| {
            ids.iter()
                .filter_map(|(name, id)| {
                    let id = (*id)?;
                    let mut node = Node::default();
                    if let Ok(Some(layout)) = world.get::<UiRectLayout>(id) {
                        node.layout = Some(layout.clone());
                    }
                    if let Ok(Some(text)) = world.get::<UiText>(id) {
                        node.text = Some(text.clone());
                    }
                    if let Ok(Some(image)) = world.get::<UiImage>(id) {
                        node.image = Some(image.clone());
                    }
                    if let Ok(Some(events)) = world.get::<UiEvents>(id) {
                        node.events = events
                            .bindings
                            .iter()
                            .map(|binding| {
                                (
                                    binding.event.map_or("any".to_owned(), |event| {
                                        event.as_str().to_owned()
                                    }),
                                    binding.function.clone(),
                                )
                            })
                            .collect();
                    }
                    Some((*name, node))
                })
                .collect::<BTreeMap<_, _>>()
        })
    });
    if read.len() != NODES.len() {
        return Err(format!(
            "17 个门面节点只查到 {} 个（{} 个名字在活世界里查不到）",
            read.len(),
            NODES.len() - read.len()
        )
        .into());
    }
    Ok(read)
}

/// 名字 → 活实体句柄。Lua 侧的 id 编码是 `index << 32 | generation`；查不到时 `entity.find`
/// 返回 nil（这里当作"节点不在"，由调用方决定是红还是记账）。
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

// ---------- ③ 标签两层的位置账 ----------

fn label_pairing(
    runtime: &mut RuntimeScene,
    rects: &BTreeMap<String, UiRect>,
    hidden: &BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (name, button) in LABELS {
        let rect = rects
            .get(*name)
            .ok_or_else(|| format!("{name} 没有解算出矩形（门面标签的实体不见了）"))?;
        let dynamic = lua_str(
            runtime,
            &format!(
                "(function() local t = ui.text_get('title:label:{button}') \
                 if not t then return 'missing' end \
                 if t.visible == false then return 'hidden' end \
                 return string.format('%.2f,%.2f', t.x, t.y) end)()"
            ),
        )?;
        let who = if hidden.contains(*name) {
            "动态层"
        } else {
            "实体层"
        };
        let mut parts = dynamic.split(',');
        let (dx, dy) = (
            parts.next().and_then(|value| value.parse::<f32>().ok()),
            parts.next().and_then(|value| value.parse::<f32>().ok()),
        );
        match (dx, dy) {
            (Some(dx), Some(dy)) => {
                let shift = (dx - rect.x).abs().max((dy - rect.y).abs());
                println!(
                    "  {name}: 实体=({:.2},{:.2}) 动态=({dx:.2},{dy:.2}) 差 {shift:.2}px ⇒ 今天由{who}画",
                    rect.x, rect.y
                );
                if shift > LABEL_ALIGN_EPS {
                    return Err(format!(
                        "{name} 的实体矩形与动态层标签差 {shift:.2}px（>{}px）：两层不是同一坐标口径，\
                         照抄任何一份都会让门面上的字挪位",
                        LABEL_ALIGN_EPS
                    )
                    .into());
                }
            }
            _ => println!(
                "  {name}: 实体=({:.2},{:.2}) 动态={dynamic} ⇒ 今天由{who}画",
                rect.x, rect.y
            ),
        }
    }
    Ok(())
}

// ---------- ④ 层序 ----------

/// `SceneDraw.texture` 走的是**约定下标**：`frame_data.rs` 写入时做过 `+1`，`texture == 0`
/// 是内置白图（`ui.panel` 那种纯色块）。拿 `texture` 直接索引 `paths` 会整体错开一格，
/// 把每一块认成邻居的贴图——这张表因此会读出"层序是乱的"而排序本身没问题（m35 翻过车）。
fn draw_path<'a>(mesh: &'a SceneMesh, draw: &SceneDraw) -> Option<&'a str> {
    if draw.texture == 0 {
        return None;
    }
    mesh.paths
        .get((draw.texture - 1) as usize)
        .map(String::as_str)
}

/// 末段文件名：门面贴图都在 `assets/art/**` 深处，整条路径会把表撑到读不出来。
/// 内置通道（白图 / 两张字形图集）本来就带 `__…__`，末段即全名。
fn tail(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 某个角色（末段名精确匹配）在本帧提交序列里的首次与末次下标；一次都没有则 `None`。
/// 一张图可以有多个分段（三块木牌 = 三个分段，中间隔着别的贴图），所以是**区间**不是点。
///
/// 命中表先收集再取首末：`next()` + `next_back()` 在"只有一个分段"时会被第一个 `next()`
/// 吃掉，于是每张单段贴图都读成"本帧没提交"（第一版就是这么把一张好表读成全红的）。
fn draw_span(mesh: &SceneMesh, name: &str) -> Option<(usize, usize)> {
    let hits: Vec<usize> = mesh
        .draws
        .iter()
        .enumerate()
        .filter(|(_, draw)| {
            if name == WHITE_ROLE {
                // 纯色面板（`ui.panel`）不占贴图槽：既可能是 `texture == 0`，也可能
                // 命中注册表里的 `__white__`，两种写法都算白图分段。
                draw.texture == 0 || draw_path(mesh, draw).is_some_and(|path| path == "__white__")
            } else {
                draw_path(mesh, draw).is_some_and(|path| tail(path) == name)
            }
        })
        .map(|(index, _)| index)
        .collect();
    Some((*hits.first()?, *hits.last()?))
}

/// 白图（纯色块）在读数表里的角色名：它没有文件名，`mesh.draws` 里以 `texture == 0` 出现。
const WHITE_ROLE: &str = "<内置白图>";

/// ④ 的期望序列 = **迁移前实测**的本帧提交序（`--capture` 打印可粘贴的这一行）。
///
/// 这份表是"一像素没动"在层序维度的那一份账：① 的矩形读数只管形状与内容，对"谁盖谁"完全瞎
/// ——一块东西搬进/搬出实体层，矩形与贴图可以一个字节都不变，而从此被别人的贴图盖住。
///
/// 表里这条**读出来的事实**（不是设计意图）：预设那 7 张图（`title_bg_pixel.png`、
/// `ui_menu_vignette.png`、三块木牌 `ui_menu_btn_wuxia_9s.png` 等）整批排在动态层分段**之前**，
/// 也就是门面今天由脚本画的高亮框、hover、四角/分隔/印章全部盖在这张背景之上。
/// 三块木牌 `ui_menu_btn_wuxia_9s.png` 实测 18432/18432 像素 alpha=255（含中心），`title_bg_pixel.png`
/// 是 IHDR 色型 2（无 alpha 通道 ⇒ 逐像素全不透明）——正因为它们不透明，**谁先提交**就是
/// 决定性的：M63b 搬家时这条序是反的（预设整批最后画 ⇒ 背景把脚本的高亮/hover/印章全埋掉，
/// 实测 `title:seal` 区域只剩夜空色），修它=这次把 `push_scene_ui_images` 挪到 `push_ui_*` 之前。
const ORDER_GOLDEN: &[&str] = &[
    "title_bg_pixel.png",
    "ui_menu_vignette.png",
    "ui_menu_ink_band.png",
    "ui_title_logo.png",
    "ui_menu_lantern.png",
    "ui_menu_btn_wuxia_9s.png",
    "ui_menu_panel_scroll_9s.png",
    WHITE_ROLE,
    "ui_menu_btn_wuxia_hover_9s.png",
    "ui_menu_divider_bamboo.png",
    "ui_menu_ornament_corner_bl.png",
    "ui_menu_ornament_corner_br.png",
    "ui_menu_ornament_corner.png",
    "ui_menu_ornament_corner_tr.png",
    "ui_menu_seal_jianghu.png",
];

/// 门面上要点名的角色（末段名），逐个报它在提交序列里的区间。
const ORDER_ROLES: &[&str] = &[
    WHITE_ROLE,
    "ui_menu_btn_wuxia_hover_9s.png",
    "ui_menu_divider_bamboo.png",
    "ui_menu_ornament_corner.png",
    "ui_menu_seal_jianghu.png",
    "title_bg_pixel.png",
    "ui_menu_vignette.png",
    "ui_title_logo.png",
    "ui_menu_lantern.png",
    "ui_menu_btn_wuxia_9s.png",
    "ui_menu_panel_scroll_9s.png",
];

/// 门面上的层序账。**下标即先后**（UI 层不写深度，后提交的盖住先提交的）。
fn layer_order(
    runtime: &mut RuntimeScene,
    capture: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W as u32, VIEW_H as u32)?;
    let actual: Vec<String> = mesh
        .draws
        .iter()
        .map(|draw| tail(draw_path(&mesh, draw).unwrap_or(WHITE_ROLE)).to_owned())
        .collect();
    println!("  本帧提交 {} 个分段（先后即画家顺序）：", mesh.draws.len());
    for (index, draw) in mesh.draws.iter().enumerate() {
        println!(
            "  {index:>3}  {:<34} 顶点 {}..{}",
            actual[index],
            draw.start,
            draw.start + draw.count
        );
    }
    for name in ORDER_ROLES {
        match draw_span(&mesh, name) {
            Some((first, last)) => println!("  {name:<34} 分段 {first}..{last}"),
            None => println!("  {name:<34} 本帧没提交"),
        }
    }
    // 诚实标注这条通道的口径：headless 顶点路径这一帧没产出任何文字分段（`mesh.draws` 里
    // 一张字形图集都没有），所以 ④ 只管图块与纯色块的层序；文字层的先后是代码事实
    // （`frame_data.rs`：预设图趟在 `push_ui_*` 之前、预设文字趟在 `push_ui_text` 之后，
    // 后者受字体图集约束不能前挪），由 §10 写明，不在这道门里。
    println!("  文字分段：无（这条读数通道这一帧不出文字 ⇒ ④ 只钉图块/色块的层序）");
    if capture {
        println!(
            "  可粘贴的期望序列：{}",
            actual
                .iter()
                .map(|name| format!("\"{name}\",\n"))
                .collect::<String>()
                .trim_end()
        );
        return Ok(());
    }
    let expected: Vec<&str> = ORDER_GOLDEN.to_vec();
    let got: Vec<&str> = actual.iter().map(String::as_str).collect();
    if got != expected {
        let mut diff = Vec::new();
        for (index, pair) in expected.iter().zip(got.iter()).enumerate() {
            if pair.0 != pair.1 {
                diff.push(format!("{index}: 期望 {}、实测 {}", pair.0, pair.1));
            }
        }
        if expected.len() != got.len() {
            diff.push(format!(
                "分段数 期望 {}、实测 {}（多/少出来的那几块不在表里）",
                expected.len(),
                got.len()
            ));
        }
        return Err(format!(
            "层序变了：\n  {}\n层序不在①的矩形读数里，所以这是唯一能证明「搬进预设的东西没被谁盖住、\n  也没盖住谁」的账（见本段注释与 docs/66 §10）。",
            diff.join("\n  ")
        )
        .into());
    }
    println!("  层序与迁移前实测一致（{} 个分段）", got.len());
    // 第二轮：**同一个世界里**撤掉再重铺。`entity.spawn()` 优先复用回收槽（LIFO），
    // 而实体层的绘制先后 = 槽位升序，所以"撤了重铺"完全可能把这一批节点铺成倒序——
    // 症状是那张 1280×720 无 alpha 的背景图排到最后、整块门面被盖成一张背景。
    // 脚本用"一个世界里只 spawn 一次"（`M.reset()` 幂等 + `M.hide()` 只在离开门面时撤）
    // 避开这条路；这一轮就是钉住"真走这条路会发生什么"，别把它当没事。
    let resawn = respawn_in_same_world(runtime)?;
    let after: Vec<&str> = resawn.iter().map(String::as_str).collect();
    if after != expected {
        let diff: Vec<String> = expected
            .iter()
            .zip(after.iter())
            .enumerate()
            .filter(|(_, (want, now))| want != now)
            .map(|(index, (want, now))| format!("{index}: 期望 {want}、重铺后 {now}"))
            .collect();
        println!(
            "  ⚠ 同一世界撤了再铺，层序**翻了**（{} 处不一致）：\n  {}",
            diff.len(),
            diff.join("\n  ")
        );
        println!(
            "  ⇒ 结论：预设实例**不能**在同一个世界里撤了重铺（脚本因此 spawn-once）。\
             这条不是门的失败，是它量到的事实。"
        );
    } else {
        println!("  同一世界撤了再铺：层序不变（回收槽这次没翻顺序）");
    }
    Ok(())
}

/// 在**当前世界**里把门面预设撤掉再重铺（走脚本自己的 `hide()`/`reset()`，不碰引擎），
/// 然后重读本帧提交序。返回与 `layer_order` 同形状的末段名序列。
fn respawn_in_same_world(
    runtime: &mut RuntimeScene,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    runtime.with_script(|script| {
        script.execute_console("local m = require('ui.title_menu') m.hide() m.reset() return true")
    })?;
    frame(runtime)?;
    let mesh = runtime.vertices(VIEW_W as u32, VIEW_H as u32)?;
    Ok(mesh
        .draws
        .iter()
        .map(|draw| tail(draw_path(&mesh, draw).unwrap_or(WHITE_ROLE)).to_owned())
        .collect())
}

// ---------- 驱动与格式化 ----------

/// 推进一帧：先走真实渲染路径（视口与 UI 布局解算只在那条路径上发生），再跑一个固定步。
fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W as u32, VIEW_H as u32)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
}

fn kind_tag(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "text",
        Kind::Image => "image",
        Kind::Panel => "panel",
    }
}

fn xy(rect: &UiRect) -> String {
    format!("{:.2},{:.2}", rect.x, rect.y)
}

fn xywh(rect: &UiRect) -> String {
    format!("{:.2},{:.2},{:.2},{:.2}", rect.x, rect.y, rect.w, rect.h)
}

fn rgb(color: &[f32; 3]) -> String {
    format!("{:.3},{:.3},{:.3}", color[0], color[1], color[2])
}

/// `Image` 的着色读数（RGBA）。与 `UiImage::to_component_str` 同一条口径：alpha=1 只印三段
/// ⇒ 既有黄金表逐字节不动；谁真给门面写半透明，第四段就进表，画面变了门就红
/// （`Text` 没有 alpha，走 [`rgb`]）。
fn tint(color: &[f32; 4]) -> String {
    if color[3] == 1.0 {
        return rgb(&[color[0], color[1], color[2]]);
    }
    format!(
        "{:.3},{:.3},{:.3},{:.3}",
        color[0], color[1], color[2], color[3]
    )
}

/// 九宫格边进黄金表：它是"这块板子怎么拉伸"的版面数据，丢了不会改矩形、只会改画面。
fn nine_slice(slice: &Option<[f32; 4]>) -> String {
    slice.map_or("-".to_owned(), |[left, right, top, bottom]| {
        format!("{left},{right},{top},{bottom}")
    })
}

/// 与 Lua `%g` 同口径（整数值不带小数点），两边才可直接比。
fn g(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{:.0}", value)
    } else {
        format!("{value}")
    }
}

fn resolve_project(args: &[String]) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let project = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            rustlua_infra::games_dir()
                .join("examples")
                .join("game_jhyt")
        });
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

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
