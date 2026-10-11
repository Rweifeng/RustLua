//! Headless acceptance drive for M72-02：**确定性变体（含雪崩收尾）与逐帧全等**。
//!
//! 需求（`tasks/done/M72-02-per-tile-tint与确定性变体与autotile.md`）：地面没有变体 = 同一种瓦
//! 铺满屏就是一张贴纸。变体必须由「格子坐标 + 种子」解出：**确定性**是底线（每帧抖一次 = 画面类
//! 门全红、黄金表作废），**不排成点阵**是同一件事的另一半（确定性有、质感没有 = 假的地表）。
//!
//! 为什么读图集条目与顶点 UV、不读像素：变体落在图集哪一格是纯数据结论，`tilemap.atlas_index`
//! 与 `SceneMesh` 已经把答案交出来。像素只能回答"看着不一样"，回答不了"第 3 格取了哪一个变体"。
//!
//! 章节：
//!   ① 哈希常数金标：8×4、`variant_seed=0x1234_5678`、`variant_count=4` 的 32 格变体序号逐格
//!      等于 `VARIANT_GOLDEN`。常数（FNV 偏移与质数、两个盐、雪崩的两个乘子）加种子是**跨宿主
//!      契约**：Rust、web 的 JS 镜像、web 的 Lua 垫片、`tools/preview_scene.py` 四份实现抄的是
//!      同一串位——改任何一个字符都该在这里红，而不是在某个浏览器里"看着不太对"；
//!   ② 逐帧全等：同一张图跑满 100 帧，每帧取「格 → 图集条目」全序列与渲染出的全序列 UV，
//!      断言与第 0 帧逐元素相等（条目没变而 UV 变了 = 解算与渲染读的不是同一份数据）；
//!   ③ 两层语义的分界：`variant_count==1 && !autotile` 时存储 id **就是**图集条目（旧图零改动），
//!      切到变体档后同一个 id 必须落进它自己那一格宽的块里；
//!   ④ 雪崩不是修饰：本门**自带一份未混洗的 FNV 参考实现**（故意再写一遍），断言它的第一行正好
//!      是 `3,2,1,0,3,2,1,0` 的周期点阵，而引擎给的第一行不是任何周期 ≤4 的重复——少了收尾这一
//!      步，§①②测的是"确定地画出一张马赛克网格"；
//!   ⑤ 种子真的进了哈希：换 seed 必须换表，换回来必须逐格复原（纯函数才可复现）；
//!   ⑥ 跨宿主：把 `plugins/webgl/web-runtime.js` 里那份 **Lua 图集解算镜像**原样喂进引擎的真
//!      Lua 5.4 VM，让它拿同一张地图（布局字段读 `tilemap.get`、格子值读 `tilemap.get_cell`）
//!      重解一遍，32 格必须与引擎逐格同值。**反面对照**：只把镜像那份表里的种子 +1，表必须变
//!      ——证明这条同值不是"两边各自抄了同一个常量表"的错觉，而是镜像真的在跑同一串位运算。
//!      四份实现的其余两份：JS 与 Lua 垫片的常数由 `scripts/check-tile-hash-parity.mjs` 钉住
//!      （同一组金标数字），`tools/preview_scene.py` 用
//!      `python -c "import sys;sys.path.insert(0,'examples/game_jhyt/tools');import preview_scene as p;print([p.variant_for([],8,4,x,0,0x12345678,4) for x in range(8)])"`
//!      复现第一行 `0,2,2,1,3,1,0,3`。
//!
//! 探针贴图路径指向一张**不存在**的文件：宿主只登记路径不做 IO，变体解算与像素无关。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_tile_variant_determinism [-- <game_jhyt-root>]

use engine::host::{RuntimeScene, SceneMesh};
use rustlua_input::input::InputSnapshot;
use std::{env, path::PathBuf};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;
const PROBE_TILESET: &str = "assets/art/tilesets/m72/probe_variant.png";
const PROBE_WIDTH: usize = 8;
const PROBE_HEIGHT: usize = 4;
const PROBE_CELLS: usize = PROBE_WIDTH * PROBE_HEIGHT;
const ATLAS_COLS: usize = 12;
const ATLAS_ROWS: usize = 8;
const KINDS: usize = 4;
const VARIANTS: usize = 4;
const SEED: u32 = 0x1234_5678;
const OTHER_SEED: u32 = 0xA5A5_1234;
const FRAMES: usize = 100;
/// §⑥ 从 web 宿主里切那份 Lua 解算镜像用的起止暗号（改动一侧就必须同时改这里）。
const MIRROR_START: &str = "__rustlua_cell_index = function(m, x, y)";
const MIRROR_END: &str = "\ntilemap = {";

