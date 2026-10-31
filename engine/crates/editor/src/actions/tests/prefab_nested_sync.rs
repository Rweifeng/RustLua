//! 嵌套预制体的内层 → 外层同步：写回记录、显式同步、结构拒绝、文档保存。
//!
//! 夹具是**真实**结构：示例工程的 `slime_green.prefab` 作内层，外层用引擎自己的
//! `instantiate` 挂一个实例再落盘（手写 TSV 只能测到编出来的形状）。
//!
//! 守住的语义：
//! * 写回外层时，外层相对内层的字段差异落成 `prefab.nested_override.*`，并记下内层签名；
//! * 内层在上次同步后动过，写回必须被拒绝（否则内层新值被钉成外层覆盖）；
//! * 显式同步 = 新内层 + 旧记录：新字段跟着内层走，外层覆盖保留；
//! * v1 只做字段级：内层加节点即拒绝，外层文件一个字节不动。

use super::*;
use crate::menu::MenuGate;
use editor::FileSignature;
use rustlua_assets::resource_db::AssetId;
use rustlua_render::sprite::SpriteComponent;
use rustlua_scene::prefab::{PREFAB_NESTED_STAMP_META_PREFIX, Prefab};
use rustlua_scene::scene::Scene;
use std::collections::BTreeMap;

const GREEN: &str = "assets/prefabs/slime_green.prefab";
const NESTED: &str = "assets/prefabs/nested_slime.prefab";
const OUTER_ROOT_FILE_ID: SceneEntityId = SceneEntityId(2);

/// 外层预制体：根 `NestedSlime` 下挂一个真实的 `slime_green` 实例，落盘后返回内存那份。
fn write_nested_green_prefab(project_root: &Path) -> Prefab {
    let inner = Prefab::load(project_root.join(GREEN)).expect("内层源可加载");
    let mut holder = Scene::new("NestedSlime");
    holder
        .add_entity(SceneEntityId(1), "NestedSlime", None)
        .unwrap();
    let staged = inner
        .instantiate(&mut holder, Some(SceneEntityId(1)), &BTreeMap::new())
        .unwrap();
    apply_prefab_instance_metadata(&mut holder, GREEN, &staged);
    let nested = Prefab::from_scene(
        &holder,
        SceneEntityId(1),
        AssetId::from_seed("nested_slime"),
        "NestedSlime",
    )
    .expect("外层预制体可创建");
    nested
        .save(project_root.join(NESTED))
        .expect("外层预制体可落盘");
    nested
}

/// 在当前场景里实例化外层预制体，返回 `(外层实例根, 内层节点)`。
fn place_nested_instance(app: &mut EditorApp, nested: &Prefab) -> (SceneEntityId, SceneEntityId) {
    let scene = app.document.current_scene.as_mut().expect("已打开场景");
    let instance = nested
        .instantiate(&mut scene.scene, None, &BTreeMap::new())
        .expect("嵌套预制体可实例化");
    apply_prefab_instance_metadata(&mut scene.scene, NESTED, &instance);
    let outer_root = instance.instance_root;
    let inner_root = scene.scene.entities[&outer_root].children[0];
    (outer_root, inner_root)
}

fn sprite_of(app: &EditorApp, entity: SceneEntityId) -> SpriteComponent {
    let payload = &app
        .document
        .current_scene
        .as_ref()
        .expect("已打开场景")
        .scene
        .entities[&entity]
        .components["Sprite"];
    SpriteComponent::from_component_str(payload).expect("场景里的 Sprite 可解析")
}

fn set_scene_sprite(app: &mut EditorApp, entity: SceneEntityId, sprite: SpriteComponent) {
    let scene = &mut app
        .document
        .current_scene
        .as_mut()
        .expect("已打开场景")
        .scene;
    scene
        .entities
        .get_mut(&entity)
        .expect("目标实体存在")
        .components
        .insert("Sprite".to_owned(), sprite.to_component_str());
}

/// 直接改写磁盘上的内层源（模拟"别人改了公共资产"）。
fn rewrite_inner_prefab(project_root: &Path, edit: impl FnOnce(&mut SpriteComponent)) {
    let path = project_root.join(GREEN);
    let mut inner = Prefab::load(&path).unwrap();
    let entity = inner.entities.get_mut(&SceneEntityId(1)).unwrap();
    let mut sprite =
        SpriteComponent::from_component_str(&entity.components["Sprite"]).expect("内层 Sprite");
    edit(&mut sprite);
    entity
        .components
        .insert("Sprite".to_owned(), sprite.to_component_str());
    inner.save(path).unwrap();
}

