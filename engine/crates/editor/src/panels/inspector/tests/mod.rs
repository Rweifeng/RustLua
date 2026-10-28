//! `panels::inspector` 的单测：覆盖行 diff、选择性 Apply/Revert、组件插入与 schema 一致性。
//!
//! 从 `panels/inspector.rs` 末尾的 `mod tests` 原样搬出（P1 结构收口）；测试名不变。
//! 目录名 `tests/` 同时让 `panels/architecture_tests.rs` 的棘轮自动跳过它（与
//! `scene_view/tests/` 同一约定）。

use super::*;
// Prefab 写路径的纯函数住在 shell（`crate::actions`）：测试直接调它们做单元级断言。
use crate::EditorApp;
use crate::actions::{
    apply_prefab_instance_metadata, build_prefab_from_instance_root, collect_prefab_rebuild_state,
};
use editor::open_project;
use rustlua_animation::animation::AnimationStateMachine;
use rustlua_assets::resource_db::AssetId;
use rustlua_infra::logging::LogSink;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::{Scene, SceneEntity, SceneEntityId};
use rustlua_ui::ui::UiText;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be valid")
        .as_nanos()
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rustlua-m14-01-inspector-{label}-{}",
        unique_token()
    ))
}

fn sample_project_root() -> PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

fn copy_dir(source: &Path, destination: &Path) {
    if !source.is_dir() {
        return;
    }
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let target_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target_path).unwrap();
            copy_dir(&source_path, &target_path);
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&source_path, &target_path).unwrap();
        }
    }
}

fn copy_sample_project(label: &str) -> PathBuf {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "sample project missing: {}",
        source.display()
    );
    let root = temp_root(label);
    fs::create_dir_all(&root).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    copy_dir(&source.join("scripts"), &root.join("scripts"));
    copy_dir(&source.join("scenes"), &root.join("scenes"));
    copy_dir(&source.join("assets"), &root.join("assets"));
    root
}

/// 打开示例工程副本并打开 **Prefab 实例场景**（`api_prefab.scene`）。
///
/// 该场景里两个 Prefab 源（`assets/prefabs/slime_green.prefab` / `slime_red.prefab`）
/// 各有实例，实例实体带 `prefab.root` / `prefab.source` / `prefab.source_id` 元数据——
/// Prefab 的 Apply / Revert / 逐字段 Override 要有真实实例才测得动。
fn sample_app_with_production_scene(label: &str) -> (EditorApp, PathBuf) {
    let root = copy_sample_project(label);
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/api_prefab.scene"));
    (app, root)
}

/// 打开示例工程副本并打开 2D 综合场景（`main.scene`）：动画/精灵/物理这些
/// "玩法侧"组件的样本都在这里。
fn sample_app_with_main_scene(label: &str) -> (EditorApp, PathBuf) {
    let root = copy_sample_project(label);
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/main.scene"));
    (app, root)
}

/// 打开示例工程副本并打开 **场景 UI 场景**（`ui_test.scene`）。
fn sample_app_with_menu_scene(label: &str) -> (EditorApp, PathBuf) {
    let root = copy_sample_project(label);
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    (app, root)
}

fn select_entity(app: &mut EditorApp, id: SceneEntityId) {
    app.selection
        .set_single(id, editor::SelectionSource::Hierarchy);
    app.sync_inspector(id);
}

fn prefab_root(scene: &Scene, prefab_source: &str) -> SceneEntityId {
    scene
        .entities
        .values()
        .find(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
                && entity
                    .metadata
                    .get("prefab.source")
                    .is_some_and(|value| value == prefab_source)
        })
        .map(|entity| entity.id)
        .expect("sample scene should contain prefab root")
}

