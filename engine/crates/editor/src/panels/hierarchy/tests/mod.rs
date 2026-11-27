use super::model::HIERARCHY_ROW_HEIGHT;
use super::{HierarchyFilter, HierarchyMatchIndex, RowHeightProbe, hierarchy_row_height};
use editor::SceneDocument;
use eframe::egui;
use rustlua_assets::resource_db::AssetId;
use rustlua_infra::logging::LogSink;
use rustlua_scene::scene::SceneEntityId;

/// **嵌套 Prefab 的创建边界**：从"含 Prefab 实例"的子树创建预制体 现在要**成功**，
/// 并把内层身份逐字记进文件（内层来源、内层编号、内层实例根），否则实例化后
/// 那一支会退化成普通节点、编辑器再也认不出它属于谁。
/// 仍然拒绝的只剩"新预制体引用自己"这一类坏数据；平实体照旧。
#[test]
fn creating_a_nested_prefab_records_the_inner_identity_and_refuses_self_reference() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-create-prefab-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
    std::fs::create_dir_all(root.join("scenes")).unwrap();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
    std::fs::write(root.join("scripts/main.lua"), b"return true\n").unwrap();
    let project = editor::open_project(&root).expect("project opens");
    let mut app = crate::EditorApp::new(project, None, LogSink::new(64));
    let scene = app.document.new_scene("main");
    let instance_root = app.tree.create_entity(scene, "Hero", None).unwrap();
    let child = app
        .tree
        .create_entity(scene, "Child", Some(instance_root))
        .unwrap();
    // 子树里带全套内层标记 = 这一支是 sword.prefab 的实例（真实数据三个键都在）。
    {
        let markers = [
            ("prefab.source", "assets/prefabs/sword.prefab"),
            ("prefab.source_id", "1"),
            ("prefab.root", "true"),
        ];
        let entity = app
            .document
            .current_scene
            .as_mut()
            .unwrap()
            .scene
            .entities
            .get_mut(&child)
            .unwrap();
        for (key, value) in markers {
            entity.metadata.insert(key.to_owned(), value.to_owned());
        }
    }
    app.selection
        .set_single(instance_root, editor::SelectionSource::Hierarchy);

    app.create_prefab_from_selected();
    let written = root.join("assets/prefabs/Hero.prefab");
    // 注：创建成功会触发后台资源刷新，`status` 会被刷新的提示覆盖，所以断言产物。
    let prefab = rustlua_scene::prefab::Prefab::load(&written)
        .unwrap_or_else(|error| panic!("嵌套预制体应创建成功（status={}）：{error}", app.status));
    assert_eq!(
        prefab.nested_sources(),
        vec!["assets/prefabs/sword.prefab".to_owned()],
        "外层必须知道自己嵌了谁"
    );
    let marked: Vec<_> = prefab
        .entities
        .values()
        .filter(|entity| entity.metadata.contains_key("prefab.source"))
        .cloned()
        .collect();
    assert_eq!(marked.len(), 1, "内层标记只该落在那个子节点上：{prefab:?}");
    assert_eq!(
        marked[0].metadata["prefab.source"],
        "assets/prefabs/sword.prefab"
    );
    assert_eq!(
        marked[0].metadata["prefab.source_id"], "1",
        "内层文件里的实体编号要逐字保住"
    );
    assert_eq!(
        marked[0].metadata["prefab.root"], "true",
        "内层实例根标记也要保住"
    );

    // **自指**：子树里挂的正是"要创建的这张"——新文件一上手就引用自己，必须拒绝且不落盘。
    std::fs::remove_file(&written).unwrap();
    app.document
        .current_scene
        .as_mut()
        .unwrap()
        .scene
        .entities
        .get_mut(&child)
        .unwrap()
        .metadata
        .insert(
            "prefab.source".to_owned(),
            "assets/prefabs/Hero.prefab".to_owned(),
        );
    app.create_prefab_from_selected();
    assert!(
        app.status.contains("嵌套会指向自己"),
        "必须明确拒绝自指并说明原因：{}",
        app.status
    );
    assert!(!written.exists(), "被拒绝时不得写出文件");

    // 平实体（去掉全部标记）照常创建，且不带任何嵌套引用。
    let entity = app
        .document
        .current_scene
        .as_mut()
        .unwrap()
        .scene
        .entities
        .get_mut(&child)
        .unwrap();
    entity.metadata.retain(|key, _| !key.starts_with("prefab."));
    app.create_prefab_from_selected();
    let flat = rustlua_scene::prefab::Prefab::load(&written).expect("平实体应照常创建");
    assert!(
        flat.nested_sources().is_empty(),
        "平 Prefab 不该带任何嵌套引用"
    );
    assert!(!flat.entities.is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// 扫出"这份 Prefab 里嵌了谁"（引擎侧事实源 `Prefab::nested_sources`）。
#[test]
fn nested_sources_report_every_prefab_reference_in_the_file() {
    let mut scene = rustlua_scene::scene::Scene::new("hero");
    let root = SceneEntityId(1);
    scene.add_entity(root, "Hero", None).unwrap();
    let child = SceneEntityId(2);
    scene.add_entity(child, "Weapon", Some(root)).unwrap();
    scene.entities.get_mut(&child).unwrap().metadata.insert(
        "prefab.source".to_owned(),
        "assets/prefabs/sword.prefab".to_owned(),
    );
    let prefab =
        rustlua_scene::prefab::Prefab::from_scene(&scene, root, AssetId::from_seed("hero"), "hero")
            .unwrap();
    assert_eq!(
        prefab.nested_sources(),
        vec!["assets/prefabs/sword.prefab".to_owned()]
    );
}

#[test]
fn new_scene_name_avoids_overwriting_existing_scenes() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-new-scene-name-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(root.join("scenes")).unwrap();
    let mut app = crate::EditorApp::new(
        editor::ProjectInfo {
            root: root.clone(),
            version: "test".to_owned(),
            directories: Vec::new(),
        },
        None,
        LogSink::new(64),
    );
    // 空项目：直接叫 main。
    assert_eq!(app.next_new_scene_name(), "main");

    // 磁盘上已有 main.scene → 退到 main2；再占掉 main2 → main3。
    std::fs::write(root.join("scenes/main.scene"), "placeholder").unwrap();
    assert_eq!(app.next_new_scene_name(), "main2");
    std::fs::write(root.join("scenes/main2.scene"), "placeholder").unwrap();
    assert_eq!(app.next_new_scene_name(), "main3");

    // 当前文档即使还没落盘也要占位，避免"新建两次同名"。
    app.document.current_scene = Some(SceneDocument::new_scene("main3"));
    assert_eq!(app.next_new_scene_name(), "main4");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hierarchy_filter_keeps_matching_ancestor_path_visible() {
    let mut document = SceneDocument::new_scene("main");
    let root = SceneEntityId(1);
    let branch = SceneEntityId(2);
    let leaf = SceneEntityId(3);
    document.scene.add_entity(root, "Root", None).unwrap();
    document
        .scene
        .add_entity(branch, "Branch", Some(root))
        .unwrap();
    document
        .scene
        .add_entity(leaf, "Leaf", Some(branch))
        .unwrap();
    let mut app = crate::EditorApp::new(
        editor::ProjectInfo {
            root: std::env::temp_dir()
                .join(format!("rustlua-hierarchy-filter-{}", std::process::id())),
            version: "test".to_owned(),
            directories: Vec::new(),
        },
        None,
        LogSink::new(64),
    );
    app.document.current_scene = Some(document);
    app.hierarchy_filter = "Leaf".to_owned();

    let scene = app.current_scene().cloned().expect("scene present");
    let root_visible = app.hierarchy_filter_matches(&scene, root);
    let branch_visible = app.hierarchy_filter_matches(&scene, branch);
    let leaf_visible = app.hierarchy_filter_matches(&scene, leaf);

    assert!(root_visible);
    assert!(branch_visible);
    assert!(leaf_visible);
}

/// 测试夹具：Root(1) -> A(2), Leaf(3)；Solo(4) 为第二个根。
fn sample_document() -> SceneDocument {
    let mut document = SceneDocument::new_scene("main");
    document
        .scene
        .add_entity(SceneEntityId(1), "Root", None)
        .unwrap();
    document
        .scene
        .add_entity(SceneEntityId(2), "A", Some(SceneEntityId(1)))
        .unwrap();
    document
        .scene
        .add_entity(SceneEntityId(3), "Leaf", Some(SceneEntityId(1)))
        .unwrap();
    document
        .scene
        .add_entity(SceneEntityId(4), "Solo", None)
        .unwrap();
    document
}

fn make_app(document: SceneDocument) -> crate::EditorApp {
    let mut app = crate::EditorApp::new(
        editor::ProjectInfo {
            root: std::env::temp_dir()
                .join(format!("rustlua-hierarchy-opts-{}", std::process::id())),
            version: "test".to_owned(),
            directories: Vec::new(),
        },
        None,
        LogSink::new(64),
    );
    app.document.current_scene = Some(document);
    app
}

/// **创建预制体不静默覆盖**：第二个同名实体再创建一次，必须落到 `Hero_1.prefab`，
/// 且原 `Hero.prefab` 的文件内容一字不变（Unity 的"同名取后缀"语义）。
#[test]
fn creating_a_prefab_with_an_existing_name_never_overwrites() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-prefab-dedupe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
    std::fs::create_dir_all(root.join("scenes")).unwrap();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.lua"), b"return true\n").unwrap();
    let project = editor::open_project(&root).expect("project opens");
    let mut app = crate::EditorApp::new(project, None, LogSink::new(64));
    let scene = app.document.new_scene("main");
    let first = app.tree.create_entity(scene, "Hero", None).unwrap();
    app.selection
        .set_single(first, editor::SelectionSource::Hierarchy);
    app.create_prefab_from_selected();
    let original = root.join("assets/prefabs/Hero.prefab");
    assert!(original.exists(), "第一份必须落盘：{}", app.status);
    let before = std::fs::read(&original).unwrap();

    // 第二个**同名**实体：再来一次。
    let second = {
        let scene = app.document.current_scene.as_mut().unwrap();
        app.tree.create_entity(scene, "Hero", None).unwrap()
    };
    app.selection
        .set_single(second, editor::SelectionSource::Hierarchy);
    app.create_prefab_from_selected();
    let deduped = root.join("assets/prefabs/Hero_1.prefab");
    assert!(
        deduped.exists(),
        "同名时必须改名而不是覆盖（status={}）",
        app.status
    );
    assert_eq!(
        std::fs::read(&original).unwrap(),
        before,
        "原预制体文件不得被改写"
    );
    // 状态栏会被随后的"正在后台刷新资源…"覆盖（既有行为），落盘路径记在 Console 里。
    assert!(
        app.console_notices
            .iter()
            .any(|line| line.contains("Hero_1.prefab")),
        "必须报出真正的落盘路径：{:?}",
        app.console_notices
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 去重命名本身：`Hero` → `Hero_1` → `Hero_2`，空目录保留原名。
#[test]
fn unique_prefab_stem_appends_the_first_free_suffix() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-prefab-stem-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(
        super::scene_ops::unique_prefab_stem(&root, "Hero"),
        "Hero",
        "无冲突时保留原名"
    );
    std::fs::write(root.join("Hero.prefab"), b"x").unwrap();
    assert_eq!(
        super::scene_ops::unique_prefab_stem(&root, "Hero"),
        "Hero_1"
    );
    std::fs::write(root.join("Hero_1.prefab"), b"x").unwrap();
    assert_eq!(
        super::scene_ops::unique_prefab_stem(&root, "Hero"),
        "Hero_2"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// H3：主选中变化时揭示——展开祖先并请求滚动；同一选中不重复抢滚动位置。
#[test]
fn selecting_an_entity_reveals_its_ancestors_and_requests_one_scroll() {
    let mut app = make_app(sample_document());
    let root = SceneEntityId(1);
    let child = SceneEntityId(2);
    // 用户先把 Root 折叠起来，再从别处（场景视图/撤销）选中 Root 下的 A。
    app.scene_state.collapsed.insert(root);
    app.select_single_hierarchy(child);

    app.maybe_reveal_primary_selection();
    assert!(
        !app.scene_state.collapsed.contains(&root),
        "祖先必须被展开，否则选中了也看不见"
    );
    assert_eq!(
        app.hierarchy_scroll_target,
        Some(child),
        "要请求把目标行滚进视口"
    );

    // 滚动请求被面板消费掉之后，同一主选中不再产生新的请求：
    // 用户手动折叠正在选中的节点时，不许被下一帧抢回去。
    app.hierarchy_scroll_target = None;
    app.scene_state.collapsed.insert(root);
    app.maybe_reveal_primary_selection();
    assert_eq!(app.hierarchy_scroll_target, None);
    assert!(
        app.scene_state.collapsed.contains(&root),
        "同一选中期间不得重新展开"
    );

    // 换一个主选中 → 再次揭示。
    app.selection
        .set_single(SceneEntityId(3), editor::SelectionSource::SceneView2D);
    app.maybe_reveal_primary_selection();
    assert_eq!(app.hierarchy_scroll_target, Some(SceneEntityId(3)));
    assert!(!app.scene_state.collapsed.contains(&root));
}

/// H4：锁定语义 = **只挡场景拾取**。层级里仍然可选中、可删除
///（场景视图里的命中与拖动跳过硬定已有 `pick_scene_2d_entity_uses_topmost_unlocked_hit` 覆盖）。
#[test]
fn a_locked_entity_stays_selectable_and_deletable_in_the_hierarchy() {
    let mut app = make_app(sample_document());
    app.scene_state.locked.insert(SceneEntityId(2));

    app.select_single_hierarchy(SceneEntityId(2));
    assert_eq!(
        app.selection.primary(),
        Some(SceneEntityId(2)),
        "锁定不该让层级行点不中"
    );

    app.delete_selected_entity();
    assert!(
        !app.document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .contains_key(&SceneEntityId(2)),
        "显式删除动作对锁定实体同样生效（锁不是写保护）"
    );
}

/// ER-04 的"类型过滤"（第 41 轮）：`type:` / `component:` 按**组件名**过滤（子串、
/// 大小写不敏感），与名称词是 AND 关系；`type:` 只输入到一半不算过滤。
#[test]
fn type_filter_matches_components_and_combines_with_name_terms() {
    let mut document = sample_document();
    {
        let entities = &mut document.scene.entities;
        entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Sprite".to_owned(), "texture=;size=1x1".to_owned());
        entities
            .get_mut(&SceneEntityId(3))
            .unwrap()
            .components
            .insert(
                "Light".to_owned(),
                "direction=0,1,0;color=1,1,1;intensity=1".to_owned(),
            );
        let solo = entities.get_mut(&SceneEntityId(4)).unwrap();
        solo.components
            .insert("UiLayout".to_owned(), "pivot=0,0".to_owned());
        solo.components
            .insert("UiEvents".to_owned(), "event=click;function=f".to_owned());
    }
    let mut app = make_app(document);

    // 组件名命中 → 该行与它的祖先路径可见（祖先自身不算命中）。
    app.hierarchy_filter = "type:sprite".to_owned();
    assert_eq!(
        app.hierarchy_visible_ids(),
        vec![SceneEntityId(1), SceneEntityId(2)]
    );
    assert_eq!(app.hierarchy_match_count(), 1, "计数只算**自身**命中的实体");

    // 子串匹配一次命中多个 UI 组件（`UiLayout`/`UiEvents` 都算）——这是最有用的用法。
    app.hierarchy_filter = "type:ui".to_owned();
    assert_eq!(app.hierarchy_visible_ids(), vec![SceneEntityId(4)]);
    assert_eq!(app.hierarchy_match_count(), 1);

    // 大小写不敏感；`component:` 与 `type:` 同义。
    app.hierarchy_filter = "component:SPRITE".to_owned();
    assert_eq!(app.hierarchy_match_count(), 1);

    // 名称词与类型词是 AND：`A` 有 Sprite → 命中；`Leaf` 只有 Light → 不命中。
    app.hierarchy_filter = "a type:sprite".to_owned();
    assert_eq!(app.hierarchy_match_count(), 1);
    app.hierarchy_filter = "Leaf type:sprite".to_owned();
    assert_eq!(app.hierarchy_match_count(), 0);
    assert!(
        app.hierarchy_visible_ids().is_empty(),
        "没有任何实体命中时不应留下祖先行"
    );

    // 只输入到一半的 `type:` → 视为没有过滤（否则每敲一个前缀就清空整棵树）。
    app.hierarchy_filter = "type:".to_owned();
    assert_eq!(app.hierarchy_visible_ids().len(), 4);
    assert_eq!(app.hierarchy_match_count(), 4);
    // 类型词不认识任何组件 → 空结果（不是"没有过滤"）。
    app.hierarchy_filter = "type:nope".to_owned();
    assert!(app.hierarchy_visible_ids().is_empty());
    assert_eq!(app.hierarchy_match_count(), 0);
}

#[test]
fn hierarchy_visible_ids_respects_collapse_and_filter() {
    let mut app = make_app(sample_document());
    app.scene_state.collapsed.insert(SceneEntityId(1));
    assert_eq!(
        app.hierarchy_visible_ids(),
        vec![SceneEntityId(1), SceneEntityId(4)]
    );
    app.hierarchy_filter = "Leaf".to_owned();
    // Leaf 是 Root 的直接子节点：A 不是 Leaf 的祖先，应被过滤掉。
    assert_eq!(
        app.hierarchy_visible_ids(),
        vec![SceneEntityId(1), SceneEntityId(3)]
    );
}

#[test]
fn visible_cache_invalidates_on_structure_and_collapse_changes() {
    let mut app = make_app(sample_document());
    let first = app.hierarchy_visible_ids();
    assert_eq!(
        first,
        vec![
            SceneEntityId(1),
            SceneEntityId(2),
            SceneEntityId(3),
            SceneEntityId(4)
        ]
    );
    // Repeat call is a cache hit with identical content.
    assert_eq!(app.hierarchy_visible_ids(), first);
    // Structural addition without an explicit revision bump must still invalidate.
    app.document
        .current_scene
        .as_mut()
        .unwrap()
        .scene
        .add_entity(SceneEntityId(5), "Added", None)
        .unwrap();
    assert!(app.hierarchy_visible_ids().contains(&SceneEntityId(5)));
    // Collapse state is part of the key.
    app.scene_state.collapsed.insert(SceneEntityId(1));
    assert_eq!(
        app.hierarchy_visible_ids(),
        vec![SceneEntityId(1), SceneEntityId(4), SceneEntityId(5)]
    );
}

#[test]
fn hierarchy_rows_carry_depth_and_effective_collapse() {
    // 行模型是虚拟滚动的唯一数据源：这里锁定深度与折叠语义，防止"少画/错缩进"退化。
    let mut app = make_app(sample_document());
    let rows = app.hierarchy_rows();
    assert_eq!(
        rows.iter()
            .map(|row| (row.id, row.depth))
            .collect::<Vec<_>>(),
        vec![
            (SceneEntityId(1), 0),
            (SceneEntityId(2), 1),
            (SceneEntityId(3), 1),
            (SceneEntityId(4), 0),
        ]
    );
    assert!(rows[0].has_children, "Root 有子节点");
    assert!(!rows[1].has_children, "A 是叶子");

    // 折叠 Root：子节点不再进入行模型，且 Root 行标记为折叠。
    app.scene_state.collapsed.insert(SceneEntityId(1));
    let rows = app.hierarchy_rows();
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![SceneEntityId(1), SceneEntityId(4)]
    );
    assert!(rows[0].effective_collapsed);

    // 过滤命中子节点时强制展开：折叠状态被 effective_collapsed=false 覆盖。
    app.hierarchy_filter = "Leaf".to_owned();
    let rows = app.hierarchy_rows();
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![SceneEntityId(1), SceneEntityId(3)]
    );
    assert!(!rows[0].effective_collapsed, "过滤强制展开时箭头显示为展开");
    assert_eq!(rows[1].depth, 1);
}

/// 索引化过滤必须与"逐行递归"参考实现逐实体等价（防止优化改变可见性语义）。
#[test]
fn match_index_is_equivalent_to_recursive_reference() {
    // 构造一个更深、更宽的树：4 个根，每根 3 层，名称里混入大小写与部分匹配词。
    let mut document = SceneDocument::new_scene("equivalent");
    let mut next = 1_u64;
    let mut ids = Vec::new();
    for root in 0..4_u64 {
        let root_id = SceneEntityId(next);
        next += 1;
        document
            .scene
            .add_entity(root_id, format!("Group{root}"), None)
            .unwrap();
        ids.push(root_id);
        let mut parents = vec![root_id];
        for depth in 0..3_u64 {
            let mut children = Vec::new();
            for slot in 0..3_u64 {
                let id = SceneEntityId(next);
                next += 1;
                let name = match (depth + slot) % 3 {
                    0 => format!("Leaf_{depth}_{slot}"),
                    1 => format!("Branch_{depth}_{slot}"),
                    _ => format!("other_{depth}_{slot}"),
                };
                document
                    .scene
                    .add_entity(id, name, Some(parents[0]))
                    .unwrap();
                // 混入组件名，让 `type:` 词也在这条等价性门里被覆盖（每个实体两种组件）。
                let entity = document.scene.entities.get_mut(&id).unwrap();
                entity.components.insert(
                    if slot % 2 == 0 { "Sprite" } else { "Light" }.to_owned(),
                    "x=1".to_owned(),
                );
                entity.components.insert(
                    if depth % 2 == 0 {
                        "UiLayout"
                    } else {
                        "Collider"
                    }
                    .to_owned(),
                    "x=1".to_owned(),
                );
                children.push(id);
                ids.push(id);
            }
            parents = children;
        }
    }
    let mut app = make_app(document);

    for query in [
        "leaf",
        "LEAF_1",
        "branch",
        "Group2",
        "missing",
        "other_2_0",
        // 类型词、类型词 + 名称词、大小写、以及"刚输入到一半的 type:"。
        "type:sprite",
        "type:ui",
        "component:transform3d",
        "leaf type:sprite",
        "branch component:UiLayout",
        "type:",
    ] {
        app.hierarchy_filter = query.to_owned();
        let scene = app.current_scene().cloned().expect("scene");
        let filter = HierarchyFilter::parse(query);
        let index = {
            let lower = app.hierarchy_lower.ensure(&scene);
            HierarchyMatchIndex::build(&scene, &filter, lower)
        };
        for id in &ids {
            let reference = app.hierarchy_filter_matches(&scene, *id);
            assert_eq!(
                index.state(*id).subtree_matches,
                reference,
                "query={query:?} entity={id:?} 索引与递归参考实现必须一致"
            );
        }
    }
}

/// H05：过滤命中计数取扁平化时一次算出的结果（不再每帧全量扫描）。
#[test]
fn match_count_matches_manual_scan() {
    let mut app = make_app(sample_document());
    app.hierarchy_filter = "Leaf".to_owned();
    let _ = app.hierarchy_rows();
    assert_eq!(app.hierarchy_match_count(), 1);
    app.hierarchy_filter = "o".to_owned();
    // Root(1)/Solo(4) 的名称含 'o'（大小写不敏感），A(2)/Leaf(3) 不含。
    assert_eq!(app.hierarchy_match_count(), 2);
}

/// 布局探针语义：取最大值、可重置、能计数（门槛判定依赖这三点）。
#[test]
fn row_height_probe_tracks_max_content_and_resets() {
    let mut probe = RowHeightProbe::default();
    assert_eq!(probe.rows(), 0);
    assert_eq!(probe.max_content_height(), 0.0);
    probe.observe(18.0);
    probe.observe(21.5);
    probe.observe(19.0);
    assert_eq!(probe.rows(), 3);
    assert_eq!(probe.max_content_height(), 21.5);
    probe.reset();
    assert_eq!(probe.rows(), 0);
    assert_eq!(probe.max_content_height(), 0.0);
}

/// 行高必须由样式推导，且不小于基础值（DPI/缩放下不塌陷）。
#[test]
fn hierarchy_row_height_follows_style_and_never_collapses() {
    let context = egui::Context::default();
    let raw = egui::RawInput::default();
    let mut height = 0.0_f32;
    let _ = context.run(raw, |context| {
        egui::CentralPanel::default().show(context, |ui| {
            height = hierarchy_row_height(ui);
        });
    });
    assert!(
        height >= HIERARCHY_ROW_HEIGHT * 0.5,
        "行高不得塌陷：{height}"
    );
    assert!(
        height >= 14.0,
        "行高必须容纳 18×18 行首控件与文本：{height}"
    );
}

#[test]
fn shift_range_select_covers_visible_range_and_falls_back_to_single() {
    let mut app = make_app(sample_document());
    app.select_single_hierarchy(SceneEntityId(2));
    app.shift_range_select(SceneEntityId(4));
    assert!(app.selection.contains(SceneEntityId(2)));
    assert!(app.selection.contains(SceneEntityId(3)));
    assert!(app.selection.contains(SceneEntityId(4)));
    assert_eq!(app.selection.primary(), Some(SceneEntityId(4)));

    // 锚点被折叠隐藏时退化为单选。
    app.selection
        .set_single(SceneEntityId(4), editor::SelectionSource::Hierarchy);
    app.scene_state.collapsed.insert(SceneEntityId(1));
    app.selection_anchor = Some(SceneEntityId(2));
    app.shift_range_select(SceneEntityId(1));
    assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
    assert_eq!(app.selection.len(), 1);
}

#[test]
fn top_most_selected_ids_drop_descendants_of_selected_ancestors() {
    let mut app = make_app(sample_document());
    app.selection.set_many(
        [SceneEntityId(1), SceneEntityId(2)],
        None,
        editor::SelectionSource::Command,
    );
    assert_eq!(app.top_most_selected_ids(), vec![SceneEntityId(1)]);
}

#[test]
fn hierarchy_collapsed_state_roundtrips_per_scene_and_drops_stale_ids() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("rustlua-hierarchy-persist-{token}"));
    let mut document = sample_document();
    document.path = Some(root.join("scenes/main.scene"));
    let mut app = make_app(document);

    app.scene_state
        .collapsed
        .extend([SceneEntityId(1), SceneEntityId(2), SceneEntityId(99)]);
    app.save_hierarchy_collapsed();
    app.scene_state.collapsed.clear();
    app.scene_state.collapsed.insert(SceneEntityId(99));
    app.load_hierarchy_collapsed();

    assert_eq!(
        app.scene_state.collapsed,
        std::collections::BTreeSet::from([SceneEntityId(1), SceneEntityId(2)])
    );
}

