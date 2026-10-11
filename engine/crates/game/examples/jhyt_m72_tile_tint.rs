//! Headless acceptance drive for M72-02：**per-tile tint 一路带到顶点色**。
//!
//! 需求（`tasks/plan/M72-02-per-tile-tint与确定性变体与autotile.md`）：季节/湿润的整体色调与阴影
//! 压暗都要落在**单格**上。顶点格式早就带 `color:[f32;4]`，结构上是免费的，缺的只是" tilemap 别再
//! 把它硬编码成不透明白"。所以这条门不测像素，测**顶点色**：像素能回答"这里变暗了"，回答不了
//! "第 7 格的那 6 个顶点带没带走这一格的颜色"。
//!
//! 章节：
//!   ① 数据层：`register_tint` 幂等（同色复用同一个 id）、`tint_at` 未染色读回**白**（乘法下白 =
//!      不变），稀疏载荷 `tints=<cellIndex>:<tintId>` + `tints_rgb` 序列化往返逐字节等价 ——
//!      编辑器与运行时读的是同一份颜色表；
//!   ② 运行时（overlay 通道）：3 格各刷不同颜色后，每格 6 个顶点的 `color` 全等于那格的颜色，
//!      其余格仍是白；**非白顶点数必须恰好 = 染色格数 ×6**（把渲染侧的 tint 改回硬编码白，这个数
//!      就是 0，门立刻红——这是本门的反面对照）；
//!   ③ 运行时（世界深度通道）：`set_world_sprite_depth(true)` 后同一份颜色必须照样进 `WorldSpriteVertex`
//!      （渲染器里 overlay 与世界两条路各有自己的 tint 赋值点，只接一条等于另一条静默丢色）；
//!   ④ alpha：`a=0.35` 要原样出现在顶点色第 4 分量（web 宿主对半透明只是近似，原生不许先丢一次）；
//!   ⑤ 撤销与当帧生效：`clear_tint` 后该格回白，**其他格的颜色一格不许变**（tint 表不能被顺手清空）；
//!   ⑥ 两种颜色不许并成一种：若 tint id 在解算里丢了，两格会读出同一个顶点色。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_tile_tint [-- <game_jhyt-root>]

use engine::host::{RuntimeScene, SceneMesh};
use rustlua_ecs::tilemap::Tilemap;
use rustlua_ecs::transform::Vec2;
use rustlua_input::input::InputSnapshot;
use std::{env, path::PathBuf};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;
const PROBE_TILESET: &str = "assets/art/tilesets/m72/probe_tint.png";
const WIDTH: usize = 8;
const HEIGHT: usize = 4;
const CELLS: usize = WIDTH * HEIGHT;
const PROBE: &str = "__m72_tint_probe";
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// 三格三种颜色：湿润压蓝、秋天压黄、阴影压暗（各带不同 alpha，顺带钉 §④）。
const TINT_A: [f32; 4] = [0.72, 0.84, 1.0, 1.0];
const TINT_B: [f32; 4] = [0.94, 0.78, 0.42, 0.35];
const TINT_C: [f32; 4] = [0.4, 0.4, 0.4, 1.0];
const TINTED: [([usize; 2], [f32; 4]); 3] = [([1, 0], TINT_A), ([5, 2], TINT_B), ([7, 3], TINT_C)];

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

