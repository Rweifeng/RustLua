//! 原 `mod sdf_tests` 里 `spatial` 这一域（WP12 阶段 1 纯搬运；源行段 6864–7246）。
//!
//! - `static_hierarchy_skips_propagation_until_something_changes`
//! - `incremental_propagation_recomputes_only_changed_subtrees`
//! - `runtime_and_document_agree_on_the_canonical_local_transform`
//! - `non_planar_local_transform_is_refused_for_2d_entities`

use super::*;
use crate::core::scene::Scene;

/// docs/54 §10 批 R：**输入没变就跳过传播**——静态层级场景不再付那三成帧成本，而任何一处
/// 局部真源变化都必须立刻退回全量传播。
///
/// 判据分四段，缺一段这条优化就不成立：① 不动的帧确实在跳过（世界值仍正确）；② 改**自己**的局部值
/// 会传播；③ 改**父**的局部值也会传播（子自己的局部没变，但世界值必须跟着父走——这一条最容易漏）；
/// ④ 回滚档（`set_spatial_skip_unchanged(false)`）每帧都传播。
#[test]
fn static_hierarchy_skips_propagation_until_something_changes() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let parent = SceneEntityId(1);
    let child = SceneEntityId(2);
    let mut scene = Scene::new("spatial_skip");
    scene.metadata.insert("spatial".into(), "local".into());
    scene.add_entity(parent, "Rig", None).unwrap();
    scene.add_entity(child, "Part", Some(parent)).unwrap();
    scene.entities.get_mut(&parent).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=2,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let input = crate::core::input::InputSnapshot::default();
    let child_world_x = |runtime: &RuntimeScene| {
        runtime
            .world_transform_2d_of(child)
            .expect("child world transform")
            .position
            .x
    };

    // ① 连跑三步：除第一帧外都应跳过，世界值保持 (12, 0)。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(child_world_x(&runtime), 12.0);
    let skipped_before = runtime.spatial_skipped_frames();
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
    }
    // 每步会调用两次传播（步末一次 + `spatial_propagate` 帧系统一次），因此用下界断言：
    // "不动的帧确实在跳过"这件事不依赖调用次数。
    assert!(
        runtime.spatial_skipped_frames() >= skipped_before + 3,
        "输入没变的帧必须整趟跳过传播（实际 {}，起点 {skipped_before}）",
        runtime.spatial_skipped_frames()
    );
    assert_eq!(child_world_x(&runtime), 12.0, "跳过不等于世界值失效");

    // ② 改自己的局部值 ⇒ 必须传播。
    let moved = crate::core::spatial_transform::LocalTransform {
        translation: crate::core::math3d::Vec3::new(5.0, 0.0, 0.0),
        ..crate::core::spatial_transform::LocalTransform::identity()
    };
    runtime
        .set_local_transform(child, moved)
        .expect("write child local");
    let skipped = runtime.spatial_skipped_frames();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    // 一步里有两次传播调用（步末 + 帧系统）：变化后的**第一次**必须真传播，第二次才可以跳过。
    // 因此这里判两件事：世界值真的更新了（说明没跳过），且跳过增量至多 1。
    assert_eq!(child_world_x(&runtime), 15.0, "10 + 5：局部值变了必须传播");
    assert!(
        runtime.spatial_skipped_frames() - skipped <= 1,
        "变化后的第一次传播不许跳过（跳过增量 {}）",
        runtime.spatial_skipped_frames() - skipped
    );

    // ③ 改**父**的局部值：子自己的局部没变，但世界值必须跟着父走。
    let parent_moved = crate::core::spatial_transform::LocalTransform {
        translation: crate::core::math3d::Vec3::new(20.0, 0.0, 0.0),
        ..crate::core::spatial_transform::LocalTransform::identity()
    };
    runtime
        .set_local_transform(parent, parent_moved)
        .expect("write parent local");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(
        child_world_x(&runtime),
        25.0,
        "父的局部值变化必须传到子的世界值（指纹覆盖父链，不只是自己的局部）"
    );

    // ④ 回滚档：关掉跳过 ⇒ 每帧都传播。
    assert!(
        runtime.set_spatial_skip_unchanged(false),
        "默认是开启的（返回前一个值）"
    );
    assert!(!runtime.spatial_skip_unchanged());
    let skipped = runtime.spatial_skipped_frames();
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
    }
    assert_eq!(
        runtime.spatial_skipped_frames(),
        skipped,
        "回滚档下不许跳过任何一帧"
    );
    assert_eq!(child_world_x(&runtime), 25.0);
    // 再打开：第一帧必须重新全量传播一次（清过指纹），之后才允许跳过。
    assert!(!runtime.set_spatial_skip_unchanged(true));
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    let skipped = runtime.spatial_skipped_frames();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert!(
        runtime.spatial_skipped_frames() > skipped,
        "重新打开后第一帧全量传播、之后允许跳过"
    );
    assert_eq!(child_world_x(&runtime), 25.0);
}

