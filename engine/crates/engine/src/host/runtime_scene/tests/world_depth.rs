//! 原 `mod sdf_tests` 里 `world_depth` 这一域（WP12 阶段 1 纯搬运；源行段 7478–7783）。
//!
//! - `world_sprite_depth_moves_world_text_into_the_world_channel`
//! - `world_sprite_depth_moves_tilemaps_into_the_world_channel`
//! - `world_sprite_depth_flag_switches_between_overlay_and_world_vertices`
//! - `world_channel_does_not_cull_by_the_2d_camera_when_the_scene_has_3d_content`
//! - `scene_declared_emitter_emits_continuously_through_the_step_pipeline`
//! - `gpu_mode_emitters_run_on_cpu_and_report_their_fallback`

use super::*;
use rustlua_ecs::particle::ParticleCpuReason;
use rustlua_scene::scene::Scene;

use crate::host::runtime_scene::ParticlePlacementReport;

/// WP6：世界文字也进世界坐标深度通道——它此前是唯一仍留在屏幕空间的世界内容。    /// 转换走 `view_projection` 的**逆映射**，因此屏幕观感与 overlay 路径逐个像素一致，
/// 但顶点有了世界坐标与 `z`，可以和精灵/瓦片一起参与深度测试。
#[test]
fn world_sprite_depth_moves_world_text_into_the_world_channel() {
    // 与 `world_text_anchors_to_world_position_and_follows_camera` 同一套场景/相机参数，
    // 保证走的是同一条字形管线（否则会因项目根不同而落到矢量字体分支，测不到本路径）。
    let mut scene = Scene::new("world_text_depth");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "WorldText".into(),
            "text=HP;font_size=24;color=1,1,1;offset=0,-20;centered=true".into(),
        );
    }
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
    runtime.set_camera_2d(Camera2D::new(Vec2::new(320.0, 180.0), 1.0).expect("camera"));

    let overlay = runtime.vertices(640, 360).expect("overlay vertices");
    assert!(!overlay.vertices.is_empty(), "世界文字应产出 overlay 顶点");
    assert!(overlay.world_sprites.is_empty());

    // 世界通道：顶点落到**实体世界坐标附近**（`WorldText` 的锚点就是实体位置 (0, 0)；
    // 相机中心只是取景，不改变锚点），并在**世界**绘制列表里登记分段。
    runtime.set_world_sprite_depth(true);
    let world = runtime.vertices(640, 360).expect("world vertices");
    assert!(
        !world.world_sprites.is_empty(),
        "世界文字应产出世界坐标顶点"
    );
    let (min_x, max_x) = world
        .world_sprites
        .iter()
        .map(|vertex| vertex.position[0])
        .fold((f32::MAX, f32::MIN), |(min, max), x| {
            (min.min(x), max.max(x))
        });
    assert!(
        min_x > -60.0 && max_x < 120.0,
        "文字顶点应落在实体世界坐标 (0, 0) 附近（而不是 NDC）：{min_x}..{max_x}"
    );
    assert!(
        world
            .world_sprites
            .iter()
            .all(|vertex| vertex.position[2] == 0.0),
        "世界文字在世界平面 z=0"
    );
    assert!(
        !world.world_sprite_draws.is_empty(),
        "世界文字的绘制分段必须登记在世界列表里（否则渲染器不会画它）"
    );
}

