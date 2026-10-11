//! Headless acceptance drive for M72-01：**图集多行多列寻址 + `.scene` 的 tiles 编码迁移**。
//!
//! 需求（`tasks/done/M72-01-*.md`）：tilemap 今天只能寻址"6 列 1 行、V 恒拉伸"的图集，
//! 重画多少瓦片都塞不进去。本门钉的是**解算与序列化两层都真的放开了**，且旧数据没被改动。
//!
//! 为什么读顶点不读像素：UV 落在图集的哪个子矩形是纯数据结论，`SceneMesh` 已经把顶点交出来
//! （`mesh.draws[i]` = `{texture, start, count}`，`texture == 0` 是内建白图、`> 0` 取
//! `paths[texture - 1]`；每格 6 顶点，第 0 个是 `uv_min`、第 2 个是 `uv_max`）。
//! 拿像素当证据的话，"第二行的格子"到底红在哪一格就说不清了。
//!
//! 章节：
//!   ① 12×8 图集逐格 UV：造一张 5×4 的探针地图、12 列 8 行，格子值铺满第 0..7 行，
//!      断言**每一格的 UV 正好是它那一格的图集子矩形**（顺序 = 块行主序 = y 再 x，跳空格）；
//!   ② 索引 0 不产出 quad：探针里留两个空格，绘制顶点数必须等于非空格数 ×6（多一个都红）；
//!   ③ 反面对照（内建，两处）：把同一张地图的 `atlas_rows` 改回 1，
//!      (a) 纯数据侧 `atlas_uv(13)` 的 V 必须**跑出 [0,1]**（越界到图集外面去采样），
//!          即 ① 那批断言对"多行"这件事不是瞎的；
//!      (b) 运行时侧 `tilemap.set_atlas(id,12,1)` 必须**报错点名越界的那一格**，
//!          引擎不许把"贴图其实只有一行"这件事静默吞掉；
//!   ④ 旧编码不许被迁移改动语义：`tiles_format=0` 的组件串读回来仍是"按 6 列切 U、
//!      V 整幅拉伸 0..1"，且仍只认 1..=6；
//!   ⑤ 随场景发布的数据真的在新编码上：山庄地面 `tiles_format=1`，格子按声明的
//!      `atlas_cols × atlas_rows` 分带（M72-04 起这张地面是 12×9，更早是 6×1 整幅拉伸），
//!      而且**渲染出来的格子数 = Lua 逐格数出来的非空格数**（这条同时兜住 chunk 脏重建与
//!      "逗号编码读漏一格"两类事故）。
//!
//! 探针贴图路径指向一张**不存在**的文件：`texture_id_of` 只登记路径不做 IO，UV 解算与像素
//! 无关。等 M72-04 出真的 12×8 图集时，这条门仍然只该校验 UV，不该依赖那张 PNG 长什么样。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_tileset_atlas [-- <game_jhyt-root>]

use engine::host::{RuntimeScene, SceneMesh};
use rustlua_ecs::tilemap::{TILES_FORMAT_ATLAS, TILES_FORMAT_LEGACY, Tilemap};
use rustlua_ecs::transform::Vec2;
use rustlua_input::input::InputSnapshot;
use std::{env, path::PathBuf};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;
const PROBE_TILESET: &str = "assets/art/tilesets/m72/probe_12x8.png";
const PROBE_WIDTH: usize = 5;
const PROBE_HEIGHT: usize = 4;
const PROBE_COLS: usize = 12;
const PROBE_ROWS: usize = 8;
/// 探针格子内容（行主序，**row 0 在地图最底**）。铺满图集第 0..7 行各一次以上，
/// 并留两个 0 空格给 §②。
const PROBE_CELLS: [u8; PROBE_WIDTH * PROBE_HEIGHT] = [
    1, 12, 13, 24, 0, // 图集行 0 与行 1
    25, 36, 37, 48, 2, // 行 2 与行 3
    49, 60, 61, 72, 0, // 行 4 与行 5
    73, 84, 85, 96, 6, // 行 6 与行 7
];

/// 一个 quad 的 UV：左下角、右上角。
type UvQuad = ([f32; 2], [f32; 2]);