/// 种子 `0x1234_5678`、`variant_count=4` 时 8×4 每格的变体序号（行主序，row 0 在地图最底）。
/// 与 `rustlua-ecs/src/tilemap.rs` 的同名常数**故意各写一遍**：那份钉数据层，这份钉"渲染读到的
/// 也是这一份"。两处一起写错成同一种错位，这条门才真的白装。
const VARIANT_GOLDEN: [u8; PROBE_CELLS] = [
    0, 2, 2, 1, 3, 1, 0, 3, //
    3, 2, 2, 0, 2, 1, 3, 2, //
    1, 1, 1, 0, 3, 1, 1, 3, //
    3, 1, 2, 0, 3, 2, 1, 1, //
];

/// §④ 对照物：与引擎同一条 FNV 链（同一串常数、同一盐顺序），只是**不做** splitmix 雪崩收尾。
const RAW_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const RAW_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;
const RAW_MIX_X: u64 = 0x9e37_79b9_7f4a_7c15;
const RAW_MIX_Y: u64 = 0xbf58_476d_1ce4_e5b9;

fn raw_variant(x: usize, y: usize, seed: u32, count: usize) -> usize {
    let mut hash = RAW_HASH_OFFSET;
    for value in [x as u64, y as u64, u64::from(seed), RAW_MIX_X, RAW_MIX_Y] {
        for shift in 0..8u32 {
            let byte = (value >> (8 * shift)) & 0xff;
            hash = (hash ^ byte).wrapping_mul(RAW_HASH_PRIME);
        }
    }
    (hash % count as u64) as usize
}

/// 存在周期 p ∈ {1,2,4} 让整行由前 p 个元素重复而成，就算"点阵"（p 只取 2 的幂：变体数在实践中
/// 就是 2/4/8 这一档，`count=4` 的周期恰好是 4）。
fn is_periodic_lattice(row: &[u8]) -> bool {
    [1usize, 2, 4].into_iter().any(|period| {
        row.len() >= 2 * period
            && row
                .iter()
                .enumerate()
                .all(|(index, value)| *value == row[index % period])
    })
}

