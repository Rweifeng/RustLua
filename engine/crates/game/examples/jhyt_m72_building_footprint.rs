//! Headless acceptance drive for M72-07：**多格建筑的占地、门洞与拆层不变量**。
//!
//! 这一包要消灭的症状是"看得见的是墙脚、看不见的是空气"：建筑贴图是 224×224 的一大张，
//! 而 quad 是矩形、屋顶是梯形、檐口是斜的——**按框标实心**就会在屋外两步远的地方撞空气，
//! **按感觉手填**（旧版每件手填一串占地格）则一边漏一边多。所以本包定的口径是：
//!
//! > 每一格 `solid` 都必须有可视依据——那一格的地表本身是水/墙（瓦自己画的），
//! > 或者有一个 sprite 的**不透明像素**压在上面；建筑还要把**同名拆层屋顶件**并进并集，
//! > 再挖掉**门洞**。（2026-10-09 裁定：本体∪屋顶并集实心，屋顶那几格反过来也必须实心。）
//!
//! `tools/building_kit.py` 是这条规则的实现，`python tools/building_kit.py scenes/manor.scene
//! --check` 已经在 verify 的 Python 段里跑。本门是它的**第二份独立实现**：门不 import Python，
//! 自己解 `.scene` 文本、自己解 PNG 的 alpha 通道、自己算紧包围盒。两份算法都说"缺 0 悬空 0"
//! 才算成立——只有一份实现时，"对拍"对的是它自己。
//!
//! 章节：
//!   ① 每格实心都有可视依据：门复算的应实心格集必须与盘上 `solid` 串**逐格相同**
//!      （缺=0、悬空=0），并报出每栋的视觉格数/实心格数/门洞/拆层件。
//!      两条反面对照走同一个函数：把一件道具的占地从"单格根碰撞"换成**整个 quad 框**
//!      必须报出悬空格并点名（这条落在东南那丛竹子上——盘上 10 栋建筑的框与不透明紧包围盒
//!      逐格相同，贴图都铺满了画框，拿房子做这条对照是空转）；把同名拆层屋顶件从
//!      并集里摘掉必须报出缺碰撞并点名屋檐格。
//!   ② 拆层不变量：每栋建筑占地必须跨多格（≥4），且**拆层前后占地逐格相同**——
//!      用整图（未拆层）算的格集必须等于"本体∪屋顶"算的格集。这条是拆层这件事
//!      没有偷偷改变玩法的唯一证据：画面换了、碰撞没换。
//!   ③ 门洞：声明的洞格必须**不在 solid 里**、贴图在那一格上**真是洞**（不透明占比
//!      ≤ `HOLE_MAX_OPAQUE_RATIO`），并且从出生点按 4 邻域 BFS **走得到**。
//!      反面对照：把洞填成实心，可达性必须当场断掉。
//!   ④ 投影贴片：每栋有体量的建筑都必须挂一件 `<名字>Shadow`（`role=cast_shadow`），
//!      且画在本体之下；不够格的（`BuildFoundation` 是平地基）必须**没有**——只查前者
//!      等于允许"给每样东西都糊一块灰"过关。
//!   ⑤ 运行期对拍：引擎读回来的 `tilemap.is_solid` 位图必须与盘上 `solid` 串逐格相同。
//!      前四节全在查文件，这一节查"引擎认的是同一张表"——`solid` 同时是物理世界
//!      （`scene_spawn.rs` 的 `solid_cell_colliders` 每个实心格挂一个静态 cuboid）。
//!
//! 用法：
//!   cargo run --quiet -p game --example jhyt_m72_building_footprint [-- <game_jhyt-root>]

use engine::{
    core::input::InputSnapshot,
    host::RuntimeScene,
};
use image::RgbaImage;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    path::{Path, PathBuf},
};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;

const SCENE: &str = "scenes/manor.scene";
const GROUND_TEXTURE: &str = "assets/art/tilesets/manor/manor_ground.png";
const TILE: f64 = 32.0;

/// 地表 kind（与 `terrain_ramp.json`、`gen_manor_scene.py` 的列映射同序号）。
const WALL_KIND: i32 = 4;
const WATER_KIND: i32 = 6;

/// 与 `building_kit.BUILDING_ROLES` / `ROOT_ROLES` 同集合。
const BUILDING_ROLES: [&str; 2] = ["building", "building_site"];
const ROOT_ROLE: &str = "prop";
const SHADOW_ROLE: &str = "cast_shadow";
const OCCLUDER_LAYER: &str = "foreground";

/// 门洞（`building_kit.OPENINGS` 的手抄副本）：实体名 + 绝对 cell。
/// 只有南门有洞——它是 `data/maps.lua` 山庄→云泽村出口格 (16,1) 所在。
const OPENINGS: &[(&str, i32, i32)] = &[("ManorGate", 16, 1), ("ManorGate", 16, 2)];
/// 声明为洞的格上贴图不透明占比的上限。口径同 `building_kit`：**只查每列最低那一格**
/// （人站着穿过的那格），它上面那格是屋檐/匾额，本来就该画满。
const HOLE_MAX_OPAQUE_RATIO: f64 = 0.35;
/// 出生点 cell（`data/maps.lua` 的 main.spawn，与 `gen_manor_scene.SPAWN` 同源）。
const SPAWN_CELL: (i32, i32) = (16, 3);