/// docs/54 §10 批 S：**逐实体增量**在真实帧里只重算变了的子树。
///
/// 判据分四段：① 首帧全量（建立缓存）；② 只挪链尾 ⇒ 路径是增量且**重算数 = 1**（不是实体
/// 总数）；③ 挪链根 ⇒ 重算整条链（世界值是沿父链组合的，父一变整棵子树过期）；④ 回滚档
/// （`set_spatial_incremental(false)`）每帧全量、世界值一致。
#[test]
fn incremental_propagation_recomputes_only_changed_subtrees() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    // 一条 3 层的链 root → middle → leaf，每层局部 x 偏移。
    let (ring, middle, leaf) = (SceneEntityId(1), SceneEntityId(2), SceneEntityId(3));
    let mut scene = Scene::new("spatial_incremental");
    scene.metadata.insert("spatial".into(), "local".into());
    scene.add_entity(ring, "Rig", None).unwrap();
    scene.add_entity(middle, "Arm", Some(ring)).unwrap();
    scene.add_entity(leaf, "Hand", Some(middle)).unwrap();
    for (id, x) in [(ring, 10.0), (middle, 2.0), (leaf, 1.0)] {
        scene.entities.get_mut(&id).unwrap().components.insert(
            "Transform".into(),
            format!("position={x},0,0;rotation=0,0,0,1;scale=1,1,1"),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let input = InputSnapshot::default();
    let world_x = |runtime: &RuntimeScene, entity: SceneEntityId| {
        runtime
            .world_transform_2d_of(entity)
            .unwrap_or_else(|| panic!("entity {entity:?} 没有世界变换"))
            .position
            .x
    };
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(world_x(&runtime, leaf), 13.0);
    // ① 首帧没有缓存可比 ⇒ 全量。
    assert!(
        runtime.spatial_path_counts()[1] >= 1,
        "首帧必须全量传播一次（计数 {:?}）",
        runtime.spatial_path_counts()
    );

    // ② 只挪链尾：一帧里重算的实体数应当是 1。
    let moved_leaf = crate::core::spatial_transform::LocalTransform {
        translation: crate::core::math3d::Vec3::new(5.0, 0.0, 0.0),
        ..crate::core::spatial_transform::LocalTransform::identity()
    };
    runtime
        .set_local_transform(leaf, moved_leaf)
        .expect("write leaf local");
    let _ = runtime.take_spatial_path_counts();
    let _ = runtime.take_spatial_recomputed_total();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(world_x(&runtime, leaf), 17.0, "10 + 2 + 5");
    assert_eq!(
        runtime.take_spatial_recomputed_total(),
        1,
        "只挪一个叶子必须只重算 1 个实体（步末那次；帧系统那次看到指纹没变会跳过）"
    );
    let counts = runtime.take_spatial_path_counts();
    assert!(counts[2] >= 1, "本步应当走过增量路径（计数 {counts:?}）");

    // ③ 挪链根：整条链都要重算（3 个实体）。
    let moved_root = crate::core::spatial_transform::LocalTransform {
        translation: crate::core::math3d::Vec3::new(20.0, 0.0, 0.0),
        ..crate::core::spatial_transform::LocalTransform::identity()
    };
    runtime
        .set_local_transform(ring, moved_root)
        .expect("write root local");
    let _ = runtime.take_spatial_recomputed_total();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(world_x(&runtime, leaf), 27.0, "20 + 2 + 5");
    assert_eq!(
        runtime.take_spatial_recomputed_total(),
        3,
        "挪链根必须重算整条链（父的世界值一变，整棵子树过期）"
    );

    // ④ 回滚档（`set_spatial_incremental(false)`）：值变化后一律全量，不再走增量。
    //    （"每帧都传播"是另一个开关 `set_spatial_skip_unchanged` 的语义，批 R 的测试管它。）
    assert!(
        runtime.set_spatial_incremental(false),
        "默认开启（返回前一个值）"
    );
    assert!(!runtime.spatial_incremental());
    let _ = runtime.take_spatial_path_counts();
    for step in 0..3 {
        let moved = crate::core::spatial_transform::LocalTransform {
            translation: crate::core::math3d::Vec3::new(1.0 + step as f32, 0.0, 0.0),
            ..crate::core::spatial_transform::LocalTransform::identity()
        };
        runtime
            .set_local_transform(leaf, moved)
            .expect("write leaf local");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
    }
    let counts = runtime.take_spatial_path_counts();
    assert_eq!(counts[2], 0, "回滚档下不许走增量路径（计数 {counts:?}）");
    assert!(
        counts[1] >= 3,
        "回滚档下每次值变化都要全量（计数 {counts:?}）"
    );
    assert_eq!(world_x(&runtime, leaf), 25.0, "20 + 2 + 3");
    // 重新打开：缓存被清，因此**下一次值变化**必须全量一次（此后又能走增量）。
    assert!(!runtime.set_spatial_incremental(true));
    let _ = runtime.take_spatial_path_counts();
    runtime
        .set_local_transform(leaf, moved_leaf)
        .expect("write leaf local");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(world_x(&runtime, leaf), 27.0, "20 + 2 + 5");
    let counts = runtime.spatial_path_counts();
    assert!(
        counts[1] >= 1,
        "重开后的第一次值变化必须全量（计数 {counts:?}）"
    );
    assert_eq!(
        counts[2], 0,
        "缓存刚建好，这一步还走不了增量（计数 {counts:?}）"
    );
    // 再改一次值 ⇒ 走增量，且只重算那一个叶子。
    let moved_again = crate::core::spatial_transform::LocalTransform {
        translation: crate::core::math3d::Vec3::new(6.0, 0.0, 0.0),
        ..crate::core::spatial_transform::LocalTransform::identity()
    };
    runtime
        .set_local_transform(leaf, moved_again)
        .expect("write leaf local");
    let _ = runtime.take_spatial_recomputed_total();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step");
    assert_eq!(world_x(&runtime, leaf), 28.0, "20 + 2 + 6");
    assert_eq!(
        runtime.take_spatial_recomputed_total(),
        1,
        "缓存建好之后的第二次值变化又只重算那个叶子"
    );
}

/// docs/54 §4.1：**文档侧与运行时侧的局部变换必须是同一份规范表示**。
///
/// 这条是"运行时只有一份权威 `LocalTransform`"在**表示层**的可执行判据：对一个真实场景，逐实体
/// 比对 `RuntimeScene::local_transform_of`（运行时从 ECS 的边界格式转出来的 `LocalTransform`）
/// 与 `scene_space::local_transforms`（文档侧直接解析出来的 `LocalTransform`）。两者任何一处改了
/// 转换口径都会在这里失败——这正是"两份真源"最容易被引入的地方。
#[test]
fn runtime_and_document_agree_on_the_canonical_local_transform() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    let path = root.join("scenes/index.scene");
    if !path.is_file() {
        return;
    }
    let scene = Scene::load(&path).expect("load shipped scene");
    let document = crate::core::scene_space::local_transforms(&scene);
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone()).expect("runtime");
    assert!(!document.is_empty(), "场景应当带局部变换");
    for entity in scene.entities.values() {
        let Some(expected) = document.get(&entity.id).copied() else {
            continue;
        };
        let actual = runtime
            .local_transform_of(entity.id)
            .unwrap_or_else(|| panic!("运行时缺少 {} 的局部变换", entity.name));
        assert!(
            (actual.translation - expected.translation).length() < 1e-5
                && (actual.scale - expected.scale).length() < 1e-5,
            "{} 的局部变换两侧不一致：文档={expected:?} 运行时={actual:?}",
            entity.name
        );
    }

    // 写回：统一表示写进实体自己的空间，round-trip 必须回到同一个值。
    let target = scene
        .entities
        .values()
        .find(|entity| document.contains_key(&entity.id))
        .expect("至少一个带变换的实体")
        .id;
    let mut moved = document[&target];
    moved.translation = moved.translation + crate::core::math3d::Vec3::new(3.0, 4.0, 0.0);
    assert!(
        runtime
            .set_local_transform(target, moved)
            .expect("写回平坦值"),
        "实体本来就有局部变换，应当写回成功"
    );
    let read_back = runtime.local_transform_of(target).expect("读回");
    assert!(
        (read_back.translation - moved.translation).length() < 1e-5,
        "写回后读到的应是同一个值：{read_back:?} vs {moved:?}"
    );
}

