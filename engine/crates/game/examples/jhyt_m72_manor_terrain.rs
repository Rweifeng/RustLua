//! Headless acceptance drive for M72-04：**manor 地表图集的色阶、边缘过渡与"一屏瓦种类"**。
//!
//! 需求（`tasks/plan/M72-04-manor地表图集重做与色阶表.md`）：地表从前是"5 种单色瓦 +
//! 直线边界"，要重做成带 4-bit 过渡块、带同族明度档的图集。本门钉的是**读起来真的花**，
//! 而且"花"是可以数的，不靠肉眼点头。
//!
//! 为什么判据取"条目 × 顶点色"这个等价类而不是只数条目：`autotile=1` 时引擎的解算是
//! if/else 两枝（`Tilemap::resolve_atlas_index`），**变体哈希那一枝根本不被走到**，所以
//! 一格草的满块永远落在同一个条目上；一屏内的重复只能由 per-tile tint 打散（M72-02 的染色层）。
//! 屏幕上两片像素相同 ⟺ 图集子矩形相同 **且** 顶点色相同，于是"同屏可见瓦种类"按这个等价类
//! 数；纯条目数同时打印，两个口径都不藏。
//!
//! 章节：
//!   ① 地表层读回来是新语义：`tiles_format=1 / 12×9 / 6 kind / autotile 开`，格子值仍只有
//!      那 5 个游戏语义 id；**整层**（关剔除）画出的条目多重集 = 逐格 `atlas_index` 的
//!      多重集（渲染器与解算器不许各说各话），且去重 ≥24（旧值 5）。
//!   ② 一屏：开剔除按实机默认相机取 mesh，(条目×染色) 等价类 ≥24；再按数据侧扫**所有**
//!      20×12 窗口取最小值 ≥24——相机停哪都不低于这条。
//!   ③ 交界：每个交界格都必须取到过渡块（不许是该 kind 的满块槽）；天然地貌（草/土/水）
//!      两两之间不许"一整条直线"（同排或同列连续同外观 ≤5 格）。
//!      反面对照：同一套统计喂一张规则矩形的合成旧地图，必须报出 ≥6 格的同外观直边。
//!   ④ 色阶门：每个**有 kind 挂上的**色族，在图集里实测到的明度档数 ≥3、且按表内深→浅
//!      顺序亮度严格递增；基础图与四季图各查一遍（四季图先按同位置置换还原成基础色阶）。
//!      反面对照：把草族压成单档再走**同一个**函数，必须红。
//!   ⑤ 调色板归属：图集每个不透明像素必须等于门内色阶表副本里的某个色；尾部空槽
//!      （103..108）全透明。反面对照：从表副本删一档，offender 必须 >0 且点名那个色。
//!   ⑥ tint 分档：地表染色取值 ⊆ {白, shaded, sunlit}，且三档都真的落在满块瓦上——
//!      否则 §② 的多样性是"靠染色打散重复"还是"根本没重复"就说不清。
//!
//! 色阶表在门里是**手抄副本**（`FAMILY_LEVELS`），故意的：门的职责是"生成器与文档必须给出
//! 同一批颜色"，自己去解析 `terrain_ramp.json` 就变成表自己跟自己一致，改了表也测不出来。
//! 副本与表飘红时本门会红在"图集里出现了表外色"，报出的 hex 就是飘掉的那一档。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_manor_terrain [-- <game_jhyt-root>]