/// 一栋建筑"算多格"的下界。盘上最小的读数 ManorGate = 7 格，4 已经能挡住
/// "整栋只标锚点那一格"的旧写法，又不会把将来真要做的小亭子判死。
const MIN_BUILDING_CELLS: usize = 4;

/// 横着占两格的道具（`building_kit.PROP_COLLISION` 的手抄副本）：只标锚点会让另一半
/// "看得见却穿得过"。
const BOX_PROPS: [&str; 2] = ["ManorFence", "ManorRack"];

/// 拆层表（`gen_manor_art.LAYER_SPLITS` 的手抄副本）：`(本体实体, 遮断件实体, 整图)`。
/// 整图路径是 §② 的唯一输入——没有它就没法问"拆层之前那一栋占的是哪几格"。
struct Split {
    base: &'static str,
    occluder: &'static str,
    full: &'static str,
}

const SPLITS: &[Split] = &[
    Split {
        base: "ManorHouse",
        occluder: "ManorHouseRoof",
        full: "assets/art/buildings/manor_house.png",
    },
    Split {
        base: "ManorShed",
        occluder: "ManorShedRoof",
        full: "assets/art/buildings/manor_shed.png",
    },
    Split {
        base: "BuildTrainingHall",
        occluder: "BuildTrainingHallRoof",
        full: "assets/art/buildings/build_training_hall.png",
    },
    Split {
        base: "BuildStorehouse",
        occluder: "BuildStorehouseRoof",
        full: "assets/art/buildings/build_storehouse.png",
    },
    Split {
        base: "BuildHerbHut",
        occluder: "BuildHerbHutRoof",
        full: "assets/art/buildings/build_herb_hut.png",
    },
    Split {
        base: "BuildTeaShed",
        occluder: "BuildTeaShedRoof",
        full: "assets/art/buildings/build_tea_shed.png",
    },
    Split {
        base: "BuildForgeStand",
        occluder: "BuildForgeStandRoof",
        full: "assets/art/buildings/build_forge_stand.png",
    },
];

/// 场景里一个 sprite 实体的门侧视图。
#[derive(Clone, Debug)]
struct Placed {
    name: String,
    role: String,
    cell: Option<(i32, i32)>,
    texture: String,
    layer: String,
    order: i32,
    x: f64,
    y: f64,
    width: u32,
    height: u32,
}

/// 占地口径的可调项。**真实断言与两条反面对照走同一个函数**——各写一遍的话，
/// 对照失手（改了口径却没改对照）就没人发现了。
#[derive(Clone, Copy)]
struct Rule {
    /// 把这一件的占地换成整个 quad 框（反面对照 A：按视觉外框标实心）。`None` = 正常口径。
    quad_box: Option<&'static str>,
    /// 不把同名拆层件并进并集（反面对照 B：拆层等于丢碰撞）。
    ignore_mates: bool,
}