/// 外层写回：外层相对内层的字段差异写成记录，并跟上内层签名。
#[test]
fn outer_write_back_records_nested_overrides_and_stamps_the_inner() {
    let (mut app, root) = open_sample_production("nested-record");
    let nested = write_nested_green_prefab(&root);
    let (outer_root, inner_root) = place_nested_instance(&mut app, &nested);

    // 只改 order：外层自己选择了 5，tint 等字段仍等于内层。
    let mut sprite = sprite_of(&app, inner_root);
    sprite.sorting.order = 5;
    set_scene_sprite(&mut app, inner_root, sprite);

    app.save_prefab_to_source(outer_root);
    assert!(app.status.contains("已写回"), "{}", app.status);

    let saved = Prefab::load(root.join(NESTED)).unwrap();
    assert_eq!(
        saved.nested_overrides().unwrap(),
        BTreeMap::from([(
            (OUTER_ROOT_FILE_ID, "property:Sprite.order".to_owned()),
            "5".to_owned()
        )]),
        "只有真正变了的字段建记录"
    );
    assert_eq!(
        saved.nested_stamp(GREEN),
        FileSignature::stamp_of(&root.join(GREEN)).as_deref(),
        "写回时记下这份快照来自哪个内层版本"
    );

    // 内层随后被外部改动：此时再写回外层必须被拒绝，磁盘内容一个字节不动。
    rewrite_inner_prefab(&root, |sprite| sprite.tint[0] = 0.2);
    let mut sprite = sprite_of(&app, inner_root);
    sprite.sorting.order = 6;
    set_scene_sprite(&mut app, inner_root, sprite);
    let before = fs::read(root.join(NESTED)).unwrap();
    app.save_prefab_to_source(outer_root);
    assert!(
        app.status.contains("从内层预制体同步"),
        "内层动过而没同步时必须拒绝写回：{}",
        app.status
    );
    assert_eq!(
        fs::read(root.join(NESTED)).unwrap(),
        before,
        "拒绝写回时外层文件一个字节都不能动"
    );
    let _ = fs::remove_dir_all(root);
}

/// 显式同步：内层的新值被采纳，外层记录继续生效；一步 Undo 回到同步前。
#[test]
fn syncing_adopts_new_inner_fields_and_keeps_outer_overrides() {
    let (mut app, root) = open_sample_production("nested-sync");
    let nested = write_nested_green_prefab(&root);
    let (outer_root, inner_root) = place_nested_instance(&mut app, &nested);

    let mut sprite = sprite_of(&app, inner_root);
    sprite.sorting.order = 5;
    set_scene_sprite(&mut app, inner_root, sprite);
    app.save_prefab_to_source(outer_root);
    assert!(app.status.contains("已写回"), "{}", app.status);

    rewrite_inner_prefab(&root, |sprite| sprite.tint[0] = 0.2);

    // 菜单门：外层根不能同步，内层节点可以。
    app.selection
        .set_single(outer_root, editor::SelectionSource::Command);
    assert!(!app.menu_gate_open(MenuGate::NestedInstance));
    app.selection
        .set_single(inner_root, editor::SelectionSource::Command);
    assert!(app.menu_gate_open(MenuGate::NestedInstance));

    app.sync_nested_prefab_from_inner(inner_root)
        .expect("同步应成功");
    assert!(app.status.contains("保留 1 个字段覆盖"), "{}", app.status);

    // 重建后实例编号会变；随后的选区指向新的外层根。
    let new_outer_root = app.selection.primary().expect("同步后应选中新实例根");
    let new_inner_root = app.current_scene().unwrap().scene.entities[&new_outer_root].children[0];
    let synced = sprite_of(&app, new_inner_root);
    assert!(
        (synced.tint[0] - 0.2).abs() < 1e-3,
        "内层的新 tint 应被采纳：{:?}",
        synced.tint
    );
    assert_eq!(synced.sorting.order, 5, "外层的 order 覆盖必须保留");

    let saved = Prefab::load(root.join(NESTED)).unwrap();
    assert_eq!(
        saved.nested_overrides().unwrap(),
        BTreeMap::from([(
            (OUTER_ROOT_FILE_ID, "property:Sprite.order".to_owned()),
            "5".to_owned()
        )]),
        "同步不制造新的覆盖记录"
    );
    assert_eq!(
        saved.nested_stamp(GREEN),
        FileSignature::stamp_of(&root.join(GREEN)).as_deref(),
        "同步后签名必须跟上新内层"
    );

    // 同步自己进历史：Undo 回到同步前的场景快照。
    app.dispatch(editor::EditorCommand::Undo);
    let restored = sprite_of(&app, new_inner_root);
    assert!(
        (restored.tint[0] - 0.6).abs() < 1e-3,
        "Undo 应回到同步前的 tint：{:?}",
        restored.tint
    );
    assert_eq!(restored.sorting.order, 5);
    let _ = fs::remove_dir_all(root);
}