/// 多选 Ctrl+D：复制**整个选中集合的顶层根**，复制体各随其源、整体成为新选中集合，
/// 且一步 Undo 全部回滚（Unity 的 Ctrl+D 语义）。
#[test]
fn multi_selection_duplicate_copies_every_top_level_entity_in_one_undo() {
    let mut app = make_app(sample_document());
    // A（Root 下）与 Solo（根下）分属两个父级：批量复制不能只挑一个。
    app.selection.set_many(
        [SceneEntityId(2), SceneEntityId(4)],
        Some(SceneEntityId(4)),
        editor::SelectionSource::Hierarchy,
    );
    app.duplicate_selected_entities();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    // Root -> [A, A_copy, Leaf]；Solo -> [Solo, Solo_copy]。
    assert_eq!(
        scene.entities[&SceneEntityId(1)].children,
        vec![SceneEntityId(2), SceneEntityId(5), SceneEntityId(3)]
    );
    assert_eq!(scene.entities[&SceneEntityId(5)].name, "A");
    assert_eq!(
        scene.entities[&SceneEntityId(5)].parent,
        Some(SceneEntityId(1))
    );
    assert_eq!(scene.entities[&SceneEntityId(6)].name, "Solo");
    assert_eq!(scene.entities[&SceneEntityId(6)].parent, None);
    // 复制体成为新的选中集合（继续按 Ctrl+D 会复制复制体，与 Unity 一致）。
    assert_eq!(app.selection.len(), 2);
    assert!(app.selection.contains(SceneEntityId(5)));
    assert!(app.selection.contains(SceneEntityId(6)));
    assert_eq!(app.selection.primary(), Some(SceneEntityId(6)));

    app.undo_with_transaction_guard();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert_eq!(
        scene.entities[&SceneEntityId(1)].children,
        vec![SceneEntityId(2), SceneEntityId(3)]
    );
    assert!(!scene.entities.contains_key(&SceneEntityId(5)));
    assert!(!scene.entities.contains_key(&SceneEntityId(6)));
}