use engine::{
    core::{
        input::InputSnapshot,
        tilemap::{TILES_FORMAT_ATLAS, Tilemap},
        transform::Vec2,
    },
    host::{RuntimeScene, SceneMesh},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;

/// 图集几何（= `tools/terrain_ramp.json` 的 `atlas`，也与场景里的声明对拍）。
const TILE_PX: usize = 32;
const ATLAS_COLS: usize = 12;
const ATLAS_ROWS: usize = 9;
const ATLAS_ENTRIES: usize = ATLAS_COLS * ATLAS_ROWS;
const KINDS: usize = 6;
const AUTOTILE_SLOTS: usize = 16;
/// 关掉 autotile 时 kind 的恒等条目（`tiles == entry`），也是 combat_arena 复用的那 6 格。
const IDENTITY_ENTRIES: [u8; KINDS] = [1, 2, 3, 4, 5, 6];
/// 图集尾部没有任何 kind 占用的槽位（1 基）：必须是全透明的。
const TAIL_SLOTS: [u8; 6] = [103, 104, 105, 106, 107, 108];

const GROUND_ATLAS: &str = "assets/art/tilesets/manor/manor_ground.png";
const SEASON_ATLAS: [&str; 4] = [
    "assets/art/tilesets/manor/manor_ground_spring.png",
    "assets/art/tilesets/manor/manor_ground_summer.png",
    "assets/art/tilesets/manor/manor_ground_autumn.png",
    "assets/art/tilesets/manor/manor_ground_winter.png",
];

/// 一屏的格数：实机默认相机（1280×720、地表 tile 32、实测 252 格）约 20.6×12.2 格，
/// 数据侧按 **20×12** 这个"最坏窗口"扫——任何一屏都包含一个这样的窗口，读数只会更高。
const SCREEN_W: usize = 20;
const SCREEN_H: usize = 12;
/// 量化判据（plan §验收方式）：同屏可见瓦种类 5 → ≥24。
const MIN_SCREEN_TILES: usize = 24;
/// 色阶门的档数下限（docs/53 §2：同族 3–5 档）。
const MIN_FAMILY_LEVELS: usize = 3;
/// "一整条直线"的上限：32px 瓦连铺 6 格 = 192px ≈ 角色宽的 4 倍，肉眼就读成一条线。
const MAX_STRAIGHT_BORDER_RUN: usize = 5;

/// `tools/terrain_ramp.json` 的 `families`：每族按**深→浅**排列（档序就是美术契约）。
const FAMILY_LEVELS: &[(&str, &[u32])] = &[
    ("grass", &[0x4A_66_36, 0x5E_7C_46, 0x6F_8F_52, 0x7F_A3_5E]),
    ("dirt", &[0x4E_3A_26, 0x63_48_2F, 0x7C_5C_3E, 0x8F_6E_4A]),
    ("stone", &[0x5C_5A_52, 0x6E_6B_62, 0x8E_8B_80, 0xA5_A1_97]),
    ("water", &[0x2F_4E_6B, 0x3E_64_85, 0x5D_82_A0, 0x7F_A3_D4]),
    ("roof", &[0x2C_36_3C, 0x3E_4A_52, 0x4A_55_60, 0x55_62_6B]),
    ("hedge", &[0x00_40_30, 0x00_50_30, 0x1E_5B_38, 0x2E_6B_45]),
];
/// `utility`：跨族通用色（墨线 / 宣纸 / 金 / 枯枝木）。允许出现在地表，
/// 但**不计入**材质族的明度档数——它们是描边与点缀，不是同一材质的受光梯度。
const UTILITY_COLORS: &[u32] = &[0x1F_23_28, 0xF2_EA_D8, 0xC9_A0_5A, 0x4A_36_28];
/// `ground_tints` 的两档乘数（顶点色是乘法：一档压冷暗、一档抬暖亮，夹着白档就是三档）。
const GROUND_TINTS: [[f32; 3]; 2] = [[0.96, 0.98, 1.0], [1.03, 1.02, 0.99]];
const TINT_WHITE: [f32; 3] = [1.0, 1.0, 1.0];

/// kind → 材质族（下标 = kind id）。kind 4「院墙」画在石族色阶上（三段式墙体 + 草帽顶，
/// 草帽顶用的是草/绿族色，属跨族点缀）；kind 5「青瓦」地表层不铺但图集里有块。
const KIND_FAMILY: [&str; KINDS + 1] = ["", "grass", "dirt", "stone", "stone", "roof", "water"];

/// 天然地貌的 kind（草/土/水）：只有它们之间的交界要求"不许是一条直线"。
/// 石板路（3）与院墙（4）是人工结构，直的是应该的。
const ORGANIC_KINDS: [u8; 3] = [1, 2, 6];

/// 一格在屏幕上的外观等价类：图集条目 + 顶点色档位。两者全同 = 两片一模一样的像素。
type Appearance = (u8, u8);

/// 一个绘制段的顶点区间（起始 + 数量）。overlay 与世界深度两条通道同构。
type DrawRange = (usize, usize);

/// mesh 顶点对门有意义的那两个字段：UV 子矩形角点 + 顶点色。
type UvColor = ([f32; 2], [f32; 4]);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;

    banner("① 地表层是新语义，且整层画出的条目 = 逐格解算的条目");
    assert_scene(&mut runtime, "main")?;
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    assert_scene(&mut runtime, "manor")?;
    let ground = first_tilemap_entity(&mut runtime)?;
    let layout = lua_table(&mut runtime, &format!("tilemap.get({ground})"))?;
    for (key, want) in [
        ("tiles_format", f64::from(TILES_FORMAT_ATLAS)),
        ("atlas_cols", ATLAS_COLS as f64),
        ("atlas_rows", ATLAS_ROWS as f64),
        ("tile_kinds", KINDS as f64),
        ("variant_count", 1.0),
        // 开启自动瓦片后格子值的**上限就是 kind 种数**，不是图集容量：这一格等于 6
        // 才说明地图读的是"逻辑瓦"，108 那条容量线只在图集布局里起作用。
        ("max_tile_id", KINDS as f64),
    ] {
        let got = field_num(&layout, key)?;
        if got != want {
            return Err(format!("地表层 {key} = {got}，应为 {want}（M72-04 的图集语义）").into());
        }
    }
    if field_str(&layout, "autotile")? != "true" {
        return Err("地表层 autotile 没开：过渡块全是死数据，边界只能画成硬直线".into());
    }
    let width = field_num(&layout, "width")? as usize;
    let height = field_num(&layout, "height")? as usize;
    let texture = field_str(&layout, "texture")?;
    if texture != GROUND_ATLAS && !SEASON_ATLAS.contains(&texture.as_str()) {
        return Err(format!("地表贴图 = {texture:?}，应为山庄地表图集之一").into());
    }
    let grid = read_ground(&mut runtime, ground, width, height)?;
    let ids: Vec<u8> = grid
        .kinds
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if ids != [1, 2, 3, 4, 6] {
        return Err(format!(
            "格子值 = {ids:?}，应为 [1,2,3,4,6]（草/土/石板/院墙/水）。美术重排只动图集与解算，\
             不许动游戏语义：farm.lua 认 tile==2 判可垦、building.lua 认 tile==3 判道路"
        )
        .into());
    }
    runtime.set_tilemap_culling(false);
    let layer = appearances_for(&mut runtime, &texture)?;
    let layer_entries: BTreeSet<u8> = layer.iter().map(|(entry, _)| *entry).collect();
    let grid_entries: BTreeSet<u8> = grid.entries.iter().copied().filter(|&e| e != 0).collect();
    if layer_entries != grid_entries {
        return Err(format!(
            "mesh 画出的条目集合 {:?} ≠ 逐格解算的集合 {:?}：渲染器读到的不是解算器算出的那张图",
            list(&layer_entries),
            list(&grid_entries)
        )
        .into());
    }
    let mut want = BTreeMap::new();
    for entry in &grid.entries {
        if *entry != 0 {
            *want.entry(*entry).or_insert(0usize) += 1;
        }
    }
    let mut got = BTreeMap::new();
    for (entry, _) in &layer {
        *got.entry(*entry).or_insert(0usize) += 1;
    }
    if want != got {
        return Err(format!(
            "整层绘制与逐格解算对不上：解算 {} 格 {} 种条目，绘制 {} 格 {} 种条目",
            want.values().sum::<usize>(),
            want.len(),
            got.values().sum::<usize>(),
            got.len()
        )
        .into());
    }
    if layer_entries.len() < MIN_SCREEN_TILES {
        return Err(format!(
            "整层去重只有 {} 个图集条目，应 ≥{MIN_SCREEN_TILES}（旧数据是 5）",
            layer_entries.len()
        )
        .into());
    }
    println!(
        "  {width}x{height} 网格、{} 格全非空；图集条目去重 {} 种（旧值 5），mesh 与 atlas_index 逐条目多重集全等",
        grid.entries.len(),
        layer_entries.len()
    );

    banner("② 一屏可见瓦种类 ≥24：实机相机与最坏窗口两个口径");
    runtime.set_tilemap_culling(true);
    let screen = appearances_for(&mut runtime, &texture)?;
    let screen_kinds: BTreeSet<Appearance> = screen.iter().copied().collect();
    let screen_entries: BTreeSet<u8> = screen.iter().map(|(entry, _)| *entry).collect();
    if screen_kinds.len() < MIN_SCREEN_TILES {
        return Err(format!(
            "实机默认相机这一屏只画出 {} 种瓦（纯条目 {} 种），应 ≥{MIN_SCREEN_TILES}",
            screen_kinds.len(),
            screen_entries.len()
        )
        .into());
    }
    let (worst_count, worst_at) = worst_window(&grid);
    if worst_count < MIN_SCREEN_TILES {
        return Err(format!(
            "最坏的 {SCREEN_W}x{SCREEN_H} 窗口只有 {worst_count} 种瓦（左上角格 {worst_at:?}），\
             应 ≥{MIN_SCREEN_TILES}：一屏的多样性不能靠相机恰好站在热闹处"
        )
        .into());
    }
    println!(
        "  实机一屏绘制 {} 格 = {} 种外观（纯条目 {} 种）；数据侧所有 {SCREEN_W}x{SCREEN_H} 窗口的最小值 {worst_count} 种（左上角 {worst_at:?}）",
        screen.len(),
        screen_kinds.len(),
        screen_entries.len()
    );

    banner("③ 交界取压边变体；天然地貌之间不许一整条直线");
    let leaked = full_block_leaks(&grid);
    if !leaked.is_empty() {
        return Err(format!(
            "{} 格交界格取到了满块瓦（首格 {:?}）：autotile 解算没生效，边界会画成一条硬线",
            leaked.len(),
            leaked[0]
        )
        .into());
    }
    let mut report = Vec::new();
    for a in ORGANIC_KINDS {
        for b in ORGANIC_KINDS {
            if a == b {
                continue;
            }
            let run = longest_border_run(&grid, a, b);
            if run.length > MAX_STRAIGHT_BORDER_RUN {
                return Err(format!(
                    "kind {a}→{b} 的交界上有一段 {} 格连排**外观完全相同**的瓦（起点 {:?}）：\
                     超过 {MAX_STRAIGHT_BORDER_RUN} 格就是一条直线",
                    run.length, run.at
                )
                .into());
            }
            report.push(format!("{a}→{b}:{}格/{} 交界", run.length, run.bordered));
        }
    }
    let structural: Vec<String> = [3u8, 4]
        .iter()
        .flat_map(|&a| [1u8, 2, 3, 4, 6].into_iter().map(move |b| (a, b)))
        .filter(|(a, b)| a != b && grid.kinds.contains(b))
        .map(|(a, b)| {
            let run = longest_border_run(&grid, a, b);
            format!("{a}→{b}:{}", run.length)
        })
        .collect();
    // 反面对照：合成一张"草地嵌规则土方矩形"的地图（重做前 manor 的几何读法）。
    // 过渡块照样解算，但矩形的一条边就是直线——本门的统计必须抓得住它。
    let legacy = longest_border_run(&synthetic_rect_grid(20, 14, 4, 2, 8, 8), 1, 2);
    if legacy.length <= MAX_STRAIGHT_BORDER_RUN {
        return Err(format!(
            "反面对照失手：规则矩形的草→土交界只报出 {} 格同外观直边（应 >{MAX_STRAIGHT_BORDER_RUN}），\
             §③ 那条断言其实是恒绿的",
            legacy.length
        )
        .into());
    }
    println!("  0 格交界取到满块瓦；天然地貌交界：{}", report.join(" "));
    println!(
        "  人工结构（路/墙，直的是应该的，仅报数）：{}",
        structural.join(" ")
    );
    println!(
        "  反面对照：合成旧图（草地嵌 8x8 规则土块）报出 {} 格同外观直边 → §③ 会红",
        legacy.length
    );

    banner("④ 色阶门：每个在用料族实测 ≥3 档明度（基础图 + 四季图）");
    let base = decode_atlas(&project, GROUND_ATLAS)?;
    let mut base_report = Vec::new();
    for family in in_use_families() {
        let levels = family_levels_rgb(family);
        // 表本身的单调性也在这里查：档序就是美术契约，写反了生成器会照着反的画。
        let count = check_family(&base, family, &levels)?;
        base_report.push(format!("{family}={count}档"));
    }
    for path in SEASON_ATLAS {
        let sheet = decode_atlas(&project, path)?;
        let remap = positional_remap(&base, &sheet, path)?;
        for family in in_use_families() {
            // 季节图的档 = 基础表该族的档经搬色映射后的像（KEEP 的族映射里没有，原色自守）。
            let levels: Vec<[u8; 3]> = family_levels_rgb(family)
                .iter()
                .map(|rgb| remap.get(rgb).copied().unwrap_or(*rgb))
                .collect();
            check_family(&sheet, family, &levels).map_err(|error| format!("{path}：{error}"))?;
        }
    }
    // 反面对照：把草族压成单档再走**同一个**函数，必须红（门不是"族存在就算绿"）。
    let grass = family_levels_rgb("grass");
    let collapsed = flatten_family(&base, "grass", grass[0]);
    match check_family(&collapsed, "grass", &[grass[0]]) {
        Ok(_) => {
            return Err("反面对照失手：草族压成单档后 §④ 仍判绿，明度断言其实没在看颜色".into());
        }
        Err(error) => println!("  反面对照：草族压成单档即红（{error}）"),
    }
    println!(
        "  基础图集：{}；四季图各族经同位置置换还原后同判",
        base_report.join(" ")
    );

    banner("⑤ 调色板归属：每个不透明像素等于色阶表；尾部空槽全透明");
    let palette = table_palette();
    let offenders = off_palette(&base, &palette);
    if !offenders.is_empty() {
        return Err(format!(
            "基础图集有 {} 个色不在色阶表里（前 6 个：{:?}）：生成器不许现写 hex",
            offenders.len(),
            &offenders[..offenders.len().min(6)]
        )
        .into());
    }
    let painted: Vec<String> = TAIL_SLOTS
        .iter()
        .filter(|slot| !opaque_colors(&base, &[**slot]).is_empty())
        .map(|slot| slot.to_string())
        .collect();
    if !painted.is_empty() {
        return Err(format!(
            "空槽条目 {painted:?} 里有像素：没有任何 kind 寻址到的槽位必须全透明，\
             越界的格子才不会画成邻居的美术"
        )
        .into());
    }
    // 反面对照：从表副本删掉草族第 3 档，offender 必须正好抓到它并点名。
    let dropped = grass[2];
    let mut shrunk = palette.clone();
    shrunk.remove(&dropped);
    let shrunk_offenders = off_palette(&base, &shrunk);
    if !shrunk_offenders.contains(&hex(dropped)) {
        return Err(format!(
            "反面对照失手：从表里删掉 {} 之后 offender 集合仍不含它，§⑤ 的归属检查是瞎的",
            hex(dropped)
        )
        .into());
    }
    println!(
        "  基础图集 {} 个不透明色全在表内、6 个空槽全透明；反面对照（删 {}）报出 {} 个 offender 并点名它",
        opaque_colors(&base, &(1..=ATLAS_ENTRIES as u8).collect::<Vec<_>>()).len(),
        hex(dropped),
        shrunk_offenders.len()
    );

    banner("⑥ 满块瓦的重复由 tint 打散：三档染色都真的落在满块格上");
    let mut used = [0usize; 3];
    let mut on_full = [0usize; 3];
    for index in 0..grid.kinds.len() {
        let tint = usize::from(grid.tints[index]);
        used[tint] += 1;
        if grid.entries[index] != 0 && grid.entries[index] == full_block_entry(grid.kinds[index]) {
            on_full[tint] += 1;
        }
    }
    if on_full.contains(&0) {
        return Err(format!(
            "满块瓦上的染色分布 = {on_full:?}（0=白 1=shaded 2=sunlit），有一档一个都没落上：\
             §② 的多样性就不是\"靠染色把重复打散\""
        )
        .into());
    }
    println!(
        "  整层染色 白{}/shaded{}/sunlit{}；满块瓦上三档齐备 {on_full:?}（§② 的读数含染色档）",
        used[0], used[1], used[2]
    );

    println!(
        "\njhyt_m72_manor_terrain_ok project={} atlas_entries={} screen_tiles={} worst_window={} straight_max={}",
        project.display(),
        layer_entries.len(),
        screen_kinds.len(),
        worst_count,
        MAX_STRAIGHT_BORDER_RUN
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// 色阶表：门内副本与从它算出来的东西
// ---------------------------------------------------------------------------

fn family_levels(family: &str) -> &'static [u32] {
    FAMILY_LEVELS
        .iter()
        .find(|(name, _)| *name == family)
        .map(|(_, levels)| *levels)
        .unwrap_or_else(|| panic!("色阶表里没有族 {family}"))
}

fn family_levels_rgb(family: &str) -> Vec<[u8; 3]> {
    family_levels(family)
        .iter()
        .map(|&hex| to_rgb(hex))
        .collect()
}

/// 有 kind 挂上的族（hedge 不在此列：它是跨族点缀色——院墙的草帽顶、地图边缘的视觉墙，
/// 自己没有图集块，档数由 §⑤ 的表内单调与归属检查兜住）。
fn in_use_families() -> Vec<&'static str> {
    let mut seen = BTreeSet::new();
    for family in KIND_FAMILY.iter().skip(1) {
        if !family.is_empty() {
            seen.insert(*family);
        }
    }
    seen.into_iter().collect()
}

