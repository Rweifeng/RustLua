//! Headless acceptance drive for M35：**角色之间的遮挡跟着脚位 Y 走**（画家顺序）。
//!
//! 需求：`game_jhyt` 里主角会被 NPC 挡住——同层（actor）角色之间的先后完全不看位置。
//!
//! 改造前的两处事实（两处都会单独让这条门红，见"反面对照"）：
//!   ① **引擎**：运行期 `sprite.set` 只写 `Sprite` + `SpriteAsset`，**不写 `SpriteSorting`**
//!      （场景加载路径 `script/scene_spawn.rs` 是写的）⇒ Lua 挂上去的 `sorting_layer=` /
//!      `order=` 在原生一律无效，实体落进"遍历序号"兜底桶；而 web 侧直接解析组件串
//!      （`plugins/webgl/web-runtime.js` 的 `entitySortKey`），两边因此各排各的。
//!   ② **游戏**：没有任何一处把 Y 写进排序。两个宿主的**主排序键都是 `层秩 × 1000 + order`**
//!      （原生 `SpriteSorting::effective_layer` / web `entitySortKey`），而原生 overlay 通道
//!      的排序是 `layer → 纹理 → 插入序`（`sprite_batch.rs`），**显式忽略 z**；带 z 的
//!      世界深度通道只有原生渲染器有、web 不认。所以 order 是唯一一份"两边都读"的深度杠杆
//!      ⇒ `scripts/world/depth.lua` 把脚位 Y 折成 order。
//!
//! 为什么不在引擎里内建 Y 排序：把 Y 插进 overlay 的排序键会压碎"同层同纹理合批"
//! （每个角色一个 draw call）；order 是作者侧桶位，合批只在**同桶**内照常发生。
//! 代价是 Y 被量化成 2 世界单位的桶（角色贴图 64 单位高，肉眼看不出）。
//!
//! **方向（这里翻过一次车，别再翻第二次）**：这台引擎的世界 +y 是**朝上**的——
//! `world_to_screen` 是 `screen.y = h/2 - view.y*zoom`（`rustlua-camera/src/camera/component.rs:138`，
//! web 的 `toCanvas` 同一条）。所以"越靠画面下方 = 越近"= **y 越小越近**，
//! 而画家顺序是后画的盖住先画的 ⇒ **order 随 y 单调递减**（`order = -floor(y/STEP)`）。
//! 写成 `+floor(y/STEP)` 不会报错，症状是"主角站在 NPC 下方反而被 NPC 盖住"，
//! 而只看 `mesh.draws` 的先后是否稳定也看不出来——所以 §② 断言的是**名次方向**，
//! 不是"排过序"。
//!
//! 章节：
//!   ① 引擎契约：运行期 `sprite.set` 的 `sorting_layer` / `order` 真的写进了渲染用的排序层
//!      （`sprite.get` 回读得到）；
//!   ② Y 决定绘制顺序：同一批实体、同样的三张图，只把 Y 倒过来 ⇒ **绘制序整体反转**
//!      （证明排的是 Y，不是实体 id 或纹理下标的巧合），且**靠画面下方那个（y 小的）最后画**；
//!   ③ 玩法真内容：山庄里主角与场上每个 NPC 的 order 恒等于 `-clamp(floor(y/2), -999, 999)`
//!      且层名仍是 actor（机制钉住数据：玩法表改了、这条换算不跟着改就会红）；
//!   ④ 夹取与特效层：y 远超图高（画面最上方=最远）时 order 钉在 -999（不溢进相邻层桶），
//!      特效在 foreground ⇒ 永远画在所有 actor 之后（Y 排序不会把刀光埋进角色里）。
//!
//! 绘制顺序是怎么读出来的：`SceneDraw` 不带排序层字段，所以用**分段在 `mesh.draws` 里的下标**
//! 当绘制先后（引擎按这个顺序提交，overlay 不写深度 ⇒ 后提交的盖住先提交的），再用
//! `mesh.paths[draw.texture - 1]` 认实体（`frame.rs`：`texture == 0` 是内置白图，
//! `texture == i > 0` 才取 `paths[i - 1]`；漏掉这个 `+1` 约定会把每个实体认成邻居的图）。
//! 探针的 Y 一律围着相机中心取：横屏 zoom=2 的可见世界域只有 640×360，绝对 Y 挑得偏就
//! 被视口剔除，看上去像"排序坏了"。探针用**单帧** `static_*.png`，场景里的角色都挂了
//! 序列帧（画的是 `idle_*.png` 条带），两者路径不同名 ⇒ 不会混；命中数 ≠ 1 直接报错，
//! 不静默取第一个。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m35_depth_sort [-- <game_jhyt-root>]

