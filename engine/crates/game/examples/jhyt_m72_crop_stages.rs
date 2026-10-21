//! Headless acceptance drive for M72-08：**作物阶段图与农田三态**。
//!
//! 需求（`tasks/done/M72-08-作物阶段图与农田三态.md`）：29 种作物以前**共用同一张三列
//! 阶段图**（`manor_farm.png` 的苗期/生长/成熟），灵芝和稻谷在田里长得一模一样，
//! 而且只有 3 档——这是"画面比场景空更伤"的第一名。本门钉三件事：
//!   ① **每种作物有自己的阶段图**，且两两不是"换个色冒充另一种"；
//!   ② **农田三态**（犁地/湿润/干涸）真的分层画出来，湿润靠可撤的叠痕 + 顶点色，
//!      不是第三张全幅贴图；
//!   ③ **只有收成动**（4 帧抖动 + 闪光 @100ms），生长阶段切换保持静态。
//!
//! 为什么阶段图唯一性判在**图集条目**而不是单体文件：跑进游戏的是
//! `assets/art/crops/manor_crops.png` 里切出来的那一格，两张不同的源图被色阶表
//! 吸附成同一格才是真事故（docs/57 §1：图集才是交付物）。
//!
//! 章节：
//!   ① 资产与三方寻址：`assets/art/crops/` 存在且阶段图 ≥116 张；图集实测 8×29；
//!      122 个阶段条目**字节两两不同**且无一空槽；门内手抄的 (row, stages) ==
//!      `data/crop_art.lua` == `data/crops.lua` 的 `stages`。
//!      反面对照：把两种作物的同一阶段配成同一张，同一个检查函数必须报红。
//!   ② 三态：**贴图侧**先量几何与像素——场景 `atlas_cols` == `data/farm_soil.lua` 的列数
//!      == 盘上 PNG 宽 ÷ 32，两列土面**字节不同**（配成同一张的反面对照必须红），湿痕那一列
//!      真是半透明叠痕（满不透明像素 <10%，否则它等于第三张全幅土面，会把垄沟抹平）；
//!      再走运行期：未垦格土面为空；翻土 → `tilled`；浇水 → 湿痕层有内容 **且该格 tint 非白**；
//!      日结算撤痕 → `dry` + 湿痕回 0 + tint 回白（撤色必须当帧，否则下一种作物发蓝）。
//!   ③ 阶段推进：稻谷（4 档 / 4 天）与灵芝（5 档 / 7 天）各浇满全程，CropGrid 瓦值
//!      单调不减、起点是第 1 档、终点恰好是最后一档；**成熟那一天的 watered_days
//!      仍等于 growth_days**（数值零改动：本包只把渲染铺细，没动生长）。
//!      同一天两种作物的瓦值必须不同（专治"共图"）。
//!   ④ 收成动画：收一格稻谷后按帧采样 CropGrid，读到 成熟 → 抖 1 → 抖 2 → 抖 3 →
//!      落回真值 共 4 个抖动帧，相邻帧间隔 5–7 帧（=100ms ±1 帧，docs/51 §2.2）；
//!      同期活动特效数 >0（收成闪光），动画结束后地块按数据落回空土。
//!   ⑤ 产量锚点：真实收获得到的个数 == `data/crops.lua` 的 `yield`（含品质派生条目），
//!      与 M72-08 改图前的基线读数一致。
//!
//! 与 M72-07 的关系：本包动 `scenes/manor.scene` 的**只有一条 CropGrid 实体**（id 31，
//! 排在三张农田图之后、预留带 5001 之前），其余改动都在 tileset 与作物贴图。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_crop_stages [-- <game_jhyt-root>]

use rustlua_input::input::InputSnapshot;
use rustlua_scripting::script::LuaRuntime;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

const TILE: usize = 32;
/// 作物图集几何（= `terrain_ramp.json` 的 `crops.atlas`，也与场景里的声明对拍）。
const ATLAS_COLS: usize = 8;
const ATLAS_ROWS: usize = 29;
/// 每行前 5 槽是阶段图，后 3 槽是收成抖动帧（`crop_kit.py` 的行布局）。
const STAGE_SLOTS: usize = 5;
const SHAKE_SLOTS: usize = 3;
const CROPS_TEXTURE: &str = "assets/art/crops/manor_crops.png";
const CROPS_DIR: &str = "assets/art/crops";
/// 三态的两张土面贴图（列数由 `data/farm_soil.lua` 声明，场景与它同源于
/// `tools/gen_manor_scene.py`）。
const FARM_TEXTURE: &str = "assets/art/tilesets/manor/manor_farm.png";
const MOIST_TEXTURE: &str = "assets/art/tilesets/manor/manor_farm_moist.png";
/// 量化判据（plan §验收方式）：阶段图 0 → ≥116 张。
const MIN_STAGE_FILES: usize = 116;