/// v1 只做字段级：内层结构变了就拒绝同步，外层文件保持原样。
#[test]
fn structural_inner_changes_are_refused_without_touching_the_outer_file() {
    let (mut app, root) = open_sample_production("nested-structure");
    let nested = write_nested_green_prefab(&root);
    let (_outer_root, inner_root) = place_nested_instance(&mut app, &nested);
    app.save_prefab_to_source(_outer_root);
    assert!(app.status.contains("已写回"), "{}", app.status);

    // 内层多挂一个节点：位置配对立刻失衡。
    let inner_path = root.join(GREEN);
    let inner = Prefab::load(&inner_path).unwrap();
    let mut scene = Scene::new("SlimeGreen");
    scene
        .add_entity(SceneEntityId(1), "SlimeGreen", None)
        .unwrap();
    let mut root_entity = inner.entities[&SceneEntityId(1)].clone();
    root_entity.children.push(SceneEntityId(2));
    root_entity.metadata.clear();
    scene.entities.insert(SceneEntityId(1), root_entity);
    scene
        .add_entity(SceneEntityId(2), "Extra", Some(SceneEntityId(1)))
        .unwrap();
    Prefab::from_scene(&scene, SceneEntityId(1), inner.asset_id, "SlimeGreen")
        .unwrap()
        .save(&inner_path)
        .unwrap();

    let before = fs::read(root.join(NESTED)).unwrap();
    let error = app
        .sync_nested_prefab_from_inner(inner_root)
        .expect_err("结构变化必须拒绝同步");
    assert!(error.contains("只支持字段级"), "{error}");
    assert_eq!(
        fs::read(root.join(NESTED)).unwrap(),
        before,
        "拒绝同步时外层文件一个字节都不能动"
    );
    let _ = fs::remove_dir_all(root);
}

/// 打开外层预制体文档时同步：就地重烘、标脏，保存后记录与签名都在文件里。
#[test]
fn prefab_document_sync_updates_the_document_and_survives_save() {
    let (mut app, root) = open_sample_production("nested-document");
    let nested = write_nested_green_prefab(&root);
    let (outer_root, inner_root) = place_nested_instance(&mut app, &nested);
    let mut sprite = sprite_of(&app, inner_root);
    sprite.sorting.order = 5;
    set_scene_sprite(&mut app, inner_root, sprite);
    app.save_prefab_to_source(outer_root);
    assert!(app.status.contains("已写回"), "{}", app.status);

    rewrite_inner_prefab(&root, |sprite| sprite.tint[0] = 0.2);

    app.perform_open_scene_resource(PathBuf::from(NESTED));
    assert!(
        app.current_scene()
            .is_some_and(editor::SceneDocument::is_prefab_document),
        "{}",
        app.status
    );
    let document_inner = OUTER_ROOT_FILE_ID;
    app.selection
        .set_single(document_inner, editor::SelectionSource::Command);
    assert!(app.menu_gate_open(MenuGate::NestedInstance));
    app.sync_nested_prefab_from_inner(document_inner)
        .expect("预制体文档里同步应成功");

    {
        let document = app.current_scene().unwrap();
        assert!(document.dirty, "同步后应标记为待保存");
        assert!(
            document
                .scene
                .metadata
                .keys()
                .any(|key| key.starts_with(PREFAB_NESTED_STAMP_META_PREFIX)),
            "文档里要带上内层签名"
        );
        let synced = sprite_of(&app, document_inner);
        assert!((synced.tint[0] - 0.2).abs() < 1e-3, "{:?}", synced.tint);
        assert_eq!(synced.sorting.order, 5);
    }

    assert!(
        app.save_scene(),
        "同步后的预制体文档应可保存：{}",
        app.status
    );
    let saved = Prefab::load(root.join(NESTED)).unwrap();
    assert_eq!(
        saved.nested_overrides().unwrap(),
        BTreeMap::from([(
            (OUTER_ROOT_FILE_ID, "property:Sprite.order".to_owned()),
            "5".to_owned()
        )]),
        "保存后记录仍在"
    );
    assert_eq!(
        saved.nested_stamp(GREEN),
        FileSignature::stamp_of(&root.join(GREEN)).as_deref(),
        "保存后签名仍在"
    );
    let _ = fs::remove_dir_all(root);
}