use engine::core::input::InputSnapshot;
use engine::host::{RuntimeScene, SceneDraw, SceneMesh};
use std::{env, path::PathBuf};

/// 设计分辨率 = 发布窗口尺寸（与 `jhyt_m32_camera` 同一口径）。
const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;
/// `world/depth.lua` 的量化步长与夹取上限（两处必须一起改）。
/// order 是**随 y 递减**的（见头注释的"方向"），所以上限以负值那侧为主：
/// `order = -floor(y/STEP)`，夹在 ±999。
const STEP: f32 = 2.0;
const ORDER_MAX: f32 = 999.0;
/// 探针共用 x：相机 x 恒 640（视口宽 == 地图宽），探针都落在可见矩形内。
const PROBE_X: f32 = 640.0;
/// 探针 Y 相对相机中心的展开量（两侧各 60）：可见半高 180，三倍余量；
/// 又远大于 `world/depth.lua` 的量化步长 2 ⇒ 三者必落在三个不同桶。
const PROBE_SPAN: f32 = 60.0;
/// 探针的单帧 Sprite 组件串：贴图互不相同（`mesh.paths` 才认得出谁是谁），
/// 层名一律 actor 且都不写 order——order 由 `world/depth.lua` 现算。
const PROBE_SPECS: [&str; 3] = [
    "texture=assets/art/characters/player_male/static_s.png;size=64x64;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    "texture=assets/art/characters/shen_qingluo/static_s.png;size=64x64;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    "texture=assets/art/characters/zhao_tiesuanpan/static_s.png;size=64x64;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
];
/// 探针编号 → 它那张单帧图（顺序与 `PROBE_SPECS` 一致，认实体用）。
const PROBE_PATHS: [&str; 3] = [
    "assets/art/characters/player_male/static_s.png",
    "assets/art/characters/shen_qingluo/static_s.png",
    "assets/art/characters/zhao_tiesuanpan/static_s.png",
];

