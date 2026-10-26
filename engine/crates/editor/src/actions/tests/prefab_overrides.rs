//! M79 P2：预制体实例**整棵子树**的覆盖聚合、就地逐项还原与那份列表的缓存。
//!
//! 三条主线各一道门：
//! * `collect_instance_overrides` 是纯函数——跨节点、带路径、结构性差异只能 note；
//! * `RevertAllPrefabFieldOverrides` 走网关——**一条** Undo、id 不重排、结构性差异原样留下，
//!   并且在历史里是**补丁形态**（与 docs/25 §24 的字段级通道接上）；
//! * 缓存键：内容/历史签名/源文件任一变化都必须让列表失效，否则显示的就是别的实例的过去。
//!
//! 跨节点的层级用合成夹具 `tower_instance()`（示例工程里的 slime 实例都是**单实体**，
//! 拿它测"深层子节点的覆盖"等于没测）；需要真实工程、真实历史与真实网关的那两条用
//! `open_sample_production`（`scenes/api_prefab.scene`，它的 `SlimeGreen1` 出厂就带
//! name + Transform.position 两项覆盖，因此断言按"基线 + 新加的那项"来写）。

use super::*;
use crate::actions::prefab_overrides::{InstanceOverrideNote, collect_instance_overrides};
use editor::EditorCommand;
use rustlua_assets::resource_db::AssetId;
use rustlua_scene::scene::Scene;

