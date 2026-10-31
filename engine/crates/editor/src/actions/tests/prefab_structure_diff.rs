//! 预制体结构差异：只读比较器 + 两条拒绝闸门同源 + 窗口的分区与定位。
//!
//! 守住的语义：
//! * 每种结构差异都有独立条目（增/删节点、换父、换序、嵌套身份、根移动）；
//! * 变体保存 / 嵌套同步的拒绝文案**点名**的条目，与窗口里列出的是同一次比较结果；
//! * 窗口只读：定位只改选区，不动文档、不写盘。

use super::*;
use crate::actions::prefab_structure_diff::{
    StructuralChangeKind, pair_and_diff_subtrees, structural_changes, summarize,
};
use crate::menu::MenuGate;
use rustlua_assets::resource_db::AssetId;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::Scene;
use std::collections::BTreeMap;

fn sample_prefab() -> Prefab {
    let mut scene = Scene::new("Sample");
    scene.add_entity(SceneEntityId(1), "Root", None).unwrap();
    scene
        .add_entity(SceneEntityId(2), "Head", Some(SceneEntityId(1)))
        .unwrap();
    scene
        .add_entity(SceneEntityId(3), "Body", Some(SceneEntityId(1)))
        .unwrap();
    Prefab::from_scene(
        &scene,
        SceneEntityId(1),
        AssetId::from_seed("sample.prefab"),
        "Sample",
    )
    .unwrap()
}

fn prefab_with(edit: impl FnOnce(&mut Scene)) -> Prefab {
    let mut scene = Scene::new("Sample");
    scene.add_entity(SceneEntityId(1), "Root", None).unwrap();
    scene
        .add_entity(SceneEntityId(2), "Head", Some(SceneEntityId(1)))
        .unwrap();
    scene
        .add_entity(SceneEntityId(3), "Body", Some(SceneEntityId(1)))
        .unwrap();
    edit(&mut scene);
    Prefab::from_scene(
        &scene,
        SceneEntityId(1),
        AssetId::from_seed("sample.prefab"),
        "Sample",
    )
    .unwrap()
}

#[test]
fn every_structural_kind_gets_its_own_entry() {
    let reference = sample_prefab();

    // 增实体：候选侧多出一个节点。
    let added = prefab_with(|scene| {
        scene
            .add_entity(SceneEntityId(4), "Tail", Some(SceneEntityId(1)))
            .unwrap();
    });
    let changes = structural_changes(&reference, &added, "基础", "当前快照");
    let change = changes
        .iter()
        .find(|change| change.kind == StructuralChangeKind::AddedEntity)
        .expect("应报多出实体");
    assert_eq!(change.candidate, Some(SceneEntityId(4)));
    assert!(change.detail.contains("Tail"), "{}", change.detail);

    // 删实体：参考侧有、候选侧没有。
    let removed = prefab_with(|scene| {
        scene.entities.remove(&SceneEntityId(3));
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .children
            .retain(|id| *id != SceneEntityId(3));
    });
    let changes = structural_changes(&reference, &removed, "基础", "当前快照");
    let change = changes
        .iter()
        .find(|change| change.kind == StructuralChangeKind::MissingEntity)
        .expect("应报缺少实体");
    assert_eq!(change.candidate, None, "缺少的实体没有可定位的候选侧编号");
    assert!(change.detail.contains("Body"), "{}", change.detail);

    // 换父：3 挂到 2 下面。
    let reparented = prefab_with(|scene| {
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .children
            .retain(|id| *id != SceneEntityId(3));
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .children
            .push(SceneEntityId(3));
        scene.entities.get_mut(&SceneEntityId(3)).unwrap().parent = Some(SceneEntityId(2));
    });
    let changes = structural_changes(&reference, &reparented, "基础", "当前快照");
    let change = changes
        .iter()
        .find(|change| change.kind == StructuralChangeKind::ParentChanged)
        .expect("应报父节点变了");
    assert_eq!(change.candidate, Some(SceneEntityId(3)));
    assert!(change.detail.contains("#1 → #2"), "{}", change.detail);

    // 换序：根的子节点顺序对调。
    let reordered = prefab_with(|scene| {
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .children
            .swap(0, 1);
    });
    let changes = structural_changes(&reference, &reordered, "基础", "当前快照");
    let change = changes
        .iter()
        .find(|change| change.kind == StructuralChangeKind::ChildrenChanged)
        .expect("应报子节点或顺序变了");
    assert_eq!(change.candidate, Some(SceneEntityId(1)));

    // 嵌套身份证：给内层节点盖上 prefab.source。
    let nested = prefab_with(|scene| {
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .metadata
            .insert("prefab.source".to_owned(), "inner.prefab".to_owned());
    });
    let changes = structural_changes(&reference, &nested, "基础", "当前快照");
    let change = changes
        .iter()
        .find(|change| change.kind == StructuralChangeKind::NestingIdentityChanged)
        .expect("应报嵌套身份变了");
    assert_eq!(change.candidate, Some(SceneEntityId(2)));
    assert!(change.detail.contains("prefab.source"), "{}", change.detail);

    // 根移动。
    let mut moved = reference.clone();
    moved.root = SceneEntityId(2);
    let changes = structural_changes(&reference, &moved, "基础", "当前快照");
    let change = changes
        .iter()
        .find(|change| change.kind == StructuralChangeKind::RootMoved)
        .expect("应报根实体变了");
    assert_eq!(change.candidate, Some(SceneEntityId(2)));
}