const RULE_REAL: Rule = Rule {
    quad_box: None,
    ignore_mates: false,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let sprites = read_scene(&project.join(SCENE))?;
    let ground = read_ground(&project.join(SCENE))?;

    banner("① 每格实心都有可视依据：门复算 vs 盘上 solid 串");
    let (want, readings) = footprint(&project, &sprites, &ground, RULE_REAL)?;
    for reading in &readings {
        println!(
            "   {:<20} 视觉 {:>3} 格 实心 {:>3} 格 门洞 {:<10}{}",
            reading.name,
            reading.visual,
            reading.solid,
            if reading.openings.is_empty() {
                "-".to_owned()
            } else {
                reading
                    .openings
                    .iter()
                    .map(|(x, y)| format!("({x},{y})"))
                    .collect::<Vec<_>>()
                    .join(" ")
            },
            if reading.mates.is_empty() {
                String::new()
            } else {
                format!("（拆层件：{}）", reading.mates.join(","))
            }
        );
    }
    let native = ground.native();
    let disk = ground.solid_cells();
    println!(
        "   地表天然实心（墙/水）{} 格 + sprite 依据 {} 格 ⇒ 应实心 {} 格；盘上 {} 格",
        native.len(),
        want.len(),
        want.union(&native).count(),
        disk.len()
    );
    report_diff(&disk, &want, &native)?;

    // 一条读数，决定下面那条对照该落在谁身上：**quad 框与不透明紧包围盒不同的建筑有几栋**。
    // 盘上实测是 0——这几张建筑贴图都铺满了自己的画框，对它们来说"按框"与"按像素"同解。
    let loose: Vec<&str> = sprites
        .iter()
        .filter(|item| {
            BUILDING_ROLES.contains(&item.role.as_str()) && item.layer != OCCLUDER_LAYER
        })
        .map(|item| Ok((item.name.as_str(), quad_cells(item) != cells_of(&project, item)?)))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?
        .into_iter()
        .filter(|(_, differs)| *differs)
        .map(|(name, _)| name)
        .collect();
    println!(
        "   按 quad 框与按不透明像素会算出不同占地的建筑：{} 栋{:>}",
        loose.len(),
        if loose.is_empty() {
            "（贴图都铺满了画框）".to_owned()
        } else {
            format!(" -> {loose:?}")
        }
    );

    // 反面对照 A：**按视觉外框标实心**——本包消灭的正是这个口径。它落在东南那丛竹子上：
    // 竹按"单格根碰撞"只标根那一格，按框标就把竹梢那几格也标成墙，而竹梢是 M72-06
    // 的遮断件（画在前景、玩家本来就该从竹子底下走过去）。上一条读数说明为什么不能拿
    // 房子做这条对照：房子的框与像素同解，照框标不会多出任何格，对照就成了空转。
    let (boxed, _) = footprint(
        &project,
        &sprites,
        &ground,
        Rule {
            quad_box: Some("ManorBambooSoutheast"),
            ignore_mates: false,
        },
    )?;
    let dangling: Vec<(i32, i32)> = boxed
        .union(&native)
        .filter(|cell| !disk.contains(cell))
        .cloned()
        .collect();
    if dangling.is_empty() {
        return Err(
            "反面对照失手：把竹子的占地从单格根换成整个 quad 框后仍然报不出悬空格——\
             说明 §① 的比对根本没在逐格看".into(),
        );
    }
    println!(
        "  反面对照 A：竹子按 quad 框算 → 悬空 {} 格，首格 {:?}（多出来那几格画的是\
         能走过去的竹梢，按规则不该实心）",
        dangling.len(),
        dangling[0]
    );
    // 反面对照 B：把同名拆层屋顶件从并集里摘掉 ⇒ 屋檐那几格必须判成缺碰撞。
    let (body_only, _) = footprint(
        &project,
        &sprites,
        &ground,
        Rule {
            quad_box: None,
            ignore_mates: true,
        },
    )?;
    let lost: Vec<(i32, i32)> = disk
        .difference(&body_only.union(&native).cloned().collect())
        .cloned()
        .collect();
    if lost.is_empty() {
        return Err(
            "反面对照失手：摘掉拆层屋顶件之后仍然报不出缺碰撞格——屋顶并进并集这条\
             其实没在参与计算".into(),
        );
    }
    println!(
        "  反面对照 B：只按本体算占地 → 缺碰撞 {} 格，首格 {:?}（屋檐会被人走进屋里）",
        lost.len(),
        lost[0]
    );

    banner("② 拆层不变量：每栋跨多格，且拆层前后占地逐格相同");
    let mut split_checked = 0usize;
    for split in SPLITS {
        let base = find(&sprites, split.base)?;
        // 拆层件必须真的在场景里：并集口径下它决定屋檐那几格实不实心，
        // 少了那一件，§① 的"缺碰撞"会红成一整片屋檐而不是这一句点名。
        find(&sprites, split.occluder)?;
        let occluder = find(&sprites, split.occluder)?;
        // 拆层**后**的占地 = 本体 ∪ 屋顶，也就是 §① 采用的那个口径。
        let mut after = cells_of(&project, base)?;
        after.extend(cells_of(&project, occluder)?);
        if after.len() < MIN_BUILDING_CELLS {
            return Err(format!(
                "{} 只占 {} 格，应 ≥{MIN_BUILDING_CELLS}：多格建筑退回了单格根碰撞的老写法",
                split.base,
                after.len()
            )
            .into());
        }
        // 拆层前 = 整图摆在**本体的左下角**（按行切图不改水平位置，也不改底边）。
        let full = Placed {
            name: split.base.to_owned(),
            role: base.role.clone(),
            cell: base.cell,
            texture: split.full.to_owned(),
            layer: base.layer.clone(),
            order: base.order,
            x: base.x,
            y: base.y,
            width: 0,
            height: 0,
        };
        let before = cells_of(&project, &full)?;
        if before != after {
            let only_before: Vec<(i32, i32)> = before.difference(&after).take(4).copied().collect();
            let only_after: Vec<(i32, i32)> = after.difference(&before).take(4).copied().collect();
            return Err(format!(
                "{} 拆层前后占地不同：拆前 {} 格 / 拆后 {} 格，只在拆前的 {only_before:?}、\
                 只在拆后的 {only_after:?}。拆层只许换画面，不许换碰撞",
                split.base,
                before.len(),
                after.len(),
            )
            .into());
        }
        split_checked += 1;
    }
    println!(
        "  {} 栋拆层建筑：占地全部 ≥{MIN_BUILDING_CELLS} 格，且\"整图算的格集 == 本体∪屋顶\"\
         （逐格相同，最大一栋 {} 格）",
        split_checked,
        readings.iter().map(|item| item.visual).max().unwrap_or(0)
    );

    banner("③ 门洞：可站、图上真是洞、从出生点走得到");
    let mut walkable = disk.clone();
    for (owner, cx, cy) in OPENINGS {
        // 洞格必须是**因为规则才被挖掉**的，不是碰巧没标上：先确认它在自己的视觉框里
        // （洞挪了位置而表没跟着改 = 声明与图分叉），再确认盘上那一格真的是空的。
        let cells = cells_of(&project, find(&sprites, owner)?)?;
        if !cells.contains(&(*cx, *cy)) {
            return Err(format!(
                "{owner} 声明的门洞 ({cx},{cy}) 不在它的视觉框里：洞挪了位置而表没跟着改"
            )
            .into());
        }
        if disk.contains(&(*cx, *cy)) {
            return Err(format!(
                "门洞 ({cx},{cy}) 在盘上仍是实心格：{owner} 的洞没被挖开，南门是一堵墙"
            )
            .into());
        }
        walkable.remove(&(*cx, *cy));
        println!("   ({cx},{cy}) 在 {owner} 的视觉框内、盘上已挖开");
    }
    // 图上真是洞：只查每列最低那一格（人穿过去的那格）。
    let mut lowest: BTreeMap<i32, i32> = BTreeMap::new();
    for (_, cx, cy) in OPENINGS {
        let entry = lowest.entry(*cx).or_insert(*cy);
        if *cy < *entry {
            *entry = *cy;
        }
    }
    for (owner, cx, cy) in OPENINGS {
        if lowest.get(cx) != Some(cy) {
            continue; // 同一列的上面那格是屋檐/匾额，本来就该画满
        }
        let item = find(&sprites, owner)?;
        let ratio = hole_opacity(&project, item, (*cx, *cy))?;
        if ratio > HOLE_MAX_OPAQUE_RATIO {
            return Err(format!(
                "{owner} 门洞 ({cx},{cy}) 的不透明占比 {ratio:.2} > {HOLE_MAX_OPAQUE_RATIO}：\
                 声明是人穿过去的通道，图上却画满了"
            )
            .into());
        }
        println!("   ({cx},{cy}) 贴图实测不透明占比 {ratio:.2} ≤ {HOLE_MAX_OPAQUE_RATIO}");
    }
    let reached = reachable_from(&ground, &walkable, SPAWN_CELL);
    for (_, cx, cy) in OPENINGS {
        if !reached.contains(&(*cx, *cy)) {
            return Err(format!(
                "门洞 ({cx},{cy}) 从出生点 {SPAWN_CELL:?} 走不到：门是死的，\
                 山庄↔云泽村的出口格就废在这里"
            )
            .into());
        }
    }
    println!(
        "  4 邻域 BFS（只走非实心格）从 {SPAWN_CELL:?} 到得了全部 {} 个洞格，可达 {} 格",
        OPENINGS.len(),
        reached.len()
    );
    // 反面对照：把洞填成实心 ⇒ 出口必须断。
    let mut sealed = walkable.clone();
    for (_, cx, cy) in OPENINGS {
        sealed.insert((*cx, *cy));
    }
    let sealed_reach = reachable_from(&ground, &sealed, SPAWN_CELL);
    if OPENINGS
        .iter()
        .any(|(_, cx, cy)| sealed_reach.contains(&(*cx, *cy)))
    {
        return Err(
            "反面对照失手：把门洞填成实心之后洞格仍然可达——§③ 的可达性其实没在依赖 solid".into(),
        );
    }
    println!("  反面对照：把洞格填成实心 → 出口当场断掉（可达集从 {} 缩到 {}）",
             reached.len(), sealed_reach.len());

    banner("④ 投影贴片：够格的每栋都有、不够格的都没有");
    let mut shadows = 0usize;
    for item in sprites
        .iter()
        .filter(|item| BUILDING_ROLES.contains(&item.role.as_str()))
    {
        let want_shadow = format!("{}Shadow", item.name);
        let found = sprites.iter().find(|other| other.name == want_shadow);
        let is_flat = item.name == "BuildFoundation"; // 平地基没有体量，不该糊一块灰
        match found {
            Some(patch) => {
                if is_flat {
                    return Err(format!(
                        "{want_shadow} 存在，但 {} 是平地基（没有高度）：给它投影等于承认\
                         \"给每样东西都糊一块灰\"是允许的",
                        item.name
                    )
                    .into());
                }
                if patch.role != SHADOW_ROLE {
                    return Err(format!("{want_shadow} 的 role 是 {:?}，应为 {SHADOW_ROLE:?}",
                                       patch.role)
                    .into());
                }
                if patch.order >= item.order || patch.layer != "world" {
                    return Err(format!(
                        "{want_shadow} 画在本体上面（layer={:?} order={} vs 本体 order={}）：\
                         投影会浮在墙上",
                        patch.layer, patch.order, item.order
                    )
                    .into());
                }
                shadows += 1;
            }
            None => {
                if !is_flat {
                    return Err(format!(
                        "{} 没有 {want_shadow}：本包要求每栋有体量的建筑都挂地面投影\
                         （平地基除外，它没有体量）",
                        item.name
                    )
                    .into());
                }
                println!("   BuildFoundation 无投影（平地基，够格门槛之外）");
            }
        }
    }
    let total_patches = sprites
        .iter()
        .filter(|item| item.role == SHADOW_ROLE)
        .count();
    println!("  {shadows} 栋建筑各有且仅有一件贴片；全图贴片共 {total_patches} 件（含树/竹/脚手架）");

    banner("⑤ 运行期对拍：引擎读回的 is_solid 位图 == 盘上 solid 串");
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "main")?;
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    assert_scene(&mut runtime, "manor")?;
    let live = ground_solid(&mut runtime)?;
    if live.cols != ground.width || live.rows != ground.height {
        return Err(format!(
            "运行期位图是 {}x{}，文件里写的是 {}x{}",
            live.cols, live.rows, ground.width, ground.height
        )
        .into());
    }
    let mut live_cells = BTreeSet::new();
    for row in 0..live.rows {
        for col in 0..live.cols {
            if live.cells[row * live.cols + col] {
                live_cells.insert((col as i32, row as i32));
            }
        }
    }
    if live_cells != disk {
        let only_file: Vec<(i32, i32)> = disk.difference(&live_cells).take(6).copied().collect();
        let only_live: Vec<(i32, i32)> = live_cells.difference(&disk).take(6).copied().collect();
        return Err(format!(
            "引擎认的实心格与文件不同：只在文件里 {only_file:?}、只在运行期 {only_live:?}。\
             solid 是物理事实源，这一条分叉意味着玩家撞的墙和门算的墙不是同一批"
        )
        .into());
    }
    println!(
        "  {}x{} 位图逐格相同，实心 {} 格（每格一个静态 cuboid，见 solid_cell_colliders）",
        live.cols,
        live.rows,
        live_cells.len()
    );

    println!(
        "\njhyt_m72_building_footprint_ok project={} solid_cells={} buildings={} splits={} shadows={}",
        project.display(),
        disk.len(),
        readings.len(),
        split_checked,
        total_patches
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// 占地复算（`building_kit.visual_and_solid` 的第二份实现）
// ---------------------------------------------------------------------------

struct Reading {
    name: String,
    visual: usize,
    solid: usize,
    openings: Vec<(i32, i32)>,
    mates: Vec<String>,
}

fn footprint(
    project: &Path,
    sprites: &[Placed],
    ground: &Ground,
    rule: Rule,
) -> Result<(BTreeSet<(i32, i32)>, Vec<Reading>), Box<dyn std::error::Error>> {
    let mut solid = BTreeSet::new();
    let mut readings = Vec::new();
    for item in sprites.iter().filter(|item| BUILDING_ROLES.contains(&item.role.as_str())) {
        if item.layer == OCCLUDER_LAYER {
            continue; // 屋顶那一件在下面按名字配对
        }
        let mut cells = if rule.quad_box == Some(item.name.as_str()) {
            quad_cells(item)
        } else {
            cells_of(project, item)?
        };
        let mut mates = Vec::new();
        if !rule.ignore_mates {
            // 配对规则是**实体名前缀**，不按 role 筛：屋顶那件的 role 是 `occluder`，
            // 按 role 筛会把它漏掉，那几格屋檐就被判成"悬空碰撞"而删掉实心。
            for other in sprites.iter().filter(|other| {
                other.name != item.name
                    && other.role != "decor"
                    && other.role != SHADOW_ROLE
                    && other.name.starts_with(item.name.as_str())
            }) {
                cells.extend(if rule.quad_box == Some(other.name.as_str()) {
                    quad_cells(other)
                } else {
                    cells_of(project, other)?
                });
                mates.push(other.name.clone());
            }
        }
        let openings: Vec<(i32, i32)> = OPENINGS
            .iter()
            .filter(|(owner, _, _)| *owner == item.name.as_str())
            .map(|(_, cx, cy)| (*cx, *cy))
            .collect();
        let unknown: Vec<(i32, i32)> = openings
            .iter()
            .filter(|cell| !cells.contains(cell))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "{} 声明的门洞 {unknown:?} 不在它的视觉框里：洞挪了位置而表没跟着改",
                item.name
            )
            .into());
        }
        for cell in &openings {
            cells.remove(cell);
        }
        readings.push(Reading {
            name: item.name.clone(),
            visual: cells.len() + openings.len(),
            solid: cells.len(),
            openings,
            mates,
        });
        solid.extend(cells);
    }
    for item in sprites.iter().filter(|item| item.role == ROOT_ROLE) {
        // 反面对照 A 的口径：把这一件从"单格根碰撞"换成整个 quad 框。
        if BOX_PROPS.contains(&item.name.as_str()) || rule.quad_box == Some(item.name.as_str()) {
            solid.extend(if rule.quad_box == Some(item.name.as_str()) {
                quad_cells(item)
            } else {
                cells_of(project, item)?
            });
            continue;
        }
        let cell = item.cell.ok_or_else(|| format!(
            "{} 是 prop 却没有 entity_meta cell：单格根碰撞没有锚点可标",
            item.name
        ))?;
        solid.insert(cell);
    }
    Ok((
        solid
            .into_iter()
            .filter(|(x, y)| *x >= 0 && *y >= 0 && (*x as usize) < ground.width && (*y as usize) < ground.height)
            .collect(),
        readings,
    ))
}

