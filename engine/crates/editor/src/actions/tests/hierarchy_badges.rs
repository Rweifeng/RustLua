//! 层级行徽标：与 Inspector 同一条覆盖 diff、按修订缓存、预制体文档只读记录不碰磁盘。
//!
//! 三条门对应三件容易走偏的事：
//! * 场景侧的"覆盖 N"必须与 Inspector 的整实例列表逐节点对得上（不能各算一份）；
//! * 缓存键漏掉修订 → 编辑后数字不动；多算代际 → 每帧重建（这里用重建次数钉住两边）；
//! * 预制体文档的嵌套/覆盖来自文件级记录，**不该**为此去读任何源预制体。

use super::*;
use crate::panels::hierarchy_badges::BadgeOrigin;
use eframe::egui;
use rustlua_scene::prefab::{PREFAB_SOURCE_META_KEY, PREFAB_VARIANT_BASE_META_KEY};
use std::collections::BTreeMap;

fn instance_root_ids(app: &EditorApp) -> Vec<SceneEntityId> {
    app.document
        .current_scene
        .as_ref()
        .expect("已打开场景")
        .scene
        .entities
        .values()
        .filter(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
        })
        .map(|entity| entity.id)
        .collect()
}

fn source_of(app: &EditorApp, entity: SceneEntityId) -> String {
    app.document
        .current_scene
        .as_ref()
        .expect("已打开场景")
        .scene
        .entities[&entity]
        .metadata
        .get(PREFAB_SOURCE_META_KEY)
        .expect("实例根带来源")
        .clone()
}

/// 场景里的"覆盖 N"与 Inspector 的整实例列表是同一个数：按节点逐一对拍。
#[test]
fn scene_badge_counts_match_the_inspector_instance_list() {
    let (mut app, root) = open_sample_production("hierarchy-badges-parity");
    app.ensure_prefab_badges();
    let roots = instance_root_ids(&app);
    assert!(!roots.is_empty(), "示例场景应带预制体实例");
    let mut compared = 0;
    for root_id in roots {
        let Some(Ok(found)) = app.prefab_instance_overrides(root_id) else {
            continue;
        };
        let mut expected: BTreeMap<SceneEntityId, usize> = BTreeMap::new();
        for row in &found.rows {
            *expected.entry(row.entity).or_default() += 1;
        }
        let origin = BadgeOrigin::InstanceSource(source_of(&app, root_id));
        for (entity, count) in expected {
            let badges = app
                .prefab_badges
                .get(entity)
                .unwrap_or_else(|| panic!("实体 {} 有覆盖行却没有徽标", entity.0));
            assert_eq!(
                badges.overrides.get(&origin),
                Some(&count),
                "实体 {} 的覆盖条数必须与 Inspector 列表一致",
                entity.0
            );
        }
        compared += 1;
    }
    assert!(compared > 0, "至少要有一个可对拍的实例");
    let _ = fs::remove_dir_all(root);
}

/// 顶层实例不是嵌套：场景里没有"实例套实例"时不该出现嵌套徽标。
#[test]
fn top_level_scene_instances_are_not_badged_as_nested() {
    let (mut app, root) = open_sample_production("hierarchy-badges-not-nested");
    app.ensure_prefab_badges();
    for id in instance_root_ids(&app) {
        if let Some(badges) = app.prefab_badges.get(id) {
            assert!(badges.nested.is_none(), "顶层实例 {} 不该带嵌套徽标", id.0);
        }
    }
    let _ = fs::remove_dir_all(root);
}

/// 缓存键：文档没变不重建；修订一变必须重建（否则编辑后数字不动）。
#[test]
fn badge_index_rebuilds_only_when_the_document_changes() {
    let (mut app, root) = open_sample_production("hierarchy-badges-cache");
    app.ensure_prefab_badges();
    let builds = app.prefab_badges.builds();
    app.ensure_prefab_badges();
    assert_eq!(
        app.prefab_badges.builds(),
        builds,
        "文档没变时第二次调用必须命中缓存"
    );
    app.document
        .current_scene
        .as_mut()
        .expect("已打开场景")
        .mark_dirty();
    app.ensure_prefab_badges();
    assert_eq!(
        app.prefab_badges.builds(),
        builds + 1,
        "修订变化后必须重建一次"
    );
    let _ = fs::remove_dir_all(root);
}