#[test]
fn summarize_names_the_first_entries_and_counting_the_rest() {
    let reference = sample_prefab();
    let mut candidate = reference.clone();
    for id in 4..9u64 {
        let mut entity = candidate.entities[&SceneEntityId(2)].clone();
        entity.id = SceneEntityId(id);
        entity.parent = None;
        entity.name = "Extra".to_owned();
        candidate.entities.insert(SceneEntityId(id), entity);
    }
    let changes = structural_changes(&reference, &candidate, "基础", "当前快照");
    assert_eq!(changes.len(), 5);
    let text = summarize(&changes);
    assert!(text.starts_with("5 项结构差异："), "{text}");
    assert!(text.contains("另有 2 项"), "只点名前 3 条：{text}");
    assert!(structural_changes(&reference, &reference, "基础", "当前快照").is_empty());
    assert_eq!(
        summarize(&structural_changes(
            &reference,
            &reference,
            "基础",
            "当前快照"
        )),
        "0 项结构差异"
    );
}

// ---- 变体保存：拒绝文案与窗口同源 ----

fn variant_fixture(label: &str) -> (EditorApp, PathBuf) {
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
    Prefab::from_scene(
        &source,
        SceneEntityId(1),
        AssetId::from_seed("enemy.prefab"),
        "Enemy",
    )
    .unwrap()
    .save(root.join("assets/prefabs/enemy.prefab"))
    .unwrap();

    let project = editor::open_project(&root).expect("临时工程应可打开");
    let app = EditorApp::new(project, None, LogSink::new(64));
    (app, root)
}

#[test]
fn variant_save_and_the_read_only_view_report_the_same_changes() {
    let (mut app, root) = variant_fixture("structure-variant");
    app.create_prefab_variant(Path::new("assets/prefabs/enemy.prefab"))
        .expect("创建变体应成功");
    app.perform_open_scene_resource(PathBuf::from("assets/prefabs/enemy_variant.prefab"));
    assert!(app.menu_gate_open(MenuGate::PrefabDocument));

    {
        let document = app.document.current_scene.as_mut().unwrap();
        document
            .scene
            .add_entity(SceneEntityId(3), "Extra", Some(SceneEntityId(1)))
            .unwrap();
        document.mark_dirty();
    }

    // 拒绝文案 = 比较器的摘要（同一次比较、同一批条目）。
    let base = Prefab::load(root.join("assets/prefabs/enemy.prefab")).unwrap();
    let current = app.current_scene().unwrap().prefab_asset().unwrap();
    let expected = summarize(&structural_changes(&base, &current, "基础", "当前快照"));
    assert!(!app.save_scene(), "增删节点属于结构差异，v1 必须拒绝");
    assert!(
        app.status.contains(&expected),
        "拒绝文案必须点名同一批条目\n期望：{expected}\n实际：{}",
        app.status
    );
    assert!(app.status.contains("Extra"), "{}", app.status);

    // 窗口：同一份差异，且候选侧编号可在当前文档里定位。
    app.open_prefab_structure_diff();
    assert_eq!(app.prefab_structure.sections.len(), 1);
    let section = &app.prefab_structure.sections[0];
    assert!(section.locatable, "当前文档就是变体，条目应可定位");
    assert_eq!(
        section.changes,
        structural_changes(&base, &current, "基础", "当前快照")
    );
    let added = section
        .changes
        .iter()
        .find(|change| change.kind == StructuralChangeKind::AddedEntity)
        .expect("窗口应列出多出的实体");
    let target = added.candidate.expect("多出的实体有候选侧编号");
    assert!(
        current.entities.contains_key(&target),
        "候选侧编号必须指向当前快照里的实体"
    );

    // 定位只改选区：文档与历史都不动。
    let entities_before = app.current_scene().unwrap().scene.entities.clone();
    let depth_before = app.history.undo_len();
    app.locate_structural_change(target);
    assert_eq!(
        app.selection.primary(),
        Some(target),
        "定位应把选区放到这条差异上"
    );
    assert_eq!(
        app.current_scene().unwrap().scene.entities,
        entities_before,
        "只读视图不许改文档"
    );
    assert_eq!(app.history.undo_len(), depth_before, "定位不进历史");
    let _ = fs::remove_dir_all(root);
}