fn to_rgb(value: u32) -> [u8; 3] {
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// Rec.601 亮度。门用它复核"表里写的顺序确实是深→浅"，与 Python 侧的 sRGB 相对亮度
/// **故意不同**：两个公式都要求同族四档单调，同时写错的概率才是零。
fn luma(rgb: [u8; 3]) -> f32 {
    0.299 * f32::from(rgb[0]) + 0.587 * f32::from(rgb[1]) + 0.114 * f32::from(rgb[2])
}

/// kind 的**满块**条目：autotile 开时掩码 `0b1111`（四邻同种）落的那一格。
fn full_block_entry(kind: u8) -> u8 {
    (1 + KINDS + usize::from(kind - 1) * AUTOTILE_SLOTS + (AUTOTILE_SLOTS - 1)) as u8
}

fn kind_entries(kind: u8) -> Vec<u8> {
    let block = 1 + KINDS + (usize::from(kind) - 1) * AUTOTILE_SLOTS;
    let mut out = vec![IDENTITY_ENTRIES[usize::from(kind) - 1]];
    out.extend((block..block + AUTOTILE_SLOTS).map(|index| index as u8));
    out
}

/// 一族的全部条目（石族同时 owning 石板与院墙，两块都要看）。
fn family_entries(family: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for kind in 1..=KINDS as u8 {
        if KIND_FAMILY[usize::from(kind)] == family {
            out.extend(kind_entries(kind));
        }
    }
    out.sort_unstable();
    out
}

// ---------------------------------------------------------------------------
// 图集：解码、按条目取区、实测色阶
// ---------------------------------------------------------------------------

/// 解好码的一张图集（RGBA8 扁平数组，行序 = UV 的 v，v=0 在贴图顶部）。
#[derive(Clone)]
struct Sheet {
    width: usize,
    height: usize,
    pixels: Vec<[u8; 4]>,
}

impl Sheet {
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        self.pixels[y * self.width + x]
    }
}