/// 不透明像素的紧包围盒换算成占地格集（本包的口径）。
fn cells_of(project: &Path, item: &Placed) -> Result<BTreeSet<(i32, i32)>, Box<dyn std::error::Error>> {
    let image = image_of(project, &item.texture)?;
    let (w, h) = (image.width() as i64, image.height() as i64);
    let mut box_x0 = w;
    let mut box_x1 = -1i64;
    let mut box_y0 = h;
    let mut box_y1 = -1i64;
    for (index, pixel) in image.pixels().enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let x = (index % w as usize) as i64;
        let y = (index / w as usize) as i64;
        box_x0 = box_x0.min(x);
        box_x1 = box_x1.max(x + 1);
        box_y0 = box_y0.min(y);
        box_y1 = box_y1.max(y + 1);
    }
    if box_x1 < 0 {
        return Err(format!("{} 整张全透明：它不该被摆进场景", item.texture).into());
    }
    // 图像 y 向下、世界 y 向上：图像顶行 box_y0 对应世界 y = item.y + (h - box_y0)
    let rect = (
        item.x + box_x0 as f64,
        item.y + (h - box_y1) as f64,
        item.x + box_x1 as f64,
        item.y + (h - box_y0) as f64,
    );
    Ok(cells_covered(rect))
}