/// 多选右键「移动到此实体下」/「移动到根节点」：整批移动，一条 Undo。
/// （以前这里只搬 `primary`，多选后与拖拽的行为不一致。）
#[test]
fn multi_selection_reparent_moves_the_whole_selection_in_one_undo() {
    let mut app = make_app(sample_document());
    app.selection.set_many(
        [SceneEntityId(2), SceneEntityId(4)],
        Some(SceneEntityId(4)),
        editor::SelectionSource::Hierarchy,
    );
    app.reparent_selected(Some(SceneEntityId(3)));
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert_eq!(
        scene.entities[&SceneEntityId(2)].parent,
        Some(SceneEntityId(3))
    );
    assert_eq!(
        scene.entities[&SceneEntityId(4)].parent,
        Some(SceneEntityId(3))
    );
    assert!(
        !scene.entities[&SceneEntityId(1)]
            .children
            .contains(&SceneEntityId(2))
    );

    app.undo_with_transaction_guard();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert_eq!(
        scene.entities[&SceneEntityId(2)].parent,
        Some(SceneEntityId(1))
    );
    assert_eq!(scene.entities[&SceneEntityId(4)].parent, None);
}

/// 多选「移动到根节点」：两个原本在不同父级下的实体都被搬到根下。
#[test]
fn multi_selection_move_to_root_uses_the_batch_path() {
    let mut app = make_app(sample_document());
    app.selection.set_many(
        [SceneEntityId(2), SceneEntityId(3)],
        Some(SceneEntityId(3)),
        editor::SelectionSource::Hierarchy,
    );
    app.reparent_selected(None);
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert_eq!(scene.entities[&SceneEntityId(2)].parent, None);
    assert_eq!(scene.entities[&SceneEntityId(3)].parent, None);
    assert!(scene.entities[&SceneEntityId(1)].children.is_empty());
}