fn decode_atlas(project: &Path, path: &str) -> Result<Sheet, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(project.join(path)).map_err(|error| format!("{path}: {error}"))?;
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("{path}: {error}"))?
        .to_rgba8();
    let (w, h) = (image.width() as usize, image.height() as usize);
    let want = (ATLAS_COLS * TILE_PX, ATLAS_ROWS * TILE_PX);
    if (w, h) != want {
        return Err(format!(
            "{path} 实测 {w}x{h}，应为 {want:?}（= atlas_cols×atlas_rows×tile）：\
             贴图尺寸与组件声明不符，切 UV 必错位（docs/57 §1 性能预算）"
        )
        .into());
    }
    Ok(Sheet {
        width: w,
        height: h,
        pixels: image
            .into_raw()
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect(),
    })
}

/// 条目 `(col,row)` 的原点：图集 row 0 在**贴图顶部**（地图的 row 0 在最底，两个坐标系相反）。
fn entry_origin(entry: u8) -> (usize, usize) {
    let position = usize::from(entry) - 1;
    (
        (position % ATLAS_COLS) * TILE_PX,
        (position / ATLAS_COLS) * TILE_PX,
    )
}

/// 若干条目区里出现过的不透明色。
fn opaque_colors(sheet: &Sheet, entries: &[u8]) -> BTreeSet<[u8; 3]> {
    let mut out = BTreeSet::new();
    for entry in entries {
        let (ox, oy) = entry_origin(*entry);
        for y in 0..TILE_PX {
            for x in 0..TILE_PX {
                let pixel = sheet.pixel(ox + x, oy + y);
                if pixel[3] != 0 {
                    out.insert([pixel[0], pixel[1], pixel[2]]);
                }
            }
        }
    }
    out
}