/// WP6：开启世界深度通道后，**瓦片层**也要产出世界坐标顶点（此前只有精灵有），
/// 否则瓦片会被挡在深度通道之外——世界内容一半有深度、一半没有，遮挡必然错。
#[test]
fn world_sprite_depth_moves_tilemaps_into_the_world_channel() {
    let root = game_root();
    let mut scene = rustlua_scene::scene::Scene::new("tilemap-world-pass");
    let id = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(id, "Map", None).unwrap();
    let entity = scene.entities.get_mut(&id).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    // 2×2 瓦片地图：用真实 tileset 纹理，保证纹理注册与 UV 都走正常路径。
    entity.components.insert(
        "Tilemap".into(),
        "tile_size=32x32;width=2;height=2;texture=assets/textures/tileset.png;\
             tiles=1111;solid=0000"
            .into(),
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

    // 默认（overlay 路径）：瓦片是 NDC 顶点，没有世界顶点。
    let overlay = runtime.vertices(640, 360).expect("overlay vertices");
    assert!(
        overlay.vertices.len() >= 24,
        "4 个瓦片应产出 24 个 overlay 顶点：{}",
        overlay.vertices.len()
    );
    assert!(overlay.world_sprites.is_empty(), "关闭时不应有世界顶点");

    // 打开世界通道：瓦片改为世界坐标（中心 (640, 360) 附近、z = 0），overlay 里不再有瓦片。
    runtime.set_world_sprite_depth(true);
    let world = runtime.vertices(640, 360).expect("world vertices");
    assert!(
        world.world_sprites.len() >= 24,
        "瓦片应产出世界顶点：{}",
        world.world_sprites.len()
    );
    assert!(
        world.vertices.len() < overlay.vertices.len(),
        "瓦片已从 overlay 路径移出：{} vs {}",
        world.vertices.len(),
        overlay.vertices.len()
    );
    assert!(
        !world.world_sprite_draws.is_empty(),
        "瓦片的世界顶点必须**登记在世界绘制分段里**（否则渲染器根本不会画它）"
    );
    assert!(
        world
            .world_sprite_draws
            .iter()
            .all(|draw| draw.render_layer == rustlua_render::layer::RenderLayer::World2D),
        "世界通道的分段必须是 World2D"
    );
    assert!(
        world.draws.iter().all(|draw| draw.count > 0),
        "overlay 分段必须仍然自洽"
    );
    // 世界坐标而不是 NDC：数值应落在千级像素附近，而不是 [-1, 1]。
    let max_x = world
        .world_sprites
        .iter()
        .map(|vertex| vertex.position[0])
        .fold(f32::MIN, f32::max);
    assert!(max_x > 100.0, "顶点应是世界坐标：max_x={max_x}");
    assert!(
        world
            .world_sprites
            .iter()
            .all(|vertex| vertex.position[2] == 0.0),
        "瓦片层就在世界平面 z=0"
    );
}

/// WP6 回滚档：开关默认关闭（精灵走 overlay 的 NDC 顶点），打开后精灵改走世界坐标 +
/// 深度的通道，两条路径的精灵集合相同（同一个批处理），只是产出形态不同。
#[test]
fn world_sprite_depth_flag_switches_between_overlay_and_world_vertices() {
    let root = game_root();
    let mut scene = rustlua_scene::scene::Scene::new("world-sprite-flag");
    let id = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(id, "Hero", None).unwrap();
    let entity = scene.entities.get_mut(&id).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity.components.insert(
        "Sprite".into(),
        "texture=assets/textures/player.png;size=16x16;pivot=0.5,0.5;z=-0.25".into(),
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

    // 默认：overlay 路径——有 NDC 顶点、没有世界顶点。
    assert!(!runtime.world_sprite_depth());
    assert!(runtime.sprite_view_projection(640, 360).is_none());
    let overlay = runtime.vertices(640, 360).expect("overlay vertices");
    assert!(!overlay.vertices.is_empty(), "overlay 应有 NDC 顶点");
    assert!(overlay.world_sprites.is_empty(), "关闭时不应产出世界顶点");

    // 打开：世界路径——世界坐标顶点（含 `z=-0.25`），overlay 精灵顶点为空。
    runtime.set_world_sprite_depth(true);
    assert!(runtime.sprite_view_projection(640, 360).is_some());
    let world = runtime.vertices(640, 360).expect("world vertices");
    assert!(!world.world_sprites.is_empty(), "打开时应产出世界顶点");
    assert_eq!(world.world_sprites.len(), 6, "一个精灵 6 个顶点");
    assert_eq!(
        world.world_sprites[0].position[2], -0.25,
        "z 来自 Sprite::z"
    );
    assert!(
        (world.world_sprites[0].position[0] - 632.0).abs() < 1e-3,
        "世界坐标应为 640-8：{:?}",
        world.world_sprites[0].position
    );
    assert!(world.world_sprites[0].position[0] > 1.0, "世界坐标不是 NDC");
    assert!(!world.world_sprite_draws.is_empty(), "应有世界绘制分段");

    // 关回去：逐字节回到 overlay 路径。
    runtime.set_world_sprite_depth(false);
    let back = runtime.vertices(640, 360).expect("overlay vertices again");
    assert_eq!(back.vertices.len(), overlay.vertices.len());
    assert!(back.world_sprites.is_empty());
}

/// docs/54 §8-④：世界通道的可见域是**宿主声明的世界视图**，不是 2D 相机。
///
/// 回归背景（`wp0_mixed_golden` 抓到的真实误丢）：2D 相机活在设计分辨率像素空间
/// （默认中心 (640,360)），而 3D 世界视图里 `(-40, 0)` 这类坐标清晰可见。场景里有 3D 内容时
/// 宿主会声明 3D 主视图，此时若仍按 2D 相机裁剪，世界内容会被**静默丢掉**。
#[test]
fn world_channel_does_not_cull_by_the_2d_camera_when_the_scene_has_3d_content() {
    let root = game_root();
    let mut scene = Scene::new("world-view-cull");
    let sprite = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(sprite, "FarSprite", None).unwrap();
    let entity = scene.entities.get_mut(&sprite).unwrap();
    // 默认 2D 相机中心 (640,360)、zoom 1 ⇒ 世界 (-40,0) 落在 2D 视口之外。
    entity.components.insert(
        "Transform".into(),
        "position=-40,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity.components.insert(
        "Sprite".into(),
        "texture=assets/textures/red-1x1.png;size=2x2;pivot=0.5,0.5;z=0".into(),
    );

    // ① 纯 2D 画面：世界通道仍按 2D 视口剔除（性能档，既有行为不变）。
    let mut flat =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone()).expect("flat");
    flat.set_world_sprite_depth(true);
    assert!(!flat.world_view_has_3d_content());
    let flat_mesh = flat.vertices(1280, 720).expect("flat vertices");
    assert!(
        flat_mesh.world_sprites.is_empty(),
        "纯 2D 画面应继续按 2D 视口剔除视口外的精灵"
    );

    // ② 同一个精灵 + 一个网格：世界视图变成 3D，世界通道不得再按 2D 相机剔除。
    let wall = rustlua_scene::scene::SceneEntityId(2);
    scene.add_entity(wall, "Wall", None).unwrap();
    let entity = scene.entities.get_mut(&wall).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    let mut mixed = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("mixed");
    mixed.set_world_sprite_depth(true);
    assert!(
        mixed.world_view_has_3d_content(),
        "场景里有 MeshRenderer 时必须认定为有 3D 内容"
    );
    let mixed_mesh = mixed.vertices(1280, 720).expect("mixed vertices");
    assert_eq!(
        mixed_mesh.world_sprites.len(),
        6,
        "有 3D 视图时世界内容不得被 2D 相机剔除（这正是被 golden 抓到的误丢）"
    );
}

/// docs/54 §8-①：**场景声明的发射器**要真的持续发射（而不是只被解析出来躺在组件表里）。
///
/// 走的是真实固定步流水线（`run_script_steps`），因此覆盖"系统注册顺序 + 累计器写回 +
/// 位置取世界变换 + 复用 Lua burst 实现"整条链。
#[test]
fn scene_declared_emitter_emits_continuously_through_the_step_pipeline() {
    let root = game_root();
    let mut scene = Scene::new("scene-emitter");
    let source = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(source, "Sparks", None).unwrap();
    let entity = scene.entities.get_mut(&source).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    // 60 粒子/秒、步长 1/60 秒 ⇒ 每步恰好 1 个。
    entity.components.insert(
        "ParticleEmitter".into(),
        "count=2;rate=60;speed=10;life=0.5;size=2;color=1,1,1,1".into(),
    );
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    let input = rustlua_input::input::InputSnapshot::default();
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
    }
    assert_eq!(
        runtime.last_scene_emission(),
        1,
        "60/秒 ÷ 60 步/秒 ⇒ 每步一个粒子"
    );
    assert_eq!(
        runtime.with_script(|script| script.particle_count()),
        3,
        "三步应累计三个粒子（寿命 0.5s，期间都还在）"
    );

    // burst-only（rate = 0）的发射器不参与连续发射。
    let mut burst_scene = Scene::new("burst-only");
    let id = rustlua_scene::scene::SceneEntityId(1);
    burst_scene.add_entity(id, "Puff", None).unwrap();
    let entity = burst_scene.entities.get_mut(&id).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity
        .components
        .insert("ParticleEmitter".into(), "count=8;life=0.5".into());
    let mut burst_runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), burst_scene).expect("runtime");
    burst_runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(burst_runtime.last_scene_emission(), 0);
    assert_eq!(
        burst_runtime.with_script(|script| script.particle_count()),
        0
    );
}