fn entity() -> SceneEntity {
    SceneEntity {
        id: SceneEntityId(1),
        name: "Enemy".to_owned(),
        parent: None,
        children: Vec::new(),
        components: std::collections::BTreeMap::from([
            (
                "Transform".to_owned(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
            ),
            ("Sprite".to_owned(), "texture=enemy.png".to_owned()),
        ]),
        resources: std::collections::BTreeMap::new(),
        metadata: std::collections::BTreeMap::from([("team".to_owned(), "red".to_owned())]),
    }
}

#[test]
fn diff_prefab_entity_reports_name_component_and_metadata_changes() {
    let source = entity();
    let mut instance = entity();
    instance.name = "Enemy Boss".to_owned();
    // 两边都是**合法载荷** ⇒ 走字段级覆盖（`position` 这一个字段变了）。
    instance.components.insert(
        "Transform".to_owned(),
        "position=10,4,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
    );
    // 实例上多出来的组件没有字段级对拍对象 ⇒ 仍是组件级覆盖（整串替换）。
    instance.components.insert(
        "Light".to_owned(),
        "direction=0,0,-1;color=1,1,1;intensity=1".to_owned(),
    );
    instance
        .metadata
        .insert("team".to_owned(), "blue".to_owned());
    instance.metadata.insert(
        "prefab.source".to_owned(),
        "assets/prefabs/enemy.prefab".to_owned(),
    );
    let overrides = diff_prefab_entity(&source, &instance);
    assert_eq!(
        overrides,
        vec![
            "name".to_owned(),
            // 组件按键名排序：`Light` 在 `Transform` 之前。
            "component:Light".to_owned(),
            "property:Transform.position".to_owned(),
            "metadata:team".to_owned()
        ]
    );
}

#[test]
fn collect_prefab_rebuild_state_preserves_detected_overrides() {
    let mut source = Scene::new("prefab");
    source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
    source
        .add_entity(SceneEntityId(2), "HitBox", Some(SceneEntityId(1)))
        .unwrap();
    source
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Transform".to_owned(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
    let prefab = Prefab::from_scene(
        &source,
        SceneEntityId(1),
        AssetId::from_seed("enemy.prefab"),
        "Enemy",
    )
    .unwrap();

    let mut scene = Scene::new("runtime");
    let instance = prefab
        .instantiate(&mut scene, None, &std::collections::BTreeMap::new())
        .unwrap();
    apply_prefab_instance_metadata(&mut scene, "assets/prefabs/enemy.prefab", &instance);
    scene
        .entities
        .get_mut(&instance.instance_root)
        .unwrap()
        .name = "Boss".to_owned();
    let child = instance.source_to_instance[&SceneEntityId(2)];
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".to_owned(),
        "position=9,4,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
    );

    let rebuild = collect_prefab_rebuild_state(
        &scene,
        instance.instance_root,
        "assets/prefabs/enemy.prefab",
        &prefab,
    )
    .unwrap();
    assert_eq!(
        rebuild
            .overrides
            .get(&(SceneEntityId(1), "name".to_owned())),
        Some(&"Boss".to_owned())
    );
    // 逐字段 Override：只记录真正变化的字段（原始文本），不再冻结整个组件串。
    // 统一 `Transform` 之后位置的字段名是 `position`（Vec3），不再是 `x`/`y` 两个字段。
    assert_eq!(
        rebuild
            .overrides
            .get(&(SceneEntityId(2), "property:Transform.position".to_owned())),
        Some(&"9,4,0".to_owned())
    );
    assert!(
        !rebuild
            .overrides
            .contains_key(&(SceneEntityId(2), "component:Transform".to_owned())),
        "有 schema 的组件必须走字段级覆盖，而不是整串覆盖"
    );
}

#[test]
fn collect_prefab_rebuild_state_rejects_mixed_subtree() {
    let mut source = Scene::new("prefab");
    source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
    let prefab = Prefab::from_scene(
        &source,
        SceneEntityId(1),
        AssetId::from_seed("enemy.prefab"),
        "Enemy",
    )
    .unwrap();

    let mut scene = Scene::new("runtime");
    let instance = prefab
        .instantiate(&mut scene, None, &std::collections::BTreeMap::new())
        .unwrap();
    apply_prefab_instance_metadata(&mut scene, "assets/prefabs/enemy.prefab", &instance);
    scene
        .add_entity(
            SceneEntityId(99),
            "LooseChild",
            Some(instance.instance_root),
        )
        .unwrap();

    let error = collect_prefab_rebuild_state(
        &scene,
        instance.instance_root,
        "assets/prefabs/enemy.prefab",
        &prefab,
    )
    .unwrap_err();
    assert!(error.contains("非同源节点") || error.contains("结构不一致"));
}