/// 一格图集条目对应的 UV 子矩形（与 `Tilemap::atlas_uv` 同一套算术，故意各写一遍：
/// 两边都写错成同一种错位，这条门就白装了）。
fn expected_uv(entry: u8) -> UvQuad {
    let position = f32::from(entry - 1);
    let col = position % PROBE_COLS as f32;
    let row = (position / PROBE_COLS as f32).floor();
    (
        [col / PROBE_COLS as f32, row / PROBE_ROWS as f32],
        [
            (col + 1.0) / PROBE_COLS as f32,
            (row + 1.0) / PROBE_ROWS as f32,
        ],
    )
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
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;

    banner("① 12×8 图集：每格的 UV 落在正确的 (col,row) 子矩形");
    let probe = spawn_probe(&mut runtime)?;
    let layout = lua_table(&mut runtime, &format!("tilemap.get({probe})"))?;
    for (key, want) in [
        ("tiles_format", TILES_FORMAT_ATLAS as f64),
        ("atlas_cols", PROBE_COLS as f64),
        ("atlas_rows", PROBE_ROWS as f64),
        ("max_tile_id", (PROBE_COLS * PROBE_ROWS) as f64),
    ] {
        let got = field_num(&layout, key)?;
        if got != want {
            return Err(format!(
                "探针地图的 {key} = {got}，应为 {want}（tilemap.get 必须把图集布局读出来说话）"
            )
            .into());
        }
    }
    let quads = quads_for(&mut runtime, PROBE_TILESET)?;
    let painted: Vec<u8> = PROBE_CELLS
        .iter()
        .copied()
        .filter(|value| *value != 0)
        .collect();
    if quads.len() != painted.len() {
        return Err(format!(
            "探针画了 {} 格，应有 {} 格（顶点数 {} 应为 {}）",
            quads.len(),
            painted.len(),
            quads.len() * 6,
            painted.len() * 6
        )
        .into());
    }
    if let Some(message) = painted.iter().enumerate().find_map(|(index, entry)| {
        let (min, max) = expected_uv(*entry);
        (quads[index] != (min, max)).then(|| {
            format!(
                "第 {index} 格（图集条目 {entry}，图集行 {:?}）UV 应为 {min:?}..{max:?}，实测 {:?}",
                i32::from(*entry - 1) / PROBE_COLS as i32,
                quads[index]
            )
        })
    }) {
        return Err(message.into());
    }
    let top_row = quads.iter().filter(|(_, max)| max[1] > 0.9).count();
    println!(
        "  {} 格逐格对上子矩形（覆盖图集 8 行；落在最上一行的 {top_row} 格 V 上限 = 1.0）",
        quads.len()
    );

    banner("② 索引 0 不产出 quad");
    let empty: Vec<usize> = PROBE_CELLS
        .iter()
        .enumerate()
        .filter(|(_, value)| **value == 0)
        .map(|(index, _)| index)
        .collect();
    for index in &empty {
        let x = index % PROBE_WIDTH;
        let y = index / PROBE_WIDTH;
        let entry = lua_num(
            &mut runtime,
            &format!("tilemap.atlas_index({probe}, {x}, {y})"),
        )?;
        if entry != 0.0 {
            return Err(
                format!("空格 ({x},{y}) 解析出图集条目 {entry}，应为 0 = 什么都不画").into(),
            );
        }
    }
    println!(
        "  {} 个空格（格 {}）解析为 0，quad 总数已按 §① 钉死 = 非空格数",
        empty.len(),
        empty.len()
    );

    banner("③ 反面对照：atlas_rows 改回 1，门必须红在第二行以后的格子上");
    // (a) 纯数据侧：V 越界到图集外面，说明 §① 的断言真的在看行数，而不是"UV 在 0..1 里就行"。
    let mut broken = Tilemap::new(Vec2::new(32.0, 32.0), PROBE_WIDTH, PROBE_HEIGHT)
        .map_err(|error| error.to_string())?;
    broken.texture = PROBE_TILESET.to_owned();
    broken.atlas_cols = PROBE_COLS;
    broken.atlas_rows = PROBE_ROWS;
    broken.tiles = PROBE_CELLS.to_vec();
    broken
        .validate_layout()
        .map_err(|error| format!("探针布局自检就失败：{error}"))?;
    broken.atlas_rows = 1;
    let broken_uv = broken.atlas_uv(13);
    if broken_uv.1.y <= 1.0 {
        return Err(format!(
            "atlas_rows 写成 1 之后条目 13 的 UV = {:?}..{:?}，V 没有越界——\
             §① 那批断言对『多行』其实是瞎的（旧实现的 V 恒 0..1 恰好也在 0..1 里）",
            broken_uv.0, broken_uv.1
        )
        .into());
    }
    println!(
        "  (a) atlas_rows=1 时条目 13 的 V = {:.3}..{:.3}（越出 0..1，§① 必红在第 3 格起的格子）",
        broken_uv.0.y, broken_uv.1.y
    );
    // (b) 运行时侧：引擎必须拒绝，而不是让美术的图集静默错位。
    let rejected = lua_expect_error(
        &mut runtime,
        &format!("tilemap.set_atlas({probe}, {PROBE_COLS}, 1)"),
        "out of atlas range",
    )?;
    println!("  (b) 运行时拒绝改回单行：{rejected}");

    banner("④ 旧编码语义不变：6 列切 U、V 整幅拉伸，且只认 1..=6");
    let legacy = Tilemap::from_component_str(
        "tile_size=32x32;width=3;height=1;texture=assets/tileset.png;tiles=123;solid=000",
    )
    .map_err(|error| error.to_string())?;
    if legacy.tiles_format != TILES_FORMAT_LEGACY
        || legacy.atlas_cols != 6
        || legacy.atlas_rows != 1
    {
        return Err(format!(
            "旧串读出来 tiles_format={} atlas={}x{}，应为 0 与 6x1（缺省布局不能跟着新编码改）",
            legacy.tiles_format, legacy.atlas_cols, legacy.atlas_rows
        )
        .into());
    }
    let (lmin, lmax) = legacy.atlas_uv(2);
    if (lmin.x, lmin.y, lmax.x, lmax.y) != (1.0 / 6.0, 0.0, 2.0 / 6.0, 1.0) {
        return Err(format!(
            "旧编码条目 2 的 UV = {lmin:?}..{lmax:?}，应为 6 列切 U 且 V 整幅 0..1"
        )
        .into());
    }
    if legacy.max_tile_id() != 6 {
        return Err(format!("旧编码可寻上界 = {}，应为 6", legacy.max_tile_id()).into());
    }
    // 新编码写回去必须把版本号与布局补齐（迁移工具的产物形态在这里钉住）。
    let resaved = legacy.to_component_str();
    if !resaved.contains("tiles_format=1")
        || !resaved.contains("atlas_cols=6")
        || !resaved.contains("atlas_rows=1")
        || !resaved.contains("tiles=1,2,3")
    {
        return Err(format!("旧串重存成新编码后载荷异常：{resaved}").into());
    }
    println!("  旧串仍解出 6x1 与拉伸 V；重存后载荷带 tiles_format=1 与逗号编码");

    banner("⑤ 发布数据：山庄地面已在新编码上，且画出的格子数 = 逐格数出来的非空格数");
    assert_scene(&mut runtime, "main")?;
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    assert_scene(&mut runtime, "manor")?;
    let ground_id = first_tilemap_entity(&mut runtime)?;
    let layout = lua_table(&mut runtime, &format!("tilemap.get({ground_id})"))?;
    let tiles_format = field_num(&layout, "tiles_format")?;
    let atlas_cols = field_num(&layout, "atlas_cols")?;
    let atlas_rows = field_num(&layout, "atlas_rows")?;
    let width = field_num(&layout, "width")? as usize;
    let height = field_num(&layout, "height")? as usize;
    let texture = field_str(&layout, "texture")?;
    if tiles_format != f64::from(TILES_FORMAT_ATLAS) {
        return Err(format!(
            "山庄地面的 tiles_format = {tiles_format}，应为 {TILES_FORMAT_ATLAS}（迁移没跑到这张）"
        )
        .into());
    }
    let counted = lua_num(
        &mut runtime,
        &format!(
            r#"(function()
                 local n = 0
                 for y = 0, {height} - 1 do
                   for x = 0, {width} - 1 do
                     if tilemap.get_cell({ground_id}, x, y) ~= 0 then n = n + 1 end
                   end
                 end
                 return n
               end)()"#
        ),
    )? as usize;
    // 视口剔除关掉才是"整层全画"档：开着剔除时，画出的格子数取决于相机站在哪，
    // 与"编码读回来是不是同一张地图"这件事无关（252 vs 920 的实测错位就是这么来的）。
    runtime.set_tilemap_culling(false);
    let ground_quads = quads_for(&mut runtime, &texture)?;
    if ground_quads.len() != counted {
        return Err(format!(
            "{texture}：渲染出了 {} 格，逐格数出来 {counted} 格非空——逗号编码与渲染读到的不是同一张地图",
            ground_quads.len()
        )
        .into());
    }
    // V 的口径跟着**实际行数**走，不写死 1.0：`atlas_rows == 1` 时"整幅拉伸 0..1"就是
    // 6×1 数据的语义，`atlas_rows > 1` 时每格只该占 1/rows 的一条带。M72-04 把山庄地面换成
    // 12×9 图集之后，这条门红在"V 上限不是 1.0"上——那是门把自己的特例当成了通则。
    let v_band = 1.0 / atlas_rows as f32;
    for (min, max) in &ground_quads {
        if (max[1] - min[1] - v_band).abs() > 1e-4 {
            return Err(format!(
                "{texture} 有格子的 V 跨度不是 1/{atlas_rows:.0} = {v_band:.6}：{:?}..{:?}（行没切匀）",
                min[1], max[1]
            )
            .into());
        }
        if (min[1] * atlas_rows as f32).fract() > 1e-4
            || (max[1] * atlas_rows as f32).fract() > 1e-4
        {
            return Err(format!(
                "{texture} 有格子的 V 边线没落在 {atlas_rows:.0} 等分线上：{:?}..{:?}",
                min[1], max[1]
            )
            .into());
        }
    }
    if ground_quads
        .iter()
        .any(|(min, _)| (min[0] * atlas_cols as f32).fract() > 1e-4)
    {
        return Err(format!("{texture} 有格子的 U 起点没落在 {atlas_cols} 等分线上").into());
    }
    println!(
        "  {texture}: fmt=1 atlas={atlas_cols}x{atlas_rows} 网格 {width}x{height}，非空格与绘制格同为 {}，V 按 {atlas_rows:.0} 行分带",
        ground_quads.len()
    );

    println!(
        "\njhyt_m72_tileset_atlas_ok project={} probe_quads={} legacy_v_stretched=true manor_cells={counted}",
        project.display(),
        quads.len()
    );
    Ok(())
}

