//! 帧提取的**稳定性**：同一份世界连提两次，读到的必须是同一份网格。
//!
//! ## 为什么单开一个域
//!
//! 桌面宿主每帧的顺序是"跑逻辑步 → `vertices()` → 渲染"，所以"不跑步、只提网格"这条路只在
//! **新宿主**上出现（web 的 GPU 通道现在就是逐帧 `vertices()` + present，模拟还没接管）。
//! 实测在浏览器里读到网格**随帧变大**（`vertices` 894 → 2136、`draws` 6 → 11，而 `paths` 恒为 22），
//! 于是这里把那个读数搬回原生：不跑步地连提若干次，看计数是否稳定。
//!
//! 这一问必须用原生测试而不是浏览器读数来回答：浏览器只能给一个时间点上的快照，
//! 而且分不清"世界真的变了"与"提取自己攒了东西"。

use super::*;
use rustlua_scene::scene::Scene;

/// 一次提取的**分通道计数**——只看总数会把"哪一路在涨"这一点丢掉。
#[derive(Debug, PartialEq, Eq)]
struct Counts {
    vertices: usize,
    draws: usize,
    paths: usize,
    world_sprites: usize,
    vector_vertices: usize,
}

fn extract(runtime: &mut RuntimeScene) -> Counts {
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    Counts {
        vertices: mesh.vertices.len(),
        draws: mesh.draws.len(),
        paths: mesh.paths.len(),
        world_sprites: mesh.world_sprites.len(),
        vector_vertices: mesh.vector_vertices.len(),
    }
}

fn sprite_scene(name: &str) -> Scene {
    let mut scene = Scene::new(name);
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/art/buildings/build_forge_stand.png;size=64x64".into(),
        );
    }
    scene
}

/// 最小世界：连提 8 次，每次都必须与第一次**逐通道相同**。
#[test]
fn repeated_extraction_without_steps_is_stable() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), sprite_scene("stable"))
        .expect("runtime scene");
    let first = extract(&mut runtime);
    for round in 1..8 {
        let next = extract(&mut runtime);
        assert_eq!(
            next, first,
            "第 {round} 次提取与第 1 次不同：不跑逻辑步时，同一份世界必须提出同一份网格"
        );
    }
}

/// 编辑态（`from_scene_edit`，不跑调度）的 Spine 必须**画得出来**。
///
/// 回归的是这条：`instantiate` 把 `output` 初始化成空，而填充它的是 `spine_advance`
/// 系统——编辑态不跑固定步，于是场景视图里挂了 `SpineSkeleton` 的实体什么都不显示
/// （Play 里正常，因为那里每步都推进）。修法是提取时对"还没采过样的骨架"补一次
/// `snap_pose()`（`dt = 0`）。
///
/// 这条门只看"有没有几何"，不钉具体顶点数：spineboy 的 setup pose 会随资产版本变，
/// 钉数字只会让未来换资产时无谓地红。
#[test]
fn editor_mode_spine_produces_geometry_without_running_the_clock() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    let manifest = root.join("assets/spine/spineboy.spine");
    if !manifest.is_file() {
        // 示例资产不在这台机器上：显式跳过，不伪造通过。
        eprintln!("[skip] 缺 {}", manifest.display());
        return;
    }
    let mut scene = Scene::new("spine_edit");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "SpineSkeleton".into(),
            "data=assets/spine/spineboy.spine;animation=idle;skin=default;speed=1;scale=1;z=0;loop=true;enabled=true;sorting_layer=default;order=10".into(),
        );
        entity.components.insert(
            "Transform".into(),
            "position=1000,800,0;rotation=0,0,0,1;scale=2,2,1".into(),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    runtime.set_spatial_inheritance(true);
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    assert!(
        !mesh.spines.vertices.is_empty(),
        "编辑态提取必须产出 Spine 顶点（否则场景视图里骨架不可见）"
    );
    assert!(
        !mesh.spines.draws.is_empty(),
        "有顶点就必须有 draw：否则几何提交了但不会画"
    );
    assert!(
        mesh.spines
            .vertices
            .iter()
            .any(|vertex| vertex.position[0] > 900.0 && vertex.position[1] > 700.0),
        "Spine 顶点应应用实体的世界位置与缩放，而不是保留骨架局部坐标"
    );
    // 索引必须落在顶点范围内，否则渲染器直接丢弃这批三角形。
    let vertex_count = mesh.spines.vertices.len();
    assert!(
        mesh.spines
            .indices
            .iter()
            .all(|index| *index < vertex_count as u32),
        "索引越界：{vertex_count} 个顶点"
    );
}