/// 一个可排序实体的当前读数。
struct Reading {
    /// 认得出来的名字（探针编号或 NPC persist_id）。
    label: String,
    /// 认得出来的贴图路径（探针才有；场上实体只用于报错文案）。
    path: Option<String>,
    y: f32,
    order: f32,
    layer: String,
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
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "main")?;
    // 进山庄：探针要落在相机看得见的世界里（门面不画世界实体）。
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    assert_scene(&mut runtime, "manor")?;

    banner("① 引擎契约：运行期 sprite.set 的 sorting_layer/order 在原生真的生效");
    // 只走引擎入口（不经 depth）：挂一个 actor+order=123 的实体，回读必须看得见。
    // 修复前这里必然红——`sprite.set` 不写 SpriteSorting，`sprite.get` 连字段都没有。
    lua_run(
        &mut runtime,
        r#"__m35_engine_probe = entity.spawn('M35EngineProbe', 'm35.engine.probe')
           transform.set(__m35_engine_probe, 640, 300)
           sprite.set(__m35_engine_probe, 'texture=assets/art/characters/lin_xiaoman/static_s.png;size=64x64;sorting_layer=actor;order=123')
           return true"#,
    )?;
    let layer = lua_str(&mut runtime, "sprite.get(__m35_engine_probe).sorting_layer")?;
    let order = lua_num(&mut runtime, "sprite.get(__m35_engine_probe).order")?;
    if layer != "actor" || order != 123.0 {
        return Err(format!(
            "运行期排序层没生效：sorting_layer={layer:?} order={order}（sprite.set 必须与场景加载同一条路径，写 SpriteSorting）"
        )
        .into());
    }
    println!("  sprite.get 回读 = sorting_layer={layer:?} order={order}");

    banner("② Y 决定绘制顺序：只把三个 actor 探针的 Y 倒过来，绘制序整体反转");
    // 探针的 Y 一律**围着相机中心取**：横屏 zoom=2 的可见世界域只有 640×360
    // （`world/camera_view.lua` 的"边界事实"），用固定绝对 Y 会直接被视口剔除——
    // 上一版探针 y=500 就是这么从 `mesh.draws` 里"消失"的，看着像排序坏了。
    let cy = camera_center_y(&mut runtime)?;
    // y 越小 = 画面越靠下 = 越近 = 越要后画（盖在上面）。
    let near_to_far = [cy - PROBE_SPAN, cy, cy + PROBE_SPAN];
    let probes = spawn_probes(&mut runtime, &near_to_far)?;
    let near = read_probes(&mut runtime, &probes)?;
    for (index, reading) in near.iter().enumerate() {
        if (reading.y - near_to_far[index]).abs() > 0.5 {
            return Err(format!(
                "{} 没落到指定 Y：读回 {:.1}，应为 {}",
                reading.label, reading.y, near_to_far[index]
            )
            .into());
        }
        let want = order_of(near_to_far[index]);
        if reading.order != want {
            return Err(format!(
                "{} (y={:.1}) 的 order={}，应为 {want}（world/depth.lua 的换算）",
                reading.label, reading.y, reading.order
            )
            .into());
        }
    }
    let near_first = draw_ranking(&mut runtime, &near)?;
    if near_first != vec![2, 1, 0] {
        return Err(format!(
            "y 从低到高（近→远）时最后画的必须是最靠画面下方那个探针（1 号），名次应为 [2, 1, 0]，\
             实测 {near_first:?}——方向又反了：y 越小越靠前，见头注释的\"方向\"一节"
        )
        .into());
    }
    println!(
        "  y=[{:.0}, {:.0}, {:.0}] order=[{}, {}, {}] 绘制序=3→2→1（y 最小的最后画）",
        near_to_far[0], near_to_far[1], near_to_far[2], near[0].order, near[1].order, near[2].order
    );

    let far_to_near = [cy + PROBE_SPAN, cy, cy - PROBE_SPAN];
    move_probes(&mut runtime, &probes, &far_to_near)?;
    let flipped = read_probes(&mut runtime, &probes)?;
    let far_first = draw_ranking(&mut runtime, &flipped)?;
    if far_first != vec![0, 1, 2] {
        return Err(format!(
            "把 Y 倒过来之后绘制序必须整体反转（应为 1→2→3），实测 {far_first:?}——\
             说明顺序不是 Y 决定的，还是实体 id / 纹理下标的巧合"
        )
        .into());
    }
    println!(
        "  y=[{:.0}, {:.0}, {:.0}] order=[{}, {}, {}] 绘制序=1→2→3（反面对照内建）",
        far_to_near[0],
        far_to_near[1],
        far_to_near[2],
        flipped[0].order,
        flipped[1].order,
        flipped[2].order
    );

    banner("③ 玩法真内容：山庄里主角与场上 NPC 的 order 恒等于脚位 Y 的桶");
    let live = live_actors(&mut runtime)?;
    if live.is_empty() {
        return Err("场上没读到任何 actor 实体（主角/NPC 都没落位？）".into());
    }
    for actor in &live {
        if actor.layer != "actor" {
            return Err(format!(
                "{} 的排序层是 {:?}，不是 actor——角色不该散落在其他层",
                actor.label, actor.layer
            )
            .into());
        }
        let want = order_of(actor.y);
        if actor.order != want {
            return Err(format!(
                "{} (y={:.1}) 的 order={}，应为 {want}",
                actor.label, actor.y, actor.order
            )
            .into());
        }
    }
    println!(
        "  场上 actor 实体 {} 个：{}",
        live.len(),
        live.iter()
            .map(|actor| format!("{}(y={:.0})={}", actor.label, actor.y, actor.order))
            .collect::<Vec<_>>()
            .join(" ")
    );

    banner("④ 夹取与特效层：order 不溢进相邻层桶，特效（foreground）永远画在 actor 之后");
    // y 远大于图高（= 画面最上方 = 最远）：order 必须钉在 -999 ⇒ effective_layer
    // = 1×1000-999 = 1 > default 的 0，不会掉进相邻排序层。
    // 这里只回读不查 draw（4000 的世界坐标早就在视口外被剔除了）。
    move_probes(&mut runtime, &probes, &[4000.0, 4002.0, 4004.0])?;
    let clamped = read_probes(&mut runtime, &probes)?;
    for reading in &clamped {
        if reading.order != -ORDER_MAX {
            return Err(format!(
                "{} 在 y={:.0} 时 order={}，应夹到 {}（超出会溢进相邻排序层）",
                reading.label, reading.y, reading.order, -ORDER_MAX
            )
            .into());
        }
    }
    // 搬回可见带内，再放一个特效在世界最上方附近：特效仍必须画在所有角色之后。
    move_probes(&mut runtime, &probes, &near_to_far)?;
    lua_run(
        &mut runtime,
        "require('systems.effects').spawn('blade_arc', 640, 8) return true",
    )?;
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let effect_at = sole_draw(&mesh, "effects/blade_arc.png")?;
    let mut positions = Vec::new();
    for reading in &clamped {
        positions.push(draw_at(
            &mesh,
            reading.path.as_deref().expect("探针有路径"),
        )?);
    }
    let last_actor = positions.iter().max().expect("至少一个探针");
    if effect_at <= *last_actor {
        return Err(format!(
            "特效 draw #{effect_at} 没有排在最靠下的 actor (draw #{last_actor}) 之后——\
             特效借 actor 层堆一个 order 会被 Y 排序埋掉，它必须在 foreground 层"
        )
        .into());
    }
    println!(
        "  夹取：order=[{}, {}, {}]（y→+∞ 钉在 -{ORDER_MAX}）；特效 draw #{effect_at} 在全部 actor（最靠后 #{last_actor}）之后",
        clamped[0].order, clamped[1].order, clamped[2].order
    );

    println!(
        "\njhyt_m35_depth_sort_ok project={} probes={} live_actors={} clamp=-{ORDER_MAX} effect_after_actor=true",
        project.display(),
        probes.len(),
        live.len()
    );
    Ok(())
}