/// docs/54 §4.1：写回非平面值到**只有 2D `Transform`** 的实体必须**报错**，而不是悄悄压平
/// （手抄 XY 会把旋转与缩放丢掉）。
#[test]
fn non_planar_local_transform_is_refused_for_2d_entities() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut scene = Scene::new("local_transform_write");
    scene.add_entity(SceneEntityId(1), "Flat", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=1,2,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let planar = crate::core::spatial_transform::LocalTransform {
        translation: crate::core::math3d::Vec3::new(5.0, 6.0, 0.0),
        rotation: crate::core::math3d::Quat::identity(),
        scale: crate::core::math3d::Vec3::new(2.0, 2.0, 1.0),
    };
    assert!(
        runtime
            .set_local_transform(SceneEntityId(1), planar)
            .unwrap()
    );
    let tilted = crate::core::spatial_transform::LocalTransform {
        // 沿 z 抬起：2D 平面装不下。
        translation: crate::core::math3d::Vec3::new(5.0, 6.0, 3.0),
        ..planar
    };
    let error = runtime
        .set_local_transform(SceneEntityId(1), tilted)
        .expect_err("非平面值必须被拒绝");
    assert!(error.to_string().contains("not planar"), "{error}");
    // 拒绝之后原值必须原样保留（不留半写状态）。
    let unchanged = runtime
        .local_transform_of(SceneEntityId(1))
        .expect("局部变换仍在");
    assert!((unchanged.translation.z).abs() < 1e-6);
    assert!((runtime.local_transform_of(SceneEntityId(99))).is_none());
}
