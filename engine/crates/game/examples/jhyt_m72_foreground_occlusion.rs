//! Headless acceptance drive for 前景遮挡层与全局光源投影（M72-06）。
//!
//! 本包要消灭的是"画面是平的"：前景压不住人、所有物体没有方向一致的投影。
//! docs/51 §2.1 与 docs/53 §3.9 早就写了这两条要求，盘上此前**一条实现都没有**。
//!
//! ① **光向事实源**：`scripts/data/lighting.lua` 是全局光源方向、投影长度、遮挡容差的
//!    唯一出处。这一节断言它的 `validate_all` 干净、它真的被登记进 `validate_data.lua`
//!    （否则"校验"是一段永远不会被调用的死代码——m23 就是为这条设的门），
//!    并把三条最容易写反的取值做成**负向对照**：光向符号（影子朝天上去不会报错）、
//!    投影透明度（a=1 是地上一个洞）、前景 order（遮断物压住刀光）。
//! ③ **拆层的结构契约**：`manor_tree.png` / `manor_house.png` 各拆成"本体块留 world +
//!    前景块进 foreground"。这一节只查盘上的东西是不是同一件：每件遮断物必须由一个本体
//!    派生（命名成对、数量对得上、没有孤儿前景 sprite）、只有 Sprite+Transform（**不许有
//!    Collider**，否则玩家会被一棵看不见的树顶住）、layer/order 与 `lighting.lua` 同档、
//!    声明尺寸 == PNG 实测、接缝等式 `occ.x==base.x && occ.y==base.y+base.h &&
//!    base.h+occ.h==full.h`，最后把两张分块**按行拼回去逐字节等于整图**——这是
//!    `gen_manor_art.check_splits` 的第二份独立实现，两边都说"铺满了"才算铺满。
//! ④ **覆盖率**：玩家能站到的位置里，前景块盖住玩家不透明像素的最大占比必须落在
//!    `lighting.occlusion` 的上下界之间。上下界都取自那个文件，门自己一个数都不写。
//!    三条口径是这条断言能不能站住的关键，全写在下面的注释里：可达集（GroundMap 的
//!    `solid`，经 `tilemap.is_solid` 现取，再与运行期按格生成的地表碰撞体逐格对拍）、
//!    像素对齐（1 texel = 1 世界单位，整数栅格）、
//!    以及**用哪一帧的角色**（序列帧的每一帧都各扫一遍，取最坏——把"恰好读到窄帧"
//!    这种运气排除掉）。
//! ⑤ **绘制序真反转**：在最深处那一格，canopy 的 `mesh.draws` 下标必须大于玩家；把四片
//!    canopy 用运行期 `sprite.set` 改回 `world` 层，方向必须翻过来，还原后必须再翻回去。
//!    这一节不需要临时场景文件：`jhyt_m35_depth_sort` §① 已经证明运行期写的
//!    `sorting_layer`/`order` 在原生真的生效。
//! ⑥ **投影贴片**（C-2）：`assets/art/shadows/` 那 16 件每一件都按
//!    `lighting.sun × shadow_reach × 本体总高` 与**本体原图的落地行**独立复算一遍落点，
//!    于是"接触点→贴片正中那个 texel"的角度**精确**等于 sun（方差 0，不是"接近"——贴片
//!    尺寸写成 `2r+1` 就是为了让"中心"落在一个 texel 上）。同一条节里还断言层带、order、
//!    tint、纯白、奇数尺寸，以及体量门槛的**两个方向**（够格必须有影子、不够格必须没有），
//!    最后把一件贴片用运行期 `sprite.set` 抬进 foreground 做反面对照。
//! ⑦ **脚下接触阴影**（C-1）：8 个朝向 × 每帧的椭圆质心恒在身体中心的**右侧**
//!    （`actor_shadow.shift_x` 那一侧）。这条盯的是"阴影烘进身体再整帧镜像"——那时
//!    w/sw/nw 三族的影子会翻到左边，一批图就有了两个光向（docs/53 §3.9）。
//! ⑧ **Y 排序容差**（C-3）：实测 order 桶宽 == `world/depth.lua` 的 STEP，且方向是
//!    y 越大 order 越小。这条口径在跨宿主绘制序上踩过"写反了两边门都绿"的坑。
//!
//! 用法：
//!   cargo run --quiet -p game --example jhyt_m72_foreground_occlusion [-- <game_jhyt-root>]

use engine::{
    core::{input::InputSnapshot, script::LuaRuntime},
    host::{RuntimeScene, SceneMesh},
};
use image::RgbaImage;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;

const SCENE: &str = "scenes/manor.scene";
const PLAYER: &str = "JianghuPlayer";
const PLAYER_DIR: &str = "assets/art/characters/player_male/";
const OCCLUDER_LAYER: &str = "foreground";
const BASE_LAYER: &str = "world";
/// §⑥ 认领投影段的依据（`entity_meta … role cast_shadow`，与 `set_cast_shadows.py` 同名）。
const SHADOW_ROLE: &str = "cast_shadow";
/// "站在地面上"的两个目录：§⑥ 用它找**够格却没有影子**的漏挂（小物件反过来由
/// `lighting.cast_shadow.min_reach_units` 挡掉，两边都断言才叫门槛）。
const BUILDING_DIR: &str = "assets/art/buildings/";
const PROP_DIR: &str = "assets/art/props/";
/// 八方向：§⑦ 每个朝向各量一次脚下阴影落在身体中心的哪一侧。
const FACINGS: [&str; 8] = ["s", "n", "e", "w", "se", "sw", "ne", "nw"];

/// 玩家 `Collider shape=ball;radius=12`，刚体建在 `Transform` 上 ⇒ 圆心就是实体坐标。
const PLAYER_RADIUS: f32 = 12.0;
/// 主角贴图 64×64 且**没有 pivot 字段** ⇒ 默认 0.5（中心对齐），所以 Transform 是四边形中心。
const PLAYER_HALF: i32 = 32;
/// 候选网格步进：2 世界单位 = 1/16 格。再密不改变结论，只拖慢门。
const SWEEP_STEP: i32 = 2;
const TILE: i32 = 32;
/// 传送后站不站得住的容差（物理每帧推进；漂出这个范围就说明那格其实是墙里）。
const HOLD_TOLERANCE: f32 = 2.0;
/// 传送后跑这么多帧再回读位置/绘制序。
const SETTLE_FRAMES: usize = 8;

/// `gen_manor_art.LAYER_SPLITS` 的门侧副本：`(本体实体名, 整图, 前景块, 本体块, 遮断物实体名)`。
/// 手抄是故意的：门若去解析那份 Python，就变成同一张表自己跟自己一致，"生成器改了切点而场景
/// 没跟着改"这一类漂移反而查不出来。副本与表飘红时报的是"拼回去不等于整图"或"接缝等式不成立"，
/// 那正是飘掉的那一条。行号一个都不抄——所有高度都从 PNG 实测，所以"切点铺满原图"是被证明的。
///
/// 第 0 行必须是正房（§④ 的"脚下钉子"按 `SPLITS[0]` 取）；余下按实体名查，不靠行号。
#[derive(Clone, Copy)]
struct Split {
    base: &'static str,
    full: &'static str,
    top: &'static str,
    bottom: &'static str,
    occluder: &'static str,
}

const SPLITS: &[Split] = &[
    Split {
        base: "ManorHouse",
        full: "assets/art/buildings/manor_house.png",
        top: "assets/art/buildings/manor_house_roof.png",
        bottom: "assets/art/buildings/manor_house_body.png",
        occluder: "ManorHouseRoof",
    },
    Split {
        base: "ManorShed",
        full: "assets/art/buildings/manor_shed.png",
        top: "assets/art/buildings/manor_shed_roof.png",
        bottom: "assets/art/buildings/manor_shed_body.png",
        occluder: "ManorShedRoof",
    },
    Split {
        base: "BuildTrainingHall",
        full: "assets/art/buildings/build_training_hall.png",
        top: "assets/art/buildings/build_training_hall_roof.png",
        bottom: "assets/art/buildings/build_training_hall_body.png",
        occluder: "BuildTrainingHallRoof",
    },
    Split {
        base: "BuildStorehouse",
        full: "assets/art/buildings/build_storehouse.png",
        top: "assets/art/buildings/build_storehouse_roof.png",
        bottom: "assets/art/buildings/build_storehouse_body.png",
        occluder: "BuildStorehouseRoof",
    },
    Split {
        base: "BuildHerbHut",
        full: "assets/art/buildings/build_herb_hut.png",
        top: "assets/art/buildings/build_herb_hut_roof.png",
        bottom: "assets/art/buildings/build_herb_hut_body.png",
        occluder: "BuildHerbHutRoof",
    },
    Split {
        base: "BuildTeaShed",
        full: "assets/art/buildings/build_tea_shed.png",
        top: "assets/art/buildings/build_tea_shed_roof.png",
        bottom: "assets/art/buildings/build_tea_shed_body.png",
        occluder: "BuildTeaShedRoof",
    },
    Split {
        base: "BuildForgeStand",
        full: "assets/art/buildings/build_forge_stand.png",
        top: "assets/art/buildings/build_forge_stand_roof.png",
        bottom: "assets/art/buildings/build_forge_stand_body.png",
        occluder: "BuildForgeStandRoof",
    },
    Split {
        base: "ManorTreeWest",
        full: "assets/art/props/manor_tree.png",
        top: "assets/art/props/manor_tree_canopy.png",
        bottom: "assets/art/props/manor_tree_trunk.png",
        occluder: "ManorTreeWestCanopy",
    },
    Split {
        base: "ManorTreeNorth",
        full: "assets/art/props/manor_tree.png",
        top: "assets/art/props/manor_tree_canopy.png",
        bottom: "assets/art/props/manor_tree_trunk.png",
        occluder: "ManorTreeNorthCanopy",
    },
    Split {
        base: "ManorTreeEast",
        full: "assets/art/props/manor_tree.png",
        top: "assets/art/props/manor_tree_canopy.png",
        bottom: "assets/art/props/manor_tree_trunk.png",
        occluder: "ManorTreeEastCanopy",
    },
    Split {
        base: "ManorTreeSoutheast",
        full: "assets/art/props/manor_tree.png",
        top: "assets/art/props/manor_tree_canopy.png",
        bottom: "assets/art/props/manor_tree_trunk.png",
        occluder: "ManorTreeSoutheastCanopy",
    },
];