/// 建三个 actor 探针并落到给定 Y（经 `world/depth` 挂——就是玩法用的那一条路）。
fn spawn_probes(
    runtime: &mut RuntimeScene,
    ys: &[f32; 3],
) -> Result<Vec<i64>, Box<dyn std::error::Error>> {
    let specs = PROBE_SPECS
        .iter()
        .enumerate()
        .map(|(index, spec)| format!("  [{}] = '{}'", index + 1, spec))
        .collect::<Vec<_>>()
        .join(",\n");
    let script = format!(
        r#"local depth = require('world.depth')
           local specs = {{
{specs}
           }}
           local ys = {{ {y0}, {y1}, {y2} }}
           __m35_probes = {{}}
           for i = 1, 3 do
             local id = entity.spawn('M35Probe' .. i, 'm35.probe.' .. i)
             transform.set(id, {PROBE_X}, ys[i])
             depth.attach(id, specs[i])
             __m35_probes[i] = id
           end
           depth.tick()
           return true"#,
        y0 = ys[0],
        y1 = ys[1],
        y2 = ys[2],
    );
    lua_run(runtime, &script)?;
    frame(runtime)?;
    let mut ids = Vec::new();
    for index in 1..=3 {
        ids.push(lua_id(runtime, &format!("__m35_probes[{index}]"))?);
    }
    Ok(ids)
}