/// P3-R07-1 出口门：`mode=gpu` 的场景发射器**照样在 CPU 上把粒子发出来**（不丢粒子、
/// 不黑屏），回退原因如实上报；宿主告知 compute 能力后原因从"设备不行"变成"参数不支持"。
#[test]
fn gpu_mode_emitters_run_on_cpu_and_report_their_fallback() {
    let root = game_root();
    let input = rustlua_input::input::InputSnapshot::default();
    let runtime_with_mode = |mode: &str| {
        let mut scene = Scene::new("particle-mode");
        let id = rustlua_scene::scene::SceneEntityId(1);
        scene.add_entity(id, "Sparks", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "ParticleEmitter".into(),
            format!("count=2;rate=60;speed=10;life=0.5;size=2;color=1,1,1,1;mode={mode}"),
        );
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime")
    };

    // 默认档：cpu 发射器的报告是干净的（没有请求、没有回退）。
    let mut cpu = runtime_with_mode("cpu");
    cpu.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    assert_eq!(cpu.last_scene_emission(), 1);
    assert_eq!(
        cpu.last_particle_placement(),
        ParticlePlacementReport {
            cpu: 1,
            ..ParticlePlacementReport::default()
        },
        "cpu 发射器不该出现在 gpu 请求或回退里"
    );

    // 宿主还没告知能力 ⇒ 按"设备没有 compute"回退，但粒子照发。
    let mut gpu = runtime_with_mode("gpu");
    gpu.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    assert_eq!(gpu.last_scene_emission(), 1, "回退不丢粒子");
    let report = gpu.last_particle_placement();
    assert_eq!((report.cpu, report.gpu, report.gpu_requested), (1, 0, 1));
    assert_eq!(
        report.fallback,
        Some(ParticleCpuReason::NoCompute),
        "没被告知能力时只能说 no_compute，不能假装 GPU 可用"
    );
    assert_eq!(
        report.fallback_bits,
        1 << ParticleCpuReason::NoCompute.index()
    );

    // 有了 compute：连续发射（rate > 0）仍是 R07-2 的范围，原因换成 continuous_rate。
    gpu.set_particle_compute_available(true);
    gpu.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    let report = gpu.last_particle_placement();
    assert_eq!(gpu.last_scene_emission(), 1, "换原因不换结果：粒子照发");
    assert_eq!(report.fallback, Some(ParticleCpuReason::ContinuousRate));
    assert_eq!(report.gpu, 0, "R07-1 还没有 GPU 派发通道");
    // 每种原因只喊一层：两步两种原因各记一位，不逐帧刷屏。
    assert_eq!(
        gpu.particle_warned,
        (1 << ParticleCpuReason::NoCompute.index())
            | (1 << ParticleCpuReason::ContinuousRate.index())
    );
}