#[test]
fn build_prefab_from_instance_root_strips_prefab_metadata() {
    let mut source = Scene::new("prefab");
    source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
    let prefab = Prefab::from_scene(
        &source,
        SceneEntityId(1),
        AssetId::from_seed("enemy.prefab"),
        "Enemy",
    )
    .unwrap();

    let mut instance_scene = Scene::new("runtime");
    let instance = prefab
        .instantiate(
            &mut instance_scene,
            None,
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
    apply_prefab_instance_metadata(
        &mut instance_scene,
        "assets/prefabs/enemy.prefab",
        &instance,
    );
    instance_scene
        .entities
        .get_mut(&instance.instance_root)
        .unwrap()
        .metadata
        .insert("designer.note".to_owned(), "boss".to_owned());

    let rebuilt =
        build_prefab_from_instance_root(&instance_scene, instance.instance_root, &prefab).unwrap();
    assert!(
        !rebuilt.entities[&rebuilt.root]
            .metadata
            .contains_key("prefab.source")
    );
    assert_eq!(
        rebuilt.entities[&rebuilt.root]
            .metadata
            .get("designer.note"),
        Some(&"boss".to_owned())
    );
}

/// 源预制体 缓存：命中时不再读盘；文件被改动后必须重载，不能显示过期差异。
#[test]
fn prefab_source_cache_hits_and_invalidates_on_file_change() {
    let (mut app, root) = sample_app_with_production_scene("prefab-cache");
    let target = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        prefab_root(scene, "assets/prefabs/slime_green.prefab")
    };
    // 制造一个 override（改名），让摘要非空。
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        let entity = scene.scene.entities.get_mut(&target).unwrap();
        entity.name = "CachedGreen".to_owned();
        scene.mark_dirty();
    }
    select_entity(&mut app, target);

    let first = app
        .prefab_override_summary(target)
        .expect("prefab instance must produce a summary")
        .expect("summary must load");
    assert!(first.iter().any(|row| row.field == "name"), "{first:?}");
    assert_eq!(app.prefab_source_cache.len(), 1, "首次读取应填充缓存");

    // 第二次读取应命中缓存（内容一致、不重新读盘）。
    let second = app
        .prefab_override_summary(target)
        .expect("summary again")
        .expect("cached summary must load");
    assert_eq!(first, second);
    assert_eq!(app.prefab_source_cache.len(), 1);

    // 外部改动源预制体：mtime/长度变化后必须重新加载并反映新内容。
    let prefab_path = root.join("assets/prefabs/slime_green.prefab");
    let mut prefab = Prefab::load(&prefab_path).unwrap();
    prefab.name = "renamed-by-tool".to_owned();
    prefab.save(&prefab_path).unwrap();
    let third = app
        .prefab_override_summary(target)
        .expect("summary after external edit")
        .expect("reloaded summary must load");
    assert_eq!(app.prefab_source_cache.len(), 1, "同一路径只保留一条缓存");
    // 缓存键是去掉 `\\?\` 前缀后的绝对路径，这里按文件名断言（两种路径写法等价）。
    let (cached_path, cached) = app.prefab_source_cache.iter().next().unwrap();
    assert!(
        cached_path.ends_with("slime_green.prefab"),
        "unexpected cache key: {}",
        cached_path.display()
    );
    assert_eq!(
        cached.prefab.name, "renamed-by-tool",
        "文件改动后缓存必须重载"
    );
    // 差异集合仍然来自源预制体 的真实内容（改名 override 依旧列出）。
    assert!(third.iter().any(|row| row.field == "name"), "{third:?}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn sample_project_reapply_prefab_preserves_overrides() {
    let (mut app, root) = sample_app_with_production_scene("reapply");
    let target = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        prefab_root(scene, "assets/prefabs/slime_green.prefab")
    };
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        let entity = scene.scene.entities.get_mut(&target).unwrap();
        entity.name = "BossGreen".to_owned();
        entity.components.insert(
            "Transform".to_owned(),
            "position=77,19,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
        scene.mark_dirty();
    }
    select_entity(&mut app, target);

    // typed action（面板按钮走的就是这条）：目标显式携带。
    assert_eq!(
        app.dispatch(editor::EditorCommand::ReapplyPrefab { entity: target }),
        editor::CommandResult::Applied
    );
    assert!(
        app.status.contains("已重新应用预制体"),
        "status should confirm prefab reapply"
    );
    let selected = app.selection.primary().unwrap();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    let entity = scene.entities.get(&selected).unwrap();
    assert_eq!(entity.name, "BossGreen");
    // 逐字段语义：被覆盖的字段保留实例取值（原始文本 `77`/`19` 不被规范化），
    // 其余字段跟随源预制体。这里用类型化读取断言，避免绑定源文件的格式细节。
    let payload = entity
        .components
        .get("Transform")
        .expect("Transform")
        .clone();
    let registry = rustlua_ecs::component_schema::builtin_registry();
    let read = |field: &str| {
        let path = rustlua_ecs::property_path::PropertyPath::parse(&format!("Transform.{field}"))
            .expect("path");
        rustlua_ecs::property_path::read_field(&registry, &payload, &path).expect("field")
    };
    assert_eq!(
        read("position"),
        rustlua_ecs::component_schema::FieldValue::Tuple(vec![77.0, 19.0, 0.0])
    );
    assert_eq!(
        read("scale"),
        rustlua_ecs::component_schema::FieldValue::Tuple(vec![1.0, 1.0, 1.0])
    );
    assert!(
        payload.contains("position=77"),
        "覆盖字段保留原始文本：{payload}"
    );
    assert_eq!(
        entity.metadata.get("prefab.source").map(String::as_str),
        Some("assets/prefabs/slime_green.prefab")
    );
    let _ = fs::remove_dir_all(root);
}