#[test]
fn duplicate_in_place_sits_right_after_source_and_undo_restores() {
    let mut app = make_app(sample_document());
    app.selection
        .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
    app.duplicate_selected_entities();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    // 复制体紧随源节点之后：Root -> [A, A, Leaf]，Solo 保持在后。
    assert_eq!(
        scene.entities[&SceneEntityId(1)].children,
        vec![SceneEntityId(2), SceneEntityId(5), SceneEntityId(3)]
    );
    assert_eq!(scene.entities[&SceneEntityId(5)].name, "A");
    assert_eq!(
        scene.entities[&SceneEntityId(5)].parent,
        Some(SceneEntityId(1))
    );

    app.undo_with_transaction_guard();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert_eq!(
        scene.entities[&SceneEntityId(1)].children,
        vec![SceneEntityId(2), SceneEntityId(3)]
    );
    assert!(!scene.entities.contains_key(&SceneEntityId(5)));
}

#[test]
fn inline_rename_rejects_empty_and_commits_valid_name() {
    let mut app = make_app(sample_document());
    app.select_single_hierarchy(SceneEntityId(2));
    app.begin_hierarchy_rename(SceneEntityId(2));

    // 空名：会话保留待重试，名称不变、不产生历史。
    app.hierarchy_rename.as_mut().unwrap().buffer = "   ".to_owned();
    app.commit_hierarchy_rename(SceneEntityId(2));
    assert!(app.hierarchy_rename.is_some());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.entities[&SceneEntityId(2)].name,
        "A"
    );

    app.hierarchy_rename.as_mut().unwrap().buffer = "Hero".to_owned();
    app.commit_hierarchy_rename(SceneEntityId(2));
    assert!(app.hierarchy_rename.is_none());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.entities[&SceneEntityId(2)].name,
        "Hero"
    );
}

#[test]
fn batch_delete_removes_selected_roots_in_one_undo_step() {
    let mut app = make_app(sample_document());
    app.selection.set_many(
        [SceneEntityId(2), SceneEntityId(4)],
        Some(SceneEntityId(2)),
        editor::SelectionSource::Hierarchy,
    );
    app.delete_selected_entity();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert!(!scene.entities.contains_key(&SceneEntityId(2)));
    assert!(!scene.entities.contains_key(&SceneEntityId(4)));
    assert_eq!(
        scene.entities[&SceneEntityId(1)].children,
        vec![SceneEntityId(3)]
    );

    app.undo_with_transaction_guard();
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert!(scene.entities.contains_key(&SceneEntityId(2)));
    assert!(scene.entities.contains_key(&SceneEntityId(4)));
}