/// 门内**手抄**的作物 → (图集行号, 阶段数)。故意不去解析 `crop_kit.py`：门的职责是
/// "生成的图、生成的寻址表、crops.lua 声明的档数必须互相对得上"，去读其中一份等于
/// 让那份表自己跟自己一致。三份飘红时门报的是飘掉的那一条。
/// 档数口径（docs/33 生长节奏 + plan §设计要点 2）：粮食 4、灵药 5、果树 3。
const CROPS: &[(&str, usize, usize)] = &[
    ("lingzhi", 1, 5),
    ("rice", 2, 4),
    ("radish", 3, 4),
    ("herb", 4, 5),
    ("polygonum", 5, 5),
    ("ginseng", 6, 5),
    ("mint", 7, 5),
    ("mulberry", 8, 4),
    ("pea", 9, 4),
    ("cucumber", 10, 4),
    ("mung_bean", 11, 4),
    ("prunella", 12, 5),
    ("eggplant", 13, 4),
    ("pumpkin", 14, 4),
    ("gouqi", 15, 5),
    ("cabbage", 16, 4),
    ("chrysanthemum", 17, 5),
    ("leek", 18, 4),
    ("winter_wheat", 19, 4),
    ("xueli", 20, 4),
    ("winter_shen", 21, 5),
    ("plum", 22, 5),
    ("winter_cabbage", 23, 4),
    ("evergreen", 24, 5),
    ("peach", 25, 3),
    ("mume", 26, 3),
    ("loquat", 27, 3),
    ("jujube", 28, 3),
    ("ginkgo", 29, 3),
];

/// `data/farm_soil.lua` 的两列土面（门内副本，与生成器同源声明）。
const SOIL_TILLED: f64 = 1.0;
const SOIL_DRY: f64 = 2.0;

/// 一块格子在三层上的读数。
#[derive(Debug, Clone, Copy)]
struct Cell {
    soil: f64,
    moist: f64,
    crop: f64,
    tint: [f64; 4],
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }

    let stage_files = section_assets(&project)?;

    let (mut rt, host) = new_runtime(&project)?;
    banner("② 农田三态：犁地 / 湿润（叠痕 + tint）/ 干涸");
    enter_manor(&mut rt)?;
    soil_sheet(&project, &mut rt)?;
    // 播种受土地类型限制（`crop.land ~= land_type` 直接 wrong_land），田块按作物挑：
    // 稻谷走粮田（field），灵芝走药田（medicinal）——写死坐标会随场景重生成飘掉。
    let field = pick_cell(&mut rt, "field")?;
    let medicinal = pick_cell(&mut rt, "medicinal")?;
    three_state(&mut rt, field)?;

    banner("③ 阶段推进：稻谷 4 档 / 灵芝 5 档，同一档两作物不许同图");
    stage_ramp(&mut rt, &host, field, medicinal)?;

    banner("④ 收成动画：4 帧抖动 @100ms + 闪光，播完落回真实地块");
    harvest_animation(&mut rt, &host, field)?;

    println!(
        "jhyt_m72_crop_stages_ok project={} stage_files={} atlas={}x{} entries={} crops={}",
        project.display(),
        stage_files,
        ATLAS_COLS,
        ATLAS_ROWS,
        ATLAS_COLS * ATLAS_ROWS,
        CROPS.len()
    );
    Ok(())
}

// ---------------------------------------------------------------- ① 资产与寻址