/// 整个 quad 框覆盖的格（反面对照 A 的口径：视觉多大不看、只看贴图矩形多大）。
fn quad_cells(item: &Placed) -> BTreeSet<(i32, i32)> {
    cells_covered((
        item.x,
        item.y,
        item.x + item.width as f64,
        item.y + item.height as f64,
    ))
}

fn cells_covered(rect: (f64, f64, f64, f64)) -> BTreeSet<(i32, i32)> {
    let (x0, y0, x1, y1) = rect;
    let mut out = BTreeSet::new();
    let mut cx = (x0 / TILE).floor() as i32;
    while f64::from(cx) * TILE < x1 {
        let mut cy = (y0 / TILE).floor() as i32;
        while f64::from(cy) * TILE < y1 {
            out.insert((cx, cy));
            cy += 1;
        }
        cx += 1;
    }
    out
}

/// 门洞那一格在贴图上有多少不透明像素（0.0 = 全空 = 真是洞）。
fn hole_opacity(
    project: &Path,
    item: &Placed,
    cell: (i32, i32),
) -> Result<f64, Box<dyn std::error::Error>> {
    let image = image_of(project, &item.texture)?;
    let (w, h) = (image.width() as i64, image.height() as i64);
    let wx0 = f64::from(cell.0) * TILE;
    let wy0 = f64::from(cell.1) * TILE;
    let ix0 = ((wx0 - item.x) / 1.0).max(0.0) as i64;
    let ix1 = ((wx0 + TILE - item.x) as i64).min(w);
    // 世界 y 向上、图像行 0 在顶：行 r 覆盖世界 `y+h-r-1 .. y+h-r`
    // ⇒ 格 `[wy0, wy0+TILE]` 对应行区间 `[y+h-wy0-TILE, y+h-wy0)`。
    // 与 `building_kit.opening_holes` 同一条式子（两边曾都写作 `- item.y`，
    // 只在 `y==0` 时碰巧对——南门正贴在地图下边界上，所以盘上没人撞过它）。
    let iy0 = (item.y as i64 + h - (wy0 + TILE) as i64).max(0);
    let iy1 = (item.y as i64 + h - wy0 as i64).min(h);
    if ix1 <= ix0 || iy1 <= iy0 {
        return Err(format!(
            "门洞 {cell:?} 落在 {} 的贴图矩形之外：格与图对不上，占比无从量起",
            item.name
        )
        .into());
    }
    let mut opaque = 0u64;
    for yy in iy0..iy1 {
        for xx in ix0..ix1 {
            if image.get_pixel(xx as u32, yy as u32)[3] > 0 {
                opaque += 1;
            }
        }
    }
    Ok(opaque as f64 / ((ix1 - ix0) * (iy1 - iy0)) as f64)
}