/// 场景里一个 sprite 实体的门侧视图。`body` 留着是给 §⑤ 反面对照原样回写 `sprite.set`
/// 用的（只把 `sorting_layer` 那一段换掉），不重拼一遍字段。
#[derive(Clone, Debug)]
struct Placed {
    name: String,
    body: String,
    texture: String,
    width: u32,
    height: u32,
    order: i32,
    layer: String,
    x: f32,
    y: f32,
    collider: bool,
    /// `tint=` 原样字符串（`r,g,b,a`，三位小数）——§⑥ 拿它跟 `lighting.shadow_tint` 对，
    /// 因为"影子多暗"在盘上只许有一处出处。
    tint: String,
    /// `entity_meta … role …`，没有则空串。§⑥ 用它认领投影段（贴图前缀只是辅助）。
    role: String,
}

/// 一张遮断物的前景块：左下角 (x,y) + 不透明像素的位图（行序与图像行一致，0 = 顶行）。
struct Occluder {
    name: String,
    texture: String,
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    on: Vec<bool>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let scripts = project.join("scripts");
    let mut lua = LuaRuntime::new()?;
    lua.set_project_root(project.to_path_buf());
    lua.set_ui_viewport(1280, 720);
    lua.set_delta(1.0 / 60.0);
    lua.execute_project_script(&scripts.join("main.lua"))?;

    banner("① 光向事实源：lighting.lua 自身干净，且它的校验真的会跑");
    let own_errors = query_str(
        &mut lua,
        "local errs = require('data.lighting').validate_all()\n\
         return tostring(#errs) .. '|' .. table.concat(errs, ' / ')",
    )?;
    if !own_errors.starts_with("0|") {
        return Err(format!("data/lighting.lua validate_all reports: {own_errors}").into());
    }
    // 登记覆盖：定义了 validate_all 却没登记 = 这段校验永远不会被调用（症状是"规则写好了
    // 却从没生效"，而不是任何报错）。这里按**路径**匹配，与 m23 §② 同一个口径。
    let registered = query_str(
        &mut lua,
        "local hit = ''\n\
         for _, entry in ipairs(require('validate_data').registered()) do\n\
           if entry.path == 'data.lighting' then hit = entry.name end\n\
         end\n\
         return hit",
    )?;
    if registered != "lighting" {
        return Err(
            "data.lighting defines validate_all but is not registered in \
             scripts/validate_data.lua (its validation never runs)"
                .into(),
        );
    }
    // 走**全量校验路径**：证明它不只是"自己调用自己"，而是 `validate_data.check` 真的会带它。
    let report = query_str(
        &mut lua,
        "return require('validate_data').check({ strict = true })",
    )?;
    if report.contains("lighting:") {
        return Err(format!("validate_data.check reported lighting issues:\n{report}").into());
    }

    banner("② 负向对照：四处最容易写反的取值必须被点名（改完立刻还原）");
    // 每条：注入 → 期望诊断子串 → 还原。期望串与 lighting.lua 的诊断文案同源。
    let cases: &[(&str, &str, &str)] = &[
        (
            "require('data.lighting').sun.dir_y = 1",
            "sun.dir_y 必须为负",
            "require('data.lighting').sun.dir_y = -1",
        ),
        (
            "require('data.lighting').shadow_tint.a = 1",
            "shadow_tint.a 必须落在 (0, 0.6)",
            "require('data.lighting').shadow_tint.a = 0.28",
        ),
        (
            // 遮断物 order 抬到特效之上 ⇒ 刀光被自己的树冠埋掉，画面上是"砍下去没亮"。
            "require('data.lighting').foreground_order.occluder = 10",
            "特效的 order 必须大于遮断物",
            "require('data.lighting').foreground_order.occluder = -100",
        ),
        (
            // 下限漏断言时，"前景层加了却一件都没压到人"也是绿的（空门）。
            "require('data.lighting').occlusion.min_cover_ratio = 0.9",
            "min_cover_ratio 必须落在 [0, max_cover_ratio)",
            "require('data.lighting').occlusion.min_cover_ratio = 0.05",
        ),
    ];
    for (inject, want, restore) in cases {
        let count_before = query_str(
            &mut lua,
            "return tostring(#require('data.lighting').validate_all())",
        )?;
        if count_before != "0" {
            return Err(format!("case baseline not clean after restore: {inject}").into());
        }
        query_str(&mut lua, inject)?;
        let errors = query_str(
            &mut lua,
            "return table.concat(require('data.lighting').validate_all(), ' / ')",
        )?;
        query_str(&mut lua, restore)?;
        if !errors.contains(want) {
            return Err(format!(
                "negative control {inject} did not report {want:?}, got: {errors}"
            )
            .into());
        }
    }

    // 后面三节全部**从这一个源取数**：门里不写第二个 0.65 / -100。
    let occluder_order = query_num(
        &mut lua,
        "require('data.lighting').foreground_order.occluder",
    )?;
    let effect_order = query_num(&mut lua, "require('data.lighting').foreground_order.effect")?;
    let max_cover = query_num(
        &mut lua,
        "require('data.lighting').occlusion.max_cover_ratio",
    )?;
    let min_cover = query_num(
        &mut lua,
        "require('data.lighting').occlusion.min_cover_ratio",
    )?;

    let sprites = read_scene(&project.join(SCENE))?;
    banner("③ 拆层结构契约：本体与前景成对、接缝等式、以及\"拼回去逐字节等于整图\"");
    let occluders: Vec<&Placed> = sprites
        .iter()
        .filter(|item| item.layer == OCCLUDER_LAYER)
        .collect();
    if occluders.len() != SPLITS.len() {
        return Err(format!(
            "{SCENE} 里 foreground 层的 sprite 有 {} 件，拆层表有 {} 行：要么漏挂一块遮断物\
             （画面上是那棵树没有冠），要么多了一件没人认领的前景 sprite（它会盖住不该盖的东西）",
            occluders.len(),
            SPLITS.len()
        )
        .into());
    }
    // 数量相同还不够：还得每一件前景都被某一行拆层表认领（孤儿前景会盖住不该盖的东西，
    // 而它不在任何一行的接缝等式里，永远不会被拼回整图那条查到）。
    let orphans: Vec<&str> = occluders
        .iter()
        .filter(|item| !SPLITS.iter().any(|row| row.occluder == item.name))
        .map(|item| item.name.as_str())
        .collect();
    if !orphans.is_empty() {
        return Err(format!(
            "foreground 层里有拆层表没认领的 sprite：{orphans:?}。它们排在一切东西之上，\
             却没有任何一处断言在管它们画的是什么"
        )
        .into());
    }
    // 排序前提（两条都在门里现取现比，不抄 lighting.lua 的结论）：遮断物必须比特效先画，
    // 否则刀光被树冠埋掉；层秩的比较交给 §⑤ 的运行期反转来证。
    if !(occluder_order < effect_order) {
        return Err(format!(
            "场景用的遮断物 order {occluder_order} 不低于特效 order {effect_order}：\
             树冠会把刀光埋掉（`lighting.validate_all` 也在钉这一条，两处一起红才是真红）"
        )
        .into());
    }
    for row in SPLITS {
        let base = find(&sprites, row.base)?;
        let occ = find(&sprites, row.occluder)?;
        if occ.collider {
            return Err(format!(
                "{} 带 Collider：遮断物是**画在人身上的那片像素**，不是墙。给它加碰撞体，玩家会\
                 被一棵透明树冠顶在门外",
                row.occluder
            )
            .into());
        }
        if occ.layer != OCCLUDER_LAYER {
            return Err(format!(
                "{} 的 sorting_layer = {:?}，应为 {OCCLUDER_LAYER:?}：层秩×1000 是排序主键，\
                 写回 world 就等于这块前景永远不会压到人（§④ 的方向断言会立刻红）",
                row.occluder, occ.layer
            )
            .into());
        }
        if base.layer != BASE_LAYER {
            return Err(format!(
                "{} 的 sorting_layer = {:?}，本体必须留在 {BASE_LAYER:?}：本体是要被人挡的，\
                 跟着进 foreground 就变成\"树干浮在玩家身上\"",
                row.base, base.layer
            )
            .into());
        }
        if occ.order as f64 != occluder_order {
            return Err(format!(
                "{} 的 order = {}，`lighting.foreground_order.occluder` 是 {occluder_order}：\
                 两处各写一套数，拆层就没了唯一口径",
                row.occluder, occ.order
            )
            .into());
        }
        if base.texture != row.bottom || occ.texture != row.top {
            return Err(format!(
                "{} 用 {:?}、{} 用 {:?}，拆层表说的是 {:?} / {:?}",
                row.base, base.texture, row.occluder, occ.texture, row.bottom, row.top
            )
            .into());
        }
        let bottom = image_of(&project, row.bottom)?;
        let top = image_of(&project, row.top)?;
        let full = image_of(&project, row.full)?;
        for (label, item, img) in [(row.base, base, &bottom), (row.occluder, occ, &top)] {
            if (item.width, item.height) != (img.width(), img.height()) {
                return Err(format!(
                    "{label} 声明 {}x{}，PNG 实测 {}x{}：改了一处另一处不会自己跟着改",
                    item.width,
                    item.height,
                    img.width(),
                    img.height()
                )
                .into());
            }
        }
        // 接缝等式：两块共用同一个 x 原点，前景块抬到本体顶边（世界 +y 朝上 ⇒ 顶边 = y+h）。
        if (occ.x - base.x).abs() > 0.001 {
            return Err(format!(
                "{}.x = {:.1} ≠ {}.x = {:.1}：两块横向错开，树冠会斜着飘出去",
                row.occluder, occ.x, row.base, base.x
            )
            .into());
        }
        if (occ.y - (base.y + base.height as f32)).abs() > 0.001 {
            return Err(format!(
                "{}.y = {:.1} ≠ {}.y + 高 = {:.1}：接缝中间留了一行空白（画面上是树冠比树干\
                 高一截）或者两块重叠",
                row.occluder,
                occ.y,
                row.base,
                base.y + base.height as f32
            )
            .into());
        }
        if base.height + occ.height != full.height() || full.width() != bottom.width() {
            return Err(format!(
                "{}({}) + {}({}) ≠ 整图 {}({})：切分没铺满，整图比两块高（或宽）",
                row.bottom,
                base.height,
                row.top,
                occ.height,
                row.full,
                full.height()
            )
            .into());
        }
        // 按行拼回去 == 整图，**逐字节**。这是 `gen_manor_art.check_splits` 的第二份实现：
        // 生成器自己说"我铺满了"不算，运行期要拿的正是这三张图。
        let joined = stack(&top, &bottom);
        if joined != rgba(&full) {
            return Err(format!(
                "{} + {} 按行拼回去 ≠ 整图 {}：两块的像素与原图对不上，接缝处要么有色阶漂移\
                 要么切点行号错了",
                row.bottom, row.top, row.full
            )
            .into());
        }
    }
    // 反面对照：把两块**上下对调**再拼一遍，必须不等于整图。少了这一条，
    // `joined != rgba(&full)` 有可能只是在比两坨全透明像素（判别力为零）。
    // 挑树冠：它的两块最"实"，上下颠倒一眼就看得出来。
    let tree = *SPLITS
        .iter()
        .find(|row| row.occluder.ends_with("Canopy"))
        .ok_or("拆层表里没有树冠行：§③ 的对调对照没有可对调的东西")?;
    let canopy = image_of(&project, tree.top)?;
    let trunk = image_of(&project, tree.bottom)?;
    let full = image_of(&project, tree.full)?;
    if stack(&trunk, &canopy) == rgba(&full) {
        return Err(
            "反面对照失手：manor_tree 的两块上下对调后拼回去仍然 == 整图，\
             §③ 的逐字节比对其实没在看内容"
                .into(),
        );
    }
    println!(
        "  {} 件遮断物与 {} 行拆层表一一成对；无 Collider、层与 order 同源于 lighting.lua；\
         接缝等式成立；两张分块按行拼回去逐字节 == 整图（含反面对照）",
        occluders.len(),
        SPLITS.len()
    );