fn same_color(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| close(*x, *y))
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

    banner("① 数据层：tint 表幂等、未染色读回白、稀疏载荷往返等价");
    let mut map = Tilemap::new(Vec2::new(32.0, 32.0), WIDTH, HEIGHT).map_err(|e| e.to_string())?;
    map.atlas_cols = 6;
    map.atlas_rows = 1;
    map.validate_layout().map_err(|e| e.to_string())?;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            map.set_cell(x, y, 1).map_err(|e| e.to_string())?;
        }
    }
    if map.tint_at(0, 0) != WHITE {
        return Err(format!(
            "没染过的格子 tint_at = {:?}，应为 [1,1,1,1]——乘法下白才等于\"什么都不变\"，\
             否则旧图一接 tint 就整体改色",
            map.tint_at(0, 0)
        )
        .into());
    }
    let id_a = map.register_tint(TINT_A).map_err(|e| e.to_string())?;
    let id_a_again = map.register_tint(TINT_A).map_err(|e| e.to_string())?;
    if id_a != id_a_again || map.tint_table.len() != 1 {
        return Err(format!(
            "同一个颜色注册了两次（id {id_a} vs {id_a_again}，表长 {}）——逐格刷同色会把 255 格的\
             颜色表吃满，美术画到第 256 格就报错",
            map.tint_table.len()
        )
        .into());
    }
    let id_b = map.register_tint(TINT_B).map_err(|e| e.to_string())?;
    map.set_tint(1, 0, id_a).map_err(|e| e.to_string())?;
    map.set_tint(5, 2, id_b).map_err(|e| e.to_string())?;
    let payload = map.to_component_str();
    if !payload.contains(&format!("tints=1:{id_a},21:{id_b}")) || !payload.contains("tints_rgb=") {
        return Err(format!(
            "稀疏载荷少了东西：{payload}\n（应为 `tints=<cellIndex>:<tintId>,…` 与 `tints_rgb=` 两段）"
        )
        .into());
    }
    if payload.contains("tints=0,0,0") {
        return Err("tint 变成逐格定长串了：40 格里只染 2 格也要写 40 个数字".into());
    }
    let restored = Tilemap::from_component_str(&payload).map_err(|e| e.to_string())?;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let before = map.tint_at(x, y);
            let after = restored.tint_at(x, y);
            if !same_color(&before, &after) {
                return Err(format!(
                    "({x},{y}) 往返前 {before:?}、往返后 {after:?}——颜色表没跟着进 .scene，\
                     编辑器另存一次就整体丢色"
                )
                .into());
            }
        }
    }
    println!(
        "  {} 格只写了 2 项（{payload}）；往返后 32 格逐格同色",
        CELLS
    );

    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    runtime.set_tilemap_culling(false);
    let probe = spawn_probe(&mut runtime)?;

    banner("② overlay 通道：染色格的 6 个顶点全部带上那格的颜色");
    let baseline = quad_colors(&mut runtime, PROBE_TILESET)?;
    if baseline.len() != CELLS {
        return Err(format!(
            "{PROBE_TILESET} 画了 {} 格，应为 {CELLS} 格（探针无空格）",
            baseline.len()
        )
        .into());
    }
    let painted_white = baseline
        .iter()
        .filter(|quad| quad.iter().all(|c| same_color(c, &WHITE)));
    if painted_white.count() != CELLS {
        return Err("一条都没染色时就有格子不是白：顶点色基线不干净，后面的断言全是噪声".into());
    }
    for &([x, y], color) in &TINTED {
        lua_run(
            &mut runtime,
            &format!(
                "tilemap.set_tint({PROBE}, {x}, {y}, {}, {}, {}, {})",
                color[0], color[1], color[2], color[3]
            ),
        )?;
    }
    frame(&mut runtime)?;
    let tinted_quads = quad_colors(&mut runtime, PROBE_TILESET)?;
    for &([x, y], color) in &TINTED {
        let quad = tinted_quads
            .get(y * WIDTH + x)
            .ok_or_else(|| format!("格子 ({x},{y}) 没画出来"))?;
        if let Some(offset) = quad.iter().position(|c| !same_color(c, &color)) {
            return Err(format!(
                "格子 ({x},{y}) 的第 {offset} 个顶点色 = {quad:?}，6 个顶点都应为 {color:?}——\
                 tint 只落到了部分顶点，同一格会画出花斑"
            )
            .into());
        }
    }
    let non_white = tinted_quads
        .iter()
        .filter(|quad| quad[0][0] != 1.0 || quad[0][1] != 1.0 || quad[0][2] != 1.0)
        .count();
    if non_white != TINTED.len() {
        return Err(format!(
            "{} 格染色后顶点色里有 {non_white} 格不是白，应为 {} 格——数字对不上就说明渲染侧把 tint\
             丢了（改回硬编码白时这条正好红：非白格数会是 0）",
            TINTED.len(),
            TINTED.len()
        )
        .into());
    }
    let untouched: Vec<usize> = (0..CELLS)
        .filter(|index| !TINTED.iter().any(|([x, y], _)| y * WIDTH + x == *index))
        .filter(|index| !same_color(&tinted_quads[*index][0], &WHITE))
        .collect();
    if !untouched.is_empty() {
        return Err(format!(
            "{} 格没染色却读到了非白顶点色（前几个格号 {untouched:?}）——tint 串到了别的格子",
            untouched.len()
        )
        .into());
    }
    println!(
        "  {} 格 × 6 顶点逐点等于注册色；非白格数 = {non_white}；其余 {} 格仍为白",
        TINTED.len(),
        CELLS - TINTED.len()
    );

    banner("③ 世界深度通道：同一份颜色必须照样进 WorldSpriteVertex");
    runtime.set_world_sprite_depth(true);
    frame(&mut runtime)?;
    let world_quads = quad_colors(&mut runtime, PROBE_TILESET)?;
    for &([x, y], color) in &TINTED {
        let quad = world_quads
            .get(y * WIDTH + x)
            .ok_or_else(|| format!("世界通道里格子 ({x},{y}) 没画出来"))?;
        if !same_color(&quad[0], &color) {
            return Err(format!(
                "世界深度通道里 ({x},{y}) 的顶点色 = {:?}，应为 {color:?}——overlay 接了 tint、\
                 世界通道没接，切了通道的宿主就整层丢色（两个赋值点都得改）",
                quad[0]
            )
            .into());
        }
    }
    runtime.set_world_sprite_depth(false);
    frame(&mut runtime)?;
    println!(
        "  世界通道 {} 格全部带色（切回 overlay 通道后复测仍为 {} 格）",
        world_quads.len(),
        quad_colors(&mut runtime, PROBE_TILESET)?.len()
    );

    banner("④ alpha 分量原样传下去");
    let alpha = tinted_quads
        .get(2 * WIDTH + 5)
        .ok_or("格子 (5,2) 没画出来")?
        .iter()
        .map(|color| color[3])
        .collect::<Vec<_>>();
    if alpha.iter().any(|value| !close(*value, TINT_B[3])) {
        return Err(format!(
            "(5,2) 的 alpha 读回 {alpha:?}，应全为 {}——顶点色只剩 RGB 的话，压暗与半透明会互相覆盖",
            TINT_B[3]
        )
        .into());
    }
    println!("  (5,2) 六个顶点的 alpha 全为 {}", TINT_B[3]);

    banner("⑤ 撤销当帧生效，且不动别的格子");
    let before_clear = tinted_quads.clone();
    lua_run(&mut runtime, &format!("tilemap.clear_tint({PROBE}, 1, 0)"))?;
    frame(&mut runtime)?;
    let after_clear = quad_colors(&mut runtime, PROBE_TILESET)?;
    if !same_color(&after_clear[0][0], &WHITE) {
        return Err(format!(
            "clear_tint 之后 (1,0) 仍是 {:?}，应为白——撤销没当帧生效（tint_ids 没进脏判定）",
            after_clear[0][0]
        )
        .into());
    }
    for &(pos, color) in [&TINTED[1], &TINTED[2]] {
        let [x, y] = pos;
        let quad = &after_clear[y * WIDTH + x];
        if !same_color(&quad[0], &color) {
            return Err(format!(
                "撤销 (1,0) 把 ({x},{y}) 也带成 {:?} 了（原为 {color:?}）——颜色表被顺手清空",
                quad[0]
            )
            .into());
        }
    }
    if after_clear.len() != before_clear.len() {
        return Err(format!(
            "撤销前后格数不同：{} vs {}",
            after_clear.len(),
            before_clear.len()
        )
        .into());
    }
    let still_non_white = after_clear
        .iter()
        .filter(|quad| !same_color(&quad[0], &WHITE))
        .count();
    // 组件侧读回必须与顶点色同一份颜色：只有顶点变了而 `tilemap.tint` 还是旧值 = 渲染缓存
    // 与组件已经分家（美术在编辑器里看到的和玩家看到的就不是同一张图）。
    for (x, y, want) in [(1usize, 0usize, WHITE), (5, 2, TINT_B)] {
        let readback = tint_readback(&mut runtime, probe, x, y)?;
        if !same_color(&readback, &want) {
            return Err(format!(
                "tilemap.tint({x},{y}) 读回 {readback:?}，应为 {want:?}（顶点色已是 {want:?}）",
            )
            .into());
        }
    }
    println!(
        "  (1,0) 回白；另两格颜色不动；当帧非白格数从 {} 降到 {still_non_white}；\
         组件读回与顶点色同一份颜色",
        TINTED.len()
    );

    banner("⑥ 两个颜色不许并成一种");
    if same_color(
        &after_clear[2 * WIDTH + 5][0],
        &after_clear[3 * WIDTH + 7][0],
    ) {
        return Err(format!(
            "(5,2) 与 (7,3) 读出同一个顶点色 {:?}：tint id 在解算里丢了，两种色调会并成一个",
            after_clear[2 * WIDTH + 5][0]
        )
        .into());
    }
    println!(
        "  (5,2) = {:?}、(7,3) = {:?}",
        after_clear[2 * WIDTH + 5][0],
        after_clear[3 * WIDTH + 7][0]
    );

    println!(
        "\njhyt_m72_tile_tint_ok project={} cells={CELLS} tinted={} non_white={non_white} alpha_pinned=true",
        project.display(),
        TINTED.len()
    );
    Ok(())
}