/// 建探针地图：`tilemap.create` → `set_atlas` → 逐格 `set_cell`（走 Lua，顺带钉这套 API）。
fn spawn_probe(runtime: &mut RuntimeScene) -> Result<i64, Box<dyn std::error::Error>> {
    let cells = PROBE_CELLS
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = index % PROBE_WIDTH;
            let y = index / PROBE_WIDTH;
            format!("  tilemap.set_cell(__m72_probe, {x}, {y}, {value})")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let script = format!(
        r#"__m72_probe = tilemap.create({PROBE_WIDTH}, {PROBE_HEIGHT}, 32, '{PROBE_TILESET}', 'M72AtlasProbe')
           tilemap.set_atlas(__m72_probe, {PROBE_COLS}, {PROBE_ROWS})
{cells}
           return __m72_probe"#
    );
    lua_run(runtime, &script)?;
    frame(runtime)?;
    lua_id(runtime, "__m72_probe")
}

/// 某张贴图本帧画出的 quad 序列（UV 左下角 + 右上角）。overlay 与世界深度通道都查，
/// 两边必须只有一边有货——瓦片层走哪条通道由 `world_sprite_depth` 开关决定，门不该
/// 因为宿主换了通道就读不到数。
fn quads_for(
    runtime: &mut RuntimeScene,
    path: &str,
) -> Result<Vec<UvQuad>, Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let overlay: Vec<(usize, usize)> = mesh
        .draws
        .iter()
        .filter(|draw| path_of(&mesh, draw.texture) == Some(path))
        .map(|draw| (draw.start as usize, draw.count as usize))
        .collect();
    let world: Vec<(usize, usize)> = mesh
        .world_sprite_draws
        .iter()
        .filter(|draw| path_of(&mesh, draw.texture_id) == Some(path))
        .map(|draw| (draw.vertex_start, draw.vertex_count))
        .collect();
    if !overlay.is_empty() && !world.is_empty() {
        return Err(format!("{path} 同时出现在 overlay 与世界深度通道——瓦片层只该走一条").into());
    }
    let uvs: Vec<[f32; 2]> = if overlay.is_empty() {
        mesh.world_sprites.iter().map(|vertex| vertex.uv).collect()
    } else {
        mesh.vertices.iter().map(|vertex| vertex.uv).collect()
    };
    Ok(quads_in(
        if overlay.is_empty() { &world } else { &overlay },
        &uvs,
    ))
}