/// 4 邻域 BFS：只走非实心格。返回可达格集（含起点）。
fn reachable_from(
    ground: &Ground,
    blocked: &BTreeSet<(i32, i32)>,
    start: (i32, i32),
) -> BTreeSet<(i32, i32)> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    if blocked.contains(&start) {
        return seen;
    }
    seen.insert(start);
    queue.push_back(start);
    while let Some((x, y)) = queue.pop_front() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = (x + dx, y + dy);
            if next.0 < 0 || next.1 < 0
                || next.0 as usize >= ground.width
                || next.1 as usize >= ground.height
                || seen.contains(&next)
                || blocked.contains(&next)
            {
                continue;
            }
            seen.insert(next);
            queue.push_back(next);
        }
    }
    seen
}

fn report_diff(
    disk: &BTreeSet<(i32, i32)>,
    want: &BTreeSet<(i32, i32)>,
    native: &BTreeSet<(i32, i32)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let all_want = want.union(native).cloned().collect::<BTreeSet<_>>();
    let missing: Vec<(i32, i32)> = all_want.difference(disk).take(6).copied().collect();
    let orphan: Vec<(i32, i32)> = disk.difference(&all_want).take(6).copied().collect();
    if missing.is_empty() && orphan.is_empty() {
        println!("   缺碰撞 0 格、悬空碰撞 0 格：每一格实心都指得出那张图上的那团像素");
        return Ok(());
    }
    let mut why = String::new();
    if !missing.is_empty() {
        why.push_str(&format!(
            "缺碰撞 {} 格（看得见墙却走得进去），例如 {:?}；",
            all_want.difference(disk).count(),
            missing
        ));
    }
    if !orphan.is_empty() {
        why.push_str(&format!(
            "悬空碰撞 {} 格（看不见东西却撞得住），例如 {:?}；",
            disk.difference(&all_want).count(),
            orphan
        ));
    }
    Err(format!("solid 串与可视依据对不上：{why}").into())
}