/// 恒等档探针（variant_count=1 / autotile=false）：本门只管顶点色，格子值取条目 1 就够。
fn spawn_probe(runtime: &mut RuntimeScene) -> Result<i64, Box<dyn std::error::Error>> {
    let cells = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| format!("tilemap.set_cell({PROBE}, {x}, {y}, 1)"))
        .collect::<Vec<_>>()
        .join("\n           ");
    let script = format!(
        r#"{PROBE} = tilemap.create({WIDTH}, {HEIGHT}, 32, '{PROBE_TILESET}', 'M72TintProbe')
           {cells}
           return {PROBE}"#
    );
    lua_run(runtime, &script)?;
    frame(runtime)?;
    lua_id(runtime, PROBE)
}

type ColorQuad = [[f32; 4]; 6];

/// 某张贴图本帧画出的每格 6 个顶点色（overlay 与世界深度通道都查，只许一边有货）。
fn quad_colors(
    runtime: &mut RuntimeScene,
    path: &str,
) -> Result<Vec<ColorQuad>, Box<dyn std::error::Error>> {
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
        .filter(|draw| world_path_of(&mesh, draw.texture_id) == Some(path))
        .map(|draw| (draw.vertex_start, draw.vertex_count))
        .collect();
    if !overlay.is_empty() && !world.is_empty() {
        return Err(format!("{path} 同时出现在 overlay 与世界深度通道——瓦片层只该走一条").into());
    }
    let colors: Vec<[f32; 4]> = if overlay.is_empty() {
        mesh.world_sprites
            .iter()
            .map(|vertex| vertex.color)
            .collect()
    } else {
        mesh.vertices.iter().map(|vertex| vertex.color).collect()
    };
    let ranges: Vec<(usize, usize)> = if overlay.is_empty() { world } else { overlay };
    // 门用"格号 = 顶点顺序 / 6"定位格子，因此一层必须只产出一段；分成多段时顺序由批处理决定，
    // 再按格号索引就会读错格子（宁可这里立刻红，也不要给出错位的"证据"）。
    if ranges.len() != 1 {
        return Err(format!(
            "{path} 本帧被切成 {} 个绘制段，门只认\"一层一段\"（否则格号与顶点顺序对不上）",
            ranges.len()
        )
        .into());
    }
    let mut out = Vec::new();
    for (start, count) in ranges {
        for quad in colors[start..start + count].chunks_exact(6) {
            let six: ColorQuad = quad
                .try_into()
                .map_err(|_| format!("{path} 的顶点数不是 6 的整数倍"))?;
            out.push(six);
        }
    }
    Ok(out)
}