/// 每格 6 顶点（三角形序 0,1,2 / 0,2,3）：第 0 个顶点的 UV 是子矩形左下角，第 2 个是右上角。
fn quads_in(ranges: &[(usize, usize)], uvs: &[[f32; 2]]) -> Vec<UvQuad> {
    let mut out = Vec::new();
    for (start, count) in ranges {
        for quad in uvs[*start..*start + *count].as_chunks::<6>().0.iter() {
            out.push((quad[0], quad[2]));
        }
    }
    out
}

fn path_of(mesh: &SceneMesh, texture: u32) -> Option<&str> {
    if texture == 0 {
        return None;
    }
    mesh.paths.get((texture - 1) as usize).map(String::as_str)
}

/// 世界里第一个带 Tilemap 的**地编**实体 id（各场景命名不一，按候选名依次找）。
/// id 必须按整数读回：这里是带代际的大整数，走 f32 会掉精度。
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

/// 跑一段必须**失败**的 Lua，并断言错误文案里点名了 `needle`（否则"拒绝"本身没被检验过）。
fn lua_expect_error(
    runtime: &mut RuntimeScene,
    code: &str,
    needle: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let message = match runtime.with_script(|script| script.execute_console(code)) {
        Ok(_) => return Err(format!("这句应当报错但没有：{code}").into()),
        Err(error) => error.to_string(),
    };
    if !message.contains(needle) {
        return Err(format!(
            "报错文案没点名 {needle:?}（实测 {message:?}）——拒绝的原因不对，等于没拒绝"
        )
        .into());
    }
    Ok(message)
}