/// 色阶门的一条判据：给定一族与它的档色（深→浅），亮度必须严格递增，
/// 且这些色在该族的条目区里**实测到**的档数 ≥[`MIN_FAMILY_LEVELS`]。
///
/// 基础图、四季图与反面对照走的都是这同一个函数——反面对照才有意义的前提。
fn check_family(sheet: &Sheet, family: &str, levels: &[[u8; 3]]) -> Result<usize, String> {
    for pair in levels.windows(2) {
        if luma(pair[0]) >= luma(pair[1]) {
            return Err(format!(
                "{family} 族的档色亮度不递增（{:?}）：表里写的顺序就是深→浅，反了生成器会照着反的画",
                levels.iter().map(|rgb| hex(*rgb)).collect::<Vec<_>>()
            ));
        }
    }
    let seen = opaque_colors(sheet, &family_entries(family));
    let present: Vec<String> = levels
        .iter()
        .filter(|rgb| seen.contains(*rgb))
        .map(|rgb| hex(*rgb))
        .collect();
    if present.len() < MIN_FAMILY_LEVELS {
        return Err(format!(
            "{family} 族在图集里只实测到 {} 档明度（{present:?}），应 ≥{MIN_FAMILY_LEVELS}：\
             一族只有一档就是平色，docs/53 §2 的星露谷口径是 3–5 档",
            present.len()
        ));
    }
    Ok(present.len())
}

/// 表里允许出现在地表的色（材质族 ∪ utility）。
fn table_palette() -> BTreeSet<[u8; 3]> {
    let mut out = BTreeSet::new();
    for (_, levels) in FAMILY_LEVELS {
        for level in levels.iter() {
            out.insert(to_rgb(*level));
        }
    }
    for utility in UTILITY_COLORS.iter() {
        out.insert(to_rgb(*utility));
    }
    out
}

/// 不在表里的色（大写 hex 报出来，方便照着补表或改回去）。
fn off_palette(sheet: &Sheet, palette: &BTreeSet<[u8; 3]>) -> Vec<String> {
    let all: Vec<u8> = (1..=ATLAS_ENTRIES as u8).collect();
    opaque_colors(sheet, &all)
        .iter()
        .filter(|rgb| !palette.contains(*rgb))
        .map(|rgb| hex(*rgb))
        .collect()
}