// ---------------------------------------------------------------------------
// `.scene` 文本读取
// ---------------------------------------------------------------------------

struct Ground {
    width: usize,
    height: usize,
    kinds: Vec<i32>,
    solid: Vec<bool>,
}

impl Ground {
    /// 墙/水那部分是"瓦自己画的"，与 sprite 无关。
    fn native(&self) -> BTreeSet<(i32, i32)> {
        (0..self.height)
            .flat_map(|row| (0..self.width).map(move |col| (col, row)))
            .filter(|(col, row)| {
                matches!(self.kinds[row * self.width + col], WALL_KIND | WATER_KIND)
            })
            .map(|(col, row)| (col as i32, row as i32))
            .collect()
    }

    fn solid_cells(&self) -> BTreeSet<(i32, i32)> {
        (0..self.height)
            .flat_map(|row| (0..self.width).map(move |col| (col, row)))
            .filter(|(col, row)| self.solid[row * self.width + col])
            .map(|(col, row)| (col as i32, row as i32))
            .collect()
    }
}

fn read_ground(path: &Path) -> Result<Ground, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut hits = Vec::new();
    for line in text.replace("\r\n", "\n").split('\n') {
        let field: Vec<&str> = line.split('\t').collect();
        if field.len() == 4 && field[0] == "component" && field[2] == "Tilemap"
            && field[3].contains(&format!("texture={GROUND_TEXTURE}"))
        {
            hits.push(field[3].to_owned());
        }
    }
    if hits.len() != 1 {
        return Err(format!(
            "期望恰好一条 {GROUND_TEXTURE} 的 Tilemap，实到 {} 条",
            hits.len()
        )
        .into());
    }
    let fields = parse_fields(&hits[0]);
    let width: usize = fields
        .get("width")
        .ok_or("Tilemap 没有 width")?
        .parse()?;
    let height: usize = fields
        .get("height")
        .ok_or("Tilemap 没有 height")?
        .parse()?;
    let kinds = fields
        .get("tiles")
        .ok_or("Tilemap 没有 tiles")?
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(|item| item.parse::<i32>())
        .collect::<Result<Vec<_>, _>>()?;
    let solid: Vec<char> = fields
        .get("solid")
        .ok_or("Tilemap 没有 solid")?
        .chars()
        .collect();
    if kinds.len() != width * height || solid.len() != width * height {
        return Err(format!(
            "tiles 串 {} / solid 串 {}，都应为 {}x{} = {}",
            kinds.len(),
            solid.len(),
            width,
            height,
            width * height
        )
        .into());
    }
    Ok(Ground {
        width,
        height,
        kinds,
        solid: solid.iter().map(|ch| *ch == '1').collect(),
    })
}