/// 相机中心 y（原生宿主的 `camera2d`）。探针的 Y 全部围着它取，见 §② 的可见带注释。
fn camera_center_y(runtime: &mut RuntimeScene) -> Result<f32, Box<dyn std::error::Error>> {
    lua_num(runtime, "camera2d.get().y")
}

/// 把已有探针搬去新的 Y，并重跑一次 `depth.tick()`（桶位变了才会重写组件串）。
fn move_probes(
    runtime: &mut RuntimeScene,
    probes: &[i64],
    ys: &[f32; 3],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut script = String::from("local depth = require('world.depth')\n");
    for (index, id) in probes.iter().enumerate() {
        script.push_str(&format!("transform.set({id}, {PROBE_X}, {})\n", ys[index]));
    }
    script.push_str("depth.tick() return true");
    lua_run(runtime, &script)?;
    frame(runtime)
}

/// 回读探针当前的 Y / order / 层名（`sorting_layer` 回读本身就是被钉的契约之一）。
fn read_probes(
    runtime: &mut RuntimeScene,
    probes: &[i64],
) -> Result<Vec<Reading>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for (index, id) in probes.iter().enumerate() {
        let y = lua_num(runtime, &format!("transform.get({id}).y"))?;
        let order = lua_num(runtime, &format!("sprite.get({id}).order"))?;
        let layer = lua_str(runtime, &format!("sprite.get({id}).sorting_layer"))?;
        if layer != "actor" {
            return Err(format!("探针 {} 的排序层是 {layer:?}，应为 \"actor\"", index + 1).into());
        }
        out.push(Reading {
            label: format!("probe{}", index + 1),
            path: Some(PROBE_PATHS[index].to_owned()),
            y,
            order,
            layer,
        });
    }
    Ok(out)
}

/// 探针之间按绘制先后给出的**名次**（`[0,1,2]` = 第 1 个探针最先画）。
fn draw_ranking(
    runtime: &mut RuntimeScene,
    probes: &[Reading],
) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let mut positions = Vec::new();
    for probe in probes {
        positions.push(draw_at(&mesh, probe.path.as_deref().expect("探针有路径"))?);
    }
    let mut ranked: Vec<(usize, usize)> = positions
        .iter()
        .enumerate()
        .map(|(probe_index, position)| (probe_index, *position))
        .collect();
    ranked.sort_by_key(|entry| entry.1);
    let mut ranking = vec![0usize; positions.len()];
    for (rank, (probe_index, _)) in ranked.into_iter().enumerate() {
        ranking[probe_index] = rank;
    }
    Ok(ranking)
}

/// 分段用的是 `SceneDraw.texture` 的**约定下标**：`frame.rs` §纹理字段写明 `texture == 0`
/// 是内置白图、`texture == i > 0` 取 `paths[i - 1]`（`frame_data.rs` 写入时就做了 `+1`）。
/// 直接拿 `texture` 去索引 `paths` 会整体错开一格，把每个实体的贴图认成邻居的那张——
/// 于是"绘制顺序"读出来是乱的，而排序本身其实没问题。
fn draw_path<'a>(mesh: &'a SceneMesh, draw: &SceneDraw) -> Option<&'a str> {
    if draw.texture == 0 {
        return None;
    }
    mesh.paths
        .get((draw.texture - 1) as usize)
        .map(String::as_str)
}