/// Revert = 丢弃全部覆盖：实例字段回到源预制体 取值（逐字），名字覆盖同样被清掉。
#[test]
fn sample_project_revert_prefab_overrides_restores_source_values() {
    let (mut app, root) = sample_app_with_production_scene("revert");
    let (target, instance_count) = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let target = prefab_root(scene, "assets/prefabs/slime_green.prefab");
        let count = scene
            .entities
            .values()
            .filter(|entity| {
                entity
                    .metadata
                    .get("prefab.root")
                    .is_some_and(|value| value == "true")
                    && entity
                        .metadata
                        .get("prefab.source")
                        .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
            })
            .count();
        (target, count)
    };
    // 期望值 = 同一 Prefab 在空场景里**全新实例化**的结果：
    // 「Revert Overrides」的语义就是"回到没有覆盖的源状态"。
    let expected = {
        let prefab = rustlua_scene::prefab::Prefab::load(
            crate::util::strip_verbatim_prefix(&app.project.root)
                .join("assets/prefabs/slime_green.prefab"),
        )
        .expect("source prefab loads");
        let mut scratch = Scene::new("scratch");
        let instance = prefab
            .instantiate(&mut scratch, None, &std::collections::BTreeMap::new())
            .expect("fresh instance");
        scratch.entities[&instance.instance_root]
            .components
            .get("Transform")
            .cloned()
            .expect("fresh instance has Transform")
    };
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        let entity = scene.scene.entities.get_mut(&target).unwrap();
        entity.name = "RevertedName".to_owned();
        entity.components.insert(
            "Transform".to_owned(),
            "position=123,456,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
        scene.mark_dirty();
    }
    select_entity(&mut app, target);

    assert_eq!(
        app.dispatch(editor::EditorCommand::RevertPrefabOverrides { entity: target }),
        editor::CommandResult::Applied
    );
    assert!(
        app.status.contains("已还原预制体覆盖"),
        "status should confirm the revert: {}",
        app.status
    );
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    // 生产示例场景里同一个 Prefab 有多个同级实例，且 revert 会重建实例（新实体 ID），
    // 因此这里做**差分**断言：实例数量不变、名字覆盖消失、且恰好出现一个
    // 取值等于"全新实例化"的实例根（即刚被 revert 的那个）。
    let roots: Vec<&rustlua_scene::scene::SceneEntity> = scene
        .entities
        .values()
        .filter(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
                && entity
                    .metadata
                    .get("prefab.source")
                    .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
        })
        .collect();
    assert_eq!(roots.len(), instance_count, "revert 不得增删实例");
    assert!(
        scene
            .entities
            .values()
            .all(|candidate| candidate.name != "RevertedName"),
        "名字覆盖必须被还原"
    );
    let reverted: Vec<&&rustlua_scene::scene::SceneEntity> = roots
        .iter()
        .filter(|entity| entity.components.get("Transform") == Some(&expected))
        .collect();
    assert_eq!(
        reverted.len(),
        1,
        "恰有一个实例根回到源预制体 取值（逐字）：{expected}"
    );
    assert_eq!(
        reverted[0]
            .metadata
            .get("prefab.source")
            .map(String::as_str),
        Some("assets/prefabs/slime_green.prefab"),
        "revert 后必须保留 Prefab 来源元数据"
    );
    let _ = fs::remove_dir_all(root);
}