fn section_assets(project: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    banner("① 阶段图资产：数量、图集几何、逐条目两两不重复 + 反面对照");

    let dir = project.join(CROPS_DIR.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !dir.is_dir() {
        return Err(
            format!("{CROPS_DIR}/ 不存在：plan §现状 记的是 29/29 缺，本包必须把它变成有").into(),
        );
    }
    let stage_files = count_stage_files(&dir)?;
    if stage_files < MIN_STAGE_FILES {
        return Err(
            format!("阶段图 {stage_files} 张，判据 ≥{MIN_STAGE_FILES}（29 种 × 4–5 档）").into(),
        );
    }
    println!("  {CROPS_DIR} 阶段图 = {stage_files} 张（判据 ≥{MIN_STAGE_FILES}）");

    let image = decode_rgba(project, CROPS_TEXTURE)?;
    let want = (ATLAS_COLS * TILE, ATLAS_ROWS * TILE);
    if (image.0, image.1) != (want.0 as u32, want.1 as u32) {
        return Err(format!(
            "{CROPS_TEXTURE} 实测 {}x{}，应为 {want:?}（= atlas_cols×atlas_rows×tile）：\
             贴图尺寸与场景声明不符，切 UV 必错位",
            image.0, image.1
        )
        .into());
    }

    // 只把**阶段槽**纳入两两比对：抖动帧是同一株的位移，跨作物撞了不代表"两种作物
    // 长一样"，而阶段槽撞了就是 plan §风险 里点名的"换个色冒充另一种"。
    let mut tiles: Vec<(String, u64, bool)> = Vec::new();
    for (id, row, stages) in CROPS {
        for slot in 0..*stages {
            let entry = entry_of(row, slot);
            let bytes = entry_bytes(&image, entry);
            let opaque = bytes.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
            tiles.push((format!("{id}.stage_{}", slot + 1), fnv1a(&bytes), opaque));
        }
    }
    let problems = tile_problems(&tiles);
    if !problems.is_empty() {
        return Err(format!(
            "阶段图唯一性判据失败（{} 条）：\n  {}",
            problems.len(),
            problems.join("\n  ")
        )
        .into());
    }
    println!(
        "  图集 {ATLAS_COLS}x{ATLAS_ROWS} 条目，阶段槽 {} 个两两不同且无一空槽",
        tiles.len()
    );

    // 反面对照（plan §验收方式）：把任意两种作物的同一阶段**配成同一张**，
    // 必须被同一个函数抓出来。判"检查函数会不会漏"只能这样判——真资产是绿的，
    // 永远证明不了红的那一半。
    let mut poisoned = tiles.clone();
    let lingzhi2 = poisoned
        .iter()
        .position(|(label, _, _)| label == "lingzhi.stage_2")
        .ok_or("lingzhi.stage_2 缺席")?;
    let rice2 = poisoned
        .iter()
        .position(|(label, _, _)| label == "rice.stage_2")
        .ok_or("rice.stage_2 缺席")?;
    poisoned[rice2].1 = poisoned[lingzhi2].1;
    let caught = tile_problems(&poisoned);
    if caught.is_empty() {
        return Err("反面对照没跑红：把稻谷第 2 档配成灵芝第 2 档，唯一性检查却全绿".into());
    }
    println!("  反面对照跑红（{} 条）：{}", caught.len(), caught[0]);

    // 抖动帧：每作物 3 槽必须都在（收成动画取不到槽就退化成"收完直接消失"）。
    for (id, row, _) in CROPS {
        for slot in 0..SHAKE_SLOTS {
            let bytes = entry_bytes(&image, entry_of(row, STAGE_SLOTS + slot));
            if !bytes.as_chunks::<4>().0.iter().any(|p| p[3] != 0) {
                return Err(format!("{id} 的抖动槽 {} 是空的", slot + 1).into());
            }
        }
    }
    println!("  抖动帧 3 槽 × {} 种作物全部有内容", CROPS.len());
    Ok(stage_files)
}

fn count_stage_files(dir: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut count = 0;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        for file in fs::read_dir(&path)? {
            let name = file?.file_name().to_string_lossy().to_string();
            if name.starts_with("stage_") && name.ends_with(".png") {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// 阶段槽（0 基）→ 图集条目号（1 基，row-major，行号 1 基）。
/// 与 `crop_kit.entry_for` / `crop_art.lua:M.entry` / 引擎 `Tilemap::atlas_uv` 同构。
fn entry_of(row: &usize, slot: usize) -> usize {
    (row - 1) * ATLAS_COLS + slot + 1
}

/// 一组 (标签, 摘要, 是否有不透明像素) 的问题清单：空槽 + 两两同图。
fn tile_problems(tiles: &[(String, u64, bool)]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen: BTreeMap<u64, &str> = BTreeMap::new();
    for (label, digest, opaque) in tiles {
        if !opaque {
            problems.push(format!("{label} 是空槽"));
            continue;
        }
        if let Some(other) = seen.insert(*digest, label.as_str()) {
            problems.push(format!("{label} 与 {other} 字节完全相同（换个色也没用）"));
        }
    }
    problems
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 解码后的位图：(宽, 高, 行优先像素)。
type RgbaImage = (u32, u32, Vec<[u8; 4]>);

/// 解码 RGBA8；返回 (宽, 高, 行优先像素)。
fn decode_rgba(project: &Path, path: &str) -> Result<RgbaImage, Box<dyn std::error::Error>> {
    let resolved = project.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let bytes = fs::read(&resolved).map_err(|error| format!("{path}: {error}"))?;
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("{path}: {error}"))?
        .to_rgba8();
    let (w, h) = (image.width(), image.height());
    Ok((
        w,
        h,
        image
            .into_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect(),
    ))
}

fn entry_bytes(image: &(u32, u32, Vec<[u8; 4]>), entry: usize) -> Vec<u8> {
    let position = entry - 1;
    let ox = (position % ATLAS_COLS) * TILE;
    let oy = (position / ATLAS_COLS) * TILE;
    let mut out = Vec::with_capacity(TILE * TILE * 4);
    for y in oy..oy + TILE {
        for x in ox..ox + TILE {
            out.extend_from_slice(&image.2[y * image.0 as usize + x]);
        }
    }
    out
}

/// 单行土面贴图里第 `slot` 列（0 基）的一格。列数已由 §② 的几何判据校过，
/// 所以这里不能借作物图集的 8 列步长。
fn column_bytes(image: &(u32, u32, Vec<[u8; 4]>), slot: usize) -> Vec<u8> {
    let ox = slot * TILE;
    let mut out = Vec::with_capacity(TILE * TILE * 4);
    for y in 0..TILE {
        for x in ox..ox + TILE {
            out.extend_from_slice(&image.2[y * image.0 as usize + x]);
        }
    }
    out
}

// ---------------------------------------------------------------- 运行期脚手架

fn new_runtime(
    project: &Path,
) -> Result<(LuaRuntime, rustlua_input::host::HostInput), Box<dyn std::error::Error>> {
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&project.join("scripts/main.lua"))?;
    Ok((runtime, host))
}

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(code)?.trim().to_owned())
}

fn run(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    want: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = lua(runtime, &format!("return {expr}"))?;
    let got: f64 = raw
        .parse()
        .map_err(|error| format!("{expr} = {raw:?} 不是数字: {error}"))?;
    if (got - want).abs() > 1e-6 {
        return Err(format!("{expr} = {got}，应为 {want}").into());
    }
    Ok(())
}

/// 取一个数值读数（Lua 的整数/浮点都按 f64 收）。
fn num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let raw = lua(runtime, &format!("return tostring({expr})"))?;
    raw.parse()
        .map_err(|error| format!("{expr} = {raw:?} 不是数字: {error}").into())
}