fn banner(title: &str) {
    println!("\n== {title} ==");
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

    let probe = spawn_variant_probe(&mut runtime)?;
    // 剔除关掉才是"整层全画"档：开着剔除时 UV 序列的长度取决于相机站在哪，
    // "逐帧全等"会退化成"这 100 帧相机没动"。
    runtime.set_tilemap_culling(false);

    banner("① 哈希常数金标：32 格变体序号逐格对上");
    let entries = entries_of(&mut runtime, probe)?;
    if entries.contains(&0) {
        let position = entries.iter().position(|entry| *entry == 0).unwrap_or(0);
        return Err(format!(
            "kind 1 的第 {position} 格解出条目 0（什么都不画）——布局 {}x{} 装得下 \
             {KINDS}×{VARIANTS} 个条目，解算却判它不可寻址",
            ATLAS_COLS, ATLAS_ROWS
        )
        .into());
    }
    let variants: Vec<u8> = entries.iter().map(|entry| entry - 1).collect();
    if variants != VARIANT_GOLDEN {
        return Err(format!(
            "第一行变体 = {:?}，金标是 {:?}——FNV 常数、盐顺序或雪崩收尾被改过，\
             四份实现里已有一份不再是同一串位",
            &variants[..PROBE_WIDTH],
            &VARIANT_GOLDEN[..PROBE_WIDTH]
        )
        .into());
    }
    println!(
        "  {} 格逐格对上（第一行 {:?}）；条目 = 1 + (kind-1)*{VARIANTS} + variant",
        variants.len(),
        &variants[..PROBE_WIDTH]
    );

    banner(&format!("② 逐帧全等：同一张图跑满 {FRAMES} 帧"));
    let baseline = quads_of(&mut runtime, PROBE_TILESET)?;
    if baseline.len() != PROBE_CELLS {
        return Err(format!(
            "{PROBE_TILESET} 画了 {} 格，应为 {PROBE_CELLS} 格（探针无空格）",
            baseline.len()
        )
        .into());
    }
    for index in 1..FRAMES {
        frame(&mut runtime)?;
        let scene = lua_str(&mut runtime, "scene.reload()")?;
        if scene != "main" {
            return Err(format!(
                "第 {index} 帧场景切到了 {scene:?}——探针地图已不在场上，后面的全等没测到东西"
            )
            .into());
        }
        let frame_entries = entries_of(&mut runtime, probe)?;
        if frame_entries != entries {
            let position = frame_entries
                .iter()
                .position(|value| *value != entries[0])
                .unwrap_or(0);
            return Err(format!(
                "第 {index} 帧第 {position} 格解出条目 {}，第 0 帧是 {}——变体在帧间漂移\
                 （用了随机数或每帧变化的种子）",
                frame_entries[position], entries[position]
            )
            .into());
        }
        let frame_quads = quads_of(&mut runtime, PROBE_TILESET)?;
        if frame_quads != baseline {
            return Err(format!(
                "第 {index} 帧的 UV 序列与第 0 帧不同（{} 格 vs {} 格）：条目没变而画出来变了，\
                 解算与渲染读的不是同一份数据",
                frame_quads.len(),
                baseline.len()
            )
            .into());
        }
    }
    println!(
        "  {FRAMES} 帧 × {PROBE_CELLS} 格条目序列全等；{FRAMES} 帧 × {} 个 quad 的 UV 序列全等",
        baseline.len()
    );

    banner("③ 两层语义的分界：恒等档 id 就是条目，变体档 id 是逻辑瓦");
    let identity = spawn_identity_probe(&mut runtime)?;
    let id_value =
        usize::from(lua_num(&mut runtime, &format!("tilemap.get_cell({identity}, 0, 0)"))? as u8);
    let identity_entries = entries_of(&mut runtime, identity)?;
    if identity_entries
        .iter()
        .any(|entry| usize::from(*entry) != id_value)
    {
        return Err(format!(
            "恒等档（variant_count=1、autotile=false）里格子值 {id_value} 解出 {:?}——\
             旧图（没开变体）的地址被改了，M72-01 的迁移白做",
            &identity_entries[..4]
        )
        .into());
    }
    lua_run(
        &mut runtime,
        &format!("tilemap.set_kinds({identity}, {KINDS}, {VARIANTS}, false, {SEED})"),
    )?;
    let block = entries_of(&mut runtime, identity)?;
    let want = 1 + (id_value - 1) * VARIANTS;
    if block
        .iter()
        .any(|entry| (*entry as usize) < want || (*entry as usize) >= want + VARIANTS)
    {
        return Err(format!(
            "同一批格子值 {id_value} 切到变体档后解出 {:?}，必须全落在 kind {id_value} 的块 \
             {want}..{} 里",
            &block[..8],
            want + VARIANTS - 1
        )
        .into());
    }
    println!(
        "  格子值 {id_value}：恒等档条目全是 {}（= id 本身）；切 {VARIANTS} 变体后落进块 {want}..{}（实测 {:?}…）",
        id_value,
        want + VARIANTS - 1,
        &block[..4]
    );

    banner("④ 雪崩收尾：少了它，变体是整齐的点阵而不是地表");
    let raw_first_row: Vec<usize> = (0..PROBE_WIDTH)
        .map(|x| raw_variant(x, 0, SEED, VARIANTS))
        .collect();
    if raw_first_row != [3, 2, 1, 0, 3, 2, 1, 0] {
        return Err(format!(
            "本门的对照物自己就不对：未混洗 FNV 的第一行是 {raw_first_row:?}，预期周期点阵 \
             [3,2,1,0,3,2,1,0]——这条断言现在是空的，先修参考实现"
        )
        .into());
    }
    let engine_first_row = &variants[..PROBE_WIDTH];
    if is_periodic_lattice(engine_first_row) {
        return Err(format!(
            "引擎给的第一行 {engine_first_row:?} 仍是周期 ≤4 的重复序列：变体排成了网格，\
             确定性有、质感没有（未混洗那份恰好就是这形状 {raw_first_row:?}）"
        )
        .into());
    }
    let mut tally = [0usize; VARIANTS];
    for value in &variants {
        tally[usize::from(*value)] += 1;
    }
    println!(
        "  未混洗第一行 = {raw_first_row:?}（周期 4 的点阵）；引擎第一行 = {engine_first_row:?}，\
        {VARIANTS} 个变体在 {PROBE_CELLS} 格里分布 {tally:?}"
    );

    banner("⑤ 种子真的进了哈希");
    lua_run(
        &mut runtime,
        &format!("tilemap.set_kinds({probe}, {KINDS}, {VARIANTS}, false, {OTHER_SEED})"),
    )?;
    let swapped = entries_of(&mut runtime, probe)?;
    if swapped == entries {
        return Err(format!(
            "把种子从 {SEED} 换成 {OTHER_SEED}，32 格的变体表一格没变——种子没进哈希，\
             两张图会共用同一套错位"
        )
        .into());
    }
    lua_run(
        &mut runtime,
        &format!("tilemap.set_kinds({probe}, {KINDS}, {VARIANTS}, false, {SEED})"),
    )?;
    let restored = entries_of(&mut runtime, probe)?;
    if restored != entries {
        return Err(
            "换回原种子后变体表与最初不同：解算不是 (x, y, seed) 的纯函数，黄金表不可复现"
                .to_owned()
                .into(),
        );
    }
    let differs = swapped.iter().zip(&entries).filter(|(a, b)| a != b).count();
    println!("  换 seed 改了 {differs}/{PROBE_CELLS} 格；换回来逐格复原（纯函数，可复现）");

    banner("⑥ web 宿主的 Lua 镜像与引擎解算同值（把垫片抄本喂进真 VM）");
    let mirror = web_lua_mirror(project.as_path())?;
    lua_run(&mut runtime, &mirror)?;
    let mirrored = mirror_entries_of(&mut runtime, probe, 0)?;
    let engine_entries: Vec<u32> = entries.iter().map(|entry| u32::from(*entry)).collect();
    if mirrored != engine_entries {
        let position = mirrored
            .iter()
            .zip(&engine_entries)
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        let (x, y) = (position % PROBE_WIDTH, position / PROBE_WIDTH);
        return Err(format!(
            "({x},{y}) 引擎解出条目 {}，web 的 Lua 垫片解出 {}——web 宿主与原生各画各的地表，\
             而这件事只会在 M72-12 的三宿主目验里被当成美术问题查半天",
            engine_entries[position], mirrored[position]
        )
        .into());
    }
    // 反面对照：镜像那份表里种子 +1 必须换表。同值若不是因为垫片真的在跑哈希，这一步会原样复现。
    let bumped = mirror_entries_of(&mut runtime, probe, 1)?;
    if bumped == mirrored {
        return Err(
            "把镜像里的 variant_seed 加 1，条目序列一格不变：垫片没读种子，§⑥ 的同值没有内容"
                .to_owned()
                .into(),
        );
    }
    println!(
        "  {} 格与引擎逐格同值（布局取自 `tilemap.get`、格子值取自 `tilemap.get_cell`、解算走垫片）；\
         镜像种子 +1 后 {}/{} 格与引擎不同",
        mirrored.len(),
        bumped.iter().zip(&mirrored).filter(|(a, b)| a != b).count(),
        PROBE_CELLS
    );

    println!(
        "\njhyt_m72_tile_variant_determinism_ok project={} frames={FRAMES} cells={PROBE_CELLS} \
         seed=0x{SEED:08x} raw_lattice={raw_first_row:?} lua_mirror_cells={}",
        project.display(),
        mirrored.len(),
    );
    Ok(())
}

