//! Prefab 变体：创建 → 字段级覆盖记录 → 基础更新后的拒绝保存与显式同步。
//!
//! 这里守的是三条不可退让的语义：
//! * 变体文件本身是完整快照（运行时不需要基础）；
//! * 保存时覆盖集合由「基础 vs 快照」重新 diff，不冻结未同步的基础改动；
//! * 结构差异（增删节点/改层级）在 v1 里明确拒绝。

use super::*;
use crate::menu::MenuGate;
use editor::FileSignature;
use rustlua_assets::resource_db::AssetId;
use rustlua_scene::prefab::{
    PREFAB_OVERRIDE_META_PREFIX, PREFAB_VARIANT_BASE_META_KEY, PREFAB_VARIANT_BASE_STAMP_META_KEY,
    Prefab,
};
use rustlua_scene::scene::Scene;
use std::collections::BTreeMap;

fn variant_app(label: &str) -> (EditorApp, PathBuf) {
    let root = temp_root(label);
    fs::create_dir_all(root.join("assets/prefabs")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("scenes")).unwrap();
    fs::write(
        root.join("project.toml"),
        "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
    )
    .unwrap();
    fs::write(root.join("scripts/main.lua"), b"return true\n").unwrap();
    fs::write(root.join("scenes/main.scene"), b"name = \"main\"\n").unwrap();

    let mut source = Scene::new("Enemy");
    source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
    source
        .add_entity(SceneEntityId(2), "Weapon", Some(SceneEntityId(1)))
        .unwrap();
    source
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("Sprite".to_owned(), sprite_with("1.000000", 3));
    let prefab = Prefab::from_scene(
        &source,
        SceneEntityId(1),
        AssetId::from_seed("enemy.prefab"),
        "Enemy",
    )
    .unwrap();
    prefab
        .save(root.join("assets/prefabs/enemy.prefab"))
        .unwrap();

    let project = editor::open_project(&root).expect("临时工程应可打开");
    let app = EditorApp::new(project, None, LogSink::new(64));
    (app, root)
}

/// `Sprite` 在组件 schema 里：覆盖能落到单个字段（`property:Sprite.tint`）而不是整条组件串。
/// 数值一律写六位小数，与 `write_builtin` 的回写口径一致，比较时不因格式差异假红。
fn sprite_with(tint_red: &str, order: u32) -> String {
    format!(
        "texture=a.png;size=16x16;pivot=0.500000,0.500000;\
         tint={tint_red},1.000000,1.000000,1.000000;sorting_layer=world;order={order}"
    )
}

fn open_variant(app: &mut EditorApp) {
    app.perform_open_scene_resource(PathBuf::from("assets/prefabs/enemy_variant.prefab"));
    assert!(
        app.current_scene()
            .is_some_and(editor::SceneDocument::is_prefab_variant),
        "打开失败：{}",
        app.status
    );
}

fn variant_path(root: &Path) -> PathBuf {
    root.join("assets/prefabs/enemy_variant.prefab")
}

fn base_path(root: &Path) -> PathBuf {
    root.join("assets/prefabs/enemy.prefab")
}