fn read_scene(path: &Path) -> Result<Vec<Placed>, Box<dyn std::error::Error>> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut names: BTreeMap<i64, String> = BTreeMap::new();
    let mut roles: BTreeMap<i64, String> = BTreeMap::new();
    let mut cells: BTreeMap<i64, (i32, i32)> = BTreeMap::new();
    let mut sprite_of: BTreeMap<i64, String> = BTreeMap::new();
    let mut transform_of: BTreeMap<i64, String> = BTreeMap::new();
    for line in text.replace("\r\n", "\n").split('\n') {
        let field: Vec<&str> = line.split('\t').collect();
        if field.len() < 3 {
            continue;
        }
        let Ok(id) = field[1].parse::<i64>() else {
            continue;
        };
        match field[0] {
            "entity" => {
                names.insert(id, field[2].to_owned());
            }
            "component" if field[2] == "Sprite" => {
                sprite_of.insert(id, field[3..].join("\t"));
            }
            "component" if field[2] == "Transform" => {
                transform_of.insert(id, field[3..].join("\t"));
            }
            "entity_meta" if field[2] == "role" => {
                roles.insert(id, field[3].to_owned());
            }
            "entity_meta" if field[2] == "cell" => {
                let mut axis = field[3].split(',');
                if let (Some(x), Some(y)) = (
                    axis.next().and_then(|v| v.parse::<i32>().ok()),
                    axis.next().and_then(|v| v.parse::<i32>().ok()),
                ) {
                    cells.insert(id, (x, y));
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (id, name) in &names {
        let (Some(body), Some(transform)) = (sprite_of.get(id), transform_of.get(id)) else {
            continue;
        };
        let fields = parse_fields(body);
        let position = parse_fields(transform)
            .get("position")
            .cloned()
            .ok_or_else(|| format!("{name} 的 Transform 没有 position 字段"))?;
        let mut axis = position.split(',');
        let x: f64 = axis.next().unwrap_or("0").parse()?;
        let y: f64 = axis.next().unwrap_or("0").parse()?;
        let size = fields
            .get("size")
            .ok_or_else(|| format!("{name} 的 Sprite 没有 size 字段"))?;
        let mut wh = size.split('x');
        let width: u32 = wh.next().unwrap_or("0").parse()?;
        let height: u32 = wh.next().unwrap_or("0").parse()?;
        out.push(Placed {
            name: name.clone(),
            role: roles.get(id).cloned().unwrap_or_default(),
            cell: cells.get(id).copied(),
            texture: fields.get("texture").cloned().unwrap_or_default(),
            layer: fields
                .get("sorting_layer")
                .cloned()
                .unwrap_or_else(|| "default".to_owned()),
            order: fields
                .get("order")
                .and_then(|value| value.parse::<f64>().ok())
                .map_or(0, |value| value as i32),
            x,
            y,
            width,
            height,
        });
    }
    out.sort_by_key(|item| item.name.clone());
    Ok(out)
}

fn parse_fields(value: &str) -> BTreeMap<String, String> {
    value
        .split(';')
        .filter_map(|part| part.split_once('='))
        .map(|(key, raw)| (key.to_owned(), raw.to_owned()))
        .collect()
}

fn find<'a>(sprites: &'a [Placed], name: &str) -> Result<&'a Placed, Box<dyn std::error::Error>> {
    sprites
        .iter()
        .find(|item| item.name == name)
        .ok_or_else(|| format!("场景里没有名为 {name} 的 sprite 实体").into())
}

fn image_of(project: &Path, rel: &str) -> Result<RgbaImage, Box<dyn std::error::Error>> {
    let path = project.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(image.to_rgba8())
}

// ---------------------------------------------------------------------------
// 运行期：引擎读回来的实心位图
// ---------------------------------------------------------------------------

struct LiveGrid {
    cols: usize,
    rows: usize,
    cells: Vec<bool>,
}

fn ground_solid(runtime: &mut RuntimeScene) -> Result<LiveGrid, Box<dyn std::error::Error>> {
    let text = lua_str(
        runtime,
        r#"(function()
             local id = entity.find('GroundMap')
             if not id or id == 0 then return 'nomap' end
             local dims = tilemap.get(id)
             local out = {}
             for row = 0, dims.height - 1 do
               for col = 0, dims.width - 1 do
                 out[#out + 1] = tilemap.is_solid(id, col, row) and '1' or '0'
               end
             end
             return string.format('%d,%d,%s', dims.width, dims.height, table.concat(out))
           end)()"#,
    )?;
    if text == "nomap" {
        return Err("运行期找不到 GroundMap：solid 位图不存在，物理事实源也就无从对拍".into());
    }
    let mut parts = text.splitn(3, ',');
    let cols: usize = parts.next().unwrap_or("0").parse()?;
    let rows: usize = parts.next().unwrap_or("0").parse()?;
    let run = parts.next().unwrap_or("");
    if run.len() != cols * rows || !run.bytes().all(|b| b == b'0' || b == b'1') {
        return Err(format!(
            "tilemap.is_solid 回读到的串长 {}，应为 {cols}x{rows} 的 0/1：位图取歪了",
            run.len()
        )
        .into());
    }
    Ok(LiveGrid {
        cols,
        rows,
        cells: run.bytes().map(|b| b == b'1').collect(),
    })
}

fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W, VIEW_H)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
}

fn assert_scene(
    runtime: &mut RuntimeScene,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = lua_str(runtime, "scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}

fn lua_run(runtime: &mut RuntimeScene, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime
        .with_script(|script| script.execute_console(code))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn lua_str(runtime: &mut RuntimeScene, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let text = runtime
        .with_script(|script| script.execute_console(&format!("return tostring({expr})")))
        .map_err(|error| error.to_string())?;
    Ok(text.trim().to_owned())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}
