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
use crate::core::scene::Scene;

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
    let root = crate::core::games_dir().join("examples").join("game_jhyt");
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

/// 工程真场景（有实体的那个）：同一问，用产物里那份场景文件。
///
/// 找不到工程就跳过——这条断言的价值在"用真数据跑"，不是"必须有真数据"。
#[test]
fn project_scene_repeated_extraction_is_stable() {
    let root = crate::core::games_dir().join("examples").join("game_jhyt");
    let path = root.join("scenes").join("village.scene");
    if !path.is_file() {
        return;
    }
    let scene = Scene::load(&path).expect("load village scene");
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let first = extract(&mut runtime);
    // 场景本身要有东西可画，否则"稳定"是平凡成立的、什么也证明不了。
    assert!(
        first.draws > 0 && first.vertices > 0,
        "village 场景应当提出非空网格，实际 {first:?}"
    );
    for round in 1..8 {
        let next = extract(&mut runtime);
        assert_eq!(
            next, first,
            "第 {round} 次提取与第 1 次不同：不跑逻辑步时，同一份世界必须提出同一份网格"
        );
    }
}