/// 造一张三节点的源预制体（`Tower/Body/Head`）并实例化进一份新场景。
///
/// 返回 `(场景, 源预制体, 源路径)`；实例侧的 id 从 `prefab.instances` 映射里取。
fn tower_instance() -> (Scene, Prefab, String) {
    let mut source = Scene::new("prefab");
    source.add_entity(SceneEntityId(1), "Tower", None).unwrap();
    source
        .add_entity(SceneEntityId(2), "Body", Some(SceneEntityId(1)))
        .unwrap();
    source
        .add_entity(SceneEntityId(3), "Head", Some(SceneEntityId(2)))
        .unwrap();
    for id in [SceneEntityId(1), SceneEntityId(2), SceneEntityId(3)] {
        source.entities.get_mut(&id).unwrap().components.insert(
            "Transform".to_owned(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
    }
    source
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("Sprite".to_owned(), "texture=a.png;size=32x32".to_owned());
    let prefab = Prefab::from_scene(
        &source,
        SceneEntityId(1),
        AssetId::from_seed("tower.prefab"),
        "Tower",
    )
    .unwrap();
    let mut scene = Scene::new("level");
    let instance = prefab
        .instantiate(&mut scene, None, &std::collections::BTreeMap::new())
        .unwrap();
    apply_prefab_instance_metadata(&mut scene, "assets/prefabs/tower.prefab", &instance);
    (scene, prefab, "assets/prefabs/tower.prefab".to_owned())
}

/// 实例根（合成夹具里只有一个，直接取带 `prefab.root=true` 的那个）。
fn fixture_root(scene: &Scene) -> SceneEntityId {
    scene
        .entities
        .values()
        .filter(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
        })
        .map(|entity| entity.id)
        .min()
        .expect("夹具里应有实例根")
}

fn source_id_of(scene: &Scene, instance_entity: SceneEntityId) -> SceneEntityId {
    SceneEntityId(
        scene
            .entities
            .get(&instance_entity)
            .unwrap()
            .metadata
            .get("prefab.source_id")
            .unwrap()
            .parse::<u64>()
            .unwrap(),
    )
}

/// 按**数值**读一条组件串里的某个字段（`field=a,b,c`），与小数位数无关。
///
/// 逐字段还原经由 `write_builtin` 重排文本（`0.600` → `0.000000` 那一族），所以断言不能钉文本；
/// "是不是覆盖"由 `rustlua_ecs::property_path::field_text_equal` 判，这里用同一口径复核取值。
fn field_numbers(payload: &str, name: &str) -> Vec<f32> {
    payload
        .split(';')
        .filter_map(|part| part.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| {
            value
                .split(',')
                .filter_map(|item| item.trim().parse::<f32>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn tint_is(payload: &str, expected: [f32; 4]) -> bool {
    field_numbers(payload, "tint") == expected.to_vec()
}

fn position_is(payload: &str, expected: [f32; 3]) -> bool {
    field_numbers(payload, "position") == expected.to_vec()
}

/// 实例里的**任意一个**节点都能被还原，所以覆盖列表按 `prefab.source_id` 反查源实体、
/// 并按父子顺序给出路径（这条是"整实例视图"的全部卖点：不用逐个点开深层子节点）。
#[test]
fn the_aggregate_list_walks_the_whole_subtree_with_readable_paths() {
    let (mut scene, prefab, source) = tower_instance();
    let root = fixture_root(&scene);
    let body = scene.entities[&root].children[0];
    let head = scene.entities[&body].children[0];

    // 三个不同深度各改一处：根改名、中间层加组件、最深的子节点改 Transform 字段。
    scene.entities.get_mut(&root).unwrap().name = "TowerPrime".to_owned();
    scene
        .entities
        .get_mut(&body)
        .unwrap()
        .components
        .insert("Quad".to_owned(), "size=8,8".to_owned());
    scene.entities.get_mut(&head).unwrap().components.insert(
        "Transform".to_owned(),
        "position=5,6,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
    );

    let found = collect_instance_overrides(&scene, root, &source, &prefab).unwrap();
    let by_label = found
        .rows
        .iter()
        .map(|row| (row.entity_label.as_str(), row.field.as_str()))
        .collect::<Vec<_>>();
    assert!(
        by_label.contains(&("TowerPrime", "name")),
        "根节点改名要出现在列表里：{by_label:?}"
    );
    assert!(
        by_label.contains(&("TowerPrime/Body", "component:Quad")),
        "中间层新增组件要按路径列出：{by_label:?}"
    );
    assert!(
        by_label.contains(&("TowerPrime/Body/Head", "property:Transform.position")),
        "深层子节点的字段级覆盖要能定位到节点：{by_label:?}"
    );
    assert_eq!(found.entity_count(), 3, "三项覆盖分布在三个节点上");
    assert!(
        found.notes.is_empty(),
        "结构一致时不该有 note：{:?}",
        found.notes
    );

    // 逐条还原用的键（实体 + 字段）必须能对上源实体：`source` 是按 `prefab.source_id` 反查的。
    let head_row = found
        .rows
        .iter()
        .find(|row| row.entity == head)
        .expect("Head 的覆盖行");
    assert_eq!(source_id_of(&scene, head), head_row.source);
    // 两侧取值是**字段级**文本（`property:` 只取那一个字段），不是整条组件串。
    assert_eq!(head_row.source_value, "0,0,0");
    assert_eq!(head_row.instance_value, "5,6,0");
}

/// "还原不了"必须被说出来，否则列表里没有会被读成"没有覆盖"。
#[test]
fn nodes_the_source_cannot_answer_for_are_notes_not_rows() {
    let (mut scene, prefab, source) = tower_instance();
    let root = fixture_root(&scene);
    // 手工加一个子节点：没有 `prefab.source_id`，源里也没有它 ⇒ SourceEntityMissing。
    scene
        .add_entity(SceneEntityId(900), "HandWelded", Some(root))
        .unwrap();
    // 再挂一个带**别的**预制体来源的节点（嵌套后置）：ForeignPrefabNode，且子树不展开。
    scene
        .add_entity(SceneEntityId(901), "Nested", Some(root))
        .unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(901))
        .unwrap()
        .metadata
        .insert(
            "prefab.source".to_owned(),
            "assets/prefabs/other.prefab".to_owned(),
        );

    let found = collect_instance_overrides(&scene, root, &source, &prefab).unwrap();
    assert!(
        found.rows.is_empty(),
        "两处都不是字段级差异：{:?}",
        found.rows
    );
    let notes = found
        .notes
        .iter()
        .map(|note| match note {
            InstanceOverrideNote::SourceEntityMissing { label, .. } => label.clone(),
            InstanceOverrideNote::ForeignPrefabNode { label, .. } => format!("foreign:{label}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        notes,
        vec![
            "Tower/HandWelded".to_owned(),
            "foreign:Tower/Nested".to_owned()
        ],
        "note 也用与 rows 同一条从实例根数下来的路径：{notes:?}"
    );
}

/// 整实例逐项还原：**一条** Undo、实体 id 一个不换、历史里是补丁形态（§24 的字段级通道）。
#[test]
fn reverting_the_whole_instance_is_one_undo_and_keeps_entity_ids() {
    let (mut app, root_dir) = open_sample_production("override-revert-all");
    let target = sample_instance_root(&app, "assets/prefabs/slime_green.prefab");
    let ids_before = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        scene
            .entities
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
    };
    // 出厂基线：这个实例已经带 name + property:Transform.position 两项覆盖。
    let baseline = app
        .prefab_instance_overrides(target)
        .expect("slime 实例在预制体里")
        .expect("源预制体可加载");
    let mut baseline_fields = baseline
        .rows
        .iter()
        .map(|row| row.field.clone())
        .collect::<Vec<_>>();
    baseline_fields.sort();
    assert_eq!(
        baseline_fields,
        vec!["name".to_owned(), "property:Transform.position".to_owned()],
        "示例场景的实例基线覆盖变了，下面按数量做的断言要跟着改：{baseline_fields:?}"
    );

    // 再加一项：改名（覆盖仍然只有 name 这一项）+ Sprite 的 tint 字段。
    {
        let doc = app.document.current_scene.as_mut().unwrap();
        let entity = doc.scene.entities.get_mut(&target).unwrap();
        entity.name = "BossGreen".to_owned();
        let payload = entity.components.get("Sprite").cloned().unwrap();
        entity.components.insert(
            "Sprite".to_owned(),
            payload.replace(
                "tint=0.600,1.000,0.600,1.000",
                "tint=1.000,0.000,0.000,1.000",
            ),
        );
        doc.mark_dirty();
    }
    let found = app.prefab_instance_overrides(target).unwrap().unwrap();
    assert_eq!(found.rows.len(), 3, "{:?}", found.rows);

    let (patch_before, snapshot_before) = app.history.storage_counts();
    let undo_before = app.history.undo_len();
    app.dispatch(EditorCommand::RevertAllPrefabFieldOverrides { entity: target });
    assert_eq!(
        app.history.undo_len(),
        undo_before + 1,
        "整实例逐项还原必须是一条历史"
    );
    // §24：全是字段级改动 ⇒ 补丁形态，一条都不该退化成整文档快照。
    assert_eq!(
        (
            app.history.storage_counts().0 - patch_before,
            app.history.storage_counts().1 - snapshot_before
        ),
        (1, 0),
        "逐项还原必须走补丁通道：{:?}",
        app.history.storage_counts()
    );
    {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene.entities.get(&target).unwrap();
        assert_eq!(entity.name, "SlimeGreen", "名字覆盖要还原");
        // 数值口径，不是文本口径：逐字段还原走 `write_builtin`，它按自己的位数重排
        // （`0.600` 存成 `0.600000`）。"是不是覆盖"因此由 `field_text_equal` 判，见下面 rows 为空。
        let sprite = entity.components.get("Sprite").unwrap().clone();
        assert!(
            !sprite.contains("1.000,0.000,0.000") && tint_is(&sprite, [0.6, 1.0, 0.6, 1.0]),
            "tint 覆盖要还原：{sprite}"
        );
        let transform = entity.components.get("Transform").unwrap().clone();
        assert!(
            position_is(&transform, [0.0, 0.0, 0.0]),
            "出厂就有的 position 覆盖同样要还原：{transform}"
        );
        let ids_after = scene
            .entities
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids_before, ids_after, "逐项还原不许重排实体 id");
    }
    assert!(
        app.status.contains("已逐项还原 3 个字段覆盖"),
        "状态栏要报数：{}",
        app.status
    );
    let after = app.prefab_instance_overrides(target).unwrap().unwrap();
    assert!(
        after.rows.is_empty(),
        "还原后不该再有覆盖（格式不同的同一个值不算覆盖）：{:?}",
        after.rows
    );

    // 一条 Undo 就该把三处都放回去（撤销的是那一条补丁，不是整份文档）。
    app.dispatch(EditorCommand::Undo);
    {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene.entities.get(&target).unwrap();
        assert_eq!(entity.name, "BossGreen", "一条 Undo 要同时收回三处覆盖");
        assert!(
            entity
                .components
                .get("Sprite")
                .unwrap()
                .contains("1.000,0.000,0.000")
        );
        let transform = entity.components.get("Transform").unwrap().clone();
        assert!(
            position_is(&transform, [224.0, 128.0, 0.0]),
            "position 也得跟着回来：{transform}"
        );
    }
    let _ = fs::remove_dir_all(root_dir);
}

/// 结构性差异不归这条命令管：手加的节点必须原样留着，并在状态里说清楚还剩几项。
#[test]
fn the_in_place_revert_leaves_structural_differences_alone() {
    let (mut app, root_dir) = open_sample_production("override-revert-structural");
    let target = sample_instance_root(&app, "assets/prefabs/slime_green.prefab");
    {
        let doc = app.document.current_scene.as_mut().unwrap();
        // 手工挂一个源里没有的子节点（id 取一个场景里必然没用的大数，避免依赖内部计数器）。
        doc.scene
            .add_entity(SceneEntityId(40040), "Extra", Some(target))
            .unwrap();
        doc.scene.entities.get_mut(&target).unwrap().name = "BossGreen".to_owned();
        doc.mark_dirty();
    }
    app.dispatch(EditorCommand::RevertAllPrefabFieldOverrides { entity: target });

    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert_eq!(
        scene.entities.get(&target).unwrap().name,
        "SlimeGreen",
        "字段级覆盖要还原"
    );
    assert!(
        scene
            .entities
            .values()
            .any(|entity| entity.name == "Extra" && entity.parent == Some(target)),
        "源里没有对应实体的子节点不许被删掉"
    );
    assert!(
        app.status.contains("结构性差异"),
        "状态栏要认领剩下的那项：{}",
        app.status
    );
    let _ = fs::remove_dir_all(root_dir);
}

/// 缓存键的三件事：内容变、历史签名变、**源文件被外部改动**变。
#[test]
fn the_override_list_cache_invalidates_on_content_history_and_source_file() {
    let (mut app, root_dir) = open_sample_production("override-cache");
    let target = sample_instance_root(&app, "assets/prefabs/slime_green.prefab");
    {
        let doc = app.document.current_scene.as_mut().unwrap();
        doc.scene.entities.get_mut(&target).unwrap().name = "BossGreen".to_owned();
        doc.mark_dirty();
    }

    let first = app.prefab_instance_overrides(target).unwrap().unwrap();
    // 基线（name + position）+ 没有新项：改名只是换了 name 这一项的取值。
    assert_eq!(first.rows.len(), 2, "{:?}", first.rows);
    let key_after_first = app
        .prefab_override_cache
        .as_ref()
        .expect("首次应填缓存")
        .key;
    // 同键 ⇒ 命中缓存（每帧的成本是一次 stat + 几次整数比较）。
    let again = app.prefab_instance_overrides(target).unwrap().unwrap();
    assert_eq!(first, again);
    assert_eq!(
        app.prefab_override_cache.as_ref().unwrap().key,
        key_after_first,
        "无变化时不该换键"
    );
    assert_eq!(key_after_first.generation, app.prefab_source_generation);

    // 1) 内容变：走一次真实编辑（网关会 bump revision 并作废缓存）。
    app.dispatch(EditorCommand::RevertPrefabOverride {
        entity: target,
        field: "name".to_owned(),
    });
    let after_edit = app.prefab_instance_overrides(target).unwrap().unwrap();
    assert_eq!(
        after_edit.rows.len(),
        1,
        "还原 name 后只剩 position 那项：{:?}",
        after_edit.rows
    );
    assert_ne!(
        app.prefab_override_cache.as_ref().unwrap().key,
        key_after_first,
        "缓存键必须跟着内容/历史签名变"
    );

    // 2) 源文件被外部改动（文档一个字没再改）：源重载次数进键，列表必须跟着变。
    //    这一条同时钉住调用顺序：**先查磁盘再比键**，否则旧缓存永远命中、外部改动永不被发现。
    let key_before_external = app.prefab_override_cache.as_ref().unwrap().key;
    let prefab_path = root_dir.join("assets/prefabs/slime_green.prefab");
    let mut source = Prefab::load(&prefab_path).unwrap();
    source.entities.get_mut(&SceneEntityId(1)).unwrap().name = "RenamedByTool".to_owned();
    source.save(&prefab_path).unwrap();
    let after_external = app.prefab_instance_overrides(target).unwrap().unwrap();
    assert_eq!(
        app.prefab_source_generation,
        key_before_external.generation + 1,
        "磁盘上的源变了就必须计入代际"
    );
    assert_eq!(
        after_external
            .rows
            .iter()
            .find(|row| row.field == "name")
            .expect("源改名后实例就成了覆盖")
            .source_value,
        "RenamedByTool"
    );
    let _ = fs::remove_dir_all(root_dir);
}

/// 从文档里找某张预制体的实例根（示例工程里同一张源有多个实例，这里取最小的那个 id，
/// 保证测试可复现且不依赖场景里的实体顺序）。
fn sample_instance_root(app: &EditorApp, prefab_source: &str) -> SceneEntityId {
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    scene
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
                    .is_some_and(|value| value == prefab_source)
        })
        .map(|entity| entity.id)
        .min()
        .unwrap_or_else(|| panic!("示例工程里没有 {prefab_source} 的实例"))
}