/// 把某族整族压成单档（只留最深那一档），只在内存里合成、不落盘：§④ 的反面对照用。
fn flatten_family(sheet: &Sheet, family: &str, keep: [u8; 3]) -> Sheet {
    let levels = family_levels_rgb(family);
    let mut out = sheet.clone();
    for entry in family_entries(family) {
        let (ox, oy) = entry_origin(entry);
        for y in 0..TILE_PX {
            for x in 0..TILE_PX {
                let index = (oy + y) * out.width + ox + x;
                let pixel = out.pixels[index];
                if levels.contains(&[pixel[0], pixel[1], pixel[2]]) {
                    out.pixels[index] = [keep[0], keep[1], keep[2], pixel[3]];
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 四季变体：与基础图的关系必须是"同位置的一一换色"
// ---------------------------------------------------------------------------

/// 季节变体图与基础图的关系：**同位置的颜色置换**（构图一字不动，只换色）。
/// 返回 `基础色 → 季节色`。任一方向不唯一就报错——那说明季节图被重画过而不是换过色，
/// 于是"同族明度档数"在季节图里根本没法对齐基础表（而 §④ 要靠这个映射去查季节图）。
fn positional_remap(
    base: &Sheet,
    season: &Sheet,
    label: &str,
) -> Result<BTreeMap<[u8; 3], [u8; 3]>, Box<dyn std::error::Error>> {
    if (base.width, base.height) != (season.width, season.height) {
        return Err(
            format!("{label}：尺寸与基础图不符（换季只换 texture=，图集几何必须同构）").into(),
        );
    }
    let mut forward: BTreeMap<[u8; 3], [u8; 3]> = BTreeMap::new();
    let mut backward: BTreeMap<[u8; 3], [u8; 3]> = BTreeMap::new();
    for index in 0..base.pixels.len() {
        let from = base.pixels[index];
        let to = season.pixels[index];
        if (from[3] == 0) != (to[3] == 0) {
            return Err(format!(
                "{label}：像素 {index} 的透明与否与基础图不一致——季节变体只许换色，不许改构图"
            )
            .into());
        }
        if from[3] == 0 {
            continue;
        }
        let (from_rgb, to_rgb) = ([from[0], from[1], from[2]], [to[0], to[1], to[2]]);
        match forward.get(&from_rgb) {
            None => {
                forward.insert(from_rgb, to_rgb);
            }
            Some(&seen) if seen != to_rgb => {
                return Err(format!(
                    "{label}：基础色 {} 在同一张图上被搬成了 {} 与 {}——一族的一档在这里断了",
                    hex(from_rgb),
                    hex(seen),
                    hex(to_rgb)
                )
                .into());
            }
            Some(_) => {}
        }
        match backward.get(&to_rgb) {
            None => {
                backward.insert(to_rgb, from_rgb);
            }
            Some(&seen) if seen != from_rgb => {
                return Err(format!(
                    "{label}：季节色 {} 同时来自基础色 {} 与 {}——搬色不许把两档合并成一档",
                    hex(to_rgb),
                    hex(seen),
                    hex(from_rgb)
                )
                .into());
            }
            Some(_) => {}
        }
    }
    Ok(forward)
}

// ---------------------------------------------------------------------------
// 地表层取数：走 Lua，与渲染器共用同一份解算
// ---------------------------------------------------------------------------

/// 地表的逐格数据：格子值（逻辑 kind）、解算出的图集条目、染色档位（0 白 / 1 / 2）。
struct GroundGrid {
    width: usize,
    height: usize,
    kinds: Vec<u8>,
    entries: Vec<u8>,
    tints: Vec<u8>,
}

const NEIGHBOR_DELTAS: [(isize, isize); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

impl GroundGrid {
    fn kind_at(&self, x: usize, y: usize) -> u8 {
        self.kinds[y * self.width + x]
    }

    fn neighbor(&self, x: usize, y: usize, dx: isize, dy: isize) -> u8 {
        let (nx, ny) = (x as isize + dx, y as isize + dy);
        if nx < 0 || ny < 0 || nx >= self.width as isize || ny >= self.height as isize {
            return 0;
        }
        self.kind_at(nx as usize, ny as usize)
    }

    fn has_neighbor_kind(&self, x: usize, y: usize, kind: u8) -> bool {
        NEIGHBOR_DELTAS
            .iter()
            .any(|&(dx, dy)| self.neighbor(x, y, dx, dy) == kind)
    }
}

/// 交界格却取到满块瓦 = 解算没看邻居（autotile 被关掉、或掩码把异种也算成邻居）。
fn full_block_leaks(grid: &GroundGrid) -> Vec<(usize, usize, u8)> {
    let mut out = Vec::new();
    for y in 0..grid.height {
        for x in 0..grid.width {
            let kind = grid.kind_at(x, y);
            if kind == 0 {
                continue;
            }
            let index = y * grid.width + x;
            let differs = NEIGHBOR_DELTAS
                .iter()
                .any(|&(dx, dy)| grid.neighbor(x, y, dx, dy) != kind);
            if differs && grid.entries[index] == full_block_entry(kind) {
                out.push((x, y, kind));
            }
        }
    }
    out
}

struct BorderRun {
    length: usize,
    at: (usize, usize),
    bordered: usize,
}

/// kind `a` 贴着 kind `b` 的那些格里，同一条直线（同排或同列）上连续外观完全相同的
/// 最长一段有多长。这就是"一整条直线边界"的可数形式。
fn longest_border_run(grid: &GroundGrid, kind: u8, other: u8) -> BorderRun {
    let mut appearance: Vec<Option<Appearance>> = vec![None; grid.kinds.len()];
    let mut bordered = 0usize;
    for y in 0..grid.height {
        for x in 0..grid.width {
            if grid.kind_at(x, y) == kind && grid.has_neighbor_kind(x, y, other) {
                let index = y * grid.width + x;
                appearance[index] = Some((grid.entries[index], grid.tints[index]));
                bordered += 1;
            }
        }
    }
    let mut best = BorderRun {
        length: 0,
        at: (0, 0),
        bordered,
    };
    let mut scan = |points: &[(usize, usize)]| {
        let mut run: Vec<usize> = Vec::new();
        for &(x, y) in points {
            let index = y * grid.width + x;
            let current = appearance[index];
            let same =
                current.is_some() && run.last().is_some_and(|&last| appearance[last] == current);
            if same {
                run.push(index);
            } else {
                if run.len() > best.length {
                    best.length = run.len();
                    best.at = (run[0] % grid.width, run[0] / grid.width);
                }
                run = if current.is_some() {
                    vec![index]
                } else {
                    Vec::new()
                };
            }
        }
        if run.len() > best.length {
            best.length = run.len();
            best.at = (run[0] % grid.width, run[0] / grid.width);
        }
    };
    let mut row = Vec::with_capacity(grid.width);
    for y in 0..grid.height {
        row.clear();
        row.extend((0..grid.width).map(|x| (x, y)));
        scan(&row);
    }
    let mut column = Vec::with_capacity(grid.height);
    for x in 0..grid.width {
        column.clear();
        column.extend((0..grid.height).map(|y| (x, y)));
        scan(&column);
    }
    best
}

/// 最坏的 `{SCREEN_W}x{SCREEN_H}` 窗口：`(外观种数, 窗口左上角格)`。
fn worst_window(grid: &GroundGrid) -> (usize, (usize, usize)) {
    let mut best = (usize::MAX, (0, 0));
    if grid.width < SCREEN_W || grid.height < SCREEN_H {
        return (0, (0, 0));
    }
    for oy in 0..=(grid.height - SCREEN_H) {
        for ox in 0..=(grid.width - SCREEN_W) {
            let mut kinds = BTreeSet::new();
            for y in oy..oy + SCREEN_H {
                for x in ox..ox + SCREEN_W {
                    let index = y * grid.width + x;
                    if grid.entries[index] != 0 {
                        kinds.insert((grid.entries[index], grid.tints[index]));
                    }
                }
            }
            if kinds.len() < best.0 {
                best = (kinds.len(), (ox, oy));
            }
        }
    }
    best
}

/// 合成"草地嵌规则土方矩形"的旧读法地图（§③ 的反面对照）：kind 语义、12×9 图集与
/// autotile 全都照现在这样生效，只有几何是直角矩形——重做前 manor 就是这个形状。
fn synthetic_rect_grid(
    w: usize,
    h: usize,
    fx: usize,
    fy: usize,
    fw: usize,
    fh: usize,
) -> GroundGrid {
    let mut kinds = vec![1u8; w * h];
    for y in fy..fy + fh {
        for x in fx..fx + fw {
            kinds[y * w + x] = 2;
        }
    }
    let mut map =
        Tilemap::new(Vec2::new(TILE_PX as f32, TILE_PX as f32), w, h).expect("合成地图尺寸合法");
    map.texture = GROUND_ATLAS.to_owned();
    map.atlas_cols = ATLAS_COLS;
    map.atlas_rows = ATLAS_ROWS;
    map.tile_kinds = KINDS;
    map.autotile = true;
    map.tiles = kinds.clone();
    map.validate_layout().expect("合成地图应落在 12x9 图集内");
    let entries = (0..w * h)
        .map(|index| map.resolve_atlas_index(index % w, index / w))
        .collect();
    GroundGrid {
        width: w,
        height: h,
        kinds,
        entries,
        tints: vec![0; w * h],
    }
}

/// 一次 Lua 往返读回整层：格子值、`atlas_index`（引擎解算）、染色（按值去重后的档位表）。
fn read_ground(
    runtime: &mut RuntimeScene,
    ground: i64,
    width: usize,
    height: usize,
) -> Result<GroundGrid, Box<dyn std::error::Error>> {
    let raw = lua_str(
        runtime,
        &format!(
            r#"(function()
                 local ids, entries, codes, keys, seen = {{}}, {{}}, {{}}, {{}}, {{}}
                 for y = 0, {height} - 1 do
                   for x = 0, {width} - 1 do
                     ids[#ids + 1] = tilemap.get_cell({ground}, x, y)
                     entries[#entries + 1] = tilemap.atlas_index({ground}, x, y)
                     local c = tilemap.tint({ground}, x, y)
                     local k = string.format('%0.3f/%0.3f/%0.3f', c.r, c.g, c.b)
                     local n = seen[k]
                     if n == nil then n = #keys + 1; seen[k] = n; keys[n] = k end
                     codes[#codes + 1] = n
                   end
                 end
                 return table.concat(ids, ',') .. ';' .. table.concat(entries, ',') .. ';'
                   .. table.concat(codes, ',') .. ';' .. table.concat(keys, '|')
               end)()"#
        ),
    )?;
    let mut parts = raw.split(';');
    let take = |part: Option<&str>, name: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        split_numbers(part.ok_or(format!("readback 缺 {name} 段"))?)
    };
    let kinds = take(parts.next(), "格子值")?;
    let entries = take(parts.next(), "条目")?;
    let codes = take(parts.next(), "染色档")?;
    let keys = parts.next().ok_or("readback 缺染色表段")?.to_owned();
    if kinds.len() != width * height || entries.len() != kinds.len() || codes.len() != kinds.len() {
        return Err(format!(
            "readback 长度不齐：格 {} / 条目 {} / 染色 {}，应为 {} 格",
            kinds.len(),
            entries.len(),
            codes.len(),
            width * height
        )
        .into());
    }
    let mut buckets = Vec::new();
    for key in keys.split('|').filter(|key| !key.is_empty()) {
        let color = parse_rgb(key)?;
        buckets.push(tint_bucket(color).ok_or(format!(
            "地表出现色阶表之外的染色 {key}：只允许白与 ground_tints 的两档"
        ))?);
    }
    if buckets.is_empty() {
        buckets.push(0);
    }
    let tints: Vec<u8> = codes
        .iter()
        .map(|&code| buckets[usize::from(code) - 1])
        .collect();
    Ok(GroundGrid {
        width,
        height,
        kinds,
        entries,
        tints,
    })
}

fn split_numbers(text: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    text.split(',')
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.trim()
                .parse::<u8>()
                .map_err(|_| format!("读回 {item:?} 不是 0..=255 的格值").into())
        })
        .collect()
}

fn parse_rgb(key: &str) -> Result<[f32; 3], Box<dyn std::error::Error>> {
    let mut out = [0.0f32; 3];
    for (slot, part) in out.iter_mut().zip(key.split('/')) {
        *slot = part
            .trim()
            .parse::<f32>()
            .map_err(|_| format!("染色档 {key} 解析失败"))?;
    }
    Ok(out)
}

/// 顶点色归档：必须与 `TINT_WHITE` / `GROUND_TINTS` 之一对上（容差 1e-3——组件串里
/// 写的就是两位小数，落到 f32 飘不出这个量级）。
fn tint_bucket(color: [f32; 3]) -> Option<u8> {
    [TINT_WHITE, GROUND_TINTS[0], GROUND_TINTS[1]]
        .iter()
        .position(|c| {
            c.iter()
                .zip(color.iter())
                .all(|(want, got)| (want - got).abs() < 1e-3)
        })
        .map(|index| index as u8)
}

// ---------------------------------------------------------------------------
// mesh 取数：UV 反解条目 + 顶点色，两者一起才是屏幕上的那一格
// ---------------------------------------------------------------------------

/// 某张地表贴图本帧画出的外观序列。overlay 与世界深度两条通道都查，且必须只有一条有货。
fn appearances_for(
    runtime: &mut RuntimeScene,
    path: &str,
) -> Result<Vec<Appearance>, Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let overlay: Vec<DrawRange> = mesh
        .draws
        .iter()
        .filter(|draw| path_of(&mesh, draw.texture) == Some(path))
        .map(|draw| (draw.start as usize, draw.count as usize))
        .collect();
    let world: Vec<DrawRange> = mesh
        .world_sprite_draws
        .iter()
        .filter(|draw| path_of(&mesh, draw.texture_id) == Some(path))
        .map(|draw| (draw.vertex_start, draw.vertex_count))
        .collect();
    if !overlay.is_empty() && !world.is_empty() {
        return Err(format!("{path} 同时出现在 overlay 与世界深度通道：瓦片层只该走一条").into());
    }
    let (ranges, vertices): (Vec<DrawRange>, Vec<UvColor>) = if overlay.is_empty() {
        (
            world,
            mesh.world_sprites
                .iter()
                .map(|vertex| (vertex.uv, vertex.color))
                .collect(),
        )
    } else {
        (
            overlay,
            mesh.vertices
                .iter()
                .map(|vertex| (vertex.uv, vertex.color))
                .collect(),
        )
    };
    let mut out = Vec::new();
    for (start, count) in ranges {
        for quad in vertices[start..start + count].chunks_exact(6) {
            let (uv, color) = quad[0];
            let entry = entry_of_uv(uv)?;
            let tint = tint_bucket([color[0], color[1], color[2]]).ok_or(format!(
                "{path} 条目 {entry} 的顶点色 {:?} 不在色阶表的染色档里",
                color
            ))?;
            out.push((entry, tint));
        }
    }
    Ok(out)
}

/// UV 左下角 → 图集条目：`Tilemap::atlas_uv` 的逆运算，按声明的列数行数。
fn entry_of_uv(uv: [f32; 2]) -> Result<u8, Box<dyn std::error::Error>> {
    let col = (uv[0] * ATLAS_COLS as f32).round() as usize;
    let row = (uv[1] * ATLAS_ROWS as f32).round() as usize;
    if col >= ATLAS_COLS || row >= ATLAS_ROWS {
        return Err(format!("UV {uv:?} 落在图集之外").into());
    }
    Ok((row * ATLAS_COLS + col + 1) as u8)
}

fn path_of(mesh: &SceneMesh, texture: u32) -> Option<&str> {
    if texture == 0 {
        return None;
    }
    mesh.paths.get((texture - 1) as usize).map(String::as_str)
}

fn list(values: &BTreeSet<u8>) -> Vec<u8> {
    values.iter().copied().collect()
}

// ---------------------------------------------------------------------------
// 宿主脚手架
// ---------------------------------------------------------------------------

/// 世界里第一个带 Tilemap 的**地编**实体 id（各场景命名不一，按候选名依次找）。
/// id 必须按整数读回：带代际的大整数走 f32 会掉精度。
fn first_tilemap_entity(runtime: &mut RuntimeScene) -> Result<i64, Box<dyn std::error::Error>> {
    let id = lua_id(
        runtime,
        r#"(function()
             for _, name in ipairs({'GroundMap','ManorGround','Ground','ground'}) do
               local id = entity.find(name)
               if id and tilemap.get(id) then return id end
             end
             return 0
           end)()"#,
    )?;
    if id == 0 {
        return Err(
            "场景里找不到带 Tilemap 的地面实体（候选名：GroundMap/ManorGround/Ground）".into(),
        );
    }
    Ok(id)
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

fn lua_id(runtime: &mut RuntimeScene, expr: &str) -> Result<i64, Box<dyn std::error::Error>> {
    let text = lua_str(runtime, expr)?;
    text.parse::<i64>()
        .map_err(|_| format!("{expr} 不是实体 id（读回 {text:?}）").into())
}

/// 把 Lua 表读成 `key=value` 行（`tilemap.get` 的字段面，不逐个 round-trip）。
fn lua_table(
    runtime: &mut RuntimeScene,
    expr: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let text = lua_str(
        runtime,
        &format!(
            r#"(function()
                 local t = {expr}
                 if type(t) ~= 'table' then return 'MISSING' end
                 local out = {{}}
                 for k, v in pairs(t) do out[#out + 1] = tostring(k) .. '=' .. tostring(v) end
                 table.sort(out)
                 return table.concat(out, ';')
               end)()"#
        ),
    )?;
    if text == "MISSING" {
        return Err(format!("{expr} 没有返回表").into());
    }
    Ok(text
        .split(';')
        .filter(|row| !row.is_empty())
        .map(|row| {
            let (key, value) = row.split_once('=').unwrap_or((row, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect())
}

fn field_num(fields: &[(String, String)], key: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let raw = fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| format!("表里没有字段 {key}"))?;
    raw.parse::<f64>()
        .map_err(|_| format!("字段 {key} = {raw:?} 不是数字").into())
}

fn field_str(fields: &[(String, String)], key: &str) -> Result<String, Box<dyn std::error::Error>> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty() && value != "nil")
        .ok_or_else(|| format!("字段 {key} 缺失或为空").into())
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