/// 选择性 Revert（docs/55 阶段 3）：只回退一个字段，其它覆盖必须原样保留；
/// 且回退是**一条** Undo（可整体收回）。
#[test]
fn selective_revert_restores_one_field_and_keeps_other_overrides() {
    let (mut app, root) = sample_app_with_production_scene("selective-revert");
    let target = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        prefab_root(scene, "assets/prefabs/slime_green.prefab")
    };
    // 制造两个独立覆盖：名字 + Transform.x。
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        let entity = scene.scene.entities.get_mut(&target).unwrap();
        entity.name = "SelectiveGreen".to_owned();
        let payload = entity
            .components
            .get("Transform")
            .cloned()
            .expect("instance has Transform");
        let path = rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
        let updated =
            rustlua_ecs::property_path::write_builtin(&payload, &path, "999,0,0").unwrap();
        entity.components.insert("Transform".to_owned(), updated);
        scene.mark_dirty();
    }
    select_entity(&mut app, target);

    // 覆盖列表里应同时出现字段级条目。
    let rows = app
        .prefab_override_summary(target)
        .expect("summary")
        .expect("loads");
    assert!(
        rows.iter()
            .any(|row| row.field == "property:Transform.position"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|row| row.field == "name"), "{rows:?}");

    let undo_before = app.history.undo_len();
    app.revert_prefab_override_field(target, "property:Transform.position");

    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    let entity = scene.entities.get(&target).expect("instance");
    let payload = entity.components.get("Transform").expect("Transform");
    let path = rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
    let registry = rustlua_ecs::component_schema::builtin_registry();
    let source_position = {
        let prefab = Prefab::load(root.join("assets/prefabs/slime_green.prefab")).unwrap();
        let source_id = entity
            .metadata
            .get("prefab.source_id")
            .and_then(|value| value.parse::<u64>().ok())
            .map(SceneEntityId)
            .expect("source id");
        let source_payload = prefab.entities[&source_id]
            .components
            .get("Transform")
            .expect("source Transform");
        rustlua_ecs::property_path::read_field(&registry, source_payload, &path)
            .expect("source position")
    };
    assert_eq!(
        rustlua_ecs::property_path::read_field(&registry, payload, &path).expect("position"),
        source_position,
        "被回退字段必须等于源取值"
    );
    assert_eq!(entity.name, "SelectiveGreen", "其它覆盖（名字）必须保留");
    assert_eq!(
        app.history.undo_len(),
        undo_before + 1,
        "选择性回退必须只产生一条历史"
    );

    // 一次 Undo 收回这次回退。
    app.dispatch(editor::EditorCommand::Undo);
    let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&target]
        .components
        .get("Transform")
        .expect("Transform")
        .clone();
    let position =
        rustlua_ecs::property_path::read_field(&registry, &payload, &path).expect("position");
    assert_eq!(
        position,
        rustlua_ecs::component_schema::FieldValue::Tuple(vec![999.0, 0.0, 0.0]),
        "Undo 后覆盖字段应恢复"
    );
    let _ = fs::remove_dir_all(root);
}

