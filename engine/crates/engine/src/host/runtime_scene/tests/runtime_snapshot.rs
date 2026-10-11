//! Play 期间编辑器层级面板要用的运行态实体读数（`runtime_spawned_entities` /
//! `scene_entity_local_transforms`）。
//!
//! 这道读数存在的理由：编辑器在 Play 时要把"游戏里现在到底有哪些实体"显示给作者，
//! 而脚本运行期 `entity.spawn` 出来的实体**不在 `.scene` 文档里**，只有世界能回答。

use super::*;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::hierarchy::Parent;
use rustlua_scene::scene::Scene;
// 与生产代码同一个编码器：断言里的期望值不能是"手抄一遍编码"，否则两处一起写错就没人发现。
use crate::host::runtime_scene::scene::encode_handle;

/// 夹具：一个带 Transform 的声明实体；`from_scene_edit` 不跑工程脚本，
/// 但 Lua API 表已经注册，测试可以直接 `entity.spawn`。
fn runtime_with_one_declared_entity() -> RuntimeScene {
    let mut scene = Scene::new("runtime_snapshot");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=100,200,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    RuntimeScene::from_scene_edit(None, scene).expect("runtime scene")
}

fn console(runtime: &mut RuntimeScene, source: &str) {
    runtime
        .with_script(|script| script.execute_console(source))
        .unwrap_or_else(|error| panic!("console failed: {error}"));
}

/// 运行期 spawn 的实体进表、声明实体不进表（声明实体在镜像里已有自己的行）。
#[test]
fn runtime_snapshot_lists_only_script_created_entities() {
    let mut runtime = runtime_with_one_declared_entity();
    console(&mut runtime, "entity.spawn('Bullet')");

    let spawned = runtime.runtime_spawned_entities();
    let names: Vec<&str> = spawned.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec!["Bullet"], "声明实体不该被当成运行期生成");
}

/// spawn 与 destroy 之间表的内容随世界变化——层级面板因此不会列出游戏里已经没了的东西。
#[test]
fn runtime_snapshot_drops_destroyed_entities() {
    let mut runtime = runtime_with_one_declared_entity();
    console(&mut runtime, "bullet = entity.spawn('Bullet')");
    assert_eq!(runtime.runtime_spawned_entities().len(), 1);

    console(&mut runtime, "entity.destroy(bullet)");
    assert!(
        runtime.runtime_spawned_entities().is_empty(),
        "销毁后仍被列出，层级就会多出一行假实体"
    );
}

/// 未登记名字的 spawn 也要有行（它带 Transform）：名字退化成 `entity.<槽位>` 而不是消失。
#[test]
fn unnamed_spawn_still_gets_a_row() {
    let mut runtime = runtime_with_one_declared_entity();
    console(&mut runtime, "entity.spawn()");

    let spawned = runtime.runtime_spawned_entities();
    assert_eq!(spawned.len(), 1);
    assert!(
        spawned[0].name.starts_with("entity."),
        "实际名字：{}",
        spawned[0].name
    );
}

/// 位置读数取世界的真实值：脚本把实体挪走之后快照必须给同一个数，
/// 否则层级/Scene 视图标的坐标与游戏里的实际位置对不上。
#[test]
fn runtime_snapshot_carries_the_live_position() {
    let mut runtime = runtime_with_one_declared_entity();
    console(
        &mut runtime,
        "local b = entity.spawn('Bullet') transform.set(b, 300, 400)",
    );

    let spawned = runtime.runtime_spawned_entities();
    assert_eq!(spawned.len(), 1);
    assert_eq!(&spawned[0].position[..2], &[300.0, 400.0]);
}

/// 句柄编码是宿主跨帧对号的键：**同一份世界两次读数必须完全相同**，而槽位被复用时
/// 必须不同（销毁再生成，代际会跳），否则上一发子弹的行会被当成这一发的。
#[test]
fn handle_key_is_stable_within_a_frame_and_distinct_after_slot_reuse() {
    let mut runtime = runtime_with_one_declared_entity();
    console(&mut runtime, "first = entity.spawn('Bullet')");
    let first = runtime.runtime_spawned_entities();
    assert_eq!(first, runtime.runtime_spawned_entities());

    console(&mut runtime, "entity.destroy(first) entity.spawn('Bullet')");
    let second = runtime.runtime_spawned_entities();
    assert_eq!(second.len(), 1);
    assert_ne!(
        first[0].handle, second[0].handle,
        "同名实体复用槽位时，只按槽位号对号会把上一发子弹的行认成这一发的"
    );
}