/// 从 web 宿主（`plugins/webgl/web-runtime.js` 内嵌的模板串）里原样切出那份 Lua 图集解算镜像。
///
/// 切的是**宿主实际发给 Lua 的同一段文本**，不是手抄副本：抄本会各自演化，而这条门要判的正是
/// "宿主那份与引擎那份是不是同一串位"。
fn web_lua_mirror(project: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let root = project
        .parent()
        .and_then(|examples| examples.parent())
        .ok_or("game_jhyt 不在 examples/ 下，找不到仓库根")?;
    let path = root.join("plugins").join("webgl").join("web-runtime.js");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("读不到 {}：{error}", path.display()))?;
    let start = source.find(MIRROR_START).ok_or_else(|| {
        format!(
            "{} 里已找不到 {MIRROR_START:?}——web 宿主的解算镜像被挪走或改名，本门 §⑥ 无从对照",
            path.display()
        )
    })?;
    let rest = &source[start..];
    let end = rest.find(MIRROR_END).ok_or_else(|| {
        format!(
            "{} 里 {MIRROR_START:?} 之后找不到 {MIRROR_END:?}——镜像段落边界变了，切出来的会是半个函数",
            path.display()
        )
    })?;
    Ok(rest[..end].to_owned())
}

/// 把同一张地图交给垫片再解一遍：`seed_bump` 只写进镜像那份表（§⑥ 的反面对照开关）。
fn mirror_entries_of(
    runtime: &mut RuntimeScene,
    id: i64,
    seed_bump: i64,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let text = lua_str(
        runtime,
        &format!(
            r#"(function()
                 local m = tilemap.get({id})
                 m.cells = {{}}
                 for y = 0, m.height - 1 do
                   for x = 0, m.width - 1 do
                     m.cells[#m.cells + 1] = tilemap.get_cell({id}, x, y)
                   end
                 end
                 m.variant_seed = m.variant_seed + {seed_bump}
                 local out = {{}}
                 for y = 0, m.height - 1 do
                   for x = 0, m.width - 1 do
                     out[#out + 1] = __rustlua_atlas_index(m, x, y)
                   end
                 end
                 return table.concat(out, ',')
               end)()"#
        ),
    )?;
    let values: Vec<u32> = text
        .split(',')
        .filter_map(|part| part.trim().parse::<u32>().ok())
        .collect();
    if values.len() != PROBE_CELLS {
        return Err(format!(
            "Lua 垫片的条目序列读回 {text:?}，长度 {}，应为 {PROBE_CELLS}（垫片里 __rustlua_atlas_index \
             没被定义？先确认 §⑥ 切出来的那段仍是完整的一段）",
            values.len()
        )
        .into());
    }
    Ok(values)
}

/// 变体档探针：8×4 全部铺 kind 1，图集 12×8（容量 96 ≥ 4 kind × 4 变体）。
fn spawn_variant_probe(runtime: &mut RuntimeScene) -> Result<i64, Box<dyn std::error::Error>> {
    let script = format!(
        r#"__m72_variant_probe = tilemap.create({PROBE_WIDTH}, {PROBE_HEIGHT}, 32, '{PROBE_TILESET}', 'M72VariantProbe')
           tilemap.set_atlas(__m72_variant_probe, {ATLAS_COLS}, {ATLAS_ROWS})
           tilemap.set_kinds(__m72_variant_probe, {KINDS}, {VARIANTS}, false, {SEED})
           {cells}
           return __m72_variant_probe"#,
        cells = fill_cells("__m72_variant_probe", 1)
    );
    lua_run(runtime, &script)?;
    frame(runtime)?;
    lua_id(runtime, "__m72_variant_probe")
}

/// 恒等档探针：同一套图集布局，但不开 kinds（`variant_count=1 / autotile=false`），
/// 格子值直接当图集条目用。
fn spawn_identity_probe(runtime: &mut RuntimeScene) -> Result<i64, Box<dyn std::error::Error>> {
    let script = format!(
        r#"__m72_identity_probe = tilemap.create({PROBE_WIDTH}, {PROBE_HEIGHT}, 32, '{PROBE_TILESET}', 'M72IdentityProbe')
           tilemap.set_atlas(__m72_identity_probe, {ATLAS_COLS}, {ATLAS_ROWS})
           {cells}
           return __m72_identity_probe"#,
        cells = fill_cells("__m72_identity_probe", KINDS)
    );
    lua_run(runtime, &script)?;
    frame(runtime)?;
    lua_id(runtime, "__m72_identity_probe")
}

fn fill_cells(handle: &str, kind: usize) -> String {
    (0..PROBE_HEIGHT)
        .flat_map(|y| (0..PROBE_WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| format!("tilemap.set_cell({handle}, {x}, {y}, {kind})"))
        .collect::<Vec<_>>()
        .join("\n           ")
}

/// 全图「格 → 图集条目」序列，行主序（y 再 x）。一次 Lua 往返取全部，免得逐格 round-trip
/// 把 {FRAMES} 帧的门拖成上千次调用。
fn entries_of(runtime: &mut RuntimeScene, id: i64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let text = lua_str(
        runtime,
        &format!(
            r#"(function()
                 local out = {{}}
                 for y = 0, {PROBE_HEIGHT} - 1 do
                   for x = 0, {PROBE_WIDTH} - 1 do
                     out[#out + 1] = tilemap.atlas_index({id}, x, y)
                   end
                 end
                 return table.concat(out, ',')
               end)()"#
        ),
    )?;
    let values: Vec<u8> = text
        .split(',')
        .filter_map(|part| part.trim().parse::<u8>().ok())
        .collect();
    if values.len() != PROBE_CELLS {
        return Err(format!(
            "条目序列读回 {text:?}，长度 {}，应为 {PROBE_CELLS}",
            values.len()
        )
        .into());
    }
    Ok(values)
}

type UvQuad = ([f32; 2], [f32; 2]);

/// 某张贴图本帧画出的 quad 序列。overlay 与世界深度通道都查，两边必须只有一边有货
/// （瓦片层走哪条通道由 `world_sprite_depth` 开关决定，门不该因为宿主换了通道就读不到数）。
fn quads_of(
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
        // 世界通道的 `texture_id` 是 `paths` 的 0 基下标；overlay 的 `SceneDraw.texture` 才是
        // 1 基（0 = 内建白图，见 `frame_data.rs:816` 的 `+1`）。两套编号不能混用。
        .filter(|draw| mesh.paths.get(draw.texture_id as usize).map(String::as_str) == Some(path))
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