/// 逐字段 Apply（docs/55 阶段 3）：把实例取值写回**源预制体 文件**并更新全部实例；
/// 应用后该字段不再是覆盖，其它覆盖保持不变。
#[test]
fn apply_prefab_override_field_writes_source_and_drops_that_override() {
    let (mut app, root) = sample_app_with_production_scene("selective-apply");
    let target = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        prefab_root(scene, "assets/prefabs/slime_green.prefab")
    };
    // 制造两个覆盖：Transform.position=999,0,0 与名字。
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        let entity = scene.scene.entities.get_mut(&target).unwrap();
        entity.name = "ApplyGreen".to_owned();
        let payload = entity
            .components
            .get("Transform")
            .cloned()
            .expect("instance has Transform");
        let path = rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
        let updated =
            rustlua_ecs::property_path::write_builtin(&payload, &path, "999,0,0").unwrap();
        entity.components.insert("Transform".to_owned(), updated);
        scene.mark_dirty();
    }
    select_entity(&mut app, target);
    let source_id = app.document.current_scene.as_ref().unwrap().scene.entities[&target]
        .metadata
        .get("prefab.source_id")
        .and_then(|value| value.parse::<u64>().ok())
        .map(SceneEntityId)
        .expect("source id");

    let undo_before = app.history.undo_len();
    app.apply_prefab_override_field(target, "property:Transform.position");
    assert!(
        app.status.contains("已把字段应用到源预制体"),
        "status: {}",
        app.status
    );

    // 1) 源预制体 文件里该字段已更新。
    let prefab = Prefab::load(root.join("assets/prefabs/slime_green.prefab"))
        .expect("source prefab reloads");
    let registry = rustlua_ecs::component_schema::builtin_registry();
    let path = rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
    assert_eq!(
        rustlua_ecs::property_path::read_field(
            &registry,
            prefab.entities[&source_id]
                .components
                .get("Transform")
                .expect("source Transform"),
            &path
        )
        .expect("source position"),
        rustlua_ecs::component_schema::FieldValue::Tuple(vec![999.0, 0.0, 0.0]),
        "源预制体 文件必须被更新"
    );

    // 2) 该字段不再是覆盖（实例 == 源），名字覆盖仍在。
    let rows = app
        .prefab_override_summary(app.selection.primary().unwrap())
        .expect("summary")
        .expect("loads");
    assert!(
        !rows
            .iter()
            .any(|row| row.field == "property:Transform.position"),
        "应用后该字段不应再是覆盖：{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.field == "name"),
        "其它覆盖必须保留：{rows:?}"
    );

    // 3) 场景更新是**一条**历史（文件写入本身不属于文档历史，这点与整体写回一致）。
    assert_eq!(app.history.undo_len(), undo_before + 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn sample_project_save_prefab_to_source_updates_all_instances() {
    let (mut app, root) = sample_app_with_production_scene("save-source");
    let (target, original_names) = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let target = prefab_root(scene, "assets/prefabs/slime_green.prefab");
        let names = scene
            .entities
            .values()
            .filter(|entity| {
                entity
                    .metadata
                    .get("prefab.root")
                    .is_some_and(|value| value == "true")
                    && entity
                        .metadata
                        .get("prefab.source")
                        .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
            })
            .map(|entity| entity.name.clone())
            .collect::<Vec<_>>();
        (target, names)
    };
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        let entity = scene.scene.entities.get_mut(&target).unwrap();
        entity.name = "SlimeGreenElite".to_owned();
        scene.mark_dirty();
    }
    select_entity(&mut app, target);

    assert_eq!(
        app.dispatch(editor::EditorCommand::SavePrefabToSource { entity: target }),
        editor::CommandResult::Applied
    );
    assert!(
        app.status.contains("已写回预制体源并更新实例"),
        "status should confirm prefab save, got: {}",
        app.status
    );
    let saved = Prefab::load(root.join("assets/prefabs/slime_green.prefab")).unwrap();
    assert_eq!(saved.entities[&saved.root].name, "SlimeGreenElite");
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    let names: Vec<_> = scene
        .entities
        .values()
        .filter(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
                && entity
                    .metadata
                    .get("prefab.source")
                    .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
        })
        .map(|entity| entity.name.as_str())
        .collect();
    assert!(
        !names.is_empty(),
        "sample scene should still contain green slimes"
    );
    assert!(
        names.contains(&"SlimeGreenElite"),
        "selected instance should rebuild from updated source"
    );
    assert!(
        original_names
            .iter()
            .filter(|name| name.as_str() != "SlimeGreen1")
            .all(|name| names.contains(&name.as_str())),
        "other instance name overrides should be preserved"
    );
    let _ = fs::remove_dir_all(root);
}