/// 在农田矩形里挑一块**该土地类型**、尚未开垦且地表是 DIRT 的格子（确定性）。
/// 硬编码坐标会在场景重生成后悄悄变成 wrong_land，挑格才是稳的。
fn pick_cell(
    runtime: &mut LuaRuntime,
    want_land: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let got = lua(
        runtime,
        &format!(
            "(function() local farm = require('systems.farm'); \
             local land = require('data.farm').land; local g = entity.find('GroundMap'); \
             for y = 4, 13 do for x = 14, 33 do \
               if not farm.is_plot(x, y) and (tilemap.get_cell(g, x, y) or 0) == 2 then \
                 for _, l in ipairs(land) do \
                   if x >= l.x0 and x <= l.x1 and y >= l.y0 and y <= l.y1 then \
                     if l.type == '{want_land}' then return x .. ',' .. y end; \
                     break; \
                   end \
                 end \
               end \
             end end; return 'none' end)()"
        ),
    )?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or(format!("没有 {want_land} 可种格：{got}"))?;
    let y = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or(format!("没有 {want_land} 可种格：{got}"))?;
    println!("  {want_land} 田格 = ({x}, {y})");
    Ok((x, y))
}

/// 地块明细里的 watered_days（map.state 的键就是 "cx,cy"）。
fn plot_number(
    runtime: &mut LuaRuntime,
    cx: i64,
    cy: i64,
    field: &str,
) -> Result<f64, Box<dyn std::error::Error>> {
    num(
        runtime,
        &format!(
            "(function() local s = map.state('farm_plots'); \
             local p = s and s['{cx},{cy}']; \
             return tostring(p and p.{field} or -1) end)()"
        ),
    )
}

fn enter_manor(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    run(runtime, "scene.load('main')")?;
    runtime.process_scene_transition()?;
    run(runtime, "on_click('BtnStart')")?;
    runtime.process_scene_transition()?;
    let scene = lua(runtime, "return scene.reload()")?;
    if scene != "manor" {
        return Err(format!("scene = {scene:?}，应为 manor").into());
    }
    // 钉晴天：雨天会让 act('up') 走 rain_watered 免体力分支，把三态的节奏搅乱。
    run(runtime, "require('world.weather').set('sunny')")?;
    Ok(())
}

fn pump(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    frames: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..frames {
        host.set_input(InputSnapshot::default());
        runtime.call_update()?;
    }
    Ok(())
}

/// 站到底下一格、朝上，让 farm 的目标格就是 (cx, cy)。
fn stand_below(
    runtime: &mut LuaRuntime,
    cx: i64,
    cy: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    run(
        runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'",
            (cx as f32 + 0.5) * 32.0,
            (cy as f32 - 0.5) * 32.0
        ),
    )
}

fn act(runtime: &mut LuaRuntime) -> Result<String, Box<dyn std::error::Error>> {
    lua(runtime, "return require('systems.farm').act('up')")
}

fn top_up_energy(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    run(runtime, "require('core.state').energy = 100")
}

fn equip(runtime: &mut LuaRuntime, tool: &str) -> Result<(), Box<dyn std::error::Error>> {
    run(
        runtime,
        &format!("inv.equip(require('core.state').inventory, 'tool', '{tool}')"),
    )
}

/// 手里有种子并选中它（播种走 farm.act 的公开路径，不直接写地块数据）。
fn plant(runtime: &mut LuaRuntime, seed: &str) -> Result<(), Box<dyn std::error::Error>> {
    run(
        runtime,
        &format!("inv.add(require('core.state').inventory, '{seed}', 8)"),
    )?;
    run(
        runtime,
        &format!("require('core.state').selected_seed = '{seed}'"),
    )?;
    top_up_energy(runtime)?;
    let action = act(runtime)?;
    if action != format!("plant_{seed}") {
        return Err(
            format!("播种返回 {action:?}，应为 plant_{seed}（田格土地类型/季节不匹配？）").into(),
        );
    }
    Ok(())
}

/// 日结算（走 farm 公开 API：+1 成长、撤当日湿痕与 tint）。
fn settle(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    run(runtime, "require('systems.farm').settle_day()")
}

/// 浇一次水（水壶在手；成熟格不会被它影响，farm 自己会走收获分支）。
fn water(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    equip(runtime, "watering_can")?;
    top_up_energy(runtime)?;
    let action = act(runtime)?;
    if !action.starts_with("water_") {
        return Err(
            format!("浇水返回 {action:?}，应为 water_<作物>（没浇上水，成长天数不会走）").into(),
        );
    }
    Ok(())
}

/// tint 近似相等：档位差 ≥0.06，浮点往返噪声在 1e-3 以下。
fn tint_near(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-4)
}