    banner("④ 覆盖率：可达位置里被前景盖住的角色像素占比必须落在 lighting 的上下界之间");
    let masks: Vec<Occluder> = occluders
        .iter()
        .map(|item| -> Result<Occluder, Box<dyn std::error::Error>> {
            let img = image_of(&project, &item.texture)?;
            Ok(Occluder {
                name: item.name.clone(),
                texture: item.texture.clone(),
                x: item.x as i32,
                y: item.y as i32,
                w: img.width() as usize,
                h: img.height() as usize,
                on: opaque_rows(&img),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "main")?;
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    assert_scene(&mut runtime, "manor")?;
    runtime.set_sprite_culling(false);
    let solid = ground_solid(&mut runtime)?;
    solid_physics_probe(&mut runtime, &solid)?;

    // 数据侧的钉子：正房脚下必须是实心格。上面的探针已经把"位图 ↔ 物理碰撞体"钉成同一张，
    // 这一条钉的是另一半——**位图覆盖没覆盖建筑物**：漏了的话玩家能走进屋里，而门算出的
    // "可达集"会比游戏里真正能站的地方大出一块，覆盖率就是在一个进不去的位置上量的。
    let house = find(&sprites, SPLITS[0].base)?;
    let hx = (house.x + house.width as f32 / 2.0) as i32;
    let hy = (house.y + house.height as f32 / 2.0) as i32;
    if solid.blocked_at(hx, hy) != Some(true) {
        return Err(format!(
            "{} 的脚下 ({hx},{hy}) 在位图上不是实心格：地基没进 `solid`，玩家会走进屋里，\
             而门的可达集比游戏里真能站的地方大出一块",
            house.name
        )
        .into());
    }
    println!(
        "  数据侧的钉子：{} 脚下 ({hx},{hy}) 是实心格（地基进了 `solid`）",
        house.name
    );

    // 角色掩码取**门读到的那张贴图与条带**，不假设是 idle_s 第 0 帧：
    // 从 mesh 找玩家四边形（贴图路径前缀认），UV 反推条带里的帧数，然后**每一帧各扫一遍**、
    // 取最坏。这样"恰好读到一帧偏窄的"不会再让门忽绿忽红，也把序列帧里最宽的一帧一起钉住。
    let (sheet, frames) = player_strip(&project, &mut runtime)?;
    let image = image_of(&project, &sheet)?;
    let strips = frame_masks(&image, frames)?;

    let mut per_occ: BTreeMap<String, Sweep> = BTreeMap::new();
    let mut scanned = 0usize;
    for offsets in &strips {
        for occ in &masks {
            let hit = sweep_occ(occ, offsets, &solid);
            scanned += 1;
            let entry = per_occ
                .get(occ.name.as_str())
                .copied()
                .unwrap_or(Sweep::default());
            let best = if hit.reachable_ratio > entry.reachable_ratio {
                Sweep {
                    reachable_ratio: hit.reachable_ratio,
                    x: hit.x,
                    y: hit.y,
                    any_ratio: entry.any_ratio.max(hit.any_ratio),
                }
            } else {
                Sweep {
                    any_ratio: entry.any_ratio.max(hit.any_ratio),
                    ..entry
                }
            };
            per_occ.insert(occ.name.clone(), best);
        }
    }
    let mut rows = Vec::new();
    let mut worst = 0.0f64;
    let mut deepest: Option<(&Occluder, i32, i32)> = None;
    let mut unconstrained = 0.0f64;
    for occ in &masks {
        let hit = per_occ
            .get(occ.name.as_str())
            .copied()
            .ok_or_else(|| format!("{} 没有扫描结果", occ.name))?;
        unconstrained = unconstrained.max(hit.any_ratio);
        rows.push(format!(
            "{} {:.3}（不限可行走 {:.3}）",
            short(&occ.name),
            hit.reachable_ratio,
            hit.any_ratio
        ));
        if hit.reachable_ratio > worst {
            worst = hit.reachable_ratio;
            deepest = Some((occ, hit.x, hit.y));
        }
    }
    let (occ, deep_x, deep_y) =
        deepest.ok_or("可达集里没有任何位置与任何前景块相交：图上的树是飘在虚空里的")?;
    if worst > max_cover {
        return Err(format!(
            "可达位置的最大覆盖 {worst:.3} 超过上界 {max_cover}：前景会把人整个吞掉。\
             最深点在 {} 的 ({deep_x},{deep_y})",
            occ.name
        )
        .into());
    }
    if worst < min_cover {
        return Err(format!(
            "最大覆盖只有 {worst:.3}，低于下限 {min_cover}：前景层加了却没压到过任何人，\
             这一层是白加的（空门）"
        )
        .into());
    }
    println!(
        "  角色条带 {sheet} 共 {frames} 帧、{} 件前景、{scanned} 次扫窗（步进 {SWEEP_STEP} 单位，\
         每帧每块各扫一遍取最坏）",
        masks.len()
    );
    println!(
        "  各处最深覆盖（可达口径 / 不限可行走的理论上界）：{}",
        rows.join("、")
    );
    println!(
        "  判据取自 lighting：max_cover_ratio = {max_cover}、min_cover_ratio = {min_cover}；\
         实际最深 {worst:.3} @ {} 的 ({deep_x},{deep_y})；全图理论上界 {unconstrained:.3}\
         （那一格在位图上是实心的、有地表碰撞体，玩家进不去 ⇒ 这一列只说明扫描没漏看）",
        occ.name
    );

    // 把玩家真的传送到最深那一格，再回读坐标。上面那条逐格探针已经证明 `solid` 位图与物理
    // 世界是同一张（359/359 实心格有碰撞体、561 空格没有），所以这条落位断言现在真的有牙：
    // 门按几何判"可达"的点，物理求解器也必须让玩家待得住——两边口径一旦分家（换算方向改了、
    // 有人给 tilemap 加了内缩/外扩），这里就会漂出容差。另外两件小事也靠它兜住：
    //   ① 扫描用的点与绘制序比的是**同一个**点（回读 = 落位，容差内），不是门自己想象的坐标；
    //   ② 传送走的是游戏自己的入口，`transform.set` 那种只改 ECS 的写法会在下一帧被刚体覆盖。
    let teleported = lua_str(
        &mut runtime,
        &format!("require('player.player').teleport({deep_x}, {deep_y})"),
    )?;
    if teleported != "true" {
        return Err(format!(
            "传送到最深点 ({deep_x},{deep_y}) 返回 {teleported:?}：下面那条绘制序断言就没有主体"
        )
        .into());
    }
    for _ in 0..SETTLE_FRAMES {
        frame(&mut runtime)?;
    }
    let held = runtime_lua(
        &mut runtime,
        &format!(
            "local t = transform.get(entity.find('{PLAYER}'))\n\
                 return string.format('%0.3f,%0.3f', t.x, t.y)"
        ),
    )?;
    let mut axes = held.split(',');
    let got_x: f32 = axes.next().unwrap_or("0").parse()?;
    let got_y: f32 = axes.next().unwrap_or("0").parse()?;
    if (got_x - deep_x as f32).abs() > HOLD_TOLERANCE
        || (got_y - deep_y as f32).abs() > HOLD_TOLERANCE
    {
        return Err(format!(
            "最深点 ({deep_x},{deep_y}) 传送后漂到 ({got_x},{got_y})（容差 {HOLD_TOLERANCE}）：\
             门按 solid 判的\"可达\"与物理求解器给的位置不一致，覆盖率是在一个站不住的地方量的"
        )
        .into());
    }
    println!(
        "  最深点经 player.teleport 落位，{SETTLE_FRAMES} 帧后停在 ({got_x},{got_y})（容差内）"
    );

    // 方向断言：在这一格，前景块的绘制下标必须**大于**玩家（后画 = 盖在上面）。
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let player_draw = player_draw_index(&mesh)?;
    let front = draw_indices(&mesh, &occ.texture)?;
    let front_min = front.into_iter().min().unwrap_or(usize::MAX);
    if front_min <= player_draw {
        return Err(format!(
            "{} 的分段下标 {front_min} ≤ 玩家的 {player_draw}：前景排在玩家之前画，画面上是\
             \"树冠长在人身后\"，本包要消灭的就是这一条",
            occ.name
        )
        .into());
    }
    println!(
        "  绘制序：{} 的分段下标 {front_min} > 玩家 {player_draw}（前景后画 ⇒ 压住人）",
        occ.name
    );

    banner("⑤ 反面对照：把所有树冠用运行期 sprite.set 改回 world 层，方向必须翻过来");
    // 走**运行期**而不是临时场景文件：`jhyt_m35_depth_sort` §① 已证明 `sprite.set` 写的
    // sorting_layer/order 在原生真的生效。四片一起改（不是只改一片）：它们共用一张
    // `manor_tree_canopy.png`，只改一片会让同一张贴图散在两个分段里，那时候"下标比较"
    // 量的就不是层带而是批处理了。按**贴图**挑而不是按行号范围：M72-07 之后拆层表里
    // 还有六件屋顶前景，它们用的是别的图，混进来会让这条断言比的是两批不同的四边形。
    let canopy = SPLITS
        .iter()
        .find(|row| row.occluder.ends_with("Canopy"))
        .ok_or("拆层表里没有树冠行：§⑤ 的反面对照没有可翻的东西")?;
    let names: Vec<&str> = SPLITS
        .iter()
        .filter(|row| row.top == canopy.top)
        .map(|row| row.occluder)
        .collect();
    for name in &names {
        let item = find(&sprites, name)?;
        if item.texture != canopy.top {
            return Err(
                format!("{name} 的贴图 = {:?}，不是 {:?}", item.texture, canopy.top).into(),
            );
        }
        let body = item
            .body
            .replace("sorting_layer=foreground", "sorting_layer=world");
        lua_run(
            &mut runtime,
            &format!("sprite.set(entity.find('{name}'), '{body}')"),
        )?;
    }
    for _ in 0..2 {
        frame(&mut runtime)?;
    }
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let player_draw = player_draw_index(&mesh)?;
    let back = draw_indices(&mesh, canopy.top)?;
    let back_max = back.into_iter().max().unwrap_or(0);
    if back_max >= player_draw {
        return Err(format!(
            "反面对照失手：四片 canopy 都改回 world 层后，它的分段下标 {back_max} 仍然 ≥ 玩家的 \
             {player_draw}。那说明 §④ 的\"前景压住人\"量的不是层带——多半是别的系统在决定绘制序"
        )
        .into());
    }
    println!("  改回 world 后 canopy 分段下标 {back_max} < 玩家 {player_draw}（方向确实翻了）");
    for name in &names {
        let item = find(&sprites, name)?;
        lua_run(
            &mut runtime,
            &format!("sprite.set(entity.find('{name}'), '{}')", item.body),
        )?;
    }
    for _ in 0..2 {
        frame(&mut runtime)?;
    }
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let player_draw = player_draw_index(&mesh)?;
    let again = draw_indices(&mesh, canopy.top)?;
    let again_min = again.into_iter().min().unwrap_or(0);
    if again_min <= player_draw {
        return Err(format!(
            "还原后 canopy 分段下标 {again_min} ≤ 玩家 {player_draw}：还原没生效，\
             那 §⑤ 改的那一半也随时会被谁改回来"
        )
        .into());
    }
    println!("  还原后再翻回：canopy {again_min} > 玩家 {player_draw}");

    banner("⑥ 投影贴片：每件独立复算一遍落点，方向必须只剩一个");
    // 数值全部现取：门里不写第二个 8 / 0.2 / -20 / 0.28。
    let cast_order = query_num(&mut lua, "require('data.lighting').cast_shadow.order")? as i32;
    let min_reach = query_num(
        &mut lua,
        "require('data.lighting').cast_shadow.min_reach_units",
    )?;
    let flatten = query_num(
        &mut lua,
        "require('data.lighting').cast_shadow.contact_flatten",
    )?;
    let reach_unit = query_num(&mut lua, "require('data.lighting').shadow_reach")?;
    let sun_x = query_num(&mut lua, "require('data.lighting').sun.dir_x")?;
    let sun_y = query_num(&mut lua, "require('data.lighting').sun.dir_y")?;
    let want_tint = query_str(
        &mut lua,
        "local t = require('data.lighting').shadow_tint\n\
         return string.format('%.3f,%.3f,%.3f,%.3f', t.r, t.g, t.b, t.a)",
    )?;
    let sun_len = (sun_x * sun_x + sun_y * sun_y).sqrt();
    let (dir_x, dir_y) = (sun_x / sun_len, sun_y / sun_len);
    let want_angle = dir_y.atan2(dir_x).to_degrees();

    let shadows: Vec<&Placed> = sprites.iter().filter(|s| s.role == SHADOW_ROLE).collect();
    if shadows.is_empty() {
        return Err(format!(
            "{SCENE} 里没有 role={SHADOW_ROLE} 的实体：C-2 的投影整段没挂上来（画面上是所有建筑\
             和树都没有右下影子），要么就是 role 标错了段。两种情况下下面这一整节都是空转的断言"
        )
        .into());
    }
    let mut shadowed: BTreeSet<String> = BTreeSet::new();
    let mut angles: Vec<f64> = Vec::new();
    for shadow in &shadows {
        let Some(base_name) = shadow.name.strip_suffix("Shadow") else {
            return Err(format!(
                "投影实体 {} 的名字不是 `{{本体}}Shadow`：门找不到它挂在谁身上，\
                 也就没法验证它的方向",
                shadow.name
            )
            .into());
        };
        let base = find(&sprites, base_name)?;
        shadowed.insert(base_name.to_owned());
        if shadow.collider {
            return Err(format!(
                "{} 带 Collider：一片影子会把玩家顶住（装饰与遮断物同一条禁令）",
                shadow.name
            )
            .into());
        }
        if shadow.layer != BASE_LAYER || shadow.order != cast_order {
            return Err(format!(
                "{} 排在 {}/order={}，应为 {BASE_LAYER}/order={cast_order}：投影必须恒在所有本体\
                 之下、装饰之上，这一档取自 lighting.cast_shadow.order",
                shadow.name, shadow.layer, shadow.order
            )
            .into());
        }
        if shadow.tint != want_tint {
            return Err(format!(
                "{} 的 tint={} 与 lighting.shadow_tint({want_tint}) 不同：暗度就有了第二份出处，\
                 改 a 的时候这一件不会跟着走",
                shadow.name, shadow.tint
            )
            .into());
        }
        if shadow.width % 2 == 0 || shadow.height % 2 == 0 {
            return Err(format!(
                "{} 声明 {}x{}：贴片尺寸必须是 `2r+1`（奇数），否则\"贴片中心\"落在四个 texel\
                 中间，角度就不是恒等式而是近似",
                shadow.name, shadow.width, shadow.height
            )
            .into());
        }
        let patch = image_of(&project, &shadow.texture)?;
        if patch.width() != shadow.width || patch.height() != shadow.height {
            return Err(format!(
                "{} 声明 {}x{}，贴图 {} 实为 {}x{}：贴片被缩放着画出来就是软边（docs/57 §1.3）",
                shadow.name,
                shadow.width,
                shadow.height,
                shadow.texture,
                patch.width(),
                patch.height()
            )
            .into());
        }
        for pixel in patch.pixels() {
            let [_, _, _, a] = pixel.0;
            if a != 0 && a != 255 {
                return Err(format!(
                    "{} 有半透明 texel（alpha={a}）：软边门只许 tint 一处带透明度",
                    shadow.texture
                )
                .into());
            }
            if a == 255 && pixel.0[..3] != [255, 255, 255] {
                return Err(format!(
                    "{} 的实体像素不是纯白（{:?}）：颜色由 shadow_tint 决定，烘进 PNG 就有了\
                     第二份暗度",
                    shadow.texture, pixel.0
                )
                .into());
            }
        }
        // ---- 落点独立复算（`tools/set_cast_shadows.py` 的第二份实现，故意不共享代码）----
        let body = image_of(&project, &base.texture)?;
        let (row, contact_x, span) = contact_of(&body)?;
        let total = f64::from(object_height(&project, base)?);
        let reach = reach_unit * total;
        if reach < min_reach {
            return Err(format!(
                "{} 的本体 {} 只有 {total:.0} 单位高（reach={reach:.2} < {min_reach}），\
                 按 lighting 的体量门槛不配投影：小物件各配一片影子会读成一地污渍",
                shadow.name, base.name
            )
            .into());
        }
        let (dx, dy) = (dir_x * reach, dir_y * reach);
        let hx = half_up(dx.abs() / 2.0);
        let hy = half_up(dy.abs() / 2.0);
        let half_span = i64::from(span / 2);
        let rx = i64::from((shadow.width - 1) / 2);
        let ry = i64::from((shadow.height - 1) / 2);
        if rx != half_span + hx || ry != half_up(half_span as f64 * flatten) + hy {
            return Err(format!(
                "{} 的半轴 ({rx},{ry}) 与复算值 ({},{}) 不同（本体落地跨 {span}、\
                 半偏移 ({hx},{hy})、flatten {flatten}）：贴片形状不再由 sun 与接触椭圆决定",
                shadow.name,
                half_span + hx,
                half_up(half_span as f64 * flatten) + hy
            )
            .into());
        }
        let foot_x = half_up(f64::from(base.x) + contact_x);
        let foot_y = half_up(f64::from(base.y) + f64::from(body.height() - 1 - row));
        let want_x = foot_x + if dx >= 0.0 { hx } else { -hx } - rx;
        let want_y = foot_y + if dy >= 0.0 { hy } else { -hy } - ry;
        if (f64::from(shadow.x) - want_x as f64).abs() > 0.001
            || (f64::from(shadow.y) - want_y as f64).abs() > 0.001
        {
            return Err(format!(
                "{} 落在 ({},{})，按接触点 ({foot_x},{foot_y}) + 半偏移复算应为 \
                 ({want_x},{want_y})：盘上的影子与光向已经分家",
                shadow.name, shadow.x, shadow.y
            )
            .into());
        }
        // 角度量的是**画面上**的两个点：贴片正中那个 texel 与本体接触点。
        let off_x = f64::from(shadow.x) + rx as f64 - foot_x as f64;
        let off_y = f64::from(shadow.y) + ry as f64 - foot_y as f64;
        angles.push(off_y.atan2(off_x).to_degrees());
    }
    // 门槛的另一半：够格的本体一件都不许漏（漏了就是"这棵树没影子"，不会报错）。
    let mut missing: Vec<String> = Vec::new();
    for item in &sprites {
        if item.layer != BASE_LAYER || item.role == SHADOW_ROLE || shadowed.contains(&item.name) {
            continue;
        }
        if !item.texture.starts_with(BUILDING_DIR) && !item.texture.starts_with(PROP_DIR) {
            continue;
        }
        let total = f64::from(object_height(&project, item)?);
        let reach = reach_unit * total;
        if reach >= min_reach {
            missing.push(format!(
                "{}({total:.0} 单位高, reach={reach:.2})",
                item.name
            ));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "这些站在地面上的物件够格（reach ≥ {min_reach}）却没有投影：{missing:?}"
        )
        .into());
    }
    let spread = angles
        .iter()
        .fold(0.0f64, |acc, a| acc.max((a - want_angle).abs()));
    if spread > 1e-9 {
        return Err(format!(
            "{} 件投影里最大角度偏离 sun({want_angle:.4}°) 达 {spread:.6}°：一批图里出现了两个\
             光向（docs/53 §3.9），症状是这棵树往右下倒、那棵树往左下倒",
            angles.len()
        )
        .into());
    }
    println!(
        "  {} 件投影：层/order/tint/尺寸/纯白逐件与 lighting 同源，够格而未挂影 {} 件；\
         角度全部精确等于 sun（{want_angle:.4}°，最大偏离 {spread:.6}°）",
        shadows.len(),
        missing.len()
    );

    // 运行期：投影恒画在人之前（影子不会把人刷黑）。挑一件本帧真的在画的贴片。
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let player_draw = player_draw_index(&mesh)?;
    let mut nearest: Option<(&Placed, usize, f64)> = None;
    for shadow in &shadows {
        let Some(index) = draw_indices(&mesh, &shadow.texture)?.into_iter().min() else {
            continue;
        };
        let distance = ((f64::from(shadow.x) - f64::from(deep_x)).powi(2)
            + (f64::from(shadow.y) - f64::from(deep_y)).powi(2))
        .sqrt();
        if nearest.map_or(true, |(_, _, best)| distance < best) {
            nearest = Some((shadow, index, distance));
        }
    }
    let (shadow, shadow_draw, distance) =
        nearest.ok_or_else(|| -> Box<dyn std::error::Error> {
            "玩家站的位置上一件投影都没画出来：下面那条\"影子不压人\"的断言没有主体".into()
        })?;
    if shadow_draw >= player_draw {
        return Err(format!(
            "{} 的分段下标 {shadow_draw} ≥ 玩家的 {player_draw}：影子排在人之后画，\
             玩家会被一片白椭圆刷黑",
            shadow.name
        )
        .into());
    }
    println!(
        "  绘制序：离玩家最近的是 {}（{distance:.0} 单位），分段下标 {shadow_draw} < 玩家 \
         {player_draw}（影子在人之前画）",
        shadow.name
    );
    // 反面对照：把这一件抬进 foreground，方向必须翻过来。不翻就说明 §⑥ 上面那条
    // 层带断言其实没在看渲染器的决定。
    let flipped = shadow
        .body
        .replace("sorting_layer=world", "sorting_layer=foreground");
    lua_run(
        &mut runtime,
        &format!("sprite.set(entity.find('{}'), '{flipped}')", shadow.name),
    )?;
    for _ in 0..2 {
        frame(&mut runtime)?;
    }
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let player_draw = player_draw_index(&mesh)?;
    let moved = draw_indices(&mesh, &shadow.texture)?
        .into_iter()
        .max()
        .unwrap_or(0);
    if moved <= player_draw {
        return Err(format!(
            "反面对照失手：{} 改到 foreground 后分段下标 {moved} 仍然 ≤ 玩家的 {player_draw}",
            shadow.name
        )
        .into());
    }
    lua_run(
        &mut runtime,
        &format!(
            "sprite.set(entity.find('{}'), '{}')",
            shadow.name, shadow.body
        ),
    )?;
    for _ in 0..2 {
        frame(&mut runtime)?;
    }
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let back = draw_indices(&mesh, &shadow.texture)?
        .into_iter()
        .min()
        .unwrap_or(usize::MAX);
    let player_draw = player_draw_index(&mesh)?;
    if back >= player_draw {
        return Err(format!(
            "还原后 {} 分段下标 {back} ≥ 玩家 {player_draw}：还原没生效",
            shadow.name
        )
        .into());
    }
    println!(
        "  反面对照：抬进 foreground 后下标 {moved} > 玩家，还原后 {back} < 玩家（层带真的在决定这件事）"
    );

    banner("⑦ 脚下接触阴影：8 个朝向 × 每帧，椭圆必须恒在身体中心的另一侧（镜像不许翻边）");
    let shift_x = query_num(&mut lua, "require('data.lighting').actor_shadow.shift_x")?;
    let shade = (
        query_num(&mut lua, "require('data.lighting').actor_shadow.color.r")? as u8,
        query_num(&mut lua, "require('data.lighting').actor_shadow.color.g")? as u8,
        query_num(&mut lua, "require('data.lighting').actor_shadow.color.b")? as u8,
        query_num(&mut lua, "require('data.lighting').actor_shadow.alpha")? as u8,
    );
    // shift_x = 0 时"两侧"没有区别，这条断言就成了永真式——先把它堵掉再往下量。
    if shift_x <= 0.0 {
        return Err(format!(
            "actor_shadow.shift_x = {shift_x}：脚下阴影正落在身体中心，§⑦ 的\"哪一侧\"就没有\
             可测的了（C-1 要消灭的正是镜像帧翻到另一侧）"
        )
        .into());
    }
    let mut frames_checked = 0usize;
    let mut centroids: Vec<f64> = Vec::new();
    for facing in FACINGS {
        let path = format!("{PLAYER_DIR}idle_{facing}.png");
        let strip = image_of(&project, &path)?;
        if strip.width() % (PLAYER_HALF as u32 * 2) != 0 {
            return Err(format!(
                "{path} 宽 {} 切不出整数帧（一帧 {} 宽）",
                strip.width(),
                PLAYER_HALF * 2
            )
            .into());
        }
        let frame_w = PLAYER_HALF as u32 * 2;
        for index in 0..strip.width() / frame_w {
            let mut weight = 0.0f64;
            let mut sum = 0.0f64;
            let mut bottom = 0u32;
            for row in 0..strip.height() {
                for col in 0..frame_w {
                    let pixel = strip.get_pixel(index * frame_w + col, row).0;
                    if pixel != [shade.0, shade.1, shade.2, shade.3] {
                        continue;
                    }
                    weight += 1.0;
                    sum += f64::from(col);
                    bottom = bottom.max(row);
                }
            }
            if weight == 0.0 {
                return Err(format!(
                    "{path} 第 {index} 帧一个 {shade:?} 的阴影像素都没有：脚下投影被烘进身体之前\
                     先丢了（`stamp_shadow` 没跑到，或者颜色/透明度与 lighting.actor_shadow 分家了）"
                )
                .into());
            }
            let centroid = sum / weight;
            // 判据是**符号**，不是数值：椭圆被身体裁掉一半，质心会跑到 +6 附近而不是 +2，
            // 而"镜像帧翻到另一侧"的那个 bug 表现为质心 < 中心。所以这里断言
            // 「在中心右侧、且偏移不小于 shift_x」，再要求 8 个朝向的读数彼此接近。
            if centroid <= f64::from(PLAYER_HALF) || centroid - f64::from(PLAYER_HALF) < shift_x {
                return Err(format!(
                    "{path} 第 {index} 帧的接触阴影质心在 x={centroid:.2}（帧中心 {}），\
                     不在 +{shift_x} 那一侧：这一帧的影子朝光长了（docs/53 §3.9）",
                    PLAYER_HALF
                )
                .into());
            }
            if bottom != strip.height() - 1 {
                return Err(format!(
                    "{path} 第 {index} 帧的阴影最低行是 {bottom}，不在帧底 {}：脚不沾地的影子\
                     读成一团雾",
                    strip.height() - 1
                )
                .into());
            }
            frames_checked += 1;
            centroids.push(centroid);
        }
    }
    let lo = centroids.iter().fold(f64::INFINITY, |a, &b| a.min(b));
    let hi = centroids.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    if hi - lo > 1.0 {
        return Err(format!(
            "8 个朝向的脚下阴影质心从 {lo:.2} 散到 {hi:.2}（跨度 {:.2} > 1 texel）：\
             一批图只能有一个光向，脚下阴影也一样",
            hi - lo
        )
        .into());
    }
    println!(
        "  {frames_checked} 帧（8 朝向 × {frames} 帧条带）质心全部在中心右侧 {lo:.2}..{hi:.2}\
         （跨度 {:.2} texel，lighting 给的 shift_x={shift_x}）；若把阴影烘进身体再镜像，\
         w/sw/nw 三族会读成 {:?}，正是 C-1 修掉的那一处",
        hi - lo,
        f64::from(PLAYER_HALF) * 2.0 - hi
    );

    banner("⑧ Y 排序容差：容差就是 depth.lua 的 STEP，差不到一桶不许翻、差一桶必须翻");
    // 事实源在 `scripts/world/depth.lua`（那条规则的三条口径都写在它的注释里），门这边
    // 与 `jhyt_m35_depth_sort` 同一个约定：手抄 STEP，两处一起改。
    const STEP: f64 = 2.0;
    // 落点取在桶的**正中间**（+0.5），这样 ±0.4 以内的物理漂移都不会跨过桶边界；
    // 而 +STEP 落在下一个桶的同一相位上，差值才是被 STEP 决定的，不是被边界决定的。
    let base_y = f64::from(deep_y) + 0.5;
    let mut buckets: Vec<(f64, i32)> = Vec::new();
    for delta in [0.0, STEP - 1.0, STEP] {
        let want = base_y + delta;
        lua_run(
            &mut runtime,
            &format!("require('player.player').teleport({deep_x}, {want})"),
        )?;
        for _ in 0..SETTLE_FRAMES {
            frame(&mut runtime)?;
        }
        let held = runtime_lua(
            &mut runtime,
            &format!(
                "local t = transform.get(entity.find('{PLAYER}'))\n\
                 local s = sprite.get(entity.find('{PLAYER}'))\n\
                 return string.format('%0.3f|%d', t.y, s.order)"
            ),
        )?;
        let (y, order) = held
            .split_once('|')
            .ok_or_else(|| format!("落位读数 {held:?} 不是 `y|order`"))?;
        let y: f64 = y.parse()?;
        let order: i32 = order.parse()?;
        if (y - want).abs() > 0.4 {
            return Err(format!(
                "传送目标 y={want}，8 帧后停在 {y}（容差 0.4）：这一档已经贴着桶边界，\
                 下面的桶宽断言量的就不再是 STEP"
            )
            .into());
        }
        buckets.push((y, order));
    }
    let (y0, o0) = buckets[0];
    let (_, o1) = buckets[1];
    let (_, o2) = buckets[2];
    if o0 != o1 {
        return Err(format!(
            "Δy = {:.1}（< STEP {STEP}）的两个位置落进了不同 order 桶（{o0} vs {o1}）：\
             桶宽比声明的小，两个只差 1 单位的角色会被硬性判出前后（症状是走过树旁时\
             抖一下），y0={y0}",
            STEP - 1.0
        )
        .into());
    }
    if o2 >= o0 {
        return Err(format!(
            "Δy = {STEP} 之后 order 从 {o0} 变成 {o2}（应当更小）：世界 +y 朝上 ⇒ y 越小越靠前\
             ⇒ order 必须随 y **单调递减**。写成递增不会报错，症状是\"主角站在 NPC 下面反而\
             被 NPC 盖住\"（`jhyt_m35_depth_sort` 的头号反例）"
        )
        .into());
    }
    if o0 - o2 != 1 {
        return Err(format!(
            "Δy = {STEP} 让 order 从 {o0} 跳到 {o2}（应当只差 1 桶）：桶宽与 STEP 不一致"
        )
        .into());
    }
    println!(
        "  实测桶宽 = STEP：Δy=1 → order 不变（{o0}），Δy={STEP} → 恰好翻一桶（{o0}→{o2}），\
         方向是 y 越大越靠后；静态物件不参与 Y 折（层秩 world < actor），压人只走 foreground\
         ——见 scripts/world/depth.lua 的容差三条"
    );
    lua_run(
        &mut runtime,
        &format!("require('player.player').teleport({deep_x}, {deep_y})"),
    )?;
    for _ in 0..SETTLE_FRAMES {
        frame(&mut runtime)?;
    }

    println!(
        "\njhyt_m72_foreground_occlusion_ok lighting 事实源干净、登记在册、4 条对照点名；\
         occluders={} worst={worst:.3} bound={max_cover} frames={frames} deepest=({},{}@{}) \
         shadows={} 角度方差={spread:.6}° 脚下阴影={frames_checked}帧 桶宽=STEP",
        occluders.len(),
        occ.name,
        deep_x,
        deep_y,
        shadows.len(),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// `.scene` 文本读取：门自己解析一遍，与运行期 mesh 互为对照
// ---------------------------------------------------------------------------

fn read_scene(path: &Path) -> Result<Vec<Placed>, Box<dyn std::error::Error>> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut names: BTreeMap<i64, String> = BTreeMap::new();
    let mut sprite_of: BTreeMap<i64, String> = BTreeMap::new();
    let mut transform_of: BTreeMap<i64, String> = BTreeMap::new();
    let mut role_of: BTreeMap<i64, String> = BTreeMap::new();
    let mut collider: Vec<i64> = Vec::new();
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
                sprite_of.insert(id, field[3].to_owned());
            }
            "component" if field[2] == "Transform" => {
                transform_of.insert(id, field[3].to_owned());
            }
            "component" if field[2] == "Collider" => {
                collider.push(id);
            }
            "entity_meta" if field[2] == "role" && field.len() > 3 => {
                role_of.insert(id, field[3].to_owned());
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (id, name) in &names {
        let Some(body) = sprite_of.get(id) else {
            continue;
        };
        let Some(transform) = transform_of.get(id) else {
            continue;
        };
        let fields = parse_fields(body);
        let position = parse_fields(transform)
            .get("position")
            .cloned()
            .ok_or_else(|| format!("{name} 的 Transform 没有 position 字段"))?;
        let mut axis = position.split(',');
        let x: f32 = axis.next().unwrap_or("0").parse()?;
        let y: f32 = axis.next().unwrap_or("0").parse()?;
        let size = fields
            .get("size")
            .ok_or_else(|| format!("{name} 的 Sprite 没有 size 字段"))?;
        let mut wh = size.split('x');
        let width: u32 = wh.next().unwrap_or("0").parse()?;
        let height: u32 = wh.next().unwrap_or("0").parse()?;
        out.push(Placed {
            name: name.clone(),
            body: body.clone(),
            texture: fields.get("texture").cloned().unwrap_or_default(),
            width,
            height,
            order: fields
                .get("order")
                .and_then(|value| value.parse::<f32>().ok())
                .map_or(0, |value| value as i32),
            layer: fields.get("sorting_layer").cloned().unwrap_or_default(),
            x,
            y,
            collider: collider.iter().any(|item| item == id),
            tint: fields.get("tint").cloned().unwrap_or_default(),
            role: role_of.get(id).cloned().unwrap_or_default(),
        });
    }
    Ok(out)
}

fn parse_fields(value: &str) -> BTreeMap<String, String> {
    value
        .split(';')
        .filter_map(|part| part.split_once('='))
        .map(|(key, item)| (key.to_owned(), item.to_owned()))
        .collect()
}

fn find<'a>(sprites: &'a [Placed], name: &str) -> Result<&'a Placed, Box<dyn std::error::Error>> {
    sprites
        .iter()
        .find(|item| item.name == name)
        .ok_or_else(|| {
            format!("{SCENE} 里没有实体 {name}：拆层表引用了一个不存在的本体/遮断物").into()
        })
}

// ---------------------------------------------------------------------------
// PNG：全部按 RGBA8 读，alpha > 0 就算"这个 texel 会画东西"
// ---------------------------------------------------------------------------

fn image_of(project: &Path, rel: &str) -> Result<RgbaImage, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(project.join(rel))
        .map_err(|error| format!("{rel} 读不到（{error}）：拆层块不在盘上，运行期只会静默不画"))?;
    image::load_from_memory(&bytes)
        .map(|image| image.to_rgba8())
        .map_err(|error| format!("{rel}: {error}").into())
}