fn lua_str(runtime: &mut RuntimeScene, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let text = runtime
        .with_script(|script| script.execute_console(&format!("return tostring({expr})")))
        .map_err(|error| error.to_string())?;
    Ok(text.trim().to_owned())
}

/// 实体 id 按整数读回（带代际的大整数，走 f32 会掉精度）。
fn lua_id(runtime: &mut RuntimeScene, expr: &str) -> Result<i64, Box<dyn std::error::Error>> {
    let text = lua_str(runtime, expr)?;
    text.parse::<i64>()
        .map_err(|_| format!("{expr} 不是实体 id（读回 {text:?}）").into())
}

fn lua_num(runtime: &mut RuntimeScene, expr: &str) -> Result<f32, Box<dyn std::error::Error>> {
    let text = runtime
        .with_script(|script| {
            script.execute_console(&format!(
                "local v = {expr}\n\
                 if type(v) == 'number' then return tostring(v) end\n\
                 return 'nil'"
            ))
        })
        .map_err(|error| error.to_string())?;
    text.trim()
        .parse::<f32>()
        .map_err(|_| format!("{expr} 不是数字（读回 {:?}）", text.trim()).into())
}

/// 把 Lua 表读成 `key=value` 行（`tilemap.get` 的字段面，不经逐个 round-trip）。
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