/// 预制体文档：嵌套与覆盖都来自文件级记录，不需要（也不该）读源。
fn prefab_document_app(label: &str) -> (EditorApp, std::path::PathBuf) {
    let (mut app, root) = open_sample_production(label);
    app.prefab_source_cache.clear();
    let mut document = editor::SceneDocument::new_scene("outer");
    document.path = Some(app.project.root.join("assets/prefabs/outer.prefab"));
    document
        .scene
        .add_entity(SceneEntityId(1), "Outer", None)
        .unwrap();
    document
        .scene
        .add_entity(SceneEntityId(2), "Inner", Some(SceneEntityId(1)))
        .unwrap();
    document
        .scene
        .add_entity(SceneEntityId(3), "Plain", Some(SceneEntityId(1)))
        .unwrap();
    let inner = document.scene.entities.get_mut(&SceneEntityId(2)).unwrap();
    inner.metadata.insert(
        PREFAB_SOURCE_META_KEY.to_owned(),
        "assets/inner.prefab".to_owned(),
    );
    inner
        .metadata
        .insert("prefab.root".to_owned(), "true".to_owned());
    document.scene.metadata.insert(
        PREFAB_VARIANT_BASE_META_KEY.to_owned(),
        "assets/base.prefab".to_owned(),
    );
    for (key, value) in [
        ("prefab.override.2.name", "MyInner"),
        ("prefab.override.2.property:Sprite.tint", "1,0,0,1"),
        ("prefab.nested_override.2.property:Sprite.order", "5"),
    ] {
        document
            .scene
            .metadata
            .insert(key.to_owned(), value.to_owned());
    }
    app.document.current_scene = Some(document);
    (app, root)
}

#[test]
fn prefab_document_badges_come_from_records_without_reading_sources() {
    let (mut app, root) = prefab_document_app("hierarchy-badges-prefab");
    app.ensure_prefab_badges();
    let badges = app
        .prefab_badges
        .get(SceneEntityId(2))
        .expect("嵌套节点应有徽标");
    assert_eq!(badges.nested.as_deref(), Some("assets/inner.prefab"));
    assert_eq!(badges.override_total(), 3, "变体记录 2 + 内层记录 1");
    assert_eq!(badges.overrides[&BadgeOrigin::VariantBase], 2);
    assert_eq!(badges.overrides[&BadgeOrigin::NestedInner], 1);
    assert!(
        app.prefab_badges.get(SceneEntityId(1)).is_none(),
        "没有记录的普通节点不标"
    );
    assert!(
        app.prefab_source_cache.is_empty(),
        "预制体文档的徽标不该去读任何源预制体（这里的 assets/inner.prefab 也不存在）"
    );
    let _ = fs::remove_dir_all(root);
}

/// 无头画一帧层级面板，收集这一帧画出来的所有文本。
fn draw_hierarchy_text(app: &mut EditorApp) -> String {
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            app.draw_hierarchy_body(ui);
        });
    });
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text_shape) => Some(text_shape.galley.text().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 徽标真的画在了行里（不只是索引里有数）：预制体文档两种徽标都要出现。
#[test]
fn hierarchy_rows_draw_both_badges() {
    let (mut app, root) = prefab_document_app("hierarchy-badges-draw");
    let text = draw_hierarchy_text(&mut app);
    assert!(text.contains("[嵌套]"), "嵌套徽标没画出来：\n{text}");
    assert!(
        text.contains("[覆盖 3]"),
        "覆盖徽标没画出来（期望 3 条记录）：\n{text}"
    );
    let _ = fs::remove_dir_all(root);
}

/// 场景里普通行不挂徽标：只有真有覆盖的实例节点才出现。
#[test]
fn plain_scene_rows_carry_no_badges() {
    let (mut app, root) = open_sample_production("hierarchy-badges-plain");
    let text = draw_hierarchy_text(&mut app);
    assert!(
        !text.contains("[嵌套]"),
        "示例场景没有实例套实例，不该出现嵌套徽标：\n{text}"
    );
    assert!(
        text.contains("[覆盖 "),
        "示例场景的实例出厂就带覆盖，应至少画出一处覆盖徽标：\n{text}"
    );
    let _ = fs::remove_dir_all(root);
}