fn rgba(image: &RgbaImage) -> Vec<u8> {
    image.as_raw().clone()
}

/// 四舍五入，`.5` **一律进位**——与 `tools/set_cast_shadows.py::half_up` 同一条式子。
/// Python 内建的 `round()` 是银行家舍入（`round(8.5)==8`），而 Rust 的 `f64::round()` 进位；
/// 贴片落点有三处会正好踩在 `.5` 上（半偏移、接触行中心、半跨×flatten），两边各用各的
/// 就会让门红在"1 像素"上，而那看起来像"光向算错了"。
fn half_up(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// 落地行与落地宽：位置取**最底不透明行**，宽度取**底部 25% 行里的最大跨度**。
/// 为什么是两件事（与工具侧 `contact_of` 同规则、两份实现）：正房本体图最底一行是台阶
/// （跨 45），它的体量却是台基（跨 200），只按最底行走投影会缩成台阶底下一小坨；反过来
/// 树那种上宽下窄的剪影不能往上找"够宽的行"，那里已经是树冠切面，拿它当落地宽等于把
/// 树冠印在地上。
///
/// 返回 `(图像行, 接触点中心 x, 落地跨度)`。
fn contact_of(image: &RgbaImage) -> Result<(u32, f64, u32), Box<dyn std::error::Error>> {
    let width = image.width();
    let height = image.height();
    let row_span = |row: u32| -> Option<(u32, u32)> {
        let mut first: Option<u32> = None;
        let mut last = 0u32;
        for col in 0..width {
            if image.get_pixel(col, row).0[3] == 0 {
                continue;
            }
            if first.is_none() {
                first = Some(col);
            }
            last = col;
        }
        first.map(|start| (start, last))
    };
    let mut bottom: Option<(u32, u32, u32)> = None;
    let mut rows: Vec<(u32, u32, u32)> = Vec::new();
    let mut row = height;
    while row > 0 {
        row -= 1;
        let Some((x0, x1)) = row_span(row) else {
            continue;
        };
        if bottom.is_none() {
            bottom = Some((row, x0, x1));
        }
        rows.push((row, x0, x1));
        if rows.len() >= std::cmp::max(2, (height / 4) as usize) {
            break;
        }
    }
    let (row, x0, x1) = bottom.ok_or_else(|| -> Box<dyn std::error::Error> {
        "整张图全透明，没有落地行可取：投影按接触椭圆算，接触点都没有就没有落点".into()
    })?;
    let widest = rows
        .into_iter()
        .max_by_key(|&(_, a, b)| b - a)
        .unwrap_or((row, x0, x1));
    Ok((row, f64::from(x0 + x1) / 2.0, widest.2 - widest.1 + 1))
}

/// 物体**总高**（投影长度按它算，不是按本体块）：拆层件的本体块只是下半张，整张才是
/// 站在地面上的那个物体的全高；其余物件用自身贴图的实测高（不抄声明，见 §⑥ 的软边断言）。
fn object_height(project: &Path, item: &Placed) -> Result<u32, Box<dyn std::error::Error>> {
    if let Some(row) = SPLITS.iter().find(|row| row.base == item.name) {
        return Ok(image_of(project, row.full)?.height());
    }
    Ok(image_of(project, &item.texture)?.height())
}

/// 一张图的不透明像素，按图像行展开（索引 `row*w + col`）。
/// 口径：`alpha > 0`。角色帧底部的接触阴影椭圆（alpha=70）因此**算角色像素**——它是 sprite
/// 自己的一部分，被树冠盖住时也该计入"人被盖住多少"，把它排除掉会让占比虚高在另一个方向上。
fn opaque_rows(image: &RgbaImage) -> Vec<bool> {
    let mut on = Vec::with_capacity((image.width() * image.height()) as usize);
    for pixel in image.as_raw().chunks_exact(4) {
        on.push(pixel[3] > 0);
    }
    on
}

/// 两块**按行拼回去**（上半张在下半张之前，图像行序），逐字节含 alpha。
fn stack(upper: &RgbaImage, lower: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity((upper.width() * (upper.height() + lower.height())) as usize);
    for image in [upper, lower] {
        for row in 0..image.height() {
            for col in 0..image.width() {
                let pixel = image.get_pixel(col, row).0;
                out.extend_from_slice(&pixel);
            }
        }
    }
    out
}

impl Occluder {
    /// 世界栅格 (gx,gy) 这一格有没有被这块前景画上东西。
    fn covers(&self, gx: i32, gy: i32) -> bool {
        let col = gx - self.x;
        let row = self.y + self.h as i32 - 1 - gy;
        if col < 0 || row < 0 || col >= self.w as i32 || row >= self.h as i32 {
            return false;
        }
        self.on[row as usize * self.w + col as usize]
    }
}

// ---------------------------------------------------------------------------
// 覆盖率扫描
// ---------------------------------------------------------------------------

/// 一条角色帧的不透明像素，写成相对**中心（= Transform）**的整数偏移。
/// 1 texel = 1 世界单位、坐标取整 ⇒ 两块可以直接按整数栅格比对，不需要插值。
/// 像素 (col,row) → 世界 (cx-32+col, cy+32-row-1) ⇒ 偏移 (col-32, 31-row)。
fn frame_masks(
    image: &RgbaImage,
    frames: usize,
) -> Result<Vec<Vec<(i32, i32)>>, Box<dyn std::error::Error>> {
    if frames == 0 || image.width() % frames as u32 != 0 {
        return Err(format!(
            "角色条带 {}x{} 切不出 {frames} 帧：UV 读回来的帧数与图宽不成整除",
            image.width(),
            image.height()
        )
        .into());
    }
    let strip_w = (image.width() / frames as u32) as usize;
    let mut out = Vec::with_capacity(frames);
    for frame_index in 0..frames {
        let mut offsets = Vec::new();
        for row in 0..image.height() as usize {
            for col in 0..strip_w {
                let pixel = image
                    .get_pixel((frame_index * strip_w + col) as u32, row as u32)
                    .0;
                if pixel[3] > 0 {
                    offsets.push((col as i32 - PLAYER_HALF, row as i32));
                }
            }
        }
        if offsets.is_empty() {
            return Err(format!(
                "条带第 {frame_index} 帧一个不透明像素都没有：那一帧是空白，覆盖率会算成 0/0"
            )
            .into());
        }
        out.push(offsets);
    }
    Ok(out)
}

/// 玩家 ball(r=12) 在 (cx,cy) 是否**不压进任何 solid 格**。
/// solid 是关卡的可行走事实源：`data/places.lua` 的出生点、`tilemap.find_path` 的寻路、
/// `systems/building.lua` 的占地判定读的都是这一张表，门跟着它们走，不自造一套。
/// 这一张**同时是物理世界**：`scene_spawn.rs` 的 `solid_cell_colliders` 给每个实心格在
/// GroundMap 的静态刚体上挂一个 cuboid（见上面那条逐格探针，359/359 命中、空格 0 命中）。
/// 门的判据比物理严一点——"碰到格边"就算挡住，而 Rapier 只禁穿透——所以门的可达集是
/// 物理允许位置的**子集**，保守方向，不会把进不去的位置算进来。真正让两者对齐的是 §④
/// 末尾那条 teleport 落位断言；"全图理论上界"那一列则把位图之外的位置一并扫过，证明
/// 上界不是靠漏看来达标的。
fn reachable(cx: i32, cy: i32, grid: &SolidGrid) -> bool {
    let min_col = (cx - PLAYER_RADIUS as i32).div_euclid(TILE);
    let max_col = (cx + PLAYER_RADIUS as i32).div_euclid(TILE);
    let min_row = (cy - PLAYER_RADIUS as i32).div_euclid(TILE);
    let max_row = (cy + PLAYER_RADIUS as i32).div_euclid(TILE);
    for col in min_col..=max_col {
        for row in min_row..=max_row {
            if col < 0 || row < 0 || col as usize >= grid.cols || row as usize >= grid.rows {
                continue;
            }
            if !grid.cells[row as usize * grid.cols + col as usize] {
                continue;
            }
            // 圆 vs AABB：取最近点，距离 ≤ r 就是压进去了。
            let near_x = (cx.max(col * TILE)).min(col * TILE + TILE - 1);
            let near_y = (cy.max(row * TILE)).min(row * TILE + TILE - 1);
            let dx = cx - near_x;
            let dy = cy - near_y;
            if dx * dx + dy * dy <= (PLAYER_RADIUS * PLAYER_RADIUS) as i32 {
                return false;
            }
        }
    }
    true
}

/// 一块前景在一帧下的扫描结果。
#[derive(Clone, Copy, Default)]
struct Sweep {
    /// 可达位置里的最大覆盖率（判据用它）。
    reachable_ratio: f64,
    x: i32,
    y: i32,
    /// 不加可行走限制的理论上界——只报不断，见 `reachable` 的注释。
    any_ratio: f64,
}

/// 扫一块前景的所有候选位置：角色 64×64 与该块矩形有交集的全部整点（步进 {SWEEP_STEP}）。
fn sweep_occ(occ: &Occluder, offsets: &[(i32, i32)], grid: &SolidGrid) -> Sweep {
    let total = offsets.len() as f64;
    let x_lo = occ.x - PLAYER_HALF + 1;
    let x_hi = occ.x + occ.w as i32 + PLAYER_HALF - 1;
    let y_lo = occ.y - PLAYER_HALF + 1;
    let y_hi = occ.y + occ.h as i32 + PLAYER_HALF - 1;
    // 网格锚在 0 的整数倍上：换一块树不会让候选点跟着平移，两次扫描的判据才可比。
    let start_x = x_lo - x_lo.rem_euclid(SWEEP_STEP);
    let start_y = y_lo - y_lo.rem_euclid(SWEEP_STEP);
    let mut best = Sweep::default();
    let mut cy = start_y;
    while cy <= y_hi {
        let mut cx = start_x;
        while cx <= x_hi {
            // 快速排除：两个矩形不相交时覆盖率必为 0，不必走一遍像素。
            if cx + PLAYER_HALF - 1 >= occ.x
                && cx - PLAYER_HALF <= occ.x + occ.w as i32 - 1
                && cy + PLAYER_HALF - 1 >= occ.y
                && cy - PLAYER_HALF <= occ.y + occ.h as i32 - 1
            {
                let mut hits = 0usize;
                for (dx, dy) in offsets {
                    if occ.covers(cx + dx, cy + PLAYER_HALF - 1 - dy) {
                        hits += 1;
                    }
                }
                let ratio = hits as f64 / total;
                if ratio > best.any_ratio {
                    best.any_ratio = ratio;
                }
                if ratio > best.reachable_ratio && reachable(cx, cy, grid) {
                    best.reachable_ratio = ratio;
                    best.x = cx;
                    best.y = cy;
                }
            }
            cx += SWEEP_STEP;
        }
        cy += SWEEP_STEP;
    }
    best
}

fn ground_solid(runtime: &mut RuntimeScene) -> Result<SolidGrid, Box<dyn std::error::Error>> {
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
        return Err("运行期找不到 GroundMap：可行走事实源不存在，覆盖率无从判起".into());
    }
    let mut parts = text.splitn(3, ',');
    let cols: usize = parts.next().unwrap_or("0").parse()?;
    let rows: usize = parts.next().unwrap_or("0").parse()?;
    let run = parts.next().unwrap_or("");
    if run.len() != cols * rows || !run.bytes().all(|b| b == b'0' || b == b'1') {
        return Err(format!(
            "tilemap.is_solid 回读到的串长 {}，应为 {cols}x{rows} 的 0/1：可行走位图取歪了",
            run.len()
        )
        .into());
    }
    let blocked = run.bytes().filter(|b| *b == b'1').count();
    println!(
        "  GroundMap 可行走位图取自运行期 tilemap.is_solid：{cols}x{rows}，实心 {blocked} 格\
         （门自己数一遍，与 `tools/gen_manor_scene.py` 的可达性契约同源）"
    );
    Ok(SolidGrid {
        cols,
        rows,
        cells: run.bytes().map(|b| b == b'1').collect(),
    })
}