/// `Animation` 面板（仍在缓冲里的组件）的编辑要能在保存 / 重开后**逐字段保留**。
///
/// 实体与剪辑名都不写死：从场景里取第一个带 `Animation` 的实体与它的第一个剪辑名，
/// 只改"帧率 + 是否循环"这两个数值字段，再比对重开后的**数值**。
#[test]
fn sample_project_animation_edit_persists_after_save_and_reopen() {
    let (mut app, root) = sample_app_with_main_scene("animation-edit");
    let (id, clip_name) = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene
            .entities
            .values()
            .find(|entity| entity.components.contains_key("Animation"))
            .expect("示例 main.scene 应当有带 Animation 的实体");
        let machine = AnimationStateMachine::from_component_str(&entity.components["Animation"])
            .expect("Animation 组件串应当可解析");
        let clip = machine.clips.first().expect("动画实体应当至少有一个 clip");
        (entity.id, clip.name.clone())
    };
    select_entity(&mut app, id);
    app.animation.machine.default = clip_name.clone();
    let clip = app
        .animation
        .machine
        .clips
        .iter_mut()
        .find(|clip| clip.name == clip_name)
        .expect("选中的剪辑应当还在面板缓冲里");
    clip.frame_rate = 12.0;
    clip.looped = false;

    app.apply_inspector();
    app.save_scene();
    app.open_scene_resource(PathBuf::from("scenes/main.scene"));

    let reopened = app.document.current_scene.as_ref().unwrap();
    let animation = AnimationStateMachine::from_component_str(
        &reopened.scene.entities[&id].components["Animation"],
    )
    .unwrap();
    assert_eq!(animation.default, clip_name);
    let clip = animation
        .clips
        .iter()
        .find(|clip| clip.name == clip_name)
        .expect("重开后应当保留同一个剪辑");
    assert_eq!(clip.frame_rate, 12.0);
    assert!(!clip.looped);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn sample_project_ui_adjustments_persist_after_save_and_reopen() {
    let (mut app, root) = sample_app_with_menu_scene("ui-adjust");
    select_entity(&mut app, SceneEntityId(3));
    // `Text` 已迁到 schema 驱动的通用字段编辑器（第 25 轮）：这里按**生产路径**
    // （typed 命令）提交字段，`UiLayout` 仍走它自己的缓冲面板。
    for (field, value) in [("text", "Production Sample Title"), ("font_size", "33.6")] {
        assert_eq!(
            app.dispatch(editor::EditorCommand::SetComponentField {
                entity: SceneEntityId(3),
                component: "Text".to_owned(),
                field: field.to_owned(),
                value: value.to_owned(),
            }),
            editor::CommandResult::Applied,
            "Text.{field} 必须能通过 typed 命令提交"
        );
    }
    // `UiLayout` 也已迁到 schema 字段编辑器（第 31 轮）：4 个 anchors + 4 个 offsets
    // 走 typed 命令，`apply_inspector` 不再参与（它只负责剩下的缓冲面板）。
    assert_eq!(
        app.dispatch(editor::EditorCommand::SetComponentFields {
            entity: SceneEntityId(3),
            component: "UiLayout".to_owned(),
            fields: vec![
                ("anchors".to_owned(), "1,0,1,0".to_owned()),
                ("offsets".to_owned(), "-48,24,-48,24".to_owned()),
            ],
        }),
        editor::CommandResult::Applied,
        "UiLayout 的锚点/偏移必须能通过 typed 命令提交"
    );

    app.save_scene();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    let reopened = app.document.current_scene.as_ref().unwrap();
    let title =
        UiText::from_component_str(&reopened.scene.entities[&SceneEntityId(3)].components["Text"])
            .unwrap();
    assert_eq!(title.text, "Production Sample Title");
    assert_eq!(title.font_size, 33.6);
    let layout = rustlua_ui::ui_layout::UiRectLayout::from_component_str(
        &reopened.scene.entities[&SceneEntityId(3)].components["UiLayout"],
    )
    .unwrap();
    assert_eq!(layout.anchors, [1.0, 0.0, 1.0, 0.0]);
    assert_eq!(layout.offsets, [-48.0, 24.0, -48.0, 24.0]);
    let _ = fs::remove_dir_all(root);
}