/// 跑逻辑步（Play 路径）之后，Spine 必须有几何。
///
/// 与上面那条编辑态用例配对：两条路径都必须画得出骨架。Play 走 `run_script_steps`，
/// 其中的 `spine_advance` 每步采样；编辑态没有固定步，靠提取时的 `snap_pose()` 保底。
#[test]
fn playing_spine_produces_geometry_after_a_step() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    let manifest = root.join("assets/spine/spineboy.spine");
    if !manifest.is_file() {
        eprintln!("[skip] 缺 {}", manifest.display());
        return;
    }
    let mut scene = Scene::new("spine_play");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "SpineSkeleton".into(),
            "data=assets/spine/spineboy.spine;animation=idle;skin=default;speed=1;scale=1;z=0;loop=true;enabled=true;sorting_layer=default;order=10".into(),
        );
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    }
    let mut runtime = RuntimeScene::from_scene(Some(root.as_path()), scene).expect("runtime scene");
    let input = rustlua_input::input::InputSnapshot::default();
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("逻辑步");
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(
            !mesh.spines.vertices.is_empty(),
            "Play 路径跑完逻辑步后 Spine 必须产出顶点"
        );
    }
}

/// Spine 的 draw 必须指向**真实存在**的图集页，否则几何提交了但采样到空白纹理。
///
/// 这是"看起来完全没渲染"最常见的形态：顶点/索引/draw 都对，只有纹理解析不出来。
/// `.spineatlas` 里的页名是相对 atlas 文件的（`spineboy.png`），装配时若不拼上
/// atlas 所在目录，就会去工程根找同名文件并失败。
#[test]
fn spine_draws_point_at_real_atlas_pages() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    let manifest = root.join("assets/spine/spineboy.spine");
    if !manifest.is_file() {
        eprintln!("[skip] 缺 {}", manifest.display());
        return;
    }
    let mut scene = Scene::new("spine_tex");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "SpineSkeleton".into(),
            "data=assets/spine/spineboy.spine;animation=idle;skin=default;speed=1;scale=1;z=0;loop=true;enabled=true;sorting_layer=default;order=10".into(),
        );
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    assert!(!mesh.spines.draws.is_empty(), "应有 draw");
    // `texture` 是登记进本帧纹理表的 id；`paths` 与 id 同序，据此反查路径。
    for draw in &mesh.spines.draws {
        let path = mesh.paths.get(draw.texture as usize).unwrap_or_else(|| {
            panic!(
                "draw.texture={} 越界（paths={}）",
                draw.texture,
                mesh.paths.len()
            )
        });
        assert!(
            !path.is_empty(),
            "Spine draw 的纹理路径是空的：意味着页表查不到"
        );
        let resolved = root.join(path);
        assert!(
            resolved.is_file(),
            "Spine 图集页解析到不存在的文件：{path}（应含 atlas 目录，如 assets/spine/spineboy.png）"
        );
    }
}

/// 诊断：装配之后 `SpinePlayer` 是否真的进了 world，以及它的 `enabled` / 动画名。
///
/// 这条不判红，只打印——用来把"装配失败"与"装配成功但没采样"分开。两者的表现
/// 完全一样（场景里什么都看不到），但修法不同。
#[test]
fn diagnose_spine_player_presence() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    if !root.join("assets/spine/spineboy.spine").is_file() {
        eprintln!("[skip] 缺 spine 资产");
        return;
    }
    let mut scene = Scene::new("probe");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "SpineSkeleton".into(),
            "data=assets/spine/spineboy.spine;animation=idle;skin=default;speed=1;scale=1;z=0;loop=true;enabled=true;sorting_layer=default;order=10".into(),
        );
    }
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    let mut rows: Vec<(String, usize, bool, String)> = Vec::new();
    runtime.script.with_world(|world| {
        world.for_each::<rustlua_spine::SpinePlayer>(|id, player| {
            rows.push((
                format!("{id:?}"),
                player.output().vertices.len(),
                player.component().enabled,
                player.component().animation.clone(),
            ));
        });
    });
    eprintln!("[probe] SpinePlayer count={} rows={rows:?}", rows.len());
    assert!(
        !rows.is_empty(),
        "装配后 world 里应该有 SpinePlayer：没有的话说明 SpineSkeleton 组件没被装配"
    );
    // 首次提取（不预先手动 snap）——这是编辑器真实路径，必须自带几何。
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    eprintln!(
        "[probe] mesh.spines vertices={} draws={} indices={}",
        mesh.spines.vertices.len(),
        mesh.spines.draws.len(),
        mesh.spines.indices.len()
    );
}