/// 一格在土面 / 湿痕 / 作物 / tint 四条通道上的读数。
fn read_cell(
    runtime: &mut LuaRuntime,
    cx: i64,
    cy: i64,
) -> Result<Cell, Box<dyn std::error::Error>> {
    let raw = lua(
        runtime,
        &format!(
            "(function() \
             local g = entity.find('FarmGrid'); local m = entity.find('FarmMoist'); \
             local c = entity.find('CropGrid'); \
             if not (g and m and c) then return '-1,-1,-1,0,0,0,0' end; \
             local t = tilemap.tint(m, {cx}, {cy}) or {{r=1,g=1,b=1,a=1}}; \
             return string.format('%s,%s,%s,%s,%s,%s,%s', \
               tostring(tilemap.get_cell(g, {cx}, {cy}) or 0), \
               tostring(tilemap.get_cell(m, {cx}, {cy}) or 0), \
               tostring(tilemap.get_cell(c, {cx}, {cy}) or 0), \
               tostring(t.r), tostring(t.g), tostring(t.b), tostring(t.a)) \
             end)()"
        ),
    )?;
    let fields: Vec<f64> = raw
        .split(',')
        .map(|part| part.parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{raw:?} 解析失败: {error}"))?;
    if fields.len() != 7 {
        return Err(format!("{raw:?} 不是 7 个字段").into());
    }
    Ok(Cell {
        soil: fields[0],
        moist: fields[1],
        crop: fields[2],
        tint: [fields[3], fields[4], fields[5], fields[6]],
    })
}

fn is_white(cell: &Cell) -> bool {
    cell.tint.iter().all(|v| (v - 1.0).abs() < 1e-6)
}

/// 图层结构（在②之前顺手判一次，红得比"瓦值不对"更早）。
fn assert_crop_layer(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    assert_num(runtime, "entity.find('CropGrid') and 1 or 0", 1.0)
        .map_err(|_| "场景里没有 CropGrid 实体：作物层没接进 manor.scene".to_owned())?;
    for (key, want) in [
        ("atlas_cols", ATLAS_COLS as f64),
        ("atlas_rows", ATLAS_ROWS as f64),
        ("max_tile_id", (ATLAS_COLS * ATLAS_ROWS) as f64),
        ("tiles_format", 1.0),
    ] {
        assert_num(
            runtime,
            &format!("(tilemap.get(entity.find('CropGrid')) or {{}}).{key} or -1"),
            want,
        )
        .map_err(|error| format!("CropGrid {key} 不对：{error}"))?;
    }
    let texture = lua(
        runtime,
        "return tostring((tilemap.get(entity.find('CropGrid')) or {}).texture)",
    )?;
    if texture != CROPS_TEXTURE {
        return Err(format!(
            "CropGrid texture = {texture:?}，应为 {CROPS_TEXTURE:?}（贴图与门读的图集不是同一张）"
        )
        .into());
    }
    Ok(())
}

// ---------------------------------------------------------------- ② 三态

/// 三态的**贴图侧**判据：场景声明的 `atlas_cols` == `data/farm_soil.lua` 的列数 ==
/// 盘上 PNG 的实测宽度 ÷ 32，且两列土面**字节不同**、湿痕那一列真是半透明叠痕。
///
/// 为什么单独钉这一条：§② 的运行期判据读的是瓦值与 tint，而瓦值是"逻辑状态"——
/// 贴图退回旧版那张六列（SPROUT/GROWING/RIPE 那一版）时瓦值照样对，画面上却切到
/// 完全不相干的那几列。本包开发中真发生过一次（把 HEAD 的美术同步回工作树时覆盖了
/// `manor_farm.png`），门当时全绿。几何与像素都在这里的，才算钉住。
fn soil_sheet(project: &Path, runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    for (entity_name, want_texture, declared_field) in [
        ("FarmGrid", FARM_TEXTURE, "farm_cols"),
        ("FarmMoist", MOIST_TEXTURE, "moist_cols"),
    ] {
        let scene_cols = num(
            runtime,
            &format!("(tilemap.get(entity.find('{entity_name}')) or {{}}).atlas_cols"),
        )?;
        let scene_rows = num(
            runtime,
            &format!("(tilemap.get(entity.find('{entity_name}')) or {{}}).atlas_rows"),
        )?;
        let declared = num(
            runtime,
            &format!("require('data.farm_soil').{declared_field}"),
        )?;
        if scene_cols != declared {
            return Err(format!(
                "{entity_name} 场景声明 atlas_cols = {scene_cols}，\
                 data/farm_soil.lua 的 {declared_field} = {declared}（两处寻址飘了）"
            )
            .into());
        }
        let image = decode_rgba(project, want_texture)?;
        if (image.0 as f64, image.1 as f64) != (scene_cols * TILE as f64, scene_rows * TILE as f64)
        {
            return Err(format!(
                "{want_texture} 实测 {}x{}，应为 ({scene_cols:.0}×{TILE})x({scene_rows:.0}×{TILE})：\
                 贴图尺寸与场景声明不符，引擎按声明切 UV 必错列",
                image.0, image.1
            )
            .into());
        }
        println!(
            "  {entity_name}：atlas_cols={scene_cols:.0} == farm_soil.{declared_field} == 贴图宽 {}",
            image.0
        );
    }

    // 两列土面必须是两张图：瓦值不同已经钉过，这里是像素侧。
    let farm = decode_rgba(project, FARM_TEXTURE)?;
    let tiles: Vec<(String, u64, bool)> = [(0usize, "犁地"), (1usize, "干涸")]
        .into_iter()
        .map(|(slot, label)| {
            let bytes = column_bytes(&farm, slot);
            let opaque = bytes.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
            (label.to_string(), fnv1a(&bytes), opaque)
        })
        .collect();
    let problems = tile_problems(&tiles);
    if !problems.is_empty() {
        return Err(format!("三态贴图侧判据失败：{}", problems.join("；")).into());
    }
    let mut poisoned = tiles.clone();
    poisoned[1].1 = poisoned[0].1;
    if tile_problems(&poisoned).is_empty() {
        return Err("反面对照没跑红：把干涸配成犁地，两列判据却全绿".into());
    }
    println!("  土面两列（犁地/干涸）字节不同；反面对照跑红：干涸 := 犁地 被抓出");

    // 湿痕是**叠痕**不是第三张土面：既要有内容，又不能整幅不透明（那会抹掉下面的垄沟）。
    let moist = decode_rgba(project, MOIST_TEXTURE)?;
    let mut translucent = 0usize;
    let mut opaque = 0usize;
    let mut any = 0usize;
    for pixel in column_bytes(&moist, 0).as_chunks::<4>().0.iter() {
        if pixel[3] > 0 {
            any += 1;
        }
        if (1..255).contains(&pixel[3]) {
            translucent += 1;
        }
        if pixel[3] == 255 {
            opaque += 1;
        }
    }
    let cells = TILE * TILE;
    if any * 2 < cells {
        return Err(
            format!("湿痕列只有 {any}/{cells} 个像素有内容：它是空图，浇水看不出湿润").into(),
        );
    }
    if opaque * 10 > cells {
        return Err(format!(
            "湿痕列有 {opaque}/{cells} 个像素满不透明：这等于第三张全幅土面，会把垄沟抹平"
        )
        .into());
    }
    println!(
        "  湿痕叠痕：有内容 {any}/{cells}、半透明 {translucent}、满不透明 {opaque}（<10% 才不盖死垄沟）"
    );
    Ok(())
}

fn three_state(
    runtime: &mut LuaRuntime,
    cell: (i64, i64),
) -> Result<(), Box<dyn std::error::Error>> {
    let (cx, cy) = cell;
    assert_crop_layer(runtime)?;
    stand_below(runtime, cx, cy)?;
    top_up_energy(runtime)?;

    // 未开垦：三层全空（透出 GroundMap 的夯土）。
    let before = read_cell(runtime, cx, cy)?;
    if before.soil != 0.0 || before.moist != 0.0 || before.crop != 0.0 {
        return Err(format!("新档空地上不该有农田内容，读到 {before:?}").into());
    }

    // 翻土 → 犁地（第一列）。
    equip(runtime, "hoe")?;
    let action = act(runtime)?;
    if action != "till" {
        return Err(format!("翻土返回 {action:?}，应为 till").into());
    }
    let tilled = read_cell(runtime, cx, cy)?;
    if tilled.soil != SOIL_TILLED {
        return Err(format!("翻土后土面 = {}，应为 {SOIL_TILLED}（犁地）", tilled.soil).into());
    }
    if !is_white(&tilled) || tilled.moist != 0.0 {
        return Err(format!("刚翻的土没浇水，不该有湿痕/tint：{tilled:?}").into());
    }
    println!(
        "  犁地：soil={} moist={} tint 白",
        tilled.soil, tilled.moist
    );

    // 播种 + 浇水 → 湿痕有内容 **且 tint 非白**（湿润是"叠痕 × 顶点色"，不是第三列）。
    plant(runtime, "seed_rice")?;
    equip(runtime, "watering_can")?;
    top_up_energy(runtime)?;
    let action = act(runtime)?;
    if action != "water_rice" {
        return Err(format!("浇水返回 {action:?}，应为 water_rice").into());
    }
    let wet = read_cell(runtime, cx, cy)?;
    if wet.moist != 1.0 {
        return Err(format!("浇水后湿痕层 = {}，应为 1", wet.moist).into());
    }
    if is_white(&wet) {
        return Err(
            format!("浇水后 tint 仍是白：{wet:?}（湿润分档没接上 tilemap.set_tint）").into(),
        );
    }
    let first = wet.tint;
    println!(
        "  湿润：soil={} moist={} tint=[{:.3},{:.3},{:.3}]",
        wet.soil, wet.moist, wet.tint[0], wet.tint[1], wet.tint[2]
    );

    // 日结算撤痕 → 干涸（第二列）+ 湿痕回 0 + tint 必须回白。
    settle(runtime)?;
    let dry = read_cell(runtime, cx, cy)?;
    if dry.soil != SOIL_DRY {
        return Err(format!("断水后土面 = {}，应为 {SOIL_DRY}（干涸裂缝）", dry.soil).into());
    }
    if dry.moist != 0.0 {
        return Err(format!("日结算没撤湿痕：{}", dry.moist).into());
    }
    if !is_white(&dry) {
        return Err(format!("撤了湿痕却没撤 tint（{:?}）：下一轮 tint 会串色", dry.tint).into());
    }
    println!("  干涸：soil={} moist={} tint 回白", dry.soil, dry.moist);

    // 档位按**已浇满的天数**走，不按浇水次数：第二天浇下去仍是第 1 档（watered_days=1
    // 还没跨过档），再结算一天才降到第 2 档。两次都判，防"分档表是死的"和"档号错位"。
    top_up_energy(runtime)?;
    act(runtime)?;
    let same = read_cell(runtime, cx, cy)?;
    if same.moist != 1.0 || !tint_near(&same.tint, &first) {
        return Err(format!(
            "第二天浇水应仍是第 1 档（watered_days=1），读到 {same:?}（第 1 档 = {first:?}）"
        )
        .into());
    }
    settle(runtime)?;
    top_up_energy(runtime)?;
    act(runtime)?;
    let again = read_cell(runtime, cx, cy)?;
    if again.moist != 1.0 {
        return Err(format!("第三次浇水湿痕 = {}", again.moist).into());
    }
    if tint_near(&again.tint, &first) {
        return Err(format!("浇满两天后 tint 仍是第一档（{first:?}），湿润分档是死的").into());
    }
    println!(
        "  复浇换档：tint [{:.3},{:.3},{:.3}] → [{:.3},{:.3},{:.3}]",
        first[0], first[1], first[2], again.tint[0], again.tint[1], again.tint[2]
    );
    Ok(())
}

// ---------------------------------------------------------------- ③ 阶段推进

/// 把一块田浇到成熟，返回逐日读到的 CropGrid 瓦值序列（含播种当天）。
fn water_until_mature(
    runtime: &mut LuaRuntime,
    cx: i64,
    cy: i64,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    let mut seq = Vec::new();
    for _ in 0..14 {
        seq.push(read_cell(runtime, cx, cy)?.crop);
        if num(runtime, "require('systems.farm').stats().mature")? > 0.5 {
            break;
        }
        settle(runtime)?;
        // 结算本身就可能把 watered_days 推过门槛（wd=3 → 4）。这时**不能再浇**：
        // farm.act 在成熟分支里会直接收获，把这一格收走，序列就断在苗期。
        if num(runtime, "require('systems.farm').stats().mature")? > 0.5 {
            continue;
        }
        water(runtime)?;
    }
    Ok(seq)
}

/// 一株作物从播种到成熟的完整走查：档数、单调性、成熟日数值锚点。
fn ramp_one(
    runtime: &mut LuaRuntime,
    id: &str,
    cell: (i64, i64),
    row: usize,
    stages: usize,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    let (cx, cy) = cell;
    stand_below(runtime, cx, cy)?;
    equip(runtime, "hoe")?;
    top_up_energy(runtime)?;
    act(runtime)?;
    plant(runtime, &format!("seed_{id}"))?;
    let start = read_cell(runtime, cx, cy)?;
    if start.crop != entry_of(&row, 0) as f64 {
        return Err(format!(
            "{id} 播种当天 CropGrid = {}，应为第 1 档 {}",
            start.crop,
            entry_of(&row, 0)
        )
        .into());
    }
    let seq = water_until_mature(runtime, cx, cy)?;
    // 单调不减、终点 = 最后一档。
    for pair in seq.windows(2) {
        if pair[1] < pair[0] {
            return Err(format!("{id} 阶段倒退：{seq:?}（渲染铺档不许让作物越种越小）").into());
        }
    }
    let mature = *seq.last().ok_or(format!("{id} 没读到阶段序列"))?;
    if mature != entry_of(&row, stages - 1) as f64 {
        return Err(format!(
            "{id} 成熟时 CropGrid = {mature}，应为最后一档 {}（共 {stages} 档）",
            entry_of(&row, stages - 1)
        )
        .into());
    }
    // 数值零改动：成熟仍发生在 watered_days == growth_days 那天（本包只把渲染铺细）。
    let days = plot_number(runtime, cx, cy, "watered_days")?;
    let growth = num(runtime, &format!("require('data.crops').{id}.growth_days"))?;
    if (days - growth).abs() > 1e-6 {
        return Err(format!(
            "{id} 成熟日的 watered_days = {days}，growth_days = {growth}：\
             阶段数从 3 变 4/5 不许动生长天数（docs/33 数值案）"
        )
        .into());
    }
    println!(
        "  {id} 阶段序列 = {seq:?}（{stages} 档 / {growth} 天，成熟日 watered_days 与数值案一致）"
    );
    // 收干净，下一块田重来。
    equip(runtime, "hoe")?;
    top_up_energy(runtime)?;
    act(runtime)?;
    Ok(seq)
}

fn stage_ramp(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    field: (i64, i64),
    medicinal: (i64, i64),
) -> Result<(), Box<dyn std::error::Error>> {
    // ② 结束时稻田是"稻种 + 已浇 2 天"。整块农田清干净，再逐作物走全程。
    run(runtime, "require('systems.farm').reset()")?;
    pump(runtime, host, 2)?;
    for cell in [field, medicinal] {
        let cleared = read_cell(runtime, cell.0, cell.1)?;
        if cleared.soil != 0.0 || cleared.crop != 0.0 {
            return Err(format!("farm.reset 没抹回空（{:?}）：{cleared:?}", cell).into());
        }
    }

    let rice = ramp_one(runtime, "rice", field, 2, 4)?;
    // 灵芝要药田（crop.land = medicinal），和稻谷不同格；同格会 wrong_land。
    let lingzhi = ramp_one(runtime, "lingzhi", medicinal, 1, 5)?;

    // 同一档序号（都还是第 1 档）时两种作物的瓦值必须不同——这是"共图"的第一道闸，
    // 读像素之前就能红。行号不同 ⇒ 图集位置不同 ⇒ 各自有自己的一张图。
    let rice_first = rice.first().copied().unwrap_or_default();
    let lingzhi_first = lingzhi.first().copied().unwrap_or_default();
    if rice_first == lingzhi_first {
        return Err(format!("稻谷与灵芝的第 1 档都是 {rice_first}：阶段图还是共用的").into());
    }
    println!("  同档对照：稻谷第 1 档 {rice_first} ≠ 灵芝第 1 档 {lingzhi_first}");

    // 寻址三方对拍：门内表 == crop_art.lua == crops.lua 的 stages。
    for (id, row, stages) in CROPS {
        let got = lua(
            runtime,
            &format!(
                "local a = require('data.crop_art').crops.{id}; \
                 return tostring(a and a.row or -1)"
            ),
        )?;
        if got != row.to_string() {
            return Err(format!("{id} 在 crop_art.lua 的行号 = {got}，门内表是 {row}").into());
        }
        let art_stages = lua(
            runtime,
            &format!("return tostring(require('data.crop_art').crops.{id}.stages)"),
        )?;
        let data_stages = lua(
            runtime,
            &format!("return tostring(require('data.crops').{id}.stages)"),
        )?;
        if art_stages != stages.to_string() || data_stages != stages.to_string() {
            return Err(format!(
                "{id} 阶段数：crop_art.lua = {art_stages}，crops.lua = {data_stages}，门内表 = {stages}"
            )
            .into());
        }
    }
    println!("  29 种作物寻址三方一致（门内表 / crop_art.lua / crops.lua）");
    Ok(())
}

// ---------------------------------------------------------------- ④ 收成动画

/// 稻谷三个品质档位的总持有量（品质只换条目名，不换个数）。
fn rice_count(runtime: &mut LuaRuntime) -> Result<f64, Box<dyn std::error::Error>> {
    num(
        runtime,
        "(function() local inv_id = require('core.state').inventory; \
         return inv.count(inv_id, 'rice') + inv.count(inv_id, 'rice_good') \
           + inv.count(inv_id, 'rice_super') end)()",
    )
}

fn harvest_animation(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    cell: (i64, i64),
) -> Result<(), Box<dyn std::error::Error>> {
    let (cx, cy) = cell;
    run(runtime, "require('systems.farm').reset()")?;
    stand_below(runtime, cx, cy)?;
    equip(runtime, "hoe")?;
    top_up_energy(runtime)?;
    act(runtime)?;
    plant(runtime, "seed_rice")?;
    water_until_mature(runtime, cx, cy)?;
    let mature_tile = read_cell(runtime, cx, cy)?.crop;
    if mature_tile != entry_of(&2, 3) as f64 {
        return Err(format!(
            "收成前瓦值 = {mature_tile}，应为稻谷最后一档 {}",
            entry_of(&2, 3)
        )
        .into());
    }
    // 前面翻土/浇水留下的条带（最长 6/12 秒 = 30 帧）全部自毁，特效计数回到 0；
    // 成熟后多等不腐烂（docs/33：成熟后停留可采），所以这里冲帧不改变地块。
    pump(runtime, host, 90)?;
    if num(runtime, "require('systems.effects').active()")? != 0.0 {
        return Err("进收成前活动特效不为 0：上一条带还没自毁，计数断言会失真".into());
    }
    let before = rice_count(runtime)?;

    equip(runtime, "hoe")?;
    top_up_energy(runtime)?;
    let action = act(runtime)?;
    if !action.starts_with("harvest") {
        return Err(format!("成熟格收获返回 {action:?}，应为 harvest_*").into());
    }
    // 同一帧内只可能多出这一条 harvest_sparkle（还没 update 过，不会有别的自毁/新生）。
    let fx_after_spawn = num(runtime, "require('systems.effects').active()")?;
    if fx_after_spawn != 1.0 {
        return Err(format!(
            "收获后活动特效 = {fx_after_spawn}，应为 1（harvest_sparkle 没接进 effects.lua 的 FX 表？）"
        )
        .into());
    }

    // 采样：动画每 100ms 换一格（=6 帧），记录每个"变化的帧号 + 读到的瓦值"。
    let mut changes: Vec<(usize, f64)> = Vec::new();
    let mut last = mature_tile;
    let mut alive_mid = 0.0;
    for frame in 1..=48 {
        pump(runtime, host, 1)?;
        let got = read_cell(runtime, cx, cy)?.crop;
        if got != last {
            changes.push((frame, got));
            last = got;
        }
        if frame == 12 {
            alive_mid = num(runtime, "require('systems.effects').active()")?;
        }
    }
    if alive_mid != 1.0 {
        return Err(format!(
            "动画放到第 12 帧时在场特效 = {alive_mid}，应为 1（收成闪光 ttl = 6/10 秒 = 36 帧，应与抖动同时在场）"
        )
        .into());
    }
    // 4 帧抖动：抖 1/2/3 + 落回真值（0）。起手那一格就是成熟瓦，不计入"变化"。
    let expected: Vec<f64> = vec![
        entry_of(&2, STAGE_SLOTS) as f64,
        entry_of(&2, STAGE_SLOTS + 1) as f64,
        entry_of(&2, STAGE_SLOTS + 2) as f64,
        0.0,
    ];
    let got: Vec<f64> = changes.iter().map(|(_, value)| *value).collect();
    if got != expected {
        return Err(format!(
            "收成抖动帧序列 = {got:?}，应为 {expected:?}（crop_art.shake_entry(rice, 0..2) 后落空）"
        )
        .into());
    }
    let frames: Vec<usize> = changes.iter().map(|(frame, _)| *frame).collect();
    if frames.len() != 4 {
        return Err(format!("收成动画只读到 {} 次换格，应为 4", frames.len()).into());
    }
    let mut gaps = Vec::new();
    let mut previous = 0;
    for frame in &frames {
        gaps.push(frame - previous);
        previous = *frame;
    }
    // 100ms @60fps = 6 帧；允许 ±1 帧（累加 1/60 到 0.1 的浮点余量）。
    for gap in &gaps {
        if !(5..=7).contains(gap) {
            return Err(format!(
                "收成帧间隔 = {gaps:?}（每格应为 100ms ≈ 6 帧，docs/51 §2.2 快动作口径）"
            )
            .into());
        }
    }
    println!("  收成 4 帧抖动 @100ms：帧号 {frames:?}，瓦值 {got:?}，第 12 帧闪光在场");

    let after_anim = read_cell(runtime, cx, cy)?;
    // 收完的格子回到"已垦空地"：土面仍是犁地列（地块记录还在），作物层必须回空。
    if after_anim.crop != 0.0 || after_anim.soil != SOIL_TILLED {
        return Err(format!("动画放完没落回真实地块：{after_anim:?}").into());
    }
    // ⑤ 产量锚点：这次收获净增的个数 == `data/crops.lua` 的 yield（改图前后同读数）。
    let gained = rice_count(runtime)? - before;
    let want = num(runtime, "require('data.crops').rice.yield")?;
    if (gained - want).abs() > 1e-6 {
        return Err(format!("稻谷实收 {gained} 个，data/crops.lua yield = {want}").into());
    }
    println!("  产量锚点：本次净收 {gained} == crops.lua yield {want}");
    Ok(())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}