#[test]
fn creating_a_variant_copies_the_snapshot_and_stamps_the_base() {
    let (mut app, root) = variant_app("variant-create");
    let status = app
        .create_prefab_variant(Path::new("assets/prefabs/enemy.prefab"))
        .expect("创建变体应成功");
    assert!(status.contains("已创建变体"), "{status}");

    let base = Prefab::load(base_path(&root)).unwrap();
    let variant = Prefab::load(variant_path(&root)).unwrap();
    assert_eq!(variant.entities, base.entities, "快照必须逐字等于基础");
    assert_eq!(variant.variant_base(), Some("assets/prefabs/enemy.prefab"));
    assert_eq!(
        variant.variant_base_stamp(),
        FileSignature::stamp_of(&base_path(&root)).as_deref(),
        "创建时记下基础签名"
    );
    assert!(
        variant.overrides().unwrap().is_empty(),
        "新变体没有任何覆盖"
    );
    assert_ne!(variant.asset_id, base.asset_id, "变体是独立资产");
    assert_eq!(
        app.selected_resource,
        Some(PathBuf::from("assets/prefabs/enemy_variant.prefab"))
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn saving_a_variant_records_field_overrides_and_refuses_an_unsynced_base() {
    let (mut app, root) = variant_app("variant-save");
    app.create_prefab_variant(Path::new("assets/prefabs/enemy.prefab"))
        .expect("创建变体应成功");
    open_variant(&mut app);

    // 改一个字段并保存：覆盖记录应精确到字段，快照本身同步更新。
    {
        let document = app.document.current_scene.as_mut().unwrap();
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Sprite".to_owned(), sprite_with("0.500000", 3));
        document.mark_dirty();
    }
    assert!(app.save_scene(), "变体应可保存：{}", app.status);
    let saved = Prefab::load(variant_path(&root)).unwrap();
    assert_eq!(
        saved.overrides().unwrap(),
        BTreeMap::from([(
            (SceneEntityId(2), "property:Sprite.tint".to_owned()),
            "0.500000,1.000000,1.000000,1.000000".to_owned()
        )]),
        "只记录真正变化的字段"
    );
    assert_eq!(
        saved.entities[&SceneEntityId(2)].components["Sprite"],
        sprite_with("0.500000", 3)
    );
    assert_eq!(
        saved.variant_base_stamp(),
        FileSignature::stamp_of(&base_path(&root)).as_deref()
    );

    // 基础在外部被改：变体保存必须被拒绝，且磁盘内容不变（不能把基础新值记成覆盖）。
    let mut base = Prefab::load(base_path(&root)).unwrap();
    base.entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("Sprite".to_owned(), sprite_with("1.000000", 7));
    base.save(base_path(&root)).unwrap();
    let before = fs::read(variant_path(&root)).unwrap();
    assert!(!app.save_scene(), "基础动过而没同步时必须拒绝保存");
    assert!(app.status.contains("从基础同步"), "{}", app.status);
    assert_eq!(
        fs::read(variant_path(&root)).unwrap(),
        before,
        "拒绝保存时磁盘文件一个字节都不能动"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn syncing_from_base_adopts_new_values_but_keeps_variant_overrides_in_one_undo() {
    let (mut app, root) = variant_app("variant-sync");
    app.create_prefab_variant(Path::new("assets/prefabs/enemy.prefab"))
        .expect("创建变体应成功");
    open_variant(&mut app);

    // 变体先拥有自己的字段覆盖（tint），基础随后只改了 order。
    {
        let document = app.document.current_scene.as_mut().unwrap();
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Sprite".to_owned(), sprite_with("0.500000", 3));
        document.mark_dirty();
    }
    assert!(app.save_scene(), "变体应可保存：{}", app.status);

    let mut base = Prefab::load(base_path(&root)).unwrap();
    base.entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("Sprite".to_owned(), sprite_with("1.000000", 7));
    base.save(base_path(&root)).unwrap();

    app.sync_prefab_variant_from_base().expect("同步应成功");
    {
        let document = app.current_scene().unwrap();
        assert!(document.dirty, "同步后应标记为待保存");
        assert_eq!(
            document.scene.entities[&SceneEntityId(2)].components["Sprite"],
            sprite_with("0.500000", 7),
            "没有覆盖的字段采用基础的新值，已有覆盖保持变体的值"
        );
    }

    // 再给变体改一个已覆盖字段，验证"覆盖在新基础上生效"。
    {
        let document = app.document.current_scene.as_mut().unwrap();
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Sprite".to_owned(), sprite_with("0.250000", 7));
        document.mark_dirty();
    }
    assert!(app.save_scene(), "同步后应可保存：{}", app.status);
    let saved = Prefab::load(variant_path(&root)).unwrap();
    assert_eq!(
        saved.entities[&SceneEntityId(2)].components["Sprite"],
        sprite_with("0.250000", 7)
    );
    assert_eq!(
        saved.overrides().unwrap(),
        BTreeMap::from([(
            (SceneEntityId(2), "property:Sprite.tint".to_owned()),
            "0.250000,1.000000,1.000000,1.000000".to_owned()
        )]),
        "基础带来的 order 不算覆盖，只留下 tint"
    );
    assert_eq!(
        saved.variant_base_stamp(),
        FileSignature::stamp_of(&base_path(&root)).as_deref(),
        "同步后 stamp 必须跟上新的基础签名"
    );

    // 同步那一步自己也进历史：Undo 应回到同步前的变体快照。
    app.dispatch(editor::EditorCommand::Undo);
    assert_eq!(
        app.current_scene().unwrap().scene.entities[&SceneEntityId(2)].components["Sprite"],
        sprite_with("0.500000", 3),
        "Undo 应回到同步前的快照（同步前的值与顺序）"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn variants_refuse_structural_edits_and_instance_write_backs() {
    let (mut app, root) = variant_app("variant-structure");
    app.create_prefab_variant(Path::new("assets/prefabs/enemy.prefab"))
        .expect("创建变体应成功");
    open_variant(&mut app);

    {
        let document = app.document.current_scene.as_mut().unwrap();
        document
            .scene
            .add_entity(SceneEntityId(3), "Extra", Some(SceneEntityId(1)))
            .unwrap();
        document.mark_dirty();
    }
    assert!(!app.save_scene(), "增删节点属于结构差异，v1 必须拒绝");
    assert!(app.status.contains("字段级"), "{}", app.status);

    // 菜单门：只有变体文档放行「从基础同步」。
    assert!(app.menu_gate_open(MenuGate::VariantDocument));

    // 变体源不能被场景实例写回（会改快照却不重算覆盖记录）。
    let variant = Prefab::load(variant_path(&root)).unwrap();
    assert!(
        crate::actions::prefab_variant::variant_write_back_refusal(&variant).is_some(),
        "变体源必须拒绝实例写回"
    );
    let base = Prefab::load(base_path(&root)).unwrap();
    assert!(
        crate::actions::prefab_variant::variant_write_back_refusal(&base).is_none(),
        "普通预制体不受影响"
    );
    let _ = fs::remove_dir_all(root);
}

/// 谱系记录只走文件级 meta：文档层往返（打开→保存→重开）后仍能认出变体。
#[test]
fn variant_document_recognizes_its_lineage_after_reopen() {
    let (mut app, root) = variant_app("variant-reopen");
    app.create_prefab_variant(Path::new("assets/prefabs/enemy.prefab"))
        .expect("创建变体应成功");
    open_variant(&mut app);
    assert!(app.menu_gate_open(MenuGate::VariantDocument));

    {
        let document = app.document.current_scene.as_mut().unwrap();
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Sprite".to_owned(), sprite_with("0.750000", 3));
        document.mark_dirty();
    }
    assert!(app.save_scene(), "变体应可保存：{}", app.status);

    // 重新从磁盘打开：谱系与覆盖记录必须仍在文档里（而不是只活在内存）。
    app.perform_open_scene_resource(PathBuf::from("assets/prefabs/enemy_variant.prefab"));
    assert!(
        app.current_scene()
            .is_some_and(editor::SceneDocument::is_prefab_variant),
        "{}",
        app.status
    );
    let document = app.current_scene().unwrap();
    let metadata = &document.scene.metadata;
    assert_eq!(
        metadata[PREFAB_VARIANT_BASE_META_KEY], "assets/prefabs/enemy.prefab",
        "谱系必须从 Prefab.metadata 穿到场景文档"
    );
    assert!(
        metadata.contains_key(PREFAB_VARIANT_BASE_STAMP_META_KEY),
        "stamp 必须一起穿过文档层"
    );
    assert!(
        metadata
            .keys()
            .any(|key| key.starts_with(PREFAB_OVERRIDE_META_PREFIX)),
        "覆盖记录必须一起穿过文档层"
    );
    let _ = fs::remove_dir_all(root);
}