// ---- 嵌套同步：拒绝文案与窗口同源 ----

const GREEN: &str = "assets/prefabs/slime_green.prefab";
const NESTED: &str = "assets/prefabs/nested_slime.prefab";
const OUTER_ROOT_FILE_ID: SceneEntityId = SceneEntityId(2);

fn write_nested_outer(project_root: &Path) -> Prefab {
    let inner = Prefab::load(project_root.join(GREEN)).expect("内层源可加载");
    let mut holder = Scene::new("NestedSlime");
    holder
        .add_entity(SceneEntityId(1), "NestedSlime", None)
        .unwrap();
    let staged = inner
        .instantiate(&mut holder, Some(SceneEntityId(1)), &BTreeMap::new())
        .unwrap();
    crate::actions::apply_prefab_instance_metadata(&mut holder, GREEN, &staged);
    let nested = Prefab::from_scene(
        &holder,
        SceneEntityId(1),
        AssetId::from_seed("nested_slime"),
        "NestedSlime",
    )
    .unwrap();
    nested.save(project_root.join(NESTED)).unwrap();
    nested
}

#[test]
fn nested_sync_refusal_and_the_read_only_view_report_the_same_changes() {
    let (mut app, root) = open_sample_production("structure-nested");
    let nested = write_nested_outer(&root);
    let (outer_root, inner_root) = {
        let scene = app.document.current_scene.as_mut().unwrap();
        let instance = nested
            .instantiate(&mut scene.scene, None, &BTreeMap::new())
            .unwrap();
        crate::actions::apply_prefab_instance_metadata(&mut scene.scene, NESTED, &instance);
        let outer_root = instance.instance_root;
        let inner_root = scene.scene.entities[&outer_root].children[0];
        (outer_root, inner_root)
    };
    app.save_prefab_to_source(outer_root);
    assert!(app.status.contains("已写回"), "{}", app.status);

    // 内层加一个节点（真实文件，模拟公共资产被改）。
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

    // 同理：拒绝文案与窗口读的是同一次比较。
    let new_inner = Prefab::load(&inner_path).unwrap();
    let outer = Prefab::load(root.join(NESTED)).unwrap();
    let (_, expected_changes) = pair_and_diff_subtrees(
        &outer,
        OUTER_ROOT_FILE_ID,
        &new_inner,
        crate::actions::prefab_nested_sync::nested_root_inner_id(&outer, OUTER_ROOT_FILE_ID)
            .unwrap(),
    )
    .unwrap();
    assert!(!expected_changes.is_empty());
    let expected = summarize(&expected_changes);

    let error = app
        .sync_nested_prefab_from_inner(inner_root)
        .expect_err("结构变化必须拒绝同步");
    assert!(error.contains("只支持字段级"), "{error}");
    assert!(
        error.contains(&expected),
        "拒绝文案必须点名同一批条目\n期望：{expected}\n实际：{error}"
    );

    // 打开外层预制体文档后，窗口把同一份差异列出来（这一层的直接嵌套实例）。
    app.perform_open_scene_resource(PathBuf::from(NESTED));
    assert!(app.menu_gate_open(MenuGate::PrefabDocument));
    app.open_prefab_structure_diff();
    let needle = format!("#{}", OUTER_ROOT_FILE_ID.0);
    let section = app
        .prefab_structure
        .sections
        .iter()
        .find(|section| section.title.contains(&needle))
        .expect("窗口应列出嵌套实例一节");
    assert!(section.locatable, "打开的就是外层预制体文件，条目应可定位");
    assert_eq!(section.changes, expected_changes, "窗口与闸门同源");
    assert!(
        section
            .changes
            .iter()
            .any(|change| change.kind == StructuralChangeKind::ChildrenChanged)
    );
    let _ = fs::remove_dir_all(root);
}