/// overlay 侧的 `SceneDraw.texture` 是 **1 基**（0 = 内建白图）。
fn path_of(mesh: &SceneMesh, texture: u32) -> Option<&str> {
    if texture == 0 {
        return None;
    }
    mesh.paths.get((texture - 1) as usize).map(String::as_str)
}

/// 世界通道侧的 `SpriteDrawBatch.texture_id` 直接就是 `paths` 下标（**0 基**，
/// `frame_data.rs:816` 在给 overlay 那一份 `+1`）：两套编号口径混用会把整条通道读成空。
fn world_path_of(mesh: &SceneMesh, texture_id: u32) -> Option<&str> {
    mesh.paths.get(texture_id as usize).map(String::as_str)
}

fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W, VIEW_H)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
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

/// 实体 id 按整数读回（带代际的大整数，走 f32 会掉精度）。
fn lua_id(runtime: &mut RuntimeScene, expr: &str) -> Result<i64, Box<dyn std::error::Error>> {
    let text = lua_str(runtime, expr)?;
    text.parse::<i64>()
        .map_err(|_| format!("{expr} 不是实体 id（读回 {text:?}）").into())
}

/// 从组件侧读回一格 tint：`tilemap.tint` 走的是 `Tilemap::tint_at`，与渲染器取的同一份数据，
/// 但顶点色已经过 chunk 缓存 —— 两边不一致说明缓存与组件分家。
fn tint_readback(
    runtime: &mut RuntimeScene,
    id: i64,
    x: usize,
    y: usize,
) -> Result<[f32; 4], Box<dyn std::error::Error>> {
    let text = lua_str(
        runtime,
        &format!(
            "(function() local c = tilemap.tint({id}, {x}, {y})\
             return string.format('%.6f,%.6f,%.6f,%.6f', c.r, c.g, c.b, c.a) end)()"
        ),
    )?;
    let parts: Vec<&str> = text.split(',').collect();
    let [r, g, b, a]: [&str; 4] = parts
        .as_slice()
        .try_into()
        .map_err(|_| format!("({x},{y}) 的 tint 读回 {text:?}，应为 4 个分量"))?;
    let parse = |value: &str| -> Result<f32, Box<dyn std::error::Error>> {
        value
            .parse::<f32>()
            .map_err(|_| format!("{value:?} 不是颜色分量").into())
    };
    Ok([parse(r)?, parse(g)?, parse(b)?, parse(a)?])
}