/// 父链用句柄表达：宿主拿它把运行期实体挂回镜像里的正确父节点（父可以是声明实体）。
#[test]
fn runtime_snapshot_reports_the_parent_handle() {
    let mut runtime = runtime_with_one_declared_entity();
    console(&mut runtime, "entity.spawn('Child')");
    let child_key = runtime.runtime_spawned_entities()[0].handle;
    // 声明实体的句柄由运行时登记表给出；真实游戏里这条边由 prefab 实例化建立，
    // 这里直接写组件，测的是快照怎么读它。
    let declared = runtime
        .script
        .scene_entity_handle(SceneEntityId(1))
        .unwrap();
    runtime.with_script(|script| {
        script.with_world_mut(|world| {
            let child = EntityId::from_raw((child_key >> 32) as u32, child_key as u32);
            world.insert(child, Parent(declared)).unwrap();
        });
    });

    let spawned = runtime.runtime_spawned_entities();
    assert_eq!(spawned[0].parent, Some(encode_handle(declared)));
}

/// 「这个运行期实体是 UI 行」这一位必须来自**活世界的组件类型**：编辑器靠它决定不按姿态
/// 给运行期 UI 画假框（镜像里没有它们的版面载荷，见 `editor` 的 docs/56 §3.53）。
/// 判据不能是名字/路径的启发式——界面实体的版面数据只存在组件里。
#[test]
fn runtime_snapshot_flags_ui_rows_from_the_live_world() {
    let mut runtime = runtime_with_one_declared_entity();
    console(&mut runtime, "entity.spawn('Bullet') entity.spawn('Panel')");
    let before = runtime.runtime_spawned_entities();
    assert!(
        before.iter().all(|row| !row.ui),
        "只有姿态（`Transform`）的实体不是 UI 行：{:?}",
        before.iter().map(|row| &row.name).collect::<Vec<_>>()
    );

    // 版面数据只在组件里，所以直接往世界插一个 `UiLayout`——判据必须跟着变。
    let panel_key = before
        .iter()
        .find(|row| row.name == "Panel")
        .expect("Panel 行")
        .handle;
    let panel = EntityId::from_raw((panel_key >> 32) as u32, panel_key as u32);
    runtime.with_script(|script| {
        script.with_world_mut(|world| {
            world
                .insert(panel, rustlua_ui::ui_layout::UiRectLayout::default())
                .unwrap();
        });
    });

    let after = runtime.runtime_spawned_entities();
    assert!(
        after.iter().find(|row| row.name == "Panel").unwrap().ui,
        "带 UiLayout 的运行期实体必须是 UI 行，否则编辑器会按姿态给它画一个假框"
    );
    assert!(
        !after.iter().find(|row| row.name == "Bullet").unwrap().ui,
        "同一次读数里非 UI 行不能被一起标上"
    );
}

/// 声明实体的活变换：仍在世界里的给出当前值，销毁后从表里消失（镜像据此摘掉那一行）。
#[test]
fn declared_live_transforms_track_the_world() {
    let mut runtime = runtime_with_one_declared_entity();
    let hero = SceneEntityId(1);
    assert_eq!(
        runtime
            .scene_entity_local_transforms()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        vec![hero],
        "声明实体实例化后就该有活变换读数"
    );

    console(&mut runtime, "transform.set(entity.find('Hero'), 42, 7)");
    assert_eq!(
        runtime.scene_entity_local_transforms()[0].1.translation.x,
        42.0
    );

    console(&mut runtime, "entity.destroy(entity.find('Hero'))");
    assert!(
        runtime.scene_entity_local_transforms().is_empty(),
        "销毁后仍给出读数，镜像里会留下一行假实体"
    );
}

/// 编辑期新增、还没实例化过的文档行**不进登记表**，因此也不该被当成"已销毁"——
/// 镜像同步用「登记过句柄 且 不在活变换表里」判定死亡，两件事缺一不可。
#[test]
fn unregistered_authored_row_is_not_reported_as_dead() {
    let runtime = runtime_with_one_declared_entity();
    assert!(
        runtime
            .scene_entity_handles()
            .into_iter()
            .all(|(id, _)| id != SceneEntityId(9_999)),
        "未登记的 id 不该出现在句柄表里"
    );
    assert!(
        !runtime
            .scene_entity_local_transforms()
            .iter()
            .any(|(id, _)| *id == SceneEntityId(9_999)),
        "未登记的 id 也不该出现在活变换表里"
    );
}