struct SolidGrid {
    cols: usize,
    rows: usize,
    cells: Vec<bool>,
}

impl SolidGrid {
    /// 世界坐标落在哪一格、那一格是不是实心（越界回 `None`，与 §④ 的换算同一个式子）。
    fn blocked_at(&self, x: i32, y: i32) -> Option<bool> {
        let col = x.div_euclid(TILE) as usize;
        let row = y.div_euclid(TILE) as usize;
        if col >= self.cols || row >= self.rows {
            return None;
        }
        Some(self.cells[row * self.cols + col])
    }
}

/// `solid` 与物理世界的关系：全图逐格打一发自 r=4 的圆，按"打到了谁"分类。
///
/// 这一条是**自我纠错**的产物：上一版的门（和 `lighting.lua` 的措辞）都写着"本项目没把
/// `solid` 做成碰撞体"，凭的是 grep 场景文件"只有一行 Collider"。探针一跑就打脸了——
/// `scene_spawn.rs` 的 `solid_cell_colliders` 会给**每个实心格**在 tilemap 实体的静态刚体上
/// 挂一个 cuboid（偏移 `(col·tile, row·tile) + 半格`），运行期 `tilemap.set_solid` 还会
/// `refresh_tilemap_physics` 重建。所以这里的断言按事实反过来写，一次钉住三件事：
///   * 门的可行走位图**就是**物理世界：实心格 100% 有地表碰撞体、空格 0 个 ⇒ §④ 的"可达"
///     不是几何想象，是玩家真能站的地方（那条 teleport 落位断言因此有了牙）；
///   * 换算方向与偏移：探针坐标用 `col·TILE+16, row·TILE+16`，与引擎同一式子。行序若反，
///     实心格命中率会塌下来；
///   * r=4 远小于半格 16 ⇒ 一发只落在正下方那一格，命中数按格可加。
/// 顺带解释 `jhyt_m72_scatter_decor` §⑤ 那个"命中别物 16 次（地表 tilemap 的实心格）"：
/// 那些命中就是这些按格生成的碰撞体（GroundMap 没有 Sprite 组件，所以那边标成 no-sprite）。
fn solid_physics_probe(
    runtime: &mut RuntimeScene,
    grid: &SolidGrid,
) -> Result<(), Box<dyn std::error::Error>> {
    let report = lua_str(
        runtime,
        &format!(
            r#"(function()
                 local map = entity.find('GroundMap')
                 local player = entity.find('{PLAYER}')
                 if not map or map == 0 or not player or player == 0 then return 'nomap 0 0 0 0 none' end
                 local solid_total, exact, mine, other, phantom = 0, 0, 0, 0, 0
                 local seen = {{}}
                 for row = 0, {rows} - 1 do
                   for col = 0, {cols} - 1 do
                     local hits = physics.overlap(col * {tile} + {half}, row * {tile} + {half}, 4)
                     local on_map = 0
                     for _, o in ipairs(hits) do
                       if o == map then
                         on_map = on_map + 1
                       elseif o == player then
                         mine = mine + 1
                       else
                         other = other + 1
                         local label = 'no-sprite#' .. tostring(o)
                         local ok, s = pcall(sprite.get, o)
                         if ok and s and s.texture then label = tostring(s.texture) end
                         seen[label] = (seen[label] or 0) + 1
                       end
                     end
                     if tilemap.is_solid(map, col, row) then
                       solid_total = solid_total + 1
                       if #hits == 1 and on_map == 1 then exact = exact + 1 end
                     elseif on_map > 0 then
                       phantom = phantom + 1
                     end
                   end
                 end
                 local keys = {{}}
                 for k in pairs(seen) do keys[#keys + 1] = k end
                 table.sort(keys)
                 local labels = 'none'
                 if #keys > 0 then
                   local parts = {{}}
                   for _, k in ipairs(keys) do parts[#parts + 1] = k .. 'x' .. seen[k] end
                   labels = table.concat(parts, ',')
                 end
                 return string.format('%d %d %d %d %d %s', solid_total, exact, mine, other, phantom, labels)
               end)()"#,
            rows = grid.rows,
            cols = grid.cols,
            tile = TILE,
            half = TILE / 2,
        ),
    )?;
    let fields: Vec<&str> = report.split_whitespace().collect();
    let [solid_total, exact, mine, other, phantom, labels] =
        <[&str; 6]>::try_from(fields.as_slice())
            .map_err(|_| format!("物理探针读不回 6 个字段：{report:?}"))?;
    let solid_total: usize = solid_total.parse()?;
    let exact: usize = exact.parse()?;
    let mine: usize = mine.parse()?;
    let other: usize = other.parse()?;
    let phantom: usize = phantom.parse()?;
    let blocked = grid.cells.iter().filter(|cell| **cell).count();
    let open = grid.cols * grid.rows - blocked;
    if solid_total != blocked {
        return Err(format!(
            "探针按 Lua 的 `tilemap.is_solid` 数出 {solid_total} 个实心格，门按 §④ 位图数出 \
             {blocked} 个：同一次运行期读两遍不一致，后面的覆盖率是在一张对不上的位图上量的"
        )
        .into());
    }
    if exact != solid_total {
        return Err(format!(
            "只有 {exact}/{solid_total} 个实心格的中心恰好取到 1 个 GroundMap 碰撞体：\
             `solid` 与物理世界脱钩了（多半是换算方向或格偏移与 `solid_cell_colliders` 不一致），\
             §④ 的「可达」不再等于玩家真能站的地方"
        )
        .into());
    }
    if phantom != 0 {
        return Err(format!(
            "{phantom} 个**空格**上有地表碰撞体：物理比位图更严，门会把一些根本进不去的位置算成可达"
        )
        .into());
    }
    if other != 0 {
        return Err(format!(
            "{other} 次命中来自 GroundMap 与玩家之外的实体（{labels}）：地表之外还冒出了别的碰撞体，\
             上面那两条「位图 == 物理」的结论只覆盖了地表，需要重新分类"
        )
        .into());
    }
    println!(
        "  逐格探针（{cols}x{rows} = {} 格，各打一发自 r=4 的 physics.overlap）：{solid_total} 个实心格\
         每格恰好取到 1 个 GroundMap 碰撞体（{exact}/{solid_total}）、{open} 个空格取到 0 个\
         （反面对照）、玩家碰撞体被撞到 {mine} 次、别的实体 0 次 ⇒ `solid` **是**按格生成的静态\
         碰撞体（`scene_spawn.rs` 的 `solid_cell_colliders`，换算与探针同式），§④ 的「可达」\
         就是物理上能站的位置",
        grid.cols * grid.rows,
        cols = grid.cols,
        rows = grid.rows,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// mesh 读数
// ---------------------------------------------------------------------------

/// 玩家在 mesh 里用的条带路径与条带帧数。
///
/// 帧数**从渲染器给的 UV 反推**（跨度 × 图宽 = 一帧的像素宽），不抄 `Animation` 串里的
/// `clip:idle_s:…:4:6:true`：那条若写错成 5，门照样按 4 扫，等于没在看。反过来按 UV 取，
/// 扫的就是"这一帧真的画出去的那一条"。贴图路径同理——不假设是 `idle_s.png`，读到什么用什么。
fn player_strip(
    project: &Path,
    runtime: &mut RuntimeScene,
) -> Result<(String, usize), Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let mut hits: Vec<(String, usize)> = Vec::new();
    for draw in &mesh.draws {
        if draw.texture == 0 {
            continue;
        }
        let Some(path) = mesh.paths.get((draw.texture - 1) as usize) else {
            continue;
        };
        if path.starts_with(PLAYER_DIR) {
            hits.push((path.clone(), (draw.count as usize) / 6));
        }
    }
    let [(path, quads)] = hits.as_slice() else {
        return Err(format!(
            "玩家在 mesh 里应恰好落在一个分段、一个四边形，实际命中 {} 段（{}）：多出来的那份是\
             同一条 clip 被画了两遍，没有的那份是没画——两种情况下算出来的覆盖率都不是玩家看见的图",
            hits.len(),
            hits.iter()
                .map(|(p, q)| format!("{p}:{q}"))
                .collect::<Vec<_>>()
                .join(",")
        )
        .into());
    };
    if *quads != 1 {
        return Err(format!("{path} 有 {quads} 个四边形（应为 1）").into());
    }
    let mut span = (f32::MAX, f32::MIN);
    for draw in &mesh.draws {
        let same = draw.texture != 0
            && mesh
                .paths
                .get((draw.texture - 1) as usize)
                .is_some_and(|registered| registered == path);
        if !same {
            continue;
        }
        let start = draw.start as usize;
        let count = draw.count as usize;
        for vertex in &mesh.vertices[start..start + count] {
            span.0 = span.0.min(vertex.uv[0]);
            span.1 = span.1.max(vertex.uv[0]);
        }
    }
    if span.0 > span.1 {
        return Err(format!("{path} 的四边形没有 UV：门无法知道画的是条带里的哪一帧").into());
    }
    let sheet = image_of(project, path)?;
    let strip = ((span.1 - span.0) * sheet.width() as f32).round();
    if strip <= 0.0 || sheet.width() as f32 % strip != 0.0 {
        return Err(format!(
            "UV 跨度 {:.4} × 图宽 {} = {strip}，切不出整除的帧数：条带帧数是渲染器给的 UV 反推的，\
             对不上就是 UV 规则变了",
            span.1 - span.0,
            sheet.width()
        )
        .into());
    }
    Ok((path.clone(), (sheet.width() as f32 / strip) as usize))
}

/// 玩家的四边形落在哪个分段（同图可能只有玩家一个实体，仍要求命中数 ≥ 1）。
fn player_draw_index(mesh: &SceneMesh) -> Result<usize, Box<dyn std::error::Error>> {
    let mut hits: Vec<usize> = Vec::new();
    for (index, draw) in mesh.draws.iter().enumerate() {
        if draw.texture == 0 {
            continue;
        }
        if mesh
            .paths
            .get((draw.texture - 1) as usize)
            .is_some_and(|path| path.starts_with(PLAYER_DIR))
        {
            hits.push(index);
        }
    }
    hits.into_iter()
        .min()
        .ok_or("mesh 里没有玩家分段，绘制序无从比较".into())
}

/// 某张贴图在 `mesh.draws` 里的所有分段下标。返回**全部**而不是唯一：四片 canopy 共用一张图，
/// 批处理把它们合成几段是渲染器的自由，门要判的是"这些段是不是都排在玩家之后"。
fn draw_indices(mesh: &SceneMesh, texture: &str) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    let hits: Vec<usize> = mesh
        .draws
        .iter()
        .enumerate()
        .filter(|(_, draw)| {
            draw.texture != 0
                && mesh
                    .paths
                    .get((draw.texture - 1) as usize)
                    .is_some_and(|path| path == texture)
        })
        .map(|(index, _)| index)
        .collect();
    if hits.is_empty() {
        return Err(format!(
            "{texture} 在 mesh.draws 里一个分段都没有：这件遮断物根本没进帧\
             （贴图没解析、组件缺字段或被别的系统删掉），这种漏不会自己报错"
        )
        .into());
    }
    Ok(hits)
}

fn short(name: &str) -> &str {
    name.strip_prefix("ManorTree")
        .or_else(|| name.strip_prefix("ManorHouse"))
        .unwrap_or(name)
}

// ---------------------------------------------------------------------------
// 宿主脚手架
// ---------------------------------------------------------------------------

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

fn query_str(runtime: &mut LuaRuntime, block: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(block)?.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    Ok(query_str(runtime, &format!("return tostring({expr})"))?.parse()?)
}

/// 直接执行一段 Lua（带 `return` 的**语句块**，不像 `lua_str` 那样把参数塞进表达式）。
/// §④ 读玩家坐标要用 `local t = …` 这种多语句形式。
fn runtime_lua(
    runtime: &mut RuntimeScene,
    block: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let text = runtime
        .with_script(|script| script.execute_console(block))
        .map_err(|error| error.to_string())?;
    Ok(text.trim().to_owned())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}