/// 某张贴图在 `mesh.draws` 里的下标 = 它被提交的先后。命中数 ≠ 1 直接报错：
/// 静默取第一个会让"两个实体同图"这种巧合把门读成绿。
fn draw_at(mesh: &SceneMesh, path: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let hits: Vec<usize> = mesh
        .draws
        .iter()
        .enumerate()
        .filter(|(_, draw)| draw_path(mesh, draw).is_some_and(|registered| registered == path))
        .map(|(index, _)| index)
        .collect();
    match hits.as_slice() {
        [only] => Ok(*only),
        [] => Err(format!(
            "{path} 在 mesh.draws 里一个分段都没有——实体没被画出来（多半是被视口剔除了：\
             探针的 Y 必须围着相机中心取，见 §②）"
        )
        .into()),
        other => Err(format!(
            "{path} 在 mesh.draws 里命中 {} 个分段（应为 1）——绘制顺序无法归给唯一实体",
            other.len()
        )
        .into()),
    }
}

/// 路径里含 `needle` 的唯一分段下标（特效条带：动画 clip 的 sheet 也带同一个文件名）。
fn sole_draw(mesh: &SceneMesh, needle: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let hits: Vec<usize> = mesh
        .draws
        .iter()
        .enumerate()
        .filter(|(_, draw)| draw_path(mesh, draw).is_some_and(|path| path.contains(needle)))
        .map(|(index, _)| index)
        .collect();
    match hits.as_slice() {
        [only] => Ok(*only),
        other => Err(format!("{needle} 命中 {} 个分段（应为 1）", other.len()).into()),
    }
}

/// `world/depth.lua` 的换算：`-clamp(floor(y / STEP), -999, 999)`。
/// 负号是方向本身（y 越小越靠画面下方 = 越近 = 越要后画），不是可选的风格。
fn order_of(y: f32) -> f32 {
    -(y / STEP).floor().clamp(-ORDER_MAX, ORDER_MAX)
}

/// 场上真实 actor 实体（主角 + 已落位 NPC）的读数。
fn live_actors(runtime: &mut RuntimeScene) -> Result<Vec<Reading>, Box<dyn std::error::Error>> {
    let script = r#"
        local out = {}
        local function push(label, id)
          if not id then return end
          local pos = transform.get(id)
          local got = sprite.get(id)
          if not pos or not got or got.order == nil then return end
          out[#out + 1] = label .. ',' .. tostring(pos.y) .. ',' .. tostring(got.order)
            .. ',' .. tostring(got.sorting_layer)
        end
        push('player', entity.find('JianghuPlayer'))
        for _, n in ipairs(require('data.npcs').list()) do
          local id = entity.find_id(n.persist_id)
          if id and entity.is_alive(id) then push(n.persist_id, id) end
        end
        return table.concat(out, ';')
    "#;
    let text = lua_str(runtime, &format!("(function() {script} end)()"))?;
    let mut out = Vec::new();
    for row in text.split(';').map(str::trim).filter(|row| !row.is_empty()) {
        let mut fields = row.split(',');
        let (Some(label), Some(y), Some(order), Some(layer)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("场上 actor 读数格式异常：{row:?}").into());
        };
        let (Ok(y), Ok(order)) = (y.parse::<f32>(), order.parse::<f32>()) else {
            return Err(format!("场上 actor 数值解析失败：{row:?}").into());
        };
        out.push(Reading {
            label: label.to_owned(),
            path: None,
            y,
            order,
            layer: layer.to_owned(),
        });
    }
    Ok(out)
}

/// 推进一帧：先走真实渲染路径（`vertices`），再跑一步脚本。
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

/// 实体 id 必须按**整数**读回：这里是带代际的大整数（1500 亿量级），走 f32 会掉精度，
/// 掉精度的 id 传回 Lua 就是 `invalid entity id`。
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

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
